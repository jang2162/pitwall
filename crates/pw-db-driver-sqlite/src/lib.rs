//! SQLite 드라이버(M3-08): `rusqlite` 0.40(`bundled`, 로컬 저장소와 같은 판 — 결정 M3-D2), 메인 Host 안에서 연결마다 블로킹
//! 스레드 하나(결정 M3-D4).
//!
//! 골격과 시험 픽스처만 있다(M3-01). `pw_db_driver` 의 `DbDriver` 와 `SqlConnection` 을 구현하고, 데스크톱 크레이트의 등록부
//! (`apps/desktop/src-tauri/src/db.rs`)에 한 줄을 더한다. SQLite 시험은 실 DB 표지 없이 세 OS 에서 돈다.
//!
//! - `fixture`(시험 · `test-support` 기능): 픽스처 스키마를 넣은 임시 파일 · 인메모리 DB.

#[cfg(any(test, feature = "test-support"))]
pub mod fixture;
