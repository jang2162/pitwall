//! 디버그 RPC(`debug.*`, 17-m6-plan M6-17 ① ~ ⑤). UI 전용이라 MCP 툴이 아니다 — 에이전트의 디버그 툴(M6-20)은 따로 선언한다.
//! 범위는 크레이트 머리 주석 「범위」.

use pw_dap::SessionId;
use pw_dap::protocol;
use pw_rpc::{ConnectionScope, Router, RpcError, code, rpc_method};
use pw_run::config::DebugMode;
use pw_run::runs::{RunId, RunInfo, no_such_run};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Debugger;
use crate::breakpoints::{Breakpoint, FileBreakpoints};
use crate::source::{SOURCE_LIMIT, SourceAt, outside_file, read_file, too_large};

/// `debug.source` 가 어댑터의 `source` 답을 기다리는 시한.
const SOURCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 프레임 경로마다 루트 밖 일반 파일이면 그 실제 경로(블로킹으로 푼다).
async fn blocking_places(
    root: std::path::PathBuf,
    paths: Vec<Option<String>>,
) -> Vec<Option<(std::path::PathBuf, crate::source::FileId)>> {
    let n = paths.len();
    tokio::task::spawn_blocking(move || {
        paths
            .iter()
            .map(|p| p.as_deref().and_then(|p| outside_file(&root, p)))
            .collect()
    })
    .await
    .unwrap_or_else(|_| vec![None; n])
}

/// `debug.variables` 한 쪽의 기본 수.
pub const VARIABLES_PAGE: u32 = 100;
/// `debug.variables` 한 쪽의 상한.
pub const VARIABLES_PAGE_LIMIT: u32 = 500;
/// `debug.stack` 의 기본 · 상한 프레임 수.
pub const STACK_LEVELS: u32 = 20;
pub const STACK_LEVELS_LIMIT: u32 = 200;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugStartParams {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기. 구성 · 어댑터 · 브레이크포인트 경로가 그 워크트리 안의 프로젝트 루트 기준이다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 구성 id(`.pitwall/run/<id>.toml` — `[debug]` 가 있어야 한다).
    pub config_id: String,
    #[serde(default)]
    pub profile: Option<String>,
    /// 콘솔 크기. 없으면 120 × 30.
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
    /// 구성에 `before` 가 있으면(17-m6-plan M6-21) 그 사슬을 먼저 끝까지 돌린다 — `run.start` 와 같은 첫 실행 확인(7001)의 열쇠.
    #[serde(default)]
    pub confirmed: Option<String>,
    /// `before` 의 `sql` 단계마다의 위험 확인(`run.start` 의 `sqlConfirmed` 와 같다).
    #[serde(default)]
    #[specta(type = Option<std::collections::BTreeMap<String, pw_rpc::JsonValue>>)]
    pub sql_confirmed: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

/// 어댑터가 `initialize` 로 알린 예외 필터 하나와 켰는가.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionFilter {
    pub filter: String,
    pub label: String,
    pub description: Option<String>,
    /// 어댑터가 기본으로 켜자고 한 것.
    pub default: bool,
    pub enabled: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugStarted {
    /// 디버그 세션인 실행(`kind = "debug"`). 콘솔은 `run.attach`, 끝내기는 `run.stop`.
    pub run: RunInfo,
    pub adapter: String,
    pub mode: DebugMode,
    pub exception_filters: Vec<ExceptionFilter>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugSessionParams {
    pub run_id: RunId,
    /// 세션 나무의 세션(`startDebugging` 의 자식). 없으면 뿌리.
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub session: Option<SessionId>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugThread {
    #[specta(type = pw_rpc::JsNumber)]
    pub id: i64,
    pub name: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugThreads {
    pub threads: Vec<DebugThread>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugStackParams {
    pub run_id: RunId,
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub session: Option<SessionId>,
    #[specta(type = pw_rpc::JsNumber)]
    pub thread_id: i64,
    /// 처음 프레임(0 이 맨 위).
    #[serde(default)]
    pub start_frame: Option<u32>,
    /// 몇 프레임(기본 20, 상한 200).
    #[serde(default)]
    pub levels: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugFrame {
    #[specta(type = pw_rpc::JsNumber)]
    pub id: i64,
    pub name: String,
    /// 소스 파일의 절대 경로(어댑터가 준 그대로 — 루트 밖일 수 있다).
    pub path: Option<String>,
    /// 경로 없는 소스(어댑터가 내용을 쥔 것)의 참조.
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub source_reference: Option<i64>,
    #[specta(type = pw_rpc::JsNumber)]
    pub line: i64,
    #[specta(type = pw_rpc::JsNumber)]
    pub column: i64,
    pub presentation_hint: Option<String>,
    /// 소스가 프로젝트 루트 밖(일반 파일)이거나 어댑터가 내용을 쥔 것(`sourceReference`)이면 `debug.source` 로 읽는 토큰 — 이
    /// 연결 · 이 실행에만 산다(M6-18). 루트 안이면 없다(창이 `fs.*` 로 연다).
    pub token: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugStack {
    pub frames: Vec<DebugFrame>,
    /// 어댑터가 안 전체 수(모르면 없다).
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub total_frames: Option<i64>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugScopesParams {
    pub run_id: RunId,
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub session: Option<SessionId>,
    #[specta(type = pw_rpc::JsNumber)]
    pub frame_id: i64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugScope {
    pub name: String,
    /// 펼칠 때 `debug.variables` 에 준다.
    #[specta(type = pw_rpc::JsNumber)]
    pub variables_reference: i64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub named_variables: Option<i64>,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub indexed_variables: Option<i64>,
    /// 읽는 데 오래 걸린다(펼칠 때만 읽는다).
    pub expensive: bool,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugScopes {
    pub scopes: Vec<DebugScope>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugVariablesParams {
    pub run_id: RunId,
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub session: Option<SessionId>,
    #[specta(type = pw_rpc::JsNumber)]
    pub variables_reference: i64,
    /// 쪽의 처음(0부터).
    #[serde(default)]
    pub start: Option<u32>,
    /// 쪽의 크기(기본 100, 상한 500).
    #[serde(default)]
    pub count: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugVariable {
    pub name: String,
    pub value: String,
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    /// 워치 · 평가에 쓸 식.
    pub evaluate_name: Option<String>,
    /// 0 이 아니면 펼칠 수 있다.
    #[specta(type = pw_rpc::JsNumber)]
    pub variables_reference: i64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub named_variables: Option<i64>,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub indexed_variables: Option<i64>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugVariables {
    pub variables: Vec<DebugVariable>,
    /// 이 쪽의 처음.
    pub start: u32,
    /// 이 쪽의 크기(요청을 상한으로 줄인 값).
    pub count: u32,
}

/// 평가의 자리(DAP `evaluate` 의 `context`).
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum EvaluateContext {
    /// 콘솔.
    #[default]
    Repl,
    /// 워치.
    Watch,
    /// 에디터의 마우스 올림.
    Hover,
}

impl EvaluateContext {
    fn wire(self) -> &'static str {
        match self {
            Self::Repl => "repl",
            Self::Watch => "watch",
            Self::Hover => "hover",
        }
    }
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugEvaluateParams {
    pub run_id: RunId,
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub session: Option<SessionId>,
    pub expression: String,
    /// 그 프레임의 문맥에서(없으면 전역).
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub frame_id: Option<i64>,
    #[serde(default)]
    pub context: EvaluateContext,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugEvaluated {
    pub result: String,
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    #[specta(type = pw_rpc::JsNumber)]
    pub variables_reference: i64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub named_variables: Option<i64>,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub indexed_variables: Option<i64>,
}

/// 스텝 · 계속 · 멈춤.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DebugAction {
    Continue,
    /// 넘기기(F8).
    Next,
    /// 들어가기(F7).
    StepIn,
    /// 나오기(⇧F8).
    StepOut,
    Pause,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugControlParams {
    pub run_id: RunId,
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub session: Option<SessionId>,
    #[specta(type = pw_rpc::JsNumber)]
    pub thread_id: i64,
    pub action: DebugAction,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugExceptionsParams {
    pub run_id: RunId,
    /// 켤 필터(어댑터가 알린 이름만 — 나머지는 끈다).
    pub filters: Vec<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugExceptions {
    pub exception_filters: Vec<ExceptionFilter>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointsListParams {
    pub project_id: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointsList {
    /// 파일(상대 경로)마다, 경로 차례.
    pub files: Vec<FileBreakpoints>,
    /// 저장 파일을 읽지 못해 빈 목록으로 시작했다 — 그 까닭과 옮겨 둔 자리(영어). 다음 저장이 성공하면 사라진다.
    pub problem: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointsSetParams {
    pub project_id: String,
    /// 프로젝트 루트 기준 상대 경로(`/` 로 나눔 — 워크트리는 같은 목록을 쓴다).
    pub path: String,
    /// 그 파일의 목록 전체(빈 목록이면 지운다). 같은 줄은 뒤의 것이 이긴다.
    pub breakpoints: Vec<Breakpoint>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointsRemoveParams {
    pub project_id: String,
    pub path: String,
    /// 지울 줄. 없으면 그 파일 전부.
    #[serde(default)]
    pub line: Option<u32>,
}

/// 어댑터가 본 브레이크포인트 하나(그 세션에서).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointState {
    /// 어댑터가 옮겼으면 옮긴 줄.
    pub line: u32,
    pub verified: bool,
    /// 확인하지 못한 까닭 등(어댑터의 글).
    pub message: Option<String>,
}

/// 이벤트 `debug.breakpoints`: 저장이 바뀌었거나(`runId` 없음) 어댑터가 확인 · 옮김을 알렸다(`runId` · `states`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointsEvent {
    pub project_id: String,
    pub run_id: Option<RunId>,
    pub path: String,
    /// 그 파일의 저장된 목록(옮긴 줄 반영 뒤).
    pub breakpoints: Vec<Breakpoint>,
    pub states: Vec<BreakpointState>,
}

/// 이벤트 `debug.stopped`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugStoppedEvent {
    pub project_id: String,
    pub run_id: RunId,
    #[specta(type = pw_rpc::JsNumber)]
    pub session: SessionId,
    /// `breakpoint` · `step` · `exception` · `pause` 등(어댑터의 글).
    pub reason: String,
    pub description: Option<String>,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub thread_id: Option<i64>,
    pub text: Option<String>,
    pub all_threads_stopped: bool,
    #[specta(type = Vec<pw_rpc::JsNumber>)]
    pub hit_breakpoint_ids: Vec<i64>,
}

/// 이벤트 `debug.continued`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugContinuedEvent {
    pub project_id: String,
    pub run_id: RunId,
    #[specta(type = pw_rpc::JsNumber)]
    pub session: SessionId,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub thread_id: Option<i64>,
    pub all_threads_continued: bool,
}

/// 이벤트 `debug.output`(콘솔에도 쓴다). 글은 16 KiB 까지 싣는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugOutputEvent {
    pub project_id: String,
    pub run_id: RunId,
    #[specta(type = pw_rpc::JsNumber)]
    pub session: SessionId,
    /// `console` · `stdout` · `stderr` · `important` 등.
    pub category: Option<String>,
    pub output: String,
}

/// 이벤트 `debug.ended` — 세션이 끝났다(실행의 끝은 `run.exited`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugEndedEvent {
    pub project_id: String,
    pub run_id: RunId,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugSourceParams {
    pub run_id: RunId,
    /// `debug.stack` 의 프레임이 준 토큰.
    pub token: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugSource {
    /// 글 전부(5 MiB 까지, UTF-8).
    pub text: String,
    /// 읽은 파일의 실제 경로(정규화 뒤 — 화면은 이것을 보인다). 어댑터가 쥔 소스면 없다.
    pub path: Option<String>,
    /// 어댑터가 보고한 경로가 실제 경로와 다르면 그것(링크 · `..` — 화면이 함께 보인다).
    pub reported: Option<String>,
    /// 어댑터가 쥔 소스의 이름(어댑터의 글 — 경로가 아니다).
    pub name: Option<String>,
    /// 그 세션의 어댑터 id.
    pub adapter: String,
}

pw_rpc::rpc_type!(
    DebugBreakpointsEvent,
    DebugStoppedEvent,
    DebugContinuedEvent,
    DebugOutputEvent,
    DebugEndedEvent
);

rpc_method! {
    /// 실행 구성의 `[debug]` 로 디버그 세션을 연다 — 세션은 실행 하나다(`kind = "debug"`, 콘솔은 `run.attach`, 끝내기는 `run.stop`).
    /// `mode = "attach"` 면 프로그램을 띄우지 않고 붙기만 한다. 그 프로젝트의 창이 아니면 permission denied, 미신뢰 프로젝트면 어댑터를
    /// 띄우지 않고 permission denied(`data.reason = "untrusted"`), 없는 구성은 not found, `[debug]` 가 없거나 어댑터를 쓸 수 없으면
    /// invalid params(`data.reason` — `noDebug` · `adapterMissing` · `adapterDisabled` · `adapterArgs` · `runtimeMissing` ·
    /// `languageServerNotRunning`). 어댑터가 시작에 실패하면 그 실행은 끝난 실행으로 남고 오류의 `data.runId` 가 그것이다.
    pub DebugStart: "debug.start" {
        params: DebugStartParams,
        result: DebugStarted,
        tier: Exec,
        roles: [Owner],
    }

    /// 스텝 · 계속 · 멈춤.
    pub DebugControl: "debug.control" {
        params: DebugControlParams,
        result: (),
        tier: Exec,
        roles: [Owner],
    }

    /// 스레드 목록.
    pub DebugThreadsMethod: "debug.threads" {
        params: DebugSessionParams,
        result: DebugThreads,
        tier: Read,
        roles: [Owner],
    }

    /// 한 스레드의 콜스택(쪽 단위).
    pub DebugStackMethod: "debug.stack" {
        params: DebugStackParams,
        result: DebugStack,
        tier: Read,
        roles: [Owner],
    }

    /// 한 프레임의 범위(지역 · 전역 등). 변수는 펼칠 때 `debug.variables` 로.
    pub DebugScopesMethod: "debug.scopes" {
        params: DebugScopesParams,
        result: DebugScopes,
        tier: Read,
        roles: [Owner],
    }

    /// 변수 한 쪽(기본 100, 상한 500). 어댑터가 쪽을 지키지 않아도 그 쪽만 준다.
    pub DebugVariablesMethod: "debug.variables" {
        params: DebugVariablesParams,
        result: DebugVariables,
        tier: Read,
        roles: [Owner],
    }

    /// 식을 디버그 대상 안에서 평가한다(콘솔 · 워치 · 마우스 올림) — 대상 안에서 코드를 돌리므로 exec 이다.
    pub DebugEvaluate: "debug.evaluate" {
        params: DebugEvaluateParams,
        result: DebugEvaluated,
        tier: Exec,
        roles: [Owner],
    }

    /// 프레임 소스를 토큰으로 읽는다(루트 밖 파일 · 어댑터가 쥔 소스 — 결정 M4-D13 과 같은 규칙, 읽기 전용). 그 프로젝트의 창 연결만
    /// 부른다 — 다른 연결 · 에이전트 범위는 permission denied, 이 연결이 받지 않은 토큰 · 끝난 실행의 토큰은 not found.
    pub DebugSourceMethod: "debug.source" {
        params: DebugSourceParams,
        result: DebugSource,
        tier: Read,
        roles: [Owner],
    }

    /// 예외 브레이크포인트: 어댑터가 알린 필터만 켜고 끈다(알리지 않은 이름은 invalid params). 어댑터마다 저장한다.
    pub DebugExceptionsSet: "debug.exceptions.set" {
        params: DebugExceptionsParams,
        result: DebugExceptions,
        tier: Write,
        roles: [Owner],
    }

    /// 저장된 브레이크포인트(`$DATA/projects/<id>/breakpoints.json`).
    pub DebugBreakpointsListMethod: "debug.breakpoints.list" {
        params: DebugBreakpointsListParams,
        result: DebugBreakpointsList,
        tier: Read,
        roles: [Owner],
    }

    /// 파일 하나의 브레이크포인트를 통째로 바꾸고 열린 세션마다 곧바로 보낸다(어댑터가 옮긴 줄 · 확인은 `debug.breakpoints` 이벤트).
    pub DebugBreakpointsSet: "debug.breakpoints.set" {
        params: DebugBreakpointsSetParams,
        result: FileBreakpoints,
        tier: Write,
        roles: [Owner],
    }

    /// 브레이크포인트 한 줄(없으면 그 파일 전부)을 지운다.
    pub DebugBreakpointsRemove: "debug.breakpoints.remove" {
        params: DebugBreakpointsRemoveParams,
        result: FileBreakpoints,
        tier: Write,
        roles: [Owner],
    }
}

/// 프로젝트 창의 프로젝트.
fn window_project(scope: &ConnectionScope) -> Option<&str> {
    match scope {
        ConnectionScope::Project(id) => Some(id),
        _ => None,
    }
}

/// 제어 · 평가 · 바꾸기는 프로젝트 창만(`runId` 와 상관없이 — 있다는 것을 알리지 않는다).
fn controlling_project(scope: &ConnectionScope) -> Result<&str, RpcError> {
    window_project(scope).ok_or_else(|| {
        RpcError::new(
            code::PERMISSION_DENIED,
            "only a project window may control its debug sessions",
        )
    })
}

/// 조회는 프로젝트 창이 아니면 없는 실행.
fn viewing_project<'a>(scope: &'a ConnectionScope, run_id: &str) -> Result<&'a str, RpcError> {
    window_project(scope).ok_or_else(|| no_such_run(run_id))
}

fn project_window(scope: &ConnectionScope, project_id: &str) -> Result<(), RpcError> {
    if window_project(scope) == Some(project_id) {
        Ok(())
    } else {
        Err(RpcError::new(
            code::PERMISSION_DENIED,
            "only the project's own window may change its breakpoints",
        )
        .with_data(json!({ "projectId": project_id })))
    }
}

pub(crate) fn adapter_error(e: pw_dap::RequestError) -> RpcError {
    match e {
        pw_dap::RequestError::Adapter(e) => RpcError::invalid_params(e.message),
        other => RpcError::internal(other.to_string()),
    }
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, debugger: Debugger) {
    let d = debugger;
    let (start, control, threads, stack, scopes, variables, evaluate, exceptions) = (
        d.clone(),
        d.clone(),
        d.clone(),
        d.clone(),
        d.clone(),
        d.clone(),
        d.clone(),
        d.clone(),
    );
    // 툴 메서드(M6-20 — `crate::tool`).
    crate::tool::register(router, d.clone());
    let (list, set, remove, source) = (d.clone(), d.clone(), d.clone(), d);
    router
        .unary(DebugStart, move |ctx, params: DebugStartParams| {
            let debugger = start.clone();
            let granted = debugger
                .host()
                .map(|h| h.parts.projects.clone())
                .and_then(|projects| {
                    pw_dap::debug_grant(
                        ctx.scope(),
                        &projects,
                        &params.project_id,
                        params.worktree.as_deref(),
                    )
                });
            let window = ctx.window().map(str::to_owned);
            let scope = ctx.scope().clone();
            // `before` 사슬은 창의 `run.start` 와 같이 사람이 본 구성으로 기록한다(M6-21).
            let seen = pw_core::registry::run_seen::SeenBy::window(
                ctx.scope(),
                &params.project_id,
                pw_core::registry::run_seen::SeenPlace::Run,
            )
            .ok();
            async move {
                let grant = granted?;
                // 어댑터를 먼저 본다 — 없으면 `before` 를 하나도 띄우지 않고 그 오류(실기 F-1 후속).
                debugger.check_adapter(&grant, &params).await?;
                debugger
                    .run_before(grant.project_id(), scope, seen, window.clone(), &params)
                    .await?;
                debugger.start(grant, window, params).await
            }
        })
        .unary(DebugControl, move |ctx, p: DebugControlParams| {
            let found = controlling_project(ctx.scope()).and_then(|project| {
                Ok((
                    control.find(project, &p.run_id)?,
                    control.dap_session(project, &p.run_id, p.session)?,
                ))
            });
            async move {
                let (run, s) = found?;
                let step = protocol::StepArguments {
                    thread_id: p.thread_id,
                    ..protocol::StepArguments::default()
                };
                match p.action {
                    DebugAction::Continue => s
                        .request::<protocol::Continue>(&protocol::ContinueArguments {
                            thread_id: p.thread_id,
                            ..protocol::ContinueArguments::default()
                        })
                        .await
                        .map(drop),
                    DebugAction::Next => s.request::<protocol::Next>(&step).await.map(drop),
                    DebugAction::StepIn => s.request::<protocol::StepIn>(&step).await.map(drop),
                    DebugAction::StepOut => s.request::<protocol::StepOut>(&step).await.map(drop),
                    DebugAction::Pause => s
                        .request::<protocol::Pause>(&protocol::PauseArguments {
                            thread_id: p.thread_id,
                        })
                        .await
                        .map(drop),
                }
                .map_err(adapter_error)?;
                // 계속 · 스텝을 받은 스레드는 더 멈춰 있지 않다(어댑터가 `continued` 를 알리지 않아도 — DAP 는 그 요청의 답으로 갈음한다).
                if p.action != DebugAction::Pause {
                    run.resumed(s.id(), Some(p.thread_id));
                }
                Ok(())
            }
        })
        .unary(DebugThreadsMethod, move |ctx, p: DebugSessionParams| {
            let found = viewing_project(ctx.scope(), &p.run_id)
                .and_then(|project| threads.dap_session(project, &p.run_id, p.session));
            async move {
                let body = found?
                    .request::<protocol::Threads>(&protocol::Empty {})
                    .await
                    .map_err(adapter_error)?;
                Ok(DebugThreads {
                    threads: body
                        .threads
                        .into_iter()
                        .map(|t| DebugThread {
                            id: t.id,
                            name: t.name,
                        })
                        .collect(),
                })
            }
        })
        .unary(DebugStackMethod, move |ctx, p: DebugStackParams| {
            let found = viewing_project(ctx.scope(), &p.run_id).and_then(|project| {
                let run = stack.find(project, &p.run_id)?;
                let session = stack.dap_session(project, &p.run_id, p.session)?;
                Ok((run, session))
            });
            let debugger = stack.clone();
            async move {
                let (run, session) = found?;
                let levels = p
                    .levels
                    .unwrap_or(STACK_LEVELS)
                    .clamp(1, STACK_LEVELS_LIMIT);
                let body = session
                    .request::<protocol::StackTrace>(&protocol::StackTraceArguments {
                        thread_id: p.thread_id,
                        start_frame: Some(i64::from(p.start_frame.unwrap_or(0))),
                        levels: Some(i64::from(levels)),
                    })
                    .await
                    .map_err(adapter_error)?;
                let frames: Vec<_> = body
                    .stack_frames
                    .into_iter()
                    .take(levels as usize)
                    .map(|f| {
                        let path = f.source.as_ref().and_then(|s| s.path.clone());
                        let source_reference = f
                            .source
                            .as_ref()
                            .and_then(|s| s.source_reference)
                            .filter(|r| *r > 0);
                        let name = f.source.as_ref().and_then(|s| s.name.clone());
                        (f, path, source_reference, name)
                    })
                    .collect();
                // 루트 밖인지는 파일을 풀어 본다 — 블로킹.
                let root = run.root.clone();
                let places = {
                    let paths: Vec<Option<String>> =
                        frames.iter().map(|(_, p, _, _)| p.clone()).collect();
                    blocking_places(root, paths).await
                };
                let dap = session.id();
                Ok(DebugStack {
                    frames: frames
                        .into_iter()
                        .zip(places)
                        .map(|((f, path, source_reference, name), real)| {
                            // `sourceReference > 0` 이면 경로가 있어도 어댑터의 글이다(DAP 규약).
                            let at = match (source_reference, &path, real) {
                                (Some(reference), _, _) => Some(SourceAt::Reference {
                                    session: dap,
                                    reference,
                                    name,
                                }),
                                (None, Some(reported), Some((real, id))) => Some(SourceAt::File {
                                    reported: reported.clone(),
                                    real,
                                    id,
                                }),
                                _ => None,
                            };
                            DebugFrame {
                                id: f.id,
                                name: f.name,
                                token: at.map(|at| {
                                    debugger.source_tokens().mint(
                                        &ctx,
                                        &run.project_id,
                                        run.run_id(),
                                        at,
                                    )
                                }),
                                path,
                                source_reference,
                                line: f.line,
                                column: f.column,
                                presentation_hint: f.presentation_hint,
                            }
                        })
                        .collect(),
                    total_frames: body.total_frames,
                })
            }
        })
        .unary(DebugScopesMethod, move |ctx, p: DebugScopesParams| {
            let found = viewing_project(ctx.scope(), &p.run_id)
                .and_then(|project| scopes.dap_session(project, &p.run_id, p.session));
            async move {
                let body = found?
                    .request::<protocol::Scopes>(&protocol::ScopesArguments {
                        frame_id: p.frame_id,
                    })
                    .await
                    .map_err(adapter_error)?;
                Ok(DebugScopes {
                    scopes: body
                        .scopes
                        .into_iter()
                        .map(|s| DebugScope {
                            name: s.name,
                            variables_reference: s.variables_reference,
                            named_variables: s.named_variables,
                            indexed_variables: s.indexed_variables,
                            expensive: s.expensive,
                        })
                        .collect(),
                })
            }
        })
        .unary(DebugVariablesMethod, move |ctx, p: DebugVariablesParams| {
            let found = viewing_project(ctx.scope(), &p.run_id)
                .and_then(|project| variables.dap_session(project, &p.run_id, p.session));
            async move { read_variables(found?, &p).await }
        })
        .unary(DebugEvaluate, move |ctx, p: DebugEvaluateParams| {
            let found = controlling_project(ctx.scope())
                .and_then(|project| evaluate.dap_session(project, &p.run_id, p.session));
            async move {
                let body = found?
                    .request::<protocol::Evaluate>(&protocol::EvaluateArguments {
                        expression: p.expression,
                        frame_id: p.frame_id,
                        context: Some(p.context.wire().to_owned()),
                    })
                    .await
                    .map_err(adapter_error)?;
                Ok(DebugEvaluated {
                    result: body.result,
                    type_name: body.type_name,
                    variables_reference: body.variables_reference,
                    named_variables: body.named_variables,
                    indexed_variables: body.indexed_variables,
                })
            }
        })
        .unary(DebugSourceMethod, move |ctx, p: DebugSourceParams| {
            let debugger = source.clone();
            let found = controlling_project(ctx.scope()).and_then(|project| {
                let entry = debugger.source_tokens().entry(&ctx, project, &p.token)?;
                if entry.run != p.run_id {
                    return Err(RpcError::new(code::NOT_FOUND, "no such source token"));
                }
                let run = debugger.find(project, &p.run_id)?;
                Ok((entry, run))
            });
            async move {
                let (entry, run) = found?;
                let adapter = run.adapter.clone();
                match entry.at {
                    SourceAt::File { reported, real, id } => {
                        let path = real.display().to_string();
                        let text = tokio::task::spawn_blocking(move || read_file(&real, id))
                            .await
                            .map_err(|e| RpcError::internal(e.to_string()))??;
                        Ok(DebugSource {
                            text,
                            reported: (reported != path).then_some(reported),
                            path: Some(path),
                            name: None,
                            adapter,
                        })
                    }
                    SourceAt::Reference {
                        session,
                        reference,
                        name,
                    } => {
                        let tree = run.session().ok_or_else(|| no_such_run(&p.run_id))?;
                        let s = tree
                            .session(session)
                            .ok_or_else(|| no_such_run(&p.run_id))?;
                        let body = s
                            .request_raw(
                                "source",
                                json!({
                                    "source": { "sourceReference": reference },
                                    "sourceReference": reference,
                                }),
                                SOURCE_TIMEOUT,
                            )
                            .await
                            .map_err(adapter_error)?;
                        let text = body
                            .get("content")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        if text.len() as u64 > SOURCE_LIMIT {
                            return Err(too_large(text.len() as u64));
                        }
                        Ok(DebugSource {
                            text: text.to_owned(),
                            path: None,
                            reported: None,
                            name,
                            adapter,
                        })
                    }
                }
            }
        })
        .unary(DebugExceptionsSet, move |ctx, p: DebugExceptionsParams| {
            let debugger = exceptions.clone();
            let project = controlling_project(ctx.scope()).map(str::to_owned);
            async move {
                let project = project?;
                Ok(DebugExceptions {
                    exception_filters: debugger
                        .set_exception_filters(&project, &p.run_id, p.filters)
                        .await?,
                })
            }
        })
        .unary(
            DebugBreakpointsListMethod,
            move |ctx, p: DebugBreakpointsListParams| {
                let result = project_window(ctx.scope(), &p.project_id)
                    .and_then(|()| list.breakpoints())
                    .and_then(|store| store.get(&p.project_id))
                    .map(|stored| DebugBreakpointsList {
                        files: stored.file_list(),
                        problem: stored.problem,
                    });
                async move { result }
            },
        )
        .unary(
            DebugBreakpointsSet,
            move |ctx, p: DebugBreakpointsSetParams| {
                let debugger = set.clone();
                let allowed = project_window(ctx.scope(), &p.project_id);
                async move {
                    allowed?;
                    debugger
                        .set_breakpoints(&p.project_id, &p.path, p.breakpoints)
                        .await
                }
            },
        )
        .unary(
            DebugBreakpointsRemove,
            move |ctx, p: DebugBreakpointsRemoveParams| {
                let debugger = remove.clone();
                let allowed = project_window(ctx.scope(), &p.project_id);
                async move {
                    allowed?;
                    debugger
                        .remove_breakpoints(&p.project_id, &p.path, p.line)
                        .await
                }
            },
        );
}

/// `debug.variables` 한 쪽(머리 주석 · ⑥): 쪽 크기를 상한으로 줄여 어댑터에 묻고, 어댑터가 쪽을 지키지 않고 다 주면 그 쪽만 자른다.
pub async fn read_variables(
    session: std::sync::Arc<pw_dap::Session>,
    p: &DebugVariablesParams,
) -> Result<DebugVariables, RpcError> {
    let start = p.start.unwrap_or(0);
    let count = p
        .count
        .unwrap_or(VARIABLES_PAGE)
        .clamp(1, VARIABLES_PAGE_LIMIT);
    let body = session
        .request::<protocol::Variables>(&protocol::VariablesArguments {
            variables_reference: p.variables_reference,
            filter: None,
            start: Some(i64::from(start)),
            count: Some(i64::from(count)),
        })
        .await
        .map_err(adapter_error)?;
    let all = body.variables;
    let page: Vec<_> = if all.len() > count as usize {
        // 쪽을 지키지 않은 어댑터 — 처음부터 다 준 것으로 보고 그 쪽을 자른다.
        all.into_iter()
            .skip(start as usize)
            .take(count as usize)
            .collect()
    } else {
        all
    };
    Ok(DebugVariables {
        variables: page
            .into_iter()
            .map(|v| DebugVariable {
                name: v.name,
                value: v.value,
                type_name: v.type_name,
                evaluate_name: v.evaluate_name,
                variables_reference: v.variables_reference,
                named_variables: v.named_variables,
                indexed_variables: v.indexed_variables,
            })
            .collect(),
        start,
        count,
    })
}
