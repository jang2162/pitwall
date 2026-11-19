//! 실행 관리자 — Host 범위의 실행 · 로그 · 트리 끝내기(17-m6-plan M6-02).
//!
//! 터미널과 다르게 **실행은 연결과 수명이 갈린다**(메인 판정 M6-P1). 실행 하나는 [`RunId`](Host 안에서 유일한 글)이고, 창을 다시
//! 불러와도 · 에이전트가 띄웠어도 같은 실행을 본다(`run.attach { runId, since }`). 끝내기는 셋이다 — 사람 · 툴의 중지, 프로젝트
//! 창이 닫힐 때([`Runs::stop_project`] — 창이 확인한 뒤, M6-06 ⑥), 앱이 끝날 때(`crate::shutdown` — 실행의 PTY 도
//! `crate::pty` 의 세션 목록에 들어 함께 끝난다).
//!
//! # 띄우기
//!
//! 길은 둘이고 둘 다 허가 값을 받아야 부를 수 있다:
//!
//! - **창**([`Runs::start`]) — `exec_grant` 를 지난 [`ExecGrant`](그 프로젝트 창만)와 구성 id. 구성(M6-01)을 읽고 argv · 작업
//!   폴더 · env 를 이때 푼다. 「사람이 본 구성」 기록(M6-11)이 이 길에 붙는다.
//! - **에이전트**([`Runs::start_agent`]) — 승인을 지난 계획의 [`AgentRunGrant`](M6-10). 구성을 다시 읽지 않고 허가의 [`RunSpec`]
//!   그대로 띄운다. 허가의 작업 폴더는 판정 때 정규화한 경로라, 띄우기 직전에 다시 정규화해 같은지 본다 — 그 사이 경로의 한 구성
//!   요소가 심링크로 바뀌었으면 띄우지 않는다(M6-10 리뷰에서 넘김 ⑦).
//!
//! 두 길 모두 **미신뢰 프로젝트는 띄우지 않는다**(메인 판정 M6-P6 — `PERMISSION_DENIED`, `data.reason = "untrusted"`). 환경은 런타임
//! 환경(03 「런타임 환경 (D26)」) 위에 구성의 env(`crate::env::RunEnv` — `env_file` → `env` → 프로필, M6-03)를 얹고 PTY 로
//! 띄운다(메인 판정 M6-P2). 에이전트 갈래는 허가의 `RunSpec.env`(계획 때 푼 값)를 얹는다.
//!
//! # 실행의 종류
//!
//! 종류는 trait [`RunKind`] 로 가른다 — 프로세스(이 모듈의 `process`)와 `sql`(M6-09) · 디버그(M6-17)가 같은 [`RunId`] · 로그 ·
//! 상태를 쓴다. 종류를 더하는 행은 [`Runs::start_kind`] 로 [`RunOutput`] 을 받아 출력을 쓰고 끝을 알린다.
//!
//! # 로그
//!
//! 실행마다 링 버퍼([`RunLog`] — 메인 판정 M6-P3): 실행 중 [`RunLimits::running`](8 MiB), 끝나면 마지막
//! [`RunLimits::finished`](1 MiB)로 줄여 프로젝트마다 최근 [`RunLimits::finished_per_project`](20)개, 모든 실행을 합쳐
//! [`RunLimits::host_total`](64 MiB) — 넘으면 가장 오래 끝난 실행부터 목록에서 뺀다.
//!
//! # 범위
//!
//! `runId` 를 받는 것은 모두 **그 실행의 프로젝트로 판정한다**. 다른 프로젝트의 `runId` 는 없는 실행과 같은 오류다
//! ([`no_such_run`] — 있다는 것을 알리지 않는다). RPC 의 범위 판정은 [`methods`] 에 있다.

pub mod chain;
mod chain_tool;
pub mod confirm;
pub mod exec_tool;
pub mod log;
pub mod methods;
pub mod ports;
mod process;
pub mod tool;

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use pw_core::bus::{EventBus, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::exec::{AgentRunGrant, ExecGrant, RunSpec};
use pw_core::registry::path::normalize;
use pw_core::registry::run_seen::SeenBy;
use pw_core::registry::{Registry, RegistryError};
use pw_rpc::{RpcError, code};
use pw_runtime::Runtime;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::watch;

use crate::config::argv::{ArgvContext, argv};
use crate::config::{RunConfigPlace, read};
use crate::env::RunEnv;
use crate::pty::Ending;

pub use log::{LogSlice, Meter, RunLog};
pub use methods::RunStartParams;
pub use ports::{RunPort, TOPIC_PORTS};
pub use process::{ProcessLauncher, ProcessSpec, PtyLauncher};

/// 실행 하나의 id. Host 안에서 유일하다(연결마다의 번호가 아니다) — 앞은 Host 를 띄울 때 정한 무작위 글, 뒤는 차례.
pub type RunId = String;

/// 실행이 시작될 때 버스로 내는 이벤트(프로젝트 범위). payload 는 [`RunInfo`].
pub const TOPIC_STARTED: &str = "run.started";
/// 실행이 끝날 때 버스로 내는 이벤트(프로젝트 범위). payload 는 [`RunInfo`](끝 코드 · 신호가 든다).
pub const TOPIC_EXITED: &str = "run.exited";

/// 창이 크기를 주지 않을 때의 PTY 크기. 콘솔이 붙으면 `run.resize` 로 바꾼다.
pub const DEFAULT_SIZE: (u16, u16) = (120, 30);

/// 버퍼 · 끝내기의 상한(메인 판정 M6-P3, M6-02 ④). 시험은 작게 줄여 쓴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    /// 실행 중인 실행 하나의 로그 상한.
    pub running: usize,
    /// 끝난 실행 하나의 로그 상한(끝날 때 이만큼으로 줄인다).
    pub finished: usize,
    /// 프로젝트마다 남기는 끝난 실행 수.
    pub finished_per_project: usize,
    /// 모든 실행의 로그를 합친 상한.
    pub host_total: usize,
    /// 중지의 SIGTERM 뒤 SIGKILL 까지.
    pub stop_grace: Duration,
    /// compose 의 SIGINT 뒤 SIGTERM 까지(compose 의 기본 컨테이너 중지 시간 10초).
    pub interrupt_grace: Duration,
    /// 듣는 포트를 읽는 간격(M6-08 — [`ports`]).
    pub port_interval: Duration,
}

const MIB: usize = 1024 * 1024;

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            running: 8 * MIB,
            finished: MIB,
            finished_per_project: 20,
            host_total: 64 * MIB,
            stop_grace: Duration::from_secs(5),
            interrupt_grace: Duration::from_secs(10),
            port_interval: Duration::from_secs(2),
        }
    }
}

/// 실행을 띄운 자리(M6-12 ② · M6-14 ⑦ 이 읽는다 — 사람이 띄운 실행은 에이전트의 정책 자동이 닿지 않는다).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LaunchedBy {
    /// A person, from the project window (`window` is the window label when known).
    Window { window: Option<String> },
    /// An agent, through a run tool (`session` is the MCP session, `client` the agent's client name when it gave one).
    Agent {
        session: String,
        client: Option<String>,
    },
}

/// 에이전트 갈래([`Runs::start_agent`])가 받는 띄운 자리 — 창 표지를 실을 수 없다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLaunch {
    pub session: String,
    pub client: Option<String>,
}

/// [`Runs::start_agent_with`] 의 나머지.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentStart {
    /// PTY 크기(없으면 [`DEFAULT_SIZE`]).
    pub size: Option<(u16, u16)>,
    /// 실행 목록에 보이는 워크트리(툴 입력의 `worktree`).
    pub worktree: Option<String>,
    /// 다시 실행한 앞 실행.
    pub restart_of: Option<RunId>,
}

/// 지금 푼 프로세스 구성 하나([`Runs::resolve_process`] — 창의 띄우기와 에이전트의 계획이 같이 쓴다).
pub(crate) struct Resolved {
    pub key: String,
    pub root: PathBuf,
    pub config: crate::config::RunConfig,
    pub cwd: PathBuf,
    pub run_env: RunEnv,
    /// 그 작업 폴더의 런타임 환경.
    pub runtime: pw_runtime::RuntimeEnv,
    pub login_shell: Vec<String>,
    /// 띄울 argv(`keep_path_first` 앞 — 해시하는 값).
    pub argv: Vec<String>,
    /// 명령 정의 본문 — 해시에 드는 값(`npm` 의 그 script, `mise-task` 는 감지가 낸 정의 — [`Runs::definition_of`]).
    pub definition: Option<String>,
    /// 첫 실행 확인에 보일 정의 본문(`mise-task` 는 그 태스크를 정의한 파일의 글)과 그 파일.
    pub definition_shown: Option<String>,
    pub definition_source: Option<String>,
    /// 런타임 기준 환경의 `HOME`(런타임 원천 파일 찾기가 멈추는 자리).
    pub home: Option<PathBuf>,
}

impl Resolved {
    /// 「사람이 본 구성」 해시(M6-11 — `crate::seen::hash`). 창의 길(첫 실행 확인 · 기록 · 화면 저장)과 에이전트의 계획이 이 하나로
    /// 해시한다.
    pub fn seen_hash(&self) -> Result<pw_core::registry::run_seen::SeenHash, String> {
        self.seen_content().map(|c| c.hash())
    }

    /// 해시의 재료(`crate::seen::content`).
    pub fn seen_content(&self) -> Result<crate::seen::SeenContent, String> {
        crate::seen::content(&crate::seen::SeenParts {
            key: &self.key,
            root: &self.root,
            config: &self.config,
            argv: &self.argv,
            cwd: &self.cwd,
            home: self.home.as_deref(),
            env: &self.run_env,
            definition: self.definition.as_deref(),
        })
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Running,
    /// 중지를 보냈고 아직 끝나지 않았다.
    Stopping,
    Exited,
}

/// 끝난 꼴.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunExit {
    /// 끝 코드. 신호로 끝났으면 `null`.
    pub code: Option<i32>,
    /// 끝낸 신호의 이름(예: `Terminated: 15`). 코드로 끝났으면 `null`.
    pub signal: Option<String>,
}

/// 실행 하나의 정보(`run.list` · `run.start` · 이벤트 `run.started` · `run.exited`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub run_id: RunId,
    pub project_id: String,
    /// 구성 열쇠(구성 id · 감지된 태스크의 열쇠).
    pub config: String,
    /// 보이는 이름(구성의 `name`).
    pub name: String,
    /// 종류: `process`(M6-02) · `sql`(M6-09) · `debug`(M6-17).
    pub kind: String,
    pub launched_by: LaunchedBy,
    /// 창이 고른 활성 워크트리 꼭대기. 주 체크아웃 · 에이전트가 띄운 실행이면 `null`(에이전트 실행의 작업 폴더는 `cwd`).
    pub worktree: Option<String>,
    /// 작업 폴더(정규화한 절대 경로) — 콘솔의 상대 경로 링크 기준(M6-06 ②).
    pub cwd: Option<String>,
    pub profile: Option<String>,
    pub pid: Option<u32>,
    pub state: RunState,
    pub exit: Option<RunExit>,
    /// 시작 시각(Unix 밀리초).
    #[specta(type = pw_rpc::JsNumber)]
    pub started_at: i64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub ended_at: Option<i64>,
    /// 「다시 실행」 으로 이 실행이 이어받은 앞 실행.
    pub restart_of: Option<RunId>,
    /// 남은 로그의 첫 오프셋(그 앞은 버려졌다).
    #[specta(type = pw_rpc::JsNumber)]
    pub log_start: u64,
    /// 로그의 끝 오프셋(쓴 바이트 수 전체).
    #[specta(type = pw_rpc::JsNumber)]
    pub log_end: u64,
    /// 프로세스 트리가 듣는 TCP 포트(포트 순 — M6-08, [`ports`]). 끝난 실행 · 프로세스가 아닌 종류는 빈 목록.
    pub ports: Vec<RunPort>,
}

/// 실행의 종류(머리 주석 「실행의 종류」). 출력과 끝은 [`RunOutput`] 으로 알린다.
pub trait RunKind: Send + Sync + 'static {
    /// [`RunInfo::kind`].
    fn kind(&self) -> &'static str;

    fn pid(&self) -> Option<u32> {
        None
    }

    /// 입력을 보낸다. 받지 않는 종류 · 끝난 실행이면 까닭.
    fn input(&self, _data: Vec<u8>) -> Result<(), String> {
        Err("this run takes no input".to_owned())
    }

    fn resize(&self, _cols: u16, _rows: u16) -> Result<(), String> {
        Ok(())
    }

    /// 끝내기를 시작한다. 기다리지 않는다. 다 끝나면(프로세스면 그 트리까지) [`RunOutput::exited`] 를 부른다. 두 번 불려도 된다.
    fn stop(&self);
}

/// 다시 실행할 때 무엇을 다시 띄우는가.
#[derive(Clone)]
pub enum Relaunch {
    /// 구성을 다시 읽어 창의 길로(구성 · `.env` 가 바뀌었으면 바뀐 것).
    Config {
        config_id: String,
        profile: Option<String>,
        worktree: Option<String>,
    },
    /// 에이전트 허가의 실행 내용 그대로(승인을 지난 값). 작업 폴더는 다시 본다.
    Spec(Box<RunSpec>),
    /// 다시 실행할 수 없다.
    None,
}

/// [`Runs::start_kind`] 가 받는 새 실행의 정보.
pub struct NewRun {
    pub project_id: String,
    pub config: String,
    pub name: String,
    pub launched_by: LaunchedBy,
    pub worktree: Option<String>,
    pub cwd: Option<PathBuf>,
    pub profile: Option<String>,
    pub relaunch: Relaunch,
    pub size: (u16, u16),
    pub restart_of: Option<RunId>,
}

/// 실행 하나. [`Runs`] 의 표와 그것을 읽는 쪽(`run.attach` 스트림)이 함께 쥔다.
pub struct Run {
    id: RunId,
    project_id: String,
    config: String,
    name: String,
    launched_by: LaunchedBy,
    worktree: Option<String>,
    cwd: Option<PathBuf>,
    profile: Option<String>,
    started_at: i64,
    restart_of: Option<RunId>,
    /// 사람이 띄웠다 — 띄운 자리가 창이거나, 다시 실행한 앞 실행이 사람이 띄운 것(`restartOf` 를 따라 처음까지). 앞 실행을 표에서
    /// 찾지 못하면 사람이 띄운 것으로 본다(엄한 쪽 — [`Run::launched_by_person`]).
    by_person: bool,
    relaunch: Relaunch,
    size: Mutex<(u16, u16)>,
    log: Mutex<RunLog>,
    status: Mutex<Status>,
    control: OnceLock<Arc<dyn RunKind>>,
    /// 출력 · 상태가 바뀔 때마다 오른다(`run.attach` 가 기다린다).
    changed: watch::Sender<u64>,
    /// 낸 이벤트. `run.exited` 가 `run.started` 보다 먼저 나가지 않게 둘을 이 잠금 안에서 낸다(곧바로 끝나는 프로그램).
    announced: Mutex<Announced>,
    /// 듣는 포트(폴링이 쓴다 — [`ports`]). `status` 다음에 잠근다.
    ports: Mutex<Vec<RunPort>>,
    /// 앞선 폴링에서 찾은 프로세스 트리. 중지(`process`)도 함께 쥐어 그 후손까지 끝낸다.
    known: Arc<Mutex<ports::Known>>,
    /// 이 실행을 띄운 사슬(`before` · 복합 구성 — [`chain`]). 사슬이 아직 띄우는 중이면 이 실행의 중지가 사슬 전체를 멈춘다.
    chain: Mutex<Option<Arc<chain::ChainCtl>>>,
}

#[derive(Default)]
struct Announced {
    started: bool,
    exited: bool,
}

struct Status {
    state: RunState,
    exit: Option<RunExit>,
    ended_at: Option<i64>,
}

impl Run {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn launched_by(&self) -> &LaunchedBy {
        &self.launched_by
    }

    /// 사람이 띄운 실행인가(17-m6-plan M6-14 ⑦ · 결정 M6-D7 · D69): 창에서 띄웠거나, `restartOf` 를 따라간 어느 앞 실행이든 창에서
    /// 띄운 것. 에이전트가 사람의 실행을 `run_restart` 로 다시 띄운 새 실행도 사람의 실행이다 — 그렇지 않으면 그 뒤의 `run_stop` 이
    /// 사람의 실행에 닿는 정책 자동을 받는다(M6-02 리뷰 R6). 앞 실행이 표에서 이미 빠졌으면 사람이 띄운 것으로 본다.
    pub fn launched_by_person(&self) -> bool {
        self.by_person
    }

    /// 실행의 구성 열쇠.
    pub fn config(&self) -> &str {
        &self.config
    }

    /// 실행이 든 워크트리(창이 준 것 · 에이전트가 준 것).
    pub fn worktree(&self) -> Option<&str> {
        self.worktree.as_deref()
    }

    /// 실행의 프로필.
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    pub fn info(&self) -> RunInfo {
        let status = lock(&self.status);
        let log = lock(&self.log);
        RunInfo {
            run_id: self.id.clone(),
            project_id: self.project_id.clone(),
            config: self.config.clone(),
            name: self.name.clone(),
            kind: self
                .control
                .get()
                .map_or(process::KIND, |k| k.kind())
                .to_owned(),
            launched_by: self.launched_by.clone(),
            worktree: self.worktree.clone(),
            cwd: self.cwd.as_ref().map(|p| p.display().to_string()),
            profile: self.profile.clone(),
            pid: self.control.get().and_then(|k| k.pid()),
            state: status.state,
            exit: status.exit.clone(),
            started_at: self.started_at,
            ended_at: status.ended_at,
            restart_of: self.restart_of.clone(),
            log_start: log.start(),
            log_end: log.end(),
            ports: lock(&self.ports).clone(),
        }
    }

    /// 폴링이 읽은 포트를 둔다. 끝난 실행에는 두지 않는다. 바뀌었으면 참.
    fn set_ports(&self, ports: Vec<RunPort>) -> bool {
        let status = lock(&self.status);
        if status.state == RunState::Exited {
            return false;
        }
        let mut now = lock(&self.ports);
        if *now == ports {
            return false;
        }
        *now = ports;
        drop(now);
        drop(status);
        self.bump();
        true
    }

    pub fn state(&self) -> RunState {
        lock(&self.status).state
    }

    /// 로그를 읽는다(M6-12 의 `run_logs` — tail · since · grep 은 [`RunLog`] 의 줄 색인으로).
    pub fn with_log<T>(&self, f: impl FnOnce(&RunLog) -> T) -> T {
        f(&lock(&self.log))
    }

    /// 끝날 때까지 기다린다.
    pub async fn exited(&self) -> RunExit {
        let mut changed = self.changed.subscribe();
        loop {
            if let Some(exit) = lock(&self.status).exit.clone() {
                return exit;
            }
            if changed.changed().await.is_err() {
                return RunExit::default();
            }
        }
    }

    /// 출력 · 상태가 바뀌기를 기다리는 수신자.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn bump(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }
}

/// 종류가 출력을 쓰고 끝을 알리는 손잡이([`RunKind`] 머리 주석).
#[derive(Clone)]
pub struct RunOutput {
    run: Arc<Run>,
    runs: Weak<Inner>,
}

impl RunOutput {
    /// 출력을 로그에 붙인다(상한을 넘으면 앞을 버린다).
    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        lock(&self.run.log).append(bytes);
        self.run.bump();
        if let Some(inner) = self.runs.upgrade()
            && inner.meter.bytes() > inner.limits.host_total
        {
            inner.evict_over_total(&mut lock(&inner.table));
        }
    }

    /// 포트 폴링이 찾은 이 실행의 프로세스 트리(M6-08). 프로세스 종류의 끝내기가 그 후손까지 끝내는 데 쓴다.
    pub(crate) fn known_tree(&self) -> Arc<Mutex<ports::Known>> {
        Arc::clone(&self.run.known)
    }

    /// 띄우는 동안 중지가 왔는가(`run.stop` · 창 닫기 — 종류는 프로그램을 풀지 않고 끝낼 수 있다).
    pub fn stop_requested(&self) -> bool {
        self.run.state() == RunState::Stopping
    }

    /// 끝났다. 처음 한 번만 듣는다.
    pub fn exited(&self, exit: RunExit) {
        {
            let mut status = lock(&self.run.status);
            if status.exit.is_some() {
                return;
            }
            status.state = RunState::Exited;
            status.exit = Some(exit);
            status.ended_at = Some(now_ms());
            lock(&self.run.ports).clear();
        }
        tracing::info!(target: "pw_run", run = %self.run.id, exit = ?lock(&self.run.status).exit, "run exited");
        if let Some(inner) = self.runs.upgrade() {
            inner.finished(&self.run);
        }
        self.run.bump();
    }
}

/// Host 를 만든 뒤에 붙이는 것.
#[derive(Clone)]
pub struct HostParts {
    /// 설정 경로(개인 전용 프로젝트의 실행 구성 폴더).
    pub paths: AppPaths,
    pub projects: Registry,
    /// `run.started` · `run.exited` 를 낸다.
    pub bus: EventBus,
}

/// 한 Host 의 실행들. 복제해도 같은 관리자다.
#[derive(Clone)]
pub struct Runs {
    inner: Arc<Inner>,
}

struct Inner {
    runtime: Arc<Runtime>,
    launcher: Arc<dyn ProcessLauncher>,
    host: OnceLock<HostParts>,
    /// `sql` 구성의 에이전트 실행(M6-14 — pw-db 가 `pw_db::run_sql::register` 에서 붙인다). 없으면 `run_start` 가 `sql` 구성을 거절한다.
    sql_tool: OnceLock<Arc<dyn exec_tool::SqlRunTool>>,
    /// 창의 사슬의 `sql` 단계(M6-21 — pw-db 가 붙인다). 없으면 창이 `sql` 단계가 든 사슬을 거절한다.
    sql_window: OnceLock<Arc<dyn chain::SqlWindowChain>>,
    limits: RunLimits,
    meter: Meter,
    table: Mutex<Table>,
    nonce: String,
    next: AtomicU64,
    poller: ports::Poller,
}

#[derive(Default)]
struct Table {
    runs: BTreeMap<RunId, Arc<Run>>,
    /// 끝난 실행(가장 오래 끝난 것부터).
    finished: VecDeque<RunId>,
}

impl Runs {
    /// PTY 로 띄우는 관리자.
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self::with(runtime, Arc::new(PtyLauncher), RunLimits::default())
    }

    /// 띄우는 것 · 상한을 정해 만든다(시험은 가짜 실행기 · 작은 상한).
    pub fn with(
        runtime: Arc<Runtime>,
        launcher: Arc<dyn ProcessLauncher>,
        limits: RunLimits,
    ) -> Self {
        let runs = Self {
            inner: Arc::new(Inner {
                runtime,
                launcher,
                host: OnceLock::new(),
                sql_tool: OnceLock::new(),
                sql_window: OnceLock::new(),
                limits,
                meter: Meter::default(),
                table: Mutex::default(),
                nonce: nonce(),
                next: AtomicU64::new(1),
                poller: ports::Poller::default(),
            }),
        };
        let mut managers = lock(&MANAGERS);
        managers.retain(|m| m.strong_count() > 0);
        managers.push(Arc::downgrade(&runs.inner));
        drop(managers);
        runs
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.inner.host.set(parts);
    }

    /// `sql` 구성을 에이전트의 `run_start` · `run_restart` 로 돌리는 것(M6-14 — pw-run 은 pw-db 에 의존하지 않으므로 pw-db 가 붙인다).
    /// 두 번째부터는 무시한다.
    pub fn attach_sql_tool(&self, tool: Arc<dyn exec_tool::SqlRunTool>) {
        let _ = self.inner.sql_tool.set(tool);
    }

    pub fn limits(&self) -> RunLimits {
        self.inner.limits
    }

    /// 살아 있는 포트 폴링 스레드 수 — 끝나지 않은 실행이 없으면 0(M6-08 ③).
    pub fn port_pollers(&self) -> usize {
        self.inner.poller.threads()
    }

    /// 모든 실행의 로그 바이트 합.
    pub fn log_bytes(&self) -> usize {
        self.inner.meter.bytes()
    }

    /// 창의 길: 구성 `configId` 를 읽어 띄운다(머리 주석 「띄우기」). 블로킹한다(구성 · 레지스트리 읽기, 로그인 셸 기다리기).
    /// 미신뢰 프로젝트는 구성을 읽기 전에 거절한다. `seen` 이 있으면 띄운 뒤 그 값으로 「사람이 본 구성」 을 기록한다(M6-11 —
    /// [`Self::record_seen`]).
    pub fn start(
        &self,
        grant: &ExecGrant,
        seen: Option<SeenBy>,
        window: Option<String>,
        params: &RunStartParams,
    ) -> Result<RunInfo, RpcError> {
        let size = size_of(params.cols, params.rows)?;
        // 사슬(`before` · 복합 구성)은 이 길로 띄우지 않는다 — `before` 를 건너뛰게 된다. `run.start` 는 [`Self::start_window`].
        if self
            .chain_of(
                grant.project_id(),
                params.worktree.as_deref(),
                &params.config_id,
            )?
            .is_some()
        {
            return Err(RpcError::invalid_params(format!(
                "run config `{}` starts other configurations first (`before` or a compound configuration); start it with run.start",
                params.config_id
            )));
        }
        self.start_config(
            grant.project_id(),
            LaunchedBy::Window { window },
            Relaunched {
                config_id: params.config_id.clone(),
                profile: params.profile.clone(),
                worktree: params.worktree.clone(),
                size,
                restart_of: None,
                seen,
                confirmed: params.confirmed.clone(),
            },
        )
    }

    fn start_config(
        &self,
        project_id: &str,
        launched_by: LaunchedBy,
        what: Relaunched,
    ) -> Result<RunInfo, RpcError> {
        let ready = self.prepare_config(project_id, &what, what.seen.is_some())?;
        let new = NewRun {
            project_id: ready.project_id,
            config: ready.key.clone(),
            name: ready.name,
            launched_by,
            worktree: what.worktree.clone(),
            cwd: Some(ready.cwd),
            profile: what.profile.clone(),
            relaunch: Relaunch::Config {
                config_id: what.config_id,
                profile: what.profile,
                worktree: what.worktree,
            },
            size: what.size,
            restart_of: what.restart_of,
        };
        let info = self.start_process(new, ready.argv, ready.env)?;
        if let Some(by) = what.seen {
            match ready.seen_hash {
                Ok(hash) => self.record_seen(&by, &ready.key, hash),
                Err(why) => {
                    tracing::debug!(target: "pw_run", key = %ready.key, %why, "the run started but its seen hash could not be made");
                }
            }
        }
        Ok(info)
    }

    /// 창의 길의 풀기: 에이전트의 계획과 같은 [`Self::resolve_process`] 로 풀고(「사람이 본 구성」 해시가 하나다 — M6-11), `check` 면 첫
    /// 실행 확인(M6-07 ⑥ — `confirm` 머리 주석)을 지난다. 띄우지 않는다.
    fn prepare_config(
        &self,
        project_id: &str,
        what: &Relaunched,
        check: bool,
    ) -> Result<Prepared, RpcError> {
        let host = self.inner.host()?;
        let (project, place) = self.config_place(project_id, what.worktree.as_deref())?;
        let (key, config) = config_of(&place, &what.config_id)?;
        let resolved = self.resolve_process(
            &project.id,
            &place.root,
            &key,
            &config,
            what.profile.as_deref(),
        )?;
        // 런타임 환경 · 로그인 셸 환경을 읽는 동안 작업 폴더가 사라졌거나 바뀌었으면 묻기 전에 거절한다 — 띄우기 앞의 재검사
        // (`start_process`)와 같은 까닭이다.
        let cwd = &resolved.cwd;
        if !(cwd.is_dir() && normalize(cwd).ok().as_deref() == Some(cwd.as_path())) {
            return Err(cwd_changed(
                "the run's working directory changed while its environment loaded".to_owned(),
            ));
        }
        let content = resolved.seen_content();
        // 편집 화면이 보이는 것(구성 파일 하나)이 해시의 전부인가 — `.env` · 정의 본문 · 런타임 원천 파일이 들면 아니다(리뷰 F2).
        let config_only = matches!(&content, Ok(c) if c.env_files.is_empty() && c.definition.is_none() && c.runtime_files.is_empty());
        let seen_hash = content.map(|c| c.hash());
        let launching = confirm::Launching {
            resolved: &resolved,
        };
        let (token, saw) = match &seen_hash {
            Ok(hash) => (
                hash.to_string(),
                host.projects.human_saw(&project.id, &key, hash),
            ),
            Err(_) => (launching.once_token(), false),
        };
        let shown = launching.shown(seen_hash.is_ok());
        if check && !saw && what.confirmed.as_deref() != Some(token.as_str()) {
            return Err(confirm::confirm_required(token, shown));
        }
        let mut env = crate::shell::child_env(&resolved.run_env.apply(&resolved.runtime));
        let mut argv = resolved.argv.clone();
        keep_path_first(&mut argv, &mut env, &resolved.login_shell);
        Ok(Prepared {
            project_id: project.id.clone(),
            key,
            name: config.name.clone(),
            cwd: resolved.cwd.clone(),
            argv,
            env,
            seen_hash,
            config_only,
            token,
            saw,
            shown,
        })
    }

    /// 구성 편집 화면이 저장한 구성의 「사람이 본 구성」 해시(17-m6-plan M6-07 ④ — `SeenPlace::Save`): 창에서 그 구성을 **프로필
    /// 없이** 띄울 때와 같은 풀기 · 같은 해시다(기록은 열쇠마다 하나). 띄우지도 묻지도 않는다. 미신뢰 · 쓸 수 없는 구성 · 해시를 낼
    /// 수 없는 구성은 그 까닭(부른 쪽은 기록하지 않는다).
    ///
    /// **편집 화면이 보인 것이 해시의 전부일 때만** 해시를 준다(리뷰 F2 — 결정 D68 「사람이 본 내용」): 해시에 `.env` 파일 · 정의
    /// 본문(npm script · mise task) · 런타임 원천 파일이 들면 그 내용은 편집 화면에 없으므로 기록하지 않는다 — 첫 실행의 확인 화면
    /// (그 내용을 모두 보이는 곳)이 기록한다.
    pub fn saved_seen_hash(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        config_id: &str,
    ) -> Result<pw_core::registry::run_seen::SeenHash, String> {
        let what = Relaunched {
            config_id: config_id.to_owned(),
            profile: None,
            worktree: worktree.map(str::to_owned),
            size: DEFAULT_SIZE,
            restart_of: None,
            seen: None,
            confirmed: None,
        };
        let ready = self
            .prepare_config(project_id, &what, false)
            .map_err(|e| e.message)?;
        if !ready.config_only {
            return Err(
                "the configuration draws on content the editor does not show (.env files, a command definition or runtime files)"
                    .to_owned(),
            );
        }
        ready.seen_hash
    }

    /// 프로젝트(레지스트리에 있고 신뢰한 것)와 그 워크트리의 구성 자리. 미신뢰 프로젝트는 구성을 읽기 전에 거절한다(M6-P6).
    fn config_place(
        &self,
        project_id: &str,
        worktree: Option<&str>,
    ) -> Result<(pw_core::registry::Project, RunConfigPlace), RpcError> {
        let host = self.inner.host()?;
        let project = host.projects.get(project_id).map_err(registry_error)?;
        if !host.projects.is_trusted(&project.id) {
            return Err(untrusted(&project.id));
        }
        let place = RunConfigPlace::new(&host.paths, &project, worktree)?;
        Ok((project, place))
    }

    /// 프로세스 구성 하나를 지금 푼다(블로킹): 작업 폴더 · M6-03 의 env(`.env` 를 지금 읽는다) · 런타임 환경 · argv · 정의 본문.
    /// 창의 길([`Self::start_config`])과 에이전트의 계획(`exec_tool`)이 같은 함수로 풀어야 「사람이 본 구성」 해시가 같다(M6-11).
    /// 신뢰는 부르는 쪽이 이미 봤다(`.env` 는 그 뒤에 읽는다 — M6-03 머리 주석). 오류 글에는 값이 없다(이름 · 경로 · 줄만).
    fn resolve_process(
        &self,
        project_id: &str,
        root: &Path,
        key: &str,
        config: &crate::config::RunConfig,
        profile: Option<&str>,
    ) -> Result<Resolved, RpcError> {
        let cwd = match &config.cwd {
            Some(rel) => crate::config::root::inside(root, rel)
                .map_err(|why| RpcError::invalid_params(format!("cwd {why}")))?,
            None => normalize(root).unwrap_or_else(|_| root.to_path_buf()),
        };
        if !cwd.is_dir() {
            return Err(RpcError::invalid_params(format!(
                "cwd `{}` is not a directory",
                cwd.display()
            )));
        }
        let run_env = RunEnv::resolve(config, profile, root)
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        let runtime = self.inner.runtime.env(project_id, &cwd);
        let login_shell = crate::shell::default_argv(&runtime.env);
        let argv = argv(
            config,
            profile,
            &ArgvContext {
                root,
                cwd: &cwd,
                login_shell: &login_shell,
            },
        )
        .map_err(RpcError::invalid_params)?;
        // 정의 본문(npm script · mise task)은 띄우기 전에 읽는다 — 띄운 값으로 해시한다(`seen` 머리 주석 「다시 읽지 않는다」, 리뷰 R7 ·
        // M6-07 리뷰 F1).
        let defined = self.definition_of(project_id, key, config, root, &cwd);
        let home = self
            .inner
            .runtime
            .login_shell_env()
            .0
            .env
            .get("HOME")
            .map(PathBuf::from);
        Ok(Resolved {
            key: key.to_owned(),
            root: root.to_path_buf(),
            config: config.clone(),
            cwd,
            run_env,
            runtime: runtime.env.clone(),
            login_shell,
            argv,
            definition: defined.as_ref().map(|d| d.hashed.clone()),
            definition_shown: defined.as_ref().map(|d| d.shown.clone()),
            definition_source: defined.and_then(|d| d.source),
            home,
        })
    }

    /// 명령 정의 본문(「사람이 본 구성」 해시와 첫 실행 확인 — M6-11 · M6-07 ⑥, M6-07 리뷰 F1): `npm` 은 `package.json` 의 그
    /// script, `mise-task` 는 M6-05 의 감지가 낸 정의(그 태스크를 정의한 파일 전체의 sha256 과 이름 — `detect::mise`)이고 확인
    /// 화면에는 그 파일의 글을 보인다. 읽지 못하면 `None`(해시를 못 내 「본 적 없음」 — 묻는다).
    fn definition_of(
        &self,
        project_id: &str,
        key: &str,
        config: &crate::config::RunConfig,
        root: &Path,
        cwd: &Path,
    ) -> Option<confirm::Defined> {
        match &config.kind {
            crate::config::RunKindConfig::Npm { .. } => {
                npm_definition(key, config, root, cwd).map(|body| confirm::Defined {
                    hashed: body.clone(),
                    shown: body,
                    source: None,
                })
            }
            crate::config::RunKindConfig::MiseTask { task } => {
                let host = self.inner.host().ok()?;
                let projects = host.projects.clone();
                let runtime = Arc::clone(&self.inner.runtime);
                let exec = crate::detect::exec::TrustedExec::new(
                    project_id,
                    Arc::new(move |id| projects.is_trusted(id)),
                    Arc::new(move |project, dir| runtime.env(project, dir).env.clone()),
                    crate::detect::exec::AUTOMATIC,
                );
                let only: [&dyn crate::detect::TaskDetector; 1] =
                    [&crate::detect::mise::MiseDetector];
                let detection = crate::detect::detect_with(&only, root, &exec);
                let found = detection.get(&crate::detect::key(crate::detect::mise::ID, task))?;
                Some(confirm::Defined {
                    hashed: found.definition.clone(),
                    shown: confirm::source_text(&found.source),
                    source: Some(found.source.display().to_string()),
                })
            }
            _ => None,
        }
    }

    /// 창에서 띄운 구성을 「사람이 본 구성」 으로 기록한다(17-m6-plan M6-02 ⑧ · M6-11): 띄운 값 그대로(argv · 작업 폴더 · M6-03 이 푼
    /// env · 정의 본문)를 `pw_run::seen::hash` 로 해시한 것([`Resolved::seen_hash`])을 `Registry::record_run_seen` 에 적는다. 해시를
    /// 만들지 못하거나(정의 본문이 없는 `mise-task` — M6-05 전) 기록하지 못하면(`readOnly` · `memory`) 건너뛴다 — 실행을 막지 않고,
    /// 다음 에이전트 실행이 한 번 묻는다.
    fn record_seen(&self, by: &SeenBy, key: &str, hash: pw_core::registry::run_seen::SeenHash) {
        let Ok(host) = self.inner.host() else {
            return;
        };
        if let Err(e) = host.projects.record_run_seen(by, &[(key.to_owned(), hash)]) {
            tracing::debug!(target: "pw_run", key, error = %e, "the run started but was not recorded as seen");
        }
    }

    /// 에이전트의 길(M6-14): 승인을 지난 계획의 `step` 번째 [`RunSpec`] 을 그대로 띄운다(머리 주석 「띄우기」). 구성을 다시 읽지
    /// 않는다. 블로킹한다(로그인 셸 기다리기).
    pub fn start_agent(
        &self,
        grant: &AgentRunGrant,
        step: usize,
        by: AgentLaunch,
        size: Option<(u16, u16)>,
    ) -> Result<RunInfo, RpcError> {
        self.start_agent_with(
            grant,
            step,
            by,
            AgentStart {
                size,
                ..AgentStart::default()
            },
        )
    }

    /// [`Self::start_agent`] 에 워크트리(실행 목록에 보이는 칸)와 다시 실행한 앞 실행(`restartOf` — M6-14 의 `run_restart`)을 준다.
    /// 앞 실행을 사람이 띄웠으면 새 실행도 사람이 띄운 실행으로 다룬다([`Run::launched_by_person`] — 결정 M6-D7).
    pub fn start_agent_with(
        &self,
        grant: &AgentRunGrant,
        step: usize,
        by: AgentLaunch,
        start: AgentStart,
    ) -> Result<RunInfo, RpcError> {
        let spec = grant.runs().get(step).ok_or_else(|| {
            RpcError::internal(format!("the approved plan has no run step {step}"))
        })?;
        self.start_spec(
            grant.project_id(),
            spec,
            LaunchedBy::Agent {
                session: by.session,
                client: by.client,
            },
            start.size.unwrap_or(DEFAULT_SIZE),
            start.restart_of,
            start.worktree,
        )
    }

    fn start_spec(
        &self,
        project_id: &str,
        spec: &RunSpec,
        launched_by: LaunchedBy,
        size: (u16, u16),
        restart_of: Option<RunId>,
        worktree: Option<String>,
    ) -> Result<RunInfo, RpcError> {
        let host = self.inner.host()?;
        if !host.projects.is_trusted(project_id) {
            return Err(untrusted(project_id));
        }
        let cwd = granted_cwd(&spec.cwd)?;
        let runtime = self.inner.runtime.env(project_id, &cwd);
        // 승인된 값 그대로 얹는다 — `.env` 를 다시 읽지 않는다(M6-10).
        let mut env = crate::shell::child_env(&crate::env::apply(&runtime.env, &spec.env));
        let mut argv = spec.argv.clone();
        keep_path_first(
            &mut argv,
            &mut env,
            &crate::shell::default_argv(&runtime.env),
        );
        let new = NewRun {
            project_id: project_id.to_owned(),
            config: spec.config.clone(),
            name: spec.config.clone(),
            launched_by,
            worktree,
            cwd: Some(cwd),
            profile: spec.profile.clone(),
            relaunch: Relaunch::Spec(Box::new(spec.clone())),
            size,
            restart_of,
        };
        self.start_process(new, argv, env)
    }

    fn start_process(
        &self,
        new: NewRun,
        argv: Vec<String>,
        env: Vec<(OsString, OsString)>,
    ) -> Result<RunInfo, RpcError> {
        let Some(cwd) = new.cwd.clone() else {
            return Err(RpcError::internal(
                "a process run needs a working directory",
            ));
        };
        let spec = ProcessSpec {
            ending: self.ending_for(&argv),
            argv,
            cwd,
            env,
            cols: new.size.0,
            rows: new.size.1,
        };
        let launcher = Arc::clone(&self.inner.launcher);
        self.start_kind(new, move |output| {
            launcher.launch(&spec, output).inspect_err(|e| {
                tracing::warn!(target: "pw_run", argc = spec.argv.len(), cwd = %spec.cwd.display(), error = %e.message, "cannot start the run");
            })
        })
    }

    /// compose 는 SIGINT 를 먼저(M6-01), 나머지는 SIGTERM.
    fn ending_for(&self, argv: &[String]) -> Ending {
        let limits = self.inner.limits;
        let program = argv
            .first()
            .and_then(|p| Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let compose = program == "docker-compose"
            || (program == "docker" && argv.get(1).is_some_and(|a| a == "compose"));
        if compose {
            Ending::InterruptThenTerminate {
                interrupt: limits.interrupt_grace,
                grace: limits.stop_grace,
            }
        } else {
            Ending::Terminate {
                grace: limits.stop_grace,
            }
        }
    }

    /// 종류 하나를 실행으로 띄운다(머리 주석 「실행의 종류」). `start` 는 [`RunOutput`] 을 받아 출력을 쓰고 끝을 알리는 종류를
    /// 돌려준다 — 실패하면 실행은 표에 남지 않는다. **허가 판정은 부르는 쪽의 몫이다**: 이 함수는 판정을 하지 않으므로 창 · 에이전트
    /// 판정(`exec_grant` · `agent_run_grant` · `pw_db::gate`)을 지난 길에서만 부른다.
    pub fn start_kind(
        &self,
        new: NewRun,
        start: impl FnOnce(RunOutput) -> Result<Arc<dyn RunKind>, RpcError>,
    ) -> Result<RunInfo, RpcError> {
        let id = format!(
            "{}-{}",
            self.inner.nonce,
            self.inner.next.fetch_add(1, Ordering::Relaxed)
        );
        let by_person = matches!(new.launched_by, LaunchedBy::Window { .. })
            || new.restart_of.as_ref().is_some_and(|before| {
                lock(&self.inner.table)
                    .runs
                    .get(before)
                    .is_none_or(|run| run.by_person)
            });
        let run = Arc::new(Run {
            id: id.clone(),
            project_id: new.project_id,
            config: new.config,
            name: new.name,
            launched_by: new.launched_by,
            worktree: new.worktree,
            cwd: new.cwd,
            profile: new.profile,
            started_at: now_ms(),
            restart_of: new.restart_of,
            by_person,
            relaunch: new.relaunch,
            size: Mutex::new(new.size),
            log: Mutex::new(RunLog::new(
                self.inner.limits.running,
                self.inner.meter.clone(),
            )),
            status: Mutex::new(Status {
                state: RunState::Running,
                exit: None,
                ended_at: None,
            }),
            control: OnceLock::new(),
            changed: watch::channel(0).0,
            announced: Mutex::default(),
            ports: Mutex::default(),
            known: Arc::default(),
            chain: Mutex::default(),
        });
        // 띄우는 동안 나온 출력 · 곧바로 끝난 실행도 받도록 표에 먼저 넣는다.
        lock(&self.inner.table)
            .runs
            .insert(id.clone(), Arc::clone(&run));
        let output = RunOutput {
            run: Arc::clone(&run),
            runs: Arc::downgrade(&self.inner),
        };
        let kind = match start(output) {
            Ok(kind) => kind,
            Err(e) => {
                let mut table = lock(&self.inner.table);
                table.runs.remove(&id);
                table.finished.retain(|f| *f != id);
                return Err(e);
            }
        };
        let _ = run.control.set(Arc::clone(&kind));
        // 띄우는 동안 온 중지(`run.stop` · 창 닫기)는 종류가 없어 상태만 바꿨다 — 여기서 보낸다. 그 사이 `stop_run` 이 종류를 보고
        // 이미 보냈을 수도 있다(두 번 불려도 된다 — `RunKind::stop`).
        if run.state() == RunState::Stopping {
            kind.stop();
        }
        if kind.pid().is_some() && run.state() != RunState::Exited {
            ports::ensure_polling(&self.inner);
        }
        let info = run.info();
        tracing::info!(target: "pw_run", run = %id, project = %info.project_id, config = %info.config, pid = ?info.pid, launched_by = ?info.launched_by, "run started");
        {
            let mut announced = lock(&run.announced);
            self.inner.publish(&info.project_id, TOPIC_STARTED, &info);
            announced.started = true;
            if run.state() == RunState::Exited && !announced.exited {
                announced.exited = true;
                self.inner
                    .publish(&run.project_id, TOPIC_EXITED, &run.info());
            }
        }
        Ok(info)
    }

    /// 그 프로젝트의 실행. 없는 실행 · 다른 프로젝트의 실행은 같은 오류([`no_such_run`])다.
    pub fn find(&self, project_id: &str, run_id: &str) -> Result<Arc<Run>, RpcError> {
        lock(&self.inner.table)
            .runs
            .get(run_id)
            .filter(|run| run.project_id == project_id)
            .cloned()
            .ok_or_else(|| no_such_run(run_id))
    }

    /// 그 프로젝트의 실행들(실행 중 · 남은 끝난 실행, 시작 순).
    pub fn list(&self, project_id: &str) -> Vec<RunInfo> {
        let runs: Vec<Arc<Run>> = lock(&self.inner.table)
            .runs
            .values()
            .filter(|run| run.project_id == project_id)
            .cloned()
            .collect();
        let mut infos: Vec<RunInfo> = runs.iter().map(|run| run.info()).collect();
        infos.sort_by(|a, b| (a.started_at, &a.run_id).cmp(&(b.started_at, &b.run_id)));
        infos
    }

    /// 중지를 시작한다(기다리지 않는다 — 끝나면 `run.exited`). 이미 끝난 실행이면 아무것도 하지 않는다.
    pub fn stop(&self, project_id: &str, run_id: &str) -> Result<RunInfo, RpcError> {
        let run = self.find(project_id, run_id)?;
        stop_run(&run);
        Ok(run.info())
    }

    /// 중지하고 끝나기(프로세스 트리까지)를 기다린다. M6-14 의 `run_restart` 가 이 뒤에 [`Self::start_agent`] 를 부른다.
    pub async fn stop_and_wait(&self, project_id: &str, run_id: &str) -> Result<RunExit, RpcError> {
        let run = self.find(project_id, run_id)?;
        stop_run(&run);
        Ok(run.exited().await)
    }

    /// 프로젝트의 실행을 모두 중지한다 — 프로젝트 창이 닫힐 때(창이 확인한 뒤, M6-06 ⑥). 기다리지 않는다.
    pub fn stop_project(&self, project_id: &str) {
        let runs: Vec<Arc<Run>> = lock(&self.inner.table)
            .runs
            .values()
            .filter(|run| run.project_id == project_id && run.state() != RunState::Exited)
            .cloned()
            .collect();
        if !runs.is_empty() {
            tracing::info!(target: "pw_run", project = project_id, count = runs.len(), "stopping the project's runs");
        }
        for run in runs {
            stop_run(&run);
        }
    }

    /// 창의 「다시 실행」: 끝난 것을 본 뒤(프로세스 트리까지) 같은 것을 새 실행으로 띄운다. 구성으로 띄운 실행은 구성을 다시
    /// 읽고, 에이전트 허가로 띄운 실행은 그 실행 내용을 다시 띄운다(작업 폴더는 다시 본다). 새 실행은 `restartOf` 로 앞 실행을
    /// 가리킨다. `grant` 는 그 실행의 프로젝트로 받은 창 허가다. 확인 열쇠 없이 부른다([`Self::restart_confirmed`]).
    pub async fn restart(
        &self,
        grant: &ExecGrant,
        seen: Option<SeenBy>,
        window: Option<String>,
        run_id: &str,
    ) -> Result<RunInfo, RpcError> {
        self.restart_confirmed(grant, seen, window, run_id, None)
            .await
    }

    /// [`Self::restart`] 에 첫 실행 확인의 열쇠(`run.restart { confirmed }` — M6-07 ⑥)를 더한 것. 구성으로 띄운 실행을 창의 증거로
    /// 다시 띄울 때는 **앞 실행을 멈추기 전에** 다시 읽은 구성이 확인을 지나는지 본다 — 확인이 필요하면 앞 실행은 그대로 돈다.
    pub async fn restart_confirmed(
        &self,
        grant: &ExecGrant,
        seen: Option<SeenBy>,
        window: Option<String>,
        run_id: &str,
        confirmed: Option<&str>,
    ) -> Result<RunInfo, RpcError> {
        let run = self.find(grant.project_id(), run_id)?;
        if seen.is_some()
            && let Relaunch::Config {
                config_id,
                profile,
                worktree,
            } = &run.relaunch
        {
            let this = self.clone();
            let project = run.project_id.clone();
            let what = Relaunched {
                config_id: config_id.clone(),
                profile: profile.clone(),
                worktree: worktree.clone(),
                size: DEFAULT_SIZE,
                restart_of: None,
                seen: None,
                confirmed: confirmed.map(str::to_owned),
            };
            tokio::task::spawn_blocking(move || {
                this.prepare_config(&project, &what, true).map(|_| ())
            })
            .await
            .map_err(|e| RpcError::internal(e.to_string()))??;
        }
        let confirmed = confirmed.map(str::to_owned);
        if matches!(run.relaunch, Relaunch::None) {
            return Err(RpcError::invalid_params(format!(
                "run `{run_id}` cannot be restarted"
            )));
        }
        stop_run(&run);
        run.exited().await;
        let size = *lock(&run.size);
        let this = self.clone();
        let launched_by = LaunchedBy::Window { window };
        tokio::task::spawn_blocking(move || match &run.relaunch {
            Relaunch::Config {
                config_id,
                profile,
                worktree,
            } => this.start_config(
                &run.project_id,
                launched_by,
                Relaunched {
                    config_id: config_id.clone(),
                    profile: profile.clone(),
                    worktree: worktree.clone(),
                    size,
                    restart_of: Some(run.id.clone()),
                    seen,
                    confirmed,
                },
            ),
            Relaunch::Spec(spec) => this.start_spec(
                &run.project_id,
                spec,
                launched_by,
                size,
                Some(run.id.clone()),
                run.worktree.clone(),
            ),
            Relaunch::None => unreachable!("checked above"),
        })
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
    }

    pub fn input(&self, project_id: &str, run_id: &str, data: Vec<u8>) -> Result<(), RpcError> {
        let run = self.find(project_id, run_id)?;
        let not_running = || RpcError::invalid_params(format!("run `{run_id}` is not running"));
        if run.state() == RunState::Exited {
            return Err(not_running());
        }
        run.control
            .get()
            .ok_or_else(not_running)?
            .input(data)
            .map_err(RpcError::invalid_params)
    }

    pub fn resize(
        &self,
        project_id: &str,
        run_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), RpcError> {
        let size = size_of(Some(cols), Some(rows))?;
        let run = self.find(project_id, run_id)?;
        *lock(&run.size) = size;
        if run.state() == RunState::Exited {
            return Ok(());
        }
        match run.control.get() {
            Some(kind) => kind.resize(cols, rows).map_err(RpcError::internal),
            None => Ok(()),
        }
    }
}

/// 이 프로세스의 실행 관리자들([`project_closed`] 가 찾는다). 관리자가 버려지면 다음 등록 때 빠진다.
static MANAGERS: Mutex<Vec<Weak<Inner>>> = Mutex::new(Vec::new());

/// 프로젝트 창이 닫혔다(창이 확인한 뒤 — M6-06 ⑥): 이 프로세스의 모든 실행 관리자에서 그 프로젝트의 실행을 중지한다. 기다리지
/// 않는다. 데스크톱 앱이 창이 사라질 때(`WindowEvent::Destroyed`, label `project-<id>`) 부른다 — 창을 다시 불러오기(`⌘R`)는 창을
/// 없애지 않으므로 실행이 그대로다. `crate::shutdown` 과 같이 Host 를 거치지 않는 자리다(창 이벤트는 Host 연결 밖에서 온다).
pub fn project_closed(project_id: &str) {
    let managers: Vec<Arc<Inner>> = lock(&MANAGERS).iter().filter_map(Weak::upgrade).collect();
    for inner in managers {
        Runs { inner }.stop_project(project_id);
    }
}

/// 구성 자리에서 쓸 수 있는 구성 하나(열쇠, 구성). 쓸 수 없으면 그 까닭(invalid params), 없으면 not found.
fn config_of(
    place: &RunConfigPlace,
    config_id: &str,
) -> Result<(String, crate::config::RunConfig), RpcError> {
    let set = read(place);
    let Some(loaded) = set.get(config_id) else {
        if let Some(unusable) = set.unusable.iter().find(|u| u.id == config_id) {
            return Err(RpcError::invalid_params(format!(
                "run config `{}` cannot be used: {}",
                unusable.id, unusable.reason
            )));
        }
        return Err(RpcError::new(
            code::NOT_FOUND,
            format!("run config `{config_id}` does not exist"),
        ));
    };
    Ok((loaded.id.clone(), loaded.config.clone()))
}

/// [`Runs::start_config`] 가 받는 것(창의 시작 · 다시 실행).
struct Relaunched {
    config_id: String,
    profile: Option<String>,
    worktree: Option<String>,
    size: (u16, u16),
    restart_of: Option<RunId>,
    /// 사람이 본 구성으로 기록할 증거(창의 연결). 에이전트 허가로 다시 띄우는 길에는 없다. 있으면 첫 실행 확인을 지난다.
    seen: Option<SeenBy>,
    /// 첫 실행 확인의 열쇠(`confirm::RunConfirmRequired::hash`).
    confirmed: Option<String>,
}

/// [`Runs::prepare_config`] 가 푼 것.
struct Prepared {
    project_id: String,
    key: String,
    name: String,
    cwd: PathBuf,
    argv: Vec<String>,
    env: Vec<(OsString, OsString)>,
    /// 「사람이 본 구성」 해시(또는 낼 수 없는 까닭).
    seen_hash: Result<pw_core::registry::run_seen::SeenHash, String>,
    /// 해시의 재료가 구성 파일 하나뿐인가(`.env` · 정의 본문 · 런타임 원천 파일이 없다) — 화면 저장이 기록해도 되는가(리뷰 F2).
    config_only: bool,
    /// 첫 실행 확인의 열쇠(해시 · 해시를 낼 수 없으면 `confirm::Launching::once_token`).
    token: String,
    /// 그 해시가 「사람이 본 구성」 인가.
    saw: bool,
    /// 첫 실행 확인에 보일 것.
    shown: confirm::RunShown,
}

impl Inner {
    fn host(&self) -> Result<&HostParts, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("runs are not attached to a host"))
    }

    fn publish(&self, project_id: &str, topic: &str, info: &impl Serialize) {
        let Some(host) = self.host.get() else {
            return;
        };
        if let Err(e) = host
            .bus
            .publish(Scope::Project(project_id.to_owned()), topic, info)
        {
            tracing::warn!(target: "pw_run", %e, topic, "cannot publish a run event");
        }
    }

    /// 끝난 실행을 줄이고 남길 수를 지킨다(머리 주석 「로그」).
    fn finished(&self, run: &Arc<Run>) {
        lock(&run.log).shrink(self.limits.finished);
        {
            let mut table = lock(&self.table);
            // 띄우기가 실패해 표에서 빠진 실행은 남기지 않는다.
            if table.runs.contains_key(&run.id) {
                table.finished.push_back(run.id.clone());
                let of_project: Vec<RunId> = table
                    .finished
                    .iter()
                    .filter(|id| {
                        table
                            .runs
                            .get(*id)
                            .is_some_and(|r| r.project_id == run.project_id)
                    })
                    .cloned()
                    .collect();
                let over = of_project
                    .len()
                    .saturating_sub(self.limits.finished_per_project);
                for id in &of_project[..over] {
                    drop_run(&mut table, id);
                }
            }
            self.evict_over_total(&mut table);
        }
        let mut announced = lock(&run.announced);
        if announced.started && !announced.exited {
            announced.exited = true;
            self.publish(&run.project_id, TOPIC_EXITED, &run.info());
        }
    }

    /// Host 총량을 넘으면 가장 오래 끝난 실행부터 뺀다. 실행 중인 실행은 빼지 않는다(각자의 상한이 있다).
    fn evict_over_total(&self, table: &mut Table) {
        while self.meter.bytes() > self.limits.host_total {
            let Some(id) = table.finished.front().cloned() else {
                return;
            };
            drop_run(table, &id);
        }
    }
}

/// 표에서 빼고 로그를 비운다(읽는 스트림이 쥐고 있어도 메모리를 돌려준다).
fn drop_run(table: &mut Table, id: &str) {
    table.finished.retain(|f| f != id);
    if let Some(run) = table.runs.remove(id) {
        lock(&run.log).clear();
        run.bump();
        tracing::debug!(target: "pw_run", run = id, "dropped a finished run");
    }
}

fn stop_run(run: &Run) {
    {
        let mut status = lock(&run.status);
        if status.state != RunState::Running {
            return;
        }
        status.state = RunState::Stopping;
    }
    tracing::info!(target: "pw_run", run = %run.id, "stopping the run");
    if let Some(kind) = run.control.get() {
        kind.stop();
    }
    run.bump();
    // 띄우는 중인 사슬의 실행이면 사슬 전체를 멈춘다(`chain` 머리 주석 「중지」).
    let chain = lock(&run.chain).clone();
    if let Some(chain) = chain {
        chain.stop();
    }
}

/// 에이전트 허가의 작업 폴더를 먼저 본다 — 런타임 환경(로그인 셸 · mise)을 기다리기 전에 일찍 거절하는 자리다. 지금 정규화한
/// 경로가 허가의 경로(판정 때 정규화한 것)와 같고 폴더여야 한다. 띄우기 바로 앞과 띄운 뒤의 재검사는 `process` 가 한다(그 머리 주석
/// 「작업 폴더」) — 여기를 지난 뒤 바뀐 것도 그쪽에서 거절한다.
fn granted_cwd(cwd: &Path) -> Result<PathBuf, RpcError> {
    let now = normalize(cwd).ok();
    if now.as_deref() == Some(cwd) && cwd.is_dir() {
        return Ok(cwd.to_path_buf());
    }
    Err(cwd_changed(
        "the run's working directory changed since it was approved".to_owned(),
    ))
}

/// 작업 폴더가 판정 뒤에 바뀌었다(`PERMISSION_DENIED`, `data.reason = "cwd_changed"`) — 아무것도 돌지 않았다.
pub fn cwd_changed(message: String) -> RpcError {
    RpcError::new(code::PERMISSION_DENIED, message).with_data(json!({ "reason": "cwd_changed" }))
}

/// 로그인 셸이 시작 파일 뒤에 실행의 PATH 를 다시 앞에 두게 하는 env 이름(값은 argv 가 아니라 env 로 — `ps` 에 원문이 남지 않게).
pub const PATH_FRONT_VAR: &str = "PITWALL_RUN_PATH";

/// `shell` 종류(`<로그인 셸> -c <명령>` — `config::argv`)는 로그인 셸의 시작 파일(macOS `/etc/profile` 의 `path_helper`, Debian 의
/// `/etc/profile`)이 PATH 를 다시 세워 런타임 제공자 · `.env` 가 앞에 둔 폴더를 뒤로 밀거나 지운다. 명령 앞에 「실행의 PATH 를 다시
/// 맨 앞에, 시작 파일이 더한 것은 그 뒤에」 를 붙인다 — 사람이 `.env` · `mise.toml` 로 고른 도구가 이긴다(리뷰 F1). 그 env 이름은
/// 명령 전에 지운다. 로그인 셸 `-c` 꼴이 아닌 argv(셸을 거치지 않는 종류)는 그대로 둔다 — 그 순서는 이미 그대로다.
fn keep_path_first(
    argv: &mut [String],
    env: &mut Vec<(OsString, OsString)>,
    login_shell: &[String],
) {
    if cfg!(not(unix)) || login_shell.is_empty() {
        return;
    }
    let shaped = argv.len() == login_shell.len() + 2
        && argv.starts_with(login_shell)
        && argv[login_shell.len()] == "-c";
    let Some(path) = env
        .iter()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.clone())
    else {
        return;
    };
    if !shaped {
        return;
    }
    let command = argv.last_mut().expect("shaped argv has a command");
    *command = format!(
        "PATH=\"${PATH_FRONT_VAR}${{PATH:+:$PATH}}\"; export PATH; unset {PATH_FRONT_VAR}\n{command}"
    );
    env.retain(|(name, _)| name != PATH_FRONT_VAR);
    env.push((PATH_FRONT_VAR.into(), path));
}

/// npm 종류의 정의 본문(`package.json` 의 그 script — `seen` 의 해시 재료). 읽지 못하면 `None`(해시가 「본 적 없음」 이 된다).
fn npm_definition(
    key: &str,
    config: &crate::config::RunConfig,
    root: &Path,
    cwd: &Path,
) -> Option<String> {
    let crate::config::RunKindConfig::Npm { script } = &config.kind else {
        return None;
    };
    crate::detect::npm::script_definition(root, cwd, script)
        .inspect_err(|why| tracing::debug!(target: "pw_run", key, %why, "cannot read the npm script definition"))
        .ok()
        .flatten()
}

fn size_of(cols: Option<u16>, rows: Option<u16>) -> Result<(u16, u16), RpcError> {
    let size = (
        cols.unwrap_or(DEFAULT_SIZE.0),
        rows.unwrap_or(DEFAULT_SIZE.1),
    );
    if size.0 == 0 || size.1 == 0 {
        return Err(RpcError::invalid_params(format!(
            "run console size must be positive, got {}x{}",
            size.0, size.1
        )));
    }
    Ok(size)
}

/// 없는 실행 — 다른 프로젝트의 실행도 이것이다(머리 주석 「범위」).
pub fn no_such_run(run_id: &str) -> RpcError {
    RpcError::new(code::NOT_FOUND, format!("run `{run_id}` does not exist"))
}

/// 미신뢰 프로젝트(메인 판정 M6-P6 — `pw_db::trust::untrusted` · `pw_lsp::grant::untrusted` 와 같은 꼴).
pub fn untrusted(project_id: &str) -> RpcError {
    RpcError::new(
        code::PERMISSION_DENIED,
        "trust the project before running its configurations",
    )
    .with_data(json!({ "reason": "untrusted", "projectId": project_id }))
}

fn registry_error(e: RegistryError) -> RpcError {
    match e {
        RegistryError::NotFound(id) => {
            RpcError::new(code::NOT_FOUND, format!("project `{id}` is not registered"))
        }
        RegistryError::Unavailable(_) => {
            RpcError::new(code::STORAGE_UNAVAILABLE, "storage unavailable")
                .with_data(serde_json::Value::String(e.to_string()))
        }
        other => RpcError::internal(other.to_string()),
    }
}

/// Host 를 띄울 때마다 다른 앞글(실행 id 가 앱을 다시 띄운 뒤의 것과 겹치지 않게).
fn nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    #[allow(clippy::cast_possible_truncation)] // 아래 32비트만 쓴다.
    let mixed = (nanos as u64) ^ u64::from(std::process::id()).rotate_left(16);
    format!("{:08x}", mixed & 0xffff_ffff)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(all(test, unix))]
mod tests;
