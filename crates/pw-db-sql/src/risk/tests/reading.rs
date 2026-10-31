//! 세션의 글 읽기를 바꾸는 문장(M3-03b 리뷰 M2, `risk/reading.rs`): 파괴 · `changesSqlReading` — 관문이 거절한다.

use pw_db_types::RiskGrade::Destructive;
use pw_db_types::SqlDialect::{self, Mysql, Postgres, Sqlite};

use crate::risk::RiskReason::ChangesSqlReading;
use crate::risk::classify_statement;

#[track_caller]
fn refused(dialect: SqlDialect, sql: &str) {
    let got = classify_statement(dialect, sql);
    assert!(
        got.grade == Destructive && got.reasons.contains(&ChangesSqlReading),
        "{dialect:?}: {sql}: {got:?}"
    );
}

#[track_caller]
fn allowed(dialect: SqlDialect, sql: &str) {
    let got = classify_statement(dialect, sql);
    assert!(
        !got.reasons.contains(&ChangesSqlReading),
        "{dialect:?}: {sql}: {got:?}"
    );
}

#[test]
fn postgres_settings_that_change_how_text_is_read_are_refused() {
    for sql in [
        "SET standard_conforming_strings = off",
        "SET standard_conforming_strings TO off",
        "SET LOCAL standard_conforming_strings = off",
        "SET SESSION standard_conforming_strings = 'off'",
        "SET standard_conforming_strings = DEFAULT",
        "SET standard_conforming_strings = on",
        "SET STANDARD_CONFORMING_STRINGS = off",
        "SET backslash_quote = on",
        "SET escape_string_warning = off",
        "SET client_encoding = 'SJIS'",
        "SET client_encoding TO 'BIG5'",
        "SET NAMES 'GBK'",
        "SELECT set_config('standard_conforming_strings', 'off', false)",
        "SELECT pg_catalog.set_config('standard_conforming_strings', 'off', true)",
        "SELECT set_config('STANDARD_CONFORMING_STRINGS', 'off', false)",
        "SELECT set_config(name, 'off', false) FROM t",
        "SELECT set_config('client_encoding', 'SJIS', false)",
        "SELECT set_config('client_encoding', v, false) FROM t",
        "SELECT * FROM set_config('standard_conforming_strings', 'off', false)",
        r#"SELECT U&"set\005fconfig"('standard_conforming_strings', 'off', false)"#,
        "SELECT 1 WHERE set_config('backslash_quote', 'on', false) IS NOT NULL",
        "UPDATE pg_settings SET setting = 'off' WHERE name = 'standard_conforming_strings'",
        "UPDATE pg_catalog.pg_settings SET setting = 'x' WHERE name = 'search_path'",
        "EXPLAIN ANALYZE UPDATE pg_settings SET setting = 'off' WHERE name = 'standard_conforming_strings'",
        "SET standard_conforming_strings = off; DELETE FROM t WHERE name = 'a\\'' OR true --'",
    ] {
        refused(Postgres, sql);
    }
    // 시작 옵션의 값으로 돌아가는 것과 다른 설정은 그대로다.
    for sql in [
        "SET search_path = x",
        "SET client_encoding = 'UTF8'",
        "SET client_encoding TO DEFAULT",
        "SET NAMES 'utf8'",
        "RESET standard_conforming_strings",
        "RESET ALL",
        "DISCARD ALL",
        "SELECT set_config('search_path', 'x', false)",
        "SELECT set_config('client_encoding', 'UTF8', false)",
        "SELECT current_setting('standard_conforming_strings')",
        "SELECT * FROM pg_settings WHERE name = 'standard_conforming_strings'",
    ] {
        allowed(Postgres, sql);
    }
}

#[test]
fn mysql_settings_that_change_how_text_is_read_are_refused() {
    for sql in [
        "SET sql_mode = 'NO_BACKSLASH_ESCAPES'",
        "SET SESSION sql_mode = 'STRICT_TRANS_TABLES,ANSI_QUOTES'",
        "SET sql_mode = 'ansi'",
        "SET @@session.sql_mode = 'ORACLE'",
        "SET @@sql_mode = 'NO_BACKSLASH_ESCAPES'",
        "SET SESSION sql_mode = CONCAT(@@sql_mode, ',NO_BACKSLASH_ESCAPES')",
        "SET sql_mode = @OLD_SQL_MODE",
        "SET sql_mode = DEFAULT",
        "SET GLOBAL sql_mode = 'NO_BACKSLASH_ESCAPES'",
        "SET @@sql_mode := 'ANSI'",
        "SET PERSIST sql_mode = 'ANSI_QUOTES'",
        "SET STATEMENT sql_mode = 'NO_BACKSLASH_ESCAPES' FOR SELECT 1",
        "SET NAMES gbk",
        "SET NAMES 'sjis'",
        "SET NAMES DEFAULT",
        "SET CHARACTER SET big5",
        "SET character_set_client = cp932",
        "SET @a = 1, character_set_client = gb18030",
        "/*!40101 SET SQL_MODE='NO_BACKSLASH_ESCAPES' */",
        "/*!50000 SET sql_mode = 'ANSI_QUOTES' */",
        "/*M! SET sql_mode = 'NO_BACKSLASH_ESCAPES' */",
    ] {
        refused(Mysql, sql);
    }
    for sql in [
        "SET sql_mode = 'STRICT_TRANS_TABLES'",
        "SET sql_mode = ''",
        "SET NAMES utf8mb4",
        "SET character_set_client = latin1",
        "SET @x = 'NO_BACKSLASH_ESCAPES'",
        "SELECT @@sql_mode",
        "/*!40101 SET NAMES utf8mb4 */",
        "/*!40101 SET @OLD_SQL_MODE=@@SQL_MODE, SQL_MODE='NO_AUTO_VALUE_ON_ZERO' */",
    ] {
        allowed(Mysql, sql);
    }
}

#[test]
fn sqlite_has_no_such_setting() {
    allowed(Sqlite, "PRAGMA encoding = 'UTF-16'");
}
