//! `Host` 추상: 한 머신에서 프로젝트를 다루는 헤드리스 서비스(`docs/design/03-architecture.md` §1).
//!
//! UI · MCP 는 Host 가 로컬인지 원격인지 모른다. 그래서 Host 와 주고받는 것은 **직렬화된 JSON-RPC 메시지뿐**이다 —
//! [`Host::connect`] 가 연결 하나를 열고, 그 위로 메시지 한 건씩([`serde_json::Value`])을 보내고 받는다.
//! in-process [`LocalHost`](crate::LocalHost) 는 pw-rpc 세션을 직접 품고, 원격 Host(Relay)는 같은 메시지를
//! pw-link 위로 실어 나르는 것으로 같은 trait 를 구현한다.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use pw_rpc::Caller;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ProjectId;

pub type HostId = String;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Host: Send + Sync + 'static {
    fn id(&self) -> &str;

    /// `caller` 의 role 로 연결 하나를 연다. 원격 Host 는 여기서 연결 · 인증을 기다린다.
    fn connect(&self, caller: Caller) -> BoxFuture<'_, Result<Connection, HostError>>;
}

/// Host 로 메시지를 보내는 쪽.
///
/// 계약: 버리면 연결이 닫힌다 — Host 는 그 연결의 진행 중 요청 · 스트림을 취소하고 구독을 걷는다.
/// 창 · 클라이언트가 사라질 때 transport 가 이것을 버리는 것으로 정리가 끝나야 한다.
pub trait FrameSink: Send + Sync {
    /// JSON-RPC 메시지 하나. 기다리지 않는다 — 처리 결과는 [`FrameSource`] 로 온다.
    fn send(&self, frame: Value);
}

/// Host 가 보내는 메시지(응답 · 스트림 청크 · 이벤트)를 받는 쪽.
pub trait FrameSource: Send {
    /// 다음 메시지. 연결이 닫히고 남은 메시지를 다 꺼냈으면 `None`.
    fn recv(&mut self) -> BoxFuture<'_, Option<Value>>;
}

/// 열린 연결. 보내는 쪽과 받는 쪽을 따로 들고 다닐 수 있게 나눠 둔다(transport 는 받는 쪽을 펌프 태스크로 옮긴다).
pub struct Connection {
    pub sink: Box<dyn FrameSink>,
    pub source: Box<dyn FrameSource>,
}

/// 연결 대상. 프로젝트로 고르면 [`HostRouter`](crate::HostRouter) 가 그 프로젝트의 Host 를 찾는다.
/// 프로젝트가 없는 창(웰컴 창)은 Host 를 직접 고른다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Target {
    HostId(HostId),
    ProjectId(ProjectId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum HostError {
    /// 라우터에 등록되지 않은 프로젝트.
    UnknownProject(ProjectId),
    /// 라우터에 등록되지 않은 Host.
    UnknownHost(HostId),
    /// 같은 id 의 Host 가 이미 등록돼 있다.
    DuplicateHost(HostId),
    /// 이미 열린 프로젝트를 다시 열었다.
    ProjectAlreadyOpen(ProjectId),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::UnknownProject(id) => write!(f, "unknown project `{id}`"),
            HostError::UnknownHost(id) => write!(f, "unknown host `{id}`"),
            HostError::DuplicateHost(id) => write!(f, "host `{id}` is already registered"),
            HostError::ProjectAlreadyOpen(id) => write!(f, "project `{id}` is already open"),
        }
    }
}

impl std::error::Error for HostError {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn target_wire_format() {
        assert_eq!(
            serde_json::from_value::<Target>(json!({"hostId": "local"})).unwrap(),
            Target::HostId("local".into())
        );
        assert_eq!(
            serde_json::from_value::<Target>(json!({"projectId": "p1"})).unwrap(),
            Target::ProjectId("p1".into())
        );
        assert!(
            serde_json::from_value::<Target>(json!({"hostId": "a", "projectId": "b"})).is_err()
        );
    }

    #[test]
    fn host_error_wire_format() {
        assert_eq!(
            serde_json::to_value(HostError::UnknownProject("p9".into())).unwrap(),
            json!({"kind": "unknownProject", "id": "p9"})
        );
    }
}
