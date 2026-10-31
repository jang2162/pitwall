//! DB 드라이버 계약(14-m3-plan.md M3-01, 결정 M3-D3).
//!
//! - [`driver`]: 공통 trait [`DbDriver`](연결 · 연결 테스트 · `capabilities()`)와 그 위의 갈래 둘 — [`SqlConnection`] ·
//!   [`KvConnection`] — 과 실행을 서버에서 멈추는 [`Interrupt`].
//! - [`budget`]: 실행 예산(연결 · 취소 정리 · 「행이 올 때마다 다시 감는」 비활동 타임아웃).
//! - [`cancel`]: 실행 하나의 취소 — 토큰과 그 연결의 인터럽트(한 번만 부른다).
//! - [`tls`]: `sslmode` 를 libpq 뜻대로 푼 TLS 계획(결정 M3-D10). TLS 라이브러리는 드라이버 크레이트가 고른다.
//! - [`tunnel`]: SSH 터널 — 시스템 `ssh -N -L` 의 argv · 로컬 포트 · 준비 확인 · 함께 쓰기 · 프로세스 그룹 수명(M3-11, 결정 M3-D11).
//! - `fake`(시험 · `test-support` 기능): 가짜 드라이버 — 실행 파이프라인 · 그리드 시험이 실 DB 없이 쓴다.
//! - `live`(시험 · `test-support` 기능): 실 DB 시험의 URL 과 준비 확인(`std::net` 만).
//!
//! trait 의 입출력은 모두 `pw-db-types` 의 serde 타입이다. 코어(`pw-db`)는 이 trait 로만 드라이버를 부르고 드라이버 크레이트에
//! 의존하지 않는다 — 드라이버는 데스크톱 크레이트가 등록부에 넣는다.
//!
//! **trait 에 메서드를 더할 때**: 드라이버 행들이 함께 구현하므로 더하는 메서드는 기본 구현(「지원하지 않음」)을 두고, 가짜
//! 드라이버와 그 시험을 함께 고친다(14 「공유 부품의 주인」). 지원 플래그를 더하면 `pw_db_types::Capabilities` 의 주석에
//! 그 플래그가 주는 갈래 · 기능을 적는다.

pub mod budget;
pub mod cancel;
pub mod driver;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
#[cfg(any(test, feature = "test-support"))]
pub mod live;
pub mod tls;
pub mod tunnel;

pub use budget::Budget;
pub use cancel::QueryCancel;
pub use driver::{
    BoxFuture, Connected, DbConnection, DbDriver, DbResult, Interrupt, KvConnection, SqlConnection,
};
pub use tls::TlsPlan;
