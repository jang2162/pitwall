//! MySQL TLS(결정 M3-D10): `sslmode` 의 계획(`pw_db_driver::TlsPlan`)을 `mysql_async` 의 `SslOpts` 로 옮긴다.
//!
//! - **루트는 OS 신뢰 저장소뿐이다.** `mysql_async` 의 `rustls-tls` 기능은 `webpki-roots` 를 늘 링크하지만 내장 루트를 늘 끄고
//!   (`disable_built_in_roots`) [`RootSource`] 가 준 루트(기본은 `rustls-native-certs` 가 읽은 OS 루트)와 설정의 `root_cert` 만 넣는다.
//!   OS 의 폐기 확인 · 기업 정책은 보지 않는다 — `mysql_async` 가 verifier 를 받지 않는다(14 §3).
//! - **TLS 이름은 설정의 DB 호스트다** — 터널로 `127.0.0.1:<로컬 포트>` 에 붙어도 `with_danger_tls_hostname_override` 로 그 이름을 쓴다.
//! - `require` · `prefer` · `allow` 는 인증서를 검증하지 않는다(`accept_invalid_certs`).
//! - **`verify-ca`(사슬만)는 받지 않는다**([`SSL_MODES`]). `mysql_async` 0.37 의 「이름 검증 건너뛰기」(`skip_domain_validation`)는
//!   rustls 오류의 글에서 `NotValidForName` 을 찾는데 rustls 0.23 의 글(`certificate not valid for name …`)에는 그 낱말이 없어 이름이
//!   틀리면 늘 거절한다 — 고르면 말없이 `verify-full` 이 된다. 받지 않는 값을 다른 값으로 바꿔 붙지 않으므로(`TlsPlan::new`) 연결 편집
//!   다이얼로그(M3-14)가 고를 수 없게 지원 목록에서 뺀다. 인증서가 내민 이름으로 다시 붙는 우회는 그 이름이 rustls 오류의 `Debug`
//!   글로만 오므로 쓰지 않는다.

use std::sync::Arc;

use mysql_async::{ClientIdentity, SslOpts};
use pw_db_driver::TlsPlan;
use pw_db_types::{DbError, DbErrorKind, SslMode, Target, TlsSettings};
use rustls::RootCertStore;

/// 이 드라이버가 받는 `sslmode` — `verify-ca` 를 뺀 전부(머리 주석).
pub const SSL_MODES: [SslMode; 5] = [
    SslMode::Disable,
    SslMode::Allow,
    SslMode::Prefer,
    SslMode::Require,
    SslMode::VerifyFull,
];

/// 설정의 TLS 계획. `verify-ca` 는 까닭과 대안을 실어 거절한다 — 붙어 보기 전이다.
pub(crate) fn plan(settings: &TlsSettings, target: &Target) -> Result<TlsPlan, DbError> {
    if settings.mode == SslMode::VerifyCa && matches!(target, Target::Network { .. }) {
        return Err(DbError::new(
            DbErrorKind::Invalid,
            "the MySQL driver does not support sslmode verify-ca (mysql_async cannot skip only the name check) \
             — use verify-full, or make the certificate name match the host",
        ));
    }
    TlsPlan::new(settings, target, &SSL_MODES)
}

/// OS 루트(DER)를 주는 자리. 기본은 [`native_roots`]. 시험은 시험 CA 를 더한 공급자를 넣는다(`MysqlDriver::with_root_source`).
pub type RootSource = Arc<dyn Fn() -> Result<Vec<Vec<u8>>, String> + Send + Sync>;

/// OS 신뢰 저장소의 루트 가운데 webpki 가 신뢰 앵커로 읽는 것만. 읽지 못하는 항목 하나가 연결 전체를 막지 않게 거른다 —
/// `mysql_async` 는 루트 하나라도 넣지 못하면 연결을 실패시킨다.
pub fn native_roots() -> Result<Vec<Vec<u8>>, String> {
    let loaded = rustls_native_certs::load_native_certs();
    let mut store = RootCertStore::empty();
    let roots: Vec<Vec<u8>> = loaded
        .certs
        .into_iter()
        .filter(|cert| store.add(cert.clone()).is_ok())
        .map(|cert| cert.as_ref().to_vec())
        .collect();
    if roots.is_empty() {
        let why = loaded.errors.first().map(ToString::to_string);
        return Err(format!(
            "no usable root certificates in the OS trust store{}",
            why.map(|w| format!(": {w}")).unwrap_or_default()
        ));
    }
    Ok(roots)
}

/// TLS 로 붙을 때의 `SslOpts`. `tls_name` 은 이름 검증 · SNI 에 쓸 이름(설정의 DB 호스트).
pub(crate) fn ssl_opts(
    plan: &TlsPlan,
    settings: &TlsSettings,
    os_roots: Vec<Vec<u8>>,
    tls_name: String,
) -> Result<SslOpts, DbError> {
    let mut roots: Vec<_> = os_roots.into_iter().map(Into::into).collect();
    if let Some(path) = &settings.root_cert {
        roots.push(path.clone().into());
    }
    let identity = match (&settings.client_cert, &settings.client_key) {
        (Some(cert), Some(key)) => {
            Some(ClientIdentity::new(cert.clone().into(), key.clone().into()))
        }
        (None, None) => None,
        _ => {
            return Err(DbError::new(
                DbErrorKind::Invalid,
                "a client certificate needs both the certificate and the key",
            ));
        }
    };
    Ok(SslOpts::default()
        .with_disable_built_in_roots(true)
        .with_root_certs(roots)
        .with_client_identity(identity)
        .with_danger_accept_invalid_certs(!plan.verify_chain)
        .with_danger_tls_hostname_override(Some(tls_name)))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pw_db_types::{SocketTarget, SslMode, Target};

    use super::*;

    fn tunnelled() -> Target {
        Target::Network {
            host: "db.internal".into(),
            port: 3306,
            connect_to: Some(SocketTarget {
                host: "127.0.0.1".into(),
                port: 50_123,
            }),
        }
    }

    /// 14-m3-plan.md M3-07 ⑥ 의 구성 시험: TLS 를 쓰는 어느 `sslmode` 에서나 내장 루트가 꺼져 있고, 루트는 공급자가 준 것뿐이며,
    /// TLS 이름은 터널이 아니라 설정의 DB 호스트다. `disable` 은 TLS 를 시도하지 않고(`TlsAttempt::Never` — 드라이버가 `SslOpts` 를
    /// 만들지 않는다), `allow` · `prefer` 는 TLS 쪽 시도에 이 `SslOpts` 를 쓴다.
    #[test]
    fn every_mode_turns_the_built_in_roots_off_and_uses_only_the_supplied_roots() {
        let supplied = vec![b"root one".to_vec(), b"root two".to_vec()];
        let expected =
            SslOpts::default().with_root_certs(supplied.iter().cloned().map(Into::into).collect());
        for mode in SSL_MODES {
            let plan = TlsPlan::new(&TlsSettings::mode(mode), &tunnelled(), &SSL_MODES).unwrap();
            let name = plan.server_name.clone().unwrap();
            let opts = ssl_opts(&plan, &TlsSettings::mode(mode), supplied.clone(), name).unwrap();
            assert!(opts.disable_built_in_roots(), "{mode:?}");
            assert_eq!(opts.root_certs(), expected.root_certs(), "{mode:?}");
            assert_eq!(
                opts.tls_hostname_override(),
                Some("db.internal"),
                "{mode:?}"
            );
            assert_eq!(opts.accept_invalid_certs(), !plan.verify_chain, "{mode:?}");
            assert!(!opts.skip_domain_validation(), "{mode:?}");
        }
    }

    #[test]
    fn a_configured_ca_is_added_after_the_os_roots_and_half_an_identity_is_refused() {
        let mut settings = TlsSettings::mode(SslMode::VerifyFull);
        settings.root_cert = Some(PathBuf::from("/etc/pitwall/ca.pem"));
        let plan = TlsPlan::new(&settings, &tunnelled(), &SSL_MODES).unwrap();
        let opts = ssl_opts(&plan, &settings, vec![b"os".to_vec()], "db.internal".into()).unwrap();
        let expected = SslOpts::default().with_root_certs(vec![
            b"os".to_vec().into(),
            PathBuf::from("/etc/pitwall/ca.pem").into(),
        ]);
        assert_eq!(opts.root_certs(), expected.root_certs());

        settings.client_cert = Some(PathBuf::from("/etc/pitwall/client.pem"));
        let half = ssl_opts(&plan, &settings, vec![], "db.internal".into());
        assert_eq!(half.unwrap_err().kind, DbErrorKind::Invalid);
    }

    #[test]
    fn verify_ca_is_refused_rather_than_turned_into_verify_full() {
        let refused = TlsPlan::new(
            &TlsSettings::mode(SslMode::VerifyCa),
            &tunnelled(),
            &SSL_MODES,
        );
        assert_eq!(refused.unwrap_err().kind, DbErrorKind::Invalid);
    }
}
