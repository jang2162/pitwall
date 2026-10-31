//! 분류기와 서버의 글 읽기를 바꾸는 세션 설정(M3-03b 리뷰 M2). 분류기 · 나누기는 세션이 열릴 때의 읽기(PG
//! `standard_conforming_strings = on` — 드라이버가 시작 옵션으로 고정한다, MySQL 은 백슬래시 두 읽기 · UTF-8)로 글을 읽는다. 세션에서
//! 그 읽기를 바꾸면 같은 세션의 다음 글을 서버가 다르게 읽는다 — `SET standard_conforming_strings = off; DELETE … WHERE name =
//! 'a\'' OR true --'` 는 분류기가 범위 있는 `DELETE` 로, 서버가 범위 없는 `DELETE` 로 읽었다(PG 17.6). 그래서 이런 문장은 등급이
//! 아니라 거절이다([`RiskReason::ChangesSqlReading`] — 관문이 그 까닭이 든 문장을 돌리지 않는다).
//!
//! 훑은 설정(PG 17 · MySQL 8.4 · MariaDB 11.8 문서와 소스의 렉서 입력):
//! - PG `standard_conforming_strings`(`'…'` 안의 `\`), `backslash_quote`(`\'` 를 받는가), `escape_string_warning`(경고뿐이라 읽기를
//!   바꾸지 않지만 같은 무리라 함께 막는다), `client_encoding`(서버는 받은 바이트를 그 인코딩으로 읽어 서버 인코딩으로 바꾼 뒤
//!   렉싱한다 — 뒤 바이트가 `\` 일 수 있는 클라이언트 전용 인코딩 BIG5 · GBK · GB18030 · SJIS · SHIFT_JIS_2004 · UHC · JOHAB 이면
//!   UTF-8 글의 바이트가 다른 글자가 된다). 꼴: `SET [SESSION | LOCAL] 이름 { = | TO } 값`, `SET NAMES`, `set_config('이름', …)`
//!   (이름이 리터럴이 아니면 알 수 없어 막는다), `UPDATE pg_settings …`(규칙이 `set_config` 를 부른다). `RESET` · `DISCARD ALL` 은
//!   시작 옵션의 값으로 돌아가므로 막지 않는다.
//! - MySQL · MariaDB `sql_mode` 의 `NO_BACKSLASH_ESCAPES` · `ANSI_QUOTES`(묶음 모드 `ANSI` · `DB2` · `MAXDB` · `MSSQL` · `ORACLE` ·
//!   `POSTGRESQL` 이 `ANSI_QUOTES` 를 담는다), `character_set_client` · `SET NAMES` · `SET CHARACTER SET` 의 BIG5 · CP932 · GBK ·
//!   GB18030 · SJIS(둘째 바이트가 `\` 일 수 있다 — 서버는 그 바이트를 앞 글자의 일부로 읽는다). 꼴: `SET [SESSION |
//!   GLOBAL | PERSIST | LOCAL] 이름 = 값`, `SET @@[scope.]이름 = 값`, 여럿을 쉼표로, MariaDB `SET STATEMENT 이름 = 값 FOR …`. 값이 리터럴이
//!   아니면(`CONCAT(@@sql_mode, …)` · `@OLD_SQL_MODE` · `DEFAULT`) 알 수 없어 막는다 — mysqldump 끝의 `SET SQL_MODE=@OLD_SQL_MODE` 도
//!   막힌다.
//! - SQLite 에는 렉싱을 바꾸는 설정이 없다(`PRAGMA` 는 이미 쓰기 이상).
//!
//! 파서가 못 읽는 `SET`(`SET @@sql_mode := …` · `SET PERSIST …` · `SET CHARACTER SET …` · `SET STATEMENT …`)은 토큰으로 본다 — 위
//! 이름이 하나라도 있으면 막는다([`unparsed_set_changes_reading`]).

use pw_db_types::SqlDialect;
use sqlparser::ast::{
    Expr, FunctionArg, FunctionArgExpr, Ident, ObjectName, Set, Statement, TableFactor, Value,
};
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, TokenWithSpan};

/// PG: 값과 상관없이 막는 설정.
const PG_ALWAYS: &[&str] = &[
    "standard_conforming_strings",
    "backslash_quote",
    "escape_string_warning",
];

/// PG `client_encoding` 으로 받는 값 — 드라이버(`tokio-postgres`)가 시작 때 고르는 UTF8 과 그 별칭, 시작 값으로 돌아가는 `DEFAULT`.
const PG_SAFE_ENCODINGS: &[&str] = &["utf8", "utf-8", "unicode", "default"];

/// MySQL `sql_mode` 에 있으면 막는 모드(위 머리 주석).
const MYSQL_UNSAFE_MODES: &[&str] = &[
    "no_backslash_escapes",
    "ansi_quotes",
    "ansi",
    "db2",
    "maxdb",
    "mssql",
    "oracle",
    "postgresql",
];

/// MySQL 의 클라이언트 문자 집합으로 두면 막는 것(둘째 바이트가 `\` 일 수 있다).
const MYSQL_UNSAFE_CHARSETS: &[&str] = &["big5", "cp932", "gbk", "gb18030", "sjis"];

/// 파서가 못 읽는 `SET` 에서 찾는 이름(두 방언을 합친다 — 못 읽는 글이라 넓게).
const UNPARSED_SET_WORDS: &[&str] = &[
    "standard_conforming_strings",
    "backslash_quote",
    "escape_string_warning",
    "client_encoding",
    "sql_mode",
    "character_set_client",
    "names",
    "charset",
    "character",
];

/// 문장이 글 읽기를 바꾸는가(`SET` · `UPDATE pg_settings`). 함수 꼴(`set_config`)은 [`set_config_changes_reading`].
pub(super) fn statement_changes_reading(statement: &Statement, dialect: SqlDialect) -> bool {
    match statement {
        Statement::Set(set) => set_changes_reading(set, dialect),
        Statement::Update(update) if dialect == SqlDialect::Postgres => {
            matches!(&update.table.relation, TableFactor::Table { name, .. }
                if last_name(name).as_deref() == Some("pg_settings"))
        }
        _ => false,
    }
}

fn set_changes_reading(set: &Set, dialect: SqlDialect) -> bool {
    match set {
        Set::SingleAssignment {
            variable, values, ..
        } => match values.as_slice() {
            [value] => assignment_changes_reading(variable, value, dialect),
            // 값이 여럿(PG `SET search_path = a, b`)이면 이름만으로 본다.
            _ => last_name(variable).is_some_and(|name| watched(&name, dialect)),
        },
        Set::MultipleAssignments { assignments } => assignments
            .iter()
            .any(|a| assignment_changes_reading(&a.name, &a.value, dialect)),
        Set::ParenthesizedAssignments { variables, .. } => variables
            .iter()
            .any(|v| last_name(v).is_some_and(|name| watched(&name, dialect))),
        Set::SetNames { charset_name, .. } => {
            !encoding_is_safe(&charset_name.value.to_ascii_lowercase(), dialect)
        }
        Set::SetNamesDefault {} => dialect == SqlDialect::Mysql,
        _ => false,
    }
}

/// 이 이름이 이 방언에서 글 읽기에 닿는 설정인가.
fn watched(name: &str, dialect: SqlDialect) -> bool {
    match dialect {
        SqlDialect::Postgres => PG_ALWAYS.contains(&name) || name == "client_encoding",
        SqlDialect::Mysql => name == "sql_mode" || name == "character_set_client",
        SqlDialect::Sqlite => false,
    }
}

fn assignment_changes_reading(variable: &ObjectName, value: &Expr, dialect: SqlDialect) -> bool {
    let Some(name) = last_name(variable) else {
        return false;
    };
    if !watched(&name, dialect) {
        return false;
    }
    let Some(value) = literal(value) else {
        return true;
    };
    let value = value.to_ascii_lowercase();
    match name.as_str() {
        "client_encoding" | "character_set_client" => !encoding_is_safe(&value, dialect),
        "sql_mode" => {
            value == "default"
                || value
                    .split(',')
                    .any(|mode| MYSQL_UNSAFE_MODES.contains(&mode.trim()))
        }
        _ => true,
    }
}

fn encoding_is_safe(value: &str, dialect: SqlDialect) -> bool {
    match dialect {
        SqlDialect::Postgres => PG_SAFE_ENCODINGS.contains(&value),
        SqlDialect::Mysql => value != "default" && !MYSQL_UNSAFE_CHARSETS.contains(&value),
        SqlDialect::Sqlite => true,
    }
}

/// PG `set_config(이름, 값, …)` 이 글 읽기를 바꾸는가. 이름 · 값이 리터럴이 아니면 알 수 없어 참이다.
pub(super) fn set_config_changes_reading(args: &[FunctionArg]) -> bool {
    // 함수 인자의 따옴표 없는 낱말은 열 이름이다 — 따옴표 글만 리터럴로 본다.
    let positional = |i: usize| match args.get(i) {
        Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(value)))) => match &value.value
        {
            Value::SingleQuotedString(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    };
    let Some(name) = positional(0) else {
        return true;
    };
    let name = name.to_ascii_lowercase();
    if PG_ALWAYS.contains(&name.as_str()) {
        return true;
    }
    if name == "client_encoding" {
        return positional(1).is_none_or(|value| {
            !encoding_is_safe(&value.to_ascii_lowercase(), SqlDialect::Postgres)
        });
    }
    false
}

/// 파서가 못 읽은 글이 `SET` 으로 시작하고 [`UNPARSED_SET_WORDS`] 의 이름을 담는가.
pub(super) fn unparsed_set_changes_reading(tokens: &[TokenWithSpan]) -> bool {
    let mut words = tokens.iter().filter_map(|t| match &t.token {
        Token::Word(word) => Some(word),
        _ => None,
    });
    let starts_with_set = tokens
        .iter()
        .map(|t| &t.token)
        .find(|t| !matches!(t, Token::Whitespace(_) | Token::SemiColon))
        .is_some_and(|t| matches!(t, Token::Word(w) if w.keyword == Keyword::SET));
    starts_with_set
        && words.any(|word| {
            let value = word.value.trim_start_matches('@').to_ascii_lowercase();
            let last = value.rsplit('.').next().unwrap_or_default();
            UNPARSED_SET_WORDS.contains(&last)
        })
}

/// 변수 이름의 마지막 조각(소문자, MySQL 의 `@@` 를 뗀다).
fn last_name(name: &ObjectName) -> Option<String> {
    let ident: &Ident = name.0.last()?.as_ident()?;
    Some(ident.value.trim_start_matches('@').to_ascii_lowercase())
}

/// 리터럴 값(따옴표 글 · 따옴표 없는 낱말). 그 밖의 식은 `None`.
fn literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Value(value) => match &value.value {
            Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => Some(s.clone()),
            Value::Boolean(b) => Some(b.to_string()),
            Value::Number(n, _) => Some(n.clone()),
            _ => None,
        },
        // `@변수` · `@@설정` 은 값을 모른다.
        Expr::Identifier(ident) if !ident.value.starts_with('@') => Some(ident.value.clone()),
        _ => None,
    }
}
