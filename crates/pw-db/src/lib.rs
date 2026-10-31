//! DB 코어(`docs/design/02-tech-stack.md` §2.7, 14-m3-plan.md).
//!
//! 드라이버는 `pw_db_driver` 의 trait 로만 부른다 — 이 크레이트는 드라이버 크레이트(`pw-db-driver-*`)에 의존하지 않는다
//! (결정 M3-D3, `tests/driver_free.rs`). 드라이버는 데스크톱 크레이트가 [`DriverRegistry`] 에 넣는다.
//!
//! - [`registry`]: 드라이버 등록부(M3-01).
//! - [`config`] · [`secret`] · [`trust`]: 연결 설정 파일 · 비밀 · 신뢰 게이트(M3-02).
//! - [`connection`] · [`session`] · [`gate`] · [`execute`]: 연결 관리자 · 세션 · 실행 관문 · 실행 파이프라인(M3-05).
//! - [`schema`]: 스키마 탐색 RPC · 캐시(M3-10).
//! - [`edit`]: 그리드 편집 적용 RPC(M3-12).
//! - [`kv`]: Redis RPC(M3-22).
//!
//! 모듈 배치의 주인은 M3-01 이다. 이후 행은 자기 모듈을 채우고, 메서드 등록 줄을 하나씩 더한다(14 「공유 부품의 주인」).

pub mod config;
pub mod connection;
pub mod edit;
pub mod execute;
pub mod gate;
pub mod kv;
pub mod registry;
pub mod schema;
pub mod secret;
pub mod session;
pub mod trust;

pub use registry::{DriverRegistry, RegisterError};
