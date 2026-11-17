//! `git_log` — 스트림 `git.log` 가 아니라 쪽 단위 `git.logPage` 의 함수([`crate::log::methods::page`]) 위(05-mcp.md 「Git」 표
//! 아래 문장). 그래프 레인은 싣지 않는다.

use std::sync::Arc;

use gix::bstr::ByteSlice;
use pw_rpc::{ConnectionScope, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::{Budget, blocking, grant};
use crate::Git;
use crate::log::commit::{clip, decode};
use crate::log::methods::{GitLogPageParams, page};
use crate::log::{LogFilter, LogOrder, MAX_PAGE};
use crate::methods::{GitRepoParams, RepoHead};

/// `limit` 이 비었을 때(결정 M5-D34).
pub const DEFAULT_LIMIT: u32 = 50;
/// `detail` 의 본문 상한(글자). 넘으면 자르고 `…` 를 붙인다.
pub const MAX_BODY: usize = 16 * 1024;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLogToolParams {
    pub project_id: String,
    /// Top of the worktree to read (absolute path). Empty means the session's worktree, or else the project's main
    /// checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Commits reachable from this branch · ref (`main` · `origin/main` · `v1.0` · `HEAD`). Empty means HEAD (as in
    /// `git log`).
    #[serde(default)]
    pub branch: Option<String>,
    /// Text contained in the author's "name <email>" (case-insensitive, not a regular expression).
    #[serde(default)]
    pub author: Option<String>,
    /// Text contained in one line of the message (subject · body) (case-insensitive, not a regular expression).
    #[serde(default)]
    pub message: Option<String>,
    /// Commits after this time (committer time, in the same forms as git's `--since` — `2026-01-31` ·
    /// `2026-01-31T09:00:00+09:00` · `2 weeks ago` · `@1700000000` for Unix seconds).
    #[serde(default)]
    pub since: Option<String>,
    /// Commits before this time (committer time, same forms as `since`).
    #[serde(default)]
    pub until: Option<String>,
    /// Only commits that changed this path (relative to the working tree, `/`-separated, a file or a folder). Globs are
    /// not accepted.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// Include each commit's message body too (from the paragraph after the subject).
    #[serde(default)]
    pub detail: bool,
    /// `nextCursor` from the previous page. Empty means from the start. If that commit is gone from the log (a force
    /// push, etc.), invalid params — call again from the start.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Commits per page, 1 – 500 (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLogToolResult {
    /// Children before their parents (`git log --topo-order`).
    pub commits: Vec<GitLogCommit>,
    /// The `cursor` for the next page. `null` on the last page.
    pub next_cursor: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLogCommit {
    /// Full hash.
    pub hash: String,
    /// Parent hashes (first parent first). Two or more for a merge commit.
    pub parents: Vec<String>,
    /// Author (with mailmap applied).
    pub author_name: String,
    pub author_email: String,
    /// Author time (Unix seconds).
    #[specta(type = pw_rpc::JsNumber)]
    pub author_time: i64,
    /// Subject (the message's first paragraph on one line).
    pub subject: String,
    /// Names of the branches · remote branches · tags pointing at this commit (`HEAD` for a detached HEAD).
    pub refs: Vec<String>,
    /// Body (from the paragraph after the subject; empty text if none). Present only with `detail`.
    pub body: Option<String>,
}

rpc_method! {
    /// One page of the commit log: for each commit its hash · parents · author · author time · subject · refs pointing
    /// at it (body with `detail`). Starts at `branch` (HEAD if missing); filters by author · message · time range ·
    /// path. An empty page if there is no repository or HEAD has no commits yet.
    pub GitLogTool: "git.tool.log" {
        params: GitLogToolParams,
        result: GitLogToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "git", name: "git_log" },
    }
}

pub(crate) async fn call(
    git: &Arc<Git>,
    scope: ConnectionScope,
    params: GitLogToolParams,
) -> Result<GitLogToolResult, RpcError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(RpcError::invalid_params(format!(
            "`limit` must be 1 to {MAX_PAGE}"
        )));
    }
    let filter = LogFilter {
        author: params.author,
        message: params.message,
        regex: false,
        ignore_case: true,
        since: time(params.since.as_deref(), "since")?,
        until: time(params.until.as_deref(), "until")?,
        paths: params.paths.unwrap_or_default(),
        hash: None,
    };
    grant(git, scope, &params.project_id).await?;
    let git = Arc::clone(git);
    blocking(move || {
        let page_params = GitLogPageParams {
            project_id: params.project_id,
            worktree: params.worktree,
            refs: Some(vec![params.branch.unwrap_or_else(|| "HEAD".to_owned())]),
            filter,
            order: LogOrder::Topo,
            cursor: params.cursor,
            limit,
        };
        if page_params.refs.as_deref() == Some(&["HEAD".to_owned()])
            && !has_head(&git, &page_params)?
        {
            return Ok(GitLogToolResult {
                commits: Vec::new(),
                next_cursor: None,
            });
        }
        let got = page(&git, &page_params)?;
        let bodies = if params.detail {
            Some(bodies(
                &git,
                &page_params,
                got.rows.iter().map(|r| r.hash.as_str()),
            )?)
        } else {
            None
        };
        let mut budget = Budget::new();
        let mut commits = Vec::with_capacity(got.rows.len());
        let mut cut = false;
        for (n, row) in got.rows.into_iter().enumerate() {
            let commit = GitLogCommit {
                hash: row.hash,
                parents: row.parents,
                author_name: row.author_name,
                author_email: row.author_email,
                author_time: row.author_time,
                subject: row.subject,
                refs: row.refs.into_iter().map(|r| r.name).collect(),
                body: bodies
                    .as_ref()
                    .map(|b| b.get(n).cloned().unwrap_or_default()),
            };
            // 첫 커밋은 예산과 상관없이 싣는다 — 쪽이 비면 나아가지 못한다.
            if !budget.take(&commit) && !commits.is_empty() {
                cut = true;
                break;
            }
            commits.push(commit);
        }
        // 예산에서 끊었으면 이 쪽의 마지막 커밋 다음이 다음 쪽이다(엔진의 커서와 같은 꼴 — 그 쪽의 마지막 커밋 해시).
        let next_cursor = if cut {
            commits.last().map(|c| c.hash.clone())
        } else {
            got.next_cursor
        };
        Ok(GitLogToolResult {
            commits,
            next_cursor,
        })
    })
    .await
}

/// git `--since` 와 같은 꼴의 시각을 유닉스 초로.
fn time(value: Option<&str>, name: &str) -> Result<Option<i64>, RpcError> {
    let Some(value) = value else {
        return Ok(None);
    };
    gix::date::parse(value.trim(), Some(gix::date::Zoned::now()))
        .map(|t| Some(t.seconds))
        .map_err(|e| RpcError::invalid_params(format!("`{name}` is not a date: `{value}`: {e}")))
}

/// 저장소가 있고 HEAD 에 커밋이 있는가 — 없으면 `HEAD` 에서 걷는 로그는 빈 쪽이다(엔진은 모르는 ref 로 거절한다).
fn has_head(git: &Git, params: &GitLogPageParams) -> Result<bool, RpcError> {
    let repo = git.repo(&GitRepoParams {
        project_id: params.project_id.clone(),
        worktree: params.worktree.clone(),
    })?;
    Ok(repo
        .repo
        .is_some_and(|r| !matches!(r.head, RepoHead::Unborn { .. })))
}

/// 커밋마다 본문(제목 다음 문단부터). 쪽을 만든 저장소를 다시 연다 — 엔진의 행에는 본문이 없다.
fn bodies<'a>(
    git: &Git,
    params: &GitLogPageParams,
    hashes: impl Iterator<Item = &'a str>,
) -> Result<Vec<String>, RpcError> {
    let Some(opened) = git.open(&params.project_id, params.worktree.as_deref())? else {
        return Ok(Vec::new());
    };
    let internal = |e: &dyn std::fmt::Display| RpcError::internal(e.to_string());
    hashes
        .map(|hash| {
            let id = gix::ObjectId::from_hex(hash.as_bytes()).map_err(|e| internal(&e))?;
            let commit = opened.repo.find_commit(id).map_err(|e| internal(&e))?;
            let decoded = commit.decode().map_err(|e| internal(&e))?;
            let body = decoded
                .message()
                .body()
                .map(|b| decode(b.as_bstr(), decoded.encoding).into_owned())
                .unwrap_or_default();
            Ok(clip(body.trim_end(), MAX_BODY))
        })
        .collect()
}
