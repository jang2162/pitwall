//! MCP 세션을 실제 rmcp 클라이언트로 돌린다: 툴 목록 · 호출 · 노출 설정과 `list_changed` · 등급 정책 · 감사 로그,
//! 세션 워크트리 · 응답 상한 · 결과 표지 · 대상 프로젝트의 노출 · 호출 취소 · 헤드리스 안내(16-m5-plan M5-03).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::marker::{Mark, Reason};
use pw_mcp::policy::Approval;
use pw_mcp::{Catalog, HostKind, Services, SessionRequest};
use pw_rpc::{Router, RpcError, rpc_method};
use rmcp::model::{CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest};
use rmcp::model::{ProtocolVersion, ServerNotification, SubscriptionFilter};
use rmcp::service::{
    MaybeSendFuture, NotificationContext, PeerRequestOptions, RunningService, ServiceError,
};
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient, ServiceExt};
use serde_json::{Map, Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// 입력 없는 쓰기 툴의 입력(쓰기 툴은 `Planned` struct 입력이어야 한다 — 카탈로그 규칙).
#[derive(Debug, Clone, serde::Deserialize, specta::Type, schemars::JsonSchema)]
pub struct NoInput {}

rpc_method! {
    /// 시험용 쓰기 툴. 정책이 거절해야 한다.
    pub TestWrite: "mcptest.write" {
        params: pw_core::registry::plan::Planned<NoInput>,
        result: (),
        tier: Write,
        mcp: { group: "project", name: "mcptest_write" },
    }

    /// 시험용 프로젝트 툴. 받은 입력을 그대로 돌려준다(채워진 `projectId` · `worktree` 를 본다).
    pub TestEcho: "mcptest.echo" {
        params: EchoParams,
        result: EchoParams,
        tier: Read,
        mcp: { group: "project", name: "mcptest_echo" },
    }

    /// 시험용: 300 KiB 글자열을 돌려준다(응답 상한을 넘는다).
    pub TestBig: "mcptest.big" {
        params: (),
        result: String,
        tier: Read,
        mcp: { group: "project", name: "mcptest_big" },
    }

    /// 시험용: 취소될 때까지 돈다.
    pub TestSlow: "mcptest.slow" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "project", name: "mcptest_slow" },
    }

    /// 시험용: Host 가 그 RPC 를 거뒀다고 답한다.
    pub TestHostCancelled: "mcptest.hostCancelled" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "project", name: "mcptest_host_cancelled" },
    }

    /// 시험용: 받은 코드와 `data.reason` 으로 실패한다(관문의 거절 · 다른 오류).
    pub TestRefuse: "mcptest.refuse" {
        params: RefuseParams,
        result: (),
        tier: Read,
        mcp: { group: "project", name: "mcptest_refuse" },
    }
}

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RefuseParams {
    pub code: i32,
    #[serde(default)]
    pub reason: Option<String>,
    /// 오류 `data` 의 최상위 `audit`(감사 상세 — 「툴 메서드 규약」). 주면 `data` 에 [`DATA_ONLY`] 도 싣는다.
    #[serde(default)]
    pub audit: Option<String>,
}

/// 감사 상세를 실은 오류의 `data` 에만 있는 글 — 감사 행에 남지 않아야 한다.
const DATA_ONLY: &str = "value-from-the-server-error-7f3a";

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct EchoParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 주면 결과의 최상위 `audit` 으로 돌아간다(감사 상세 — 「툴 메서드 규약」).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<String>,
}

/// 오래 도는 가짜 핸들러가 남기는 것: 시작했다, 끝나기 전에 버려졌다.
#[derive(Clone, Default)]
struct Probe {
    started: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

/// 핸들러 future 안에 둔다. future 가 끝나지 않고 버려지면 표지를 세운다.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// 시험용 툴 메서드를 Host 에 더한다.
fn test_methods(probe: Probe) -> impl FnOnce(&mut Router) + 'static {
    move |router: &mut Router| {
        router.unary(TestEcho, |_ctx, params: EchoParams| async move {
            Ok::<_, RpcError>(params)
        });
        router.unary(TestBig, |_ctx, ()| async {
            Ok::<_, RpcError>("x".repeat(300 * 1024))
        });
        router.unary(TestSlow, move |_ctx, ()| {
            let probe = probe.clone();
            async move {
                let flag = DropFlag(probe.dropped.clone());
                probe.started.notify_one();
                tokio::time::sleep(Duration::from_secs(60)).await;
                std::mem::forget(flag);
                Ok::<(), RpcError>(())
            }
        });
        router.unary(TestHostCancelled, |_ctx, ()| async {
            Err::<(), _>(RpcError::cancelled())
        });
        router.unary(TestRefuse, |_ctx, p: RefuseParams| async move {
            let error = RpcError::new(p.code, "refused");
            Err::<(), _>(match (p.reason, p.audit) {
                (reason, Some(audit)) => error.with_data(
                    json!({ "reason": reason, "audit": audit, "statements": [DATA_ONLY] }),
                ),
                (Some(reason), None) => error.with_data(json!({ "reason": reason })),
                (None, None) => error,
            })
        });
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
        Self::with_methods(|_| {})
    }

    /// [`Self::new`] 에 메서드를 더 등록한 Host. 세션의 카탈로그는 Host 가 처리하는 메서드만 보인다.
    fn with_methods(extra: impl FnOnce(&mut Router) + 'static) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        // Windows 의 canonicalize 는 verbatim 접두를 붙인다 — 레지스트리가 저장하는 꼴로 맞춘다.
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        let host = Arc::new(
            LocalHost::builder("local")
                .config(paths.clone())
                .methods(move |router| {
                    router.unary(
                        TestWrite,
                        |_ctx, _p: pw_core::registry::plan::Planned<NoInput>| async {
                            Ok::<(), RpcError>(())
                        },
                    );
                    extra(router);
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
        self.services_on(HostKind::App)
    }

    fn services_on(&self, host_kind: HostKind) -> Arc<Services> {
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
            catalog: Arc::new(Catalog::linked().routed_by(self.host.router())),
            approval: Approval::Unavailable,
            transport: "test",
            host_kind,
            sessions: Default::default(),
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
        self.connect_on(request, HostKind::App).await
    }

    async fn connect_on(
        &self,
        request: SessionRequest,
        host_kind: HostKind,
    ) -> (
        RunningService<RoleClient, Client>,
        Client,
        CancellationToken,
    ) {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let (r, w) = tokio::io::split(server_io);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            self.services_on(host_kind),
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
/// 실제로 툴이 되는 메서드의 고정 목록은 모든 메서드 크레이트가 링크되는 데스크톱 크레이트의 `mcp_tools_are_exactly_these`(`apps/desktop/src-tauri/src/mcp_tests.rs`).
#[test]
fn the_catalog_follows_the_mcp_flag() {
    let catalog = Catalog::linked();
    let names: Vec<_> = catalog.tools().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "mcptest_big",
            "mcptest_echo",
            "mcptest_host_cancelled",
            "mcptest_refuse",
            "mcptest_slow",
            "mcptest_write",
            "project_info",
            "project_list",
            "project_open",
            "project_register",
            "settings_get"
        ]
    );
    for info in pw_rpc::methods() {
        let exposed = catalog.tools().iter().any(|t| t.method == info.meta.name);
        assert_eq!(exposed, info.meta.mcp.is_some(), "{}", info.meta.name);
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
        [
            "mcptest_write",
            "project_info",
            "project_list",
            "project_register",
            "settings_get"
        ]
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
    assert_eq!(tool_names(&client).await, ["project_list", "settings_get"]);
    let hidden = client
        .peer()
        .call_tool(CallToolRequestParams::new("project_info"))
        .await;
    assert!(hidden.is_err(), "꺼진 툴이 불렸다: {hidden:?}");

    std::fs::write(pitwall.join("project.toml"), "schema = 1\n").unwrap();
    tokio::time::timeout(Duration::from_secs(10), handler.notify.notified())
        .await
        .expect("tools/list_changed 가 오지 않았다");
    assert_eq!(
        tool_names(&client).await,
        ["project_info", "project_list", "settings_get"]
    );
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
    assert_eq!(
        tool_names(&client).await,
        [
            "mcptest_write",
            "project_info",
            "project_register",
            "settings_get"
        ]
    );
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
    assert_eq!(
        tool_names(&client).await,
        [
            "mcptest_write",
            "project_list",
            "project_register",
            "settings_get"
        ]
    );
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
    assert_eq!(
        tool_names(&client).await,
        ["project_info", "project_list", "settings_get"]
    );
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
        [
            "mcptest_write",
            "project_info",
            "project_list",
            "settings_get"
        ]
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
    assert_eq!(tool_names(&client).await.len(), 5);
    std::fs::create_dir_all(&f.paths.config).unwrap();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp.tools]\nproject_list = false\n",
    )
    .unwrap();
    // 변경이 서버에 닿았음을 목록으로 확인한 뒤, 알림이 오지 않았는지 본다.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while tool_names(&client).await.len() != 4 {
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
    assert_eq!(
        names,
        [
            "mcptest_write",
            "project_info",
            "project_list",
            "project_register",
            "settings_get"
        ]
    );
}

/// `tools/call` 결과의 표지.
fn mark(result: &CallToolResult) -> Option<Mark> {
    result.meta.as_ref().and_then(|m| Mark::read(m))
}

/// 시험용 툴 메서드를 더한 Fixture 와 그 오래 도는 핸들러의 표지.
fn fixture_with_test_tools() -> (Fixture, Probe) {
    let probe = Probe::default();
    (Fixture::with_methods(test_methods(probe.clone())), probe)
}

/// `git worktree add` 가 만드는 모양을 손으로 만든다: `main`(메인 체크아웃)의 연결 워크트리 `wt`. 돌려주는 것은 워크트리 꼭대기.
fn fake_worktree(main: &Path, wt: &Path) -> PathBuf {
    let admin = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::write(admin.join("commondir"), "../..\n").unwrap();
    std::fs::create_dir_all(wt).unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    std::fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    pw_core::registry::path::simplify(std::fs::canonicalize(wt).unwrap())
}

/// ① 미등록 연결 워크트리 안에서 연 세션: 입력에 `worktree` 가 있는 툴이 세션 프로젝트를 겨누면 빈 `worktree` 가 그 워크트리
/// 꼭대기로 채워진다. 준 값은 덮지 않고, 다른 프로젝트를 겨누면 채우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_in_a_linked_worktree_fills_the_worktree_input() {
    let (f, _) = fixture_with_test_tools();
    let top = fake_worktree(&f.work("alpha"), &f.base.join("wt-alpha"));
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(top.clone()),
        })
        .await;
    let top = top.display().to_string();

    let filled = call(&client, "mcptest_echo", json!({})).await;
    let echoed = filled.structured_content.expect("object 결과");
    assert_eq!(echoed["projectId"], json!(f.projects[0]));
    assert_eq!(echoed["worktree"], json!(top));
    for empty in [json!(null), json!("")] {
        let filled = call(&client, "mcptest_echo", json!({ "worktree": empty })).await;
        assert_eq!(filled.structured_content.unwrap()["worktree"], json!(top));
    }

    let given = call(&client, "mcptest_echo", json!({"worktree": "/given"})).await;
    assert_eq!(
        given.structured_content.unwrap()["worktree"],
        json!("/given")
    );

    let other = call(&client, "mcptest_echo", json!({"projectId": f.projects[1]})).await;
    assert_eq!(other.structured_content.unwrap()["worktree"], json!(null));
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 메인 체크아웃에서 연 세션에는 활성 워크트리가 없다 — `worktree` 를 채우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_in_the_main_checkout_leaves_the_worktree_empty() {
    let (f, _) = fixture_with_test_tools();
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(f.work("alpha")),
        })
        .await;
    let echoed = call(&client, "mcptest_echo", json!({})).await;
    assert_eq!(echoed.structured_content.unwrap()["worktree"], json!(null));
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// ② 응답 한 건이 256 KiB 를 넘으면 「범위를 좁혀라」 오류다. 감사 행은 `error`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_response_becomes_a_narrow_the_request_error() {
    let (f, _) = fixture_with_test_tools();
    let (client, _, cancel) = f.connect(SessionRequest::default()).await;
    let big = call(&client, "mcptest_big", json!({})).await;
    assert_eq!(big.is_error, Some(true));
    assert!(text(&big).contains("narrow the request"), "{}", text(&big));
    assert!(text(&big).len() < 1024, "잘린 결과도 싣지 않는다");
    assert_eq!(mark(&big), Some(Mark::error(Some(Reason::TooLarge))));
    let rows = audit_rows(&f.paths.state, 1);
    assert_eq!(
        (rows[0].1.as_str(), rows[0].2.as_str()),
        ("mcptest_big", "error")
    );
    assert_eq!(pw_mcp::server::MAX_RESPONSE_BYTES, 256 * 1024);
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 감사 상세(「툴 메서드 규약」, 16-m5-plan M5-07 ⑦): 결과의 최상위 `audit` 글은 성공 행의 상세로, 오류 `data` 의 `audit` 은
/// 「오류 글 · 상세」 로 간다 — 그때 `data` 의 다른 것(서버 오류 글의 값)은 감사 행에 남지 않는다. 상세는 감사의 상한에서 잘린다.
/// `audit` 이 없는 툴은 지금처럼 성공 행의 상세가 비고 오류 행은 오류 글(`data` 포함) 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_top_level_audit_text_goes_to_the_audit_detail() {
    let (f, _) = fixture_with_test_tools();
    let (client, _, cancel) = f.connect(SessionRequest::default()).await;
    let ok = call(
        &client,
        "mcptest_echo",
        json!({ "projectId": f.projects[0], "audit": "statements=2 grade=read" }),
    )
    .await;
    assert_ne!(ok.is_error, Some(true));
    assert_eq!(
        ok.structured_content.as_ref().unwrap()["audit"],
        "statements=2 grade=read",
        "the agent sees it too"
    );
    let long = "a".repeat(2000);
    call(
        &client,
        "mcptest_echo",
        json!({ "projectId": f.projects[0], "audit": long }),
    )
    .await;
    call(
        &client,
        "mcptest_echo",
        json!({ "projectId": f.projects[0] }),
    )
    .await;
    let denied = pw_rpc::code::PERMISSION_DENIED;
    for (code, reason) in [(denied, Some("notAQuery")), (3003, None)] {
        let failed = call(
            &client,
            "mcptest_refuse",
            json!({ "code": code, "reason": reason, "audit": "statements=1 grade=write" }),
        )
        .await;
        assert_eq!(failed.is_error, Some(true));
    }
    call(
        &client,
        "mcptest_refuse",
        json!({ "code": 3003, "reason": "plain" }),
    )
    .await;
    let rows = audit_rows(&f.paths.state, 6);
    let details: Vec<Option<&str>> = rows.iter().map(|r| r.3.as_deref()).collect();
    assert_eq!(details[0], Some("statements=2 grade=read"));
    let clipped = details[1].unwrap();
    assert!(
        clipped.starts_with("aaa") && clipped.chars().count() <= pw_mcp::audit::MAX_DETAIL + 1,
        "{} chars",
        clipped.chars().count()
    );
    assert_eq!(
        details[2], None,
        "a tool without `audit` leaves the detail empty"
    );
    assert_eq!(
        details[3],
        Some(format!("refused ({denied}) · statements=1 grade=write").as_str())
    );
    assert_eq!(
        details[4],
        Some("refused (3003) · statements=1 grade=write")
    );
    for detail in &details[3..5] {
        assert!(!detail.unwrap().contains(DATA_ONLY), "{detail:?}");
    }
    assert!(
        details[5].unwrap().contains("plain"),
        "without `audit` the error keeps its data: {:?}",
        details[5]
    );
    let outcomes: Vec<&str> = rows.iter().map(|r| r.2.as_str()).collect();
    assert_eq!(outcomes, ["ok", "ok", "ok", "denied", "error", "error"]);
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// ③ 성공 · 거절 · 노출 안 됨 · 툴 오류 · 취소가 결과 표지로 갈린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_results_carry_an_outcome_mark() {
    let (f, _) = fixture_with_test_tools();
    std::fs::create_dir_all(&f.paths.config).unwrap();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp.tools]\nmcptest_slow = false\n",
    )
    .unwrap();
    f.host.config().unwrap().global.reload().unwrap();
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(f.work("beta")),
        })
        .await;

    let ok = call(&client, "project_list", json!({})).await;
    assert_eq!(mark(&ok), Some(Mark::ok()));
    let structured = call(&client, "project_info", json!({})).await;
    assert_eq!(mark(&structured), Some(Mark::ok()));

    let denied = call(&client, "mcptest_write", json!({})).await;
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(mark(&denied), Some(Mark::denied(Reason::NeedsApproval)));

    let failed = call(&client, "project_info", json!({"projectId": "nope"})).await;
    assert_eq!(failed.is_error, Some(true));
    assert_eq!(mark(&failed), Some(Mark::error(None)));

    let cancelled = call(&client, "mcptest_host_cancelled", json!({})).await;
    assert_eq!(cancelled.is_error, Some(true));
    assert_eq!(mark(&cancelled), Some(Mark::error(Some(Reason::Cancelled))));

    // 노출 안 된 툴(꺼진 툴 · 모르는 이름)은 JSON-RPC invalid params 이고 그 `data` 에 같은 표지가 있다.
    for name in ["mcptest_slow", "no_such_tool"] {
        let hidden = client
            .peer()
            .call_tool(CallToolRequestParams::new(name))
            .await;
        let Err(ServiceError::McpError(error)) = hidden else {
            panic!("노출 안 된 툴이 불렸다: {hidden:?}");
        };
        let data = error.data.expect("오류 data");
        assert_eq!(
            Mark::read(data.as_object().unwrap()),
            Some(Mark::denied(Reason::NotExposed)),
            "{name}"
        );
    }
    let rows = audit_rows(&f.paths.state, 7);
    let outcomes: Vec<_> = rows.iter().map(|r| (r.1.as_str(), r.2.as_str())).collect();
    assert_eq!(
        outcomes,
        [
            ("project_list", "ok"),
            ("project_info", "ok"),
            ("mcptest_write", "denied"),
            ("project_info", "error"),
            ("mcptest_host_cancelled", "error"),
            ("mcptest_slow", "denied"),
            ("no_such_tool", "denied"),
        ]
    );
    assert_eq!(rows[4].3.as_deref(), Some("cancelled"));
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 툴의 Host RPC 가 `PERMISSION_DENIED` 로 답하면 결과는 `denied` 이고(CLI 종료 3) 까닭은 `data.reason` 의 코드다 — 모르거나
/// 없으면 `permission_denied`. 다른 오류(DB 의 비밀이 필요하다 3101 등)는 그대로 `error` 다(16-m5-plan M5-06, 메인 판정).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_permission_denied_tool_error_is_a_denied_result() {
    let (f, _) = fixture_with_test_tools();
    let (client, _, cancel) = f.connect(SessionRequest::default()).await;
    let denied = pw_rpc::code::PERMISSION_DENIED;
    let cases = [
        (
            denied,
            Some("notOpenedByUser"),
            Mark::denied(Reason::NotOpenedByUser),
        ),
        (denied, Some("untrusted"), Mark::denied(Reason::Untrusted)),
        (
            denied,
            Some("productionBlocked"),
            Mark::denied(Reason::ProductionBlocked),
        ),
        (
            denied,
            Some("fromTheFuture"),
            Mark::denied(Reason::PermissionDenied),
        ),
        (denied, None, Mark::denied(Reason::PermissionDenied)),
        (3101, Some("notOpenedByUser"), Mark::error(None)),
        (pw_rpc::code::INVALID_PARAMS, None, Mark::error(None)),
    ];
    for (code, reason, expected) in cases {
        let result = call(
            &client,
            "mcptest_refuse",
            json!({ "code": code, "reason": reason }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(mark(&result), Some(expected), "{code} {reason:?}");
    }
    let rows = audit_rows(&f.paths.state, cases.len());
    let outcomes: Vec<&str> = rows.iter().map(|r| r.2.as_str()).collect();
    assert_eq!(
        outcomes,
        [
            "denied", "denied", "denied", "denied", "denied", "error", "error"
        ]
    );
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// ④ 세션 프로젝트 alpha 에서 beta 의 `projectId` 로 부르면 beta 의 `[mcp]` 로도 판정한다(결정 M5-D28). beta 가 끈 툴은 거절되고
/// 감사 로그에 `denied`. beta 의 구버전(v0) `project.toml` 은 바이트 그대로이고 `.backup/` 도 생기지 않는다. 설정이 없는 gamma 는
/// 세션의 노출만 따른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_into_another_project_also_needs_that_projects_exposure() {
    let f = Fixture::new();
    let beta = f.work("beta");
    std::fs::create_dir_all(beta.join(".pitwall")).unwrap();
    std::fs::write(
        beta.join(".pitwall/project.toml"),
        "# hand written v0\n[mcp.tools]\nproject_info = false\n",
    )
    .unwrap();
    let before = tree(&beta);
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(f.work("alpha")),
        })
        .await;
    // 세션(alpha)에서는 보인다.
    assert!(
        tool_names(&client)
            .await
            .contains(&"project_info".to_owned())
    );

    let into_beta = call(&client, "project_info", json!({"projectId": f.projects[1]})).await;
    assert_eq!(into_beta.is_error, Some(true));
    assert_eq!(mark(&into_beta), Some(Mark::denied(Reason::NotExposed)));
    assert!(
        text(&into_beta).contains(&f.projects[1]),
        "{}",
        text(&into_beta)
    );

    let into_gamma = call(&client, "project_info", json!({"projectId": f.projects[2]})).await;
    assert_eq!(mark(&into_gamma), Some(Mark::ok()));
    let own = call(&client, "project_info", json!({})).await;
    assert_eq!(mark(&own), Some(Mark::ok()));

    let rows = audit_rows(&f.paths.state, 3);
    assert_eq!(
        (rows[0].0.as_deref(), rows[0].1.as_str(), rows[0].2.as_str()),
        (Some(f.projects[1].as_str()), "project_info", "denied")
    );
    client.cancel().await.unwrap();
    cancel.cancel();
    assert_eq!(tree(&beta), before, "대상 프로젝트의 설정 폴더가 바뀌었다");
    assert!(!beta.join(".pitwall/.backup").exists());
}

/// 대상 프로젝트의 커밋된 설정도 좁히기만 한다 — 세션이 끈 툴을 대상 프로젝트가 켜도 부를 수 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_projects_config_cannot_widen_the_session_exposure() {
    let f = Fixture::new();
    let alpha = f.work("alpha/.pitwall");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::write(
        alpha.join("project.local.toml"),
        "schema = 1\n[mcp.tools]\nproject_info = false\n",
    )
    .unwrap();
    let beta = f.work("beta/.pitwall");
    std::fs::create_dir_all(&beta).unwrap();
    std::fs::write(
        beta.join("project.local.toml"),
        "schema = 1\n[mcp.tools]\nproject_info = true\n",
    )
    .unwrap();
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(f.work("alpha")),
        })
        .await;
    let hidden = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("project_info").with_arguments(
                json!({"projectId": f.projects[1]})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await;
    assert!(hidden.is_err(), "세션이 끈 툴이 불렸다: {hidden:?}");
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 감사 행 중 `tool` 의 것이 나올 때까지 기다린다.
fn audit_row_of(state: &Path, tool: &str) -> (Option<String>, String, String, Option<String>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(conn) = rusqlite_open(&state.join("mcp/audit.db"))
            && let Some(row) = read_rows(&conn).into_iter().find(|r| r.1 == tool)
        {
            return row;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no audit row for {tool}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

async fn wait_dropped(probe: &Probe, within: Duration) {
    let deadline = tokio::time::Instant::now() + within;
    while !probe.dropped.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "Host 의 핸들러 future 가 버려지지 않았다"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn slow_call() -> ClientRequest {
    ClientRequest::CallToolRequest(CallToolRequest::new(CallToolRequestParams::new(
        "mcptest_slow",
    )))
}

/// ⑤ MCP 의 취소 알림이 오면 그 호출의 Host RPC 를 거둔다 — Host 의 핸들러 future 가 버려지고(세션은 그대로 돈다) 감사 행은
/// 결과 `error` · 까닭 `cancelled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_call_drops_the_host_handler() {
    let (f, probe) = fixture_with_test_tools();
    let (client, _, cancel) = f.connect(SessionRequest::default()).await;
    let handle = client
        .peer()
        .send_cancellable_request(slow_call(), PeerRequestOptions::no_options())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), probe.started.notified())
        .await
        .expect("가짜 핸들러가 시작하지 않았다");
    handle.cancel(Some("test".into())).await.unwrap();
    wait_dropped(&probe, Duration::from_secs(5)).await;

    // 세션은 살아 있다 — 버려진 것은 그 호출 하나다.
    let list = call(&client, "project_list", json!({})).await;
    assert_eq!(mark(&list), Some(Mark::ok()));
    let row = audit_row_of(&f.paths.state, "mcptest_slow");
    assert_eq!(
        (row.2.as_str(), row.3.as_deref()),
        ("error", Some("cancelled"))
    );
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// ⑤ 세션이 끊겨도 같다: 돌던 호출의 Host 핸들러가 버려지고 감사 행이 `error` · `cancelled` 로 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_session_cancels_its_running_calls() {
    let (f, probe) = fixture_with_test_tools();
    let (client, _, _cancel) = f.connect(SessionRequest::default()).await;
    let _handle = client
        .peer()
        .send_cancellable_request(slow_call(), PeerRequestOptions::no_options())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), probe.started.notified())
        .await
        .expect("가짜 핸들러가 시작하지 않았다");
    client.cancel().await.unwrap();
    // rmcp 는 끊긴 뒤 남은 응답을 5초까지 기다렸다가 핸들러의 취소 토큰을 건다.
    wait_dropped(&probe, Duration::from_secs(15)).await;
    let row = audit_row_of(&f.paths.state, "mcptest_slow");
    assert_eq!(
        (row.2.as_str(), row.3.as_deref()),
        ("error", Some("cancelled"))
    );
}

/// ⑥ 헤드리스 Host 에 붙은 세션의 안내문에 「앱을 켜면 다시 시작하지 않아도 나머지 툴이 나온다」 가 있고 앱 세션에는 없다. 헤드리스의 `write` 이상
/// 툴 거절은 까닭 `headless_write` 다. 판정은 연결 방식이 아니라 `Services::host_kind` 다 — 이 시험의 세션은 둘 다 프로세스 안
/// duplex(`transport: "test"`)로 붙는다(앱 없는 CLI 가 헤드리스 Host 에 붙는 꼴).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_headless_session_tells_the_agent_to_start_the_app() {
    let f = Fixture::new();
    let instructions = |client: &RunningService<RoleClient, Client>| {
        client
            .peer()
            .peer_info()
            .and_then(|info| info.instructions.clone())
            .unwrap_or_default()
    };
    let (headless, _, cancel) = f
        .connect_on(SessionRequest::default(), HostKind::Headless)
        .await;
    assert!(
        instructions(&headless).contains(pw_mcp::server::HEADLESS_NOTICE),
        "{}",
        instructions(&headless)
    );
    assert!(instructions(&headless).contains("Start the Pitwall app"));
    // 넘겨 붙기(M5-20) 뒤로 다시 시작하라고 하지 않는다.
    assert!(!instructions(&headless).contains("restart this MCP server"));
    let write = call(&headless, "mcptest_write", json!({})).await;
    assert_eq!(mark(&write), Some(Mark::denied(Reason::HeadlessWrite)));
    headless.cancel().await.unwrap();
    cancel.cancel();

    let (app, _, cancel) = f.connect_on(SessionRequest::default(), HostKind::App).await;
    assert!(instructions(&app).starts_with("Pitwall IDE"));
    assert!(
        !instructions(&app).contains("restart"),
        "{}",
        instructions(&app)
    );
    let write = call(&app, "mcptest_write", json!({})).await;
    assert_eq!(mark(&write), Some(Mark::denied(Reason::NeedsApproval)));
    app.cancel().await.unwrap();
    cancel.cancel();
}

/// ④ 대상 프로젝트의 커밋된 공유 `project.toml` 은 좁히기만 한다: 전역이 끈 툴을 세션 프로젝트(alpha)는 `.local.toml` 로 켰고,
/// 대상(beta)은 공유 설정으로 켰다 — alpha 로는 불리고 beta 로는 `not_exposed` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_target_projects_shared_config_cannot_widen_its_exposure() {
    let f = Fixture::new();
    std::fs::create_dir_all(&f.paths.config).unwrap();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp.tools]\nproject_info = false\n",
    )
    .unwrap();
    f.host.config().unwrap().global.reload().unwrap();
    let alpha = f.work("alpha/.pitwall");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::write(
        alpha.join("project.local.toml"),
        "schema = 1\n[mcp.tools]\nproject_info = true\n",
    )
    .unwrap();
    let beta = f.work("beta/.pitwall");
    std::fs::create_dir_all(&beta).unwrap();
    std::fs::write(
        beta.join("project.toml"),
        "schema = 1\n[mcp.tools]\nproject_info = true\n",
    )
    .unwrap();
    let (client, _, cancel) = f
        .connect(SessionRequest {
            project: None,
            cwd: Some(f.work("alpha")),
        })
        .await;
    let own = call(&client, "project_info", json!({})).await;
    assert_eq!(mark(&own), Some(Mark::ok()), "{}", text(&own));
    let into_beta = call(&client, "project_info", json!({"projectId": f.projects[1]})).await;
    assert_eq!(
        mark(&into_beta),
        Some(Mark::denied(Reason::NotExposed)),
        "{}",
        text(&into_beta)
    );
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// ④ 프로젝트 없는 세션의 `projectId` 는 언제나 대상 프로젝트다 — 그 프로젝트가 끈 툴은 `not_exposed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_without_a_project_checks_every_target_project() {
    let f = Fixture::new();
    let beta = f.work("beta/.pitwall");
    std::fs::create_dir_all(&beta).unwrap();
    std::fs::write(
        beta.join("project.local.toml"),
        "schema = 1\n[mcp.tools]\nproject_info = false\n",
    )
    .unwrap();
    let (client, _, cancel) = f.connect(SessionRequest::default()).await;
    let into_beta = call(&client, "project_info", json!({"projectId": f.projects[1]})).await;
    assert_eq!(mark(&into_beta), Some(Mark::denied(Reason::NotExposed)));
    let into_alpha = call(&client, "project_info", json!({"projectId": f.projects[0]})).await;
    assert_eq!(mark(&into_alpha), Some(Mark::ok()));
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// ⑤ 세션을 연 쪽이 세션 토큰을 취소하면(앱 종료 · 소켓 서버가 연결을 거둠) 돌던 호출의 Host 핸들러가 곧바로 버려지고 감사 행이
/// `error` · `cancelled` 로 남는다. 클라이언트는 아직 붙어 있다 — 끊김의 5초 drain 을 기다리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_session_token_cancels_its_running_calls() {
    let (f, probe) = fixture_with_test_tools();
    let (client, _, cancel) = f.connect(SessionRequest::default()).await;
    let _handle = client
        .peer()
        .send_cancellable_request(slow_call(), PeerRequestOptions::no_options())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), probe.started.notified())
        .await
        .expect("가짜 핸들러가 시작하지 않았다");
    cancel.cancel();
    wait_dropped(&probe, Duration::from_secs(1)).await;
    let row = audit_row_of(&f.paths.state, "mcptest_slow");
    assert_eq!(
        (row.2.as_str(), row.3.as_deref()),
        ("error", Some("cancelled"))
    );
    drop(client);
}
