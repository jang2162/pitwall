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
    /// 볼 워크트리의 꼭대기(절대 경로). 비면 세션의 워크트리, 그것도 없으면 프로젝트의 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 이 경로(작업 트리 기준, `/` 구분 — 폴더면 그 아래 전체)만 본다.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// 앞 쪽의 `nextCursor`. 비면 첫 파일부터.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 한 쪽의 파일 수, 1 ~ 1000(기본 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusToolResult {
    /// 파일 경로의 기준인 작업 트리 꼭대기(절대 경로). `null` 이면 프로젝트에 git 저장소가 없다.
    pub work_tree: Option<String>,
    pub summary: GitStatusSummary,
    /// 바뀐 파일의 한 쪽(경로순).
    pub files: Vec<GitStatusEntry>,
    /// 다음 쪽을 부를 `cursor`. 마지막 쪽이면 `null`.
    pub next_cursor: Option<String>,
    /// 읽지 못하는 저장소 형식이면 그 까닭(나머지는 빈다).
    pub unsupported: Option<GitUnsupported>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusSummary {
    /// HEAD 가 있는 브랜치 · 분리된 커밋 · 아직 커밋이 없는 브랜치.
    pub head: Option<RepoHead>,
    /// 지금 브랜치의 upstream 과 ahead/behind(그 ref 가 없어졌으면 `track: null`).
    pub upstream: Option<GitUpstream>,
    /// 진행 중인 merge · rebase · cherry-pick · revert · bisect · am.
    pub operation: Option<RepoOperation>,
    /// 스테이징한 변경이 있는 파일 수. 수는 모두 이 쪽만이 아니라 바뀐 파일 전체(`paths` 를 줬으면 그 안)의 것이다.
    pub staged: u32,
    /// 스테이징하지 않은 변경이 있는 파일 수(untracked 는 빼고).
    pub unstaged: u32,
    pub untracked: u32,
    /// 충돌 중인 파일 수. 충돌 파일은 다른 수에 세지 않는다.
    pub conflicted: u32,
}

rpc_method! {
    /// 프로젝트 git 저장소의 작업 트리 상태: 요약(HEAD · upstream 과 ahead/behind · 진행 중 작업 · 스테이징 · 미스테이징 ·
    /// untracked · 충돌 파일 수)과 바뀐 파일 목록의 한 쪽(경로 · 스테이징한 변경 · 스테이징하지 않은 변경 · 충돌).
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
