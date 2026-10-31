//! 입력 상한(H4): 큰 글 · 긴 연산자 사슬 · 깊은 중첩이 프로세스를 끄거나 붙잡지 않고 파괴로 닫힌다. 시간 표는
//! `examples/classify_bench.rs`(release).

use pw_db_types::RiskGrade::{Destructive, Read, Write};
use pw_db_types::SqlDialect::{self, Mysql, Postgres};

use crate::risk::RiskReason::*;
use crate::risk::{
    Classification, MAX_OPERATOR_TOKENS, MAX_STATEMENT_BYTES, RECURSION_LIMIT, classify_statement,
};

/// 2 MiB 스택(tokio 작업 스레드 · `spawn_blocking` 의 기본)의 스레드에서 분류한다. 넘치면 시험 프로세스가 abort 된다.
fn on_small_stack(dialect: SqlDialect, sql: String) -> Classification {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || classify_statement(dialect, &sql))
        .unwrap()
        .join()
        .unwrap()
}

/// 상한 바로 안쪽의 사슬 꼴(연산자 토큰 수가 `MAX_OPERATOR_TOKENS - 20` 근처).
fn chains() -> Vec<(&'static str, String)> {
    let n = MAX_OPERATOR_TOKENS - 20;
    let or_terms: Vec<String> = (0..n / 2).map(|i| format!("c = {i}")).collect();
    vec![
        ("plus", format!("SELECT {}1", "1+".repeat(n))),
        ("concat", format!("SELECT {}'a'", "'a' || ".repeat(n))),
        (
            "and",
            format!("DELETE FROM t WHERE {}a=1", "a=1 AND ".repeat(n / 2)),
        ),
        (
            "or",
            format!("DELETE FROM t WHERE {}", or_terms.join(" OR ")),
        ),
        (
            "not_or",
            format!("DELETE FROM t WHERE NOT ({})", or_terms.join(" OR ")),
        ),
        (
            "is_true",
            format!("DELETE FROM t WHERE a{}", " IS TRUE".repeat(n / 2)),
        ),
    ]
}

#[test]
fn chains_just_under_the_cap_are_classified_on_a_small_stack() {
    for (name, sql) in chains() {
        let classification = on_small_stack(Postgres, sql);
        assert_ne!(classification.reasons, vec![TooLarge], "{name}");
        assert!(classification.grade <= Write, "{name}: {classification:?}");
    }
}

#[test]
fn long_chains_over_the_cap_are_too_large_without_aborting() {
    for sql in [
        format!("SELECT {}1", "1+".repeat(50_000)),
        format!("DELETE FROM t WHERE {}1=1", "1=1 AND ".repeat(200_000)),
        format!(
            "SELECT * FROM t WHERE {}",
            "a IS NULL OR ".repeat(20_000) + "a = 1"
        ),
    ] {
        let classification = on_small_stack(Postgres, sql);
        assert_eq!(classification.grade, Destructive);
        assert_eq!(classification.reasons, vec![TooLarge]);
    }
    // 값 목록의 쉼표 · 괄호 · 부호는 세지 않는다 — 큰 INSERT 는 연산자 상한에 걸리지 않는다.
    let rows = "(1, -2, 'x', NULL),".repeat(MAX_OPERATOR_TOKENS * 2);
    let classification = classify_statement(
        Postgres,
        &format!("INSERT INTO t VALUES {rows}(0, 0, '', NULL)"),
    );
    assert_eq!(
        (classification.grade, classification.reasons),
        (Write, vec![Dml])
    );
}

#[test]
fn text_over_the_byte_cap_is_too_large() {
    let at_cap = format!(
        "SELECT 1{}",
        " ".repeat(MAX_STATEMENT_BYTES - "SELECT 1".len())
    );
    assert_eq!(classify_statement(Postgres, &at_cap).grade, Read);
    let over = format!("{at_cap} ");
    let classification = classify_statement(Postgres, &over);
    assert_eq!(
        (classification.grade, classification.reasons),
        (Destructive, vec![TooLarge])
    );
    // MySQL 은 읽기 수만큼 나눠 쓴다 — 백슬래시가 있으면 읽기가 둘이라 절반이 상한이다.
    let half = format!("SELECT '\\\\'{}", " ".repeat(MAX_STATEMENT_BYTES / 2));
    let classification = classify_statement(Mysql, &half);
    assert_eq!(
        (classification.grade, classification.reasons),
        (Destructive, vec![TooLarge])
    );
}

#[test]
fn nesting_over_the_recursion_limit_is_too_deep() {
    let under = format!(
        "SELECT {}1{}",
        "(".repeat(RECURSION_LIMIT - 5),
        ")".repeat(RECURSION_LIMIT - 5)
    );
    assert_eq!(classify_statement(Postgres, &under).grade, Read);
    for sql in [
        format!(
            "SELECT {}1{}",
            "(".repeat(RECURSION_LIMIT + 1),
            ")".repeat(RECURSION_LIMIT + 1)
        ),
        format!(
            "SELECT * FROM t WHERE id IN {}SELECT 1{}",
            "(".repeat(200),
            ")".repeat(200)
        ),
        format!(
            "SELECT * FROM {}SELECT 1{} AS x",
            "(".repeat(200),
            ")".repeat(200)
        ),
    ] {
        let classification = on_small_stack(Postgres, sql);
        assert_eq!(
            (classification.grade, classification.reasons),
            (Destructive, vec![TooDeep])
        );
    }
    // 긴 `NOT` 사슬은 파서가 그보다 먼저 거절한다 — 어느 쪽이든 파괴.
    let classification = on_small_stack(Postgres, format!("SELECT {}1", "NOT ".repeat(1_000)));
    assert_eq!(classification.grade, Destructive);
}

/// OR 사슬 술어는 DBX 의 세제곱(release 로 2,000 항 11 s)이 아니다 — CPU 시간으로 본다(벽시계는 붐비는 기기에서 흔들린다).
#[cfg(unix)]
#[test]
fn an_or_chain_predicate_is_not_cubic() {
    use std::time::Duration;

    fn thread_cpu_time() -> Duration {
        let mut spec = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `spec` 은 살아 있는 지역 변수이고 clock_gettime 은 그 자리에 쓰기만 한다.
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut spec) },
            0
        );
        Duration::new(spec.tv_sec as u64, spec.tv_nsec as u32)
    }
    let terms: Vec<String> = (0..2_000).map(|i| format!("c = {i}")).collect();
    let sql = format!("DELETE FROM t WHERE {}", terms.join(" OR "));
    let started = thread_cpu_time();
    let classification = classify_statement(Postgres, &sql);
    let spent = thread_cpu_time() - started;
    assert_eq!(
        (classification.grade, classification.reasons),
        (Write, vec![Dml])
    );
    // debug 빌드에서 수십 ms 다. 세제곱이면 release 로도 10 s 를 넘는다.
    assert!(spent < Duration::from_secs(2), "{spent:?}");
}
