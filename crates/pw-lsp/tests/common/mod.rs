//! 통합 시험의 바탕: 신뢰한 프로젝트 하나와 그 허가, 가짜 서버를 띄우는 도우미.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_lsp::fake::Scenario;
use pw_lsp::log::{ServerLog, lsp_log_dir};
use pw_lsp::{LanguageServer, LaunchSpec, LspGrant, ServerEvent, ServerOptions, lsp_grant};
use pw_rpc::ConnectionScope;
use pw_runtime::RuntimeEnv;
use serde_json::Value;
use tokio::sync::mpsc;

/// 신뢰한 프로젝트 하나(임시 폴더)와 그 창의 허가.
pub struct Project {
    _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub root: PathBuf,
    pub state: PathBuf,
    pub projects: Registry,
    pub grant: LspGrant,
}

pub fn project() -> Project {
    let tmp = tempfile::tempdir().unwrap();
    let base = simplify(fs::canonicalize(tmp.path()).unwrap());
    let app = AppPaths::under(&base.join("app"));
    let state = app.state.clone();
    let projects = Registry::new(app, None, EventBus::new(DEFAULT_CAPACITY));
    let root = base.join("project");
    fs::create_dir_all(&root).unwrap();
    let id = projects
        .create(NewProject {
            root: root.clone(),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    projects.set_trust(&id, true).unwrap();
    let grant = lsp_grant(&ConnectionScope::Project(id.clone()), &projects, &id, None).unwrap();
    Project {
        _tmp: tmp,
        base,
        root,
        state,
        projects,
        grant,
    }
}

/// 띄운 가짜 서버와 그 주변.
pub struct Fake {
    pub server: LanguageServer,
    pub events: mpsc::UnboundedReceiver<ServerEvent>,
    pub record: PathBuf,
    pub log_dir: PathBuf,
}

impl Project {
    /// `scenario` 를 쓰고(받은 메시지 기록 파일을 붙여) 가짜 서버를 띄운다. `trace` 면 프로토콜 기록도 로그에.
    pub async fn fake(&self, name: &str, mut scenario: Scenario, trace: bool) -> Fake {
        let record = self.base.join(format!("{name}.jsonl"));
        scenario.record = Some(record.clone());
        let path = scenario.write(&self.base, name).unwrap();
        let log_dir = lsp_log_dir(&self.state);
        let log = ServerLog::open(&log_dir, "fake", name, trace).unwrap();
        let (tx, events) = mpsc::unbounded_channel();
        let server = LanguageServer::spawn(
            &self.grant,
            fake_spec(&self.root, &path),
            ServerOptions {
                instance: name.to_owned(),
                log: Some(log),
                events: Some(tx),
                ..ServerOptions::default()
            },
        )
        .await
        .unwrap();
        Fake {
            server,
            events,
            record,
            log_dir,
        }
    }
}

pub fn fake_spec(root: &Path, scenario: &Path) -> LaunchSpec {
    LaunchSpec {
        server_id: "fake".to_owned(),
        program: PathBuf::from(env!("CARGO_BIN_EXE_pw-lsp-fake")),
        args: vec![scenario.as_os_str().to_owned()],
        cwd: root.to_path_buf(),
        env: RuntimeEnv::process(),
        extra_env: BTreeMap::new(),
    }
}

/// 가짜 서버가 받은 메시지들.
pub fn received(record: &Path) -> Vec<Value> {
    fs::read_to_string(record)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// `check` 가 참이 될 때까지(최대 `limit`) 기다린다.
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
    events: &mut mpsc::UnboundedReceiver<ServerEvent>,
    limit: Duration,
    mut pick: impl FnMut(ServerEvent) -> Option<T>,
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

/// 프로세스 그룹 `group` 에 든 pid(`pgrep -g`).
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

/// 그 pid 의 프로세스가 있는가(좀비 제외 — `ps` 의 상태가 `Z` 면 없다).
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}
