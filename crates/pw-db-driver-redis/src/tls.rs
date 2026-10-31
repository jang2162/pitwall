//! TCP · TLS 스트림(결정 M3-D10). redis 크레이트의 TLS 기능을 켜지 않고 Pitwall 이 스트림을 만들어
//! `MultiplexedConnection::new_with_config` 에 넘긴다 — 검증은 PG 와 같은 OS 신뢰 저장소(`rustls-platform-verifier`)다.
//!
//! - `disable`: 평문 TCP.
//! - `require`: TLS 만, 인증서를 검증하지 않는다(서명만 본다 — 화면에 「서버 인증서를 검증하지 않음」).
//! - `verify-full`: OS 신뢰 저장소 + 설정의 `root_cert`(PEM)로 사슬과 이름을 검증한다. TLS 이름은 늘 설정의 DB 호스트다(터널
//!   너머에서도).
//!
//! 그 밖의 `sslmode`(`allow` · `prefer` · `verify-ca`)는 받지 않는다([`crate::SSL_MODES`]) — `TlsPlan::new` 가 거절한다.

use std::path::Path;
use std::sync::Arc;

use pw_db_driver::tls::TlsAttempt;
use pw_db_driver::{DbResult, TlsPlan};
use pw_db_types::{DbError, DbErrorKind, TlsSettings};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// `MultiplexedConnection` 에 넘기는 스트림(평문이든 TLS 든).
pub(crate) trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// 계획대로 TLS 를 얹는다. `used_tls` 는 [`TlsPlan::status`] 에 넘긴다.
pub(crate) async fn wrap(
    tcp: TcpStream,
    plan: &TlsPlan,
    settings: &TlsSettings,
) -> DbResult<(Box<dyn Io>, bool)> {
    if plan.attempt == TlsAttempt::Never {
        return Ok((Box::new(tcp), false));
    }
    let config = client_config(plan, settings)?;
    let name = plan
        .server_name
        .clone()
        .ok_or_else(|| DbError::new(DbErrorKind::Invalid, "TLS needs a host name"))?;
    let name = ServerName::try_from(name)
        .map_err(|e| DbError::new(DbErrorKind::Invalid, format!("bad TLS host name: {e}")))?;
    let stream = TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|e| DbError::new(DbErrorKind::Tls, format!("TLS handshake failed: {e}")))?;
    Ok((Box::new(stream), true))
}

/// rustls 설정. 공급자는 `ring` 하나다(결정 M3-D14 — `aws-lc-rs` 를 들이지 않는다).
pub(crate) fn client_config(plan: &TlsPlan, settings: &TlsSettings) -> DbResult<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier: Arc<dyn ServerCertVerifier> = if plan.verify_chain {
        let extra = match &settings.root_cert {
            Some(path) => read_certs(path)?,
            None => Vec::new(),
        };
        let verifier = if extra.is_empty() {
            rustls_platform_verifier::Verifier::new(provider.clone())
        } else {
            rustls_platform_verifier::Verifier::new_with_extra_roots(extra, provider.clone())
        }
        .map_err(|e| DbError::new(DbErrorKind::Tls, format!("TLS verifier: {e}")))?;
        Arc::new(verifier)
    } else {
        Arc::new(NoVerification(provider.signature_verification_algorithms))
    };
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| DbError::new(DbErrorKind::Tls, format!("TLS versions: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    match (&settings.client_cert, &settings.client_key) {
        (None, None) => Ok(builder.with_no_client_auth()),
        (Some(cert), Some(key)) => {
            let chain = read_certs(cert)?;
            let key = PrivateKeyDer::from_pem_file(key).map_err(|e| {
                DbError::new(
                    DbErrorKind::Tls,
                    format!("cannot read the client key {}: {e}", key.display()),
                )
            })?;
            builder
                .with_client_auth_cert(chain, key)
                .map_err(|e| DbError::new(DbErrorKind::Tls, format!("client certificate: {e}")))
        }
        _ => Err(DbError::new(
            DbErrorKind::Invalid,
            "a client certificate needs both the certificate and the key",
        )),
    }
}

fn read_certs(path: &Path) -> DbResult<Vec<CertificateDer<'static>>> {
    let unreadable = |e: &dyn std::fmt::Display| {
        DbError::new(
            DbErrorKind::Tls,
            format!("cannot read certificates from {}: {e}", path.display()),
        )
    };
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|e| unreadable(&e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| unreadable(&e))?;
    if certs.is_empty() {
        return Err(unreadable(&"no PEM certificate in the file"));
    }
    Ok(certs)
}

/// `require`: 인증서를 믿는다. 핸드셰이크의 서명은 여전히 그 인증서의 키로 확인한다(가로챈 쪽이 다른 키로 서명하면 끊긴다 —
/// 인증서 자체를 믿지 않으므로 중간자는 막지 못한다).
#[derive(Debug)]
struct NoVerification(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for NoVerification {
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
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}
