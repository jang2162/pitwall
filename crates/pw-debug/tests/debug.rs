//! 디버그 세션 · 브레이크포인트(17-m6-plan.md M6-17 완료 기준 ① ~ ⑨). 어댑터는 가짜(`pw-debug-fake-adapter` — pw-dap 의 가짜와 같은
//! 소스), 실행 관리자 · 실행 구성 · 레지스트리는 진짜다. 디버그 대상의 PTY 는 진짜 `PtyLauncher` 를 센다.

// 실행 관리자의 PTY · 가짜 로그인 셸 — unix 만(pw-run 의 시험과 같다).
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_dap::Timeouts;
use pw_dap::fake::{Action, Reply, Scenario};
use pw_debug::adapter::Adapters;
use pw_debug::{Breakpoint, Breakpoints, Debugger, HostParts};
use pw_ext::{DapTransport, DebugAdapterDecl, NameRef};
use pw_lsp_servers::{AdapterConfig, AdapterLaunch, AdapterSetup, LspResolution, LspResolveStep};
use pw_rpc::{
    Caller, ConnectionScope, EventHub, Id, Outbound, Response, Role, Router, RpcError, Session,
    code,
};
use pw_run::runs::{
    ProcessLauncher, ProcessSpec, PtyLauncher, RunKind, RunLimits, RunOutput, RunState, Runs,
};
use pw_runtime::{LoginShell, Runtime, RuntimeEnv};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

const SOON: Duration = Duration::from_secs(10);

// ---- 가짜 어댑터 정의 · 해석 ----------------------------------------------------------------------------------------------

/// 어댑터 해석 흉내: 정의 하나(`fake`, rust 를 맡는다)와 그 실행 파일 = 가짜 어댑터(인자 = 시나리오 경로).
struct FakeAdapters {
    scenario: Mutex<PathBuf>,
    setups: AtomicUsize,
    args: Mutex<Option<Vec<String>>>,
    /// 언어 서버가 여는 어댑터(java-debug 의 꼴 — `start`)로 푼다.
    via_language_server: std::sync::atomic::AtomicBool,
}

fn definition(scenario: &Path, args: Option<Vec<String>>) -> DebugAdapterDecl {
    DebugAdapterDecl {
        id: "fake".into(),
        pos: None,
        languages: vec![NameRef {
            name: "rust".into(),
            pos: None,
        }],
        mason: None,
        command: Some("fake".into()),
        args: args.unwrap_or_else(|| vec![scenario.to_string_lossy().into_owned()]),
        env: BTreeMap::new(),
        transport: DapTransport::Stdio,
        launch: Some(json!({ "program": "{program}", "cwd": "{cwd}", "env": "{env}" })),
        attach: Some(json!({ "port": "{port}" })),
        requires: Vec::new(),
        project_local: None,
        start: None,
    }
}

impl Adapters for FakeAdapters {
    fn definitions(&self) -> Vec<DebugAdapterDecl> {
        vec![definition(&self.scenario.lock().unwrap(), None)]
    }

    fn setup(
        &self,
        _project_id: &str,
        _worktree: Option<&str>,
        adapter_id: &str,
        _dir: &Path,
    ) -> Result<AdapterSetup, RpcError> {
        self.setups.fetch_add(1, Ordering::SeqCst);
        assert_eq!(adapter_id, "fake");
        let def = definition(&self.scenario.lock().unwrap(), None);
        // 사람의 `[dap.fake] args` 흉내(있으면 정의의 것을 이긴다).
        let args = self
            .args
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(def.args.clone());
        let launch = if self.via_language_server.load(Ordering::SeqCst) {
            AdapterLaunch::LanguageServer {
                server: "jdtls".into(),
                command: "vscode.java.startDebugSession".into(),
            }
        } else {
            AdapterLaunch::Process(LspResolution::Found {
                path: PathBuf::from(env!("CARGO_BIN_EXE_pw-debug-fake-adapter")),
                step: LspResolveStep::Path,
                version: None,
                skipped: Vec::new(),
            })
        };
        Ok(AdapterSetup {
            extension: "dev.example.fake".into(),
            config: AdapterConfig {
                enabled: true,
                path: None,
                args,
                env: BTreeMap::new(),
            },
            launch,
            env: RuntimeEnv::process(),
            definition: def,
        })
    }
}

// ---- 디버그 대상 띄우기(진짜 PTY) 세기 ------------------------------------------------------------------------------------

#[derive(Default)]
struct Counting {
    calls: AtomicUsize,
    /// 띄우기 직전에 부른다(끝내기를 끼워 넣는 자리).
    before: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    launched: Mutex<Vec<Arc<dyn RunKind>>>,
}

impl ProcessLauncher for Counting {
    fn launch(&self, spec: &ProcessSpec, output: RunOutput) -> Result<Arc<dyn RunKind>, RpcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(before) = &*self.before.lock().unwrap() {
            before();
        }
        let kind = PtyLauncher.launch(spec, output)?;
        self.launched.lock().unwrap().push(Arc::clone(&kind));
        Ok(kind)
    }
}

// ---- 바탕 -----------------------------------------------------------------------------------------------------------------

struct Fixture {
    tmp: tempfile::TempDir,
    paths: AppPaths,
    projects: Registry,
    runs: Runs,
    debugger: Debugger,
    adapters: Arc<FakeAdapters>,
    launcher: Arc<Counting>,
    project: String,
    root: PathBuf,
    record: PathBuf,
}

fn timeouts() -> Timeouts {
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

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let projects = Registry::new(paths.clone(), None, bus.clone());
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        let shell = base.join("fake-login-shell");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nHOME='{}'; export HOME\nPATH=/usr/bin:/bin; export PATH\nSHELL=/bin/sh; export SHELL\neval \"$4\"\n",
                home.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = Arc::new(Runtime::new(LoginShell::new(
            Some(shell),
            Duration::from_secs(5),
        )));
        let runs = Runs::with(
            Arc::clone(&runtime),
            Arc::new(PtyLauncher),
            RunLimits {
                stop_grace: Duration::from_millis(500),
                ..RunLimits::default()
            },
        );
        runs.attach(pw_run::runs::HostParts {
            paths: paths.clone(),
            projects: projects.clone(),
            bus: bus.clone(),
        });
        let record = base.join("record.jsonl");
        let adapters = Arc::new(FakeAdapters {
            scenario: Mutex::new(base.join("scenario.json")),
            setups: AtomicUsize::new(0),
            args: Mutex::new(None),
            via_language_server: Default::default(),
        });
        let launcher = Arc::new(Counting::default());
        let debugger = Debugger::with(
            runs.clone(),
            runtime,
            Arc::clone(&adapters) as Arc<dyn Adapters>,
            Arc::clone(&launcher) as Arc<dyn ProcessLauncher>,
            timeouts(),
        );
        debugger.attach(HostParts {
            paths: paths.clone(),
            projects: projects.clone(),
            bus: bus.clone(),
        });
        let mut f = Self {
            tmp,
            paths,
            projects,
            runs,
            debugger,
            adapters,
            launcher,
            project: String::new(),
            root: PathBuf::new(),
            record,
        };
        let (project, root) = f.project("project", true);
        f.project = project;
        f.root = root;
        f.config(&f.root.clone(), "app", "");
        f.scenario(Scenario::default());
        f
    }

    fn base(&self) -> PathBuf {
        simplify(fs::canonicalize(self.tmp.path()).unwrap())
    }

    fn project(&self, name: &str, trusted: bool) -> (String, PathBuf) {
        let root = self.base().join(name);
        fs::create_dir_all(root.join("src")).unwrap();
        let id = self
            .projects
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        if trusted {
            self.projects.set_trust(&id, true).unwrap();
        }
        (id, root)
    }

    /// 실행 구성 `id`(cargo — 어댑터는 언어로 고른다) 와 `[debug]` 에 더할 줄.
    fn config(&self, root: &Path, id: &str, debug: &str) {
        let dir = root.join(".pitwall/run");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{id}.toml")),
            format!(
                "schema = 1\nname = \"{id}\"\ntype = \"cargo\"\ncommand = \"run\"\nenv = {{ APP_MODE = \"debug\" }}\n[debug]\nprogram = \"target/debug/app\"\n{debug}\n"
            ),
        )
        .unwrap();
    }

    fn scenario(&self, mut scenario: Scenario) {
        scenario.record = Some(self.record.clone());
        let path = self.adapters.scenario.lock().unwrap().clone();
        fs::write(&path, serde_json::to_vec_pretty(&scenario).unwrap()).unwrap();
    }

    fn records(&self) -> Vec<Value> {
        fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// 어댑터가 받은 요청(그 명령).
    fn received(&self, command: &str) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter_map(|r| r.get("msg").cloned())
            .filter(|m| m["type"] == "request" && m["command"] == command)
            .collect()
    }

    fn client(&self) -> Client {
        self.client_in(ConnectionScope::Project(self.project.clone()))
    }

    fn client_in(&self, scope: ConnectionScope) -> Client {
        self.client_as(Caller {
            role: Role::Owner,
            scope,
            window: Some("project-window".into()),
        })
    }

    fn client_as(&self, caller: Caller) -> Client {
        let mut router = Router::new();
        pw_run::runs::methods::register(&mut router, self.runs.clone());
        pw_debug::methods::register(&mut router, self.debugger.clone());
        let (session, rx) = Session::new(Arc::new(router), EventHub::new(), caller);
        Client {
            session,
            rx,
            next_id: 1,
            responses: Vec::new(),
        }
    }

    fn stored(&self) -> Vec<(String, Vec<u32>)> {
        self.debugger
            .breakpoints()
            .unwrap()
            .get(&self.project)
            .unwrap()
            .files
            .into_iter()
            .map(|(p, l)| (p, l.iter().map(|b| b.line).collect()))
            .collect()
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
            let message = tokio::time::timeout(Duration::from_secs(30), self.rx.recv())
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

    async fn err(&mut self, method: &str, params: Value) -> RpcError {
        match self.call(method, params).await {
            Ok(v) => panic!("{method} succeeded: {v}"),
            Err(e) => e,
        }
    }
}

async fn eventually(wait: Duration, mut check: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + wait;
    loop {
        if check() {
            return true;
        }
        if Instant::now() > until {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn start_params(f: &Fixture, config: &str) -> Value {
    json!({ "projectId": f.project, "configId": config })
}

async fn start(f: &Fixture, c: &mut Client) -> String {
    let started = c.ok("debug.start", start_params(f, "app")).await;
    started["run"]["runId"].as_str().unwrap().to_owned()
}

async fn stop(f: &Fixture, c: &mut Client, run_id: &str) {
    c.ok("run.stop", json!({ "runId": run_id })).await;
    let run = f.runs.find(&f.project, run_id).unwrap();
    tokio::time::timeout(SOON, run.exited())
        .await
        .expect("the debug run did not end");
}

fn bp(line: u32) -> Value {
    json!({ "line": line })
}

// ---- ① 저장 · 다시 읽기 ---------------------------------------------------------------------------------------------------

/// ① 브레이크포인트는 `$DATA/projects/<id>/breakpoints.json` 에 남고, 앱을 다시 켠 것(새 저장소)이 그대로 읽는다 — 조건 · 로그
/// 메시지 · hit count · 켬/끔까지.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakpoints_are_stored_and_read_again() {
    let f = Fixture::new();
    let mut c = f.client();
    let saved = c
        .ok(
            "debug.breakpoints.set",
            json!({
                "projectId": f.project, "path": "src/main.rs",
                "breakpoints": [
                    { "line": 7, "condition": "i > 3" },
                    { "line": 3, "enabled": false },
                    { "line": 9, "logMessage": "i = {i}", "hitCondition": "5" }
                ]
            }),
        )
        .await;
    assert_eq!(saved["path"], "src/main.rs");
    let file = f
        .paths
        .data
        .join("projects")
        .join(&f.project)
        .join("breakpoints.json");
    assert!(file.is_file(), "{}", file.display());

    // 앱을 다시 켠 것: 같은 `$DATA` 의 새 저장소.
    let again = Breakpoints::new(f.paths.data.clone());
    let list = again.get(&f.project).unwrap().files["src/main.rs"].clone();
    assert_eq!(
        list,
        [
            Breakpoint {
                enabled: false,
                ..Breakpoint::at(3)
            },
            Breakpoint {
                condition: Some("i > 3".into()),
                ..Breakpoint::at(7)
            },
            Breakpoint {
                log_message: Some("i = {i}".into()),
                hit_condition: Some("5".into()),
                ..Breakpoint::at(9)
            },
        ]
    );
    let listed = c
        .ok("debug.breakpoints.list", json!({ "projectId": f.project }))
        .await;
    assert_eq!(
        listed["files"][0]["breakpoints"].as_array().unwrap().len(),
        3
    );

    // 지우기 — 한 줄, 그다음 파일 전부.
    c.ok(
        "debug.breakpoints.remove",
        json!({ "projectId": f.project, "path": "src/main.rs", "line": 7 }),
    )
    .await;
    assert_eq!(f.stored(), [("src/main.rs".to_owned(), vec![3, 9])]);
    c.ok(
        "debug.breakpoints.remove",
        json!({ "projectId": f.project, "path": "src/main.rs" }),
    )
    .await;
    assert!(
        Breakpoints::new(f.paths.data.clone())
            .get(&f.project)
            .unwrap()
            .files
            .is_empty()
    );

    // 루트 밖 · 절대 경로는 받지 않는다.
    for path in ["../x.rs", "/etc/passwd"] {
        let e = c
            .err(
                "debug.breakpoints.set",
                json!({ "projectId": f.project, "path": path, "breakpoints": [bp(1)] }),
            )
            .await;
        assert_eq!(e.code, code::INVALID_PARAMS, "{path}");
    }
}

/// ① 의 범위: 브레이크포인트는 그 프로젝트 창만 읽고 바꾼다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_project_window_touches_its_breakpoints() {
    let f = Fixture::new();
    let (other, _) = f.project("other", true);
    for scope in [
        ConnectionScope::Project(other),
        ConnectionScope::Agent,
        ConnectionScope::Host,
    ] {
        let mut c = f.client_in(scope.clone());
        for (method, params) in [
            ("debug.breakpoints.list", json!({ "projectId": f.project })),
            (
                "debug.breakpoints.set",
                json!({ "projectId": f.project, "path": "a.rs", "breakpoints": [bp(1)] }),
            ),
        ] {
            let e = c.err(method, params).await;
            assert_eq!(e.code, code::PERMISSION_DENIED, "{method} from {scope:?}");
        }
    }
    assert!(f.stored().is_empty());
}

// ---- ② · ③ 세션 중의 브레이크포인트 ------------------------------------------------------------------------------------------

/// ② 세션 중 `set` 은 곧바로 어댑터의 `setBreakpoints` 로 간다(그 세션의 워크트리 루트에 이은 절대 경로, 켠 것만, 조건 그대로).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_set_during_a_session_goes_to_the_adapter_at_once() {
    let f = Fixture::new();
    let mut c = f.client();
    let run_id = start(&f, &mut c).await;
    assert!(
        f.received("setBreakpoints").is_empty(),
        "nothing was stored yet"
    );
    c.ok(
        "debug.breakpoints.set",
        json!({
            "projectId": f.project, "path": "src/lib.rs",
            "breakpoints": [{ "line": 4, "condition": "n == 2" }, { "line": 6, "enabled": false }]
        }),
    )
    .await;
    assert!(eventually(SOON, || !f.received("setBreakpoints").is_empty()).await);
    let sent = &f.received("setBreakpoints")[0]["arguments"];
    assert_eq!(
        sent["source"]["path"],
        f.root.join("src/lib.rs").to_string_lossy().as_ref()
    );
    assert_eq!(
        sent["breakpoints"],
        json!([{ "line": 4, "condition": "n == 2" }])
    );
    stop(&f, &mut c, &run_id).await;
}

/// ③ 어댑터가 옮긴 줄이 저장에 반영된다 — 시작 때의 `setBreakpoints` 답과, 세션 중의 `breakpoint` 이벤트(`changed`) 둘 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lines_the_adapter_moves_are_stored() {
    let f = Fixture::new();
    f.debugger
        .breakpoints()
        .unwrap()
        .set_file(
            &f.project,
            "src/main.rs",
            vec![Breakpoint::at(3), Breakpoint::at(10)],
        )
        .unwrap();
    f.scenario(
        Scenario::default()
            .reply(
                "setBreakpoints",
                Reply::body(json!({ "breakpoints": [
                    { "id": 1, "verified": true, "line": 5 },
                    { "id": 2, "verified": false, "line": 10, "message": "no code" }
                ]})),
            )
            .on(
                "threads",
                vec![Action::Event {
                    event: "breakpoint".into(),
                    body: json!({ "reason": "changed", "breakpoint": { "id": 2, "verified": true, "line": 12 } }),
                }],
            ),
    );
    let mut c = f.client();
    let run_id = start(&f, &mut c).await;
    assert_eq!(f.stored(), [("src/main.rs".to_owned(), vec![5, 10])]);
    // 세션 중의 바뀜: 어댑터가 2 번을 12 줄로.
    c.ok("debug.threads", json!({ "runId": run_id })).await;
    assert!(
        eventually(SOON, || f.stored()
            == [("src/main.rs".to_owned(), vec![5, 12])])
        .await,
        "{:?}",
        f.stored()
    );
    stop(&f, &mut c, &run_id).await;
}

// ---- ④ · ⑤ 허가 · 범위 ------------------------------------------------------------------------------------------------------

/// ④ 미신뢰 프로젝트의 `debug.start` 는 어댑터를 띄우지 않는다(어댑터 해석도 하지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_starts_no_adapter() {
    let f = Fixture::new();
    let (untrusted, root) = f.project("untrusted", false);
    f.config(&root, "app", "");
    let mut c = f.client_in(ConnectionScope::Project(untrusted.clone()));
    let e = c
        .err(
            "debug.start",
            json!({ "projectId": untrusted, "configId": "app" }),
        )
        .await;
    assert_eq!(e.code, code::PERMISSION_DENIED);
    assert_eq!(e.data.as_ref().unwrap()["reason"], "untrusted");
    assert_eq!(f.adapters.setups.load(Ordering::SeqCst), 0);
    assert!(f.records().is_empty(), "the fake adapter never started");
    assert!(f.runs.list(&untrusted).is_empty());
}

/// ⑤ 다른 프로젝트 창 · 에이전트 범위 · Host 의 `debug.start` 는 거절이고, 다른 프로젝트 창 · Host · 에이전트의
/// `debug.evaluate { 그 세션 }` 은 「없는 실행」 이거나 거절이다(있다는 것을 알리지 않는다 — 없는 id 와 같은 오류).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_windows_and_agents_cannot_start_or_evaluate() {
    let f = Fixture::new();
    let (other, _) = f.project("other", true);
    for scope in [
        ConnectionScope::Project(other.clone()),
        ConnectionScope::Agent,
        ConnectionScope::Host,
    ] {
        let mut c = f.client_in(scope.clone());
        let e = c.err("debug.start", start_params(&f, "app")).await;
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
    }
    assert_eq!(f.adapters.setups.load(Ordering::SeqCst), 0);
    assert!(f.records().is_empty());

    f.scenario(Scenario::default().reply(
        "evaluate",
        Reply::body(json!({ "result": "2", "variablesReference": 0 })),
    ));
    let mut mine = f.client();
    let run_id = start(&f, &mut mine).await;
    let evaluate = |id: &str| json!({ "runId": id, "expression": "1 + 1" });
    // 그 프로젝트 창은 평가한다.
    mine.ok("debug.evaluate", evaluate(&run_id)).await;
    let mut theirs = f.client_in(ConnectionScope::Project(other));
    let real = theirs.err("debug.evaluate", evaluate(&run_id)).await;
    let none = theirs.err("debug.evaluate", evaluate("no-such-run")).await;
    assert_eq!(real.code, code::NOT_FOUND);
    assert_eq!(
        (real.code, real.message.replace(&run_id, "<id>")),
        (none.code, none.message.replace("no-such-run", "<id>"))
    );
    let real = theirs
        .err("debug.threads", json!({ "runId": run_id }))
        .await;
    assert_eq!(real.code, code::NOT_FOUND);
    // 파일 창은 디스패처가 먼저 거절한다 — 진짜 id 와 없는 id 가 같다.
    let mut file = f.client_as(Caller::file(Role::Owner, f.root.join("src/main.rs")));
    let real = file.err("debug.evaluate", evaluate(&run_id)).await;
    let none = file.err("debug.evaluate", evaluate("no-such-run")).await;
    assert_eq!((real.code, &real.data), (none.code, &none.data));
    for scope in [ConnectionScope::Agent, ConnectionScope::Host] {
        let mut c = f.client_in(scope.clone());
        let e = c.err("debug.evaluate", evaluate(&run_id)).await;
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
        let e = c.err("debug.threads", json!({ "runId": run_id })).await;
        assert_eq!(e.code, code::NOT_FOUND, "{scope:?}");
    }
    assert_eq!(
        f.received("evaluate").len(),
        1,
        "only the project window's evaluate reached the adapter"
    );
    stop(&f, &mut mine, &run_id).await;
}

// ---- ⑥ 변수의 쪽 ----------------------------------------------------------------------------------------------------------

/// ⑥ `debug.variables` 는 쪽 단위다: 어댑터에 `start` · `count` 를 주고(상한 500), 쪽을 지키지 않는 어댑터가 다 주면 그 쪽만 자른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn variables_come_in_pages() {
    let f = Fixture::new();
    let all: Vec<Value> = (0..10)
        .map(
            |i| json!({ "name": format!("v{i}"), "value": i.to_string(), "variablesReference": 0 }),
        )
        .collect();
    f.scenario(Scenario::default().reply("variables", Reply::body(json!({ "variables": all }))));
    let mut c = f.client();
    let run_id = start(&f, &mut c).await;
    let page = c
        .ok(
            "debug.variables",
            json!({ "runId": run_id, "variablesReference": 7, "start": 2, "count": 3 }),
        )
        .await;
    let names: Vec<&str> = page["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["v2", "v3", "v4"]);
    let asked = &f.received("variables")[0]["arguments"];
    assert_eq!(
        (
            asked["variablesReference"].clone(),
            asked["start"].clone(),
            asked["count"].clone()
        ),
        (json!(7), json!(2), json!(3))
    );
    let page = c
        .ok(
            "debug.variables",
            json!({ "runId": run_id, "variablesReference": 7, "count": 100000 }),
        )
        .await;
    assert_eq!(page["count"], 500);
    assert_eq!(f.received("variables")[1]["arguments"]["count"], 500);
    stop(&f, &mut c, &run_id).await;
}

// ---- ⑦ 예외 필터 ----------------------------------------------------------------------------------------------------------

/// ⑦ 어댑터가 `initialize` 로 알린 예외 필터만 켜고 끌 수 있고, 켠 필터가 `setExceptionBreakpoints` 로 간다. 켠 것은 어댑터마다 남아
/// 다음 세션의 시작에 실린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_announced_exception_filters_can_be_turned_on() {
    let f = Fixture::new();
    f.scenario(Scenario {
        capabilities: json!({
            "exceptionBreakpointFilters": [
                { "filter": "uncaught", "label": "Uncaught", "default": true },
                { "filter": "all", "label": "All" }
            ]
        }),
        ..Scenario::default()
    });
    let mut c = f.client();
    let started = c.ok("debug.start", start_params(&f, "app")).await;
    let run_id = started["run"]["runId"].as_str().unwrap().to_owned();
    assert_eq!(
        started["exceptionFilters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (
                e["filter"].as_str().unwrap(),
                e["enabled"].as_bool().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("uncaught", false), ("all", false)]
    );
    let before = f.received("setExceptionBreakpoints").len();
    let e = c
        .err(
            "debug.exceptions.set",
            json!({ "runId": run_id, "filters": ["uncaught", "never-announced"] }),
        )
        .await;
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert_eq!(
        f.received("setExceptionBreakpoints").len(),
        before,
        "a refused set sends nothing"
    );
    let set = c
        .ok(
            "debug.exceptions.set",
            json!({ "runId": run_id, "filters": ["uncaught"] }),
        )
        .await;
    assert_eq!(set["exceptionFilters"][0]["enabled"], true);
    let sent = f.received("setExceptionBreakpoints");
    assert_eq!(
        sent.last().unwrap()["arguments"]["filters"],
        json!(["uncaught"])
    );
    stop(&f, &mut c, &run_id).await;

    // 다음 세션은 시작 때 켠 필터를 싣는다.
    let n = f.received("setExceptionBreakpoints").len();
    let run_id = start(&f, &mut c).await;
    let sent = f.received("setExceptionBreakpoints");
    assert_eq!(sent.len(), n + 1);
    assert_eq!(
        sent.last().unwrap()["arguments"]["filters"],
        json!(["uncaught"])
    );
    stop(&f, &mut c, &run_id).await;
}

// ---- ⑧ attach ------------------------------------------------------------------------------------------------------------

/// ⑧ `mode = "attach"` 구성은 `attach` 요청만 보내고 프로그램을 띄우지 않는다(디버그 대상 띄우기 0). 끝낼 때 대상을 끝내라고 하지
/// 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attach_config_launches_nothing() {
    let f = Fixture::new();
    f.config(&f.root, "app", "mode = \"attach\"\nport = 9229");
    let mut c = f.client();
    let started = c.ok("debug.start", start_params(&f, "app")).await;
    assert_eq!(started["mode"], "attach");
    let run_id = started["run"]["runId"].as_str().unwrap().to_owned();
    assert!(f.received("launch").is_empty());
    let attach = f.received("attach");
    assert_eq!(attach.len(), 1);
    assert_eq!(attach[0]["arguments"], json!({ "port": 9229 }));
    assert_eq!(f.launcher.calls.load(Ordering::SeqCst), 0);
    stop(&f, &mut c, &run_id).await;
    let disconnect = f.received("disconnect");
    assert_eq!(
        disconnect.last().unwrap()["arguments"]["terminateDebuggee"],
        false
    );
}

/// launch 구성의 요청은 틀을 `[debug]` 의 값과 Pitwall 의 값(`{cwd}` · `{env}`)으로 채운 것이다. Pitwall 의 값을 `[debug]` 에
/// 적으면 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_launch_request_fills_the_template() {
    let f = Fixture::new();
    let mut c = f.client();
    let run_id = start(&f, &mut c).await;
    let launch = &f.received("launch")[0]["arguments"];
    assert_eq!(launch["program"], "target/debug/app");
    assert_eq!(launch["cwd"], f.root.to_string_lossy().as_ref());
    assert_eq!(launch["env"], json!({ "APP_MODE": "debug" }));
    stop(&f, &mut c, &run_id).await;
    assert_eq!(
        f.received("disconnect").last().unwrap()["arguments"]["terminateDebuggee"],
        true
    );

    f.config(&f.root, "app", "cwd = \"/\"");
    let e = c.err("debug.start", start_params(&f, "app")).await;
    assert_eq!(e.code, code::INVALID_PARAMS);
}

/// ⑥ 사람의 `[dap.<id>] args` 도 자리 규칙을 지난다 — stdio 어댑터에 `{port}` 를 적으면 띄우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_users_adapter_arguments_follow_the_placeholder_rules() {
    let f = Fixture::new();
    *f.adapters.args.lock().unwrap() = Some(vec!["--port".into(), "{port}".into()]);
    let mut c = f.client();
    let e = c.err("debug.start", start_params(&f, "app")).await;
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert_eq!(e.data.as_ref().unwrap()["reason"], "adapterArgs");
    assert!(f.records().is_empty());
}

// ---- ⑨ runInTerminal → 실행 관리자의 PTY ------------------------------------------------------------------------------------

/// ⑨ 어댑터의 `runInTerminal` 이 실행 관리자의 디버그 실행에 진짜 PTY 프로세스를 만들고, 그 출력이 그 실행의 콘솔(링 버퍼)에 나온다.
/// 어댑터에는 그 pid 가 간다. 대상이 받은 env 는 런타임 환경 위에 어댑터가 준 것이다. 실행을 멈추면 대상의 트리도 끝난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_in_terminal_runs_in_the_debug_runs_console() {
    let f = Fixture::new();
    f.scenario(
        Scenario {
            capabilities: json!({ "supportsConfigurationDoneRequest": true }),
            ..Scenario::default()
        }
        .on(
            "launch",
            vec![Action::Request {
                command: "runInTerminal".into(),
                arguments: json!({
                    "kind": "integrated",
                    "cwd": f.root.to_string_lossy(),
                    "args": ["/bin/sh", "-c", "echo \"hello from $GREETING\"; exec sleep 30"],
                    "env": { "GREETING": "the debuggee" }
                }),
                unless_arg: None,
            }],
        )
        .reply("launch", Reply::default().after(&["configurationDone"])),
    );
    let mut c = f.client();
    let run_id = start(&f, &mut c).await;
    assert!(
        eventually(SOON, || f.launcher.calls.load(Ordering::SeqCst) == 1).await,
        "{:?}",
        f.records()
    );
    let run = f.runs.find(&f.project, &run_id).unwrap();
    assert_eq!(run.info().kind, "debug");
    let text = || run.with_log(|l| String::from_utf8_lossy(&l.read(0, 1 << 20).bytes).into_owned());
    assert!(
        eventually(SOON, || text().contains("hello from the debuggee")).await,
        "console: {:?}",
        text()
    );
    // 어댑터는 띄운 대상의 pid 를 받았다.
    let answer = f
        .records()
        .into_iter()
        .filter_map(|r| r.get("msg").cloned())
        .find(|m| m["type"] == "response" && m["command"] == "runInTerminal")
        .expect("the adapter got an answer");
    assert_eq!(answer["success"], true, "{answer}");
    let pid = answer["body"]["processId"].as_u64().unwrap() as i32;
    assert_eq!(run.info().pid, Some(pid as u32));
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "the debuggee runs");
    stop(&f, &mut c, &run_id).await;
    assert_eq!(run.state(), RunState::Exited);
    assert!(
        eventually(SOON, || unsafe { libc::kill(pid, 0) } != 0).await,
        "the debuggee was stopped with the run"
    );
}

/// 실행을 멈추면 어댑터를 끊고 `debug.*` 가 그 실행을 더 찾지 않는다. 끝난 디버그 세션은 끝난 실행으로 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_the_run_ends_the_debug_session() {
    let f = Fixture::new();
    let mut c = f.client();
    let run_id = start(&f, &mut c).await;
    c.ok("debug.threads", json!({ "runId": run_id })).await;
    stop(&f, &mut c, &run_id).await;
    assert_eq!(f.received("disconnect").len(), 1);
    let e = c.err("debug.threads", json!({ "runId": run_id })).await;
    assert_eq!(e.code, code::NOT_FOUND);
    let listed = c.ok("run.list", json!({ "projectId": f.project })).await;
    assert_eq!(listed["runs"][0]["kind"], "debug");
    assert_eq!(listed["runs"][0]["state"], "exited");
}

/// 어댑터가 시작에 실패하면 실행은 끝난 실행으로 남고 그 콘솔에 까닭이 있다. 오류의 `data.runId` 가 그 실행이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_start_leaves_an_ended_run_with_the_reason() {
    let f = Fixture::new();
    f.scenario(Scenario::default().reply("launch", Reply::error("cannot find program")));
    let mut c = f.client();
    let e = c.err("debug.start", start_params(&f, "app")).await;
    let run_id = e.data.as_ref().unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = f.runs.find(&f.project, &run_id).unwrap();
    assert_eq!(run.state(), RunState::Exited);
    let text = run.with_log(|l| String::from_utf8_lossy(&l.read(0, 1 << 20).bytes).into_owned());
    assert!(text.contains("cannot find program"), "{text}");
}

// ---- Java 의 attach 틀: 언어 서버가 여는 어댑터 ------------------------------------------------------------------------------

/// 언어 서버 흉내: `jdtls` 에 `vscode.java.startDebugSession` 을 받으면 `port` 를 답한다.
struct FakeJdtls {
    port: u16,
    asked: Mutex<Vec<(String, String)>>,
    /// 명령을 받은 (프로젝트, 루트) — 디버그 세션을 연 그 프로젝트 · 체크아웃이어야 한다(M6-19).
    scope: Mutex<Vec<(String, std::path::PathBuf)>>,
}

impl pw_lsp_servers::LspCommands for FakeJdtls {
    fn execute_command<'a>(
        &'a self,
        project_id: &'a str,
        root: &'a std::path::Path,
        server: &'a str,
        command: &'a str,
        _arguments: Vec<Value>,
    ) -> pw_lsp_servers::CommandAnswer<'a> {
        self.asked
            .lock()
            .unwrap()
            .push((server.to_owned(), command.to_owned()));
        self.scope
            .lock()
            .unwrap()
            .push((project_id.to_owned(), root.to_owned()));
        Box::pin(async move { (server == "jdtls").then(|| Ok(json!(self.port))) })
    }
}

/// Java(결정 M6-P20 — attach 만): 언어 서버가 떠 있지 않으면 거절하고(아무것도 띄우지 않는다), 떠 있으면 그 서버에 명령을 보내
/// 답한 `127.0.0.1` 의 포트에 붙어 `attach` 를 보낸다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_language_server_adapter_attaches_through_the_port_it_opens() {
    let f = Fixture::new();
    f.adapters.via_language_server.store(true, Ordering::SeqCst);
    f.config(&f.root, "app", "mode = \"attach\"\nport = 5005");
    let mut c = f.client();
    let e = c.err("debug.start", start_params(&f, "app")).await;
    assert_eq!(
        e.data.as_ref().unwrap()["reason"],
        "languageServerNotRunning"
    );
    assert!(f.runs.list(&f.project).is_empty(), "no run was made");

    // 언어 서버가 연 어댑터 흉내: 가짜 어댑터를 포트로 띄워 둔다.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let scenario = f.adapters.scenario.lock().unwrap().clone();
    let mut adapter = tokio::process::Command::new(env!("CARGO_BIN_EXE_pw-debug-fake-adapter"))
        .arg(&scenario)
        .args(["--port", &port.to_string(), "--host", "127.0.0.1"])
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        eventually(SOON, || std::net::TcpStream::connect(("127.0.0.1", port))
            .is_ok())
        .await,
        "the fake adapter listens"
    );
    let jdtls = Arc::new(FakeJdtls {
        port,
        asked: Mutex::default(),
        scope: Mutex::default(),
    });
    f.debugger
        .attach_language_servers(Arc::clone(&jdtls) as Arc<dyn pw_lsp_servers::LspCommands>);
    let started = c.ok("debug.start", start_params(&f, "app")).await;
    assert_eq!(
        *jdtls.asked.lock().unwrap(),
        [(
            "jdtls".to_owned(),
            "vscode.java.startDebugSession".to_owned()
        )]
    );
    // 명령은 세션을 연 프로젝트 · 그 체크아웃의 루트로 간다(M6-19 — 다른 프로젝트의 jdtls 로 가지 않게).
    assert_eq!(
        *jdtls.scope.lock().unwrap(),
        [(f.project.clone(), f.root.clone())]
    );
    let attach = f.received("attach");
    assert_eq!(attach.last().unwrap()["arguments"], json!({ "port": 5005 }));
    assert!(f.received("launch").is_empty());
    let run_id = started["run"]["runId"].as_str().unwrap().to_owned();
    stop(&f, &mut c, &run_id).await;
    let _ = adapter.kill().await;
}

// ---- 리뷰 F3 · 권고 1(G) -------------------------------------------------------------------------------------------------

/// 리뷰 F3: `runInTerminal` 이 PTY 를 띄우는 동안 디버그 세션이 끝나면(끝내기가 이미 PTY 목록을 본 뒤), 띄운 디버그 대상을 곧바로
/// 끝내고 어댑터에는 실패로 답한다 — 주인 없는 프로세스가 남지 않는다. 띄우기 직전에 실행을 멈추고 그 실행이 끝날 때까지 기다려
/// 끝내기를 끼워 넣는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_program_started_after_the_session_ended_is_stopped() {
    let f = Fixture::new();
    f.scenario(Scenario::default().on(
        "launch",
        vec![Action::Request {
            command: "runInTerminal".into(),
            arguments: json!({
                "cwd": f.root.to_string_lossy(),
                "args": ["/bin/sh", "-c", "exec sleep 30"]
            }),
            unless_arg: None,
        }],
    ));
    let runs = f.runs.clone();
    let project = f.project.clone();
    *f.launcher.before.lock().unwrap() = Some(Box::new(move || {
        // 띄우기 스레드(블로킹) 안: 실행을 멈추고, 끝내기가 실행의 끝을 알릴 때까지 기다린다.
        let run = runs.list(&project).pop().expect("the debug run");
        runs.stop(&project, &run.run_id).unwrap();
        let until = Instant::now() + SOON;
        while runs.find(&project, &run.run_id).unwrap().state() != RunState::Exited {
            assert!(Instant::now() < until, "the debug run did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
    }));
    let mut c = f.client();
    let _ = c.call("debug.start", start_params(&f, "app")).await;
    assert!(eventually(SOON, || !f.launcher.launched.lock().unwrap().is_empty()).await);
    let pid = f.launcher.launched.lock().unwrap()[0].pid().unwrap() as i32;
    assert!(
        eventually(SOON, || unsafe { libc::kill(pid, 0) } != 0).await,
        "the late program {pid} was left running"
    );
    let answered = || {
        f.records()
            .into_iter()
            .filter_map(|r| r.get("msg").cloned())
            .find(|m| m["type"] == "response" && m["command"] == "runInTerminal")
    };
    // 어댑터는 이미 끊겼을 수 있다(답이 닿지 않음) — 닿았다면 실패여야 한다.
    if let Some(answer) = answered() {
        assert_eq!(answer["success"], false, "{answer}");
    }
}

/// 리뷰 권고 1(G): 세션에 예외 필터를 보낼 때 그 세션의 어댑터가 알린 것만 보낸다 — 저장에 알리지 않은 이름이 있어도(손으로 고친
/// 파일 · 다른 판의 어댑터) 걸러진다. 자식 세션(`startDebugging`)이 열릴 때의 다시 보내기로 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_filters_the_session_announced_are_sent() {
    let f = Fixture::new();
    let file = f
        .paths
        .data
        .join("projects")
        .join(&f.project)
        .join("breakpoints.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        r#"{ "schema": 1, "exceptionFilters": { "fake": ["bogus", "uncaught"] } }"#,
    )
    .unwrap();
    f.scenario(
        Scenario {
            capabilities: json!({
                "exceptionBreakpointFilters": [{ "filter": "uncaught", "label": "Uncaught" }]
            }),
            ..Scenario::default()
        }
        .on(
            "threads",
            vec![Action::Request {
                command: "startDebugging".into(),
                arguments: json!({ "configuration": { "child": true }, "request": "launch" }),
                unless_arg: None,
            }],
        ),
    );
    let mut c = f.client();
    let run_id = start(&f, &mut c).await;
    // 시작 뒤에 자식을 연다(어댑터가 `threads` 를 받으면).
    c.ok("debug.threads", json!({ "runId": run_id })).await;
    // 시작 때 하나(pw-dap 이 거름), 자식의 시작 때 하나, 자식이 열린 뒤 다시 보내기 하나 — 모두 알린 것만.
    assert!(
        eventually(SOON, || f.received("setExceptionBreakpoints").len() >= 3).await,
        "{:#?}",
        f.records()
    );
    for sent in f.received("setExceptionBreakpoints") {
        assert_eq!(sent["arguments"]["filters"], json!(["uncaught"]), "{sent}");
    }
    stop(&f, &mut c, &run_id).await;
}

// ---- M6-18 프레임 소스 토큰 -----------------------------------------------------------------------------------------------

/// M6-18 ②: 루트 밖 프레임(일반 파일 · 어댑터가 쥔 소스)에만 토큰이 달리고, 그 토큰으로 이 연결만 `debug.source` 를 읽는다. 에이전트 ·
/// Host 범위는 거절(메인 판정 — 에이전트가 루트 밖 파일을 읽는 길이 되지 않게), 다른 창 연결은 없는 토큰, 실행이 끝나면 토큰이 죽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frame_sources_outside_the_root_are_read_through_tokens() {
    let f = Fixture::new();
    let inside = f.root.join("src/main.rs");
    fs::write(&inside, "fn main() {}\n").unwrap();
    let outside = f.base().join("outside.rs");
    fs::write(&outside, "pub fn lib() {}\n").unwrap();
    f.scenario(
        Scenario::default()
            .reply(
                "stackTrace",
                Reply::body(json!({ "stackFrames": [
                    { "id": 1, "name": "main", "line": 1, "column": 1,
                      "source": { "path": inside.to_str().unwrap() } },
                    { "id": 2, "name": "lib", "line": 1, "column": 1,
                      "source": { "path": outside.to_str().unwrap() } },
                    { "id": 3, "name": "gen", "line": 2, "column": 1,
                      "source": { "name": "gen.js", "sourceReference": 7 } },
                    { "id": 4, "name": "dir", "line": 1, "column": 1,
                      "source": { "path": f.base().to_str().unwrap() } },
                ] })),
            )
            .reply(
                "source",
                Reply::body(json!({ "content": "// generated\n" })),
            ),
    );
    let mut c = f.client();
    let run_id = start(&f, &mut c).await;
    let stack = c
        .ok("debug.stack", json!({ "runId": run_id, "threadId": 1 }))
        .await;
    let frames = stack["frames"].as_array().unwrap();
    assert_eq!(frames[0]["token"], Value::Null, "inside the root");
    assert_eq!(frames[3]["token"], Value::Null, "a folder is not a file");
    let file = frames[1]["token"].as_str().unwrap().to_owned();
    let reference = frames[2]["token"].as_str().unwrap().to_owned();
    assert_eq!(file.len(), 32);

    let read = c
        .ok("debug.source", json!({ "runId": run_id, "token": file }))
        .await;
    assert_eq!(read["text"], "pub fn lib() {}\n");
    assert_eq!(read["path"], outside.to_str().unwrap());
    let read = c
        .ok(
            "debug.source",
            json!({ "runId": run_id, "token": reference }),
        )
        .await;
    assert_eq!(read["text"], "// generated\n");
    assert_eq!(f.received("source")[0]["arguments"]["sourceReference"], 7);

    // 에이전트 · Host 범위는 토큰을 알아도 거절된다.
    for scope in [ConnectionScope::Agent, ConnectionScope::Host] {
        let mut other = f.client_in(scope);
        let e = other
            .err("debug.source", json!({ "runId": run_id, "token": file }))
            .await;
        assert_eq!(e.code, code::PERMISSION_DENIED);
    }
    // 같은 프로젝트의 다른 창 연결은 그 토큰을 받은 적이 없다.
    let mut window = f.client();
    let e = window
        .err("debug.source", json!({ "runId": run_id, "token": file }))
        .await;
    assert_eq!(e.code, code::NOT_FOUND);

    stop(&f, &mut c, &run_id).await;
    let e = c
        .err("debug.source", json!({ "runId": run_id, "token": file }))
        .await;
    assert_eq!(e.code, code::NOT_FOUND);
}
