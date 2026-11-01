//! `[lsp.<서버 id>]` 설정(결정 M4-D22): 전역 `settings.toml` 과 프로젝트 `.pitwall/project.toml` 의 같은 표를 읽어 정의 위에
//! 합친다. 키는 `enabled` · `path` · `args` · `env` · `settings` · `features.disable` 이다. `[lsp]` 바로 아래의 표가 아닌 값
//! (`idle_shutdown_minutes` 등)은 서버 전역 설정이라 여기서 읽지 않는다(서버 관리자 M4-05 의 몫).
//!
//! 합치는 규칙: 프로젝트가 전역을 이긴다(키마다). `settings` 는 정의 → 전역 → 프로젝트 순으로 깊게 합친다(객체는 키마다, 그 밖의
//! 값은 덮는다). `env` 는 변수마다 덮는다. `args` 는 통째로 바꾼다. 끈 기능은 정의의 것에 설정의 것(프로젝트 > 전역)을 더한다.
//!
//! 형식이 틀린 키는 경고하고 그 키만 버린다 — 설정 하나가 틀렸다고 서버 목록 전체가 사라지지 않게.

use std::collections::BTreeMap;

use pw_ext::{LanguageServerDecl, LspFeature};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

/// 설정 파일에서 이 기능의 표 이름(`[lsp]`).
pub const SETTINGS_KEY: &str = "lsp";

/// 설정 한 겹(전역 또는 프로젝트)의 `[lsp.<id>]` 하나. 적지 않은 키는 `None` 이다.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerSettings {
    pub enabled: Option<bool>,
    pub path: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: BTreeMap<String, String>,
    pub settings: Option<Json>,
    pub disable: Option<Vec<LspFeature>>,
}

/// 설정이 어느 겹에서 왔는가. 상대 경로 `path` 를 무엇에 맞춰 풀지가 이것으로 갈린다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspSettingsOrigin {
    /// `$CONFIG/settings.toml` · `settings.local.toml`.
    Global,
    /// `.pitwall/project.toml` · `project.local.toml`(또는 개인 전용 폴더).
    Project,
}

/// `[lsp.<id>] path` 와 그것을 적은 겹.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredPath {
    pub value: String,
    pub origin: LspSettingsOrigin,
}

/// 정의 위에 설정을 합친 서버 하나의 값. 서버 관리자(M4-05)가 띄우고 `workspace/configuration` 에 답할 때 쓴다.
///
/// `env` · `settings` 에는 사람이 적은 비밀(토큰 등)이 있을 수 있어 `Debug` 는 손으로 쓴다 — `env` 는 이름만, `settings` 는 맨 위
/// 키 이름만 찍는다(`pw_runtime::RuntimeEnv` 와 같은 규칙, `docs/design/03-architecture.md` §5-1).
#[derive(Clone, PartialEq)]
pub struct ServerConfig {
    pub enabled: bool,
    pub path: Option<ConfiguredPath>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// `workspace/configuration` 의 답의 바탕. 객체다(정의 · 설정 어디에도 없으면 빈 객체).
    pub settings: Json,
    /// 정렬돼 있고 겹치지 않는다.
    pub disabled_features: Vec<LspFeature>,
}

impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerConfig")
            .field("enabled", &self.enabled)
            .field("path", &self.path)
            .field("args", &self.args)
            .field("env_names", &self.env.keys().collect::<Vec<_>>())
            .field("settings_keys", &top_keys(&self.settings))
            .field("disabled_features", &self.disabled_features)
            .finish()
    }
}

/// 객체의 맨 위 키 이름(값은 찍지 않는다). 객체가 아니면 비어 있다.
pub(crate) fn top_keys(value: &Json) -> Vec<&str> {
    value
        .as_object()
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// 설정 파일 하나의 `[lsp]` 값에서 서버 id 별 설정. `origin` 은 경고에 붙일 이름이다.
pub fn parse_layer(
    value: Option<&Json>,
    origin: &str,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, ServerSettings> {
    let mut out = BTreeMap::new();
    let Some(value) = value else { return out };
    let Some(table) = value.as_object() else {
        warnings.push(format!(
            "{origin}: `[{SETTINGS_KEY}]` must be a table; ignored"
        ));
        return out;
    };
    for (id, entry) in table {
        // 표가 아닌 값은 서버 전역 설정이다(머리 주석).
        let Some(entry) = entry.as_object() else {
            continue;
        };
        out.insert(
            id.clone(),
            parse_server(
                entry,
                &format!("{origin}: `[{SETTINGS_KEY}.{id}]`"),
                warnings,
            ),
        );
    }
    out
}

fn parse_server(entry: &Map<String, Json>, at: &str, warnings: &mut Vec<String>) -> ServerSettings {
    let mut s = ServerSettings::default();
    for (key, value) in entry {
        let mut bad =
            |expected: &str| warnings.push(format!("{at}: `{key}` must be {expected}; ignored"));
        match key.as_str() {
            "enabled" => match value.as_bool() {
                Some(b) => s.enabled = Some(b),
                None => bad("true or false"),
            },
            "path" => match value.as_str() {
                Some(p) if !p.trim().is_empty() => s.path = Some(p.to_owned()),
                _ => bad("a non-empty string"),
            },
            "args" => match strings(value) {
                Some(args) => s.args = Some(args),
                None => bad("an array of strings"),
            },
            "env" => match value.as_object() {
                Some(vars) => {
                    for (name, v) in vars {
                        match v.as_str() {
                            Some(v) if !name.is_empty() && !name.contains('=') => {
                                s.env.insert(name.clone(), v.to_owned());
                            }
                            _ => warnings.push(format!(
                                "{at}: `env.{name}` must be a string with a variable name; ignored"
                            )),
                        }
                    }
                }
                None => bad("a table of strings"),
            },
            "settings" => match value {
                Json::Object(_) => s.settings = Some(value.clone()),
                _ => bad("a table"),
            },
            "features" => match value.get("disable").map(strings) {
                Some(Some(names)) => {
                    let mut features = Vec::new();
                    for name in names {
                        match LspFeature::from_key(&name) {
                            Some(f) => features.push(f),
                            None => warnings.push(format!(
                                "{at}: unknown feature `{name}` in `features.disable` is ignored"
                            )),
                        }
                    }
                    s.disable = Some(features);
                }
                Some(None) => warnings.push(format!(
                    "{at}: `features.disable` must be an array of strings; ignored"
                )),
                None => {}
            },
            _ => warnings.push(format!("{at}: unknown key `{key}` is ignored")),
        }
    }
    s
}

fn strings(value: &Json) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// 정의 위에 전역 · 프로젝트 설정을 합친다(머리 주석의 규칙).
pub fn server_config(
    def: &LanguageServerDecl,
    global: Option<&ServerSettings>,
    project: Option<&ServerSettings>,
) -> ServerConfig {
    let layers: Vec<(LspSettingsOrigin, &ServerSettings)> = [
        global.map(|s| (LspSettingsOrigin::Global, s)),
        project.map(|s| (LspSettingsOrigin::Project, s)),
    ]
    .into_iter()
    .flatten()
    .collect();
    // 뒤의 겹(프로젝트)이 이긴다.
    let last = |pick: &dyn Fn(&ServerSettings) -> bool| {
        layers.iter().rev().find(|(_, s)| pick(s)).copied()
    };
    let enabled = last(&|s| s.enabled.is_some())
        .and_then(|(_, s)| s.enabled)
        .unwrap_or(true);
    let path = last(&|s| s.path.is_some()).map(|(origin, s)| ConfiguredPath {
        value: s.path.clone().unwrap_or_default(),
        origin,
    });
    let args = last(&|s| s.args.is_some())
        .and_then(|(_, s)| s.args.clone())
        .unwrap_or_else(|| def.args.clone());
    let mut env = def.env.clone();
    let mut settings = def
        .settings
        .clone()
        .unwrap_or_else(|| Json::Object(Map::new()));
    for (_, layer) in &layers {
        env.extend(layer.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        if let Some(over) = &layer.settings {
            deep_merge(&mut settings, over);
        }
    }
    let mut disabled_features = def.disabled_features.clone();
    if let Some((_, s)) = last(&|s| s.disable.is_some()) {
        disabled_features.extend(s.disable.iter().flatten().copied());
    }
    disabled_features.sort();
    disabled_features.dedup();
    ServerConfig {
        enabled,
        path,
        args,
        env,
        settings,
        disabled_features,
    }
}

/// `over` 를 `base` 위에 깊게 합친다: 둘 다 객체면 키마다 합치고, 아니면 `over` 가 이긴다.
pub fn deep_merge(base: &mut Json, over: &Json) {
    match (base, over) {
        (Json::Object(base), Json::Object(over)) => {
            for (key, value) in over {
                match base.get_mut(key) {
                    Some(slot) => deep_merge(slot, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, over) => *base = over.clone(),
    }
}

/// `.` 으로 나눈 키 경로에 값을 넣는다(중간 객체가 없거나 객체가 아니면 객체로 만든다). `initialization_options` 의
/// `project_local_options`(`tsserver.path`)에 쓴다.
pub fn set_path(target: &mut Json, dotted: &str, value: Json) {
    let mut slot = target;
    for key in dotted.split('.') {
        if !slot.is_object() {
            *slot = Json::Object(Map::new());
        }
        slot = slot
            .as_object_mut()
            .expect("made an object above")
            .entry(key.to_owned())
            .or_insert(Json::Null);
    }
    *slot = value;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn def(settings: &str) -> LanguageServerDecl {
        let source = format!(
            "id = \"dev.example.x\"\nname = \"X\"\nversion = \"1.0.0\"\nengines = {{ pitwall = \"*\" }}\n\
             [contributes.language_servers.ls]\nlanguages = [\"rust\"]\ncommand = \"ls\"\nargs = [\"--stdio\"]\n\
             env = {{ A = \"def\", B = \"def\" }}\nfeatures = {{ disable = [\"formatting\"] }}\nsettings = {settings}\n"
        );
        let (manifest, diagnostics) = pw_ext::manifest::parse_manifest(&source, "x");
        assert_eq!(diagnostics, vec![]);
        manifest.unwrap().contributes.language_servers.remove(0)
    }

    fn layer(value: Json, warnings: &mut Vec<String>) -> ServerSettings {
        parse_layer(Some(&value), "test", warnings)
            .remove("ls")
            .unwrap()
    }

    /// 정의 · 전역 · 프로젝트의 `settings` 가 깊게 합쳐지고(객체는 키마다, 배열 · 값은 덮는다) 그 밖의 키는 프로젝트가 이긴다.
    #[test]
    fn merges_definition_global_and_project_settings_deeply() {
        let def = def(
            "{ ra = { cargo = { features = \"all\", target = \"x86\" }, checkOnSave = true, list = [1, 2] } }",
        );
        let mut warnings = Vec::new();
        let global = layer(
            json!({"ls": {
                "enabled": false,
                "path": "/opt/ls",
                "args": ["--global"],
                "env": {"B": "global", "C": "global"},
                "settings": {"ra": {"cargo": {"target": "aarch64"}, "list": [3]}},
                "features": {"disable": ["hover"]},
            }}),
            &mut warnings,
        );
        let project = layer(
            json!({"ls": {
                "enabled": true,
                "env": {"C": "project"},
                "settings": {"ra": {"cargo": {"extra": true}, "checkOnSave": false}},
                "features": {"disable": ["rename"]},
            }}),
            &mut warnings,
        );
        assert_eq!(warnings, Vec::<String>::new());
        let config = server_config(&def, Some(&global), Some(&project));
        assert_eq!(
            config.settings,
            json!({"ra": {
                "cargo": {"features": "all", "target": "aarch64", "extra": true},
                "checkOnSave": false,
                "list": [3],
            }})
        );
        assert!(config.enabled);
        assert_eq!(
            config.path,
            Some(ConfiguredPath {
                value: "/opt/ls".into(),
                origin: LspSettingsOrigin::Global
            })
        );
        assert_eq!(config.args, ["--global"]);
        assert_eq!(
            config.env,
            BTreeMap::from(
                [("A", "def"), ("B", "global"), ("C", "project")]
                    .map(|(k, v)| (k.into(), v.into()))
            )
        );
        // 프로젝트의 목록이 전역의 것을 이기고 정의의 것에 더해진다.
        assert_eq!(
            config.disabled_features,
            [LspFeature::Rename, LspFeature::Formatting]
        );
    }

    #[test]
    fn without_settings_the_definition_stands() {
        let def = def("{ a = 1 }");
        let config = server_config(&def, None, None);
        assert!(config.enabled);
        assert_eq!(config.path, None);
        assert_eq!(config.args, ["--stdio"]);
        assert_eq!(config.settings, json!({"a": 1}));
        assert_eq!(config.disabled_features, [LspFeature::Formatting]);
    }

    #[test]
    fn wrong_types_are_warned_and_dropped_key_by_key() {
        let mut warnings = Vec::new();
        let layers = parse_layer(
            Some(&json!({
                "idle_shutdown_minutes": 10,
                "ls": {
                    "enabled": "yes",
                    "path": "",
                    "args": "--stdio",
                    "env": {"A": 1, "B": "ok"},
                    "settings": [1],
                    "features": {"disable": ["hover", "inlay_hints"]},
                    "command": "x",
                },
            })),
            "settings.toml",
            &mut warnings,
        );
        assert_eq!(layers.keys().collect::<Vec<_>>(), ["ls"]);
        let ls = &layers["ls"];
        assert_eq!(ls.enabled, None);
        assert_eq!(ls.path, None);
        assert_eq!(ls.args, None);
        assert_eq!(ls.env, BTreeMap::from([("B".into(), "ok".into())]));
        assert_eq!(ls.settings, None);
        assert_eq!(ls.disable, Some(vec![LspFeature::Hover]));
        let at = "settings.toml: `[lsp.ls]`";
        assert_eq!(
            warnings,
            [
                format!("{at}: `args` must be an array of strings; ignored"),
                format!("{at}: unknown key `command` is ignored"),
                format!("{at}: `enabled` must be true or false; ignored"),
                format!("{at}: `env.A` must be a string with a variable name; ignored"),
                format!("{at}: unknown feature `inlay_hints` in `features.disable` is ignored"),
                format!("{at}: `path` must be a non-empty string; ignored"),
                format!("{at}: `settings` must be a table; ignored"),
            ]
        );
    }

    #[test]
    fn set_path_builds_the_nested_objects() {
        let mut options = json!({"tsserver": {"logVerbosity": "off"}, "x": 1});
        set_path(
            &mut options,
            "tsserver.path",
            json!("/p/node_modules/typescript/lib"),
        );
        set_path(&mut options, "x.y", json!(true));
        assert_eq!(
            options,
            json!({
                "tsserver": {"logVerbosity": "off", "path": "/p/node_modules/typescript/lib"},
                "x": {"y": true},
            })
        );
    }
}
