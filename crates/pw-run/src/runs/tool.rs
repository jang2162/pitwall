//! 실행 툴 ① 조회 — `run_config_list` · `run_status` · `run_logs`(17-m6-plan M6-12, `docs/design/05-mcp.md` 「실행 구성」 표).
//! 그룹 `run`, 모두 `read`, 앱 Host 에만 있다(헤드리스 Host 는 `pw-run` 의 메서드를 등록하지 않는다).
//!
//! - **관문**: `agent_grant`(MCP 세션의 연결 범위 · 레지스트리에 있는 프로젝트, `pw_core::registry::agent`). 창 · Host 범위 연결은
//!   거절한다 — 사람의 화면은 `run.config.list` · `run.list` · `run.attach` 를 쓴다.
//! - **범위**: `runId` 를 받는 툴은 관문을 지난 `projectId` 의 실행만 본다(`Runs::find`). 다른 프로젝트의 `runId` 는 없는 실행과 같은
//!   오류(`NOT_FOUND`)다 — 있다는 것을 알리지 않는다(M6-12 ④). 사람이 띄운 실행의 상태 · 로그도 읽는다 — 읽기는 조회 등급이고
//!   결정 D69 가 막는 것은 다루기(`run_stop` · `run_restart`)다.
//! - **띄운 자리**: 사람(창)인지 에이전트인지만 보인다. 에이전트 세션 id · 클라이언트 이름은 싣지 않는다 — 에이전트는 다른 세션의
//!   정보를 읽지 못한다(16-m5-plan M5-10 — `mcp.sessions` 는 에이전트 범위 연결을 거절한다).
//! - **구성의 env 값**: 키 이름이 비밀을 가리키는 값은 `[redacted]`, 그 밖의 값은 URL 비밀번호 자리를 가린다([`redacted`] —
//!   판정은 `pw_run::config::shown`). 개인 전용 폴더(앱 설정 폴더)의 `.local.toml` 은 에이전트가 파일 툴로 읽을 수 없는 자리다.
//! - **로그**: 프로그램이 찍은 출력이라 내용은 가리지 않는다. 터미널 제어(ANSI 이스케이프 · 제어 문자)만 걷는다([`text`]). 응답은
//!   [`LOG_BUDGET`] 안에서 자른다(세션 상한 256 KiB 아래 — 결정 M5-D34). 오프셋은 링 버퍼의 바이트 오프셋(`run.attach` 의 `since`
//!   와 같다)이다.

#[cfg(test)]
mod tests;
mod text;

use std::collections::BTreeMap;

use pw_core::registry::agent::agent_grant;
use pw_rpc::{ConnectionScope, Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{LaunchedBy, Run, RunExit, RunId, RunInfo, RunLog, RunState, Runs};
use crate::config::read::RunConfigLayer;
use crate::config::{RunConfig, RunConfigPlace, read, shown};
use crate::detect::exec::CacheOnly;
use crate::detect::{self, TaskListState};

pub use text::clean_line;

/// `run_logs` 결과의 글(JSON 으로 이스케이프한 크기)의 상한. 세션은 256 KiB 를 넘는 응답을 「범위를 좁혀라」 오류로 바꾼다
/// (`pw_mcp::server::MAX_RESPONSE_BYTES`) — 나머지 칸과 여유를 남긴다(git · LSP 툴과 같은 192 KiB).
pub const LOG_BUDGET: usize = 192 * 1024;
/// `tail` 의 기본(05 「설계 메모」 — 기본 마지막 200줄).
pub const DEFAULT_TAIL: u32 = 200;
/// `tail` 의 상한.
pub const MAX_TAIL: u32 = 10_000;
/// `since` 만 준 호출이 한 번에 훑는 원래 바이트의 상한. 그 뒤는 `nextSince` 로 잇는다.
pub const SINCE_WINDOW: usize = 1024 * 1024;
/// `grep` 정규식 글의 상한(글자 수)과 컴파일한 크기의 상한.
pub const GREP_MAX_CHARS: usize = 1000;
pub const GREP_SIZE_LIMIT: usize = 1024 * 1024;
/// 가린 env 값(`settings_get` 의 `[redacted]` 와 같다).
pub const REDACTED: &str = "[redacted]";

// --- run_config_list ------------------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolConfigListParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Shared configurations (`.pitwall/run/*.toml`) are read from that worktree and
    /// `cwd` is relative to the project root inside it. Empty means the session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
}

/// Which file a configuration value came from.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RunToolConfigFileKind {
    /// `.pitwall/run/<id>.toml` — committed with the repository.
    Shared,
    /// `<id>.local.toml` — the person's own file (not committed).
    Local,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolConfigFile {
    pub kind: RunToolConfigFileKind,
    /// Absolute path.
    pub path: String,
}

/// A usable run configuration.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolConfig {
    /// Configuration id (the file name) — `configId` for `run_start`.
    pub id: String,
    /// The merged configuration. `env` values whose name points to a secret (`TOKEN`, `PASSWORD`, `SECRET`, `KEY` …) are
    /// shown as `[redacted]`, and the password in a URL inside any other `env` value (`postgres://user:pass@host`) is
    /// masked. Commands and arguments are shown as written.
    pub config: RunConfig,
    /// Files that gave values (shared first).
    pub files: Vec<RunToolConfigFile>,
}

/// A configuration file that cannot be used, and why.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolUnusableConfig {
    pub id: String,
    /// The file's `name`, if it could be read.
    pub name: Option<String>,
    /// Why (which file and what — an unknown `type`, a TOML error with its line and column, a `cwd` outside the root …).
    /// File contents are not quoted.
    pub reason: String,
    pub files: Vec<RunToolConfigFile>,
}

/// A task found in the project's files without a configuration file (npm scripts, Make targets, just recipes, compose
/// services).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolDetectedTask {
    /// `detected:<detector>:<name>`.
    pub key: String,
    /// Detector id (`npm`, `make`, `just`, `compose` …).
    pub detector: String,
    /// The file it was found in (absolute path).
    pub source: String,
    /// The configuration it would run as. Save it with `run_config_upsert` to make it a configuration file.
    pub config: RunConfig,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolDetectProblem {
    pub detector: String,
    /// Why the detector failed or left a task out (which file · what · which line). File contents are not quoted.
    pub reason: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RunToolTaskListState {
    /// Not read yet — only the person can have Pitwall read it (it runs the build tool).
    NotRead,
    /// Read; the listed tasks are current.
    Read,
    /// Read before the build files changed — only the common tasks are listed until the person reads it again.
    Stale,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolTaskList {
    pub detector: String,
    pub state: RunToolTaskListState,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolConfigList {
    /// Usable configurations (by id).
    pub configs: Vec<RunToolConfig>,
    /// Configuration files that cannot be used (by id).
    pub unusable: Vec<RunToolUnusableConfig>,
    /// Detected tasks (by detector, then name).
    pub detected: Vec<RunToolDetectedTask>,
    pub detect_problems: Vec<RunToolDetectProblem>,
    /// Detectors whose full task list is read only when the person asks for it in Pitwall (gradle, maven) — until then
    /// only their common tasks are listed.
    pub task_lists: Vec<RunToolTaskList>,
    /// Why the configuration folder itself could not be read. Its configurations are not listed.
    pub problem: Option<String>,
}

// --- run_status -----------------------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolStatusParams {
    pub project_id: String,
    /// One run. Without it, every run of the project that Pitwall still keeps.
    #[serde(default)]
    pub run_id: Option<RunId>,
}

/// Who started a run.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RunToolLauncher {
    /// A person, from the project window.
    Person,
    /// An agent, through a run tool.
    Agent,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RunToolState {
    Running,
    /// A stop was sent and the run has not ended yet.
    Stopping,
    Exited,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolExit {
    /// Exit code. `null` if a signal ended it.
    pub code: Option<i32>,
    /// The signal that ended it (for example `Terminated: 15`). `null` if it exited with a code.
    pub signal: Option<String>,
}

/// One run.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolRun {
    /// `runId` for `run_logs`.
    pub run_id: RunId,
    /// Configuration id, or the detected task's key.
    pub config: String,
    pub name: String,
    /// `process` (later also `sql` and `debug`).
    pub kind: String,
    pub launched_by: RunToolLauncher,
    /// The worktree the person's window had active, if not the main checkout.
    pub worktree: Option<String>,
    /// Working directory (absolute path).
    pub cwd: Option<String>,
    pub profile: Option<String>,
    pub pid: Option<u32>,
    pub state: RunToolState,
    /// Set once the run has ended.
    pub exit: Option<RunToolExit>,
    /// Start time (Unix milliseconds).
    #[specta(type = pw_rpc::JsNumber)]
    pub started_at: i64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub ended_at: Option<i64>,
    /// The run this one restarted.
    pub restart_of: Option<RunId>,
    /// First log offset still kept (output before it was dropped).
    #[specta(type = pw_rpc::JsNumber)]
    pub log_start: u64,
    /// Log end offset (bytes written so far).
    #[specta(type = pw_rpc::JsNumber)]
    pub log_end: u64,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolStatus {
    /// Running runs and the finished runs Pitwall still keeps (the latest 20 per project), oldest start first.
    pub runs: Vec<RunToolRun>,
}

// --- run_logs -------------------------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolLogsParams {
    pub project_id: String,
    pub run_id: RunId,
    /// The last this many lines (after `grep`). Default 200 without `since`; with `since`, all lines from there. Up to
    /// 10000.
    #[serde(default)]
    pub tail: Option<u32>,
    /// Log offset to read from (`nextSince` of the previous call). If that output was already dropped, reading starts
    /// at the first byte still kept and `truncated` is set.
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub since: Option<u64>,
    /// Regular expression (Rust `regex` syntax, up to 1000 characters); only lines matching it. Matched against the
    /// text with terminal escapes removed.
    #[serde(default)]
    pub grep: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolLogs {
    pub run_id: RunId,
    pub state: Option<RunToolState>,
    pub exit: Option<RunToolExit>,
    /// The lines, joined by `\n`, with terminal escapes (colors, cursor moves) and control characters removed. A line
    /// still being written (no newline yet) is included.
    pub text: String,
    /// Number of lines in `text`.
    pub lines: u32,
    /// Offset of the first returned line.
    #[specta(type = pw_rpc::JsNumber)]
    pub from: u64,
    /// Pass as `since` to read on from here (new output, or the rest when the result was cut).
    #[specta(type = pw_rpc::JsNumber)]
    pub next_since: u64,
    /// First log offset still kept. Output before it was dropped (the log keeps the last 8 MiB of a running run and
    /// the last 1 MiB of a finished one).
    #[specta(type = pw_rpc::JsNumber)]
    pub log_start: u64,
    /// Log end offset.
    #[specta(type = pw_rpc::JsNumber)]
    pub log_end: u64,
    /// Not everything asked for is here: output at `since` was dropped (`dropped`), or the result was cut to fit the
    /// response size (about 192 KiB — older lines are left out with `tail`, later ones with `since`; read on from
    /// `nextSince`).
    pub truncated: bool,
    /// Bytes between `since` and `logStart` that were dropped before this call.
    #[specta(type = pw_rpc::JsNumber)]
    pub dropped: u64,
}

rpc_method! {
    /// This project's run configurations: usable ones (merged from the shared `.pitwall/run/<id>.toml` and the
    /// person's `<id>.local.toml`), files that cannot be used and why, and tasks detected from project files (npm
    /// scripts, Make targets, just recipes, compose services — with the file they came from). `env` values whose name
    /// points to a secret are `[redacted]` and URL passwords in `env` values are masked. Runs nothing and writes nothing.
    pub RunToolConfigListMethod: "run.tool.configList" {
        params: RunToolConfigListParams,
        result: RunToolConfigList,
        tier: Read,
        roles: [Owner],
        mcp: { group: "run", name: "run_config_list" },
    }

    /// This project's runs: running ones and the finished ones Pitwall still keeps (the latest 20 per project) — state,
    /// exit code or signal, pid, working directory, start and end times, and whether a person (from the Pitwall window)
    /// or an agent started it. With `runId`, that run only (`not found` if it is not a run of this project).
    pub RunToolStatusMethod: "run.tool.status" {
        params: RunToolStatusParams,
        result: RunToolStatus,
        tier: Read,
        roles: [Owner],
        mcp: { group: "run", name: "run_status" },
    }

    /// Read a run's output (also of runs a person started). By default the last 200 lines; `tail` sets the count,
    /// `since` reads on from an offset (`nextSince` of an earlier call — poll with it for new output), `grep` keeps
    /// only lines matching a regular expression. Terminal escapes are removed. The result is cut to about 192 KiB
    /// (`truncated`). Output is kept in memory only: the last 8 MiB while running, the last 1 MiB after the run ends,
    /// and only for the latest 20 finished runs of the project — older output is gone (`logStart`, `dropped`). A
    /// `runId` that is not a run of this project is `not found`.
    pub RunToolLogsMethod: "run.tool.logs" {
        params: RunToolLogsParams,
        result: RunToolLogs,
        tier: Read,
        roles: [Owner],
        mcp: { group: "run", name: "run_logs" },
    }
}

impl Runs {
    /// 관문을 지난 프로젝트(블로킹 — 레지스트리 읽기).
    fn tool_grant(
        &self,
        scope: &ConnectionScope,
        project_id: &str,
    ) -> Result<pw_core::registry::agent::AgentGrant, RpcError> {
        let host = self
            .inner
            .host
            .get()
            .ok_or_else(|| RpcError::internal("runs are not attached to a host"))?;
        agent_grant(scope, &host.projects, project_id)
    }

    /// `run_config_list`(블로킹). `run.config.list` 와 같은 읽기 · 감지이되 감지 기억(`detect::Seen`)은 건드리지 않는다 — 그것은
    /// 창의 목록이 바뀌었을 때 알리려고 둔 것이라, 툴이 기억을 바꾸면 창이 변경 알림을 놓친다.
    pub fn tool_config_list(
        &self,
        scope: &ConnectionScope,
        params: &RunToolConfigListParams,
    ) -> Result<RunToolConfigList, RpcError> {
        let grant = self.tool_grant(scope, &params.project_id)?;
        let host = self
            .inner
            .host
            .get()
            .ok_or_else(|| RpcError::internal("runs are not attached to a host"))?;
        let place = RunConfigPlace::new(&host.paths, grant.project(), params.worktree.as_deref())?;
        let set = read(&place);
        // 창이 아닌 연결의 `run.config.list` 와 같다(M6-05): 프로세스를 띄우는 감지(mise · gradle · maven)는 돌리지 않고 캐시만 읽는다.
        let exec = CacheOnly::new(crate::config::methods::cache_dir(
            &host.paths,
            grant.project_id(),
        ));
        let detection = detect::detect(&place.root, &exec);
        Ok(RunToolConfigList {
            configs: set
                .configs
                .into_iter()
                .map(|c| RunToolConfig {
                    id: c.id,
                    config: redacted(c.config),
                    files: files(&c.layers),
                })
                .collect(),
            unusable: set
                .unusable
                .into_iter()
                .map(|u| RunToolUnusableConfig {
                    id: u.id,
                    name: u.name,
                    reason: u.reason,
                    files: files(&u.layers),
                })
                .collect(),
            detected: detection
                .tasks
                .into_iter()
                .map(|t| RunToolDetectedTask {
                    key: t.key(),
                    detector: t.detector.to_owned(),
                    source: t.source.display().to_string(),
                    config: redacted(t.config),
                })
                .collect(),
            detect_problems: detection
                .problems
                .into_iter()
                .map(|p| RunToolDetectProblem {
                    detector: p.detector,
                    reason: p.reason,
                })
                .collect(),
            task_lists: detection
                .lists
                .into_iter()
                .map(|l| RunToolTaskList {
                    detector: l.detector,
                    state: match l.state {
                        TaskListState::NotRead => RunToolTaskListState::NotRead,
                        TaskListState::Read => RunToolTaskListState::Read,
                        TaskListState::Stale => RunToolTaskListState::Stale,
                    },
                })
                .collect(),
            problem: set.problem,
        })
    }

    /// `run_status`(블로킹).
    pub fn tool_status(
        &self,
        scope: &ConnectionScope,
        params: &RunToolStatusParams,
    ) -> Result<RunToolStatus, RpcError> {
        let grant = self.tool_grant(scope, &params.project_id)?;
        let runs = match &params.run_id {
            Some(run_id) => vec![self.find(grant.project_id(), run_id)?.info()],
            None => self.list(grant.project_id()),
        };
        Ok(RunToolStatus {
            runs: runs.into_iter().map(tool_run).collect(),
        })
    }

    /// `run_logs`(블로킹 — 정규식 컴파일 · 버퍼 훑기).
    pub fn tool_logs(
        &self,
        scope: &ConnectionScope,
        params: &RunToolLogsParams,
    ) -> Result<RunToolLogs, RpcError> {
        let grant = self.tool_grant(scope, &params.project_id)?;
        let query = Query::new(params)?;
        let run = self.find(grant.project_id(), &params.run_id)?;
        Ok(logs(&run, &query))
    }
}

fn files(layers: &[RunConfigLayer]) -> Vec<RunToolConfigFile> {
    layers
        .iter()
        .map(|l| RunToolConfigFile {
            kind: match l.origin {
                pw_core::config::LayerOrigin::Local => RunToolConfigFileKind::Local,
                _ => RunToolConfigFileKind::Shared,
            },
            path: l.path.clone(),
        })
        .collect()
}

/// env 값을 가린다(구성 · 프로필 · `[debug]` 아래 어느 깊이든 `env` 표) — 이름이 비밀 단어면 `[redacted]`
/// (`shown::is_secret_name` — `GIT_CONFIG_KEY_<n>` 은 아니다), 아니면 값 안의 URL 비밀번호 자리만(`shown::url_password_masked`).
/// 에이전트에게 주는 조회 결과라 승인 화면과 같은 판정을 쓰되 비밀 이름의 값은 해시도 싣지 않는다. 명령 · 인자는 가리지 않는다.
pub fn redacted(mut config: RunConfig) -> RunConfig {
    fn value(name: &str, value: &str) -> String {
        if shown::is_secret_name(name) {
            REDACTED.to_owned()
        } else {
            shown::url_password_masked(value)
        }
    }
    fn mask(env: &mut BTreeMap<String, String>) {
        for (name, v) in env.iter_mut() {
            *v = value(name, v);
        }
    }
    /// `[debug]` 의 어댑터 템플릿 값: 키가 `env` 인 표의 글자 값.
    fn mask_debug(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    if key == "env"
                        && let serde_json::Value::Object(env) = child
                    {
                        for (name, entry) in env.iter_mut() {
                            if let serde_json::Value::String(text) = entry {
                                *text = value(name, text);
                            } else {
                                mask_debug(entry);
                            }
                        }
                    } else {
                        mask_debug(child);
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(mask_debug),
            _ => {}
        }
    }
    mask(&mut config.env);
    for profile in config.profiles.values_mut() {
        mask(&mut profile.env);
    }
    if let Some(debug) = config.debug.as_mut() {
        for (key, child) in debug.settings.iter_mut() {
            if key == "env"
                && let serde_json::Value::Object(env) = child
            {
                for (name, entry) in env.iter_mut() {
                    if let serde_json::Value::String(text) = entry {
                        *text = value(name, text);
                    }
                }
            } else {
                mask_debug(child);
            }
        }
    }
    config
}

fn tool_state(state: RunState) -> RunToolState {
    match state {
        RunState::Running => RunToolState::Running,
        RunState::Stopping => RunToolState::Stopping,
        RunState::Exited => RunToolState::Exited,
    }
}

fn tool_exit(exit: RunExit) -> RunToolExit {
    RunToolExit {
        code: exit.code,
        signal: exit.signal,
    }
}

/// 띄운 자리는 사람 · 에이전트만 — 세션 id · 클라이언트 이름은 싣지 않는다(머리 주석 「띄운 자리」).
fn tool_run(info: RunInfo) -> RunToolRun {
    RunToolRun {
        run_id: info.run_id,
        config: info.config,
        name: info.name,
        kind: info.kind,
        launched_by: match info.launched_by {
            LaunchedBy::Window { .. } => RunToolLauncher::Person,
            LaunchedBy::Agent { .. } => RunToolLauncher::Agent,
        },
        worktree: info.worktree,
        cwd: info.cwd,
        profile: info.profile,
        pid: info.pid,
        state: tool_state(info.state),
        exit: info.exit.map(tool_exit),
        started_at: info.started_at,
        ended_at: info.ended_at,
        restart_of: info.restart_of,
        log_start: info.log_start,
        log_end: info.log_end,
    }
}

// --- 로그 고르기 ----------------------------------------------------------------------------------------------------

/// 검사를 지난 `run_logs` 입력.
pub struct Query {
    tail: Option<usize>,
    since: Option<u64>,
    grep: Option<regex::Regex>,
}

impl Query {
    pub fn new(params: &RunToolLogsParams) -> Result<Self, RpcError> {
        if let Some(tail) = params.tail
            && tail > MAX_TAIL
        {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("`tail` must be at most {MAX_TAIL}"),
            )
            .with_data(json!({ "max": MAX_TAIL })));
        }
        let grep = match &params.grep {
            None => None,
            Some(pattern) if pattern.chars().count() > GREP_MAX_CHARS => {
                return Err(RpcError::new(
                    code::INVALID_PARAMS,
                    format!("`grep` must be at most {GREP_MAX_CHARS} characters"),
                ));
            }
            Some(pattern) => Some(
                regex::RegexBuilder::new(pattern)
                    .size_limit(GREP_SIZE_LIMIT)
                    .build()
                    .map_err(|e| {
                        RpcError::new(
                            code::INVALID_PARAMS,
                            format!("`grep` is not a valid regular expression: {e}"),
                        )
                    })?,
            ),
        };
        Ok(Self {
            tail: params.tail.map(|t| t as usize),
            since: params.since,
            grep,
        })
    }

    /// `since` 가 없을 때의 줄 수.
    fn tail(&self) -> Option<usize> {
        match (self.tail, self.since) {
            (Some(n), _) => Some(n),
            (None, None) => Some(DEFAULT_TAIL as usize),
            (None, Some(_)) => None,
        }
    }
}

/// 버퍼에서 읽은 조각.
struct Raw {
    start: u64,
    end: u64,
    /// `bytes` 의 첫 오프셋.
    from: u64,
    bytes: Vec<u8>,
    /// `since` 앞에서 버려진 바이트.
    dropped: u64,
}

/// 읽을 범위를 줄 색인으로 정하고(버퍼를 처음부터 다시 훑지 않는다 — `tail` 은 [`RunLog::tail_lines`]) 그 범위만 복사한다.
fn raw(log: &RunLog, query: &Query) -> Raw {
    let (start, end) = (log.start(), log.end());
    let dropped = query.since.map_or(0, |s| start.saturating_sub(s));
    let mut lo = query.since.map_or(start, |s| s.clamp(start, end));
    let mut hi = end;
    if query.grep.is_none() {
        match query.tail() {
            Some(n) => lo = lo.max(log.tail_lines(n)),
            None => hi = end.min(lo.saturating_add(SINCE_WINDOW as u64)),
        }
    }
    let slice = log.read(lo, usize::try_from(hi - lo).unwrap_or(usize::MAX));
    Raw {
        start,
        end,
        from: slice.from,
        bytes: slice.bytes,
        dropped,
    }
}

/// 줄 하나: 오프셋과 걷은 글.
struct Line {
    offset: u64,
    text: String,
}

fn logs(run: &Run, query: &Query) -> RunToolLogs {
    let raw = run.with_log(|log| raw(log, query));
    let state = run.state();
    let exit = run.info().exit;
    let to = raw.from + raw.bytes.len() as u64;
    // 줄로 가른다. 창(`SINCE_WINDOW`)이 끝에 못 미쳐 마지막 줄이 창 밖으로 이어지면 그 줄은 다음 호출의 몫이다(그 줄 하나뿐이면
    // 그대로 둔다 — 아래 상한이 자른다).
    let mut pieces: Vec<(u64, &[u8])> = Vec::new();
    let mut offset = raw.from;
    for piece in raw.bytes.split_inclusive(|&b| b == b'\n') {
        pieces.push((offset, piece));
        offset += piece.len() as u64;
    }
    let mut next = to;
    let window_cut = to < raw.end;
    if window_cut
        && pieces.len() > 1
        && pieces.last().is_some_and(|(_, p)| p.last() != Some(&b'\n'))
    {
        next = pieces.pop().map_or(to, |(o, _)| o);
    }
    let lines: Vec<Line> = pieces
        .into_iter()
        .map(|(offset, piece)| Line {
            offset,
            text: text::clean_line(piece.strip_suffix(b"\n").unwrap_or(piece)),
        })
        .filter(|l| query.grep.as_ref().is_none_or(|re| re.is_match(&l.text)))
        .collect();
    let mut truncated = raw.dropped > 0 || window_cut;
    let chosen: Vec<Line> = match query.tail() {
        // 뒤에서부터 상한까지 — 넘으면 앞(오래된 줄)을 던다.
        Some(n) => {
            let skip = lines.len().saturating_sub(n);
            let mut kept: Vec<Line> = Vec::new();
            let mut used = 0;
            for mut line in lines.into_iter().skip(skip).rev() {
                let cost = json_len(&line.text) + 2;
                if used + cost > LOG_BUDGET {
                    truncated = true;
                    if kept.is_empty() {
                        line.text = keep_end(&line.text, LOG_BUDGET.saturating_sub(2));
                        kept.push(line);
                    }
                    break;
                }
                used += cost;
                kept.push(line);
            }
            kept.reverse();
            kept
        }
        // 앞에서부터 상한까지 — 넘으면 뒤를 던고 다음 `since` 는 던 첫 줄.
        None => {
            let mut kept: Vec<Line> = Vec::new();
            let mut used = 0;
            let mut iter = lines.into_iter().peekable();
            while let Some(mut line) = iter.next() {
                let cost = json_len(&line.text) + 2;
                if used + cost > LOG_BUDGET {
                    truncated = true;
                    if kept.is_empty() {
                        // 한 줄이 상한보다 크다 — 그 줄의 앞만 싣고 다음 줄로 넘어간다(같은 자리를 다시 읽지 않게).
                        line.text = keep_start(&line.text, LOG_BUDGET.saturating_sub(2));
                        next = iter.peek().map_or(next, |l| l.offset);
                        kept.push(line);
                    } else {
                        next = line.offset;
                    }
                    break;
                }
                used += cost;
                kept.push(line);
            }
            kept
        }
    };
    let from = chosen.first().map_or(next, |l| l.offset);
    RunToolLogs {
        run_id: run.id().to_owned(),
        state: Some(tool_state(state)),
        exit: exit.map(tool_exit),
        lines: u32::try_from(chosen.len()).unwrap_or(u32::MAX),
        text: chosen
            .into_iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n"),
        from,
        next_since: next,
        log_start: raw.start,
        log_end: raw.end,
        truncated,
        dropped: raw.dropped,
    }
}

/// JSON 문자열로 쓸 때의 바이트 수(따옴표 빼고). 걷은 글에 남는 제어 문자는 탭뿐이다.
fn json_len(text: &str) -> usize {
    text.chars()
        .map(|c| match c {
            '"' | '\\' | '\t' | '\n' | '\r' | '\u{8}' | '\u{c}' => 2,
            c if c.is_control() => 6,
            c => c.len_utf8(),
        })
        .sum()
}

/// 앞에서 `budget` 바이트(JSON 크기) 안.
fn keep_start(text: &str, budget: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|c| {
            used += json_len(c.encode_utf8(&mut [0; 4]));
            used <= budget
        })
        .collect()
}

/// 뒤에서 `budget` 바이트(JSON 크기) 안.
fn keep_end(text: &str, budget: usize) -> String {
    let mut used = 0;
    let mut kept: Vec<char> = text
        .chars()
        .rev()
        .take_while(|c| {
            used += json_len(c.encode_utf8(&mut [0; 4]));
            used <= budget
        })
        .collect();
    kept.reverse();
    kept.into_iter().collect()
}

/// 블로킹 스레드에서 돌린다.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 툴 메서드를 붙인다(`super::methods::register` 가 부른다).
pub fn register(router: &mut Router, runs: Runs) {
    let (list, status, logs) = (runs.clone(), runs.clone(), runs);
    router
        .unary(
            RunToolConfigListMethod,
            move |ctx, params: RunToolConfigListParams| {
                let (runs, scope) = (list.clone(), ctx.scope().clone());
                blocking(move || runs.tool_config_list(&scope, &params))
            },
        )
        .unary(
            RunToolStatusMethod,
            move |ctx, params: RunToolStatusParams| {
                let (runs, scope) = (status.clone(), ctx.scope().clone());
                blocking(move || runs.tool_status(&scope, &params))
            },
        )
        .unary(RunToolLogsMethod, move |ctx, params: RunToolLogsParams| {
            let (runs, scope) = (logs.clone(), ctx.scope().clone());
            blocking(move || runs.tool_logs(&scope, &params))
        });
}
