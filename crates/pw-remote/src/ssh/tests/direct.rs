//! 직접 연결 고르기(18-m7-plan M7-24 ④ · 시험 ②): 기억한 머신이 있으면 QUIC + Noise 를 먼저 해 보고, 안 되면 SSH 로 떨어진다(가짜
//! `ssh`). 처음 붙는 호스트는 직접 연결을 하지 않는다. 직접 연결 전용 호스트는 떨어지지 않는다. 받는 쪽은 루프백의 진짜 QUIC +
//! Noise Relay(`pw_link::serve_noise`)다.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicU32;

use pw_link::LinkStream;
use pw_link::pair::{CodeBook, Pending};
use pw_link::quic::QuicListener;
use pw_link::{Pairing, PairingCode, PairingTable, RelayIdentity, RelayParts};
use pw_rpc::Role;

use super::*;

/// 시험용 대기 코드 하나.
struct Book {
    code: Mutex<Option<PairingCode>>,
    attempts: AtomicU32,
    pairings: PairingTable,
}

impl CodeBook for Book {
    fn attempt(&self) -> io::Result<Option<Pending>> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Ok(self.code.lock().unwrap().clone().map(|code| Pending {
            code,
            role: Role::Owner,
            name: None,
        }))
    }

    fn complete(
        &self,
        _code: &PairingCode,
        key: &pw_link::PublicKey,
        role: Role,
        name: &str,
    ) -> io::Result<()> {
        *self.code.lock().unwrap() = None;
        self.pairings.upsert(Pairing {
            key: *key,
            role,
            name: name.to_owned(),
        });
        Ok(())
    }
}

struct QuicRelay {
    key: Arc<DeviceKey>,
    port: u16,
    pairings: PairingTable,
    listener: Arc<QuicListener>,
}

/// 루프백의 QUIC + Noise Relay. `paired` 면 이 시험의 Desktop 키(`Fixed`)를 owner 로 넣어 둔다.
fn quic_relay(paired: bool, code: Option<&str>) -> QuicRelay {
    let key = Arc::new(DeviceKey::generate().unwrap());
    let desktop = DeviceKey::from_seed(&[7; 32]).public();
    let pairings = PairingTable::new(if paired {
        vec![Pairing {
            key: desktop,
            role: Role::Owner,
            name: "mac".into(),
        }]
    } else {
        Vec::new()
    });
    let book = Arc::new(Book {
        code: Mutex::new(code.map(|c| PairingCode::parse(c).unwrap())),
        attempts: AtomicU32::new(0),
        pairings: pairings.clone(),
    });
    let parts = RelayParts::new(
        Arc::new(RelayIdentity {
            key: Arc::clone(&key),
            versions: Versions::new(VERSION, "dev"),
        }),
        pairings.clone(),
        Arc::new(pw_core::LocalHost::new("relay")) as Arc<dyn pw_core::Host>,
        LinkLimits::default(),
    );
    let listener = Arc::new(QuicListener::bind((Ipv4Addr::LOCALHOST, 0).into()).unwrap());
    let port = listener.local_addr().unwrap().port();
    {
        let listener = Arc::clone(&listener);
        tokio::spawn(async move {
            while let Some(incoming) = listener.next().await {
                let (parts, book) = (parts.clone(), Arc::clone(&book));
                tokio::spawn(async move {
                    if let Ok(stream) = incoming.accept().await {
                        let _ = pw_link::serve_noise(stream, &parts, &*book, |_| {}).await;
                    }
                });
            }
        });
    }
    QuicRelay {
        key,
        port,
        pairings,
        listener,
    }
}

fn entry(direct: Option<u16>, ssh: bool) -> HostEntry {
    HostEntry {
        name: "mini".into(),
        host: "127.0.0.1".into(),
        user: None,
        port: None,
        source: RemoteHostSource::Settings,
        ssh_args: Vec::new(),
        direct,
        ssh,
    }
}

async fn connect_entry(
    backend: &SshBackend,
    entry: &HostEntry,
    expect: Option<&str>,
) -> (Result<Link, ConnectError>, Vec<Progress>) {
    let seen = Mutex::new(Vec::new());
    let progress = |p: Progress| seen.lock().unwrap().push(p);
    let result = backend.connect(entry, expect, &progress).await;
    (result, seen.into_inner().unwrap())
}

fn vias(seen: &[Progress]) -> Vec<RemoteVia> {
    seen.iter()
        .filter_map(|p| match p {
            Progress::Via(v) => Some(*v),
            _ => None,
        })
        .collect()
}

/// 같은 LAN · tailnet: 기억한 머신이면 SSH 없이 QUIC + Noise 로 붙는다 — ssh 단계가 없고 `via` 가 `direct` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remembered_machine_links_directly_without_ssh() {
    let s = setup();
    let backend = s.backend(&[], None);
    backend.remember_direct("mini");
    let relay = quic_relay(true, None);
    let machine = relay.key.id();
    let (result, seen) = connect_entry(
        &backend,
        &entry(Some(relay.port), true),
        Some(machine.as_str()),
    )
    .await;
    let link = result.expect("the direct link stands");
    assert_eq!(link.machine, machine.as_str());
    assert_eq!(stages(&seen), [RemoteStage::Direct]);
    assert_eq!(vias(&seen), [RemoteVia::Direct]);
    (link.close)();
}

/// 시험 ②: QUIC 가 막히면(답 없는 포트) SSH 로 떨어진다 — 직접 연결을 먼저 해 보고 그 뒤 ssh 단계가 이어진다. 연결 로그에 떨어진
/// 까닭이 남는다(여기 가짜 Relay 는 핸드셰이크에서 실패하므로 `via` 까지 가지 않는다 — 단계만 본다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocked_direct_connection_falls_back_to_ssh() {
    let s = setup();
    let backend = s.backend(&[], None);
    backend.remember_direct("mini");
    let silent = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = silent.local_addr().unwrap().port();
    let started = std::time::Instant::now();
    let seen_ssh_at = Arc::new(Mutex::new(None));
    let at = Arc::clone(&seen_ssh_at);
    let progress = move |p: Progress| {
        if matches!(p, Progress::Stage(RemoteStage::Ssh)) {
            at.lock().unwrap().get_or_insert(started.elapsed());
        }
    };
    let _ = backend
        .connect(
            &entry(Some(port), true),
            Some("SHA256:fakemachine"),
            &progress,
        )
        .await;
    // 짧은 시한(1.2초) 뒤 SSH 로 간다 — 주소 하나마다 3초 · 전체 6초를 기다리지 않는다.
    let waited = seen_ssh_at.lock().unwrap().expect("no ssh stage");
    assert!(
        waited < crate::ssh::DIRECT_FIRST_TIMEOUT + Duration::from_millis(600),
        "waited {waited:?} before ssh"
    );
    // 직접 연결이 실패했으니 다음 붙기는 SSH 로 바로(QUIC 를 보내지 않는다).
    let (_, again) = connect_entry(
        &backend,
        &entry(Some(port), true),
        Some("SHA256:fakemachine"),
    )
    .await;
    assert!(!stages(&again).contains(&RemoteStage::Direct));
    backend.remember_direct("mini");
    let (result, seen) = connect_entry(
        &backend,
        &entry(Some(port), true),
        Some("SHA256:fakemachine"),
    )
    .await;
    let stages = stages(&seen);
    assert_eq!(stages.first(), Some(&RemoteStage::Direct));
    assert!(stages.contains(&RemoteStage::Ssh), "{stages:?}");
    assert!(!vias(&seen).contains(&RemoteVia::Direct));
    assert!(
        seen.iter()
            .any(|p| matches!(p, Progress::Note(n) if n.contains("connecting over ssh"))),
        "{seen:?}"
    );
    assert!(result.is_err());
}

/// 처음 붙는 호스트(기억한 머신 없음)는 직접 연결을 하지 않는다 — QUIC 위에서 처음 보는 키를 믿는 길이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_contact_never_tries_a_direct_connection() {
    let s = setup();
    let backend = s.backend(&[], None);
    let relay = quic_relay(true, None);
    let (_, seen) = connect_entry(&backend, &entry(Some(relay.port), true), None).await;
    assert_eq!(stages(&seen).first(), Some(&RemoteStage::Ssh));
    assert!(!stages(&seen).contains(&RemoteStage::Direct));
}

/// 직접 연결이 다른 머신(다른 키의 Relay — 가짜 광고 · 주소 가로채기)에 닿으면 붙지 않고, 연결 로그에 경고를 남기고 SSH 로 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_machine_at_the_direct_address_is_a_warning_not_a_link() {
    let s = setup();
    let backend = s.backend(&[], None);
    backend.remember_direct("mini");
    let other = quic_relay(true, None);
    let remembered = DeviceKey::generate().unwrap().id();
    let (_, seen) = connect_entry(
        &backend,
        &entry(Some(other.port), true),
        Some(remembered.as_str()),
    )
    .await;
    assert!(
        seen.iter()
            .any(|p| matches!(p, Progress::Warn(w) if w.contains("MachineMismatch"))),
        "{seen:?}"
    );
    assert!(!vias(&seen).contains(&RemoteVia::Direct));
    assert!(stages(&seen).contains(&RemoteStage::Ssh));
}

/// 직접 연결 전용 호스트(`ssh = false`)는 SSH 로 떨어지지 않고 직접 연결 단계의 실패로 끝난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_direct_only_host_does_not_fall_back() {
    let s = setup();
    let backend = s.backend(&[], None);
    let silent = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = silent.local_addr().unwrap().port();
    let (result, seen) = connect_entry(&backend, &entry(Some(port), false), Some("SHA256:x")).await;
    let e = result.err().unwrap();
    assert_eq!(e.stage, RemoteStage::Direct);
    assert_eq!(stages(&seen), [RemoteStage::Direct]);
    let (result, seen) = connect_entry(&backend, &entry(Some(port), false), None).await;
    assert_eq!(result.err().unwrap().stage, RemoteStage::Direct);
    assert!(stages(&seen).is_empty());
}

/// 코드 입력 화면의 길: 주소 · 코드로 페어링하면 그 Relay 의 머신 id 와 role 을 받고, 그 키가 페어링 표에 든다. 틀린 코드면 아무것도
/// 페어링되지 않는다. 그 뒤 직접 연결로 붙는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_code_pairs_this_desktop_over_a_direct_connection() {
    let s = setup();
    let backend = s.backend(&[], None);
    let relay = quic_relay(false, Some("246810"));
    let wrong = backend.pair_direct("127.0.0.1", relay.port, "135791").await;
    assert!(wrong.is_err(), "{wrong:?}");
    assert!(relay.pairings.snapshot().is_empty());
    let relay = quic_relay(false, Some("246810"));
    let (machine, role) = backend
        .pair_direct("127.0.0.1", relay.port, "246810")
        .await
        .unwrap();
    assert_eq!(machine, relay.key.id().as_str());
    assert_eq!(role, Role::Owner);
    assert_eq!(relay.pairings.snapshot().len(), 1);
    let (result, _) =
        connect_entry(&backend, &entry(Some(relay.port), false), Some(&machine)).await;
    (result.unwrap().close)();
}

/// 리뷰 F3: 직접 연결 주소를 모르는 SSH 호스트(기억한 머신은 있다 — SSH 로 설치한 호스트의 하루 첫 연결)는 QUIC 를 한 번도 보내지
/// 않고 SSH 로 바로 붙는다. 그 주소에 Relay 가 받고 있어도 그렇다(받는 쪽이 본 Retry 0).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ssh_host_without_a_known_direct_address_sends_no_quic() {
    let s = setup();
    let backend = s.backend(&[], None);
    let relay = quic_relay(true, None);
    let machine = relay.key.id();
    let (_, seen) = connect_entry(
        &backend,
        &entry(Some(relay.port), true),
        Some(machine.as_str()),
    )
    .await;
    assert_eq!(stages(&seen).first(), Some(&RemoteStage::Ssh));
    assert!(!stages(&seen).contains(&RemoteStage::Direct));
    assert_eq!(
        relay.listener.retries(),
        0,
        "a quic packet reached the relay"
    );
}

/// 이 시험 프로세스 안의 진짜 Relay 하나: SSH `proxy` 자리(TCP — 가짜 원격의 스크립트가 bash `/dev/tcp` 로 잇는다)와 직접 연결
/// 자리(QUIC + Noise)가 같은 기기 키 · 페어링 표를 쓴다.
struct BothRelay {
    key: Arc<DeviceKey>,
    tcp_port: u16,
    quic_port: u16,
    _listener: Arc<QuicListener>,
}

async fn both_relay() -> BothRelay {
    let key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(vec![Pairing {
        key: DeviceKey::from_seed(&[7; 32]).public(),
        role: Role::Owner,
        name: "mac".into(),
    }]);
    let parts = RelayParts::new(
        Arc::new(RelayIdentity {
            key: Arc::clone(&key),
            versions: Versions::new(VERSION, "dev"),
        }),
        pairings.clone(),
        Arc::new(pw_core::LocalHost::new("relay")) as Arc<dyn pw_core::Host>,
        LinkLimits::default(),
    );
    let tcp = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let tcp_port = tcp.local_addr().unwrap().port();
    {
        let parts = parts.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = tcp.accept().await {
                let parts = parts.clone();
                tokio::spawn(async move {
                    let _ = pw_link::serve(LinkStream::split(socket), &parts).await;
                });
            }
        });
    }
    let listener = Arc::new(QuicListener::bind((Ipv4Addr::LOCALHOST, 0).into()).unwrap());
    let quic_port = listener.local_addr().unwrap().port();
    {
        let (listener, book) = (
            Arc::clone(&listener),
            Arc::new(Book {
                code: Mutex::new(None),
                attempts: AtomicU32::new(0),
                pairings,
            }),
        );
        tokio::spawn(async move {
            while let Some(incoming) = listener.next().await {
                let (parts, book) = (parts.clone(), Arc::clone(&book));
                tokio::spawn(async move {
                    if let Ok(stream) = incoming.accept().await {
                        let _ = pw_link::serve_noise(stream, &parts, &*book, |_| {}).await;
                    }
                });
            }
        });
    }
    BothRelay {
        key,
        tcp_port,
        quic_port,
        _listener: listener,
    }
}

/// 가짜 원격의 Relay 스크립트: `pair` 는 그 Relay 의 머신 id 를, `proxy` 는 bash `/dev/tcp` 로 시험 Relay 의 TCP 에 잇는다.
fn bridging_script(relay: &BothRelay) -> String {
    format!(
        "#!/bin/bash\ncase \"$1\" in\n  --verify) exit 1 ;;\n  pair) echo '{{\"machine\":\"{}\",\"added\":false}}' ;;\n  \
         proxy) exec 3<>/dev/tcp/127.0.0.1/{}; cat <&3 & cat >&3; wait ;;\n  *) exit 2 ;;\nesac\n",
        relay.key.id().as_str(),
        relay.tcp_port
    )
}

/// 리뷰 F3: 직접 연결 주소를 알지만 그 포트가 답하지 않으면(데몬이 직접 연결을 받지 않음) 짧은 시한 뒤 SSH 로 떨어지고, 붙은 링크는
/// `via = ssh` 로 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_silent_known_address_falls_back_to_ssh_and_says_ssh() {
    let relay = both_relay().await;
    let s = setup_with(&bridging_script(&relay));
    let backend = s.backend(&[], None);
    backend.remember_direct("mini");
    let silent = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = silent.local_addr().unwrap().port();
    let machine = relay.key.id();
    let (result, seen) =
        connect_entry(&backend, &entry(Some(port), true), Some(machine.as_str())).await;
    let link = result.expect("the ssh link stands");
    assert_eq!(stages(&seen).first(), Some(&RemoteStage::Direct));
    assert!(stages(&seen).contains(&RemoteStage::Ssh));
    assert_eq!(vias(&seen), [RemoteVia::Ssh]);
    (link.close)();
}

/// SSH 링크가 선 뒤 그 호스트의 직접 연결 포트가 답하면 주소를 배워, 다음 붙기는 SSH 없이 직접 연결로 붙는다(`via = direct`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ssh_link_learns_the_direct_address_for_the_next_time() {
    let relay = both_relay().await;
    let s = setup_with(&bridging_script(&relay));
    let backend = s.backend(&[], None);
    let machine = relay.key.id();
    let host = entry(Some(relay.quic_port), true);
    let (first, seen) = connect_entry(&backend, &host, Some(machine.as_str())).await;
    let first = first.expect("the ssh link stands");
    assert!(!stages(&seen).contains(&RemoteStage::Direct));
    assert_eq!(vias(&seen), [RemoteVia::Ssh]);
    // 배우기는 링크 뒤에 따로 돈다.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !backend.knows_direct("mini") {
        assert!(
            std::time::Instant::now() < deadline,
            "the direct address was not learned"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (first.close)();
    let (second, seen) = connect_entry(&backend, &host, Some(machine.as_str())).await;
    let second = second.expect("the direct link stands");
    assert_eq!(stages(&seen), [RemoteStage::Direct]);
    assert_eq!(vias(&seen), [RemoteVia::Direct]);
    (second.close)();
}
