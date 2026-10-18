//! 성능 측정 표시(docs/design/06-roadmap.md 「성능 목표 (검증 기준)」, 재는 쪽은 `crates/pw-perf`).
//!
//! [`MARK_ENV`] 에 파일 경로를 주고 띄우면 앱이 두 시각을 그 파일에 JSON 한 줄씩 덧붙인다 — `main` 에 들어온 때와 프로젝트
//! 창이 첫 화면을 그린 때(`first_frame`). 변수가 없으면 아무것도 쓰지 않고, 프로젝트 창 주소에 `perf=1` 도 붙지 않아
//! 프론트엔드는 표시 코드를 돌리지 않는다(src/perf-mark.ts).
//!
//! 변수를 지우지 않는 이유: CLI 콜드 스타트는 앱을 떼어 다시 띄우는데(detach.rs) 그 자식이 같은 환경을 물려받아야 첫 프레임을
//! 남긴다. 그래서 측정 모드로 띄운 앱이 여는 프로세스도 변수를 물려받는다 — 측정할 때만 켜는 변수다.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, Runtime, State, WebviewWindow};

/// 표시를 덧붙일 파일의 절대 경로.
pub const MARK_ENV: &str = "PITWALL_PERF_MARK";

/// 측정 모드인 프로젝트 창의 주소에 붙는 쿼리(src/perf-mark.ts 의 `perfMarkRequested`).
const QUERY: (&str, &str) = ("perf", "1");

/// 표시 파일. `None` 이면 측정 모드가 아니다.
pub struct PerfMarks {
    file: Option<PathBuf>,
}

impl PerfMarks {
    pub fn from_env() -> Self {
        Self::at(std::env::var_os(MARK_ENV).map(PathBuf::from))
    }

    pub fn at(file: Option<PathBuf>) -> Self {
        Self { file }
    }

    /// `event` 와 지금 시각을 덧붙인다. 쓰지 못하면 로그만 남긴다 — 표시 파일이 앱을 막지 않는다.
    pub fn record(&self, event: &str, window: Option<&str>) {
        let Some(file) = &self.file else { return };
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let line = mark_line(event, std::process::id(), epoch_ms, window);
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
fn mark_line(event: &str, pid: u32, epoch_ms: f64, window: Option<&str>) -> String {
    let mut line =
        serde_json::json!({ "event": event, "pid": pid, "epoch_ms": epoch_ms, "window": window })
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

/// 프로젝트 창이 첫 화면을 그렸다(src/perf-mark.ts). 측정 모드가 아니면 아무것도 하지 않는다.
#[tauri::command]
pub fn perf_first_frame<R: Runtime>(window: WebviewWindow<R>, marks: State<'_, PerfMarks>) {
    marks.record("first_frame", Some(window.label()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_is_one_json_line() {
        let line = mark_line("first_frame", 42, 1.5, Some("project-a"));
        assert!(line.ends_with('\n') && !line.trim_end().contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"event": "first_frame", "pid": 42, "epoch_ms": 1.5, "window": "project-a"})
        );
    }

    #[test]
    fn marks_append_to_the_file_only_in_measurement_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("marks.jsonl");
        PerfMarks::at(None).record("main", None);
        assert!(!file.exists());
        let marks = PerfMarks::at(Some(file.clone()));
        marks.record("main", None);
        marks.record("first_frame", Some("project-a"));
        let text = std::fs::read_to_string(&file).unwrap();
        let events: Vec<String> = text
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["event"].to_string())
            .collect();
        assert_eq!(events, ["\"main\"", "\"first_frame\""]);
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
