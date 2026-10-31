//! 트랜잭션 블록을 열고 닫는 문장 가려내기.
//!
//! `tokio-postgres` 는 서버의 트랜잭션 상태(ReadyForQuery 의 상태 바이트)를 내놓지 않는다. 사람이 콘솔에 친 `BEGIN` · `COMMIT`
//! 을 SQL 로 그대로 보내면 드라이버가 아는 상태와 서버가 갈린다 — 자동 커밋 탭의 포털 트랜잭션이 사람의 트랜잭션을 커밋하는 식으로.
//! 그래서 이 문장들은 `execute` 가 거절하고, 파이프라인(M3-05)이 `begin` · `commit` · `rollback` 메서드로 옮긴다. `PREPARE
//! TRANSACTION`(2단계 커밋)은 옮길 메서드가 없어 거절로 남는다. `SAVEPOINT` · `RELEASE` · `ROLLBACK TO` 는 트랜잭션 블록을
//! 열거나 닫지 않으므로 통과하고, `ROLLBACK TO` 가 성공하면 실패한 트랜잭션이 다시 쓸 수 있게 된다([`is_rollback_to`]).
//! `COMMIT PREPARED` · `ROLLBACK PREPARED` 는 지금 트랜잭션과 상관없어 통과한다.

/// 거절할 문장이면 사람이 읽는 까닭.
pub(crate) fn refused(sql: &str) -> Option<&'static str> {
    let words = leading_words(sql, 3);
    let word = |i: usize| words.get(i).map(String::as_str);
    let ends_or_starts = match word(0)? {
        "BEGIN" | "END" | "ABORT" => true,
        "START" => word(1) == Some("TRANSACTION"),
        "COMMIT" => word(1) != Some("PREPARED"),
        "ROLLBACK" => !is_rollback_to(sql) && word(1) != Some("PREPARED"),
        "PREPARE" if word(1) == Some("TRANSACTION") => {
            return Some(
                "PREPARE TRANSACTION (two-phase commit) is not supported in the console; commit or roll back the transaction instead",
            );
        }
        _ => false,
    };
    ends_or_starts.then_some(
        "transaction control statements are not run as SQL: use the console's transaction mode and its commit / rollback (the session runs BEGIN, COMMIT and ROLLBACK itself)",
    )
}

/// `ROLLBACK [WORK | TRANSACTION] TO [SAVEPOINT] 이름`.
pub(crate) fn is_rollback_to(sql: &str) -> bool {
    let words = leading_words(sql, 3);
    match words.as_slice() {
        [first, second, ..] if first == "ROLLBACK" && second == "TO" => true,
        [first, second, third] if first == "ROLLBACK" => {
            matches!(second.as_str(), "WORK" | "TRANSACTION") && third == "TO"
        }
        _ => false,
    }
}

/// PG 렉서(`scan.l`)의 공백: `[ \t\n\r\f\v]`. `char::is_ascii_whitespace` 는 `\v` 를 빼므로 쓰지 않는다 — 서버가 공백으로 보는 글자를
/// 거르개가 낱말 밖의 글자로 보면 `"\x0bBEGIN"` 이 낱말 0개로 지나가 서버에서 `BEGIN` 이 된다(M3-06 리뷰 H1).
fn is_pg_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c | 0x0b)
}

/// PG 식별자의 첫 글자 `[A-Za-z\200-\377_]` 와 이어지는 글자(`$` · 숫자 더함).
fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_cont(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit() || b == b'$'
}

/// 앞 낱말 `n` 개(ASCII 대문자). PG 렉서와 같게 읽는다: 공백([`is_pg_space`]), `--` 주석은 `\n` **또는 `\r`** 에서 끝나고, `/* */` 는
/// 겹친다. 첫 낱말 앞의 `;` 는 빈 문장이라 건너뛴다(PG 는 `";BEGIN"` 을 `BEGIN` 하나로 읽는다). 식별자는 PG 처럼 높은 바이트 · `$` ·
/// 숫자까지 한 낱말이다 — `BEGIN$x` · `BEGIN` 뒤에 붙은 한글은 키워드가 아니다(서버도 식별자로 읽어 문법 오류다). 그 밖의 글자에서 멈춘다.
fn leading_words(sql: &str, n: usize) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut words = Vec::new();
    while words.len() < n && i < bytes.len() {
        match bytes[i] {
            b if is_pg_space(b) => i += 1,
            b';' if words.is_empty() => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 0_usize;
                while i < bytes.len() {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b if is_ident_start(b) => {
                let start = i;
                while i < bytes.len() && is_ident_cont(bytes[i]) {
                    i += 1;
                }
                // 높은 바이트는 UTF-8 글자 중간에서 끊기지 않는다(이어지는 바이트도 높은 바이트다).
                words.push(String::from_utf8_lossy(&bytes[start..i]).to_ascii_uppercase());
            }
            _ => break,
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_block_statements_are_refused() {
        for sql in [
            "BEGIN",
            "begin isolation level serializable",
            "START TRANSACTION READ ONLY",
            "COMMIT",
            "commit and chain",
            "END",
            "ABORT",
            "ROLLBACK",
            "rollback and chain",
            "ROLLBACK WORK",
            "  -- note\n /* a /* nested */ comment */ COMMIT;",
            // PG 렉서와 같게(M3-06 리뷰 H1): \v · \f 는 공백, `--` 주석은 \r 에서도 끝난다, 앞의 빈 문장 `;`.
            "\x0bBEGIN",
            "\x0cBEGIN",
            "-- note\rBEGIN",
            "-- note\r\nCOMMIT",
            ";BEGIN",
            " ; ;\tCOMMIT",
            "/* a /* b */ c */BEGIN",
            "\x0b-- x\r/* y */\x0cROLLBACK",
        ] {
            assert!(refused(sql).is_some(), "{sql}");
        }
        let prepare = refused("PREPARE TRANSACTION 'x'").unwrap();
        assert!(prepare.contains("two-phase"), "{prepare}");
    }

    #[test]
    fn savepoints_and_other_statements_pass() {
        for sql in [
            "SAVEPOINT a",
            "RELEASE SAVEPOINT a",
            "ROLLBACK TO a",
            "rollback to savepoint a",
            "ROLLBACK WORK TO SAVEPOINT a",
            "COMMIT PREPARED 'x'",
            "ROLLBACK PREPARED 'x'",
            "PREPARE q AS SELECT 1",
            "DO $$ BEGIN PERFORM 1; END $$",
            "SELECT 'BEGIN'",
            "/* BEGIN */ SELECT 1",
            "starter_table",
            // 서버도 키워드가 아니라 식별자로 읽는다(문법 오류) — 거르개가 막지 않아도 서버에서 BEGIN 이 되지 않는다.
            "BEGIN$x",
            "BEGIN한",
            "\u{3000}BEGIN",
            "SELECT 1; BEGIN",
            "",
        ] {
            assert_eq!(refused(sql), None, "{sql}");
        }
        assert!(is_rollback_to("ROLLBACK TRANSACTION TO s"));
        assert!(!is_rollback_to("ROLLBACK"));
        assert!(!is_rollback_to("SELECT 1"));
    }
}
