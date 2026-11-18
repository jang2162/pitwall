//! 태스크 감지(17-m6-plan M6-04 · M6-05, F7-2): 프로젝트 루트의 파일에서 「감지된 태스크」 — 파일 없는 가상 실행 구성 — 를
//! 찾는다. `run.config.list` 의 `detected` 가 이것이고, 「구성으로 저장」 은 그 `config` 를 `run.config.save`(M6-01 의 쓰기)로 쓴다.
//!
//! - **감지기 목록**은 [`DETECTORS`] 하나다. M6-04 는 파일만 읽는 감지기(`package.json` scripts · `Makefile` 대상 · `justfile`
//!   레시피 · compose 서비스)를 두고, M6-05 가 프로세스를 띄우는 감지기(mise · gradle · maven)를 같은 목록에 더한다.
//! - **프로세스는 [`DetectExec`] 로만 띄운다.** 감지기는 `std::process` 를 직접 부르지 않는다 — 그래서 미신뢰 프로젝트에서 무엇이
//!   도는지가 그 실행기 하나로 정해진다(M6-05 가 신뢰 판정을 거기에 건다). 파일만 읽는 감지기는 미신뢰 프로젝트에서도 보인다
//!   (메인 판정 M6-P6 — 실행은 M6-02 의 신뢰 판정을 지난다).
//! - **캐시하지 않는다.** 목록을 부를 때마다 루트의 파일 몇 개를 읽는다(각 1 MiB 까지). 실행(M6-02) · 「사람이 본 구성」 해시
//!   (M6-11)도 그때 다시 감지한다 — 오래된 감지 결과의 정의 본문으로 해시하면 바뀐 명령이 승인 없이 돈다.
//! - **파일 감시**([`follow_files`]): 감지 출처 파일([`TaskDetector::sources`])이 바뀌면 그 프로젝트의 감지만 다시 하고, 결과가
//!   달라졌으면 `config.changed { changed: ["run/detected"] }`(프로젝트 범위)를 낸다 — 창이 목록을 다시 부른다.
//! - **이름이 정의다**: 감지된 태스크의 열쇠는 `detected:<감지기>:<이름>`([`key`])이고, 「사람이 본 구성」 해시(M6-11)는
//!   [`DetectedTask::definition`] — 감지 출처에서 그 태스크를 정의한 본문 — 을 넣는다. `npm` 종류의 저장된 구성은
//!   [`npm::script_definition`] 이 같은 본문을 준다.
//! - **읽기의 경계**([`file`]): 루트의 정규 파일만, 1 MiB 까지, 루트 밖을 가리키는 링크는 읽지 않는다. 감지기 하나가 실패하면 그
//!   감지기만 비우고 까닭을 [`DetectProblem`] 으로 남긴다. 까닭에는 파일 내용을 싣지 않는다(줄 번호 · 개수만).
//! - **증폭 막기**: 1 MiB 파일 하나라도 정의 본문은 곱으로 불어날 수 있다(한 규칙 줄의 대상 수천 × 긴 레시피, 한 앵커를 가리키는
//!   서비스 수천). 그래서 감지기는 이름을 먼저 고르고([`pick_names`] — 이름순 [`MAX_TASKS`] 개, [`MAX_NAME_LEN`] 까지) 고른 것만
//!   정의를 만들며, 만드는 **도중에** 정의 합계를 [`Budget`]([`MAX_DEFINITION_BYTES`])으로 센다 — 넘는 순간 그 감지기를 까닭으로
//!   비운다. 다 만든 뒤 자르지 않는다.

pub mod cache;
pub mod compose;
pub mod exec;
pub mod file;
pub mod gradle;
pub mod just;
pub mod make;
pub mod maven;
pub mod mise;
pub mod npm;
pub mod refresh;
pub mod watch;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{RunConfig, validate};

pub use watch::{Seen, follow_files, subscribe_files};

/// 감지된 태스크 열쇠의 머리.
pub const KEY_PREFIX: &str = "detected:";

/// 감지기 하나가 내는 태스크 수의 상한. 넘으면 이름순 앞의 것만 두고 까닭을 남긴다(커밋된 `package.json` 하나로 목록 · 선택기가
/// 무거워지지 않게). 감지기는 정의를 만들기 전에 이것으로 자른다([`pick_names`]).
pub const MAX_TASKS: usize = 500;

/// 감지된 태스크 이름의 길이 상한(바이트). 넘는 이름은 빼고 까닭을 남긴다 — 열쇠(`detected:<감지기>:<이름>`)를 기록하는 쪽(M6-11)도
/// 길이 상한이 있다.
pub const MAX_NAME_LEN: usize = 200;

/// 감지기 하나가 만드는 정의 본문의 합계 상한(4 MiB). 만드는 도중에 센다([`Budget`]).
pub const MAX_DEFINITION_BYTES: usize = 4 * 1024 * 1024;

/// 감지된 태스크의 열쇠 `detected:<감지기>:<이름>` — 「사람이 본 구성」 기록(M6-11)의 구성 열쇠. 감지기 id 에는 `:` 이 없으므로
/// 이름에 `:` 이 있어도(`build:prod`) 첫 두 `:` 로 가른다([`parse_key`]).
pub fn key(detector: &str, name: &str) -> String {
    format!("{KEY_PREFIX}{detector}:{name}")
}

/// [`key`] 의 반대 — (감지기, 이름). 감지된 태스크의 열쇠가 아니면 `None`.
pub fn parse_key(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix(KEY_PREFIX)?.split_once(':')
}

/// 감지기가 프로세스를 띄우는 유일한 길(M6-05 의 mise · gradle · maven). 파일만 읽는 감지기는 부르지 않는다 — 시험이 가짜 실행기의
/// 불린 수로 본다.
pub trait DetectExec: Send + Sync {
    /// `argv` 를 `cwd` 에서 돌려 표준 출력을 준다. 실패 · 거절은 까닭.
    fn output(&self, detector: &str, argv: &[String], cwd: &Path) -> Result<String, String>;

    /// 누를 때만 읽는 태스크 목록(gradle · maven)의 캐시 폴더(`$DATA/projects/<id>/` — [`cache`]). 없으면 그 목록은 늘 「읽기 전」 이다.
    fn cache_dir(&self) -> Option<&Path> {
        None
    }
}

/// 아무것도 띄우지 않는 실행기 — 부르면 거절이다. 프로세스를 띄우는 감지기가 없는 동안(M6-05 전)의 기본값.
pub struct NoExec;

impl DetectExec for NoExec {
    fn output(&self, detector: &str, _argv: &[String], _cwd: &Path) -> Result<String, String> {
        Err(format!(
            "the `{detector}` detector may not start processes here"
        ))
    }
}

/// 감지기 하나가 보는 자리.
pub struct DetectContext<'a> {
    /// 프로젝트 루트(활성 워크트리 안).
    pub root: &'a Path,
    /// 루트에 있는 항목의 이름(정확한 대소문자 — 대소문자를 가리지 않는 파일 시스템에서 `makefile` 로 `Makefile` 을 찾지 않게).
    pub entries: &'a BTreeSet<String>,
    pub exec: &'a dyn DetectExec,
}

/// 감지기 하나가 찾은 것.
#[derive(Debug, Default)]
pub struct Found {
    pub tasks: Vec<DetectedTask>,
    /// 감지에서 뺀 태스크의 까닭(이름이 셸에 안전하지 않다 등). 파일 내용을 싣지 않는다.
    pub skipped: Vec<String>,
    /// 누를 때만 읽는 목록(gradle · maven)의 상태 — 그 감지기만 채운다. 창(M6-07)이 「태스크 읽기」 · 「다시 읽기」 를 보인다.
    pub list: Option<TaskListState>,
}

/// 누를 때만 읽는 태스크 목록의 상태(결정 M6-D8 · D70).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum TaskListState {
    /// 아직 읽지 않았다 — 흔한 태스크만 보인다.
    NotRead,
    /// 읽은 목록이 보인다(빌드 파일이 그때와 같다).
    Read,
    /// 읽은 뒤 빌드 파일 · 런타임 원천 파일이 바뀌었다 — 흔한 태스크만 보이고 다시 읽어야 한다.
    Stale,
}

/// 감지기 하나의 누를 때만 읽는 목록(`run.config.list` 의 `taskLists`). 사람의 누름은 `run.tasks.refresh { detector }`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    pub detector: String,
    pub state: TaskListState,
}

/// 감지기. [`DETECTORS`] 에 하나씩 둔다.
pub trait TaskDetector: Sync {
    /// 감지기 id — 열쇠의 가운데(`detected:<id>:<이름>`), 목록의 `detector`.
    fn id(&self) -> &'static str;
    /// 감지 출처가 될 수 있는 루트의 파일 이름. 파일 감시가 이 이름이 바뀌면 다시 감지한다.
    fn sources(&self) -> &'static [&'static str];
    /// 파일 감시: 이 경로가 바뀌면 다시 감지해 볼 만한가. 기본은 파일 이름이 [`Self::sources`] 에 있는가(어느 깊이든).
    fn watches(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| self.sources().contains(&name))
    }
    /// 감지. 출처 파일이 없으면 빈 [`Found`]. 실패(읽을 수 없음 · 형식 오류)는 까닭 — 그 감지기만 빈다.
    fn detect(&self, cx: &DetectContext<'_>) -> Result<Found, String>;
}

/// 감지기 목록 — 목록의 순서가 감지 결과의 순서다. 앞의 넷은 파일만 읽고(M6-04), 뒤의 셋은 프로세스를 띄운다(M6-05 — mise 는
/// 신뢰한 프로젝트에서 목록마다, gradle · maven 은 사람이 「태스크 읽기」 를 누를 때만).
pub static DETECTORS: &[&dyn TaskDetector] = &[
    &npm::NpmDetector,
    &make::MakeDetector,
    &just::JustDetector,
    &compose::ComposeDetector,
    &mise::MiseDetector,
    &gradle::GradleDetector,
    &maven::MavenDetector,
];

/// 감지된 태스크 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedTask {
    /// 감지기 id.
    pub detector: &'static str,
    /// 출처 안의 이름(script · 대상 · 레시피 · 서비스).
    pub name: String,
    /// 감지 출처 파일.
    pub source: PathBuf,
    /// 가상 구성 — 「구성으로 저장」 하면 이것이 파일이 된다. [`validate`] 를 지난 것만 있다.
    pub config: RunConfig,
    /// 명령 정의 본문 — 「사람이 본 구성」 해시(M6-11)가 넣는다. 감지기마다 다르다: `package.json` script 는 그 script 와 npm 이
    /// 함께 돌리는 `pre` · `post` script 의 본문, `Makefile` · `justfile` 은 **출처 파일 전체**의 sha256 과 그 이름([`file_definition`]
    /// — 선행 대상 · `SHELL` · `set shell` 처럼 그 태스크의 실행을 바꾸는 것이 같은 파일의 다른 자리에 있다), compose 는 그 서비스의
    /// 값(별칭을 펼친 것). 기록 · 로그에 쓰지 않는다(env 값이 있을 수 있다).
    pub definition: String,
}

impl DetectedTask {
    /// `detected:<감지기>:<이름>`.
    pub fn key(&self) -> String {
        key(self.detector, &self.name)
    }
}

/// 감지기 하나의 실패 · 뺀 태스크의 까닭. 목록(`run.config.list` 의 `detectProblems`)에 남는다 — 감지 목록이 조용히 짧아지지 않게.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DetectProblem {
    /// 감지기 id(`npm` · `make` · `just` · `compose` …).
    pub detector: String,
    /// 까닭(어느 파일 · 무엇 · 몇째 줄). 파일 내용은 담지 않는다.
    pub reason: String,
}

/// 프로젝트 하나의 감지 결과.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    /// [`DETECTORS`] 순서, 감지기 안에서는 이름순.
    pub tasks: Vec<DetectedTask>,
    pub problems: Vec<DetectProblem>,
    /// 누를 때만 읽는 목록의 상태(감지기 순서).
    pub lists: Vec<TaskList>,
}

impl Detection {
    /// 열쇠(`detected:<감지기>:<이름>`)로 찾는다.
    pub fn get(&self, key: &str) -> Option<&DetectedTask> {
        let (detector, name) = parse_key(key)?;
        self.tasks
            .iter()
            .find(|t| t.detector == detector && t.name == name)
    }
}

/// `root` 의 태스크를 모두 감지한다. 디스크에 쓰지 않는다. 프로세스는 `exec` 로만 띄운다(M6-04 의 감지기는 띄우지 않는다).
pub fn detect(root: &Path, exec: &dyn DetectExec) -> Detection {
    detect_with(DETECTORS, root, exec)
}

/// [`detect`] 를 주어진 감지기들로.
pub fn detect_with(
    detectors: &[&dyn TaskDetector],
    root: &Path,
    exec: &dyn DetectExec,
) -> Detection {
    let mut out = Detection::default();
    let entries = match file::entries(root) {
        Ok(entries) => entries,
        Err(reason) => {
            for detector in detectors {
                out.problems.push(DetectProblem {
                    detector: detector.id().to_owned(),
                    reason: reason.clone(),
                });
            }
            return out;
        }
    };
    let cx = DetectContext {
        root,
        entries: &entries,
        exec,
    };
    for detector in detectors {
        let problem = |reason: String| DetectProblem {
            detector: detector.id().to_owned(),
            reason,
        };
        let found = match detector.detect(&cx) {
            Ok(found) => found,
            Err(reason) => {
                out.problems.push(problem(reason));
                continue;
            }
        };
        let mut tasks = Vec::new();
        let mut invalid = 0;
        for task in found.tasks {
            // 감지된 구성도 저장된 구성과 같은 검증을 지난다 — `-` 로 시작하는 script 이름 등은 실행할 수 없다.
            if validate(&task.config, root).is_ok() {
                tasks.push(task);
            } else {
                invalid += 1;
            }
        }
        out.problems.extend(found.skipped.into_iter().map(problem));
        if let Some(state) = found.list {
            out.lists.push(TaskList {
                detector: detector.id().to_owned(),
                state,
            });
        }
        if invalid > 0 {
            out.problems.push(problem(format!(
                "skipped {invalid} task(s) that cannot be run as a configuration (a name starting with `-` and the like)"
            )));
        }
        tasks.sort_by(|a, b| a.name.cmp(&b.name));
        if tasks.len() > MAX_TASKS {
            out.problems.push(problem(format!(
                "found {} tasks; only the first {MAX_TASKS} by name are listed",
                tasks.len()
            )));
            tasks.truncate(MAX_TASKS);
        }
        out.tasks.extend(tasks);
    }
    out
}

/// 누를 때만 읽는 목록(gradle · maven)을 캐시에서 — 지금 빌드 파일의 열쇠로 읽은 목록이 있으면 그것, 없으면 `None`(부른 쪽이 정적
/// 목록을 쓴다). 상태는 `found.list` 에, 열쇠를 낼 수 없는 까닭은 `found.skipped` 에 남긴다. 프로세스를 띄우지 않는다.
pub(crate) fn pressed_list(
    cx: &DetectContext<'_>,
    detector: &str,
    kind: cache::BuildFiles,
    found: &mut Found,
) -> Option<Vec<String>> {
    let lookup = match cx.exec.cache_dir() {
        None => Ok(cache::Lookup::Missing),
        Some(dir) => cache::lookup(dir, detector, || cache::key(cx.root, kind)),
    };
    let (state, names) = match lookup {
        Ok(cache::Lookup::Hit(names)) => (TaskListState::Read, Some(names)),
        Ok(cache::Lookup::Stale) => (TaskListState::Stale, None),
        Ok(cache::Lookup::Missing) => (TaskListState::NotRead, None),
        Err(reason) => {
            found.skipped.push(reason);
            (TaskListState::Stale, None)
        }
    };
    found.list = Some(state);
    names
}

/// 정의 본문의 합계 예산 — 감지기 하나가 정의를 만드는 동안 쓴 바이트를 센다.
#[derive(Debug)]
pub struct Budget {
    left: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self::new(MAX_DEFINITION_BYTES)
    }
}

impl Budget {
    pub fn new(bytes: usize) -> Self {
        Self { left: bytes }
    }

    /// 남은 바이트.
    pub fn left(&self) -> usize {
        self.left
    }

    /// `bytes` 를 쓴다. 남은 것보다 많으면 까닭(그 감지기를 비운다).
    pub fn take(&mut self, bytes: usize) -> Result<(), String> {
        self.left = self.left.checked_sub(bytes).ok_or_else(over_budget)?;
        Ok(())
    }
}

/// 예산을 넘었을 때의 까닭.
pub(crate) fn over_budget() -> String {
    format!(
        "the task definitions add up to more than {} KiB (a name repeated many times or aliases); Pitwall stops reading the file",
        MAX_DEFINITION_BYTES / 1024
    )
}

/// 정의를 만들 이름을 고른다 — 이름순 · 중복 없이 · [`MAX_NAME_LEN`] 까지 · 앞의 [`MAX_TASKS`] 개. 뺀 까닭은 `skipped` 에 더한다
/// (`source` 는 출처 파일 이름). 정의를 만들기 **전에** 부른다.
pub fn pick_names(
    source: &str,
    names: impl IntoIterator<Item = String>,
    skipped: &mut Vec<String>,
) -> Vec<String> {
    let mut long = 0;
    let all: BTreeSet<String> = names
        .into_iter()
        .filter(|n| {
            let ok = n.len() <= MAX_NAME_LEN;
            long += usize::from(!ok);
            ok
        })
        .collect();
    if long > 0 {
        skipped.push(format!(
            "{source}: skipped {long} task(s) whose names are longer than {MAX_NAME_LEN} bytes"
        ));
    }
    if all.len() > MAX_TASKS {
        skipped.push(format!(
            "{source}: found {} tasks; only the first {MAX_TASKS} by name are listed",
            all.len()
        ));
    }
    all.into_iter().take(MAX_TASKS).collect()
}

/// `Makefile` · `justfile` 의 정의 본문: `sha256:<파일 전체의 hex> <이름>`. 파일의 어느 자리를 고쳐도 그 파일의 모든 태스크가
/// 달라진다(선행 대상의 레시피 · 변수 · `SHELL` · `set shell` 이 그 태스크의 실행을 바꾼다 — 리뷰 「고쳐야 하는 것」 2).
pub fn file_definition(file_sha256: &str, name: &str) -> String {
    format!("sha256:{file_sha256} {name}")
}

/// 글의 sha256(hex).
pub(crate) fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 셸 종류(`shell`)로 감지한 태스크의 이름이 셸 글 안에 안전한가: 영문자 · 숫자 · `_` 로 시작하고 영문자 · 숫자 · `_` · `-` · `.` ·
/// `+` · `/` 만. 그 밖의 이름은 감지에서 빼고 까닭을 남긴다 — 따옴표 · `$` · 공백 · `;` 를 셸 글로 옮기는 규칙은 셸마다(로그인
/// 셸 · PowerShell) 다르다.
pub fn shell_safe_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_NAME_LEN
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+' | b'/'))
}

/// 셸 종류의 명령 글 `<program> '<name>'`. 이름은 [`shell_safe_name`] 을 지난 것이고, 홑따옴표로 감싼다 — POSIX 셸과
/// PowerShell 이 모두 안의 글자를 그대로 한 인자로 읽는다(안에 `'` 가 없으므로 이스케이프가 필요 없다).
pub(crate) fn shell_command(program: &str, name: &str) -> String {
    debug_assert!(shell_safe_name(name));
    format!("{program} '{name}'")
}

/// 줄 번호 목록을 짧게(`3, 9, 14 …`).
pub(crate) fn lines_list(lines: &[usize]) -> String {
    const SHOWN: usize = 10;
    let mut out: Vec<String> = lines.iter().take(SHOWN).map(usize::to_string).collect();
    if lines.len() > SHOWN {
        out.push("…".to_owned());
    }
    out.join(", ")
}

#[cfg(test)]
mod process_tests;
#[cfg(test)]
mod tests;
