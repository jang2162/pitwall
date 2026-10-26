//! 이력 쓰기 RPC. UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 쓰기는 M5 에서도 툴이 아니다).
//!
//! - `git.merge` · `git.cherryPick` · `git.revert` · `git.rebase`: 결과는 [`GitHistoryResult`] — 충돌이면 `stopped` 와 진행 중 작업.
//! - `git.reset`: soft · mixed · hard. hard 만 작업 트리를 바꿔 미저장 확인을 묻는다. `expected` 로 [되돌리기] 가 그사이 움직인
//!   HEAD 를 덮지 않게 한다.
//! - `git.resetPreview`: reset hard 가 잃을 변경(읽기 — 쓰기 잠금을 기다리지 않는다).

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::lost::{self, GitLostChanges};
use super::{
    FastForward, GitHistoryResult, GitResetResult, Job, ResetMode, Watched, check_rev, display,
};
use crate::write::{Changes, GitWrite, WriteError};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitMergeParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 들여올 리비전(브랜치 이름이면 merge 메시지가 그 이름을 쓴다).
    pub rev: String,
    #[serde(default)]
    pub fast_forward: FastForward,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitsParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 적용할 커밋들, 적용할 차례대로(로그에서 고른 것이면 오래된 것부터). 비면 invalid params.
    pub commits: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitRebaseParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 이 위로 옮긴다(`git rebase <upstream>`).
    pub upstream: String,
    /// rebase 할 로컬 브랜치의 짧은 이름. 있으면 git 이 그 브랜치로 체크아웃한 뒤 rebase 한다. 없으면 지금 HEAD.
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitResetParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// HEAD 를 옮길 커밋.
    pub commit: String,
    pub mode: ResetMode,
    /// HEAD 가 이 커밋(전체 해시)일 때만 한다 — 아니면 아무것도 하지 않고 conflict(`-32004`, `data.head` 가 지금 HEAD). 확인
    /// 다이얼로그를 띄운 때의 HEAD(`git.resetPreview` 의 `head`)나 [되돌리기] 의 `moved.current` 를 넣는다.
    #[serde(default)]
    pub expected: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitResetPreviewParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub commit: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitResetPreview {
    pub work_tree: String,
    /// 지금 HEAD 의 커밋(`git.reset` 의 `expected` 로 넘긴다). 태어나지 않은 브랜치면 `null`.
    pub head: Option<String>,
    /// `commit` 이 가리키는 커밋(전체 해시).
    pub target: String,
    pub lost: GitLostChanges,
}

rpc_method! {
    /// 리비전을 지금 브랜치에 merge 한다(`git merge --no-edit`, fast-forward 선택). 충돌이면 `outcome: stopped` 와 진행 중 작업.
    /// git 이 거절하면(덮어써질 로컬 변경, `--ff-only` 로 못 함 등) 원문으로 실패한다.
    pub GitMergeMethod: "git.merge" {
        params: GitMergeParams,
        result: GitHistoryResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 커밋들을 차례로 cherry-pick 한다. 하나가 충돌하면 앞의 것은 커밋된 채 `outcome: stopped` — 나머지는
    /// `git.operation.continue` 가 잇는다.
    pub GitCherryPickMethod: "git.cherryPick" {
        params: GitCommitsParams,
        result: GitHistoryResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 커밋들을 차례로 되돌리는 역 커밋을 만든다(`git revert --no-edit`). 충돌이면 `outcome: stopped`.
    pub GitRevertMethod: "git.revert" {
        params: GitCommitsParams,
        result: GitHistoryResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 비대화형 rebase(`git rebase <upstream> [<branch>]`). 충돌이면 `outcome: stopped` 와 진행 중 작업, 끝나면 `moved` 가 그 브랜치의
    /// 이전 끝.
    pub GitRebaseMethod: "git.rebase" {
        params: GitRebaseParams,
        result: GitHistoryResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// HEAD 를 커밋으로 옮긴다(`git reset --soft|--mixed|--hard`). hard 는 확인 뒤에 부른다(결정 M2-D6 ① — 잃을 변경은
    /// `git.resetPreview`). 결과의 `moved` 로 [되돌리기].
    pub GitResetMethod: "git.reset" {
        params: GitResetParams,
        result: GitResetResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// reset hard 가 잃을 변경: 커밋하지 않은 추적 파일의 변경과 덮어써질 untracked 파일. gix 로 읽는다(git 을 띄우지 않는다).
    pub GitResetPreviewMethod: "git.resetPreview" {
        params: GitResetPreviewParams,
        result: GitResetPreview,
        tier: Read,
        roles: [Owner],
    }
}

/// Task 제목(`extensions/i18n-default/i18n/*/project.ftl`).
pub const TITLE_MERGE: &str = "task-title-git-merge";
pub const TITLE_CHERRY_PICK: &str = "task-title-git-cherry-pick";
pub const TITLE_REVERT: &str = "task-title-git-revert";
pub const TITLE_REBASE: &str = "task-title-git-rebase";
pub const TITLE_RESET: &str = "task-title-git-reset";

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, write: Arc<GitWrite>) {
    let w = Arc::clone(&write);
    router.unary(GitMergeMethod, move |_ctx, params| {
        merge(Arc::clone(&w), params)
    });
    let w = Arc::clone(&write);
    router.unary(GitCherryPickMethod, move |_ctx, params| {
        pick(Arc::clone(&w), params, Pick::CherryPick)
    });
    let w = Arc::clone(&write);
    router.unary(GitRevertMethod, move |_ctx, params| {
        pick(Arc::clone(&w), params, Pick::Revert)
    });
    let w = Arc::clone(&write);
    router.unary(GitRebaseMethod, move |_ctx, params| {
        rebase(Arc::clone(&w), params)
    });
    let w = Arc::clone(&write);
    router.unary(GitResetMethod, move |_ctx, params| {
        reset(Arc::clone(&w), params)
    });
    let w = Arc::clone(&write);
    router.unary(GitResetPreviewMethod, move |_ctx, params| {
        reset_preview(Arc::clone(&w), params)
    });
}

/// `git.merge`.
pub async fn merge(
    write: Arc<GitWrite>,
    params: GitMergeParams,
) -> Result<GitHistoryResult, RpcError> {
    check_rev("rev", &params.rev)?;
    let mut args: Vec<OsString> = vec!["merge".into(), "--no-edit".into()];
    match params.fast_forward {
        FastForward::Allow => {}
        FastForward::Only => args.push("--ff-only".into()),
        FastForward::Never => args.push("--no-ff".into()),
    }
    args.push(params.rev.into());
    let job = Job {
        request: super::request(
            &params.project_id,
            params.worktree.as_deref(),
            TITLE_MERGE,
            Changes::WorkTree,
        ),
        watched: Watched::Head,
        args,
        classify: super::merge_outcome,
    };
    Ok(super::run(&write, job).await?)
}

#[derive(Debug, Clone, Copy)]
pub enum Pick {
    CherryPick,
    Revert,
}

/// `git.cherryPick` · `git.revert`.
pub async fn pick(
    write: Arc<GitWrite>,
    params: GitCommitsParams,
    kind: Pick,
) -> Result<GitHistoryResult, RpcError> {
    if params.commits.is_empty() {
        return Err(RpcError::invalid_params("`commits` must not be empty"));
    }
    for commit in &params.commits {
        check_rev("commits", commit)?;
    }
    let (args, title): (Vec<OsString>, _) = match kind {
        Pick::CherryPick => (vec!["cherry-pick".into()], TITLE_CHERRY_PICK),
        Pick::Revert => (vec!["revert".into(), "--no-edit".into()], TITLE_REVERT),
    };
    let args = args
        .into_iter()
        .chain(params.commits.into_iter().map(OsString::from))
        .collect();
    let job = Job {
        request: super::request(
            &params.project_id,
            params.worktree.as_deref(),
            title,
            Changes::WorkTree,
        ),
        watched: Watched::Head,
        args,
        classify: super::rewrite_outcome,
    };
    Ok(super::run(&write, job).await?)
}

/// `git.rebase`.
pub async fn rebase(
    write: Arc<GitWrite>,
    params: GitRebaseParams,
) -> Result<GitHistoryResult, RpcError> {
    check_rev("upstream", &params.upstream)?;
    let mut args: Vec<OsString> = vec!["rebase".into(), params.upstream.into()];
    let watched = match params.branch {
        Some(branch) => {
            check_rev("branch", &branch)?;
            let name = format!("refs/heads/{branch}");
            args.push(branch.into());
            Watched::Ref(name)
        }
        None => Watched::Head,
    };
    let job = Job {
        request: super::request(
            &params.project_id,
            params.worktree.as_deref(),
            TITLE_REBASE,
            Changes::WorkTree,
        ),
        watched,
        args,
        classify: super::rewrite_outcome,
    };
    Ok(super::run(&write, job).await?)
}

/// `git.reset`.
pub async fn reset(
    write: Arc<GitWrite>,
    params: GitResetParams,
) -> Result<GitResetResult, RpcError> {
    check_rev("commit", &params.commit)?;
    let changes = match params.mode {
        ResetMode::Hard => Changes::WorkTree,
        ResetMode::Soft | ResetMode::Mixed => Changes::None,
    };
    let request = super::request(
        &params.project_id,
        params.worktree.as_deref(),
        TITLE_RESET,
        changes,
    );
    Ok(super::reset(&write, request, params.commit, params.mode, params.expected).await?)
}

/// `git.resetPreview`. 요청이 취소되면(이 future 가 버려지면) status 계산도 멈춘다.
pub async fn reset_preview(
    write: Arc<GitWrite>,
    params: GitResetPreviewParams,
) -> Result<GitResetPreview, RpcError> {
    check_rev("commit", &params.commit)?;
    let git = Arc::clone(write.git());
    let interrupt = Arc::new(AtomicBool::new(false));
    let _stop = StopOnDrop(Arc::clone(&interrupt));
    tokio::task::spawn_blocking(move || {
        let opened = git
            .open(&params.project_id, params.worktree.as_deref())?
            .ok_or_else(|| RpcError::from(WriteError::NoRepository))?;
        let head = opened.repo.head_id().ok().map(|id| id.to_string());
        let (target, lost) = lost::lost(&opened, &params.commit, Some(&interrupt))?;
        Ok(GitResetPreview {
            work_tree: display(&opened.target.work_tree),
            head,
            target,
            lost,
        })
    })
    .await
    .map_err(|e| RpcError::internal(e.to_string()))?
}

struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
