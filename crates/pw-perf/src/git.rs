//! git 시나리오(`pw-perf --git-repo <저장소>`): 앱이 프로젝트 창에서 돌린 시나리오(`apps/desktop/src/perf-scenario.ts`)의 표시를 읽어
//! 13-m2-plan.md 의 세 완료 기준 — M2-14 의 ⌘9 첫 그림 · 10만 행 스크롤, M2-15 의 1만 줄 diff 첫 그림, M2-13 의 터미널 체크아웃 →
//! 상태바 — 을 회마다 모으고 저장소별로 요약한다. 무엇을 어떻게 재는지는 CONTRIBUTING.md 「성능 기준선」 절의 「git 시나리오」.

use serde::{Deserialize, Serialize};

use crate::marks::Mark;
use crate::report::{Build, Commit, Hardware, Memory, Os, environment};
use crate::stats::{Summary, summarize};

/// 측정하는 쪽이 체크아웃하는 브랜치(`pw_git::testing::synth::PERF_BRANCH`).
pub const DEFAULT_CHECKOUT: &str = "perf-checkout";

/// ⌘9 → 첫 행이 그려진 프레임(06 「성능 목표」, 판정 조건은 결정 M2-D10 — commit-graph 있음 · 없음 각각).
pub const TARGET_OPEN_MS: f64 = 500.0;
/// 10만 행을 끝까지 연속 스크롤할 때 16.7 ms 안인 프레임의 몫(M2-14).
pub const TARGET_SCROLL_SHARE: f64 = 0.95;
/// 1만 줄 바뀐 파일의 diff 첫 그림(M2-15).
pub const TARGET_DIFF_MS: f64 = 200.0;
/// 터미널 `git checkout` → 상태바(M2-13).
pub const TARGET_BRANCH_MS: f64 = 500.0;

/// 결과 파일만 받은 사람도 읽을 수 있게 싣는 정의(정본은 CONTRIBUTING.md 「성능 기준선」 의 「git 시나리오」).
pub const DEFINITIONS: GitDefinitions = GitDefinitions {
    open: "in the webview: from dispatching a ⌘9 keydown on the document (no OS key event) to the animation frame after the Git log canvas first drew a short hash",
    scroll: "after every row arrived (row count unchanged for 1 s): fling = top to bottom in 2000 frames, steady = 2 rows per frame for 600 frames; interval = rAF timestamp deltas, share within 16.7 ms judged on fling",
    diff: "in the webview: from a dblclick on the first log row (the 10,000-changed-line commit) to the frame after the commit tab's diff lines (.pw-diff-line) appeared — includes git.show/git.diff/git.blob and IPC",
    branch: "from the harness's wall clock just before it runs `git checkout <branch>` in the repository to the webview's wall clock at the frame after the status bar branch name changed",
    memory: "phys_footprint of the app instance right after the scenario finished (100k rows loaded, one commit tab open); not an idle baseline",
};

#[derive(Debug, Clone, Serialize)]
pub struct GitDefinitions {
    pub open: &'static str,
    pub scroll: &'static str,
    pub diff: &'static str,
    pub branch: &'static str,
    pub memory: &'static str,
}

/// src/perf-frames.ts 의 `FrameStats`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub within16_7: f64,
    pub over18: f64,
}

/// src/perf-frames.ts 의 `ScrollResult`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Scroll {
    pub interval: FrameStats,
    pub work: FrameStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

/// 한 회에 시나리오가 잰 것.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measure {
    pub open_ms: f64,
    pub first_draw_ms: f64,
    pub first_hash: String,
    pub open_viewport: Viewport,
    pub rows: u64,
    pub all_rows_ms: f64,
    pub scroll_viewport: Viewport,
    pub fling: Scroll,
    pub steady: Scroll,
    pub diff_ms: f64,
    pub diff_path: Option<String>,
    pub painted_lines: u64,
    pub branch_from: Option<String>,
    pub branch_to: String,
    pub branch_ms: f64,
}

/// 시나리오가 어디까지 왔나([`progress`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// 다음 표시를 기다린다.
    Waiting,
    /// 앱이 체크아웃을 기다린다 — 측정하는 쪽이 아직 체크아웃하지 않았다.
    CheckoutReady,
    Done(Box<Measure>),
    Failed(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Open {
    open_ms: f64,
    first_draw_ms: f64,
    first_hash: String,
    viewport: Viewport,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rows {
    rows: u64,
    all_rows_ms: f64,
}

#[derive(Deserialize)]
struct Scrolled {
    viewport: Viewport,
    fling: Scroll,
    steady: Scroll,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffOpen {
    ms: f64,
    path: Option<String>,
    painted_lines: u64,
}

#[derive(Deserialize)]
struct Ready {
    branch: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Shown {
    branch: String,
    epoch_ms: f64,
}

#[derive(Deserialize)]
struct Failure {
    step: String,
    message: String,
}

fn data<T: for<'de> Deserialize<'de>>(
    marks: &[Mark],
    pid: u32,
    event: &str,
) -> Result<Option<T>, String> {
    let Some(mark) = marks.iter().find(|m| m.event == event && m.pid == pid) else {
        return Ok(None);
    };
    let value = mark.data.clone().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value)
        .map(Some)
        .map_err(|e| format!("the app's `{event}` mark is malformed: {e}"))
}

/// `pid` 의 앱이 남긴 시나리오 표시에서 진행을 읽는다. `checked_out_at` 은 측정하는 쪽이 `git checkout` 을 띄우기 직전의 벽시계
/// (아직 하지 않았으면 `None`).
pub fn progress(marks: &[Mark], pid: u32, checked_out_at: Option<f64>) -> Progress {
    match read(marks, pid, checked_out_at) {
        Ok(p) => p,
        Err(e) => Progress::Failed(e),
    }
}

fn read(marks: &[Mark], pid: u32, checked_out_at: Option<f64>) -> Result<Progress, String> {
    if let Some(f) = data::<Failure>(marks, pid, "scenario_error")? {
        return Err(format!(
            "the scenario failed at `{}`: {}",
            f.step, f.message
        ));
    }
    let Some(ready) = data::<Ready>(marks, pid, "checkout_ready")? else {
        return Ok(Progress::Waiting);
    };
    let Some(checked_out_at) = checked_out_at else {
        return Ok(Progress::CheckoutReady);
    };
    if marks
        .iter()
        .all(|m| m.event != "scenario_done" || m.pid != pid)
    {
        return Ok(Progress::Waiting);
    }
    let missing = |event: &str| format!("the scenario finished without a `{event}` mark");
    let open: Open = data(marks, pid, "git_log_open")?.ok_or_else(|| missing("git_log_open"))?;
    let rows: Rows = data(marks, pid, "git_log_rows")?.ok_or_else(|| missing("git_log_rows"))?;
    let scrolled: Scrolled =
        data(marks, pid, "git_log_scroll")?.ok_or_else(|| missing("git_log_scroll"))?;
    let diff: DiffOpen = data(marks, pid, "diff_open")?.ok_or_else(|| missing("diff_open"))?;
    let shown: Shown = data(marks, pid, "branch_shown")?.ok_or_else(|| missing("branch_shown"))?;
    Ok(Progress::Done(Box::new(Measure {
        open_ms: open.open_ms,
        first_draw_ms: open.first_draw_ms,
        first_hash: open.first_hash,
        open_viewport: open.viewport,
        rows: rows.rows,
        all_rows_ms: rows.all_rows_ms,
        scroll_viewport: scrolled.viewport,
        fling: scrolled.fling,
        steady: scrolled.steady,
        diff_ms: diff.ms,
        diff_path: diff.path,
        painted_lines: diff.painted_lines,
        branch_from: ready.branch,
        branch_to: shown.branch,
        branch_ms: shown.epoch_ms - checked_out_at,
    })))
}

/// 저장소 하나의 한 회.
#[derive(Debug, Clone, Serialize)]
pub struct GitRun {
    pub index: usize,
    pub warmup: bool,
    /// 요약에서 뺀 이유(창이 가려졌다, 화면이 잠겼다). `None` 이면 유효한 회다.
    pub excluded: Option<String>,
    pub app_pid: u32,
    /// 메모리를 합친 묶음 키(책임 프로세스).
    pub group: u32,
    /// 시작 → 프로젝트 창 첫 프레임(참고 — 10만 커밋 저장소를 연 콜드 스타트).
    pub cold_start_ms: f64,
    pub measure: Measure,
    /// 시나리오가 끝난 직후의 메모리.
    pub memory: Memory,
    pub killed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitSummary {
    pub open_ms: Option<Summary>,
    pub first_draw_ms: Option<Summary>,
    pub all_rows_ms: Option<Summary>,
    pub fling_within16_7: Option<Summary>,
    pub fling_p95_ms: Option<Summary>,
    pub steady_within16_7: Option<Summary>,
    pub diff_ms: Option<Summary>,
    pub branch_ms: Option<Summary>,
    pub footprint_bytes: Option<Summary>,
}

/// 중앙값이 목표 안인가(값이 없으면 `None`).
#[derive(Debug, Clone, Serialize)]
pub struct GitTargets {
    pub open_ms: f64,
    pub scroll_share: f64,
    pub diff_ms: f64,
    pub branch_ms: f64,
    pub open_met: Option<bool>,
    pub scroll_met: Option<bool>,
    pub diff_met: Option<bool>,
    pub branch_met: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoResult {
    pub path: String,
    /// `.git/objects/info/commit-graph`(또는 `commit-graphs/`)가 있었는가.
    pub commit_graph: bool,
    /// `git rev-list --count --all`.
    pub commits: Option<u64>,
    pub runs: Vec<GitRun>,
    pub valid_runs: usize,
    pub summary: GitSummary,
    pub targets: GitTargets,
}

pub fn summarize_git(runs: &[GitRun]) -> GitSummary {
    let measured: Vec<&GitRun> = runs
        .iter()
        .filter(|r| !r.warmup && r.excluded.is_none())
        .collect();
    let values =
        |f: &dyn Fn(&GitRun) -> f64| summarize(&measured.iter().map(|r| f(r)).collect::<Vec<_>>());
    GitSummary {
        open_ms: values(&|r| r.measure.open_ms),
        first_draw_ms: values(&|r| r.measure.first_draw_ms),
        all_rows_ms: values(&|r| r.measure.all_rows_ms),
        fling_within16_7: values(&|r| r.measure.fling.interval.within16_7),
        fling_p95_ms: values(&|r| r.measure.fling.interval.p95),
        steady_within16_7: values(&|r| r.measure.steady.interval.within16_7),
        diff_ms: values(&|r| r.measure.diff_ms),
        branch_ms: values(&|r| r.measure.branch_ms),
        footprint_bytes: values(&|r| r.memory.footprint_bytes as f64),
    }
}

pub fn git_targets(summary: &GitSummary) -> GitTargets {
    GitTargets {
        open_ms: TARGET_OPEN_MS,
        scroll_share: TARGET_SCROLL_SHARE,
        diff_ms: TARGET_DIFF_MS,
        branch_ms: TARGET_BRANCH_MS,
        open_met: summary.open_ms.map(|s| s.median <= TARGET_OPEN_MS),
        scroll_met: summary
            .fling_within16_7
            .map(|s| s.median >= TARGET_SCROLL_SHARE),
        diff_met: summary.diff_ms.map(|s| s.median <= TARGET_DIFF_MS),
        branch_met: summary.branch_ms.map(|s| s.median <= TARGET_BRANCH_MS),
    }
}

fn met(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "within target",
        Some(false) => "over target",
        None => "no value",
    }
}

fn cell(s: Option<Summary>, unit: &str) -> String {
    s.map_or("—".into(), |s| {
        format!(
            "{:.1}{unit} ({:.1}–{:.1}, n={})",
            s.median, s.min, s.max, s.n
        )
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct GitSettings {
    pub runs: usize,
    pub warmup: usize,
    /// 측정하는 쪽이 체크아웃하는 브랜치.
    pub checkout: String,
}

/// git 시나리오의 `result.json`.
#[derive(Debug, Clone, Serialize)]
pub struct GitReport {
    pub schema: u32,
    pub scenario: &'static str,
    pub definitions: GitDefinitions,
    pub build: Build,
    pub commit: Commit,
    pub os: Os,
    pub hardware: Hardware,
    pub settings: GitSettings,
    pub repos: Vec<RepoResult>,
    /// 유효한 회가 어느 저장소에서든 `--runs` 의 절반보다 적을 때. 있으면 pw-perf 는 3 으로 끝난다.
    pub warning: Option<String>,
}

/// git 시나리오의 `summary.md`.
pub fn git_report_markdown(report: &GitReport) -> String {
    let mut md = String::from("## Pitwall git scenario (M2-13 · M2-14 · M2-15)\n\n");
    if let Some(warning) = &report.warning {
        md.push_str(&format!("> **Warning:** {warning}.\n\n"));
    }
    if report.build.profile != "release" {
        md.push_str(&format!(
            "> **Not a baseline:** measured a `{}` build. The targets are judged on release bundles.\n\n",
            report.build.profile
        ));
    }
    md.push_str(&environment(
        &report.build,
        &report.commit,
        &report.os,
        &report.hardware,
    ));
    md.push_str(&format!(
        "\n- runs per repository: {} measured + {} warm-up · checkout branch `{}` · medians over valid measured runs\n",
        report.settings.runs, report.settings.warmup, report.settings.checkout
    ));
    md.push_str(&git_markdown(&report.repos));
    md
}

/// 저장소별 요약 표(summary.md 의 본문).
pub fn git_markdown(repos: &[RepoResult]) -> String {
    let mut md = String::new();
    for repo in repos {
        let s = &repo.summary;
        let t = &repo.targets;
        md.push_str(&format!(
            "\n### {} (commit-graph: {}, commits: {})\n\nValid runs: {}\n\n| Item | Median (range, n) | Target | Result |\n|---|---|---|---|\n",
            repo.path,
            if repo.commit_graph { "yes" } else { "no" },
            repo.commits.map_or("?".into(), |n| n.to_string()),
            repo.valid_runs
        ));
        let rows: [(&str, String, String, Option<bool>); 7] = [
            (
                "⌘9 → first rows frame (M2-14)",
                cell(s.open_ms, " ms"),
                format!("≤ {TARGET_OPEN_MS} ms"),
                t.open_met,
            ),
            (
                "fling: frames within 16.7 ms (M2-14)",
                cell(s.fling_within16_7, ""),
                format!("≥ {TARGET_SCROLL_SHARE}"),
                t.scroll_met,
            ),
            (
                "fling: frame interval p95",
                cell(s.fling_p95_ms, " ms"),
                "—".into(),
                None,
            ),
            (
                "steady: frames within 16.7 ms",
                cell(s.steady_within16_7, ""),
                "—".into(),
                None,
            ),
            (
                "10k-line diff first paint (M2-15)",
                cell(s.diff_ms, " ms"),
                format!("≤ {TARGET_DIFF_MS} ms"),
                t.diff_met,
            ),
            (
                "git checkout → status bar (M2-13)",
                cell(s.branch_ms, " ms"),
                format!("≤ {TARGET_BRANCH_MS} ms"),
                t.branch_met,
            ),
            (
                "every row received",
                cell(s.all_rows_ms, " ms"),
                "—".into(),
                None,
            ),
        ];
        for (item, value, target, result) in rows {
            let result = if target == "—" { "—" } else { met(result) };
            md.push_str(&format!("| {item} | {value} | {target} | {result} |\n"));
        }
        md.push_str(&format!(
            "| footprint after the scenario | {} | — | — |\n",
            s.footprint_bytes.map_or("—".into(), |f| format!(
                "{:.1} MB ({:.1}–{:.1})",
                f.median / 1e6,
                f.min / 1e6,
                f.max / 1e6
            ))
        ));
        md.push_str("\n| Run | Excluded | Open ms | Fling ≤16.7 | Diff ms | Branch ms | Rows | Viewport |\n|---|---|---|---|---|---|---|---|\n");
        for r in &repo.runs {
            let m = &r.measure;
            md.push_str(&format!(
                "| {}{} | {} | {:.1} | {:.2} | {:.1} | {:.1} | {} | {}×{} |\n",
                r.index,
                if r.warmup { " (warm-up)" } else { "" },
                r.excluded.as_deref().unwrap_or("—"),
                m.open_ms,
                m.fling.interval.within16_7,
                m.diff_ms,
                m.branch_ms,
                m.rows,
                m.scroll_viewport.width,
                m.scroll_viewport.height
            ));
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marks::parse;

    fn stats(within: f64) -> serde_json::Value {
        serde_json::json!({"n": 2000, "p50": 16.6, "p95": 16.9, "p99": 20.0, "max": 33.0, "within16_7": within, "over18": 0.01})
    }

    fn line(event: &str, pid: u32, epoch_ms: f64, data: serde_json::Value) -> String {
        serde_json::json!({"event": event, "pid": pid, "epoch_ms": epoch_ms, "window": "project-a", "hidden": null, "data": data}).to_string() + "\n"
    }

    fn until_ready(pid: u32) -> String {
        let scroll = serde_json::json!({"interval": stats(0.97), "work": stats(1.0)});
        let viewport = serde_json::json!({"width": 1200, "height": 300});
        [
            line("git_log_open", pid, 1.0, serde_json::json!({"openMs": 180.5, "firstDrawMs": 160.0, "firstHash": "abcdef01", "viewport": viewport})),
            line("git_log_rows", pid, 2.0, serde_json::json!({"rows": 100002, "allRowsMs": 2400.0})),
            line("git_log_scroll", pid, 3.0, serde_json::json!({"rows": 100002, "viewport": viewport, "fling": scroll, "steady": scroll})),
            line("diff_open", pid, 4.0, serde_json::json!({"ms": 150.0, "path": "perf/big.rs", "paintedLines": 40})),
            line("checkout_ready", pid, 5.0, serde_json::json!({"branch": "main"})),
        ]
        .concat()
    }

    /// 체크아웃을 기다리는 동안은 끝나지 않고, 측정하는 쪽이 체크아웃한 뒤 `branch_shown` 이 오면 그 차이가 상태바 시간이다.
    #[test]
    fn the_scenario_waits_for_the_checkout_and_measures_the_status_bar_from_it() {
        let pid = 42;
        let early = parse(&line("git_log_open", pid, 1.0, serde_json::json!({})));
        assert_eq!(progress(&early, pid, None), Progress::Waiting);
        let ready = parse(&until_ready(pid));
        assert_eq!(progress(&ready, pid, None), Progress::CheckoutReady);
        assert_eq!(progress(&ready, pid, Some(10_000.0)), Progress::Waiting);
        let done = parse(
            &[
                until_ready(pid),
                line(
                    "branch_shown",
                    pid,
                    6.0,
                    serde_json::json!({"branch": "perf-checkout", "epochMs": 10_120.5}),
                ),
                line("scenario_done", pid, 7.0, serde_json::json!({})),
            ]
            .concat(),
        );
        let Progress::Done(m) = progress(&done, pid, Some(10_000.0)) else {
            panic!("{:?}", progress(&done, pid, Some(10_000.0)));
        };
        assert_eq!(m.branch_ms, 120.5);
        assert_eq!(m.open_ms, 180.5);
        assert_eq!(m.rows, 100_002);
        assert_eq!(m.fling.interval.within16_7, 0.97);
        assert_eq!(m.diff_path.as_deref(), Some("perf/big.rs"));
        assert_eq!(m.branch_from.as_deref(), Some("main"));
        assert_eq!(m.branch_to, "perf-checkout");
        // 다른 프로세스의 표시는 보지 않는다.
        assert_eq!(progress(&done, 7, Some(10_000.0)), Progress::Waiting);
    }

    #[test]
    fn an_error_mark_or_a_malformed_mark_fails_the_run() {
        let pid = 42;
        let failed = parse(&line(
            "scenario_error",
            pid,
            1.0,
            serde_json::json!({"step": "diff open", "message": "timed out waiting for the diff lines"}),
        ));
        assert_eq!(
            progress(&failed, pid, None),
            Progress::Failed(
                "the scenario failed at `diff open`: timed out waiting for the diff lines".into()
            )
        );
        let malformed = parse(&line(
            "checkout_ready",
            pid,
            1.0,
            serde_json::json!({"branch": 3}),
        ));
        let Progress::Failed(why) = progress(&malformed, pid, None) else {
            panic!("a malformed mark passed");
        };
        assert!(why.contains("`checkout_ready` mark is malformed"), "{why}");
    }

    #[test]
    fn targets_judge_the_median_and_the_fling_share() {
        let summary = |open: f64, share: f64, diff: f64, branch: f64| GitSummary {
            open_ms: summarize(&[open]),
            first_draw_ms: None,
            all_rows_ms: None,
            fling_within16_7: summarize(&[share]),
            fling_p95_ms: None,
            steady_within16_7: summarize(&[0.5]),
            diff_ms: summarize(&[diff]),
            branch_ms: summarize(&[branch]),
            footprint_bytes: None,
        };
        let t = git_targets(&summary(500.0, 0.95, 200.0, 500.0));
        assert_eq!(
            (t.open_met, t.scroll_met, t.diff_met, t.branch_met),
            (Some(true), Some(true), Some(true), Some(true))
        );
        let t = git_targets(&summary(500.1, 0.94, 200.1, 500.1));
        assert_eq!(
            (t.open_met, t.scroll_met, t.diff_met, t.branch_met),
            (Some(false), Some(false), Some(false), Some(false))
        );
    }
}
