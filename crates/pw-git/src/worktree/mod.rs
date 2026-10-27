//! 워크트리 목록 · 체크아웃의 브랜치(13-m2-plan.md M2-13 — F5-15 의 목록 · 전환, 결정 M2-D5; 10-ui.md S1 의 웰컴 창 브랜치).
//!
//! - `git.worktrees`: 프로젝트(활성 워크트리)의 저장소에 붙은 작업 트리 — 메인 체크아웃과 연결 워크트리(`git worktree list` 와 같은
//!   차례: 메인, 그다음 관리 폴더 이름순) — 와 각 HEAD. 브랜치 팝업이 목록을 보이고, 고르면 창이 M1-25 의 워크트리 전환 경로로
//!   바꾼다(그때 보낼 `worktree` 값이 [`GitWorktree::switch_to`]). gix 로만 읽는다 — 미신뢰 프로젝트도 저장소 설정의 프로그램을
//!   돌리지 않는다(연 저장소의 열기 옵션을 그대로 쓴다, [`crate::repo::open_options`]).
//! - `git.projectBranches`: 웰컴 창의 행마다 현재 브랜치. git 도 gix 도 띄우지 않고 `HEAD` 파일만 읽는다([`read_head`]) — 웰컴
//!   창은 Host 연결로 읽기만 한다. reftable 저장소는 브랜치 대신 `unsupported` 다(웰컴은 비운다).

use std::path::Path;
use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::methods::{GitRepoParams, RepoHead};
use crate::repo::{display, head};
use crate::status::GitUnsupported;
use crate::{Git, registry_error};

/// [`read_head`] 의 답.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadBranch {
    /// HEAD 가 `refs/heads/<이름>` 을 가리킨다.
    Branch(String),
    /// 분리된 HEAD · 저장소 밖 · 읽지 못함(일반 파일이 아닌 `HEAD` · `.git` 포함).
    None,
    /// 이 읽기가 알 수 없는 저장소 형식. reftable 저장소의 `HEAD` 는 자리표시 `ref: refs/heads/.invalid` 라(진짜 HEAD 는
    /// `reftable/` 안에 있다) 파일만 읽어서는 브랜치를 모른다 — 상태바의 「지원하지 않는 저장소」 와 같은 까닭이다.
    Unsupported(GitUnsupported),
}

/// git 의 reftable 저장소가 `HEAD` 파일에 적는 자리표시 브랜치. `refs/heads/` 아래 이름은 `.` 으로 시작할 수 없어 진짜 브랜치와
/// 겹치지 않는다.
const REFTABLE_PLACEHOLDER: &str = ".invalid";

/// 이보다 긴 `HEAD` · `.git` 파일은 읽지 않는다(둘 다 한 줄이다).
const MAX_POINTER_FILE: u64 = 4096;

/// `path` 가 일반 파일일 때만 앞 [`MAX_POINTER_FILE`] 바이트를 읽는다. FIFO · 장치 파일은 열면서 막히지 않게 `O_NONBLOCK` 으로
/// 열고(unix), 연 핸들의 종류를 본 뒤 읽는다 — 이름으로 본 뒤 여는 사이에 바뀌어도 막히지 않는다. 압축으로 받은 저장소에는
/// `HEAD` 가 FIFO 일 수 있다(클론으로는 생기지 않는다).
fn read_pointer_file(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_POINTER_FILE).read_to_string(&mut text).ok()?;
    Some(text)
}

/// `start` 를 담은 git 체크아웃의 HEAD 가 가리키는 브랜치. git 을 실행하지 않는다 — 가장 가까운 `.git`(폴더, 또는 연결 워크트리 ·
/// 서브모듈의 `gitdir:` 파일)의 `HEAD` 파일만 읽는다. 일반 파일이 아니면 읽지 않는다([`read_pointer_file`]).
pub fn read_head(start: &Path) -> HeadBranch {
    let Some(dot_git) = start
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|p| p.exists())
    else {
        return HeadBranch::None;
    };
    let gitdir = if dot_git.is_dir() {
        dot_git
    } else {
        let Some(text) = read_pointer_file(&dot_git) else {
            return HeadBranch::None;
        };
        let Some(pointer) = text
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("gitdir:"))
            .map(str::trim)
        else {
            return HeadBranch::None;
        };
        let pointer = Path::new(pointer);
        if pointer.is_absolute() {
            pointer.to_path_buf()
        } else {
            match dot_git.parent() {
                Some(parent) => parent.join(pointer),
                None => return HeadBranch::None,
            }
        }
    };
    let Some(head) = read_pointer_file(&gitdir.join("HEAD")) else {
        return HeadBranch::None;
    };
    let name = head.trim().strip_prefix("ref: refs/heads/");
    if name == Some(REFTABLE_PLACEHOLDER) || gitdir.join("reftable").is_dir() {
        return HeadBranch::Unsupported(GitUnsupported::Reftable);
    }
    match name {
        Some(name) if !name.is_empty() => HeadBranch::Branch(name.to_owned()),
        _ => HeadBranch::None,
    }
}

/// `start` 를 담은 체크아웃의 현재 브랜치([`read_head`]). 분리된 HEAD · 저장소 밖 · 읽기 실패 · 알 수 없는 형식은 `None`.
pub fn read_branch(start: &Path) -> Option<String> {
    match read_head(start) {
        HeadBranch::Branch(name) => Some(name),
        HeadBranch::None | HeadBranch::Unsupported(_) => None,
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktrees {
    /// 저장소의 작업 트리 꼭대기(요청이 본 것). `null` 이면 「저장소 없음」 이고 목록은 빈다.
    pub work_tree: Option<String>,
    /// 메인 체크아웃(bare 저장소면 없다), 그다음 연결 워크트리(관리 폴더 이름순).
    pub worktrees: Vec<GitWorktree>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktree {
    /// 작업 트리 꼭대기(절대 경로).
    pub path: String,
    /// 그 작업 트리의 HEAD. 폴더가 사라졌거나 읽지 못하면 `null`.
    pub head: Option<RepoHead>,
    /// 저장소의 메인 체크아웃.
    pub main: bool,
    /// 요청한 창이 보고 있는 작업 트리.
    pub current: bool,
    /// `git worktree lock` 으로 잠겼다.
    pub locked: bool,
    /// 폴더가 없다(`git worktree prune` 이 치울 것).
    pub missing: bool,
    /// 이 프로젝트 창이 이 작업 트리로 바꿀 때 보낼 값. 이 프로젝트의 루트를 그 작업 트리 안에 둘 수 없으면(위조 판정 ·
    /// 다른 저장소 체크아웃에 등록된 프로젝트) `null` — 목록에만 보인다.
    pub switch_to: Option<GitWorktreeSwitch>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktreeSwitch {
    /// 활성 워크트리(`ActiveWorktree.top`). `null` 이면 프로젝트의 메인 체크아웃(주 루트의 작업 트리)으로 돌아간다.
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitProjectBranchesParams {}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitProjectBranches {
    /// 이 Host 의 프로젝트(레지스트리 차례).
    pub projects: Vec<GitProjectBranch>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitProjectBranch {
    pub project_id: String,
    /// 주 루트를 담은 체크아웃의 브랜치. 분리된 HEAD · 저장소 밖 · 읽지 못함 · 알 수 없는 형식이면 `null`.
    pub branch: Option<String>,
    /// 파일만 읽어서는 브랜치를 알 수 없는 저장소 형식(reftable)이면 그 까닭(`git.refs` 의 `unsupported` 와 같은 값). 보통 `null`.
    pub unsupported: Option<GitUnsupported>,
}

impl Git {
    /// `git.worktrees`.
    pub fn worktrees(&self, params: &GitRepoParams) -> Result<GitWorktrees, RpcError> {
        let Some(opened) = self.open(&params.project_id, params.worktree.as_deref())? else {
            return Ok(GitWorktrees {
                work_tree: None,
                worktrees: Vec::new(),
            });
        };
        let project = self
            .host()?
            .projects
            .get(&params.project_id)
            .map_err(registry_error)?;
        // 프로젝트의 메인 체크아웃: 주 루트를 담은 작업 트리. 이 작업 트리로 바꾸는 값은 `worktree: null` 이다.
        let home = match params.worktree {
            None => Some(opened.target.work_tree.clone()),
            Some(_) => self
                .open(&params.project_id, None)?
                .map(|o| o.target.work_tree),
        };
        let current = display(&opened.target.work_tree);
        let home = home.map(|h| display(&h));
        let switch_to = |path: &str| {
            if home.as_deref() == Some(path) {
                return Some(GitWorktreeSwitch { worktree: None });
            }
            pw_core::registry::path::worktree_root(project.root(), Path::new(path))
                .ok()
                .map(|_| GitWorktreeSwitch {
                    worktree: Some(path.to_owned()),
                })
        };
        let read_error = |e: gix::Error| {
            RpcError::internal(format!("cannot list the worktrees of {current}: {e}"))
        };

        let repo = &opened.repo;
        let mut worktrees = Vec::new();
        let main = repo.main_repo().map_err(read_error)?;
        if let Some(top) = main.workdir() {
            let path = display(top);
            worktrees.push(GitWorktree {
                head: head(&main).ok(),
                main: true,
                current: path == current,
                locked: false,
                missing: !top.is_dir(),
                switch_to: switch_to(&path),
                path,
            });
        }
        for proxy in repo.worktrees().map_err(read_error)? {
            let locked = proxy.is_locked();
            let Ok(base) = proxy.base() else { continue };
            let path = display(&base);
            let missing = !base.is_dir();
            let head = proxy
                .into_repo_with_possibly_inaccessible_worktree()
                .ok()
                .and_then(|r| head(&r).ok());
            worktrees.push(GitWorktree {
                head,
                main: false,
                current: path == current,
                locked,
                missing,
                switch_to: if missing { None } else { switch_to(&path) },
                path,
            });
        }
        Ok(GitWorktrees {
            work_tree: Some(current),
            worktrees,
        })
    }

    /// `git.projectBranches`.
    pub fn project_branches(&self) -> Result<GitProjectBranches, RpcError> {
        let projects = self.host()?.projects.list().map_err(registry_error)?;
        Ok(GitProjectBranches {
            projects: projects
                .into_iter()
                .map(|p| {
                    let (branch, unsupported) = match read_head(p.root()) {
                        HeadBranch::Branch(name) => (Some(name), None),
                        HeadBranch::None => (None, None),
                        HeadBranch::Unsupported(reason) => (None, Some(reason)),
                    };
                    GitProjectBranch {
                        project_id: p.id,
                        branch,
                        unsupported,
                    }
                })
                .collect(),
        })
    }
}

rpc_method! {
    /// 저장소의 작업 트리 — 메인 체크아웃과 연결 워크트리 — 와 각 HEAD(gix). 저장소가 없으면 `workTree: null` 과 빈 목록. 각 항목의
    /// `switchTo` 가 프로젝트 창이 그 작업 트리로 바꿀 때 보낼 활성 워크트리다.
    pub GitWorktreesMethod: "git.worktrees" {
        params: GitRepoParams,
        result: GitWorktrees,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 프로젝트마다 주 루트를 담은 체크아웃의 브랜치(웰컴 창). git 을 실행하지 않고 `HEAD` 파일을 읽는다 — 분리된 HEAD 면 `null`.
    pub GitProjectBranchesMethod: "git.projectBranches" {
        params: GitProjectBranchesParams,
        result: GitProjectBranches,
        tier: Read,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 디스크 IO 라 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, git: Arc<Git>) {
    let g = Arc::clone(&git);
    router.unary(GitWorktreesMethod, move |_ctx, params: GitRepoParams| {
        let git = Arc::clone(&g);
        async move {
            tokio::task::spawn_blocking(move || git.worktrees(&params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
    router.unary(
        GitProjectBranchesMethod,
        move |_ctx, _params: GitProjectBranchesParams| {
            let git = Arc::clone(&git);
            async move {
                tokio::task::spawn_blocking(move || git.project_branches())
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
            }
        },
    );
}
