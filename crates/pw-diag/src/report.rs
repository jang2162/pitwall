//! 「진단 정보 복사」의 텍스트: 버전 · OS · 활성 기능 · 크래시 리포트 설정 · 최근 오류 로그. 전부 [`mask`] 를 거친다.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use regex::Regex;

use crate::logging::{CORE, UI};
use crate::mask::mask;
use crate::rolling::dated_files;

/// 구성요소마다 싣는 최근 경고 · 오류 수.
pub const RECENT_ENTRIES: usize = 10;
/// 항목 하나의 최대 길이(바이트). 패닉 스택이 텍스트 전체를 차지하지 않게 자른다.
const ENTRY_MAX_BYTES: usize = 4000;
/// 파일마다 끝에서부터 읽는 양. 로그 파일이 커도 진단 복사가 느려지지 않게 한다.
const TAIL_BYTES: u64 = 256 * 1024;
/// 구성요소마다 읽는 최근 파일 수. 자정 직후에도 어제의 오류가 보이게 둘을 본다.
const RECENT_FILES: usize = 2;

// tracing-subscriber fmt 의 줄 머리: `2026-10-16T01:02:03.123456Z  WARN target: …`. 이 모양이 아닌 줄은 앞 항목의 이어진
// 줄(패닉 스택 등)이다.
static ENTRY_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s")
        .unwrap()
});

type CrashReports = Arc<dyn Fn() -> bool + Send + Sync>;

pub struct Diagnostics {
    log_dir: PathBuf,
    app_version: String,
    home: Option<PathBuf>,
    features: RwLock<Vec<String>>,
    crash_reports: RwLock<CrashReports>,
}

impl Diagnostics {
    /// `log_dir` 는 [`LogConfig::dir`](crate::LogConfig::dir) 와 같은 디렉터리. 홈 디렉터리는 이 프로세스의 것을 쓴다.
    pub fn new(log_dir: impl Into<PathBuf>, app_version: impl Into<String>) -> Self {
        Self {
            log_dir: log_dir.into(),
            app_version: app_version.into(),
            home: std::env::home_dir(),
            features: RwLock::default(),
            crash_reports: RwLock::new(Arc::new(|| false)),
        }
    }

    /// 가릴 홈 디렉터리를 바꾼다(시험용).
    #[must_use]
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    /// 「활성 기능」 줄에 실을 이름들. 앱이 Host 를 다 꾸민 뒤 넣는다.
    pub fn set_features(&self, features: impl IntoIterator<Item = String>) {
        let mut features: Vec<String> = features.into_iter().collect();
        features.sort();
        features.dedup();
        *self
            .features
            .write()
            .unwrap_or_else(PoisonError::into_inner) = features;
    }

    /// 「crash reports」 줄의 값을 읽는 함수. 진단을 모을 때마다 부른다(설정을 다시 읽으면 따라간다). 넣지 않으면 꺼짐.
    pub fn set_crash_reports(&self, read: impl Fn() -> bool + Send + Sync + 'static) {
        *self
            .crash_reports
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Arc::new(read);
    }

    /// 가린 진단 텍스트.
    pub fn collect(&self) -> String {
        let features = self
            .features
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .join(", ");
        let build = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let read = Arc::clone(
            &self
                .crash_reports
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        );
        let crash = if read() { "on" } else { "off" };
        let mut text = format!(
            "Pitwall diagnostics\n\
             version: {} ({build} build)\n\
             os: {} {} ({})\n\
             crash reports: {crash}\n\
             features: {}\n",
            self.app_version,
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::env::consts::FAMILY,
            if features.is_empty() { "-" } else { &features },
        );
        for component in [CORE, UI] {
            text.push_str(&format!(
                "\nrecent warnings and errors ({component} log):\n"
            ));
            match recent_entries(&self.log_dir, component, RECENT_ENTRIES) {
                Ok(entries) if entries.is_empty() => text.push_str("(none)\n"),
                Ok(entries) => {
                    for entry in entries {
                        // 가린 뒤 자른다. 먼저 자르면 경계에 걸린 토큰이 모양 규칙보다 짧아져 앞부분이 샌다.
                        let entry = truncate(&mask(&entry, self.home.as_deref()), ENTRY_MAX_BYTES);
                        text.push_str(&entry);
                        text.push('\n');
                    }
                }
                Err(e) => text.push_str(&format!("(could not read logs: {e})\n")),
            }
        }
        // 머리 줄(버전 · 기능 · 읽기 오류)도 가린다. 이미 가린 항목에 다시 걸어도 결과가 바뀌지 않는다.
        mask(&text, self.home.as_deref())
    }
}

/// 최근 파일들에서 WARN · ERROR 항목을 시간순으로 최대 `limit` 개. 로그 디렉터리가 없으면 빈 목록.
fn recent_entries(dir: &Path, component: &str, limit: usize) -> io::Result<Vec<String>> {
    let files = match dated_files(dir, component) {
        Ok(files) => files,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let start = files.len().saturating_sub(RECENT_FILES);
    let mut entries = Vec::new();
    for (_, path) in &files[start..] {
        let tail = read_tail(path, TAIL_BYTES)?;
        entries.extend(
            split_entries(&tail)
                .into_iter()
                .filter(|(level, _)| matches!(*level, "WARN" | "ERROR"))
                .map(|(_, entry)| entry.to_owned()),
        );
    }
    let skip = entries.len().saturating_sub(limit);
    Ok(entries.split_off(skip))
}

fn read_tail(path: &Path, max: u64) -> io::Result<String> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let cut = len > max;
    if cut {
        file.seek(SeekFrom::Start(len - max))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // 중간에서 자른 첫 줄은 버린다.
    Ok(match (cut, text.find('\n')) {
        (true, Some(i)) => text[i + 1..].to_owned(),
        _ => text,
    })
}

/// 줄 머리로 항목을 나눈다. 머리 없는 줄은 앞 항목에 붙고, 첫 머리 앞의 줄은 버린다.
fn split_entries(text: &str) -> Vec<(&str, &str)> {
    let mut entries: Vec<(&str, usize, usize)> = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some(c) = ENTRY_HEAD.captures(line) {
            entries.push((
                c.get(1).map_or("", |m| m.as_str()),
                offset,
                offset + line.len(),
            ));
        } else if let Some(last) = entries.last_mut() {
            last.2 = offset + line.len();
        }
        offset += line.len();
    }
    entries
        .into_iter()
        .map(|(level, start, end)| (level, text[start..end].trim_end()))
        .collect()
}

fn truncate(entry: &str, max: usize) -> String {
    if entry.len() <= max {
        return entry.to_owned();
    }
    let mut end = max;
    while !entry.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} …(truncated)", &entry[..end])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_dir::TestDir;

    const HOME: &str = "/Users/alice";

    fn diagnostics(dir: &Path) -> Diagnostics {
        let diag = Diagnostics::new(dir, "1.2.3").with_home(Some(HOME.into()));
        diag.set_features(["log".into(), "host".into(), "diag".into(), "host".into()]);
        diag
    }

    #[test]
    fn header_lists_version_os_features_and_crash_setting() {
        let dir = TestDir::new();
        let text = diagnostics(dir.path()).collect();
        assert!(
            text.starts_with("Pitwall diagnostics\nversion: 1.2.3 ("),
            "{text}"
        );
        assert!(text.contains(&format!(
            "os: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
        assert!(text.contains("crash reports: off\n"), "{text}");
        assert!(text.contains("features: diag, host, log\n"), "{text}");
        assert!(text.contains("(core log):\n(none)\n"), "{text}");
    }

    #[test]
    fn crash_reports_line_follows_the_setting() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = TestDir::new();
        let diag = diagnostics(dir.path());
        let enabled = Arc::new(AtomicBool::new(true));
        let read = Arc::clone(&enabled);
        diag.set_crash_reports(move || read.load(Ordering::SeqCst));
        assert!(diag.collect().contains("crash reports: on\n"));
        enabled.store(false, Ordering::SeqCst);
        assert!(diag.collect().contains("crash reports: off\n"));
    }

    #[test]
    fn recent_errors_are_collected_and_masked() {
        let dir = TestDir::new();
        let token = "ghp_A1b2C3d4E5f6G7h8I9j0K1l2";
        fs::write(
            dir.path().join("core.2026-10-15.log"),
            "2026-10-15T23:59:00.000001Z ERROR pw_core: yesterday failed\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("core.2026-10-16.log"),
            format!(
                "2026-10-16T01:00:00.000001Z  INFO pw_core: started\n\
                 2026-10-16T01:00:01.000001Z  WARN pw_core: cannot read {HOME}/work/acme/.pitwall/project.toml\n\
                 2026-10-16T01:00:02.000001Z ERROR pw_core: push failed for alice with {token}\n\
                 2026-10-16T01:00:03.000001Z ERROR pitwall::panic: panic: boom\n\
                 stack backtrace:\n   0: pw_core::run\n             at {HOME}/work/acme/src/run.rs:4:5\n"
            ),
        )
        .unwrap();
        fs::write(
            dir.path().join("ui.2026-10-16.log"),
            "2026-10-16T01:00:04.000001Z ERROR uncaught TypeError source=\"window.error\"\n",
        )
        .unwrap();
        let text = diagnostics(dir.path()).collect();
        for leaked in [HOME, "alice", "acme", token, "A1b2C3"] {
            assert!(!text.contains(leaked), "`{leaked}` leaked:\n{text}");
        }
        assert!(text.contains("ERROR pw_core: yesterday failed"), "{text}");
        assert!(text.contains("WARN pw_core: cannot read <path>"), "{text}");
        assert!(
            text.contains("push failed for <user> with <secret>"),
            "{text}"
        );
        assert!(
            text.contains(
                "panic: boom\nstack backtrace:\n   0: pw_core::run\n             at <path>"
            ),
            "{text}"
        );
        assert!(!text.contains("started"), "INFO 는 싣지 않는다:\n{text}");
        assert!(
            text.contains("(ui log):\n2026-10-16T01:00:04.000001Z ERROR uncaught TypeError"),
            "{text}"
        );
    }

    #[test]
    fn tokens_on_the_truncation_boundary_do_not_leak() {
        let dir = TestDir::new();
        let ghp = "ghp_A1b2C3d4E5f6G7h8I9j0K1l2";
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJlc2lnbmF0dXJl";
        let head = "2026-10-16T01:00:00.000001Z ERROR pw_core: ";
        let mut lines = String::new();
        // 토큰이 ENTRY_MAX_BYTES 경계 바로 앞에서 시작해 경계를 넘게 한다.
        for token in [ghp, jwt] {
            let pad = "x ".repeat((ENTRY_MAX_BYTES - head.len() - 10) / 2);
            lines.push_str(&format!("{head}{pad}{token} tail\n"));
        }
        fs::write(dir.path().join("core.2026-10-16.log"), lines).unwrap();
        let text = diagnostics(dir.path()).collect();
        for leaked in ["ghp_", "A1b2", "eyJ"] {
            assert!(!text.contains(leaked), "`{leaked}` leaked:\n{text}");
        }
        assert_eq!(text.matches("<secret>").count(), 2, "{text}");
    }

    #[test]
    fn only_the_latest_entries_are_kept() {
        let dir = TestDir::new();
        let lines: String = (0..25)
            .map(|i| format!("2026-10-16T01:00:{i:02}.000001Z ERROR pw_core: e{i}\n"))
            .collect();
        fs::write(dir.path().join("core.2026-10-16.log"), lines).unwrap();
        let entries = recent_entries(dir.path(), CORE, RECENT_ENTRIES).unwrap();
        assert_eq!(entries.len(), RECENT_ENTRIES);
        assert!(entries[0].ends_with("e15"), "{entries:?}");
        assert!(entries[9].ends_with("e24"), "{entries:?}");
    }

    #[test]
    fn long_entries_are_truncated_on_a_char_boundary() {
        let long = "가".repeat(3000);
        let out = truncate(&long, ENTRY_MAX_BYTES);
        assert!(out.len() <= ENTRY_MAX_BYTES + " …(truncated)".len());
        assert!(out.ends_with("…(truncated)"));
    }

    #[test]
    fn missing_log_dir_is_not_an_error() {
        let dir = TestDir::new();
        let entries = recent_entries(&dir.path().join("nope"), CORE, 5).unwrap();
        assert!(entries.is_empty());
    }
}
