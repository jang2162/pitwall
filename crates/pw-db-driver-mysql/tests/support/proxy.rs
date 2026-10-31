//! 실 서버 앞의 TLS 중계 — 시험 CA 가 서명한 `db.pitwall.test` 인증서를 드라이버에 내민다(14-m3-plan.md M3-07 ⑥).
//!
//! MySQL 은 평문 핸드셰이크 뒤에 `SSLRequest` 로 TLS 를 시작하므로 TLS 앞단을 그대로 둘 수 없다. 중계는 서버의 첫 패킷을 넘기고,
//! 드라이버가 `SSLRequest` 를 보내면 그것을 서버에도 넘긴 뒤 양쪽과 따로 TLS 를 맺고(서버 쪽은 검증하지 않는다) 그 사이를 잇는다.
//! 양쪽이 같은 순서로 같은 패킷을 주고받으므로 순번 · 인증은 그대로 맞는다. 서버는 TLS 를 켠 채여야 한다(MySQL 8.4 · MariaDB 11.8 의
//! 공식 이미지는 기본으로 켠다).
//!
//! 인증서(`tests/fixtures`)는 `openssl` 로 만든 P-256 키 · 100년짜리다: `ca.pem`(CA, 키는 버렸다), `server.pem` · `server.key`
//! (SAN `DNS:db.pitwall.test`, EKU serverAuth).

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{CryptoProvider, ring};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// 서버 핸드셰이크 · 클라이언트 응답의 능력 비트 `CLIENT_SSL`.
const CLIENT_SSL: u32 = 0x0800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 드라이버가 TLS 를 청하면 시험 인증서로 받는다. 평문 드라이버는 그대로 잇는다.
    Tls,
    /// 서버의 `CLIENT_SSL` 을 지운다 — TLS 를 못 하는 서버처럼 보인다.
    NoTls,
    /// `Tls` 와 같되 평문 드라이버에는 `require_secure_transport` 의 오류(3159)로 답한다.
    RequireTls,
}

pub fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// 시험 CA(DER).
pub fn test_ca() -> Vec<u8> {
    CertificateDer::from_pem_file(fixture("ca.pem"))
        .unwrap()
        .as_ref()
        .to_vec()
}

/// 127.0.0.1 의 빈 포트에서 듣고 `upstream` 으로 잇는다. 돌려받은 주소로 붙는다.
pub async fn start(upstream: SocketAddr, mode: Mode) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config()));
    let connector = TlsConnector::from(Arc::new(upstream_config()));
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                return;
            };
            let (acceptor, connector) = (acceptor.clone(), connector.clone());
            tokio::spawn(async move {
                let _ = relay(client, upstream, mode, acceptor, connector).await;
            });
        }
    });
    address
}

async fn read_packet(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).await?;
    let length =
        usize::from(header[0]) | usize::from(header[1]) << 8 | usize::from(header[2]) << 16;
    let mut packet = header.to_vec();
    packet.resize(4 + length, 0);
    stream.read_exact(&mut packet[4..]).await?;
    Ok(packet)
}

/// 서버 핸드셰이크(v10)에서 아래 두 바이트 능력의 자리: 판본 글(NUL 끝) · 연결 id 4 · 인증 데이터 8 · 채움 1 뒤.
fn capability_offset(packet: &[u8]) -> usize {
    let version_end = packet[5..].iter().position(|b| *b == 0).unwrap() + 5;
    version_end + 1 + 4 + 8 + 1
}

fn client_capabilities(packet: &[u8]) -> u32 {
    u32::from_le_bytes(packet[4..8].try_into().unwrap())
}

async fn relay(
    mut client: TcpStream,
    upstream: SocketAddr,
    mode: Mode,
    acceptor: TlsAcceptor,
    connector: TlsConnector,
) -> std::io::Result<()> {
    let mut server = TcpStream::connect(upstream).await?;
    let mut greeting = read_packet(&mut server).await?;
    if mode == Mode::NoTls {
        let at = capability_offset(&greeting);
        greeting[at + 1] &= !((CLIENT_SSL >> 8) as u8);
    }
    client.write_all(&greeting).await?;
    if mode == Mode::NoTls {
        tokio::io::copy_bidirectional(&mut client, &mut server).await?;
        return Ok(());
    }
    let first = read_packet(&mut client).await?;
    let is_ssl_request = first.len() == 4 + 32 && client_capabilities(&first) & CLIENT_SSL != 0;
    if !is_ssl_request {
        if mode == Mode::RequireTls {
            let mut payload = vec![0xff];
            payload.extend_from_slice(&3159_u16.to_le_bytes());
            payload.extend_from_slice(b"#08004Connections using insecure transport are prohibited");
            let mut packet = (payload.len() as u32).to_le_bytes()[..3].to_vec();
            packet.push(first[3] + 1);
            packet.extend_from_slice(&payload);
            client.write_all(&packet).await?;
            return Ok(());
        }
        server.write_all(&first).await?;
        tokio::io::copy_bidirectional(&mut client, &mut server).await?;
        return Ok(());
    }
    server.write_all(&first).await?;
    let mut server = connector
        .connect(ServerName::try_from("mysql-upstream").unwrap(), server)
        .await?;
    let mut client = acceptor.accept(client).await?;
    tokio::io::copy_bidirectional(&mut client, &mut server).await?;
    Ok(())
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

fn server_config() -> ServerConfig {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(fixture("server.pem"))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(fixture("server.key")).unwrap();
    ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap()
}

/// 실 서버 쪽 TLS. 서버의 자체 서명 인증서를 검증하지 않는다(시험 대상은 드라이버 쪽 검증이다).
fn upstream_config() -> ClientConfig {
    ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny(provider())))
        .with_no_client_auth()
}

#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, tokio_rustls::rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        tokio_rustls::rustls::crypto::verify_tls12_signature(
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
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        tokio_rustls::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
