//! git 시나리오(`pw-perf --git-repo <저장소>`): 앱이 프로젝트 창에서 돌린 시나리오(`apps/desktop/src/perf-scenario.ts`)의 표시를 읽어
//! 13-m2-plan.md 의 완료 기준 중 release 번들에서 재는 것 — M2-14 의 ⌘9 첫 그림 · 10만 행 스크롤, M2-15 의 1만 줄 diff 첫 그림,
//! M2-13 의 터미널 체크아웃 → 상태바, M2-17 의 수정 표시 · 타이핑 중 막힘, M2-16 의 1만 파일 목록 스크롤 · 터미널 `git add` → 그룹
//! 이동 — 을 회마다 모으고 저장소별로 요약한다. 무엇을 어떻게 재는지는 CONTRIBUTING.md 「성능 기준선」 절의 「git 시나리오」.

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
/// 저장 안 한 한 줄 편집 → 그 줄의 수정 표시(M2-17).
pub const TARGET_GUTTER_MS: f64 = 100.0;
/// 5,000줄 파일에서 초당 10자 30초 타이핑 중 메인 스레드가 한 번에 막힌 시간의 최댓값(M2-17 — 「50 ms 넘는 롱태스크가 없다」).
/// 「없다」 라서 중앙값이 아니라 유효한 회 **모두**의 최댓값으로 판정한다 — 한 회라도 넘으면 넘은 것이다.
pub const TARGET_BLOCK_MS: f64 = 50.0;
/// 1만 개 바뀐 파일의 커밋 목록을 끝까지 스크롤할 때 16.7 ms 안인 프레임의 몫(M2-16 — M1-14 와 같은 기준).
pub const TARGET_COMMIT_SCROLL_SHARE: f64 = 0.95;
/// 터미널 `git add` → 커밋 목록에서 그 파일이 스테이징됨 그룹으로(M2-16).
pub const TARGET_STAGE_MS: f64 = 500.0;

/// 측정하는 쪽이 모두 고치는 파일들의 폴더(`pw_git::testing::synth::PERF_CHANGES_DIR`). 앱이 `stage_ready` 로 고르는 경로도 이 아래여야 한다.
pub const CHANGES_DIR: &str = "perf/many";
/// 앱이 여는 타이핑 대상(`pw_git::testing::synth::PERF_TYPING_PATH`). 픽스처가 이것까지 들었는지 보는 데 쓴다.
pub const TYPING_PATH: &str = "perf/typing.rs";

/// 결과 파일만 받은 사람도 읽을 수 있게 싣는 정의(정본은 CONTRIBUTING.md 「성능 기준선」 의 「git 시나리오」).
pub const DEFINITIONS: GitDefinitions = GitDefinitions {
    open: "in the webview: from dispatching a ⌘9 keydown on the document (no OS key event) to the animation frame after the Git log canvas first drew a short hash",
    scroll: "after every row arrived (row count unchanged for 1 s): fling = top to bottom in 2000 frames, steady = 2 rows per frame for 600 frames; interval = rAF timestamp deltas, share within 16.7 ms judged on fling",
    diff: "in the webview: from a dblclick on the first log row (the 10,000-changed-line commit) to the frame after the commit tab's diff lines (.pw-diff-line) appeared — includes git.show/git.diff/git.blob and IPC",
    branch: "from the harness's wall clock just before it runs `git checkout <branch>` in the repository to the webview's wall clock at the frame after the status bar branch name changed",
    gutter: "in the webview: from dispatching an unsaved one-line edit to perf/typing.rs (5,000 lines, the gutter's HEAD base already loaded) to the frame after a `modified` gutter mark appeared",
    typing: "300 one-character inserts at 100 ms intervals (10 chars/s for 30 s) into perf/typing.rs; a 5 ms setTimeout probe runs meanwhile and a block is its callback gap minus 5 ms (WebKit has no Long Tasks API); judged on the longest block; the edits are reverted afterwards (nothing is saved)",
    commit_scroll: "the harness appends a line to each of the 10,000 files in perf/many; after ⌘0 (webview event) and every row arrived (row count unchanged for 1 s): fling = top to bottom in 1500 frames, steady = 2 rows per frame for 600 frames; share within 16.7 ms judged on fling",
    stage: "from the harness's wall clock just before it runs `git add <file>` in the repository to the webview's wall clock at the frame after that file's row appeared in the staged group",
    memory: "phys_footprint of the app instance right after the scenario finished (100k rows loaded, one commit tab and perf/typing.rs open, 10,000 changed files listed); not an idle baseline",
    judgement: "medians over valid measured runs, except the typing block: it is judged on the largest block of every valid run (\"no long task over 50 ms\" fails if any run has one); a run whose Git log row count differs from `git rev-list --count --all` fails",
    trust: "the project is registered untrusted (the default): blame uses the native (gix) engine, the git CLI paths of trusted projects are not measured",
};

#[derive(Debug, Clone, Serialize)]
pub struct GitDefinitions {
    pub open: &'static str,
    pub scroll: &'static str,
    pub diff: &'static str,
    pub branch: &'static str,
    pub gutter: &'static str,
    pub typing: &'static str,
    pub commit_scroll: &'static str,
    pub stage: &'static str,
    pub memory: &'static str,
    pub judgement: &'static str,
    pub trust: &'static str,
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

/// 타이핑 중 메인 스레드의 막힘(src/perf-scenario.ts 의 `BlockStats`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocks {
    /// 탐침이 돈 횟수.
    pub samples: u64,
    /// 가장 긴 막힘(ms).
    pub max_ms: f64,
    /// 50 ms 를 넘은 막힘의 수.
    pub over50: u64,
}

/// 타이핑 단계(M2-17).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Typing {
    pub keys: u64,
    pub lines: u64,
    pub blocks: Blocks,
    /// 키 하나의 `view.dispatch` 가 동기로 든 시간(ms) — 중앙값 · 최댓값.
    pub dispatch_p50_ms: f64,
    pub dispatch_max_ms: f64,
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
    pub gutter_ms: f64,
    pub typing: Typing,
    pub commit_rows: u64,
    pub commit_all_rows_ms: f64,
    pub commit_viewport: Viewport,
    pub commit_fling: Scroll,
    pub commit_steady: Scroll,
    pub stage_path: String,
    pub stage_ms: f64,
}

/// 앱이 기다리는, 측정하는 쪽이 저장소에 할 일.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `git checkout <--checkout>`(`checkout_ready`).
    Checkout,
    /// [`CHANGES_DIR`] 의 파일을 모두 고친다(`changes_ready`).
    Change,
    /// `git add <경로>`(`stage_ready`). 경로는 [`CHANGES_DIR`] 아래다.
    Stage(String),
}

/// 측정하는 쪽이 이미 한 일. 시각은 그 git 을 띄우기 직전의 벽시계.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Acted {
    pub checked_out_at: Option<f64>,
    pub changed: bool,
    pub staged_at: Option<f64>,
}

/// 시나리오가 어디까지 왔나([`progress`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// 다음 표시를 기다린다.
    Waiting,
    /// 앱이 기다린다 — 측정하는 쪽이 아직 하지 않았다.
    Act(Action),
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
struct GutterMark {
    ms: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitScrolled {
    rows: u64,
    all_rows_ms: f64,
    viewport: Viewport,
    fling: Scroll,
    steady: Scroll,
}

#[derive(Deserialize)]
struct StageReady {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Staged {
    path: String,
    epoch_ms: f64,
}

/// 앱이 고르라고 한 경로가 [`CHANGES_DIR`] 바로 아래 파일 하나인가 — 측정하는 쪽은 그 밖의 경로를 `git add` 하지 않는다.
pub fn stage_path_ok(path: &str) -> bool {
    path.strip_prefix(CHANGES_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|name| !name.is_empty() && !name.contains('/') && name != "." && name != "..")
}

/// 저장소의 [`CHANGES_DIR`] 에 있는 파일마다 줄 하나를 덧붙인다(「1만 개 바뀐 파일」). 고친 파일 수를 돌려준다 — 되돌리기는
/// `git checkout HEAD -- <CHANGES_DIR>`.
pub fn write_changes(repo: &std::path::Path) -> std::io::Result<usize> {
    use std::io::Write as _;
    let mut changed = 0;
    for entry in std::fs::read_dir(repo.join(CHANGES_DIR))? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(entry.path())?
            .write_all(b"changed by pw-perf\n")?;
        changed += 1;
    }
    Ok(changed)
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

/// `pid` 의 앱이 남긴 시나리오 표시에서 진행을 읽는다. `acted` 는 측정하는 쪽이 이미 한 일이다.
pub fn progress(marks: &[Mark], pid: u32, acted: Acted) -> Progress {
    match read(marks, pid, acted) {
        Ok(p) => p,
        Err(e) => Progress::Failed(e),
    }
}

fn read(marks: &[Mark], pid: u32, acted: Acted) -> Result<Progress, String> {
    if let Some(f) = data::<Failure>(marks, pid, "scenario_error")? {
        return Err(format!(
            "the scenario failed at `{}`: {}",
            f.step, f.message
        ));
    }
    let Some(ready) = data::<Ready>(marks, pid, "checkout_ready")? else {
        return Ok(Progress::Waiting);
    };
    let Some(checked_out_at) = acted.checked_out_at else {
        return Ok(Progress::Act(Action::Checkout));
    };
    if data::<serde_json::Value>(marks, pid, "changes_ready")?.is_none() {
        return Ok(Progress::Waiting);
    }
    if !acted.changed {
        return Ok(Progress::Act(Action::Change));
    }
    let Some(stage) = data::<StageReady>(marks, pid, "stage_ready")? else {
        return Ok(Progress::Waiting);
    };
    if !stage_path_ok(&stage.path) {
        return Err(format!(
            "the app asked to stage `{}`, which is not a file right under {CHANGES_DIR}",
            stage.path
        ));
    }
    let Some(staged_at) = acted.staged_at else {
        return Ok(Progress::Act(Action::Stage(stage.path)));
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
    let gutter: GutterMark =
        data(marks, pid, "gutter_mark")?.ok_or_else(|| missing("gutter_mark"))?;
    let typing: Typing = data(marks, pid, "typing")?.ok_or_else(|| missing("typing"))?;
    let commit: CommitScrolled =
        data(marks, pid, "commit_list_scroll")?.ok_or_else(|| missing("commit_list_scroll"))?;
    let staged: Staged =
        data(marks, pid, "staged_shown")?.ok_or_else(|| missing("staged_shown"))?;
    if staged.path != stage.path {
        return Err(format!(
            "the app asked to stage `{}` but reported `{}`",
            stage.path, staged.path
        ));
    }
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
        gutter_ms: gutter.ms,
        typing,
        commit_rows: commit.rows,
        commit_all_rows_ms: commit.all_rows_ms,
        commit_viewport: commit.viewport,
        commit_fling: commit.fling,
        commit_steady: commit.steady,
        stage_path: staged.path,
        stage_ms: staged.epoch_ms - staged_at,
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
    pub gutter_ms: Option<Summary>,
    pub typing_max_block_ms: Option<Summary>,
    pub typing_over50: Option<Summary>,
    pub commit_all_rows_ms: Option<Summary>,
    pub commit_fling_within16_7: Option<Summary>,
    pub commit_fling_p95_ms: Option<Summary>,
    pub commit_steady_within16_7: Option<Summary>,
    pub stage_ms: Option<Summary>,
    pub footprint_bytes: Option<Summary>,
}

/// 끝나지 못한 회 하나.
#[derive(Debug, Clone, Serialize)]
pub struct RunFailure {
    pub index: usize,
    pub warmup: bool,
    pub reason: String,
}

/// Git 로그가 저장소의 커밋을 모두 보였는가(「10만 행을 끝까지」 — 덜 받은 채 멈춘 목록을 스크롤한 회는 잰 것이 아니다).
pub fn check_rows(measure: &Measure, commits: u64) -> Result<(), String> {
    if measure.rows == commits {
        Ok(())
    } else {
        Err(format!(
            "the Git log settled at {} rows but the repository has {commits} commits (git rev-list --count --all)",
            measure.rows
        ))
    }
}

/// 중앙값이 목표 안인가(값이 없으면 `None`).
#[derive(Debug, Clone, Serialize)]
pub struct GitTargets {
    pub open_ms: f64,
    pub scroll_share: f64,
    pub diff_ms: f64,
    pub branch_ms: f64,
    pub gutter_ms: f64,
    pub block_ms: f64,
    pub commit_scroll_share: f64,
    pub stage_ms: f64,
    pub open_met: Option<bool>,
    pub scroll_met: Option<bool>,
    pub diff_met: Option<bool>,
    pub branch_met: Option<bool>,
    pub gutter_met: Option<bool>,
    pub block_met: Option<bool>,
    pub commit_scroll_met: Option<bool>,
    pub stage_met: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoResult {
    pub path: String,
    /// `.git/objects/info/commit-graph`(또는 `commit-graphs/`)가 있었는가.
    pub commit_graph: bool,
    /// `git rev-list --count --all` — Git 로그 목록이 보여야 하는 행 수.
    pub commits: Option<u64>,
    pub runs: Vec<GitRun>,
    /// 끝나지 못한 회(멈춤 · 단계 실패 · 행 수 어긋남). 요약에 들지 않는다.
    pub failures: Vec<RunFailure>,
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
        gutter_ms: values(&|r| r.measure.gutter_ms),
        typing_max_block_ms: values(&|r| r.measure.typing.blocks.max_ms),
        typing_over50: values(&|r| r.measure.typing.blocks.over50 as f64),
        commit_all_rows_ms: values(&|r| r.measure.commit_all_rows_ms),
        commit_fling_within16_7: values(&|r| r.measure.commit_fling.interval.within16_7),
        commit_fling_p95_ms: values(&|r| r.measure.commit_fling.interval.p95),
        commit_steady_within16_7: values(&|r| r.measure.commit_steady.interval.within16_7),
        stage_ms: values(&|r| r.measure.stage_ms),
        footprint_bytes: values(&|r| r.memory.footprint_bytes as f64),
    }
}

pub fn git_targets(summary: &GitSummary) -> GitTargets {
    GitTargets {
        open_ms: TARGET_OPEN_MS,
        scroll_share: TARGET_SCROLL_SHARE,
        diff_ms: TARGET_DIFF_MS,
        branch_ms: TARGET_BRANCH_MS,
        gutter_ms: TARGET_GUTTER_MS,
        block_ms: TARGET_BLOCK_MS,
        commit_scroll_share: TARGET_COMMIT_SCROLL_SHARE,
        stage_ms: TARGET_STAGE_MS,
        open_met: summary.open_ms.map(|s| s.median <= TARGET_OPEN_MS),
        scroll_met: summary
            .fling_within16_7
            .map(|s| s.median >= TARGET_SCROLL_SHARE),
        diff_met: summary.diff_ms.map(|s| s.median <= TARGET_DIFF_MS),
        branch_met: summary.branch_ms.map(|s| s.median <= TARGET_BRANCH_MS),
        gutter_met: summary.gutter_ms.map(|s| s.median <= TARGET_GUTTER_MS),
        // 「없다」: 유효한 회 모두의 최댓값(중앙값은 참고로만 표에 싣는다).
        block_met: summary
            .typing_max_block_ms
            .map(|s| s.max <= TARGET_BLOCK_MS),
        commit_scroll_met: summary
            .commit_fling_within16_7
            .map(|s| s.median >= TARGET_COMMIT_SCROLL_SHARE),
        stage_met: summary.stage_ms.map(|s| s.median <= TARGET_STAGE_MS),
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
    /// 측정을 끝까지 하지 못하고 멈춘 까닭(중단 신호 · 저장소를 되돌리지 못함 · 앱을 치우지 못함). 그때까지의 회는 남긴다.
    pub stopped: Option<String>,
}

/// git 시나리오의 `summary.md`.
pub fn git_report_markdown(report: &GitReport) -> String {
    let mut md =
        String::from("## Pitwall git scenario (M2-13 · M2-14 · M2-15 · M2-16 · M2-17)\n\n");
    if let Some(stopped) = &report.stopped {
        md.push_str(&format!(
            "> **Stopped early:** {stopped}. The tables hold the runs before it.\n\n"
        ));
    }
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
        "\n- runs per repository: {} measured + {} warm-up · checkout branch `{}` · medians over valid measured runs, except the typing block (largest over every valid run) · project registered untrusted\n",
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
        let rows: [(&str, String, String, Option<bool>); 15] = [
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
            (
                "one-line edit → gutter mark (M2-17)",
                cell(s.gutter_ms, " ms"),
                format!("≤ {TARGET_GUTTER_MS} ms"),
                t.gutter_met,
            ),
            (
                "typing 30 s: longest main-thread block (M2-17, judged on the max)",
                cell(s.typing_max_block_ms, " ms"),
                format!("max ≤ {TARGET_BLOCK_MS} ms in every run"),
                t.block_met,
            ),
            (
                "typing 30 s: blocks over 50 ms",
                cell(s.typing_over50, ""),
                "—".into(),
                None,
            ),
            (
                "commit list fling: frames within 16.7 ms (M2-16)",
                cell(s.commit_fling_within16_7, ""),
                format!("≥ {TARGET_COMMIT_SCROLL_SHARE}"),
                t.commit_scroll_met,
            ),
            (
                "commit list fling: frame interval p95",
                cell(s.commit_fling_p95_ms, " ms"),
                "—".into(),
                None,
            ),
            (
                "commit list steady: frames within 16.7 ms",
                cell(s.commit_steady_within16_7, ""),
                "—".into(),
                None,
            ),
            (
                "git add → staged group (M2-16)",
                cell(s.stage_ms, " ms"),
                format!("≤ {TARGET_STAGE_MS} ms"),
                t.stage_met,
            ),
            (
                "10,000 changed files listed",
                cell(s.commit_all_rows_ms, " ms"),
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
        md.push_str("\n| Run | Excluded | Open ms | Fling ≤16.7 | Diff ms | Branch ms | Gutter ms | Block max ms | Commit fling ≤16.7 | Stage ms | Rows | Viewport |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        for r in &repo.runs {
            let m = &r.measure;
            md.push_str(&format!(
                "| {}{} | {} | {:.1} | {:.2} | {:.1} | {:.1} | {:.1} | {:.1} | {:.2} | {:.1} | {} | {}×{} |\n",
                r.index,
                if r.warmup { " (warm-up)" } else { "" },
                r.excluded.as_deref().unwrap_or("—"),
                m.open_ms,
                m.fling.interval.within16_7,
                m.diff_ms,
                m.branch_ms,
                m.gutter_ms,
                m.typing.blocks.max_ms,
                m.commit_fling.interval.within16_7,
                m.stage_ms,
                m.rows,
                m.scroll_viewport.width,
                m.scroll_viewport.height
            ));
        }
        if !repo.failures.is_empty() {
            md.push_str("\n| Failed run | Reason |\n|---|---|\n");
            for f in &repo.failures {
                md.push_str(&format!(
                    "| {}{} | {} |\n",
                    f.index,
                    if f.warmup { " (warm-up)" } else { "" },
                    f.reason.replace('|', "\\|").replace('\n', " ")
                ));
            }
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

    /// 체크아웃 뒤 타이핑 · 커밋 목록 단계의 표시(`changes_ready` 까지).
    fn until_changes(pid: u32) -> String {
        let scroll = serde_json::json!({"interval": stats(0.98), "work": stats(1.0)});
        [
            line("branch_shown", pid, 6.0, serde_json::json!({"branch": "perf-checkout", "epochMs": 10_120.5})),
            line("gutter_mark", pid, 7.0, serde_json::json!({"ms": 48.5, "line": 12})),
            line("typing", pid, 8.0, serde_json::json!({"keys": 300, "lines": 5000, "blocks": {"samples": 5000, "maxMs": 31.5, "over50": 0}, "dispatchP50Ms": 0.4, "dispatchMaxMs": 3.0})),
            line("changes_ready", pid, 9.0, serde_json::json!({})),
            line("commit_list_scroll", pid, 10.0, serde_json::json!({"rows": 10_003, "allRowsMs": 900.0, "viewport": {"width": 320, "height": 700}, "fling": scroll, "steady": scroll})),
        ]
        .concat()
    }

    /// 측정하는 쪽이 할 일(체크아웃 → 파일 고치기 → `git add`)을 차례로 받고, 한 뒤에야 다음으로 간다. 상태바 · 그룹 이동 시간은 그 일의
    /// 시작 벽시계에서 잰다.
    #[test]
    fn the_scenario_waits_for_each_harness_action_and_measures_from_it() {
        let pid = 42;
        let none = Acted::default();
        let early = parse(&line("git_log_open", pid, 1.0, serde_json::json!({})));
        assert_eq!(progress(&early, pid, none), Progress::Waiting);
        let ready = parse(&until_ready(pid));
        assert_eq!(progress(&ready, pid, none), Progress::Act(Action::Checkout));
        let checked = Acted {
            checked_out_at: Some(10_000.0),
            ..none
        };
        assert_eq!(progress(&ready, pid, checked), Progress::Waiting);
        let changes = parse(&[until_ready(pid), until_changes(pid)].concat());
        assert_eq!(
            progress(&changes, pid, checked),
            Progress::Act(Action::Change)
        );
        let changed = Acted {
            changed: true,
            ..checked
        };
        assert_eq!(progress(&changes, pid, changed), Progress::Waiting);
        let stage = [
            until_ready(pid),
            until_changes(pid),
            line(
                "stage_ready",
                pid,
                11.0,
                serde_json::json!({"path": "perf/many/f00000.txt"}),
            ),
        ]
        .concat();
        assert_eq!(
            progress(&parse(&stage), pid, changed),
            Progress::Act(Action::Stage("perf/many/f00000.txt".into()))
        );
        let staged = Acted {
            staged_at: Some(20_000.0),
            ..changed
        };
        assert_eq!(progress(&parse(&stage), pid, staged), Progress::Waiting);
        let done = parse(
            &[
                stage,
                line(
                    "staged_shown",
                    pid,
                    12.0,
                    serde_json::json!({"path": "perf/many/f00000.txt", "epochMs": 20_210.0}),
                ),
                line("scenario_done", pid, 13.0, serde_json::json!({})),
            ]
            .concat(),
        );
        let Progress::Done(m) = progress(&done, pid, staged) else {
            panic!("{:?}", progress(&done, pid, staged));
        };
        assert_eq!(m.branch_ms, 120.5);
        assert_eq!(m.open_ms, 180.5);
        assert_eq!(m.rows, 100_002);
        assert_eq!(m.fling.interval.within16_7, 0.97);
        assert_eq!(m.diff_path.as_deref(), Some("perf/big.rs"));
        assert_eq!(m.branch_from.as_deref(), Some("main"));
        assert_eq!(m.branch_to, "perf-checkout");
        assert_eq!(m.gutter_ms, 48.5);
        assert_eq!(m.typing.blocks.max_ms, 31.5);
        assert_eq!(m.typing.keys, 300);
        assert_eq!(m.commit_rows, 10_003);
        assert_eq!(m.commit_fling.interval.within16_7, 0.98);
        assert_eq!(m.stage_path, "perf/many/f00000.txt");
        assert_eq!(m.stage_ms, 210.0);
        // 다른 프로세스의 표시는 보지 않는다.
        assert_eq!(progress(&done, 7, staged), Progress::Waiting);
    }

    /// 앱이 고르라고 한 경로는 픽스처 폴더 바로 아래 파일 하나여야 한다 — 측정하는 쪽은 그 밖을 `git add` 하지 않는다.
    #[test]
    fn the_harness_stages_only_a_file_right_under_the_changes_folder() {
        assert!(stage_path_ok("perf/many/f00000.txt"));
        for bad in [
            "perf/many",
            "perf/many/",
            "perf/many/..",
            "perf/many/../../x",
            "perf/many/sub/f.txt",
            "perf/manyx/f.txt",
            "perf/typing.rs",
            "/perf/many/f.txt",
            ".",
        ] {
            assert!(!stage_path_ok(bad), "{bad}");
        }
        let pid = 42;
        let marks = parse(
            &[
                until_ready(pid),
                until_changes(pid),
                line(
                    "stage_ready",
                    pid,
                    11.0,
                    serde_json::json!({"path": "perf/many/../../x"}),
                ),
            ]
            .concat(),
        );
        let acted = Acted {
            checked_out_at: Some(1.0),
            changed: true,
            staged_at: None,
        };
        let Progress::Failed(why) = progress(&marks, pid, acted) else {
            panic!("a path outside the fixture folder passed");
        };
        assert!(why.contains("not a file right under perf/many"), "{why}");
    }

    /// 목록이 저장소의 커밋을 다 받지 못한 채 멈췄으면 그 회는 실패다.
    #[test]
    fn a_run_whose_log_rows_differ_from_the_commit_count_fails() {
        let pid = 42;
        let done = parse(
            &[
                until_ready(pid),
                until_changes(pid),
                line(
                    "stage_ready",
                    pid,
                    11.0,
                    serde_json::json!({"path": "perf/many/f00000.txt"}),
                ),
                line(
                    "staged_shown",
                    pid,
                    12.0,
                    serde_json::json!({"path": "perf/many/f00000.txt", "epochMs": 20_210.0}),
                ),
                line("scenario_done", pid, 13.0, serde_json::json!({})),
            ]
            .concat(),
        );
        let acted = Acted {
            checked_out_at: Some(10_000.0),
            changed: true,
            staged_at: Some(20_000.0),
        };
        let Progress::Done(m) = progress(&done, pid, acted) else {
            panic!("not done");
        };
        assert_eq!(check_rows(&m, 100_002), Ok(()));
        let why = check_rows(&m, 100_003).unwrap_err();
        assert!(
            why.contains("settled at 100002 rows") && why.contains("100003 commits"),
            "{why}"
        );
    }

    /// 끝나지 못한 회와 멈춘 까닭이 요약에 남는다.
    #[test]
    fn the_summary_lists_failed_runs_and_why_the_measurement_stopped() {
        let repo = RepoResult {
            path: "target/perf-git/graph".into(),
            commit_graph: true,
            commits: Some(100_003),
            runs: Vec::new(),
            failures: vec![RunFailure {
                index: 2,
                warmup: false,
                reason: "run 2: the scenario did not finish within 420s".into(),
            }],
            valid_runs: 0,
            summary: summarize_git(&[]),
            targets: git_targets(&summarize_git(&[])),
        };
        let md = git_markdown(std::slice::from_ref(&repo));
        assert!(
            md.contains("| 2 | run 2: the scenario did not finish within 420s |"),
            "{md}"
        );
    }

    #[test]
    fn every_file_in_the_changes_folder_gets_one_more_line() {
        let repo = std::env::temp_dir().join(format!("pw-perf-changes-{}", std::process::id()));
        let dir = repo.join(CHANGES_DIR);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for i in 0..3 {
            std::fs::write(dir.join(format!("f{i}.txt")), format!("file {i}\n")).unwrap();
        }
        std::fs::write(repo.join("perf/other.txt"), "x\n").unwrap();
        let changed = write_changes(&repo);
        let read = |p: &str| std::fs::read_to_string(repo.join(p)).unwrap();
        let (f1, other) = (read("perf/many/f1.txt"), read("perf/other.txt"));
        std::fs::remove_dir_all(&repo).unwrap();
        assert_eq!(changed.unwrap(), 3);
        assert_eq!(f1, "file 1\nchanged by pw-perf\n");
        assert_eq!(other, "x\n");
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
            progress(&failed, pid, Acted::default()),
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
        let Progress::Failed(why) = progress(&malformed, pid, Acted::default()) else {
            panic!("a malformed mark passed");
        };
        assert!(why.contains("`checkout_ready` mark is malformed"), "{why}");
    }

    #[test]
    fn targets_judge_the_median_and_the_fling_share() {
        // 목표 값 그대로면 안이고, 조금 넘으면 밖이다.
        let summary = |over: f64| GitSummary {
            open_ms: summarize(&[TARGET_OPEN_MS + over]),
            first_draw_ms: None,
            all_rows_ms: None,
            fling_within16_7: summarize(&[TARGET_SCROLL_SHARE - over / 10.0]),
            fling_p95_ms: None,
            steady_within16_7: summarize(&[0.5]),
            diff_ms: summarize(&[TARGET_DIFF_MS + over]),
            branch_ms: summarize(&[TARGET_BRANCH_MS + over]),
            gutter_ms: summarize(&[TARGET_GUTTER_MS + over]),
            typing_max_block_ms: summarize(&[TARGET_BLOCK_MS + over]),
            typing_over50: None,
            commit_all_rows_ms: None,
            commit_fling_within16_7: summarize(&[TARGET_COMMIT_SCROLL_SHARE - over / 10.0]),
            commit_fling_p95_ms: None,
            commit_steady_within16_7: None,
            stage_ms: summarize(&[TARGET_STAGE_MS + over]),
            footprint_bytes: None,
        };
        let met = |t: GitTargets| {
            [
                t.open_met,
                t.scroll_met,
                t.diff_met,
                t.branch_met,
                t.gutter_met,
                t.block_met,
                t.commit_scroll_met,
                t.stage_met,
            ]
        };
        assert_eq!(met(git_targets(&summary(0.0))), [Some(true); 8]);
        assert_eq!(met(git_targets(&summary(0.1))), [Some(false); 8]);
        // 막힘은 중앙값이 아니라 최댓값: 다섯 회 중 두 회가 넘으면 중앙값(40)이 안이어도 넘은 것이다.
        let mut s = summary(0.0);
        s.typing_max_block_ms = summarize(&[30.0, 35.0, 40.0, 80.0, 120.0]);
        assert_eq!(git_targets(&s).block_met, Some(false));
        s.typing_max_block_ms = summarize(&[30.0, 35.0, 40.0, 45.0, 50.0]);
        assert_eq!(git_targets(&s).block_met, Some(true));
    }
}
