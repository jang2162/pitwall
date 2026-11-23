//! 포트 포워딩 채널(18-m7-plan M7-22 — 완료 기준 ② 와 링크 예산 · 방향). 「원격 머신」 의 TCP 서버는 이 시험 프로세스의 루프백
//! 포트다(Relay 는 같은 프로세스에서 돈다). 실제 SSH · 원격 머신은 `relay-live` 의 것이다.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::forward::ForwardTo;

/// 이쪽(Desktop) 루프백 소켓 한 쌍 — 사람의 브라우저 쪽(`client`)과 포워딩이 받은 쪽(`accepted`).
async fn local_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (accepted, _) = listener.accept().await.unwrap();
    (client, accepted)
}

/// 「원격」 서버: 받은 연결마다 `serve` 를 돌린다. 받은 연결 수를 센다.
async fn remote_server<F, Fut>(serve: F) -> (u16, Arc<AtomicUsize>)
where
    F: Fn(TcpStream) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(serve(socket));
        }
    });
    (port, accepted)
}

/// 다 읽고(쓰기를 닫을 때까지) 그대로 돌려주고 닫는다.
async fn echo_all(mut socket: TcpStream) {
    let mut all = Vec::new();
    socket.read_to_end(&mut all).await.unwrap();
    socket.write_all(&all).await.unwrap();
}

/// 바이트가 두 방향으로 지나고, 요청을 보낸 뒤 쓰기를 닫는(반쪽 닫기) 클라이언트도 답을 끝까지 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forward_carries_bytes_both_ways_and_keeps_a_half_close() {
    let fx = fixture(Setup::default());
    fx.attach().await;
    let (port, accepted) = remote_server(echo_all).await;
    let (mut client, socket) = local_pair().await;
    let remote = Arc::clone(&fx.remote);
    let carried = tokio::spawn(async move {
        remote
            .forward(ForwardTo::new("127.0.0.1", port), socket)
            .await
    });
    client.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
    client.shutdown().await.unwrap();
    let mut got = Vec::new();
    tokio::time::timeout(WAIT, client.read_to_end(&mut got))
        .await
        .expect("the answer did not arrive")
        .unwrap();
    assert_eq!(got, b"GET / HTTP/1.0\r\n\r\n");
    tokio::time::timeout(WAIT, carried)
        .await
        .expect("the forward did not end")
        .unwrap()
        .unwrap();
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    eventually("the forward channel closed", || {
        fx.remote.link().unwrap().channel_count() == 0
    })
    .await;
}

/// 작은 링크 예산(창 64 KiB · 채널 보내기 버퍼 128 KiB · 링크 보내기 예산 256 KiB)에서 8 MiB 응답이 **채널을 닫지 않고** 다 오고,
/// 그 사람이 한동안 읽지 않아도(브라우저가 멈춤) 같은 링크의 RPC 채널은 돈다. 포워딩이 TCP 읽기를 멈추지 않고 채널에 쌓으면
/// (`send_msg_paced` 대신 `send_msg`) 보내기 버퍼가 차서 채널이 닫히고 이 시험이 실패한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_response_flows_within_the_link_budgets_without_closing_the_channel() {
    const TOTAL: usize = 8 * 1024 * 1024;
    let limits = LinkLimits {
        window: 64 * 1024,
        send_buffer: 128 * 1024,
        send_budget: 256 * 1024,
        receive_budget: 64 * 1024 * 1024,
        ..LinkLimits::default()
    };
    let fx = fixture(Setup {
        limits,
        ..Setup::default()
    });
    fx.attach().await;
    let (port, _) = remote_server(|mut socket: TcpStream| async move {
        let mut request = [0u8; 4];
        socket.read_exact(&mut request).await.unwrap();
        let body: Vec<u8> = (0..TOTAL).map(|i| (i % 251) as u8).collect();
        let _ = socket.write_all(&body).await;
    })
    .await;
    let (mut client, socket) = local_pair().await;
    let remote = Arc::clone(&fx.remote);
    tokio::spawn(async move {
        remote
            .forward(ForwardTo::new("127.0.0.1", port), socket)
            .await
    });
    client.write_all(b"GET ").await.unwrap();
    // 사람 쪽이 한동안 읽지 않는다 — 그동안 다른 채널의 RPC 가 돈다.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut rpc = Client::open(&*fx.remote, fx.caller(Role::Owner, ConnectionScope::Host)).await;
    assert_eq!(
        rpc.call(1, "test.echo", json!({"text": "still here"}))
            .await
            .unwrap()["text"],
        "still here"
    );
    let mut got = Vec::with_capacity(TOTAL);
    let mut buf = vec![0u8; 64 * 1024];
    while got.len() < TOTAL {
        let n = tokio::time::timeout(WAIT, client.read(&mut buf))
            .await
            .expect("the response stalled")
            .unwrap();
        assert!(n > 0, "the forward closed after {} bytes", got.len());
        got.extend_from_slice(&buf[..n]);
    }
    assert!(
        got.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8),
        "the bytes arrived out of order"
    );
}

/// owner 가 아닌 기기의 `forward` 는 Relay 가 거절하고 TCP 를 열지 않는다(메인 판정 M7-P15 — 완료 기준 ②). Desktop 을 손으로
/// 짜서(Desktop 의 이른 거절을 건너뛰고) 보낸다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_readonly_link_cannot_open_a_forward() {
    let fx = fixture(Setup {
        paired: Some(Role::Readonly),
        ..Setup::default()
    });
    let (port, accepted) = remote_server(echo_all).await;
    let mut framed = raw_desktop(&fx).await;
    framed
        .write
        .write(&LinkFrame::new(
            1,
            Kind::Forward,
            json!(ForwardTo::new("127.0.0.1", port)),
        ))
        .await
        .unwrap();
    let answer = loop {
        let frame = raw_read(&mut framed).await.expect("the link closed");
        if frame.channel == 1 {
            break frame;
        }
    };
    assert_eq!(answer.kind, Kind::Refused, "{answer:?}");
    assert!(
        answer.body["reason"].as_str().unwrap().contains("owner"),
        "{answer:?}"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "the relay opened a TCP connection"
    );

    // Desktop 쪽도 owner 가 아닌 링크로는 채널을 열지 않는다.
    let fx = fixture(Setup {
        paired: Some(Role::Readonly),
        ..Setup::default()
    });
    fx.attach().await;
    let (_client, socket) = local_pair().await;
    let refused = fx
        .remote
        .forward(ForwardTo::new("127.0.0.1", port), socket)
        .await
        .unwrap_err();
    assert!(matches!(refused, HostError::Refused(_)), "{refused:?}");
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

/// 포워딩은 Desktop → Relay 한 방향이다. Relay(털린 원격의 같은 uid 프로세스가 Relay 자리에 섰다고 본다)가 Desktop 쪽으로
/// `forward` 를 보내도 Desktop 은 이 컴퓨터의 루프백 서비스에 TCP 를 열지 않고 링크를 끊는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_cannot_open_a_forward_toward_the_desktop() {
    let relay_key = DeviceKey::generate().unwrap();
    let (transport, acceptor) = transport::memory(64 * 1024);
    let remote = Arc::new(desktop_for(transport, &relay_key));
    let (port, accepted) = remote_server(echo_all).await;
    let attach = {
        let remote = Arc::clone(&remote);
        tokio::spawn(async move { remote.attach().await })
    };
    let mut framed = raw_relay(&acceptor, &relay_key, Role::Owner).await;
    attach.await.unwrap().expect("attach");
    framed
        .write
        .write(&LinkFrame::new(
            1,
            Kind::Forward,
            json!(ForwardTo::new("127.0.0.1", port)),
        ))
        .await
        .unwrap();
    let state = wait_state(remote.state(), |s| {
        matches!(s, LinkState::Disconnected { .. })
    })
    .await;
    assert!(
        matches!(
            &state,
            LinkState::Disconnected {
                reason: CloseReason::Protocol(_)
            }
        ),
        "{state:?}"
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "the desktop opened a TCP connection"
    );
}

/// 원격에서 아무도 듣지 않는 포트는 그 까닭으로 거절되고, 받은 로컬 연결은 닫힌다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forward_to_a_closed_remote_port_is_refused() {
    let fx = fixture(Setup::default());
    fx.attach().await;
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let (mut client, socket) = local_pair().await;
    let refused = fx
        .remote
        .forward(ForwardTo::new("127.0.0.1", port), socket)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, HostError::Refused(why) if why.contains(&port.to_string())),
        "{refused:?}"
    );
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());
    assert_eq!(fx.remote.link().unwrap().channel_count(), 0);
}

/// Relay 가 청한 것과 다른 대상(원격의 다른 호스트 · 포트)에 이었다고 답하면 Desktop 은 그 채널을 받지 않는다 — 정직한 Relay 의 버그 ·
/// 판 어긋남을 잡는 확인이다(털린 Relay 는 그대로 되돌려 말할 수 있어 막지 못한다 — 리뷰 F2)(메인 판정
/// 2026-11-24). 답의 대상을 보지 않으면 이 시험이 실패한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desktop_refuses_a_relay_that_answers_another_target() {
    let relay_key = DeviceKey::generate().unwrap();
    let (transport, acceptor) = transport::memory(64 * 1024);
    let remote = Arc::new(desktop_for(transport, &relay_key));
    let attach = {
        let remote = Arc::clone(&remote);
        tokio::spawn(async move { remote.attach().await })
    };
    let mut framed = raw_relay(&acceptor, &relay_key, Role::Owner).await;
    attach.await.unwrap().expect("attach");
    let (mut client, socket) = local_pair().await;
    let forwarding = {
        let remote = Arc::clone(&remote);
        tokio::spawn(async move {
            remote
                .forward(ForwardTo::new("localhost", 5173), socket)
                .await
        })
    };
    let asked = loop {
        let frame = raw_read(&mut framed).await.expect("the link closed");
        if frame.kind == Kind::Forward {
            break frame;
        }
    };
    assert_eq!(asked.body, json!({"host": "localhost", "port": 5173}));
    framed
        .write
        .write(&LinkFrame::new(
            asked.channel,
            Kind::Accepted,
            json!({"host": "10.0.0.5", "port": 22}),
        ))
        .await
        .unwrap();
    let refused = tokio::time::timeout(WAIT, forwarding)
        .await
        .expect("the forward did not end")
        .unwrap()
        .unwrap_err();
    assert!(matches!(refused, HostError::Refused(_)), "{refused:?}");
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());
    assert_eq!(remote.link().unwrap().channel_count(), 0);
}

/// Relay 도 원격 쪽 대상을 그 머신의 루프백으로만 받는다 — Desktop 의 판정을 건너뛴(손으로 짠) owner 링크가 원격의 다른 호스트를
/// 청해도 열지 않는다(메인 판정 2026-11-24).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_refuses_a_target_off_its_loopback() {
    let fx = fixture(Setup::default());
    let mut framed = raw_desktop(&fx).await;
    for (channel, host) in [(1, "db.internal"), (3, "10.0.0.5"), (5, "0.0.0.0")] {
        framed
            .write
            .write(&LinkFrame::new(
                channel,
                Kind::Forward,
                json!({"host": host, "port": 5432}),
            ))
            .await
            .unwrap();
        let answer = loop {
            let frame = raw_read(&mut framed).await.expect("the link closed");
            if frame.channel == channel {
                break frame;
            }
        };
        assert_eq!(answer.kind, Kind::Refused, "{host}: {answer:?}");
        assert!(
            answer.body["reason"].as_str().unwrap().contains("loopback"),
            "{host}: {answer:?}"
        );
    }
}
