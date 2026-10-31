use serde_json::{Value, json};

use super::*;

fn wire(value: &DbValue) -> Value {
    serde_json::to_value(value).unwrap()
}

/// 완료 기준 ①: 직렬화 스냅숏. 같은 표를 왕복(역직렬화)으로도 본다.
#[test]
fn values_serialize_by_the_m3_d8_rules() {
    let limit = CellLimit(4);
    let table = [
        // 정수는 값으로 가른다.
        (
            DbValue::int(1_i128 << 53),
            json!({ "int": "9007199254740992" }),
        ),
        (
            DbValue::int(MAX_SAFE_INTEGER),
            json!({ "int": 9_007_199_254_740_991_i64 }),
        ),
        (
            DbValue::int(-MAX_SAFE_INTEGER),
            json!({ "int": -9_007_199_254_740_991_i64 }),
        ),
        (
            DbValue::int(-(1_i128 << 53)),
            json!({ "int": "-9007199254740992" }),
        ),
        (
            DbValue::int(u64::MAX),
            json!({ "int": "18446744073709551615" }),
        ),
        (DbValue::int(5_i64), json!({ "int": 5 })),
        (DbValue::decimal("0.1"), json!({ "decimal": "0.1" })),
        (
            DbValue::bytes(&[0xde, 0xad], limit),
            json!({ "bytes": { "hex": "0xdead", "size": 2, "truncated": false } }),
        ),
        // 셀 상한 + 1 바이트: 앞부분과 원래 크기, truncated.
        (
            DbValue::bytes(&[0xde, 0xad, 0xbe, 0xef, 0x01], limit),
            json!({ "bytes": { "hex": "0xdeadbeef", "size": 5, "truncated": true } }),
        ),
        (
            DbValue::bytes(&[0xde, 0xad, 0xbe, 0xef], limit),
            json!({ "bytes": { "hex": "0xdeadbeef", "size": 4, "truncated": false } }),
        ),
        (DbValue::Null, json!("null")),
        (DbValue::Bool(true), json!({ "bool": true })),
        (DbValue::float(1.5), json!({ "float": 1.5 })),
        (DbValue::float(f64::NAN), json!({ "float": "NaN" })),
        (
            DbValue::float(f64::NEG_INFINITY),
            json!({ "float": "-Infinity" }),
        ),
        (DbValue::text("abcd", limit), json!({ "text": "abcd" })),
        (
            DbValue::json("{\"a\":1}", CellLimit::DEFAULT),
            json!({ "json": "{\"a\":1}" }),
        ),
        (
            DbValue::Datetime("2026-10-31 09:00:00+09".into()),
            json!({ "datetime": "2026-10-31 09:00:00+09" }),
        ),
        (
            DbValue::Array(vec![DbValue::int(1_i64), DbValue::Null]),
            json!({ "array": [{ "int": 1 }, "null"] }),
        ),
        (
            DbValue::other("'a':1", "tsvector", CellLimit::DEFAULT),
            json!({ "other": { "text": "'a':1", "typeName": "tsvector" } }),
        ),
    ];
    for (value, expected) in table {
        assert_eq!(wire(&value), expected, "{value:?}");
        let back: DbValue = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(back, value, "{expected}");
    }
}

#[test]
fn text_over_the_cell_limit_keeps_a_prefix_on_a_char_boundary() {
    // 「가」 는 UTF-8 3바이트다. 상한 4 는 둘째 글자 가운데라 한 글자만 남는다.
    let value = DbValue::text("가나다", CellLimit(4));
    assert_eq!(
        wire(&value),
        json!({ "truncated": { "of": "text", "prefix": "가", "size": 9, "typeName": null } })
    );
    let json = DbValue::json("[1,2,3]", CellLimit(3));
    assert_eq!(
        wire(&json),
        json!({ "truncated": { "of": "json", "prefix": "[1,", "size": 7, "typeName": null } })
    );
    let other = DbValue::other("POINT(1 2)", "geometry", CellLimit(5));
    assert_eq!(
        wire(&other),
        json!({ "truncated": { "of": "other", "prefix": "POINT", "size": 10, "typeName": "geometry" } })
    );
}
