//! mise tasks(17-m6-plan M6-05 ①): `mise tasks ls --json` 을 프로젝트 루트에서 돌려 태스크마다 종류 `mise-task`(`mise run <task>`)의
//! 가상 구성을 만든다. 프로젝트 코드를 돌리는 감지라 **신뢰한 프로젝트에서만** 돈다 — 실행기([`super::exec::TrustedExec`])가 판정하고,
//! 미신뢰면 띄우지 않고 까닭이 목록에 남는다.
//!
//! - **언제 띄우는가**: 루트와 그 위 폴더(홈 앞까지)에 mise 설정 파일이 있거나(`pw_runtime::mise::config_files`) 루트에 파일 태스크
//!   폴더([`TASK_DIRS`])가 있을 때만. 없으면 아무것도 띄우지 않는다. 캐시하지 않는다 — 목록을 부를 때마다 새로 읽는다(버전 파일이
//!   바뀌면 파일 감시가 감지를 다시 해 창에 알린다 — [`TaskDetector::watches`]).
//! - **출력 꼴**: 고정한 mise 두 판(2025.1.0 · 2026.4.3)의 실제 출력 픽스처(`fixtures/mise-tasks/`)로 시험한다. 쓰는 칸은 `name` ·
//!   `source` · `file`(2026) · `hide` 뿐이고 모르는 칸은 무시한다. mise 가 설정을 신뢰하지 않으면 2026 은 실패로 끝나고 2025 는 경고만
//!   찍고 파일 태스크만 낸다 — 둘 다 stderr 의 문장([`not_trusted_by_mise`])으로 가려 이 감지기를 비운다(반쪽 목록을 보이지 않는다).
//! - **정의 본문**(「사람이 본 구성」 — M6-11): 그 태스크를 정의한 파일(파일 태스크는 그 파일, TOML 태스크는 `source` 의 설정 파일)
//!   **전체**의 sha256 과 이름([`super::file_definition`]). `depends` · `env` · `dir` 같은 정의도 그 파일에 있어 함께 덮인다. 파일
//!   하나는 [`MAX_SOURCE`] 까지, 감지 한 번에 읽는 합계는 [`MAX_SOURCES_TOTAL`] 까지(파일 태스크 수백 개 × 큰 파일의 증폭을 막는다) —
//!   넘으면 이 감지기를 까닭으로 비운다. 같은 파일은 한 번만 읽는다.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{DetectContext, DetectedTask, Found, TaskDetector, file_definition, pick_names};
use crate::config::{RunConfig, RunKindConfig};

pub const ID: &str = "mise";

/// 파일 태스크 폴더(루트 기준 — mise 의 「File Tasks」 문서).
pub const TASK_DIRS: [&str; 5] = [
    "mise-tasks",
    ".mise-tasks",
    "mise/tasks",
    ".mise/tasks",
    ".config/mise/tasks",
];

/// 정의 본문을 위해 읽는 파일 하나의 상한.
pub const MAX_SOURCE: u64 = 1024 * 1024;
/// 감지 한 번에 정의 본문을 위해 읽는 파일 합계의 상한.
pub const MAX_SOURCES_TOTAL: u64 = 16 * 1024 * 1024;

/// mise 가 이 폴더의 설정을 신뢰하지 않을 때 stderr 에 찍는 문장의 일부(두 판 픽스처 — `pw_runtime::mise` 와 같은 문장).
const NOT_TRUSTED: &str = "are not trusted";

/// mise 가 설정을 신뢰하지 않을 때의 까닭.
pub const NOT_TRUSTED_REASON: &str =
    "mise does not trust this project's config; run `mise trust` in the project to list its tasks";

/// stderr 가 「mise 가 설정을 신뢰하지 않음」 인가.
pub fn not_trusted_by_mise(stderr: &str) -> bool {
    stderr.contains(NOT_TRUSTED)
}

/// `mise tasks ls --json` 의 항목 하나(쓰는 칸만).
#[derive(Debug, Deserialize)]
struct Listed {
    name: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    hide: bool,
}

pub struct MiseDetector;

impl TaskDetector for MiseDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn sources(&self) -> &'static [&'static str] {
        &[
            "mise.toml",
            ".mise.toml",
            "mise.local.toml",
            ".mise.local.toml",
            ".tool-versions",
        ]
    }

    /// mise 의 설정 · 버전 파일(`pw_runtime::mise::is_runtime_file`)과 파일 태스크 폴더 안의 파일.
    fn watches(&self, path: &Path) -> bool {
        pw_runtime::mise::is_runtime_file(path) || in_task_dir(path)
    }

    fn detect(&self, cx: &DetectContext<'_>) -> Result<Found, String> {
        if !uses_mise(cx.root) {
            return Ok(Found::default());
        }
        let argv: Vec<String> = ["mise", "tasks", "ls", "--json"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let out = cx.exec.output(ID, &argv, cx.root)?;
        tasks(&out)
    }
}

/// 루트와 그 위(홈 앞까지)에 mise 설정이 있거나 루트에 파일 태스크 폴더가 있는가 — 없으면 mise 를 띄우지 않는다.
fn uses_mise(root: &Path) -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    !pw_runtime::mise::config_files(root, home.as_deref())
        .0
        .is_empty()
        || TASK_DIRS.iter().any(|d| root.join(d).is_dir())
}

fn in_task_dir(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('\\', "/");
    TASK_DIRS
        .iter()
        .any(|d| text.contains(&format!("/{d}/")) || text.starts_with(&format!("{d}/")))
}

/// `mise tasks ls --json` 의 출력 → 감지된 태스크. 숨은 태스크(`hide`)는 뺀다.
pub fn tasks(stdout: &str) -> Result<Found, String> {
    let listed: Vec<Listed> = serde_json::from_str(stdout).map_err(|e| {
        format!(
            "`mise tasks ls --json` printed an output Pitwall does not know (line {}, column {})",
            e.line(),
            e.column()
        )
    })?;
    let mut found = Found::default();
    let mut by_name: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut unsourced = 0;
    for task in listed.into_iter().filter(|t| !t.hide) {
        match task.file.or(task.source) {
            Some(path) if Path::new(&path).is_absolute() => {
                by_name
                    .entry(task.name)
                    .or_insert_with(|| PathBuf::from(path));
            }
            _ => unsourced += 1,
        }
    }
    if unsourced > 0 {
        found.skipped.push(format!(
            "skipped {unsourced} mise task(s) without the file that defines them"
        ));
    }
    let names = pick_names("mise tasks", by_name.keys().cloned(), &mut found.skipped);
    let mut digests: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut read_total = 0u64;
    for name in names {
        let source = by_name[&name].clone();
        let digest = match digests.get(&source) {
            Some(d) => d.clone(),
            None => {
                let d = digest(&source, &mut read_total)?;
                digests.insert(source.clone(), d.clone());
                d
            }
        };
        found.tasks.push(DetectedTask {
            detector: ID,
            name: name.clone(),
            source,
            config: RunConfig {
                name: name.clone(),
                kind: RunKindConfig::MiseTask { task: name.clone() },
                cwd: None,
                env: Default::default(),
                env_file: Vec::new(),
                profiles: Default::default(),
                debug: None,
            },
            definition: file_definition(&digest, &name),
        });
    }
    Ok(found)
}

/// 정의한 파일의 sha256 — 정규 파일만, [`MAX_SOURCE`] 까지, 합계 [`MAX_SOURCES_TOTAL`] 까지. 까닭에 내용을 싣지 않는다.
fn digest(path: &Path, read_total: &mut u64) -> Result<String, String> {
    let shown = path.display();
    let meta = fs::metadata(path).map_err(|e| format!("mise task file {shown}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("mise task file {shown} is not a regular file"));
    }
    if meta.len() > MAX_SOURCE {
        return Err(format!(
            "mise task file {shown} is larger than {} KiB; Pitwall does not read it",
            MAX_SOURCE / 1024
        ));
    }
    *read_total += meta.len();
    if *read_total > MAX_SOURCES_TOTAL {
        return Err(format!(
            "the files defining mise tasks add up to more than {} MiB; Pitwall stops reading them",
            MAX_SOURCES_TOTAL / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_SOURCE + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("mise task file {shown}: {e}"))?;
    if bytes.len() as u64 > MAX_SOURCE {
        return Err(format!(
            "mise task file {shown} is larger than {} KiB; Pitwall does not read it",
            MAX_SOURCE / 1024
        ));
    }
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// 종류 `mise-task` 의 저장된 구성의 정의 본문(「사람이 본 구성」 — M6-11): 같은 루트의 감지 결과에서 그 태스크의 정의.
/// 감지에 없으면(mise 가 그 태스크를 모른다 · 미신뢰) `None` — 부른 쪽은 「본 적 없음」 으로 다룬다.
pub fn task_definition<'a>(detection: &'a super::Detection, task: &str) -> Option<&'a str> {
    detection
        .get(&super::key(ID, task))
        .map(|t| t.definition.as_str())
}
