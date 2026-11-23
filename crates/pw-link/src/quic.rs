//! 직접 연결 전송(18-m7-plan M7-24 ① · 08 「연결 방식」): QUIC(`quinn`) 연결 하나에 양방향 스트림 하나가 링크 하나다. 기본 포트
//! [`DEFAULT_PORT`].
//!
//! - **QUIC 의 TLS 는 신뢰 근거가 아니다.** 받는 쪽은 뜰 때마다 자체 서명 인증서를 만들고(이름은 고정값 [`SERVER_NAME`] — 머신
//!   이름 · 사용자명 · 키 지문을 싣지 않는다), 붙는 쪽은 어떤 인증서든 받는다(TLS 서명이 그 인증서의 키로 맞는지만 본다). 상대가
//!   누구인지는 그 위의 Noise(`crate::noise` — M7-23)와 기억한 머신 키가 판정한다. 그래서 이 전송은 **[`crate::SecuredTransport`]
//!   로 감싸서만** 쓰고, 받는 쪽은 [`crate::serve_noise`] 로만 잇는다 — Noise 없이 링크 틀을 받는 길이 없다.
//! - **0-RTT 를 받지 않는다**: 받는 쪽은 세션 티켓을 내지 않고(`send_tls13_tickets = 0`) 이른 데이터 상한이 0 이다. 붙는 쪽도 이른
//!   데이터를 켜지 않는다. 리플레이할 0-RTT 데이터가 없다.
//! - **주소 검증**: 검증되지 않은 주소의 첫 패킷에는 Retry 를 보낸다(받는 쪽이 일을 하기 전에 그 주소에서 왕복 하나를 요구한다 —
//!   위조한 출발 주소로 받는 쪽을 증폭기로 쓰지 못하게. quinn 은 검증 전 3배 송신 상한도 지킨다).
//! - **연결 하나에 스트림 하나**: 양방향 스트림 동시 1개 · 단방향 0개 · 데이터그램 없음. 스트림을 연 지 [`STREAM_TIMEOUT`] 안에
//!   열지 않으면 닫는다. 증명 전 자원 상한(연결 수 · 주소별 · 전체 시한)은 받는 쪽 데몬이 건다(`pw_relay::direct`).
//! - 듣는 소켓은 `SO_REUSEADDR` 없이 연다(다른 프로그램이 쥔 포트를 가리지 않게 — 08 「보안 모델」 포워딩 줄과 같은 까닭).

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use pw_core::BoxFuture;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::transport::{LinkStream, Transport};

/// 직접 연결의 기본 포트(08 「연결 방식」 · `relay.toml [listen] port`).
pub const DEFAULT_PORT: u16 = 7420;
/// LAN 검색(mDNS)의 서비스 종류(18-m7-plan M7-24 ③). 광고에는 무작위 이름 · 포트 · 링크 프로토콜 판만 싣는다(`pw_relay::advertise`).
pub const MDNS_SERVICE: &str = "_pitwall-relay._udp.local.";
/// 광고 TXT 의 링크 프로토콜 판 키.
pub const MDNS_PROTOCOL_KEY: &str = "pv";
/// ALPN — 다른 QUIC 서비스와 섞이지 않게.
pub const ALPN: &[u8] = b"pitwall-link/1";
/// 인증서 · SNI 의 이름(고정값 — 머신을 드러내지 않는다).
pub const SERVER_NAME: &str = "pitwall-relay";
/// 붙는 쪽이 QUIC 연결을 세우는 시한(주소 하나마다).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 받는 쪽이 연결 뒤 스트림 하나가 열리기를 기다리는 시한.
pub const STREAM_TIMEOUT: Duration = Duration::from_secs(5);
/// 응답 없는 QUIC 연결을 닫는 시간. 링크의 하트비트(10초 ping · 30초)보다 길다 — 끊김 판정은 링크가 한다.
const IDLE_TIMEOUT: Duration = Duration::from_secs(40);
/// NAT 이 길을 잊지 않게 보내는 keep-alive.
const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// 스트림 · 연결의 받기 창. 링크 틀 상한(64 MiB)은 조각으로 흐른다 — 창은 흐름 제어일 뿐이다.
const STREAM_WINDOW: u32 = 1 << 20;
const CONNECTION_WINDOW: u32 = 2 << 20;
/// 받는 쪽이 아직 받지 않은(앱이 고르지 않은) 연결의 수 상한.
const MAX_INCOMING: usize = 64;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn tls_error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("quic tls: {e}"))
}

fn transport_config(server: bool) -> io::Result<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    // 상대가 열 수 있는 스트림: 받는 쪽은 양방향 하나, 붙는 쪽은 받지 않는다.
    t.max_concurrent_bidi_streams(u32::from(server).into());
    t.max_concurrent_uni_streams(0u32.into());
    t.datagram_receive_buffer_size(None);
    t.stream_receive_window(STREAM_WINDOW.into());
    t.receive_window(CONNECTION_WINDOW.into());
    t.max_idle_timeout(Some(
        IDLE_TIMEOUT
            .try_into()
            .map_err(|e| io::Error::other(format!("quic idle timeout: {e}")))?,
    ));
    t.keep_alive_interval(Some(KEEP_ALIVE));
    Ok(t)
}

/// 받는 쪽 설정: 뜰 때마다 새 자체 서명 인증서, 0-RTT 없음, 주소 이동 없음.
pub fn server_config() -> io::Result<quinn::ServerConfig> {
    let certified =
        rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_owned()]).map_err(tls_error)?;
    let cert = CertificateDer::from(certified.cert.der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(tls_error)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    // 0-RTT 를 받지 않는다: 이른 데이터 0 · 세션 티켓 없음(다시 붙기는 늘 새 핸드셰이크).
    tls.max_early_data_size = 0;
    tls.send_tls13_tickets = 0;
    let crypto = QuicServerConfig::try_from(tls).map_err(tls_error)?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config(true)?));
    config.migration(false);
    config.max_incoming(MAX_INCOMING);
    Ok(config)
}

/// 어떤 인증서든 받는다(TLS 서명은 그 인증서의 키로 본다). 상대 판정은 Noise 가 한다(머리 주석).
#[derive(Debug)]
struct AnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
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
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
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

/// 붙는 쪽 설정: 어떤 인증서든 받고, 이른 데이터를 켜지 않는다.
pub fn client_config() -> io::Result<quinn::ClientConfig> {
    let provider = provider();
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_error)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.enable_early_data = false;
    let crypto = QuicClientConfig::try_from(tls).map_err(tls_error)?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config(false)?));
    Ok(config)
}

/// 스트림의 쓰는 쪽. 연결(과 붙는 쪽의 엔드포인트)을 쥐고 있어 스트림이 사는 동안 연결이 닫히지 않는다.
struct Write {
    send: quinn::SendStream,
    _connection: quinn::Connection,
    _endpoint: Option<quinn::Endpoint>,
}

impl AsyncWrite for Write {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.send)
            .poll_write(cx, buf)
            .map_err(io::Error::from)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.send), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.send), cx)
    }
}

/// 스트림의 읽는 쪽.
struct Read {
    recv: quinn::RecvStream,
    connection: quinn::Connection,
}

impl AsyncRead for Read {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl Drop for Read {
    fn drop(&mut self) {
        // 읽는 쪽을 버렸다 — 링크가 끝났다. 쓰는 쪽이 남아 있어도 연결을 닫는다.
        self.connection.close(0u32.into(), b"closed");
    }
}

fn stream(
    connection: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    endpoint: Option<quinn::Endpoint>,
) -> LinkStream {
    LinkStream::new(
        Read {
            recv,
            connection: connection.clone(),
        },
        Write {
            send,
            _connection: connection,
            _endpoint: endpoint,
        },
    )
}

fn connection_error(e: quinn::ConnectionError) -> io::Error {
    let kind = match e {
        quinn::ConnectionError::TimedOut => io::ErrorKind::TimedOut,
        quinn::ConnectionError::ConnectionClosed(_)
        | quinn::ConnectionError::ApplicationClosed(_)
        | quinn::ConnectionError::Reset => io::ErrorKind::ConnectionReset,
        _ => io::ErrorKind::ConnectionAborted,
    };
    io::Error::new(kind, format!("quic: {e}"))
}

/// 받는 쪽: 주소 하나에 묶인 QUIC 엔드포인트.
pub struct QuicListener {
    endpoint: quinn::Endpoint,
    retries: Arc<AtomicUsize>,
}

impl QuicListener {
    /// `addr` 에 묶는다. 와일드카드 주소(`0.0.0.0` · `::`)는 받지 않는다 — 듣는 주소는 고른 인터페이스의 주소다(`pw_relay::listen`).
    pub fn bind(addr: SocketAddr) -> io::Result<Self> {
        if addr.ip().is_unspecified() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a direct-connection listener binds one interface address, never a wildcard",
            ));
        }
        // std 의 bind 는 SO_REUSEADDR 를 켜지 않는다.
        let socket = std::net::UdpSocket::bind(addr)?;
        let runtime = quinn::default_runtime()
            .ok_or_else(|| io::Error::other("no async runtime for quic"))?;
        let endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(server_config()?),
            socket,
            runtime,
        )?;
        Ok(Self {
            endpoint,
            retries: Arc::default(),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }

    /// 지금까지 보낸 Retry 수(주소 검증 — 시험 · 로그).
    pub fn retries(&self) -> usize {
        self.retries.load(Ordering::SeqCst)
    }

    /// 다음에 들어온, **주소가 검증된** 연결. 검증되지 않은 주소에는 Retry 를 보내고 다음을 기다린다. 엔드포인트가 닫히면 `None`.
    pub async fn next(&self) -> Option<QuicIncoming> {
        loop {
            let incoming = self.endpoint.accept().await?;
            if !incoming.remote_address_validated() {
                if incoming.may_retry() {
                    self.retries.fetch_add(1, Ordering::SeqCst);
                    if let Err(e) = incoming.retry() {
                        e.into_incoming().refuse();
                    }
                } else {
                    incoming.refuse();
                }
                continue;
            }
            return Some(QuicIncoming { inner: incoming });
        }
    }

    /// 듣기를 멈추고 열린 연결을 닫는다.
    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"relay stopped listening");
    }
}

impl Drop for QuicListener {
    fn drop(&mut self) {
        self.close();
    }
}

/// 주소가 검증된, 아직 받지 않은 연결.
pub struct QuicIncoming {
    inner: quinn::Incoming,
}

impl QuicIncoming {
    pub fn remote(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// 받지 않고 닫는다(자리가 없다 등).
    pub fn refuse(self) {
        self.inner.refuse();
    }

    /// QUIC 핸드셰이크를 마치고 상대가 연 스트림 하나를 받는다. 시한은 부르는 쪽이 건다(스트림 기다림은 [`STREAM_TIMEOUT`]).
    pub async fn accept(self) -> io::Result<LinkStream> {
        let connection = self
            .inner
            .accept()
            .map_err(connection_error)?
            .await
            .map_err(connection_error)?;
        let (send, recv) = match tokio::time::timeout(STREAM_TIMEOUT, connection.accept_bi()).await
        {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => return Err(connection_error(e)),
            Err(_) => {
                connection.close(0u32.into(), b"no stream");
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "quic: the peer opened no stream",
                ));
            }
        };
        Ok(stream(connection, send, recv, None))
    }
}

/// 붙는 쪽: 주소(호스트 이름 · IP)와 포트로 QUIC 연결을 열고 스트림 하나를 낸다. **[`crate::SecuredTransport`] 로 감싸서 쓴다.**
pub struct QuicTransport {
    host: String,
    port: u16,
    connect_timeout: Duration,
}

impl QuicTransport {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            connect_timeout: CONNECT_TIMEOUT,
        }
    }

    /// 주소 하나마다의 연결 시한.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    async fn open_one(&self, addr: SocketAddr) -> io::Result<LinkStream> {
        let local: SocketAddr = match addr.ip() {
            IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let endpoint = quinn::Endpoint::client(local)?;
        let connecting = endpoint
            .connect_with(client_config()?, addr, SERVER_NAME)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("quic: {e}")))?;
        let connection = tokio::time::timeout(self.connect_timeout, connecting)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "quic: no answer"))?
            .map_err(connection_error)?;
        let (send, recv) = connection.open_bi().await.map_err(connection_error)?;
        Ok(stream(connection, send, recv, Some(endpoint)))
    }
}

/// `addr` 에 직접 연결 받는 쪽이 있는가(QUIC 핸드셰이크만 하고 스트림을 열지 않고 닫는다 — 검색 결과의 표시). 상대를 증명하지
/// 않는다 — 있다는 것만 본다.
pub async fn probe(addr: SocketAddr, timeout: Duration) -> bool {
    let local: SocketAddr = match addr.ip() {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let Ok(endpoint) = quinn::Endpoint::client(local) else {
        return false;
    };
    let Ok(config) = client_config() else {
        return false;
    };
    let Ok(connecting) = endpoint.connect_with(config, addr, SERVER_NAME) else {
        return false;
    };
    match tokio::time::timeout(timeout, connecting).await {
        Ok(Ok(connection)) => {
            connection.close(0u32.into(), b"probe");
            true
        }
        _ => false,
    }
}

impl Transport for QuicTransport {
    fn describe(&self) -> String {
        format!("quic {}:{}", self.host, self.port)
    }

    fn open(&self) -> BoxFuture<'_, io::Result<LinkStream>> {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((self.host.as_str(), self.port))
                .await?
                .collect();
            let mut last = io::Error::new(io::ErrorKind::NotFound, "quic: no address");
            for addr in addrs {
                match self.open_one(addr).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => last = e,
                }
            }
            Err(last)
        })
    }
}
