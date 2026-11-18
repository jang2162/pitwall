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
    /// 앞선 폴링에서 찾은 프로세스 트리.
    known: Mutex<ports::Known>,
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
            },
        )
    }

    fn start_config(
        &self,
        project_id: &str,
        launched_by: LaunchedBy,
        what: Relaunched,
    ) -> Result<RunInfo, RpcError> {
        let host = self.inner.host()?;
        let project = host.projects.get(project_id).map_err(registry_error)?;
        if !host.projects.is_trusted(&project.id) {
            return Err(untrusted(&project.id));
        }
        let place = RunConfigPlace::new(&host.paths, &project, what.worktree.as_deref())?;
        let set = read(&place);
        let Some(loaded) = set.get(&what.config_id) else {
            if let Some(unusable) = set.unusable.iter().find(|u| u.id == what.config_id) {
                return Err(RpcError::invalid_params(format!(
                    "run config `{}` cannot be used: {}",
                    unusable.id, unusable.reason
                )));
            }
            return Err(RpcError::new(
                code::NOT_FOUND,
                format!("run config `{}` does not exist", what.config_id),
            ));
        };
        let config = &loaded.config;
        let cwd = match &config.cwd {
            Some(rel) => crate::config::root::inside(&place.root, rel)
                .map_err(|why| RpcError::invalid_params(format!("cwd {why}")))?,
            None => normalize(&place.root).unwrap_or_else(|_| place.root.clone()),
        };
        if !cwd.is_dir() {
            return Err(RpcError::invalid_params(format!(
                "cwd `{}` is not a directory",
                cwd.display()
            )));
        }
        // 신뢰는 위에서 봤다 — `.env` 는 그 뒤에 읽는다(M6-03 머리 주석). 오류 글에는 값이 없다(이름 · 경로 · 줄만).
        let run_env = RunEnv::resolve(config, what.profile.as_deref(), &place.root)
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        let runtime = self.inner.runtime.env(&project.id, &cwd);
        let login_shell = crate::shell::default_argv(&runtime.env);
        let argv = argv(
            config,
            what.profile.as_deref(),
            &ArgvContext {
                root: &place.root,
                cwd: &cwd,
                login_shell: &login_shell,
            },
        )
        .map_err(RpcError::invalid_params)?;
        let mut env = crate::shell::child_env(&run_env.apply(&runtime.env));
        let argv_seen = argv.clone();
        let mut argv = argv;
        keep_path_first(&mut argv, &mut env, &login_shell);
        // 정의 본문(npm script)은 띄우기 전에 읽는다 — 띄운 값으로 해시한다(`seen` 머리 주석 「다시 읽지 않는다」, 리뷰 R7).
        let definition = what
            .seen
            .as_ref()
            .and_then(|_| npm_definition(&loaded.id, config, &place.root, &cwd));
        let new = NewRun {
            project_id: project.id.clone(),
            config: loaded.id.clone(),
            name: config.name.clone(),
            launched_by,
            worktree: what.worktree.clone(),
            cwd: Some(cwd.clone()),
            profile: what.profile.clone(),
            relaunch: Relaunch::Config {
                config_id: what.config_id,
                profile: what.profile,
                worktree: what.worktree,
            },
            size: what.size,
            restart_of: what.restart_of,
        };
        let info = self.start_process(new, argv, env)?;
        if let Some(by) = what.seen {
            self.record_seen(
                &by,
                &loaded.id,
                &place.root,
                config,
                &argv_seen,
                &cwd,
                &run_env,
                definition.as_deref(),
            );
        }
        Ok(info)
    }

    /// 창에서 띄운 구성을 「사람이 본 구성」 으로 기록한다(17-m6-plan M6-02 ⑧ · M6-11): 띄운 값 그대로(argv · 작업 폴더 · M6-03 이 푼
    /// env · 정의 본문)를 `pw_run::seen::hash` 로 해시해 `Registry::record_run_seen` 에 적는다. 해시를 만들지 못하거나(정의 본문이
    /// 없는 `mise-task` — M6-05 전) 기록하지 못하면(`readOnly` · `memory`) 건너뛴다 — 실행을 막지 않고, 다음 에이전트 실행이 한 번
    /// 묻는다.
    #[allow(clippy::too_many_arguments)] // 해시의 재료 그대로다.
    fn record_seen(
        &self,
        by: &SeenBy,
        key: &str,
        root: &Path,
        config: &crate::config::RunConfig,
        argv: &[String],
        cwd: &Path,
        env: &RunEnv,
        definition: Option<&str>,
    ) {
        let Ok(host) = self.inner.host() else {
            return;
        };
        let home = self
            .inner
            .runtime
            .login_shell_env()
            .0
            .env
            .get("HOME")
            .map(PathBuf::from);
        let parts = crate::seen::SeenParts {
            key,
            root,
            config,
            argv,
            cwd,
            home: home.as_deref(),
            env,
            definition,
        };
        match crate::seen::hash(&parts) {
            Ok(hash) => {
                if let Err(e) = host.projects.record_run_seen(by, &[(key.to_owned(), hash)]) {
                    tracing::debug!(target: "pw_run", key, error = %e, "the run started but was not recorded as seen");
                }
            }
            Err(why) => {
                tracing::debug!(target: "pw_run", key, %why, "the run started but its seen hash could not be made");
            }
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
            size.unwrap_or(DEFAULT_SIZE),
            None,
        )
    }

    fn start_spec(
        &self,
        project_id: &str,
        spec: &RunSpec,
        launched_by: LaunchedBy,
        size: (u16, u16),
        restart_of: Option<RunId>,
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
            worktree: None,
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
            known: Mutex::default(),
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
    /// 가리킨다. `grant` 는 그 실행의 프로젝트로 받은 창 허가다.
    pub async fn restart(
        &self,
        grant: &ExecGrant,
        seen: Option<SeenBy>,
        window: Option<String>,
        run_id: &str,
    ) -> Result<RunInfo, RpcError> {
        let run = self.find(grant.project_id(), run_id)?;
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
                },
            ),
            Relaunch::Spec(spec) => this.start_spec(
                &run.project_id,
                spec,
                launched_by,
                size,
                Some(run.id.clone()),
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

/// [`Runs::start_config`] 가 받는 것(창의 시작 · 다시 실행).
struct Relaunched {
    config_id: String,
    profile: Option<String>,
    worktree: Option<String>,
    size: (u16, u16),
    restart_of: Option<RunId>,
    /// 사람이 본 구성으로 기록할 증거(창의 연결). 에이전트 허가로 다시 띄우는 길에는 없다.
    seen: Option<SeenBy>,
}

impl Inner {
    fn host(&self) -> Result<&HostParts, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("runs are not attached to a host"))
    }

    fn publish(&self, project_id: &str, topic: &str, info: &RunInfo) {
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
