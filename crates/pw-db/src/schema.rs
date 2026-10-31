//! 스키마 탐색 RPC `db.schema.*` 와 캐시 · 키 판정(M3-10).
//!
//! - **RPC**: `db.schema.databases` · `schemas` · `objects { kind, filter?, cursor?, limit ≤ 500 }` · `columns` · `indexes` ·
//!   `foreignKeys` · `ddl`, 그리고 수동 새로 고침 `db.schema.refresh`. 입력은 모두 `projectId` 를 담은 struct 이고 결과는 쪽 단위다 —
//!   M5 의 `db_schema` 툴이 될 수 있는 꼴(결정 M3-D19). 객체 목록은 드라이버가 서버 쪽에서 거르고 나누고(테이블 1만 개 스키마에서도
//!   첫 쪽만 읽는다), 나머지 목록은 드라이버가 준 목록을 Host 가 `cursor` · `limit` 으로 자른다.
//! - **관문**: 연결 범위([`DbGrant`] — 그 프로젝트 창만)와 신뢰 게이트를 **캐시를 보기 전에** 지난다 — 캐시에 든 것도 다른 창 ·
//!   미신뢰 프로젝트에는 내주지 않는다. 연결은 `crate::connection::connect` 로만 연다.
//! - **메타데이터용 연결**: 사람의 세션(콘솔 탭)과 따로, (프로젝트, 워크트리, 연결 id, 카탈로그)마다 하나를 열어 둔다. 사람의 세션
//!   연결과 갈라져 있으므로 **커밋된 스키마만 본다** — 수동 커밋 탭의 커밋 전 DDL 은 그 탭의 `COMMIT` 뒤 무효화로 보인다. 그 연결을
//!   처음 연 창의 RPC 연결이 닫히거나 [`IDLE_CLOSE`] 동안 쓰지 않으면 닫는다. 스키마 단계가 있는 드라이버(지원 플래그 `schemas` —
//!   PG)는 연결이 그 DB 의 카탈로그만 읽으므로 DB 마다 따로 열되, (프로젝트, 연결 id)마다 [`MAX_LINKS`] 개를 넘으면 가장 오래 안 쓴
//!   것을 닫는다. 질의 도중 RPC 가 취소되면(핸들러 future 가 버려진다) 그 연결을 다시 쓰지 않고 닫는다.
//! - **캐시**: (연결, DB, 스키마) 단위 메모리 캐시, Host 전체 [`MAX_CACHED`] 항목(넘으면 가장 오래 안 쓴 것부터 버린다). 콘솔에서
//!   DDL 등급 이상의 문장이 끝나거나(실패도 — MySQL 은 실패한 DDL 도 암묵 커밋한다) `COMMIT` 하면 실행 파이프라인이
//!   [`DbSessions::schema_touched`] 를 불러 **그 연결의 캐시를 워크트리와 상관없이 모두** 버리고 `db.schemaChanged` 를 낸다. 수동
//!   새로 고침도 같다. 연결 설정이 바뀌면 그 설정으로 채운 캐시를 버린다.
//! - **키 판정**([`row_key`], 그리드 편집 M3-12 이 쓴다): NOT NULL 열만의 PK, 아니면 NOT NULL 열만으로 된 유일 인덱스(부분
//!   인덱스 · 식 인덱스는 제외). 없으면 키 없는 테이블이다(결정 M3-D9).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_core::bus::Scope;
use pw_db_driver::budget::within;
use pw_db_driver::{DbConnection, SqlConnection};
use pw_db_types::{
    ColumnInfo, DatabaseInfo, DbError, DbErrorKind, Ddl, ForeignKeyInfo, IndexInfo, ObjectKind,
    ObjectPage, ObjectQuery, ObjectRef, SchemaInfo,
};
use pw_rpc::{Ctx, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::config::DbConnectionConfig;
use crate::gate::{DbGrant, db_grant};
use crate::session::{DbSession, DbSessions, DbTunnelLease, blocking, db_error, lock};

/// 스키마가 바뀌었을 수 있다. 내용은 [`DbSchemaChanged`]. 그 프로젝트 범위로만 나간다.
pub const TOPIC_SCHEMA_CHANGED: &str = "db.schemaChanged";

/// 쪽 크기의 상한(객체 목록의 `limit`, 그 밖 목록의 기본값).
pub const MAX_PAGE: u32 = 500;

/// 이름 · 필터 · 커서의 길이 상한(바이트).
const MAX_NAME: usize = 1024;

/// 메타데이터 질의 하나의 시간 상한(쪽 단위 실행의 비활동 타임아웃과 같다). 넘으면 그 연결을 버린다 — 질의가 어디서 멈췄는지 모른다.
const META_WAIT: Duration = Duration::from_secs(30);

/// (프로젝트, 연결 id)마다 열어 두는 메타데이터용 연결의 상한. DB 마다 따로 여는 드라이버(PG)에서 DB 가 많은 서버(테넌트마다 DB)를
/// 훑어도 서버의 연결 상한(PG `max_connections` 기본 100)을 사람의 콘솔 세션 · 다른 앱과 나눠 쓸 수 있게 한다. 트리는 한 번에 한두
/// DB 를 펼치므로 넷이면 오가며 펼친 DB 가 대개 열린 채이고, 넘으면 가장 오래 안 쓴 것을 닫는다(다시 펼치면 다시 연다).
pub const MAX_LINKS: usize = 4;

/// 이만큼 쓰지 않은 메타데이터용 연결을 닫는다 — 탐색기를 펼쳐 둔 채 쉬는 창이 서버 연결을 쥐고 있지 않게.
pub const IDLE_CLOSE: Duration = Duration::from_secs(5 * 60);

/// 캐시 항목 수 상한(Host 전체). 항목 하나는 객체 목록 한 쪽(≤ 500개)이나 객체 하나의 열 · 인덱스 · FK 목록 · DDL 이다. 서버 쪽
/// 찾기(글자마다 요청)가 쪽마다 항목을 만들므로 상한 없이는 Host 메모리가 쓰는 동안 계속 는다. 넘으면 가장 오래 안 쓴 것부터 버린다.
pub const MAX_CACHED: usize = 512;

/// 대상 연결(입력 struct 의 공통 몫).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSchemaConnectionParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
    /// 앞 쪽의 `nextCursor`. 처음 쪽은 비운다.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 쪽 크기(1 ~ 500, 기본 500).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSchemaSchemasParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
    /// 비우면 연결 설정의 DB.
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSchemaObjectsParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
    #[serde(default)]
    pub database: Option<String>,
    /// 비우면 연결의 기본 스키마(PG `current_schema()`).
    #[serde(default)]
    pub schema: Option<String>,
    pub kind: ObjectKind,
    /// 이름에 이 글이 든 것만(서버 쪽 필터, 대소문자 무시).
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    /// 쪽 크기(1 ~ 500).
    pub limit: u32,
}

/// 객체 하나에 대한 요청(`columns` · `indexes` · `foreignKeys` · `ddl`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSchemaObjectParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
    pub object: ObjectRef,
    /// `ddl` 은 쪽이 없다 — 주면 무시한다.
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSchemaRefreshParams {
    pub project_id: String,
    pub connection_id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSchemaDisconnectParams {
    pub project_id: String,
    pub connection_id: String,
}

macro_rules! list_type {
    ($(#[$doc:meta])* $name:ident, $item:ty) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
        )]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            pub items: Vec<$item>,
            /// 다음 쪽이 있으면 그 커서(창은 그대로 돌려줄 뿐이다).
            pub next_cursor: Option<String>,
        }
    };
}

list_type!(
    /// DB 목록 한 쪽.
    DbSchemaDatabases,
    DatabaseInfo
);
list_type!(
    /// 스키마 목록 한 쪽.
    DbSchemaSchemas,
    SchemaInfo
);
list_type!(
    /// 열 목록 한 쪽.
    DbSchemaColumns,
    ColumnInfo
);
list_type!(
    /// 인덱스 목록 한 쪽.
    DbSchemaIndexes,
    IndexInfo
);
list_type!(
    /// 외래 키 목록 한 쪽.
    DbSchemaForeignKeys,
    ForeignKeyInfo
);

/// `db.schemaChanged`: 그 연결의 스키마 캐시를 버렸다 — 창은 펼친 노드를 다시 읽는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSchemaChanged {
    pub project_id: String,
    pub connection_id: String,
}

rpc_method! {
    /// DB 목록(지원 플래그 `multipleDatabases`). 그 프로젝트 창 + 신뢰한 프로젝트만. 메타데이터용 연결을 처음 쓰면 연다(비밀 ·
    /// 확인이 필요하면 3101 · 3102, 드라이버가 실패하면 3003).
    pub DbSchemaDatabasesMethod: "db.schema.databases" {
        params: DbSchemaConnectionParams,
        result: DbSchemaDatabases,
        tier: Exec,
        roles: [Owner],
    }

    /// 스키마 목록(지원 플래그 `schemas`).
    pub DbSchemaSchemasMethod: "db.schema.schemas" {
        params: DbSchemaSchemasParams,
        result: DbSchemaSchemas,
        tier: Exec,
        roles: [Owner],
    }

    /// 객체 목록 한 쪽 — 필터와 쪽은 서버 쪽에서 한다(`limit` 1 ~ 500).
    pub DbSchemaObjectsMethod: "db.schema.objects" {
        params: DbSchemaObjectsParams,
        result: ObjectPage,
        tier: Exec,
        roles: [Owner],
    }

    /// 테이블 · 뷰의 열.
    pub DbSchemaColumnsMethod: "db.schema.columns" {
        params: DbSchemaObjectParams,
        result: DbSchemaColumns,
        tier: Exec,
        roles: [Owner],
    }

    /// 테이블의 인덱스.
    pub DbSchemaIndexesMethod: "db.schema.indexes" {
        params: DbSchemaObjectParams,
        result: DbSchemaIndexes,
        tier: Exec,
        roles: [Owner],
    }

    /// 테이블의 외래 키.
    pub DbSchemaForeignKeysMethod: "db.schema.foreignKeys" {
        params: DbSchemaObjectParams,
        result: DbSchemaForeignKeys,
        tier: Exec,
        roles: [Owner],
    }

    /// 객체의 DDL 글(`⌘B`).
    pub DbSchemaDdlMethod: "db.schema.ddl" {
        params: DbSchemaObjectParams,
        result: Ddl,
        tier: Exec,
        roles: [Owner],
    }

    /// 수동 새로 고침: 그 연결의 스키마 캐시를 버리고 `db.schemaChanged` 를 낸다. 연결을 열지 않는다. 그 프로젝트 창만.
    pub DbSchemaRefreshMethod: "db.schema.refresh" {
        params: DbSchemaRefreshParams,
        result: (),
        tier: Read,
        roles: [Owner],
    }

    /// 탐색기의 「끊기」: 그 연결(모든 워크트리 · DB)의 메타데이터용 연결을 닫는다. 캐시는 둔다 — 스키마가 바뀐 것은 아니다. 연결을
    /// 열지 않고 신뢰 게이트를 지나지 않는다(닫기만 한다 — 신뢰를 거둔 뒤에도 사람이 닫을 수 있어야 한다). 그 프로젝트 창만. 도는
    /// 조회가 있으면 그것이 끝난 뒤 닫는다.
    pub DbSchemaDisconnectMethod: "db.schema.disconnect" {
        params: DbSchemaDisconnectParams,
        result: (),
        tier: Read,
        roles: [Owner],
    }
}

/// 테이블의 행 키(그리드 편집이 `WHERE <키> = ?` 로 쓴다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowKey {
    /// 키를 이루는 인덱스의 이름.
    pub index: String,
    pub columns: Vec<String>,
    pub primary: bool,
}

/// 키 판정(결정 M3-D9): PK 가 있으면 그것, 없으면 **NOT NULL 열만으로 된** 유일 인덱스 가운데 열이 가장 적은 것(같으면 이름 순).
/// 부분 인덱스(PG `WHERE …`)는 몇 행만 유일하게 하므로, 열 목록에 없는 이름(식 인덱스의 식)이 낀 인덱스는 행을 열 값으로 가리키지
/// 못하므로 키가 아니다. NULL 허용 열이 낀 유일 인덱스는 NULL 이 여러 번 들어갈 수 있어 키가 아니다 — PK 도 같다(SQLite 의 rowid
/// 테이블은 `INTEGER PRIMARY KEY` 가 아닌 PK 열에 NULL 을 받는다. 드라이버가 그 열을 NULL 허용으로 낸다). 없으면 `None`(키 없는
/// 테이블 — 그리드 편집을 받지 않는다).
pub fn row_key(columns: &[ColumnInfo], indexes: &[IndexInfo]) -> Option<RowKey> {
    let not_null = |name: &String| columns.iter().any(|c| &c.name == name && !c.nullable);
    let usable =
        |i: &&IndexInfo| !i.partial && !i.columns.is_empty() && i.columns.iter().all(not_null);
    let key = |i: &IndexInfo| RowKey {
        index: i.name.clone(),
        columns: i.columns.clone(),
        primary: i.primary,
    };
    if let Some(primary) = indexes.iter().filter(usable).find(|i| i.primary) {
        return Some(key(primary));
    }
    indexes
        .iter()
        .filter(usable)
        .filter(|i| i.unique)
        .min_by(|a, b| {
            a.columns
                .len()
                .cmp(&b.columns.len())
                .then_with(|| a.name.cmp(&b.name))
        })
        .map(key)
}

/// 연결 하나(워크트리마다 설정이 다를 수 있다).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnKey {
    project: String,
    worktree: Option<String>,
    connection: String,
}

/// 메타데이터용 연결 하나의 열쇠. `catalog` 는 DB 마다 따로 여는 드라이버(PG)의 DB.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LinkKey {
    conn: ConnKey,
    catalog: Option<String>,
}

/// 드라이버에 보낼 요청.
#[derive(Debug, Clone)]
enum Request {
    Databases,
    Schemas(Option<String>),
    Objects(ObjectQuery),
    Columns(ObjectRef),
    Indexes(ObjectRef),
    ForeignKeys(ObjectRef),
    Ddl(ObjectRef),
}

/// 캐시의 항목 종류.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Item {
    Databases,
    Schemas,
    Objects {
        kind: ObjectKind,
        filter: Option<String>,
        cursor: Option<String>,
        limit: u32,
    },
    Columns(String),
    Indexes(String),
    ForeignKeys(String),
    Ddl(String),
}

/// 캐시 열쇠: (연결, DB, 스키마) + 항목.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    conn: ConnKey,
    database: Option<String>,
    schema: Option<String>,
    item: Item,
}

impl Request {
    fn key(&self, conn: &ConnKey) -> CacheKey {
        let at = |database: &Option<String>, schema: &Option<String>, item| CacheKey {
            conn: conn.clone(),
            database: database.clone(),
            schema: schema.clone(),
            item,
        };
        match self {
            Self::Databases => at(&None, &None, Item::Databases),
            Self::Schemas(database) => at(database, &None, Item::Schemas),
            Self::Objects(q) => at(
                &q.database,
                &q.schema,
                Item::Objects {
                    kind: q.kind,
                    filter: q.filter.clone(),
                    cursor: q.cursor.clone(),
                    limit: q.limit,
                },
            ),
            Self::Columns(o) => at(&o.database, &o.schema, Item::Columns(o.name.clone())),
            Self::Indexes(o) => at(&o.database, &o.schema, Item::Indexes(o.name.clone())),
            Self::ForeignKeys(o) => at(&o.database, &o.schema, Item::ForeignKeys(o.name.clone())),
            Self::Ddl(o) => at(&o.database, &o.schema, Item::Ddl(o.name.clone())),
        }
    }

    /// 이 요청이 읽는 DB(카탈로그).
    fn database(&self) -> Option<&String> {
        match self {
            Self::Databases => None,
            Self::Schemas(database) => database.as_ref(),
            Self::Objects(q) => q.database.as_ref(),
            Self::Columns(o) | Self::Indexes(o) | Self::ForeignKeys(o) | Self::Ddl(o) => {
                o.database.as_ref()
            }
        }
    }

    async fn run(self, conn: &mut dyn SqlConnection) -> Result<Cached, DbError> {
        Ok(match self {
            Self::Databases => Cached::Databases(conn.databases().await?),
            Self::Schemas(database) => Cached::Schemas(conn.schemas(database).await?),
            Self::Objects(q) => Cached::Objects(conn.objects(q).await?),
            Self::Columns(o) => Cached::Columns(conn.columns(o).await?),
            Self::Indexes(o) => Cached::Indexes(conn.indexes(o).await?),
            Self::ForeignKeys(o) => Cached::ForeignKeys(conn.foreign_keys(o).await?),
            Self::Ddl(o) => Cached::Ddl(conn.ddl(o).await?),
        })
    }
}

#[derive(Debug, Clone)]
enum Cached {
    Databases(Vec<DatabaseInfo>),
    Schemas(Vec<SchemaInfo>),
    Objects(ObjectPage),
    Columns(Vec<ColumnInfo>),
    Indexes(Vec<IndexInfo>),
    ForeignKeys(Vec<ForeignKeyInfo>),
    Ddl(Ddl),
}

/// 열어 둔 메타데이터용 연결.
struct MetaConn {
    conn: Box<dyn SqlConnection>,
    /// 연 때의 연결 설정 — 지금 것과 다르면 닫고 다시 연다.
    config: DbConnectionConfig,
    /// 이 연결의 번호(지켜보는 태스크가 자기가 본 연결만 닫게).
    id: u64,
    /// 닫히면 취소된다(연 창을 지켜보는 태스크가 끝난다).
    retired: CancellationToken,
    /// SSH 터널(M3-11). 연결을 쥐는 동안 함께 쥐고 [`retire`] 가 연결을 닫은 뒤에 놓는다 — 먼저 버리면 ssh 가 끝나 연결이 끊긴다.
    /// 필드 순서가 버리는 순서다: 런타임 밖에서 그냥 버려질 때도 `conn` 이 먼저 버려지게 맨 뒤에 둔다.
    tunnel: Option<DbTunnelLease>,
}

/// 질의하는 동안 연결을 쥔다. 질의가 끝나기 전에 버려지면(RPC 취소는 핸들러 future 를 그 자리에서 버린다) 연결도 닫는다 — 프로토콜
/// 한가운데서 멈춘 연결(MySQL 의 읽다 만 패킷 · 닫지 못한 준비 문장)을 다음 조회가 쓰면 어긋난 응답을 읽는다.
struct InFlight(Option<MetaConn>);

impl InFlight {
    fn conn(&mut self) -> Option<&mut dyn SqlConnection> {
        Some(self.0.as_mut()?.conn.as_mut())
    }

    /// 질의가 끝났다 — 연결을 돌려받는다.
    fn land(mut self) -> Option<MetaConn> {
        self.0.take()
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let Some(meta) = self.0.take() else { return };
        tracing::debug!(target: "pw_db", "a db metadata query was dropped mid-way; closing its connection");
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(retire(meta));
            }
            // 런타임 밖(종료 중)이면 닫기를 기다릴 수 없다 — 버리면 드라이버가 소켓을 닫는다.
            Err(_) => meta.retired.cancel(),
        }
    }
}

/// (프로젝트, 워크트리, 연결 id, 카탈로그) 하나의 메타데이터용 연결 자리.
struct MetaLink {
    open: Arc<AsyncMutex<Option<MetaConn>>>,
    /// 자리가 표에서 빠졌다(상한 · 쉼 · 창 닫힘) — 이 자리를 쥔 조회는 표에서 새 자리를 받는다.
    gone: AtomicBool,
    /// 마지막으로 조회가 쓴 때(가장 오래 안 쓴 것 고르기 · 쉼).
    used: Mutex<Instant>,
}

impl MetaLink {
    fn new() -> Self {
        Self {
            open: Arc::default(),
            gone: AtomicBool::new(false),
            used: Mutex::new(Instant::now()),
        }
    }

    fn touch(&self) {
        *lock(&self.used) = Instant::now();
    }

    fn used(&self) -> Instant {
        *lock(&self.used)
    }

    fn gone(&self) -> bool {
        self.gone.load(Ordering::SeqCst)
    }
}

type Links = Arc<Mutex<HashMap<LinkKey, Arc<MetaLink>>>>;

/// 캐시 항목.
struct Entry {
    value: Cached,
    /// 마지막으로 읽거나 넣은 순번(가장 오래 안 쓴 것 고르기).
    used: u64,
}

/// 캐시와 무효화 순번 — 한 잠금 안에 둔다. 순번을 보고 넣는 사이에 무효화가 끼면 낡은 값이 비운 캐시에 들어간다.
#[derive(Default)]
struct Cache {
    entries: HashMap<CacheKey, Entry>,
    /// (프로젝트, 연결 id)마다 무효화 순번 — 버리기 전에 시작한 조회가 버린 뒤에 낡은 값을 넣지 못하게. 조회를 시작할 때 만든다
    /// (신뢰 게이트와 설정 읽기를 지난 연결만 — 아무 연결 id 의 새로 고침이 항목을 늘리지 않는다).
    generations: HashMap<(String, String), u64>,
    tick: u64,
}

impl Cache {
    fn get(&mut self, key: &CacheKey) -> Option<Cached> {
        self.tick += 1;
        let entry = self.entries.get_mut(key)?;
        entry.used = self.tick;
        Some(entry.value.clone())
    }

    fn generation(&mut self, project: &str, connection: &str) -> u64 {
        *self
            .generations
            .entry((project.to_owned(), connection.to_owned()))
            .or_default()
    }

    fn bump(&mut self, project: &str, connection: &str) {
        if let Some(g) = self
            .generations
            .get_mut(&(project.to_owned(), connection.to_owned()))
        {
            *g += 1;
        }
    }

    /// `generation` 에 시작한 조회의 결과를 넣는다 — 그 뒤 무효화가 있었으면 넣지 않는다.
    fn put(&mut self, key: CacheKey, value: Cached, generation: u64) {
        let now = self
            .generations
            .get(&(key.conn.project.clone(), key.conn.connection.clone()))
            .copied()
            .unwrap_or(0);
        if now != generation {
            return;
        }
        self.tick += 1;
        self.entries.insert(
            key,
            Entry {
                value,
                used: self.tick,
            },
        );
        while self.entries.len() > MAX_CACHED {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }
}

/// 스키마 캐시와 메타데이터용 연결(`DbSessions` 가 하나 갖는다).
pub struct SchemaState {
    cache: Mutex<Cache>,
    /// 캐시를 채운 설정(연결마다). 지금 설정과 다르면 그 연결의 캐시를 버린다.
    configs: Mutex<HashMap<ConnKey, DbConnectionConfig>>,
    links: Links,
    /// 쉬는 연결을 닫는 시간([`IDLE_CLOSE`], 시험이 줄인다).
    idle_close: Mutex<Duration>,
    /// 지금까지 연 메타데이터용 연결 수(시험).
    opened: AtomicU64,
}

impl Default for SchemaState {
    fn default() -> Self {
        Self {
            cache: Mutex::default(),
            configs: Mutex::default(),
            links: Links::default(),
            idle_close: Mutex::new(IDLE_CLOSE),
            opened: AtomicU64::new(0),
        }
    }
}

impl SchemaState {
    /// 그 연결(모든 워크트리)의 캐시를 버린다.
    fn invalidate(&self, project: &str, connection: &str) {
        let mut cache = lock(&self.cache);
        cache.bump(project, connection);
        cache
            .entries
            .retain(|k, _| !(k.conn.project == project && k.conn.connection == connection));
    }

    /// 캐시를 채운 설정이 `now` 와 다르면 그 워크트리의 캐시를 버린다.
    fn follow_config(&self, conn: &ConnKey, now: &DbConnectionConfig) {
        let mut configs = lock(&self.configs);
        if configs.get(conn) == Some(now) {
            return;
        }
        configs.insert(conn.clone(), now.clone());
        drop(configs);
        let mut cache = lock(&self.cache);
        cache.bump(&conn.project, &conn.connection);
        cache.entries.retain(|k, _| &k.conn != conn);
    }

    fn cached(&self, key: &CacheKey) -> Option<Cached> {
        lock(&self.cache).get(key)
    }

    /// `key` 의 자리를 준다. 새 자리를 만들어 그 (프로젝트, 연결 id)의 자리가 [`MAX_LINKS`] 를 넘으면 가장 오래 안 쓴 자리를 표에서
    /// 빼서 돌려준다 — 부른 쪽이 그 연결을 닫은 뒤에 새 연결을 연다.
    fn link(&self, key: &LinkKey) -> (Arc<MetaLink>, Vec<Arc<MetaLink>>) {
        let mut links = lock(&*self.links);
        if let Some(link) = links.get(key) {
            link.touch();
            return (Arc::clone(link), Vec::new());
        }
        let same = |k: &LinkKey| {
            k.conn.project == key.conn.project && k.conn.connection == key.conn.connection
        };
        let mut evicted = Vec::new();
        while links.keys().filter(|k| same(k)).count() >= MAX_LINKS {
            let Some(oldest) = links
                .iter()
                .filter(|(k, _)| same(k))
                .min_by_key(|(_, l)| l.used())
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(link) = links.remove(&oldest) {
                link.gone.store(true, Ordering::SeqCst);
                evicted.push(link);
            }
        }
        let link = Arc::new(MetaLink::new());
        links.insert(key.clone(), Arc::clone(&link));
        (link, evicted)
    }

    /// 그 (프로젝트, 연결 id)의 자리를 워크트리 · 카탈로그와 상관없이 모두 표에서 뺀다. 그 자리를 쥔 조회는 다음에 새 자리를 받는다.
    fn take_links(&self, project: &str, connection: &str) -> Vec<Arc<MetaLink>> {
        let mut links = lock(&*self.links);
        let keys: Vec<LinkKey> = links
            .keys()
            .filter(|k| k.conn.project == project && k.conn.connection == connection)
            .cloned()
            .collect();
        keys.iter()
            .filter_map(|k| links.remove(k))
            .inspect(|link| link.gone.store(true, Ordering::SeqCst))
            .collect()
    }

    /// 쉬는 메타데이터용 연결을 닫는 시간을 바꾼다(시험 — 이미 연 연결에는 닿지 않는다).
    pub fn set_idle_close(&self, idle: Duration) {
        *lock(&self.idle_close) = idle;
    }

    /// 지금까지 연 메타데이터용 연결 수(시험).
    pub fn connections_opened(&self) -> u64 {
        self.opened.load(Ordering::SeqCst)
    }

    /// 지금 열어 둔 메타데이터용 연결 자리 수(시험).
    pub fn open_links(&self) -> usize {
        lock(&*self.links).len()
    }
}

/// 표에서 뺀 자리의 연결을 닫는다(그 자리에서 도는 조회가 있으면 끝나기를 기다린다).
async fn shut(link: &MetaLink) {
    link.gone.store(true, Ordering::SeqCst);
    let mut open = link.open.lock().await;
    if let Some(meta) = open.take() {
        retire(meta).await;
    }
}

/// 이름 · 필터 · 커서: 1024 바이트 이하, 빈 글은 거절.
fn check_text(what: &str, value: Option<&str>) -> Result<(), RpcError> {
    match value {
        Some(v) if v.is_empty() || v.len() > MAX_NAME => Err(RpcError::invalid_params(format!(
            "{what} must be 1-{MAX_NAME} bytes"
        ))),
        _ => Ok(()),
    }
}

fn check_object(object: &ObjectRef) -> Result<(), RpcError> {
    check_text("object.database", object.database.as_deref())?;
    check_text("object.schema", object.schema.as_deref())?;
    check_text("object.name", Some(&object.name))
}

fn check_limit(limit: u32) -> Result<u32, RpcError> {
    if (1..=MAX_PAGE).contains(&limit) {
        Ok(limit)
    } else {
        Err(RpcError::invalid_params(format!(
            "limit must be 1-{MAX_PAGE}"
        )))
    }
}

/// 드라이버가 준 목록 하나를 `cursor`(앞에서 건너뛴 수) · `limit` 으로 자른다.
fn slice<T: Clone>(
    items: &[T],
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<(Vec<T>, Option<String>), RpcError> {
    let limit = check_limit(limit.unwrap_or(MAX_PAGE))? as usize;
    let start = match cursor {
        None => 0,
        Some(c) => c
            .parse::<usize>()
            .ok()
            .filter(|s| *s <= items.len())
            .ok_or_else(|| RpcError::invalid_params("cursor is not one this list gave"))?,
    };
    let end = start.saturating_add(limit).min(items.len());
    Ok((
        items[start..end].to_vec(),
        (end < items.len()).then(|| end.to_string()),
    ))
}

fn unexpected() -> RpcError {
    RpcError::internal("the schema cache returned another kind of item")
}

/// 이 연결을 다시 쓰지 말아야 하는 오류(연결이 어디서 멈췄는지 모른다).
fn breaks_connection(error: &DbError) -> bool {
    matches!(
        error.kind,
        DbErrorKind::Disconnected | DbErrorKind::Timeout | DbErrorKind::Cancelled
    )
}

impl DbSessions {
    /// 실행 파이프라인이 부른다: 콘솔에서 DDL 등급 이상의 문장이 끝났거나(실패 포함) `COMMIT` 했다.
    pub(crate) fn schema_touched(&self, session: &DbSession) {
        self.schema_changed(&session.project_id, &session.connection_id);
    }

    /// 그 연결의 스키마 캐시를 버리고 `db.schemaChanged` 를 그 프로젝트 범위로 낸다.
    pub fn schema_changed(&self, project_id: &str, connection_id: &str) {
        self.schema.invalidate(project_id, connection_id);
        tracing::debug!(target: "pw_db", project = project_id, connection = connection_id, "db schema cache dropped");
        let Some(bus) = self.bus() else { return };
        let payload = DbSchemaChanged {
            project_id: project_id.to_owned(),
            connection_id: connection_id.to_owned(),
        };
        if let Err(e) = bus.publish(
            Scope::Project(project_id.to_owned()),
            TOPIC_SCHEMA_CHANGED,
            &payload,
        ) {
            tracing::warn!(target: "pw_db", %e, "cannot publish db.schemaChanged");
        }
    }

    /// 그 연결의 메타데이터용 연결을 모두 닫는다(탐색기의 「끊기」). 캐시는 둔다.
    pub async fn close_metadata(&self, project_id: &str, connection_id: &str) {
        for link in self.schema.take_links(project_id, connection_id) {
            shut(&link).await;
        }
    }

    /// 스키마 캐시 · 메타데이터용 연결(시험).
    pub fn schema_state(&self) -> &SchemaState {
        &self.schema
    }

    /// 관문(연결 범위는 부른 쪽이 [`DbGrant`] 로 지났다) → 신뢰 · 지금 설정 → 캐시 → 메타데이터용 연결.
    async fn meta(
        self: &Arc<Self>,
        ctx: &Ctx,
        grant: &DbGrant,
        worktree: Option<&str>,
        connection_id: &str,
        request: Request,
    ) -> Result<Cached, RpcError> {
        check_text("worktree", worktree)?;
        let (c, project, wt, id) = (
            Arc::clone(&self.connections),
            grant.project_id().to_owned(),
            worktree.map(str::to_owned),
            connection_id.to_owned(),
        );
        // 신뢰는 캐시를 보기 전에 본다 — 신뢰를 거둔 프로젝트에는 캐시에 든 것도 내주지 않는다.
        let config = blocking(move || {
            c.trust_gate(&project, wt.as_deref())?;
            let loaded = c.load(&project, wt.as_deref())?;
            let connection = loaded
                .get(&id)
                .ok_or_else(|| crate::config::not_found(&id))?;
            connection.config.clone().map_err(|e| {
                RpcError::invalid_params(format!("db connection `{id}` is invalid: {e}"))
            })
        })
        .await?;
        let driver = self.drivers.get(config.driver.id()).ok_or_else(|| {
            db_error(DbError::new(
                DbErrorKind::Unsupported,
                format!("no db driver `{}` is installed", config.driver.id()),
            ))
        })?;
        let capabilities = driver.capabilities();
        if !capabilities.sql {
            return Err(RpcError::invalid_params(format!(
                "db connection `{connection_id}` has no schema to browse"
            )));
        }
        let conn = ConnKey {
            project: grant.project_id().to_owned(),
            worktree: worktree.map(str::to_owned),
            connection: connection_id.to_owned(),
        };
        self.schema.follow_config(&conn, &config);
        let key = request.key(&conn);
        if let Some(hit) = self.schema.cached(&key) {
            return Ok(hit);
        }
        let generation = lock(&self.schema.cache).generation(&conn.project, &conn.connection);
        let catalog = if capabilities.schemas {
            request.database().cloned()
        } else {
            None
        };
        let link_key = LinkKey {
            conn: conn.clone(),
            catalog: catalog.clone(),
        };
        // 자리를 받아 잠근다. 기다리는 사이에 자리가 표에서 빠졌으면(상한 · 쉼 · 창 닫힘) 새 자리를 받는다.
        let (link, mut open) = loop {
            let (link, evicted) = self.schema.link(&link_key);
            for old in evicted {
                shut(&old).await;
            }
            let open = Arc::clone(&link.open).lock_owned().await;
            if !link.gone() {
                break (link, open);
            }
        };
        // 같은 것을 먼저 기다린 조회가 채웠으면 그것을 쓴다.
        if let Some(hit) = self.schema.cached(&key) {
            return Ok(hit);
        }
        if open.as_ref().is_some_and(|m| m.config != config)
            && let Some(stale) = open.take()
        {
            retire(stale).await;
        }
        if open.is_none() {
            let opened =
                crate::connection::connect(self, grant, worktree, connection_id, catalog).await?;
            let tunnel = opened.tunnel;
            let sql = match opened.connection {
                DbConnection::Sql(sql) => sql,
                other => {
                    let _ = other.close().await;
                    drop(tunnel);
                    return Err(RpcError::invalid_params(format!(
                        "db connection `{connection_id}` has no schema to browse"
                    )));
                }
            };
            let id = self.schema.opened.fetch_add(1, Ordering::SeqCst);
            let retired = CancellationToken::new();
            self.watch(
                ctx.connection_closed(),
                retired.clone(),
                id,
                &link_key,
                &link,
            );
            *open = Some(MetaConn {
                conn: sql,
                config: opened.config,
                id,
                retired,
                tunnel,
            });
        }
        link.touch();
        // 질의하는 동안 연결은 자리 밖(`InFlight`)에 있다 — 이 future 가 버려지면 연결도 닫힌다.
        let mut flight = InFlight(open.take());
        let Some(conn) = flight.conn() else {
            return Err(unexpected());
        };
        let result = within(META_WAIT, "metadata", request.run(conn)).await;
        let meta = flight.land();
        match result {
            Ok(value) => {
                *open = meta;
                lock(&self.schema.cache).put(key, value.clone(), generation);
                Ok(value)
            }
            Err(error) => {
                match meta {
                    Some(broken) if breaks_connection(&error) => retire(broken).await,
                    meta => *open = meta,
                }
                Err(db_error(error))
            }
        }
    }

    /// 메타데이터용 연결을 지켜보다 닫는다: 연 창의 RPC 연결이 닫히거나(사람이 누를 때만 연결이 열려 있게 — 결정 M3-D13) 쉬는
    /// 시간이 지나면 자리를 표에서 빼고 연결을 닫는다. 연결이 다른 길로 닫히면(`retired`) 끝난다.
    fn watch(
        &self,
        window_closed: CancellationToken,
        retired: CancellationToken,
        id: u64,
        key: &LinkKey,
        link: &Arc<MetaLink>,
    ) {
        let (link, key, links) = (
            Arc::clone(link),
            key.clone(),
            Arc::clone(&self.schema.links),
        );
        let idle = *lock(&self.schema.idle_close);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = window_closed.cancelled() => break,
                    () = retired.cancelled() => return,
                    () = tokio::time::sleep_until(link.used() + idle) => {
                        if link.used() + idle <= Instant::now() {
                            break;
                        }
                    }
                }
            }
            {
                let mut links = lock(&*links);
                if links.get(&key).is_some_and(|l| Arc::ptr_eq(l, &link)) {
                    links.remove(&key);
                }
            }
            link.gone.store(true, Ordering::SeqCst);
            let mut open = link.open.lock().await;
            if open.as_ref().is_some_and(|m| m.id == id)
                && let Some(meta) = open.take()
            {
                retire(meta).await;
            }
        });
    }

    async fn databases(
        self: &Arc<Self>,
        ctx: &Ctx,
        p: DbSchemaConnectionParams,
    ) -> Result<DbSchemaDatabases, RpcError> {
        let grant = db_grant(ctx.scope(), &p.project_id)?;
        check_text("cursor", p.cursor.as_deref())?;
        let Cached::Databases(all) = self
            .meta(
                ctx,
                &grant,
                p.worktree.as_deref(),
                &p.connection_id,
                Request::Databases,
            )
            .await?
        else {
            return Err(unexpected());
        };
        let (items, next_cursor) = slice(&all, p.cursor.as_deref(), p.limit)?;
        Ok(DbSchemaDatabases { items, next_cursor })
    }

    async fn schemas(
        self: &Arc<Self>,
        ctx: &Ctx,
        p: DbSchemaSchemasParams,
    ) -> Result<DbSchemaSchemas, RpcError> {
        let grant = db_grant(ctx.scope(), &p.project_id)?;
        check_text("database", p.database.as_deref())?;
        check_text("cursor", p.cursor.as_deref())?;
        let request = Request::Schemas(p.database.clone());
        let Cached::Schemas(all) = self
            .meta(
                ctx,
                &grant,
                p.worktree.as_deref(),
                &p.connection_id,
                request,
            )
            .await?
        else {
            return Err(unexpected());
        };
        let (items, next_cursor) = slice(&all, p.cursor.as_deref(), p.limit)?;
        Ok(DbSchemaSchemas { items, next_cursor })
    }

    async fn objects(
        self: &Arc<Self>,
        ctx: &Ctx,
        p: DbSchemaObjectsParams,
    ) -> Result<ObjectPage, RpcError> {
        let grant = db_grant(ctx.scope(), &p.project_id)?;
        check_text("database", p.database.as_deref())?;
        check_text("schema", p.schema.as_deref())?;
        check_text("filter", p.filter.as_deref())?;
        check_text("cursor", p.cursor.as_deref())?;
        let request = Request::Objects(ObjectQuery {
            database: p.database,
            schema: p.schema,
            kind: p.kind,
            filter: p.filter,
            cursor: p.cursor,
            limit: check_limit(p.limit)?,
        });
        match self
            .meta(
                ctx,
                &grant,
                p.worktree.as_deref(),
                &p.connection_id,
                request,
            )
            .await?
        {
            Cached::Objects(page) => Ok(page),
            _ => Err(unexpected()),
        }
    }

    /// `columns` · `indexes` · `foreignKeys` · `ddl` 의 공통 몫.
    async fn object_item(
        self: &Arc<Self>,
        ctx: &Ctx,
        p: &DbSchemaObjectParams,
        request: impl FnOnce(ObjectRef) -> Request,
    ) -> Result<Cached, RpcError> {
        let grant = db_grant(ctx.scope(), &p.project_id)?;
        check_object(&p.object)?;
        check_text("cursor", p.cursor.as_deref())?;
        self.meta(
            ctx,
            &grant,
            p.worktree.as_deref(),
            &p.connection_id,
            request(p.object.clone()),
        )
        .await
    }

    /// 테이블의 행 키(그리드 편집 M3-12): 캐시를 지나 열 · 인덱스를 읽고 [`row_key`] 로 판정한다. 관문은 부른 쪽이 지났다.
    pub async fn table_row_key(
        self: &Arc<Self>,
        ctx: &Ctx,
        grant: &DbGrant,
        worktree: Option<&str>,
        connection_id: &str,
        table: ObjectRef,
    ) -> Result<Option<RowKey>, RpcError> {
        Ok(self
            .table_shape(ctx, grant, worktree, connection_id, table)
            .await?
            .1)
    }

    /// 테이블의 열과 행 키(그리드 편집 M3-12 — 열 이름을 방언 규칙으로 맞추는 데 열 목록이 필요하다). 캐시를 지난다. 관문은 부른 쪽이
    /// 지났다.
    pub async fn table_shape(
        self: &Arc<Self>,
        ctx: &Ctx,
        grant: &DbGrant,
        worktree: Option<&str>,
        connection_id: &str,
        table: ObjectRef,
    ) -> Result<(Vec<ColumnInfo>, Option<RowKey>), RpcError> {
        check_object(&table)?;
        let Cached::Columns(columns) = self
            .meta(
                ctx,
                grant,
                worktree,
                connection_id,
                Request::Columns(table.clone()),
            )
            .await?
        else {
            return Err(unexpected());
        };
        let Cached::Indexes(indexes) = self
            .meta(ctx, grant, worktree, connection_id, Request::Indexes(table))
            .await?
        else {
            return Err(unexpected());
        };
        let key = row_key(&columns, &indexes);
        Ok((columns, key))
    }
}

/// 메타데이터용 연결을 닫는다.
async fn retire(meta: MetaConn) {
    let MetaConn {
        conn,
        retired,
        tunnel,
        ..
    } = meta;
    retired.cancel();
    if let Err(e) = within(Duration::from_secs(10), "close", conn.close()).await {
        tracing::info!(target: "pw_db", %e, "closing a db metadata connection failed");
    }
    // 터널은 연결을 닫은 뒤에 놓는다.
    drop(tunnel);
}

/// `db.schema.*` 를 붙인다.
pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    let s = sessions;
    let (databases, schemas, objects, columns, indexes, foreign_keys, ddl, refresh, disconnect) = (
        Arc::clone(&s),
        Arc::clone(&s),
        Arc::clone(&s),
        Arc::clone(&s),
        Arc::clone(&s),
        Arc::clone(&s),
        Arc::clone(&s),
        Arc::clone(&s),
        s,
    );
    router
        .unary(
            DbSchemaDatabasesMethod,
            move |ctx, p: DbSchemaConnectionParams| {
                let s = Arc::clone(&databases);
                async move { s.databases(&ctx, p).await }
            },
        )
        .unary(
            DbSchemaSchemasMethod,
            move |ctx, p: DbSchemaSchemasParams| {
                let s = Arc::clone(&schemas);
                async move { s.schemas(&ctx, p).await }
            },
        )
        .unary(
            DbSchemaObjectsMethod,
            move |ctx, p: DbSchemaObjectsParams| {
                let s = Arc::clone(&objects);
                async move { s.objects(&ctx, p).await }
            },
        )
        .unary(
            DbSchemaColumnsMethod,
            move |ctx, p: DbSchemaObjectParams| {
                let s = Arc::clone(&columns);
                async move {
                    let Cached::Columns(all) = s.object_item(&ctx, &p, Request::Columns).await?
                    else {
                        return Err(unexpected());
                    };
                    let (items, next_cursor) = slice(&all, p.cursor.as_deref(), p.limit)?;
                    Ok(DbSchemaColumns { items, next_cursor })
                }
            },
        )
        .unary(
            DbSchemaIndexesMethod,
            move |ctx, p: DbSchemaObjectParams| {
                let s = Arc::clone(&indexes);
                async move {
                    let Cached::Indexes(all) = s.object_item(&ctx, &p, Request::Indexes).await?
                    else {
                        return Err(unexpected());
                    };
                    let (items, next_cursor) = slice(&all, p.cursor.as_deref(), p.limit)?;
                    Ok(DbSchemaIndexes { items, next_cursor })
                }
            },
        )
        .unary(
            DbSchemaForeignKeysMethod,
            move |ctx, p: DbSchemaObjectParams| {
                let s = Arc::clone(&foreign_keys);
                async move {
                    let Cached::ForeignKeys(all) =
                        s.object_item(&ctx, &p, Request::ForeignKeys).await?
                    else {
                        return Err(unexpected());
                    };
                    let (items, next_cursor) = slice(&all, p.cursor.as_deref(), p.limit)?;
                    Ok(DbSchemaForeignKeys { items, next_cursor })
                }
            },
        )
        .unary(DbSchemaDdlMethod, move |ctx, p: DbSchemaObjectParams| {
            let s = Arc::clone(&ddl);
            async move {
                match s.object_item(&ctx, &p, Request::Ddl).await? {
                    Cached::Ddl(ddl) => Ok(ddl),
                    _ => Err(unexpected()),
                }
            }
        })
        .unary(
            DbSchemaRefreshMethod,
            move |ctx, p: DbSchemaRefreshParams| {
                let s = Arc::clone(&refresh);
                async move {
                    db_grant(ctx.scope(), &p.project_id)?;
                    check_text("connectionId", Some(&p.connection_id))?;
                    s.schema_changed(&p.project_id, &p.connection_id);
                    Ok(())
                }
            },
        )
        .unary(
            DbSchemaDisconnectMethod,
            move |ctx, p: DbSchemaDisconnectParams| {
                let s = Arc::clone(&disconnect);
                async move {
                    db_grant(ctx.scope(), &p.project_id)?;
                    check_text("connectionId", Some(&p.connection_id))?;
                    s.close_metadata(&p.project_id, &p.connection_id).await;
                    Ok(())
                }
            },
        );
}

#[cfg(test)]
mod tests;
