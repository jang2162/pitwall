use pw_db_types::{
    CellLimit, DbBytes, DbFloat, DbInt, DbOther, DbTruncated, DbValue, ObjectRef, SqlDialect,
    TruncatedKind,
};

use super::*;

const PG: SqlDialect = SqlDialect::Postgres;
const MY: SqlDialect = SqlDialect::Mysql;
const LITE: SqlDialect = SqlDialect::Sqlite;

fn text(s: &str) -> DbValue {
    DbValue::Text(s.to_owned())
}

#[test]
fn plain_strings_only_double_the_quote() {
    for d in [PG, MY, LITE] {
        assert_eq!(literal(d, &text("it's")).unwrap(), "'it''s'");
        assert_eq!(literal(d, &text("")).unwrap(), "''");
        assert_eq!(
            literal(d, &text("한글 \"따옴표\"")).unwrap(),
            "'한글 \"따옴표\"'"
        );
    }
}

#[test]
fn backslashes_take_a_form_that_does_not_depend_on_server_settings() {
    // PG: E'' 는 standard_conforming_strings 와 상관없이 백슬래시를 이스케이프로 읽는다.
    assert_eq!(literal(PG, &text(r"a\b'c")).unwrap(), r"E'a\\b''c'");
    // MySQL: NO_BACKSLASH_ESCAPES 에서도 아니어도 같은 값 — hex.
    assert_eq!(literal(MY, &text(r"a\b")).unwrap(), "_utf8mb4 X'615c62'");
    // SQLite 에는 백슬래시 이스케이프가 없다.
    assert_eq!(literal(LITE, &text(r"a\b")).unwrap(), r"'a\b'");
}

#[test]
fn control_characters_never_appear_raw() {
    assert_eq!(
        literal(PG, &text("a\nb\u{85}")).unwrap(),
        r"E'a\x0ab\u0085'"
    );
    assert_eq!(literal(MY, &text("a\nb")).unwrap(), "_utf8mb4 X'610a62'");
    assert_eq!(
        literal(LITE, &text("a\0b")).unwrap(),
        "CAST(X'610062' AS TEXT)"
    );
    assert_eq!(literal(MY, &text("a\0")).unwrap(), "_utf8mb4 X'6100'");
    for d in [PG, MY, LITE] {
        let out = literal(d, &text("x\r\ny\t")).unwrap();
        assert!(!out.chars().any(char::is_control), "{d:?}: {out}");
    }
}

#[test]
fn pg_refuses_nul_in_strings_and_identifiers() {
    assert_eq!(literal(PG, &text("a\0")), Err(LiteralError::Nul));
    assert_eq!(quote_ident(PG, "a\0"), Err(LiteralError::Nul));
}

#[test]
fn identifiers_cannot_break_out_of_their_quotes() {
    assert_eq!(quote_ident(PG, r#"a"b"#).unwrap(), r#""a""b""#);
    assert_eq!(
        quote_ident(LITE, r#"x" ; DROP TABLE t; --"#).unwrap(),
        r#""x"" ; DROP TABLE t; --""#
    );
    assert_eq!(quote_ident(MY, "a`b").unwrap(), "`a``b`");
    assert_eq!(quote_ident(MY, "a]b\"c").unwrap(), "`a]b\"c`");
    assert_eq!(quote_ident(PG, "a]b`c").unwrap(), r#""a]b`c""#);
}

#[test]
fn numbers_bools_and_null() {
    assert_eq!(literal(PG, &DbValue::Null).unwrap(), "NULL");
    assert_eq!(literal(PG, &DbValue::Bool(true)).unwrap(), "TRUE");
    assert_eq!(literal(MY, &DbValue::Bool(false)).unwrap(), "FALSE");
    assert_eq!(literal(LITE, &DbValue::Bool(true)).unwrap(), "1");
    assert_eq!(literal(PG, &DbValue::int(-42)).unwrap(), "-42");
    assert_eq!(
        literal(MY, &DbValue::int(u64::MAX)).unwrap(),
        "18446744073709551615"
    );
    assert_eq!(literal(PG, &DbValue::float(0.1)).unwrap(), "0.1");
    assert_eq!(literal(PG, &DbValue::float(1e300)).unwrap(), "1e300");
    assert_eq!(
        literal(PG, &DbValue::Decimal("-12.3400".into())).unwrap(),
        "-12.3400"
    );
}

#[test]
fn non_numeric_text_in_number_slots_is_quoted() {
    // 서버(또는 프로세스 밖 드라이버)가 준 글이 숫자가 아니면 문장 밖으로 새지 않게 문자열로 싼다.
    assert_eq!(
        literal(PG, &DbValue::Decimal("NaN".into())).unwrap(),
        "'NaN'"
    );
    assert_eq!(
        literal(MY, &DbValue::Decimal("1); DROP TABLE t; --".into())).unwrap(),
        "'1); DROP TABLE t; --'"
    );
    assert_eq!(
        literal(PG, &DbValue::Int(DbInt::Text("1 OR 1=1".into()))).unwrap(),
        "'1 OR 1=1'"
    );
    for ok in ["1", "-1", "+1.5", ".5", "5.", "1e10", "1.5E-7"] {
        assert!(is_number(ok), "{ok}");
    }
    for bad in [
        "", "-", ".", "e5", "1e", "1e+", "1.2.3", "0x10", "1_000", "١٢",
    ] {
        assert!(!is_number(bad), "{bad}");
    }
}

#[test]
fn non_finite_floats_follow_the_dialect() {
    let nan = DbValue::Float(DbFloat::Text("NaN".into()));
    let inf = DbValue::float(f64::INFINITY);
    assert_eq!(literal(PG, &nan).unwrap(), "'NaN'");
    assert_eq!(literal(PG, &inf).unwrap(), "'Infinity'");
    assert_eq!(literal(LITE, &inf).unwrap(), "9e999");
    assert_eq!(
        literal(LITE, &DbValue::float(f64::NEG_INFINITY)).unwrap(),
        "-9e999"
    );
    assert_eq!(literal(LITE, &nan), Err(LiteralError::Unsupported));
    assert_eq!(literal(MY, &inf), Err(LiteralError::Unsupported));
}

#[test]
fn bytes_use_hex_forms() {
    let b = DbValue::bytes(&[0xde, 0xad, 0x00, b'\''], CellLimit::DEFAULT);
    assert_eq!(literal(PG, &b).unwrap(), "decode('dead0027', 'hex')");
    assert_eq!(literal(MY, &b).unwrap(), "X'dead0027'");
    assert_eq!(literal(LITE, &b).unwrap(), "X'dead0027'");
    let empty = DbValue::bytes(&[], CellLimit::DEFAULT);
    assert_eq!(literal(MY, &empty).unwrap(), "X''");
    // 와이어 값이 hex 가 아니면 거절한다.
    let forged = DbValue::Bytes(DbBytes {
        hex: "0x00'); DROP TABLE t; --".into(),
        size: 1,
        truncated: false,
    });
    assert_eq!(literal(PG, &forged), Err(LiteralError::Unsupported));
}

#[test]
fn truncated_values_are_refused() {
    let t = DbValue::Truncated(DbTruncated {
        of: TruncatedKind::Text,
        prefix: "abc".into(),
        size: 10_000_000,
        type_name: None,
    });
    let b = DbValue::bytes(&[1; 10], CellLimit(4));
    for d in [PG, MY, LITE] {
        assert_eq!(literal(d, &t), Err(LiteralError::Truncated));
        assert_eq!(literal(d, &b), Err(LiteralError::Truncated));
    }
}

#[test]
fn pg_arrays_use_the_array_text_form_inside_a_string_literal() {
    let a = DbValue::Array(vec![
        DbValue::int(1),
        DbValue::Null,
        text(r#"a"b\c"#),
        DbValue::Array(vec![text("x,y")]),
    ]);
    assert_eq!(
        literal(PG, &a).unwrap(),
        r#"E'{"1",NULL,"a\\"b\\\\c",{"x,y"}}'"#
    );
    assert_eq!(literal(MY, &a), Err(LiteralError::Unsupported));
    assert_eq!(literal(LITE, &a), Err(LiteralError::Unsupported));
}

#[test]
fn other_values_are_their_text() {
    let o = DbValue::Other(DbOther {
        text: "'a':1".into(),
        type_name: "tsvector".into(),
    });
    assert_eq!(literal(PG, &o).unwrap(), "'''a'':1'");
}

#[test]
fn insert_text_writes_one_statement_per_row() {
    let rows = vec![
        vec![DbValue::int(1), text("a'b")],
        vec![DbValue::Null, text("c")],
    ];
    let cols = vec!["id".to_owned(), "na\"me".to_owned()];
    assert_eq!(
        insert_text(PG, None, &cols, &rows).unwrap(),
        "INSERT INTO MY_TABLE (\"id\", \"na\"\"me\") VALUES (1, 'a''b');\n\
         INSERT INTO MY_TABLE (\"id\", \"na\"\"me\") VALUES (NULL, 'c');\n"
    );
    let table = ObjectRef {
        database: None,
        schema: Some("public".into()),
        name: "t`x".into(),
    };
    assert_eq!(
        insert_text(MY, Some(&table), &cols[..1], &[vec![DbValue::int(7)]]).unwrap(),
        "INSERT INTO `public`.`t``x` (`id`) VALUES (7);\n"
    );
}

#[test]
fn insert_text_names_the_failing_cell() {
    let rows = vec![
        vec![text("ok")],
        vec![DbValue::Truncated(DbTruncated {
            of: TruncatedKind::Json,
            prefix: "{".into(),
            size: 9,
            type_name: None,
        })],
    ];
    assert_eq!(
        insert_text(LITE, None, &["j".to_owned()], &rows),
        Err(InsertTextError {
            row: Some(1),
            column: Some(0),
            reason: LiteralError::Truncated
        })
    );
    assert_eq!(
        insert_text(LITE, None, &["a".to_owned()], &[vec![]]),
        Err(InsertTextError {
            row: Some(0),
            column: None,
            reason: LiteralError::Unsupported
        })
    );
}
