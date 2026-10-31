//! 키-값 갈래(Redis)의 입출력: 키 스캔, 타입별 값 읽기(쪽 단위), 명령 응답.
//!
//! 드라이버 구현은 M3-09, RPC 는 M3-22. Redis 의 키 · 값은 바이트라 UTF-8 이 아닌 키도 그대로 지우고 이름을 바꿀 수 있게
//! [`KvKey`] 로 다룬다.

use pw_rpc::JsNumber;
use serde::{Deserialize, Serialize};

use crate::value::DbValue;

/// 키. UTF-8 이면 문자열 그대로, 아니면 `{ "hex": "0x…" }`.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(untagged)]
pub enum KvKey {
    Text(String),
    Bytes(KvKeyBytes),
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KvKeyBytes {
    /// `0x` 로 시작하는 소문자 hex.
    pub hex: String,
}

impl KvKey {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self::Text(text.to_owned()),
            Err(_) => {
                let mut hex = String::from("0x");
                for b in bytes {
                    hex.push_str(&format!("{b:02x}"));
                }
                Self::Bytes(KvKeyBytes { hex })
            }
        }
    }

    /// 서버에 보낼 바이트. hex 가 틀렸으면 `None`.
    pub fn to_bytes(&self) -> Option<Vec<u8>> {
        match self {
            Self::Text(text) => Some(text.as_bytes().to_vec()),
            Self::Bytes(KvKeyBytes { hex }) => {
                let digits = hex.strip_prefix("0x")?;
                if digits.len() % 2 != 0 {
                    return None;
                }
                (0..digits.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok())
                    .collect()
            }
        }
    }
}

/// 값의 타입.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum KvType {
    String,
    Hash,
    List,
    Set,
    Zset,
    Stream,
    /// RedisJSON(`ReJSON-RL`).
    Json,
    /// 그 밖의 모듈 타입. `TYPE` 이 돌려준 이름.
    Other(String),
}

/// 스캔 한 번의 요청.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KvScanRequest {
    /// `MATCH` 패턴. 없으면 모든 키.
    pub pattern: Option<String>,
    /// 앞 묶음의 커서. 처음은 `None`.
    pub cursor: Option<String>,
    /// 한 묶음의 키 수(`COUNT` 힌트, 기본 1,000).
    pub count: u32,
}

/// 스캔 한 묶음: 키마다 타입 · TTL · 크기(결정 M3-D15 의 1,000키 파이프라인).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KvScanPage {
    pub keys: Vec<KvKeyInfo>,
    /// 다음 묶음의 커서. `None` 이면 스캔이 끝났다.
    pub next_cursor: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KvKeyInfo {
    pub key: KvKey,
    pub kind: KvType,
    /// 남은 수명(밀리초). 만료가 없으면 `None`.
    #[specta(type = Option<JsNumber>)]
    pub ttl_ms: Option<u64>,
    /// 크기(바이트, `MEMORY USAGE` — 없으면 타입별 길이). 모르면 `None`.
    #[specta(type = Option<JsNumber>)]
    pub size: Option<u64>,
}

/// 값 읽기의 범위. 컬렉션은 쪽 단위로 읽는다(쪽 하나 4 MiB 예산).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KvRange {
    /// 앞 쪽의 [`KvValuePage::next_cursor`]. 처음은 `None`.
    pub cursor: Option<String>,
    /// 한 쪽의 원소 수 상한(string 은 바이트 창의 크기).
    pub count: u32,
}

/// 값 한 쪽.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvValuePage {
    pub value: KvValue,
    /// 전체 길이(원소 수, string 은 바이트). 모르면 `None`.
    #[specta(type = Option<JsNumber>)]
    pub total: Option<u64>,
    pub next_cursor: Option<String>,
}

/// 타입별 값. 원소는 UTF-8 이면 텍스트, 아니면 바이트 [`DbValue`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum KvValue {
    /// 키가 없다(그사이 지워졌거나 만료).
    Missing,
    String(DbValue),
    Hash(Vec<KvField>),
    List(Vec<DbValue>),
    Set(Vec<DbValue>),
    Zset(Vec<KvScored>),
    Stream(Vec<KvStreamEntry>),
    Json(DbValue),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvField {
    pub field: DbValue,
    pub value: DbValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvScored {
    pub member: DbValue,
    /// 점수(Redis 가 준 글 그대로 — `inf` 등).
    pub score: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvStreamEntry {
    pub id: String,
    pub fields: Vec<KvField>,
}

/// 이름 바꾸기의 결과(`RENAMENX` — 결정 M3-D15).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum KvRenameOutcome {
    Renamed,
    /// 대상 키가 이미 있어 바꾸지 않았다 — 덮으려면 「덮어쓰기」 확인을 받고 `overwrite` 로 다시 부른다.
    TargetExists,
}

/// 명령 응답(RESP).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum KvReply {
    Nil,
    /// 상태 응답(`+OK`).
    Status(String),
    /// 오류 응답(`-ERR …`).
    Error(String),
    Int(DbValue),
    Double(String),
    Bool(bool),
    /// 벌크 문자열.
    Bulk(DbValue),
    Array(Vec<KvReply>),
    Map(Vec<KvReplyPair>),
    /// 콘솔 응답 상한(맨 위 원소 10,000 · 4 MiB — 드라이버가 정한다)에서 끊은 응답. 맨 위만 감싼다.
    Truncated(KvReplyTruncated),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvReplyTruncated {
    /// 상한까지의 앞부분(배열 · 맵은 앞 원소들).
    pub reply: Box<KvReply>,
    /// 원래 응답의 맨 위 원소 수(배열 · 맵의 길이, 값 하나면 1).
    #[specta(type = JsNumber)]
    pub elements: u64,
    /// 원래 응답의 값 바이트 합.
    #[specta(type = JsNumber)]
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvReplyPair {
    pub key: KvReply,
    pub value: KvReply,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keys_round_trip_through_text_or_hex() {
        let text = KvKey::from_bytes(b"session:1");
        assert_eq!(serde_json::to_value(&text).unwrap(), json!("session:1"));
        let bytes = KvKey::from_bytes(&[0xff, 0x00, 0x41]);
        assert_eq!(
            serde_json::to_value(&bytes).unwrap(),
            json!({ "hex": "0xff0041" })
        );
        assert_eq!(bytes.to_bytes().unwrap(), vec![0xff, 0x00, 0x41]);
        let back: KvKey = serde_json::from_value(json!({ "hex": "0xff0041" })).unwrap();
        assert_eq!(back, bytes);
        assert_eq!(
            KvKey::Bytes(KvKeyBytes { hex: "0xf".into() }).to_bytes(),
            None
        );
    }
}
