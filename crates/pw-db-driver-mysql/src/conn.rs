//! SQL 갈래: 연결 하나, 결과 집합 커서, 커서 버리기, 취소, 트랜잭션.
//!
//! **문장 하나 = 서버 문장 하나**(파이프라인의 문장 나누기가 서버 설정과 어긋나 틀리게 나눠도 숨은 둘째 문장이 위험 분류를 비켜 가지
//! 못하게): `mysql_async` 는 능력 `CLIENT_MULTI_STATEMENTS` 를 늘 알리고 끄는 설정이 없어(세션의 `COM_SET_OPTION` 도 보내는 길이
//! 없다), 사람의 문장은 먼저 준비 문장(COM_STMT_PREPARE)으로 보낸다 — 서버는 준비할 글을 문장 하나로 읽고, 뒤에 문장이 더 있으면
//! 1064 로 거절한다(첫 문장이 `CREATE PROCEDURE` 여도). 서버가 준비 문장으로 받지 않는 문장(1295 — MySQL 의 `USE` ·
//! `CREATE PROCEDURE` 등)만 텍스트 프로토콜로 보낸다: 1295 는 글 전체를 문장 하나로 읽은 뒤에 나오므로 그 글은 문장 하나다
//! — 두 서버 모두 준비 모드에서 `multi_statements = false` 로 `parse_sql` 을 끝낸 뒤 `check_prepared_statement` 에서만
//! `ER_UNSUPPORTED_PS` 를 낸다(MySQL 8.4.6 · MariaDB 11.8.3 `sql/sql_prepare.cc`, 문법 `sql_yacc.yy` 에는 그 오류가 없다). 진단 문장
//! (`SHOW WARNINGS` 등)은 준비 시도가 진단 영역을 덮으므로 정확한 꼴일 때만 준비 없이 텍스트로 보낸다(`diag`). 문장 캐시는 0 이고
//! `mysql_async` 는 그때 준비 문장을 닫지 않으므로 문장이 끝나면 직접 닫는다.
//!
//! **트랜잭션 상태**는 서버의 상태 플래그(`SERVER_STATUS_IN_TRANS`)로만 읽는다. 오류 패킷에는 그 플래그가 없으므로 트랜잭션 안에서
//! 오류가 나면 `COM_PING` 하나로 OK 패킷을 받아 다시 읽는다 — 문장이 아니라 진단 영역을 건드리지 않는다(MySQL 8.4 은 `DO 0` 같은
//! 문장도 진단 영역을 지운다). 트랜잭션이 끝났으면: 오류가 트랜잭션 전체를 되돌리는 것(교착 1213, `innodb_rollback_on_timeout`
//! 이 켜진 서버의 잠금 대기 1205)이면 `DbError::rolled_back`, 그 밖은 **암묵 커밋**이다 — DDL · `LOCK TABLES` · `TRUNCATE` 등은
//! 실행 전에 열린 트랜잭션을 커밋한 뒤 실패할 수 있다. 그때는 되돌리지 않았으므로 `rolled_back` 이 아니고 상태만 `Idle` 이 된다.
//!
//! **커서**는 결과 집합 스트리밍이다. `mysql_async` 는 읽다 만 결과 집합을 연결 안에 「남은 결과」 로 들고 있으므로, `fetch` 마다
//! 그 연결로 `QueryResult` 를 다시 만들어 credit 만큼만 행을 읽는다 — 읽지 않은 행은 서버 쪽에서 소켓이 막혀 기다린다.
//!
//! **커서 버리기**(14-m3-plan.md M3-07): 결과 집합은 서버가 다 보낼 때까지 연결을 잡는다. 그 커서를 연 요청의 등급이 조회면 다른
//! 연결로 `KILL QUERY <id>` 를 보내 문장만 끝내고(트랜잭션은 그대로) 남은 패킷을 받아 버린다. 조회가 아닌 문장(MariaDB `RETURNING`
//! 등)은 문장을 죽이면 그 쓰기가 되돌려지므로 남은 행을 끝까지 읽어 버린다. 어느 쪽이든 연결을 닫지 않는다.
//!
//! **취소**도 새 연결로 `KILL QUERY <id>` 다. `SELECT SLEEP(30)` 처럼 죽여도 오류 없이 끝나는 문장이 있어 「인터럽트를 받은 조회」
//! 는 끝이 정상이어도 취소로 돌려준다. 조회가 아닌 문장이 오류 없이 끝났으면 그 쓰기는 끝난 것이므로 정상으로 돌려준다.
//! (MySQL 8.4 의 `SLEEP` 은 죽여도 1 을 돌려주고, MariaDB 11.8 은 1317 오류로 끝난다.) 아직 가는 중인 `KILL QUERY` 가 다음 문장을
//! 죽이지 않게, 인터럽트를 받은 문장 뒤에는 그 `KILL` 이 끝나기를 기다린다. 쉬는 연결에 닿은 `KILL QUERY` 는 다음 문장에 남지
//! 않는다(MySQL 8.4 · MariaDB 11.8 에서 확인).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use mysql_async::consts::StatusFlags;
use mysql_async::prelude::Queryable;
use mysql_async::{
    BinaryProtocol, Column, Conn, Opts, Params, QueryResult, Statement as Prepared, TextProtocol,
};
use pw_db_driver::budget::within;
use pw_db_driver::{BoxFuture, DbResult, Interrupt, SqlConnection};
use pw_db_types::{
    BeginOptions, CellLimit, ColumnInfo, DatabaseInfo, DbError, DbErrorKind, Ddl, ExecuteRequest,
    ExplainPlan, FetchLimit, ForeignKeyInfo, IndexInfo, ObjectPage, ObjectQuery, ObjectRef,
    PlanFormat, RiskGrade, RowBatch, StatementEnd, StatementStarted, TransactionState,
};

use crate::error::{Phase, code, map, server_code};
use crate::infile::{self, RefuseLocalInfile};
use crate::{diag, meta, value};

/// 교착 — 서버가 트랜잭션 전체를 되돌렸다.
const DEADLOCK: u16 = 1213;
/// 잠금 대기 시간 초과 — `innodb_rollback_on_timeout` 이 켜진 서버에서만 트랜잭션 전체를 되돌린다.
const LOCK_WAIT_TIMEOUT: u16 = 1205;

/// 진행 중 문장을 다른 연결의 `KILL QUERY` 로 멈춘다.
pub(crate) struct Interrupter {
    /// 이 연결을 연 설정(TLS 포함) — `KILL` 연결도 같은 설정으로 연다.
    opts: Opts,
    /// 서버의 연결 id(`CONNECTION_ID()` — MariaDB 는 64비트라 핸드셰이크의 32비트 값을 쓰지 않는다).
    id: u64,
    /// `KILL` 연결을 열고 보내는 데 쓰는 시간 상한.
    timeout: Duration,
    state: Mutex<Statement>,
    /// 보내는 중인 `KILL` 을 잡는다 — 문장을 마친 연결이 그 `KILL` 이 끝나기를 기다린다.
    kill: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Statement {
    running: bool,
    interrupted: bool,
}

impl Interrupter {
    pub(crate) fn new(opts: Opts, id: u64, timeout: Duration) -> Self {
        Self {
            opts,
            id,
            timeout,
            state: Mutex::default(),
            kill: tokio::sync::Mutex::new(()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, Statement> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn kill_query(&self) -> DbResult<()> {
        within(self.timeout, "cancel", async {
            let mut conn = Conn::new(self.opts.clone())
                .await
                .map_err(|e| map(e, Phase::Connect))?;
            let killed = conn.query_drop(format!("KILL QUERY {}", self.id)).await;
            let _ = conn.disconnect().await;
            match killed {
                Ok(()) => Ok(()),
                // 그사이 연결이 사라졌다.
                Err(e) if server_code(&e) == Some(code::UNKNOWN_THREAD) => Ok(()),
                Err(e) => Err(map(e, Phase::Run)),
            }
        })
        .await
    }
}

impl Interrupt for Interrupter {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            let _sending = self.kill.lock().await;
            {
                let mut state = self.state();
                if !state.running {
                    return Ok(());
                }
                state.interrupted = true;
            }
            self.kill_query().await
        })
    }
}

/// 연 커서.
struct Cursor {
    /// 준비 문장으로 보냈으면 그 문장(바이너리 프로토콜). 끝나면 닫는다. 텍스트 프로토콜이면 `None`.
    prepared: Option<Prepared>,
    columns: Arc<[Column]>,
    grade: RiskGrade,
    cell_limit: CellLimit,
}

pub(crate) struct MysqlConnection {
    conn: Option<Conn>,
    cursor: Option<Cursor>,
    interrupter: Arc<Interrupter>,
    state: TransactionState,
    /// 서버가 `LOCAL INFILE` 로 파일을 청했다는 표지(`infile` 머리 주석).
    infile_asked: Arc<AtomicBool>,
    /// 방금 끝난 문장의 오류가 트랜잭션 전체를 되돌렸다 — 다음 [`Self::error`] 가 `DbError::rolled_back` 에 싣는다.
    rolled_back: bool,
    /// 서버의 `innodb_rollback_on_timeout`(연결 때 읽는다).
    rollback_on_timeout: bool,
}

async fn next_row(conn: &mut Conn, binary: bool) -> mysql_async::Result<Option<mysql_async::Row>> {
    if binary {
        QueryResult::<BinaryProtocol>::new(conn).next().await
    } else {
        QueryResult::<TextProtocol>::new(conn).next().await
    }
}

/// 남은 결과(지금 결과 집합의 남은 행과 그 뒤의 결과 집합)를 끝까지 받아 버린다.
async fn drain(conn: &mut Conn, binary: bool) -> mysql_async::Result<()> {
    if binary {
        QueryResult::<BinaryProtocol>::new(conn).drop_result().await
    } else {
        QueryResult::<TextProtocol>::new(conn).drop_result().await
    }
}

/// 사람의 글을 서버 문장 하나로 준비한다(머리 주석의 「문장 하나」). 서버가 준비 문장으로 받지 않는 문장(1295)이면 `None` — 그때만
/// 텍스트 프로토콜로 보낸다.
pub(crate) async fn prepare(conn: &mut Conn, sql: &str) -> mysql_async::Result<Option<Prepared>> {
    match conn.prep(sql).await {
        Ok(prepared) => Ok(Some(prepared)),
        Err(e) if server_code(&e) == Some(code::UNSUPPORTED_PS) => Ok(None),
        Err(e) => Err(e),
    }
}

/// 결과 첫 줄의 첫 열(실행 계획). 사람의 글이 들어가므로 [`prepare`] 를 지난다.
async fn first_text(conn: &mut Conn, sql: String) -> mysql_async::Result<Option<String>> {
    match prepare(conn, &sql).await? {
        Some(prepared) => {
            let first = conn.exec_first(&prepared, ()).await;
            conn.close(prepared).await?;
            first
        }
        None => conn.query_first(sql).await,
    }
}

fn no_cursor() -> DbError {
    DbError::new(DbErrorKind::Invalid, "there is no open cursor")
}

impl MysqlConnection {
    pub(crate) fn new(
        conn: Conn,
        interrupter: Arc<Interrupter>,
        infile_asked: Arc<AtomicBool>,
        rollback_on_timeout: bool,
    ) -> Self {
        let mut this = Self {
            conn: Some(conn),
            cursor: None,
            interrupter,
            state: TransactionState::Idle,
            infile_asked,
            rolled_back: false,
            rollback_on_timeout,
        };
        this.refresh_state();
        this
    }

    fn conn(&mut self) -> &mut Conn {
        self.conn
            .as_mut()
            .expect("the connection is open until close")
    }

    /// 마지막 OK 패킷의 상태 플래그로 트랜잭션 상태를 읽는다(`SET autocommit = 0` 뒤의 암묵 트랜잭션도 보인다).
    fn refresh_state(&mut self) {
        if let Some(ok) = self.conn().last_ok_packet() {
            self.state = if ok
                .status_flags()
                .contains(StatusFlags::SERVER_STATUS_IN_TRANS)
            {
                TransactionState::InTransaction
            } else {
                TransactionState::Idle
            };
        }
    }

    fn begin_statement(&mut self) {
        self.rolled_back = false;
        // 앞 문장(연결 때의 질의 · 트랜잭션 제어 등)에서 세워진 표지가 이 문장의 것으로 읽히지 않게.
        RefuseLocalInfile::asked(&self.infile_asked);
        *self.interrupter.state() = Statement {
            running: true,
            interrupted: false,
        };
    }

    /// 문장을 마친다. 그 문장이 인터럽트를 받았는지 돌려준다.
    async fn end_statement(&mut self, error: Option<&mysql_async::Error>) -> bool {
        let interrupted = {
            let mut state = self.interrupter.state();
            state.running = false;
            std::mem::take(&mut state.interrupted)
        };
        if interrupted {
            let interrupter = self.interrupter.clone();
            // 가는 중인 `KILL` 이 다음 문장에 닿지 않게 그것이 끝나기를 기다린다(머리 주석).
            let _sent = interrupter.kill.lock().await;
        }
        match error {
            None => self.refresh_state(),
            // 오류 패킷에는 상태 플래그가 없다 — 트랜잭션 안이었으면 `COM_PING` 의 OK 패킷으로 다시 읽는다(머리 주석).
            Some(e) if self.state == TransactionState::InTransaction => {
                if self.conn().ping().await.is_ok() {
                    self.refresh_state();
                }
                let whole = match server_code(e) {
                    Some(DEADLOCK) => true,
                    Some(LOCK_WAIT_TIMEOUT) => self.rollback_on_timeout,
                    _ => false,
                };
                // 끝났는데 되돌리는 오류가 아니면 암묵 커밋이다 — 되돌렸다고 알리지 않는다.
                self.rolled_back = whole && self.state == TransactionState::Idle;
            }
            Some(_) => {}
        }
        interrupted
    }

    /// 문장을 마친 뒤의 오류. 트랜잭션 전체가 되돌려졌으면 그 표지를 싣는다.
    fn error(&mut self, error: mysql_async::Error) -> DbError {
        let mut mapped = map(error, Phase::Run);
        mapped.rolled_back = std::mem::take(&mut self.rolled_back);
        mapped
    }

    /// 방금 끝난 문장에 서버가 로컬 파일을 청했으면 거절 오류(파일은 보내지 않았다 — `infile` 머리 주석).
    fn refused_infile(&self) -> DbResult<()> {
        if RefuseLocalInfile::asked(&self.infile_asked) {
            Err(DbError::new(DbErrorKind::Invalid, infile::REFUSED))
        } else {
            Ok(())
        }
    }

    /// 준비 문장을 닫는다. 남은 결과를 먼저 받아 낸 뒤에 부른다. 닫지 못하면 연결이 깨진 것이고 다음 호출이 그 오류를 낸다.
    async fn release(&mut self, prepared: Option<Prepared>) {
        if let Some(prepared) = prepared {
            let _ = self.conn().close(prepared).await;
        }
    }

    /// 커서를 버린다(머리 주석의 「커서 버리기」).
    async fn discard(&mut self) -> DbResult<()> {
        let Some(cursor) = self.cursor.take() else {
            return Ok(());
        };
        if cursor.grade.is_read() {
            // 보내지 못했으면 남은 행을 다 읽어 버린다 — 느리지만 연결은 그대로다.
            let _ = self.interrupter.interrupt().await;
        }
        let drained = drain(self.conn(), cursor.prepared.is_some()).await;
        self.release(cursor.prepared).await;
        self.end_statement(drained.as_ref().err()).await;
        self.refused_infile()?;
        match drained {
            Ok(()) => Ok(()),
            Err(e)
                if cursor.grade.is_read() && server_code(&e) == Some(code::QUERY_INTERRUPTED) =>
            {
                Ok(())
            }
            Err(e) => Err(self.error(e)),
        }
    }

    /// 행을 돌려주지 않는 문장 하나(트랜잭션 제어 등). 인터럽트를 받는다.
    async fn simple(&mut self, sql: String) -> DbResult<()> {
        self.discard().await?;
        self.begin_statement();
        let result = self.conn().query_drop(sql).await;
        self.end_statement(result.as_ref().err()).await;
        self.refused_infile()?;
        result.map_err(|e| self.error(e))
    }

    /// 메타데이터 · 실행 계획처럼 결과를 통째로 받는 조회 하나. 인터럽트를 받는다.
    async fn whole<T>(
        &mut self,
        query: impl for<'c> FnOnce(&'c mut Conn) -> BoxFuture<'c, mysql_async::Result<T>>,
    ) -> DbResult<T> {
        self.discard().await?;
        self.begin_statement();
        let result = query(self.conn()).await;
        let interrupted = self.end_statement(result.as_ref().err()).await;
        self.refused_infile()?;
        match result {
            Ok(_) if interrupted => Err(DbError::cancelled()),
            Ok(value) => Ok(value),
            Err(e) => Err(self.error(e)),
        }
    }

    async fn start(&mut self, request: ExecuteRequest) -> DbResult<StatementStarted> {
        self.discard().await?;
        let params = request
            .params
            .iter()
            .map(value::param)
            .collect::<DbResult<Vec<_>>>()?;
        self.begin_statement();
        let diagnostic = params.is_empty() && diag::is_diagnostic(&request.sql);
        let prepared = if diagnostic {
            Ok(None)
        } else {
            prepare(self.conn(), &request.sql).await
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(e) => {
                self.end_statement(Some(&e)).await;
                return Err(self.error(e));
            }
        };
        if prepared.is_none() && !params.is_empty() {
            self.end_statement(None).await;
            return Err(DbError::new(
                DbErrorKind::Invalid,
                "the server does not prepare this statement, so it cannot take bind values",
            ));
        }
        let binary = prepared.is_some();
        let conn = self.conn();
        let started = match &prepared {
            Some(prepared) => {
                let params = if params.is_empty() {
                    Params::Empty
                } else {
                    Params::Positional(params)
                };
                conn.exec_iter(prepared, params)
                    .await
                    .map(|result| (result.columns(), result.affected_rows()))
            }
            None => conn
                .query_iter(request.sql.as_str())
                .await
                .map(|result| (result.columns(), result.affected_rows())),
        };
        let (columns, affected) = match started {
            Ok(started) => started,
            Err(e) => {
                // 거절된 LOCAL INFILE 처럼 오류 뒤에도 빈 결과가 남을 수 있다 — 받아 낸 뒤 문장을 닫는다.
                let _ = drain(self.conn(), binary).await;
                self.release(prepared).await;
                self.end_statement(Some(&e)).await;
                self.refused_infile()?;
                return Err(self.error(e));
            }
        };
        match columns.filter(|c| !c.is_empty()) {
            Some(columns) => {
                let metas = columns.iter().map(value::column_meta).collect();
                self.cursor = Some(Cursor {
                    prepared,
                    columns,
                    grade: request.grade,
                    cell_limit: request.cell_limit,
                });
                Ok(StatementStarted {
                    columns: metas,
                    end: None,
                })
            }
            None => {
                // 행이 없는 문장. 뒤따르는 결과(프로시저의 마지막 OK 등)까지 받아 낸다.
                let drained = drain(self.conn(), binary).await;
                self.release(prepared).await;
                let interrupted = self.end_statement(drained.as_ref().err()).await;
                self.refused_infile()?;
                drained.map_err(|e| self.error(e))?;
                if interrupted && request.grade.is_read() {
                    return Err(DbError::cancelled());
                }
                Ok(StatementStarted {
                    columns: Vec::new(),
                    end: Some(StatementEnd {
                        rows_affected: Some(affected),
                    }),
                })
            }
        }
    }

    async fn read(&mut self, limit: FetchLimit) -> DbResult<RowBatch> {
        let cursor = self.cursor.as_ref().ok_or_else(no_cursor)?;
        let (binary, columns, cell_limit, grade) = (
            cursor.prepared.is_some(),
            cursor.columns.clone(),
            cursor.cell_limit,
            cursor.grade,
        );
        let max_rows = usize::try_from(limit.max_rows).unwrap_or(usize::MAX);
        let max_bytes = usize::try_from(limit.max_bytes).unwrap_or(usize::MAX);
        let mut rows = Vec::new();
        let mut bytes = 0_usize;
        while rows.len() < max_rows && (rows.is_empty() || bytes < max_bytes) {
            match next_row(self.conn(), binary).await {
                Ok(Some(row)) => {
                    let cells: Vec<_> = columns
                        .iter()
                        .zip(row.unwrap())
                        .map(|(column, v)| value::cell(column, v, cell_limit))
                        .collect();
                    bytes += cells.iter().map(value::approx_size).sum::<usize>();
                    rows.push(cells);
                }
                Ok(None) => {
                    let prepared = self.cursor.take().and_then(|c| c.prepared);
                    let drained = drain(self.conn(), binary).await;
                    self.release(prepared).await;
                    let interrupted = self.end_statement(drained.as_ref().err()).await;
                    self.refused_infile()?;
                    drained.map_err(|e| self.error(e))?;
                    if interrupted && grade.is_read() {
                        return Err(DbError::cancelled());
                    }
                    return Ok(RowBatch {
                        rows,
                        end: Some(StatementEnd {
                            rows_affected: None,
                        }),
                    });
                }
                Err(e) => {
                    let prepared = self.cursor.take().and_then(|c| c.prepared);
                    self.release(prepared).await;
                    self.end_statement(Some(&e)).await;
                    return Err(self.error(e));
                }
            }
        }
        Ok(RowBatch { rows, end: None })
    }
}

/// 식별자를 backtick 으로 감싼다(안의 backtick 은 두 번).
pub(crate) fn quote(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

impl SqlConnection for MysqlConnection {
    fn interrupter(&self) -> Arc<dyn Interrupt> {
        self.interrupter.clone()
    }

    fn transaction_state(&self) -> TransactionState {
        self.state
    }

    fn read_transaction_state(&mut self) -> BoxFuture<'_, DbResult<TransactionState>> {
        Box::pin(async move {
            // 오류 뒤 상태를 다시 읽는 것과 같은 길 — `COM_PING` 의 OK 패킷 상태 플래그(머리 주석). 문장이 아니라 진단 영역을
            // 건드리지 않는다.
            self.conn().ping().await.map_err(|e| map(e, Phase::Run))?;
            self.refresh_state();
            Ok(self.state)
        })
    }

    fn execute(&mut self, request: ExecuteRequest) -> BoxFuture<'_, DbResult<StatementStarted>> {
        Box::pin(self.start(request))
    }

    fn fetch(&mut self, limit: FetchLimit) -> BoxFuture<'_, DbResult<RowBatch>> {
        Box::pin(self.read(limit))
    }

    fn discard_cursor(&mut self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(self.discard())
    }

    fn begin(&mut self, options: BeginOptions) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            // MySQL 의 `START TRANSACTION` 은 열린 트랜잭션을 말없이 커밋한다 — 사람 모르게 커밋하지 않게 거절한다.
            if self.state != TransactionState::Idle {
                return Err(DbError::new(
                    DbErrorKind::Invalid,
                    "a transaction is already open",
                ));
            }
            let sql = if options.read_only {
                "START TRANSACTION READ ONLY"
            } else {
                "START TRANSACTION"
            };
            self.simple(sql.to_owned()).await
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(self.simple("COMMIT".to_owned()))
    }

    fn rollback(&mut self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(self.simple("ROLLBACK".to_owned()))
    }

    fn savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(self.simple(format!("SAVEPOINT {}", quote(&name))))
    }

    fn rollback_to_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(self.simple(format!("ROLLBACK TO SAVEPOINT {}", quote(&name))))
    }

    fn release_savepoint(&mut self, name: String) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(self.simple(format!("RELEASE SAVEPOINT {}", quote(&name))))
    }

    fn explain(&mut self, sql: String) -> BoxFuture<'_, DbResult<ExplainPlan>> {
        Box::pin(async move {
            let text = self
                .whole(|conn| Box::pin(first_text(conn, format!("EXPLAIN FORMAT=JSON {sql}"))))
                .await?
                .ok_or_else(|| DbError::new(DbErrorKind::Server, "EXPLAIN returned no plan"))?;
            Ok(ExplainPlan {
                format: PlanFormat::Json,
                text,
            })
        })
    }

    fn databases(&mut self) -> BoxFuture<'_, DbResult<Vec<DatabaseInfo>>> {
        Box::pin(self.whole(|conn| Box::pin(meta::databases(conn))))
    }

    fn objects(&mut self, query: ObjectQuery) -> BoxFuture<'_, DbResult<ObjectPage>> {
        Box::pin(async move {
            let limit = meta::page_size(query.limit)?;
            self.whole(move |conn| Box::pin(meta::objects(conn, query, limit)))
                .await
        })
    }

    fn columns(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ColumnInfo>>> {
        Box::pin(self.whole(move |conn| Box::pin(meta::columns(conn, object))))
    }

    fn indexes(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<IndexInfo>>> {
        Box::pin(self.whole(move |conn| Box::pin(meta::indexes(conn, object))))
    }

    fn foreign_keys(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Vec<ForeignKeyInfo>>> {
        Box::pin(self.whole(move |conn| Box::pin(meta::foreign_keys(conn, object))))
    }

    fn ddl(&mut self, object: ObjectRef) -> BoxFuture<'_, DbResult<Ddl>> {
        Box::pin(async move {
            let text = self
                .whole(move |conn| Box::pin(meta::ddl(conn, object)))
                .await?
                .ok_or_else(|| DbError::new(DbErrorKind::Invalid, "no such table or view"))?;
            Ok(Ddl { text })
        })
    }

    fn close(mut self: Box<Self>) -> BoxFuture<'static, DbResult<()>> {
        Box::pin(async move {
            // 조회가 아닌 문장의 남은 행은 읽어 버린다 — 연결을 끊으면 그 쓰기가 되돌려진다.
            let discarded = self.discard().await;
            let conn = self
                .conn
                .take()
                .expect("the connection is open until close");
            conn.disconnect().await.map_err(|e| map(e, Phase::Run))?;
            discarded
        })
    }
}
