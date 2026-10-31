use pw_db_types::SqlDialect::{self, Mysql, Postgres, Sqlite};

use super::*;

fn tx(dialect: SqlDialect, text: &str) -> Option<TxControl> {
    transaction_control(dialect, text)
}

const BEGIN: Option<TxControl> = Some(TxControl::Begin { read_only: None });
const COMMIT: Option<TxControl> = Some(TxControl::Commit { chain: false });
const ROLLBACK: Option<TxControl> = Some(TxControl::Rollback { chain: false });

#[test]
fn opening_statements_become_begin_in_every_dialect() {
    for dialect in [Postgres, Mysql, Sqlite] {
        assert_eq!(tx(dialect, "BEGIN"), BEGIN, "{dialect:?}");
        assert_eq!(tx(dialect, "  begin  -- go\n"), BEGIN, "{dialect:?}");
        assert_eq!(tx(dialect, "/* c */ Begin Work"), BEGIN, "{dialect:?}");
    }
    assert_eq!(tx(Postgres, "BEGIN TRANSACTION"), BEGIN);
    assert_eq!(tx(Sqlite, "BEGIN DEFERRED TRANSACTION"), BEGIN);
    for dialect in [Postgres, Mysql] {
        assert_eq!(tx(dialect, "START TRANSACTION"), BEGIN);
        assert_eq!(
            tx(dialect, "start transaction read only"),
            Some(TxControl::Begin {
                read_only: Some(true)
            })
        );
        assert_eq!(
            tx(dialect, "START TRANSACTION READ WRITE"),
            Some(TxControl::Begin {
                read_only: Some(false)
            })
        );
    }
    assert_eq!(
        tx(Postgres, "BEGIN READ ONLY"),
        Some(TxControl::Begin {
            read_only: Some(true)
        })
    );
    // MySQL 실행 주석 안의 BEGIN 도 서버가 돌리는 BEGIN 이다.
    assert_eq!(tx(Mysql, "/*!40101 BEGIN */"), BEGIN);
    assert_eq!(tx(Mysql, "# note\nBEGIN"), BEGIN);
}

#[test]
fn closing_statements_become_commit_or_rollback_with_chain() {
    for dialect in [Postgres, Mysql, Sqlite] {
        assert_eq!(tx(dialect, "COMMIT"), COMMIT, "{dialect:?}");
        assert_eq!(tx(dialect, "rollback"), ROLLBACK, "{dialect:?}");
    }
    assert_eq!(tx(Postgres, "END"), COMMIT);
    assert_eq!(tx(Sqlite, "END TRANSACTION"), COMMIT);
    assert_eq!(tx(Postgres, "ABORT"), ROLLBACK);
    assert_eq!(tx(Postgres, "COMMIT WORK"), COMMIT);
    assert_eq!(
        tx(Postgres, "COMMIT AND CHAIN"),
        Some(TxControl::Commit { chain: true })
    );
    assert_eq!(
        tx(Mysql, "ROLLBACK WORK AND CHAIN"),
        Some(TxControl::Rollback { chain: true })
    );
    assert_eq!(tx(Postgres, "ABORT AND NO CHAIN"), ROLLBACK);
    assert_eq!(tx(Mysql, "COMMIT AND NO CHAIN NO RELEASE"), COMMIT);
}

#[test]
fn savepoints_prepared_and_compound_statements_pass_through() {
    assert_eq!(tx(Postgres, "ROLLBACK TO SAVEPOINT a"), None);
    assert_eq!(tx(Postgres, "ROLLBACK WORK TO a"), None);
    assert_eq!(tx(Sqlite, "ROLLBACK TRANSACTION TO a"), None);
    assert_eq!(tx(Postgres, "SAVEPOINT a"), None);
    assert_eq!(tx(Postgres, "RELEASE SAVEPOINT a"), None);
    assert_eq!(tx(Postgres, "COMMIT PREPARED 'x'"), None);
    assert_eq!(tx(Postgres, "ROLLBACK PREPARED 'x'"), None);
    assert_eq!(tx(Mysql, "BEGIN NOT ATOMIC SELECT 1; END"), None);
    assert_eq!(tx(Mysql, "END"), None, "END is not a MySQL statement");
    assert_eq!(tx(Sqlite, "START TRANSACTION"), None);
    assert_eq!(tx(Postgres, "SELECT 1"), None);
    assert_eq!(tx(Postgres, "BEGINNING"), None);
    assert_eq!(tx(Postgres, "BEGIN한"), None);
    assert_eq!(tx(Postgres, ""), None);
}

#[test]
fn options_the_methods_cannot_carry_are_refused_not_dropped() {
    for (dialect, text) in [
        (Postgres, "BEGIN ISOLATION LEVEL SERIALIZABLE"),
        (Postgres, "START TRANSACTION READ ONLY, DEFERRABLE"),
        (Mysql, "START TRANSACTION WITH CONSISTENT SNAPSHOT"),
        (Mysql, "COMMIT RELEASE"),
        (Sqlite, "BEGIN IMMEDIATE"),
        (Sqlite, "BEGIN EXCLUSIVE TRANSACTION"),
        (Postgres, "COMMIT 'x'"),
    ] {
        assert!(
            matches!(tx(dialect, text), Some(TxControl::Refused(_))),
            "{dialect:?} {text}"
        );
    }
    assert!(matches!(
        tx(Postgres, "PREPARE TRANSACTION 'x'"),
        Some(TxControl::Refused(PREPARE_TRANSACTION))
    ));
}

#[test]
fn mysql_implicit_commits_follow_the_manual() {
    for text in [
        "CREATE TABLE t (id int)",
        "create index i on t (a)",
        "DROP TABLE t",
        "ALTER TABLE t ADD c int",
        "RENAME TABLE a TO b",
        "TRUNCATE TABLE t",
        "LOCK TABLES t WRITE",
        "UNLOCK TABLES",
        "GRANT SELECT ON *.* TO u",
        "ANALYZE TABLE t",
        "CHECK TABLE t",
        "OPTIMIZE TABLE t",
        "FLUSH PRIVILEGES",
        "RESET MASTER",
        "SET autocommit = 1",
        "SET @@session.autocommit=ON",
        "SET PASSWORD = 'x'",
        "START REPLICA",
        "CHANGE REPLICATION SOURCE TO SOURCE_HOST='h'",
        "/*!50001 DROP VIEW v */",
        "INSTALL PLUGIN p SONAME 'p.so'",
        "CACHE INDEX t IN c",
        "LOAD INDEX INTO CACHE t",
    ] {
        assert!(implicitly_commits(Mysql, text), "{text}");
    }
    for text in [
        "CREATE TEMPORARY TABLE t (id int)",
        "DROP TEMPORARY TABLE t",
        "CREATE OR REPLACE TEMPORARY TABLE t (id int)",
        "INSERT INTO t VALUES (1)",
        "SELECT 1",
        "RESET PERSIST",
        "SET @a = 1",
        "START TRANSACTION",
        "CHECKSUM TABLE t",
        "LOAD DATA INFILE 'x' INTO TABLE t",
    ] {
        assert!(!implicitly_commits(Mysql, text), "{text}");
    }
    assert!(!implicitly_commits(Postgres, "CREATE TABLE t (id int)"));
    assert!(!implicitly_commits(Sqlite, "DROP TABLE t"));
}

/// `SET` 목록의 뒤쪽 · 값이 숫자로 시작한 뒤의 `autocommit` 도 찾는다(M3-05 리뷰 L1 — 앞 낱말만 읽으면 숫자에서 멈췄다). 낱말
/// 일부(`autocommit_x`)는 아니다.
#[test]
fn autocommit_anywhere_in_a_set_list_is_an_implicit_commit() {
    for text in [
        "SET @a = 1, autocommit = 1",
        "SET @a := 1, autocommit = 1",
        "SET NAMES utf8mb4, @@SESSION.AUTOCOMMIT = 1",
        "SET sql_mode = '', `autocommit` = 1",
    ] {
        assert!(implicitly_commits(Mysql, text), "{text}");
    }
    for text in ["SET @a = 1, @autocommit_x = 2", "SELECT 1, autocommit"] {
        assert!(!implicitly_commits(Mysql, text), "{text}");
    }
}

/// 끝낼 수 있는 문장: 암묵 커밋 문장 + `CALL` · `EXECUTE` · MariaDB `BEGIN NOT ATOMIC`(MySQL 만).
#[test]
fn calls_and_prepared_statements_may_end_a_mysql_transaction() {
    for text in [
        "CALL p()",
        "call db.p(1)",
        "EXECUTE s",
        "BEGIN NOT ATOMIC COMMIT; END",
        "DROP TABLE t",
    ] {
        assert!(may_end_transaction(Mysql, text), "{text}");
    }
    for text in ["INSERT INTO t VALUES (1)", "SELECT 1", "PREPARE s FROM 'x'"] {
        assert!(!may_end_transaction(Mysql, text), "{text}");
    }
    assert!(!may_end_transaction(Postgres, "CALL p()"));
    assert!(!may_end_transaction(Sqlite, "DROP TABLE t"));
}

#[test]
fn set_and_reset_change_session_settings() {
    for (dialect, text) in [
        (Postgres, "SET transaction_read_only = off"),
        (Postgres, "set transaction read write"),
        (Postgres, "RESET ALL"),
        (
            Postgres,
            "/* x */ SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE",
        ),
        (Mysql, "/*!80000 SET */ @@transaction_read_only = 0"),
        (Mysql, "SET TRANSACTION READ WRITE"),
        (Sqlite, "SET x = 1"),
    ] {
        assert!(
            changes_session_settings(dialect, text),
            "{dialect:?} {text}"
        );
    }
    for text in [
        "SELECT set_config('a', 'b', false)",
        "SETTINGS",
        "UPDATE t SET a = 1",
    ] {
        assert!(!changes_session_settings(Postgres, text), "{text}");
    }
}

/// 실행 계획 RPC 가 거절하는 계획 · 통계 문장(M3-15): 주석 · MySQL 실행 주석 뒤의 첫 낱말까지 본다. 계획 · 통계가 아닌 문장과
/// 그 낱말이 뒤에 오는 문장은 아니다.
#[test]
fn plan_and_analyze_statements_are_found_behind_comments() {
    for (dialect, text) in [
        (Postgres, "EXPLAIN ANALYZE DELETE FROM t"),
        (Postgres, "explain (analyze) delete from t"),
        (Postgres, "/* a */ -- b\n EXPLAIN SELECT 1"),
        (Postgres, "ANALYZE t"),
        (Postgres, "analyse t"),
        (Mysql, "/*!80018 EXPLAIN ANALYZE */ SELECT 1"),
        (Mysql, "# note\nDESCRIBE t"),
        (Mysql, "desc t"),
        (Sqlite, "EXPLAIN QUERY PLAN SELECT 1"),
    ] {
        assert!(plans_or_analyzes(dialect, text), "{dialect:?} {text}");
    }
    for (dialect, text) in [
        (Postgres, "SELECT 'EXPLAIN'"),
        (Postgres, "DELETE FROM explain"),
        (Mysql, "SELECT * FROM t ORDER BY a DESC"),
        (Mysql, "/* EXPLAIN */ SELECT 1"),
        (Sqlite, ""),
    ] {
        assert!(!plans_or_analyzes(dialect, text), "{dialect:?} {text}");
    }
}
