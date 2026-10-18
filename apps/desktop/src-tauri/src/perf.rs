//! 성능 측정 표시(docs/design/06-roadmap.md 「성능 목표 (검증 기준)」, 재는 쪽은 `crates/pw-perf`).
//!
//! [`MARK_ENV`] 에 파일 경로를 주고 띄우면 앱이 두 시각을 그 파일에 JSON 한 줄씩 덧붙인다 — `main` 에 들어온 때와 프로젝트
//! 창이 첫 화면을 그린 때(`first_frame`). `first_frame` 에는 그 전에 창이 가려져 있었는지(`hidden`)도 싣고, 그 뒤에도 창이
//! 가려질 때마다 `hidden` 표시를 하나씩 남긴다 — 가려진 창은 WebKit 이 프레임을 미루고 메모리를 줄여 그 회의 수치가
//! 기준선이 아니다. 변수가 없으면 아무것도 쓰지 않고, 프로젝트 창 주소에 `perf=1` 도 붙지 않아 프론트엔드는 표시 코드를
//! 돌리지 않는다(src/perf-mark.tsx).
//!
//! 측정 모드에서 [`PARENT_ENV`] 에 측정하는 프로세스(하네스)의 pid 를 주면, 앱은 그 pid 가 사라질 때 스스로 끝난다
//! ([`watch_parent`]). 하네스가 SIGKILL 등으로 정리 없이 죽어도 떼어 띄운 앱이 임시 데이터로 남지 않게 한다.
//!
//! 변수를 지우지 않는 이유: CLI 콜드 스타트는 앱을 떼어 다시 띄우는데(detach.rs) 그 자식이 같은 환경을 물려받아야 첫 프레임을
//! 남긴다. 그래서 측정 모드로 띄운 앱이 여는 프로세스도 변수를 물려받는다 — 측정할 때만 켜는 변수다.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, Runtime, State, WebviewWindow};

/// 표시를 덧붙일 파일의 절대 경로.
pub const MARK_ENV: &str = "PITWALL_PERF_MARK";

/// 측정하는 프로세스의 pid. 측정 모드([`MARK_ENV`])에서만 읽는다.
pub const PARENT_ENV: &str = "PITWALL_PERF_PARENT";

/// 측정 모드인 프로젝트 창의 주소에 붙는 쿼리(src/perf-mark.tsx 의 `perfMarkRequested`).
const QUERY: (&str, &str) = ("perf", "1");

/// 표시 파일. `None` 이면 측정 모드가 아니다.
pub struct PerfMarks {
    file: Option<PathBuf>,
    parent: Option<u32>,
}

impl PerfMarks {
    pub fn from_env() -> Self {
        let file = std::env::var_os(MARK_ENV).map(PathBuf::from);
        let parent = file
            .as_ref()
            .and_then(|_| std::env::var(PARENT_ENV).ok()?.parse().ok());
        Self { file, parent }
    }

    #[cfg(test)]
    pub fn at(file: Option<PathBuf>) -> Self {
        Self { file, parent: None }
    }

    /// `event` 와 지금 시각을 덧붙인다. 쓰지 못하면 로그만 남긴다 — 표시 파일이 앱을 막지 않는다.
    pub fn record(&self, event: &str, window: Option<&str>, hidden: Option<bool>) {
        let Some(file) = &self.file else { return };
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let line = mark_line(event, std::process::id(), epoch_ms, window, hidden);
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if let Err(e) = written {
            tracing::warn!(path = %file.display(), %e, "cannot write a perf mark");
        }
    }
}

/// 표시 한 줄(개행 포함). 형식은 `crates/pw-perf` 의 `Mark` 가 읽는다.
fn mark_line(
    event: &str,
    pid: u32,
    epoch_ms: f64,
    window: Option<&str>,
    hidden: Option<bool>,
) -> String {
    let mut line = serde_json::json!({
        "event": event, "pid": pid, "epoch_ms": epoch_ms, "window": window, "hidden": hidden,
    })
    .to_string();
    line.push('\n');
    line
}

/// 프로젝트 창 주소에 더할 쿼리. 측정 모드가 아니면 없다.
pub fn project_query<R: Runtime>(app: &AppHandle<R>) -> Option<(&'static str, &'static str)> {
    app.try_state::<PerfMarks>()
        .is_some_and(|marks| marks.file.is_some())
        .then_some(QUERY)
}

/// 측정하는 프로세스([`PARENT_ENV`])가 사라지면 앱을 정상 종료한다(단일 인스턴스 소켓을 지운다). 주 인스턴스의 setup 에서
/// 부른다 — 떼어 띄우기 전의 프로세스는 오지 않는다. 측정 모드가 아니거나 pid 가 없으면 아무것도 하지 않는다.
pub fn watch_parent<R: Runtime>(app: &AppHandle<R>) {
    let Some(parent) = app.try_state::<PerfMarks>().and_then(|marks| marks.parent) else {
        return;
    };
    let app = app.clone();
    watch(parent, std::time::Duration::from_millis(250), move || {
        tracing::warn!(parent, "the measuring process is gone; quitting");
        app.exit(0);
    });
}

/// `pid` 가 사라지면 `on_gone` 을 한 번 부르는 감시 스레드.
fn watch(pid: u32, every: std::time::Duration, on_gone: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        while alive(pid) {
            std::thread::sleep(every);
        }
        on_gone();
    });
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: 신호 0 은 존재 확인만 한다. 권한이 없어도(EPERM) 살아 있는 것이다.
    let sent = unsafe { libc::kill(pid, 0) } == 0;
    sent || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Windows 는 측정하지 않는다 — 살아 있는 것으로 보아 끝내지 않는다.
#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

/// 프로젝트 창이 첫 화면을 그렸다(src/perf-mark.tsx). `hidden` 은 그 전에 문서가 한 번이라도 가려졌는가. 측정 모드가
/// 아니면 아무것도 하지 않는다.
#[tauri::command]
pub fn perf_first_frame<R: Runtime>(
    window: WebviewWindow<R>,
    marks: State<'_, PerfMarks>,
    hidden: bool,
) {
    marks.record("first_frame", Some(window.label()), Some(hidden));
}

/// 프로젝트 창의 문서가 가려졌다(src/perf-mark.tsx, 첫 프레임 뒤에도 가려질 때마다). 측정 모드가 아니면 아무것도 하지 않는다.
#[tauri::command]
pub fn perf_hidden<R: Runtime>(window: WebviewWindow<R>, marks: State<'_, PerfMarks>) {
    marks.record("hidden", Some(window.label()), Some(true));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_is_one_json_line() {
        let line = mark_line("first_frame", 42, 1.5, Some("project-a"), Some(true));
        assert!(line.ends_with('\n') && !line.trim_end().contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"event": "first_frame", "pid": 42, "epoch_ms": 1.5, "window": "project-a", "hidden": true})
        );
    }

    #[test]
    fn marks_append_to_the_file_only_in_measurement_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("marks.jsonl");
        PerfMarks::at(None).record("main", None, None);
        assert!(!file.exists());
        let marks = PerfMarks::at(Some(file.clone()));
        marks.record("main", None, None);
        marks.record("first_frame", Some("project-a"), Some(false));
        let text = std::fs::read_to_string(&file).unwrap();
        let events: Vec<String> = text
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["event"].to_string())
            .collect();
        assert_eq!(events, ["\"main\"", "\"first_frame\""]);
    }

    /// 측정하는 프로세스가 끝나면 감시가 알린다. 살아 있는 동안은 알리지 않는다.
    #[cfg(unix)]
    #[test]
    fn the_watch_fires_once_the_measuring_process_is_gone() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        watch(
            child.id(),
            std::time::Duration::from_millis(10),
            move || {
                tx.send(()).unwrap();
            },
        );
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "살아 있는 동안 알렸다"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("끝난 뒤 알리지 않았다");
    }

    #[test]
    fn only_measurement_mode_adds_the_perf_query() {
        let app = tauri::test::mock_app();
        assert_eq!(project_query(app.handle()), None, "상태가 없으면 없다");
        app.manage(PerfMarks::at(None));
        assert_eq!(project_query(app.handle()), None);

        let app = tauri::test::mock_app();
        app.manage(PerfMarks::at(Some(PathBuf::from("/tmp/marks.jsonl"))));
        assert_eq!(project_query(app.handle()), Some(("perf", "1")));
    }
}
