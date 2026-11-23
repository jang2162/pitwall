//! 좁은 명령 `open` 의 행선지(18-m7-plan M7-15 ② · 결정 M7-D3): 원격 셸의 열기 요청이 링크로 붙은 Desktop 의 `open.listen` 에
//! 닿는다 — 듣는 owner Desktop 가운데 마지막 입력이 있던 것 하나로, 없으면 거절. 듣기는 증명된 owner 링크의 Host 범위만이다.
//! 실제 Relay Host(`pw_relay::host::assemble_with`)와 데몬(`Daemon::accept`)을 메모리 전송으로 잇는다.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::open::OpenRequests;
use pw_core::{Connection, Host};
use pw_link::transport::{self, MemoryAcceptor, MemoryTransport};
use pw_link::{
    DesktopIdentity, DeviceKey, Expect, LinkLimits, Pairing, PairingTable, RelayIdentity,
    RelayParts, RemoteHost, Transport,
};
use pw_relay::command::{self, Command};
use pw_relay::config::{IdleConfig, RelayConfig};
use pw_relay::daemon::{Activity, Daemon, DaemonParts, MAX_UNPROVEN};
use pw_relay::narrow::RelayNarrow;
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

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
    root: PathBuf,
    project: String,
}

async fn accept_all(daemon: Daemon, acceptor: MemoryAcceptor) {
    while let Some(stream) = acceptor.accept().await {
        daemon.accept(stream);
    }
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let work = base.join("work");
    let root = work.join("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    let askpass = pw_git::write::Askpass::default();
    let requests = OpenRequests::default();
    let assembled = pw_relay::host::assemble_with(
        pw_relay::paths::relay_paths(&AppPaths::under(&base.join("app"))),
        &RelayConfig {
            roots: vec![work],
            ..RelayConfig::default()
        },
        tokio::runtime::Handle::current(),
        askpass.clone(),
        requests.clone(),
    );
    let registry = assembled.host.registry().cloned().unwrap();
    let project = registry
        .create(pw_core::registry::NewProject {
            root: root.clone(),
            ..Default::default()
        })
        .unwrap()
        .id;
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(Vec::new());
    let daemon = Daemon::new(DaemonParts {
        link: RelayParts {
            identity: Arc::new(RelayIdentity {
                key: Arc::clone(&relay_key),
                versions: pw_relay::versions(),
            }),
            pairings: pairings.clone(),
            host: Arc::new(assembled.host) as Arc<dyn Host>,
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
    Fixture {
        _dir: dir,
        daemon,
        relay_key,
        pairings,
        transport: Arc::new(transport),
        root,
        project,
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

    /// 원격 셸의 `pitwall-relay open <path>`(데몬 소켓의 좁은 명령).
    async fn open(&self, path: &Path, line: Option<u32>) -> command::Reply {
        let stream = self.transport.open().await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(30),
            command::request_on(stream, Command::Open, json!({"path": path, "line": line})),
        )
        .await
        .expect("no answer to open")
        .unwrap()
    }
}

/// 링크 너머 RPC 연결 하나.
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

    /// `open.listen` 을 연다 — 서면 듣기 표를, 거절되면 그 오류를.
    async fn listen(&mut self) -> Result<String, Value> {
        let opened = self.call("open.listen", json!({})).await;
        if !opened["error"].is_null() {
            return Err(opened["error"].clone());
        }
        let stream = opened["result"]["streamId"].clone();
        self.0.sink.send(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream, "credit": 64}}));
        match self.chunk().await {
            Ok(first) => Ok(first["listener"].as_str().unwrap().to_owned()),
            Err(e) => Err(e),
        }
    }

    async fn chunk(&mut self) -> Result<Value, Value> {
        loop {
            let frame = self.next().await;
            match frame["method"].as_str() {
                Some("stream.chunk") => return Ok(frame["params"]["data"].clone()),
                Some("stream.end") => return Err(frame["params"]["error"].clone()),
                _ => {}
            }
        }
    }
}

/// 열기 요청을 듣는 Desktop 흉내: 질문에는 `idle` 로 답하고 받은 열기를 보낸다.
fn desktop_loop(
    mut rpc: Rpc,
    token: String,
    idle: u64,
) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok(event) = rpc.chunk().await else { return };
            let request_id = event["requestId"].clone();
            match event["kind"].as_str() {
                Some("query") => {
                    rpc.send(
                        "open.answer",
                        json!({"listener": token, "requestId": request_id, "answer": {"kind": "idle", "idleMs": idle}}),
                    );
                }
                Some("open") => {
                    rpc.send(
                        "open.answer",
                        json!({"listener": token, "requestId": request_id, "answer": {"kind": "opened", "openedIn": "newWindow"}}),
                    );
                    let _ = tx.send(event);
                }
                _ => {}
            }
        }
    });
    rx
}

/// 원격 셸의 열기 요청이 링크로 붙은 Desktop 가운데 **마지막 입력이 있던 것**에만 간다 — 그 Desktop 에 창이 없어도(Desktop 이
/// 연다). 경로는 이 머신의 그 프로젝트 · 실제 경로로 실려 가고, 활성 체크아웃을 바꾸는 값(워크트리)은 메인 체크아웃이면 비어 있다.
/// 듣는 Desktop 이 없으면 거절하고 원격에 까닭을 준다(18-m7-plan M7-15 시험 ④).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shell_open_goes_to_the_desktop_with_the_latest_input() {
    let f = fixture();
    let readme = f.root.join("README.md");

    let refused = f.open(&readme, Some(2)).await.unwrap_err();
    assert_eq!(refused, pw_core::open::desks::NO_DESKTOP);

    let away = f.desktop(Role::Owner, "office").await;
    let here = f.desktop(Role::Owner, "laptop").await;
    let mut away_rpc = Rpc::open(&away, Caller::new(Role::Owner)).await;
    let away_token = away_rpc.listen().await.unwrap();
    let mut here_rpc = Rpc::open(&here, Caller::new(Role::Owner)).await;
    let here_token = here_rpc.listen().await.unwrap();
    let mut away_opens = desktop_loop(away_rpc, away_token, 3_600_000);
    let mut here_opens = desktop_loop(here_rpc, here_token, 800);

    let opened = f.open(&readme, Some(2)).await.unwrap();
    assert_eq!(opened["openedIn"], "newWindow");
    let event = tokio::time::timeout(WAIT, here_opens.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event["projectId"], json!(f.project));
    assert_eq!(event["path"], json!(readme));
    assert_eq!(event["line"], json!(2));
    assert_eq!(event["worktree"], Value::Null);
    assert!(
        away_opens.try_recv().is_err(),
        "the idle desktop got the open"
    );

    // 등록된 프로젝트 밖은 Desktop 에 묻지도 않는다.
    let outside = f.open(Path::new("/etc/hosts"), None).await.unwrap_err();
    assert!(outside.contains("not inside a project"), "{outside}");
    assert_eq!(f.daemon.unproven(), 0);
}

/// `open.listen` 은 증명된 owner 링크의 Host 범위 연결만 받는다 — owner 링크의 프로젝트 창 연결 · 에이전트 연결, 읽기 전용 · 모바일
/// 기기의 링크(role 이 잘린다)는 거절이다(메인 판정 2026-11-23 — 표 시험).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_proven_owner_links_host_connection_listens() {
    let f = fixture();
    let owner = f.desktop(Role::Owner, "mac").await;
    let readonly = f.desktop(Role::Readonly, "viewer").await;
    let mobile = f.desktop(Role::Mobile, "phone").await;
    let project = Caller {
        role: Role::Owner,
        scope: ConnectionScope::Project(f.project.clone()),
        window: Some("project-x".into()),
    };
    let cases = [
        (&owner, Caller::new(Role::Owner), true),
        (&owner, project, false),
        (&owner, Caller::agent(Role::Owner), false),
        (&readonly, Caller::new(Role::Owner), false),
        (&mobile, Caller::new(Role::Owner), false),
    ];
    for (i, (remote, caller, admitted)) in cases.into_iter().enumerate() {
        let what = format!("case {i}: {caller:?}");
        let mut rpc = Rpc::open(remote, caller).await;
        let listened = rpc.listen().await;
        assert_eq!(listened.is_ok(), admitted, "{what}: {listened:?}");
    }
}
