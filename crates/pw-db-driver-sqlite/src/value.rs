//! SQLite 값 ↔ [`DbValue`](결정 M3-D8). SQLite 의 값은 다섯 저장 갈래뿐이다 — INTEGER 는 정수 규칙(±(2^53−1) 밖은 문자열),
//! REAL, TEXT, BLOB(hex), NULL. 열의 선언 타입(`NUMERIC` · `DATETIME` …)은 값을 바꾸지 않는다.

use pw_db_types::{CellLimit, DbError, DbFloat, DbInt, DbValue};
use rusqlite::types::{Value, ValueRef};

use crate::error::invalid;

pub(crate) fn to_db(value: ValueRef<'_>, limit: CellLimit) -> DbValue {
    match value {
        ValueRef::Null => DbValue::Null,
        ValueRef::Integer(v) => DbValue::int(v),
        ValueRef::Real(v) => DbValue::float(v),
        // TEXT 가 UTF-8 이 아닐 수 있다(SQLite 는 검사하지 않는다) — 깨진 바이트는 대체 문자로 보인다.
        ValueRef::Text(bytes) => DbValue::text(String::from_utf8_lossy(bytes), limit),
        ValueRef::Blob(bytes) => DbValue::bytes(bytes, limit),
    }
}

/// 묶음 크기([`FetchLimit::max_bytes`](pw_db_types::FetchLimit))를 재는 값의 대략 크기.
pub(crate) fn size(value: ValueRef<'_>) -> usize {
    match value {
        ValueRef::Null => 1,
        ValueRef::Integer(_) | ValueRef::Real(_) => 8,
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
    }
}

/// 바인드 값. 셀 상한에 잘린 값 · 배열은 SQLite 에 넣을 수 없어 거절한다.
pub(crate) fn to_sqlite(value: &DbValue) -> Result<Value, DbError> {
    Ok(match value {
        DbValue::Null => Value::Null,
        DbValue::Bool(v) => Value::Integer(i64::from(*v)),
        DbValue::Int(DbInt::Number(v)) => Value::Integer(*v),
        DbValue::Int(DbInt::Text(text)) => match text.parse::<i64>() {
            Ok(v) => Value::Integer(v),
            // i64 밖(BIGINT UNSIGNED 등에서 온 값)은 SQLite 정수에 들지 않는다 — 글로 넣고 열 친화도에 맡긴다.
            Err(_) => Value::Text(text.clone()),
        },
        DbValue::Float(DbFloat::Number(v)) => Value::Real(*v),
        DbValue::Float(DbFloat::Text(text)) => Value::Real(match text.as_str() {
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => f64::NAN,
        }),
        DbValue::Decimal(text)
        | DbValue::Text(text)
        | DbValue::Json(text)
        | DbValue::Datetime(text)
        | DbValue::Uuid(text) => Value::Text(text.clone()),
        DbValue::Other(other) => Value::Text(other.text.clone()),
        DbValue::Bytes(bytes) if bytes.truncated => {
            return Err(invalid("a truncated byte value cannot be bound"));
        }
        DbValue::Bytes(bytes) => Value::Blob(
            decode_hex(&bytes.hex).ok_or_else(|| invalid("a byte value must be 0x hex"))?,
        ),
        DbValue::Truncated(_) => return Err(invalid("a truncated value cannot be bound")),
        DbValue::Array(_) => return Err(invalid("SQLite has no array values")),
    })
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let digits = hex.strip_prefix("0x")?.as_bytes();
    if digits.len() % 2 != 0 {
        return None;
    }
    digits
        .chunks(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high << 4 | low).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_follow_the_db_value_rules() {
        let limit = CellLimit(4);
        assert_eq!(to_db(ValueRef::Integer(5), limit), DbValue::int(5));
        assert_eq!(
            to_db(ValueRef::Integer(9_007_199_254_740_993), limit),
            DbValue::Int(DbInt::Text("9007199254740993".into()))
        );
        assert_eq!(to_db(ValueRef::Real(0.5), limit), DbValue::float(0.5));
        assert_eq!(
            to_db(ValueRef::Blob(&[0xde, 0xad, 0xbe, 0xef, 0x01]), limit),
            DbValue::bytes(&[0xde, 0xad, 0xbe, 0xef, 0x01], limit)
        );
        assert_eq!(
            to_db(ValueRef::Text("한글이다".as_bytes()), CellLimit(7)),
            DbValue::text("한글이다", CellLimit(7))
        );
    }

    #[test]
    fn bind_values_round_trip_and_refuse_what_sqlite_cannot_hold() {
        assert_eq!(
            to_sqlite(&DbValue::bytes(&[0, 0xff, 0x10], CellLimit::DEFAULT)).unwrap(),
            Value::Blob(vec![0, 0xff, 0x10])
        );
        assert_eq!(
            to_sqlite(&DbValue::int(-9_007_199_254_740_993_i64)).unwrap(),
            Value::Integer(-9_007_199_254_740_993)
        );
        assert_eq!(
            to_sqlite(&DbValue::int(u64::MAX)).unwrap(),
            Value::Text(u64::MAX.to_string())
        );
        assert_eq!(to_sqlite(&DbValue::Bool(true)).unwrap(), Value::Integer(1));
        assert!(to_sqlite(&DbValue::bytes(&[1, 2, 3], CellLimit(1))).is_err());
        assert!(to_sqlite(&DbValue::Array(vec![])).is_err());
        assert_eq!(decode_hex("0xzz"), None);
        assert_eq!(decode_hex("0xabc"), None);
    }
}
