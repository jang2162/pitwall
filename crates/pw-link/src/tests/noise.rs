//! Noise 세션 · 코드 페어링(18-m7-plan M7-23 시험 ① ② ④ — 공격자 쪽). 대기 코드의 만료 · 시도 수의 파일 영속은 `pw_relay::codes`
//! 의 시험이 본다 — 여기서는 시험용 [`Book`] 으로 프로토콜의 판정을 본다.

use std::sync::atomic::AtomicU32;

use tokio::io::AsyncWriteExt;

use super::*;
use crate::noise;
use crate::pair::{CodeBook, PairError, PairingCode, Pending, pair_with_code};
use crate::relay::Served;
use crate::transport::{LinkStream, Transport};
use crate::{SecuredTransport, dial, serve_noise};

/// 시험용 대기 코드 하나. 시도는 [`crate::pair::MAX_ATTEMPTS`] 까지.
struct Book {
    pending: Mutex<Option<(PairingCode, Role)>>,
    attempts: AtomicU32,
    pairings: PairingTable,
}

impl Book {
    fn new(code: &str, role: Role, pairings: PairingTable) -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(Some((PairingCode::parse(code).unwrap(), role))),
            attempts: AtomicU32::new(0),
            pairings,
        })
    }

    fn attempts(&self) -> u32 {
        self.attempts.load(Ordering::SeqCst)
    }
}

impl CodeBook for Book {
    fn attempt(&self) -> std::io::Result<Option<Pending>> {
        let mut pending = self.pending.lock().unwrap();
        let Some((code, role)) = pending.clone() else {
            return Ok(None);
        };
        if self.attempts.fetch_add(1, Ordering::SeqCst) + 1 >= crate::pair::MAX_ATTEMPTS {
            *pending = None;
        }
        Ok(Some(Pending {
            code,
            role,
            name: None,
        }))
    }

    fn complete(
        &self,
        code: &PairingCode,
        key: &crate::PublicKey,
        role: Role,
        name: &str,
    ) -> std::io::Result<()> {
        let mut pending = self.pending.lock().unwrap();
        if pending.as_ref().is_some_and(|(c, _)| c != code) {
            return Err(std::io::Error::other("the code changed"));
        }
        *pending = None;
        self.pairings.upsert(Pairing {
            key: *key,
            role,
            name: name.to_owned(),
        });
        Ok(())
    }
}

/// Noise 로 받는 Relay 하나.
struct NoiseRelay {
    host: Arc<RelayHost>,
    key: Arc<DeviceKey>,
    pairings: PairingTable,
    book: Arc<Book>,
    transport: Arc<transport::MemoryTransport>,
    results: Arc<Mutex<Vec<Result<Served, ServeError>>>>,
}

fn noise_relay(paired: Vec<Pairing>, code: &str, role: Role) -> NoiseRelay {
    let host = RelayHost::new("relay");
    let key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(paired);
    let book = Book::new(code, role, pairings.clone());
    let (transport, acceptor) = transport::memory(64 * 1024);
    let parts = RelayParts::new(
        Arc::new(RelayIdentity {
            key: Arc::clone(&key),
            versions: versions(),
        }),
        pairings.clone(),
        Arc::clone(&host) as Arc<dyn Host>,
        LinkLimits::default(),
    );
    let results = Arc::new(Mutex::new(Vec::new()));
    {
        let (results, book) = (Arc::clone(&results), Arc::clone(&book));
        tokio::spawn(async move {
            while let Some(stream) = acceptor.accept().await {
                let (parts, results, book) =
                    (parts.clone(), Arc::clone(&results), Arc::clone(&book));
                tokio::spawn(async move {
                    let result = serve_noise(stream, &parts, &*book, |_| {}).await;
                    results.lock().unwrap().push(result);
                });
            }
        });
    }
    NoiseRelay {
        host,
        key,
        pairings,
        book,
        transport: Arc::new(transport),
        results,
    }
}

impl NoiseRelay {
    async fn secured(&self, key: &DeviceKey) -> LinkStream {
        noise::initiate(self.transport.open().await.unwrap(), key)
            .await
            .unwrap()
    }

    async fn pair(&self, key: &Arc<DeviceKey>, code: &str) -> Result<crate::CodePaired, PairError> {
        let stream = self.secured(key).await;
        pair_with_code(stream, &identity(key), code, "laptop", WAIT).await
    }

    fn last(&self) -> Option<Result<Served, ServeError>> {
        self.results.lock().unwrap().last().cloned()
    }
}

fn identity(key: &Arc<DeviceKey>) -> DesktopIdentity {
    DesktopIdentity {
        key: Arc::clone(key),
        versions: versions(),
    }
}

fn paired(key: &DeviceKey, role: Role) -> Pairing {
    Pairing {
        key: key.public(),
        role,
        name: "mac".into(),
    }
}

// ── ① 페어링하지 않은 기기 · ④ role ─────────────────────────────────────────────────────────────────────────────

/// 시험 ①: 페어링하지 않은 기기 키는 Noise 를 마쳐도 그 위 핸드셰이크에서 거절되고, RPC 하나도 Host 에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpaired_key_over_noise_gets_no_rpc() {
    let relay = noise_relay(Vec::new(), "123456", Role::Owner);
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let stream = relay.secured(&desktop).await;
    let refused = dial(
        stream,
        &identity(&desktop),
        &Expect::Machine(relay.key.id()),
        LinkLimits::default(),
    )
    .await
    .err();
    assert_eq!(refused, Some(DialError::Rejected(RejectReason::Unpaired)));
    assert_eq!(relay.host.connects(), 0);
}

/// 시험 ④ · 연결: 페어링된 기기는 Noise 위에서 M7-03 링크를 열고(`SecuredTransport` — 다시 붙을 때도 같은 길), role 은 페어링 표의
/// 것보다 높아지지 않는다 — owner 를 주장한 readonly 기기의 쓰기는 `PERMISSION_DENIED`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paired_device_links_over_noise_with_its_paired_role_only() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let relay = noise_relay(
        vec![paired(&desktop, Role::Readonly)],
        "123456",
        Role::Owner,
    );
    let transport = SecuredTransport::new(
        ForwardTransport(Arc::clone(&relay.transport)),
        Arc::clone(&desktop),
    );
    assert!(transport.describe().starts_with("noise over "));
    let remote = RemoteHost::new(
        "mini",
        Arc::new(transport),
        Arc::new(identity(&desktop)),
        Expect::Machine(relay.key.id()),
        LinkLimits::default(),
    );
    remote.attach().await.unwrap();
    assert_eq!(remote.link().unwrap().role(), Role::Readonly);
    let mut client = Client::open(
        &remote,
        Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project("p1".into()),
            window: Some("w".into()),
        },
    )
    .await;
    let denied = client
        .call(1, "test.write", json!({"text": "x"}))
        .await
        .unwrap_err();
    assert_eq!(denied["code"], json!(pw_rpc::code::PERMISSION_DENIED));
    assert_eq!(
        client
            .call(2, "test.echo", json!({"text": "ok"}))
            .await
            .unwrap()["text"],
        "ok"
    );
}

/// 공유 메모리 전송을 `Transport` 로(`SecuredTransport` 가 감싼다).
struct ForwardTransport(Arc<transport::MemoryTransport>);

impl Transport for ForwardTransport {
    fn describe(&self) -> String {
        self.0.describe()
    }

    fn open(&self) -> pw_core::BoxFuture<'_, std::io::Result<LinkStream>> {
        self.0.open()
    }
}

// ── 결속: 중계 끼워 넣기(M7-03 리뷰 권고 3) ─────────────────────────────────────────────────────────────────────────

/// 두 평문 스트림 사이를 그대로 잇는다(중계자).
fn splice(a: LinkStream, b: LinkStream) {
    let (mut ar, mut aw) = (a.read, a.write);
    let (mut br, mut bw) = (b.read, b.write);
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut ar, &mut bw).await;
        let _ = bw.shutdown().await;
    });
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut br, &mut aw).await;
        let _ = aw.shutdown().await;
    });
}

/// Desktop 쪽 결속: 털린 Relay(A)가 Desktop 과 자기 키로 Noise 를 하고, 안쪽 바이트를 다른 Relay(B — Desktop 이 페어링된 진짜 Relay)
/// 로 그대로 중계한다. B 의 `hello` 는 B 의 서명을 실어 맞지만 Noise 상대(A)의 키가 아니다 — Desktop 은 서명하지 않고 끊는다(처음 붙는
/// 호스트라도). 결속이 없으면 A 는 세션 키를 쥔 채 그 링크에 틀을 끼워 넣는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desktop_refuses_a_hello_relayed_through_another_noise_peer() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    // B 는 결속을 보지 않는 길(평문 `serve`)로 받는다 — Desktop 쪽 검사만 본다.
    let fx = fixture(Setup::default());
    let b_side = fx.transport_open().await;
    fx.pairings.upsert(paired(&desktop, Role::Owner));

    let attacker = DeviceKey::generate().unwrap();
    let (d_end, a_end) = tokio::io::duplex(64 * 1024);
    let a = tokio::spawn(async move {
        noise::respond(LinkStream::split(a_end), &attacker)
            .await
            .unwrap()
    });
    let d = noise::initiate(LinkStream::split(d_end), &desktop)
        .await
        .unwrap();
    splice(a.await.unwrap(), b_side);
    let dialed = dial(
        d,
        &identity(&desktop),
        &Expect::FirstContact,
        LinkLimits::default(),
    )
    .await
    .err();
    assert_eq!(
        dialed,
        Some(DialError::Protocol(
            "the relay's key is not the noise peer".to_owned()
        )),
        "the desktop signed for a relay behind another noise peer"
    );
    assert_eq!(fx.relay_host.connects(), 0);
}

/// Relay 쪽 결속: 중계자(A)가 Relay(B)와 자기 키로 Noise 를 하고, 안쪽에 페어링된 Desktop 의 `hello` · 서명을 그대로 중계한다(A 가
/// Desktop 과는 결속을 보지 않는 평문으로 이었다 — Relay 쪽 검사만 본다). B 는 Noise 상대가 아닌 키의 `hello` 를 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_refuses_a_hello_relayed_through_another_noise_peer() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let relay = noise_relay(vec![paired(&desktop, Role::Owner)], "123456", Role::Owner);
    let attacker = DeviceKey::generate().unwrap();
    let to_b = relay.secured(&attacker).await;
    let (d_end, a_end) = tokio::io::duplex(64 * 1024);
    splice(LinkStream::split(a_end), to_b);
    let dialed = dial(
        LinkStream::split(d_end),
        &identity(&desktop),
        &Expect::Machine(relay.key.id()),
        LinkLimits::default(),
    )
    .await
    .err();
    assert_eq!(dialed, Some(DialError::Rejected(RejectReason::Protocol)));
    assert_eq!(relay.host.connects(), 0);
}

// ── ② 코드 페어링 ───────────────────────────────────────────────────────────────────────────────────────────

/// 맞는 코드는 Noise 로 증명된 그 키를 코드의 role 로 넣는다. 그 뒤 그 기기는 Noise 링크를 연다(머신 id 는 코드로 증명된 Relay 키).
/// 한 번 쓴 코드는 끝이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_right_code_pairs_the_noise_key_once_with_the_chosen_role() {
    let relay = noise_relay(Vec::new(), "123456", Role::Mobile);
    let phone = Arc::new(DeviceKey::generate().unwrap());
    let done = relay.pair(&phone, "123 456").await.unwrap();
    assert_eq!(done.machine, relay.key.id());
    assert_eq!(done.role, Role::Mobile);
    assert_eq!(relay.pairings.role_of(&phone.public()), Some(Role::Mobile));
    assert_eq!(relay.pairings.snapshot()[0].name, "laptop");
    eventually("the relay did not report the pairing", || {
        matches!(
            relay.last(),
            Some(Ok(Served::Paired {
                role: Role::Mobile,
                ..
            }))
        )
    })
    .await;

    let link = dial(
        relay.secured(&phone).await,
        &identity(&phone),
        &Expect::Machine(done.machine.clone()),
        LinkLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(link.role(), Role::Mobile);

    // 같은 코드를 다시 쓰지 못한다.
    let other = Arc::new(DeviceKey::generate().unwrap());
    assert_eq!(
        relay.pair(&other, "123456").await.err(),
        Some(PairError::Rejected(RejectReason::Code))
    );
    assert_eq!(relay.pairings.role_of(&other.public()), None);
}

/// 틀린 코드는 거절되고, 시도는 연결마다가 아니라 그 코드에 대해 센다 — 세 번 틀리면 맞는 코드도 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_wrong_codes_discard_the_code() {
    let relay = noise_relay(Vec::new(), "123456", Role::Owner);
    let guesser = Arc::new(DeviceKey::generate().unwrap());
    for (i, guess) in ["000000", "111111", "222222"].iter().enumerate() {
        assert_eq!(
            relay.pair(&guesser, guess).await.err(),
            Some(PairError::Rejected(RejectReason::Code)),
            "guess {i}"
        );
    }
    assert_eq!(relay.book.attempts(), 3);
    assert_eq!(
        relay.pair(&guesser, "123456").await.err(),
        Some(PairError::Rejected(RejectReason::Code)),
        "the code survived three wrong guesses"
    );
    assert!(relay.pairings.snapshot().is_empty());
}

/// 답(SPAKE2 메시지)을 받고 확인을 보내지 않고 끊는 연결도 시도 하나다 — 그러지 않으면 답으로 추측 하나를 맞춰 보고 끊기를
/// 되풀이한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_drops_after_the_relay_answer_spends_an_attempt() {
    let relay = noise_relay(Vec::new(), "123456", Role::Owner);
    let guesser = Arc::new(DeviceKey::generate().unwrap());
    let stream = relay.secured(&guesser).await;
    let mut framed = Framed::handshake(stream);
    let (_state, spake_a) = spake2::Spake2::<spake2::Ed25519Group>::start_a(
        &spake2::Password::new(b"000000"),
        &spake2::Identity::new(b"x"),
        &spake2::Identity::new(b"y"),
    );
    framed
        .write
        .write(&LinkFrame::control(
            Kind::Pair,
            json!({
                "protocol": PROTOCOL, "app": "1.2.3", "releaseChannel": "stable",
                "device": guesser.id(), "key": guesser.public(), "name": "x",
                "spake": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &spake_a),
            }),
        ))
        .await
        .unwrap();
    let answer = raw_read(&mut framed).await.unwrap();
    assert_eq!(answer.kind, Kind::Pair);
    drop(framed);
    assert_eq!(
        relay.book.attempts(),
        1,
        "a dropped attempt was not counted"
    );
}

/// 코드 페어링은 Noise 위에서만 한다: Desktop 은 Noise 가 아닌 스트림으로 코드를 보내지 않고, Relay 의 평문 받기(`serve` — 데몬
/// 소켓 · SSH 링크)는 `pair` 첫 틀을 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_pairing_runs_only_over_noise() {
    let fx = fixture(Setup {
        paired: None,
        ..Setup::default()
    });
    let key = Arc::new(DeviceKey::generate().unwrap());
    assert_eq!(
        pair_with_code(
            fx.transport_open().await,
            &identity(&key),
            "123456",
            "x",
            WAIT
        )
        .await
        .err(),
        Some(PairError::NotSecured)
    );
    let mut framed = Framed::handshake(fx.transport_open().await);
    framed
        .write
        .write(&LinkFrame::control(Kind::Pair, json!({"spake": ""})))
        .await
        .unwrap();
    let reply = raw_read(&mut framed).await.unwrap();
    assert_eq!(reply.kind, Kind::Reject);
    assert_eq!(
        serde_json::from_value::<Reject>(reply.body).unwrap().reason,
        RejectReason::Protocol
    );
    assert!(fx.pairings.snapshot().is_empty());
}

/// 채널 결속: 코드를 모르는 중간자(A)가 Desktop ↔ A, A ↔ Relay 두 Noise 세션 사이에서 PAKE 메시지를 옮긴다. A 는 Relay 쪽 세션의
/// 키가 자기 것이라 `pair.key` 에 자기 키를 넣는다(Desktop 키는 그 세션의 상대가 아니라 거절된다). 대본이 각 세션의 핸드셰이크 해시에
/// 묶여 있어 Relay 는 옮겨 온 확인을 받지 않는다 — A 의 키는 페어링 표에 들지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pake_messages_moved_to_another_session_pair_nothing() {
    let relay = noise_relay(Vec::new(), "123456", Role::Owner);
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let attacker = Arc::new(DeviceKey::generate().unwrap());

    // Desktop ↔ A(Noise, A 의 키).
    let (d_end, a_end) = tokio::io::duplex(64 * 1024);
    let a_key = Arc::clone(&attacker);
    let a_side = tokio::spawn(async move {
        noise::respond(LinkStream::split(a_end), &a_key)
            .await
            .unwrap()
    });
    let d_side = noise::initiate(LinkStream::split(d_end), &desktop)
        .await
        .unwrap();
    let mut from_desktop = Framed::handshake(a_side.await.unwrap());
    // A ↔ Relay(Noise, A 의 키).
    let mut to_relay = Framed::handshake(relay.secured(&attacker).await);

    let desktop_pairs = tokio::spawn({
        let desktop = Arc::clone(&desktop);
        async move { pair_with_code(d_side, &identity(&desktop), "123456", "laptop", WAIT).await }
    });
    let mitm = tokio::spawn(async move {
        // pair: Desktop 의 SPAKE2 메시지를 A 의 키로 옮긴다.
        let mut hello = raw_read(&mut from_desktop).await.unwrap();
        hello.body["key"] = json!(attacker.public());
        hello.body["device"] = json!(attacker.id());
        to_relay.write.write(&hello).await.unwrap();
        // Relay 의 답(키는 Relay 의 것 — Desktop 의 Noise 상대(A)가 아니라 Desktop 이 거절한다). 결속을 보려고 A 의 키로 바꿔 건넨다.
        let mut answer = raw_read(&mut to_relay).await.unwrap();
        if answer.kind == Kind::Pair {
            answer.body["key"] = json!(attacker.public());
        }
        from_desktop.write.write(&answer).await.unwrap();
        // Desktop 의 확인을 Relay 로.
        if let Some(confirm) = raw_read(&mut from_desktop).await {
            to_relay.write.write(&confirm).await.unwrap();
            raw_read(&mut to_relay).await
        } else {
            None
        }
    });
    let relay_said = mitm.await.unwrap();
    let desktop_got = desktop_pairs.await.unwrap();
    assert!(
        desktop_got.is_err(),
        "the desktop paired through a man in the middle"
    );
    assert_eq!(
        relay_said.map(|f| f.kind),
        Some(Kind::Reject),
        "the relay accepted a confirmation from another session"
    );
    assert!(
        relay.pairings.snapshot().is_empty(),
        "the man in the middle got its key paired"
    );
}

/// 코드는 로그 · `Debug` 에 실리지 않는다(맞는 페어링 · 틀린 시도 모두 — `tracing` 을 받아 본다).
#[tokio::test(flavor = "current_thread")]
async fn the_code_stays_out_of_logs_and_debug() {
    let code = PairingCode::generate().unwrap();
    assert!(!format!("{code:?}").contains(code.reveal()));
    let logs = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = {
        let logs = Arc::clone(&logs);
        move || LogWriter(Arc::clone(&logs))
    };
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(writer)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let relay = noise_relay(Vec::new(), code.reveal(), Role::Owner);
    let phone = Arc::new(DeviceKey::generate().unwrap());
    let _ = relay.pair(&phone, "999999").await;
    relay.pair(&phone, code.reveal()).await.unwrap();
    tracing::info!("flush");
    let text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(text.contains("paired a device by code"), "{text}");
    assert!(
        !text.contains(code.reveal()),
        "the code reached the log: {text}"
    );
    // 문자열 형식이 바뀌어도 숫자 여섯이 이어진 꼴로 새지 않게.
    let digits: Vec<String> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|w| w.len() == 6)
        .map(str::to_owned)
        .collect();
    assert!(!digits.iter().any(|d| d == code.reveal() || d == "999999"));
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 코드의 꼴: 여섯 자리 숫자만, 공백 · `-` 는 뺀다. 만든 코드는 늘 여섯 자리다.
#[test]
fn codes_are_six_digits() {
    for _ in 0..200 {
        let code = PairingCode::generate().unwrap();
        assert_eq!(code.reveal().len(), 6);
        assert!(code.reveal().bytes().all(|b| b.is_ascii_digit()));
    }
    assert!(PairingCode::parse("123-456").is_some());
    assert!(PairingCode::parse("12345").is_none());
    assert!(PairingCode::parse("12345a").is_none());
    assert!(PairingCode::parse("1234567").is_none());
}

/// 코드를 모르는 가짜 Relay: Noise 를 받고(`noise_key`), Desktop 의 `pair` 에 아무 SPAKE2 답과 `reply_key` 를 대고, Desktop 의
/// 확인 뒤 아무 MAC 으로 `confirm` 을 보낸다(중간자가 Relay 를 사칭하는 자리).
async fn fake_relay(stream: LinkStream, noise_key: DeviceKey, reply_key: crate::PublicKey) {
    let secured = noise::respond(stream, &noise_key).await.unwrap();
    let mut framed = Framed::handshake(secured);
    let Some(hello) = raw_read(&mut framed).await else {
        return;
    };
    assert_eq!(hello.kind, Kind::Pair);
    let (_state, spake_b) = spake2::Spake2::<spake2::Ed25519Group>::start_b(
        &spake2::Password::new(b"000000"),
        &spake2::Identity::new(b"a"),
        &spake2::Identity::new(b"b"),
    );
    let b64 =
        |bytes: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    framed
        .write
        .write(&LinkFrame::control(
            Kind::Pair,
            json!({ "key": reply_key, "spake": b64(&spake_b) }),
        ))
        .await
        .unwrap();
    if raw_read(&mut framed).await.is_none() {
        return;
    }
    framed
        .write
        .write(&LinkFrame::control(
            Kind::Confirm,
            json!({ "mac": b64(&[7u8; 32]), "role": "owner" }),
        ))
        .await
        .unwrap();
}

/// 리뷰 F1: Relay 를 사칭하는 중간자(코드를 모름)가 확인 MAC 을 지어 보내면 Desktop 은 그 Relay 키를 받지 않는다
/// (`RelayNotProven` — 받으면 `CodePaired.machine` 이 중간자 키가 되어 그 뒤 `dial` 이 중간자를 그 호스트로 믿는다). Relay 가
/// Noise 상대와 다른 Ed25519 키를 대면 확인 전에 끊는다(`Protocol`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desktop_refuses_a_relay_that_does_not_prove_the_code() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    for other_key in [false, true] {
        let impostor = DeviceKey::generate().unwrap();
        let reply_key = if other_key {
            DeviceKey::generate().unwrap().public()
        } else {
            impostor.public()
        };
        let (d_end, r_end) = tokio::io::duplex(64 * 1024);
        let relay = tokio::spawn(fake_relay(LinkStream::split(r_end), impostor, reply_key));
        let secured = noise::initiate(LinkStream::split(d_end), &desktop)
            .await
            .unwrap();
        let paired = pair_with_code(secured, &identity(&desktop), "123456", "laptop", WAIT).await;
        let expected = if other_key {
            PairError::Protocol("the relay's key is not the noise peer".to_owned())
        } else {
            PairError::RelayNotProven
        };
        assert_eq!(paired.err(), Some(expected), "other key: {other_key}");
        relay.await.unwrap();
    }
}

/// 리뷰 R1: 페어링되는 키는 그 Noise 세션이 증명한 키다 — Noise 키 A 로 붙어 `pair.key` 에 남의 키 B 를 대면 Relay 는 시도를 쓰기
/// 전에 거절하고 표에 아무것도 넣지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pairing_for_a_key_other_than_the_noise_peer_is_refused() {
    let relay = noise_relay(Vec::new(), "123456", Role::Owner);
    let (a, b) = (
        DeviceKey::generate().unwrap(),
        DeviceKey::generate().unwrap(),
    );
    let mut framed = Framed::handshake(relay.secured(&a).await);
    let (_state, spake_a) = spake2::Spake2::<spake2::Ed25519Group>::start_a(
        &spake2::Password::new(b"123456"),
        &spake2::Identity::new(b"x"),
        &spake2::Identity::new(b"y"),
    );
    framed
        .write
        .write(&LinkFrame::control(
            Kind::Pair,
            json!({
                "protocol": PROTOCOL, "app": "1.2.3", "releaseChannel": "stable",
                "device": b.id(), "key": b.public(), "name": "x",
                "spake": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &spake_a),
            }),
        ))
        .await
        .unwrap();
    let reply = raw_read(&mut framed).await.unwrap();
    assert_eq!(reply.kind, Kind::Reject);
    assert_eq!(
        serde_json::from_value::<Reject>(reply.body).unwrap().reason,
        RejectReason::Protocol
    );
    assert_eq!(
        relay.book.attempts(),
        0,
        "a refused pairing spent an attempt"
    );
    assert!(relay.pairings.snapshot().is_empty());
}
