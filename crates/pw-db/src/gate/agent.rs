//! 에이전트의 DB 입구(16-m5-plan.md M5-06 ①, 결정 M5-D2 · M5-D27 · M5-D37). 판정 순서와 두 기억(「사람이 연 연결」 · 대상에 묶은
//! 표지)은 상위 모듈 머리 주석의 「에이전트의 입구」.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pw_core::config::mcp_policy::{self, Production};
use pw_core::registry::Project;
use pw_core::registry::agent::AgentGrant;
use pw_core::registry::path::simplify;
use pw_core::registry::plan::Approved;
use pw_db_types::DriverKind;
use pw_rpc::{RpcError, code as rpc_code};
use serde_json::json;

use super::DbGrant;
use crate::config::{DbConnectionConfig, Loaded, LoadedConnection, check_connection_id, lock};
use crate::session::DbSessions;

/// `data.reason`: 사람이 이번 앱 실행에서 그 프로젝트 창으로 (그 연결 id, 지금 대상)을 연 적이 없다.
pub const NOT_OPENED_BY_USER: &str = "notOpenedByUser";
/// `data.reason`: 프로덕션 표지가 있는 대상이다(`[mcp.policy] production` 의 `block` — 기본). `ask` 면 사람이 연 연결에 한해
/// 호출마다 승인을 청한다([`DbGrant::needs_approval`]).
pub const PRODUCTION_BLOCKED: &str = "productionBlocked";

/// 푼 대상 — 「사람이 연 연결」 의 열쇠(표지의 열쇠는 [`DbTarget::mark_key`]). 드라이버 · 호스트 · 포트(적지 않았으면 드라이버
/// 기본) · DB · 사용자 · 터널 목적지, SQLite 면 파일의 실제 경로(링크를 끝까지 푼 것 — 워크트리마다 상대 경로가 다른 파일을
/// 가리킨다). 호스트는 소문자로 맞춘다(DNS 이름은 대소문자를 가리지 않는다 — `DB.example.com` 으로 표지를 비껴가지 못하게). TLS
/// 설정은 넣지 않는다 — 같은 서버 · DB 를 `sslmode` 만 바꿔 가리켜 표지를 비껴가지 못하게(TLS 의 확인은 `crate::trust` 가 따로 본다).
/// 남는 한계: 호스트 별칭(`localhost` · `127.0.0.1`), 터널 목적지의 다른 표기, DB 이름의 대소문자는 다른 대상으로 본다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbTarget {
    pub driver: DriverKind,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub tunnel: Option<String>,
    pub file: Option<PathBuf>,
}

impl DbTarget {
    /// 연결 설정의 대상. `root` 는 그 설정을 읽은 활성 워크트리 안의 프로젝트 루트(SQLite 상대 경로의 기준).
    pub fn of(config: &DbConnectionConfig, root: &Path) -> Self {
        let file = config.path.as_deref().map(|path| {
            let path = Path::new(path);
            let joined = if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            std::fs::canonicalize(&joined).map_or(joined, simplify)
        });
        Self {
            driver: config.driver,
            host: config
                .host
                .as_deref()
                .map(|h| h.trim_end_matches('.').to_ascii_lowercase()),
            port: config.effective_port(),
            database: config.database.clone(),
            user: config.user.clone(),
            tunnel: config.ssh_tunnel.as_ref().map(|t| t.host_ref.clone()),
            file,
        }
    }

    /// 푼 연결의 대상. SQLite 는 신뢰 판정이 드라이버에 넘길 실제 경로(`spec` 의 파일)로 만든다 — 판정한 경로와 견주는 경로가
    /// 갈리지 않게(그 사이에 링크를 바꿔 끼워도).
    pub(crate) fn of_resolved(resolved: &crate::trust::ResolvedConnection) -> Self {
        let mut target = Self::of(&resolved.config, &resolved.root);
        if let pw_db_types::Target::File { path } = &resolved.spec.target {
            target.file = Some(path.clone());
        }
        target
    }

    /// 표지의 열쇠(결정 M5-D37 — 메인 판정 2026-11-14). 한 연결로 다른 DB 를 읽을 수 있는 드라이버(MySQL · MariaDB — 드라이버 id
    /// `mysql`)는 DB 이름을 뺀다: 그 서버의 어느 DB 가 프로덕션이면 그 서버 · 사용자로 가는 에이전트 호출은 모두 프로덕션이다
    /// (`db_query` 가 SQL 글 안의 `other_db.t` 를 지킬 수 없다). Redis 도 DB 번호를 뺀다(메인 판정 2026-11-17 — M5-19): 한 연결이
    /// `SELECT` 로 다른 번호로 옮겨 가고 번호 사이에 권한 경계가 없다. 번호를 열쇠에 두면 같은 번호의 다른 표기(`database` 입력
    /// `"00"` · `"+0"`, 설정에서 뺀 번호와 `"0"`)로 표지를 비껴간다 — 드라이버는 그 모두를 같은 번호로 읽는다. PG 는 DB 이름을
    /// 둔다(한 연결이 다른 DB 를 읽지 못한다). SQLite 는 파일이 열쇠다(`ATTACH` 는 실행 분류의 몫 — M5-07).
    ///
    /// Redis 는 사용자도 맞춘다(메인 판정 2026-11-17 — M5-19 리뷰 R1): 사용자를 적지 않은 것 · 빈 글 · `default` 는 모두 Redis 의
    /// `default` 사용자다(드라이버는 빈 사용자를 적지 않은 것으로 다룬다). ACL 사용자 이름은 대소문자를 가리므로 `Default` 는 다른
    /// 사용자다.
    pub fn mark_key(&self) -> Self {
        match self.driver {
            DriverKind::Mysql => Self {
                database: None,
                ..self.clone()
            },
            DriverKind::Redis => Self {
                database: None,
                user: self
                    .user
                    .clone()
                    .filter(|u| !u.is_empty() && u != REDIS_DEFAULT_USER),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    /// 호출이 설정의 DB 대신 고른 DB 의 대상. `None` 이면 그대로.
    pub fn with_database(&self, database: Option<&str>) -> Self {
        match database {
            Some(db) => Self {
                database: Some(db.to_owned()),
                ..self.clone()
            },
            None => self.clone(),
        }
    }
}

/// Redis 에서 사용자를 적지 않은 `AUTH` 가 쓰는 사용자.
const REDIS_DEFAULT_USER: &str = "default";

/// 에이전트에게 적용하는 표지(결정 M5-D37). 조여지기만 한다([`DbMarks::tighten`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbMarks {
    pub production: bool,
    pub read_only: bool,
}

impl DbMarks {
    pub fn of(config: &DbConnectionConfig) -> Self {
        Self {
            production: config.production,
            read_only: config.read_only,
        }
    }

    /// 둘 가운데 엄한 것.
    pub fn tighten(self, other: Self) -> Self {
        Self {
            production: self.production || other.production,
            read_only: self.read_only || other.read_only,
        }
    }
}

/// 대상 → 이번 실행 중 본 가장 엄한 표지. `Connections` 가 하나 갖는다.
#[derive(Debug, Default)]
pub(crate) struct MarkMemory(Mutex<HashMap<DbTarget, DbMarks>>);

impl MarkMemory {
    /// 읽어 올린 연결 설정들의 표지를 더한다(`Connections::load` · `refresh`). 틀린 설정은 대상을 풀 수 없어 건너뛴다.
    pub fn see(&self, root: &Path, list: &[LoadedConnection]) {
        let seen: Vec<(DbTarget, DbMarks)> = list
            .iter()
            .filter_map(|c| c.config.as_ref().ok())
            .map(|config| (DbTarget::of(config, root).mark_key(), DbMarks::of(config)))
            .filter(|(_, marks)| *marks != DbMarks::default())
            .collect();
        if seen.is_empty() {
            return;
        }
        let mut memory = lock(&self.0);
        for (target, marks) in seen {
            let entry = memory.entry(target).or_default();
            *entry = entry.tighten(marks);
        }
    }

    /// 그 대상에서 본 가장 엄한 표지(본 적 없으면 표지 없음).
    pub fn get(&self, target: &DbTarget) -> DbMarks {
        lock(&self.0)
            .get(&target.mark_key())
            .copied()
            .unwrap_or_default()
    }
}

/// 「사람이 연 연결」: (프로젝트 id, 연결 id, 푼 대상). `DbSessions` 가 하나 갖는다.
#[derive(Debug, Default)]
pub(crate) struct OpenedMemory(Mutex<HashSet<(String, String, DbTarget)>>);

impl OpenedMemory {
    pub fn record(&self, project_id: &str, connection_id: &str, target: DbTarget) {
        let added = lock(&self.0).insert((project_id.to_owned(), connection_id.to_owned(), target));
        if added {
            tracing::info!(target: "pw_db", project = project_id, connection = connection_id, "db connection opened by the user; agent tools may use it");
        }
    }

    pub fn contains(&self, project_id: &str, connection_id: &str, target: &DbTarget) -> bool {
        lock(&self.0).contains(&(
            project_id.to_owned(),
            connection_id.to_owned(),
            target.clone(),
        ))
    }
}

/// 에이전트의 증거가 묶인 판정.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBinding {
    pub(super) connection_id: String,
    /// 연결 이름(설정의 `name` — 승인 요청이 보인다).
    pub(super) name: String,
    pub(super) worktree: Option<String>,
    pub(super) target: DbTarget,
    pub(super) marks: DbMarks,
    pub(super) approval: AgentApproval,
}

impl AgentBinding {
    /// 이 판정으로 쓰는 호출마다 승인을 청한다(프로덕션 대상 · `production = "ask"`).
    pub fn asks_approval(&self) -> bool {
        self.approval == AgentApproval::Required
    }
}

/// 프로덕션 `ask` 의 승인(16-m5-plan M5-18 ④ — 「프로덕션 태그 연결의 판정」 표).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentApproval {
    /// 프로덕션 대상이 아니다 — 승인 없이 쓴다(쓰기 툴은 제 등급으로 따로 묻는다).
    NotNeeded,
    /// 프로덕션 대상이고 `production = "ask"` 다 — 이 호출의 승인([`DbGrant::approve`])을 받기 전에는 연결을 열지 않는다.
    Required,
    /// 그 승인을 받았다(계획 해시).
    Given(String),
}

/// 그 프로젝트의 `[mcp.policy] production`(사람의 설정에서만 — 읽는 함수는 MCP 세션과 같은 [`mcp_policy::read_project_over`],
/// 메인 판정 2026-11-17). 전역 설정 스토어가 없는 Host(시험 등)는 프로젝트 설정만 본다. 경고는 로그에 남긴다. 블로킹이다.
pub(crate) fn production_policy(sessions: &DbSessions, project: &Project) -> Production {
    let Ok(host) = sessions.connections.host() else {
        return Production::Block;
    };
    let global = sessions.connections.settings.store().map(|s| s.snapshot());
    let global_mcp = global
        .as_ref()
        .and_then(|g| g.get("settings", mcp_policy::MCP_KEY));
    let (policy, warnings) = mcp_policy::read_project_over(&host.paths, global_mcp, project);
    for w in warnings {
        tracing::warn!(target: "pw_db", project = %project.id, "{w}");
    }
    policy.production
}

/// `PERMISSION_DENIED`(`data.reason = "notOpenedByUser"`).
pub fn not_opened_by_user(project_id: &str, connection_id: &str) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        format!(
            "db connection `{connection_id}` was not opened in Pitwall during this run (or its target changed since): \
             open it once in the project's DB explorer in Pitwall, then call again"
        ),
    )
    .with_data(json!({
        "reason": NOT_OPENED_BY_USER,
        "projectId": project_id,
        "connectionId": connection_id,
    }))
}

/// `PERMISSION_DENIED`(`data.reason = "productionBlocked"`).
pub fn production_blocked(project_id: &str, connection_id: &str) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        format!(
            "db connection `{connection_id}` points at a target marked production; agent tools do not use production connections"
        ),
    )
    .with_data(json!({
        "reason": PRODUCTION_BLOCKED,
        "projectId": project_id,
        "connectionId": connection_id,
    }))
}

/// `PERMISSION_DENIED`(`data.reason = "readOnlyConnection"`): 판정 뒤 그 대상에 read-only 표지가 더해졌다.
fn marked_read_only(project_id: &str, connection_id: &str) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        format!(
            "db connection `{connection_id}` was marked read-only after this call was checked; nothing ran — call again"
        ),
    )
    .with_data(json!({
        "reason": "readOnlyConnection",
        "projectId": project_id,
        "connectionId": connection_id,
    }))
}

/// 에이전트의 DB 툴 호출이 읽는 연결 설정: **그 저장소의 메인 체크아웃**의 것(표지를 더하려고 — 결정 M5-D37)과 호출의 워크트리의
/// 것. 메인 체크아웃은 그 프로젝트의 주 루트이고, 연결 워크트리를 따로 등록한 프로젝트면 신뢰가 따르는 메인 저장소 프로젝트
/// (`Registry::main_repository_of`)의 주 루트도 읽는다. 돌려주는 것은 호출의 워크트리의 것이다. 메인 체크아웃의 설정을 읽지 못하면
/// (깨진 파일 등) 그 표지는 더하지 못하고 넘어간다 — 앞서 이번 실행에서 읽힌 표지는 기억에 남아 있다. 블로킹이다.
pub(crate) fn agent_view(
    sessions: &DbSessions,
    project_id: &str,
    worktree: Option<&str>,
) -> Result<Loaded, RpcError> {
    let connections = &sessions.connections;
    let main_repository = connections
        .host()?
        .projects
        .main_repository_of(project_id)
        .ok()
        .flatten()
        .map(|p| p.id);
    let mains = worktree
        .is_some()
        .then(|| project_id.to_owned())
        .into_iter()
        .chain(main_repository);
    for main in mains {
        if let Err(e) = connections.load(&main, None) {
            tracing::info!(target: "pw_db", project = %main, %e, "cannot read the main checkout's db connections for agent marks");
        }
    }
    connections.load(project_id, worktree)
}

/// 틀린 연결 설정의 까닭(serde 오류 글)은 툴 응답 · 감사 로그로 내보내지 않는다 — 사람이 실수로 적은 비밀 값이 그 글에 실린다.
pub(crate) fn invalid_settings(connection_id: &str) -> String {
    format!("db connection `{connection_id}` has invalid settings; open it in Pitwall to see why")
}

/// 프로젝트 설정을 읽지 못한 까닭도 같다(설정 계층의 오류 글은 파일 줄을 싣는다).
pub(crate) const UNREADABLE_SETTINGS: &str =
    "cannot read the project's settings files; open the project in Pitwall to see why";

/// 연결 하나에 대한 에이전트의 판정(신뢰 게이트는 부른 쪽이 지났다). `database` 는 호출이 설정의 DB 대신 고른 DB — 그 대상의
/// 표지도 본다. `production` 은 그 프로젝트의 `[mcp.policy] production`([`production_policy`]): 프로덕션 대상이면 `block` 은
/// 거절(`productionBlocked`), `ask` 는 「사람이 연 연결」 판정을 그대로 지난 뒤 승인을 청하는 증거다(연 적 없으면 `ask` 여도
/// `notOpenedByUser` — 결정 M5-D21).
pub(crate) fn judge_for_agent(
    sessions: &DbSessions,
    loaded: &Loaded,
    connection: &LoadedConnection,
    database: Option<&str>,
    production: Production,
) -> Result<AgentBinding, RpcError> {
    let project_id = loaded.project.id.as_str();
    let config = connection
        .config
        .as_ref()
        .map_err(|_| RpcError::invalid_params(invalid_settings(&connection.id)))?;
    let target = DbTarget::of(config, &loaded.root);
    let marks_memory = &sessions.connections.marks;
    let mut marks = DbMarks::of(config).tighten(marks_memory.get(&target));
    if database.is_some_and(|db| config.database.as_deref() != Some(db)) {
        marks = marks.tighten(marks_memory.get(&target.with_database(database)));
    }
    if marks.production && production == Production::Block {
        tracing::info!(target: "pw_db", project = project_id, connection = %connection.id, "agent db access refused: production target");
        return Err(production_blocked(project_id, &connection.id));
    }
    if !sessions
        .opened
        .contains(project_id, &connection.id, &target)
    {
        tracing::info!(target: "pw_db", project = project_id, connection = %connection.id, "agent db access refused: not opened by the user");
        return Err(not_opened_by_user(project_id, &connection.id));
    }
    Ok(AgentBinding {
        connection_id: connection.id.clone(),
        name: config.name.clone(),
        worktree: None,
        target,
        marks,
        approval: if marks.production {
            AgentApproval::Required
        } else {
            AgentApproval::NotNeeded
        },
    })
}

/// MCP 세션의 DB 입구(가칭 `db_grant_for_agent` 그대로). `agent` 는 `pw_core::registry::agent::agent_grant` 를 지난 증거,
/// `worktree` 는 호출의 활성 워크트리 꼭대기(없으면 메인 체크아웃), `database` 는 호출이 설정의 DB 대신 고를 DB(없으면 설정의 것).
///
/// 신뢰 게이트(`untrusted`) → 연결 설정 읽기(메인 체크아웃 · 그 워크트리 — 표지를 더한다) → 프로덕션 표지(`[mcp.policy]
/// production` 이 `block` 이면 `productionBlocked`) → 「사람이 연 연결」(`notOpenedByUser`). 프로덕션 대상을 `ask` 로 지난 증거는
/// [`DbGrant::needs_approval`] 이고, 툴은 계획을 만들어 승인([`DbGrant::approve`])을 받기 전에는 그 증거로 연결을 열 수 없다. 없는 연결은 not found, 틀린 설정은 invalid params. 돌려준 증거는 그 연결 id · 워크트리 ·
/// 대상에만 쓰인다 — 연결을 여는 자리가 그때 설정을 다시 읽어 견준다. 블로킹이다(설정 파일 · 레지스트리) — 블로킹 스레드에서 부른다.
pub fn db_grant_for_agent(
    sessions: &DbSessions,
    agent: &AgentGrant,
    worktree: Option<&str>,
    connection_id: &str,
    database: Option<&str>,
) -> Result<DbGrant, RpcError> {
    check_connection_id(connection_id)?;
    let project_id = agent.project_id();
    sessions.connections.trust_gate(project_id, worktree)?;
    let loaded = agent_view(sessions, project_id, worktree)?;
    if loaded.connections.is_err() {
        return Err(RpcError::internal(UNREADABLE_SETTINGS));
    }
    let connection = loaded
        .get(connection_id)
        .ok_or_else(|| crate::config::not_found(connection_id))?;
    let production = production_policy(sessions, agent.project());
    let mut binding = judge_for_agent(sessions, &loaded, connection, database, production)?;
    binding.worktree = worktree.map(str::to_owned);
    Ok(DbGrant {
        project_id: project_id.to_owned(),
        agent: Some(Box::new(binding)),
    })
}

/// 승인 계획이 해시에 넣는 대상 값(16-m5-plan M5-12 ② — 연결의 푼 대상 · 표지. 방언은 부른 쪽이 따로 넣는다).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbPlanTarget<'a> {
    pub connection_id: &'a str,
    pub worktree: Option<&'a str>,
    pub target: &'a DbTarget,
    pub marks: DbMarks,
}

impl DbGrant {
    /// 프로덕션 대상을 `production = "ask"` 로 지난 에이전트의 증거다 — 툴은 이 호출의 계획을 만들어 승인을 받아야 한다
    /// ([`DbGrant::approve`]). 그 전에는 연결을 여는 자리 · 스키마 캐시가 이 증거를 받지 않는다.
    pub fn needs_approval(&self) -> bool {
        self.agent
            .as_deref()
            .is_some_and(|a| a.approval == AgentApproval::Required)
    }

    /// 계획의 승인([`Approved`] — `pw_core::registry::plan::approve` 밖에서는 만들 수 없다)을 이 증거에 단다.
    pub fn approve(&mut self, approved: &Approved) {
        if let Some(binding) = self.agent.as_deref_mut()
            && binding.approval == AgentApproval::Required
        {
            binding.approval = AgentApproval::Given(approved.hash().to_owned());
        }
    }

    /// 에이전트의 증거가 묶인 연결 이름. 창의 증거면 `None`.
    pub fn agent_connection_name(&self) -> Option<&str> {
        self.agent.as_deref().map(|a| a.name.as_str())
    }

    /// 승인 계획의 대상 값(에이전트의 증거만).
    pub fn plan_target(&self) -> Option<DbPlanTarget<'_>> {
        self.agent.as_deref().map(|a| DbPlanTarget {
            connection_id: &a.connection_id,
            worktree: a.worktree.as_deref(),
            target: &a.target,
            marks: a.marks,
        })
    }

    /// 연결을 여는 자리 · 캐시를 보는 자리에서: 에이전트의 증거면 그 연결 id · 워크트리 · 지금 설정의 대상이 판정한 것과 같아야 한다.
    /// 아니면 `notOpenedByUser`(그사이 설정이 바뀌었다). `marks` 는 그 자리에서 다시 읽은 설정의 표지에 그 대상의 기억을 조인 것이다 —
    /// 판정(과 승인 계획) 뒤에 표지가 더해졌으면 열지 않는다(프로덕션이면 `productionBlocked`, read-only 면 `readOnlyConnection`).
    /// 승인 뒤 · 연결 전에 설정을 바꿔 끼우는 것을 막는 자리다(16-m5-plan M5-18 — 승인 해시는 판정 때의 값이라 이 틈을 덮지 못한다).
    /// 창의 증거는 늘 지난다.
    pub(crate) fn check_agent_use(
        &self,
        worktree: Option<&str>,
        connection_id: Option<&str>,
        target: &DbTarget,
        marks: DbMarks,
    ) -> Result<(), RpcError> {
        let Some(binding) = self.agent.as_deref() else {
            return Ok(());
        };
        let id = connection_id.unwrap_or("(draft)");
        if binding.approval == AgentApproval::Required {
            // 툴이 승인을 지나지 않고 프로덕션 대상을 열려 했다(프로그래밍 오류) — 열지 않는다(fail-closed).
            tracing::error!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: a production target without the call's approval");
            return Err(RpcError::internal(
                "this db call needs approval in Pitwall before it opens a production connection",
            ));
        }
        if connection_id != Some(binding.connection_id.as_str())
            || worktree != binding.worktree.as_deref()
            || *target != binding.target
        {
            tracing::info!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: the connection is not the one the gate judged");
            return Err(not_opened_by_user(&self.project_id, id));
        }
        if marks.production && !binding.marks.production {
            tracing::info!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: marked production after the gate judged it");
            return Err(production_blocked(&self.project_id, id));
        }
        if marks.read_only && !binding.marks.read_only {
            tracing::info!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: marked read-only after the gate judged it");
            return Err(marked_read_only(&self.project_id, id));
        }
        Ok(())
    }
}
