//! QUIC 직접 연결(18-m7-plan M7-24 ①): Noise 를 거친 링크만 받는다 · 주소 검증 · 0-RTT 없음 · 인증서는 신뢰 근거가 아니다.
//! 받는 쪽은 시험용 루프백 주소에 묶는다(듣는 인터페이스 고르기는 `pw_relay::listen` 의 시험이 본다).

use std::net::{Ipv4Addr, SocketAddr};

use tokio::io::AsyncWriteExt;

use super::noise::{Book, identity, paired};
use super::*;
use crate::quic::{QuicListener, QuicTransport};
use crate::relay::Served;
use crate::transport::Transport;
use crate::{SecuredTransport, serve_noise};

fn loopback() -> SocketAddr {
    (Ipv4Addr::LOCALHOST, 0).into()
}

/// QUIC 로 받는 Relay 하나 — 받은 스트림은 `serve_noise` 로만 잇는다(데몬과 같다).
struct QuicRelay {
    host: Arc<RelayHost>,
    key: Arc<DeviceKey>,
    listener: Arc<QuicListener>,
    results: Arc<Mutex<Vec<Result<Served, ServeError>>>>,
}

fn quic_relay(paired: Vec<Pairing>) -> QuicRelay {
    let host = RelayHost::new("relay");
    let key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(paired);
    let book = Book::new("123456", Role::Mobile, pairings.clone());
    let parts = RelayParts::new(
        Arc::new(RelayIdentity {
            key: Arc::clone(&key),
            versions: versions(),
        }),
        pairings,
        Arc::clone(&host) as Arc<dyn Host>,
        // 받는 쪽의 시한을 줄인다(데몬은 전체 5초 시한을 따로 건다 — `pw_relay::direct`).
        LinkLimits {
            dead_after: Duration::from_secs(2),
            ..LinkLimits::default()
        },
    );
    let listener = Arc::new(QuicListener::bind(loopback()).unwrap());
    let results = Arc::new(Mutex::new(Vec::new()));
    {
        let (listener, results) = (Arc::clone(&listener), Arc::clone(&results));
        tokio::spawn(async move {
            while let Some(incoming) = listener.next().await {
                let (parts, results, book) =
                    (parts.clone(), Arc::clone(&results), Arc::clone(&book));
                tokio::spawn(async move {
                    let result = match incoming.accept().await {
                        Ok(stream) => serve_noise(stream, &parts, &*book, |_| {}).await,
                        Err(e) => Err(ServeError::Io(e.to_string())),
                    };
                    results.lock().unwrap().push(result);
                });
            }
        });
    }
    QuicRelay {
        host,
        key,
        listener,
        results,
    }
}

impl QuicRelay {
    fn transport(&self) -> QuicTransport {
        let addr = self.listener.local_addr().unwrap();
        QuicTransport::new(addr.ip().to_string(), addr.port())
    }

    fn results(&self) -> usize {
        self.results.lock().unwrap().len()
    }
}

/// 페어링된 기기는 QUIC + Noise 위에서 M7-03 링크를 열고 RPC 를 부른다(`SecuredTransport<QuicTransport>` — Desktop 의 길).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paired_device_links_over_quic_and_noise() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let relay = quic_relay(vec![paired(&desktop, Role::Owner)]);
    let transport = SecuredTransport::new(relay.transport(), Arc::clone(&desktop));
    assert!(transport.describe().starts_with("noise over quic "));
    let remote = RemoteHost::new(
        "mini",
        Arc::new(transport),
        Arc::new(identity(&desktop)),
        Expect::Machine(relay.key.id()),
        LinkLimits::default(),
    );
    remote.attach().await.unwrap();
    assert_eq!(remote.link().unwrap().role(), Role::Owner);
    let mut client = Client::open(
        &remote,
        Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project("p1".into()),
            window: Some("w".into()),
        },
    )
    .await;
    let big = client
        .call(1, "test.big", json!({"bytes": 300_000}))
        .await
        .unwrap();
    assert_eq!(big.as_str().map(str::len), Some(300_000));
    assert_eq!(
        relay.listener.retries(),
        1,
        "the address was validated once"
    );
}

/// QUIC 위에서 Noise 없이 링크 틀(`hello`)을 보내면 받는 쪽은 Noise 핸드셰이크로 읽다가 끊고, Host 에는 아무것도 닿지 않는다 — QUIC
/// 의 TLS 만으로 링크를 받는 길이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn link_frames_over_quic_without_noise_reach_nothing() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let relay = quic_relay(vec![paired(&desktop, Role::Owner)]);
    let stream = relay.transport().open().await.unwrap();
    let mut framed = Framed::handshake(stream);
    // 페어링된 기기의 진짜 hello — 그래도 Noise 밖이라 받지 않는다.
    let _ = tokio::time::timeout(
        Duration::from_secs(3),
        raw_hello(&mut framed, &desktop, versions()),
    )
    .await;
    eventually("the relay ends the plain connection", || {
        relay.results() == 1
    })
    .await;
    let result = relay.results.lock().unwrap().pop().unwrap();
    assert!(
        matches!(result, Err(ServeError::Io(_) | ServeError::Timeout)),
        "{result:?}"
    );
    assert_eq!(relay.host.connects(), 0);
}

/// 어떤 인증서든 받는 붙는 쪽이라도 상대 판정은 Noise 와 기억한 머신 키가 한다: 다른 Relay(가짜 광고 · 같은 주소를 가로챈 상대)에
/// 기억한 머신으로 붙으면 링크가 서지 않고, 코드 페어링도 코드를 모르는 Relay 에는 되지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_relay_at_the_address_is_refused_by_noise_and_the_remembered_key() {
    let desktop = Arc::new(DeviceKey::generate().unwrap());
    let real = DeviceKey::generate().unwrap();
    // 가짜 Relay 는 이 Desktop 을 페어링해 두었다(가장 나쁜 경우 — 받아 주는 상대).
    let other = quic_relay(vec![paired(&desktop, Role::Owner)]);
    let transport = SecuredTransport::new(other.transport(), Arc::clone(&desktop));
    let remote = RemoteHost::new(
        "mini",
        Arc::new(transport),
        Arc::new(identity(&desktop)),
        Expect::Machine(real.id()),
        LinkLimits::default(),
    );
    let refused = remote.attach().await.err();
    assert!(
        matches!(refused, Some(DialError::MachineMismatch { .. })),
        "{refused:?}"
    );
    assert_eq!(other.host.connects(), 0);

    // 코드 페어링: 가짜 Relay 는 사람이 친 코드를 모른다(제 코드 123456 와 다른 값을 친다).
    let stream = crate::noise::initiate(other.transport().open().await.unwrap(), &desktop)
        .await
        .unwrap();
    let paired = crate::pair_with_code(stream, &identity(&desktop), "654321", "laptop", WAIT).await;
    assert!(paired.is_err(), "{paired:?}");
}

/// 0-RTT 를 받지 않는다: 받는 쪽은 세션 티켓을 내지 않아, 이른 데이터를 켠 붙는 쪽도 두 번째 연결에서 0-RTT 로 보낼 수 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_rtt_is_never_available() {
    let listener = Arc::new(QuicListener::bind(loopback()).unwrap());
    let addr = listener.local_addr().unwrap();
    {
        let listener = Arc::clone(&listener);
        tokio::spawn(async move {
            while let Some(incoming) = listener.next().await {
                tokio::spawn(async move {
                    if let Ok(mut stream) = incoming.accept().await {
                        let _ = stream.write.write_all(b"x").await;
                        let mut sink = Vec::new();
                        let _ =
                            tokio::io::AsyncReadExt::read_to_end(&mut stream.read, &mut sink).await;
                    }
                });
            }
        });
    }
    // 이른 데이터를 켠 붙는 쪽(이 크레이트의 붙는 쪽은 켜지 않는다 — 받는 쪽만으로 막히는지 본다).
    let endpoint = quinn::Endpoint::client((Ipv4Addr::UNSPECIFIED, 0).into()).unwrap();
    let config = {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Trusting(provider)))
            .with_no_client_auth();
        tls.alpn_protocols = vec![crate::quic::ALPN.to_vec()];
        tls.enable_early_data = true;
        quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap(),
        ))
    };
    for _ in 0..2 {
        let connecting = endpoint
            .connect_with(config.clone(), addr, crate::quic::SERVER_NAME)
            .unwrap();
        match connecting.into_0rtt() {
            Ok(_) => panic!("0-RTT was offered"),
            Err(connecting) => {
                let connection = connecting.await.unwrap();
                let (mut send, mut recv) = connection.open_bi().await.unwrap();
                send.write_all(b"hello").await.unwrap();
                let mut one = [0u8; 1];
                tokio::io::AsyncReadExt::read_exact(&mut recv, &mut one)
                    .await
                    .unwrap();
                connection.close(0u32.into(), b"done");
            }
        }
    }
}

#[derive(Debug)]
struct Trusting(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for Trusting {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// 받는 쪽 인증서에는 고정 이름만 실린다 — 이 머신의 호스트 이름 · 사용자명이 없다(QUIC 핸드셰이크는 Noise 전에 평문 인증서를
/// 보이지 않지만 TLS 1.3 은 인증서를 암호화하므로 능동 상대에게만 보인다 — 그 상대에게도 무엇을 드러내지 않게).
#[test]
fn the_server_certificate_names_nothing_of_this_machine() {
    let certified =
        rcgen::generate_simple_self_signed(vec![crate::quic::SERVER_NAME.to_owned()]).unwrap();
    let der = certified.cert.der().to_vec();
    let contains = |needle: &str| der.windows(needle.len()).any(|w| w == needle.as_bytes());
    assert!(contains(crate::quic::SERVER_NAME));
    for private in [std::env::var("USER").ok(), std::env::var("HOSTNAME").ok()]
        .into_iter()
        .flatten()
        .filter(|s| s.len() >= 3)
    {
        assert!(!contains(&private), "the certificate carries {private}");
    }
    // 실제 받는 쪽 설정이 이 함수로 인증서를 만든다(설정이 세워진다).
    crate::quic::server_config().unwrap();
}

/// 와일드카드 주소에는 묶지 않는다.
#[tokio::test]
async fn a_wildcard_listener_is_refused() {
    for addr in ["0.0.0.0:0", "[::]:0"] {
        let addr: SocketAddr = addr.parse().unwrap();
        assert!(QuicListener::bind(addr).is_err(), "{addr}");
    }
}

/// 같은 주소 · 포트를 다른 프로그램이 쥐고 있으면 묶지 못한다(가리지 않는다 — `SO_REUSEADDR` 없음).
#[tokio::test]
async fn a_taken_port_is_not_shared() {
    let held = std::net::UdpSocket::bind(loopback()).unwrap();
    let addr = held.local_addr().unwrap();
    assert!(QuicListener::bind(addr).is_err());
}

/// 응답 없는 주소는 시한 안에 실패로 끝난다(SSH 로 떨어질 수 있게).
#[tokio::test]
async fn a_silent_address_fails_within_the_connect_timeout() {
    let silent = std::net::UdpSocket::bind(loopback()).unwrap();
    let addr = silent.local_addr().unwrap();
    let transport = QuicTransport::new(addr.ip().to_string(), addr.port())
        .with_connect_timeout(Duration::from_millis(300));
    let started = std::time::Instant::now();
    let failed = transport.open().await.err().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(failed.kind(), std::io::ErrorKind::TimedOut);
}
