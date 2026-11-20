//! Host 의 디버그(03 「코어 모듈」 의 `dap` — 17-m6-plan M6-17): 디버그 세션 · 브레이크포인트 저장 · `debug.*` RPC. DAP 클라이언트
//! (`pw-dap`, M6-15) 위에서 실행 구성(M6-01)의 `[debug]` 를 어댑터 정의(M6-16)로 옮기고, 세션을 실행 관리자(M6-02)의 실행 하나로
//! 둔다(메인 판정 M6-P15 — [`run`]).
//!
//! # 시작(`debug.start`)
//!
//! 1. **허가**: `pw_dap::debug_grant` — 그 프로젝트 창의 연결(`exec_grant`)과 신뢰. 미신뢰 프로젝트 · 다른 프로젝트 창 · 에이전트
//!    범위는 어댑터를 띄우지 않고 거절이다. 디버그 대상을 띄우는 `runInTerminal`(역요청)도 같은 허가를 역요청마다 다시 본다.
//! 2. **구성**: 실행과 같은 판정으로 읽는다 — 그 워크트리의 구성(쓸 수 없는 구성은 거절), 작업 폴더는 루트 안, `.env` 와 프로필은
//!    M6-03 의 규칙(`RunEnv::resolve`). `[debug]` 가 없으면 거절.
//! 3. **어댑터**: [`adapter`] — 고르기 · 해석 · 인자의 자리 · 요청 틀 채우기.
//! 4. **실행**: `Runs::start_kind` 로 실행 하나를 만들고(종류 `debug`) 그 실행의 콘솔에 어댑터 출력과 디버그 대상의 PTY 를 잇는다.
//! 5. **세션**: 저장된 브레이크포인트(켠 것)와 그 어댑터에 켠 예외 필터를 처음 브레이크포인트로 싣고 `DebugSession::start`. 어댑터가
//!    옮긴 줄은 저장에 반영한다. 시작하는 동안 저장이 바뀌었으면 다시 보낸다.
//!
//! `mode = "attach"` 면 요청이 `attach` 이고 아무 프로그램도 띄우지 않는다 — 대상은 사람이 JDWP · `--inspect` 등을 켜고 띄운
//! 것이다(메인 판정 M6-P20). 끝낼 때도 대상을 끝내지 않고 끊기만 한다.
//!
//! # 이벤트(프로젝트 범위)
//!
//! `debug.stopped` · `debug.continued` · `debug.output`(어댑터의 출력 — 콘솔에도 쓴다, `telemetry` 는 버린다) · `debug.ended` ·
//! `debug.breakpoints`(저장이 바뀌었거나 어댑터가 확인 · 옮김을 알렸다). 03 「모듈」 표의 이름이다.
//!
//! # 범위
//!
//! `runId` 를 받는 `debug.*` 는 그 실행의 프로젝트 창만 — `run.*` 와 같은 판정(M6-02 ⑤): 다른 프로젝트의 `runId` 는 없는 실행이고,
//! 프로젝트 창이 아닌 연결(Host · 에이전트 범위)은 조회가 없는 실행, 제어 · 평가가 `PERMISSION_DENIED` 다. 파일 창은 디스패처가 먼저
//! 거절한다(`file_scope` 가 아니다 — 진짜 id 와 없는 id 의 거절이 같다). 브레이크포인트 RPC 는 그 프로젝트 창만.
//!
//! 끝내기는 실행의 중지(`run.stop`)다 — 디버그 실행도 실행이다. 다시 실행(`run.restart`)은 하지 않는다(`Relaunch::None`).

pub mod adapter;
pub mod breakpoints;
pub mod methods;
pub mod run;
mod source;
pub mod tool;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

use pw_core::bus::{EventBus, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::Registry;
use pw_dap::protocol::{
    self, Breakpoint as DapBreakpoint, Capabilities, SetBreakpointsArguments,
    SetExceptionBreakpointsArguments, Source, SourceBreakpoint,
};
use pw_dap::{
    DebugGrant, DebugSession, InitialBreakpoints, Session, SessionEvent, SessionEventKind,
    SessionId, SessionOptions, StartError, StartRequest, Timeouts,
};
use pw_lsp_servers::LspCommands;
use pw_rpc::{RpcError, code};
use pw_run::config::{DebugMode, RunConfigPlace};
use pw_run::env::RunEnv;
use pw_run::runs::{
    LaunchedBy, NewRun, ProcessLauncher, PtyLauncher, Relaunch, RunId, RunKind, Runs, no_such_run,
};
use pw_runtime::Runtime;
use serde_json::json;
use tokio::sync::mpsc;

pub use adapter::Adapters;
pub use breakpoints::{Breakpoint, Breakpoints, FileBreakpoints};
pub use methods::*;
pub use run::{DebugRun, KIND};

use crate::run::{RunTerminals, exit_of, lock};

pub const TOPIC_STOPPED: &str = "debug.stopped";
pub const TOPIC_CONTINUED: &str = "debug.continued";
pub const TOPIC_OUTPUT: &str = "debug.output";
pub const TOPIC_ENDED: &str = "debug.ended";
pub const TOPIC_BREAKPOINTS: &str = "debug.breakpoints";

/// 이벤트 채널의 크기(세션 하나).
const EVENTS: usize = 512;
/// `debug.output` 이벤트 하나에 싣는 글의 상한(바이트) — 콘솔에는 다 쓴다.
const OUTPUT_EVENT_LIMIT: usize = 16 * 1024;

/// Host 를 만든 뒤 붙이는 것.
#[derive(Clone)]
pub struct HostParts {
    /// 설정 경로(실행 구성 폴더)와 앱 데이터(`breakpoints.json`).
    pub paths: AppPaths,
    pub projects: Registry,
    pub bus: EventBus,
}

struct Host {
    parts: HostParts,
    breakpoints: Breakpoints,
}

/// 한 Host 의 디버그 세션들. 복제해도 같다.
#[derive(Clone)]
pub struct Debugger {
    inner: Arc<Inner>,
}

struct Inner {
    runs: Runs,
    runtime: Arc<Runtime>,
    adapters: Arc<dyn Adapters>,
    launcher: Arc<dyn ProcessLauncher>,
    commands: OnceLock<Arc<dyn LspCommands>>,
    host: OnceLock<Host>,
    timeouts: Timeouts,
    sessions: Mutex<BTreeMap<RunId, Arc<DebugRun>>>,
    /// 프레임 소스의 읽기 토큰(`debug.source` — [`source`]).
    sources: source::SourceTokens,
}

/// 시작 전에 블로킹으로 푼 것.
struct Prepared {
    config_id: String,
    name: String,
    mode: DebugMode,
    cwd: PathBuf,
    setup: pw_lsp_servers::AdapterSetup,
    arguments: serde_json::Map<String, serde_json::Value>,
    /// `[debug] connect` — 대상이 띄운 어댑터의 주소(M6-19b).
    listening: Option<std::net::SocketAddr>,
}

impl Debugger {
    /// 디버그 대상을 PTY 로 띄우는 디버거.
    pub fn new(runs: Runs, runtime: Arc<Runtime>, adapters: Arc<dyn Adapters>) -> Self {
        Self::with(
            runs,
            runtime,
            adapters,
            Arc::new(PtyLauncher),
            Timeouts::default(),
        )
    }

    /// 디버그 대상을 띄우는 것 · 시한을 정해 만든다(시험).
    pub fn with(
        runs: Runs,
        runtime: Arc<Runtime>,
        adapters: Arc<dyn Adapters>,
        launcher: Arc<dyn ProcessLauncher>,
        timeouts: Timeouts,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                runs,
                runtime,
                adapters,
                launcher,
                commands: OnceLock::new(),
                host: OnceLock::new(),
                timeouts,
                sessions: Mutex::default(),
                sources: source::SourceTokens::default(),
            }),
        }
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let breakpoints = Breakpoints::new(parts.paths.data.clone());
        let _ = self.inner.host.set(Host { parts, breakpoints });
    }

    /// 언어 서버가 여는 어댑터(`start` — java-debug)가 명령을 보낼 곳. 없으면 그런 어댑터는 「서버가 떠 있지 않다」 로 거절한다.
    pub fn attach_language_servers(&self, commands: Arc<dyn LspCommands>) {
        let _ = self.inner.commands.set(commands);
    }

    fn host(&self) -> Result<&Host, RpcError> {
        self.inner
            .host
            .get()
            .ok_or_else(|| RpcError::internal("the debugger is not attached to a host"))
    }

    /// 브레이크포인트 저장(M6-20 의 툴도 이것을 쓴다).
    pub fn breakpoints(&self) -> Result<Breakpoints, RpcError> {
        Ok(self.host()?.breakpoints.clone())
    }

    pub(crate) fn source_tokens(&self) -> &source::SourceTokens {
        &self.inner.sources
    }

    /// 그 프로젝트의 디버그 세션(다른 프로젝트 · 없는 실행 · 디버그 실행이 아닌 것은 같은 「없는 실행」).
    pub fn find(&self, project_id: &str, run_id: &str) -> Result<Arc<DebugRun>, RpcError> {
        lock(&self.inner.sessions)
            .get(run_id)
            .filter(|r| r.project_id == project_id)
            .cloned()
            .ok_or_else(|| no_such_run(run_id))
    }

    /// 그 프로젝트의 디버그 세션들.
    pub fn sessions_of(&self, project_id: &str) -> Vec<Arc<DebugRun>> {
        lock(&self.inner.sessions)
            .values()
            .filter(|r| r.project_id == project_id)
            .cloned()
            .collect()
    }

    fn publish<T: serde::Serialize>(&self, project_id: &str, topic: &str, payload: &T) {
        let Ok(host) = self.host() else {
            return;
        };
        if let Err(e) =
            host.parts
                .bus
                .publish(Scope::Project(project_id.to_owned()), topic, payload)
        {
            tracing::warn!(target: "pw_debug", %e, topic, "cannot publish a debug event");
        }
    }

    /// 디버그 세션을 연다(머리 주석 「시작」). `grant` 는 `pw_dap::debug_grant` 를 지난 것이다.
    /// 디버그 시작 앞(17-m6-plan M6-21 — 리뷰 F5): 구성의 `before` 사슬을 `run.start` 와 같은 확인(7001 · 3001)으로 끝까지 돌린다.
    /// `before` 가 없으면 아무것도 하지 않는다. 확인이 필요하면 그 오류로 답한다 — 디버그 세션은 열지 않는다.
    pub async fn run_before(
        &self,
        project_id: &str,
        scope: pw_rpc::ConnectionScope,
        seen: Option<pw_core::registry::run_seen::SeenBy>,
        window: Option<String>,
        params: &DebugStartParams,
    ) -> Result<(), RpcError> {
        self.inner
            .runs
            .run_before_window(
                project_id,
                scope,
                seen,
                window,
                pw_run::runs::RunStartParams {
                    project_id: params.project_id.clone(),
                    worktree: params.worktree.clone(),
                    config_id: params.config_id.clone(),
                    profile: params.profile.clone(),
                    cols: params.cols,
                    rows: params.rows,
                    confirmed: params.confirmed.clone(),
                    sql_confirmed: params.sql_confirmed.clone(),
                },
            )
            .await
    }

    /// `before` 사슬 앞의 어댑터 검사([`preflight`]) — 띄울 수 없으면 그 오류(`adapterMissing` 등). `run_before` 앞에 부른다.
    pub async fn check_adapter(
        &self,
        grant: &DebugGrant,
        params: &DebugStartParams,
    ) -> Result<(), RpcError> {
        let host = self.host()?;
        let paths = host.parts.paths.clone();
        let projects = host.parts.projects.clone();
        let adapters = Arc::clone(&self.inner.adapters);
        let project_id = grant.project_id().to_owned();
        let root = grant.root().to_owned();
        let params = params.clone();
        blocking(move || preflight(&paths, &projects, &*adapters, &project_id, &root, &params))
            .await
    }

    pub async fn start(
        &self,
        grant: DebugGrant,
        window: Option<String>,
        params: DebugStartParams,
    ) -> Result<DebugStarted, RpcError> {
        let size = (
            params.cols.unwrap_or(pw_run::runs::DEFAULT_SIZE.0),
            params.rows.unwrap_or(pw_run::runs::DEFAULT_SIZE.1),
        );
        if size.0 == 0 || size.1 == 0 {
            return Err(RpcError::invalid_params("the console size must not be 0"));
        }
        let host = self.host()?;
        let paths = host.parts.paths.clone();
        let projects = host.parts.projects.clone();
        let breakpoints = host.breakpoints.clone();
        let adapters = Arc::clone(&self.inner.adapters);
        let project_id = grant.project_id().to_owned();
        let root = grant.root().to_owned();
        let prepared = {
            let params = params.clone();
            let project_id = project_id.clone();
            let root = root.clone();
            blocking(move || prepare(&paths, &projects, &*adapters, &project_id, &root, &params))
                .await?
        };
        let port = if prepared.listening.is_some() {
            None
        } else {
            adapter::language_server_port(
                &project_id,
                &root,
                &prepared.setup,
                self.inner.commands.get().cloned(),
                self.inner.timeouts.connect,
            )
            .await?
        };
        let spec = adapter::spec(&prepared.setup, &root, port, prepared.listening)?;
        let adapter_id = prepared.setup.definition.id.clone();

        let mut made: Option<Arc<DebugRun>> = None;
        let info = self.inner.runs.start_kind(
            NewRun {
                project_id: project_id.clone(),
                config: prepared.config_id.clone(),
                name: prepared.name.clone(),
                launched_by: LaunchedBy::Window { window },
                worktree: params.worktree.clone(),
                cwd: Some(prepared.cwd.clone()),
                profile: params.profile.clone(),
                relaunch: Relaunch::None,
                size,
                restart_of: None,
            },
            |output| {
                let run = Arc::new(DebugRun::new(
                    project_id.clone(),
                    adapter_id.clone(),
                    prepared.mode,
                    root.clone(),
                    output,
                    size,
                ));
                made = Some(Arc::clone(&run));
                Ok(run as Arc<dyn RunKind>)
            },
        )?;
        let run = made.expect("start_kind called the closure");
        let _ = run.run_id.set(info.run_id.clone());
        lock(&self.inner.sessions).insert(info.run_id.clone(), Arc::clone(&run));
        tracing::info!(target: "pw_debug", run = %info.run_id, project = %project_id, adapter = %adapter_id, mode = ?prepared.mode, "debug session starting");

        // 처음 브레이크포인트: 저장된 것(켠 것) — 시작하는 동안 바뀌면 시작 뒤 다시 보낸다.
        let generation = breakpoints.generation();
        let stored = breakpoints.get(&project_id).unwrap_or_default();
        let sent = initial_sources(&root, &stored.files);
        let filters = stored
            .exception_filters
            .get(&adapter_id)
            .cloned()
            .unwrap_or_default();
        let request = StartRequest {
            mode: match prepared.mode {
                DebugMode::Launch => protocol::StartMode::Launch,
                DebugMode::Attach => protocol::StartMode::Attach,
            },
            arguments: prepared.arguments,
            breakpoints: InitialBreakpoints {
                sources: sent.iter().map(|(_, args)| args.clone()).collect(),
                functions: Vec::new(),
                exception_filters: filters,
            },
        };
        let (events, received) = mpsc::channel(EVENTS);
        tokio::spawn(self.clone().pump(Arc::clone(&run), received));
        let terminals = Arc::new(RunTerminals {
            run: Arc::downgrade(&run),
            runtime: Arc::clone(&self.inner.runtime),
            launcher: Arc::clone(&self.inner.launcher),
            grace: self.inner.runs.limits().stop_grace,
        });
        let started = DebugSession::start(
            grant,
            spec,
            request,
            SessionOptions {
                events,
                terminals,
                locale: None,
                timeouts: self.inner.timeouts,
            },
        )
        .await;
        match started {
            Ok((session, configured)) => {
                let session = Arc::new(session);
                let root_id = session.root().id();
                *lock(&run.capabilities) = configured.capabilities.clone();
                let _ = run.session.set(Arc::clone(&session));
                for ((rel, args), answers) in sent.iter().zip(&configured.sources) {
                    self.answered(&run, root_id, rel, args, answers);
                }
                if breakpoints.generation() != generation {
                    // 시작 중에 열린 자식도 시작 때의 목록을 받았다 — 나무 전체에 지금 것을 보낸다.
                    for s in session.sessions() {
                        self.push_all(&run, &s).await;
                    }
                }
                tokio::spawn(self.clone().supervise(Arc::clone(&run)));
                tracing::info!(target: "pw_debug", run = %info.run_id, "debug session started");
                Ok(DebugStarted {
                    run: self
                        .inner
                        .runs
                        .find(&project_id, &info.run_id)
                        .map(|r| r.info())
                        .unwrap_or(info),
                    adapter: adapter_id,
                    mode: prepared.mode,
                    exception_filters: exception_filters(
                        &configured.capabilities,
                        &configured.exception_filters,
                    ),
                })
            }
            Err(e) => {
                let message = e.to_string();
                tracing::info!(target: "pw_debug", run = %info.run_id, error = %message, "debug session did not start");
                run.console(&format!("\n[debug] {message}\n"));
                self.finish(&run, None).await;
                let code = match e {
                    StartError::Refused(_) => code::PERMISSION_DENIED,
                    _ => code::INTERNAL_ERROR,
                };
                Err(
                    RpcError::new(code, format!("cannot start debugging: {message}"))
                        .with_data(json!({ "runId": info.run_id })),
                )
            }
        }
    }

    /// 끝내기를 기다렸다가 끝낸다: 중지 · 어댑터의 `terminated` · 뿌리 세션의 끝 · 디버그 대상 PTY 의 끝(실행의 끝).
    async fn supervise(self, run: Arc<DebugRun>) {
        let Some(session) = run.session().cloned() else {
            return;
        };
        let root = Arc::clone(session.root());
        let ran = self.inner.runs.find(&run.project_id, run.run_id()).ok();
        tokio::select! {
            _ = root.wait_ended() => {}
            _ = run.end.notified() => {}
            _ = async {
                match &ran {
                    Some(r) => { r.exited().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
        self.finish(&run, Some(&session)).await;
    }

    /// 한 번만: 어댑터를 끊고(`launch` 면 대상도 끝내라고), 디버그 대상의 PTY 를 끝내고, 실행의 끝을 알리고, `debug.ended`.
    async fn finish(&self, run: &Arc<DebugRun>, session: Option<&Arc<DebugSession>>) {
        if run.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        run.session_ended(None);
        let mut adapter_exit = None;
        if let Some(session) = session {
            let ended = session
                .disconnect(Some(run.mode == DebugMode::Launch))
                .await;
            adapter_exit = ended.exit_code;
        }
        // PTY 가 있으면 그 끝(트리까지 끝난 뒤)이 실행의 끝을 알린다. 이미 끝났으면 이미 알렸다.
        if !run.stop_terminals() {
            run.output.exited(exit_of(run, adapter_exit));
        }
        lock(&self.inner.sessions).remove(run.run_id());
        self.inner.sources.forget_run(run.run_id());
        self.publish(
            &run.project_id,
            TOPIC_ENDED,
            &DebugEndedEvent {
                project_id: run.project_id.clone(),
                run_id: run.run_id().to_owned(),
            },
        );
        tracing::info!(target: "pw_debug", run = %run.run_id(), "debug session ended");
    }

    /// 세션 나무의 이벤트를 콘솔 · 버스 · 저장으로.
    async fn pump(self, run: Arc<DebugRun>, mut received: mpsc::Receiver<SessionEvent>) {
        while let Some(event) = received.recv().await {
            let root = run.session().map(|s| s.root().id());
            let is_root = root.is_none_or(|r| r == event.session);
            match event.kind {
                SessionEventKind::Output(o) => {
                    if o.category.as_deref() == Some("telemetry") {
                        continue;
                    }
                    run.console(&o.output);
                    let mut output = o.output;
                    if output.len() > OUTPUT_EVENT_LIMIT {
                        let mut at = OUTPUT_EVENT_LIMIT;
                        while !output.is_char_boundary(at) {
                            at -= 1;
                        }
                        output.truncate(at);
                    }
                    self.publish(
                        &run.project_id,
                        TOPIC_OUTPUT,
                        &DebugOutputEvent {
                            project_id: run.project_id.clone(),
                            run_id: run.run_id().to_owned(),
                            session: event.session,
                            category: o.category,
                            output,
                        },
                    );
                }
                SessionEventKind::Stopped(s) => {
                    let all_threads_stopped = s.all_threads_stopped.unwrap_or(false);
                    run.stopped(
                        event.session,
                        run::Stop {
                            reason: s.reason.clone(),
                            description: s.description.clone(),
                            thread_id: s.thread_id,
                            all_threads_stopped,
                        },
                    );
                    self.publish(
                        &run.project_id,
                        TOPIC_STOPPED,
                        &DebugStoppedEvent {
                            project_id: run.project_id.clone(),
                            run_id: run.run_id().to_owned(),
                            session: event.session,
                            reason: s.reason,
                            description: s.description,
                            thread_id: s.thread_id,
                            text: s.text,
                            all_threads_stopped,
                            hit_breakpoint_ids: s.hit_breakpoint_ids,
                        },
                    )
                }
                SessionEventKind::Continued(c) => {
                    let all = c.all_threads_continued.unwrap_or(true);
                    run.resumed(event.session, (!all).then_some(c.thread_id));
                    self.publish(
                        &run.project_id,
                        TOPIC_CONTINUED,
                        &DebugContinuedEvent {
                            project_id: run.project_id.clone(),
                            run_id: run.run_id().to_owned(),
                            session: event.session,
                            thread_id: Some(c.thread_id),
                            all_threads_continued: all,
                        },
                    )
                }
                SessionEventKind::Exited(e) => {
                    if is_root {
                        *lock(&run.debuggee_exit) = i32::try_from(e.exit_code).ok();
                    }
                }
                SessionEventKind::Terminated(_) | SessionEventKind::Ended(_) => {
                    run.session_ended(Some(event.session));
                    // 시작이 끝나기 전(뿌리를 아직 모른다)의 끝도 뿌리의 것으로 본다 — 끝내기 표지는 남아 있다가 시작 뒤에 끝낸다.
                    if is_root {
                        run.end.notify_one();
                    } else if let Some(child) = run.session().and_then(|s| s.session(event.session))
                    {
                        // 자식 세션의 끝 — 그 자식만 끊는다.
                        tokio::spawn(async move {
                            child.disconnect(None).await;
                        });
                    }
                }
                SessionEventKind::Breakpoint(b) => {
                    self.adapter_moved(&run, event.session, b.breakpoint)
                }
                SessionEventKind::ChildStarted { child } => {
                    // 자식은 시작 때의 브레이크포인트를 받았다 — 그 뒤에 바뀐 것까지 지금 것으로 다시.
                    if let Some(child) = run.session().and_then(|s| s.session(child)) {
                        let me = self.clone();
                        let run = Arc::clone(&run);
                        // 이 이벤트는 자식의 `initialize` 전에 온다 — 시작 순서를 마친 뒤에 보낸다.
                        tokio::spawn(async move {
                            if child.wait_configured().await {
                                me.push_all(&run, &child).await;
                            }
                        });
                    }
                }
                SessionEventKind::Thread(_) => {}
            }
        }
    }

    /// 어댑터의 `setBreakpoints` 답을 반영한다: 옮긴 줄을 저장에, id 를 표에, 확인 상태를 이벤트로.
    fn answered(
        &self,
        run: &DebugRun,
        session: SessionId,
        rel: &str,
        sent: &SetBreakpointsArguments,
        answers: &[DapBreakpoint],
    ) {
        let mut moves = Vec::new();
        let mut states = Vec::new();
        {
            let mut ids = lock(&run.ids);
            ids.retain(|(s, _), (path, _)| !(*s == session && path == rel));
            for (asked, answer) in sent.breakpoints.iter().zip(answers) {
                let asked_line = u32::try_from(asked.line).unwrap_or(0);
                let line = answer
                    .line
                    .and_then(|l| u32::try_from(l).ok())
                    .filter(|l| *l > 0)
                    .unwrap_or(asked_line);
                if line != asked_line {
                    moves.push((asked_line, line));
                }
                if let Some(id) = answer.id {
                    ids.insert((session, id), (rel.to_owned(), line));
                }
                states.push(BreakpointState {
                    line,
                    verified: answer.verified,
                    message: answer.message.clone(),
                });
            }
        }
        self.after_answer(run, rel, &moves, states);
    }

    /// 어댑터가 `breakpoint` 이벤트로 알린 바뀜(옮긴 줄 · 확인).
    fn adapter_moved(&self, run: &DebugRun, session: SessionId, answer: DapBreakpoint) {
        let Some(id) = answer.id else {
            return;
        };
        let Some(line) = answer
            .line
            .and_then(|l| u32::try_from(l).ok())
            .filter(|l| *l > 0)
        else {
            return;
        };
        let found = {
            let mut ids = lock(&run.ids);
            ids.get_mut(&(session, id)).map(|entry| {
                let before = entry.1;
                entry.1 = line;
                (entry.0.clone(), before)
            })
        };
        let Some((rel, before)) = found else {
            return;
        };
        let states = vec![BreakpointState {
            line,
            verified: answer.verified,
            message: answer.message,
        }];
        let moves = if before == line {
            Vec::new()
        } else {
            vec![(before, line)]
        };
        self.after_answer(run, &rel, &moves, states);
    }

    fn after_answer(
        &self,
        run: &DebugRun,
        rel: &str,
        moves: &[(u32, u32)],
        states: Vec<BreakpointState>,
    ) {
        let Ok(store) = self.breakpoints() else {
            return;
        };
        if !moves.is_empty()
            && let Err(e) = store.moved(&run.project_id, rel, moves)
        {
            tracing::warn!(target: "pw_debug", error = %e.message, "cannot store the lines the debug adapter moved");
        }
        let breakpoints = store
            .get(&run.project_id)
            .ok()
            .and_then(|s| s.files.get(rel).cloned())
            .unwrap_or_default();
        self.publish(
            &run.project_id,
            TOPIC_BREAKPOINTS,
            &DebugBreakpointsEvent {
                project_id: run.project_id.clone(),
                run_id: Some(run.run_id().to_owned()),
                path: rel.to_owned(),
                breakpoints,
                states,
            },
        );
    }

    /// 한 파일의 지금 목록(켠 것)을 그 세션에 보낸다.
    async fn push_file(
        &self,
        run: &DebugRun,
        session: &Arc<Session>,
        rel: &str,
        list: &[Breakpoint],
    ) {
        if session.ended().is_some() {
            return;
        }
        let args = source_arguments(&run.root, rel, list);
        match session.request::<protocol::SetBreakpoints>(&args).await {
            Ok(body) => self.answered(run, session.id(), rel, &args, &body.breakpoints),
            Err(e) => {
                tracing::info!(target: "pw_debug", run = %run.run_id(), error = %e, "the debug adapter did not take the breakpoints");
            }
        }
    }

    /// 저장된 모든 파일(지운 파일은 빈 목록으로)과 예외 필터를 그 세션에 다시 보낸다.
    async fn push_all(&self, run: &DebugRun, session: &Arc<Session>) {
        let Ok(store) = self.breakpoints() else {
            return;
        };
        let stored = store.get(&run.project_id).unwrap_or_default();
        let known: Vec<String> = {
            let ids = lock(&run.ids);
            ids.values().map(|(p, _)| p.clone()).collect()
        };
        for (rel, list) in &stored.files {
            self.push_file(run, session, rel, list).await;
        }
        for rel in known {
            if !stored.files.contains_key(&rel) {
                self.push_file(run, session, &rel, &[]).await;
            }
        }
        let filters = stored
            .exception_filters
            .get(&run.adapter)
            .cloned()
            .unwrap_or_default();
        self.push_exception_filters(session, &filters).await;
    }

    async fn push_exception_filters(&self, session: &Arc<Session>, filters: &[String]) {
        if session.ended().is_some() {
            return;
        }
        let announced = session.capabilities().exception_breakpoint_filters;
        let filters: Vec<String> = filters
            .iter()
            .filter(|f| announced.iter().any(|a| &a.filter == *f))
            .cloned()
            .collect();
        if announced.is_empty() {
            return;
        }
        if let Err(e) = session
            .request::<protocol::SetExceptionBreakpoints>(&SetExceptionBreakpointsArguments {
                filters,
            })
            .await
        {
            tracing::info!(target: "pw_debug", error = %e, "the debug adapter did not take the exception filters");
        }
    }

    /// 파일 하나의 목록을 저장하고(통째로), 그 프로젝트의 세션마다 곧바로 보낸다.
    pub async fn set_breakpoints(
        &self,
        project_id: &str,
        path: &str,
        list: Vec<Breakpoint>,
    ) -> Result<FileBreakpoints, RpcError> {
        let store = self.breakpoints()?;
        let saved = store.set_file(project_id, path, list)?;
        let rel = breakpoints::relative_path(path)?;
        self.changed(project_id, &rel, &saved).await;
        Ok(FileBreakpoints {
            path: rel,
            breakpoints: saved,
        })
    }

    /// 한 줄(없으면 그 파일 전부)을 지우고 세션마다 곧바로 보낸다.
    pub async fn remove_breakpoints(
        &self,
        project_id: &str,
        path: &str,
        line: Option<u32>,
    ) -> Result<FileBreakpoints, RpcError> {
        let store = self.breakpoints()?;
        let saved = store.remove(project_id, path, line)?;
        let rel = breakpoints::relative_path(path)?;
        self.changed(project_id, &rel, &saved).await;
        Ok(FileBreakpoints {
            path: rel,
            breakpoints: saved,
        })
    }

    /// 하나를 넣거나 바꾸고 세션마다 곧바로 보낸다(M6-20 의 툴).
    pub async fn upsert_breakpoint(
        &self,
        project_id: &str,
        path: &str,
        breakpoint: Breakpoint,
    ) -> Result<FileBreakpoints, RpcError> {
        let store = self.breakpoints()?;
        let saved = store.upsert(project_id, path, breakpoint)?;
        let rel = breakpoints::relative_path(path)?;
        self.changed(project_id, &rel, &saved).await;
        Ok(FileBreakpoints {
            path: rel,
            breakpoints: saved,
        })
    }

    /// 한 줄을 바꾸고(지금 것이 `expected` 일 때만 — [`Breakpoints::replace_line`]) 세션마다 곧바로 보낸다(M6-20 의 툴이 승인 뒤에).
    pub async fn replace_breakpoint(
        &self,
        project_id: &str,
        path: &str,
        line: u32,
        expected: Option<&Breakpoint>,
        new: Option<Breakpoint>,
    ) -> Result<FileBreakpoints, RpcError> {
        let store = self.breakpoints()?;
        let saved = store.replace_line(project_id, path, line, expected, new)?;
        let rel = breakpoints::relative_path(path)?;
        self.changed(project_id, &rel, &saved).await;
        Ok(FileBreakpoints {
            path: rel,
            breakpoints: saved,
        })
    }

    /// Host 의 프로젝트 레지스트리(툴의 관문 — `agent_grant`).
    pub(crate) fn projects(&self) -> Result<Registry, RpcError> {
        Ok(self.host()?.parts.projects.clone())
    }

    async fn changed(&self, project_id: &str, rel: &str, saved: &[Breakpoint]) {
        self.publish(
            project_id,
            TOPIC_BREAKPOINTS,
            &DebugBreakpointsEvent {
                project_id: project_id.to_owned(),
                run_id: None,
                path: rel.to_owned(),
                breakpoints: saved.to_vec(),
                states: Vec::new(),
            },
        );
        for run in self.sessions_of(project_id) {
            let Some(session) = run.session().cloned() else {
                // 아직 시작 중 — 시작이 끝나면 저장이 바뀐 것을 보고 다시 보낸다.
                continue;
            };
            for s in session.sessions() {
                self.push_file(&run, &s, rel, saved).await;
            }
        }
    }

    /// 예외 필터를 켜고 끈다 — 그 세션의 어댑터가 `initialize` 로 알린 필터만. 저장(어댑터마다)하고 세션 나무에 곧바로 보낸다.
    pub async fn set_exception_filters(
        &self,
        project_id: &str,
        run_id: &str,
        filters: Vec<String>,
    ) -> Result<Vec<ExceptionFilter>, RpcError> {
        let run = self.find(project_id, run_id)?;
        let capabilities = lock(&run.capabilities).clone();
        if let Some(unknown) = filters.iter().find(|f| {
            !capabilities
                .exception_breakpoint_filters
                .iter()
                .any(|a| &a.filter == *f)
        }) {
            return Err(RpcError::invalid_params(format!(
                "the debug adapter `{}` has no exception filter `{unknown}`",
                run.adapter
            )));
        }
        let mut filters = filters;
        filters.sort();
        filters.dedup();
        self.breakpoints()?
            .set_exception_filters(project_id, &run.adapter, filters.clone())?;
        if let Some(session) = run.session().cloned() {
            for s in session.sessions() {
                self.push_exception_filters(&s, &filters).await;
            }
        }
        Ok(exception_filters(&capabilities, &filters))
    }

    /// 그 디버그 세션의 DAP 세션 하나(없으면 뿌리). 시작 중이거나 없는 세션이면 invalid params.
    pub fn dap_session(
        &self,
        project_id: &str,
        run_id: &str,
        session: Option<SessionId>,
    ) -> Result<Arc<Session>, RpcError> {
        let run = self.find(project_id, run_id)?;
        let tree = run
            .session()
            .ok_or_else(|| RpcError::invalid_params("the debug session is still starting"))?;
        match session {
            None => Ok(Arc::clone(tree.root())),
            Some(id) => tree
                .session(id)
                .ok_or_else(|| RpcError::invalid_params(format!("no debug session {id}"))),
        }
    }
}

/// 처음 보낼 파일들(켠 것이 하나라도 있는 파일): (상대 경로, `setBreakpoints` 인자).
fn initial_sources(
    root: &Path,
    files: &BTreeMap<String, Vec<Breakpoint>>,
) -> Vec<(String, SetBreakpointsArguments)> {
    files
        .iter()
        .filter(|(_, list)| list.iter().any(|b| b.enabled))
        .map(|(rel, list)| (rel.clone(), source_arguments(root, rel, list)))
        .collect()
}

fn source_arguments(root: &Path, rel: &str, list: &[Breakpoint]) -> SetBreakpointsArguments {
    let path = rel
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part));
    SetBreakpointsArguments {
        source: Source {
            name: rel.rsplit('/').next().map(str::to_owned),
            path: Some(path.to_string_lossy().into_owned()),
            ..Source::default()
        },
        breakpoints: list
            .iter()
            .filter(|b| b.enabled)
            .map(|b| SourceBreakpoint {
                line: i64::from(b.line),
                condition: b.condition.clone(),
                hit_condition: b.hit_condition.clone(),
                log_message: b.log_message.clone(),
                ..SourceBreakpoint::default()
            })
            .collect(),
        source_modified: None,
    }
}

/// 어댑터가 알린 예외 필터와 켠 것.
fn exception_filters(capabilities: &Capabilities, enabled: &[String]) -> Vec<ExceptionFilter> {
    capabilities
        .exception_breakpoint_filters
        .iter()
        .map(|f| ExceptionFilter {
            filter: f.filter.clone(),
            label: f.label.clone(),
            description: f.description.clone(),
            default: f.default,
            enabled: enabled.contains(&f.filter),
        })
        .collect()
}

/// 구성을 읽고 어댑터를 푼다(블로킹 — 머리 주석 「시작」 2 · 3).
/// 구성을 읽는다(프로젝트 · 구성 자리 · 구성). [`prepare`] 와 [`preflight`] 가 함께 쓴다.
fn load_config(
    paths: &AppPaths,
    projects: &Registry,
    project_id: &str,
    params: &DebugStartParams,
) -> Result<(RunConfigPlace, String, pw_run::config::RunConfig), RpcError> {
    let project = projects.get(project_id).map_err(|e| match e {
        pw_core::registry::RegistryError::NotFound(id) => {
            RpcError::new(code::NOT_FOUND, format!("project `{id}` is not registered"))
        }
        other => RpcError::internal(other.to_string()),
    })?;
    let place = RunConfigPlace::new(paths, &project, params.worktree.as_deref())?;
    let set = pw_run::config::read(&place);
    let Some(loaded) = set.get(&params.config_id) else {
        if let Some(unusable) = set.unusable.iter().find(|u| u.id == params.config_id) {
            return Err(RpcError::invalid_params(format!(
                "run config `{}` cannot be used: {}",
                unusable.id, unusable.reason
            )));
        }
        return Err(RpcError::new(
            code::NOT_FOUND,
            format!("run config `{}` does not exist", params.config_id),
        ));
    };
    let (id, config) = (loaded.id.clone(), loaded.config.clone());
    Ok((place, id, config))
}

/// 구성의 어댑터를 해석한다: 고르기 · 해석 · `[debug] connect` · (띄울 때) 런타임.
fn resolve_adapter(
    adapters: &dyn Adapters,
    project_id: &str,
    root: &Path,
    params: &DebugStartParams,
    config: &pw_run::config::RunConfig,
) -> Result<(pw_lsp_servers::AdapterSetup, Option<std::net::SocketAddr>), RpcError> {
    let definitions = adapters.definitions();
    let adapter_id = adapter::choose(config, &definitions)?;
    let mut setup = adapters.setup(project_id, params.worktree.as_deref(), &adapter_id, root)?;
    let listening = adapter::listening(&setup.definition, config)?;
    if listening.is_none() {
        adapters.find_runtimes(&mut setup)?;
    }
    Ok((setup, listening))
}

fn no_debug_section(id: &str) -> RpcError {
    RpcError::invalid_params(format!("the run config `{id}` has no [debug] section"))
        .with_data(json!({ "reason": "noDebug" }))
}

/// `before` 사슬 앞의 검사(실기 F-1 후속 — 메인 판정 2026-11-21): 어댑터를 해석해 띄울 수 없으면(꺼짐 · 설치 안 됨 · 런타임 없음)
/// 그 오류를 준다 — `before`(빌드 등)를 끝까지 돌린 뒤에야 「없다」 를 알지 않게. 시작은 [`prepare`] 로 다시 해석한다(그사이 설치 ·
/// 정의가 바뀔 수 있다). `[debug] connect`(debugpy `attach_connect`)는 설치를 보지 않는다.
fn preflight(
    paths: &AppPaths,
    projects: &Registry,
    adapters: &dyn Adapters,
    project_id: &str,
    root: &Path,
    params: &DebugStartParams,
) -> Result<(), RpcError> {
    let (_place, id, config) = load_config(paths, projects, project_id, params)?;
    if config.debug.is_none() {
        return Err(no_debug_section(&id));
    }
    let (setup, listening) = resolve_adapter(adapters, project_id, root, params, &config)?;
    adapter::launchable(&setup, listening)
}

fn prepare(
    paths: &AppPaths,
    projects: &Registry,
    adapters: &dyn Adapters,
    project_id: &str,
    root: &Path,
    params: &DebugStartParams,
) -> Result<Prepared, RpcError> {
    let (place, id, config) = load_config(paths, projects, project_id, params)?;
    let config = &config;
    let debug = config.debug.as_ref().ok_or_else(|| no_debug_section(&id))?;
    let cwd = match &config.cwd {
        Some(rel) => pw_run::config::root::inside(&place.root, rel)
            .map_err(|why| RpcError::invalid_params(format!("cwd {why}")))?,
        None => std::fs::canonicalize(&place.root)
            .map(pw_core::registry::path::simplify)
            .unwrap_or_else(|_| place.root.clone()),
    };
    if !cwd.is_dir() {
        return Err(RpcError::invalid_params(format!(
            "cwd `{}` is not a directory",
            cwd.display()
        )));
    }
    let run_env = RunEnv::resolve(config, params.profile.as_deref(), &place.root)
        .map_err(|e| RpcError::invalid_params(e.to_string()))?;
    let (setup, listening) = resolve_adapter(adapters, project_id, root, params, config)?;
    let arguments =
        adapter::request_arguments(&setup.definition, config, &cwd, run_env.vars(), root)?;
    Ok(Prepared {
        config_id: id,
        name: config.name.clone(),
        mode: debug.mode,
        cwd,
        setup,
        arguments,
        listening,
    })
}

/// 블로킹 스레드에서 돌린다(구성 · `.env` · 레지스트리 읽기, 어댑터 해석).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}
