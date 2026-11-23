//! Relay 의 MCP 엔드포인트 · 원격 승인 전달(18-m7-plan M7-20). 에이전트 입장에서 본다.
//!
//! 가짜 Host 가 없다: 실제 Relay Host(`pw_relay::host::assemble_serving` — Relay 의 레지스트리 · 설정 · 툴), 실제 데몬
//! (`Daemon::accept` — 링크 증명 · role 자르기 · 붙은 링크 세기)를 메모리 전송으로 잇고, 에이전트는 실제 MCP 소켓
//! (`pw_mcp::socket` — Relay 의 `$STATE/relay/run/` 아래)에 JSON-RPC 줄로 붙는다. Desktop 은 페어링한 기기 키로 붙은 링크이고
//! 그 창은 링크 너머 RPC 연결로 `mcp.approvals` 를 듣고 `mcp.approval.answer` 로 답한다.
//!
//! 쓰기 툴은 실제 `run_config_upsert`(write — `.pitwall/run/<id>.toml`), 프로젝트 없는 쓰기는 실제 `project_register`.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::{Connection, Host, LocalHost};
use pw_link::transport::{self, MemoryAcceptor, MemoryTransport};
use pw_link::{
    DesktopIdentity, DeviceKey, Expect, LinkLimits, Pairing, PairingTable, RelayIdentity,
    RelayParts, RemoteHost, Transport,
};
use pw_relay::config::{IdleConfig, RelayConfig};
use pw_relay::daemon::{Activity, Daemon, DaemonParts, MAX_UNPROVEN};
use pw_relay::narrow::RelayNarrow;
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

const WAIT: Duration = Duration::from_secs(15);

struct Idle;

impl Activity for Idle {
    fn runs(&self) -> usize {
        0
    }
    fn terminals(&self) -> usize {
        0
    }
    fn stop_language_servers(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = usize> + Send + '_>> {
        Box::pin(async { 0 })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    daemon: Daemon,
    relay_key: Arc<DeviceKey>,
    pairings: PairingTable,
    transport: Arc<MemoryTransport>,
    socket: PathBuf,
    paths: AppPaths,
    root: PathBuf,
    work: PathBuf,
    project: String,
    sessions: pw_mcp::sessions::Sessions,
}

async fn accept_all(daemon: Daemon, acceptor: MemoryAcceptor) {
    while let Some(stream) = acceptor.accept().await {
        daemon.accept(stream);
    }
}

/// Relay 의 전역 설정(`$CONFIG/relay/settings.toml`)을 `settings` 로 두고 띄운다.
fn fixture(settings: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let work = base.join("work");
    let root = work.join("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    let paths = pw_relay::paths::relay_paths(&AppPaths::under(&base.join("app")));
    std::fs::create_dir_all(&paths.config).unwrap();
    std::fs::write(paths.config.join("settings.toml"), settings).unwrap();
    let askpass = pw_git::write::Askpass::default();
    let requests = pw_core::open::OpenRequests::default();
    let sessions = pw_mcp::sessions::Sessions::new();
    let assembled = pw_relay::host::assemble_serving(
        paths.clone(),
        &RelayConfig {
            roots: vec![work.clone()],
            ..RelayConfig::default()
        },
        tokio::runtime::Handle::current(),
        askpass.clone(),
        requests.clone(),
        sessions.clone(),
    );
    let registry = assembled.host.registry().cloned().unwrap();
    let project = registry
        .create(pw_core::registry::NewProject {
            root: root.clone(),
            ..Default::default()
        })
        .unwrap()
        .id;
    let host: Arc<LocalHost> = Arc::new(assembled.host);
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(Vec::new());
    let daemon = Daemon::new(DaemonParts {
        link: RelayParts {
            identity: Arc::new(RelayIdentity {
                key: Arc::clone(&relay_key),
                versions: pw_relay::versions(),
            }),
            pairings: pairings.clone(),
            host: host.clone() as Arc<dyn Host>,
            limits: LinkLimits::default(),
            resumes: pw_link::Resumes::default(),
        },
        activity: Arc::new(Idle),
        narrow: Arc::new(RelayNarrow::new(askpass, requests, Some(registry))),
        idle: IdleConfig::default(),
        name: Some("mini".into()),
        max_unproven: MAX_UNPROVEN,
    });
    let (transport, acceptor) = transport::memory(1024 * 1024);
    tokio::spawn(accept_all(daemon.clone(), acceptor));

    let presence = daemon.presence();
    let services = pw_relay::mcp::services(
        &host,
        &paths,
        sessions.clone(),
        Arc::new(move || presence.has_owner()),
    );
    let uid = pw_mcp::socket::current_uid();
    let socket = pw_relay::mcp::endpoint(&paths, "dev", "0.0.0", Path::new("/tmp"), uid).unwrap();
    let listener = pw_mcp::socket::bind(&socket, uid).unwrap();
    let connections = pw_mcp::socket::Connections::default();
    daemon.count_agents(connections.clone());
    tokio::spawn(pw_mcp::socket::accept_loop(
        listener,
        services,
        connections,
        tokio_util::sync::CancellationToken::new(),
    ));
    Fixture {
        _dir: dir,
        daemon,
        relay_key,
        pairings,
        transport: Arc::new(transport),
        socket,
        paths,
        root,
        work,
        project,
        sessions,
    }
}

impl Fixture {
    /// 그 role 로 페어링한 Desktop 의 링크.
    async fn desktop(&self, role: Role, name: &str) -> RemoteHost {
        let key = Arc::new(DeviceKey::generate().unwrap());
        self.pairings.upsert(Pairing {
            key: key.public(),
            role,
            name: name.into(),
        });
        let remote = RemoteHost::new(
            "mini",
            Arc::clone(&self.transport) as Arc<dyn Transport>,
            Arc::new(DesktopIdentity {
                key,
                versions: pw_relay::versions(),
            }),
            Expect::Machine(self.relay_key.id()),
            LinkLimits::default(),
        );
        remote.attach().await.unwrap();
        remote
    }

    /// 붙은 owner 링크 수가 `n` 이 될 때까지.
    async fn owners(&self, n: usize) {
        let presence = self.daemon.presence();
        tokio::time::timeout(WAIT, async {
            while presence.links().owner != n {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owner links never settled");
    }

    /// 창 하나가 `mcp.approvals` 를 연다 — 승인 허브가 그 듣기를 받을 때까지 기다린다(받기 전에 부른 호출은 「창 없음」 이다).
    /// 거절되면 그 오류.
    async fn listen(&self, rpc: &mut Rpc, project: Option<&str>) -> Result<(), Value> {
        let before = self.sessions.approvals().listeners();
        rpc.listen(project).await?;
        let approvals = self.sessions.approvals().clone();
        tokio::select! {
            ended = rpc.event() => Err(ended.err().unwrap_or(Value::Null)),
            () = async {
                while approvals.listeners() <= before {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } => Ok(()),
        }
    }

    /// 그 프로젝트 창의 연결(링크 너머 — Desktop 이 그 머신 멤버로 연 프로젝트 범위 연결).
    fn project_caller(&self, window: &str) -> Caller {
        Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(self.project.clone()),
            window: Some(window.into()),
        }
    }

    /// 원격 머신의 에이전트(`pitwall-relay mcp` 가 붙는 소켓).
    async fn agent(&self) -> Agent {
        let stream = pw_mcp::socket::connect(
            &self.socket,
            pw_mcp::socket::current_uid(),
            pw_mcp::SessionRequest {
                project: None,
                cwd: Some(self.root.clone()),
            },
        )
        .await
        .unwrap();
        Agent::start(stream).await
    }

    fn run_file(&self, id: &str) -> PathBuf {
        self.root.join(".pitwall/run").join(format!("{id}.toml"))
    }
}

/// MCP 클라이언트 흉내(줄 단위 JSON-RPC). 답은 id 로 가른다 — 승인을 기다리는 호출과 다른 호출이 겹친다.
struct Agent {
    write: Mutex<Option<tokio::net::unix::OwnedWriteHalf>>,
    waiting: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    next: std::sync::atomic::AtomicI64,
}

impl Agent {
    async fn start(stream: tokio::net::UnixStream) -> Self {
        let (read, write) = stream.into_split();
        let waiting: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> = Arc::default();
        let routes = Arc::clone(&waiting);
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(id) = frame["id"].as_i64()
                    && frame.get("method").is_none()
                    && let Some(tx) = routes.lock().unwrap().remove(&id)
                {
                    let _ = tx.send(frame);
                }
            }
        });
        let agent = Self {
            write: Mutex::new(Some(write)),
            waiting,
            next: std::sync::atomic::AtomicI64::new(0),
        };
        let init = agent
            .request(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "remote-agent", "version": "1"}}),
            )
            .await;
        assert!(init["result"].is_object(), "{init}");
        agent
            .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        agent
    }

    async fn send(&self, frame: Value) {
        let mut write = self
            .write
            .lock()
            .unwrap()
            .take()
            .expect("one writer at a time");
        let mut line = serde_json::to_vec(&frame).unwrap();
        line.push(b'\n');
        write.write_all(&line).await.unwrap();
        *self.write.lock().unwrap() = Some(write);
    }

    async fn request(&self, method: &str, params: Value) -> Value {
        let rx = self.start_request(method, params).await;
        tokio::time::timeout(WAIT, rx)
            .await
            .expect("no answer in time")
            .unwrap()
    }

    async fn start_request(&self, method: &str, params: Value) -> oneshot::Receiver<Value> {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id, tx);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        rx
    }

    /// `tools/call` 을 보내고 답을 기다리지 않는다.
    async fn spawn_call(&self, tool: &str, args: Value) -> oneshot::Receiver<Value> {
        self.start_request("tools/call", json!({"name": tool, "arguments": args}))
            .await
    }

    async fn call(&self, tool: &str, args: Value) -> Value {
        let frame = self
            .request("tools/call", json!({"name": tool, "arguments": args}))
            .await;
        frame["result"].clone()
    }

    async fn tools(&self) -> Vec<String> {
        let frame = self.request("tools/list", json!({})).await;
        frame["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    }
}

async fn settle(rx: oneshot::Receiver<Value>) -> Value {
    tokio::time::timeout(WAIT, rx)
        .await
        .expect("no answer in time")
        .unwrap()["result"]
        .clone()
}

fn reason(result: &Value) -> Option<&str> {
    result["_meta"][pw_mcp::marker::REASON].as_str()
}

fn outcome(result: &Value) -> &str {
    result["_meta"][pw_mcp::marker::OUTCOME].as_str().unwrap()
}

/// 링크 너머 RPC 연결 하나(Desktop 의 창).
struct Rpc(Connection, i64);

impl Rpc {
    async fn open(remote: &RemoteHost, caller: Caller) -> Self {
        Self(remote.connect(caller).await.unwrap(), 0)
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(WAIT, self.0.source.recv())
            .await
            .expect("no frame in time")
            .expect("closed")
    }

    fn send(&mut self, method: &str, params: Value) -> i64 {
        self.1 += 1;
        self.0
            .sink
            .send(json!({"jsonrpc": "2.0", "id": self.1, "method": method, "params": params}));
        self.1
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        loop {
            let frame = self.next().await;
            if frame["id"] == json!(id) {
                return frame;
            }
        }
    }

    /// `mcp.approvals` 를 연다. 거절되면 그 오류.
    async fn listen(&mut self, project: Option<&str>) -> Result<(), Value> {
        let params = project.map_or_else(|| json!({}), |p| json!({"projectId": p}));
        let opened = self.call("mcp.approvals", params).await;
        if !opened["error"].is_null() {
            return Err(opened["error"].clone());
        }
        let stream = opened["result"]["streamId"].clone();
        self.0.sink.send(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream, "credit": 64}}));
        Ok(())
    }

    /// 다음 승인 이벤트(스트림 항목).
    async fn event(&mut self) -> Result<Value, Value> {
        loop {
            let frame = self.next().await;
            match frame["method"].as_str() {
                Some("stream.chunk") => return Ok(frame["params"]["data"].clone()),
                Some("stream.end") => return Err(frame["params"]["error"].clone()),
                _ => {}
            }
        }
    }

    /// 다음 「열림」 요청.
    async fn opened(&mut self) -> Value {
        loop {
            let event = self.event().await.expect("the approval stream ended");
            if event["kind"] == "open" {
                return event["request"].clone();
            }
        }
    }

    /// `wait` 동안 이벤트가 없는가.
    async fn quiet(&mut self, wait: Duration) -> bool {
        tokio::time::timeout(wait, self.0.source.recv())
            .await
            .is_err()
    }

    async fn answer(&mut self, id: &Value, answer: &str) -> Value {
        self.call("mcp.approval.answer", json!({"id": id, "answer": answer}))
            .await
    }
}

fn upsert_args(project: &str, id: &str) -> Value {
    json!({
        "projectId": project,
        "id": id,
        "config": {"name": id, "type": "shell", "command": "echo hi"},
    })
}

/// ① Relay 세션은 그 머신만이다 — `checkout` 에 다른 머신을 주면 거절이고 아무것도 읽지 않는다. 스키마에도 `checkout` 이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_machine_checkout_is_refused_on_a_relay_session() {
    let f = fixture("");
    let _mac = f.desktop(Role::Owner, "mac").await;
    f.owners(1).await;
    let agent = f.agent().await;

    let listed = agent.request("tools/list", json!({})).await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    for tool in tools {
        assert!(
            tool["inputSchema"]["properties"].get("checkout").is_none(),
            "{} offers `checkout`",
            tool["name"]
        );
    }

    let local = agent
        .call("run_config_list", json!({"projectId": f.project}))
        .await;
    assert_eq!(outcome(&local), "ok", "{local}");
    for machine in ["mac", "SHA256:other-machine"] {
        let refused = agent
            .call(
                "run_config_list",
                json!({"projectId": f.project, "checkout": {"machine": machine}}),
            )
            .await;
        assert_eq!(outcome(&refused), "denied", "{refused}");
        assert_eq!(reason(&refused), Some("machine_unavailable"), "{refused}");
    }
    let write = agent
        .call(
            "run_config_upsert",
            json!({"projectId": f.project, "id": "x", "config": {"name": "x", "type": "shell", "command": "echo"}, "checkout": {"machine": "mac"}}),
        )
        .await;
    assert_eq!(reason(&write), Some("machine_unavailable"), "{write}");
    assert!(!f.run_file("x").exists());
}

/// ② · ⑤ `readonly` · `mobile` 기기의 링크와 에이전트 범위 연결은 승인을 듣지 못하고, 승인 요청이 그쪽으로 가지 않는다 — owner
/// Desktop 이 붙어 있어도 그 프로젝트를 듣는 창이 readonly 뿐이면 기다리지 않고 거절된다. 에이전트 범위 연결은 원격에서도
/// 답하지 못한다(듣기도 답도).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn readonly_links_and_agent_connections_neither_hear_nor_answer() {
    let f = fixture("");
    let owner = f.desktop(Role::Owner, "mac").await;
    let readonly = f.desktop(Role::Readonly, "viewer").await;
    let mobile = f.desktop(Role::Mobile, "phone").await;
    f.owners(1).await;

    // 듣기: readonly · mobile 은 role 에서, 에이전트 범위 연결은 `mcp.*` 의 문에서 거절이다.
    for (remote, caller, what) in [
        (&readonly, f.project_caller("project-x"), "readonly"),
        (&mobile, f.project_caller("project-x"), "mobile"),
        (&owner, Caller::agent(Role::Owner), "agent"),
    ] {
        let mut rpc = Rpc::open(remote, caller.clone()).await;
        assert!(
            f.listen(&mut rpc, Some(&f.project)).await.is_err(),
            "{what} listened"
        );
        assert_eq!(f.sessions.approvals().listeners(), 0, "{what}");
        // 프로젝트 없이 듣기(웰컴 · 설정 창의 꼴 — 링크의 non-owner 기기는 Host 범위로 잘려 범위 판정은 지난다)도 role 에서 거절이다.
        let mut rpc = Rpc::open(remote, caller).await;
        assert!(
            f.listen(&mut rpc, None).await.is_err(),
            "{what} listened without a project"
        );
        assert_eq!(f.sessions.approvals().listeners(), 0, "{what}");
    }

    // 승인 요청이 readonly 링크에 가지 않는다 — 듣는 owner 창이 없으니 곧바로 거절(창 없음).
    let agent = f.agent().await;
    let started = std::time::Instant::now();
    let refused = agent
        .call("run_config_upsert", upsert_args(&f.project, "a"))
        .await;
    assert_eq!(reason(&refused), Some("needs_approval"), "{refused}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!f.run_file("a").exists());

    // owner 창이 들을 때의 요청에 에이전트 범위 연결 · readonly 링크는 답하지 못한다.
    let mut window = Rpc::open(&owner, f.project_caller("project-x")).await;
    f.listen(&mut window, Some(&f.project)).await.unwrap();
    let call = agent
        .spawn_call("run_config_upsert", upsert_args(&f.project, "b"))
        .await;
    let request = window.opened().await;
    let id = request["id"].clone();
    for (remote, caller, what) in [
        (&owner, Caller::agent(Role::Owner), "agent"),
        (&readonly, f.project_caller("project-x"), "readonly"),
    ] {
        let mut rpc = Rpc::open(remote, caller).await;
        let answered = rpc.answer(&id, "once").await;
        assert!(answered["error"].is_object(), "{what} answered: {answered}");
    }
    assert!(!f.run_file("b").exists());
    let denied = window.answer(&id, "deny").await;
    assert!(denied["error"].is_null(), "{denied}");
    let result = settle(call).await;
    assert_eq!(reason(&result), Some("approval_denied"), "{result}");
    assert!(!f.run_file("b").exists());
}

/// ③ 두 Desktop 이 같은 프로젝트를 들으면 둘 다 보고, 먼저 답한 것만 효과다. 둘째 답은 `NOT_FOUND`(닫힌 요청)이고 둘째 창에는
/// 닫힘이 간다. ④(대조) 듣는 창이 없으면 기다리지 않고 거절.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_of_two_desktops_to_answer_wins() {
    let f = fixture("");
    let mac = f.desktop(Role::Owner, "mac").await;
    let laptop = f.desktop(Role::Owner, "laptop").await;
    f.owners(2).await;
    let agent = f.agent().await;

    let started = std::time::Instant::now();
    let nobody = agent
        .call("run_config_upsert", upsert_args(&f.project, "n"))
        .await;
    assert_eq!(reason(&nobody), Some("needs_approval"), "{nobody}");
    assert!(started.elapsed() < Duration::from_secs(5), "it waited");

    let mut first = Rpc::open(&mac, f.project_caller("project-1")).await;
    f.listen(&mut first, Some(&f.project)).await.unwrap();
    let mut second = Rpc::open(&laptop, f.project_caller("project-1")).await;
    f.listen(&mut second, Some(&f.project)).await.unwrap();

    let call = agent
        .spawn_call("run_config_upsert", upsert_args(&f.project, "first"))
        .await;
    let seen_first = first.opened().await;
    let seen_second = second.opened().await;
    assert_eq!(seen_first["id"], seen_second["id"]);
    assert_eq!(seen_first["tool"], "run_config_upsert");
    // Relay 세션의 요청에는 다른 머신 표지가 없다 — 그 머신의 로컬 에이전트다. 「<머신> 의 에이전트」 는 그 링크의 Desktop 이 붙인다.
    assert_eq!(seen_first["machine"], Value::Null);
    let id = seen_first["id"].clone();

    let won = second.answer(&id, "once").await;
    assert!(won["error"].is_null(), "{won}");
    // 먼저 답하지 않은 창에는 닫힘이 간다 — 그 뒤의 답은 닫힌 요청이다.
    loop {
        let event = first.event().await.unwrap();
        if event["kind"] == "closed" {
            assert_eq!(event["id"], id);
            break;
        }
    }
    let late = first.answer(&id, "deny").await;
    assert_eq!(
        late["error"]["code"],
        json!(pw_rpc::code::NOT_FOUND),
        "{late}"
    );
    let result = settle(call).await;
    assert_eq!(outcome(&result), "ok", "{result}");
    assert!(f.run_file("first").exists());
}

/// 열기 요청을 듣는 Desktop 흉내(`open.listen` — Desktop 의 `remote_open` 처럼 창 label 을 싣고 연다): 「입력 뒤 지난 시간」 질문에
/// `idle` 로 답한다. 돌려준 것을 버리면 듣기를 그만둔다.
async fn desk(remote: &RemoteHost, idle: u64) -> tokio::task::JoinHandle<()> {
    let mut rpc = Rpc::open(
        remote,
        Caller {
            window: Some("open-listener".into()),
            ..Caller::new(Role::Owner)
        },
    )
    .await;
    let opened = rpc.call("open.listen", json!({})).await;
    assert!(opened["error"].is_null(), "{opened}");
    let stream = opened["result"]["streamId"].clone();
    rpc.0.sink.send(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream, "credit": 64}}));
    let first = rpc.event().await.unwrap();
    let token = first["listener"].as_str().unwrap().to_owned();
    tokio::spawn(async move {
        while let Ok(event) = rpc.event().await {
            if event["kind"] == "query" {
                rpc.send(
                    "open.answer",
                    json!({"listener": token, "requestId": event["requestId"], "answer": {"kind": "idle", "idleMs": idle}}),
                );
            }
        }
    })
}

/// ⑥ Desktop 이 둘일 때 프로젝트 없는 요청(`project_register`)은 한 Desktop 의 창 하나에만 보이고, 그 Desktop 은 **마지막으로 사람
/// 입력이 있던 것**이다(결정 M7-D3 — 열기 요청과 같은 길, 입력 시각은 Desktop 이 낸다). 나중에 듣기 시작했어도 입력이 더 오래된
/// Desktop 으로 가지 않는다. 듣는 Desktop(`open.listen`)이 없으면 다른 창으로 떨어지지 않고 거절이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_projectless_request_goes_to_the_desktop_with_the_latest_input() {
    let f = fixture("");
    let mac = f.desktop(Role::Owner, "mac").await;
    let laptop = f.desktop(Role::Owner, "laptop").await;
    f.owners(2).await;
    // 입력이 최근인 laptop 이 먼저 듣고, 한 시간 전에 입력이 있던 mac 이 나중에 듣는다(허브의 「가장 최근 듣기」 라면 mac 이다).
    let mut here = Rpc::open(&laptop, f.project_caller("project-1")).await;
    f.listen(&mut here, Some(&f.project)).await.unwrap();
    let mut away = Rpc::open(&mac, f.project_caller("project-1")).await;
    f.listen(&mut away, Some(&f.project)).await.unwrap();

    let agent = f.agent().await;
    let other = f.work.join("other");
    std::fs::create_dir_all(&other).unwrap();

    // 열기 요청을 듣는 Desktop 이 없다 — 마지막 입력을 모르니 창이 있어도 보이지 않고 거절이다.
    let refused = agent.call("project_register", json!({"root": other})).await;
    assert_eq!(reason(&refused), Some("needs_approval"), "{refused}");
    assert!(here.quiet(Duration::from_millis(200)).await);
    assert!(away.quiet(Duration::from_millis(200)).await);

    let _away_desk = desk(&mac, 3_600_000).await;
    let _here_desk = desk(&laptop, 800).await;
    let mut call = agent
        .spawn_call("project_register", json!({"root": other}))
        .await;
    let request = tokio::select! {
        r = &mut call => panic!("the call ended without asking: {r:?}"),
        request = here.opened() => request,
        request = away.opened() => panic!("the desktop with the older input got it: {request}"),
    };
    assert_eq!(request["notThisProject"], true);
    assert_eq!(request["projectId"], Value::Null);
    assert!(
        away.quiet(Duration::from_millis(500)).await,
        "both desktops saw it"
    );
    let answered = here.answer(&request["id"], "deny").await;
    assert!(answered["error"].is_null(), "{answered}");
    let result = settle(call).await;
    assert_eq!(reason(&result), Some("approval_denied"), "{result}");
}

/// ⑦ 붙은 owner Desktop 이 없으면 `auto_approve` 에 든 등급이어도 `write` 이상은 헤드리스처럼 거절된다(결정 M7-D10 ④). 붙으면
/// 그 머신의 `auto_approve` 로 묻지 않고 돈다(대조 — ① 그 머신의 로컬 규칙). readonly 링크만 붙은 것은 붙은 owner 가 아니다.
/// owner 가 떠나면 다시 거절이다. 읽기는 늘 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_an_owner_desktop_writes_are_refused_even_when_auto_approved() {
    let f = fixture("[mcp]\nauto_approve = [\"write\"]\n");
    let agent = f.agent().await;

    let read = agent
        .call("run_config_list", json!({"projectId": f.project}))
        .await;
    assert_eq!(outcome(&read), "ok", "{read}");
    let refused = agent
        .call("run_config_upsert", upsert_args(&f.project, "alone"))
        .await;
    assert_eq!(outcome(&refused), "denied", "{refused}");
    assert_eq!(reason(&refused), Some("headless_write"), "{refused}");
    assert!(!f.run_file("alone").exists());

    let viewer = f.desktop(Role::Readonly, "viewer").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let still = agent
        .call("run_config_upsert", upsert_args(&f.project, "viewer"))
        .await;
    assert_eq!(reason(&still), Some("headless_write"), "{still}");
    assert!(!f.run_file("viewer").exists());

    let owner = f.desktop(Role::Owner, "mac").await;
    f.owners(1).await;
    let auto = agent
        .call("run_config_upsert", upsert_args(&f.project, "auto"))
        .await;
    assert_eq!(outcome(&auto), "ok", "{auto}");
    assert!(f.run_file("auto").exists());

    owner.detach();
    f.owners(0).await;
    let gone = agent
        .call("run_config_upsert", upsert_args(&f.project, "gone"))
        .await;
    assert_eq!(reason(&gone), Some("headless_write"), "{gone}");
    assert!(!f.run_file("gone").exists());
    drop(viewer);
}

/// MCP 연결이 열려 있으면 링크가 없어도 유휴가 아니다 — 원격 에이전트의 세션이 데몬의 유휴 끝내기로 끊기지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_agent_connection_holds_the_daemon() {
    let f = fixture("");
    assert!(!f.daemon.busy().holds());
    let agent = f.agent().await;
    assert_eq!(f.daemon.busy().agents, 1);
    assert!(f.daemon.busy().holds());
    let _ = agent.tools().await;
    drop(agent);
    tokio::time::timeout(WAIT, async {
        while f.daemon.busy().agents != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent connection was not let go");
}

/// 감사는 Relay 의 `$STATE/relay/mcp/audit.db` 에 남고(M7-P11) 「에이전트 활동」 은 그 머신 연결로 `mcp.audit.list` 를 읽는다. 인자
/// 원문(명령줄)은 감사에 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_audit_is_the_relays_own_and_read_over_the_link() {
    let f = fixture("[mcp]\nauto_approve = [\"write\"]\n");
    let owner = f.desktop(Role::Owner, "mac").await;
    f.owners(1).await;
    let agent = f.agent().await;
    let secret_command = "echo relay-audit-marker-7f3a";
    let done = agent
        .call(
            "run_config_upsert",
            json!({"projectId": f.project, "id": "audited", "config": {"name": "audited", "type": "shell", "command": secret_command}}),
        )
        .await;
    assert_eq!(outcome(&done), "ok", "{done}");
    assert!(
        f.paths.state.ends_with("relay"),
        "{}",
        f.paths.state.display()
    );
    let audit = f.paths.state.join("mcp/audit.db");
    let mut panel = Rpc::open(&owner, f.project_caller("project-1")).await;
    let rows = tokio::time::timeout(WAIT, async {
        loop {
            let page = panel
                .call("mcp.audit.list", json!({"projectId": f.project}))
                .await;
            let entries = page["result"]["entries"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if entries
                .iter()
                .any(|e| e["tool"] == "run_config_upsert" && e["outcome"] == "ok")
            {
                return entries;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the relay audit has no row for the call");
    assert!(audit.exists(), "{}", audit.display());
    let text = serde_json::to_string(&rows).unwrap();
    assert!(!text.contains("relay-audit-marker-7f3a"), "{text}");
    let raw = std::fs::read(&audit).unwrap();
    assert!(
        !raw.windows(secret_command.len())
            .any(|w| w == secret_command.as_bytes()),
        "the audit file holds the command line"
    );
}
