//! 에이전트의 DB 입구(16-m5-plan.md M5-06 ①, 결정 M5-D2 · M5-D27 · M5-D37). 판정 순서와 두 기억(「사람이 연 연결」 · 대상에 묶은
//! 표지)은 상위 모듈 머리 주석의 「에이전트의 입구」.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pw_core::registry::agent::AgentGrant;
use pw_core::registry::path::simplify;
use pw_db_types::DriverKind;
use pw_rpc::{RpcError, code as rpc_code};
use serde_json::json;

use super::DbGrant;
use crate::config::{DbConnectionConfig, Loaded, LoadedConnection, check_connection_id, lock};
use crate::session::DbSessions;

/// `data.reason`: 사람이 이번 앱 실행에서 그 프로젝트 창으로 (그 연결 id, 지금 대상)을 연 적이 없다.
pub const NOT_OPENED_BY_USER: &str = "notOpenedByUser";
/// `data.reason`: 프로덕션 표지가 있는 대상이다(`[mcp.policy] production` 의 `block` — `ask` 갈래는 M5-18).
pub const PRODUCTION_BLOCKED: &str = "productionBlocked";

/// 푼 대상 — 「사람이 연 연결」 과 표지의 열쇠. 드라이버 · 호스트 · 포트(적지 않았으면 드라이버 기본) · DB · 사용자 · 터널 목적지,
/// SQLite 면 파일의 실제 경로(링크를 끝까지 푼 것 — 워크트리마다 상대 경로가 다른 파일을 가리킨다). 호스트는 소문자로 맞춘다
/// (DNS 이름은 대소문자를 가리지 않는다 — `DB.example.com` 으로 표지를 비껴가지 못하게). TLS 설정은 넣지 않는다 — 같은 서버 · DB 를
/// `sslmode` 만 바꿔 가리켜 표지를 비껴가지 못하게(TLS 의 확인은 `crate::trust` 가 따로 본다).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

/// 에이전트에게 적용하는 표지(결정 M5-D37). 조여지기만 한다([`DbMarks::tighten`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
            .map(|config| (DbTarget::of(config, root), DbMarks::of(config)))
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
        lock(&self.0).get(target).copied().unwrap_or_default()
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
    pub(super) worktree: Option<String>,
    pub(super) target: DbTarget,
    pub(super) marks: DbMarks,
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

/// 에이전트의 DB 툴 호출이 읽는 연결 설정: 그 프로젝트 메인 체크아웃의 것(표지를 더하려고 — 결정 M5-D37)과 호출의 워크트리의 것.
/// 돌려주는 것은 호출의 워크트리의 것이다. 메인 체크아웃의 설정을 읽지 못하면(깨진 파일 등) 그 표지는 더하지 못하고 넘어간다 —
/// 앞서 이번 실행에서 읽힌 표지는 기억에 남아 있다. 블로킹이다.
pub(crate) fn agent_view(
    sessions: &DbSessions,
    project_id: &str,
    worktree: Option<&str>,
) -> Result<Loaded, RpcError> {
    let connections = &sessions.connections;
    if worktree.is_some()
        && let Err(e) = connections.load(project_id, None)
    {
        tracing::info!(target: "pw_db", project = project_id, %e, "cannot read the main checkout's db connections for agent marks");
    }
    connections.load(project_id, worktree)
}

/// 연결 하나에 대한 에이전트의 판정(신뢰 게이트는 부른 쪽이 지났다). `database` 는 호출이 설정의 DB 대신 고른 DB — 그 대상의
/// 표지도 본다.
pub(crate) fn judge_for_agent(
    sessions: &DbSessions,
    loaded: &Loaded,
    connection: &LoadedConnection,
    database: Option<&str>,
) -> Result<AgentBinding, RpcError> {
    let project_id = loaded.project.id.as_str();
    let config = connection.config.as_ref().map_err(|e| {
        RpcError::invalid_params(format!("db connection `{}` is invalid: {e}", connection.id))
    })?;
    let target = DbTarget::of(config, &loaded.root);
    let marks_memory = &sessions.connections.marks;
    let mut marks = DbMarks::of(config).tighten(marks_memory.get(&target));
    if database.is_some_and(|db| config.database.as_deref() != Some(db)) {
        marks = marks.tighten(marks_memory.get(&target.with_database(database)));
    }
    if marks.production {
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
        worktree: None,
        target,
        marks,
    })
}

/// MCP 세션의 DB 입구(가칭 `db_grant_for_agent` 그대로). `agent` 는 `pw_core::registry::agent::agent_grant` 를 지난 증거,
/// `worktree` 는 호출의 활성 워크트리 꼭대기(없으면 메인 체크아웃), `database` 는 호출이 설정의 DB 대신 고를 DB(없으면 설정의 것).
///
/// 신뢰 게이트(`untrusted`) → 연결 설정 읽기(메인 체크아웃 · 그 워크트리 — 표지를 더한다) → 프로덕션 표지(`productionBlocked`) →
/// 「사람이 연 연결」(`notOpenedByUser`). 없는 연결은 not found, 틀린 설정은 invalid params. 돌려준 증거는 그 연결 id · 워크트리 ·
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
    if let Err(problem) = &loaded.connections {
        return Err(RpcError::internal(format!(
            "cannot read the project config: {problem}"
        )));
    }
    let connection = loaded
        .get(connection_id)
        .ok_or_else(|| crate::config::not_found(connection_id))?;
    let mut binding = judge_for_agent(sessions, &loaded, connection, database)?;
    binding.worktree = worktree.map(str::to_owned);
    Ok(DbGrant {
        project_id: project_id.to_owned(),
        agent: Some(Box::new(binding)),
    })
}

impl DbGrant {
    /// 연결을 여는 자리 · 캐시를 보는 자리에서: 에이전트의 증거면 그 연결 id · 워크트리 · 지금 설정의 대상이 판정한 것과 같아야 한다.
    /// 아니면 `notOpenedByUser`(그사이 설정이 바뀌었다). 창의 증거는 늘 지난다.
    pub(crate) fn check_agent_use(
        &self,
        worktree: Option<&str>,
        connection_id: Option<&str>,
        target: &DbTarget,
    ) -> Result<(), RpcError> {
        let Some(binding) = self.agent.as_deref() else {
            return Ok(());
        };
        let id = connection_id.unwrap_or("(draft)");
        if connection_id != Some(binding.connection_id.as_str())
            || worktree != binding.worktree.as_deref()
            || *target != binding.target
        {
            tracing::info!(target: "pw_db", project = %self.project_id, connection = id, "agent db access refused: the connection is not the one the gate judged");
            return Err(not_opened_by_user(&self.project_id, id));
        }
        Ok(())
    }
}
