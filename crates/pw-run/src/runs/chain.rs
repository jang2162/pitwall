//! 사슬 실행 — `before` 와 복합 구성(17-m6-plan M6-21 — 결정 M6-D5 · D67 · D68, 메인 판정 2026-11-19). 사슬의 꼴(단계 · 차례)은
//! `config::graph` 가 펴고, 이 모듈이 실행 관리자의 실행으로 띄운다.
//!
//! # 차례
//!
//! - `before` 와 복합 구성의 `sequence` 는 차례로 — 앞 단계가 **끝 코드 0 으로 끝나야** 다음을 띄운다. 실패(0 이 아닌 끝 코드 ·
//!   신호 · 띄우기 실패)하면 거기서 멈추고 남은 단계를 띄우지 않는다. `parallel` 은 함께 띄운다. 한 갈래가 실패해도 이미 도는
//!   실행은 그대로 두고, 아직 띄우지 않은 것은 어느 갈래든 띄우지 않는다.
//! - 마지막 단계(본 구성)는 띄우고 끝을 기다리지 않는다 — 서버가 그렇다.
//!
//! # 띄우는 것은 확인 · 승인 때 푼 내용뿐
//!
//! 단계마다 띄울 것은 사슬을 시작하기 전에 모두 푼다 — 프로세스는 argv · 작업 폴더 · env(`.env` 는 그때 읽은 값), `sql` 은 그때
//! 읽어 분류한 파일 글. 사람(창의 첫 실행 확인 · `sql` 위험 확인)과 에이전트(승인 계획)가 본 것이 그것이고, **앞 단계가 도는 사이
//! 뒤 단계의 구성 · `.env` · 명령 정의 · 런타임 원천 파일 · SQL 파일이 바뀌면 그 단계 앞에서 멈춘다**(`planChanged` — 다시 읽어
//! 돌리지 않는다, fail-closed). 그 판정이 [`ChainStepRun::recheck`] 다: 이미 무엇이 돈 뒤에 띄우는 단계는 그 자리에서 지금 내용을
//! 다시 풀어 확인 · 승인 때의 내용 해시와 견준다. 대가: 코드 생성 뒤 마이그레이션을 돌리는 사슬은 그 단계 앞에서 끊긴다(17-m6-plan
//! M6-21 ⑤).
//!
//! # 중지
//!
//! 사슬이 **아직 띄우는 중이면** 그 사슬의 어느 실행을 멈춰도(사람의 `run.stop` · 에이전트의 `run_stop` · 창 닫기) 사슬 전체가
//! 멈춘다 — 도는 실행을 모두 멈추고 남은 단계를 띄우지 않는다. 다 띄운 뒤에는 실행마다 따로다(복합 구성의 한 서버만 다시 띄울 수
//! 있게). 에이전트의 호출이 버려지면(MCP 취소 · 끊김 — 되돌릴 수 없는 지점 앞) 사슬도 멈춘다. 되돌릴 수 없는 지점(`sql` 단계의
//! 커밋) 뒤의 취소는 호출을 버리지 않으므로, 다음 단계를 띄우기 전에 취소를 보고 남은 단계를 띄우지 않는다.
//!
//! # 보이기
//!
//! 사슬의 진행 · 끝 · 실패 단계는 프로젝트 이벤트 [`TOPIC_CHAIN`]([`RunChainEvent`])으로 낸다 — 창은 그것으로 「사슬이 `seed` 에서
//! 멈췄다」 를 보인다.
//!
//! # 창의 길
//!
//! [`Runs::start_window`]: `run.start` 의 구성이 사슬이면 모든 확인을 **첫 단계를 띄우기 전에** 받는다 — 프로세스 단계와 복합
//! 구성이 모두 「사람이 본 구성」 이 아니면 첫 실행 확인(`7001` — `steps` 에 모든 단계), 그 뒤 위험 확인이 필요한 `sql` 단계마다
//! `3001`(`data.config` 에 그 구성 열쇠). 창은 받은 확인을 `confirmed` · `sqlConfirmed`(구성 열쇠 → 확인)에 실어 다시 부른다. 모든
//! 확인을 지나면 `sql` 확인을 모두 쓰고(한 번만 쓰인다 — 다른 단계 · 다른 사슬 · 다시 실은 호출에는 쓰이지 않는다), 「사람이 본
//! 구성」 을 기록하고, 사슬을 Host 의 백그라운드 태스크로 띄운다. `run.start` 는 처음 띄운 실행의 [`RunInfo`] 로 답한다.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::Poll;

use pw_core::registry::exec::ExecGrant;
use pw_core::registry::run_seen::{SeenBy, SeenHash};
use pw_rpc::{ConnectionScope, RpcError, code};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{
    LaunchedBy, NewRun, Relaunch, Relaunched, Run, RunExit, RunId, RunInfo, RunStartParams, Runs,
    confirm, lock, size_of, stop_run,
};
use crate::config::graph::{self, Chain, Node};
use crate::config::{RunConfig, RunKindConfig};

/// 사슬 이벤트의 토픽(프로젝트 범위). payload 는 [`RunChainEvent`].
pub const TOPIC_CHAIN: &str = "run.chain";
pw_rpc::rpc_event!(TOPIC_CHAIN, roles: [Owner, Mobile, Readonly]);

/// 사슬의 상태.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RunChainState {
    /// 단계를 띄우는 중(앞 단계의 끝을 기다리는 중 포함).
    Running,
    /// 모든 단계를 띄웠다.
    Done,
    /// 한 단계가 실패했다 — 남은 단계는 띄우지 않았다.
    Failed,
    /// 멈췄다(사슬의 실행을 멈춤 · 호출 취소).
    Stopped,
}

/// 이벤트 `run.chain`(머리 주석 「보이기」).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunChainEvent {
    pub chain_id: String,
    pub project_id: String,
    /// 사슬을 띄운 구성(맨 위 구성 열쇠).
    pub config: String,
    pub state: RunChainState,
    /// 지금까지 띄운 실행(띄운 차례).
    pub runs: Vec<RunId>,
    /// 실패한 · 멈춘 단계의 구성 열쇠.
    pub step: Option<String>,
    /// 실패한 까닭(영어 한 줄 — 값 없음).
    pub message: Option<String>,
    pub launched_by: LaunchedBy,
}

pw_rpc::rpc_type!(RunChainEvent);

/// 보낼 수 있는 future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// 사슬 하나의 제어(실행마다 쥔다 — `Run::chain`). 머리 주석 「중지」.
pub struct ChainCtl {
    state: Mutex<CtlState>,
}

#[derive(Default)]
struct CtlState {
    /// 다 띄웠다(또는 실패 · 멈춤으로 끝났다) — 그 뒤의 중지는 실행마다 따로다.
    finished: bool,
    stopped: bool,
    runs: Vec<Weak<Run>>,
}

impl ChainCtl {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::default(),
        })
    }

    /// 띄우는 중이면 사슬 전체를 멈춘다: 도는 실행을 모두 멈추고, 남은 단계를 띄우지 않는다.
    pub(crate) fn stop(&self) {
        let runs: Vec<Arc<Run>> = {
            let mut state = lock(&self.state);
            if state.finished || state.stopped {
                return;
            }
            state.stopped = true;
            state.runs.iter().filter_map(Weak::upgrade).collect()
        };
        tracing::info!(target: "pw_run", runs = runs.len(), "stopping the whole chain");
        for run in runs {
            stop_run(&run);
        }
    }

    fn stopped(&self) -> bool {
        lock(&self.state).stopped
    }

    /// 띄운 실행을 사슬에 넣는다. 이미 멈춘 사슬이면 거짓(부른 쪽이 그 실행을 멈춘다).
    fn attach(self: &Arc<Self>, run: &Arc<Run>) -> bool {
        {
            let mut state = lock(&self.state);
            if state.stopped {
                return false;
            }
            if !state
                .runs
                .iter()
                .any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(run)))
            {
                state.runs.push(Arc::downgrade(run));
            }
        }
        *lock(&run.chain) = Some(Arc::clone(self));
        true
    }

    fn finish(&self) {
        lock(&self.state).finished = true;
    }
}

/// 버려지면(에이전트의 호출이 취소 · 끊김) 띄우는 중인 사슬을 멈춘다.
pub(crate) struct StopChainOnDrop(pub Arc<ChainCtl>);

impl Drop for StopChainOnDrop {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// 띄울 단계 하나(머리 주석 「띄우는 것은 확인 · 승인 때 푼 내용뿐」).
pub(crate) struct ChainStepRun {
    pub key: String,
    /// 확인 · 승인 때 푼 내용으로 띄운다. 인자가 참이면(이미 무엇이 돈 뒤) 먼저 지금 내용을 다시 풀어 확인 · 승인 때와 견주고,
    /// 다르면 띄우지 않고 [`plan_changed`].
    ///
    /// 띄운 실행은 그 자리에서(블로킹 작업 안에서도) [`attach_or_stop`] 으로 사슬에 넣는다 — 호출이 버려진 뒤 끝난 띄우기도 멈춘
    /// 사슬을 보고 그 실행을 멈춘다(리뷰 F4).
    pub launch: StepLaunch,
}

/// [`ChainStepRun::launch`] — (다시 볼까, 사슬) → 띄운 실행.
pub(crate) type StepLaunch =
    Box<dyn FnOnce(bool, Arc<ChainCtl>) -> BoxFuture<Result<RunInfo, RpcError>> + Send>;

/// 띄운 실행을 사슬에 넣는다. 사슬이 이미 멈췄으면 그 실행을 멈추고 `chainStopped`.
pub(crate) fn attach_or_stop(
    runs: &Runs,
    project_id: &str,
    key: &str,
    info: RunInfo,
    ctl: &Arc<ChainCtl>,
) -> Result<RunInfo, RpcError> {
    let run = runs.find(project_id, &info.run_id)?;
    if ctl.attach(&run) {
        Ok(info)
    } else {
        stop_run(&run);
        Err(chain_stopped(key))
    }
}

/// 시험이 단계 띄우기 안(재확인 뒤 · 띄우기 앞, 블로킹 작업 안)에 끼우는 자리 — 구성 열쇠를 받는다.
type LaunchHook = Arc<dyn Fn(&str) + Send + Sync>;
static LAUNCH_HOOK: Mutex<Option<LaunchHook>> = Mutex::new(None);

/// 시험 전용: 에이전트 사슬의 프로세스 단계를 띄우기 직전에 부를 함수(nextest 는 시험마다 프로세스가 따로다).
#[doc(hidden)]
pub fn set_launch_hook(hook: Option<LaunchHook>) {
    *lock(&LAUNCH_HOOK) = hook;
}

pub(crate) fn launch_hook(key: &str) {
    let hook = lock(&LAUNCH_HOOK).clone();
    if let Some(hook) = hook {
        hook(key);
    }
}

/// 앞 단계가 도는 사이 단계의 내용이 바뀌었다(`PERMISSION_DENIED`, `data.reason = "planChanged"`) — 그 단계는 돌지 않았다.
pub fn plan_changed(key: &str) -> RpcError {
    RpcError::new(
        code::PERMISSION_DENIED,
        format!(
            "run config `{key}` changed while an earlier step of the chain ran (its file, .env files, command definition, \
             runtime files or SQL files); the chain stopped before it — start it again to see and approve the new content"
        ),
    )
    .with_data(json!({ "reason": "planChanged", "config": key }))
}

/// 단계가 끝 코드 0 으로 끝나지 않았다(`CONFLICT`, `data.reason = "stepFailed"`).
fn step_failed(key: &str, exit: &RunExit) -> RpcError {
    let how = match (&exit.code, &exit.signal) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("ended by a signal ({signal})"),
        (None, None) => "ended without an exit code".to_owned(),
    };
    RpcError::new(
        code::CONFLICT,
        format!("run config `{key}` {how}; the chain stopped there and did not start the rest"),
    )
    .with_data(json!({ "reason": "stepFailed", "config": key, "exit": exit }))
}

/// 사슬이 멈춰 그 단계를 띄우지 않았다(`CONFLICT`, `data.reason = "chainStopped"`). `CANCELLED` 가 아니다 — 그 코드면 MCP 세션이
/// 감사 상세를 「취소」 로 덮어 앞 단계(커밋한 `sql` 단계)의 상세가 사라진다.
fn chain_stopped(key: &str) -> RpcError {
    RpcError::new(
        code::CONFLICT,
        format!("the chain was stopped before run config `{key}` started"),
    )
    .with_data(json!({ "reason": "chainStopped", "config": key }))
}

/// 사슬 하나의 실행기.
pub(crate) struct ChainExec {
    runs: Runs,
    id: String,
    project_id: String,
    key: String,
    launched_by: LaunchedBy,
    ctl: Arc<ChainCtl>,
    steps: Mutex<Vec<Option<ChainStepRun>>>,
    /// 한 단계라도 띄웠다 — 그 뒤의 단계는 띄우기 전에 다시 본다.
    started: AtomicBool,
    /// 한 갈래가 실패했다 — 다른 갈래도 남은 것을 띄우지 않는다.
    halted: AtomicBool,
    launched: Mutex<Vec<RunInfo>>,
    first: Mutex<Option<oneshot::Sender<Result<RunInfo, RpcError>>>>,
    /// 에이전트 호출의 취소(되돌릴 수 없는 지점 뒤에도 남은 단계를 띄우지 않게).
    cancel: Option<CancellationToken>,
    failed: Mutex<Option<(String, String)>>,
    /// 처음 실패한 단계의 오류(다른 갈래가 그 뒤에 받은 `chainStopped` 가 아니라 — 리뷰 F2).
    first_error: Mutex<Option<RpcError>>,
}

impl ChainExec {
    pub(crate) fn new(
        runs: &Runs,
        project_id: &str,
        key: &str,
        launched_by: LaunchedBy,
        steps: Vec<ChainStepRun>,
        cancel: Option<CancellationToken>,
    ) -> (Arc<Self>, oneshot::Receiver<Result<RunInfo, RpcError>>) {
        let (tx, rx) = oneshot::channel();
        let id = format!(
            "{}-c{}",
            runs.inner.nonce,
            runs.inner.next.fetch_add(1, Ordering::Relaxed)
        );
        let exec = Arc::new(Self {
            runs: runs.clone(),
            id,
            project_id: project_id.to_owned(),
            key: key.to_owned(),
            launched_by,
            ctl: ChainCtl::new(),
            steps: Mutex::new(steps.into_iter().map(Some).collect()),
            started: AtomicBool::new(false),
            halted: AtomicBool::new(false),
            launched: Mutex::default(),
            first: Mutex::new(Some(tx)),
            cancel,
            failed: Mutex::default(),
            first_error: Mutex::default(),
        });
        (exec, rx)
    }

    pub(crate) fn ctl(&self) -> Arc<ChainCtl> {
        Arc::clone(&self.ctl)
    }

    /// 사슬을 끝까지(모든 단계를 띄우거나 실패할 때까지) 돌린다. 띄운 실행들(띄운 차례).
    pub(crate) async fn run(self: Arc<Self>, root: Node) -> Result<Vec<RunInfo>, RpcError> {
        self.run_with(root, false).await
    }

    /// [`Self::run`] 인데 `wait_all` 이면 마지막 단계의 끝까지 기다린다(디버그 시작 앞의 `before` 사슬).
    pub(crate) async fn run_with(
        self: Arc<Self>,
        root: Node,
        wait_all: bool,
    ) -> Result<Vec<RunInfo>, RpcError> {
        self.publish(RunChainState::Running);
        let result = Arc::clone(&self).node(root, wait_all).await;
        // 병렬 갈래의 결과는 목록 차례로 모인다 — 사슬이 알릴 것은 처음 실패한 단계다.
        let result = match (result, lock(&self.first_error).take()) {
            (Err(_), Some(first)) => Err(first),
            (result, _) => result,
        };
        self.ctl.finish();
        let state = match &result {
            Ok(()) => RunChainState::Done,
            Err(_) if self.ctl.stopped() => RunChainState::Stopped,
            Err(_) => RunChainState::Failed,
        };
        if let Err(e) = &result {
            lock(&self.failed).get_or_insert_with(|| {
                let step = e
                    .data
                    .as_ref()
                    .and_then(|d| d.get("config"))
                    .and_then(Value::as_str)
                    .unwrap_or(&self.key)
                    .to_owned();
                (step, e.message.clone())
            });
        }
        self.publish(state);
        tracing::info!(target: "pw_run", chain = %self.id, config = %self.key, ?state, "chain ended");
        if let Some(tx) = lock(&self.first).take() {
            let _ = tx.send(match &result {
                Ok(()) => Err(RpcError::internal("the chain started nothing")),
                Err(e) => Err(e.clone()),
            });
        }
        result.map(|()| lock(&self.launched).clone())
    }

    fn node(self: Arc<Self>, node: Node, wait: bool) -> BoxFuture<Result<(), RpcError>> {
        Box::pin(async move {
            match node {
                Node::Step(i) => self.step(i, wait).await,
                Node::Sequence(items) => {
                    let last = items.len().saturating_sub(1);
                    for (j, item) in items.into_iter().enumerate() {
                        Arc::clone(&self).node(item, wait || j < last).await?;
                    }
                    Ok(())
                }
                Node::Parallel(items) => {
                    let branches: Vec<BoxFuture<Result<(), RpcError>>> = items
                        .into_iter()
                        .map(|item| Arc::clone(&self).node(item, wait))
                        .collect();
                    join_all(branches).await.into_iter().collect()
                }
            }
        })
    }

    async fn step(self: Arc<Self>, i: usize, wait: bool) -> Result<(), RpcError> {
        let step = lock(&self.steps)[i]
            .take()
            .ok_or_else(|| RpcError::internal(format!("chain step {i} ran twice")))?;
        let result = self.launch_step(step, wait).await;
        if let Err(e) = &result {
            self.halted.store(true, Ordering::SeqCst);
            let follow_up = e
                .data
                .as_ref()
                .and_then(|d| d.get("reason"))
                .is_some_and(|r| r == "chainStopped")
                && !self.ctl.stopped();
            if !follow_up {
                lock(&self.first_error).get_or_insert_with(|| e.clone());
            }
        }
        result
    }

    async fn launch_step(&self, step: ChainStepRun, wait: bool) -> Result<(), RpcError> {
        let key = step.key.clone();
        if self.ctl.stopped()
            || self.halted.load(Ordering::SeqCst)
            || self
                .cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(chain_stopped(&key));
        }
        let recheck = self.started.swap(true, Ordering::SeqCst);
        let info = (step.launch)(recheck, Arc::clone(&self.ctl)).await?;
        let info = attach_or_stop(&self.runs, &self.project_id, &key, info, &self.ctl)?;
        let run = self.runs.find(&self.project_id, &info.run_id)?;
        lock(&self.launched).push(info.clone());
        if let Some(tx) = lock(&self.first).take() {
            let _ = tx.send(Ok(info));
        }
        self.publish(RunChainState::Running);
        if wait {
            // 에이전트 호출의 취소(되돌릴 수 없는 지점 뒤에도 — 리뷰 R2)는 기다리던 단계까지 사슬 전체를 멈춘다.
            let exit = match &self.cancel {
                Some(cancel) => tokio::select! {
                    exit = run.exited() => exit,
                    () = cancel.cancelled() => {
                        self.ctl.stop();
                        return Err(chain_stopped(&key));
                    }
                },
                None => run.exited().await,
            };
            if exit.code != Some(0) {
                return Err(if self.ctl.stopped() {
                    chain_stopped(&key)
                } else {
                    step_failed(&key, &exit)
                });
            }
        }
        Ok(())
    }

    fn publish(&self, state: RunChainState) {
        let (step, message) = lock(&self.failed).clone().unzip();
        let event = RunChainEvent {
            chain_id: self.id.clone(),
            project_id: self.project_id.clone(),
            config: self.key.clone(),
            state,
            runs: lock(&self.launched)
                .iter()
                .map(|r| r.run_id.clone())
                .collect(),
            step,
            message,
            launched_by: self.launched_by.clone(),
        };
        self.runs
            .inner
            .publish(&self.project_id, TOPIC_CHAIN, &event);
    }
}

/// 모두 끝날 때까지 함께 돌린다(future 를 버리면 모두 버려진다 — 띄운 태스크가 남지 않는다).
async fn join_all<T>(mut futures: Vec<BoxFuture<T>>) -> Vec<T> {
    let mut out: Vec<Option<T>> = futures.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (slot, future) in out.iter_mut().zip(futures.iter_mut()) {
            if slot.is_none() {
                match future.as_mut().poll(cx) {
                    Poll::Ready(value) => *slot = Some(value),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    out.into_iter()
        .map(|v| v.expect("every branch finished"))
        .collect()
}

/// 내용 해시 여럿을 하나로(사슬 전체의 확인 · 승인 열쇠).
pub(crate) fn combined(kind: &str, parts: &[(String, String)]) -> String {
    let text = json!({ "v": kind, "parts": parts }).to_string();
    format!(
        "sha256:{}",
        crate::env::hex(&Sha256::digest(text.as_bytes()))
    )
}

// --- 창의 길 --------------------------------------------------------------------------------------------------------

/// 창의 `sql` 단계(pw-db 가 [`Runs::attach_sql_window`] 로 붙인다 — pw-run 은 pw-db 에 의존하지 않는다). `run.sql.start` 와 같은
/// 관문 · 읽기 · 분류 · 위험 확인(`judge_window`)을 하되, **확인을 쓰지 않는다** — 쓰기는 사슬의 모든 확인을 지난 뒤
/// [`SqlWindowStep::consume`] 이다.
pub trait SqlWindowChain: Send + Sync + 'static {
    /// 위험 확인이 필요하면 그 `3001` 오류(쓰지 않은 확인은 그대로 남는다).
    fn prepare(
        &self,
        request: SqlWindowRequest,
    ) -> BoxFuture<Result<Box<dyn SqlWindowStep>, RpcError>>;
}

/// [`SqlWindowChain::prepare`] 가 받는 것.
pub struct SqlWindowRequest {
    /// 부른 연결의 범위(그 프로젝트 창).
    pub scope: ConnectionScope,
    pub project_id: String,
    pub worktree: Option<String>,
    pub key: String,
    pub config: RunConfig,
    /// 창이 실은 그 구성의 위험 확인(`DbConfirmed` 의 JSON).
    pub confirmed: Option<Value>,
}

/// 판정을 지난 창의 `sql` 단계.
pub trait SqlWindowStep: Send {
    /// 쓴 확인을 쓴다(한 번만 — 그사이 다른 호출이 썼으면 오류).
    fn consume(&self) -> Result<(), RpcError>;
    /// 확인 때 읽은 글로 띄운다. `recheck` 면 먼저 파일 · 연결 설정을 다시 읽어 확인 때와 같은지 본다(다르면 [`plan_changed`]).
    fn launch(
        self: Box<Self>,
        runs: Runs,
        launched_by: LaunchedBy,
        recheck: bool,
    ) -> BoxFuture<Result<RunInfo, RpcError>>;
}

impl Runs {
    /// 창의 `sql` 사슬 단계(pw-db 가 붙인다). 두 번째부터는 무시한다.
    pub fn attach_sql_window(&self, chain: Arc<dyn SqlWindowChain>) {
        let _ = self.inner.sql_window.set(chain);
    }

    /// 창의 `run.start`(머리 주석 「창의 길」): 구성이 사슬이 아니면 [`Runs::start`] 그대로.
    pub async fn start_window(
        &self,
        grant: ExecGrant,
        scope: ConnectionScope,
        seen: Option<SeenBy>,
        window: Option<String>,
        params: RunStartParams,
    ) -> Result<RunInfo, RpcError> {
        let chain = {
            let (runs, project, worktree, id) = (
                self.clone(),
                grant.project_id().to_owned(),
                params.worktree.clone(),
                params.config_id.clone(),
            );
            blocking(move || runs.chain_of(&project, worktree.as_deref(), &id)).await?
        };
        let Some(chain) = chain else {
            let runs = self.clone();
            return blocking(move || runs.start(&grant, seen, window, &params)).await;
        };
        self.start_window_chain(grant, scope, seen, window, params, chain, None)
            .await
    }

    /// 창의 `run.restart`: 구성으로 띄운 실행의 구성이 지금 사슬이면(`before` 가 있다) 사슬을 다시 띄운다 — 모든 확인을 받은 뒤
    /// 앞 실행을 멈추고, 그 구성의 단계가 `restartOf` 를 받는다. 아니면 [`Runs::restart_confirmed`] 그대로.
    pub async fn restart_window(
        &self,
        grant: ExecGrant,
        scope: ConnectionScope,
        seen: Option<SeenBy>,
        window: Option<String>,
        params: super::methods::RunRestartParams,
    ) -> Result<RunInfo, RpcError> {
        let run = self.find(grant.project_id(), &params.run_id)?;
        if seen.is_some()
            && let Relaunch::Config {
                config_id,
                profile,
                worktree,
            } = &run.relaunch
        {
            let chain = {
                let (runs, project, worktree, id) = (
                    self.clone(),
                    run.project_id().to_owned(),
                    worktree.clone(),
                    config_id.clone(),
                );
                blocking(move || runs.chain_of(&project, worktree.as_deref(), &id)).await?
            };
            if let Some(chain) = chain {
                let size = *lock(&run.size);
                let start = RunStartParams {
                    project_id: run.project_id().to_owned(),
                    worktree: worktree.clone(),
                    config_id: config_id.clone(),
                    profile: profile.clone(),
                    cols: Some(size.0),
                    rows: Some(size.1),
                    confirmed: params.confirmed.clone(),
                    sql_confirmed: params.sql_confirmed.clone(),
                };
                return self
                    .start_window_chain(grant, scope, seen, window, start, chain, Some(run))
                    .await;
            }
        }
        self.restart_confirmed(
            &grant,
            seen,
            window,
            &params.run_id,
            params.confirmed.as_deref(),
        )
        .await
    }

    /// 그 구성의 사슬(사슬이 아니면 `None`). 미신뢰 프로젝트는 구성을 읽기 전에 거절한다.
    pub(crate) fn chain_of(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        config_id: &str,
    ) -> Result<Option<Chain>, RpcError> {
        let (_, place) = self.config_place(project_id, worktree)?;
        let set = crate::config::read(&place);
        let (key, config) = super::config_of(&place, config_id)?;
        if !graph::is_chain(&config) {
            return Ok(None);
        }
        let configs: BTreeMap<&str, &RunConfig> = set
            .configs
            .iter()
            .map(|c| (c.id.as_str(), &c.config))
            .collect();
        let unusable = |k: &str| set.unusable.iter().any(|u| u.id == k);
        graph::expand(&|k| configs.get(k).copied(), &unusable, &key)
            .map(Some)
            .map_err(|why| {
                RpcError::invalid_params(format!("run config `{key}` cannot be used: {why}"))
            })
    }

    #[allow(clippy::too_many_arguments)] // 창의 띄우기 하나의 재료 — 다시 실행(`restart_of`)이 하나 더한다.
    pub(crate) async fn start_window_chain(
        &self,
        grant: ExecGrant,
        scope: ConnectionScope,
        seen: Option<SeenBy>,
        window: Option<String>,
        params: RunStartParams,
        chain: Chain,
        restart: Option<Arc<Run>>,
    ) -> Result<RunInfo, RpcError> {
        self.window_chain(
            grant.project_id().to_owned(),
            scope,
            seen,
            window,
            params,
            chain,
            restart,
            false,
        )
        .await?
        .ok_or_else(|| RpcError::internal("the chain started nothing"))
    }

    /// 창의 사슬(머리 주석 「창의 길」). `before_only` 면 끝까지 기다리고 `None`.
    #[allow(clippy::too_many_arguments)]
    async fn window_chain(
        &self,
        project_id: String,
        scope: ConnectionScope,
        seen: Option<SeenBy>,
        window: Option<String>,
        params: RunStartParams,
        chain: Chain,
        restart: Option<Arc<Run>>,
        before_only: bool,
    ) -> Result<Option<RunInfo>, RpcError> {
        let restart_of = restart
            .as_ref()
            .map(|run| (run.config().to_owned(), run.id().to_owned()));
        let size = size_of(params.cols, params.rows)?;
        if params.profile.is_some() && chain_top(&chain).is_none() {
            return Err(RpcError::invalid_params(format!(
                "run config `{}` is a compound configuration; profiles apply to one configuration",
                chain.key
            )));
        }
        let host = self.inner.host()?.clone();
        // 1. 프로세스 단계를 모두 푼다(블로킹 — `.env` · 런타임 환경 · 정의 본문).
        let process = {
            let (runs, chain, project, params) = (
                self.clone(),
                chain.clone(),
                project_id.clone(),
                params.clone(),
            );
            blocking(move || {
                let mut out = BTreeMap::new();
                for (i, step) in chain.steps.iter().enumerate() {
                    if matches!(step.config.kind, RunKindConfig::Sql { .. }) {
                        continue;
                    }
                    let what = window_step(&params, step, size);
                    out.insert(i, runs.prepare_config(&project, &what, false)?);
                }
                Ok(out)
            })
            .await?
        };
        // 2. 첫 실행 확인 — 프로세스 단계와 복합 구성이 모두 「사람이 본 구성」 이 아니면 모든 단계를 보이고 묻는다.
        let compounds: Vec<(String, SeenHash)> = chain
            .compounds
            .iter()
            .map(|(k, c)| (k.clone(), crate::seen::compound_hash(k, c)))
            .collect();
        let saw = process.values().all(|p| p.saw)
            && compounds
                .iter()
                .all(|(k, h)| host.projects.human_saw(&project_id, k, h));
        let mut tokens: Vec<(String, String)> = chain
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let token = process
                    .get(&i)
                    .map_or_else(String::new, |p| p.token.clone());
                (s.key.clone(), token)
            })
            .collect();
        tokens.extend(compounds.iter().map(|(k, h)| (k.clone(), h.to_string())));
        tokens.push(("order".to_owned(), chain.order_text()));
        let token = combined("pitwall.run.chain.confirm.v1", &tokens);
        if !saw && params.confirmed.as_deref() != Some(token.as_str()) {
            let steps: Vec<confirm::RunShown> = chain
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| match process.get(&i) {
                    Some(p) => p.shown.clone(),
                    None => sql_shown(&s.key, &s.config),
                })
                .collect();
            let mut main = chain
                .steps
                .iter()
                .enumerate()
                .rev()
                .find_map(|(i, _)| process.get(&i))
                .map_or_else(|| steps[steps.len() - 1].clone(), |p| p.shown.clone());
            // 맨 위 경고는 사슬 전체를 본다 — 앞 단계에만 비밀 같은 값이 있어도 뜬다(메인 판정 2026-11-21).
            main.secret_looking = steps.iter().any(|s| s.secret_looking);
            return Err(confirm::confirm_required_with(
                confirm::RunConfirmRequired {
                    hash: token,
                    shown: main,
                    steps,
                    order: Some(chain.order_text()),
                    title: Some(chain.name.clone()),
                },
            ));
        }
        // 3. `sql` 단계의 위험 확인 — 모두 판정만 하고(쓰지 않는다) 하나라도 필요하면 그 `3001`.
        let mut sql_steps: BTreeMap<usize, Box<dyn SqlWindowStep>> = BTreeMap::new();
        for (i, step) in chain.steps.iter().enumerate() {
            if !matches!(step.config.kind, RunKindConfig::Sql { .. }) {
                continue;
            }
            let Some(sql) = self.inner.sql_window.get().cloned() else {
                return Err(RpcError::invalid_params(
                    "sql configurations cannot run in this Pitwall host",
                ));
            };
            let confirmed = params
                .sql_confirmed
                .as_ref()
                .and_then(|m| m.get(&step.key))
                .cloned();
            let prepared = sql
                .prepare(SqlWindowRequest {
                    scope: scope.clone(),
                    project_id: project_id.clone(),
                    worktree: params.worktree.clone(),
                    key: step.key.clone(),
                    config: step.config.clone(),
                    confirmed,
                })
                .await
                .map_err(|mut e| {
                    if let Some(Value::Object(data)) = e.data.as_mut() {
                        data.insert("config".to_owned(), Value::String(step.key.clone()));
                    }
                    e
                })?;
            sql_steps.insert(i, prepared);
        }
        // 4. 모든 확인을 지났다 — `sql` 확인을 쓰고, 본 구성을 기록하고, (다시 실행이면 앞 실행이 끝난 것을 본 뒤) 띄운다.
        for step in sql_steps.values() {
            step.consume()?;
        }
        if let Some(run) = &restart {
            stop_run(run);
            run.exited().await;
        }
        if let Some(by) = &seen {
            let mut records: Vec<(String, SeenHash)> = chain
                .steps
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    process
                        .get(&i)
                        .and_then(|p| p.seen_hash.clone().ok())
                        .map(|h| (s.key.clone(), h))
                })
                .collect();
            records.extend(compounds.iter().cloned());
            if let Err(e) = host.projects.record_run_seen(by, &records) {
                tracing::debug!(target: "pw_run", error = %e, "the chain was not recorded as seen");
            }
        }
        let launched_by = LaunchedBy::Window { window };
        let mut process = process;
        let mut runs_of: Vec<ChainStepRun> = Vec::with_capacity(chain.steps.len());
        for (i, step) in chain.steps.iter().enumerate() {
            let key = step.key.clone();
            let restart = restart_of
                .as_ref()
                .filter(|(k, _)| *k == key)
                .map(|(_, id)| id.clone());
            if let Some(prepared) = sql_steps.remove(&i) {
                let runs = self.clone();
                let by = launched_by.clone();
                runs_of.push(ChainStepRun {
                    key,
                    launch: Box::new(move |recheck, _ctl| prepared.launch(runs, by, recheck)),
                });
                continue;
            }
            let ready = process
                .remove(&i)
                .ok_or_else(|| RpcError::internal("a chain step was not prepared"))?;
            let what = window_step(&params, step, size);
            let (runs, project, by) = (self.clone(), project_id.clone(), launched_by.clone());
            runs_of.push(ChainStepRun {
                key,
                launch: Box::new(move |recheck, ctl| {
                    Box::pin(async move {
                        blocking(move || {
                            if recheck {
                                // 앞 단계가 도는 사이 바뀌었으면 띄우지 않는다(머리 주석) — 다시 푼 것은 견주기만 한다.
                                match runs.prepare_config(&project, &what, false) {
                                    Ok(now) if now.token == ready.token => {}
                                    _ => return Err(plan_changed(&ready.key)),
                                }
                            }
                            let new = NewRun {
                                project_id: project.clone(),
                                config: ready.key.clone(),
                                name: ready.name.clone(),
                                launched_by: by,
                                worktree: what.worktree.clone(),
                                cwd: Some(ready.cwd.clone()),
                                profile: what.profile.clone(),
                                relaunch: Relaunch::Config {
                                    config_id: what.config_id.clone(),
                                    profile: what.profile.clone(),
                                    worktree: what.worktree.clone(),
                                },
                                size: what.size,
                                restart_of: restart,
                            };
                            let info = runs.start_process(new, ready.argv, ready.env)?;
                            attach_or_stop(&runs, &project, &ready.key, info, &ctl)
                        })
                        .await
                    })
                }),
            });
        }
        let (exec, first) =
            ChainExec::new(self, &project_id, &chain.key, launched_by, runs_of, None);
        tracing::info!(target: "pw_run", chain = %exec.id, config = %chain.key, steps = chain.steps.len(), before_only, "starting a chain from the window");
        if before_only {
            // 디버그 시작 앞: 끝까지 기다린다. 부른 쪽(창의 RPC)이 버려지면 사슬도 멈춘다.
            let _guard = StopChainOnDrop(exec.ctl());
            exec.run_with(chain.root, true).await?;
            return Ok(None);
        }
        tokio::spawn(exec.run(chain.root));
        first
            .await
            .map_err(|_| RpcError::internal("the chain ended without starting a run"))?
            .map(Some)
    }

    /// 디버그 시작(`debug.start` — M6-18)의 앞: 구성에 `before` 가 있으면 그 사슬을 창의 길과 같은 확인(7001 · 3001)을 받아 끝까지
    /// 돌린다(모두 끝 코드 0). 맨 위 구성은 띄우지 않는다 — 부른 쪽이 디버그로 띄운다. `before` 가 없으면 아무것도 하지 않는다.
    #[allow(clippy::too_many_arguments)] // `start_window` 와 같은 재료.
    pub async fn run_before_window(
        &self,
        project_id: &str,
        scope: ConnectionScope,
        seen: Option<SeenBy>,
        window: Option<String>,
        params: RunStartParams,
    ) -> Result<(), RpcError> {
        let chain = {
            let (runs, project, worktree, id) = (
                self.clone(),
                project_id.to_owned(),
                params.worktree.clone(),
                params.config_id.clone(),
            );
            blocking(move || runs.chain_of(&project, worktree.as_deref(), &id)).await?
        };
        let Some(before) = chain.and_then(|c| c.before_part()) else {
            return Ok(());
        };
        self.window_chain(
            project_id.to_owned(),
            scope,
            seen,
            window,
            params,
            before,
            None,
            true,
        )
        .await
        .map(|_| ())
    }
}

/// 사슬의 맨 위 프로세스 단계(복합 구성이 맨 위면 없다).
fn chain_top(chain: &Chain) -> Option<&graph::ChainStep> {
    chain.steps.iter().find(|s| s.top)
}

/// 창의 단계 하나의 풀기 재료 — 프로필은 맨 위 구성에만.
fn window_step(params: &RunStartParams, step: &graph::ChainStep, size: (u16, u16)) -> Relaunched {
    Relaunched {
        config_id: step.key.clone(),
        profile: if step.top {
            params.profile.clone()
        } else {
            None
        },
        worktree: params.worktree.clone(),
        size,
        restart_of: None,
        seen: None,
        confirmed: None,
    }
}

/// 첫 실행 확인에 보이는 `sql` 단계(문장은 뒤의 위험 확인이 보인다).
fn sql_shown(key: &str, config: &RunConfig) -> confirm::RunShown {
    let definition = match &config.kind {
        RunKindConfig::Sql {
            connection,
            files,
            transaction,
        } => Some(format!(
            "connection: {connection}\nfiles: {}\ntransaction: {}",
            files.join(", "),
            transaction.name()
        )),
        _ => None,
    };
    confirm::RunShown {
        key: key.to_owned(),
        name: config.name.clone(),
        kind: config.kind.type_name().to_owned(),
        argv: Vec::new(),
        cwd: ".".to_owned(),
        profile: None,
        env: Vec::new(),
        env_files: Vec::new(),
        definition,
        definition_source: None,
        secret_looking: false,
        recordable: false,
    }
}

/// 블로킹 스레드에서 돌린다.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}
