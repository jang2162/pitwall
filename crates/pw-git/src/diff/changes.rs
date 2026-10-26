//! `git.diff`: 두 쪽의 바뀐 파일 목록과 파일별 hunk.
//!
//! - 커밋 ↔ 커밋 · 커밋 ↔ index 는 커밋의 트리를 index 꼴로 펴서 두 index 를 견준다(gix 의 `diff::index`, `git status` 의 HEAD ↔
//!   index 와 같은 길). 이름 바뀜은 gix 의 추적기로 찾는다 — `git diff --find-renames` 와 같은 기준(내용 50% 이상, 복사는 찾지 않음,
//!   후보 1000개 상한). 비율 계산은 git 과 달라 경계에 가까운 짝은 갈릴 수 있다.
//! - index ↔ 작업 트리는 gix status 의 index ↔ 작업 트리 판정(stat · 내용 해시)으로 바뀐 항목을 찾는다. untracked 와 이름 바뀜은
//!   보지 않는다(`git diff` 와 같다). 서브모듈은 gix status 에 맡기지 않고(그 저장소를 저장소 설정째 연다) 이 크레이트의 열기
//!   경로(신뢰 그대로)로 서브모듈의 HEAD 만 읽는다 — 서브모듈 안의 변경(`-dirty`)은 보이지 않는다.
//! - 내용은 저장소에 담긴 꼴(`ToGit` — 작업 트리 파일은 git 처럼 clean · process 필터 · 줄바꿈 변환을 거친다)로 견준다. textconv ·
//!   외부 diff 는 신뢰해도 쓰지 않는다. 미신뢰 프로젝트는 필터 드라이버를 모두 떼고 읽는다 — 저장소 설정의 것은 열 때부터 고르지
//!   않고(`crate::repo::open_options`), 사람의 전역 · 시스템 설정의 것(`filter.lfs.*` 등)은 [`without_filter_drivers`] 가 뗀다.
//!   git CLI 는 쓰지 않는다.
//! - 작업 트리 쪽 파일은 읽기 전에 M1-01 의 규칙 A(실제 경로가 작업 트리 안 — `pw_fs::scope`)를 거친다. 없는 쪽(지워진 파일의
//!   새 쪽)은 디스크를 읽지 않는 빈 쪽이다. 한계: 판정과 gix 의 읽기 사이에 다른 로컬 프로세스가 경로의 폴더를 루트 밖 링크로
//!   바꾸면 그 파일을 읽는다(`pw_fs::scope` 「쓰기」 한계와 같은 경합).
//! - 대용량 기준(`pw_fs::limits::READ_ONLY_MAX`)을 넘는 쪽은 객체 헤더 · 파일 크기만 보고 내용을 풀지 않는다(`tooLarge`). 편집
//!   상한(`EDITABLE_MAX`)을 넘는 쪽이 있는 파일은 `git.blob` 과 같은 차례([`super::blob::BIG_BLOBS`])를 얻어 diff 하고, 끝나면 그
//!   메모리를 놓는다. 한계: 이름 바뀜 추적이 견주는 더한 · 지운 파일의 내용은 이 차례 없이 푼다(gix 의 추적기 안).
//! - hunk 는 응답 크기의 상한을 둔다: 파일 하나 [`FILE_HUNKS_MAX`](넘으면 `tooLarge`), 응답 전체 [`DIFF_HUNKS_MAX`](닿은 뒤의
//!   파일은 `hunksOmitted` — `paths` 로 그 파일만 다시 부르면 받는다). 크기는 [`super::hunks::size`] 로 센다.
//! - 파일 하나의 내용을 읽지 못하면 그 파일에 `error` 를 달고 나머지는 낸다.

use std::path::{Path, PathBuf};
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
use pw_fs::limits::{EDITABLE_MAX, READ_ONLY_MAX};
use pw_fs::scope::Roots;
use pw_lang::attributes::RepoAttributes;
use pw_lang::{HEAD_BYTES, generated};
use pw_rpc::RpcError;

use super::blob::{BIG_BLOB_WAIT, BIG_BLOBS, BIG_BLOBS_AT_ONCE};
use super::hunks;
use super::methods::{DiffFile, DiffSide, FileStatus, GitDiff, GitDiffParams, SubmodulePointer};
use crate::Opened;

/// 파일 하나의 hunk 가 응답에서 차지할 수 있는 크기([`super::hunks::size`]). 편집 상한(결정 M1-D9)과 같다 — 넘으면 `tooLarge`.
pub const FILE_HUNKS_MAX: u64 = EDITABLE_MAX;

/// 응답 하나의 hunk 크기 합의 상한. 대용량 기준(결정 M1-D9)과 같다 — 닿은 뒤의 파일은 `hunksOmitted`.
pub const DIFF_HUNKS_MAX: u64 = READ_ONLY_MAX;

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
    let worktree = matches!(pair, Pair::IndexWorkTree).then(|| WorkTree::new(work_tree));
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
        Pair::IndexWorkTree => worktree_changes(opened, &filter, worktree.as_ref())?,
    };
    cache.clear_resource_cache_keep_allocation();
    entries.sort_by(|a, b| a.path().cmp(b.path()));

    let attributes = RepoAttributes::load(
        work_tree,
        Some(repo.common_dir()),
        entries.iter().map(|e| parent_dir(e.path())),
    );
    let mut files = Vec::with_capacity(entries.len());
    let mut budget = DIFF_HUNKS_MAX;
    for entry in entries {
        let file = file(
            &mut cache,
            &objects,
            &attributes,
            worktree.as_ref(),
            entry,
            params.ignore_whitespace,
            &mut budget,
        );
        cache.clear_resource_cache_keep_allocation();
        // `git diff -w` 는 공백만 바뀐 파일을 목록에서도 뺀다(이름 · 모드가 바뀐 것은 남긴다).
        if params.ignore_whitespace
            && file.status == FileStatus::Modified
            && file.old_mode == file.new_mode
            && file.added == 0
            && file.removed == 0
            && file.error.is_none()
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

/// 미신뢰 프로젝트의 저장소에서 필터 드라이버(`filter.<x>.clean` · `smudge` · `process`)를 모두 뗀다 — 저장소 설정의 것뿐 아니라
/// 사람의 전역 · 시스템 설정의 것도(git lfs 가 `~/.gitconfig` 에 쓰는 `filter.lfs.*` 등). 어느 드라이버를 돌릴지는 저장소의
/// `.gitattributes` 가 고르므로, 미신뢰 저장소가 프로세스를 띄우지 못하려면 드라이버 자체가 없어야 한다(결정 M2-D8,
/// 03-architecture.md 「보안」). 요청마다 새로 연 핸들의 메모리 안 설정만 바꾼다(파일은 그대로다). 드라이버가 없는 `filter`
/// 속성은 git 처럼 무시된다 — LFS 파일은 포인터 그대로 견준다. diff 파이프라인과 작업 트리 판정(gix status) 둘 다 이 설정에서
/// 드라이버를 고른다.
pub fn without_filter_drivers(repo: &mut gix::Repository) -> Result<(), RpcError> {
    let mut config = repo.config_snapshot_mut();
    let ids: Vec<_> = config
        .sections_and_ids_by_name("filter")
        .into_iter()
        .flatten()
        .map(|(_, id)| id)
        .collect();
    for id in ids {
        config.remove_section_by_id(id);
    }
    config.commit().map_err(internal)?;
    Ok(())
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
            let parent = match super::parent_ids(repo, &commit)?.into_iter().next() {
                Some(parent) => {
                    let parent = repo.find_commit(parent).map_err(internal)?;
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
fn worktree_changes(
    opened: &Opened,
    filter: &PathFilter,
    worktree: Option<&WorkTree>,
) -> Result<Vec<Entry>, RpcError> {
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
    let Some(worktree) = worktree else {
        return Ok(out);
    };
    for (path, id) in submodules {
        let old = Some(Side {
            path: path.clone(),
            mode: IndexMode::COMMIT,
            id: Some(id),
        });
        // git 처럼: 그 자리가 없거나 든 폴더가 링크면 지움, 파일 · 링크면 종류 바뀜, 폴더면 체크아웃된 서브모듈의 HEAD 를 견준다
        // (체크아웃되지 않은 빈 폴더 · 저장소가 아닌 폴더는 바뀌지 않은 것).
        let place = worktree.check(path.as_ref(), IndexMode::SYMLINK).ok();
        let at = place
            .as_ref()
            .and_then(|p| std::fs::symlink_metadata(p).ok())
            .zip(place);
        let entry = match at {
            None => Entry {
                status: FileStatus::Deleted,
                old,
                new: None,
                mode_only: false,
            },
            Some((meta, _)) if !meta.is_dir() => Entry {
                status: FileStatus::TypeChanged,
                old,
                new: Some(Side {
                    path,
                    mode: file_mode(&meta),
                    id: None,
                }),
                mode_only: false,
            },
            Some((_, place)) => match submodule_head(opened, &place) {
                Some(head) if head != id => Entry {
                    status: FileStatus::Modified,
                    old,
                    new: Some(Side {
                        path,
                        mode: IndexMode::COMMIT,
                        id: Some(head),
                    }),
                    mode_only: false,
                },
                _ => continue,
            },
        };
        out.push(entry);
    }
    Ok(out)
}

/// 작업 트리 항목의 git 모드(심링크 · 실행 파일 · 파일).
fn file_mode(meta: &std::fs::Metadata) -> IndexMode {
    if meta.file_type().is_symlink() {
        return IndexMode::SYMLINK;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return IndexMode::FILE_EXECUTABLE;
        }
    }
    IndexMode::FILE
}

/// 체크아웃된 서브모듈의 HEAD. `dir` 은 작업 트리 안의 폴더다(링크가 아니고 든 폴더가 규칙 A 를 지났다). 체크아웃되지
/// 않았거나(빈 폴더) 그 자리가 서브모듈 저장소가 아니면 `None` — 그때는 git 처럼 바뀐 것으로 보지 않는다(index 의 커밋과 같다고
/// 본다).
fn submodule_head(opened: &Opened, dir: &Path) -> Option<ObjectId> {
    let repo = crate::repo::discover(dir, opened.trusted).ok()?.ok()?;
    let found = repo.workdir()?;
    if std::fs::canonicalize(found).ok()? != std::fs::canonicalize(dir).ok()? {
        return None;
    }
    repo.head_id().ok().map(|id| id.detach())
}

/// 작업 트리 쪽 파일을 읽기 전의 판정(M1-01 의 규칙 A — `pw_fs::scope` 의 읽기 판정).
pub(crate) struct WorkTree {
    dir: PathBuf,
    roots: Roots,
}

impl WorkTree {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            roots: Roots::new([dir.to_path_buf()]),
        }
    }

    /// `rel` 의 실제 경로가 작업 트리 안이면 그 경로. 심링크는 링크 자체(대상 글자)를 읽으므로 든 폴더만 본다 — 중간 폴더가 밖을
    /// 가리키는 링크면 거절한다.
    pub(crate) fn check(&self, rel: &BStr, mode: IndexMode) -> Result<PathBuf, String> {
        let path = self.dir.join(
            gix::path::from_bstr(rel).map_err(|e| format!("cannot use the path `{rel}`: {e}"))?,
        );
        let judged = if mode == IndexMode::SYMLINK {
            path.parent().unwrap_or(&self.dir)
        } else {
            &path
        };
        self.roots.readable(judged).map_err(|_| {
            format!("`{rel}` goes through a symlink that points outside the work tree")
        })?;
        Ok(path)
    }
}

/// 항목 하나의 내용 diff 와 표시. 내용을 읽지 못하면 `error` 를 단다. `budget` 은 응답에 남은 hunk 크기다.
fn file(
    cache: &mut Platform,
    objects: &Counted<'_, gix::OdbHandle>,
    attributes: &RepoAttributes,
    worktree: Option<&WorkTree>,
    entry: Entry,
    ignore_whitespace: bool,
    budget: &mut u64,
) -> DiffFile {
    let path = entry.path().to_owned();
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
        hunks_omitted: false,
        generated: false,
        submodule: None,
        added: 0,
        removed: 0,
        hunks: Vec::new(),
        error: None,
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
        return out;
    }
    let blob_like = |s: &Option<Side>| s.as_ref().is_none_or(|s| kind(s.mode).is_some());
    if entry.mode_only || !blob_like(&entry.old) || !blob_like(&entry.new) {
        out.generated = generated_flag(attributes, path.as_ref(), None);
        return out;
    }
    let seat = match big(objects, worktree, &entry) {
        Ok(false) => None,
        Ok(true) => match BIG_BLOBS.enter(BIG_BLOB_WAIT) {
            Some(seat) => Some(seat),
            None => {
                out.error = Some(format!(
                    "git.diff is busy: {BIG_BLOBS_AT_ONCE} large files are already being read"
                ));
                out.generated = generated_flag(attributes, path.as_ref(), None);
                return out;
            }
        },
        Err(e) => {
            out.error = Some(e);
            out.generated = generated_flag(attributes, path.as_ref(), None);
            return out;
        }
    };
    if let Err(e) = contents(
        cache,
        objects,
        attributes,
        worktree,
        &entry,
        ignore_whitespace,
        budget,
        &mut out,
    ) {
        out.error = Some(e);
        out.generated = generated_flag(attributes, path.as_ref(), None);
    }
    if seat.is_some() {
        // 큰 내용을 다음 파일 · 요청까지 들고 있지 않는다 — 차례를 놓기 전에 버퍼째 놓는다.
        cache.clear_resource_cache();
    }
    out
}

/// 한쪽이라도 편집 상한을 넘고 대용량 기준 이하다(내용을 풀어 diff 한다). 작업 트리 쪽은 규칙 A 를 먼저 거친다.
fn big(
    objects: &Counted<'_, gix::OdbHandle>,
    worktree: Option<&WorkTree>,
    entry: &Entry,
) -> Result<bool, String> {
    use gix::objs::FindHeader;
    let mut big = false;
    for side in [&entry.old, &entry.new].into_iter().flatten() {
        let size = match (side.id, worktree) {
            (Some(id), _) => objects
                .try_header(&id)
                .map_err(|e| format!("cannot read `{}`: {e}", side.path))?
                .map_or(0, |h| h.size),
            (None, Some(worktree)) => {
                let path = worktree.check(side.path.as_ref(), side.mode)?;
                std::fs::symlink_metadata(&path).map_or(0, |m| m.len())
            }
            (None, None) => 0,
        };
        big |= size > EDITABLE_MAX && size <= READ_ONLY_MAX;
    }
    Ok(big)
}

/// 두 쪽의 내용을 읽어 `out` 의 바이너리 · 대용량 · hunk · 줄 수 · 생성 표시를 채운다.
#[allow(clippy::too_many_arguments)] // 한 파일의 diff 에 드는 것을 그대로 받는다 — 묶어도 이 함수 하나만 쓴다.
fn contents(
    cache: &mut Platform,
    objects: &Counted<'_, gix::OdbHandle>,
    attributes: &RepoAttributes,
    worktree: Option<&WorkTree>,
    entry: &Entry,
    ignore_whitespace: bool,
    budget: &mut u64,
    out: &mut DiffFile,
) -> Result<(), String> {
    let path = entry.path();
    let hash = objects.hash;
    // 없는 쪽은 있는 쪽의 종류로 둔다.
    let fallback = entry
        .old
        .as_ref()
        .or(entry.new.as_ref())
        .and_then(|s| kind(s.mode))
        .unwrap_or(EntryKind::Blob);
    let set = |cache: &mut Platform, side: &Option<Side>, which: ResourceKind| {
        let result = match side {
            Some(s) => {
                if s.id.is_none()
                    && let Some(worktree) = worktree
                {
                    worktree.check(s.path.as_ref(), s.mode)?;
                }
                cache.set_resource(
                    s.id.unwrap_or_else(|| ObjectId::null(hash)),
                    kind(s.mode).unwrap_or(fallback),
                    s.path.as_ref(),
                    which,
                    objects,
                )
            }
            // 없는 쪽은 빈 쪽이다. null id 는 작업 트리 루트가 있으면 「디스크의 그 경로를 읽어라」 라서(지운 파일의 자리에 다른 것 —
            // 루트 밖을 가리키는 폴더 링크 — 이 있을 수 있다) 루트를 떼고 넣는다.
            None => {
                let roots = std::mem::take(&mut cache.filter.roots);
                let result =
                    cache.set_resource(ObjectId::null(hash), fallback, path, which, objects);
                cache.filter.roots = roots;
                result
            }
        };
        result.map_err(|e| format!("cannot read `{path}`: {e}"))
    };
    set(cache, &entry.old, ResourceKind::OldOrSource)?;
    set(cache, &entry.new, ResourceKind::NewOrDestination)?;
    let prepared = cache
        .prepare_diff()
        .map_err(|e| format!("cannot diff `{path}`: {e}"))?;
    let algorithm = match prepared.operation {
        Operation::SourceOrDestinationIsBinary => {
            let large = |d: gix::diff::blob::platform::resource::Data<'_>| matches!(d, gix::diff::blob::platform::resource::Data::Binary { size } if size > READ_ONLY_MAX);
            out.too_large = large(prepared.old.data) || large(prepared.new.data);
            out.binary = !out.too_large;
            out.generated = generated_flag(attributes, path, None);
            return Ok(());
        }
        Operation::InternalDiff { algorithm } => algorithm,
        // 외부 diff 는 고르지 않는다(`skip_internal_diff_if_external_is_configured` 가 꺼져 있다).
        Operation::ExternalCommand { .. } => gix::diff::blob::Algorithm::Myers,
    };
    let old = prepared.old.data.as_slice().unwrap_or_default();
    let new = prepared.new.data.as_slice().unwrap_or_default();
    let lines = hunks::hunks(
        old,
        new,
        algorithm,
        ignore_whitespace,
        FILE_HUNKS_MAX.min(*budget),
    );
    out.added = lines.added;
    out.removed = lines.removed;
    match lines.hunks {
        Some(hunks) => {
            *budget -= lines.size;
            out.hunks = hunks;
        }
        None if lines.size > FILE_HUNKS_MAX => out.too_large = true,
        None => out.hunks_omitted = true,
    }
    let head = if entry.new.is_some() { new } else { old };
    out.generated = generated_flag(attributes, path, Some(head));
    Ok(())
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
