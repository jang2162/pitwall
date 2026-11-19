//! `pw-perf-flood --rate <바이트/초> --secs <초> --stamp <파일>`: 실행 콘솔 측정(17-m6-plan M6-22 ②)의 출력 많은 실행. 표준 출력에
//! 로그 꼴의 줄(약 100바이트, 레벨 낱말 — `INFO` 가 대부분이고 50줄에 하나 `WARN`, 200줄에 하나 `ERROR` — 콘솔의 레벨 강조가 돈다)을
//! 10 ms 마다 `rate / 100` 바이트씩 `secs` 초 동안 쓰고 끝난다. 쓰기가 막혀(PTY 를 읽는 쪽이 느리다) 늦어지면 따라잡지 않고 다음
//! 틱으로 간다 — 실제로 쓴 양과 늦은 틱 수를 `--stamp` 파일(JSON)에 남긴다. 번들에 들지 않는다(`pw-perf-m6` 이 실행 구성의 명령으로
//! 준다).

use std::io::Write as _;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(10);

fn epoch_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// `n` 번째 줄(줄바꿈 포함 약 100바이트).
fn line(n: u64) -> String {
    let level = if n % 200 == 199 {
        "ERROR"
    } else if n % 50 == 49 {
        "WARN "
    } else {
        "INFO "
    };
    format!(
        "{level} [worker-{:02}] request {n:010} handled in {:>4} ms path=/api/v1/items/{:06} status=200 bytes={:>6}\n",
        n % 16,
        n % 997,
        n % 100_000,
        (n * 37) % 65_536
    )
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let (Some(rate), Some(secs)) = (
        value("--rate").and_then(|v| v.parse::<u64>().ok()),
        value("--secs").and_then(|v| v.parse::<u64>().ok()),
    ) else {
        eprintln!("usage: pw-perf-flood --rate <bytes/s> --secs <s> [--stamp <file>]");
        return std::process::ExitCode::from(2);
    };
    let per_tick = (rate / 100).max(1) as usize;
    let started = Instant::now();
    let start_epoch = epoch_ms();
    let mut out = std::io::stdout().lock();
    let mut n = 0u64;
    let mut pending = String::new();
    let mut written = 0u64;
    let mut late = 0u64;
    let ticks = secs * 100;
    for tick in 0..ticks {
        while pending.len() < per_tick {
            pending.push_str(&line(n));
            n += 1;
        }
        let chunk: String = pending.drain(..per_tick).collect();
        if out.write_all(chunk.as_bytes()).is_err() {
            break;
        }
        written += chunk.len() as u64;
        let _ = out.flush();
        let due = TICK * (tick as u32 + 1);
        match due.checked_sub(started.elapsed()) {
            Some(wait) => std::thread::sleep(wait),
            None => late += 1,
        }
    }
    let _ = out.flush();
    let elapsed = started.elapsed().as_secs_f64();
    if let Some(stamp) = value("--stamp") {
        let _ = std::fs::write(
            stamp,
            format!(
                "{{\"bytes\":{written},\"lines\":{n},\"secs\":{elapsed:.3},\"start_epoch_ms\":{start_epoch:.1},\"end_epoch_ms\":{:.1},\"late_ticks\":{late},\"ticks\":{ticks},\"rate\":{rate}}}\n",
                epoch_ms()
            ),
        );
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_carry_level_words_at_their_rates() {
        let lines: Vec<String> = (0..200).map(line).collect();
        assert_eq!(lines.iter().filter(|l| l.starts_with("ERROR")).count(), 1);
        assert_eq!(lines.iter().filter(|l| l.starts_with("WARN")).count(), 3);
        assert!(
            lines
                .iter()
                .all(|l| l.ends_with('\n') && (90..130).contains(&l.len()))
        );
    }
}
