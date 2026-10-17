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
use rmcp::service::{MaybeSendFuture, NotificationContext, RunningService};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
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
        let base = std::fs::canonicalize(tmp.path()).unwrap();
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

/// 툴은 `mcp: true` 로 고른 선언뿐이다. 새 RPC 가 생겨도 이 목록은 저절로 늘지 않는다 — 늘리려면 선언에 `mcp: true` 를
/// 달고 이 목록을 함께 고친다.
#[test]
fn only_methods_marked_mcp_become_tools() {
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
