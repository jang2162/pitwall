//! Pitwall MCP 서버(`docs/design/05-mcp.md`).
//!
//! - [`catalog`]: `rpc_method!` 선언 중 `mcp: true` 인 것에서 만든 툴 목록(이름 · 설명 · 등급 · 입출력 스키마).
//! - [`exposure`]: 툴 노출 설정(`[mcp]` 프리셋 · `[mcp.tools]` on/off, 전역 → 프로젝트).
//! - [`policy`]: 권한 등급별 정책(지금은 `read` 만 허용).
//! - [`audit`]: 감사 로그(`$STATE/mcp/audit.db`)와 인자 요약 규칙.
//! - [`server`]: MCP 세션 하나(rmcp). 툴 호출을 Host 의 RPC 로 옮긴다.
//! - [`socket`]: 실행 중인 앱의 로컬 소켓(unix). 위치 · 권한 · 워크트리 격리 · 핸드셰이크.
//! - [`proxy`]: `pitwall mcp` 의 stdio ↔ 소켓 프록시.
//!
//! MCP 메시지(툴 설명 · 오류 문장)는 번역하지 않는다 — 에이전트가 읽는 계약이다.

pub mod audit;
pub mod catalog;
mod client;
pub mod exposure;
pub mod policy;
pub mod proxy;
pub mod server;
#[cfg(unix)]
pub mod socket;

pub use catalog::{Catalog, Tool};
pub use client::{CallError, HostClient};
pub use server::{ServeError, Services, SessionRequest, serve};
