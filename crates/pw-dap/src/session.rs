//! 디버그 세션 나무와 그 상태기계(17-m6-plan.md M6-15 ③ · ⑤).
//!
//! **나무** — [`DebugSession`] 은 `debug.start` 하나다. 뿌리 세션 하나와, 어댑터가 역요청 `startDebugging` 으로 연 자식 세션들
//! (js-debug 는 디버그 대상 프로세스마다 자식 세션을 연다)을 쥔다. 자식은 같은 어댑터에 새로 붙는다 — stdio 어댑터면 같은
//! 정의로 프로세스를 하나 더 띄우고, TCP 어댑터면 같은 포트에 연결을 하나 더 연다. 세션마다 [`SessionId`] 가 있고 이벤트는 그
//! id 를 달고 한 채널([`SessionOptions::events`])로 온다.
//!
//! **시작 순서**(⑤ — 어댑터마다 `initialized` 이벤트를 내는 때가 다르다): `initialize` 응답 → `launch`/`attach` 를 **보내고 답을
//! 기다리지 않는다** → `initialized` 이벤트를 기다린다 → `setBreakpoints`(소스마다) · `setFunctionBreakpoints` ·
//! `setExceptionBreakpoints` → `configurationDone` → 그제야 `launch`/`attach` 의 답. `initialized` 전에는 브레이크포인트를 보내지
//! 않는다. `launch` 의 답을 `configurationDone` 뒤로 미루는 어댑터(debugpy)도, `initialized` 를 `initialize` 응답 직후에 내는
//! 어댑터(codelldb)도 이 순서로 된다.
//!
//! **끝** — 세션은 우리가 끊거나([`Session::disconnect`]), 어댑터가 연결을 닫거나 죽거나, 프로토콜을 어겨 우리가 죽이면 끝난다.
//! 끝나면 [`SessionEventKind::Ended`] 가 한 번 나고, 그 세션의 자식도 끝난다. 뿌리가 끝나면 허가([`DebugGrant`])를 거둔다 — 그
//! 뒤의 `runInTerminal` 은 거절이다. 손잡이([`DebugSession`])를 버리면 어댑터를 모두 트리째 끝낸다.
//!
//! **상한** — 이벤트 채널은 부른 쪽이 정한 크기다. 꽉 차면 `output` 은 버리고(센다 — [`DebugSession::dropped_output`]),
//! `Ended` 도 버린다([`Session::ended`] 로 알 수 있다). 그 밖의 이벤트(정지 등)를 넘기지 못하면 그 세션의 어댑터를 끝낸다 —
//! 정지를 잃은 디버그 화면이 틀린 상태를 보이지 않게. 역요청은 세션마다 동시에 [`MAX_REVERSE_REQUESTS`] 개, 자식 세션은 나무마다
//! [`MAX_CHILD_SESSIONS`] 개까지다.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

use crate::adapter::{
    AdapterSpec, LOOPBACK, LaunchError, Transport, free_port, socket_args, tcp_args,
};
use crate::connection::{Closed, Connection, Inbound, RequestError};
use crate::grant::DebugGrant;
use crate::process::AdapterProcess;
use crate::protocol::{
    self, Breakpoint, BreakpointEvent, Capabilities, Command, ContinuedEvent, Event, ExitedEvent,
    FunctionBreakpoint, InitializeArguments, OutputEvent, RunInTerminalRequestArguments,
    RunInTerminalResponse, SetBreakpointsArguments, SetExceptionBreakpointsArguments,
    SetFunctionBreakpointsArguments, StartDebuggingRequestArguments, StartMode, StoppedEvent,
    TerminatedEvent, ThreadEvent,
};
use crate::socket::{self, Attempt, Halves, SocketPath};
use crate::terminal::Terminals;

/// 세션 하나에서 동시에 처리하는 역요청 수. 넘으면 곧바로 실패로 답한다.
pub const MAX_REVERSE_REQUESTS: usize = 16;
/// 나무 하나의 자식 세션 수(끝난 것 포함). 넘으면 `startDebugging` 을 실패로 답한다.
pub const MAX_CHILD_SESSIONS: usize = 32;

/// 세션의 id(나무 안에서 유일, 뿌리는 1).
pub type SessionId = u64;

/// 기다리는 시간들.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// 요청 하나의 기본 시한([`Session::request`]).
    pub request: Duration,
    /// `initialize` 의 답.
    pub initialize: Duration,
    /// `launch`/`attach` 를 보낸 뒤 `initialized` 이벤트까지.
    pub initialized: Duration,
    /// `launch`/`attach` 의 답(`configurationDone` 뒤까지 — 디버그 대상이 뜨는 시간이 든다).
    pub launch: Duration,
    /// TCP 어댑터가 포트를 열기까지.
    pub connect: Duration,
    /// `disconnect` 의 답.
    pub disconnect: Duration,
    /// 끊은 뒤 어댑터가 스스로 끝나기를 기다리는 시간. 넘으면 트리째 끝낸다.
    pub exit_grace: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(10),
            initialize: Duration::from_secs(10),
            initialized: Duration::from_secs(30),
            launch: Duration::from_secs(60),
            connect: Duration::from_secs(10),
            disconnect: Duration::from_secs(2),
            exit_grace: Duration::from_secs(1),
        }
    }
}

/// [`DebugSession::start`] 의 선택.
pub struct SessionOptions {
    /// 이벤트를 받을 곳(크기는 부른 쪽이 정한다 — 머리 주석의 「상한」). 나무마다 따로 만든다.
    pub events: mpsc::Sender<SessionEvent>,
    /// `runInTerminal` 으로 디버그 대상을 띄울 실행 관리자.
    pub terminals: Arc<dyn Terminals>,
    /// `initialize` 의 `locale`(UI 언어).
    pub locale: Option<String>,
    pub timeouts: Timeouts,
}

impl fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionOptions")
            .field("locale", &self.locale)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

/// 시작할 때 보낼 브레이크포인트.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitialBreakpoints {
    /// 소스마다 하나(`setBreakpoints`).
    pub sources: Vec<SetBreakpointsArguments>,
    /// 어댑터가 함수 브레이크포인트를 알리면 보낸다.
    pub functions: Vec<FunctionBreakpoint>,
    /// 켤 예외 필터. 어댑터가 `initialize` 응답으로 알린 것만 보낸다(나머지는 버리고 로그만).
    pub exception_filters: Vec<String>,
}

/// 시작할 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    pub mode: StartMode,
    /// `launch`/`attach` 의 인자(어댑터 정의의 틀에 구성의 값을 채운 것 — M6-16 · M6-17).
    pub arguments: Map<String, Value>,
    pub breakpoints: InitialBreakpoints,
}

/// 시작이 정한 것: 어댑터의 능력과 브레이크포인트 응답(어댑터가 옮긴 줄 · `verified`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configured {
    pub capabilities: Capabilities,
    /// [`InitialBreakpoints::sources`] 와 같은 차례로 소스마다의 응답.
    pub sources: Vec<Vec<Breakpoint>>,
    pub functions: Vec<Breakpoint>,
    /// 실제로 보낸 예외 필터.
    pub exception_filters: Vec<String>,
}

#[derive(Debug)]
pub enum StartError {
    /// 허가가 없다(거둔 허가 · 루트 밖 작업 디렉터리).
    Refused(String),
    Launch(LaunchError),
    /// 시작 순서 중 요청이 실패했다(어느 요청인가).
    Request(&'static str, RequestError),
    /// `initialized` 이벤트가 오지 않았다.
    NotInitialized,
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(why) => write!(f, "refused: {why}"),
            Self::Launch(e) => e.fmt(f),
            Self::Request(step, e) => write!(f, "`{step}` failed: {e}"),
            Self::NotInitialized => {
                f.write_str("the debug adapter did not send `initialized` in time")
            }
        }
    }
}

impl std::error::Error for StartError {}

/// 세션이 끝난 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// 우리가 끊었다.
    Stopped,
    /// 어댑터가 연결을 닫았거나 끝났다(까닭).
    Gone(String),
    /// 프로토콜 위반으로 우리가 죽였다(까닭 — 틀 상한 초과 등).
    Killed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub reason: EndReason,
    /// 이 세션이 띄운 어댑터 프로세스의 종료 코드(신호로 끝났거나 프로세스가 없으면 없다).
    pub exit_code: Option<i32>,
}

/// 부른 쪽이 받는 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    pub session: SessionId,
    pub kind: SessionEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEventKind {
    Stopped(StoppedEvent),
    Continued(ContinuedEvent),
    Output(OutputEvent),
    /// 디버그가 끝났다(어댑터가 알렸다 — 연결은 아직 열려 있을 수 있다. 끊기는 부른 쪽이 한다).
    Terminated(TerminatedEvent),
    /// 디버그 대상이 끝났다.
    Exited(ExitedEvent),
    Breakpoint(BreakpointEvent),
    Thread(ThreadEvent),
    /// 어댑터가 `startDebugging` 으로 자식 세션을 열었다(이 이벤트의 `session` 이 부모).
    ChildStarted {
        child: SessionId,
    },
    /// 이 세션이 끝났다(한 번).
    Ended(Ended),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 붙는 자리(포트 · 소켓 경로).
#[derive(Clone)]
enum Endpoint {
    Port(u16),
    /// 버려지면(나무가 끝나면) 폴더째 지운다.
    Socket(Arc<SocketPath>),
}

/// TCP · 소켓 어댑터: 뿌리가 띄운 프로세스와 그 자리(자식은 여기에 붙는다).
struct SharedAdapter {
    endpoint: Endpoint,
    process: Option<Arc<AdapterProcess>>,
}

struct Tree {
    grant: DebugGrant,
    adapter: AdapterSpec,
    events: mpsc::Sender<SessionEvent>,
    terminals: Arc<dyn Terminals>,
    locale: Option<String>,
    timeouts: Timeouts,
    /// 자식에게도 보낼 처음 브레이크포인트.
    breakpoints: InitialBreakpoints,
    shared: Mutex<Option<SharedAdapter>>,
    sessions: Mutex<BTreeMap<SessionId, Arc<Session>>>,
    next_id: AtomicU64,
    children: AtomicUsize,
    dropped_output: AtomicU64,
}

impl Tree {
    /// 이벤트를 넘긴다(머리 주석의 「상한」).
    fn emit(&self, session: &Session, kind: SessionEventKind) {
        let event = SessionEvent {
            session: session.id,
            kind,
        };
        let Err(mpsc::error::TrySendError::Full(event)) = self.events.try_send(event) else {
            return;
        };
        match event.kind {
            SessionEventKind::Output(_) => {
                self.dropped_output.fetch_add(1, Ordering::SeqCst);
            }
            SessionEventKind::Ended(_) => {}
            other => session.connection.violate(format!(
                "debug events are not being handled fast enough (queue full at {other:?})"
            )),
        }
    }

    fn session(&self, id: SessionId) -> Option<Arc<Session>> {
        lock(&self.sessions).get(&id).cloned()
    }

    fn children_of(&self, id: SessionId) -> Vec<Arc<Session>> {
        lock(&self.sessions)
            .values()
            .filter(|s| s.parent == Some(id))
            .cloned()
            .collect()
    }

    /// 어댑터에 새로 붙는다(stdio — 프로세스를 띄운다, TCP — 뿌리면 띄우고 붙고 자식이면 그 포트에 붙는다).
    async fn link(
        self: &Arc<Self>,
        root: bool,
    ) -> Result<
        (
            Connection,
            mpsc::Receiver<Inbound>,
            Option<Arc<AdapterProcess>>,
            bool,
        ),
        StartError,
    > {
        let label = self.adapter.adapter_id.clone();
        match &self.adapter.transport {
            Transport::Stdio => {
                let spawned = AdapterProcess::spawn(&self.adapter, self.adapter.args.clone(), true)
                    .await
                    .map_err(|e| StartError::Launch(LaunchError::Io(e)))?;
                let (Some(stdin), Some(stdout)) = (spawned.stdin, spawned.stdout) else {
                    return Err(StartError::Launch(LaunchError::Io(std::io::Error::other(
                        "the adapter has no stdio pipes",
                    ))));
                };
                let process = spawned.process;
                let killer = Arc::clone(&process);
                let (connection, inbound) =
                    Connection::open(label, stdout, stdin, Box::new(move || killer.kill()));
                Ok((connection, inbound, Some(process), true))
            }
            Transport::Tcp { .. } | Transport::Socket { .. } => {
                let existing = lock(&self.shared)
                    .as_ref()
                    .map(|t| (t.endpoint.clone(), t.process.clone()));
                let (endpoint, process, owns) = match existing {
                    Some((endpoint, process)) if !root => (endpoint, process, false),
                    _ => {
                        let (endpoint, args) = self.endpoint()?;
                        let spawned = AdapterProcess::spawn(&self.adapter, args, false)
                            .await
                            .map_err(|e| StartError::Launch(LaunchError::Io(e)))?;
                        *lock(&self.shared) = Some(SharedAdapter {
                            endpoint: endpoint.clone(),
                            process: Some(Arc::clone(&spawned.process)),
                        });
                        (endpoint, Some(spawned.process), true)
                    }
                };
                let (read, write) =
                    connect(&endpoint, process.as_deref(), self.timeouts.connect).await?;
                let killer = process.clone();
                let (connection, inbound) = Connection::open(
                    label,
                    read,
                    write,
                    Box::new(move || {
                        if let Some(p) = &killer {
                            p.kill();
                        }
                    }),
                );
                Ok((connection, inbound, process, owns))
            }
            Transport::Connect { port } => {
                let endpoint = Endpoint::Port(*port);
                *lock(&self.shared) = Some(SharedAdapter {
                    endpoint: endpoint.clone(),
                    process: None,
                });
                let (read, write) = connect(&endpoint, None, self.timeouts.connect).await?;
                let (connection, inbound) = Connection::open(label, read, write, Box::new(|| {}));
                Ok((connection, inbound, None, false))
            }
        }
    }

    /// 뿌리가 띄울 어댑터의 자리와 그 argv(프로그램 다음).
    fn endpoint(&self) -> Result<(Endpoint, Vec<std::ffi::OsString>), StartError> {
        match &self.adapter.transport {
            Transport::Tcp { port_args } => {
                let port = free_port().map_err(|e| StartError::Launch(LaunchError::Io(e)))?;
                let args =
                    tcp_args(&self.adapter.args, port_args, port).map_err(StartError::Launch)?;
                Ok((Endpoint::Port(port), args))
            }
            Transport::Socket {
                socket_args: template,
            } => {
                let path = SocketPath::new().map_err(|e| StartError::Launch(LaunchError::Io(e)))?;
                let args = socket_args(&self.adapter.args, template, path.path())
                    .map_err(StartError::Launch)?;
                Ok((Endpoint::Socket(Arc::new(path)), args))
            }
            Transport::Stdio | Transport::Connect { .. } => {
                Err(StartError::Launch(LaunchError::Io(std::io::Error::other(
                    "this adapter has no address to listen on",
                ))))
            }
        }
    }

    /// 세션 하나를 열고 시작 순서를 지난다. 실패하면 그 세션을 끝낸다.
    async fn open(
        self: &Arc<Self>,
        parent: Option<SessionId>,
        request: StartRequest,
    ) -> Result<(Arc<Session>, Configured), StartError> {
        let (connection, inbound, process, owns_process) = self.link(parent.is_none()).await?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let session = Arc::new(Session {
            id,
            parent,
            tree: Arc::downgrade(self),
            connection,
            process,
            owns_process,
            capabilities: Mutex::new(Capabilities::default()),
            initialized: watch::channel(false).0,
            ending: AtomicBool::new(false),
            ended: watch::channel(None).0,
            serving: AtomicUsize::new(0),
        });
        lock(&self.sessions).insert(id, Arc::clone(&session));
        tokio::spawn(pump(Arc::clone(self), Arc::clone(&session), inbound));
        // 어댑터 프로세스가 끝나면 그 프로세스에 붙은 연결을 닫는다(리뷰 F1 b) — 연결의 상대가 어댑터가 아니어도(포트를 먼저
        // 차지한 다른 프로세스) 세션이 어댑터보다 오래 살지 않는다.
        if let Some(process) = session.process.clone() {
            let connection = session.connection.clone();
            tokio::spawn(async move {
                let exit = process.wait().await;
                connection.close(Closed::Gone(format!(
                    "the debug adapter exited (code {:?})",
                    exit.code
                )));
            });
        }
        if let Some(parent) = parent.and_then(|p| self.session(p)) {
            self.emit(&parent, SessionEventKind::ChildStarted { child: id });
        }
        match configure(self, &session, request).await {
            Ok(configured) => Ok((session, configured)),
            Err(e) => {
                tracing::info!(target: "pw_dap", adapter = %self.adapter.adapter_id, session = id, %e, "debug session failed to start");
                session.shut(None, self.timeouts).await;
                Err(e)
            }
        }
    }

    /// 역요청 `startDebugging`.
    async fn start_child(
        self: &Arc<Self>,
        parent: SessionId,
        args: Value,
    ) -> Result<Value, String> {
        let args: StartDebuggingRequestArguments = serde_json::from_value(args)
            .map_err(|e| format!("bad startDebugging arguments: {e}"))?;
        self.grant.check_start_child(self.parent_ending(parent))?;
        if self.children.fetch_add(1, Ordering::SeqCst) >= MAX_CHILD_SESSIONS {
            self.children.fetch_sub(1, Ordering::SeqCst);
            return Err(format!("at most {MAX_CHILD_SESSIONS} child sessions"));
        }
        let request = StartRequest {
            mode: args.request,
            arguments: args.configuration,
            breakpoints: self.breakpoints.clone(),
        };
        let (child, _) = self
            .open(Some(parent), request)
            .await
            .map_err(|e| e.to_string())?;
        // 여는 사이 부모 · 나무가 끝났으면(그 정리에 들지 못했다) 곧바로 끊는다.
        if let Err(why) = self.grant.check_start_child(self.parent_ending(parent)) {
            child.shut(None, self.timeouts).await;
            return Err(why);
        }
        Ok(Value::Object(Map::new()))
    }

    /// 그 세션이 끊는 중이거나 끝났는가(없으면 끝난 것).
    fn parent_ending(&self, id: SessionId) -> bool {
        self.session(id)
            .is_none_or(|s| s.ending.load(Ordering::SeqCst) || s.ended.borrow().is_some())
    }
}

/// 그 자리에 붙는다. 어댑터가 열기까지 되풀이하되, 그 사이 어댑터가 끝나면 그만둔다. 소켓의 상대가 어댑터가 아니면 붙지 않고
/// 어댑터를 끝낸다(`crate::socket`).
async fn connect(
    endpoint: &Endpoint,
    process: Option<&AdapterProcess>,
    limit: Duration,
) -> Result<Halves, StartError> {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let attempt = match endpoint {
            Endpoint::Port(port) => match TcpStream::connect((LOOPBACK, *port)).await {
                Ok(stream) => {
                    let (read, write) = stream.into_split();
                    Attempt::Connected((Box::new(read), Box::new(write)))
                }
                Err(e) => Attempt::NotYet(e),
            },
            Endpoint::Socket(path) => {
                socket::connect_once(path.path(), process.and_then(AdapterProcess::pid)).await
            }
        };
        let e = match attempt {
            Attempt::Connected(halves) => return Ok(halves),
            Attempt::Impostor(why) => {
                tracing::warn!(target: "pw_dap", %why, "refused to connect to the debug adapter's socket");
                if let Some(p) = process {
                    p.kill();
                }
                return Err(StartError::Launch(LaunchError::Impostor(why)));
            }
            Attempt::NotYet(e) => e,
        };
        if let Some(exit) = process.and_then(AdapterProcess::exited) {
            return Err(StartError::Launch(LaunchError::Io(std::io::Error::other(
                format!(
                    "the debug adapter exited (code {:?}) before it started listening",
                    exit.code
                ),
            ))));
        }
        if tokio::time::Instant::now() >= deadline {
            if let Some(p) = process {
                p.kill();
            }
            return Err(StartError::Launch(LaunchError::Io(e)));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// 시작 순서(머리 주석).
async fn configure(
    tree: &Arc<Tree>,
    session: &Arc<Session>,
    request: StartRequest,
) -> Result<Configured, StartError> {
    let t = tree.timeouts;
    let init = InitializeArguments {
        client_id: Some("pitwall".to_owned()),
        client_name: Some("Pitwall".to_owned()),
        adapter_id: tree.adapter.adapter_id.clone(),
        locale: tree.locale.clone(),
        lines_start_at1: true,
        columns_start_at1: true,
        path_format: "path".to_owned(),
        supports_variable_type: true,
        supports_variable_paging: true,
        supports_run_in_terminal_request: true,
        supports_start_debugging_request: true,
        supports_args_can_be_interpreted_by_shell: false,
        supports_ansi_styling: false,
    };
    let capabilities = session
        .request_with::<protocol::Initialize>(&init, t.initialize)
        .await
        .map_err(|e| StartError::Request("initialize", e))?;
    *lock(&session.capabilities) = capabilities.clone();
    // `launch`/`attach` 는 보내 두고 답은 `configurationDone` 뒤에 본다.
    let mut launch = {
        let connection = session.connection.clone();
        let command = request.mode.command();
        let arguments = Value::Object(request.arguments);
        let limit = t.launch;
        tokio::spawn(async move { connection.request(command, &arguments, limit).await })
    };
    let mut initialized = session.initialized.subscribe();
    let mut launch_done: Option<Result<Value, RequestError>> = None;
    let wait = async {
        loop {
            if *initialized.borrow_and_update() {
                return Ok(());
            }
            tokio::select! {
                changed = initialized.changed() => {
                    if changed.is_err() {
                        return Err(StartError::NotInitialized);
                    }
                }
                done = &mut launch, if launch_done.is_none() => {
                    let done = done.unwrap_or_else(|e| Err(RequestError::Closed(e.to_string())));
                    if let Err(e) = &done {
                        return Err(StartError::Request(request.mode.command(), e.clone()));
                    }
                    launch_done = Some(done);
                }
            }
        }
    };
    match tokio::time::timeout(t.initialized, wait).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(session
                .connection
                .closed()
                .map_or(StartError::NotInitialized, |c| {
                    StartError::Request(
                        request.mode.command(),
                        RequestError::Closed(close_text(&c)),
                    )
                }));
        }
    }
    let mut sources = Vec::with_capacity(request.breakpoints.sources.len());
    for source in &request.breakpoints.sources {
        let body = session
            .request::<protocol::SetBreakpoints>(source)
            .await
            .map_err(|e| StartError::Request("setBreakpoints", e))?;
        sources.push(body.breakpoints);
    }
    let mut functions = Vec::new();
    if capabilities.supports_function_breakpoints && !request.breakpoints.functions.is_empty() {
        functions = session
            .request::<protocol::SetFunctionBreakpoints>(&SetFunctionBreakpointsArguments {
                breakpoints: request.breakpoints.functions.clone(),
            })
            .await
            .map_err(|e| StartError::Request("setFunctionBreakpoints", e))?
            .breakpoints;
    }
    let exception_filters: Vec<String> = request
        .breakpoints
        .exception_filters
        .iter()
        .filter(|f| {
            let known = capabilities
                .exception_breakpoint_filters
                .iter()
                .any(|k| &k.filter == *f);
            if !known {
                tracing::debug!(target: "pw_dap", filter = %f, "the adapter did not announce this exception filter; not sent");
            }
            known
        })
        .cloned()
        .collect();
    if !capabilities.exception_breakpoint_filters.is_empty() {
        session
            .request::<protocol::SetExceptionBreakpoints>(&SetExceptionBreakpointsArguments {
                filters: exception_filters.clone(),
            })
            .await
            .map_err(|e| StartError::Request("setExceptionBreakpoints", e))?;
    }
    if capabilities.supports_configuration_done_request {
        session
            .request::<protocol::ConfigurationDone>(&protocol::Empty {})
            .await
            .map_err(|e| StartError::Request("configurationDone", e))?;
    }
    let launched = match launch_done {
        Some(done) => done,
        None => launch
            .await
            .unwrap_or_else(|e| Err(RequestError::Closed(e.to_string()))),
    };
    launched.map_err(|e| StartError::Request(request.mode.command(), e))?;
    Ok(Configured {
        capabilities,
        sources,
        functions,
        exception_filters,
    })
}

fn close_text(closed: &Closed) -> String {
    match closed {
        Closed::Stopped => "the debug session was stopped".to_owned(),
        Closed::Gone(why) | Closed::Killed(why) => why.clone(),
    }
}

/// 세션 하나의 이벤트 · 역요청을 처리하고, 연결이 닫히면 끝을 알린다.
async fn pump(tree: Arc<Tree>, session: Arc<Session>, mut inbound: mpsc::Receiver<Inbound>) {
    while let Some(message) = inbound.recv().await {
        match message {
            Inbound::Event { event, body } => match Event::parse(&event, body) {
                Ok(Some(event)) => match kind_of(event) {
                    Some(kind) => tree.emit(&session, kind),
                    None => {
                        session.initialized.send_replace(true);
                    }
                },
                Ok(None) => {
                    tracing::debug!(target: "pw_dap", adapter = %tree.adapter.adapter_id, session = session.id, %event, "ignored an unknown event");
                }
                Err(e) => {
                    tracing::debug!(target: "pw_dap", adapter = %tree.adapter.adapter_id, session = session.id, %e, "dropped an unreadable event");
                }
            },
            Inbound::Request {
                seq,
                command,
                arguments,
            } => reverse(&tree, &session, seq, command, arguments),
        }
    }
    // 읽기가 끝났다 — 연결을 닫고(이미 닫혔으면 그 까닭이 남는다), 이 세션이 띄운 어댑터가 끝나기를 기다린다.
    session.connection.close(Closed::Gone(
        "the debug adapter closed the connection".to_owned(),
    ));
    let closed = session.connection.wait_closed().await;
    let exit_code = match (&session.process, session.owns_process) {
        (Some(process), true) => {
            let exit = match tokio::time::timeout(tree.timeouts.exit_grace, process.wait()).await {
                Ok(exit) => exit,
                Err(_) => {
                    process.kill();
                    process.wait().await
                }
            };
            exit.code
        }
        _ => None,
    };
    let reason = match closed {
        Closed::Killed(why) => EndReason::Killed(why),
        _ if session.ending.load(Ordering::SeqCst) => EndReason::Stopped,
        Closed::Stopped => EndReason::Stopped,
        Closed::Gone(why) => EndReason::Gone(match exit_code {
            Some(code) => format!("{why} (adapter exit code {code})"),
            None => why,
        }),
    };
    let ended = Ended { reason, exit_code };
    if session.parent.is_none() {
        tree.grant.revoke();
    }
    session.ended.send_replace(Some(ended.clone()));
    tree.emit(&session, SessionEventKind::Ended(ended));
    // 자식도 끝낸다.
    for child in tree.children_of(session.id) {
        child.ending.store(true, Ordering::SeqCst);
        child.connection.close(Closed::Stopped);
    }
}

/// 부른 쪽에 넘길 이벤트. `initialized` 는 시작 순서의 것이라 넘기지 않는다(`None`).
fn kind_of(event: Event) -> Option<SessionEventKind> {
    Some(match event {
        Event::Initialized => return None,
        Event::Stopped(e) => SessionEventKind::Stopped(e),
        Event::Continued(e) => SessionEventKind::Continued(e),
        Event::Output(e) => SessionEventKind::Output(e),
        Event::Terminated(e) => SessionEventKind::Terminated(e),
        Event::Exited(e) => SessionEventKind::Exited(e),
        Event::Breakpoint(e) => SessionEventKind::Breakpoint(e),
        Event::Thread(e) => SessionEventKind::Thread(e),
    })
}

/// 역요청 하나. 처리는 태스크에서 하고(이벤트를 막지 않게) 답한다.
fn reverse(tree: &Arc<Tree>, session: &Arc<Session>, seq: i64, command: String, arguments: Value) {
    if session.serving.fetch_add(1, Ordering::SeqCst) >= MAX_REVERSE_REQUESTS {
        session.serving.fetch_sub(1, Ordering::SeqCst);
        session.connection.respond(
            seq,
            &command,
            &Err("too many debug adapter requests in flight".to_owned()),
        );
        return;
    }
    let (tree, session) = (Arc::clone(tree), Arc::clone(session));
    tokio::spawn(async move {
        let result = match command.as_str() {
            "runInTerminal" => run_in_terminal(&tree, &session, arguments).await,
            "startDebugging" => tree.start_child(session.id, arguments).await,
            other => Err(format!("unsupported request `{other}`")),
        };
        if let Err(why) = &result {
            tracing::info!(target: "pw_dap", adapter = %tree.adapter.adapter_id, session = session.id, %command, %why, "refused a debug adapter request");
        }
        session.connection.respond(seq, &command, &result);
        session.serving.fetch_sub(1, Ordering::SeqCst);
    });
}

async fn run_in_terminal(
    tree: &Tree,
    session: &Session,
    arguments: Value,
) -> Result<Value, String> {
    let args: RunInTerminalRequestArguments = serde_json::from_value(arguments)
        .map_err(|e| format!("bad runInTerminal arguments: {e}"))?;
    let ending = session.ending.load(Ordering::SeqCst) || session.ended.borrow().is_some();
    let run = tree.grant.check_run_in_terminal(ending, args)?;
    let started = tree
        .terminals
        .run_in_terminal(tree.grant.exec(), run)
        .await?;
    serde_json::to_value(RunInTerminalResponse {
        process_id: started.process_id.map(i64::from),
        shell_process_id: started.shell_process_id.map(i64::from),
    })
    .map_err(|e| e.to_string())
}

/// 어댑터에 붙은 세션 하나.
pub struct Session {
    id: SessionId,
    parent: Option<SessionId>,
    tree: Weak<Tree>,
    connection: Connection,
    process: Option<Arc<AdapterProcess>>,
    /// 이 세션이 프로세스를 띄웠다(stdio 의 모든 세션 · TCP 의 뿌리). 끊을 때 그 프로세스가 끝나기를 기다리고 넘으면 죽인다.
    owns_process: bool,
    capabilities: Mutex<Capabilities>,
    initialized: watch::Sender<bool>,
    /// 끊기를 시작했다 — 역요청을 거절한다.
    ending: AtomicBool,
    ended: watch::Sender<Option<Ended>>,
    serving: AtomicUsize,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("parent", &self.parent)
            .field("pid", &self.pid())
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn parent(&self) -> Option<SessionId> {
        self.parent
    }

    /// 어댑터 프로세스의 pid(unix 는 그 프로세스 그룹 id 와 같다). 이미 열린 포트에 붙었으면 없다.
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().and_then(|p| p.pid())
    }

    /// `initialize` 응답의 능력.
    pub fn capabilities(&self) -> Capabilities {
        lock(&self.capabilities).clone()
    }

    /// 끝났으면 그 모양.
    pub fn ended(&self) -> Option<Ended> {
        self.ended.borrow().clone()
    }

    /// 끝날 때까지 기다린다.
    pub async fn wait_ended(&self) -> Ended {
        let mut rx = self.ended.subscribe();
        loop {
            if let Some(ended) = rx.borrow_and_update().clone() {
                return ended;
            }
            if rx.changed().await.is_err() {
                return Ended {
                    reason: EndReason::Gone("the session is gone".to_owned()),
                    exit_code: None,
                };
            }
        }
    }

    /// 요청 하나(시한은 [`Timeouts::request`]).
    pub async fn request<C: Command>(
        &self,
        arguments: &C::Arguments,
    ) -> Result<C::Body, RequestError> {
        let limit = self
            .tree
            .upgrade()
            .map_or_else(|| Timeouts::default().request, |t| t.timeouts.request);
        self.request_with::<C>(arguments, limit).await
    }

    pub async fn request_with<C: Command>(
        &self,
        arguments: &C::Arguments,
        timeout: Duration,
    ) -> Result<C::Body, RequestError> {
        let arguments = to_value(arguments)?;
        let body = self.request_raw(C::NAME, arguments, timeout).await?;
        // 본문은 선택이다 — 빠진 본문(능력이 없는 `initialize` 응답 등)은 빈 객체로 읽는다.
        let body = if body.is_null() {
            Value::Object(Map::new())
        } else {
            body
        };
        serde_json::from_value(body).map_err(|e| RequestError::Decode(format!("{}: {e}", C::NAME)))
    }

    /// 이름과 JSON 인자로(쓰는 타입이 없는 요청 — `source` · `exceptionInfo` 등).
    pub async fn request_raw(
        &self,
        command: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        self.connection.request(command, &arguments, timeout).await
    }

    /// 표에 없어 버린 응답의 수(시한을 넘긴 요청의 늦은 답).
    pub fn late_responses(&self) -> u64 {
        self.connection.late_responses()
    }

    /// DAP 메시지가 아니어서 버린 본문의 수.
    pub fn unreadable_messages(&self) -> u64 {
        self.connection.unreadable_messages()
    }

    /// 이 세션과 그 자식을 끊는다: 자식부터 끊고, `disconnect`(시한 [`Timeouts::disconnect`])를 보내고 연결을 닫은 뒤, 이 세션이
    /// 띄운 어댑터가 [`Timeouts::exit_grace`] 안에 끝나지 않으면 트리째 끝낸다. 끝날 때까지 기다린다.
    pub async fn disconnect(&self, terminate_debuggee: Option<bool>) -> Ended {
        let timeouts = self
            .tree
            .upgrade()
            .map_or_else(Timeouts::default, |t| t.timeouts);
        self.shut(terminate_debuggee, timeouts).await
    }

    async fn shut(&self, terminate_debuggee: Option<bool>, timeouts: Timeouts) -> Ended {
        self.ending.store(true, Ordering::SeqCst);
        let tree = self.tree.upgrade();
        if let Some(tree) = &tree {
            if self.parent.is_none() {
                tree.grant.revoke();
            }
            // 자식은 몇 개뿐이고 각자 시한이 있다 — 차례로 끊어도 상한이 있다.
            for child in tree.children_of(self.id) {
                Box::pin(child.shut(terminate_debuggee, timeouts)).await;
            }
        }
        if self.connection.closed().is_none() {
            let args = protocol::DisconnectArguments {
                terminate_debuggee,
                ..protocol::DisconnectArguments::default()
            };
            let _ = self
                .request_with::<protocol::Disconnect>(&args, timeouts.disconnect)
                .await;
            self.connection.close(Closed::Stopped);
        }
        let wait = timeouts.exit_grace + timeouts.disconnect;
        match tokio::time::timeout(wait, self.wait_ended()).await {
            Ok(ended) => ended,
            Err(_) => {
                if let Some(p) = &self.process {
                    p.kill();
                }
                self.wait_ended().await
            }
        }
    }
}

fn to_value<T: Serialize>(value: &T) -> Result<Value, RequestError> {
    serde_json::to_value(value).map_err(|e| RequestError::Decode(e.to_string()))
}

/// `debug.start` 하나 — 세션 나무(머리 주석).
pub struct DebugSession {
    tree: Arc<Tree>,
    root: Arc<Session>,
}

impl fmt::Debug for DebugSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DebugSession")
            .field("project", &self.tree.grant.project_id())
            .field("adapter", &self.tree.adapter.adapter_id)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl DebugSession {
    /// 어댑터를 띄우고(또는 붙고) 시작 순서를 지난다. 허가([`DebugGrant`])가 있어야 부를 수 있다 — 어댑터의 작업 디렉터리가 그
    /// 루트 안이어야 하고 거둔 허가면 거절이다(아무것도 띄우지 않는다). 실패하면 띄운 어댑터를 끝내고 오류를 돌려준다.
    pub async fn start(
        grant: DebugGrant,
        adapter: AdapterSpec,
        request: StartRequest,
        options: SessionOptions,
    ) -> Result<(Self, Configured), StartError> {
        if grant.revoked() {
            return Err(StartError::Refused(
                "the debug grant was revoked".to_owned(),
            ));
        }
        if !matches!(adapter.transport, Transport::Connect { .. }) && !grant.contains(&adapter.cwd)
        {
            return Err(StartError::Refused(format!(
                "{} is outside the project root {}",
                adapter.cwd.display(),
                grant.root().display()
            )));
        }
        let tree = Arc::new(Tree {
            grant,
            adapter,
            events: options.events,
            terminals: options.terminals,
            locale: options.locale,
            timeouts: options.timeouts,
            breakpoints: request.breakpoints.clone(),
            shared: Mutex::new(None),
            sessions: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            children: AtomicUsize::new(0),
            dropped_output: AtomicU64::new(0),
        });
        let (root, configured) = match tree.open(None, request).await {
            Ok(opened) => opened,
            Err(e) => {
                tree.grant.revoke();
                return Err(e);
            }
        };
        tracing::info!(target: "pw_dap", project = tree.grant.project_id(), adapter = %tree.adapter.adapter_id, pid = ?root.pid(), "debug session started");
        Ok((Self { tree, root }, configured))
    }

    pub fn root(&self) -> &Arc<Session> {
        &self.root
    }

    pub fn grant(&self) -> &DebugGrant {
        &self.tree.grant
    }

    /// 나무의 세션 하나(끝난 자식도 남는다).
    pub fn session(&self, id: SessionId) -> Option<Arc<Session>> {
        self.tree.session(id)
    }

    /// 나무의 모든 세션(id 차례 — 뿌리가 먼저).
    pub fn sessions(&self) -> Vec<Arc<Session>> {
        lock(&self.tree.sessions).values().cloned().collect()
    }

    /// 이벤트 채널이 꽉 차 버린 `output` 이벤트 수.
    pub fn dropped_output(&self) -> u64 {
        self.tree.dropped_output.load(Ordering::SeqCst)
    }

    /// 나무 전체를 끊는다([`Session::disconnect`] 를 뿌리에).
    pub async fn disconnect(&self, terminate_debuggee: Option<bool>) -> Ended {
        self.root.disconnect(terminate_debuggee).await
    }
}

impl Drop for DebugSession {
    fn drop(&mut self) {
        // 끊지 않고 버렸다 — 허가를 거두고 어댑터를 모두 트리째 끝낸다.
        self.tree.grant.revoke();
        for session in lock(&self.tree.sessions).values() {
            session.ending.store(true, Ordering::SeqCst);
            session.connection.close(Closed::Stopped);
            if let Some(p) = &session.process {
                p.kill();
            }
        }
    }
}
