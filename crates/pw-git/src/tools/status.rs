//! `git_status` — `git.status` · `git.refs` · `git.repo` 의 함수 위: 요약(HEAD · upstream · ahead/behind · 진행 중 작업 · 수)과
//! 바뀐 파일 목록의 한 쪽.

use std::sync::Arc;

use pw_rpc::{ConnectionScope, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::{Budget, blocking, grant};
use crate::methods::{GitRepoParams, RepoHead, RepoOperation};
use crate::status::{
    GitChange, GitStatusEntry, GitStatusParams, GitUnsupported, GitUpstream, Status,
};

/// `limit` 이 비었을 때(결정 M5-D34).
pub const DEFAULT_LIMIT: u32 = 200;
/// `limit` 상한.
pub const MAX_LIMIT: u32 = 1000;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusToolParams {
    pub project_id: String,
    /// Top of the worktree to read (absolute path). Empty means the session's worktree, or else the project's main
    /// checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Only these paths (relative to the working tree, `/`-separated — a folder means everything under it).
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// `nextCursor` from the previous page. Empty means from the first file.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Files per page, 1 – 1000 (default 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusToolResult {
    /// Top of the working tree that file paths are relative to (absolute path). `null` means the project has no git
    /// repository.
    pub work_tree: Option<String>,
    pub summary: GitStatusSummary,
    /// One page of changed files (sorted by path).
    pub files: Vec<GitStatusEntry>,
    /// The `cursor` for the next page. `null` on the last page.
    pub next_cursor: Option<String>,
    /// If the repository format cannot be read, the reason (the rest is empty).
    pub unsupported: Option<GitUnsupported>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusSummary {
    /// The branch HEAD is on · a detached commit · a branch with no commits yet.
    pub head: Option<RepoHead>,
    /// The current branch's upstream and ahead/behind (`track: null` if that ref is gone).
    pub upstream: Option<GitUpstream>,
    /// A merge · rebase · cherry-pick · revert · bisect · am in progress.
    pub operation: Option<RepoOperation>,
    /// Number of files with staged changes. All counts cover every changed file (within `paths`, if given), not just
    /// this page.
    pub staged: u32,
    /// Number of files with unstaged changes (untracked files excluded).
    pub unstaged: u32,
    pub untracked: u32,
    /// Number of files in conflict. Conflicted files are not counted in the other numbers.
    pub conflicted: u32,
}

rpc_method! {
    /// Working tree state of the project's git repository: a summary (HEAD · upstream and ahead/behind · operation in
    /// progress · counts of staged · unstaged · untracked · conflicted files) and one page of the changed files (path ·
    /// staged change · unstaged change · conflict).
    pub GitStatusTool: "git.tool.status" {
        params: GitStatusToolParams,
        result: GitStatusToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "git", name: "git_status" },
    }
}

pub(crate) async fn call(
    status: &Arc<Status>,
    scope: ConnectionScope,
    params: GitStatusToolParams,
) -> Result<GitStatusToolResult, RpcError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(RpcError::invalid_params(format!(
            "`limit` must be 1 to {MAX_LIMIT}"
        )));
    }
    if params.cursor.as_deref() == Some("") {
        return Err(RpcError::invalid_params("`cursor` is empty"));
    }
    grant(status.git(), scope, &params.project_id).await?;
    let computed = status
        .status(GitStatusParams {
            project_id: params.project_id.clone(),
            worktree: params.worktree.clone(),
            paths: params.paths,
        })
        .await?;
    let repo_params = GitRepoParams {
        project_id: params.project_id,
        worktree: params.worktree,
    };
    let (repo, refs) = {
        let s = Arc::clone(status);
        blocking(move || Ok((s.git().repo(&repo_params)?, s.refs(&repo_params)?))).await?
    };
    let upstream = refs
        .branches
        .iter()
        .find(|b| b.head)
        .and_then(|b| b.upstream.clone());
    let mut summary = GitStatusSummary {
        head: refs.head,
        upstream,
        operation: repo.repo.and_then(|r| r.operation),
        ..GitStatusSummary::default()
    };
    for entry in &computed.entries {
        count(&mut summary, entry);
    }
    let (files, next_cursor) = page(
        computed.entries,
        params.cursor.as_deref(),
        limit as usize,
        &mut Budget::new(),
    );
    Ok(GitStatusToolResult {
        work_tree: computed.work_tree,
        summary,
        files,
        next_cursor,
        unsupported: computed.unsupported,
    })
}

fn count(summary: &mut GitStatusSummary, entry: &GitStatusEntry) {
    if entry.conflict.is_some() {
        summary.conflicted += 1;
        return;
    }
    if entry.staged.is_some() {
        summary.staged += 1;
    }
    match entry.unstaged {
        Some(GitChange::Untracked) => summary.untracked += 1,
        Some(_) => summary.unstaged += 1,
        None => {}
    }
}

/// 경로순 `entries` 에서 `cursor`(앞 쪽의 마지막 경로) 다음부터 많아야 `limit` 개 — 예산이 먼저 차면 거기까지. 다음 쪽이 있으면 이
/// 쪽의 마지막 경로가 커서다(쪽 사이에 파일이 바뀌어도 경로 순서로 이어진다).
fn page(
    entries: Vec<GitStatusEntry>,
    cursor: Option<&str>,
    limit: usize,
    budget: &mut Budget,
) -> (Vec<GitStatusEntry>, Option<String>) {
    let mut rest = entries
        .into_iter()
        .filter(|e| cursor.is_none_or(|c| e.path.as_str() > c))
        .peekable();
    let mut files = Vec::new();
    while let Some(entry) = rest.peek() {
        // 첫 항목은 예산과 상관없이 싣는다 — 쪽이 비면 다음 커서가 없어 나아가지 못한다.
        if files.len() >= limit || (!budget.take(entry) && !files.is_empty()) {
            break;
        }
        files.extend(rest.next());
    }
    let next = rest
        .peek()
        .is_some()
        .then(|| files.last().map(|e| e.path.clone()))
        .flatten();
    (files, next)
}
