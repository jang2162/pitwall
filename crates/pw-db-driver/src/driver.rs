//! 드라이버 trait.
//!
//! [`DbDriver`] 가 연결을 열면 갈래 하나가 나온다 — 지원 플래그 `sql` 이면 [`SqlConnection`], `kv` 면 [`KvConnection`]. 연결
//! 하나는 한 번에 한 일만 한다(콘솔 탭 하나 = 고정 연결 하나, M3-05). 그래서 갈래 trait 의 메서드는 `&mut self` 이고, 다른
//! 태스크에서 진행 중 실행을 멈추는 것은 [`Interrupt`] 가 한다.
//!
//! 입력은 모두 소유 값이다 — 프로세스 밖 구현이 요청을 직렬화해 보낼 수 있게(JDBC 브리지 F6-3b · 외부 확장 M8).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use pw_db_types::{
    BeginOptions, Capabilities, ColumnInfo, ConnectionSpec, DatabaseInfo, DbError, Ddl,
    ExecuteRequest, ExplainPlan, FetchLimit, ForeignKeyInfo, IndexInfo, KvKey, KvKeyInfo, KvRange,
    KvRenameOutcome, KvReply, KvScanPage, KvScanRequest, KvValuePage, ObjectPage, ObjectQuery,
    ObjectRef, RowBatch, SchemaInfo, ServerInfo, SqlDialect, StatementStarted, TransactionState,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type DbResult<T> = Result<T, DbError>;

/// 드라이버 하나(PG · MySQL · SQLite · Redis, 또는 프로세스 밖 드라이버).
pub trait DbDriver: Send + Sync {
    /// 등록부의 키. 설정 파일의 `driver` 값과 같다(`pw_db_types::DriverKind::id`).
    fn id(&self) -> &str;

    /// SQL 갈래의 방언(위험도 분류 · 문장 나누기가 고른다). KV 드라이버는 `None`.
    fn dialect(&self) -> Option<SqlDialect>;

    /// 지원 플래그. 연결마다 바뀌지 않는다.
    fn capabilities(&self) -> Capabilities;

    /// 연결을 연다. 연결 타임아웃은 `spec.connect_timeout_ms` 다. 풀을 쓰는 드라이버는 풀에서 꺼낸 연결이고, 닫으면 돌려준다.
    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>>;

    /// 연결 테스트: 붙어 보고 서버 판본 · TLS 상태를 돌려준 뒤 닫는다. 저장하지 않은 초안으로도 부른다(M3-05).
    fn test(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<ServerInfo>> {
        Box::pin(async move {
            let Connected { connection, server } = self.connect(spec).await?;
            connection.close().await?;
            Ok(server)
        })
    }
}

/// 연 연결과 서버 정보.
pub struct Connected {
    pub connection: DbConnection,
    pub server: ServerInfo,
}

/// 갈래 하나.
pub enum DbConnection {
    Sql(Box<dyn SqlConnection>),
    Kv(Box<dyn KvConnection>),
}

impl DbConnection {
    /// 닫는다. 진행 중인 커서는 버린다.
    pub fn close(self) -> BoxFuture<'static, DbResult<()>> {
        match self {
            Self::Sql(c) => c.close(),
            Self::Kv(c) => c.close(),
        }
    }

    /// 지원 플래그의 갈래와 맞는지 — 등록부가 연 연결을 넘기기 전에 본다.
    pub fn matches(&self, capabilities: &Capabilities) -> bool {
        match self {
            Self::Sql(_) => capabilities.sql,
            Self::Kv(_) => capabilities.kv,
        }
    }
}

/// 진행 중 실행을 서버에서 멈춘다(PG CancelRequest · MySQL `KILL QUERY` · SQLite `interrupt` · Redis 요청 버리기).
///
/// 연결을 쥔 태스크 밖에서 부른다 — 실행 중인 `fetch` 는 취소 오류로 돌아온다. 연결은 닫지 않는다. 도는 실행이 없으면 아무것도
/// 하지 않는다.
pub trait Interrupt: Send + Sync {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>>;
}

/// SQL 갈래(지원 플래그 `sql`).
///
/// 실행은 문장 하나씩이다: [`execute`](Self::execute) 가 문장을 시작하고, 행이 있으면 커서가 열려 [`fetch`](Self::fetch) 로
/// 원하는 만큼만 읽는다(credit 백프레셔 — 파이프라인은 창이 받은 만큼만 `fetch` 한다). 그리드 보유 행 상한에 닿으면
/// [`discard_cursor`](Self::discard_cursor) 로 커서만 버린다 — **연결은 그대로, 세션의 트랜잭션도 그대로**다(결정 M3-D7).
/// 커서가 열린 채 다른 메서드를 부르면 드라이버가 먼저 커서를 버린다.
///
/// **자동 커밋**: [`transaction_state`](Self::transaction_state) 가 `Idle` 일 때 시작한 문장은 자동 커밋이다 — 드라이버가 커서를
/// 위해 암묵 트랜잭션이 필요하면(PG 포털 — M3-06) 스스로 열고, 문장이 끝나거나 커서를 버릴 때 커밋한다. 그동안에도
/// `transaction_state` 는 `Idle` 로 답한다(사람이 연 트랜잭션이 아니다).
pub trait SqlConnection: Send {
    /// 이 연결의 인터럽트. 여러 번 불러도 같은 연결을 멈춘다.
    fn interrupter(&self) -> Arc<dyn Interrupt>;

    /// 지금 트랜잭션 상태. 문장 · 트랜잭션 메서드가 끝날 때마다 드라이버가 갱신한다.
    fn transaction_state(&self) -> TransactionState;

    /// 문장 하나를 시작한다. `request.grade` 는 M3-03 의 분류기가 매긴 그 문장의 등급이다(커서 버리기가 쓴다).
    fn execute(&mut self, request: ExecuteRequest) -> BoxFuture<'_, DbResult<StatementStarted>>;

    /// 열린 커서에서 `limit` 만큼 읽는다. 커서가 다 읽히면 `end` 가 있고 커서는 닫힌다. 커서가 없으면 `Invalid`.
    fn fetch(&mut self, limit: FetchLimit) -> BoxFuture<'_, DbResult<RowBatch>>;

    /// 커서를 버린다. 방법은 그 커서를 연 [`ExecuteRequest::grade`] 가 정한다 — 조회면 남은 행을 받지 않고 서버에서 멈춰도
    /// 된다(PG 포털 `Close` · MySQL `KILL QUERY`). 조회가 아니면 문장을 멈추지 않는다 — 멈추면 그 쓰기가 되돌려지는 드라이버
    /// (MySQL · MariaDB `… RETURNING` 등)는 남은 행을 끝까지 읽어 버린다(14-m3-plan.md M3-07). 어느 쪽이든 연결과 세션의
    /// 트랜잭션은 그대로다. 커서가 없으면 아무것도 하지 않는다.
    fn discard_cursor(&mut self) -> BoxFuture<'_, DbResult<()>>;

    /// 트랜잭션을 연다(지원 플래그 `transactions`, 읽기 전용은 `readOnlyTransaction`).
    fn begin(&mut self, options: BeginOptions) -> BoxFuture<'_, DbResult<()>>;

    /// 트랜잭션을 끝낸다. **실패한 트랜잭션(`TransactionState::Failed`)이면** 드라이버는 그래도 트랜잭션을 끝내고(PG 는 `COMMIT` 에
    /// 롤백으로 답한다) `rolled_back` 이 켜진 오류로 답한다 — 롤백을 성공으로 보이지 않는다(결정 M3-D21). 커밋 자체가 실패하면(지연
    /// 제약 등) 서버가 트랜잭션 전체를 되돌렸으므로 그 오류도 `rolled_back` 이다. 실패 상태가 없는 드라이버(오류가 트랜잭션을 깨지
    /// 않는 SQLite · MySQL 등)에는 해당 드라이버만의 약속이다.
    fn commit(&mut self) -> BoxFuture<'_, DbResult<()>>;

    fn rollback(&mut self) -> BoxFuture<'_, DbResult<()>>;

    /// 세이브포인트(지원 플래그 `savepoints`). 이름은 파이프라인이 만든다(사람 입력이 아니다).
    fn savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        let _ = name;
        unsupported("savepoints")
    }

    fn rollback_to_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        let _ = name;
        unsupported("savepoints")
    }

    fn release_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        let _ = name;
        unsupported("savepoints")
    }

    /// 실행 계획(지원 플래그 `explain`). 문장의 위험도는 파이프라인이 먼저 본다(`EXPLAIN ANALYZE` 는 안의 문장 등급).
    fn explain(&mut self, sql: String) -> BoxFuture<'_, DbResult<ExplainPlan>> {
        let _ = sql;
        unsupported("explain")
    }

    /// DB 목록(지원 플래그 `multipleDatabases`).
    fn databases(&mut self) -> BoxFuture<'_, DbResult<Vec<DatabaseInfo>>> {
        unsupported("databases")
    }

    /// 스키마 목록(지원 플래그 `schemas`).
    fn schemas(&mut self, database: Option<String>) -> BoxFuture<'_, DbResult<Vec<SchemaInfo>>> {
        let _ = database;
        unsupported("schemas")
    }

    /// 객체 목록 한 쪽(지원 플래그 `metadata`). 필터와 쪽은 서버 쪽에서 한다.
    fn objects(&mut self, query: ObjectQuery) -> BoxFuture<'_, DbResult<ObjectPage>> {
        let _ = query;
        unsupported("metadata")
    }

    fn columns(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ColumnInfo>>> {
        let _ = object;
        unsupported("metadata")
    }

    fn indexes(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<IndexInfo>>> {
        let _ = object;
        unsupported("metadata")
    }

    fn foreign_keys(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ForeignKeyInfo>>> {
        let _ = object;
        unsupported("metadata")
    }

    fn ddl(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Ddl>> {
        let _ = object;
        unsupported("metadata")
    }

    /// 닫는다. 열린 트랜잭션은 DB 가 롤백한다 — 닫기 전에 묻는 것은 화면의 몫이다(결정 M3-D21).
    fn close(self: Box<Self>) -> BoxFuture<'static, DbResult<()>>;
}

/// 키-값 갈래(지원 플래그 `kv`, Redis — M3-09). 연결 하나는 DB 번호 하나다(결정 M3-D15).
///
/// 값 쓰기 전용 메서드는 없다 — 값을 바꾸는 것은 명령 콘솔의 [`command`](Self::command)(분류 · 확인을 거친다)다.
pub trait KvConnection: Send {
    fn interrupter(&self) -> Arc<dyn Interrupt>;

    /// 키 한 묶음과 묶음 안 키마다 타입 · TTL · 크기.
    fn scan(&mut self, request: KvScanRequest) -> BoxFuture<'_, DbResult<KvScanPage>>;

    /// 키 하나의 타입 · TTL · 크기를 다시 읽는다(값 뷰어가 TTL 을 고친 뒤 등). 키가 없으면 `None`.
    fn key_info(&mut self, key: KvKey) -> BoxFuture<'_, DbResult<Option<KvKeyInfo>>> {
        let _ = key;
        unsupported("key info")
    }

    /// 값 한 쪽.
    fn read(&mut self, key: KvKey, range: KvRange) -> BoxFuture<'_, DbResult<KvValuePage>>;

    /// TTL 을 바꾼다. 키가 없으면 `false`.
    fn set_ttl(&mut self, key: KvKey, ttl_ms: u64) -> BoxFuture<'_, DbResult<bool>>;

    /// TTL 을 지운다(만료 없음). 키가 없거나 TTL 이 없었으면 `false`.
    fn persist(&mut self, key: KvKey) -> BoxFuture<'_, DbResult<bool>>;

    /// 키를 지우고 지운 수를 돌려준다.
    fn delete(&mut self, keys: Vec<KvKey>) -> BoxFuture<'_, DbResult<u64>>;

    /// 이름을 바꾼다. `overwrite` 가 아니면 대상이 있을 때 바꾸지 않는다(`RENAMENX`).
    fn rename(
        &mut self,
        from: KvKey,
        to: KvKey,
        overwrite: bool,
    ) -> BoxFuture<'_, DbResult<KvRenameOutcome>>;

    /// 명령 하나. 분류(허용 목록 — M3-09)는 부르는 쪽이 먼저 했다.
    fn command(&mut self, argv: Vec<String>) -> BoxFuture<'_, DbResult<KvReply>>;

    fn close(self: Box<Self>) -> BoxFuture<'static, DbResult<()>>;
}

fn unsupported<'a, T: Send + 'a>(what: &'static str) -> BoxFuture<'a, DbResult<T>> {
    Box::pin(std::future::ready(Err(DbError::unsupported(what))))
}
