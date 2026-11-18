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
//!
//! [mcp.groups]
//! db = false                   # 그룹(`catalog::GROUPS`) — 툴의 선언이 그룹을 싣는다
//! ```
//!
//! 판정 순서: 이름이 정확히 같은 항목 → 맞는 glob 중 가장 긴 것 → 그룹 → 프리셋. 그룹은 이름이 아니라 선언의 칸이라 꺼 둔 그룹에
//! 뒤에 더해진 툴도 꺼진 채 나온다(glob `"project_*"` 는 `settings_get` 처럼 접두사 없는 이름을 놓친다 — 결정 M5-D23).
//! **커밋되는 공유 프로젝트 설정(`.pitwall/project.toml`)은 좁히기만 한다** — 넓히는 것은 전역 · `.local.toml` · 개인 전용
//! 설정만([`Exposure::narrowed_by`]). 툴 키 · 그룹 키 · 프리셋 모두 같다. 프리셋은 등급으로 고른다 — `read-only` 는 `read`,
//! `standard` 는 `read` · `write`, `full` 은 전부. 전역 → 프로젝트 병합은 설정 계층의 규칙(`pw_core::config::merge`)이라
//! `[mcp.tools]` · `[mcp.groups]` 는 키별로 합쳐진다.
//!
//! 값이 잘못되면(모르는 프리셋 · 그룹, bool 이 아닌 툴 · 그룹 값) 그 값을 버리고 경고를 남긴다. 모르는 프리셋은 가장 좁은 `read-only` 로 본다 —
//! 설정을 잘못 쓴 사람이 의도보다 많이 열리지 않게.

use std::collections::BTreeSet;

use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{ConfigMode, Project};
use pw_rpc::Tier;
use serde_json::{Map, Value};

use crate::catalog::{Catalog, GROUPS, Tool};

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

    /// 설정 파일에 쓰는 이름.
    pub fn as_str(self) -> &'static str {
        match self {
            Preset::ReadOnly => "read-only",
            Preset::Standard => "standard",
            Preset::Full => "full",
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

/// 툴 하나를 켜거나 끈 규칙(판정 순서의 단계). [`Exposure::verdict`].
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ExposureRule {
    /// 프리셋이 등급으로 골랐다(`[mcp] preset` — 설정이 없으면 `standard`).
    Preset,
    /// `[mcp.tools]` 의 그 이름 키.
    Tool,
    /// `[mcp.tools]` 의 glob 키(맞는 것 가운데 가장 긴 것).
    Glob,
    /// `[mcp.groups]` 의 그 그룹 키.
    Group,
    /// 공유 레이어를 빼면 켜지는데 커밋되는 공유 설정(`.pitwall/project.toml`)이 껐다.
    Shared,
}

/// [`Exposure::verdict`] 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub enabled: bool,
    pub rule: ExposureRule,
    /// 그 규칙의 키: 툴 이름 · glob · 그룹 · 프리셋 이름. [`ExposureRule::Shared`] 는 없다.
    pub key: Option<String>,
}

/// 병합된 `[mcp]` 를 읽은 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exposure {
    pub preset: Preset,
    /// `[mcp.tools]` 의 (이름 또는 glob, 켬).
    pub tools: Vec<(String, bool)>,
    /// `[mcp.groups]` 의 (그룹, 켬). 그룹은 [`GROUPS`] 가운데 하나다.
    pub groups: Vec<(String, bool)>,
    /// 이 위로는 열리지 않는다([`Self::narrowed_by`]).
    pub ceiling: Option<Box<Exposure>>,
}

impl Default for Exposure {
    /// 설정이 없을 때. 내장 기본값(`settings.toml` 의 `mcp.preset`)과 같다.
    fn default() -> Self {
        Self {
            preset: Preset::Standard,
            tools: Vec::new(),
            groups: Vec::new(),
            ceiling: None,
        }
    }
}

impl Exposure {
    /// 전역 `[mcp]` 위에 프로젝트 `[mcp]` 를 덮어 읽는다. 경고는 사람이 읽을 문장이다. 프로젝트 `[mcp]` 에 전역에서만 읽는
    /// 표(`[mcp.audit]` — 결정 M5-D10)가 있으면 효과가 없다는 경고도 여기서 낸다 — 프로젝트의 `[mcp]` 를 읽는 자리가 이것이다.
    pub fn from_layers(global: Option<&Value>, project: Option<&Value>) -> (Self, Vec<String>) {
        let mut merged = Map::new();
        for layer in [global, project].into_iter().flatten() {
            if let Value::Object(table) = layer {
                pw_core::config::merge::merge(&mut merged, table.clone());
            }
        }
        let mut warnings: Vec<String> = crate::audit::project_table_warning(project)
            .into_iter()
            .collect();
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
        let mut groups = Vec::new();
        match merged.get("groups") {
            None => {}
            Some(Value::Object(table)) => {
                for (group, on) in table {
                    match on {
                        Value::Bool(_) if !GROUPS.contains(&group.as_str()) => {
                            warnings.push(format!(
                                "mcp.groups.{group} is not a tool group ({}); ignored",
                                GROUPS.join(", ")
                            ));
                        }
                        Value::Bool(on) => groups.push((group.clone(), *on)),
                        other => warnings.push(format!(
                            "mcp.groups.{group} must be true or false, found {other}; ignored"
                        )),
                    }
                }
            }
            Some(other) => warnings.push(format!("mcp.groups must be a table, found {other}")),
        }
        (
            Self {
                preset,
                tools,
                groups,
                ceiling: None,
            },
            warnings,
        )
    }

    /// `ceiling` 이 켜지 않은 툴은 켜지 않는다. 커밋되는 공유 프로젝트 설정은 노출을 좁히기만 한다 — 공유 레이어까지 읽은 값을
    /// 공유 레이어를 뺀 값(전역 + `.local.toml`)으로 누른다. 저장소를 클론한 것만으로 에이전트에게 열리는 툴이 늘지 않는다.
    #[must_use]
    pub fn narrowed_by(self, ceiling: Exposure) -> Self {
        Self {
            ceiling: Some(Box::new(ceiling)),
            ..self
        }
    }

    pub fn enabled(&self, tool: &Tool) -> bool {
        self.verdict(tool).enabled
    }

    /// 그 툴이 켜졌는지와 그것을 정한 규칙. [`Self::enabled`] 는 이것의 `enabled` 다 — 판정은 이 한 벌이다(`mcp.tools.list` 의
    /// 「꺼진 까닭」 도 이것을 그대로 싣는다).
    ///
    /// 상한([`Self::narrowed_by`])이 있으면: 상한이 끈 툴은 상한의 판정(공유 설정이 없어도 꺼진다), 상한은 켰는데 공유
    /// 설정까지 읽은 판정이 끈 툴은 [`ExposureRule::Shared`], 둘 다 켰으면 공유 설정까지 읽은 판정이다.
    pub fn verdict(&self, tool: &Tool) -> Verdict {
        let own = self.own(tool);
        let Some(ceiling) = &self.ceiling else {
            return own;
        };
        let upper = ceiling.verdict(tool);
        if !upper.enabled {
            upper
        } else if !own.enabled {
            Verdict {
                enabled: false,
                rule: ExposureRule::Shared,
                key: None,
            }
        } else {
            own
        }
    }

    fn own(&self, tool: &Tool) -> Verdict {
        let by = |rule, key: &str, enabled| Verdict {
            enabled,
            rule,
            key: Some(key.to_owned()),
        };
        if let Some((p, on)) = self.tools.iter().find(|(p, _)| *p == tool.name) {
            return by(ExposureRule::Tool, p, *on);
        }
        let glob = self
            .tools
            .iter()
            .filter(|(p, _)| p.contains('*') && glob_match(p, &tool.name))
            .max_by_key(|(p, _)| p.len());
        if let Some((p, on)) = glob {
            return by(ExposureRule::Glob, p, *on);
        }
        match self.groups.iter().find(|(g, _)| g == tool.group) {
            Some((g, on)) => by(ExposureRule::Group, g, *on),
            None => by(
                ExposureRule::Preset,
                self.preset.as_str(),
                self.preset.allows(tool.tier),
            ),
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

/// 전역 `[mcp]` 위에 프로젝트 `[mcp]` 를 덮어 읽는다. `without_shared`(저장소 모드의 공유 레이어를 뺀 읽기)가 있으면 그 값으로
/// 누른다 — 커밋된 설정은 노출을 좁히기만 한다. 세션 프로젝트 · 대상 프로젝트(결정 M5-D28) · `mcp.tools.list` 가 같은 규칙을 쓴다.
pub fn layered(
    global_mcp: Option<&Value>,
    project_mcp: Option<&Value>,
    without_shared: Option<&ConfigSource>,
) -> Exposure {
    let (mut exposure, warnings) = Exposure::from_layers(global_mcp, project_mcp);
    if let Some(source) = without_shared {
        // 공유 레이어를 뺀 값을 다시 읽는다. 읽지 못하면(로컬 파일이 깨짐) 전역만을 상한으로 둔다 — 넓어지지 않는다.
        let local = source.load().ok();
        let (ceiling, _) = Exposure::from_layers(
            global_mcp,
            local.as_ref().and_then(|s| s.get("project", KEY)),
        );
        exposure = exposure.narrowed_by(ceiling);
    }
    for w in warnings {
        tracing::warn!("{w}");
    }
    exposure
}

/// 등록된 프로젝트 하나의 노출: 전역 설정 위에 그 프로젝트 설정(체크아웃은 프로젝트 루트)을 [`layered`] 로 덮는다. 프로젝트
/// 설정은 **디스크에 쓰지 않고** 읽는다(구버전 파일은 메모리에서만 올린다). 읽지 못하면 경고를 남기고 전역만 쓴다.
/// 세션이 대상 프로젝트를 판정할 때(결정 M5-D28)와 `mcp.tools.list { projectId }` 가 쓴다.
pub fn of_project(paths: &AppPaths, global: &ConfigStore, project: &Project) -> Exposure {
    let global = global.snapshot();
    let source = match project.config_source(paths, project.root()) {
        Ok(source) => Some(source.without_writes()),
        Err(e) => {
            tracing::warn!(%e, project = project.id, "cannot read the target project config");
            None
        }
    };
    let loaded = source.as_ref().and_then(|s| match s.load() {
        Ok(snapshot) => Some(snapshot),
        Err(e) => {
            tracing::warn!(%e, project = project.id, "cannot read the target project config");
            None
        }
    });
    let without_shared = source
        .filter(|_| project.config_mode == ConfigMode::Repository)
        .map(ConfigSource::without_shared);
    layered(
        global.get("settings", KEY),
        loaded.as_ref().and_then(|s| s.get("project", KEY)),
        without_shared.as_ref(),
    )
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

    /// `[mcp.audit]` 은 전역 설정에서만 읽는다(결정 M5-D10) — 프로젝트의 `[mcp]` 에 있으면 경고가 남고, 전역에 있으면 남지 않는다.
    #[test]
    fn an_audit_table_in_the_project_settings_leaves_a_warning() {
        let audit = json!({"audit": {"retention_days": 1}});
        let (_, global_only) = Exposure::from_layers(Some(&audit), None);
        assert!(global_only.is_empty(), "{global_only:?}");
        let (_, project) = Exposure::from_layers(Some(&json!({"preset": "full"})), Some(&audit));
        assert_eq!(project.len(), 1, "{project:?}");
        assert!(project[0].contains("mcp.audit") && project[0].contains("no effect"));
    }

    #[test]
    fn a_group_turned_off_hides_every_tool_in_it() {
        let (exposure, warnings) = Exposure::from_layers(
            Some(&json!({"preset": "full", "groups": {"db": false}})),
            None,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let on = |group, name: &str, tier| exposure.enabled(&Tool::fake_in(group, name, tier));
        assert!(!on("db", "db_schema", Tier::Read));
        assert!(!on("db", "db_execute", Tier::Dangerous));
        // 이름에 그룹 접두사가 없어도 — 그룹은 선언의 칸이다.
        assert!(!on("db", "sql_explain", Tier::Read));
        assert!(on("code", "diagnostics", Tier::Read));
        assert!(
            on("project", "db_lookalike", Tier::Read),
            "이름이 아니라 그룹으로"
        );
    }

    #[test]
    fn exact_keys_and_globs_beat_the_group_and_the_group_beats_the_preset() {
        let (exposure, _) = Exposure::from_layers(
            Some(&json!({
                "preset": "read-only",
                "tools": {"db_schema": true, "db_conn*": true, "git_status": false},
                "groups": {"db": false, "git": true}
            })),
            None,
        );
        let on = |group, name: &str, tier| exposure.enabled(&Tool::fake_in(group, name, tier));
        assert!(on("db", "db_schema", Tier::Read), "정확한 키");
        assert!(on("db", "db_connection_list", Tier::Read), "glob");
        assert!(!on("db", "db_query", Tier::Read), "그룹");
        assert!(
            on("git", "git_commit", Tier::Write),
            "그룹이 프리셋(read-only)보다 앞선다"
        );
        assert!(
            !on("git", "git_status", Tier::Read),
            "정확한 키가 켜진 그룹을 이긴다"
        );
        assert!(
            !on("code", "rename", Tier::Write),
            "그룹 키가 없으면 프리셋"
        );
    }

    /// 공유 설정의 `groups` 는 켜지 못하고 끄기만 한다. 로컬은 켠다.
    #[test]
    fn shared_settings_cannot_turn_a_group_on() {
        let global = json!({"groups": {"db": false}});
        let shared = json!({"groups": {"db": true, "git": false}});
        let (with_shared, _) = Exposure::from_layers(Some(&global), Some(&shared));
        let (ceiling, _) = Exposure::from_layers(Some(&global), None);
        let exposure = with_shared.narrowed_by(ceiling);
        let on = |group, name: &str| exposure.enabled(&Tool::fake_in(group, name, Tier::Read));
        assert!(!on("db", "db_schema"), "공유가 켠 그룹은 전역이 끈 채로");
        assert!(!on("git", "git_status"), "공유가 끈 그룹은 꺼진다");
        assert!(on("code", "diagnostics"));

        let local = json!({"groups": {"db": true}});
        let (with_local, _) = Exposure::from_layers(Some(&global), Some(&local));
        let (ceiling, _) = Exposure::from_layers(Some(&global), Some(&local));
        assert!(with_local.narrowed_by(ceiling).enabled(&Tool::fake_in(
            "db",
            "db_schema",
            Tier::Read
        )));
    }

    /// 판정의 규칙: 상한이 켰는데 공유 설정이 끈 툴만 `shared` 다. 공유 설정이 켜려다 상한에 막힌 툴은 상한의 규칙이다.
    #[test]
    fn the_verdict_names_the_rule_and_tells_shared_narrowing_apart() {
        let global = json!({"preset": "read-only", "tools": {"project_info": false}});
        let shared =
            json!({"preset": "full", "tools": {"project_info": true, "project_list": false}});
        let (with_shared, _) = Exposure::from_layers(Some(&global), Some(&shared));
        let (ceiling, _) = Exposure::from_layers(Some(&global), None);
        let exposure = with_shared.narrowed_by(ceiling);
        let verdict = |name: &str, tier| exposure.verdict(&Tool::fake(name, tier));
        let v = |enabled, rule, key: Option<&str>| Verdict {
            enabled,
            rule,
            key: key.map(str::to_owned),
        };
        assert_eq!(
            verdict("project_list", Tier::Read),
            v(false, ExposureRule::Shared, None),
            "공유가 끈 툴"
        );
        assert_eq!(
            verdict("project_info", Tier::Read),
            v(false, ExposureRule::Tool, Some("project_info")),
            "공유가 켜려 했지만 전역의 키가 끈다"
        );
        assert_eq!(
            verdict("settings_set", Tier::Write),
            v(false, ExposureRule::Preset, Some("read-only")),
            "공유의 full 은 넘지 못한다 — 전역 프리셋"
        );
        assert_eq!(
            verdict("project_open", Tier::Read),
            v(true, ExposureRule::Preset, Some("full")),
            "켜진 툴은 공유까지 읽은 판정"
        );
        // 상한이 없으면 shared 는 나오지 않는다. enabled() 는 verdict 의 enabled 다.
        let (plain, _) = Exposure::from_layers(
            Some(&json!({"tools": {"a*": false}, "groups": {"db": false}})),
            None,
        );
        assert_eq!(
            plain.verdict(&Tool::fake("ab", Tier::Read)),
            v(false, ExposureRule::Glob, Some("a*"))
        );
        assert_eq!(
            plain.verdict(&Tool::fake_in("db", "x", Tier::Read)),
            v(false, ExposureRule::Group, Some("db"))
        );
        for name in ["ab", "x", "project_list"] {
            let t = Tool::fake(name, Tier::Read);
            assert_eq!(exposure.enabled(&t), exposure.verdict(&t).enabled);
        }
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

    /// 공유 설정이 넓히려 해도(프리셋 full, 꺼진 툴 켜기) 상한(전역 + 로컬)을 넘지 않는다. 좁히는 것은 그대로 먹고, 로컬은 넓힌다.
    #[test]
    fn a_ceiling_lets_shared_settings_narrow_but_not_widen() {
        let global = json!({"preset": "read-only", "tools": {"project_info": false}});
        let shared = json!({"preset": "full", "tools": {"project_info": true, "project_list": false, "*": true}});
        let (with_shared, _) = Exposure::from_layers(Some(&global), Some(&shared));
        let (ceiling, _) = Exposure::from_layers(Some(&global), None);
        let exposure = with_shared.narrowed_by(ceiling);
        let on = |name: &str, tier| exposure.enabled(&Tool::fake(name, tier));
        assert!(
            !on("project_info", Tier::Read),
            "공유가 켠 툴은 전역이 끈 채로"
        );
        assert!(
            !on("settings_set", Tier::Write),
            "공유의 full 은 전역 read-only 를 넘지 않는다"
        );
        assert!(!on("project_list", Tier::Read), "공유가 끈 툴은 꺼진다");
        assert!(on("project_open", Tier::Read));

        // 로컬(.local.toml)이 켠 것은 상한에도 들어가 켜진다.
        let local = json!({"tools": {"project_info": true}});
        let mut merged = shared.as_object().unwrap().clone();
        pw_core::config::merge::merge(&mut merged, local.as_object().unwrap().clone());
        let (with_shared, _) = Exposure::from_layers(Some(&global), Some(&Value::Object(merged)));
        let (ceiling, _) = Exposure::from_layers(Some(&global), Some(&local));
        assert!(
            with_shared
                .narrowed_by(ceiling)
                .enabled(&Tool::fake("project_info", Tier::Read))
        );
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let mut c = std::process::Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                c.env_remove(name);
            }
        }
        let out = c
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", dir.join(".no-gitconfig"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// git 이 추적하는(커밋된) `.local.toml` 은 사람의 설정이 아니다 — 노출을 넓히지 못한다(17-m6-plan m6-localpolicy). 추적을
    /// 풀면 사람의 설정이라 넓힌다.
    #[test]
    fn a_committed_local_file_cannot_widen_the_exposure() {
        use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
        use pw_core::config::LayerOrigin;
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("settings.toml"),
            "schema = 1\n[mcp]\npreset = \"read-only\"\n",
        )
        .unwrap();
        let global = ConfigStore::open(
            ConfigSource::global(&paths),
            EventBus::new(DEFAULT_CAPACITY),
            Scope::Host,
        );
        let root = std::fs::canonicalize(tmp.path()).unwrap().join("proj");
        let local = root.join(".pitwall/project.local.toml");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "schema = 1\n[mcp]\npreset = \"full\"\n").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["add", "-f", ".pitwall/project.local.toml"]);
        let project = Project {
            id: "0123456789abcdef".into(),
            name: "p".into(),
            host: "local".into(),
            roots: vec![root.display().to_string()],
            favorite: false,
            tags: Vec::new(),
            config_mode: ConfigMode::Repository,
            slug: "p".into(),
            created_at: 0,
            opened_at: None,
        };
        let write_tool = Tool::fake("settings_set", Tier::Write);
        // 재현의 전제: 그 파일을 읽기는 한다(공유 레이어로).
        let layered = ConfigSource::project(&root)
            .without_writes()
            .load_layered()
            .unwrap();
        assert_eq!(
            layered
                .snapshot
                .file("project")
                .unwrap()
                .layers
                .last()
                .unwrap()
                .origin,
            LayerOrigin::Shared
        );
        assert!(!of_project(&paths, &global, &project).enabled(&write_tool));

        git(
            &root,
            &["rm", "-q", "--cached", ".pitwall/project.local.toml"],
        );
        assert!(of_project(&paths, &global, &project).enabled(&write_tool));
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
        let (exposure, warnings) = Exposure::from_layers(
            Some(&json!({"groups": {"lsp": false, "db": "off", "git": false}})),
            None,
        );
        assert_eq!(exposure.groups, [("git".to_owned(), false)]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let (exposure, warnings) = Exposure::from_layers(Some(&json!({"groups": false})), None);
        assert!(exposure.groups.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let (exposure, _) = Exposure::from_layers(None, None);
        assert_eq!(exposure, Exposure::default());
    }
}
