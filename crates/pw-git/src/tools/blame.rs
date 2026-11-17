//! `git_blame` — `git.blame` 은 스트림이라 툴이 될 수 없다(05-mcp.md 「툴 레지스트리」). 줄 범위를 받는 unary 로, `git.blame` 과
//! 같은 엔진([`crate::blame::methods::prepare`] · [`crate::blame::methods::run`])의 청크를 모아 그 범위로 자른다. 엔진과 신뢰 규칙
//! (신뢰한 프로젝트는 `git blame --incremental`, 미신뢰는 gix — 06-roadmap.md 「결정 기록」 D39)은 거기서 정한다.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use pw_rpc::{ConnectionScope, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::{Budget, grant};
use crate::Git;
use crate::blame::methods::{BlameOut, GitBlameParams, prepare, run};
use crate::blame::{BlameChunk, BlameCommit, BlameEngine};

/// 한 번에 묻는 줄 수 상한.
pub const MAX_LINES: u32 = 500;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBlameToolParams {
    pub project_id: String,
    /// Top of the worktree to read (absolute path). Empty means the session's worktree, or else the project's main
    /// checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// File path relative to the working tree (`/`-separated).
    pub path: String,
    /// Revision (hash · branch · tag · `HEAD~1`). Empty means HEAD. Uncommitted changes in the working tree are not
    /// blamed.
    #[serde(default)]
    pub rev: Option<String>,
    /// First line (from 1).
    pub start_line: u32,
    /// Last line (inclusive). Up to 500 lines from `startLine`.
    pub end_line: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBlameToolResult {
    /// The commit blamed (the hash `rev` resolved to).
    pub commit: String,
    /// Number of lines of the file in that commit.
    pub lines: u32,
    /// `git` (`git blame -M` — lines moved within the file are attributed to their original commit) or `gix` (moved
    /// lines belong to the commit that moved them).
    pub engine: BlameEngine,
    /// Whether lines moved within the file were attributed to their original commit.
    pub moves: bool,
    /// Ranges covering the requested lines (in line order, not overlapping). Only the first ones when `truncated`.
    pub ranges: Vec<GitBlameToolRange>,
    /// The response size limit was reached, so lines from `nextLine` on are not included — call again with `startLine`
    /// set to that line.
    pub truncated: bool,
    /// The first line not included when `truncated`; otherwise `null`.
    pub next_line: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBlameToolRange {
    pub start_line: u32,
    /// Last line (inclusive).
    pub end_line: u32,
    /// The commit that last changed these lines.
    pub commit: String,
    /// Path in that commit (the old name if it was renamed).
    pub source_path: String,
    /// Author (with mailmap applied).
    pub author_name: String,
    pub author_email: String,
    /// Author time (Unix seconds).
    #[specta(type = pw_rpc::JsNumber)]
    pub author_time: i64,
    /// Commit subject.
    pub subject: String,
}

rpc_method! {
    /// The commit · author · time · subject that last changed a range of lines (`startLine` – `endLine`, up to 500
    /// lines at a time) of a file at one revision (HEAD if not given). A range outside the file, or a file missing at
    /// that revision, is invalid params. When the response size limit is reached, `truncated` and `nextLine`.
    pub GitBlameTool: "git.tool.blame" {
        params: GitBlameToolParams,
        result: GitBlameToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "git", name: "git_blame" },
    }
}

/// 청크를 모은다.
#[derive(Default)]
struct Collect(Mutex<Vec<BlameChunk>>);

impl BlameOut for Collect {
    async fn send(&self, chunk: BlameChunk) -> Result<(), RpcError> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(chunk);
        Ok(())
    }
}

pub(crate) async fn call(
    git: &Arc<Git>,
    scope: ConnectionScope,
    params: GitBlameToolParams,
) -> Result<GitBlameToolResult, RpcError> {
    let (start, end) = (params.start_line, params.end_line);
    if start == 0 || end < start {
        return Err(RpcError::invalid_params(format!(
            "lines {start}..{end} are not a range (`startLine` is 1 or more, `endLine` is `startLine` or more)"
        )));
    }
    if end - start >= MAX_LINES {
        return Err(RpcError::invalid_params(format!(
            "lines {start}..{end} are more than {MAX_LINES}"
        )));
    }
    grant(git, scope, &params.project_id).await?;
    let at = params.rev.clone().unwrap_or_else(|| "HEAD".to_owned());
    let path = params.path.clone();
    let prepared = prepare(
        git,
        GitBlameParams {
            project_id: params.project_id,
            worktree: params.worktree,
            path: params.path,
            rev: params.rev,
        },
    )
    .await?;
    let Some((opened, subject)) = prepared else {
        return Err(RpcError::invalid_params(format!(
            "no file `{path}` at `{at}` (or no git repository, or no commit yet)"
        )));
    };
    if end > subject.lines {
        return Err(RpcError::invalid_params(format!(
            "lines {start}..{end} are outside `{path}`, which has {} lines at `{at}`",
            subject.lines
        )));
    }
    let collect = Collect::default();
    run(git, opened, subject, &collect).await?;
    let chunks = collect
        .0
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    clip(chunks, start, end)
}

/// 모은 청크를 `start ..= end` 로 자르고 줄 순서로 놓는다. 같은 커밋 · 같은 경로가 이어지면 한 범위로 합친다. 응답 예산에 드는
/// 앞쪽만 싣는다.
fn clip(chunks: Vec<BlameChunk>, start: u32, end: u32) -> Result<GitBlameToolResult, RpcError> {
    let mut file = None;
    let mut commits: HashMap<String, BlameCommit> = HashMap::new();
    let mut ranges = Vec::new();
    for chunk in chunks {
        file = file.or(chunk.file);
        commits.extend(chunk.commits.into_iter().map(|c| (c.hash.clone(), c)));
        ranges.extend(chunk.ranges);
    }
    let file = file.ok_or_else(|| RpcError::internal("blame sent no file"))?;
    ranges.sort_by_key(|r| r.line);
    let mut out: Vec<GitBlameToolRange> = Vec::new();
    for r in ranges {
        let (first, last) = (r.line.max(start), (r.line + r.count - 1).min(end));
        if first > last {
            continue;
        }
        if let Some(prev) = out.last_mut()
            && prev.commit == r.commit
            && prev.source_path == r.source_path
            && prev.end_line + 1 == first
        {
            prev.end_line = last;
            continue;
        }
        let commit = commits
            .get(&r.commit)
            .ok_or_else(|| RpcError::internal(format!("blame sent no commit {}", r.commit)))?;
        out.push(GitBlameToolRange {
            start_line: first,
            end_line: last,
            commit: r.commit,
            source_path: r.source_path,
            author_name: commit.author_name.clone(),
            author_email: commit.author_email.clone(),
            author_time: commit.author_time,
            subject: commit.subject.clone(),
        });
    }
    // 예산 안의 앞쪽만 — 범위마다 커밋 정보를 되풀이하므로 잘게 갈린 범위는 500줄로도 상한을 넘을 수 있다(결정 M5-D34).
    let mut budget = Budget::new();
    let mut next_line = None;
    let mut ranges = Vec::with_capacity(out.len());
    for range in out {
        // 첫 범위는 예산과 상관없이 싣는다 — 비면 `nextLine` 이 제자리라 나아가지 못한다.
        if !budget.take(&range) && !ranges.is_empty() {
            next_line = Some(range.start_line);
            break;
        }
        ranges.push(range);
    }
    Ok(GitBlameToolResult {
        commit: file.commit,
        lines: file.lines,
        engine: file.engine,
        moves: file.moves,
        ranges,
        truncated: next_line.is_some(),
        next_line,
    })
}
