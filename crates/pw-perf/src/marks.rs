//! 앱이 표시 파일에 남기는 줄(`apps/desktop/src-tauri/src/perf.rs` 의 `mark_line`).

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Mark {
    /// `main`(프로세스가 `main` 에 들어왔다) 또는 `first_frame`(프로젝트 창이 첫 화면을 그렸다).
    pub event: String,
    pub pid: u32,
    /// 앱이 잰 벽시계 시각(Unix epoch, ms). 측정하는 쪽의 시작 시각과 같은 시계다.
    pub epoch_ms: f64,
    #[serde(default)]
    pub window: Option<String>,
    /// `first_frame` 만: 그 전에 창(문서)이 한 번이라도 가려졌는가.
    #[serde(default)]
    pub hidden: Option<bool>,
    /// 시나리오의 단계(`perf_report`)만: 프론트엔드가 잰 값(src/perf-scenario.ts).
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// 읽을 수 없는 줄은 건너뛴다 — 앱이 아직 쓰는 중인 마지막 줄이 반쪽일 수 있다.
pub fn parse(text: &str) -> Vec<Mark> {
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// `event` 의 첫 표시. `pid` 를 주면 그 프로세스의 것만 본다.
pub fn first<'a>(marks: &'a [Mark], event: &str, pid: Option<u32>) -> Option<&'a Mark> {
    marks
        .iter()
        .find(|m| m.event == event && pid.is_none_or(|p| p == m.pid))
}

/// 멈춘 회(첫 프레임 · 시나리오를 기다리다 시간이 다 됨)의 실패 까닭에 붙이는 진단: 마지막 진행 표시와 그 뒤로 흐른 시간, 창의
/// 가려짐(`hidden` · `visible` 표시), 기다리는 동안 화면이 잠겼는가. 가려진 창 · 잠긴 화면에서는 WebKit 이 rAF 를 멈추고
/// 타이머를 늦춰 회가 제품 탓 없이 멈춘다 — 이것으로 둘을 가른다. `now_ms` 는 기다림이 끝난 벽시계(Unix epoch, ms).
pub fn stall_note(marks: &[Mark], now_ms: f64, locked: bool) -> String {
    let visibility = |m: &&Mark| m.event == "hidden" || m.event == "visible";
    let progress = match marks.iter().rev().find(|m| !visibility(m)) {
        Some(m) => format!(
            "last mark `{}` {:.1} s before",
            m.event,
            (now_ms - m.epoch_ms) / 1000.0
        ),
        None => "no mark".to_owned(),
    };
    let hidden = marks.iter().filter(|m| m.event == "hidden").count();
    let visible = marks.iter().filter(|m| m.event == "visible").count();
    let window = match marks.iter().rev().find(visibility) {
        None => "window never hidden".to_owned(),
        Some(last) if last.event == "hidden" => format!(
            "window hidden {hidden}x, visible again {visible}x, hidden at the end (for {:.1} s)",
            (now_ms - last.epoch_ms) / 1000.0
        ),
        Some(_) => {
            format!("window hidden {hidden}x, visible again {visible}x, visible at the end")
        }
    };
    let screen = if locked {
        "screen locked while waiting"
    } else {
        "screen not locked while waiting"
    };
    format!("{progress}; {window}; {screen}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(event: &str, epoch_ms: f64) -> Mark {
        Mark {
            event: event.into(),
            pid: 7,
            epoch_ms,
            window: Some("project-a".into()),
            hidden: None,
            data: None,
        }
    }

    #[test]
    fn a_stall_note_names_the_last_step_and_whether_the_window_stayed_hidden() {
        let run = [
            mark("main", 0.0),
            mark("first_frame", 500.0),
            mark("gutter_mark", 38_000.0),
            mark("hidden", 48_000.0),
            mark("visible", 50_000.0),
            mark("changes_ready", 156_000.0),
            mark("hidden", 245_000.0),
        ];
        assert_eq!(
            stall_note(&run, 420_000.0, false),
            "last mark `changes_ready` 264.0 s before; window hidden 2x, visible again 1x, hidden at the end (for 175.0 s); screen not locked while waiting"
        );
        assert_eq!(
            stall_note(&run[..5], 60_000.0, true),
            "last mark `gutter_mark` 22.0 s before; window hidden 1x, visible again 1x, visible at the end; screen locked while waiting"
        );
        assert_eq!(
            stall_note(&run[..1], 60_000.0, false),
            "last mark `main` 60.0 s before; window never hidden; screen not locked while waiting"
        );
        assert_eq!(
            stall_note(&[], 1.0, false),
            "no mark; window never hidden; screen not locked while waiting"
        );
    }

    #[test]
    fn marks_are_read_line_by_line_and_a_torn_line_is_skipped() {
        let text = concat!(
            r#"{"event":"main","pid":10,"epoch_ms":1.0,"window":null}"#,
            "\n",
            r#"{"event":"main","pid":11,"epoch_ms":2.0,"window":null}"#,
            "\n",
            r#"{"event":"first_frame","pid":11,"epoch_ms":3.5,"window":"project-a","hidden":true}"#,
            "\n",
            r#"{"event":"first_fr"#,
        );
        let marks = parse(text);
        assert_eq!(marks.len(), 3);
        assert_eq!(first(&marks, "main", None).unwrap().pid, 10);
        assert_eq!(first(&marks, "main", Some(11)).unwrap().epoch_ms, 2.0);
        let frame = first(&marks, "first_frame", None).unwrap();
        assert_eq!(frame.window.as_deref(), Some("project-a"));
        assert_eq!(frame.hidden, Some(true));
        assert_eq!(first(&marks, "main", None).unwrap().hidden, None);
        assert!(first(&marks, "first_frame", Some(10)).is_none());
    }
}
