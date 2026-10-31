//! 결과 열 · 셀 값 · 바인드 값의 변환(결정 M3-D8).
//!
//! 셀은 열의 타입으로 가른다 — 텍스트 프로토콜(바인드 없는 실행)은 모든 값이 글로 오고, 바이너리 프로토콜(바인드가 있는 실행)은
//! 정수 · 실수 · 날짜가 타입이 있는 값으로 온다. 어느 쪽이든 결과는 같다: 정수는 값으로 가르고(`BIGINT UNSIGNED` 도 ±2^53−1 밖이면
//! 문자열), `DECIMAL` 은 문자열, 이진 문자열(`BLOB` · `BINARY` · 문자 집합 `binary`)은 hex, `JSON` 은 글 그대로, 날짜 · 시각은
//! 서버의 텍스트(`TIMESTAMP` 는 세션 시간대). MariaDB 의 `JSON` 은 `LONGTEXT` 의 별칭이라 열 타입으로 가를 수 없어 텍스트다.

use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::{Column, Value};
use pw_db_types::{CellLimit, ColumnMeta, DbError, DbErrorKind, DbFloat, DbInt, DbValue};

/// 문자 집합 `binary` 의 번호 — 이 문자 집합의 문자열 열은 바이트다.
const BINARY_CHARSET: u16 = 63;

pub(crate) fn column_meta(column: &Column) -> ColumnMeta {
    ColumnMeta {
        name: column.name_str().into_owned(),
        db_type: type_name(column),
        nullable: Some(!column.flags().contains(ColumnFlags::NOT_NULL_FLAG)),
    }
}

/// 열의 타입 이름(`BIGINT UNSIGNED` · `DECIMAL` · `BLOB` · `JSON` …).
pub(crate) fn type_name(column: &Column) -> String {
    use ColumnType::*;
    let flags = column.flags();
    let binary = is_binary(column);
    let base = match column.column_type() {
        MYSQL_TYPE_TINY => "TINYINT",
        MYSQL_TYPE_SHORT => "SMALLINT",
        MYSQL_TYPE_INT24 => "MEDIUMINT",
        MYSQL_TYPE_LONG => "INT",
        MYSQL_TYPE_LONGLONG => "BIGINT",
        MYSQL_TYPE_YEAR => "YEAR",
        MYSQL_TYPE_FLOAT => "FLOAT",
        MYSQL_TYPE_DOUBLE => "DOUBLE",
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "DECIMAL",
        MYSQL_TYPE_BIT => "BIT",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "DATE",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "TIME",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "DATETIME",
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "TIMESTAMP",
        MYSQL_TYPE_JSON => "JSON",
        MYSQL_TYPE_GEOMETRY => "GEOMETRY",
        MYSQL_TYPE_VECTOR => "VECTOR",
        MYSQL_TYPE_NULL => "NULL",
        _ if flags.contains(ColumnFlags::ENUM_FLAG) => "ENUM",
        _ if flags.contains(ColumnFlags::SET_FLAG) => "SET",
        MYSQL_TYPE_ENUM => "ENUM",
        MYSQL_TYPE_SET => "SET",
        MYSQL_TYPE_TINY_BLOB | MYSQL_TYPE_MEDIUM_BLOB | MYSQL_TYPE_LONG_BLOB | MYSQL_TYPE_BLOB => {
            if binary {
                "BLOB"
            } else {
                "TEXT"
            }
        }
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING => {
            if binary {
                "VARBINARY"
            } else {
                "VARCHAR"
            }
        }
        MYSQL_TYPE_STRING => {
            if binary {
                "BINARY"
            } else {
                "CHAR"
            }
        }
        _ => "UNKNOWN",
    };
    let numeric = matches!(
        column.column_type(),
        MYSQL_TYPE_TINY
            | MYSQL_TYPE_SHORT
            | MYSQL_TYPE_INT24
            | MYSQL_TYPE_LONG
            | MYSQL_TYPE_LONGLONG
            | MYSQL_TYPE_FLOAT
            | MYSQL_TYPE_DOUBLE
            | MYSQL_TYPE_DECIMAL
            | MYSQL_TYPE_NEWDECIMAL
    );
    if numeric && flags.contains(ColumnFlags::UNSIGNED_FLAG) {
        format!("{base} UNSIGNED")
    } else {
        base.to_owned()
    }
}

fn is_binary(column: &Column) -> bool {
    column.character_set() == BINARY_CHARSET
}

/// 셀 하나. `value` 는 텍스트 · 바이너리 프로토콜 어느 쪽의 값이든 된다.
pub(crate) fn cell(column: &Column, value: Value, limit: CellLimit) -> DbValue {
    use ColumnType::*;
    let bytes = match value {
        Value::NULL => return DbValue::Null,
        Value::Int(v) => return DbValue::int(v),
        Value::UInt(v) => return DbValue::int(v),
        Value::Float(v) => return DbValue::float(f64::from(v)),
        Value::Double(v) => return DbValue::float(v),
        Value::Date(..) | Value::Time(..) => {
            return DbValue::Datetime(datetime_text(column, &value));
        }
        Value::Bytes(bytes) => bytes,
    };
    match column.column_type() {
        MYSQL_TYPE_NULL => DbValue::Null,
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONG
        | MYSQL_TYPE_LONGLONG | MYSQL_TYPE_YEAR => {
            match utf8(&bytes).and_then(|t| t.parse::<i128>().ok()) {
                Some(v) => DbValue::int(v),
                None => other(column, &bytes, limit),
            }
        }
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => {
            match utf8(&bytes).and_then(|t| t.parse::<f64>().ok()) {
                Some(v) => DbValue::float(v),
                None => other(column, &bytes, limit),
            }
        }
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => match utf8(&bytes) {
            Some(text) => DbValue::decimal(text),
            None => other(column, &bytes, limit),
        },
        MYSQL_TYPE_DATE
        | MYSQL_TYPE_NEWDATE
        | MYSQL_TYPE_TIME
        | MYSQL_TYPE_TIME2
        | MYSQL_TYPE_DATETIME
        | MYSQL_TYPE_DATETIME2
        | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2 => match utf8(&bytes) {
            Some(text) => DbValue::Datetime(text.to_owned()),
            None => other(column, &bytes, limit),
        },
        MYSQL_TYPE_JSON => match String::from_utf8(bytes) {
            Ok(text) => DbValue::json(text, limit),
            Err(e) => DbValue::bytes(e.as_bytes(), limit),
        },
        MYSQL_TYPE_BIT | MYSQL_TYPE_GEOMETRY | MYSQL_TYPE_VECTOR => DbValue::bytes(&bytes, limit),
        _ if is_binary(column) => DbValue::bytes(&bytes, limit),
        _ => match String::from_utf8(bytes) {
            Ok(text) => DbValue::text(text, limit),
            // 문자 집합이 글이라는데 UTF-8 이 아니면(옛 latin1 자료 등) 바이트로 보인다 — 글자를 조용히 바꾸지 않는다.
            Err(e) => DbValue::bytes(e.as_bytes(), limit),
        },
    }
}

fn utf8(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok()
}

fn other(column: &Column, bytes: &[u8], limit: CellLimit) -> DbValue {
    DbValue::other(String::from_utf8_lossy(bytes), type_name(column), limit)
}

/// 바이너리 프로토콜의 날짜 · 시각을 서버의 텍스트 꼴로(소수 자릿수는 열의 `decimals`).
fn datetime_text(column: &Column, value: &Value) -> String {
    let digits = usize::from(column.decimals().min(6));
    let fraction = |micros: u32| {
        if digits == 0 {
            String::new()
        } else {
            format!(".{:06}", micros)[..=digits].to_owned()
        }
    };
    match *value {
        Value::Date(y, mo, d, h, mi, s, us) => match column.column_type() {
            ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => {
                format!("{y:04}-{mo:02}-{d:02}")
            }
            _ => format!(
                "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}{}",
                fraction(us)
            ),
        },
        Value::Time(negative, days, h, mi, s, us) => {
            let hours = days * 24 + u32::from(h);
            let sign = if negative { "-" } else { "" };
            format!("{sign}{hours:02}:{mi:02}:{s:02}{}", fraction(us))
        }
        _ => String::new(),
    }
}

/// 셀 하나의 대략 크기(바이트) — `FetchLimit::max_bytes` 를 센다.
pub(crate) fn approx_size(value: &DbValue) -> usize {
    match value {
        DbValue::Null | DbValue::Bool(_) => 1,
        DbValue::Int(DbInt::Number(_)) | DbValue::Float(DbFloat::Number(_)) => 8,
        DbValue::Int(DbInt::Text(t)) | DbValue::Float(DbFloat::Text(t)) => t.len(),
        DbValue::Decimal(t)
        | DbValue::Text(t)
        | DbValue::Json(t)
        | DbValue::Datetime(t)
        | DbValue::Uuid(t) => t.len(),
        DbValue::Bytes(b) => b.hex.len(),
        DbValue::Array(items) => items.iter().map(approx_size).sum(),
        DbValue::Truncated(t) => t.prefix.len(),
        DbValue::Other(o) => o.text.len(),
    }
}

/// 바인드 값(그리드 편집의 DML — M3-12). 서버가 열 타입으로 바꾸도록 대개 글로 넘긴다.
pub(crate) fn param(value: &DbValue) -> Result<Value, DbError> {
    let invalid = |what: &str| {
        DbError::new(
            DbErrorKind::Invalid,
            format!("{what} cannot be bound as a MySQL parameter"),
        )
    };
    Ok(match value {
        DbValue::Null => Value::NULL,
        DbValue::Bool(b) => Value::Int(i64::from(*b)),
        DbValue::Int(DbInt::Number(v)) => Value::Int(*v),
        DbValue::Int(DbInt::Text(t)) => match (t.parse::<i64>(), t.parse::<u64>()) {
            (Ok(v), _) => Value::Int(v),
            (_, Ok(v)) => Value::UInt(v),
            _ => Value::Bytes(t.clone().into_bytes()),
        },
        DbValue::Float(DbFloat::Number(v)) => Value::Double(*v),
        DbValue::Float(DbFloat::Text(t))
        | DbValue::Decimal(t)
        | DbValue::Text(t)
        | DbValue::Json(t)
        | DbValue::Datetime(t)
        | DbValue::Uuid(t) => Value::Bytes(t.clone().into_bytes()),
        DbValue::Other(o) => Value::Bytes(o.text.clone().into_bytes()),
        DbValue::Bytes(b) if b.truncated => return Err(invalid("a truncated byte value")),
        DbValue::Bytes(b) => {
            Value::Bytes(decode_hex(&b.hex).ok_or_else(|| invalid("a malformed hex value"))?)
        }
        DbValue::Truncated(_) => return Err(invalid("a truncated value")),
        DbValue::Array(_) => return Err(invalid("an array")),
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
    use pw_db_types::{DbBytes, MAX_SAFE_INTEGER};

    use super::*;

    const UTF8MB4: u16 = 255;

    fn col(kind: ColumnType, flags: ColumnFlags, charset: u16) -> Column {
        Column::new(kind)
            .with_flags(flags)
            .with_character_set(charset)
    }

    fn text(kind: ColumnType, flags: ColumnFlags, charset: u16, raw: &str) -> DbValue {
        cell(
            &col(kind, flags, charset),
            Value::Bytes(raw.as_bytes().to_vec()),
            CellLimit::DEFAULT,
        )
    }

    /// 14-m3-plan.md M3-07 ⑤ 의 값 표를 실 DB 없이(텍스트 프로토콜의 바이트로) 본다. live 시험이 같은 표를 실 서버에서 본다.
    #[test]
    fn text_protocol_values_follow_the_value_rules() {
        use ColumnType::*;
        let unsigned = ColumnFlags::UNSIGNED_FLAG;
        let none = ColumnFlags::empty();
        assert_eq!(
            text(MYSQL_TYPE_LONGLONG, none, BINARY_CHARSET, "5"),
            DbValue::int(5)
        );
        let max = text(
            MYSQL_TYPE_LONGLONG,
            unsigned,
            BINARY_CHARSET,
            "18446744073709551615",
        );
        assert_eq!(
            max,
            DbValue::Int(DbInt::Text("18446744073709551615".into()))
        );
        let edge = text(
            MYSQL_TYPE_LONGLONG,
            none,
            BINARY_CHARSET,
            &MAX_SAFE_INTEGER.to_string(),
        );
        assert_eq!(edge, DbValue::Int(DbInt::Number(MAX_SAFE_INTEGER)));
        assert_eq!(
            text(
                MYSQL_TYPE_NEWDECIMAL,
                none,
                BINARY_CHARSET,
                "12345678901234567890.1234567890"
            ),
            DbValue::Decimal("12345678901234567890.1234567890".into())
        );
        assert_eq!(
            text(
                MYSQL_TYPE_BLOB,
                ColumnFlags::BINARY_FLAG,
                BINARY_CHARSET,
                "\u{1}\u{2}"
            ),
            DbValue::bytes(&[1, 2], CellLimit::DEFAULT)
        );
        assert_eq!(
            text(MYSQL_TYPE_BLOB, none, UTF8MB4, "long text"),
            DbValue::Text("long text".into())
        );
        assert_eq!(
            text(MYSQL_TYPE_JSON, none, BINARY_CHARSET, r#"{"a": 1}"#),
            DbValue::Json(r#"{"a": 1}"#.into())
        );
        assert_eq!(
            text(
                MYSQL_TYPE_DATETIME,
                none,
                BINARY_CHARSET,
                "2026-10-31 12:34:56.123456"
            ),
            DbValue::Datetime("2026-10-31 12:34:56.123456".into())
        );
        assert_eq!(
            text(MYSQL_TYPE_DOUBLE, none, BINARY_CHARSET, "1.5"),
            DbValue::float(1.5)
        );
        assert_eq!(
            text(MYSQL_TYPE_VAR_STRING, none, BINARY_CHARSET, "ab"),
            DbValue::bytes(b"ab", CellLimit::DEFAULT)
        );
        assert_eq!(
            cell(
                &col(MYSQL_TYPE_LONG, none, 63),
                Value::NULL,
                CellLimit::DEFAULT
            ),
            DbValue::Null
        );
        // 글 열에 UTF-8 이 아닌 바이트가 오면 바이트로 보인다.
        let latin1 = cell(
            &col(MYSQL_TYPE_VAR_STRING, none, UTF8MB4),
            Value::Bytes(vec![0xe9]),
            CellLimit::DEFAULT,
        );
        assert_eq!(latin1, DbValue::bytes(&[0xe9], CellLimit::DEFAULT));
    }

    #[test]
    fn binary_protocol_values_match_the_text_ones() {
        use ColumnType::*;
        let none = ColumnFlags::empty();
        let unsigned = col(
            MYSQL_TYPE_LONGLONG,
            ColumnFlags::UNSIGNED_FLAG,
            BINARY_CHARSET,
        );
        assert_eq!(
            cell(&unsigned, Value::UInt(u64::MAX), CellLimit::DEFAULT),
            DbValue::Int(DbInt::Text(u64::MAX.to_string()))
        );
        assert_eq!(
            cell(&unsigned, Value::UInt(5), CellLimit::DEFAULT),
            DbValue::int(5)
        );
        let datetime = col(MYSQL_TYPE_DATETIME, none, BINARY_CHARSET).with_decimals(6);
        assert_eq!(
            cell(
                &datetime,
                Value::Date(2026, 10, 31, 12, 34, 56, 123_456),
                CellLimit::DEFAULT
            ),
            DbValue::Datetime("2026-10-31 12:34:56.123456".into())
        );
        let seconds = col(MYSQL_TYPE_TIMESTAMP, none, BINARY_CHARSET).with_decimals(2);
        assert_eq!(
            cell(
                &seconds,
                Value::Date(2026, 1, 2, 3, 4, 5, 670_000),
                CellLimit::DEFAULT
            ),
            DbValue::Datetime("2026-01-02 03:04:05.67".into())
        );
        let date = col(MYSQL_TYPE_DATE, none, BINARY_CHARSET);
        assert_eq!(
            cell(
                &date,
                Value::Date(2026, 1, 2, 0, 0, 0, 0),
                CellLimit::DEFAULT
            ),
            DbValue::Datetime("2026-01-02".into())
        );
        let time = col(MYSQL_TYPE_TIME, none, BINARY_CHARSET);
        assert_eq!(
            cell(&time, Value::Time(true, 1, 2, 3, 4, 0), CellLimit::DEFAULT),
            DbValue::Datetime("-26:03:04".into())
        );
    }

    #[test]
    fn type_names_carry_unsigned_and_binary() {
        use ColumnType::*;
        let name = |kind, flags, charset| type_name(&col(kind, flags, charset));
        assert_eq!(
            name(
                MYSQL_TYPE_LONGLONG,
                ColumnFlags::UNSIGNED_FLAG,
                BINARY_CHARSET
            ),
            "BIGINT UNSIGNED"
        );
        assert_eq!(
            name(MYSQL_TYPE_BLOB, ColumnFlags::BINARY_FLAG, BINARY_CHARSET),
            "BLOB"
        );
        assert_eq!(name(MYSQL_TYPE_BLOB, ColumnFlags::empty(), UTF8MB4), "TEXT");
        assert_eq!(
            name(MYSQL_TYPE_STRING, ColumnFlags::ENUM_FLAG, UTF8MB4),
            "ENUM"
        );
        assert_eq!(
            name(MYSQL_TYPE_NEWDECIMAL, ColumnFlags::empty(), BINARY_CHARSET),
            "DECIMAL"
        );
    }

    #[test]
    fn params_bind_values_without_losing_precision() {
        assert_eq!(param(&DbValue::int(5)).unwrap(), Value::Int(5));
        assert_eq!(
            param(&DbValue::int(u64::MAX)).unwrap(),
            Value::UInt(u64::MAX)
        );
        assert_eq!(
            param(&DbValue::decimal("0.1")).unwrap(),
            Value::Bytes(b"0.1".to_vec())
        );
        assert_eq!(
            param(&DbValue::bytes(&[0xde, 0xad], CellLimit::DEFAULT)).unwrap(),
            Value::Bytes(vec![0xde, 0xad])
        );
        let truncated = DbValue::Bytes(DbBytes {
            hex: "0xde".into(),
            size: 2,
            truncated: true,
        });
        assert_eq!(param(&truncated).unwrap_err().kind, DbErrorKind::Invalid);
        assert_eq!(
            param(&DbValue::Array(vec![])).unwrap_err().kind,
            DbErrorKind::Invalid
        );
        assert_eq!(param(&DbValue::Bool(true)).unwrap(), Value::Int(1));
    }
}
