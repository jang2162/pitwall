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
//! - **상태는 서버에 물어 맞춘다**: 오류 뒤에는 드라이버의 마지막 값을 믿지 않고 `SqlConnection::read_transaction_state` 로 서버에
//!   묻는다. 트랜잭션이 끝났으면 드라이버가 되돌렸다고 말한 것(`rolledBack`) · 암묵 커밋 문장(`committedImplicitly`)만 그렇게 알리고,
//!   그 밖은 「끝났는데 커밋인지 롤백인지 모름」(`transactionEnded` — MySQL `CALL` · `EXECUTE` 가 안에서 커밋한 뒤 실패 등)이다. 물을
//!   수 없으면 「알 수 없음」(`transactionUnknown`)이고, 연결이 끊겼으면 끊길 때 하던 일이 커밋일 수 있었으면(COMMIT, 커밋할 수 있는
//!   문장) 결과를 모른다고 알린다 — 롤백 · 커밋을 단정하지 않는다(M3-05 리뷰 M1 · L3). 실행과 실행 사이에 드라이버 상태가 바뀌었으면
//!   (정리 예산을 넘겨 버려진 문장이 나중에 끝남) 다음 실행 · 커밋 · 롤백이 먼저 서버에 물어 맞추고, 이미 끝난 트랜잭션의 커밋 ·
//!   롤백은 성공으로 답하지 않는다(L2).
//! - **창이 닫히면**(RPC 연결이 닫히면) 그 창이 연 세션 중 열린 트랜잭션이 없는 것은 곧바로 닫는다. 열린 트랜잭션이 있는 세션은
//!   닫지 않는다 — 새로고침 · 웹뷰 재시작도 연결을 닫기 때문이다. Host 가 (창 [`pw_rpc::Ctx::window`], 탭 `tabId`)에 묶어
//!   [`DEFAULT_UNATTENDED_MINUTES`] 분(설정 `[db] unattended_transaction_minutes`) 보존하고, 같은 창의 같은 탭이
//!   `db.session.resume` 으로 다시 붙으면 그 세션을 「열린 트랜잭션」 그대로 잇는다. 그동안 아무도 붙지 않으면 롤백하고 닫되 몰래가
//!   아니다 — 알림(`notification.show`)과 `db.session.unattendedRollbacks` 로 다음에 그 프로젝트 창이 열릴 때도 읽히는 기록을
//!   남긴다. 창 · 탭을 모르는 연결(원격 등)은 보존하지 않고 곧바로 롤백 · 기록한다. 창을 실제로 닫을 때 묻는 것은 화면의 몫이다(M3-15).
//! - **설정이 바뀐 연결**은 저절로 다시 붙지 않고 `configChanged` 가 된다 — `config.changed` 이벤트로 알고, 감시 밖의 변경(메인
//!   체크아웃의 `.local.toml` 을 워크트리 창이 못 들을 때 등)은 실행마다 설정을 다시 읽어 연 때와 견줘 잡는다. 그 세션은 새 실행을
//!   받지 않고 커밋 · 롤백 · 닫기만 받는다. 다시 읽기는 그 연결의 설정 파일 자리(`Connections::connection_files`)의 수정 시각 · 크기가
//!   연 때와 다를 때만 한다.
//! - **확인은 한 번만 쓰인다**: 확인 해시는 세션의 실행 순번에 묶인다 — 실행이 관문을 지날 때마다 순번이 올라, 받은 확인은 그 세션의
//!   다음 실행 하나에만 맞는다(M3-05 리뷰 L4).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pw_core::bus::{EventBus, Filter, Received, Scope, Subscription};
use pw_core::config::ConfigStore;
use pw_core::notification::{Level, Notification};
use pw_db_driver::budget::within;
use pw_db_driver::{Budget, DbConnection, QueryCancel};
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

/// 그리드 보유 바이트 상한의 설정 키(전역 `settings` 의 `[db] grid_max_bytes` — 받은 셀 바이트 합, `crate::execute::cell_bytes`)와
/// 기본값 64 MiB. 셀 상한(1 MiB) × 행 상한(10만)이 웹뷰 메모리에 그대로 쌓이지 않게 둔다(M3-16 리뷰, 메인 판정). 웹뷰는 글을 UTF-16
/// 으로 들고 객체 머리가 붙으므로 실제 메모리는 이 값의 몇 배다.
pub const GRID_MAX_BYTES_KEY: &str = "db.grid_max_bytes";
pub const DEFAULT_GRID_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// 연결을 닫을 때 드라이버를 기다리는 시간.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

/// 창의 연결이 닫힌 뒤 열린 트랜잭션을 보존하는 시간의 설정 키(전역 `settings` 의 `[db] unattended_transaction_minutes`)와
/// 기본값. 10분은 새로고침 · 웹뷰 재시작 · 개발 중 전체 리로드(몇 초)와 실수로 닫은 프로젝트 창을 다시 여는 시간을 넉넉히 덮고, 사람
/// 없이 남은 트랜잭션의 잠금이 다른 작업을 막는 시간은 짧게 묶는다(결정 M3-D21 의 근거 — 열린 트랜잭션은 사람이 있는 동안 저절로
/// 롤백하지 않는다. 이것은 사람이 붙어 있지 않을 때다).
pub const UNATTENDED_MINUTES_KEY: &str = "db.unattended_transaction_minutes";
pub const DEFAULT_UNATTENDED_MINUTES: u64 = 10;

/// 보존 시간 뒤 롤백한 기록을 프로젝트마다 남기는 수.
const UNATTENDED_RECORDS: usize = 32;

/// 탭 id 의 길이 상한.
const MAX_TAB_ID: usize = 256;

/// 「보존 시간 뒤 롤백했다」 알림의 Fluent 메시지 id(`extensions/i18n-default/i18n/*/project.ftl`). 인자 `connection` · `minutes`.
pub const UNATTENDED_ROLLBACK_MESSAGE: &str = "db-unattended-transaction-rolled-back";
/// 「창 · 탭을 몰라 보존하지 못하고 롤백했다」 알림의 메시지 id. 인자 `connection`.
pub const CLOSED_ROLLBACK_MESSAGE: &str = "db-closed-transaction-rolled-back";

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
    /// 연결이 끊겼다 — 열린 트랜잭션은 DB 가 롤백했다(커밋 도중이었으면 결과를 모른다 — 알림 `connectionLost` 의
    /// `outcomeUnknown`). 다시 붙는 것은 사람이 새 세션을 열 때다.
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
    /// 서버 · 드라이버가 트랜잭션 전체를 되돌렸다(교착, 실패한 커밋, SQLite 쓰기 취소 등 — 드라이버가 그렇게 말했고 서버에 물은
    /// 상태도 끝남이다). `message` 는 그 오류.
    RolledBack { message: String },
    /// MySQL · MariaDB 의 암묵 커밋 문장(DDL · `LOCK TABLES` 등)이 열린 트랜잭션을 커밋했다(문장이 실패했어도).
    CommittedImplicitly,
    /// 열린 트랜잭션이 끝났는데 커밋인지 롤백인지 서버가 알리지 않았다 — MySQL `CALL` · `EXECUTE` 가 안에서 커밋하거나 롤백한 뒤
    /// 실패, 드라이버가 모르는 전체 롤백 코드, 실행 사이에 버려진 문장이 끝남 등. `message` 는 그 단계의 오류(있으면). 사람이 다른
    /// 연결로 확인해야 한다.
    TransactionEnded { message: Option<String> },
    /// 오류 뒤 서버에 트랜잭션 상태를 묻지 못했다 — 열려 있는지 끝났는지 모른다. 세션 상태는 마지막으로 안 값 그대로다. `message`
    /// 는 묻지 못한 까닭.
    TransactionUnknown { message: String },
    /// 파이프라인이 열지 않은 트랜잭션이 열렸다(PG 의 숨은 블록, MySQL `SET autocommit = 0` 등) — 그 연결은 이 탭에 묶이고 커밋 ·
    /// 롤백을 기다린다.
    TransactionOpened,
    /// 연결이 끊겼다. 열려 있던 트랜잭션은 DB 가 롤백했다 — 다만 `outcomeUnknown` 이면 끊길 때 커밋 중이었거나 커밋할 수 있는
    /// 문장(MySQL `CALL` 등)이 돌고 있어 커밋됐는지 알 수 없다.
    ConnectionLost {
        had_transaction: bool,
        outcome_unknown: bool,
    },
    /// 연결 설정이 바뀌었다.
    ConfigChanged,
    /// 세션을 연 창의 연결이 닫혔다(새로고침 · 웹뷰 재시작 · 창 닫기). 열린 트랜잭션을 `until`(유닉스 밀리초)까지 보존한다 — 그 창의
    /// 그 탭이 `db.session.resume` 으로 다시 붙지 않으면 그때 롤백한다.
    Unattended {
        #[specta(type = JsNumber)]
        until: u64,
    },
    /// 아무도 붙지 않아 열린 트랜잭션을 롤백하고 세션을 닫았다(`db.session.unattendedRollbacks` 에 기록이 남는다).
    UnattendedRolledBack,
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
    /// 이 세션을 쥐는 콘솔 탭의 id(창 안에서 탭마다 다르고 탭을 복원해도 같은 값, 1~256자). 있으면 창이 새로고침돼도 열린
    /// 트랜잭션이 있는 세션을 보존하고 `db.session.resume` 으로 다시 붙을 수 있다.
    #[serde(default)]
    pub tab_id: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSessionResumeParams {
    pub project_id: String,
    pub tab_id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbUnattendedRollbacksParams {
    pub project_id: String,
}

/// 보존 시간 안에 아무도 붙지 않아 롤백한 열린 트랜잭션 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbUnattendedRollback {
    pub connection_id: String,
    /// 연결 이름.
    pub name: String,
    pub tab_id: Option<String>,
    /// 트랜잭션을 연 시각(유닉스 밀리초).
    #[specta(type = Option<JsNumber>)]
    pub since: Option<u64>,
    /// 롤백한 시각(유닉스 밀리초).
    #[specta(type = JsNumber)]
    pub rolled_back_at: u64,
    /// 보존한 시간(분). 창 · 탭을 몰라 보존하지 않았으면 0.
    #[specta(type = JsNumber)]
    pub kept_minutes: u64,
    /// 롤백 문장이 실패했으면 그 오류 — 그래도 연결을 닫았으므로 DB 가 롤백했다.
    pub rollback_error: Option<String>,
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

    /// 창이 새로고침 · 웹뷰 재시작 뒤 복원한 탭의 세션을 다시 잡는다: 같은 창(`Ctx::window`)의 같은 `tabId` 로 열렸고 창의 연결이
    /// 닫혀 Host 가 보존 중인 열린 트랜잭션 세션이 있으면 그 세션(「열린 트랜잭션」 그대로), 없으면 `null`. 새 연결은 열지 않는다
    /// (결정 M3-D13). 그 프로젝트 창의 연결만(다른 프로젝트는 permission denied, 다른 창은 `null`).
    pub DbSessionResume: "db.session.resume" {
        params: DbSessionResumeParams,
        result: Option<DbSessionOpened>,
        tier: Write,
        roles: [Owner],
    }

    /// 보존 시간 안에 아무도 붙지 않아 롤백한 열린 트랜잭션의 기록(이 Host 가 도는 동안, 프로젝트마다 최근 32개, 오래된 것부터).
    /// 창이 열릴 때 읽어 사람에게 보인다. 그 프로젝트 창의 연결만.
    pub DbSessionUnattendedRollbacks: "db.session.unattendedRollbacks" {
        params: DbUnattendedRollbacksParams,
        result: Vec<DbUnattendedRollback>,
        tier: Read,
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

    /// 열린 트랜잭션을 커밋한다(없으면 아무것도 하지 않는다). 실패한 트랜잭션이면 롤백되고 3003(`data.error.rolledBack`). 커밋 ·
    /// 롤백 전에 이미 끝나 있었으면(실행 사이에 끝남 — 결과 모름) 3003 이고 알림 `transactionEnded`. 실행 중이면 conflict(`busy`).
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
    /// 연결이 쓰는 SSH 터널(M3-11). 연결과 함께 버린다.
    pub tunnel: Option<DbTunnelLease>,
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
    /// 사람의 문장. `implicit_commit` 은 MySQL 암묵 커밋 문장, `may_end` 은 열린 트랜잭션을 끝낼 수 있는 문장(암묵 커밋 + `CALL` ·
    /// `EXECUTE` 등 — `pw_db_sql::txn::may_end_transaction`).
    Statement {
        implicit_commit: bool,
        may_end: bool,
    },
    Begin,
    Commit,
    Rollback,
    /// 실행과 실행 사이 — 파이프라인이 아무것도 하지 않았는데 드라이버 상태가 바뀌었다(버려진 문장이 나중에 끝남).
    Between,
}

impl Step {
    /// 이 단계 도중 연결이 끊겼으면 열린 트랜잭션이 커밋됐을 수 있다 — 「DB 가 롤백했다」 고 단정하지 않는다.
    fn may_commit(self) -> bool {
        matches!(
            self,
            Step::Commit | Step::Between | Step::Statement { may_end: true, .. }
        )
    }
}

impl Link {
    pub fn sql(&mut self) -> Option<&mut Box<dyn pw_db_driver::SqlConnection>> {
        match self.conn.as_mut() {
            Some(DbConnection::Sql(conn)) => Some(conn),
            _ => None,
        }
    }

    /// 사람의 트랜잭션이 열려 있다(파이프라인이 연 read-only 트랜잭션은 빼고).
    pub fn holds_transaction(&self) -> bool {
        self.tx != TransactionState::Idle && !self.owned_read_only
    }

    /// 오류 없이 끝난 단계: 드라이버가 서버의 답으로 갱신한 상태(`transaction_state`)로 세션을 맞추고 알릴 것을 돌려준다.
    pub fn reconcile(
        &mut self,
        step: Step,
        error: Option<&mut DbError>,
    ) -> Option<DbSessionNotice> {
        let after = match self.conn.as_ref() {
            Some(DbConnection::Sql(conn)) => conn.transaction_state(),
            _ => TransactionState::Idle,
        };
        self.apply(step, error, after)
    }

    /// 단계의 끝을 세션에 반영한다. 연결이 끊겼으면 [`lose`](Self::lose). 사람의 트랜잭션 안에서 오류가 났으면 드라이버의 마지막
    /// 값을 믿지 않고 서버에 물어(`read_transaction_state`) 맞춘다 — 물을 수 없으면 「알 수 없음」 이다(머리 주석).
    pub async fn settle(
        &mut self,
        step: Step,
        error: Option<&mut DbError>,
    ) -> Option<DbSessionNotice> {
        match error {
            Some(e) if e.kind == DbErrorKind::Disconnected => Some(self.lose(step.may_commit())),
            Some(e) if self.holds_transaction() => match self.read_state().await {
                Ok(after) => self.apply(step, Some(e), after),
                Err(read) if read.kind == DbErrorKind::Disconnected => {
                    Some(self.lose(step.may_commit()))
                }
                Err(read) => Some(DbSessionNotice::TransactionUnknown {
                    message: read.message,
                }),
            },
            error => self.reconcile(step, error),
        }
    }

    /// 실행과 실행 사이에 드라이버 상태가 세션과 달라졌으면 서버에 물어 맞춘다(머리 주석의 L2). 같으면 아무것도 하지 않는다.
    pub async fn catch_up(&mut self) -> Option<DbSessionNotice> {
        let conn = self.sql()?;
        if conn.transaction_state() == self.tx {
            return None;
        }
        match self.read_state().await {
            Ok(after) => self.apply(Step::Between, None, after),
            Err(read) if read.kind == DbErrorKind::Disconnected => {
                Some(self.lose(Step::Between.may_commit()))
            }
            Err(read) => Some(DbSessionNotice::TransactionUnknown {
                message: read.message,
            }),
        }
    }

    /// 서버에 지금 트랜잭션 상태를 묻는다(취소 정리 예산 안에서).
    async fn read_state(&mut self) -> Result<TransactionState, DbError> {
        let Some(conn) = self.sql() else {
            return Err(DbError::new(
                DbErrorKind::Disconnected,
                "the connection is closed",
            ));
        };
        within(
            Budget::CONSOLE.cancel_cleanup,
            "transaction state",
            conn.read_transaction_state(),
        )
        .await
    }

    /// 단계 뒤의 상태 `after` 로 세션을 맞추고 알릴 것을 돌려준다. 트랜잭션이 끝났으면 까닭을 가른다: 드라이버가 되돌렸다고 말한 오류
    /// · 실패한 COMMIT(끝난 COMMIT 오류는 커밋이 아니다 — M3-06 H2 계약)은 `rolledBack`, 암묵 커밋 문장은 `committedImplicitly`,
    /// 그 밖은 `transactionEnded`(모름). 오류의 `rolled_back` 은 실제 상태가 끝남일 때만 남긴다.
    fn apply(
        &mut self,
        step: Step,
        error: Option<&mut DbError>,
        after: TransactionState,
    ) -> Option<DbSessionNotice> {
        let before = self.tx;
        self.tx = after;
        let mut error = error;
        if after != TransactionState::Idle
            && let Some(e) = error.as_deref_mut()
        {
            // 「트랜잭션 전체를 되돌렸다」 는 열린 트랜잭션과 맞지 않는다.
            e.rolled_back = false;
        }
        match (before, after) {
            (TransactionState::Idle, TransactionState::Idle) => None,
            (TransactionState::Idle, _) => {
                self.since = Some(now_ms());
                matches!(step, Step::Statement { .. } | Step::Between)
                    .then_some(DbSessionNotice::TransactionOpened)
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
                    (
                        Step::Statement {
                            implicit_commit: true,
                            ..
                        },
                        _,
                    ) => Some(DbSessionNotice::CommittedImplicitly),
                    (Step::Statement { .. } | Step::Between, error) => {
                        Some(DbSessionNotice::TransactionEnded {
                            message: error.map(|e| e.message.clone()),
                        })
                    }
                };
                // 파이프라인이 연 read-only 트랜잭션에는 사람의 것이 없다 — 끝나도 알리지 않는다.
                notice.filter(|_| !owned)
            }
            _ => None,
        }
    }

    /// 연결이 끊겼다: 연결을 버리고(닫기는 뒤에서) 알릴 것을 돌려준다. `may_have_committed` 면 끊길 때 하던 일이 열린 트랜잭션을
    /// 커밋했을 수 있다 — 결과를 모른다고 알린다.
    pub fn lose(&mut self, may_have_committed: bool) -> DbSessionNotice {
        let had_transaction = self.holds_transaction();
        self.tx = TransactionState::Idle;
        self.since = None;
        self.owned_read_only = false;
        let tunnel = self.tunnel.take();
        if let Some(conn) = self.conn.take() {
            tokio::spawn(async move {
                let _ = within(CLOSE_WAIT, "close", conn.close()).await;
                drop(tunnel);
            });
        }
        DbSessionNotice::ConnectionLost {
            had_transaction,
            outcome_unknown: had_transaction && may_have_committed,
        }
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
    pub server: ServerInfo,
    /// 연 때의 연결 설정(실행마다 지금 것과 견준다).
    pub config: DbConnectionConfig,
    pub config_changed: AtomicBool,
    /// 설정 파일 자리(처음 다시 볼 때 정한다)와 마지막으로 같다고 본 때의 수정 시각 · 크기([`stamp_of`]).
    config_files: OnceLock<Vec<PathBuf>>,
    config_stamp: Mutex<Option<Stamp>>,
    /// 확인 해시의 실행 순번(관문을 지날 때마다 오른다 — 확인은 한 번만 쓰인다).
    confirm_epoch: AtomicU64,
    /// 이 세션을 연 창과 탭(보존 · 다시 붙기의 열쇠).
    window: Option<String>,
    tab: Option<String>,
    /// 지금 이 세션을 쥔 창의 연결이 닫히면 취소되는 토큰과 그 붙음의 순번(다시 붙을 때마다 오른다).
    owner: Mutex<Owner>,
    /// 세션이 닫혔다.
    pub closed: CancellationToken,
    pub link: Arc<AsyncMutex<Link>>,
    pub running: Mutex<Option<Arc<Running>>>,
    view: Mutex<View>,
}

struct Owner {
    closed: CancellationToken,
    generation: u64,
}

/// 설정 파일들의 (수정 시각, 크기). 없는 파일은 `None`.
type Stamp = Vec<Option<(SystemTime, u64)>>;

fn stamp_of(files: &[PathBuf]) -> Stamp {
    files
        .iter()
        .map(|f| {
            let meta = std::fs::metadata(f).ok()?;
            Some((meta.modified().ok()?, meta.len()))
        })
        .collect()
}

impl DbSession {
    pub fn dialect(&self) -> Option<SqlDialect> {
        self.driver.dialect
    }

    /// 확인 해시에 넣는 세션 열쇠: 세션 id 와 지금 실행 순번.
    pub fn confirm_key(&self) -> String {
        format!("{}#{}", self.id, self.confirm_epoch.load(Ordering::SeqCst))
    }

    /// 실행이 관문을 지났다 — 지금까지 낸 확인을 모두 쓸모없게 한다.
    pub fn spend_confirmations(&self) {
        self.confirm_epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// 사람의 트랜잭션이 열려 있다(창이 보는 모습으로).
    fn shows_transaction(&self) -> bool {
        lock(&self.view).since.is_some()
    }

    fn opened(&self) -> DbSessionOpened {
        DbSessionOpened {
            session: self.info(),
            name: self.name.clone(),
            read_only: self.read_only,
            production: self.production,
            driver: self.driver.clone(),
            server: self.server.clone(),
        }
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
    /// SQL 실행의 방언. KV 실행(`db.kv.*`, M3-22)은 `None`.
    pub dialect: Option<SqlDialect>,
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

/// 연결을 여는 터널(M3-11 — `crate::tunnel::SshTunnels`). 없으면 터널이 있는 연결은 열지 않는다.
pub trait DbTunnels: Send + Sync {
    /// `spec` 의 `connect_to` 를 터널의 로컬 끝으로 채우고, 그 터널을 쓰는 동안 쥘 리스를 돌려준다. `root` 는 활성 워크트리 안의
    /// 프로젝트 루트(런타임 환경을 고른다). 관문 · 신뢰 게이트 · 확인을 지난 뒤에만 부른다(`crate::connection`).
    fn open<'a>(
        &'a self,
        project_id: &'a str,
        root: &'a std::path::Path,
        host_ref: &'a str,
        spec: &'a mut pw_db_types::ConnectionSpec,
    ) -> pw_db_driver::BoxFuture<'a, Result<DbTunnelLease, RpcError>>;
}

/// 연결이 터널을 쓰는 동안 쥐는 것(M3-11). 버리면 그 연결의 몫이 끝난다 — 마지막 몫이면 ssh 가 끝난다. **연결을 쥐는 동안 함께
/// 쥔다**: [`crate::connection::OpenedConnection::tunnel`] 을 먼저 버리면 연결이 살아 있어도 터널이 닫힌다.
pub struct DbTunnelLease {
    _keep: Box<dyn std::any::Any + Send + Sync>,
    lost: CancellationToken,
}

impl DbTunnelLease {
    /// `keep` 은 쥐고 있을 것(터널 구현의 리스), `lost` 는 터널이 끊기면 취소되는 토큰.
    pub fn new(keep: impl std::any::Any + Send + Sync, lost: CancellationToken) -> Self {
        Self {
            _keep: Box::new(keep),
            lost,
        }
    }

    /// 터널이 끊기면 깨어난다(돌려준 토큰을 취소해도 터널은 끝나지 않는다).
    pub fn lost(&self) -> CancellationToken {
        self.lost.child_token()
    }
}

impl std::fmt::Debug for DbTunnelLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbTunnelLease")
            .field("lost", &self.lost.is_cancelled())
            .finish_non_exhaustive()
    }
}

/// 세션 · 실행 파이프라인의 Host 쪽 상태. 데스크톱 크레이트가 하나 만들어 [`register`] 로 붙인다.
pub struct DbSessions {
    pub(crate) connections: Arc<Connections>,
    pub(crate) drivers: DriverRegistry,
    pub(crate) tunnels: OnceLock<Arc<dyn DbTunnels>>,
    global: OnceLock<ConfigStore>,
    /// 0 이면 설정을 따른다.
    grid_max_rows: AtomicU64,
    /// 0 이면 설정을 따른다.
    grid_max_bytes: AtomicU64,
    /// 창 없이 남은 열린 트랜잭션의 보존 시간(시험 · 내장 고정값). `None` 이면 설정을 따른다.
    unattended_keep: Mutex<Option<Duration>>,
    /// 보존 시간 뒤 롤백한 기록(프로젝트 id → 최근 것).
    unattended: Mutex<HashMap<String, std::collections::VecDeque<DbUnattendedRollback>>>,
    sessions: Mutex<HashMap<String, Arc<DbSession>>>,
    pub(crate) executions: Mutex<HashMap<String, Arc<Running>>>,
    /// 스키마 캐시와 메타데이터용 연결(M3-10).
    pub(crate) schema: crate::schema::SchemaState,
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
            grid_max_bytes: AtomicU64::new(0),
            unattended_keep: Mutex::new(None),
            unattended: Mutex::default(),
            sessions: Mutex::default(),
            executions: Mutex::default(),
            schema: crate::schema::SchemaState::default(),
            next_id: AtomicU64::new(1),
        }
    }

    /// 전역 설정(`[db] grid_max_rows` · `grid_max_bytes` 등). 두 번째부터는 무시한다.
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

    /// 그리드 보유 바이트 상한을 설정 대신 고정한다(시험 · 내장). `None` 이면 설정을 따른다.
    pub fn set_grid_max_bytes(&self, bytes: Option<u64>) {
        self.grid_max_bytes
            .store(bytes.map_or(0, |b| b.max(1)), Ordering::SeqCst);
    }

    /// 그리드 보유 바이트 상한: 고정값, 아니면 설정 `[db] grid_max_bytes`(1 이상의 정수), 아니면 64 MiB.
    pub fn grid_max_bytes(&self) -> u64 {
        match self.grid_max_bytes.load(Ordering::SeqCst) {
            0 => self
                .global
                .get()
                .and_then(|g| {
                    g.snapshot()
                        .get("settings", GRID_MAX_BYTES_KEY)
                        .and_then(serde_json::Value::as_u64)
                })
                .filter(|n| *n >= 1)
                .unwrap_or(DEFAULT_GRID_MAX_BYTES),
            n => n,
        }
    }

    /// 창 없이 남은 열린 트랜잭션의 보존 시간을 설정 대신 고정한다(시험 · 내장). `None` 이면 설정을 따른다.
    pub fn set_unattended_keep(&self, keep: Option<Duration>) {
        *lock(&self.unattended_keep) = keep;
    }

    /// 창 없이 남은 열린 트랜잭션의 보존 시간: 고정값, 아니면 설정 `[db] unattended_transaction_minutes`(1 이상의 정수), 아니면
    /// [`DEFAULT_UNATTENDED_MINUTES`] 분.
    pub fn unattended_keep(&self) -> Duration {
        if let Some(keep) = *lock(&self.unattended_keep) {
            return keep;
        }
        let minutes = self
            .global
            .get()
            .and_then(|g| {
                g.snapshot()
                    .get("settings", UNATTENDED_MINUTES_KEY)
                    .and_then(serde_json::Value::as_u64)
            })
            .filter(|n| *n >= 1)
            .unwrap_or(DEFAULT_UNATTENDED_MINUTES);
        Duration::from_secs(minutes.saturating_mul(60))
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
        if let Some(tab) = &params.tab_id {
            valid_tab_id(tab)?;
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
        let tunnel_lost = opened.tunnel.as_ref().map(DbTunnelLease::lost);
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
            server: opened.server,
            config: opened.config,
            config_changed: AtomicBool::new(false),
            config_files: OnceLock::new(),
            config_stamp: Mutex::new(None),
            confirm_epoch: AtomicU64::new(0),
            window: ctx.window().map(str::to_owned),
            tab: params.tab_id.clone(),
            owner: Mutex::new(Owner {
                closed: ctx.connection_closed(),
                generation: 0,
            }),
            closed: CancellationToken::new(),
            link: Arc::new(AsyncMutex::new(Link {
                conn: Some(opened.connection),
                tunnel: opened.tunnel,
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
        self.watch_owner(&session, 0);
        if let Some(lost) = tunnel_lost {
            self.watch_tunnel(&session, lost);
        }
        Ok(session.opened())
    }

    /// 세션의 SSH 터널이 끊기기를 기다린다(M3-11): 끊기면 도는 실행을 멈추고 연결을 버려 「끊김」(`disconnected` +
    /// `connectionLost`)을 낸다. 다시 붙는 것은 사람이 누를 때다(새 세션).
    fn watch_tunnel(self: &Arc<Self>, session: &Arc<DbSession>, lost: CancellationToken) {
        let (this, watched) = (Arc::clone(self), Arc::clone(session));
        tokio::spawn(async move {
            tokio::select! {
                () = lost.cancelled() => {}
                () = watched.closed.cancelled() => return,
            }
            let running = lock(&watched.running).clone();
            if let Some(running) = running {
                running.halt().await;
            }
            let mut link = watched.link.lock().await;
            if link.conn.is_none() || watched.closed.is_cancelled() {
                // 실행이 끊김을 먼저 보고 알렸거나 세션이 닫혔다.
                return;
            }
            // 실행과 실행 사이 — 열린 트랜잭션은 DB 가 롤백한다.
            let notice = link.lose(false);
            watched.sync(&link);
            drop(link);
            tracing::info!(target: "pw_db", session = %watched.id, connection = %watched.connection_id, "the ssh tunnel of a db session ended");
            this.publish(&watched, Some(notice));
        });
    }

    /// 세션을 쥔 창의 연결이 닫히기를 기다린다(머리 주석의 「창이 닫히면」). `generation` 은 지금 붙음의 순번이다.
    fn watch_owner(self: &Arc<Self>, session: &Arc<DbSession>, generation: u64) {
        let owner_closed = lock(&session.owner).closed.clone();
        let (this, watched) = (Arc::clone(self), Arc::clone(session));
        tokio::spawn(async move {
            tokio::select! {
                () = owner_closed.cancelled() => this.owner_left(&watched, generation).await,
                () = watched.closed.cancelled() => {}
            }
        });
    }

    /// 세션을 쥔 창의 연결이 닫혔다(새로고침 · 웹뷰 재시작 · 창 닫기): 열린 트랜잭션이 없으면 닫고, 있으면 보존한다.
    async fn owner_left(self: &Arc<Self>, session: &Arc<DbSession>, generation: u64) {
        let running = lock(&session.running).clone();
        if let Some(running) = running {
            running.halt().await;
        }
        // 실행이 끝나 상태를 맞출 때까지 기다린다.
        let holds = session.link.lock().await.holds_transaction();
        if lock(&session.owner).generation != generation {
            return; // 그새 다른 연결이 다시 붙었다.
        }
        if !holds {
            tracing::info!(target: "pw_db", session = %session.id, "the window's connection closed; closing its db session");
            self.close_session(session).await;
            return;
        }
        if session.window.is_none() || session.tab.is_none() {
            // 다시 붙을 열쇠가 없다 — 보존하지 않는다. 그래도 몰래가 아니다.
            self.roll_back_unattended(session, generation, Duration::ZERO)
                .await;
            return;
        }
        let keep = self.unattended_keep();
        let until = now_ms().saturating_add(keep.as_millis() as u64);
        tracing::warn!(target: "pw_db", session = %session.id, connection = %session.connection_id, keep_secs = keep.as_secs(), "the window's connection closed with an open transaction; keeping the db session for the window to come back");
        self.publish(session, Some(DbSessionNotice::Unattended { until }));
        tokio::select! {
            () = tokio::time::sleep(keep) => {}
            () = session.closed.cancelled() => return,
        }
        self.roll_back_unattended(session, generation, keep).await;
    }

    /// 아무도 붙지 않은 세션의 열린 트랜잭션을 롤백하고 닫은 뒤 알리고 기록한다. 그새 다시 붙었으면 아무것도 하지 않는다.
    async fn roll_back_unattended(
        &self,
        session: &Arc<DbSession>,
        generation: u64,
        kept: Duration,
    ) {
        {
            // 다시 붙기(`resume`)와 같은 잠금 안에서 판정하고 세션을 떼어 낸다 — 둘 중 하나만 이긴다.
            let mut sessions = lock(&self.sessions);
            if lock(&session.owner).generation != generation
                || sessions.remove(&session.id).is_none()
            {
                return;
            }
        }
        session.closed.cancel();
        let mut link = session.link.lock().await;
        let since = link.since;
        let rollback_error = match link.sql() {
            Some(conn) if conn.transaction_state() != TransactionState::Idle => {
                within(Budget::CONSOLE.cancel_cleanup, "rollback", conn.rollback())
                    .await
                    .err()
                    .map(|e| e.message)
            }
            _ => None,
        };
        link.tx = TransactionState::Idle;
        link.since = None;
        if let Some(conn) = link.conn.take()
            && let Err(e) = within(CLOSE_WAIT, "close", conn.close()).await
        {
            tracing::info!(target: "pw_db", session = %session.id, %e, "closing the db connection failed");
        }
        link.tunnel = None;
        session.sync(&link);
        drop(link);
        let kept_minutes = (kept.as_millis() as u64).div_ceil(60_000);
        tracing::warn!(target: "pw_db", session = %session.id, connection = %session.connection_id, kept_minutes, ?rollback_error, "rolled back an unattended db transaction and closed its session");
        let record = DbUnattendedRollback {
            connection_id: session.connection_id.clone(),
            name: session.name.clone(),
            tab_id: session.tab.clone(),
            since,
            rolled_back_at: now_ms(),
            kept_minutes,
            rollback_error,
        };
        {
            let mut all = lock(&self.unattended);
            let list = all.entry(session.project_id.clone()).or_default();
            if list.len() == UNATTENDED_RECORDS {
                list.pop_front();
            }
            list.push_back(record);
        }
        self.publish(session, Some(DbSessionNotice::UnattendedRolledBack));
        if let Some(bus) = self.bus() {
            let notification = if kept.is_zero() {
                Notification::new(Level::Warning, CLOSED_ROLLBACK_MESSAGE)
            } else {
                Notification::new(Level::Warning, UNATTENDED_ROLLBACK_MESSAGE)
                    .arg("minutes", kept_minutes.to_string())
            };
            notification
                .arg("connection", session.name.clone())
                .show(&bus, Scope::Project(session.project_id.clone()));
        }
    }

    /// `db.session.resume`.
    pub fn resume(
        self: &Arc<Self>,
        ctx: &Ctx,
        params: DbSessionResumeParams,
    ) -> Result<Option<DbSessionOpened>, RpcError> {
        db_grant(ctx.scope(), &params.project_id)?;
        valid_tab_id(&params.tab_id)?;
        let Some(window) = ctx.window() else {
            return Ok(None);
        };
        let found = {
            let sessions = lock(&self.sessions);
            let found = sessions
                .values()
                .find(|s| {
                    s.project_id == params.project_id
                        && s.window.as_deref() == Some(window)
                        && s.tab.as_deref() == Some(params.tab_id.as_str())
                        && lock(&s.owner).closed.is_cancelled()
                        && s.shows_transaction()
                })
                .cloned();
            found.map(|session| {
                let generation = {
                    let mut owner = lock(&session.owner);
                    owner.generation += 1;
                    owner.closed = ctx.connection_closed();
                    owner.generation
                };
                (session, generation)
            })
        };
        let Some((session, generation)) = found else {
            return Ok(None);
        };
        tracing::info!(target: "pw_db", session = %session.id, "the window came back; resuming its db session");
        self.watch_owner(&session, generation);
        self.publish(&session, None);
        Ok(Some(session.opened()))
    }

    /// `db.session.unattendedRollbacks`.
    pub fn unattended_rollbacks(
        &self,
        ctx: &Ctx,
        params: DbUnattendedRollbacksParams,
    ) -> Result<Vec<DbUnattendedRollback>, RpcError> {
        db_grant(ctx.scope(), &params.project_id)?;
        Ok(lock(&self.unattended)
            .get(&params.project_id)
            .map(|list| list.iter().cloned().collect())
            .unwrap_or_default())
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
        link.tunnel = None;
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
        if link.sql().is_none() {
            return Err(match &link.conn {
                Some(DbConnection::Kv(_)) => {
                    RpcError::invalid_params("this connection has no transactions")
                }
                _ => db_error(DbError::new(
                    DbErrorKind::Disconnected,
                    "the connection is closed",
                )),
            });
        }
        // 실행 사이에 끝났으면(버려진 문장 등) 먼저 서버에 물어 맞춘다 — 이미 끝난 트랜잭션의 커밋 · 롤백을 성공으로 답하지 않는다.
        let caught = link.catch_up().await;
        let refused = match &caught {
            Some(DbSessionNotice::TransactionEnded { .. }) => Some(DbError::new(
                DbErrorKind::Invalid,
                "the transaction had already ended before this commit or rollback; the server did not say whether it was committed or rolled back",
            )),
            Some(DbSessionNotice::ConnectionLost { .. }) => Some(DbError::new(
                DbErrorKind::Disconnected,
                "the connection is closed",
            )),
            _ => None,
        };
        let conn = match (refused, link.sql()) {
            (None, Some(conn)) => conn,
            (refused, _) => {
                session.sync(&link);
                drop(link);
                self.publish(&session, caught);
                let error = refused.unwrap_or_else(|| {
                    DbError::new(DbErrorKind::Disconnected, "the connection is closed")
                });
                return Err(RpcError::new(code::DB_ERROR, error.message.clone())
                    .with_data(json!({ "error": error, "session": session.info() })));
            }
        };
        if conn.transaction_state() == TransactionState::Idle {
            // 열린 트랜잭션이 없다 — 상태만 맞춘다.
            let notice = link.reconcile(Step::Rollback, None).or(caught);
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
        let step = if commit { Step::Commit } else { Step::Rollback };
        let notice = link.settle(step, error.as_mut()).await.or(caught);
        session.sync(&link);
        drop(link);
        self.publish(&session, notice);
        if commit {
            self.schema_touched(&session);
        }
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
            config_differs(&c, &s)
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

/// 세션의 연결 설정이 연 때와 다른가(블로킹). 설정 파일 자리의 수정 시각 · 크기가 마지막으로 같다고 본 때와 같으면 다시 읽지
/// 않는다(머리 주석).
fn config_differs(connections: &Connections, session: &DbSession) -> Result<bool, RpcError> {
    let files = match session.config_files.get() {
        Some(files) => files,
        None => {
            let files = connections.connection_files(
                &session.project_id,
                session.worktree.as_deref(),
                &session.connection_id,
            )?;
            session.config_files.get_or_init(|| files)
        }
    };
    // 읽기 전에 찍는다 — 읽는 동안 바뀌면 다음 실행이 다시 읽는다.
    let stamp = stamp_of(files);
    if lock(&session.config_stamp).as_ref() == Some(&stamp) {
        return Ok(false);
    }
    let differs = current_config(connections, session)?.as_ref() != Some(&session.config);
    if !differs {
        *lock(&session.config_stamp) = Some(stamp);
    }
    Ok(differs)
}

/// 탭 id: 1~256자, 제어 문자 없음.
fn valid_tab_id(tab: &str) -> Result<(), RpcError> {
    if tab.is_empty() || tab.len() > MAX_TAB_ID || tab.chars().any(char::is_control) {
        return Err(RpcError::invalid_params(
            "tabId must be 1-256 characters without control characters",
        ));
    }
    Ok(())
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

/// M3-05 의 메서드를 붙인다(세션 · 다시 붙기 · 실행 · 쪽 단위 실행 · 분류 · 연결 테스트).
pub fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    let (open, close, mode, commit, rollback, resume, unattended) = (
        Arc::clone(&sessions),
        Arc::clone(&sessions),
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
        })
        .unary(
            DbSessionResume,
            move |ctx, params: DbSessionResumeParams| {
                let s = Arc::clone(&resume);
                async move { s.resume(&ctx, params) }
            },
        )
        .unary(
            DbSessionUnattendedRollbacks,
            move |ctx, params: DbUnattendedRollbacksParams| {
                let s = Arc::clone(&unattended);
                async move { s.unattended_rollbacks(&ctx, params) }
            },
        );
    crate::execute::register(router, Arc::clone(&sessions));
    crate::schema::register(router, Arc::clone(&sessions));
    crate::kv::register(router, Arc::clone(&sessions));
    crate::edit::register(router, Arc::clone(&sessions));
    crate::connection::register(router, sessions);
}
