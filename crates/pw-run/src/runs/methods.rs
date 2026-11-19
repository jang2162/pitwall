//! 실행 RPC(`run.*`, 17-m6-plan M6-02 ⑤). UI 전용이라 MCP 툴이 아니다 — 에이전트는 실행 툴(M6-12 ~ 14)이 [`Runs`] 를 직접 부른다.
//!
//! 범위(머리 주석 「범위」, 메인 판정 M6-P1):
//!
//! - `run.start` 는 `exec_grant` — 그 프로젝트 창만(다른 프로젝트 창 · Host · 에이전트 범위는 `PERMISSION_DENIED`).
//! - `runId` 를 받는 것은 **그 실행의 프로젝트로 판정한다**: 연결이 그 프로젝트 창(`scope_may_write` 와 같은 범위)이어야 하고,
//!   다른 프로젝트의 `runId` 는 없는 실행과 같은 오류(`NOT_FOUND`)다 — 있다는 것을 알리지 않는다. 프로젝트 창이 아닌 연결(Host ·
//!   에이전트 범위)은 띄우기 · 입력 · 중지(`run.stop` · `run.restart` · `run.input` · `run.resize`)가 `runId` 와 상관없이
//!   `PERMISSION_DENIED` 이고, 보기(`run.attach`)는 없는 실행이다. 파일 창은 디스패처가 먼저 거절한다(`file_scope` 가 아니다).
//! - `run.list { projectId }` 는 그 프로젝트 창만.
//!
//! `run.attach` 는 스트림이다: `attached` 하나 → `output` 여럿 → (끝났으면) `exited` 하나. 스트림을 끊어도 실행은 그대로 돈다 —
//! 다시 붙을 때 받은 마지막 오프셋을 `since` 로 주면 잃은 출력이 없다(버퍼에서 버려진 앞이 아니면). M7 의 다시 붙기가 이 꼴을 쓴다.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pw_core::registry::exec::{Launch, exec_grant};
use pw_core::registry::run_seen::{SeenBy, SeenPlace};
use pw_core::registry::state::scope_may_write;
use pw_rpc::{ConnectionScope, Router, RpcError, StreamSink, code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Run, RunExit, RunId, RunInfo, RunState, Runs, no_such_run};
use crate::pty::CHUNK;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunStartParams {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기(`ActiveWorktree.top`). 구성은 그 워크트리의 것을 읽고 작업 폴더도 그 안의 프로젝트 루트 기준이다.
    /// 그 프로젝트의 연결 워크트리가 아니면 invalid params.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 구성 id(`.pitwall/run/<id>.toml`).
    pub config_id: String,
    /// 고를 프로필(`[profiles.<이름>]`).
    #[serde(default)]
    pub profile: Option<String>,
    /// 콘솔 크기. 없으면 120 × 30(콘솔이 붙으면 `run.resize`).
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunIdParams {
    pub run_id: RunId,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunListParams {
    pub project_id: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunList {
    /// 실행 중인 것과 남은 끝난 것(프로젝트마다 최근 20개), 시작 순.
    pub runs: Vec<RunInfo>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunAttachParams {
    pub run_id: RunId,
    /// 이 오프셋부터 보낸다(받은 마지막 `output` 의 `offset` + 바이트 수). 없으면 0 — 남은 처음부터.
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub since: Option<u64>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunInputParams {
    pub run_id: RunId,
    /// 보낼 글자(키 입력 · 붙여넣기). 받은 순서대로 쓰인다.
    pub data: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunResizeParams {
    pub run_id: RunId,
    pub cols: u16,
    pub rows: u16,
}

/// `run.attach` 스트림의 항목.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RunAttachEvent {
    /// 첫 항목: 지금의 실행. `from` 은 처음 보낼 오프셋 — `since` 앞이 버퍼에서 버려졌으면 `since` 보다 크고, 그 차이가
    /// `dropped` 바이트다.
    Attached {
        run: Box<RunInfo>,
        #[specta(type = pw_rpc::JsNumber)]
        from: u64,
        #[specta(type = pw_rpc::JsNumber)]
        dropped: u64,
    },
    /// 출력. `data` 는 바이트의 base64(청크 경계가 UTF-8 문자 가운데일 수 있다). `offset` 이 앞 항목의 끝보다 크면 그 사이는 보내기
    /// 전에 버퍼에서 버려졌다(소비자가 느리다).
    Output {
        #[specta(type = pw_rpc::JsNumber)]
        offset: u64,
        data: String,
    },
    /// 마지막 항목 — 실행이 끝났고 남은 출력을 다 보냈다.
    Exited { exit: RunExit },
}

rpc_method! {
    /// 실행 구성 하나를 띄운다(PTY — 런타임 환경 위에 구성의 `env_file` · `env` · 프로필). 실행은 Host 의 것이라 이 연결이 닫혀도 돈다 — 출력은
    /// `run.attach`, 끝은 이벤트 `run.exited`. 그 프로젝트의 창이 아닌 연결이면 permission denied, 미신뢰 프로젝트면 띄우지 않고
    /// permission denied(`data.reason = "untrusted"`), 작업 폴더가 푼 뒤에 바뀌었거나 사라졌으면 띄우지 않고 permission
    /// denied(`data.reason = "cwd_changed"`), 없는 구성은 not found, 쓸 수 없는 구성 · 프로젝트 밖 작업 폴더 · 없는
    /// 프로필 · 읽지 못한 `env_file` 은 invalid params.
    pub RunStart: "run.start" {
        params: RunStartParams,
        result: RunInfo,
        tier: Exec,
        roles: [Owner],
    }

    /// 실행을 중지한다 — 프로세스 세션 전체에 SIGTERM, 5초 안에 끝나지 않은 것은 SIGKILL(compose 는 SIGINT 를 먼저). 기다리지
    /// 않는다(끝나면 `run.exited`). 이미 끝난 실행이면 아무것도 하지 않는다.
    pub RunStop: "run.stop" {
        params: RunIdParams,
        result: RunInfo,
        tier: Exec,
        roles: [Owner],
    }

    /// 다시 실행: 중지하고 프로세스 트리가 끝난 것을 본 뒤 같은 구성을 새 실행으로 띄운다(`restartOf` 가 앞 실행).
    pub RunRestart: "run.restart" {
        params: RunIdParams,
        result: RunInfo,
        tier: Exec,
        roles: [Owner],
    }

    /// 그 프로젝트의 실행(실행 중 · 남은 끝난 실행)과 상태 · 끝 코드.
    pub RunListMethod: "run.list" {
        params: RunListParams,
        result: RunList,
        tier: Read,
        roles: [Owner],
    }

    /// 실행의 출력을 `since` 오프셋부터 보낸다(앞에 쌓인 것 → 새로 나오는 것 → 끝). 스트림을 끊어도 실행은 돈다.
    pub RunAttach: "run.attach" {
        params: RunAttachParams,
        stream: RunAttachEvent,
        tier: Read,
        roles: [Owner],
    }

    /// 실행에 입력을 보낸다. 끝난 실행이면 invalid params.
    pub RunInput: "run.input" {
        params: RunInputParams,
        result: (),
        tier: Exec,
        roles: [Owner],
    }

    /// 실행 콘솔의 PTY 크기를 바꾼다. 0 인 크기는 invalid params.
    pub RunResize: "run.resize" {
        params: RunResizeParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

/// 프로젝트 창의 프로젝트. 아니면 `None`.
fn window_project(scope: &ConnectionScope) -> Option<&str> {
    match scope {
        ConnectionScope::Project(id) => Some(id),
        _ => None,
    }
}

/// 띄우기 · 입력 · 중지는 프로젝트 창만(`runId` 와 상관없이 — 있다는 것을 알리지 않는다).
fn controlling_project(scope: &ConnectionScope) -> Result<&str, RpcError> {
    window_project(scope).ok_or_else(|| {
        RpcError::new(
            code::PERMISSION_DENIED,
            "only a project window may control its runs",
        )
    })
}

/// `run.attach` 본체.
async fn attach(
    run: Arc<Run>,
    since: u64,
    sink: StreamSink<RunAttachEvent>,
) -> Result<(), RpcError> {
    let mut changed = run.subscribe();
    let from = run.with_log(|log| log.read(since, 0).from);
    sink.send(RunAttachEvent::Attached {
        run: Box::new(run.info()),
        from,
        dropped: from.saturating_sub(since),
    })
    .await?;
    let mut next = from;
    loop {
        // 읽기 전에 본 것으로 표시한다 — 읽은 뒤에 붙은 출력은 `changed` 가 깨운다.
        changed.borrow_and_update();
        let slice = run.with_log(|log| log.read(next, CHUNK));
        if !slice.bytes.is_empty() {
            next = slice.end();
            sink.send(RunAttachEvent::Output {
                offset: slice.from,
                data: STANDARD.encode(&slice.bytes),
            })
            .await?;
            continue;
        }
        if run.state() == RunState::Exited {
            // 종류는 출력을 다 쓴 뒤에 끝을 알린다 — 여기서 빈 조각이면 남은 출력이 없다.
            let exit = run.exited().await;
            return sink.send(RunAttachEvent::Exited { exit }).await;
        }
        if changed.changed().await.is_err() {
            return Ok(());
        }
    }
}

/// 블로킹 스레드에서 돌린다(구성 · 레지스트리 읽기, 로그인 셸 기다리기).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 핸들러를 붙인다. 입력은 핸들러의 동기 구간에서 큐에 넣는다 — 세션은 요청이 온 순서대로 그 구간을 부르므로 키 입력 순서가
/// 지켜진다(터미널과 같다).
pub fn register(router: &mut Router, runs: Runs) {
    // 실행 툴 ① 조회(M6-12) — MCP 툴 메서드.
    super::tool::register(router, runs.clone());
    super::exec_tool::register(router, runs.clone());
    let (start, stop, restart, list, attach_runs, input, resize) = (
        runs.clone(),
        runs.clone(),
        runs.clone(),
        runs.clone(),
        runs.clone(),
        runs.clone(),
        runs,
    );
    router
        .unary(RunStart, move |ctx, params: RunStartParams| {
            let runs = start.clone();
            let granted = exec_grant(ctx.scope(), &params.project_id, &Launch::Process);
            // 사람이 그 프로젝트 창에서 띄운다 — 띄운 값이 「사람이 본 구성」 이 된다(M6-02 ⑧ · M6-11).
            let seen = SeenBy::window(ctx.scope(), &params.project_id, SeenPlace::Run).ok();
            let window = ctx.window().map(str::to_owned);
            async move {
                let grant = granted?;
                blocking(move || runs.start(&grant, seen, window, &params)).await
            }
        })
        .unary(RunStop, move |ctx, params: RunIdParams| {
            let result = controlling_project(ctx.scope())
                .and_then(|project| stop.stop(project, &params.run_id));
            async move { result }
        })
        .unary(RunRestart, move |ctx, params: RunIdParams| {
            let runs = restart.clone();
            let granted = controlling_project(ctx.scope())
                .and_then(|project| exec_grant(ctx.scope(), project, &Launch::Process));
            let seen = controlling_project(ctx.scope())
                .ok()
                .and_then(|project| SeenBy::window(ctx.scope(), project, SeenPlace::Run).ok());
            let window = ctx.window().map(str::to_owned);
            async move { runs.restart(&granted?, seen, window, &params.run_id).await }
        })
        .unary(RunListMethod, move |ctx, params: RunListParams| {
            let result = if scope_may_write(ctx.scope(), &params.project_id) {
                Ok(RunList {
                    runs: list.list(&params.project_id),
                })
            } else {
                Err(RpcError::new(
                    code::PERMISSION_DENIED,
                    "only the project's own window may list its runs",
                )
                .with_data(json!({ "projectId": params.project_id })))
            };
            async move { result }
        })
        .stream(RunAttach, move |ctx, params: RunAttachParams, sink| {
            let found = match window_project(ctx.scope()) {
                Some(project) => attach_runs.find(project, &params.run_id),
                None => Err(no_such_run(&params.run_id)),
            };
            async move { attach(found?, params.since.unwrap_or(0), sink).await }
        })
        .unary(RunInput, move |ctx, params: RunInputParams| {
            let result = controlling_project(ctx.scope())
                .and_then(|project| input.input(project, &params.run_id, params.data.into_bytes()));
            async move { result }
        })
        .unary(RunResize, move |ctx, params: RunResizeParams| {
            let result = controlling_project(ctx.scope()).and_then(|project| {
                resize.resize(project, &params.run_id, params.cols, params.rows)
            });
            async move { result }
        });
}
