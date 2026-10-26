//! `git.diff`: 두 쪽의 바뀐 파일 목록과 파일별 hunk.
//!
//! - 커밋 ↔ 커밋 · 커밋 ↔ index 는 커밋의 트리를 index 꼴로 펴서 두 index 를 견준다(gix 의 `diff::index`, `git status` 의 HEAD ↔
//!   index 와 같은 길). 이름 바뀜은 gix 의 추적기로 찾는다 — `git diff --find-renames` 와 같은 기준(내용 50% 이상, 복사는 찾지 않음,
//!   후보 1000개 상한). 비율 계산은 git 과 달라 경계에 가까운 짝은 갈릴 수 있다.
//! - index ↔ 작업 트리는 gix status 의 index ↔ 작업 트리 판정(stat · 내용 해시)으로 바뀐 항목을 찾는다. untracked 와 이름 바뀜은
//!   보지 않는다(`git diff` 와 같다). 서브모듈은 gix status 에 맡기지 않고(그 저장소를 저장소 설정째 연다) 이 크레이트의 열기
//!   경로(신뢰 그대로)로 서브모듈의 HEAD 만 읽는다 — 서브모듈 안의 변경(`-dirty`)은 보이지 않는다.
//! - 내용은 저장소에 담긴 꼴(`ToGit` — 작업 트리 파일은 git 처럼 clean 필터 · 줄바꿈 변환을 거친다)로 견준다. textconv 는 쓰지
//!   않는다. 미신뢰 프로젝트의 저장소는 저장소 설정의 드라이버를 고르지 않게 열려 있으므로(`crate::repo::open_options`) 필터 ·
//!   textconv · 외부 diff 프로그램이 돌지 않는다. git CLI 는 쓰지 않는다.
//! - 대용량 기준(`pw_fs::limits::READ_ONLY_MAX`)을 넘는 쪽은 객체 헤더 · 파일 크기만 보고 내용을 풀지 않는다(`tooLarge`).

use std::sync::atomic::{AtomicU64, Ordering};

use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use gix::diff::blob::pipeline::{Mode, WorktreeRoots};
use gix::diff::blob::platform::prepare_diff::Operation;
use gix::diff::blob::{Platform, ResourceKind};
use gix::index::entry::Mode as IndexMode;
use gix::objs::tree::EntryKind;
use gix::status::index_worktree::Item;
use gix::status::plumbing::index_as_worktree::{Change as WorktreeChange, EntryStatus};
use pw_fs::limits::READ_ONLY_MAX;
use pw_lang::attributes::RepoAttributes;
use pw_lang::{HEAD_BYTES, generated};
use pw_rpc::RpcError;

use super::hunks;
use super::methods::{DiffFile, DiffSide, FileStatus, GitDiff, GitDiffParams, SubmodulePointer};
use crate::Opened;

/// diff 를 만드는 동안 객체 저장소에서 풀어 낸 바이트(헤더만 본 것은 세지 않는다). 시험이 대용량 파일을 풀지 않았는지 본다.
#[derive(Debug, Default)]
pub struct Stats {
    pub object_bytes: AtomicU64,
}

/// 객체 저장소를 감싸 풀어 낸 바이트를 센다.
struct Counted<'a, T> {
    inner: &'a T,
    stats: &'a Stats,
    /// 저장소의 해시 종류(작업 트리 · 없는 쪽을 나타내는 null id).
    hash: gix::hash::Kind,
}

impl<T: gix::objs::Find> gix::objs::Find for Counted<'_, T> {
    fn try_find<'a>(
        &self,
        id: &gix::oid,
        buffer: &'a mut Vec<u8>,
    ) -> gix::Result<Option<gix::objs::Data<'a>>> {
        let found = self.inner.try_find(id, buffer)?;
        if let Some(data) = &found {
            self.stats
                .object_bytes
                .fetch_add(data.data.len() as u64, Ordering::Relaxed);
        }
        Ok(found)
    }
}

impl<T: gix::objs::FindHeader> gix::objs::FindHeader for Counted<'_, T> {
    fn try_header(&self, id: &gix::oid) -> gix::Result<Option<gix::objs::Header>> {
        self.inner.try_header(id)
    }
}

/// diff 의 한쪽에서 본 항목 하나.
#[derive(Debug, Clone)]
struct Side {
    path: BString,
    mode: IndexMode,
    /// 작업 트리 쪽이면 `None`.
    id: Option<ObjectId>,
}

#[derive(Debug, Clone)]
struct Entry {
    status: FileStatus,
    old: Option<Side>,
    new: Option<Side>,
    /// 작업 트리 쪽이 실행 비트만 바뀌었다 — 내용을 다시 읽지 않는다.
    mode_only: bool,
}

impl Entry {
    /// 목록의 자리(새 경로, 지워졌으면 옛 경로).
    fn path(&self) -> &BStr {
        self.new
            .as_ref()
            .or(self.old.as_ref())
            .map(|s| s.path.as_ref())
            .unwrap_or_default()
    }
}

/// 고른 두 쪽.
enum Pair {
    /// 옛 트리(없으면 빈 트리) ↔ 새 트리.
    Trees {
        old: Option<(ObjectId, ObjectId)>,
        new: (ObjectId, ObjectId),
    },
    /// 옛 트리(없으면 빈 트리) ↔ index.
    TreeIndex {
        old: Option<(ObjectId, ObjectId)>,
    },
    IndexWorkTree,
}

/// `git.diff` 를 연 저장소에서.
pub fn diff(opened: &Opened, params: &GitDiffParams, stats: &Stats) -> Result<GitDiff, RpcError> {
    let repo = &opened.repo;
    let pair = pair(repo, params)?;
    let filter = PathFilter::new(params.paths.as_deref())?;
    let (old_commit, new_commit) = match &pair {
        Pair::Trees { old, new } => (old.map(|(c, _)| c), Some(new.0)),
        Pair::TreeIndex { old } => (old.map(|(c, _)| c), None),
        Pair::IndexWorkTree => (None, None),
    };
    let objects = Counted {
        inner: &repo.objects,
        stats,
        hash: repo.object_hash(),
    };
    let work_tree = &opened.target.work_tree;
    let roots = match pair {
        Pair::IndexWorkTree => WorktreeRoots {
            old_root: None,
            new_root: Some(work_tree.clone()),
        },
        _ => WorktreeRoots::default(),
    };
    let mut cache = repo
        .diff_resource_cache(Mode::ToGit, roots)
        .map_err(internal)?;
    cache.filter.options.large_file_threshold_bytes = READ_ONLY_MAX;
    let mut entries = match pair {
        Pair::Trees { old, new } => {
            let lhs = tree_state(repo, old.map(|(_, t)| t))?;
            let rhs = tree_state(repo, Some(new.1))?;
            index_changes(repo, lhs, rhs, &filter, &mut cache, &objects)?
        }
        Pair::TreeIndex { old } => {
            let lhs = tree_state(repo, old.map(|(_, t)| t))?;
            let index = repo.index_or_empty().map_err(internal)?;
            let rhs = gix::index::State::clone(&index);
            index_changes(repo, lhs, rhs, &filter, &mut cache, &objects)?
        }
        Pair::IndexWorkTree => worktree_changes(opened, &filter)?,
    };
    cache.clear_resource_cache_keep_allocation();
    entries.sort_by(|a, b| a.path().cmp(b.path()));

    let attributes = RepoAttributes::load(
        work_tree,
        Some(repo.common_dir()),
        entries.iter().map(|e| parent_dir(e.path())),
    );
    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        let file = file(
            &mut cache,
            &objects,
            &attributes,
            entry,
            params.ignore_whitespace,
        )?;
        cache.clear_resource_cache_keep_allocation();
        // `git diff -w` 는 공백만 바뀐 파일을 목록에서도 뺀다(이름 · 모드가 바뀐 것은 남긴다).
        if params.ignore_whitespace
            && file.status == FileStatus::Modified
            && file.old_mode == file.new_mode
            && file.hunks.is_empty()
            && !file.binary
            && !file.too_large
            && file.submodule.is_none()
        {
            continue;
        }
        files.push(file);
    }
    Ok(GitDiff {
        work_tree: pw_core::registry::path::normalize(work_tree)
            .unwrap_or_else(|_| work_tree.clone())
            .display()
            .to_string(),
        old_commit: old_commit.map(|c| c.to_string()),
        new_commit: new_commit.map(|c| c.to_string()),
        files,
    })
}

/// 요청의 두 쪽을 고른다. 커밋은 `(커밋, 트리)` 다.
fn pair(repo: &gix::Repository, params: &GitDiffParams) -> Result<Pair, RpcError> {
    let invalid = || {
        RpcError::invalid_params(
            "unsupported diff sides: use commit↔commit (or a commit and its first parent), commit↔index, or index↔workTree",
        )
    };
    match (&params.from, &params.to) {
        (None, DiffSide::Commit { rev }) => {
            let commit = super::commit(repo, rev)?;
            let tree = tree_of(&commit)?;
            let parent = match commit.parent_ids().next() {
                Some(parent) => {
                    let parent = parent.object().map_err(internal)?.into_commit();
                    Some((parent.id, tree_of(&parent)?))
                }
                None => None,
            };
            Ok(Pair::Trees {
                old: parent,
                new: (commit.id, tree),
            })
        }
        (Some(DiffSide::Commit { rev: from }), DiffSide::Commit { rev: to }) => {
            let old = super::commit(repo, from)?;
            let new = super::commit(repo, to)?;
            Ok(Pair::Trees {
                old: Some((old.id, tree_of(&old)?)),
                new: (new.id, tree_of(&new)?),
            })
        }
        (None, DiffSide::Index) => {
            // 커밋이 없는 저장소(태어나지 않은 HEAD)는 빈 트리와 견준다.
            let head = repo.head_commit().ok();
            let old = match head {
                Some(commit) => Some((commit.id, tree_of(&commit)?)),
                None => None,
            };
            Ok(Pair::TreeIndex { old })
        }
        (Some(DiffSide::Commit { rev }), DiffSide::Index) => {
            let commit = super::commit(repo, rev)?;
            Ok(Pair::TreeIndex {
                old: Some((commit.id, tree_of(&commit)?)),
            })
        }
        (None | Some(DiffSide::Index), DiffSide::WorkTree) => Ok(Pair::IndexWorkTree),
        _ => Err(invalid()),
    }
}

fn tree_of(commit: &gix::Commit<'_>) -> Result<ObjectId, RpcError> {
    commit.tree_id().map(|id| id.detach()).map_err(internal)
}

/// 트리를 index 꼴로 편다. `None` 이면 빈 트리.
fn tree_state(
    repo: &gix::Repository,
    tree: Option<ObjectId>,
) -> Result<gix::index::State, RpcError> {
    match tree {
        Some(tree) => Ok(repo.index_from_tree(&tree).map_err(internal)?.into()),
        None => Ok(gix::index::State::new(repo.object_hash())),
    }
}

/// 두 index 꼴의 차이(이름 바뀜 포함). 거르기는 이름 바뀜을 찾기 전에 한다.
fn index_changes(
    repo: &gix::Repository,
    mut lhs: gix::index::State,
    mut rhs: gix::index::State,
    filter: &PathFilter,
    cache: &mut Platform,
    objects: &Counted<'_, gix::OdbHandle>,
) -> Result<Vec<Entry>, RpcError> {
    use gix::diff::index::{ChangeRef, RewriteOptions};
    // 충돌 중인 경로(단계 1~3)는 빼고 본다 — gix 는 왼쪽에 충돌 항목을 받지 않고 오른쪽의 것은 건너뛴다.
    lhs.remove_entries(|_, path, e| !filter.matches(path) || e.stage_raw() != 0);
    rhs.remove_entries(|_, path, _| !filter.matches(path));
    let work_tree = repo.workdir().unwrap_or_else(|| repo.git_dir());
    let mut search = gix::pathspec::Search::from_specs(None, None, work_tree).map_err(internal)?;
    let mut out = Vec::new();
    gix::diff::index(
        &lhs,
        &rhs,
        |change| -> gix::Result<_> {
            let side = |path: &BStr, mode: IndexMode, id: &gix::oid| Side {
                path: path.to_owned(),
                mode,
                id: Some(id.to_owned()),
            };
            let entry = match change {
                ChangeRef::Addition {
                    location,
                    entry_mode,
                    id,
                    ..
                } => Entry {
                    status: FileStatus::Added,
                    old: None,
                    new: Some(side(&location, entry_mode, &id)),
                    mode_only: false,
                },
                ChangeRef::Deletion {
                    location,
                    entry_mode,
                    id,
                    ..
                } => Entry {
                    status: FileStatus::Deleted,
                    old: Some(side(&location, entry_mode, &id)),
                    new: None,
                    mode_only: false,
                },
                ChangeRef::Modification {
                    location,
                    previous_entry_mode,
                    previous_id,
                    entry_mode,
                    id,
                    ..
                } => Entry {
                    status: if class(previous_entry_mode) == class(entry_mode) {
                        FileStatus::Modified
                    } else {
                        FileStatus::TypeChanged
                    },
                    old: Some(side(&location, previous_entry_mode, &previous_id)),
                    new: Some(side(&location, entry_mode, &id)),
                    mode_only: false,
                },
                ChangeRef::Rewrite {
                    source_location,
                    source_entry_mode,
                    source_id,
                    location,
                    entry_mode,
                    id,
                    ..
                } => Entry {
                    status: FileStatus::Renamed,
                    old: Some(side(&source_location, source_entry_mode, &source_id)),
                    new: Some(side(&location, entry_mode, &id)),
                    mode_only: false,
                },
            };
            out.push(entry);
            Ok(std::ops::ControlFlow::Continue(()))
        },
        Some(RewriteOptions {
            resource_cache: cache,
            find: objects,
            rewrites: gix::diff::Rewrites::default(),
        }),
        &mut search,
        &mut |_, _, _, _| false,
    )
    .map_err(internal)?;
    Ok(out)
}

/// index ↔ 작업 트리의 바뀐 항목.
fn worktree_changes(opened: &Opened, filter: &PathFilter) -> Result<Vec<Entry>, RpcError> {
    use gix::status::{Submodule, UntrackedFiles};
    let repo = &opened.repo;
    let index = repo.index_or_empty().map_err(internal)?;
    let mut state = gix::index::State::clone(&index);
    state.remove_entries(|_, path, _| !filter.matches(path));
    let submodules: Vec<(BString, ObjectId)> = state
        .entries()
        .iter()
        .filter(|e| e.mode == IndexMode::COMMIT && e.stage_raw() == 0)
        .map(|e| (BString::from(e.path(&state).to_vec()), e.id))
        .collect();
    let index = gix::worktree::IndexPersistedOrInMemory::InMemory(gix::index::File::from_state(
        state,
        repo.index_path(),
    ));
    let items = repo
        .status(gix::progress::Discard)
        .map_err(internal)?
        .untracked_files(UntrackedFiles::None)
        .index_worktree_rewrites(None)
        .index_worktree_submodules(Submodule::Given {
            ignore: gix::submodule::config::Ignore::All,
            check_dirty: false,
        })
        .index(index)
        .into_index_worktree_iter(Vec::<BString>::new())
        .map_err(internal)?;
    let mut out = Vec::new();
    for item in items {
        let Item::Modification {
            entry,
            rela_path,
            status,
            ..
        } = item.map_err(internal)?
        else {
            continue;
        };
        if entry.mode == IndexMode::COMMIT {
            continue;
        }
        let old = Side {
            path: rela_path.clone(),
            mode: entry.mode,
            id: Some(entry.id),
        };
        let worktree = |mode: IndexMode| Side {
            path: rela_path.clone(),
            mode,
            id: None,
        };
        let change = match status {
            EntryStatus::Change(WorktreeChange::Removed) => Entry {
                status: FileStatus::Deleted,
                old: Some(old),
                new: None,
                mode_only: false,
            },
            EntryStatus::Change(WorktreeChange::Type { worktree_mode }) => Entry {
                status: FileStatus::TypeChanged,
                old: Some(old),
                new: Some(worktree(worktree_mode)),
                mode_only: false,
            },
            EntryStatus::Change(WorktreeChange::Modification {
                executable_bit_changed,
                content_change,
                ..
            }) => {
                let mode = if !executable_bit_changed {
                    entry.mode
                } else if entry.mode == IndexMode::FILE_EXECUTABLE {
                    IndexMode::FILE
                } else {
                    IndexMode::FILE_EXECUTABLE
                };
                Entry {
                    status: FileStatus::Modified,
                    old: Some(old),
                    new: Some(worktree(mode)),
                    mode_only: content_change.is_none(),
                }
            }
            // `git add --intent-to-add`: index 에는 빈 자리만 있다 — 새 파일로 보인다(`git diff` 와 같다).
            EntryStatus::IntentToAdd => Entry {
                status: FileStatus::Added,
                old: None,
                new: Some(worktree(entry.mode)),
                mode_only: false,
            },
            EntryStatus::Change(WorktreeChange::SubmoduleModification(_))
            | EntryStatus::NeedsUpdate(_)
            | EntryStatus::Conflict { .. } => continue,
        };
        out.push(change);
    }
    for (path, id) in submodules {
        let head = submodule_head(opened, path.as_ref());
        if head != Some(id) {
            out.push(Entry {
                status: FileStatus::Modified,
                old: Some(Side {
                    path: path.clone(),
                    mode: IndexMode::COMMIT,
                    id: Some(id),
                }),
                new: Some(Side {
                    path,
                    mode: IndexMode::COMMIT,
                    id: head,
                }),
                mode_only: false,
            });
        }
    }
    Ok(out)
}

/// 체크아웃된 서브모듈의 HEAD. 체크아웃되지 않았거나(빈 폴더) 그 자리가 서브모듈 저장소가 아니면 `None` — 그때는 git 처럼 바뀐
/// 것으로 보지 않는다(index 의 커밋과 같다고 본다).
fn submodule_head(opened: &Opened, path: &BStr) -> Option<ObjectId> {
    let dir = opened
        .target
        .work_tree
        .join(gix::path::from_bstr(path).ok()?);
    let repo = crate::repo::discover(&dir, opened.trusted).ok()?.ok()?;
    let found = repo.workdir()?;
    if std::fs::canonicalize(found).ok()? != std::fs::canonicalize(&dir).ok()? {
        return None;
    }
    repo.head_id().ok().map(|id| id.detach())
}

/// 항목 하나의 내용 diff 와 표시.
fn file(
    cache: &mut Platform,
    objects: &Counted<'_, gix::OdbHandle>,
    attributes: &RepoAttributes,
    entry: Entry,
    ignore_whitespace: bool,
) -> Result<DiffFile, RpcError> {
    let path = entry.path().to_owned();
    let hash = objects.hash;
    let mut out = DiffFile {
        status: entry.status,
        path: path.to_str_lossy().into_owned(),
        old_path: match entry.status {
            FileStatus::Renamed => entry
                .old
                .as_ref()
                .map(|s| s.path.to_str_lossy().into_owned()),
            _ => None,
        },
        old_mode: entry.old.as_ref().map(|s| mode(s.mode)),
        new_mode: entry.new.as_ref().map(|s| mode(s.mode)),
        old_id: entry
            .old
            .as_ref()
            .and_then(|s| s.id)
            .map(|id| id.to_string()),
        new_id: entry
            .new
            .as_ref()
            .and_then(|s| s.id)
            .map(|id| id.to_string()),
        binary: false,
        too_large: false,
        generated: false,
        submodule: None,
        added: 0,
        removed: 0,
        hunks: Vec::new(),
    };
    let is_commit = |s: &Option<Side>| s.as_ref().is_some_and(|s| s.mode == IndexMode::COMMIT);
    if is_commit(&entry.old) || is_commit(&entry.new) {
        out.submodule = Some(SubmodulePointer {
            old: entry
                .old
                .as_ref()
                .filter(|s| s.mode == IndexMode::COMMIT)
                .and_then(|s| s.id)
                .map(|id| id.to_string()),
            new: entry
                .new
                .as_ref()
                .filter(|s| s.mode == IndexMode::COMMIT)
                .and_then(|s| s.id)
                .map(|id| id.to_string()),
        });
        out.generated = generated_flag(attributes, path.as_ref(), None);
        return Ok(out);
    }
    let blob_like = |s: &Option<Side>| s.as_ref().is_none_or(|s| kind(s.mode).is_some());
    if entry.mode_only || !blob_like(&entry.old) || !blob_like(&entry.new) {
        out.generated = generated_flag(attributes, path.as_ref(), None);
        return Ok(out);
    }
    // 없는 쪽은 있는 쪽의 종류로 둔다(null id · 없는 작업 트리 파일 = 빈 쪽).
    let fallback = entry
        .old
        .as_ref()
        .or(entry.new.as_ref())
        .and_then(|s| kind(s.mode))
        .unwrap_or(EntryKind::Blob);
    let set = |cache: &mut Platform, side: &Option<Side>, which: ResourceKind| {
        let (id, kind, path) = match side {
            Some(s) => (
                s.id.unwrap_or_else(|| ObjectId::null(hash)),
                kind(s.mode).unwrap_or(fallback),
                s.path.as_ref(),
            ),
            None => (ObjectId::null(hash), fallback, path.as_ref()),
        };
        cache
            .set_resource(id, kind, path, which, objects)
            .map_err(|e| RpcError::internal(format!("cannot read `{path}`: {e}")))
    };
    set(cache, &entry.old, ResourceKind::OldOrSource)?;
    set(cache, &entry.new, ResourceKind::NewOrDestination)?;
    let prepared = cache
        .prepare_diff()
        .map_err(|e| RpcError::internal(format!("cannot diff `{path}`: {e}")))?;
    let algorithm = match prepared.operation {
        Operation::SourceOrDestinationIsBinary => {
            let large = |d: gix::diff::blob::platform::resource::Data<'_>| matches!(d, gix::diff::blob::platform::resource::Data::Binary { size } if size > READ_ONLY_MAX);
            out.too_large = large(prepared.old.data) || large(prepared.new.data);
            out.binary = !out.too_large;
            out.generated = generated_flag(attributes, path.as_ref(), None);
            return Ok(out);
        }
        Operation::InternalDiff { algorithm } => algorithm,
        // 외부 diff 는 고르지 않는다(`skip_internal_diff_if_external_is_configured` 가 꺼져 있다).
        Operation::ExternalCommand { .. } => gix::diff::blob::Algorithm::Myers,
    };
    let old = prepared.old.data.as_slice().unwrap_or_default();
    let new = prepared.new.data.as_slice().unwrap_or_default();
    out.hunks = hunks::hunks(old, new, algorithm, ignore_whitespace);
    for line in out.hunks.iter().flat_map(|h| &h.lines) {
        match line.kind {
            super::methods::LineKind::Added => out.added += 1,
            super::methods::LineKind::Removed => out.removed += 1,
            super::methods::LineKind::Context => {}
        }
    }
    let head = if entry.new.is_some() { new } else { old };
    out.generated = generated_flag(attributes, path.as_ref(), Some(head));
    Ok(out)
}

/// `linguist-generated` 속성, 없으면 Linguist 규칙(경로 · 앞부분). `head` 가 없으면(바이너리 · 대용량) 경로 규칙만.
fn generated_flag(attributes: &RepoAttributes, path: &BStr, head: Option<&[u8]>) -> bool {
    let path = path.to_str_lossy();
    if let Some(flag) = attributes.get(&path).generated {
        return flag;
    }
    let head = head.map(|h| String::from_utf8_lossy(&h[..h.len().min(HEAD_BYTES)]));
    generated::generated(&path, head.as_deref()).is_some()
}

/// 종류(blob · 심링크 · 서브모듈). 종류가 바뀌면 `typeChanged` 다.
fn class(mode: IndexMode) -> u8 {
    match mode {
        IndexMode::SYMLINK => 1,
        IndexMode::COMMIT => 2,
        IndexMode::DIR => 3,
        _ => 0,
    }
}

/// diff 파이프라인이 받는 종류. 내용이 없는 것(서브모듈 · 폴더)은 `None`.
fn kind(mode: IndexMode) -> Option<EntryKind> {
    match mode {
        IndexMode::FILE => Some(EntryKind::Blob),
        IndexMode::FILE_EXECUTABLE => Some(EntryKind::BlobExecutable),
        IndexMode::SYMLINK => Some(EntryKind::Link),
        _ => None,
    }
}

fn mode(mode: IndexMode) -> String {
    format!("{:06o}", mode.bits())
}

fn parent_dir(path: &BStr) -> &str {
    let path = path.to_str().unwrap_or_default();
    path.rfind('/').map_or("", |i| &path[..i])
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

/// `paths` 거르기: 그 경로와 같거나 그 아래(`<경로>/…`)면 맞는다. 없으면 모두 맞는다.
pub struct PathFilter(Option<Vec<BString>>);

impl PathFilter {
    fn new(paths: Option<&[String]>) -> Result<Self, RpcError> {
        let Some(paths) = paths else {
            return Ok(Self(None));
        };
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            let trimmed = p.trim_end_matches('/');
            if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.contains('\\') {
                return Err(RpcError::invalid_params(format!(
                    "`{p}` must be a path relative to the repository top, separated by `/`"
                )));
            }
            out.push(BString::from(trimmed));
        }
        Ok(Self(Some(out)))
    }

    fn matches(&self, path: &BStr) -> bool {
        let Some(paths) = &self.0 else {
            return true;
        };
        paths.iter().any(|p| {
            path.starts_with(p.as_slice()) && (path.len() == p.len() || path[p.len()] == b'/')
        })
    }
}
