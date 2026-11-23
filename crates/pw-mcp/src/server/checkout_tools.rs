//! 세션 툴의 처리(`crate::session_tools` — 18-m7-plan M7-19). 노출 · 감사 · 승인은 Host 툴과 같은 자리(`Session` 의 도우미)를 쓴다.
//!
//! **`checkout_send` 의 길**(M7-17 의 화면 흐름과 같은 RPC — Desktop 의 세션이 두 머신 연결을 잇는다):
//!
//! 1. 계획(부작용 없음): 대상 `git.send.theirs`(가진 커밋 · 브랜치 끝) → 원본 `git.send.compare`(끝 커밋 · 보낼 커밋 수 ·
//!    fast-forward) → 대상 `git.send.plan`(대상 Host 가 자기가 바꿀 것으로 만든 해시). 갈라졌으면 여기서 오류이고 대상은 그대로다.
//!    세션의 계획 해시는 (원본 · 대상 머신 · 체크아웃 · 브랜치 · 대상 끝 커밋 · 원본 끝 커밋 · 대상 계획 해시)를 덮는다.
//! 2. 승인: 대상이 다른 머신이면 `approve_remote`(머신 허용 · 미신뢰 · `auto_approve` 미적용 — M7-18), 이 머신이면 `approve`. 원본이
//!    다른 머신이고 대상이 이 머신(가져오기)이면 계획이 `auto_approve` 를 끈다 — 다른 머신의 커밋(그 커밋이 가져온 훅)이 이 머신에서
//!    돈다(이월 메모 「훅 비대칭」).
//! 3. 해시 재확인: 승인 뒤 같은 읽기로 계획을 다시 세워 해시가 다르면 돌리지 않는다(`plan_changed`) — 그사이 원본에 더한 커밋 ·
//!    움직인 대상.
//! 4. 부작용(선기록 뒤): 원본 `git.send.prepare`(**계획 때 읽은 끝 커밋 id 로** — 브랜치 이름으로 다시 읽지 않는다) → 대상
//!    `receive.begin` · `chunk` · `finish` → `apply`(`expected` = 계획 때 읽은 대상 끝). 받기 · 맞추기에는 대상 계획 해시를 승인 표로
//!    싣는다 — 대상 Host 가 다시 만들어 대조한다(`pw_git::send::agent`). git 이 멈추면 그 원문을 툴 오류로, 대상은 그대로다.

use pw_core::registry::bundle::ProjectBundle;
use pw_core::registry::plan::{PlanChange, PlanRules};
use serde::Deserialize;

use super::*;
use crate::checkout::LinkState;
use crate::session_tools::{
    CheckoutEntry, CheckoutListInput, CheckoutListResult, CheckoutMachine, CheckoutRef,
    CheckoutSendInput, CheckoutSendResult, LIST_METHOD, SEND_METHOD, source_machine_name,
};

/// git 쓰기 실행기의 「듣는 창 없음」(`pw_git::write::code::NO_WINDOW`) — 미저장 확인을 물을 창이 없다.
const NO_WINDOW: i64 = 2006;

/// `checkout_send` 의 한쪽.
struct Side {
    /// 머신 id(이 머신이면 `local`).
    machine: String,
    /// 에이전트에게 보일 머신 이름.
    shown: String,
    /// 그 머신의 프로젝트 id.
    project: String,
    worktree: Option<String>,
    /// 다른 머신이면 그 연결.
    remote: Option<RemoteAim>,
}

impl Side {
    fn at(&self) -> Value {
        serde_json::json!({ "projectId": self.project, "worktree": self.worktree })
    }

    fn describe(&self) -> String {
        match &self.worktree {
            Some(w) => format!("{} ({w})", self.shown),
            None => self.shown.clone(),
        }
    }
}

/// 계획 때 두 머신에서 읽은 것.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SendRead {
    theirs: Vec<String>,
    /// 대상 브랜치의 끝(없으면 만든다).
    tip: Option<String>,
    /// 원본 끝 커밋 — 보낼 커밋.
    commit: String,
    commits: u32,
    /// 대상 Host 의 계획 해시(받기 · 맞추기의 승인 표).
    target_plan: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Theirs {
    theirs: Vec<String>,
    branch_tip: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Compare {
    commit: Option<String>,
    commits: u32,
    fast_forward: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetPlan {
    hash: String,
    branch_tip: Option<String>,
}

/// 부작용 단계가 실패한 까닭.
enum SendFailure {
    Call(CallError),
    /// git 이 멈췄다(대상은 그대로) — 에이전트에게 갈 글.
    Stopped(String),
    Other(String),
}

impl From<CallError> for SendFailure {
    fn from(e: CallError) -> Self {
        SendFailure::Call(e)
    }
}

fn error(message: String) -> Refusal {
    Refusal {
        message,
        mark: Mark::error(None),
    }
}

fn refusal_of(e: CallError) -> Refusal {
    match e {
        CallError::Cancelled => Refusal::cancelled(),
        CallError::Rpc { code, .. } if code == i64::from(pw_rpc::code::CANCELLED) => {
            Refusal::cancelled()
        }
        e => error(e.to_string()),
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

fn parse<T: serde::de::DeserializeOwned>(method: &str, value: Value) -> Result<T, Refusal> {
    serde_json::from_value(value).map_err(|e| error(format!("unexpected `{method}` result: {e}")))
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty())
}

impl Session {
    /// 세션 툴 하나(`Session::call` 이 노출을 본 뒤 부른다).
    pub(super) async fn session_tool(
        &self,
        tool: &Tool,
        mut args: Map<String, Value>,
        started: Instant,
        cancel: &CancellationToken,
    ) -> CallToolResult {
        if !args.contains_key(PROJECT_ID)
            && let Some(project) = &self.inner.project
        {
            args.insert(PROJECT_ID.into(), Value::String(project.id.clone()));
        }
        match tool.method {
            LIST_METHOD => self.checkout_list(tool, args, started, cancel).await,
            SEND_METHOD => self.checkout_send(tool, args, started, cancel).await,
            other => failed(
                format!("`{other}` is not a session tool"),
                Mark::error(None),
            ),
        }
    }

    /// 거절 · 오류 행을 남기고 그 결과.
    fn refused(
        &self,
        tool: &Tool,
        args: &Map<String, Value>,
        project_id: Option<String>,
        refusal: Refusal,
        started: Instant,
    ) -> CallToolResult {
        self.audit(
            None,
            &tool.name,
            args,
            project_id,
            refusal.mark.outcome,
            Some(refusal.message.clone()),
            None,
            started,
        );
        failed(refusal.message, refusal.mark)
    }

    /// 다른 머신을 아는 앱 세션의 그 묶음.
    fn session_bundle(&self, project_id: &str) -> Result<(&dyn Machines, ProjectBundle), Refusal> {
        let (Some(machines), false) = (self.inner.services.machines.as_deref(), self.headless())
        else {
            return Err(Refusal::denied(
                Reason::MachineUnavailable,
                "this session does not know other checkouts: only a session of the running Pitwall desktop app does"
                    .to_owned(),
            ));
        };
        Ok((machines, bundle_of(machines, project_id)?))
    }

    /// 묶음의 이 머신 멤버 설정 · 노출(다른 툴과 같은 판정 — `Session::call` 의 대상 프로젝트 판정).
    async fn bundle_member_allows(
        &self,
        tool: &Tool,
        local: &str,
        cancel: &CancellationToken,
    ) -> Result<(), Refusal> {
        if let Some(refusal) = self.target_config_unreadable(local, cancel).await? {
            return Err(refusal);
        }
        if self.inner.project.as_ref().is_none_or(|p| p.id != local) {
            match self.target_exposes(tool, local, cancel).await {
                Ok(true) => {}
                Ok(false) => {
                    return Err(Refusal::denied(
                        Reason::NotExposed,
                        format!(
                            "`{}` is turned off in the [mcp] settings of project `{local}`",
                            tool.name
                        ),
                    ));
                }
                Err(e) => return Err(refusal_of(e)),
            }
        }
        Ok(())
    }

    fn input<T: serde::de::DeserializeOwned>(
        tool: &Tool,
        args: &Map<String, Value>,
    ) -> Result<T, Refusal> {
        serde_json::from_value(Value::Object(args.clone()))
            .map_err(|e| error(format!("invalid input for `{}`: {e}", tool.name)))
    }

    // --- checkout_list ---

    async fn checkout_list(
        &self,
        tool: &Tool,
        args: Map<String, Value>,
        started: Instant,
        cancel: &CancellationToken,
    ) -> CallToolResult {
        let project_id = args
            .get(PROJECT_ID)
            .and_then(Value::as_str)
            .map(str::to_owned);
        match self.list(tool, &args, cancel).await {
            Ok((routing, result)) => {
                let value = serde_json::to_value(result).expect("결과는 직렬화된다");
                self.finished(None, &tool.name, &args, routing, tool, value, started, None)
            }
            Err(refusal) => self.refused(tool, &args, project_id, refusal, started),
        }
    }

    async fn list(
        &self,
        tool: &Tool,
        args: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> Result<(Option<String>, CheckoutListResult), Refusal> {
        let input: CheckoutListInput = Self::input(tool, args)?;
        let (machines, bundle) = self.session_bundle(&input.project_id)?;
        let local = bundle.local_project().map(str::to_owned);
        if let Some(local) = &local {
            self.bundle_member_allows(tool, local, cancel).await?;
        }
        let mut out = Vec::new();
        for member in &bundle.members {
            if checkout::is_local(&member.machine) {
                let checkouts = self
                    .worktrees(&self.inner.client, &member.project_id, cancel)
                    .await?;
                out.push(CheckoutMachine {
                    machine: member.machine.clone(),
                    state: "local".into(),
                    checkouts,
                });
                continue;
            }
            let shown = machines
                .machine_name(&member.machine)
                .unwrap_or_else(|| member.machine.clone());
            // 가벼운 확인만 — 붙어 있지 않은 머신에는 링크를 열지 않는다(붙은 머신은 그 링크를 쥐고 읽는다).
            let state = tokio::select! {
                state = machines.link_state(&member.machine) => state,
                () = cancel.cancelled() => return Err(Refusal::cancelled()),
            };
            let checkouts = if state == LinkState::Connected {
                let target = Unlinked {
                    machine: member.machine.clone(),
                    project: member.project_id.clone(),
                    routing: local.clone().unwrap_or_else(|| member.project_id.clone()),
                    shown: shown.clone(),
                };
                match self.link(target, cancel).await {
                    Ok(aim) => {
                        self.worktrees(&aim.link.client, &member.project_id, cancel)
                            .await?
                    }
                    Err(refusal) if refusal.mark.reason == Some(Reason::Cancelled) => {
                        return Err(refusal);
                    }
                    Err(_) => None,
                }
            } else {
                None
            };
            out.push(CheckoutMachine {
                machine: shown,
                state: state.as_str().into(),
                checkouts,
            });
        }
        Ok((
            local,
            CheckoutListResult {
                name: bundle.name.clone(),
                machines: out,
            },
        ))
    }

    /// 그 머신 프로젝트의 체크아웃들(`git.worktrees`). 저장소가 없거나 읽지 못하면 `None`.
    async fn worktrees(
        &self,
        client: &HostClient,
        project: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<CheckoutEntry>>, Refusal> {
        let answer = client
            .call_until(
                "git.worktrees",
                serde_json::json!({ "projectId": project }),
                cancel,
            )
            .await;
        let value = match answer {
            Ok(value) => value,
            Err(CallError::Cancelled) => return Err(Refusal::cancelled()),
            Err(e) => {
                tracing::debug!(%e, project, "cannot list the worktrees for checkout_list");
                return Ok(None);
            }
        };
        let Some(items) = value.get("worktrees").and_then(Value::as_array) else {
            return Ok(None);
        };
        Ok(Some(
            items
                .iter()
                .filter(|w| w.get("missing") != Some(&Value::Bool(true)))
                .filter_map(|w| {
                    Some(CheckoutEntry {
                        path: w.get("path")?.as_str()?.to_owned(),
                        main: w.get("main").and_then(Value::as_bool).unwrap_or(false),
                        branch: branch_of(w.get("head")),
                    })
                })
                .collect(),
        ))
    }

    // --- checkout_send ---

    async fn checkout_send(
        &self,
        tool: &Tool,
        mut args: Map<String, Value>,
        started: Instant,
        cancel: &CancellationToken,
    ) -> CallToolResult {
        let name = tool.name.clone();
        let project_id = args
            .get(PROJECT_ID)
            .and_then(Value::as_str)
            .map(str::to_owned);
        let prepared = match self.send_sides(tool, &mut args, cancel).await {
            Ok(prepared) => prepared,
            Err(refusal) => return self.refused(tool, &args, project_id, refusal, started),
        };
        let (routing, source, target, branch) = prepared;
        let project_id = Some(routing.clone());
        let source_name = source_machine_name(&source.machine);
        let read = match self
            .read_send(&source, &target, &branch, &source_name, cancel)
            .await
        {
            Ok(read) => read,
            Err(refusal) => return self.refused(tool, &args, project_id, refusal, started),
        };
        let plan = send_plan(&args, &source, &target, &branch, &read);
        let hold = match &self.inner.services.approval {
            Approval::Route(route) => Some(route.hold()),
            Approval::Unavailable => None,
        };
        let decided = match &target.remote {
            Some(aim) => self.approve_remote(tool, &plan, aim, cancel).await,
            None => self.approve(tool, &plan, Some(&routing), cancel).await,
        };
        drop(hold);
        let mode = match decided {
            Ok(mode) => mode,
            Err(refusal) => return self.refused(tool, &args, project_id, refusal, started),
        };
        // 해시 재확인 — 승인을 기다리는 사이 원본에 더한 커밋 · 움직인 대상은 이 승인으로 돌지 않는다.
        let again = match self
            .read_send(&source, &target, &branch, &source_name, cancel)
            .await
        {
            Ok(again) => again,
            Err(refusal) => return self.refused(tool, &args, project_id, refusal, started),
        };
        if send_plan(&args, &source, &target, &branch, &again).hash != plan.hash {
            let refusal = Refusal::denied(
                Reason::PlanChanged,
                format!(
                    "the plan of `{name}` changed after it was approved (the source got new commits or the target \
                     branch moved), so nothing was sent; call again to ask for a new approval"
                ),
            );
            return self.refused(tool, &args, project_id, refusal, started);
        }
        let row = match self
            .begin(
                &name,
                &args,
                project_id.clone(),
                Some(mode),
                Some(plan.hash.clone()),
            )
            .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::warn!(%e, tool = name, "cannot write the mcp audit row before the call; refusing it");
                let refusal = Refusal::denied(
                    Reason::AuditUnavailable,
                    format!(
                        "`{name}` needs its audit log entry written before it runs, and Pitwall could not write \
                         it, so it did not run; check that the Pitwall state directory is writable"
                    ),
                );
                return self.refused(tool, &args, project_id, refusal, started);
            }
        };
        match self
            .send(&source, &target, &branch, &source_name, &read, cancel)
            .await
        {
            Ok(result) => {
                let value = serde_json::to_value(result).expect("결과는 직렬화된다");
                self.finished(
                    Some(row),
                    &name,
                    &args,
                    project_id,
                    tool,
                    value,
                    started,
                    None,
                )
            }
            Err(SendFailure::Call(CallError::Rpc { code, .. })) if code == NO_WINDOW => {
                let message = format!(
                    "the target checkout on {} has no Pitwall window to confirm its unsaved documents, so it was \
                     not changed; open that checkout in the Pitwall window of this project (make it the active \
                     checkout) and call again",
                    target.describe()
                );
                self.audit(
                    Some(row),
                    &name,
                    &args,
                    project_id,
                    Outcome::Error,
                    Some(message.clone()),
                    None,
                    started,
                );
                failed(message, Mark::error(None))
            }
            Err(SendFailure::Call(e)) => {
                self.call_failed(Some(row), &name, &args, project_id, e, started, None)
            }
            Err(SendFailure::Stopped(message) | SendFailure::Other(message)) => {
                self.audit(
                    Some(row),
                    &name,
                    &args,
                    project_id,
                    Outcome::Error,
                    Some(message.clone()),
                    None,
                    started,
                );
                failed(message, Mark::error(None))
            }
        }
    }

    /// 입력을 풀어 두 쪽을 정하고(묶음 · 머신 · 멤버) 승인을 물을 창 · 노출을 본 뒤 다른 머신에 붙는다. `args` 는 감사 인자
    /// 요약에 남길 꼴(머신 id · 워크트리)로 바꾼다.
    async fn send_sides(
        &self,
        tool: &Tool,
        args: &mut Map<String, Value>,
        cancel: &CancellationToken,
    ) -> Result<(String, Side, Side, String), Refusal> {
        let input: CheckoutSendInput = Self::input(tool, args)?;
        let (machines, bundle) = self.session_bundle(&input.project_id)?;
        let (source, source_project) = resolve(machines, &bundle, &input.source)?;
        let (target, target_project) = resolve(machines, &bundle, &input.target)?;
        let (source_tree, target_tree) = (
            empty_to_none(input.source.worktree.clone()),
            empty_to_none(input.target.worktree.clone()),
        );
        if source == target {
            return Err(error(if checkout::is_local(&source) {
                "`source` and `target` are on this computer and share one git repository; switch branches with git \
                 there instead"
                    .to_owned()
            } else {
                "`source` and `target` are on the same machine and share one git repository; give checkouts on two \
                 machines"
                    .to_owned()
            }));
        }
        args.insert(
            "source".into(),
            checkout::audit_value(&source, source_tree.as_deref()),
        );
        args.insert(
            "target".into(),
            checkout::audit_value(&target, target_tree.as_deref()),
        );
        // 승인을 물을 자리 — 묶음의 이 머신 멤버 창이 열려 있어야 한다(05 「체크아웃」).
        let Some(routing) = bundle.local_project().map(str::to_owned) else {
            return Err(Refusal::denied(
                Reason::MachineUnavailable,
                format!(
                    "project `{}` has no checkout on this computer, so Pitwall has no window to ask in",
                    bundle.name
                ),
            ));
        };
        args.insert(PROJECT_ID.into(), Value::String(routing.clone()));
        let hears = match &self.inner.services.approval {
            Approval::Route(route) => route.hears(&routing),
            Approval::Unavailable => false,
        };
        if !hears {
            return Err(Refusal::denied(
                Reason::MachineUnavailable,
                format!(
                    "`{}` needs the window of project `{}` open in Pitwall to ask for approval; open it and call again",
                    tool.name, bundle.name
                ),
            ));
        }
        self.bundle_member_allows(tool, &routing, cancel).await?;
        let side = |machine: String, project: String, worktree: Option<String>| {
            let shown = if checkout::is_local(&machine) {
                "this computer".to_owned()
            } else {
                machines
                    .machine_name(&machine)
                    .unwrap_or_else(|| machine.clone())
            };
            Side {
                machine,
                shown,
                project,
                worktree,
                remote: None,
            }
        };
        let mut source = side(source, source_project, source_tree);
        let mut target = side(target, target_project, target_tree);
        for s in [&mut source, &mut target] {
            if !checkout::is_local(&s.machine) {
                let aim = self
                    .link(
                        Unlinked {
                            machine: s.machine.clone(),
                            project: s.project.clone(),
                            routing: routing.clone(),
                            shown: s.shown.clone(),
                        },
                        cancel,
                    )
                    .await?;
                s.remote = Some(aim);
            }
        }
        let branch = match empty_to_none(input.branch) {
            Some(branch) => branch,
            None => self.current_branch(&source, cancel).await?,
        };
        args.insert("branch".into(), Value::String(branch.clone()));
        Ok((routing, source, target, branch))
    }

    fn client_of<'a>(&'a self, side: &'a Side) -> &'a HostClient {
        self.client_for(side.remote.as_ref())
    }

    /// 원본 체크아웃이 올라 있는 브랜치(`branch` 를 주지 않았을 때).
    async fn current_branch(
        &self,
        source: &Side,
        cancel: &CancellationToken,
    ) -> Result<String, Refusal> {
        let value = self
            .client_of(source)
            .call_until("git.worktrees", source.at(), cancel)
            .await
            .map_err(refusal_of)?;
        value
            .get("worktrees")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|w| w.get("current") == Some(&Value::Bool(true)))
            })
            .and_then(|w| branch_of(w.get("head")))
            .ok_or_else(|| {
                error(format!(
                    "the source checkout on {} is not on a branch; give `branch`",
                    source.describe()
                ))
            })
    }

    /// 계획의 읽기(머리 주석 1). 부작용이 없다.
    async fn read_send(
        &self,
        source: &Side,
        target: &Side,
        branch: &str,
        source_name: &str,
        cancel: &CancellationToken,
    ) -> Result<SendRead, Refusal> {
        let (src, dst) = (self.client_of(source), self.client_of(target));
        let mut at = target.at();
        at["branch"] = Value::String(branch.to_owned());
        let theirs: Theirs = parse(
            "git.send.theirs",
            dst.call_until("git.send.theirs", at, cancel)
                .await
                .map_err(refusal_of)?,
        )?;
        let mut at = source.at();
        at["branch"] = Value::String(branch.to_owned());
        at["theirs"] = serde_json::to_value(&theirs.theirs).expect("글자열 목록");
        at["expected"] = serde_json::to_value(&theirs.branch_tip).expect("커밋 id");
        let compared: Compare = parse(
            "git.send.compare",
            src.call_until("git.send.compare", at, cancel)
                .await
                .map_err(refusal_of)?,
        )?;
        let Some(commit) = compared.commit else {
            return Err(error(format!(
                "branch `{branch}` does not exist on {}; nothing was sent",
                source.describe()
            )));
        };
        if !compared.fast_forward {
            return Err(error(format!(
                "branch `{branch}` on {} has commits that {} does not have (the branches diverged), so nothing was \
                 sent and the target is unchanged; merge or rebase so the target's tip is in the source branch, then \
                 call again",
                target.describe(),
                source.describe()
            )));
        }
        let mut at = target.at();
        at["sourceMachine"] = Value::String(source_name.to_owned());
        at["branch"] = Value::String(branch.to_owned());
        at["commit"] = Value::String(commit.clone());
        let planned: TargetPlan = parse(
            "git.send.plan",
            dst.call_until("git.send.plan", at, cancel)
                .await
                .map_err(refusal_of)?,
        )?;
        if planned.branch_tip != theirs.branch_tip {
            return Err(error(format!(
                "branch `{branch}` on {} moved while the send was being planned; call again",
                target.describe()
            )));
        }
        Ok(SendRead {
            theirs: theirs.theirs,
            tip: theirs.branch_tip,
            commit,
            commits: compared.commits,
            target_plan: planned.hash,
        })
    }

    /// 부작용(머리 주석 4). 계획 때 읽은 값으로만 한다.
    async fn send(
        &self,
        source: &Side,
        target: &Side,
        branch: &str,
        source_name: &str,
        read: &SendRead,
        cancel: &CancellationToken,
    ) -> Result<CheckoutSendResult, SendFailure> {
        let (src, dst) = (self.client_of(source), self.client_of(target));
        let ticket = serde_json::json!({ "hash": read.target_plan });
        let mut at = source.at();
        at["branch"] = Value::String(branch.to_owned());
        at["commit"] = Value::String(read.commit.clone());
        at["theirs"] = serde_json::to_value(&read.theirs).expect("글자열 목록");
        let mut stream = src.stream("git.send.prepare", at, cancel).await?;
        let info = match next_item(&mut stream, cancel).await {
            Some(Ok(info)) if info.get("kind") == Some(&Value::from("info")) => info,
            Some(Err(e)) => return Err(e.into()),
            _ => {
                return Err(SendFailure::Other(
                    "the source did not describe the bundle".into(),
                ));
            }
        };
        let size = info.get("size").and_then(Value::as_u64).unwrap_or(0);
        let uncommitted = info.get("uncommitted") == Some(&Value::Bool(true));
        let mut at = target.at();
        at["sourceMachine"] = Value::String(source_name.to_owned());
        at["branch"] = Value::String(branch.to_owned());
        at["commit"] = Value::String(read.commit.clone());
        at["size"] = Value::from(size);
        at[APPROVAL_KEY] = ticket.clone();
        let begun = dst.call_until("git.send.receive.begin", at, cancel).await?;
        let upload = begun.get("uploadId").cloned().unwrap_or(Value::Null);
        loop {
            match next_item(&mut stream, cancel).await {
                None => break,
                Some(Err(e)) => return Err(e.into()),
                Some(Ok(item)) => {
                    dst.call_until(
                        "git.send.receive.chunk",
                        serde_json::json!({
                            "uploadId": upload,
                            "offset": item.get("offset"),
                            "data": item.get("data"),
                        }),
                        cancel,
                    )
                    .await?;
                }
            }
        }
        drop(stream);
        let finished = dst
            .call_until(
                "git.send.receive.finish",
                serde_json::json!({ "uploadId": upload }),
                cancel,
            )
            .await?;
        if finished.get("commit").and_then(Value::as_str) != Some(read.commit.as_str()) {
            return Err(SendFailure::Other(format!(
                "the target received a different commit than {}; it was not applied",
                read.commit
            )));
        }
        let mut at = target.at();
        at["sourceMachine"] = Value::String(source_name.to_owned());
        at["branch"] = Value::String(branch.to_owned());
        at["commit"] = Value::String(read.commit.clone());
        at["expected"] = serde_json::to_value(&read.tip).expect("커밋 id");
        at[APPROVAL_KEY] = ticket;
        let applied = match dst
            .call_until_settled("git.send.apply", at, cancel, SETTLE_AFTER_CANCEL)
            .await
        {
            Settled::Answered(answer)
            | Settled::AnsweredAfterCancel(answer)
            | Settled::FinishedBeforeCancel(answer) => answer?,
            Settled::Cancelled | Settled::Unsettled => return Err(CallError::Cancelled.into()),
        };
        let work_tree = applied
            .get("workTree")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let incoming = applied
            .get("incomingRef")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let outcome = applied.get("outcome").cloned().unwrap_or(Value::Null);
        match outcome.get("kind").and_then(Value::as_str) {
            Some("applied") => {
                let num = |k: &str| {
                    outcome
                        .get(k)
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0)
                };
                let flag = |k: &str| outcome.get(k) == Some(&Value::Bool(true));
                Ok(CheckoutSendResult {
                    branch: branch.to_owned(),
                    commit: read.commit.clone(),
                    previous: read.tip.clone(),
                    commits: read.commits,
                    created: flag("created"),
                    switched: flag("switched"),
                    lfs_pointers: num("lfsPointers"),
                    uncommitted,
                    work_tree,
                    audit: format!(
                        "sent {} commit(s) of {branch} to {}",
                        read.commits, target.machine
                    ),
                })
            }
            _ => {
                let reason = outcome
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("stopped");
                let git = outcome
                    .get("message")
                    .and_then(Value::as_str)
                    .map(|m| format!("\n{}", m.trim_end()))
                    .unwrap_or_default();
                Err(SendFailure::Stopped(format!(
                    "the target stopped the send ({reason}); branch `{branch}` on {} was not changed, and the \
                     received commits are in `{incoming}` there{git}",
                    target.describe()
                )))
            }
        }
    }
}

/// 스트림의 다음 항목. 취소되면 `Cancelled`(스트림을 버리면 `stream.cancel` 이 간다).
async fn next_item(
    stream: &mut crate::client::HostStream<'_>,
    cancel: &CancellationToken,
) -> Option<Result<Value, CallError>> {
    tokio::select! {
        item = stream.next() => item,
        () = cancel.cancelled() => Some(Err(CallError::Cancelled)),
    }
}

/// 체크아웃 입력의 머신 id 와 그 머신의 묶음 멤버 프로젝트.
fn resolve(
    machines: &dyn Machines,
    bundle: &ProjectBundle,
    side: &CheckoutRef,
) -> Result<(String, String), Refusal> {
    let given = side.machine.trim();
    let machine = if checkout::is_local(given) {
        given.to_owned()
    } else {
        machines.machine_id(given).ok_or_else(|| {
            error(format!(
                "unknown machine `{given}`; checkout_list shows the machines of the project"
            ))
        })?
    };
    let member = bundle.member(&machine).ok_or_else(|| {
        error(format!(
            "project `{}` has no checkout on machine `{given}`; checkout_list shows its machines",
            bundle.name
        ))
    })?;
    Ok((machine, member.project_id.clone()))
}

/// `RepoHead` 의 브랜치 이름(브랜치 위 · 아직 커밋 없는 브랜치).
fn branch_of(head: Option<&Value>) -> Option<String> {
    let head = head?;
    match head.get("kind").and_then(Value::as_str) {
        Some("branch" | "unborn") => head.get("name").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// 세션의 계획(머리 주석 1). 해시가 덮는 것: 채운 입력(머신 id · 워크트리 · 브랜치) · 두 프로젝트 · 원본 끝 커밋 · 보낼 커밋 수 ·
/// 대상 끝 커밋 · 대상 계획 해시 · 보이는 글.
fn send_plan(
    args: &Map<String, Value>,
    source: &Side,
    target: &Side,
    branch: &str,
    read: &SendRead,
) -> ToolPlan {
    let into = match &target.worktree {
        Some(w) => format!("worktree {w}"),
        None => "main checkout".to_owned(),
    };
    let result = match &read.tip {
        None => format!(
            "create branch `{branch}` at {} and check it out",
            short(&read.commit)
        ),
        Some(tip) if *tip == read.commit => {
            format!(
                "branch `{branch}` is already at {}; check it out",
                short(tip)
            )
        }
        Some(tip) => format!(
            "fast-forward `{branch}` {} → {} and check it out",
            short(tip),
            short(&read.commit)
        ),
    };
    let hooks = format!(
        "Git runs the hooks of the repository on {} while it fetches, switches and fast-forwards. If its \
         core.hooksPath points into the work tree, those are hooks that come with the sent commits.",
        target.shown
    );
    // 다른 머신의 커밋을 이 머신으로 가져오면 그 커밋의 훅이 이 머신에서 돈다 — `auto_approve` 가 닿지 않게(묻는다).
    let auto_approve = !(source.remote.is_some() && target.remote.is_none());
    ToolPlan::builder(SEND_METHOD, Tier::Write, &Value::Object(args.clone()))
        .summary(format!(
            "Send branch `{branch}` ({} commit(s)) from {} to {}",
            read.commits,
            source.describe(),
            target.describe()
        ))
        .change(PlanChange::Text {
            label: "Target".into(),
            text: format!("{} · {into} · branch `{branch}`", target.shown),
        })
        .change(PlanChange::Text {
            label: "Result".into(),
            text: result,
        })
        .change(PlanChange::Text {
            label: "Git hooks".into(),
            text: hooks,
        })
        .target(
            "source",
            &serde_json::json!({
                "machine": source.machine,
                "project": source.project,
                "commit": read.commit,
                "commits": read.commits,
            }),
        )
        .target(
            "target",
            &serde_json::json!({
                "machine": target.machine,
                "project": target.project,
                "branchTip": read.tip,
                "plan": read.target_plan,
            }),
        )
        .rules(PlanRules {
            auto_approve,
            remember: Remember::Tool,
            policy_auto: false,
        })
        .build()
}
