//! 창의 사슬(`before` · 복합 구성 — 17-m6-plan M6-21, 메인 판정 2026-11-19)의 `sql` 단계. 사슬 자체는 `pw_run::runs::chain` 이
//! 돌리고, pw-run 은 pw-db 에 의존하지 않으므로 이 모듈이 [`SqlWindowChain`] 을 구현해 실행 관리자에 붙인다(`super::register`).
//!
//! `run.sql.start` 와 같은 관문(`db_grant` — 그 프로젝트 창) · 읽기 · 분류 · 위험 확인([`SqlRunPlan::judge_window`])이다. 다른 것은
//! **확인을 판정만 하고 쓰지 않는다**는 것 하나다: 사슬은 모든 단계의 확인을 첫 단계를 띄우기 전에 모으므로(`3001` 을 여러 번), 앞
//! 단계의 확인을 쓰고 나서 뒤 단계가 다시 물으면 앞 확인이 사라진다. 사슬이 모든 확인을 지난 뒤 [`SqlWindowStep::consume`] 이 확인
//! 순번을 올린다 — 판정한 그 순번일 때만(그사이 다른 호출이 썼으면 다시 묻는다). 그래서 확인은 한 번만 쓰이고, 다른 단계 · 다른
//! 사슬 · 다시 실은 호출에는 쓰이지 않는다(확인 해시에 구성 열쇠와 순번이 든다 — [`SqlRunPlan::confirm_hash`]).
//!
//! 띄우기는 확인 때 읽은 글이다 — 앞 단계가 돈 뒤면(`recheck`) 파일 · 연결 설정을 다시 읽어 확인 해시가 같은지만 보고, 다르면
//! `planChanged` 로 그 단계 앞에서 멈춘다(다시 읽은 글을 돌리지 않는다).

use std::sync::Arc;

use pw_rpc::RpcError;
use pw_run::runs::chain::{
    BoxFuture, SqlWindowChain, SqlWindowRequest, SqlWindowStep, plan_changed,
};
use pw_run::runs::{LaunchedBy, RunInfo, Runs};

use super::{SqlRunPlan, SqlRunTarget};
use crate::gate::{DbConfirmed, DbGrant, code, db_grant};
use crate::session::DbSessions;

/// [`SqlWindowChain`] 의 pw-db 구현.
pub struct WindowSqlChain {
    sessions: Arc<DbSessions>,
}

impl WindowSqlChain {
    pub fn new(sessions: Arc<DbSessions>) -> Self {
        Self { sessions }
    }
}

impl SqlWindowChain for WindowSqlChain {
    fn prepare(
        &self,
        request: SqlWindowRequest,
    ) -> BoxFuture<Result<Box<dyn SqlWindowStep>, RpcError>> {
        let sessions = Arc::clone(&self.sessions);
        Box::pin(async move {
            let grant = db_grant(&request.scope, &request.project_id)?;
            let target = SqlRunTarget::of(
                &request.project_id,
                request.worktree.as_deref(),
                &request.key,
                &request.config,
            )
            .ok_or_else(|| RpcError::internal("the step is not an sql configuration"))?;
            let confirmed: Option<DbConfirmed> = request
                .confirmed
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| {
                    RpcError::invalid_params(format!(
                        "sqlConfirmed.{}: not a confirmation: {e}",
                        request.key
                    ))
                })?;
            let plan = sessions.plan_sql_run(&grant, &target).await?;
            let epoch_key = format!("{}\u{0}{}", target.project_id, target.config);
            let epoch = crate::session::lock(&sessions.sql_run_confirms)
                .get(&epoch_key)
                .copied()
                .unwrap_or(0);
            let used = plan.judge_window(confirmed.as_ref(), epoch)?;
            Ok(Box::new(WindowStep {
                sessions,
                grant,
                target,
                plan,
                epoch_key,
                epoch,
                used,
            }) as Box<dyn SqlWindowStep>)
        })
    }
}

struct WindowStep {
    sessions: Arc<DbSessions>,
    grant: DbGrant,
    target: SqlRunTarget,
    plan: SqlRunPlan,
    epoch_key: String,
    /// 판정한 확인 순번.
    epoch: u64,
    /// 판정이 확인을 썼다(조회만이면 확인이 없다).
    used: bool,
}

impl SqlWindowStep for WindowStep {
    fn consume(&self) -> Result<(), RpcError> {
        if !self.used {
            return Ok(());
        }
        let mut epochs = crate::session::lock(&self.sessions.sql_run_confirms);
        let now = epochs.get(&self.epoch_key).copied().unwrap_or(0);
        if now != self.epoch {
            return Err(RpcError::new(
                code::CONFIRM_REQUIRED,
                format!(
                    "the confirmation for `{}` was already used; confirm again",
                    self.target.config
                ),
            )
            .with_data(
                serde_json::json!({ "reason": "confirmationUsed", "config": self.target.config }),
            ));
        }
        epochs.insert(self.epoch_key.clone(), now.wrapping_add(1));
        Ok(())
    }

    fn launch(
        self: Box<Self>,
        runs: Runs,
        launched_by: LaunchedBy,
        recheck: bool,
    ) -> BoxFuture<Result<RunInfo, RpcError>> {
        Box::pin(async move {
            let WindowStep {
                sessions,
                grant,
                target,
                plan,
                ..
            } = *self;
            if recheck {
                let now = sessions
                    .plan_sql_run(&grant, &target)
                    .await
                    .map_err(|_| plan_changed(&target.config))?;
                if now.confirm_hash(0) != plan.confirm_hash(0) {
                    return Err(plan_changed(&target.config));
                }
            }
            let started = sessions
                .start_sql_run(&runs, &grant, plan, launched_by, None)
                .await?;
            Ok(started.info)
        })
    }
}
