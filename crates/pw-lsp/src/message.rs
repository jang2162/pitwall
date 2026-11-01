//! JSON-RPC 2.0 메시지(LSP 의 바탕): 요청 · 응답 · 알림. 본문 하나를 [`Incoming`] 으로 가르고, 보낼 것을 바이트로 만든다.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 요청 id. 클라이언트가 만드는 id 는 늘 수지만, 서버가 거는 요청은 글일 수 있다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::String(s) => write!(f, "{s:?}"),
        }
    }
}

/// 응답의 `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ResponseError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(code::METHOD_NOT_FOUND, format!("unhandled method {method}"))
    }

    pub fn invalid_params(detail: impl Into<String>) -> Self {
        Self::new(code::INVALID_PARAMS, detail)
    }
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ResponseError {}

/// JSON-RPC · LSP 오류 코드.
pub mod code {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    /// 서버가 `initialize` 전에 요청을 받았다.
    pub const SERVER_NOT_INITIALIZED: i32 = -32002;
    /// 요청이 취소됐다(`$/cancelRequest` 를 받은 서버의 답).
    pub const REQUEST_CANCELLED: i32 = -32800;
    /// 요청을 처리하는 사이 문서가 바뀌었다.
    pub const CONTENT_MODIFIED: i32 = -32801;
    /// 서버가 스스로 취소했다(3.17).
    pub const SERVER_CANCELLED: i32 = -32802;
}

/// 받은 메시지 하나.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// 서버가 거는 요청 — 답해야 한다.
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// 우리 요청의 응답. 서버가 요청을 읽지 못하면 `id` 가 `null` 일 수 있다.
    Response {
        id: Option<RequestId>,
        result: Result<Value, ResponseError>,
    },
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<ResponseError>,
}

/// 본문을 메시지로. `jsonrpc` 판은 보지 않는다(서버가 빠뜨려도 받는다).
pub fn parse(body: &[u8]) -> Result<Incoming, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| format!("not a JSON-RPC message: {e}"))?;
    // serde 는 struct 를 배열에서도 읽는다 — LSP 에는 묶음(batch) 메시지가 없으므로 객체만 받는다.
    if !value.is_object() {
        return Err("a JSON-RPC message must be an object".to_owned());
    }
    let raw: Raw =
        serde_json::from_value(value).map_err(|e| format!("not a JSON-RPC message: {e}"))?;
    let id = match raw.id {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            serde_json::from_value::<RequestId>(value.clone())
                .map_err(|_| format!("bad id {value}"))?,
        ),
    };
    let params = raw.params.unwrap_or(Value::Null);
    match (raw.method, id) {
        (Some(method), Some(id)) => Ok(Incoming::Request { id, method, params }),
        (Some(method), None) => Ok(Incoming::Notification { method, params }),
        (None, id) => {
            let result = match (raw.error, raw.result) {
                (Some(error), _) => Err(error),
                // `result: null` 은 serde 의 `Option` 에서 `None` 이 된다 — 답이 `null` 인 응답이다.
                (None, result) => Ok(result.unwrap_or(Value::Null)),
            };
            Ok(Incoming::Response { id, result })
        }
    }
}

pub fn request(id: &RequestId, method: &str, params: &Value) -> Vec<u8> {
    to_bytes(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
}

pub fn notification(method: &str, params: &Value) -> Vec<u8> {
    to_bytes(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
}

pub fn response(id: &RequestId, result: &Result<Value, ResponseError>) -> Vec<u8> {
    to_bytes(&match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

fn to_bytes(value: &Value) -> Vec<u8> {
    // Value 의 직렬화는 실패하지 않는다(맵 키가 늘 글이다).
    serde_json::to_vec(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_told_apart_by_id_and_method() {
        let req = parse(br#"{"jsonrpc":"2.0","id":"a","method":"m","params":[1]}"#).unwrap();
        assert_eq!(
            req,
            Incoming::Request {
                id: RequestId::String("a".into()),
                method: "m".into(),
                params: json!([1])
            }
        );
        let note = parse(br#"{"jsonrpc":"2.0","method":"n"}"#).unwrap();
        assert_eq!(
            note,
            Incoming::Notification {
                method: "n".into(),
                params: Value::Null
            }
        );
        let ok = parse(br#"{"jsonrpc":"2.0","id":3,"result":null}"#).unwrap();
        assert_eq!(
            ok,
            Incoming::Response {
                id: Some(RequestId::Number(3)),
                result: Ok(Value::Null)
            }
        );
        let err =
            parse(br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"x"}}"#).unwrap();
        assert_eq!(
            err,
            Incoming::Response {
                id: None,
                result: Err(ResponseError::new(code::PARSE_ERROR, "x"))
            }
        );
        assert!(parse(b"[]").is_err());
        assert!(parse(br#"{"id":1.5,"result":1}"#).is_err());
    }

    #[test]
    fn written_messages_parse_back() {
        let id = RequestId::Number(7);
        let back = parse(&request(&id, "x", &json!({"k": 1}))).unwrap();
        assert!(matches!(
            back,
            Incoming::Request {
                id: RequestId::Number(7),
                ..
            }
        ));
        let back = parse(&response(&id, &Err(ResponseError::method_not_found("y")))).unwrap();
        assert!(
            matches!(back, Incoming::Response { result: Err(e), .. } if e.code == code::METHOD_NOT_FOUND)
        );
    }
}
