//! 연결 하나 = 그 연결을 쥔 태스크 하나([`run`]). trait 메서드는 명령을 보내고 답을 기다린다.
//!
//! **까닭**: 포털은 `tokio-postgres` 의 `Transaction`(연결을 빌린 값)으로만 만들고 읽는다. 사람이 연 트랜잭션은 여러 trait 호출에
//! 걸쳐 살아야 하므로, 연결을 쥔 태스크가 트랜잭션을 지역 변수로 들고 그 안에서 다음 명령을 받는다(`manual`).
//!
//! **행 스트림은 포털**(14-m3-plan.md M3-06): 행을 돌려주는 문장은 `Bind` 로 포털을 만들고 `fetch` 마다 `Execute`(최대 행 =
//! credit)로 그만큼만 읽는다 — 서버는 나머지를 보내지 않고 기다린다. 포털은 트랜잭션 안에서만 살므로 자동 커밋 탭은 실행마다
//! 트랜잭션을 열고 끝나면(커서를 버려도) `COMMIT` 한다(`idle_execute`) — 자동 커밋의 뜻이다(`INSERT … RETURNING` 이 상한을
//! 넘어도 쓰기는 남는다). 행을 돌려주지 않는 문장은 트랜잭션 없이 그대로 돈다(`VACUUM` 처럼 트랜잭션 블록에서 못 도는 문장).
//! **커서 버리기**는 포털을 닫는 것(`Close`)이고 트랜잭션은 그대로다. 조회가 아닌 문장의 포털은 닫기 전에 남은 행을 끝까지 읽어
//! 버린다(문장을 멈추지 않는다 — `SqlConnection::discard_cursor` 의 약속).
//!
//! **상태**: 수동 트랜잭션에서 서버 오류 · 취소가 나면 `Failed`(롤백만 된다). 자동 커밋의 포털 트랜잭션은 오류면 롤백하고 `Idle`
//! 이다 — 사람이 연 트랜잭션이 아니다. `Failed` 에서의 `commit` 은 `COMMIT` 을 보내 트랜잭션을 끝내고(서버는 롤백으로 답한다)
//! `rolled_back` 오류로 답한다 — 성공으로 보이지 않는다(결정 M3-D21).
//!
//! **숨은 트랜잭션 블록**(M3-06 리뷰 H1): 트랜잭션 문장은 거르개(`control`)가 막지만, 거르개가 놓쳐 자동 커밋 탭의 문장이 서버에
//! 블록을 열었으면 그 연결은 그 탭에 묶는다 — 행을 돌려주지 않는 자동 커밋 문장 뒤마다 서버에 블록 안인지 묻고([`block_state`]),
//! 안이면 탭의 상태를 「트랜잭션 열림」 으로 바꿔 사람의 커밋 · 롤백을 기다린다(몰래 커밋도 몰래 롤백도 하지 않는다). 닫을 때도
//! 묻고, 블록 안이거나 물을 수 없으면 풀에 돌려주지 않고 닫는다. 풀의 재활용 문장(`DISCARD ALL`)은 블록 안에서 실패하므로 그래도
//! 새면 풀이 그 연결을 버린다(`lib.rs`).
//!
//! **메타데이터**(M3-06 리뷰 M1): 사람의 트랜잭션이 열려 있으면 메타데이터 질의는 풀의 다른 연결로 돈다 — 그 질의의 실패가 사람의
//! 트랜잭션을 `Failed` 로 만들지 않게. 실행 계획은 사람의 트랜잭션이 보는 것을 봐야 하므로 같은 연결에서 세이브포인트로 감싼다.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use deadpool_postgres::{Object, Pool};
use futures_util::StreamExt;
use pw_db_driver::DbResult;
use pw_db_types::{
    BeginOptions, CellLimit, ColumnMeta, DbError, ExecuteRequest, FetchLimit, Row, RowBatch,
    StatementEnd, StatementStarted, TransactionState,
};
use tokio::sync::{mpsc, oneshot};
use tokio_postgres::{CancelToken, Client, Portal, Statement, Transaction};

use crate::control;
use crate::error::{db_error, invalid, pool_error};
use crate::meta::{self, MetaRequest, MetaResponse};
use crate::value::{self, Raw, TextRequests};

pub(crate) type Reply<T> = oneshot::Sender<DbResult<T>>;
pub(crate) type Commands = mpsc::UnboundedReceiver<Command>;

pub(crate) enum Command {
    Execute(ExecuteRequest, Reply<StatementStarted>),
    Fetch(FetchLimit, Reply<RowBatch>),
    Discard(Reply<()>),
    Begin(BeginOptions, Reply<()>),
    Commit(Reply<()>),
    Rollback(Reply<()>),
    Savepoint(SavepointOp, String, Reply<()>),
    Meta(MetaRequest, Reply<MetaResponse>),
    /// 서버의 트랜잭션 블록 상태를 묻는다([`block_state`]).
    State(Reply<TransactionState>),
    Close(Reply<()>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SavepointOp {
    Create,
    RollbackTo,
    Release,
}

impl SavepointOp {
    fn sql(self, name: &str) -> String {
        let name = quote_ident(name);
        match self {
            Self::Create => format!("SAVEPOINT {name}"),
            Self::RollbackTo => format!("ROLLBACK TO SAVEPOINT {name}"),
            Self::Release => format!("RELEASE SAVEPOINT {name}"),
        }
    }
}

pub(crate) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// 태스크와 trait 쪽이 함께 보는 것.
pub(crate) struct Shared {
    state: Mutex<TransactionState>,
    /// 서버에서 도는 요청이 있다 — 인터럽트는 이때만 CancelRequest 를 보낸다(쉬는 연결에 보내면 다음 문장이 멈출 수 있다).
    pub(crate) running: AtomicBool,
    /// 시험: 서버의 텍스트 출력 되돌려 받기를 일부러 실패시킨다(`value::text_outputs`).
    pub(crate) text_fault: AtomicBool,
    /// 시험: 트랜잭션 문장 거르개를 건너뛴다 — 거르개가 놓친 경우의 겹방어([`block_state`])를 시험한다.
    pub(crate) skip_filter: AtomicBool,
    /// 지금 도는 요청이 이 연결이 아니라 풀의 다른 연결(메타데이터)에 있으면 그 연결의 취소 토큰.
    cancel_elsewhere: Mutex<Option<CancelToken>>,
    /// 연결한 DB(`current_database()`) — 메타데이터가 다른 DB 를 묻으면 거절한다.
    pub(crate) database: String,
}

impl Shared {
    pub(crate) fn new(database: String) -> Self {
        Self {
            state: Mutex::new(TransactionState::Idle),
            running: AtomicBool::new(false),
            text_fault: AtomicBool::new(false),
            skip_filter: AtomicBool::new(false),
            cancel_elsewhere: Mutex::new(None),
            database,
        }
    }

    pub(crate) fn state(&self) -> TransactionState {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set(&self, state: TransactionState) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    /// 인터럽트가 보낼 곳: 다른 연결에서 도는 요청이 있으면 그 토큰.
    pub(crate) fn cancel_elsewhere(&self) -> Option<CancelToken> {
        self.cancel_elsewhere
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_cancel_elsewhere(&self, token: Option<CancelToken>) {
        *self
            .cancel_elsewhere
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = token;
    }

    /// 서버 요청 하나를 「도는 중」 으로 표시하고 기다린다.
    pub(crate) async fn busy<T>(&self, call: impl Future<Output = DbResult<T>>) -> DbResult<T> {
        self.running.store(true, Ordering::SeqCst);
        let result = call.await;
        self.running.store(false, Ordering::SeqCst);
        result
    }
}

fn pg<T>(result: Result<T, tokio_postgres::Error>) -> DbResult<T> {
    result.map_err(|e| db_error(&e))
}

fn send<T>(reply: Reply<T>, result: DbResult<T>) {
    // 부른 쪽이 기다림을 버렸으면(취소 정리 시간 초과 등) 답할 곳이 없다 — 연결은 다음 명령을 받는다.
    let _ = reply.send(result);
}

/// 수동 트랜잭션에서 이 오류가 트랜잭션을 실패시키는가 — 서버가 받은 요청의 오류(SQLSTATE 가 있다)만 그렇다.
fn aborts(error: &DbError) -> bool {
    error.code.is_some()
}

/// 연결을 쥐고 명령을 받는다. 닫히면(또는 trait 쪽이 사라지면) 쉬는 연결은 풀에 돌려주고, 트랜잭션이 열린 연결 · 블록 여부를
/// 확인하지 못한 연결은 풀에서 떼어 닫는다 — DB 가 그 트랜잭션을 롤백한다.
pub(crate) async fn run(
    mut object: Object,
    mut commands: Commands,
    shared: Arc<Shared>,
    pool: Pool,
) {
    let mut pending: Option<Command> = None;
    // 닫기의 답은 연결을 풀에 돌려주거나 떼어 닫은 **뒤에** 보낸다 — 부른 쪽이 곧바로 다음 탭을 열면 같은 연결을 받는다.
    let mut closing: Option<Reply<()>> = None;
    let detach = loop {
        let Some(command) = next(&mut pending, &mut commands).await else {
            break !closes_clean(&object).await;
        };
        let client: &mut Client = &mut object;
        match command {
            Command::Close(reply) => {
                closing = Some(reply);
                break !closes_clean(client).await;
            }
            Command::Execute(request, reply) => {
                match idle_execute(client, request, reply, &mut commands, &shared).await {
                    IdleNext::Done => {}
                    IdleNext::Pending(command) => pending = Some(command),
                    IdleNext::Block => {
                        // 거르개가 놓친 문장이 블록을 열었다. 그 블록을 사람의 트랜잭션으로 쥔다 — `BEGIN` 은 블록 안이라
                        // 서버가 경고(25001)만 내고 아무것도 바꾸지 않는다.
                        // 블록이 이미 실패했으면(롤백만 된다) 포털이 필요 없으니 연결로 바로 쥔다.
                        if shared.state() == TransactionState::Failed {
                            if let Some(reply) = hold_failed(client, &mut commands, &shared).await {
                                closing = reply;
                                break true;
                            }
                            continue;
                        }
                        let Ok(tx) = client.transaction().await else {
                            // 블록을 쥘 수 없다 — 풀에 돌려주지 않고 닫는다(DB 가 롤백한다). 다음 명령은 끊김으로 답한다.
                            break true;
                        };
                        if let Some(reply) = hold(tx, &mut commands, &shared, &pool).await {
                            closing = reply;
                            break true;
                        }
                    }
                }
            }
            Command::Begin(options, reply) => {
                let started = shared
                    .busy(async {
                        pg(client
                            .build_transaction()
                            .read_only(options.read_only)
                            .start()
                            .await)
                    })
                    .await;
                let tx = match started {
                    Ok(tx) => tx,
                    Err(e) => {
                        send(reply, Err(e));
                        continue;
                    }
                };
                shared.set(TransactionState::InTransaction);
                send(reply, Ok(()));
                if let Some(reply) = hold(tx, &mut commands, &shared, &pool).await {
                    closing = reply;
                    break true;
                }
            }
            Command::Fetch(_, reply) => send(reply, Err(invalid("no open cursor"))),
            Command::Discard(reply) => send(reply, Ok(())),
            Command::Commit(reply) | Command::Rollback(reply) => {
                send(reply, Err(invalid("no transaction is open")));
            }
            Command::Savepoint(_, _, reply) => {
                send(reply, Err(invalid("savepoints need an open transaction")));
            }
            Command::Meta(request, reply) => {
                let result = shared.busy(meta::run(client, request, &shared)).await;
                send(reply, result);
            }
            Command::State(reply) => send(reply, shared.busy(block_state(client)).await),
        }
    };
    shared.set(TransactionState::Idle);
    if detach {
        drop(Object::take(object));
    } else {
        drop(object);
    }
    if let Some(reply) = closing {
        send(reply, Ok(()));
    }
}

/// 쉬는 연결을 풀에 돌려줘도 되는가 — 서버가 트랜잭션 블록 밖이라고 답할 때만.
async fn closes_clean(client: &Client) -> bool {
    matches!(block_state(client).await, Ok(TransactionState::Idle))
}

/// 서버의 트랜잭션 블록 상태. `set_config(…, is_local => true)` 는 블록 밖이면 그 문장의 암묵 트랜잭션과 함께 사라지고 블록
/// 안에서만 다음 문장까지 남는다 — 두 문장으로 그것을 본다. 오류 없이 답하므로 서버 로그에 오류가 쌓이지 않는다(블록 안에서만
/// 실패하는 문장(`SAVEPOINT`)을 쓰면 자동 커밋 문장마다 서버 로그에 ERROR 가 남는다). 실패한 블록이면 첫 문장이 25P02 다. 블록
/// 안이면 이 설정은 그 블록이 끝날 때 사라진다.
pub(crate) async fn block_state(client: &Client) -> DbResult<TransactionState> {
    let (set, read) = tokio::join!(
        client.batch_execute("SELECT pg_catalog.set_config('pitwall.block_probe', 'in', true)"),
        client.query_one(
            "SELECT pg_catalog.current_setting('pitwall.block_probe', true)",
            &[]
        )
    );
    if let Err(error) = set {
        let error = db_error(&error);
        return match error.code.as_deref() {
            Some("25P02") => Ok(TransactionState::Failed),
            _ => Err(error),
        };
    }
    let value: Option<String> = pg(pg(read)?.try_get(0))?;
    Ok(if value.as_deref() == Some("in") {
        TransactionState::InTransaction
    } else {
        TransactionState::Idle
    })
}

/// 사람의 트랜잭션을 끝날 때까지 쥔다. 탭이 닫혔으면 `Some`(안에 닫기의 답 — 사라졌으면 `None`): 연결을 풀에서 떼어 닫는다(DB 가
/// 롤백한다).
async fn hold(
    tx: Transaction<'_>,
    commands: &mut Commands,
    shared: &Shared,
    pool: &Pool,
) -> Option<Option<Reply<()>>> {
    shared.set(TransactionState::InTransaction);
    match manual(&tx, commands, shared, pool).await {
        End::Commit(reply) => {
            let failed = shared.state() == TransactionState::Failed;
            // COMMIT 이 오류면(지연 제약 등) 서버가 트랜잭션 전체를 되돌렸다. 실패한 트랜잭션의 COMMIT 은 서버가 오류 없이
            // 롤백으로 끝낸다 — 그 사실 그대로 `rolled_back` 오류로 알린다(성공으로 보이지 않게, 결정 M3-D21). 다만 COMMIT 을 보낸
            // 뒤 연결이 끊겼으면 서버가 커밋했는지 알 수 없다 — 롤백이라고 말하지 않는다(M3-05 리뷰 L3).
            let result = shared
                .busy(async { pg(tx.commit().await) })
                .await
                .map_err(|e| {
                    if e.kind == pw_db_types::DbErrorKind::Disconnected {
                        e
                    } else {
                        e.with_rolled_back()
                    }
                })
                .and_then(|()| if failed { Err(failed_commit()) } else { Ok(()) });
            shared.set(TransactionState::Idle);
            send(reply, result);
            None
        }
        End::Rollback(reply) => {
            let result = shared.busy(async { pg(tx.rollback().await) }).await;
            shared.set(TransactionState::Idle);
            send(reply, result);
            None
        }
        End::Close(reply) => {
            drop(tx);
            Some(Some(reply))
        }
        End::Hangup => Some(None),
    }
}

/// 실패한 트랜잭션에 `commit` 을 부른 답.
fn failed_commit() -> DbError {
    DbError::new(
        pw_db_types::DbErrorKind::Server,
        "the transaction had failed, so COMMIT rolled it back",
    )
    .with_code("25P02")
    .with_rolled_back()
}

/// 이미 실패한 숨은 블록(롤백만 된다)을 끝날 때까지 쥔다 — 포털 · 문장은 쓸 수 없으니 연결로 `COMMIT` · `ROLLBACK` 만 보낸다.
async fn hold_failed(
    client: &Client,
    commands: &mut Commands,
    shared: &Shared,
) -> Option<Option<Reply<()>>> {
    shared.set(TransactionState::Failed);
    loop {
        let Some(command) = commands.recv().await else {
            return Some(None);
        };
        match command {
            Command::Commit(reply) => {
                let result = pg(client.batch_execute("COMMIT").await).and(Err(failed_commit()));
                shared.set(TransactionState::Idle);
                send(reply, result);
                return None;
            }
            Command::Rollback(reply) => {
                let result = pg(client.batch_execute("ROLLBACK").await);
                shared.set(TransactionState::Idle);
                send(reply, result);
                return None;
            }
            Command::Close(reply) => return Some(Some(reply)),
            Command::Execute(_, reply) => send(reply, Err(aborted())),
            Command::Fetch(_, reply) => send(reply, Err(invalid("no open cursor"))),
            Command::Discard(reply) => send(reply, Ok(())),
            Command::Begin(_, reply) | Command::Savepoint(_, _, reply) => {
                send(reply, Err(aborted()))
            }
            Command::Meta(_, reply) => send(reply, Err(aborted())),
            Command::State(reply) => send(reply, shared.busy(block_state(client)).await),
        }
    }
}

fn aborted() -> DbError {
    DbError::new(
        pw_db_types::DbErrorKind::Server,
        "current transaction is aborted, commands ignored until end of transaction block",
    )
    .with_code("25P02")
}

async fn next(pending: &mut Option<Command>, commands: &mut Commands) -> Option<Command> {
    match pending.take() {
        Some(command) => Some(command),
        None => commands.recv().await,
    }
}

enum End {
    Commit(Reply<()>),
    Rollback(Reply<()>),
    Close(Reply<()>),
    Hangup,
}

/// 사람이 연 트랜잭션 안.
async fn manual(
    tx: &Transaction<'_>,
    commands: &mut Commands,
    shared: &Shared,
    pool: &Pool,
) -> End {
    let mut pending: Option<Command> = None;
    loop {
        let Some(command) = next(&mut pending, commands).await else {
            return End::Hangup;
        };
        match command {
            Command::Commit(reply) => return End::Commit(reply),
            Command::Rollback(reply) => return End::Rollback(reply),
            Command::Close(reply) => return End::Close(reply),
            Command::Begin(_, reply) => send(reply, Err(invalid("a transaction is already open"))),
            Command::Execute(request, reply) => {
                pending = manual_execute(tx, request, reply, commands, shared).await;
            }
            Command::Savepoint(op, name, reply) => {
                let result = shared
                    .busy(async { pg(tx.batch_execute(&op.sql(&name)).await) })
                    .await;
                match &result {
                    Ok(()) if op == SavepointOp::RollbackTo => {
                        shared.set(TransactionState::InTransaction)
                    }
                    Err(e) if aborts(e) => shared.set(TransactionState::Failed),
                    _ => {}
                }
                send(reply, result);
            }
            Command::Fetch(_, reply) => send(reply, Err(invalid("no open cursor"))),
            Command::Discard(reply) => send(reply, Ok(())),
            Command::Meta(MetaRequest::Explain(sql), reply) => {
                // 실행 계획은 사람의 트랜잭션이 보는 것을 봐야 한다 — 같은 연결, 세이브포인트 안. 실패해도 트랜잭션은 그대로다.
                let result = shared
                    .busy(meta::explain_in_savepoint(tx.client(), &sql))
                    .await
                    .map(MetaResponse::Explain);
                send(reply, result);
            }
            Command::Meta(request, reply) => {
                // 사람의 트랜잭션 밖(풀의 다른 연결)에서 — 실패가 사람의 트랜잭션을 깨지 않는다.
                send(reply, elsewhere(pool, request, shared).await);
            }
            Command::State(reply) => send(reply, shared.busy(block_state(tx.client())).await),
        }
    }
}

/// 메타데이터를 풀의 다른 연결로. 그동안 인터럽트는 그 연결을 멈춘다.
async fn elsewhere(pool: &Pool, request: MetaRequest, shared: &Shared) -> DbResult<MetaResponse> {
    let object = pool.get().await.map_err(pool_error)?;
    object.clear_type_cache();
    shared.set_cancel_elsewhere(Some(object.cancel_token()));
    let result = shared.busy(meta::run(&object, request, shared)).await;
    shared.set_cancel_elsewhere(None);
    result
}

/// 문장을 준비하고 바인드 값을 맞춘다. 서버에 닿기 전 거절(트랜잭션 문장 · 바인드 값)은 트랜잭션을 건드리지 않는다.
async fn prepare(
    client: &Client,
    request: &ExecuteRequest,
    shared: &Shared,
) -> DbResult<(Statement, Vec<value::TextParam>)> {
    if !shared.skip_filter.load(Ordering::SeqCst)
        && let Some(why) = control::refused(&request.sql)
    {
        return Err(invalid(why));
    }
    let statement = shared
        .busy(async { pg(client.prepare(&request.sql).await) })
        .await?;
    if statement.params().len() != request.params.len() {
        return Err(invalid(format!(
            "the statement takes {} bind values, {} given",
            statement.params().len(),
            request.params.len()
        )));
    }
    let params = request
        .params
        .iter()
        .map(value::param)
        .collect::<DbResult<Vec<_>>>()?;
    Ok((statement, params))
}

fn columns(statement: &Statement) -> Vec<ColumnMeta> {
    statement
        .columns()
        .iter()
        .map(|c| ColumnMeta {
            name: c.name().to_owned(),
            db_type: value::type_name(c.type_()),
            nullable: None,
        })
        .collect()
}

fn ended(rows_affected: u64) -> StatementStarted {
    StatementStarted {
        columns: Vec::new(),
        end: Some(StatementEnd {
            rows_affected: Some(rows_affected),
        }),
    }
}

/// 자동 커밋 문장 뒤에 할 일.
enum IdleNext {
    Done,
    /// 커서가 열린 채 온 다른 명령.
    Pending(Command),
    /// 문장이 서버에 트랜잭션 블록을 열었다(거르개가 놓쳤다) — 그 블록을 사람의 트랜잭션으로 쥔다.
    Block,
}

/// 자동 커밋 탭의 문장.
async fn idle_execute(
    client: &mut Client,
    request: ExecuteRequest,
    reply: Reply<StatementStarted>,
    commands: &mut Commands,
    shared: &Shared,
) -> IdleNext {
    let (statement, params) = match prepare(client, &request, shared).await {
        Ok(prepared) => prepared,
        Err(e) => {
            send(reply, Err(e));
            return IdleNext::Done;
        }
    };
    if statement.columns().is_empty() {
        let result = shared
            .busy(async { pg(client.execute_raw(&statement, &params).await) })
            .await;
        // 행을 돌려주지 않는 문장만 블록을 열 수 있다(`BEGIN` 류). 블록이 열렸으면 답하기 전에 상태를 맞춘다 — 부른 쪽이 답을 받은
        // 뒤 `transaction_state` 를 보면 「트랜잭션 열림」 이다. 물을 수 없으면 그 오류를 답에 싣는다(연결이 끊긴 것이다).
        let next = match &result {
            Ok(_) => match block_state(client).await {
                Ok(TransactionState::Idle) => IdleNext::Done,
                Ok(state) => {
                    shared.set(state);
                    IdleNext::Block
                }
                Err(e) => {
                    send(reply, Err(e));
                    return IdleNext::Done;
                }
            },
            Err(_) => IdleNext::Done,
        };
        send(reply, result.map(ended));
        return next;
    }
    let tx = match shared.busy(async { pg(client.transaction().await) }).await {
        Ok(tx) => tx,
        Err(e) => {
            send(reply, Err(e));
            return IdleNext::Done;
        }
    };
    let portal = match shared
        .busy(async { pg(tx.bind_raw(&statement, &params).await) })
        .await
    {
        Ok(portal) => portal,
        Err(e) => {
            let _ = tx.rollback().await;
            send(reply, Err(binary_hint(e, &statement)));
            return IdleNext::Done;
        }
    };
    send(
        reply,
        Ok(StatementStarted {
            columns: columns(&statement),
            end: None,
        }),
    );
    let end = cursor(&tx, portal, &statement, &request, commands, shared).await;
    // 문장이 끝났으면(다 읽었거나 커서를 버렸으면) 커밋, 오류면 롤백. 그 결과가 마지막 답이다.
    match end {
        CursorEnd::Done(reply, batch) => send(reply, commit(tx, shared).await.map(|()| batch)),
        CursorEnd::Discarded(reply, Ok(())) => send(reply, commit(tx, shared).await),
        CursorEnd::Discarded(reply, Err(e)) => {
            let _ = tx.rollback().await;
            send(reply, Err(e));
        }
        CursorEnd::Failed(reply, e) => {
            let _ = tx.rollback().await;
            send(reply, Err(e));
        }
        CursorEnd::Interrupted(command, discarded) => {
            match discarded {
                Ok(()) => drop(commit(tx, shared).await),
                Err(_) => drop(tx.rollback().await),
            }
            return IdleNext::Pending(command);
        }
        CursorEnd::Hangup(discarded) => match discarded {
            Ok(()) => drop(commit(tx, shared).await),
            Err(_) => drop(tx.rollback().await),
        },
    }
    IdleNext::Done
}

/// 결과 열에 바이너리 send 함수가 없는 타입(`aclitem` 등)이 있으면 서버는 Bind · Execute 를 42883 으로 거절한다 —
/// `tokio-postgres` 는 모든 열을 바이너리로 요구하므로 드라이버가 텍스트로 바꿔 받을 수 없다(14 §3 이월). 어느 열 · 어느 타입이
/// 문제인지와 `::text` 로 바꿔 다시 실행하라는 힌트를 싣는다. 문장은 돌지 않았다(포털을 시작하기 전에 거절된다).
fn binary_hint(mut error: DbError, statement: &Statement) -> DbError {
    const PREFIX: &str = "no binary output function available for type ";
    if error.code.as_deref() != Some("42883") || !error.message.starts_with(PREFIX) {
        return error;
    }
    let ty = error.message[PREFIX.len()..]
        .split(['\n', ' '])
        .next()
        .unwrap_or_default()
        .to_owned();
    let columns: Vec<(&str, String)> = statement
        .columns()
        .iter()
        .filter(|c| {
            let element = match c.type_().kind() {
                tokio_postgres::types::Kind::Array(element) => element.name(),
                _ => c.type_().name(),
            };
            element == ty
        })
        .map(|c| (c.name(), value::type_name(c.type_())))
        .collect();
    let casts = columns
        .iter()
        .map(|(name, _)| format!("{}::text", quote_ident(name)))
        .collect::<Vec<_>>()
        .join(", ");
    let named = columns
        .iter()
        .map(|(name, ty)| format!("{name} (type {ty})"))
        .collect::<Vec<_>>()
        .join(", ");
    error.message = if columns.is_empty() {
        format!(
            "PostgreSQL cannot send type {ty} in binary, which this driver reads — cast that value to text (`…::text`) and run again\n{}",
            error.message
        )
    } else {
        format!(
            "PostgreSQL cannot send column {named} in binary, which this driver reads — select {casts} instead and run again\n{}",
            error.message
        )
    };
    error
}

async fn commit(tx: Transaction<'_>, shared: &Shared) -> DbResult<()> {
    shared.busy(async { pg(tx.commit().await) }).await
}

/// 사람이 연 트랜잭션 안의 문장.
async fn manual_execute(
    tx: &Transaction<'_>,
    request: ExecuteRequest,
    reply: Reply<StatementStarted>,
    commands: &mut Commands,
    shared: &Shared,
) -> Option<Command> {
    let fail = |e: &DbError| {
        if aborts(e) {
            shared.set(TransactionState::Failed);
        }
    };
    let (statement, params) = match prepare(tx.client(), &request, shared).await {
        Ok(prepared) => prepared,
        Err(e) => {
            fail(&e);
            send(reply, Err(e));
            return None;
        }
    };
    if statement.columns().is_empty() {
        let result = shared
            .busy(async { pg(tx.execute_raw(&statement, &params).await) })
            .await;
        match &result {
            Ok(_) if control::is_rollback_to(&request.sql) => {
                shared.set(TransactionState::InTransaction);
            }
            Err(e) => fail(e),
            Ok(_) => {}
        }
        send(reply, result.map(ended));
        return None;
    }
    let portal = match shared
        .busy(async { pg(tx.bind_raw(&statement, &params).await) })
        .await
    {
        Ok(portal) => portal,
        Err(e) => {
            fail(&e);
            send(reply, Err(binary_hint(e, &statement)));
            return None;
        }
    };
    send(
        reply,
        Ok(StatementStarted {
            columns: columns(&statement),
            end: None,
        }),
    );
    match cursor(tx, portal, &statement, &request, commands, shared).await {
        CursorEnd::Done(reply, batch) => send(reply, Ok(batch)),
        CursorEnd::Discarded(reply, result) => {
            if let Err(e) = &result {
                fail(e);
            }
            send(reply, result);
        }
        CursorEnd::Failed(reply, e) => {
            fail(&e);
            send(reply, Err(e));
        }
        CursorEnd::Interrupted(command, discarded) => {
            if let Err(e) = &discarded {
                fail(e);
            }
            return Some(command);
        }
        CursorEnd::Hangup(_) => {}
    }
    None
}

enum CursorEnd {
    /// 다 읽었다 — 마지막 묶음과 그 답.
    Done(Reply<RowBatch>, RowBatch),
    /// 커서를 버렸다.
    Discarded(Reply<()>, DbResult<()>),
    /// 읽다가 오류.
    Failed(Reply<RowBatch>, DbError),
    /// 커서가 열린 채 다른 명령이 왔다 — 커서는 버렸다.
    Interrupted(Command, DbResult<()>),
    /// trait 쪽이 사라졌다 — 커서는 버렸다.
    Hangup(DbResult<()>),
}

/// 열린 포털에서 credit 만큼 읽는다. 커서가 끝나거나 버려질 때까지 명령을 받는다.
async fn cursor(
    tx: &Transaction<'_>,
    portal: Portal,
    statement: &Statement,
    request: &ExecuteRequest,
    commands: &mut Commands,
    shared: &Shared,
) -> CursorEnd {
    let mut buffered: VecDeque<Row> = VecDeque::new();
    let mut end: Option<StatementEnd> = None;
    loop {
        let Some(command) = commands.recv().await else {
            return CursorEnd::Hangup(discard(tx, portal, request, end.is_some(), shared).await);
        };
        match command {
            Command::Fetch(limit, reply) => {
                if buffered.is_empty() && end.is_none() {
                    let read = shared
                        .busy(read_portal(
                            tx,
                            &portal,
                            statement,
                            request,
                            limit.max_rows,
                            shared,
                        ))
                        .await;
                    match read {
                        Ok((rows, done)) => {
                            buffered.extend(rows);
                            end = done;
                        }
                        Err(e) => return CursorEnd::Failed(reply, binary_hint(e, statement)),
                    }
                }
                let rows = take(&mut buffered, limit);
                if buffered.is_empty() && end.is_some() {
                    return CursorEnd::Done(reply, RowBatch { rows, end });
                }
                send(reply, Ok(RowBatch { rows, end: None }));
            }
            Command::Discard(reply) => {
                let result = discard(tx, portal, request, end.is_some(), shared).await;
                return CursorEnd::Discarded(reply, result);
            }
            other => {
                let result = discard(tx, portal, request, end.is_some(), shared).await;
                return CursorEnd::Interrupted(other, result);
            }
        }
    }
}

/// 포털에서 `max_rows` 까지 읽어 값으로 바꾼다. 포털이 끝났으면 그 끝을 함께 돌려준다.
async fn read_portal(
    tx: &Transaction<'_>,
    portal: &Portal,
    statement: &Statement,
    request: &ExecuteRequest,
    max_rows: u32,
    shared: &Shared,
) -> DbResult<(Vec<Row>, Option<StatementEnd>)> {
    let max_rows = i32::try_from(max_rows.max(1)).unwrap_or(i32::MAX);
    let stream = pg(tx.query_portal_raw(portal, max_rows).await)?;
    let mut stream = std::pin::pin!(stream);
    let limit: CellLimit = request.cell_limit;
    let mut texts = TextRequests::default();
    let mut cells = Vec::new();
    while let Some(row) = stream.next().await {
        let row = pg(row)?;
        let mut cell_row = Vec::with_capacity(statement.columns().len());
        for (i, column) in statement.columns().iter().enumerate() {
            let Raw(raw) = pg(row.try_get(i))?;
            cell_row.push(value::decode(column.type_(), raw, limit, &mut texts));
        }
        cells.push(cell_row);
    }
    // 포털이 끝까지 돌았으면 CommandComplete 의 행 수가 있고, 최대 행에서 멈췄으면(PortalSuspended) 없다.
    let end = stream.rows_affected().map(|n| StatementEnd {
        rows_affected: (!request.grade.is_read()).then_some(n),
    });
    let outputs = if texts.is_empty() {
        Vec::new()
    } else {
        value::text_outputs(
            tx.client(),
            &texts,
            shared.text_fault.load(Ordering::SeqCst),
        )
        .await?
    };
    let rows = cells
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|cell| value::finish(cell, &texts, &outputs, limit))
                .collect()
        })
        .collect();
    Ok((rows, end))
}

/// 묶음 하나 — `max_rows` 와 `max_bytes` 중 먼저 닿는 쪽까지(한 행은 바이트 상한을 넘어도 통째로).
fn take(buffered: &mut VecDeque<Row>, limit: FetchLimit) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut bytes = 0_usize;
    while rows.len() < limit.max_rows.max(1) as usize
        && (rows.is_empty() || bytes < limit.max_bytes as usize)
    {
        let Some(row) = buffered.pop_front() else {
            break;
        };
        bytes += row.iter().map(value::approximate_size).sum::<usize>();
        rows.push(row);
    }
    rows
}

/// 커서를 버린다: 조회면 포털을 닫고(서버가 나머지를 만들지 않는다), 조회가 아니면 남은 행을 끝까지 읽어 버린 뒤 닫는다.
async fn discard(
    tx: &Transaction<'_>,
    portal: Portal,
    request: &ExecuteRequest,
    finished: bool,
    shared: &Shared,
) -> DbResult<()> {
    if !finished && !request.grade.is_read() {
        shared
            .busy(async {
                // 최대 행 0 = 남은 행 전부.
                let stream = pg(tx.query_portal_raw(&portal, 0).await)?;
                let mut stream = std::pin::pin!(stream);
                while let Some(row) = stream.next().await {
                    pg(row)?;
                }
                Ok(())
            })
            .await?;
    }
    // 포털을 놓으면 `Close` 를 보낸다(다음 요청보다 먼저 간다).
    drop(portal);
    Ok(())
}
