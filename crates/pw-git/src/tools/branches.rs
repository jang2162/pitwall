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
    /// 볼 워크트리의 꼭대기(절대 경로). 비면 세션의 워크트리, 그것도 없으면 프로젝트의 메인 체크아웃. HEAD 여부가 워크트리마다
    /// 다르다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 태그도 싣는다.
    #[serde(default)]
    pub include_tags: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchListToolResult {
    /// 저장소의 작업 트리 꼭대기. `null` 이면 프로젝트에 git 저장소가 없다.
    pub work_tree: Option<String>,
    pub head: Option<RepoHead>,
    /// 로컬 브랜치(이름 · 해시 · HEAD 여부 · upstream 과 ahead/behind), ref 이름순.
    pub branches: Vec<GitBranch>,
    /// 원격 추적 브랜치, ref 이름순.
    pub remote_branches: Vec<GitRemoteBranch>,
    /// `includeTags` 일 때만 있다.
    pub tags: Option<Vec<GitTag>>,
    /// 응답 크기 상한에 닿아 뒤쪽(원격 브랜치 · 태그부터)을 싣지 못했다.
    pub truncated: bool,
    /// 읽지 못하는 저장소 형식이면 그 까닭(나머지는 빈다).
    pub unsupported: Option<GitUnsupported>,
}

rpc_method! {
    /// 프로젝트 git 저장소의 로컬 · 원격 브랜치(이름 · 해시 · upstream 과 ahead/behind · HEAD 여부). 태그는 `includeTags`.
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
