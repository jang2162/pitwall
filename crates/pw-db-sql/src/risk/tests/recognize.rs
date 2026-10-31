//! 좁은 인식기(`risk/recognize.rs`): 정확한 꼴은 조회가 아닌 등급, 조금이라도 다르면 파괴 · `unparsed`.

use pw_db_types::RiskGrade::{self, Destructive, UnboundedWrite, Write};
use pw_db_types::SqlDialect::{self, Mysql, Postgres, Sqlite};

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

#[track_caller]
fn expect_unparsed(dialect: SqlDialect, sql: &str) {
    expect(dialect, sql, Destructive, &[Unparsed]);
}

#[test]
fn locking_clauses_the_parser_cannot_read_are_locking_reads() {
    expect(
        Mysql,
        "SELECT * FROM t LOCK IN SHARE MODE",
        Write,
        &[LockingRead],
    );
    expect(
        Mysql,
        "SELECT * FROM t WHERE id = 1 lock in share mode;",
        Write,
        &[LockingRead],
    );
    expect(
        Postgres,
        "SELECT * FROM t FOR NO KEY UPDATE",
        Write,
        &[LockingRead],
    );
    expect(
        Postgres,
        "SELECT * FROM t FOR KEY SHARE NOWAIT",
        Write,
        &[LockingRead],
    );
    // 바꾼 글을 다시 분류하므로 더 높은 등급은 그대로다(규칙 (가)).
    expect(
        Postgres,
        "DELETE FROM t WHERE id IN (SELECT id FROM u FOR KEY SHARE)",
        UnboundedWrite,
        &[UnboundedWhere],
    );
    expect(
        Postgres,
        "SELECT * FROM t FOR NO KEY UPDATE; DROP TABLE x",
        Destructive,
        &[Drop],
    );
    // 바꿔도 읽지 못하면 파괴 그대로.
    expect_unparsed(Postgres, "SELECT * FROM t FOR KEY SHARE OF");
    // 다른 방언의 꼴은 바꾸지 않는다.
    expect_unparsed(Sqlite, "SELECT * FROM t LOCK IN SHARE MODE");
}

#[test]
fn sqlite_pragma_detach_and_reindex() {
    for sql in [
        "PRAGMA table_info(t)",
        "PRAGMA main.table_info(t)",
        "PRAGMA journal_mode = WAL",
        "pragma journal_mode=wal;",
        "PRAGMA cache_size = -2000",
    ] {
        expect(Sqlite, sql, Write, &[Pragma]);
    }
    expect(Sqlite, "DETACH DATABASE other", Write, &[Attach]);
    expect(Sqlite, "DETACH other", Write, &[Attach]);
    for sql in ["REINDEX", "REINDEX t", "REINDEX main.t"] {
        expect(Sqlite, sql, Write, &[Maintenance]);
    }
    // 꼴이 조금이라도 다르면 인식하지 않는다(규칙 (나)).
    for sql in [
        "PRAGMA table_info(t); DROP TABLE x",
        "PRAGMA table_info(t, u)",
        "PRAGMA journal_mode = WAL WAL",
        "DETACH DATABASE a b",
        "REINDEX t u",
    ] {
        expect_unparsed(Sqlite, sql);
    }
}

#[test]
fn postgres_refresh_materialized_view() {
    for sql in [
        "REFRESH MATERIALIZED VIEW v",
        "REFRESH MATERIALIZED VIEW CONCURRENTLY s.v",
        "refresh materialized view v with no data;",
        "REFRESH MATERIALIZED VIEW v WITH DATA",
    ] {
        expect(Postgres, sql, Write, &[Maintenance]);
    }
    expect_unparsed(Postgres, "REFRESH MATERIALIZED VIEW v; DROP TABLE t");
    expect_unparsed(Postgres, "REFRESH MATERIALIZED VIEW v WITH");
    expect_unparsed(Mysql, "REFRESH MATERIALIZED VIEW v");
}

#[test]
fn mysql_load_data_infile_from_the_server_only() {
    for sql in [
        "LOAD DATA INFILE '/var/lib/mysql-files/x.csv' INTO TABLE t",
        "LOAD DATA LOW_PRIORITY INFILE '/x.csv' REPLACE INTO TABLE db.t",
        "LOAD DATA INFILE '/x.csv' IGNORE INTO TABLE t CHARACTER SET utf8mb4 FIELDS TERMINATED BY ',' ENCLOSED BY '\"' LINES TERMINATED BY '\\n' IGNORE 1 LINES (a, b)",
        "LOAD DATA CONCURRENT INFILE '/x.csv' INTO TABLE t PARTITION (p0, p1) COLUMNS TERMINATED BY '\\t' OPTIONALLY ENCLOSED BY '\"' ESCAPED BY '\\\\' LINES STARTING BY 'x' TERMINATED BY '\\n' IGNORE 2 ROWS (a, @skip, `c`)",
        "LOAD DATA INFILE '/x.csv' INTO TABLE t CHARACTER SET 'utf8mb4';",
    ] {
        expect(Mysql, sql, Write, &[DataTransfer]);
    }
    for sql in [
        // 사람 기기의 파일을 보낸다 — 파괴 그대로.
        "LOAD DATA LOCAL INFILE '/x.csv' INTO TABLE t",
        // 식을 쓰는 `SET` 절.
        "LOAD DATA INFILE '/x.csv' INTO TABLE t (a) SET b = SLEEP(10)",
        "LOAD DATA INFILE '/x.csv' INTO TABLE t; DROP TABLE u",
        "LOAD DATA INFILE @path INTO TABLE t",
        // 재리뷰 L-D: 꼬리의 아무 낱말 — 문서의 순서 · 절 밖.
        "LOAD DATA INFILE 'f' INTO TABLE t LOCAL",
        "LOAD DATA INFILE 'f' INTO TABLE t INTO OUTFILE 'g'",
        "LOAD DATA INFILE 'f' INTO TABLE t DROP",
        "LOAD DATA INFILE 'f' INTO TABLE t SELECT 1",
        "LOAD DATA INFILE 'f' INTO TABLE t LINES TERMINATED BY ',' FIELDS TERMINATED BY ','",
        "LOAD DATA INFILE 'f' INTO TABLE t FIELDS",
        "LOAD DATA INFILE 'f' INTO TABLE t IGNORE x LINES",
        "LOAD DATA INFILE 'f' INTO TABLE t (a, (b))",
        "LOAD DATA INFILE 'f' INTO TABLE t (a) (b)",
    ] {
        expect_unparsed(Mysql, sql);
    }
}

#[test]
fn bodies_the_recognizers_leave_destructive() {
    for (dialect, sql) in [
        (Postgres, "DO $$ BEGIN DROP TABLE t; END $$"),
        (Mysql, "SELECT * FROM t INTO OUTFILE '/tmp/x'"),
        (Mysql, "HANDLER t OPEN"),
        (Mysql, "PREPARE s FROM 'DROP TABLE t'"),
    ] {
        expect_unparsed(dialect, sql);
    }
}
