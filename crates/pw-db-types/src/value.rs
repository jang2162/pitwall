//! 셀 값 `DbValue`(14-m3-plan.md 결정 M3-D8). 그리드 · 복사 · MCP 가 같은 표현을 쓴다.
//!
//! 와이어는 serde 의 외부 태그다 — `"null"`, `{"int":5}`, `{"int":"9007199254740992"}`, `{"decimal":"0.1"}`,
//! `{"bytes":{"hex":"0xdead","size":2,"truncated":false}}`. JS 의 수는 2^53 을 넘으면 조용히 틀리므로 정수는 **값으로**
//! 가른다 — ±(2^53−1) 안은 수, 밖은 문자열(DB 타입이 `int8` · `BIGINT UNSIGNED` 여도 같다). 드라이버는 값을 직접 만들지 말고
//! 아래 생성 함수(`DbValue::int` · `bytes` · `text` …)를 거친다 — 규칙(정수 경계 · 셀 상한)이 한 곳에 있게.

use pw_rpc::JsNumber;
use serde::{Deserialize, Serialize};

/// JS 에서 정확한 가장 큰 정수(2^53 − 1). 이 범위 밖의 정수는 문자열로 나간다.
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// 셀 하나가 실어 나를 수 있는 최대 크기(바이트). 넘는 값은 앞부분과 원래 크기만 나간다.
///
/// 결과 행은 Host 에 남지 않으므로(14 §3 「셀 상한을 넘는 값 전체 읽기」) 잘린 값의 나머지를 다시 읽는 길은 키로 다시 읽는 것뿐이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellLimit(pub usize);

impl CellLimit {
    /// 기본 셀 상한 — 1 MiB. 설정 페이지(M3-20)가 바꾼다.
    pub const DEFAULT: Self = Self(1024 * 1024);
}

impl Default for CellLimit {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 셀 값.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DbValue {
    Null,
    Bool(bool),
    /// 정수. ±(2^53−1) 안은 수, 밖은 문자열.
    Int(DbInt),
    /// 실수. 유한하면 수, `NaN` · `Infinity` · `-Infinity` 는 문자열(JSON 에 그 수가 없다).
    Float(DbFloat),
    /// decimal · numeric · money. 정밀도를 잃지 않게 늘 문자열.
    Decimal(String),
    Text(String),
    /// 바이트. 늘 `0x` hex, 셀 상한을 넘으면 앞부분만.
    Bytes(DbBytes),
    /// json · jsonb. 서버가 준 글 그대로(파싱하지 않는다).
    Json(String),
    /// 날짜 · 시각 · 타임스탬프. 서버가 준 텍스트 그대로 — timestamptz 는 서버 세션 시간대의 텍스트다.
    Datetime(String),
    Uuid(String),
    Array(Vec<DbValue>),
    /// 셀 상한을 넘은 텍스트 · json · 그 밖 값의 앞부분.
    Truncated(DbTruncated),
    /// 위 어디에도 들지 않는 값: 서버의 텍스트 표현과 DB 타입 이름.
    Other(DbOther),
}

/// 정수 값. 수인지 문자열인지는 값의 크기가 정한다 — [`DbValue::int`] 로 만든다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(untagged)]
pub enum DbInt {
    /// |값| ≤ 2^53 − 1.
    Number(#[specta(type = JsNumber)] i64),
    /// 그 밖. 10진 문자열.
    Text(String),
}

/// 실수 값. 유한하지 않은 값은 그 이름의 문자열이다 — [`DbValue::float`] 로 만든다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(untagged)]
pub enum DbFloat {
    Number(#[specta(type = JsNumber)] f64),
    /// `NaN` · `Infinity` · `-Infinity`.
    Text(String),
}

/// 바이트 값.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbBytes {
    /// `0x` 로 시작하는 소문자 hex. 잘렸으면 앞부분만.
    pub hex: String,
    /// 원래 크기(바이트).
    #[specta(type = JsNumber)]
    pub size: u64,
    pub truncated: bool,
}

/// 셀 상한을 넘어 잘린 값.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbTruncated {
    /// 원래 값의 종류.
    pub of: TruncatedKind,
    /// 앞부분(UTF-8 문자 경계에서 자른다).
    pub prefix: String,
    /// 원래 크기(UTF-8 바이트).
    #[specta(type = JsNumber)]
    pub size: u64,
    /// `of` 가 `other` 일 때의 DB 타입 이름.
    pub type_name: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum TruncatedKind {
    Text,
    Json,
    Other,
}

/// 분류되지 않은 값.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbOther {
    pub text: String,
    /// DB 가 부르는 타입 이름(PG `tsvector`, MySQL `GEOMETRY` 등).
    pub type_name: String,
}

impl DbValue {
    /// 정수. i64 · u64 · i128 모두 이것으로 — 경계 규칙은 값으로만 정한다.
    pub fn int(value: impl Into<i128>) -> Self {
        let value = value.into();
        match i64::try_from(value) {
            Ok(v) if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&v) => {
                Self::Int(DbInt::Number(v))
            }
            _ => Self::Int(DbInt::Text(value.to_string())),
        }
    }

    pub fn float(value: f64) -> Self {
        if value.is_finite() {
            Self::Float(DbFloat::Number(value))
        } else if value.is_nan() {
            Self::Float(DbFloat::Text("NaN".to_owned()))
        } else if value > 0.0 {
            Self::Float(DbFloat::Text("Infinity".to_owned()))
        } else {
            Self::Float(DbFloat::Text("-Infinity".to_owned()))
        }
    }

    pub fn decimal(text: impl Into<String>) -> Self {
        Self::Decimal(text.into())
    }

    /// 바이트. 셀 상한을 넘으면 앞부분 `limit` 바이트만 hex 로.
    pub fn bytes(value: &[u8], limit: CellLimit) -> Self {
        let kept = &value[..value.len().min(limit.0)];
        let mut hex = String::with_capacity(2 + kept.len() * 2);
        hex.push_str("0x");
        for b in kept {
            hex.push(char::from_digit(u32::from(b >> 4), 16).expect("nibble"));
            hex.push(char::from_digit(u32::from(b & 0xf), 16).expect("nibble"));
        }
        Self::Bytes(DbBytes {
            hex,
            size: value.len() as u64,
            truncated: kept.len() < value.len(),
        })
    }

    /// 텍스트. 셀 상한을 넘으면 [`DbValue::Truncated`].
    pub fn text(value: impl Into<String>, limit: CellLimit) -> Self {
        let value = value.into();
        match truncate(&value, limit) {
            None => Self::Text(value),
            Some(prefix) => Self::Truncated(DbTruncated {
                of: TruncatedKind::Text,
                prefix,
                size: value.len() as u64,
                type_name: None,
            }),
        }
    }

    /// json · jsonb 의 글. 셀 상한을 넘으면 [`DbValue::Truncated`].
    pub fn json(value: impl Into<String>, limit: CellLimit) -> Self {
        let value = value.into();
        match truncate(&value, limit) {
            None => Self::Json(value),
            Some(prefix) => Self::Truncated(DbTruncated {
                of: TruncatedKind::Json,
                prefix,
                size: value.len() as u64,
                type_name: None,
            }),
        }
    }

    /// 분류되지 않은 값. 셀 상한을 넘으면 [`DbValue::Truncated`].
    pub fn other(text: impl Into<String>, type_name: impl Into<String>, limit: CellLimit) -> Self {
        let text = text.into();
        let type_name = type_name.into();
        match truncate(&text, limit) {
            None => Self::Other(DbOther { text, type_name }),
            Some(prefix) => Self::Truncated(DbTruncated {
                of: TruncatedKind::Other,
                prefix,
                size: text.len() as u64,
                type_name: Some(type_name),
            }),
        }
    }
}

/// 상한을 넘으면 상한 안의 마지막 문자 경계까지의 앞부분.
fn truncate(value: &str, limit: CellLimit) -> Option<String> {
    if value.len() <= limit.0 {
        return None;
    }
    let mut end = limit.0;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Some(value[..end].to_owned())
}

#[cfg(test)]
mod tests;
