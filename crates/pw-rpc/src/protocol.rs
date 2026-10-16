//! 내장 프로토콜: 취소 · 스트림 · 이벤트 구독의 메서드 이름과 메시지 타입.
//!
//! 선언한 메서드(`rpc_method!`)와 달리 세션이 직접 처리하고, 모든 role 이 쓸 수 있다.
//! 클라이언트 → 서버 제어 메시지(`request.cancel` · `stream.credit` · `stream.cancel`)는 알림으로 보낸다.
//! id 를 붙여 보내면 `null` 로 답한다. 이미 끝난 대상에 대한 제어는 조용히 무시한다 — 완료와 취소의 경합은 정상이다.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;

use crate::error::RpcError;
use crate::message::Id;
use crate::typegen::{JsNumber, JsonValue};

/// 클라이언트 → 서버(알림). 진행 중인 **unary** 요청을 취소한다. 취소된 요청은 `CANCELLED` 오류로 응답된다.
/// 스트림은 요청 id 가 아니라 `streamId` 로 취소한다([`STREAM_CANCEL`]).
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

/// 위 이름 전부(상수 이름, 값). 생성기가 TS 상수로 내보낸다 — 이름을 더하면 여기에도 더한다.
pub const NAMES: [(&str, &str); 8] = [
    ("REQUEST_CANCEL", REQUEST_CANCEL),
    ("STREAM_CREDIT", STREAM_CREDIT),
    ("STREAM_CANCEL", STREAM_CANCEL),
    ("STREAM_CHUNK", STREAM_CHUNK),
    ("STREAM_END", STREAM_END),
    ("EVENTS_SUBSCRIBE", EVENTS_SUBSCRIBE),
    ("EVENTS_UNSUBSCRIBE", EVENTS_UNSUBSCRIBE),
    ("EVENTS_EVENT", EVENTS_EVENT),
];

/// 세션 안의 일련번호(1부터). TS 로는 `number` 다([`JsNumber`]).
pub type StreamId = u64;
/// 허브 안의 일련번호. TS 로는 `number` 다([`JsNumber`]).
pub type SubscriptionId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequestParams {
    pub id: Id,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StreamCreditParams {
    #[specta(type = JsNumber)]
    pub stream_id: StreamId,
    pub credit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StreamCancelParams {
    #[specta(type = JsNumber)]
    pub stream_id: StreamId,
}

/// 스트림 메서드 요청의 응답. 초기 credit 은 0 이다 — 클라이언트가 이 id 로 `stream.credit` 을 보내야 청크가 온다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StreamOpened {
    #[specta(type = JsNumber)]
    pub stream_id: StreamId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StreamChunk {
    #[specta(type = JsNumber)]
    pub stream_id: StreamId,
    #[specta(type = JsonValue)]
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StreamEnd {
    #[specta(type = JsNumber)]
    pub stream_id: StreamId,
    #[serde(flatten)]
    pub outcome: StreamOutcome,
}

/// 스트림의 종료 상태. 와이어에서는 `status` 필드로 구분한다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeParams {
    /// 없으면(`null` 이거나 키가 없으면) Host 범위 — 어느 프로젝트에도 속하지 않는 이벤트(전역 설정 변경 등)를 받는다.
    /// 프로젝트 없는 창(웰컴 · 설정)이 이것으로 구독한다. 한 구독은 한 범위만 받는다.
    #[serde(default)]
    pub project_id: Option<String>,
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Subscribed {
    #[specta(type = JsNumber)]
    pub subscription_id: SubscriptionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeParams {
    #[specta(type = JsNumber)]
    pub subscription_id: SubscriptionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EventMessage {
    #[specta(type = JsNumber)]
    pub subscription_id: SubscriptionId,
    /// Host 범위 이벤트면 `null`.
    pub project_id: Option<String>,
    pub topic: String,
    #[specta(type = JsonValue)]
    pub payload: Value,
}
