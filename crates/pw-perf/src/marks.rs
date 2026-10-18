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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_read_line_by_line_and_a_torn_line_is_skipped() {
        let text = concat!(
            r#"{"event":"main","pid":10,"epoch_ms":1.0,"window":null}"#,
            "\n",
            r#"{"event":"main","pid":11,"epoch_ms":2.0,"window":null}"#,
            "\n",
            r#"{"event":"first_frame","pid":11,"epoch_ms":3.5,"window":"project-a"}"#,
            "\n",
            r#"{"event":"first_fr"#,
        );
        let marks = parse(text);
        assert_eq!(marks.len(), 3);
        assert_eq!(first(&marks, "main", None).unwrap().pid, 10);
        assert_eq!(first(&marks, "main", Some(11)).unwrap().epoch_ms, 2.0);
        let frame = first(&marks, "first_frame", None).unwrap();
        assert_eq!(frame.window.as_deref(), Some("project-a"));
        assert!(first(&marks, "first_frame", Some(10)).is_none());
    }
}
