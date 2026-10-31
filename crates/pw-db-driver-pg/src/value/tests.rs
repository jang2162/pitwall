use pw_db_types::{CellLimit, DbValue};
use tokio_postgres::types::Type;

use super::*;

/// numeric 바이너리를 만든다(자리 · 가중치 · 부호 · 표시 소수 자리).
fn numeric_bytes(digits: &[i16], weight: i16, sign: u16, dscale: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&i16::try_from(digits.len()).unwrap().to_be_bytes());
    out.extend_from_slice(&weight.to_be_bytes());
    out.extend_from_slice(&sign.to_be_bytes());
    out.extend_from_slice(&dscale.to_be_bytes());
    for d in digits {
        out.extend_from_slice(&d.to_be_bytes());
    }
    out
}

#[test]
fn numeric_matches_the_server_text() {
    // (자리, 가중치, 부호, 표시 소수 자리) → PG 의 numeric_out 글.
    let table: [(&[i16], i16, u16, u16, &str); 10] = [
        (&[], 0, 0, 0, "0"),
        (&[], 0, 0, 2, "0.00"),
        (&[1000], -1, 0, 1, "0.1"),
        (&[1000], -1, 0, 3, "0.100"),
        (&[1, 2345, 6780], 1, 0, 3, "12345.678"),
        (&[10], 1, 0, 0, "100000"),
        (&[5], -2, 0x4000, 8, "-0.00000005"),
        (&[9007, 1992, 5474, 993], 3, 0, 0, "9007199254740993"),
        (&[], 0, 0xC000, 0, "NaN"),
        (&[], 0, 0xF000, 0, "-Infinity"),
    ];
    for (digits, weight, sign, dscale, text) in table {
        assert_eq!(
            numeric(&numeric_bytes(digits, weight, sign, dscale)).as_deref(),
            Some(text),
            "{digits:?} {weight} {sign:#x} {dscale}"
        );
    }
    assert_eq!(numeric(&[0, 1]), None, "short input");
    assert_eq!(
        numeric(&numeric_bytes(&[10_000], 0, 0, 0)),
        None,
        "a digit is below 10000"
    );
}

fn decode_one(ty: &Type, raw: &[u8]) -> (Cell, TextRequests) {
    let mut texts = TextRequests::default();
    let cell = decode(ty, Some(raw), CellLimit::DEFAULT, &mut texts);
    (cell, texts)
}

/// 서버 없이 풀리는 값(텍스트 출력 요청이 없어야 한다).
fn value_of(ty: &Type, raw: &[u8]) -> DbValue {
    let (cell, texts) = decode_one(ty, raw);
    assert!(texts.is_empty(), "{ty} asked the server for text: {cell:?}");
    finish(cell, &texts, &[], CellLimit::DEFAULT)
}

#[test]
fn integers_follow_the_safe_integer_rule_and_other_binary_types_decode() {
    assert_eq!(value_of(&Type::INT8, &5_i64.to_be_bytes()), DbValue::int(5));
    let big = value_of(&Type::INT8, &(1_i64 << 53).to_be_bytes());
    assert_eq!(
        serde_json::to_value(&big).unwrap()["int"],
        "9007199254740992"
    );
    assert_eq!(
        value_of(&Type::INT2, &(-3_i16).to_be_bytes()),
        DbValue::int(-3)
    );
    assert_eq!(value_of(&Type::BOOL, &[1]), DbValue::Bool(true));
    assert_eq!(
        value_of(&Type::FLOAT4, &0.1_f32.to_be_bytes()),
        DbValue::float(0.1),
        "float4 goes through its shortest text"
    );
    assert_eq!(
        value_of(&Type::BYTEA, &[0xde, 0xad, 0xbe, 0xef]),
        DbValue::bytes(&[0xde, 0xad, 0xbe, 0xef], CellLimit::DEFAULT)
    );
    assert_eq!(
        value_of(&Type::JSONB, b"\x01{\"a\": 1}"),
        DbValue::Json("{\"a\": 1}".into())
    );
    assert_eq!(
        value_of(&Type::JSON, b"[1,2]"),
        DbValue::Json("[1,2]".into())
    );
    let id = [
        0xa0, 0xee, 0xbc, 0x99, 0x9c, 0x0b, 0x4e, 0xf8, 0xbb, 0x6d, 0x6b, 0xb9, 0xbd, 0x38, 0x0a,
        0x11,
    ];
    assert_eq!(
        value_of(&Type::UUID, &id),
        DbValue::Uuid("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".into())
    );
    assert_eq!(
        value_of(&Type::NUMERIC, &numeric_bytes(&[1000], -1, 0, 1)),
        DbValue::Decimal("0.1".into())
    );
}

#[test]
fn time_money_and_unknown_types_ask_the_server_for_text() {
    for ty in [
        Type::TIMESTAMPTZ,
        Type::TIMESTAMP,
        Type::DATE,
        Type::INTERVAL,
        Type::MONEY,
        Type::INET,
    ] {
        let (cell, texts) = decode_one(&ty, &[0; 8]);
        assert_eq!(cell, Cell::Text(0), "{ty}");
        assert_eq!(texts.items.len(), 1);
    }
    // 깨진 바이트도 서버에 맡긴다(로컬에서 틀린 값을 만들지 않는다).
    let (cell, _) = decode_one(&Type::INT8, &[1, 2]);
    assert_eq!(cell, Cell::Text(0));

    let texts = decode_one(&Type::TIMESTAMPTZ, &[0; 8]).1;
    let outputs = vec![Ok("2026-10-31 09:00:00+09".to_owned())];
    assert_eq!(
        finish(Cell::Text(0), &texts, &outputs, CellLimit::DEFAULT),
        DbValue::Datetime("2026-10-31 09:00:00+09".into())
    );
    let failed = finish(
        Cell::Text(0),
        &texts,
        &[Err("boom".to_owned())],
        CellLimit::DEFAULT,
    );
    match failed {
        DbValue::Other(other) => {
            assert_eq!(other.type_name, "timestamptz");
            assert!(
                other.text.contains("cannot display: boom"),
                "{}",
                other.text
            );
        }
        other => panic!("{other:?}"),
    }
}

/// 배열 바이너리를 만든다(원소는 `None` 이면 NULL).
fn array_bytes(element: &Type, dims: &[i32], items: &[Option<Vec<u8>>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&i32::try_from(dims.len()).unwrap().to_be_bytes());
    out.extend_from_slice(&i32::from(items.iter().any(Option::is_none)).to_be_bytes());
    out.extend_from_slice(&element.oid().to_be_bytes());
    for d in dims {
        out.extend_from_slice(&d.to_be_bytes());
        out.extend_from_slice(&1_i32.to_be_bytes());
    }
    for item in items {
        match item {
            None => out.extend_from_slice(&(-1_i32).to_be_bytes()),
            Some(bytes) => {
                out.extend_from_slice(&i32::try_from(bytes.len()).unwrap().to_be_bytes());
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

#[test]
fn arrays_nest_by_dimension_and_keep_text_requests_inside() {
    let int = |n: i32| Some(n.to_be_bytes().to_vec());
    let flat = array_bytes(&Type::INT4, &[3], &[int(1), None, int(3)]);
    assert_eq!(
        value_of(&Type::INT4_ARRAY, &flat),
        DbValue::Array(vec![DbValue::int(1), DbValue::Null, DbValue::int(3)])
    );
    let square = array_bytes(&Type::INT4, &[2, 2], &[int(1), int(2), int(3), int(4)]);
    assert_eq!(
        value_of(&Type::INT4_ARRAY, &square),
        DbValue::Array(vec![
            DbValue::Array(vec![DbValue::int(1), DbValue::int(2)]),
            DbValue::Array(vec![DbValue::int(3), DbValue::int(4)]),
        ])
    );
    assert_eq!(
        value_of(&Type::INT4_ARRAY, &array_bytes(&Type::INT4, &[], &[])),
        DbValue::Array(vec![])
    );

    let stamps = array_bytes(
        &Type::TIMESTAMPTZ,
        &[2],
        &[Some(vec![0; 8]), Some(vec![1; 8])],
    );
    let (cell, texts) = decode_one(&Type::TIMESTAMPTZ_ARRAY, &stamps);
    assert_eq!(cell, Cell::Array(vec![Cell::Text(0), Cell::Text(1)]));
    assert_eq!(texts.items.len(), 2);

    // 차원이 바이트보다 크다고 거짓말하면 서버에 맡긴다(큰 메모리를 잡지 않는다).
    let lying = array_bytes(&Type::INT4, &[i32::MAX], &[int(1)]);
    assert_eq!(decode_one(&Type::INT4_ARRAY, &lying).0, Cell::Text(0));
}

#[test]
fn bind_values_become_postgres_text_input() {
    let text = |v: &DbValue| param(v).unwrap().0;
    assert_eq!(text(&DbValue::Null), None);
    assert_eq!(
        text(&DbValue::int(1_i64 << 53)).as_deref(),
        Some("9007199254740992")
    );
    assert_eq!(text(&DbValue::Bool(false)).as_deref(), Some("f"));
    assert_eq!(
        text(&DbValue::bytes(&[0xde, 0xad], CellLimit::DEFAULT)).as_deref(),
        Some("\\xdead")
    );
    assert_eq!(
        text(&DbValue::Array(vec![
            DbValue::Text("a\"b\\c".into()),
            DbValue::Null,
            DbValue::Array(vec![DbValue::int(1)]),
        ]))
        .as_deref(),
        Some(r#"{"a\"b\\c",NULL,{"1"}}"#)
    );
    let truncated = DbValue::bytes(&[1, 2, 3], CellLimit(1));
    assert!(param(&truncated).is_err());
}

#[test]
fn type_names_read_like_sql() {
    assert_eq!(type_name(&Type::INT4_ARRAY), "int4[]");
    assert_eq!(type_name(&Type::TIMESTAMPTZ), "timestamptz");
}
