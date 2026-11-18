//! Pitwall 의 DAP 클라이언트(03 「코어 모듈」 의 `dap`, 17-m6-plan.md M6-15 — 메인 판정 M6-P7: 틀은 `pw-lsp` 와 공용, 메시지
//! 타입은 직접 정의).
//!
//! - 틀: `pw_wire`(`Content-Length` 머리 + JSON 본문 — LSP 와 같은 상한 · 같은 「넘으면 그 프로세스를 죽인다」 규칙).
//! - 메시지([`protocol`]): 요청 · 응답 · 이벤트, 쓰는 명령 · 이벤트 · 역요청의 타입. 규격 판은 그 머리 주석.
//! - 어댑터([`adapter`]): 띄울 것([`AdapterSpec`])과 붙는 길([`Transport`] — stdio 먼저, 그다음 소켓 경로(0700 폴더의 unix 소켓 ·
//!   named pipe — 상대가 어댑터인지 본다), TCP 는 `127.0.0.1` 에만).
//! - 프로세스: 어댑터를 자기 프로세스 그룹(Windows 는 Job Object)으로 띄우고 트리째 끝낸다(`pw-lsp` 와 같은 규칙).
//! - 연결: 요청 표 · 시한 · 틀 위반이면 어댑터를 죽인다 · 쓰기 큐 상한.
//! - 세션([`session`]): 시작 순서(`initialized` 전에는 브레이크포인트를 보내지 않는다), 역요청 `runInTerminal`(허가 판정을 지나
//!   실행 관리자로 — [`grant`] · [`terminal`]) · `startDebugging`(자식 세션 — 세션 나무), 끝과 이벤트.
//! - 가짜 어댑터의 시나리오 형식([`fake`] — 어댑터는 `src/bin/pw-dap-fake.rs`).
//!
//! Host 에 잇는 것(`debug.*` RPC · 브레이크포인트 저장 · `RunKind` 갈래 · 실행 관리자에 `runInTerminal` 잇기)은 M6-17, 어댑터 정의 ·
//! 해석 · 설치는 M6-16 이다.

pub mod adapter;
mod connection;
pub mod fake;
pub mod grant;
mod process;
pub mod protocol;
pub mod session;
mod socket;
pub mod terminal;

pub use adapter::{AdapterSpec, LOOPBACK, LaunchError, Transport};
pub use connection::{MAX_QUEUED_BYTES, RequestError};
pub use grant::{DebugGrant, debug_grant};
pub use process::MAX_LOG_LINE;
pub use session::{
    Configured, DebugSession, EndReason, Ended, InitialBreakpoints, Session, SessionEvent,
    SessionEventKind, SessionId, SessionOptions, StartError, StartRequest, Timeouts,
};
pub use terminal::{NoTerminals, TerminalRun, TerminalStarted, Terminals};
