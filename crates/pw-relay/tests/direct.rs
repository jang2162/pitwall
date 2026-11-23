//! 데몬의 직접 연결 받기(18-m7-plan M7-24 ① ② — QUIC + Noise): 페어링된 기기만 링크를 연다 · 증명 전 자리는 주소별 상한과 전체
//! 시한 하나 · 대기 코드(파일)로 코드 페어링. 받는 쪽은 시험용 루프백에 묶는다(듣는 인터페이스 고르기는 `pw_relay::listen`).

#![cfg(unix)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pw_core::{BoxFuture, Connection, Host, HostError, LocalHost};
use pw_link::quic::{QuicListener, QuicTransport};
use pw_link::{
    DesktopIdentity, DeviceKey, DialError, Expect, LinkLimits, Pairing, PairingTable, RejectReason,
    RelayIdentity, RelayParts, RemoteHost, SecuredTransport, Transport,
};
use pw_relay::codes::{FileCodeBook, system_clock};
use pw_relay::command::NotYet;
use pw_relay::config::IdleConfig;
use pw_relay::daemon::{Activity, Daemon, DaemonParts, MAX_UNPROVEN, MAX_UNPROVEN_PER_ADDRESS};
use pw_rpc::{Caller, Role};

const WAIT: Duration = Duration::from_secs(10);

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
    daemon: Daemon,
    host: Arc<Counting>,
    relay_key: Arc<DeviceKey>,
    pairings: PairingTable,
    listener: Arc<QuicListener>,
    data: tempfile::TempDir,
}

fn fixture(paired: Vec<Pairing>) -> Fixture {
    let host = Arc::new(Counting {
        inner: LocalHost::new("local"),
        connects: AtomicUsize::new(0),
    });
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(paired);
    let daemon = Daemon::new(DaemonParts {
        link: RelayParts {
            identity: Arc::new(RelayIdentity {
                key: Arc::clone(&relay_key),
                versions: pw_relay::versions(),
            }),
            pairings: pairings.clone(),
            host: Arc::clone(&host) as Arc<dyn Host>,
            limits: LinkLimits::default(),
            resumes: pw_link::Resumes::default(),
        },
        activity: Arc::new(Idle),
        narrow: Arc::new(NotYet),
        idle: IdleConfig::default(),
        name: None,
        max_unproven: MAX_UNPROVEN,
    });
    let data = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        data.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let listener = Arc::new(QuicListener::bind((Ipv4Addr::LOCALHOST, 0).into()).unwrap());
    let codes = Arc::new(FileCodeBook::new(
        data.path().to_owned(),
        Some(pairings.clone()),
        system_clock(),
    ));
    {
        let (daemon, listener) = (daemon.clone(), Arc::clone(&listener));
        tokio::spawn(async move { daemon.serve_direct(listener, codes).await });
    }
    Fixture {
        daemon,
        host,
        relay_key,
        pairings,
        listener,
        data,
    }
}

impl Fixture {
    fn addr(&self) -> SocketAddr {
        self.listener.local_addr().unwrap()
    }

    fn quic(&self) -> QuicTransport {
        let addr = self.addr();
        QuicTransport::new(addr.ip().to_string(), addr.port())
    }

    fn remote(&self, key: &Arc<DeviceKey>) -> RemoteHost {
        RemoteHost::new(
            "mini",
            Arc::new(SecuredTransport::new(self.quic(), Arc::clone(key))),
            Arc::new(DesktopIdentity {
                key: Arc::clone(key),
                versions: pw_relay::versions(),
            }),
            Expect::Machine(self.relay_key.id()),
            LinkLimits::default(),
        )
    }
}

fn paired(key: &DeviceKey, role: Role) -> Pairing {
    Pairing {
        key: key.public(),
        role,
        name: "mac".into(),
    }
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// 같은 tailnet · LAN 의 페어링된 기기: QUIC + Noise 로 링크를 열고, 증명되면 증명 전 자리(주소별 포함)를 놓고 붙은 링크로 센다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paired_device_links_directly_and_frees_its_unproven_slot() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let fx = fixture(vec![paired(&desktop, Role::Owner)]);
    let remote = fx.remote(&desktop);
    remote.attach().await.unwrap();
    assert_eq!(remote.link().unwrap().role(), Role::Owner);
    eventually("the link is counted", || {
        fx.daemon.presence().links().owner == 1
    })
    .await;
    assert_eq!(fx.daemon.unproven(), 0);
    assert_eq!(fx.daemon.unproven_from(LOCAL), 0);
    let _conn = remote
        .connect(Caller {
            role: Role::Owner,
            scope: pw_rpc::ConnectionScope::Host,
            window: None,
        })
        .await
        .unwrap();
    assert_eq!(fx.host.connects.load(Ordering::SeqCst), 1);
}

/// tailnet 의 다른 기기(페어링 안 됨)는 Noise 위 첫 핸드셰이크에서 거절되고 Host 에 아무것도 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpaired_device_is_refused_at_the_handshake() {
    let fx = fixture(Vec::new());
    let stranger = Arc::new(DeviceKey::generate().unwrap());
    let refused = fx.remote(&stranger).attach().await.err();
    assert_eq!(refused, Some(DialError::Rejected(RejectReason::Unpaired)));
    assert_eq!(fx.host.connects.load(Ordering::SeqCst), 0);
    eventually("the slot comes back", || fx.daemon.unproven() == 0).await;
}

/// 증명 전 자리: 한 주소는 [`MAX_UNPROVEN_PER_ADDRESS`] 까지만 쥔다 — 그 다음 연결은 받지 않는다. 증명하지 않는 연결은 전체 시한
/// 뒤 닫히고 자리가 돌아온다(그 뒤에는 페어링된 기기가 붙는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_address_holds_a_few_unproven_slots_for_a_short_time_only() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let fx = fixture(vec![paired(&desktop, Role::Owner)]);
    fx.daemon.set_direct_timeout(Duration::from_millis(1500));
    // 스트림을 열고 아무것도 보내지 않는 연결들(Noise 를 시작하지 않는다).
    let mut held = Vec::new();
    for _ in 0..MAX_UNPROVEN_PER_ADDRESS {
        held.push(fx.quic().open().await.unwrap());
    }
    eventually("each connection holds a slot", || {
        fx.daemon.unproven_from(LOCAL) == MAX_UNPROVEN_PER_ADDRESS
    })
    .await;
    let refused = fx
        .quic()
        .with_connect_timeout(Duration::from_secs(2))
        .open()
        .await;
    assert!(refused.is_err(), "a fifth unproven connection was taken");
    assert_eq!(fx.daemon.unproven_from(LOCAL), MAX_UNPROVEN_PER_ADDRESS);
    // 전체 시한이 지나면 닫힌다.
    eventually("the silent connections are dropped", || {
        fx.daemon.unproven_from(LOCAL) == 0 && fx.daemon.unproven() == 0
    })
    .await;
    drop(held);
    fx.remote(&desktop).attach().await.unwrap();
}

/// Noise 를 시작했지만 끝내지 않는(조금씩 보내는) 연결도 전체 시한 하나로 끝난다 — Noise · 첫 틀 · 코드 페어링마다의 `dead_after`
/// (30초)를 기다리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_handshake_ends_at_the_one_deadline() {
    use tokio::io::AsyncWriteExt;
    let fx = fixture(Vec::new());
    fx.daemon.set_direct_timeout(Duration::from_millis(800));
    let mut stream = fx.quic().open().await.unwrap();
    // Noise 메시지 길이만 보내고 몸통을 보내지 않는다.
    stream.write.write_all(&[0, 32]).await.unwrap();
    stream.write.flush().await.unwrap();
    eventually("the slow connection holds a slot", || {
        fx.daemon.unproven_from(LOCAL) == 1
    })
    .await;
    let started = std::time::Instant::now();
    eventually("the slow connection is dropped", || {
        fx.daemon.unproven_from(LOCAL) == 0
    })
    .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    let mut rest = Vec::new();
    let read = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::AsyncReadExt::read_to_end(&mut stream.read, &mut rest),
    )
    .await;
    assert!(read.is_ok(), "the relay did not close the connection");
}

/// 수동 배치 Relay: 대기 코드(파일 — `FileCodeBook`)로 새 기기가 직접 연결 위에서 페어링하고, 곧 그 기기로 링크를 연다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_code_pairs_a_new_device_over_a_direct_connection() {
    let fx = fixture(Vec::new());
    let (code, _) = pw_relay::codes::issue(
        fx.data.path(),
        Role::Owner,
        Some("laptop"),
        (system_clock())(),
    )
    .unwrap();
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let me = DesktopIdentity {
        key: Arc::clone(&desktop),
        versions: pw_relay::versions(),
    };
    let secured = pw_link::noise::initiate(fx.quic().open().await.unwrap(), &desktop)
        .await
        .unwrap();
    let paired = pw_link::pair_with_code(secured, &me, code.reveal(), "laptop", WAIT)
        .await
        .unwrap();
    assert_eq!(paired.machine, fx.relay_key.id());
    assert_eq!(paired.role, Role::Owner);
    assert!(fx.pairings.role_of(&desktop.public()).is_some());
    fx.remote(&desktop).attach().await.unwrap();
}

/// QUIC 위에서 Noise 없이 링크 핸드셰이크(`hello` · 서명)를 하면 — 페어링된 기기 키로라도 — 데몬은 받지 않고 Host 에 아무것도 닿지
/// 않는다. QUIC 의 TLS 만으로 링크를 받는 길이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_over_quic_without_noise_gets_nothing() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let fx = fixture(vec![paired(&desktop, Role::Owner)]);
    fx.daemon.set_direct_timeout(Duration::from_secs(2));
    let stream = fx.quic().open().await.unwrap();
    let dialed = tokio::time::timeout(
        WAIT,
        pw_link::dial(
            stream,
            &DesktopIdentity {
                key: Arc::clone(&desktop),
                versions: pw_relay::versions(),
            },
            &Expect::Machine(fx.relay_key.id()),
            LinkLimits::default(),
        ),
    )
    .await
    .expect("the relay neither linked nor closed");
    assert!(dialed.is_err(), "a plain link stood over quic");
    assert_eq!(fx.host.connects.load(Ordering::SeqCst), 0);
    assert_eq!(fx.daemon.presence().links().all, 0);
}

/// 리뷰 F1: 직접 연결의 증명 전 자리는 소켓(SSH `proxy` · 좁은 명령)과 따로다 — 여러 주소로 직접 연결 자리를 다 채워도(전체 상한
/// [`MAX_DIRECT_UNPROVEN`]) 소켓으로 온 SSH 링크는 곧바로 붙는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_direct_slots_leave_the_ssh_socket_free() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let fx = fixture(vec![paired(&desktop, Role::Owner)]);
    // 주소 열 개(가짜 — 이 기기 루프백은 하나뿐이다)로 자리를 채운다. 주소마다 4 · 전체 상한까지.
    let mut held = Vec::new();
    for host in 1..=10u8 {
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, host));
        while let Some(slot) = fx.daemon.hold_direct_slot(ip) {
            held.push(slot);
        }
    }
    // SSH proxy 자리(데몬 소켓 — 메모리 전송으로 `accept` 에 넘긴다).
    let (transport, acceptor) = pw_link::transport::memory(256 * 1024);
    {
        let daemon = fx.daemon.clone();
        tokio::spawn(async move {
            while let Some(stream) = acceptor.accept().await {
                assert!(daemon.accept(stream), "the socket had no slot left");
            }
        });
    }
    let remote = RemoteHost::new(
        "mini",
        Arc::new(transport),
        Arc::new(DesktopIdentity {
            key: Arc::clone(&desktop),
            versions: pw_relay::versions(),
        }),
        Expect::Machine(fx.relay_key.id()),
        LinkLimits::default(),
    );
    tokio::time::timeout(WAIT, remote.attach())
        .await
        .expect("the ssh link did not stand while direct slots were full")
        .unwrap();
    assert_eq!(held.len(), pw_relay::daemon::MAX_DIRECT_UNPROVEN);
    drop(held);
}

/// 리뷰 F2: 증명된 직접 링크는 증명 전 시한이 지나도 산다 — 시한의 몇 배를 기다린 뒤에도 새 연결이 오간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proven_direct_link_outlives_the_proof_deadline() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let fx = fixture(vec![paired(&desktop, Role::Owner)]);
    fx.daemon.set_direct_timeout(Duration::from_millis(300));
    let remote = fx.remote(&desktop);
    remote.attach().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        fx.daemon.presence().links().all,
        1,
        "the link was cut at the deadline"
    );
    let _conn = tokio::time::timeout(
        WAIT,
        remote.connect(Caller {
            role: Role::Owner,
            scope: pw_rpc::ConnectionScope::Host,
            window: None,
        }),
    )
    .await
    .expect("no answer over the proven link")
    .unwrap();
    assert_eq!(fx.host.connects.load(Ordering::SeqCst), 1);
}
