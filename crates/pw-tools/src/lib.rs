//! 도구(언어 서버 · 포매터 · 디버거) — Mason registry 데이터와 설치(07-extensibility.md §4.4, D29).
//!
//! 이 크레이트의 데이터 쪽(15-m4-plan.md M4-03 · 결정 M4-D16 · D17):
//! - [`pinned`]: `registry.toml` — Pitwall 판이 고정한 registry 판(산출물 해시까지)과 기본 번들 패키지의 (판, 대상, 파일)별 SHA-256.
//! - [`index`]: 읽은 registry 한 판과 바이너리에 묶인 내장 스냅숏(기본 번들 패키지의 정의).
//! - [`package`]: 패키지 정의 파싱(미신뢰 — 상한 · 모르는 키는 「지원하지 않음」)과 대상별 풀기.
//! - [`purl`] · [`expr`] · [`target`]: `source.id` · `{{ … }}` 표현식 · 설치 대상.
//! - [`overrides`]: `overrides/<패키지>.toml` — registry 정의 위에 Pitwall 이 받을 판 · 주소 · `bin` 을 덮어 고정하는 틀.
//! - [`source`] · [`fetch`]: 산출물 받기(HTTPS 만 · 상한) · 해시 확인 · `$CACHE/registry/<판>/` 캐시 · 「최신」 모드.
//! - [`registry`] · [`methods`]: Host 의 상태와 `tools.registry.*` RPC.
//!
//! 설치기(`$DATA/tools`, 설치 기록 · `ToolGrant`)는 M4-04 가 이 크레이트에 더한다 — 데이터 모델은 읽기만 한다.

pub mod expr;
pub mod fetch;
pub mod index;
pub mod methods;
pub mod overrides;
pub mod package;
pub mod pinned;
pub mod purl;
pub mod registry;
pub mod source;
pub mod target;

#[cfg(test)]
mod test_http;

pub use index::{Index, RegistryOrigin};
pub use overrides::{Override, Overrides};
pub use package::{Package, ResolvedTool, ToolUnsupported};
pub use pinned::Pinned;
pub use registry::ToolsRegistry;
pub use source::RegistryMode;
pub use target::ToolTarget;
