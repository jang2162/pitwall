//! 내장 프로토콜: 취소 · 스트림 · 이벤트 구독의 메서드 이름과 메시지 타입.
//!
//! 선언한 메서드(`rpc_method!`)와 달리 세션이 직접 처리하고, 모든 role 이 쓸 수 있다.
//! 클라이언트 → 서버 제어 메시지(`request.cancel` · `stream.credit` · `stream.cancel`)는 알림으로 보낸다.
//! id 를 붙여 보내면 `null` 로 답한다. 이미 끝난 대상에 대한 제어는 조용히 무시한다 — 완료와 취소의 경합은 정상이다.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::RpcError;
use crate::message::Id;

/// 클라이언트 → 서버(알림). 진행 중인 unary 요청을 취소한다. 취소된 요청은 `CANCELLED` 오류로 응답된다.
pub const REQUEST_CANCEL: &str = "request.cancel";
/// 클라이언트 → 서버(알림). 스트림에 credit 을 더한다. 서버는 받은 credit 만큼만 청크를 보낸다.
pub const STREAM_CREDIT: &str = "stream.credit";
/// 클라이언트 → 서버(알림). 스트림을 취소한다. 서버는 `stream.end`(cancelled) 로 닫는다.
pub const STREAM_CANCEL: &str = "stream.cancel";
/// 서버 → 클라이언트. 청크 하나.
pub const STREAM_CHUNK: &str = "stream.chunk";
/// 서버 → 클라이언트. 스트림의 마지막 메시지. 이후 그 `streamId` 로는 아무것도 오지 않는다.
pub const STREAM_END: &str = "stream.end";
/// 클라이언트 → 서버(요청). 응답은 [`Subscribed`].
pub const EVENTS_SUBSCRIBE: &str = "events.subscribe";
/// 클라이언트 → 서버(요청). 응답은 `null`.
pub const EVENTS_UNSUBSCRIBE: &str = "events.unsubscribe";
/// 서버 → 클라이언트. 구독에 맞는 이벤트 하나.
pub const EVENTS_EVENT: &str = "events.event";

pub type StreamId = u64;
pub type SubscriptionId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequestParams {
    pub id: Id,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCreditParams {
    pub stream_id: StreamId,
    pub credit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCancelParams {
    pub stream_id: StreamId,
}

/// 스트림 메서드 요청의 응답. 초기 credit 은 0 이다 — 클라이언트가 이 id 로 `stream.credit` 을 보내야 청크가 온다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamOpened {
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamChunk {
    pub stream_id: StreamId,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEnd {
    pub stream_id: StreamId,
    #[serde(flatten)]
    pub outcome: StreamOutcome,
}

/// 스트림의 종료 상태. 와이어에서는 `status` 필드로 구분한다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum StreamOutcome {
    /// 핸들러가 정상 종료했다.
    Completed,
    /// 핸들러가 오류를 돌려줬거나 패닉했다.
    Failed { error: RpcError },
    /// 클라이언트가 `stream.cancel` 을 보냈거나 세션이 닫혔다.
    Cancelled,
}

/// `docs/design/03-architecture.md` §4 의 `events.subscribe({ projectId, topics })`.
/// 토픽은 정확히 일치해야 한다(와일드카드 없음).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeParams {
    pub project_id: String,
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscribed {
    pub subscription_id: SubscriptionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeParams {
    pub subscription_id: SubscriptionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventMessage {
    pub subscription_id: SubscriptionId,
    pub project_id: String,
    pub topic: String,
    pub payload: Value,
}
