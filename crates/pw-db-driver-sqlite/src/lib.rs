//! SQLite 드라이버(14-m3-plan.md M3-08): `rusqlite` 0.40(`bundled`, 로컬 저장소와 같은 판 — 결정 M3-D2), 메인 Host 안에서 연결마다
//! 블로킹 스레드 하나(결정 M3-D4).
//!
//! - [`connection`]: 연결 스레드와 그 앞의 [`SqlConnection`](pw_db_driver::SqlConnection) 구현. async 쪽은 채널로만 말하고
//!   tokio 워커를 잡지 않는다. 취소는 스레드 밖으로 미리 꺼내 둔 `InterruptHandle` 이다.
//! - [`guard`]: 여는 플래그 · 늘 끄는 것(확장 로드 · `trusted_schema`, 결정 M3-D13) · `ATTACH` · `VACUUM INTO` 대상의 루트 판정.
//! - [`meta`]: 메타데이터(`sqlite_schema` · `pragma_table_info` · `pragma_index_list` · `pragma_foreign_key_list`)와 `EXPLAIN
//!   QUERY PLAN`.
//! - [`value`]: 셀 값(INTEGER 는 정수 규칙 · REAL · TEXT · BLOB 은 hex)과 바인드 값.
//! - [`error`]: rusqlite 오류 → [`DbError`](pw_db_types::DbError).
//! - `fixture`(시험 · `test-support` 기능): 픽스처 스키마를 넣은 임시 파일 · 인메모리 DB.
//!
//! SQLite 시험은 실 DB 표지 없이 세 OS 에서 돈다.

mod connection;
mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod fixture;
mod guard;
mod meta;
mod value;

use std::time::Duration;

use pw_db_driver::budget::within;
use pw_db_driver::{BoxFuture, Connected, DbDriver, DbResult};
use pw_db_types::{Capabilities, ConnectionSpec, DriverKind, SqlDialect};

/// SQLite 드라이버. 상태가 없다 — 연결마다 스레드 하나가 그 연결을 쥔다.
#[derive(Debug, Default, Clone, Copy)]
pub struct SqliteDriver;

impl SqliteDriver {
    pub fn new() -> Self {
        Self
    }
}

impl DbDriver for SqliteDriver {
    fn id(&self) -> &str {
        DriverKind::Sqlite.id()
    }

    fn dialect(&self) -> Option<SqlDialect> {
        Some(SqlDialect::Sqlite)
    }

    /// 스키마 단계 · DB 고르기는 없다(붙인 DB 는 객체 질의의 `database` 로 고른다). 파일 DB 라 `sslmode` 도 없다.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            sql: true,
            kv: false,
            server_cursor: true,
            cancel: true,
            transactions: true,
            savepoints: true,
            read_only_transaction: true,
            explain: true,
            metadata: true,
            schemas: false,
            multiple_databases: false,
            ssl_modes: Vec::new(),
        }
    }

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        Box::pin(async move {
            let limit = Duration::from_millis(spec.connect_timeout_ms.into());
            within(limit, "connect", connection::open(spec)).await
        })
    }
}

#[cfg(test)]
mod tests;
