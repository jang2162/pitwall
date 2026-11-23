//! 직접 연결 고르기(18-m7-plan M7-24 ④ · 시험 ②): 기억한 머신이 있으면 QUIC + Noise 를 먼저 해 보고, 안 되면 SSH 로 떨어진다(가짜
//! `ssh`). 처음 붙는 호스트는 직접 연결을 하지 않는다. 직접 연결 전용 호스트는 떨어지지 않는다. 받는 쪽은 루프백의 진짜 QUIC +
//! Noise Relay(`pw_link::serve_noise`)다.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicU32;

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
    _listener: Arc<QuicListener>,
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
        _listener: listener,
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
    let silent = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = silent.local_addr().unwrap().port();
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
