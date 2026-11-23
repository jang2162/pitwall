//! 에이전트 갈래의 커밋 보내기(18-m7-plan.md M7-19 — 메인 판정 2026-11-23 Q1): MCP 세션의 `checkout_send` 가 대상 머신의 받기 ·
//! 맞추기를 그 세션의 `Agent` 범위 연결로 부른다.
//!
//! 다른 쓰기 툴과 같은 꼴로 **대상 Host 가 승인을 스스로 대조한다**: 대상 Host 가 자기가 바꿀 것 — 프로젝트 · 워크트리 · 원본
//! 머신 · 브랜치 · 받을 커밋, 그리고 지금 읽은 그 브랜치의 끝 — 으로 계획 해시를 만들고([`plan`]), 세션은 계획 때 그 해시를
//! `git.send.plan` 으로 받아 사람의 승인을 얻은 뒤 승인 표에 실어 `receive.begin` · `apply` 를 부른다. 두 메서드는 `Agent` 범위면
//! 계획을 다시 만들어 표의 해시와 맞을 때만 돈다([`authorize`]) — 승인 없는 받기 · 맞추기, 승인 뒤 다른 브랜치 · 커밋으로 바꿔치기,
//! 승인 뒤 대상 브랜치가 움직인 것이 모두 「승인 필요」(`mismatch`)로 막힌다. 창의 `Project` 범위 길은 그대로다(표를 보지 않는다).
//!
//! 바뀔 경로는 해시에 넣지 않는다: 받기 전의 대상은 받을 커밋의 객체가 없어 경로를 셀 수 없다 — 받을 커밋 id 와 브랜치 끝이 그
//! 경로를 정한다(같은 두 커밋이면 같은 경로다).

use std::sync::Arc;

use pw_core::registry::plan::{ApprovalTicket, Approved, ToolPlan, approve};
use pw_rpc::{ConnectionScope, Tier};
use serde::Serialize;

use super::{branch_name, commit_id, machine_id, ref_target};
use crate::write::{GitWrite, WriteError};

/// 대상 계획의 메서드 이름(해시가 덮는다).
pub const PLAN_METHOD: &str = "git.send.apply";

/// 대상 Host 가 바꿀 것 — 계획 해시가 덮는 입력.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTarget {
    pub project_id: String,
    pub worktree: Option<String>,
    pub source_machine: String,
    pub branch: String,
    pub commit: String,
}

impl SendTarget {
    fn check(&self) -> Result<(), WriteError> {
        machine_id(&self.source_machine)?;
        branch_name(&self.branch)?;
        commit_id("commit", &self.commit)?;
        Ok(())
    }
}

/// 대상의 계획: 바꿀 것과 지금의 브랜치 끝(`tip` — 없으면 「만든다」). 보이는 글은 세션이 만든다 — 이 계획은 해시만 쓴다.
pub fn plan(target: &SendTarget, tip: Option<&str>) -> ToolPlan {
    ToolPlan::builder(PLAN_METHOD, Tier::Write, target)
        .summary(format!(
            "receive `{}` at {} from `{}` and check it out",
            target.branch, target.commit, target.source_machine
        ))
        .target("branchTip", &tip)
        .build()
}

/// 그 브랜치의 지금 끝(gix).
pub async fn branch_tip(
    write: &Arc<GitWrite>,
    project_id: &str,
    worktree: Option<&str>,
    branch: &str,
) -> Result<Option<String>, WriteError> {
    let git = Arc::clone(write.git());
    let (project_id, worktree, name) = (
        project_id.to_owned(),
        worktree.map(str::to_owned),
        format!("refs/heads/{branch}"),
    );
    tokio::task::spawn_blocking(move || {
        let opened = git
            .open(&project_id, worktree.as_deref())
            .map_err(WriteError::Rpc)?
            .ok_or(WriteError::NoRepository)?;
        Ok(ref_target(&opened.repo, &name).map(|id| id.to_string()))
    })
    .await
    .map_err(|e| WriteError::Internal(e.to_string()))?
}

/// 계획 해시와 지금의 브랜치 끝(`git.send.plan`).
pub async fn planned(
    write: &Arc<GitWrite>,
    target: &SendTarget,
) -> Result<(ToolPlan, Option<String>), WriteError> {
    target.check()?;
    let tip = branch_tip(
        write,
        &target.project_id,
        target.worktree.as_deref(),
        &target.branch,
    )
    .await?;
    Ok((plan(target, tip.as_deref()), tip))
}

/// 받기 · 맞추기를 부른 연결이 써도 되는가. 그 프로젝트 창(`Project`)이면 `None`(지금 판정 그대로 — 표를 보지 않는다). MCP
/// 세션(`Agent`)이면 그 프로젝트가 레지스트리에 있고(`agent_grant`) 다시 만든 계획을 승인 표가 지나야 `Some`. 그 밖의 범위는
/// permission denied — 받기 · 맞추기의 안쪽 판정(`scope_may_write`)이 그대로 거절하게 `None` 을 돌려준다.
pub async fn authorize(
    write: &Arc<GitWrite>,
    scope: &ConnectionScope,
    ticket: Option<&ApprovalTicket>,
    target: &SendTarget,
) -> Result<Option<Approved>, WriteError> {
    if *scope != ConnectionScope::Agent {
        return Ok(None);
    }
    {
        let git = Arc::clone(write.git());
        let (scope, project_id) = (scope.clone(), target.project_id.clone());
        tokio::task::spawn_blocking(move || {
            let host = git.host()?;
            pw_core::registry::agent::agent_grant(&scope, &host.projects, &project_id)
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))?
        .map_err(WriteError::Rpc)?;
    }
    let (plan, _) = planned(write, target).await?;
    approve(ticket, plan).map(Some).map_err(WriteError::Rpc)
}
