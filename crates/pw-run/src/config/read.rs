//! 실행 구성 읽기 — 설정 계층(03 「설정 계층」 · D9)의 `run/<id>` 파일을 **파일 하나씩** 읽는다.
//!
//! 레이어 · 폴백 · 병합은 `pw_core::config::ConfigSource` 와 같다: 공유 `run/<id>.toml` 위에 개인 `run/<id>.local.toml` 을 키별로
//! 덮고(`pw_core::config::merge`), 연결 워크트리면 개인 파일이 없을 때 메인 체크아웃의 것을 쓴다(파일 단위). 개인 전용 프로젝트는
//! 같은 구조가 `$CONFIG/projects/<slug>/run/` 에 있다. 디스크에 쓰지 않는다(`ConfigSource::without_writes` 와 같이 구버전 파일은
//! 메모리에서만 올린다) — 목록만 봐도 커밋된 파일이 바뀌면 안 된다.
//!
//! `ConfigSource::load` 와 다른 것 하나: 그것은 파일 하나라도 깨지면 전체가 실패하지만, 여기는 구성마다 따로 판정해 깨진 것을
//! 「쓸 수 없음(까닭)」 으로 남긴다(외부 도구 목록과 같다 — 고치려던 구성이 목록에서 사라지고 다른 것이 돌면 안 된다). 모르는
//! `type` · TOML 형식 오류 · 앱보다 새 `schema` · 검증 실패 · 쓸 수 없는 id · 대상 없는 링크가 모두 그렇다.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use pw_core::config::merge::{merge, to_json};
use pw_core::config::migrate::{FileMigrateError, MigrateError, Schemas, migrate_in_memory};
use pw_core::config::{AppPaths, ConfigSource, LayerOrigin};
use pw_core::registry::path::{WorktreeRootError, worktree_root};
use pw_core::registry::{ConfigMode, Project};
use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{DIR, FILE_KEYS, RunConfig, valid_id, validate};

/// 한 프로젝트(활성 워크트리 하나)의 실행 구성 자리.
#[derive(Clone)]
pub struct RunConfigPlace {
    pub project_id: String,
    /// 활성 워크트리 안의 프로젝트 루트(없으면 주 루트). `cwd` · `env_file` 의 기준.
    pub root: PathBuf,
    /// 활성 워크트리의 설정(공유 파일을 읽고 쓰는 곳, 연결 워크트리면 개인 파일은 메인 체크아웃으로 폴백).
    pub(crate) source: ConfigSource,
    /// 저장소 안의 `.pitwall/` 인가(개인 전용 폴더가 아닌가) — 쓸 때 링크를 따르지 않는다(`write` 머리 주석).
    pub(crate) repository: bool,
}

impl RunConfigPlace {
    /// `worktree` 는 창의 활성 워크트리 꼭대기(`ActiveWorktree.top`). 그 프로젝트의 연결 워크트리가 아니면 invalid params.
    pub fn new(
        paths: &AppPaths,
        project: &Project,
        worktree: Option<&str>,
    ) -> Result<Self, RpcError> {
        let root = match worktree {
            None => project.root().to_path_buf(),
            Some(top) => worktree_root(project.root(), Path::new(top)).map_err(|e| match e {
                WorktreeRootError::Relative => {
                    RpcError::invalid_params(format!("worktree `{top}` must be an absolute path"))
                }
                WorktreeRootError::NotOurs => RpcError::invalid_params(format!(
                    "`{top}` is not a worktree of project `{}`",
                    project.id
                )),
            })?,
        };
        let config_error = |e: pw_core::config::ConfigError| RpcError::internal(e.to_string());
        Ok(Self {
            project_id: project.id.clone(),
            source: project.config_source(paths, &root).map_err(config_error)?,
            repository: project.config_mode == ConfigMode::Repository,
            root,
        })
    }

    /// 공유 파일들의 폴더(`<활성 워크트리 루트>/.pitwall/run` 또는 개인 전용 폴더의 `run`).
    pub fn shared_dir(&self) -> PathBuf {
        self.source.dir().join(DIR)
    }
}

/// 레이어 파일 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigLayer {
    /// `shared`(`<id>.toml`) · `local`(`<id>.local.toml`).
    pub origin: LayerOrigin,
    pub path: String,
}

/// 쓸 수 있는 구성 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedRunConfig {
    pub id: String,
    pub config: RunConfig,
    /// 아래(공유)부터.
    pub layers: Vec<RunConfigLayer>,
}

/// 쓸 수 없는 구성 하나 — 목록에 남는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct UnusableRunConfig {
    pub id: String,
    /// 파일의 `name`(읽을 수 있었으면). 실행 선택기가 id 대신 보인다.
    pub name: Option<String>,
    /// 왜 쓸 수 없는가(어느 파일 · 무엇). 환경 변수 값은 담지 않는다.
    pub reason: String,
    pub layers: Vec<RunConfigLayer>,
}

/// 한 프로젝트의 구성들(id 순).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunConfigSet {
    pub configs: Vec<LoadedRunConfig>,
    pub unusable: Vec<UnusableRunConfig>,
    /// 폴더 자체를 읽지 못한 까닭(권한 등). 그러면 그 폴더의 구성은 목록에 없다.
    pub problem: Option<String>,
}

impl RunConfigSet {
    pub fn get(&self, id: &str) -> Option<&LoadedRunConfig> {
        self.configs.iter().find(|c| c.id == id)
    }

    /// 쓸 수 있든 없든 그 id 가 목록에 있는가.
    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some() || self.unusable.iter().any(|u| u.id == id)
    }

    /// 대소문자만 다른 다른 구성(대소문자를 가리지 않는 파일 시스템에서는 같은 파일이다).
    pub fn case_twin(&self, id: &str) -> Option<&str> {
        self.configs
            .iter()
            .map(|c| c.id.as_str())
            .chain(self.unusable.iter().map(|u| u.id.as_str()))
            .find(|other| *other != id && other.eq_ignore_ascii_case(id))
    }
}

/// 찾은 파일(아직 읽지 않음).
#[derive(Default)]
struct Files {
    shared: Option<PathBuf>,
    local: Option<PathBuf>,
}

/// 프로젝트의 실행 구성을 모두 읽는다. 디스크에 쓰지 않는다.
pub fn read(place: &RunConfigPlace) -> RunConfigSet {
    let mut set = RunConfigSet::default();
    let mut found: BTreeMap<String, Files> = BTreeMap::new();
    let mut problems = Vec::new();
    let primary = place.source.dir().join(DIR);
    match list(&primary) {
        Ok(files) => {
            for (id, local, path) in files {
                let entry = found.entry(id).or_default();
                if local {
                    entry.local = Some(path);
                } else {
                    entry.shared = Some(path);
                }
            }
        }
        Err(e) => problems.push(e),
    }
    if let Some(fallback) = place.source.local_fallback_dir() {
        match list(&fallback.join(DIR)) {
            Ok(files) => {
                for (id, local, path) in files {
                    if local {
                        let entry = found.entry(id).or_default();
                        if entry.local.is_none() {
                            entry.local = Some(path);
                        }
                    }
                }
            }
            Err(e) => problems.push(e),
        }
    }
    for (id, files) in found {
        match load_one(&place.root, &id, &files) {
            Ok(loaded) => set.configs.push(loaded),
            Err(unusable) => set.unusable.push(*unusable),
        }
    }
    if !problems.is_empty() {
        set.problem = Some(problems.join("; "));
    }
    set
}

fn layers_of(files: &Files) -> Vec<RunConfigLayer> {
    [
        (LayerOrigin::Shared, &files.shared),
        (LayerOrigin::Local, &files.local),
    ]
    .into_iter()
    .filter_map(|(origin, path)| {
        path.as_ref().map(|p| RunConfigLayer {
            origin,
            path: p.display().to_string(),
        })
    })
    .collect()
}

fn load_one(
    root: &Path,
    id: &str,
    files: &Files,
) -> Result<LoadedRunConfig, Box<UnusableRunConfig>> {
    let layers = layers_of(files);
    let unusable = |name: Option<String>, reason: String| {
        Box::new(UnusableRunConfig {
            id: id.to_owned(),
            name,
            reason,
            layers: layers.clone(),
        })
    };
    let mut value = Map::new();
    let schemas = Schemas::builtin();
    for path in [&files.shared, &files.local].into_iter().flatten() {
        let layer = read_layer(path, schemas.get(DIR)).map_err(|reason| unusable(None, reason))?;
        merge(&mut value, layer);
    }
    let name = value.get("name").and_then(Value::as_str).map(str::to_owned);
    if !valid_id(id) {
        return Err(unusable(
            name,
            format!("the id `{id}` (the file name) must be 1-64 letters, digits, `-` or `_`"),
        ));
    }
    let config = from_file_value(value).map_err(|reason| unusable(name.clone(), reason))?;
    validate(&config, root).map_err(|reason| unusable(name.clone(), reason))?;
    Ok(LoadedRunConfig {
        id: id.to_owned(),
        config,
        layers,
    })
}

/// 레이어 파일 하나의 값(`schema` 를 뺀 것). 구버전은 메모리에서만 올린다.
fn read_layer(
    path: &Path,
    migrator: Option<&pw_core::config::migrate::Migrator>,
) -> Result<Map<String, Value>, String> {
    match migrate_in_memory(path, migrator) {
        Ok(m) => {
            let mut map = to_json(m.doc.as_table());
            map.remove(pw_core::config::migrate::SCHEMA_KEY);
            Ok(map)
        }
        Err(FileMigrateError::Migrate {
            error: MigrateError::FutureVersion { found, supported },
            ..
        }) => Err(format!(
            "{}: `schema = {found}` is newer than this app reads (up to {supported}); update Pitwall to use it",
            path.display()
        )),
        Err(e) => Err(e.to_string()),
    }
}

/// 병합한 파일 값(파일 키 — snake_case)을 구성으로. 검증([`validate`])은 하지 않는다.
pub fn from_file_value(mut value: Map<String, Value>) -> Result<RunConfig, String> {
    for (file, rpc) in FILE_KEYS {
        // RPC 이름으로 적은 키는 파일 키가 아니다 — 모르는 키로 두면 조용히 무시되므로 알린다.
        if value.contains_key(*rpc) {
            return Err(format!("unknown key `{rpc}`; the file key is `{file}`"));
        }
        if let Some(v) = value.remove(*file) {
            value.insert((*rpc).to_owned(), v);
        }
    }
    serde_json::from_value(Value::Object(value)).map_err(|e| e.to_string())
}

/// 구성을 파일 값(파일 키 — snake_case)으로. 없는 값(`null`)은 뺀다.
pub fn to_file_value(config: &RunConfig) -> Map<String, Value> {
    let Value::Object(mut map) = serde_json::to_value(config).expect("a run config serializes")
    else {
        unreachable!("a run config is an object")
    };
    for (file, rpc) in FILE_KEYS {
        if let Some(v) = map.remove(*rpc) {
            map.insert((*file).to_owned(), v);
        }
    }
    map
}

/// `dir/*.toml` 의 (id, 개인 파일인가, 경로). 숨김 파일은 건너뛴다. 폴더가 없으면 빈 목록. 대상 없는 심링크도 목록에 든다 — 읽을
/// 때 「쓸 수 없음」 이 된다(조용히 빠지면 「구성이 사라졌다」 로만 보인다).
fn list(dir: &Path) -> Result<Vec<(String, bool, PathBuf)>, String> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot list {}: {e}", dir.display())),
    };
    let mut out = Vec::new();
    for item in read {
        let item = item.map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
        let name = item.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let Some(stem) = name.strip_suffix(".toml") else {
            continue;
        };
        let path = item.path();
        match fs::metadata(&path) {
            Ok(meta) if !meta.is_file() => continue,
            _ => {}
        }
        let (id, local) = match stem.strip_suffix(".local") {
            Some(id) => (id, true),
            None => (stem, false),
        };
        out.push((id.to_owned(), local, path));
    }
    Ok(out)
}
