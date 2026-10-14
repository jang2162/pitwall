//! Pitwall RPC 계약: JSON-RPC 2.0 메시지, 메서드 정의, 스트림(credit), 이벤트 구독, 취소.
//!
//! transport 에 의존하지 않는다. transport(Tauri IPC · stdio · WebSocket)는 받은 프레임을
//! [`Session::handle_text`] 에 넣고, [`Session::new`] 가 돌려준 수신기의 [`Outbound`] 를 직렬화해 보내기만 한다.
//!
//! - 메서드 선언: [`rpc_method!`] → [`MethodMeta`](이름 · 입출력 타입 · 권한 등급 · role)
//! - 핸들러 등록 · 열거: [`Router`]
//! - 권한 검사: 호출자의 [`Role`] 이 메서드의 [`RoleSet`] 에 있어야 한다([`MethodMeta::permits`])
//! - 스트림 · 취소 · 구독의 와이어 형식: [`protocol`]

mod error;
mod events;
mod message;
mod method;
pub mod protocol;
mod router;
mod session;

pub use error::{RpcError, code};
pub use events::EventHub;
pub use message::{Id, Notification, Outbound, Response};
pub use method::{
    Method, MethodKind, MethodMeta, RESERVED_PREFIXES, Role, RoleSet, StreamMethod, Tier,
    UnaryMethod,
};
pub use router::{Caller, Ctx, Router, StreamSink};
pub use session::Session;
