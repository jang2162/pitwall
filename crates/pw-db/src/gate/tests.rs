use pw_db_types::RiskGrade;

use super::*;

fn planned(dialect: SqlDialect, sql: &str) -> Vec<Planned> {
    plan(dialect, sql, None).unwrap()
}

fn policy(read_only: bool, production: bool) -> Policy {
    Policy {
        read_only,
        production,
        name: "prod pg".into(),
    }
}

fn confirm_data(e: &RpcError) -> DbConfirmRequired {
    assert_eq!(e.code, code::CONFIRM_REQUIRED, "{e:?}");
    serde_json::from_value(e.data.clone().unwrap()).unwrap()
}

fn confirmed(e: &RpcError, name: Option<&str>) -> DbConfirmed {
    let data = confirm_data(e);
    DbConfirmed {
        grade: data.grade,
        statements_hash: data.statements_hash,
        name: name.map(str::to_owned),
    }
}

#[test]
fn only_the_project_window_gets_a_grant() {
    assert_eq!(
        db_grant(&ConnectionScope::Project("p".into()), "p")
            .unwrap()
            .project_id(),
        "p"
    );
    for scope in [
        ConnectionScope::Project("other".into()),
        ConnectionScope::Host,
    ] {
        let e = db_grant(&scope, "p").unwrap_err();
        assert_eq!(e.code, rpc_code::PERMISSION_DENIED);
        assert_eq!(e.data.unwrap()["reason"], "notProjectWindow");
    }
}

/// 관문은 연결의 드라이버 방언으로 분류한다 — 같은 글이 방언에 따라 등급이 갈린다.
#[test]
fn the_same_text_grades_by_dialect() {
    let text = "/*! DROP TABLE t */ SELECT 1";
    assert_eq!(
        grade_of(&planned(SqlDialect::Mysql, text)),
        RiskGrade::Destructive
    );
    assert_eq!(
        grade_of(&planned(SqlDialect::Postgres, text)),
        RiskGrade::Read
    );
}

#[test]
fn plan_picks_the_selected_statements_in_order() {
    let sql = "SELECT 1; UPDATE t SET a = 1 WHERE id = 2; DELETE FROM t";
    let all = planned(SqlDialect::Postgres, sql);
    assert_eq!(all.len(), 3);
    assert_eq!(grade_of(&all), RiskGrade::UnboundedWrite);
    let picked = plan(SqlDialect::Postgres, sql, Some(&[1, 0, 1])).unwrap();
    assert_eq!(picked.iter().map(|s| s.index).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(picked[1].text, "UPDATE t SET a = 1 WHERE id = 2");
    assert_eq!(grade_of(&picked), RiskGrade::Write);
    assert!(plan(SqlDialect::Postgres, sql, Some(&[3])).is_err());
    assert!(plan(SqlDialect::Postgres, "-- nothing", None).is_err());
}

/// read-only 연결은 조회 밖을 모두 거절한다 — `SET` · `RESET` · `SET TRANSACTION` 과 트랜잭션 제어, 파싱 실패 포함.
#[test]
fn read_only_refuses_everything_but_queries() {
    let ro = policy(true, false);
    for sql in [
        "DELETE FROM t WHERE id = 1",
        "SET transaction_read_only = off",
        "SET TRANSACTION READ WRITE",
        "RESET ALL",
        "BEGIN",
        "COMMIT",
        "SELEC oops",
        "SELECT 1; UPDATE t SET a = 1 WHERE id = 1",
    ] {
        let statements = planned(SqlDialect::Postgres, sql);
        let hash = statements_hash(Some("s"), SqlDialect::Postgres, &statements, &[]);
        let e = judge(&ro, &statements, &[], &hash, None).unwrap_err();
        assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{sql}");
        assert_eq!(e.data.unwrap()["reason"], "readOnlyConnection", "{sql}");
    }
    let statements = planned(SqlDialect::Postgres, "SELECT * FROM t");
    judge(&ro, &statements, &[], "h", None).unwrap();
}

/// 결정 M3-D20 의 정책: 조회 · 쓰기는 그대로(프로덕션의 쓰기는 확인), 범위 없는 쓰기 · DDL · 파괴는 확인.
#[test]
fn grades_follow_the_confirmation_policy() {
    let dev = policy(false, false);
    let prod = policy(false, true);
    let run = |p: &Policy, sql: &str| {
        let statements = planned(SqlDialect::Postgres, sql);
        let hash = statements_hash(Some("s"), SqlDialect::Postgres, &statements, &[]);
        judge(p, &statements, &[], &hash, None)
    };
    run(&dev, "SELECT 1").unwrap();
    run(&dev, "UPDATE t SET a = 1 WHERE id = 1").unwrap();
    run(&prod, "SELECT 1").unwrap();
    for sql in [
        "DELETE FROM t",
        "CREATE TABLE x (id int)",
        "DROP TABLE t",
        "SELEC oops",
    ] {
        assert_eq!(
            run(&dev, sql).unwrap_err().code,
            code::CONFIRM_REQUIRED,
            "{sql}"
        );
    }
    let e = run(&prod, "UPDATE t SET a = 1 WHERE id = 1").unwrap_err();
    assert!(!confirm_data(&e).name_required);
    let e = run(&prod, "DROP TABLE t").unwrap_err();
    assert!(confirm_data(&e).name_required);
    assert!(!confirm_data(&run(&dev, "DROP TABLE t").unwrap_err()).name_required);
}

/// 확인은 그 SQL · 그 세션 · 그 효과에만 쓰인다: 해시가 다르거나 등급이 낮거나 이름이 틀리면 다시 3001.
#[test]
fn a_confirmation_binds_to_the_statements_session_and_name() {
    let dev = policy(false, false);
    let sql = "DELETE FROM t";
    let statements = planned(SqlDialect::Postgres, sql);
    let hash = statements_hash(Some("s1"), SqlDialect::Postgres, &statements, &[]);
    let asked = judge(&dev, &statements, &[], &hash, None).unwrap_err();
    let ok = confirmed(&asked, None);
    judge(&dev, &statements, &[], &hash, Some(&ok)).unwrap();

    // 한 글자 바꾼 SQL 은 해시가 다르다.
    let other = planned(SqlDialect::Postgres, "DELETE FROM u");
    let other_hash = statements_hash(Some("s1"), SqlDialect::Postgres, &other, &[]);
    let e = judge(&dev, &other, &[], &other_hash, Some(&ok)).unwrap_err();
    assert_eq!(confirm_data(&e).rejected.as_deref(), Some("hashMismatch"));
    // 다른 세션 · 다른 방언 · 효과가 붙으면 해시가 다르다.
    assert_ne!(
        hash,
        statements_hash(Some("s2"), SqlDialect::Postgres, &statements, &[])
    );
    assert_ne!(
        hash,
        statements_hash(None, SqlDialect::Postgres, &statements, &[])
    );
    assert_ne!(
        hash,
        statements_hash(Some("s1"), SqlDialect::Mysql, &statements, &[])
    );
    assert_ne!(
        hash,
        statements_hash(
            Some("s1"),
            SqlDialect::Postgres,
            &statements,
            &[DbTxEffect::ImplicitCommit]
        )
    );
    // 등급이 낮은 확인.
    let low = DbConfirmed {
        grade: RiskGrade::Write,
        ..ok.clone()
    };
    let e = judge(&dev, &statements, &[], &hash, Some(&low)).unwrap_err();
    assert_eq!(confirm_data(&e).rejected.as_deref(), Some("gradeTooLow"));

    // 프로덕션 파괴: 이름이 연결 이름과 같아야 한다.
    let prod = policy(false, true);
    let drop = planned(SqlDialect::Postgres, "DROP TABLE t");
    let drop_hash = statements_hash(Some("s1"), SqlDialect::Postgres, &drop, &[]);
    let asked = judge(&prod, &drop, &[], &drop_hash, None).unwrap_err();
    for name in [None, Some("prod"), Some("PROD PG")] {
        let e = judge(
            &prod,
            &drop,
            &[],
            &drop_hash,
            Some(&confirmed(&asked, name)),
        )
        .unwrap_err();
        assert_eq!(
            confirm_data(&e).rejected.as_deref(),
            Some("nameMismatch"),
            "{name:?}"
        );
    }
    judge(
        &prod,
        &drop,
        &[],
        &drop_hash,
        Some(&confirmed(&asked, Some("prod pg"))),
    )
    .unwrap();
}

#[test]
fn a_transaction_effect_needs_confirmation_even_for_plain_writes() {
    let dev = policy(false, false);
    let statements = planned(SqlDialect::Mysql, "LOCK TABLES t WRITE");
    assert_eq!(grade_of(&statements), RiskGrade::Write);
    assert!(statements[0].implicit_commit);
    let effects = [DbTxEffect::ImplicitCommit];
    let hash = statements_hash(Some("s"), SqlDialect::Mysql, &statements, &effects);
    let e = judge(&dev, &statements, &effects, &hash, None).unwrap_err();
    assert_eq!(confirm_data(&e).effects, effects);
    judge(&dev, &statements, &[], &hash, None).unwrap();
}

#[test]
fn untranslatable_transaction_statements_are_refused_before_anything_runs() {
    let statements = planned(SqlDialect::Postgres, "SELECT 1; PREPARE TRANSACTION 'x'");
    let e = refuse_untranslatable(&statements).unwrap_err();
    assert_eq!(e.code, rpc_code::INVALID_PARAMS);
    assert_eq!(e.data.unwrap()["index"], 1);
    refuse_untranslatable(&planned(SqlDialect::Postgres, "BEGIN; COMMIT AND CHAIN")).unwrap();
}

/// 분류는 큰 스택의 전용 스레드에서 돈다(블로킹 스레드풀의 2 MiB 가 아니다).
#[tokio::test]
async fn classification_runs_on_the_big_stack_thread() {
    let name = on_classifier_thread(|| std::thread::current().name().map(str::to_owned))
        .await
        .unwrap();
    assert_eq!(name.as_deref(), Some(CLASSIFIER_THREAD));
    // 2 MiB 를 넘는 스택을 쓴다 — 큰 스택이 아니면 프로세스가 끝난다.
    let deep = on_classifier_thread(|| {
        fn burn(n: usize) -> usize {
            let pad = std::hint::black_box([0_u8; 4096]);
            if n == 0 {
                pad[0] as usize
            } else {
                burn(n - 1) + pad[1] as usize
            }
        }
        burn(1500)
    })
    .await
    .unwrap();
    assert_eq!(deep, 0);
}
