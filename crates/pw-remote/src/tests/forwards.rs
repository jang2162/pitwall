//! 포트 포워딩의 로컬 쪽(18-m7-plan M7-22 — 완료 기준 ① ④ 와 연결 상한). 가짜 링크의 포워더는 「원격」 을 이 컴퓨터의
//! `127.0.0.1:<원격 포트>` 로 곧바로 잇는다 — 링크 채널 쪽은 `pw-link` 의 시험(`tests::forward`)이 본다.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use pw_link::forward::ForwardSetting;

/// 지금 비어 있는 루프백 포트 하나.
async fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// 받은 것을 그대로 돌려주는 「원격」 서버.
async fn echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = socket.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

async fn forwards_until(
    remote: &RemoteHosts,
    pred: impl Fn(&[RemoteForward]) -> bool,
) -> Vec<RemoteForward> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let now = remote.forwards(Some("mini"));
            if pred(&now) {
                return now;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("forwards never matched: {:?}", remote.forwards(None)))
}

async fn round_trip(port: u16, text: &[u8]) -> Vec<u8> {
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(text).await.unwrap();
    let mut got = vec![0u8; text.len()];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut got))
        .await
        .expect("no answer")
        .unwrap();
    got
}

/// 설정의 포워딩은 붙을 때 열리고(루프백에서만), `local_host = "0.0.0.0"` 인 것은 열지 않고 까닭을 보이며, 쓰는 창이 모두 닫히면
/// 닫힌다(완료 기준 ① · ②). 루프백 판정을 걷으면 `0.0.0.0` 포워딩이 열려 이 시험이 실패한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_forwards_open_on_loopback_when_connected_and_close_with_the_last_use() {
    let s = setup();
    let echo = echo_server().await;
    let (good, open_to_all) = (free_port().await, free_port().await);
    *lock(&s.fake.port_forwards) = vec![
        ForwardSetting {
            local_port: good,
            remote_port: echo,
            remote_host: None,
            local_host: None,
        },
        ForwardSetting {
            local_port: open_to_all,
            remote_port: echo,
            remote_host: None,
            local_host: Some("0.0.0.0".into()),
        },
    ];
    assert!(
        s.remote.forwards(None).is_empty(),
        "opened before connecting"
    );
    let use_ = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    let list = forwards_until(&s.remote, |f| f.len() == 2).await;
    assert_eq!(list[0].origin, RemoteForwardOrigin::Settings);
    assert_eq!(list[0].local_host, "127.0.0.1");
    assert_eq!(list[0].local_port, good);
    assert_eq!(list[0].remote_host, "localhost");
    assert_eq!(list[0].state, RemoteForwardState::Listening);
    assert!(
        matches!(
            &list[1].state,
            RemoteForwardState::Failed { error } if error.kind == RemoteForwardFailureKind::NotLoopback
        ),
        "{:?}",
        list[1]
    );
    assert_eq!(list[1].local_host, "0.0.0.0");
    // 그 포트는 어느 주소에서도 듣지 않는다.
    TcpListener::bind(("0.0.0.0", open_to_all))
        .await
        .expect("the 0.0.0.0 forward was opened");
    assert!(
        s.remote
            .log("mini")
            .iter()
            .any(|l| l.text.contains("not opened") && l.text.contains("loopback")),
        "no warning in the connection log"
    );

    assert_eq!(round_trip(good, b"ping").await, b"ping");
    assert_eq!(s.fake.carried.load(Ordering::SeqCst), 1);

    drop(use_);
    assert!(s.remote.forwards(None).is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    TcpListener::bind(("127.0.0.1", good))
        .await
        .expect("the forward still listens after the link closed");
}

/// 실행 탭의 포워딩: 로컬 포트가 이미 쓰이면 그 번호를 실은 오류로 끝나고(완료 기준 ④), 다른 포트(0 — OS 가 고름)로 다시 열면
/// 루프백에서 듣는다. 쥔 것을 버리면 닫힌다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_forward_reports_a_local_port_in_use_and_opens_on_another() {
    let s = setup();
    let echo = echo_server().await;
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let busy = taken.local_addr().unwrap().port();
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;

    let err = s
        .remote
        .open_forward("mini", busy, None)
        .await
        .err()
        .unwrap();
    assert_eq!(err, ForwardOpenError::PortInUse(busy));
    assert!(s.remote.forwards(None).is_empty());

    let forward = s.remote.open_forward("mini", echo, Some(0)).await.unwrap();
    let info = forward.watch.borrow().clone().unwrap();
    assert_eq!(info.origin, RemoteForwardOrigin::Run);
    assert_eq!(info.remote_host, "localhost");
    assert_eq!(info.remote_port, echo);
    assert_eq!(info.local_host, "127.0.0.1");
    assert_ne!(info.local_port, 0);
    assert_eq!(round_trip(info.local_port, b"hello").await, b"hello");

    // 상태바의 [끄기]: 스트림이 끝을 본다.
    let mut watch = forward.watch.clone();
    assert!(s.remote.close_forward(info.id));
    watch.changed().await.unwrap();
    assert!(watch.borrow().is_none());
    assert!(s.remote.forwards(None).is_empty());
    drop(forward);
}

/// 한 호스트에 동시에 잇는 연결은 [`MAX_CONNECTIONS`] 까지다 — 이 컴퓨터의 다른 프로세스가 루프백 포트로 연결을 몰아 열어 링크의
/// 채널 상한을 채우지 못하게. 넘는 연결은 곧 닫힌다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwarded_connections_are_capped_per_host() {
    let s = setup();
    // 받기만 하고 닫지 않는 「원격」 서버.
    let hold = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let remote_port = hold.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Ok((socket, _)) = hold.accept().await {
            kept.push(socket);
        }
    });
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    let forward = s
        .remote
        .open_forward("mini", remote_port, Some(0))
        .await
        .unwrap();
    let port = forward.watch.borrow().clone().unwrap().local_port;
    let mut clients = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        clients.push(TcpStream::connect(("127.0.0.1", port)).await.unwrap());
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while s.fake.carried.load(Ordering::SeqCst) < MAX_CONNECTIONS {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the connections under the cap were not carried");
    let mut over = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(10), over.read(&mut buf))
        .await
        .expect("the connection over the cap was kept open")
        .unwrap_or(0);
    assert_eq!(n, 0);
    assert_eq!(s.fake.carried.load(Ordering::SeqCst), MAX_CONNECTIONS);
}

/// 실행 탭 포워딩 하나와 설정 포워딩 하나를 연 채로 붙인다.
async fn forwarding(s: &Setup) -> (Use, ForwardUse) {
    let echo = echo_server().await;
    *lock(&s.fake.port_forwards) = vec![ForwardSetting {
        local_port: free_port().await,
        remote_port: echo,
        remote_host: None,
        local_host: None,
    }];
    let use_ = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    let run = s.remote.open_forward("mini", echo, Some(0)).await.unwrap();
    forwards_until(&s.remote, |f| f.len() == 2).await;
    (use_, run)
}

/// 링크가 이어 붙는 동안(`resuming`)은 포워딩을 두고, 링크가 끊기면(다시 붙기) 그 호스트의 포워딩을 모두 닫는다 — 실행 탭의
/// 스트림도 끝난다. 다시 붙으면 설정의 것만 다시 연다(메인 판정 2026-11-24).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwards_survive_resuming_and_close_when_the_link_drops() {
    let s = setup();
    let (_use, run) = forwarding(&s).await;
    s.fake.last_link().resuming.send_replace(true);
    until(&s.remote, |st| matches!(st, RemoteStatus::Resuming { .. })).await;
    assert_eq!(s.remote.forwards(None).len(), 2, "closed while resuming");
    s.fake.last_link().resuming.send_replace(false);
    until(&s.remote, connected).await;

    let mut watch = run.watch.clone();
    s.fake.last_link().drop_link();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::Reconnecting { .. })
    })
    .await;
    assert!(
        s.remote.forwards(None).is_empty(),
        "a forward outlived the link"
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while watch.borrow_and_update().is_some() {
            watch.changed().await.unwrap();
        }
    })
    .await
    .expect("the run forward stream did not end");
    until(&s.remote, connected).await;
    let again = forwards_until(&s.remote, |f| !f.is_empty()).await;
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].origin, RemoteForwardOrigin::Settings);
}

/// 다른 머신이 답하면(`machineChanged`) 그 호스트의 포워딩을 모두 닫는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwards_close_when_the_machine_changes() {
    let s = setup();
    let (_use, _run) = forwarding(&s).await;
    s.fake.push(changed(MINI, "SHA256:other"));
    s.fake.last_link().drop_link();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::MachineChanged { .. })
    })
    .await;
    assert!(s.remote.forwards(None).is_empty());
}

/// 설정에서 호스트를 지우면(`config.changed`) 그 호스트의 포워딩을 모두 닫는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwards_close_when_the_host_is_removed_from_the_settings() {
    let s = setup();
    let (_use, _run) = forwarding(&s).await;
    s.fake.gone.store(true, Ordering::SeqCst);
    s.bus
        .publish(Scope::Host, pw_core::config::TOPIC_CHANGED, &"settings")
        .unwrap();
    forwards_until(&s.remote, <[RemoteForward]>::is_empty).await;
}

/// 포워딩 RPC 는 owner 만이다 — 같은 Host 범위라도 readonly · mobile 기기는 거절된다(범위 표는 `every_remote_method_takes_only_a_host_connection`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forward_methods_are_for_the_owner_only() {
    let s = setup();
    let registered = s.remote.clone();
    let host = LocalHost::builder("local")
        .methods(move |r| methods::register(r, &registered))
        .build();
    for role in [pw_rpc::Role::Readonly, pw_rpc::Role::Mobile] {
        for (method, params) in [
            ("remote.forwards.list", serde_json::json!({})),
            (
                "remote.forward.open",
                serde_json::json!({ "name": "mini", "remotePort": 5173 }),
            ),
            ("remote.forward.close", serde_json::json!({ "id": 1 })),
        ] {
            let mut conn = host
                .connect(pw_rpc::Caller {
                    role,
                    scope: pw_rpc::ConnectionScope::Host,
                    window: None,
                })
                .await
                .unwrap();
            conn.sink.send(
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
            );
            let frame = conn.source.recv().await.unwrap();
            assert_eq!(
                frame["error"]["code"],
                serde_json::json!(pw_rpc::code::PERMISSION_DENIED),
                "{method} {role:?}: {frame}"
            );
        }
    }
    assert!(s.remote.forwards(None).is_empty());
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 0);
}
