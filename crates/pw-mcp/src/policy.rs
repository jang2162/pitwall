//! 권한 등급별 정책(`docs/design/05-mcp.md` 「권한 모델」, 16-m5-plan M5-12). 노출([`crate::exposure`])과 별개다 — 보이는 툴도
//! 여기서 거절되거나 승인을 기다릴 수 있다.
//!
//! | 등급 | 설계의 기본 정책 | 지금 |
//! |---|---|---|
//! | `read` | 자동 허용 | 허용. 툴이 호출마다 승인을 청하면(계획 — `pw_core::registry::plan`) 그 호출은 승인을 지난다 |
//! | `write` · `exec` | UI 승인 | 앱 세션은 그 프로젝트 창에 묻는다([`crate::approval`]). `auto_approve` 에 그 등급이 있으면 묻지 않는다. 헤드리스는 **거절**(`headless_write`) |
//! | `dangerous` | 매번 승인, 자동 승인 불가 | 매번 묻는다 — `auto_approve` 에 있어도(경고), 세션 허용도 없다 |
//!
//! **설정**(결정 M5-D31): `[mcp] auto_approve`(등급 목록)는 전역 → 대상 프로젝트로 덮어 읽되 **커밋되는 공유 설정
//! (`.pitwall/project.toml`)은 넓히지 못한다** — 공유 레이어까지 읽은 값과 공유 레이어를 뺀 값에 둘 다 있는 등급만 자동이다
//! (`Exposure::narrowed_by` 와 같은 원리). `[mcp.policy]` 표는 **사람의 설정(전역 · `.pitwall/*.local.toml` · 개인 전용
//! `project.toml`)에서만** 읽는다 — 공유 설정의 그 표는 효과가 없고 경고를 남긴다. 키는 지금 `approval_timeout_secs`(기본 300)
//! 하나이고 `production` 은 16-m5-plan M5-18 이 더한다. 판정은 **대상 프로젝트**의 것이다(프로젝트 없는 호출은 전역만). 설정은
//! 호출마다 디스크에서 쓰지 않고 읽는다(`ConfigSource::without_writes`).
//!
//! 헤드리스에는 승인 경로가 없다 — 승인을 건너뛰는 플래그(`--approve-mode`)는 두지 않는다(결정 M5-D32).

use std::time::Duration;

use pw_core::config::{AppPaths, ConfigSource, ConfigStore, LayerOrigin};
use pw_core::registry::{ConfigMode, Project};
use pw_rpc::Tier;
use serde_json::{Map, Value};

use crate::approval::Approvals;
use crate::exposure::KEY;
use crate::server::HostKind;
use crate::sessions::Sessions;

/// 승인 시한의 기본값(05 「감사 로그」 의 「승인 요청 라우팅」 — 5분).
pub const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
/// `[mcp.policy]` 표의 이름(`[mcp]` 아래).
pub const POLICY_TABLE: &str = "policy";
/// `[mcp]` 의 자동 승인 키.
pub const AUTO_APPROVE: &str = "auto_approve";

/// 승인을 받을 길이 있는가.
#[derive(Clone)]
pub enum Approval {
    /// 승인 경로가 없다(헤드리스 · 앱 없는 CLI · 승인을 붙이지 않은 시험).
    Unavailable,
    /// 그 프로젝트 창에 물을 수 있다(앱).
    Route(Approvals),
}

impl Approval {
    /// Host 종류에 맞는 승인 길: 앱이면 세션 등록부의 승인 허브(`mcp.approvals` 를 듣는 창들), 헤드리스면 없음.
    pub fn for_host(kind: HostKind, sessions: &Sessions) -> Self {
        match kind {
            HostKind::App => Approval::Route(sessions.approvals().clone()),
            HostKind::Headless => Approval::Unavailable,
        }
    }
}

impl std::fmt::Debug for Approval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Approval::Unavailable => "Unavailable",
            Approval::Route(_) => "Route",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// 사람에게(또는 `auto_approve`) 물어야 한다 — 계획을 받아 승인을 지난다.
    Ask,
    /// 거절. 이유는 에이전트에게 그대로 보이고 감사 로그에 남는다.
    Deny(String),
}

/// 선언 등급만으로의 판정. `read` 툴이 호출마다 청하는 승인은 툴의 답(계획)이 정한다 — 여기서는 보지 않는다.
pub fn decide(tier: Tier, approval: &Approval) -> Decision {
    match (tier, approval) {
        (Tier::Read, _) => Decision::Allow,
        (Tier::Write | Tier::Exec | Tier::Dangerous, Approval::Route(_)) => Decision::Ask,
        (Tier::Write | Tier::Exec | Tier::Dangerous, Approval::Unavailable) => {
            Decision::Deny(format!(
                "`{}` tools need approval in the Pitwall app, and this session has no approval route \
                 (start the Pitwall app, then restart this MCP server)",
                tier_name(tier)
            ))
        }
    }
}

pub(crate) fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Read => "read",
        Tier::Write => "write",
        Tier::Exec => "exec",
        Tier::Dangerous => "dangerous",
    }
}

fn parse_tier(s: &str) -> Option<Tier> {
    [Tier::Read, Tier::Write, Tier::Exec, Tier::Dangerous]
        .into_iter()
        .find(|t| tier_name(*t) == s)
}

/// 한 판정 대상(프로젝트 또는 전역)의 승인 정책.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// 묻지 않고 허용하는 등급. `dangerous` 는 들어가지 않는다.
    pub auto_approve: Vec<Tier>,
    /// 사람의 답을 기다리는 시간(`[mcp.policy] approval_timeout_secs`).
    pub approval_timeout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            auto_approve: Vec::new(),
            approval_timeout: DEFAULT_APPROVAL_TIMEOUT,
        }
    }
}

impl Policy {
    pub fn auto_approves(&self, tier: Tier) -> bool {
        tier != Tier::Dangerous && self.auto_approve.contains(&tier)
    }
}

/// 레이어별 `[mcp]` 값에서 정책을 읽는다. 경고는 사람이 읽을 문장이다.
///
/// - `global`: 전역 `[mcp]`.
/// - `project`: 대상 프로젝트의 `[mcp]` — 공유 레이어까지 병합한 것.
/// - `human`: 같은 프로젝트의 `[mcp]` 에서 **공유 레이어를 뺀 것**(전역 · `.local.toml` · 개인 전용 `project.toml`). 개인 전용
///   프로젝트면 `project` 와 같다.
/// - `shared_policy`: 커밋되는 공유 `project.toml` 에 `[mcp.policy]` 가 있다(효과 없음 — 경고).
pub fn read(
    global: Option<&Value>,
    project: Option<&Value>,
    human: Option<&Value>,
    shared_policy: bool,
) -> (Policy, Vec<String>) {
    let mut warnings = Vec::new();
    let full = merged(global, project);
    let narrow = merged(global, human);
    let wide = auto_approve(full.get(AUTO_APPROVE), &mut warnings);
    // 공유 레이어를 뺀 값의 경고는 위와 겹치므로 버린다.
    let ceiling = auto_approve(narrow.get(AUTO_APPROVE), &mut Vec::new());
    let auto_approve = wide.into_iter().filter(|t| ceiling.contains(t)).collect();
    if shared_policy {
        warnings.push(
            "mcp.policy in the shared .pitwall/project.toml has no effect; set it in the global settings \
             or in .pitwall/project.local.toml"
                .to_owned(),
        );
    }
    let mut approval_timeout = DEFAULT_APPROVAL_TIMEOUT;
    match narrow.get(POLICY_TABLE) {
        None => {}
        Some(Value::Object(table)) => {
            if let Some(value) = table.get("approval_timeout_secs") {
                match value.as_u64() {
                    Some(secs) if secs > 0 => approval_timeout = Duration::from_secs(secs),
                    _ => warnings.push(format!(
                        "mcp.policy.approval_timeout_secs must be a positive integer, found {value}; using {}",
                        DEFAULT_APPROVAL_TIMEOUT.as_secs()
                    )),
                }
            }
        }
        Some(other) => warnings.push(format!("mcp.policy must be a table, found {other}")),
    }
    (
        Policy {
            auto_approve,
            approval_timeout,
        },
        warnings,
    )
}

fn merged(global: Option<&Value>, project: Option<&Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for layer in [global, project].into_iter().flatten() {
        if let Value::Object(table) = layer {
            pw_core::config::merge::merge(&mut out, table.clone());
        }
    }
    out
}

fn auto_approve(value: Option<&Value>, warnings: &mut Vec<String>) -> Vec<Tier> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Value::Array(items) = value else {
        warnings.push(format!(
            "mcp.auto_approve must be a list of tiers, found {value}; ignored"
        ));
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        match item.as_str().and_then(parse_tier) {
            Some(Tier::Dangerous) => warnings.push(
                "mcp.auto_approve cannot hold `dangerous`; dangerous tools always ask".to_owned(),
            ),
            Some(tier) if !out.contains(&tier) => out.push(tier),
            Some(_) => {}
            None => warnings.push(format!(
                "mcp.auto_approve: `{item}` is not read, write or exec; ignored"
            )),
        }
    }
    out
}

/// 등록된 프로젝트 하나의 정책: 전역 위에 그 프로젝트 설정(프로젝트 루트)을 [`read`] 로. **디스크에 쓰지 않고** 읽는다.
/// 프로젝트 설정을 읽지 못하면 경고를 남기고 전역만 쓴다(공유 레이어로 넓어지지 않는다). 경고는 로그에 남긴다.
pub fn of_project(paths: &AppPaths, global: &ConfigStore, project: &Project) -> Policy {
    logged(read_project(paths, global, project))
}

/// [`of_project`] 의 경고를 돌려주는 꼴.
pub fn read_project(
    paths: &AppPaths,
    global: &ConfigStore,
    project: &Project,
) -> (Policy, Vec<String>) {
    let global = global.snapshot();
    let global_mcp = global.get("settings", KEY);
    let unreadable = |e: &dyn std::fmt::Display| {
        let (policy, mut warnings) = read(global_mcp, None, None, false);
        warnings.push(format!(
            "cannot read the config of project `{}` for the mcp policy ({e}); using the global settings",
            project.id
        ));
        (policy, warnings)
    };
    let source = match project.config_source(paths, project.root()) {
        Ok(source) => source.without_writes(),
        Err(e) => return unreadable(&e),
    };
    let layered = match source.load_layered() {
        Ok(layered) => layered,
        Err(e) => return unreadable(&e),
    };
    let full = layered.snapshot.get("project", KEY).cloned();
    if project.config_mode != ConfigMode::Repository {
        // 개인 전용 project.toml 은 사람의 설정이다.
        return read(global_mcp, full.as_ref(), full.as_ref(), false);
    }
    let shared_policy = layered.layers.get("project").is_some_and(|layers| {
        layers.iter().any(|l| {
            l.origin == LayerOrigin::Shared
                && l.value.get(KEY).and_then(|m| m.get(POLICY_TABLE)).is_some()
        })
    });
    let (human, broken) = match ConfigSource::without_shared(source).load() {
        Ok(snapshot) => (snapshot.get("project", KEY).cloned(), None),
        // 로컬 파일이 깨졌다 — 공유 레이어만 남기면 넓어지므로 전역만을 사람의 설정으로 둔다.
        Err(e) => (None, Some(e.to_string())),
    };
    let (policy, mut warnings) = read(global_mcp, full.as_ref(), human.as_ref(), shared_policy);
    if let Some(e) = broken {
        warnings.push(format!(
            "cannot read the local config of project `{}` for the mcp policy ({e}); using the global settings",
            project.id
        ));
    }
    (policy, warnings)
}

/// 프로젝트 없는 호출의 정책: 전역 설정만.
pub fn of_global(global: &ConfigStore) -> Policy {
    let global = global.snapshot();
    logged(read(global.get("settings", KEY), None, None, false))
}

fn logged((policy, warnings): (Policy, Vec<String>)) -> Policy {
    for w in warnings {
        tracing::warn!("{w}");
    }
    policy
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn read_is_allowed_and_everything_else_is_denied_without_an_approval_route() {
        assert_eq!(decide(Tier::Read, &Approval::Unavailable), Decision::Allow);
        for tier in [Tier::Write, Tier::Exec, Tier::Dangerous] {
            match decide(tier, &Approval::Unavailable) {
                Decision::Deny(reason) => assert!(reason.contains(tier_name(tier)), "{reason}"),
                other => panic!("{tier:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn with_an_approval_route_non_read_tiers_ask() {
        let route = Approval::Route(Approvals::default());
        assert_eq!(decide(Tier::Read, &route), Decision::Allow);
        for tier in [Tier::Write, Tier::Exec, Tier::Dangerous] {
            assert_eq!(decide(tier, &route), Decision::Ask, "{tier:?}");
        }
    }

    #[test]
    fn auto_approve_reads_tiers_and_never_dangerous() {
        let (policy, warnings) = read(
            Some(&json!({"auto_approve": ["write", "dangerous", "bogus", "write"]})),
            None,
            None,
            false,
        );
        // 프로젝트가 없으면 전역이 두 읽기 모두다.
        assert_eq!(policy.auto_approve, [Tier::Write]);
        let (policy, _) = read(
            Some(&json!({"auto_approve": ["write", "dangerous"]})),
            None,
            Some(&json!({})),
            false,
        );
        assert_eq!(policy.auto_approve, [Tier::Write]);
        assert!(policy.auto_approves(Tier::Write));
        assert!(!policy.auto_approves(Tier::Dangerous));
        assert!(
            warnings.iter().any(|w| w.contains("dangerous")),
            "{warnings:?}"
        );
        assert!(warnings.iter().any(|w| w.contains("bogus")), "{warnings:?}");
    }

    /// 커밋되는 공유 설정은 자동 승인을 넓히지 못하고 좁히기만 한다.
    #[test]
    fn the_shared_layer_cannot_widen_auto_approve() {
        // 공유 project.toml 이 write 를 더했다: 공유 레이어를 뺀 값에는 없다.
        let (policy, _) = read(
            None,
            Some(&json!({"auto_approve": ["write"]})),
            Some(&json!({})),
            false,
        );
        assert!(policy.auto_approve.is_empty());
        // .local.toml 이 더한 것은 먹는다(둘 다에 있다).
        let local = json!({"auto_approve": ["write"]});
        let (policy, _) = read(None, Some(&local), Some(&local), false);
        assert_eq!(policy.auto_approve, [Tier::Write]);
        // 공유 설정이 빈 목록으로 좁히면 좁아진다.
        let (policy, _) = read(
            Some(&json!({"auto_approve": ["write"]})),
            Some(&json!({"auto_approve": []})),
            Some(&json!({})),
            false,
        );
        assert!(policy.auto_approve.is_empty());
    }

    #[test]
    fn the_policy_table_comes_only_from_the_human_layers() {
        let (policy, warnings) = read(
            Some(&json!({"policy": {"approval_timeout_secs": 30}})),
            Some(&json!({"policy": {"approval_timeout_secs": 1}})),
            Some(&json!({})),
            true,
        );
        assert_eq!(policy.approval_timeout, Duration::from_secs(30));
        assert!(
            warnings.iter().any(|w| w.contains("no effect")),
            "{warnings:?}"
        );
        let (policy, warnings) = read(
            None,
            None,
            Some(&json!({"policy": {"approval_timeout_secs": -3}})),
            false,
        );
        assert_eq!(policy.approval_timeout, DEFAULT_APPROVAL_TIMEOUT);
        assert!(!warnings.is_empty());
    }
}
