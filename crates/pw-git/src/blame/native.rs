//! gix blame(미신뢰 프로젝트 · git 을 찾지 못했을 때 — [`super`] 머리 주석).
//!
//! gix-blame 은 결과를 다 만든 뒤에 돌려준다 — 첫 청크가 전체 시간 뒤에 나간다. 멈춤은 객체 읽기마다 본다([`Objects`]): 멈춤
//! 신호가 켜지면 다음 객체 읽기가 실패해 blame 이 끝난다.
//!
//! 범위의 `previous` 는 git 과 같은 규칙으로 구한다: 커밋의 부모를 차례로 보며 그 부모에 같은 경로가 있으면 그것, 없으면 그
//! 부모와의 이름 바뀜(기본 50% 비슷함)의 원래 경로 — 처음 찾은 부모의 것이다. 얕은 클론의 경계 커밋은 부모가 없다.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::FindExt;
use pw_rpc::RpcError;

use super::{
    BlameChunk, BlameCommit, BlameEngine, BlameFile, BlamePrevious, BlameRange, Chunker, Subject,
    internal, path_text,
};
use crate::log::commit::{self, MAX_IDENT, Mailmap, Text, clip};

/// gix-blame 에 주는 객체 저장소. 멈춤 신호가 켜지면 객체 읽기가 실패하고, 얕은 클론의 경계 커밋은 부모 없이(`parent` 줄을
/// 걷어) 보인다 — git 처럼 그 커밋에서 blame 이 멈춘다(gix-blame 은 얕은 클론을 몰라 없는 부모를 읽다 실패한다).
pub(super) struct Objects<'a, O> {
    pub(super) odb: &'a O,
    pub(super) stop: &'a AtomicBool,
    pub(super) shallow: &'a HashSet<ObjectId>,
}

impl<O: gix::objs::Find> gix::objs::Find for Objects<'_, O> {
    fn try_find<'b>(
        &self,
        id: &gix::oid,
        buffer: &'b mut Vec<u8>,
    ) -> Result<Option<gix::objs::Data<'b>>, gix::Error> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(gix::Error::from_error(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "blame cancelled",
            )));
        }
        if !self.shallow.contains(id) {
            return self.odb.try_find(id, buffer);
        }
        let mut raw = Vec::new();
        let Some(found) = self.odb.try_find(id, &mut raw)? else {
            return Ok(None);
        };
        let (kind, object_hash) = (found.kind, found.object_hash);
        *buffer = if kind == gix::object::Kind::Commit {
            without_parents(found.data)
        } else {
            found.data.to_vec()
        };
        Ok(Some(gix::objs::Data {
            kind,
            object_hash,
            data: buffer,
        }))
    }
}

/// 커밋 객체의 머리(첫 빈 줄 앞)에서 `parent` 줄을 뺀다.
fn without_parents(commit: &[u8]) -> Vec<u8> {
    let head_end = commit.find(b"\n\n").map_or(commit.len(), |i| i + 1);
    let (head, rest) = commit.split_at(head_end);
    let mut out = Vec::with_capacity(commit.len());
    for line in head.split_inclusive(|&b| b == b'\n') {
        if !line.starts_with(b"parent ") {
            out.extend_from_slice(line);
        }
    }
    out.extend_from_slice(rest);
    out
}

impl<O: gix::objs::FindHeader> gix::objs::FindHeader for Objects<'_, O> {
    fn try_header(&self, id: &gix::oid) -> Result<Option<gix::objs::Header>, gix::Error> {
        self.odb.try_header(id)
    }
}

/// `subject` 를 blame 해 청크로 `emit` 한다(첫 청크에 [`BlameFile`], 그 뒤는 [`super::CHUNK`] 범위씩). `emit` 이 `false` 를
/// 돌려주면 멈추고 `Ok(false)`. `stop` 이 켜지면 `CANCELLED`. `trusted` 는 연 저장소의 신뢰다(mailmap 설정을 볼지).
pub fn run(
    repo: &gix::Repository,
    trusted: bool,
    subject: &Subject,
    stop: &AtomicBool,
    emit: &mut dyn FnMut(BlameChunk) -> bool,
) -> Result<bool, RpcError> {
    let mut cache = repo.diff_resource_cache_for_tree_diff().map_err(internal)?;
    let options = gix::blame::Options {
        diff_algorithm: repo.diff_algorithm().map_err(internal)?,
        ranges: gix::blame::BlameRanges::default(),
        since: None,
        rewrites: Some(gix::diff::Rewrites::default()),
        debug_track_path: false,
    };
    let shallow = shallow(repo)?;
    // commit-graph 의 부모는 걷을 수 없다 — 얕은 클론이면 쓰지 않는다(git 도 얕은 저장소에서는 commit-graph 를 쓰지 않는다).
    let graph = if shallow.is_empty() {
        repo.commit_graph_if_enabled().map_err(internal)?
    } else {
        None
    };
    let outcome = gix::blame::file(
        Objects {
            odb: &repo.objects,
            stop,
            shallow: &shallow,
        },
        gix::blame::Start::Commit(subject.commit),
        graph,
        &mut cache,
        subject.path.as_bstr(),
        options,
    );
    if stop.load(Ordering::Relaxed) {
        return Err(RpcError::cancelled());
    }
    let outcome = outcome.map_err(|e| RpcError::internal(format!("blame failed: {e}")))?;
    // git 은 `blame.showRoot` 가 없으면 첫 커밋도 경계로 보인다.
    let show_root = repo.config_snapshot().boolean("blame.showRoot") == Some(true);
    let mailmap = Mailmap::always(repo, trusted);
    let mut chunker = Chunker::new(BlameFile {
        commit: subject.commit.to_string(),
        lines: subject.lines,
        engine: BlameEngine::Gix,
    });
    let mut previous: HashMap<(ObjectId, BString), Option<BlamePrevious>> = HashMap::new();
    let mut buf = Vec::new();
    for entry in &outcome.entries {
        if stop.load(Ordering::Relaxed) {
            return Err(RpcError::cancelled());
        }
        let hash = entry.commit_id.to_string();
        if !chunker.knows(&hash) {
            let found = repo
                .objects
                .find_commit(&entry.commit_id, &mut buf)
                .map_err(internal)?;
            let text = Text::read(&found, &mailmap)?;
            chunker.commit(BlameCommit {
                hash: hash.clone(),
                author_name: clip(&text.author_name, MAX_IDENT),
                author_email: clip(&text.author_email, MAX_IDENT),
                author_time: text.author_time,
                subject: commit::subject(&found),
                boundary: shallow.contains(&entry.commit_id)
                    || (!show_root && found.parents().next().is_none()),
            });
        }
        let path = entry
            .source_file_name
            .clone()
            .unwrap_or_else(|| subject.path.clone());
        let key = (entry.commit_id, path);
        let before = match previous.get(&key) {
            Some(found) => found.clone(),
            None => {
                let found = previous_of(repo, &shallow, entry.commit_id, key.1.as_bstr())?;
                previous.insert(key.clone(), found.clone());
                found
            }
        };
        chunker.range(BlameRange {
            line: entry.start_in_blamed_file + 1,
            count: entry.len.get(),
            commit: hash,
            source_path: path_text(key.1.as_bstr()),
            source_line: entry.start_in_source_file + 1,
            previous: before,
        });
        if chunker.due() && !emit(chunker.take()) {
            return Ok(false);
        }
    }
    Ok(chunker.finish().is_none_or(emit))
}

/// 얕은 클론의 경계 커밋(`.git/shallow`).
fn shallow(repo: &gix::Repository) -> Result<HashSet<ObjectId>, RpcError> {
    Ok(repo
        .shallow_commits()
        .map_err(internal)?
        .map(|commits| commits.iter().copied().collect())
        .unwrap_or_default())
}

/// `commit` 의 `path` 의 「이 커밋 이전으로」(머리 주석).
fn previous_of(
    repo: &gix::Repository,
    shallow: &HashSet<ObjectId>,
    commit: ObjectId,
    path: &BStr,
) -> Result<Option<BlamePrevious>, RpcError> {
    if shallow.contains(&commit) {
        return Ok(None);
    }
    let found = repo.find_commit(commit).map_err(internal)?;
    let tree = found.tree().map_err(internal)?;
    for parent in found.parent_ids() {
        let parent_tree = repo
            .find_commit(parent)
            .map_err(internal)?
            .tree()
            .map_err(internal)?;
        let exists = parent_tree
            .lookup_entry_by_path(path.to_path_lossy())
            .map_err(internal)?
            .is_some_and(|e| e.mode().is_blob_or_symlink());
        let origin = if exists {
            Some(path.to_owned())
        } else {
            rename_source(repo, &parent_tree, &tree, path)?
        };
        if let Some(origin) = origin {
            return Ok(Some(BlamePrevious {
                commit: parent.to_string(),
                path: path_text(origin.as_bstr()),
            }));
        }
    }
    Ok(None)
}

/// `old` → `new` 에서 `path` 로 이름이 바뀐 파일의 원래 경로.
fn rename_source(
    repo: &gix::Repository,
    old: &gix::Tree<'_>,
    new: &gix::Tree<'_>,
    path: &BStr,
) -> Result<Option<BString>, RpcError> {
    let changes = repo
        .diff_tree_to_tree(
            old,
            new,
            gix::diff::Options::default().with_rewrites(Some(gix::diff::Rewrites::default())),
        )
        .map_err(internal)?;
    Ok(changes.into_iter().find_map(|change| match change {
        gix::object::tree::diff::ChangeDetached::Rewrite {
            source_location,
            location,
            copy: false,
            ..
        } if location == path => Some(source_location),
        _ => None,
    }))
}
