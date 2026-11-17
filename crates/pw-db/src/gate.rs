//! 실행 관문(M3-05, 결정 M3-D18 · M3-D20 · M3-D6) — 연결을 열거나 실행하기 전의 한 자리.
//!
//! 1. **연결 범위** — [`DbGrant`] 를 만드는 입구는 둘이다: 창의 입구 [`db_grant`](`scope_may_write` 를 지난
//!    `ConnectionScope::Project(그 id)` 연결 — 다른 프로젝트 창 · 웰컴 · 설정 · 파일 창 · MCP 세션은 `PERMISSION_DENIED`)와 MCP
//!    세션의 입구 [`db_grant_for_agent`](결정 M5-D2 · M5-D27 · M5-D37 — 「사람이 연 연결」 과 대상에 묶은 표지, 아래 「에이전트의
//!    입구」). 연결을 열거나 실행하는 함수(`crate::connection`, `crate::session`)는 이것을 받아야 하고, 이 모듈 밖에서는 만들 수
//!    없다 — 창의 증거도 에이전트의 증거도:
//!
//!    ```compile_fail
//!    let _ = pw_db::gate::DbGrant { project_id: String::from("p"), agent: None };
//!    ```
//!
//!    ```compile_fail
//!    fn forge(grant: pw_db::gate::DbGrant) -> pw_db::gate::DbGrant {
//!        pw_db::gate::DbGrant { agent: None, ..grant }
//!    }
//!    ```
//!
//! 2. **신뢰 게이트** — M3-02 의 `db_trust_gate`(`Connections::trust_gate`)를 연결을 열 때와 실행마다 지난다.
//! 3. **분류** — 문장 나누기(M3-04)와 위험도 분류(M3-03)를 **연결의 드라이버 방언**으로(요청의 방언이 아니다 — 같은 글이 방언에 따라
//!    등급이 갈린다). 분류는 큰 스택의 전용 스레드에서 부른다([`on_classifier_thread`] — M3-03 의 입력 상한이 2 MiB 스택을 지키지만
//!    넘침은 프로세스를 끄므로 겹방어). 등급별 정책([`judge`], 결정 M3-D20): read-only 연결은 조회 밖을 모두 거절하고 `SET` · `RESET` ·
//!    `SET TRANSACTION` 도 거절한다. 확인이 필요하면 [`code::CONFIRM_REQUIRED`] 가 문장 해시를 돌려주고, 창이 확인을 받은 뒤
//!    `confirmed { grade, statementsHash, name? }` 로 다시 부른다 — Host 는 지금 실행할 문장들(세션 id · 방언 · 원문 · 트랜잭션 효과)로
//!    해시를 다시 만들어 같고 등급이 그 이상일 때만 돌린다. 프로덕션 연결의 파괴 등급은 `name` 이 연결 이름과 같아야 한다.
//! 4. **read-only 실행은 드라이버의 읽기 전용 트랜잭션 안에서**(결정 M3-D6) — 실행 파이프라인(`crate::execute`)이 연다.
//!
//! # 에이전트의 입구
//!
//! [`db_grant_for_agent`] 는 `pw_core::registry::agent::agent_grant` 를 지난 호출(MCP 세션의 툴 메서드)에만 [`DbGrant`] 를 준다.
//! 판정 순서: 연결 id 꼴 → 신뢰 게이트(`untrusted`) → 연결 설정 읽기(메인 체크아웃 · 그 워크트리 — 둘 다 표지를 기억에 더한다) →
//! 표지(그 대상에서 본 가장 엄한 것, 앱 실행을 넘어 기억한다 — 프로덕션이면 `[mcp.policy] production` 이 `block`(기본)일 때
//! `productionBlocked`) → 「사람이 연 연결」 또는 그 MCP 세션의 「승인받은 연결」(아니면 `notOpenedByUser`). 연 적 없는 연결에 「이
//! 연결 열기」 승인을 청하는 증거는 `db_connection_test` 의 입구 [`db_grant_to_open`] 만 준다(16-m5-plan M5-17 ③, 결정 M5-D21 —
//! 프로덕션 대상은 그 입구도 묻지 않고 거절한다). 프로덕션 대상을 `ask` 로 지난 증거는 **호출마다 승인을 청한다**
//! (16-m5-plan M5-18 ④): 툴은 계획(`pw_core::registry::plan`)을 만들어 승인을 받고 [`DbGrant::approve`] 로 그 증거에 단다 — 그
//! 전에는 연결을 여는 자리 · 스키마 캐시가 그 증거를 거절한다(fail-closed). 정책은 MCP 세션과 같은 함수로 읽는다
//! (`pw_core::config::mcp_policy` — 사람의 설정에서만, 메인 판정 2026-11-17). 거절은 모두
//! `PERMISSION_DENIED` 이고 `data.reason` 이 까닭이다. 그 증거는 (연결 id · 워크트리 · 푼 대상)에 묶인다 — 연결을 여는 자리
//! (`crate::connection` 의 드라이버 연결 직전, `crate::schema` 의 캐시 앞)가 그때 다시 읽은 설정의 대상이 증거와 다르면 연결을
//! 열지 않고 `notOpenedByUser` 로 거절한다(판정과 열기 사이에 설정 파일이 바뀌는 틈).
//!
//! - **「사람이 연 연결」**: 그 프로젝트 창의 증거([`db_grant`])로 저장한 연결을 드라이버까지 연 적이 있는 (프로젝트, 연결 id, 푼
//!   대상). 연결을 여는 길은 모두 `crate::connection` 의 한 자리를 지나므로(세션 · 메타데이터 · 쪽 읽기 · 연결 테스트) 거기서
//!   기록한다. 저장 전 초안의 연결 테스트는 기록하지 않는다. `DbSessions` 의 메모리에만 있다(앱을 끄면 사라진다).
//! - **「승인받은 연결」**(M5-17): `db_connection_test` 가 「이 연결 열기」 승인을 받은 (MCP 세션, 프로젝트, 연결 id, 푼 대상). MCP
//!   세션 하나가 Host 연결 하나라 세션 열쇠는 `pw_rpc::Ctx::session` 이다. 그 세션의 다른 DB 툴도 그 연결을 쓰고, 세션이 끝나면
//!   지운다. 「사람이 연 연결」 기록과 섞지 않는다 — 에이전트의 증거로 연 연결은 그 기록에 더하지 않는다. 프로덕션 대상은 이
//!   기록으로 지나지 못한다.
//! - **표지**([`DbMarks`]): `Connections` 가 연결 설정을 읽어 올리는 자리(`load` · `refresh` — 목록 · 열기 · 툴 호출, 어느
//!   워크트리의 것이든)에서 연결마다 프로덕션 · read-only 표지를 그 **푼 대상**([`DbTarget`])에 묶어 더한다. 연결 id 와 무관하고
//!   사람이 열었는지와도 무관하다. 조여지기만 한다 — 설정에서 지워도, 표지 없는 새 연결 id 로 같은 대상을 가리켜도 남는다.
//!   대상만 열쇠다(프로젝트를 가리지 않는다 — 같은 대상은 같은 데이터다). 한 연결로 다른 DB 를 읽을 수 있는 드라이버(MySQL ·
//!   MariaDB — 메인 판정 2026-11-14, Redis 의 DB 번호 — 메인 판정 2026-11-17)는 열쇠에서 DB 를 뺀다(`DbTarget::mark_key`).
//!   에이전트 호출은 그 저장소의 메인
//!   체크아웃(등록 워크트리면 메인 저장소 프로젝트의 주 루트)의 연결 설정도 읽는다.

use std::fmt::Write as _;

use pw_core::registry::state::scope_may_write;
use pw_db_sql::risk::{RiskReason, classify_statement_readings};
use pw_db_sql::split::{SqlRange, split};
use pw_db_sql::txn::{
    TxControl, changes_session_settings, implicitly_commits, may_end_transaction,
    transaction_control,
};
use pw_db_types::{RiskGrade, SqlDialect};
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

/// 실행 관문의 애플리케이션 오류 코드(`data` 는 객체). M3-02 의 31xx 와 겹치지 않는다.
pub mod code {
    /// 확인이 필요하다(`data` 는 [`super::DbConfirmRequired`]). 창은 등급 · 문장을 보여 주고 확인을 받은 뒤 `confirmed` 로 다시 부른다.
    pub const CONFIRM_REQUIRED: i32 = 3001;
    /// 이 취소는 트랜잭션 전체를 되돌린다(SQLite 수동 트랜잭션 안의 쓰기). `data`: `executionId` · `reason`(`rollsBackTransaction`).
    /// 창은 그것을 알리고 확인을 받은 뒤 `confirmRollback: true` 로 다시 부른다.
    pub const CANCEL_ROLLS_BACK: i32 = 3002;
    /// DB 가 거절했다(커밋 · 롤백 · 연결 실패 등). `data.error` 는 `DbError`, 세션 RPC 면 `data.session` 에 지금 세션 상태.
    pub const DB_ERROR: i32 = 3003;
}

/// 연결 범위를 지난 증거(결정 M3-D18 · M5-D2). [`db_grant`] · [`db_grant_for_agent`] 밖에서는 만들 수 없다(머리 주석의
/// `compile_fail`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbGrant {
    project_id: String,
    /// 에이전트의 입구가 준 것이면 그 판정(연결 · 대상 · 표지). 창의 증거면 `None`.
    agent: Option<Box<AgentBinding>>,
}

impl DbGrant {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// MCP 세션의 증거인가([`db_grant_for_agent`]).
    pub fn is_agent(&self) -> bool {
        self.agent.is_some()
    }

    /// 에이전트의 증거가 묶인 연결 id. 창의 증거면 `None`.
    pub fn agent_connection_id(&self) -> Option<&str> {
        self.agent.as_deref().map(|a| a.connection_id.as_str())
    }

    /// 에이전트에게 적용하는 표지(결정 M5-D37 — 그 대상에서 본 가장 엄한 것, 앱 실행을 넘어 기억한다). 창의 증거면 `None` — 창은 연결 설정의
    /// 표지를 따른다. 실행 파이프라인은 이 read-only 를 연결 설정의 것과 함께 본다(표지를 따로 읽어 판정하지 않는다).
    pub fn agent_marks(&self) -> Option<DbMarks> {
        self.agent.as_deref().map(|a| a.marks)
    }

    /// 에이전트의 증거가 판정한 연결의 드라이버. 창의 증거면 `None`.
    pub fn agent_driver(&self) -> Option<pw_db_types::DriverKind> {
        self.agent.as_deref().map(|a| a.target.driver)
    }
}

/// 그 프로젝트 창의 연결(`ConnectionScope::Project(project_id)`)만 [`DbGrant`] 를 받는다. 판정은 git 쓰기 · 실행 · 파일 쓰기와 같은
/// `scope_may_write` 다.
pub fn db_grant(scope: &ConnectionScope, project_id: &str) -> Result<DbGrant, RpcError> {
    if scope_may_write(scope, project_id) {
        Ok(DbGrant {
            project_id: project_id.to_owned(),
            agent: None,
        })
    } else {
        tracing::info!(target: "pw_db", project = project_id, ?scope, "db access refused: not the project's window");
        Err(not_project_window(project_id))
    }
}

/// `PERMISSION_DENIED`(`data.reason = "notProjectWindow"`).
pub fn not_project_window(project_id: &str) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        "only the project's own window may open or use its db connections",
    )
    .with_data(json!({ "reason": "notProjectWindow", "projectId": project_id }))
}

/// 문장 하나의 분류(창이 받는 꼴).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbStatementClass {
    /// 나눈 문장 가운데 몇 번째인가(`db.sql.split` 의 순서).
    pub index: u32,
    pub range: SqlRange,
    pub grade: RiskGrade,
    pub reasons: Vec<RiskReason>,
}

/// 실행이 트랜잭션에 미치는 효과 — 등급과 따로 확인을 받는다(결정 M3-D21).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbTxEffect {
    /// MySQL · MariaDB 에서 열린 트랜잭션을 실행 전에 커밋하는 문장(DDL · `LOCK TABLES` · `TRUNCATE` 등). 문장이 실패해도 앞의 쓰기는
    /// 커밋된다.
    ImplicitCommit,
}

/// [`code::CONFIRM_REQUIRED`] 의 `data`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConfirmRequired {
    /// 실행할 문장들의 가장 높은 등급.
    pub grade: RiskGrade,
    /// 실행할 문장들(고른 것만).
    pub statements: Vec<DbStatementClass>,
    pub statements_hash: String,
    pub effects: Vec<DbTxEffect>,
    /// 연결 이름을 쳐야 한다(프로덕션 연결의 파괴 등급).
    pub name_required: bool,
    /// 앞서 실은 `confirmed` 를 받지 않은 까닭(`hashMismatch` · `gradeTooLow` · `nameMismatch`). 처음 묻는 것이면 `null`.
    pub rejected: Option<String>,
}

/// 창이 사람의 확인을 받은 증거. `statementsHash` 는 [`code::CONFIRM_REQUIRED`] 가 준 그대로.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConfirmed {
    pub grade: RiskGrade,
    pub statements_hash: String,
    /// 사람이 친 연결 이름(프로덕션 파괴 등급).
    #[serde(default)]
    pub name: Option<String>,
}

/// 실행할 문장 하나.
#[derive(Debug, Clone)]
pub(crate) struct Planned {
    pub index: u32,
    pub range: SqlRange,
    pub text: String,
    pub grade: RiskGrade,
    pub reasons: Vec<RiskReason>,
    /// 트랜잭션 제어 문장이면 옮긴 것.
    pub control: Option<TxControl>,
    pub implicit_commit: bool,
    /// 열린 트랜잭션을 끝낼 수 있다(암묵 커밋 문장 + MySQL `CALL` · `EXECUTE` 등 — `pw_db_sql::txn::may_end_transaction`).
    pub may_end: bool,
    pub settings: bool,
    /// 서버가 고를 수 있는 읽기 가운데 분류기가 읽지 못해 등급에 넣지 않은 것이 있었다
    /// (`pw_db_sql::risk::StatementReadings::unread_reading`). 조회만 받는 호출만 본다.
    pub unread_reading: bool,
}

impl Planned {
    pub fn class(&self) -> DbStatementClass {
        DbStatementClass {
            index: self.index,
            range: self.range,
            grade: self.grade,
            reasons: self.reasons.clone(),
        }
    }

    /// 조회만 받는 자리(read-only 연결 · `db.query.page` · `queryOnly`)가 이 문장을 조회로 보는가. 서버가 고를 수 있는 읽기 가운데
    /// 분류기가 읽지 못한 것이 있으면([`Planned::unread_reading`]) 조회가 아니다 — 그 읽기를 서버만 읽으면 조회 등급의 글이 파일
    /// · 변수를 쓴다(M3-17 리뷰 X3, MySQL 의 읽기 전용 트랜잭션은 `INTO OUTFILE` · `INTO @변수` 를 막지 않는다 — M3-03b 리뷰 M1).
    pub fn is_strict_query(&self) -> bool {
        self.grade == RiskGrade::Read
            && !self.settings
            && self.control.is_none()
            && !self.unread_reading
    }

    /// 조회만 받는 자리가 거절하며 싣는 분류. 읽지 못한 읽기 때문에 거절한 조회는 파싱 실패(파괴 · `unparsed`)로 싣는다 — 조회
    /// 등급 그대로면 「조회인데 거절」 로 읽힌다.
    pub fn strict_class(&self) -> DbStatementClass {
        if self.grade == RiskGrade::Read && self.unread_reading {
            DbStatementClass {
                grade: RiskGrade::Destructive,
                reasons: vec![RiskReason::Unparsed],
                ..self.class()
            }
        } else {
            self.class()
        }
    }
}

/// 분류 스레드의 스택. M3-03 의 입력 상한은 2 MiB 에서도 넘치지 않게 잡혀 있다 — 이것은 그 위의 겹방어다.
const CLASSIFIER_STACK: usize = 16 << 20;
/// 분류 스레드의 이름(시험이 본다).
pub const CLASSIFIER_THREAD: &str = "pw-db-classify";

/// `f` 를 큰 스택의 전용 스레드에서 돌린다(분류 · 나누기). 블로킹 스레드풀(2 MiB)을 쓰지 않는다.
pub async fn on_classifier_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, RpcError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name(CLASSIFIER_THREAD.to_owned())
        .stack_size(CLASSIFIER_STACK)
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| RpcError::internal(format!("cannot start the classifier thread: {e}")))?;
    rx.await
        .map_err(|_| RpcError::internal("the classifier thread stopped"))
}

/// RPC 가 받는 SQL 의 상한(`db.sql.split` 과 같다).
pub use pw_db_sql::split::MAX_SQL_BYTES;

/// `sql` 을 나누고 문장마다 분류한다. `selected` 가 있으면 그 번호의 문장만(순서는 원문 순서, 겹침은 하나로). 문장이 없으면
/// invalid params. 블로킹 — [`on_classifier_thread`] 에서 부른다.
pub(crate) fn plan(
    dialect: SqlDialect,
    sql: &str,
    selected: Option<&[u32]>,
) -> Result<Vec<Planned>, RpcError> {
    if sql.len() > MAX_SQL_BYTES {
        return Err(RpcError::invalid_params(format!(
            "the SQL is {} bytes, over the {MAX_SQL_BYTES} byte limit",
            sql.len()
        )));
    }
    let statements = split(dialect, sql);
    let mut chosen: Vec<usize> = match selected {
        None => (0..statements.len()).collect(),
        Some(indices) => {
            let mut chosen = Vec::with_capacity(indices.len());
            for &i in indices {
                let i = i as usize;
                if i >= statements.len() {
                    return Err(RpcError::invalid_params(format!(
                        "statement {i} is out of range: the SQL has {} statements",
                        statements.len()
                    )));
                }
                chosen.push(i);
            }
            chosen
        }
    };
    chosen.sort_unstable();
    chosen.dedup();
    if chosen.is_empty() {
        return Err(RpcError::invalid_params("there is no statement to run"));
    }
    Ok(chosen
        .into_iter()
        .map(|i| {
            let statement = &statements[i];
            let text = statement.text(sql).to_owned();
            let readings = classify_statement_readings(dialect, &text);
            let class = readings.classification;
            Planned {
                index: i as u32,
                range: statement.range,
                grade: class.grade,
                reasons: class.reasons,
                control: transaction_control(dialect, &text),
                implicit_commit: implicitly_commits(dialect, &text),
                may_end: may_end_transaction(dialect, &text),
                settings: changes_session_settings(dialect, &text),
                unread_reading: readings.unread_reading,
                text,
            }
        })
        .collect())
}

/// 문장들의 가장 높은 등급.
pub(crate) fn grade_of(statements: &[Planned]) -> RiskGrade {
    statements
        .iter()
        .map(|s| s.grade)
        .max()
        .unwrap_or(RiskGrade::Read)
}

/// 확인 해시: 세션 id · 방언 · 문장 원문 · 트랜잭션 효과. 길이를 앞에 붙여 이어 붙여도 경계가 섞이지 않는다.
pub(crate) fn statements_hash(
    session: Option<&str>,
    dialect: SqlDialect,
    statements: &[Planned],
    effects: &[DbTxEffect],
) -> String {
    let mut hasher = Sha256::new();
    let mut part = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    part(b"pitwall.db.confirm.v1");
    part(session.unwrap_or("").as_bytes());
    part(format!("{dialect:?}").as_bytes());
    for statement in statements {
        part(statement.text.as_bytes());
    }
    for effect in effects {
        part(format!("{effect:?}").as_bytes());
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// 연결의 정책 입력.
#[derive(Debug, Clone)]
pub(crate) struct Policy {
    pub read_only: bool,
    pub production: bool,
    pub name: String,
}

/// 정책 판정. 돌려도 되면 `Ok`, 아니면 거절 · 확인 요청 오류. `hash` 는 [`statements_hash`] 로 만든 지금 문장들의 해시.
pub(crate) fn judge(
    policy: &Policy,
    statements: &[Planned],
    effects: &[DbTxEffect],
    hash: &str,
    confirmed: Option<&DbConfirmed>,
) -> Result<(), RpcError> {
    let grade = grade_of(statements);
    if policy.read_only {
        let refused: Vec<DbStatementClass> = statements
            .iter()
            .filter(|s| !s.is_strict_query())
            .map(Planned::strict_class)
            .collect();
        if !refused.is_empty() {
            return Err(RpcError::new(
                rpc_code::PERMISSION_DENIED,
                "this connection is read-only: only queries run",
            )
            .with_data(json!({ "reason": "readOnlyConnection", "statements": refused })));
        }
        return Ok(());
    }
    let name_required = policy.production && grade == RiskGrade::Destructive;
    let needs_confirm = grade >= RiskGrade::UnboundedWrite
        || (policy.production && grade >= RiskGrade::Write)
        || !effects.is_empty();
    if !needs_confirm {
        return Ok(());
    }
    let rejected = match confirmed {
        None => None,
        Some(c) if c.statements_hash != hash => Some("hashMismatch"),
        Some(c) if c.grade < grade => Some("gradeTooLow"),
        Some(c) if name_required && c.name.as_deref() != Some(policy.name.as_str()) => {
            Some("nameMismatch")
        }
        Some(_) => return Ok(()),
    };
    let data = DbConfirmRequired {
        grade,
        statements: statements.iter().map(Planned::class).collect(),
        statements_hash: hash.to_owned(),
        effects: effects.to_vec(),
        name_required,
        rejected: rejected.map(str::to_owned),
    };
    Err(RpcError::new(
        code::CONFIRM_REQUIRED,
        "confirm before running these statements",
    )
    .with_data(serde_json::to_value(data).unwrap_or_default()))
}

/// 세션의 글 읽기를 바꾸는 문장([`RiskReason::ChangesSqlReading`] — PG `standard_conforming_strings` · MySQL `sql_mode` 의
/// `NO_BACKSLASH_ESCAPES` 등)이 있으면 아무것도 돌리기 전에 거절한다(`PERMISSION_DENIED`, `data.reason = "changesSqlReading"`,
/// `data.statements` 는 그 문장들). 확인으로 풀지 않는다 — 그 뒤 같은 세션의 글을 분류기와 서버가 다르게 읽어, 분류가 범위 있는
/// `DELETE` 로 본 글이 범위 없이 돈다(M3-03b 리뷰 M2).
pub(crate) fn refuse_reading_changes(statements: &[Planned]) -> Result<(), RpcError> {
    let refused: Vec<DbStatementClass> = statements
        .iter()
        .filter(|s| s.reasons.contains(&RiskReason::ChangesSqlReading))
        .map(Planned::class)
        .collect();
    if refused.is_empty() {
        return Ok(());
    }
    Err(RpcError::new(
        rpc_code::PERMISSION_DENIED,
        "this statement changes how the server reads SQL text, so Pitwall's classification would no longer hold",
    )
    .with_data(json!({ "reason": "changesSqlReading", "statements": refused })))
}

/// 메서드로 옮길 수 없는 트랜잭션 제어 문장(`PREPARE TRANSACTION` · 옮기지 못하는 옵션)이 있으면 아무것도 돌리기 전에 거절한다.
pub(crate) fn refuse_untranslatable(statements: &[Planned]) -> Result<(), RpcError> {
    for statement in statements {
        if let Some(TxControl::Refused(why)) = statement.control {
            return Err(RpcError::invalid_params(why).with_data(json!({
                "reason": "transactionStatement",
                "index": statement.index,
                "message": why,
            })));
        }
    }
    Ok(())
}

mod agent;

pub use agent::{
    AgentBinding, DbMarks, DbPlanTarget, DbTarget, LOWERS_MARKS, MOVES_MARKED_TARGET,
    NOT_OPENED_BY_USER, PRODUCTION_BLOCKED, db_grant_for_agent, db_grant_to_open,
    not_opened_by_user, production_blocked,
};
pub(crate) use agent::{
    ApprovedMemory, Asker, MarkMemory, OpenedMemory, UNREADABLE_SETTINGS, agent_view,
    check_agent_write, invalid_settings, judge_for_agent, layer_marks_or_strict, production_policy,
};

#[cfg(test)]
mod tests;
