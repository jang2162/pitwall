//! 실행 파일로 확인하는 넘겨 붙기(16-m5-plan M5-20, 결정 M5-D3). 앱보다 먼저 띄운 `pitwall mcp` 가 앱이 뜬 뒤 앱으로 넘겨 붙는다.
//! GUI 를 띄우지 않는다 — 이 시험 프로세스가 앱 자리에서 소켓을 연 시험 Host 다(`tests/tool.rs` 와 같은 꼴).
//!
//! - 헤드리스로 시작한 세션의 `tools/list` 는 코어 툴만이고, 안내문은 다시 시작하라고 하지 않는다.
//! - 시험 Host 가 소켓을 연 뒤의 다음 요청에서 `tools/list_changed` 가 오고 `tools/list` 가 앱의 목록이다.
//! - 그 뒤 호출의 감사 행은 앱 세션의 것이다(앱의 세션 등록부에 있는 id, 클라이언트는 처음 보낸 `initialize` 의 이름).
//! - 앱이 닫히면 프록시와 같은 꼴로 끝난다: 기다리던 요청에 `-32001`, stderr 에 이유, 종료 코드 1.
//! - 앱 찾기(소켓 확인)는 넘기기 전의 `tools/list` · `tools/call` 요청 하나에 한 번이다 — 로그의 줄 수로 본다.
//! - 헤드리스에서 끝난 호출의 감사 행은 넘겨 붙은 뒤 프로세스가 끝나도 남는다.
//!
//! 넘기는 도중에 헤드리스에서 돌던 호출은 바이너리로 결정적으로 만들 수 없어(헤드리스의 툴은 곧바로 끝난다)
//! `src/mcp/handover_tests.rs` 가 같은 함수(`mcp::handover::serve_headless`)로 본다.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use pw_core::{EventBus, HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::sessions::Sessions;
use pw_mcp::{Catalog, HostKind, Services, socket};
use pw_rpc::{RpcError, rpc_method};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// 앱과 같은 단일 인스턴스 키(main.rs 의 `single_instance_key`)를 계산한다 — 소켓 이름이 그 키에서 나온다.
#[allow(dead_code)] // 키 계산에 쓰는 것만 부른다.
#[path = "../src/instance.rs"]
mod instance;

/// `mcp::handover::CHECK_LOG` 와 같은 문장.
const CHECK_LOG: &str = "mcp handover: looking for the Pitwall app";
/// 헤드리스가 내는 코어 툴. `project_register`(write)는 목록에 있고 부르면 `headless_write` 로 거절된다(16-m5-plan M5-16).
const CORE_TOOLS: [&str; 4] = [
    "project_info",
    "project_list",
    "project_register",
    "settings_get",
];

rpc_method! {
    /// 시험용: 앱에만 있고 오래 돈다.
    pub HandoverSlow: "handovertest.slow" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "project", name: "handovertest_slow" },
    }
}

fn test_instance() -> String {
    format!("handover{}", std::process::id())
}

fn real(path: &Path) -> PathBuf {
    pw_core::registry::path::simplify(std::fs::canonicalize(path).unwrap())
}

/// 프로젝트 둘(`alpha` · `beta`)을 등록한 임시 `PITWALL_*_DIR`.
fn workspace() -> (tempfile::TempDir, PathBuf, AppPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = real(tmp.path());
    let paths = AppPaths::under(&dir);
    let registry = Registry::new(paths.clone(), None, EventBus::new(16));
    for name in ["alpha", "beta"] {
        let root = dir.join("work").join(name);
        std::fs::create_dir_all(&root).unwrap();
        registry
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap();
    }
    drop(registry);
    (tmp, dir, paths)
}

/// main.rs 의 `single_instance_key` 와 같은 계산.
fn instance_key(paths: &AppPaths) -> String {
    let conf: Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
            .unwrap(),
    )
    .unwrap();
    let base = conf["identifier"].as_str().unwrap();
    let data =
        pw_core::registry::path::normalize(&paths.data).unwrap_or_else(|_| paths.data.clone());
    if cfg!(debug_assertions) {
        instance::with_test_suffix(
            instance::identifier(base, Some((Path::new(env!("CARGO_MANIFEST_DIR")), &data))),
            Some(&test_instance()),
            true,
        )
    } else {
        instance::release_identifier(base, Some(&data))
    }
}

/// 앱 자리의 시험 Host: 앱 전용 가짜 툴 `handovertest_slow` 를 단 Host 의 세션을 그 키의 소켓에서 받는다. 버리면 닫힌다.
struct TestApp {
    runtime: Option<tokio::runtime::Runtime>,
    cancel: CancellationToken,
    started: Arc<Notify>,
    sessions: Sessions,
}

impl TestApp {
    fn start(paths: &AppPaths) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let started = Arc::new(Notify::new());
        let s = started.clone();
        let host = Arc::new(
            LocalHost::builder("local")
                .config(paths.clone())
                .methods(move |router| {
                    router.unary(HandoverSlow, move |_ctx, ()| {
                        let started = s.clone();
                        async move {
                            started.notify_one();
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            Ok::<(), RpcError>(())
                        }
                    });
                })
                .build(),
        );
        let router = Arc::new(HostRouter::new());
        router.add_host(host.clone()).unwrap();
        let sessions = Sessions::default();
        let services = Arc::new(Services {
            router,
            host_id: "local".into(),
            paths: paths.clone(),
            global: host.config().unwrap().global.clone(),
            bus: host.bus().clone(),
            audit: Arc::new(AuditLog::under_state(&paths.state)),
            catalog: Arc::new(Catalog::linked().routed_by(host.router())),
            approval: Approval::Unavailable,
            transport: "socket",
            host_kind: HostKind::App,
            sessions: sessions.clone(),
            machines: None,
        });
        let uid = socket::current_uid();
        let path = socket::endpoint(
            &instance_key(paths),
            &paths.state,
            &std::env::temp_dir(),
            uid,
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let listener = runtime
            .block_on(async { socket::bind(&path, uid) })
            .unwrap();
        runtime.spawn(socket::accept_loop(
            listener,
            services,
            socket::Connections::default(),
            cancel.clone(),
        ));
        Self {
            runtime: Some(runtime),
            cancel,
            started,
            sessions,
        }
    }

    fn wait_started(&self) {
        self.runtime.as_ref().unwrap().block_on(async {
            tokio::time::timeout(Duration::from_secs(20), self.started.notified())
                .await
                .expect("앱의 가짜 툴이 시작하지 않았다")
        });
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

/// 띄운 `pitwall mcp` 와 그 stdout 줄.
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl Mcp {
    fn spawn(dir: &Path) -> Self {
        Self::spawn_in(dir, dir)
    }

    /// `cwd` 에서 띄운다 — 프로젝트 안이면 그 프로젝트가 세션 프로젝트다.
    fn spawn_in(dir: &Path, cwd: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pitwall"))
            .arg("mcp")
            .current_dir(cwd)
            .env("PITWALL_CONFIG_DIR", dir.join("config"))
            .env("PITWALL_DATA_DIR", dir.join("data"))
            .env("PITWALL_STATE_DIR", dir.join("state"))
            .env("PITWALL_CACHE_DIR", dir.join("cache"))
            .env("PITWALL_TEST_INSTANCE", test_instance())
            .env("PITWALL_DETACHED", "1")
            .env_remove("PITWALL_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let stdin = child.stdin.take();
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn recv(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(20))
            .expect("pitwall mcp 가 20초 안에 아무것도 쓰지 않았다");
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout 이 JSON 줄이 아니다 ({e}): {line}"))
    }

    /// 그 id 의 응답까지 읽는다. 그 사이의 알림도 돌려준다.
    fn until(&self, id: i64) -> (Value, Vec<Value>) {
        let mut before = Vec::new();
        loop {
            let message = self.recv();
            if message.get("method").is_none() && message["id"] == json!(id) {
                return (message, before);
            }
            before.push(message);
        }
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> (Value, Vec<Value>) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.until(id)
    }

    fn call(&mut self, id: i64, tool: &str) -> (Value, Vec<Value>) {
        self.request(id, "tools/call", json!({"name": tool, "arguments": {}}))
    }

    /// 2025-11-25 클라이언트의 핸드셰이크. 서버 안내문을 돌려준다.
    fn initialize(&mut self) -> String {
        let (answer, _) = self.request(
            0,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "handover-test", "version": "1.0"},
            }),
        );
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        answer["result"]["instructions"]
            .as_str()
            .unwrap_or_else(|| panic!("initialize 의 결과가 아니다: {answer}"))
            .to_owned()
    }

    /// stdin 을 닫지 않고(또는 닫은 뒤) 끝날 때까지 기다린다. (종료 코드, stderr).
    fn finish(mut self) -> (Option<i32>, String) {
        let mut stderr = self.child.stderr.take().unwrap();
        let err = std::thread::spawn(move || {
            let mut s = String::new();
            stderr.read_to_string(&mut s).unwrap();
            s
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                self.child.kill().unwrap();
                panic!("`pitwall mcp` 가 끝나지 않았다");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        (status.code(), err.join().unwrap())
    }
}

fn tool_names(answer: &Value) -> Vec<String> {
    let mut names: Vec<String> = answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list 의 결과가 아니다: {answer}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

fn ok(answer: &Value) {
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
}

/// 감사 로그의 (툴, 세션, 클라이언트), 오래된 것부터. 기다리지 않는다.
fn audit(paths: &AppPaths) -> Vec<(String, String, Option<String>)> {
    let file = paths.state.join("mcp").join("audit.db");
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT tool, session, client FROM calls ORDER BY id") else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// 감사 행이 `count` 개가 될 때까지 기다린다.
fn audit_rows(paths: &AppPaths, count: usize) -> Vec<(String, String, Option<String>)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let rows = audit(paths);
        if rows.len() >= count {
            return rows;
        }
        assert!(
            Instant::now() < deadline,
            "감사 행이 {count} 개가 되지 않았다: {rows:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `$STATE/logs/mcp.<날짜>.log` 에서 앱을 찾은 횟수.
fn checks(paths: &AppPaths) -> usize {
    let Ok(entries) = paths.state.join("logs").read_dir() else {
        return 0;
    };
    entries
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with("mcp.") && name.ends_with(".log")
        })
        .map(|p| {
            std::fs::read_to_string(p)
                .unwrap()
                .matches(CHECK_LOG)
                .count()
        })
        .sum()
}

#[test]
fn a_headless_session_hands_over_when_the_app_starts() {
    let (_tmp, dir, paths) = workspace();
    let mut mcp = Mcp::spawn(&dir);

    let instructions = mcp.initialize();
    assert!(
        instructions.contains("appear in this session without a restart"),
        "{instructions}"
    );
    let (list, _) = mcp.request(1, "tools/list", json!({}));
    assert_eq!(tool_names(&list), CORE_TOOLS, "헤드리스는 코어 툴만이다");
    let (answer, _) = mcp.call(2, "project_list");
    ok(&answer);
    let rows = audit_rows(&paths, 1);
    let headless_session = rows[0].1.clone();
    // ping 은 앱을 찾지 않는다.
    mcp.request(3, "ping", json!({}));

    // 앱이 뜬다. 다음 요청에서 넘겨 붙는다.
    let app = TestApp::start(&paths);
    let (list, before) = mcp.request(4, "tools/list", json!({}));
    assert!(
        before
            .iter()
            .any(|m| m["method"] == json!("notifications/tools/list_changed")),
        "tools/list_changed 가 오지 않았다: {before:?}"
    );
    let names = tool_names(&list);
    assert!(
        names.contains(&"handovertest_slow".to_owned()),
        "앱의 목록이 아니다: {names:?}"
    );

    // 그 뒤 호출의 감사 행은 앱 세션의 것이다.
    let (answer, _) = mcp.call(5, "project_list");
    ok(&answer);
    let rows = audit_rows(&paths, 2);
    let (tool, app_session, client) = rows[1].clone();
    assert_eq!(tool, "project_list");
    assert_ne!(app_session, headless_session, "세션 id 가 넘기며 바뀐다");
    let live: Vec<(String, u64)> = app
        .sessions
        .list()
        .into_iter()
        .map(|s| (s.id, s.calls))
        .collect();
    assert_eq!(
        live,
        [(app_session, 1)],
        "감사 행의 세션이 앱의 세션이고, 앱 세션은 넘긴 뒤의 호출만 센다"
    );
    assert_eq!(
        client.as_deref(),
        Some("handover-test 1.0"),
        "앱 세션은 클라이언트가 처음 보낸 initialize 로 열렸다"
    );
    // 헤드리스에서 끝난 호출의 행은 그대로다.
    assert_eq!(rows[0].1, headless_session);

    // 앱이 도중에 닫히면 프록시와 같은 꼴로 끝난다.
    mcp.send(json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "handovertest_slow", "arguments": {}}}));
    app.wait_started();
    drop(app);
    let answer = mcp.recv();
    assert_eq!(answer["id"], json!(6), "{answer}");
    assert_eq!(answer["error"]["code"], json!(-32001), "{answer}");
    let (code, stderr) = mcp.finish();
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("the Pitwall app closed the connection"),
        "{stderr}"
    );

    // 앱 찾기는 넘기기 전의 tools 요청(1 · 2 · 4)마다 한 번이다. 시작할 때 한 번 본 것과 ping 은 세지 않는다.
    assert_eq!(checks(&paths), 3);
}

/// 헤드리스에서 끝난 호출의 감사 행은 곧바로 넘겨 붙고 끝나도 남는다 — 헤드리스 세션이 기록을 다 쓴 뒤에 프로세스가 끝난다
/// (`crates/pw-mcp/tests/audit_flush.rs` 가 지킨 결함이 넘겨 붙기에서 다시 생기지 않는다). 클라이언트가 닫으면 0 이다.
#[test]
fn a_call_finished_headless_keeps_its_audit_row_after_the_handover() {
    let (_tmp, dir, paths) = workspace();
    let app_paths = paths.clone();
    let mut mcp = Mcp::spawn(&dir);
    mcp.initialize();
    // 앱을 먼저 만들어 두지 않는다 — 첫 호출은 헤드리스다. 응답을 받자마자 앱을 띄우고 넘긴 뒤 곧바로 닫는다(감사 행을 기다리지 않는다).
    let (answer, _) = mcp.call(1, "project_info");
    let _ = answer;
    let app = TestApp::start(&app_paths);
    let (list, _) = mcp.request(2, "tools/list", json!({}));
    assert!(tool_names(&list).contains(&"handovertest_slow".to_owned()));
    mcp.stdin = None;
    let (code, stderr) = mcp.finish();
    assert_eq!(code, Some(0), "stderr: {stderr}");
    let rows = audit(&paths);
    assert_eq!(
        rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["project_info"],
        "헤드리스에서 끝난 호출의 감사 행이 없다"
    );
    drop(app);
}

/// 넘겨 붙은 앱 세션은 `pitwall mcp` 를 시작할 때와 같은 세션 요청(프로젝트 · cwd)으로 열린다 — 세션 프로젝트가 같고, 그 프로젝트의
/// 커밋된 공유 설정(`.pitwall/project.toml` 의 `[mcp]`)이 끈 툴은 넘긴 뒤에도 보이지 않는다(결정 M5-D28). 빈 세션 요청으로 붙으면
/// 세션 프로젝트가 없어 그 좁히기가 빠진다.
#[test]
fn the_app_session_keeps_the_session_project() {
    let (_tmp, dir, paths) = workspace();
    let alpha = dir.join("work").join("alpha");
    std::fs::create_dir_all(alpha.join(".pitwall")).unwrap();
    std::fs::write(
        alpha.join(".pitwall").join("project.toml"),
        "[mcp.tools]\nsettings_get = false\n",
    )
    .unwrap();
    let alpha_id = Registry::new(paths.clone(), None, EventBus::new(16))
        .list()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "alpha")
        .unwrap()
        .id;
    let mut mcp = Mcp::spawn_in(&dir, &alpha);
    mcp.initialize();
    let (list, _) = mcp.request(1, "tools/list", json!({}));
    assert_eq!(
        tool_names(&list),
        ["project_info", "project_list", "project_register"],
        "헤드리스 세션에 alpha 의 좁히기가 걸린다"
    );

    let app = TestApp::start(&paths);
    let (list, before) = mcp.request(2, "tools/list", json!({}));
    assert!(
        before
            .iter()
            .any(|m| m["method"] == json!("notifications/tools/list_changed")),
        "{before:?}"
    );
    let names = tool_names(&list);
    assert!(
        names.contains(&"handovertest_slow".to_owned()),
        "앱의 목록이 아니다: {names:?}"
    );
    assert!(
        !names.contains(&"settings_get".to_owned()),
        "넘긴 뒤 프로젝트가 끈 툴이 보인다: {names:?}"
    );
    let projects: Vec<Option<String>> = app
        .sessions
        .list()
        .into_iter()
        .map(|s| s.project_id)
        .collect();
    assert_eq!(
        projects,
        [Some(alpha_id)],
        "앱 세션의 세션 프로젝트가 alpha 다"
    );

    mcp.stdin = None;
    let (code, stderr) = mcp.finish();
    assert_eq!(code, Some(0), "stderr: {stderr}");
    drop(app);
}
