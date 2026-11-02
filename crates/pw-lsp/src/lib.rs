//! Pitwall 의 LSP 클라이언트(03 「코어 모듈」 의 `lsp`, 15-m4-plan.md — 결정 M4-D9: 자체 구현 + `lsp-types`).
//!
//! 이 크레이트의 바탕(M4-01): 전송([`transport`] — stdio 의 `Content-Length` 틀), JSON-RPC 메시지([`message`]), 서버 하나의 연결과
//! 수명([`server`] — 요청 표 · 취소 · 시한 · `initialize` · 서버 → 클라이언트 요청 · 동적 등록 · 끝내기), 서버를 띄우는 실행 허가
//! ([`grant`] — 신뢰를 타입으로 증명, 결정 M4-D8), 프로세스 트리([`process`]), 서버 로그([`log`]), 가짜 서버의 시나리오 형식
//! ([`fake`] — 서버는 `src/bin/pw-lsp-fake.rs`), 실 서버 시험 도우미([`live`]), Host 가 강제로 끝난 뒤 남은 서버 그룹 거두기
//! ([`ledger`]).
//!
//! 그 위의 서버 관리자(M4-05): 인스턴스 · 수명 · 문서 동기화 · 요청 길([`manager`]), 문서 텍스트와 UTF-16 위치([`document`]),
//! 작업 공간 루트([`root`]), 서버 런타임 찾기([`requires`]), 진행 → `Task`([`progress`]), 서버 → 클라이언트 요청 처리기의 등록부
//! ([`handlers`]), RPC([`methods`] — [`register`]). 웹뷰 LSP 다리(M4-07)의 Host 쪽은 문서의 기능 칸([`bridge`] —
//! `lsp.document.capabilities`), 문서 심볼([`symbols`] — `lsp.symbols.document`, M4-10), 서버가 등록한 글롭대로 파일 바뀜 전하기([`watched`] — M4-05b). 기능 행(진단 · 내비게이션 · 완성 · 편집 · 포맷 · 심볼)은 자기 모듈을 이
//! 크레이트에 두고 서버에는 [`LspManager::request`] 로만 닿는다(15 「공유 부품의 주인」).
//!
//! 참고 구현은 읽기만 했다 — VS Code `vscode-languageserver-node`(MIT)의 취소 · 진행 · 동기화 규칙과 LSP 3.17 명세. Zed(GPL) ·
//! Helix(MPL)의 코드는 보지 않았다(작업 공간 루트의 「가장 위 마커」 규칙은 Helix 의 동작 설명만 견줬다).

pub mod bridge;
pub mod capabilities;
pub mod document;
pub mod edit;
pub mod fake;
pub mod grant;
pub mod handlers;
pub mod ledger;
pub mod live;
pub mod log;
pub mod manager;
pub mod message;
pub mod methods;
pub mod navigation;
pub mod process;
pub mod progress;
pub mod requires;
pub mod root;
pub mod server;
pub mod symbols;
pub mod transport;
pub mod uri;
pub mod watched;

pub use grant::{LspGrant, lsp_grant};
pub use manager::{
    DocView, FeatureAnswer, FeatureRequest, HostParts, InstanceKey, LspManager, ManagerOptions,
};
pub use process::LaunchSpec;
pub use server::{
    Exit, ExitReason, Handlers, InitializeOptions, LanguageServer, RequestError, RequestOptions,
    ServerEvent, ServerOptions, Stopped,
};

/// 이 크레이트의 RPC 를 붙인다. 기능 행은 자기 모듈의 `register` 를 여기에 한 줄씩 더한다(15 「공유 부품의 주인」).
pub fn register(router: &mut pw_rpc::Router, manager: &LspManager) {
    methods::register(router, manager);
    bridge::register(router, manager);
    edit::register(router, manager);
    navigation::register(router, manager);
    symbols::register(router, manager);
}
