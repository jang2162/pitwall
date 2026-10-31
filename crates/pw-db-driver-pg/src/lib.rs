//! PostgreSQL 드라이버(14-m3-plan.md M3-06): `tokio-postgres` + `deadpool-postgres`, TLS 는 `tokio-postgres-rustls` +
//! `rustls-platform-verifier`(결정 M3-D1 · M3-D10).
//!
//! - [`PgDriver`]: `pw_db_driver::DbDriver`. 데스크톱 크레이트의 등록부(`apps/desktop/src-tauri/src/db.rs`)가 넣는다.
//! - 연결: `sslmode` 는 libpq 뜻 그대로(`tls`), 터널이면 TCP 는 `connect_to`(`hostaddr`)로, TLS 이름은 설정의 DB `host` 로.
//!   연결 결과의 TLS 상태는 서버의 `pg_stat_ssl` 로 확인한다(`allow` · `prefer` 는 어느 쪽이든 될 수 있다).
//! - 연결 하나 = 그 연결을 쥔 태스크 하나(`session`): 포털 행 스트림 · 자동 커밋 / 수동 트랜잭션 · 커서 버리기.
//! - 값 변환(결정 M3-D8)은 `value`, 메타데이터 · 실행 계획은 `meta`, 취소는 [`PgInterrupt`](와이어 CancelRequest).
//!
//! **풀**: 설정(대상 · 사용자 · 비밀 · TLS · 연결 시간)마다 풀 하나. 세션 연결도 풀에서 꺼낸다. 닫을 때 서버가 트랜잭션 블록 밖이라고
//! 답한 쉬는 연결만 풀로 돌아가고, 다음에 꺼낼 때 `DISCARD ALL` 로 세션 상태를 모두 지운다(`RESET ALL` · `DEALLOCATE ALL` — 사람의
//! `PREPARE` 가 다음 탭에 넘어가지 않는다 · `DISCARD TEMP` …). `DISCARD ALL` 은 트랜잭션 블록 안에서 실패하므로 그래도 블록이 열린
//! 연결이 돌아왔으면 풀이 그 연결을 버린다. `DEALLOCATE ALL` 이 `tokio-postgres` 가 캐시한 타입 조회 문장도 지우므로 꺼낼 때마다
//! `clear_type_cache` 한다. 트랜잭션이 열린 채 닫힌 연결은 풀에서 떼어 닫는다(DB 가 롤백한다). 5분 넘게 쉰 연결은 다음 `connect` 때 닫는다. 연결 테스트는 풀을 쓰지 않는다 — 저장하지 않은
//! 초안의 비밀로 연 연결을 남기지 않는다.
//!
//! 실 DB 시험은 `#[cfg(pw_live_pg)]` 를 달고 함수 이름을 `live_pg_` 로 시작한다(`pw_db_driver::live` 머리 주석) — `src/live_tests.rs`.

mod control;
mod error;
#[cfg(all(test, pw_live_pg))]
mod live_tests;
mod meta;
mod session;
mod tls;
mod value;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use pw_db_driver::budget::within;
use pw_db_driver::tls::TlsAttempt;
use pw_db_driver::{
    BoxFuture, Connected, DbConnection, DbDriver, DbResult, Interrupt, SqlConnection, TlsPlan,
};
use pw_db_types::{
    BeginOptions, Capabilities, ColumnInfo, ConnectionSpec, DatabaseInfo, DbError, DbErrorKind,
    Ddl, DriverKind, ExecuteRequest, ExplainPlan, FetchLimit, ForeignKeyInfo, IndexInfo,
    ObjectPage, ObjectQuery, ObjectRef, RowBatch, SchemaInfo, ServerInfo, SqlDialect, SslMode,
    StatementStarted, Target, TransactionState,
};
use tokio::sync::{mpsc, oneshot};
use tokio_postgres::CancelToken;
use tokio_postgres::config::SslMode as PgSslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::error::{db_error, gone, invalid, pool_error};
use crate::meta::{MetaRequest, MetaResponse};
use crate::session::{Command, Reply, SavepointOp, Shared};

/// 풀 하나가 여는 연결 상한. 콘솔 탭 하나가 연결 하나를 쥔다.
const POOL_MAX: usize = 64;

/// 이만큼 쉰 풀 연결은 닫는다.
const IDLE_MAX: Duration = Duration::from_secs(300);

/// PostgreSQL 드라이버.
#[derive(Default)]
pub struct PgDriver {
    /// 풀 열쇠(설정 전부 — 비밀 포함, 메모리에만) → 풀.
    pools: Mutex<HashMap<String, Pool>>,
}

impl PgDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// 연결을 열고(시도 순서는 `sslmode`) 그 연결을 쥔 태스크를 띄운다.
    async fn open(&self, spec: &ConnectionSpec) -> DbResult<(PgConnection, ServerInfo)> {
        let plan = TlsPlan::new(&spec.tls, &spec.target, &SslMode::ALL)?;
        let tls = tls::connector(&plan, &spec.tls)?;
        let mut last = None;
        for &mode in attempts(plan.attempt) {
            let config = pg_config(spec, mode).await?;
            let pool = self.pool(spec, mode, &config, &tls)?;
            match pool.get().await.map_err(pool_error) {
                Ok(object) => {
                    object.clear_type_cache();
                    let (server, database) = match startup(&object, &plan).await {
                        Ok(found) => found,
                        Err(e) => {
                            drop(deadpool_postgres::Object::take(object));
                            return Err(e);
                        }
                    };
                    let shared = Arc::new(Shared::new(database));
                    let interrupt = Arc::new(PgInterrupt {
                        token: object.cancel_token(),
                        tls: tls.clone(),
                        shared: shared.clone(),
                    });
                    let (commands, receiver) = mpsc::unbounded_channel();
                    tokio::spawn(session::run(object, receiver, shared.clone(), pool.clone()));
                    return Ok((
                        PgConnection {
                            commands,
                            shared,
                            interrupt,
                        },
                        server,
                    ));
                }
                Err(e) if retry(&e) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| invalid("no connection attempt")))
    }

    /// 설정과 시도마다의 풀. 오래 쉰 연결 · 빈 풀은 이때 치운다.
    fn pool(
        &self,
        spec: &ConnectionSpec,
        mode: PgSslMode,
        config: &tokio_postgres::Config,
        tls: &MakeRustlsConnect,
    ) -> DbResult<Pool> {
        let key = pool_key(spec, mode);
        let mut pools = self
            .pools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pools.retain(|k, pool| {
            pool.retain(|_, metrics| metrics.last_used() < IDLE_MAX);
            *k == key || pool.status().size > 0
        });
        if let Some(pool) = pools.get(&key) {
            return Ok(pool.clone());
        }
        let timeout = Some(Duration::from_millis(u64::from(spec.connect_timeout_ms)));
        let manager = Manager::from_config(
            config.clone(),
            tls.clone(),
            ManagerConfig {
                recycling_method: RecyclingMethod::Custom("DISCARD ALL".to_owned()),
            },
        );
        let pool = Pool::builder(manager)
            .max_size(POOL_MAX)
            .runtime(Runtime::Tokio1)
            .wait_timeout(timeout)
            .create_timeout(timeout)
            .recycle_timeout(timeout)
            .build()
            .map_err(|e| invalid(e.to_string()))?;
        pools.insert(key, pool.clone());
        Ok(pool)
    }
}

/// `sslmode` 의 시도 순서. `allow` 는 평문을 먼저, `prefer` 는 TLS 를 먼저(서버가 TLS 를 못 하면 `tokio-postgres` 가 평문으로).
fn attempts(attempt: TlsAttempt) -> &'static [PgSslMode] {
    match attempt {
        TlsAttempt::Never => &[PgSslMode::Disable],
        TlsAttempt::PlainFirst => &[PgSslMode::Disable, PgSslMode::Require],
        TlsAttempt::TlsFirst => &[PgSslMode::Prefer, PgSslMode::Disable],
        TlsAttempt::Required => &[PgSslMode::Require],
    }
}

/// 다음 시도로 넘어가는 실패 — 서버가 그 방식을 거절했다(pg_hba · 인증 · TLS). 닿지 못했거나 시간이 넘은 것은 다시 해도 같다.
fn retry(error: &DbError) -> bool {
    matches!(
        error.kind,
        DbErrorKind::Tls | DbErrorKind::Auth | DbErrorKind::Server
    )
}

fn pool_key(spec: &ConnectionSpec, mode: PgSslMode) -> String {
    // `Debug` 는 비밀을 가리므로 비밀은 따로 붙인다. 이 열쇠는 로그 · 오류로 나가지 않는다.
    format!(
        "{:?}\u{0}{:?}\u{0}{:?}\u{0}{:?}\u{0}{:?}\u{0}{}\u{0}{:?}",
        spec.target,
        spec.database,
        spec.user,
        spec.tls,
        mode,
        spec.connect_timeout_ms,
        spec.password.as_ref().map(|p| p.expose()),
    )
}

/// 드라이버 설정 → `tokio-postgres` 설정. 터널이면 TCP 는 `connect_to`(`hostaddr`), TLS 이름은 설정의 `host`.
async fn pg_config(spec: &ConnectionSpec, mode: PgSslMode) -> DbResult<tokio_postgres::Config> {
    let Target::Network {
        host,
        port,
        connect_to,
    } = &spec.target
    else {
        return Err(invalid("postgres connects over the network, not to a file"));
    };
    let mut config = tokio_postgres::Config::new();
    config
        .host(host)
        .ssl_mode(mode)
        .application_name("Pitwall")
        .connect_timeout(Duration::from_millis(u64::from(spec.connect_timeout_ms)))
        .keepalives(true);
    match connect_to {
        Some(socket) => {
            let address = match socket.host.parse::<IpAddr>() {
                Ok(ip) => ip,
                Err(_) => tokio::net::lookup_host((socket.host.as_str(), socket.port))
                    .await
                    .ok()
                    .and_then(|mut found| found.next())
                    .map(|a| a.ip())
                    .ok_or_else(|| {
                        DbError::new(
                            DbErrorKind::Connect,
                            format!("cannot resolve the tunnel address {}", socket.host),
                        )
                    })?,
            };
            config.hostaddr(address).port(socket.port);
        }
        None => {
            config.port(*port);
        }
    }
    if let Some(database) = &spec.database {
        config.dbname(database);
    }
    if let Some(user) = &spec.user {
        config.user(user);
    }
    if let Some(password) = &spec.password {
        config.password(password.expose());
    }
    Ok(config)
}

/// 붙은 뒤 한 번: 서버 판본 · 연결한 DB · 이 연결이 TLS 인지.
async fn startup(
    client: &tokio_postgres::Client,
    plan: &TlsPlan,
) -> DbResult<(ServerInfo, String)> {
    let row = client
        .query_one(
            "SELECT pg_catalog.current_setting('server_version'), pg_catalog.current_database()::text, \
                    COALESCE((SELECT s.ssl FROM pg_catalog.pg_stat_ssl s WHERE s.pid = pg_catalog.pg_backend_pid()), false)",
            &[],
        )
        .await
        .map_err(|e| db_error(&e))?;
    let version: String = row.try_get(0).map_err(|e| db_error(&e))?;
    let database: String = row.try_get(1).map_err(|e| db_error(&e))?;
    let ssl: bool = row.try_get(2).map_err(|e| db_error(&e))?;
    Ok((
        ServerInfo {
            version: Some(version),
            tls: plan.status(ssl),
        },
        database,
    ))
}

impl DbDriver for PgDriver {
    fn id(&self) -> &str {
        DriverKind::Postgres.id()
    }

    fn dialect(&self) -> Option<SqlDialect> {
        Some(SqlDialect::Postgres)
    }

    fn capabilities(&self) -> Capabilities {
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

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        Box::pin(async move {
            let limit = Duration::from_millis(u64::from(spec.connect_timeout_ms));
            let (connection, server) = within(limit, "connect", self.open(&spec)).await?;
            Ok(Connected {
                connection: DbConnection::Sql(Box::new(connection)),
                server,
            })
        })
    }

    /// 풀을 쓰지 않고 붙어 보고 닫는다.
    fn test(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<ServerInfo>> {
        Box::pin(async move {
            let limit = Duration::from_millis(u64::from(spec.connect_timeout_ms));
            within(limit, "connect", test_connection(&spec)).await
        })
    }
}

async fn test_connection(spec: &ConnectionSpec) -> DbResult<ServerInfo> {
    let plan = TlsPlan::new(&spec.tls, &spec.target, &SslMode::ALL)?;
    let tls = tls::connector(&plan, &spec.tls)?;
    let mut last = None;
    for &mode in attempts(plan.attempt) {
        let config = pg_config(spec, mode).await?;
        match config.connect(tls.clone()).await {
            Ok((client, connection)) => {
                let task = tokio::spawn(connection);
                let found = startup(&client, &plan).await;
                drop(client);
                task.abort();
                return found.map(|(server, _)| server);
            }
            Err(e) => {
                let e = db_error(&e);
                if !retry(&e) {
                    return Err(e);
                }
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| invalid("no connection attempt")))
}

/// 진행 중 실행을 서버에서 멈춘다 — 새 연결로 CancelRequest(같은 TLS 설정). 도는 요청이 없으면 보내지 않는다.
pub struct PgInterrupt {
    token: CancelToken,
    tls: MakeRustlsConnect,
    shared: Arc<Shared>,
}

impl Interrupt for PgInterrupt {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            if !self.shared.running.load(Ordering::SeqCst) {
                return Ok(());
            }
            // 메타데이터가 풀의 다른 연결에서 돌고 있으면 그 연결을 멈춘다.
            let token = self
                .shared
                .cancel_elsewhere()
                .unwrap_or_else(|| self.token.clone());
            token
                .cancel_query(self.tls.clone())
                .await
                .map_err(|e| db_error(&e))
        })
    }
}

/// 연 연결. 메서드는 연결을 쥔 태스크에 명령을 보내고 답을 기다린다.
pub struct PgConnection {
    commands: mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
    interrupt: Arc<PgInterrupt>,
}

impl PgConnection {
    fn call<T: Send + 'static>(
        &self,
        command: impl FnOnce(Reply<T>) -> Command,
    ) -> BoxFuture<'static, DbResult<T>> {
        let (reply, answer) = oneshot::channel();
        let sent = self.commands.send(command(reply));
        Box::pin(async move {
            sent.map_err(|_| gone())?;
            answer.await.unwrap_or_else(|_| Err(gone()))
        })
    }

    fn meta(&self, request: MetaRequest) -> BoxFuture<'static, DbResult<MetaResponse>> {
        self.call(|reply| Command::Meta(request, reply))
    }

    /// 시험: 서버의 텍스트 출력 되돌려 받기를 일부러 실패시킨다.
    /// 시험: 트랜잭션 문장 거르개를 건너뛴다(거르개가 놓친 경우의 겹방어를 본다).
    #[cfg(all(test, pw_live_pg))]
    pub(crate) fn skip_statement_filter(&self, skip: bool) {
        self.shared.skip_filter.store(skip, Ordering::SeqCst);
    }

    #[cfg(all(test, pw_live_pg))]
    pub(crate) fn fail_text_output(&self, fail: bool) {
        self.shared.text_fault.store(fail, Ordering::SeqCst);
    }
}

fn unexpected<T>() -> DbResult<T> {
    Err(invalid("unexpected metadata response"))
}

impl SqlConnection for PgConnection {
    fn interrupter(&self) -> Arc<dyn Interrupt> {
        self.interrupt.clone()
    }

    fn transaction_state(&self) -> TransactionState {
        self.shared.state()
    }

    fn read_transaction_state(&mut self) -> BoxFuture<'_, DbResult<TransactionState>> {
        self.call(Command::State)
    }

    fn execute(&mut self, request: ExecuteRequest) -> BoxFuture<'_, DbResult<StatementStarted>> {
        self.call(|reply| Command::Execute(request, reply))
    }

    fn fetch(&mut self, limit: FetchLimit) -> BoxFuture<'_, DbResult<RowBatch>> {
        self.call(|reply| Command::Fetch(limit, reply))
    }

    fn discard_cursor(&mut self) -> BoxFuture<'_, DbResult<()>> {
        self.call(Command::Discard)
    }

    fn begin(&mut self, options: BeginOptions) -> BoxFuture<'_, DbResult<()>> {
        self.call(|reply| Command::Begin(options, reply))
    }

    fn commit(&mut self) -> BoxFuture<'_, DbResult<()>> {
        self.call(Command::Commit)
    }

    fn rollback(&mut self) -> BoxFuture<'_, DbResult<()>> {
        self.call(Command::Rollback)
    }

    fn savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        self.call(|reply| Command::Savepoint(SavepointOp::Create, name, reply))
    }

    fn rollback_to_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        self.call(|reply| Command::Savepoint(SavepointOp::RollbackTo, name, reply))
    }

    fn release_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        self.call(|reply| Command::Savepoint(SavepointOp::Release, name, reply))
    }

    fn explain(&mut self, sql: String) -> BoxFuture<'_, DbResult<ExplainPlan>> {
        let answer = self.meta(MetaRequest::Explain(sql));
        Box::pin(async move {
            match answer.await? {
                MetaResponse::Explain(plan) => Ok(plan),
                _ => unexpected(),
            }
        })
    }

    fn databases(&mut self) -> BoxFuture<'_, DbResult<Vec<DatabaseInfo>>> {
        let answer = self.meta(MetaRequest::Databases);
        Box::pin(async move {
            match answer.await? {
                MetaResponse::Databases(list) => Ok(list),
                _ => unexpected(),
            }
        })
    }

    fn schemas(&mut self, database: Option<String>) -> BoxFuture<'_, DbResult<Vec<SchemaInfo>>> {
        let answer = self.meta(MetaRequest::Schemas(database));
        Box::pin(async move {
            match answer.await? {
                MetaResponse::Schemas(list) => Ok(list),
                _ => unexpected(),
            }
        })
    }

    fn objects(&mut self, query: ObjectQuery) -> BoxFuture<'_, DbResult<ObjectPage>> {
        let answer = self.meta(MetaRequest::Objects(query));
        Box::pin(async move {
            match answer.await? {
                MetaResponse::Objects(page) => Ok(page),
                _ => unexpected(),
            }
        })
    }

    fn columns(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ColumnInfo>>> {
        let answer = self.meta(MetaRequest::Columns(object));
        Box::pin(async move {
            match answer.await? {
                MetaResponse::Columns(list) => Ok(list),
                _ => unexpected(),
            }
        })
    }

    fn indexes(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<IndexInfo>>> {
        let answer = self.meta(MetaRequest::Indexes(object));
        Box::pin(async move {
            match answer.await? {
                MetaResponse::Indexes(list) => Ok(list),
                _ => unexpected(),
            }
        })
    }

    fn foreign_keys(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ForeignKeyInfo>>> {
        let answer = self.meta(MetaRequest::ForeignKeys(object));
        Box::pin(async move {
            match answer.await? {
                MetaResponse::ForeignKeys(list) => Ok(list),
                _ => unexpected(),
            }
        })
    }

    fn ddl(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Ddl>> {
        let answer = self.meta(MetaRequest::Ddl(object));
        Box::pin(async move {
            match answer.await? {
                MetaResponse::Ddl(ddl) => Ok(ddl),
                _ => unexpected(),
            }
        })
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, DbResult<()>> {
        let answer = self.call(Command::Close);
        Box::pin(async move {
            // 태스크가 이미 끝났으면(끊긴 연결) 닫을 것이 없다.
            match answer.await {
                Err(e) if e.kind == DbErrorKind::Disconnected => Ok(()),
                other => other,
            }
        })
    }
}

#[cfg(test)]
mod tests;
