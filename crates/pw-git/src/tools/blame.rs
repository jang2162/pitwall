//! `git_blame` — `git.blame` 은 스트림이라 툴이 될 수 없다(05-mcp.md 「툴 레지스트리」). 줄 범위를 받는 unary 로, `git.blame` 과
//! 같은 엔진([`crate::blame::methods::prepare`] · [`crate::blame::methods::run`])의 청크를 모아 그 범위로 자른다. 엔진과 신뢰 규칙
//! (신뢰한 프로젝트는 `git blame --incremental`, 미신뢰는 gix — 06-roadmap.md 「결정 기록」 D39)은 거기서 정한다.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use pw_rpc::{ConnectionScope, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::grant;
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
    /// 볼 워크트리의 꼭대기(절대 경로). 비면 세션의 워크트리, 그것도 없으면 프로젝트의 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 작업 트리 기준 파일 경로(`/` 구분).
    pub path: String,
    /// 리비전(해시 · 브랜치 · 태그 · `HEAD~1`). 비면 HEAD. 작업 트리의 커밋하지 않은 변경은 blame 하지 않는다.
    #[serde(default)]
    pub rev: Option<String>,
    /// 첫 줄(1부터).
    pub start_line: u32,
    /// 끝 줄(포함). `startLine` 부터 500줄까지.
    pub end_line: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBlameToolResult {
    /// blame 한 커밋(`rev` 를 푼 해시).
    pub commit: String,
    /// 그 커밋에서의 파일 줄 수.
    pub lines: u32,
    /// `git`(`git blame -M` — 파일 안에서 옮긴 줄을 원래 커밋으로 돌린다) 또는 `gix`(옮긴 줄은 옮긴 커밋의 것).
    pub engine: BlameEngine,
    /// 파일 안에서 옮긴 줄을 원래 커밋으로 돌렸는가.
    pub moves: bool,
    /// 묻은 줄 범위를 덮는 범위들(줄 순서, 겹치지 않는다).
    pub ranges: Vec<GitBlameToolRange>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBlameToolRange {
    pub start_line: u32,
    /// 끝 줄(포함).
    pub end_line: u32,
    /// 이 줄들을 마지막으로 바꾼 커밋.
    pub commit: String,
    /// 그 커밋에서의 경로(이름이 바뀌었으면 옛 이름).
    pub source_path: String,
    /// 작성자(mailmap 적용).
    pub author_name: String,
    pub author_email: String,
    /// 작성 시각(유닉스 초).
    #[specta(type = pw_rpc::JsNumber)]
    pub author_time: i64,
    /// 커밋 제목.
    pub subject: String,
}

rpc_method! {
    /// 파일 한 리비전(없으면 HEAD)의 줄 범위(`startLine` ~ `endLine`, 한 번에 500줄까지)를 마지막으로 바꾼 커밋 · 작성자 · 시각 ·
    /// 제목. 범위가 파일 밖이거나 그 리비전에 파일이 없으면 invalid params.
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

/// 모은 청크를 `start ..= end` 로 자르고 줄 순서로 놓는다. 같은 커밋 · 같은 경로가 이어지면 한 범위로 합친다.
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
    Ok(GitBlameToolResult {
        commit: file.commit,
        lines: file.lines,
        engine: file.engine,
        moves: file.moves,
        ranges: out,
    })
}
