//! 실행 파이프라인(M3-05, 결정 M3-D7 · M3-D19 · M3-D21).
//!
//! **실행** 스트림 `db.query.execute { sessionId, sql, statements?, confirmed? }`: 관문(`crate::gate`)을 지난 뒤 문장마다
//! `started { index, range, grade }` → `columns` → `rows`(청크) … → `done { rowsAffected, elapsedMs, truncated }` 또는 `error` 를
//! 낸다(맨 앞에 `accepted { executionId }`). 오류가 난 문장 뒤의 문장은 돌리지 않는다.
//!
//! - **한 번 실행 = 서버 문장 하나**: M3-04 가 나눈 문장을 하나씩 드라이버에 보내고 여러 문장을 한 요청에 싣지 않는다 — 서버 설정으로
//!   나누기 · 분류가 서버와 갈라져도 숨은 둘째 문장이 등급을 비켜 가지 못하고 서버 문법 오류가 된다.
//! - **credit 백프레셔**: 첫 청크는 첫 화면 몫(500행 또는 256 KiB 중 먼저)을 받자마자 보내고, 그 뒤는 창이 받은 만큼만 서버 커서에서
//!   읽는다 — 실행 태스크는 청크 하나를 넘기고 그것이 창으로 나갈 때(credit 을 쓸 때)까지 다음 `fetch` 를 하지 않는다(읽은 행 ≤ 보낸
//!   행 + 한 청크).
//! - **그리드 보유 상한** — 행(`[db] grid_max_rows`, 기본 10만)과 받은 셀 바이트 합(`[db] grid_max_bytes`, 기본 64 MiB — 셀
//!   상한 1 MiB × 10만 행이 웹뷰에 그대로 쌓이지 않게, M3-16 리뷰) 중 먼저 닿는 쪽에서 한 행을 더 읽어 넘는지 보고, 넘으면 드라이버의
//!   「커서 버리기」 를 불러 `truncated: true` — **연결은 닫지 않고 세션의 트랜잭션은 그대로**다. 다시 실행하지 않는다. 드라이버는
//!   `ExecuteRequest.grade` 로 버리는 방법을 고른다(조회가 아니면 끝까지 읽어 버린다). 바이트는 [`cell_bytes`] 로 센다(창의
//!   `ResultBuffer` 가 같은 규칙으로 세어 「상한에서 멈춤(N행 · M MiB)」 을 보인다). 한 묶음은 남은 바이트만큼만 청하지만 드라이버는
//!   행을 통째로 주므로 한 행만큼 넘을 수 있다.
//! - **트랜잭션 제어 문장**(`BEGIN` · `START TRANSACTION` · `COMMIT` · `END` · `ROLLBACK` · `ABORT`, `AND CHAIN` 포함)은 드라이버의
//!   `begin` · `commit` · `rollback` 으로 옮긴다(`pw_db_sql::txn`). 옮길 수 없는 것(`PREPARE TRANSACTION` 등)은 아무것도 돌리기
//!   전에 거절한다.
//! - **수동 커밋**이면 첫 문장 앞에서 트랜잭션을 연다. **read-only 연결**이면 실행이 드라이버의 읽기 전용 트랜잭션 안에서 돈다 —
//!   자동 커밋 탭이면 파이프라인이 열고 실행이 끝나면 롤백한다.
//! - **취소** `db.query.cancel { executionId, confirmRollback? }`(스트림 취소도 같다): 실행 id → 취소 토큰 → 드라이버 인터럽트.
//!   SQLite 수동 트랜잭션 안의 쓰기를 취소하면 트랜잭션 전체가 롤백된다 — 그때는 `confirmRollback` 없이는 `3002` 로 묻는다.
//!   드라이버가 보고한 전체 롤백(`DbError.rolledBack`)으로 세션 상태를 맞춘다.
//! - **예산**: 연결 10 s · 취소 정리 5 s · 비활동 타임아웃(콘솔은 없음, 쪽 단위 실행은 30 s — `pw_db_driver::Budget`).
//! - **진행**은 `db.queryProgress`(프로젝트 범위 이벤트, [`DbQueryProgress`])로도 낸다 — 문장의 시작 · 끝과 그 사이 0.5 s 마다.
//!
//! **쪽 단위** `db.query.page { projectId, worktree?, connectionId, sql, limit }`(결정 M3-D19 — M5 가 `db_query` 툴로 켤 수 있는
//! 꼴): 스트림이 아니다. 문장 하나만, 조회가 아니면 거절하고, 새 연결의 읽기 전용 트랜잭션 안에서 `limit` 행까지 읽고 롤백한 뒤
//! 닫는다. `confirmed` 를 받지 않는다.
//!
//! **분류** `db.sql.classify { projectId, sql, sessionId? | connectionId? | dialect?, statements? }`: 나누기 + 문장마다 분류 +
//! 확인 해시. 콘솔 UI · 실행 관문 · MCP 가 이 하나를 쓴다. 방언은 세션 · 연결의 드라이버 것이 앞서고, `dialect` 는 연결이 없는 편집기
//! 표시용이다.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_db_driver::budget::{Inactivity, within};
use pw_db_driver::{Budget, DbConnection, QueryCancel, SqlConnection};
use pw_db_sql::split::SqlRange;
use pw_db_sql::txn::TxControl;
use pw_db_types::{
    BeginOptions, ColumnMeta, DbError, DbErrorKind, DbValue, ExecuteRequest, FetchLimit, RiskGrade,
    Row, SqlDialect, TransactionState,
};
use pw_rpc::{Ctx, JsNumber, Router, RpcError, StreamSink, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{OwnedMutexGuard, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::gate::{
    DbConfirmed, DbStatementClass, DbTxEffect, Planned, Policy, code, db_grant, grade_of, judge,
    on_classifier_thread, plan, refuse_untranslatable, statements_hash,
};
use crate::session::{
    Current, DbSession, DbSessionMode, DbSessionNotice, DbSessions, Link, Running, Step,
    TOPIC_QUERY_PROGRESS, blocking, db_error, lock,
};

/// 첫 청크 뒤의 읽기 몫.
const NEXT_FETCH: FetchLimit = FetchLimit {
    max_rows: 1000,
    max_bytes: 512 * 1024,
};
/// `db.queryProgress` 를 내는 간격.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);
/// `db.query.page` 의 `limit` 상한.
pub const MAX_PAGE_ROWS: u32 = 10_000;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbQueryExecuteParams {
    pub session_id: String,
    /// 편집기의 SQL(전부 — 범위는 이 글 기준 UTF-16).
    pub sql: String,
    /// 돌릴 문장 번호(`db.sql.split` 의 순서). 없으면 전부.
    #[serde(default)]
    pub statements: Option<Vec<u32>>,
    /// 확인(`3001` 을 받은 뒤 사람의 확인을 거쳐).
    #[serde(default)]
    pub confirmed: Option<DbConfirmed>,
}

/// 실행 스트림의 항목.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbQueryEvent {
    /// 관문을 지났다. `executionId` 로 취소한다.
    Accepted {
        execution_id: String,
        statements: u32,
    },
    Started {
        index: u32,
        range: SqlRange,
        grade: RiskGrade,
    },
    /// 행을 돌려주는 문장의 열.
    Columns {
        index: u32,
        columns: Vec<ColumnMeta>,
    },
    Rows {
        index: u32,
        rows: Vec<Row>,
    },
    Done {
        index: u32,
        #[specta(type = Option<JsNumber>)]
        rows_affected: Option<u64>,
        #[specta(type = JsNumber)]
        elapsed_ms: u64,
        /// 그리드 보유 행 상한에서 끊었다(커서를 버렸다 — 트랜잭션은 그대로).
        truncated: bool,
    },
    /// 문장이 실패했다(취소는 `error.kind = cancelled`). 뒤의 문장은 돌지 않는다.
    Error {
        index: u32,
        error: DbError,
        #[specta(type = JsNumber)]
        elapsed_ms: u64,
    },
}

/// [`TOPIC_QUERY_PROGRESS`] 의 내용.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbQueryProgress {
    pub session_id: String,
    pub execution_id: String,
    pub statement_index: u32,
    /// 이 문장에서 지금까지 받은 행.
    #[specta(type = JsNumber)]
    pub rows: u64,
    #[specta(type = JsNumber)]
    pub elapsed_ms: u64,
    /// 이 문장이 끝났다.
    pub finished: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbQueryCancelParams {
    pub execution_id: String,
    /// 이 취소가 트랜잭션 전체를 되돌린다는 것(`3002`)을 사람이 확인했다.
    #[serde(default)]
    pub confirm_rollback: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbQueryPageParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
    /// 조회 문장 하나.
    pub sql: String,
    /// 돌려줄 행의 상한(1 ~ [`MAX_PAGE_ROWS`]).
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbQueryPage {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Row>,
    /// `limit` 을 넘는 행이 더 있었다.
    pub truncated: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSqlClassifyParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 열린 세션 — 그 연결의 방언으로 분류하고, 해시를 그 세션에 묶는다(실행 관문이 받는 해시).
    #[serde(default)]
    pub session_id: Option<String>,
    /// 연결 — 그 드라이버의 방언으로 분류한다.
    #[serde(default)]
    pub connection_id: Option<String>,
    /// 연결이 없는 편집기 표시용 방언.
    #[serde(default)]
    pub dialect: Option<SqlDialect>,
    pub sql: String,
    #[serde(default)]
    pub statements: Option<Vec<u32>>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSqlClassification {
    pub statements: Vec<DbStatementClass>,
    /// 가장 높은 등급.
    pub grade: RiskGrade,
    /// 실행 관문이 견주는 해시(세션을 주지 않았으면 어느 세션에도 맞지 않는다).
    pub statements_hash: String,
    pub effects: Vec<DbTxEffect>,
}

rpc_method! {
    /// 세션에서 문장들을 돌린다(스트림). 관문: 그 세션을 연 프로젝트 창만 · 신뢰 · 연결 설정이 연 때와 같음 · 등급 정책(확인이
    /// 필요하면 3001, read-only 연결은 조회 밖을 거절). 항목은 `accepted` → 문장마다 `started` → `columns` → `rows` … →
    /// `done` | `error`.
    pub DbQueryExecute: "db.query.execute" {
        params: DbQueryExecuteParams,
        stream: DbQueryEvent,
        tier: Exec,
        roles: [Owner],
    }

    /// 실행을 취소한다(드라이버 인터럽트). 그 세션을 연 프로젝트 창만. SQLite 수동 트랜잭션 안의 쓰기면 `confirmRollback` 없이는
    /// 3002. 끝난 실행이면 아무것도 하지 않는다.
    pub DbQueryCancel: "db.query.cancel" {
        params: DbQueryCancelParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// 쪽 단위 조회(스트림이 아니다): 조회 문장 하나를 새 연결의 읽기 전용 트랜잭션 안에서 `limit` 행까지. 조회가 아니면 거절.
    pub DbQueryPageMethod: "db.query.page" {
        params: DbQueryPageParams,
        result: DbQueryPage,
        tier: Exec,
        roles: [Owner],
    }

    /// SQL 을 나누고 문장마다 위험도를 매긴다(M3-04 + M3-03). 방언은 세션 · 연결의 드라이버 것, 없으면 `dialect`.
    pub DbSqlClassify: "db.sql.classify" {
        params: DbSqlClassifyParams,
        result: DbSqlClassification,
        tier: Read,
        roles: [Owner],
    }
}

/// 실행 태스크가 창으로 보내는 길. 청크 하나를 넘기면 그것이 창으로 나갈 때까지 기다린다(credit).
struct Out {
    /// 용량 2: 확인을 기다리는 항목 하나 + 마지막 항목 하나.
    tx: mpsc::Sender<(DbQueryEvent, Option<oneshot::Sender<()>>)>,
    stop: CancellationToken,
}

/// 실행이 멈췄다(취소 · 창이 스트림을 버림).
struct Halted;

impl Out {
    async fn emit(&self, event: DbQueryEvent) -> Result<(), Halted> {
        if self.stop.is_cancelled() {
            return Err(Halted);
        }
        let (ack, acked) = oneshot::channel();
        self.tx.send((event, Some(ack))).await.map_err(|_| Halted)?;
        tokio::select! {
            biased;
            sent = acked => sent.map_err(|_| Halted),
            () = self.stop.cancelled() => Err(Halted),
        }
    }

    /// 마지막 항목: 기다리지 않는다(창이 credit 을 주면 나간다).
    fn last(&self, event: DbQueryEvent) {
        let _ = self.tx.try_send((event, None));
    }
}

/// 문장 하나의 끝.
enum Outcome {
    Done {
        rows_affected: Option<u64>,
        truncated: bool,
    },
    Failed(DbError),
}

fn disconnected() -> DbError {
    DbError::new(DbErrorKind::Disconnected, "the connection is closed")
}

/// 스트림이 버려지면(창이 취소 · 연결이 닫힘) 실행을 멈춘다.
struct HaltOnDrop(Option<Arc<Running>>);

impl Drop for HaltOnDrop {
    fn drop(&mut self) {
        if let Some(running) = self.0.take() {
            running.stop.cancel();
            tokio::spawn(async move { running.halt().await });
        }
    }
}

/// 스크립트에 트랜잭션 효과가 있는가: 열린 트랜잭션(또는 수동 커밋 · 앞의 `BEGIN`) 안에서 암묵 커밋 문장이 돈다.
fn effects_of(statements: &[Planned], in_transaction: bool, manual: bool) -> Vec<DbTxEffect> {
    let mut open = in_transaction || manual;
    for statement in statements {
        match statement.control {
            Some(TxControl::Begin { .. }) => open = true,
            Some(TxControl::Commit { chain } | TxControl::Rollback { chain }) => {
                open = chain || manual;
            }
            _ if statement.implicit_commit && open => return vec![DbTxEffect::ImplicitCommit],
            _ => {}
        }
    }
    Vec::new()
}

impl DbSessions {
    /// `db.query.execute`.
    pub(crate) async fn execute(
        self: Arc<Self>,
        ctx: Ctx,
        params: DbQueryExecuteParams,
        sink: StreamSink<DbQueryEvent>,
    ) -> Result<(), RpcError> {
        let (session, _grant) = self.session_for(&ctx, &params.session_id)?;
        let Some(dialect) = session.dialect() else {
            return Err(RpcError::invalid_params("this connection does not run SQL"));
        };
        self.recheck(&session).await?;
        let (sql, selected) = (params.sql, params.statements);
        let planned =
            on_classifier_thread(move || plan(dialect, &sql, selected.as_deref())).await??;
        refuse_untranslatable(&planned)?;
        if session.read_only && !session.driver.capabilities.read_only_transaction {
            return Err(RpcError::new(
                rpc_code::PERMISSION_DENIED,
                "this read-only connection's driver has no read-only transactions",
            )
            .with_data(json!({ "reason": "readOnlyConnection" })));
        }
        let mut link = Arc::clone(&session.link).try_lock_owned().map_err(|_| {
            RpcError::new(rpc_code::CONFLICT, "the session is running statements")
                .with_data(json!({ "reason": "busy" }))
        })?;
        // 실행 사이에 드라이버 상태가 바뀌었으면(버려진 문장이 나중에 끝남) 서버에 물어 맞춘다 — 까닭은 모른다.
        let notice = link.catch_up().await;
        if notice.is_some() {
            session.sync(&link);
            self.publish(&session, notice);
        }
        let Some(conn) = link.sql() else {
            return Err(db_error(disconnected()));
        };
        let interrupter = conn.interrupter();
        let in_transaction = link.tx != TransactionState::Idle && !link.owned_read_only;
        let effects = effects_of(&planned, in_transaction, link.mode == DbSessionMode::Manual);
        let hash = statements_hash(Some(&session.confirm_key()), dialect, &planned, &effects);
        let policy = Policy {
            read_only: session.read_only,
            production: session.production,
            name: session.name.clone(),
        };
        judge(
            &policy,
            &planned,
            &effects,
            &hash,
            params.confirmed.as_ref(),
        )?;
        // 관문을 지났다 — 낸 확인은 이것으로 끝이다(재사용 · 늦게 온 확인 거절).
        session.spend_confirmations();

        let execution_id = self.next_id("dbx-");
        let running = Arc::new(Running {
            execution_id: execution_id.clone(),
            session_id: session.id.clone(),
            project_id: session.project_id.clone(),
            dialect: Some(dialect),
            cancel: QueryCancel::new(interrupter, Budget::CONSOLE.cancel_cleanup),
            stop: CancellationToken::new(),
            current: std::sync::Mutex::new(None),
        });
        lock(&self.executions).insert(execution_id.clone(), Arc::clone(&running));
        *lock(&session.running) = Some(Arc::clone(&running));
        tracing::info!(target: "pw_db", session = %session.id, execution = %execution_id, statements = planned.len(), grade = ?grade_of(&planned), "running db statements");

        let (tx, mut rx) = mpsc::channel(2);
        let out = Out {
            tx,
            stop: running.stop.clone(),
        };
        let limit = GridLimit {
            rows: self.grid_max_rows(),
            bytes: self.grid_max_bytes(),
        };
        let task = tokio::spawn(run_script(
            Arc::clone(&self),
            Arc::clone(&session),
            link,
            planned,
            Arc::clone(&running),
            out,
            limit,
        ));
        let mut guard = HaltOnDrop(Some(running));
        while let Some((event, ack)) = rx.recv().await {
            sink.send(event).await?;
            if let Some(ack) = ack {
                let _ = ack.send(());
            }
        }
        guard.0 = None;
        let _ = task.await;
        Ok(())
    }

    /// `db.query.cancel`.
    pub(crate) async fn cancel(
        &self,
        ctx: &Ctx,
        params: DbQueryCancelParams,
    ) -> Result<(), RpcError> {
        let Some(running) = lock(&self.executions).get(&params.execution_id).cloned() else {
            return Ok(());
        };
        db_grant(ctx.scope(), &running.project_id)?;
        let current = *lock(&running.current);
        if running.dialect == Some(SqlDialect::Sqlite)
            && !params.confirm_rollback
            && current.is_some_and(|c| !c.grade.is_read() && c.in_transaction)
        {
            return Err(RpcError::new(
                code::CANCEL_ROLLS_BACK,
                "cancelling this statement rolls back the whole transaction",
            )
            .with_data(json!({
                "executionId": running.execution_id,
                "sessionId": running.session_id,
                "reason": "rollsBackTransaction",
            })));
        }
        tracing::info!(target: "pw_db", execution = %running.execution_id, "cancelling db statements");
        running.halt().await;
        Ok(())
    }

    fn progress(
        &self,
        session: &DbSession,
        running: &Running,
        index: u32,
        rows: u64,
        started: Instant,
        finished: bool,
    ) {
        let Some(bus) = self.bus() else { return };
        let payload = DbQueryProgress {
            session_id: session.id.clone(),
            execution_id: running.execution_id.clone(),
            statement_index: index,
            rows,
            elapsed_ms: started.elapsed().as_millis() as u64,
            finished,
        };
        let _ = bus.publish(
            pw_core::bus::Scope::Project(session.project_id.clone()),
            TOPIC_QUERY_PROGRESS,
            &payload,
        );
    }

    /// `db.query.page`.
    pub(crate) async fn page(
        &self,
        ctx: &Ctx,
        params: DbQueryPageParams,
    ) -> Result<DbQueryPage, RpcError> {
        let grant = db_grant(ctx.scope(), &params.project_id)?;
        if params.limit == 0 || params.limit > MAX_PAGE_ROWS {
            return Err(RpcError::invalid_params(format!(
                "limit must be 1-{MAX_PAGE_ROWS}"
            )));
        }
        // 방언을 먼저 정하고 분류한다 — 거절할 문장이면 연결을 열지 않는다(비밀도 읽지 않는다).
        let dialect = self
            .connection_dialect(
                &params.project_id,
                params.worktree.as_deref(),
                &params.connection_id,
                true,
            )
            .await?;
        let sql = params.sql.clone();
        let planned = on_classifier_thread(move || plan(dialect, &sql, None)).await??;
        let [statement] = planned.as_slice() else {
            return Err(RpcError::invalid_params(
                "db.query.page runs exactly one statement",
            ));
        };
        if statement.grade != RiskGrade::Read || statement.settings || statement.control.is_some() {
            return Err(RpcError::new(
                rpc_code::PERMISSION_DENIED,
                "db.query.page runs queries only",
            )
            .with_data(json!({ "reason": "notAQuery", "statements": [statement.class()] })));
        }
        let opened = crate::connection::connect(
            self,
            &grant,
            params.worktree.as_deref(),
            &params.connection_id,
            None,
        )
        .await?;
        // 터널은 연결을 닫을 때까지 쥔다(이 함수 끝에서 놓는다).
        let _tunnel = opened.tunnel;
        let mut conn = match opened.connection {
            DbConnection::Sql(conn) => conn,
            other => {
                let _ = other.close().await;
                return Err(RpcError::invalid_params("this connection does not run SQL"));
            }
        };
        if !opened.driver.capabilities.read_only_transaction {
            let _ = conn.close().await;
            return Err(RpcError::new(
                rpc_code::PERMISSION_DENIED,
                "this driver has no read-only transactions",
            )
            .with_data(json!({ "reason": "readOnlyConnection" })));
        }
        let result = read_page(conn.as_mut(), statement, params.limit).await;
        let _ = within(Budget::PAGE.cancel_cleanup, "rollback", conn.rollback()).await;
        let _ = within(Budget::PAGE.connect, "close", conn.close()).await;
        result.map_err(db_error)
    }

    /// 연결의 드라이버 방언(신뢰 판정을 지나야 하면 `trusted`).
    async fn connection_dialect(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        connection_id: &str,
        trusted: bool,
    ) -> Result<SqlDialect, RpcError> {
        let c = Arc::clone(&self.connections);
        let (project, worktree, id) = (
            project_id.to_owned(),
            worktree.map(str::to_owned),
            connection_id.to_owned(),
        );
        let driver = blocking(move || {
            if trusted {
                c.trust_gate(&project, worktree.as_deref())?;
            }
            let loaded = c.load(&project, worktree.as_deref())?;
            let connection = loaded
                .get(&id)
                .ok_or_else(|| crate::config::not_found(&id))?;
            let config = connection.config.clone().map_err(|e| {
                RpcError::invalid_params(format!("db connection `{id}` is invalid: {e}"))
            })?;
            Ok(config.driver)
        })
        .await?;
        self.drivers
            .get(driver.id())
            .and_then(|d| d.dialect())
            .ok_or_else(|| {
                RpcError::invalid_params(format!(
                    "db connection `{connection_id}` does not run SQL"
                ))
            })
    }

    /// `db.sql.classify`.
    pub(crate) async fn classify(
        &self,
        ctx: &Ctx,
        params: DbSqlClassifyParams,
    ) -> Result<DbSqlClassification, RpcError> {
        let (dialect, session_id, in_transaction, manual) = if let Some(id) = &params.session_id {
            let (session, _grant) = self.session_for(ctx, id)?;
            if session.project_id != params.project_id {
                return Err(RpcError::invalid_params(format!(
                    "db session `{id}` belongs to another project"
                )));
            }
            let dialect = session
                .dialect()
                .ok_or_else(|| RpcError::invalid_params("this connection does not run SQL"))?;
            let info = session.info();
            let open = !matches!(info.state, crate::session::DbSessionState::Idle);
            (
                dialect,
                Some(session.confirm_key()),
                open,
                info.mode == DbSessionMode::Manual,
            )
        } else if let Some(id) = &params.connection_id {
            let dialect = self
                .connection_dialect(&params.project_id, params.worktree.as_deref(), id, false)
                .await?;
            (dialect, None, false, false)
        } else if let Some(dialect) = params.dialect {
            (dialect, None, false, false)
        } else {
            return Err(RpcError::invalid_params(
                "give sessionId, connectionId or dialect",
            ));
        };
        let (sql, selected) = (params.sql, params.statements);
        let planned =
            on_classifier_thread(move || plan(dialect, &sql, selected.as_deref())).await??;
        let effects = effects_of(&planned, in_transaction, manual);
        Ok(DbSqlClassification {
            grade: grade_of(&planned),
            statements_hash: statements_hash(session_id.as_deref(), dialect, &planned, &effects),
            statements: planned.iter().map(Planned::class).collect(),
            effects,
        })
    }
}

/// 쪽 하나를 읽는다: 읽기 전용 트랜잭션 · 행 제한 · 비활동 30 s.
async fn read_page(
    conn: &mut dyn SqlConnection,
    statement: &Planned,
    limit: u32,
) -> Result<DbQueryPage, DbError> {
    let mut inactivity = Inactivity::new(Budget::PAGE.inactivity);
    inactivity
        .guard(conn.begin(BeginOptions { read_only: true }))
        .await?;
    let started = inactivity
        .guard(conn.execute(ExecuteRequest::new(statement.text.clone(), RiskGrade::Read)))
        .await?;
    let mut page = DbQueryPage {
        columns: started.columns,
        rows: Vec::new(),
        truncated: false,
    };
    if started.end.is_some() {
        return Ok(page);
    }
    loop {
        // 한 행을 더 읽어 넘는지 본다.
        let room = limit as usize + 1 - page.rows.len();
        let batch = inactivity
            .guard(conn.fetch(FetchLimit {
                max_rows: room.min(NEXT_FETCH.max_rows as usize) as u32,
                max_bytes: NEXT_FETCH.max_bytes,
            }))
            .await?;
        let empty = batch.rows.is_empty();
        page.rows.extend(batch.rows);
        if page.rows.len() > limit as usize {
            page.rows.truncate(limit as usize);
            page.truncated = true;
            if batch.end.is_none() {
                within(
                    Budget::PAGE.cancel_cleanup,
                    "discard",
                    conn.discard_cursor(),
                )
                .await?;
            }
            return Ok(page);
        }
        if batch.end.is_some() {
            return Ok(page);
        }
        if empty {
            return Err(DbError::new(
                DbErrorKind::Invalid,
                "the driver returned no rows",
            ));
        }
    }
}

/// 그리드 보유 상한(결정 M3-D7 의 행 상한 + M3-16 리뷰의 바이트 상한). 먼저 닿는 쪽에서 커서를 버린다.
#[derive(Debug, Clone, Copy)]
struct GridLimit {
    rows: u64,
    bytes: u64,
}

/// 그리드 보유 바이트 상한이 세는 셀 하나의 크기: 글(텍스트 · decimal · json · 시각 · uuid · 잘린 값의 앞부분 · 그 밖 값의 글)은 UTF-8
/// 바이트, 바이트 값은 받은 hex 글의 길이, 배열은 요소의 합 + 2, 그 밖(NULL · 참거짓 · 수 · 문자열로 온 큰 정수)은 8.
/// 창(`apps/desktop/src/db/grid/result-buffer.ts` 의 `cellBytes`)이 같은 규칙으로 센다 — 바꾸면 둘을 함께 바꾼다.
pub fn cell_bytes(value: &DbValue) -> u64 {
    let n = match value {
        DbValue::Text(s)
        | DbValue::Decimal(s)
        | DbValue::Json(s)
        | DbValue::Datetime(s)
        | DbValue::Uuid(s) => s.len(),
        DbValue::Bytes(b) => b.hex.len(),
        DbValue::Truncated(t) => t.prefix.len(),
        DbValue::Other(o) => o.text.len(),
        DbValue::Array(items) => return items.iter().map(cell_bytes).sum::<u64>() + 2,
        _ => 8,
    };
    n as u64
}

/// 실행 태스크: 세션 잠금을 쥐고 문장을 하나씩 돌린다.
async fn run_script(
    sessions: Arc<DbSessions>,
    session: Arc<DbSession>,
    mut link: OwnedMutexGuard<Link>,
    planned: Vec<Planned>,
    running: Arc<Running>,
    out: Out,
    limit: GridLimit,
) {
    let accepted = DbQueryEvent::Accepted {
        execution_id: running.execution_id.clone(),
        statements: planned.len() as u32,
    };
    let mut halted = out.emit(accepted).await.is_err();
    for statement in &planned {
        if halted {
            break;
        }
        let started = Instant::now();
        if out
            .emit(DbQueryEvent::Started {
                index: statement.index,
                range: statement.range,
                grade: statement.grade,
            })
            .await
            .is_err()
        {
            halted = true;
            break;
        }
        sessions.progress(&session, &running, statement.index, 0, started, false);
        let (outcome, notice, stopped) = run_statement(
            &sessions, &session, &mut link, statement, &running, &out, limit, started,
        )
        .await;
        *lock(&running.current) = None;
        session.sync(&link);
        if notice.is_some() {
            sessions.publish(&session, notice);
        }
        // 스키마 캐시(M3-10): DDL 이상은 실패해도 버린다 — MySQL 은 실패한 DDL 도 암묵 커밋한다.
        if statement.grade >= RiskGrade::Ddl
            || matches!(statement.control, Some(TxControl::Commit { .. }))
        {
            sessions.schema_touched(&session);
        }
        let elapsed_ms = started.elapsed().as_millis() as u64;
        sessions.progress(&session, &running, statement.index, 0, started, true);
        match outcome {
            Outcome::Done {
                rows_affected,
                truncated,
            } => {
                let done = DbQueryEvent::Done {
                    index: statement.index,
                    rows_affected,
                    elapsed_ms,
                    truncated,
                };
                if stopped || out.emit(done).await.is_err() {
                    halted = true;
                }
            }
            Outcome::Failed(error) => {
                tracing::info!(target: "pw_db", execution = %running.execution_id, index = statement.index, kind = ?error.kind, code = ?error.code, "db statement failed");
                out.last(DbQueryEvent::Error {
                    index: statement.index,
                    error,
                    elapsed_ms,
                });
                break;
            }
        }
        if halted {
            // 멈춘 실행의 남은 문장은 돌리지 않는다. 지금 문장이 성공했으면 그 끝을 남긴다.
            break;
        }
    }
    if halted && out.stop.is_cancelled() {
        tracing::info!(target: "pw_db", execution = %running.execution_id, "db statements stopped");
    }
    // 파이프라인이 연 read-only 트랜잭션을 닫는다.
    if link.owned_read_only {
        if let Some(conn) = link.sql() {
            let _ = within(Budget::CONSOLE.cancel_cleanup, "rollback", conn.rollback()).await;
        }
        let _ = link.reconcile(Step::Rollback, None);
    }
    session.sync(&link);
    drop(link);
    sessions.publish(&session, None);
    lock(&sessions.executions).remove(&running.execution_id);
    let mut slot = lock(&session.running);
    if slot
        .as_ref()
        .is_some_and(|r| r.execution_id == running.execution_id)
    {
        *slot = None;
    }
}

/// 단계의 끝을 세션에 반영한다(끊김 · 트랜잭션 끝남 — 오류 뒤에는 서버에 물어). 알릴 것을 돌려준다.
async fn settle(
    link: &mut Link,
    step: Step,
    error: Option<&mut DbError>,
) -> Option<DbSessionNotice> {
    link.settle(step, error).await
}

/// 문장 하나. (끝, 알릴 것, 멈춤 — 창이 받기를 멈췄거나 취소됐다).
#[allow(
    clippy::too_many_arguments,
    reason = "one statement's whole context; split helpers would pass the same values"
)]
async fn run_statement(
    sessions: &DbSessions,
    session: &DbSession,
    link: &mut Link,
    statement: &Planned,
    running: &Running,
    out: &Out,
    grid: GridLimit,
    started: Instant,
) -> (Outcome, Option<DbSessionNotice>, bool) {
    if let Some(control) = statement.control {
        return run_control(session, link, control, running).await;
    }
    if let Err(mut e) = running
        .cancel
        .run(link.ensure_transaction(session.read_only))
        .await
    {
        let notice = settle(link, Step::Begin, Some(&mut e)).await;
        return (Outcome::Failed(e), notice, false);
    }
    let implicit = Step::Statement {
        implicit_commit: statement.implicit_commit,
        may_end: statement.may_end,
    };
    *lock(&running.current) = Some(Current {
        grade: statement.grade,
        in_transaction: link.tx != TransactionState::Idle && !link.owned_read_only,
    });
    let Some(conn) = link.sql() else {
        return (Outcome::Failed(disconnected()), None, false);
    };
    let request = ExecuteRequest::new(statement.text.clone(), statement.grade);
    let begun = running.cancel.run(conn.execute(request)).await;
    let begun = match begun {
        Ok(begun) => begun,
        Err(mut e) => {
            let notice = settle(link, implicit, Some(&mut e)).await;
            return (Outcome::Failed(e), notice, false);
        }
    };
    if let Some(end) = begun.end {
        let notice = settle(link, implicit, None).await;
        return (
            Outcome::Done {
                rows_affected: end.rows_affected,
                truncated: false,
            },
            notice,
            false,
        );
    }
    let index = statement.index;
    if out
        .emit(DbQueryEvent::Columns {
            index,
            columns: begun.columns,
        })
        .await
        .is_err()
    {
        return stop_cursor(link, implicit).await;
    }
    let mut sent: u64 = 0;
    let mut sent_bytes: u64 = 0;
    let mut limit = FetchLimit::FIRST_SCREEN;
    let mut last_progress = Instant::now();
    loop {
        let Some(conn) = link.sql() else {
            return (Outcome::Failed(disconnected()), None, false);
        };
        let room = grid.rows.saturating_sub(sent);
        let room_bytes = grid.bytes.saturating_sub(sent_bytes);
        if room == 0 || room_bytes == 0 {
            // 상한(행 · 바이트 중 먼저 닿은 쪽)에 닿았다: 한 행을 더 읽어 넘는지 본다. 넘으면 커서만 버린다 — 연결 · 트랜잭션은 그대로.
            let probe = running.cancel.run(conn.fetch(FetchLimit::rows(1))).await;
            match probe {
                Err(mut e) => {
                    let notice = settle(link, implicit, Some(&mut e)).await;
                    return (Outcome::Failed(e), notice, false);
                }
                Ok(batch) if batch.rows.is_empty() && batch.end.is_some() => {
                    let notice = settle(link, implicit, None).await;
                    let rows_affected = batch.end.and_then(|e| e.rows_affected);
                    return (
                        Outcome::Done {
                            rows_affected,
                            truncated: false,
                        },
                        notice,
                        false,
                    );
                }
                Ok(_) => {
                    let discarded = running.cancel.run(conn.discard_cursor()).await;
                    if let Err(mut e) = discarded {
                        let notice = settle(link, implicit, Some(&mut e)).await;
                        return (Outcome::Failed(e), notice, false);
                    }
                    tracing::info!(target: "pw_db", session = %session.id, rows = sent, bytes = sent_bytes, "db result truncated at the grid limit");
                    let notice = settle(link, implicit, None).await;
                    return (
                        Outcome::Done {
                            rows_affected: None,
                            truncated: true,
                        },
                        notice,
                        false,
                    );
                }
            }
        }
        let want = FetchLimit {
            max_rows: room.min(u64::from(limit.max_rows)) as u32,
            max_bytes: room_bytes.min(u64::from(limit.max_bytes)) as u32,
        };
        let batch = match running.cancel.run(conn.fetch(want)).await {
            Ok(batch) => batch,
            Err(mut e) => {
                let notice = settle(link, implicit, Some(&mut e)).await;
                return (Outcome::Failed(e), notice, false);
            }
        };
        let count = batch.rows.len() as u64;
        let bytes: u64 = batch
            .rows
            .iter()
            .flat_map(|row| row.iter())
            .map(cell_bytes)
            .sum();
        if count > 0
            && out
                .emit(DbQueryEvent::Rows {
                    index,
                    rows: batch.rows,
                })
                .await
                .is_err()
        {
            if batch.end.is_some() {
                let notice = settle(link, implicit, None).await;
                return (
                    Outcome::Done {
                        rows_affected: batch.end.and_then(|e| e.rows_affected),
                        truncated: false,
                    },
                    notice,
                    true,
                );
            }
            return stop_cursor(link, implicit).await;
        }
        sent += count;
        sent_bytes = sent_bytes.saturating_add(bytes);
        if last_progress.elapsed() >= PROGRESS_EVERY {
            last_progress = Instant::now();
            sessions.progress(session, running, index, sent, started, false);
        }
        if let Some(end) = batch.end {
            let notice = settle(link, implicit, None).await;
            return (
                Outcome::Done {
                    rows_affected: end.rows_affected,
                    truncated: false,
                },
                notice,
                false,
            );
        }
        if count == 0 {
            return (
                Outcome::Failed(DbError::new(
                    DbErrorKind::Invalid,
                    "the driver returned no rows and no end",
                )),
                None,
                false,
            );
        }
        limit = NEXT_FETCH;
    }
}

/// 창이 받기를 멈췄거나 취소됐다: 열린 커서를 버리고(연결 · 트랜잭션은 그대로) 취소로 끝낸다.
async fn stop_cursor(link: &mut Link, step: Step) -> (Outcome, Option<DbSessionNotice>, bool) {
    let mut error = DbError::cancelled();
    if let Some(conn) = link.sql() {
        let discarded = within(
            Budget::CONSOLE.cancel_cleanup,
            "discard",
            conn.discard_cursor(),
        )
        .await;
        if let Err(e) = discarded {
            error = e;
        }
    }
    let notice = settle(link, step, Some(&mut error)).await;
    (Outcome::Failed(error), notice, true)
}

/// 트랜잭션 제어 문장을 드라이버 메서드로.
async fn run_control(
    session: &DbSession,
    link: &mut Link,
    control: TxControl,
    running: &Running,
) -> (Outcome, Option<DbSessionNotice>, bool) {
    let done = Outcome::Done {
        rows_affected: None,
        truncated: false,
    };
    let Some(conn) = link.sql() else {
        return (Outcome::Failed(disconnected()), None, false);
    };
    let (result, step, chain) = match control {
        TxControl::Begin { read_only } => {
            let options = BeginOptions {
                read_only: read_only.unwrap_or(false) || session.read_only,
            };
            (
                running.cancel.run(conn.begin(options)).await,
                Step::Begin,
                false,
            )
        }
        TxControl::Commit { chain } => {
            (running.cancel.run(conn.commit()).await, Step::Commit, chain)
        }
        TxControl::Rollback { chain } => (
            running.cancel.run(conn.rollback()).await,
            Step::Rollback,
            chain,
        ),
        TxControl::Refused(why) => {
            return (
                Outcome::Failed(DbError::new(DbErrorKind::Invalid, why)),
                None,
                false,
            );
        }
    };
    let mut error = result.err();
    let notice = settle(link, step, error.as_mut()).await;
    if let Some(error) = error {
        return (Outcome::Failed(error), notice, false);
    }
    if chain {
        let Some(conn) = link.sql() else {
            return (Outcome::Failed(disconnected()), notice, false);
        };
        let begun = running
            .cancel
            .run(conn.begin(BeginOptions {
                read_only: session.read_only,
            }))
            .await;
        let mut error = begun.err();
        let chained = settle(link, Step::Begin, error.as_mut()).await;
        if let Some(error) = error {
            return (Outcome::Failed(error), notice.or(chained), false);
        }
    }
    (done, notice, false)
}

pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    let (execute, cancel, page, classify) = (
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        sessions,
    );
    router
        .stream(
            DbQueryExecute,
            move |ctx, params: DbQueryExecuteParams, sink| {
                Arc::clone(&execute).execute(ctx, params, sink)
            },
        )
        .unary(DbQueryCancel, move |ctx, params: DbQueryCancelParams| {
            let s = Arc::clone(&cancel);
            async move { s.cancel(&ctx, params).await }
        })
        .unary(DbQueryPageMethod, move |ctx, params: DbQueryPageParams| {
            let s = Arc::clone(&page);
            async move { s.page(&ctx, params).await }
        })
        .unary(DbSqlClassify, move |ctx, params: DbSqlClassifyParams| {
            let s = Arc::clone(&classify);
            async move { s.classify(&ctx, params).await }
        });
}
