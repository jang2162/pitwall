//! JSON-RPC 2.0 메시지: 들어오는 요청의 검증과 나가는 응답 · 알림의 직렬화.

use serde::de::DeserializeOwned;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::RpcError;

/// 요청 id. JSON-RPC 2.0 은 숫자 · 문자열 · null 을 허용한다.
/// 숫자는 정수(i64)만 받는다 — 분수 id 는 명세가 비권장하고 비교가 불안정하다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
    Null,
}

/// 검증을 통과한 요청 또는 알림. `id` 가 없으면 알림이다.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Incoming {
    pub id: Option<Id>,
    pub method: String,
    /// 생략된 params 는 `null` 이다 — `()` 입력이 그대로 역직렬화된다.
    pub params: Value,
}

/// 들어온 JSON 값 하나를 검증한다.
///
/// - `Ok(None)`: 상대가 보낸 응답 객체. 이 서버는 상대에게 요청을 보내지 않으므로 버린다.
/// - `Err((id, e))`: 요청으로 읽을 수 없다. `id` 를 읽을 수 있었으면 그 id 로, 아니면 null 로 답한다.
pub(crate) fn parse(value: Value) -> Result<Option<Incoming>, (Id, RpcError)> {
    let Value::Object(mut obj) = value else {
        return Err((
            Id::Null,
            RpcError::invalid_request("message must be an object"),
        ));
    };
    let id = match obj.remove("id") {
        None => None,
        Some(raw) => match parse_id(raw) {
            Some(id) => Some(id),
            None => return Err((Id::Null, RpcError::invalid_request("invalid id"))),
        },
    };
    let fail = |msg: &str| {
        Err((
            id.clone().unwrap_or(Id::Null),
            RpcError::invalid_request(msg),
        ))
    };

    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return fail("jsonrpc must be \"2.0\"");
    }
    let method = match obj.remove("method") {
        Some(Value::String(method)) => method,
        Some(_) => return fail("method must be a string"),
        None if obj.contains_key("result") || obj.contains_key("error") => return Ok(None),
        None => return fail("missing method"),
    };
    // 명세는 params 가 있으면 객체나 배열이어야 한다고 하지만, `null` 을 보내는 클라이언트가 흔해 생략과 같게 받는다.
    let params = match obj.remove("params") {
        None | Some(Value::Null) => Value::Null,
        Some(p @ (Value::Object(_) | Value::Array(_))) => p,
        Some(_) => return fail("params must be an object or an array"),
    };
    Ok(Some(Incoming { id, method, params }))
}

fn parse_id(raw: Value) -> Option<Id> {
    match raw {
        Value::Null => Some(Id::Null),
        Value::String(s) => Some(Id::String(s)),
        Value::Number(n) => n.as_i64().map(Id::Number),
        _ => None,
    }
}

/// 요청에 대한 응답.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub id: Id,
    pub outcome: Result<Value, RpcError>,
}

impl Response {
    /// 성공 결과를 타입으로 읽는다. 오류 응답이거나 타입이 맞지 않으면 그 오류를 돌려준다.
    pub fn result_as<T: DeserializeOwned>(&self) -> Result<T, RpcError> {
        match &self.outcome {
            Ok(v) => {
                serde_json::from_value(v.clone()).map_err(|e| RpcError::internal(e.to_string()))
            }
            Err(e) => Err(e.clone()),
        }
    }
}

impl Serialize for Response {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("jsonrpc", "2.0")?;
        map.serialize_entry("id", &self.id)?;
        match &self.outcome {
            Ok(result) => map.serialize_entry("result", result)?,
            Err(error) => map.serialize_entry("error", error)?,
        }
        map.end()
    }
}

/// 서버 → 클라이언트 알림. 이름은 항상 내장 프로토콜 이름이다([`crate::protocol`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub method: &'static str,
    pub params: Value,
}

impl Notification {
    pub fn params_as<T: DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_value(self.params.clone())
    }
}

impl Serialize for Notification {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("jsonrpc", "2.0")?;
        map.serialize_entry("method", self.method)?;
        map.serialize_entry("params", &self.params)?;
        map.end()
    }
}

/// 세션이 transport 로 내보내는 메시지. transport 는 이것을 직렬화해 보내기만 한다.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Outbound {
    Response(Response),
    Notification(Notification),
}

impl Outbound {
    pub fn to_text(&self) -> String {
        // Value 와 문자열 키만 담으므로 직렬화는 실패하지 않는다.
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn into_response(self) -> Option<Response> {
        match self {
            Outbound::Response(r) => Some(r),
            Outbound::Notification(_) => None,
        }
    }

    pub fn into_notification(self) -> Option<Notification> {
        match self {
            Outbound::Notification(n) => Some(n),
            Outbound::Response(_) => None,
        }
    }
}

pub(crate) fn notification<T: Serialize>(method: &'static str, params: &T) -> Outbound {
    Outbound::Notification(Notification {
        method,
        params: serde_json::to_value(params).unwrap_or(Value::Object(Map::new())),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::code;

    fn err_code(value: Value) -> (Id, i32) {
        let (id, e) = parse(value).unwrap_err();
        (id, e.code)
    }

    #[test]
    fn parses_request_and_notification() {
        let req =
            parse(json!({"jsonrpc": "2.0", "id": 7, "method": "git.log", "params": {"a": 1}}))
                .unwrap()
                .unwrap();
        assert_eq!(req.id, Some(Id::Number(7)));
        assert_eq!(req.method, "git.log");
        assert_eq!(req.params, json!({"a": 1}));

        let note = parse(json!({"jsonrpc": "2.0", "method": "x.y"}))
            .unwrap()
            .unwrap();
        assert_eq!(note.id, None);
        assert_eq!(note.params, Value::Null);

        let null_params =
            parse(json!({"jsonrpc": "2.0", "id": 1, "method": "x.y", "params": null}));
        assert_eq!(null_params.unwrap().unwrap().params, Value::Null);
    }

    #[test]
    fn rejects_malformed_requests_with_the_id_when_readable() {
        assert_eq!(
            err_code(json!({"jsonrpc": "1.0", "id": "a", "method": "x.y"})),
            (Id::String("a".into()), code::INVALID_REQUEST)
        );
        assert_eq!(
            err_code(json!({"jsonrpc": "2.0", "id": 1})),
            (Id::Number(1), code::INVALID_REQUEST)
        );
        assert_eq!(
            err_code(json!({"jsonrpc": "2.0", "id": 1, "method": "x.y", "params": 3})),
            (Id::Number(1), code::INVALID_REQUEST)
        );
        assert_eq!(
            err_code(json!({"jsonrpc": "2.0", "id": 1.5, "method": "x.y"})),
            (Id::Null, code::INVALID_REQUEST)
        );
        assert_eq!(err_code(json!([1, 2])), (Id::Null, code::INVALID_REQUEST));
    }

    #[test]
    fn ignores_responses_from_the_peer() {
        assert_eq!(
            parse(json!({"jsonrpc": "2.0", "id": 1, "result": 3})).unwrap(),
            None
        );
    }

    #[test]
    fn serializes_wire_format() {
        let ok = Outbound::Response(Response {
            id: Id::Number(1),
            outcome: Ok(json!({"x": 1})),
        });
        assert_eq!(
            serde_json::from_str::<Value>(&ok.to_text()).unwrap(),
            json!({"jsonrpc": "2.0", "id": 1, "result": {"x": 1}})
        );
        let err = Outbound::Response(Response {
            id: Id::Null,
            outcome: Err(RpcError::cancelled()),
        });
        assert_eq!(
            serde_json::from_str::<Value>(&err.to_text()).unwrap(),
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": code::CANCELLED, "message": "cancelled"}})
        );
    }
}
