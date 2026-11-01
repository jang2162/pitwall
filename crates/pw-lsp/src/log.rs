//! 서버 로그(15-m4-plan.md M4-01 ⑤): 서버의 stderr 와 — 켜면 — 프로토콜 기록을 `$STATE/logs/lsp/<서버 id>-<인스턴스>.<날짜>.log`
//! 에 쓴다. 회전 · 보관은 앱 로그와 같은 `pw_diag::RollingFile`(하루 단위, 기본 7일 — 03 「로그 · 진단」 의 구성요소별 회전
//! 파일)이다.
//!
//! 줄 머리는 UTC 시각(밀리초)과 종류(`stderr` · `-->` 보냄 · `<--` 받음 · `note`)다. 프로토콜 기록은 본문 전체를 한 줄로 쓴다 —
//! 문서 내용이 실리므로 설정으로 켤 때만 쓴다(기본 끔). 쓰기가 실패해도 서버는 계속 돈다.
//!
//! **상한**(M4-01 리뷰 L4): 줄 하나는 [`MAX_LOG_LINE`] 바이트까지(넘는 부분은 「잘림」 으로 적는다 — 프로토콜 기록의 큰 본문도),
//! 인스턴스 파일 하나에 하루 [`DAILY_LIMIT`] 바이트까지 쓰고 그 뒤로는 그날이 끝날 때까지 「이후 생략」 한 줄만 남기고 버린다.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use pw_diag::logging::DEFAULT_RETENTION_DAYS;
use pw_diag::{RollingFile, SystemClock};

/// 줄 하나의 상한(UI 로그의 줄 상한과 같다).
pub const MAX_LOG_LINE: usize = 8 * 1024;
/// 인스턴스 파일 하나의 하루 상한.
pub const DAILY_LIMIT: usize = 32 * 1024 * 1024;

/// `$STATE/logs/lsp`.
pub fn lsp_log_dir(state: &Path) -> PathBuf {
    state.join("logs").join("lsp")
}

/// 줄의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Stderr,
    Sent,
    Received,
    Note,
}

impl Line {
    fn tag(self) -> &'static str {
        match self {
            Self::Stderr => "stderr",
            Self::Sent => "-->",
            Self::Received => "<--",
            Self::Note => "note",
        }
    }
}

/// 인스턴스 하나의 로그 파일.
pub struct ServerLog {
    file: RollingFile,
    trace: bool,
    daily_limit: usize,
    /// (UTC 날짜, 그날 쓴 바이트, 「이후 생략」 을 적었나).
    today: Mutex<(String, usize, bool)>,
}

impl std::fmt::Debug for ServerLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerLog")
            .field("component", &self.file.component())
            .field("trace", &self.trace)
            .finish()
    }
}

impl ServerLog {
    /// `dir`(보통 [`lsp_log_dir`]) 아래 `<server_id>-<instance>` 구성요소의 회전 파일을 연다. 파일 이름에 쓸 수 없는 글자는 `_`
    /// 로 바꾼다. `trace` 면 주고받은 메시지 본문도 쓴다.
    pub fn open(dir: &Path, server_id: &str, instance: &str, trace: bool) -> io::Result<Arc<Self>> {
        Self::open_with_limit(dir, server_id, instance, trace, DAILY_LIMIT)
    }

    /// [`Self::open`] 에 하루 상한을 준다(시험).
    pub fn open_with_limit(
        dir: &Path,
        server_id: &str,
        instance: &str,
        trace: bool,
        daily_limit: usize,
    ) -> io::Result<Arc<Self>> {
        let component = format!("{}-{}", file_safe(server_id), file_safe(instance));
        let file = RollingFile::open(
            dir,
            component,
            DEFAULT_RETENTION_DAYS,
            Arc::new(SystemClock),
        )?;
        Ok(Arc::new(Self {
            file,
            trace,
            daily_limit,
            today: Mutex::new((String::new(), 0, false)),
        }))
    }

    /// 프로토콜 기록을 쓰는가.
    pub fn traces(&self) -> bool {
        self.trace
    }

    /// 한 줄을 쓴다. 줄바꿈은 걷고(줄 하나 = 기록 하나) [`MAX_LOG_LINE`] 에서 자른다. 그날 상한을 넘으면 버린다.
    pub fn line(&self, kind: Line, text: &str) {
        let text = text.trim_end_matches(['\r', '\n']);
        let now = utc_now();
        let text = if text.len() > MAX_LOG_LINE {
            let mut at = MAX_LOG_LINE;
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            format!("{} … ({} bytes cut)", &text[..at], text.len() - at)
        } else {
            text.to_owned()
        };
        let record = format!("{now} {} {}\n", kind.tag(), text.replace(['\r', '\n'], " "));
        let mut today = self.today.lock().unwrap_or_else(PoisonError::into_inner);
        let date = &now[..10];
        if today.0 != date {
            *today = (date.to_owned(), 0, false);
        }
        if today.1 + record.len() > self.daily_limit {
            if !today.2 {
                today.2 = true;
                let note = format!(
                    "{now} note this log reached its daily limit of {} bytes; further lines today are dropped\n",
                    self.daily_limit
                );
                self.write(note.as_bytes());
            }
            return;
        }
        today.1 += record.len();
        self.write(record.as_bytes());
    }

    fn write(&self, bytes: &[u8]) {
        let mut file = &self.file;
        if let Err(e) = file.write_all(bytes) {
            tracing::debug!(target: "pw_lsp", %e, "cannot write the language server log");
        }
    }

    /// 메시지 본문(켰을 때만).
    pub(crate) fn message(&self, kind: Line, body: &[u8]) {
        if self.trace {
            self.line(kind, &String::from_utf8_lossy(body));
        }
    }
}

fn file_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn utc_now() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since.as_secs();
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(i64::try_from(days).unwrap_or(0));
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        since.subsec_millis()
    )
}

/// 1970-01-01 부터의 날 수 → (년, 월, 일). Howard Hinnant 의 `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_map_to_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_759), (2026, 11, 2));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn names_are_made_file_safe() {
        assert_eq!(file_safe("rust-analyzer"), "rust-analyzer");
        assert_eq!(file_safe("a/b c:d"), "a_b_c_d");
    }

    fn only_file(dir: &Path) -> String {
        let files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        std::fs::read_to_string(&files[0]).unwrap()
    }

    /// 리뷰 L4: 줄은 상한에서 잘리고, 하루 상한을 넘으면 「이후 생략」 한 줄 뒤로 버린다.
    #[test]
    fn lines_are_cut_and_a_day_has_a_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let log = ServerLog::open_with_limit(tmp.path(), "fake", "1", true, 64 * 1024).unwrap();
        log.line(Line::Stderr, &"한".repeat(MAX_LOG_LINE));
        let text = only_file(tmp.path());
        assert!(text.len() < MAX_LOG_LINE + 200, "{}", text.len());
        assert!(
            text.trim_end().ends_with("bytes cut)"),
            "{}",
            &text[text.len() - 60..]
        );
        log.message(Line::Sent, "x".repeat(1024 * 1024).as_bytes());
        for i in 0..10_000 {
            log.line(Line::Stderr, &format!("line {i} {}", "y".repeat(100)));
        }
        let text = only_file(tmp.path());
        assert!(text.len() <= 64 * 1024 + 300, "{}", text.len());
        assert_eq!(text.matches("daily limit").count(), 1);
        assert!(text.trim_end().ends_with("further lines today are dropped"));
    }

    #[test]
    fn lines_land_in_the_rotating_file_and_traces_only_when_on() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = lsp_log_dir(tmp.path());
        let quiet = ServerLog::open(&dir, "fake", "1", false).unwrap();
        quiet.line(Line::Stderr, "hello\n");
        quiet.message(Line::Sent, b"{\"secret\":1}");
        let loud = ServerLog::open(&dir, "fake", "2", true).unwrap();
        loud.message(Line::Received, b"{\"id\":1}");
        let read = |instance: &str| {
            let entry = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| {
                    p.file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .starts_with(&format!("fake-{instance}."))
                })
                .unwrap();
            std::fs::read_to_string(entry).unwrap()
        };
        let one = read("1");
        assert!(one.ends_with(" stderr hello\n"), "{one}");
        assert!(!one.contains("secret"));
        assert!(read("2").contains("<-- {\"id\":1}"));
    }
}
