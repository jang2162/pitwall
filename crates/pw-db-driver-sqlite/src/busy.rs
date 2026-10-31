//! 잠금 기다림(busy 핸들러). [`BUSY_TIMEOUT`] 을 **벽시계**로 잰다.
//!
//! SQLite 의 기본 핸들러(`sqlite3_busy_timeout`)는 기다린 시간을 시계로 재지 않는다 — 자기가 청한 잠(1 · 2 · 5 … 100 ms)의 길이를
//! 더해 그 합이 제한에 닿으면 그만둔다. 잠이 늦게 깨는 기계(타이머를 묶는 macOS — GitHub 의 macOS 러너, 백그라운드 QoS 프로세스)
//! 에서는 명목 5 s 를 채우는 잠 59 번이 실제로 10 s 를 넘는다. 사람은 5 s 의 두 배를 기다리고, 닫기가 스레드를 기다리는 시간
//! (`CLOSE_WAIT` 10 s — 기다리는 중의 잠금은 인터럽트로 깨지 않는다)도 넘는다. 그래서 같은 간격으로 자되, 기다림이 시작한 때부터의
//! 실제 시간으로 그만둔다.
//!
//! 상태는 스레드에 둔다. rusqlite 의 핸들러는 상태 없는 함수이고, SQLite 는 문장을 돌리는 스레드에서 핸들러를 부른다 — 한 스레드는
//! 한 번에 한 잠금만 기다린다. SQLite 는 기다림의 호출 수 `count` 를 문장마다 0 부터 센다. 그런데 준비(prepare) 단계의 기다림은
//! 앞 문장이 남긴 수에서 시작할 수 있어서, 0 이 아니어도 바로 앞 호출의 다음 수가 아니거나 앞 호출이 끝난 지 오래면 새 기다림으로 본다.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crate::guard::BUSY_TIMEOUT;

/// 잠 간격(ms) — SQLite 기본 핸들러와 같다. 마지막 값을 되풀이한다.
const NAPS_MS: [u64; 12] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100];

/// 앞 호출이 깬 뒤 이만큼 지나서 온 호출은 새 기다림이다. 같은 기다림의 다음 호출은 잠금을 한 번 다시 본 뒤 곧바로 온다.
const SAME_WAIT_GAP: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
struct Wait {
    started: Instant,
    count: i32,
    woke: Instant,
}

thread_local! {
    static WAIT: Cell<Option<Wait>> = const { Cell::new(None) };
}

/// `sqlite3_busy_handler` 에 거는 함수. 참이면 잔 뒤 잠금을 다시 본다, 거짓이면 그 문장이 `SQLITE_BUSY` 로 끝난다.
pub(crate) fn wait(count: i32) -> bool {
    let (wait, nap) = next(WAIT.get(), count, Instant::now(), BUSY_TIMEOUT);
    let Some(nap) = nap else {
        WAIT.set(None);
        return false;
    };
    std::thread::sleep(nap);
    WAIT.set(Some(Wait {
        woke: Instant::now(),
        ..wait
    }));
    true
}

/// 이번 호출의 기다림과 잘 시간(`None` 이면 그만둔다). 시계를 인자로 받아, 늦게 깨는 시계로 시험한다.
fn next(
    previous: Option<Wait>,
    count: i32,
    now: Instant,
    timeout: Duration,
) -> (Wait, Option<Duration>) {
    let started = previous
        .filter(|w| {
            w.count.checked_add(1) == Some(count)
                && now.saturating_duration_since(w.woke) < SAME_WAIT_GAP
        })
        .map_or(now, |w| w.started);
    let wait = Wait {
        started,
        count,
        woke: now,
    };
    let left = timeout.saturating_sub(now.saturating_duration_since(started));
    if left.is_zero() {
        return (wait, None);
    }
    let step = NAPS_MS[usize::try_from(count).unwrap_or(0).min(NAPS_MS.len() - 1)];
    (wait, Some(Duration::from_millis(step).min(left)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

    /// 한 기다림을 끝까지 돌린다. `late` 는 잠 하나가 늦게 깨는 정도(잠 길이 → 실제로 걸린 길이). (실제로 걸린 시간, 청한 잠의 합).
    fn run(late: impl Fn(Duration) -> Duration) -> (Duration, Duration) {
        let start = Instant::now();
        let (mut now, mut previous, mut asked) = (start, None, Duration::ZERO);
        for count in 0.. {
            let (wait, nap) = next(previous, count, now, TIMEOUT);
            let Some(nap) = nap else { break };
            asked += nap;
            now += late(nap);
            previous = Some(Wait { woke: now, ..wait });
        }
        (now - start, asked)
    }

    /// CI 의 macOS 러너에서 `busy` 가 10 s 에 온 까닭: 잠마다 두 배로 늦게 깨도 그만두는 때는 실제 시간 5 s 다(늦게 깬 마지막 잠
    /// 하나만큼 넘는다). 청한 잠의 합은 5 s 의 반 남짓이라, 그 합으로 재면(SQLite 기본 핸들러) 그만큼 더 기다린다.
    #[test]
    fn a_clock_that_wakes_late_still_gives_up_at_the_timeout() {
        let (waited, asked) = run(|nap| nap * 2 + Duration::from_millis(1));
        assert!(
            (TIMEOUT..TIMEOUT + Duration::from_millis(250)).contains(&waited),
            "gave up after {waited:?}"
        );
        assert!(asked < Duration::from_secs(3), "asked for {asked:?}");

        let (waited, asked) = run(|nap| nap);
        assert_eq!((waited, asked), (TIMEOUT, TIMEOUT));
    }

    /// 잠 간격은 SQLite 기본 핸들러와 같고, 남은 시간보다 길게 자지 않는다.
    #[test]
    fn naps_follow_sqlite_s_schedule_and_stop_at_the_timeout() {
        let now = Instant::now();
        let naps: Vec<u64> = (0..14)
            .map(|count| {
                let nap = next(None, count, now, TIMEOUT).1.unwrap();
                u64::try_from(nap.as_millis()).unwrap()
            })
            .collect();
        assert_eq!(
            naps,
            [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100, 100, 100]
        );

        let started = now;
        let previous = Wait {
            started,
            count: 40,
            woke: now + Duration::from_millis(4_970),
        };
        let at = previous.woke;
        assert_eq!(
            next(Some(previous), 41, at, TIMEOUT).1,
            Some(Duration::from_millis(30))
        );
        let previous = Wait {
            woke: now + TIMEOUT,
            ..previous
        };
        assert_eq!(next(Some(previous), 41, now + TIMEOUT, TIMEOUT).1, None);
    }

    /// 같은 기다림은 바로 앞 호출의 다음 수로 곧바로 온 호출뿐이다. 0 부터 다시 셌거나, 수가 건너뛰었거나, 앞 호출이 깬 지 오래면
    /// 새 기다림이라 5 s 를 처음부터 잰다.
    #[test]
    fn only_the_next_call_right_after_a_nap_continues_a_wait() {
        let start = Instant::now();
        let previous = Wait {
            started: start,
            count: 7,
            woke: start + Duration::from_secs(4),
        };
        let soon = previous.woke + Duration::from_millis(5);
        assert_eq!(next(Some(previous), 8, soon, TIMEOUT).0.started, start);
        for (count, at) in [(0, soon), (9, soon), (8, previous.woke + SAME_WAIT_GAP)] {
            let (wait, nap) = next(Some(previous), count, at, TIMEOUT);
            assert_eq!(wait.started, at, "count {count}");
            assert!(nap.is_some());
        }
    }
}
