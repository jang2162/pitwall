//! 세션(M3-05, 결정 M3-D21): 콘솔 탭 하나 = 고정 연결 하나.
//!
//! - **열기** `db.session.open { projectId, worktree?, connectionId, database? }` → `sessionId`. 관문의 연결 범위([`DbGrant`])와
//!   신뢰 게이트를 지나 연결 설정을 풀고(`crate::connection`) 드라이버 연결을 하나 연다. 세션은 그 연결을 닫을 때까지 쥔다.
//! - **자동 커밋 / 수동 커밋**(`db.session.setMode`): 수동이면 파이프라인이 첫 문장 앞에서 트랜잭션을 열고, 그 트랜잭션은
//!   `db.session.commit` · `rollback` 전까지 그대로다 — **쉬어도 저절로 롤백하지 않는다**, 행 상한 끊기 · 편집 실패도 끝내지 않는다.
//!   열린 트랜잭션이 있으면 자동으로 바꾸지 않는다(몰래 커밋하지 않는다 — `CONFLICT`).
//! - **세션 상태** `idle` · `inTransaction { since }` · `failed { since }`(PG 처럼 오류 뒤 롤백만 되는 트랜잭션) · `disconnected` ·
//!   `configChanged` 를 `db.sessionChanged`(프로젝트 범위 이벤트, [`DbSessionChanged`])로 낸다. 드라이버가 보고하는 상태 변화 —
//!   `DbError::rolled_back`, PG 의 숨은 트랜잭션 블록(탭에 묶인 연결), MySQL 의 암묵 커밋, SQLite 쓰기 취소의 전체 롤백, 끊긴 연결 —
//!   는 [`DbSessionNotice`] 로 함께 실어 사람에게 알린다. 몰래 커밋도 몰래 롤백도 하지 않는다.
//! - **세션 · 실행 id 의 범위**: id 를 받는 RPC 는 그 세션을 연 프로젝트의 창(연결 범위 `Project(그 id)`)에서만 — 다른 창이 id 를
//!   알아도 `PERMISSION_DENIED` 다.
//! - **창이 닫히면** 그 창(RPC 연결)이 연 세션을 모두 닫는다(`Ctx::connection_closed`). 닫기 전에 묻는 것은 화면의 몫이다(M3-15).
//! - **설정이 바뀐 연결**은 저절로 다시 붙지 않고 `configChanged` 가 된다 — `config.changed` 이벤트로 알고, 감시 밖의 변경(메인
//!   체크아웃의 `.local.toml` 을 워크트리 창이 못 들을 때 등)은 실행마다 설정을 다시 읽어 연 때와 견줘 잡는다. 그 세션은 새 실행을
//!   받지 않고 커밋 · 롤백 · 닫기만 받는다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pw_core::bus::{EventBus, Filter, Received, Scope, Subscription};
use pw_core::config::ConfigStore;
use pw_db_driver::budget::within;
use pw_db_driver::{DbConnection, QueryCancel};
use pw_db_types::{
    BeginOptions, DbError, DbErrorKind, DriverInfo, RiskGrade, ServerInfo, SqlDialect,
    TransactionState,
};
use pw_rpc::{Ctx, JsNumber, Router, RpcError, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::config::{Connections, DbConnectionConfig, FILE_PREFIX, TOPIC_CHANGED};
use crate::gate::{DbGrant, code, db_grant};
use crate::registry::DriverRegistry;

/// 세션 상태가 바뀌었다. 내용은 [`DbSessionChanged`].
pub const TOPIC_SESSION_CHANGED: &str = "db.sessionChanged";
/// 실행의 진행. 내용은 [`DbQueryProgress`](crate::execute::DbQueryProgress).
pub const TOPIC_QUERY_PROGRESS: &str = "db.queryProgress";

/// 그리드 보유 행 상한의 설정 키(전역 `settings` 의 `[db] grid_max_rows`)와 기본값(06 「성능 목표」 의 10만 행).
pub const GRID_MAX_ROWS_KEY: &str = "db.grid_max_rows";
pub const DEFAULT_GRID_MAX_ROWS: u64 = 100_000;

/// 연결을 닫을 때 드라이버를 기다리는 시간.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbSessionMode {
    /// 자동 커밋: 문장마다 커밋된다(사람이 친 `BEGIN` 은 그 `COMMIT` · `ROLLBACK` 까지 트랜잭션을 연다).
    #[default]
    Auto,
    /// 수동 커밋: 파이프라인이 첫 문장 앞에서 트랜잭션을 열고 [커밋] · [롤백] 까지 그대로 둔다.
    Manual,
}

/// 열린 트랜잭션(설정이 바뀐 세션에서 그대로 남은 것).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbOpenTransaction {
    /// 연 시각(유닉스 밀리초).
    #[specta(type = JsNumber)]
    pub since: u64,
    /// 오류 뒤 롤백만 되는 트랜잭션(PG).
    pub failed: bool,
}

/// 세션 상태.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbSessionState {
    Idle,
    /// 트랜잭션이 열려 있다. `since` 는 연 시각(유닉스 밀리초 — 상태바의 열린 시간).
    InTransaction {
        #[specta(type = JsNumber)]
        since: u64,
    },
    /// 오류 · 취소 뒤 롤백만 되는 트랜잭션(PG). 커밋해도 롤백된다.
    Failed {
        #[specta(type = JsNumber)]
        since: u64,
    },
    /// 연결이 끊겼다 — 열린 트랜잭션은 DB 가 롤백했다. 다시 붙는 것은 사람이 새 세션을 열 때다.
    Disconnected,
    /// 연결 설정이 바뀌었다 — 저절로 다시 붙지 않는다(「설정 바뀜 — 다시 연결」). 새 실행은 받지 않고, 열린 트랜잭션은 커밋 · 롤백할
    /// 수 있다.
    ConfigChanged {
        transaction: Option<DbOpenTransaction>,
    },
}

/// 사람에게 알릴 상태 변화(드라이버의 보고에서).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbSessionNotice {
    /// 서버 · 드라이버가 트랜잭션 전체를 되돌렸다(교착, 실패한 트랜잭션의 커밋, SQLite 쓰기 취소 등). `message` 는 그 오류.
    RolledBack { message: String },
    /// MySQL · MariaDB 의 문장이 열린 트랜잭션을 암묵적으로 커밋했다(문장이 실패했어도).
    CommittedImplicitly,
    /// 파이프라인이 열지 않은 트랜잭션이 열렸다(PG 의 숨은 블록, MySQL `SET autocommit = 0` 등) — 그 연결은 이 탭에 묶이고 커밋 ·
    /// 롤백을 기다린다.
    TransactionOpened,
    /// 연결이 끊겼다. 열려 있던 트랜잭션은 DB 가 롤백했다.
    ConnectionLost { had_transaction: bool },
    /// 연결 설정이 바뀌었다.
    ConfigChanged,
}

/// 세션 하나의 지금 모습.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSessionInfo {
    pub session_id: String,
    pub project_id: String,
    pub connection_id: String,
    pub mode: DbSessionMode,
    pub state: DbSessionState,
}

/// [`TOPIC_SESSION_CHANGED`] 의 내용.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSessionChanged {
    pub session: DbSessionInfo,
    /// 바뀐 까닭(알릴 것이 있으면).
    pub notice: Option<DbSessionNotice>,
}

/// `db.session.open` 의 답.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSessionOpened {
    pub session: DbSessionInfo,
    /// 연결 이름(프로덕션 파괴 등급 확인에서 친다).
    pub name: String,
    pub read_only: bool,
    pub production: bool,
    /// 드라이버 id · 방언 · 지원 플래그(창이 기능을 켜고 끈다).
    pub driver: DriverInfo,
    pub server: ServerInfo,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSessionOpenParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 공유 설정을 그 워크트리에서 읽는다.
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
    /// 처음 고를 DB(없으면 설정의 것).
    #[serde(default)]
    pub database: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSessionParams {
    pub session_id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSessionSetModeParams {
    pub session_id: String,
    pub mode: DbSessionMode,
}

rpc_method! {
    /// 콘솔 탭의 세션을 연다: 연결 하나를 열어 닫을 때까지 쥔다. 그 프로젝트 창의 연결 + 신뢰한 프로젝트만(아니면 permission
    /// denied, `data.reason` 이 `notProjectWindow` · `untrusted`). 비밀 · 확인이 필요하면 M3-02 의 3101 · 3102, 드라이버가 실패하면
    /// 3003(`data.error`). 창(RPC 연결)이 닫히면 세션도 닫힌다.
    pub DbSessionOpen: "db.session.open" {
        params: DbSessionOpenParams,
        result: DbSessionOpened,
        tier: Exec,
        roles: [Owner],
    }

    /// 세션을 닫는다(도는 실행은 취소한다). 열린 트랜잭션은 DB 가 롤백한다 — 묻는 것은 화면의 몫(M3-15). 없는 세션이면 아무것도
    /// 하지 않는다.
    pub DbSessionClose: "db.session.close" {
        params: DbSessionParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// 자동 / 수동 커밋을 바꾼다. 트랜잭션이 열려 있으면 자동으로 바꾸지 않는다(conflict, `data.reason = "transactionOpen"`).
    pub DbSessionSetMode: "db.session.setMode" {
        params: DbSessionSetModeParams,
        result: DbSessionInfo,
        tier: Write,
        roles: [Owner],
    }

    /// 열린 트랜잭션을 커밋한다(없으면 아무것도 하지 않는다). 실패한 트랜잭션이면 롤백되고 3003(`data.error.rolledBack`). 실행 중이면
    /// conflict(`busy`).
    pub DbSessionCommit: "db.session.commit" {
        params: DbSessionParams,
        result: DbSessionInfo,
        tier: Write,
        roles: [Owner],
    }

    /// 열린 트랜잭션을 롤백한다(없으면 아무것도 하지 않는다). 실행 중이면 conflict(`busy`).
    pub DbSessionRollback: "db.session.rollback" {
        params: DbSessionParams,
        result: DbSessionInfo,
        tier: Write,
        roles: [Owner],
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// 블로킹 일(설정 · 키체인 · 레지스트리)을 블로킹 스레드에서.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 드라이버 오류 → [`code::DB_ERROR`](`data.error`).
pub(crate) fn db_error(error: DbError) -> RpcError {
    RpcError::new(code::DB_ERROR, error.message.clone()).with_data(json!({ "error": error }))
}

fn busy() -> RpcError {
    RpcError::new(
        rpc_code::CONFLICT,
        "the session is running statements; cancel or wait",
    )
    .with_data(json!({ "reason": "busy" }))
}

pub(crate) fn config_changed_error(connection_id: &str) -> RpcError {
    RpcError::new(
        rpc_code::CONFLICT,
        format!("the settings of db connection `{connection_id}` changed; open a new session"),
    )
    .with_data(json!({ "reason": "configChanged", "connectionId": connection_id }))
}

/// 세션이 쥔 연결과 그 트랜잭션. 실행 · 커밋 · 롤백은 이 잠금을 쥐고 한다(한 번에 하나).
pub(crate) struct Link {
    /// 끊겼으면 `None`.
    pub conn: Option<DbConnection>,
    pub mode: DbSessionMode,
    /// 마지막으로 맞춘 드라이버의 트랜잭션 상태.
    pub tx: TransactionState,
    pub since: Option<u64>,
    /// 지금 트랜잭션은 파이프라인이 read-only 실행을 위해 연 것이다(자동 커밋 탭 — 실행이 끝나면 롤백한다).
    pub owned_read_only: bool,
}

/// 파이프라인이 한 일(상태 변화의 까닭을 가른다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Statement { implicit_commit: bool },
    Begin,
    Commit,
    Rollback,
}

impl Link {
    pub fn sql(&mut self) -> Option<&mut Box<dyn pw_db_driver::SqlConnection>> {
        match self.conn.as_mut() {
            Some(DbConnection::Sql(conn)) => Some(conn),
            _ => None,
        }
    }

    /// 드라이버의 트랜잭션 상태를 읽어 세션에 맞추고, 사람에게 알릴 것을 돌려준다. `error` 는 그 단계의 오류다 — 트랜잭션이 끝났는데
    /// 드라이버가 되돌렸다고 말하지 않은 오류는, 암묵 커밋 문장이 아니면 되돌린 것으로 본다(`rolled_back` 을 채운다 — MySQL 이
    /// 모르는 전체 롤백 코드, 실패한 COMMIT).
    pub fn reconcile(
        &mut self,
        step: Step,
        error: Option<&mut DbError>,
    ) -> Option<DbSessionNotice> {
        let before = self.tx;
        let after = match self.conn.as_ref() {
            Some(DbConnection::Sql(conn)) => conn.transaction_state(),
            _ => TransactionState::Idle,
        };
        self.tx = after;
        match (before, after) {
            (TransactionState::Idle, TransactionState::Idle) => None,
            (TransactionState::Idle, _) => {
                self.since = Some(now_ms());
                matches!(step, Step::Statement { .. }).then_some(DbSessionNotice::TransactionOpened)
            }
            (_, TransactionState::Idle) => {
                self.since = None;
                let owned = std::mem::take(&mut self.owned_read_only);
                let notice = match (step, error) {
                    (Step::Begin | Step::Rollback, _) | (Step::Commit, None) => None,
                    (Step::Commit, Some(error)) => {
                        error.rolled_back = true;
                        Some(DbSessionNotice::RolledBack {
                            message: error.message.clone(),
                        })
                    }
                    (Step::Statement { .. }, Some(error)) if error.rolled_back => {
                        Some(DbSessionNotice::RolledBack {
                            message: error.message.clone(),
                        })
                    }
                    (Step::Statement { implicit_commit }, Some(error)) if !implicit_commit => {
                        error.rolled_back = true;
                        Some(DbSessionNotice::RolledBack {
                            message: error.message.clone(),
                        })
                    }
                    (Step::Statement { .. }, _) => Some(DbSessionNotice::CommittedImplicitly),
                };
                // 파이프라인이 연 read-only 트랜잭션에는 사람의 것이 없다 — 끝나도 알리지 않는다.
                notice.filter(|_| !owned)
            }
            _ => None,
        }
    }

    /// 연결이 끊겼다: 연결을 버리고(닫기는 뒤에서) 알릴 것을 돌려준다.
    pub fn lose(&mut self) -> DbSessionNotice {
        let had_transaction = self.tx != TransactionState::Idle && !self.owned_read_only;
        self.tx = TransactionState::Idle;
        self.since = None;
        self.owned_read_only = false;
        if let Some(conn) = self.conn.take() {
            tokio::spawn(async move {
                let _ = within(CLOSE_WAIT, "close", conn.close()).await;
            });
        }
        DbSessionNotice::ConnectionLost { had_transaction }
    }

    /// 수동 커밋 탭 · read-only 실행에 트랜잭션이 필요하면 연다. 연 것이 없으면 아무것도 하지 않는다.
    pub async fn ensure_transaction(&mut self, read_only: bool) -> Result<(), DbError> {
        let manual = self.mode == DbSessionMode::Manual;
        let Some(conn) = self.sql() else {
            return Err(DbError::new(
                DbErrorKind::Disconnected,
                "the connection is closed",
            ));
        };
        if conn.transaction_state() != TransactionState::Idle || !(manual || read_only) {
            return Ok(());
        }
        conn.begin(BeginOptions { read_only }).await?;
        let _ = self.reconcile(Step::Begin, None);
        if !manual {
            self.owned_read_only = true;
        }
        Ok(())
    }
}

/// 창이 보는 모습(실행이 잠금을 쥔 동안에도 읽는다).
#[derive(Debug, Clone, Copy)]
struct View {
    mode: DbSessionMode,
    tx: TransactionState,
    since: Option<u64>,
    connected: bool,
}

pub(crate) struct DbSession {
    pub id: String,
    pub project_id: String,
    pub worktree: Option<String>,
    pub connection_id: String,
    pub name: String,
    pub read_only: bool,
    pub production: bool,
    pub driver: DriverInfo,
    /// 연 때의 연결 설정(실행마다 지금 것과 견준다).
    pub config: DbConnectionConfig,
    pub config_changed: AtomicBool,
    /// 세션이 닫혔다.
    pub closed: CancellationToken,
    pub link: Arc<AsyncMutex<Link>>,
    pub running: Mutex<Option<Arc<Running>>>,
    view: Mutex<View>,
}

impl DbSession {
    pub fn dialect(&self) -> Option<SqlDialect> {
        self.driver.dialect
    }

    pub fn info(&self) -> DbSessionInfo {
        let view = *lock(&self.view);
        let transaction = view.since.and_then(|since| match view.tx {
            TransactionState::Idle => None,
            TransactionState::InTransaction => Some(DbOpenTransaction {
                since,
                failed: false,
            }),
            TransactionState::Failed => Some(DbOpenTransaction {
                since,
                failed: true,
            }),
        });
        let state = if self.config_changed.load(Ordering::SeqCst) {
            DbSessionState::ConfigChanged { transaction }
        } else if !view.connected {
            DbSessionState::Disconnected
        } else {
            match transaction {
                None => DbSessionState::Idle,
                Some(DbOpenTransaction {
                    since,
                    failed: false,
                }) => DbSessionState::InTransaction { since },
                Some(DbOpenTransaction {
                    since,
                    failed: true,
                }) => DbSessionState::Failed { since },
            }
        };
        DbSessionInfo {
            session_id: self.id.clone(),
            project_id: self.project_id.clone(),
            connection_id: self.connection_id.clone(),
            mode: view.mode,
            state,
        }
    }

    /// 잠금 안의 상태를 창이 보는 모습으로 옮긴다.
    pub fn sync(&self, link: &Link) {
        let since = if link.tx == TransactionState::Idle || link.owned_read_only {
            None
        } else {
            link.since
        };
        *lock(&self.view) = View {
            mode: link.mode,
            tx: if since.is_some() {
                link.tx
            } else {
                TransactionState::Idle
            },
            since,
            connected: link.conn.is_some(),
        };
    }
}

/// 도는 실행 하나.
pub(crate) struct Running {
    pub execution_id: String,
    pub session_id: String,
    pub project_id: String,
    pub dialect: SqlDialect,
    pub cancel: QueryCancel,
    /// 실행 전체를 멈춘다(창이 credit 을 주지 않아 멈춘 전송도 깨운다).
    pub stop: CancellationToken,
    /// 지금 드라이버에서 도는 문장(취소가 트랜잭션 전체를 되돌리는지 판정한다).
    pub current: Mutex<Option<Current>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Current {
    pub grade: RiskGrade,
    pub in_transaction: bool,
}

impl Running {
    /// 실행을 멈춘다: 전송을 깨우고 드라이버를 인터럽트한다(한 번만 간다).
    pub async fn halt(&self) {
        self.stop.cancel();
        if let Err(e) = self.cancel.cancel().await {
            tracing::info!(target: "pw_db", execution = %self.execution_id, %e, "cancel did not finish cleanly");
        }
    }
}

/// 연결을 여는 터널(M3-11 이 채운다). 없으면 터널이 있는 연결은 열지 않는다.
pub trait DbTunnels: Send + Sync {
    /// `spec` 의 `connect_to` 를 터널의 로컬 끝으로 채운다. 터널은 그 연결이 닫힐 때까지 산다(구현의 몫).
    fn open<'a>(
        &'a self,
        project_id: &'a str,
        host_ref: &'a str,
        spec: &'a mut pw_db_types::ConnectionSpec,
    ) -> pw_db_driver::BoxFuture<'a, Result<(), RpcError>>;
}

/// 세션 · 실행 파이프라인의 Host 쪽 상태. 데스크톱 크레이트가 하나 만들어 [`register`] 로 붙인다.
pub struct DbSessions {
    pub(crate) connections: Arc<Connections>,
    pub(crate) drivers: DriverRegistry,
    pub(crate) tunnels: OnceLock<Arc<dyn DbTunnels>>,
    global: OnceLock<ConfigStore>,
    /// 0 이면 설정을 따른다.
    grid_max_rows: AtomicU64,
    sessions: Mutex<HashMap<String, Arc<DbSession>>>,
    pub(crate) executions: Mutex<HashMap<String, Arc<Running>>>,
    next_id: AtomicU64,
}

impl DbSessions {
    pub fn new(connections: Arc<Connections>, drivers: DriverRegistry) -> Self {
        Self {
            connections,
            drivers,
            tunnels: OnceLock::new(),
            global: OnceLock::new(),
            grid_max_rows: AtomicU64::new(0),
            sessions: Mutex::default(),
            executions: Mutex::default(),
            next_id: AtomicU64::new(1),
        }
    }

    /// 전역 설정(`[db] grid_max_rows`). 두 번째부터는 무시한다.
    pub fn attach_settings(&self, global: ConfigStore) {
        let _ = self.global.set(global);
    }

    /// 터널(M3-11). 두 번째부터는 무시한다.
    pub fn attach_tunnels(&self, tunnels: Arc<dyn DbTunnels>) {
        let _ = self.tunnels.set(tunnels);
    }

    /// 그리드 보유 행 상한을 설정 대신 고정한다(시험 · 내장). `None` 이면 설정을 따른다.
    pub fn set_grid_max_rows(&self, rows: Option<u64>) {
        self.grid_max_rows
            .store(rows.map_or(0, |r| r.max(1)), Ordering::SeqCst);
    }

    /// 그리드 보유 행 상한(결정 M3-D7): 고정값, 아니면 설정 `[db] grid_max_rows`(1 이상의 정수), 아니면 10만.
    pub fn grid_max_rows(&self) -> u64 {
        match self.grid_max_rows.load(Ordering::SeqCst) {
            0 => self
                .global
                .get()
                .and_then(|g| {
                    g.snapshot()
                        .get("settings", GRID_MAX_ROWS_KEY)
                        .and_then(serde_json::Value::as_u64)
                })
                .filter(|n| *n >= 1)
                .unwrap_or(DEFAULT_GRID_MAX_ROWS),
            n => n,
        }
    }

    pub(crate) fn next_id(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// 열린 세션 수(시험).
    pub fn session_count(&self) -> usize {
        lock(&self.sessions).len()
    }

    pub(crate) fn bus(&self) -> Option<EventBus> {
        self.connections.host().ok().map(|h| h.bus.clone())
    }

    /// 세션 상태를 낸다.
    pub(crate) fn publish(&self, session: &DbSession, notice: Option<DbSessionNotice>) {
        let Some(bus) = self.bus() else { return };
        let payload = DbSessionChanged {
            session: session.info(),
            notice,
        };
        if let Some(notice) = &payload.notice {
            tracing::info!(target: "pw_db", session = %session.id, ?notice, "db session changed");
        }
        if let Err(e) = bus.publish(
            Scope::Project(session.project_id.clone()),
            TOPIC_SESSION_CHANGED,
            &payload,
        ) {
            tracing::warn!(target: "pw_db", %e, "cannot publish db.sessionChanged");
        }
    }

    /// id 의 세션. 없으면 not found, 그 세션을 연 프로젝트의 창이 아니면 permission denied.
    pub(crate) fn session_for(
        &self,
        ctx: &Ctx,
        session_id: &str,
    ) -> Result<(Arc<DbSession>, DbGrant), RpcError> {
        let session = lock(&self.sessions)
            .get(session_id)
            .cloned()
            .ok_or_else(|| {
                RpcError::new(
                    rpc_code::NOT_FOUND,
                    format!("db session `{session_id}` is not open"),
                )
            })?;
        let grant = db_grant(ctx.scope(), &session.project_id)?;
        Ok((session, grant))
    }

    /// `db.session.open`.
    pub async fn open(
        self: &Arc<Self>,
        ctx: &Ctx,
        params: DbSessionOpenParams,
    ) -> Result<DbSessionOpened, RpcError> {
        let grant = db_grant(ctx.scope(), &params.project_id)?;
        if let Some(database) = &params.database
            && (database.is_empty()
                || database.len() > 256
                || database.chars().any(char::is_control))
        {
            return Err(RpcError::invalid_params(
                "database must be 1-256 characters without control characters",
            ));
        }
        let opened = crate::connection::connect(
            self,
            &grant,
            params.worktree.as_deref(),
            &params.connection_id,
            params.database.clone(),
        )
        .await?;
        let id = self.next_id("dbs-");
        let tx = match &opened.connection {
            DbConnection::Sql(conn) => conn.transaction_state(),
            DbConnection::Kv(_) => TransactionState::Idle,
        };
        let session = Arc::new(DbSession {
            id: id.clone(),
            project_id: params.project_id.clone(),
            worktree: params.worktree.clone(),
            connection_id: params.connection_id.clone(),
            name: opened.config.name.clone(),
            read_only: opened.config.read_only,
            production: opened.config.production,
            driver: opened.driver.clone(),
            config: opened.config,
            config_changed: AtomicBool::new(false),
            closed: CancellationToken::new(),
            link: Arc::new(AsyncMutex::new(Link {
                conn: Some(opened.connection),
                mode: DbSessionMode::Auto,
                tx,
                since: (tx != TransactionState::Idle).then(now_ms),
                owned_read_only: false,
            })),
            running: Mutex::new(None),
            view: Mutex::new(View {
                mode: DbSessionMode::Auto,
                tx,
                since: (tx != TransactionState::Idle).then(now_ms),
                connected: true,
            }),
        });
        lock(&self.sessions).insert(id.clone(), Arc::clone(&session));
        tracing::info!(target: "pw_db", project = %session.project_id, connection = %session.connection_id, session = %id, "db session opened");
        // 창(RPC 연결)이 닫히면 이 세션을 닫는다.
        let owner_closed = ctx.connection_closed();
        let (this, watched) = (Arc::clone(self), Arc::clone(&session));
        tokio::spawn(async move {
            tokio::select! {
                () = owner_closed.cancelled() => {
                    tracing::info!(target: "pw_db", session = %watched.id, "the window closed; closing its db session");
                    this.close_session(&watched).await;
                }
                () = watched.closed.cancelled() => {}
            }
        });
        Ok(DbSessionOpened {
            session: session.info(),
            name: session.name.clone(),
            read_only: session.read_only,
            production: session.production,
            driver: session.driver.clone(),
            server: opened.server,
        })
    }

    /// 세션을 닫는다: 도는 실행을 멈추고 연결을 닫는다. 이미 닫혔으면 아무것도 하지 않는다.
    pub(crate) async fn close_session(&self, session: &Arc<DbSession>) {
        if lock(&self.sessions).remove(&session.id).is_none() {
            return;
        }
        session.closed.cancel();
        let running = lock(&session.running).clone();
        if let Some(running) = running {
            running.halt().await;
        }
        let mut link = session.link.lock().await;
        if let Some(conn) = link.conn.take()
            && let Err(e) = within(CLOSE_WAIT, "close", conn.close()).await
        {
            tracing::info!(target: "pw_db", session = %session.id, %e, "closing the db connection failed");
        }
        tracing::info!(target: "pw_db", session = %session.id, "db session closed");
    }

    /// `db.session.close`.
    pub async fn close(&self, ctx: &Ctx, params: DbSessionParams) -> Result<(), RpcError> {
        match self.session_for(ctx, &params.session_id) {
            Ok((session, _grant)) => {
                self.close_session(&session).await;
                Ok(())
            }
            Err(e) if e.code == rpc_code::NOT_FOUND => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// `db.session.setMode`.
    pub async fn set_mode(
        &self,
        ctx: &Ctx,
        params: DbSessionSetModeParams,
    ) -> Result<DbSessionInfo, RpcError> {
        let (session, _grant) = self.session_for(ctx, &params.session_id)?;
        let mut link = Arc::clone(&session.link)
            .try_lock_owned()
            .map_err(|_| busy())?;
        if params.mode == DbSessionMode::Auto
            && link.mode == DbSessionMode::Manual
            && link.tx != TransactionState::Idle
            && !link.owned_read_only
        {
            return Err(RpcError::new(
                rpc_code::CONFLICT,
                "commit or roll back the open transaction before switching to auto-commit",
            )
            .with_data(json!({ "reason": "transactionOpen" })));
        }
        link.mode = params.mode;
        session.sync(&link);
        drop(link);
        self.publish(&session, None);
        Ok(session.info())
    }

    /// `db.session.commit` · `rollback`.
    pub async fn finish(
        &self,
        ctx: &Ctx,
        params: DbSessionParams,
        commit: bool,
    ) -> Result<DbSessionInfo, RpcError> {
        let (session, _grant) = self.session_for(ctx, &params.session_id)?;
        let mut link = Arc::clone(&session.link)
            .try_lock_owned()
            .map_err(|_| busy())?;
        let Some(conn) = link.sql() else {
            return Err(match &link.conn {
                Some(DbConnection::Kv(_)) => {
                    RpcError::invalid_params("this connection has no transactions")
                }
                _ => db_error(DbError::new(
                    DbErrorKind::Disconnected,
                    "the connection is closed",
                )),
            });
        };
        if conn.transaction_state() == TransactionState::Idle {
            // 열린 트랜잭션이 없다 — 상태만 맞춘다.
            let notice = link.reconcile(Step::Rollback, None);
            session.sync(&link);
            drop(link);
            self.publish(&session, notice);
            return Ok(session.info());
        }
        let result = if commit {
            conn.commit().await
        } else {
            conn.rollback().await
        };
        let mut error = result.err();
        let notice = match &error {
            Some(e) if e.kind == DbErrorKind::Disconnected => Some(link.lose()),
            _ => link.reconcile(
                if commit { Step::Commit } else { Step::Rollback },
                error.as_mut(),
            ),
        };
        session.sync(&link);
        drop(link);
        self.publish(&session, notice);
        match error {
            None => {
                tracing::info!(target: "pw_db", session = %session.id, commit, "db transaction finished");
                Ok(session.info())
            }
            Some(e) => Err(RpcError::new(code::DB_ERROR, e.message.clone())
                .with_data(json!({ "error": e, "session": session.info() }))),
        }
    }

    /// 실행 전 다시 보기: 신뢰(관문 ②)와 연결 설정. 설정이 연 때와 다르면 `configChanged` 로 두고 conflict 오류.
    pub(crate) async fn recheck(&self, session: &Arc<DbSession>) -> Result<(), RpcError> {
        let (c, s) = (Arc::clone(&self.connections), Arc::clone(session));
        let changed = blocking(move || {
            c.trust_gate(&s.project_id, s.worktree.as_deref())?;
            Ok(current_config(&c, &s)?.as_ref() != Some(&s.config))
        })
        .await?;
        if changed {
            self.mark_config_changed(session);
        }
        if session.config_changed.load(Ordering::SeqCst) {
            return Err(config_changed_error(&session.connection_id));
        }
        Ok(())
    }

    fn mark_config_changed(&self, session: &DbSession) {
        if !session.config_changed.swap(true, Ordering::SeqCst) {
            tracing::info!(target: "pw_db", session = %session.id, connection = %session.connection_id, "db connection config changed under an open session");
            self.publish(session, Some(DbSessionNotice::ConfigChanged));
        }
    }

    /// `config.changed` 의 `db/<id>` 를 받은 프로젝트의 세션을 다시 본다(블로킹).
    fn refresh(&self, project_id: &str, changed: &[String]) {
        let sessions: Vec<Arc<DbSession>> = lock(&self.sessions)
            .values()
            .filter(|s| {
                s.project_id == project_id
                    && changed
                        .iter()
                        .any(|c| c.strip_prefix(FILE_PREFIX) == Some(s.connection_id.as_str()))
            })
            .cloned()
            .collect();
        for session in sessions {
            match current_config(&self.connections, &session) {
                Ok(now) if now.as_ref() == Some(&session.config) => {}
                _ => self.mark_config_changed(&session),
            }
        }
    }
}

/// 세션의 연결 설정 지금 값(그 세션의 활성 워크트리). 없거나 틀렸으면 `None`.
fn current_config(
    connections: &Connections,
    session: &DbSession,
) -> Result<Option<DbConnectionConfig>, RpcError> {
    let loaded = connections.load(&session.project_id, session.worktree.as_deref())?;
    Ok(loaded
        .get(&session.connection_id)
        .and_then(|c| c.config.clone().ok()))
}

/// `config.changed` 구독. Host 를 만든 직후에 연다.
pub fn subscribe_config(bus: &EventBus) -> Subscription {
    bus.subscribe(Filter::all().topics([TOPIC_CHANGED]))
}

#[derive(Debug, Deserialize)]
struct ConfigChangedPayload {
    #[serde(default)]
    changed: Vec<String>,
}

/// 구독이 닫힐 때까지 연결 설정이 바뀐 세션을 `configChanged` 로 둔다. 이벤트를 놓치면 열린 세션을 모두 다시 본다.
pub async fn follow_config(mut events: Subscription, sessions: Arc<DbSessions>) {
    while let Some(received) = events.recv().await {
        let work: Vec<(String, Vec<String>)> = match received {
            Received::Event(event) => {
                let Scope::Project(project) = &event.scope else {
                    continue;
                };
                let Ok(payload) =
                    serde_json::from_value::<ConfigChangedPayload>(event.payload.clone())
                else {
                    continue;
                };
                if !payload.changed.iter().any(|c| c.starts_with(FILE_PREFIX)) {
                    continue;
                }
                vec![(project.clone(), payload.changed)]
            }
            Received::Lagged(_) => lock(&sessions.sessions)
                .values()
                .map(|s| {
                    (
                        s.project_id.clone(),
                        vec![format!("{FILE_PREFIX}{}", s.connection_id)],
                    )
                })
                .collect(),
        };
        let s = Arc::clone(&sessions);
        let _ = tokio::task::spawn_blocking(move || {
            for (project, changed) in work {
                s.refresh(&project, &changed);
            }
        })
        .await;
    }
}

/// M3-05 의 메서드를 붙인다(세션 · 실행 · 쪽 단위 실행 · 분류 · 연결 테스트).
pub fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    let (open, close, mode, commit, rollback) = (
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        Arc::clone(&sessions),
    );
    router
        .unary(DbSessionOpen, move |ctx, params: DbSessionOpenParams| {
            let s = Arc::clone(&open);
            async move { s.open(&ctx, params).await }
        })
        .unary(DbSessionClose, move |ctx, params: DbSessionParams| {
            let s = Arc::clone(&close);
            async move { s.close(&ctx, params).await }
        })
        .unary(
            DbSessionSetMode,
            move |ctx, params: DbSessionSetModeParams| {
                let s = Arc::clone(&mode);
                async move { s.set_mode(&ctx, params).await }
            },
        )
        .unary(DbSessionCommit, move |ctx, params: DbSessionParams| {
            let s = Arc::clone(&commit);
            async move { s.finish(&ctx, params, true).await }
        })
        .unary(DbSessionRollback, move |ctx, params: DbSessionParams| {
            let s = Arc::clone(&rollback);
            async move { s.finish(&ctx, params, false).await }
        });
    crate::execute::register(router, Arc::clone(&sessions));
    crate::connection::register(router, sessions);
}
