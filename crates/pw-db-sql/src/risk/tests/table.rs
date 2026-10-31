//! 14-m3-plan.md M3-03 행의 완료 기준(다섯 등급 × 세 방언 표 · 파싱 실패 10개 · 부작용 함수 · 항진식)과 「고칠 것」 ⑤ 의 시험이
//! 없던 갈래.

use pw_db_types::RiskGrade::{self, Ddl, Destructive, Read, UnboundedWrite, Write};
use pw_db_types::SqlDialect::{self, Mysql, Postgres, Sqlite};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::tokenizer::Tokenizer;

use crate::risk::RiskReason::{self, *};
use crate::risk::{Classification, Found, classify_statement, leading_keyword_floor};

#[track_caller]
fn expect(dialect: SqlDialect, sql: &str, grade: RiskGrade, reasons: &[RiskReason]) {
    assert_eq!(
        classify_statement(dialect, sql),
        Classification {
            grade,
            reasons: reasons.to_vec()
        },
        "{dialect:?}: {sql}"
    );
}

#[test]
fn every_grade_has_three_examples_in_each_dialect() {
    let table: &[(RiskGrade, SqlDialect, &[&str])] = &[
        (
            Read,
            Postgres,
            &[
                "SELECT * FROM users WHERE id = 1",
                "WITH x AS (SELECT 1) SELECT * FROM x",
                "EXPLAIN SELECT * FROM t",
                "SHOW search_path",
            ],
        ),
        (
            Read,
            Mysql,
            &[
                "SELECT * FROM users",
                "SHOW TABLES",
                "DESCRIBE users",
                "SELECT lower(name) FROM t",
            ],
        ),
        (
            Read,
            Sqlite,
            &[
                "SELECT * FROM users",
                "SELECT count(*) FROM t WHERE a > 1",
                "VALUES (1, 2)",
                "SELECT * FROM t ORDER BY a LIMIT 10",
            ],
        ),
        (
            Write,
            Postgres,
            &[
                "INSERT INTO t (a) VALUES (1)",
                "UPDATE t SET a = 1 WHERE id = 7",
                "DELETE FROM t WHERE id = 7",
                "SELECT nextval('s')",
            ],
        ),
        (
            Write,
            Mysql,
            &[
                "INSERT INTO t VALUES (1)",
                "UPDATE t SET a = 1 WHERE id = 7",
                "DELETE FROM t WHERE id = 7 LIMIT 1",
                "SELECT GET_LOCK('x', 1)",
            ],
        ),
        (
            Write,
            Sqlite,
            &[
                "INSERT INTO t VALUES (1)",
                "UPDATE t SET a = 1 WHERE id = 7",
                "DELETE FROM t WHERE id = 7",
                "INSERT OR REPLACE INTO t VALUES (1)",
            ],
        ),
        (
            UnboundedWrite,
            Postgres,
            &[
                "UPDATE t SET a = 1",
                "DELETE FROM t",
                "DELETE FROM t WHERE 1 = 1",
                "UPDATE t SET a = 1 WHERE id IN (SELECT id FROM u)",
            ],
        ),
        (
            UnboundedWrite,
            Mysql,
            &[
                "UPDATE t SET a = 1",
                "DELETE FROM t",
                "UPDATE t SET a = 1 WHERE TRUE",
            ],
        ),
        (
            UnboundedWrite,
            Sqlite,
            &[
                "UPDATE t SET a = 1",
                "DELETE FROM t",
                "DELETE FROM t WHERE a = a",
                "UPDATE OR REPLACE t SET a = 1 WHERE id = 1",
            ],
        ),
        (
            Ddl,
            Postgres,
            &[
                "CREATE TABLE t (id int)",
                "ALTER TABLE t ADD COLUMN b int",
                "CREATE INDEX i ON t (a)",
                "GRANT SELECT ON t TO r",
            ],
        ),
        (
            Ddl,
            Mysql,
            &[
                "CREATE TABLE t (id INT)",
                "ALTER TABLE t ADD COLUMN b INT",
                "RENAME TABLE a TO b",
                "CREATE VIEW v AS SELECT 1",
            ],
        ),
        (
            Ddl,
            Sqlite,
            &[
                "CREATE TABLE t (id INTEGER)",
                "CREATE INDEX i ON t (a)",
                "ALTER TABLE t RENAME TO u",
                "CREATE VIEW v AS SELECT 1",
            ],
        ),
        (
            Destructive,
            Postgres,
            &[
                "DROP TABLE t",
                "TRUNCATE t",
                "DROP SCHEMA s CASCADE",
                "ALTER TABLE t DROP COLUMN a",
            ],
        ),
        (
            Destructive,
            Mysql,
            &["DROP TABLE t", "TRUNCATE TABLE t", "DROP DATABASE d"],
        ),
        (
            Destructive,
            Sqlite,
            &["DROP TABLE t", "DROP INDEX i", "DROP VIEW v"],
        ),
    ];
    for (grade, dialect, sqls) in table {
        assert!(sqls.len() >= 3);
        for sql in *sqls {
            let classification = classify_statement(*dialect, sql);
            assert_eq!(classification.grade, *grade, "{dialect:?}: {sql}");
            // 조회 밖의 등급은 까닭이 있다 — 다이얼로그가 등급과 함께 보인다.
            assert_eq!(
                classification.reasons.is_empty(),
                *grade == Read,
                "{dialect:?}: {sql} {classification:?}"
            );
        }
    }
}

#[test]
fn parse_failures_are_destructive_and_unparsed() {
    let failures: &[(SqlDialect, &str)] = &[
        // 끊긴 문장
        (Postgres, "SELECT * FROM"),
        (Postgres, "DELETE FROM t WHERE"),
        (Postgres, "DROP TABLE"),
        (Mysql, "UPDATE t SET a ="),
        (Sqlite, "INSERT INTO t VALUES (1"),
        (Sqlite, "SELECT 'unterminated"),
        // 방언에 없는(파서가 모르는) 문법
        (Postgres, "DO $$ BEGIN DROP TABLE t; END $$"),
        (Postgres, "SELECT * FROM t FOR NO KEY UPDATE"),
        (Postgres, "REFRESH MATERIALIZED VIEW v"),
        (Mysql, "SELECT * FROM t LOCK IN SHARE MODE"),
        (Mysql, "LOAD DATA INFILE '/tmp/x' INTO TABLE t"),
        (Sqlite, "PRAGMA journal_mode = WAL"),
    ];
    assert!(failures.len() >= 10);
    for (dialect, sql) in failures {
        expect(*dialect, sql, Destructive, &[Unparsed]);
    }
}

#[test]
fn deep_nesting_is_unparsed_without_overflowing_the_stack() {
    let sql = format!("SELECT {}1{}", "(".repeat(5_000), ")".repeat(5_000));
    expect(Postgres, &sql, Destructive, &[Unparsed]);
}

#[test]
fn side_effect_reads_are_writes() {
    let calls: &[(SqlDialect, &str)] = &[
        (Postgres, "SELECT nextval('s')"),
        (Postgres, "SELECT setval('s', 1)"),
        (Postgres, "SELECT pg_sleep(1)"),
        (Postgres, "SELECT pg_sleep_for('1 second')"),
        (Postgres, "SELECT pg_catalog.pg_sleep(1)"),
        (Postgres, "SELECT dblink('host=x', 'SELECT 1')"),
        (Postgres, "SELECT dblink_exec('host=x', 'DELETE FROM t')"),
        (Postgres, "SELECT set_config('search_path', 'x', false)"),
        (Postgres, "SELECT lo_export(1, '/tmp/x')"),
        (Postgres, "SELECT lo_import('/etc/passwd')"),
        (Postgres, "SELECT pg_read_file('/etc/passwd')"),
        (Postgres, "SELECT pg_advisory_lock(1)"),
        (Postgres, "SELECT pg_advisory_xact_lock(1)"),
        (Postgres, "SELECT pg_try_advisory_lock(1)"),
        (Postgres, "SELECT * FROM t WHERE id = (SELECT nextval('s'))"),
        (Postgres, "WITH x AS (SELECT pg_sleep(1)) SELECT * FROM x"),
        (
            Postgres,
            "SELECT * FROM dblink('host=x', 'DELETE FROM t') AS r(a int)",
        ),
        (Mysql, "SELECT SLEEP(1)"),
        (Mysql, "SELECT GET_LOCK('a', 1)"),
        (Mysql, "SELECT RELEASE_LOCK('a')"),
        (Mysql, "SELECT BENCHMARK(10, 1)"),
        (Mysql, "SELECT LOAD_FILE('/etc/passwd')"),
        (Sqlite, "SELECT load_extension('evil')"),
    ];
    for (dialect, sql) in calls {
        expect(*dialect, sql, Write, &[SideEffectFunction]);
    }
    // 이름이 비슷한 열 · 함수는 그대로 조회다.
    for sql in [
        "SELECT lower('x')",
        "SELECT sleeper FROM t",
        "SELECT my_lo_thing(1)",
        "SELECT 'pg_sleep(1)'",
    ] {
        expect(Postgres, sql, Read, &[]);
    }
}

#[test]
fn server_admin_functions_are_unbounded_writes_wherever_they_hide() {
    // PG 는 `BEGIN READ ONLY` 안에서도 이것들을 막지 않는다 — 분류가 첫 방어다(`risk.rs` 의 `SERVER_ADMIN_FUNCTIONS`).
    for sql in [
        "SELECT pg_terminate_backend(1)",
        "SELECT pg_cancel_backend(1)",
        "SELECT pg_reload_conf()",
        "SELECT pg_rotate_logfile()",
        // 대소문자 · 스키마 한정
        "SELECT PG_TERMINATE_BACKEND(1)",
        "SELECT pg_catalog.pg_terminate_backend(1)",
        "SELECT \"pg_catalog\".\"pg_terminate_backend\"(1)",
        // 서브쿼리 · CTE · 함수 인자 · FROM · WHERE · ORDER BY · 집합 연산
        "SELECT * FROM t WHERE id = (SELECT pg_terminate_backend(1))",
        "SELECT * FROM t WHERE EXISTS (SELECT pg_cancel_backend(pid) FROM pg_stat_activity)",
        "WITH k AS (SELECT pg_terminate_backend(pid) FROM pg_stat_activity) SELECT count(*) FROM k",
        "SELECT coalesce(pg_terminate_backend(1), false)",
        "SELECT * FROM (SELECT pg_terminate_backend(pid) FROM pg_stat_activity) AS x",
        "SELECT * FROM pg_stat_activity, LATERAL (SELECT pg_terminate_backend(pid)) AS k",
        "SELECT pid FROM pg_stat_activity WHERE pg_terminate_backend(pid)",
        "SELECT pid FROM pg_stat_activity ORDER BY pg_cancel_backend(pid)",
        "SELECT 1 UNION ALL SELECT pg_reload_conf()::int",
        "SELECT nextval('s'), pg_terminate_backend(1)",
        // 조회가 아닌 문장 안
        "INSERT INTO log SELECT pg_terminate_backend(pid) FROM pg_stat_activity",
        "EXPLAIN ANALYZE SELECT pg_terminate_backend(1)",
    ] {
        expect(Postgres, sql, UnboundedWrite, &[ServerAdminFunction]);
    }
    // 돌리지 않는 EXPLAIN 과 문자열 · 열 이름은 조회다.
    for sql in [
        "EXPLAIN SELECT pg_terminate_backend(1)",
        "SELECT 'pg_terminate_backend(1)'",
        "SELECT pg_terminate_backend_count FROM stats",
    ] {
        expect(Postgres, sql, Read, &[]);
    }
}

#[test]
fn completion_criteria_examples() {
    expect(
        Postgres,
        "SELECT nextval('s')",
        Write,
        &[SideEffectFunction],
    );
    expect(Postgres, "SELECT pg_sleep(1)", Write, &[SideEffectFunction]);
    expect(
        Postgres,
        "SELECT * FROM t FOR UPDATE",
        Write,
        &[LockingRead],
    );
    expect(
        Postgres,
        "UPDATE t SET a=1 WHERE 1=1",
        UnboundedWrite,
        &[UnboundedWhere],
    );
    expect(
        Postgres,
        "UPDATE t SET a=1",
        UnboundedWrite,
        &[MissingWhere],
    );
}

#[test]
fn locking_clauses_inside_subqueries_are_writes() {
    // 맨 바깥 조회의 잠금은 AST(`query.locks`)가, 안쪽 조회의 잠금은 토큰 검사가 잡는다.
    expect(
        Postgres,
        "SELECT * FROM (SELECT * FROM t FOR UPDATE) AS x",
        Write,
        &[LockingRead],
    );
    expect(
        Postgres,
        "SELECT * FROM t WHERE id IN (SELECT id FROM u FOR SHARE)",
        Write,
        &[LockingRead],
    );
    expect(
        Postgres,
        "SELECT * FROM t FOR UPDATE SKIP LOCKED",
        Write,
        &[LockingRead],
    );
    expect(Mysql, "SELECT * FROM t FOR UPDATE", Write, &[LockingRead]);
    for sql in [
        "SELECT 'for update' FROM t",
        "SELECT \"for\", \"update\" FROM t",
        "SELECT 1 /* for update */",
    ] {
        expect(Postgres, sql, Read, &[]);
    }
}

#[test]
fn session_and_transaction_statements_are_writes() {
    for (dialect, sql) in [
        (Postgres, "BEGIN"),
        (Postgres, "COMMIT"),
        (Postgres, "ROLLBACK"),
        (Postgres, "SAVEPOINT a"),
        (Postgres, "RELEASE SAVEPOINT a"),
        (Postgres, "ROLLBACK TO SAVEPOINT a"),
        (Postgres, "SET TRANSACTION READ WRITE"),
        (Mysql, "START TRANSACTION READ ONLY"),
        (Mysql, "COMMIT"),
        (Sqlite, "BEGIN IMMEDIATE"),
        (Sqlite, "COMMIT"),
    ] {
        expect(dialect, sql, Write, &[TransactionControl]);
    }
    for (dialect, sql) in [
        (Postgres, "SET search_path = x"),
        (Postgres, "SET ROLE admin"),
        (Postgres, "RESET ALL"),
        (Postgres, "DISCARD ALL"),
        (Mysql, "SET autocommit = 0"),
        (Mysql, "SET @a = 1"),
        (Mysql, "USE other_db"),
    ] {
        expect(dialect, sql, Write, &[SessionState]);
    }
}

#[test]
fn lock_call_maintenance_attach_pragma_and_copy() {
    expect(
        Postgres,
        "LOCK TABLE t IN ACCESS EXCLUSIVE MODE",
        Write,
        &[Lock],
    );
    expect(Mysql, "LOCK TABLES t WRITE", Write, &[Lock]);
    expect(Mysql, "UNLOCK TABLES", Write, &[Lock]);

    expect(Postgres, "CALL p()", Write, &[Procedural]);
    expect(Mysql, "CALL p(1)", Write, &[Procedural]);
    expect(Postgres, "EXECUTE p", Write, &[Procedural]);
    // 준비한 문장은 `EXECUTE` 때 무엇인지 모르므로 준비할 때 안의 문장으로 매긴다.
    expect(
        Postgres,
        "PREPARE p AS DELETE FROM t",
        UnboundedWrite,
        &[MissingWhere],
    );
    expect(Postgres, "PREPARE q AS SELECT 1", Write, &[Procedural]);

    expect(Postgres, "VACUUM t", Write, &[Maintenance]);
    expect(Postgres, "ANALYZE t", Write, &[Maintenance]);
    expect(Sqlite, "VACUUM", Write, &[Maintenance]);

    expect(
        Sqlite,
        "ATTACH DATABASE 'other.db' AS other",
        Write,
        &[Attach],
    );
    // DBX 의 키워드 경로가 허용하던 안전 pragma 도 쓰기다(읽는 꼴과 바꾸는 꼴이 같은 이름이다).
    expect(Sqlite, "PRAGMA user_version", Write, &[Pragma]);
    expect(Sqlite, "PRAGMA foreign_keys = 1", Write, &[Pragma]);

    expect(Postgres, "COPY t FROM '/tmp/x.csv'", Write, &[DataTransfer]);
    expect(Postgres, "COPY t TO STDOUT", Write, &[DataTransfer]);
    expect(
        Postgres,
        "COPY (SELECT * FROM t) TO PROGRAM 'cat > /tmp/x'",
        Destructive,
        &[CopyProgram],
    );
    expect(
        Postgres,
        "COPY t FROM PROGRAM 'curl http://x'",
        Destructive,
        &[CopyProgram],
    );
}

#[test]
fn statements_the_parser_cannot_read_stay_destructive() {
    // 「고칠 것」 ③ 의 「쓰기 이상」 가운데 sqlparser 0.63 이 읽지 못하는 꼴 — fail-closed 로 파괴 · `unparsed` 다.
    for (dialect, sql) in [
        (Postgres, "DO $$ BEGIN PERFORM 1; END $$"),
        (Mysql, "LOAD DATA INFILE '/tmp/x' INTO TABLE t"),
        (Mysql, "LOAD DATA LOCAL INFILE '/tmp/x' INTO TABLE t"),
        (Mysql, "SELECT * FROM t INTO OUTFILE '/tmp/x'"),
        (Mysql, "SELECT 1 /*!50000 INTO OUTFILE '/tmp/x' */"),
        (Sqlite, "VACUUM INTO '/tmp/copy.db'"),
        (Sqlite, "DETACH DATABASE other"),
        (Sqlite, "PRAGMA table_info(t)"),
    ] {
        expect(dialect, sql, Destructive, &[Unparsed]);
    }
}

#[test]
fn mysql_and_mariadb_executable_comments_are_read() {
    expect(Mysql, "/*!50000 DROP TABLE t */", Destructive, &[Drop]);
    expect(
        Mysql,
        "/*! DELETE FROM users */",
        UnboundedWrite,
        &[MissingWhere],
    );
    // MariaDB 는 `/*M! … */` 도 실행한다 — 파서는 이것을 주석으로 읽으므로 펼쳐서 읽는다.
    expect(Mysql, "/*M!100100 DROP TABLE t */", Destructive, &[Drop]);
    expect(
        Mysql,
        "SELECT 1;\n/*M!100100 DELETE FROM t */",
        UnboundedWrite,
        &[MissingWhere],
    );
    // 앞에 여러 바이트 글자가 있어도 같은 자리를 펼친다.
    expect(
        Mysql,
        "SELECT '한글'; /*M! DROP TABLE t */",
        Destructive,
        &[Drop],
    );
    // 일반 주석 · 문자열 안은 실행되지 않는다. PG · SQLite 에는 실행 주석이 없다.
    expect(Mysql, "SELECT 1 /* DROP TABLE t */", Read, &[]);
    expect(Mysql, "SELECT '/*M! DROP TABLE t */'", Read, &[]);
    expect(Postgres, "SELECT 1 /*!50000 DROP TABLE t */", Read, &[]);
    expect(Sqlite, "SELECT 1 /*M! DROP TABLE t */", Read, &[]);
    // MySQL 의 `@변수 :=` 대입은 세션을 바꾼다. PG 의 `:=` 는 이름 붙은 인자다.
    expect(Mysql, "SELECT @a := 1", Write, &[SessionState]);
    expect(Postgres, "SELECT f(x := 1)", Read, &[]);
}

#[test]
fn explain_analyze_grades_the_inner_statement() {
    expect(Postgres, "EXPLAIN DELETE FROM t", Read, &[]);
    expect(
        Postgres,
        "EXPLAIN ANALYZE DELETE FROM t",
        UnboundedWrite,
        &[MissingWhere],
    );
    expect(
        Postgres,
        "EXPLAIN (ANALYZE) DELETE FROM t",
        UnboundedWrite,
        &[MissingWhere],
    );
    expect(
        Postgres,
        "EXPLAIN (ANALYZE, BUFFERS) UPDATE t SET a = 1 WHERE id = 1",
        Write,
        &[Dml],
    );
    expect(
        Postgres,
        "EXPLAIN ANALYZE SELECT pg_sleep(1)",
        Write,
        &[SideEffectFunction],
    );
    expect(Postgres, "EXPLAIN (FORMAT JSON) DELETE FROM t", Read, &[]);
}

#[test]
fn several_statements_take_the_highest_grade_and_its_reasons() {
    expect(
        Postgres,
        "SELECT 1; UPDATE t SET a = 1 WHERE id = 1; DROP TABLE t",
        Destructive,
        &[Drop],
    );
    expect(
        Postgres,
        "DELETE FROM t; TRUNCATE u",
        Destructive,
        &[Truncate],
    );
    expect(
        Postgres,
        "DROP TABLE t; TRUNCATE u",
        Destructive,
        &[Drop, Truncate],
    );
    expect(
        Postgres,
        "BEGIN; UPDATE t SET a = 1; COMMIT",
        UnboundedWrite,
        &[MissingWhere],
    );
    // 한 문장의 파싱 실패가 글 전체를 파괴로 만든다(나눠 문장마다 매기는 것은 M3-05 의 `db.sql.classify`).
    expect(Postgres, "SELECT 1; SELEC 2", Destructive, &[Unparsed]);
}

#[test]
fn empty_text_runs_nothing_and_is_read() {
    for sql in ["", "   ", "-- just a note", "/* block */", ";"] {
        expect(Postgres, sql, Read, &[]);
    }
}

#[test]
fn dropping_columns_is_destructive() {
    expect(
        Postgres,
        "ALTER TABLE t DROP COLUMN a",
        Destructive,
        &[Drop],
    );
    expect(Mysql, "ALTER TABLE t DROP COLUMN a", Destructive, &[Drop]);
    expect(
        Postgres,
        "ALTER TABLE t ADD COLUMN a int",
        Ddl,
        &[RiskReason::Ddl],
    );
}

#[test]
fn the_leading_keyword_floor_raises_drop_and_ddl_verbs() {
    // AST 갈래 목록이 모르는 꼴이 와도 첫 낱말이 바닥을 정한다. 목록이 지금은 모든 꼴을 아는 것이라 함수로 직접 본다.
    let floor = |sql: &str| {
        let tokens = Tokenizer::new(&PostgreSqlDialect {}, sql)
            .tokenize()
            .unwrap();
        let mut found = Found::default();
        leading_keyword_floor(&tokens, &mut found);
        found.finish()
    };
    assert_eq!(
        floor("DROP SOMETHING_NEW x"),
        Classification {
            grade: Destructive,
            reasons: vec![Drop]
        }
    );
    assert_eq!(
        floor("SELECT 1; truncate x"),
        Classification {
            grade: Destructive,
            reasons: vec![Truncate]
        }
    );
    assert_eq!(
        floor("CREATE SOMETHING_NEW x"),
        Classification {
            grade: Ddl,
            reasons: vec![RiskReason::Ddl]
        }
    );
    assert_eq!(
        floor("SELECT 'DROP'; \"DROP\" x"),
        Classification {
            grade: Read,
            reasons: vec![]
        }
    );
}

#[test]
fn classification_serializes_in_camel_case() {
    let value =
        serde_json::to_value(classify_statement(Postgres, "DELETE FROM t WHERE 1 = 1")).unwrap();
    assert_eq!(
        value,
        serde_json::json!({ "grade": "unboundedWrite", "reasons": ["unboundedWhere"] })
    );
    assert_eq!(
        serde_json::to_value(RiskReason::Unparsed).unwrap(),
        "unparsed"
    );
}
