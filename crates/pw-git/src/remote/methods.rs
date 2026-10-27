//! 원격 RPC(fetch · pull · push, push 계획). UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 쓰기는 M5 에서도 툴이 아니다). 계획 말고는
//! 모두 쓰기 실행기로 돌고 askpass 를 켠다 — 오류 코드는 `crate::write::code`(git 의 원문은 `2004` 의 `data.stderr`).

use std::sync::Arc;

use pw_rpc::{Router, rpc_method};
use serde::{Deserialize, Serialize};

use super::{
    GitPullMode, GitPushExpectedTarget, GitPushLease, GitPushRefusal, GitPushTargetSource,
    GitPushTerminal, redact,
};
use crate::write::{GitWrite, OperationInfo};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitFetchParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 원격 이름. 없으면 원격 전부(`--all`).
    #[serde(default)]
    pub remote: Option<String>,
    /// 원격에서 사라진 원격 추적 브랜치를 지운다(`--prune`).
    #[serde(default)]
    pub prune: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitFetchResult {
    /// 저장소의 작업 트리 꼭대기(절대 경로).
    pub work_tree: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPullParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub mode: GitPullMode,
    /// 원격 이름. 없으면 지금 브랜치의 upstream.
    #[serde(default)]
    pub remote: Option<String>,
    /// 원격의 브랜치 이름(`remote` 와 함께만).
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPullResult {
    pub work_tree: String,
    /// 끝난 뒤의 진행 중 작업. `null` 이면 없다.
    pub operation: Option<OperationInfo>,
    /// 충돌로 멈췄다 — 실패가 아니라 충돌을 풀고 `git.operation.continue`(또는 `abort`)할 차례다.
    pub stopped: bool,
}

/// push 입력. force 는 `forceWithLease` 하나뿐이다 — 모르는 필드(`force` 등)는 받지 않고 invalid params 로 거절한다(결정 M2-D6 ②).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitPushParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 보낼 로컬 브랜치. 없으면 HEAD 의 브랜치.
    #[serde(default)]
    pub branch: Option<String>,
    /// 원격 이름. 없으면 그 브랜치의 push 원격(`branch.<b>.pushRemote` → `remote.pushDefault` → upstream 원격 → 원격이 하나뿐이면
    /// 그것 → `origin`).
    #[serde(default)]
    pub remote: Option<String>,
    /// 원격에서 바꿀 ref(`refs/…` 전체 이름, 아니면 브랜치 이름 → `refs/heads/<이름>`). 없으면 터미널의 `git push` 가 보낼 곳을
    /// git 의 규칙대로 셈한다 — git 이 거절할 상황이면 보내지 않고 `2102`(`git.push.plan` 의 `refusal`)로 거절한다.
    #[serde(default)]
    pub remote_ref: Option<String>,
    /// 보낸 뒤 그 원격 브랜치를 upstream 으로 정한다(`--set-upstream`).
    #[serde(default)]
    pub set_upstream: bool,
    /// force push: 창이 마지막으로 본 그 원격 브랜치의 해시(`git.refs` 의 원격 추적 브랜치 — 전체 16진 해시). 원격이 그 사이
    /// 움직였으면 git 이 거절하고 원격은 그대로다(`--force-with-lease=<ref>:<해시>` + `--force-if-includes`). 창이 경고한 뒤에만
    /// 준다.
    #[serde(default)]
    pub force_with_lease: Option<String>,
    /// 창이 보인 대상(`git.push.plan` 의 `remote` · `remoteRef` 그대로). 주면 다시 센 대상이 이것과 다를 때(계획 뒤에 설정이
    /// 바뀌었을 때) 아무것도 보내지 않고 `-32004`(conflict, `data` 는 지금 셈한 `{ remote, remoteRef }`)로 끝난다.
    #[serde(default)]
    pub expected_target: Option<GitPushExpectedTarget>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushResult {
    pub work_tree: String,
    /// 보낸 로컬 브랜치의 짧은 이름.
    pub branch: String,
    /// 원격 이름(주소면 사용자 정보를 지운 것).
    pub remote: String,
    /// 원격에서 바뀐 ref(`refs/…`).
    pub remote_ref: String,
    /// 실제로 보낸 refspec(`refs/heads/<branch>:<remoteRef>`) — 대상을 명시해 보냈다(13-m2-plan.md M2-09 「push 대상 결정 규칙」).
    pub refspec: String,
    /// 터미널의 `git push` 가 이 push 와 다르게 할 일. 보통 `null`.
    pub terminal: Option<GitPushTerminal>,
}

/// push 계획 입력: `git.push` 와 같은 칸(force · upstream 설정을 뺀 것).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitPushPlanParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub remote_ref: Option<String>,
}

/// 같은 입력으로 `git.push` 를 부르면 할 일. `refusal` 이 있으면 `git.push` 는 아무것도 보내지 않고 `2102` 로 거절한다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushPlanResult {
    pub work_tree: String,
    /// 보낼 로컬 브랜치의 짧은 이름.
    pub branch: String,
    /// 원격 이름(주소면 사용자 정보를 지운 것).
    pub remote: String,
    /// 원격에서 바꿀 ref. 거절이면 `null`.
    pub remote_ref: Option<String>,
    /// 보낼 refspec(`refs/heads/<branch>:<remoteRef>`). 거절이면 `null`.
    pub refspec: Option<String>,
    /// 대상을 정한 것. 거절이면 `null`.
    pub target: Option<GitPushTargetSource>,
    /// 터미널의 `git push` 가 이 push 와 다르게 할 일. 보통 `null`.
    pub terminal: Option<GitPushTerminal>,
    /// force 의 기대값을 읽을 곳(대상의 원격 추적 ref 와 그 커밋). 원격 추적 ref 가 정해지지 않으면 `null`.
    pub lease: Option<GitPushLease>,
    pub refusal: Option<GitPushRefusal>,
}

rpc_method! {
    /// fetch: 원격 하나 또는 전부, prune 선택. 인증은 askpass 로 창에 묻는다.
    pub GitFetchMethod: "git.fetch" {
        params: GitFetchParams,
        result: GitFetchResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// pull(merge 또는 rebase). 작업 트리 전체가 미저장 확인 대상이다. 충돌로 멈추면 `stopped: true` 와 그 진행 중 작업.
    pub GitPullMethod: "git.pull" {
        params: GitPullParams,
        result: GitPullResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// push 계획: `git.push` 가 같은 입력으로 보낼 곳 · 터미널과의 차이 · force 의 기대값 · 거절 이유. gix 로만 읽는다(미신뢰
    /// 프로젝트에서도 프로세스 0). 푸시 다이얼로그(M2-20)가 보내기 전에 보인다.
    pub GitPushPlanMethod: "git.push.plan" {
        params: GitPushPlanParams,
        result: GitPushPlanResult,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// push: 로컬 브랜치 하나를 그 원격 ref 로(대상은 `git.push.plan` 과 같은 계산). upstream 설정 선택. force 는
    /// `forceWithLease` 뿐이다. git 이 거절할 대상이면 `2102`, `expectedTarget` 와 대상이 다르면 `-32004`, 원격이 앞서 있으면
    /// git 의 거절 원문으로 실패한다(`2004`).
    pub GitPushMethod: "git.push" {
        params: GitPushParams,
        result: GitPushResult,
        tier: Write,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, write: Arc<GitWrite>) {
    let w = Arc::clone(&write);
    router.unary(GitFetchMethod, move |_ctx, p: GitFetchParams| {
        let w = Arc::clone(&w);
        async move {
            let work_tree = super::fetch(&w, p.project_id, p.worktree, p.remote, p.prune).await?;
            Ok(GitFetchResult { work_tree })
        }
    });
    let w = Arc::clone(&write);
    router.unary(GitPullMethod, move |_ctx, p: GitPullParams| {
        let w = Arc::clone(&w);
        async move {
            let pulled =
                super::pull(&w, p.project_id, p.worktree, p.mode, p.remote, p.branch).await?;
            Ok(GitPullResult {
                work_tree: pulled.work_tree,
                operation: pulled.operation,
                stopped: pulled.stopped,
            })
        }
    });
    let w = Arc::clone(&write);
    router.unary(GitPushMethod, move |_ctx, p: GitPushParams| {
        let w = Arc::clone(&w);
        async move {
            let pushed = super::push(
                &w,
                p.project_id,
                p.worktree,
                p.branch,
                p.remote,
                p.remote_ref,
                p.set_upstream,
                p.force_with_lease,
                p.expected_target,
            )
            .await?;
            Ok(GitPushResult {
                work_tree: pushed.work_tree,
                refspec: pushed.plan.refspec(),
                branch: pushed.plan.branch,
                remote: redact(&pushed.plan.remote),
                remote_ref: pushed.plan.remote_ref,
                terminal: pushed.terminal,
            })
        }
    });
    let w = Arc::clone(&write);
    router.unary(GitPushPlanMethod, move |_ctx, p: GitPushPlanParams| {
        let w = Arc::clone(&w);
        async move {
            let (work_tree, planned) = super::plan(
                w.git(),
                p.project_id,
                p.worktree,
                p.branch,
                p.remote,
                p.remote_ref,
            )
            .await?;
            Ok(plan_result(work_tree, &planned))
        }
    });
}

/// 계획의 응답 꼴.
pub fn plan_result(work_tree: String, planned: &super::Planned) -> GitPushPlanResult {
    let ready = planned.ready();
    GitPushPlanResult {
        work_tree,
        branch: planned.branch.clone(),
        remote: redact(&planned.remote),
        remote_ref: ready.as_ref().ok().map(|p| p.remote_ref.clone()),
        refspec: ready.as_ref().ok().map(super::PushPlan::refspec),
        target: planned.target.as_ref().ok().map(|(_, source)| *source),
        terminal: planned.terminal_difference(),
        lease: planned.lease.clone(),
        refusal: ready.err(),
    }
}
