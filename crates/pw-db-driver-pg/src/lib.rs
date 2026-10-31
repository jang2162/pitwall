//! PostgreSQL 드라이버(M3-06): `tokio-postgres` + `deadpool-postgres`, TLS 는 `tokio-postgres-rustls` + `rustls-platform-verifier`(결정 M3-D1 · M3-D10).
//!
//! 골격만 있다(M3-01). `pw_db_driver` 의 `DbDriver` 와 갈래 trait 를 구현하고, 데스크톱 크레이트의 등록부
//! (`apps/desktop/src-tauri/src/db.rs`)에 한 줄을 더한다. 실 DB 시험은 `#[cfg(pw_live_pg)]` 를 달고 이름에 `live_pg` 를
//! 넣는다(`pw_db_driver::live` 머리 주석) — CI `db-live` 의 하한(`scripts/db-live.toml`)은 그 행이 올린다.
