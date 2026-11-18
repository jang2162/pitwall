//! 실행 하나의 출력 — 상한 있는 링 버퍼와 줄 색인(17-m6-plan M6-02 ③ — 메인 판정 M6-P3).
//!
//! 오프셋은 실행이 시작된 뒤 쓴 바이트 수다(버린 것 포함). 앞을 버리면 [`RunLog::start`] 가 버린 바이트 수만큼 간다 — 그래서
//! `run.attach { since }` · `run_logs` 의 `since` 는 버려진 앞을 「잃은 바이트」 로 알 수 있다.
//!
//! 줄 색인은 남은 범위 안의 줄 시작 오프셋이다(`\n` 다음 바이트). `run_logs` 의 tail · since · grep 이 버퍼를 처음부터 다시 훑지
//! 않고 줄 경계를 찾는다. 줄이 아주 길어도 색인은 `\n` 수만큼만 자란다.
//!
//! **상한과 계량은 줄 색인까지 센다**([`RunLog::memory`] — 출력 바이트 + 색인 한 칸에 8 바이트). 빈 줄만 쓰는 프로그램이면 색인이
//! 출력보다 8배 크다 — 그것도 실행 하나의 상한(8 MiB)과 Host 전체의 계량기([`Meter`], 64 MiB) 안이다. 실행 관리자가 Host 총량을
//! 넘으면 가장 오래 끝난 실행부터 버린다.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Host 전체 로그 바이트의 계량기. 복제해도 같은 값이다.
#[derive(Debug, Clone, Default)]
pub struct Meter(Arc<AtomicUsize>);

impl Meter {
    pub fn bytes(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }

    fn add(&self, n: usize) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    fn sub(&self, n: usize) {
        self.0.fetch_sub(n, Ordering::Relaxed);
    }
}

/// 읽은 조각([`RunLog::read`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSlice {
    /// `bytes` 의 첫 바이트의 오프셋. 요청한 `since` 보다 크면 그 사이는 버려졌다.
    pub from: u64,
    pub bytes: Vec<u8>,
}

impl LogSlice {
    /// 이 조각 바로 뒤의 오프셋.
    pub fn end(&self) -> u64 {
        self.from + self.bytes.len() as u64
    }
}

/// 줄 색인 한 칸이 차지하는 바이트([`RunLog::memory`]).
pub const LINE_ENTRY: usize = size_of::<u64>();

/// 실행 하나의 링 버퍼.
#[derive(Debug)]
pub struct RunLog {
    buf: VecDeque<u8>,
    /// `buf[0]` 의 오프셋 — 지금까지 버린 바이트 수.
    start: u64,
    /// 남은 범위 안의 줄 시작 오프셋(오름차순). 첫 줄의 시작이 버려졌으면 첫 원소는 버려진 뒤의 첫 줄 시작이다.
    lines: VecDeque<u64>,
    cap: usize,
    meter: Meter,
}

impl RunLog {
    /// 상한 `cap` 바이트의 빈 버퍼.
    pub fn new(cap: usize, meter: Meter) -> Self {
        meter.add(LINE_ENTRY);
        Self {
            buf: VecDeque::new(),
            start: 0,
            lines: VecDeque::from([0]),
            cap,
            meter,
        }
    }

    /// 남은 첫 바이트의 오프셋(버린 바이트 수).
    pub fn start(&self) -> u64 {
        self.start
    }

    /// 다음에 쓸 바이트의 오프셋(쓴 바이트 수 전체).
    pub fn end(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    /// 남은 바이트 수.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// 상한 · 계량기가 세는 크기: 남은 출력 바이트 + 줄 색인([`LINE_ENTRY`] × 칸 수).
    pub fn memory(&self) -> usize {
        self.buf.len() + LINE_ENTRY * self.lines.len()
    }

    /// 뒤에 붙이고, 상한을 넘으면 앞을 버린다.
    pub fn append(&mut self, bytes: &[u8]) {
        let mut offset = self.end();
        let mut new_lines = 0;
        for &b in bytes {
            offset += 1;
            if b == b'\n' {
                self.lines.push_back(offset);
                new_lines += 1;
            }
        }
        self.buf.extend(bytes);
        self.meter.add(bytes.len() + LINE_ENTRY * new_lines);
        self.trim_to(self.cap);
    }

    /// 상한을 `cap` 으로 바꾸고 넘는 앞을 버린다(끝난 실행을 1 MiB 로 줄인다).
    pub fn shrink(&mut self, cap: usize) {
        self.cap = cap;
        self.trim_to(cap);
        self.buf.shrink_to_fit();
        self.lines.shrink_to_fit();
    }

    /// 모두 버린다(Host 총량을 넘어 실행을 목록에서 뺄 때). 오프셋은 그대로 간다.
    pub fn clear(&mut self) {
        self.trim_to(0);
        self.cap = 0;
        self.buf.shrink_to_fit();
        self.lines.shrink_to_fit();
    }

    /// [`Self::memory`] 가 `cap` 안에 들 때까지 앞을 버린다 — 버리는 바이트는 가장 적게(앞을 버리면 그 안의 색인도 빠진다).
    fn trim_to(&mut self, cap: usize) {
        let before = self.memory();
        if before <= cap {
            return;
        }
        let len = self.buf.len();
        let fits = |drop: usize| {
            let first = self.start + drop as u64;
            let lines = self.lines.len() - self.lines.partition_point(|&l| l < first);
            len - drop + LINE_ENTRY * lines <= cap
        };
        // `fits` 는 버릴수록 참이 된다(단조) — 참이 되는 가장 작은 값을 찾는다.
        let drop = if fits(len) {
            let (mut lo, mut hi) = (0, len);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if fits(mid) {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            lo
        } else {
            len
        };
        self.buf.drain(..drop);
        self.start += drop as u64;
        while self.lines.front().is_some_and(|&l| l < self.start) {
            self.lines.pop_front();
        }
        if self.memory() > cap {
            // 다 버려도 끝의 줄 시작 하나가 남는 경우(상한 0 — 목록에서 뺄 때).
            self.lines.clear();
        }
        self.meter.sub(before - self.memory());
    }

    /// `since` 부터 최대 `max` 바이트. `since` 가 버려진 앞이면 남은 처음부터다(`from` 이 `since` 보다 크다). 끝보다 뒤면 빈
    /// 조각(`from` 은 끝).
    pub fn read(&self, since: u64, max: usize) -> LogSlice {
        let from = since.clamp(self.start, self.end());
        let skip = usize::try_from(from - self.start).unwrap_or(usize::MAX);
        let bytes = self.buf.iter().skip(skip).take(max).copied().collect();
        LogSlice { from, bytes }
    }

    /// 마지막 `n` 줄의 시작 오프셋. 마지막 줄이 `\n` 으로 끝나지 않았어도 한 줄로 센다. 남은 줄이 `n` 보다 적으면 남은 처음
    /// (앞이 버려져 첫 줄이 잘렸으면 잘린 그 줄의 남은 처음).
    pub fn tail_lines(&self, n: usize) -> u64 {
        if n == 0 {
            return self.end();
        }
        // 앞이 잘린 첫 줄은 색인에 시작이 없다. 끝이 줄 시작이면(마지막 바이트가 `\n`) 그 시작은 아직 줄이 아니다.
        let partial = usize::from(self.lines.front() != Some(&self.start));
        let trailing = usize::from(!self.is_empty() && self.lines.back() == Some(&self.end()));
        let count = partial + self.lines.len() - trailing;
        if n >= count {
            return self.start;
        }
        self.lines[count - n - partial]
    }

    /// `[from, to)` 안의 줄 시작 오프셋들.
    pub fn line_starts(&self, from: u64, to: u64) -> impl Iterator<Item = u64> + '_ {
        let first = self.lines.partition_point(|&l| l < from);
        self.lines
            .range(first..)
            .copied()
            .take_while(move |&l| l < to)
    }
}

impl Drop for RunLog {
    fn drop(&mut self) {
        self.meter.sub(self.memory());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1024 * 1024;

    /// 메인 판정 M6-P3: 8 MiB 링에 9 MiB 를 쓰면 앞 1 MiB 가 사라지고, 오프셋이 그만큼 간다 — `since: 0` 은 남은 처음(1 MiB)부터다.
    /// 줄바꿈이 없는 출력이라 색인은 버려진 첫 줄 시작뿐이다.
    #[test]
    fn writing_nine_mib_into_an_eight_mib_ring_drops_the_first_mib() {
        let meter = Meter::default();
        let mut log = RunLog::new(8 * MIB, meter.clone());
        let chunk: Vec<u8> = (0..64 * 1024).map(|i| b'a' + (i % 26) as u8).collect();
        for _ in 0..(9 * MIB / chunk.len()) {
            log.append(&chunk);
        }
        assert_eq!(log.end(), (9 * MIB) as u64);
        assert_eq!(log.start(), MIB as u64);
        assert_eq!(log.len(), 8 * MIB);
        assert_eq!(meter.bytes(), 8 * MIB);
        let slice = log.read(0, 16);
        assert_eq!(slice.from, MIB as u64);
        // 오프셋 1 MiB 의 바이트는 그 오프셋으로 만든 값이다(앞을 버렸어도 오프셋과 내용이 맞는다).
        assert_eq!(slice.bytes[0], b'a' + (MIB % chunk.len() % 26) as u8);
        log.shrink(MIB);
        assert_eq!(log.start(), (8 * MIB) as u64);
        assert_eq!(meter.bytes(), MIB);
        drop(log);
        assert_eq!(meter.bytes(), 0);
    }

    #[test]
    fn the_line_index_follows_the_retained_range() {
        // 13 바이트 + 색인 셋(24) = 37 > 28 — `o` 하나를 버리면 첫 줄 시작(0)도 색인에서 빠져 12 + 16 = 28 이다. 잘린 첫 줄(`ne`)은
        // 남은 처음에서 시작한다.
        let mut log = RunLog::new(28, Meter::default());
        log.append(b"one\ntwo\nthree");
        assert_eq!(log.start(), 1);
        assert_eq!(log.memory(), 28);
        assert_eq!(log.tail_lines(1), 8);
        assert_eq!(log.tail_lines(2), 4);
        assert_eq!(log.tail_lines(3), 1);
        assert_eq!(log.tail_lines(9), 1);
        // 줄 시작이 하나 늘어 다시 넘는다 — 가장 적게 버린다: 넷(`ne\nt`)을 버리면 9 + 16 = 25.
        log.append(b"\n");
        assert_eq!(log.start(), 5);
        assert_eq!(
            log.tail_lines(1),
            8,
            "a trailing newline does not open a line"
        );
        assert_eq!(log.tail_lines(2), 5);
        assert_eq!(log.line_starts(0, 100).collect::<Vec<_>>(), [8, 14]);
        assert_eq!(log.line_starts(5, 14).collect::<Vec<_>>(), [8]);
        assert_eq!(log.read(8, 100).bytes, b"three\n");
        assert_eq!(log.read(99, 100).from, log.end());
    }

    /// 리뷰 F2: 빈 줄만 쓰면 색인이 출력의 8배다 — 그것도 상한 · 계량기 안이다(색인을 세지 않으면 16 MiB 의 `\n` 에 색인만
    /// 64 MiB 가 쌓여 실패한다).
    #[test]
    fn the_line_index_counts_against_the_cap_and_the_meter() {
        let meter = Meter::default();
        let mut log = RunLog::new(8 * MIB, meter.clone());
        let newlines = vec![b'\n'; MIB];
        for _ in 0..16 {
            log.append(&newlines);
        }
        assert!(log.memory() <= 8 * MIB, "{}", log.memory());
        assert_eq!(meter.bytes(), log.memory());
        let indexed = log.line_starts(0, u64::MAX).count();
        assert!(log.len() + LINE_ENTRY * indexed <= 8 * MIB);
        // 색인이 남은 출력 그대로를 가리킨다 — 마지막 줄들은 여전히 찾을 수 있다.
        assert_eq!(log.tail_lines(1), log.end() - 1);
        log.shrink(MIB);
        assert!(log.memory() <= MIB);
        assert_eq!(meter.bytes(), log.memory());
        log.clear();
        assert_eq!((log.memory(), meter.bytes()), (0, 0));
    }

    #[test]
    fn clearing_keeps_the_offsets_moving() {
        let meter = Meter::default();
        let mut log = RunLog::new(100, meter.clone());
        log.append(b"abc\n");
        log.clear();
        assert_eq!((log.start(), log.end(), meter.bytes()), (4, 4, 0));
        assert_eq!(
            log.read(0, 10),
            LogSlice {
                from: 4,
                bytes: Vec::new()
            }
        );
    }
}
