use pw_db_types::{
    CellLimit, DbBytes, DbTruncated, DbValue, ObjectRef, RiskGrade, SqlDialect, TruncatedKind,
};

use super::*;
use crate::literal::testing::{Rng, text};
use crate::risk::classify_statement;
use crate::split::split;

const ALL: [SqlDialect; 3] = [SqlDialect::Postgres, SqlDialect::Mysql, SqlDialect::Sqlite];

fn table(dialect: SqlDialect, name: &str) -> ObjectRef {
    match dialect {
        SqlDialect::Postgres => ObjectRef {
            database: Some("app".into()),
            schema: Some("public".into()),
            name: name.into(),
        },
        SqlDialect::Mysql => ObjectRef {
            database: Some("app".into()),
            schema: None,
            name: name.into(),
        },
        SqlDialect::Sqlite => ObjectRef {
            database: None,
            schema: None,
            name: name.into(),
        },
    }
}

fn cell(column: &str, value: DbValue) -> DbEditCell {
    DbEditCell {
        column: column.into(),
        value,
    }
}

fn key(id: i64) -> Vec<DbEditCell> {
    vec![cell("id", DbValue::int(id))]
}

fn text_value(s: &str) -> DbValue {
    DbValue::text(s, CellLimit::DEFAULT)
}

/// 셀 둘 수정 + 추가 하나 + 삭제 하나.
fn scenario() -> DbEditChanges {
    DbEditChanges {
        updates: vec![DbEditUpdate {
            key: key(1),
            values: vec![
                cell("name", text_value("it's")),
                cell("note", DbValue::Null),
            ],
        }],
        inserts: vec![DbEditInsert {
            values: vec![cell("id", DbValue::int(3)), cell("name", text_value("c"))],
        }],
        deletes: vec![DbEditDelete { key: key(2) }],
    }
}

fn ids() -> Vec<String> {
    vec!["id".to_owned()]
}

#[test]
fn postgres_statements_bind_values_and_qualify_the_table() {
    let d = SqlDialect::Postgres;
    let out = build(d, &table(d, "t"), &ids(), &scenario()).unwrap();
    let sql: Vec<&str> = out.iter().map(|s| s.sql.as_str()).collect();
    assert_eq!(
        sql,
        [
            r#"DELETE FROM "app"."public"."t" WHERE "id" = $1"#,
            r#"UPDATE "app"."public"."t" SET "name" = $1, "note" = $2 WHERE "id" = $3"#,
            r#"INSERT INTO "app"."public"."t" ("id", "name") VALUES ($1, $2)"#,
        ]
    );
    let preview: Vec<&str> = out.iter().map(|s| s.preview.as_str()).collect();
    assert_eq!(
        preview,
        [
            r#"DELETE FROM "app"."public"."t" WHERE "id" = 2"#,
            r#"UPDATE "app"."public"."t" SET "name" = 'it''s', "note" = NULL WHERE "id" = 1"#,
            r#"INSERT INTO "app"."public"."t" ("id", "name") VALUES (3, 'c')"#,
        ]
    );
    assert_eq!(
        out[1].params,
        [text_value("it's"), DbValue::Null, DbValue::int(1)]
    );
    let changes: Vec<DbEditChangeRef> = out.iter().map(|s| s.change).collect();
    assert_eq!(
        changes,
        [
            DbEditChangeRef {
                kind: DbEditChangeKind::Delete,
                index: 0
            },
            DbEditChangeRef {
                kind: DbEditChangeKind::Update,
                index: 0
            },
            DbEditChangeRef {
                kind: DbEditChangeKind::Insert,
                index: 0
            },
        ]
    );
    assert!(out.iter().all(|s| s.probe.is_none()));
}

#[test]
fn mysql_statements_use_backticks_and_probe_updates_with_a_locking_read() {
    let d = SqlDialect::Mysql;
    let out = build(d, &table(d, "t"), &ids(), &scenario()).unwrap();
    let sql: Vec<&str> = out.iter().map(|s| s.sql.as_str()).collect();
    assert_eq!(
        sql,
        [
            "DELETE FROM `app`.`t` WHERE `id` = ?",
            "UPDATE `app`.`t` SET `name` = ?, `note` = ? WHERE `id` = ?",
            "INSERT INTO `app`.`t` (`id`, `name`) VALUES (?, ?)",
        ]
    );
    let probe = out[1].probe.as_ref().unwrap();
    assert_eq!(
        probe.sql,
        "SELECT 1 FROM `app`.`t` WHERE `id` = ? FOR UPDATE"
    );
    assert_eq!(probe.params, [DbValue::int(1)]);
    assert!(out[0].probe.is_none() && out[2].probe.is_none());
}

#[test]
fn sqlite_statements_name_main_when_the_table_has_no_schema() {
    let d = SqlDialect::Sqlite;
    let out = build(d, &table(d, "t"), &ids(), &scenario()).unwrap();
    assert_eq!(out[0].sql, r#"DELETE FROM "main"."t" WHERE "id" = ?"#);
    let attached = ObjectRef {
        database: Some("aux".into()),
        schema: None,
        name: "t".into(),
    };
    let out = build(d, &attached, &ids(), &scenario()).unwrap();
    assert_eq!(out[0].sql, r#"DELETE FROM "aux"."t" WHERE "id" = ?"#);
}

#[test]
fn composite_keys_follow_the_key_column_order() {
    let d = SqlDialect::Postgres;
    let changes = DbEditChanges {
        deletes: vec![DbEditDelete {
            // 창이 보낸 순서와 상관없이 키 열의 순서로.
            key: vec![cell("b", DbValue::int(2)), cell("a", DbValue::int(1))],
        }],
        ..DbEditChanges::default()
    };
    let out = build(d, &table(d, "t"), &["a".into(), "b".into()], &changes).unwrap();
    assert_eq!(
        out[0].sql,
        r#"DELETE FROM "app"."public"."t" WHERE "a" = $1 AND "b" = $2"#
    );
    assert_eq!(out[0].params, [DbValue::int(1), DbValue::int(2)]);
}

#[test]
fn an_insert_without_values_takes_the_defaults() {
    let changes = DbEditChanges {
        inserts: vec![DbEditInsert { values: vec![] }],
        ..DbEditChanges::default()
    };
    let got: Vec<String> = ALL
        .iter()
        .map(|&d| {
            build(d, &table(d, "t"), &ids(), &changes).unwrap()[0]
                .sql
                .clone()
        })
        .collect();
    assert_eq!(
        got,
        [
            r#"INSERT INTO "app"."public"."t" DEFAULT VALUES"#,
            "INSERT INTO `app`.`t` () VALUES ()",
            r#"INSERT INTO "main"."t" DEFAULT VALUES"#,
        ]
    );
}

/// 값은 글에 들어가지 않는다 — 바인드 값에만 있고, 미리보기는 그 값의 리터럴이다.
#[test]
fn a_hostile_value_stays_a_bind_value() {
    let hostile = "'; DROP TABLE t; --";
    for d in ALL {
        let changes = DbEditChanges {
            updates: vec![DbEditUpdate {
                key: key(1),
                values: vec![cell("name", text_value(hostile))],
            }],
            ..DbEditChanges::default()
        };
        let out = build(d, &table(d, "t"), &ids(), &changes).unwrap();
        assert!(!out[0].sql.contains("DROP"), "{d:?}: {}", out[0].sql);
        assert_eq!(out[0].params[0], text_value(hostile));
        assert!(
            out[0]
                .preview
                .contains(&literal(d, &text_value(hostile)).unwrap())
        );
    }
}

/// 미리보기의 리터럴은 바인드 값과 같은 자리 · 같은 값이다: 글의 자리 표시를 순서대로 `literal(값)` 으로 바꾸면 미리보기다.
#[test]
fn the_preview_is_the_bind_form_with_each_value_as_its_literal() {
    let (mut rng, seed) = Rng::from_env();
    for d in ALL {
        for _ in 0..200 {
            let value = |rng: &mut Rng| -> DbValue {
                match rng.below(6) {
                    0 => DbValue::Null,
                    1 => DbValue::int(rng.next_u64() as i64 as i128),
                    2 => DbValue::float((rng.below(1_000_000) as f64) / 7.0),
                    3 => {
                        let bytes: Vec<u8> =
                            (0..rng.below(8)).map(|_| rng.below(256) as u8).collect();
                        DbValue::bytes(&bytes, CellLimit::DEFAULT)
                    }
                    4 => DbValue::Bool(rng.chance(50)),
                    _ => text_value(&text(rng, d != SqlDialect::Postgres)),
                }
            };
            let changes = DbEditChanges {
                updates: vec![DbEditUpdate {
                    key: vec![cell("id", DbValue::int(rng.below(1000) as i128 + 1))],
                    values: (0..12)
                        .map(|i| cell(&format!("c{i}"), value(&mut rng)))
                        .collect(),
                }],
                inserts: vec![DbEditInsert {
                    values: (0..3)
                        .map(|i| cell(&format!("c{i}"), value(&mut rng)))
                        .collect(),
                }],
                deletes: vec![],
            };
            let out = build(d, &table(d, "t"), &ids(), &changes)
                .unwrap_or_else(|e| panic!("seed {seed}: {e:?}"));
            for statement in &out {
                assert_eq!(
                    substitute(d, &statement.sql, &statement.params),
                    statement.preview,
                    "seed {seed}"
                );
            }
        }
    }
}

/// 바인드 글의 자리 표시를 왼쪽부터 한 번 훑어 리터럴로 바꾼다(바꾼 리터럴을 다시 보지 않는다 — 값에 `$1` · `?` 가 들 수 있다).
/// 바인드 글에는 값이 없고 이 시험의 이름에는 자리 표시 글자가 없다.
fn substitute(d: SqlDialect, sql: &str, params: &[DbValue]) -> String {
    let literals: Vec<String> = params.iter().map(|v| literal(d, v).unwrap()).collect();
    let mut out = String::new();
    let mut next = literals.iter();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match (d, c) {
            (SqlDialect::Postgres, '$') => {
                let mut n = String::new();
                while let Some(digit) = chars.next_if(char::is_ascii_digit) {
                    n.push(digit);
                }
                let n: usize = n.parse().unwrap();
                out.push_str(&literals[n - 1]);
            }
            (SqlDialect::Mysql | SqlDialect::Sqlite, '?') => out.push_str(next.next().unwrap()),
            _ => out.push(c),
        }
    }
    out
}

/// 편집 DML 은 분류기에서 쓰기를 넘지 않고, 글 하나가 문장 하나다 — 이름이 문장을 끝내려 해도 따옴표 안에 머문다.
#[test]
fn edit_statements_are_single_writes_even_with_hostile_names() {
    let names = [
        "t",
        "t\"; DROP TABLE x; --",
        "t`; DROP TABLE x; --",
        "t'; DROP TABLE x; --",
        "t]; DROP TABLE x",
        "$1",
        "?",
        "한글 테이블",
    ];
    for d in ALL {
        for name in names {
            let mut t = table(d, name);
            if t.schema.is_some() {
                t.schema = Some(name.into());
            }
            if t.database.is_some() {
                t.database = Some(name.into());
            }
            let changes = DbEditChanges {
                updates: vec![DbEditUpdate {
                    key: vec![cell(name, DbValue::int(1))],
                    values: vec![cell(&format!("{name}2"), text_value("'; DROP TABLE t; --"))],
                }],
                inserts: vec![DbEditInsert {
                    values: vec![cell(name, DbValue::int(2))],
                }],
                deletes: vec![DbEditDelete {
                    key: vec![cell(name, DbValue::int(3))],
                }],
            };
            let out = build(d, &t, &[name.to_owned()], &changes).unwrap();
            for statement in &out {
                let mut texts = vec![statement.sql.clone(), statement.preview.clone()];
                if let Some(p) = &statement.probe {
                    texts.push(p.sql.clone());
                }
                for text in texts {
                    assert_eq!(split(d, &text).len(), 1, "{d:?}: {text}");
                    let grade = classify_statement(d, &text).grade;
                    assert!(grade <= RiskGrade::Write, "{d:?} {grade:?}: {text}");
                }
            }
        }
    }
}

fn reason(d: SqlDialect, t: &ObjectRef, key: &[&str], changes: DbEditChanges) -> DbEditError {
    let key: Vec<String> = key.iter().map(|k| (*k).to_owned()).collect();
    build(d, t, &key, &changes).unwrap_err()
}

fn update(key: Vec<DbEditCell>, values: Vec<DbEditCell>) -> DbEditChanges {
    DbEditChanges {
        updates: vec![DbEditUpdate { key, values }],
        ..DbEditChanges::default()
    }
}

#[test]
fn changes_that_cannot_name_one_row_are_refused() {
    let d = SqlDialect::Postgres;
    let t = table(d, "t");
    let set = || vec![cell("name", text_value("x"))];
    let at = DbEditChangeRef {
        kind: DbEditChangeKind::Update,
        index: 0,
    };

    assert_eq!(
        reason(d, &t, &[], scenario()).reason,
        DbEditErrorReason::NoRowKey
    );
    assert_eq!(
        reason(d, &t, &["id"], DbEditChanges::default()).reason,
        DbEditErrorReason::NoChanges
    );
    // 키 열이 빠짐 · 남음 · 두 번 · NULL.
    let e = reason(d, &t, &["id", "k"], update(key(1), set()));
    assert_eq!(e.reason, DbEditErrorReason::KeyMismatch);
    assert_eq!(e.change, Some(at));
    let extra = vec![cell("id", DbValue::int(1)), cell("name", text_value("a"))];
    assert_eq!(
        reason(d, &t, &["id"], update(extra, set())).reason,
        DbEditErrorReason::KeyMismatch
    );
    let twice = vec![cell("id", DbValue::int(1)), cell("id", DbValue::int(2))];
    assert_eq!(
        reason(d, &t, &["id", "k"], update(twice, set())).reason,
        DbEditErrorReason::DuplicateColumn
    );
    let null = reason(
        d,
        &t,
        &["id"],
        update(vec![cell("id", DbValue::Null)], set()),
    );
    assert_eq!(
        (null.reason, null.column.as_deref()),
        (DbEditErrorReason::NullKey, Some("id"))
    );
    // 키 열을 고친다 · 바꿀 열이 없다 · 같은 열 두 번.
    let e = reason(
        d,
        &t,
        &["id"],
        update(key(1), vec![cell("id", DbValue::int(9))]),
    );
    assert_eq!(
        (e.reason, e.column.as_deref()),
        (DbEditErrorReason::KeyColumnChanged, Some("id"))
    );
    assert_eq!(
        reason(d, &t, &["id"], update(key(1), vec![])).reason,
        DbEditErrorReason::NoValues
    );
    let dup = vec![cell("name", text_value("a")), cell("name", text_value("b"))];
    assert_eq!(
        reason(d, &t, &["id"], update(key(1), dup)).reason,
        DbEditErrorReason::DuplicateColumn
    );
    // 같은 행을 수정과 삭제가 · 두 수정이 가리킨다.
    let both = DbEditChanges {
        updates: vec![DbEditUpdate {
            key: key(1),
            values: set(),
        }],
        deletes: vec![DbEditDelete { key: key(1) }],
        ..DbEditChanges::default()
    };
    let e = reason(d, &t, &["id"], both);
    assert_eq!(e.reason, DbEditErrorReason::DuplicateRow);
    assert_eq!(e.change, Some(at));
    let twice = DbEditChanges {
        updates: vec![
            DbEditUpdate {
                key: key(1),
                values: set(),
            },
            DbEditUpdate {
                key: key(1),
                values: set(),
            },
        ],
        ..DbEditChanges::default()
    };
    assert_eq!(
        reason(d, &t, &["id"], twice).reason,
        DbEditErrorReason::DuplicateRow
    );
    // 빈 열 이름.
    assert_eq!(
        reason(
            d,
            &t,
            &["id"],
            update(key(1), vec![cell("", text_value("x"))])
        )
        .reason,
        DbEditErrorReason::EmptyName
    );
    // 상한.
    let many = DbEditChanges {
        deletes: (0..=MAX_EDIT_CHANGES as i64)
            .map(|i| DbEditDelete { key: key(i) })
            .collect(),
        ..DbEditChanges::default()
    };
    assert_eq!(
        reason(d, &t, &["id"], many).reason,
        DbEditErrorReason::TooManyChanges
    );
}

#[test]
fn tables_must_be_qualified_where_the_session_could_move_them() {
    let changes = || DbEditChanges {
        deletes: vec![DbEditDelete { key: key(1) }],
        ..DbEditChanges::default()
    };
    let named = |database: Option<&str>, schema: Option<&str>| ObjectRef {
        database: database.map(str::to_owned),
        schema: schema.map(str::to_owned),
        name: "t".into(),
    };
    let refused = |d, t: ObjectRef| reason(d, &t, &["id"], changes()).reason;
    // PG: DB 와 스키마 둘 다(DB 는 연결과 다르면 서버가 거절한다 — 메타데이터를 읽은 DB 와 같은 DB 에만 쓴다).
    assert_eq!(
        refused(SqlDialect::Postgres, named(None, Some("public"))),
        DbEditErrorReason::UnqualifiedTable
    );
    assert_eq!(
        refused(SqlDialect::Postgres, named(Some("app"), None)),
        DbEditErrorReason::UnqualifiedTable
    );
    // MySQL: DB(`USE` 에 기대지 않는다). 스키마 칸에 와도 받되 둘이 다르면 거절.
    assert_eq!(
        refused(SqlDialect::Mysql, named(None, None)),
        DbEditErrorReason::UnqualifiedTable
    );
    assert_eq!(
        refused(SqlDialect::Mysql, named(Some("a"), Some("b"))),
        DbEditErrorReason::UnqualifiedTable
    );
    let out = build(
        SqlDialect::Mysql,
        &named(None, Some("app")),
        &ids(),
        &changes(),
    )
    .unwrap();
    assert_eq!(out[0].sql, "DELETE FROM `app`.`t` WHERE `id` = ?");
    assert_eq!(
        refused(SqlDialect::Sqlite, named(Some("a"), Some("b"))),
        DbEditErrorReason::UnqualifiedTable
    );
    // 빈 이름 · NUL.
    assert_eq!(
        refused(SqlDialect::Sqlite, named(Some(""), None)),
        DbEditErrorReason::EmptyName
    );
    assert_eq!(
        refused(SqlDialect::Postgres, named(Some("app"), Some("a\0b"))),
        DbEditErrorReason::Nul
    );
}

/// 리터럴로 적을 수 없는 값은 바인드로 들어가더라도 거절한다 — 사람이 본 글과 서버가 받은 값이 같아야 한다.
#[test]
fn values_without_a_faithful_literal_are_refused_at_their_cell() {
    let truncated = DbValue::Truncated(DbTruncated {
        of: TruncatedKind::Text,
        prefix: "abc".into(),
        size: 9_000_000,
        type_name: None,
    });
    let cut_bytes = DbValue::Bytes(DbBytes {
        hex: "0xab".into(),
        size: 9,
        truncated: true,
    });
    let cases = [
        (
            SqlDialect::Postgres,
            truncated,
            DbEditErrorReason::Truncated,
        ),
        (SqlDialect::Sqlite, cut_bytes, DbEditErrorReason::Truncated),
        (
            SqlDialect::Mysql,
            DbValue::Array(vec![DbValue::int(1)]),
            DbEditErrorReason::Unsupported,
        ),
        (
            SqlDialect::Postgres,
            text_value("a\0b"),
            DbEditErrorReason::Nul,
        ),
    ];
    for (d, value, want) in cases {
        let e = reason(
            d,
            &table(d, "t"),
            &["id"],
            update(key(1), vec![cell("v", value.clone())]),
        );
        assert_eq!((e.reason, e.column.as_deref()), (want, Some("v")), "{d:?}");
        // 키 값이어도 · 새 행이어도.
        let e = reason(
            d,
            &table(d, "t"),
            &["id"],
            DbEditChanges {
                deletes: vec![DbEditDelete {
                    key: vec![cell("id", value.clone())],
                }],
                ..DbEditChanges::default()
            },
        );
        assert_eq!(e.reason, want, "{d:?}");
        let e = reason(
            d,
            &table(d, "t"),
            &["id"],
            DbEditChanges {
                inserts: vec![DbEditInsert {
                    values: vec![cell("v", value)],
                }],
                ..DbEditChanges::default()
            },
        );
        assert_eq!(e.reason, want, "{d:?}");
    }
}

#[test]
fn the_wire_shape_is_camel_case_with_optional_lists() {
    let changes: DbEditChanges = serde_json::from_value(serde_json::json!({
        "deletes": [{ "key": [{ "column": "id", "value": { "int": 1 } }] }]
    }))
    .unwrap();
    assert_eq!(changes.len(), 1);
    assert!(changes.updates.is_empty() && changes.inserts.is_empty());
    let e = DbEditError::at(
        DbEditChangeRef {
            kind: DbEditChangeKind::Update,
            index: 2,
        },
        Some("id"),
        DbEditErrorReason::KeyColumnChanged,
    );
    assert_eq!(
        serde_json::to_value(e).unwrap(),
        serde_json::json!({ "change": { "kind": "update", "index": 2 }, "column": "id", "reason": "keyColumnChanged" })
    );
}
