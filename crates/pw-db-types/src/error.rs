//! 드라이버 오류. 실행 스트림의 `error` 항목(M3-05)이 그대로 싣는다 — 비밀 값을 메시지에 넣지 않는다.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbError {
    pub kind: DbErrorKind,
    /// Human-readable text (may be the server's message as is).
    pub message: String,
    /// The server's error code (PG SQLSTATE `25006`, MySQL error number `1792`, etc.).
    pub code: Option<String>,
    /// True only when the server or driver rolled back the whole transaction — the pipeline (M3-05) sets the session
    /// state to idle and tells the person (e.g. a write inside a manual SQLite transaction was cancelled). Missing
    /// means false.
    #[serde(default)]
    pub rolled_back: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbErrorKind {
    /// Could not reach the server (name resolution · TCP · tunnel).
    Connect,
    /// Authentication failed.
    Auth,
    /// TLS negotiation or certificate verification failed.
    Tls,
    /// A budget (connect · response · idle timeout) was exceeded.
    Timeout,
    /// Cancelled (including SQLite's `interrupted`).
    Cancelled,
    /// Gave up waiting for a lock (SQLite `busy`).
    Busy,
    /// A write was refused on a read-only transaction or connection (PG 25006 · MySQL 1792 · SQLite `readonly`).
    ReadOnly,
    /// A feature the driver does not support (a trait method whose capability flag is off).
    Unsupported,
    /// The server refused the statement (syntax · constraint · permission, etc.). `code` holds the server's code.
    Server,
    /// The connection was lost. It reconnects only when the person asks for it (decision M3-D13).
    Disconnected,
    /// The request was wrong (`fetch` without a cursor, an unknown savepoint, etc.).
    Invalid,
}

impl DbError {
    pub fn new(kind: DbErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: None,
            rolled_back: false,
        }
    }

    /// 이 오류와 함께 트랜잭션 전체가 되돌려졌다.
    pub fn with_rolled_back(mut self) -> Self {
        self.rolled_back = true;
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn unsupported(what: &str) -> Self {
        Self::new(
            DbErrorKind::Unsupported,
            format!("{what} is not supported by this driver"),
        )
    }

    pub fn cancelled() -> Self {
        Self::new(DbErrorKind::Cancelled, "cancelled")
    }

    pub fn timeout(what: &str) -> Self {
        Self::new(DbErrorKind::Timeout, format!("{what} timed out"))
    }
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.code {
            Some(code) => write!(f, "{} ({code})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for DbError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolled_back_is_camel_case_and_defaults_to_false() {
        let error = DbError::new(DbErrorKind::Cancelled, "interrupted").with_rolled_back();
        assert_eq!(serde_json::to_value(&error).unwrap()["rolledBack"], true);
        let old: DbError =
            serde_json::from_str(r#"{"kind":"server","message":"x","code":null}"#).unwrap();
        assert!(!old.rolled_back);
    }
}
