//! `git.log`(스트림) · `git.logPage`(쪽 단위). UI 전용이라 MCP 툴이 아니다 — 결정 M2-D7 대로 M5 가 `git.logPage` 를 툴로 켤 수
//! 있게 입력을 struct(`projectId` 포함)로 둔다(스트림 메서드는 툴이 될 수 없다).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pw_rpc::{Router, RpcError, StreamSink, rpc_method};
use serde::{Deserialize, Serialize};

use super::{LogChunk, LogFilter, LogOrder, LogRow, MAX_PAGE, Query, Window, run};
use crate::Git;

/// 받는 쪽이 읽기를 기다리는 청크 수(그 뒤로는 걷기 스레드가 멈춰 기다린다).
const PENDING_CHUNKS: usize = 2;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLogParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 브랜치 필터: 시작할 ref(`main`, `origin/main`, `refs/tags/v1`, `HEAD` — git 처럼 짧은 이름을 찾는다). 그래프는 유지된다.
    /// 없으면 모든 브랜치 · 원격 · 태그 · HEAD(`git log --branches --remotes --tags HEAD`).
    #[serde(default)]
    pub refs: Option<Vec<String>>,
    #[serde(default)]
    pub filter: LogFilter,
    #[serde(default)]
    pub order: LogOrder,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLogPageParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub refs: Option<Vec<String>>,
    #[serde(default)]
    pub filter: LogFilter,
    #[serde(default)]
    pub order: LogOrder,
    /// 앞 쪽의 `nextCursor`. 없으면 처음부터. 그 쪽의 마지막 커밋이 로그에서 사라졌으면(force push 등) invalid params
    /// (`stale cursor`) — 처음부터 다시 부른다.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 1 ~ 500.
    pub limit: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLogPage {
    pub rows: Vec<LogRow>,
    /// 다음 쪽을 부를 커서(불투명 — 이 쪽의 마지막 커밋을 가리킨다). 더 없으면 `null`. 다음 쪽은 그 커밋 다음 행부터이므로 쪽
    /// 사이에 새 커밋이 생겨도 행이 겹치지 않는다(새 커밋이 그 커밋보다 앞에 오면 이어지는 쪽에 보이지 않는다).
    pub next_cursor: Option<String>,
    /// 전체 행 수(그래프 목록만, 걸러진 목록은 `null`).
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub total: Option<u32>,
}

rpc_method! {
    /// 커밋 로그(F5-1 · F5-2). 행마다 해시 · 부모 · 제목 · 작성자 · 시각 · ref 장식 · 레인 배치. 첫 청크는 200행을 배치하자마자
    /// 보내고(`total` 이 실린다) 나머지는 500행씩. 브랜치 밖의 필터가 켜지면 행의 `graph` 가 `null` 이고 맞는 커밋만 온다.
    /// 저장소가 없으면 청크 없이 끝난다. 없는 프로젝트 · 워크트리, 모르는 ref, 잘못된 정규식 · 경로 · 해시는 invalid params.
    pub GitLogMethod: "git.log" {
        params: GitLogParams,
        stream: LogChunk,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// `git.log` 의 쪽 단위(결정 M2-D7). 같은 행을 `cursor` 부터 많아야 `limit`(1~500)행. 저장소가 없으면 빈 쪽.
    pub GitLogPageMethod: "git.logPage" {
        params: GitLogPageParams,
        result: GitLogPage,
        tier: Read,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다(`crate::methods::register` 가 부른다).
pub fn register(router: &mut Router, git: Arc<Git>) {
    let stream_git = Arc::clone(&git);
    router.stream(GitLogMethod, move |_ctx, params: GitLogParams, sink| {
        let git = Arc::clone(&stream_git);
        async move { stream(git, params, &sink).await }
    });
    router.unary(GitLogPageMethod, move |_ctx, params: GitLogPageParams| {
        let git = Arc::clone(&git);
        async move {
            tokio::task::spawn_blocking(move || page(&git, &params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
}

async fn stream(
    git: Arc<Git>,
    params: GitLogParams,
    sink: &StreamSink<LogChunk>,
) -> Result<(), RpcError> {
    let stop = Arc::new(AtomicBool::new(false));
    // 스트림이 끝나 이 future 가 버려지면 걷기도 멈춘다.
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<LogChunk>(PENDING_CHUNKS);
    let worker = tokio::task::spawn_blocking(move || {
        let query = Query::new(params.refs, &params.filter, params.order)?;
        let Some(opened) = git.open(&params.project_id, params.worktree.as_deref())? else {
            return Ok(());
        };
        // 받는 쪽이 없으면(스트림이 끝남) 더 만들지 않는다.
        run(
            &opened.repo,
            opened.trusted,
            &query,
            Window::default(),
            &stop,
            &mut |chunk| tx.blocking_send(chunk).is_ok(),
        )
        .map(drop)
    });
    while let Some(chunk) = rx.recv().await {
        sink.send(chunk).await?;
    }
    worker
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

fn page(git: &Git, params: &GitLogPageParams) -> Result<GitLogPage, RpcError> {
    if !(1..=MAX_PAGE).contains(&params.limit) {
        return Err(RpcError::invalid_params(format!(
            "`limit` must be 1 to {MAX_PAGE}"
        )));
    }
    let after = match params.cursor.as_deref() {
        None => None,
        Some(c) => Some(
            gix::ObjectId::from_hex(c.as_bytes())
                .map_err(|_| RpcError::invalid_params(format!("bad `cursor`: `{c}`")))?,
        ),
    };
    let query = Query::new(params.refs.clone(), &params.filter, params.order)?;
    let empty = GitLogPage {
        rows: Vec::new(),
        next_cursor: None,
        total: None,
    };
    let Some(opened) = git.open(&params.project_id, params.worktree.as_deref())? else {
        return Ok(empty);
    };
    let mut rows = Vec::new();
    let mut total = None;
    let window = Window {
        after,
        limit: Some(params.limit as usize),
    };
    let outcome = run(
        &opened.repo,
        opened.trusted,
        &query,
        window,
        &AtomicBool::new(false),
        &mut |chunk| {
            total = total.or(chunk.total);
            rows.extend(chunk.rows);
            true
        },
    )?;
    let next_cursor = outcome
        .more
        .then(|| rows.last().map(|r| r.hash.clone()))
        .flatten();
    Ok(GitLogPage {
        rows,
        next_cursor,
        total,
    })
}

struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
