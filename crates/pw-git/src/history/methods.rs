//! 이력 쓰기 RPC. UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 쓰기는 M5 에서도 툴이 아니다).
//!
//! - `git.merge` · `git.cherryPick` · `git.revert` · `git.rebase`: 결과는 [`GitHistoryResult`] — 충돌이면 `stopped` 와 진행 중 작업.
//! - `git.reset`: soft · mixed · hard · keep. hard · keep 은 작업 트리를 바꿔 미저장 확인을 묻는다. `expected` · `expectedRef` 로
//!   [되돌리기] 가 그사이 움직인 HEAD 나 옮겨 간 브랜치를 덮지 않게 하고, hard(늘) · keep · mixed(잃을 것이 있을 때)는 미리보기의
//!   `fingerprint` 로 확인한 뒤 바뀐 작업 트리 · index 를 지우지 않게 한다.
//! - `git.resetPreview`: reset hard · keep · mixed 가 잃을 변경(읽기 — 쓰기 잠금을 기다리지 않는다).

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pw_rpc::{ConnectionScope, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::lost::{self, GitLostChanges};
use super::{
    FastForward, GitHistoryResult, GitResetResult, Job, ResetGuard, ResetMode, Watched, check_rev,
    display,
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
    /// HEAD 가 이 커밋(전체 해시)일 때만 한다 — 아니면 아무것도 하지 않고 conflict(`-32004`, `data.head` 가 지금 HEAD ·
    /// `data.refName` 이 HEAD 의 ref). [되돌리기] 의 `moved.current` 를 넣는다.
    #[serde(default)]
    pub expected: Option<String>,
    /// HEAD 가 가리키는 ref(전체 이름, 분리돼 있으면 `HEAD`)가 이것일 때만 한다 — 아니면 conflict. [되돌리기] 의 `moved.refName`
    /// 을 넣는다(같은 커밋의 다른 브랜치로 옮겨 갔으면 그 브랜치를 되돌리지 않는다).
    #[serde(default)]
    pub expected_ref: Option<String>,
    /// 확인 다이얼로그에 보인 `git.resetPreview`(같은 `mode`)의 `fingerprint`. hard 에는 꼭 있어야 하고, keep · mixed 는 잃을 것이 있으면
    /// 있어야 한다(없이 부르면 conflict 로 그 목록을 받는다 — [되돌리기] 가 그때 묻는다). 잠금 안에서 다시 읽은 잃을 변경(HEAD
    /// 포함)의 지문이 다르면 아무것도 하지 않고 conflict — `data` 에 새 `head` · `lost` · `fingerprint`(다시 물을 때 쓴다,
    /// `fingerprint` 가 `null` 이면 확인할 수 없다).
    #[serde(default)]
    pub fingerprint: Option<String>,
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
    /// `hard`(기본) · `keep`(되돌리기) · `mixed`(index 에만 있는 스테이징 내용). `soft` 면 invalid params.
    #[serde(default = "hard")]
    pub mode: ResetMode,
}

fn hard() -> ResetMode {
    ResetMode::Hard
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
    /// 이 미리보기의 지문 — 확인한 뒤 `git.reset`(같은 `mode`)의 `fingerprint` 로 넘긴다. `null` 이면 잃을 폴더(`dir/`) 아래가
    /// 너무 많아(`lost::FINGERPRINT_ENTRIES` 개 초과) 확인할 수 없다 — 그 reset 은 할 수 없다(덜 보고 셈하지 않는다).
    pub fingerprint: Option<String>,
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
    /// HEAD 를 커밋으로 옮긴다(`git reset --soft|--mixed|--hard|--keep`, 서브모듈 작업 트리는 그대로). hard 와 잃을 것이 있는 keep · mixed 는
    /// 확인 뒤에 미리보기의 지문과 함께 부른다(결정 M2-D6 ① — 잃을 변경은 `git.resetPreview`). 결과의 `moved` 로 [되돌리기].
    pub GitResetMethod: "git.reset" {
        params: GitResetParams,
        result: GitResetResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// reset hard · keep · mixed 가 잃을 변경: 커밋하지 않은 추적 파일의 변경과 덮어써질 추적하지 않는 것(mixed 는 index 에만 있는
    /// 스테이징 내용). gix 로 읽는다(git 을 띄우지 않는다).
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
    router.unary(GitMergeMethod, move |ctx, params| {
        merge(Arc::clone(&w), ctx.scope().clone(), params)
    });
    let w = Arc::clone(&write);
    router.unary(GitCherryPickMethod, move |ctx, params| {
        pick(
            Arc::clone(&w),
            ctx.scope().clone(),
            params,
            Pick::CherryPick,
        )
    });
    let w = Arc::clone(&write);
    router.unary(GitRevertMethod, move |ctx, params| {
        pick(Arc::clone(&w), ctx.scope().clone(), params, Pick::Revert)
    });
    let w = Arc::clone(&write);
    router.unary(GitRebaseMethod, move |ctx, params| {
        rebase(Arc::clone(&w), ctx.scope().clone(), params)
    });
    let w = Arc::clone(&write);
    router.unary(GitResetMethod, move |ctx, params| {
        reset(Arc::clone(&w), ctx.scope().clone(), params)
    });
    let w = Arc::clone(&write);
    router.unary(GitResetPreviewMethod, move |_ctx, params| {
        reset_preview(Arc::clone(&w), params)
    });
}

/// `git.merge`.
pub async fn merge(
    write: Arc<GitWrite>,
    caller: ConnectionScope,
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
            caller,
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
    caller: ConnectionScope,
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
            caller,
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
    caller: ConnectionScope,
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
            caller,
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
    caller: ConnectionScope,
    params: GitResetParams,
) -> Result<GitResetResult, RpcError> {
    check_rev("commit", &params.commit)?;
    if params.mode == ResetMode::Hard && params.fingerprint.is_none() {
        return Err(RpcError::invalid_params(
            "a hard reset needs the `fingerprint` of the confirmed `git.resetPreview`",
        ));
    }
    let changes = match params.mode {
        ResetMode::Hard | ResetMode::Keep => Changes::WorkTree,
        ResetMode::Soft | ResetMode::Mixed => Changes::None,
    };
    let request = super::request(
        caller,
        &params.project_id,
        params.worktree.as_deref(),
        TITLE_RESET,
        changes,
    );
    let guard = ResetGuard {
        expected: params.expected,
        expected_ref: params.expected_ref,
        fingerprint: params.fingerprint,
    };
    Ok(super::reset(&write, request, params.commit, params.mode, guard).await?)
}

/// `git.resetPreview`. 요청이 취소되면(이 future 가 버려지면) status 계산도 멈춘다.
pub async fn reset_preview(
    write: Arc<GitWrite>,
    params: GitResetPreviewParams,
) -> Result<GitResetPreview, RpcError> {
    check_rev("commit", &params.commit)?;
    if params.mode == ResetMode::Soft {
        return Err(RpcError::invalid_params(
            "a reset preview is for `hard`, `keep` or `mixed`",
        ));
    }
    let git = Arc::clone(write.git());
    let interrupt = Arc::new(AtomicBool::new(false));
    let _stop = StopOnDrop(Arc::clone(&interrupt));
    tokio::task::spawn_blocking(move || {
        let opened = git
            .open(&params.project_id, params.worktree.as_deref())?
            .ok_or_else(|| RpcError::from(WriteError::NoRepository))?;
        let read = lost::lost(&opened, &params.commit, params.mode, Some(&interrupt))?;
        Ok(GitResetPreview {
            work_tree: display(&opened.target.work_tree),
            head: read.head,
            target: read.target,
            lost: read.lost,
            fingerprint: read.fingerprint,
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
