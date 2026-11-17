//! DB 의 MCP 툴 메서드(16-m5-plan.md M5-06 ② ③, `docs/design/05-mcp.md` 「DB」 표). 그룹 `db`, 둘 다 `read`.
//!
//! - `db_connection_list` ← `db.tool.connectionList`: 연결 목록(비밀 없이) — 대상(호스트 · 포트 · DB · SQLite 경로), 에이전트에게
//!   적용하는 read-only · 프로덕션 표지(결정 M5-D37 — 대상에서 본 가장 엄한 것), 신뢰 · 사용 가능 상태, 그리고 **에이전트가 지금 쓸
//!   수 있는가**(사람이 열었는가 · 프로덕션 차단 — 관문 [`db_grant_for_agent`] 과 같은 판정). 미신뢰 프로젝트에서도 목록은 준다.
//! - `db_schema` ← `db.tool.schema`: 관문 [`db_grant_for_agent`] 를 지나 창의 탐색기와 같은 메타데이터용 연결 · 캐시
//!   (`crate::schema`)로 읽는다. `object` 가 없으면 한 단계의 목록(`kind` — DB · 스키마 · 객체 종류, 기본 `table`), 있으면 그 객체의
//!   열(키 포함) · 인덱스 · FK, `detail` 이 `ddl` 이면 DDL 글. SQL 드라이버만 — Redis 연결이면 `unsupported`.
//!
//! - `db_query` ← `db.tool.query`(M5-07): 조회만 도는 SQL 실행 — 입력은 `sql` 또는 `file`. 관문 [`db_grant_for_agent`] 를 지나
//!   실행 파이프라인 [`crate::script`](여러 문장 · `file` 읽기 · 읽기 전용 트랜잭션 · 감사 상세)로 돈다. 조회가 아닌 문장이 하나라도
//!   있으면 아무것도 돌리지 않고 거절한다(쓰기는 `db_execute` — M5-18). SQL 드라이버만.
//!
//! **프로덕션 `ask`**(M5-18 ④): `[mcp.policy] production = "ask"`(사람의 설정)이면 사람이 연 프로덕션 연결을 `db_schema` ·
//! `db_query` 도 **호출마다 승인받아** 쓴다 — 입력을 `Planned` 로 감싸고, 관문의 증거가 승인을 청할 때만 계획을 낸다
//! ([`crate::tool_plan::approve_production_read`]). 그 밖의 호출은 승인 없이 바로 돈다. `db_connection_list` 는 그 연결을
//! `usable` · `asksApproval` 로 보인다.
//!
//! 툴 모두 MCP 세션의 연결 범위만 받는다(`agent_grant` — 창 · Host 범위는 `notAgentSession`). UI 의 RPC(`db.connection.list` ·
//! `db.schema.*`)는 그대로 창 범위만 받는다.

use std::sync::Arc;

use pw_core::registry::agent::agent_grant;
use pw_core::registry::plan::Planned;
use pw_db_driver::budget::within;
use pw_db_driver::{Budget, DbConnection, QueryCancel};
use pw_db_types::{
    CellLimit, ColumnInfo, DbError, DbErrorKind, DriverKind, ForeignKeyInfo, IndexInfo, ObjectKind,
    ObjectQuery, ObjectRef,
};
use pw_rpc::{Ctx, DEFAULT_LIST_LIMIT, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::config::{DbConnectionConfig, DbConnectionState};
use crate::execute::MAX_PAGE_ROWS;
use crate::gate::{DbMarks, DbTarget, agent_view, db_grant_for_agent, production_policy};
use crate::gate::{on_classifier_thread, plan};
use crate::schema::{Cached, Request, check_limit, check_object, check_text, slice, unexpected};
use crate::script::{
    CancelOnDrop, DEFAULT_MAX_ROWS, DbScriptError, DbScriptNotRun, DbScriptNotRunReason,
    DbScriptResult, ResponseRoom, ScriptInput, TOOL_CELL_BYTES, audit_detail, fit_rows,
    read_script_file, refuse_non_queries, run_read_only,
};
use crate::session::{DbSessions, blocking, db_error};
use crate::tool_plan::{approve_production_read, sql_change};

// --- db_connection_list ---------------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolConnectionListParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Connection settings (`.pitwall/db/*.toml`) are read from that worktree.
    /// Empty means the session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
}

/// Whether the connection can be opened now (the same as the window's state — when a secret or a confirmation is
/// needed, an agent cannot answer it).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolConnectionState {
    /// The project is not trusted.
    Untrusted,
    /// The settings file is wrong (`message`).
    Invalid,
    /// A password is needed — the person enters it in Pitwall.
    SecretRequired,
    /// The person's confirmation is needed — the person confirms it in Pitwall.
    ApprovalRequired,
    Ready,
}

/// Why the agent cannot use this connection now (the same as `data.reason` when a DB tool refuses).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolUnusable {
    /// The project is not trusted.
    Untrusted,
    /// The settings file is wrong.
    Invalid,
    /// The target is marked production and `[mcp.policy] production` is `"block"` (the default) — agent tools do not use
    /// it.
    ProductionBlocked,
    /// The person has not opened this connection (with its current target) in Pitwall's DB explorer during this app run
    /// — try again after the person has opened it once, or ask the person to approve opening it for this session with
    /// `db_connection_test`.
    NotOpenedByUser,
}

/// One connection (no secrets).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolConnection {
    /// Connection id (`connectionId` for the other DB tools).
    pub id: String,
    /// `null` if the settings are wrong.
    pub name: Option<String>,
    pub driver: Option<DriverKind>,
    pub host: Option<String>,
    /// Port in use (the driver's default if not set).
    pub port: Option<u16>,
    pub database: Option<String>,
    /// SQLite file (as written in the settings — a relative path is relative to the project root).
    pub path: Option<String>,
    /// Read-only mark as applied to agents (the strictest one seen for this target during this app run).
    pub read_only: bool,
    /// Production mark as applied to agents (same rule). When on, the DB tools do not use the connection — unless the
    /// person set `[mcp.policy] production = "ask"`, and then every call on it asks the person (`asksApproval`).
    pub production: bool,
    pub state: DbToolConnectionState,
    /// Whether the agent DB tools (`db_schema`, etc.) can use this connection now.
    pub usable: bool,
    /// Why it cannot be used (`null` when `usable` is true).
    pub unusable_reason: Option<DbToolUnusable>,
    /// Every call of the DB tools on this connection asks the person in Pitwall first (a production target with
    /// `[mcp.policy] production = "ask"`). There is no "allow for this session" for it.
    pub asks_approval: bool,
    /// If the settings are wrong, a short notice (the values are not included — see the reason in Pitwall).
    pub message: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolConnectionList {
    /// Sorted by id.
    pub connections: Vec<DbToolConnection>,
    pub trusted: bool,
    /// If the project settings could not be read, a short notice (the list is then empty; no values are included).
    pub problem: Option<String>,
}

// --- db_schema ------------------------------------------------------------------------------------------------------

/// What to list. `database` · `schema` list databases · schemas; the rest list objects of that kind.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolSchemaKind {
    Database,
    Schema,
    Table,
    View,
    MaterializedView,
    Sequence,
    Function,
    Procedure,
    Trigger,
}

impl DbToolSchemaKind {
    fn object_kind(self) -> Option<ObjectKind> {
        Some(match self {
            Self::Database | Self::Schema => return None,
            Self::Table => ObjectKind::Table,
            Self::View => ObjectKind::View,
            Self::MaterializedView => ObjectKind::MaterializedView,
            Self::Sequence => ObjectKind::Sequence,
            Self::Function => ObjectKind::Function,
            Self::Procedure => ObjectKind::Procedure,
            Self::Trigger => ObjectKind::Trigger,
        })
    }
}

/// What to read when `object` is given.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolSchemaDetail {
    /// Columns · indexes · foreign keys.
    #[default]
    Columns,
    /// DDL text.
    Ddl,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolSchemaParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Connection settings are read from that worktree. Empty means the session's
    /// worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Connection id from `db_connection_list`.
    pub connection_id: String,
    /// Empty means the connection settings' database.
    #[serde(default)]
    pub database: Option<String>,
    /// Empty means the connection's default schema (PG `current_schema()`). Not used by drivers without schemas (MySQL
    /// · SQLite).
    #[serde(default)]
    pub schema: Option<String>,
    /// Name of a table, view, etc. When given, that object's columns · indexes · foreign keys (or its DDL with `detail:
    /// "ddl"`). In the form `schema.name` the part before the dot is read as the schema (the database for drivers
    /// without schemas) — when `schema` (or `database`) is given separately, the whole name including the dot is the
    /// name.
    #[serde(default)]
    pub object: Option<String>,
    /// What to list when `object` is not given. Defaults to `table`.
    #[serde(default)]
    pub kind: Option<DbToolSchemaKind>,
    /// Object lists: only names containing this text (server side, case-insensitive).
    #[serde(default)]
    pub filter: Option<String>,
    /// `nextCursor` from the previous page. Empty for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Page size (1 – 500, default 100).
    #[serde(default)]
    pub limit: Option<u32>,
    /// When `object` is given: `columns` (default — columns · indexes · foreign keys) or `ddl`.
    #[serde(default)]
    pub detail: Option<DbToolSchemaDetail>,
}

/// What the result is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolSchemaLevel {
    Databases,
    Schemas,
    Objects,
    Object,
    Ddl,
}

/// One item of a list.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolSchemaItem {
    pub name: String,
    /// The kind, for an object list.
    pub kind: Option<ObjectKind>,
    pub comment: Option<String>,
}

/// One column.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolColumn {
    pub name: String,
    /// The DB's type text.
    #[serde(rename = "type")]
    pub db_type: String,
    pub nullable: bool,
    /// Text of the default expression.
    pub default: Option<String>,
    /// Whether the column is part of the primary key.
    pub primary_key: bool,
    pub comment: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolSchema {
    pub level: DbToolSchemaLevel,
    /// The page, for a list (empty when `object` was given).
    pub items: Vec<DbToolSchemaItem>,
    /// The object read, when `object` was given (qualified name).
    pub object: Option<ObjectRef>,
    pub columns: Option<Vec<DbToolColumn>>,
    pub indexes: Option<Vec<IndexInfo>>,
    pub foreign_keys: Option<Vec<ForeignKeyInfo>>,
    pub ddl: Option<String>,
    /// Cursor of the next page (pass it back as `cursor`). Missing means this is the last page.
    pub next_cursor: Option<String>,
}

// --- db_query -------------------------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolQueryParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Connection settings and `file` are read from that worktree. Empty means the
    /// session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Connection id from `db_connection_list`.
    pub connection_id: String,
    /// SQL to run (several statements — split by the dialect's delimiter). Cannot be given together with `file`.
    #[serde(default)]
    pub sql: Option<String>,
    /// SQL file — a path relative to the project root (inside the chosen worktree, if one is given). A UTF-8 regular
    /// file inside the root, up to 5 MiB. Cannot be given together with `sql`.
    #[serde(default)]
    pub file: Option<String>,
    /// Maximum rows returned per statement (1 – 10000, default 100). Not paged — past it the statement is `truncated`.
    #[serde(default)]
    pub max_rows: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbToolQuery {
    /// Number of statements after splitting.
    pub statements: u32,
    /// One result per statement that ran (in order). The failed statement and the ones after it are not included.
    pub results: Vec<DbScriptResult>,
    /// If a statement failed, that statement and its error. The statements after it did not run.
    pub error: Option<DbScriptError>,
    /// Statements not run because the response budget was used up (`count` statements from `index` to the end, `reason:
    /// "responseBudget"`). The statement before them is `truncated`. Narrow the query (fewer rows · columns) and call
    /// again, or call those statements on their own.
    pub not_run: Option<DbScriptNotRun>,
    /// The file read, when `file` was given (path relative to the root).
    pub file: Option<String>,
    /// Details kept in the audit log — statement count · highest grade · statement hashes (for `file`, its path and
    /// content hash). No SQL text or values.
    pub audit: String,
}

rpc_method! {
    /// This project's DB connections (no secrets): id · name · driver · target (host · port · database · SQLite path) ·
    /// read-only · production · state, plus whether the agent DB tools can use it now (`usable`) and why not. The agent
    /// DB tools use only connections the person opened in Pitwall's DB explorer during this app run (`notOpenedByUser`)
    /// and do not use connections marked production (`productionBlocked`) unless the person allowed it with approval for
    /// every call (`asksApproval`). Does not open any connection.
    pub DbToolConnectionListMethod: "db.tool.connectionList" {
        params: DbToolConnectionListParams,
        result: DbToolConnectionList,
        tier: Read,
        roles: [Owner],
        mcp: { group: "db", name: "db_connection_list" },
    }

    /// Read a DB schema (SQL drivers only — PG · MySQL/MariaDB · SQLite). Without `object`, one page of one level's
    /// list (`kind`: `database` · `schema` · `table` (default) · `view`, etc. — object lists are within `database` ·
    /// `schema` and filtered by `filter`); with it, that object's columns (name · type · null · default · primary key)
    /// · indexes · foreign keys, or its DDL text with `detail: "ddl"`. Sees the same as Pitwall's DB explorer. Uses
    /// only connections the person opened in Pitwall's DB explorer during this app run, or that this session was
    /// approved to open through `db_connection_test` (otherwise `notOpenedByUser`; it never asks by itself) and does
    /// not use connections marked production
    /// (`productionBlocked`) — unless the person set them to ask, and then each call waits for the person's approval in
    /// Pitwall. For a Redis connection, `unsupported` — use `db_kv_scan`.
    pub DbToolSchemaMethod: "db.tool.schema" {
        params: Planned<DbToolSchemaParams>,
        result: DbToolSchema,
        tier: Read,
        roles: [Owner],
        mcp: { group: "db", name: "db_schema" },
    }

    /// Run read-only SQL (SQL drivers only — PG · MySQL/MariaDB · SQLite). The input is either `sql` (text) or `file`
    /// (an SQL file path relative to the project root — Pitwall reads it when it runs). The statements are split by the
    /// dialect and run in order in one read-only transaction, with one result per statement (columns · rows ·
    /// `truncated` · duration). **Only queries run** — if any statement is not a query (a write · DDL · `SET` ·
    /// transaction control · `ATTACH` · text that cannot be parsed), nothing runs and the call is refused (`notAQuery`
    /// — writes go through `db_execute`). If a statement fails, it stops there and returns the results so far with
    /// `error` (statement index · line). Rows are limited to `maxRows` per statement (default 100), cells are cut at 4
    /// KiB (`truncated` values). The whole response is limited to about 192 KiB — when the rows fill it, that statement
    /// is `truncated` and the later statements do not run and are reported as `notRun` (`count` from `index`, `reason:
    /// "responseBudget"`). **There is no next page** — if a result was cut, narrow the WHERE · LIMIT · columns and call
    /// again (a new call sees a different snapshot).
    /// Uses only connections the person opened in Pitwall's DB explorer during this app run, or that this session was
    /// approved to open through `db_connection_test` (`notOpenedByUser`), and does not use a target marked production — for MySQL · MariaDB, any database on the same server · user
    /// (`productionBlocked`) — unless the person set production targets to ask, and then each call shows its statements
    /// to the person and waits for approval in Pitwall. The result's `audit` is the detail kept in the audit log
    /// (statement count · grade · hashes) and is shown to the agent too.
    pub DbToolQueryMethod: "db.tool.query" {
        params: Planned<DbToolQueryParams>,
        result: DbToolQuery,
        tier: Read,
        roles: [Owner],
        mcp: { group: "db", name: "db_query" },
    }
}

impl DbSessions {
    /// `db_connection_list`(블로킹).
    fn tool_connection_list(
        &self,
        scope: &pw_rpc::ConnectionScope,
        session: pw_rpc::SessionId,
        p: &DbToolConnectionListParams,
    ) -> Result<DbToolConnectionList, RpcError> {
        let c = &self.connections;
        let agent = agent_grant(scope, &c.host()?.projects, &p.project_id)?;
        let loaded = agent_view(self, agent.project_id(), p.worktree.as_deref())?;
        let trusted = c.trusted(agent.project_id());
        let production = production_policy(self, agent.project());
        let approvals = if trusted {
            Some(c.approvals(agent.project_id())?.0)
        } else {
            None
        };
        let list = match &loaded.connections {
            Ok(list) => list,
            Err(_) => {
                return Ok(DbToolConnectionList {
                    connections: Vec::new(),
                    trusted,
                    problem: Some(crate::gate::UNREADABLE_SETTINGS.to_owned()),
                });
            }
        };
        let connections = list
            .iter()
            .map(|connection| {
                let entry = c.entry(&loaded, connection, approvals.as_ref());
                let (state, message) = match &entry.state {
                    DbConnectionState::Untrusted => (DbToolConnectionState::Untrusted, None),
                    // 까닭 글(serde 오류)은 값을 싣는다 — 툴 응답 · 감사 로그로 내보내지 않는다.
                    DbConnectionState::Invalid { .. } => (
                        DbToolConnectionState::Invalid,
                        Some(crate::gate::invalid_settings(&connection.id)),
                    ),
                    DbConnectionState::SecretRequired { .. } => {
                        (DbToolConnectionState::SecretRequired, None)
                    }
                    DbConnectionState::ApprovalRequired { .. } => {
                        (DbToolConnectionState::ApprovalRequired, None)
                    }
                    DbConnectionState::Ready => (DbToolConnectionState::Ready, None),
                };
                let config: Option<&DbConnectionConfig> = connection.config.as_ref().ok();
                let marks = config.map_or(DbMarks::default(), |config| {
                    DbMarks::of(config).tighten(c.marks.get(&DbTarget::of(config, &loaded.root)))
                });
                let (unusable, asks_approval) = if !trusted {
                    (Some(DbToolUnusable::Untrusted), false)
                } else if config.is_none() {
                    (Some(DbToolUnusable::Invalid), false)
                } else {
                    let asker = crate::gate::Asker {
                        session: Some(session),
                        may_ask_to_open: false,
                    };
                    match crate::gate::judge_for_agent(
                        self, &loaded, connection, None, production, asker,
                    ) {
                        Ok(binding) => (None, binding.asks_approval()),
                        Err(e) => (Some(unusable_of(&e)), false),
                    }
                };
                DbToolConnection {
                    id: connection.id.clone(),
                    name: config.map(|c| c.name.clone()),
                    driver: config.map(|c| c.driver),
                    host: config.and_then(|c| c.host.clone()),
                    port: config.and_then(DbConnectionConfig::effective_port),
                    database: config.and_then(|c| c.database.clone()),
                    path: config.and_then(|c| c.path.clone()),
                    read_only: marks.read_only,
                    production: marks.production,
                    state,
                    usable: unusable.is_none(),
                    unusable_reason: unusable,
                    asks_approval,
                    message,
                }
            })
            .collect();
        Ok(DbToolConnectionList {
            connections,
            trusted,
            problem: None,
        })
    }

    /// `db_schema`.
    async fn tool_schema(
        self: &Arc<Self>,
        ctx: &Ctx,
        planned: Planned<DbToolSchemaParams>,
    ) -> Result<DbToolSchema, RpcError> {
        let p = &planned.params;
        for (what, value) in [
            ("worktree", &p.worktree),
            ("database", &p.database),
            ("schema", &p.schema),
            ("object", &p.object),
            ("filter", &p.filter),
            ("cursor", &p.cursor),
        ] {
            check_text(what, value.as_deref())?;
        }
        let limit = check_limit(p.limit.unwrap_or(DEFAULT_LIST_LIMIT))?;
        // 이 호출이 실제로 읽을 DB 마다 관문을 지난다(메인 판정 — 객체 목록 · `database` 입력, 그리고 `object` 의 점 이름에서
        // 나온 DB). 표지는 그 DB 의 대상에서도 본다.
        // 지금 드라이버로는 두 번째 관문이 첫 관문과 같은 판정이다 — 점 이름으로 DB 를 고르는 드라이버는 MySQL · MariaDB 뿐이고 그
        // 표지 열쇠는 DB 이름을 뺀다. DB 를 열쇠에 두면서 점 이름으로 DB 를 고르는 드라이버가 생기면 그때 갈린다(이중 방어).
        let gate = |database: Option<String>| {
            let (s, scope, session, project, worktree, id) = (
                Arc::clone(self),
                ctx.scope().clone(),
                ctx.session(),
                p.project_id.clone(),
                p.worktree.clone(),
                p.connection_id.clone(),
            );
            blocking(move || {
                let agent = agent_grant(&scope, &s.connections.host()?.projects, &project)?;
                db_grant_for_agent(
                    &s,
                    &agent,
                    session,
                    worktree.as_deref(),
                    &id,
                    database.as_deref(),
                )
            })
        };
        let first = gate(p.database.clone()).await?;
        let driver = first
            .agent_driver()
            .and_then(|kind| self.drivers.get(kind.id()))
            .ok_or_else(|| {
                db_error(DbError::new(
                    DbErrorKind::Unsupported,
                    "the db driver of this connection is not installed",
                ))
            })?;
        let caps = driver.capabilities();
        if !caps.sql {
            return Err(db_error(DbError::new(
                DbErrorKind::Unsupported,
                format!(
                    "db_schema reads SQL databases only; `{}` is a key-value store — use db_kv_scan and db_kv_get",
                    p.connection_id
                ),
            )));
        }
        let object = p.object.as_deref().map(|object| {
            object_ref(
                object,
                p.database.clone(),
                p.schema.clone(),
                caps.schemas,
                caps.multiple_databases,
            )
        });
        let mut grant = match object.as_ref().and_then(|o| o.database.clone()) {
            Some(database) if Some(&database) != p.database.as_ref() => {
                gate(Some(database)).await?
            }
            _ => first,
        };
        // 프로덕션 `ask`: 메타데이터용 연결 · 캐시를 보기 전에 이 호출의 승인을 받는다.
        approve_production_read(
            &mut grant,
            "db.tool.schema",
            p,
            planned.ticket.as_ref(),
            driver.dialect(),
            &schema_what(p),
            None,
        )?;
        let worktree = p.worktree.as_deref();
        let meta = |request| self.meta(ctx, &grant, worktree, &p.connection_id, request);
        let Some(object) = object else {
            let kind = p.kind.unwrap_or(DbToolSchemaKind::Table);
            return match kind.object_kind() {
                None if kind == DbToolSchemaKind::Database => {
                    let Cached::Databases(all) = meta(Request::Databases).await? else {
                        return Err(unexpected());
                    };
                    let (page, next_cursor) = slice(&all, p.cursor.as_deref(), Some(limit))?;
                    Ok(list(
                        DbToolSchemaLevel::Databases,
                        page.into_iter().map(|d| named(d.name)).collect(),
                        next_cursor,
                    ))
                }
                None => {
                    let Cached::Schemas(all) = meta(Request::Schemas(p.database.clone())).await?
                    else {
                        return Err(unexpected());
                    };
                    let (page, next_cursor) = slice(&all, p.cursor.as_deref(), Some(limit))?;
                    Ok(list(
                        DbToolSchemaLevel::Schemas,
                        page.into_iter().map(|s| named(s.name)).collect(),
                        next_cursor,
                    ))
                }
                Some(kind) => {
                    let Cached::Objects(page) = meta(Request::Objects(ObjectQuery {
                        database: p.database.clone(),
                        schema: p.schema.clone(),
                        kind,
                        filter: p.filter.clone(),
                        cursor: p.cursor.clone(),
                        limit,
                    }))
                    .await?
                    else {
                        return Err(unexpected());
                    };
                    Ok(list(
                        DbToolSchemaLevel::Objects,
                        page.items
                            .into_iter()
                            .map(|o| DbToolSchemaItem {
                                name: o.name,
                                kind: Some(o.kind),
                                comment: o.comment,
                            })
                            .collect(),
                        page.next_cursor,
                    ))
                }
            };
        };
        check_object(&object)?;
        let mut result = list(DbToolSchemaLevel::Object, Vec::new(), None);
        result.object = Some(object.clone());
        if p.detail.unwrap_or_default() == DbToolSchemaDetail::Ddl {
            let Cached::Ddl(ddl) = meta(Request::Ddl(object)).await? else {
                return Err(unexpected());
            };
            result.level = DbToolSchemaLevel::Ddl;
            result.ddl = Some(ddl.text);
            return Ok(result);
        }
        let Cached::Columns(columns) = meta(Request::Columns(object.clone())).await? else {
            return Err(unexpected());
        };
        let Cached::Indexes(indexes) = meta(Request::Indexes(object.clone())).await? else {
            return Err(unexpected());
        };
        let Cached::ForeignKeys(foreign_keys) = meta(Request::ForeignKeys(object)).await? else {
            return Err(unexpected());
        };
        result.columns = Some(columns_with_keys(columns, &indexes));
        result.indexes = Some(indexes);
        result.foreign_keys = Some(foreign_keys);
        Ok(result)
    }
}

impl DbSessions {
    /// `db_query`.
    async fn tool_query(
        self: &Arc<Self>,
        ctx: &Ctx,
        planned_input: Planned<DbToolQueryParams>,
    ) -> Result<DbToolQuery, RpcError> {
        let p = &planned_input.params;
        check_text("worktree", p.worktree.as_deref())?;
        let input = ScriptInput::of(p.sql.clone(), p.file.clone())?;
        let max_rows = p.max_rows.unwrap_or(DEFAULT_MAX_ROWS);
        if max_rows == 0 || max_rows > MAX_PAGE_ROWS {
            return Err(RpcError::invalid_params(format!(
                "maxRows must be 1-{MAX_PAGE_ROWS}"
            )));
        }
        // 관문(M5-06 의 입구)을 먼저 — 거절할 호출이면 파일도 읽지 않는다.
        let (s, scope, project, worktree, id) = (
            Arc::clone(self),
            ctx.scope().clone(),
            p.project_id.clone(),
            p.worktree.clone(),
            p.connection_id.clone(),
        );
        let session = ctx.session();
        let mut grant = blocking(move || {
            let agent = agent_grant(&scope, &s.connections.host()?.projects, &project)?;
            db_grant_for_agent(&s, &agent, session, worktree.as_deref(), &id, None)
        })
        .await?;
        let driver = grant
            .agent_driver()
            .and_then(|kind| self.drivers.get(kind.id()))
            .ok_or_else(|| {
                db_error(DbError::new(
                    DbErrorKind::Unsupported,
                    "the db driver of this connection is not installed",
                ))
            })?;
        let dialect = match driver.dialect() {
            Some(dialect) if driver.capabilities().sql => dialect,
            _ => {
                return Err(db_error(DbError::new(
                    DbErrorKind::Unsupported,
                    format!(
                        "db_query runs SQL databases only; `{}` is a key-value store — use db_kv_scan and db_kv_get",
                        p.connection_id
                    ),
                )));
            }
        };
        // `file` 은 지금 한 번 읽고, 읽은 그 글을 분류하고 그 글을 돌린다 — 파일을 다시 열지 않는다.
        let (source, file) = match input {
            ScriptInput::Sql(sql) => (sql, None),
            ScriptInput::File(path) => {
                let (s, project, worktree) =
                    (Arc::clone(self), p.project_id.clone(), p.worktree.clone());
                let file = blocking(move || {
                    read_script_file(
                        &s.connections.host()?.projects,
                        &project,
                        worktree.as_deref(),
                        &path,
                    )
                })
                .await?;
                (file.text.clone(), Some(file))
            }
        };
        let text = source.clone();
        let planned = on_classifier_thread(move || plan(dialect, &text, None)).await??;
        let audit = audit_detail(dialect, &planned, file.as_ref());
        // 연결을 열기 전에 거절한다(비밀도 읽지 않는다).
        refuse_non_queries("db_query", &source, &planned, &audit)?;
        let with_audit = |e: RpcError| with_audit(e, &audit);
        // 프로덕션 `ask`: 연결을 열기 전에 이 호출의 문장들을 보이고 승인을 받는다(돌릴 것은 지금 읽은 그 글이다).
        let change = grant
            .needs_approval()
            .then(|| sql_change(&grant, &source, &planned, file.as_ref(), false));
        approve_production_read(
            &mut grant,
            "db.tool.query",
            p,
            planned_input.ticket.as_ref(),
            Some(dialect),
            "with SQL",
            change,
        )
        .map_err(with_audit)?;
        let opened =
            crate::connection::connect(self, &grant, p.worktree.as_deref(), &p.connection_id, None)
                .await
                .map_err(with_audit)?;
        let tunnel = opened.tunnel;
        let mut conn = match opened.connection {
            DbConnection::Sql(conn) => conn,
            other => {
                let _ = other.close().await;
                return Err(with_audit(RpcError::invalid_params(
                    "this connection does not run SQL",
                )));
            }
        };
        if !opened.driver.capabilities.read_only_transaction {
            let _ = conn.close().await;
            return Err(with_audit(db_error(DbError::new(
                DbErrorKind::Unsupported,
                "this driver has no read-only transactions, so db_query cannot run on it",
            ))));
        }
        let cell = CellLimit(TOOL_CELL_BYTES.min(self.cell_limit().0));
        let file_path = file.map(|f| f.path);
        // 응답 예산(결정 M5-D34 · 리뷰 B2): 행을 읽는 그때 센다. 고정 몫은 결과의 틀(파일 경로 · 감사 상세 · 「돌리지 않음」 표지)과
        // 실패한 문장의 오류 몫이다.
        let mut room = ResponseRoom::new(
            serde_json::to_string(&DbToolQuery {
                statements: u32::MAX,
                results: Vec::new(),
                error: None,
                not_run: Some(DbScriptNotRun {
                    index: u32::MAX,
                    line: u32::MAX,
                    count: u32::MAX,
                    reason: DbScriptNotRunReason::ResponseBudget,
                }),
                file: file_path.clone(),
                audit: audit.clone(),
            })
            .map_or(0, |s| s.len())
                + ERROR_ROOM,
        );
        let cancel = QueryCancel::new(conn.interrupter(), Budget::PAGE.cancel_cleanup);
        // 실행은 연결을 쥔 태스크에서 돈다 — 호출이 버려져도(취소 · 세션 끊김) 태스크가 드라이버 호출을 버리고 롤백한 뒤 닫는다.
        let guard = CancelOnDrop::new(cancel.clone());
        let run = {
            let (source, planned) = (source.clone(), planned.clone());
            tokio::spawn(async move {
                // 터널은 연결을 닫을 때까지 쥔다.
                let _tunnel = tunnel;
                let ran = run_read_only(
                    conn.as_mut(),
                    &source,
                    &planned,
                    max_rows,
                    cell,
                    &cancel,
                    &mut room,
                )
                .await;
                let _ = within(Budget::PAGE.cancel_cleanup, "rollback", conn.rollback()).await;
                let _ = within(Budget::PAGE.connect, "close", conn.close()).await;
                ran
            })
        };
        let ran = run
            .await
            .map_err(|e| with_audit(RpcError::internal(e.to_string())))?;
        guard.disarm();
        let ran = ran.map_err(|e| with_audit(db_error(e)))?;
        let mut result = DbToolQuery {
            statements: u32::try_from(planned.len()).unwrap_or(u32::MAX),
            results: ran.results,
            error: ran.error,
            not_run: ran.not_run,
            file: file_path,
            audit,
        };
        // 겹방어: 실행 중의 셈이 오류 몫을 넘는 오류 글 등으로 어긋났으면 행을 마저 던다.
        let rows: Vec<Vec<pw_db_types::Row>> = result
            .results
            .iter_mut()
            .map(|r| std::mem::take(&mut r.rows))
            .collect();
        let fixed = serde_json::to_string(&result).map_or(0, |s| s.len());
        for (r, rows) in result.results.iter_mut().zip(rows) {
            r.rows = rows;
        }
        fit_rows(&mut result.results, fixed);
        Ok(result)
    }
}

/// 실패한 문장의 오류(서버 글 · 범위)에 남겨 두는 응답 예산의 몫.
pub(crate) const ERROR_ROOM: usize = 8 * 1024;

/// 오류 `data` 의 최상위 `audit` 에 감사 상세를 싣는다(`crates/pw-mcp` 머리 주석 「툴 메서드 규약」 — 세션이 감사 행으로 옮긴다).
/// `data` 가 object 가 아니면(글 등) `{ message, audit }` 로 감싼다.
pub(crate) fn with_audit(mut error: RpcError, audit: &str) -> RpcError {
    let data = match error.data.take() {
        Some(serde_json::Value::Object(mut map)) => {
            map.insert("audit".into(), audit.into());
            serde_json::Value::Object(map)
        }
        Some(other) => serde_json::json!({ "message": other, "audit": audit }),
        None => serde_json::json!({ "audit": audit }),
    };
    error.data = Some(data);
    error
}

/// 프로덕션 `ask` 의 승인 요청에 보이는 「무엇을 읽는가」.
fn schema_what(p: &DbToolSchemaParams) -> String {
    let mut what = match (&p.object, p.detail.unwrap_or_default()) {
        (Some(object), DbToolSchemaDetail::Ddl) => format!("the DDL of `{object}`"),
        (Some(object), DbToolSchemaDetail::Columns) => format!("the columns of `{object}`"),
        (None, _) => format!(
            "the schema list ({})",
            match p.kind.unwrap_or(DbToolSchemaKind::Table) {
                DbToolSchemaKind::Database => "databases",
                DbToolSchemaKind::Schema => "schemas",
                _ => "objects",
            }
        ),
    };
    if let Some(database) = &p.database {
        what.push_str(&format!(" in database `{database}`"));
    }
    what
}

/// 거절 오류의 `data.reason` → 까닭.
fn unusable_of(error: &RpcError) -> DbToolUnusable {
    let reason = error
        .data
        .as_ref()
        .and_then(|d| d.get("reason"))
        .and_then(serde_json::Value::as_str);
    match reason {
        Some(crate::gate::PRODUCTION_BLOCKED) => DbToolUnusable::ProductionBlocked,
        Some(crate::gate::NOT_OPENED_BY_USER) => DbToolUnusable::NotOpenedByUser,
        Some("untrusted") => DbToolUnusable::Untrusted,
        _ => DbToolUnusable::Invalid,
    }
}

fn named(name: String) -> DbToolSchemaItem {
    DbToolSchemaItem {
        name,
        kind: None,
        comment: None,
    }
}

fn list(
    level: DbToolSchemaLevel,
    items: Vec<DbToolSchemaItem>,
    next_cursor: Option<String>,
) -> DbToolSchema {
    DbToolSchema {
        level,
        items,
        object: None,
        columns: None,
        indexes: None,
        foreign_keys: None,
        ddl: None,
        next_cursor,
    }
}

/// `object` 입력을 한정한 이름으로. `schema.name` 꼴이고 스키마(스키마가 없는 드라이버는 DB)를 따로 주지 않았으면 점 앞을 그것으로
/// 읽는다.
fn object_ref(
    object: &str,
    database: Option<String>,
    schema: Option<String>,
    schemas: bool,
    databases: bool,
) -> ObjectRef {
    let split = || {
        object
            .split_once('.')
            .filter(|(head, tail)| !head.is_empty() && !tail.is_empty())
    };
    if schemas {
        if schema.is_none()
            && let Some((schema, name)) = split()
        {
            return ObjectRef {
                database,
                schema: Some(schema.to_owned()),
                name: name.to_owned(),
            };
        }
        return ObjectRef {
            database,
            schema,
            name: object.to_owned(),
        };
    }
    if databases
        && database.is_none()
        && let Some((database, name)) = split()
    {
        return ObjectRef {
            database: Some(database.to_owned()),
            schema: None,
            name: name.to_owned(),
        };
    }
    ObjectRef {
        database,
        schema: None,
        name: object.to_owned(),
    }
}

/// 열에 기본 키 표시를 단다.
fn columns_with_keys(columns: Vec<ColumnInfo>, indexes: &[IndexInfo]) -> Vec<DbToolColumn> {
    let primary: Vec<&String> = indexes
        .iter()
        .filter(|i| i.primary)
        .flat_map(|i| &i.columns)
        .collect();
    columns
        .into_iter()
        .map(|c| DbToolColumn {
            primary_key: primary.contains(&&c.name),
            name: c.name,
            db_type: c.db_type,
            nullable: c.nullable,
            default: c.default,
            comment: c.comment,
        })
        .collect()
}

/// 툴 메서드를 붙인다(`crate::session::register` 가 부른다).
pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    let list_sessions = Arc::clone(&sessions);
    router
        .unary(
            DbToolConnectionListMethod,
            move |ctx, p: DbToolConnectionListParams| {
                let s = Arc::clone(&list_sessions);
                async move {
                    let (scope, session) = (ctx.scope().clone(), ctx.session());
                    blocking(move || s.tool_connection_list(&scope, session, &p)).await
                }
            },
        )
        .unary(DbToolSchemaMethod, {
            let sessions = Arc::clone(&sessions);
            move |ctx, p: Planned<DbToolSchemaParams>| {
                let s = Arc::clone(&sessions);
                async move { s.tool_schema(&ctx, p).await }
            }
        })
        .unary(
            DbToolQueryMethod,
            move |ctx, p: Planned<DbToolQueryParams>| {
                let s = Arc::clone(&sessions);
                async move { s.tool_query(&ctx, p).await }
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_names_split_at_the_first_dot_unless_the_qualifier_is_given() {
        let pg = |o: &str, schema: Option<&str>| {
            object_ref(o, None, schema.map(str::to_owned), true, true)
        };
        assert_eq!(pg("public.users", None).schema.as_deref(), Some("public"));
        assert_eq!(pg("public.users", None).name, "users");
        assert_eq!(pg("users", None).schema, None);
        assert_eq!(pg("a.b", Some("s")).name, "a.b");
        let mysql = object_ref("app.users", None, None, false, true);
        assert_eq!(
            (mysql.database.as_deref(), mysql.name.as_str()),
            (Some("app"), "users")
        );
        let sqlite = object_ref("a.b", None, None, false, false);
        assert_eq!((sqlite.database, sqlite.name.as_str()), (None, "a.b"));
        assert_eq!(pg(".x", None).name, ".x");
    }
}
