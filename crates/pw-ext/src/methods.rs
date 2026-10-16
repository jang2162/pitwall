//! 확장 레지스트리 RPC 메서드와 그 핸들러.

use std::sync::Arc;

use pw_rpc::{JsonValue, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::manifest::{KeymapKind, Severity};
use crate::registry::{Appearance, Registry};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ThemeAppearance {
    Light,
    Dark,
}

impl From<Appearance> for ThemeAppearance {
    fn from(a: Appearance) -> Self {
        match a {
            Appearance::Light => ThemeAppearance::Light,
            Appearance::Dark => ThemeAppearance::Dark,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ContributionIds {
    pub themes: Vec<String>,
    pub keymaps: Vec<String>,
    pub languages: Vec<String>,
    pub external_tools: Vec<String>,
    /// 언어 태그.
    pub translations: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub builtin: bool,
    /// `builtin:<폴더>` 또는 폴더의 절대 경로.
    pub location: String,
    pub contributes: ContributionIds,
    /// `[wasm]` 이 있는가. 지금은 실행하지 않는다.
    pub wasm: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionDiagnostic {
    pub severity: DiagnosticSeverity,
    pub file: String,
    /// 1부터 센다. 위치를 모르면 없다.
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionList {
    /// 등록된 확장. 내장이 먼저다.
    pub extensions: Vec<ExtensionInfo>,
    /// 읽는 중에 나온 오류 · 경고. 오류가 있는 확장은 `extensions` 에 없다.
    pub diagnostics: Vec<ExtensionDiagnostic>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ThemeInfo {
    pub id: String,
    pub label: String,
    /// 기여한 확장의 id.
    pub extension: String,
    /// 파일에 있는 변형. 하나뿐이면 어느 외관을 골라도 그 변형을 쓴다(07 「3. 테마」 「형식」 행).
    pub appearances: Vec<ThemeAppearance>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ThemeList {
    pub themes: Vec<ThemeInfo>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ThemeGetParams {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeFile {
    pub id: String,
    pub extension: String,
    /// 테마 파일(JSON) 그대로. 토큰 검사는 화면(`parseTheme`)이 한다.
    #[specta(type = JsonValue)]
    pub content: serde_json::Value,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum KeymapContributionKind {
    /// 전체 키맵. 기본 키맵 대신 하나를 고른다.
    Base,
    /// 베이스 위에 병합하는 일부 바인딩.
    Bindings,
}

impl From<KeymapKind> for KeymapContributionKind {
    fn from(kind: KeymapKind) -> Self {
        match kind {
            KeymapKind::Base => KeymapContributionKind::Base,
            KeymapKind::Bindings => KeymapContributionKind::Bindings,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KeymapInfo {
    pub id: String,
    /// 기여한 확장의 id.
    pub extension: String,
    pub kind: KeymapContributionKind,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KeymapList {
    pub keymaps: Vec<KeymapInfo>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KeymapGetParams {
    pub id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct KeymapFile {
    pub id: String,
    pub extension: String,
    pub kind: KeymapContributionKind,
    /// 키맵 파일(TOML) 텍스트 그대로. 형식 검사는 키맵 엔진(packages/keymap)이 한다.
    pub content: String,
}

rpc_method! {
    /// 등록된 확장과 읽는 중에 나온 진단.
    pub ExtList: "ext.list" {
        params: (),
        result: ExtensionList,
        tier: Read,
    }

    /// 등록된 테마 목록.
    pub ThemeListMethod: "theme.list" {
        params: (),
        result: ThemeList,
        tier: Read,
    }

    /// 테마 하나의 파일 내용. 없는 id 면 invalid params 오류다.
    pub ThemeGet: "theme.get" {
        params: ThemeGetParams,
        result: ThemeFile,
        tier: Read,
    }

    /// 등록된 키맵 기여(베이스 키맵 · 바인딩 묶음) 목록.
    pub KeymapListMethod: "keymap.list" {
        params: (),
        result: KeymapList,
        tier: Read,
    }

    /// 키맵 기여 하나의 파일 텍스트. 없는 id 면 invalid params 오류다.
    pub KeymapGet: "keymap.get" {
        params: KeymapGetParams,
        result: KeymapFile,
        tier: Read,
    }
}

/// 메서드의 핸들러를 붙인다. 레지스트리는 시작할 때 한 번 읽은 것이다.
pub fn register(router: &mut Router, registry: Arc<Registry>) {
    let list = Arc::clone(&registry);
    let themes = Arc::clone(&registry);
    let keymaps = Arc::clone(&registry);
    let keymap = Arc::clone(&registry);
    router
        .unary(KeymapListMethod, move |_ctx, ()| {
            let result = keymap_list(&keymaps);
            async move { Ok(result) }
        })
        .unary(KeymapGet, move |_ctx, params: KeymapGetParams| {
            let result = keymap_file(&keymap, &params.id);
            async move { result }
        })
        .unary(ExtList, move |_ctx, ()| {
            let result = extension_list(&list);
            async move { Ok(result) }
        })
        .unary(ThemeListMethod, move |_ctx, ()| {
            let result = theme_list(&themes);
            async move { Ok(result) }
        })
        .unary(ThemeGet, move |_ctx, params: ThemeGetParams| {
            let result = theme_file(&registry, &params.id);
            async move { result }
        });
}

fn extension_list(registry: &Registry) -> ExtensionList {
    let extensions = registry
        .extensions()
        .iter()
        .map(|e| {
            let c = &e.manifest.contributes;
            ExtensionInfo {
                id: e.manifest.id.clone(),
                name: e.manifest.name.clone(),
                version: e.manifest.version.to_string(),
                builtin: e.root.is_builtin(),
                location: e.root.display(None),
                contributes: ContributionIds {
                    themes: c.themes.iter().map(|t| t.id.clone()).collect(),
                    keymaps: c.keymaps.iter().map(|k| k.id.clone()).collect(),
                    languages: c.languages.iter().map(|l| l.id.clone()).collect(),
                    external_tools: c.external_tools.iter().map(|t| t.id.clone()).collect(),
                    translations: c.translations.iter().map(|t| t.locale.clone()).collect(),
                },
                wasm: e.manifest.wasm.is_some(),
            }
        })
        .collect();
    let diagnostics = registry
        .diagnostics()
        .iter()
        .map(|d| ExtensionDiagnostic {
            severity: match d.severity {
                Severity::Error => DiagnosticSeverity::Error,
                Severity::Warning => DiagnosticSeverity::Warning,
            },
            file: d.file.clone(),
            line: d.pos.map(|p| p.line),
            column: d.pos.map(|p| p.column),
            message: d.message.clone(),
        })
        .collect();
    ExtensionList {
        extensions,
        diagnostics,
    }
}

fn theme_list(registry: &Registry) -> ThemeList {
    ThemeList {
        themes: registry
            .themes()
            .map(|(e, t)| ThemeInfo {
                id: t.id.clone(),
                label: t.label.clone(),
                extension: e.manifest.id.clone(),
                appearances: t.appearances.iter().copied().map(Into::into).collect(),
            })
            .collect(),
    }
}

fn keymap_list(registry: &Registry) -> KeymapList {
    KeymapList {
        keymaps: registry
            .keymaps()
            .map(|(e, k)| KeymapInfo {
                id: k.id.clone(),
                extension: e.manifest.id.clone(),
                kind: k.kind.into(),
            })
            .collect(),
    }
}

fn keymap_file(registry: &Registry, id: &str) -> Result<KeymapFile, RpcError> {
    let keymap = registry
        .keymap(id)
        .ok_or_else(|| RpcError::invalid_params(format!("unknown keymap `{id}`")))?;
    // 크기는 등록할 때 MAX_CONTENT_BYTES 로 막았다. 그 뒤에 파일이 바뀌었을 수 있어 읽기 · UTF-8 실패는 오류로 낸다.
    let bytes = keymap
        .file
        .read()
        .map_err(|e| RpcError::internal(format!("{}: {e}", keymap.file.display())))?;
    let content = String::from_utf8(bytes.into_owned())
        .map_err(|_| RpcError::internal(format!("{}: not UTF-8", keymap.file.display())))?;
    Ok(KeymapFile {
        id: keymap.id,
        extension: keymap.extension,
        kind: keymap.kind.into(),
        content,
    })
}

fn theme_file(registry: &Registry, id: &str) -> Result<ThemeFile, RpcError> {
    let (extension, theme) = registry
        .theme(id)
        .ok_or_else(|| RpcError::invalid_params(format!("unknown theme `{id}`")))?;
    Ok(ThemeFile {
        id: theme.id.clone(),
        extension: extension.manifest.id.clone(),
        content: theme.content.clone(),
    })
}

#[cfg(test)]
mod tests {
    use pw_rpc::{Caller, EventHub, Outbound, Role, Session};
    use semver::Version;
    use serde_json::{Value, json};

    use super::*;
    use crate::builtin;

    async fn call(registry: Registry, request: Value) -> Value {
        let mut router = Router::new();
        register(&mut router, Arc::new(registry));
        let (session, mut rx) =
            Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
        session.handle_value(request);
        match rx.recv().await.unwrap() {
            reply @ Outbound::Response(_) => serde_json::to_value(reply).unwrap(),
            other => panic!("expected a response, got {other:?}"),
        }
    }

    fn builtin_registry() -> Registry {
        Registry::from_roots(&Version::new(0, 0, 0), builtin::roots())
    }

    #[tokio::test]
    async fn theme_get_returns_the_builtin_default_theme_file() {
        let reply = call(
            builtin_registry(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "theme.get", "params": {"id": "pitwall-default"}}),
        )
        .await;
        let result = &reply["result"];
        assert_eq!(result["extension"], "dev.pitwall.theme-default", "{reply}");
        let file: Value = serde_json::from_str(include_str!(
            "../../../extensions/theme-default/themes/default.json"
        ))
        .unwrap();
        assert_eq!(result["content"], file);
    }

    #[tokio::test]
    async fn theme_get_rejects_an_unknown_id() {
        let reply = call(
            builtin_registry(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "theme.get", "params": {"id": "nope"}}),
        )
        .await;
        assert_eq!(
            reply["error"]["code"],
            pw_rpc::code::INVALID_PARAMS,
            "{reply}"
        );
        assert_eq!(reply["error"]["data"], "unknown theme `nope`", "{reply}");
    }

    #[tokio::test]
    async fn keymap_list_and_get_return_the_builtin_default_keymap() {
        let reply = call(
            builtin_registry(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "keymap.list"}),
        )
        .await;
        assert_eq!(
            reply["result"]["keymaps"],
            json!([{"id": "pitwall-default", "extension": "dev.pitwall.keymap-default", "kind": "base"}]),
            "{reply}"
        );

        let reply = call(
            builtin_registry(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "keymap.get", "params": {"id": "pitwall-default"}}),
        )
        .await;
        assert_eq!(
            reply["result"]["content"],
            include_str!("../../../extensions/keymap-default/keymaps/default.toml"),
            "{reply}"
        );
        assert_eq!(reply["result"]["kind"], "base");

        let reply = call(
            builtin_registry(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "keymap.get", "params": {"id": "nope"}}),
        )
        .await;
        assert_eq!(reply["error"]["data"], "unknown keymap `nope`", "{reply}");
    }

    #[tokio::test]
    async fn theme_list_and_ext_list_describe_the_builtin_extensions() {
        let reply = call(
            builtin_registry(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "theme.list"}),
        )
        .await;
        assert_eq!(
            reply["result"]["themes"],
            json!([{"id": "pitwall-default", "label": "Pitwall", "extension": "dev.pitwall.theme-default", "appearances": ["light", "dark"]}]),
            "{reply}"
        );

        let reply = call(
            builtin_registry(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "ext.list"}),
        )
        .await;
        let tools = reply["result"]["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "dev.pitwall.external-tools")
            .unwrap_or_else(|| panic!("{reply}"));
        assert_eq!(tools["builtin"], true);
        assert_eq!(tools["location"], "builtin:external-tools");
        assert_eq!(tools["contributes"]["externalTools"][0], "orca");
    }
}
