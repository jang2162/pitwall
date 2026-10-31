use pw_db_sql::edit::{DbEditCell, DbEditDelete, DbEditUpdate};
use pw_db_types::{CellLimit, DbValue};

use super::*;

fn policy(read_only: bool, production: bool) -> Policy {
    Policy {
        read_only,
        production,
        name: "prod".into(),
    }
}

fn planned(sql: &str) -> Vec<Planned> {
    plan(SqlDialect::Postgres, sql, None).unwrap()
}

/// 편집 DML 이 관문의 분류에서 쓰기를 넘으면 거절한다 — 문장 둘 · DDL · 파괴 · 트랜잭션 제어 · 세션 설정.
#[test]
fn only_single_writes_pass_as_edit_statements() {
    let ok = r#"UPDATE "app"."public"."t" SET "v" = $1 WHERE "id" = $2"#;
    assert_eq!(
        check_planned(ok, planned(ok)).unwrap().grade,
        RiskGrade::Write
    );
    for refused in [
        "UPDATE t SET v = 1 WHERE id = 1; DROP TABLE t",
        "DROP TABLE t",
        "ALTER TABLE t ADD c int",
        "UPDATE t SET v = 1",
        "COMMIT",
        "SET search_path = x",
    ] {
        let e = check_planned(refused, planned(refused)).unwrap_err();
        assert_eq!(e.code, rpc_code::INVALID_PARAMS, "{refused}");
        assert_eq!(e.data.unwrap()["reason"], "notAWrite", "{refused}");
    }
}

/// 등급 정책은 관문의 `judge` 다: read-only 는 거절, 프로덕션의 쓰기는 미리보기 해시를 확인으로 받는다. 쓰기를 넘는 문장은 그
/// 확인으로 지나지 못한다(쓰기까지만 싣는다).
#[test]
fn the_gate_takes_the_preview_as_the_confirmation_of_a_write_only() {
    let write = planned(r#"UPDATE "t" SET "v" = $1 WHERE "id" = $2"#);
    assert!(
        gate_policy(
            &policy(false, false),
            "dbs-1#0",
            SqlDialect::Postgres,
            &write
        )
        .is_ok()
    );
    assert!(
        gate_policy(
            &policy(false, true),
            "dbs-1#0",
            SqlDialect::Postgres,
            &write
        )
        .is_ok()
    );
    let e = gate_policy(
        &policy(true, false),
        "dbs-1#0",
        SqlDialect::Postgres,
        &write,
    )
    .unwrap_err();
    assert_eq!(e.code, rpc_code::PERMISSION_DENIED);
    for above in ["DELETE FROM t", "ALTER TABLE t ADD c int", "DROP TABLE t"] {
        for production in [false, true] {
            let e = gate_policy(
                &policy(false, production),
                "dbs-1#0",
                SqlDialect::Postgres,
                &planned(above),
            )
            .unwrap_err();
            assert_eq!(e.code, code::CONFIRM_REQUIRED, "{above}");
            assert_eq!(e.data.unwrap()["rejected"], "gradeTooLow", "{above}");
        }
    }
}

fn statements(value: &str) -> Vec<EditStatement> {
    let changes = DbEditChanges {
        updates: vec![DbEditUpdate {
            key: vec![DbEditCell {
                column: "id".into(),
                value: DbValue::int(1),
            }],
            values: vec![DbEditCell {
                column: "v".into(),
                value: DbValue::text(value, CellLimit::DEFAULT),
            }],
        }],
        deletes: vec![DbEditDelete {
            key: vec![DbEditCell {
                column: "id".into(),
                value: DbValue::int(2),
            }],
        }],
        ..DbEditChanges::default()
    };
    build(SqlDialect::Postgres, &table("t"), &["id".into()], &changes).unwrap()
}

fn table(name: &str) -> ObjectRef {
    ObjectRef {
        database: Some("app".into()),
        schema: Some("public".into()),
        name: name.into(),
    }
}

/// `previewHash` 는 세션(과 그 확인 순번) · 방언 · 적용 방식 · 테이블 · 문장 글 · 바인드 값 바이트에 묶인다.
#[test]
fn the_preview_hash_binds_session_table_statements_and_values() {
    let hash = |key: &str, d, applies, t: &ObjectRef, s: &[EditStatement]| {
        preview_hash(key, d, applies, t, s)
    };
    let own = DbEditApplies::OwnTransaction;
    let base = hash(
        "dbs-1#0",
        SqlDialect::Postgres,
        own,
        &table("t"),
        &statements("a"),
    );
    assert_eq!(
        base,
        hash(
            "dbs-1#0",
            SqlDialect::Postgres,
            own,
            &table("t"),
            &statements("a")
        )
    );
    let others = [
        hash(
            "dbs-2#0",
            SqlDialect::Postgres,
            own,
            &table("t"),
            &statements("a"),
        ),
        hash(
            "dbs-1#1",
            SqlDialect::Postgres,
            own,
            &table("t"),
            &statements("a"),
        ),
        hash(
            "dbs-1#0",
            SqlDialect::Mysql,
            own,
            &table("t"),
            &statements("a"),
        ),
        hash(
            "dbs-1#0",
            SqlDialect::Postgres,
            DbEditApplies::OpenTransaction,
            &table("t"),
            &statements("a"),
        ),
        hash(
            "dbs-1#0",
            SqlDialect::Postgres,
            own,
            &table("u"),
            &statements("a"),
        ),
        hash(
            "dbs-1#0",
            SqlDialect::Postgres,
            own,
            &table("t"),
            &statements("b"),
        ),
        hash(
            "dbs-1#0",
            SqlDialect::Postgres,
            own,
            &table("t"),
            &statements("a")[..1],
        ),
    ];
    for (i, other) in others.iter().enumerate() {
        assert_ne!(&base, other, "case {i}");
    }
    // 같은 글자라도 값의 종류(바이트)가 다르면 다르다.
    let mut as_json = statements("1");
    as_json[1].params[0] = DbValue::Json("1".into());
    assert_ne!(
        hash(
            "dbs-1#0",
            SqlDialect::Postgres,
            own,
            &table("t"),
            &statements("1")
        ),
        hash("dbs-1#0", SqlDialect::Postgres, own, &table("t"), &as_json)
    );
}
