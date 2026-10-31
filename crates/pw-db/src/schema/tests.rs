//! 키 판정 표(14-m3-plan.md M3-10 완료 기준 ③)와 목록 자르기.

use pw_db_types::{ColumnInfo, IndexInfo};

use super::{RowKey, row_key, slice};

fn col(name: &str, nullable: bool) -> ColumnInfo {
    ColumnInfo {
        name: name.to_owned(),
        db_type: "integer".to_owned(),
        nullable,
        default: None,
        comment: None,
    }
}

fn index(name: &str, columns: &[&str], primary: bool, unique: bool, partial: bool) -> IndexInfo {
    IndexInfo {
        name: name.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        primary,
        unique,
        partial,
    }
}

fn key(index: &str, columns: &[&str], primary: bool) -> Option<RowKey> {
    Some(RowKey {
        index: index.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        primary,
    })
}

/// 표 한 줄: (설명, 열, 인덱스, 기대 키).
type Case = (
    &'static str,
    Vec<ColumnInfo>,
    Vec<IndexInfo>,
    Option<RowKey>,
);

#[test]
fn the_row_key_table() {
    let cases: Vec<Case> = vec![
        (
            "PK",
            vec![col("id", false), col("email", true)],
            vec![index("t_pkey", &["id"], true, true, false)],
            key("t_pkey", &["id"], true),
        ),
        (
            "PK wins over a smaller NOT NULL unique index",
            vec![col("a", false), col("b", false), col("c", false)],
            vec![
                index("t_c_key", &["c"], false, true, false),
                index("t_pkey", &["a", "b"], true, true, false),
            ],
            key("t_pkey", &["a", "b"], true),
        ),
        (
            "unique index on NOT NULL columns",
            vec![col("tenant", false), col("code", false), col("note", true)],
            vec![index(
                "t_tenant_code",
                &["tenant", "code"],
                false,
                true,
                false,
            )],
            key("t_tenant_code", &["tenant", "code"], false),
        ),
        (
            "unique index with a nullable column is not a key",
            vec![col("tenant", false), col("email", true)],
            vec![index(
                "t_tenant_email",
                &["tenant", "email"],
                false,
                true,
                false,
            )],
            None,
        ),
        (
            "partial unique index (PG WHERE …) is not a key",
            vec![col("email", false)],
            vec![index("t_live_email", &["email"], false, true, true)],
            None,
        ),
        (
            "partial index skipped, the plain one taken",
            vec![col("email", false), col("id", false)],
            vec![
                index("t_live_email", &["email"], false, true, true),
                index("t_id", &["id"], false, true, false),
            ],
            key("t_id", &["id"], false),
        ),
        (
            "expression index (no such column) is not a key",
            vec![col("email", false)],
            vec![index(
                "t_lower_email",
                &["lower(email)"],
                false,
                true,
                false,
            )],
            None,
        ),
        (
            "a plain index is not a key",
            vec![col("id", false)],
            vec![index("t_id", &["id"], false, false, false)],
            None,
        ),
        ("no index: no key", vec![col("id", false)], vec![], None),
        (
            "of two NOT NULL unique indexes the narrower, then by name",
            vec![col("a", false), col("b", false), col("c", false)],
            vec![
                index("t_ab", &["a", "b"], false, true, false),
                index("t_z", &["c"], false, true, false),
                index("t_y", &["b"], false, true, false),
            ],
            key("t_y", &["b"], false),
        ),
    ];
    for (what, columns, indexes, expected) in cases {
        assert_eq!(row_key(&columns, &indexes), expected, "{what}");
    }
}

#[test]
fn lists_are_sliced_by_cursor_and_limit() {
    let items: Vec<u32> = (0..7).collect();
    let (first, next) = slice(&items, None, Some(3)).unwrap();
    assert_eq!(first, [0, 1, 2]);
    assert_eq!(next.as_deref(), Some("3"));
    let (rest, next) = slice(&items, next.as_deref(), Some(5)).unwrap();
    assert_eq!(rest, [3, 4, 5, 6]);
    assert_eq!(next, None);
    let (all, next) = slice(&items, None, None).unwrap();
    assert_eq!((all.len(), next), (7, None));
    assert!(slice(&items, Some("8"), None).is_err());
    assert!(slice(&items, Some("x"), None).is_err());
    assert!(slice(&items, None, Some(0)).is_err());
    assert!(slice(&items, None, Some(501)).is_err());
}
