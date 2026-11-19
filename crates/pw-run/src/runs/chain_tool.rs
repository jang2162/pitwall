//! 에이전트의 `run_start` · `run_restart` 가 사슬(`before` · 복합 구성)을 띄우는 길(17-m6-plan M6-21 ④ ⑤ — 결정 D68 재리뷰 B1).
//! 툴 메서드는 `exec_tool` 이고, 구성이 사슬이면 계획 · 띄우기를 여기로 넘긴다. 차례 · 중지 · 「확인 때 푼 내용만」 은 `chain`
//! 머리 주석.
//!
//! - **계획에 모든 단계를 싣는다**: 프로세스 단계는 지금 푼 [`RunSpec`](해시에 든다)과 그 보이는 내용, `sql` 단계는 pw-db 가 지금
//!   읽어 분류한 계획(`PlanChange::Sql` — 문장 전부), 그리고 차례 한 줄. 대상 값 `content` 는 단계마다의 내용 해시와 복합 구성의
//!   해시 · 차례를 합친 것이다.
//! - **등급은 단계 가운데 가장 높은 것**(`sql` 의 쓰기가 들면 `dangerous` — 매번 묻는다, 기억 없음).
//! - **정책 자동은 모두가 「사람이 본 구성」 일 때만**: 프로세스 단계마다 그 내용 해시 · 지나는 복합 구성마다 그 해시가
//!   `Registry::human_saw` 이고, `sql` 단계가 모두 조회만(정책 자동)일 때. 사람이 본 구성 사이에 본 적 없는 구성 하나가 끼면
//!   전체가 묻는다. [허용] 하면 단계 · 복합 구성마다 (열쇠, 해시)를 기록한다(`PlanBuilder::seen`).
//! - **승인 뒤에는 계획의 것만 띄운다**: 프로세스는 허가의 `RunSpec`, `sql` 은 계획 때 읽은 글. 앞 단계가 돈 뒤의 단계는 띄우기 전에
//!   지금 내용을 다시 풀어 계획의 해시와 견주고, 다르면 `planChanged` 로 사슬을 멈춘다.
//! - 감사 상세는 맨 위 구성 · 합친 내용 해시 · 단계 수 · `runId` 들뿐이다(명령줄 · env 값 · SQL 원문이 없다).

use std::sync::Arc;

use pw_core::registry::agent::AgentGrant;
use pw_core::registry::exec::{RunSpec, agent_run_grant};
use pw_core::registry::plan::{Approved, PlanChange, PlanRules, Planned, Remember, ToolPlan};
use pw_rpc::{Ctx, RpcError, Tier};
use serde::Serialize;
use serde_json::{Value, json};

use super::chain::{
    ChainExec, ChainStepRun, StopChainOnDrop, attach_or_stop, combined, plan_changed,
};
use super::exec_tool::{
    AgentGrantRef, Before, Prepared, RunToolStarted, SqlToolLaunch, SqlToolRequest, StartTarget,
    blocking, fallback_content, human_key, shown_changes, with_audit,
};
use super::tool::tool_run;
use super::{AgentLaunch, AgentStart, LaunchedBy, Runs};
use crate::config::graph::{self, Chain};
use crate::config::{RunConfig, RunConfigPlace, RunKindConfig};

/// 사슬의 `sql` 단계 하나의 끝(구성 열쇠, 요약 · 감사 상세).
type SqlDone = (String, tokio::sync::oneshot::Receiver<(Value, String)>);

/// 사슬이 끝난 뒤 `sql` 단계의 요약을 기다리는 시한(실행이 끝난 뒤 요약이 오는 틈 — 길지 않다).
const SQL_SUMMARY_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// 승인을 지난 사슬(띄울 것).
pub(crate) struct ChainPlanned {
    chain: Chain,
    steps: Vec<AgentStep>,
    content: String,
}

enum AgentStep {
    Process {
        key: String,
        /// 허가의 `runs()` 안 자리.
        spec: usize,
        content: String,
        profile: Option<String>,
    },
    Sql {
        key: String,
        launch: Box<dyn SqlToolLaunch>,
        request: Box<SqlToolRequest>,
        /// 계획의 대상 값(`content` · `db`) — 다시 계획해 견준다.
        targets: Value,
    },
}

/// 단계 하나의 계획 재료.
struct PlannedStep {
    tier: Tier,
    /// 정책 자동이어도 되는가(프로세스: 사람이 본 구성, `sql`: pw-db 의 규칙).
    auto: bool,
    /// `sql` 단계가 매번 묻는가(`EVERY_TIME`).
    every_time: bool,
    content: String,
    changes: Vec<PlanChange>,
    seen: Option<(String, String)>,
}

impl Runs {
    /// 사슬의 계획을 만들고 승인을 지난다(첫 호출은 「승인 필요」 로 돌아간다).
    #[allow(clippy::too_many_arguments)] // `exec_tool::Runs::plan_and_approve` 의 재료 그대로.
    pub(super) async fn plan_chain<P: Serialize + Sync>(
        &self,
        ctx: &Ctx,
        method: &str,
        planned: &Planned<P>,
        agent: &AgentGrant,
        target: StartTarget<'_>,
        before: Option<&Before>,
        place: &RunConfigPlace,
        key: &str,
    ) -> Result<(Approved, Prepared), RpcError> {
        let host_projects = self.inner.host()?.projects.clone();
        let project_id = agent.project_id().to_owned();
        let chain = {
            let set = crate::config::read(place);
            let configs: std::collections::BTreeMap<&str, &RunConfig> = set
                .configs
                .iter()
                .map(|c| (c.id.as_str(), &c.config))
                .collect();
            let unusable = |k: &str| set.unusable.iter().any(|u| u.id == k);
            graph::expand(&|k| configs.get(k).copied(), &unusable, key).map_err(|why| {
                RpcError::invalid_params(format!("run config `{key}` cannot be used: {why}"))
            })?
        };
        let top_process = chain.steps.iter().any(|s| s.top);
        if target.profile.is_some() && !top_process {
            return Err(RpcError::invalid_params(format!(
                "run config `{key}` is a compound configuration; profiles apply to one configuration"
            )));
        }
        let worktree = target.worktree.map(str::to_owned);
        let human = before.is_some_and(|b| b.by_person);
        let mut specs: Vec<RunSpec> = Vec::new();
        let mut steps: Vec<AgentStep> = Vec::with_capacity(chain.steps.len());
        let mut planned_steps: Vec<PlannedStep> = Vec::with_capacity(chain.steps.len());
        for (i, step) in chain.steps.iter().enumerate() {
            let n = i + 1;
            let profile = if step.top {
                target.profile.map(str::to_owned)
            } else {
                None
            };
            let restart_of = before
                .filter(|b| b.config == step.key)
                .map(|b| b.run_id.clone());
            if matches!(step.config.kind, RunKindConfig::Sql { .. }) {
                let Some(sql) = self.inner.sql_tool.get().cloned() else {
                    return Err(RpcError::invalid_params(
                        "sql configurations cannot run in this Pitwall host",
                    ));
                };
                let request = SqlToolRequest {
                    scope: ctx.scope().clone(),
                    session: ctx.session(),
                    project_id: project_id.clone(),
                    worktree: worktree.clone(),
                    key: step.key.clone(),
                    config: step.config.clone(),
                    restart_of,
                };
                let plan = sql.plan(request.clone()).await?;
                let targets: Value = plan
                    .targets
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
                let content = targets
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                planned_steps.push(PlannedStep {
                    tier: plan.tier,
                    auto: plan.rules.policy_auto,
                    every_time: plan.rules.remember == Remember::Never,
                    content,
                    changes: vec![
                        PlanChange::Text {
                            label: format!("Step {n}: {}", step.key),
                            text: plan.summary.clone(),
                        },
                        plan.change.clone(),
                    ],
                    seen: None,
                });
                steps.push(AgentStep::Sql {
                    key: step.key.clone(),
                    launch: plan.launch,
                    request: Box::new(request),
                    targets,
                });
                continue;
            }
            let (resolved, config) = {
                let (runs, agent_project, worktree, profile, key) = (
                    self.clone(),
                    agent.project().clone(),
                    worktree.clone(),
                    profile.clone(),
                    step.key.clone(),
                );
                blocking(move || {
                    runs.plan_process_for(
                        &AgentGrantRef(&agent_project),
                        worktree.as_deref(),
                        &key,
                        profile.as_deref(),
                    )
                })
                .await?
            };
            let seen = resolved.seen_hash();
            let content = match &seen {
                Ok(hash) => hash.as_str().to_owned(),
                Err(_) => fallback_content(&resolved),
            };
            let saw = seen
                .as_ref()
                .is_ok_and(|h| host_projects.human_saw(&project_id, &step.key, h));
            let mut changes = vec![PlanChange::Text {
                label: format!("Step {n}: {}", step.key),
                // 사람이 본 단계인지는 적지 않는다 — [허용] 의 기록이 둘째 호출의 계획을 바꾸면 안 된다(`plan_chain` 의 끝).
                text: format!("{} — {}", config.name, config.kind.type_name()),
            }];
            changes.extend(
                shown_changes(&resolved, &config)
                    .into_iter()
                    .map(|c| match c {
                        PlanChange::Text { label, text } => PlanChange::Text {
                            label: format!("Step {n}: {label}"),
                            text,
                        },
                        other => other,
                    }),
            );
            planned_steps.push(PlannedStep {
                tier: Tier::Exec,
                auto: saw,
                every_time: false,
                content: content.clone(),
                changes,
                seen: seen.ok().map(|h| (step.key.clone(), h.as_str().to_owned())),
            });
            specs.push(RunSpec {
                config: step.key.clone(),
                argv: resolved.argv.clone(),
                cwd: resolved.cwd.clone(),
                env: resolved.run_env.vars().clone(),
                profile: profile.clone(),
            });
            steps.push(AgentStep::Process {
                key: step.key.clone(),
                spec: specs.len() - 1,
                content,
                profile,
            });
        }
        // 복합 구성마다 (열쇠, 해시, 사람이 봤는가).
        let compounds: Vec<(String, String, bool)> = chain
            .compounds
            .iter()
            .map(|(k, c)| {
                let hash = crate::seen::compound_hash(k, c);
                let saw = host_projects.human_saw(&project_id, k, &hash);
                (k.clone(), hash.to_string(), saw)
            })
            .collect();
        let compounds_saw = compounds.iter().all(|(_, _, saw)| *saw);
        let mut parts: Vec<(String, String)> = chain
            .steps
            .iter()
            .zip(&planned_steps)
            .map(|(s, p)| (s.key.clone(), p.content.clone()))
            .collect();
        parts.extend(compounds.iter().map(|(k, h, _)| (k.clone(), h.clone())));
        parts.push(("order".to_owned(), chain.order_text()));
        let content = combined("pitwall.run.chain.plan.v1", &parts);
        let tier = planned_steps
            .iter()
            .map(|p| p.tier)
            .max()
            .unwrap_or(Tier::Exec);
        let every_time = planned_steps.iter().any(|p| p.every_time) || tier == Tier::Dangerous;
        let all_seen = planned_steps.iter().all(|p| p.auto) && compounds_saw;
        let steps_target: Vec<Value> = chain
            .steps
            .iter()
            .zip(&planned_steps)
            .map(|(s, p)| json!({ "config": s.key, "kind": s.config.kind.type_name(), "content": p.content }))
            .collect();
        let sql_targets: Vec<Value> = steps
            .iter()
            .filter_map(|s| match s {
                AgentStep::Sql { key, targets, .. } => {
                    Some(json!({ "config": key, "targets": targets }))
                }
                AgentStep::Process { .. } => None,
            })
            .collect();
        let order = chain.order_text();
        let make = |policy_auto: bool| {
            let rules = if every_time {
                PlanRules {
                    auto_approve: false,
                    remember: Remember::Never,
                    policy_auto: false,
                }
            } else if human {
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
            let restart = before.map(|b| b.run_id.as_str());
            let mut builder = ToolPlan::builder(method, tier, &planned.params)
                .summary(format!(
                    "{} `{key}` — {} configurations in order: {order}",
                    if restart.is_some() { "Restart" } else { "Run" },
                    chain.steps.len()
                ))
                .change(PlanChange::Text {
                    label: "Order".to_owned(),
                    text: format!(
                        "{order}\nEach step before the last waits until the one before it exited with code 0; a failure stops the chain there."
                    ),
                })
                .target("content", &content)
                .target("steps", &steps_target)
                .target("sql", &sql_targets)
                .rules(rules)
                .runs(specs.clone());
            for p in &planned_steps {
                for change in &p.changes {
                    builder = builder.change(change.clone());
                }
                if let Some((k, h)) = &p.seen {
                    builder = builder.seen(k.clone(), h.clone());
                }
            }
            for (k, h, _) in &compounds {
                builder = builder.seen(k.clone(), h.clone());
            }
            if let Some(run_id) = restart {
                builder = builder.target("restartOf", &run_id);
            }
            builder.build()
        };
        let auto = all_seen && !every_time && !human;
        let audit = format!("config={key} content={content} steps={}", chain.steps.len());
        // [허용] 의 기록이 둘째 호출의 계획을 정책 자동으로 바꾼다 — `exec_tool` 의 같은 자리와 같은 까닭으로 꺼진 계획도 견준다.
        let approved = match planned.approve(make(auto)) {
            Ok(approved) => approved,
            Err(e) if auto && planned.ticket.is_some() => planned
                .approve(make(false))
                .map_err(|_| with_audit(e, &audit))?,
            Err(e) => return Err(with_audit(e, &audit)),
        };
        Ok((
            approved,
            Prepared::Chain(Box::new(ChainPlanned {
                chain,
                steps,
                content,
            })),
        ))
    }

    /// 승인을 지난 사슬을 띄운다(다시 실행의 앞 실행은 부른 쪽이 이미 멈췄다). 사슬이 다 띄워지거나 실패할 때까지 기다린다.
    pub(super) async fn launch_chain(
        &self,
        ctx: &Ctx,
        agent: &AgentGrant,
        approved: Approved,
        planned: ChainPlanned,
        worktree: Option<String>,
        before: Option<&Before>,
    ) -> Result<RunToolStarted, RpcError> {
        let ChainPlanned {
            chain,
            steps,
            content,
        } = planned;
        let has_process = steps.iter().any(|s| matches!(s, AgentStep::Process { .. }));
        let grant = if has_process {
            Some(Arc::new(agent_run_grant(agent, &approved)?))
        } else {
            None
        };
        let session = format!("{:?}", ctx.session());
        let no_return = ctx.point_of_no_return();
        let project_id = agent.project_id().to_owned();
        let agent_project = agent.project().clone();
        let mut runs_of = Vec::with_capacity(steps.len());
        // `sql` 단계의 끝(요약 · 감사 상세 — 단독 `sql` 실행과 같은 것, 리뷰 F3). 사슬이 끝난 뒤 모은다.
        let sql_done: Arc<std::sync::Mutex<Vec<SqlDone>>> = Arc::default();
        for step in steps {
            match step {
                AgentStep::Process {
                    key,
                    spec,
                    content,
                    profile,
                } => {
                    let runs = self.clone();
                    let grant = grant.clone().expect("a process step has a grant");
                    let (worktree, session, project) =
                        (worktree.clone(), session.clone(), agent_project.clone());
                    let restart_of = before.filter(|b| b.config == key).map(|b| b.run_id.clone());
                    let step_key = key.clone();
                    let project_id = project_id.clone();
                    runs_of.push(ChainStepRun {
                        key,
                        launch: Box::new(move |recheck, ctl| {
                            Box::pin(blocking(move || {
                                if recheck {
                                    let now = runs.plan_process_for(
                                        &AgentGrantRef(&project),
                                        worktree.as_deref(),
                                        &step_key,
                                        profile.as_deref(),
                                    );
                                    let same = now.is_ok_and(|(resolved, _)| {
                                        let now = match resolved.seen_hash() {
                                            Ok(h) => h.as_str().to_owned(),
                                            Err(_) => fallback_content(&resolved),
                                        };
                                        now == content
                                    });
                                    if !same {
                                        return Err(plan_changed(&step_key));
                                    }
                                }
                                super::chain::launch_hook(&step_key);
                                let info = runs.start_agent_with(
                                    &grant,
                                    spec,
                                    AgentLaunch {
                                        session,
                                        client: None,
                                    },
                                    AgentStart {
                                        size: None,
                                        worktree,
                                        restart_of,
                                    },
                                )?;
                                // 블로킹 작업 안에서 넣는다 — 그사이 호출이 버려져 사슬이 멈췄으면 이 실행도 멈춘다(리뷰 F4).
                                attach_or_stop(&runs, &project_id, &step_key, info, &ctl)
                            }))
                        }),
                    });
                }
                AgentStep::Sql {
                    key,
                    launch,
                    request,
                    targets,
                } => {
                    let runs = self.clone();
                    let sql = self.inner.sql_tool.get().cloned();
                    let (approved, no_return, session) =
                        (approved.clone(), no_return.clone(), session.clone());
                    let step_key = key.clone();
                    let (done_list, project_id) = (Arc::clone(&sql_done), project_id.clone());
                    runs_of.push(ChainStepRun {
                        key,
                        launch: Box::new(move |recheck, ctl| {
                            Box::pin(async move {
                                if recheck {
                                    let sql = sql.ok_or_else(|| plan_changed(&step_key))?;
                                    let now = sql
                                        .plan(*request)
                                        .await
                                        .map_err(|_| plan_changed(&step_key))?;
                                    let same = ["content", "db", "dialect"].iter().all(|name| {
                                        now.targets.iter().find(|(k, _)| k == name).map(|(_, v)| v)
                                            == targets.get(*name)
                                    });
                                    if !same {
                                        return Err(plan_changed(&step_key));
                                    }
                                }
                                let started = launch
                                    .launch(
                                        approved,
                                        runs.clone(),
                                        LaunchedBy::Agent {
                                            session,
                                            client: None,
                                        },
                                        no_return,
                                    )
                                    .await?;
                                done_list
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .push((step_key.clone(), started.done));
                                attach_or_stop(&runs, &project_id, &step_key, started.info, &ctl)
                            })
                        }),
                    });
                }
            }
        }
        let (exec, _first) = ChainExec::new(
            self,
            &project_id,
            &chain.key,
            LaunchedBy::Agent {
                session: session.clone(),
                client: None,
            },
            runs_of,
            Some(ctx.cancellation().clone()),
        );
        // 호출이 버려지면(MCP 취소 · 끊김 — 되돌릴 수 없는 지점 앞) 띄우는 중인 사슬을 멈춘다. 다 띄운 뒤에는 아무것도 하지 않는다.
        let _guard = StopChainOnDrop(exec.ctl());
        let audit = format!(
            "config={} content={content} steps={}",
            chain.key,
            chain.steps.len()
        );
        let result = exec.run(chain.root).await;
        // `sql` 단계의 요약 · 감사 상세(트랜잭션의 끝 — `transaction=committed` · `unknown` …)를 성공이든 실패든 싣는다.
        let done = std::mem::take(
            &mut *sql_done
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let mut summaries = serde_json::Map::new();
        let mut audit = audit;
        for (key, rx) in done {
            match tokio::time::timeout(SQL_SUMMARY_WAIT, rx).await {
                Ok(Ok((summary, sql_audit))) => {
                    audit.push_str(&format!(" step={key} {sql_audit}"));
                    summaries.insert(key, summary);
                }
                _ => audit.push_str(&format!(" step={key} transaction=unknown")),
            }
        }
        let launched = result.map_err(|e| with_audit(e, &audit))?;
        let ids: Vec<&str> = launched.iter().map(|r| r.run_id.as_str()).collect();
        let audit = format!("{audit} runIds={}", ids.join(","));
        let last = launched
            .last()
            .cloned()
            .ok_or_else(|| RpcError::internal("the chain started nothing"))?;
        Ok(RunToolStarted {
            run: tool_run(last),
            sql: (!summaries.is_empty()).then_some(Value::Object(summaries)),
            audit,
            chain: launched.into_iter().map(tool_run).collect(),
        })
    }
}
