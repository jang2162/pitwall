//! `git.blob`: 리비전(또는 index)의 파일 내용을 `fs.read` 처럼 전체 · 범위로 읽는다.
//!
//! gix 에는 객체를 스트림으로 읽는 길이 없어 blob 은 통째로 푼 뒤 범위를 자른다. 그래서 대용량 기준(50 MiB,
//! `pw_fs::limits::READ_ONLY_MAX`)을 넘는 blob 은 객체 헤더로 크기만 알리고 풀지 않는다(`tooLarge` — 범위 읽기도 하지 않는다,
//! 13-m2-plan M2-04 메모). 편집 상한(5 MiB)을 넘는 blob 은 Host 전체에서 [`BIG_BLOBS_AT_ONCE`] 개씩만 함께 푼다 — 메모리가
//! 50 MiB × 요청 수로 늘지 않게(`pw-fs` 검색의 큰 파일 상한과 같은 방식). `git.diff` 의 큰 파일도 같은 차례를 쓴다.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use gix::bstr::ByteSlice;
use gix::objs::tree::EntryKind;
use pw_fs::judge::{Bom, judge};
use pw_fs::limits::{EDITABLE_MAX, READ_ONLY_MAX, SNIFF_BYTES};
use pw_rpc::RpcError;

use super::methods::{GitBlob, GitBlobParams};

/// 5 MiB 넘는 blob 을 함께 푸는 수의 상한.
pub const BIG_BLOBS_AT_ONCE: usize = 2;

/// 차례를 기다리는 최대 시간. 넘으면 「바쁨」 오류.
#[cfg(not(test))]
pub(crate) const BIG_BLOB_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
pub(crate) const BIG_BLOB_WAIT: Duration = Duration::from_secs(1);

pub(crate) static BIG_BLOBS: Gate = Gate::new(BIG_BLOBS_AT_ONCE);

pub fn blob(repo: &gix::Repository, params: &GitBlobParams) -> Result<GitBlob, RpcError> {
    let path = params.path.as_str();
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return Err(RpcError::invalid_params(format!(
            "`{path}` must be a path relative to the repository top, separated by `/`"
        )));
    }
    let (id, kind) = match &params.rev {
        Some(rev) => {
            let commit = super::commit(repo, rev)?;
            let tree = commit.tree().map_err(internal)?;
            let entry = tree
                .lookup_entry_by_path(path)
                .map_err(internal)?
                .ok_or_else(|| not_found(path, rev))?;
            (entry.object_id(), entry.mode().kind())
        }
        None => {
            let index = repo.index_or_empty().map_err(internal)?;
            let entry = index
                .entry_by_path(path.as_bytes().as_bstr())
                .filter(|e| e.stage_raw() == 0)
                .ok_or_else(|| not_found(path, "the index"))?;
            let kind = match entry.mode {
                gix::index::entry::Mode::FILE => EntryKind::Blob,
                gix::index::entry::Mode::FILE_EXECUTABLE => EntryKind::BlobExecutable,
                gix::index::entry::Mode::SYMLINK => EntryKind::Link,
                gix::index::entry::Mode::COMMIT => EntryKind::Commit,
                _ => EntryKind::Tree,
            };
            (entry.id, kind)
        }
    };
    let mode = match kind {
        EntryKind::Blob => "100644",
        EntryKind::BlobExecutable => "100755",
        EntryKind::Link => "120000",
        EntryKind::Tree => {
            return Err(RpcError::invalid_params(format!("`{path}` is a folder")));
        }
        EntryKind::Commit => {
            return Err(RpcError::invalid_params(format!("`{path}` is a submodule")));
        }
    };
    if params.length.is_some_and(|l| l > READ_ONLY_MAX) {
        return Err(RpcError::invalid_params(format!(
            "a range read is at most {READ_ONLY_MAX} bytes"
        )));
    }
    let size = repo.find_header(id).map_err(internal)?.size();
    let offset = params.offset.unwrap_or(0);
    if size > READ_ONLY_MAX {
        return Ok(GitBlob {
            id: id.to_string(),
            mode: mode.to_owned(),
            size,
            too_large: true,
            offset,
            eof: false,
            content: None,
            judgment: None,
        });
    }
    let _seat = if size > EDITABLE_MAX {
        Some(BIG_BLOBS.enter(BIG_BLOB_WAIT).ok_or_else(|| {
            RpcError::internal(format!(
                "git.blob is busy: {BIG_BLOBS_AT_ONCE} large blobs are already being read"
            ))
        })?)
    } else {
        None
    };
    let object = repo.find_object(id).map_err(internal)?;
    let data = &object.data;
    let start = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(data.len());
    let end = match params.length {
        Some(length) => start
            .saturating_add(usize::try_from(length).unwrap_or(usize::MAX))
            .min(data.len()),
        None => data.len(),
    };
    let range = &data[start..end];
    let head = &data[..data.len().min(SNIFF_BYTES)];
    let bom = if start == 0 {
        Bom::detect(head).map_or(0, Bom::byte_len).min(range.len())
    } else {
        0
    };
    let eof = end == data.len();
    let judgment = judge(head, &range[bom..], size, !eof);
    Ok(GitBlob {
        id: id.to_string(),
        mode: mode.to_owned(),
        size,
        too_large: false,
        offset: start as u64,
        eof,
        content: Some(pw_fs::content(params.bytes, &judgment, range, bom)),
        judgment: Some(judgment),
    })
}

fn not_found(path: &str, at: &str) -> RpcError {
    RpcError::new(pw_rpc::code::NOT_FOUND, format!("`{path}` is not in {at}"))
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

/// 블로킹 스레드의 자리 수 상한.
pub(crate) struct Gate {
    used: Mutex<usize>,
    freed: Condvar,
    max: usize,
}

impl Gate {
    const fn new(max: usize) -> Self {
        Self {
            used: Mutex::new(0),
            freed: Condvar::new(),
            max,
        }
    }

    /// 자리를 얻는다. `wait` 안에 못 얻으면 `None`.
    pub(crate) fn enter(&self, wait: Duration) -> Option<Seat<'_>> {
        let used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut used, timeout) = self
            .freed
            .wait_timeout_while(used, wait, |used| *used >= self.max)
            .unwrap_or_else(PoisonError::into_inner);
        if timeout.timed_out() && *used >= self.max {
            return None;
        }
        *used += 1;
        Some(Seat(self))
    }
}

pub(crate) struct Seat<'a>(&'a Gate);

impl Drop for Seat<'_> {
    fn drop(&mut self) {
        *self.0.used.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.freed.notify_one();
    }
}
