//! DAP 메시지(Debug Adapter Protocol 1.71.x — 17-m6-plan.md M6-15 ②, 메인 판정 M6-P7: 타입은 직접 정의한다).
//!
//! **바탕** — 메시지는 셋이다: 요청(`type = "request"` · `command` · `arguments`), 응답(`type = "response"` · `request_seq` ·
//! `success` · `message` · `body`), 이벤트(`type = "event"` · `event` · `body`). 보내는 쪽마다 `seq` 를 1 부터 하나씩 올린다.
//! 본문 하나를 [`parse`] 로 [`Incoming`] 으로 가르고, 보낼 것은 [`request`] · [`response`] · [`event`] 로 바이트로 만든다.
//!
//! **쓰는 것만** — 요청은 [`Command`] 를 단 타입 하나씩(initialize · launch · attach · setBreakpoints · setFunctionBreakpoints ·
//! setExceptionBreakpoints · configurationDone · threads · stackTrace · scopes · variables · evaluate · continue · next · stepIn ·
//! stepOut · pause · disconnect · terminate), 이벤트는 [`Event`] 의 갈래(initialized · stopped · continued · output · terminated ·
//! exited · breakpoint · thread), 역요청은 [`RunInTerminalRequestArguments`] · [`StartDebuggingRequestArguments`]. 규격에 있으나
//! 쓰지 않는 칸은 두지 않는다 — 어댑터가 보낸 모르는 칸은 읽을 때 버린다(`deny_unknown_fields` 를 달지 않는다). 모르는 이벤트는
//! [`Event::parse`] 가 `None` 을 내고 부른 쪽이 로그만 남긴다.
//!
//! 정본은 <https://github.com/microsoft/debug-adapter-protocol> 의 `debugAdapterProtocol.json`(확인한 판: 저장소 HEAD
//! `1b52c204`, 그 파일의 마지막 바뀜 `1d5bfe02` 2026-11-01, changelog 의 맨 위 1.71.x — 2026-11-18).

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// 받은 메시지 하나.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// 어댑터가 거는 요청(역요청 — `runInTerminal` · `startDebugging`). 답해야 한다.
    Request {
        seq: i64,
        command: String,
        arguments: Value,
    },
    /// 우리 요청의 응답.
    Response {
        request_seq: i64,
        command: String,
        /// 성공이면 `body`(없으면 `null`), 실패면 `message`(없으면 빈 글)와 `body`.
        result: Result<Value, ErrorResponse>,
    },
    Event {
        event: String,
        body: Value,
    },
}

/// 실패한 응답의 `message` 와 `body`(`ErrorResponse.body.error` 가 있으면 그 `format`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorResponse {
    pub message: String,
    /// `body.error.format` — 사람이 읽을 글(자리 표지 `{name}` 그대로).
    pub detail: Option<String>,
}

impl std::fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(detail) if !detail.is_empty() => f.write_str(detail),
            _ if self.message.is_empty() => f.write_str("the debug adapter refused the request"),
            _ => f.write_str(&self.message),
        }
    }
}

/// 바탕 메시지의 칸. 이름은 규격 그대로다 — `request_seq` 는 밑줄이다(camelCase 가 아니다).
#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    seq: Option<i64>,
    #[serde(default)]
    request_seq: Option<i64>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    event: Option<String>,
}

/// 본문을 메시지로. 객체가 아니거나, `type` 이 셋 중 하나가 아니거나, 그 갈래에 꼭 있어야 할 칸이 없으면 오류(그 메시지만 버린다 —
/// 틀 오류가 아니다, `pw_wire` 머리 주석).
pub fn parse(body: &[u8]) -> Result<Incoming, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| format!("not a DAP message: {e}"))?;
    if !value.is_object() {
        return Err("a DAP message must be an object".to_owned());
    }
    let raw: Raw = serde_json::from_value(value).map_err(|e| format!("not a DAP message: {e}"))?;
    match raw.kind.as_str() {
        "request" => Ok(Incoming::Request {
            seq: raw.seq.ok_or("a request without seq")?,
            command: raw.command.ok_or("a request without command")?,
            arguments: raw.arguments.unwrap_or(Value::Null),
        }),
        "response" => {
            let body = raw.body.unwrap_or(Value::Null);
            let result = if raw.success.ok_or("a response without success")? {
                Ok(body)
            } else {
                let detail = body
                    .pointer("/error/format")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                Err(ErrorResponse {
                    message: raw.message.unwrap_or_default(),
                    detail,
                })
            };
            Ok(Incoming::Response {
                request_seq: raw.request_seq.ok_or("a response without request_seq")?,
                command: raw.command.unwrap_or_default(),
                result,
            })
        }
        "event" => Ok(Incoming::Event {
            event: raw.event.ok_or("an event without a name")?,
            body: raw.body.unwrap_or(Value::Null),
        }),
        other => Err(format!("unknown message type {other:?}")),
    }
}

pub fn request(seq: i64, command: &str, arguments: &Value) -> Vec<u8> {
    let mut message = json!({ "seq": seq, "type": "request", "command": command });
    if !arguments.is_null() {
        message["arguments"] = arguments.clone();
    }
    to_bytes(&message)
}

/// 역요청 `request_seq` 의 응답. 실패면 `message` 에 까닭을 싣는다.
pub fn response(
    seq: i64,
    request_seq: i64,
    command: &str,
    result: &Result<Value, String>,
) -> Vec<u8> {
    to_bytes(&match result {
        Ok(body) => {
            let mut message = json!({
                "seq": seq, "type": "response", "request_seq": request_seq, "success": true, "command": command,
            });
            if !body.is_null() {
                message["body"] = body.clone();
            }
            message
        }
        Err(why) => json!({
            "seq": seq, "type": "response", "request_seq": request_seq, "success": false, "command": command,
            "message": why,
        }),
    })
}

/// 이벤트(가짜 어댑터가 쓴다).
pub fn event(seq: i64, event: &str, body: &Value) -> Vec<u8> {
    let mut message = json!({ "seq": seq, "type": "event", "event": event });
    if !body.is_null() {
        message["body"] = body.clone();
    }
    to_bytes(&message)
}

fn to_bytes(value: &Value) -> Vec<u8> {
    // Value 의 직렬화는 실패하지 않는다(맵 키가 늘 글이다).
    serde_json::to_vec(value).unwrap_or_default()
}

/// 요청 하나의 이름 · 인자 · 응답 본문.
pub trait Command {
    const NAME: &'static str;
    type Arguments: Serialize;
    type Body: DeserializeOwned;
}

/// 본문을 보지 않는 응답(아무 꼴이든 받는다).
pub type NoBody = serde::de::IgnoredAny;

macro_rules! command {
    ($(#[$doc:meta])* $ty:ident, $name:literal, $args:ty, $body:ty) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy)]
        pub enum $ty {}

        impl Command for $ty {
            const NAME: &'static str = $name;
            type Arguments = $args;
            type Body = $body;
        }
    };
}

command!(Initialize, "initialize", InitializeArguments, Capabilities);
command!(
    /// 인자는 어댑터마다 다르다(어댑터 정의의 `launch` 틀 — M6-16).
    Launch,
    "launch",
    Map<String, Value>,
    NoBody
);
command!(
    /// 인자는 어댑터마다 다르다(어댑터 정의의 `attach` 틀 — M6-16).
    Attach,
    "attach",
    Map<String, Value>,
    NoBody
);
command!(
    SetBreakpoints,
    "setBreakpoints",
    SetBreakpointsArguments,
    BreakpointsBody
);
command!(
    SetFunctionBreakpoints,
    "setFunctionBreakpoints",
    SetFunctionBreakpointsArguments,
    BreakpointsBody
);
command!(
    SetExceptionBreakpoints,
    "setExceptionBreakpoints",
    SetExceptionBreakpointsArguments,
    OptionalBreakpointsBody
);
command!(ConfigurationDone, "configurationDone", Empty, NoBody);
command!(Threads, "threads", Empty, ThreadsBody);
command!(
    StackTrace,
    "stackTrace",
    StackTraceArguments,
    StackTraceBody
);
command!(Scopes, "scopes", ScopesArguments, ScopesBody);
command!(Variables, "variables", VariablesArguments, VariablesBody);
command!(Evaluate, "evaluate", EvaluateArguments, EvaluateBody);
command!(Continue, "continue", ContinueArguments, ContinueBody);
command!(Next, "next", StepArguments, NoBody);
command!(StepIn, "stepIn", StepArguments, NoBody);
command!(StepOut, "stepOut", StepArguments, NoBody);
command!(Pause, "pause", PauseArguments, NoBody);
command!(Disconnect, "disconnect", DisconnectArguments, NoBody);
command!(Terminate, "terminate", TerminateArguments, NoBody);

/// 인자 없는 요청(`{}` 로 보낸다 — `null` 을 싫어하는 어댑터가 있다).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeArguments {
    #[serde(rename = "clientID", skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(rename = "adapterID")]
    pub adapter_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    pub lines_start_at1: bool,
    pub columns_start_at1: bool,
    /// `path` 만 쓴다 — 역요청의 `cwd` 도 경로로 온다.
    pub path_format: String,
    pub supports_variable_type: bool,
    pub supports_variable_paging: bool,
    pub supports_run_in_terminal_request: bool,
    pub supports_start_debugging_request: bool,
    /// 거짓 — `runInTerminal` 은 셸 없이 argv 로 띄운다.
    pub supports_args_can_be_interpreted_by_shell: bool,
    #[serde(rename = "supportsANSIStyling")]
    pub supports_ansi_styling: bool,
}

/// `initialize` 응답의 능력(쓰는 것만).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Capabilities {
    pub supports_configuration_done_request: bool,
    pub supports_function_breakpoints: bool,
    pub supports_conditional_breakpoints: bool,
    pub supports_hit_conditional_breakpoints: bool,
    pub supports_evaluate_for_hovers: bool,
    pub exception_breakpoint_filters: Vec<ExceptionBreakpointsFilter>,
    pub supports_log_points: bool,
    pub supports_terminate_request: bool,
    pub support_terminate_debuggee: bool,
    pub supports_exception_filter_options: bool,
    pub supports_delayed_stack_trace_loading: bool,
    pub supports_single_thread_execution_requests: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionBreakpointsFilter {
    pub filter: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub supports_condition: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 0 보다 크면 내용은 `source` 요청으로 받는다(경로가 없는 소스).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_reference: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBreakpoint {
    pub line: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_message: Option<String>,
}

/// 어댑터가 정한 브레이크포인트(옮긴 줄 · 확인 여부).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
    /// `pending` · `failed`(1.65 부터).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsArguments {
    pub source: Source,
    pub breakpoints: Vec<SourceBreakpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_modified: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointsBody {
    pub breakpoints: Vec<Breakpoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionalBreakpointsBody {
    #[serde(default)]
    pub breakpoints: Option<Vec<Breakpoint>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionBreakpoint {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetFunctionBreakpointsArguments {
    pub breakpoints: Vec<FunctionBreakpoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetExceptionBreakpointsArguments {
    pub filters: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsBody {
    pub threads: Vec<Thread>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceArguments {
    pub thread_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_frame: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    pub line: i64,
    pub column: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
    /// `normal` · `label` · `subtle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackTraceBody {
    pub stack_frames: Vec<StackFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_frames: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesArguments {
    pub frame_id: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<String>,
    pub variables_reference: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named_variables: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_variables: Option<i64>,
    pub expensive: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopesBody {
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesArguments {
    pub variables_reference: i64,
    /// `indexed` · `named`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate_name: Option<String>,
    pub variables_reference: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named_variables: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_variables: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariablesBody {
    pub variables: Vec<Variable>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateArguments {
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<i64>,
    /// `watch` · `repl` · `hover` · `clipboard` · `variables`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateBody {
    pub result: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub variables_reference: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named_variables: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_variables: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueArguments {
    pub thread_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub single_thread: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueBody {
    /// 없으면 참으로 본다(1.70 이 기본값을 밝혔다).
    #[serde(default)]
    pub all_threads_continued: Option<bool>,
}

/// `next` · `stepIn` · `stepOut` 의 인자.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepArguments {
    pub thread_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub single_thread: Option<bool>,
    /// `statement` · `line` · `instruction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseArguments {
    pub thread_id: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate_debuggee: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend_debuggee: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminateArguments {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<bool>,
}

/// 역요청 `runInTerminal` 의 인자.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInTerminalRequestArguments {
    /// `integrated`(없으면 이것) · `external`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 작업 디렉터리. `pathFormat = "path"` 로 알렸으므로 경로다(1.71 부터 URI 일 수도 있다고 적지만 그것은 `uri` 를 알린 때다).
    pub cwd: String,
    /// 첫째가 실행할 명령.
    pub args: Vec<String>,
    /// 기본 환경에 더하거나(글) 지울(`null`) 변수.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, Option<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_can_be_interpreted_by_shell: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInTerminalResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_process_id: Option<i64>,
}

/// `launch` · `attach`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartMode {
    Launch,
    Attach,
}

impl StartMode {
    pub fn command(self) -> &'static str {
        match self {
            Self::Launch => Launch::NAME,
            Self::Attach => Attach::NAME,
        }
    }
}

/// 역요청 `startDebugging` 의 인자.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartDebuggingRequestArguments {
    pub configuration: Map<String, Value>,
    pub request: StartMode,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoppedEvent {
    /// `step` · `breakpoint` · `exception` · `pause` · `entry` · `goto` · `function breakpoint` · `data breakpoint` · …
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_focus_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_threads_stopped: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hit_breakpoint_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuedEvent {
    pub thread_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_threads_continued: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEvent {
    /// `console` · `important` · `stdout` · `stderr` · `telemetry` · …(없으면 `console`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables_reference: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminatedEvent {
    /// 어댑터가 다시 시작해 달라고 한다(값은 다음 `launch` · `attach` 의 `__restart`). 첫 판은 다시 시작하지 않는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitedEvent {
    pub exit_code: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakpointEvent {
    /// `changed` · `new` · `removed`.
    pub reason: String,
    pub breakpoint: Breakpoint,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEvent {
    /// `started` · `exited`.
    pub reason: String,
    pub thread_id: i64,
}

/// 쓰는 이벤트.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Initialized,
    Stopped(StoppedEvent),
    Continued(ContinuedEvent),
    Output(OutputEvent),
    Terminated(TerminatedEvent),
    Exited(ExitedEvent),
    Breakpoint(BreakpointEvent),
    Thread(ThreadEvent),
}

impl Event {
    /// 이름과 본문에서. 모르는 이벤트면 `Ok(None)`, 아는 이벤트의 본문이 틀렸으면 `Err`.
    pub fn parse(name: &str, body: Value) -> Result<Option<Self>, String> {
        fn typed<T: DeserializeOwned>(name: &str, body: Value) -> Result<T, String> {
            // 본문을 빠뜨린 어댑터가 있다 — `terminated` 처럼 칸이 모두 선택이면 빈 객체로 읽는다.
            let body = if body.is_null() { json!({}) } else { body };
            serde_json::from_value(body).map_err(|e| format!("bad `{name}` event: {e}"))
        }
        Ok(Some(match name {
            "initialized" => Self::Initialized,
            "stopped" => Self::Stopped(typed(name, body)?),
            "continued" => Self::Continued(typed(name, body)?),
            "output" => Self::Output(typed(name, body)?),
            "terminated" => Self::Terminated(typed(name, body)?),
            "exited" => Self::Exited(typed(name, body)?),
            "breakpoint" => Self::Breakpoint(typed(name, body)?),
            "thread" => Self::Thread(typed(name, body)?),
            _ => return Ok(None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_told_apart_by_type() {
        let req = parse(
            br#"{"seq":3,"type":"request","command":"runInTerminal","arguments":{"cwd":"/"}}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            Incoming::Request {
                seq: 3,
                command: "runInTerminal".into(),
                arguments: json!({"cwd": "/"})
            }
        );
        let ok = parse(br#"{"seq":4,"type":"response","request_seq":1,"success":true,"command":"threads","body":{"threads":[]}}"#).unwrap();
        assert_eq!(
            ok,
            Incoming::Response {
                request_seq: 1,
                command: "threads".into(),
                result: Ok(json!({"threads": []}))
            }
        );
        let err = parse(br#"{"seq":5,"type":"response","request_seq":2,"success":false,"command":"evaluate","message":"notStopped","body":{"error":{"id":1,"format":"not stopped"}}}"#).unwrap();
        let Incoming::Response { result: Err(e), .. } = err else {
            panic!("{err:?}")
        };
        assert_eq!(e.message, "notStopped");
        assert_eq!(e.to_string(), "not stopped");
        let ev = parse(br#"{"seq":6,"type":"event","event":"initialized"}"#).unwrap();
        assert_eq!(
            ev,
            Incoming::Event {
                event: "initialized".into(),
                body: Value::Null
            }
        );
        for bad in [
            &b"[]"[..],
            b"{}",
            br#"{"seq":1,"type":"notice"}"#,
            br#"{"seq":1,"type":"response","request_seq":1}"#,
            br#"{"seq":1,"type":"request"}"#,
            br#"{"seq":1,"type":"event"}"#,
            b"not json",
        ] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn written_messages_parse_back() {
        let back = parse(&request(7, "threads", &json!({}))).unwrap();
        assert!(
            matches!(back, Incoming::Request { seq: 7, ref command, .. } if command == "threads")
        );
        let back = parse(&response(8, 2, "runInTerminal", &Err("no".into()))).unwrap();
        assert!(
            matches!(back, Incoming::Response { request_seq: 2, result: Err(ref e), .. } if e.message == "no")
        );
        let back = parse(&event(9, "exited", &json!({"exitCode": 3}))).unwrap();
        let Incoming::Event { event, body } = back else {
            panic!()
        };
        assert_eq!(
            Event::parse(&event, body).unwrap(),
            Some(Event::Exited(ExitedEvent { exit_code: 3 }))
        );
    }

    #[test]
    fn unknown_events_are_none_and_bad_known_events_are_errors() {
        assert_eq!(Event::parse("loadedSource", json!({})).unwrap(), None);
        assert!(Event::parse("stopped", json!({"threadId": 1})).is_err());
        assert_eq!(
            Event::parse("terminated", Value::Null).unwrap(),
            Some(Event::Terminated(TerminatedEvent::default()))
        );
        // 모르는 칸은 버린다.
        assert_eq!(
            Event::parse("thread", json!({"reason":"started","threadId":2,"extra":1})).unwrap(),
            Some(Event::Thread(ThreadEvent {
                reason: "started".into(),
                thread_id: 2
            }))
        );
    }

    #[test]
    fn initialize_arguments_use_the_spec_names() {
        let args = serde_json::to_value(InitializeArguments {
            client_id: Some("pitwall".into()),
            adapter_id: "x".into(),
            path_format: "path".into(),
            supports_ansi_styling: true,
            ..InitializeArguments::default()
        })
        .unwrap();
        assert_eq!(args["clientID"], "pitwall");
        assert_eq!(args["adapterID"], "x");
        assert_eq!(args["supportsANSIStyling"], true);
        assert_eq!(args["supportsRunInTerminalRequest"], false);
        let caps: Capabilities = serde_json::from_value(json!({
            "supportsConfigurationDoneRequest": true,
            "exceptionBreakpointFilters": [{"filter": "uncaught", "label": "Uncaught"}],
            "somethingNew": 1
        }))
        .unwrap();
        assert!(caps.supports_configuration_done_request);
        assert_eq!(caps.exception_breakpoint_filters[0].filter, "uncaught");
    }
}
