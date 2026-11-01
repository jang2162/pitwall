//! Pitwall 의 LSP 클라이언트(03 「코어 모듈」 의 `lsp`, 15-m4-plan.md — 결정 M4-D9: 자체 구현 + `lsp-types`).
//!
//! 이 크레이트의 바탕(M4-01): 전송([`transport`] — stdio 의 `Content-Length` 틀), JSON-RPC 메시지([`message`]), 서버 하나의 연결과
//! 수명([`server`] — 요청 표 · 취소 · 시한 · `initialize` · 서버 → 클라이언트 요청 · 동적 등록 · 끝내기), 서버를 띄우는 실행 허가
//! ([`grant`] — 신뢰를 타입으로 증명, 결정 M4-D8), 프로세스 트리([`process`]), 서버 로그([`log`]), 가짜 서버의 시나리오 형식
//! ([`fake`] — 서버는 `src/bin/pw-lsp-fake.rs`), 실 서버 시험 도우미([`live`]), Host 가 강제로 끝난 뒤 남은 서버 그룹 거두기
//! ([`ledger`]).
//!
//! 서버 관리자 · 문서 동기화 · 기능 라우팅 · RPC 는 M4-05 가 이 위에 더한다(15 「공유 부품의 주인」). 참고 구현은 읽기만 했다 —
//! VS Code `vscode-languageserver-node`(MIT)의 취소 · 진행 규칙과 LSP 3.17 명세. Zed(GPL) · Helix(MPL)의 코드는 보지 않았다.

pub mod capabilities;
pub mod fake;
pub mod grant;
pub mod ledger;
pub mod live;
pub mod log;
pub mod message;
pub mod process;
pub mod server;
pub mod transport;
pub mod uri;

pub use grant::{LspGrant, lsp_grant};
pub use process::LaunchSpec;
pub use server::{
    Exit, ExitReason, Handlers, InitializeOptions, LanguageServer, RequestError, RequestOptions,
    ServerEvent, ServerOptions, Stopped,
};
