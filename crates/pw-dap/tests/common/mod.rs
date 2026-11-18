//! 통합 시험의 바탕: 프로젝트 루트 하나와 그 허가, 가짜 어댑터를 띄우는 도우미, 실행 관리자 흉내.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::exec::ExecGrant;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_dap::fake::Scenario;
use pw_dap::protocol::StartMode;
use pw_dap::terminal::TerminalFuture;
use pw_dap::{
    AdapterSpec, Configured, DebugGrant, DebugSession, SessionEvent, SessionOptions, StartError,
    StartRequest, TerminalRun, TerminalStarted, Terminals, Timeouts, Transport, debug_grant,
};
use pw_rpc::ConnectionScope;
use pw_runtime::RuntimeEnv;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

pub const SOON: Duration = Duration::from_secs(5);

pub struct Project {
    _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub root: PathBuf,
    pub record: PathBuf,
    pub projects: Registry,
    pub id: String,
}

/// 신뢰한 프로젝트 하나(임시 폴더 — 실제 경로).
pub fn project() -> Project {
    let tmp = tempfile::tempdir().unwrap();
    let base = simplify(std::fs::canonicalize(tmp.path()).unwrap());
    let root = base.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let record = base.join("record.jsonl");
    let projects = Registry::new(
        AppPaths::under(&base.join("app")),
        None,
        EventBus::new(DEFAULT_CAPACITY),
    );
    let id = projects
        .create(NewProject {
            root: root.clone(),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    projects.set_trust(&id, true).unwrap();
    Project {
        _tmp: tmp,
        base,
        root,
        record,
        projects,
        id,
    }
}

impl Project {
    /// 그 프로젝트 창의 허가.
    pub fn grant(&self) -> DebugGrant {
        debug_grant(
            &ConnectionScope::Project(self.id.clone()),
            &self.projects,
            &self.id,
            None,
        )
        .unwrap()
    }

    /// 시나리오를 쓰고(기록 파일을 이 프로젝트의 것으로) 그 어댑터 정의.
    pub fn adapter(&self, mut scenario: Scenario, transport: Transport) -> AdapterSpec {
        scenario.record = Some(self.record.clone());
        let path = scenario.write(&self.base, "scenario").unwrap();
        AdapterSpec {
            adapter_id: "fake".to_owned(),
            program: PathBuf::from(env!("CARGO_BIN_EXE_pw-dap-fake")),
            args: vec![path.into_os_string()],
            transport,
            cwd: self.root.clone(),
            env: RuntimeEnv::process(),
            extra_env: BTreeMap::new(),
        }
    }

    /// 가짜 어댑터가 남긴 줄들.
    pub fn records(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// 클라이언트가 보낸 메시지들(차례대로, 모든 연결).
    pub fn received(&self) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter_map(|r| r.get("msg").cloned())
            .collect()
    }

    /// 받은 요청의 명령 이름(차례대로).
    pub fn commands(&self) -> Vec<String> {
        self.received()
            .iter()
            .filter(|m| m["type"] == "request")
            .filter_map(|m| m["command"].as_str().map(str::to_owned))
            .collect()
    }

    /// 역요청 `command` 에 대한 클라이언트의 응답들.
    pub fn responses_to(&self, command: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|m| m["type"] == "response" && m["command"] == command)
            .collect()
    }
}

pub fn socket() -> Transport {
    Transport::Socket {
        socket_args: vec!["--socket".into(), "{socket}".into()],
    }
}

pub fn tcp() -> Transport {
    Transport::Tcp {
        port_args: vec![
            "--port".into(),
            "{port}".into(),
            "--host".into(),
            "{host}".into(),
        ],
    }
}

/// 실행 관리자 흉내: 받은 것을 적고 pid 를 지어 답한다.
#[derive(Default)]
pub struct RecordingTerminals {
    pub runs: Mutex<Vec<(String, TerminalRun)>>,
}

impl Terminals for RecordingTerminals {
    fn run_in_terminal<'a>(&'a self, grant: &'a ExecGrant, run: TerminalRun) -> TerminalFuture<'a> {
        self.runs
            .lock()
            .unwrap()
            .push((grant.project_id().to_owned(), run));
        Box::pin(async {
            Ok(TerminalStarted {
                process_id: Some(4242),
                shell_process_id: None,
            })
        })
    }
}

pub struct Started {
    pub debug: DebugSession,
    pub configured: Configured,
    pub events: mpsc::Receiver<SessionEvent>,
    pub terminals: Arc<RecordingTerminals>,
}

pub fn launch(arguments: Value) -> StartRequest {
    StartRequest {
        mode: StartMode::Launch,
        arguments: match arguments {
            Value::Object(map) => map,
            _ => Map::new(),
        },
        breakpoints: Default::default(),
    }
}

pub fn short_timeouts() -> Timeouts {
    Timeouts {
        request: Duration::from_secs(5),
        initialize: Duration::from_secs(5),
        initialized: Duration::from_secs(5),
        launch: Duration::from_secs(5),
        connect: Duration::from_secs(5),
        disconnect: Duration::from_secs(1),
        exit_grace: Duration::from_millis(500),
    }
}

pub async fn try_start(
    grant: DebugGrant,
    adapter: AdapterSpec,
    request: StartRequest,
) -> (
    Result<(DebugSession, Configured), StartError>,
    mpsc::Receiver<SessionEvent>,
    Arc<RecordingTerminals>,
) {
    try_start_with(grant, adapter, request, short_timeouts()).await
}

pub async fn try_start_with(
    grant: DebugGrant,
    adapter: AdapterSpec,
    request: StartRequest,
    timeouts: Timeouts,
) -> (
    Result<(DebugSession, Configured), StartError>,
    mpsc::Receiver<SessionEvent>,
    Arc<RecordingTerminals>,
) {
    let (tx, rx) = mpsc::channel(256);
    let terminals = Arc::new(RecordingTerminals::default());
    let options = SessionOptions {
        events: tx,
        terminals: terminals.clone(),
        locale: Some("ko".to_owned()),
        timeouts,
    };
    (
        DebugSession::start(grant, adapter, request, options).await,
        rx,
        terminals,
    )
}

pub async fn start(
    p: &Project,
    scenario: Scenario,
    transport: Transport,
    request: StartRequest,
) -> Started {
    let (started, events, terminals) =
        try_start(p.grant(), p.adapter(scenario, transport), request).await;
    let (debug, configured) = started.unwrap_or_else(|e| panic!("start failed: {e}"));
    Started {
        debug,
        configured,
        events,
        terminals,
    }
}

pub async fn eventually(limit: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 다음 이벤트 중 `pick` 이 고르는 것(최대 `limit`).
pub async fn next_event<T>(
    events: &mut mpsc::Receiver<SessionEvent>,
    limit: Duration,
    mut pick: impl FnMut(SessionEvent) -> Option<T>,
) -> Option<T> {
    tokio::time::timeout(limit, async {
        while let Some(event) = events.recv().await {
            if let Some(found) = pick(event) {
                return Some(found);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// 그 프로세스 그룹에 남은 pid.
#[cfg(unix)]
pub fn group_members(group: u32) -> Vec<u32> {
    let out = std::process::Command::new("pgrep")
        .args(["-g", &group.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

pub fn path_str(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

/// 그 pid 의 프로세스를 끝낸다(시험이 띄우게 한 남는 자식 정리).
#[cfg(unix)]
pub fn kill(pid: u64) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}
