//! `sslmode` 를 libpq 뜻대로 푼 TLS 계획(결정 M3-D10). TLS 라이브러리 · 루트 저장소는 드라이버 크레이트가 고른다 — 여기는
//! 드라이버 셋(PG · MySQL · Redis)이 같은 뜻으로 읽게 하는 표 하나다.
//!
//! - 무엇을 시도하는가: `disable` 은 평문만, `allow` 는 평문을 먼저(서버가 요구하면 TLS), `prefer` 는 TLS 를 먼저(안 되면
//!   평문), `require` · `verify-ca` · `verify-full` 은 TLS 만.
//! - 무엇을 검증하는가: `verify-ca` 는 사슬만, `verify-full` 은 사슬과 이름. 나머지는 검증하지 않는다(화면에 「서버 인증서를
//!   검증하지 않음」 — M3-19).
//! - TLS 이름은 늘 설정의 DB 호스트다 — 터널로 `127.0.0.1:<로컬 포트>` 에 붙어도 같다.

use pw_db_types::{DbError, DbErrorKind, SslMode, Target, TlsSettings, TlsStatus};

/// 무엇을 시도하는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsAttempt {
    /// 평문만(`disable`).
    Never,
    /// 평문을 먼저, 서버가 거절하면 TLS(`allow`).
    PlainFirst,
    /// TLS 를 먼저, 서버가 TLS 를 못 하면 평문(`prefer`).
    TlsFirst,
    /// TLS 만(`require` · `verify-ca` · `verify-full`).
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsPlan {
    pub attempt: TlsAttempt,
    /// 인증서 사슬을 루트(OS 신뢰 저장소 + 설정의 `root_cert`)까지 검증한다.
    pub verify_chain: bool,
    /// 인증서의 이름이 [`TlsPlan::server_name`] 과 맞는지 본다.
    pub verify_name: bool,
    /// TLS 이름(SNI · 이름 검증). 설정의 DB 호스트. 파일 DB 는 `None`.
    pub server_name: Option<String>,
}

impl TlsPlan {
    /// 설정을 푼다. 드라이버가 받지 않는 `sslmode`(`supported` 밖)는 거절한다 — 받지 않는 값을 다른 값으로 바꿔 붙지 않는다.
    pub fn new(
        settings: &TlsSettings,
        target: &Target,
        supported: &[SslMode],
    ) -> Result<Self, DbError> {
        let server_name = match target {
            Target::Network { host, .. } => Some(host.clone()),
            Target::File { .. } => None,
        };
        if server_name.is_none() {
            return Ok(Self {
                attempt: TlsAttempt::Never,
                verify_chain: false,
                verify_name: false,
                server_name,
            });
        }
        if !supported.contains(&settings.mode) {
            return Err(DbError::new(
                DbErrorKind::Invalid,
                format!(
                    "sslmode {:?} is not supported by this driver",
                    settings.mode
                ),
            ));
        }
        let (attempt, verify_chain, verify_name) = match settings.mode {
            SslMode::Disable => (TlsAttempt::Never, false, false),
            SslMode::Allow => (TlsAttempt::PlainFirst, false, false),
            SslMode::Prefer => (TlsAttempt::TlsFirst, false, false),
            SslMode::Require => (TlsAttempt::Required, false, false),
            SslMode::VerifyCa => (TlsAttempt::Required, true, false),
            SslMode::VerifyFull => (TlsAttempt::Required, true, true),
        };
        Ok(Self {
            attempt,
            verify_chain,
            verify_name,
            server_name,
        })
    }

    /// 연결 결과의 TLS 상태. `used_tls` 는 실제로 TLS 로 붙었는지(`allow` · `prefer` 는 어느 쪽이든 될 수 있다).
    pub fn status(&self, used_tls: bool) -> TlsStatus {
        if self.server_name.is_none() {
            TlsStatus::NotApplicable
        } else if !used_tls {
            TlsStatus::Plaintext
        } else if self.verify_chain && self.verify_name {
            TlsStatus::Verified
        } else {
            TlsStatus::Unverified
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pw_db_types::SocketTarget;

    use super::*;

    fn tunnelled() -> Target {
        Target::Network {
            host: "db.internal".into(),
            port: 5432,
            connect_to: Some(SocketTarget {
                host: "127.0.0.1".into(),
                port: 50_123,
            }),
        }
    }

    #[test]
    fn modes_follow_libpq() {
        let table = [
            (
                SslMode::Disable,
                TlsAttempt::Never,
                false,
                false,
                TlsStatus::Plaintext,
            ),
            (
                SslMode::Allow,
                TlsAttempt::PlainFirst,
                false,
                false,
                TlsStatus::Unverified,
            ),
            (
                SslMode::Prefer,
                TlsAttempt::TlsFirst,
                false,
                false,
                TlsStatus::Unverified,
            ),
            (
                SslMode::Require,
                TlsAttempt::Required,
                false,
                false,
                TlsStatus::Unverified,
            ),
            (
                SslMode::VerifyCa,
                TlsAttempt::Required,
                true,
                false,
                TlsStatus::Unverified,
            ),
            (
                SslMode::VerifyFull,
                TlsAttempt::Required,
                true,
                true,
                TlsStatus::Verified,
            ),
        ];
        for (mode, attempt, chain, name, status) in table {
            let plan = TlsPlan::new(&TlsSettings::mode(mode), &tunnelled(), &SslMode::ALL).unwrap();
            assert_eq!(
                (plan.attempt, plan.verify_chain, plan.verify_name),
                (attempt, chain, name),
                "{mode:?}"
            );
            // 터널로 붙어도 TLS 이름은 설정의 DB 호스트다.
            assert_eq!(plan.server_name.as_deref(), Some("db.internal"));
            let used_tls = attempt != TlsAttempt::Never;
            assert_eq!(plan.status(used_tls), status, "{mode:?}");
            assert_eq!(plan.status(false), TlsStatus::Plaintext);
        }
    }

    #[test]
    fn unsupported_modes_are_refused_and_files_have_no_tls() {
        let redis = [SslMode::Disable, SslMode::Require, SslMode::VerifyFull];
        let refused = TlsPlan::new(&TlsSettings::mode(SslMode::Prefer), &tunnelled(), &redis);
        assert_eq!(refused.unwrap_err().kind, DbErrorKind::Invalid);

        let file = Target::File {
            path: PathBuf::from("/tmp/a.db"),
        };
        let plan = TlsPlan::new(&TlsSettings::mode(SslMode::VerifyFull), &file, &[]).unwrap();
        assert_eq!(plan.attempt, TlsAttempt::Never);
        assert_eq!(plan.status(false), TlsStatus::NotApplicable);
    }
}
