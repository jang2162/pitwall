//! 가짜 SQL 드라이버 — 실행 파이프라인(M3-05) · 스키마 캐시(M3-10) · 그리드 시험이 실 DB 없이 쓴다.
//!
//! 문장마다 응답([`FakeResponse`] — 열 · 행 수 · 지연 · 오류 · 멈춤)을 정하고, 불린 메서드를 연결 번호와 함께 차례대로 기록한다
//! ([`FakeCall`], [`FakeDriver::calls_of`] — 번호는 `connect` 순서대로 0 부터). 트랜잭션 · 인터럽트는 **연결마다 따로**다.
//! 트랜잭션은 작은 모형이다: 등급([`ExecuteRequest::grade`])이 조회가 아닌 문장을 「쓰기」 로 보고, 문장이 끝날 때(행이 없으면
//! 곧바로, 행이 있으면 커서를 끝까지 읽었거나 끝까지 읽어 버렸을 때) 트랜잭션 밖이면 곧바로, 안이면 `commit` 때
//! [`FakeDriver::committed`](드라이버 전체)에 넣는다. `rollback` · 세이브포인트 되돌리기 · 취소된 문장의 쓰기는 버린다.
//! 커서 버리기는 등급을 따른다 — 조회는 서버에서 멈추고(남은 행을 읽지 않는다), 조회가 아니면 남은 행을 끝까지 읽어 버려 쓰기가
//! 남는다(MySQL 의 길, 14-m3-plan.md M3-07). 읽은 행 수([`FakeDriver::rows_read`])로 「소비자가 credit 을 주지 않으면 드라이버가
//! 더 읽지 않는다」 를 시험한다.
//!
//! KV 갈래(Redis)의 가짜는 [`kv`] 다(M3-09).
//!
//! ```
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! use pw_db_driver::fake::{FakeCall, FakeDriver, FakeResponse};
//! use pw_db_driver::{DbConnection, DbDriver};
//! use pw_db_types::{ExecuteRequest, FetchLimit, RiskGrade};
//!
//! let driver = FakeDriver::new();
//! driver.respond("SELECT", FakeResponse::rows(3));
//! let DbConnection::Sql(mut conn) = driver.connect(FakeDriver::spec()).await.unwrap().connection else {
//!     unreachable!()
//! };
//! conn.execute(ExecuteRequest::new("SELECT * FROM t", RiskGrade::Read)).await.unwrap();
//! let batch = conn.fetch(FetchLimit::rows(10)).await.unwrap();
//! assert_eq!(batch.rows.len(), 3);
//! assert!(driver.calls_of(0).contains(&FakeCall::Execute("SELECT * FROM t".into())));
//! # });
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use pw_db_types::{
    BeginOptions, Capabilities, CellLimit, ColumnInfo, ColumnMeta, ConnectionSpec, DbError,
    DbErrorKind, DbValue, Ddl, DriverKind, ExecuteRequest, ExplainPlan, FetchLimit, ForeignKeyInfo,
    IndexInfo, ObjectInfo, ObjectPage, ObjectQuery, ObjectRef, PlanFormat, RiskGrade, Row,
    RowBatch, SchemaInfo, ServerInfo, SqlDialect, SslMode, StatementEnd, StatementStarted, Target,
    TlsSettings, TlsStatus, TransactionState,
};
use tokio::sync::Notify;

use crate::driver::{
    BoxFuture, Connected, DbConnection, DbDriver, DbResult, Interrupt, SqlConnection,
};

pub mod kv;

/// 가짜 드라이버가 기록하는 호출.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeCall {
    Connect,
    Execute(String),
    Fetch {
        max_rows: u32,
    },
    /// 커서 버리기. `drained` 면 남은 행을 끝까지 읽어 버렸다(조회가 아닌 문장 — 쓰기가 남는다), 아니면 서버에서 멈췄다(조회).
    /// 열린 커서가 없었으면 `drained: false`.
    DiscardCursor {
        drained: bool,
    },
    /// 인터럽트. 그 연결에서 도는 `fetch` 가 없으면 기록만 하고 아무것도 하지 않는다.
    Interrupt,
    Begin {
        read_only: bool,
    },
    Commit,
    Rollback,
    Savepoint(String),
    RollbackToSavepoint(String),
    ReleaseSavepoint(String),
    Explain(String),
    /// 메타데이터 메서드(`objects` · `columns` …)의 이름.
    Metadata(&'static str),
    /// 서버에 트랜잭션 상태를 물었다([`SqlConnection::read_transaction_state`]).
    ReadTransactionState,
    Close,
}

/// 문장이 열린 트랜잭션을 저절로 끝내는 방식(드라이버가 보고하는 상태 변화 — 실행 파이프라인이 세션 상태로 옮긴다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeEnding {
    /// 문장 앞에서 열린 트랜잭션을 커밋한다(MySQL 의 DDL · `LOCK TABLES` 등 암묵 커밋 — 문장이 실패해도 커밋은 남는다).
    ImplicitCommit,
    /// 트랜잭션 전체를 되돌린다(교착 등). 문장이 오류로 끝나면 그 오류에 `rolled_back` 을 싣는다.
    RollBack,
    /// 트랜잭션 전체를 되돌렸지만 오류에 `rolled_back` 을 싣지 않는다 — 드라이버가 그 오류 코드를 모른다(MySQL 의 3101 등). 끝난
    /// 것은 트랜잭션 상태로만 보인다.
    RollBackSilently,
    /// 자동 커밋이었는데 서버에 트랜잭션 블록이 열렸다(PG 의 숨은 블록 — M3-06 리뷰 H1). 연결은 그 블록을 쥐고 `InTransaction` 이다.
    OpenBlock,
}

/// 문장 하나에 대한 응답.
#[derive(Debug, Clone)]
pub struct FakeResponse {
    /// 결과 열. 비어 있으면 행을 돌려주지 않는 문장이다(쓰기인지는 요청의 등급이 정한다).
    pub columns: Vec<ColumnMeta>,
    /// 만들어 낼 행 수. 행 `i` 의 첫 열은 정수 `i`, 나머지 열은 텍스트 `r{i}c{열}`.
    pub rows: u64,
    /// 쓰기의 바뀐 행 수.
    pub rows_affected: Option<u64>,
    /// `execute` 가 돌려줄 오류.
    pub error: Option<DbError>,
    /// 이만큼 행을 내준 뒤의 `fetch` 가 돌려줄 오류.
    pub fail_after: Option<(u64, DbError)>,
    /// `fetch` 한 번의 지연. 인터럽트가 오면 곧바로 취소 오류다.
    pub fetch_delay: Duration,
    /// 첫 `fetch` 가 인터럽트를 받을 때까지 돌아오지 않는다(오래 도는 쿼리).
    pub hang: bool,
    /// 문장이 열린 트랜잭션을 저절로 끝내거나 연다([`FakeEnding`]). `execute` 가 오류를 돌려주기 전에 적용한다.
    pub ending: Option<FakeEnding>,
}

impl FakeResponse {
    /// `id int8, name text` 두 열의 `rows` 행.
    pub fn rows(rows: u64) -> Self {
        Self {
            columns: vec![column("id", "int8"), column("name", "text")],
            rows,
            rows_affected: None,
            error: None,
            fail_after: None,
            fetch_delay: Duration::ZERO,
            hang: false,
            ending: None,
        }
    }

    /// 행이 없는 문장(`affected` 행이 바뀜). 등급이 조회가 아니면 쓰기로 기록된다.
    pub fn write(affected: u64) -> Self {
        Self {
            columns: Vec::new(),
            rows: 0,
            rows_affected: Some(affected),
            ..Self::rows(0)
        }
    }

    /// `execute` 가 이 오류로 끝난다.
    pub fn error(error: DbError) -> Self {
        Self {
            error: Some(error),
            ..Self::rows(0)
        }
    }

    pub fn with_columns(mut self, columns: Vec<ColumnMeta>) -> Self {
        self.columns = columns;
        self
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.fetch_delay = delay;
        self
    }

    pub fn failing_after(mut self, rows: u64, error: DbError) -> Self {
        self.fail_after = Some((rows, error));
        self
    }

    pub fn hanging(mut self) -> Self {
        self.hang = true;
        self
    }

    pub fn ending(mut self, ending: FakeEnding) -> Self {
        self.ending = Some(ending);
        self
    }
}

/// 테이블 하나의 메타데이터.
#[derive(Debug, Clone, Default)]
pub struct FakeTable {
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub ddl: String,
}

pub fn column(name: &str, db_type: &str) -> ColumnMeta {
    ColumnMeta {
        name: name.to_owned(),
        db_type: db_type.to_owned(),
        nullable: Some(true),
    }
}

#[derive(Default)]
struct Script {
    /// (SQL 에 든 글, 응답). 뒤에 더한 것이 먼저 맞는다.
    responses: Vec<(String, FakeResponse)>,
    connect_error: Option<DbError>,
    /// `read_transaction_state` 가 돌려줄 오류(서버에 묻지 못했다).
    state_read_error: Option<DbError>,
    /// `commit` 이 돌려줄 오류(트랜잭션 기록은 그대로 둔다 — 끊긴 연결의 COMMIT 처럼 결과를 모른다).
    commit_error: Option<DbError>,
    objects: Vec<ObjectInfo>,
    tables: HashMap<String, FakeTable>,
}

#[derive(Default)]
struct Ledger {
    in_transaction: bool,
    failed: bool,
    /// 트랜잭션 안에서 아직 커밋하지 않은 쓰기.
    pending: Vec<String>,
    /// (이름, 그때의 `pending` 길이).
    savepoints: Vec<(String, usize)>,
}

/// 드라이버 전체가 함께 쓰는 것: 응답 · 호출 기록 · 커밋된 쓰기 · 읽은 행 수.
struct State {
    id: String,
    dialect: SqlDialect,
    capabilities: Capabilities,
    /// 트랜잭션 안에서 도는 `fetch` 를 인터럽트하면 트랜잭션 전체가 롤백된다(SQLite 의 쓰기 취소). 아니면 `Failed`(PG).
    interrupt_rolls_back: AtomicBool,
    script: Mutex<Script>,
    /// (연결 번호, 호출).
    calls: Mutex<Vec<(u32, FakeCall)>>,
    committed: Mutex<Vec<String>>,
    rows_read: AtomicU64,
    next_connection: AtomicU32,
    /// 연결 번호 → 그 연결의 것(시험이 실행 밖에서 트랜잭션을 끝낼 때 — [`FakeDriver::end_transaction_behind`]).
    sessions: Mutex<HashMap<u32, Arc<Session>>>,
}

impl State {
    fn record(&self, connection: u32, call: FakeCall) {
        lock(&self.calls).push((connection, call));
    }
}

/// 연결 하나의 것: 트랜잭션과 인터럽트. 그 연결의 [`FakeInterrupt`] 와만 함께 쓴다.
struct Session {
    id: u32,
    ledger: Mutex<Ledger>,
    /// 지금 `fetch` 가 기다리는 중이다. 인터럽트는 이때만 듣는다.
    in_flight: AtomicBool,
    /// 도는 `fetch` 에 인터럽트가 왔다.
    interrupted: AtomicBool,
    wake: Notify,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 가짜 SQL 드라이버. 복제하면 같은 기록을 본다.
#[derive(Clone)]
pub struct FakeDriver {
    state: Arc<State>,
}

impl Default for FakeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeDriver {
    /// 모든 SQL 기능을 지원하는 가짜. 응답을 정하지 않은 문장은 `FakeResponse::rows(0)`.
    pub fn new() -> Self {
        Self::with_capabilities(Self::full_capabilities())
    }

    pub fn with_capabilities(capabilities: Capabilities) -> Self {
        Self::build("fake", SqlDialect::Postgres, capabilities)
    }

    /// 등록부의 id(설정 파일의 `driver` 값 — `postgres` · `mysql` · `sqlite`)와 방언을 고른 가짜. 실행 관문이 연결의 방언으로 분류하는지
    /// 보는 시험이 쓴다.
    pub fn named(id: &str, dialect: SqlDialect) -> Self {
        Self::build(id, dialect, Self::full_capabilities())
    }

    fn build(id: &str, dialect: SqlDialect, capabilities: Capabilities) -> Self {
        Self {
            state: Arc::new(State {
                id: id.to_owned(),
                dialect,
                capabilities,
                interrupt_rolls_back: AtomicBool::new(false),
                script: Mutex::default(),
                calls: Mutex::default(),
                committed: Mutex::default(),
                rows_read: AtomicU64::new(0),
                next_connection: AtomicU32::new(0),
                sessions: Mutex::default(),
            }),
        }
    }

    pub fn full_capabilities() -> Capabilities {
        Capabilities {
            sql: true,
            kv: false,
            server_cursor: true,
            cancel: true,
            transactions: true,
            savepoints: true,
            read_only_transaction: true,
            explain: true,
            metadata: true,
            schemas: true,
            multiple_databases: true,
            ssl_modes: SslMode::ALL.to_vec(),
        }
    }

    /// 가짜에 넘길 연결 설정(아무 값이나 받는다).
    pub fn spec() -> ConnectionSpec {
        let mut spec = ConnectionSpec::new(
            DriverKind::Postgres.id(),
            Target::Network {
                host: "fake".into(),
                port: 5432,
                connect_to: None,
            },
        );
        spec.tls = TlsSettings::mode(SslMode::Disable);
        spec
    }

    /// SQL 에 `needle` 이 든 문장의 응답. 나중에 정한 것이 먼저 맞는다.
    pub fn respond(&self, needle: &str, response: FakeResponse) {
        lock(&self.state.script)
            .responses
            .push((needle.to_owned(), response));
    }

    /// 트랜잭션 안에서 도는 `fetch` 를 인터럽트하면 트랜잭션 전체를 롤백하고 `rolled_back` 취소 오류로 답한다(SQLite 의 길). 기본은
    /// PG 의 길 — 트랜잭션이 `Failed` 가 된다.
    pub fn interrupt_rolls_back(&self, yes: bool) {
        self.state.interrupt_rolls_back.store(yes, Ordering::SeqCst);
    }

    /// `connect` 가 이 오류로 끝난다.
    pub fn fail_connect(&self, error: DbError) {
        lock(&self.state.script).connect_error = Some(error);
    }

    /// `read_transaction_state` 가 이 오류로 끝난다(`None` 이면 다시 답한다) — 서버에 상태를 묻지 못한 경우.
    pub fn fail_state_reads(&self, error: Option<DbError>) {
        lock(&self.state.script).state_read_error = error;
    }

    /// `commit` 이 이 오류로 끝난다(`None` 이면 다시 커밋한다). 트랜잭션 기록은 건드리지 않는다.
    pub fn fail_commit(&self, error: Option<DbError>) {
        lock(&self.state.script).commit_error = error;
    }

    /// 연결 하나(`connect` 순서대로 0 부터)의 열린 트랜잭션을 파이프라인 모르게 되돌린다 — 정리 예산을 넘겨 버려진 문장이 나중에
    /// 끝나며 트랜잭션을 롤백한 경우(SQLite 쓰기 인터럽트). 드라이버의 `transaction_state` 도 곧바로 `Idle` 이다.
    pub fn end_transaction_behind(&self, connection: u32) {
        if let Some(session) = lock(&self.state.sessions).get(&connection) {
            *lock(&session.ledger) = Ledger::default();
        }
    }

    /// `objects` 가 쪽으로 나눠 돌려줄 객체 목록.
    pub fn set_objects(&self, objects: Vec<ObjectInfo>) {
        lock(&self.state.script).objects = objects;
    }

    /// 이름이 `name` 인 테이블의 열 · 인덱스 · FK · DDL.
    pub fn set_table(&self, name: &str, table: FakeTable) {
        lock(&self.state.script)
            .tables
            .insert(name.to_owned(), table);
    }

    /// 지금까지 불린 메서드(모든 연결).
    pub fn calls(&self) -> Vec<FakeCall> {
        lock(&self.state.calls)
            .iter()
            .map(|(_, c)| c.clone())
            .collect()
    }

    /// 연결 하나(`connect` 순서대로 0 부터)가 부른 메서드.
    pub fn calls_of(&self, connection: u32) -> Vec<FakeCall> {
        lock(&self.state.calls)
            .iter()
            .filter(|(id, _)| *id == connection)
            .map(|(_, c)| c.clone())
            .collect()
    }

    /// `call` 과 같은 호출의 수(모든 연결).
    pub fn count(&self, call: &FakeCall) -> usize {
        lock(&self.state.calls)
            .iter()
            .filter(|(_, c)| c == call)
            .count()
    }

    /// 커서에서 지금까지 만들어 낸 행 수(모든 연결, 끝까지 읽어 버린 행 포함).
    pub fn rows_read(&self) -> u64 {
        self.state.rows_read.load(Ordering::SeqCst)
    }

    /// 커밋된 쓰기(문장 글)의 순서(모든 연결).
    pub fn committed(&self) -> Vec<String> {
        lock(&self.state.committed).clone()
    }

    fn response_for(&self, sql: &str) -> FakeResponse {
        let script = lock(&self.state.script);
        script
            .responses
            .iter()
            .rev()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| FakeResponse::rows(0))
    }
}

impl DbDriver for FakeDriver {
    fn id(&self) -> &str {
        &self.state.id
    }

    fn dialect(&self) -> Option<SqlDialect> {
        Some(self.state.dialect)
    }

    fn capabilities(&self) -> Capabilities {
        self.state.capabilities.clone()
    }

    fn connect(&self, _spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        Box::pin(async move {
            let id = self.state.next_connection.fetch_add(1, Ordering::SeqCst);
            self.state.record(id, FakeCall::Connect);
            if let Some(error) = lock(&self.state.script).connect_error.clone() {
                return Err(error);
            }
            let session = Arc::new(Session {
                id,
                ledger: Mutex::default(),
                in_flight: AtomicBool::new(false),
                interrupted: AtomicBool::new(false),
                wake: Notify::new(),
            });
            lock(&self.state.sessions).insert(id, Arc::clone(&session));
            let connection = FakeConnection {
                driver: self.clone(),
                session,
                cursor: None,
            };
            Ok(Connected {
                connection: DbConnection::Sql(Box::new(connection)),
                server: ServerInfo {
                    version: Some("fake 1.0".into()),
                    tls: TlsStatus::Plaintext,
                },
            })
        })
    }
}

struct Cursor {
    response: FakeResponse,
    next: u64,
    cell_limit: CellLimit,
    sql: String,
    grade: RiskGrade,
}

struct FakeConnection {
    driver: FakeDriver,
    session: Arc<Session>,
    cursor: Option<Cursor>,
}

impl FakeConnection {
    fn record(&self, call: FakeCall) {
        self.driver.state.record(self.session.id, call);
    }

    /// 쓰기 하나가 끝났다: 트랜잭션 밖이면 곧바로 커밋, 안이면 그 트랜잭션의 대기 목록에.
    fn land_write(&self, sql: &str) {
        let mut ledger = lock(&self.session.ledger);
        if ledger.in_transaction {
            ledger.pending.push(sql.to_owned());
        } else {
            lock(&self.driver.state.committed).push(sql.to_owned());
        }
    }

    /// 커서의 문장이 끝까지 돌았다. 조회가 아니었으면 그 쓰기가 남는다.
    fn finish_cursor(&mut self) {
        if let Some(cursor) = self.cursor.take()
            && !cursor.grade.is_read()
        {
            self.land_write(&cursor.sql);
        }
    }

    /// 도는 `fetch` 에 인터럽트가 왔으면 취소 오류. 트랜잭션 안이었으면 그 트랜잭션은 롤백만 된다(PG 와 같다) — 또는
    /// [`FakeDriver::interrupt_rolls_back`] 이면 통째로 롤백되고 오류가 `rolled_back` 을 싣는다(SQLite 와 같다).
    fn take_interrupt(&self) -> DbResult<()> {
        if self.session.interrupted.swap(false, Ordering::SeqCst) {
            if self
                .driver
                .state
                .interrupt_rolls_back
                .load(Ordering::SeqCst)
            {
                let mut ledger = lock(&self.session.ledger);
                if ledger.in_transaction {
                    *ledger = Ledger::default();
                    return Err(DbError::cancelled().with_rolled_back());
                }
                return Err(DbError::cancelled());
            }
            self.fail_transaction();
            return Err(DbError::cancelled());
        }
        Ok(())
    }

    /// 응답의 [`FakeEnding`] 을 트랜잭션에 적용한다. 되돌렸으면 true.
    fn apply_ending(&self, ending: Option<FakeEnding>) -> bool {
        let mut ledger = lock(&self.session.ledger);
        match ending {
            None => false,
            Some(FakeEnding::ImplicitCommit) => {
                if ledger.in_transaction {
                    let pending = std::mem::take(&mut ledger.pending);
                    lock(&self.driver.state.committed).extend(pending);
                    *ledger = Ledger::default();
                }
                false
            }
            Some(FakeEnding::RollBack) => {
                let was = ledger.in_transaction;
                *ledger = Ledger::default();
                was
            }
            Some(FakeEnding::RollBackSilently) => {
                *ledger = Ledger::default();
                false
            }
            Some(FakeEnding::OpenBlock) => {
                if !ledger.in_transaction {
                    ledger.in_transaction = true;
                    ledger.failed = false;
                }
                false
            }
        }
    }

    fn fail_transaction(&self) {
        let mut ledger = lock(&self.session.ledger);
        if ledger.in_transaction {
            ledger.failed = true;
        }
    }

    /// `duration` 동안(또는 영영) 기다리되 인터럽트가 오면 곧바로 취소 오류.
    async fn wait(&self, duration: Option<Duration>) -> DbResult<()> {
        let session = &self.session;
        session.in_flight.store(true, Ordering::SeqCst);
        let mut notified = std::pin::pin!(session.wake.notified());
        notified.as_mut().enable();
        let result = async {
            self.take_interrupt()?;
            match duration {
                Some(d) => {
                    tokio::select! {
                        () = notified => {}
                        () = tokio::time::sleep(d) => return Ok(()),
                    }
                }
                None => notified.await,
            }
            self.take_interrupt()
        }
        .await;
        session.in_flight.store(false, Ordering::SeqCst);
        session.interrupted.store(false, Ordering::SeqCst);
        result
    }
}

fn row(index: u64, columns: usize, limit: CellLimit) -> Row {
    (0..columns)
        .map(|c| {
            if c == 0 {
                DbValue::int(index)
            } else {
                DbValue::text(format!("r{index}c{c}"), limit)
            }
        })
        .collect()
}

impl SqlConnection for FakeConnection {
    fn interrupter(&self) -> Arc<dyn Interrupt> {
        Arc::new(FakeInterrupt {
            driver: self.driver.clone(),
            session: Arc::clone(&self.session),
        })
    }

    fn transaction_state(&self) -> TransactionState {
        let ledger = lock(&self.session.ledger);
        match (ledger.in_transaction, ledger.failed) {
            (false, _) => TransactionState::Idle,
            (true, false) => TransactionState::InTransaction,
            (true, true) => TransactionState::Failed,
        }
    }

    fn read_transaction_state(&mut self) -> BoxFuture<'_, DbResult<TransactionState>> {
        Box::pin(async move {
            self.record(FakeCall::ReadTransactionState);
            if let Some(error) = lock(&self.driver.state.script).state_read_error.clone() {
                return Err(error);
            }
            Ok(self.transaction_state())
        })
    }

    fn execute(&mut self, request: ExecuteRequest) -> BoxFuture<'_, DbResult<StatementStarted>> {
        Box::pin(async move {
            self.cursor = None;
            self.record(FakeCall::Execute(request.sql.clone()));
            let response = self.driver.response_for(&request.sql);
            if lock(&self.session.ledger).failed {
                return Err(
                    DbError::new(DbErrorKind::Server, "current transaction is aborted")
                        .with_code("25P02"),
                );
            }
            let rolled_back = self.apply_ending(response.ending);
            if let Some(mut error) = response.error.clone() {
                let mut ledger = lock(&self.session.ledger);
                if ledger.in_transaction {
                    ledger.failed = true;
                }
                if rolled_back {
                    error.rolled_back = true;
                }
                return Err(error);
            }
            if response.columns.is_empty() {
                if !request.grade.is_read() {
                    self.land_write(&request.sql);
                }
                return Ok(StatementStarted {
                    columns: Vec::new(),
                    end: Some(StatementEnd {
                        rows_affected: response.rows_affected,
                    }),
                });
            }
            let columns = response.columns.clone();
            self.cursor = Some(Cursor {
                response,
                next: 0,
                cell_limit: request.cell_limit,
                sql: request.sql,
                grade: request.grade,
            });
            Ok(StatementStarted { columns, end: None })
        })
    }

    fn fetch(&mut self, limit: FetchLimit) -> BoxFuture<'_, DbResult<RowBatch>> {
        Box::pin(async move {
            self.record(FakeCall::Fetch {
                max_rows: limit.max_rows,
            });
            let Some(cursor) = self.cursor.as_ref() else {
                return Err(DbError::new(DbErrorKind::Invalid, "no open cursor"));
            };
            let (hang, delay) = (cursor.response.hang, cursor.response.fetch_delay);
            let waited = if hang {
                self.wait(None).await
            } else if !delay.is_zero() {
                self.wait(Some(delay)).await
            } else {
                Ok(())
            };
            // 취소된 문장은 커서째 버린다 — 그 쓰기도 남지 않는다.
            waited.inspect_err(|_| self.cursor = None)?;
            let cursor = self.cursor.as_mut().expect("checked above");
            let columns = cursor.response.columns.len();
            let mut rows = Vec::new();
            let mut bytes = 0_usize;
            while cursor.next < cursor.response.rows
                && rows.len() < limit.max_rows as usize
                && bytes < limit.max_bytes as usize
            {
                if let Some((after, error)) = &cursor.response.fail_after
                    && cursor.next >= *after
                {
                    let error = error.clone();
                    self.cursor = None;
                    self.fail_transaction();
                    return Err(error);
                }
                let r = row(cursor.next, columns, cursor.cell_limit);
                bytes += r.iter().map(approximate_size).sum::<usize>();
                rows.push(r);
                cursor.next += 1;
                self.driver.state.rows_read.fetch_add(1, Ordering::SeqCst);
            }
            let end = (cursor.next >= cursor.response.rows).then_some(StatementEnd {
                rows_affected: Some(cursor.next),
            });
            if end.is_some() {
                self.finish_cursor();
            }
            Ok(RowBatch { rows, end })
        })
    }

    fn discard_cursor(&mut self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            let Some(cursor) = self.cursor.as_mut() else {
                self.record(FakeCall::DiscardCursor { drained: false });
                return Ok(());
            };
            if cursor.grade.is_read() {
                // 조회: 서버에서 멈춘다 — 남은 행을 만들지 않는다.
                self.cursor = None;
                self.record(FakeCall::DiscardCursor { drained: false });
            } else {
                // 조회가 아님: 멈추면 쓰기가 되돌려지므로 남은 행을 끝까지 읽어 버린다.
                let remaining = cursor.response.rows - cursor.next;
                cursor.next = cursor.response.rows;
                self.driver
                    .state
                    .rows_read
                    .fetch_add(remaining, Ordering::SeqCst);
                self.finish_cursor();
                self.record(FakeCall::DiscardCursor { drained: true });
            }
            Ok(())
        })
    }

    fn begin(&mut self, options: BeginOptions) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.record(FakeCall::Begin {
                read_only: options.read_only,
            });
            let mut ledger = lock(&self.session.ledger);
            if ledger.in_transaction {
                return Err(DbError::new(
                    DbErrorKind::Invalid,
                    "already in a transaction",
                ));
            }
            ledger.in_transaction = true;
            ledger.failed = false;
            Ok(())
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.record(FakeCall::Commit);
            if let Some(error) = lock(&self.driver.state.script).commit_error.clone() {
                return Err(error);
            }
            let mut ledger = lock(&self.session.ledger);
            let pending = std::mem::take(&mut ledger.pending);
            // 실패한 트랜잭션의 COMMIT 은 롤백이다(PG 와 같다) — trait 의 약속대로 `rolled_back` 오류로 답한다.
            let failed = ledger.failed;
            if !failed {
                lock(&self.driver.state.committed).extend(pending);
            }
            ledger.in_transaction = false;
            ledger.failed = false;
            ledger.savepoints.clear();
            if failed {
                return Err(DbError::new(
                    DbErrorKind::Server,
                    "the transaction had failed, so COMMIT rolled it back",
                )
                .with_code("25P02")
                .with_rolled_back());
            }
            Ok(())
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.record(FakeCall::Rollback);
            let mut ledger = lock(&self.session.ledger);
            ledger.pending.clear();
            ledger.in_transaction = false;
            ledger.failed = false;
            ledger.savepoints.clear();
            Ok(())
        })
    }

    fn savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.record(FakeCall::Savepoint(name.clone()));
            let mut ledger = lock(&self.session.ledger);
            if !ledger.in_transaction {
                return Err(DbError::new(DbErrorKind::Invalid, "no transaction"));
            }
            let at = ledger.pending.len();
            ledger.savepoints.push((name, at));
            Ok(())
        })
    }

    fn rollback_to_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.record(FakeCall::RollbackToSavepoint(name.clone()));
            let mut ledger = lock(&self.session.ledger);
            let Some(index) = ledger.savepoints.iter().rposition(|(n, _)| *n == name) else {
                return Err(DbError::new(DbErrorKind::Invalid, "no such savepoint"));
            };
            let at = ledger.savepoints[index].1;
            ledger.savepoints.truncate(index + 1);
            ledger.pending.truncate(at);
            ledger.failed = false;
            Ok(())
        })
    }

    fn release_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.record(FakeCall::ReleaseSavepoint(name.clone()));
            let mut ledger = lock(&self.session.ledger);
            let Some(index) = ledger.savepoints.iter().rposition(|(n, _)| *n == name) else {
                return Err(DbError::new(DbErrorKind::Invalid, "no such savepoint"));
            };
            ledger.savepoints.truncate(index);
            Ok(())
        })
    }

    fn explain(&mut self, sql: String) -> BoxFuture<'_, DbResult<ExplainPlan>> {
        Box::pin(async move {
            self.record(FakeCall::Explain(sql));
            Ok(ExplainPlan {
                format: PlanFormat::Json,
                text: r#"[{"Plan":{"Node Type":"Seq Scan"}}]"#.to_owned(),
            })
        })
    }

    fn schemas(&mut self, _database: Option<String>) -> BoxFuture<'_, DbResult<Vec<SchemaInfo>>> {
        Box::pin(async move {
            self.record(FakeCall::Metadata("schemas"));
            Ok(vec![SchemaInfo {
                name: "public".into(),
            }])
        })
    }

    fn objects(&mut self, query: ObjectQuery) -> BoxFuture<'_, DbResult<ObjectPage>> {
        Box::pin(async move {
            self.record(FakeCall::Metadata("objects"));
            let script = lock(&self.driver.state.script);
            let filter = query.filter.as_deref().map(str::to_lowercase);
            let matching: Vec<&ObjectInfo> = script
                .objects
                .iter()
                .filter(|o| o.kind == query.kind)
                .filter(|o| {
                    filter
                        .as_deref()
                        .is_none_or(|f| o.name.to_lowercase().contains(f))
                })
                .collect();
            let start: usize = match query.cursor.as_deref() {
                None => 0,
                Some(c) => c
                    .parse()
                    .map_err(|_| DbError::new(DbErrorKind::Invalid, "bad cursor"))?,
            };
            let end = (start + query.limit as usize).min(matching.len());
            let items = matching
                .get(start..end)
                .unwrap_or_default()
                .iter()
                .map(|o| (*o).clone())
                .collect();
            Ok(ObjectPage {
                items,
                next_cursor: (end < matching.len()).then(|| end.to_string()),
            })
        })
    }

    fn columns(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ColumnInfo>>> {
        Box::pin(async move {
            self.record(FakeCall::Metadata("columns"));
            Ok(self.table(&object).columns)
        })
    }

    fn indexes(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<IndexInfo>>> {
        Box::pin(async move {
            self.record(FakeCall::Metadata("indexes"));
            Ok(self.table(&object).indexes)
        })
    }

    fn foreign_keys(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ForeignKeyInfo>>> {
        Box::pin(async move {
            self.record(FakeCall::Metadata("foreignKeys"));
            Ok(self.table(&object).foreign_keys)
        })
    }

    fn ddl(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Ddl>> {
        Box::pin(async move {
            self.record(FakeCall::Metadata("ddl"));
            Ok(Ddl {
                text: self.table(&object).ddl,
            })
        })
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, DbResult<()>> {
        Box::pin(async move {
            self.record(FakeCall::Close);
            Ok(())
        })
    }
}

impl FakeConnection {
    fn table(&self, object: &ObjectRef) -> FakeTable {
        lock(&self.driver.state.script)
            .tables
            .get(&object.name)
            .cloned()
            .unwrap_or_default()
    }
}

/// 값의 대략 크기(묶음의 바이트 상한을 재는 데만 쓴다).
fn approximate_size(value: &DbValue) -> usize {
    match value {
        DbValue::Text(s) | DbValue::Decimal(s) | DbValue::Json(s) | DbValue::Datetime(s) => s.len(),
        _ => 8,
    }
}

struct FakeInterrupt {
    driver: FakeDriver,
    session: Arc<Session>,
}

impl Interrupt for FakeInterrupt {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            let session = &self.session;
            self.driver.state.record(session.id, FakeCall::Interrupt);
            // 도는 `fetch` 가 없으면 아무것도 하지 않는다(`Interrupt` 의 계약).
            if session.in_flight.load(Ordering::SeqCst) {
                session.interrupted.store(true, Ordering::SeqCst);
                session.wake.notify_waiters();
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
