//! redis 값 · 오류를 `pw-db-types` 의 값 · 오류로.

use pw_db_types::{CellLimit, DbError, DbErrorKind, DbValue, KvReply, KvReplyPair};
use redis::{ErrorKind, RedisError, Value};

/// 값 하나(벌크 문자열 · 원소)의 상한 — 값 쪽 예산과 같다(쪽 하나 4 MiB, 14-m3-plan.md M3-09).
pub(crate) const VALUE_LIMIT: CellLimit = CellLimit(crate::PAGE_BUDGET);

/// 바이트: UTF-8 이면 텍스트, 아니면 hex 바이트. 상한을 넘으면 앞부분만.
pub(crate) fn bytes_value(bytes: &[u8]) -> DbValue {
    match std::str::from_utf8(bytes) {
        Ok(text) => DbValue::text(text, VALUE_LIMIT),
        Err(_) => DbValue::bytes(bytes, VALUE_LIMIT),
    }
}

/// 명령 응답. 오류 응답(`-ERR …`)은 [`KvReply::Error`] 다 — 콘솔에서는 그것도 응답이다.
pub(crate) fn reply(value: Value) -> KvReply {
    match value {
        Value::Nil => KvReply::Nil,
        Value::Int(i) => KvReply::Int(DbValue::int(i)),
        Value::BulkString(bytes) => KvReply::Bulk(bytes_value(&bytes)),
        Value::Array(items) | Value::Set(items) | Value::Push { data: items, .. } => {
            KvReply::Array(items.into_iter().map(reply).collect())
        }
        Value::SimpleString(text) => KvReply::Status(text),
        Value::Okay => KvReply::Status("OK".to_owned()),
        Value::Map(pairs) => KvReply::Map(
            pairs
                .into_iter()
                .map(|(key, value)| KvReplyPair {
                    key: reply(key),
                    value: reply(value),
                })
                .collect(),
        ),
        Value::Attribute { data, .. } => reply(*data),
        Value::Double(d) => KvReply::Double(d.to_string()),
        Value::Boolean(b) => KvReply::Bool(b),
        Value::VerbatimString { text, .. } => KvReply::Bulk(DbValue::text(text, VALUE_LIMIT)),
        Value::BigNumber(digits) => KvReply::Int(DbValue::Int(pw_db_types::DbInt::Text(
            String::from_utf8_lossy(&digits).into_owned(),
        ))),
        Value::ServerError(error) => {
            KvReply::Error(server_error_text(error.code(), error.details()))
        }
        // redis 1.x 의 `Value` 는 `#[non_exhaustive]` 다 — 새 갈래는 글로 보인다.
        #[allow(unreachable_patterns)]
        other => KvReply::Status(format!("{other:?}")),
    }
}

fn server_error_text(code: &str, details: Option<&str>) -> String {
    match details {
        Some(details) if !details.is_empty() => format!("{code} {details}"),
        _ => code.to_owned(),
    }
}

/// 서버의 오류 응답인가(연결 · 프로토콜 오류가 아니라).
pub(crate) fn is_server_reply(error: &RedisError) -> bool {
    matches!(error.kind(), ErrorKind::Server(_) | ErrorKind::Extension)
}

/// 오류 응답의 글(`ERR …`, `WRONGTYPE …`).
pub(crate) fn server_reply_text(error: &RedisError) -> String {
    server_error_text(error.code().unwrap_or("ERR"), error.detail())
}

/// redis 오류 → 드라이버 오류. 비밀은 오류 글에 없다(redis-rs 는 비밀번호를 글에 싣지 않는다).
pub(crate) fn db_error(error: RedisError) -> DbError {
    if error.kind() == ErrorKind::AuthenticationFailed
        || error
            .code()
            .is_some_and(|c| c == "NOAUTH" || c == "WRONGPASS")
    {
        return DbError::new(DbErrorKind::Auth, error.to_string());
    }
    if error.is_timeout() {
        return DbError::new(DbErrorKind::Timeout, error.to_string());
    }
    if error.is_connection_dropped() || error.is_io_error() || error.is_unrecoverable_error() {
        return DbError::new(DbErrorKind::Disconnected, error.to_string());
    }
    if is_server_reply(&error) {
        let mut db = DbError::new(DbErrorKind::Server, server_reply_text(&error));
        if let Some(code) = error.code() {
            db = db.with_code(code);
        }
        return db;
    }
    DbError::new(DbErrorKind::Server, error.to_string())
}

/// 연결을 버려야 하는 오류인가(끊김 · 프로토콜이 어긋남).
pub(crate) fn breaks_connection(error: &DbError) -> bool {
    matches!(error.kind, DbErrorKind::Disconnected)
}

#[cfg(test)]
mod tests {
    use pw_db_types::DbBytes;

    use super::*;

    #[test]
    fn replies_keep_text_bytes_numbers_and_nesting() {
        let value = Value::Array(vec![
            Value::Okay,
            Value::Int(9_007_199_254_740_993),
            Value::BulkString(b"hello".to_vec()),
            Value::BulkString(vec![0xff, 0x00]),
            Value::Nil,
            Value::Map(vec![(Value::SimpleString("k".into()), Value::Double(1.5))]),
            Value::Boolean(true),
        ]);
        let KvReply::Array(items) = reply(value) else {
            panic!("array")
        };
        assert_eq!(items[0], KvReply::Status("OK".into()));
        // 2^53 밖의 정수는 문자열이다(결정 M3-D8).
        assert_eq!(
            items[1],
            KvReply::Int(DbValue::Int(pw_db_types::DbInt::Text(
                "9007199254740993".into()
            )))
        );
        assert_eq!(items[2], KvReply::Bulk(DbValue::Text("hello".into())));
        assert_eq!(
            items[3],
            KvReply::Bulk(DbValue::Bytes(DbBytes {
                hex: "0xff00".into(),
                size: 2,
                truncated: false
            }))
        );
        assert_eq!(items[4], KvReply::Nil);
        assert_eq!(
            items[5],
            KvReply::Map(vec![KvReplyPair {
                key: KvReply::Status("k".into()),
                value: KvReply::Double("1.5".into())
            }])
        );
        assert_eq!(items[6], KvReply::Bool(true));
    }

    #[test]
    fn big_values_are_cut_at_the_page_budget() {
        let big = vec![b'a'; crate::PAGE_BUDGET + 10];
        let DbValue::Truncated(cut) = bytes_value(&big) else {
            panic!("truncated")
        };
        assert_eq!(cut.size, (crate::PAGE_BUDGET + 10) as u64);
        assert_eq!(cut.prefix.len(), crate::PAGE_BUDGET);
    }
}
