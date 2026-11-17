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

/// A cell value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DbValue {
    Null,
    Bool(bool),
    /// Integer. A number within ±(2^53−1), a string outside it.
    Int(DbInt),
    /// Floating point. A number when finite; `NaN` · `Infinity` · `-Infinity` are strings (JSON has no such numbers).
    Float(DbFloat),
    /// decimal · numeric · money. Always a string, so no precision is lost.
    Decimal(String),
    Text(String),
    /// Bytes. Always `0x` hex; only the beginning past the cell limit.
    Bytes(DbBytes),
    /// json · jsonb. The text as the server sent it (not parsed).
    Json(String),
    /// Date · time · timestamp. The text as the server sent it — timestamptz is text in the server session's time zone.
    Datetime(String),
    Uuid(String),
    Array(Vec<DbValue>),
    /// The beginning of a text · json · other value past the cell limit.
    Truncated(DbTruncated),
    /// A value that fits none of the above: the server's text form and the DB type name.
    Other(DbOther),
}

/// An integer value. Its size decides whether it is a number or a string — build it with [`DbValue::int`].
///
/// When received it is chosen again by the rule — `{"int": 9007199254740993}` is taken as a string and `{"int": "5"}`
/// as a number (so the same value gets the same shape even when a bind value from the window or an out-of-process
/// driver breaks the rule). Text that is not an integer is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, schemars::JsonSchema)]
#[serde(untagged)]
pub enum DbInt {
    /// |value| ≤ 2^53 − 1.
    Number(#[specta(type = JsNumber)] i64),
    /// Anything else. A decimal string.
    Text(String),
}

impl<'de> Deserialize<'de> for DbInt {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Signed(i64),
            Unsigned(u64),
            Text(String),
        }
        let value: i128 = match Raw::deserialize(d)? {
            Raw::Signed(v) => v.into(),
            Raw::Unsigned(v) => v.into(),
            Raw::Text(t) => t
                .parse()
                .map_err(|_| serde::de::Error::custom(format!("not an integer: {t:?}")))?,
        };
        match DbValue::int(value) {
            DbValue::Int(int) => Ok(int),
            _ => unreachable!("DbValue::int makes an Int"),
        }
    }
}

/// A floating-point value. Non-finite values are strings with their names — build it with [`DbValue::float`]. Any other
/// text is refused when received.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type, schemars::JsonSchema)]
#[serde(untagged)]
pub enum DbFloat {
    Number(#[specta(type = JsNumber)] f64),
    /// `NaN` · `Infinity` · `-Infinity`.
    Text(String),
}

impl<'de> Deserialize<'de> for DbFloat {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(f64),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Number(v) => Ok(Self::Number(v)),
            Raw::Text(t) => match t.as_str() {
                "NaN" | "Infinity" | "-Infinity" => Ok(Self::Text(t)),
                _ => Err(serde::de::Error::custom(format!(
                    "a float is a number or NaN / Infinity / -Infinity, not {t:?}"
                ))),
            },
        }
    }
}

/// A bytes value.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbBytes {
    /// Lowercase hex starting with `0x`. Only the beginning if truncated.
    pub hex: String,
    /// Original size (bytes).
    #[specta(type = JsNumber)]
    pub size: u64,
    pub truncated: bool,
}

/// A value truncated at the cell limit.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbTruncated {
    /// Kind of the original value.
    pub of: TruncatedKind,
    /// The beginning (cut at a UTF-8 character boundary).
    pub prefix: String,
    /// Original size (UTF-8 bytes).
    #[specta(type = JsNumber)]
    pub size: u64,
    /// The DB type name when `of` is `other`.
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

/// A value that fits no other kind.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbOther {
    pub text: String,
    /// The type name the DB uses (PG `tsvector`, MySQL `GEOMETRY`, etc.).
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
