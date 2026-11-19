//! 디버그 툴(17-m6-plan.md M6-20 완료 기준 ② · ④ · ⑤ · ⑥ · ⑦과 메인 판정 2026-11-19): 툴 메서드를 에이전트 범위 연결로 부른다.
//! 승인은 세션 대신 시험이 한다 — 승인 표 없이 불러 계획(`6001`)을 받고, 그 해시를 실어 다시 부른다. 어댑터는 가짜
//! (`pw-debug-fake-adapter`), 실행 관리자 · 실행 구성 · 레지스트리 · 저장은 진짜다. 목록 · 등급 · 프리셋 · 감사는 데스크톱 크레이트의
//! `mcp_tests::debug` 가 진짜 MCP 세션으로 본다.

// 실행 관리자의 PTY · 가짜 로그인 셸 — unix 만(`tests/debug.rs` 와 같다).
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::plan::APPROVAL_REQUIRED;
use pw_core::registry::{NewProject, Registry};
use pw_dap::Timeouts;
use pw_dap::fake::{Action, Reply, Scenario};
use pw_debug::adapter::Adapters;
use pw_debug::{Breakpoint, Debugger, HostParts};
use pw_ext::{DapTransport, DebugAdapterDecl, NameRef};
use pw_lsp_servers::{AdapterConfig, AdapterLaunch, AdapterSetup, LspResolution, LspResolveStep};
use pw_rpc::{
    Caller, ConnectionScope, EventHub, Id, Outbound, Response, Role, Router, RpcError, Session,
    code,
};
use pw_run::runs::{PtyLauncher, RunLimits, Runs};
use pw_runtime::{LoginShell, Runtime, RuntimeEnv};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

const SOON: Duration = Duration::from_secs(10);

// ---- 바탕(가짜 어댑터 · 프로젝트) — `tests/debug.rs` 의 것을 줄인 것 ------------------------------------------------------

struct FakeAdapters {
    scenario: PathBuf,
}

fn definition(scenario: &Path) -> DebugAdapterDecl {
    DebugAdapterDecl {
        id: "fake".into(),
        pos: None,
        languages: vec![NameRef {
            name: "rust".into(),
            pos: None,
        }],
        mason: None,
        command: Some("fake".into()),
        args: vec![scenario.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        transport: DapTransport::Stdio,
        launch: Some(json!({ "program": "{program}", "cwd": "{cwd}" })),
        attach: None,
        requires: Vec::new(),
        project_local: None,
        start: None,
    }
}

impl Adapters for FakeAdapters {
    fn definitions(&self) -> Vec<DebugAdapterDecl> {
        vec![definition(&self.scenario)]
    }

    fn setup(
        &self,
        _project_id: &str,
        _worktree: Option<&str>,
        _adapter_id: &str,
        _dir: &Path,
    ) -> Result<AdapterSetup, RpcError> {
        let def = definition(&self.scenario);
        Ok(AdapterSetup {
            extension: "dev.example.fake".into(),
            config: AdapterConfig {
                enabled: true,
                path: None,
                args: def.args.clone(),
                env: BTreeMap::new(),
            },
            launch: AdapterLaunch::Process(LspResolution::Found {
                path: PathBuf::from(env!("CARGO_BIN_EXE_pw-debug-fake-adapter")),
                step: LspResolveStep::Path,
                version: None,
                skipped: Vec::new(),
            }),
            env: RuntimeEnv::process(),
            definition: def,
        })
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    projects: Registry,
    runs: Runs,
    debugger: Debugger,
    scenario: PathBuf,
    record: PathBuf,
    project: String,
    root: PathBuf,
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
        let scenario = base.join("scenario.json");
        let debugger = Debugger::with(
            runs.clone(),
            runtime,
            Arc::new(FakeAdapters {
                scenario: scenario.clone(),
            }) as Arc<dyn Adapters>,
            Arc::new(PtyLauncher),
            Timeouts {
                request: Duration::from_secs(5),
                initialize: Duration::from_secs(5),
                initialized: Duration::from_secs(5),
                launch: Duration::from_secs(5),
                connect: Duration::from_secs(5),
                disconnect: Duration::from_secs(1),
                exit_grace: Duration::from_millis(500),
            },
        );
        debugger.attach(HostParts {
            paths,
            projects: projects.clone(),
            bus,
        });
        let mut f = Self {
            _tmp: tmp,
            base: base.clone(),
            projects,
            runs,
            debugger,
            scenario,
            record: base.join("record.jsonl"),
            project: String::new(),
            root: PathBuf::new(),
        };
        let (project, root) = f.project("project");
        f.project = project;
        f.root = root;
        f.scenario(Scenario::default());
        f
    }

    /// 신뢰한 프로젝트 하나(`src/main.rs` · `src/lib.rs` 와 실행 구성 `app` — `[debug]` 가 있다).
    fn project(&self, name: &str) -> (String, PathBuf) {
        let root = self.base.join(name);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        let dir = root.join(".pitwall/run");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("app.toml"),
            "schema = 1\nname = \"app\"\ntype = \"cargo\"\ncommand = \"run\"\n[debug]\nprogram = \"target/debug/app\"\n",
        )
        .unwrap();
        let id = self
            .projects
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        self.projects.set_trust(&id, true).unwrap();
        (id, root)
    }

    fn scenario(&self, mut scenario: Scenario) {
        scenario.record = Some(self.record.clone());
        fs::write(
            &self.scenario,
            serde_json::to_vec_pretty(&scenario).unwrap(),
        )
        .unwrap();
    }

    fn received(&self, command: &str) -> Vec<Value> {
        fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|r| r.get("msg").cloned())
            .filter(|m| m["type"] == "request" && m["command"] == command)
            .collect()
    }

    fn agent(&self) -> Client {
        self.client(ConnectionScope::Agent)
    }

    fn window(&self, project: &str) -> Client {
        self.client(ConnectionScope::Project(project.to_owned()))
    }

    fn client(&self, scope: ConnectionScope) -> Client {
        let mut router = Router::new();
        pw_run::runs::methods::register(&mut router, self.runs.clone());
        pw_debug::methods::register(&mut router, self.debugger.clone());
        let (session, rx) = Session::new(
            Arc::new(router),
            EventHub::new(),
            Caller {
                role: Role::Owner,
                scope,
                window: Some("w".into()),
            },
        );
        Client {
            session,
            rx,
            next_id: 1,
            responses: Vec::new(),
        }
    }

    /// 저장된 그 파일의 브레이크포인트.
    fn stored(&self, project: &str, rel: &str) -> Vec<Breakpoint> {
        self.debugger
            .breakpoints()
            .unwrap()
            .get(project)
            .unwrap()
            .files
            .get(rel)
            .cloned()
            .unwrap_or_default()
    }

    /// 사람(그 프로젝트 창)이 디버그 세션을 연다.
    async fn start(&self, project: &str) -> String {
        let started = self
            .window(project)
            .ok(
                "debug.start",
                json!({ "projectId": project, "configId": "app" }),
            )
            .await;
        started["run"]["runId"].as_str().unwrap().to_owned()
    }

    async fn stop(&self, project: &str, run_id: &str) {
        self.window(project)
            .ok("run.stop", json!({ "runId": run_id }))
            .await;
        let run = self.runs.find(project, run_id).unwrap();
        tokio::time::timeout(SOON, run.exited())
            .await
            .expect("the debug run did not end");
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

    /// 승인 표 없이 불러 계획을 받는다(부작용 없음).
    async fn plan(&mut self, method: &str, params: Value) -> Value {
        let e = self.err(method, params).await;
        assert_eq!(e.code, APPROVAL_REQUIRED, "{e:?}");
        e.data.unwrap()["plan"].clone()
    }

    /// 그 계획의 해시를 실어 다시 부른다(사람이 승인한 것).
    async fn approved(
        &mut self,
        method: &str,
        params: Value,
        plan: &Value,
    ) -> Result<Value, RpcError> {
        let mut params = params;
        params["pitwall/approval"] = json!({ "hash": plan["hash"] });
        self.call(method, params).await
    }

    /// 계획을 받고 곧바로 승인한다.
    async fn set(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let plan = self.plan(method, params.clone()).await;
        self.approved(method, params, &plan).await
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

const SET: &str = "debug.tool.breakpointSet";
const SET_EXPR: &str = "debug.tool.breakpointSetExpr";
const LIST: &str = "debug.tool.breakpointList";
const STATE: &str = "debug.tool.state";

fn conditional(line: u32, enabled: bool) -> Breakpoint {
    Breakpoint {
        enabled,
        condition: Some("n == 2".into()),
        log_message: Some("n={n}".into()),
        ..Breakpoint::at(line)
    }
}

// ---- breakpoint_set ------------------------------------------------------------------------------------------------------

/// 평범한 브레이크포인트: 계획 단계는 아무것도 쓰지 않고(`write` 계획), 승인 뒤 저장 · 표지 · 목록에 보인다. 끄기 · 지우기도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_breakpoint_is_set_turned_off_and_removed_after_approval() {
    let f = Fixture::new();
    let mut c = f.agent();
    let params = json!({ "projectId": f.project, "path": "src/main.rs", "line": 3 });
    let plan = c.plan(SET, params.clone()).await;
    assert_eq!(plan["tier"], "write");
    assert!(
        f.stored(&f.project, "src/main.rs").is_empty(),
        "planning wrote"
    );
    let done = c.approved(SET, params, &plan).await.unwrap();
    assert_eq!(done["path"], "src/main.rs");
    assert_eq!(
        done["breakpoint"],
        json!({ "line": 3, "enabled": true, "condition": null, "logMessage": null, "hitCondition": null, "byAgent": true })
    );
    assert_eq!(
        f.stored(&f.project, "src/main.rs"),
        [Breakpoint {
            agent: true,
            ..Breakpoint::at(3)
        }]
    );
    // 절대 경로도 같은 열쇠다.
    let abs = f.root.join("src/main.rs").display().to_string();
    c.set(
        SET,
        json!({ "projectId": f.project, "path": abs, "line": 3, "enabled": false }),
    )
    .await
    .unwrap();
    assert!(!f.stored(&f.project, "src/main.rs")[0].enabled);
    let listed = c.ok(LIST, json!({ "projectId": f.project })).await;
    assert_eq!(listed["files"][0]["path"], "src/main.rs");
    assert_eq!(listed["files"][0]["file"], json!(abs));
    c.set(
        SET,
        json!({ "projectId": f.project, "path": "src/main.rs", "line": 3, "remove": true }),
    )
    .await
    .unwrap();
    assert!(f.stored(&f.project, "src/main.rs").is_empty());
    let e = c
        .err(
            SET,
            json!({ "projectId": f.project, "path": "src/main.rs", "line": 3, "remove": true }),
        )
        .await;
    assert_eq!(e.code, code::NOT_FOUND);
}

/// ② `breakpoint_set` 으로 식을 넣는 길이 없다: 입력의 `condition` · `logMessage` · `hitCondition` 은 모르는 키로 거절된다(계획도
/// 없다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakpoint_set_takes_no_expressions() {
    let f = Fixture::new();
    let mut c = f.agent();
    for (key, value) in [
        ("condition", "x > 1"),
        ("logMessage", "x={x}"),
        ("hitCondition", "3"),
    ] {
        let e = c
            .err(
                SET,
                json!({ "projectId": f.project, "path": "src/main.rs", "line": 3, key: value }),
            )
            .await;
        assert_eq!(e.code, code::INVALID_PARAMS, "{key}: {e:?}");
    }
    assert!(f.stored(&f.project, "src/main.rs").is_empty());
}

/// 메인 판정 Q3: 식이 있는 브레이크포인트를 `breakpoint_set` 으로 끄면 식은 그대로 남고, 꺼진 것을 켜는 것은 거절이다(승인을 묻기도
/// 전에 — 계획이 없다). 지우기는 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakpoint_set_keeps_expressions_and_cannot_turn_them_on() {
    let f = Fixture::new();
    let store = f.debugger.breakpoints().unwrap();
    store
        .set_file(&f.project, "src/main.rs", vec![conditional(4, true)])
        .unwrap();
    let mut c = f.agent();
    let off = json!({ "projectId": f.project, "path": "src/main.rs", "line": 4, "enabled": false });
    let plan = c.plan(SET, off.clone()).await;
    // 승인 화면에는 그 식이 가려지지 않고 보인다.
    let shown = plan["changes"].to_string();
    assert!(
        shown.contains("n == 2") && shown.contains("n={n}"),
        "{shown}"
    );
    c.approved(SET, off, &plan).await.unwrap();
    assert_eq!(
        f.stored(&f.project, "src/main.rs"),
        [Breakpoint {
            agent: true,
            ..conditional(4, false)
        }],
        "turning it off keeps the expressions"
    );
    let e = c
        .err(
            SET,
            json!({ "projectId": f.project, "path": "src/main.rs", "line": 4 }),
        )
        .await;
    assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    assert_eq!(e.data.as_ref().unwrap()["reason"], "hasExpressions");
    assert!(
        !e.message.contains("n == 2"),
        "errors do not quote the expression"
    );
    assert!(!f.stored(&f.project, "src/main.rs")[0].enabled);
    c.set(
        SET,
        json!({ "projectId": f.project, "path": "src/main.rs", "line": 4, "remove": true }),
    )
    .await
    .unwrap();
    assert!(f.stored(&f.project, "src/main.rs").is_empty());

    // 식이 하나뿐인 꺼진 것도 켜지 못한다 — hit count 만, 로그 메시지만.
    let hit_only = Breakpoint {
        enabled: false,
        hit_condition: Some(">= 3".into()),
        ..Breakpoint::at(7)
    };
    let log_only = Breakpoint {
        enabled: false,
        log_message: Some("here".into()),
        ..Breakpoint::at(9)
    };
    store
        .set_file(
            &f.project,
            "src/lib.rs",
            vec![hit_only.clone(), log_only.clone()],
        )
        .unwrap();
    for line in [7, 9] {
        let e = c
            .err(
                SET,
                json!({ "projectId": f.project, "path": "src/lib.rs", "line": line }),
            )
            .await;
        assert_eq!(
            e.data.as_ref().unwrap()["reason"],
            "hasExpressions",
            "line {line}: {e:?}"
        );
    }
    assert_eq!(f.stored(&f.project, "src/lib.rs"), [hit_only, log_only]);
}

/// 메인 판정 F1: 미신뢰 프로젝트에서 `breakpoint_set_expr` 는 묻기 전에 거절이고(`untrusted`, 계획 없음, 저장 없음),
/// `breakpoint_set` 은 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_takes_plain_breakpoints_but_no_expressions() {
    let f = Fixture::new();
    f.projects.set_trust(&f.project, false).unwrap();
    let mut c = f.agent();
    let e = c
        .err(
            SET_EXPR,
            json!({ "projectId": f.project, "path": "src/main.rs", "line": 2, "condition": "x > 1" }),
        )
        .await;
    assert_eq!(e.code, code::PERMISSION_DENIED, "{e:?}");
    assert_eq!(e.data.as_ref().unwrap()["reason"], "untrusted");
    assert!(f.stored(&f.project, "src/main.rs").is_empty());
    c.set(
        SET,
        json!({ "projectId": f.project, "path": "src/main.rs", "line": 2 }),
    )
    .await
    .unwrap();
    assert_eq!(f.stored(&f.project, "src/main.rs").len(), 1);
}

/// 승인 뒤에는 계획 때 본 그 줄일 때만 바꾼다: 승인을 기다리는 동안 사람이 그 줄에 조건을 넣었으면 돌지 않고 사람의 것이 남는다
/// (덮어쓰거나 조건을 지우지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_line_changed_after_planning_is_not_overwritten() {
    let f = Fixture::new();
    let mut c = f.agent();
    let params = json!({ "projectId": f.project, "path": "src/main.rs", "line": 4 });
    let plan = c.plan(SET, params.clone()).await;
    f.window(&f.project)
        .ok(
            "debug.breakpoints.set",
            json!({ "projectId": f.project, "path": "src/main.rs",
                    "breakpoints": [{ "line": 4, "condition": "n == 2", "logMessage": "n={n}" }] }),
        )
        .await;
    // 다시 만든 계획(그 줄의 지금 것이 해시에 든다)이 승인한 것과 달라 돌지 않는다 — 세션은 다시 묻지 않고 `plan_changed` 로
    // 거절한다. 승인과 쓰기 사이의 바뀜은 `Breakpoints::replace_line` 이 막는다(단위 시험).
    let e = c.approved(SET, params, &plan).await.unwrap_err();
    assert_eq!(e.code, APPROVAL_REQUIRED, "{e:?}");
    assert_eq!(e.data.as_ref().unwrap()["mismatch"], true);
    assert_eq!(
        f.stored(&f.project, "src/main.rs"),
        [conditional(4, true)],
        "the person's breakpoint is kept as it was"
    );
}

/// ④ 루트 밖 경로는 두 툴 모두 거절이다 — `..`, 루트 밖 절대 경로, 루트 밖을 가리키는 링크, 다른 프로젝트의 파일. 계획도 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paths_outside_the_project_are_refused() {
    let f = Fixture::new();
    let (other, other_root) = f.project("other");
    fs::write(f.base.join("outside.rs"), "x\n").unwrap();
    std::os::unix::fs::symlink(f.base.join("outside.rs"), f.root.join("src/link.rs")).unwrap();
    let mut c = f.agent();
    let outside = f.base.join("outside.rs").display().to_string();
    let others = other_root.join("src/main.rs").display().to_string();
    for path in [
        "../outside.rs",
        outside.as_str(),
        "src/link.rs",
        others.as_str(),
    ] {
        for (method, extra) in [(SET, json!({})), (SET_EXPR, json!({ "condition": "true" }))] {
            let mut params = json!({ "projectId": f.project, "path": path, "line": 1 });
            params
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let e = c.err(method, params).await;
            assert_eq!(e.code, code::INVALID_PARAMS, "{method} {path}: {e:?}");
            assert_eq!(
                e.data.as_ref().unwrap()["reason"],
                "outsideProject",
                "{method} {path}"
            );
        }
    }
    assert!(
        f.debugger
            .breakpoints()
            .unwrap()
            .get(&f.project)
            .unwrap()
            .files
            .is_empty()
    );
    assert!(
        f.debugger
            .breakpoints()
            .unwrap()
            .get(&other)
            .unwrap()
            .files
            .is_empty()
    );
}

// ---- breakpoint_set_expr -------------------------------------------------------------------------------------------------

/// `breakpoint_set_expr`: 계획은 `exec` 이고 식을 하나씩 그대로 보이며, 세션 허용은 그 식의 해시에만 닿는다. 승인 뒤 저장되고
/// 표지가 달리며, 열린 디버그 세션에 곧바로 간다. 식이 없으면 `breakpoint_set` 으로 안내한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expression_breakpoint_is_exec_and_reaches_the_open_session() {
    let f = Fixture::new();
    let run_id = f.start(&f.project).await;
    let mut c = f.agent();
    let params = json!({
        "projectId": f.project, "path": "src/lib.rs", "line": 1,
        "condition": "secret_token.len() > 3", "hitCondition": ">= 2"
    });
    let plan = c.plan(SET_EXPR, params.clone()).await;
    assert_eq!(plan["tier"], "exec");
    let labels: Vec<(&str, &str)> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["label"].as_str().unwrap(), c["text"].as_str().unwrap()))
        .collect();
    assert!(
        labels.contains(&("Condition", "secret_token.len() > 3")),
        "{labels:?}"
    );
    assert!(labels.contains(&("Hit count", ">= 2")), "{labels:?}");
    assert_eq!(plan["rules"]["remember"]["kind"], "key");
    let key = plan["rules"]["remember"]["key"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!key.contains("secret_token"), "the key is a hash: {key}");
    assert!(
        f.received("setBreakpoints").is_empty(),
        "planning sent nothing"
    );

    c.approved(SET_EXPR, params, &plan).await.unwrap();
    assert_eq!(
        f.stored(&f.project, "src/lib.rs"),
        [Breakpoint {
            condition: Some("secret_token.len() > 3".into()),
            hit_condition: Some(">= 2".into()),
            agent: true,
            ..Breakpoint::at(1)
        }]
    );
    assert!(eventually(SOON, || !f.received("setBreakpoints").is_empty()).await);
    assert_eq!(
        f.received("setBreakpoints")[0]["arguments"]["breakpoints"],
        json!([{ "line": 1, "condition": "secret_token.len() > 3", "hitCondition": ">= 2" }])
    );
    // 다른 식은 다른 열쇠다.
    let other = c
        .plan(
            SET_EXPR,
            json!({ "projectId": f.project, "path": "src/lib.rs", "line": 1, "condition": "x" }),
        )
        .await;
    assert_ne!(other["rules"]["remember"]["key"], json!(key));
    let e = c
        .err(
            SET_EXPR,
            json!({ "projectId": f.project, "path": "src/lib.rs", "line": 1 }),
        )
        .await;
    assert_eq!(e.code, code::INVALID_PARAMS);
    f.stop(&f.project, &run_id).await;
}

/// 메인 판정 Q2: 사람이 그 파일을 통째로 다시 저장해도(화면은 `agent` 를 되돌려 보내지 않는다) 같은 줄 · 같은 내용이면 표지가
/// 남고, 사람이 그 줄을 바꾸면 사람 것이 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_mark_survives_a_window_save_until_the_person_changes_the_line() {
    let f = Fixture::new();
    let mut c = f.agent();
    c.set(
        SET,
        json!({ "projectId": f.project, "path": "src/main.rs", "line": 2 }),
    )
    .await
    .unwrap();
    let mut w = f.window(&f.project);
    w.ok(
        "debug.breakpoints.set",
        json!({ "projectId": f.project, "path": "src/main.rs",
                "breakpoints": [{ "line": 2 }, { "line": 7 }] }),
    )
    .await;
    let marks: Vec<(u32, bool)> = f
        .stored(&f.project, "src/main.rs")
        .iter()
        .map(|b| (b.line, b.agent))
        .collect();
    assert_eq!(marks, [(2, true), (7, false)]);
    w.ok(
        "debug.breakpoints.set",
        json!({ "projectId": f.project, "path": "src/main.rs",
                "breakpoints": [{ "line": 2, "enabled": false }] }),
    )
    .await;
    assert!(
        !f.stored(&f.project, "src/main.rs")[0].agent,
        "the person changed it"
    );
}

/// 툴은 MCP 세션의 연결만 받는다 — 프로젝트 창 · Host 연결은 `agent_grant` 가 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_agent_connections_call_the_tools() {
    let f = Fixture::new();
    for scope in [
        ConnectionScope::Project(f.project.clone()),
        ConnectionScope::Host,
    ] {
        let mut c = f.client(scope.clone());
        for (method, params) in [
            (LIST, json!({ "projectId": f.project })),
            (
                SET,
                json!({ "projectId": f.project, "path": "src/main.rs", "line": 1 }),
            ),
            (STATE, json!({ "projectId": f.project })),
        ] {
            let e = c.err(method, params).await;
            assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?} {method}: {e:?}");
        }
    }
}

// ---- debug_state ---------------------------------------------------------------------------------------------------------

/// 멈춘 세션의 대본: `pause` 를 받으면 멈추고 `continue` 를 받으면 다시 돈다. 스레드 둘, 프레임 30, 영역 둘(하나는 비싸다),
/// 변수는 512 자를 넘는 값 · 펼칠 수 있는 것 · 긴 이름.
fn stopping_scenario() -> Scenario {
    let frames: Vec<Value> = (0..30)
        .map(|i| {
            json!({ "id": 100 + i, "name": format!("frame{i}"), "line": i + 1, "column": 1,
                          "source": { "path": "/x/main.rs" } })
        })
        .collect();
    Scenario::default()
        .reply(
            "threads",
            Reply::body(json!({ "threads": [{ "id": 1, "name": "main" }, { "id": 2, "name": "worker" }] })),
        )
        .reply(
            "stackTrace",
            Reply::body(json!({ "stackFrames": frames, "totalFrames": 30 })),
        )
        .reply(
            "scopes",
            Reply::body(json!({ "scopes": [
                { "name": "Locals", "variablesReference": 5, "expensive": false },
                { "name": "Registers", "variablesReference": 6, "expensive": true }
            ]})),
        )
        .reply(
            "variables",
            Reply::body(json!({ "variables": [
                { "name": "token", "value": "v".repeat(600), "type": "String", "variablesReference": 0 },
                { "name": "nested", "value": "Point", "variablesReference": 9 }
            ]})),
        )
        .on(
            "pause",
            vec![Action::Event {
                event: "stopped".into(),
                body: json!({ "reason": "pause", "threadId": 1, "allThreadsStopped": true }),
            }],
        )
        .on(
            "threads",
            vec![Action::Event {
                event: "continued".into(),
                body: json!({ "threadId": 1, "allThreadsContinued": true }),
            }],
        )
        .on(
            "continue",
            vec![Action::Event {
                event: "continued".into(),
                body: json!({ "threadId": 1, "allThreadsContinued": true }),
            }],
        )
}

async fn pause(f: &Fixture, run_id: &str) {
    f.window(&f.project)
        .ok(
            "debug.control",
            json!({ "runId": run_id, "threadId": 1, "action": "pause" }),
        )
        .await;
    let run = f.debugger.find(&f.project, run_id).unwrap();
    assert!(
        eventually(SOON, || !run.stops().is_empty()).await,
        "never stopped"
    );
}

/// ⑥ 세션이 없으면 오류가 아니라 「세션 없음」, 멈추지 않았으면 「실행 중」 만(어댑터에 아무것도 묻지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_session_or_while_running_the_state_says_so() {
    let f = Fixture::new();
    f.scenario(stopping_scenario());
    let mut c = f.agent();
    let none = c.ok(STATE, json!({ "projectId": f.project })).await;
    assert_eq!(none["state"], "noSession");
    assert_eq!(none["sessionId"], Value::Null);
    let none = c
        .ok(
            STATE,
            json!({ "projectId": f.project, "sessionId": "run-nope" }),
        )
        .await;
    assert_eq!(none["state"], "noSession");

    let run_id = f.start(&f.project).await;
    let running = c.ok(STATE, json!({ "projectId": f.project })).await;
    assert_eq!(running["state"], "running");
    assert_eq!(running["sessionId"], json!(run_id));
    assert_eq!(running["frames"], json!([]));
    assert!(f.received("threads").is_empty() && f.received("stackTrace").is_empty());

    // 멈췄다가 사람이 계속하면 다시 「실행 중」.
    pause(&f, &run_id).await;
    let stopped = c.ok(STATE, json!({ "projectId": f.project })).await;
    assert_eq!(stopped["state"], "stopped");
    f.window(&f.project)
        .ok(
            "debug.control",
            json!({ "runId": run_id, "threadId": 1, "action": "continue" }),
        )
        .await;
    let run = f.debugger.find(&f.project, &run_id).unwrap();
    assert!(eventually(SOON, || run.stops().is_empty()).await);
    let again = c.ok(STATE, json!({ "projectId": f.project })).await;
    assert_eq!(again["state"], "running");

    // 어댑터의 `continued` 이벤트만으로도(사람의 요청 없이 — 여기서는 `threads` 에 딸려 온다) 다시 돈다.
    pause(&f, &run_id).await;
    f.window(&f.project)
        .ok("debug.threads", json!({ "runId": run_id }))
        .await;
    assert!(
        eventually(SOON, || run.stops().is_empty()).await,
        "the continued event was not seen"
    );

    // 어댑터가 `continued` 를 알리지 않는 스텝(DAP 는 요청의 답으로 갈음한다)도 사람의 스텝이 지운다.
    pause(&f, &run_id).await;
    f.window(&f.project)
        .ok(
            "debug.control",
            json!({ "runId": run_id, "threadId": 1, "action": "next" }),
        )
        .await;
    let stepped = c.ok(STATE, json!({ "projectId": f.project })).await;
    assert_eq!(stepped["state"], "running");

    f.stop(&f.project, &run_id).await;
    let ended = c
        .ok(
            STATE,
            json!({ "projectId": f.project, "sessionId": run_id }),
        )
        .await;
    assert_eq!(ended["state"], "noSession", "an ended session is gone");
}

/// ⑤ 멈춘 세션의 상한: 프레임은 위 20, 변수는 맨 위 프레임의 영역 한 단계(펼칠 수 있는 것을 읽지 않고 비싼 영역도 읽지 않는다),
/// 글은 512 자. 프레임에는 창 전용 칸(`token` 등)이 없다 — 이 툴의 프레임 칸은 정확히 넷이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_session_is_read_one_level_within_the_limits() {
    let f = Fixture::new();
    f.scenario(stopping_scenario());
    let run_id = f.start(&f.project).await;
    pause(&f, &run_id).await;
    let mut c = f.agent();
    let state = c.ok(STATE, json!({ "projectId": f.project })).await;
    assert_eq!(state["state"], "stopped");
    assert_eq!(state["stop"]["reason"], "pause");
    assert_eq!(state["threads"].as_array().unwrap().len(), 2);
    let frames = state["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 20);
    assert_eq!(frames[0]["name"], "frame0");
    let keys: Vec<&str> = frames[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["column", "line", "name", "path"]);
    assert_eq!(f.received("stackTrace")[0]["arguments"]["levels"], 20);
    // 한 단계: 영역 `Locals`(5) 만 읽고, 펼칠 수 있는 `nested`(9) 와 비싼 `Registers`(6) 는 읽지 않는다.
    let asked: Vec<Value> = f
        .received("variables")
        .iter()
        .map(|r| r["arguments"]["variablesReference"].clone())
        .collect();
    assert_eq!(asked, [json!(5)]);
    let locals = &state["locals"];
    assert_eq!(locals[1]["name"], "Registers");
    assert_eq!(locals[1]["skipped"], true);
    let token = &locals[0]["variables"][0];
    assert_eq!(token["value"].as_str().unwrap().chars().count(), 513);
    assert_eq!(token["clipped"], true);
    assert_eq!(locals[0]["variables"][1]["expandable"], true);
    assert!(serde_json::to_vec(&state).unwrap().len() <= pw_debug::tool::STATE_BUDGET);
    f.stop(&f.project, &run_id).await;
}

/// ⑦ 다른 프로젝트의 세션은 보이지 않는다: 그 `sessionId` 를 주어도, 주지 않아도 「세션 없음」 이고 그 어댑터에 아무것도 묻지
/// 않는다(프로젝트 판정을 걷으면 실패).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_projects_session_is_no_session() {
    let f = Fixture::new();
    f.scenario(stopping_scenario());
    let (other, _) = f.project("other");
    let theirs = f.start(&other).await;
    f.window(&other)
        .ok(
            "debug.control",
            json!({ "runId": theirs, "threadId": 1, "action": "pause" }),
        )
        .await;
    let run = f.debugger.find(&other, &theirs).unwrap();
    assert!(eventually(SOON, || !run.stops().is_empty()).await);

    let mut c = f.agent();
    for params in [
        json!({ "projectId": f.project, "sessionId": theirs }),
        json!({ "projectId": f.project }),
    ] {
        let state = c.ok(STATE, params.clone()).await;
        assert_eq!(state["state"], "noSession", "{params}");
        assert_eq!(state["otherSessions"], json!([]), "{params}");
    }
    assert!(
        f.received("threads").is_empty(),
        "the other project's adapter was asked"
    );
    // 그 프로젝트로 부르면 보인다(대조).
    let mine = c.ok(STATE, json!({ "projectId": other })).await;
    assert_eq!(mine["state"], "stopped");
    f.stop(&other, &theirs).await;
}
