//! 데몬의 받기: 첫 틀로 링크와 좁은 명령을 가르기, 증명 전 연결 수 상한, 첫 틀 상한, 붙은 링크 세기, 유휴 끝내기(18-m7-plan M7-04).
//! 소켓 대신 메모리 전송으로 [`Daemon::accept`] 에 넘긴다 — 소켓 자체(폴더 · uid · 한 판에 하나)는 `pw_relay::socket` 과 바이너리
//! 시험(`apps/relay/tests`)이 본다.

#![cfg(unix)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pw_core::{BoxFuture, Connection, Host, HostError, LocalHost};
use pw_link::frame::{Framed, Kind, LinkFrame};
use pw_link::transport::{self, MemoryAcceptor};
use pw_link::{
    DesktopIdentity, DeviceKey, Expect, LinkLimits, LinkState, LinkStream, Pairing, PairingTable,
    RelayIdentity, RelayParts, RemoteHost, Transport,
};
use pw_relay::command::{self, Command, NotYet};
use pw_relay::config::IdleConfig;
use pw_relay::daemon::{Activity, Daemon, DaemonParts};
use pw_rpc::{Caller, Role};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

/// 연결을 세는 Host.
struct Counting {
    inner: LocalHost,
    connects: AtomicUsize,
}

impl Host for Counting {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn connect(&self, caller: Caller) -> BoxFuture<'_, Result<Connection, HostError>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.inner.connect(caller)
    }
}

#[derive(Default)]
struct FakeActivity {
    runs: AtomicUsize,
    terminals: AtomicUsize,
    lsp_stops: AtomicUsize,
}

impl Activity for FakeActivity {
    fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }

    fn terminals(&self) -> usize {
        self.terminals.load(Ordering::SeqCst)
    }

    fn stop_language_servers(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = usize> + Send + '_>> {
        self.lsp_stops.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { 0 })
    }
}

struct Fixture {
    daemon: Daemon,
    host: Arc<Counting>,
    activity: Arc<FakeActivity>,
    relay_key: Arc<DeviceKey>,
    pairings: PairingTable,
}

fn fixture(max_unproven: usize, idle: IdleConfig) -> Fixture {
    let host = Arc::new(Counting {
        inner: LocalHost::new("local"),
        connects: AtomicUsize::new(0),
    });
    let activity = Arc::new(FakeActivity::default());
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(Vec::new());
    let daemon = Daemon::new(DaemonParts {
        link: RelayParts {
            identity: Arc::new(RelayIdentity {
                key: Arc::clone(&relay_key),
                versions: pw_relay::versions(),
            }),
            pairings: pairings.clone(),
            host: Arc::clone(&host) as Arc<dyn Host>,
            limits: LinkLimits::default(),
        },
        activity: Arc::clone(&activity) as Arc<dyn Activity>,
        narrow: Arc::new(NotYet),
        idle,
        name: Some("box".into()),
        max_unproven,
    });
    Fixture {
        daemon,
        host,
        activity,
        relay_key,
        pairings,
    }
}

fn default_fixture() -> Fixture {
    fixture(pw_relay::daemon::MAX_UNPROVEN, IdleConfig::default())
}

/// 데몬 소켓 자리 — 연 스트림을 데몬에 넘긴다.
fn serve_memory(fx: &Fixture) -> transport::MemoryTransport {
    let (transport, acceptor) = transport::memory(1024 * 1024);
    let daemon = fx.daemon.clone();
    tokio::spawn(accept_all(daemon, acceptor));
    transport
}

async fn accept_all(daemon: Daemon, acceptor: MemoryAcceptor) {
    while let Some(stream) = acceptor.accept().await {
        daemon.accept(stream);
    }
}

async fn open(transport: &transport::MemoryTransport) -> LinkStream {
    transport.open().await.unwrap()
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// `status` 는 데몬의 판 · 이름 · 붙은 링크 · 남은 일을 답한다. `open` · `askpass` 는 몸통이 들어오기 전(M7-15)까지 「아직 없음」.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn narrow_commands_answer_once() {
    let fx = default_fixture();
    fx.activity.runs.store(2, Ordering::SeqCst);
    let transport = serve_memory(&fx);
    let status = command::request_on(open(&transport).await, Command::Status, Value::Null)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status["version"], json!(pw_relay::VERSION));
    assert_eq!(status["name"], json!("box"));
    assert_eq!(status["links"], json!(0));
    assert_eq!(status["runs"], json!(2));
    assert_eq!(status["pid"], json!(std::process::id()));
    for command in [Command::Open, Command::Askpass] {
        let reply = command::request_on(open(&transport).await, command, json!({}))
            .await
            .unwrap();
        assert!(reply.unwrap_err().contains("not available"));
    }
    eventually("narrow commands still hold unproven slots", || {
        fx.daemon.unproven() == 0
    })
    .await;
}

/// 좁은 명령으로 연 연결로는 RPC 도 링크 틀도 지나지 않고, 표에 없는 첫 틀(페어링 · 채널 열기 · 메시지)은 거절된다 — Host 연결 0,
/// 페어링 표 그대로(18-m7-plan M7-04 시험 ⑦).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_narrow_connection_carries_no_rpc_and_no_pairing() {
    let fx = default_fixture();
    let transport = serve_memory(&fx);
    let desktop_key = DeviceKey::generate().unwrap();

    // 좁은 명령 뒤에 몰아 보낸 링크 틀 · RPC.
    let mut framed = Framed::handshake(open(&transport).await);
    framed
        .write
        .write_encoded(&serde_json::to_vec(&json!({"kind": "status"})).unwrap())
        .await
        .unwrap();
    for frame in [
        LinkFrame::new(
            1,
            Kind::Connect,
            json!({"role": "owner", "scope": {"kind": "host"}}),
        ),
        LinkFrame::new(
            1,
            Kind::Msg,
            json!({"jsonrpc": "2.0", "id": 1, "method": "host.ping"}),
        ),
    ] {
        let _ = framed.write.write(&frame).await;
    }
    let (answer, _) = framed.read.read_value().await.unwrap().unwrap();
    assert_eq!(answer["kind"], json!("result"));
    assert!(
        tokio::time::timeout(WAIT, framed.read.read_value())
            .await
            .unwrap()
            .unwrap()
            .is_none(),
        "the daemon kept the narrow connection open"
    );

    // 표에 없는 첫 틀.
    for first in [
        json!({"kind": "pair", "body": {"key": desktop_key.public(), "role": "owner"}}),
        json!({"ch": 1, "kind": "connect", "body": {"role": "owner", "scope": {"kind": "host"}}}),
        json!({"ch": 0, "kind": "msg", "body": {"jsonrpc": "2.0", "id": 1, "method": "host.ping"}}),
        json!({"kind": "welcome", "body": {"role": "owner"}}),
        json!({"ch": 0, "kind": "auth"}),
    ] {
        let mut framed = Framed::handshake(open(&transport).await);
        framed
            .write
            .write_encoded(&serde_json::to_vec(&first).unwrap())
            .await
            .unwrap();
        let (answer, _) = framed.read.read_value().await.unwrap().unwrap();
        assert_eq!(answer["kind"], json!("error"), "{first}");
        assert!(framed.read.read_value().await.unwrap().is_none(), "{first}");
    }
    assert_eq!(fx.host.connects.load(Ordering::SeqCst), 0);
    assert!(fx.pairings.snapshot().is_empty());
}

/// 증명 전 연결은 [`DaemonParts::max_unproven`] 개까지다 — 넘는 연결은 곧바로 닫히고, 자리가 비면 다시 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unproven_connections_are_capped() {
    let fx = fixture(2, IdleConfig::default());
    let transport = serve_memory(&fx);
    // 아무것도 보내지 않는 연결 둘이 자리를 다 쥔다.
    let silent_a = open(&transport).await;
    let _silent_b = open(&transport).await;
    eventually("two slots taken", || fx.daemon.unproven() == 2).await;

    let mut refused = Framed::handshake(open(&transport).await);
    assert!(
        tokio::time::timeout(WAIT, refused.read.read_value())
            .await
            .unwrap()
            .unwrap()
            .is_none(),
        "a third unproven connection was kept"
    );
    assert_eq!(fx.daemon.unproven(), 2);

    drop(silent_a);
    eventually("the closed connection gave its slot back", || {
        fx.daemon.unproven() == 1
    })
    .await;
    let status = command::request_on(open(&transport).await, Command::Status, Value::Null)
        .await
        .unwrap();
    assert!(status.is_ok());
}

/// 첫 틀이 `pw_link::HANDSHAKE_MAX_FRAME` 을 넘으면 데몬은 그 연결을 닫는다(좁은 명령이든 링크든).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_first_frame_closes_the_connection() {
    let fx = default_fixture();
    let transport = serve_memory(&fx);
    let mut framed = Framed::new(open(&transport).await);
    let pad = "a".repeat(pw_link::HANDSHAKE_MAX_FRAME);
    let _ = framed
        .write
        .write_encoded(&serde_json::to_vec(&json!({"kind": "status", "body": pad})).unwrap())
        .await;
    assert!(
        tokio::time::timeout(WAIT, framed.read.read_value())
            .await
            .unwrap()
            .unwrap()
            .is_none(),
        "the daemon answered an oversized first frame"
    );
    eventually("the slot was kept", || fx.daemon.unproven() == 0).await;
}

/// 증명된 owner 링크는 붙어 있는 동안 [`pw_relay::daemon::Presence`] 에 센다(M7-20 의 「붙은 owner Desktop」 판정이 본다). 링크가
/// 끊기면 빠진다. 증명 전 자리는 증명 때 놓는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proven_owner_link_counts_while_it_stays() {
    let fx = default_fixture();
    let desktop_key = Arc::new(DeviceKey::generate().unwrap());
    fx.pairings.upsert(Pairing {
        key: desktop_key.public(),
        role: Role::Owner,
        name: "mac".into(),
    });
    let transport = serve_memory(&fx);
    let presence = fx.daemon.presence();
    assert!(!presence.has_owner());

    let remote = RemoteHost::new(
        "box",
        Arc::new(transport) as Arc<dyn Transport>,
        Arc::new(DesktopIdentity {
            key: Arc::clone(&desktop_key),
            versions: pw_relay::versions(),
        }),
        Expect::Machine(fx.relay_key.id()),
        LinkLimits::default(),
    );
    remote.attach().await.unwrap();
    eventually("the owner link is not counted", || presence.has_owner()).await;
    assert_eq!(presence.links().all, 1);
    assert_eq!(fx.daemon.unproven(), 0);
    assert_eq!(fx.daemon.status().owner_links, 1);

    // 링크 너머 RPC 가 이 Host 에 닿는다.
    let mut conn = remote.connect(Caller::new(Role::Owner)).await.unwrap();
    conn.sink
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "host.ping"}));
    let reply = tokio::time::timeout(WAIT, conn.source.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply["id"], json!(1));
    assert_eq!(fx.host.connects.load(Ordering::SeqCst), 1);

    remote.detach();
    let mut state = remote.state();
    let _ = tokio::time::timeout(
        WAIT,
        state.wait_for(|s| !matches!(s, LinkState::Connected { .. })),
    )
    .await;
    eventually("the closed link is still counted", || {
        presence.links().all == 0
    })
    .await;
    assert!(!presence.has_owner());
}

/// 읽기 전용 기기의 링크는 붙어 있어도 owner 가 아니다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_readonly_link_is_not_an_owner() {
    let fx = default_fixture();
    let desktop_key = Arc::new(DeviceKey::generate().unwrap());
    fx.pairings.upsert(Pairing {
        key: desktop_key.public(),
        role: Role::Readonly,
        name: "phone".into(),
    });
    let transport = serve_memory(&fx);
    let remote = RemoteHost::new(
        "box",
        Arc::new(transport) as Arc<dyn Transport>,
        Arc::new(DesktopIdentity {
            key: desktop_key,
            versions: pw_relay::versions(),
        }),
        Expect::Machine(fx.relay_key.id()),
        LinkLimits::default(),
    );
    remote.attach().await.unwrap();
    let presence = fx.daemon.presence();
    eventually("the link is not counted", || presence.links().all == 1).await;
    assert!(!presence.has_owner());
}

/// 실행 하나가 남은 동안은 유휴 끝내기가 오지 않고, 끝나면 `exit_after` 뒤에 온다(18-m7-plan M7-04 시험 ④). 터미널 · 링크도 같다.
#[tokio::test(start_paused = true)]
async fn a_live_run_holds_off_the_idle_exit() {
    let minutes = |m: u64| Duration::from_secs(m * 60);
    let fx = fixture(
        4,
        IdleConfig {
            exit_after: minutes(30),
            stop_lsp_after: minutes(10),
        },
    );
    fx.activity.runs.store(1, Ordering::SeqCst);
    assert!(
        tokio::time::timeout(minutes(300), fx.daemon.wait_idle())
            .await
            .is_err(),
        "the daemon went idle with a run left"
    );
    fx.activity.runs.store(0, Ordering::SeqCst);
    fx.activity.terminals.store(1, Ordering::SeqCst);
    assert!(
        tokio::time::timeout(minutes(300), fx.daemon.wait_idle())
            .await
            .is_err(),
        "the daemon went idle with a terminal left"
    );
    fx.activity.terminals.store(0, Ordering::SeqCst);
    let start = tokio::time::Instant::now();
    tokio::time::timeout(minutes(300), fx.daemon.wait_idle())
        .await
        .expect("no idle exit with nothing left");
    let waited = start.elapsed();
    assert!(
        waited >= minutes(30) && waited <= minutes(31),
        "idle exit after {waited:?}"
    );
    assert!(
        fx.activity.lsp_stops.load(Ordering::SeqCst) >= 1,
        "the language servers were not stopped"
    );
}

fn owner_remote(fx: &Fixture, transport: transport::MemoryTransport) -> RemoteHost {
    let desktop_key = Arc::new(DeviceKey::generate().unwrap());
    fx.pairings.upsert(Pairing {
        key: desktop_key.public(),
        role: Role::Owner,
        name: "mac".into(),
    });
    RemoteHost::new(
        "box",
        Arc::new(transport) as Arc<dyn Transport>,
        Arc::new(DesktopIdentity {
            key: desktop_key,
            versions: pw_relay::versions(),
        }),
        Expect::Machine(fx.relay_key.id()),
        LinkLimits::default(),
    )
}

/// 붙은 링크가 있는 동안은 유휴 끝내기가 오지 않는다 — 실행 · 터미널이 없어도(리뷰 F1: Desktop 이 붙어 있는데 데몬이 끝나면 그
/// 링크의 터미널 트리가 함께 끝난다). 링크가 닫히면 `exit_after` 뒤에 온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_linked_desktop_holds_off_the_idle_exit() {
    let short = Duration::from_millis(300);
    let fx = fixture(
        4,
        IdleConfig {
            exit_after: short,
            stop_lsp_after: short,
        },
    );
    let transport = serve_memory(&fx);
    let remote = owner_remote(&fx, transport);
    remote.attach().await.unwrap();
    let presence = fx.daemon.presence();
    eventually("the link is not counted", || presence.links().all == 1).await;
    assert!(
        tokio::time::timeout(short * 6, fx.daemon.wait_idle())
            .await
            .is_err(),
        "the daemon went idle with a desktop linked"
    );
    remote.detach();
    eventually("the closed link is still counted", || {
        presence.links().all == 0
    })
    .await;
    tokio::time::timeout(short * 6, fx.daemon.wait_idle())
        .await
        .expect("no idle exit after the link closed");
}

/// 첫 틀을 보내지 않는 연결은 `FIRST_FRAME_TIMEOUT`(5초) 뒤 닫히고 자리를 돌려준다(리뷰 권고 2 — E1).
#[tokio::test(start_paused = true)]
async fn a_connection_without_a_first_frame_is_closed_after_the_timeout() {
    let fx = default_fixture();
    let transport = serve_memory(&fx);
    let mut silent = Framed::handshake(open(&transport).await);
    let start = tokio::time::Instant::now();
    let closed = tokio::time::timeout(Duration::from_secs(60), silent.read.read_value())
        .await
        .expect("the silent connection was kept past a minute");
    assert!(closed.unwrap().is_none());
    let waited = start.elapsed();
    assert!(
        waited >= pw_relay::daemon::FIRST_FRAME_TIMEOUT && waited < Duration::from_secs(6),
        "{waited:?}"
    );
    assert_eq!(fx.daemon.unproven(), 0);
}
