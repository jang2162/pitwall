//! 원격 RPC(fetch · pull · push). UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 쓰기는 M5 에서도 툴이 아니다). 모두 쓰기
//! 실행기로 돌고 askpass 를 켠다 — 오류 코드는 `crate::write::code`(git 의 원문은 `2004` 의 `data.stderr`).

use std::sync::Arc;

use pw_rpc::{Router, rpc_method};
use serde::{Deserialize, Serialize};

use super::{GitPullMode, GitPushConfigured};
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
    /// 원격 이름. 없으면 그 브랜치의 push 원격(`branch.<b>.pushRemote` → `remote.pushDefault` → upstream 원격 → `origin`).
    #[serde(default)]
    pub remote: Option<String>,
    /// 보낸 뒤 그 원격 브랜치를 upstream 으로 정한다(`--set-upstream`).
    #[serde(default)]
    pub set_upstream: bool,
    /// force push: 창이 마지막으로 본 그 원격 브랜치의 해시(`git.refs` 의 원격 추적 브랜치 — 전체 16진 해시). 원격이 그 사이
    /// 움직였으면 git 이 거절하고 원격은 그대로다(`--force-with-lease=<ref>:<해시>` + `--force-if-includes`). 창이 경고한 뒤에만
    /// 준다.
    #[serde(default)]
    pub force_with_lease: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushResult {
    pub work_tree: String,
    /// 보낸 로컬 브랜치의 짧은 이름.
    pub branch: String,
    pub remote: String,
    /// 원격에서 바뀐 ref(`refs/heads/…`).
    pub remote_ref: String,
    /// 실제로 보낸 refspec(`refs/heads/<branch>:<remoteRef>`). 설정의 push refspec · `push.default` 를 쓰지 않고 이 값을 명시해
    /// 보냈다(13-m2-plan.md M2-09 「push 대상 결정 규칙」).
    pub refspec: String,
    /// 설정의 push refspec(`remote.<remote>.push`)이 이 push 와 다른 곳을 가리키면 그 내용 — 푸시 다이얼로그가 경고로 보인다.
    /// 보통 `null`.
    pub configured: Option<GitPushConfigured>,
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
    /// push: 로컬 브랜치 하나를 그 원격 브랜치로. upstream 설정 선택. force 는 `forceWithLease` 뿐이다. 원격이 앞서 있으면 git 의
    /// 거절 원문으로 실패한다(`2004`).
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
                p.set_upstream,
                p.force_with_lease,
            )
            .await?;
            Ok(GitPushResult {
                work_tree: pushed.work_tree,
                refspec: pushed.plan.refspec(),
                branch: pushed.plan.branch,
                remote: pushed.plan.remote,
                remote_ref: pushed.plan.remote_ref,
                configured: pushed.configured,
            })
        }
    });
}
