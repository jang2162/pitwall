//! `git.blob`: 리비전(또는 index)의 파일 내용을 `fs.read` 처럼 전체 · 범위로 읽는다.
//!
//! gix 에는 객체를 스트림으로 읽는 길이 없어 blob 은 통째로 푼 뒤 범위를 자른다. 그래서 대용량 기준(50 MiB,
//! `pw_fs::limits::READ_ONLY_MAX`)을 넘는 blob 은 객체 헤더로 크기만 알리고 풀지 않는다(`tooLarge` — 범위 읽기도 하지 않는다,
//! 13-m2-plan M2-04 메모). 편집 상한(5 MiB)을 넘는 blob 은 Host 전체에서 [`BIG_BLOBS_AT_ONCE`] 개씩만 함께 푼다 — 메모리가
//! 50 MiB × 요청 수로 늘지 않게(`pw-fs` 검색의 큰 파일 상한과 같은 방식). `git.diff` 의 큰 파일도 같은 차례를 쓴다.
//!
//! `workTreeForm`(에디터 거터의 기준, 13-m2-plan M2-17)은 푼 blob 을 `git checkout` 의 내장 변환으로 작업 트리 꼴로 바꾼다
//! ([`work_tree_form`]). 거터는 파일을 열 때마다 저절로 부르므로 외부 필터 드라이버는 신뢰한 프로젝트에서도 돌리지 않는다.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use gix::bstr::{BStr, ByteSlice};
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
    if params.work_tree_form && (params.offset.is_some() || params.length.is_some()) {
        return Err(RpcError::invalid_params(
            "`workTreeForm` reads the whole blob: drop `offset` and `length`",
        ));
    }
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
            filter: None,
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
    let converted;
    let data: &[u8] = if params.work_tree_form && kind != EntryKind::Link {
        match work_tree_form(repo, path, &object.data)? {
            Form::Bytes(bytes) => {
                converted = bytes;
                &converted
            }
            Form::Filtered(driver) => {
                return Ok(GitBlob {
                    id: id.to_string(),
                    mode: mode.to_owned(),
                    size,
                    too_large: false,
                    offset: 0,
                    eof: true,
                    content: None,
                    judgment: None,
                    filter: Some(driver),
                });
            }
        }
    } else {
        &object.data
    };
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
        filter: None,
    })
}

/// [`work_tree_form`] 의 결과.
enum Form {
    /// 작업 트리 꼴의 바이트.
    Bytes(Vec<u8>),
    /// 그 경로에 외부 필터 드라이버가 걸려 있다(드라이버 이름).
    Filtered(String),
}

/// blob 을 `git checkout` 이 쓰는 꼴로 바꾼다: `ident` → 줄바꿈(`core.autocrlf` · `core.eol` · `text` · `eol` 속성) →
/// `working-tree-encoding`(gix 의 필터 파이프라인 — 속성은 작업 트리의 `.gitattributes`, 맨 저장소면 HEAD 의 것).
///
/// 외부 드라이버(`filter.<x>.smudge` · `process`)는 돌리지 않는다. 그 경로의 `filter` 속성이 무엇이든 가리키면 바꾸지 않고
/// [`Form::Filtered`] 다 — 드라이버가 어느 설정에 있든(전역 · 저장소 설정 — 미신뢰는 저장소 설정을 읽지 않는다) 없든, 신뢰 ·
/// 미신뢰 같다(13-m2-plan M2-17 리뷰 F4). 드라이버 정의를 찾아 판정하면 미신뢰 저장소의 설정에만 있는 드라이버를 놓쳐 smudge
/// 전의 내용을 기준으로 삼는다. 변환은 드라이버를 모두 뗀 핸들([`super::changes::without_filter_drivers`])로 해서 프로세스가
/// 뜨지 않는다.
fn work_tree_form(repo: &gix::Repository, path: &str, data: &[u8]) -> Result<Form, RpcError> {
    let rela = path.as_bytes().as_bstr();
    let (pipeline, _index) = repo.filter_pipeline(None).map_err(internal)?;
    let (_, mut attributes) = pipeline.into_parts();
    let mut found =
        attributes.selected_attribute_matches(["filter", "ident", "working-tree-encoding"]);
    attributes
        .at_entry(rela, None, &repo.objects)
        .map_err(internal)?
        .matching_attributes(&mut found);
    let mut ident = false;
    let mut filter = None;
    let mut encoding = None;
    for m in found.iter_selected() {
        let value = match m.assignment.state {
            gix::attrs::StateRef::Value(v) => Some(v.as_bstr().to_owned()),
            _ => None,
        };
        match m.assignment.name.as_str() {
            "ident" => ident = m.assignment.state.is_set(),
            "working-tree-encoding" => encoding = value,
            "filter" => filter = value,
            _ => {}
        }
    }
    if let Some(name) = filter {
        return Ok(Form::Filtered(name.to_str_lossy().into_owned()));
    }
    if let Some(label) = encoding {
        reproducible_encoding(path, label.as_ref())?;
    }
    let mut plain = repo.clone();
    super::changes::without_filter_drivers(&mut plain)?;
    let (mut pipeline, _index) = plain.filter_pipeline(None).map_err(internal)?;
    use gix::filter::plumbing::pipeline::convert::{ToWorktreeOutcome, to_worktree};
    let outcome = pipeline
        .convert_to_worktree(
            data,
            rela,
            to_worktree::Options {
                can_delay: gix::filter::plumbing::driver::apply::Delay::Forbid,
                unknown_encoding: to_worktree::UnknownEncoding::Fail,
            },
        )
        .map_err(internal)?;
    let bytes = match outcome {
        ToWorktreeOutcome::Unchanged(bytes) | ToWorktreeOutcome::Buffer(bytes) => bytes.to_vec(),
        ToWorktreeOutcome::Process(_) => {
            return Err(RpcError::internal(format!(
                "`{path}` went through a filter driver although none is configured"
            )));
        }
    };
    Ok(Form::Bytes(if ident {
        git_ident(
            bytes,
            &gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Blob, data)
                .map_err(internal)?
                .to_string(),
        )
    } else {
        bytes
    }))
}

/// `working-tree-encoding` 의 인코딩을 git 과 같은 바이트로 쓸 수 있는가. gix 는 encoding_rs 로 쓰는데, encoding_rs 는 WHATWG
/// 규칙대로 UTF-16 계열을 UTF-8 로 쓴다(`output_encoding`) — 그 파일은 git 과 다른 바이트가 된다. 이름을 모르는 인코딩(git 은
/// iconv 로 읽는 `UTF-16LE-BOM` · `UTF-32` 등)도 쓸 수 없다. 둘 다 틀린 기준 대신 invalid params 다.
fn reproducible_encoding(path: &str, label: &BStr) -> Result<(), RpcError> {
    match encoding_rs::Encoding::for_label(label) {
        Some(encoding) if encoding.output_encoding() == encoding => Ok(()),
        _ => Err(RpcError::invalid_params(format!(
            "`{path}` has working-tree-encoding `{label}`, which cannot be written like git does"
        ))),
    }
}

/// gix(gix-filter 0.36)의 `ident` 는 `$Id: <해시>$` 로 펼치고 git 은 `$Id: <해시> $` 로 펼친다(convert.c `ident_to_worktree`).
/// git 은 `$Id$` · `$Id: …$` 를 모두 그 꼴로 바꾸므로, gix 가 펼친 자리(`$Id: <이 blob 의 해시>$`)는 모두 git 이 펼친 자리다.
fn git_ident(bytes: Vec<u8>, hash: &str) -> Vec<u8> {
    let theirs = format!("$Id: {hash}$");
    let ours = format!("$Id: {hash} $");
    if bytes.find(theirs.as_bytes()).is_none() {
        return bytes;
    }
    bytes.replace(theirs.as_bytes(), ours.as_bytes())
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
