//! MCP 세션을 실제 rmcp 클라이언트로 돌린다: 툴 목록 · 호출 · 노출 설정과 `list_changed` · 등급 정책 · 감사 로그.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::{Catalog, Services, SessionRequest};
use pw_rpc::{RpcError, rpc_method};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::model::{ProtocolVersion, ServerNotification, SubscriptionFilter};
use rmcp::service::{MaybeSendFuture, NotificationContext, RunningService};
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient, ServiceExt};
use serde_json::{Map, Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

rpc_method! {
    /// 시험용 쓰기 툴. 정책이 거절해야 한다.
    pub TestWrite: "mcptest.write" {
        params: (),
        result: (),
        tier: Write,
        mcp: true,
    }
}

#[derive(Clone, Default)]
struct Client {
    list_changed: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl ClientHandler for Client {
    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.list_changed.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_one();
        std::future::ready(())
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    paths: AppPaths,
    host: Arc<LocalHost>,
    projects: Vec<String>,
}

impl Fixture {
    /// 프로젝트 셋(`alpha` · `beta` · `gamma`)을 등록한 Host.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        // Windows 의 canonicalize 는 verbatim 접두를 붙인다 — 레지스트리가 저장하는 꼴로 맞춘다.
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        let host = Arc::new(
            LocalHost::builder("local")
                .config(paths.clone())
                .methods(|router| {
                    router.unary(TestWrite, |_ctx, ()| async { Ok::<(), RpcError>(()) });
                })
                .build(),
        );
        let registry = host.registry().unwrap();
        let projects = ["alpha", "beta", "gamma"]
            .iter()
            .map(|name| {
                let root = base.join("work").join(name);
                std::fs::create_dir_all(&root).unwrap();
                registry
                    .create(NewProject {
                        root,
                        ..NewProject::default()
                    })
                    .unwrap()
                    .id
            })
            .collect();
        std::fs::write(base.join("work/beta/Cargo.toml"), "").unwrap();
        Self {
            _tmp: tmp,
            base,
            paths,
            host,
            projects,
        }
    }

    fn services(&self) -> Arc<Services> {
        let router = Arc::new(HostRouter::new());
        router.add_host(self.host.clone()).unwrap();
        let config = self.host.config().unwrap();
        Arc::new(Services {
            router,
            host_id: "local".into(),
            paths: self.paths.clone(),
            global: config.global.clone(),
            bus: self.host.bus().clone(),
            audit: Arc::new(AuditLog::under_state(&self.paths.state)),
            catalog: Arc::new(Catalog::linked()),
            approval: Approval::Unavailable,
            transport: "test",
        })
    }

    async fn connect(
        &self,
        request: SessionRequest,
    ) -> (
        RunningService<RoleClient, Client>,
        Client,
        CancellationToken,
    ) {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let (r, w) = tokio::io::split(server_io);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            self.services(),
            request,
            (r, w),
            cancel.clone(),
        ));
        let client = Client::default();
        let running = client.clone().serve(client_io).await.unwrap();
        (running, client, cancel)
    }

    fn work(&self, name: &str) -> PathBuf {
        self.base.join("work").join(name)
    }
}

async fn call(
    client: &RunningService<RoleClient, Client>,
    name: &str,
    args: Value,
) -> CallToolResult {
    let mut params = CallToolRequestParams::new(name.to_owned());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    client.peer().call_tool(params).await.unwrap()
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

async fn tool_names(client: &RunningService<RoleClient, Client>) -> Vec<String> {
    client
        .peer()
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// 감사 로그 쓰기는 응답 뒤에 블로킹 스레드에서 끝난다. 행이 `n` 개가 될 때까지 기다린다.
fn audit_rows(state: &Path, n: usize) -> Vec<(Option<String>, String, String, Option<String>)> {
    let file = state.join("mcp/audit.db");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(conn) = rusqlite_open(&file) {
            let rows = read_rows(&conn);
            if rows.len() >= n {
                return rows;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "audit rows did not reach {n}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn rusqlite_open(file: &Path) -> Result<rusqlite::Connection, rusqlite::Error> {
    rusqlite::Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
}

fn read_rows(conn: &rusqlite::Connection) -> Vec<(Option<String>, String, String, Option<String>)> {
    let Ok(mut stmt) =
        conn.prepare("SELECT project_id, tool, outcome, detail FROM calls ORDER BY id")
    else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// 카탈로그는 `mcp: true` 로 고른 선언만 툴로 만든다. 이 시험 바이너리에 링크된 선언(pw-core 와 이 파일의 시험용 선언)만 본다 —
/// 실제로 툴이 되는 메서드의 고정 목록은 모든 메서드 크레이트가 링크되는 데스크톱 크레이트의 `mcp_tools_are_exactly_these`.
#[test]
fn the_catalog_follows_the_mcp_flag() {
    let catalog = Catalog::linked();
    let names: Vec<_> = catalog.tools().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["mcptest_write", "project_info", "project_list"]);
    for info in pw_rpc::methods() {
        let exposed = catalog.tools().iter().any(|t| t.method == info.meta.name);
        assert_eq!(exposed, info.meta.mcp, "{}", info.meta.name);
    }
}

#[test]
fn tool_schemas_come_from_the_rpc_types() {
    let catalog = Catalog::linked();
    let list = catalog.get("project_list").unwrap();
    assert!(list.unit_params);
    assert_eq!(list.input_schema_for(false)["type"], json!("object"));
    assert!(
        list.output_schema.is_none(),
        "배열 결과는 outputSchema 를 두지 않는다"
    );
    let info = catalog.get("project_info").unwrap();
    let schema = info.input_schema_for(false);
    assert_eq!(schema["required"], json!(["projectId"]));
    assert_eq!(schema["properties"]["projectId"]["type"], json!("string"));
    assert!(info.input_schema_for(true).get("required").is_none());
    let output = info.output_schema.as_ref().unwrap();
    assert_eq!(output["type"], json!("object"));
    assert!(output["properties"].get("languages").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_and_calls_project_tools() {
    let f = Fixture::new();
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(f.work("beta")),
        })
        .await;
    // 프리셋 standard: read · write 툴이 보인다(write 는 부르면 정책이 거절한다).
    assert_eq!(
        tool_names(&client).await,
        ["mcptest_write", "project_info", "project_list"]
    );

    let list = call(&client, "project_list", json!({})).await;
    assert_eq!(list.is_error, Some(false));
    let projects: Vec<Value> = serde_json::from_str(&text(&list)).unwrap();
    let names: Vec<_> = projects
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "beta", "gamma"]);

    // cwd 로 정해진 세션 프로젝트(beta)가 projectId 의 기본값이다.
    let info = call(&client, "project_info", json!({})).await;
    let info = info
        .structured_content
        .expect("object 결과는 structuredContent");
    assert_eq!(info["project"]["id"], json!(f.projects[1]));
    assert_eq!(info["languages"], json!(["rust"]));
    let other = call(&client, "project_info", json!({"projectId": f.projects[0]})).await;
    assert_eq!(
        other.structured_content.unwrap()["project"]["name"],
        json!("alpha")
    );
    let missing = call(&client, "project_info", json!({"projectId": "nope"})).await;
    assert_eq!(missing.is_error, Some(true));
    assert!(
        text(&missing).contains("unknown project"),
        "{}",
        text(&missing)
    );

    // 쓰기 등급은 승인 경로가 없어 거절된다. RPC 핸들러까지 가지 않는다.
    let write = call(&client, "mcptest_write", json!({})).await;
    assert_eq!(write.is_error, Some(true));
    assert!(text(&write).contains("approval"), "{}", text(&write));

    let rows = audit_rows(&f.paths.state, 5);
    let summary: Vec<_> = rows
        .iter()
        .map(|(p, t, o, _)| (p.clone(), t.as_str(), o.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (None, "project_list", "ok"),
            (Some(f.projects[1].clone()), "project_info", "ok"),
            (Some(f.projects[0].clone()), "project_info", "ok"),
            (Some("nope".into()), "project_info", "error"),
            (None, "mcptest_write", "denied"),
        ]
    );
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 프로젝트 설정의 `[mcp.tools]` 가 전역을 덮고, 설정이 바뀌면 `tools/list_changed` 가 온다. 꺼진 툴은 부를 수 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exposure_follows_global_and_project_config() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.paths.config).unwrap();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp]\npreset = \"read-only\"\n",
    )
    .unwrap();
    f.host.config().unwrap().global.reload().unwrap();
    let pitwall = f.work("alpha/.pitwall");
    std::fs::create_dir_all(&pitwall).unwrap();
    std::fs::write(
        pitwall.join("project.toml"),
        "schema = 1\n[mcp.tools]\nproject_info = false\n",
    )
    .unwrap();
    let watching = f.host.watch_config(&tokio::runtime::Handle::current());
    watching.unwrap();

    let (client, handler, cancel) = f
        .connect(SessionRequest {
            project: Some(PathBuf::from("alpha")),
            cwd: Some(f.base.join("work")),
        })
        .await;
    assert_eq!(tool_names(&client).await, ["project_list"]);
    let hidden = client
        .peer()
        .call_tool(CallToolRequestParams::new("project_info"))
        .await;
    assert!(hidden.is_err(), "꺼진 툴이 불렸다: {hidden:?}");

    std::fs::write(pitwall.join("project.toml"), "schema = 1\n").unwrap();
    tokio::time::timeout(Duration::from_secs(10), handler.notify.notified())
        .await
        .expect("tools/list_changed 가 오지 않았다");
    assert_eq!(tool_names(&client).await, ["project_info", "project_list"]);
    let rows = audit_rows(&f.paths.state, 1);
    assert_eq!(rows[0].1, "project_info");
    assert_eq!(rows[0].2, "denied");
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 알려지지 않은 경로(`--project` 가 등록 프로젝트 밖)여도 세션은 뜬다. projectId 를 주지 않으면 필수 오류다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_without_a_project_requires_project_id() {
    let f = Fixture::new();
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: Some(f.base.join("elsewhere")),
            cwd: None,
        })
        .await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let info = tools.iter().find(|t| t.name == "project_info").unwrap();
    assert_eq!(info.input_schema["required"], json!(["projectId"]));
    let result = call(&client, "project_info", Value::Object(Map::new())).await;
    assert_eq!(result.is_error, Some(true), "{}", text(&result));
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 2026-07-28 클라이언트(초기화 핸드셰이크 없음 — Claude Code 2.1.302 가 이것으로 붙는다): clientInfo 는 요청마다 오고,
/// 툴 목록 변경은 `subscriptions/listen` 구독으로만 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stateless_client_gets_client_info_audited_and_list_changes_by_subscription() {
    let f = Fixture::new();
    f.host
        .watch_config(&tokio::runtime::Handle::current())
        .unwrap();
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let (r, w) = tokio::io::split(server_io);
    let cancel = CancellationToken::new();
    tokio::spawn(pw_mcp::serve(
        f.services(),
        SessionRequest::default(),
        (r, w),
        cancel.clone(),
    ));
    let client = Client::default()
        .serve_with_lifecycle(
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();
    let list = call(&client, "project_list", json!({})).await;
    assert_eq!(list.is_error, Some(false));
    audit_rows(&f.paths.state, 1);
    let conn = rusqlite_open(&f.paths.state.join("mcp/audit.db")).unwrap();
    let recorded: Option<String> = conn
        .query_row("SELECT client FROM calls", [], |r| r.get(0))
        .unwrap();
    assert!(
        recorded.is_some_and(|c| c.starts_with("rmcp")),
        "clientInfo 가 감사 로그에 없다"
    );

    let mut subscription = client
        .peer()
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await
        .unwrap();
    std::fs::create_dir_all(&f.paths.config).unwrap();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp.tools]\nproject_list = false\n",
    )
    .unwrap();
    let note = tokio::time::timeout(Duration::from_secs(10), subscription.next())
        .await
        .expect("구독으로 tools/list_changed 가 오지 않았다")
        .unwrap();
    assert!(
        matches!(
            note,
            Some(ServerNotification::ToolListChangedNotification(_))
        ),
        "{note:?}"
    );
    assert_eq!(tool_names(&client).await, ["mcptest_write", "project_info"]);
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 클라이언트가 끊으면 세션이 쥔 것(버스 구독 · Host 연결 · list_changed 태스크)이 모두 걷힌다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finished_session_releases_its_subscriptions() {
    let f = Fixture::new();
    let before = f.host.bus().subscriber_count();
    let (client, _, _cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(f.work("alpha")),
        })
        .await;
    call(&client, "project_list", json!({})).await;
    assert!(f.host.bus().subscriber_count() > before);
    client.cancel().await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while f.host.bus().subscriber_count() != before {
        assert!(
            std::time::Instant::now() < deadline,
            "세션이 끝났는데 버스 구독이 남았다: {} > {before}",
            f.host.bus().subscriber_count()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 폴더 아래 모든 파일의 (상대 경로, 바이트).
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((
                    path.strip_prefix(dir).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                ));
            }
        }
    }
    out.sort();
    out
}

/// 세션은 프로젝트 설정을 읽기만 한다: 사람이 손으로 쓴 v0 `.pitwall/project.toml` 이 있어도 세션을 열고 툴을 불러도
/// 프로젝트 폴더의 바이트가 하나도 바뀌지 않고 `.backup/` 도 생기지 않는다. 그 파일의 `[mcp]` 는 노출에 반영된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_never_writes_the_project_folder() {
    let f = Fixture::new();
    let project = f.work("beta");
    std::fs::create_dir_all(project.join(".pitwall")).unwrap();
    std::fs::write(
        project.join(".pitwall/project.toml"),
        "# hand written\n[mcp.tools]\nproject_info = false\n",
    )
    .unwrap();
    let before = tree(&project);
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(project.clone()),
        })
        .await;
    assert_eq!(tool_names(&client).await, ["mcptest_write", "project_list"]);
    call(&client, "project_list", json!({})).await;
    client.cancel().await.unwrap();
    cancel.cancel();
    assert_eq!(tree(&project), before, "세션이 프로젝트 폴더를 고쳤다");
    assert!(!project.join(".pitwall/.backup").exists());
}

/// 세션의 프로젝트 설정 이벤트는 세션 안에서 끝난다 — 앱 버스에 `config.*` 를 한 번 더 내지 않는다(깨진 설정이어도).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_does_not_echo_project_config_events_on_the_app_bus() {
    let f = Fixture::new();
    let project = f.work("gamma");
    std::fs::create_dir_all(project.join(".pitwall")).unwrap();
    std::fs::write(project.join(".pitwall/project.toml"), "not = [valid").unwrap();
    let mut events = f.host.bus().subscribe(
        pw_core::Filter::all()
            .topics([pw_core::config::TOPIC_CHANGED, pw_core::config::TOPIC_ERROR]),
    );
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(project),
        })
        .await;
    call(&client, "project_list", json!({})).await;
    assert!(
        events.try_recv().is_none(),
        "앱 버스에 세션의 설정 이벤트가 나갔다"
    );
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 커밋되는 `.pitwall/project.toml` 은 노출을 넓히지 못한다. 넓히는 것은 `.local.toml` 이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_project_config_cannot_widen_exposure() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.paths.config).unwrap();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp]\npreset = \"read-only\"\n",
    )
    .unwrap();
    f.host.config().unwrap().global.reload().unwrap();
    let pitwall = f.work("alpha/.pitwall");
    std::fs::create_dir_all(&pitwall).unwrap();
    std::fs::write(
        pitwall.join("project.toml"),
        "schema = 1\n[mcp]\npreset = \"full\"\n[mcp.tools]\nmcptest_write = true\n\"*\" = true\n",
    )
    .unwrap();
    let request = || SessionRequest {
        project: None,
        cwd: Some(f.work("alpha")),
    };
    let (client, _, cancel) = f.connect(request()).await;
    assert_eq!(tool_names(&client).await, ["project_info", "project_list"]);
    client.cancel().await.unwrap();
    cancel.cancel();

    std::fs::write(
        pitwall.join("project.local.toml"),
        "schema = 1\n[mcp.tools]\nmcptest_write = true\n",
    )
    .unwrap();
    let (client, _, cancel) = f.connect(request()).await;
    assert_eq!(
        tool_names(&client).await,
        ["mcptest_write", "project_info", "project_list"]
    );
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 2026-07-28 클라이언트는 구독(`subscriptions/listen`)을 열지 않으면 `tools/list_changed` 를 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stateless_client_without_a_subscription_gets_no_list_changed() {
    let f = Fixture::new();
    f.host
        .watch_config(&tokio::runtime::Handle::current())
        .unwrap();
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let (r, w) = tokio::io::split(server_io);
    let cancel = CancellationToken::new();
    tokio::spawn(pw_mcp::serve(
        f.services(),
        SessionRequest::default(),
        (r, w),
        cancel.clone(),
    ));
    let handler = Client::default();
    let client = handler
        .clone()
        .serve_with_lifecycle(
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();
    assert_eq!(tool_names(&client).await.len(), 3);
    std::fs::create_dir_all(&f.paths.config).unwrap();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp.tools]\nproject_list = false\n",
    )
    .unwrap();
    // 변경이 서버에 닿았음을 목록으로 확인한 뒤, 알림이 오지 않았는지 본다.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while tool_names(&client).await.len() != 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "설정 변경이 반영되지 않았다"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(handler.list_changed.load(Ordering::SeqCst), 0);
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 선언은 링크돼 있어도 Host 가 처리하지 않는 메서드(여기서는 `mcptest.write`)는 카탈로그에서 빠진다.
#[test]
fn the_catalog_keeps_only_methods_the_host_routes() {
    let host = LocalHost::new("local");
    let names: Vec<_> = Catalog::linked()
        .routed_by(host.router())
        .tools()
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(names.is_empty(), "{names:?}");
    let f = Fixture::new();
    let names: Vec<_> = Catalog::linked()
        .routed_by(f.host.router())
        .tools()
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(names, ["mcptest_write", "project_info", "project_list"]);
}
