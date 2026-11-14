//! `settings_get` 툴 메서드(`docs/design/16-m5-plan.md` M5-09 ①, `docs/design/05-mcp.md` 「프로젝트」). `read`, 그룹 `project`, 코어
//! 툴이라 헤드리스 Host 에도 붙는다(결정 M5-D3).
//!
//! - **디스크에 쓰지 않고 읽는다**([`ConfigSource::without_writes`]). 조회 툴이 구버전 파일을 마이그레이션해 저장소의 커밋된
//!   `.pitwall/project.toml` 을 고치거나 `.backup/` 을 만들면 안 된다(05 「툴 레지스트리」 의 `project_info` 와 같은 까닭). 그래서
//!   `config.get`(Host 가 연 프로젝트의 `ConfigStore` — 「설정을 붙여 연 프로젝트만」)을 쓰지 않고 부를 때마다 파일을 새로 읽는다.
//! - `projectId` 가 없으면 전역 설정(`$CONFIG/settings.toml` · `settings.local.toml`), 있으면 그 프로젝트의 병합된 설정 — 전역 위에
//!   프로젝트 설정(`project.toml` · `project.local.toml`)을 얹는다(병합 규칙은 [`super::merge::merge`]). 어느 키를 프로젝트 설정에서
//!   읽는지는 기능마다 다르다(`[open_with.tools]` 는 프로젝트에서 버린다 등) — 툴은 파일에 적힌 것을 병합해 보일 뿐이다.
//! - 값마다 어느 레이어에서 왔는지 싣는다([`SettingLayer`]).
//! - **비밀**: 비밀은 설정 파일에 두지 않지만(키체인 · env 참조 — 06-roadmap.md D47) 키 이름으로 한 번 더 거른다
//!   ([`crate::secret::is_secret_key`] — 감사 로그의 인자 요약과 같은 한 벌). 실행 파일을 정하는 키(`[git] path` ·
//!   `[open_with.tools]` · `[lsp.<id>]` 의 명령)의 값은 그대로 싣는다 — 에이전트가 파일을 직접 읽는 것과 같은 범위다(메인 판정
//!   2026-11-14, 16-m5-plan 결정 M5-D9 곁).
//! - 설정 파일을 읽지 못한 까닭(TOML 파싱 오류)은 응답에 싣지 않는다 — 파서의 오류 글은 문제 줄의 값을 인용한다. 어느 파일인지만
//!   알린다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pw_rpc::{ConnectionScope, JsonValue, Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::merge::merge;
use super::{
    AppPaths, ConfigError, ConfigLayer, ConfigSource, LayerOrigin, LayerValue, LayeredSnapshot,
};
use crate::registry::path::{WorktreeRootError, worktree_root};
use crate::registry::{ConfigMode, Registry};
use crate::secret::{REDACTED, is_secret_key};

/// 전역 설정 파일 id.
const GLOBAL_FILE: &str = "settings";
/// 프로젝트 설정 파일 id.
const PROJECT_FILE: &str = "project";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SettingsGetToolParams {
    /// 없으면(`null`) 전역 설정, 있으면 그 프로젝트의 병합된 설정(전역 위에 프로젝트 설정). 세션에 프로젝트가 있으면 키를 빼면
    /// 세션 프로젝트다 — 전역만 보려면 `null` 을 준다.
    #[serde(default)]
    pub project_id: Option<String>,
    /// 볼 워크트리의 꼭대기(절대 경로). 비면 세션의 워크트리, 그것도 없으면 프로젝트의 메인 체크아웃. 커밋된
    /// `.pitwall/project.toml` 은 워크트리(브랜치)마다 다를 수 있다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 이 키와 그 아래만. 표와 키를 점으로 잇는다(`mcp`, `mcp.tools`, `open_with.default`). 점 · 공백이 든 키 이름은 결과의 `key`
    /// 처럼 큰따옴표로 감싼다.
    #[serde(default)]
    pub key: Option<String>,
}

/// 값이 온 레이어. 아래(먼저 적용)부터 `builtin` → `global` → `shared` → `personal` 이다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SettingLayer {
    /// 앱의 내장 기본값.
    Builtin,
    /// 전역 설정(`$CONFIG/settings.toml` · `settings.local.toml`).
    Global,
    /// 저장소에 커밋되는 프로젝트 설정(`.pitwall/project.toml`).
    Shared,
    /// 커밋되지 않는 개인 프로젝트 설정(`.pitwall/project.local.toml`, 개인 전용 프로젝트의 `$CONFIG/projects/<slug>/` 파일).
    Personal,
}

/// 값 하나(테이블이 아닌 것 — 배열은 통째로 한 값이다).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingEntry {
    /// 표와 키를 점으로 이은 것(`mcp.preset`). 점 · 공백 등이 든 키 이름은 큰따옴표로 감싼다(`mcp.tools."project_*"`).
    pub key: String,
    /// 이긴 레이어의 값. `redacted` 면 가린 값이다.
    #[specta(type = JsonValue)]
    pub value: Value,
    pub layer: SettingLayer,
    /// 키 이름이 비밀을 가리켜(`token` · `password` · `apiKey` …) 값(또는 그 안의 비밀 키 값)을 가렸다.
    pub redacted: bool,
}

/// 읽은 설정 파일 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFileInfo {
    pub layer: SettingLayer,
    pub path: String,
    pub schema: u32,
    /// 앱보다 새 스키마라 읽기만 한 파일(값은 그대로 싣는다).
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsGetToolResult {
    /// 프로젝트의 설정이면 그 id. 전역이면 `null`.
    pub project_id: Option<String>,
    /// 키 이름순.
    pub entries: Vec<SettingEntry>,
    /// 값을 읽은 설정 파일(아래 레이어부터). 내장 기본값은 없다.
    pub files: Vec<SettingsFileInfo>,
}

rpc_method! {
    /// 설정 키 조회 — 디스크에 쓰지 않고 읽는다. `projectId` 가 없으면(`null`) 전역 설정, 있으면 그 프로젝트의 병합된 설정(전역
    /// 위에 프로젝트 설정). `key`(`표.키` 접두)로 좁힌다. 값마다 어느 레이어(builtin · global · shared · personal)에서 왔는지 싣는다.
    /// 키 이름이 비밀을 가리키는 값은 가린다. 그 밖의 값은 설정 파일 그대로다 — 에이전트가 파일을 직접 읽는 것과 같은 범위.
    pub ConfigToolGet: "config.tool.get" {
        params: SettingsGetToolParams,
        result: SettingsGetToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "project", name: "settings_get" },
    }
}

/// 핸들러를 붙인다. 레지스트리 · 설정 파일을 디스크에서 읽으므로 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, paths: AppPaths, registry: Registry) {
    router.unary(ConfigToolGet, move |ctx, params: SettingsGetToolParams| {
        let (paths, registry, scope) = (paths.clone(), registry.clone(), ctx.scope().clone());
        async move {
            tokio::task::spawn_blocking(move || settings_get(&scope, &paths, &registry, &params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
}

/// `settings_get` 의 본체.
pub fn settings_get(
    scope: &ConnectionScope,
    paths: &AppPaths,
    registry: &Registry,
    params: &SettingsGetToolParams,
) -> Result<SettingsGetToolResult, RpcError> {
    let mut layers: Vec<(SettingLayer, Map<String, Value>)> = Vec::new();
    let mut files = Vec::new();
    let global = ConfigSource::global(paths)
        .without_writes()
        .load_layered()
        .map_err(read_error)?;
    collect(
        &global,
        GLOBAL_FILE,
        |_| SettingLayer::Global,
        &mut layers,
        &mut files,
    );
    let project_id = match &params.project_id {
        None => {
            if !matches!(scope, ConnectionScope::Agent) {
                return Err(not_agent_session());
            }
            if params.worktree.is_some() {
                return Err(RpcError::invalid_params(
                    "`worktree` needs a `projectId`".to_owned(),
                ));
            }
            None
        }
        Some(id) => {
            let grant = crate::registry::agent::agent_grant(scope, registry, id)?;
            let project = grant.project();
            let root = match params.worktree.as_deref().filter(|w| !w.is_empty()) {
                None => project.root().to_path_buf(),
                Some(top) => checkout_root(project.root(), top, id)?,
            };
            let source = project
                .config_source(paths, &root)
                .map_err(read_error)?
                .without_writes();
            let loaded = source.load_layered().map_err(read_error)?;
            let personal_project = project.config_mode == ConfigMode::Personal;
            collect(
                &loaded,
                PROJECT_FILE,
                |origin| match origin {
                    LayerOrigin::Shared if !personal_project => SettingLayer::Shared,
                    _ => SettingLayer::Personal,
                },
                &mut layers,
                &mut files,
            );
            Some(id.clone())
        }
    };
    let filter = params
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty());
    Ok(SettingsGetToolResult {
        project_id,
        entries: entries(&layers, filter),
        files,
    })
}

/// 스냅숏의 `id` 파일 레이어들을 `layers` · `files` 에 더한다. 내장 기본값은 [`SettingLayer::Builtin`], 나머지는 `of` 가 정한다.
fn collect(
    loaded: &LayeredSnapshot,
    id: &str,
    of: impl Fn(LayerOrigin) -> SettingLayer,
    layers: &mut Vec<(SettingLayer, Map<String, Value>)>,
    files: &mut Vec<SettingsFileInfo>,
) {
    let layer_of = |origin| match origin {
        LayerOrigin::Builtin => SettingLayer::Builtin,
        other => of(other),
    };
    for LayerValue { origin, value } in loaded.layers.get(id).into_iter().flatten() {
        layers.push((layer_of(*origin), value.clone()));
    }
    let read = loaded.snapshot.file(id).map(|f| f.layers.as_slice());
    for ConfigLayer {
        origin,
        path,
        schema,
        read_only,
    } in read.into_iter().flatten()
    {
        if let Some(path) = path {
            files.push(SettingsFileInfo {
                layer: layer_of(*origin),
                path: path.clone(),
                schema: *schema,
                read_only: *read_only,
            });
        }
    }
}

/// 레이어들을 아래부터 병합한 값을 키마다 펼치고, 그 키를 마지막으로 정한 레이어를 붙인다. 병합 규칙([`merge`])과 같게 센다 —
/// 위 레이어의 스칼라 · 배열은 아래의 같은 자리(그 아래 테이블 포함)를 통째로 이기고, 테이블끼리는 키별로 섞인다.
fn entries(
    layers: &[(SettingLayer, Map<String, Value>)],
    filter: Option<&str>,
) -> Vec<SettingEntry> {
    let mut merged = Map::new();
    let mut from: BTreeMap<Vec<String>, SettingLayer> = BTreeMap::new();
    for (layer, value) in layers {
        provenance(value, *layer, &mut Vec::new(), &mut from);
        merge(&mut merged, value.clone());
    }
    let mut leaves = Vec::new();
    flatten(&merged, &mut Vec::new(), &mut leaves);
    leaves
        .into_iter()
        .filter_map(|(path, value)| {
            let key = render_key(&path);
            if let Some(filter) = filter
                && !(key == filter
                    || key
                        .strip_prefix(filter)
                        .is_some_and(|rest| rest.starts_with('.')))
            {
                return None;
            }
            let layer = from.get(&path).copied().unwrap_or(SettingLayer::Builtin);
            let (value, redacted) = if path.iter().any(|segment| is_secret_key(segment)) {
                (Value::String(REDACTED.to_owned()), true)
            } else {
                redact(value)
            };
            Some(SettingEntry {
                key,
                value,
                layer,
                redacted,
            })
        })
        .collect()
}

fn provenance(
    map: &Map<String, Value>,
    layer: SettingLayer,
    prefix: &mut Vec<String>,
    from: &mut BTreeMap<Vec<String>, SettingLayer>,
) {
    for (key, value) in map {
        prefix.push(key.clone());
        match value {
            Value::Object(inner) => {
                // 아래 레이어가 이 자리에 둔 스칼라는 테이블로 바뀐다(병합은 테이블이 통째로 이긴다).
                from.remove(prefix.as_slice());
                provenance(inner, layer, prefix, from);
            }
            _ => {
                // 아래 레이어가 이 자리에 둔 테이블은 통째로 진다.
                from.retain(|path, _| !(path.len() > prefix.len() && path.starts_with(prefix)));
                from.insert(prefix.clone(), layer);
            }
        }
        prefix.pop();
    }
}

/// 테이블이 아닌 값마다 (경로, 값). 빈 테이블은 값이 없어 빠진다.
fn flatten(
    map: &Map<String, Value>,
    prefix: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, Value)>,
) {
    for (key, value) in map {
        prefix.push(key.clone());
        match value {
            Value::Object(inner) => flatten(inner, prefix, out),
            other => out.push((prefix.clone(), other.clone())),
        }
        prefix.pop();
    }
}

/// 배열 안의 테이블(`[[...]]`)에 든 비밀 키의 값을 가린다. 가린 것이 있으면 `true`.
fn redact(value: Value) -> (Value, bool) {
    match value {
        Value::Object(map) => {
            let mut hidden = false;
            let map = map
                .into_iter()
                .map(|(k, v)| {
                    if is_secret_key(&k) {
                        hidden = true;
                        (k, Value::String(REDACTED.to_owned()))
                    } else {
                        let (v, h) = redact(v);
                        hidden |= h;
                        (k, v)
                    }
                })
                .collect();
            (Value::Object(map), hidden)
        }
        Value::Array(items) => {
            let mut hidden = false;
            let items = items
                .into_iter()
                .map(|v| {
                    let (v, h) = redact(v);
                    hidden |= h;
                    v
                })
                .collect();
            (Value::Array(items), hidden)
        }
        other => (other, false),
    }
}

/// TOML 의 점 키 꼴: 맨 키(`A-Za-z0-9_-`)는 그대로, 아니면 큰따옴표로 감싼다.
fn render_key(path: &[String]) -> String {
    path.iter()
        .map(|segment| {
            let bare = !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if bare {
                segment.clone()
            } else {
                Value::String(segment.clone()).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// 워크트리 꼭대기 → 그 워크트리 안의 프로젝트 루트(`openWith.list` 와 같은 판정).
fn checkout_root(project_root: &Path, top: &str, project_id: &str) -> Result<PathBuf, RpcError> {
    worktree_root(project_root, Path::new(top)).map_err(|e| match e {
        WorktreeRootError::Relative => {
            RpcError::invalid_params(format!("worktree `{top}` must be an absolute path"))
        }
        WorktreeRootError::NotOurs => RpcError::invalid_params(format!(
            "`{top}` is not a worktree of project `{project_id}`"
        )),
    })
}

/// 설정 파일을 읽지 못했다. 파서의 오류 글은 싣지 않는다(문제 줄의 값을 인용한다) — 어느 파일인지와 종류만.
fn read_error(e: ConfigError) -> RpcError {
    let what = match &e {
        ConfigError::Io { .. } => "cannot be read",
        ConfigError::Parse { .. } => "is not valid TOML",
        ConfigError::Migrate { .. } => "has a schema this app cannot read",
        ConfigError::InvalidSlug(_) => "has an invalid personal settings folder",
    };
    let file = e.path().map_or_else(
        || "the project settings".to_owned(),
        |p| format!("`{}`", p.display()),
    );
    RpcError::internal(format!(
        "{file} {what}; open it in Pitwall or an editor to see the error"
    ))
}

/// MCP 세션이 아닌 연결이 툴 메서드를 불렀다(`agent_grant` 의 거절과 같은 꼴, 프로젝트가 없을 뿐이다).
fn not_agent_session() -> RpcError {
    RpcError::new(
        code::PERMISSION_DENIED,
        "only an mcp session may call agent tools",
    )
    .with_data(serde_json::json!({ "reason": "notAgentSession" }))
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
