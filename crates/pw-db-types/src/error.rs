//! 드라이버 오류. 실행 스트림의 `error` 항목(M3-05)이 그대로 싣는다 — 비밀 값을 메시지에 넣지 않는다.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbError {
    pub kind: DbErrorKind,
    /// 사람이 읽는 글(서버 메시지 그대로일 수 있다).
    pub message: String,
    /// 서버의 오류 코드(PG SQLSTATE `25006`, MySQL 오류 번호 `1792` 등).
    pub code: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbErrorKind {
    /// 서버에 닿지 못했다(이름 풀기 · TCP · 터널).
    Connect,
    /// 인증 실패.
    Auth,
    /// TLS 협상 · 인증서 검증 실패.
    Tls,
    /// 예산(연결 · 응답 · 비활동 타임아웃)을 넘었다.
    Timeout,
    /// 취소됐다(SQLite 의 `interrupted` 포함).
    Cancelled,
    /// 잠금을 기다리다 포기했다(SQLite `busy`).
    Busy,
    /// 읽기 전용 트랜잭션 · 연결에서 쓰기가 거절됐다(PG 25006 · MySQL 1792 · SQLite `readonly`).
    ReadOnly,
    /// 드라이버가 지원하지 않는 기능(지원 플래그가 꺼진 trait 메서드).
    Unsupported,
    /// 서버가 문장을 거절했다(문법 · 제약 · 권한 등). `code` 에 서버 코드.
    Server,
    /// 연결이 끊겼다. 다시 붙는 것은 사람이 누를 때다(결정 M3-D13).
    Disconnected,
    /// 요청이 틀렸다(커서 없이 `fetch`, 알 수 없는 세이브포인트 등).
    Invalid,
}

impl DbError {
    pub fn new(kind: DbErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: None,
        }
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
