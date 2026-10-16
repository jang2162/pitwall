//! 일 단위로 회전하고 오래된 파일을 지우는 로그 파일.
//!
//! 파일 이름은 `<구성요소>.<YYYY-MM-DD>.log` 이고 날짜는 UTC 다. 줄 머리의 시각(tracing-subscriber 기본 형식)도 UTC 라
//! 파일 경계와 줄 시각이 어긋나지 않는다. 시계를 주입받아 회전 · 보관 정리를 시험할 수 있다.
//!
//! tracing-appender 를 쓰지 않은 이유: 보관 기준이 파일 「개수」(`max_log_files`)이고 시계를 주입할 수 없다.
//! 하루에 파일이 하나도 안 생긴 날이 있으면 개수 기준은 7일보다 오래된 파일을 남긴다.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_subscriber::fmt::MakeWriter;

/// 지금 시각. 시험은 고정 · 이동 가능한 시계를 넣는다.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> SystemTime;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

pub struct RollingFile {
    dir: PathBuf,
    component: String,
    retention: NonZeroU32,
    clock: Arc<dyn Clock>,
    current: Mutex<Option<Current>>,
}

struct Current {
    date: String,
    file: File,
}

impl RollingFile {
    /// 디렉터리를 만들고 오늘 파일을 연다. 열 때와 날짜가 바뀔 때마다 보관 기간이 지난 파일을 지운다.
    ///
    /// `retention_days` 는 오늘을 포함한 날 수다 — 7 이면 오늘과 앞선 6일의 파일을 남긴다.
    pub fn open(
        dir: impl Into<PathBuf>,
        component: impl Into<String>,
        retention_days: NonZeroU32,
        clock: Arc<dyn Clock>,
    ) -> io::Result<Self> {
        let this = Self {
            dir: dir.into(),
            component: component.into(),
            retention: retention_days,
            clock,
            current: Mutex::new(None),
        };
        fs::create_dir_all(&this.dir)?;
        let mut current = this.lock();
        this.roll(&mut current)?;
        drop(current);
        Ok(this)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn component(&self) -> &str {
        &self.component
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Current>> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 오늘 날짜의 파일이 열려 있게 한다. 날짜가 바뀌었으면 새 파일을 열고 오래된 파일을 지운다.
    fn roll(&self, current: &mut Option<Current>) -> io::Result<()> {
        let today = utc_date(self.clock.now());
        if current.as_ref().is_some_and(|c| c.date == today) {
            return Ok(());
        }
        let file = OpenOptions::new().create(true).append(true).open(log_path(
            &self.dir,
            &self.component,
            &today,
        ))?;
        *current = Some(Current {
            date: today.clone(),
            file,
        });
        // 정리가 실패해도(권한 · 다른 프로세스가 연 파일) 로그 쓰기는 계속한다.
        let _ = prune(&self.dir, &self.component, &today, self.retention);
        Ok(())
    }

    fn write_record(&self, buf: &[u8]) -> io::Result<()> {
        let mut current = self.lock();
        self.roll(&mut current)?;
        match current.as_mut() {
            Some(c) => c.file.write_all(buf),
            None => Err(io::Error::other("log file is not open")),
        }
    }
}

impl Write for &RollingFile {
    // tracing-subscriber 의 fmt 는 이벤트 하나를 버퍼에 다 만든 뒤 `write_all` 한 번으로 넘긴다. 여기서 버퍼 전체를
    // 쓰고 전체 길이를 돌려주므로 이벤트 하나가 날짜 경계에서 두 파일로 갈리지 않는다.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_record(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.lock().as_mut() {
            Some(c) => c.file.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for RollingFile {
    type Writer = &'a RollingFile;

    fn make_writer(&'a self) -> Self::Writer {
        self
    }
}

pub(crate) fn log_path(dir: &Path, component: &str, date: &str) -> PathBuf {
    dir.join(format!("{component}.{date}.log"))
}

/// `dir` 안의 `<component>.<날짜>.log` 파일을 날짜 오름차순으로. 이름이 정확히 이 모양인 파일만 본다.
pub(crate) fn dated_files(dir: &Path, component: &str) -> io::Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(date) = parse_log_name(name, component) {
            files.push((date.to_owned(), entry.path()));
        }
    }
    files.sort();
    Ok(files)
}

fn parse_log_name<'a>(name: &'a str, component: &str) -> Option<&'a str> {
    let date = name
        .strip_prefix(component)?
        .strip_prefix('.')?
        .strip_suffix(".log")?;
    let b = date.as_bytes();
    let shape = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    shape.then_some(date)
}

/// 보관 기간이 지난 파일을 지운다. 지운 파일 수를 돌려준다. 이름이 `<component>.<날짜>.log` 가 아닌 파일은 건드리지 않는다.
fn prune(dir: &Path, component: &str, today: &str, retention: NonZeroU32) -> io::Result<usize> {
    let Some(today_days) = days_of(today) else {
        return Ok(0);
    };
    // YYYY-MM-DD 는 사전순이 날짜순이다.
    let oldest_kept = civil_date(today_days - i64::from(retention.get()) + 1);
    let mut removed = 0;
    for (date, path) in dated_files(dir, component)? {
        if date < oldest_kept && fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// UTC 날짜 `YYYY-MM-DD`.
pub(crate) fn utc_date(time: SystemTime) -> String {
    let secs = match time.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    };
    civil_date(secs.div_euclid(86_400))
}

/// 1970-01-01 부터의 날 수 → `YYYY-MM-DD`. Howard Hinnant 의 `civil_from_days`.
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// `YYYY-MM-DD` → 1970-01-01 부터의 날 수. Howard Hinnant 의 `days_from_civil`.
fn days_of(date: &str) -> Option<i64> {
    let year: i64 = date.get(0..4)?.parse().ok()?;
    let month: i64 = date.get(5..7)?.parse().ok()?;
    let day: i64 = date.get(8..10)?.parse().ok()?;
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::test_dir::TestDir;

    /// 시험용 시계. 초 단위 UNIX 시각을 바꿔 가며 쓴다.
    pub(crate) struct FakeClock(AtomicU64);

    impl FakeClock {
        pub(crate) fn at(date: &str) -> Arc<Self> {
            let secs = u64::try_from(days_of(date).unwrap() * 86_400).unwrap();
            Arc::new(Self(AtomicU64::new(secs + 3600)))
        }

        pub(crate) fn advance_days(&self, days: u64) {
            self.0.fetch_add(days * 86_400, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> SystemTime {
            UNIX_EPOCH + Duration::from_secs(self.0.load(Ordering::SeqCst))
        }
    }

    fn seven() -> NonZeroU32 {
        NonZeroU32::new(7).unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn date_math_round_trips() {
        for date in ["1970-01-01", "2000-02-29", "2024-12-31", "2026-03-01"] {
            assert_eq!(civil_date(days_of(date).unwrap()), date);
        }
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(86_400 * 365)),
            "1971-01-01"
        );
    }

    #[test]
    fn writes_go_to_todays_file_and_roll_at_midnight() {
        let dir = TestDir::new();
        let clock = FakeClock::at("2026-10-16");
        let log = RollingFile::open(dir.path(), "core", seven(), clock.clone()).unwrap();
        (&log).write_all(b"first\n").unwrap();
        clock.advance_days(1);
        (&log).write_all(b"second\n").unwrap();
        assert_eq!(
            names(dir.path()),
            ["core.2026-10-16.log", "core.2026-10-17.log"]
        );
        let first = fs::read_to_string(dir.path().join("core.2026-10-16.log")).unwrap();
        let second = fs::read_to_string(dir.path().join("core.2026-10-17.log")).unwrap();
        assert_eq!((first.as_str(), second.as_str()), ("first\n", "second\n"));
    }

    #[test]
    fn reopening_appends_to_the_same_day() {
        let dir = TestDir::new();
        let clock = FakeClock::at("2026-10-16");
        for line in ["a\n", "b\n"] {
            let log = RollingFile::open(dir.path(), "ui", seven(), clock.clone()).unwrap();
            (&log).write_all(line.as_bytes()).unwrap();
        }
        let text = fs::read_to_string(dir.path().join("ui.2026-10-16.log")).unwrap();
        assert_eq!(text, "a\nb\n");
    }

    #[test]
    fn keeps_seven_days_and_removes_older_files_of_the_same_component() {
        let dir = TestDir::new();
        for name in [
            "core.2026-10-08.log", // 8일 전: 지운다
            "core.2026-10-09.log", // 7일 전: 지운다
            "core.2026-10-10.log", // 6일 전: 남긴다(오늘 포함 7일)
            "core.2026-10-15.log",
            "ui.2026-10-01.log", // 다른 구성요소: core 정리가 건드리지 않는다
            "core.backup.log",   // 이름 모양이 다르다: 건드리지 않는다
            // 모양이 다른데 사전순으로는 기준 날짜보다 작은 이름. 모양 검사만이 이것들을 지킨다.
            "core.0-old.log",
            "core.2026-old.log",
            "core.2026-01-1x.log",
            "notes.txt",
        ] {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        let clock = FakeClock::at("2026-10-16");
        let _log = RollingFile::open(dir.path(), "core", seven(), clock).unwrap();
        assert_eq!(
            names(dir.path()),
            [
                "core.0-old.log",
                "core.2026-01-1x.log",
                "core.2026-10-10.log",
                "core.2026-10-15.log",
                "core.2026-10-16.log",
                "core.2026-old.log",
                "core.backup.log",
                "notes.txt",
                "ui.2026-10-01.log",
            ]
        );
    }

    #[test]
    fn rolling_over_prunes_files_that_aged_out_while_running() {
        let dir = TestDir::new();
        let clock = FakeClock::at("2026-10-01");
        let log = RollingFile::open(dir.path(), "core", seven(), clock.clone()).unwrap();
        (&log).write_all(b"x\n").unwrap();
        clock.advance_days(7);
        (&log).write_all(b"y\n").unwrap();
        assert_eq!(names(dir.path()), ["core.2026-10-08.log"]);
    }
}
