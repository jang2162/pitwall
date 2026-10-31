//! M3-03b 리뷰: 같은 함수를 다른 꼴로 부르기 — PG attribute notation `(식).함수`(H1)와 `U&"…"` 이름(H2) — 와 PG 가 함수를 부르는
//! 다른 자리의 훑기, contrib 밖 확장의 SQL 글 실행 함수(L1). 이름은 정본(`canonical_name`)으로 바꾼 뒤 목록과 맞춘다.

use pw_db_types::RiskGrade::{self, Destructive, Read, UnboundedWrite, Write};
use pw_db_types::SqlDialect::{self, Postgres};

use crate::risk::RiskReason::{self, *};
use crate::risk::{Classification, classify_statement};

#[track_caller]
fn expect(sql: &str, grade: RiskGrade, reasons: &[RiskReason]) {
    expect_in(Postgres, sql, grade, reasons);
}

#[track_caller]
fn expect_in(dialect: SqlDialect, sql: &str, grade: RiskGrade, reasons: &[RiskReason]) {
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
fn attribute_notation_is_a_function_call() {
    for sql in [
        "SELECT (1).pg_terminate_backend",
        "SELECT ((1)).pg_cancel_backend",
        "SELECT (pid).pg_terminate_backend FROM pg_stat_activity",
        "SELECT 1 WHERE (1).pg_terminate_backend",
        "SELECT ('select 1').ts_stat",
        "SELECT * FROM t WHERE (q).ts_stat IS NOT NULL",
        "SELECT ('select 1').crosstab2",
        "SELECT ('host=x').dblink_exec",
        "SELECT (x)[1].pg_terminate_backend FROM t",
        "SELECT (x).a.pg_cancel_backend FROM t",
        "SELECT (1).\"PG_TERMINATE_BACKEND\"",
        "SELECT (1).pg_catalog_x, (2).pg_terminate_backend",
    ] {
        expect(sql, UnboundedWrite, &[ServerAdmin]);
    }
    expect(
        "SELECT ('s'::regclass).nextval",
        Write,
        &[SideEffectFunction],
    );
    expect("SELECT (1).pg_sleep", Write, &[SideEffectFunction]);
    // 필드 접근 그대로인 것 · 괄호 없는 `t.f`(표 전체 행을 넘기는 꼴 — 목록의 함수는 composite 를 받지 않는다)는 조회다.
    expect("SELECT (x).a, (x).b FROM t", Read, &[]);
    expect("SELECT t.nextval FROM t", Read, &[]);
}

#[test]
fn unicode_escaped_names_are_read_as_the_server_reads_them() {
    for sql in [
        r#"SELECT U&"query\005fto_xml"('select 1', true, false, '')"#,
        r#"SELECT "pg_catalog".U&"ts\005fstat"('select 1')"#,
        r#"SELECT U&"pg\005fterminate\005fbackend"(1)"#,
        r#"SELECT U&"pg\+00005fterminate_backend"(1)"#,
        r#"SELECT (1).U&"pg\005fcancel\005fbackend""#,
        r#"SELECT U&"cron".schedule('* * * * *', 'select 1')"#,
        r#"SELECT U&"\0063ron".schedule('* * * * *', 'select 1')"#,
    ] {
        expect(sql, UnboundedWrite, &[ServerAdmin]);
    }
    expect(r#"SELECT U&"nextval"('s')"#, Write, &[SideEffectFunction]);
    expect(
        r#"SELECT U&"next\0076al"('s')"#,
        Write,
        &[SideEffectFunction],
    );
    // 대리 쌍도 푼다 — 목록 밖 이름이면 조회.
    expect(r#"SELECT U&"f\D83D\DE00"(1)"#, Read, &[]);
    expect(r#"SELECT U&"my\005ffunction"(1)"#, Read, &[]);
    // 풀 수 없는 이름(틀린 이스케이프 · 짝 없는 대리)과 `UESCAPE` 는 알 수 없어 조회로 두지 않는다.
    for sql in [
        r#"SELECT U&"pg\zzzzterminate"(1)"#,
        r#"SELECT U&"a\00"(1)"#,
        r#"SELECT U&"a\D83Dx"(1)"#,
        r#"SELECT U&"pg_catalog\zz".nextval('s')"#,
        r#"SELECT U&"a!0061" UESCAPE '!'(1)"#,
        // `FROM` 의 `U&` 표 함수는 파서가 못 읽는다.
        r#"SELECT * FROM U&"dblink"('host=x', 'select 1') AS d(a int)"#,
    ] {
        expect(sql, Destructive, &[Unparsed]);
    }
    // 큰따옴표 · 대소문자 · 공백 · 주석이 끼인 꼴은 파서가 조각으로 나눠 준다.
    for sql in [
        r#"SELECT "PG_TERMINATE_BACKEND"(1)"#,
        "SELECT pg_catalog . pg_terminate_backend /* x */ (1)",
        "SELECT pg_catalog\n.\n--x\npg_terminate_backend(1)",
    ] {
        expect(sql, UnboundedWrite, &[ServerAdmin]);
    }
}

/// PG 가 함수를 부르는 자리를 훑은 표(M3-03b): 집계의 `FILTER` · `ORDER BY` · `WITHIN GROUP`, 창 함수, `ROW` · 배열 생성자, 형
/// 변환, `LATERAL` · `FROM` 의 함수, `VALUES`, `GROUP BY` · `HAVING` · `DISTINCT ON` · `LIMIT`, 배열 첨자, 이름 붙은 인자. 모두 범위
/// 없는 쓰기 이상이어야 한다(파서가 못 읽는 꼴은 파괴).
#[test]
fn every_place_that_calls_a_function_is_seen() {
    for sql in [
        "SELECT count(*) FILTER (WHERE pg_terminate_backend(pid)) FROM pg_stat_activity",
        "SELECT string_agg(application_name, ',' ORDER BY pg_terminate_backend(pid)) FROM pg_stat_activity",
        "SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY pg_cancel_backend(pid)::int) FROM pg_stat_activity",
        "SELECT row_number() OVER (PARTITION BY pg_terminate_backend(pid)) FROM pg_stat_activity",
        "SELECT sum(1) OVER (ORDER BY pg_cancel_backend(pid)) FROM pg_stat_activity",
        "SELECT sum(1) OVER w FROM pg_stat_activity WINDOW w AS (ORDER BY pg_cancel_backend(pid))",
        "SELECT ROW(pg_terminate_backend(1))",
        "SELECT ARRAY[pg_terminate_backend(1)]",
        "SELECT ARRAY(SELECT pg_terminate_backend(1))",
        "SELECT CAST(pg_terminate_backend(1) AS text)",
        "SELECT pg_terminate_backend(1)::text",
        "SELECT * FROM pg_stat_activity a, LATERAL pg_terminate_backend(a.pid) k",
        "SELECT * FROM pg_terminate_backend(1)",
        "SELECT * FROM ROWS FROM (pg_terminate_backend(1))",
        "VALUES (pg_terminate_backend(1))",
        "SELECT 1 FROM t GROUP BY pg_cancel_backend(1)",
        "SELECT 1 FROM t HAVING pg_cancel_backend(1)",
        "SELECT DISTINCT ON (pg_cancel_backend(1)) 1",
        "SELECT 1 LIMIT pg_cancel_backend(1)::int",
        "SELECT (ARRAY[1])[pg_cancel_backend(1)::int]",
        "SELECT pg_terminate_backend(pid => 1)",
        "SELECT pg_terminate_backend(VARIADIC ARRAY[1])",
        "SELECT 1 WHERE 1 IN (pg_terminate_backend(1)::int)",
        "SELECT xmlelement(name a, pg_terminate_backend(1))",
        "SELECT json_build_object('a', pg_terminate_backend(1))",
        "INSERT INTO t (a) VALUES (pg_terminate_backend(1))",
        "UPDATE t SET a = pg_terminate_backend(1) WHERE id = 1",
        "DELETE FROM t WHERE id = 1 RETURNING pg_terminate_backend(1)",
        "CREATE TABLE x AS SELECT pg_terminate_backend(1)",
        "DECLARE c CURSOR FOR SELECT pg_terminate_backend(1)",
    ] {
        let got = classify_statement(Postgres, sql);
        assert!(got.grade >= UnboundedWrite, "{sql}: {got:?}");
    }
}

/// contrib 밖 확장의 SQL 글 실행 함수(M3-03b 리뷰 L1).
#[test]
fn common_extensions_that_run_sql_text_are_unbounded_writes() {
    for sql in [
        "SELECT * FROM pgr_dijkstra('select id, source, target, cost from edges', 1, 2)",
        "SELECT lives_ok('select 1')",
        "SELECT throws_ok('select 1', 'P0001')",
        "SELECT results_eq('select 1', 'select 1')",
        "SELECT is_empty('select 1')",
        "SELECT * FROM runtests()",
        "SELECT run_command_on_workers('select 1')",
        "SELECT oracle_execute('ora', 'begin null; end;')",
        "SELECT aws_s3.query_export_to_s3('select 1', 'b', 'k')",
        "SELECT graphql.resolve('mutation { x }')",
    ] {
        expect(sql, UnboundedWrite, &[ServerAdmin]);
    }
    // 이름이 흔한 것은 그 스키마로 한정했을 때만이다.
    expect("SELECT resolve('x')", Read, &[]);
    expect("SELECT query_export_to_s3('x')", Read, &[]);
}
