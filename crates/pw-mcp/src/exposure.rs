//! 툴 노출 설정(`docs/design/05-mcp.md` §3-1): 꺼진 툴은 `tools/list` 에 나오지 않고 부를 수도 없다.
//!
//! ```toml
//! # $CONFIG/settings.toml (전역) — 프로젝트의 .pitwall/project.toml(또는 .local.toml)이 같은 키로 덮는다
//! [mcp]
//! preset = "standard"          # read-only | standard | full
//!
//! [mcp.tools]
//! project_info = false         # 툴 하나
//! "project_*" = true           # glob(`*` 는 아무 글자열)
//! ```
//!
//! 판정 순서: 이름이 정확히 같은 항목 → 맞는 glob 중 가장 긴 것 → 프리셋. 프리셋은 등급으로 고른다 — `read-only` 는 `read`,
//! `standard` 는 `read` · `write`, `full` 은 전부. 전역 → 프로젝트 병합은 설정 계층의 규칙(`pw_core::config::merge`)이라
//! `[mcp.tools]` 는 키별로 합쳐진다.
//!
//! 값이 잘못되면(모르는 프리셋, bool 이 아닌 툴 값) 그 값을 버리고 경고를 남긴다. 모르는 프리셋은 가장 좁은 `read-only` 로 본다 —
//! 설정을 잘못 쓴 사람이 의도보다 많이 열리지 않게.

use std::collections::BTreeSet;

use pw_rpc::Tier;
use serde_json::{Map, Value};

use crate::catalog::{Catalog, Tool};

/// 설정 파일 키(`settings.toml` 과 프로젝트 `project.toml` 의 `[mcp]`).
pub const KEY: &str = "mcp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    ReadOnly,
    Standard,
    Full,
}

impl Preset {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read-only" => Some(Preset::ReadOnly),
            "standard" => Some(Preset::Standard),
            "full" => Some(Preset::Full),
            _ => None,
        }
    }

    pub fn allows(self, tier: Tier) -> bool {
        match self {
            Preset::ReadOnly => tier == Tier::Read,
            Preset::Standard => matches!(tier, Tier::Read | Tier::Write),
            Preset::Full => true,
        }
    }
}

/// 병합된 `[mcp]` 를 읽은 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exposure {
    pub preset: Preset,
    /// `[mcp.tools]` 의 (이름 또는 glob, 켬).
    pub tools: Vec<(String, bool)>,
}

impl Default for Exposure {
    /// 설정이 없을 때. 내장 기본값(`settings.toml` 의 `mcp.preset`)과 같다.
    fn default() -> Self {
        Self {
            preset: Preset::Standard,
            tools: Vec::new(),
        }
    }
}

impl Exposure {
    /// 전역 `[mcp]` 위에 프로젝트 `[mcp]` 를 덮어 읽는다. 경고는 사람이 읽을 문장이다.
    pub fn from_layers(global: Option<&Value>, project: Option<&Value>) -> (Self, Vec<String>) {
        let mut merged = Map::new();
        for layer in [global, project].into_iter().flatten() {
            if let Value::Object(table) = layer {
                pw_core::config::merge::merge(&mut merged, table.clone());
            }
        }
        let mut warnings = Vec::new();
        let preset = match merged.get("preset") {
            None => Preset::Standard,
            Some(Value::String(s)) => Preset::parse(s).unwrap_or_else(|| {
                warnings.push(format!(
                    "mcp.preset `{s}` is not read-only, standard or full; using read-only"
                ));
                Preset::ReadOnly
            }),
            Some(other) => {
                warnings.push(format!(
                    "mcp.preset must be a string, found {other}; using read-only"
                ));
                Preset::ReadOnly
            }
        };
        let mut tools = Vec::new();
        match merged.get("tools") {
            None => {}
            Some(Value::Object(table)) => {
                for (name, on) in table {
                    match on {
                        Value::Bool(on) => tools.push((name.clone(), *on)),
                        other => warnings.push(format!(
                            "mcp.tools.{name} must be true or false, found {other}; ignored"
                        )),
                    }
                }
            }
            Some(other) => warnings.push(format!("mcp.tools must be a table, found {other}")),
        }
        (Self { preset, tools }, warnings)
    }

    pub fn enabled(&self, tool: &Tool) -> bool {
        if let Some((_, on)) = self.tools.iter().find(|(p, _)| *p == tool.name) {
            return *on;
        }
        let glob = self
            .tools
            .iter()
            .filter(|(p, _)| p.contains('*') && glob_match(p, &tool.name))
            .max_by_key(|(p, _)| p.len());
        match glob {
            Some((_, on)) => *on,
            None => self.preset.allows(tool.tier),
        }
    }

    /// 보이는 툴의 이름.
    pub fn visible(&self, catalog: &Catalog) -> BTreeSet<String> {
        catalog
            .tools()
            .iter()
            .filter(|t| self.enabled(t))
            .map(|t| t.name.clone())
            .collect()
    }
}

/// `*` 만 특별한 glob. `*` 는 빈 글자열을 포함한 아무 글자열이다.
fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, rest) = parts.split_first().expect("split 은 하나 이상을 낸다");
    let Some(mut tail) = name.strip_prefix(first) else {
        return false;
    };
    let Some((last, middle)) = rest.split_last() else {
        return tail.is_empty();
    };
    for part in middle {
        match tail.find(part) {
            Some(i) => tail = &tail[i + part.len()..],
            None => return false,
        }
    }
    tail.len() >= last.len() && tail.ends_with(last)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn glob_matches_star_anywhere() {
        assert!(glob_match("project_*", "project_list"));
        assert!(glob_match("*_list", "project_list"));
        assert!(glob_match("p*t_l*", "project_list"));
        assert!(glob_match("*", "x"));
        assert!(!glob_match("project_*", "run_list"));
        assert!(!glob_match("*_info", "project_list"));
        assert!(!glob_match("project_list_*", "project_list"));
        assert!(!glob_match("ab*ba", "aba"));
    }

    #[test]
    fn presets_choose_by_tier() {
        assert!(Preset::ReadOnly.allows(Tier::Read));
        assert!(!Preset::ReadOnly.allows(Tier::Write));
        assert!(Preset::Standard.allows(Tier::Write));
        assert!(!Preset::Standard.allows(Tier::Exec));
        assert!(!Preset::Standard.allows(Tier::Dangerous));
        assert!(Preset::Full.allows(Tier::Dangerous));
    }

    #[test]
    fn an_exact_name_beats_globs_and_the_longest_glob_beats_the_preset() {
        let (exposure, _) = Exposure::from_layers(
            Some(&json!({
                "preset": "standard",
                "tools": {"project_*": false, "project_l*": true, "project_info": true, "run_*": true}
            })),
            None,
        );
        let on = |name: &str, tier| exposure.enabled(&Tool::fake(name, tier));
        assert!(on("project_info", Tier::Read), "정확한 이름");
        assert!(on("project_list", Tier::Read), "더 긴 glob");
        assert!(!on("project_open", Tier::Read), "짧은 glob");
        assert!(on("run_start", Tier::Exec), "glob 이 프리셋보다 앞선다");
        assert!(on("settings_set", Tier::Write), "프리셋 standard");
        assert!(!on("db_execute", Tier::Dangerous), "프리셋 standard");
    }

    #[test]
    fn project_layer_overrides_global_key_by_key() {
        let global = json!({"preset": "full", "tools": {"a": false, "b": false}});
        let project = json!({"preset": "read-only", "tools": {"b": true}});
        let (exposure, warnings) = Exposure::from_layers(Some(&global), Some(&project));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(exposure.preset, Preset::ReadOnly);
        assert_eq!(
            exposure.tools,
            [("a".to_owned(), false), ("b".to_owned(), true)]
        );
    }

    #[test]
    fn bad_values_fall_back_to_the_narrowest_preset_and_are_reported() {
        let (exposure, warnings) = Exposure::from_layers(
            Some(&json!({"preset": "everything", "tools": {"x": "yes"}})),
            None,
        );
        assert_eq!(exposure.preset, Preset::ReadOnly);
        assert!(exposure.tools.is_empty());
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let (exposure, _) = Exposure::from_layers(None, None);
        assert_eq!(exposure, Exposure::default());
    }
}
