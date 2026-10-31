//! MySQL 글의 여러 읽기(`risk/mysql.rs`): 실행 주석의 판본(H3)과 백슬래시 이스케이프(M2).

use pw_db_types::RiskGrade::{self, Destructive, Read, UnboundedWrite, Write};
use pw_db_types::SqlDialect::{Mysql, Postgres, Sqlite};

use crate::risk::RiskReason::{self, *};
use crate::risk::{Classification, classify_statement, classify_statement_readings};

#[track_caller]
fn expect(sql: &str, grade: RiskGrade, reasons: &[RiskReason]) {
    assert_eq!(
        classify_statement(Mysql, sql),
        Classification {
            grade,
            reasons: reasons.to_vec()
        },
        "{sql}"
    );
}

#[test]
fn a_where_hidden_in_a_versioned_or_mariadb_comment_does_not_bound_the_write() {
    // MySQL 8 은 판본이 더 큰 `/*!99999 */` 과 모든 `/*M! */` 을 버린다 — 표 전체가 지워진다.
    for sql in [
        "DELETE FROM t /*M! WHERE id = 1 */",
        "DELETE FROM t /*M!100000 WHERE id = 1 */",
        "DELETE FROM t /*!99999 WHERE id = 1 */",
        "UPDATE t SET a = 1 /*!99999 WHERE id = 1 */",
        // 판본이 붙은 주석은 서버마다 다르다 — 지금 서버가 실행할 판본이어도 버리는 읽기를 함께 본다.
        "DELETE FROM t /*!50000 WHERE id = 1 */",
    ] {
        expect(sql, UnboundedWrite, &[MissingWhere]);
    }
    // 판본 없는 `/*!` 는 MySQL · MariaDB 가 늘 실행한다.
    expect("DELETE FROM t /*! WHERE id = 1 */", Write, &[Dml]);
    // 주석 안이 경계를 넓히는 꼴도 잡는다.
    expect(
        "DELETE FROM t WHERE id = 1 /*!99999 OR 1 = 0 */",
        UnboundedWrite,
        &[UnboundedWhere],
    );
}

#[test]
fn executable_comment_combinations_and_their_limits() {
    // 둘을 섞으면(하나만 실행) 어느 조합은 읽지 못한다 — 파괴.
    expect(
        "DELETE FROM t /*!50000 WHERE */ /*!99999 id = 1 AND */ 1 = 1",
        Destructive,
        &[Unparsed],
    );
    // 안의 `DROP` 은 펼친 읽기로 파괴.
    expect("SELECT 1 /*!80000 ; */", Destructive, &[Unparsed]);
    expect("/*!80000 DROP TABLE t */", Destructive, &[Drop]);
    // 같은 판본은 한 열쇠다(조합이 늘지 않는다).
    expect(
        "SELECT /*!50001 1, */ /*!50001 2, */ /*!50002 3, */ /*!50003 4, */ /*M! 5, */ 6",
        Read,
        &[],
    );
    // 서로 다른 열쇠가 넷을 넘으면 조합이 너무 많다.
    expect(
        "SELECT /*!50001 1, */ /*!50002 2, */ /*!50003 3, */ /*!50004 4, */ /*M! 5, */ 6",
        Destructive,
        &[Unparsed],
    );
    // 닫히지 않거나 겹친 실행 주석.
    expect("SELECT 1 /*!50000 , 2", Destructive, &[Unparsed]);
    expect(
        "SELECT 1 /*!50000 , /*!50001 2 */ */",
        Destructive,
        &[Unparsed],
    );
    // 문자열 · 일반 주석 안의 `/*!` 는 실행 주석이 아니다.
    expect("SELECT '/*!99999 x */' FROM t", Read, &[]);
    expect(
        "DELETE FROM t WHERE id = 1 -- /*!99999 OR 1 = 1 */",
        Write,
        &[Dml],
    );
}

#[test]
fn a_backslash_that_splits_differently_without_escapes_is_destructive() {
    // `NO_BACKSLASH_ESCAPES` 서버는 `'a\'` 에서 문자열을 닫고 뒤의 `DROP` 을 본다.
    for sql in [
        r"SELECT 'a\'; DROP TABLE t; -- '",
        r#"SELECT "a\"; DROP TABLE t; -- ""#,
        r"SELECT 'a\'; DROP TABLE t; SELECT 'x",
    ] {
        let classification = classify_statement(Mysql, sql);
        assert_eq!(classification.grade, Destructive, "{sql}");
        assert!(
            classification.reasons.contains(&Unparsed),
            "{sql}: {classification:?}"
        );
    }
    // 나뉨이 같아도 뒤의 읽기가 경계를 넓히면 높은 쪽.
    expect(
        r"DELETE FROM t WHERE c = 'x\' OR 1=1 -- '",
        UnboundedWrite,
        &[UnboundedWhere],
    );
}

/// 넣지 않은 읽기는 알린다 — 조회만 받는 호출(`db.query.page` · `queryOnly`)이 그 글을 조회로 보지 않게(M3-17 리뷰 X3). 등급은
/// 그대로다(사람이 치는 일반 실행의 규칙, `common_escaped_quotes_stay_as_they_are`).
#[test]
fn a_reading_left_out_of_the_grade_is_reported() {
    let readings = |sql: &str| classify_statement_readings(Mysql, sql);
    // 뒤의 읽기(`NO_BACKSLASH_ESCAPES`)로는 `INTO OUTFILE` · `INTO @변수` 가 붙은 조회 — 파서는 못 읽고 서버는 읽는다.
    for sql in [
        r#"SELECT * FROM t WHERE name = 'a\' OR id = 1 INTO OUTFILE "/var/lib/mysql-files/x.txt" -- '"#,
        r"SELECT * FROM t WHERE name = 'a\' OR id = 1 INTO @x, @y -- '",
    ] {
        let got = readings(sql);
        assert_eq!(got.classification.grade, Read, "{sql}: {got:?}");
        assert!(got.unread_reading, "{sql}: {got:?}");
    }
    // 쓰기여도 알린다(등급은 앞의 읽기 그대로).
    let update = readings(r"UPDATE t SET a = 'x\' INTO @v -- ' WHERE id = 1");
    assert_eq!(update.classification.grade, Write);
    assert!(update.unread_reading, "{update:?}");
    // 흔한 데이터(`'a\'b'`)는 뒤의 읽기에서 따옴표가 닫히지 않는다 — 서버가 그 문장에서 멈추므로 넣지 않은 읽기가 아니다.
    assert!(!readings(r"INSERT INTO t VALUES ('a\'b', 1), ('c', 2)").unread_reading);
    // 뒤의 읽기를 읽었거나(분류에 들어갔다), 따옴표가 닫히지 않아 서버가 멈추거나, 백슬래시가 없으면 넣지 않은 읽기가 없다.
    for sql in [
        r"DELETE FROM t WHERE c = 'x\' OR 1=1 -- '",
        r"SELECT * FROM t WHERE c = 'x\' OR 1=1 -- '",
        r"SELECT 'it\'s'",
        "SELECT * FROM t INTO OUTFILE '/tmp/x'",
        "SELECT 1",
    ] {
        assert!(!readings(sql).unread_reading, "{sql}: {:?}", readings(sql));
    }
    // PG · SQLite 는 읽기가 하나다.
    for dialect in [Postgres, Sqlite] {
        assert!(!classify_statement_readings(dialect, r"SELECT 'a\' OR 1 = 1 -- '").unread_reading);
    }
}

#[test]
fn common_escaped_quotes_stay_as_they_are() {
    // 뒤의 읽기에서 따옴표가 닫히지 않으면 서버는 그 문장에서 멈춘다 — 앞에서 끝난 문장만 본다.
    expect(r"SELECT 'it\'s'", Read, &[]);
    expect(r"SELECT 1; SELECT 'it\'s'", Read, &[]);
    // 뒤의 읽기가 엉켜 읽히지 않으면 그 읽기는 등급에 넣지 않는다.
    expect(r"INSERT INTO t VALUES ('a\'b', 1), ('c', 2)", Write, &[Dml]);
    expect(
        r"UPDATE t SET name = 'O\'Brien' WHERE id = 1",
        Write,
        &[Dml],
    );
    expect(r"SELECT 'C:\\temp'", Read, &[]);
}
