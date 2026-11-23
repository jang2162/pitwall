//! 이어 붙기(18-m7-plan M7-09 ① — 완료 기준 ① ② ⑧). 전송은 가운데에서 끊을 수 있는 가짜 전송이다 — 끊으면 가운데가 잠깐 받은
//! 바이트를 버리다가(실제 끊김처럼 그사이 보낸 틀은 사라진다) 양쪽에 끝을 보인다.

use std::sync::atomic::AtomicBool;

use tokio::task::AbortHandle;

use super::*;
use crate::desktop::handshake;
use crate::handshake::Resume;
use crate::transport::{LinkStream, Transport};

/// 가운데에서 끊을 수 있는 전송. `open` 마다 Desktop ↔ 가운데 ↔ Relay 의 두 duplex 를 잇고, [`Self::cut`] 은 가운데를 버린다.
struct Cuttable {
    tx: tokio::sync::mpsc::UnboundedSender<LinkStream>,
    pipes: Mutex<Vec<(AbortHandle, Arc<AtomicBool>)>>,
    /// 참이면 새 전송을 열지 못한다(네트워크가 아직 없다).
    down: AtomicBool,
    opens: AtomicUsize,
}

impl Cuttable {
    /// 가운데가 받는 바이트를 버리기 시작하고(그사이 양쪽이 보낸 틀은 사라진다) 조금 뒤 전송을 끊는다.
    async fn cut(&self) {
        let pipes: Vec<_> = self.pipes.lock().unwrap().drain(..).collect();
        for (_, lost) in &pipes {
            lost.store(true, Ordering::SeqCst);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        for (pipe, _) in pipes {
            pipe.abort();
        }
    }
}

/// 한 방향 잇기. `lost` 면 읽은 것을 버린다.
async fn pipe(
    mut from: tokio::io::ReadHalf<tokio::io::DuplexStream>,
    mut to: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lost: Arc<AtomicBool>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let Ok(n) = from.read(&mut buf).await else {
            return;
        };
        if n == 0 {
            return;
        }
        if lost.load(Ordering::SeqCst) {
            continue;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

impl Transport for Cuttable {
    fn describe(&self) -> String {
        "cuttable".to_owned()
    }

    fn open(&self) -> pw_core::BoxFuture<'_, std::io::Result<LinkStream>> {
        Box::pin(async move {
            if self.down.load(Ordering::SeqCst) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "the network is down",
                ));
            }
            self.opens.fetch_add(1, Ordering::SeqCst);
            let (desktop, near) = tokio::io::duplex(64 * 1024);
            let (far, relay) = tokio::io::duplex(64 * 1024);
            let lost = Arc::new(AtomicBool::new(false));
            let (near_read, near_write) = tokio::io::split(near);
            let (far_read, far_write) = tokio::io::split(far);
            let both = tokio::spawn({
                let lost = Arc::clone(&lost);
                async move {
                    tokio::join!(
                        pipe(near_read, far_write, Arc::clone(&lost)),
                        pipe(far_read, near_write, lost),
                    );
                }
            });
            self.pipes.lock().unwrap().push((both.abort_handle(), lost));
            self.tx
                .send(LinkStream::split(relay))
                .map_err(|_| std::io::Error::other("no acceptor"))?;
            Ok(LinkStream::split(desktop))
        })
    }
}

struct Resumable {
    relay_host: Arc<RelayHost>,
    relay_key: Arc<DeviceKey>,
    desktop_key: Arc<DeviceKey>,
    pairings: PairingTable,
    remote: Arc<RemoteHost>,
    transport: Arc<Cuttable>,
    resumes: Resumes,
    serve_results: Arc<Mutex<Vec<Result<CloseReason, ServeError>>>>,
}

fn resumable(limits: LinkLimits) -> Resumable {
    let relay_host = RelayHost::new("relay");
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let desktop_key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(vec![Pairing {
        key: desktop_key.public(),
        role: Role::Owner,
        name: "mac".into(),
    }]);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let transport = Arc::new(Cuttable {
        tx,
        pipes: Mutex::default(),
        down: AtomicBool::new(false),
        opens: AtomicUsize::new(0),
    });
    let parts = RelayParts::new(
        Arc::new(RelayIdentity {
            key: Arc::clone(&relay_key),
            versions: versions(),
        }),
        pairings.clone(),
        Arc::clone(&relay_host) as Arc<dyn Host>,
        limits,
    );
    let resumes = parts.resumes.clone();
    let serve_results = Arc::new(Mutex::new(Vec::new()));
    {
        let results = Arc::clone(&serve_results);
        tokio::spawn(async move {
            while let Some(stream) = rx.recv().await {
                let (parts, results) = (parts.clone(), Arc::clone(&results));
                tokio::spawn(async move {
                    let result = serve(stream, &parts).await;
                    results.lock().unwrap().push(result);
                });
            }
        });
    }
    let remote = Arc::new(RemoteHost::new(
        "mini",
        Arc::clone(&transport) as Arc<dyn crate::Transport>,
        Arc::new(DesktopIdentity {
            key: Arc::clone(&desktop_key),
            versions: versions(),
        }),
        Expect::Machine(relay_key.id()),
        limits,
    ));
    Resumable {
        relay_host,
        relay_key,
        desktop_key,
        pairings,
        remote,
        transport,
        resumes,
        serve_results,
    }
}

fn quick() -> LinkLimits {
    LinkLimits {
        resume_within: Duration::from_secs(20),
        ..LinkLimits::default()
    }
}

/// 스트림의 청크를 끝까지(또는 연결이 닫힐 때까지) 받는다.
async fn ticks(client: &mut Client, count: u32, gap_ms: u32) -> u64 {
    let stream = client
        .call(1, "test.ticks", json!({"count": count, "gapMs": gap_ms}))
        .await
        .unwrap()["streamId"]
        .as_u64()
        .unwrap();
    client.notify(
        "stream.credit",
        json!({"streamId": stream, "credit": 1_000_000}),
    );
    stream
}

/// 완료 기준 ①: 시한 안에 다시 붙으면 스트림의 청크를 하나도 잃지 않는다 — 끊는 순간 가운데에 있던 틀도 다시 온다. Host 쪽 연결은
/// 하나 그대로다(새 연결 · 스트림 다시 열기 없음). 상태는 `Reconnecting` 을 거쳐 `Connected` 로 돌아온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_that_resumes_within_the_deadline_loses_no_stream_chunk() {
    let fx = resumable(quick());
    fx.remote.attach().await.unwrap();
    let mut state = fx.remote.state();
    let mut client = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    let count = 400;
    let stream = ticks(&mut client, count, 2).await;
    // 몇 개를 받은 뒤 끊고, 잠깐 붙지 못하게 한다.
    for _ in 0..20 {
        client.next().await.expect("the stream did not start");
    }
    fx.transport.down.store(true, Ordering::SeqCst);
    fx.transport.cut().await;
    state = {
        let reconnecting = wait_state(state, |s| *s == LinkState::Reconnecting).await;
        assert_eq!(reconnecting, LinkState::Reconnecting);
        fx.remote.state()
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    fx.transport.down.store(false, Ordering::SeqCst);
    wait_state(state, |s| matches!(s, LinkState::Connected { .. })).await;

    let (chunks, end) = client.drain_stream(stream).await;
    // 끊기 전에 받은 20개는 `next` 가 이미 꺼냈다 — 남은 것이 그 뒤에 빈틈 · 겹침 없이 이어진다.
    let first = chunks
        .first()
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(count));
    assert_eq!(
        first, 20,
        "chunks before the cut went missing or came twice"
    );
    assert_eq!(
        chunks,
        (20..count).map(|i| json!(i)).collect::<Vec<_>>(),
        "a chunk was lost or repeated across the resume"
    );
    assert_eq!(end, "completed");
    assert_eq!(
        fx.relay_host.connects(),
        1,
        "the resume opened a new host connection"
    );
    assert!(fx.transport.opens.load(Ordering::SeqCst) >= 2);
    eventually("the resumed transport's serve did not hand over", || {
        fx.serve_results
            .lock()
            .unwrap()
            .contains(&Ok(CloseReason::Resumed))
    })
    .await;
    // 이어 붙은 링크로 새 요청도 지난다.
    assert_eq!(
        client
            .call(9, "test.echo", json!({"text": "after"}))
            .await
            .unwrap()["text"],
        "after"
    );
}

/// 완료 기준 ②: 시한이 지나면 채널이 닫히고 Host 의 보통 정리가 돈다(스트림 핸들러가 버려진다) — Desktop 은 `Disconnected`, 창의
/// 연결은 끝나고, Relay 의 이어 붙기 등록부는 빈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn past_the_deadline_the_link_closes_and_the_host_cleans_up() {
    let fx = resumable(LinkLimits {
        resume_within: Duration::from_millis(600),
        ..LinkLimits::default()
    });
    fx.remote.attach().await.unwrap();
    let state = fx.remote.state();
    let mut client = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    ticks(&mut client, 1_000_000, 5).await;
    client.next().await.expect("the stream did not start");
    assert_eq!(fx.resumes.len(), 1);
    fx.transport.down.store(true, Ordering::SeqCst);
    fx.transport.cut().await;
    // 시한 전에는 Host 의 스트림이 산다.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fx.relay_host.drops(),
        0,
        "the host gave up before the deadline"
    );
    wait_state(state, |s| matches!(s, LinkState::Disconnected { .. })).await;
    eventually("the host kept the stream past the deadline", || {
        fx.relay_host.drops() == 1
    })
    .await;
    eventually("the relay kept the expired link", || fx.resumes.is_empty()).await;
    // 창의 연결은 끝난다(남은 것을 다 꺼내면 `None`).
    let ended = tokio::time::timeout(WAIT, async {
        while client.conn.source.recv().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "the window's connection outlived the link");
}

/// 손으로 이어 붙기 핸드셰이크를 한다(Desktop 쪽 함수 그대로 — 키 · 토큰만 고른다).
async fn try_resume(
    fx: &Resumable,
    key: &Arc<DeviceKey>,
    token: crate::handshake::ResumeToken,
) -> Result<(), DialError> {
    let stream = fx.transport.open().await.unwrap();
    let mut framed = Framed::handshake(stream);
    let me = DesktopIdentity {
        key: Arc::clone(key),
        versions: versions(),
    };
    handshake(
        &mut framed,
        &me,
        &Expect::Machine(fx.relay_key.id()),
        Some(Resume { token, received: 0 }),
    )
    .await
    .map(|_| ())
}

/// 완료 기준 ⑧: 한 번 쓴 토큰과, 다른 기기 키로 내민 토큰(그 기기의 증명이 아닌 토큰)은 거절한다 — 링크는 그대로다. 토큰은
/// 이어 붙을 때마다 바뀐다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_used_token_or_one_without_its_device_proof_is_refused() {
    let fx = resumable(quick());
    fx.remote.attach().await.unwrap();
    let link = fx.remote.link().unwrap();
    let first = link.resume_token().expect("the relay gave no resume token");

    // 다른 기기(페어링된 owner)가 이 링크의 토큰을 내민다 — 그 키의 링크가 아니다.
    let other = Arc::new(DeviceKey::generate().unwrap());
    fx.pairings.upsert(Pairing {
        key: other.public(),
        role: Role::Owner,
        name: "thief".into(),
    });
    assert_eq!(
        try_resume(&fx, &other, first.clone()).await,
        Err(DialError::Rejected(RejectReason::Resume))
    );
    assert!(!link.is_suspended(), "a refused resume suspended the link");

    // 끊고 이어 붙으면 토큰이 바뀐다.
    let state = fx.remote.state();
    fx.transport.cut().await;
    wait_state(state, |s| *s == LinkState::Reconnecting).await;
    wait_state(fx.remote.state(), |s| {
        matches!(s, LinkState::Connected { .. })
    })
    .await;
    let second = link.resume_token().unwrap();
    assert_ne!(first, second, "the token did not change after its use");

    // 쓴 토큰은 같은 기기 키여도 거절.
    assert_eq!(
        try_resume(&fx, &fx.desktop_key, first).await,
        Err(DialError::Rejected(RejectReason::Resume))
    );
    // 링크는 그대로 돈다.
    let mut client = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    assert_eq!(
        client
            .call(1, "test.echo", json!({"text": "still"}))
            .await
            .unwrap()["text"],
        "still"
    );
}

/// 기기 해제 · role 변경은 멈춘 링크도 닫는다(M7-02 리뷰 권고 4 — 이어 붙기로 옛 role 의 채널을 되살리지 못한다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_suspended_link_of_an_unpaired_device_does_not_resume() {
    let fx = resumable(quick());
    fx.remote.attach().await.unwrap();
    let mut client = Client::open(&*fx.remote, Caller::new(Role::Owner)).await;
    ticks(&mut client, 1_000_000, 5).await;
    client.next().await.expect("the stream did not start");
    fx.transport.down.store(true, Ordering::SeqCst);
    fx.transport.cut().await;
    wait_state(fx.remote.state(), |s| *s == LinkState::Reconnecting).await;
    fx.pairings.upsert(Pairing {
        key: fx.desktop_key.public(),
        role: Role::Readonly,
        name: "mac".into(),
    });
    eventually("the relay kept the link of a changed role", || {
        fx.relay_host.drops() == 1
    })
    .await;
    fx.transport.down.store(false, Ordering::SeqCst);
    wait_state(fx.remote.state(), |s| {
        matches!(s, LinkState::Disconnected { .. })
    })
    .await;
    assert_eq!(fx.relay_host.connects(), 1);
}
