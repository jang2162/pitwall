//! Gradle 태스크(17-m6-plan M6-05 ② — 결정 M6-D8 · D70). 태스크 목록은 빌드 스크립트를 돌려야 나온다(`./gradlew tasks --all`)
//! — **사람이 「태스크 읽기」(`run.tasks.refresh`)를 누를 때만** [`read`] 가 돌고 결과를 캐시([`super::cache`])한다. 감지([`detect`
//! 의 갈래](GradleDetector))는 프로세스를 띄우지 않는다: 캐시의 열쇠가 지금 빌드 파일과 맞으면 그 목록, 아니면 래퍼 · 빌드 파일이
//! 있다는 것과 흔한 태스크(`build` · `test`, 루트 빌드 파일에 Spring Boot 플러그인이 보이면 `bootRun`)만.
//!
//! 정의 본문(「사람이 본 구성」 — M6-11)은 태스크 이름이다 — 빌드 스크립트는 해시가 덮지 않는다(M6-D6 ⓒ). 이름은 Gradle 의 경로
//! 꼴(`app:bootRun`)만 받는다([`task_name`]) — `-` 로 시작하면 옵션으로 읽히므로 뺀다.

use std::path::{Path, PathBuf};

use super::cache::BuildFiles;
use super::{
    Budget, DetectContext, DetectExec, DetectedTask, Found, TaskDetector, file, pick_names,
};
use crate::config::{RunConfig, RunKindConfig};

pub const ID: &str = "gradle";

/// 래퍼(루트의 것). 실행 구성의 argv 와 같은 이름(`config::argv`).
pub const WRAPPER: &str = if cfg!(windows) {
    "gradlew.bat"
} else {
    "gradlew"
};

/// 감지 출처가 되는 루트의 파일 — 앞의 것이 목록의 `source` 다.
const ROOT_FILES: [&str; 4] = [
    "build.gradle.kts",
    "build.gradle",
    "settings.gradle.kts",
    "settings.gradle",
];

/// 읽기 전에 보이는 흔한 태스크.
const COMMON: [&str; 2] = ["build", "test"];
/// 루트 빌드 파일에 이것이 보이면 `bootRun` 을 더한다.
const SPRING_BOOT: &str = "org.springframework.boot";

pub struct GradleDetector;

impl TaskDetector for GradleDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn sources(&self) -> &'static [&'static str] {
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
            "gradle.properties",
            "gradle-wrapper.properties",
            "libs.versions.toml",
            WRAPPER,
        ]
    }

    fn detect(&self, cx: &DetectContext<'_>) -> Result<Found, String> {
        let Some(source) = ROOT_FILES
            .iter()
            .chain(std::iter::once(&WRAPPER))
            .find(|name| cx.entries.contains(**name))
        else {
            return Ok(Found::default());
        };
        let mut found = Found::default();
        let listed = super::pressed_list(cx, ID, BuildFiles::Gradle, &mut found);
        let names = match listed {
            Some(names) => names,
            None => common(cx)?,
        };
        build_tasks(ID, &cx.root.join(source), names, &mut found, |name| {
            RunKindConfig::Gradle { tasks: vec![name] }
        })?;
        Ok(found)
    }
}

/// 읽기 전의 정적 목록.
fn common(cx: &DetectContext<'_>) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = COMMON.iter().map(|s| (*s).to_owned()).collect();
    for name in ROOT_FILES.iter().take(2) {
        if cx.entries.contains(*name)
            && let Some((_, text)) = file::read(cx.root, name)?
            && text.contains(SPRING_BOOT)
        {
            names.push("bootRun".to_owned());
            break;
        }
    }
    Ok(names)
}

/// 이름들 → 감지된 태스크(Gradle · Maven 공용). 이름 꼴([`task_name`])이 아닌 것은 빼고 까닭을 남긴다. 정의 본문은 이름이다.
pub(crate) fn build_tasks(
    detector: &'static str,
    source: &Path,
    names: Vec<String>,
    found: &mut Found,
    kind: impl Fn(String) -> RunKindConfig,
) -> Result<(), String> {
    let mut budget = Budget::default();
    for name in plain_names(detector, names, &mut found.skipped) {
        budget.take(name.len())?;
        found.tasks.push(DetectedTask {
            detector,
            name: name.clone(),
            source: source.to_path_buf(),
            config: RunConfig {
                name: name.clone(),
                kind: kind(name.clone()),
                cwd: None,
                env: Default::default(),
                env_file: Vec::new(),
                profiles: Default::default(),
                debug: None,
                before: Vec::new(),
            },
            definition: name,
        });
    }
    Ok(())
}

/// 읽은 이름 → 목록에 둘 이름: 꼴([`task_name`] — 셸에 안전한 글자와 `:`, [`super::MAX_NAME_LEN`] 까지)이 아닌 것을 빼고 이름순 ·
/// 중복 없이 앞의 [`super::MAX_TASKS`] 개(`pick_names`). 뺀 까닭은 `skipped` 에. 캐시에 쓰기 전(`run.tasks.refresh`)과 캐시를
/// 읽은 뒤(감지) 둘 다 지난다 — 손으로 고친 캐시도 같은 규칙이다.
pub fn plain_names(detector: &str, names: Vec<String>, skipped: &mut Vec<String>) -> Vec<String> {
    let total = names.len();
    let names: Vec<String> = names.into_iter().filter(|n| task_name(n)).collect();
    if names.len() < total {
        skipped.push(format!(
            "skipped {} task(s) whose names are not plain task names",
            total - names.len()
        ));
    }
    pick_names(detector, names, skipped)
}

/// Gradle · Maven 태스크 이름으로 받는 꼴: 영문자 · 숫자 · `_` 로 시작하고 영문자 · 숫자 · `_` · `-` · `.` · `:` · `+` 만.
pub fn task_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= super::MAX_NAME_LEN
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':' | b'+'))
}

/// 루트의 래퍼(있으면 절대 경로), 없으면 맨 이름 `gradle`.
fn program(root: &Path) -> String {
    let wrapper: PathBuf = root.join(WRAPPER);
    if wrapper.is_file() {
        wrapper.display().to_string()
    } else {
        "gradle".to_owned()
    }
}

/// 「태스크 읽기」: `gradle tasks --all` 을 루트에서 돌려 태스크 이름을 읽는다. `--no-daemon` · `-Dorg.gradle.daemon=false` 를 주지만
/// `org.gradle.jvmargs` 가 있으면 gradle 은 그래도 1회용 데몬(새 세션)을 띄운다 — 그것은 실행기가 자손으로 따라가 끝낸다
/// ([`super::exec`] 머리 주석). 이름은 여기서 거르지 않는다 — 쓰기 전([`plain_names`])과 감지가 거른다.
pub fn read(root: &Path, exec: &dyn DetectExec) -> Result<Vec<String>, String> {
    let argv: Vec<String> = [
        program(root).as_str(),
        "tasks",
        "--all",
        "--quiet",
        "--console=plain",
        "--no-daemon",
        "-Dorg.gradle.daemon=false",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let out = exec.output(ID, &argv, root)?;
    let names = parse_tasks(&out);
    if names.is_empty() {
        return Err(
            "`gradle tasks --all` listed no task (an output format Pitwall does not know)"
                .to_owned(),
        );
    }
    Ok(names)
}

/// `gradle tasks --all` 의 출력에서 태스크 이름: 무리 머리(이름 줄 + `---` 줄) 아래 빈 줄까지의 `<이름>` · `<이름> - <설명>` 줄.
/// 이름에 공백이 있는 줄은 뺀다 — `Rules` 무리의 `Pattern: clean<TaskName>: …` 이 그렇다.
pub fn parse_tasks(out: &str) -> Vec<String> {
    let lines: Vec<&str> = out.lines().map(str::trim_end).collect();
    let is_rule = |l: &str| l.len() >= 3 && l.bytes().all(|b| b == b'-');
    let mut names = Vec::new();
    let mut in_group = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if !in_group {
            if !line.is_empty() && !is_rule(line) && lines.get(i + 1).is_some_and(|l| is_rule(l)) {
                in_group = true;
                i += 2;
                continue;
            }
        } else if line.is_empty() {
            in_group = false;
        } else {
            let name = line.split(" - ").next().unwrap_or_default().trim();
            if !name.is_empty() && !name.contains(char::is_whitespace) {
                names.push(name.to_owned());
            }
        }
        i += 1;
    }
    names
}
