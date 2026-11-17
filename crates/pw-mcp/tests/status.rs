//! MCP 상태 RPC(16-m5-plan.md M5-10): 세션 등록부와 `mcp.sessions` · `mcp.sessionsChanged`, `mcp.audit.list` 의 쪽 넘기기 ·
//! 필터와 `mcp.activity`, `mcp.tools.list` 의 노출 판정(「꺼진 까닭」 다섯). RPC 는 창과 같은 길(Host 연결)로 부른다.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{Filter, HostRouter, LocalHost, Received, Scope, Target};
use pw_mcp::audit::{AuditLog, Entry, Outcome};
use pw_mcp::methods::{self, HostParts, Mcp};
use pw_mcp::policy::Approval;
use pw_mcp::sessions::{Sessions, TOPIC_ACTIVITY, TOPIC_SESSIONS_CHANGED};
use pw_mcp::{CallError, Catalog, HostClient, HostKind, Services, SessionRequest};
use pw_rpc::{Caller, Role, RpcError, rpc_method};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// 입력 없는 쓰기 툴의 입력(쓰기 툴은 `Planned` struct 입력이어야 한다 — 카탈로그 규칙).
#[derive(Debug, Clone, serde::Deserialize, specta::Type, schemars::JsonSchema)]
pub struct NoInput {}

rpc_method! {
    /// 시험용: `[mcp.tools]` 의 정확한 이름 키가 끈다.
    pub ProbeExact: "mcptest.probeExact" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "project", name: "probe_exact" },
    }

    /// 시험용: `[mcp.tools]` 의 glob 키가 끈다.
    pub ProbeGlob: "mcptest.probeGlob" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "project", name: "probe_glob" },
    }

    /// 시험용: `[mcp.groups]` 의 db 가 끈다.
    pub ProbeDb: "mcptest.probeDb" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "db", name: "probe_db" },
    }

    /// 시험용: 프리셋 read-only 가 끈다(write 등급).
    pub ProbeWrite: "mcptest.probeWrite" {
        params: pw_core::registry::plan::Planned<NoInput>,
        result: (),
        tier: Write,
        mcp: { group: "project", name: "probe_write" },
    }

    /// 시험용: 공유 `.pitwall/project.toml` 이 끈다.
    pub ProbeShared: "mcptest.probeShared" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "project", name: "probe_shared" },
    }

    /// 시험용: 켜진 툴(프리셋).
    pub ProbeOn: "mcptest.probeOn" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "project", name: "probe_on" },
    }
}

const GLOBAL: &str = r#"
[mcp]
preset = "read-only"
[mcp.tools]
probe_exact = false
"probe_g*" = false
[mcp.groups]
db = false
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    paths: AppPaths,
    host: Arc<LocalHost>,
    sessions: Sessions,
    alpha: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("settings.toml"),
            format!("schema = 1\n{GLOBAL}"),
        )
        .unwrap();
        let sessions = Sessions::new();
        let mcp = Arc::new(Mcp::new(sessions.clone()));
        let mcp_methods = Arc::clone(&mcp);
        let host = Arc::new(
            LocalHost::builder("local")
                .config(paths.clone())
                .methods(|router| {
                    let ok = |_ctx, ()| async { Ok::<(), RpcError>(()) };
                    router.unary(ProbeExact, ok);
                    router.unary(ProbeGlob, ok);
                    router.unary(ProbeDb, ok);
                    router.unary(
                        ProbeWrite,
                        |_ctx, _p: pw_core::registry::plan::Planned<NoInput>| async {
                            Ok::<(), RpcError>(())
                        },
                    );
                    router.unary(ProbeShared, ok);
                    router.unary(ProbeOn, ok);
                })
                .methods(move |router| methods::register(router, mcp_methods))
                .build(),
        );
        let root = base.join("work/alpha");
        std::fs::create_dir_all(root.join(".pitwall")).unwrap();
        std::fs::write(
            root.join(".pitwall/project.toml"),
            "schema = 1\n[mcp.tools]\nprobe_shared = false\n",
        )
        .unwrap();
        let alpha = host
            .registry()
            .unwrap()
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap()
            .id;
        let config = host.config().unwrap();
        sessions.attach(host.bus().clone());
        mcp.attach(HostParts {
            paths: paths.clone(),
            global: config.global.clone(),
            projects: host.registry().unwrap().clone(),
            catalog: Arc::new(Catalog::linked().routed_by(host.router())),
            audit: Arc::new(AuditLog::under_state(&paths.state)),
        });
        Self {
            _tmp: tmp,
            base,
            paths,
            host,
            sessions,
            alpha,
        }
    }

    fn services(&self) -> Arc<Services> {
        let router = Arc::new(HostRouter::new());
        router.add_host(self.host.clone()).unwrap();
        Arc::new(Services {
            router,
            host_id: "local".into(),
            paths: self.paths.clone(),
            global: self.host.config().unwrap().global.clone(),
            bus: self.host.bus().clone(),
            audit: Arc::new(AuditLog::under_state(&self.paths.state)),
            catalog: Arc::new(Catalog::linked().routed_by(self.host.router())),
            approval: Approval::Unavailable,
            transport: "test",
            host_kind: HostKind::App,
            sessions: self.sessions.clone(),
        })
    }

    /// MCP 세션 하나(세션 프로젝트 `alpha`).
    async fn connect(&self) -> (RunningService<RoleClient, ()>, CancellationToken) {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let (r, w) = tokio::io::split(server_io);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            self.services(),
            SessionRequest {
                project: None,
                cwd: Some(self.base.join("work/alpha")),
            },
            (r, w),
            cancel.clone(),
        ));
        let client = ().serve(client_io).await.unwrap();
        (client, cancel)
    }

    /// 창과 같은 연결(owner).
    async fn window(&self) -> HostClient {
        self.window_as(Caller::new(Role::Owner)).await
    }

    async fn window_as(&self, caller: Caller) -> HostClient {
        let router = HostRouter::new();
        router.add_host(self.host.clone()).unwrap();
        HostClient::new(
            router
                .connect(&Target::HostId("local".into()), caller)
                .await
                .unwrap(),
        )
    }

    fn entry(&self, tool: &str, project: Option<&str>, outcome: Outcome) -> Entry {
        Entry {
            session: "s".into(),
            client: Some("probe 1".into()),
            project_id: project.map(str::to_owned),
            tool: tool.into(),
            args: "{}".into(),
            outcome,
            detail: None,
            result_size: None,
            duration: Duration::from_millis(1),
            approval: None,
            plan_hash: None,
        }
    }
}

async fn sessions(window: &HostClient) -> Vec<Value> {
    match window.call("mcp.sessions", Value::Null).await.unwrap() {
        Value::Array(list) => list,
        other => panic!("{other}"),
    }
}

/// `mcp.sessions` 가 `n` 개가 될 때까지 기다린다 — 등록은 서버가 초기화를 마친 뒤에, 해제는 세션 태스크가 끝날 때 일어난다.
async fn sessions_reach(window: &HostClient, n: usize) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let list = sessions(window).await;
        if list.len() == n {
            return list;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "sessions did not reach {n}: {list:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn payloads(events: &mut pw_core::Subscription) -> Vec<Value> {
    std::iter::from_fn(|| events.try_recv())
        .map(|r| match r {
            Received::Event(e) => e.payload.clone(),
            Received::Lagged(n) => panic!("lagged {n}"),
        })
        .collect()
}

/// 툴을 부른다. 노출되지 않은 툴은 JSON-RPC 오류로 끝난다 — 그것도 호출이다(감사 행이 남는다).
async fn call(client: &RunningService<RoleClient, ()>, name: &str) {
    let _ = client
        .peer()
        .call_tool(CallToolRequestParams::new(name.to_owned()))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_and_closing_sessions_changes_the_list_and_publishes_once_each() {
    let f = Fixture::new();
    let mut events = f.host.bus().subscribe(
        Filter::all()
            .scope(Scope::Host)
            .topics([TOPIC_SESSIONS_CHANGED]),
    );
    let window = f.window().await;
    assert!(sessions(&window).await.is_empty());

    let (first, first_cancel) = f.connect().await;
    let list = sessions_reach(&window, 1).await;
    assert_eq!(list[0]["projectId"], json!(f.alpha));
    assert_eq!(list[0]["transport"], "test");
    assert_eq!(list[0]["calls"], 0);
    // rmcp 의 기본 clientInfo — initialize 로 안 이름이 등록 때 실린다.
    let client = list[0]["client"].as_str().unwrap().to_owned();
    assert!(client.starts_with("rmcp "), "{client}");
    assert!(list[0]["startedAt"].as_i64().unwrap() > 0);

    call(&first, "probe_on").await;
    call(&first, "probe_exact").await; // 거절된 호출도 호출이다.
    assert_eq!(sessions(&window).await[0]["calls"], 2);

    let (second, second_cancel) = f.connect().await;
    let list = sessions_reach(&window, 2).await;
    assert_ne!(list[0]["id"], list[1]["id"]);

    first.cancel().await.unwrap();
    first_cancel.cancel();
    sessions_reach(&window, 1).await;
    second.cancel().await.unwrap();
    second_cancel.cancel();
    sessions_reach(&window, 0).await;

    // 늦게 오는 이벤트가 없는지 잠깐 더 본다.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let totals: Vec<Value> = payloads(&mut events);
    assert_eq!(
        totals,
        [
            json!({"total": 1}),
            json!({"total": 2}),
            json!({"total": 1}),
            json!({"total": 0})
        ],
        "열고 닫을 때 한 번씩 — 호출은 이벤트를 내지 않는다"
    );
}

/// 감사 행이 쓰이면 `mcp.activity` 가 그 프로젝트 범위로 나간다(행 id 와 프로젝트만 싣는다). 프로젝트 없는 호출은 Host 범위.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_written_audit_row_publishes_activity_in_its_project_scope() {
    let f = Fixture::new();
    let mut project = f.host.bus().subscribe(
        Filter::all()
            .scope(Scope::Project(f.alpha.clone()))
            .topics([TOPIC_ACTIVITY]),
    );
    let mut host = f
        .host
        .bus()
        .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_ACTIVITY]));
    let (client, cancel) = f.connect().await;
    // `project_info` 는 프로젝트 툴이라 세션 프로젝트가 채워진다. `project_list` 는 프로젝트가 없다.
    call(&client, "project_info").await;
    call(&client, "project_list").await;
    client.cancel().await.unwrap();
    cancel.cancel();
    let window = f.window().await;
    sessions_reach(&window, 0).await; // 세션이 끝날 때 감사 기록이 다 쓰인다.

    let page = window.call("mcp.audit.list", json!({})).await.unwrap();
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let (newest, oldest) = (&entries[0], &entries[1]);
    assert_eq!(newest["tool"], "project_list");
    assert_eq!(newest["projectId"], Value::Null);
    assert_eq!(oldest["tool"], "project_info");
    assert_eq!(oldest["projectId"], json!(f.alpha));
    assert_eq!(
        payloads(&mut project),
        [json!({"id": oldest["id"], "projectId": f.alpha})]
    );
    assert_eq!(
        payloads(&mut host),
        [json!({"id": newest["id"], "projectId": null})]
    );
}

/// 감사 쓰기가 실패하면 `mcp.activity` 를 내지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_audit_write_publishes_no_activity() {
    let f = Fixture::new();
    // 감사 폴더 자리에 파일을 두어 열지 못하게 한다.
    std::fs::create_dir_all(&f.paths.state).unwrap();
    std::fs::write(f.paths.state.join("mcp"), "not a directory").unwrap();
    let mut events = f
        .host
        .bus()
        .subscribe(Filter::all().topics([TOPIC_ACTIVITY]));
    let (client, cancel) = f.connect().await;
    call(&client, "probe_on").await;
    client.cancel().await.unwrap();
    cancel.cancel();
    sessions_reach(&f.window().await, 0).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(payloads(&mut events).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_list_pages_newest_first_without_overlap_and_filters() {
    let f = Fixture::new();
    let log = AuditLog::under_state(&f.paths.state);
    let mut written = Vec::new();
    for i in 0..1200 {
        let (tool, project, outcome) = match i % 4 {
            0 => ("probe_on", Some("p1"), Outcome::Ok),
            1 => ("probe_on", None, Outcome::Error),
            2 => ("probe_exact", Some("p2"), Outcome::Denied),
            _ => ("project_list", Some("p1"), Outcome::Ok),
        };
        written.push(log.record(&f.entry(tool, project, outcome)).unwrap());
    }
    let window = f.window().await;
    let page = |params: Value| {
        let window = &window;
        async move { window.call("mcp.audit.list", params).await }
    };

    // limit 500 → 500 · 500 · 200, 최신부터, 겹침 없음.
    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    let mut sizes = Vec::new();
    loop {
        let result = page(json!({"limit": 500, "cursor": cursor})).await.unwrap();
        let ids: Vec<i64> = result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_i64().unwrap())
            .collect();
        sizes.push(ids.len());
        seen.extend(ids);
        cursor = result["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(sizes, [500, 500, 200]);
    let mut newest_first = written.clone();
    newest_first.reverse();
    assert_eq!(seen, newest_first, "최신부터 · 겹침 없음 · 빠짐 없음");

    // 기본 limit 은 100.
    let first = page(json!({})).await.unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), 100);

    // 필터: 결과 · 툴 · 프로젝트(그 프로젝트와 프로젝트 없는 행).
    let all = |result: Value| result["entries"].as_array().unwrap().clone();
    let denied = all(page(json!({"outcome": "denied", "limit": 500}))
        .await
        .unwrap());
    assert_eq!(denied.len(), 300);
    assert!(
        denied
            .iter()
            .all(|e| e["outcome"] == "denied" && e["tool"] == "probe_exact")
    );
    let listed = all(page(json!({"tool": "project_list", "limit": 500}))
        .await
        .unwrap());
    assert_eq!(listed.len(), 300);
    assert!(listed.iter().all(|e| e["tool"] == "project_list"));
    let p1 = page(json!({"projectId": "p1", "limit": 500}))
        .await
        .unwrap();
    let p1_rows = all(p1.clone());
    assert_eq!(p1_rows.len(), 500);
    assert!(!p1["nextCursor"].is_null());
    assert!(
        p1_rows
            .iter()
            .all(|e| e["projectId"] == "p1" || e["projectId"].is_null()),
        "다른 프로젝트의 행은 없다"
    );
    let combined = all(page(
        json!({"projectId": "p1", "outcome": "error", "tool": "probe_on", "limit": 500}),
    )
    .await
    .unwrap());
    assert_eq!(combined.len(), 300);
    assert!(combined.iter().all(|e| e["projectId"].is_null()));
    let row = &combined[0];
    assert_eq!(row["client"], "probe 1");
    assert_eq!(row["session"], "s");
    assert!(row["at"].as_i64().unwrap() > 0);

    // 범위 밖 limit · 틀린 커서는 invalid params.
    for bad in [
        json!({"limit": 501}),
        json!({"limit": 0}),
        json!({"cursor": "x"}),
    ] {
        match page(bad.clone()).await {
            Err(CallError::Rpc { code, .. }) => {
                assert_eq!(code, i64::from(pw_rpc::code::INVALID_PARAMS), "{bad}");
            }
            other => panic!("{bad}: {other:?}"),
        }
    }
}

/// 감사 파일이 아직 없으면 빈 쪽이고 파일을 만들지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_list_without_a_file_is_empty() {
    let f = Fixture::new();
    let result = f
        .window()
        .await
        .call("mcp.audit.list", json!({}))
        .await
        .unwrap();
    assert_eq!(result, json!({"entries": [], "nextCursor": null}));
    assert!(!f.paths.state.join("mcp/audit.db").exists());
}

fn tool<'a>(list: &'a [Value], name: &str) -> &'a Value {
    list.iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("{name} not in {list:?}"))
}

/// 「꺼진 까닭」 다섯: 프리셋 · 툴 키 · glob · 그룹 · 공유 설정이 좁힘. 판정은 세션과 같은 `Exposure::verdict` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_list_reports_why_each_tool_is_on_or_off() {
    let f = Fixture::new();
    let log = AuditLog::under_state(&f.paths.state);
    for _ in 0..3 {
        log.record(&f.entry("probe_on", Some(&f.alpha), Outcome::Ok))
            .unwrap();
    }
    log.record(&f.entry("probe_on", None, Outcome::Ok)).unwrap();
    let window = f.window().await;

    let global = window.call("mcp.tools.list", json!({})).await.unwrap();
    let global = global.as_array().unwrap();
    let verdict = |list: &[Value], name: &str| {
        let t = tool(list, name);
        (t["enabled"].clone(), t["rule"].clone(), t["key"].clone())
    };
    assert_eq!(
        verdict(global, "probe_exact"),
        (json!(false), json!("tool"), json!("probe_exact"))
    );
    assert_eq!(
        verdict(global, "probe_glob"),
        (json!(false), json!("glob"), json!("probe_g*"))
    );
    assert_eq!(
        verdict(global, "probe_db"),
        (json!(false), json!("group"), json!("db"))
    );
    assert_eq!(
        verdict(global, "probe_write"),
        (json!(false), json!("preset"), json!("read-only"))
    );
    assert_eq!(
        verdict(global, "probe_on"),
        (json!(true), json!("preset"), json!("read-only"))
    );
    // 전역만 보면 공유 설정은 없다.
    assert_eq!(
        verdict(global, "probe_shared"),
        (json!(true), json!("preset"), json!("read-only"))
    );

    let project = window
        .call("mcp.tools.list", json!({"projectId": f.alpha}))
        .await
        .unwrap();
    let project = project.as_array().unwrap();
    assert_eq!(
        verdict(project, "probe_shared"),
        (json!(false), json!("shared"), Value::Null)
    );
    assert_eq!(
        verdict(project, "probe_exact"),
        (json!(false), json!("tool"), json!("probe_exact"))
    );
    assert_eq!(
        verdict(project, "probe_on"),
        (json!(true), json!("preset"), json!("read-only"))
    );

    // 카탈로그의 칸 · 최근 호출 수(프로젝트를 주면 그 프로젝트의 행만).
    let on = tool(global, "probe_on");
    assert_eq!(on["tier"], "read");
    assert_eq!(on["group"], "project");
    assert_eq!(on["description"], "시험용: 켜진 툴(프리셋).");
    assert_eq!(on["inputSchema"]["type"], "object");
    assert_eq!(on["recentCalls"], 4);
    assert_eq!(tool(project, "probe_on")["recentCalls"], 3);
    assert_eq!(tool(global, "probe_db")["group"], "db");
    assert_eq!(tool(global, "probe_write")["tier"], "write");
    assert_eq!(tool(global, "probe_exact")["recentCalls"], 0);
    // 이 Host 가 처리하는 툴만 — 카탈로그와 같다.
    let names: Vec<&str> = global.iter().map(|t| t["name"].as_str().unwrap()).collect();
    let catalog = Catalog::linked().routed_by(f.host.router());
    let expected: Vec<&str> = catalog.tools().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, expected);

    // 없는 프로젝트는 레지스트리의 오류 그대로.
    assert!(
        window
            .call("mcp.tools.list", json!({"projectId": "nope"}))
            .await
            .is_err()
    );
}

/// `mcp.*` 는 에이전트에게 열지 않는다 — 툴이 아니고(선언에 `mcp` 가 없다), 파일 범위 연결도 부르지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_rpcs_are_not_tools_and_not_for_file_windows() {
    let f = Fixture::new();
    let catalog = Catalog::linked();
    for (method, params) in [
        ("mcp.sessions", Value::Null),
        ("mcp.tools.list", json!({})),
        ("mcp.audit.list", json!({})),
    ] {
        assert!(
            catalog.tools().iter().all(|t| t.method != method),
            "{method} 는 툴이 아니다"
        );
        let file = f
            .window_as(Caller::file(Role::Owner, f.base.join("work/alpha/a.txt")))
            .await;
        match file.call(method, params.clone()).await {
            Err(CallError::Rpc { code, data, .. }) => {
                assert_eq!(code, i64::from(pw_rpc::code::PERMISSION_DENIED), "{method}");
                assert_eq!(data.unwrap()["scope"], "file", "{method}");
            }
            other => panic!("{method}: 파일 범위 연결은 부르지 못한다: {other:?}"),
        }
        // 창(owner)은 부른다.
        f.window().await.call(method, params).await.unwrap();
    }
}

/// 에이전트 범위 연결(MCP 세션이 Host 에 붙는 연결 — `Caller::agent`)은 role 이 owner 라도 거절된다. `project.setTrust` 와 같은 꼴
/// (`PERMISSION_DENIED`, `reason: agentSession`). 거절은 `methods::register` 의 한 문(`WindowsOnly`)이 한다.
async fn assert_refuses_agent(method: &str, params: Value) {
    let f = Fixture::new();
    let agent = f.window_as(Caller::agent(Role::Owner)).await;
    match agent.call(method, params.clone()).await {
        Err(CallError::Rpc { code, data, .. }) => {
            assert_eq!(code, i64::from(pw_rpc::code::PERMISSION_DENIED), "{method}");
            let data = data.unwrap();
            assert_eq!(data["reason"], "agentSession", "{method}");
            assert_eq!(data["method"], method);
        }
        other => panic!("{method}: 에이전트 범위 연결은 부르지 못한다: {other:?}"),
    }
    // 같은 role 의 창은 부른다 — 막는 것은 범위다.
    f.window().await.call(method, params).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_sessions_refuses_agent_connections() {
    assert_refuses_agent("mcp.sessions", Value::Null).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_tools_list_refuses_agent_connections() {
    assert_refuses_agent("mcp.tools.list", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_audit_list_refuses_agent_connections() {
    assert_refuses_agent("mcp.audit.list", json!({})).await;
}

/// 행 수가 `limit` 의 정확한 배수면 마지막 쪽의 `nextCursor` 가 `null` 이다 — 빈 쪽이 하나 더 생기지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_list_ends_without_an_empty_page_on_an_exact_multiple() {
    let f = Fixture::new();
    let log = AuditLog::under_state(&f.paths.state);
    for _ in 0..1000 {
        log.record(&f.entry("probe_on", None, Outcome::Ok)).unwrap();
    }
    let window = f.window().await;
    let first = window
        .call("mcp.audit.list", json!({"limit": 500}))
        .await
        .unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), 500);
    let cursor = first["nextCursor"].clone();
    assert!(!cursor.is_null());
    let second = window
        .call("mcp.audit.list", json!({"limit": 500, "cursor": cursor}))
        .await
        .unwrap();
    assert_eq!(second["entries"].as_array().unwrap().len(), 500);
    assert_eq!(second["nextCursor"], Value::Null, "마지막 쪽");
}

/// 「최근 호출 수」 는 30일(`methods::RECENT`) 안의 행만 센다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_calls_count_only_the_last_thirty_days() {
    let f = Fixture::new();
    let log = AuditLog::under_state(&f.paths.state);
    let old = log.record(&f.entry("probe_on", None, Outcome::Ok)).unwrap();
    let edge = log.record(&f.entry("probe_on", None, Outcome::Ok)).unwrap();
    log.record(&f.entry("probe_on", None, Outcome::Ok)).unwrap();
    let day = 24 * 60 * 60 * 1000_i64;
    let conn = rusqlite::Connection::open(f.paths.state.join("mcp/audit.db")).unwrap();
    // 31일 전 행은 빠지고, 29일 전 행은 든다.
    conn.execute(
        "UPDATE calls SET at = at - ?1 WHERE id = ?2",
        (31 * day, old),
    )
    .unwrap();
    conn.execute(
        "UPDATE calls SET at = at - ?1 WHERE id = ?2",
        (29 * day, edge),
    )
    .unwrap();
    drop(conn);
    let list = f
        .window()
        .await
        .call("mcp.tools.list", json!({}))
        .await
        .unwrap();
    assert_eq!(tool(list.as_array().unwrap(), "probe_on")["recentCalls"], 2);
}

/// `mcp.audit.clear` 는 그 프로젝트의 행만 지운다 — 프로젝트 없는 행 · 다른 프로젝트의 행은 남는다. 부르는 것은 그 프로젝트의
/// 창뿐이다(Host 범위 연결 · 다른 프로젝트의 창은 `PERMISSION_DENIED`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_clear_removes_only_that_projects_rows_from_its_own_window() {
    let f = Fixture::new();
    let log = AuditLog::under_state(&f.paths.state);
    for project in [
        Some(f.alpha.as_str()),
        None,
        Some("other"),
        Some(f.alpha.as_str()),
    ] {
        log.record(&f.entry("probe_on", project, Outcome::Ok))
            .unwrap();
    }
    let params = json!({"projectId": f.alpha});
    for (who, caller) in [
        ("host window", Caller::new(Role::Owner)),
        (
            "another project's window",
            Caller {
                scope: pw_rpc::ConnectionScope::Project("other".into()),
                ..Caller::new(Role::Owner)
            },
        ),
    ] {
        match f
            .window_as(caller)
            .await
            .call("mcp.audit.clear", params.clone())
            .await
        {
            Err(CallError::Rpc { code, data, .. }) => {
                assert_eq!(code, i64::from(pw_rpc::code::PERMISSION_DENIED), "{who}");
                assert_eq!(data.unwrap()["reason"], "notProjectWindow", "{who}");
            }
            other => panic!("{who}: {other:?}"),
        }
    }
    let own = f
        .window_as(Caller {
            scope: pw_rpc::ConnectionScope::Project(f.alpha.clone()),
            ..Caller::new(Role::Owner)
        })
        .await;
    let cleared = own.call("mcp.audit.clear", params).await.unwrap();
    assert_eq!(cleared, json!({"deleted": 2}));
    let page = f
        .window()
        .await
        .call("mcp.audit.list", json!({}))
        .await
        .unwrap();
    let left: Vec<Value> = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["projectId"].clone())
        .collect();
    assert_eq!(left, [json!("other"), Value::Null]);
}

/// `mcp.audit.clear` 도 에이전트 범위 연결을 거절한다(`WindowsOnly` — 그 프로젝트의 id 를 줘도). 툴도 아니고, 단일 파일 창도
/// 부르지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_audit_clear_refuses_agent_connections() {
    let f = Fixture::new();
    assert!(
        Catalog::linked()
            .tools()
            .iter()
            .all(|t| t.method != "mcp.audit.clear")
    );
    let log = AuditLog::under_state(&f.paths.state);
    log.record(&f.entry("probe_on", Some(&f.alpha), Outcome::Ok))
        .unwrap();
    let params = json!({"projectId": f.alpha});
    match f
        .window_as(Caller::agent(Role::Owner))
        .await
        .call("mcp.audit.clear", params.clone())
        .await
    {
        Err(CallError::Rpc { code, data, .. }) => {
            assert_eq!(code, i64::from(pw_rpc::code::PERMISSION_DENIED));
            let data = data.unwrap();
            assert_eq!(data["reason"], "agentSession");
            assert_eq!(data["method"], "mcp.audit.clear");
        }
        other => panic!("에이전트 범위 연결은 지우지 못한다: {other:?}"),
    }
    // 단일 파일 창도 부르지 못한다(`file_scope` 를 달지 않았다).
    let file = f
        .window_as(Caller::file(Role::Owner, f.base.join("work/alpha/a.txt")))
        .await;
    match file.call("mcp.audit.clear", params).await {
        Err(CallError::Rpc { code, data, .. }) => {
            assert_eq!(code, i64::from(pw_rpc::code::PERMISSION_DENIED));
            assert_eq!(data.unwrap()["scope"], "file");
        }
        other => panic!("파일 범위 연결은 지우지 못한다: {other:?}"),
    }
    let (rows, _) = log
        .list(&pw_mcp::audit::Query {
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1, "지워지지 않았다");
}

/// `mcp.audit.list` 는 둘째 판의 칸(승인 방식 · 계획 해시)과 결과 `started` 를 돌려주고 `started` 로 거른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_list_returns_the_approval_the_plan_hash_and_started_rows() {
    let f = Fixture::new();
    let log = AuditLog::under_state(&f.paths.state);
    let approved = Entry {
        approval: Some(pw_mcp::audit::AuditApproval::Window),
        plan_hash: Some("sha256:plan".into()),
        ..f.entry("probe_write", Some(&f.alpha), Outcome::Ok)
    };
    let done = log.begin(&approved).unwrap();
    log.finish(
        done,
        &Entry {
            approval: None,
            plan_hash: None,
            ..approved.clone()
        },
    )
    .unwrap();
    let unknown = log.begin(&approved).unwrap();
    log.record(&f.entry("probe_on", None, Outcome::Ok)).unwrap();
    let window = f.window().await;
    let page = window.call("mcp.audit.list", json!({})).await.unwrap();
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["approval"], Value::Null);
    assert_eq!(entries[0]["planHash"], Value::Null);
    assert_eq!(entries[1]["id"], unknown);
    assert_eq!(entries[1]["outcome"], "started");
    assert_eq!(entries[1]["durationMs"], 0);
    assert_eq!(entries[1]["resultSize"], Value::Null);
    assert_eq!(entries[2]["id"], done);
    assert_eq!(entries[2]["outcome"], "ok");
    assert_eq!(entries[2]["approval"], "window");
    assert_eq!(entries[2]["planHash"], "sha256:plan");
    let started = window
        .call("mcp.audit.list", json!({"outcome": "started"}))
        .await
        .unwrap();
    let ids: Vec<Value> = started["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].clone())
        .collect();
    assert_eq!(ids, [json!(unknown)]);
}
