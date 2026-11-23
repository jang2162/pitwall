//! 원본 쪽 읽기(`git.send.compare` — 18-m7-plan M7-19): 에이전트의 `checkout_send` 계획이 원본에서 읽는 것. 보낼 브랜치의 지금 끝,
//! 대상이 가진 커밋(`theirs`)에 없는 커밋 수, 대상 브랜치 끝(`expected`)이 그 끝의 조상인가(fast-forward — 원본이 그 커밋을 갖지
//! 않으면 대상에 원본이 모르는 커밋이 있어 갈라진 것이다). gix 로 읽는다 — git CLI 를 띄우지 않고 원본에 쓰지 않는다.

use std::sync::Arc;

use gix::ObjectId;
use serde::{Deserialize, Serialize};

use super::{branch_name, commit_id, contains, ref_target};
use crate::Git;
use crate::write::WriteError;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendCompareParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub branch: String,
    /// 대상이 가진 커밋(`git.send.theirs`). 원본에 없는 것은 무시한다.
    #[serde(default)]
    pub theirs: Vec<String>,
    /// 대상 브랜치의 지금 끝(`git.send.theirs` 의 `branchTip`). `null` 은 대상에 그 브랜치가 없다.
    #[serde(default)]
    pub expected: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendCompare {
    /// 원본의 그 브랜치 끝. 원본에 그 브랜치가 없으면 `null`(나머지는 비었다).
    pub commit: Option<String>,
    /// 그 끝과 조상 가운데 `theirs` 에 없는 커밋 수 — 보낼 커밋 수.
    pub commits: u32,
    /// 대상 브랜치가 없거나, 그 끝이 원본 끝 자신이거나 조상이다. 거짓이면 갈라짐이다.
    pub fast_forward: bool,
    /// 원본 작업 트리 · index 에 커밋 안 한 변경이 있다(추적하지 않는 파일은 세지 않는다). 그 변경은 가지 않는다.
    pub uncommitted: bool,
}

pub async fn compare(
    git: &Arc<Git>,
    p: GitSendCompareParams,
) -> Result<GitSendCompare, WriteError> {
    branch_name(&p.branch)?;
    let theirs = p
        .theirs
        .iter()
        .map(|t| commit_id("theirs", t))
        .collect::<Result<Vec<_>, _>>()?;
    let expected = p
        .expected
        .as_deref()
        .map(|e| commit_id("expected", e))
        .transpose()?;
    let git = Arc::clone(git);
    tokio::task::spawn_blocking(move || {
        let opened = git
            .open(&p.project_id, p.worktree.as_deref())
            .map_err(WriteError::Rpc)?
            .ok_or(WriteError::NoRepository)?;
        read(&opened.repo, &p.branch, &theirs, expected)
    })
    .await
    .map_err(|e| WriteError::Internal(e.to_string()))?
}

fn read(
    repo: &gix::Repository,
    branch: &str,
    theirs: &[ObjectId],
    expected: Option<ObjectId>,
) -> Result<GitSendCompare, WriteError> {
    let internal = |e: &dyn std::fmt::Display| WriteError::Internal(e.to_string());
    let Some(tip) = ref_target(repo, &format!("refs/heads/{branch}")) else {
        return Ok(GitSendCompare {
            commit: None,
            commits: 0,
            fast_forward: false,
            uncommitted: false,
        });
    };
    let hidden: Vec<ObjectId> = theirs
        .iter()
        .copied()
        .filter(|t| repo.has_object(t))
        .collect();
    let mut commits = 0u32;
    for info in repo
        .rev_walk([tip])
        .with_hidden(hidden)
        .all()
        .map_err(|e| internal(&e))?
    {
        info.map_err(|e| internal(&e))?;
        commits = commits.saturating_add(1);
    }
    let fast_forward = match expected {
        None => true,
        Some(e) => repo.has_object(e) && contains(repo, tip, e) == Some(true),
    };
    let uncommitted = repo.is_dirty().unwrap_or_else(|e| {
        tracing::debug!(target: "pw_git", error = %e, "cannot tell whether the source has uncommitted changes");
        false
    });
    Ok(GitSendCompare {
        commit: Some(tip.to_string()),
        commits,
        fast_forward,
        uncommitted,
    })
}
