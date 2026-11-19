//! 실 어댑터 시험(17-m6-plan.md M6-19 `dap-live`). 어댑터는 Pitwall 의 설치기로 받은 실제 것이고(`scripts/dap-live.mjs` 가 받아
//! `PW_TEST_DAP_<어댑터>` 로 준다 — 표지는 `live_cfg.rs`), 정의는 내장 언어 확장의 것, 해석은 앱과 같은 `LspServers`(`[dap.<id>]
//! path`)다. 실행 관리자 · 실행 구성 · 레지스트리 · PTY 도 진짜다. 시나리오마다 작은 프로그램에서 브레이크포인트 → 멈춤 → 스택 ·
//! 변수 → 계속 → 끝을 돌고, 그동안 이 시험 프로세스의 자손이 듣는 TCP 주소가 모두 loopback 인지 잰다(어댑터가 포트를 묶는 주소 —
//! M6-16 리뷰에서 넘김).
//!
//! Java(결정 M6-P20 — attach 만)는 실제 jdtls 를 서버 관리자(`LspManager`)로 띄워 java-debug jar 를 `bundles` 로 싣고,
//! `vscode.java.startDebugSession` 이 연 포트로 붙는다. 결정 D71 의 「다른 주소로 붙으면 거절된다」 는 이 기기의 LAN 주소로 그 포트에
//! 붙어 본다 — 패치한 jar 는 거절하고, 상류 jar(`PW_TEST_DAP_JAVA_DEBUG_UPSTREAM`)는 받아들인다(구분력).
//!
//! 시험 이름은 `live_dap_<어댑터>_` 로 시작한다 — `scripts/dap-live.mjs` 의 필터가 그 접두어로 고르고 하한(`scripts/dap-live.toml`)과
//! 견준다.

#![cfg(unix)]
// 어댑터마다 표지가 따로 켜져 한 어댑터만 받았을 때 다른 어댑터의 도우미 · import 가 쓰이지 않는다.
#![allow(unused_imports, dead_code)]
#![cfg(any(
    pw_live_dap_codelldb,
    pw_live_dap_js_debug,
    pw_live_dap_debugpy,
    pw_live_dap_java_debug
))]

use std::collections::BTreeSet;
use std::fs;
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::Filter;
use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Received, Scope, Subscription};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_core::task::TaskSpawner;
use pw_dap::Timeouts;
use pw_debug::adapter::Adapters;
use pw_debug::{Debugger, HostParts};
use pw_lsp_servers::{Located, LspServers, ToolLocator};
use pw_rpc::{
    Caller, ConnectionScope, EventHub, Id, Outbound, Response, Role, Router, RpcError, Session,
};
use pw_run::runs::{ProcessLauncher, PtyLauncher, RunLimits, RunState, Runs};
use pw_runtime::{LoginShell, Runtime};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

/// 멈춤 · 끝을 기다리는 시한. JVM · jdtls 가 끼면 처음이 느리다.
const WAIT: Duration = Duration::from_secs(120);

fn timeouts() -> Timeouts {
    Timeouts {
        request: Duration::from_secs(30),
        initialize: Duration::from_secs(60),
        initialized: Duration::from_secs(60),
        launch: Duration::from_secs(120),
        connect: Duration::from_secs(60),
        disconnect: Duration::from_secs(5),
        exit_grace: Duration::from_secs(2),
    }
}

fn var(name: &str) -> PathBuf {
    PathBuf::from(
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| panic!("{name} is not set (the live test was built with it)")),
    )
}

// ---- 바탕: 앱과 같은 배선 -----------------------------------------------------------------------------------------------------

/// java-debug 의 jar 를 jdtls 의 `bundles` 로 내놓는 설치본 찾기(설치기의 `locate_share` 흉내 — 경로는 시험이 고른다).
struct JarLocator(PathBuf);

impl ToolLocator for JarLocator {
    fn locate(&self, _package: &str, _bin: &str) -> Located {
        Located::Absent
    }

    fn locate_share(&self, package: &str, _share: &str) -> Located {
        if package == "java-debug-adapter" {
            Located::Installed {
                path: self.0.clone(),
                version: "test".into(),
            }
        } else {
            Located::Absent
        }
    }
}

struct Live {
    _tmp: tempfile::TempDir,
    bus: EventBus,
    runs: Runs,
    debugger: Debugger,
    manager: Option<pw_lsp::LspManager>,
    router: Arc<Router>,
    project: String,
    root: PathBuf,
}

impl Live {
    /// `settings` 는 전역 설정 파일에 덧붙일 줄(`[dap.<id>] path` · `[lsp.jdtls] path`). `jar` 를 주면 서버 관리자를 붙이고 그
    /// jar 를 jdtls 의 `bundles` 로 싣는다.
    fn new(settings: &str, jar: Option<PathBuf>) -> Self {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_test_writer()
            .try_init();
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(
            paths.config.join("settings.toml"),
            format!("schema = 1\n{settings}"),
        )
        .unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let projects = Registry::new(paths.clone(), None, bus.clone());
        // 로그인 셸 흉내: 시험 프로세스의 PATH(node · python · JDK · rustc)를 그대로 쓰고, jdtls 의 작업 공간 색인은 임시 폴더에.
        let shell = base.join("login-shell");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nPATH='{}'; export PATH\nJDTLS_DATA='{}'; export JDTLS_DATA\nunset JAVA_HOME\neval \"$4\"\n",
                std::env::var("PATH").unwrap(),
                base.join("jdtls-data").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = Arc::new(Runtime::new(LoginShell::new(
            Some(shell),
            Duration::from_secs(60),
        )));
        let extensions = Arc::new(pw_ext::Registry::from_roots(
            &semver::Version::new(0, 0, 0),
            pw_ext::builtin::roots(),
        ));
        let servers = Arc::new(LspServers::new(
            Arc::clone(&extensions),
            Arc::clone(&runtime),
        ));
        servers.attach(pw_lsp_servers::HostParts {
            paths: paths.clone(),
            global: global.clone(),
            projects: projects.clone(),
        });
        let runs = Runs::with(
            Arc::clone(&runtime),
            Arc::new(PtyLauncher),
            RunLimits::default(),
        );
        runs.attach(pw_run::runs::HostParts {
            paths: paths.clone(),
            projects: projects.clone(),
            bus: bus.clone(),
        });
        let debugger = Debugger::with(
            runs.clone(),
            Arc::clone(&runtime),
            Arc::clone(&servers) as Arc<dyn Adapters>,
            Arc::new(PtyLauncher) as Arc<dyn ProcessLauncher>,
            timeouts(),
        );
        debugger.attach(HostParts {
            paths: paths.clone(),
            projects: projects.clone(),
            bus: bus.clone(),
        });
        let mut router = Router::new();
        pw_run::runs::methods::register(&mut router, runs.clone());
        pw_debug::methods::register(&mut router, debugger.clone());
        let manager = jar.map(|jar| {
            servers.set_tools(Arc::new(JarLocator(jar)));
            let manager = pw_lsp::LspManager::new(
                Arc::clone(&servers),
                Arc::clone(&extensions),
                pw_lsp::manager::ManagerOptions::default(),
            );
            manager.attach(pw_lsp::manager::HostParts {
                projects: projects.clone(),
                bus: bus.clone(),
                tasks: TaskSpawner::new(bus.clone(), Scope::Host, CancellationToken::new()),
                global: global.clone(),
                state_dir: paths.state.clone(),
                files: None,
            });
            tokio::spawn(manager.clone().follow(pw_lsp::LspManager::subscribe(&bus)));
            pw_lsp::register(&mut router, &manager);
            debugger.attach_language_servers(
                Arc::new(manager.clone()) as Arc<dyn pw_lsp_servers::LspCommands>
            );
            manager
        });
        let root = base.join("project");
        fs::create_dir_all(&root).unwrap();
        let project = projects
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        projects.set_trust(&project, true).unwrap();
        Self {
            _tmp: tmp,
            bus,
            runs,
            debugger,
            manager,
            router: Arc::new(router),
            project,
            root,
        }
    }

    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    /// 실행 구성 `app`(종류 shell — 어댑터는 `[debug] adapter` 로 고른다). `debug` 는 `[debug]` 의 나머지 줄.
    fn config(&self, adapter: &str, debug: &str) {
        self.file(
            ".pitwall/run/app.toml",
            &format!(
                "schema = 1\nname = \"app\"\ntype = \"shell\"\ncommand = \"true\"\n[debug]\nadapter = \"{adapter}\"\n{debug}\n"
            ),
        );
    }

    fn client(&self) -> Client {
        let caller = Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(self.project.clone()),
            window: Some("project-window".into()),
        };
        let (session, rx) = Session::new(Arc::clone(&self.router), EventHub::new(), caller);
        Client {
            session,
            rx,
            next_id: 1,
            responses: Vec::new(),
        }
    }

    fn events(&self) -> Subscription {
        self.bus.subscribe(
            Filter::all()
                .scope(Scope::Project(self.project.clone()))
                .topics([pw_debug::TOPIC_STOPPED, pw_debug::TOPIC_ENDED]),
        )
    }

    /// 그 실행의 콘솔(PTY · 어댑터 출력)에 쌓인 글.
    fn console(&self, run_id: &str) -> String {
        self.runs
            .find(&self.project, run_id)
            .map(|run| {
                run.with_log(|l| String::from_utf8_lossy(&l.read(0, 1 << 20).bytes).into_owned())
            })
            .unwrap_or_default()
    }
}

struct Client {
    session: Session,
    rx: UnboundedReceiver<Outbound>,
    next_id: i64,
    responses: Vec<Response>,
}

impl Client {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            if let Some(i) = self.responses.iter().position(|r| r.id == Id::Number(id)) {
                return self.responses.remove(i).outcome;
            }
            let message = tokio::time::timeout(WAIT, self.rx.recv())
                .await
                .expect("timed out waiting for a response")
                .expect("session closed");
            if let Outbound::Response(r) = message {
                self.responses.push(r);
            }
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        self.call(method, params)
            .await
            .unwrap_or_else(|e| panic!("{method}: {e:?}"))
    }
}

/// `topic` 이벤트 중 `run_id` 의 것을 기다린다.
async fn next_event(events: &mut Subscription, topic: &str, run_id: &str) -> Value {
    let until = Instant::now() + WAIT + Duration::from_secs(5);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        let got = tokio::time::timeout(left, events.recv())
            .await
            .unwrap_or_else(|_| panic!("no {topic} for {run_id} in {WAIT:?}"))
            .expect("the bus closed");
        if let Received::Event(e) = got
            && e.topic == topic
            && e.payload["runId"] == run_id
        {
            return e.payload.clone();
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_port(port: u16) {
    let until = Instant::now() + WAIT;
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < until,
            "nothing listens on 127.0.0.1:{port}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 끝날 때 죽이는 자식(attach 대상).
struct Target(Child);

impl Drop for Target {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(program: &Path, args: &[&str], cwd: &Path) -> Target {
    Target(
        Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // attach 대상의 진단(`Debugger attached.` 등)은 시험 출력에 남긴다.
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| panic!("{}: {e}", program.display())),
    )
}

fn which(name: &str) -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(name))
                .find(|p| p.is_file())
        })
        .unwrap_or_else(|| panic!("{name} is not on PATH"))
}

// ---- 듣는 주소 재기 -----------------------------------------------------------------------------------------------------------

/// 이 시험 프로세스의 자손(어댑터 · 디버그 대상 · jdtls)이 TCP 로 듣는 주소들(`lsof` — 「<명령> <주소>:<포트>」).
fn listening_addresses() -> Vec<String> {
    let ps = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
        .expect("ps");
    let pairs: Vec<(u32, u32)> = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace().map(|s| s.parse::<u32>().ok());
            Some((it.next()??, it.next()??))
        })
        .collect();
    let mut tree = BTreeSet::from([std::process::id()]);
    loop {
        let before = tree.len();
        for (pid, ppid) in &pairs {
            if tree.contains(ppid) {
                tree.insert(*pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    tree.remove(&std::process::id());
    if tree.is_empty() {
        return Vec::new();
    }
    let pids: Vec<String> = tree.iter().map(u32::to_string).collect();
    let out = Command::new("lsof")
        .args(["-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-p", &pids.join(",")])
        .output()
        .expect("lsof");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1)
        .filter_map(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            let name = cols.iter().rev().find(|c| c.contains(':'))?;
            Some(format!("{} {}", cols.first()?, name))
        })
        .collect()
}

fn is_loopback(address: &str) -> bool {
    let host = address.rsplit_once(':').map_or(address, |(h, _)| h);
    matches!(host, "127.0.0.1" | "[::1]" | "localhost")
}

/// 재고 출력하고, loopback 이 아닌 것이 있으면 실패한다(`allowed` 는 시험이 일부러 연 포트 — attach 대상의 디버그 포트).
fn assert_only_loopback(what: &str, allowed: &[u16]) {
    let found = listening_addresses();
    println!("{what}: listening {found:?}");
    let open: Vec<&String> = found
        .iter()
        .filter(|a| {
            let addr = a.split_whitespace().last().unwrap_or_default();
            let port = addr
                .rsplit_once(':')
                .and_then(|(_, p)| p.parse::<u16>().ok());
            !is_loopback(addr) && !port.is_some_and(|p| allowed.contains(&p))
        })
        .collect();
    assert!(
        open.is_empty(),
        "{what}: something listens beyond the loopback address: {open:?}"
    );
}

// ---- 한 바퀴: 브레이크포인트 → 멈춤 → 스택 · 변수 → 계속 ------------------------------------------------------------------

/// 브레이크포인트를 두고 시작해, 그 줄에서 멈추고 `variable` 의 값이 `value` 인지 본 뒤 계속한다. 멈춘 동안 듣는 주소를 잰다.
/// 실행 id 를 돌려준다.
async fn break_and_inspect(
    live: &Live,
    c: &mut Client,
    file: &str,
    line: u32,
    variable: &str,
    value: &str,
    allowed_ports: &[u16],
) -> Stopped {
    let mut events = live.events();
    let mut all = live
        .bus
        .subscribe(Filter::all().scope(Scope::Project(live.project.clone())));
    c.ok(
        "debug.breakpoints.set",
        json!({ "projectId": live.project, "path": file, "breakpoints": [{ "line": line }] }),
    )
    .await;
    let started = c
        .ok(
            "debug.start",
            json!({ "projectId": live.project, "configId": "app" }),
        )
        .await;
    let run_id = started["run"]["runId"].as_str().unwrap().to_owned();
    let stopped = tokio::time::timeout(
        WAIT,
        next_event(&mut events, pw_debug::TOPIC_STOPPED, &run_id),
    )
    .await
    .unwrap_or_else(|_| {
        let mut seen = Vec::new();
        while let Some(Received::Event(e)) = all.try_recv() {
            seen.push(format!("{} {}", e.topic, e.payload));
        }
        panic!(
            "never stopped at {file}:{line}; console:\n{}\nevents:\n{}",
            live.console(&run_id),
            seen.join("\n")
        )
    });
    println!("stopped: {stopped}");
    let session = stopped["session"].clone();
    let thread = stopped["threadId"].as_i64().expect("a thread id");
    let stack = c
        .ok(
            "debug.stack",
            json!({ "runId": run_id, "session": session, "threadId": thread }),
        )
        .await;
    let top = &stack["frames"][0];
    assert_eq!(top["line"], line, "{stack}");
    assert!(
        top["path"].as_str().unwrap_or_default().ends_with(file),
        "{stack}"
    );
    let scopes = c
        .ok(
            "debug.scopes",
            json!({ "runId": run_id, "session": session, "frameId": top["id"] }),
        )
        .await;
    let mut seen = Vec::new();
    let mut found = None;
    for scope in scopes["scopes"].as_array().unwrap() {
        if scope["expensive"] == true {
            continue;
        }
        let vars = c
            .ok(
                "debug.variables",
                json!({ "runId": run_id, "session": session, "variablesReference": scope["variablesReference"] }),
            )
            .await;
        for v in vars["variables"].as_array().unwrap() {
            seen.push(format!("{}={}", v["name"], v["value"]));
            if v["name"] == variable {
                found = Some(v["value"].as_str().unwrap().to_owned());
            }
        }
        if found.is_some() {
            break;
        }
    }
    assert_eq!(found.as_deref(), Some(value), "{seen:?}");
    assert_only_loopback(&format!("{file} stopped"), allowed_ports);
    Stopped {
        run_id,
        session,
        thread,
        events,
    }
}

/// 멈춘 세션.
struct Stopped {
    run_id: String,
    session: Value,
    thread: i64,
    events: Subscription,
}

/// launch: 계속하고, 다시 멈추면(같은 줄의 다른 위치 등) 또 계속해 실행이 끝나기를 기다린다.
async fn continue_to_end(live: &Live, c: &mut Client, mut s: Stopped) -> String {
    let mut resumed = 0;
    loop {
        c.ok(
            "debug.control",
            json!({ "runId": s.run_id, "session": s.session, "threadId": s.thread, "action": "continue" }),
        )
        .await;
        resumed += 1;
        assert!(resumed <= 5, "kept stopping: {}", live.console(&s.run_id));
        let until = Instant::now() + WAIT;
        let again = loop {
            let left = until.saturating_duration_since(Instant::now());
            let got = tokio::time::timeout(left, s.events.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("the debug run did not end: {}", live.console(&s.run_id))
                })
                .expect("the bus closed");
            let Received::Event(e) = got else { continue };
            if e.payload["runId"] != s.run_id.as_str() {
                continue;
            }
            if e.topic == pw_debug::TOPIC_ENDED {
                break None;
            }
            if e.topic == pw_debug::TOPIC_STOPPED {
                break Some(e.payload.clone());
            }
        };
        match again {
            None => break,
            Some(stopped) => {
                s.session = stopped["session"].clone();
                s.thread = stopped["threadId"].as_i64().unwrap();
            }
        }
    }
    ended(live, &s.run_id).await;
    s.run_id
}

/// 실행이 끝나기를 기다린다.
async fn ended(live: &Live, run_id: &str) {
    let run = live.runs.find(&live.project, run_id).unwrap();
    tokio::time::timeout(WAIT, run.exited())
        .await
        .unwrap_or_else(|_| panic!("the debug run did not end: {}", live.console(run_id)));
    assert_eq!(run.state(), RunState::Exited);
}

/// attach 한 실행을 끝낸다(대상은 두고 끊는다).
async fn stop(live: &Live, c: &mut Client, run_id: &str) {
    c.ok("run.stop", json!({ "runId": run_id })).await;
    ended(live, run_id).await;
}

// ---- codelldb(Rust) -----------------------------------------------------------------------------------------------------------

#[cfg(pw_live_dap_codelldb)]
fn codelldb() -> Live {
    Live::new(
        &format!(
            "[dap.codelldb]\npath = \"{}\"\n",
            var("PW_TEST_DAP_CODELLDB").display()
        ),
        None,
    )
}

#[cfg(pw_live_dap_codelldb)]
fn rustc(live: &Live, source: &str, out: &str) -> PathBuf {
    let status = Command::new("rustc")
        .args(["-g", "-C", "opt-level=0", "-o", out, source])
        .current_dir(&live.root)
        .status()
        .expect("rustc");
    assert!(status.success());
    live.root.join(out)
}

#[cfg(pw_live_dap_codelldb)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_codelldb_launch_stops_at_a_breakpoint() {
    let live = codelldb();
    live.file(
        "app.rs",
        "fn main() {\n    let answer = 41 + 1;\n    let shown = answer;\n    println!(\"answer {shown}\");\n}\n",
    );
    let program = rustc(&live, "app.rs", "app");
    live.config("codelldb", &format!("program = \"{}\"", program.display()));
    let mut c = live.client();
    let stopped = break_and_inspect(&live, &mut c, "app.rs", 3, "answer", "42", &[]).await;
    let run_id = continue_to_end(&live, &mut c, stopped).await;
    assert!(
        live.console(&run_id).contains("answer 42"),
        "{}",
        live.console(&run_id)
    );
}

#[cfg(pw_live_dap_codelldb)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_codelldb_attach_to_a_running_process() {
    let live = codelldb();
    live.file(
        "wait.rs",
        "fn main() {\n    loop {\n        let answer = 41 + 1;\n        std::hint::black_box(answer);\n        std::thread::sleep(std::time::Duration::from_millis(50));\n    }\n}\n",
    );
    let program = rustc(&live, "wait.rs", "wait");
    let target = spawn(&program, &[], &live.root);
    live.config(
        "codelldb",
        &format!("mode = \"attach\"\npid = {}", target.0.id()),
    );
    let mut c = live.client();
    let stopped = break_and_inspect(&live, &mut c, "wait.rs", 4, "answer", "42", &[]).await;
    stop(&live, &mut c, &stopped.run_id).await;
}

// ---- js-debug(Node) -----------------------------------------------------------------------------------------------------------

#[cfg(pw_live_dap_js_debug)]
fn js_debug() -> Live {
    Live::new(
        &format!(
            "[dap.js-debug]\npath = \"{}\"\n",
            var("PW_TEST_DAP_JS_DEBUG").display()
        ),
        None,
    )
}

#[cfg(pw_live_dap_js_debug)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_js_debug_launch_stops_at_a_breakpoint() {
    let live = js_debug();
    let app = live.file(
        "app.js",
        "const answer = 41 + 1;\nconsole.log(\"answer\", answer);\n",
    );
    live.config("js-debug", &format!("program = \"{}\"", app.display()));
    let mut c = live.client();
    let stopped = break_and_inspect(&live, &mut c, "app.js", 2, "answer", "42", &[]).await;
    let run_id = continue_to_end(&live, &mut c, stopped).await;
    assert!(
        live.console(&run_id).contains("answer 42"),
        "{}",
        live.console(&run_id)
    );
}

/// Node attach(결정 M6-P20): `node --inspect=127.0.0.1:<포트>` 로 떠 있는 프로세스에 붙어 그 줄에서 멈춘다. js-debug 는 DAP 명령
/// 이름(`attach`)이 아니라 인자의 `request` 로 기본값을 고른다 — 그 값이 빠지면 launch 기본값으로 붙어 자식 세션이 브레이크포인트를
/// 묶지 않는다(M6-19b 보고). 정의의 attach 틀이 `request = "attach"` 를 싣는다.
#[cfg(pw_live_dap_js_debug)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_js_debug_attach_to_an_inspecting_process() {
    let live = js_debug();
    live.file(
        "wait.js",
        "let answer = 0;\nsetInterval(() => {\n  answer = 41 + 1;\n  globalThis.shown = answer;\n}, 50);\n",
    );
    let port = free_port();
    let inspect = format!("--inspect=127.0.0.1:{port}");
    let _target = spawn(&which("node"), &[&inspect, "wait.js"], &live.root);
    wait_port(port).await;
    live.config(
        "js-debug",
        &format!("mode = \"attach\"\nport = {port}\naddress = \"127.0.0.1\""),
    );
    let mut c = live.client();
    let stopped = break_and_inspect(&live, &mut c, "wait.js", 4, "answer", "42", &[port]).await;
    stop(&live, &mut c, &stopped.run_id).await;
}

// ---- debugpy(Python) ----------------------------------------------------------------------------------------------------------

#[cfg(pw_live_dap_debugpy)]
fn debugpy() -> Live {
    Live::new(
        &format!(
            "[dap.debugpy]\npath = \"{}\"\n",
            var("PW_TEST_DAP_DEBUGPY").display()
        ),
        None,
    )
}

/// 설치기가 만든 debugpy 전용 venv 의 python(런처 `<설치 폴더>/pitwall-bin/debugpy-adapter` 옆의 `venv/`).
#[cfg(pw_live_dap_debugpy)]
fn debugpy_python() -> PathBuf {
    let launcher = var("PW_TEST_DAP_DEBUGPY");
    let python = launcher
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("venv/bin/python");
    assert!(python.exists(), "{}", python.display());
    python
}

#[cfg(pw_live_dap_debugpy)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_debugpy_launch_stops_at_a_breakpoint() {
    let live = debugpy();
    let app = live.file("app.py", "answer = 41 + 1\nprint(\"answer\", answer)\n");
    live.config(
        "debugpy",
        &format!(
            "program = \"{}\"\npython = \"{}\"",
            app.display(),
            which("python3").display()
        ),
    );
    let mut c = live.client();
    let stopped = break_and_inspect(&live, &mut c, "app.py", 2, "answer", "42", &[]).await;
    let run_id = continue_to_end(&live, &mut c, stopped).await;
    assert!(
        live.console(&run_id).contains("answer 42"),
        "{}",
        live.console(&run_id)
    );
}

#[cfg(pw_live_dap_debugpy)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_debugpy_attach_to_a_listening_process() {
    let live = debugpy();
    // 어댑터가 127.0.0.1 의 그 포트에서 기다리고(`listen`), 대상은 `python -m debugpy --connect` 로 붙어 온다 — 시작이 어댑터가 듣기를
    // 기다리는 동안 대상을 띄운다.
    let port = free_port();
    live.file(
        "wait.py",
        "import time\nwhile True:\n    answer = 41 + 1\n    time.sleep(0.05)\n",
    );
    live.config(
        "debugpy",
        &format!("mode = \"attach\"\nlisten = {{ host = \"127.0.0.1\", port = {port} }}"),
    );
    let root = live.root.clone();
    let connect = format!("127.0.0.1:{port}");
    let target = tokio::spawn(async move {
        // 붙어 보지 않고 듣기만 확인한다 — 어댑터는 먼저 붙은 연결을 대상으로 받는다.
        let until = Instant::now() + WAIT;
        while !listening_addresses()
            .iter()
            .any(|a| a.ends_with(&format!(":{port}")))
        {
            assert!(
                Instant::now() < until,
                "the adapter never listened on {port}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        spawn(
            &debugpy_python(),
            &["-m", "debugpy", "--connect", &connect, "wait.py"],
            &root,
        )
    });
    let mut c = live.client();
    let stopped = break_and_inspect(&live, &mut c, "wait.py", 4, "answer", "42", &[port]).await;
    let _target = target.await.unwrap();
    stop(&live, &mut c, &stopped.run_id).await;
}

/// 디버그 대상이 띄운 어댑터에 붙는 attach(17-m6-plan M6-19b): `python -m debugpy --listen 127.0.0.1:<포트>` 로 띄운 대상에 구성의
/// `connect` 로 붙어 그 줄에서 멈춘다. Pitwall 은 어댑터를 띄우지 않는다 — 붙는 상대가 이 사용자의 프로세스인지 본 뒤에 말한다.
#[cfg(pw_live_dap_debugpy)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_debugpy_attach_to_a_listening_target() {
    let live = debugpy();
    let port = free_port();
    live.file(
        "wait.py",
        "import time\nwhile True:\n    answer = 41 + 1\n    time.sleep(0.05)\n",
    );
    let listen = format!("127.0.0.1:{port}");
    let _target = spawn(
        &debugpy_python(),
        &["-m", "debugpy", "--listen", &listen, "wait.py"],
        &live.root,
    );
    // 붙어 보지 않고 듣기만 확인한다 — 빈 연결로 대상의 어댑터를 건드리지 않는다. 그 어댑터는 대상이 떼어 띄워 이 시험의 자손이
    // 아니다(`listening_addresses` 가 못 본다) — 포트로 찾는다.
    let until = Instant::now() + WAIT;
    while !Command::new("lsof")
        .args([
            "-nP",
            &format!("-iTCP@127.0.0.1:{port}"),
            "-sTCP:LISTEN",
            "-t",
        ])
        .output()
        .is_ok_and(|o| !o.stdout.is_empty())
    {
        assert!(
            Instant::now() < until,
            "the target never listened on {port}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    live.config(
        "debugpy",
        &format!("mode = \"attach\"\nconnect = {{ port = {port} }}"),
    );
    let mut c = live.client();
    let stopped = break_and_inspect(&live, &mut c, "wait.py", 4, "answer", "42", &[port]).await;
    stop(&live, &mut c, &stopped.run_id).await;
}

// ---- java-debug(JVM — attach 만) ------------------------------------------------------------------------------------------------

#[cfg(pw_live_dap_java_debug)]
const APP_JAVA: &str = "public class App {\n    public static void main(String[] args) throws Exception {\n        while (true) {\n            int answer = 41 + 1;\n            Thread.sleep(50);\n        }\n    }\n}\n";

/// jdtls 를 서버 관리자로 띄우고(App.java 를 연다) `jar` 를 `bundles` 로 싣는다. 서버가 명령에 답할 때까지 기다린다.
#[cfg(pw_live_dap_java_debug)]
async fn jdtls_with(jar: PathBuf) -> (Live, Client) {
    let live = Live::new(
        &format!(
            "[lsp.jdtls]\npath = \"{}\"\n",
            var("PW_TEST_LSP_JDTLS").display()
        ),
        Some(jar),
    );
    let app = live.file("App.java", APP_JAVA);
    let mut c = live.client();
    c.ok(
        "lsp.document.open",
        json!({ "projectId": live.project, "path": app, "language": "Java", "version": 0, "text": APP_JAVA }),
    )
    .await;
    (live, c)
}

/// java-debug 를 띄워(`vscode.java.startDebugSession`) 그 포트를 받는다 — 디버그 세션과 같은 길(`LspCommands`).
#[cfg(pw_live_dap_java_debug)]
async fn java_debug_port(live: &Live) -> u16 {
    use pw_lsp_servers::LspCommands as _;
    let manager = live.manager.as_ref().unwrap();
    let until = Instant::now() + WAIT;
    loop {
        match manager
            .execute_command(
                &live.project,
                &live.root,
                "jdtls",
                "vscode.java.startDebugSession",
                Vec::new(),
            )
            .await
        {
            Some(Ok(port)) => return u16::try_from(port.as_u64().unwrap()).unwrap(),
            Some(Err(e)) => panic!("jdtls refused to start java-debug: {e}"),
            None => {
                assert!(Instant::now() < until, "jdtls never came up");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// 이 기기의 LAN 주소(기본 경로의 바깥 주소 — UDP 를 「연결」 만 하고 보내지 않는다).
#[cfg(pw_live_dap_java_debug)]
fn lan_address() -> IpAddr {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("192.0.2.1:9").expect("a default route");
    let ip = socket.local_addr().unwrap().ip();
    assert!(
        !ip.is_loopback() && !ip.is_unspecified(),
        "this machine has no LAN address to try ({ip})"
    );
    ip
}

/// 그 포트에 LAN 주소로 붙어 DAP `initialize` 를 보내고 답을 받는가.
#[cfg(pw_live_dap_java_debug)]
fn answers_over(ip: IpAddr, port: u16) -> Result<String, String> {
    use std::io::{Read as _, Write as _};
    let mut stream = TcpStream::connect_timeout(&(ip, port).into(), Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body =
        r#"{"seq":1,"type":"request","command":"initialize","arguments":{"adapterID":"probe"}}"#;
    stream
        .write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .map_err(|e| e.to_string())?;
    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

/// 결정 D71: 패치한 java-debug 는 127.0.0.1 에만 듣는다 — 이 기기의 LAN 주소로 붙으면 거절되고, 127.0.0.1 로는 DAP 가 답한다.
#[cfg(pw_live_dap_java_debug)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_java_debug_refuses_a_connection_from_another_address() {
    let (live, _c) = jdtls_with(var("PW_TEST_DAP_JAVA_DEBUG")).await;
    let port = java_debug_port(&live).await;
    let lan = lan_address();
    let found = listening_addresses();
    println!("patched java-debug on {port}: listening {found:?}, LAN {lan}");
    assert!(
        found
            .iter()
            .any(|a| a.ends_with(&format!("127.0.0.1:{port}"))),
        "{found:?}"
    );
    let refused = answers_over(lan, port);
    assert!(refused.is_err(), "a LAN connection was served: {refused:?}");
    let served = answers_over(IpAddr::from([127, 0, 0, 1]), port).unwrap();
    assert!(served.contains("\"initialize\""), "{served}");
    assert_only_loopback("patched java-debug", &[]);
}

/// 구분력: 상류 그대로의 jar 는 같은 시험에서 LAN 주소로 붙은 연결에도 DAP 로 답한다(모든 주소에 묶인다 — D71 이 고친 것).
#[cfg(pw_live_dap_java_debug)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_java_debug_upstream_jar_serves_another_address() {
    let (live, _c) = jdtls_with(var("PW_TEST_DAP_JAVA_DEBUG_UPSTREAM")).await;
    let port = java_debug_port(&live).await;
    let lan = lan_address();
    let found = listening_addresses();
    println!("upstream java-debug on {port}: listening {found:?}, LAN {lan}");
    let served = answers_over(lan, port).unwrap_or_else(|e| {
        panic!("the upstream jar refused the LAN connection ({e}) — the discriminating check no longer discriminates")
    });
    assert!(served.contains("\"initialize\""), "{served}");
}

/// JVM attach(결정 M6-P20): JDWP 를 켜고 띄운 JVM 에 jdtls 가 연 java-debug 로 붙어 멈추고 변수를 읽는다.
#[cfg(pw_live_dap_java_debug)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_dap_java_debug_attach_to_a_jdwp_process() {
    let (live, mut c) = jdtls_with(var("PW_TEST_DAP_JAVA_DEBUG")).await;
    let status = Command::new(which("javac"))
        .args(["-g", "-d", "classes", "App.java"])
        .current_dir(&live.root)
        .status()
        .expect("javac");
    assert!(status.success());
    let port = free_port();
    let agent =
        format!("-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=127.0.0.1:{port}");
    let _target = spawn(
        &which("java"),
        &[&agent, "-cp", "classes", "App"],
        &live.root,
    );
    wait_port(port).await;
    // 서버가 떠서 명령에 답할 때까지(디버그 시작은 떠 있지 않으면 거절한다).
    let _ = java_debug_port(&live).await;
    live.config("java-debug", &format!("mode = \"attach\"\nport = {port}"));
    let stopped = break_and_inspect(&live, &mut c, "App.java", 5, "answer", "42", &[port]).await;
    stop(&live, &mut c, &stopped.run_id).await;
}
