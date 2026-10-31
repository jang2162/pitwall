//! 분류 입력 상한(`risk::MAX_STATEMENT_BYTES` · `MAX_OPERATOR_TOKENS`)의 근거를 잰다 — 최악 입력의 분류 시간과, 2 MiB 스택(tokio
//! 작업 스레드 · `spawn_blocking` 의 기본)에서 넘치는지.
//!
//! ```text
//! cargo run --release -p pw-db-sql --example classify_bench -- time            # 상한 근처 최악 입력의 시간 표(중앙값, 5회)
//! cargo run --release -p pw-db-sql --example classify_bench -- stack <꼴> <n> [KiB=2048]   # 그 스택의 스레드에서 한 번 분류(넘치면 abort)
//! cargo run -p pw-db-sql --example classify_bench -- stack <꼴> <n>             # 같은 것을 debug 빌드로(프레임이 커서 먼저 넘친다)
//! ```
//!
//! `<꼴>` 은 [`input`] 의 이름. 넘침은 잡을 수 없으므로 꼴 · 길이마다 프로세스를 따로 띄워 잰다. 시간 표는 벽시계와 함께 그
//! 스레드의 CPU 시간을 적는다 — 다른 일로 붐비는 기기에서는 벽시계가 몇 배로 흔들리므로 판정은 CPU 시간으로 한다.

use std::time::{Duration, Instant};

use pw_db_sql::risk::{MAX_OPERATOR_TOKENS, MAX_STATEMENT_BYTES, classify_statement};
use pw_db_types::SqlDialect;

/// 최악 입력. `n` 은 연산자 토큰 수(사슬) 또는 대략의 바이트 수(평평한 꼴).
fn input(kind: &str, n: usize) -> (SqlDialect, String) {
    let pg = SqlDialect::Postgres;
    let my = SqlDialect::Mysql;
    match kind {
        // 같은 우선순위 연산자의 왼쪽 사슬 — 깊이가 n 인 AST.
        "plus" => (pg, format!("SELECT {}1", "1+".repeat(n))),
        "concat" => (pg, format!("SELECT {}'a'", "'a' || ".repeat(n))),
        // `1=1 AND` 는 연산자 둘.
        "and" => (
            pg,
            format!("DELETE FROM t WHERE {}a=1", "a=1 AND ".repeat(n / 2)),
        ),
        "or" => {
            let terms: Vec<String> = (0..n / 2).map(|i| format!("c = {i}")).collect();
            (pg, format!("DELETE FROM t WHERE {}", terms.join(" OR ")))
        }
        "not_or" => {
            let terms: Vec<String> = (0..n / 2).map(|i| format!("c = {i}")).collect();
            (
                pg,
                format!("DELETE FROM t WHERE NOT ({})", terms.join(" OR ")),
            )
        }
        "is_true" => (pg, format!("DELETE FROM t WHERE a{}", " IS TRUE".repeat(n))),
        // 재귀 한도에 걸리는 꼴(괄호).
        "paren" => (pg, format!("SELECT {}1{}", "(".repeat(n), ")".repeat(n))),
        // 평평한 큰 글 — `n` 바이트 근처.
        "insert" => {
            let row = "(1, 'abcdefgh', -2.5, NULL),";
            let rows = row.repeat(n / row.len());
            (pg, format!("INSERT INTO t VALUES {}(0, '', 0, NULL)", rows))
        }
        "in_list" => {
            let item = "123456,";
            (
                pg,
                format!(
                    "SELECT * FROM t WHERE id IN ({}0)",
                    item.repeat(n / item.len())
                ),
            )
        }
        "statements" => (pg, "SELECT 1;".repeat(n / 9)),
        // MySQL 의 가장 많은 읽기: 백슬래시(읽기 둘) × 판본 열쇠 넷(조합 16) = 32 번 분류.
        "mysql_readings" => {
            let item = "('a\\'b', 1),";
            let rows = item.repeat(n / item.len());
            (
                my,
                format!(
                    "INSERT INTO t VALUES /*!50001 (0, 0), */ /*!50002 (0, 0), */ /*!50003 (0, 0), */ /*!50004 (0, 0), */ {rows}(0, 0)"
                ),
            )
        }
        // 백슬래시만(읽기 둘).
        "mysql_backslash" => {
            let item = "('a\\'b', 1),";
            (
                my,
                format!("INSERT INTO t VALUES {}(0, 0)", item.repeat(n / item.len())),
            )
        }
        "mysql_or" => {
            let terms: Vec<String> = (0..n / 2).map(|i| format!("c = '{i}\\''")).collect();
            (
                my,
                format!(
                    "DELETE FROM t WHERE {} /*!50001 */ /*!50002 */ /*!50003 */ /*!50004 */",
                    terms.join(" OR ")
                ),
            )
        }
        _ => panic!("모르는 꼴: {kind}"),
    }
}

/// 이 스레드가 쓴 CPU 시간.
#[cfg(unix)]
fn thread_cpu_time() -> Duration {
    let mut spec = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `spec` 은 살아 있는 지역 변수이고 clock_gettime 은 그 자리에 쓰기만 한다.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut spec) };
    assert_eq!(status, 0, "clock_gettime");
    Duration::new(spec.tv_sec as u64, spec.tv_nsec as u32)
}

#[cfg(not(unix))]
fn thread_cpu_time() -> Duration {
    Duration::ZERO
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("stack") => {
            let kind = args[2].clone();
            let n: usize = args[3].parse().expect("n");
            let kib: usize = args.get(4).map_or(2048, |kib| kib.parse().expect("KiB"));
            let handle = std::thread::Builder::new()
                .stack_size(kib << 10)
                .spawn(move || {
                    let (dialect, text) = input(&kind, n);
                    let started = Instant::now();
                    let classification = classify_statement(dialect, &text);
                    (text.len(), started.elapsed(), classification)
                })
                .expect("spawn");
            let (bytes, elapsed, classification) = handle.join().expect("join");
            println!(
                "{} {} stack={kib}KiB bytes={bytes} {elapsed:?} {classification:?}",
                args[2], args[3]
            );
        }
        Some("time") => {
            let cap = MAX_OPERATOR_TOKENS;
            let bytes = MAX_STATEMENT_BYTES - 64;
            let cases: &[(&str, usize)] = &[
                ("plus", cap - 10),
                ("concat", cap - 10),
                ("and", cap - 20),
                ("or", cap - 20),
                ("not_or", cap - 20),
                ("is_true", cap - 20),
                ("paren", 49),
                ("insert", bytes),
                ("in_list", bytes),
                ("statements", bytes),
                // MySQL 은 바이트 상한을 읽기 수로 나눠 쓴다 — 예산 바로 안쪽.
                ("mysql_backslash", MAX_STATEMENT_BYTES / 2 - 64),
                ("mysql_readings", MAX_STATEMENT_BYTES / 32 - 200),
                ("mysql_or", 2_000),
            ];
            println!("| 꼴 | 바이트 | 결과 | CPU 중앙값 | CPU 최대 | 벽시계 중앙값 |");
            println!("| --- | --- | --- | --- | --- | --- |");
            for (kind, n) in cases {
                let (dialect, text) = input(kind, *n);
                let mut cpu: Vec<Duration> = Vec::new();
                let mut wall: Vec<Duration> = Vec::new();
                let mut last = None;
                for _ in 0..5 {
                    let (cpu_started, wall_started) = (thread_cpu_time(), Instant::now());
                    last = Some(classify_statement(dialect, &text));
                    wall.push(wall_started.elapsed());
                    cpu.push(thread_cpu_time() - cpu_started);
                }
                cpu.sort();
                wall.sort();
                let last = last.expect("ran");
                let ms = |duration: Duration| duration.as_secs_f64() * 1e3;
                println!(
                    "| {kind} | {} | {:?} {:?} | {:.1} ms | {:.1} ms | {:.1} ms |",
                    text.len(),
                    last.grade,
                    last.reasons,
                    ms(cpu[2]),
                    ms(cpu[4]),
                    ms(wall[2])
                );
            }
        }
        _ => eprintln!("사용법은 이 파일의 머리 주석"),
    }
}
