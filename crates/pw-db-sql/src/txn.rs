//! 트랜잭션을 열고 닫는 문장과 MySQL 의 암묵 커밋 문장을 가려낸다(실행 파이프라인 M3-05).
//!
//! - [`transaction_control`]: 사람이 친 `BEGIN` · `START TRANSACTION` · `COMMIT` · `END` · `ROLLBACK` · `ABORT`(`AND CHAIN` 포함)는
//!   파이프라인이 드라이버의 `begin` · `commit` · `rollback` 메서드로 옮긴다 — 드라이버는 이 문장을 SQL 로 받지 않는다(PG 드라이버는
//!   서버의 트랜잭션 상태를 모르므로 SQL 로 보내면 세션 상태가 서버와 갈린다, M3-06). 메서드가 옮기지 못하는 뜻(`ISOLATION LEVEL` ·
//!   `WITH CONSISTENT SNAPSHOT` · SQLite 의 `IMMEDIATE` · MySQL 의 `RELEASE`)이 붙어 있으면 조용히 버리지 않고 거절한다.
//!   `PREPARE TRANSACTION`(2단계 커밋)도 옮길 메서드가 없어 거절이다. `SAVEPOINT` · `RELEASE SAVEPOINT` · `ROLLBACK TO` 와
//!   `COMMIT PREPARED` · `ROLLBACK PREPARED` 는 트랜잭션 블록을 열거나 닫지 않으므로 여기서 고르지 않는다(SQL 그대로 간다).
//! - [`changes_session_settings`]: `SET` · `RESET` — read-only 연결의 관문이 거절한다(결정 M3-D6).
//! - [`implicitly_commits`]: MySQL · MariaDB 에서 열린 트랜잭션을 실행 전에 커밋하는 문장(DDL · `LOCK TABLES` · `TRUNCATE` 등,
//!   MySQL 8.4 매뉴얼 「Statements That Cause an Implicit Commit」). 수동 트랜잭션에서 실행하기 전에 확인을 받는다(결정 M3-D21).
//!
//! 위험도 등급은 이것과 상관없이 [`crate::risk::classify_statement`] 가 매긴다 — 여기서 놓친 문장도 등급은 쓰기 이상이고, 드라이버의
//! 상태 보고(PG 의 숨은 블록, MySQL 의 상태 플래그, SQLite 의 `is_autocommit`)로 세션 상태가 맞춰진다. 읽기는 앞 낱말만 본다: 공백,
//! `--` · `/* */`(겹침) 주석, MySQL 의 `#` 주석과 실행 주석 `/*! … */`(안은 코드로 읽는다)을 지나 식별자 낱말을 모은다.

use pw_db_types::SqlDialect;

/// 트랜잭션 제어 문장을 옮긴 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxControl {
    /// 트랜잭션을 연다. `read_only` 는 `READ ONLY`(true) · `READ WRITE`(false), 없으면 `None`.
    Begin { read_only: Option<bool> },
    /// 커밋한다. `chain` 이면 같은 특성의 트랜잭션을 곧바로 다시 연다(`AND CHAIN`).
    Commit { chain: bool },
    /// 롤백한다. `chain` 은 위와 같다.
    Rollback { chain: bool },
    /// 트랜잭션 제어 문장이지만 메서드로 옮길 수 없다 — 사람이 읽는 까닭.
    Refused(&'static str),
}

const UNSUPPORTED_OPTION: &str = "this transaction statement has options the console cannot carry over (isolation level, consistent snapshot, IMMEDIATE / EXCLUSIVE, RELEASE): run BEGIN on its own and set the options with SET TRANSACTION, or use the console's transaction mode";
const PREPARE_TRANSACTION: &str = "PREPARE TRANSACTION (two-phase commit) is not supported in the console; commit or roll back the transaction instead";

/// 문장 하나가 트랜잭션 제어 문장이면 옮긴 것. 아니면 `None`(SQL 그대로 드라이버에 간다).
pub fn transaction_control(dialect: SqlDialect, text: &str) -> Option<TxControl> {
    let lexed = lex(dialect, text, 12);
    let words: Vec<&str> = lexed.words.iter().map(String::as_str).collect();
    let (first, rest) = words.split_first()?;
    match *first {
        "BEGIN" => begin(dialect, rest, lexed.rest),
        "START" if rest.first() == Some(&"TRANSACTION") => start(dialect, &rest[1..], lexed.rest),
        "COMMIT" => {
            if rest.first() == Some(&"PREPARED") {
                return None;
            }
            Some(finish(dialect, rest, lexed.rest, true))
        }
        "END" if dialect != SqlDialect::Mysql => Some(finish(dialect, rest, lexed.rest, true)),
        "ROLLBACK" => {
            let after = skip_noise(rest);
            if matches!(after.first(), Some(&"TO" | &"PREPARED")) {
                return None;
            }
            Some(finish(dialect, rest, lexed.rest, false))
        }
        "ABORT" if dialect == SqlDialect::Postgres => {
            Some(finish(dialect, rest, lexed.rest, false))
        }
        "PREPARE" if dialect == SqlDialect::Postgres && rest.first() == Some(&"TRANSACTION") => {
            Some(TxControl::Refused(PREPARE_TRANSACTION))
        }
        _ => None,
    }
}

/// `WORK` · `TRANSACTION` 같은 군말을 건너뛴다.
fn skip_noise<'a, 'b>(words: &'a [&'b str]) -> &'a [&'b str] {
    match words.first() {
        Some(&"WORK" | &"TRANSACTION") => &words[1..],
        _ => words,
    }
}

/// `BEGIN …`. MariaDB 의 `BEGIN NOT ATOMIC …` 같은 복합 문장은 트랜잭션 제어가 아니다.
fn begin(dialect: SqlDialect, rest: &[&str], leftover: bool) -> Option<TxControl> {
    let rest = match dialect {
        SqlDialect::Sqlite => match rest.first() {
            Some(&"DEFERRED") => &rest[1..],
            Some(&"IMMEDIATE" | &"EXCLUSIVE") => {
                return Some(TxControl::Refused(UNSUPPORTED_OPTION));
            }
            _ => rest,
        },
        _ => rest,
    };
    let rest = skip_noise(rest);
    if dialect == SqlDialect::Mysql && !rest.is_empty() {
        // MySQL 의 `BEGIN` 은 `BEGIN [WORK]` 뿐이다. 그 밖은 복합 문장(MariaDB `BEGIN NOT ATOMIC`)이다.
        return None;
    }
    if rest.first() == Some(&"NOT") {
        return None;
    }
    Some(modes(rest, leftover))
}

/// `START TRANSACTION …`(SQLite 에는 없다).
fn start(dialect: SqlDialect, rest: &[&str], leftover: bool) -> Option<TxControl> {
    if dialect == SqlDialect::Sqlite {
        return None;
    }
    Some(modes(rest, leftover))
}

/// 트랜잭션 특성: `READ ONLY` · `READ WRITE` 만 옮긴다(쉼표는 낱말 사이의 구분으로 읽었다).
fn modes(rest: &[&str], leftover: bool) -> TxControl {
    if leftover {
        return TxControl::Refused(UNSUPPORTED_OPTION);
    }
    match rest {
        [] => TxControl::Begin { read_only: None },
        ["READ", "ONLY"] => TxControl::Begin {
            read_only: Some(true),
        },
        ["READ", "WRITE"] => TxControl::Begin {
            read_only: Some(false),
        },
        _ => TxControl::Refused(UNSUPPORTED_OPTION),
    }
}

/// `COMMIT` · `END` · `ROLLBACK` · `ABORT` 의 꼬리: `[WORK | TRANSACTION] [AND [NO] CHAIN]`. MySQL 의 `[NO] RELEASE`(연결을 끊는다)
/// 는 옮기지 못한다.
fn finish(dialect: SqlDialect, rest: &[&str], leftover: bool, commit: bool) -> TxControl {
    let make = |chain| {
        if commit {
            TxControl::Commit { chain }
        } else {
            TxControl::Rollback { chain }
        }
    };
    if leftover {
        return TxControl::Refused(UNSUPPORTED_OPTION);
    }
    let rest = skip_noise(rest);
    let (chain, rest) = match rest {
        ["AND", "CHAIN", tail @ ..] if dialect != SqlDialect::Sqlite => (true, tail),
        ["AND", "NO", "CHAIN", tail @ ..] if dialect != SqlDialect::Sqlite => (false, tail),
        tail => (false, tail),
    };
    match rest {
        [] => make(chain),
        ["NO", "RELEASE"] if dialect == SqlDialect::Mysql => make(chain),
        _ => TxControl::Refused(UNSUPPORTED_OPTION),
    }
}

/// MySQL · MariaDB 에서 이 문장이 열린 트랜잭션을 실행 전에 커밋하는가. 다른 방언은 늘 false 다.
///
/// 매뉴얼의 목록을 앞 낱말로 고른다: DDL(`ALTER` · `CREATE` · `DROP` · `RENAME` · `TRUNCATE` — `TEMPORARY` 표는 빼고), 사용자 · 권한
/// (`GRANT` · `REVOKE` · `SET PASSWORD`), `LOCK` · `UNLOCK`, 관리(`ANALYZE` · `CHECK` · `OPTIMIZE` · `REPAIR` · `CACHE INDEX` ·
/// `LOAD INDEX` · `FLUSH` · `RESET` — `RESET PERSIST` 는 빼고), `INSTALL` · `UNINSTALL`, 복제 제어(`START` · `STOP` ·
/// `CHANGE REPLICATION|MASTER`), `IMPORT TABLE`, 그리고 `autocommit` 을 바꾸는 `SET`(켜면 커밋한다 — 값은 보지 않고 묻는다).
pub fn implicitly_commits(dialect: SqlDialect, text: &str) -> bool {
    if dialect != SqlDialect::Mysql {
        return false;
    }
    let lexed = lex(dialect, text, 64);
    let words: Vec<&str> = lexed.words.iter().map(String::as_str).collect();
    let Some((first, rest)) = words.split_first() else {
        return false;
    };
    let head = &rest[..rest.len().min(4)];
    match *first {
        "CREATE" | "DROP" => !head.contains(&"TEMPORARY"),
        "ALTER" | "RENAME" | "TRUNCATE" | "GRANT" | "REVOKE" | "LOCK" | "UNLOCK" | "ANALYZE"
        | "OPTIMIZE" | "REPAIR" | "FLUSH" | "INSTALL" | "UNINSTALL" => true,
        "CHECK" => rest.first() == Some(&"TABLE"),
        "CACHE" | "LOAD" => rest.first() == Some(&"INDEX"),
        "RESET" => rest.first() != Some(&"PERSIST"),
        "IMPORT" => rest.first() == Some(&"TABLE"),
        "START" | "STOP" => matches!(
            rest.first(),
            Some(&"REPLICA" | &"SLAVE" | &"GROUP_REPLICATION" | &"ALL")
        ),
        "CHANGE" => matches!(rest.first(), Some(&"REPLICATION" | &"MASTER")),
        "SET" => rest.first() == Some(&"PASSWORD") || words.contains(&"AUTOCOMMIT"),
        _ => false,
    }
}

/// `SET` · `RESET` 으로 시작하는 문장인가(`SET TRANSACTION` · `SET SESSION CHARACTERISTICS` 포함). read-only 연결의 실행 관문이
/// 등급과 상관없이 거절한다 — 읽기 전용 트랜잭션은 첫 문장의 `SET transaction_read_only = off` · `SET TRANSACTION READ WRITE` 로
/// 풀린다(결정 M3-D6).
pub fn changes_session_settings(dialect: SqlDialect, text: &str) -> bool {
    let lexed = lex(dialect, text, 1);
    matches!(
        lexed.words.first().map(String::as_str),
        Some("SET" | "RESET")
    )
}

struct Lexed {
    /// 앞 낱말(ASCII 대문자). 낱말 사이의 쉼표 · `=` · `@` · `.` 은 건너뛴다.
    words: Vec<String>,
    /// 낱말이 아닌 글자(따옴표 · 숫자로 시작하는 값 등)에서 멈췄다.
    rest: bool,
}

/// 앞 낱말을 `limit` 개까지 읽는다(머리 주석의 규칙).
fn lex(dialect: SqlDialect, text: &str, limit: usize) -> Lexed {
    let bytes = text.as_bytes();
    let mysql = dialect == SqlDialect::Mysql;
    let mut i = 0;
    let mut words = Vec::new();
    while i < bytes.len() {
        let b = bytes[i];
        // 공백(PG 처럼 `\v` · `\f` 포함)과 첫 낱말 앞의 빈 문장 `;`.
        if b.is_ascii_whitespace() || b == 0x0b || b == 0x0c || (b == b';' && words.is_empty()) {
            i += 1;
        } else if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                i += 1;
            }
        } else if mysql && b == b'#' {
            while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                i += 1;
            }
        } else if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            if mysql && matches!(bytes.get(i + 2), Some(b'!')) {
                // 실행 주석: 판본 숫자를 지나 안을 코드로 읽는다. 닫는 `*/` 는 아래에서 건너뛴다.
                i += 3;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            } else if mysql && bytes.get(i + 2) == Some(&b'M') && bytes.get(i + 3) == Some(&b'!') {
                i += 4;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                i = skip_block_comment(bytes, i, !mysql);
            }
        } else if mysql && b == b'*' && bytes.get(i + 1) == Some(&b'/') {
            i += 2;
        } else if words.len() < limit && (b.is_ascii_alphabetic() || b == b'_') {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'$'))
            {
                i += 1;
            }
            if i < bytes.len() && bytes[i] >= 0x80 {
                // 한글 등이 붙은 식별자 — 키워드가 아니다.
                return Lexed { words, rest: true };
            }
            words.push(text[start..i].to_ascii_uppercase());
        } else if !words.is_empty() && matches!(b, b',' | b'=' | b'@' | b'.') {
            i += 1;
        } else if words.len() >= limit {
            break;
        } else {
            return Lexed { words, rest: true };
        }
    }
    Lexed { words, rest: false }
}

/// `/* … */` 를 건너뛴다. `nested` 면 PG 처럼 겹친 주석을 센다. 닫히지 않으면 끝까지.
fn skip_block_comment(bytes: &[u8], mut i: usize, nested: bool) -> usize {
    let mut depth = 0_usize;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') && (nested || depth == 0) {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    i
}

#[cfg(test)]
mod tests;
