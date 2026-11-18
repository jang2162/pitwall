//! `open_with_upsert` 툴 메서드(`docs/design/16-m5-plan.md` M5-16 ②, `docs/design/05-mcp.md` 「외부 도구」). `write`, 그룹
//! `open_with`, 앱 Host 만, 프로젝트가 없는 쓰기다(승인은 가장 최근에 앞에 있던 창에 묻는다 — 결정 M5-D29).
//!
//! 전역 설정 `$CONFIG/settings.toml` 의 `[open_with.tools.<id>]` 표를 **통째로** 입력의 것으로 바꾼다(03-architecture.md
//! 「`[open_with]` — 외부 도구 설정」). 있던 그 표의 다른 키는 남지 않는다 — 계획의 diff 가 옛 값과 새 값을 함께 보인다. 확장이
//! 기여한 도구면 표에 없는 키는 확장의 값이다([`crate::tools`] 의 덮어쓰기 규칙 — `command` 를 적으면 확장의 `file_command` 는
//! 버린다).
//!
//! - **명령 형식 검사는 목록과 같은 한 벌이다**([`crate::tools`] 의 설정 읽기 — `pw_ext::template::check_command`: argv 배열, 첫
//!   요소에 변수 금지, `command` 에 파일 변수 금지, `file_command` 는 `{file}`). 틀리면 승인을 묻기 전에 거절한다. 확장에 없는 새
//!   도구는 `label` 과 `command` 가 있어야 한다.
//! - **명령을 정의하는 쓰기라 늘 묻는다**(결정 M5-D31): 계획 규칙이 `auto_approve` 를 끄고 「이 세션 동안 허용」 을 두지 않는다
//!   ([`Remember::Never`]). 그 명령은 뒤에 사람이 「다음에서 열기」 를 눌러 돌리므로 정의마다 사람이 본다.
//! - **두 단계**(`pw_core::registry::plan` 머리 주석): 첫 호출은 `settings.toml` 을 읽어 고친 뒤 내용을 만들기만 한다(쓰지 않는다).
//!   해시는 입력과 고치기 전 파일 내용을 덮는다 — 승인 뒤 사람이 파일을 고쳤으면 다시 만든 계획의 해시가 달라 돌지 않는다. 승인
//!   뒤에는 **계획 때 만든 내용 그대로** 쓰고, 쓰기 직전에 파일이 계획 때 읽은 내용인지 비교한다
//!   (`pw_core::config::write::apply_planned`) — 다시 읽어 새로 만들지 않는다.
//! - `settings.local.toml` 이 같은 도구를 적었으면 그 키가 이긴다 — 계획 요약과 결과에 알린다(그 파일은 고치지 않는다).

use pw_core::config::write::{
    Links, PlannedEdit, SETTINGS_FILE, WriteError, apply_planned, plan_table,
};
use pw_core::config::{ConfigSource, LayerOrigin};
use pw_core::registry::plan::{PlanChange, PlanRules, Planned, Remember, ToolPlan};
use pw_rpc::{ConnectionScope, RpcError, Tier, code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::OpenWith;
use crate::methods::ExternalToolInfo;
use crate::tools::{SETTINGS_KEY, check_tool_setting};

/// 이 툴 메서드의 RPC 이름.
const METHOD: &str = "openWith.tool.upsert";

/// 도구 id 의 최대 길이.
pub const MAX_ID: usize = 64;

/// The OS a tool is offered on.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum OpenWithPlatform {
    Macos,
    Linux,
    Windows,
}

impl OpenWithPlatform {
    fn name(self) -> &'static str {
        match self {
            OpenWithPlatform::Macos => "macos",
            OpenWithPlatform::Linux => "linux",
            OpenWithPlatform::Windows => "windows",
        }
    }
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OpenWithUpsertParams {
    /// Tool id. An id from `open_with_list` changes that tool; a new id adds a tool. Lowercase letters, digits, `-`
    /// and `_`, starting with a letter or digit, up to 64.
    pub id: String,
    /// Name shown in the "Open with" menu. Required for a new tool.
    #[serde(default)]
    pub label: Option<String>,
    /// Command that opens the project, as an argv array run without a shell: the first element is the program (no
    /// variables in it), the others may use `{project_root}` and `{branch}`. Example: `["zed", "{project_root}"]`.
    /// Required for a new tool.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Command that opens a file, as an argv array like `command`; it must use `{file}` and may use `{line}`,
    /// `{column}`, `{project_root}` and `{branch}`. Example: `["zed", "{file}:{line}:{column}"]`.
    #[serde(default)]
    pub file_command: Option<Vec<String>>,
    /// The OSes the tool is offered on. Without it, the contributing extension's list, or every OS for a new tool.
    #[serde(default)]
    pub platforms: Option<Vec<OpenWithPlatform>>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OpenWithUpsertResult {
    /// The global settings file that was written.
    pub file: String,
    /// The tool as `open_with_list` now shows it. `null` when it is not offered on this OS.
    pub tool: Option<ExternalToolInfo>,
    /// `settings.local.toml` also sets this tool, and its keys win over the ones written.
    pub overridden_locally: bool,
}

rpc_method! {
    /// Add an external tool for "Open with", or change one, in the global settings (`[open_with.tools.<id>]` in
    /// `settings.toml`). The tool's settings entry is replaced as a whole by the fields given; for a tool contributed
    /// by an extension, fields left out keep the extension's values (giving `command` drops the extension's
    /// `file_command` unless it is given too). Commands are argv arrays run without a shell, and the first element
    /// cannot use variables; a malformed command or a new tool without `label` and `command` is refused before asking.
    /// Always needs approval in Pitwall, every call: the request shows the settings file before and after.
    pub OpenWithToolUpsert: "openWith.tool.upsert" {
        params: Planned<OpenWithUpsertParams>,
        result: OpenWithUpsertResult,
        tier: Write,
        roles: [Owner],
        mcp: { group: "open_with", name: "open_with_upsert" },
    }
}

/// 계획 때 만든 것 — 승인 뒤에는 이것만 쓴다.
#[derive(Debug, Clone)]
pub struct PlannedUpsert {
    pub edit: PlannedEdit,
    /// 그 id 가 지금 목록(확장 · 설정)에 있는 도구다.
    pub replaces: bool,
    pub overridden_locally: bool,
}

impl PlannedUpsert {
    /// 승인 요청이 보일 계획. 해시는 입력 · 파일 경로 · 고치기 전 내용을 덮는다.
    pub fn plan(&self, params: &OpenWithUpsertParams) -> ToolPlan {
        let path = self.edit.path.display().to_string();
        let mut summary = if self.replaces {
            format!(
                "Change the external tool `{}` in the global settings",
                params.id
            )
        } else {
            format!(
                "Add the external tool `{}` to the global settings",
                params.id
            )
        };
        if self.overridden_locally {
            summary.push_str(" (settings.local.toml also sets this tool, and its keys win)");
        }
        ToolPlan::builder(METHOD, Tier::Write, params)
            .summary(summary)
            .change(PlanChange::File {
                path: path.clone(),
                before: self.edit.before.clone(),
                after: Some(self.edit.after.clone()),
            })
            .target("file", &path)
            .target("before", &self.edit.before)
            .target("overriddenLocally", &self.overridden_locally)
            // 명령을 정의하는 쓰기 — 자동 승인도 세션 허용도 없다(결정 M5-D31).
            .rules(PlanRules {
                auto_approve: false,
                remember: Remember::Never,
                policy_auto: false,
            })
            .build()
    }
}

/// 도구 id 의 꼴: 영소문자 · 숫자로 시작하고 영소문자 · 숫자 · `-` · `_`, [`MAX_ID`] 자까지.
pub fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID
        && id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// 입력을 설정 표의 키 · 값으로(설정 파일의 키 이름 — `file_command`).
fn setting_entries(params: &OpenWithUpsertParams) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    if let Some(label) = &params.label {
        out.push(("label", Value::from(label.clone())));
    }
    if let Some(command) = &params.command {
        out.push(("command", Value::from(command.clone())));
    }
    if let Some(file_command) = &params.file_command {
        out.push(("file_command", Value::from(file_command.clone())));
    }
    if let Some(platforms) = &params.platforms {
        out.push((
            "platforms",
            Value::from(platforms.iter().map(|p| p.name()).collect::<Vec<_>>()),
        ));
    }
    out
}

fn write_error(e: WriteError) -> RpcError {
    match e {
        WriteError::Changed { .. } => RpcError::new(code::PERMISSION_DENIED, e.to_string())
            .with_data(serde_json::json!({ "reason": "planChanged" })),
        other => RpcError::internal(other.to_string()),
    }
}

impl OpenWith {
    /// `open_with_upsert` 의 계획 단계: 형식을 보고 `settings.toml` 을 고친 뒤 내용을 만든다. 쓰지 않는다.
    pub fn plan_upsert(&self, params: &OpenWithUpsertParams) -> Result<PlannedUpsert, RpcError> {
        let host = self.host()?;
        if !valid_id(&params.id) {
            return Err(RpcError::invalid_params(format!(
                "tool id `{}` must be 1 to {MAX_ID} lowercase letters, digits, `-` or `_`, starting with a letter or digit",
                params.id
            )));
        }
        let entries = setting_entries(params);
        if entries.is_empty() {
            return Err(RpcError::invalid_params(
                "give at least one of `label`, `command`, `fileCommand` and `platforms`",
            ));
        }
        let table: Map<String, Value> = entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        let mut warnings = Vec::new();
        check_tool_setting(&params.id, &Value::Object(table), &mut warnings)
            .map_err(RpcError::invalid_params)?;
        let contributed = self
            .extensions
            .external_tools()
            .any(|(_, decl)| decl.id == params.id);
        if !contributed && (params.label.is_none() || params.command.is_none()) {
            return Err(RpcError::invalid_params(format!(
                "`{}` is not a tool contributed by an extension, so it needs both `label` and `command`",
                params.id
            )));
        }
        let file = host.paths.config.join(SETTINGS_FILE);
        let edit = plan_table(
            &file,
            "settings",
            &[SETTINGS_KEY, "tools", &params.id],
            &entries,
        )
        .map_err(|e| RpcError::internal(e.to_string()))?;
        let replaces = contributed || self.tools(host).iter().any(|t| t.id == params.id);
        Ok(PlannedUpsert {
            edit,
            replaces,
            overridden_locally: self.set_locally(&params.id),
        })
    }

    /// `settings.local.toml` 이 `[open_with.tools.<id>]` 를 적었는가. 읽지 못하면 아니라고 본다(경고).
    fn set_locally(&self, id: &str) -> bool {
        let Ok(host) = self.host() else { return false };
        match ConfigSource::global(&host.paths)
            .without_writes()
            .load_layered()
        {
            Ok(loaded) => loaded.layers.get("settings").is_some_and(|layers| {
                layers.iter().any(|l| {
                    l.origin == LayerOrigin::Local
                        && l.value
                            .get(SETTINGS_KEY)
                            .and_then(|o| o.get("tools"))
                            .and_then(|t| t.get(id))
                            .is_some()
                })
            }),
            Err(e) => {
                tracing::warn!(target: "pw_open_with", %e, "cannot read the global settings layers");
                false
            }
        }
    }

    /// 승인을 지난 쓰기: 계획 때 만든 내용 그대로, 파일이 계획 때 읽은 그대로일 때만.
    pub fn apply_upsert(
        &self,
        id: &str,
        planned: &PlannedUpsert,
    ) -> Result<OpenWithUpsertResult, RpcError> {
        let host = self.host()?;
        // 전역 설정은 사람의 dotfiles 링크일 수 있다 — 대상 파일을 고친다(설정 창의 쓰기와 같다).
        apply_planned(&planned.edit, Links::Follow).map_err(write_error)?;
        if let Err(e) = host.global.reload() {
            tracing::warn!(target: "pw_open_with", %e, "wrote the global settings but cannot reload them");
        }
        let tool = self
            .tools(host)
            .into_iter()
            .find(|t| t.id == id)
            .map(crate::tool_info);
        Ok(OpenWithUpsertResult {
            file: planned.edit.path.display().to_string(),
            tool,
            overridden_locally: planned.overridden_locally,
        })
    }

    /// `open_with_upsert` 의 본체. 블로킹(설정 파일).
    pub fn upsert(
        &self,
        scope: &ConnectionScope,
        input: &Planned<OpenWithUpsertParams>,
    ) -> Result<OpenWithUpsertResult, RpcError> {
        if !matches!(scope, ConnectionScope::Agent) {
            return Err(RpcError::new(
                code::PERMISSION_DENIED,
                "only an mcp session may call agent tools",
            )
            .with_data(serde_json::json!({ "reason": "notAgentSession" })));
        }
        let planned = self.plan_upsert(&input.params)?;
        // 이 줄 위에는 부작용이 없다 — 승인 표가 없거나 맞지 않으면 계획을 실은 「승인 필요」 로 돌아간다.
        input.approve(planned.plan(&input.params))?;
        self.apply_upsert(&input.params.id, &planned)
    }
}

#[cfg(test)]
mod tests;
