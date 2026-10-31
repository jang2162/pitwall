//! M3-03 리뷰가 찾은 구멍: `EXPLAIN (ANALYSE …)`(H1), 읽기 전용 트랜잭션을 지나는 PG 함수(H2), 서버 전체에 닿는 MySQL 문장(M1),
//! `DELETE LOW_PRIORITY`(L2).

use pw_db_types::RiskGrade::{self, Destructive, UnboundedWrite, Write};
use pw_db_types::SqlDialect::{self, Mysql, Postgres};

use crate::risk::RiskReason::{self, *};
use crate::risk::{Classification, classify_statement};

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
fn explain_with_the_british_analyse_option_runs_the_statement() {
    // PG 는 `ANALYSE` 를 `ANALYZE` 와 같은 옵션으로 받아 안의 문장을 실제로 돌린다.
    expect(
        Postgres,
        "EXPLAIN (ANALYSE) DELETE FROM t",
        UnboundedWrite,
        &[MissingWhere],
    );
    expect(
        Postgres,
        "EXPLAIN (ANALYSE, FORMAT JSON) UPDATE t SET a=1",
        UnboundedWrite,
        &[MissingWhere],
    );
    expect(
        Postgres,
        "EXPLAIN (ANALYSE true) DROP TABLE t",
        Destructive,
        &[Drop],
    );
    expect(
        Postgres,
        "EXPLAIN (VERBOSE, ANALYSE) DELETE FROM t",
        UnboundedWrite,
        &[MissingWhere],
    );
    expect(
        Postgres,
        "EXPLAIN (analyse) SELECT pg_terminate_backend(1)",
        UnboundedWrite,
        &[ServerAdmin],
    );
    expect(
        Postgres,
        "EXPLAIN (Analyze) DELETE FROM t",
        UnboundedWrite,
        &[MissingWhere],
    );
    // 괄호 없는 `EXPLAIN ANALYSE` 는 파서가 읽지 못한다.
    expect(
        Postgres,
        "EXPLAIN ANALYSE DELETE FROM t",
        Destructive,
        &[Unparsed],
    );
}

#[test]
fn server_admin_functions_that_pass_a_read_only_transaction() {
    for sql in [
        "SELECT pg_notify('jobs', 'run')",
        "SELECT pg_drop_replication_slot('s')",
        "SELECT pg_create_physical_replication_slot('s')",
        "SELECT pg_create_logical_replication_slot('s', 'pgoutput')",
        "SELECT pg_copy_physical_replication_slot('a', 'b')",
        "SELECT pg_copy_logical_replication_slot('a', 'b')",
        "SELECT pg_replication_slot_advance('s', '0/0')",
        "SELECT pg_replication_origin_create('x')",
        "SELECT pg_replication_origin_drop('x')",
        "SELECT pg_promote()",
        "SELECT pg_switch_wal()",
        "SELECT pg_create_restore_point('x')",
        "SELECT pg_backup_start('x')",
        "SELECT pg_backup_stop()",
        "SELECT pg_wal_replay_pause()",
        "SELECT pg_wal_replay_resume()",
        "SELECT pg_stat_reset()",
        "SELECT pg_stat_reset_shared('bgwriter')",
        "SELECT pg_log_backend_memory_contexts(1)",
        "SELECT pg_logical_emit_message(true, 'x', 'y')",
        "SELECT pg_file_write('/tmp/x', 'y', false)",
        "SELECT pg_file_rename('/tmp/x', '/tmp/y')",
        "SELECT pg_file_unlink('/tmp/x')",
        "SELECT pg_file_sync('/tmp/x')",
        "SELECT pg_rotate_logfile()",
    ] {
        expect(Postgres, sql, UnboundedWrite, &[ServerAdmin]);
    }
    for sql in [
        "SELECT pg_ls_dir('/')",
        "SELECT pg_ls_waldir()",
        "SELECT pg_stat_file('/etc/passwd')",
        "SELECT txid_current()",
        "SELECT pg_current_xact_id()",
    ] {
        expect(Postgres, sql, Write, &[SideEffectFunction]);
    }
    // 이름이 겹치는 읽기 함수는 조회다.
    for sql in [
        "SELECT pg_stat_get_numscans(1)",
        "SELECT pg_last_wal_replay_lsn()",
        "SELECT * FROM pg_stat_activity",
    ] {
        expect(Postgres, sql, RiskGrade::Read, &[]);
    }
}

#[test]
fn mysql_statements_that_reach_the_whole_server_match_admin_functions() {
    for sql in [
        "KILL 123",
        "KILL QUERY 123",
        "SET GLOBAL read_only = OFF",
        "SET @@global.read_only = 0",
        "SET @@GLOBAL.max_connections = 10",
        "FLUSH TABLES WITH READ LOCK",
        "FLUSH PRIVILEGES",
    ] {
        expect(Mysql, sql, UnboundedWrite, &[ServerAdmin]);
    }
    for sql in [
        "SET SESSION sql_mode = ''",
        "SET @@session.sql_mode = ''",
        "SET autocommit = 0",
    ] {
        expect(Mysql, sql, Write, &[SessionState]);
    }
    // 파서가 읽지 못하는 꼴은 이미 파괴다.
    expect(
        Mysql,
        "SET PERSIST max_connections = 10",
        Destructive,
        &[Unparsed],
    );
    expect(
        Postgres,
        "ALTER SYSTEM SET work_mem = '1GB'",
        Destructive,
        &[Unparsed],
    );
}

#[test]
fn mysql_delete_modifiers_are_not_tables() {
    expect(
        Mysql,
        "DELETE LOW_PRIORITY FROM t WHERE id = 1",
        Write,
        &[Dml],
    );
    expect(Mysql, "DELETE QUICK FROM t WHERE id = 1", Write, &[Dml]);
    expect(
        Mysql,
        "DELETE LOW_PRIORITY FROM t",
        UnboundedWrite,
        &[MissingWhere],
    );
    // 진짜 여러 표 DELETE 는 그대로 범위 없는 쓰기다.
    expect(
        Mysql,
        "DELETE t, u FROM t JOIN u ON t.id = u.id WHERE t.id = 1",
        UnboundedWrite,
        &[MultiTable],
    );
}
