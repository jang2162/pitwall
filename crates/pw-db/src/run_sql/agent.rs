//! 에이전트의 `run_start` · `run_restart` 가 `sql` 구성을 돌리는 길(17-m6-plan M6-14 ① ④ ⑨ — 머리 주석 ④). 툴 메서드 자체는
//! `pw_run::runs::exec_tool` 에 있고(실행 툴 셋이 한 자리), pw-run 은 pw-db 에 의존하지 않으므로 이 모듈이
//! [`pw_run::runs::exec_tool::SqlRunTool`] 을 구현해 실행 관리자에 붙인다(`super::register`).
//!
//! - **관문**: `agent_grant` → [`db_grant_for_agent`](D60 — 사람이 그 프로젝트 창에서 이번 앱 실행 중에 연 연결만, 프로덕션은
//!   `[mcp.policy] production` 이 `block`(기본)이면 거절 · `ask` 면 매번 묻는다). 연 적 없는 연결은 「Pitwall 에서 먼저 열어라」 거절이다
//!   (`notOpenedByUser` — 묻는 것으로 바꾸지 않는다).
//! - **분류**: [`DbSessions::plan_sql_run`] 이 파일들을 지금 읽어 분류하고, [`SqlRunPlan::refuse_before_asking`] 이 `db_execute` 와
//!   같은 것(못 읽은 문장 · 글 읽기를 바꾸는 문장 · 트랜잭션 제어 · read-only 연결의 쓰기)을 묻기 전에 거절한다.
//! - **계획 등급**: 조회만이면 `read` + 정책 자동(프로덕션 `ask` 대상이면 매번 묻는다), 그 밖은 `dangerous` + 매번
//!   ([`EVERY_TIME`]). 해시는 `db_execute` 와 같이 연결의 푼 대상 · 방언 · 표지를 덮고(`DbGrant::plan_target`), 파일마다 경로와 내용
//!   해시 · 문장 원문을 덮는다(`PlanChange::Sql` — 파일만 바꿔도 계획이 다르다).
//! - **실행**: 승인을 증거에 달고 [`DbSessions::start_sql_run`] — 연결 직전의 재확인 · 커밋 경계의 「되돌릴 수 없는 지점」 은 그대로다.

use std::sync::Arc;

use pw_core::registry::agent::agent_grant;
use pw_core::registry::plan::{Approved, PlanRules, Remember};
use pw_rpc::{PointOfNoReturn, RpcError, Tier};
use pw_run::runs::exec_tool::{
    SendFuture, SqlRunTool, SqlToolLaunch, SqlToolPlan, SqlToolRequest, SqlToolStarted,
};
use pw_run::runs::{LaunchedBy, Runs};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{SqlRunPlan, SqlRunTarget};
use crate::gate::{DbGrant, db_grant_for_agent};
use crate::session::{DbSessions, blocking};
use crate::tool_plan::EVERY_TIME;

/// [`SqlRunTool`] 의 pw-db 구현.
pub struct AgentSqlRuns {
    sessions: Arc<DbSessions>,
}

impl AgentSqlRuns {
    pub fn new(sessions: Arc<DbSessions>) -> Self {
        Self { sessions }
    }
}

impl SqlRunTool for AgentSqlRuns {
    fn plan(&self, request: SqlToolRequest) -> SendFuture<Result<SqlToolPlan, RpcError>> {
        let sessions = Arc::clone(&self.sessions);
        Box::pin(async move { plan(sessions, request).await })
    }
}

async fn plan(sessions: Arc<DbSessions>, request: SqlToolRequest) -> Result<SqlToolPlan, RpcError> {
    let target = SqlRunTarget::of(
        &request.project_id,
        request.worktree.as_deref(),
        &request.key,
        &request.config,
    )
    .ok_or_else(|| RpcError::internal("the configuration is not an sql configuration"))?;
    let grant = {
        let (s, scope, project, worktree, connection, session) = (
            Arc::clone(&sessions),
            request.scope.clone(),
            request.project_id.clone(),
            request.worktree.clone(),
            target.connection.clone(),
            request.session,
        );
        blocking(move || {
            let agent = agent_grant(&scope, &s.connections.host()?.projects, &project)?;
            db_grant_for_agent(&s, &agent, session, worktree.as_deref(), &connection, None)
        })
        .await?
    };
    let plan = sessions
        .plan_sql_run(&grant, &target)
        .await?
        .with_restart_of(request.restart_of.clone());
    plan.refuse_before_asking().map_err(tool_words)?;
    let audit = plan.audit_detail();
    let query_only = plan.query_only();
    let tier = if query_only {
        Tier::Read
    } else {
        Tier::Dangerous
    };
    let content = format!(
        "sha256:{}",
        Sha256::digest(audit.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    // 조회만이면 정책 자동(05 「`sql` 종류 실행 구성」). 프로덕션 `ask` 대상은 조회도 매번 묻는다.
    let rules = if query_only && !grant.needs_approval() {
        PlanRules {
            auto_approve: true,
            remember: Remember::Key {
                key: format!("{}@{content}", request.key),
            },
            policy_auto: true,
        }
    } else {
        EVERY_TIME
    };
    let marks = plan.marks();
    let count = plan.statement_count();
    let summary = format!(
        "Run the sql configuration `{}`: {count} statement{} on the {}DB connection `{}`{}",
        request.key,
        if count == 1 { "" } else { "s" },
        if marks.production { "production " } else { "" },
        plan.connection_name(),
        if query_only {
            " — queries only, in a read-only transaction"
        } else if plan.irreversible() {
            " — some cannot be rolled back"
        } else {
            ""
        },
    );
    let targets = vec![
        (
            "db".to_owned(),
            serde_json::to_value(grant.plan_target()).unwrap_or(Value::Null),
        ),
        (
            "dialect".to_owned(),
            serde_json::to_value(Some(plan.dialect())).unwrap_or(Value::Null),
        ),
        ("content".to_owned(), Value::String(content)),
    ];
    Ok(SqlToolPlan {
        tier,
        summary,
        change: plan.plan_change(),
        targets,
        rules,
        audit,
        launch: Box::new(Launch {
            sessions,
            grant,
            plan,
        }),
    })
}

/// `db_execute` 의 거절 글을 이 툴의 말로(트랜잭션은 구성의 `transaction` 이 정한다).
fn tool_words(mut error: RpcError) -> RpcError {
    error.message = error
        .message
        .replace(
            "db_execute runs the whole call in one transaction",
            "the configuration's `transaction` setting decides the transactions",
        )
        .replace("db_execute runs only", "run_start runs only");
    error
}

struct Launch {
    sessions: Arc<DbSessions>,
    grant: DbGrant,
    plan: SqlRunPlan,
}

impl SqlToolLaunch for Launch {
    fn launch(
        self: Box<Self>,
        approved: Approved,
        runs: Runs,
        launched_by: LaunchedBy,
        no_return: PointOfNoReturn,
    ) -> SendFuture<Result<SqlToolStarted, RpcError>> {
        let Launch {
            sessions,
            mut grant,
            plan,
        } = *self;
        Box::pin(async move {
            grant.approve(&approved);
            let started = sessions
                .start_sql_run(&runs, &grant, plan, launched_by, Some(no_return))
                .await?;
            let (tx, done) = tokio::sync::oneshot::channel();
            let summary = started.done;
            tokio::spawn(async move {
                if let Ok(summary) = summary.await {
                    let audit = summary.audit.clone();
                    let _ = tx.send((serde_json::to_value(&summary).unwrap_or(Value::Null), audit));
                }
            });
            Ok(SqlToolStarted {
                info: started.info,
                done,
            })
        })
    }
}
