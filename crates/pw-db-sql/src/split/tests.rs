use std::sync::Arc;

use pw_rpc::{Caller, EventHub, Outbound, Role, Router, Session};
use serde_json::{Value, json};

use super::*;

const PG: SqlDialect = SqlDialect::Postgres;
const MY: SqlDialect = SqlDialect::Mysql;
const LITE: SqlDialect = SqlDialect::Sqlite;

/// 나눈 문장의 원문들.
fn texts(dialect: SqlDialect, sql: &str) -> Vec<&str> {
    split(dialect, sql).iter().map(|s| s.text(sql)).collect()
}

/// 표 시험 한 줄: 방언, 스크립트, 기대하는 문장들.
fn table(cases: &[(SqlDialect, &str, &[&str])]) {
    for (dialect, sql, want) in cases {
        assert_eq!(texts(*dialect, sql), *want, "{dialect:?}: {sql:?}");
    }
}

#[test]
fn a_semicolon_inside_a_dollar_quote_does_not_split() {
    table(&[
        (
            PG,
            "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql; SELECT 2",
            &[
                "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql",
                "SELECT 2",
            ],
        ),
        // 꼬리표가 있는 인용 안의 `$$` 와 `;` 는 닫지 않는다.
        (
            PG,
            "DO $fn$ BEGIN PERFORM '$$;'; END $fn$; SELECT 3;",
            &["DO $fn$ BEGIN PERFORM '$$;'; END $fn$", "SELECT 3"],
        ),
        // `$1` 은 매개변수, `a$b$` 는 식별자 — 달러 인용이 아니다.
        (
            PG,
            "SELECT $1; SELECT a$b$; SELECT 4",
            &["SELECT $1", "SELECT a$b$", "SELECT 4"],
        ),
        // 달러 인용은 PG 만이다.
        (MY, "SELECT $$; SELECT 5", &["SELECT $$", "SELECT 5"]),
    ]);
}

#[test]
fn a_semicolon_inside_a_comment_does_not_split() {
    table(&[
        (
            PG,
            "SELECT 1 -- a; b\n, 2; SELECT /* c; d */ 3",
            &["SELECT 1 -- a; b\n, 2", "SELECT /* c; d */ 3"],
        ),
        // PG 의 블록 주석은 겹친다.
        (
            PG,
            "SELECT /* a /* b; */ c; */ 1; SELECT 2",
            &["SELECT /* a /* b; */ c; */ 1", "SELECT 2"],
        ),
        // MySQL 은 겹치지 않는다 — 첫 `*/` 에서 닫힌다.
        (
            MY,
            "SELECT /* a /* b */ 1; SELECT 2",
            &["SELECT /* a /* b */ 1", "SELECT 2"],
        ),
        (MY, "SELECT 1 # a; b\n; SELECT 2", &["SELECT 1", "SELECT 2"]),
        // `#` 은 MySQL 만의 주석이다(PG 에서는 연산자).
        (PG, "SELECT 1 # 2; SELECT 3", &["SELECT 1 # 2", "SELECT 3"]),
        // MySQL 의 `--` 는 뒤에 공백이 있어야 주석이다.
        (MY, "SELECT 1--1; SELECT 2", &["SELECT 1--1", "SELECT 2"]),
        (
            PG,
            "SELECT 1--1; SELECT 2\n; SELECT 3",
            &["SELECT 1", "SELECT 3"],
        ),
        (
            LITE,
            "SELECT 1 /* ; */ + 1; -- ;\nSELECT 2",
            &["SELECT 1 /* ; */ + 1", "SELECT 2"],
        ),
    ]);
}

#[test]
fn a_comment_marker_inside_a_string_is_text() {
    table(&[
        (
            PG,
            "SELECT '--;', '/*;' ; SELECT \"a--;b\" FROM t",
            &["SELECT '--;', '/*;'", "SELECT \"a--;b\" FROM t"],
        ),
        (
            MY,
            "SELECT '-- ;', \"# ;\", `a;--b`; SELECT 2",
            &["SELECT '-- ;', \"# ;\", `a;--b`", "SELECT 2"],
        ),
        (
            LITE,
            "SELECT '--;', [a;--b], `c;d`; SELECT 2",
            &["SELECT '--;', [a;--b], `c;d`", "SELECT 2"],
        ),
        // 같은 따옴표 둘은 글자 하나다.
        (
            PG,
            "SELECT 'it''s; --'; SELECT 2",
            &["SELECT 'it''s; --'", "SELECT 2"],
        ),
    ]);
}

#[test]
fn backslash_escapes_follow_the_dialect() {
    table(&[
        // MySQL 의 문자열은 백슬래시가 따옴표를 감춘다.
        (
            MY,
            r"SELECT 'a\'; b'; SELECT 2",
            &[r"SELECT 'a\'; b'", "SELECT 2"],
        ),
        (
            MY,
            r#"SELECT "a\"; b"; SELECT 2"#,
            &[r#"SELECT "a\"; b""#, "SELECT 2"],
        ),
        // PG 표준 문자열에서는 아니다 — `E'…'` 에서만.
        (PG, r"SELECT 'a\'; SELECT 2", &[r"SELECT 'a\'", "SELECT 2"]),
        (
            PG,
            r"SELECT E'a\'; b'; SELECT 2",
            &[r"SELECT E'a\'; b'", "SELECT 2"],
        ),
        (
            LITE,
            r"SELECT 'a\'; SELECT 2",
            &[r"SELECT 'a\'", "SELECT 2"],
        ),
    ]);
}

#[test]
fn a_procedure_defined_under_delimiter_is_one_statement() {
    let sql = "SELECT 0;\n\
               DELIMITER $$\n\
               CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  IF x THEN SELECT 2; END IF;\nEND$$\n\
               CREATE FUNCTION f() RETURNS int RETURN 1 $$\n\
               DELIMITER ;\n\
               CALL p();\n\
               SELECT 3";
    assert_eq!(
        texts(MY, sql),
        [
            "SELECT 0",
            "CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  IF x THEN SELECT 2; END IF;\nEND",
            "CREATE FUNCTION f() RETURNS int RETURN 1",
            "CALL p()",
            "SELECT 3",
        ]
    );
    // 바꾼 구분자는 블록과 상관없이 나누고, `;` 는 나누지 않는다. 따옴표로 감싼 인자도 받는다.
    table(&[
        (
            MY,
            "delimiter //\nSELECT 1; SELECT 2//SELECT 3 //",
            &["SELECT 1; SELECT 2", "SELECT 3"],
        ),
        (
            MY,
            "DELIMITER '$$'\nSELECT 1$$SELECT 2",
            &["SELECT 1", "SELECT 2"],
        ),
        // 인자가 없으면 줄만 건너뛰고 구분자를 그대로 둔다.
        (
            MY,
            "DELIMITER \nSELECT 1; SELECT 2",
            &["SELECT 1", "SELECT 2"],
        ),
        // `DELIMITER` 는 MySQL 만의 클라이언트 명령이고 문장 머리에서만이다.
        (
            PG,
            "DELIMITER //\nSELECT 1; SELECT 2",
            &["DELIMITER //\nSELECT 1", "SELECT 2"],
        ),
        (
            MY,
            "SELECT delimiter FROM t; SELECT 2",
            &["SELECT delimiter FROM t", "SELECT 2"],
        ),
    ]);
}

#[test]
fn routine_bodies_keep_their_semicolons_without_delimiter() {
    table(&[
        // MySQL 저장 프로시저: 겹친 블록 · CASE 문 · 레이블.
        (
            MY,
            "CREATE DEFINER=`root`@`%` PROCEDURE p() lbl: BEGIN DECLARE a int; BEGIN SET a = 1; END; \
             CASE a WHEN 1 THEN SELECT 1; END CASE; WHILE a < 3 DO SET a = a + 1; END WHILE; \
             SELECT CASE WHEN a THEN 1 END; END lbl; SELECT 2",
            &[
                "CREATE DEFINER=`root`@`%` PROCEDURE p() lbl: BEGIN DECLARE a int; BEGIN SET a = 1; END; \
                 CASE a WHEN 1 THEN SELECT 1; END CASE; WHILE a < 3 DO SET a = a + 1; END WHILE; \
                 SELECT CASE WHEN a THEN 1 END; END lbl",
                "SELECT 2",
            ],
        ),
        (
            MY,
            "CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW BEGIN SET NEW.a = 1; SET NEW.b = 2; END; SELECT 1",
            &[
                "CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW BEGIN SET NEW.a = 1; SET NEW.b = 2; END",
                "SELECT 1",
            ],
        ),
        (
            MY,
            "CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO BEGIN DELETE FROM a; DELETE FROM b; END; SELECT 1",
            &[
                "CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO BEGIN DELETE FROM a; DELETE FROM b; END",
                "SELECT 1",
            ],
        ),
        // MariaDB 의 루틴 밖 복합문.
        (
            MY,
            "BEGIN NOT ATOMIC SELECT 1; SELECT 2; END; SELECT 3",
            &["BEGIN NOT ATOMIC SELECT 1; SELECT 2; END", "SELECT 3"],
        ),
        // SQLite 트리거. WHEN 절의 CASE … END 는 블록 밖이다.
        (
            LITE,
            "CREATE TEMP TRIGGER IF NOT EXISTS t AFTER INSERT ON x WHEN CASE WHEN 1 THEN 1 END \
             BEGIN UPDATE y SET a = CASE WHEN 1 THEN 2 END; DELETE FROM z; END; SELECT 1",
            &[
                "CREATE TEMP TRIGGER IF NOT EXISTS t AFTER INSERT ON x WHEN CASE WHEN 1 THEN 1 END \
                 BEGIN UPDATE y SET a = CASE WHEN 1 THEN 2 END; DELETE FROM z; END",
                "SELECT 1",
            ],
        ),
        // PG 14 의 SQL 표준 본문.
        (
            PG,
            "CREATE OR REPLACE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; SELECT 2; END; SELECT 3",
            &[
                "CREATE OR REPLACE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; SELECT 2; END",
                "SELECT 3",
            ],
        ),
    ]);
}

#[test]
fn begin_outside_a_routine_does_not_open_a_block() {
    table(&[
        // 트랜잭션 시작.
        (
            PG,
            "BEGIN; UPDATE t SET a = 1; END; COMMIT",
            &["BEGIN", "UPDATE t SET a = 1", "END", "COMMIT"],
        ),
        (
            MY,
            "BEGIN; SELECT 1; COMMIT",
            &["BEGIN", "SELECT 1", "COMMIT"],
        ),
        (
            LITE,
            "BEGIN IMMEDIATE; SELECT 1; END",
            &["BEGIN IMMEDIATE", "SELECT 1", "END"],
        ),
        // 열 이름이 event · begin 이어도 표가 먼저 나온 CREATE 는 루틴이 아니다(뒤에 `end` 열이 없어 블록이 열리면 닫히지
        // 않는다).
        (
            MY,
            "CREATE TABLE log (event varchar(9), begin datetime); SELECT 1",
            &[
                "CREATE TABLE log (event varchar(9), begin datetime)",
                "SELECT 1",
            ],
        ),
        // 루틴이 아닌 문장의 `begin` 열.
        (
            MY,
            "SELECT begin FROM t; SELECT 2",
            &["SELECT begin FROM t", "SELECT 2"],
        ),
        // 본문 없는 루틴.
        (
            MY,
            "CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW SET NEW.a = 1; SELECT 1",
            &[
                "CREATE TRIGGER t BEFORE INSERT ON x FOR EACH ROW SET NEW.a = 1",
                "SELECT 1",
            ],
        ),
        (
            PG,
            "SELECT CASE WHEN a THEN 1 END; SELECT 2",
            &["SELECT CASE WHEN a THEN 1 END", "SELECT 2"],
        ),
    ]);
}

#[test]
fn the_last_statement_needs_no_semicolon() {
    table(&[
        (PG, "SELECT 1; SELECT 2", &["SELECT 1", "SELECT 2"]),
        (MY, "SELECT 1;\nSELECT 2\n", &["SELECT 1", "SELECT 2"]),
        (LITE, "SELECT 1", &["SELECT 1"]),
        // 끝의 주석 · 공백은 범위에 들지 않는다.
        (PG, "SELECT 1 -- 끝\n  ", &["SELECT 1"]),
    ]);
}

#[test]
fn whitespace_and_comment_only_chunks_are_not_statements() {
    table(&[
        (PG, "", &[]),
        (PG, " ;; \n ; ", &[]),
        (PG, "SELECT 1;; ;SELECT 2;", &["SELECT 1", "SELECT 2"]),
        (PG, "-- 머리\n/* 설명 */ SELECT 1; -- 꼬리", &["SELECT 1"]),
        // 문장 앞의 주석은 범위에 들지 않는다 — 윗줄 끝 주석이 다음 문장에 붙지 않게.
        (PG, "SELECT 1; -- 하나\nSELECT 2", &["SELECT 1", "SELECT 2"]),
        // MySQL 실행 주석은 내용이다.
        (
            MY,
            "/*!40101 SET NAMES utf8 */;\n/* 그냥 주석 */;\nSELECT 1",
            &["/*!40101 SET NAMES utf8 */", "SELECT 1"],
        ),
    ]);
}

#[test]
fn unterminated_quotes_run_to_the_end() {
    table(&[
        (
            PG,
            "SELECT 1; SELECT 'a; SELECT 2",
            &["SELECT 1", "SELECT 'a; SELECT 2"],
        ),
        (
            PG,
            "SELECT 1; SELECT $$a; SELECT 2",
            &["SELECT 1", "SELECT $$a; SELECT 2"],
        ),
        // 닫히지 않은 주석은 내용이 아니므로 앞 토큰에서 끝난다.
        (LITE, "SELECT 1 /* a; SELECT 2", &["SELECT 1"]),
    ]);
}

/// 한글(BMP — UTF-16 한 단위, UTF-8 세 바이트)과 이모지(UTF-16 두 단위)가 섞인 문장의 범위는 UTF-16 이다.
#[test]
fn ranges_are_utf16_offsets() {
    let sql = "SELECT '한글'; SELECT '😀' AS 이모지;\nSELECT 3";
    let got = split(PG, sql);
    let ranges: Vec<_> = got.iter().map(|s| (s.range.from, s.range.to)).collect();
    // "SELECT '한글'" = 11 단위, "; " 뒤 "SELECT '😀' AS 이모지" = 6+1+1+2+1+1+2+1+3 = 18 단위.
    assert_eq!(ranges, [(0, 11), (13, 31), (33, 41)]);
    let utf16: Vec<u16> = sql.encode_utf16().collect();
    for s in &got {
        let by_range =
            String::from_utf16(&utf16[s.range.from as usize..s.range.to as usize]).unwrap();
        assert_eq!(by_range, s.text(sql));
    }
    assert_eq!(got[1].text(sql), "SELECT '😀' AS 이모지");
}

#[test]
fn the_statement_at_the_cursor() {
    // 「SELECT 1」 0..8, `;` 8, 빈 줄 · 들여쓰기 9..13, 「SELECT 2」 13..21, `;` 21, 줄바꿈 22, 「SELECT 3」 23..31.
    let sql = "SELECT 1;\n\n  SELECT 2;\nSELECT 3";
    let at = |offset| statement_at(PG, sql, offset).map(|(i, s)| (i, s.text(sql).to_owned()));
    let first = Some((0, "SELECT 1".to_owned()));
    let second = Some((1, "SELECT 2".to_owned()));
    let third = Some((2, "SELECT 3".to_owned()));
    assert_eq!(at(0), first);
    assert_eq!(at(4), first);
    // 문장 끝(`;` 앞)과 `;` 뒤, 두 문장 사이의 빈 줄 · 들여쓰기는 앞 문장이다.
    assert_eq!(at(8), first);
    assert_eq!(at(9), first);
    assert_eq!(at(10), first);
    assert_eq!(at(12), first);
    // 다음 문장의 첫 글자부터는 그 문장이다.
    assert_eq!(at(13), second);
    assert_eq!(at(21), second);
    assert_eq!(at(22), second);
    assert_eq!(at(23), third);
    assert_eq!(at(31), third);
    // 첫 문장 앞이면 첫 문장, 문장이 없으면 없다.
    assert_eq!(
        statement_at(PG, "  \n SELECT 1", 1).map(|(i, _)| i),
        Some(0)
    );
    assert_eq!(statement_at(PG, " -- 주석만\n", 3), None);
    // 빈 줄이 아니라 `;` 기준이다 — 빈 줄로 갈린 한 문장은 하나다.
    let sql = "SELECT 1\n\nFROM t;\nSELECT 2";
    assert_eq!(
        statement_at(PG, sql, 10).unwrap().1.text(sql),
        "SELECT 1\n\nFROM t"
    );
}

/// RPC 세션으로 부른다.
async fn call(method: &str, params: Value) -> Value {
    let mut router = Router::new();
    register(&mut router);
    let (session, mut rx) = Session::new(
        Arc::new(router),
        EventHub::new(),
        Caller::new(Role::Readonly),
    );
    session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
    match rx.recv().await.unwrap() {
        reply @ Outbound::Response(_) => serde_json::to_value(reply).unwrap(),
        other => panic!("expected a response, got {other:?}"),
    }
}

#[tokio::test]
async fn split_and_statement_at_over_rpc() {
    let sql = "SELECT '한'; SELECT 2";
    let reply = call("db.sql.split", json!({"dialect": "postgres", "sql": sql})).await;
    assert_eq!(
        reply["result"],
        json!({"statements": [{"from": 0, "to": 10}, {"from": 12, "to": 20}]})
    );
    let reply = call(
        "db.sql.statementAt",
        json!({"dialect": "mysql", "sql": sql, "offset": 11}),
    )
    .await;
    assert_eq!(
        reply["result"],
        json!({"statement": {"index": 0, "range": {"from": 0, "to": 10}}})
    );
    let reply = call(
        "db.sql.statementAt",
        json!({"dialect": "sqlite", "sql": "  ", "offset": 2}),
    )
    .await;
    assert_eq!(reply["result"], json!({"statement": null}));
}

#[tokio::test]
async fn an_offset_past_the_end_is_invalid() {
    // 20 단위의 끝(20)은 받고 21 은 거절한다.
    let sql = "SELECT '한'; SELECT 2";
    let end = call(
        "db.sql.statementAt",
        json!({"dialect": "postgres", "sql": sql, "offset": 20}),
    )
    .await;
    assert_eq!(end["result"]["statement"]["index"], 1);
    let past = call(
        "db.sql.statementAt",
        json!({"dialect": "postgres", "sql": sql, "offset": 21}),
    )
    .await;
    assert_eq!(past["error"]["code"], pw_rpc::code::INVALID_PARAMS);
}

/// 완료 기준: 1 MiB 스크립트(문장 1만 개) 나누기가 release 에서 50 ms 안. `mise run test-release` 가 이름으로 고른다
/// (`cargo nextest run --release -p pw-db-sql --run-ignored only -E 'test(=split::tests::a_1mib_script_of_10000_statements_splits_within_50ms)'`).
/// debug 빌드에서는 재기만 하고 판정하지 않는다.
#[test]
#[ignore = "release 빌드 성능 측정(위 명령)"]
fn a_1mib_script_of_10000_statements_splits_within_50ms() {
    // 따옴표 · 주석 · 한글 · 달러 인용 · 블록을 섞은 문장 다섯 갈래를 돌려 쓰고, 문장마다 공백으로 105 바이트 안팎을 채운다.
    let shapes = [
        "SELECT a, b, '문자열; -- 아님' FROM t WHERE id = 1",
        "UPDATE t SET name = 'it''s' /* 주석; */ WHERE id = 2",
        "INSERT INTO \"표\" (a) VALUES ($$x;y$$)",
        "CREATE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; END",
        "DELETE FROM t -- 꼬리 주석;\nWHERE id = 3",
    ];
    let mut sql = String::with_capacity(1 << 20);
    for i in 0..10_000 {
        let shape = shapes[i % shapes.len()];
        sql.push_str(shape);
        sql.push_str(";\n");
        let target = (i + 1) * (1 << 20) / 10_000;
        while sql.len() < target {
            sql.push(' ');
        }
    }
    assert!(sql.len() >= 1 << 20, "{}", sql.len());
    assert_eq!(split(PG, &sql).len(), 10_000);
    let mut times = Vec::new();
    for _ in 0..11 {
        let started = std::time::Instant::now();
        let statements = split(PG, &sql);
        times.push(started.elapsed());
        assert_eq!(statements.len(), 10_000);
    }
    times.sort();
    let median = times[times.len() / 2];
    eprintln!(
        "split of {} bytes: median {median:?}, all {times:?}",
        sql.len()
    );
    if !cfg!(debug_assertions) {
        assert!(median < std::time::Duration::from_millis(50), "{median:?}");
    }
}
