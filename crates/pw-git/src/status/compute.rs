//! 작업 트리 상태를 gix 로 계산한다 — `git status --porcelain=v2 --untracked-files=all --ignore-submodules=dirty` 와 같은 내용.
//!
//! - HEAD↔index(스테이징한 변경)는 경로 한정이어도 저장소 전체로 계산하고 결과만 거른다: 이름 바뀜은 지운 쪽과 더한 쪽을 짝지어
//!   찾으므로 한쪽만 보면 「더함」 으로 잘못 보인다. index 와 트리만 비교해 작업 트리를 읽지 않고, HEAD 트리 · index 가 그대로면
//!   앞 결과를 쓴다([`StagedCache`]).
//! - index↔작업 트리와 untracked 는 경로 한정이면 그 경로만 본다(pathspec, [`paths::pathspec`]). untracked 는 파일마다 낸다
//!   (`-uall` — 트리에 파일마다 색을 칠한다). 중첩 저장소는 git 처럼 그 폴더 하나(`dir/`)다.
//! - 서브모듈은 가리키는 커밋(포인터)만 본다(결정 M2-D3 — 서브모듈 안의 변경 · untracked 는 보지 않는다).
//! - index↔작업 트리의 이름 바뀜은 찾지 않는다(git 도 untracked 를 짝으로 쓰지 않는다).
//! - index 를 다시 쓰지 않는다(git 의 `GIT_OPTIONAL_LOCKS=0` 과 같다) — 사람의 저장소를 바꾸지 않는다. 시각만 바뀐 파일은 매번 내용을
//!   비교한다.
//!
//! 필터 드라이버(`filter.<x>.clean` · `process`)는 내용 비교에서 돈다 — 미신뢰 프로젝트에서는 어디에 정의된 것이든 돌리지 않는다
//! (`git.status` 가 연 저장소에서 `crate::diff::changes::without_filter_drivers` 로 뗀다, 결정 M2-D8).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use gix::bstr::BString;
use gix::status::index_worktree::Item as WorktreeItem;
use gix::status::plumbing::index_as_worktree::{Change, Conflict, EntryStatus};

use super::paths;
use super::{GitChange, GitConflict, GitStatusEntry};

/// 계산이 멈춘 이유.
#[derive(Debug)]
pub enum Stopped {
    /// 멈춤 깃발이 섰다(취소).
    Interrupted,
    Failed(String),
}

impl<E: std::fmt::Display> From<E> for Stopped {
    fn from(e: E) -> Self {
        Stopped::Failed(e.to_string())
    }
}

/// index 의 항목 수. 큰 저장소의 전체 status 를 작업(`Task`)으로 돌릴지 가른다. index 가 없으면 0.
pub fn index_entries(repo: &gix::Repository) -> Result<usize, String> {
    Ok(repo
        .index_or_empty()
        .map_err(|e| e.to_string())?
        .entries()
        .len())
}

/// HEAD↔index 변경 하나(스테이징한 변경).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub path: String,
    /// 이름 바뀜 · 복사의 원래 경로.
    pub orig: Option<String>,
    pub change: GitChange,
}

/// 저장소마다 마지막 HEAD↔index 결과. 작업 트리만 바뀌는 동안(편집 → `git.statusChanged { paths }` → 경로 한정 status)은 HEAD 트리와
/// index 가 그대로라 다시 계산하지 않는다 — 10만 항목에서 트리를 index 로 펼치는 데 50 ms 쯤 든다. 키는 (git 디렉터리, 연 때의 신뢰,
/// HEAD 트리, index 체크섬, 이름 바뀜 설정)이다. 체크섬을 쓰지 않는 index(`index.skipHash`)는 담지 않는다.
#[derive(Default)]
pub struct StagedCache {
    entries: Mutex<HashMap<RepoKey, Cached>>,
}

/// 담은 결과와 그 키.
type Cached = (StagedKey, Arc<Vec<Staged>>);

/// (git 디렉터리, 연 때의 신뢰).
type RepoKey = (PathBuf, bool);
/// (HEAD 트리, index 체크섬, 이름 바뀜 설정([`rename_config`])).
type StagedKey = (gix::ObjectId, gix::ObjectId, Vec<Option<BString>>);

/// 담아 두는 저장소 수. 넘으면 비운다.
const CACHED_REPOSITORIES: usize = 16;

impl StagedCache {
    fn get(&self, repo: &RepoKey, key: &StagedKey) -> Option<Arc<Vec<Staged>>> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries
            .get(repo)
            .filter(|(k, _)| k == key)
            .map(|(_, staged)| Arc::clone(staged))
    }

    fn put(&self, repo: RepoKey, key: StagedKey, staged: Arc<Vec<Staged>>) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.len() >= CACHED_REPOSITORIES && !entries.contains_key(&repo) {
            entries.clear();
        }
        entries.insert(repo, (key, staged));
    }
}

/// 경로별 상태. `limit` 이 있으면 그 경로(폴더면 그 아래)의 항목만. 경로순(바이트순, git 과 같다). `interrupt` 가 서면 멈춘다.
/// `cache` 가 있으면 HEAD↔index 결과를 거기서 찾고 담는다 — `trusted` 는 `repo` 를 연 신뢰다(키에 넣는다).
pub fn status(
    repo: &gix::Repository,
    limit: Option<&[String]>,
    interrupt: &Arc<AtomicBool>,
    cache: Option<(&StagedCache, bool)>,
) -> Result<Vec<GitStatusEntry>, Stopped> {
    let mut entries: BTreeMap<String, GitStatusEntry> = BTreeMap::new();
    let index = repo.index_or_empty()?;
    let wanted =
        |path: &str| limit.is_none_or(|limit| limit.iter().any(|l| paths::is_within(path, l)));

    // index↔작업 트리 · untracked · 충돌.
    let patterns: Vec<BString> = limit
        .map(|limit| limit.iter().map(|p| paths::pathspec(p)).collect())
        .unwrap_or_default();
    let platform = repo
        .status(gix::progress::Discard)?
        .index(gix::worktree::IndexPersistedOrInMemory::Persisted(
            index.clone(),
        ))
        .untracked_files(gix::status::UntrackedFiles::Files)
        .index_worktree_submodules(gix::status::Submodule::Given {
            ignore: gix::submodule::config::Ignore::Dirty,
            check_dirty: false,
        })
        .index_worktree_rewrites(None)
        .should_interrupt_owned(Arc::clone(interrupt));
    let mut iter = platform.into_index_worktree_iter(patterns)?;
    for item in iter.by_ref() {
        let item = item?;
        let Some((path, change)) = worktree_change(&item) else {
            continue;
        };
        if !wanted(&path) {
            continue;
        }
        let entry = entries.entry(path.clone()).or_insert_with(|| blank(path));
        match change {
            Unstaged::Change(change) => entry.unstaged = Some(change),
            Unstaged::Conflict(conflict) => entry.conflict = Some(conflict),
        }
    }
    if interrupt.load(Ordering::Relaxed) {
        return Err(Stopped::Interrupted);
    }

    // HEAD↔index. 충돌한 경로(단계 1~3 만 있다)는 gix 가 내지 않는다 — git 처럼 충돌만 보인다.
    let tree = repo.head_tree_id_or_empty()?.detach();
    let key = index
        .checksum()
        .map(|checksum| (tree, checksum, rename_config(repo)));
    let slot = cache.map(|(cache, trusted)| (cache, (repo.git_dir().to_path_buf(), trusted)));
    let cached = match (&slot, &key) {
        (Some((cache, repo_key)), Some(key)) => cache.get(repo_key, key),
        _ => None,
    };
    let all = match cached {
        Some(all) => all,
        None => {
            let all = Arc::new(staged(repo, tree, &index, interrupt)?);
            if let (Some((cache, repo_key)), Some(key)) = (slot, key) {
                cache.put(repo_key, key, Arc::clone(&all));
            }
            all
        }
    };
    for s in all.iter() {
        if !(wanted(&s.path) || s.orig.as_deref().is_some_and(wanted)) {
            continue;
        }
        let entry = entries
            .entry(s.path.clone())
            .or_insert_with(|| blank(s.path.clone()));
        entry.staged = Some(s.change);
        entry.orig_path = s.orig.clone();
    }
    Ok(entries.into_values().collect())
}

/// HEAD 트리 `tree` ↔ `index` 의 변경 전부(경로순은 아니다).
fn staged(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    index: &gix::index::State,
    interrupt: &Arc<AtomicBool>,
) -> Result<Vec<Staged>, Stopped> {
    let expanded = expanded(repo, index)?;
    let mut out = Vec::new();
    repo.tree_index_status(
        &tree,
        expanded.as_ref().unwrap_or(index),
        None,
        track_renames(repo),
        |change, _, _| {
            if interrupt.load(Ordering::Relaxed) {
                return Ok(std::ops::ControlFlow::Break(()));
            }
            let (path, orig, change) = staged_change(&change);
            out.push(Staged { path, orig, change });
            Ok(std::ops::ControlFlow::Continue(()))
        },
    )?;
    if interrupt.load(Ordering::Relaxed) {
        return Err(Stopped::Interrupted);
    }
    Ok(out)
}

/// HEAD↔index 의 이름 바뀜 찾기를 정하는 설정 값([`track_renames`] 가 읽는 것). 설정만 바꿔도 앞 결과를 쓰지 않게 캐시 키에 넣는다.
fn rename_config(repo: &gix::Repository) -> Vec<Option<BString>> {
    let config = repo.config_snapshot();
    [
        "status.renames",
        "status.renameLimit",
        "diff.renames",
        "diff.renameLimit",
    ]
    .iter()
    .map(|key| config.string(*key))
    .collect()
}

/// git 처럼 이름 바뀜 찾기를 정한다: `status.renames`(없으면 `diff.renames`, 그것도 없으면 켬 — `copies` 면 복사도),
/// `status.renameLimit`(없으면 `diff.renameLimit`). gix 0.90 의 `TrackRenames::AsConfigured` 는 `status.renames` 대신
/// `merge.renames` 를 읽어 쓰지 않는다(`gix::config::tree::Status::RENAMES` 가 `merge` 절에 묶여 있다).
fn track_renames(repo: &gix::Repository) -> gix::status::tree_index::TrackRenames {
    use gix::bstr::ByteSlice;
    use gix::status::tree_index::TrackRenames;
    let config = repo.config_snapshot();
    // `Some(true)` 이름 바뀜, `Some(false)` 끔, `None` 복사까지.
    let mode =
        ["status.renames", "diff.renames"]
            .iter()
            .find_map(|key| match config.boolean(*key) {
                Some(on) => Some(Some(on)),
                None => config
                    .string(*key)
                    .filter(|v| v.as_bytes() == b"copies" || v.as_bytes() == b"copy")
                    .map(|_| None),
            });
    let limit = ["status.renameLimit", "diff.renameLimit"]
        .iter()
        .find_map(|key| config.integer(*key))
        .and_then(|n| usize::try_from(n).ok());
    let default = gix::diff::Rewrites::default();
    match mode.unwrap_or(Some(true)) {
        Some(false) => TrackRenames::Disabled,
        mode => TrackRenames::Given(gix::diff::Rewrites {
            copies: mode.is_none().then(gix::diff::rewrites::Copies::default),
            limit: limit.unwrap_or(default.limit),
            ..default
        }),
    }
}

/// sparse index(`git sparse-checkout --sparse-index`)의 접힌 폴더 항목을 그 트리의 파일들로 펼친 사본 — gix 의 HEAD↔index 비교는
/// 접힌 항목을 받지 않는다. 펼친 파일은 git 이 펼칠 때처럼 skip-worktree 다. 접힌 항목이 없으면 `None`. index 파일은 그대로다.
fn expanded(
    repo: &gix::Repository,
    index: &gix::index::State,
) -> Result<Option<gix::index::State>, Stopped> {
    if !index.is_sparse() {
        return Ok(None);
    }
    let mut out = gix::index::State::new(repo.object_hash());
    for entry in index.entries() {
        let path = entry.path(index);
        if !entry.mode.is_sparse() {
            out.dangerously_push_entry(entry.stat, entry.id, entry.flags, entry.mode, path);
            continue;
        }
        let tree = gix::index::State::from_tree(&entry.id, &repo.objects, Default::default())?;
        // 접힌 항목의 경로는 `/` 로 끝난다(`folded/`).
        let mut full = BString::from(path.to_vec());
        if !full.ends_with(b"/") {
            full.push(b'/');
        }
        let prefix = full.len();
        for file in tree.entries() {
            full.truncate(prefix);
            full.extend_from_slice(file.path(&tree));
            out.dangerously_push_entry(
                gix::index::entry::Stat::default(),
                file.id,
                file.flags | gix::index::entry::Flags::SKIP_WORKTREE,
                file.mode,
                full.as_ref(),
            );
        }
    }
    out.sort_entries();
    Ok(Some(out))
}

fn blank(path: String) -> GitStatusEntry {
    GitStatusEntry {
        path,
        orig_path: None,
        staged: None,
        unstaged: None,
        conflict: None,
    }
}

enum Unstaged {
    Change(GitChange),
    Conflict(GitConflict),
}

/// index↔작업 트리 항목 하나. 보일 것이 없으면(시각만 바뀜 · ignore 된 것) `None`.
fn worktree_change(item: &WorktreeItem) -> Option<(String, Unstaged)> {
    match item {
        WorktreeItem::Modification {
            rela_path, status, ..
        } => {
            let change = match status {
                EntryStatus::Conflict { summary, .. } => Unstaged::Conflict(conflict(*summary)),
                EntryStatus::Change(Change::Removed) => Unstaged::Change(GitChange::Deleted),
                EntryStatus::Change(Change::Type { .. }) => {
                    Unstaged::Change(GitChange::TypeChanged)
                }
                EntryStatus::Change(Change::Modification {
                    executable_bit_changed,
                    content_change,
                    ..
                }) => {
                    if !executable_bit_changed && content_change.is_none() {
                        return None;
                    }
                    Unstaged::Change(GitChange::Modified)
                }
                EntryStatus::Change(Change::SubmoduleModification(_)) => {
                    Unstaged::Change(GitChange::Modified)
                }
                // `git add --intent-to-add`: git 은 `.A` 로 보인다.
                EntryStatus::IntentToAdd => Unstaged::Change(GitChange::Added),
                EntryStatus::NeedsUpdate(_) => return None,
            };
            Some((paths::from_git(rela_path.as_ref()), change))
        }
        WorktreeItem::DirectoryContents { entry, .. } => {
            if entry.status != gix::dir::entry::Status::Untracked {
                return None;
            }
            let mut path = paths::from_git(entry.rela_path.as_ref());
            // 폴더 하나로 낸 것(중첩 저장소)은 git 처럼 `/` 로 끝낸다.
            if entry.disk_kind.is_some_and(|kind| kind.is_dir()) {
                path.push('/');
            }
            Some((path, Unstaged::Change(GitChange::Untracked)))
        }
        // index↔작업 트리 이름 바뀜은 켜지 않았다.
        WorktreeItem::Rewrite { .. } => None,
    }
}

fn conflict(summary: Conflict) -> GitConflict {
    match summary {
        Conflict::BothDeleted => GitConflict::BothDeleted,
        Conflict::AddedByUs => GitConflict::AddedByUs,
        Conflict::DeletedByThem => GitConflict::DeletedByThem,
        Conflict::AddedByThem => GitConflict::AddedByThem,
        Conflict::DeletedByUs => GitConflict::DeletedByUs,
        Conflict::BothAdded => GitConflict::BothAdded,
        Conflict::BothModified => GitConflict::BothModified,
    }
}

/// HEAD↔index 변경 하나: (경로, 옛 경로(이름 바뀜 · 복사), 변경). 아직 담기지 않은 `--intent-to-add` 항목은 gix 가 내지 않는다
/// (git 처럼 스테이징한 것이 아니다).
fn staged_change(
    change: &gix::diff::index::ChangeRef<'_, '_>,
) -> (String, Option<String>, GitChange) {
    use gix::diff::index::ChangeRef;
    match change {
        ChangeRef::Addition { location, .. } => {
            (paths::from_git(location.as_ref()), None, GitChange::Added)
        }
        ChangeRef::Deletion { location, .. } => {
            (paths::from_git(location.as_ref()), None, GitChange::Deleted)
        }
        ChangeRef::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            ..
        } => {
            let kind = if mode_kind(*previous_entry_mode) == mode_kind(*entry_mode) {
                GitChange::Modified
            } else {
                GitChange::TypeChanged
            };
            (paths::from_git(location.as_ref()), None, kind)
        }
        ChangeRef::Rewrite {
            source_location,
            location,
            copy,
            ..
        } => (
            paths::from_git(location.as_ref()),
            Some(paths::from_git(source_location.as_ref())),
            if *copy {
                GitChange::Copied
            } else {
                GitChange::Renamed
            },
        ),
    }
}

/// 파일 · 실행 파일은 같은 종류다 — 실행 비트만 바뀐 것은 git 처럼 `M` 이다.
fn mode_kind(mode: gix::index::entry::Mode) -> Option<gix::object::tree::EntryKind> {
    use gix::object::tree::EntryKind;
    mode.to_tree_entry_mode().map(|m| match m.kind() {
        EntryKind::BlobExecutable => EntryKind::Blob,
        kind => kind,
    })
}
