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
use pw_fs::limits::READ_ONLY_MAX;
use pw_rpc::RpcError;

use super::{
    BlameChunk, BlameCommit, BlameEngine, BlameFile, BlamePrevious, BlameRange, Chunker, Subject,
    internal, path_text,
};
use crate::log::commit::{self, MAX_IDENT, Mailmap, Text, clip};

/// gix-blame 에 주는 객체 저장소. 멈춤 신호가 켜지면 객체 읽기가 실패하고, 얕은 클론의 경계 커밋은 부모 없이(`parent` 줄을
/// 걷어) 보인다 — git 처럼 그 커밋에서 blame 이 멈춘다(gix-blame 은 얕은 클론을 몰라 없는 부모를 읽다 실패한다).
/// [`READ_ONLY_MAX`] 를 넘는 blob 은 읽지 않고 실패하며 `too_large` 를 켠다([`text_cache`]).
pub(super) struct Objects<'a, O> {
    pub(super) odb: &'a O,
    pub(super) stop: &'a AtomicBool,
    pub(super) shallow: &'a HashSet<ObjectId>,
    pub(super) too_large: &'a AtomicBool,
}

impl<O> Objects<'_, O> {
    fn refuse_large(&self, kind: gix::object::Kind, size: u64) -> Result<(), gix::Error> {
        if kind == gix::object::Kind::Blob && size > READ_ONLY_MAX {
            self.too_large.store(true, Ordering::Relaxed);
            return Err(gix::Error::from_error(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                "blob larger than the blame limit",
            )));
        }
        Ok(())
    }
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
            let found = self.odb.try_find(id, buffer)?;
            if let Some(found) = &found {
                self.refuse_large(found.kind, found.data.len() as u64)?;
            }
            return Ok(found);
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
        let header = self.odb.try_header(id)?;
        if let Some(header) = &header {
            self.refuse_large(header.kind, header.size)?;
        }
        Ok(header)
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
    let mut cache = text_cache(repo)?;
    let options = gix::blame::Options {
        diff_algorithm: repo.diff_algorithm().map_err(internal)?,
        ranges: gix::blame::BlameRanges::default(),
        since: None,
        rewrites: Some(gix::diff::Rewrites::default()),
        debug_track_path: false,
    };
    let shallow = shallow(repo)?;
    let too_large = AtomicBool::new(false);
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
            too_large: &too_large,
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
    if too_large.load(Ordering::Relaxed) {
        return Err(RpcError::invalid_params(format!(
            "a version of `{}` (or a file its rename is matched against) is larger than {READ_ONLY_MAX} bytes; blame reads at most that much",
            subject.path
        )));
    }
    let outcome = outcome.map_err(|e| RpcError::internal(format!("blame failed: {e}")))?;
    // git 은 `blame.showRoot` 가 없으면 첫 커밋도 경계로 보인다.
    let show_root = repo.config_snapshot().boolean("blame.showRoot") == Some(true);
    let mailmap = Mailmap::always(repo, trusted);
    let mut chunker = Chunker::new(BlameFile {
        commit: subject.commit.to_string(),
        lines: subject.lines,
        engine: BlameEngine::Gix,
        moves: false,
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
                let found =
                    previous_of(repo, &mut cache, &shallow, entry.commit_id, key.1.as_bstr())?;
                previous.insert(key.clone(), found.clone());
                found
            }
        };
        chunker.range(BlameRange {
            line: entry.start_in_blamed_file + 1,
            count: entry.len.get(),
            commit: hash,
            source_path: path_text(key.1.as_bstr())?,
            source_line: entry.start_in_source_file + 1,
            previous: before,
        });
        if chunker.due() && !emit(chunker.take()) {
            return Ok(false);
        }
    }
    Ok(chunker.finish().is_none_or(emit))
}

/// blame 이 견주는 리소스 캐시. 모든 경로를 텍스트로 본다 — git blame 은 바이너리(앞 8000바이트의 NUL) · `-diff` · `binary` 속성을
/// 보지 않고 커밋의 바이트를 줄로 견준다. gix 의 tree diff 용 캐시는 그런 파일을 빈 데이터로 주어, gix-blame 이 「바뀐 것 없음」
/// 으로 보고 줄을 조용히 부모로 넘긴다(debug 빌드는 gix-blame 의 줄 대조가 패닉한다). 그래서 저장소의 속성을 읽지 않고 모든
/// 경로에 「바이너리 아님 · textconv 없음」 드라이버를 준다. 내용은 저장소 그대로(`Mode::ToGit` — 필터 · textconv 가 돌지 않는다).
/// 크기 기준은 [`READ_ONLY_MAX`] 다(텍스트와 같은 상한). 그보다 큰 버전은 빈 데이터가 되므로 [`Objects`] 가 그 blob 을 읽지 않고
/// 실패해 blame 이 invalid params 로 끝난다 — 조용히 다른 결과를 내지 않는다.
fn text_cache(repo: &gix::Repository) -> Result<gix::diff::blob::Platform, RpcError> {
    use gix::diff::blob::{Driver, Pipeline, Platform, pipeline, platform};
    use gix::worktree::stack::{State, state};

    const DRIVER: &str = "pitwall-blame-text";
    let mut collection = gix::attrs::search::MetadataCollection::default();
    let mut globals = gix::attrs::Search::default();
    globals
        .add_patterns_buffer(
            format!("* diff={DRIVER}").as_bytes(),
            "[pitwall-blame]".into(),
            None,
            &mut collection,
            false,
        )
        .map_err(internal)?;
    // `IdMapping` 에 매핑이 없으니 저장소의 `.gitattributes` 를 읽지 않는다. 경로는 쓰이지 않는다(작업 트리를 보지 않는다).
    let attributes = gix::worktree::Stack::new(
        repo.git_dir(),
        State::AttributesStack(state::Attributes::new(
            globals,
            None,
            state::attributes::Source::IdMapping,
            collection,
        )),
        gix::glob::pattern::Case::Sensitive,
        Vec::new(),
        Vec::new(),
    );
    let filter = gix::filter::plumbing::Pipeline::new(
        repo.command_context().map_err(internal)?,
        repo.object_hash(),
        gix::filter::Pipeline::options(repo).map_err(internal)?,
    );
    Ok(Platform::new(
        platform::Options {
            algorithm: Some(repo.diff_algorithm().map_err(internal)?),
            skip_internal_diff_if_external_is_configured: false,
        },
        Pipeline::new(
            pipeline::WorktreeRoots::default(),
            filter,
            vec![Driver {
                name: DRIVER.into(),
                is_binary: Some(false),
                ..Driver::default()
            }],
            pipeline::Options {
                large_file_threshold_bytes: READ_ONLY_MAX,
                fs: gix::fs::Capabilities::default(),
            },
        ),
        pipeline::Mode::ToGit,
        attributes,
    ))
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
    cache: &mut gix::diff::blob::Platform,
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
            rename_source(repo, cache, &parent_tree, &tree, path)?
        };
        if let Some(origin) = origin {
            return Ok(Some(BlamePrevious {
                commit: parent.to_string(),
                path: path_text(origin.as_bstr())?,
            }));
        }
    }
    Ok(None)
}

/// `old` → `new` 에서 `path` 로 이름이 바뀐 파일의 원래 경로. 비슷함은 blame 과 같은 [`text_cache`] 로 잰다 — gix 의 기본 캐시는
/// 바이너리 파일을 빈 데이터로 보아 이름 바뀜을 찾지 못한다(git 은 찾는다).
fn rename_source(
    repo: &gix::Repository,
    cache: &mut gix::diff::blob::Platform,
    old: &gix::Tree<'_>,
    new: &gix::Tree<'_>,
    path: &BStr,
) -> Result<Option<BString>, RpcError> {
    use gix::diff::tree_with_rewrites::{ChangeRef, Options};

    let mut found = None;
    gix::diff::tree_with_rewrites(
        gix::objs::TreeRefIter::from_bytes(&old.data, old.id.kind()),
        gix::objs::TreeRefIter::from_bytes(&new.data, new.id.kind()),
        cache,
        &mut gix::diff::tree::State::default(),
        &repo.objects,
        |change| {
            if let ChangeRef::Rewrite {
                source_location,
                location,
                copy: false,
                ..
            } = change
                && location == path
            {
                found = Some(source_location.to_owned());
                return Ok(std::ops::ControlFlow::Break(()));
            }
            Ok(std::ops::ControlFlow::Continue(()))
        },
        Options {
            location: Some(gix::diff::tree::recorder::Location::Path),
            rewrites: Some(gix::diff::Rewrites::default()),
        },
    )
    .map_err(internal)?;
    Ok(found)
}
