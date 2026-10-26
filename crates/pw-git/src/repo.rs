//! 저장소 해석(결정 M2-D3): 프로젝트의 git 기능이 다루는 저장소 하나를 찾고, 그 HEAD 와 진행 중 작업을 gix 로 읽는다.
//!
//! - 대상은 활성 워크트리 안의 프로젝트 루트(없으면 주 루트)를 담은 저장소다. 프로젝트 루트가 저장소의 하위 폴더(모노레포의
//!   일부)여도 저장소 전체다. 찾기는 git 과 같이 루트에서 위로 가장 가까운 `.git` 이다.
//! - 찾은 작업 트리의 `.git` 이 파일이면 M1-05 의 위조 판정(`pw_core::config::gitdir::git_file`)을 거친다 — 관리 폴더가 그
//!   저장소의 `worktrees/<이름>` 바로 아래가 아니거나 되돌림 포인터가 맞지 않는 연결 워크트리는 저장소로 보지 않는다. `commondir` 이
//!   없는 관리 폴더(서브모듈)는 git 처럼 따라간다.
//! - 저장소 밖, bare 저장소(작업 트리가 없다), 위조 `.git` 은 모두 「저장소 없음」 이다 — git 기능 전체가 빈 상태가 된다
//!   (10-ui.md 「빈 상태 · 오류 상태」).
//!
//! git 프로세스를 띄우지 않는다(결정 M2-D8 「읽기는 gix」).

use std::path::{Path, PathBuf};

use gix::state::InProgress;
use pw_core::config::gitdir::{GitFile, git_file};

use crate::methods::{RepoHead, RepoInfo, RepoOperation};

/// 저장소를 찾지 못한 이유. 모두 「저장소 없음」 으로 끝나지만 로그에 남긴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoRepo {
    /// 루트와 그 조상에 저장소가 없다(또는 찾기가 파일 시스템 경계에서 멈췄다).
    Outside,
    /// 작업 트리가 없는 저장소(bare).
    Bare(PathBuf),
    /// 작업 트리의 `.git` 파일이 위조됐거나 읽을 수 없다.
    InvalidGitFile(PathBuf),
}

/// `root` 를 담은 저장소를 연다. 찾지 못하면 `Ok(Err(이유))`, 찾았는데 열지 못하면(손상 등) `Err`.
pub fn discover(root: &Path) -> Result<Result<gix::Repository, NoRepo>, String> {
    use gix::discover::repository::Path as Found;
    use gix::discover::upwards::Error as NotFound;
    let found = match gix::discover::upwards(root) {
        Ok((found, _trust)) => found,
        Err(e) => {
            return match e.downcast_any_ref::<NotFound>() {
                Some(
                    NotFound::NoGitRepository { .. }
                    | NotFound::NoGitRepositoryWithinCeiling { .. }
                    | NotFound::NoGitRepositoryWithinFs { .. },
                ) => Ok(Err(NoRepo::Outside)),
                _ => Err(e.to_string()),
            };
        }
    };
    let work_dir = match found {
        Found::WorkTree(work_dir) | Found::LinkedWorkTree { work_dir, .. } => work_dir,
        Found::Repository(git_dir) => return Ok(Err(NoRepo::Bare(git_dir))),
    };
    if work_dir.join(".git").is_file() && git_file(&work_dir) == GitFile::Invalid {
        return Ok(Err(NoRepo::InvalidGitFile(work_dir)));
    }
    let repo = gix::open(&work_dir).map_err(|e| e.to_string())?;
    if repo.workdir().is_none() {
        return Ok(Err(NoRepo::Bare(repo.git_dir().to_path_buf())));
    }
    Ok(Ok(repo))
}

/// 열린 저장소의 위치 · HEAD · 진행 중 작업.
pub fn info(repo: &gix::Repository) -> Result<RepoInfo, String> {
    let work_tree = repo
        .workdir()
        .ok_or("the repository has no work tree")?
        .to_path_buf();
    let head = repo.head().map_err(|e| e.to_string())?;
    let head = match head.kind {
        gix::head::Kind::Symbolic(reference) => RepoHead::Branch {
            name: reference.name.shorten().to_string(),
            commit: reference
                .target
                .try_id()
                .map(|id| id.to_string())
                .ok_or("HEAD points to a symbolic reference chain")?,
        },
        gix::head::Kind::Unborn(name) => RepoHead::Unborn {
            name: name.as_ref().shorten().to_string(),
        },
        gix::head::Kind::Detached { target, .. } => RepoHead::Detached {
            commit: target.to_string(),
        },
    };
    Ok(RepoInfo {
        work_tree: display(&work_tree),
        git_dir: display(repo.git_dir()),
        common_dir: display(repo.common_dir()),
        linked_worktree: repo.git_dir() != repo.common_dir(),
        head,
        operation: repo.state().map(operation),
    })
}

/// git 의 `wt_status_get_state` 와 같은 판정(gix `Repository::state`)을 화면이 보일 종류로 묶는다.
fn operation(state: InProgress) -> RepoOperation {
    match state {
        InProgress::Merge => RepoOperation::Merge,
        InProgress::Rebase | InProgress::RebaseInteractive | InProgress::ApplyMailboxRebase => {
            RepoOperation::Rebase
        }
        InProgress::CherryPick | InProgress::CherryPickSequence => RepoOperation::CherryPick,
        InProgress::Revert | InProgress::RevertSequence => RepoOperation::Revert,
        InProgress::Bisect => RepoOperation::Bisect,
        InProgress::ApplyMailbox => RepoOperation::Am,
    }
}

/// 레지스트리 · 파일 RPC 와 같은 정규화(실제 경로, Windows verbatim 접두 없음)를 거친 경로.
fn display(path: &Path) -> String {
    pw_core::registry::path::normalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}
