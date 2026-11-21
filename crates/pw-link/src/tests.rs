//! 링크 시험(가짜 전송 — 18-m7-plan M7-03 시험 ① ~ ⑩ · M7-02 ②). 보안 판정은 여기서 단위 시험으로 단다(`relay-live` 는 CI 에서만
//! 돈다). 공격자 쪽(규칙을 어기는 Desktop · Relay)은 크레이트 안의 서명 글([`transcript`])로 손으로 짠다.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_core::{Connection, Host, HostError, HostRouter, LocalHost, Target};
use pw_rpc::{Caller, ConnectionScope, Role, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::device::{DeviceKey, random_challenge};
use crate::frame::{Framed, Kind, LinkFrame};
use crate::handshake::{
    Auth, Challenge, Hello, PROTOCOL, Reject, RejectReason, Signer, Versions, Welcome, transcript,
};
use crate::{
    CloseReason, DesktopIdentity, DialError, Expect, LinkLimits, LinkState, Pairing, PairingTable,
    RelayIdentity, RelayParts, RemoteHost, ServeError, serve, transport,
};

const WAIT: Duration = Duration::from_secs(10);

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
struct Text {
    text: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
struct Big {
    bytes: u32,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
struct Ticks {
    count: u32,
    gap_ms: u32,
}

pw_rpc::rpc_method! {
    /// 받은 글을 돌려준다.
    TestEcho: "test.echo" {
        params: Text,
        result: Text,
        tier: Read,
    }

    /// `bytes` 글자의 글을 돌려준다.
    TestBig: "test.big" {
        params: Big,
        result: String,
        tier: Read,
    }

    /// 0 부터 `count` 개의 수를 `gap_ms` 마다.
    TestTicks: "test.ticks" {
        params: Ticks,
        stream: u32,
        tier: Read,
    }

    /// owner 만 부르는 쓰기.
    TestWrite: "test.write" {
        params: Text,
        result: Text,
        tier: Write,
    }
}

/// 스트림 핸들러가 버려진(끝난 · 취소된) 수.
#[derive(Default)]
struct Drops(AtomicUsize);

struct DropGuard(Arc<Drops>);

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Relay 쪽 Host: 시험 메서드를 든 LocalHost 에 연결 · 호출자를 적는다.
struct RelayHost {
    inner: LocalHost,
    callers: Mutex<Vec<Caller>>,
    drops: Arc<Drops>,
}

impl RelayHost {
    fn new(id: &str) -> Arc<Self> {
        let drops = Arc::new(Drops::default());
        let ticks_drops = Arc::clone(&drops);
        let inner = LocalHost::builder(id)
            .methods(move |router| {
                router
                    .unary(TestEcho, |_ctx, p: Text| async move { Ok(p) })
                    .unary(TestBig, |_ctx, p: Big| async move {
                        Ok("x".repeat(p.bytes as usize))
                    })
                    .unary(TestWrite, |_ctx, p: Text| async move { Ok(p) })
                    .stream(TestTicks, move |_ctx, p: Ticks, sink| {
                        let guard = DropGuard(Arc::clone(&ticks_drops));
                        async move {
                            let _guard = guard;
                            for i in 0..p.count {
                                sink.send(i).await?;
                                tokio::time::sleep(Duration::from_millis(u64::from(p.gap_ms)))
                                    .await;
                            }
                            Ok::<(), RpcError>(())
                        }
                    });
            })
            .build();
        Arc::new(Self {
            inner,
            callers: Mutex::default(),
            drops,
        })
    }

    fn connects(&self) -> usize {
        self.callers.lock().unwrap().len()
    }

    fn drops(&self) -> usize {
        self.drops.0.load(Ordering::SeqCst)
    }
}

impl Host for RelayHost {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn connect(&self, caller: Caller) -> pw_core::BoxFuture<'_, Result<Connection, HostError>> {
        self.callers.lock().unwrap().push(caller.clone());
        self.inner.connect(caller)
    }
}

fn versions() -> Versions {
    Versions::new("1.2.3", "stable")
}

/// 링크 하나의 양쪽: Relay(메모리 전송을 받아 [`serve`])와 Desktop([`RemoteHost`]).
struct Fixture {
    relay_host: Arc<RelayHost>,
    relay_key: Arc<DeviceKey>,
    desktop_key: Arc<DeviceKey>,
    pairings: PairingTable,
    remote: Arc<RemoteHost>,
    transport: Arc<transport::MemoryTransport>,
    serve_results: Arc<Mutex<Vec<Result<CloseReason, ServeError>>>>,
}

struct Setup {
    limits: LinkLimits,
    /// 페어링 표에 넣을 Desktop 의 role(`None` 이면 넣지 않는다).
    paired: Option<Role>,
    desktop_versions: Versions,
    /// Desktop 이 기대하는 머신 id 를 다른 키의 것으로.
    expect_other_machine: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            limits: LinkLimits::default(),
            paired: Some(Role::Owner),
            desktop_versions: versions(),
            expect_other_machine: false,
        }
    }
}

fn fixture(setup: Setup) -> Fixture {
    let relay_host = RelayHost::new("relay");
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let desktop_key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(
        setup
            .paired
            .map(|role| Pairing {
                key: desktop_key.public(),
                role,
                name: "mac".into(),
            })
            .into_iter()
            .collect(),
    );
    let (transport, acceptor) = transport::memory(64 * 1024);
    let transport = Arc::new(transport);
    let parts = RelayParts {
        identity: Arc::new(RelayIdentity {
            key: Arc::clone(&relay_key),
            versions: versions(),
        }),
        pairings: pairings.clone(),
        host: Arc::clone(&relay_host) as Arc<dyn Host>,
        limits: setup.limits,
    };
    let serve_results = Arc::new(Mutex::new(Vec::new()));
    {
        let results = Arc::clone(&serve_results);
        tokio::spawn(async move {
            while let Some(stream) = acceptor.accept().await {
                let (parts, results) = (parts.clone(), Arc::clone(&results));
                tokio::spawn(async move {
                    let result = serve(stream, &parts).await;
                    results.lock().unwrap().push(result);
                });
            }
        });
    }
    let expect = if setup.expect_other_machine {
        Expect::Machine(DeviceKey::generate().unwrap().id())
    } else {
        Expect::Machine(relay_key.id())
    };
    let remote = Arc::new(RemoteHost::new(
        "mini",
        Arc::clone(&transport) as Arc<dyn crate::Transport>,
        Arc::new(DesktopIdentity {
            key: Arc::clone(&desktop_key),
            versions: setup.desktop_versions,
        }),
        expect,
        setup.limits,
    ));
    Fixture {
        relay_host,
        relay_key,
        desktop_key,
        pairings,
        remote,
        transport,
        serve_results,
    }
}

/// JSON-RPC 를 주고받는 시험 클라이언트(창 자리).
struct Client {
    conn: Connection,
    backlog: Vec<Value>,
}

impl Client {
    async fn open(host: &dyn Host, caller: Caller) -> Self {
        Self {
            conn: host.connect(caller).await.expect("connect"),
            backlog: Vec::new(),
        }
    }

    fn request(&self, id: i64, method: &str, params: Value) {
        self.conn
            .sink
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn notify(&self, method: &str, params: Value) {
        self.conn
            .sink
            .send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    async fn next(&mut self) -> Option<Value> {
        tokio::time::timeout(WAIT, self.conn.source.recv())
            .await
            .expect("no frame in time")
    }

    async fn response(&mut self, id: i64) -> Result<Value, Value> {
        if let Some(at) = self.backlog.iter().position(|f| f["id"] == json!(id)) {
            let frame = self.backlog.remove(at);
            return outcome(&frame);
        }
        loop {
            let frame = self.next().await.expect("connection closed");
            if frame["id"] == json!(id) {
                return outcome(&frame);
            }
            self.backlog.push(frame);
        }
    }

    async fn call(&mut self, id: i64, method: &str, params: Value) -> Result<Value, Value> {
        self.request(id, method, params);
        self.response(id).await
    }

    /// 스트림 하나를 끝까지 받는다: (청크, 끝 상태).
    async fn drain_stream(&mut self, stream_id: u64) -> (Vec<Value>, String) {
        let mut chunks = Vec::new();
        let mut backlog = std::mem::take(&mut self.backlog).into_iter();
        loop {
            let frame = match backlog.next() {
                Some(frame) => frame,
                None => self.next().await.expect("connection closed"),
            };
            if frame["params"]["streamId"] != json!(stream_id) {
                self.backlog.push(frame);
                continue;
            }
            match frame["method"].as_str() {
                Some("stream.chunk") => chunks.push(frame["params"]["data"].clone()),
                Some("stream.end") => {
                    self.backlog.extend(backlog);
                    return (
                        chunks,
                        frame["params"]["status"].as_str().unwrap().to_owned(),
                    );
                }
                _ => self.backlog.push(frame),
            }
        }
    }
}

fn outcome(frame: &Value) -> Result<Value, Value> {
    match frame.get("error") {
        Some(error) => Err(error.clone()),
        None => Ok(frame["result"].clone()),
    }
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ── 가짜 Desktop · Relay(규칙을 어기는 쪽) ───────────────────────────────────────────────────────────────────────────

/// 손으로 짠 Desktop 의 `hello`. 돌려준 도전값으로 서명 글을 만든다.
async fn raw_hello(framed: &mut Framed, key: &DeviceKey, versions: Versions) -> Challenge {
    let challenge = Challenge(random_challenge().unwrap());
    let hello = Hello {
        versions,
        device: key.id(),
        key: key.public(),
        challenge,
        signature: None,
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Hello, json!(hello)))
        .await
        .unwrap();
    challenge
}

async fn raw_read(framed: &mut Framed) -> Option<LinkFrame> {
    match tokio::time::timeout(WAIT, framed.read.read())
        .await
        .expect("no frame in time")
    {
        Ok(Some((frame, _))) => Some(frame),
        Ok(None) | Err(_) => None,
    }
}

/// 링크가 닫힐 때까지 읽는다(하트비트 틀은 건너뛴다). 그 사이에 온 다른 틀.
async fn read_until_closed(framed: &mut Framed) -> Vec<LinkFrame> {
    let mut seen = Vec::new();
    while let Some(frame) = raw_read(framed).await {
        if !matches!(frame.kind, Kind::Ping | Kind::Pong) {
            seen.push(frame);
        }
    }
    seen
}

fn desktop_signature(
    key: &DeviceKey,
    mine: &Challenge,
    theirs: &Hello,
    relay_key: &crate::PublicKey,
) -> Auth {
    let text = transcript(
        Signer::Desktop,
        PROTOCOL,
        mine,
        &theirs.challenge,
        &key.public(),
        relay_key,
    );
    Auth {
        relay_key: *relay_key,
        signature: key.sign(&text),
    }
}

/// 짝이 맞는 Desktop 핸드셰이크를 손으로 끝낸다(그 뒤에 규칙을 어긴다).
async fn raw_desktop(fx: &Fixture) -> Framed {
    let mut framed = Framed::new(fx.transport_open().await);
    let mine = raw_hello(&mut framed, &fx.desktop_key, versions()).await;
    let theirs: Hello = serde_json::from_value(raw_read(&mut framed).await.unwrap().body).unwrap();
    let auth = desktop_signature(&fx.desktop_key, &mine, &theirs, &theirs.key);
    framed
        .write
        .write(&LinkFrame::control(Kind::Auth, json!(auth)))
        .await
        .unwrap();
    let welcome = raw_read(&mut framed).await.unwrap();
    assert_eq!(welcome.kind, Kind::Welcome);
    framed
}

impl Fixture {
    async fn transport_open(&self) -> crate::LinkStream {
        crate::Transport::open(&*self.transport).await.unwrap()
    }

    async fn attach(&self) {
        self.remote.attach().await.expect("attach");
    }

    fn caller(&self, role: Role, scope: ConnectionScope) -> Caller {
        Caller {
            role,
            scope,
            window: Some("project-1".into()),
        }
    }
}

fn connect_frame(channel: u32) -> LinkFrame {
    LinkFrame::new(
        channel,
        Kind::Connect,
        json!({"role": "owner", "scope": {"kind": "project", "id": "p1"}}),
    )
}

fn msg_frame(channel: u32) -> LinkFrame {
    LinkFrame::new(
        channel,
        Kind::Msg,
        json!({"jsonrpc": "2.0", "id": 1, "method": "test.write", "params": {"text": "x"}}),
    )
}

// ── ① 채널 둘의 요청 · 스트림 · 취소가 섞이지 않는다 ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_channels_keep_their_requests_streams_and_cancels_apart() {
    let fx = fixture(Setup::default());
    fx.attach().await;
    let mut a = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    let mut b = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;

    // 같은 요청 id 를 두 채널에서 쓴다 — 세션이 따로라 각자 자기 답을 받는다.
    a.request(7, "test.echo", json!({"text": "from a"}));
    b.request(7, "test.echo", json!({"text": "from b"}));
    assert_eq!(b.response(7).await.unwrap()["text"], "from b");
    assert_eq!(a.response(7).await.unwrap()["text"], "from a");

    // 두 채널에 같은 id 의 스트림. a 의 것을 취소해도 b 는 끝까지 돈다.
    let sa = a
        .call(1, "test.ticks", json!({"count": 100_000, "gapMs": 1}))
        .await
        .unwrap()["streamId"]
        .as_u64()
        .unwrap();
    let sb = b
        .call(1, "test.ticks", json!({"count": 5, "gapMs": 20}))
        .await
        .unwrap()["streamId"]
        .as_u64()
        .unwrap();
    a.notify("stream.credit", json!({"streamId": sa, "credit": 1000}));
    b.notify("stream.credit", json!({"streamId": sb, "credit": 1000}));
    a.notify("stream.cancel", json!({"streamId": sa}));
    let (chunks_a, end_a) = a.drain_stream(sa).await;
    let (chunks_b, end_b) = b.drain_stream(sb).await;
    assert_eq!(end_a, "cancelled");
    assert!(chunks_a.len() < 100_000);
    assert_eq!(end_b, "completed");
    assert_eq!(chunks_b, (0..5).map(|i| json!(i)).collect::<Vec<_>>());
    // 다른 채널의 틀이 끼지 않았다.
    assert!(
        a.backlog
            .iter()
            .all(|f| f["params"]["streamId"] != json!(sb) || sa == sb)
    );
    assert!(b.backlog.is_empty(), "b got stray frames: {:?}", b.backlog);
    assert_eq!(fx.relay_host.connects(), 2);
}

// ── ② 채널을 닫으면 Host 의 그 연결 스트림이 끝난다 ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_a_channel_ends_that_connection_on_the_host() {
    let fx = fixture(Setup::default());
    fx.attach().await;
    let mut a = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    let mut b = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    let sa = a
        .call(1, "test.ticks", json!({"count": 1_000_000, "gapMs": 5}))
        .await
        .unwrap()["streamId"]
        .as_u64()
        .unwrap();
    a.notify("stream.credit", json!({"streamId": sa, "credit": 10}));
    assert!(a.next().await.is_some(), "the stream did not start");
    assert_eq!(fx.relay_host.drops(), 0);

    drop(a);
    eventually("the host kept the closed channel's stream", || {
        fx.relay_host.drops() == 1
    })
    .await;
    // 다른 채널은 그대로 돈다.
    assert_eq!(
        b.call(2, "test.echo", json!({"text": "still"}))
            .await
            .unwrap()["text"],
        "still"
    );
    let link = fx.remote.link().unwrap();
    eventually("the closed channel stayed open", || {
        link.channel_count() == 1
    })
    .await;
}

// ── ③ 판이 다른 핸드셰이크는 RPC 하나도 지나지 않는다 ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_mismatch_lets_no_rpc_through() {
    for other in [
        Versions::new("1.2.4", "stable"),
        Versions::new("1.2.3", "preview"),
        Versions {
            protocol: PROTOCOL + 1,
            ..versions()
        },
    ] {
        let fx = fixture(Setup {
            desktop_versions: other.clone(),
            ..Setup::default()
        });
        let state = fx.remote.state();
        let err = fx.remote.attach().await.unwrap_err();
        assert_eq!(
            err,
            DialError::VersionMismatch {
                ours: other.clone(),
                theirs: Some(versions())
            }
        );
        assert!(matches!(*state.borrow(), LinkState::VersionMismatch { .. }));
        assert!(matches!(
            fx.remote.connect(Caller::new(Role::Owner)).await,
            Err(HostError::Disconnected(_))
        ));

        // 판이 다른 hello 뒤에 채널 열기 · 메시지를 몰아 보내도 Host 에 닿지 않는다.
        let mut framed = Framed::new(fx.transport_open().await);
        raw_hello(&mut framed, &fx.desktop_key, other).await;
        framed.write.write(&connect_frame(1)).await.unwrap();
        framed.write.write(&msg_frame(1)).await.unwrap();
        let seen = read_until_closed(&mut framed).await;
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].kind, Kind::Reject);
        let reject: Reject = serde_json::from_value(seen[0].body.clone()).unwrap();
        assert_eq!(reject.reason, RejectReason::Version);
        assert_eq!(fx.relay_host.connects(), 0);
    }
}

// ── ④ 본문 상한을 넘는 틀은 링크를 끊는다 ──────────────────────────────────────────────────────────────────────────────

/// 본문 없이 상한을 넘는 `Content-Length` 머리만 쓴다 — 받는 쪽은 본문을 읽지 않고 끊어야 한다.
async fn write_oversized_header(framed: &mut Framed) {
    use tokio::io::AsyncWriteExt;
    let header = format!("Content-Length: {}\r\n\r\n", pw_wire::MAX_BODY_BYTES + 1);
    framed
        .write
        .inner
        .write_all(header.as_bytes())
        .await
        .unwrap();
    framed.write.inner.flush().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_over_the_body_limit_breaks_the_link() {
    let fx = fixture(Setup::default());
    // 핸드셰이크 전.
    let mut framed = Framed::new(fx.transport_open().await);
    write_oversized_header(&mut framed).await;
    assert!(read_until_closed(&mut framed).await.is_empty());

    // 핸드셰이크 뒤 — 열린 채널이 있어도 링크째 끊는다.
    let mut framed = raw_desktop(&fx).await;
    framed.write.write(&connect_frame(1)).await.unwrap();
    while raw_read(&mut framed).await.unwrap().kind != Kind::Accepted {}
    write_oversized_header(&mut framed).await;
    assert!(read_until_closed(&mut framed).await.is_empty());
    eventually(
        "the relay did not end the link on the oversized frame",
        || {
            fx.serve_results
                .lock()
                .unwrap()
                .iter()
                .any(|r| matches!(r, Ok(CloseReason::Protocol(why)) if why.contains("larger than")))
        },
    )
    .await;

    // Desktop 쪽도 같다 — Relay 가 상한을 넘는 틀을 보내면 링크가 끊긴다.
    let (transport, acceptor) = transport::memory(64 * 1024);
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let remote = desktop_for(transport, &relay_key);
    let state = remote.state();
    let relay = tokio::spawn(async move {
        let mut framed = raw_relay(&acceptor, &relay_key, Role::Owner).await;
        write_oversized_header(&mut framed).await;
        read_until_closed(&mut framed).await;
    });
    remote.attach().await.unwrap();
    wait_state(state, |s| {
        matches!(
            s,
            LinkState::Disconnected {
                reason: CloseReason::Protocol(_)
            }
        )
    })
    .await;
    relay.await.unwrap();
}

/// 손으로 짠 Relay 와 붙을 Desktop(그 Relay 키를 기대한다).
fn desktop_for(transport: transport::MemoryTransport, relay_key: &DeviceKey) -> RemoteHost {
    RemoteHost::new(
        "mini",
        Arc::new(transport),
        Arc::new(DesktopIdentity {
            key: Arc::new(DeviceKey::generate().unwrap()),
            versions: versions(),
        }),
        Expect::Machine(relay_key.id()),
        LinkLimits::default(),
    )
}

/// 짝이 맞는 Relay 핸드셰이크를 손으로 끝낸다(Desktop 서명은 보지 않는다 — 그 뒤에 규칙을 어긴다).
async fn raw_relay(acceptor: &transport::MemoryAcceptor, key: &DeviceKey, role: Role) -> Framed {
    let mut framed = Framed::new(acceptor.accept().await.unwrap());
    let theirs: Hello = serde_json::from_value(raw_read(&mut framed).await.unwrap().body).unwrap();
    let challenge = Challenge(random_challenge().unwrap());
    let text = transcript(
        Signer::Relay,
        PROTOCOL,
        &theirs.challenge,
        &challenge,
        &theirs.key,
        &key.public(),
    );
    let hello = Hello {
        versions: versions(),
        device: key.id(),
        key: key.public(),
        challenge,
        signature: Some(key.sign(&text)),
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Hello, json!(hello)))
        .await
        .unwrap();
    assert_eq!(raw_read(&mut framed).await.unwrap().kind, Kind::Auth);
    framed
        .write
        .write(&LinkFrame::control(Kind::Welcome, json!(Welcome { role })))
        .await
        .unwrap();
    framed
}

async fn wait_state(
    mut state: tokio::sync::watch::Receiver<LinkState>,
    want: impl Fn(&LinkState) -> bool,
) -> LinkState {
    tokio::time::timeout(WAIT * 10, async {
        loop {
            {
                let now = state.borrow_and_update();
                if want(&now) {
                    return now.clone();
                }
            }
            state.changed().await.expect("state sender gone");
        }
    })
    .await
    .expect("the link state did not arrive")
}

// ── ⑤ 보내기 버퍼를 채운 채널만 닫히고 다른 채널은 돈다 ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_channel_with_a_full_send_buffer_closes() {
    let limits = LinkLimits {
        window: 4096,
        send_buffer: 64 * 1024,
        ..LinkLimits::default()
    };
    let fx = fixture(Setup {
        limits,
        ..Setup::default()
    });
    fx.attach().await;
    let mut stuck = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    let mut other = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    // 이 창은 읽지 않는다 — 창(credit)이 돌아오지 않아 Relay 에 그 채널의 응답이 쌓인다.
    for id in 0..10 {
        stuck.request(id, "test.big", json!({"bytes": 32 * 1024}));
    }
    // 그 사이 다른 채널은 돈다.
    for id in 0..20 {
        let text = format!("ping {id}");
        assert_eq!(
            other
                .call(id, "test.echo", json!({"text": text}))
                .await
                .unwrap()["text"],
            text
        );
    }
    let link = fx.remote.link().unwrap();
    eventually("the stuck channel stayed open", || {
        link.channel_count() == 1
    })
    .await;
    // 막힌 채널은 창 안에서 받은 것까지만 내고 끝난다.
    let mut got = 0;
    while let Some(frame) = stuck.next().await {
        assert!(frame.get("result").is_some(), "{frame}");
        got += 1;
    }
    assert!(got < 10, "the stuck channel got every response ({got})");
    assert_eq!(
        other
            .call(99, "test.echo", json!({"text": "after"}))
            .await
            .unwrap()["text"],
        "after"
    );
    assert!(matches!(
        *fx.remote.state().borrow(),
        LinkState::Connected { .. }
    ));
}

/// 창을 지키지 않는 상대(credit 없이 계속 보냄)의 채널은 받는 쪽이 닫는다 — 받는 메모리는 창으로 묶인다(M7-P19).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_that_overruns_the_window_loses_the_channel() {
    let (transport, acceptor) = transport::memory(1024 * 1024);
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let remote = desktop_for(transport, &relay_key);
    let key = Arc::clone(&relay_key);
    let relay = tokio::spawn(async move {
        let mut framed = raw_relay(&acceptor, &key, Role::Owner).await;
        loop {
            let frame = raw_read(&mut framed).await.unwrap();
            if frame.kind == Kind::Connect {
                framed
                    .write
                    .write(&LinkFrame::new(frame.channel, Kind::Accepted, Value::Null))
                    .await
                    .unwrap();
                // 창(기본 1 MiB)을 넘도록 credit 없이 보낸다.
                let big = "y".repeat(256 * 1024);
                for i in 0..8 {
                    let body = json!({"jsonrpc": "2.0", "method": "flood", "params": {"i": i, "big": big}});
                    framed
                        .write
                        .write(&LinkFrame::new(frame.channel, Kind::Msg, body))
                        .await
                        .unwrap();
                }
                return read_until_closed(&mut framed).await;
            }
        }
    });
    remote.attach().await.unwrap();
    let mut client = Client::open(&remote, Caller::new(Role::Owner)).await;
    let mut got = 0;
    // 창을 넘긴 메시지부터는 받지 않고 채널이 끝난다.
    tokio::time::sleep(Duration::from_millis(200)).await;
    while client.next().await.is_some() {
        got += 1;
    }
    assert!(got < 8, "received {got} messages past the window");
    remote.detach();
    let seen = relay.await.unwrap();
    assert!(
        seen.iter()
            .any(|f| f.kind == Kind::Close && f.body["reason"] == "flow control violated"),
        "{seen:?}"
    );
}

// ── ⑥ 두 Host 에 같은 프로젝트 id 를 묶어도 라우팅이 갈린다 ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_project_id_on_the_local_and_a_remote_host_routes_apart() {
    let fx = fixture(Setup::default());
    fx.attach().await;
    let router = HostRouter::new();
    router.add_host(Arc::new(LocalHost::new("local"))).unwrap();
    router
        .add_host(Arc::clone(&fx.remote) as Arc<dyn Host>)
        .unwrap();
    router.bind_project("p1".into(), "local").unwrap();
    router.bind_project("p1".into(), "mini").unwrap();
    let info = |target: Target| {
        let router = &router;
        async move {
            let mut client = Client {
                conn: router
                    .connect(&target, Caller::new(Role::Owner))
                    .await
                    .unwrap(),
                backlog: Vec::new(),
            };
            client.call(1, "host.info", Value::Null).await.unwrap()["hostId"].clone()
        }
    };
    assert_eq!(info(Target::ProjectId("p1".into())).await, "local");
    assert_eq!(
        info(Target::Member {
            host: "mini".into(),
            project: "p1".into()
        })
        .await,
        "relay"
    );
    assert_eq!(
        fx.relay_host.callers.lock().unwrap()[0].scope,
        ConnectionScope::Project("p1".into())
    );
}

// ── ⑦ 페어링 표에 없는 Desktop 키 · 틀린 서명의 링크는 RPC 하나도 지나지 않는다 ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpaired_desktop_gets_no_role_and_no_rpc() {
    let fx = fixture(Setup {
        paired: None,
        ..Setup::default()
    });
    let state = fx.remote.state();
    assert_eq!(
        fx.remote.attach().await.unwrap_err(),
        DialError::Rejected(RejectReason::Unpaired)
    );
    assert!(matches!(
        *state.borrow(),
        LinkState::Disconnected {
            reason: CloseReason::Handshake(_)
        }
    ));

    // 증명이 맞아도 표에 없으면, 뒤에 몰아 보낸 채널 열기 · 승인 답은 Host 에 닿지 않는다.
    let mut framed = Framed::new(fx.transport_open().await);
    let mine = raw_hello(&mut framed, &fx.desktop_key, versions()).await;
    let theirs: Hello = serde_json::from_value(raw_read(&mut framed).await.unwrap().body).unwrap();
    let auth = desktop_signature(&fx.desktop_key, &mine, &theirs, &theirs.key);
    framed
        .write
        .write(&LinkFrame::control(Kind::Auth, json!(auth)))
        .await
        .unwrap();
    framed.write.write(&connect_frame(1)).await.unwrap();
    framed.write.write(&msg_frame(1)).await.unwrap();
    let seen = read_until_closed(&mut framed).await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        serde_json::from_value::<Reject>(seen[0].body.clone())
            .unwrap()
            .reason,
        RejectReason::Unpaired
    );
    assert_eq!(fx.relay_host.connects(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_signature_or_a_false_device_id_gets_no_rpc() {
    let fx = fixture(Setup::default());
    // 표에 있는 키를 대지만 그 비밀키가 없다 — 다른 키로 서명한다.
    let mut framed = Framed::new(fx.transport_open().await);
    let mine = raw_hello(&mut framed, &fx.desktop_key, versions()).await;
    let theirs: Hello = serde_json::from_value(raw_read(&mut framed).await.unwrap().body).unwrap();
    let forger = DeviceKey::generate().unwrap();
    let text = transcript(
        Signer::Desktop,
        PROTOCOL,
        &mine,
        &theirs.challenge,
        &fx.desktop_key.public(),
        &theirs.key,
    );
    let auth = Auth {
        relay_key: theirs.key,
        signature: forger.sign(&text),
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Auth, json!(auth)))
        .await
        .unwrap();
    framed.write.write(&connect_frame(1)).await.unwrap();
    framed.write.write(&msg_frame(1)).await.unwrap();
    let seen = read_until_closed(&mut framed).await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        serde_json::from_value::<Reject>(seen[0].body.clone())
            .unwrap()
            .reason,
        RejectReason::BadSignature
    );

    // 기기 id 가 그 키의 지문이 아니다.
    let mut framed = Framed::new(fx.transport_open().await);
    let hello = Hello {
        versions: versions(),
        device: DeviceKey::generate().unwrap().id(),
        key: fx.desktop_key.public(),
        challenge: Challenge([0; 32]),
        signature: None,
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Hello, json!(hello)))
        .await
        .unwrap();
    let seen = read_until_closed(&mut framed).await;
    assert_eq!(
        serde_json::from_value::<Reject>(seen[0].body.clone())
            .unwrap()
            .reason,
        RejectReason::Protocol
    );

    // 첫 틀이 hello 가 아니다(채널 열기부터).
    let mut framed = Framed::new(fx.transport_open().await);
    framed.write.write(&connect_frame(1)).await.unwrap();
    framed.write.write(&msg_frame(1)).await.unwrap();
    let seen = read_until_closed(&mut framed).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(fx.relay_host.connects(), 0);
}

/// 지난 연결에서 엿본 서명을 새 연결에 내밀어도(재생) Relay 의 새 도전값과 맞지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_signature_is_refused() {
    let fx = fixture(Setup::default());
    let mut first = Framed::new(fx.transport_open().await);
    let mine = raw_hello(&mut first, &fx.desktop_key, versions()).await;
    let theirs: Hello = serde_json::from_value(raw_read(&mut first).await.unwrap().body).unwrap();
    let seen_auth = desktop_signature(&fx.desktop_key, &mine, &theirs, &theirs.key);
    first
        .write
        .write(&LinkFrame::control(Kind::Auth, json!(seen_auth)))
        .await
        .unwrap();
    assert_eq!(raw_read(&mut first).await.unwrap().kind, Kind::Welcome);

    // 공격자: Desktop 의 공개키 · 지난 도전값 · 지난 서명만 안다.
    let mut replay = Framed::new(fx.transport_open().await);
    let hello = Hello {
        versions: versions(),
        device: fx.desktop_key.id(),
        key: fx.desktop_key.public(),
        challenge: mine,
        signature: None,
    };
    replay
        .write
        .write(&LinkFrame::control(Kind::Hello, json!(hello)))
        .await
        .unwrap();
    let fresh: Hello = serde_json::from_value(raw_read(&mut replay).await.unwrap().body).unwrap();
    assert_ne!(fresh.challenge, theirs.challenge);
    replay
        .write
        .write(&LinkFrame::control(Kind::Auth, json!(seen_auth)))
        .await
        .unwrap();
    replay.write.write(&connect_frame(1)).await.unwrap();
    let seen = read_until_closed(&mut replay).await;
    assert_eq!(
        serde_json::from_value::<Reject>(seen[0].body.clone())
            .unwrap()
            .reason,
        RejectReason::BadSignature
    );
    assert_eq!(fx.relay_host.connects(), 0);
}

// ── ⑧ 그 호스트 항목에 기억한 머신 id 와 다른 키의 Relay 에는 붙지 않는다 ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_with_another_key_than_remembered_is_not_attached() {
    let fx = fixture(Setup {
        expect_other_machine: true,
        ..Setup::default()
    });
    let state = fx.remote.state();
    let err = fx.remote.attach().await.unwrap_err();
    let DialError::MachineMismatch { found, .. } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(*found, fx.relay_key.id());
    assert!(matches!(*state.borrow(), LinkState::MachineMismatch { .. }));
    assert!(fx.remote.link().is_none());
    assert_eq!(fx.relay_host.connects(), 0);
    // Desktop 은 서명하지 않았다 — Relay 는 증명을 받지 못하고 끝났다.
    eventually("the relay is still waiting", || {
        !fx.serve_results.lock().unwrap().is_empty()
    })
    .await;
    assert!(matches!(
        fx.serve_results.lock().unwrap()[0],
        Err(ServeError::Io(_))
    ));
}

/// 처음 붙는 호스트는 본 머신 id 를 돌려주고 그 뒤로 그 id 만 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_contact_learns_the_machine_id() {
    let fx = fixture(Setup::default());
    let remote = RemoteHost::new(
        "mini",
        Arc::clone(&fx.transport) as Arc<dyn crate::Transport>,
        Arc::new(DesktopIdentity {
            key: Arc::clone(&fx.desktop_key),
            versions: versions(),
        }),
        Expect::FirstContact,
        LinkLimits::default(),
    );
    assert_eq!(remote.attach().await.unwrap(), fx.relay_key.id());
    assert_eq!(remote.attach().await.unwrap(), fx.relay_key.id());
}

// ── ⑨ 다른 Relay 의 도전값으로 만든 Desktop 서명은 거절(채널 바인딩) ─────────────────────────────────────────────────────

/// Relay A(털린 머신)가 Desktop 과 Relay B 사이에서 도전값을 중계해 Desktop 의 서명을 B 에 내민다. Desktop 은 A 의 키로 서명 글을
/// 만들었으므로 B 는 받지 않는다 — `relayKey` 를 B 의 키로 바꿔 내밀어도 서명이 맞지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signature_made_for_another_relay_is_refused() {
    let fx = fixture(Setup::default());
    for swap_relay_key in [false, true] {
        let (to_a, a_acceptor) = transport::memory(64 * 1024);
        let a_key = DeviceKey::generate().unwrap();
        // Desktop 은 이 호스트를 A 로 안다(A 의 머신 id 를 기억).
        let desktop = RemoteHost::new(
            "a",
            Arc::new(to_a),
            Arc::new(DesktopIdentity {
                key: Arc::clone(&fx.desktop_key),
                versions: versions(),
            }),
            Expect::Machine(a_key.id()),
            LinkLimits::default(),
        );
        let attach = tokio::spawn(async move { desktop.attach().await });

        let mut from_desktop = Framed::new(a_acceptor.accept().await.unwrap());
        let mut to_b = Framed::new(fx.transport_open().await);
        let desktop_hello = raw_read(&mut from_desktop).await.unwrap();
        let d: Hello = serde_json::from_value(desktop_hello.body.clone()).unwrap();
        to_b.write.write(&desktop_hello).await.unwrap();
        let b: Hello = serde_json::from_value(raw_read(&mut to_b).await.unwrap().body).unwrap();
        // A 는 B 의 도전값을 자기 hello 에 실어 Desktop 에 보낸다(A 의 서명은 맞다).
        let a_text = transcript(
            Signer::Relay,
            PROTOCOL,
            &d.challenge,
            &b.challenge,
            &d.key,
            &a_key.public(),
        );
        let a_hello = Hello {
            versions: versions(),
            device: a_key.id(),
            key: a_key.public(),
            challenge: b.challenge,
            signature: Some(a_key.sign(&a_text)),
        };
        from_desktop
            .write
            .write(&LinkFrame::control(Kind::Hello, json!(a_hello)))
            .await
            .unwrap();
        let auth_frame = raw_read(&mut from_desktop).await.unwrap();
        assert_eq!(auth_frame.kind, Kind::Auth);
        let mut auth: Auth = serde_json::from_value(auth_frame.body).unwrap();
        assert_eq!(auth.relay_key, a_key.public());
        if swap_relay_key {
            auth.relay_key = b.key;
        }
        to_b.write
            .write(&LinkFrame::control(Kind::Auth, json!(auth)))
            .await
            .unwrap();
        to_b.write.write(&connect_frame(1)).await.unwrap();
        to_b.write.write(&msg_frame(1)).await.unwrap();
        let seen = read_until_closed(&mut to_b).await;
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(
            serde_json::from_value::<Reject>(seen[0].body.clone())
                .unwrap()
                .reason,
            RejectReason::BadSignature,
            "swap {swap_relay_key}"
        );
        drop(from_desktop);
        assert!(attach.await.unwrap().is_err());
    }
    assert_eq!(fx.relay_host.connects(), 0);
}

// ── ⑩ 응답 없는 전송은 시한 뒤 끊김 이벤트(가짜 시계) ─────────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn a_silent_relay_ends_in_a_disconnected_event_after_the_deadline() {
    let (transport, acceptor) = transport::memory(64 * 1024);
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let remote = desktop_for(transport, &relay_key);
    let state = remote.state();
    // 핸드셰이크까지만 하고 그 뒤로는 읽지도 쓰지도 않는다(스트림은 쥔다 — 전송은 닫히지 않는다).
    let key = Arc::clone(&relay_key);
    let silent = tokio::spawn(async move {
        let framed = raw_relay(&acceptor, &key, Role::Owner).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
        drop(framed);
    });
    remote.attach().await.unwrap();
    let started = tokio::time::Instant::now();
    let ended = wait_state(state, |s| matches!(s, LinkState::Disconnected { .. })).await;
    assert_eq!(
        ended,
        LinkState::Disconnected {
            reason: CloseReason::HeartbeatTimeout
        }
    );
    let waited = started.elapsed();
    let limits = LinkLimits::default();
    assert!(waited >= limits.dead_after, "gave up after {waited:?}");
    assert!(
        waited <= limits.dead_after + limits.ping_every,
        "waited {waited:?}"
    );
    assert!(matches!(
        remote.connect(Caller::new(Role::Owner)).await,
        Err(HostError::Disconnected(_))
    ));
    silent.abort();
}

/// Relay 쪽도 같다 — 말이 없는 Desktop 의 링크는 시한 뒤 닫히고 그 채널의 Host 연결이 걷힌다.
#[tokio::test(start_paused = true)]
async fn a_silent_desktop_is_dropped_by_the_relay() {
    let fx = fixture(Setup::default());
    let mut framed = raw_desktop(&fx).await;
    framed.write.write(&connect_frame(1)).await.unwrap();
    while raw_read(&mut framed).await.unwrap().kind != Kind::Accepted {}
    let started = tokio::time::Instant::now();
    // 이제 읽지도 쓰지도 않는다.
    eventually_paused(|| !fx.serve_results.lock().unwrap().is_empty()).await;
    assert_eq!(
        *fx.serve_results.lock().unwrap(),
        [Ok(CloseReason::HeartbeatTimeout)]
    );
    let waited = started.elapsed();
    assert!(waited >= LinkLimits::default().dead_after, "{waited:?}");
    drop(framed);
}

async fn eventually_paused(mut check: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("never happened");
}

// ── M7-02 ② 링크 호출자의 role · 범위 ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_readonly_link_claiming_owner_is_cut_to_readonly() {
    let fx = fixture(Setup {
        paired: Some(Role::Readonly),
        ..Setup::default()
    });
    fx.attach().await;
    assert_eq!(fx.remote.link().unwrap().role(), Role::Readonly);
    let mut client = Client::open(
        &*fx.remote,
        fx.caller(Role::Owner, ConnectionScope::Project("p1".into())),
    )
    .await;
    let denied = client
        .call(1, "test.write", json!({"text": "x"}))
        .await
        .unwrap_err();
    assert_eq!(denied["code"], json!(pw_rpc::code::PERMISSION_DENIED));
    assert_eq!(denied["data"]["role"], "readonly");
    assert_eq!(
        client
            .call(2, "test.echo", json!({"text": "ok"}))
            .await
            .unwrap()["text"],
        "ok"
    );
    let seen = fx.relay_host.callers.lock().unwrap()[0].clone();
    assert_eq!(seen.role, Role::Readonly);
    assert_eq!(
        seen.scope,
        ConnectionScope::Host,
        "a readonly link got a project scope"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owner_link_keeps_its_claims_with_the_window_tagged_by_device() {
    let fx = fixture(Setup::default());
    fx.attach().await;
    let mut client = Client::open(
        &*fx.remote,
        fx.caller(Role::Owner, ConnectionScope::Project("p1".into())),
    )
    .await;
    assert_eq!(
        client
            .call(1, "test.write", json!({"text": "x"}))
            .await
            .unwrap()["text"],
        "x"
    );
    let seen = fx.relay_host.callers.lock().unwrap()[0].clone();
    assert_eq!(seen.role, Role::Owner);
    assert_eq!(seen.scope, ConnectionScope::Project("p1".into()));
    assert_eq!(
        seen.window,
        Some(format!("{}/project-1", fx.desktop_key.id()))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_scope_over_a_link_is_refused() {
    let fx = fixture(Setup::default());
    fx.attach().await;
    let result = fx
        .remote
        .connect(Caller::file(Role::Owner, "/tmp/a.txt".into()))
        .await;
    assert!(
        matches!(result, Err(HostError::Refused(_))),
        "{:?}",
        result.err()
    );
    assert_eq!(fx.relay_host.connects(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_channel_count_is_capped_per_link() {
    let fx = fixture(Setup {
        limits: LinkLimits {
            max_channels: 2,
            ..LinkLimits::default()
        },
        ..Setup::default()
    });
    fx.attach().await;
    let one = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    let _two = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    assert!(matches!(
        fx.remote.connect(Caller::new(Role::Owner)).await,
        Err(HostError::Refused(_))
    ));
    drop(one);
    let link = fx.remote.link().unwrap();
    eventually("the closed channel stayed counted", || {
        link.channel_count() == 1
    })
    .await;
    // Relay 쪽에서도 채널이 걷힌 뒤에는 다시 열린다.
    let mut opened = None;
    for _ in 0..100 {
        if let Ok(conn) = fx.remote.connect(Caller::new(Role::Owner)).await {
            opened = Some(conn);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(opened.is_some());
}

/// 기기 해제 · role 변경은 그 기기의 링크를 끊는다 — 열린 구독은 role 을 다시 보지 않으므로 끊는 것이 계약이다(M7-02 리뷰 권고 4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpairing_or_changing_the_role_drops_the_link_and_its_subscriptions() {
    for change in ["remove", "readonly"] {
        let fx = fixture(Setup::default());
        fx.attach().await;
        let state = fx.remote.state();
        let mut client = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
        client
            .call(1, "events.subscribe", json!({"topics": ["task.progress"]}))
            .await
            .unwrap();
        // 다른 기기의 변경은 이 링크를 건드리지 않는다.
        fx.pairings.upsert(Pairing {
            key: DeviceKey::generate().unwrap().public(),
            role: Role::Owner,
            name: "other".into(),
        });
        assert_eq!(
            client
                .call(2, "test.echo", json!({"text": "on"}))
                .await
                .unwrap()["text"],
            "on"
        );

        match change {
            "remove" => assert!(fx.pairings.remove(&fx.desktop_key.public())),
            _ => fx.pairings.upsert(Pairing {
                key: fx.desktop_key.public(),
                role: Role::Readonly,
                name: "mac".into(),
            }),
        }
        wait_state(state, |s| {
            *s == LinkState::Disconnected {
                reason: CloseReason::PeerClosed,
            }
        })
        .await;
        assert_eq!(
            client.next().await,
            None,
            "{change}: the subscription outlived the link"
        );
        eventually("the relay did not drop the link", || {
            fx.serve_results.lock().unwrap().first() == Some(&Ok(CloseReason::PairingChanged))
        })
        .await;
    }
}
