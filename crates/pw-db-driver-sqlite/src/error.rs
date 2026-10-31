//! rusqlite 오류 → [`DbError`]. 코드는 SQLite 의 확장 결과 코드(`5` = `SQLITE_BUSY`, `9` = `SQLITE_INTERRUPT` …)다.

use pw_db_types::{DbError, DbErrorKind};
use rusqlite::ErrorCode;

/// `denial` 은 authorizer 가 거절한 까닭이다 — SQLite 는 `not authorized …` 만 돌려주므로 그 자리에 까닭을 싣는다.
pub(crate) fn map(error: rusqlite::Error, denial: Option<String>) -> DbError {
    let (failure, message) = match error {
        rusqlite::Error::SqliteFailure(failure, message) => (failure, message),
        // 준비 단계의 오류는 rusqlite 가 문장 안 위치와 함께 낸다.
        rusqlite::Error::SqlInputError { error, msg, .. } => (error, Some(msg)),
        error @ (rusqlite::Error::MultipleStatement
        | rusqlite::Error::InvalidParameterCount(..)
        | rusqlite::Error::InvalidParameterName(_)
        | rusqlite::Error::ToSqlConversionFailure(_)
        | rusqlite::Error::InvalidQuery
        | rusqlite::Error::NulError(_)) => {
            return DbError::new(DbErrorKind::Invalid, error.to_string());
        }
        error => return DbError::new(DbErrorKind::Server, error.to_string()),
    };
    let kind = match failure.code {
        ErrorCode::OperationInterrupted => DbErrorKind::Cancelled,
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => DbErrorKind::Busy,
        ErrorCode::ReadOnly => DbErrorKind::ReadOnly,
        ErrorCode::CannotOpen => DbErrorKind::Connect,
        _ => DbErrorKind::Server,
    };
    let message = denial.or(message).unwrap_or_else(|| failure.to_string());
    DbError::new(kind, message).with_code(failure.extended_code.to_string())
}

/// 연결 스레드가 끝났다(닫혔거나 멈췄다).
pub(crate) fn disconnected() -> DbError {
    DbError::new(DbErrorKind::Disconnected, "the SQLite connection is closed")
}

pub(crate) fn invalid(message: impl Into<String>) -> DbError {
    DbError::new(DbErrorKind::Invalid, message)
}
