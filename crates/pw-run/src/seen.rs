//! 「사람이 본 구성」 해시(17-m6-plan M6-11 ① — 결정 M6-D6). 해시를 계산하는 주인은 이 모듈이다(공유 부품 표 — 기록의 주인
//! M6-11). 저장 · 판정 · 기록 증거는 `pw_core::registry::run_seen` 이다.
//!
//! 해시는 세 묶음이다([`SeenContent`]):
//! 1. **구성 전체를 파싱한 값** — [`RunConfig::canonical`](주석 · 공백 · 키 순서 · 레이어 나눔이 들지 않는다. `[debug]` · 프로필 ·
//!    종류의 모든 필드, M6-21 이 더할 `before` · `configs` · `mode` 도 `RunConfig` 의 필드면 그 정규형에 든다).
//! 2. **풀린 실행 내용** — argv(프로필을 얹은 것, M6-01 [`crate::config::argv`]) · 작업 폴더 · M6-03 의 [`RunEnv`] 가 낸 것(얹는
//!    env 의 이름과 값 [`RunEnv::vars`], `env_file` 마다의 경로와 내용 sha256 [`RunEnv::files`], 프로필), 그리고 **런타임 환경의
//!    원천 파일**([`runtime_files`] — 런타임 제공자 mise 가 그 작업 폴더에서 읽는 `mise.toml` · `.tool-versions` · `.nvmrc` 등)의
//!    경로와 내용 sha256. 런타임 환경은 얹는 env 아래 칸이고 `mise.toml` 의 `[env]` 로 `NODE_OPTIONS` 를 넣을 수 있다 — `.env` 와
//!    같은 길이라 같이 덮는다(D68). 런타임 env 의 값은 넣지 않는다(로그인 셸의 값 — `SHLVL` · `TMPDIR` — 이 섞여 매번 묻는다).
//!    mise task 의 `depends` · `env` 같은 태스크 정의도 이 파일들에 있어 함께 덮인다. `[runtime] provider` 설정은 넣지 않는다 —
//!    제공자를 고를 뿐 값을 더하지 않는다(`mise` 의 값은 이 파일들에서, `shell` · `none` 은 사용자 · 앱의 환경에서 온다).
//! 3. **명령 정의 본문** — M6-04 의 감지가 낸 것: `npm` 은 `package.json` 의 그 script 와 `pre` · `post` script
//!    (`detect::npm::script_definition`), 감지된 태스크(열쇠 [`detect::key`])는 [`detect::DetectedTask::definition`] — `Makefile` ·
//!    `justfile` 은 그 파일 전체의 sha256 과 이름(`detect::file_definition`). `mise-task` 는 그 태스크의 정의(M6-05). 그 밖(레시피가
//!    부르는 스크립트, 빌드 스크립트, 프로그램 소스)은 덮지 않는다(M6-D6 ⓒ).
//!
//! 2 · 3 의 값은 실행을 띄우는 쪽이 이미 푼 것을 그대로 받는다([`SeenParts`]) — `.env` 를 다시 읽지 않는다([`RunEnv`] 의
//! 다이제스트는 값을 낸 그 바이트의 것이다). 다시 읽으면 승인 · 실행과 다른 것을 해시하게 된다(`pw_core::registry::plan` 머리
//! 주석). 실행 관리자(M6-02 창 경로) · 실행 툴(M6-14 계획) · 저장(`run.config.save`)이 모두 이 함수 하나로 해시한다 — 같은
//! 내용이면 어디서 기록했든 같은 해시다.
//!
//! **워크트리마다 따로 보지 않는다**: 작업 폴더 · 루트 안의 절대 경로 argv(래퍼 `gradlew` 등) · 런타임 원천 파일은 프로젝트 루트
//! 기준으로 적는다 — 같은 구성을 다른 워크트리에서 띄워도 같은 해시다.
//!
//! **작업 폴더는 이 함수가 실제 경로로 푼다**(심링크 · `..`) — 부른 쪽이 `root.join(cwd)` 를 넘겨도 링크를 바꿔치기한 것이 해시에
//! 잡힌다. 풀 수 없거나(없는 폴더) 루트 밖이면 오류다.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use pw_runtime::mise;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub use pw_core::registry::run_seen::SeenHash;

use crate::config::{RunConfig, RunKindConfig};
use crate::detect;
use crate::env::{EnvFileDigest, RunEnv, hex};

/// 루트 안 절대 경로를 적는 머리. NUL 은 argv 에 올 수 없어(`config::validate`) 실제 인자와 겹치지 않는다.
const ROOT_MARK: &str = "\0root\0";

/// 해시 입력의 판. 덮는 것의 꼴을 바꾸면 올린다 — 옛 기록은 새 해시와 맞지 않아 한 번 다시 묻는다.
/// - 1: 구성 · argv · 작업 폴더 · 얹는 env · `env_file` 다이제스트 · 프로필 · 정의 본문.
/// - 2: 런타임 환경의 원천 파일(`runtime_files`)을 더했다(M6-11 리뷰 F1 — `mise.toml` 의 `[env]` 가 해시 밖이었다).
///   다이제스트를 M6-03 의 [`EnvFileDigest`] 로 바꾼 것(F3)은 직렬화가 같아(`{"path","sha256"}`) 판을 올리지 않았다.
const CONTENT_VERSION: u32 = 2;

/// 띄우는 쪽이 푼 실행 내용 — 실행을 띄울 때(또는 계획을 만들 때) 쓴 값 그대로다.
pub struct SeenParts<'a> {
    /// 구성 열쇠(구성 id 또는 [`detect::key`]).
    pub key: &'a str,
    /// 프로젝트 루트(활성 워크트리 안). 작업 폴더 · argv · 런타임 원천 파일의 경로를 이것 기준으로 적는다.
    pub root: &'a Path,
    pub config: &'a RunConfig,
    /// 띄울 argv(프로필을 얹은 것).
    pub argv: &'a [String],
    /// 작업 폴더(절대 경로 — 루트 안). 이 함수가 실제 경로로 푼다.
    pub cwd: &'a Path,
    /// 런타임 제공자가 멈추는 홈 폴더(`pw_runtime` 의 기준 환경 `HOME`). [`runtime_files`] 가 이 폴더 앞에서 멈춘다 — 홈의
    /// `~/.config/mise` 는 mise 의 전역 설정이다.
    pub home: Option<&'a Path>,
    /// M6-03 이 이 구성 · 프로필로 푼 env(얹는 변수 · `env_file` 다이제스트 · 프로필).
    pub env: &'a RunEnv,
    /// 명령 정의 본문(M6-04 의 감지 — [`needs_definition`] 이면 있어야 한다).
    pub definition: Option<&'a str>,
}

/// 해시하는 내용. [`content`] 가 채운다.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenContent {
    /// 구성 전체를 파싱한 값의 정규형.
    pub config: String,
    /// 풀린 argv. 루트 안의 절대 경로는 루트 기준으로 적는다.
    pub argv: Vec<String>,
    /// 작업 폴더(실제 경로를 루트 기준으로, `/` 구분).
    pub cwd: String,
    /// 런타임 환경 위에 얹는 env 의 이름과 값([`RunEnv::vars`]).
    pub env: BTreeMap<String, String>,
    /// `env_file` 마다의 경로와 내용 sha256([`RunEnv::files`], 구성의 순서).
    pub env_files: Vec<EnvFileDigest>,
    /// 고른 프로필.
    pub profile: Option<String>,
    /// 명령 정의 본문.
    pub definition: Option<String>,
    /// 런타임 환경의 원천 파일의 경로와 내용 sha256, 경로 순.
    pub runtime_files: Vec<EnvFileDigest>,
}

impl SeenContent {
    pub fn hash(&self) -> SeenHash {
        #[derive(Serialize)]
        struct Versioned<'a> {
            v: u32,
            content: &'a SeenContent,
        }
        // 구조체 필드 순서와 `BTreeMap` 의 순서가 고정이라 같은 내용은 같은 글이다(구성은 이미 정규형 글이다).
        let text = serde_json::to_string(&Versioned {
            v: CONTENT_VERSION,
            content: self,
        })
        .expect("seen content serializes to JSON");
        SeenHash::parse(&format!("sha256:{}", hex(&Sha256::digest(text.as_bytes()))))
            .expect("a sha256 hex digest is a seen hash")
    }
}

/// 값(argv · env 값 · 정의 본문 · 구성 글)을 보이지 않는다 — 로그 · 오류에 원문을 남기지 않는다(M5 의 규약, M6-03 ③).
impl fmt::Debug for SeenContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeenContent")
            .field("configLen", &self.config.len())
            .field("argc", &self.argv.len())
            .field("cwd", &self.cwd)
            .field("envKeys", &self.env.keys().collect::<Vec<_>>())
            .field("envFiles", &self.env_files)
            .field("profile", &self.profile)
            .field("definitionLen", &self.definition.as_ref().map(String::len))
            .field("runtimeFiles", &self.runtime_files)
            .finish()
    }
}

/// 이 열쇠 · 종류는 명령 정의 본문을 해시에 넣어야 하는가 — `npm` · `mise-task` · 감지된 태스크([`detect::parse_key`]).
pub fn needs_definition(key: &str, kind: &RunKindConfig) -> bool {
    detect::parse_key(key).is_some()
        || matches!(
            kind,
            RunKindConfig::Npm { .. } | RunKindConfig::MiseTask { .. }
        )
}

/// [`SeenContent`] 를 채운다. 빠진 것이 있으면(정의 본문, 이 구성의 것이 아닌 env, 루트 밖 · 없는 작업 폴더, 읽지 못한 런타임
/// 원천 파일) 오류다 — 부른 쪽은 「본 적 없음」 으로 다룬다(묻는다). 오류 글에 값을 쓰지 않는다.
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
    let read: Vec<&str> = parts.env.files().iter().map(|f| f.path.as_str()).collect();
    if listed != read {
        return Err(format!(
            "`{}`: the env files read do not match the configuration's env_file",
            parts.key
        ));
    }
    let outside = || {
        format!(
            "`{}`: the working directory is not a folder inside the project root",
            parts.key
        )
    };
    // 작업 폴더는 실제 경로로 풀어 실제 루트와 견준다 — 루트 안의 링크가 밖이나 다른 폴더를 가리키면 그것이 해시에 든다.
    let real_root = fs::canonicalize(parts.root).map_err(|_| outside())?;
    let real_cwd = fs::canonicalize(parts.cwd).map_err(|_| outside())?;
    let cwd = relative(&[&real_root], &real_cwd).ok_or_else(outside)?;
    // argv 의 래퍼 경로는 루트를 이어 붙인 글이라 루트를 글 그대로와 실제 경로 둘로 본다.
    let roots: [&Path; 2] = [parts.root, &real_root];
    let mut runtime = Vec::new();
    for path in runtime_files(&real_cwd, parts.home) {
        let contents = read_small(&path).map_err(|why| {
            format!(
                "`{}`: cannot read the runtime file {}: {why}",
                parts.key,
                path.display()
            )
        })?;
        runtime.push(EnvFileDigest {
            path: rooted(&roots, &path.to_string_lossy()),
            sha256: hex(&Sha256::digest(&contents)),
        });
    }
    runtime.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(SeenContent {
        config: config.canonical(),
        argv: parts.argv.iter().map(|arg| rooted(&roots, arg)).collect(),
        cwd,
        env: parts.env.vars().clone(),
        env_files: parts.env.files().to_vec(),
        profile: parts.env.profile().map(str::to_owned),
        definition: parts.definition.map(str::to_owned),
        runtime_files: runtime,
    })
}

/// [`content`] 의 해시.
pub fn hash(parts: &SeenParts<'_>) -> Result<SeenHash, String> {
    content(parts).map(|c| c.hash())
}

/// 복합 구성(17-m6-plan M6-21)의 「사람이 본 구성」 해시. 복합 구성은 프로세스가 없어 [`content`] 의 풀린 내용이 없다 — 덮는
/// 것은 열쇠와 구성 전체를 파싱한 값([`RunConfig::canonical`] — `configs` · `mode` · `before`)이다. 그것이 부르는 구성은 각자의
/// 해시로 본다(결정 D68 — 사슬이 자동이려면 모두가 사람이 본 것이어야 한다).
pub fn compound_hash(key: &str, config: &RunConfig) -> SeenHash {
    let text = serde_json::json!({
        "v": "pitwall.run.compound.v1",
        "key": key,
        "config": config.canonical(),
    })
    .to_string();
    SeenHash::parse(&format!("sha256:{}", hex(&Sha256::digest(text.as_bytes()))))
        .expect("a sha256 hex digest is a seen hash")
}

/// 런타임 원천 파일 하나의 상한. mise 설정은 작다 — 넘으면 해시하지 않는다(본 적 없음 — 묻는다).
const MAX_RUNTIME_FILE: u64 = 5 * 1024 * 1024;

/// 런타임 제공자(mise — `pw_runtime::mise`)가 `cwd` 에서 읽는 원천 파일: `cwd` 와 그 위 폴더(`home` 앞까지)의 mise 설정 파일 ·
/// 조각 · `.tool-versions`([`mise::config_files`])와, 바뀌면 런타임 환경이 달라지는 파일([`mise::is_runtime_file`] — 환경 이름이
/// 붙은 `mise.<env>.toml`, `.nvmrc` 같은 언어별 버전 파일). 런타임 환경을 다시 계산하는 파일 감시와 같은 목록이다. 경로 순.
pub fn runtime_files(cwd: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut found: BTreeSet<PathBuf> = mise::config_files(cwd, home).0.into_iter().collect();
    for folder in cwd.ancestors() {
        if home.is_some_and(|h| folder == h) {
            break;
        }
        let Ok(entries) = fs::read_dir(folder) else {
            continue;
        };
        for path in entries.filter_map(Result::ok).map(|e| e.path()) {
            if mise::is_runtime_file(&path) && path.is_file() {
                found.insert(path);
            }
        }
    }
    found.into_iter().collect()
}

fn read_small(path: &Path) -> Result<Vec<u8>, String> {
    let len = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_RUNTIME_FILE {
        return Err(format!("larger than {MAX_RUNTIME_FILE} bytes"));
    }
    fs::read(path).map_err(|e| e.to_string())
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
