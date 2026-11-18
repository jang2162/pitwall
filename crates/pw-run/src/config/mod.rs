//! 실행 구성(17-m6-plan M6-01 — 메인 판정 M6-P4): `.pitwall/run/<id>.toml` 의 타입 · 검증 · JSON Schema · 정규형, 파일 읽기
//! ([`read`]), 쓰기([`write`]), `run.config.*` RPC([`methods`]), 종류마다의 argv([`argv`]).
//!
//! 파일(첫 줄 `schema = 1`, `<id>` 는 파일 이름):
//!
//! ```toml
//! schema = 1
//! name = "Dev Server"
//! type = "gradle"               # 종류 — 아래 갈래
//! tasks = ["bootRun"]
//! cwd = "backend"               # 프로젝트 루트 기준. 루트 밖(`..` · 링크로 밖)이면 쓸 수 없다
//! env = { SPRING_PROFILES_ACTIVE = "local" }
//! env_file = [".env.local"]     # 목록, 프로젝트 루트 안만
//! [profiles.prod-like]          # 실행 선택기에서 고르는 덮어쓰기 — `env` 는 키별로 얹고 `args` 는 통째로 바꾼다(M6-03)
//! env = { SPRING_PROFILES_ACTIVE = "prod" }
//! [debug]                       # 디버그 세션(M6-17)이 읽는다 — `adapter` · `mode` 밖의 키는 어댑터 틀의 자리
//! mode = "attach"
//! port = 5005
//! ```
//!
//! - **종류**([`RunKindConfig`]): `shell` 만 글(`command` — 로그인 셸의 `-c`)이고, 그 밖은 필드에서 argv 를 만든다 — 필드를 셸
//!   글로 이어 붙이지 않는다([`argv`]). 종류를 더하는 행(M6-09 `sql` · M6-21 `compound`)은 갈래 하나와 [`argv::argv`] ·
//!   [`validate`] 의 갈래를 더한다.
//! - **키 이름**: 파일은 snake_case(`env_file`), RPC 는 camelCase(`envFile`) — 둘이 다른 키는 [`FILE_KEYS`] 표 하나로 옮긴다.
//! - **모르는 키**는 읽을 때 무시하고(새 버전 Pitwall 이 더한 키), 쓸 때 그대로 둔다([`write`]). 모르는 `type` · 형식 오류 ·
//!   검증 실패는 버리지 않고 「쓸 수 없음(까닭)」 이다([`read`]).
//! - **정규형**([`RunConfig::canonical`]): 파싱한 값의 JSON(키 정렬 · 공백 없음) — 주석 · 공백 · 키 순서 · 레이어 나눔과 무관하다.
//!   「사람이 본 구성」 해시(M6-11)의 「구성 전체를 파싱한 값」 이 이것이다.

pub mod argv;
pub mod methods;
pub mod read;
pub mod root;
pub mod write;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use read::{LoadedRunConfig, RunConfigPlace, RunConfigSet, UnusableRunConfig, read};
pub use write::{RunConfigEdit, RunConfigLocation, apply, plan, write};

/// 실행 구성 파일의 스키마 버전.
pub const SCHEMA: u32 = 1;
/// 설정 폴더(`.pitwall/` · 개인 전용 폴더) 안의 실행 구성 폴더.
pub const DIR: &str = "run";

/// 파일 키(snake_case)와 RPC 키(camelCase)가 다른 최상위 키: (파일, RPC). 한 낱말 키는 같으므로 여기에 없다. 여러 낱말 키를
/// 더하면 여기에 더한다 — 시험 `multi_word_keys_are_mapped` 가 스키마의 camelCase 키를 이 표와 맞춰 본다.
pub const FILE_KEYS: &[(&str, &str)] = &[("env_file", "envFile")];

/// One run configuration: the common fields plus the kind (`type`, [`RunKindConfig`]). Doc comments of the configuration types are
/// English — they become the JSON Schema descriptions an agent reads (M6-13, decision M5-D39).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    /// Display name (run selector, console tab).
    pub name: String,
    #[serde(flatten)]
    pub kind: RunKindConfig,
    /// Working directory, relative to the project root (inside the active worktree). Defaults to the project root. A path that
    /// resolves outside the root (`..`, a symbolic link) makes the configuration unusable.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Environment variables (name to value), applied over the runtime environment and `envFile`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// `.env` files relative to the project root, applied in order. A file outside the root makes the configuration unusable.
    #[serde(default)]
    pub env_file: Vec<String>,
    /// Named overrides picked in the run selector.
    #[serde(default)]
    pub profiles: BTreeMap<String, RunProfile>,
    /// How to debug this configuration. Without it the configuration cannot be debugged.
    #[serde(default)]
    pub debug: Option<DebugConfig>,
}

/// The kind of configuration (`type`) and its fields. Every kind except `shell` builds its command line from these fields, one
/// field value per argument — nothing is joined into a shell string.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum RunKindConfig {
    /// A shell command line, passed verbatim to the login shell's `-c` (pipes and expansions are the shell's).
    Shell { command: String },
    /// A `package.json` script. The package manager (npm, pnpm, yarn, bun) follows the lockfile.
    Npm { script: String },
    /// Gradle tasks. Uses the `gradlew` wrapper when there is one.
    Gradle { tasks: Vec<String> },
    /// Maven goals. Uses the `mvnw` wrapper when there is one.
    Maven { tasks: Vec<String> },
    /// `cargo <command> <args…>`.
    Cargo {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// `go <command> [package] <args…>`.
    Go {
        command: String,
        #[serde(default)]
        package: Option<String>,
        #[serde(default)]
        args: Vec<String>,
    },
    /// `python -m <module>` or `python <script>`; exactly one of the two.
    Python {
        #[serde(default)]
        module: Option<String>,
        #[serde(default)]
        script: Option<String>,
        #[serde(default)]
        args: Vec<String>,
    },
    /// `docker compose [-f <file>] up [services…]`. Stopping sends SIGINT first so compose stops the containers.
    DockerCompose {
        #[serde(default)]
        services: Vec<String>,
        /// Compose file relative to the project root. Without it compose finds its own.
        #[serde(default)]
        file: Option<String>,
    },
    /// `mise run <task>`.
    MiseTask { task: String },
}

impl RunKindConfig {
    /// 파일의 `type` 값.
    pub fn type_name(&self) -> &'static str {
        match self {
            RunKindConfig::Shell { .. } => "shell",
            RunKindConfig::Npm { .. } => "npm",
            RunKindConfig::Gradle { .. } => "gradle",
            RunKindConfig::Maven { .. } => "maven",
            RunKindConfig::Cargo { .. } => "cargo",
            RunKindConfig::Go { .. } => "go",
            RunKindConfig::Python { .. } => "python",
            RunKindConfig::DockerCompose { .. } => "docker-compose",
            RunKindConfig::MiseTask { .. } => "mise-task",
        }
    }

    /// 프로필의 `args` 가 바꾸는 인자 자리가 있는 종류인가.
    pub fn takes_args(&self) -> bool {
        matches!(
            self,
            RunKindConfig::Cargo { .. } | RunKindConfig::Go { .. } | RunKindConfig::Python { .. }
        )
    }
}

/// A profile (`[profiles.<name>]`): overrides applied over the configuration when it is picked.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunProfile {
    /// Applied key by key over the configuration's `env`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Replaces the kind's `args` as a whole. Only for kinds with `args` (cargo, go, python).
    #[serde(default)]
    pub args: Option<Vec<String>>,
}

/// `[debug]`. Keys other than `adapter` and `mode` (`program`, `port`, `pid` …) fill the debug adapter's launch or attach
/// template and are kept as written.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DebugConfig {
    /// Debug adapter id. Without it the adapter is chosen by language.
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub mode: DebugMode,
    /// The adapter template's other keys, as written in the file.
    #[serde(flatten)]
    #[specta(type = BTreeMap<String, pw_rpc::JsonValue>)]
    pub settings: BTreeMap<String, Value>,
}

/// `launch` starts the program under the debugger; `attach` only connects to a program that is already running.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DebugMode {
    #[default]
    Launch,
    Attach,
}

impl RunConfig {
    /// 정규형: 파싱한 값을 RPC 의 키 이름(camelCase)으로 JSON 으로 쓴 것 — 객체 키는 모든 깊이에서 사전순, 공백 없음, 빈 값도
    /// 적는다(없는 `cwd` 는 `null`, 빈 `env` 는 `{}`). 같은 구성(주석 · 공백 · 키 순서 · `env` 를 인라인 표로 쓰든 `[env]` 로
    /// 쓰든, 어느 레이어에 적었든)이면 같은 글이다. 「사람이 본 구성」 해시(M6-11)의 「구성 전체를 파싱한 값」 이다.
    pub fn canonical(&self) -> String {
        let value = serde_json::to_value(self).expect("a run config serializes to JSON");
        let mut out = String::new();
        write_sorted(&value, &mut out);
        out
    }
}

fn write_sorted(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_sorted(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_sorted(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// RPC 의 구성 타입이 받는 JSON Schema(`schemars`). 폼(M6-07 — 생성물 `schema.json` 의 `run.config.save` 입력에도 같은 것이
/// 들어 있다)과 MCP 입력 검증(M6-13)이 같은 것을 쓴다.
pub fn json_schema() -> schemars::Schema {
    schemars::schema_for!(RunConfig)
}

/// 구성 id(= 파일 이름): 영문자 · 숫자 · `-` · `_`, 1~64 글자. `.` 을 받지 않는다 — `a.local` 은 `a` 의 개인 파일과 겹친다.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 환경 변수 이름: `[A-Za-z_][A-Za-z0-9_]*`, 256 바이트까지.
pub fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 256
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// 구성이 쓸 수 있는 꼴인가 — 저장(`run.config.save`)은 이것을 지나야 하고, 읽은 파일이 지나지 못하면 「쓸 수 없음」 이다.
/// `root` 는 프로젝트 루트(활성 워크트리 안)다 — `cwd` · `envFile` · compose `file` 이 그 안으로 풀리는지 디스크에서 본다
/// ([`root::inside`]).
pub fn validate(config: &RunConfig, root: &std::path::Path) -> Result<(), String> {
    text("name", &config.name)?;
    if config.name.chars().any(char::is_control) {
        return Err("name must not contain control characters".to_owned());
    }
    validate_kind(&config.kind, root)?;
    if let Some(cwd) = &config.cwd {
        root::inside(root, cwd).map_err(|why| format!("cwd {why}"))?;
    }
    validate_env("env", &config.env)?;
    for file in &config.env_file {
        root::inside(root, file).map_err(|why| format!("env_file {why}"))?;
    }
    for (name, profile) in &config.profiles {
        if name.is_empty() || name.chars().any(|c| c.is_control() || c == '.') {
            return Err(format!(
                "profile name {name:?} must be non-empty without dots or control characters"
            ));
        }
        validate_env(&format!("profiles.{name}.env"), &profile.env)?;
        if let Some(args) = &profile.args {
            if !config.kind.takes_args() {
                return Err(format!(
                    "profiles.{name}.args: type `{}` takes no args",
                    config.kind.type_name()
                ));
            }
            argv_items(&format!("profiles.{name}.args"), args)?;
        }
    }
    if let Some(debug) = &config.debug
        && let Some(adapter) = &debug.adapter
    {
        text("debug.adapter", adapter)?;
    }
    Ok(())
}

fn validate_kind(kind: &RunKindConfig, root: &std::path::Path) -> Result<(), String> {
    match kind {
        RunKindConfig::Shell { command } => {
            text("command", command)?;
            no_nul("command", command)
        }
        RunKindConfig::Npm { script } => arg("script", script),
        RunKindConfig::Gradle { tasks } | RunKindConfig::Maven { tasks } => {
            if tasks.is_empty() {
                return Err("tasks is empty".to_owned());
            }
            argv_items("tasks", tasks)
        }
        RunKindConfig::Cargo { command, args } => {
            arg("command", command)?;
            argv_items("args", args)
        }
        RunKindConfig::Go {
            command,
            package,
            args,
        } => {
            arg("command", command)?;
            if let Some(package) = package {
                arg("package", package)?;
            }
            argv_items("args", args)
        }
        RunKindConfig::Python {
            module,
            script,
            args,
        } => {
            match (module, script) {
                (Some(m), None) => arg("module", m)?,
                (None, Some(s)) => arg("script", s)?,
                _ => {
                    return Err(
                        "a python config needs exactly one of `module` and `script`".to_owned()
                    );
                }
            }
            argv_items("args", args)
        }
        RunKindConfig::DockerCompose { services, file } => {
            argv_items("services", services)?;
            if let Some(file) = file {
                root::inside(root, file).map_err(|why| format!("file {why}"))?;
            }
            Ok(())
        }
        RunKindConfig::MiseTask { task } => arg("task", task),
    }
}

fn validate_env(what: &str, env: &BTreeMap<String, String>) -> Result<(), String> {
    for (name, value) in env {
        if !valid_env_name(name) {
            return Err(format!(
                "{what}: `{name}` is not an environment variable name"
            ));
        }
        // 값은 오류 글에 쓰지 않는다(비밀일 수 있다 — M6-03).
        if value.contains('\0') {
            return Err(format!("{what}.{name}: the value contains a NUL character"));
        }
    }
    Ok(())
}

fn text(what: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{what} is empty"))
    } else {
        Ok(())
    }
}

fn no_nul(what: &str, value: &str) -> Result<(), String> {
    if value.contains('\0') {
        Err(format!("{what} contains a NUL character"))
    } else {
        Ok(())
    }
}

/// argv 한 칸이 될 필수 값.
fn arg(what: &str, value: &str) -> Result<(), String> {
    text(what, value)?;
    no_nul(what, value)
}

/// argv 칸들(빈 칸은 받는다 — `""` 도 인자다).
fn argv_items(what: &str, items: &[String]) -> Result<(), String> {
    items
        .iter()
        .enumerate()
        .try_for_each(|(i, item)| no_nul(&format!("{what}[{i}]"), item))
}

#[cfg(test)]
mod tests;
