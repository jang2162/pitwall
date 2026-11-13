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
    /// 볼 워크트리의 꼭대기(절대 경로). 비면 세션의 워크트리, 그것도 없으면 프로젝트의 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 이 브랜치 · ref(`main` · `origin/main` · `v1.0` · `HEAD`)에서 닿는 커밋. 비면 HEAD(`git log` 와 같다).
    #[serde(default)]
    pub branch: Option<String>,
    /// 작성자 「이름 <메일>」 에 든 글자(대소문자를 가리지 않는다, 정규식이 아니다).
    #[serde(default)]
    pub author: Option<String>,
    /// 메시지(제목 · 본문)의 한 줄에 든 글자(대소문자를 가리지 않는다, 정규식이 아니다).
    #[serde(default)]
    pub message: Option<String>,
    /// 이 시각 이후 커밋(커미터 시각, git `--since` 와 같은 꼴 — `2026-01-31` · `2026-01-31T09:00:00+09:00` · `2 weeks ago` ·
    /// 유닉스 초는 `@1700000000`).
    #[serde(default)]
    pub since: Option<String>,
    /// 이 시각 이전 커밋(커미터 시각, `since` 와 같은 꼴).
    #[serde(default)]
    pub until: Option<String>,
    /// 이 경로(작업 트리 기준, `/` 구분, 파일이나 폴더) 하나라도 바꾼 커밋만. 글롭은 받지 않는다.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// 커밋마다 메시지 본문(제목 다음 문단부터)도 싣는다.
    #[serde(default)]
    pub detail: bool,
    /// 앞 쪽의 `nextCursor`. 비면 처음부터. 그 커밋이 로그에서 사라졌으면(force push 등) invalid params — 처음부터 다시 부른다.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 한 쪽의 커밋 수, 1 ~ 500(기본 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLogToolResult {
    /// 자식이 부모보다 먼저(`git log --topo-order`).
    pub commits: Vec<GitLogCommit>,
    /// 다음 쪽을 부를 `cursor`. 마지막 쪽이면 `null`.
    pub next_cursor: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLogCommit {
    /// 전체 해시.
    pub hash: String,
    /// 부모 해시(첫 부모가 앞). 머지 커밋이면 둘 이상.
    pub parents: Vec<String>,
    /// 작성자(mailmap 적용).
    pub author_name: String,
    pub author_email: String,
    /// 작성 시각(유닉스 초).
    #[specta(type = pw_rpc::JsNumber)]
    pub author_time: i64,
    /// 제목(메시지 첫 문단을 한 줄로).
    pub subject: String,
    /// 이 커밋을 가리키는 브랜치 · 원격 브랜치 · 태그 이름(분리된 HEAD 는 `HEAD`).
    pub refs: Vec<String>,
    /// 본문(제목 다음 문단부터, 없으면 빈 글자). `detail` 일 때만 있다.
    pub body: Option<String>,
}

rpc_method! {
    /// 커밋 로그의 한 쪽: 커밋마다 해시 · 부모 · 작성자 · 작성 시각 · 제목 · 가리키는 ref(본문은 `detail`). 시작은 `branch`(없으면
    /// HEAD), 필터는 작성자 · 메시지 · 기간 · 경로. 저장소가 없거나 HEAD 에 아직 커밋이 없으면 빈 쪽이다.
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
