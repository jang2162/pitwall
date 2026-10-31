//! `mysql_async` 오류 → [`DbError`]. 메시지는 서버 · 드라이버의 글 그대로다 — 비밀번호는 거기 담기지 않는다.

use mysql_async::{DriverError, Error, IoError, TlsError};
use pw_db_types::{DbError, DbErrorKind};

/// 이 드라이버가 갈래를 가르는 서버 오류 번호(MySQL · MariaDB 공통).
pub(crate) mod code {
    pub const ACCESS_DENIED_DB: u16 = 1044;
    pub const ACCESS_DENIED: u16 = 1045;
    pub const UNKNOWN_THREAD: u16 = 1094;
    pub const LOCK_WAIT_TIMEOUT: u16 = 1205;
    pub const OPTION_PREVENTS_STATEMENT: u16 = 1290;
    /// 준비 문장(COM_STMT_PREPARE)으로 받지 않는 문장(MySQL 의 `USE` · `CREATE PROCEDURE` 등).
    pub const UNSUPPORTED_PS: u16 = 1295;
    pub const QUERY_INTERRUPTED: u16 = 1317;
    pub const ACCOUNT_LOCKED: u16 = 1698;
    pub const READ_ONLY_TRANSACTION: u16 = 1792;
    pub const READ_ONLY_MODE: u16 = 1836;
    pub const MAX_EXECUTION_TIME: u16 = 3024;
    /// `require_secure_transport` 서버가 평문 연결을 거절했다(MariaDB 도 같은 번호).
    pub const SECURE_TRANSPORT_REQUIRED: u16 = 3159;
}

/// 오류가 난 자리. 입출력 오류의 갈래가 달라진다 — 여는 중이면 「닿지 못했다」, 연 뒤면 「끊겼다」.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Connect,
    Run,
}

pub(crate) fn server_code(error: &Error) -> Option<u16> {
    match error {
        Error::Server(server) => Some(server.code),
        _ => None,
    }
}

/// TLS 핸드셰이크가 낸 rustls 오류. tokio-rustls 는 그것을 `io::Error` 에 싸서 돌려준다.
pub(crate) fn rustls_error(error: &Error) -> Option<&rustls::Error> {
    match error {
        Error::Io(IoError::Tls(TlsError::Tls(tls))) => Some(tls),
        Error::Io(IoError::Io(io)) => io.get_ref()?.downcast_ref::<rustls::Error>(),
        _ => None,
    }
}

pub(crate) fn map(error: Error, phase: Phase) -> DbError {
    if rustls_error(&error).is_some() {
        return DbError::new(DbErrorKind::Tls, error.to_string());
    }
    match error {
        Error::Server(server) => {
            let kind = match server.code {
                code::QUERY_INTERRUPTED => DbErrorKind::Cancelled,
                code::MAX_EXECUTION_TIME => DbErrorKind::Timeout,
                code::LOCK_WAIT_TIMEOUT => DbErrorKind::Busy,
                code::READ_ONLY_TRANSACTION
                | code::OPTION_PREVENTS_STATEMENT
                | code::READ_ONLY_MODE => DbErrorKind::ReadOnly,
                code::ACCESS_DENIED | code::ACCESS_DENIED_DB | code::ACCOUNT_LOCKED
                    if phase == Phase::Connect =>
                {
                    DbErrorKind::Auth
                }
                code::SECURE_TRANSPORT_REQUIRED => DbErrorKind::Tls,
                _ => DbErrorKind::Server,
            };
            DbError::new(kind, server.message).with_code(server.code.to_string())
        }
        Error::Io(IoError::Tls(tls)) => DbError::new(DbErrorKind::Tls, tls.to_string()),
        Error::Io(IoError::Io(io)) => {
            let kind = match phase {
                Phase::Connect => DbErrorKind::Connect,
                Phase::Run => DbErrorKind::Disconnected,
            };
            DbError::new(kind, io.to_string())
        }
        Error::Driver(DriverError::NoClientSslFlagFromServer) => {
            DbError::new(DbErrorKind::Tls, "the server does not support TLS")
        }
        Error::Driver(DriverError::ConnectionClosed | DriverError::PoolDisconnected) => {
            DbError::new(DbErrorKind::Disconnected, "the connection is closed")
        }
        Error::Driver(
            driver @ (DriverError::Params(_)
            | DriverError::MixedParams
            | DriverError::StmtParamsMismatch { .. }
            | DriverError::StmtParamsNumberExceedsLimit { .. }),
        ) => DbError::new(DbErrorKind::Invalid, driver.to_string()),
        Error::Driver(
            driver @ (DriverError::UnknownAuthPlugin { .. }
            | DriverError::CleartextPluginDisabled
            | DriverError::MysqlOldPasswordDisabled),
        ) => DbError::new(DbErrorKind::Auth, driver.to_string()),
        Error::Url(url) => DbError::new(DbErrorKind::Invalid, url.to_string()),
        other => DbError::new(DbErrorKind::Server, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use mysql_async::ServerError;

    use super::*;

    fn server(code: u16) -> Error {
        Error::Server(ServerError {
            code,
            message: format!("error {code}"),
            state: "HY000".into(),
        })
    }

    #[test]
    fn server_codes_pick_the_kind() {
        let table = [
            (code::QUERY_INTERRUPTED, Phase::Run, DbErrorKind::Cancelled),
            (
                code::READ_ONLY_TRANSACTION,
                Phase::Run,
                DbErrorKind::ReadOnly,
            ),
            (code::ACCESS_DENIED, Phase::Connect, DbErrorKind::Auth),
            // 연 뒤의 1045(다른 사용자로 바꾸기 등)는 인증 실패가 아니라 문장의 거절이다.
            (code::ACCESS_DENIED, Phase::Run, DbErrorKind::Server),
            (
                code::SECURE_TRANSPORT_REQUIRED,
                Phase::Connect,
                DbErrorKind::Tls,
            ),
            (1064, Phase::Run, DbErrorKind::Server),
        ];
        for (number, phase, kind) in table {
            let error = map(server(number), phase);
            assert_eq!(
                (error.kind, error.code.as_deref()),
                (kind, Some(number.to_string().as_str()))
            );
        }
    }

    #[test]
    fn io_errors_depend_on_the_phase_and_tls_is_found_inside_io() {
        let io = || Error::Io(IoError::Io(std::io::Error::other("reset")));
        assert_eq!(map(io(), Phase::Connect).kind, DbErrorKind::Connect);
        assert_eq!(map(io(), Phase::Run).kind, DbErrorKind::Disconnected);
        let wrapped = Error::Io(IoError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
        )));
        assert!(rustls_error(&wrapped).is_some());
        assert_eq!(map(wrapped, Phase::Connect).kind, DbErrorKind::Tls);
        let no_tls = Error::Driver(DriverError::NoClientSslFlagFromServer);
        assert_eq!(map(no_tls, Phase::Connect).kind, DbErrorKind::Tls);
    }
}
