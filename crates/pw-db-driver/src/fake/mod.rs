//! 가짜 SQL 드라이버 — 실행 파이프라인(M3-05) · 스키마 캐시(M3-10) · 그리드 시험이 실 DB 없이 쓴다.
//!
//! 문장마다 응답([`FakeResponse`] — 열 · 행 수 · 지연 · 오류 · 멈춤)을 정하고, 불린 메서드를 차례대로 기록한다([`FakeCall`]).
//! 트랜잭션은 작은 모형이다: 행을 돌려주지 않는 문장을 「쓰기」 로 보고 트랜잭션 밖이면 곧바로, 안이면 `commit` 때
//! [`FakeDriver::committed`] 에 넣는다(`rollback` · 세이브포인트 되돌리기는 버린다). 읽은 행 수([`FakeDriver::rows_read`])로
//! 「소비자가 credit 을 주지 않으면 드라이버가 더 읽지 않는다」 를 시험한다.
//!
//! ```
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! use pw_db_driver::fake::{FakeCall, FakeDriver, FakeResponse};
//! use pw_db_driver::{DbConnection, DbDriver};
//! use pw_db_types::{ExecuteRequest, FetchLimit};
//!
//! let driver = FakeDriver::new();
//! driver.respond("SELECT", FakeResponse::rows(3));
//! let DbConnection::Sql(mut conn) = driver.connect(FakeDriver::spec()).await.unwrap().connection else {
//!     unreachable!()
//! };
//! conn.execute(ExecuteRequest::sql("SELECT * FROM t")).await.unwrap();
//! let batch = conn.fetch(FetchLimit::rows(10)).await.unwrap();
//! assert_eq!(batch.rows.len(), 3);
//! assert!(driver.calls().contains(&FakeCall::Execute("SELECT * FROM t".into())));
//! # });
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use pw_db_types::{
    BeginOptions, Capabilities, CellLimit, ColumnInfo, ColumnMeta, ConnectionSpec, DbError,
    DbErrorKind, DbValue, Ddl, DriverKind, ExecuteRequest, ExplainPlan, FetchLimit, ForeignKeyInfo,
    IndexInfo, ObjectInfo, ObjectPage, ObjectQuery, ObjectRef, PlanFormat, Row, RowBatch,
    SchemaInfo, ServerInfo, SqlDialect, SslMode, StatementEnd, StatementStarted, Target,
    TlsSettings, TlsStatus, TransactionState,
};
use tokio::sync::Notify;

use crate::driver::{
    BoxFuture, Connected, DbConnection, DbDriver, DbResult, Interrupt, SqlConnection,
};

/// 가짜 드라이버가 기록하는 호출.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeCall {
    Connect,
    Execute(String),
    Fetch {
        max_rows: u32,
    },
    DiscardCursor,
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
    Close,
}

/// 문장 하나에 대한 응답.
#[derive(Debug, Clone)]
pub struct FakeResponse {
    /// 결과 열. 비어 있으면 행을 돌려주지 않는 문장(쓰기)이다.
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
        }
    }

    /// 행이 없는 쓰기(`affected` 행이 바뀜).
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
    committed: Vec<String>,
}

struct State {
    capabilities: Capabilities,
    script: Mutex<Script>,
    calls: Mutex<Vec<FakeCall>>,
    ledger: Mutex<Ledger>,
    rows_read: AtomicU64,
    /// 인터럽트가 와서 아직 소비되지 않았다. 다음 `execute` 가 지운다.
    interrupted: AtomicBool,
    wake: Notify,
}

impl State {
    fn record(&self, call: FakeCall) {
        lock(&self.calls).push(call);
    }
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
        Self {
            state: Arc::new(State {
                capabilities,
                script: Mutex::default(),
                calls: Mutex::default(),
                ledger: Mutex::default(),
                rows_read: AtomicU64::new(0),
                interrupted: AtomicBool::new(false),
                wake: Notify::new(),
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
        ConnectionSpec {
            driver: DriverKind::Postgres.id().to_owned(),
            target: Target::Network {
                host: "fake".into(),
                port: 5432,
                connect_to: None,
            },
            database: None,
            user: None,
            password: None,
            tls: TlsSettings::mode(SslMode::Disable),
            read_only: false,
            connect_timeout_ms: 10_000,
            response_timeout_ms: None,
        }
    }

    /// SQL 에 `needle` 이 든 문장의 응답. 나중에 정한 것이 먼저 맞는다.
    pub fn respond(&self, needle: &str, response: FakeResponse) {
        lock(&self.state.script)
            .responses
            .push((needle.to_owned(), response));
    }

    /// `connect` 가 이 오류로 끝난다.
    pub fn fail_connect(&self, error: DbError) {
        lock(&self.state.script).connect_error = Some(error);
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

    /// 지금까지 불린 메서드.
    pub fn calls(&self) -> Vec<FakeCall> {
        lock(&self.state.calls).clone()
    }

    /// `call` 과 같은 호출의 수.
    pub fn count(&self, call: &FakeCall) -> usize {
        lock(&self.state.calls)
            .iter()
            .filter(|c| *c == call)
            .count()
    }

    /// 커서에서 지금까지 만들어 낸 행 수.
    pub fn rows_read(&self) -> u64 {
        self.state.rows_read.load(Ordering::SeqCst)
    }

    /// 커밋된 쓰기(문장 글)의 순서.
    pub fn committed(&self) -> Vec<String> {
        lock(&self.state.ledger).committed.clone()
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
        "fake"
    }

    fn dialect(&self) -> Option<SqlDialect> {
        Some(SqlDialect::Postgres)
    }

    fn capabilities(&self) -> Capabilities {
        self.state.capabilities.clone()
    }

    fn connect(&self, _spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        Box::pin(async move {
            self.state.record(FakeCall::Connect);
            if let Some(error) = lock(&self.state.script).connect_error.clone() {
                return Err(error);
            }
            let connection = FakeConnection {
                driver: self.clone(),
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
}

struct FakeConnection {
    driver: FakeDriver,
    cursor: Option<Cursor>,
}

impl FakeConnection {
    fn state(&self) -> &State {
        &self.driver.state
    }

    /// 인터럽트가 오면 취소 오류. 트랜잭션 안이었으면 그 트랜잭션은 롤백만 된다(PG 와 같다).
    fn take_interrupt(&self) -> DbResult<()> {
        if self.state().interrupted.swap(false, Ordering::SeqCst) {
            self.fail_transaction();
            return Err(DbError::cancelled());
        }
        Ok(())
    }

    fn fail_transaction(&self) {
        let mut ledger = lock(&self.state().ledger);
        if ledger.in_transaction {
            ledger.failed = true;
        }
    }

    /// `duration` 동안(또는 영영) 기다리되 인터럽트가 오면 곧바로 취소 오류.
    async fn wait(&self, duration: Option<Duration>) -> DbResult<()> {
        let state = self.state();
        let mut notified = std::pin::pin!(state.wake.notified());
        notified.as_mut().enable();
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
        })
    }

    fn transaction_state(&self) -> TransactionState {
        let ledger = lock(&self.state().ledger);
        match (ledger.in_transaction, ledger.failed) {
            (false, _) => TransactionState::Idle,
            (true, false) => TransactionState::InTransaction,
            (true, true) => TransactionState::Failed,
        }
    }

    fn execute(&mut self, request: ExecuteRequest) -> BoxFuture<'_, DbResult<StatementStarted>> {
        Box::pin(async move {
            self.cursor = None;
            self.state().interrupted.store(false, Ordering::SeqCst);
            self.state().record(FakeCall::Execute(request.sql.clone()));
            let response = self.driver.response_for(&request.sql);
            {
                let mut ledger = lock(&self.state().ledger);
                if ledger.failed {
                    return Err(DbError::new(
                        DbErrorKind::Server,
                        "current transaction is aborted",
                    )
                    .with_code("25P02"));
                }
                if let Some(error) = response.error.clone() {
                    if ledger.in_transaction {
                        ledger.failed = true;
                    }
                    return Err(error);
                }
                if response.columns.is_empty() {
                    if ledger.in_transaction {
                        ledger.pending.push(request.sql.clone());
                    } else {
                        ledger.committed.push(request.sql.clone());
                    }
                    return Ok(StatementStarted {
                        columns: Vec::new(),
                        end: Some(StatementEnd {
                            rows_affected: response.rows_affected,
                        }),
                    });
                }
            }
            let columns = response.columns.clone();
            self.cursor = Some(Cursor {
                response,
                next: 0,
                cell_limit: request.cell_limit,
            });
            Ok(StatementStarted { columns, end: None })
        })
    }

    fn fetch(&mut self, limit: FetchLimit) -> BoxFuture<'_, DbResult<RowBatch>> {
        Box::pin(async move {
            self.state().record(FakeCall::Fetch {
                max_rows: limit.max_rows,
            });
            let Some(cursor) = self.cursor.as_ref() else {
                return Err(DbError::new(DbErrorKind::Invalid, "no open cursor"));
            };
            let (hang, delay) = (cursor.response.hang, cursor.response.fetch_delay);
            if hang {
                self.wait(None).await.inspect_err(|_| self.cursor = None)?;
            } else if !delay.is_zero() {
                self.wait(Some(delay))
                    .await
                    .inspect_err(|_| self.cursor = None)?;
            } else {
                self.take_interrupt().inspect_err(|_| self.cursor = None)?;
            }
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
                self.cursor = None;
            }
            Ok(RowBatch { rows, end })
        })
    }

    fn discard_cursor(&mut self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.state().record(FakeCall::DiscardCursor);
            self.cursor = None;
            Ok(())
        })
    }

    fn begin(&mut self, options: BeginOptions) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.state().record(FakeCall::Begin {
                read_only: options.read_only,
            });
            let mut ledger = lock(&self.state().ledger);
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
            self.state().record(FakeCall::Commit);
            let mut ledger = lock(&self.state().ledger);
            let pending = std::mem::take(&mut ledger.pending);
            // 실패한 트랜잭션의 COMMIT 은 롤백이다(PG 와 같다).
            if !ledger.failed {
                ledger.committed.extend(pending);
            }
            ledger.in_transaction = false;
            ledger.failed = false;
            ledger.savepoints.clear();
            Ok(())
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.state().record(FakeCall::Rollback);
            let mut ledger = lock(&self.state().ledger);
            ledger.pending.clear();
            ledger.in_transaction = false;
            ledger.failed = false;
            ledger.savepoints.clear();
            Ok(())
        })
    }

    fn savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            self.state().record(FakeCall::Savepoint(name.clone()));
            let mut ledger = lock(&self.state().ledger);
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
            self.state()
                .record(FakeCall::RollbackToSavepoint(name.clone()));
            let mut ledger = lock(&self.state().ledger);
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
            self.state()
                .record(FakeCall::ReleaseSavepoint(name.clone()));
            let mut ledger = lock(&self.state().ledger);
            let Some(index) = ledger.savepoints.iter().rposition(|(n, _)| *n == name) else {
                return Err(DbError::new(DbErrorKind::Invalid, "no such savepoint"));
            };
            ledger.savepoints.truncate(index);
            Ok(())
        })
    }

    fn explain(&mut self, sql: String) -> BoxFuture<'_, DbResult<ExplainPlan>> {
        Box::pin(async move {
            self.state().record(FakeCall::Explain(sql));
            Ok(ExplainPlan {
                format: PlanFormat::Json,
                text: r#"[{"Plan":{"Node Type":"Seq Scan"}}]"#.to_owned(),
            })
        })
    }

    fn schemas(&mut self, _database: Option<String>) -> BoxFuture<'_, DbResult<Vec<SchemaInfo>>> {
        Box::pin(async move {
            self.state().record(FakeCall::Metadata("schemas"));
            Ok(vec![SchemaInfo {
                name: "public".into(),
            }])
        })
    }

    fn objects(&mut self, query: ObjectQuery) -> BoxFuture<'_, DbResult<ObjectPage>> {
        Box::pin(async move {
            self.state().record(FakeCall::Metadata("objects"));
            let script = lock(&self.state().script);
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
            self.state().record(FakeCall::Metadata("columns"));
            Ok(self.table(&object).columns)
        })
    }

    fn indexes(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<IndexInfo>>> {
        Box::pin(async move {
            self.state().record(FakeCall::Metadata("indexes"));
            Ok(self.table(&object).indexes)
        })
    }

    fn foreign_keys(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ForeignKeyInfo>>> {
        Box::pin(async move {
            self.state().record(FakeCall::Metadata("foreignKeys"));
            Ok(self.table(&object).foreign_keys)
        })
    }

    fn ddl(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Ddl>> {
        Box::pin(async move {
            self.state().record(FakeCall::Metadata("ddl"));
            Ok(Ddl {
                text: self.table(&object).ddl,
            })
        })
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, DbResult<()>> {
        Box::pin(async move {
            self.state().record(FakeCall::Close);
            Ok(())
        })
    }
}

impl FakeConnection {
    fn table(&self, object: &ObjectRef) -> FakeTable {
        lock(&self.state().script)
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
}

impl Interrupt for FakeInterrupt {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            let state = &self.driver.state;
            state.record(FakeCall::Interrupt);
            state.interrupted.store(true, Ordering::SeqCst);
            state.wake.notify_waiters();
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
