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
        "SET standard_conforming_strings = 0",
        "SET standard_conforming_strings TO false",
        "SET standard_conforming_strings = 'of'",
        "SET client_encoding TO DEFAULT",
        "SET client_encoding = 'LATIN1'",
        "SELECT set_config('standard_conforming_strings', current_setting('x'), false)",
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
        // 허용 목록(M3-03b 재리뷰): 켜짐은 시작 옵션의 값과 같다 — pg_dump 머리가 지난다.
        "SET standard_conforming_strings = on",
        "SET standard_conforming_strings TO true",
        "SET standard_conforming_strings = 1",
        "SET standard_conforming_strings = 'on'",
        "SELECT set_config('standard_conforming_strings', 'on', false)",
        "SET statement_timeout = 0",
        "SET lock_timeout = 0",
        "SET idle_in_transaction_session_timeout = 0",
        "SET client_encoding = 'UTF8'; SET standard_conforming_strings = on",
        "SELECT pg_catalog.set_config('search_path', '', false)",
        "SET check_function_bodies = false",
        "SET xmloption = content",
        "SET client_min_messages = warning",
        "SET row_security = off",
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
        // 수 값(M3-03b 재리뷰 N1): MySQL 은 `sql_mode` 를 비트 수로, 문자 집합을 번호로 받는다.
        "SET sql_mode = 1048576",
        "SET SESSION sql_mode = 4",
        "SET sql_mode = 262144",
        "SET sql_mode = 0",
        "SET sql_mode = TRUE",
        "SET character_set_client = 28",
        "SET character_set_connection = 28",
        "SET character_set_connection = gbk",
        "SET sql_mode = 0x100000",
        "SET NAMES 'notacharset'",
        "SET sql_mode = 'DEFAULT'",
        // mysqldump 의 표마다 있는 복원 — 값을 모른다.
        "/*!40101 SET character_set_client = @saved_cs_client */",
        "/*!40101 SET SQL_MODE=@OLD_SQL_MODE */",
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
        "/*!40101 SET character_set_client = utf8mb4 */",
        "SET NAMES koi8r",
        "SET NAMES utf8mb4 COLLATE utf8mb4_0900_ai_ci",
        "SET character_set_connection = utf8mb4",
        "SET sql_mode = 'TRADITIONAL'",
        "SET sql_mode = STRICT_ALL_TABLES",
    ] {
        allowed(Mysql, sql);
    }
}

#[test]
fn sqlite_has_no_such_setting() {
    allowed(Sqlite, "PRAGMA encoding = 'UTF-16'");
}
