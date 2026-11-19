//! 디버그 툴 메서드(17-m6-plan M6-20, `docs/design/05-mcp.md` 「디버그」 표). 그룹 `debug`, 앱 Host 에만 있다(헤드리스 Host 는
//! `pw-debug` 의 메서드를 붙이지 않는다).
//!
//! - `breakpoint_list`(read) · `breakpoint_set`(write) · `breakpoint_set_expr`(exec) · `debug_state`(read). 에이전트가 스텝 · 계속 ·
//!   식 평가를 부르는 툴은 없다(05).
//! - **관문**: `agent_grant`(MCP 세션의 연결 범위 · 레지스트리에 있는 프로젝트). 창 · Host 범위 연결은 거절한다 — 사람의 화면은
//!   `debug.breakpoints.*` · `debug.*` 를 쓴다.
//! - **식은 실행과 같다**(메인 판정 M6-P17): 조건 · 로그 메시지 · hit count 는 어댑터가 디버그 대상 안에서 평가하는 글이다. 그래서
//!   그것을 넣는 길은 `exec` 로 선언한 `breakpoint_set_expr` 하나다 — `breakpoint_set` 의 입력에는 그 칸이 없고(모르는 키는
//!   거절), 식이 있는 꺼진 브레이크포인트를 켜지 못한다(메인 판정 2026-11-19 Q3 — 사람이 꺼 둔 식을 `write` 의 자동 승인으로 다시
//!   돌게 하지 않는다). 끄기 · 지우기는 되고, 끌 때 식은 그대로 남는다. 둘로 가른 것은 선언 등급이 상한이라서다(M6-10 ②).
//! - **두 단계**(`pw_core::registry::plan`): 계획은 그 줄의 앞뒤 브레이크포인트(식은 가리지 않고 그대로 보인다)이고 계획 단계는
//!   아무것도 쓰지 않는다. 승인 뒤에는 계획 때 읽은 그 줄과 지금 것이 같을 때만 바꾼다(`Breakpoints::replace_line` — 다르면
//!   `CONFLICT`, 다시 묻지 않는다). 승인 규칙: `breakpoint_set` 은 기본(`auto_approve` 의 `write` · 세션 허용은 그 툴),
//!   `breakpoint_set_expr` 는 `Remember::Key { key: <식의 해시> }` — 「이 세션 동안 허용」 이 같은 식에만 닿는다.
//! - **미신뢰 프로젝트**에서는 `breakpoint_set_expr` 를 묻기 전에 거절한다(`PERMISSION_DENIED`, `data.reason = "untrusted"` —
//!   메인 판정 2026-11-19 F1): 미신뢰 동안 저장된 식이 신뢰로 바뀐 뒤 사람이 모르게 돌지 않게. `breakpoint_set` 은 미신뢰에서도
//!   된다.
//! - 에이전트가 두거나 바꾼 브레이크포인트에는 표지(`Breakpoint::agent`)를 단다 — 거터가 그린다(M6-18). 저장되므로 사람이 나중에
//!   띄우는 세션에서도 돈다(M6-P11).
//! - **경로**: 프로젝트 루트(워크트리면 그 안의 프로젝트 루트) 안의 일반 파일만(`pw_fs::scope::Roots::readable` — 심링크를 푼 실제
//!   경로). 저장은 루트 기준 상대 경로라 워크트리끼리 같은 목록이다.
//! - **`debug_state`**: 그 프로젝트의 디버그 세션만(`Debugger::find` · `sessions_of`) — 다른 프로젝트의 `sessionId` · 끝난 세션 ·
//!   없는 id 는 모두 오류가 아니라 「세션 없음」 이다(있다는 것을 알리지 않는다). 멈춰 있지 않으면 「실행 중」 만. 멈춰 있으면 정지
//!   까닭 · 스레드 · 위 [`FRAMES`] 프레임 · 맨 위 프레임의 지역 변수 한 단계(펼치지 않는다)를 싣고, 글은 [`TEXT_LIMIT`] 글자에서
//!   자르고 응답은 [`STATE_BUDGET`] 안에서 덜어 낸다. 변수 값에는 비밀이 있을 수 있다 — 감사에는 결과가 남지 않는다(인자 요약과
//!   상세만). 사람의 세션을 읽기만 한다(스텝 · 계속을 부르지 않는다).
//! - **감사**: 입력의 `condition` · `logMessage` · `hitCondition` 은 글자 수만 남는다(`pw_mcp::audit::CONTENT_WORDS` 의
//!   `condition` · `log`). 오류 글에 식을 인용하지 않는다.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_core::registry::agent::{AgentGrant, agent_grant};
use pw_core::registry::path::{WorktreeRootError, worktree_root};
use pw_core::registry::plan::{PlanChange, PlanRules, Planned, Remember, ToolPlan};
use pw_dap::protocol;
use pw_fs::scope::Roots;
use pw_rpc::{ConnectionScope, Router, RpcError, Tier, code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::Debugger;
use crate::breakpoints::{self, Breakpoint, normalized};
use crate::methods::adapter_error;
use crate::run::DebugRun;

/// RPC 이름.
pub const LIST_METHOD: &str = "debug.tool.breakpointList";
pub const SET_METHOD: &str = "debug.tool.breakpointSet";
pub const SET_EXPR_METHOD: &str = "debug.tool.breakpointSetExpr";
pub const STATE_METHOD: &str = "debug.tool.state";

/// `debug_state` 가 싣는 프레임 수(맨 위부터).
pub const FRAMES: usize = 20;
/// `debug_state` 가 싣는 스레드 수.
pub const THREADS: usize = 100;
/// 맨 위 프레임의 영역(scope) 하나에서 읽는 변수 수.
pub const LOCALS_PER_SCOPE: u32 = 100;
/// 값 · 이름 · 타입 글 하나의 상한(글자).
pub const TEXT_LIMIT: usize = 512;
/// `debug_state` 결과(JSON)의 상한. 세션은 256 KiB 를 넘는 응답을 오류로 바꾼다(`pw_mcp::server::MAX_RESPONSE_BYTES`) — 여유를
/// 남긴다(`run_logs` 와 같은 192 KiB).
pub const STATE_BUDGET: usize = 192 * 1024;

// --- 입출력 ----------------------------------------------------------------------------------------------------------

/// A breakpoint as the tools show it.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolBreakpoint {
    /// 1-based line.
    pub line: u32,
    /// Disabled breakpoints are kept but not sent to the debug adapter.
    pub enabled: bool,
    /// Stop only when this expression is true (evaluated inside the debugged program).
    pub condition: Option<String>,
    /// Log this message instead of stopping (`{expr}` parts are evaluated inside the debugged program).
    pub log_message: Option<String>,
    /// Stop only on some hits (the debug adapter's syntax, evaluated by the adapter).
    pub hit_condition: Option<String>,
    /// An agent set or changed it (Pitwall marks it in the editor gutter).
    pub by_agent: bool,
}

impl From<&Breakpoint> for DebugToolBreakpoint {
    fn from(b: &Breakpoint) -> Self {
        Self {
            line: b.line,
            enabled: b.enabled,
            condition: b.condition.clone(),
            log_message: b.log_message.clone(),
            hit_condition: b.hit_condition.clone(),
            by_agent: b.agent,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolBreakpointFile {
    /// Path relative to the project root (`/`-separated). Breakpoints are shared by all worktrees of the project.
    pub path: String,
    /// The file in the worktree asked for (absolute path).
    pub file: String,
    /// By line.
    pub breakpoints: Vec<DebugToolBreakpoint>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugToolBreakpointListParams {
    pub project_id: String,
    /// Top of the worktree (absolute path) that `path` and the listed `file`s are in. Empty means the session's worktree, or
    /// else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Only this file (absolute, or relative to the project root).
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolBreakpointList {
    /// Files with breakpoints, by path.
    pub files: Vec<DebugToolBreakpointFile>,
    /// Why the stored breakpoints could not be read (the file was set aside and the list started empty), if so.
    pub problem: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugToolBreakpointSetParams {
    pub project_id: String,
    /// Top of the worktree (absolute path) that `path` is in. Empty means the session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// A file inside the project root (absolute, or relative to the project root).
    pub path: String,
    /// 1-based line.
    pub line: u32,
    /// Turn the breakpoint on (the default) or off. A breakpoint that has a condition, log message or hit count cannot be
    /// turned on with this tool — use `breakpoint_set_expr`; turning it off keeps them.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Remove the breakpoint at this line (with its condition, log message and hit count).
    #[serde(default)]
    pub remove: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugToolBreakpointSetExprParams {
    pub project_id: String,
    /// Top of the worktree (absolute path) that `path` is in. Empty means the session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// A file inside the project root (absolute, or relative to the project root).
    pub path: String,
    /// 1-based line.
    pub line: u32,
    /// Stop only when this expression is true. It runs inside the debugged program.
    #[serde(default)]
    pub condition: Option<String>,
    /// Log this message instead of stopping; `{expr}` parts run inside the debugged program.
    #[serde(default)]
    pub log_message: Option<String>,
    /// Stop only on some hits, in the debug adapter's syntax (for example `3` or `>= 3`).
    #[serde(default)]
    pub hit_condition: Option<String>,
    /// Turn the breakpoint on (the default) or off.
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolBreakpointSet {
    /// Path relative to the project root.
    pub path: String,
    /// The file (absolute path in the worktree asked for).
    pub file: String,
    pub line: u32,
    /// The breakpoint at that line now (`null` after a removal).
    pub breakpoint: Option<DebugToolBreakpoint>,
    /// All breakpoints of that file now.
    pub breakpoints: Vec<DebugToolBreakpoint>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugToolStateParams {
    pub project_id: String,
    /// A debug session (its run id, `kind = "debug"` in `run_status`). Without it, a stopped session of the project if
    /// there is one, else any.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Where a debug session is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DebugToolSessionState {
    /// No debug session of this project (or no such session).
    NoSession,
    /// The debug adapter is still starting.
    Starting,
    /// Running — not stopped. Nothing else is read.
    Running,
    /// Stopped (at a breakpoint, after a step, on an exception, paused).
    Stopped,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolStop {
    /// `breakpoint`, `step`, `exception`, `pause`, `entry` … (the debug adapter's word).
    pub reason: String,
    pub description: Option<String>,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub thread_id: Option<i64>,
    pub all_threads_stopped: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolThread {
    #[specta(type = pw_rpc::JsNumber)]
    pub id: i64,
    pub name: String,
}

/// A stack frame (the top one first).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolFrame {
    pub name: String,
    /// Source file (absolute path as the debug adapter gave it — it can be outside the project).
    pub path: Option<String>,
    #[specta(type = pw_rpc::JsNumber)]
    pub line: i64,
    #[specta(type = pw_rpc::JsNumber)]
    pub column: i64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolVariable {
    pub name: String,
    /// The value as the debug adapter shows it, cut to 512 characters (`clipped`).
    pub value: String,
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    /// It has children; they are not read (one level only).
    pub expandable: bool,
    /// The name, value or type was cut.
    pub clipped: bool,
}

/// One scope of the top frame (locals, arguments, registers …).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolScope {
    pub name: String,
    /// The debug adapter says this scope is expensive to read; it was not read.
    pub skipped: bool,
    pub variables: Vec<DebugToolVariable>,
    /// More variables than listed (at most 100 per scope are read, and fewer when the response is cut).
    pub more: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolState {
    pub state: DebugToolSessionState,
    /// The debug session shown (its run id).
    pub session_id: Option<String>,
    /// The project's other debug sessions (run ids).
    pub other_sessions: Vec<String>,
    /// Why it stopped (only when stopped).
    pub stop: Option<DebugToolStop>,
    /// Threads (at most 100; only when stopped).
    pub threads: Vec<DebugToolThread>,
    /// The stopped thread's top 20 frames (only when stopped).
    pub frames: Vec<DebugToolFrame>,
    /// The top frame's variables, one level (only when stopped).
    pub locals: Vec<DebugToolScope>,
    /// Frames or variables were left out to keep the response under about 192 KiB.
    pub truncated: bool,
    /// What could not be read from the debug adapter (it may have resumed meanwhile).
    pub problem: Option<String>,
}

impl DebugToolState {
    fn empty(state: DebugToolSessionState) -> Self {
        Self {
            state,
            session_id: None,
            other_sessions: Vec::new(),
            stop: None,
            threads: Vec::new(),
            frames: Vec::new(),
            locals: Vec::new(),
            truncated: false,
            problem: None,
        }
    }
}

rpc_method! {
    /// This project's breakpoints (stored by Pitwall and shared by its worktrees): file, line, on or off, condition, log
    /// message, hit count, and whether an agent set it. `path` limits it to one file. Writes nothing.
    pub DebugToolBreakpointListMethod: "debug.tool.breakpointList" {
        params: DebugToolBreakpointListParams,
        result: DebugToolBreakpointList,
        tier: Read,
        roles: [Owner],
        mcp: { group: "debug", name: "breakpoint_list" },
    }

    /// Set a plain breakpoint at a line of a file inside the project, turn one on or off (`enabled`), or remove one
    /// (`remove`). It takes no condition, log message or hit count — those run inside the debugged program, use
    /// `breakpoint_set_expr`. A breakpoint that already has one can be turned off or removed here but not turned on. The
    /// person approves it in Pitwall first. Breakpoints are stored: they also apply to debug sessions started later, and an
    /// open debug session gets them at once.
    pub DebugToolBreakpointSetMethod: "debug.tool.breakpointSet" {
        params: Planned<DebugToolBreakpointSetParams>,
        result: DebugToolBreakpointSet,
        tier: Write,
        roles: [Owner],
        mcp: { group: "debug", name: "breakpoint_set" },
    }

    /// Set a breakpoint with a condition, a log message (a logpoint) or a hit count at a line of a file inside the project.
    /// These expressions run inside the debugged program each time the line is reached — in open debug sessions at once and
    /// in debug sessions the person starts later (breakpoints are stored) — so this is an `exec` tool: the person sees the
    /// expressions and approves them in Pitwall first. To change only the line or on/off, or to remove it, use
    /// `breakpoint_set`.
    pub DebugToolBreakpointSetExprMethod: "debug.tool.breakpointSetExpr" {
        params: Planned<DebugToolBreakpointSetExprParams>,
        result: DebugToolBreakpointSet,
        tier: Exec,
        roles: [Owner],
        mcp: { group: "debug", name: "breakpoint_set_expr" },
    }

    /// The state of this project's debug session: `noSession`, `starting`, `running`, or `stopped` with why, the threads,
    /// the stopped thread's top 20 frames and the top frame's variables one level deep (values cut to 512 characters, the
    /// response to about 192 KiB). It only reads; there is no tool to step, continue or evaluate. A `sessionId` that is not a
    /// debug session of this project is `noSession`.
    pub DebugToolStateMethod: "debug.tool.state" {
        params: DebugToolStateParams,
        result: DebugToolState,
        tier: Read,
        roles: [Owner],
        mcp: { group: "debug", name: "debug_state" },
    }
}

// --- 경로 --------------------------------------------------------------------------------------------------------------

/// 툴의 경로를 푼 것.
#[derive(Debug, Clone)]
struct Place {
    /// 그 워크트리 안의 프로젝트 루트.
    root: PathBuf,
    /// 루트 기준 상대 경로(저장 열쇠).
    rel: String,
}

impl Place {
    fn file(&self) -> String {
        file_in(&self.root, &self.rel)
    }
}

fn file_in(root: &Path, rel: &str) -> String {
    rel.split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
        .display()
        .to_string()
}

/// 그 워크트리 안의 프로젝트 루트(없으면 주 루트).
fn checkout_root(grant: &AgentGrant, worktree: Option<&str>) -> Result<PathBuf, RpcError> {
    match worktree.filter(|w| !w.is_empty()) {
        None => Ok(grant.project().root().to_path_buf()),
        Some(top) => worktree_root(grant.project().root(), Path::new(top)).map_err(|e| match e {
            WorktreeRootError::Relative => {
                RpcError::invalid_params(format!("worktree `{top}` must be an absolute path"))
            }
            WorktreeRootError::NotOurs => RpcError::invalid_params(format!(
                "`{top}` is not a worktree of project `{}`",
                grant.project_id()
            )),
        }),
    }
}

/// `path` 를 루트 안의 일반 파일로 푼다(머리 주석 「경로」). 블로킹.
fn place(grant: &AgentGrant, worktree: Option<&str>, path: &str) -> Result<Place, RpcError> {
    let root = checkout_root(grant, worktree)?;
    let roots = Roots::confirmed([root.clone()]);
    let given = Path::new(path);
    let requested = if given.is_absolute() {
        given.to_path_buf()
    } else {
        root.join(given)
    };
    let outside = |why: String| {
        RpcError::invalid_params(format!("path: {why}"))
            .with_data(json!({ "reason": "outsideProject" }))
    };
    let found = roots
        .readable(&requested)
        .map_err(|d| outside(d.to_string()))?;
    let rel: Vec<String> = found
        .real
        .strip_prefix(&found.root)
        .map_err(|_| outside(format!("`{path}` is outside the project root")))?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if rel.is_empty() {
        return Err(RpcError::invalid_params(format!(
            "path: `{path}` is the project folder, not a file"
        )));
    }
    let meta = std::fs::metadata(&found.real).map_err(|e| {
        RpcError::invalid_params(format!("path: cannot read `{}`: {e}", found.real.display()))
    })?;
    if !meta.is_file() {
        return Err(RpcError::invalid_params(format!(
            "path: `{}` is not a file",
            found.real.display()
        )));
    }
    let rel = breakpoints::relative_path(&rel.join("/"))?;
    Ok(Place { root, rel })
}

// --- 계획 ----------------------------------------------------------------------------------------------------------------

/// 계획 · 결과에 보이는 브레이크포인트 한 줄(식은 가리지 않는다 — 사람이 승인할 글이다).
fn describe(b: Option<&Breakpoint>) -> String {
    let Some(b) = b else {
        return "none".to_owned();
    };
    let mut out = format!("line {}, {}", b.line, if b.enabled { "on" } else { "off" });
    for (label, text) in [
        ("condition", &b.condition),
        ("log message", &b.log_message),
        ("hit count", &b.hit_condition),
    ] {
        if let Some(text) = text {
            out.push_str(&format!("\n  {label}: {text}"));
        }
    }
    out
}

/// 식의 해시(세션 허용의 열쇠 — 같은 식에만 닿는다).
fn expressions_key(b: &Breakpoint) -> String {
    let covered = json!({
        "condition": b.condition,
        "logMessage": b.log_message,
        "hitCondition": b.hit_condition,
    });
    let mut hasher = Sha256::new();
    hasher.update(covered.to_string().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("breakpoint-expressions:sha256:{hex}")
}

/// 계획 때 읽은 것(블로킹 — 레지스트리 · 경로 · 저장 읽기).
struct Planning {
    place: Place,
    project_id: String,
    before: Option<Breakpoint>,
}

fn planning(
    debugger: &Debugger,
    scope: &ConnectionScope,
    project_id: &str,
    worktree: Option<&str>,
    path: &str,
    line: u32,
    expressions: bool,
) -> Result<Planning, RpcError> {
    let projects = debugger.projects()?;
    let grant = agent_grant(scope, &projects, project_id)?;
    // 식은 신뢰한 프로젝트에서만(메인 판정 2026-11-19 F1): 미신뢰 동안 저장된 식이 신뢰로 바뀐 뒤 사람이 모르게 도는 길을 닫는다.
    // 평범한 브레이크포인트(위치 · 켬/끔 · 지우기)는 미신뢰에서도 둔다.
    if expressions && !projects.is_trusted(grant.project_id()) {
        return Err(RpcError::new(
            code::PERMISSION_DENIED,
            "trust the project before setting breakpoints with expressions — they run inside the debugged program",
        )
        .with_data(json!({ "reason": "untrusted", "projectId": grant.project_id() })));
    }
    if line == 0 {
        return Err(RpcError::invalid_params("breakpoint lines start at 1"));
    }
    let place = place(&grant, worktree, path)?;
    let stored = debugger.breakpoints()?.get(grant.project_id())?;
    if stored.locked {
        return Err(RpcError::internal(format!(
            "the breakpoints are not saved: {}",
            stored
                .problem
                .as_deref()
                .unwrap_or("the file could not be read")
        )));
    }
    let before = stored
        .files
        .get(&place.rel)
        .and_then(|list| list.iter().find(|b| b.line == line))
        .cloned();
    Ok(Planning {
        place,
        project_id: grant.project_id().to_owned(),
        before,
    })
}

fn set_result(place: &Place, line: u32, saved: &[Breakpoint]) -> DebugToolBreakpointSet {
    DebugToolBreakpointSet {
        path: place.rel.clone(),
        file: place.file(),
        line,
        breakpoint: saved.iter().find(|b| b.line == line).map(Into::into),
        breakpoints: saved.iter().map(Into::into).collect(),
    }
}

impl Debugger {
    /// `breakpoint_list`(블로킹).
    pub fn tool_breakpoint_list(
        &self,
        scope: &ConnectionScope,
        p: &DebugToolBreakpointListParams,
    ) -> Result<DebugToolBreakpointList, RpcError> {
        let grant = agent_grant(scope, &self.projects()?, &p.project_id)?;
        let root = checkout_root(&grant, p.worktree.as_deref())?;
        let only = match &p.path {
            Some(path) => Some(place(&grant, p.worktree.as_deref(), path)?.rel),
            None => None,
        };
        let stored = self.breakpoints()?.get(grant.project_id())?;
        Ok(DebugToolBreakpointList {
            files: stored
                .files
                .iter()
                .filter(|(rel, list)| !list.is_empty() && only.as_ref().is_none_or(|o| o == *rel))
                .map(|(rel, list)| DebugToolBreakpointFile {
                    path: rel.clone(),
                    file: file_in(&root, rel),
                    breakpoints: list.iter().map(Into::into).collect(),
                })
                .collect(),
            problem: stored.problem,
        })
    }

    /// `breakpoint_set`: 계획(블로킹 부분) → 승인 → 그 줄만 바꾼다.
    pub async fn tool_breakpoint_set(
        &self,
        scope: &ConnectionScope,
        planned: &Planned<DebugToolBreakpointSetParams>,
    ) -> Result<DebugToolBreakpointSet, RpcError> {
        let p = &planned.params;
        let (me, scope2, params) = (self.clone(), scope.clone(), p.clone());
        let plan = blocking(move || {
            planning(
                &me,
                &scope2,
                &params.project_id,
                params.worktree.as_deref(),
                &params.path,
                params.line,
                false,
            )
        })
        .await?;
        let at = format!("{}:{}", plan.place.rel, p.line);
        let enabled = p.enabled.unwrap_or(true);
        let after = if p.remove {
            if plan.before.is_none() {
                return Err(RpcError::new(
                    code::NOT_FOUND,
                    format!("there is no breakpoint at {at}"),
                ));
            }
            None
        } else {
            match &plan.before {
                None => Some(Breakpoint {
                    enabled,
                    agent: true,
                    ..Breakpoint::at(p.line)
                }),
                Some(b) => {
                    if b.has_expressions() && !b.enabled && enabled {
                        return Err(RpcError::invalid_params(format!(
                            "the breakpoint at {at} has a condition, log message or hit count, which runs inside the \
                             debugged program; breakpoint_set cannot turn it on — use breakpoint_set_expr so the person \
                             approves the expressions"
                        ))
                        .with_data(json!({ "reason": "hasExpressions" })));
                    }
                    Some(Breakpoint {
                        enabled,
                        agent: true,
                        ..b.clone()
                    })
                }
            }
        };
        let summary = match (&plan.before, &after) {
            (_, None) => format!("Remove the breakpoint at {at}"),
            (None, Some(_)) => format!("Set a breakpoint at {at}"),
            (Some(_), Some(b)) => format!(
                "Turn the breakpoint at {at} {}",
                if b.enabled { "on" } else { "off" }
            ),
        };
        let tool_plan = ToolPlan::builder(SET_METHOD, Tier::Write, p)
            .summary(summary)
            .target("before", &plan.before)
            .change(PlanChange::Text {
                label: format!("Breakpoint {at}"),
                text: format!(
                    "Before: {}\nAfter: {}",
                    describe(plan.before.as_ref()),
                    describe(after.as_ref())
                ),
            })
            .rules(PlanRules::default())
            .build();
        planned.approve(tool_plan)?;
        // 승인 뒤에는 계획 때 읽은 그 줄이 그대로일 때만 바꾼다(머리 주석 「두 단계」).
        let saved = self
            .replace_breakpoint(
                &plan.project_id,
                &plan.place.rel,
                p.line,
                plan.before.as_ref(),
                after,
            )
            .await?;
        Ok(set_result(&plan.place, p.line, &saved.breakpoints))
    }

    /// `breakpoint_set_expr`.
    pub async fn tool_breakpoint_set_expr(
        &self,
        scope: &ConnectionScope,
        planned: &Planned<DebugToolBreakpointSetExprParams>,
    ) -> Result<DebugToolBreakpointSet, RpcError> {
        let p = &planned.params;
        let (me, scope2, params) = (self.clone(), scope.clone(), p.clone());
        let plan = blocking(move || {
            planning(
                &me,
                &scope2,
                &params.project_id,
                params.worktree.as_deref(),
                &params.path,
                params.line,
                true,
            )
        })
        .await?;
        let at = format!("{}:{}", plan.place.rel, p.line);
        // 저장과 같은 정리(빈 글은 없음, 글 상한) — 승인을 묻기 전에 거절한다.
        let mut checked = normalized(vec![Breakpoint {
            line: p.line,
            enabled: p.enabled.unwrap_or(true),
            condition: p.condition.clone(),
            log_message: p.log_message.clone(),
            hit_condition: p.hit_condition.clone(),
            agent: true,
        }])?;
        let after = checked.pop().expect("one breakpoint in, one out");
        if !after.has_expressions() {
            return Err(RpcError::invalid_params(
                "give a condition, log message or hit count; for a plain breakpoint use breakpoint_set",
            ));
        }
        let mut builder = ToolPlan::builder(SET_EXPR_METHOD, Tier::Exec, p)
            .summary(format!(
                "Set a breakpoint with expressions that run inside the debugged program at {at}"
            ))
            .target("before", &plan.before)
            .change(PlanChange::Text {
                label: format!("Breakpoint {at}"),
                text: format!(
                    "Before: {}\nAfter: {}",
                    describe(plan.before.as_ref()),
                    describe(Some(&after))
                ),
            });
        for (label, text) in [
            ("Condition", &after.condition),
            ("Log message", &after.log_message),
            ("Hit count", &after.hit_condition),
        ] {
            if let Some(text) = text {
                builder = builder.change(PlanChange::Text {
                    label: label.to_owned(),
                    text: text.clone(),
                });
            }
        }
        let tool_plan = builder
            .change(PlanChange::Text {
                label: "Warning".to_owned(),
                text: "The debug adapter evaluates these inside the debugged program each time the line is reached — in \
                       open debug sessions now and in debug sessions started later (breakpoints are stored)."
                    .to_owned(),
            })
            .rules(PlanRules {
                remember: Remember::Key {
                    key: expressions_key(&after),
                },
                ..PlanRules::default()
            })
            .build();
        planned.approve(tool_plan)?;
        let saved = self
            .replace_breakpoint(
                &plan.project_id,
                &plan.place.rel,
                p.line,
                plan.before.as_ref(),
                Some(after),
            )
            .await?;
        Ok(set_result(&plan.place, p.line, &saved.breakpoints))
    }

    /// `debug_state`.
    pub async fn tool_state(
        &self,
        scope: &ConnectionScope,
        p: &DebugToolStateParams,
    ) -> Result<DebugToolState, RpcError> {
        let (me, scope2, project_id) = (self.clone(), scope.clone(), p.project_id.clone());
        let grant = blocking(move || agent_grant(&scope2, &me.projects()?, &project_id)).await?;
        let project = grant.project_id();
        let sessions: Vec<Arc<DebugRun>> = self
            .sessions_of(project)
            .into_iter()
            .filter(|r| !r.is_finished())
            .collect();
        let chosen = match &p.session_id {
            // 다른 프로젝트 · 없는 · 끝난 세션은 모두 「세션 없음」(머리 주석).
            Some(id) => self.find(project, id).ok().filter(|r| !r.is_finished()),
            None => sessions
                .iter()
                .find(|r| !r.stops().is_empty())
                .or_else(|| sessions.first())
                .cloned(),
        };
        let Some(run) = chosen else {
            return Ok(DebugToolState::empty(DebugToolSessionState::NoSession));
        };
        let mut state = DebugToolState::empty(DebugToolSessionState::Running);
        state.session_id = Some(run.run_id().to_owned());
        state.other_sessions = sessions
            .iter()
            .filter(|r| r.run_id() != run.run_id())
            .map(|r| r.run_id().to_owned())
            .collect();
        let Some(tree) = run.session() else {
            state.state = DebugToolSessionState::Starting;
            return Ok(state);
        };
        let root = tree.root().id();
        let stops = run.stops();
        let Some((session_id, stop)) = stops
            .iter()
            .find(|(id, _)| *id == root)
            .or_else(|| stops.first())
            .cloned()
        else {
            return Ok(state);
        };
        let Some(session) = tree.session(session_id) else {
            return Ok(state);
        };
        state.state = DebugToolSessionState::Stopped;
        state.stop = Some(DebugToolStop {
            reason: clip(&stop.reason).0,
            description: stop.description.as_deref().map(|d| clip(d).0),
            thread_id: stop.thread_id,
            all_threads_stopped: stop.all_threads_stopped,
        });
        if let Err(e) = read_stopped(&session, stop.thread_id, &mut state).await {
            state.problem = Some(clip(&e.message).0);
        }
        fit(&mut state);
        Ok(state)
    }
}

/// 멈춘 세션에서 스레드 · 프레임 · 맨 위 프레임의 변수 한 단계를 읽는다.
async fn read_stopped(
    session: &pw_dap::Session,
    thread: Option<i64>,
    state: &mut DebugToolState,
) -> Result<(), RpcError> {
    let threads = session
        .request::<protocol::Threads>(&protocol::Empty {})
        .await
        .map_err(adapter_error)?;
    state.threads = threads
        .threads
        .iter()
        .take(THREADS)
        .map(|t| DebugToolThread {
            id: t.id,
            name: clip(&t.name).0,
        })
        .collect();
    let Some(thread) = thread.or_else(|| threads.threads.first().map(|t| t.id)) else {
        return Ok(());
    };
    let stack = session
        .request::<protocol::StackTrace>(&protocol::StackTraceArguments {
            thread_id: thread,
            start_frame: Some(0),
            levels: Some(FRAMES as i64),
        })
        .await
        .map_err(adapter_error)?;
    state.frames = stack
        .stack_frames
        .iter()
        .take(FRAMES)
        .map(|f| DebugToolFrame {
            name: clip(&f.name).0,
            path: f
                .source
                .as_ref()
                .and_then(|s| s.path.as_deref())
                .map(|p| clip(p).0),
            line: f.line,
            column: f.column,
        })
        .collect();
    let Some(top) = stack.stack_frames.first() else {
        return Ok(());
    };
    let scopes = session
        .request::<protocol::Scopes>(&protocol::ScopesArguments { frame_id: top.id })
        .await
        .map_err(adapter_error)?;
    for s in scopes.scopes {
        let mut scope = DebugToolScope {
            name: clip(&s.name).0,
            skipped: s.expensive,
            variables: Vec::new(),
            more: false,
        };
        if !s.expensive && s.variables_reference > 0 {
            let body = session
                .request::<protocol::Variables>(&protocol::VariablesArguments {
                    variables_reference: s.variables_reference,
                    filter: None,
                    start: Some(0),
                    count: Some(i64::from(LOCALS_PER_SCOPE)),
                })
                .await
                .map_err(adapter_error)?;
            let known = s
                .named_variables
                .unwrap_or(0)
                .saturating_add(s.indexed_variables.unwrap_or(0));
            scope.more = body.variables.len() > LOCALS_PER_SCOPE as usize
                || known > i64::from(LOCALS_PER_SCOPE);
            scope.variables = body
                .variables
                .into_iter()
                .take(LOCALS_PER_SCOPE as usize)
                .map(|v| {
                    let (name, a) = clip(&v.name);
                    let (value, b) = clip(&v.value);
                    let (type_name, c) = match v.type_name.as_deref().map(clip) {
                        Some((t, cut)) => (Some(t), cut),
                        None => (None, false),
                    };
                    DebugToolVariable {
                        name,
                        value,
                        type_name,
                        expandable: v.variables_reference > 0,
                        clipped: a || b || c,
                    }
                })
                .collect();
        }
        state.locals.push(scope);
    }
    Ok(())
}

/// [`TEXT_LIMIT`] 글자에서 자른다. 잘랐으면 참.
fn clip(text: &str) -> (String, bool) {
    match text.char_indices().nth(TEXT_LIMIT) {
        Some((end, _)) => (format!("{}…", &text[..end]), true),
        None => (text.to_owned(), false),
    }
}

fn size(state: &DebugToolState) -> usize {
    serde_json::to_vec(state).map_or(usize::MAX, |v| v.len())
}

/// 응답을 [`STATE_BUDGET`] 안으로: 뒤 영역의 변수부터, 그다음 아래 프레임 · 스레드부터 덜어 낸다. 덜어 낸 항목의 크기(쉼표
/// 하나 더)를 빼 가며 세고, 끝에 한 번 더 재서 넘으면 되풀이한다.
fn fit(state: &mut DebugToolState) {
    fn len<T: Serialize>(item: &T) -> usize {
        serde_json::to_vec(item).map_or(0, |v| v.len()) + 1
    }
    loop {
        let mut total = size(state);
        if total <= STATE_BUDGET {
            return;
        }
        state.truncated = true;
        let before = total;
        while total > STATE_BUDGET {
            if let Some(scope) = state
                .locals
                .iter_mut()
                .rev()
                .find(|s| !s.variables.is_empty())
            {
                total -= scope.variables.pop().as_ref().map_or(0, len);
                scope.more = true;
            } else if state.frames.len() > 1 {
                total -= state.frames.pop().as_ref().map_or(0, len);
            } else if let Some(thread) = state.threads.pop() {
                total -= len(&thread);
            } else {
                return;
            }
        }
        if size(state) >= before {
            return;
        }
    }
}

/// 블로킹 스레드에서 돌린다(레지스트리 · 경로 · 저장 읽기).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 핸들러를 붙인다(`methods::register` 가 부른다).
pub(crate) fn register(router: &mut Router, debugger: Debugger) {
    let (list, set, set_expr, state) = (
        debugger.clone(),
        debugger.clone(),
        debugger.clone(),
        debugger,
    );
    router
        .unary(
            DebugToolBreakpointListMethod,
            move |ctx, p: DebugToolBreakpointListParams| {
                let (d, scope) = (list.clone(), ctx.scope().clone());
                blocking(move || d.tool_breakpoint_list(&scope, &p))
            },
        )
        .unary(
            DebugToolBreakpointSetMethod,
            move |ctx, planned: Planned<DebugToolBreakpointSetParams>| {
                let (d, scope) = (set.clone(), ctx.scope().clone());
                async move { d.tool_breakpoint_set(&scope, &planned).await }
            },
        )
        .unary(
            DebugToolBreakpointSetExprMethod,
            move |ctx, planned: Planned<DebugToolBreakpointSetExprParams>| {
                let (d, scope) = (set_expr.clone(), ctx.scope().clone());
                async move { d.tool_breakpoint_set_expr(&scope, &planned).await }
            },
        )
        .unary(DebugToolStateMethod, move |ctx, p: DebugToolStateParams| {
            let (d, scope) = (state.clone(), ctx.scope().clone());
            async move { d.tool_state(&scope, &p).await }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_are_cut_at_the_limit() {
        let long = "é".repeat(TEXT_LIMIT + 5);
        let (cut, clipped) = clip(&long);
        assert!(clipped);
        assert_eq!(
            cut.chars().count(),
            TEXT_LIMIT + 1,
            "the limit plus the ellipsis"
        );
        assert_eq!(clip("short"), ("short".to_owned(), false));
    }

    #[test]
    fn the_expression_key_follows_the_expressions_only() {
        let a = Breakpoint {
            condition: Some("x > 1".into()),
            ..Breakpoint::at(3)
        };
        let moved = Breakpoint {
            line: 9,
            ..a.clone()
        };
        let other = Breakpoint {
            condition: Some("x > 2".into()),
            ..Breakpoint::at(3)
        };
        assert_eq!(expressions_key(&a), expressions_key(&moved));
        assert_ne!(expressions_key(&a), expressions_key(&other));
        // 로그 메시지 · hit count 만 다른 짝도 다른 열쇠다 — 세션 허용이 그 식에 닿지 않는다.
        let logged = Breakpoint {
            log_message: Some("x={x}".into()),
            ..a.clone()
        };
        let counted = Breakpoint {
            hit_condition: Some(">= 2".into()),
            ..a.clone()
        };
        assert_ne!(expressions_key(&a), expressions_key(&logged));
        assert_ne!(expressions_key(&a), expressions_key(&counted));
        assert_ne!(expressions_key(&logged), expressions_key(&counted));
    }

    /// 응답 상한: 큰 변수가 많으면 덜어 내고 `truncated` 를 켠다.
    #[test]
    fn a_big_state_is_cut_under_the_budget() {
        let variable = DebugToolVariable {
            name: "v".into(),
            value: "x".repeat(TEXT_LIMIT),
            type_name: None,
            expandable: false,
            clipped: false,
        };
        let mut state = DebugToolState::empty(DebugToolSessionState::Stopped);
        state.locals = (0..10)
            .map(|i| DebugToolScope {
                name: format!("s{i}"),
                skipped: false,
                variables: vec![variable.clone(); LOCALS_PER_SCOPE as usize],
                more: false,
            })
            .collect();
        assert!(size(&state) > STATE_BUDGET);
        fit(&mut state);
        assert!(size(&state) <= STATE_BUDGET);
        assert!(state.truncated);
        assert!(state.locals.last().unwrap().more);
    }
}
