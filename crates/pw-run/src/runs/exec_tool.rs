//! 실행 툴 ③ 실행 — `run_start` · `run_stop` · `run_restart`(17-m6-plan M6-14, `docs/design/05-mcp.md` 「실행 구성」 표). 그룹
//! `run`, 앱 Host 만(헤드리스 Host 는 `pw-run` 의 메서드를 등록하지 않는다).
//!
//! - **관문**: `agent_grant`(MCP 세션의 연결 범위 · 레지스트리에 있는 프로젝트). 창 · Host 범위 연결은 거절한다 — 사람은 `run.start`
//!   · `run.sql.start` 를 쓴다. **미신뢰 프로젝트는 묻지 않고 거절한다**(M6-02 와 같은 `untrusted` — 계획을 만들기 전, 구성도
//!   `.env` 도 읽지 않는다).
//! - **선언 등급과 계획 등급**(17-m6-plan 결정 M6-P9): `run_start` · `run_restart` 는 선언 `dangerous`(닿을 수 있는 가장 높은 등급 —
//!   `sql` 구성의 쓰기), `run_stop` 은 `exec`. 계획 등급은 프로세스 구성이 `exec`, `sql` 구성은 실행하는 그때의 분류로 조회만이면
//!   `read`, 그 밖은 `dangerous`(매번 묻는다 — `pw_db::tool_plan::EVERY_TIME`). 낮춘 등급은 `pw_mcp::catalog::LOWERED_PLAN_TIERS` 에
//!   있다.
//! - **계획 = 띄울 것 그대로**(M6-14 ②): 프로세스 구성은 지금 구성 · `.env` 를 읽어 푼 [`RunSpec`](argv · 작업 폴더 · env 값 ·
//!   프로필)을 계획에 싣고(`PlanBuilder::runs` — 해시에 든다), 사람에게는 그 내용을 [`PlanChange::Text`] 로 보인다 — env 는 이름과
//!   값과 어디서 왔는지(구성 · 프로필 · 그 `.env` 파일), 비밀 단어 이름의 값은 `••••` 와 해시 앞 8자, 값 안 URL 의 비밀번호도
//!   가린다(`config::shown` — `GIT_CONFIG_KEY_<n>` 은 가리지 않는다). argv 는 가리지 않는다(실행될 글이다) — 비밀처럼 보이면
//!   「Warning」 을 덧붙인다. 대상 값에 구성의 내용 해시(「사람이 본 구성」 해시 — M6-11)를 넣는다: 승인 뒤 구성 · `.env` ·
//!   스크립트 본문 · 런타임 원천 파일이 바뀌면 다시 만든 계획의 해시가 달라 돌지 않는다(`plan_changed`). 승인 뒤에는 계획의
//!   `RunSpec` 만 띄운다(`agent_run_grant` → `Runs::start_agent_with`) — 구성을 다시 읽지 않는다.
//! - **정책 자동**(M6-10 ③ · M6-11): 프로세스 구성은 신뢰한 프로젝트이고 내용 해시가 「사람이 본 구성」(`Registry::human_saw`)이면
//!   `policyAuto`. 사람이 [허용] 하면 그 해시를 기록할 수 있게 계획에 (구성 열쇠, 해시)를 싣는다(`PlanBuilder::seen`) — 기록은
//!   `mcp.approval.answer` 가 한다(이 핸들러는 에이전트 범위라 기록하지 않는다). `sql` 구성은 조회만이면 `policyAuto`(프로덕션 `ask`
//!   대상은 매번 묻는다).
//! - **기억의 단위**(M6-14 ⑧): 세션 허용 · CLI 허용의 열쇠는 `<구성 열쇠>@<내용 해시>` — 그 구성의 그 내용에만 닿는다.
//! - **사람이 띄운 실행**(결정 M6-D7 · D69 — `Run::launched_by_person`): `run_stop` · `run_restart` 는 그 프로젝트의 어느 실행이든
//!   다루되, 사람이 띄운 실행에는 정책 자동도 `auto_approve` 도 없이 묻는다. 열쇠는 `human:<runId>` — 세션 허용은 그 실행 하나에
//!   닿는다. 에이전트가 띄운 실행의 `run_stop` 열쇠는 `<구성 열쇠>` 다. 다른 프로젝트의 `runId` 는 없는 실행이다.
//! - **`sql` 구성**은 pw-db 가 붙인 [`SqlRunTool`] 로 돈다(pw-run 은 pw-db 에 의존하지 않는다) — `db_execute` 와 같은 관문 · 분류 ·
//!   거절 · 호출 수명(끝날 때까지 기다려 요약과 `runId` 를 함께 돌려준다, MCP 취소는 그 실행의 중지 = `QueryCancel`, 커밋 경계의
//!   「되돌릴 수 없는 지점」).
//! - **감사**: 결과의 `audit` 은 구성 열쇠 · 내용 해시 · `runId` 만 든다 — 명령줄 원문 · env 값은 남기지 않는다(`pw_mcp::audit`
//!   의 `CONTENT_WORDS` 가 입력도 거른다).

use std::future::Future;
use std::pin::Pin;

use pw_core::registry::agent::{AgentGrant, agent_grant};
use pw_core::registry::exec::{RunSpec, agent_run_grant};
use pw_core::registry::plan::{Approved, PlanChange, PlanRules, Planned, Remember, ToolPlan};
use pw_rpc::{
    ConnectionScope, Ctx, PointOfNoReturn, Router, RpcError, SessionId, Tier, rpc_method,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::tool::{RunToolRun, tool_run};
use super::{
    AgentLaunch, AgentStart, LaunchedBy, Resolved, RunId, RunInfo, Runs, config_of, untrusted,
};
use crate::config::{RunConfig, RunKindConfig, shown};
use crate::env::EnvOrigin;

/// `run.tool.start` 의 RPC 이름.
pub const START_METHOD: &str = "run.tool.start";
/// `run.tool.stop` 의 RPC 이름.
pub const STOP_METHOD: &str = "run.tool.stop";
/// `run.tool.restart` 의 RPC 이름.
pub const RESTART_METHOD: &str = "run.tool.restart";

/// 사람이 띄운 실행을 겨누는 허용 열쇠의 머리(`human:<runId>`).
pub const HUMAN_KEY_PREFIX: &str = "human:";

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunToolStartParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). The configuration is read from that worktree and runs in the project root
    /// inside it. Empty means the session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Run configuration id (see `run_config_list`). Detected tasks run after saving them with `run_config_upsert`.
    pub config_id: String,
    /// A profile of that configuration (`profiles` — its `env` and `args` override the configuration's).
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunToolRunParams {
    pub project_id: String,
    /// The run (`runId` from `run_start` or `run_status`). A run of another project is `not found`.
    pub run_id: RunId,
}

/// What `run_start` and `run_restart` started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunToolStarted {
    /// The new run. A process run is returned as soon as it started — read its output with `run_logs`.
    pub run: RunToolRun,
    /// For an `sql` configuration, which waits until the run ends: statements run, the transactions and how each ended
    /// (`committed` · `rolledBack` · `unknown` · `readOnly` · `autoCommitted`), the failed statement if one failed, whether
    /// it was stopped, and the files that did not run. The console output is also in `run_logs`.
    #[specta(type = Option<pw_rpc::JsonValue>)]
    pub sql: Option<Value>,
    /// Audit details: the configuration id, its content hash and the run id (no command line or values).
    pub audit: String,
    /// For a configuration with `before` or a `compound` configuration: every run the chain started, in the order it started
    /// them (`run` is the last). Empty otherwise.
    #[serde(default)]
    pub chain: Vec<RunToolRun>,
}

/// What `run_stop` did.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolStopped {
    /// The run after the stop was sent (`stopping`, or `exited` if it had already ended). It ends with SIGTERM, then
    /// SIGKILL after 5 seconds; an `sql` run is interrupted and rolled back.
    pub run: RunToolRun,
}

rpc_method! {
    /// Start a run configuration (see `run_config_list`) in Pitwall. A process configuration is started and returned at
    /// once with its `runId` (read the output with `run_logs`, stop it with `run_stop`); an `sql` configuration runs its
    /// files on its DB connection and this waits until it ends. The person approves what runs in Pitwall first — the
    /// command line, working directory, profile and environment values — unless they already saw and ran this exact
    /// configuration content in a trusted project, or the `sql` configuration only queries. A configuration changed since
    /// (its file, its `.env` files, the script it names, the runtime files) is asked again. Untrusted projects do not run.
    /// An `sql` configuration's connection must have been opened by the person in Pitwall, and every statement that is not
    /// a query is approved each time. A configuration with `before`, or a `compound` one, starts a chain: every step is in
    /// the plan, each step before the last waits until the one before it exited with code 0 (a failure stops the chain and
    /// nothing after it starts; a step whose content changed while an earlier step ran stops with `planChanged`), and this
    /// returns once every step started (`chain` lists the runs). It runs without asking only when the person saw every
    /// configuration of the chain in this content.
    pub RunToolStartMethod: "run.tool.start" {
        params: Planned<RunToolStartParams>,
        result: RunToolStarted,
        tier: Dangerous,
        roles: [Owner],
        mcp: { group: "run", name: "run_start" },
    }

    /// Stop a run of this project (also one a person started — then the person is asked). The process tree gets SIGTERM,
    /// then SIGKILL after 5 seconds; an `sql` run is interrupted and its open transaction rolled back. Returns at once.
    pub RunToolStopMethod: "run.tool.stop" {
        params: Planned<RunToolRunParams>,
        result: RunToolStopped,
        tier: Exec,
        roles: [Owner],
        mcp: { group: "run", name: "run_stop" },
    }

    /// Stop a run of this project, wait until it ended, and start its configuration again as a new run (`restartOf` points
    /// to the old one). The configuration is read again, so the person approves what runs now, the same way as
    /// `run_start`. A run a person started is always asked about.
    pub RunToolRestartMethod: "run.tool.restart" {
        params: Planned<RunToolRunParams>,
        result: RunToolStarted,
        tier: Dangerous,
        roles: [Owner],
        mcp: { group: "run", name: "run_restart" },
    }
}

// --- sql 구성(pw-db 가 붙인다) ----------------------------------------------------------------------------------------

/// 보낼 수 있는 future.
pub type SendFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// `sql` 구성을 에이전트의 `run_start` · `run_restart` 로 돌리는 것([`Runs::attach_sql_tool`] — pw-db 의 `pw_db::run_sql` 이
/// 붙인다). 이 모듈은 계획의 등급 · 규칙(사람이 띄운 실행이면 묻기) · 승인 · 다시 실행의 앞 실행 끝내기를 하고, 관문 · 파일 읽기 ·
/// 분류 · 묻기 전 거절 · 실행은 pw-db 가 `db_execute` 와 같은 함수로 한다.
pub trait SqlRunTool: Send + Sync + 'static {
    /// 관문(사람이 연 연결 · 프로덕션 정책) → 파일을 지금 읽어 분류 → 묻기 전 거절까지. 부작용이 없다(연결을 열지 않는다).
    fn plan(&self, request: SqlToolRequest) -> SendFuture<Result<SqlToolPlan, RpcError>>;
}

/// [`SqlRunTool::plan`] 이 받는 것.
#[derive(Debug, Clone)]
pub struct SqlToolRequest {
    /// 부른 연결의 범위(에이전트) · 세션.
    pub scope: ConnectionScope,
    pub session: SessionId,
    pub project_id: String,
    pub worktree: Option<String>,
    /// 구성 열쇠(구성 id).
    pub key: String,
    pub config: RunConfig,
    /// 다시 실행이면 앞 실행.
    pub restart_of: Option<RunId>,
}

/// [`SqlRunTool::plan`] 의 답: 계획에 실을 것과, 승인 뒤 띄우는 것.
pub struct SqlToolPlan {
    /// `read`(조회만) 또는 `dangerous`.
    pub tier: Tier,
    pub summary: String,
    pub change: PlanChange,
    /// 해시에 넣는 대상 값(연결의 푼 대상 · 방언 · 표지).
    pub targets: Vec<(String, Value)>,
    /// pw-db 가 정한 규칙(조회만이면 정책 자동, 그 밖 · 프로덕션 `ask` 는 매번).
    pub rules: PlanRules,
    /// 감사 상세(파일마다 경로와 내용 해시 — SQL 원문 · 값이 없다).
    pub audit: String,
    pub launch: Box<dyn SqlToolLaunch>,
}

/// 승인 뒤 `sql` 실행을 띄운다.
pub trait SqlToolLaunch: Send {
    /// 연결을 열고 실행 관리자의 실행 하나로 띄운다. `no_return` 은 그 호출의 「되돌릴 수 없는 지점」 이다.
    fn launch(
        self: Box<Self>,
        approved: Approved,
        runs: Runs,
        launched_by: LaunchedBy,
        no_return: PointOfNoReturn,
    ) -> SendFuture<Result<SqlToolStarted, RpcError>>;
}

/// 띄운 `sql` 실행.
pub struct SqlToolStarted {
    pub info: RunInfo,
    /// 끝나면 (요약, 감사 상세).
    pub done: tokio::sync::oneshot::Receiver<(Value, String)>,
}

// --- 계획 ----------------------------------------------------------------------------------------------------------

/// 띄울 것 하나의 계획(승인 뒤 띄우는 쪽이 쓴다).
pub(super) enum Prepared {
    /// 사슬(`before` · 복합 구성 — M6-21, `super::chain_tool`).
    Chain(Box<super::chain_tool::ChainPlanned>),
    Process {
        spec: RunSpec,
        content: String,
    },
    Sql {
        launch: Box<dyn SqlToolLaunch>,
        audit: String,
    },
}

/// 다시 실행의 앞 실행(사람이 띄웠는지 · 그 id).
pub(super) struct Before {
    pub(super) run_id: RunId,
    pub(super) by_person: bool,
    /// 앞 실행의 구성 열쇠(사슬을 다시 띄우면 이 열쇠의 단계가 `restartOf` 를 받는다).
    pub(super) config: String,
}

impl Runs {
    fn exec_grant_for_agent(
        &self,
        scope: &ConnectionScope,
        project_id: &str,
    ) -> Result<AgentGrant, RpcError> {
        let host = self.inner.host()?;
        agent_grant(scope, &host.projects, project_id)
    }

    fn trusted(&self, project_id: &str) -> bool {
        self.inner
            .host()
            .is_ok_and(|h| h.projects.is_trusted(project_id))
    }

    /// 계획을 만들고 승인을 지난다(첫 호출은 여기서 「승인 필요」 로 돌아간다). `before` 는 다시 실행의 앞 실행.
    async fn plan_and_approve<P: Serialize + Sync>(
        &self,
        ctx: &Ctx,
        method: &str,
        planned: &Planned<P>,
        agent: &AgentGrant,
        target: StartTarget<'_>,
        before: Option<&Before>,
    ) -> Result<(Approved, Prepared), RpcError> {
        let scope = ctx.scope().clone();
        let host_projects = self.inner.host()?.projects.clone();
        let (project_id, worktree, config_id, profile) = (
            agent.project_id().to_owned(),
            target.worktree.map(str::to_owned),
            target.config_id.to_owned(),
            target.profile.map(str::to_owned),
        );
        // 미신뢰 프로젝트는 묻지 않고 거절한다 — 구성 · `.env` 를 읽기 전(M6-14 ⑦, M6-P6).
        if !host_projects.is_trusted(&project_id) {
            return Err(untrusted(&project_id));
        }
        let (place_project, place) = {
            let (runs, project_id, worktree) = (self.clone(), project_id.clone(), worktree.clone());
            blocking(move || runs.config_place(&project_id, worktree.as_deref())).await?
        };
        let (key, config) = config_of(&place, &config_id)?;
        let human = before.is_some_and(|b| b.by_person);
        if crate::config::graph::is_chain(&config) {
            return self
                .plan_chain(ctx, method, planned, agent, target, before, &place, &key)
                .await;
        }
        if matches!(config.kind, RunKindConfig::Sql { .. }) {
            let Some(sql) = self.inner.sql_tool.get().cloned() else {
                return Err(RpcError::invalid_params(
                    "sql configurations cannot run in this Pitwall host",
                ));
            };
            let planned_sql = sql
                .plan(SqlToolRequest {
                    scope,
                    session: ctx.session(),
                    project_id: place_project.id.clone(),
                    worktree: worktree.clone(),
                    key: key.clone(),
                    config: config.clone(),
                    restart_of: before.map(|b| b.run_id.clone()),
                })
                .await?;
            let mut rules = planned_sql.rules.clone();
            if human {
                // 사람이 띄운 실행: 정책 자동 · `auto_approve` 없이 묻고, 세션 허용은 그 실행 하나에(매번 묻는 계획은 그대로 매번).
                rules.policy_auto = false;
                rules.auto_approve = false;
                if rules.remember != Remember::Never {
                    let content = planned_sql
                        .targets
                        .iter()
                        .find(|(name, _)| name == "content")
                        .and_then(|(_, v)| v.as_str())
                        .unwrap_or_default();
                    rules.remember = Remember::Key {
                        key: human_key(before, content),
                    };
                }
            }
            let mut builder = ToolPlan::builder(method, planned_sql.tier, &planned.params)
                .summary(planned_sql.summary.clone())
                .change(planned_sql.change.clone())
                .rules(rules);
            for (name, value) in &planned_sql.targets {
                builder = builder.target(name, value);
            }
            if let Some(before) = before {
                builder = builder.target("restartOf", &before.run_id);
            }
            let approved = planned
                .approve(builder.build())
                .map_err(|e| with_audit(e, &planned_sql.audit))?;
            return Ok((
                approved,
                Prepared::Sql {
                    launch: planned_sql.launch,
                    audit: planned_sql.audit,
                },
            ));
        }
        let (resolved, config) = {
            let (runs, agent_project, worktree, profile, key) = (
                self.clone(),
                agent.project().clone(),
                worktree.clone(),
                profile.clone(),
                key.clone(),
            );
            blocking(move || {
                let agent = AgentGrantRef(&agent_project);
                runs.plan_process_for(&agent, worktree.as_deref(), &key, profile.as_deref())
            })
            .await?
        };
        let seen = resolved.seen_hash();
        let content = match &seen {
            Ok(hash) => hash.as_str().to_owned(),
            Err(why) => {
                tracing::debug!(target: "pw_run", key, %why, "no seen hash for the run plan; it is asked");
                fallback_content(&resolved)
            }
        };
        let saw = seen
            .as_ref()
            .is_ok_and(|hash| host_projects.human_saw(&project_id, &key, hash));
        let spec = RunSpec {
            config: key.clone(),
            argv: resolved.argv.clone(),
            cwd: resolved.cwd.clone(),
            env: resolved.run_env.vars().clone(),
            profile: profile.clone(),
        };
        let changes = shown_changes(&resolved, &config);
        let make = |policy_auto: bool| {
            let rules = if human {
                PlanRules {
                    auto_approve: false,
                    remember: Remember::Key {
                        key: human_key(before, &content),
                    },
                    policy_auto: false,
                }
            } else {
                PlanRules {
                    auto_approve: true,
                    remember: Remember::Key {
                        key: format!("{key}@{content}"),
                    },
                    policy_auto,
                }
            };
            let mut builder = ToolPlan::builder(method, Tier::Exec, &planned.params)
                .summary(summary(&key, &config, before))
                .target("content", &content)
                .rules(rules)
                .runs([spec.clone()]);
            for change in &changes {
                builder = builder.change(change.clone());
            }
            if let Ok(hash) = &seen {
                builder = builder.seen(key.clone(), hash.as_str());
            }
            if let Some(before) = before {
                builder = builder.target("restartOf", &before.run_id);
            }
            builder.build()
        };
        // 사람이 [허용] 하면 `mcp.approval.answer` 가 그 내용을 「사람이 본 구성」 으로 기록한다 — 승인 표를 실은 둘째 호출이 다시 만든
        // 계획은 그 기록으로 정책 자동이 켜져 해시가 달라진다. 사람이 승인한 것은 정책 자동이 꺼진 그 계획이므로, 지금 본 것이어도
        // 꺼진 계획이 표와 맞으면 지난다(띄울 것 · 내용 해시는 같다). 반대(정책 자동으로 승인된 뒤 기록이 사라짐)는 다시 묻는다.
        let approved = match planned.approve(make(saw)) {
            Ok(approved) => approved,
            Err(e) if saw && planned.ticket.is_some() => {
                planned.approve(make(false)).map_err(|_| e)?
            }
            Err(e) => return Err(e),
        };
        Ok((approved, Prepared::Process { spec, content }))
    }

    /// [`Self::plan_process`] 의 몸(관문의 프로젝트로).
    pub(super) fn plan_process_for(
        &self,
        agent: &AgentGrantRef<'_>,
        worktree: Option<&str>,
        config_id: &str,
        profile: Option<&str>,
    ) -> Result<(Resolved, RunConfig), RpcError> {
        let (project, place) = self.config_place(&agent.0.id, worktree)?;
        let (key, config) = config_of(&place, config_id)?;
        let resolved = self.resolve_process(&project.id, &place.root, &key, &config, profile)?;
        Ok((resolved, config))
    }

    /// 승인을 지난 계획을 띄운다.
    async fn launch_prepared(
        &self,
        ctx: &Ctx,
        agent: &AgentGrant,
        approved: Approved,
        prepared: Prepared,
        worktree: Option<String>,
        before: Option<&Before>,
    ) -> Result<RunToolStarted, RpcError> {
        if let Some(before) = before {
            self.stop_and_wait(agent.project_id(), &before.run_id)
                .await?;
        }
        let launched = AgentLaunch {
            session: format!("{:?}", ctx.session()),
            client: None,
        };
        match prepared {
            Prepared::Chain(chain) => {
                self.launch_chain(ctx, agent, approved, *chain, worktree, before)
                    .await
            }
            Prepared::Process { spec, content } => {
                let grant = agent_run_grant(agent, &approved)?;
                let runs = self.clone();
                let start = AgentStart {
                    size: None,
                    worktree,
                    restart_of: before.map(|b| b.run_id.clone()),
                };
                let info =
                    blocking(move || runs.start_agent_with(&grant, 0, launched, start)).await?;
                let audit = format!(
                    "config={} content={content} runId={}",
                    spec.config, info.run_id
                );
                Ok(RunToolStarted {
                    run: tool_run(info),
                    sql: None,
                    audit,
                    chain: Vec::new(),
                })
            }
            Prepared::Sql { launch, audit } => {
                let started = launch
                    .launch(
                        approved,
                        self.clone(),
                        LaunchedBy::Agent {
                            session: launched.session,
                            client: launched.client,
                        },
                        ctx.point_of_no_return(),
                    )
                    .await
                    .map_err(|e| with_audit(e, &audit))?;
                let run_id = started.info.run_id.clone();
                // 호출이 버려지면(MCP 취소 · 끊김 — 되돌릴 수 없는 지점 앞) 그 실행을 멈춘다(`QueryCancel`).
                let guard = StopOnDrop {
                    runs: self.clone(),
                    project_id: agent.project_id().to_owned(),
                    run_id: Some(run_id.clone()),
                };
                let (summary, audit) = started.done.await.map_err(|_| {
                    with_audit(
                        RpcError::internal("the sql run ended without a summary"),
                        &audit,
                    )
                })?;
                guard.disarm();
                let info = self
                    .find(agent.project_id(), &run_id)
                    .map(|run| run.info())
                    .unwrap_or(started.info);
                Ok(RunToolStarted {
                    run: tool_run(info),
                    sql: Some(summary),
                    audit: format!("{audit} runId={run_id}"),
                    chain: Vec::new(),
                })
            }
        }
    }

    /// `run_start`.
    async fn tool_start(
        &self,
        ctx: &Ctx,
        planned: Planned<RunToolStartParams>,
    ) -> Result<RunToolStarted, RpcError> {
        let p = &planned.params;
        let agent = {
            let (runs, scope, project) = (self.clone(), ctx.scope().clone(), p.project_id.clone());
            blocking(move || runs.exec_grant_for_agent(&scope, &project)).await?
        };
        let (approved, prepared) = self
            .plan_and_approve(
                ctx,
                START_METHOD,
                &planned,
                &agent,
                StartTarget {
                    worktree: p.worktree.as_deref(),
                    config_id: &p.config_id,
                    profile: p.profile.as_deref(),
                },
                None,
            )
            .await?;
        self.launch_prepared(ctx, &agent, approved, prepared, p.worktree.clone(), None)
            .await
    }

    /// `run_restart`.
    async fn tool_restart(
        &self,
        ctx: &Ctx,
        planned: Planned<RunToolRunParams>,
    ) -> Result<RunToolStarted, RpcError> {
        let p = &planned.params;
        let agent = {
            let (runs, scope, project) = (self.clone(), ctx.scope().clone(), p.project_id.clone());
            blocking(move || runs.exec_grant_for_agent(&scope, &project)).await?
        };
        let run = self.find(agent.project_id(), &p.run_id)?;
        let kind = run.info().kind;
        if kind != super::process::KIND && kind != SQL_KIND {
            return Err(RpcError::invalid_params(format!(
                "run `{}` is a `{kind}` run; run_restart restarts process and sql runs",
                p.run_id
            )));
        }
        let before = Before {
            run_id: run.id().to_owned(),
            by_person: run.launched_by_person(),
            config: run.config().to_owned(),
        };
        let (config, worktree, profile) = (
            run.config().to_owned(),
            run.worktree().map(str::to_owned),
            run.profile().map(str::to_owned),
        );
        let (approved, prepared) = self
            .plan_and_approve(
                ctx,
                RESTART_METHOD,
                &planned,
                &agent,
                StartTarget {
                    worktree: worktree.as_deref(),
                    config_id: &config,
                    profile: profile.as_deref(),
                },
                Some(&before),
            )
            .await?;
        self.launch_prepared(ctx, &agent, approved, prepared, worktree, Some(&before))
            .await
    }

    /// `run_stop`(블로킹 아님 — 중지를 보내고 돌아온다).
    async fn tool_stop(
        &self,
        ctx: &Ctx,
        planned: Planned<RunToolRunParams>,
    ) -> Result<RunToolStopped, RpcError> {
        let p = &planned.params;
        let agent = {
            let (runs, scope, project) = (self.clone(), ctx.scope().clone(), p.project_id.clone());
            blocking(move || runs.exec_grant_for_agent(&scope, &project)).await?
        };
        let run = self.find(agent.project_id(), &p.run_id)?;
        let info = run.info();
        let by_person = run.launched_by_person();
        let rules = if by_person {
            PlanRules {
                auto_approve: false,
                remember: Remember::Key {
                    key: format!("{HUMAN_KEY_PREFIX}{}", run.id()),
                },
                policy_auto: false,
            }
        } else {
            // 에이전트가 띄운(승인을 지난) 실행을 끄는 것 — 새로 띄우는 것이 없다. 신뢰한 프로젝트면 묻지 않는다.
            PlanRules {
                auto_approve: true,
                remember: Remember::Key {
                    key: run.config().to_owned(),
                },
                policy_auto: self.trusted(agent.project_id()),
            }
        };
        let who = if by_person { "a person" } else { "an agent" };
        let text = format!(
            "Run: {}\nConfiguration: {}\nKind: {}\nStarted by: {who}\nState: {:?}{}",
            run.id(),
            run.config(),
            info.kind,
            info.state,
            info.pid
                .map(|p| format!("\nProcess id: {p}"))
                .unwrap_or_default(),
        );
        let plan = ToolPlan::builder(STOP_METHOD, Tier::Exec, &p)
            .summary(format!(
                "Stop the run `{}` of `{}` (started by {who})",
                run.id(),
                run.config()
            ))
            .change(PlanChange::Text {
                label: "Stop".to_owned(),
                text,
            })
            .target("byPerson", &by_person)
            .rules(rules)
            .build();
        planned.approve(plan)?;
        let info = self.stop(agent.project_id(), &p.run_id)?;
        Ok(RunToolStopped {
            run: tool_run(info),
        })
    }
}

/// 실행 관리자의 `sql` 종류 이름(`pw_db::run_sql` 의 `RunKind::kind`).
const SQL_KIND: &str = "sql";

/// 띄울 구성(`run_start` 의 입력, `run_restart` 의 앞 실행).
pub(super) struct StartTarget<'a> {
    pub(super) worktree: Option<&'a str>,
    pub(super) config_id: &'a str,
    pub(super) profile: Option<&'a str>,
}

/// 관문을 지난 프로젝트(블로킹 스레드로 넘기는 꼴).
pub(super) struct AgentGrantRef<'a>(pub(super) &'a pw_core::registry::Project);

/// 사람이 띄운 실행을 다시 띄우는 허용의 열쇠 `human:<runId>:<내용 해시>`(결정 D69 · M6-14 ⑦ ⑧): 「이 세션 동안 허용」 은 그 실행
/// 하나를 **그 내용으로** 다시 띄우는 것에만 닿는다 — 허용 뒤 구성 · `.env` · `sql` 파일이 바뀌면 다른 열쇠라 다시 묻는다(그 실행은
/// 끝난 뒤에도 표에 남아 같은 `runId` 로 또 부를 수 있다). `run_stop` 은 띄우는 것이 없어 `human:<runId>` 다.
pub(super) fn human_key(before: Option<&Before>, content: &str) -> String {
    format!(
        "{HUMAN_KEY_PREFIX}{}:{content}",
        before.map(|b| b.run_id.as_str()).unwrap_or_default()
    )
}

/// 「사람이 본 구성」 해시를 만들지 못한 구성(정의 본문이 없는 `mise-task` 등)의 내용 해시 — 띄울 것과 `.env` 다이제스트 · 구성
/// 정규형. 해시의 꼴(`sha256:`)만 같고 기록과 견주지 않는다(늘 묻는다).
pub(super) fn fallback_content(resolved: &Resolved) -> String {
    let covered = json!({
        "v": "pitwall.run.plan.content.v1",
        "config": resolved.config.canonical(),
        "argv": resolved.argv,
        "cwd": resolved.cwd,
        "env": resolved.run_env.vars(),
        "envFiles": resolved.run_env.files(),
        "profile": resolved.run_env.profile(),
        "definition": resolved.definition,
    });
    let digest = Sha256::digest(covered.to_string().as_bytes());
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn summary(key: &str, config: &RunConfig, before: Option<&Before>) -> String {
    let what = if config.name.is_empty() || config.name == key {
        format!("`{key}`")
    } else {
        format!("`{}` ({key})", config.name)
    };
    match before {
        None => format!("Run {what} — {}", config.kind.type_name()),
        Some(before) if before.by_person => format!(
            "Restart the run `{}` that a person started: {what} — {}",
            before.run_id,
            config.kind.type_name()
        ),
        Some(before) => format!(
            "Restart the run `{}`: {what} — {}",
            before.run_id,
            config.kind.type_name()
        ),
    }
}

/// 사람이 보는 계획(머리 주석 「계획 = 띄울 것 그대로」): 띄울 것 · env · `.env` 파일, 그리고 argv 가 비밀처럼 보이면 경고.
pub(super) fn shown_changes(resolved: &Resolved, config: &RunConfig) -> Vec<PlanChange> {
    let mut changes = Vec::new();
    let argv = serde_json::to_string(&resolved.argv).unwrap_or_default();
    let mut run = format!(
        "Kind: {}\nCommand (argv): {argv}\nWorking directory: {}",
        config.kind.type_name(),
        resolved.cwd.display(),
    );
    match resolved.run_env.profile() {
        Some(profile) => run.push_str(&format!("\nProfile: {profile}")),
        None => run.push_str("\nProfile: (none)"),
    }
    changes.push(PlanChange::Text {
        label: "Run".to_owned(),
        text: run,
    });
    let vars = resolved.run_env.vars();
    let env = if vars.is_empty() {
        "(nothing added to the runtime environment)".to_owned()
    } else {
        vars.iter()
            .map(|(name, value)| {
                let from = match resolved.run_env.origin(name) {
                    Some(EnvOrigin::File(path)) => format!("from {path}"),
                    Some(EnvOrigin::Profile) => "from the profile".to_owned(),
                    Some(EnvOrigin::Config) | None => "from the configuration".to_owned(),
                };
                format!("{name}={}    [{from}]", shown::env_value(name, value))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    changes.push(PlanChange::Text {
        label: "Environment".to_owned(),
        text: env,
    });
    if !resolved.run_env.files().is_empty() {
        let files = resolved
            .run_env
            .files()
            .iter()
            .map(|f| format!("{}  sha256 {}", f.path, f.sha256))
            .collect::<Vec<_>>()
            .join("\n");
        changes.push(PlanChange::Text {
            label: "Environment files".to_owned(),
            text: files,
        });
    }
    if let Some(warning) = shown::secret_warning(resolved.argv.iter().map(String::as_str)) {
        changes.push(PlanChange::Text {
            label: "Warning".to_owned(),
            text: warning,
        });
    }
    changes
}

/// 오류 `data` 의 최상위 `audit` 에 감사 상세를 싣는다(`pw_mcp` 의 「툴 메서드 규약」).
pub(super) fn with_audit(mut error: RpcError, audit: &str) -> RpcError {
    let data = match error.data.take() {
        Some(Value::Object(mut map)) => {
            map.entry("audit").or_insert_with(|| audit.into());
            Value::Object(map)
        }
        Some(other) => json!({ "message": other, "audit": audit }),
        None => json!({ "audit": audit }),
    };
    error.data = Some(data);
    error
}

/// 버려지면 그 실행을 멈춘다(`sql` 실행을 기다리는 호출 — MCP 취소 · 끊김).
struct StopOnDrop {
    runs: Runs,
    project_id: String,
    run_id: Option<RunId>,
}

impl StopOnDrop {
    fn disarm(mut self) {
        self.run_id = None;
    }
}

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if let Some(run_id) = self.run_id.take() {
            tracing::info!(target: "pw_run", run = %run_id, "the call waiting for the sql run went away; stopping the run");
            let _ = self.runs.stop(&self.project_id, &run_id);
        }
    }
}

/// 블로킹 스레드에서 돌린다.
pub(super) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 툴 메서드를 붙인다(`super::methods::register` 가 부른다).
pub fn register(router: &mut Router, runs: Runs) {
    let (start, stop, restart) = (runs.clone(), runs.clone(), runs);
    router
        .unary(
            RunToolStartMethod,
            move |ctx, planned: Planned<RunToolStartParams>| {
                let runs = start.clone();
                async move { runs.tool_start(&ctx, planned).await }
            },
        )
        .unary(
            RunToolStopMethod,
            move |ctx, planned: Planned<RunToolRunParams>| {
                let runs = stop.clone();
                async move { runs.tool_stop(&ctx, planned).await }
            },
        )
        .unary(
            RunToolRestartMethod,
            move |ctx, planned: Planned<RunToolRunParams>| {
                let runs = restart.clone();
                async move { runs.tool_restart(&ctx, planned).await }
            },
        );
}
