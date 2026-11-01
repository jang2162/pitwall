//! M3-21 측정 번들의 가짜 postgres 드라이버(cargo feature `perf-fake-db` — 기본은 꺼져 있다, Cargo.toml 「features」).
//!
//! 14-m3-plan.md 의 M3-16 · M3-13 완료 기준은 가짜 드라이버로 release 번들을 잰다: 10만 행 × 10열 결과와 테이블 1만 개 스키마.
//! feature 를 켠 번들이 측정 모드([`crate::perf::MARK_ENV`])이고 [`ENV`] 까지 받았을 때만 [`crate::db::drivers`] 가 실 PG
//! 드라이버 대신 이것을 `postgres` 로 등록한다. 그래서 측정 하네스(`crates/pw-perf`, `scripts/perf-m3.mjs`)가 연결 설정에
//! `driver = "postgres"` 를 적으면 이 가짜로 간다. 응답은 SQL 에 든 글로 고른다:
//!
//! - [`ROWS_TABLE`]: [`ROWS`] 행 × [`COLUMNS`] 열(첫 열 정수, 나머지는 `r{행}c{열}` 글 — 첫 행의 둘째 칸이 `r0c1`), 지연 없음.
//! - [`STREAM_TABLE`]: 같은 행을 `fetch` 마다 [`STREAM_DELAY`] 씩 쉬며 — 스트리밍 중 스크롤을 잴 만큼 길게 받는다.
//! - 객체 목록: 스키마 `public` 의 테이블 [`TABLES`] 개(`table_00000` …). DB 목록은 가짜에 없어 `multiple_databases` 를 끈다
//!   (탐색기 트리가 연결 → 스키마 → 묶음이 된다).
//!
//! 측정하는 쪽은 앱 로그의 [`LOG_MARK`] 줄로 가짜가 등록됐는지 보고, 출시 번들에 이 글이 없는지(바이너리에 [`LOG_MARK`] 가 없다)도
//! 본다(scripts/perf-m3.mjs).

use std::sync::Arc;
use std::time::Duration;

use pw_db_driver::fake::{FakeDriver, FakeResponse, column};
use pw_db_types::{ObjectInfo, ObjectKind, SqlDialect};

/// 가짜를 고르는 변수(값은 보지 않는다). 측정 모드에서만 읽는다.
pub const ENV: &str = "PITWALL_PERF_FAKE_DB";
/// 등록했다는 로그 줄의 글. 측정하는 쪽이 찾는다.
pub const LOG_MARK: &str = "perf-fake-db: fake postgres driver registered";

pub const ROWS: u64 = 100_000;
pub const COLUMNS: usize = 10;
pub const TABLES: usize = 10_000;
pub const ROWS_TABLE: &str = "perf_rows";
pub const STREAM_TABLE: &str = "perf_stream";
pub const STREAM_DELAY: Duration = Duration::from_millis(30);

/// 측정 모드이고 가짜를 달라고 했는가.
pub fn requested() -> bool {
    std::env::var_os(crate::perf::MARK_ENV).is_some() && std::env::var_os(ENV).is_some()
}

/// `postgres` 자리의 가짜.
pub fn driver() -> Arc<FakeDriver> {
    let mut capabilities = FakeDriver::full_capabilities();
    capabilities.multiple_databases = false;
    let driver = FakeDriver::named_with("postgres", SqlDialect::Postgres, capabilities);
    let columns: Vec<_> = (0..COLUMNS)
        .map(|c| {
            if c == 0 {
                column("id", "int8")
            } else {
                column(&format!("c{c}"), "text")
            }
        })
        .collect();
    driver.respond(
        ROWS_TABLE,
        FakeResponse::rows(ROWS).with_columns(columns.clone()),
    );
    driver.respond(
        STREAM_TABLE,
        FakeResponse::rows(ROWS)
            .with_columns(columns)
            .with_delay(STREAM_DELAY),
    );
    driver.set_objects(
        (0..TABLES)
            .map(|i| ObjectInfo {
                name: format!("table_{i:05}"),
                kind: ObjectKind::Table,
                comment: None,
            })
            .collect(),
    );
    tracing::info!(rows = ROWS, tables = TABLES, "{LOG_MARK}");
    Arc::new(driver)
}
