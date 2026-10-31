//! 연결 스레드와 그 앞의 [`SqlConnection`].
//!
//! 연결 하나에 전용 블로킹 스레드 하나가 `rusqlite::Connection` 을 쥐고(결정 M3-D4), async 쪽은 명령 채널로만 말한다 — tokio
//! 워커는 SQLite 를 기다리지 않는다. 행 커서는 그 스레드 안의 `Statement` 다: [`execute`](SqlConnection::execute) 가 문장을
//! 준비해 열을 돌려주고, 스레드는 그 문장을 쥔 채 다음 명령을 기다리며 [`fetch`](SqlConnection::fetch) 마다 credit 만큼만 걷는다.
//! 다른 명령이 오면 먼저 커서를 버린다.
//!
//! **커서 버리기**는 문장을 리셋하는 것이다 — 등급과 상관없이 같다. SQLite 는 `… RETURNING` 의 쓰기를 첫 걸음에 모두 하고,
//! 리셋해도 그 쓰기는 남는다(MySQL 과 달리 남은 행을 읽어 버릴 까닭이 없다). 트랜잭션은 그대로다.
//!
//! **취소**는 스레드 밖으로 미리 꺼낸 `InterruptHandle` 이다(`sqlite3_interrupt`). 도는 문장은 `interrupted` 로 끝나고, 도는
//! 문장이 없으면 SQLite 가 다음 문장을 시작할 때 표지를 지운다. 명시 트랜잭션 안의 쓰기를 멈추면 SQLite 가 그 트랜잭션을 롤백할
//! 수 있다 — 트랜잭션 상태는 명령이 끝날 때마다 `is_autocommit` 으로 다시 읽는다.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use pw_db_driver::{BoxFuture, Connected, DbConnection, DbResult, Interrupt, SqlConnection};
use pw_db_types::{
    BeginOptions, CellLimit, ColumnInfo, ColumnMeta, ConnectionSpec, DbError, DbErrorKind, Ddl,
    ExecuteRequest, ExplainPlan, FetchLimit, ForeignKeyInfo, IndexInfo, ObjectPage, ObjectQuery,
    ObjectRef, Row, RowBatch, ServerInfo, StatementEnd, StatementStarted, TlsStatus,
    TransactionState,
};
use rusqlite::{Connection, InterruptHandle, Rows, Statement};
use tokio::sync::oneshot;

use crate::error::{disconnected, invalid};
use crate::guard::{self, Guard};
use crate::{meta, value};

type Reply<T> = oneshot::Sender<DbResult<T>>;
type Job = Box<dyn FnOnce(&Worker) + Send>;

enum Command {
    Execute(ExecuteRequest, Reply<StatementStarted>),
    Fetch(FetchLimit, Reply<RowBatch>),
    Discard(Reply<()>),
    /// 커서와 상관없는 일(트랜잭션 · 메타데이터 · 실행 계획). 답은 일이 스스로 보낸다.
    Job(Job),
    Close(Reply<()>),
}

/// 연결을 연다: 스레드를 띄우고 그 안에서 파일을 연 뒤 인터럽트 손잡이를 받아 온다.
pub(crate) async fn open(spec: ConnectionSpec) -> DbResult<Connected> {
    let (ready_tx, ready_rx) = oneshot::channel();
    let (commands, inbox) = mpsc::channel();
    let in_transaction = Arc::new(AtomicBool::new(false));
    let shared = in_transaction.clone();
    thread::Builder::new()
        .name("pw-db-sqlite".to_owned())
        .spawn(move || match Worker::open(&spec, shared) {
            Ok(worker) => {
                // 연결을 기다리던 쪽이 떠났으면(연결 타임아웃) 연결을 닫고 끝난다.
                if ready_tx
                    .send(Ok(worker.conn.get_interrupt_handle()))
                    .is_ok()
                {
                    worker.run(&inbox);
                }
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        })
        .map_err(|e| {
            DbError::new(
                DbErrorKind::Connect,
                format!("cannot start the SQLite thread: {e}"),
            )
        })?;
    let handle = ready_rx.await.map_err(|_| disconnected())??;
    Ok(Connected {
        connection: DbConnection::Sql(Box::new(SqliteConnection {
            commands: Some(commands),
            interrupt: Arc::new(SqliteInterrupt(handle)),
            in_transaction,
        })),
        server: ServerInfo {
            version: Some(rusqlite::version().to_owned()),
            tls: TlsStatus::NotApplicable,
        },
    })
}

/// async 쪽 손잡이. 놓으면(drop) 도는 문장을 멈추고 스레드가 연결을 닫고 끝난다.
struct SqliteConnection {
    /// `close` 뒤에는 `None`.
    commands: Option<mpsc::Sender<Command>>,
    interrupt: Arc<SqliteInterrupt>,
    in_transaction: Arc<AtomicBool>,
}

impl SqliteConnection {
    fn send<T: Send + 'static>(
        &self,
        command: impl FnOnce(Reply<T>) -> Command,
    ) -> BoxFuture<'static, DbResult<T>> {
        let (reply, answer) = oneshot::channel();
        let sent = self
            .commands
            .as_ref()
            .is_some_and(|commands| commands.send(command(reply)).is_ok());
        Box::pin(async move {
            if !sent {
                return Err(disconnected());
            }
            answer.await.map_err(|_| disconnected())?
        })
    }

    fn job<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Worker) -> DbResult<T> + Send + 'static,
    ) -> BoxFuture<'static, DbResult<T>> {
        self.send(|reply| {
            Command::Job(Box::new(move |worker| {
                let result = work(worker);
                worker.reply(reply, result);
            }))
        })
    }
}

impl Drop for SqliteConnection {
    fn drop(&mut self) {
        if self.commands.take().is_some() {
            // 버려진 실행(무한 루프 등)이 스레드를 붙잡지 않게 한다. 도는 문장이 없으면 아무것도 하지 않는다.
            self.interrupt.0.interrupt();
        }
    }
}

impl SqlConnection for SqliteConnection {
    fn interrupter(&self) -> Arc<dyn Interrupt> {
        self.interrupt.clone()
    }

    fn transaction_state(&self) -> TransactionState {
        if self.in_transaction.load(Ordering::SeqCst) {
            TransactionState::InTransaction
        } else {
            TransactionState::Idle
        }
    }

    fn execute(&mut self, request: ExecuteRequest) -> BoxFuture<'_, DbResult<StatementStarted>> {
        self.send(|reply| Command::Execute(request, reply))
    }

    fn fetch(&mut self, limit: FetchLimit) -> BoxFuture<'_, DbResult<RowBatch>> {
        self.send(|reply| Command::Fetch(limit, reply))
    }

    fn discard_cursor(&mut self) -> BoxFuture<'_, DbResult<()>> {
        self.send(Command::Discard)
    }

    fn begin(&mut self, options: BeginOptions) -> BoxFuture<'_, DbResult<()>> {
        self.job(move |worker| worker.begin(options))
    }

    fn commit(&mut self) -> BoxFuture<'_, DbResult<()>> {
        self.job(|worker| worker.batch("COMMIT"))
    }

    fn rollback(&mut self) -> BoxFuture<'_, DbResult<()>> {
        self.job(|worker| worker.batch("ROLLBACK"))
    }

    fn savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        self.job(move |worker| worker.batch(&format!("SAVEPOINT {}", meta::quote(&name))))
    }

    fn rollback_to_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        self.job(move |worker| {
            worker.batch(&format!("ROLLBACK TO SAVEPOINT {}", meta::quote(&name)))
        })
    }

    fn release_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        self.job(move |worker| worker.batch(&format!("RELEASE SAVEPOINT {}", meta::quote(&name))))
    }

    fn explain(&mut self, sql: String) -> BoxFuture<'_, DbResult<ExplainPlan>> {
        self.job(move |worker| meta::explain(&worker.conn, &sql).map_err(|e| worker.guard.error(e)))
    }

    fn objects(&mut self, query: ObjectQuery) -> BoxFuture<'_, DbResult<ObjectPage>> {
        self.job(move |worker| meta::objects(&worker.conn, &query))
    }

    fn columns(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ColumnInfo>>> {
        self.job(move |worker| meta::columns(&worker.conn, &object))
    }

    fn indexes(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<IndexInfo>>> {
        self.job(move |worker| meta::indexes(&worker.conn, &object))
    }

    fn foreign_keys(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ForeignKeyInfo>>> {
        self.job(move |worker| meta::foreign_keys(&worker.conn, &object))
    }

    fn ddl(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Ddl>> {
        self.job(move |worker| meta::ddl(&worker.conn, &object))
    }

    fn close(mut self: Box<Self>) -> BoxFuture<'static, DbResult<()>> {
        let closing = self.send(Command::Close);
        // 닫는 중에는 인터럽트하지 않는다(Drop).
        self.commands = None;
        closing
    }
}

struct SqliteInterrupt(InterruptHandle);

impl Interrupt for SqliteInterrupt {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>> {
        self.0.interrupt();
        Box::pin(std::future::ready(Ok(())))
    }
}

/// 스레드 안의 연결.
struct Worker {
    conn: Connection,
    guard: Guard,
    /// read-only 연결 — `query_only` 가 늘 켜져 있다.
    read_only: bool,
    /// 이 연결이 연 읽기 전용 트랜잭션이 열려 있다.
    read_only_transaction: Cell<bool>,
    in_transaction: Arc<AtomicBool>,
}

impl Worker {
    fn open(spec: &ConnectionSpec, in_transaction: Arc<AtomicBool>) -> DbResult<Self> {
        let (conn, guard) = guard::open(spec)?;
        Ok(Self {
            conn,
            guard,
            read_only: spec.read_only,
            read_only_transaction: Cell::new(false),
            in_transaction,
        })
    }

    fn run(self, inbox: &mpsc::Receiver<Command>) {
        let mut pending = None;
        loop {
            let command = match pending.take() {
                Some(command) => command,
                // 손잡이를 놓았다 — 연결을 닫는다(열린 트랜잭션은 SQLite 가 롤백한다).
                None => match inbox.recv() {
                    Ok(command) => command,
                    Err(_) => return,
                },
            };
            self.guard.clear_denial();
            match command {
                Command::Execute(request, reply) => pending = self.execute(request, reply, inbox),
                Command::Fetch(_, reply) => self.reply(reply, Err(invalid("no open cursor"))),
                Command::Discard(reply) => self.reply(reply, Ok(())),
                Command::Job(job) => job(&self),
                Command::Close(reply) => {
                    self.in_transaction.store(false, Ordering::SeqCst);
                    let closed = self
                        .conn
                        .close()
                        .map_err(|(_, e)| crate::error::map(e, None));
                    let _ = reply.send(closed);
                    return;
                }
            }
        }
    }

    /// 상태를 맞춘 뒤 답한다 — async 쪽이 답을 받은 뒤 읽는 `transaction_state` 가 이미 새 값이다.
    fn reply<T>(&self, reply: Reply<T>, result: DbResult<T>) {
        self.settle();
        let _ = reply.send(result);
    }

    fn settle(&self) {
        let autocommit = self.conn.is_autocommit();
        if self.read_only_transaction.get() && autocommit {
            // 읽기 전용 트랜잭션이 끝났다(롤백 · 커밋 · 인터럽트) — `query_only` 를 연결 본래의 값으로 되돌린다.
            self.read_only_transaction.set(false);
            if !self.read_only {
                let _ = self.guard.set_query_only(&self.conn, false);
            }
        }
        self.in_transaction.store(!autocommit, Ordering::SeqCst);
    }

    fn batch(&self, sql: &str) -> DbResult<()> {
        self.conn
            .execute_batch(sql)
            .map_err(|e| self.guard.error(e))
    }

    /// 읽기 전용 트랜잭션은 `BEGIN` + `query_only`(결정 M3-D6). 끝나면 [`settle`](Self::settle) 이 되돌린다.
    fn begin(&self, options: BeginOptions) -> DbResult<()> {
        self.batch("BEGIN")?;
        if options.read_only {
            self.read_only_transaction.set(true);
            if let Err(error) = self.guard.set_query_only(&self.conn, true) {
                let error = self.guard.error(error);
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
        Ok(())
    }

    fn prepare(&self, request: &ExecuteRequest) -> DbResult<Statement<'_>> {
        let mut statement = self
            .conn
            .prepare(&request.sql)
            .map_err(|e| self.guard.error(e))?;
        if statement.parameter_count() != request.params.len() {
            return Err(invalid(format!(
                "the statement takes {} bind values, got {}",
                statement.parameter_count(),
                request.params.len()
            )));
        }
        for (index, param) in request.params.iter().enumerate() {
            statement
                .raw_bind_parameter(index + 1, value::to_sqlite(param)?)
                .map_err(|e| self.guard.error(e))?;
        }
        Ok(statement)
    }

    /// 문장 하나. 행이 없는 문장은 끝까지 돌리고, 행이 있으면 열을 답한 뒤 커서를 쥐고 다음 명령을 기다린다.
    fn execute(
        &self,
        request: ExecuteRequest,
        reply: Reply<StatementStarted>,
        inbox: &mpsc::Receiver<Command>,
    ) -> Option<Command> {
        // 주석 · 공백뿐인 글은 SQLite 에 문장이 없다(빈 `Statement` 를 돌리면 API 오용 오류다).
        if skip_trivia(&request.sql).is_empty() {
            self.reply(
                reply,
                Ok(StatementStarted {
                    columns: Vec::new(),
                    end: Some(StatementEnd {
                        rows_affected: None,
                    }),
                }),
            );
            return None;
        }
        let mut statement = match self.prepare(&request) {
            Ok(statement) => statement,
            Err(error) => {
                self.reply(reply, Err(error));
                return None;
            }
        };
        let changes_before = self.conn.total_changes();
        if statement.column_count() == 0 {
            let result = statement
                .raw_execute()
                .map(|_| StatementStarted {
                    columns: Vec::new(),
                    end: Some(self.end(&request.sql, statement.readonly(), changes_before)),
                })
                .map_err(|e| self.guard.error(e));
            drop(statement);
            self.reply(reply, result);
            return None;
        }
        let columns = statement
            .columns()
            .iter()
            .map(|column| ColumnMeta {
                name: column.name().to_owned(),
                db_type: column.decl_type().unwrap_or_default().to_owned(),
                nullable: None,
            })
            .collect();
        let width = statement.column_count();
        let read_only = statement.readonly();
        self.reply(reply, Ok(StatementStarted { columns, end: None }));
        let mut rows = statement.raw_query();
        loop {
            let Ok(command) = inbox.recv() else {
                return None;
            };
            match command {
                Command::Fetch(limit, reply) => {
                    self.guard.clear_denial();
                    match fetch(&mut rows, limit, request.cell_limit, width) {
                        Ok((batch, false)) => {
                            self.reply(
                                reply,
                                Ok(RowBatch {
                                    rows: batch,
                                    end: None,
                                }),
                            );
                        }
                        Ok((batch, true)) => {
                            // 문장을 리셋해 잠금을 놓은 뒤 답한다.
                            drop(rows);
                            let end = self.end(&request.sql, read_only, changes_before);
                            self.reply(
                                reply,
                                Ok(RowBatch {
                                    rows: batch,
                                    end: Some(end),
                                }),
                            );
                            return None;
                        }
                        Err(error) => {
                            drop(rows);
                            self.reply(reply, Err(self.guard.error(error)));
                            return None;
                        }
                    }
                }
                Command::Discard(reply) => {
                    drop(rows);
                    self.reply(reply, Ok(()));
                    return None;
                }
                // 커서와 상관없는 명령 — 커서를 버리고(리셋) 그 명령을 처리한다.
                other => return Some(other),
            }
        }
    }

    /// 문장의 끝. 바뀐 행 수는 그 문장이 행을 바꿨거나 DML 로 시작할 때만 있다 — `sqlite3_changes` 는 DML 이 아닌 문장
    /// (`CREATE` 등) 뒤에 앞 DML 의 값을 그대로 둔다. `WITH … DELETE` 처럼 DML 이 뒤에 오는 문장이 행을 바꾸지 않았으면 `None` 이다.
    fn end(&self, sql: &str, read_only: bool, changes_before: u64) -> StatementEnd {
        let changed = self.conn.total_changes() != changes_before;
        StatementEnd {
            rows_affected: (changed || (!read_only && starts_with_dml(sql)))
                .then(|| self.conn.changes()),
        }
    }
}

/// 커서에서 `limit` 만큼 걷는다. 두 번째 값은 커서가 다 읽혔는지.
fn fetch(
    rows: &mut Rows<'_>,
    limit: FetchLimit,
    cell_limit: CellLimit,
    width: usize,
) -> rusqlite::Result<(Vec<Row>, bool)> {
    let max_rows = limit.max_rows as usize;
    let max_bytes = limit.max_bytes as usize;
    let mut batch = Vec::new();
    let mut bytes = 0;
    while batch.len() < max_rows && (batch.is_empty() || bytes < max_bytes) {
        let Some(row) = rows.next()? else {
            return Ok((batch, true));
        };
        let mut values = Vec::with_capacity(width);
        for index in 0..width {
            let cell = row.get_ref(index)?;
            bytes += value::size(cell);
            values.push(value::to_db(cell, cell_limit));
        }
        batch.push(values);
    }
    Ok((batch, false))
}

/// 앞의 공백 · 주석 · `;` 를 건너뛴 나머지.
fn skip_trivia(sql: &str) -> &str {
    let mut rest = sql;
    loop {
        rest = rest.trim_start().trim_start_matches(';');
        if let Some(line) = rest.strip_prefix("--") {
            rest = line.split_once('\n').map_or("", |(_, after)| after);
        } else if let Some(block) = rest.strip_prefix("/*") {
            rest = block.split_once("*/").map_or("", |(_, after)| after);
        } else if rest.starts_with(|c: char| c.is_whitespace() || c == ';') {
            continue;
        } else {
            return rest;
        }
    }
}

/// 주석 · 공백 뒤 첫 낱말이 `INSERT` · `UPDATE` · `DELETE` · `REPLACE` 인가.
fn starts_with_dml(sql: &str) -> bool {
    let word: String = skip_trivia(sql)
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    ["INSERT", "UPDATE", "DELETE", "REPLACE"]
        .iter()
        .any(|dml| word.eq_ignore_ascii_case(dml))
}
