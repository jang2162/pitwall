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
        // MCP 세션(결정 M5-D27) — DB 툴은 MCP 세션용 입구를 따로 지난다(M5-06).
        ConnectionScope::Agent,
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

/// 겹방어: 분류기가 `SET` · 트랜잭션 제어를 조회로 잘못 매겨도 read-only 관문은 거절한다(결정 M3-D6 — 읽기 전용 트랜잭션은 첫 `SET` 으로
/// 풀린다).
#[test]
fn read_only_refuses_settings_even_when_graded_as_reads() {
    let ro = policy(true, false);
    for sql in ["SET transaction_read_only = off", "BEGIN"] {
        let mut statements = planned(SqlDialect::Postgres, sql);
        statements[0].grade = RiskGrade::Read;
        statements[0].reasons.clear();
        let e = judge(&ro, &statements, &[], "h", None).unwrap_err();
        assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{sql}");
    }
}

/// M3-03b 리뷰 M1: 서버가 고를 수 있는 읽기를 분류기가 읽지 못한 조회(MySQL `NO_BACKSLASH_ESCAPES` 읽기의 `INTO OUTFILE`)는
/// read-only 연결에서 조회가 아니다 — MySQL 의 읽기 전용 트랜잭션은 `INTO OUTFILE` · `INTO @변수` 를 막지 않는다. 실은 문장은 파괴 ·
/// `unparsed`. 보통 연결은 등급(조회)대로 확인 없이 지난다.
#[test]
fn read_only_refuses_a_query_with_a_reading_the_classifier_could_not_read() {
    let sql = r#"SELECT * FROM t WHERE name = 'a\' OR id = 1 INTO OUTFILE "/tmp/x" -- '"#;
    let statements = planned(SqlDialect::Mysql, sql);
    assert_eq!(statements[0].grade, RiskGrade::Read);
    assert!(statements[0].unread_reading);
    let e = judge(&policy(true, false), &statements, &[], "h", None).unwrap_err();
    assert_eq!(e.code, rpc_code::PERMISSION_DENIED);
    let data = e.data.unwrap();
    assert_eq!(data["reason"], "readOnlyConnection");
    assert_eq!(data["statements"][0]["grade"], "destructive");
    assert_eq!(data["statements"][0]["reasons"], json!(["unparsed"]));
    judge(&policy(false, false), &statements, &[], "h", None).unwrap();
    // 뒤의 읽기를 읽은 조회는 read-only 에서도 돈다.
    let read = planned(
        SqlDialect::Mysql,
        r"SELECT * FROM t WHERE c = 'x\' OR 1=1 -- '",
    );
    judge(&policy(true, false), &read, &[], "h", None).unwrap();
}

/// M3-03b 리뷰 M2: 글 읽기를 바꾸는 문장은 확인으로 풀리지 않는 거절이다(`changesSqlReading`). 같은 글의 다른 문장도 돌지 않는다.
#[test]
fn statements_that_change_how_text_is_read_are_refused() {
    for (dialect, sql) in [
        (
            SqlDialect::Postgres,
            "SET standard_conforming_strings = off; DELETE FROM t WHERE name = 'a\\'' OR true --'",
        ),
        (
            SqlDialect::Postgres,
            "SELECT set_config('standard_conforming_strings', 'off', false)",
        ),
        (SqlDialect::Mysql, "SET sql_mode = 'NO_BACKSLASH_ESCAPES'"),
        (SqlDialect::Mysql, "SET NAMES gbk"),
    ] {
        let statements = planned(dialect, sql);
        let e = refuse_reading_changes(&statements).unwrap_err();
        assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{sql}");
        let data = e.data.unwrap();
        assert_eq!(data["reason"], "changesSqlReading", "{sql}");
        assert_eq!(data["statements"].as_array().unwrap().len(), 1, "{sql}");
    }
    refuse_reading_changes(&planned(
        SqlDialect::Postgres,
        "SET search_path = x; SELECT 1",
    ))
    .unwrap();
}

/// 에이전트의 증거는 관문이 판정한 연결 id · 워크트리 · 대상에만 쓰인다 — 판정과 열기 사이에 설정이 바뀌면(다른 대상) 열지 않는다
/// (16-m5-plan M5-06). 창의 증거는 늘 지난다.
#[test]
fn an_agent_grant_is_used_only_for_the_judged_connection() {
    let target = |host: &str| agent::DbTarget {
        driver: pw_db_types::DriverKind::Postgres,
        host: Some(host.into()),
        port: Some(5432),
        database: Some("app".into()),
        user: None,
        tunnel: None,
        file: None,
    };
    let grant = DbGrant {
        project_id: "p".into(),
        agent: Some(Box::new(agent::AgentBinding {
            connection_id: "dev".into(),
            name: "Dev".into(),
            worktree: Some("/wt".into()),
            target: target("localhost"),
            marks: agent::DbMarks::default(),
            approval: agent::AgentApproval::NotNeeded,
        })),
    };
    assert!(grant.is_agent());
    grant
        .check_agent_use(
            Some("/wt"),
            Some("dev"),
            &target("localhost"),
            agent::DbMarks::default(),
        )
        .unwrap();
    for (worktree, id, host) in [
        (Some("/wt"), Some("dev"), "elsewhere"),
        (Some("/wt"), Some("other"), "localhost"),
        (None, Some("dev"), "localhost"),
        (Some("/wt"), None, "localhost"),
    ] {
        let e = grant
            .check_agent_use(worktree, id, &target(host), agent::DbMarks::default())
            .unwrap_err();
        assert_eq!(e.data.unwrap()["reason"], "notOpenedByUser");
    }
    let window = db_grant(&ConnectionScope::Project("p".into()), "p").unwrap();
    assert!(!window.is_agent());
    window
        .check_agent_use(
            None,
            Some("any"),
            &target("elsewhere"),
            agent::DbMarks::default(),
        )
        .unwrap();
}

/// 프로덕션 `ask` 로 지난 증거(16-m5-plan M5-18 ④)는 이 호출의 승인(`Approved` — 계획 해시가 맞은 표)을 달기 전에는 연결을 여는
/// 자리 · 스키마 캐시를 지나지 못한다. 툴이 승인을 잊어도 열리지 않는다(fail-closed).
#[test]
fn a_production_ask_grant_opens_nothing_until_the_call_is_approved() {
    use pw_core::registry::plan::{ApprovalTicket, ToolPlan, approve};
    let target = agent::DbTarget {
        driver: pw_db_types::DriverKind::Postgres,
        host: Some("prod".into()),
        port: Some(5432),
        database: Some("app".into()),
        user: None,
        tunnel: None,
        file: None,
    };
    let prod_marks = agent::DbMarks {
        production: true,
        read_only: false,
    };
    let mut grant = DbGrant {
        project_id: "p".into(),
        agent: Some(Box::new(agent::AgentBinding {
            connection_id: "prod".into(),
            name: "Prod".into(),
            worktree: None,
            target: target.clone(),
            marks: agent::DbMarks {
                production: true,
                read_only: false,
            },
            approval: agent::AgentApproval::Required,
        })),
    };
    assert!(grant.needs_approval());
    let e = grant
        .check_agent_use(None, Some("prod"), &target, prod_marks)
        .unwrap_err();
    assert_eq!(e.code, pw_rpc::code::INTERNAL_ERROR, "{e:?}");
    let plan = ToolPlan::builder("db.tool.query", pw_rpc::Tier::Read, &json!({"x": 1})).build();
    let approved = approve(
        Some(&ApprovalTicket {
            hash: plan.hash.clone(),
        }),
        plan,
    )
    .unwrap();
    grant.approve(&approved);
    assert!(!grant.needs_approval());
    grant
        .check_agent_use(None, Some("prod"), &target, prod_marks)
        .unwrap();
}

/// 판정 뒤 그 대상에 표지가 더해졌으면(승인 뒤 · 연결 전에 설정을 바꿔 끼움) 연결을 여는 자리가 열지 않는다 — 프로덕션이면
/// `productionBlocked`, read-only 면 `readOnlyConnection`. 판정 때와 같거나 느슨한 표지는 지난다(16-m5-plan M5-18).
#[test]
fn marks_added_after_the_gate_judged_refuse_the_open() {
    let target = agent::DbTarget {
        driver: pw_db_types::DriverKind::Postgres,
        host: Some("db".into()),
        port: Some(5432),
        database: Some("app".into()),
        user: None,
        tunnel: None,
        file: None,
    };
    let grant = DbGrant {
        project_id: "p".into(),
        agent: Some(Box::new(agent::AgentBinding {
            connection_id: "dev".into(),
            name: "Dev".into(),
            worktree: None,
            target: target.clone(),
            marks: agent::DbMarks::default(),
            approval: agent::AgentApproval::NotNeeded,
        })),
    };
    let marks = |production, read_only| agent::DbMarks {
        production,
        read_only,
    };
    grant
        .check_agent_use(None, Some("dev"), &target, marks(false, false))
        .unwrap();
    for (m, why) in [
        (marks(true, false), "productionBlocked"),
        (marks(false, true), "readOnlyConnection"),
        (marks(true, true), "productionBlocked"),
    ] {
        let e = grant
            .check_agent_use(None, Some("dev"), &target, m)
            .unwrap_err();
        assert_eq!(e.data.unwrap()["reason"], why, "{m:?}");
    }
}

/// 대상의 호스트는 소문자로, 포트는 드라이버 기본으로 맞춘다 — 표지를 대소문자 · 적지 않은 포트로 비껴가지 못한다. TLS 설정은
/// 대상이 아니다.
#[test]
fn targets_ignore_host_case_default_ports_and_tls() {
    let target = |value: serde_json::Value| {
        agent::DbTarget::of(
            &crate::config::from_file_value(&value).unwrap(),
            std::path::Path::new("/r"),
        )
    };
    let a = target(json!({ "name": "a", "driver": "postgres", "host": "DB.Example." }));
    let b = target(
        json!({ "name": "b", "driver": "postgres", "host": "db.example",
                           "port": 5432, "sslmode": "disable" }),
    );
    assert_eq!(a, b);
    let c = target(json!({ "name": "c", "driver": "postgres", "host": "db.example", "user": "x" }));
    assert_ne!(a, c, "another user is another target");
}

/// MySQL · MariaDB 의 표지 열쇠는 DB 이름만 뺀다 — 터널 목적지 · 사용자가 다르면 다른 열쇠다(메인 판정 2026-11-14). PG 는 DB 를 둔다.
#[test]
fn mysql_mark_keys_drop_only_the_database() {
    let target = |value: serde_json::Value| {
        agent::DbTarget::of(
            &crate::config::from_file_value(&value).unwrap(),
            std::path::Path::new("/r"),
        )
    };
    let my = |db: &str, tunnel: &str, user: &str| {
        target(
            json!({ "name": "m", "driver": "mysql", "host": "db.example", "database": db,
                       "user": user, "ssh_tunnel": { "host_ref": tunnel } }),
        )
    };
    let base = my("app", "bastion-a", "root");
    assert_eq!(base.mark_key(), my("other", "bastion-a", "root").mark_key());
    assert_ne!(base.mark_key(), my("app", "bastion-b", "root").mark_key());
    assert_ne!(base.mark_key(), my("app", "bastion-a", "reader").mark_key());
    // 「사람이 연 연결」 의 열쇠(대상 전체)는 DB 도 본다.
    assert_ne!(base, my("other", "bastion-a", "root"));
    let pg = |db: &str| {
        target(json!({ "name": "p", "driver": "postgres", "host": "db.example", "database": db }))
    };
    assert_ne!(pg("app").mark_key(), pg("other").mark_key());
}

/// Redis 의 표지 열쇠도 DB 번호를 뺀다(메인 판정 2026-11-17 — M5-19): 같은 번호의 다른 표기(`"00"` · `"+0"` · 번호를 뺀 설정)도,
/// 다른 번호도 같은 열쇠다. 사용자 · 포트가 다르면 다른 열쇠이고, 「사람이 연 연결」 의 열쇠(대상 전체)는 번호도 본다.
#[test]
fn redis_mark_keys_drop_the_database_number() {
    let redis = |db: Option<&str>, user: Option<&str>, port: u16| {
        let mut value =
            json!({ "name": "r", "driver": "redis", "host": "cache.example", "port": port });
        if let Some(db) = db {
            value["database"] = json!(db);
        }
        if let Some(user) = user {
            value["user"] = json!(user);
        }
        agent::DbTarget::of(
            &crate::config::from_file_value(&value).unwrap(),
            std::path::Path::new("/r"),
        )
    };
    let base = redis(Some("0"), None, 6379);
    for same in [
        redis(None, None, 6379),
        redis(Some("00"), None, 6379),
        redis(Some("+0"), None, 6379),
        redis(Some("7"), None, 6379),
    ] {
        assert_eq!(base.mark_key(), same.mark_key());
        assert_eq!(base.mark_key(), base.with_database(Some("00")).mark_key());
    }
    assert_ne!(
        base.mark_key(),
        redis(Some("0"), Some("reader"), 6379).mark_key()
    );
    assert_ne!(base.mark_key(), redis(Some("0"), None, 6380).mark_key());
    assert_ne!(base, redis(Some("7"), None, 6379));
}

/// Redis 의 표지 열쇠는 사용자를 적지 않은 것 · 빈 글 · `default` 를 같은 사용자로 본다(메인 판정 2026-11-17 — M5-19 리뷰 R1).
/// `Default`(대소문자) · 다른 사용자는 다르다. 「사람이 연 연결」 의 열쇠(대상 전체)는 그대로 가른다.
#[test]
fn redis_mark_keys_treat_the_default_user_spellings_as_one() {
    let redis = |user: Option<&str>| {
        let mut value = json!({ "name": "r", "driver": "redis", "host": "cache.example" });
        if let Some(user) = user {
            value["user"] = json!(user);
        }
        agent::DbTarget::of(
            &crate::config::from_file_value(&value).unwrap(),
            std::path::Path::new("/r"),
        )
    };
    let none = redis(None).mark_key();
    assert_eq!(none, redis(Some("")).mark_key());
    assert_eq!(none, redis(Some("default")).mark_key());
    assert_ne!(none, redis(Some("Default")).mark_key());
    assert_ne!(none, redis(Some("reader")).mark_key());
    assert_ne!(redis(None), redis(Some("default")));
    // MySQL 의 사용자는 그대로다.
    let my = |user: &str| {
        agent::DbTarget::of(
            &crate::config::from_file_value(
                &json!({ "name": "m", "driver": "mysql", "host": "db", "user": user }),
            )
            .unwrap(),
            std::path::Path::new("/r"),
        )
        .mark_key()
    };
    assert_ne!(my(""), my("default"));
}
