//! 넘겨 붙기 층의 시험(16-m5-plan M5-20). 진짜 바이너리로 결정적으로 만들 수 없는 것을 여기서 본다 — 헤드리스의 툴은 모두 곧바로
//! 끝나는 코어 툴이라, 넘기는 도중에 헤드리스에서 돌던 호출을 바이너리로는 만들 수 없다(바이너리에 시험 전용 길을 두지 않는다).
//!
//! 양쪽 모두 진짜 세션이다: 헤드리스 쪽은 `pitwall mcp` 가 쓰는 [`super::serve_headless`] 그대로(느린 가짜 툴을 단 헤드리스 Host),
//! 앱 쪽은 `dial` 이 프로세스 안 duplex 로 연 `pw_mcp::serve`(앱 Host — 소켓 핸드셰이크만 빠진 `connect_app` 자리). 클라이언트는
//! 줄 단위 JSON-RPC 를 손으로 쓴다(클라이언트가 실제로 보내는 줄 그대로).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use pw_core::LocalHost;
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use pw_core::{EventBus, HostRouter};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::{Catalog, HostKind, Services};
use pw_rpc::{ConnectionScope, MethodInfo, RpcError};
use rmcp::model::{GetMeta, JsonRpcMessage, ServerJsonRpcMessage, ServerNotification};
use tokio::io::{AsyncReadExt, DuplexStream};
use tokio::sync::{Notify, mpsc};

use super::*;

mod fake {
    use pw_rpc::{McpTool, Method, MethodKind, MethodMeta, RoleSet, Tier, UnaryMethod};

    /// 손으로 만든 선언 — `rpc_method!` 로 만들면 레지스트리에 들어가 툴 목록 시험(`mcp_tests.rs`)에 섞인다.
    macro_rules! fake {
        ($ty:ident, $method:literal, $name:literal) => {
            #[derive(Debug, Clone, Copy)]
            pub struct $ty;
            impl Method for $ty {
                type Params = ();
                const META: MethodMeta = MethodMeta {
                    name: $method,
                    kind: MethodKind::Unary,
                    tier: Tier::Read,
                    roles: RoleSet::default_for(Tier::Read),
                    params: "",
                    output: "",
                    doc: concat!("Fake tool `", $name, "`."),
                    mcp: Some(McpTool {
                        group: Some("project"),
                        name: Some($name),
                    }),
                    file_scope: false,
                };
            }
            impl UnaryMethod for $ty {
                type Result = ();
            }
        };
    }

    // 헤드리스 Host 가 처리한다: 풀어 줄 때까지 돈다.
    fake!(Slow, "handover.slow", "handover_slow");

    #[derive(
        Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, schemars::JsonSchema,
    )]
    pub struct WriteParams {}

    /// 앱 Host 만 처리하는 쓰기 툴: 계획을 만들고 승인 표가 맞을 때만 돈다(16-m5-plan M5-12).
    #[derive(Debug, Clone, Copy)]
    pub struct Write;
    impl Method for Write {
        type Params = pw_core::registry::plan::Planned<WriteParams>;
        const META: MethodMeta = MethodMeta {
            name: "handover.write",
            kind: MethodKind::Unary,
            tier: Tier::Write,
            roles: RoleSet::default_for(Tier::Write),
            params: "",
            output: "",
            doc: "Fake tool `handover_write`.",
            mcp: Some(McpTool {
                group: Some("project"),
                name: Some("handover_write"),
            }),
            file_scope: false,
        };
    }
    impl UnaryMethod for Write {
        type Result = ();
    }
    // 앱 Host 만 처리한다: 부른 연결의 범위를 적는다.
    fake!(AppOnly, "handover.appOnly", "handover_app_only");
}

const SLOW: &str = "handover_slow";
const APP_ONLY: &str = "handover_app_only";
const WRITE: &str = "handover_write";

/// 헤드리스 · 앱 두 Host 와 그 세션 자리. 둘은 같은 `$STATE`(같은 감사 로그 파일)를 쓴다 — 진짜 `pitwall mcp` 와 앱처럼.
struct Fixture {
    _tmp: tempfile::TempDir,
    paths: AppPaths,
    headless: Arc<Services>,
    app: Arc<Services>,
    started: Arc<Notify>,
    release: Arc<Notify>,
    /// 앱 전용 툴을 부른 연결의 범위.
    scopes: Arc<Mutex<Vec<ConnectionScope>>>,
    /// 앱 전용 툴을 부른 Host 연결(`Ctx::session`) — 차례대로.
    host_sessions: Arc<Mutex<Vec<pw_rpc::SessionId>>>,
    /// 켜면 `dial` 이 앱 세션을 연다(앱이 뜬 꼴).
    app_up: Arc<AtomicBool>,
    dials: Arc<AtomicUsize>,
    cancel: CancellationToken,
    /// 앱 쪽 세션 등록부 — 그 승인 허브를 창 자리에서 듣는다.
    app_sessions: pw_mcp::sessions::Sessions,
    /// 승인을 지나 돈 쓰기 툴 수.
    writes: Arc<AtomicUsize>,
}

fn linked() -> Catalog {
    let fakes = [
        MethodInfo::unary::<fake::Slow>(),
        MethodInfo::unary::<fake::AppOnly>(),
        MethodInfo::unary::<fake::Write>(),
    ];
    Catalog::from_methods(fakes.iter().chain(pw_rpc::methods())).unwrap()
}

/// 앱 · 헤드리스 세션 자리. 승인 길은 `mcp::services` 와 같은 규칙이다(`Approval::for_host` — 앱만 그 등록부의 허브).
fn services(
    paths: &AppPaths,
    host: &Arc<LocalHost>,
    kind: HostKind,
    sessions: pw_mcp::sessions::Sessions,
) -> Arc<Services> {
    let router = Arc::new(HostRouter::new());
    router.add_host(host.clone()).unwrap();
    Arc::new(Services {
        router,
        host_id: crate::LOCAL_HOST_ID.into(),
        paths: paths.clone(),
        global: host.config().unwrap().global.clone(),
        bus: host.bus().clone(),
        audit: Arc::new(AuditLog::under_state(&paths.state)),
        catalog: Arc::new(linked().routed_by(host.router())),
        approval: Approval::for_host(kind, &sessions),
        transport: "test",
        host_kind: kind,
        sessions,
    })
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("settings.toml"), "schema = 1\n").unwrap();
        let registry = Registry::new(paths.clone(), None, EventBus::new(16));
        let root = tmp.path().join("work").join("alpha");
        std::fs::create_dir_all(&root).unwrap();
        registry
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap();
        drop(registry);

        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (s, r) = (started.clone(), release.clone());
        let headless_host = Arc::new(
            LocalHost::builder(crate::LOCAL_HOST_ID)
                .config(paths.clone())
                .read_only_registry()
                .methods(move |router| {
                    router.unary(fake::Slow, move |_ctx, ()| {
                        let (started, release) = (s.clone(), r.clone());
                        async move {
                            started.notify_one();
                            release.notified().await;
                            Ok::<(), RpcError>(())
                        }
                    });
                })
                .build(),
        );
        let scopes = Arc::new(Mutex::new(Vec::new()));
        let seen = scopes.clone();
        let host_sessions = Arc::new(Mutex::new(Vec::new()));
        let seen_sessions = host_sessions.clone();
        let writes = Arc::new(AtomicUsize::new(0));
        let wrote = writes.clone();
        let app_host = Arc::new(
            LocalHost::builder(crate::LOCAL_HOST_ID)
                .config(paths.clone())
                .methods(move |router| {
                    router.unary(fake::AppOnly, move |ctx, ()| {
                        seen.lock().unwrap().push(ctx.scope().clone());
                        seen_sessions.lock().unwrap().push(ctx.session());
                        async { Ok::<(), RpcError>(()) }
                    });
                    router.unary(
                        fake::Write,
                        move |_ctx, p: pw_core::registry::plan::Planned<fake::WriteParams>| {
                            let plan = pw_core::registry::plan::ToolPlan::builder(
                                "handover.write",
                                pw_rpc::Tier::Write,
                                &p.params,
                            )
                            .summary("write")
                            .build();
                            let wrote = wrote.clone();
                            async move {
                                p.approve(plan)?;
                                wrote.fetch_add(1, Ordering::SeqCst);
                                Ok::<(), RpcError>(())
                            }
                        },
                    );
                })
                .build(),
        );
        // 앱만 설정을 따라간다 — 넘긴 뒤 앱의 목록 변경이 옮겨진 구독으로 가는지 본다.
        app_host
            .watch_config(&tokio::runtime::Handle::current())
            .unwrap();
        let app_sessions = pw_mcp::sessions::Sessions::new();
        Self {
            headless: services(
                &paths,
                &headless_host,
                HostKind::Headless,
                pw_mcp::sessions::Sessions::new(),
            ),
            app: services(&paths, &app_host, HostKind::App, app_sessions.clone()),
            app_sessions,
            writes,
            _tmp: tmp,
            paths,
            started,
            release,
            scopes,
            host_sessions,
            app_up: Arc::default(),
            dials: Arc::default(),
            cancel: CancellationToken::new(),
        }
    }

    /// `pitwall mcp` 의 `dial_app` 자리: 앱이 떠 있으면 앱 세션에 붙은 연결.
    fn dial(&self) -> impl FnMut() -> std::future::Ready<Dial<DuplexStream>> + use<> {
        let (app, up, dials, cancel) = (
            self.app.clone(),
            self.app_up.clone(),
            self.dials.clone(),
            self.cancel.clone(),
        );
        move || {
            dials.fetch_add(1, Ordering::SeqCst);
            if !up.load(Ordering::SeqCst) {
                return std::future::ready(Dial::Absent);
            }
            let (ours, theirs) = tokio::io::duplex(64 * 1024);
            tokio::spawn(pw_mcp::serve(
                app.clone(),
                SessionRequest::default(),
                tokio::io::split(theirs),
                cancel.child_token(),
            ));
            std::future::ready(Dial::Connected(ours))
        }
    }

    /// `pitwall mcp` 를 띄운다: 클라이언트와 `serve_headless` 의 결과.
    fn start(&self) -> (Client, tokio::task::JoinHandle<Outcome>) {
        let (client_out, stdin) = tokio::io::duplex(64 * 1024);
        let (stdout, client_in) = tokio::io::duplex(64 * 1024);
        let running = tokio::spawn(serve_headless(
            self.headless.clone(),
            SessionRequest::default(),
            stdin,
            stdout,
            self.dial(),
        ));
        let client = Client {
            write: Some(client_out),
            read: BufReader::new(client_in),
        };
        (client, running)
    }

    /// 감사 로그의 (툴, 세션). 기다리지 않는다.
    fn audit(&self) -> Vec<(String, String)> {
        let file = self.paths.state.join("mcp").join("audit.db");
        let Ok(conn) = rusqlite::Connection::open_with_flags(
            &file,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare("SELECT tool, session FROM calls ORDER BY id") else {
            return Vec::new();
        };
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// 그 툴의 감사 행이 생길 때까지 기다려 그 세션.
    async fn audit_session(&self, tool: &str) -> String {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some((_, session)) = self.audit().into_iter().find(|(t, _)| t == tool) {
                return session;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{tool} 의 감사 행이 없다"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct Client {
    write: Option<DuplexStream>,
    read: BufReader<DuplexStream>,
}

impl Client {
    async fn send(&mut self, message: Value) {
        let mut line = serde_json::to_vec(&message).unwrap();
        line.push(b'\n');
        let write = self.write.as_mut().unwrap();
        write.write_all(&line).await.unwrap();
        write.flush().await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        let n = tokio::time::timeout(Duration::from_secs(10), self.read.read_line(&mut line))
            .await
            .expect("pitwall mcp 가 10초 안에 아무것도 쓰지 않았다")
            .unwrap();
        assert!(n > 0, "pitwall mcp 가 stdout 을 닫았다");
        serde_json::from_str(&line).unwrap()
    }

    /// 그 id 의 응답이 올 때까지 읽는다. 그 사이의 알림 · 다른 응답도 함께 돌려준다.
    async fn until(&mut self, id: i64) -> (Value, Vec<Value>) {
        let mut before = Vec::new();
        loop {
            let message = self.recv().await;
            if message.get("method").is_none() && message["id"] == json!(id) {
                return (message, before);
            }
            before.push(message);
        }
    }

    async fn until_after(&mut self, message: Value, id: i64) -> (Value, Vec<Value>) {
        self.send(message).await;
        self.until(id).await
    }

    fn close(&mut self) {
        self.write = None;
    }

    /// 2025-11-25 클라이언트의 핸드셰이크.
    async fn initialize(&mut self) {
        self.send(json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "handover-test", "version": "1.0"},
            },
        }))
        .await;
        let (answer, _) = self.until(0).await;
        assert!(answer.get("result").is_some(), "{answer}");
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
    }
}

fn tool_names(answer: &Value) -> Vec<String> {
    answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list 의 결과가 아니다: {answer}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

fn is_list_changed(message: &Value) -> bool {
    message["method"] == json!("notifications/tools/list_changed")
}

/// 넘기는 도중에 헤드리스에서 돌던 호출은 헤드리스에서 끝나 응답을 잃지 않고, 그 감사 행은 헤드리스 세션의 것으로 남는다(층이
/// 돌아오기 전에 쓰였다). 넘긴 뒤 목록 · 호출은 앱 세션의 것이고, 앱 세션은 에이전트 범위로 열린다(M5-02). 앱 찾기는 넘기기 전의
/// `tools/list` · `tools/call` 요청마다 한 번이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_running_during_the_handover_keeps_its_answer() {
    let f = Fixture::new();
    let (mut client, running) = f.start();
    client.initialize().await;
    assert_eq!(
        f.dials.load(Ordering::SeqCst),
        0,
        "initialize 는 앱을 찾지 않는다"
    );

    client
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .await;
    let (list, _) = client.until(1).await;
    let names = tool_names(&list);
    assert!(names.contains(&SLOW.to_owned()), "{names:?}");
    assert!(
        !names.contains(&APP_ONLY.to_owned()),
        "헤드리스 목록이다: {names:?}"
    );
    assert_eq!(f.dials.load(Ordering::SeqCst), 1);

    // 헤드리스에서 오래 도는 호출.
    client
        .send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": SLOW, "arguments": {}}}))
        .await;
    tokio::time::timeout(Duration::from_secs(10), f.started.notified())
        .await
        .expect("느린 툴이 시작하지 않았다");
    assert_eq!(f.dials.load(Ordering::SeqCst), 2);
    client
        .send(json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}))
        .await;
    client.until(3).await;
    assert_eq!(
        f.dials.load(Ordering::SeqCst),
        2,
        "ping 은 앱을 찾지 않는다"
    );

    // 앱이 뜬다. 다음 요청에서 넘겨 붙고 `tools/list_changed` 가 먼저 온다.
    f.app_up.store(true, Ordering::SeqCst);
    client
        .send(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"}))
        .await;
    let (list, before) = client.until(4).await;
    assert_eq!(f.dials.load(Ordering::SeqCst), 3);
    assert!(
        before.iter().any(is_list_changed),
        "tools/list_changed 가 오지 않았다: {before:?}"
    );
    // 앱이 다시 받은 `initialize` 의 응답(id 0)은 클라이언트에 오지 않는다 — 같은 id 의 응답이 두 번이 된다.
    assert!(
        before.iter().all(|m| m.get("method").is_some()),
        "넘기는 사이에 응답이 새었다: {before:?}"
    );
    let names = tool_names(&list);
    assert!(
        names.contains(&APP_ONLY.to_owned()),
        "앱 목록이 아니다: {names:?}"
    );
    assert!(
        !names.contains(&SLOW.to_owned()),
        "앱 목록이 아니다: {names:?}"
    );

    // 앱 세션으로 간다 — 에이전트 범위.
    client
        .send(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": APP_ONLY, "arguments": {}}}))
        .await;
    let (answer, _) = client.until(5).await;
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
    assert_eq!(*f.scopes.lock().unwrap(), [ConnectionScope::Agent]);
    client
        .send(json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "project_list", "arguments": {}}}))
        .await;
    client.until(6).await;
    assert_eq!(
        f.dials.load(Ordering::SeqCst),
        3,
        "넘긴 뒤에는 앱을 찾지 않는다"
    );

    // 헤드리스의 호출은 넘긴 뒤 rmcp 의 drain(입력이 닫힌 세션이 남은 핸들러를 기다리는 5초)보다 오래 돈다 — 넘길 때 헤드리스의
    // 입력을 곧바로 닫으면 그 사이에 버려져 응답이 오지 않는다. 이제 끝난다 — 응답이 클라이언트에 온다.
    tokio::time::sleep(Duration::from_secs(6)).await;
    f.release.notify_one();
    let (answer, _) = client.until(2).await;
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");

    let app_session = f.audit_session("project_list").await;
    assert_eq!(f.audit_session(APP_ONLY).await, app_session);
    let app_sessions: Vec<(String, u64)> = f
        .app
        .sessions
        .list()
        .into_iter()
        .map(|s| (s.id, s.calls))
        .collect();
    assert_eq!(
        app_sessions,
        [(app_session.clone(), 2)],
        "감사 행이 앱 세션의 것이고, 앱 세션은 넘긴 뒤의 호출 둘만 센다"
    );
    let app_client = f.app.sessions.list()[0].client.clone();
    assert_eq!(
        app_client.as_deref(),
        Some("handover-test 1.0"),
        "앱 세션은 클라이언트의 initialize 로 열렸다"
    );

    client.close();
    let outcome = tokio::time::timeout(Duration::from_secs(15), running)
        .await
        .expect("pitwall mcp 가 끝나지 않았다")
        .unwrap();
    assert_eq!(outcome.end.unwrap(), End::ClientClosed);
    outcome.session.unwrap();
    // 돌아왔을 때 헤드리스 세션은 이미 끝났다(등록부에서 빠지는 것은 감사 기록을 다 쓴 뒤다).
    assert!(
        f.headless.sessions.list().is_empty(),
        "헤드리스 세션이 아직 돈다"
    );
    // 기다리지 않고 읽는다 — 헤드리스 세션은 감사 기록을 다 쓴 뒤에 끝났다.
    let rows = f.audit();
    let slow = rows
        .iter()
        .find(|(tool, _)| tool == SLOW)
        .unwrap_or_else(|| panic!("헤드리스에서 끝난 호출의 감사 행이 없다: {rows:?}"));
    assert_ne!(
        slow.1, app_session,
        "헤드리스의 호출은 헤드리스 세션의 것이다"
    );
}

/// 넘긴 뒤 헤드리스 호출이 아직 도는 중에 클라이언트가 닫아도, `serve_headless` 는 그 호출이 끝나고 헤드리스 세션이 감사 기록을 다
/// 쓴 뒤에 돌아온다 — 돌아온 뒤 곧바로 프로세스가 끝나도 행이 남는다(`crates/pw-mcp/tests/audit_flush.rs` 가 지킨 결함).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_during_a_headless_call_waits_for_its_audit_row() {
    let f = Fixture::new();
    let (mut client, running) = f.start();
    client.initialize().await;
    client
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": SLOW, "arguments": {}}}))
        .await;
    tokio::time::timeout(Duration::from_secs(10), f.started.notified())
        .await
        .expect("느린 툴이 시작하지 않았다");
    f.app_up.store(true, Ordering::SeqCst);
    let (list, _) = client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            2,
        )
        .await;
    assert!(tool_names(&list).contains(&APP_ONLY.to_owned()));

    client.close();
    let release = f.release.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        release.notify_one();
    });
    let outcome = tokio::time::timeout(Duration::from_secs(15), running)
        .await
        .expect("pitwall mcp 가 끝나지 않았다")
        .unwrap();
    assert_eq!(outcome.end.unwrap(), End::ClientClosed);
    assert!(
        f.headless.sessions.list().is_empty(),
        "헤드리스 세션이 아직 돈다"
    );
    let rows = f.audit();
    assert!(
        rows.iter().any(|(tool, _)| tool == SLOW),
        "헤드리스에서 끝난 호출의 감사 행이 없다: {rows:?}"
    );
}

/// 2026-07-28 클라이언트(`initialize` 없이 요청마다 `_meta`): 헤드리스에 연 `subscriptions/listen` 이 같은 id 로 앱에 옮겨지고,
/// 그 구독 id 를 실은 `tools/list_changed` 가 온다(rmcp 의 클라이언트 꼴로 읽힌다). 앱의 두 번째 수락 알림은 오지 않고, 그 뒤 앱의
/// 툴 목록이 바뀌면 옮겨진 구독으로 알림이 온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_2026_subscription_moves_to_the_app() {
    let f = Fixture::new();
    let (mut client, running) = f.start();
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "handover-2026", "version": "2.0"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    client
        .send(json!({
            "jsonrpc": "2.0", "id": "listen-1", "method": "subscriptions/listen",
            "params": {"_meta": meta, "notifications": {"toolsListChanged": true}},
        }))
        .await;
    let ack = client.recv().await;
    assert_eq!(
        ack["method"],
        json!("notifications/subscriptions/acknowledged"),
        "{ack}"
    );

    f.app_up.store(true, Ordering::SeqCst);
    client
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta}}))
        .await;
    let (list, before) = client.until(1).await;
    let names = tool_names(&list);
    assert!(
        names.contains(&APP_ONLY.to_owned()),
        "앱 목록이 아니다: {names:?}"
    );
    let changed: Vec<&Value> = before.iter().filter(|m| is_list_changed(m)).collect();
    assert_eq!(changed.len(), 1, "{before:?}");
    let parsed: ServerJsonRpcMessage = serde_json::from_value(changed[0].clone()).unwrap();
    let JsonRpcMessage::Notification(notification) = parsed else {
        panic!("알림이 아니다: {}", changed[0]);
    };
    assert!(matches!(
        notification.notification,
        ServerNotification::ToolListChangedNotification(_)
    ));
    assert_eq!(
        notification.notification.get_meta().subscription_id(),
        Some(rmcp::model::RequestId::String("listen-1".into()))
    );
    assert!(
        !before
            .iter()
            .any(|m| m["method"] == json!("notifications/subscriptions/acknowledged")),
        "앱의 수락 알림이 클라이언트에 갔다: {before:?}"
    );

    // 구독이 앱에 있다: 앱의 툴 목록이 바뀌면 그 구독 id 로 알림이 온다(헤드리스는 설정을 따라가지 않는다).
    std::fs::write(
        f.paths.config.join("settings.toml"),
        format!("schema = 1\n[mcp.tools]\n{APP_ONLY} = false\n"),
    )
    .unwrap();
    let changed = loop {
        let message = client.recv().await;
        if is_list_changed(&message) {
            break message;
        }
    };
    assert_eq!(
        changed["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        json!("listen-1"),
        "{changed}"
    );

    client.close();
    let outcome = tokio::time::timeout(Duration::from_secs(15), running)
        .await
        .expect("pitwall mcp 가 끝나지 않았다")
        .unwrap();
    assert_eq!(outcome.end.unwrap(), End::ClientClosed);
    // 클라이언트에 남은 줄 가운데 헤드리스가 옮긴 구독에 낸 최종 결과는 없다.
    let mut rest = String::new();
    client.read.read_to_string(&mut rest).await.unwrap();
    assert!(!rest.contains("\"error\""), "닫힌 뒤 오류가 나갔다: {rest}");
}

/// 넘긴 뒤 앱이 닫히면 앱이 받은 요청마다 `-32001` 오류가 나가고 [`End::AppClosed`] 다(`pw_mcp::proxy` 와 같은 꼴).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_closing_after_the_handover_answers_its_pending_requests() {
    let (client_out, stdin) = tokio::io::duplex(4096);
    let (mut stdout, client_in) = tokio::io::duplex(4096);
    let (headless_ours, headless_theirs) = tokio::io::duplex(4096);
    let (app_ours, app_theirs) = tokio::io::duplex(4096);
    let mut app = Some(app_ours);
    let running = tokio::spawn(async move {
        run(stdin, &mut stdout, headless_ours, move || {
            std::future::ready(app.take().map_or(Dial::Absent, Dial::Connected))
        })
        .await
    });
    let mut client = Client {
        write: Some(client_out),
        read: BufReader::new(client_in),
    };
    // 헤드리스는 입력이 닫힐 때까지 읽기만 하고 닫는다 — 이 시험의 요청은 넘기게 한 요청부터 앱이 받는다.
    tokio::spawn(async move {
        let mut headless = headless_theirs;
        let mut sink = Vec::new();
        let _ = headless.read_to_end(&mut sink).await;
    });
    let (app_read, mut app_write) = tokio::io::split(app_theirs);
    let mut app_read = BufReader::new(app_read);
    client
        .send(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "x"}}))
        .await;
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), app_read.read_line(&mut line))
        .await
        .expect("넘기게 한 요청이 앱에 가지 않았다")
        .unwrap();
    assert!(line.contains("\"id\":7"), "{line}");
    app_write.shutdown().await.unwrap();
    drop((app_read, app_write));

    let answer = client.recv().await;
    assert_eq!(answer["id"], json!(7));
    assert_eq!(answer["error"]["code"], json!(APP_CLOSED));
    assert_eq!(
        running.await.unwrap().unwrap(),
        End::AppClosed { pending: 1 }
    );
}

/// 넘기기 전에 헤드리스에서 시작된 호출의 취소 알림은 넘긴 뒤에도 헤드리스로 간다 — 그 호출의 감사 행은 헤드리스 세션의
/// `cancelled` 다(앱으로 가면 헤드리스의 호출은 풀릴 때까지 돌아 행이 생기지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_for_a_headless_call_reaches_the_headless_session() {
    let f = Fixture::new();
    let (mut client, running) = f.start();
    client.initialize().await;
    client
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": SLOW, "arguments": {}}}))
        .await;
    tokio::time::timeout(Duration::from_secs(10), f.started.notified())
        .await
        .expect("느린 툴이 시작하지 않았다");
    f.app_up.store(true, Ordering::SeqCst);
    let (list, _) = client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            2,
        )
        .await;
    assert!(tool_names(&list).contains(&APP_ONLY.to_owned()));

    client
        .send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}}))
        .await;
    let file = f.paths.state.join("mcp").join("audit.db");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (outcome, detail) = loop {
        if let Ok(conn) =
            rusqlite::Connection::open_with_flags(&file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            && let Ok(row) = conn.query_row(
                "SELECT outcome, detail FROM calls WHERE tool = ?1",
                [SLOW],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
        {
            break row;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "취소가 헤드리스에 닿지 않았다 — 헤드리스의 호출이 끝나지 않았다"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        (outcome.as_str(), detail.as_deref()),
        ("error", Some("cancelled"))
    );
    client.close();
    let outcome = tokio::time::timeout(Duration::from_secs(15), running)
        .await
        .expect("pitwall mcp 가 끝나지 않았다")
        .unwrap();
    assert_eq!(outcome.end.unwrap(), End::ClientClosed);
}

// ---- 가짜 서버로 보는 갈래(시각을 멈춘 런타임 — 시한 · 물러남을 기다리지 않는다) ----

/// 줄 단위 가짜 MCP 서버. 받은 줄마다 `answer` 가 준 줄을 쓰고, 돌려준 송신기로 아무 때나 줄을 끼워 넣는다.
fn fake_server(
    stream: DuplexStream,
    answer: impl Fn(&Value) -> Vec<Value> + Send + 'static,
) -> (Arc<Mutex<Vec<Value>>>, mpsc::UnboundedSender<Value>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let (inject, mut injected) = mpsc::unbounded_channel::<Value>();
    let seen = received.clone();
    tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(stream);
        let mut read = BufReader::new(read);
        loop {
            let mut line = String::new();
            let out = tokio::select! {
                n = read.read_line(&mut line) => match n {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        let value: Value = serde_json::from_str(&line).unwrap();
                        seen.lock().unwrap().push(value.clone());
                        answer(&value)
                    }
                },
                Some(value) = injected.recv() => vec![value],
            };
            for value in out {
                let mut bytes = serde_json::to_vec(&value).unwrap();
                bytes.push(b'\n');
                if write.write_all(&bytes).await.is_err() {
                    return;
                }
            }
        }
    });
    (received, inject)
}

/// 요청마다 `{"from": who}` 를 결과로 답한다.
fn answer_from(who: &'static str) -> impl Fn(&Value) -> Vec<Value> + Clone + Send + 'static {
    move |message| match message.get("id") {
        Some(id) if message.get("method").is_some() => {
            vec![json!({"jsonrpc": "2.0", "id": id, "result": {"from": who}})]
        }
        _ => Vec::new(),
    }
}

/// 가짜 헤드리스 · 가짜 앱 사이에 [`run`] 을 띄운다. `app` 은 `dial` 이 부를 때마다 새 가짜 앱 연결의 답을 만든다.
struct Fakes {
    client: Client,
    headless: mpsc::UnboundedSender<Value>,
    headless_seen: Arc<Mutex<Vec<Value>>>,
    dials: Arc<AtomicUsize>,
    app_seen: Arc<Mutex<Vec<Value>>>,
}

impl Fakes {
    fn start<G>(app: G) -> Self
    where
        G: Fn(&Value) -> Vec<Value> + Clone + Send + 'static,
    {
        Self::start_with(answer_from("headless"), app)
    }

    fn start_with<G>(
        headless_answer: impl Fn(&Value) -> Vec<Value> + Send + 'static,
        app: G,
    ) -> Self
    where
        G: Fn(&Value) -> Vec<Value> + Clone + Send + 'static,
    {
        let (client_out, stdin) = tokio::io::duplex(64 * 1024);
        let (mut stdout, client_in) = tokio::io::duplex(64 * 1024);
        let (headless_ours, headless_theirs) = tokio::io::duplex(64 * 1024);
        let (headless_seen, headless) = fake_server(headless_theirs, headless_answer);
        let dials = Arc::new(AtomicUsize::new(0));
        let app_seen = Arc::new(Mutex::new(Vec::new()));
        let (counter, seen) = (dials.clone(), app_seen.clone());
        let dial = move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let (ours, theirs) = tokio::io::duplex(64 * 1024);
            let (app_received, _) = fake_server(theirs, app.clone());
            // 이 연결이 받은 줄을 시험이 볼 수 있게 — 마지막 연결의 것.
            let seen = seen.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    let lines = app_received.lock().unwrap().clone();
                    *seen.lock().unwrap() = lines;
                    if Arc::strong_count(&app_received) == 1 {
                        return;
                    }
                }
            });
            std::future::ready(Dial::Connected(ours))
        };
        tokio::spawn(async move { run(stdin, &mut stdout, headless_ours, dial).await });
        Self {
            client: Client {
                write: Some(client_out),
                read: BufReader::new(client_in),
            },
            headless,
            headless_seen,
            dials,
            app_seen,
        }
    }
}

fn refuse_initialize(message: &Value) -> Vec<Value> {
    if message["method"] == json!("initialize") {
        return vec![
            json!({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32600, "message": "no"}}),
        ];
    }
    answer_from("app")(message)
}

fn ignore_initialize(message: &Value) -> Vec<Value> {
    if message["method"] == json!("initialize") {
        return Vec::new();
    }
    answer_from("app")(message)
}

/// 앱이 다시 보낸 `initialize` 를 거절하면 넘기지 않는다: 넘기게 한 요청은 헤드리스가 답하고 `list_changed` 가 없다. 그 뒤
/// [`RETRY_AFTER`] 동안은 다시 찾지 않고, 지나면 다음 tools 요청이 다시 찾는다.
#[tokio::test(start_paused = true)]
async fn a_refused_handover_stays_headless_and_backs_off() {
    let mut fakes = Fakes::start(refuse_initialize);
    let client = &mut fakes.client;
    client.initialize().await;

    client
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .await;
    let (answer, before) = client.until(1).await;
    assert_eq!(answer["result"]["from"], json!("headless"), "{answer}");
    assert!(before.is_empty(), "넘기지 않았는데 무언가 왔다: {before:?}");
    assert_eq!(fakes.dials.load(Ordering::SeqCst), 1);

    // 물러나는 동안은 찾지 않는다.
    tokio::time::advance(RETRY_AFTER - Duration::from_secs(1)).await;
    let (answer, _) = client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            2,
        )
        .await;
    assert_eq!(answer["result"]["from"], json!("headless"), "{answer}");
    assert_eq!(
        fakes.dials.load(Ordering::SeqCst),
        1,
        "물러나는 동안 앱을 찾았다"
    );

    // 지나면 다시 찾는다(또 거절 — 헤드리스로 남는다).
    tokio::time::advance(Duration::from_secs(2)).await;
    let (answer, _) = client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
            3,
        )
        .await;
    assert_eq!(answer["result"]["from"], json!("headless"), "{answer}");
    assert_eq!(fakes.dials.load(Ordering::SeqCst), 2);
}

/// 앱이 다시 보낸 `initialize` 에 [`HANDSHAKE_TIMEOUT`] 안에 답하지 않으면 넘기지 않는다 — 그 요청은 헤드리스가 답한다.
#[tokio::test(start_paused = true)]
async fn a_silent_app_times_out_and_stays_headless() {
    let mut fakes = Fakes::start(ignore_initialize);
    let client = &mut fakes.client;
    client.initialize().await;
    let started = tokio::time::Instant::now();
    let (answer, before) = client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            1,
        )
        .await;
    assert_eq!(answer["result"]["from"], json!("headless"), "{answer}");
    assert!(before.is_empty(), "{before:?}");
    assert!(
        started.elapsed() >= HANDSHAKE_TIMEOUT,
        "시한을 기다리지 않았다"
    );
    let (_, _) = client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "x"}}),
            2,
        )
        .await;
    assert_eq!(
        fakes.dials.load(Ordering::SeqCst),
        1,
        "물러나는 동안 앱을 찾았다"
    );
}

/// 넘길 때 앱의 `initialize` 응답(id 0)은 클라이언트에 가지 않는다. 클라이언트가 받는 것은 `list_changed` 와 넘기게 한 요청의 앱
/// 응답뿐이다.
#[tokio::test(start_paused = true)]
async fn the_app_initialize_answer_does_not_reach_the_client() {
    let mut fakes = Fakes::start(answer_from("app"));
    let client = &mut fakes.client;
    client.initialize().await;
    let (answer, before) = client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            1,
        )
        .await;
    assert_eq!(answer["result"]["from"], json!("app"), "{answer}");
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(is_list_changed(&before[0]), "{before:?}");
    // 앱은 같은 initialize 를 받았다.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let app = fakes.app_seen.lock().unwrap().clone();
    assert_eq!(app[0]["method"], json!("initialize"), "{app:?}");
    assert_eq!(
        app[0]["params"]["clientInfo"]["name"],
        json!("handover-test")
    );
    // 클라이언트가 닫으면 남은 줄에도 id 0 응답이 없다.
    client.close();
    let mut rest = String::new();
    client.read.read_to_string(&mut rest).await.unwrap();
    assert!(!rest.contains("\"id\":0"), "{rest}");
}

/// 2026-07-28: 헤드리스가 구독을 수락하기 전에 넘기면 클라이언트는 앱의 수락을 정확히 한 번, 그 구독의 `list_changed` 보다 먼저
/// 받는다(rmcp 클라이언트는 수락 전의 다른 알림을 받으면 구독을 실패로 끝낸다). 넘긴 뒤 늦게 온 헤드리스의 수락은 버린다.
#[tokio::test(start_paused = true)]
async fn a_subscription_moved_before_its_acknowledgment_is_acknowledged_first() {
    // 헤드리스는 구독을 받기만 하고 수락하지 않는다(수락이 이 층을 지나기 전에 넘기는 꼴).
    let acknowledge = |message: &Value| {
        if message["method"] == json!("subscriptions/listen") {
            return vec![json!({
                "jsonrpc": "2.0", "method": "notifications/subscriptions/acknowledged",
                "params": {"_meta": {"io.modelcontextprotocol/subscriptionId": message["id"]}, "notifications": {"toolsListChanged": true}},
            })];
        }
        answer_from("app")(message)
    };
    let mut fakes = Fakes::start_with(|_: &Value| Vec::new(), acknowledge);
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "handover-2026", "version": "2.0"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    // 헤드리스가 답하지 않는 ping 하나 — 헤드리스에 남은 요청이 있어 넘긴 뒤에도 헤드리스가 열려 있다(늦은 수락을 끼울 수 있게).
    fakes
        .client
        .send(json!({"jsonrpc": "2.0", "id": "p", "method": "ping"}))
        .await;
    fakes
        .client
        .send(json!({
            "jsonrpc": "2.0", "id": "listen-1", "method": "subscriptions/listen",
            "params": {"_meta": meta, "notifications": {"toolsListChanged": true}},
        }))
        .await;
    let (answer, before) = fakes
        .client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta}}),
            1,
        )
        .await;
    assert_eq!(answer["result"]["from"], json!("app"), "{answer}");
    let methods: Vec<&str> = before
        .iter()
        .map(|m| m["method"].as_str().unwrap_or("<response>"))
        .collect();
    assert_eq!(
        methods,
        [
            "notifications/subscriptions/acknowledged",
            "notifications/tools/list_changed"
        ],
        "{before:?}"
    );
    for message in &before {
        assert_eq!(
            message["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            json!("listen-1"),
            "{message}"
        );
    }
    assert!(
        fakes.headless_seen.lock().unwrap()[1]["method"] == json!("subscriptions/listen"),
        "구독은 처음에 헤드리스로 갔다"
    );

    // 헤드리스의 수락이 늦게 와도 클라이언트에 가지 않는다.
    fakes
        .headless
        .send(json!({
            "jsonrpc": "2.0", "method": "notifications/subscriptions/acknowledged",
            "params": {"_meta": {"io.modelcontextprotocol/subscriptionId": "listen-1"}, "notifications": {"toolsListChanged": true}},
        }))
        .unwrap();
    let (_, before) = fakes
        .client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": meta}}),
            2,
        )
        .await;
    assert!(before.is_empty(), "늦은 헤드리스 수락이 갔다: {before:?}");
}

/// 넘겨 붙은 앱 세션도 승인 경로를 받는다(16-m5-plan M5-12 — 넘긴 세션이 승인 없는 세션이 되면 안 된다): 넘기기 전 헤드리스의
/// 쓰기 툴은 없고, 넘긴 뒤의 쓰기 툴 호출은 앱의 승인 허브를 듣는 창에 요청으로 뜬다. 요청의 클라이언트는 처음 보낸
/// `initialize` 의 이름이다. 허용하면 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_handed_over_session_asks_for_approval() {
    let f = Fixture::new();
    let (mut client, running) = f.start();
    client.initialize().await;
    // 앱이 뜬다 — 다음 tools 요청이 넘긴다.
    f.app_up.store(true, Ordering::SeqCst);
    let (listed, _) = client
        .until_after(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            1,
        )
        .await;
    assert!(tool_names(&listed).contains(&WRITE.to_owned()), "{listed}");

    // 세션 프로젝트가 없는 세션이다 — 프로젝트 없는 요청으로 가장 최근 창에 묻는다.
    let mut window = f.app_sessions.approvals().listen(
        None,
        Some("welcome"),
        pw_mcp::approval::Connection::named("welcome"),
    );
    client
        .send(json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": WRITE, "arguments": {}},
        }))
        .await;
    let request = match tokio::time::timeout(Duration::from_secs(10), window.recv())
        .await
        .expect("넘겨 붙은 세션이 승인을 묻지 않았다")
    {
        Some(pw_mcp::approval::McpApprovalEvent::Open { request }) => *request,
        other => panic!("{other:?}"),
    };
    assert_eq!(request.tool, WRITE);
    assert_eq!(request.client.as_deref(), Some("handover-test 1.0"));
    assert!(!request.cli);
    assert_eq!(f.writes.load(Ordering::SeqCst), 0, "묻기 전에 돌았다");
    f.app_sessions
        .approvals()
        .answer(
            &pw_mcp::approval::Answerer {
                scope: &ConnectionScope::Host,
                connection: pw_mcp::approval::Connection::named("welcome"),
            },
            request.id,
            pw_mcp::approval::McpApprovalAnswer::Once,
        )
        .unwrap();
    let (answer, _) = client.until(2).await;
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
    assert_eq!(f.writes.load(Ordering::SeqCst), 1);
    client.close();
    let _ = tokio::time::timeout(Duration::from_secs(10), running).await;
}

/// 넘겨 붙은 앱 세션도 MCP 세션 하나가 Host 연결 하나다(16-m5-plan M5-17 — DB 의 「승인받은 연결」 은 `Ctx::session` 을 세션
/// 열쇠로 쓴다): 넘긴 뒤 같은 클라이언트의 호출은 모두 같은 Host 연결로 오고, 다른 `pitwall mcp` 의 호출은 다른 연결로 온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_handed_over_session_keeps_one_host_connection() {
    let f = Fixture::new();
    f.app_up.store(true, Ordering::SeqCst);
    let call = |id: i64| json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": APP_ONLY, "arguments": {}}});
    let (mut client, running) = f.start();
    client.initialize().await;
    for id in 1..=3 {
        let (answer, _) = client.until_after(call(id), id).await;
        assert_eq!(answer["result"]["isError"], json!(false), "{answer}");
    }
    let (mut other, other_running) = f.start();
    other.initialize().await;
    let (answer, _) = other.until_after(call(1), 1).await;
    assert_eq!(answer["result"]["isError"], json!(false), "{answer}");

    let seen = f.host_sessions.lock().unwrap().clone();
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert!(seen[..3].iter().all(|s| *s == seen[0]), "{seen:?}");
    assert_ne!(
        seen[3], seen[0],
        "another pitwall mcp is another Host connection"
    );
    client.close();
    other.close();
    let _ = tokio::time::timeout(Duration::from_secs(10), running).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), other_running).await;
}
