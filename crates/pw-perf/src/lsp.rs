//! LSP · 심볼 시나리오(`pw-perf --scenario …`): 앱이 프로젝트 창에서 돌린 시나리오(`apps/desktop/src/perf-scenario-lsp.ts`)의 표시와
//! 가짜 언어 서버(`pw-perf-lsp`, [`crate::lsp_fake`])가 남긴 표지를 읽어 15-m4-plan.md M4-19 가 release 번들에서 재는 것 — 06 「성능
//! 목표」 의 LSP · 편집 행(결정 M4-D19)과 심볼로 이동 · 클래스로 이동(M4-16) — 을 회마다 모으고 요약한다. 무엇을 어떻게 재는지는
//! CONTRIBUTING.md 「성능 기준선」 절의 「LSP 시나리오」.
//!
//! - `lsp` · `lsp-typing` · `lsp-typing-off`: 회마다 임시 프로젝트에 픽스처([`write_fixture`])를 쓰고 신뢰로 등록한다. 전역 설정
//!   (`$CONFIG/settings.toml`)의 `[lsp.rust-analyzer] path` 로 가짜 서버를 끼운다 — 끔(`lsp-typing-off`)은 `enabled = false`.
//! - `symbols` · `classes`: `--project` 의 합성 저장소(`pw-index` 의 `perf_repo` 예제가 만든다)를 미신뢰로 등록한다 — 서버는 뜨지 않고
//!   심볼 인덱스만 돈다.
//! - `lsp-idle`: `--project`(이 저장소의 사본)를 신뢰로 등록하고 `[lsp.rust-analyzer] path` 를 `--lsp-server` 로. 서버가 준비된 뒤
//!   `--idle-secs` 를 기다려 메모리를 잰다 — Pitwall 의 프로세스와 서버를 나눠 적는다. 서버는 앱 프로세스의 자손([`descendants`] —
//!   서버는 앱의 책임 pid 그룹에 들지 않는다)이고, 그룹의 보조 프로세스(`SandboxHelper` 등)는 앱 몫이다.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::git::{FrameStats, RunFailure, Scroll, TARGET_VSYNC_MS, Viewport};
use crate::lsp_fake::{Config, Problems};
use crate::marks::Mark;
use crate::report::{Build, Commit, Hardware, Memory, Os, environment};
use crate::stats::{Summary, summarize};

/// 앱 시나리오 이름(`apps/desktop/src-tauri/src/perf.rs` 의 `SCENARIOS` 가운데 이 모듈의 것).
pub const SCENARIOS: &[&str] = &[
    "lsp",
    "lsp-typing",
    "lsp-typing-off",
    "lsp-idle",
    "symbols",
    "classes",
];

/// 픽스처(앱 시나리오 `perf-scenario-lsp.ts` 의 같은 이름 — 시험이 같은지 본다).
pub const MAIN_PATH: &str = "src/main.rs";
pub const MAIN_FIRST_LINE: &str = "// pitwall perf: lsp target";
pub const PROBLEMS_PATH: &str = "src/problems.rs";
pub const COMPLETION_LINE: usize = 20;
pub const DIAG_LINE: usize = 30;
pub const DEF_LINE: usize = 40;
pub const TYPING_LINE: usize = 50;
pub const DEF_WORD: &str = "target_fn";
pub const TARGET_FIRST_LINE: &str = "// pitwall perf: definition target";
/// 정의 대상 줄(1 부터) — 가짜에는 0 부터(`definition_line`)로 준다.
pub const DEF_TARGET_LINE: usize = 401;
pub const DEF_TARGETS: usize = 10;
pub const PROBLEM_FILES: usize = 2_000;
pub const PROBLEMS_PER_FILE: usize = 5;
const MAIN_LINES: usize = 600;
const TARGET_LINES: usize = 500;
/// 가짜 서버의 표지 파일(회 폴더 안 — 끝나면 결과 폴더로 옮긴다).
pub const STAMPS_FILE: &str = "lsp-stamps.jsonl";
/// 가짜 서버의 설정 파일(회 폴더 안).
pub const FAKE_CONFIG_FILE: &str = "pw-perf-lsp.json";
/// 끼우는 서버 정의(내장 Rust 확장의 `rust-analyzer` — 픽스처가 `Cargo.toml` 을 둬 루트 표지가 맞는다).
pub const SERVER_ID: &str = "rust-analyzer";
/// `lsp-idle` 이 여는 파일(이 저장소).
pub const IDLE_PATH: &str = "crates/pw-core/src/lib.rs";

/// 목표(06 「성능 목표」 의 LSP · 편집 행 — 결정 M4-D19).
pub const TARGET_COMPLETION_MS: f64 = 50.0;
pub const TARGET_DIAGNOSTICS_MS: f64 = 50.0;
pub const TARGET_DEFINITION_MS: f64 = 100.0;
pub const TARGET_TYPING_DELTA_MS: f64 = 2.0;
pub const TARGET_INDEX_MS: f64 = 15_000.0;
pub const TARGET_INDEX_MEMORY_MB: f64 = 120.0;
pub const TARGET_QUERY_MS: f64 = 30.0;
pub const TARGET_PROBLEMS_MS: f64 = 200.0;
pub const TARGET_SCROLL_SHARE: f64 = 0.95;

/// 결과 파일만 받은 사람도 읽을 수 있게 싣는 정의(정본은 CONTRIBUTING.md 「성능 기준선」 의 「LSP 시나리오」).
pub const DEFINITIONS: &[(&str, &str)] = &[
    (
        "server",
        "a fake language server (`pw-perf-lsp`) plugged into the bundled `rust-analyzer` definition by the global setting `[lsp.rust-analyzer] path`; it answers at once and stamps the wall clock of what it sent (the targets are Pitwall's side only, M4-D19)",
    ),
    (
        "completion",
        "in the webview: from inserting `.` (the server's trigger character, `view.dispatch` with `input.type`) to the frame that painted the autocomplete popup with the server's first item (the end mark of every \"frame\" row: the DOM change that made it true, seen by a MutationObserver → the next animation frame → the first task after that frame, as typing's `painted`; each sample starts after a 0 – 16.7 ms pause so the start is not locked to the frame phase); 20 per run; per-run value = median",
    ),
    (
        "diagnostics",
        "publish → frame: from the fake server's stamp just before it wrote the `publishDiagnostics` that marks the edited line to the wall clock of the frame that painted that line's underline (`.pw-diagnostic`); 20 per run; per-run value = median. edit → frame (reference) also holds the client's `didChange`",
    ),
    (
        "definition",
        "answer → frame: from the fake server's stamp just before it wrote the `textDocument/definition` answer (a different file each time, so opening it is included) to the wall clock of the frame that painted that file's editor with the cursor on the target line; `gotoDeclaration` (⌘B's function) is called directly — keymap handling is not included; 10 per run; per-run value = median",
    ),
    (
        "typing",
        "300 characters at 50 ms, each inserted with `view.dispatch` (`input.type`, keymap · IME not included); per key: insert → after the next frame's rendering (a message posted from the next animation frame); per-run value = p95. LSP on and off are separate runs (`lsp-typing` · `lsp-typing-off`, the latter with `[lsp.rust-analyzer] enabled = false`); 06 judges on − off ≤ 2 ms on the medians of the per-run p95",
    ),
    (
        "problems",
        "opening `src/problems.rs` makes the fake server publish 5 diagnostics to each of 2,000 files (10,000); after the status bar counts are complete: ⌘6 keydown → the frame that painted the first problem row; then fling top to bottom in 1500 frames, judged as 06 「스크롤 60fps」 (share within one refresh ≥ 0.95 and median interval ≤ 17 ms)",
    ),
    (
        "symbols",
        "a synthetic repository of 20,000 files and 500,000 symbols registered untrusted (no language server); first index = launch → `index.status` ready with nothing pending (polled every 50 ms on a connection of the same project); then, with no wait, the first popup (⌘⌥O for `symbols`, ⌘O for `classes`) and the query typed a character at a time: input event → the frame that painted that query's rows (`totalMs`), of which input event → the rows in the DOM (`seenMs`) and the `index.symbols.search` round trip (`rpcMs`, Host + IPC); the second popup is measured the same way afterwards (not a first use)",
    ),
    (
        "idle",
        "`lsp-idle`: this repository's copy registered trusted with the real server at `--lsp-server`; after the file opened and the server stayed ready for 15 s, wait idle_secs and sample phys_footprint; Pitwall's processes (`pitwall`, WebKit and the instance's helpers) and the server's (the descendants of the app process — the server is not in the app's responsible-pid group) are reported apart",
    ),
    (
        "memory",
        "phys_footprint by the responsible-pid group plus the descendants of the app process (the language server and its children), sampled after the scenario (symbols: after the popups — the Rust index lives in the `pitwall` process)",
    ),
];

/// 픽스처의 `src/main.rs`.
pub fn main_text() -> String {
    let mut s = String::new();
    for line in 1..=MAIN_LINES {
        let text = match line {
            1 => MAIN_FIRST_LINE.to_owned(),
            2 => String::new(),
            3 => "fn main() {".to_owned(),
            COMPLETION_LINE => "    item".to_owned(),
            DIAG_LINE => "    // diagnostics here".to_owned(),
            DEF_LINE => format!("    {DEF_WORD}();"),
            TYPING_LINE => "    // typing here".to_owned(),
            MAIN_LINES => "}".to_owned(),
            n => format!("    let v{n} = {n};"),
        };
        s.push_str(&text);
        s.push('\n');
    }
    s
}

/// 정의 대상 `src/t<k>.rs`.
pub fn target_text(k: usize) -> String {
    let mut s = format!("{TARGET_FIRST_LINE} {k}\n");
    for line in 2..=TARGET_LINES {
        if line == DEF_TARGET_LINE {
            let _ = writeln!(s, "pub fn {DEF_WORD}() {{}}");
        } else {
            let _ = writeln!(s, "pub const C{line}: u64 = {line};");
        }
    }
    s
}

fn file_uri(path: &Path) -> String {
    // 퍼센트 인코딩 · Windows 의 `file:///C:/…` 꼴은 url 이 만든다 — `file://` 에 경로를 그대로 붙이면 공백 · `\\` 가 URI 를 깬다.
    url::Url::from_file_path(path)
        .expect("the fixture paths are absolute")
        .into()
}

/// 회 폴더의 프로젝트에 픽스처를 쓰고, 가짜 서버의 설정(`run_root/pw-perf-lsp.json`)과 전역 설정(`config_dir/settings.toml`)을 쓴다.
/// `fake` 가 없으면(`lsp-typing-off`) 서버를 끈다. 경로는 정규화된 것이어야 한다 — 가짜가 답하는 URI 가 앱이 아는 경로와 같아야 한다.
pub fn write_fixture(
    project: &Path,
    run_root: &Path,
    config_dir: &Path,
    fake: Option<&Path>,
) -> Result<(), String> {
    let write = |rel: &str, text: &str| {
        let path = project.join(rel);
        std::fs::create_dir_all(path.parent().expect("a file under the project"))
            .and_then(|()| std::fs::write(&path, text))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"perf-lsp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    write(MAIN_PATH, &main_text())?;
    for k in 0..DEF_TARGETS {
        write(&format!("src/t{k}.rs"), &target_text(k))?;
    }
    write(PROBLEMS_PATH, "// pitwall perf: problems trigger\n")?;
    let mut problem_files = Vec::with_capacity(PROBLEM_FILES);
    for i in 0..PROBLEM_FILES {
        let rel = format!("src/p/f{i:04}.rs");
        write(
            &rel,
            &(0..PROBLEMS_PER_FILE + 1)
                .map(|k| format!("pub const P{k}: u64 = {k};\n"))
                .collect::<String>(),
        )?;
        problem_files.push(file_uri(&project.join(rel)));
    }
    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let settings = match fake {
        Some(fake) => {
            let config = Config {
                stamps: Some(run_root.join(STAMPS_FILE)),
                completion_items: 50,
                definition_targets: (0..DEF_TARGETS)
                    .map(|k| file_uri(&project.join(format!("src/t{k}.rs"))))
                    .collect(),
                definition_line: (DEF_TARGET_LINE - 1) as u32,
                problems: Some(Problems {
                    trigger: format!("/{PROBLEMS_PATH}"),
                    files: problem_files,
                    per_file: PROBLEMS_PER_FILE,
                }),
            };
            let config_path = run_root.join(FAKE_CONFIG_FILE);
            std::fs::write(
                &config_path,
                serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            server_settings(fake, &[&config_path])
        }
        None => format!("schema = 1\n\n[lsp.{SERVER_ID}]\nenabled = false\n"),
    };
    std::fs::write(config_dir.join("settings.toml"), settings).map_err(|e| e.to_string())
}

/// `[lsp.rust-analyzer] path` 로 `server` 를 끼우는 전역 설정.
pub fn server_settings(server: &Path, args: &[&Path]) -> String {
    let quote = |p: &Path| toml_string(&p.display().to_string());
    let args: Vec<String> = args.iter().map(|a| quote(a)).collect();
    format!(
        "schema = 1\n\n[lsp.{SERVER_ID}]\npath = {}\nargs = [{}]\n",
        quote(server),
        args.join(", ")
    )
}

fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 가짜 서버의 표지 한 줄.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    pub event: String,
    pub epoch_ms: f64,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub n: Option<u64>,
}

/// 표지 파일을 읽는다(찢긴 줄은 건너뛴다).
pub fn parse_stamps(text: &str) -> Vec<Stamp> {
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagSample {
    edit_epoch_ms: f64,
    shown_epoch_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefSample {
    ask_epoch_ms: f64,
    shown_epoch_ms: f64,
    path: String,
}

/// 짝지은 표본 하나: 앱 쪽 시작(편집 · 요청) → 프레임, 서버가 보낸 때 → 프레임.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Paired {
    pub client_ms: f64,
    pub server_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Typing {
    pub lsp: bool,
    pub keys: u64,
    pub frame: FrameStats,
    pub painted: FrameStats,
    /// 키별 몫을 묶음별로 줄인 것(`perf-scenario-lsp.ts` 의 `typingBreakdown` — 이 칸을 싣기 전의 앱은 없다).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakdown: Option<TypingBreakdown>,
}

/// 타이핑 키마다의 몫(`typing_keys` 표지 — 표지 하나의 크기 상한 16 KB 안에 들게 열 이름 + 줄로 싣는다). 결과 파일에 남겨 키별로
/// 다시 본다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypingKeys {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// 타이핑 키의 묶음: 모든 키, 그 프레임 뒤 자동완성 팝업이 있던 키, 없던 키.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypingBreakdown {
    pub all: TypingGroup,
    pub popup: TypingGroup,
    pub no_popup: TypingGroup,
}

/// 한 묶음의 분포(ms): 키 → 그 프레임 뒤(`painted`), `view.dispatch` 동기 시간(`dispatch`), 그중 `didChange` 보내기(`change`),
/// 뷰 갱신 합(`update`), rAF 콜백 합(`raf`), dispatch 뒤 → 다음 rAF(`wait`), rAF → 그 프레임 뒤(`render`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypingGroup {
    pub keys: u64,
    pub painted: FrameStats,
    pub dispatch: FrameStats,
    /// 그중 상태 계산(`tr.state` — 자동완성 다시 거르기). 이 칸을 싣기 전의 앱은 없다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<FrameStats>,
    /// 그중 배치를 읽어 강제한 동기 배치(`getBoundingClientRect`). 이 칸을 싣기 전의 앱은 없다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<FrameStats>,
    pub change: FrameStats,
    pub update: FrameStats,
    pub raf: FrameStats,
    pub wait: FrameStats,
    pub render: FrameStats,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProblemsOpen {
    pub ms: f64,
    pub rows: u64,
    pub viewport: Viewport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemsScroll {
    #[serde(flatten)]
    pub scroll: Scroll,
    pub scroll_height: f64,
    pub viewport: Viewport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexReady {
    pub epoch_ms: f64,
    pub from_frame_ms: f64,
    pub files: u64,
    pub symbols: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyTime {
    pub query: String,
    pub total_ms: f64,
    /// 입력 이벤트 → 결과 줄이 DOM 에 든 때(M4-19 리뷰 뒤의 시나리오만 적는다).
    #[serde(default)]
    pub seen_ms: Option<f64>,
    /// 입력 이벤트 → 결과 줄 DOM 뒤 첫 rAF(15-m4-plan §3 의 ⌘⌥O 첫 키 행 뒤의 시나리오만 적는다).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_ms: Option<f64>,
    pub rpc_ms: Option<f64>,
    /// `lsp.workspaceSymbol` 왕복(묻지 않았으면 없다).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsp_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolKeys {
    pub popup: String,
    pub open_ms: f64,
    pub first: KeyTime,
    pub keys: Vec<KeyTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerReady {
    pub epoch_ms: f64,
    pub from_frame_ms: f64,
}

/// 한 회에 잰 것(시나리오마다 해당하는 칸만).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LspMeasure {
    pub ready_ms: Option<f64>,
    pub completion_ms: Vec<f64>,
    pub diagnostics: Vec<Paired>,
    pub definition: Vec<Paired>,
    pub typing: Option<Typing>,
    pub problems_loaded_ms: Option<f64>,
    pub problems_open: Option<ProblemsOpen>,
    pub problems_scroll: Option<ProblemsScroll>,
    pub problems_published: Option<u64>,
    pub index: Option<IndexReady>,
    pub symbol_keys: Option<SymbolKeys>,
    pub symbol_keys_second: Option<SymbolKeys>,
    pub server_ready: Option<ServerReady>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typing_keys: Option<TypingKeys>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LspProgress {
    Waiting,
    Done(Box<LspMeasure>),
    Failed(String),
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
    serde_json::from_value(mark.data.clone().unwrap_or(Value::Null))
        .map(Some)
        .map_err(|e| format!("the app's `{event}` mark is malformed: {e}"))
}

/// 시나리오의 표시가 이미 있는가(`lsp-idle` 은 `server_ready` 를 보고 유휴 시간을 센다).
pub fn mark_of(marks: &[Mark], pid: u32, event: &str) -> Option<Mark> {
    marks
        .iter()
        .find(|m| m.event == event && m.pid == pid)
        .cloned()
}

/// `pid` 의 앱이 남긴 표시와 가짜 서버의 표지에서 진행을 읽는다.
pub fn progress(marks: &[Mark], stamps: &[Stamp], pid: u32, scenario: &str) -> LspProgress {
    match read(marks, stamps, pid, scenario) {
        Ok(p) => p,
        Err(e) => LspProgress::Failed(e),
    }
}

fn read(marks: &[Mark], stamps: &[Stamp], pid: u32, scenario: &str) -> Result<LspProgress, String> {
    if let Some(f) = data::<Failure>(marks, pid, "scenario_error")? {
        return Err(format!(
            "the scenario failed at `{}`: {}",
            f.step, f.message
        ));
    }
    if mark_of(marks, pid, "scenario_done").is_none() {
        return Ok(LspProgress::Waiting);
    }
    let missing = |event: &str| format!("the scenario finished without a `{event}` mark");
    let mut m = LspMeasure {
        ready_ms: data::<Value>(marks, pid, "lsp_ready")?.and_then(|v| v["ms"].as_f64()),
        typing: data(marks, pid, "typing")?,
        typing_keys: data(marks, pid, "typing_keys")?,
        problems_loaded_ms: data::<Value>(marks, pid, "problems_loaded")?
            .and_then(|v| v["ms"].as_f64()),
        problems_open: data(marks, pid, "problems_open")?,
        problems_scroll: data(marks, pid, "problems_scroll")?,
        index: data(marks, pid, "index_ready")?,
        symbol_keys: data(marks, pid, "symbol_keys")?,
        symbol_keys_second: data(marks, pid, "symbol_keys_second")?,
        server_ready: data(marks, pid, "server_ready")?,
        ..LspMeasure::default()
    };
    match scenario {
        "lsp" => {
            let completion =
                data::<Value>(marks, pid, "completion")?.ok_or_else(|| missing("completion"))?;
            m.completion_ms = serde_json::from_value(completion["samples"].clone())
                .map_err(|e| format!("the app's `completion` mark is malformed: {e}"))?;
            let diags: Vec<DiagSample> = data::<Value>(marks, pid, "diagnostics")?
                .map(|v| serde_json::from_value(v["samples"].clone()))
                .ok_or_else(|| missing("diagnostics"))?
                .map_err(|e| format!("the app's `diagnostics` mark is malformed: {e}"))?;
            m.diagnostics = pair_diagnostics(&diags, stamps)?;
            let defs: Vec<DefSample> = data::<Value>(marks, pid, "definition")?
                .map(|v| serde_json::from_value(v["samples"].clone()))
                .ok_or_else(|| missing("definition"))?
                .map_err(|e| format!("the app's `definition` mark is malformed: {e}"))?;
            m.definition = pair_definitions(&defs, stamps)?;
            if m.problems_open.is_none() {
                return Err(missing("problems_open"));
            }
            if m.problems_scroll.is_none() {
                return Err(missing("problems_scroll"));
            }
            let published = stamps
                .iter()
                .rfind(|s| s.event == "problems")
                .and_then(|s| s.count);
            if published != Some((PROBLEM_FILES * PROBLEMS_PER_FILE) as u64) {
                return Err(format!(
                    "the fake server published {published:?} problems, not {}",
                    PROBLEM_FILES * PROBLEMS_PER_FILE
                ));
            }
            m.problems_published = published;
            if stamps.iter().all(|s| s.event != "completion") {
                return Err("the fake server never answered a completion request — the popup did not come from it".into());
            }
        }
        "lsp-typing" | "lsp-typing-off" => {
            let typing = m.typing.as_ref().ok_or_else(|| missing("typing"))?;
            let on = scenario == "lsp-typing";
            if typing.lsp != on {
                return Err(format!("the typing mark says lsp = {}", typing.lsp));
            }
            // 켬이면 가짜가 키마다 진단을 보냈어야 한다(서버가 실제로 따라왔다). 끔이면 서버가 뜨지 않았어야 한다.
            let publishes = stamps.iter().filter(|s| s.event == "publish").count() as u64;
            if on && publishes < typing.keys {
                return Err(format!(
                    "the fake server published {publishes} times for {} keys — it did not follow the typing",
                    typing.keys
                ));
            }
            if !on && !stamps.is_empty() {
                return Err("the fake server ran in the LSP-off run".into());
            }
        }
        "symbols" | "classes" => {
            m.index.as_ref().ok_or_else(|| missing("index_ready"))?;
            let keys = m
                .symbol_keys
                .as_ref()
                .ok_or_else(|| missing("symbol_keys"))?;
            if keys.popup != scenario {
                return Err(format!(
                    "the first popup was {}, not {scenario}",
                    keys.popup
                ));
            }
            m.symbol_keys_second
                .as_ref()
                .ok_or_else(|| missing("symbol_keys_second"))?;
        }
        "lsp-idle" => {
            m.server_ready
                .as_ref()
                .ok_or_else(|| missing("server_ready"))?;
        }
        other => return Err(format!("unknown LSP scenario {other}")),
    }
    Ok(LspProgress::Done(Box::new(m)))
}

/// 진단 표본마다: 편집한 뒤 가짜가 처음으로 진단을 단(개수 > 0) 발행을 짝짓는다.
fn pair_diagnostics(samples: &[DiagSample], stamps: &[Stamp]) -> Result<Vec<Paired>, String> {
    samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // 벽시계는 ms 아래가 잘릴 수 있다 — 편집 직전 시각보다 1 ms 이른 것까지 받는다.
            let publish = stamps
                .iter()
                .find(|p| {
                    p.event == "publish"
                        && p.count.unwrap_or(0) > 0
                        && p.uri.as_deref().is_some_and(|u| u.ends_with(MAIN_PATH))
                        && p.epoch_ms >= s.edit_epoch_ms - 1.0
                })
                .ok_or_else(|| {
                    format!("diagnostics sample {i}: the fake server never published the marked line after the edit")
                })?;
            if publish.epoch_ms > s.shown_epoch_ms {
                return Err(format!(
                    "diagnostics sample {i}: the underline frame ({}) precedes the publish ({})",
                    s.shown_epoch_ms, publish.epoch_ms
                ));
            }
            Ok(Paired {
                client_ms: round(s.shown_epoch_ms - s.edit_epoch_ms),
                server_ms: round(s.shown_epoch_ms - publish.epoch_ms),
            })
        })
        .collect()
}

/// 정의 표본 k ↔ 가짜의 k 번째 정의 답(대상 파일이 같은지 본다).
fn pair_definitions(samples: &[DefSample], stamps: &[Stamp]) -> Result<Vec<Paired>, String> {
    let answers: Vec<&Stamp> = stamps.iter().filter(|s| s.event == "definition").collect();
    if answers.len() != samples.len() {
        return Err(format!(
            "{} definition samples but the fake server answered {} definition requests",
            samples.len(),
            answers.len()
        ));
    }
    samples
        .iter()
        .zip(answers)
        .enumerate()
        .map(|(i, (s, a))| {
            if !a.uri.as_deref().is_some_and(|u| u.ends_with(&s.path)) {
                return Err(format!(
                    "definition sample {i}: the app showed {} but the server answered {:?}",
                    s.path, a.uri
                ));
            }
            if a.epoch_ms > s.shown_epoch_ms || a.epoch_ms < s.ask_epoch_ms - 1.0 {
                return Err(format!(
                    "definition sample {i}: the answer ({}) is outside the request ({}) → frame ({})",
                    a.epoch_ms, s.ask_epoch_ms, s.shown_epoch_ms
                ));
            }
            Ok(Paired {
                client_ms: round(s.shown_epoch_ms - s.ask_epoch_ms),
                server_ms: round(s.shown_epoch_ms - a.epoch_ms),
            })
        })
        .collect()
}

fn round(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// 메모리를 Pitwall(앱 · WebKit)과 서버로 나눈다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct MemorySplit {
    /// `pitwall` 과 그 밖의 보조 프로세스(SandboxHelper 등 — 서버도 WebKit 도 아닌 것).
    pub app_footprint_bytes: u64,
    pub webkit_footprint_bytes: u64,
    pub server_footprint_bytes: u64,
    pub server_rss_bytes: u64,
}

/// `root` 의 자손 pid(자신은 빼고, 손자 이하 포함). `parents` 는 살아 있는 프로세스마다 (pid, 부모 pid).
///
/// 언어 서버는 앱의 책임 프로세스 그룹에 들지 않는다(2026-11-10 실측 — 그룹에는 rust-analyzer 가 없었다). 그래서 서버는 앱
/// 프로세스의 자손으로 찾는다. WebKit 의 프로세스는 launchd 의 자식이라 여기에 들지 않는다.
pub fn descendants(root: u32, parents: &[(u32, u32)]) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid) in parents {
            if ppid == parent && pid != root && found.insert(pid) {
                frontier.push(pid);
            }
        }
    }
    found
}

/// `servers`(앱 pid 의 자손 — [`descendants`])는 서버 몫, `pitwall` 은 앱, WebKit 은 WebKit, 그 밖(그룹의 보조 프로세스)은 앱 몫.
pub fn split(memory: &Memory, servers: &BTreeSet<u32>) -> MemorySplit {
    let mut s = MemorySplit::default();
    for p in &memory.processes {
        if p.name.starts_with("com.apple.WebKit") {
            s.webkit_footprint_bytes += p.footprint_bytes;
        } else if p.name != "pitwall" && servers.contains(&p.pid) {
            s.server_footprint_bytes += p.footprint_bytes;
            s.server_rss_bytes += p.rss_bytes;
        } else {
            s.app_footprint_bytes += p.footprint_bytes;
        }
    }
    s
}

/// 한 회.
#[derive(Debug, Clone, Serialize)]
pub struct LspRun {
    pub index: usize,
    pub warmup: bool,
    pub excluded: Option<String>,
    pub app_pid: u32,
    pub group: u32,
    pub cold_start_ms: f64,
    /// 띄우기 직전의 벽시계(심볼 인덱스의 첫 색인 = 이때 → 준비).
    pub launch_epoch_ms: f64,
    pub measure: LspMeasure,
    pub memory: Memory,
    pub split: MemorySplit,
    pub killed: bool,
}

/// 타이핑 키별 몫의 요약 줄 하나: 열쇠, 설명, 회의 묶음에서 값을 고르는 함수.
type BreakdownRow<'a> = (
    &'static str,
    &'static str,
    &'a dyn Fn(&TypingBreakdown) -> Option<f64>,
);

/// 요약 표의 한 줄.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub key: &'static str,
    pub item: &'static str,
    pub summary: Option<Summary>,
    pub unit: &'static str,
    pub target: Option<String>,
    pub met: Option<bool>,
}

fn median(values: &[f64]) -> Option<f64> {
    summarize(values).map(|s| s.median)
}

fn max(values: &[f64]) -> Option<f64> {
    summarize(values).map(|s| s.max)
}

/// 회의 값 하나씩 → 줄들. 판정은 회 값의 중앙값(`median`)으로 하되, 「키 입력당」 처럼 모든 키에 걸린 목표는 회 값(회의 가장 느린 키)의 최댓값으로 한다.
pub fn summarize_lsp(runs: &[LspRun]) -> Vec<Row> {
    let measured: Vec<&LspRun> = runs
        .iter()
        .filter(|r| !r.warmup && r.excluded.is_none())
        .collect();
    let per = |f: &dyn Fn(&LspRun) -> Option<f64>| {
        summarize(&measured.iter().filter_map(|r| f(r)).collect::<Vec<_>>())
    };
    let by_median = |s: Option<Summary>, target: f64| s.map(|s| s.median < target);
    let by_max = |s: Option<Summary>, target: f64| s.map(|s| s.max < target);
    let mut rows = Vec::new();
    let mut push = |key, item, summary: Option<Summary>, unit, target: Option<String>, met| {
        if summary.is_some() {
            rows.push(Row {
                key,
                item,
                summary,
                unit,
                target,
                met,
            });
        }
    };
    let ms = |v: f64| format!("< {v} ms");

    let s = per(&|r| median(&r.measure.completion_ms));
    push(
        "completion_ms",
        "completion: `.` → popup frame (per-run median of 20)",
        s,
        " ms",
        Some(ms(TARGET_COMPLETION_MS)),
        by_median(s, TARGET_COMPLETION_MS),
    );
    let s = per(&|r| max(&r.measure.completion_ms));
    push(
        "completion_max_ms",
        "completion: slowest of the run (reference)",
        s,
        " ms",
        None,
        None,
    );
    let server = |p: &[Paired]| median(&p.iter().map(|x| x.server_ms).collect::<Vec<_>>());
    let server_max = |p: &[Paired]| max(&p.iter().map(|x| x.server_ms).collect::<Vec<_>>());
    let client = |p: &[Paired]| median(&p.iter().map(|x| x.client_ms).collect::<Vec<_>>());
    let s = per(&|r| server(&r.measure.diagnostics));
    push(
        "diagnostics_ms",
        "diagnostics: server publish → underline frame (per-run median of 20)",
        s,
        " ms",
        Some(ms(TARGET_DIAGNOSTICS_MS)),
        by_median(s, TARGET_DIAGNOSTICS_MS),
    );
    let s = per(&|r| server_max(&r.measure.diagnostics));
    push(
        "diagnostics_max_ms",
        "diagnostics: publish → frame, slowest of the run (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| client(&r.measure.diagnostics));
    push(
        "diagnostics_edit_ms",
        "diagnostics: edit → underline frame, didChange included (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| server(&r.measure.definition));
    push(
        "definition_ms",
        "go to definition: server answer → target frame, another file opened (per-run median of 10)",
        s,
        " ms",
        Some(ms(TARGET_DEFINITION_MS)),
        by_median(s, TARGET_DEFINITION_MS),
    );
    let s = per(&|r| server_max(&r.measure.definition));
    push(
        "definition_max_ms",
        "go to definition: answer → frame, slowest of the run (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| client(&r.measure.definition));
    push(
        "definition_ask_ms",
        "go to definition: request → target frame (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.ready_ms);
    push(
        "lsp_ready_ms",
        "open main.rs → language server ready (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.typing.as_ref().map(|t| t.painted.p95));
    push(
        "typing_p95_ms",
        "typing: key → after the frame, per-run p95 (compare LSP on and off: on − off ≤ 2 ms)",
        s,
        " ms",
        Some(format!("on − off ≤ {TARGET_TYPING_DELTA_MS} ms")),
        None,
    );
    let s = per(&|r| r.measure.typing.as_ref().map(|t| t.painted.p50));
    push(
        "typing_p50_ms",
        "typing: key → after the frame, per-run p50 (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.typing.as_ref().map(|t| t.painted.max));
    push(
        "typing_max_ms",
        "typing: key → after the frame, slowest of the run (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.typing.as_ref().map(|t| t.frame.p95));
    push(
        "typing_frame_p95_ms",
        "typing: key → next animation frame, per-run p95 (reference)",
        s,
        " ms",
        None,
        None,
    );
    // 켬 − 끔을 키별로 가른 몫(참고 — 판정은 위의 p95 다). 회마다 그 묶음의 p95.
    let group = |r: &LspRun, pick: &dyn Fn(&TypingBreakdown) -> Option<f64>| {
        r.measure
            .typing
            .as_ref()
            .and_then(|t| t.breakdown.as_ref())
            .and_then(pick)
    };
    let p95 =
        |g: &TypingGroup, f: &dyn Fn(&TypingGroup) -> &FrameStats| (g.keys > 0).then(|| f(g).p95);
    let breakdown: [BreakdownRow<'_>; 13] = [
        (
            "typing_dispatch_p95_ms",
            "  of which `view.dispatch` (sync: state · view update, didChange send), per-run p95 (reference)",
            &|b| p95(&b.all, &|g| &g.dispatch),
        ),
        (
            "typing_state_p95_ms",
            "  of which the state update (autocomplete re-filtering), per-run p95 (reference)",
            &|b| {
                b.all
                    .state
                    .as_ref()
                    .map(|s| s.p95)
                    .filter(|_| b.all.keys > 0)
            },
        ),
        (
            "typing_layout_p95_ms",
            "  of which forced layout (`getBoundingClientRect` during the update), per-run p95 (reference)",
            &|b| {
                b.all
                    .layout
                    .as_ref()
                    .map(|s| s.p95)
                    .filter(|_| b.all.keys > 0)
            },
        ),
        (
            "typing_change_p95_ms",
            "  of which sending didChange (`lsp.document.change`, sync part), per-run p95 (reference)",
            &|b| p95(&b.all, &|g| &g.change),
        ),
        (
            "typing_wait_p95_ms",
            "  after dispatch → next animation frame, per-run p95 (reference)",
            &|b| p95(&b.all, &|g| &g.wait),
        ),
        (
            "typing_render_p95_ms",
            "  animation frame → after its rendering, per-run p95 (reference)",
            &|b| p95(&b.all, &|g| &g.render),
        ),
        (
            "typing_popup_keys",
            "typing: keys with the autocomplete popup drawn after the frame (reference)",
            &|b| Some(b.popup.keys as f64),
        ),
        (
            "typing_popup_p95_ms",
            "typing, keys with the popup: key → after the frame, per-run p95 (reference)",
            &|b| p95(&b.popup, &|g| &g.painted),
        ),
        (
            "typing_popup_dispatch_p95_ms",
            "  of which `view.dispatch`, per-run p95 (reference)",
            &|b| p95(&b.popup, &|g| &g.dispatch),
        ),
        (
            "typing_popup_state_p95_ms",
            "  of which the state update (re-filtering), per-run p95 (reference)",
            &|b| {
                b.popup
                    .state
                    .as_ref()
                    .map(|s| s.p95)
                    .filter(|_| b.popup.keys > 0)
            },
        ),
        (
            "typing_popup_layout_p95_ms",
            "  of which forced layout, per-run p95 (reference)",
            &|b| {
                b.popup
                    .layout
                    .as_ref()
                    .map(|s| s.p95)
                    .filter(|_| b.popup.keys > 0)
            },
        ),
        (
            "typing_popup_render_p95_ms",
            "  animation frame → after its rendering, per-run p95 (reference)",
            &|b| p95(&b.popup, &|g| &g.render),
        ),
        (
            "typing_no_popup_p95_ms",
            "typing, keys without the popup: key → after the frame, per-run p95 (reference)",
            &|b| p95(&b.no_popup, &|g| &g.painted),
        ),
    ];
    for (key, item, pick) in breakdown {
        let s = per(&|r| group(r, pick));
        let unit = if key == "typing_popup_keys" {
            ""
        } else {
            " ms"
        };
        push(key, item, s, unit, None, None);
    }
    let s = per(&|r| r.measure.problems_loaded_ms);
    push(
        "problems_loaded_ms",
        "problems: open the trigger → 10,000 counted in the status bar (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.problems_open.as_ref().map(|p| p.ms));
    push(
        "problems_open_ms",
        "problems: ⌘6 → first row frame (10,000 diagnostics)",
        s,
        " ms",
        Some(ms(TARGET_PROBLEMS_MS)),
        by_median(s, TARGET_PROBLEMS_MS),
    );
    let within = per(&|r| {
        r.measure
            .problems_scroll
            .as_ref()
            .and_then(|p| p.scroll.interval.within_vsync)
    });
    let vsync = per(&|r| {
        r.measure
            .problems_scroll
            .as_ref()
            .and_then(|p| p.scroll.interval.vsync_ms)
    });
    let scroll_met = within
        .zip(vsync)
        .map(|(w, v)| w.median >= TARGET_SCROLL_SHARE && v.median <= TARGET_VSYNC_MS);
    push(
        "problems_scroll_share",
        "problems fling: frames within one display refresh",
        within,
        "",
        Some(format!(
            "≥ {TARGET_SCROLL_SHARE} and median interval ≤ {TARGET_VSYNC_MS} ms"
        )),
        scroll_met,
    );
    push(
        "problems_scroll_vsync_ms",
        "problems fling: median frame interval",
        vsync,
        " ms",
        None,
        None,
    );
    let s = per(&|r| {
        r.measure
            .problems_scroll
            .as_ref()
            .map(|p| p.scroll.interval.within16_7)
    });
    push(
        "problems_scroll_16_7",
        "problems fling: frames within 16.7 ms (reference)",
        s,
        "",
        None,
        None,
    );
    let s = per(&|r| {
        r.measure
            .index
            .as_ref()
            .map(|i| i.epoch_ms - r.launch_epoch_ms)
    });
    push(
        "index_first_ms",
        "symbol index: launch → ready (first index, 20,000 files · 500,000 symbols)",
        s,
        " ms",
        Some(format!("< {TARGET_INDEX_MS} ms (8 cores)")),
        by_median(s, TARGET_INDEX_MS),
    );
    let s = per(&|r| r.measure.index.as_ref().map(|i| i.from_frame_ms));
    push(
        "index_from_frame_ms",
        "symbol index: first frame → ready (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.index.as_ref().map(|i| i.symbols as f64));
    push("index_symbols", "symbol index: symbols", s, "", None, None);
    let first = |k: &Option<SymbolKeys>| k.as_ref().map(|k| k.first.total_ms);
    let first_rpc = |k: &Option<SymbolKeys>| k.as_ref().and_then(|k| k.first.rpc_ms);
    let s = per(&|r| first(&r.measure.symbol_keys));
    push(
        "first_key_ms",
        "first popup after the window opened: first key → results frame",
        s,
        " ms",
        Some(ms(TARGET_QUERY_MS)),
        by_max(s, TARGET_QUERY_MS),
    );
    let s = per(&|r| first_rpc(&r.measure.symbol_keys));
    push(
        "first_key_rpc_ms",
        "  of which index.symbols.search round trip (Host + IPC)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.symbol_keys.as_ref().and_then(|k| k.first.seen_ms));
    push(
        "first_key_seen_ms",
        "  of which input → the rows in the DOM (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| {
        r.measure
            .symbol_keys
            .as_ref()
            .and_then(|k| Some(k.first.frame_ms? - k.first.seen_ms?))
    });
    push(
        "first_key_wait_ms",
        "  of which the rows in the DOM → next animation frame (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.symbol_keys.as_ref().and_then(|k| k.first.lsp_ms));
    push(
        "first_key_lsp_ms",
        "  lsp.workspaceSymbol round trip of the first key (reference — the rows wait for it when a server may answer)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| r.measure.symbol_keys.as_ref().map(|k| k.open_ms));
    push(
        "first_open_ms",
        "first popup: key → input shown (reference)",
        s,
        " ms",
        None,
        None,
    );
    let keys = |k: &Option<SymbolKeys>, f: &dyn Fn(&[f64]) -> Option<f64>, rpc: bool| {
        k.as_ref().and_then(|k| {
            f(&k.keys
                .iter()
                .skip(1)
                .filter_map(|t| if rpc { t.rpc_ms } else { Some(t.total_ms) })
                .collect::<Vec<_>>())
        })
    };
    let s = per(&|r| keys(&r.measure.symbol_keys, &max, false));
    push(
        "keys_max_ms",
        "first popup, keys after the first: slowest key → results frame (per run)",
        s,
        " ms",
        Some(ms(TARGET_QUERY_MS)),
        by_max(s, TARGET_QUERY_MS),
    );
    let s = per(&|r| keys(&r.measure.symbol_keys, &median, false));
    push(
        "keys_median_ms",
        "first popup, keys after the first: median key (per run)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| keys(&r.measure.symbol_keys, &max, true));
    push(
        "keys_rpc_max_ms",
        "  of which the slowest index.symbols.search round trip (per run)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| first(&r.measure.symbol_keys_second));
    push(
        "second_first_key_ms",
        "second popup (not a first use): first key → results frame",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| keys(&r.measure.symbol_keys_second, &max, false));
    push(
        "second_keys_max_ms",
        "second popup: slowest of the other keys (per run)",
        s,
        " ms",
        Some(ms(TARGET_QUERY_MS)),
        by_max(s, TARGET_QUERY_MS),
    );
    let s = per(&|r| r.measure.server_ready.as_ref().map(|s| s.from_frame_ms));
    push(
        "server_ready_ms",
        "first frame → the language server ready and quiet for 15 s (reference)",
        s,
        " ms",
        None,
        None,
    );
    let s = per(&|r| Some(r.cold_start_ms));
    push(
        "cold_start_ms",
        "cold start → first frame (reference)",
        s,
        " ms",
        None,
        None,
    );
    let mb = |b: u64| b as f64 / 1e6;
    let s = per(&|r| Some(mb(r.memory.footprint_bytes)));
    push(
        "footprint_mb",
        "footprint after the scenario, every process of the instance",
        s,
        " MB",
        None,
        None,
    );
    let s = per(&|r| Some(mb(r.split.app_footprint_bytes)));
    push(
        "app_footprint_mb",
        "  of which the app process (`pitwall` — Host, LSP client, symbol index — and the instance's helpers)",
        s,
        " MB",
        None,
        None,
    );
    let s = per(&|r| Some(mb(r.split.webkit_footprint_bytes)));
    push(
        "webkit_footprint_mb",
        "  of which WebKit",
        s,
        " MB",
        None,
        None,
    );
    let s =
        per(&|r| (r.split.server_footprint_bytes > 0).then(|| mb(r.split.server_footprint_bytes)));
    push(
        "server_footprint_mb",
        "  of which the language server's processes",
        s,
        " MB",
        None,
        None,
    );
    let s = per(&|r| (r.split.server_rss_bytes > 0).then(|| mb(r.split.server_rss_bytes)));
    push(
        "server_rss_mb",
        "language server RSS (reported apart, not a target)",
        s,
        " MB",
        None,
        None,
    );
    rows
}

#[derive(Debug, Clone, Serialize)]
pub struct LspSettings {
    pub scenario: String,
    pub runs: usize,
    pub warmup: usize,
    pub idle_secs: u64,
    pub project: Option<String>,
    pub server: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LspReport {
    pub schema: u32,
    pub scenario: String,
    pub definitions: Value,
    pub build: Build,
    pub commit: Commit,
    pub os: Os,
    pub hardware: Hardware,
    pub settings: LspSettings,
    pub runs: Vec<LspRun>,
    pub failures: Vec<RunFailure>,
    pub valid_runs: usize,
    pub rows: Vec<Row>,
    pub warning: Option<String>,
    pub stopped: Option<String>,
}

pub fn definitions() -> Value {
    DEFINITIONS
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

fn met(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "within target",
        Some(false) => "over target",
        None => "no verdict",
    }
}

/// `summary.md`.
pub fn lsp_report_markdown(report: &LspReport) -> String {
    let mut md = format!(
        "## Pitwall LSP scenario `{}` (M4-19: 06 LSP · editing rows, M4-16)\n\n",
        report.scenario
    );
    if let Some(stopped) = &report.stopped {
        let _ = write!(
            md,
            "> **Stopped early:** {stopped}. The tables hold the runs before it.\n\n"
        );
    }
    if let Some(warning) = &report.warning {
        let _ = write!(md, "> **Warning:** {warning}.\n\n");
    }
    if report.build.profile != "release" {
        let _ = write!(
            md,
            "> **Not a baseline:** measured a `{}` build. The targets are judged on release bundles.\n\n",
            report.build.profile
        );
    }
    md.push_str(&environment(
        &report.build,
        &report.commit,
        &report.os,
        &report.hardware,
    ));
    let st = &report.settings;
    let _ = writeln!(
        md,
        "\n- runs: {} measured ({} valid) + {} warm-up · per-run values, then medians over valid measured runs",
        st.runs, report.valid_runs, st.warmup
    );
    if let Some(label) = &st.label {
        let _ = writeln!(md, "- label: {label}");
    }
    if let Some(project) = &st.project {
        let _ = writeln!(md, "- project: `{project}`");
    }
    if let Some(server) = &st.server {
        let _ = writeln!(md, "- server: `{server}`");
    }
    md.push_str("\n| Item | Median (range, n) | Target | Result |\n|---|---|---|---|\n");
    for row in &report.rows {
        let Some(s) = row.summary else { continue };
        let value = format!(
            "{:.2}{} ({:.2}–{:.2}, n={})",
            s.median, row.unit, s.min, s.max, s.n
        );
        let (target, result) = match &row.target {
            Some(t) => (t.as_str(), met(row.met)),
            None => ("—", "—"),
        };
        let _ = writeln!(md, "| {} | {value} | {target} | {result} |", row.item);
    }
    md.push_str("\n| Run | Excluded | Cold start ms | Footprint MB | App MB | Server MB |\n|---|---|---|---|---|---|\n");
    for r in &report.runs {
        let _ = writeln!(
            md,
            "| {}{} | {} | {:.0} | {:.1} | {:.1} | {:.1} |",
            r.index,
            if r.warmup { " (warm-up)" } else { "" },
            r.excluded.as_deref().unwrap_or("—"),
            r.cold_start_ms,
            r.memory.footprint_bytes as f64 / 1e6,
            r.split.app_footprint_bytes as f64 / 1e6,
            r.split.server_footprint_bytes as f64 / 1e6,
        );
    }
    if !report.failures.is_empty() {
        md.push_str("\n| Failed run | Reason |\n|---|---|\n");
        for f in &report.failures {
            let _ = writeln!(
                md,
                "| {}{} | {} |",
                f.index,
                if f.warmup { " (warm-up)" } else { "" },
                f.reason.replace('|', "\\|").replace('\n', " ")
            );
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_fake;
    use crate::report::Process;

    fn mark(event: &str, pid: u32, data: Value) -> Mark {
        serde_json::from_value(json!({
            "event": event, "pid": pid, "epoch_ms": 0.0, "data": data,
        }))
        .unwrap()
    }

    fn stamp(event: &str, epoch: f64, extra: Value) -> Stamp {
        let mut v = json!({ "event": event, "epochMs": epoch });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }

    fn stats(p95: f64) -> Value {
        json!({ "n": 300, "p50": 1.0, "p95": p95, "p99": p95, "max": p95, "within16_7": 1.0, "over18": 0.0, "vsyncMs": 17.0, "withinVsync": 1.0 })
    }

    fn lsp_marks(pid: u32) -> Vec<Mark> {
        vec![
            mark("lsp_ready", pid, json!({ "ms": 300.0 })),
            mark("completion", pid, json!({ "samples": [20.0, 22.0, 30.0] })),
            mark(
                "diagnostics",
                pid,
                json!({ "samples": [
                    { "editEpochMs": 1000.0, "shownEpochMs": 1030.0 },
                    { "editEpochMs": 2000.0, "shownEpochMs": 2040.0 },
                ] }),
            ),
            mark(
                "definition",
                pid,
                json!({ "samples": [
                    { "askEpochMs": 3000.0, "shownEpochMs": 3060.0, "path": "src/t0.rs" },
                    { "askEpochMs": 4000.0, "shownEpochMs": 4080.0, "path": "src/t1.rs" },
                ] }),
            ),
            mark("problems_loaded", pid, json!({ "ms": 900.0 })),
            mark(
                "problems_open",
                pid,
                json!({ "ms": 70.0, "rows": 40, "viewport": { "width": 800.0, "height": 300.0 } }),
            ),
            mark(
                "problems_scroll",
                pid,
                json!({ "interval": stats(18.0), "work": stats(2.0), "scrollHeight": 264000.0, "viewport": { "width": 800.0, "height": 300.0 } }),
            ),
            mark("scenario_done", pid, json!({})),
        ]
    }

    fn lsp_stamps() -> Vec<Stamp> {
        vec![
            stamp(
                "publish",
                990.0,
                json!({ "uri": "file:///p/src/main.rs", "count": 0 }),
            ),
            stamp(
                "publish",
                1005.0,
                json!({ "uri": "file:///p/src/main.rs", "count": 1 }),
            ),
            stamp(
                "publish",
                1100.0,
                json!({ "uri": "file:///p/src/main.rs", "count": 0 }),
            ),
            stamp(
                "publish",
                2010.0,
                json!({ "uri": "file:///p/src/main.rs", "count": 1 }),
            ),
            stamp("completion", 500.0, json!({})),
            stamp(
                "definition",
                3010.0,
                json!({ "uri": "file:///p/src/t0.rs", "n": 0 }),
            ),
            stamp(
                "definition",
                4020.0,
                json!({ "uri": "file:///p/src/t1.rs", "n": 1 }),
            ),
            stamp("problems", 5000.0, json!({ "count": 10_000 })),
        ]
    }

    #[test]
    fn diagnostics_and_definitions_are_paired_with_the_server_stamps() {
        let LspProgress::Done(m) = progress(&lsp_marks(7), &lsp_stamps(), 7, "lsp") else {
            panic!(
                "not done: {:?}",
                progress(&lsp_marks(7), &lsp_stamps(), 7, "lsp")
            );
        };
        assert_eq!(m.completion_ms, [20.0, 22.0, 30.0]);
        assert_eq!(
            m.diagnostics,
            [
                Paired {
                    client_ms: 30.0,
                    server_ms: 25.0
                },
                Paired {
                    client_ms: 40.0,
                    server_ms: 30.0
                },
            ]
        );
        assert_eq!(
            m.definition,
            [
                Paired {
                    client_ms: 60.0,
                    server_ms: 50.0
                },
                Paired {
                    client_ms: 80.0,
                    server_ms: 60.0
                },
            ]
        );
        assert_eq!(m.problems_published, Some(10_000));
        // 다른 pid 의 표시는 읽지 않는다.
        assert_eq!(
            progress(&lsp_marks(7), &lsp_stamps(), 8, "lsp"),
            LspProgress::Waiting
        );
    }

    #[test]
    fn a_run_whose_server_did_not_do_its_part_fails() {
        let marks = lsp_marks(7);
        // 밑줄을 단 발행이 없다.
        let no_publish: Vec<Stamp> = lsp_stamps()
            .into_iter()
            .filter(|s| s.event != "publish" || s.count == Some(0))
            .collect();
        assert!(
            matches!(progress(&marks, &no_publish, 7, "lsp"), LspProgress::Failed(e) if e.contains("never published"))
        );
        // 정의 답의 파일이 앱이 보인 파일과 다르다.
        let mut swapped = lsp_stamps();
        for s in &mut swapped {
            if s.event == "definition" {
                s.uri = Some("file:///p/src/t9.rs".into());
            }
        }
        assert!(
            matches!(progress(&marks, &swapped, 7, "lsp"), LspProgress::Failed(e) if e.contains("server answered"))
        );
        // 문제 1만 개를 보내지 않았다.
        let few: Vec<Stamp> = lsp_stamps()
            .into_iter()
            .filter(|s| s.event != "problems")
            .collect();
        assert!(
            matches!(progress(&marks, &few, 7, "lsp"), LspProgress::Failed(e) if e.contains("problems"))
        );
        // 완성 답이 가짜에서 오지 않았다.
        let none: Vec<Stamp> = lsp_stamps()
            .into_iter()
            .filter(|s| s.event != "completion")
            .collect();
        assert!(
            matches!(progress(&marks, &none, 7, "lsp"), LspProgress::Failed(e) if e.contains("completion"))
        );
        // 시나리오 오류는 단계와 함께.
        let failed = vec![mark(
            "scenario_error",
            7,
            json!({ "step": "definition", "message": "timed out" }),
        )];
        assert!(
            matches!(progress(&failed, &[], 7, "lsp"), LspProgress::Failed(e) if e.contains("`definition`"))
        );
    }

    #[test]
    fn typing_runs_check_whether_the_server_followed() {
        let marks = |lsp: bool| {
            vec![
                mark(
                    "typing",
                    7,
                    json!({ "lsp": lsp, "keys": 2, "frame": stats(9.0), "painted": stats(11.0) }),
                ),
                mark("scenario_done", 7, json!({})),
            ]
        };
        let publishes = vec![
            stamp(
                "publish",
                1.0,
                json!({ "uri": "file:///p/src/main.rs", "count": 0 }),
            ),
            stamp(
                "publish",
                2.0,
                json!({ "uri": "file:///p/src/main.rs", "count": 0 }),
            ),
        ];
        assert!(matches!(
            progress(&marks(true), &publishes, 7, "lsp-typing"),
            LspProgress::Done(_)
        ));
        assert!(
            matches!(progress(&marks(true), &publishes[..1], 7, "lsp-typing"), LspProgress::Failed(e) if e.contains("did not follow"))
        );
        assert!(matches!(
            progress(&marks(false), &[], 7, "lsp-typing-off"),
            LspProgress::Done(_)
        ));
        assert!(
            matches!(progress(&marks(false), &publishes, 7, "lsp-typing-off"), LspProgress::Failed(e) if e.contains("LSP-off"))
        );
        assert!(matches!(
            progress(&marks(true), &publishes, 7, "lsp-typing-off"),
            LspProgress::Failed(_)
        ));
    }

    fn keys(popup: &str, first: f64, rest: &[f64]) -> Value {
        let key = |q: &str, t: f64| json!({ "query": q, "totalMs": t, "rpcMs": t / 2.0 });
        // 첫 키만 DOM · 그 뒤 rAF · 서버 왕복까지 적는다(옛 결과처럼 없는 칸도 읽혀야 한다 — 나머지 키).
        let first_key = json!({
            "query": "h", "totalMs": first, "seenMs": first - 12.0, "frameMs": first - 2.0,
            "rpcMs": first / 2.0, "lspMs": first / 4.0,
        });
        let mut keys = vec![first_key.clone()];
        keys.extend(rest.iter().map(|&t| key("ha", t)));
        json!({ "popup": popup, "openMs": 5.0, "first": first_key, "keys": keys })
    }

    fn symbol_run(index: usize, first: f64, rest: &[f64]) -> LspRun {
        let marks = vec![
            mark(
                "index_ready",
                7,
                json!({ "epochMs": 2500.0, "fromFrameMs": 2000.0, "files": 20000, "symbols": 500000 }),
            ),
            mark("symbol_keys", 7, keys("symbols", first, rest)),
            mark("symbol_keys_second", 7, keys("classes", 9.0, &[8.0])),
            mark("scenario_done", 7, json!({})),
        ];
        let LspProgress::Done(measure) = progress(&marks, &[], 7, "symbols") else {
            panic!("not done");
        };
        LspRun {
            index,
            warmup: false,
            excluded: None,
            app_pid: 7,
            group: 7,
            cold_start_ms: 400.0,
            launch_epoch_ms: 1000.0,
            measure: *measure,
            memory: Memory {
                footprint_bytes: 0,
                rss_bytes: 0,
                processes: Vec::new(),
            },
            split: MemorySplit::default(),
            killed: false,
        }
    }

    #[test]
    fn per_keystroke_targets_are_judged_on_the_slowest_key_and_first_index_on_the_median() {
        let runs = vec![
            symbol_run(0, 40.0, &[10.0, 12.0]),
            symbol_run(1, 20.0, &[11.0, 29.0]),
            symbol_run(2, 25.0, &[9.0, 10.0]),
        ];
        let rows = summarize_lsp(&runs);
        let row = |k: &str| rows.iter().find(|r| r.key == k).unwrap();
        // 첫 키: 한 회라도 30 ms 를 넘으면 넘는다.
        assert_eq!(row("first_key_ms").summary.unwrap().max, 40.0);
        assert_eq!(row("first_key_ms").met, Some(false));
        // 나머지 키(첫 키 제외): 가장 느린 키 29 ms — 안.
        assert_eq!(row("keys_max_ms").summary.unwrap().max, 29.0);
        assert_eq!(row("keys_max_ms").met, Some(true));
        // 첫 키의 갈래: DOM 까지 · DOM → 다음 rAF(프레임 대기) · 서버 왕복.
        assert_eq!(row("first_key_seen_ms").summary.unwrap().max, 28.0);
        assert_eq!(row("first_key_wait_ms").summary.unwrap().max, 10.0);
        assert_eq!(row("first_key_lsp_ms").summary.unwrap().max, 10.0);
        assert_eq!(row("index_first_ms").summary.unwrap().median, 1500.0);
        assert_eq!(row("index_first_ms").met, Some(true));
        // 다른 시나리오의 칸은 줄을 만들지 않는다.
        assert!(rows.iter().all(|r| r.key != "completion_ms"));
        // 준비 회 · 제외 회는 빠진다.
        let mut warm = symbol_run(3, 500.0, &[500.0]);
        warm.warmup = true;
        let mut hidden = symbol_run(4, 500.0, &[500.0]);
        hidden.excluded = Some("hidden".into());
        let rows = summarize_lsp(&[runs, vec![warm, hidden]].concat());
        assert_eq!(
            rows.iter()
                .find(|r| r.key == "first_key_ms")
                .unwrap()
                .summary
                .unwrap()
                .n,
            3
        );
    }

    #[test]
    fn typing_keys_are_split_into_popup_and_no_popup_rows() {
        let group = |keys: u64, painted: f64| {
            json!({
                "keys": keys, "painted": stats(painted), "dispatch": stats(painted / 4.0),
                "state": stats(painted / 8.0), "layout": stats(painted / 16.0),
                "change": stats(0.5), "update": stats(1.0), "raf": stats(0.3),
                "wait": stats(painted / 2.0), "render": stats(2.0),
            })
        };
        let marks = |breakdown: Option<Value>| {
            let mut typing =
                json!({ "lsp": false, "keys": 300, "frame": stats(9.0), "painted": stats(14.0) });
            let keys = breakdown.is_some().then(|| {
                mark(
                    "typing_keys",
                    7,
                    json!({ "columns": ["ch", "paintedMs"], "rows": [["i", 12.0]] }),
                )
            });
            if let Some(b) = breakdown {
                typing["breakdown"] = b;
            }
            let mut out = vec![mark("typing", 7, typing)];
            out.extend(keys);
            out.push(mark("scenario_done", 7, json!({})));
            out
        };
        let split =
            json!({ "all": group(300, 16.0), "popup": group(0, 0.0), "noPopup": group(300, 14.0) });
        let LspProgress::Done(measure) = progress(&marks(Some(split)), &[], 7, "lsp-typing-off")
        else {
            panic!("not done");
        };
        assert_eq!(measure.typing_keys.as_ref().unwrap().rows.len(), 1);
        let run = LspRun {
            index: 0,
            warmup: false,
            excluded: None,
            app_pid: 7,
            group: 7,
            cold_start_ms: 400.0,
            launch_epoch_ms: 1000.0,
            measure: *measure,
            memory: Memory {
                footprint_bytes: 0,
                rss_bytes: 0,
                processes: Vec::new(),
            },
            split: MemorySplit::default(),
            killed: false,
        };
        let rows = summarize_lsp(std::slice::from_ref(&run));
        let row = |k: &str| {
            rows.iter()
                .find(|r| r.key == k)
                .map(|r| r.summary.unwrap().max)
        };
        assert_eq!(row("typing_dispatch_p95_ms"), Some(4.0));
        assert_eq!(row("typing_change_p95_ms"), Some(0.5));
        assert_eq!(row("typing_state_p95_ms"), Some(2.0));
        assert_eq!(row("typing_layout_p95_ms"), Some(1.0));
        assert_eq!(row("typing_wait_p95_ms"), Some(8.0));
        assert_eq!(row("typing_popup_keys"), Some(0.0));
        assert_eq!(row("typing_no_popup_p95_ms"), Some(14.0));
        // 팝업 키가 없는 회(끔)는 팝업 묶음의 줄을 만들지 않는다.
        assert_eq!(row("typing_popup_p95_ms"), None);
        // 키별 몫을 싣기 전의 결과도 읽힌다 — 그 줄들이 없을 뿐이다.
        let LspProgress::Done(old) = progress(&marks(None), &[], 7, "lsp-typing-off") else {
            panic!("not done");
        };
        assert!(old.typing.unwrap().breakdown.is_none());
    }

    #[test]
    fn memory_is_split_between_pitwall_and_the_server() {
        let p = |pid, name: &str, fp| Process {
            pid,
            name: name.into(),
            footprint_bytes: fp,
            rss_bytes: fp * 2,
        };
        let memory = Memory {
            footprint_bytes: 0,
            rss_bytes: 0,
            processes: vec![
                p(1, "pitwall", 60),
                p(2, "com.apple.WebKit.WebContent", 150),
                p(3, "com.apple.WebKit.GPU", 20),
                p(4, "rust-analyzer", 900),
                p(5, "rust-analyzer-proc-macro-srv", 40),
                // 앱의 책임 그룹에 든 macOS 보조 프로세스 — 서버가 아니다(2026-11-10 의 실측이 이것을 서버로 셌다).
                p(6, "com.apple.audio.SandboxHelper", 5),
                p(7, "SetStoreUpdateService", 2),
            ],
        };
        let servers = BTreeSet::from([4, 5]);
        assert_eq!(
            split(&memory, &servers),
            MemorySplit {
                app_footprint_bytes: 67,
                webkit_footprint_bytes: 170,
                server_footprint_bytes: 940,
                server_rss_bytes: 1880,
            }
        );
    }

    #[test]
    fn the_servers_are_every_descendant_of_the_app_process() {
        // 100 = 앱. 4 = 서버, 5 = 서버의 자식(proc-macro 서버), 9 = 서버의 손자. 2 · 3 = WebKit(launchd 1 의 자식), 50 = 남.
        let parents = [
            (100, 1),
            (2, 1),
            (3, 1),
            (4, 100),
            (5, 4),
            (9, 5),
            (50, 1),
            (51, 50),
        ];
        assert_eq!(descendants(100, &parents), BTreeSet::from([4, 5, 9]));
        assert_eq!(descendants(4, &parents), BTreeSet::from([5, 9]));
        assert!(descendants(77, &parents).is_empty());
    }

    #[test]
    fn the_fixture_has_what_the_app_scenario_looks_for() {
        // 공백 · 비 ASCII 가 든 폴더 — 가짜가 답하는 URI 가 퍼센트 인코딩되는지 본다.
        let dir =
            std::env::temp_dir().join(format!("pw-perf 한글 lsp-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let fake = Path::new("/opt/pw perf/pw-perf-lsp");
        write_fixture(&project, &dir, &dir.join("config"), Some(fake)).unwrap();
        let main = std::fs::read_to_string(project.join(MAIN_PATH)).unwrap();
        let line = |n: usize| main.lines().nth(n - 1).unwrap().to_owned();
        assert_eq!(line(1), MAIN_FIRST_LINE);
        assert!(line(DEF_LINE).contains(DEF_WORD));
        assert!(
            !main.contains(lsp_fake::DIAG_MARK),
            "the file starts without diagnostics"
        );
        let target = std::fs::read_to_string(project.join("src/t3.rs")).unwrap();
        assert_eq!(
            target.lines().next(),
            Some(format!("{TARGET_FIRST_LINE} 3").as_str())
        );
        assert_eq!(
            target.lines().nth(DEF_TARGET_LINE - 1),
            Some("pub fn target_fn() {}")
        );
        assert!(project.join("src/p/f1999.rs").is_file());
        let settings = std::fs::read_to_string(dir.join("config/settings.toml")).unwrap();
        assert!(
            settings.contains("[lsp.rust-analyzer]\npath = \"/opt/pw perf/pw-perf-lsp\""),
            "{settings}"
        );
        let config: Config =
            serde_json::from_slice(&std::fs::read(dir.join(FAKE_CONFIG_FILE)).unwrap()).unwrap();
        assert_eq!(config.definition_line as usize, DEF_TARGET_LINE - 1);
        let target = &config.definition_targets[0];
        assert!(target.ends_with("/project/src/t0.rs"), "{target}");
        assert!(!target.contains(' ') && target.is_ascii(), "{target}");
        assert!(
            target.contains("/pw-perf%20%ED%95%9C%EA%B8%80%20lsp-fixture-"),
            "{target}"
        );
        assert_eq!(
            url::Url::parse(target).unwrap().to_file_path(),
            Ok(project.join("src").join("t0.rs")),
        );
        assert_eq!(config.problems.as_ref().unwrap().files.len(), PROBLEM_FILES);
        write_fixture(&project, &dir, &dir.join("config"), None).unwrap();
        let off = std::fs::read_to_string(dir.join("config/settings.toml")).unwrap();
        assert!(
            off.contains("enabled = false") && !off.contains("path"),
            "{off}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 앱 시나리오(perf-scenario-lsp.ts)와 이 모듈 · 가짜가 같은 값을 쓴다.
    #[test]
    fn the_constants_match_the_app_scenario() {
        let ts = include_str!("../../../apps/desktop/src/perf-scenario-lsp.ts");
        let has = |name: &str, value: &str| {
            assert!(
                ts.contains(&format!("export const {name} = {value};")),
                "{name} = {value} in perf-scenario-lsp.ts"
            );
        };
        let quoted = |v: &str| format!("\"{v}\"");
        has("MAIN_PATH", &quoted(MAIN_PATH));
        has("MAIN_FIRST_LINE", &quoted(MAIN_FIRST_LINE));
        has("PROBLEMS_PATH", &quoted(PROBLEMS_PATH));
        has("COMPLETION_LINE", &COMPLETION_LINE.to_string());
        has("DIAG_LINE", &DIAG_LINE.to_string());
        has("DEF_LINE", &DEF_LINE.to_string());
        has("TYPING_LINE", &TYPING_LINE.to_string());
        has("DEF_WORD", &quoted(DEF_WORD));
        has("DIAG_MARK", &quoted(lsp_fake::DIAG_MARK));
        has("COMPLETION_LABEL", &quoted(lsp_fake::COMPLETION_LABEL));
        has("TARGET_FIRST_LINE", &quoted(TARGET_FIRST_LINE));
        has("DEF_TARGET_LINE", &DEF_TARGET_LINE.to_string());
        has("DEF_TARGETS", &DEF_TARGETS.to_string());
        has("PROBLEM_FILES", "2_000");
        has("PROBLEMS_PER_FILE", &PROBLEMS_PER_FILE.to_string());
        has("IDLE_PATH", &quoted(IDLE_PATH));
        assert_eq!(PROBLEM_FILES, 2_000);
        for scenario in SCENARIOS {
            assert!(ts.contains(&quoted(scenario)), "{scenario}");
        }
    }
}
