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
//! 툴 모두 MCP 세션의 연결 범위만 받는다(`agent_grant` — 창 · Host 범위는 `notAgentSession`). UI 의 RPC(`db.connection.list` ·
//! `db.schema.*`)는 그대로 창 범위만 받는다.

use std::sync::Arc;

use pw_core::registry::agent::agent_grant;
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
use crate::gate::{DbMarks, DbTarget, agent_view, db_grant_for_agent};
use crate::gate::{on_classifier_thread, plan};
use crate::schema::{Cached, Request, check_limit, check_object, check_text, slice, unexpected};
use crate::script::{
    CancelOnDrop, DEFAULT_MAX_ROWS, DbScriptError, DbScriptResult, ScriptInput, TOOL_CELL_BYTES,
    audit_detail, fit_rows, read_script_file, refuse_non_queries, run_read_only,
};
use crate::session::{DbSessions, blocking, db_error};

// --- db_connection_list ---------------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolConnectionListParams {
    pub project_id: String,
    /// 워크트리 꼭대기(절대 경로). 연결 설정(`.pitwall/db/*.toml`)을 그 워크트리에서 읽는다. 비우면 세션의 워크트리, 없으면 메인
    /// 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
}

/// 연결을 지금 열 수 있는가(창의 상태와 같다 — 비밀 · 확인이 필요하면 에이전트는 답할 수 없다).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolConnectionState {
    /// 프로젝트를 신뢰하지 않았다.
    Untrusted,
    /// 설정 파일이 틀렸다(`message`).
    Invalid,
    /// 비밀번호가 필요하다 — 사람이 Pitwall 에서 넣는다.
    SecretRequired,
    /// 사람의 확인이 필요하다 — 사람이 Pitwall 에서 확인한다.
    ApprovalRequired,
    Ready,
}

/// 에이전트가 이 연결을 지금 쓸 수 없는 까닭(DB 툴이 거절할 때의 `data.reason` 과 같다).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolUnusable {
    /// 프로젝트를 신뢰하지 않았다.
    Untrusted,
    /// 설정 파일이 틀렸다.
    Invalid,
    /// 프로덕션 표지가 있는 대상이다 — 에이전트 툴은 쓰지 않는다.
    ProductionBlocked,
    /// 사람이 이번 앱 실행에서 Pitwall 의 DB 탐색기로 이 연결(지금 대상)을 연 적이 없다 — 사람이 한 번 연 뒤 다시.
    NotOpenedByUser,
}

/// 연결 하나(비밀 없음).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolConnection {
    /// 연결 id(다른 DB 툴의 `connectionId`).
    pub id: String,
    /// 설정이 틀렸으면 `null`.
    pub name: Option<String>,
    pub driver: Option<DriverKind>,
    pub host: Option<String>,
    /// 쓰이는 포트(적지 않았으면 드라이버 기본).
    pub port: Option<u16>,
    pub database: Option<String>,
    /// SQLite 파일(설정에 적힌 그대로 — 상대 경로면 프로젝트 루트 기준).
    pub path: Option<String>,
    /// read-only 표지 — 에이전트에게 적용하는 것(이 대상에서 이번 앱 실행 중 본 가장 엄한 것).
    pub read_only: bool,
    /// 프로덕션 표지 — 에이전트에게 적용하는 것(같은 규칙). 켜져 있으면 DB 툴이 쓰지 않는다.
    pub production: bool,
    pub state: DbToolConnectionState,
    /// 에이전트의 DB 툴(`db_schema` 등)이 지금 이 연결을 쓸 수 있는가.
    pub usable: bool,
    /// 쓸 수 없는 까닭(`usable` 이 참이면 `null`).
    pub unusable_reason: Option<DbToolUnusable>,
    /// 설정이 틀렸으면 그 안내 글(값을 싣지 않는다 — 까닭은 Pitwall 에서 본다).
    pub message: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolConnectionList {
    /// id 순.
    pub connections: Vec<DbToolConnection>,
    pub trusted: bool,
    /// 프로젝트 설정을 읽지 못했으면 그 안내 글(그러면 목록이 비어 있다. 값을 싣지 않는다).
    pub problem: Option<String>,
}

// --- db_schema ------------------------------------------------------------------------------------------------------

/// 목록의 단계. `database` · `schema` 는 DB · 스키마 목록, 나머지는 그 종류의 객체 목록이다.
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

/// `object` 를 줬을 때 무엇을 읽는가.
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
    /// 열 · 인덱스 · FK.
    #[default]
    Columns,
    /// DDL 글.
    Ddl,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolSchemaParams {
    pub project_id: String,
    /// 워크트리 꼭대기(절대 경로). 연결 설정을 그 워크트리에서 읽는다. 비우면 세션의 워크트리, 없으면 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
    /// `db_connection_list` 의 연결 id.
    pub connection_id: String,
    /// 비우면 연결 설정의 DB.
    #[serde(default)]
    pub database: Option<String>,
    /// 비우면 연결의 기본 스키마(PG `current_schema()`). 스키마가 없는 드라이버(MySQL · SQLite)는 쓰지 않는다.
    #[serde(default)]
    pub schema: Option<String>,
    /// 테이블 · 뷰 등의 이름. 주면 그 객체의 열 · 인덱스 · FK(또는 `detail: "ddl"` 이면 DDL). `schema.name` 꼴이면 점 앞을 스키마로
    /// (스키마가 없는 드라이버는 DB 로) 읽는다 — `schema`(또는 `database`)를 따로 주면 점째 이름이다.
    #[serde(default)]
    pub object: Option<String>,
    /// `object` 가 없을 때 무엇의 목록인가. 기본 `table`.
    #[serde(default)]
    pub kind: Option<DbToolSchemaKind>,
    /// 객체 목록: 이름에 이 글이 든 것만(서버 쪽, 대소문자 무시).
    #[serde(default)]
    pub filter: Option<String>,
    /// 앞 쪽의 `nextCursor`. 처음 쪽은 비운다.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 쪽 크기(1 ~ 500, 기본 100).
    #[serde(default)]
    pub limit: Option<u32>,
    /// `object` 를 줬을 때: `columns`(기본 — 열 · 인덱스 · FK) 또는 `ddl`.
    #[serde(default)]
    pub detail: Option<DbToolSchemaDetail>,
}

/// 결과가 무엇인가.
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

/// 목록의 한 항목.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolSchemaItem {
    pub name: String,
    /// 객체 목록이면 그 종류.
    pub kind: Option<ObjectKind>,
    pub comment: Option<String>,
}

/// 열 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolColumn {
    pub name: String,
    /// DB 의 타입 글.
    #[serde(rename = "type")]
    pub db_type: String,
    pub nullable: bool,
    /// 기본값 식의 글.
    pub default: Option<String>,
    /// 기본 키에 든 열인가.
    pub primary_key: bool,
    pub comment: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolSchema {
    pub level: DbToolSchemaLevel,
    /// 목록이면 그 쪽(`object` 를 줬으면 빈 목록).
    pub items: Vec<DbToolSchemaItem>,
    /// `object` 를 줬으면 읽은 객체(한정한 이름).
    pub object: Option<ObjectRef>,
    pub columns: Option<Vec<DbToolColumn>>,
    pub indexes: Option<Vec<IndexInfo>>,
    pub foreign_keys: Option<Vec<ForeignKeyInfo>>,
    pub ddl: Option<String>,
    /// 다음 쪽의 커서(그대로 `cursor` 로 돌려준다). 없으면 마지막 쪽이다.
    pub next_cursor: Option<String>,
}

// --- db_query -------------------------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolQueryParams {
    pub project_id: String,
    /// 워크트리 꼭대기(절대 경로). 연결 설정과 `file` 을 그 워크트리에서 읽는다. 비우면 세션의 워크트리, 없으면 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
    /// `db_connection_list` 의 연결 id.
    pub connection_id: String,
    /// 돌릴 SQL(문장 여럿 — 방언의 구분자로 나눈다). `file` 과 함께 줄 수 없다.
    #[serde(default)]
    pub sql: Option<String>,
    /// SQL 파일 — 프로젝트 루트(워크트리를 줬으면 그 워크트리 안의 프로젝트 루트) 기준 경로. 루트 안의 UTF-8 일반 파일, 5 MiB 까지.
    /// `sql` 과 함께 줄 수 없다.
    #[serde(default)]
    pub file: Option<String>,
    /// 문장마다 돌려줄 행의 상한(1 ~ 10000, 기본 100). 쪽 단위가 아니다 — 넘으면 그 문장의 `truncated`.
    #[serde(default)]
    pub max_rows: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbToolQuery {
    /// 나눈 문장 수.
    pub statements: u32,
    /// 돈 문장마다 결과 하나(차례대로). 실패한 문장과 그 뒤 문장은 없다.
    pub results: Vec<DbScriptResult>,
    /// 문장이 실패했으면 그 문장과 오류. 그 뒤의 문장은 돌지 않았다.
    pub error: Option<DbScriptError>,
    /// `file` 을 줬으면 읽은 파일(루트 기준 경로).
    pub file: Option<String>,
    /// 감사 로그에 남는 상세 — 문장 수 · 가장 높은 등급 · 문장 해시(`file` 이면 경로와 내용 해시). SQL 원문 · 값은 없다.
    pub audit: String,
}

rpc_method! {
    /// 이 프로젝트의 DB 연결 목록(비밀 없음): id · 이름 · 드라이버 · 대상(호스트 · 포트 · DB · SQLite 경로) · read-only · 프로덕션 ·
    /// 상태, 그리고 에이전트의 DB 툴이 지금 쓸 수 있는가(`usable`)와 그 까닭. 에이전트의 DB 툴은 사람이 이번 앱 실행에서 Pitwall 의
    /// DB 탐색기로 연 연결만 쓰고(`notOpenedByUser`), 프로덕션 표지가 있는 연결은 쓰지 않는다(`productionBlocked`). 연결을 열지 않는다.
    pub DbToolConnectionListMethod: "db.tool.connectionList" {
        params: DbToolConnectionListParams,
        result: DbToolConnectionList,
        tier: Read,
        roles: [Owner],
        mcp: { group: "db", name: "db_connection_list" },
    }

    /// DB 스키마 조회(SQL 드라이버만 — PG · MySQL/MariaDB · SQLite). `object` 가 없으면 한 단계의 목록 한 쪽(`kind`: `database` ·
    /// `schema` · `table`(기본) · `view` 등 — 객체 목록은 `database` · `schema` 안에서, `filter` 로 거른다), 있으면 그 객체의 열(이름 ·
    /// 타입 · null · 기본값 · 기본 키) · 인덱스 · FK, `detail: "ddl"` 이면 DDL 글. Pitwall 의 DB 탐색기와 같은 것을 본다. 사람이 이번
    /// 앱 실행에서 Pitwall 의 DB 탐색기로 연 연결만 쓰고(아니면 `notOpenedByUser` — 사람이 한 번 연 뒤 다시), 프로덕션 표지가 있는
    /// 연결은 쓰지 않는다. Redis 연결이면 `unsupported` — `db_kv_scan` 을 쓴다.
    pub DbToolSchemaMethod: "db.tool.schema" {
        params: DbToolSchemaParams,
        result: DbToolSchema,
        tier: Read,
        roles: [Owner],
        mcp: { group: "db", name: "db_schema" },
    }

    /// 조회 SQL 실행(SQL 드라이버만 — PG · MySQL/MariaDB · SQLite). 입력은 `sql`(글) 또는 `file`(프로젝트 루트 기준 SQL 파일 경로 —
    /// Pitwall 이 실행하는 그때 읽는다) 가운데 하나. 문장을 방언대로 나눠 읽기 전용 트랜잭션 하나에서 차례로 돌리고, 문장마다 결과
    /// 하나(열 · 행 · `truncated` · 소요)를 돌려준다. **조회만 돈다** — 조회가 아닌 문장(쓰기 · DDL · `SET` · 트랜잭션 제어 ·
    /// `ATTACH` · 읽지 못하는 글)이 하나라도 있으면 아무것도 돌리지 않고 거절한다(`notAQuery` — 쓰기는 `db_execute`). 문장 하나가
    /// 실패하면 거기서 멈추고 그때까지의 결과와 `error`(문장 번호 · 줄)를 준다. 행은 문장마다 `maxRows`(기본 100), 셀은 4 KiB 에서
    /// 잘린다(`truncated` 값). **다음 쪽은 없다** — 잘렸으면 WHERE · LIMIT 로 좁혀 다시 부른다(다시 부르면 다른 스냅숏이다).
    /// 사람이 이번 앱 실행에서 Pitwall 의 DB 탐색기로 연 연결만 쓰고(`notOpenedByUser`), 프로덕션 표지가 있는 대상 — MySQL ·
    /// MariaDB 는 같은 서버 · 사용자의 어느 DB 라도 — 은 쓰지 않는다(`productionBlocked`). 결과의 `audit` 은 감사 로그에 남는
    /// 상세(문장 수 · 등급 · 해시)이고 에이전트에게도 보인다.
    pub DbToolQueryMethod: "db.tool.query" {
        params: DbToolQueryParams,
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
        p: &DbToolConnectionListParams,
    ) -> Result<DbToolConnectionList, RpcError> {
        let c = &self.connections;
        let agent = agent_grant(scope, &c.host()?.projects, &p.project_id)?;
        let loaded = agent_view(self, agent.project_id(), p.worktree.as_deref())?;
        let trusted = c.trusted(agent.project_id());
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
                let unusable = if !trusted {
                    Some(DbToolUnusable::Untrusted)
                } else if config.is_none() {
                    Some(DbToolUnusable::Invalid)
                } else {
                    match crate::gate::judge_for_agent(self, &loaded, connection, None) {
                        Ok(_) => None,
                        Err(e) => Some(unusable_of(&e)),
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
        p: DbToolSchemaParams,
    ) -> Result<DbToolSchema, RpcError> {
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
            let (s, scope, project, worktree, id) = (
                Arc::clone(self),
                ctx.scope().clone(),
                p.project_id.clone(),
                p.worktree.clone(),
                p.connection_id.clone(),
            );
            blocking(move || {
                let agent = agent_grant(&scope, &s.connections.host()?.projects, &project)?;
                db_grant_for_agent(&s, &agent, worktree.as_deref(), &id, database.as_deref())
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
                    "db_schema reads SQL databases only; `{}` is a key-value store — use db_kv_scan",
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
        let grant = match object.as_ref().and_then(|o| o.database.clone()) {
            Some(database) if Some(&database) != p.database.as_ref() => {
                gate(Some(database)).await?
            }
            _ => first,
        };
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
        p: DbToolQueryParams,
    ) -> Result<DbToolQuery, RpcError> {
        check_text("worktree", p.worktree.as_deref())?;
        let input = ScriptInput::of(p.sql, p.file)?;
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
        let grant = blocking(move || {
            let agent = agent_grant(&scope, &s.connections.host()?.projects, &project)?;
            db_grant_for_agent(&s, &agent, worktree.as_deref(), &id, None)
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
                        "db_query runs SQL databases only; `{}` is a key-value store — use db_kv_get",
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
        let cancel = QueryCancel::new(conn.interrupter(), Budget::PAGE.cancel_cleanup);
        // 실행은 연결을 쥔 태스크에서 돈다 — 호출이 버려져도(취소 · 세션 끊김) 태스크가 드라이버 호출을 버리고 롤백한 뒤 닫는다.
        let guard = CancelOnDrop::new(cancel.clone());
        let run = {
            let (source, planned) = (source.clone(), planned.clone());
            tokio::spawn(async move {
                // 터널은 연결을 닫을 때까지 쥔다.
                let _tunnel = tunnel;
                let ran =
                    run_read_only(conn.as_mut(), &source, &planned, max_rows, cell, &cancel).await;
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
            file: file.map(|f| f.path),
            audit,
        };
        // 응답 상한(결정 M5-D34) — 행을 뺀 결과의 크기를 먼저 센다.
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

/// 오류 `data` 의 최상위 `audit` 에 감사 상세를 싣는다(`crates/pw-mcp` 머리 주석 「툴 메서드 규약」 — 세션이 감사 행으로 옮긴다).
/// `data` 가 object 가 아니면(글 등) `{ message, audit }` 로 감싼다.
fn with_audit(mut error: RpcError, audit: &str) -> RpcError {
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
                    let scope = ctx.scope().clone();
                    blocking(move || s.tool_connection_list(&scope, &p)).await
                }
            },
        )
        .unary(DbToolSchemaMethod, {
            let sessions = Arc::clone(&sessions);
            move |ctx, p: DbToolSchemaParams| {
                let s = Arc::clone(&sessions);
                async move { s.tool_schema(&ctx, p).await }
            }
        })
        .unary(DbToolQueryMethod, move |ctx, p: DbToolQueryParams| {
            let s = Arc::clone(&sessions);
            async move { s.tool_query(&ctx, p).await }
        });
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
