//! reset hard · keep · mixed 가 잃을 변경(결정 M2-D6 ① — 확인 다이얼로그는 M2-20). 읽기라 gix 와 파일 시스템으로만 한다 — 미신뢰 프로젝트면
//! 어느 설정의 필터 드라이버도 떼고 연다(결정 M2-D8).
//!
//! 계산은 「대상 트리의 경로마다 그 경로 · 앞(조상) · 아래(폴더면 그 안)의 디스크」 를 직접 본다 — 추적 중인 경로도 건너뛰지 않고,
//! index 의 skip-worktree · assume-unchanged 플래그도 믿지 않는다(그 항목은 `git.status` 가 보지 않지만 reset 은 덮는다).
//!
//! - `changes`: 커밋하지 않은 추적 파일의 변경.
//!   - hard: 스테이징한 것 · 작업 트리 · 충돌(`git.status` 와 같은 계산, [`crate::status::compute`]) — reset hard 가 index 와 작업 트리를
//!     대상 커밋으로 맞추므로 모두 사라진다.
//!   - keep: 작업 트리 편집은 git 이 지키거나(두 커밋에서 같은 경로) 거절한다(다른 경로). 잃는 것은 디스크와 다른 스테이징 내용
//!     (index 를 대상으로 되돌린다) · 충돌이다.
//!   - 둘 다: skip-worktree · assume-unchanged 항목인데 디스크에 index 와 다른 것이 있으면(내용 · 종류 · 실행 비트 — 필터 없이
//!     바이트로 비교한다) 그 경로. keep 은 두 커밋에서 다른 경로만.
//!   - mixed: 작업 트리는 그대로라 index 에만 있는 내용만 잃는다 — 단계 0 항목의 blob 이 HEAD 와도, 대상 커밋의 같은 경로와도,
//!     디스크(필터 없이 바이트, 종류 · 실행 비트는 보지 않는다)와도 다른 경로. 빈 blob(`git add -N` 포함) · 서브모듈은 뺀다.
//!     충돌의 단계 1~3 은 커밋에 있는 내용이라 세지 않는다. `overwritten` 은 늘 비어 있다.
//! - `overwritten`: 추적하지 않는 것(untracked · ignore 된 파일, 중첩 저장소) 중 git 이 묻지 않고 덮어쓰거나 지우는 것. 대상 트리의
//!   파일 · 링크 경로마다(keep 은 HEAD 와 다른 경로만):
//!   - 그 경로에 index 에 없는 파일 · 링크가 있으면 그것. ignore 된 파일도 센다 — 추적하다 `.gitignore` 로 뺀 `.env` 를 그 전 커밋으로
//!     되돌리면 덮어써진다. keep 도 ignore 된 것은 묻지 않고 덮는다(untracked 면 거절하지만 가르지 않고 싣는다).
//!   - 그 경로가 폴더면(대상에서는 파일 — 추적하던 파일을 지우고 만든 폴더도) 그 아래에 index 에 없는 것이 있을 때 폴더째(`dir/` —
//!     중첩 저장소도).
//!   - 조상 경로에 폴더 대신 index 에 없는 파일 · 링크가 있으면 그것(대상의 `a/b.txt` 를 쓰려면 파일 `a` 를 지운다 — 추적하던 폴더를
//!     지우고 만든 파일도).
//!
//!   대상에 없고 index 에만 있는 경로는 보지 않는다 — git 은 그 파일만 지우고, 그 자리가 폴더면 그대로 둔다. 대소문자 · 유니코드
//!   정규화를 무시하는 파일 시스템(macOS)에서는 대상의 `readme.md` 가 디스크의 `README.md` 로 열린다 — 디스크의 실제 이름으로 싣고,
//!   그 이름이 추적 중이면(대소문자만 바꾼 이름 바꾸기) 싣지 않는다. `changes` 에 이미 있는 경로(`git rm --cached` 한 파일)는 싣지
//!   않는다.
//! - 서브모듈 안의 편집은 세지 않는다 — reset 이 `--no-recurse-submodules` 로 서브모듈 작업 트리를 건드리지 않는다(`git.reset`).
//! - 지문(`fingerprint`): 방식 · HEAD · 대상 커밋 · 목록과 목록의 경로마다 index 항목 · 디스크 메타데이터(종류 · 크기 · 시각 · inode ·
//!   ctime)의 해시. `dir/` 항목은 그 아래 전부(경로와 메타데이터 — 내용을 고치면 시각이 바뀐다)를 넣는다. 확인한 뒤 그사이 무엇이
//!   바뀌었으면 reset 이 아무것도 하지 않고 거절한다. `dir/` 아래가 모두 [`FINGERPRINT_ENTRIES`] 개를 넘으면 지문이 `None` 이다 —
//!   덜 보고 셈하지 않는다(그 reset 은 확인할 수 없어 하지 않는다).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::Metadata;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gix::bstr::{BStr, ByteSlice};
use gix::index::entry::{Flags, Mode};
use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};

use super::ResetMode;
use crate::Opened;
use crate::status::compute::{self, Stopped};
use crate::status::{GitChange, GitStatusEntry};

/// 지문에 넣는 `dir/` 항목 아래의 항목 수 상한(모든 `dir/` 을 합쳐).
pub const FINGERPRINT_ENTRIES: usize = 10_000;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLostChanges {
    /// 사라지는 변경(`git.status` 의 항목 꼴 — untracked 는 빠진다). 경로순.
    pub changes: Vec<GitStatusEntry>,
    /// 덮어써지거나 지워지는 추적하지 않는 것(작업 트리 기준 상대 경로, `/`, 디스크의 실제 이름). 추적하지 않는 것이 든 폴더가 통째로
    /// 파일로 바뀌면 `dir/`. 경로순.
    pub overwritten: Vec<String>,
}

impl GitLostChanges {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.overwritten.is_empty()
    }
}

/// [`lost`] 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lost {
    /// 읽은 때의 HEAD 커밋. 태어나지 않은 브랜치면 `None`.
    pub head: Option<String>,
    /// 대상 커밋(전체 해시).
    pub target: String,
    pub lost: GitLostChanges,
    /// 모듈 머리 주석의 지문. `dir/` 아래가 너무 많으면 `None`.
    pub fingerprint: Option<String>,
}

/// 경로의 (mode, 객체) — 트리 · index 를 같은 꼴로 비교한다.
type Blob = (gix::objs::tree::EntryMode, gix::ObjectId);

/// `rev` 로 `mode`(hard · keep · mixed — soft 는 keep 처럼 센다) reset 하면 잃을 변경. `interrupt` 가 서면 취소 오류다.
pub fn lost(
    opened: &Opened,
    rev: &str,
    mode: ResetMode,
    interrupt: Option<&Arc<AtomicBool>>,
) -> Result<Lost, RpcError> {
    let hard = mode == ResetMode::Hard;
    let mut repo = opened.repo.clone();
    if !opened.trusted {
        crate::diff::changes::without_filter_drivers(&mut repo)?;
    }
    let internal = |e: &dyn std::fmt::Display| RpcError::internal(e.to_string());
    let target = repo
        .rev_parse_single(rev)
        .map_err(|e| RpcError::invalid_params(format!("cannot resolve `{rev}`: {e}")))?
        .object()
        .map_err(|e| internal(&e))?
        .peel_to_commit()
        .map_err(|e| RpcError::invalid_params(format!("`{rev}` is not a commit: {e}")))?;
    let head = repo.head_id().ok().map(|id| id.to_string());
    let interrupt = interrupt.map_or_else(|| Arc::new(AtomicBool::new(false)), Arc::clone);
    let index = repo.index_or_empty().map_err(|e| internal(&e))?;
    let work_tree = opened.target.work_tree.as_path();
    let target_files = files(&target.tree().map_err(|e| internal(&e))?)?;
    let head_files = match repo.head_tree_id_or_empty() {
        Ok(id) if !id.is_empty_tree() => files(&repo.find_tree(id).map_err(|e| internal(&e))?)?,
        Ok(_) => HashMap::new(),
        Err(e) => return Err(internal(&e)),
    };
    let lost = if mode == ResetMode::Mixed {
        GitLostChanges {
            changes: index_only(
                &repo,
                &index,
                work_tree,
                &head_files,
                &target_files,
                &interrupt,
            )?,
            overwritten: Vec::new(),
        }
    } else {
        on_disk(
            &repo,
            &index,
            work_tree,
            &head_files,
            &target_files,
            hard,
            &interrupt,
        )?
    };

    let fingerprint = fingerprint(
        &repo,
        &index,
        work_tree,
        mode,
        head.as_deref(),
        &target.id.to_string(),
        &lost,
    );
    Ok(Lost {
        head,
        target: target.id.to_string(),
        lost,
        fingerprint,
    })
}

/// hard · keep 이 잃을 변경(모듈 머리 주석의 `changes` · `overwritten`).
fn on_disk(
    repo: &gix::Repository,
    index: &gix::index::State,
    work_tree: &Path,
    head_files: &HashMap<String, Blob>,
    target_files: &HashMap<String, Blob>,
    hard: bool,
    interrupt: &Arc<AtomicBool>,
) -> Result<GitLostChanges, RpcError> {
    let entries = compute::status(repo, None, interrupt, None).map_err(|e| match e {
        Stopped::Interrupted => RpcError::cancelled(),
        Stopped::Failed(e) => RpcError::internal(format!("cannot read the status: {e}")),
    })?;
    let mut changes: HashMap<String, GitStatusEntry> = HashMap::new();
    for entry in entries {
        let untracked = entry.unstaged == Some(GitChange::Untracked);
        let unstaged = entry.unstaged.is_some() && !untracked;
        // `git rm --cached` 한 파일은 스테이징한 삭제이면서 untracked 다 — 경로마다 한 번만, 변경 쪽에 싣는다.
        let lost = if hard {
            entry.staged.is_some() || entry.conflict.is_some() || unstaged
        } else {
            entry.conflict.is_some() || (entry.staged.is_some() && unstaged)
        };
        if lost {
            changes.insert(entry.path.clone(), entry);
        }
    }

    // keep 은 두 커밋에서 같은 경로를 건드리지 않는다.
    let touched = |path: &str| hard || head_files.get(path) != target_files.get(path);

    // skip-worktree · assume-unchanged 항목: git.status 가 보지 않는다.
    let file_mode = repo
        .config_snapshot()
        .boolean("core.fileMode")
        .unwrap_or(true);
    for e in index.entries() {
        if interrupt.load(Ordering::Relaxed) {
            return Err(RpcError::cancelled());
        }
        if !e
            .flags
            .intersects(Flags::SKIP_WORKTREE | Flags::ASSUME_VALID)
            || e.stage_raw() != 0
            || e.mode.is_submodule()
            || e.mode.is_sparse()
        {
            continue;
        }
        let Ok(path) = e.path(index).to_str() else {
            continue;
        };
        if !touched(path) {
            continue;
        }
        let Some(change) = differs_on_disk(repo, work_tree, path, e, file_mode, interrupt)? else {
            continue;
        };
        changes
            .entry(path.to_owned())
            .or_insert_with(|| blank(path))
            .unstaged
            .get_or_insert(change);
    }
    let mut lost = GitLostChanges {
        changes: changes.into_values().collect(),
        overwritten: Vec::new(),
    };
    lost.changes.sort_by(|a, b| a.path.cmp(&b.path));

    let tracked = |rel: &str| index.entry_index_by_path(rel.as_bytes().as_bstr()).is_ok();
    let in_changes: HashSet<&str> = lost.changes.iter().map(|e| e.path.as_str()).collect();
    let mut overwritten = Vec::new();
    // 이미 본 조상 폴더와 그것이 막혔는가(폴더가 아니다).
    let mut checked_dirs: HashMap<String, bool> = HashMap::new();
    let mut paths: Vec<(&String, &Blob)> = target_files.iter().collect();
    paths.sort();
    for (path, (entry_mode, _)) in paths {
        if interrupt.load(Ordering::Relaxed) {
            return Err(RpcError::cancelled());
        }
        if !touched(path) {
            continue;
        }
        // 조상 중 폴더가 아닌 것(파일 · 링크) — 대상의 경로를 쓰려면 git 이 지운다.
        let mut blocked = false;
        for (at, _) in path.match_indices('/') {
            let dir = &path[..at];
            if let Some(&was) = checked_dirs.get(dir) {
                if was {
                    blocked = true;
                    break;
                }
                continue;
            }
            let block = match std::fs::symlink_metadata(work_tree.join(dir)) {
                Ok(meta) if meta.is_dir() => false,
                Ok(_) => {
                    let real = disk_name(work_tree, dir);
                    if !tracked(dir) && !tracked(&real) {
                        overwritten.push(real);
                    }
                    true
                }
                Err(_) => true,
            };
            checked_dirs.insert(dir.to_owned(), block);
            if block {
                blocked = true;
                break;
            }
        }
        // 서브모듈 경로는 git 이 폴더를 만들거나 그대로 둔다 — 그 안을 덮지 않는다.
        if blocked || entry_mode.is_commit() {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(work_tree.join(path)) else {
            continue;
        };
        let real = disk_name(work_tree, path);
        if meta.is_dir() {
            if has_untracked(work_tree, &real, &tracked) {
                overwritten.push(format!("{real}/"));
            }
        } else if !tracked(path) && !tracked(&real) && !in_changes.contains(real.as_str()) {
            overwritten.push(real);
        }
    }
    overwritten.sort();
    overwritten.dedup();
    lost.overwritten = overwritten;
    Ok(lost)
}

/// mixed 가 잃을 변경(모듈 머리 주석의 mixed). 경로순.
fn index_only(
    repo: &gix::Repository,
    index: &gix::index::State,
    work_tree: &Path,
    head_files: &HashMap<String, Blob>,
    target_files: &HashMap<String, Blob>,
    interrupt: &AtomicBool,
) -> Result<Vec<GitStatusEntry>, RpcError> {
    let empty = gix::ObjectId::empty_blob(repo.object_hash());
    let mut changes = Vec::new();
    for e in index.entries() {
        if interrupt.load(Ordering::Relaxed) {
            return Err(RpcError::cancelled());
        }
        if e.stage_raw() != 0 || e.mode.is_submodule() || e.mode.is_sparse() || e.id == empty {
            continue;
        }
        let Ok(path) = e.path(index).to_str() else {
            continue;
        };
        let head = head_files.get(path);
        if head.is_some_and(|(_, id)| *id == e.id)
            || target_files.get(path).is_some_and(|(_, id)| *id == e.id)
        {
            continue;
        }
        let link = e.mode == Mode::SYMLINK;
        let full = work_tree.join(path);
        let unstaged = match std::fs::symlink_metadata(&full) {
            Ok(meta) if !meta.is_dir() => {
                if disk_blob(repo, &full, &meta, interrupt)? == Some(e.id) {
                    continue;
                }
                if link == meta.file_type().is_symlink() {
                    GitChange::Modified
                } else {
                    GitChange::TypeChanged
                }
            }
            _ => GitChange::Deleted,
        };
        let staged = match head {
            None => GitChange::Added,
            Some((mode, _)) if mode.is_link() != link => GitChange::TypeChanged,
            Some(_) => GitChange::Modified,
        };
        changes.push(GitStatusEntry {
            staged: Some(staged),
            unstaged: Some(unstaged),
            ..blank(path)
        });
    }
    Ok(changes)
}

/// 트리의 파일 · 링크 · 서브모듈 경로(폴더는 빠진다).
fn files(tree: &gix::Tree<'_>) -> Result<HashMap<String, Blob>, RpcError> {
    let mut records = gix::traverse::tree::Recorder::default();
    tree.traverse()
        .depthfirst(&mut records)
        .map_err(|e| RpcError::internal(e.to_string()))?;
    Ok(records
        .records
        .into_iter()
        .filter(|r| !r.mode.is_tree())
        .filter_map(|r| Some((r.filepath.to_str().ok()?.to_owned(), (r.mode, r.oid))))
        .collect())
}

fn blank(path: &str) -> GitStatusEntry {
    GitStatusEntry {
        path: path.to_owned(),
        orig_path: None,
        staged: None,
        unstaged: None,
        conflict: None,
    }
}

/// index 항목 `e` 와 디스크의 `path` 가 다르면 그 변경. 디스크에 없거나 폴더면 `None`(없는 것은 잃을 것이 없고, 폴더는
/// `overwritten` 이 본다). 내용은 필터 없이 바이트로 비교한다 — 필터가 있으면 같아도 다르다고 볼 수 있다(잃는 쪽으로 센다).
fn differs_on_disk(
    repo: &gix::Repository,
    work_tree: &Path,
    path: &str,
    e: &gix::index::Entry,
    file_mode: bool,
    interrupt: &AtomicBool,
) -> Result<Option<GitChange>, RpcError> {
    let full = work_tree.join(path);
    let Ok(meta) = std::fs::symlink_metadata(&full) else {
        return Ok(None);
    };
    if meta.is_dir() {
        return Ok(None);
    }
    let link = e.mode == Mode::SYMLINK;
    if link != meta.file_type().is_symlink() {
        return Ok(Some(GitChange::TypeChanged));
    }
    #[cfg(unix)]
    if !link && file_mode {
        use std::os::unix::fs::PermissionsExt;
        let exec = meta.permissions().mode() & 0o111 != 0;
        if exec != (e.mode == Mode::FILE_EXECUTABLE) {
            return Ok(Some(GitChange::Modified));
        }
    }
    #[cfg(not(unix))]
    let _ = file_mode;
    Ok(match disk_blob(repo, &full, &meta, interrupt)? {
        Some(id) if id == e.id => None,
        _ => Some(GitChange::Modified),
    })
}

/// 디스크의 파일 · 링크(`meta` 는 `full` 의 것)를 필터 없이 blob 으로 해시한다 — 링크는 가리키는 곳. 읽을 수 없으면 `None`.
fn disk_blob(
    repo: &gix::Repository,
    full: &Path,
    meta: &Metadata,
    interrupt: &AtomicBool,
) -> Result<Option<gix::ObjectId>, RpcError> {
    let hash = repo.object_hash();
    let id = if meta.file_type().is_symlink() {
        let Ok(to) = std::fs::read_link(full) else {
            return Ok(None);
        };
        gix::objs::compute_hash(
            hash,
            gix::objs::Kind::Blob,
            to.as_os_str().as_encoded_bytes(),
        )
    } else {
        let Ok(mut file) = std::fs::File::open(full) else {
            return Ok(None);
        };
        gix::objs::compute_stream_hash(
            hash,
            gix::objs::Kind::Blob,
            &mut file,
            meta.len(),
            &mut gix::progress::Discard,
            interrupt,
        )
    };
    if interrupt.load(Ordering::Relaxed) {
        return Err(RpcError::cancelled());
    }
    Ok(id.ok())
}

/// 작업 트리 기준 상대 경로를 디스크의 실제 이름으로(성분마다 같은 파일인 디렉터리 항목을 찾는다). 못 찾으면 받은 성분 그대로.
fn disk_name(work_tree: &Path, rel: &str) -> String {
    let mut dir = work_tree.to_path_buf();
    let mut out: Vec<String> = Vec::new();
    for part in rel.split('/') {
        let name = real_component(&dir, part).unwrap_or_else(|| part.to_owned());
        dir.push(&name);
        out.push(name);
    }
    out.join("/")
}

fn real_component(dir: &Path, part: &str) -> Option<String> {
    let want = std::fs::symlink_metadata(dir.join(part)).ok()?;
    let mut found = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name == part {
            return Some(name);
        }
        if found.is_none()
            && let Ok(meta) = entry.metadata()
            && same_file(&name, part, &meta, &want)
        {
            found = Some(name);
        }
    }
    found
}

#[cfg(unix)]
fn same_file(_name: &str, _part: &str, a: &Metadata, b: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(name: &str, part: &str, _a: &Metadata, _b: &Metadata) -> bool {
    name.eq_ignore_ascii_case(part)
}

/// `dir`(작업 트리 기준 상대) 아래에 index 에 없는 것(파일 · 링크 · `.git`)이 있는가. 링크는 따라가지 않는다.
fn has_untracked(work_tree: &Path, dir: &str, tracked: &dyn Fn(&str) -> bool) -> bool {
    let mut stack = vec![dir.to_owned()];
    while let Some(rel) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(work_tree.join(&rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                return true;
            };
            let child = format!("{rel}/{name}");
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && name != ".git" => stack.push(child),
                _ if !tracked(&child) => return true,
                _ => {}
            }
        }
    }
    false
}

/// 모듈 머리 주석의 지문(저장소의 해시 종류로 만든 해시). `dir/` 아래가 [`FINGERPRINT_ENTRIES`] 개를 넘으면 `None`.
fn fingerprint(
    repo: &gix::Repository,
    index: &gix::index::State,
    work_tree: &Path,
    mode: ResetMode,
    head: Option<&str>,
    target: &str,
    lost: &GitLostChanges,
) -> Option<String> {
    let mut text = format!(
        "mode {mode:?}\nhead {}\ntarget {target}\n",
        head.unwrap_or("-")
    );
    for entry in &lost.changes {
        let _ = writeln!(
            text,
            "change {} {:?} {:?} {:?}",
            entry.path, entry.staged, entry.unstaged, entry.conflict
        );
        let path: &BStr = entry.path.as_bytes().as_bstr();
        if let Some(range) = index.entry_range(path) {
            for e in &index.entries()[range] {
                let _ = writeln!(
                    text,
                    " index {} {:?} {:?} {:?} {}",
                    e.id,
                    e.stage(),
                    e.mode,
                    e.flags,
                    e.stat.size
                );
            }
        }
        disk(&mut text, &work_tree.join(&entry.path));
    }
    let mut budget = FINGERPRINT_ENTRIES;
    for path in &lost.overwritten {
        let _ = writeln!(text, "overwritten {path}");
        let rel = path.trim_end_matches('/');
        disk(&mut text, &work_tree.join(rel));
        if path.ends_with('/') && !tree(&mut text, work_tree, rel, &mut budget) {
            return None;
        }
    }
    gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Blob, text.as_bytes())
        .ok()
        .map(|id| id.to_string())
}

/// `dir` 아래 전부(경로순, 링크는 따라가지 않는다)의 경로와 메타데이터. `budget` 을 다 쓰면 `false`.
fn tree(text: &mut String, work_tree: &Path, dir: &str, budget: &mut usize) -> bool {
    let mut stack = vec![dir.to_owned()];
    while let Some(rel) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(work_tree.join(&rel)) else {
            continue;
        };
        let mut names: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
        names.sort();
        for name in names.into_iter().rev() {
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            let child = format!("{rel}/{}", name.to_string_lossy());
            let full = work_tree.join(&child);
            let _ = write!(text, " in {child}");
            disk(text, &full);
            if std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_dir()) {
                stack.push(child);
            }
        }
    }
    true
}

fn disk(text: &mut String, path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        text.push_str(" disk -\n");
        return;
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let _ = write!(text, " disk {:?} {} {mtime}", meta.file_type(), meta.len());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = write!(
            text,
            " {} {} {} {}",
            meta.ino(),
            meta.mode(),
            meta.ctime(),
            meta.ctime_nsec()
        );
    }
    text.push('\n');
}
