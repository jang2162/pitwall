//! 1번 제공자 「mise」(03 §5-1 「런타임 제공자」 표, F14-2): 디렉터리의 mise 설정이 고정한 도구 버전의 환경.
//!
//! - 감지([`Mise::applies_to`]): 디렉터리와 그 위 폴더(사용자 홈 폴더 바로 아래까지 — 홈의 `~/.config/mise` 는 mise 의 전역
//!   설정이라 프로젝트의 것이 아니다)에 mise 설정 파일([`CONFIG_FILES`])이 있으면 쓴다. `.tool-versions` 만 있으면 asdf 의
//!   파일이기도 하므로 로그인 셸 PATH 에 `mise` 가 있을 때만 쓴다.
//! - 환경([`Mise::resolve`]): 로그인 셸 PATH 의 `mise` 로 `mise env --json` 을 그 디렉터리에서 돌린다(자식 환경은 로그인 셸
//!   환경 전체). 나온 변수를 로그인 셸 환경 위에 덮는다 — `PATH` 는 mise 가 도구 폴더를 앞에 붙인 전체 값이다.
//! - 실패의 갈래([`RuntimeProblem`]): `mise` 가 PATH 에 없음, mise 가 설정 파일을 신뢰하지 않음(`mise trust` — mise 2026 은 오류로
//!   끝나고, 2025 는 경고만 찍고 설정을 빼고 끝난다. 둘 다 stderr 의 문장으로 가른다), 그 밖의 실패(끝 코드 · 시간 초과 · JSON 이
//!   아닌 출력). 부른 쪽([`crate::Runtime`])이 로그인 셸 환경으로 물러나고 결과를 남기지 않는다.
//!
//! 프로젝트의 설정 파일이 hook · env 스크립트를 돌릴 수 있으므로 신뢰한 프로젝트에서만 고른다([`RuntimeProvider::runs_project_code`]).
//! 출력 형식은 고정한 mise 두 버전의 픽스처(`fixtures/mise/`)로 시험한다.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::env::{EXCLUDED, RuntimeEnv};
use crate::methods::{RuntimeProblem, RuntimeSource};
use crate::process::{self, RunError};
use crate::provider::{Layer, ProviderError, RuntimeProvider};

/// `mise env` 를 기다리는 시간(12-m1-plan.md M1-16 「시간 초과 권장 5초」).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// `mise env` 출력(stdout + stderr)의 상한. 넘으면 실패다.
pub const MAX_OUTPUT: usize = 16 * 1024 * 1024;

/// 한 폴더에서 찾는 mise 설정 파일(그 폴더 기준). mise 의 「Configuration」 문서의 파일 목록 중 환경 이름이 붙지 않는 것과
/// 옛 이름(`.rtx.toml`)이다. `.tool-versions` 는 따로 다룬다([`TOOL_VERSIONS`]).
pub const CONFIG_FILES: [&str; 11] = [
    ".config/mise/config.toml",
    ".config/mise/mise.toml",
    ".config/mise/config.local.toml",
    ".config/mise.toml",
    "mise/config.toml",
    ".mise/config.toml",
    ".rtx.toml",
    "mise.toml",
    ".mise.toml",
    "mise.local.toml",
    ".mise.local.toml",
];

/// 설정 조각 폴더. 안의 `*.toml` 이 설정 파일이다.
pub const CONF_D: &str = ".config/mise/conf.d";

/// asdf 와 함께 쓰는 버전 파일.
pub const TOOL_VERSIONS: &str = ".tool-versions";

/// mise 가 이 디렉터리의 설정 파일을 신뢰하지 않을 때 stderr 에 찍는 문장의 일부(2025.1.0 · 2026.4.3 픽스처).
const NOT_TRUSTED: &str = "are not trusted";

/// mise 제공자.
#[derive(Debug, Clone)]
pub struct Mise {
    timeout: Duration,
}

impl Default for Mise {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT)
    }
}

impl Mise {
    pub const ID: &'static str = "mise";

    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

/// `dir` 과 그 위 폴더의 mise 설정 파일(가까운 것부터)과, 그중 `.tool-versions` 가 아닌 것이 있는가. `home` 에 닿으면 멈춘다
/// (홈 폴더 자신은 보지 않는다 — `~/.config/mise` 는 전역 설정이다).
pub fn config_files(dir: &Path, home: Option<&Path>) -> (Vec<PathBuf>, bool) {
    let mut found = Vec::new();
    let mut mise_specific = false;
    for folder in dir.ancestors() {
        if home.is_some_and(|h| folder == h) {
            break;
        }
        for name in CONFIG_FILES {
            let path = folder.join(name);
            if path.is_file() {
                found.push(path);
                mise_specific = true;
            }
        }
        if let Ok(entries) = std::fs::read_dir(folder.join(CONF_D)) {
            let mut pieces: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension() == Some(OsStr::new("toml")) && p.is_file())
                .collect();
            pieces.sort();
            mise_specific |= !pieces.is_empty();
            found.extend(pieces);
        }
        let versions = folder.join(TOOL_VERSIONS);
        if versions.is_file() {
            found.push(versions);
        }
    }
    (found, mise_specific)
}

/// 바뀌면 mise 의 환경이 달라질 수 있는 파일인가(파일 감시 → 다시 계산, 03 §5-1 「캐시 · 무효화」). mise 설정 파일 · 조각,
/// `.tool-versions`, 환경 이름이 붙은 `mise.<env>.toml`, mise 가 읽을 수 있는 언어별 버전 파일(`.nvmrc` 등).
pub fn is_runtime_file(path: &Path) -> bool {
    const IDIOMATIC: [&str; 10] = [
        ".nvmrc",
        ".node-version",
        ".python-version",
        ".python-versions",
        ".ruby-version",
        ".go-version",
        ".java-version",
        ".sdkmanrc",
        ".bun-version",
        ".terraform-version",
    ];
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if name == TOOL_VERSIONS || IDIOMATIC.contains(&name) {
        return true;
    }
    if CONFIG_FILES.iter().any(|f| path.ends_with(f)) {
        return true;
    }
    let toml = Path::new(name).extension() == Some(OsStr::new("toml"));
    // `mise.<env>.toml` · `.mise.<env>.local.toml` 와 설정 조각.
    toml && (name.starts_with("mise.")
        || name.starts_with(".mise.")
        || path.parent().is_some_and(|p| p.ends_with(CONF_D)))
}

/// `base` 의 PATH(절대 경로 항목만)에서 `mise` 실행 파일.
pub fn find_mise(base: &RuntimeEnv) -> Option<PathBuf> {
    let name = if cfg!(windows) { "mise.exe" } else { "mise" };
    std::env::split_paths(&base.path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn home(base: &RuntimeEnv) -> Option<PathBuf> {
    base.get("HOME")
        .or_else(|| base.get("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
}

impl RuntimeProvider for Mise {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn runs_project_code(&self) -> bool {
        true
    }

    fn applies_to(&self, dir: &Path, base: &RuntimeEnv) -> bool {
        let (files, mise_specific) = config_files(dir, home(base).as_deref());
        // `.tool-versions` 만이면 asdf 사용자일 수 있다 — mise 가 없으면 경고 없이 로그인 셸 환경이다.
        mise_specific || (!files.is_empty() && find_mise(base).is_some())
    }

    fn resolve(&self, dir: &Path, base: &RuntimeEnv) -> Result<Layer, ProviderError> {
        let program = find_mise(base).ok_or_else(|| {
            ProviderError::new(
                RuntimeProblem::NotInstalled,
                "mise is not on the login shell PATH",
            )
        })?;
        let (files, _) = config_files(dir, home(base).as_deref());
        let mut command = Command::new(&program);
        command
            .args(["env", "--json"])
            .current_dir(dir)
            .env_clear()
            .envs(&base.vars)
            .env("PATH", &base.path)
            // 오류 문장에 색 코드가 섞이지 않게(상태바 · 로그에 원문이 간다).
            .env("NO_COLOR", "1");
        let output = process::run(command, self.timeout, MAX_OUTPUT).map_err(|e| {
            let reason = match e {
                RunError::Spawn(e) => format!("cannot start {}: {e}", program.display()),
                RunError::TimedOut => format!(
                    "{} env did not finish within {:?}",
                    program.display(),
                    self.timeout
                ),
                RunError::TooLarge => format!(
                    "{} env printed more than {MAX_OUTPUT} bytes",
                    program.display()
                ),
            };
            ProviderError::new(RuntimeProblem::Failed, reason)
        })?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains(NOT_TRUSTED) {
            return Err(ProviderError::new(
                RuntimeProblem::NotTrusted,
                format!("mise does not trust the config here: {}", summary(&stderr)),
            ));
        }
        if !output.status.success() {
            return Err(ProviderError::new(
                RuntimeProblem::Failed,
                format!("mise env failed ({}): {}", output.status, summary(&stderr)),
            ));
        }
        let vars = parse_env_json(&output.stdout).map_err(|e| {
            ProviderError::new(RuntimeProblem::Failed, format!("mise env --json: {e}"))
        })?;
        let mut env = base.clone();
        apply(&mut env, vars);
        Ok(Layer {
            env,
            source: RuntimeSource::ConfigFiles {
                files: files
                    .iter()
                    .map(|f| f.to_string_lossy().into_owned())
                    .collect(),
            },
        })
    }
}

/// `mise env --json` 의 출력: 변수 이름 → 값(문자열)의 객체. 값이 문자열이 아닌 항목은 버린다(로그에 남긴다).
pub fn parse_env_json(stdout: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(stdout).map_err(|e| format!("not JSON ({e})"))?;
    let serde_json::Value::Object(map) = value else {
        return Err("not a JSON object".to_owned());
    };
    let mut vars = BTreeMap::new();
    for (name, value) in map {
        match value {
            serde_json::Value::String(s) => {
                vars.insert(name, s);
            }
            other => {
                tracing::warn!(target: "pw_runtime", name, %other, "ignoring a non-string value from mise env --json");
            }
        }
    }
    if !vars.contains_key("PATH") {
        return Err("no PATH".to_owned());
    }
    Ok(vars)
}

/// mise 의 변수를 `env` 위에 덮는다. `PATH` 는 통째로 바꾸고 셸 고유 변수([`EXCLUDED`])는 버린다.
fn apply(env: &mut RuntimeEnv, vars: BTreeMap<String, String>) {
    let overlay = RuntimeEnv::from_vars(
        vars.into_iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
    );
    debug_assert!(
        overlay
            .vars
            .keys()
            .all(|k| !EXCLUDED.iter().any(|x| k == *x))
    );
    env.path = overlay.path;
    env.vars.extend(overlay.vars);
}

/// stderr 의 줄들을 한 줄로(앞뒤 공백 · 빈 줄 제외, 길면 뒤쪽 500자).
fn summary(stderr: &str) -> String {
    let joined = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() > 500 {
        format!("…{}", chars[chars.len() - 500..].iter().collect::<String>())
    } else {
        joined
    }
}

#[cfg(test)]
pub(crate) mod tests;
