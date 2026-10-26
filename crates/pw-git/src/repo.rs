//! 저장소 해석(결정 M2-D3): 프로젝트의 git 기능이 다루는 저장소 하나를 찾고, 그 HEAD 와 진행 중 작업을 gix 로 읽는다.
//!
//! - 대상은 활성 워크트리 안의 프로젝트 루트(없으면 주 루트)를 담은 저장소다. 프로젝트 루트가 저장소의 하위 폴더(모노레포의
//!   일부)여도 저장소 전체다. 찾기는 git 과 같이 루트에서 위로 가장 가까운 `.git` 이다.
//! - 찾은 작업 트리의 `.git` 이 파일이면 M1-05 의 위조 판정(`pw_core::config::gitdir::git_file`)을 거친다 — 관리 폴더가 그
//!   저장소의 `worktrees/<이름>` 바로 아래가 아니거나 되돌림 포인터가 맞지 않는 연결 워크트리, 관리 폴더의 `core.worktree` 가 이
//!   폴더를 되가리키지 않는 서브모듈(다른 저장소의 `.git` 을 직접 가리키는 `.git` 파일 포함)은 저장소로 보지 않는다.
//! - 저장소 밖, bare 저장소(작업 트리가 없다), 위조 `.git`, `core.worktree` 가 `.git` 이 있는 폴더가 아닌 곳을 가리키는 저장소는
//!   모두 「저장소 없음」 이다 — git 기능 전체가 빈 상태가 된다(10-ui.md 「빈 상태 · 오류 상태」). 마지막 것은 git CLI 가 작업 트리에서
//!   다시 찾으면 다른 저장소(또는 없음)를 보기 때문이다 — 읽기(gix)와 쓰기(CLI)가 언제나 같은 `.git` 을 보게 한다.
//! - 미신뢰 프로젝트의 저장소는 저장소 안의 설정(`.git/config` · `config.worktree` 와 그것이 include 한 파일)을 프로그램 · 경로를
//!   정하는 데 쓰지 않고 연다([`open_options`]) — 이후 행이 gix 로 status · diff 를 읽을 때 저장소가 정한 필터 드라이버
//!   (`filter.<x>.clean` · `process`) · `diff.<x>.textconv` 가 돌지 않는다(결정 M2-D8, 03-architecture.md 「보안」). 신뢰하면 gix 의
//!   기본(소유자 · `safe.directory` 판정)대로 연다. 사람의 전역 설정은 어느 쪽이든 쓴다.
//!
//! git 프로세스를 띄우지 않는다(결정 M2-D8 「읽기는 gix」). 이 크레이트가 켠 gix 기능에는 프로그램을 띄우는 경로(`gix-command` ·
//! `gix-filter`)가 아예 없다 — 위 열기 옵션은 그 기능을 켜는 행을 위한 것이다.

use std::borrow::Cow;
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
    /// `.git` 은 `found` 에 있는데 `core.worktree` 가 작업 트리를 다른 폴더 `work_tree` 로 정했다.
    WorkTreeElsewhere { found: PathBuf, work_tree: PathBuf },
}

/// 저장소를 여는 옵션. `trusted` 가 아니면 저장소 안의 설정 섹션을 프로그램 · 경로 · 원격처럼 gix 가 「믿을 만한 설정에서만」 읽는
/// 값에 쓰지 않는다(gix 의 `filter_config_section`). 그 밖의 값(`core.bare` · `core.worktree` · 객체 형식)은 그대로 읽는다.
///
/// gix 의 기본 판정은 섹션의 신뢰(소유자)로 가르는데, 사람이 전역 설정에 `safe.directory = *` 를 두면 남의 압축에서 풀린 저장소도
/// 믿는다. 미신뢰 프로젝트는 소유자 · `safe.directory` 와 상관없이 저장소 안의 설정을 버린다.
pub fn open_options(trusted: bool) -> gix::open::Options {
    let options = gix::open::Options::default();
    if trusted {
        options
    } else {
        options.filter_config_section(outside_the_repository)
    }
}

fn outside_the_repository(meta: &gix::config::file::Metadata) -> bool {
    meta.source.kind() != gix::config::source::Kind::Repository
}

/// `root` 를 담은 저장소를 연다(`trusted` 는 [`open_options`]). 찾지 못하면 `Ok(Err(이유))`, 찾았는데 열지 못하면(손상 등) `Err`.
pub fn discover(root: &Path, trusted: bool) -> Result<Result<gix::Repository, NoRepo>, String> {
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
    let repo = gix::open_opts(&work_dir, open_options(trusted)).map_err(|e| e.to_string())?;
    if repo.workdir().is_none() {
        return Ok(Err(NoRepo::Bare(repo.git_dir().to_path_buf())));
    }
    // gix 는 신뢰로 열 때만 저장소 설정의 `core.worktree` 를 쓰고(그것도 「믿을 만한 설정에서만」 읽는 값이다), git CLI 는 언제나
    // 쓴다. 값이 있으면 둘 다 그 폴더를 작업 트리로 보므로 값 자체로 가른다.
    if let Some(work_tree) = configured_work_tree(&repo)
        && !same_dir(&work_tree, &work_dir)
    {
        return Ok(Err(NoRepo::WorkTreeElsewhere {
            found: work_dir,
            work_tree,
        }));
    }
    Ok(Ok(repo))
}

/// 저장소 자신의 설정 파일(git 디렉터리 · 공유 git 디렉터리 안)에 적힌 `core.worktree` 가 가리키는 폴더. git 처럼 마지막 값이고,
/// 상대 경로는 git 디렉터리 기준이다. 값이 없으면 `None`.
fn configured_work_tree(repo: &gix::Repository) -> Option<PathBuf> {
    let (git_dir, common_dir) = (repo.git_dir(), repo.common_dir());
    let value = repo.config_snapshot().plumbing().string_filter(
        "core.worktree",
        |meta: &gix::config::file::Metadata| {
            meta.path
                .as_deref()
                .is_some_and(|p| p.starts_with(git_dir) || p.starts_with(common_dir))
        },
    )?;
    // 경로로 바꾸지 못하는 값(Windows 의 UTF-8 아닌 바이트)은 빈 경로 — 어느 폴더와도 같지 않아 「저장소 없음」 이 된다.
    let Ok(value) = gix::path::from_bstr(value).map(Cow::into_owned) else {
        return Some(PathBuf::new());
    };
    Some(if value.is_absolute() {
        value
    } else {
        git_dir.join(value)
    })
}

/// 두 경로가 실제 경로로 같은 폴더인가. 실제 경로를 못 구하면 다르다.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
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
