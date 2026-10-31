//! DBX 에서 옮긴 시험 34개(시험마다 위에 `DBX:` 로 원래 이름). 원본 API 셋(`classify_sql_risk*` · `is_dangerous_sql_for_database` ·
//! `prove_read_only_for_database`)의 단언을 [`classify_statement`] 하나의 등급으로 바꿨다 — 등급 대응은 `risk.rs` 머리 주석.
//! DBX 의 `DatabaseType` 은 방언으로 옮겼다(PG 계열 → PG, Oracle · SQL Server · Salesforce · 검색 엔진은 Pitwall 에 연결이 없어
//! 그 글이 PG · MySQL 콘솔에 들어왔을 때로).
//!
//! 출처: DBX(<https://github.com/t8y2/dbx>) 커밋 `c5e7e0430653aaef198800a48f9dec46a7cfed5e` 의
//! `crates/dbx-sql-core/src/sql_risk.rs` 의 `mod tests` 를 옮겨 수정함. Apache License 2.0 — 고지는 `licenses/vendored.md` 「DBX」.
//!
//! 기대값을 바꾼 까닭은 여섯 중 하나다(시험 안에 [A]~[F] 로 적었다):
//! - [A] 파싱 실패는 파괴 · `unparsed`(fail-closed) — DBX 는 키워드 폴백으로 쓰기 · 조회를 답했다. 정확한 꼴 인식기(`recognize.rs`)가
//!   읽는 잠금절(`LOCK IN SHARE MODE` · `FOR KEY SHARE`)은 DBX 와 같은 쓰기다.
//! - [B] 다섯 갈래 — DBX 의 `Ddl` 에서 `DROP` · `TRUNCATE` 가 파괴로, `is_dangerous` 인 `UPDATE` · `DELETE` 가 범위 없는 쓰기로,
//!   `Transaction` 이 쓰기로 갈렸다. `is_dangerous` 였던 `INSERT` 꼴(upsert · `INSERT … SELECT`)은 쓰기다(F6-9 의 범위 없는
//!   쓰기는 `UPDATE` · `DELETE` 다).
//! - [C] 읽기 증명(`ReadProof`)은 옮기지 않았다 — 증명이 거절하던 목록 밖 함수 · 창 함수 · 여러 문장은 분류에서 조회다(목록 밖
//!   함수는 DB 수준 읽기 전용이 막는다, 결정 M3-D6). 같은 글을 분류의 기대값으로 단언한다.
//! - [D] 목록을 넓혔다 — `SLEEP` · `GET_LOCK` · `pg_sleep` · `lo_import` 가 든 조회는 쓰기다.
//! - [E] 다른 세션 · 서버에 닿는 관리 함수(`pg_terminate_backend` · `pg_cancel_backend` …)는 범위 없는 쓰기다(DBX 는 쓰기 + dangerous).
//!   인자의 SQL 글을 실행하는 함수(`dblink` …)도 같다 — DBX 는 `dblink` 를 부작용 함수(쓰기)로 봤다.
//! - [F] PG 의 `SELECT … INTO 표` 는 표를 만든다(`CREATE TABLE … AS` 와 같다) — DDL(DBX 는 쓰기).

use pw_db_types::RiskGrade::{self, Ddl, Destructive, Read, UnboundedWrite, Write};
use pw_db_types::SqlDialect::{self, Mysql, Postgres, Sqlite};

use crate::risk::{RiskReason, classify_statement};

#[track_caller]
fn expect(dialect: SqlDialect, sql: &str, grade: RiskGrade) {
    assert_eq!(
        classify_statement(dialect, sql).grade,
        grade,
        "{dialect:?}: {sql}"
    );
}

#[track_caller]
fn expect_all(dialect: SqlDialect, sqls: &[&str], grade: RiskGrade) {
    for sql in sqls {
        expect(dialect, sql, grade);
    }
}

#[track_caller]
fn expect_unparsed(dialect: SqlDialect, sql: &str) {
    let classification = classify_statement(dialect, sql);
    assert_eq!(
        (classification.grade, classification.reasons),
        (Destructive, vec![RiskReason::Unparsed]),
        "{dialect:?}: {sql}"
    );
}

// DBX: mysql_proof_accepts_plain_reads_and_allowlisted_functions
#[test]
fn mysql_plain_reads_are_read() {
    expect_all(
        Mysql,
        &[
            "SELECT * FROM users",
            "SELECT COUNT(*) FROM orders",
            "SELECT id, name FROM users WHERE id IN (1, 2, 3)",
            "SELECT id, name FROM users WHERE id IN (SELECT user_id FROM roles) ORDER BY created_at LIMIT 10",
            "SELECT 'update' FROM t",
            "SELECT a FROM t UNION ALL SELECT b FROM u",
            "WITH totals AS (SELECT COUNT(*) AS n FROM orders) SELECT n FROM totals",
            "SELECT CONCAT(first_name, ' ', last_name) AS full_name FROM users",
            "SELECT id FROM users ORDER BY rand() LIMIT 1",
            "SHOW TABLES",
            "SHOW CREATE TABLE users",
            "EXPLAIN SELECT * FROM users",
            "DESC users",
            "DESCRIBE users",
            "-- only a comment header\nSELECT id FROM users",
            "SELECT 1",
        ],
        Read,
    );
}

// DBX: mysql_proof_rejects_session_writes_locking_and_unknown_functions
#[test]
fn mysql_session_writes_locking_and_side_effects() {
    expect_all(
        Mysql,
        &[
            "SELECT @a := 1",
            "SELECT 1 INTO @current_id",
            "SELECT * FROM users FOR UPDATE",
            "SELECT * FROM users FOR SHARE",
            // [D]
            "SELECT SLEEP(10)",
            "SELECT GET_LOCK('x', 1)",
            "SELECT id FROM users WHERE EXISTS (SELECT SLEEP(1))",
            "WITH x AS (SELECT SLEEP(1)) SELECT 1",
            "SET autocommit = 1",
        ],
        Write,
    );
    // 인식기가 잠금절을 `FOR SHARE` 로 읽는다(DBX 와 같은 쓰기).
    expect(Mysql, "SELECT * FROM users LOCK IN SHARE MODE", Write);
    // [A] 파서가 `INTO OUTFILE` 을 읽지 못한다.
    expect_unparsed(Mysql, "SELECT * FROM users INTO OUTFILE '/tmp/x'");
    expect_unparsed(Mysql, "SELEC * FORM users");
    expect_unparsed(Mysql, "SELECT id FROM users INTO @current_id LIMIT 1");
    // [C] 목록 밖 함수 · 창 함수 · 여러 조회 · 조회의 EXPLAIN ANALYZE 는 조회다.
    expect_all(
        Mysql,
        &[
            "SELECT LAST_INSERT_ID()",
            "SELECT my_custom_udf(1)",
            "SELECT secret_schema.fn(1) FROM t",
            "SELECT ROW_NUMBER() OVER (ORDER BY id) FROM users",
            "SELECT id FROM users; SELECT 1",
            "EXPLAIN ANALYZE SELECT * FROM users",
        ],
        Read,
    );
}

// DBX: postgres_proof_accepts_plain_reads_and_rejects_side_effects
#[test]
fn postgres_plain_reads_and_side_effects() {
    expect_all(
        Postgres,
        &[
            "SELECT * FROM users",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "SELECT * FROM t WHERE id IN (SELECT id FROM u)",
            "SELECT coalesce(a, b) FROM t",
            "SELECT split_part(email, '@', 2) FROM users",
            "EXPLAIN SELECT 1",
            "SELECT 1",
        ],
        Read,
    );
    expect_all(
        Postgres,
        &[
            "WITH w AS (INSERT INTO t VALUES (1) RETURNING *) SELECT * FROM w",
            "SELECT * FROM t FOR UPDATE",
            "SELECT * FROM t FOR SHARE",
            "SELECT nextval('seq')",
            "SELECT setval('seq', 1)",
            // [D]
            "SELECT pg_sleep(1)",
            "SELECT lo_import('/etc/passwd')",
        ],
        Write,
    );
    // [F]
    expect(Postgres, "SELECT * INTO new_t FROM t", Ddl);
    // sqlparser 는 PG 의 `FOR NO KEY UPDATE` · `FOR KEY SHARE` 를 읽지 못한다 — 인식기가 `FOR UPDATE` · `FOR SHARE` 로 읽는다.
    expect(Postgres, "SELECT * FROM t FOR NO KEY UPDATE", Write);
    expect(Postgres, "SELECT * FROM t FOR KEY SHARE", Write);
    // [C]
    expect_all(
        Postgres,
        &[
            "EXPLAIN ANALYZE SELECT 1",
            "SELECT my_udf()",
            "SELECT id, row_number() OVER () FROM t",
            "SELECT 1; SELECT 2",
        ],
        Read,
    );
}

// DBX: proof_visitor_covers_from_subqueries_ctes_lateral_and_order_by
#[test]
fn side_effect_visitor_covers_from_subqueries_ctes_lateral_and_order_by() {
    // DBX 의 분류(`visit_expressions`)는 FROM 서브쿼리를 건너뛰었다 — 여기서는 조회 전체를 본다.
    expect_all(
        Postgres,
        &[
            "SELECT * FROM (SELECT pg_sleep(1)) AS x",
            "SELECT * FROM t ORDER BY pg_sleep(1)",
            "SELECT a FROM t, LATERAL (SELECT nextval('s') FROM u) d",
        ],
        Write,
    );
    // [E] `dblink` 는 SQL 글을 실행하는 함수 — 새 연결에서 돌아 읽기 전용 트랜잭션 밖이다.
    expect(
        Postgres,
        "SELECT * FROM dblink('host=x', 'DELETE FROM t') AS r(a int)",
        UnboundedWrite,
    );
    expect(Mysql, "SELECT * FROM (SELECT SLEEP(1)) AS x", Write);
    expect(Mysql, "SELECT * FROM t ORDER BY SLEEP(1)", Write);
    // [C] 목록 밖 함수(`my_udf` · `generate_series` · `md5`)는 조회다.
    expect_all(
        Postgres,
        &[
            "SELECT a FROM t, LATERAL (SELECT my_udf() FROM u) d",
            "SELECT * FROM generate_series(1, 10)",
            "SELECT * FROM (SELECT 1) AS x ORDER BY md5(id)",
        ],
        Read,
    );
}

// DBX: proof_maps_gaussdb_family_to_postgres_proof
#[test]
fn postgres_family_sql_uses_the_postgres_dialect() {
    // GaussDB · openGauss 연결은 Pitwall 에 없다 — 같은 글을 PG 방언으로.
    expect_all(
        Postgres,
        &["SELECT 1", "SELECT * FROM users", "EXPLAIN SELECT * FROM t"],
        Read,
    );
    // [B] WHERE 없는 UPDATE.
    expect(Postgres, "UPDATE users SET x = 1", UnboundedWrite);
    expect(Postgres, "SELECT nextval('seq')", Write);
}

// DBX: proof_only_applies_to_enabled_dialects
#[test]
fn oracle_and_doris_sql_in_supported_dialects() {
    // Oracle · Doris 연결은 Pitwall 에 없다 — 같은 글이 PG · MySQL 콘솔에 들어왔을 때.
    expect(Postgres, "SELECT 1", Read);
    expect(Mysql, "SELECT 1", Read);
    expect(Postgres, "SELECT LENGTH(1) FROM dual", Read);
    // [A] PG 방언은 `@` 데이터베이스 링크를 읽지 못한다. MySQL 방언은 읽기로 읽는다 — 어느 쪽이든 쓰기가 숨지 않는다.
    expect_unparsed(Postgres, "SELECT * FROM t@dblink");
    expect(Mysql, "SELECT * FROM t@dblink", Read);
}

// DBX: oracle_proof_keeps_the_guard_rails_the_ast_cannot_see
#[test]
fn oracle_guard_rail_inputs() {
    // [A]
    expect_unparsed(Postgres, "SELECT * FROM t@dblink");
    expect_unparsed(Postgres, "SELECT * FROM DUAL@dblink");
    // [F] PG 에서는 표 `v` 를 만든다.
    expect(Postgres, "SELECT COUNT(*) INTO v FROM t", Ddl);
    // [C] 시퀀스 의사 열(`seq.NEXTVAL`)은 함수 호출이 아니라 열 이름으로 읽힌다 — Oracle 연결이 없으므로 PG 콘솔에서는 없는 열이다.
    expect_all(
        Postgres,
        &[
            "SELECT seq.NEXTVAL FROM dual",
            "SELECT seq.CURRVAL FROM dual",
            "WITH cte AS (SELECT 1 AS a FROM dual) SELECT * FROM cte",
            "SELECT 1 FROM dual UNION SELECT 2 FROM dual",
            "SELECT 1; SELECT 2",
        ],
        Read,
    );
    expect(Postgres, "SELECT * FROM t FOR UPDATE", Write);
    // [A] PG 방언에 `MINUS` 가 없다.
    expect_unparsed(Postgres, "SELECT 1 FROM dual MINUS SELECT 2 FROM dual");
}

// DBX: oracle_proof_accepts_reviewed_builtins_only
#[test]
fn oracle_builtin_reads() {
    expect_all(
        Postgres,
        &[
            "SELECT LENGTH(1) FROM dual",
            "SELECT COUNT(*) FROM users",
            "SELECT UPPER(name) FROM users",
            "SELECT NVL(name, 'x') FROM users",
            "SELECT TO_CHAR(SYSDATE, 'YYYY-MM-DD') FROM dual",
            "SELECT * FROM t WHERE id IN (1, 2, 3)",
            "SELECT * FROM t WHERE (a = 1 OR b = 2)",
            "SELECT CAST(a AS VARCHAR2(10)) FROM t",
            "SELECT * FROM (SELECT * FROM users)",
            "SELECT SYSDATE, ROWNUM, USER FROM dual",
            "SELECT CURRENT_DATE FROM dual",
            "SELECT CURRENT_TIME FROM dual",
            "SELECT CURRENT_TIMESTAMP FROM dual",
            "SELECT LOCALTIME FROM dual",
            "SELECT LOCALTIMESTAMP FROM dual",
            "SELECT CURRENT_TIME(3) FROM dual",
            // [C]
            "SELECT my_udf(a) FROM t",
            "SELECT my_pkg.fn(a) FROM t",
            "SELECT SYS_CONTEXT('USERENV', 'SESSION_USER') FROM dual",
            "SELECT SYS_GUID() FROM dual",
            "SELECT ROW_NUMBER() OVER (ORDER BY a) FROM t",
            "SELECT * FROM TABLE(my_func())",
            "SELECT LISTAGG(a, ',') WITHIN GROUP (ORDER BY a) FROM t",
        ],
        Read,
    );
    // [A] Oracle 전용 문법.
    for sql in [
        "SELECT * FROM t AS OF TIMESTAMP SYSTIMESTAMP - INTERVAL '1' HOUR",
        "SELECT * FROM t VERSIONS BETWEEN TIMESTAMP SYSTIMESTAMP - INTERVAL '1' HOUR AND SYSTIMESTAMP",
    ] {
        expect_unparsed(Postgres, sql);
    }
}

// DBX: oracle_proof_rejects_statements_hidden_behind_a_hash_identifier
#[test]
fn hash_text_does_not_hide_a_following_statement() {
    // `#` 는 Oracle 에서 이름 글자, PG 에서 XOR 연산자, MySQL 에서 줄 주석이다. 어느 방언으로 읽든 서버가 돌릴 쓰기가 조회로
    // 나오지 않는다 — PG 는 뒤 문장을 그대로 보고, MySQL 은 서버처럼 줄 끝까지 주석으로 버린다.
    expect_all(
        Postgres,
        &["SELECT a#b; DELETE FROM t", "SELECT 'x' # y; DELETE FROM t"],
        UnboundedWrite,
    );
    expect(Postgres, "SELECT a#b INTO v FROM t", Ddl);
    expect(Postgres, "SELECT x#for FROM t", Read);
    // [A]
    for sql in [
        "SELECT a#b FROM t@link",
        "SELECT 1 #; COMMIT",
        "SELECT a#b; BEGIN DELETE FROM t; END;",
    ] {
        expect_unparsed(Postgres, sql);
    }
    // PG · MySQL · SQLite 에는 `EXECUTE IMMEDIATE '<글>'` 이 없다(서버가 거절한다). 파서는 준비한 문장의 실행으로 읽는다 — 쓰기.
    let classification =
        classify_statement(Postgres, "SELECT a#b; EXECUTE IMMEDIATE 'DROP TABLE t'");
    assert_eq!(
        (classification.grade, classification.reasons),
        (Write, vec![RiskReason::Procedural])
    );
    // MySQL: 한 줄이면 `#` 뒤는 실행되지 않는다. 줄을 바꾸면 다음 문장이 보인다.
    expect(Mysql, "SELECT a#b; DELETE FROM t", Read);
    expect(Mysql, "SELECT a#b\n; DELETE FROM t", UnboundedWrite);
    expect(Mysql, "SELECT a#b\n; DROP TABLE t", Destructive);
}

// DBX: classify_select_statements
#[test]
fn classify_select_statements() {
    expect(Postgres, "SELECT * FROM users", Read);
    expect(
        Mysql,
        "SELECT id, name FROM users WHERE active = true",
        Read,
    );
    expect(Mysql, "SHOW TABLES", Read);
    expect(Mysql, "DESCRIBE users", Read);
    expect(Postgres, "EXPLAIN SELECT * FROM users", Read);
}

// DBX: classify_mysql_show_triggers_as_read_only
#[test]
fn classify_mysql_show_triggers_as_read_only() {
    expect_all(
        Mysql,
        &[
            "SHOW TRIGGERS;",
            "SHOW TRIGGERS FROM `rs_main` LIKE 'trg_order_items_after_%';",
        ],
        Read,
    );
    // [A] sqlparser 는 `SHOW TRIGGERS … WHERE` 를 읽지 못한다(DBX 는 키워드 폴백으로 조회).
    expect_unparsed(
        Mysql,
        "show triggers in `rs_main` where `Event` = 'INSERT';",
    );
}

// DBX: classify_supported_show_statements_as_read_only
#[test]
fn classify_supported_show_statements_as_read_only() {
    expect(Mysql, "SHOW COLLATION", Read);
    expect(Mysql, "SHOW CHARACTER SET", Read);
    expect(Postgres, "SHOW search_path", Read);
}

// DBX: mysql_show_triggers_preserves_write_detection
#[test]
fn mysql_show_triggers_preserves_write_detection() {
    // [B] WHERE 없는 DELETE 는 범위 없는 쓰기, DROP 은 파괴. 실행 주석 안의 문장도 읽는다.
    expect(
        Mysql,
        "SHOW TRIGGERS; DELETE FROM order_items",
        UnboundedWrite,
    );
    expect(Mysql, "SHOW TRIGGERS; DROP TABLE order_items", Destructive);
    expect(
        Mysql,
        "SHOW TRIGGERS; /*!50000 DELETE FROM order_items */",
        UnboundedWrite,
    );
}

// DBX: classify_mysql_legacy_shared_lock_reads_like_for_share
#[test]
fn classify_mysql_legacy_shared_lock_reads() {
    // 파서가 `LOCK IN SHARE MODE` 를 읽지 못하지만 인식기가 `FOR SHARE` 로 읽는다 — DBX 와 같은 쓰기.
    for sql in [
        "SELECT * FROM users LOCK IN SHARE MODE",
        "SELECT * FROM users lock in share mode",
        "SELECT id FROM users WHERE id = 1 LOCK IN SHARE MODE",
        "WITH c AS (SELECT 1) SELECT * FROM users LOCK IN SHARE MODE",
        "SELECT * FROM (SELECT * FROM users LOCK IN SHARE MODE) AS locked_users",
        "WITH locked AS (SELECT * FROM users LOCK IN SHARE MODE) SELECT * FROM locked",
        "SELECT (SELECT id FROM users LOCK IN SHARE MODE)",
    ] {
        expect(Mysql, sql, Write);
    }
    // 이름 · 문자열에 든 같은 낱말은 조회다.
    expect_all(
        Mysql,
        &[
            "SELECT lock_in_share_mode FROM settings",
            "SELECT mode FROM share WHERE id = 1",
            "SELECT 'LOCK IN SHARE MODE'",
            "SELECT `lock`, `in`, share, mode FROM settings",
        ],
        Read,
    );
}

// DBX: classify_cte_read
#[test]
fn classify_cte_read() {
    expect(Postgres, "WITH cte AS (SELECT 1) SELECT * FROM cte", Read);
}

// DBX: classify_writable_ctes_recursively
#[test]
fn classify_writable_ctes_recursively() {
    expect_all(
        Postgres,
        &[
            "WITH inserted AS (INSERT INTO users (id) VALUES (1) RETURNING id) SELECT * FROM inserted",
            "WITH updated AS (UPDATE users SET active = true WHERE id = 1 RETURNING id) SELECT * FROM updated",
            "WITH deleted AS (DELETE FROM users WHERE id = 1 RETURNING id) SELECT * FROM deleted",
            "WITH outer_cte AS (WITH deleted AS (DELETE FROM users WHERE id = 1 RETURNING id) SELECT * FROM deleted) SELECT * FROM outer_cte",
        ],
        Write,
    );
    // [B] MERGE 는 범위 없는 쓰기.
    expect(
        Postgres,
        "WITH merged AS (MERGE INTO users USING staged_users ON users.id = staged_users.id WHEN MATCHED THEN UPDATE SET active = true RETURNING users.id) SELECT * FROM merged",
        UnboundedWrite,
    );
}

// DBX: writable_cte_danger_tracks_the_nested_mutation
#[test]
fn writable_cte_grade_tracks_the_nested_mutation() {
    expect_all(
        Postgres,
        &[
            "WITH updated AS (UPDATE users SET active = true WHERE id = 1 RETURNING id) SELECT * FROM updated",
            "WITH deleted AS (DELETE FROM users WHERE id = 1 RETURNING id) SELECT * FROM deleted",
            "WITH outer_cte AS (WITH deleted AS (DELETE FROM users WHERE id = 1 RETURNING id) SELECT * FROM deleted) SELECT * FROM outer_cte",
        ],
        Write,
    );
    expect_all(
        Postgres,
        &[
            "WITH updated AS (UPDATE users SET active = true RETURNING id) SELECT * FROM updated",
            "WITH deleted AS (DELETE FROM users RETURNING id) SELECT * FROM deleted",
            "WITH outer_cte AS (WITH deleted AS (DELETE FROM users RETURNING id) SELECT * FROM deleted) SELECT * FROM outer_cte",
        ],
        UnboundedWrite,
    );
}

// DBX: classify_write_statements
#[test]
fn classify_write_statements() {
    expect(Postgres, "INSERT INTO users VALUES (1)", Write);
    // [B]
    expect(Postgres, "UPDATE users SET name = 'x'", UnboundedWrite);
    expect(Postgres, "DELETE FROM users", UnboundedWrite);
    expect(
        Postgres,
        "EXPLAIN ANALYZE DELETE FROM users",
        UnboundedWrite,
    );
    // [F]
    expect(Postgres, "SELECT * INTO backup_users FROM users", Ddl);
    // [A]
    expect_unparsed(Mysql, "SELECT * FROM users INTO OUTFILE '/tmp/users.csv'");
    // [B] 실행 주석 안을 읽는다 — WHERE 없는 DELETE.
    expect(Mysql, "/*! DELETE FROM users */", UnboundedWrite);
}

// DBX: mcp_forbids_persistent_database_switching_in_every_permission_mode
#[test]
fn database_switching_is_a_session_write() {
    // DBX 는 MCP 에서 따로 막았다. 여기서는 세션 상태를 바꾸는 쓰기다 — read-only 연결 · MCP 쪽 단위 실행은 조회만 받으므로 같은
    // 결과로 막힌다(결정 M3-D19).
    for sql in [
        "USE reporting",
        "-- target database\nUSE reporting",
        "SELECT 1; USE reporting",
        "/*!50000 USE reporting */",
    ] {
        let classification = classify_statement(Mysql, sql);
        assert_eq!(
            (classification.grade, classification.reasons),
            (Write, vec![RiskReason::SessionState]),
            "{sql}"
        );
    }
    expect_all(
        Mysql,
        &[
            "SELECT `use` FROM feature_flags",
            "SELECT 'USE reporting'",
            "SELECT 1",
        ],
        Read,
    );
    // PG 에는 실행 주석이 없다 — 주석뿐인 글은 아무것도 돌리지 않는다.
    expect(Postgres, "/*!50000 USE reporting */", Read);
    // [A] SQL Server · Snowflake 꼴은 이 방언들이 읽지 못한다.
    expect_unparsed(Mysql, "USE [reporting]");
    expect_unparsed(Postgres, "USE DATABASE reporting");
}

// DBX: classify_known_side_effect_selects_and_copy_as_writes
#[test]
fn classify_known_side_effect_selects_and_copy() {
    expect_all(
        Postgres,
        &[
            "SELECT setval('user_id_seq', 42)",
            "SELECT nextval('user_id_seq')",
            "SELECT * FROM users FOR UPDATE",
            "COPY users TO '/tmp/users.csv'",
        ],
        Write,
    );
    // 다른 세션을 끊는 관리 함수는 범위 없는 쓰기다(DBX 는 쓰기 + dangerous).
    // [E]
    expect(Postgres, "SELECT pg_terminate_backend(42)", UnboundedWrite);
    // 인식기가 `FOR SHARE` 로 읽는다(DBX 와 같은 쓰기).
    expect(Postgres, "SELECT * FROM users FOR KEY SHARE", Write);
    // DB 서버에서 셸 명령을 돌린다 — 파괴(DBX 는 쓰기 + dangerous).
    let classification = classify_statement(
        Postgres,
        "COPY (SELECT * FROM users) TO PROGRAM 'cat > /tmp/users.csv'",
    );
    assert_eq!(
        (classification.grade, classification.reasons),
        (Destructive, vec![RiskReason::CopyProgram])
    );
}

// DBX: classify_backend_query_cancellation_as_a_side_effect
#[test]
fn classify_backend_query_cancellation_as_a_side_effect() {
    // [E]
    expect(Postgres, "SELECT pg_cancel_backend(42)", UnboundedWrite);
    // Kingbase 의 같은 함수(DBX 목록 그대로).
    expect(Postgres, "SELECT sys_cancel_backend(42)", UnboundedWrite);
}

// DBX: classify_dialect_specific_select_into_as_write
#[test]
fn classify_dialect_specific_select_into() {
    // [A] 파서가 `INTO OUTFILE` · `INTO DUMPFILE` 을 읽지 못한다(실행 주석 안이어도).
    for sql in [
        "SELECT 3156 INTO OUTFILE '/var/lib/mysql-files/dbx_ro_probe.txt'",
        "SELECT 3156 INTO DUMPFILE '/var/lib/mysql-files/dbx_ro_probe.bin'",
        "SELECT 3156 /*!50000 INTO OUTFILE '/var/lib/mysql-files/dbx_ro_probe.txt' */",
    ] {
        expect_unparsed(Mysql, sql);
    }
    // [F] PG 계열의 `SELECT … INTO` 는 표를 만든다.
    expect(Postgres, "SELECT * INTO copied_users FROM users", Ddl);
    // [F] SQL Server 의 임시 표 이름(`#…`)도 PG 방언에서는 표를 만드는 `SELECT … INTO` 로 읽힌다.
    expect(Postgres, "SELECT * INTO #copied_users FROM users", Ddl);
}

// DBX: typed_classification_preserves_existing_risk_levels
#[test]
fn classification_keeps_read_and_ddl_levels() {
    expect(Postgres, "SELECT * FROM users", Read);
    expect(Postgres, "CREATE TABLE users (id INT)", Ddl);
    // SQLite 는 `SELECT … INTO` 를 거절하지만 DB 가 거절하리라 믿지 않는다 — 쓰기(DBX 는 조회).
    let classification = classify_statement(Sqlite, "SELECT 1 INTO unsupported");
    assert_eq!(
        (classification.grade, classification.reasons),
        (Write, vec![RiskReason::SelectInto])
    );
}

// DBX: classify_ddl_statements
#[test]
fn classify_ddl_statements() {
    expect(Postgres, "CREATE TABLE users (id INT)", Ddl);
    expect(Postgres, "ALTER TABLE users ADD COLUMN age INT", Ddl);
    // [B]
    expect(Postgres, "DROP TABLE users", Destructive);
    expect(Postgres, "TRUNCATE TABLE users", Destructive);
}

// DBX: high_risk_sql_requires_central_permission_for_unbounded_changes
#[test]
fn unbounded_changes_are_unbounded_writes() {
    // [B] DBX 의 `is_dangerous` 가 참이던 UPDATE · DELETE.
    expect_all(
        Mysql,
        &[
            "DELETE FROM users",
            "UPDATE users SET active = 0",
            "DELETE FROM users WHERE 1 = 1",
            "UPDATE users SET active = 0 WHERE TRUE",
            "DELETE FROM users WHERE id = id",
            "DELETE FROM users WHERE lower(email) = lower(email)",
            "DELETE FROM users WHERE NOT (1 = 0)",
            "UPDATE users SET active = 0 WHERE 2 > 1",
            "DELETE FROM users WHERE id IS NULL OR id IS NOT NULL",
            "DELETE FROM users WHERE id IS NULL OR NOT (((id IS NULL)))",
            "DELETE FROM users WHERE id IS NOT NULL OR NOT (((id IS NOT NULL)))",
            "DELETE FROM users WHERE id = 1 OR id <> 1",
            "DELETE FROM users WHERE id != 1 OR 1 = id",
            "DELETE FROM users WHERE status = 'disabled' OR status != 'disabled'",
            "DELETE FROM users WHERE (id = 1 OR status = 'disabled') OR 1 != id OR id IS NULL",
            "DELETE FROM users WHERE id > 1 OR id <= 1",
            "DELETE FROM users WHERE 1 >= id OR id > 1",
            "DELETE FROM users WHERE id >= 1 OR 1 > id",
            "DELETE FROM users WHERE id IN (1) OR id NOT IN (1) OR id IS NULL",
            "DELETE FROM users WHERE id IN (1, 2) OR id NOT IN (2, 1) OR id IS NULL",
            "DELETE FROM users WHERE id BETWEEN 1 AND 2 OR id NOT BETWEEN 1 AND 2 OR id IS NULL",
            "DELETE FROM users WHERE id = 1 OR (id <> 1 AND TRUE) OR id IS NULL",
            "DELETE FROM users WHERE id <=> id",
            "UPDATE users SET active = 0 WHERE name LIKE '%' OR name IS NULL",
            "UPDATE users SET active = 0 WHERE name LIKE '%%' OR name IS NULL",
            "DELETE FROM users WHERE (id IS NULL OR status = 'disabled') OR id IS NOT NULL",
            "UPDATE users SET active = 0 WHERE abs(1) = 1",
            "DELETE FROM users WHERE lower('A') = 'a'",
            "DELETE FROM users WHERE coalesce(NULL, 1) = 1",
            "DELETE FROM users WHERE LOWER(_utf8mb4'A') = 'a'",
            "UPDATE users JOIN accounts ON accounts.id = users.account_id SET users.active = 0 WHERE users.id = 1",
            "DELETE users FROM users JOIN accounts ON accounts.id = users.account_id WHERE users.id = 1",
        ],
        UnboundedWrite,
    );
    expect_all(
        Postgres,
        &[
            "DELETE FROM users WHERE id IS NOT DISTINCT FROM id",
            "DELETE FROM users WHERE EXTRACT(YEAR FROM DATE '2026-01-01') = 2026",
            "DELETE FROM users WHERE DATE '2026-01-01' < CURRENT_DATE",
            "DELETE FROM users WHERE USER = CURRENT_USER",
            "DELETE FROM users WHERE id IN (SELECT id FROM archived_users)",
            "DELETE FROM users WHERE EXISTS (SELECT 1 FROM archived_users)",
            "UPDATE users SET active = false FROM accounts WHERE users.account_id = accounts.id",
        ],
        UnboundedWrite,
    );
    // 범위가 있는 쓰기.
    expect_all(
        Mysql,
        &[
            "DELETE FROM users WHERE id = 1",
            "UPDATE users SET active = 0 WHERE id = 1",
            "UPDATE users SET active = 0 WHERE abs(id) = 1",
            "DELETE FROM users WHERE lower(email) = 'disabled@example.com'",
            "DELETE FROM users WHERE id IS NULL OR status IS NOT NULL",
            "DELETE FROM users WHERE (id IS NULL OR NOT (((id IS NULL)))) AND tenant_id = 1",
            "DELETE FROM users WHERE (id = 1 OR id <> 1) AND tenant_id = 1",
            "DELETE FROM users WHERE status = 'pending' OR status <> 'disabled'",
            "DELETE FROM users WHERE id IN (1) OR id IN (2)",
            "DELETE FROM users WHERE id BETWEEN 1 AND 2 OR id BETWEEN 4 AND 5",
            "UPDATE users SET active = 0 WHERE name LIKE 'admin%'",
            "UPDATE users SET active = 0 WHERE status = 'inactive' AND tenant_id = 1",
            "INSERT INTO users (id) VALUES (1)",
            // [B] DBX 는 upsert · `INSERT … SELECT` 를 dangerous 로 봤다. F6-9 의 범위 없는 쓰기는 UPDATE · DELETE 다.
            "REPLACE INTO users (id) VALUES (1)",
            "INSERT INTO users (id) VALUES (1) ON DUPLICATE KEY UPDATE active = 1",
            "INSERT INTO users (id) SELECT id FROM staged_users",
        ],
        Write,
    );
    expect_all(
        Postgres,
        &[
            "DELETE FROM users WHERE EXTRACT(YEAR FROM created_at) = 2026",
            "INSERT INTO users (id) VALUES (1) ON CONFLICT (id) DO UPDATE SET active = true",
            "INSERT INTO users (id) VALUES (1) ON CONFLICT (id) DO NOTHING",
        ],
        Write,
    );
    // [B]
    expect(Mysql, "TRUNCATE TABLE users", Destructive);
}

// DBX: select_into_requires_central_high_risk_permission_for_supported_databases
#[test]
fn select_into_grades() {
    // [F]
    expect(Postgres, "SELECT * INTO copied_users FROM users", Ddl);
    expect(Postgres, "SELECT * INTO #copied_users FROM users", Ddl);
    // [A]
    expect_unparsed(Mysql, "SELECT 1 INTO OUTFILE '/tmp/dbx-probe.txt'");
    expect(Postgres, "SELECT 1", Read);
    expect(Sqlite, "SELECT 1 INTO unsupported", Write);
}

// DBX: classify_transaction_statements
#[test]
fn classify_transaction_statements() {
    // [B] DBX 의 `Transaction` 등급은 없다 — 트랜잭션 제어는 쓰기(결정 M3-D6).
    for sql in ["BEGIN", "COMMIT", "ROLLBACK"] {
        let classification = classify_statement(Postgres, sql);
        assert_eq!(
            (classification.grade, classification.reasons),
            (Write, vec![RiskReason::TransactionControl]),
            "{sql}"
        );
    }
}

// DBX: classify_multi_statement_returns_highest_risk
#[test]
fn classify_multi_statement_returns_highest_risk() {
    expect(Postgres, "SELECT 1; INSERT INTO users VALUES (1)", Write);
    // [B] DBX 는 `BEGIN; DROP …` 을 `Transaction` 으로 답했다 — 다섯 갈래에서는 파괴가 가장 높다.
    expect(Postgres, "BEGIN; DROP TABLE users", Destructive);
}

// DBX: classify_fallback_on_parse_error
#[test]
fn parse_errors_fail_closed() {
    // [A] DBX 는 폴백으로 읽기 쉬운 글을 조회로 답했다(이 시험의 글은 원래 파싱도 됐다).
    expect(Postgres, "SELECT * FROM users", Read);
    expect_unparsed(Postgres, "SELECT * FROM users WHERE");
}

// DBX: classify_unknown_statement_is_write
#[test]
fn classify_unknown_statement_is_write() {
    expect(Postgres, "GRANT SELECT ON users TO admin", Ddl);
    let classification = classify_statement(Postgres, "LISTEN channel");
    assert_eq!(
        (classification.grade, classification.reasons),
        (Write, vec![RiskReason::Other])
    );
}

// DBX: classifies_search_engine_rest_risk_by_method_and_path
#[test]
fn search_engine_rest_text_is_unparsed() {
    // [A] Elasticsearch 연결은 Pitwall 에 없다 — REST 글이 SQL 콘솔에 오면 어느 방언도 읽지 못한다.
    for dialect in [Postgres, Mysql, Sqlite] {
        for sql in [
            "GET /_cluster/health",
            "POST /products/_search\n{}",
            "PUT /products/_doc/1\n{}",
            "DELETE /products",
        ] {
            expect_unparsed(dialect, sql);
        }
    }
}

// DBX: classifies_salesforce_soql_as_read_even_when_sqlparser_cannot
#[test]
fn salesforce_soql_text() {
    // Salesforce 연결은 Pitwall 에 없다. SQL 로 읽히는 SOQL 은 조회고, 읽히지 않는 꼴은 파괴다.
    expect_all(
        Postgres,
        &[
            "SELECT Id, Name FROM Account",
            "SELECT Id, (SELECT Id FROM Contacts) FROM Account",
            "SELECT Id FROM Case WHERE Subject = 'Delete request'",
        ],
        Read,
    );
    // [A]
    expect_unparsed(Postgres, "SELECT FIELDS(ALL) FROM Account LIMIT 200");
    expect_unparsed(
        Postgres,
        "SELECT Id FROM Opportunity WHERE CloseDate = LAST_N_DAYS:7",
    );
}

// DBX: classifies_scoped_salesforce_dml_as_a_safe_write
#[test]
fn salesforce_pseudo_commands_are_unparsed() {
    // [A] DBX 의 Salesforce 의사 명령(`DBX SALESFORCE DML` + JSON)은 SQL 이 아니다.
    for sql in [
        "DBX SALESFORCE DML\n{\"op\":\"update\",\"object\":\"Opportunity\",\"id\":\"006x\",\"fields\":{\"StageName\":\"Closed Won\"}}",
        "DBX SALESFORCE DML\n{\"op\":\"insert\",\"object\":\"Lead\",\"fields\":{\"Company\":\"Acme\"}}",
        "DBX SALESFORCE DML\n{\"op\":\"delete\",\"object\":\"Lead\",\"id\":\"00Qx\"}",
        "DBX SALESFORCE DML\n{\"op\":\"update\",\"object\":\"Account\",\"id\":\"001x\",\"fields\":{\"Description\":\"a; USE prod\"}}",
    ] {
        expect_unparsed(Postgres, sql);
        expect_unparsed(Mysql, sql);
    }
}

// DBX: classifies_unscoped_salesforce_writes_as_high_risk
#[test]
fn salesforce_unscoped_text() {
    for sql in [
        "DBX SALESFORCE DML\n{\"op\":\"update\",\"object\":\"Account\",\"fields\":{\"Name\":\"x\"}}",
        "DBX SALESFORCE DML\n{\"op\":\"upsert\",\"object\":\"Account\",\"externalId\":\"E-1\"}",
        "DBX SALESFORCE BULK\n{}",
    ] {
        expect_unparsed(Postgres, sql);
    }
    // [B] SQL 로 읽히는 `DELETE FROM Account` 는 WHERE 없는 DELETE 다.
    expect(Postgres, "DELETE FROM Account", UnboundedWrite);
}
