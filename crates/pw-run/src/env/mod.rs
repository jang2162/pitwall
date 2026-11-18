//! 실행의 env(17-m6-plan M6-03 — F7-6): 구성의 `env_file` · `env` · 고른 프로필을 런타임 환경 위에 얹는다.
//!
//! - **덮는 순서**: 런타임 환경(03 §5-1) → `env_file`(목록 순서, 파일 안에서는 뒤의 줄) → 구성의 `env` → 고른 프로필의 `env`.
//!   뒤의 것이 이긴다. 런타임 환경을 뺀 나머지 셋을 합친 것이 「얹는 env」([`RunEnv::vars`] — `pw_core::registry::exec::RunSpec`
//!   의 `env`)이고, 띄우는 쪽(M6-02)이 그것을 [`apply`] 로 런타임 환경에 얹는다. 프로필의 `args` 는 `config::argv` 가 본다.
//! - **`.env` 파일**: 문법은 [`dotenv`](변수 확장 없음). 경로는 프로젝트 루트(활성 워크트리 안) 기준이고, 실행하는 그때 다시
//!   판정한다 — 루트 밖(`..` · 절대 경로 · 링크로 밖)이면 거절(`config::root::inside`, 구성 검증과 같은 규칙)하고, 연 핸들이 일반
//!   파일이며 여전히 루트 안인지 본다(`pw_fs::scope::Roots::readable` · `open_inside` — 판정과 열기 사이에 링크가 바뀌어도, FIFO 로
//!   바뀌어도). 파일 하나는 [`MAX_ENV_FILE_BYTES`] 까지. 없는 파일은 오류다(조용히 건너뛰면 다른 값으로 돈다).
//! - **한 번 읽는다**: 파일 바이트를 한 번 읽어 그것으로 값과 해시를 낸다 — 승인(M6-14)한 계획의 값과 띄우는 값이 같다.
//! - **비밀**(03 §5-1 「비밀」): 값은 로그 · 오류 · 감사 · 시험 메시지에 쓰지 않는다. [`RunEnv`] · [`EnvError`] 의 `Debug` ·
//!   `Display` 는 이름 · 경로 · 줄 번호만 보인다. 사람에게 값을 보이는 곳은 승인 계획 하나이고([`RunEnv::masked`] — 비밀 단어
//!   키는 `pw_core::secret` 로 가린다), 그 계획의 글은 감사에 남지 않는다(계획 해시만).
//! - **「사람이 본 구성」 해시**(M6-11)의 입력: 풀린 env([`RunEnv::vars`] — 이름과 값)와 `env_file` 마다의 다이제스트
//!   ([`RunEnv::files`] — 루트 기준 경로와 내용 sha256). 해시하는 것은 M6-11 이다(공유 부품 표 — 기록의 주인). 다이제스트의
//!   sha256 은 값을 낸 바로 그 바이트의 것이다(한 번 읽은 버퍼 하나에서 둘 다 낸다 — 다시 읽는 틈이 없다).
//! - 미신뢰 프로젝트의 거절(F1-7 · 메인 판정 M6-P6)은 띄우는 쪽(M6-02)이 이것을 부르기 전에 한다 — 여기는 신뢰를 보지 않는다.

pub mod dotenv;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io::Read as _;
use std::path::Path;

use pw_runtime::RuntimeEnv;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::RunConfig;

/// `.env` 파일 하나의 상한(바이트).
pub const MAX_ENV_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// 구성 하나(와 고른 프로필)의 env — 런타임 환경 위에 얹을 변수와, 그 변수를 낸 `.env` 파일들의 해시.
///
/// `Debug` 는 손으로 쓴다 — 값을 찍지 않고 이름 · 파일 경로 · 프로필만(03 §5-1 「비밀」). derive 로 바꾸지 않는다(시험
/// `debug_and_errors_never_show_values` 가 본다).
#[derive(Clone, PartialEq, Eq)]
pub struct RunEnv {
    vars: BTreeMap<String, String>,
    files: Vec<EnvFileDigest>,
    profile: Option<String>,
}

/// 읽은 `.env` 파일 하나: 구성에 적힌 경로(프로젝트 루트 기준 — 워크트리와 무관하다)와, [`RunEnv::vars`] 의 값을 낸 바로 그
/// 바이트의 sha256(소문자 16진 64자). 「사람이 본 구성」 해시(M6-11)의 입력이다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvFileDigest {
    pub path: String,
    pub sha256: String,
}

impl RunEnv {
    /// `config` 의 env 를 푼다. `profile` 은 실행 선택기에서 고른 프로필(없는 이름이면 오류), `root` 는 프로젝트 루트(활성 워크트리
    /// 안 — `RunConfigPlace::root`). `.env` 파일을 지금 읽는다(블로킹 — 비동기 쪽은 블로킹 스레드에서 부른다).
    pub fn resolve(
        config: &RunConfig,
        profile: Option<&str>,
        root: &Path,
    ) -> Result<Self, EnvError> {
        let profile_env = match profile {
            None => BTreeMap::new(),
            Some(name) => config
                .profiles
                .get(name)
                .ok_or_else(|| EnvError::UnknownProfile(name.to_owned()))?
                .env
                .clone(),
        };
        let mut vars = BTreeMap::new();
        let mut files = Vec::with_capacity(config.env_file.len());
        for rel in &config.env_file {
            let bytes = read_env_file(root, rel)?;
            let text =
                std::str::from_utf8(&bytes).map_err(|_| EnvError::NotUtf8 { file: rel.clone() })?;
            let pairs = dotenv::parse(text).map_err(|error| EnvError::Syntax {
                file: rel.clone(),
                error,
            })?;
            for (name, value) in pairs {
                insert(&mut vars, name, value);
            }
            files.push(EnvFileDigest {
                path: rel.clone(),
                sha256: hex(&Sha256::digest(&bytes)),
            });
        }
        for (name, value) in config.env.iter().chain(&profile_env) {
            insert(&mut vars, name.clone(), value.clone());
        }
        Ok(Self {
            vars,
            files,
            profile: profile.map(str::to_owned),
        })
    }

    /// 런타임 환경 위에 얹을 변수(`env_file` → `env` → 프로필 `env` 를 합친 것). `RunSpec::env` 에 그대로 싣는다.
    pub fn vars(&self) -> &BTreeMap<String, String> {
        &self.vars
    }

    /// [`Self::vars`] 를 꺼낸다.
    pub fn into_vars(self) -> BTreeMap<String, String> {
        self.vars
    }

    /// 읽은 `.env` 파일들의 다이제스트(구성의 순서) — 「사람이 본 구성」 해시(M6-11)의 입력.
    pub fn files(&self) -> &[EnvFileDigest] {
        &self.files
    }

    /// 고른 프로필.
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// 띄울 프로세스의 환경 전체 — `runtime` 위에 [`Self::vars`] 를 얹은 것([`apply`]).
    pub fn apply(&self, runtime: &RuntimeEnv) -> RuntimeEnv {
        apply(runtime, &self.vars)
    }

    /// 승인 계획(M6-14 ②)에 보일 변수: [`Self::vars`] 에서 이름이 비밀 단어인 것(`pw_core::secret::is_secret_key` — `API_KEY` ·
    /// `DB_PASSWORD`)의 값을 `[redacted]` 로 바꾼 것. 사람이 승인하는 화면에만 쓴다 — 감사 · 로그에는 이것도 쓰지 않는다.
    pub fn masked(&self) -> BTreeMap<String, String> {
        self.vars
            .iter()
            .map(|(name, value)| {
                let shown = if pw_core::secret::is_secret_key(name) {
                    pw_core::secret::REDACTED.to_owned()
                } else {
                    value.clone()
                };
                (name.clone(), shown)
            })
            .collect()
    }
}

impl fmt::Debug for RunEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunEnv")
            .field("var_names", &self.vars.keys().collect::<Vec<_>>())
            .field(
                "env_files",
                &self.files.iter().map(|d| &d.path).collect::<Vec<_>>(),
            )
            .field("profile", &self.profile)
            .finish()
    }
}

/// `runtime` 위에 `vars` 를 얹은 환경. `PATH` 는 [`RuntimeEnv::path`] 를 바꾼다. Windows 는 이름의 대소문자를 가리지 않는다
/// (`Path` 가 `PATH` 를 바꾼다). 에이전트 갈래(M6-10)는 승인된 `RunSpec::env` 를 이것으로 얹는다 — `.env` 를 다시 읽지 않는다.
pub fn apply(runtime: &RuntimeEnv, vars: &BTreeMap<String, String>) -> RuntimeEnv {
    let mut env = runtime.clone();
    for (name, value) in vars {
        if same_name(name, "PATH") {
            env.path = OsString::from(value);
            continue;
        }
        if cfg!(windows) {
            env.vars
                .retain(|existing, _| !existing.eq_ignore_ascii_case(name));
        }
        env.vars.insert(OsString::from(name), OsString::from(value));
    }
    env
}

/// `.env` 를 풀지 못했다. 값 · 파일 내용은 싣지 않는다 — 파일 경로(구성에 적힌 것) · 줄 번호 · 까닭만.
#[derive(Debug)]
pub enum EnvError {
    /// 고른 프로필이 구성에 없다.
    UnknownProfile(String),
    /// 파일이 프로젝트 루트 밖으로 풀린다(`..` · 절대 경로 · 링크) — `why` 는 `config::root::inside` 의 까닭.
    Outside { file: String, why: String },
    /// 파일을 열거나 읽지 못했다(없음 · 권한 · 일반 파일이 아님 · 읽는 사이 링크가 밖으로 바뀜).
    Read { file: String, error: std::io::Error },
    /// [`MAX_ENV_FILE_BYTES`] 를 넘는다.
    TooLarge { file: String },
    /// UTF-8 이 아니다.
    NotUtf8 { file: String },
    /// 문법 오류([`dotenv`]).
    Syntax {
        file: String,
        error: dotenv::SyntaxError,
    },
}

impl fmt::Display for EnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvError::UnknownProfile(name) => write!(f, "profile `{name}` does not exist"),
            EnvError::Outside { file, why } => write!(f, "env_file `{file}`: {why}"),
            EnvError::Read { file, error } => write!(f, "env_file `{file}`: cannot read: {error}"),
            EnvError::TooLarge { file } => write!(
                f,
                "env_file `{file}` is larger than {} MiB",
                MAX_ENV_FILE_BYTES / 1024 / 1024
            ),
            EnvError::NotUtf8 { file } => write!(f, "env_file `{file}` is not UTF-8 text"),
            EnvError::Syntax { file, error } => write!(f, "env_file `{file}` {error}"),
        }
    }
}

impl std::error::Error for EnvError {}

/// 판정 → 열기 → 다시 판정 → 상한까지 읽기.
fn read_env_file(root: &Path, rel: &str) -> Result<Vec<u8>, EnvError> {
    let file = || rel.to_owned();
    let real = crate::config::root::inside(root, rel)
        .map_err(|why| EnvError::Outside { file: file(), why })?;
    let real_root = std::fs::canonicalize(root).map_err(|error| EnvError::Read {
        file: file(),
        error,
    })?;
    let roots = pw_fs::scope::Roots::new([pw_core::registry::path::simplify(real_root)]);
    let readable = roots.readable(&real).map_err(|denied| EnvError::Outside {
        file: file(),
        why: denied.to_string(),
    })?;
    let handle = roots
        .open_inside(&readable.real)
        .map_err(|error| EnvError::Read {
            file: file(),
            error,
        })?;
    let mut bytes = Vec::new();
    handle
        .take(MAX_ENV_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| EnvError::Read {
            file: file(),
            error,
        })?;
    if bytes.len() as u64 > MAX_ENV_FILE_BYTES {
        return Err(EnvError::TooLarge { file: file() });
    }
    Ok(bytes)
}

/// 뒤의 것이 이긴다. Windows 는 이름의 대소문자를 가리지 않으므로 같은 이름의 다른 꼴을 지운다.
fn insert(vars: &mut BTreeMap<String, String>, name: String, value: String) {
    if cfg!(windows) {
        vars.retain(|existing, _| !existing.eq_ignore_ascii_case(&name));
    }
    vars.insert(name, value);
}

fn same_name(name: &str, wanted: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(wanted)
    } else {
        name == wanted
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

#[cfg(test)]
mod tests;
