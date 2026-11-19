//! 실행 구성의 `sql` 종류(17-m6-plan M6-09 · 결정 D55 · 메인 판정 M6-P4 · M6-P16, `docs/design/05-mcp.md` 「`sql` 종류 실행
//! 구성」). 구성(`connection` · `files` · `transaction`)은 `pw_run::config` 의 갈래이고, 이 모듈이 그것을 **프로세스 없이** 돌린다.
//!
//! 1. **실행 경로는 하나**(D55): 읽기 · 분류 · 실행은 `db_query` · `db_execute` 의 파이프라인([`crate::script`])이다 — 파일은
//!    [`crate::script::read_script_file`](루트 안 · 일반 파일 · UTF-8 · 5 MiB)로, 분류는 Host 의 분류기 하나(`crate::gate::plan`),
//!    실행은 [`crate::script::run_files`](파일 여럿 · 트랜잭션 방식 셋). 연결은 `crate::connection::connect` 하나로 연다(신뢰 게이트 ·
//!    비밀 · 확인 · 터널 · 에이전트 증거의 재확인).
//! 2. **실행할 때마다 분류한다**([`DbSessions::plan_sql_run`]): 구성은 그대로여도 파일은 바뀔 수 있다. Host 가 그때 파일들을 한
//!    번 읽고, 읽은 그 글을 분류하고, 그 글을 돌린다 — 파일을 다시 열지 않는다. 등급은 파일 전체에서 가장 높은 것이다(D45).
//! 3. **사람이 화면에서 돌릴 때**(`run.sql.start` — 메인 판정 M6-P16): 그 클릭이 D48 의 「사람이 누름」 이다(연결을 연다 —
//!    「사람이 연 연결」 로 기록된다). 그 프로젝트 창만(`db_grant`), 미신뢰 프로젝트는 구성 · 파일을 읽기 전에 거절한다(연결하지
//!    않는다). read-only 연결의 조회 밖 문장은 거절(`readOnlyConnection`), 조회가 아닌 문장이 하나라도 있으면 SQL 콘솔과 같은 위험
//!    확인(`3001` — `data` 는 [`RunSqlConfirmRequired`], 창은 `db/safety/risk-confirm.tsx` 를 그대로 띄운다 · 프로덕션 연결의 파괴
//!    등급이면 연결 이름 치기). **확인은 한 번만 쓰이고 읽은 내용 · 연결에 묶인다**([`SqlRunPlan::confirm_hash`] — 구성의 확인
//!    순번 · 연결 id · 푼 대상 · 표지 · 방언 · 방식 · 파일마다 경로 · 내용 해시 · 문장 원문): 확인 뒤 파일이나 연결 설정이 바뀌면, 또
//!    같은 확인을 다시 실으면 돌리지 않고 다시 묻는다. 파일은 64개 · 합계 [`MAX_TOTAL_BYTES`](5 MiB)까지.
//! 4. **에이전트의 길**(M6-14 가 채운다): `agent_grant` → `crate::gate::db_grant_for_agent` → [`DbSessions::plan_sql_run`] →
//!    [`SqlRunPlan::refuse_before_asking`](`db_execute` 와 같은 거절) → [`SqlRunPlan::plan_change`] 로 승인 → [`DbSessions::start_sql_run`]
//!    (커밋 경계에 — `none` 이면 첫 문장 앞에 — 그 호출의 `PointOfNoReturn`) → [`SqlRunStarted::done`] 을 기다려 요약을 돌려준다. 감사 상세는
//!    [`SqlRunPlan::audit_detail`] 에 끝([`SqlRunSummary::audit`] — `transaction=`)을 붙인 것이다. SQL 원문 · 값은 남지 않는다.
//! 5. **실행 관리자에 든다**(M6-02 ⑥ — `pw_run::runs::RunKind`): `RunId` · 상태 · 로그가 프로세스 실행과 같고, 콘솔에 문장마다
//!    한 줄([`console`]), 중지는 그 연결의 인터럽트(`QueryCancel`)다. 커밋을 시작한 뒤의 중지는 커밋을 끊지 않는다 — 커밋이 끝난 뒤
//!    다음 파일 앞에서 멈춘다. 다시 실행(`run.restart`)은 없다(`Relaunch::None`) — 창은 `run.sql.start` 를 다시 부른다(확인을 다시
//!    받는다).

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use pw_core::registry::plan::{PlanChange, SqlFile, SqlPlanTransaction, SqlStatement, SqlTarget};
use pw_db_driver::budget::within;
use pw_db_driver::{Budget, DbConnection, QueryCancel};
use pw_db_types::{DbError, DbErrorKind, DbValue, RiskGrade, SqlDialect};
use pw_rpc::{PointOfNoReturn, Router, RpcError, code as rpc_code, rpc_method};
use pw_run::config::{RunConfig, RunConfigPlace, RunKindConfig, read};
use pw_run::runs::{LaunchedBy, NewRun, Relaunch, RunExit, RunInfo, RunKind, RunOutput, Runs};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::gate::{
    DbConfirmRequired, DbConfirmed, DbGrant, DbMarks, DbStatementClass, DbTxEffect, Planned, code,
    db_grant, grade_of, on_classifier_thread, plan, refuse_reading_changes, statements_hash,
};
use crate::script::{
    FilesEvent, FilesRan, LineIndex, PlannedFile, RunControl, SqlTransaction, SqlTransactionEnd,
    StatementEvent, read_script_file, run_files,
};
use crate::session::{DbSessions, blocking, db_error};
use crate::tool_plan::sql_grade;

mod agent;
mod window_chain;
pub use agent::AgentSqlRuns;
pub use window_chain::WindowSqlChain;

/// 한 실행이 읽는 파일 합계의 상한(바이트) — `db_query` · `db_execute` 의 `file` 하나와 같은 5 MiB(결정 D33,
/// `crate::script::MAX_FILE_BYTES`). 파일 수의 상한은 `pw_run::config::SQL_MAX_FILES`(64). 넘으면 띄우기 전에 거절한다.
pub const MAX_TOTAL_BYTES: u64 = crate::script::MAX_FILE_BYTES;

/// 콘솔 크기(줄 바꿈만 쓰므로 뜻이 없다 — 실행 관리자의 칸).
const SIZE: (u16, u16) = (120, 30);

/// 돌릴 `sql` 구성 하나(구성 파일에서 푼 것).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlRunTarget {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기. 연결 설정과 파일을 그 워크트리에서 읽는다.
    pub worktree: Option<String>,
    /// 구성 열쇠(구성 id).
    pub config: String,
    /// 보이는 이름(구성의 `name`).
    pub name: String,
    pub connection: String,
    pub files: Vec<String>,
    pub transaction: SqlTransaction,
}

impl SqlRunTarget {
    /// `sql` 구성이면 그 대상. 다른 종류면 `None`.
    pub fn of(
        project_id: &str,
        worktree: Option<&str>,
        key: &str,
        config: &RunConfig,
    ) -> Option<Self> {
        let RunKindConfig::Sql {
            connection,
            files,
            transaction,
        } = &config.kind
        else {
            return None;
        };
        Some(Self {
            project_id: project_id.to_owned(),
            worktree: worktree.map(str::to_owned),
            config: key.to_owned(),
            name: config.name.clone(),
            connection: connection.clone(),
            files: files.clone(),
            transaction: *transaction,
        })
    }
}

/// 실행하는 그때 읽고 분류한 `sql` 구성(머리 주석 ②). 만드는 길은 [`DbSessions::plan_sql_run`] 하나다.
#[derive(Debug, Clone)]
pub struct SqlRunPlan {
    target: SqlRunTarget,
    dialect: SqlDialect,
    connection_name: String,
    /// 창의 증거면 연결 설정의 병합 표지, 에이전트의 증거면 그 대상에서 본 가장 엄한 것(결정 M5-D37).
    marks: DbMarks,
    /// 창의 증거면 연결 설정의 푼 대상(`DbTarget` — 호스트 · 포트 · DB · 사용자 · 터널 · SQLite 경로). 확인 해시가 덮는다 — 확인
    /// 뒤 연결 설정이 다른 서버를 가리키면 그 확인으로 돌지 않는다. 에이전트의 증거면 `None`(승인 해시가 대상을 덮는다 —
    /// `crate::tool_plan::db_plan`).
    target_key: Option<String>,
    files: Vec<PlannedFile>,
    /// 다시 실행이면 앞 실행(M6-14 의 `run_restart` — 새 실행의 `restartOf`).
    restart_of: Option<String>,
}

impl SqlRunPlan {
    /// 다시 실행한 앞 실행을 단다(새 실행의 `restartOf`).
    #[must_use]
    pub fn with_restart_of(mut self, restart_of: Option<String>) -> Self {
        self.restart_of = restart_of;
        self
    }

    pub fn target(&self) -> &SqlRunTarget {
        &self.target
    }

    pub fn dialect(&self) -> SqlDialect {
        self.dialect
    }

    pub fn marks(&self) -> DbMarks {
        self.marks
    }

    pub fn connection_name(&self) -> &str {
        &self.connection_name
    }

    fn statements(&self) -> impl Iterator<Item = &Planned> {
        self.files.iter().flat_map(|f| f.planned.iter())
    }

    /// 문장 수(파일 전체).
    pub fn statement_count(&self) -> usize {
        self.files.iter().map(|f| f.planned.len()).sum()
    }

    /// 파일 전체에서 가장 높은 등급(D45).
    pub fn grade(&self) -> RiskGrade {
        self.statements()
            .map(|s| s.grade)
            .max()
            .unwrap_or(RiskGrade::Read)
    }

    /// 조회만인가(`Planned::is_strict_query` — `db.query.page` · `db_query` 와 같은 판정). 조회만이면 읽기 전용 트랜잭션으로 돈다.
    pub fn query_only(&self) -> bool {
        self.statements().all(Planned::is_strict_query)
    }

    /// 쓰기로 돌릴 때 트랜잭션으로 되돌릴 수 없는 문장(암묵 커밋 — MySQL · MariaDB 의 DDL 등)이 있다.
    pub fn irreversible(&self) -> bool {
        !self.read_only_run() && self.statements().any(|s| s.implicit_commit || s.may_end)
    }

    /// 실패해도 앞 파일(`per_file`) · 앞 문장(`none`)이 남는다.
    pub fn keeps_on_failure(&self) -> bool {
        !self.read_only_run() && self.target.transaction != SqlTransaction::All
    }

    /// 읽기 전용 트랜잭션으로 도는가 — 조회만이거나 read-only 연결(그 밖의 문장은 판정이 거절한다).
    fn read_only_run(&self) -> bool {
        self.query_only() || self.marks.read_only
    }

    /// 읽은 파일들(루트 기준 경로 · 내용 해시).
    pub fn files(&self) -> Vec<SqlFile> {
        self.files
            .iter()
            .map(|f| SqlFile {
                path: f.file.path.clone(),
                sha256: f.file.content_hash.clone(),
            })
            .collect()
    }

    /// 트랜잭션 효과(결정 M3-D21): 트랜잭션으로 묶어 돌리는데 암묵 커밋 문장이 있다.
    fn effects(&self) -> Vec<DbTxEffect> {
        if self.target.transaction != SqlTransaction::None
            && !self.read_only_run()
            && self.statements().any(|s| s.implicit_commit)
        {
            vec![DbTxEffect::ImplicitCommit]
        } else {
            Vec::new()
        }
    }

    /// 확인 해시(머리 주석 ③): 확인 순번(`epoch` — 한 번 쓰면 오른다) · 연결 id · 연결의 푼 대상 · 표지(프로덕션 · 읽기 전용) ·
    /// 방언 · 트랜잭션 방식 · 파일마다 경로와 읽은 바이트의 해시 · 문장 원문 · 효과. 길이를 앞에 붙여 경계가 섞이지 않는다. 같은 순번 ·
    /// 같은 연결 설정 · 같은 파일 내용이면 같다 — 파일 하나의 한 글자, 연결의 호스트, 표지 하나만 바뀌어도 다르다.
    pub fn confirm_hash(&self, epoch: u64) -> String {
        let mut hasher = Sha256::new();
        let mut part = |bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        part(b"pitwall.run.sql.confirm.v3");
        part(&epoch.to_le_bytes());
        // 구성 열쇠 — 내용이 같은 다른 구성에 그 확인이 쓰이지 않게(M6-21 — 사슬의 단계마다 확인).
        part(self.target.config.as_bytes());
        part(self.target.connection.as_bytes());
        part(self.target_key.as_deref().unwrap_or("").as_bytes());
        part(&[
            u8::from(self.marks.production),
            u8::from(self.marks.read_only),
        ]);
        part(format!("{:?}", self.dialect).as_bytes());
        part(self.target.transaction.name().as_bytes());
        for file in &self.files {
            part(file.file.path.as_bytes());
            part(file.file.content_hash.as_bytes());
            for statement in &file.planned {
                part(statement.text.as_bytes());
            }
        }
        for effect in self.effects() {
            part(format!("{effect:?}").as_bytes());
        }
        hex(&hasher.finalize())
    }

    /// 감사 상세(`db_execute` 의 `crate::script::audit_detail` 과 같은 꼴 — SQL 원문 · 값이 없다): `statements=<수>
    /// grade=<가장 높은 등급> sql=sha256:<문장 해시> mode=<방식>` 뒤에 파일마다 `file=<경로> content=sha256:<내용 해시>`.
    pub fn audit_detail(&self) -> String {
        let all: Vec<Planned> = self.statements().cloned().collect();
        let mut detail = format!(
            "statements={} grade={} sql=sha256:{} mode={}",
            all.len(),
            grade_word(grade_of(&all)),
            statements_hash(None, self.dialect, &all, &[]),
            self.target.transaction.name(),
        );
        for file in &self.files {
            let _ = write!(
                detail,
                " file={} content=sha256:{}",
                file.file.path, file.file.content_hash
            );
        }
        detail
    }

    /// 승인 계획의 SQL 갈래(17-m6-plan M6-09 ⑥): 파일들 · 문장마다 파일과 줄 · 트랜잭션 방식 · 「실패해도 앞 파일 · 앞 문장이
    /// 남는다」 · 「되돌릴 수 없다」. 연결의 표지는 이 계획의 표지다.
    pub fn plan_change(&self) -> PlanChange {
        let mut statements = Vec::with_capacity(self.statement_count());
        for (at, file) in self.files.iter().enumerate() {
            let lines = LineIndex::new(&file.file.text);
            for s in &file.planned {
                statements.push(SqlStatement {
                    index: s.index,
                    line: lines.line(s.range),
                    grade: sql_grade(s.grade),
                    text: s.text.clone(),
                    file: Some(at as u32),
                });
            }
        }
        PlanChange::Sql {
            connection: SqlTarget {
                name: self.connection_name.clone(),
                production: self.marks.production,
                read_only: self.marks.read_only,
            },
            files: self.files(),
            statements,
            transaction: Some(match self.target.transaction {
                SqlTransaction::All => SqlPlanTransaction::All,
                SqlTransaction::PerFile => SqlPlanTransaction::PerFile,
                SqlTransaction::None => SqlPlanTransaction::None,
            }),
            keeps_on_failure: self.keeps_on_failure(),
            irreversible: self.irreversible(),
        }
    }

    /// 에이전트의 길이 묻기 전에 거절하는 것(`db_execute` 와 같다 — `crate::execute_tool` 머리 주석 ③): 분류기가 읽지 못한 문장,
    /// 세션의 글 읽기를 바꾸는 문장, 트랜잭션 제어 문장(구성의 `transaction` 이 트랜잭션을 정한다), read-only 연결의 조회 밖 문장.
    pub fn refuse_before_asking(&self) -> Result<(), RpcError> {
        let audit = self.audit_detail();
        for file in &self.files {
            crate::execute_tool::refuse_before_asking(&file.file.text, &file.planned, &audit)?;
        }
        if self.marks.read_only {
            self.refuse_on_read_only()
                .map_err(|e| crate::tools::with_audit(e, &audit))?;
        }
        Ok(())
    }

    /// read-only 연결의 조회 밖 문장(`crate::gate::judge` 의 read-only 갈래와 같은 거절 — `readOnlyConnection`).
    fn refuse_on_read_only(&self) -> Result<(), RpcError> {
        let refused: Vec<DbStatementClass> = self
            .global()
            .into_iter()
            .filter(|s| !s.is_strict_query())
            .map(|s| s.strict_class())
            .collect();
        if refused.is_empty() {
            return Ok(());
        }
        Err(RpcError::new(
            rpc_code::PERMISSION_DENIED,
            "this connection is read-only: only queries run",
        )
        .with_data(serde_json::json!({ "reason": "readOnlyConnection", "statements": refused })))
    }

    /// 창의 판정(머리 주석 ③): 돌려도 되면 `Ok`. read-only 연결의 조회 밖 문장은 거절, 조회가 아닌 문장이 하나라도 있으면 확인을
    /// 청한다(`3001`) — `confirmed` 가 지금 읽은 내용의 해시와 같고 등급이 그 이상이며, 프로덕션 연결의 파괴 등급이면 친 이름이
    /// 연결 이름과 같을 때만 지난다. `epoch` 는 그 구성의 확인 순번이다. 확인을 써서 지났으면 `Ok(true)` — 부른 쪽이 순번을 올려
    /// 그 확인을 버린다(한 번만 쓰인다).
    pub fn judge_window(
        &self,
        confirmed: Option<&DbConfirmed>,
        epoch: u64,
    ) -> Result<bool, RpcError> {
        refuse_reading_changes(&self.global())?;
        self.refuse_transaction_control()?;
        if self.marks.read_only {
            return self.refuse_on_read_only().map(|()| false);
        }
        let grade = self.grade();
        let effects = self.effects();
        if self.query_only() && effects.is_empty() {
            return Ok(false);
        }
        let hash = self.confirm_hash(epoch);
        let name_required = self.marks.production && grade == RiskGrade::Destructive;
        let rejected = match confirmed {
            None => None,
            Some(c) if c.statements_hash != hash => Some("hashMismatch"),
            Some(c) if c.grade < grade => Some("gradeTooLow"),
            Some(c)
                if name_required && c.name.as_deref() != Some(self.connection_name.as_str()) =>
            {
                Some("nameMismatch")
            }
            Some(_) => return Ok(true),
        };
        let (sql, statements) = self.joined();
        let data = RunSqlConfirmRequired {
            request: DbConfirmRequired {
                grade,
                statements,
                statements_hash: hash,
                effects,
                name_required,
                rejected: rejected.map(str::to_owned),
            },
            connection: RunSqlConnection {
                id: self.target.connection.clone(),
                name: self.connection_name.clone(),
                production: self.marks.production,
                read_only: self.marks.read_only,
            },
            files: self
                .files
                .iter()
                .map(|f| RunSqlFile {
                    path: f.file.path.clone(),
                    content_hash: f.file.content_hash.clone(),
                    statements: f.planned.len() as u32,
                })
                .collect(),
            transaction: self.target.transaction,
            sql,
        };
        Err(RpcError::new(
            code::CONFIRM_REQUIRED,
            "confirm before running these statements",
        )
        .with_data(serde_json::to_value(data).unwrap_or_default()))
    }

    /// 트랜잭션 제어 문장은 받지 않는다 — 구성의 `transaction` 이 트랜잭션을 정한다(파일 안의 `COMMIT` 이 `all` 을 몰래 나누지
    /// 않게).
    fn refuse_transaction_control(&self) -> Result<(), RpcError> {
        for file in &self.files {
            let lines = LineIndex::new(&file.file.text);
            if let Some(s) = file.planned.iter().find(|s| {
                s.control.is_some()
                    || s.reasons
                        .contains(&pw_db_sql::risk::RiskReason::TransactionControl)
            }) {
                return Err(RpcError::new(
                    rpc_code::PERMISSION_DENIED,
                    format!(
                        "{}:{} controls the transaction; an sql configuration's `transaction` decides the transactions — remove \
                         BEGIN / COMMIT / ROLLBACK / SAVEPOINT / SET TRANSACTION",
                        file.file.path,
                        lines.line(s.range)
                    ),
                )
                .with_data(serde_json::json!({
                    "reason": crate::execute_tool::TRANSACTION_STATEMENT,
                    "file": file.file.path,
                    "statements": [s.class()],
                })));
            }
        }
        Ok(())
    }

    /// 파일 전체의 문장을 한 줄로 세운 것(번호는 0 부터 이어서) — 확인 다이얼로그 · read-only 거절이 보는 꼴.
    fn global(&self) -> Vec<Planned> {
        let mut out = Vec::with_capacity(self.statement_count());
        for s in self.statements() {
            let mut s = s.clone();
            s.index = out.len() as u32;
            out.push(s);
        }
        out
    }

    /// 확인 다이얼로그의 글: 파일들을 줄바꿈 하나로 이은 글과, 그 글 기준(UTF-16)으로 옮긴 문장 범위 · 이어 센 번호.
    fn joined(&self) -> (String, Vec<DbStatementClass>) {
        let mut sql = String::new();
        let mut classes = Vec::with_capacity(self.statement_count());
        for file in &self.files {
            let base = sql.encode_utf16().count() as u32;
            for s in &file.planned {
                let mut class = s.class();
                class.index = classes.len() as u32;
                class.range.from += base;
                class.range.to += base;
                classes.push(class);
            }
            sql.push_str(&file.file.text);
            sql.push('\n');
        }
        (sql, classes)
    }
}

/// [`code::CONFIRM_REQUIRED`] 의 `data` — 실행 구성 `sql` 종류의 확인(머리 주석 ③). `request` 를 SQL 콘솔의 위험 확인
/// (`risk-confirm.tsx`)에 그대로 넘기고 `sql` 을 그 글로 준다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunSqlConfirmRequired {
    /// 문장 번호는 파일을 이어 0 부터 센 것, 범위는 `sql` 기준이다. `statementsHash` 를 `confirmed` 로 그대로 돌려준다.
    pub request: DbConfirmRequired,
    pub connection: RunSqlConnection,
    pub files: Vec<RunSqlFile>,
    pub transaction: SqlTransaction,
    /// 파일들을 줄바꿈 하나로 이은 글(보이기만 한다 — 실행은 Host 가 다시 읽은 글로 한다).
    pub sql: String,
}

/// 확인하는 연결.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunSqlConnection {
    pub id: String,
    pub name: String,
    pub production: bool,
    pub read_only: bool,
}

/// 확인하는 파일 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunSqlFile {
    /// 프로젝트 루트 기준 경로.
    pub path: String,
    /// 읽은 바이트의 sha256(hex).
    pub content_hash: String,
    /// 이 파일의 문장 수.
    pub statements: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunSqlStartParams {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기(`ActiveWorktree.top`). 구성 · 연결 설정 · 파일을 그 워크트리에서 읽는다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// `sql` 구성 id(`.pitwall/run/<id>.toml`).
    pub config_id: String,
    /// 앞 호출의 `3001` 을 사람이 확인한 증거(`risk-confirm.tsx` 가 돌려준 그대로).
    #[serde(default)]
    pub confirmed: Option<DbConfirmed>,
}

rpc_method! {
    /// `sql` 실행 구성 하나를 돌린다 — 프로세스를 띄우지 않고 그 DB 연결에서(실행 관리자의 실행 하나: 출력은 `run.attach`, 끝은
    /// `run.exited`, 중지는 `run.stop`). 파일들은 지금 읽어 분류한다. 그 프로젝트의 창이 아닌 연결 · 미신뢰 프로젝트(연결하지
    /// 않는다)는 permission denied, `sql` 이 아닌 구성 · 쓸 수 없는 구성은 invalid params, read-only 연결의 조회 밖 문장 ·
    /// 트랜잭션 제어 문장은 permission denied. 조회가 아닌 문장이 있으면 3001(`data` 는 `RunSqlConfirmRequired`) — 사람이 확인한 뒤
    /// `confirmed` 를 실어 다시 부른다(파일이 그사이 바뀌었으면 다시 3001). 비밀 · 확인이 필요한 연결은 3101 · 3102.
    pub RunSqlStart: "run.sql.start" {
        params: RunSqlStartParams,
        result: RunInfo,
        tier: Exec,
        roles: [Owner],
    }
}

/// 실행 하나의 끝(에이전트의 길이 기다려 돌려준다 — M6-14 ⑨).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SqlRunSummary {
    /// Statements that ran (including the one that failed).
    pub statements: u32,
    /// The transactions in order — which files each covered and how it ended.
    pub transactions: Vec<SqlRunTransaction>,
    /// The statement that failed, if one did. Nothing after it ran.
    pub failed: Option<SqlRunFailure>,
    /// The run was stopped before it finished.
    pub stopped: bool,
    /// Files that did not run (after a failure or a stop).
    pub not_run: Vec<String>,
    /// Audit details: the plan's (`SqlRunPlan::audit_detail`) plus `transaction=` — the last transaction's end
    /// (`committed` · `rolledBack` · `unknown` · `readOnly` · `autoCommitted`). No SQL text or values.
    pub audit: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SqlRunTransaction {
    pub files: Vec<String>,
    pub end: SqlTransactionEnd,
    /// `<file>:<line>` of the last statement that committed on its own (MySQL · MariaDB DDL) in a transaction that did not
    /// commit: the statements up to it are kept even though the transaction was rolled back.
    pub implicit_commit_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SqlRunFailure {
    pub file: String,
    /// Statement index within the file (from 0).
    pub index: u32,
    pub line: u32,
    pub error: DbError,
}

/// [`DbSessions::start_sql_run`] 의 답.
pub struct SqlRunStarted {
    pub info: RunInfo,
    /// 실행이 끝나면(콘솔에 끝을 쓴 뒤) 요약.
    pub done: tokio::sync::oneshot::Receiver<SqlRunSummary>,
}

impl DbSessions {
    /// 구성의 파일들을 지금 읽고 분류한다(머리 주석 ②). 신뢰 게이트를 먼저 지난다 — 미신뢰 프로젝트면 파일도 연결 설정도 읽지
    /// 않는다. 연결은 열지 않는다. 창의 증거면 연결 설정(그 워크트리)의 드라이버 · 이름 · 표지를, 에이전트의 증거면 관문이 판정한
    /// 드라이버 · 이름 · 표지를 쓴다(그 증거의 연결이 구성의 연결이어야 한다).
    pub async fn plan_sql_run(
        self: &Arc<Self>,
        grant: &DbGrant,
        target: &SqlRunTarget,
    ) -> Result<SqlRunPlan, RpcError> {
        if grant.project_id() != target.project_id {
            return Err(RpcError::internal("the db grant is for another project"));
        }
        let (s, project, worktree) = (
            Arc::clone(self),
            target.project_id.clone(),
            target.worktree.clone(),
        );
        blocking(move || {
            s.connections
                .trust_gate(&project, worktree.as_deref())
                .map(|_| ())
        })
        .await?;
        let (kind, name, marks, target_key) = if grant.is_agent() {
            if grant.agent_connection_id() != Some(target.connection.as_str()) {
                return Err(RpcError::internal(
                    "the agent db grant is for another connection",
                ));
            }
            (
                grant.agent_driver(),
                grant.agent_connection_name().unwrap_or("?").to_owned(),
                grant.agent_marks().unwrap_or_default(),
                None,
            )
        } else {
            let (s, project, worktree, id) = (
                Arc::clone(self),
                target.project_id.clone(),
                target.worktree.clone(),
                target.connection.clone(),
            );
            blocking(move || {
                let loaded = s.connections.load(&project, worktree.as_deref())?;
                let list = loaded.connections.map_err(RpcError::invalid_params)?;
                let found = list.into_iter().find(|c| c.id == id).ok_or_else(|| {
                    RpcError::new(
                        rpc_code::NOT_FOUND,
                        format!("db connection `{id}` does not exist"),
                    )
                })?;
                let config = found
                    .config
                    .map_err(|_| RpcError::invalid_params(crate::gate::invalid_settings(&id)))?;
                let target = crate::gate::DbTarget::of(&config, &loaded.root);
                Ok((
                    Some(config.driver),
                    config.name.clone(),
                    DbMarks::of(&config),
                    Some(format!("{target:?}")),
                ))
            })
            .await?
        };
        let driver = kind
            .and_then(|kind| self.drivers.get(kind.id()))
            .ok_or_else(|| {
                db_error(DbError::new(
                    DbErrorKind::Unsupported,
                    "the db driver of this connection is not installed",
                ))
            })?;
        let dialect = match driver.dialect() {
            Some(dialect) if driver.capabilities().sql => dialect,
            _ => {
                return Err(db_error(DbError::new(
                    DbErrorKind::Unsupported,
                    format!(
                        "an sql configuration runs on SQL databases only; `{}` is a key-value store",
                        target.connection
                    ),
                )));
            }
        };
        let (s, project, worktree, paths) = (
            Arc::clone(self),
            target.project_id.clone(),
            target.worktree.clone(),
            target.files.clone(),
        );
        let read = blocking(move || {
            let projects = &s.connections.host()?.projects;
            // 파일 수(`pw_run::config::SQL_MAX_FILES`)는 구성 검증이, 합계 크기는 여기서 읽는 그때 본다 — 넘으면 띄우기 전에 거절.
            if paths.len() > pw_run::config::SQL_MAX_FILES {
                return Err(RpcError::invalid_params(format!(
                    "an sql configuration runs at most {} files",
                    pw_run::config::SQL_MAX_FILES
                )));
            }
            let mut total = 0u64;
            let mut read = Vec::with_capacity(paths.len());
            for path in &paths {
                let file = read_script_file(projects, &project, worktree.as_deref(), path)?;
                total += file.text.len() as u64;
                if total > MAX_TOTAL_BYTES {
                    return Err(RpcError::invalid_params(format!(
                        "the files of this sql configuration are larger than {MAX_TOTAL_BYTES} bytes together (at `{}`)",
                        file.path
                    ))
                    .with_data(serde_json::json!({ "reason": "tooLarge" })));
                }
                read.push(file);
            }
            Ok(read)
        })
        .await?;
        let files = on_classifier_thread(move || {
            read.into_iter()
                .map(|file| {
                    // 빈 파일(주석뿐)은 문장 없는 파일이다 — 실패가 아니다.
                    let planned = if file.text.trim().is_empty() {
                        Vec::new()
                    } else {
                        match plan(dialect, &file.text, None) {
                            Ok(planned) => planned,
                            Err(e) if e.message == "there is no statement to run" => Vec::new(),
                            Err(e) => {
                                return Err(RpcError::invalid_params(format!(
                                    "{}: {}",
                                    file.path, e.message
                                )));
                            }
                        }
                    };
                    Ok(PlannedFile { file, planned })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .await??;
        Ok(SqlRunPlan {
            target: target.clone(),
            dialect,
            connection_name: name,
            marks,
            target_key,
            files,
            restart_of: None,
        })
    }

    /// 판정(창: [`SqlRunPlan::judge_window`], 에이전트: 승인)을 지난 계획을 실행 관리자의 실행 하나로 돌린다(머리 주석 ⑤). 연결을
    /// 먼저 연다 — 열지 못하면(비밀 · 확인 · 드라이버 오류, 에이전트 증거의 재확인) 실행은 생기지 않고 그 오류다. `no_return` 은
    /// 에이전트 호출의 「되돌릴 수 없는 지점」 이다 — 첫 커밋 앞에서 세운다(서지 못하면 커밋하지 않고 롤백한다). 트랜잭션이
    /// 없는 실행(`none`)은 첫 문장을 보내기 직전에 세운다(서지 못하면 아무 문장도 보내지 않는다).
    pub async fn start_sql_run(
        self: &Arc<Self>,
        runs: &Runs,
        grant: &DbGrant,
        plan: SqlRunPlan,
        launched_by: LaunchedBy,
        no_return: Option<PointOfNoReturn>,
    ) -> Result<SqlRunStarted, RpcError> {
        let no_return = no_return.map(|point| Arc::new(move || point.pass()) as NoReturn);
        self.start_sql_run_with(runs, grant, plan, launched_by, no_return)
            .await
    }

    /// [`Self::start_sql_run`] 인데 「되돌릴 수 없는 지점」 을 함수로 받는다(시험이 그 자리를 본다).
    #[doc(hidden)]
    pub async fn start_sql_run_with(
        self: &Arc<Self>,
        runs: &Runs,
        grant: &DbGrant,
        plan: SqlRunPlan,
        launched_by: LaunchedBy,
        no_return: Option<NoReturn>,
    ) -> Result<SqlRunStarted, RpcError> {
        let target = plan.target.clone();
        let read_only = plan.read_only_run();
        if plan.marks.read_only && !plan.query_only() {
            return Err(plan.refuse_on_read_only().unwrap_err());
        }
        let opened = crate::connection::connect(
            self,
            grant,
            target.worktree.as_deref(),
            &target.connection,
            None,
        )
        .await?;
        let tunnel = opened.tunnel;
        let conn = match opened.connection {
            DbConnection::Sql(conn) => conn,
            other => {
                let _ = other.close().await;
                return Err(RpcError::invalid_params("this connection does not run SQL"));
            }
        };
        let caps = &opened.driver.capabilities;
        let unsupported = if read_only {
            (!caps.read_only_transaction).then_some("this driver has no read-only transactions")
        } else if target.transaction != SqlTransaction::None {
            (!caps.transactions).then_some("this driver has no transactions")
        } else {
            None
        };
        if let Some(why) = unsupported {
            let _ = conn.close().await;
            return Err(db_error(DbError::new(
                DbErrorKind::Unsupported,
                format!("{why}, so this sql configuration cannot run on it"),
            )));
        }
        let cancel = QueryCancel::new(conn.interrupter(), Budget::PAGE.cancel_cleanup);
        let control = Arc::new(SqlRunControl {
            phase: Mutex::new(Phase::Running),
            cancel: cancel.clone(),
            no_return,
            hooks: Mutex::new(Some(self.test_hooks.take_commit_hooks())),
        });
        let (done_tx, done) = tokio::sync::oneshot::channel();
        let new = NewRun {
            project_id: target.project_id.clone(),
            config: target.config.clone(),
            name: target.name.clone(),
            launched_by,
            worktree: target.worktree.clone(),
            cwd: None,
            profile: None,
            relaunch: Relaunch::None,
            size: SIZE,
            restart_of: plan.restart_of.clone(),
        };
        let sessions = Arc::clone(self);
        let kind_control = Arc::clone(&control);
        let mut conn = Some(conn);
        let info = runs.start_kind(new, move |output| {
            let conn = conn
                .take()
                .ok_or_else(|| RpcError::internal("started twice"))?;
            tokio::spawn(async move {
                let _tunnel = tunnel;
                let summary = drive(
                    &sessions, conn, &plan, read_only, &cancel, &control, &output,
                )
                .await;
                let code = i32::from(summary.failed.is_some() || summary.stopped);
                let _ = done_tx.send(summary);
                output.exited(RunExit {
                    code: Some(code),
                    signal: None,
                });
            });
            Ok(Arc::new(SqlRun {
                control: kind_control,
            }) as Arc<dyn RunKind>)
        })?;
        Ok(SqlRunStarted { info, done })
    }
}

/// 실행 본체: 콘솔에 머리 · 문장마다 한 줄 · 트랜잭션의 끝을 쓰고, DDL 이상이 시작됐으면 스키마 캐시를 버리고, 연결을 닫는다.
#[allow(clippy::too_many_arguments)]
async fn drive(
    sessions: &Arc<DbSessions>,
    mut conn: Box<dyn pw_db_driver::SqlConnection>,
    plan: &SqlRunPlan,
    read_only: bool,
    cancel: &QueryCancel,
    control: &SqlRunControl,
    output: &RunOutput,
) -> SqlRunSummary {
    output.write(console::header(plan, read_only).as_bytes());
    let mut ordinal = 0u32;
    let paths: Vec<String> = plan.files.iter().map(|f| f.file.path.clone()).collect();
    let mut ddl_started = false;
    let ran = {
        let mut observe = |event: FilesEvent<'_>| {
            let line = match event {
                FilesEvent::Statement { file, event } => {
                    ordinal += 1;
                    let path = &paths[file];
                    match event {
                        StatementEvent::Ran { result, rows_total } => {
                            if result.grade >= RiskGrade::Ddl {
                                ddl_started = true;
                            }
                            console::statement(ordinal, path, result, rows_total)
                        }
                        StatementEvent::RanUnlisted => String::new(),
                        StatementEvent::Failed(error) => {
                            ddl_started |= plan.files[file]
                                .planned
                                .iter()
                                .any(|s| s.index == error.index && s.grade >= RiskGrade::Ddl);
                            console::failure(ordinal, path, error)
                        }
                    }
                }
                FilesEvent::Ended(transaction) => console::ended(
                    &paths[transaction.files.clone()],
                    transaction.end,
                    transaction.commit_error.as_ref(),
                    transaction
                        .implicit_commit_at
                        .map(|(at, line)| format!("{}:{line}", paths[at]))
                        .as_deref(),
                ),
            };
            output.write(line.as_bytes());
        };
        run_files(
            conn.as_mut(),
            &plan.files,
            plan.target.transaction,
            read_only,
            cancel,
            control,
            &mut observe,
        )
        .await
    };
    let summary = summarize(plan, &ran);
    output.write(console::footer(&summary).as_bytes());
    if ddl_started {
        sessions.schema_changed(&plan.target.project_id, &plan.target.connection);
    }
    let _ = within(Budget::PAGE.connect, "close", conn.close()).await;
    summary
}

fn summarize(plan: &SqlRunPlan, ran: &FilesRan) -> SqlRunSummary {
    let path = |at: usize| plan.files[at].file.path.clone();
    let reached = ran
        .transactions
        .last()
        .map_or(0, |t| t.files.end)
        .max(ran.failed.as_ref().map_or(0, |(at, _)| at + 1));
    let not_run = if ran.failed.is_some() || ran.stopped {
        (reached..plan.files.len()).map(path).collect()
    } else {
        Vec::new()
    };
    let end = ran.last_end();
    SqlRunSummary {
        statements: ran.statements,
        transactions: ran
            .transactions
            .iter()
            .map(|t| SqlRunTransaction {
                files: t.files.clone().map(path).collect(),
                end: t.end,
                implicit_commit_at: t
                    .implicit_commit_at
                    .map(|(at, line)| format!("{}:{line}", path(at))),
            })
            .collect(),
        failed: ran.failed.as_ref().map(|(at, e)| SqlRunFailure {
            file: path(*at),
            index: e.index,
            line: e.line,
            error: e.error.clone(),
        }),
        stopped: ran.stopped,
        not_run,
        audit: format!(
            "{} transaction={}",
            plan.audit_detail(),
            end.map_or("none", SqlTransactionEnd::word)
        ),
    }
}

/// 실행의 다스림(머리 주석 ⑤ — `crate::script::RunControl`).
struct SqlRunControl {
    phase: Mutex<Phase>,
    cancel: QueryCancel,
    no_return: Option<NoReturn>,
    /// 시험이 커밋 앞뒤에 끼워 넣는 동작(`DbSessions::test_hooks` — 앱에서는 비어 있다).
    hooks: Mutex<Option<crate::session::CommitHooks>>,
}

/// 「되돌릴 수 없는 지점」 을 세우는 것 — 에이전트 호출의 `PointOfNoReturn::pass`(시험은 직접 준 함수).
pub type NoReturn = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    /// 커밋 중 — 중지는 커밋을 끊지 않는다.
    Committing,
    Stopped,
    /// 커밋 중에 중지가 왔다 — 커밋이 끝나면 멈춘다.
    StoppedWhileCommitting,
}

impl SqlRunControl {
    fn stop(&self) {
        let mut phase = crate::session::lock(&self.phase);
        match *phase {
            Phase::Running => {
                *phase = Phase::Stopped;
                drop(phase);
                let cancel = self.cancel.clone();
                match tokio::runtime::Handle::try_current() {
                    Ok(runtime) => {
                        runtime.spawn(async move {
                            let _ = cancel.cancel().await;
                        });
                    }
                    Err(_) => {
                        tracing::warn!(target: "pw_db", "no runtime to interrupt a stopped sql run")
                    }
                }
            }
            Phase::Committing => *phase = Phase::StoppedWhileCommitting,
            Phase::Stopped | Phase::StoppedWhileCommitting => {}
        }
    }
}

impl RunControl for SqlRunControl {
    fn stopped(&self) -> bool {
        matches!(
            *crate::session::lock(&self.phase),
            Phase::Stopped | Phase::StoppedWhileCommitting
        )
    }

    fn begin_commit(&self) -> bool {
        let mut phase = crate::session::lock(&self.phase);
        if *phase != Phase::Running {
            return false;
        }
        // 에이전트 호출이면 RPC 의 표지를 먼저 세운다 — 못 세웠으면 취소가 먼저 왔다(커밋하지 않는다).
        if let Some(point) = &self.no_return
            && !point()
        {
            *phase = Phase::Stopped;
            return false;
        }
        *phase = Phase::Committing;
        true
    }

    fn begin_irreversible(&self) -> bool {
        let mut phase = crate::session::lock(&self.phase);
        if *phase != Phase::Running {
            return false;
        }
        if let Some(point) = &self.no_return
            && !point()
        {
            *phase = Phase::Stopped;
            return false;
        }
        true
    }

    fn before_commit(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let hook = crate::session::lock(&self.hooks)
            .as_mut()
            .and_then(|h| h.before_commit.take());
        Box::pin(async move {
            if let Some(hook) = hook {
                hook().await;
            }
        })
    }

    fn after_commit_point(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let hook = crate::session::lock(&self.hooks)
            .as_mut()
            .and_then(|h| h.after_commit_point.take());
        Box::pin(async move {
            if let Some(hook) = hook {
                hook().await;
            }
        })
    }

    fn end_commit(&self) {
        let mut phase = crate::session::lock(&self.phase);
        if *phase == Phase::Committing {
            *phase = Phase::Running;
        }
    }
}

/// 실행 관리자에 드는 `sql` 종류(M6-02 ⑥).
struct SqlRun {
    control: Arc<SqlRunControl>,
}

impl RunKind for SqlRun {
    fn kind(&self) -> &'static str {
        "sql"
    }

    fn stop(&self) {
        self.control.stop();
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn grade_word(grade: RiskGrade) -> String {
    serde_json::to_value(grade)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{grade:?}"))
}

/// 실행 콘솔 · `run_logs` 의 글(05-mcp.md 「`sql` 종류 실행 구성」 — 메인 판정 M6-P4). 에이전트도 읽으므로 영어다(결정 M5-D39).
/// 줄 끝은 `\r\n`(콘솔은 터미널이다). 결과 행 전체는 남기지 않는다 — 조회는 앞 [`crate::script::RUN_ROWS_SHOWN`] 행만.
pub mod console {
    use super::*;
    use crate::script::{DbScriptError, DbScriptResult};

    const NL: &str = "\r\n";

    pub(crate) fn header(plan: &SqlRunPlan, read_only: bool) -> String {
        let files = plan.files.len();
        format!(
            "sql: {} file{} on `{}`{}, transaction {}{}{NL}",
            files,
            if files == 1 { "" } else { "s" },
            plan.connection_name,
            if plan.marks.production {
                " (production)"
            } else {
                ""
            },
            plan.target.transaction.name(),
            if read_only {
                " (queries only — read-only transaction)"
            } else {
                ""
            },
        )
    }

    /// `#<번호> <파일>:<줄> <등급> <영향 행 수 또는 조회 행 수> <소요 ms>`, 조회면 그 아래 앞 행들의 표.
    pub(crate) fn statement(
        ordinal: u32,
        path: &str,
        result: &DbScriptResult,
        rows_total: Option<u64>,
    ) -> String {
        let count = if !result.columns.is_empty() {
            let rows = rows_total.unwrap_or(result.rows.len() as u64);
            format!("{rows} row{}", if rows == 1 { "" } else { "s" })
        } else {
            match result.rows_affected {
                Some(n) => format!("{n} row{} affected", if n == 1 { "" } else { "s" }),
                None => "done".to_owned(),
            }
        };
        let mut out = format!(
            "#{ordinal} {path}:{} {} {count} {} ms{NL}",
            result.line,
            grade_word(result.grade),
            result.elapsed_ms
        );
        if !result.columns.is_empty() {
            out.push_str(&table(result, rows_total));
        }
        out
    }

    /// 오류는 DB 원문과 문장 번호 · 줄.
    pub(crate) fn failure(ordinal: u32, path: &str, error: &DbScriptError) -> String {
        let code = error
            .error
            .code
            .as_deref()
            .map(|c| format!(" [{c}]"))
            .unwrap_or_default();
        format!(
            "#{ordinal} {path}:{} error{code}: {}{NL}",
            error.line,
            one_line(&error.error.message)
        )
    }

    /// 트랜잭션의 끝(`db_execute` 의 끝과 같은 말 — committed · rolled back · commit outcome unknown).
    pub(crate) fn ended(
        files: &[String],
        end: SqlTransactionEnd,
        commit_error: Option<&DbError>,
        implicit_commit_at: Option<&str>,
    ) -> String {
        let list = files.join(", ");
        let why = commit_error
            .map(|e| format!(" — the commit failed: {}", one_line(&e.message)))
            .unwrap_or_default();
        match end {
            SqlTransactionEnd::Committed => format!("committed: {list}{NL}"),
            // 암묵 커밋 문장(MySQL · MariaDB 의 DDL)이 돌았으면 그 앞은 이미 커밋됐다 — 「아무것도 남지 않는다」 고 쓰지 않는다
            // (`db_execute` 의 `irreversible` 과 같은 판정).
            SqlTransactionEnd::RolledBack if implicit_commit_at.is_some() => format!(
                "rolled back after an implicit commit: {list} — the statement at {} committed the statements before it on \
                 its own, so they are kept; the rest was rolled back{why}{NL}",
                implicit_commit_at.unwrap_or_default()
            ),
            SqlTransactionEnd::RolledBack => {
                format!(
                    "rolled back: {list} — nothing of {} is kept{why}{NL}",
                    if files.len() == 1 {
                        "this file"
                    } else {
                        "these files"
                    }
                )
            }
            SqlTransactionEnd::Unknown => format!(
                "commit outcome unknown: {list}{why} — the changes may or may not be in the database{NL}"
            ),
            SqlTransactionEnd::ReadOnly => format!("read-only, rolled back: {list}{NL}"),
            SqlTransactionEnd::AutoCommitted => {
                format!("no transaction: every statement of {list} that ran is committed{NL}")
            }
        }
    }

    pub(crate) fn footer(summary: &SqlRunSummary) -> String {
        let mut out = String::new();
        if !summary.not_run.is_empty() {
            let _ = write!(out, "not run: {}{NL}", summary.not_run.join(", "));
        }
        let verdict = if summary.stopped {
            "stopped"
        } else if summary.failed.is_some() {
            "failed"
        } else {
            "done"
        };
        let _ = write!(
            out,
            "{verdict}: {} statement{}{NL}",
            summary.statements,
            if summary.statements == 1 { "" } else { "s" }
        );
        out
    }

    fn table(result: &DbScriptResult, rows_total: Option<u64>) -> String {
        let header: Vec<String> = result.columns.iter().map(|c| one_line(&c.name)).collect();
        let rows: Vec<Vec<String>> = result
            .rows
            .iter()
            .map(|row| row.iter().map(cell).collect())
            .collect();
        let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
        for row in &rows {
            for (i, c) in row.iter().enumerate() {
                if let Some(w) = widths.get_mut(i) {
                    *w = (*w).max(c.chars().count());
                }
            }
        }
        let line = |cells: &[String]| {
            let mut s = String::from("  |");
            for (i, w) in widths.iter().enumerate() {
                let c = cells.get(i).map(String::as_str).unwrap_or("");
                let pad = w.saturating_sub(c.chars().count());
                let _ = write!(s, " {c}{} |", " ".repeat(pad));
            }
            s.push_str(NL);
            s
        };
        let mut out = line(&header);
        for row in &rows {
            out.push_str(&line(row));
        }
        let total = rows_total.unwrap_or(rows.len() as u64);
        let more = total.saturating_sub(rows.len() as u64);
        if more > 0 {
            let _ = write!(
                out,
                "  … {more} more row{}{NL}",
                if more == 1 { "" } else { "s" }
            );
        }
        out
    }

    /// 셀 하나(셀 상한 `RUN_CELL_BYTES` 에서 잘린 값은 `…`).
    fn cell(value: &DbValue) -> String {
        let text = match value {
            DbValue::Null => "NULL".to_owned(),
            DbValue::Bool(b) => b.to_string(),
            DbValue::Truncated(t) => format!("{}…", t.prefix),
            other => match serde_json::to_value(other) {
                Ok(serde_json::Value::Object(map)) => match map.into_iter().next() {
                    Some((_, serde_json::Value::String(s))) => s,
                    Some((_, v)) => v.to_string(),
                    None => String::new(),
                },
                Ok(v) => v.to_string(),
                Err(_) => "?".to_owned(),
            },
        };
        one_line(&text)
    }

    /// 줄바꿈 · 제어 문자를 보이는 꼴로(한 문장이 한 줄 — 콘솔의 터미널 제어 문자를 옮기지 않는다).
    pub(crate) fn one_line(text: &str) -> String {
        text.chars()
            .map(|c| match c {
                '\n' | '\r' | '\t' => ' ',
                c if c.is_control() => '\u{fffd}',
                c => c,
            })
            .collect()
    }
}

/// 창의 `run.sql.start`(머리 주석 ③).
async fn start_from_window(
    sessions: &Arc<DbSessions>,
    runs: &Runs,
    grant: DbGrant,
    window: Option<String>,
    params: RunSqlStartParams,
) -> Result<RunInfo, RpcError> {
    let (s, project, worktree, id) = (
        Arc::clone(sessions),
        params.project_id.clone(),
        params.worktree.clone(),
        params.config_id.clone(),
    );
    // 미신뢰 프로젝트는 구성을 읽기 전에 거절한다(연결하지 않는다 — M6-P6).
    let target = blocking(move || {
        let host = s.connections.host()?;
        let project = host
            .projects
            .get(&project)
            .map_err(pw_core::registry::methods::rpc_error)?;
        if !host.projects.is_trusted(&project.id) {
            return Err(pw_run::runs::untrusted(&project.id));
        }
        let place = RunConfigPlace::new(&host.paths, &project, worktree.as_deref())?;
        let set = read(&place);
        let Some(loaded) = set.get(&id) else {
            if let Some(unusable) = set.unusable.iter().find(|u| u.id == id) {
                return Err(RpcError::invalid_params(format!(
                    "run config `{}` cannot be used: {}",
                    unusable.id, unusable.reason
                )));
            }
            return Err(RpcError::new(
                rpc_code::NOT_FOUND,
                format!("run config `{id}` does not exist"),
            ));
        };
        // `before` 가 있는 구성은 사슬이다 — 이 길로 돌리면 `before` 를 건너뛴다(M6-21 — `run.start` 가 사슬을 띄운다).
        if !loaded.config.before.is_empty() {
            return Err(RpcError::invalid_params(format!(
                "run config `{id}` runs other configurations first (`before`); start it with run.start"
            )));
        }
        SqlRunTarget::of(&project.id, worktree.as_deref(), &loaded.id, &loaded.config).ok_or_else(
            || {
                RpcError::invalid_params(format!(
                    "run config `{id}` is not an sql configuration; start it with run.start"
                ))
            },
        )
    })
    .await?;
    let plan = sessions.plan_sql_run(&grant, &target).await?;
    // 확인은 한 번만 쓰인다: 순번을 읽어 판정하고, 확인을 써서 지났으면 같은 잠금 안에서 올린다 — 같은 확인을 함께 실은 두
    // 호출 가운데 하나만 지난다(뒤 호출은 `hashMismatch` 로 다시 묻는다).
    {
        let key = format!("{}\u{0}{}", target.project_id, target.config);
        let mut epochs = crate::session::lock(&sessions.sql_run_confirms);
        let epoch = epochs.get(&key).copied().unwrap_or(0);
        if plan.judge_window(params.confirmed.as_ref(), epoch)? {
            epochs.insert(key, epoch.wrapping_add(1));
        }
    }
    let started = sessions
        .start_sql_run(runs, &grant, plan, LaunchedBy::Window { window }, None)
        .await?;
    Ok(started.info)
}

/// `run.sql.start` 를 붙인다(데스크톱 Host 가 실행 관리자와 함께 부른다).
pub fn register(router: &mut Router, sessions: Arc<DbSessions>, runs: Runs) {
    // 에이전트의 `run_start` · `run_restart` 가 `sql` 구성을 이 길로 돌린다(M6-14 — pw-run 은 pw-db 에 의존하지 않는다).
    runs.attach_sql_tool(Arc::new(AgentSqlRuns::new(Arc::clone(&sessions))));
    // 창의 사슬(`before` · 복합 구성)의 `sql` 단계(M6-21).
    runs.attach_sql_window(Arc::new(WindowSqlChain::new(Arc::clone(&sessions))));
    router.unary(RunSqlStart, move |ctx, params: RunSqlStartParams| {
        let (sessions, runs) = (Arc::clone(&sessions), runs.clone());
        let granted = db_grant(ctx.scope(), &params.project_id);
        let window = ctx.window().map(str::to_owned);
        async move { start_from_window(&sessions, &runs, granted?, window, params).await }
    });
}
