//! TLS 설정(결정 M3-D10): `sslmode` 를 푼 계획(`pw_db_driver::TlsPlan`)을 rustls 설정으로 바꾼다.
//!
//! - 루트는 OS 신뢰 저장소(`rustls-platform-verifier`)에 설정의 `root_cert`(PEM 묶음)를 더한 것이다 — 사내 CA 와 시험 CA 가
//!   들어오는 자리다.
//! - `verify-full`: 사슬과 이름. `verify-ca`: 사슬만([`ChainOnly`]). 나머지(`allow` · `prefer` · `require`)는 인증서를 보지 않고
//!   핸드셰이크 서명만 확인한다([`NoVerification`]) — 화면이 「검증하지 않음」 을 보인다.
//! - TLS 이름은 늘 설정의 DB 호스트다. 터널로 `127.0.0.1` 에 붙어도 같다 — `tokio-postgres` 의 `host`(이름) · `hostaddr`(TCP)
//!   가 이것을 나눈다(`connect`).
//! - 암호 공급자는 `ring` 하나다.

use std::path::Path;
use std::sync::Arc;

use pw_db_driver::TlsPlan;
use pw_db_types::{DbError, DbErrorKind, TlsSettings};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio_postgres_rustls::MakeRustlsConnect;

/// 계획대로 검증하는 TLS 연결기. 평문만 쓰는 계획(`disable`)에도 만든다 — `tokio-postgres` 가 TLS 를 시도하지 않으면 쓰이지 않는다.
pub(crate) fn connector(
    plan: &TlsPlan,
    settings: &TlsSettings,
) -> Result<MakeRustlsConnect, DbError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier: Arc<dyn ServerCertVerifier> = if plan.verify_chain {
        let roots = match &settings.root_cert {
            Some(path) => read_certs(path)?,
            None => Vec::new(),
        };
        let platform =
            rustls_platform_verifier::Verifier::new_with_extra_roots(roots, provider.clone())
                .map_err(|e| tls_error(format!("cannot load the OS trust store: {e}")))?;
        if plan.verify_name {
            Arc::new(platform)
        } else {
            Arc::new(ChainOnly { inner: platform })
        }
    } else {
        Arc::new(NoVerification {
            provider: provider.clone(),
        })
    };
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| tls_error(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let config = match (&settings.client_cert, &settings.client_key) {
        (Some(cert), Some(key)) => {
            let certs = read_certs(cert)?;
            let key = PrivateKeyDer::from_pem_file(key).map_err(|e| {
                tls_error(format!("cannot read the client key {}: {e}", key.display()))
            })?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| tls_error(format!("bad client certificate: {e}")))?
        }
        (None, None) => builder.with_no_client_auth(),
        _ => {
            return Err(DbError::new(
                DbErrorKind::Invalid,
                "a client certificate needs both the certificate and the key",
            ));
        }
    };
    Ok(MakeRustlsConnect::new(config))
}

fn read_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, DbError> {
    let certs = CertificateDer::pem_file_iter(path)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .map_err(|e| {
            tls_error(format!(
                "cannot read certificates from {}: {e}",
                path.display()
            ))
        })?;
    if certs.is_empty() {
        return Err(tls_error(format!("no certificate in {}", path.display())));
    }
    Ok(certs)
}

fn tls_error(message: String) -> DbError {
    DbError::new(DbErrorKind::Tls, message)
}

/// `verify-ca`: 사슬은 OS 검증기로 보고 이름은 보지 않는다(libpq 의 뜻).
///
/// 이름 오류만 걸러 내는 방식은 검증기가 이름 오류를 먼저 내면 사슬 오류를 가린다(macOS 의 신뢰 평가는 오류 하나를 낸다). 그래서
/// 인증서가 스스로 내세우는 DNS 이름 하나로 바꿔 검증한다 — 그 이름은 맞으므로 남는 오류는 사슬의 것이다. DNS 이름이 없는
/// 인증서(IP 만)는 설정의 이름으로 검증하고 이름 오류만 받아들인다.
#[derive(Debug)]
struct ChainOnly {
    inner: rustls_platform_verifier::Verifier,
}

impl ServerCertVerifier for ChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let own_name = webpki::EndEntityCert::try_from(end_entity)
            .ok()
            .and_then(|cert| cert.valid_dns_names().next().map(str::to_owned))
            .and_then(|name| ServerName::try_from(name).ok());
        match own_name {
            Some(name) => {
                self.inner
                    .verify_server_cert(end_entity, intermediates, &name, ocsp_response, now)
            }
            None => match self.inner.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ) {
                Err(rustls::Error::InvalidCertificate(
                    CertificateError::NotValidForName
                    | CertificateError::NotValidForNameContext { .. },
                )) => Ok(ServerCertVerified::assertion()),
                other => other,
            },
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// `allow` · `prefer` · `require`: 인증서를 검증하지 않는다(libpq 의 뜻). 핸드셰이크 서명은 확인한다 — 그 인증서의 키를 가진
/// 서버와 이야기하는 것까지는 보장한다.
#[derive(Debug)]
struct NoVerification {
    provider: Arc<CryptoProvider>,
}

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
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
