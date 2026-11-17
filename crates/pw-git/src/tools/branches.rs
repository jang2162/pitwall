//! `git_branch_list` — `git.refs` 의 함수 위: 로컬 · 원격 브랜치, `includeTags` 면 태그.

use std::sync::Arc;

use pw_rpc::{ConnectionScope, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::{Budget, blocking, grant};
use crate::methods::{GitRepoParams, RepoHead};
use crate::status::{GitBranch, GitRemoteBranch, GitTag, GitUnsupported, Status};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchListToolParams {
    pub project_id: String,
    /// Top of the worktree to read (absolute path). Empty means the session's worktree, or else the project's main
    /// checkout. Which branch is HEAD differs per worktree.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Include tags too.
    #[serde(default)]
    pub include_tags: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchListToolResult {
    /// Top of the repository's working tree. `null` means the project has no git repository.
    pub work_tree: Option<String>,
    pub head: Option<RepoHead>,
    /// Local branches (name · hash · whether HEAD · upstream and ahead/behind), sorted by ref name.
    pub branches: Vec<GitBranch>,
    /// Remote-tracking branches, sorted by ref name.
    pub remote_branches: Vec<GitRemoteBranch>,
    /// Present only with `includeTags`.
    pub tags: Option<Vec<GitTag>>,
    /// The response size limit was reached, so the tail (from remote branches · tags on) is not included.
    pub truncated: bool,
    /// If the repository format cannot be read, the reason (the rest is empty).
    pub unsupported: Option<GitUnsupported>,
}

rpc_method! {
    /// Local and remote branches of the project's git repository (name · hash · upstream and ahead/behind · whether
    /// HEAD). Tags with `includeTags`.
    pub GitBranchListTool: "git.tool.branchList" {
        params: GitBranchListToolParams,
        result: GitBranchListToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "git", name: "git_branch_list" },
    }
}

pub(crate) async fn call(
    status: &Arc<Status>,
    scope: ConnectionScope,
    params: GitBranchListToolParams,
) -> Result<GitBranchListToolResult, RpcError> {
    grant(status.git(), scope, &params.project_id).await?;
    let repo_params = GitRepoParams {
        project_id: params.project_id,
        worktree: params.worktree,
    };
    let refs = {
        let s = Arc::clone(status);
        blocking(move || s.refs(&repo_params)).await?
    };
    let mut budget = Budget::new();
    let mut truncated = false;
    let branches = fill(refs.branches, &mut budget, &mut truncated);
    let remote_branches = fill(refs.remote_branches, &mut budget, &mut truncated);
    let tags = params
        .include_tags
        .then(|| fill(refs.tags, &mut budget, &mut truncated));
    Ok(GitBranchListToolResult {
        work_tree: refs.work_tree,
        head: refs.head,
        branches,
        remote_branches,
        tags,
        truncated,
        unsupported: refs.unsupported,
    })
}

/// 예산에 드는 앞쪽만. 한 번 넘치면 뒤의 목록도 싣지 않는다(목록 사이를 건너뛰어 빈자리를 숨기지 않게).
fn fill<T: Serialize>(items: Vec<T>, budget: &mut Budget, truncated: &mut bool) -> Vec<T> {
    let mut out = Vec::new();
    for item in items {
        if *truncated || !budget.take(&item) {
            *truncated = true;
            break;
        }
        out.push(item);
    }
    out
}
