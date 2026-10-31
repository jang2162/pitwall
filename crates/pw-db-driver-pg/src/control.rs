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

/// 앞 낱말 `n` 개(대문자). 공백 · `--` 주석 · `/* */`(겹침 포함) 주석을 건너뛰고, 낱말이 아닌 글자에서 멈춘다.
fn leading_words(sql: &str, n: usize) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut words = Vec::new();
    while words.len() < n && i < bytes.len() {
        match bytes[i] {
            b if b.is_ascii_whitespace() => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
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
            b if b.is_ascii_alphabetic() || b == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                words.push(sql[start..i].to_ascii_uppercase());
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
            "",
        ] {
            assert_eq!(refused(sql), None, "{sql}");
        }
        assert!(is_rollback_to("ROLLBACK TRANSACTION TO s"));
        assert!(!is_rollback_to("ROLLBACK"));
        assert!(!is_rollback_to("SELECT 1"));
    }
}
