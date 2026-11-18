//! 「사람이 본 구성」 해시의 내용을 구성에서 채운다(17-m6-plan M6-11 ① — 결정 M6-D6). 저장 · 판정 · 기록 증거는
//! `pw_core::registry::run_seen` 이다 — 이 모듈은 [`SeenContent`] 를 만들고 해시한다.
//!
//! 해시는 세 묶음이다:
//! 1. **구성 전체를 파싱한 값** — [`RunConfig::canonical`](주석 · 공백 · 키 순서 · 레이어 나눔이 들지 않는다. `[debug]` · 프로필 ·
//!    종류의 모든 필드, M6-21 이 더할 `before` · `configs` · `mode` 도 그 정규형에 든다).
//! 2. **풀린 실행 내용** — argv(프로필을 얹은 것, M6-01 [`crate::config::argv`]) · 작업 폴더 · env 의 이름과 값(M6-03 이 덮은
//!    결과) · `env_file` 의 경로와 각 파일 내용의 sha256 · 프로필.
//! 3. **명령 정의 본문** — `npm` 은 `package.json` 의 그 script, `mise-task` 는 `mise tasks ls --json` 의 그 태스크 `run`, 감지된
//!    태스크(열쇠 [`detected_key`])는 그 감지 출처의 본문. 그 밖(`Makefile` · `justfile` 레시피가 부르는 스크립트, 빌드 스크립트,
//!    프로그램 소스)은 덮지 않는다(M6-D6 ⓒ).
//!
//! 2 · 3 의 값은 실행을 띄우는 쪽이 이미 푼 것을 그대로 받는다([`SeenParts`]) — env 를 푸는 것은 M6-03, 정의 본문을 읽는 것은
//! 감지기(M6-04 · 05)의 몫이다. 해시를 다시 읽은 값으로 만들면 승인 · 실행과 다른 것을 해시하게 된다(계획 때 읽은 값으로 —
//! `pw_core::registry::plan` 머리 주석). 실행 관리자(M6-02 창 경로) · 실행 툴(M6-14 계획) · 저장(`run.config.save`)이 모두 이
//! 함수 하나로 해시한다 — 같은 내용이면 어디서 기록했든 같은 해시다.
//!
//! **워크트리마다 따로 보지 않는다**: 작업 폴더와 루트 안의 절대 경로 argv(래퍼 `gradlew` 등)는 프로젝트 루트 기준으로 적는다 —
//! 같은 구성을 다른 워크트리에서 띄워도 같은 해시다.

use std::collections::BTreeMap;
use std::path::{Component, Path};

pub use pw_core::registry::run_seen::{EnvFileDigest, SeenContent, SeenHash};

use crate::config::{RunConfig, RunKindConfig};

/// 감지된 태스크의 구성 열쇠 머리. 구성 id 는 `:` 를 받지 않으므로(`config::valid_id`) 겹치지 않는다.
pub const DETECTED_PREFIX: &str = "detected:";

/// 감지된 태스크의 구성 열쇠 `detected:<감지기>:<이름>`.
pub fn detected_key(detector: &str, name: &str) -> String {
    format!("{DETECTED_PREFIX}{detector}:{name}")
}

/// 루트 안 절대 경로를 적는 머리. NUL 은 argv 에 올 수 없어(`config::validate`) 실제 인자와 겹치지 않는다.
const ROOT_MARK: &str = "\0root\0";

/// 띄우는 쪽이 푼 실행 내용 — 실행을 띄울 때(또는 계획을 만들 때) 쓴 값 그대로다.
pub struct SeenParts<'a> {
    /// 구성 열쇠(구성 id 또는 [`detected_key`]).
    pub key: &'a str,
    /// 프로젝트 루트(활성 워크트리 안). 작업 폴더 · argv 의 절대 경로를 이것 기준으로 적는다.
    pub root: &'a Path,
    pub config: &'a RunConfig,
    pub profile: Option<&'a str>,
    /// 띄울 argv(프로필을 얹은 것).
    pub argv: &'a [String],
    /// 작업 폴더(절대 경로 — 루트 안).
    pub cwd: &'a Path,
    /// 런타임 환경 위에 얹는 env(덮은 결과 — M6-03).
    pub env: &'a BTreeMap<String, String>,
    /// 구성의 `env_file` 마다 읽은 내용, 구성의 순서대로(`None` = 파일이 없다).
    pub env_files: &'a [EnvFileRead<'a>],
    /// 명령 정의 본문([`needs_definition`] 이면 있어야 한다).
    pub definition: Option<&'a str>,
}

/// `env_file` 하나를 읽은 것.
#[derive(Clone, Copy)]
pub struct EnvFileRead<'a> {
    /// 구성에 적힌 경로 그대로.
    pub path: &'a str,
    pub contents: Option<&'a [u8]>,
}

/// 이 열쇠 · 종류는 명령 정의 본문을 해시에 넣어야 하는가 — `npm` · `mise-task` · 감지된 태스크.
pub fn needs_definition(key: &str, kind: &RunKindConfig) -> bool {
    key.starts_with(DETECTED_PREFIX)
        || matches!(
            kind,
            RunKindConfig::Npm { .. } | RunKindConfig::MiseTask { .. }
        )
}

/// [`SeenContent`] 를 채운다. 빠진 것이 있으면(정의 본문 · `env_file` 의 내용, 루트 밖 작업 폴더) 오류다 — 부른 쪽은 「본 적
/// 없음」 으로 다룬다(묻는다). 오류 글에 값을 쓰지 않는다.
pub fn content(parts: &SeenParts<'_>) -> Result<SeenContent, String> {
    let config = parts.config;
    if needs_definition(parts.key, &config.kind) && parts.definition.is_none() {
        return Err(format!(
            "`{}`: the command definition is missing (type `{}`)",
            parts.key,
            config.kind.type_name()
        ));
    }
    let listed: Vec<&str> = config.env_file.iter().map(String::as_str).collect();
    let read: Vec<&str> = parts.env_files.iter().map(|f| f.path).collect();
    if listed != read {
        return Err(format!(
            "`{}`: the env files read do not match the configuration's env_file",
            parts.key
        ));
    }
    // 루트를 글 그대로와 실제 경로(심링크를 푼 것) 둘로 본다 — 작업 폴더는 정규화한 경로로 오고(M6-10 의 허가) 래퍼 경로는
    // 루트를 이어 붙인 글이다.
    let real = std::fs::canonicalize(parts.root).ok();
    let roots: Vec<&Path> = std::iter::once(parts.root).chain(real.as_deref()).collect();
    let cwd = relative(&roots, parts.cwd).ok_or_else(|| {
        format!(
            "`{}`: the working directory is outside the project root",
            parts.key
        )
    })?;
    Ok(SeenContent {
        config: config.canonical(),
        argv: parts.argv.iter().map(|arg| rooted(&roots, arg)).collect(),
        cwd,
        env: parts.env.clone(),
        env_files: parts
            .env_files
            .iter()
            .map(|f| EnvFileDigest::of(f.path, f.contents))
            .collect(),
        profile: parts.profile.map(str::to_owned),
        definition: parts.definition.map(str::to_owned),
    })
}

/// [`content`] 의 해시.
pub fn hash(parts: &SeenParts<'_>) -> Result<SeenHash, String> {
    content(parts).map(|c| c.hash())
}

/// `path` 를 루트(`roots` 가운데 앞부분이 맞는 것) 기준 `/` 구분 글로. 루트 밖이거나 `..` 가 남으면 `None`. 루트 자신은 `.`.
fn relative(roots: &[&Path], path: &Path) -> Option<String> {
    let rest = roots.iter().find_map(|root| path.strip_prefix(root).ok())?;
    let mut parts = Vec::new();
    for component in rest.components() {
        match component {
            Component::Normal(name) => parts.push(name.to_str()?.to_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}

/// 루트 안의 절대 경로 인자는 루트 기준으로 적는다(워크트리마다 같은 해시). 그 밖은 그대로.
fn rooted(roots: &[&Path], arg: &str) -> String {
    let path = Path::new(arg);
    if path.is_absolute()
        && let Some(rel) = relative(roots, path)
    {
        return format!("{ROOT_MARK}{rel}");
    }
    arg.to_owned()
}

#[cfg(test)]
mod tests;
