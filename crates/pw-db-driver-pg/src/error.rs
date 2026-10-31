//! `tokio-postgres` · `deadpool-postgres` 오류 → [`DbError`]. 서버 오류는 SQLSTATE 를 `code` 에 싣는다.

use pw_db_types::{DbError, DbErrorKind};

pub(crate) fn db_error(error: &tokio_postgres::Error) -> DbError {
    if let Some(db) = error.as_db_error() {
        let code = db.code().code();
        let kind = match code {
            "57014" => DbErrorKind::Cancelled,
            "25006" => DbErrorKind::ReadOnly,
            // 관리자 종료 · 서버 종료 · 연결 예외.
            "57P01" | "57P02" | "57P03" => DbErrorKind::Disconnected,
            // 08P01(프로토콜 위반)은 요청이 틀린 것이고 연결은 산다.
            "08P01" => DbErrorKind::Server,
            c if c.starts_with("08") => DbErrorKind::Disconnected,
            c if c.starts_with("28") => DbErrorKind::Auth,
            _ => DbErrorKind::Server,
        };
        let mut message = db.message().to_owned();
        if let Some(detail) = db.detail() {
            message.push_str("\nDETAIL: ");
            message.push_str(detail);
        }
        if let Some(hint) = db.hint() {
            message.push_str("\nHINT: ");
            message.push_str(hint);
        }
        return DbError::new(kind, message).with_code(code);
    }
    if error.is_closed() {
        return DbError::new(DbErrorKind::Disconnected, "the connection is closed");
    }
    // 그 밖의 종류는 공개 API 가 없어 글의 머리로 가른다(`tokio_postgres::error::Kind` 의 Display).
    let message = error.to_string();
    let kind = if message.starts_with("error performing TLS handshake") {
        DbErrorKind::Tls
    } else if message.starts_with("timeout") {
        DbErrorKind::Timeout
    } else if message.starts_with("error connecting to server") {
        DbErrorKind::Connect
    } else if message.starts_with("error communicating with the server") {
        DbErrorKind::Disconnected
    } else if message.starts_with("authentication error") {
        DbErrorKind::Auth
    } else if message.starts_with("invalid configuration") {
        DbErrorKind::Invalid
    } else {
        DbErrorKind::Server
    };
    DbError::new(kind, message)
}

pub(crate) fn pool_error(error: deadpool_postgres::PoolError) -> DbError {
    use deadpool_postgres::PoolError;
    match error {
        PoolError::Backend(e) => db_error(&e),
        PoolError::Timeout(_) => DbError::timeout("connect"),
        other => DbError::new(DbErrorKind::Connect, other.to_string()),
    }
}

/// 연결을 쥔 태스크가 사라졌다(연결이 끊겼거나 닫혔다).
pub(crate) fn gone() -> DbError {
    DbError::new(DbErrorKind::Disconnected, "the connection is closed")
}

pub(crate) fn invalid(message: impl Into<String>) -> DbError {
    DbError::new(DbErrorKind::Invalid, message)
}
