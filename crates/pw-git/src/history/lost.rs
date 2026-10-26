//! reset hard 가 잃을 변경(결정 M2-D6 ① — 확인 다이얼로그는 M2-20). 읽기라 gix 와 파일 시스템으로만 한다 — 미신뢰 프로젝트면 어느
//! 설정의 필터 드라이버도 떼고 연다(결정 M2-D8).
//!
//! - `changes`: 커밋하지 않은 추적 파일의 변경(스테이징한 것 · 작업 트리 · 충돌 — `git.status` 와 같은 계산,
//!   [`crate::status::compute`]). reset hard 가 index 와 작업 트리를 대상 커밋으로 맞추므로 모두 사라진다.
//! - `overwritten`: 추적하지 않는 것(untracked · ignore 된 파일, 중첩 저장소) 중 git 이 묻지 않고 덮어쓰거나 지우는 것. reset hard 는
//!   대상 트리의 경로를 모두 쓰므로 index 에 없는 대상 경로마다 디스크를 본다:
//!   - 그 경로에 파일 · 링크가 있으면 그것. ignore 된 파일도 센다 — 추적하다 `.gitignore` 로 뺀 `.env` 를 그 전 커밋으로 되돌리면
//!     덮어써진다.
//!   - 그 경로가 폴더면(대상에서는 파일) 그 아래에 추적하지 않는 것이 있을 때 폴더째(`dir/` — 중첩 저장소도).
//!   - 조상 경로에 폴더 대신 파일 · 링크가 있으면 그것(대상의 `a/b.txt` 를 쓰려면 파일 `a` 를 지운다).
//!
//!   대소문자 · 유니코드 정규화를 무시하는 파일 시스템(macOS)에서는 대상의 `readme.md` 가 디스크의 `README.md` 로 열린다 — 디스크의
//!   실제 이름으로 싣고, 그 이름이 추적 중이면(대소문자만 바꾼 이름 바꾸기) 싣지 않는다. `changes` 에 이미 있는 경로
//!   (`git rm --cached` 한 파일)는 싣지 않는다.
//! - 서브모듈 안의 편집은 세지 않는다 — reset 이 `--no-recurse-submodules` 로 서브모듈 작업 트리를 건드리지 않는다(`git.reset`).
//! - 지문(`fingerprint`): HEAD · 대상 커밋 · 목록과 목록의 경로마다 index 항목 · 디스크 메타데이터(종류 · 크기 · 시각 · inode)의
//!   해시. 확인한 뒤 그사이 무엇이 바뀌었으면 reset hard 가 아무것도 하지 않고 거절한다. `dir/` 항목은 그 폴더 자체의 메타데이터만
//!   본다(폴더째 잃는다고 확인한 것이다).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs::Metadata;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gix::bstr::{BStr, ByteSlice};
use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};

use crate::Opened;
use crate::status::compute::{self, Stopped};
use crate::status::{GitChange, GitStatusEntry};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLostChanges {
    /// 사라지는 변경(`git.status` 의 항목 그대로 — untracked 는 빠진다). 경로순.
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
    /// 모듈 머리 주석의 지문.
    pub fingerprint: String,
}

/// `rev` 로 reset hard 하면 잃을 변경. `interrupt` 가 서면 취소 오류다.
pub fn lost(
    opened: &Opened,
    rev: &str,
    interrupt: Option<&Arc<AtomicBool>>,
) -> Result<Lost, RpcError> {
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
    let entries = compute::status(&repo, None, &interrupt, None).map_err(|e| match e {
        Stopped::Interrupted => RpcError::cancelled(),
        Stopped::Failed(e) => RpcError::internal(format!("cannot read the status: {e}")),
    })?;
    let mut lost = GitLostChanges::default();
    for entry in entries {
        let untracked = entry.unstaged == Some(GitChange::Untracked);
        // `git rm --cached` 한 파일은 스테이징한 삭제이면서 untracked 다 — 경로마다 한 번만, 변경 쪽에 싣는다.
        if entry.staged.is_some()
            || entry.conflict.is_some()
            || (entry.unstaged.is_some() && !untracked)
        {
            lost.changes.push(entry);
        }
    }

    let index = repo.index_or_empty().map_err(|e| internal(&e))?;
    let work_tree = opened.target.work_tree.as_path();
    let mut records = gix::traverse::tree::Recorder::default();
    target
        .tree()
        .map_err(|e| internal(&e))?
        .traverse()
        .depthfirst(&mut records)
        .map_err(|e| internal(&e))?;
    let tracked = |rel: &str| index.entry_index_by_path(rel.as_bytes().as_bstr()).is_ok();
    let in_changes: HashSet<&str> = lost.changes.iter().map(|e| e.path.as_str()).collect();
    let mut overwritten = Vec::new();
    let mut checked_dirs = HashSet::new();
    for record in &records.records {
        if interrupt.load(Ordering::Relaxed) {
            return Err(RpcError::cancelled());
        }
        if record.mode.is_tree() {
            continue;
        }
        let Ok(path) = record.filepath.to_str() else {
            continue;
        };
        if tracked(path) {
            continue;
        }
        // 조상 중 폴더가 아닌 것(파일 · 링크) — 대상의 경로를 쓰려면 git 이 지운다.
        let mut blocked = false;
        for (at, _) in path.match_indices('/') {
            let dir = &path[..at];
            if !checked_dirs.insert(dir.to_owned()) {
                continue;
            }
            match std::fs::symlink_metadata(work_tree.join(dir)) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    let real = disk_name(work_tree, dir);
                    if !tracked(&real) {
                        overwritten.push(real);
                    }
                    blocked = true;
                    break;
                }
                Err(_) => {
                    blocked = true;
                    break;
                }
            }
        }
        // 서브모듈 경로는 git 이 폴더를 만들거나 그대로 둔다 — 그 안을 덮지 않는다.
        if blocked || record.mode.is_commit() {
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
        } else if !tracked(&real) && !in_changes.contains(real.as_str()) {
            overwritten.push(real);
        }
    }
    overwritten.sort();
    overwritten.dedup();
    lost.overwritten = overwritten;

    let fingerprint = fingerprint(
        &repo,
        &index,
        work_tree,
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

/// 모듈 머리 주석의 지문(저장소의 해시 종류로 만든 해시).
fn fingerprint(
    repo: &gix::Repository,
    index: &gix::index::State,
    work_tree: &Path,
    head: Option<&str>,
    target: &str,
    lost: &GitLostChanges,
) -> String {
    let mut text = format!("head {}\ntarget {target}\n", head.unwrap_or("-"));
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
                    " index {} {:?} {:?} {}",
                    e.id,
                    e.stage(),
                    e.mode,
                    e.stat.size
                );
            }
        }
        disk(&mut text, &work_tree.join(&entry.path));
    }
    for path in &lost.overwritten {
        let _ = writeln!(text, "overwritten {path}");
        disk(&mut text, &work_tree.join(path.trim_end_matches('/')));
    }
    gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Blob, text.as_bytes())
        .map_or_else(|_| text.clone(), |id| id.to_string())
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
