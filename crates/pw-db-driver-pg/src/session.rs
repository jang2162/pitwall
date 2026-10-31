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
//! 이다 — 사람이 연 트랜잭션이 아니다.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use deadpool_postgres::Object;
use futures_util::StreamExt;
use pw_db_driver::DbResult;
use pw_db_types::{
    BeginOptions, CellLimit, ColumnMeta, DbError, ExecuteRequest, FetchLimit, Row, RowBatch,
    StatementEnd, StatementStarted, TransactionState,
};
use tokio::sync::{mpsc, oneshot};
use tokio_postgres::{Client, Portal, Statement, Transaction};

use crate::control;
use crate::error::{db_error, invalid};
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
#[derive(Debug)]
pub(crate) struct Shared {
    state: Mutex<TransactionState>,
    /// 서버에서 도는 요청이 있다 — 인터럽트는 이때만 CancelRequest 를 보낸다(쉬는 연결에 보내면 다음 문장이 멈출 수 있다).
    pub(crate) running: AtomicBool,
    /// 시험: 서버의 텍스트 출력 되돌려 받기를 일부러 실패시킨다(`value::text_outputs`).
    pub(crate) text_fault: AtomicBool,
    /// 연결한 DB(`current_database()`) — 메타데이터가 다른 DB 를 묻으면 거절한다.
    pub(crate) database: String,
}

impl Shared {
    pub(crate) fn new(database: String) -> Self {
        Self {
            state: Mutex::new(TransactionState::Idle),
            running: AtomicBool::new(false),
            text_fault: AtomicBool::new(false),
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

/// 연결을 쥐고 명령을 받는다. 닫히면(또는 trait 쪽이 사라지면) 쉬는 연결은 풀에 돌려주고, 트랜잭션이 열린 연결은 풀에서 떼어 닫는다 —
/// DB 가 그 트랜잭션을 롤백한다.
pub(crate) async fn run(mut object: Object, mut commands: Commands, shared: Arc<Shared>) {
    let mut pending: Option<Command> = None;
    let detach = loop {
        let Some(command) = next(&mut pending, &mut commands).await else {
            break false;
        };
        let client: &mut Client = &mut object;
        match command {
            Command::Close(reply) => {
                send(reply, Ok(()));
                break false;
            }
            Command::Execute(request, reply) => {
                pending = idle_execute(client, request, reply, &mut commands, &shared).await;
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
                match manual(&tx, &mut commands, &shared).await {
                    End::Commit(reply) => {
                        // 실패한 트랜잭션의 COMMIT 은 서버가 롤백으로 끝낸다(오류 없이). COMMIT 이 오류면(지연 제약 등) 서버가 트랜잭션
                        // 전체를 되돌렸다 — `rolled_back` 으로 알린다. 어느 쪽이든 트랜잭션은 끝났다.
                        let result = shared
                            .busy(async { pg(tx.commit().await) })
                            .await
                            .map_err(DbError::with_rolled_back);
                        shared.set(TransactionState::Idle);
                        send(reply, result);
                    }
                    End::Rollback(reply) => {
                        let result = shared.busy(async { pg(tx.rollback().await) }).await;
                        shared.set(TransactionState::Idle);
                        send(reply, result);
                    }
                    End::Close(reply) => {
                        drop(tx);
                        send(reply, Ok(()));
                        break true;
                    }
                    End::Hangup => {
                        drop(tx);
                        break true;
                    }
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
                let result = meta::run(client, request, &shared).await;
                send(reply, result);
            }
        }
    };
    shared.set(TransactionState::Idle);
    if detach {
        drop(Object::take(object));
    }
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
async fn manual(tx: &Transaction<'_>, commands: &mut Commands, shared: &Shared) -> End {
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
            Command::Meta(request, reply) => {
                let result = meta::run(tx.client(), request, shared).await;
                if result.as_ref().is_err_and(aborts) {
                    shared.set(TransactionState::Failed);
                }
                send(reply, result);
            }
        }
    }
}

/// 문장을 준비하고 바인드 값을 맞춘다. 서버에 닿기 전 거절(트랜잭션 문장 · 바인드 값)은 트랜잭션을 건드리지 않는다.
async fn prepare(
    client: &Client,
    request: &ExecuteRequest,
    shared: &Shared,
) -> DbResult<(Statement, Vec<value::TextParam>)> {
    if let Some(why) = control::refused(&request.sql) {
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

/// 자동 커밋 탭의 문장. 남은 명령(커서가 열린 채 온 다른 명령)이 있으면 돌려준다.
async fn idle_execute(
    client: &mut Client,
    request: ExecuteRequest,
    reply: Reply<StatementStarted>,
    commands: &mut Commands,
    shared: &Shared,
) -> Option<Command> {
    let (statement, params) = match prepare(client, &request, shared).await {
        Ok(prepared) => prepared,
        Err(e) => {
            send(reply, Err(e));
            return None;
        }
    };
    if statement.columns().is_empty() {
        let result = shared
            .busy(async { pg(client.execute_raw(&statement, &params).await) })
            .await;
        send(reply, result.map(ended));
        return None;
    }
    let tx = match shared.busy(async { pg(client.transaction().await) }).await {
        Ok(tx) => tx,
        Err(e) => {
            send(reply, Err(e));
            return None;
        }
    };
    let portal = match shared
        .busy(async { pg(tx.bind_raw(&statement, &params).await) })
        .await
    {
        Ok(portal) => portal,
        Err(e) => {
            let _ = tx.rollback().await;
            send(reply, Err(e));
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
            return Some(command);
        }
        CursorEnd::Hangup(discarded) => match discarded {
            Ok(()) => drop(commit(tx, shared).await),
            Err(_) => drop(tx.rollback().await),
        },
    }
    None
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
            send(reply, Err(e));
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
                        Err(e) => return CursorEnd::Failed(reply, e),
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
