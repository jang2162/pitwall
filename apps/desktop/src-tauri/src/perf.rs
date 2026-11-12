//! 성능 측정 표시(docs/design/06-roadmap.md 「성능 목표 (검증 기준)」, 재는 쪽은 `crates/pw-perf`).
//!
//! [`MARK_ENV`] 에 파일 경로를 주고 띄우면 앱이 두 시각을 그 파일에 JSON 한 줄씩 덧붙인다 — `main` 에 들어온 때와 프로젝트
//! 창이 첫 화면을 그린 때(`first_frame`). `first_frame` 에는 그 전에 창이 가려져 있었는지(`hidden`)도 싣고, 그 뒤에도 창이
//! 가려질 때마다 `hidden`, 다시 보일 때마다 `visible` 표시를 하나씩 남긴다 — 가려진 창은 WebKit 이 프레임을 미루고 메모리를 줄여 그 회의 수치가
//! 기준선이 아니다. 변수가 없으면 아무것도 쓰지 않고, 프로젝트 창 주소에 `perf=1` 도 붙지 않아 프론트엔드는 표시 코드를
//! 돌리지 않는다(src/perf-mark.tsx).
//!
//! 측정 모드에서 [`PARENT_ENV`] 에 측정하는 프로세스(하네스)의 pid 를 주면, 앱은 그 pid 가 사라질 때 스스로 끝난다
//! ([`watch_parent`]). 하네스가 SIGKILL 등으로 정리 없이 죽어도 떼어 띄운 앱이 임시 데이터로 남지 않게 한다.
//!
//! 측정 모드에서 [`SCENARIO_ENV`] 에 시나리오 이름([`SCENARIOS`])을 주면 프로젝트 창 주소에 `perfScenario=<이름>` 도 붙고, 프론트엔드가
//! 첫 프레임 뒤에 그 시나리오를 돌려 단계마다 [`perf_report`] 로 표시를 남긴다(src/perf-scenario.ts — 표시의 `data` 에 잰 값).
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

/// 프로젝트 창이 첫 프레임 뒤에 돌릴 시나리오. 측정 모드([`MARK_ENV`])에서만 읽는다.
pub const SCENARIO_ENV: &str = "PITWALL_PERF_SCENARIO";

/// 아는 시나리오(src/perf-scenario.ts 의 `SCENARIOS` — `db` · `db-idle` 은 src/perf-scenario-db.ts, `lsp` 부터는
/// src/perf-scenario-lsp.ts). 모르는 이름은 무시한다 — 창 주소에는 이 목록의 값만 들어간다.
pub const SCENARIOS: &[&str] = &[
    "git",
    "db",
    "db-idle",
    "lsp",
    "lsp-typing",
    "lsp-typing-off",
    "lsp-idle",
    "symbols",
    "classes",
];

/// 시나리오가 남기는 표시 이름(src/perf-scenario.ts · src/perf-scenario-db.ts · src/perf-scenario-lsp.ts 의 `env.report` — 시험이 그 두 파일과 같은지 본다). 하네스가 판정에 쓰는
/// 표시(`first_frame` · `hidden` · `visible` · `main`)는 웹뷰가 남기지 못하고, 모르는 이름도 남지 않는다.
pub const SCENARIO_EVENTS: &[&str] = &[
    "git_log_open",
    "git_log_rows",
    "git_log_scroll",
    "diff_open",
    "checkout_ready",
    "branch_shown",
    "gutter_mark",
    "typing",
    "changes_ready",
    "commit_list_scroll",
    "stage_ready",
    "staged_shown",
    "scenario_done",
    "scenario_error",
    // DB 시나리오(M3-21).
    "grid_open",
    "grid_rows",
    "grid_scroll",
    "grid_stream_scroll",
    "explorer_tables",
    "redis_open",
    "redis_keys",
    "redis_scroll",
    "db_connected",
    // LSP · 심볼 시나리오(M4-19).
    "lsp_ready",
    "completion",
    "diagnostics",
    "definition",
    "problems_loaded",
    "problems_open",
    "problems_scroll",
    "index_ready",
    "symbol_keys",
    "symbol_keys_second",
    "server_ready",
    // 타이핑 키마다의 몫(15-m4-plan §3 「타이핑 지연 LSP 켬 − 끔」).
    "typing_keys",
    // 키 사이 · 유휴 중에 그리기 갱신을 거는 출처(15-m4-plan §3 「켬 창의 프레임 대기」 · 「rust-analyzer 창이 유휴에도 다시 그리는 까닭」).
    "typing_sources",
    "idle_sources",
];

/// 시나리오 표시 하나의 `data` 상한(JSON 바이트). 가장 큰 것(스크롤 통계)이 1 KiB 남짓이다.
pub const SCENARIO_DATA_MAX: usize = 16 * 1024;

/// 측정 모드인 프로젝트 창의 주소에 붙는 쿼리(src/perf-mark.tsx 의 `perfMarkRequested`).
const QUERY: (&str, &str) = ("perf", "1");

/// 시나리오를 알리는 쿼리 이름(src/perf-mark.tsx 의 `perfScenarioRequested`).
const SCENARIO_QUERY: &str = "perfScenario";

/// 표시 파일. `None` 이면 측정 모드가 아니다.
pub struct PerfMarks {
    file: Option<PathBuf>,
    parent: Option<u32>,
    scenario: Option<&'static str>,
}

impl PerfMarks {
    pub fn from_env() -> Self {
        let file = std::env::var_os(MARK_ENV).map(PathBuf::from);
        let parent = file
            .as_ref()
            .and_then(|_| std::env::var(PARENT_ENV).ok()?.parse().ok());
        let scenario = file
            .as_ref()
            .and_then(|_| scenario_named(&std::env::var(SCENARIO_ENV).ok()?));
        Self {
            file,
            parent,
            scenario,
        }
    }

    #[cfg(test)]
    pub fn at(file: Option<PathBuf>) -> Self {
        Self {
            file,
            parent: None,
            scenario: None,
        }
    }

    /// `event` 와 지금 시각을 덧붙인다. 쓰지 못하면 로그만 남긴다 — 표시 파일이 앱을 막지 않는다.
    pub fn record(&self, event: &str, window: Option<&str>, hidden: Option<bool>) {
        self.record_with(event, window, hidden, None);
    }

    /// [`Self::record`] 에 잰 값(`data`)을 싣는다.
    pub fn record_with(
        &self,
        event: &str,
        window: Option<&str>,
        hidden: Option<bool>,
        data: Option<&serde_json::Value>,
    ) {
        let Some(file) = &self.file else { return };
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let line = mark_line(event, std::process::id(), epoch_ms, window, hidden, data);
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

fn scenario_named(name: &str) -> Option<&'static str> {
    SCENARIOS.iter().copied().find(|s| *s == name)
}

/// 표시 한 줄(개행 포함). 형식은 `crates/pw-perf` 의 `Mark` 가 읽는다. `data` 는 있을 때만 싣는다.
fn mark_line(
    event: &str,
    pid: u32,
    epoch_ms: f64,
    window: Option<&str>,
    hidden: Option<bool>,
    data: Option<&serde_json::Value>,
) -> String {
    let mut value = serde_json::json!({
        "event": event, "pid": pid, "epoch_ms": epoch_ms, "window": window, "hidden": hidden,
    });
    if let Some(data) = data {
        value["data"] = data.clone();
    }
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// 프로젝트 창 주소에 더할 쿼리. 측정 모드가 아니면 없다.
pub fn project_query<R: Runtime>(app: &AppHandle<R>) -> Vec<(&'static str, &'static str)> {
    let Some(marks) = app.try_state::<PerfMarks>().filter(|m| m.file.is_some()) else {
        return Vec::new();
    };
    let mut query = vec![QUERY];
    query.extend(marks.scenario.map(|s| (SCENARIO_QUERY, s)));
    query
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

/// 가려졌던 프로젝트 창의 문서가 다시 보인다(src/perf-mark.tsx). 하네스가 멈춘 회의 끝에 창이 가려져 있었는지 가른다. 측정
/// 모드가 아니면 아무것도 하지 않는다.
#[tauri::command]
pub fn perf_visible<R: Runtime>(window: WebviewWindow<R>, marks: State<'_, PerfMarks>) {
    marks.record("visible", Some(window.label()), Some(false));
}

/// 시나리오의 한 단계(src/perf-scenario.ts): 이름(`event`)과 잰 값(`data`). 측정 모드가 아니면 아무것도 하지 않는다.
#[tauri::command]
pub fn perf_report<R: Runtime>(
    window: WebviewWindow<R>,
    marks: State<'_, PerfMarks>,
    event: String,
    data: serde_json::Value,
) {
    if let Some(why) = scenario_report_refused(&event, &data) {
        tracing::warn!(%event, why, "ignoring a perf scenario report");
        return;
    }
    marks.record_with(&event, Some(window.label()), None, Some(&data));
}

/// 남기지 않을 시나리오 표시의 까닭(아는 이름 · 크기 상한 밖).
fn scenario_report_refused(event: &str, data: &serde_json::Value) -> Option<&'static str> {
    if !SCENARIO_EVENTS.contains(&event) {
        return Some("unknown event name");
    }
    if data.to_string().len() > SCENARIO_DATA_MAX {
        return Some("data over the size limit");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시나리오가 부르는 이름이 모두 받아지고, 하네스의 표시 이름 · 모르는 이름 · 큰 data 는 남지 않는다.
    #[test]
    fn only_the_scenario_s_own_reports_are_recorded() {
        let sources = [
            include_str!("../../src/perf-scenario.ts"),
            include_str!("../../src/perf-scenario-db.ts"),
            include_str!("../../src/perf-scenario-lsp.ts"),
        ];
        let mut used: Vec<&str> = sources
            .iter()
            .flat_map(|source| source.split("env.report(\"").skip(1))
            .filter_map(|rest| rest.split('"').next())
            .collect();
        used.sort_unstable();
        used.dedup();
        let mut known = SCENARIO_EVENTS.to_vec();
        known.sort_unstable();
        assert_eq!(used, known);
        let small = serde_json::json!({"ms": 1.5});
        for event in SCENARIO_EVENTS {
            assert_eq!(scenario_report_refused(event, &small), None, "{event}");
        }
        for event in ["first_frame", "hidden", "visible", "main", "scenario_done "] {
            assert_eq!(
                scenario_report_refused(event, &small),
                Some("unknown event name"),
                "{event}"
            );
        }
        let big = serde_json::json!({"text": "x".repeat(SCENARIO_DATA_MAX)});
        assert_eq!(
            scenario_report_refused("typing", &big),
            Some("data over the size limit")
        );
    }

    #[test]
    fn a_mark_is_one_json_line() {
        let line = mark_line("first_frame", 42, 1.5, Some("project-a"), Some(true), None);
        assert!(line.ends_with('\n') && !line.trim_end().contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"event": "first_frame", "pid": 42, "epoch_ms": 1.5, "window": "project-a", "hidden": true})
        );
        let data = serde_json::json!({"openMs": 120.5});
        let line = mark_line(
            "git_log_open",
            42,
            2.0,
            Some("project-a"),
            None,
            Some(&data),
        );
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["data"], data);
        assert_eq!(v["event"], "git_log_open");
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
        assert_eq!(project_query(app.handle()), [], "상태가 없으면 없다");
        app.manage(PerfMarks::at(None));
        assert_eq!(project_query(app.handle()), []);

        let app = tauri::test::mock_app();
        app.manage(PerfMarks::at(Some(PathBuf::from("/tmp/marks.jsonl"))));
        assert_eq!(project_query(app.handle()), [("perf", "1")]);
    }

    /// 시나리오는 측정 모드에서만, 아는 이름만 주소에 붙는다.
    #[test]
    fn a_known_scenario_joins_the_perf_query() {
        assert_eq!(scenario_named("git"), Some("git"));
        assert_eq!(scenario_named("git&x=1"), None);
        assert_eq!(scenario_named(""), None);

        let app = tauri::test::mock_app();
        app.manage(PerfMarks {
            file: Some(PathBuf::from("/tmp/marks.jsonl")),
            parent: None,
            scenario: Some("git"),
        });
        assert_eq!(
            project_query(app.handle()),
            [("perf", "1"), ("perfScenario", "git")]
        );

        let app = tauri::test::mock_app();
        app.manage(PerfMarks {
            file: None,
            parent: None,
            scenario: Some("git"),
        });
        assert_eq!(project_query(app.handle()), []);
    }
}
