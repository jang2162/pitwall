//! JSON-RPC 오류 객체와 오류 코드.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 오류 코드.
///
/// 계약: 모듈(핸들러)이 정의하는 애플리케이션 오류 코드는 `-32768..=-32000` 밖에서 고른다.
/// 그 범위는 JSON-RPC 2.0 과 이 크레이트가 쓴다.
pub mod code {
    /// 본문이 JSON 이 아니다.
    pub const PARSE_ERROR: i32 = -32700;
    /// JSON 이지만 요청 객체가 아니다.
    pub const INVALID_REQUEST: i32 = -32600;
    /// 등록되지 않은 메서드.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// params 가 메서드의 입력 타입으로 역직렬화되지 않는다.
    pub const INVALID_PARAMS: i32 = -32602;
    /// 서버 내부 오류(직렬화 실패, 핸들러 패닉 등).
    pub const INTERNAL_ERROR: i32 = -32603;

    // -32000..=-32099 는 JSON-RPC 2.0 이 「구현 정의 서버 오류」로 남겨 둔 범위다. 프로젝트 전용 코드는 여기서 고른다.

    /// 호출자의 role 이 메서드의 role 메타데이터에 없다.
    pub const PERMISSION_DENIED: i32 = -32001;
    /// 요청이나 스트림이 취소됐다.
    pub const CANCELLED: i32 = -32002;
}

/// JSON-RPC 2.0 오류 객체. 와이어 형식 그대로 직렬화된다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn parse_error(detail: impl Into<String>) -> Self {
        Self::new(code::PARSE_ERROR, "parse error").with_data(Value::String(detail.into()))
    }

    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Self::new(code::INVALID_REQUEST, "invalid request").with_data(Value::String(detail.into()))
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(code::METHOD_NOT_FOUND, "method not found")
            .with_data(Value::String(method.to_owned()))
    }

    pub fn invalid_params(detail: impl Into<String>) -> Self {
        Self::new(code::INVALID_PARAMS, "invalid params").with_data(Value::String(detail.into()))
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(code::INTERNAL_ERROR, "internal error").with_data(Value::String(detail.into()))
    }

    pub fn cancelled() -> Self {
        Self::new(code::CANCELLED, "cancelled")
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}
