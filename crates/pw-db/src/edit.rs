//! 그리드 편집 적용 RPC `db.edit.*`(14-m3-plan.md M3-12, 결정 M3-D9 · M3-D18 · M3-D20 · M3-D21). 문장 만들기는
//! `pw_db_sql::edit`, 이 모듈은 관문 · 세션 · 트랜잭션이다.
//!
//! - **`db.edit.rowKey { sessionId, table }`** → 테이블의 행 키(M3-10 의 판정 `crate::schema::row_key`). 없으면 `null` — 창은
//!   편집 칸을 끄고 까닭을 보인다(M3-17).
//! - **`db.edit.preview { sessionId, table, changes }`** → 문장마다 미리보기 글(값은 리터럴) · 등급 · `previewHash`, 적용 방식
//!   (`applies`). **`db.edit.apply { sessionId, table, changes, previewHash }`** 는 같은 변경을 다시 만들어 해시가 같을 때만 돈다 —
//!   다르면 `CONFLICT`(`previewChanged`). `confirmed` 를 따로 받지 않는다: 변경 제출 미리보기의 [적용] 이 결정 M3-D20 의 확인이고
//!   `previewHash` 가 그 문장들에 묶는다(프로덕션 연결의 「쓰기도 확인」 을 이 미리보기가 채운다).
//! - **`previewHash`** 는 SHA-256 으로 이것들에 묶인다: 세션 id 와 그 세션의 확인 순번(`DbSession::confirm_key` — 관문을 지난
//!   실행이 있으면 올라 미리보기가 쓸모없어진다 · 적용도 관문을 지나 올리므로 같은 해시로 두 번 적용하지 못한다), 방언, 적용
//!   방식(자기 트랜잭션 · 열린 트랜잭션), 한정한 테이블(DB · 스키마 · 이름), 문장마다 서버에 보낼 글(자리 표시)과 바인드 값의
//!   바이트(`DbValue` 의 와이어 JSON). 미리보기 글은 같은 조각에서 만든 리터럴 판이라 따로 넣지 않는다.
//! - **관문**: 그 세션을 연 프로젝트 창만(`session_for` — 다른 프로젝트 창 · 웰컴 · 설정 · 파일 창은 `PERMISSION_DENIED`), 부를
//!   때마다 신뢰 · 연결 설정을 다시 본다(`recheck`), read-only 연결은 거절(`readOnlyConnection` — 메타데이터를 읽기 전에). 만든
//!   문장은 실행 관문과 같은 나누기 · 분류(`crate::gate::plan`, 연결의 방언)를 지나 **문장 하나 · 등급 쓰기 이하**여야 하고, 등급
//!   정책은 관문의 `judge` 가 본다 — 미리보기 해시가 맞은 것을 확인으로 싣되 등급은 쓰기까지만이다(편집 DML 이 쓰기를 넘으면
//!   거절, 이 길로 `confirmed` 를 받는 실행이 생기지 않는다).
//! - **적용**: 자동 커밋 탭(열린 트랜잭션 없음)이면 `BEGIN` … `COMMIT` — 하나라도 영향 행 수가 1 이 아니면 전부 롤백하고 그 변경을
//!   알린다(`CONFLICT`, `data.reason = "rowCount"`, `data.failure`). 수동 커밋 탭이거나 열린 트랜잭션이 있으면 그 안에서
//!   `SAVEPOINT` 를 걸고, 실패하면 **그 세이브포인트까지만** 되돌린다(사람의 다른 변경은 남는다, 커밋은 사람). 실패한 트랜잭션
//!   (PG)에는 걸지 않는다(`CONFLICT`, `transactionFailed`). DB 오류는 `3003`(`data.error` · `data.session` · `data.failure`).
//!   `data.failure.undone` 이 무엇이 되돌려졌는지 말한다 — 서버가 트랜잭션 전체를 끝냈으면(MySQL 교착 등) `transaction` 이고 세션
//!   상태 이벤트가 그 까닭을 싣는다. 적용은 Host 태스크에서 돈다: 창의 요청이 취소되거나 연결이 닫히면 드라이버를 멈춰 되돌리고,
//!   중간에 버려져 열린 트랜잭션이 남지 않는다. 문장 하나가 [`EDIT_STATEMENT_WAIT`](시험은 `set_edit_statement_wait`)를 넘기면
//!   (잠금 기다림 등) 멈추고 되돌린다.
//! - **세션 상태**는 실행 파이프라인과 같은 `Link::settle` 로 맞춘다(오류 뒤에는 서버에 묻는다). 편집 DML 은 스키마를 바꾸지 않아
//!   스키마 캐시를 버리지 않는다.
//!
//! 입력은 모두 struct 다(M5 가 툴로 켤 수 있는 꼴, 결정 M3-D19). `mcp: true` 는 달지 않는다.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pw_db_driver::budget::within;
use pw_db_driver::{Budget, QueryCancel};
use pw_db_sql::edit::{
    DbEditChangeRef, DbEditChanges, DbEditError, DbEditErrorReason, EditStatement,
    MAX_EDIT_CHANGES, build, edit_table,
};
use pw_db_types::{
    BeginOptions, DbError, DbErrorKind, ExecuteRequest, FetchLimit, ObjectRef, RiskGrade,
    SqlDialect, TransactionState,
};
use pw_rpc::{Ctx, JsNumber, Router, RpcError, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::OwnedMutexGuard;
use tokio_util::sync::CancellationToken;

use crate::gate::{
    DbConfirmed, Planned, Policy, code, grade_of, judge, on_classifier_thread, plan,
    statements_hash,
};
use crate::session::{
    DbSession, DbSessionInfo, DbSessionMode, DbSessionNotice, DbSessionState, DbSessions, Link,
    Running, Step, db_error, lock,
};

/// 편집 문장 하나를 기다리는 시간. 넘으면(다른 세션의 잠금 등) 멈추고 편집을 되돌린다 — 자동 커밋 탭의 편집 트랜잭션이 앞 문장의
/// 잠금을 쥔 채 오래 서 있지 않게. 쪽 단위 실행의 비활동 타임아웃(`Budget::PAGE`)과 같다.
pub const EDIT_STATEMENT_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbEditRowKeyParams {
    pub session_id: String,
    /// 테이블. PG 는 `database` · `schema`, MySQL 은 `database` 가 있어야 한다(SQLite 는 없으면 `main`).
    pub table: ObjectRef,
}

/// 테이블의 행 키(M3-10 의 판정).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbRowKey {
    /// 키를 이루는 인덱스(PK 또는 NOT NULL 열만의 유일 인덱스)의 이름.
    pub index: String,
    pub columns: Vec<String>,
    pub primary: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbEditRowKey {
    /// 키 없는 테이블이면 `null` — 그리드 편집을 받지 않는다(결정 M3-D9).
    pub key: Option<DbRowKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbEditPreviewParams {
    pub session_id: String,
    pub table: ObjectRef,
    pub changes: DbEditChanges,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbEditApplyParams {
    pub session_id: String,
    pub table: ObjectRef,
    pub changes: DbEditChanges,
    /// `db.edit.preview` 가 준 그대로.
    pub preview_hash: String,
}

/// 편집이 어느 트랜잭션에서 도는가.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbEditApplies {
    /// 자동 커밋 탭: 편집만의 트랜잭션을 열고 끝에 커밋한다.
    OwnTransaction,
    /// 수동 커밋 탭 · 열린 트랜잭션: 그 안의 세이브포인트에서. 커밋은 사람이 한다.
    OpenTransaction,
}

/// 미리보기의 문장 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbEditPreviewStatement {
    /// 적용 순서(삭제 → 수정 → 추가).
    pub index: u32,
    pub change: DbEditChangeRef,
    /// 값을 리터럴로 적은 글(`pw_db_sql::literal`). 서버에는 같은 문장이 바인드 값으로 간다.
    pub sql: String,
    pub grade: RiskGrade,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbEditPreview {
    pub statements: Vec<DbEditPreviewStatement>,
    /// 가장 높은 등급(쓰기를 넘지 않는다).
    pub grade: RiskGrade,
    /// 행을 가리킨 키 열.
    pub key: Vec<String>,
    pub applies: DbEditApplies,
    /// `db.edit.apply` 에 그대로 싣는다.
    pub preview_hash: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbEditApplied {
    pub statements: u32,
    #[specta(type = JsNumber)]
    pub rows_affected: u64,
    /// 적용 뒤 세션(수동 커밋 탭이면 트랜잭션이 열린 채다).
    pub session: DbSessionInfo,
}

/// 실패한 편집에서 되돌린 것.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbEditUndone {
    /// 편집만의 트랜잭션을 롤백했다(자동 커밋 탭) — 아무것도 바뀌지 않았다.
    Edit,
    /// 세이브포인트까지 되돌렸다 — 열린 트랜잭션의 앞선 변경은 남는다.
    Savepoint,
    /// 서버가 트랜잭션 전체를 끝냈다(교착 · 끊김 등) — 세션 상태 이벤트가 까닭을 싣는다.
    Transaction,
    /// 되돌렸는지 모른다(되돌리기 · 커밋이 실패했다).
    Unknown,
}

/// 실패한 편집: 어느 문장 · 변경에서, 영향 행 수(알면), 무엇이 되돌려졌는가. 커밋 · 세이브포인트 정리의 실패면 문장이 없다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbEditFailure {
    pub statement: Option<u32>,
    pub change: Option<DbEditChangeRef>,
    #[specta(type = Option<JsNumber>)]
    pub rows_affected: Option<u64>,
    pub undone: DbEditUndone,
}

rpc_method! {
    /// 테이블의 행 키(PK 또는 NOT NULL 열만의 유일 키 — M3-10 의 판정, 스키마 캐시를 지난다). 없으면 `key: null`. 관문: 그 세션을 연
    /// 프로젝트 창만 · 신뢰 · 연결 설정.
    pub DbEditRowKeyMethod: "db.edit.rowKey" {
        params: DbEditRowKeyParams,
        result: DbEditRowKey,
        tier: Exec,
        roles: [Owner],
    }

    /// 변경을 방언의 DML 로 만들어 미리보기 글(값은 리터럴) · 등급 · `previewHash` 를 준다. 실행하지 않는다. 키 없는 테이블 · 키 열
    /// 수정 · 리터럴로 적을 수 없는 값은 `INVALID_PARAMS`(`data` 는 `DbEditError`), read-only 연결은 `PERMISSION_DENIED`.
    pub DbEditPreviewMethod: "db.edit.preview" {
        params: DbEditPreviewParams,
        result: DbEditPreview,
        tier: Exec,
        roles: [Owner],
    }

    /// 미리보기한 변경을 적용한다. 같은 변경 · 같은 세션 상태로 다시 만든 해시가 `previewHash` 와 다르면 `CONFLICT`
    /// (`previewChanged`). 자동 커밋 탭이면 한 트랜잭션(영향 행이 1 이 아닌 변경이 있으면 전부 롤백 — `CONFLICT` `rowCount`),
    /// 열린 트랜잭션이면 세이브포인트까지만 되돌린다.
    pub DbEditApplyMethod: "db.edit.apply" {
        params: DbEditApplyParams,
        result: DbEditApplied,
        tier: Exec,
        roles: [Owner],
    }
}

// 실패의 `data.failure` 는 메서드의 출력이 아니라 오류에 실리므로 창이 쓸 타입으로 따로 등록한다.
pw_rpc::rpc_type!(DbEditFailure, DbEditUndone);

/// 만든 문장과 그 분류.
struct Prepared {
    session: Arc<DbSession>,
    dialect: SqlDialect,
    table: ObjectRef,
    key: Vec<String>,
    statements: Vec<EditStatement>,
    /// 문장마다 하나(관문의 분류).
    planned: Vec<Planned>,
    /// MySQL 수정의 잠그는 읽기의 등급(문장 번호 → 등급).
    probes: Vec<Option<RiskGrade>>,
}

fn edit_error(e: DbEditError) -> RpcError {
    RpcError::new(rpc_code::INVALID_PARAMS, e.describe())
        .with_data(serde_json::to_value(e).unwrap_or_default())
}

fn read_only() -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        "this connection is read-only: grid edits do not run",
    )
    .with_data(json!({ "reason": "readOnlyConnection" }))
}

fn conflict(reason: &str, message: &str) -> RpcError {
    RpcError::new(rpc_code::CONFLICT, message.to_owned()).with_data(json!({ "reason": reason }))
}

fn disconnected() -> DbError {
    DbError::new(DbErrorKind::Disconnected, "the connection is closed")
}

/// 문장 하나 · 등급 쓰기 이하 · 트랜잭션 제어 · 세션 설정이 아님 — 편집 DML 이 관문의 분류에서 쓰기를 넘지 않는다.
fn check_planned(sql: &str, mut planned: Vec<Planned>) -> Result<Planned, RpcError> {
    let refused = |why: &str| {
        tracing::warn!(target: "pw_db", %why, "an edit statement did not classify as a single write");
        RpcError::new(
            rpc_code::INVALID_PARAMS,
            format!("the edit statement is not a single write ({why})"),
        )
        .with_data(json!({ "reason": "notAWrite", "sql": sql }))
    };
    if planned.len() != 1 {
        return Err(refused("not one statement"));
    }
    let statement = planned.remove(0);
    if statement.grade > RiskGrade::Write {
        return Err(refused("graded above a write"));
    }
    if statement.control.is_some() || statement.settings {
        return Err(refused("a transaction or settings statement"));
    }
    Ok(statement)
}

/// 등급 정책(관문의 `judge`, 결정 M3-D20). 미리보기의 [적용] 이 확인이므로 해시가 맞은 확인을 싣되 등급은 쓰기까지다 — 쓰기를 넘는
/// 문장이면 `judge` 가 받지 않는다(그 오류를 그대로 돌려주고 이 길에는 `confirmed` 를 실을 자리가 없다).
fn gate_policy(
    policy: &Policy,
    confirm_key: &str,
    dialect: SqlDialect,
    planned: &[Planned],
) -> Result<(), RpcError> {
    let hash = statements_hash(Some(confirm_key), dialect, planned, &[]);
    let confirmed = DbConfirmed {
        grade: RiskGrade::Write,
        statements_hash: hash.clone(),
        name: None,
    };
    judge(policy, planned, &[], &hash, Some(&confirmed))
}

fn policy_of(session: &DbSession) -> Policy {
    Policy {
        read_only: session.read_only,
        production: session.production,
        name: session.name.clone(),
    }
}

/// 미리보기 해시(머리 주석의 「`previewHash`」).
fn preview_hash(
    confirm_key: &str,
    dialect: SqlDialect,
    applies: DbEditApplies,
    table: &ObjectRef,
    statements: &[EditStatement],
) -> String {
    let mut hasher = Sha256::new();
    let mut part = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    part(b"pitwall.db.edit.v1");
    part(confirm_key.as_bytes());
    part(format!("{dialect:?}").as_bytes());
    part(format!("{applies:?}").as_bytes());
    for name in [&table.database, &table.schema, &Some(table.name.clone())] {
        match name {
            Some(n) => {
                part(b"1");
                part(n.as_bytes());
            }
            None => part(b"0"),
        }
    }
    for statement in statements {
        part(statement.sql.as_bytes());
        part(&(statement.params.len() as u64).to_le_bytes());
        for value in &statement.params {
            part(&serde_json::to_vec(value).unwrap_or_default());
        }
    }
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 세션이 보이는 상태로 고른 적용 방식. 실패한 트랜잭션 · 끊긴 연결은 거절.
fn applies_of(info: &DbSessionInfo) -> Result<DbEditApplies, RpcError> {
    match info.state {
        DbSessionState::Disconnected => Err(db_error(disconnected())),
        DbSessionState::Failed { .. } => Err(transaction_failed()),
        DbSessionState::InTransaction { .. } => Ok(DbEditApplies::OpenTransaction),
        _ if info.mode == DbSessionMode::Manual => Ok(DbEditApplies::OpenTransaction),
        _ => Ok(DbEditApplies::OwnTransaction),
    }
}

fn transaction_failed() -> RpcError {
    conflict(
        "transactionFailed",
        "the open transaction failed; roll it back before editing",
    )
}

impl DbSessions {
    /// 관문(범위 · 신뢰 · 설정 · read-only) → 테이블 한정 → 행 키 → 문장 → 분류.
    async fn prepare_edit(
        self: &Arc<Self>,
        ctx: &Ctx,
        session_id: &str,
        table: &ObjectRef,
        changes: DbEditChanges,
    ) -> Result<Prepared, RpcError> {
        let (session, grant) = self.session_for(ctx, session_id)?;
        let dialect = session
            .dialect()
            .ok_or_else(|| RpcError::invalid_params("this connection does not run SQL"))?;
        self.recheck(&session).await?;
        if session.read_only {
            return Err(read_only());
        }
        // 메타데이터(연결)를 읽기 전에 입력을 본다.
        let table = edit_table(dialect, table).map_err(edit_error)?;
        if changes.is_empty() || changes.len() > MAX_EDIT_CHANGES {
            return Err(edit_error(DbEditError {
                change: None,
                column: None,
                reason: if changes.is_empty() {
                    DbEditErrorReason::NoChanges
                } else {
                    DbEditErrorReason::TooManyChanges
                },
            }));
        }
        let (columns, key) = self
            .table_shape(
                ctx,
                &grant,
                session.worktree.as_deref(),
                &session.connection_id,
                table.clone(),
            )
            .await?;
        let columns: Vec<String> = columns.into_iter().map(|c| c.name).collect();
        let key = key
            .ok_or_else(|| {
                edit_error(DbEditError {
                    change: None,
                    column: None,
                    reason: DbEditErrorReason::NoRowKey,
                })
            })?
            .columns;
        let (t, k) = (table.clone(), key.clone());
        let (statements, planned, probes) = on_classifier_thread(move || {
            let statements = build(dialect, &t, &columns, &k, &changes).map_err(edit_error)?;
            let mut planned = Vec::with_capacity(statements.len());
            let mut probes = Vec::with_capacity(statements.len());
            for (i, statement) in statements.iter().enumerate() {
                let mut one = check_planned(&statement.sql, plan(dialect, &statement.sql, None)?)?;
                one.index = i as u32;
                planned.push(one);
                probes.push(match &statement.probe {
                    Some(probe) => {
                        Some(check_planned(&probe.sql, plan(dialect, &probe.sql, None)?)?.grade)
                    }
                    None => None,
                });
            }
            Ok::<_, RpcError>((statements, planned, probes))
        })
        .await??;
        Ok(Prepared {
            session,
            dialect,
            table,
            key,
            statements,
            planned,
            probes,
        })
    }

    /// `db.edit.rowKey`.
    pub(crate) async fn edit_row_key(
        self: &Arc<Self>,
        ctx: &Ctx,
        p: DbEditRowKeyParams,
    ) -> Result<DbEditRowKey, RpcError> {
        let (session, grant) = self.session_for(ctx, &p.session_id)?;
        let dialect = session
            .dialect()
            .ok_or_else(|| RpcError::invalid_params("this connection does not run SQL"))?;
        self.recheck(&session).await?;
        let table = edit_table(dialect, &p.table).map_err(edit_error)?;
        let key = self
            .table_row_key(
                ctx,
                &grant,
                session.worktree.as_deref(),
                &session.connection_id,
                table,
            )
            .await?;
        Ok(DbEditRowKey {
            key: key.map(|k| DbRowKey {
                index: k.index,
                columns: k.columns,
                primary: k.primary,
            }),
        })
    }

    /// `db.edit.preview`.
    pub(crate) async fn edit_preview(
        self: &Arc<Self>,
        ctx: &Ctx,
        p: DbEditPreviewParams,
    ) -> Result<DbEditPreview, RpcError> {
        let prepared = self
            .prepare_edit(ctx, &p.session_id, &p.table, p.changes)
            .await?;
        let session = &prepared.session;
        let applies = applies_of(&session.info())?;
        gate_policy(
            &policy_of(session),
            &session.confirm_key(),
            prepared.dialect,
            &prepared.planned,
        )?;
        Ok(DbEditPreview {
            statements: prepared
                .statements
                .iter()
                .zip(&prepared.planned)
                .map(|(s, planned)| DbEditPreviewStatement {
                    index: planned.index,
                    change: s.change,
                    sql: s.preview.clone(),
                    grade: planned.grade,
                })
                .collect(),
            grade: grade_of(&prepared.planned),
            key: prepared.key.clone(),
            applies,
            preview_hash: preview_hash(
                &session.confirm_key(),
                prepared.dialect,
                applies,
                &prepared.table,
                &prepared.statements,
            ),
        })
    }

    /// `db.edit.apply`.
    pub(crate) async fn edit_apply(
        self: Arc<Self>,
        ctx: Ctx,
        p: DbEditApplyParams,
    ) -> Result<DbEditApplied, RpcError> {
        let prepared = self
            .prepare_edit(&ctx, &p.session_id, &p.table, p.changes)
            .await?;
        let session = Arc::clone(&prepared.session);
        let mut link = Arc::clone(&session.link)
            .try_lock_owned()
            .map_err(|_| conflict("busy", "the session is running statements"))?;
        // 실행 사이에 드라이버 상태가 바뀌었으면 서버에 물어 맞춘다(실행 파이프라인과 같다).
        if let Some(notice) = link.catch_up().await {
            session.sync(&link);
            self.publish(&session, Some(notice));
        }
        if link.sql().is_none() {
            return Err(db_error(disconnected()));
        }
        if link.tx == TransactionState::Failed && link.holds_transaction() {
            return Err(transaction_failed());
        }
        let applies = if link.mode == DbSessionMode::Manual || link.holds_transaction() {
            DbEditApplies::OpenTransaction
        } else {
            DbEditApplies::OwnTransaction
        };
        let hash = preview_hash(
            &session.confirm_key(),
            prepared.dialect,
            applies,
            &prepared.table,
            &prepared.statements,
        );
        if hash != p.preview_hash {
            return Err(conflict(
                "previewChanged",
                "the edit or the session changed since the preview; preview it again",
            ));
        }
        gate_policy(
            &policy_of(&session),
            &session.confirm_key(),
            prepared.dialect,
            &prepared.planned,
        )?;
        // 관문을 지났다 — 이 미리보기와 낸 확인은 이것으로 끝이다(같은 해시로 두 번 적용하지 못한다).
        session.spend_confirmations();

        let Some(conn) = link.sql() else {
            return Err(db_error(disconnected()));
        };
        let running = Arc::new(Running {
            execution_id: self.next_id("dbe-"),
            session_id: session.id.clone(),
            project_id: session.project_id.clone(),
            dialect: Some(prepared.dialect),
            cancel: QueryCancel::new(conn.interrupter(), Budget::CONSOLE.cancel_cleanup),
            stop: CancellationToken::new(),
            current: std::sync::Mutex::new(None),
        });
        // 창이 닫혀 세션을 닫으면 그 자리의 실행을 멈춘다(`close_session`).
        *lock(&session.running) = Some(Arc::clone(&running));
        tracing::info!(target: "pw_db", session = %session.id, statements = prepared.statements.len(), ?applies, "applying a grid edit");
        let task = tokio::spawn(run_edit(
            Arc::clone(&self),
            link,
            prepared,
            applies,
            Arc::clone(&running),
        ));
        // 요청이 버려지면(창의 취소 · 연결이 닫힘) 드라이버를 멈춘다 — 태스크는 되돌리기까지 끝낸다.
        let mut guard = HaltOnDrop(Some(running));
        let result = task.await.map_err(|e| RpcError::internal(e.to_string()))?;
        guard.0 = None;
        result
    }
}

/// 요청이 버려지면 편집을 멈춘다(되돌리기는 태스크가 한다).
struct HaltOnDrop(Option<Arc<Running>>);

impl Drop for HaltOnDrop {
    fn drop(&mut self) {
        if let Some(running) = self.0.take() {
            tokio::spawn(async move { running.halt().await });
        }
    }
}

async fn run_edit(
    sessions: Arc<DbSessions>,
    mut link: OwnedMutexGuard<Link>,
    prepared: Prepared,
    applies: DbEditApplies,
    running: Arc<Running>,
) -> Result<DbEditApplied, RpcError> {
    let session = Arc::clone(&prepared.session);
    let mut run = EditRun {
        sessions: &sessions,
        session: &session,
        link: &mut link,
        running: &running,
    };
    let result = run.apply(&prepared, applies).await;
    session.sync(&link);
    let info = session.info();
    drop(link);
    sessions.publish(&session, None);
    let mut slot = lock(&session.running);
    if slot
        .as_ref()
        .is_some_and(|r| r.execution_id == running.execution_id)
    {
        *slot = None;
    }
    drop(slot);
    match result {
        Ok((statements, rows_affected)) => {
            tracing::info!(target: "pw_db", session = %session.id, statements, "grid edit applied");
            Ok(DbEditApplied {
                statements,
                rows_affected,
                session: info,
            })
        }
        Err(Failed { failure, error }) => {
            tracing::info!(target: "pw_db", session = %session.id, ?failure, error = ?error.as_ref().map(|e| e.kind), "grid edit failed");
            Err(match error {
                Some(error) => RpcError::new(code::DB_ERROR, error.message.clone())
                    .with_data(json!({ "error": error, "session": info, "failure": failure })),
                None => RpcError::new(
                    rpc_code::CONFLICT,
                    "a change did not touch exactly one row; the edit was undone",
                )
                .with_data(json!({ "reason": "rowCount", "session": info, "failure": failure })),
            })
        }
    }
}

/// 실패: 무엇이 실패했고(DB 오류면 그것) 무엇을 되돌렸는가.
struct Failed {
    failure: DbEditFailure,
    error: Option<DbError>,
}

/// 한 번의 적용(연결 잠금을 쥔 동안).
struct EditRun<'a> {
    sessions: &'a DbSessions,
    session: &'a DbSession,
    link: &'a mut Link,
    running: &'a Running,
}

impl EditRun<'_> {
    /// 세션 상태를 맞추고 알릴 것이 있으면 알린다.
    async fn settle(&mut self, step: Step, error: Option<&mut DbError>) {
        if let Some(notice) = self.link.settle(step, error).await {
            self.notify(notice);
        }
    }

    fn notify(&mut self, notice: DbSessionNotice) {
        self.session.sync(self.link);
        self.sessions.publish(self.session, Some(notice));
    }

    /// (문장 수, 영향 행 합).
    async fn apply(
        &mut self,
        prepared: &Prepared,
        applies: DbEditApplies,
    ) -> Result<(u32, u64), Failed> {
        let opening = Failed {
            failure: DbEditFailure {
                statement: None,
                change: None,
                rows_affected: None,
                undone: DbEditUndone::Edit,
            },
            error: None,
        };
        let savepoint = match applies {
            DbEditApplies::OwnTransaction => {
                let begun = match self.link.sql() {
                    Some(conn) => {
                        self.running
                            .cancel
                            .run(conn.begin(BeginOptions::default()))
                            .await
                    }
                    None => Err(disconnected()),
                };
                let mut error = begun.err();
                self.settle(Step::Begin, error.as_mut()).await;
                if let Some(error) = error {
                    return Err(Failed {
                        error: Some(error),
                        ..opening
                    });
                }
                None
            }
            DbEditApplies::OpenTransaction => {
                // 수동 커밋 탭에 아직 트랜잭션이 없으면 사람의 트랜잭션을 연다(콘솔의 첫 문장과 같다).
                if let Err(mut error) = self
                    .running
                    .cancel
                    .run(self.link.ensure_transaction(false))
                    .await
                {
                    self.settle(Step::Begin, Some(&mut error)).await;
                    return Err(Failed {
                        error: Some(error),
                        ..opening
                    });
                }
                let name = self.sessions.next_id("pitwall_edit_");
                let made = match self.link.sql() {
                    Some(conn) => self.running.cancel.run(conn.savepoint(name.clone())).await,
                    None => Err(disconnected()),
                };
                if let Err(mut error) = made {
                    self.settle(statement_step(), Some(&mut error)).await;
                    let undone = if self.link.holds_transaction() {
                        DbEditUndone::Savepoint
                    } else {
                        DbEditUndone::Transaction
                    };
                    return Err(Failed {
                        failure: DbEditFailure {
                            undone,
                            ..opening.failure
                        },
                        error: Some(error),
                    });
                }
                Some(name)
            }
        };

        let mut total = 0u64;
        let mut failed: Option<(DbEditFailure, Option<DbError>)> = None;
        for (i, statement) in prepared.statements.iter().enumerate() {
            let grade = prepared.planned[i].grade;
            let outcome = self.one(statement, grade, prepared.probes[i]).await;
            let failure = |rows_affected| DbEditFailure {
                statement: Some(i as u32),
                change: Some(statement.change),
                rows_affected,
                undone: DbEditUndone::Unknown,
            };
            match outcome {
                Ok(Some(1)) => total += 1,
                Ok(rows) => {
                    failed = Some((failure(rows), None));
                    break;
                }
                Err(mut error) => {
                    self.settle(statement_step(), Some(&mut error)).await;
                    failed = Some((failure(None), Some(error)));
                    break;
                }
            }
        }
        if let Some((mut failure, error)) = failed {
            failure.undone = self.undo(savepoint.as_deref()).await;
            return Err(Failed { failure, error });
        }

        // 끝: 자기 트랜잭션은 커밋, 세이브포인트는 놓는다(트랜잭션은 사람의 것으로 남는다).
        let finished = match &savepoint {
            None => {
                // 커밋은 끝까지 기다린다 — 도중에 버리면 결과도 연결의 프로토콜 상태도 모른다. 요청이 버려져도 커밋은 끝난다.
                let committed = match self.link.sql() {
                    Some(conn) => conn.commit().await,
                    None => Err(disconnected()),
                };
                let mut error = committed.err();
                self.settle(Step::Commit, error.as_mut()).await;
                error.map(|e| {
                    let undone = if e.rolled_back {
                        DbEditUndone::Edit
                    } else {
                        DbEditUndone::Unknown
                    };
                    (e, undone)
                })
            }
            Some(name) => {
                let released = match self.link.sql() {
                    Some(conn) => {
                        within(
                            Budget::CONSOLE.cancel_cleanup,
                            "release savepoint",
                            conn.release_savepoint(name.clone()),
                        )
                        .await
                    }
                    None => Err(disconnected()),
                };
                match released {
                    Ok(()) => {
                        let _ = self.link.reconcile(statement_step(), None);
                        None
                    }
                    Err(mut error) => {
                        self.settle(statement_step(), Some(&mut error)).await;
                        let undone = self.undo(Some(name)).await;
                        Some((error, undone))
                    }
                }
            }
        };
        match finished {
            None => Ok((prepared.statements.len() as u32, total)),
            Some((error, undone)) => Err(Failed {
                failure: DbEditFailure {
                    statement: None,
                    change: None,
                    rows_affected: None,
                    undone,
                },
                error: Some(error),
            }),
        }
    }

    /// 문장 하나 → 영향 행 수(드라이버가 모르면 `None`). MySQL 수정은 먼저 잠그는 읽기로 맞는 행을 세어(2행까지) 정확히 1행일 때만
    /// 수정한다 — 아니면 그 수를 돌려준다(MySQL 의 영향 행은 바뀐 행이라 키가 여러 행에 맞아도 하나만 바뀌면 1 이다).
    async fn one(
        &mut self,
        statement: &EditStatement,
        grade: RiskGrade,
        probe_grade: Option<RiskGrade>,
    ) -> Result<Option<u64>, DbError> {
        let timer = Timer::start(self.running, self.sessions.edit_statement_wait());
        let result = self.execute(statement, grade, probe_grade).await;
        timer.finish(result)
    }

    async fn execute(
        &mut self,
        statement: &EditStatement,
        grade: RiskGrade,
        probe_grade: Option<RiskGrade>,
    ) -> Result<Option<u64>, DbError> {
        let conn = self.link.sql().ok_or_else(disconnected)?;
        let probed = match (&statement.probe, probe_grade) {
            (Some(probe), Some(probe_grade)) => {
                let mut request = ExecuteRequest::new(probe.sql.clone(), probe_grade);
                request.params = probe.params.clone();
                let started = self.running.cancel.run(conn.execute(request)).await?;
                let matched = if started.end.is_some() {
                    0
                } else {
                    let batch = self
                        .running
                        .cancel
                        .run(conn.fetch(FetchLimit::rows(2)))
                        .await?;
                    if batch.end.is_none() {
                        self.running.cancel.run(conn.discard_cursor()).await?;
                    }
                    batch.rows.len() as u64
                };
                if matched != 1 {
                    return Ok(Some(matched));
                }
                true
            }
            _ => false,
        };
        let mut request = ExecuteRequest::new(statement.sql.clone(), grade);
        request.params = statement.params.clone();
        let started = self.running.cancel.run(conn.execute(request)).await?;
        let affected = match started.end {
            Some(end) => end.rows_affected,
            None => {
                // 편집 DML 은 행을 돌려주지 않는다 — 돌려주면 커서를 버리고 영향 행 수를 모른다고 본다.
                self.running.cancel.run(conn.discard_cursor()).await?;
                None
            }
        };
        Ok(match affected {
            // 잠근 그 한 행이다 — 값이 그대로면 MySQL 이 0 으로 센다.
            Some(0 | 1) if probed => Some(1),
            affected => affected,
        })
    }

    /// 실패한 편집을 되돌린다. 취소된 뒤에도 돌도록 실행의 취소 토큰을 쓰지 않고 정리 예산 안에서 부른다.
    async fn undo(&mut self, savepoint: Option<&str>) -> DbEditUndone {
        let cleanup = Budget::CONSOLE.cancel_cleanup;
        match savepoint {
            None => {
                if !self.link.holds_transaction() {
                    // 서버가 이미 끝냈다(끊김이면 DB 가 롤백한다) — 편집 트랜잭션에는 사람의 것이 없다.
                    return DbEditUndone::Edit;
                }
                let rolled = match self.link.sql() {
                    Some(conn) => within(cleanup, "rollback", conn.rollback()).await,
                    None => Err(disconnected()),
                };
                let mut error = rolled.err();
                let failed = error.is_some();
                self.settle(Step::Rollback, error.as_mut()).await;
                if !failed || !self.link.holds_transaction() {
                    DbEditUndone::Edit
                } else {
                    DbEditUndone::Unknown
                }
            }
            Some(name) => {
                if !self.link.holds_transaction() {
                    return DbEditUndone::Transaction;
                }
                let rolled = match self.link.sql() {
                    Some(conn) => {
                        within(
                            cleanup,
                            "rollback to savepoint",
                            conn.rollback_to_savepoint(name.to_owned()),
                        )
                        .await
                    }
                    None => Err(disconnected()),
                };
                if let Err(mut error) = rolled {
                    self.settle(statement_step(), Some(&mut error)).await;
                    return if self.link.holds_transaction() {
                        DbEditUndone::Unknown
                    } else {
                        DbEditUndone::Transaction
                    };
                }
                if let Some(conn) = self.link.sql() {
                    // 되돌린 세이브포인트는 남아 있다 — 놓는다(실패해도 되돌림은 끝났다).
                    let _ = within(
                        cleanup,
                        "release savepoint",
                        conn.release_savepoint(name.to_owned()),
                    )
                    .await;
                }
                // 실패한 트랜잭션(PG)이 세이브포인트로 되살아났다 — 드라이버 상태로 맞춘다.
                let _ = self.link.reconcile(statement_step(), None);
                DbEditUndone::Savepoint
            }
        }
    }
}

/// 편집 문장의 단계(사람의 문장과 같은 꼴 — 트랜잭션을 끝내는 문장이 아니다).
fn statement_step() -> Step {
    Step::Statement {
        implicit_commit: false,
        may_end: false,
    }
}

/// 문장 하나의 기다림 상한([`DbSessions::edit_statement_wait`], 기본 [`EDIT_STATEMENT_WAIT`]). 넘으면 실행을 멈추고, 그 문장의 취소 오류를 타임아웃으로 바꾼다.
struct Timer {
    task: tokio::task::JoinHandle<()>,
    fired: Arc<AtomicBool>,
    wait: Duration,
}

impl Timer {
    fn start(running: &Running, wait: Duration) -> Self {
        let fired = Arc::new(AtomicBool::new(false));
        let (flag, cancel, stop) = (
            Arc::clone(&fired),
            running.cancel.clone(),
            running.stop.clone(),
        );
        let task = tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            flag.store(true, Ordering::SeqCst);
            stop.cancel();
            let _ = cancel.cancel().await;
        });
        Self { task, fired, wait }
    }

    fn finish<T>(self, result: Result<T, DbError>) -> Result<T, DbError> {
        self.task.abort();
        match result {
            Err(e) if self.fired.load(Ordering::SeqCst) => {
                let mut timeout = DbError::timeout("an edit statement");
                timeout.message = format!(
                    "an edit statement waited over {} ms (locked by another session?): {}",
                    self.wait.as_millis(),
                    e.message
                );
                Err(timeout)
            }
            other => other,
        }
    }
}

impl DbSessions {
    /// 편집 문장 하나의 기다림 상한을 고정한다(시험). `None` 이면 [`EDIT_STATEMENT_WAIT`].
    pub fn set_edit_statement_wait(&self, wait: Option<Duration>) {
        self.edit_wait_ms.store(
            wait.map_or(0, |w| (w.as_millis() as u64).max(1)),
            Ordering::SeqCst,
        );
    }

    /// 편집 문장 하나의 기다림 상한.
    pub fn edit_statement_wait(&self) -> Duration {
        match self.edit_wait_ms.load(Ordering::SeqCst) {
            0 => EDIT_STATEMENT_WAIT,
            ms => Duration::from_millis(ms),
        }
    }
}

pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    let (key, preview, apply) = (Arc::clone(&sessions), Arc::clone(&sessions), sessions);
    router
        .unary(DbEditRowKeyMethod, move |ctx, p: DbEditRowKeyParams| {
            let s = Arc::clone(&key);
            async move { s.edit_row_key(&ctx, p).await }
        })
        .unary(DbEditPreviewMethod, move |ctx, p: DbEditPreviewParams| {
            let s = Arc::clone(&preview);
            async move { s.edit_preview(&ctx, p).await }
        })
        .unary(DbEditApplyMethod, move |ctx, p: DbEditApplyParams| {
            let s = Arc::clone(&apply);
            async move { s.edit_apply(ctx, p).await }
        });
}

#[cfg(test)]
mod tests;
