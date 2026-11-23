//! 설정 창 「MCP」 쪽(16-m5-plan.md M5-15)의 읽기 · 쓰기: `mcp.settings.get { projectId? }` · `mcp.settings.set`.
//!
//! - **쓰기는 전역 `[mcp]` 만**이다(결정 M5-D36). 프로젝트 설정(공유 `.pitwall/project.toml` · `.local.toml` · 개인 전용
//!   `project.toml`)은 이 RPC 로 쓰지 않는다 — 프로젝트가 덮은 값은 [`McpSettingsView::project`] 로 보이기만 하고 화면은 그
//!   파일을 연다. 쓰는 키는 다섯이다: `preset` · `auto_approve` · `[mcp.groups] <그룹>` · `[mcp.tools] <툴>` ·
//!   `[mcp.audit] retention_days`. `[mcp.policy]`(승인 시한 · 프로덕션)는 이 화면의 것이 아니다(계획 행에 없다).
//! - **값 판정은 Host 가 한다** — 화면이 막지 않는다. 판정은 Host 가 설정을 읽는 함수와 같은 규칙이다: 프리셋은
//!   [`Preset::parse`], 그룹은 [`GROUPS`], 툴은 이 Host 의 카탈로그에 있는 이름(glob 키는 쓰지 않는다), 자동 승인은 등급 이름이고
//!   `dangerous` 는 거절, 보관 일수는 [`audit::retention_days`] 가 경고 없이 받는 값(0 이상 정수 — 0 은 지우지 않음) 가운데
//!   [`MAX_RETENTION_DAYS`](3650 — 10년) 이하다. 상한은 이 쓰기의 것이다 — 사람이 파일에 손으로 쓴 더 큰 값은 Host 가 그대로
//!   읽고(M5-11 의 읽기) 화면도 그 값을 보인다. 거절은
//!   `INVALID_PARAMS` 이고 `data` 는 [`McpSettingRefused`](창이 `reason` 을 번역한다).
//! - 쓰기는 그 키 하나만 고친다(`pw_core::config::write::write_key` — 주석 · 다른 키 보존, 값이 같으면 쓰지 않음). 바꿨으면
//!   전역 설정을 바로 다시 읽어 `config.changed` 가 나가고, 열린 MCP 세션이 노출을 다시 읽어 보이는 툴이 바뀌면
//!   `tools/list_changed` 를 보낸다(`pw_mcp::server` 의 `exposure_feed`).
//! - **읽기의 값은 Host 의 판정과 같은 함수로 낸다**(메인 판정 2026-11-17): 프리셋 · 그룹은 [`Exposure::from_layers`], 자동 승인은
//!   [`policy::read`] · [`policy::read_project`], 보관 일수는 [`audit::retention_days`]. 툴 하나하나의 켜짐과 그 까닭은
//!   `mcp.tools.list` 의 판정이다 — 이 RPC 는 그것을 다시 내지 않는다.

use std::path::Path;

use pw_core::config::LayerOrigin;
use pw_core::config::write::{Links, SETTINGS_FILE, WriteError, write_key};
use pw_core::registry::{ConfigMode, Project};
use pw_rpc::{JsonValue, RpcError, Tier, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::audit::{self, DEFAULT_RETENTION_DAYS};
use crate::catalog::GROUPS;
use crate::exposure::{Exposure, KEY, Preset};
use crate::http;
use crate::methods::{HostParts, Mcp};
use crate::policy::{self, AUTO_APPROVE};

/// 설정 창이 쓸 수 있는 감사 보관 일수의 상한(10년, 메인 판정 2026-11-17 — M5-15 리뷰). 0(지우지 않음)은 따로 받는다.
pub const MAX_RETENTION_DAYS: u32 = 3650;

/// 설정 파일 종류(`settings.toml` 이 부르는 이름 — `pw_core::config::write`).
const SETTINGS_KIND: &str = "settings";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpSettingsGetParams {
    /// 그 프로젝트가 덮은 값도 싣는다(읽기만 — 결정 M5-D36). 없으면 전역만.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// 전역 `[mcp]` 의 지금 값(Host 가 판정에 쓰는 값)과, 고른 프로젝트가 덮은 것.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpSettingsView {
    /// 전역 설정 파일(`$CONFIG/settings.toml`) — 「파일로 열기」 가 여는 경로. 아직 없을 수 있다.
    pub file: String,
    /// 지금 프리셋(`read-only` · `standard` · `full`). 파일 값이 잘못됐으면 Host 가 쓰는 `read-only` 다.
    pub preset: String,
    /// 묻지 않고 허용하는 등급(`dangerous` 는 들지 않는다).
    pub auto_approve: Vec<Tier>,
    /// [`GROUPS`] 차례. `enabled` 가 `null` 이면 그 그룹 키가 없다(툴마다 프리셋을 따른다).
    pub groups: Vec<McpGroupSetting>,
    /// 감사 로그 보관 일수(0 은 지우지 않음).
    pub retention_days: u32,
    pub default_retention_days: u32,
    /// 이 화면이 쓸 수 있는 보관 일수의 상한([`MAX_RETENTION_DAYS`]).
    pub max_retention_days: u32,
    /// 전역 `[mcp]` 를 읽다 버린 값의 경고(Host 의 영어 문장 그대로).
    pub warnings: Vec<String>,
    /// `projectId` 를 줬을 때만.
    pub project: Option<McpProjectSettings>,
    /// Streamable HTTP(`[mcp.http]` — 18-m7-plan M7-25)의 지금 상태. 이 Host 에 HTTP 가 붙어 있지 않으면(헤드리스 · 시험) `null`.
    /// 토큰은 싣지 않는다 — `mcp.http.token` 으로 따로 읽는다.
    pub http: Option<crate::http::McpHttpStatus>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpGroupSetting {
    pub group: String,
    pub enabled: Option<bool>,
}

/// 프로젝트가 덮은 `[mcp]`(읽기만).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpProjectSettings {
    pub project_id: String,
    /// 그 프로젝트의 설정 파일(「파일로 열기」 가 여는 경로 — 화면은 경로를 조립하지 않는다).
    pub files: Vec<McpSettingsFile>,
    /// 그 프로젝트 파일들이 `[mcp]` 아래에 적은 키(`preset` · `auto_approve` · `groups.db` · `tools.db_query` …)와 값, 적은 파일.
    /// 공유 파일의 값은 좁히기만 한다 — 툴의 실제 켜짐은 `mcp.tools.list { projectId }` 의 판정이다.
    pub overrides: Vec<McpSettingOverride>,
    /// 그 프로젝트에서 자동 승인되는 등급(공유 설정은 넓히지 못한다 — `policy::read_project`).
    pub auto_approve: Vec<Tier>,
    /// 그 프로젝트의 `[mcp]` 를 읽다 버린 값 · 효과 없는 표의 경고(Host 의 영어 문장 그대로).
    pub warnings: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum McpSettingsFileKind {
    /// 커밋되는 `.pitwall/project.toml`(좁히기만 한다).
    Shared,
    /// 개인 덮어쓰기 `.pitwall/project.local.toml`.
    Local,
    /// 개인 전용 프로젝트의 `$CONFIG/projects/<slug>/project.toml`.
    Personal,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpSettingsFile {
    pub kind: McpSettingsFileKind,
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpSettingOverride {
    /// `[mcp]` 아래의 점으로 이은 키(`groups.db`).
    pub key: String,
    #[specta(type = JsonValue)]
    pub value: Value,
    pub file: McpSettingsFileKind,
}

/// `mcp.settings.set` 이 쓰는 키.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum McpSettingKey {
    /// `[mcp] preset` — 값은 `read-only` · `standard` · `full`.
    Preset,
    /// `[mcp] auto_approve` — 값은 등급 이름 목록(`read` · `write` · `exec`).
    AutoApprove,
    /// `[mcp.groups] <name>` — 값은 bool.
    Group,
    /// `[mcp.tools] <name>` — 값은 bool. `name` 은 이 Host 의 툴 이름(glob 은 받지 않는다).
    Tool,
    /// `[mcp.audit] retention_days` — 값은 0 ~ [`MAX_RETENTION_DAYS`] 정수(0 은 지우지 않음).
    RetentionDays,
    /// `[mcp.http] enabled` — 값은 bool(18-m7-plan M7-25 — 기본 꺼짐).
    HttpEnabled,
    /// `[mcp.http] port` — 값은 1 ~ 65535 정수.
    HttpPort,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpSettingsSetParams {
    pub key: McpSettingKey,
    /// `group` · `tool` 의 그룹 · 툴 이름. 다른 키는 비운다.
    #[serde(default)]
    pub name: Option<String>,
    /// 새 값. `null` 이면 파일에서 그 키를 지운다(기본값으로).
    #[specta(type = Option<JsonValue>)]
    #[serde(default)]
    pub value: Option<Value>,
}

/// 거절의 까닭(창이 번역한다).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum McpSettingRefusal {
    /// 모르는 프리셋.
    UnknownPreset,
    /// `auto_approve` 에 `dangerous` — 위험 등급은 늘 묻는다.
    DangerousAutoApprove,
    /// `auto_approve` 에 등급이 아닌 것.
    UnknownTier,
    /// 모르는 그룹.
    UnknownGroup,
    /// 이 Host 에 없는 툴.
    UnknownTool,
    /// 보관 일수가 0 ~ [`MAX_RETENTION_DAYS`](3650) 정수가 아니다.
    BadRetentionDays,
    /// 포트가 1 ~ 65535 정수가 아니다.
    BadPort,
    /// 그 키의 꼴이 아니다(bool 자리에 글 · `name` 이 없음 등).
    BadValue,
}

/// `mcp.settings.set` 거절의 `data`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpSettingRefused {
    pub key: McpSettingKey,
    pub reason: McpSettingRefusal,
}

rpc_method! {
    /// 설정 창 「MCP」 쪽의 값: 전역 `[mcp]` 의 프리셋 · 자동 승인 · 그룹 키 · 감사 보관 일수(Host 가 판정에 쓰는 값)와 전역
    /// 설정 파일 경로. `projectId` 를 주면 그 프로젝트의 설정 파일들 · 그 파일들이 덮은 `[mcp]` 키 · 그 프로젝트의 자동 승인도
    /// 싣는다(읽기만). 툴 하나하나의 판정은 `mcp.tools.list` 다.
    pub McpSettingsGet: "mcp.settings.get" {
        params: McpSettingsGetParams,
        result: McpSettingsView,
        tier: Read,
        roles: [Owner],
    }

    /// 전역 `settings.toml` 의 `[mcp]` 키 하나를 쓴다(`value: null` 이면 지운다). 프로젝트 설정은 쓰지 않는다. 값은 Host 가
    /// 판정한다 — 모르는 프리셋 · 그룹 · 툴, `auto_approve` 의 `dangerous`, 0 ~ 3650 정수가 아닌 보관 일수는 쓰지 않고
    /// `INVALID_PARAMS`(`data` 는 `McpSettingRefused`). 쓴 뒤 설정을 다시 읽어 열린 MCP 세션의 툴 목록이 바뀌고
    /// (`tools/list_changed`), 새 전역 값을 돌려준다.
    pub McpSettingsSet: "mcp.settings.set" {
        params: McpSettingsSetParams,
        result: McpSettingsView,
        tier: Write,
        roles: [Owner],
    }
}

// 거절의 `data` — 메서드가 참조하지 않아 따로 등록한다(창이 `reason` 을 번역한다).
pw_rpc::rpc_type!(McpSettingRefused);

fn refused(key: McpSettingKey, reason: McpSettingRefusal, message: String) -> RpcError {
    RpcError::invalid_params(message).with_data(
        serde_json::to_value(McpSettingRefused { key, reason })
            .expect("McpSettingRefused serializes"),
    )
}

/// 판정을 지난 쓰기: `[mcp]` 아래 키 경로와 새 값(`None` 이면 지움).
struct Write {
    path: Vec<String>,
    value: Option<toml_edit::Value>,
}

/// 등급 이름 → 등급. Host 가 `auto_approve` 를 읽을 때와 같은 이름(`pw_core::config::mcp_policy::tier_name`)이다.
fn parse_tier(s: &str) -> Option<Tier> {
    [Tier::Read, Tier::Write, Tier::Exec, Tier::Dangerous]
        .into_iter()
        .find(|t| policy::tier_name(*t) == s)
}

/// 값 판정. Host 가 읽는 규칙과 같다(모듈 문서).
fn validate(parts: &HostParts, params: &McpSettingsSetParams) -> Result<Write, RpcError> {
    use McpSettingKey as K;
    use McpSettingRefusal as R;
    let key = params.key;
    let value = params.value.as_ref().filter(|v| !v.is_null());
    let bad = |message: String| refused(key, R::BadValue, message);
    let name = || {
        params
            .name
            .as_deref()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| bad(format!("{key:?} needs a name")))
    };
    let bool_value = || match value {
        None => Ok(None),
        Some(Value::Bool(on)) => Ok(Some(toml_edit::Value::from(*on))),
        Some(other) => Err(bad(format!("must be true, false or null, found {other}"))),
    };
    let write = |path: &[&str], value| Write {
        path: path.iter().map(|s| (*s).to_owned()).collect(),
        value,
    };
    Ok(match key {
        K::Preset => {
            let value = match value {
                None => None,
                Some(Value::String(s)) if Preset::parse(s).is_some() => {
                    Some(toml_edit::Value::from(s.as_str()))
                }
                Some(other) => {
                    return Err(refused(
                        key,
                        R::UnknownPreset,
                        format!("mcp.preset must be read-only, standard or full, found {other}"),
                    ));
                }
            };
            write(&["preset"], value)
        }
        K::AutoApprove => {
            let value = match value {
                None => None,
                Some(Value::Array(items)) => {
                    let mut tiers = Vec::new();
                    for item in items {
                        let tier = item.as_str().and_then(parse_tier);
                        match tier {
                            Some(Tier::Dangerous) => {
                                return Err(refused(
                                    key,
                                    R::DangerousAutoApprove,
                                    "mcp.auto_approve cannot hold `dangerous`; dangerous tools always ask"
                                        .to_owned(),
                                ));
                            }
                            Some(tier) => tiers.push(tier),
                            None => {
                                return Err(refused(
                                    key,
                                    R::UnknownTier,
                                    format!(
                                        "mcp.auto_approve: `{item}` is not read, write or exec"
                                    ),
                                ));
                            }
                        }
                    }
                    // 등급 차례로 하나씩 — 파일에 같은 목록이 늘 같은 글로 남는다.
                    let names: toml_edit::Array = [Tier::Read, Tier::Write, Tier::Exec]
                        .into_iter()
                        .filter(|t| tiers.contains(t))
                        .map(policy::tier_name)
                        .collect();
                    Some(toml_edit::Value::Array(names))
                }
                Some(other) => {
                    return Err(bad(format!(
                        "mcp.auto_approve must be a list of tiers, found {other}"
                    )));
                }
            };
            write(&[AUTO_APPROVE], value)
        }
        K::Group => {
            let group = name()?;
            if !GROUPS.contains(&group) {
                return Err(refused(
                    key,
                    R::UnknownGroup,
                    format!("`{group}` is not a tool group ({})", GROUPS.join(", ")),
                ));
            }
            write(&["groups", group], bool_value()?)
        }
        K::Tool => {
            let tool = name()?;
            if parts.catalog.get(tool).is_none() {
                return Err(refused(
                    key,
                    R::UnknownTool,
                    format!("`{tool}` is not a tool of this host"),
                ));
            }
            write(&["tools", tool], bool_value()?)
        }
        K::RetentionDays => {
            let value = match value {
                None => None,
                Some(v) => {
                    // Host 가 읽는 함수가 경고 없이 받는 값만 쓴다 — 화면에 보인 값과 실제 보관이 갈리지 않는다.
                    let (days, warning) = audit::retention_days(Some(
                        &json!({ audit::SETTINGS_TABLE: { audit::RETENTION_KEY: v } }),
                    ));
                    if let Some(warning) = warning {
                        return Err(refused(key, R::BadRetentionDays, warning));
                    }
                    if days > MAX_RETENTION_DAYS {
                        return Err(refused(
                            key,
                            R::BadRetentionDays,
                            format!(
                                "mcp.audit.retention_days must be at most {MAX_RETENTION_DAYS} days \
                                 (0 keeps everything), found {days}"
                            ),
                        ));
                    }
                    Some(toml_edit::Value::from(i64::from(days)))
                }
            };
            write(&[audit::SETTINGS_TABLE, audit::RETENTION_KEY], value)
        }
        K::HttpEnabled => write(&[http::TABLE, http::ENABLED_KEY], bool_value()?),
        K::HttpPort => {
            let value = match value {
                None => None,
                Some(v) => match v.as_u64().and_then(|n| u16::try_from(n).ok()) {
                    Some(port) if port != 0 => Some(toml_edit::Value::from(i64::from(port))),
                    _ => {
                        return Err(refused(
                            key,
                            R::BadPort,
                            format!(
                                "mcp.http.port must be a port number from 1 to 65535, found {v}"
                            ),
                        ));
                    }
                },
            };
            write(&[http::TABLE, http::PORT_KEY], value)
        }
    })
}

impl Mcp {
    /// `mcp.settings.get`. 디스크 IO(프로젝트 설정)를 한다.
    pub fn settings(&self, params: &McpSettingsGetParams) -> Result<McpSettingsView, RpcError> {
        let parts = self.parts()?;
        let mut view = global_view(parts);
        view.http = self.sessions().http().map(http::Http::status);
        if let Some(id) = &params.project_id {
            let project = parts
                .projects
                .get(id)
                .map_err(pw_core::registry::methods::rpc_error)?;
            view.project = Some(project_view(parts, &project));
        }
        Ok(view)
    }

    /// `mcp.settings.set`: 판정 → 전역 `settings.toml` 의 그 키만 쓰기 → 다시 읽기. 디스크 IO 를 한다.
    pub fn set_setting(&self, params: &McpSettingsSetParams) -> Result<McpSettingsView, RpcError> {
        let parts = self.parts()?;
        let write = validate(parts, params)?;
        let mut path: Vec<&str> = vec![KEY];
        path.extend(write.path.iter().map(String::as_str));
        let file = parts.global.source().dir().join(SETTINGS_FILE);
        let changed = write_key(&file, SETTINGS_KIND, &path, write.value, Links::Follow)
            .map_err(|e: WriteError| RpcError::internal(e.to_string()))?;
        if changed {
            tracing::info!(key = path.join("."), "mcp setting written");
            parts
                .global
                .reload()
                .map_err(|e| RpcError::internal(e.to_string()))?;
        }
        let mut view = global_view(parts);
        view.http = self.sessions().http().map(http::Http::status);
        Ok(view)
    }
}

fn global_view(parts: &HostParts) -> McpSettingsView {
    let snapshot = parts.global.snapshot();
    let global = snapshot.get("settings", KEY);
    let (exposure, mut warnings) = Exposure::from_layers(global, None);
    let (policy, policy_warnings) = policy::read(global, None, None, false);
    let (retention_days, retention_warning) = audit::retention_days(global);
    warnings.extend(policy_warnings);
    warnings.extend(retention_warning);
    McpSettingsView {
        file: parts
            .global
            .source()
            .dir()
            .join(SETTINGS_FILE)
            .display()
            .to_string(),
        preset: exposure.preset.as_str().to_owned(),
        auto_approve: policy.auto_approve,
        groups: GROUPS
            .iter()
            .map(|group| McpGroupSetting {
                group: (*group).to_owned(),
                enabled: exposure
                    .groups
                    .iter()
                    .find(|(g, _)| g == group)
                    .map(|(_, on)| *on),
            })
            .collect(),
        retention_days,
        default_retention_days: DEFAULT_RETENTION_DAYS,
        max_retention_days: MAX_RETENTION_DAYS,
        warnings,
        project: None,
        http: None,
    }
}

/// 그 프로젝트의 설정 파일 · 덮은 키 · 자동 승인. 프로젝트 설정은 디스크에 쓰지 않고 읽는다(`without_writes`).
fn project_view(parts: &HostParts, project: &Project) -> McpProjectSettings {
    // 프로젝트 설정을 못 읽으면 자동 승인이 없다(에이전트 호출은 거절된다). 사람의 화면이라 읽지 못한 파일과 까닭을 그대로 보인다.
    let (policy, mut warnings) = match policy::read_project(&parts.paths, &parts.global, project) {
        Ok(read) => read,
        Err(e) => (
            policy::Policy::default(),
            vec![format!("{} — {}", e.message(), e.detail)],
        ),
    };
    let mut files = Vec::new();
    let mut overrides = Vec::new();
    match project.config_source(&parts.paths, project.root()) {
        Ok(source) => {
            let source = source.without_writes();
            let personal = project.config_mode == ConfigMode::Personal;
            let shared_kind = if personal {
                McpSettingsFileKind::Personal
            } else {
                McpSettingsFileKind::Shared
            };
            files.push(file_entry(shared_kind, &source.dir().join("project.toml")));
            if !personal {
                files.push(file_entry(
                    McpSettingsFileKind::Local,
                    &source.local_file("project"),
                ));
            }
            match source.load_layered() {
                Ok(layered) => {
                    let project_mcp = layered.snapshot.get("project", KEY);
                    let (_, read_warnings) = Exposure::from_layers(None, project_mcp);
                    warnings.extend(read_warnings);
                    for layer in layered.layers.get("project").into_iter().flatten() {
                        let kind = match layer.origin {
                            LayerOrigin::Builtin => continue,
                            LayerOrigin::Shared => shared_kind,
                            LayerOrigin::Local => McpSettingsFileKind::Local,
                        };
                        if let Some(Value::Object(mcp)) = layer.value.get(KEY) {
                            flatten(mcp, "", kind, &mut overrides);
                        }
                    }
                }
                Err(e) => warnings.push(format!("cannot read the project config: {e}")),
            }
        }
        Err(e) => warnings.push(format!("cannot read the project config: {e}")),
    }
    let mut seen = std::collections::HashSet::new();
    warnings.retain(|w| seen.insert(w.clone()));
    McpProjectSettings {
        project_id: project.id.clone(),
        files,
        overrides,
        auto_approve: policy.auto_approve,
        warnings,
    }
}

fn file_entry(kind: McpSettingsFileKind, path: &Path) -> McpSettingsFile {
    McpSettingsFile {
        kind,
        path: path.display().to_string(),
        exists: std::fs::symlink_metadata(path).is_ok(),
    }
}

/// `[mcp]` 표를 점으로 이은 키로 편다. 표 아래 표(`groups` · `tools` · `audit` · `policy`)는 한 단계 들어가고 그 값은 그대로다.
fn flatten(
    table: &Map<String, Value>,
    prefix: &str,
    file: McpSettingsFileKind,
    out: &mut Vec<McpSettingOverride>,
) {
    for (key, value) in table {
        let dotted = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Object(inner) if prefix.is_empty() => flatten(inner, &dotted, file, out),
            _ => out.push(McpSettingOverride {
                key: dotted,
                value: value.clone(),
                file,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_spreads_one_level_of_tables() {
        let mcp = json!({"preset": "full", "groups": {"db": false}, "tools": {"a_b": true}});
        let mut out = Vec::new();
        flatten(
            mcp.as_object().unwrap(),
            "",
            McpSettingsFileKind::Shared,
            &mut out,
        );
        let keys: Vec<&str> = out.iter().map(|o| o.key.as_str()).collect();
        assert_eq!(keys, ["groups.db", "preset", "tools.a_b"]);
    }
}
