//! MCP 승인 정책의 설정 읽기 — `[mcp] auto_approve` 와 `[mcp.policy]` 표(`docs/design/05-mcp.md` 「권한 모델」, 16-m5-plan
//! M5-12 ⑥ · M5-18 ④). **읽는 함수는 이것 하나다**(메인 판정 2026-11-17 — M5-18): MCP 세션(`pw_mcp::policy` 가 다시 내보낸다)과
//! DB 툴의 관문(`pw_db::gate` — 프로덕션 `ask`)이 같은 함수를 부른다. 툴 크레이트가 `pw-mcp` 에 의존하지 않아서 여기 있다
//! (`crate::registry::plan` 과 같은 까닭).
//!
//! **설정**(결정 M5-D31): `[mcp] auto_approve`(등급 목록)는 전역 → 대상 프로젝트로 덮어 읽되 **커밋되는 공유 설정
//! (`.pitwall/project.toml`)은 넓히지 못한다** — 공유 레이어까지 읽은 값과 공유 레이어를 뺀 값에 둘 다 있는 등급만 자동이다
//! (`pw_mcp::exposure::Exposure::narrowed_by` 와 같은 원리). `[mcp.policy]` 표는 **사람의 설정(전역 · `.pitwall/*.local.toml` ·
//! 개인 전용 `project.toml`)에서만** 읽는다 — 공유 설정의 그 표는 효과가 없고 경고를 남긴다. 키는 둘이다:
//!
//! - `approval_timeout_secs`(기본 300) — 사람의 답을 기다리는 시간.
//! - `production`(`"block"` 기본 · `"ask"`) — 프로덕션 표지가 있는 DB 대상(16-m5-plan 「프로덕션 태그 연결의 판정」). `ask` 면
//!   사람이 연 프로덕션 연결에 한해 조회까지 매번 승인받는다. 모르는 값은 `block` 으로 읽고 경고한다(더 막는 쪽).
//!
//! 판정은 **대상 프로젝트**의 것이다(프로젝트 없는 호출은 전역만). 설정은 호출마다 디스크에서 쓰지 않고 읽는다
//! (`ConfigSource::without_writes`).
//!
//! **프로젝트 설정을 읽지 못하면 정책이 없다**([`ProjectConfigUnreadable`] — fail-closed): 전역으로 떨어지면 깨진 공유 파일 하나가
//! 사람이 `.local.toml` 에 둔 좁히기를 푼다(17-m6-plan M6-01 리뷰 1).

use std::time::Duration;

use pw_rpc::Tier;
use serde_json::{Map, Value};

use super::{AppPaths, ConfigSource, ConfigStore, LayerOrigin};
use crate::registry::{ConfigMode, Project};

/// 설정의 `[mcp]` 표 이름(`pw_mcp::exposure::KEY` 와 같다).
pub const MCP_KEY: &str = "mcp";
/// 승인 시한의 기본값(05 「감사 로그」 의 「승인 요청 라우팅」 — 5분).
pub const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
/// `[mcp.policy]` 표의 이름(`[mcp]` 아래).
pub const POLICY_TABLE: &str = "policy";
/// `[mcp]` 의 자동 승인 키.
pub const AUTO_APPROVE: &str = "auto_approve";
/// `[mcp.policy]` 의 프로덕션 키.
pub const PRODUCTION: &str = "production";

/// `[mcp.policy] production` — 프로덕션 표지가 있는 DB 대상에 대한 에이전트 툴의 정책.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Production {
    /// 조회도 쓰기도 막는다(기본).
    #[default]
    Block,
    /// 사람이 연 연결이면 호출마다 승인받는다(세션 허용 · `auto_approve` 없음). 연 적 없는 연결은 여전히 거절이다.
    Ask,
}

impl Production {
    pub fn as_str(self) -> &'static str {
        match self {
            Production::Block => "block",
            Production::Ask => "ask",
        }
    }
}

/// 한 판정 대상(프로젝트 또는 전역)의 승인 정책.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// 묻지 않고 허용하는 등급. `dangerous` 는 들어가지 않는다.
    pub auto_approve: Vec<Tier>,
    /// 사람의 답을 기다리는 시간(`[mcp.policy] approval_timeout_secs`).
    pub approval_timeout: Duration,
    /// 프로덕션 표지가 있는 DB 대상(`[mcp.policy] production`).
    pub production: Production,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            auto_approve: Vec::new(),
            approval_timeout: DEFAULT_APPROVAL_TIMEOUT,
            production: Production::Block,
        }
    }
}

impl Policy {
    pub fn auto_approves(&self, tier: Tier) -> bool {
        tier != Tier::Dangerous && self.auto_approve.contains(&tier)
    }
}

/// 등급의 설정 이름(`read` · `write` · `exec` · `dangerous`).
pub fn tier_name(tier: Tier) -> &'static str {
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
    let mut production = Production::Block;
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
            if let Some(value) = table.get(PRODUCTION) {
                match value.as_str() {
                    Some("block") => production = Production::Block,
                    Some("ask") => production = Production::Ask,
                    _ => warnings.push(format!(
                        "mcp.policy.production must be \"block\" or \"ask\", found {value}; using \"block\""
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
            production,
        },
        warnings,
    )
}

fn merged(global: Option<&Value>, project: Option<&Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for layer in [global, project].into_iter().flatten() {
        if let Value::Object(table) = layer {
            super::merge::merge(&mut out, table.clone());
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

/// 프로젝트 설정 파일(어느 레이어든 — `.pitwall/project.toml` · `db/*.toml` · `*.local.toml`, 개인 전용 폴더의 것)을 읽지 못해
/// 그 프로젝트의 정책을 정할 수 없다. **전역으로 떨어지지 않는다**(fail-closed — 17-m6-plan M6-01 리뷰 1, 05 「권한 모델」):
/// 공유 파일 하나가 깨져 사람이 `.local.toml` 에 둔 좁히기가 풀리면 안 된다. MCP 세션은 그 프로젝트를 겨누는 에이전트 호출을
/// 거절하고(까닭 `project_config_unreadable`), DB 툴의 관문도 같은 거절이다([`Self::to_rpc_error`]). 사람의 화면은
/// [`Self::detail`](파일 · 까닭)을 보인다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfigUnreadable {
    pub project_id: String,
    /// 읽지 못한 파일(알면).
    pub path: Option<std::path::PathBuf>,
    /// 설정 계층의 오류 글 — 파일의 줄을 실을 수 있어 **사람의 화면에만** 보인다(에이전트 · 감사 · 로그에는 싣지 않는다).
    pub detail: String,
}

/// [`ProjectConfigUnreadable`] 의 `PERMISSION_DENIED` 오류가 싣는 `data.reason`(`pw_mcp::marker` 의 `project_config_unreadable`).
pub const PROJECT_CONFIG_UNREADABLE: &str = "projectConfigUnreadable";

impl ProjectConfigUnreadable {
    fn new(project_id: &str, error: &super::ConfigError) -> Self {
        Self {
            project_id: project_id.to_owned(),
            path: error.path().map(std::path::Path::to_path_buf),
            detail: error.to_string(),
        }
    }

    /// 에이전트에게 보일 문장 — 파일 내용 없이 프로젝트와 할 일만.
    pub fn message(&self) -> String {
        format!(
            "cannot read the settings files of project `{}`, so its MCP policy is unknown and agent calls to it are refused; \
             open the project in Pitwall to see which file is broken and fix it",
            self.project_id
        )
    }

    /// `PERMISSION_DENIED`(`data.reason = "projectConfigUnreadable"`).
    pub fn to_rpc_error(&self) -> pw_rpc::RpcError {
        pw_rpc::RpcError::new(pw_rpc::code::PERMISSION_DENIED, self.message()).with_data(
            serde_json::json!({ "reason": PROJECT_CONFIG_UNREADABLE, "projectId": self.project_id }),
        )
    }
}

impl std::fmt::Display for ProjectConfigUnreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// 등록된 프로젝트 하나의 정책과 경고: 전역 위에 그 프로젝트 설정(프로젝트 루트)을 [`read`] 로. **디스크에 쓰지 않고** 읽는다.
/// 프로젝트 설정의 어느 레이어든 읽지 못하면 [`ProjectConfigUnreadable`] 이다 — 전역으로 떨어지지 않는다. 경고를 로그에 남기는
/// 것은 부른 쪽이다(`pw_mcp::policy::of_project` — 이 크레이트는 로그를 쓰지 않는다).
pub fn read_project(
    paths: &AppPaths,
    global: &ConfigStore,
    project: &Project,
) -> Result<(Policy, Vec<String>), ProjectConfigUnreadable> {
    let global = global.snapshot();
    read_project_over(paths, global.get("settings", MCP_KEY), project)
}

/// [`read_project`] 를 전역 `[mcp]` 값으로(전역 설정 스토어가 없는 Host — 시험 등 — 는 `None`).
pub fn read_project_over(
    paths: &AppPaths,
    global_mcp: Option<&Value>,
    project: &Project,
) -> Result<(Policy, Vec<String>), ProjectConfigUnreadable> {
    let unreadable = |e: &super::ConfigError| ProjectConfigUnreadable::new(&project.id, e);
    let source = project
        .config_source(paths, project.root())
        .map_err(|e| unreadable(&e))?
        .without_writes();
    let layered = source.load_layered().map_err(|e| unreadable(&e))?;
    // 대상 없는 링크는 설정 계층이 알림으로 두고 건너뛴다. 정책 읽기는 그것도 「못 읽음」 이다 — 사람의 `.local.toml` 이 dotfiles
    // 링크이고 그 대상이 사라지면 사람이 둔 좁히기가 조용히 빠진다(17-m6-plan M6-01 재확인 리뷰 1).
    if let Some(path) = layered.snapshot.notices.iter().find_map(|n| match n {
        super::ConfigNotice::BrokenLink { path } => Some(path),
        _ => None,
    }) {
        return Err(ProjectConfigUnreadable {
            project_id: project.id.clone(),
            path: Some(std::path::PathBuf::from(path)),
            detail: format!("{path}: a symbolic link whose target does not exist"),
        });
    }
    let full = layered.snapshot.get("project", MCP_KEY).cloned();
    if project.config_mode != ConfigMode::Repository {
        // 개인 전용 project.toml 은 사람의 설정이다.
        return Ok(read(global_mcp, full.as_ref(), full.as_ref(), false));
    }
    let shared_policy = layered.layers.get("project").is_some_and(|layers| {
        layers.iter().any(|l| {
            l.origin == LayerOrigin::Shared
                && l.value
                    .get(MCP_KEY)
                    .and_then(|m| m.get(POLICY_TABLE))
                    .is_some()
        })
    });
    // 공유 레이어를 뺀 읽기는 위 읽기가 읽은 파일의 부분이라 보통 실패하지 않지만, 그 사이 파일이 바뀌었을 수 있다 — 그래도 닫는다.
    let human = ConfigSource::without_shared(source)
        .load()
        .map_err(|e| unreadable(&e))?
        .get("project", MCP_KEY)
        .cloned();
    Ok(read(
        global_mcp,
        full.as_ref(),
        human.as_ref(),
        shared_policy,
    ))
}

/// 프로젝트 없는 호출의 정책과 경고: 전역 `[mcp]` 만.
pub fn read_global(global: &ConfigStore) -> (Policy, Vec<String>) {
    let global = global.snapshot();
    read(global.get("settings", MCP_KEY), None, None, false)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn production_comes_only_from_the_human_layers() {
        // 기본은 block.
        assert_eq!(
            read(None, None, None, false).0.production,
            Production::Block
        );
        // 공유 project.toml 의 ask 는 효과가 없다(공유 레이어를 뺀 값에 없다) — 경고는 부른 쪽이 `shared_policy` 로 단다.
        let (policy, warnings) = read(
            None,
            Some(&json!({"policy": {"production": "ask"}})),
            Some(&json!({})),
            true,
        );
        assert_eq!(policy.production, Production::Block);
        assert!(
            warnings.iter().any(|w| w.contains("no effect")),
            "{warnings:?}"
        );
        // .local.toml(사람의 설정)의 ask 는 먹는다.
        let local = json!({"policy": {"production": "ask"}});
        assert_eq!(
            read(None, Some(&local), Some(&local), false).0.production,
            Production::Ask
        );
        // 전역의 ask 도 먹고, 사람의 프로젝트 설정이 block 으로 덮는다.
        let global = json!({"policy": {"production": "ask"}});
        assert_eq!(
            read(Some(&global), None, None, false).0.production,
            Production::Ask
        );
        let block = json!({"policy": {"production": "block"}});
        assert_eq!(
            read(Some(&global), Some(&block), Some(&block), false)
                .0
                .production,
            Production::Block
        );
    }

    fn project(root: &std::path::Path, mode: ConfigMode) -> Project {
        Project {
            id: "0123456789abcdef".into(),
            name: "p".into(),
            host: "local".into(),
            roots: vec![root.display().to_string()],
            favorite: false,
            tags: Vec::new(),
            config_mode: mode,
            slug: "p".into(),
            created_at: 0,
            opened_at: None,
        }
    }

    fn put(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// 리뷰(17-m6-plan M6-01 리뷰 1)의 재현: 전역이 `read`·`write`·`exec` 자동 · 프로덕션 `ask`, 사람의 `.local.toml` 이 `read` ·
    /// `block` 으로 좁힌다. 깨진 실행 구성(`run/x.toml`)은 그 좁히기를 풀지 않고, 정책이 든 파일(어느 레이어든)이나 DB 연결
    /// 파일을 못 읽으면 전역으로 떨어지지 않고 정책이 없다.
    #[test]
    fn an_unreadable_project_file_never_falls_back_to_the_global_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        let pw = root.join(".pitwall");
        put(
            &pw.join("project.local.toml"),
            "schema = 1\n[mcp]\nauto_approve = [\"read\"]\n[mcp.policy]\nproduction = \"block\"\n",
        );
        let paths = AppPaths::under(&tmp.path().join("app"));
        let global =
            json!({"auto_approve": ["read", "write", "exec"], "policy": {"production": "ask"}});
        let p = project(&root, ConfigMode::Repository);
        let narrowed = Policy {
            auto_approve: vec![Tier::Read],
            approval_timeout: DEFAULT_APPROVAL_TIMEOUT,
            production: Production::Block,
        };
        assert_eq!(
            read_project_over(&paths, Some(&global), &p).unwrap().0,
            narrowed
        );

        // 깨진 실행 구성 하나는 정책을 바꾸지 않는다(실행 구성은 이 스냅숏에 들지 않는다).
        put(&pw.join("run/x.toml"), "name = \"x\n");
        assert_eq!(
            read_project_over(&paths, Some(&global), &p).unwrap().0,
            narrowed
        );

        for (file, text) in [
            ("db/y.toml", "name = \"y\n"),
            ("project.toml", "schema = 1\n[mcp\n"),
            (
                "project.local.toml",
                "schema = 1\n[mcp]\nauto_approve = [\"read\"\n",
            ),
        ] {
            let path = pw.join(file);
            let before = std::fs::read_to_string(&path).ok();
            put(&path, text);
            let err = read_project_over(&paths, Some(&global), &p).unwrap_err();
            assert_eq!(err.project_id, p.id, "{file}");
            assert_eq!(err.path.as_deref(), Some(path.as_path()), "{file}");
            // 에이전트에게 가는 글에는 파일 내용이 없다.
            let rpc = err.to_rpc_error();
            assert_eq!(rpc.code, pw_rpc::code::PERMISSION_DENIED);
            assert_eq!(rpc.data.unwrap()["reason"], PROJECT_CONFIG_UNREADABLE);
            assert!(
                !rpc.message.contains("auto_approve") && !rpc.message.contains(file),
                "{}",
                rpc.message
            );
            match before {
                Some(text) => put(&path, &text),
                None => std::fs::remove_file(&path).unwrap(),
            }
        }
        assert_eq!(
            read_project_over(&paths, Some(&global), &p).unwrap().0,
            narrowed
        );

        // 크기 상한(1 MiB)을 넘는 파일은 문법이 맞아도 읽지 않는다 — 거절(fail-closed).
        let big = pw.join("db/big.toml");
        put(
            &big,
            &format!(
                "name = \"big\"\n# {}\n",
                "x".repeat(super::super::robust::MAX_CONFIG_BYTES as usize)
            ),
        );
        let err = read_project_over(&paths, Some(&global), &p).unwrap_err();
        assert_eq!(err.path.as_deref(), Some(big.as_path()));
        assert!(err.detail.contains("larger than"), "{}", err.detail);
        std::fs::remove_file(&big).unwrap();

        // 사람의 `.local.toml` 이 대상 없는 링크(옮겨진 dotfiles)여도 거절이다 — 건너뛰면 좁히기가 빠져 전역으로 넓어진다.
        #[cfg(unix)]
        {
            let local = pw.join("project.local.toml");
            let saved = std::fs::read_to_string(&local).unwrap();
            std::fs::remove_file(&local).unwrap();
            std::os::unix::fs::symlink(tmp.path().join("moved-away.toml"), &local).unwrap();
            let err = read_project_over(&paths, Some(&global), &p).unwrap_err();
            assert_eq!(err.path.as_deref(), Some(local.as_path()));
            std::fs::remove_file(&local).unwrap();
            put(&local, &saved);
            assert_eq!(
                read_project_over(&paths, Some(&global), &p).unwrap().0,
                narrowed
            );
        }

        // 개인 전용 프로젝트도 같다.
        let personal = project(&root, ConfigMode::Personal);
        put(
            &paths.personal_project_dir("p").join("project.toml"),
            "[mcp\n",
        );
        assert!(read_project_over(&paths, Some(&global), &personal).is_err());
    }

    #[test]
    fn an_unknown_production_value_blocks_and_warns() {
        for value in [json!("allow"), json!(true), json!("ASK")] {
            let (policy, warnings) = read(
                Some(&json!({"policy": {"production": value}})),
                None,
                None,
                false,
            );
            assert_eq!(policy.production, Production::Block, "{value}");
            assert!(
                warnings.iter().any(|w| w.contains("mcp.policy.production")),
                "{warnings:?}"
            );
        }
    }
}
