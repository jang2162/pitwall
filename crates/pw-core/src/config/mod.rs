//! 설정 계층 — `docs/design/03-architecture.md` 「5. 데이터 저장」(파일 위치 · 범위 · 병합 · 스키마 버전).
//!
//! - [`paths`]: `$CONFIG`/`$DATA`/`$STATE`/`$CACHE` 해석(XDG, `PITWALL_*_DIR`, Windows).
//! - [`ConfigSource`]: 어디서 읽는가(저장소 `.pitwall/` · 개인 전용 `$CONFIG/projects/<slug>/` · 전역 `$CONFIG/`)와 읽기.
//! - [`merge`]: 레이어 병합 규칙.
//! - [`migrate`]: `schema` 버전 · 단계별 마이그레이션 · 원본 보관.
//! - [`gitdir`]: 연결 워크트리 → 메인 체크아웃(`.local.toml` 폴백).
//! - [`ConfigStore`]: 현재 스냅숏 · 다시 읽기 · 파일 감시 · 이벤트.
//! - [`methods`]: 읽기 전용 RPC(`config.paths`, `config.get`).

pub mod gitdir;
pub mod merge;
pub mod methods;
pub mod migrate;
pub mod paths;
mod source;
mod store;

pub use paths::{AppPaths, OsFamily, PathEnv, PathError};
pub use source::{
    ConfigError, ConfigFile, ConfigLayer, ConfigNotice, ConfigSnapshot, ConfigSource, LayerOrigin,
    PROJECT_BACKUP_DIR, PROJECT_DIR,
};
pub use store::{
    ConfigChanged, ConfigFault, ConfigStore, DEFAULT_DEBOUNCE, TOPIC_CHANGED, TOPIC_ERROR,
};
