//! 언어 서버 하나의 연결과 수명(15-m4-plan.md M4-01 ① · ② · ④).
//!
//! **띄우기** — [`LanguageServer::spawn`] 은 [`LspGrant`] 를 받아야 열린다(결정 M4-D8 — `crate::grant` 머리 주석). 서버는 자기 프로세스
//! 그룹(Windows 는 Job Object)으로 뜬다(`crate::process`).
//!
//! **태스크 넷** — 쓰기(보낼 틀을 stdin 으로), 읽기(stdout 의 틀을 가른다), stderr(줄마다 서버 로그로), 기다리기(서버가 끝나면 남은
//! 자손을 끝내고 대기 중인 요청을 모두 실패시킨다). 읽기가 틀 오류(머리 8 KiB · 본문 64 MiB 를 넘음 등)를 보면 그 서버를 트리째
//! 죽인다 — 서버 하나가 Host 를 막지 않는다.
//!
//! **요청** — id 는 연결마다 1 부터 오르는 수다. 요청마다 시한(기본 [`DEFAULT_REQUEST_TIMEOUT`])과 취소 토큰([`RequestOptions`])이
//! 있다. 시한이 지나거나, 토큰이 취소되거나, 부른 쪽이 기다리던 future 를 버리면 표에서 지우고 `$/cancelRequest` 를 보낸다. 그 뒤에
//! 오는 응답은 표에 없으므로 버린다([`LanguageServer::late_responses`] 가 센다).
//!
//! **서버 → 클라이언트 요청** — [`Handlers`] 에 단 처리기가 먼저, 없으면 이 행이 답하는 것: `window/workDoneProgress/create`(`null`),
//! `window/showMessageRequest`(버튼 없이 [`ServerEvent::ShowMessageRequest`] 로 알리고 `null`), `client/registerCapability` ·
//! `unregisterCapability`(동적 등록 표를 고친다). 나머지는 `MethodNotFound` 다 — `workspace/configuration` · `workspace/applyEdit` ·
//! `window/showDocument` 같은 것은 그 기능 행이 처리기를 단다(15 「공유 부품의 주인」).
//!
//! **알림** — `window/logMessage` · `window/showMessage` · `$/logTrace` 는 서버 로그와 `tracing` 으로, 나머지(`$/progress` ·
//! `textDocument/publishDiagnostics` 등)는 [`ServerEvent::Notification`] 으로 넘긴다.
//!
//! **상한**(M4-01 리뷰 H1 · H2 — 서버 하나가 Host 메모리를 끝없이 먹지 않게): 보낼 틀이 [`MAX_QUEUED_BYTES`] 넘게 쌓이면(서버가
//! stdin 을 읽지 않는다) 그 서버를 망가진 것으로 보고 트리째 끝낸다. 서버 → 클라이언트 요청은 동시에 [`MAX_SERVER_REQUESTS`] 개까지
//! 처리하고 넘으면 곧바로 `RequestFailed` 로 답한다. 이벤트 채널([`ServerOptions::events`])은 부른 쪽이 정한 크기의 채널이고, 꽉
//! 차면 `$/progress` 알림은 버리고 그 밖의 이벤트면 서버를 끝낸다(`Exited` 는 [`LanguageServer::exited`] 로도 알 수 있어 버릴 수
//! 있다). **채널은 서버마다 따로 둔다** — 여럿이 나누면 꽉 찬 책임이 채운 서버가 아니라 그때 알림을 낸 서버에게 가 죄 없는 서버가
//! 끝난다(M4-01 재리뷰 N2). stderr 는 줄마다 [`MAX_STDERR_LINE`] 바이트까지만 쥐고(줄바꿈이 오지 않아도 — 넘는 바이트는 세기만 한다), 로그 파일의
//! 하루 상한은 `crate::log` 가 지킨다.
//!
//! **끝내기** — [`LanguageServer::shutdown`]: `shutdown`(시한 [`SHUTDOWN_TIMEOUT`]) → `exit` → stdin 닫기 → [`EXIT_GRACE`] 안에
//! 끝나지 않으면 트리째 SIGKILL. 전체가 [`STOP_BUDGET`](2 s) 안이다. 서버가 끝나면(스스로든 우리가 끝냈든) 그룹에 남은 자손도
//! 끝낸다. 손잡이를 버려도(`Drop`) 트리째 끝낸다.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lsp_types::{Registration, RegistrationParams, UnregistrationParams};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::capabilities::{client_capabilities_json, static_supports};
use crate::grant::{LspGrant, real_path};
use crate::ledger::{Ledger, Record};
use crate::log::{Line, ServerLog};
use crate::message::{self, Incoming, RequestId, ResponseError};
use crate::process::{self, LaunchSpec, ProcessTree};
use crate::transport::{FrameReader, write_frame};
use crate::uri::file_uri;

/// 요청 하나의 기본 시한. 기능 행이 [`RequestOptions::timeout`] 으로 바꾼다.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// `initialize` 의 시한. 넘으면 서버를 끝낸다.
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(10);
/// `shutdown` 요청의 시한.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
/// `exit` 뒤 서버가 스스로 끝나기를 기다리는 시간. 넘으면 트리째 SIGKILL.
pub const EXIT_GRACE: Duration = Duration::from_millis(500);
/// 끝내기 전체의 상한.
pub const STOP_BUDGET: Duration = Duration::from_secs(2);
/// 보낼 틀이 쌓일 수 있는 바이트(본문 버퍼의 용량에 틀마다 [`FRAME_OVERHEAD`] 를 더해 센다). 넘으면 서버를 끝낸다 — 편집 등급 문서(≤ 5 MiB)의 전체
/// 동기화가 여럿 쌓여도 닿지 않는 크기다.
pub const MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;
/// 틀 하나의 머리 몫.
const FRAME_OVERHEAD: usize = 64;
/// 동시에 처리하는 서버 → 클라이언트 요청 수.
pub const MAX_SERVER_REQUESTS: usize = 64;
/// stderr 한 줄에서 쥐는 바이트(UI 로그의 줄 상한과 같다).
pub const MAX_STDERR_LINE: usize = 8 * 1024;

/// 요청 하나의 시한과 취소.
#[derive(Debug, Clone)]
pub struct RequestOptions {
    pub timeout: Duration,
    /// 이것을 취소하면 `$/cancelRequest` 를 보내고 [`RequestError::Cancelled`] 로 끝난다.
    pub cancel: CancellationToken,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_REQUEST_TIMEOUT,
            cancel: CancellationToken::new(),
        }
    }
}

impl RequestOptions {
    pub fn timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::default()
        }
    }

    pub fn cancel(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RequestError {
    /// 시한 안에 답이 없었다(`$/cancelRequest` 를 보냈다).
    Timeout,
    /// 부른 쪽이 취소했다(`$/cancelRequest` 를 보냈다).
    Cancelled,
    /// 서버가 오류로 답했다.
    Server(ResponseError),
    /// 서버가 끝났거나 연결이 끊겼다(이유).
    Closed(String),
    /// 매개변수 · 결과를 형으로 바꾸지 못했다.
    Decode(String),
    /// 보내지 않았다 — 허가의 루트 밖 작업 공간 등(까닭).
    Refused(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("the language server did not answer in time"),
            Self::Cancelled => f.write_str("the request was cancelled"),
            Self::Server(e) => write!(f, "the language server answered with an error: {e}"),
            Self::Closed(reason) => write!(f, "the language server is gone: {reason}"),
            Self::Decode(detail) => write!(f, "cannot decode: {detail}"),
            Self::Refused(why) => write!(f, "refused: {why}"),
        }
    }
}

impl std::error::Error for RequestError {}

/// 서버가 끝난 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    /// 우리가 끝냈다([`LanguageServer::shutdown`] · [`LanguageServer::kill`]).
    Stopped,
    /// 서버가 스스로 끝났다(죽음 포함).
    Exited,
    /// 프로토콜을 어겨 우리가 죽였다(틀 상한 초과 등 — 이유).
    Killed(String),
}

/// 끝난 서버.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    /// 종료 코드. 신호로 끝났으면 없다.
    pub code: Option<i32>,
    pub reason: ExitReason,
}

/// 서버에서 온 것 중 부른 쪽(서버 관리자 — M4-05)이 받는 것.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    /// 이 모듈이 다루지 않는 알림(`$/progress` · `textDocument/publishDiagnostics` 등).
    Notification { method: String, params: Value },
    /// `window/showMessageRequest` — 첫 판은 버튼 없이 알림으로 보인다(답은 이미 `null` 로 했다).
    ShowMessageRequest { kind: i64, message: String },
    /// 동적 등록 · 해제로 능력이 바뀌었다.
    CapabilitiesChanged,
    /// 서버가 끝났다.
    Exited(Exit),
}

pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<Value, ResponseError>> + Send>>;
type Handler = Arc<dyn Fn(Value) -> HandlerFuture + Send + Sync>;

/// 서버 → 클라이언트 요청의 처리기 표(메서드 이름 → 처리기). 이 모듈의 기본 처리보다 먼저 불린다.
#[derive(Clone, Default)]
pub struct Handlers(HashMap<String, Handler>);

impl Handlers {
    pub fn new() -> Self {
        Self::default()
    }

    /// `method` 의 처리기를 단다(같은 이름이면 바꾼다).
    pub fn on<F, Fut>(&mut self, method: impl Into<String>, handler: F) -> &mut Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, ResponseError>> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |params| Box::pin(handler(params)));
        self.0.insert(method.into(), handler);
        self
    }

    fn get(&self, method: &str) -> Option<Handler> {
        self.0.get(method).cloned()
    }
}

impl fmt::Debug for Handlers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// [`LanguageServer::spawn`] 의 선택.
#[derive(Debug, Default)]
pub struct ServerOptions {
    /// 인스턴스 이름(로그 파일 · 로그 줄). 서버 관리자가 정한다.
    pub instance: String,
    /// 서버 로그(`crate::log`). 없으면 stderr 는 `tracing` 으로만 간다.
    pub log: Option<Arc<ServerLog>>,
    pub handlers: Handlers,
    /// 알림 · 상태를 받을 곳. 크기는 부른 쪽이 정한다 — 꽉 차면의 규칙은 머리 주석의 「상한」. **서버마다 따로 만든 채널을
    /// 준다**(같은 `Sender` 를 여러 서버에 복제해 주지 않는다): 꽉 차면 그때 알림을 낸 서버가 끝나므로, 나누면 채널을 채운 서버가
    /// 아니라 다른 서버가 끝날 수 있다(M4-01 재리뷰 N2).
    pub events: Option<mpsc::Sender<ServerEvent>>,
    /// 강제 종료 뒤 거두기의 기록부(`crate::ledger`). 없으면 기록하지 않는다.
    pub ledger: Option<Arc<Ledger>>,
}

/// `initialize` 의 선택.
#[derive(Debug, Clone)]
pub struct InitializeOptions {
    /// 작업 공간 루트(`rootUri` · `workspaceFolders` 하나).
    pub root: PathBuf,
    pub initialization_options: Option<Value>,
    pub timeout: Duration,
}

impl InitializeOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            initialization_options: None,
            timeout: INITIALIZE_TIMEOUT,
        }
    }
}

/// [`LanguageServer::shutdown`] 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    /// `shutdown` 에 서버가 답했다.
    pub answered_shutdown: bool,
    /// 시간 안에 스스로 끝나지 않아 트리째 SIGKILL 했다.
    pub killed: bool,
    pub elapsed: Duration,
}

/// 보낼 것.
enum Outgoing {
    /// 본문과 그것이 큐에서 차지한다고 센 바이트.
    Frame(Vec<u8>, usize),
    /// stdin 을 닫는다(`exit` 뒤).
    Close,
}

#[derive(Default)]
struct Capabilities {
    /// `initialize` 응답의 `capabilities` 그대로.
    raw: Value,
    typed: lsp_types::ServerCapabilities,
    /// 동적 등록(등록 id → 등록).
    registrations: BTreeMap<String, Registration>,
}

type Pending = HashMap<RequestId, oneshot::Sender<Result<Value, ResponseError>>>;

struct Shared {
    server_id: String,
    instance: String,
    outgoing: mpsc::UnboundedSender<Outgoing>,
    /// 보내려고 쌓아 둔 바이트(쓰기 태스크가 쓰면 줄인다).
    queued: Arc<AtomicUsize>,
    /// 처리 중인 서버 → 클라이언트 요청 수.
    serving: AtomicUsize,
    /// 버린 `$/progress` 알림 수(이벤트 채널이 꽉 찼다).
    dropped_progress: AtomicU64,
    /// 허가의 루트(정규화).
    root: PathBuf,
    /// 기록부의 기록(서버 그룹이 끝나면 지운다).
    record: Mutex<Option<Record>>,
    pending: Mutex<Pending>,
    next_id: AtomicI64,
    capabilities: Mutex<Capabilities>,
    /// 끝났으면 그 까닭(새 요청은 곧바로 실패한다).
    closed: Mutex<Option<String>>,
    /// 서버가 끝나도록 우리가 시작했다(끝 까닭을 「끝냈다」 로 적는다).
    stopping: AtomicBool,
    /// 프로토콜 위반으로 죽였다(까닭).
    killed: Mutex<Option<String>>,
    /// 기다리기 태스크가 서버를 거뒀다 — 그 뒤로는 그룹 id 가 다른 프로세스에 다시 쓰일 수 있어 신호를 보내지 않는다.
    reaped: AtomicBool,
    late: AtomicU64,
    log: Option<Arc<ServerLog>>,
    events: Option<mpsc::Sender<ServerEvent>>,
    handlers: Handlers,
    tree: ProcessTree,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn send(&self, body: Vec<u8>) -> Result<(), RequestError> {
        self.check_open()?;
        // 길이가 아니라 잡은 메모리(용량)로 센다 — JSON 직렬화 버퍼는 길이의 두 배까지 잡는다.
        let cost = body.capacity() + FRAME_OVERHEAD;
        let before = self.queued.fetch_add(cost, Ordering::SeqCst);
        if before + cost > MAX_QUEUED_BYTES {
            self.queued.fetch_sub(cost, Ordering::SeqCst);
            let reason = format!(
                "the server stopped reading its input ({} MiB waiting to be sent)",
                before >> 20
            );
            self.violate(reason.clone());
            return Err(RequestError::Closed(reason));
        }
        if let Some(log) = &self.log {
            log.message(Line::Sent, &body);
        }
        self.outgoing
            .send(Outgoing::Frame(body, cost))
            .map_err(|_| {
                self.queued.fetch_sub(cost, Ordering::SeqCst);
                RequestError::Closed("the connection is closed".to_owned())
            })
    }

    fn check_open(&self) -> Result<(), RequestError> {
        match &*lock(&self.closed) {
            Some(reason) => Err(RequestError::Closed(reason.clone())),
            None => Ok(()),
        }
    }

    /// 이벤트를 넘긴다. 채널이 꽉 차면 `$/progress` 와 `Exited` 는 버리고, 그 밖이면 서버를 끝낸다(머리 주석의 「상한」).
    fn emit(&self, event: ServerEvent) {
        let Some(events) = &self.events else {
            return;
        };
        let Err(mpsc::error::TrySendError::Full(event)) = events.try_send(event) else {
            return;
        };
        match event {
            ServerEvent::Notification { method, .. } if method == "$/progress" => {
                self.dropped_progress.fetch_add(1, Ordering::SeqCst);
            }
            ServerEvent::Exited(_) => {}
            other => self.violate(format!(
                "server events are not being handled fast enough (queue full at {other:?})"
            )),
        }
    }

    fn note(&self, text: &str) {
        if let Some(log) = &self.log {
            log.line(Line::Note, text);
        }
    }

    /// 트리째 끝낸다(거둔 뒤에는 아무것도 하지 않는다).
    fn kill_tree(&self) {
        if !self.reaped.load(Ordering::SeqCst) {
            self.tree.kill();
        }
    }

    /// 연결을 닫는다: 새 요청을 막고 대기 중인 요청을 모두 실패시킨다.
    fn close(&self, reason: &str) {
        lock(&self.closed).get_or_insert_with(|| reason.to_owned());
        let pending: Vec<_> = lock(&self.pending).drain().collect();
        for (_, waiter) in pending {
            // 받는 쪽이 `Closed` 로 바꾼다(보낸 쪽이 사라짐).
            drop(waiter);
        }
    }

    /// 틀 · 프로토콜 위반: 서버를 트리째 죽이고 닫는다.
    fn violate(&self, reason: String) {
        tracing::warn!(target: "pw_lsp", server = %self.server_id, instance = %self.instance, %reason, "killing the language server");
        self.note(&format!("killed: {reason}"));
        lock(&self.killed).get_or_insert_with(|| reason.clone());
        self.close(&reason);
        self.kill_tree();
    }

    fn dispatch(self: &Arc<Self>, body: &[u8]) {
        if let Some(log) = &self.log {
            log.message(Line::Received, body);
        }
        match message::parse(body) {
            Err(e) => {
                tracing::debug!(target: "pw_lsp", server = %self.server_id, %e, "dropped an unreadable message");
                self.note(&format!("dropped an unreadable message: {e}"));
            }
            Ok(Incoming::Response {
                id: Some(id),
                result,
            }) => {
                let waiter = lock(&self.pending).remove(&id);
                match waiter {
                    Some(waiter) => {
                        let _ = waiter.send(result);
                    }
                    None => {
                        self.late.fetch_add(1, Ordering::SeqCst);
                        tracing::debug!(target: "pw_lsp", server = %self.server_id, %id, "dropped a late or unknown response");
                    }
                }
            }
            Ok(Incoming::Response { id: None, result }) => {
                tracing::debug!(target: "pw_lsp", server = %self.server_id, ?result, "the server answered a message it could not read");
            }
            Ok(Incoming::Notification { method, params }) => self.notification(method, params),
            Ok(Incoming::Request { id, method, params }) => {
                if self.serving.fetch_add(1, Ordering::SeqCst) >= MAX_SERVER_REQUESTS {
                    self.serving.fetch_sub(1, Ordering::SeqCst);
                    let busy = ResponseError::new(
                        message::code::REQUEST_FAILED,
                        "too many server requests in flight",
                    );
                    let _ = self.send(message::response(&id, &Err(busy)));
                    return;
                }
                let shared = Arc::clone(self);
                tokio::spawn(async move {
                    let result = shared.handle(&method, params).await;
                    if let Err(e) = &result {
                        tracing::debug!(target: "pw_lsp", server = %shared.server_id, %method, %e, "answered a server request with an error");
                    }
                    let _ = shared.send(message::response(&id, &result));
                    shared.serving.fetch_sub(1, Ordering::SeqCst);
                });
            }
        }
    }

    fn notification(&self, method: String, params: Value) {
        match method.as_str() {
            "window/logMessage" | "window/showMessage" => {
                let kind = params.get("type").and_then(Value::as_i64).unwrap_or(4);
                let text = params.get("message").and_then(Value::as_str).unwrap_or("");
                tracing::debug!(target: "pw_lsp", server = %self.server_id, %method, kind, message = text);
                self.note(&format!("{method} [{kind}] {text}"));
            }
            "$/logTrace" => {
                let text = params.get("message").and_then(Value::as_str).unwrap_or("");
                self.note(&format!("trace {text}"));
            }
            _ => self.emit(ServerEvent::Notification { method, params }),
        }
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value, ResponseError> {
        if let Some(handler) = self.handlers.get(method) {
            return handler(params).await;
        }
        match method {
            "window/workDoneProgress/create" => Ok(Value::Null),
            // 클라이언트 능력으로 알리므로 처리기가 달리기 전(M4-05)에는 항목마다 `null`(「설정 없음」)로 답한다 — 오류보다 서버가
            // 덜 시끄럽다(M4-01 리뷰 L2).
            "workspace/configuration" => {
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                Ok(Value::Array(vec![Value::Null; items]))
            }
            "window/showMessageRequest" => {
                let kind = params.get("type").and_then(Value::as_i64).unwrap_or(3);
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                self.note(&format!("{method} [{kind}] {message}"));
                self.emit(ServerEvent::ShowMessageRequest { kind, message });
                Ok(Value::Null)
            }
            "client/registerCapability" => {
                let params: RegistrationParams = serde_json::from_value(params)
                    .map_err(|e| ResponseError::invalid_params(e.to_string()))?;
                {
                    let mut caps = lock(&self.capabilities);
                    for r in params.registrations {
                        caps.registrations.insert(r.id.clone(), r);
                    }
                }
                self.emit(ServerEvent::CapabilitiesChanged);
                Ok(Value::Null)
            }
            "client/unregisterCapability" => {
                let params: UnregistrationParams = serde_json::from_value(params)
                    .map_err(|e| ResponseError::invalid_params(e.to_string()))?;
                {
                    let mut caps = lock(&self.capabilities);
                    for u in params.unregisterations {
                        caps.registrations.remove(&u.id);
                    }
                }
                self.emit(ServerEvent::CapabilitiesChanged);
                Ok(Value::Null)
            }
            _ => Err(ResponseError::method_not_found(method)),
        }
    }
}

/// 요청 하나가 표에 있는 동안의 손잡이. 답을 받기 전에 버려지면(시한 · 취소 · future 를 버림) 표에서 지우고 `$/cancelRequest` 를
/// 보낸다.
struct InFlight<'a> {
    shared: &'a Shared,
    id: Option<RequestId>,
}

impl InFlight<'_> {
    fn answered(mut self) {
        self.id = None;
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        if lock(&self.shared.pending).remove(&id).is_some() {
            let _ = self.shared.send(message::notification(
                "$/cancelRequest",
                &json!({ "id": id }),
            ));
        }
    }
}

/// 떠 있는 언어 서버 하나.
pub struct LanguageServer {
    shared: Arc<Shared>,
    exit: watch::Receiver<Option<Exit>>,
    pid: Option<u32>,
}

impl fmt::Debug for LanguageServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LanguageServer")
            .field("server_id", &self.shared.server_id)
            .field("instance", &self.shared.instance)
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl LanguageServer {
    /// 서버를 띄운다. 허가([`LspGrant`])가 있어야 부를 수 있다 — 신뢰와 실행의 연결 범위를 지났다는 증거다. 띄우기 전에 작업
    /// 디렉터리가 허가의 루트 안인지, 허가를 만든 연결이 아직 열려 있는지, 지금도 신뢰하는지 다시 본다(아니면 `PermissionDenied`,
    /// 아무것도 띄우지 않는다 — 리뷰 M1 · M2, 재리뷰 N5). `initialize` 는 [`Self::initialize`] 로 따로 보낸다. tokio 런타임 안이면 어느 스레드에서 불러도 된다 — fork 는 프로세스
    /// 수명 동안 사는 전용 스레드가 한다(Linux 의 `PR_SET_PDEATHSIG` 는 fork 한 스레드에 묶인다 — `crate::process` 의 `launcher`,
    /// M4-01 재리뷰 N3 · M4-05 리뷰 M4).
    pub async fn spawn(
        grant: &LspGrant,
        spec: LaunchSpec,
        options: ServerOptions,
    ) -> std::io::Result<Self> {
        if !grant.contains(&spec.cwd) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{} is outside the project root {}",
                    spec.cwd.display(),
                    grant.root().display()
                ),
            ));
        }
        if !grant.connection_open() {
            tracing::info!(target: "pw_lsp", project = grant.project_id(), "language server refused: the window's connection is closed");
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the connection that granted this launch is closed",
            ));
        }
        if !grant.still_trusted() {
            tracing::info!(target: "pw_lsp", project = grant.project_id(), "language server refused: trust was revoked");
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("project `{}` is no longer trusted", grant.project_id()),
            ));
        }
        let marker = options.ledger.as_ref().and_then(|_| Ledger::marker());
        let (mut child, tree) = process::spawn_on_launcher(spec.clone(), marker.clone()).await?;
        let pid = child.id();
        let record = match (&options.ledger, pid, &marker) {
            (Some(ledger), Some(pid), Some(marker)) => {
                let argv: Vec<std::ffi::OsString> =
                    std::iter::once(spec.program.clone().into_os_string())
                        .chain(spec.args.iter().cloned())
                        .collect();
                ledger.record(pid, &argv, marker)
            }
            _ => None,
        };
        tracing::info!(target: "pw_lsp", project = grant.project_id(), server = %spec.server_id, instance = %options.instance, ?pid, program = %spec.program.display(), "language server started");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (outgoing, outgoing_rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            server_id: spec.server_id,
            instance: options.instance,
            outgoing,
            queued: Arc::clone(&queued),
            serving: AtomicUsize::new(0),
            dropped_progress: AtomicU64::new(0),
            root: grant.root().to_path_buf(),
            record: Mutex::new(record),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            capabilities: Mutex::new(Capabilities::default()),
            closed: Mutex::new(None),
            stopping: AtomicBool::new(false),
            killed: Mutex::new(None),
            reaped: AtomicBool::new(false),
            late: AtomicU64::new(0),
            log: options.log,
            events: options.events,
            handlers: options.handlers,
            tree,
        });
        if let Some(log) = &shared.log {
            log.line(
                Line::Note,
                &format!("started {} (pid {pid:?})", spec.program.display()),
            );
        }
        let (exit_tx, exit) = watch::channel(None);
        if let Some(stdin) = stdin {
            tokio::spawn(write_loop(stdin, outgoing_rx, queued));
        }
        if let Some(stdout) = stdout {
            tokio::spawn(read_loop(Arc::clone(&shared), stdout));
        }
        if let Some(stderr) = stderr {
            tokio::spawn(stderr_loop(Arc::clone(&shared), stderr));
        }
        tokio::spawn(wait_loop(Arc::clone(&shared), child, exit_tx));
        Ok(Self { shared, exit, pid })
    }

    pub fn server_id(&self) -> &str {
        &self.shared.server_id
    }

    pub fn instance(&self) -> &str {
        &self.shared.instance
    }

    /// 서버 프로세스의 pid(unix 는 프로세스 그룹 id 와 같다).
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// `initialize` → `initialized`. 실패하면(시한 포함) 서버를 트리째 끝내고 끝나기를 기다린 뒤 오류를 돌려준다.
    pub async fn initialize(
        &self,
        options: InitializeOptions,
    ) -> Result<lsp_types::InitializeResult, RequestError> {
        if !real_path(&options.root).is_some_and(|real| real.starts_with(&self.shared.root)) {
            return Err(RequestError::Refused(format!(
                "the workspace root {} is outside the project root {}",
                options.root.display(),
                self.shared.root.display()
            )));
        }
        let uri = file_uri(&options.root).ok_or_else(|| {
            RequestError::Decode(format!(
                "{} is not an absolute path",
                options.root.display()
            ))
        })?;
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "Pitwall", "version": env!("CARGO_PKG_VERSION") },
            "rootPath": options.root,
            "rootUri": uri,
            "workspaceFolders": [{ "uri": uri, "name": folder_name(&options.root) }],
            "capabilities": client_capabilities_json(),
            "initializationOptions": options.initialization_options,
            "trace": "off",
        });
        let result = self
            .request_raw(
                "initialize",
                params,
                RequestOptions::timeout(options.timeout),
            )
            .await;
        let value = match result {
            Ok(value) => value,
            Err(e) => {
                self.shared
                    .note(&format!("initialize failed: {e}; stopping the server"));
                self.kill();
                self.wait_exit(STOP_BUDGET).await;
                return Err(e);
            }
        };
        let raw = value.get("capabilities").cloned().unwrap_or(Value::Null);
        let typed = serde_json::from_value::<lsp_types::ServerCapabilities>(raw.clone())
            .unwrap_or_else(|e| {
                tracing::debug!(target: "pw_lsp", server = %self.shared.server_id, %e, "server capabilities do not fit lsp-types; querying the raw JSON");
                lsp_types::ServerCapabilities::default()
            });
        let server_info = value
            .get("serverInfo")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());
        {
            let mut caps = lock(&self.shared.capabilities);
            caps.raw = raw;
            caps.typed = typed.clone();
        }
        self.notify_raw("initialized", json!({}))?;
        Ok(lsp_types::InitializeResult {
            capabilities: typed,
            server_info,
        })
    }

    /// 요청 하나(머리 주석의 「요청」).
    pub async fn request_raw(
        &self,
        method: &str,
        params: Value,
        options: RequestOptions,
    ) -> Result<Value, RequestError> {
        self.shared.check_open()?;
        let id = RequestId::Number(self.shared.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = oneshot::channel();
        lock(&self.shared.pending).insert(id.clone(), tx);
        let in_flight = InFlight {
            shared: &self.shared,
            id: Some(id.clone()),
        };
        self.shared.send(message::request(&id, method, &params))?;
        let outcome = tokio::select! {
            answer = rx => Some(answer),
            () = options.cancel.cancelled() => None,
            () = tokio::time::sleep(options.timeout) => None,
        };
        match outcome {
            Some(Ok(answer)) => {
                in_flight.answered();
                answer.map_err(RequestError::Server)
            }
            Some(Err(_)) => {
                in_flight.answered();
                Err(RequestError::Closed(self.closed_reason()))
            }
            None => {
                // 표에서 지우고 `$/cancelRequest` 를 보낸다(InFlight 의 Drop).
                drop(in_flight);
                if options.cancel.is_cancelled() {
                    Err(RequestError::Cancelled)
                } else {
                    Err(RequestError::Timeout)
                }
            }
        }
    }

    /// 형이 있는 요청.
    pub async fn request<R: lsp_types::request::Request>(
        &self,
        params: R::Params,
        options: RequestOptions,
    ) -> Result<R::Result, RequestError> {
        let params =
            serde_json::to_value(params).map_err(|e| RequestError::Decode(e.to_string()))?;
        let value = self.request_raw(R::METHOD, params, options).await?;
        serde_json::from_value(value).map_err(|e| RequestError::Decode(e.to_string()))
    }

    pub fn notify_raw(&self, method: &str, params: Value) -> Result<(), RequestError> {
        self.shared.send(message::notification(method, &params))
    }

    pub fn notify<N: lsp_types::notification::Notification>(
        &self,
        params: N::Params,
    ) -> Result<(), RequestError> {
        let params =
            serde_json::to_value(params).map_err(|e| RequestError::Decode(e.to_string()))?;
        self.notify_raw(N::METHOD, params)
    }

    /// `initialize` 응답의 정적 능력.
    pub fn capabilities(&self) -> lsp_types::ServerCapabilities {
        lock(&self.shared.capabilities).typed.clone()
    }

    /// `initialize` 응답의 `capabilities` 그대로(서버가 보낸 JSON).
    pub fn capabilities_json(&self) -> Value {
        lock(&self.shared.capabilities).raw.clone()
    }

    /// 서버가 `method` 를 다루는가: 그 메서드의 동적 등록이 있거나 정적 능력이 있다(`crate::capabilities::static_supports`).
    pub fn supports(&self, method: &str) -> bool {
        let caps = lock(&self.shared.capabilities);
        caps.registrations.values().any(|r| r.method == method)
            || static_supports(&caps.raw, method)
    }

    /// `method` 의 동적 등록들(등록 id 순). `workspace/didChangeWatchedFiles` 의 글롭 등 — 등록 옵션은 그 행이 읽는다.
    pub fn registrations(&self, method: &str) -> Vec<Registration> {
        lock(&self.shared.capabilities)
            .registrations
            .values()
            .filter(|r| r.method == method)
            .cloned()
            .collect()
    }

    /// 답이 오기 전에 버린 요청(취소 · 시한)에 늦게 온 응답 수 — 버렸다.
    pub fn late_responses(&self) -> u64 {
        self.shared.late.load(Ordering::SeqCst)
    }

    /// 이벤트 채널이 꽉 차 버린 `$/progress` 알림 수.
    pub fn dropped_progress(&self) -> u64 {
        self.shared.dropped_progress.load(Ordering::SeqCst)
    }

    /// 보내려고 쌓아 둔 바이트(틀마다 머리 몫 포함).
    pub fn queued_bytes(&self) -> usize {
        self.shared.queued.load(Ordering::SeqCst)
    }

    /// 끝났으면 그 결과.
    pub fn exit_status(&self) -> Option<Exit> {
        self.exit.borrow().clone()
    }

    /// 끝날 때까지 기다린다.
    pub async fn exited(&self) -> Exit {
        let mut exit = self.exit.clone();
        loop {
            if let Some(done) = exit.borrow_and_update().clone() {
                return done;
            }
            if exit.changed().await.is_err() {
                return Exit {
                    code: None,
                    reason: ExitReason::Exited,
                };
            }
        }
    }

    /// 곧바로 트리째 끝낸다(기다리지 않는다).
    pub fn kill(&self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.shared.close("the language server was stopped");
        self.shared.kill_tree();
    }

    /// 머리 주석의 「끝내기」. 이미 끝났으면 곧바로 돌아온다.
    pub async fn shutdown(&self) -> Stopped {
        let started = Instant::now();
        self.shared.stopping.store(true, Ordering::SeqCst);
        let mut answered_shutdown = false;
        if self.exit_status().is_none() && self.shared.check_open().is_ok() {
            answered_shutdown = self
                .request_raw(
                    "shutdown",
                    Value::Null,
                    RequestOptions::timeout(SHUTDOWN_TIMEOUT),
                )
                .await
                .is_ok();
            let _ = self.notify_raw("exit", Value::Null);
        }
        self.shared.close("the language server was stopped");
        let _ = self.shared.outgoing.send(Outgoing::Close);
        let mut killed = false;
        if !self.wait_exit(EXIT_GRACE).await {
            killed = true;
            self.shared
                .note("the server did not exit in time; killing its process tree");
            self.shared.kill_tree();
            let left = STOP_BUDGET.saturating_sub(started.elapsed());
            self.wait_exit(left).await;
        }
        let elapsed = started.elapsed();
        tracing::info!(target: "pw_lsp", server = %self.shared.server_id, instance = %self.shared.instance, answered_shutdown, killed, ?elapsed, "language server stopped");
        Stopped {
            answered_shutdown,
            killed,
            elapsed,
        }
    }

    fn closed_reason(&self) -> String {
        lock(&self.shared.closed)
            .clone()
            .unwrap_or_else(|| "the connection closed".to_owned())
    }

    /// `limit` 안에 끝났는가.
    async fn wait_exit(&self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.exited()).await.is_ok()
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.shared.close("the language server handle was dropped");
        self.shared.kill_tree();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut(chunks: &[&[u8]]) -> (Vec<String>, LineCutter) {
        let mut lines = Vec::new();
        let mut cutter = LineCutter::default();
        for chunk in chunks {
            cutter.feed(chunk, &mut |l: &str| lines.push(l.to_owned()));
        }
        (lines, cutter)
    }

    #[test]
    fn stderr_lines_split_on_newlines_and_carriage_returns_across_reads() {
        let (lines, mut cutter) = cut(&[b"one\ntw", b"o\r\nthr", b"ee\r50%\r"]);
        assert_eq!(lines, ["one", "two", "three", "50%"]);
        let mut rest = Vec::new();
        cutter.feed(b"tail", &mut |l: &str| rest.push(l.to_owned()));
        cutter.finish(&mut |l: &str| rest.push(l.to_owned()));
        assert_eq!(rest, ["tail"]);
    }

    /// 리뷰 H1: 줄바꿈이 오지 않아도 줄 상한만큼만 쥐고, 앞부분 한 줄과 「잘림」 한 줄을 낸다.
    #[test]
    fn a_line_without_a_newline_keeps_only_the_cap() {
        let chunk = vec![b'a'; 64 * 1024];
        let mut lines = Vec::new();
        let mut cutter = LineCutter::default();
        for _ in 0..160 {
            // 10 MiB
            cutter.feed(&chunk, &mut |l: &str| lines.push(l.to_owned()));
            assert!(cutter.held() <= MAX_STDERR_LINE * 2, "{}", cutter.held());
        }
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_STDERR_LINE + " …".len());
        cutter.feed(b"\nnext\n", &mut |l: &str| lines.push(l.to_owned()));
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[1],
            format!(
                "… (the line above was cut: {} more bytes dropped)",
                10 * 1024 * 1024 - MAX_STDERR_LINE
            )
        );
        assert_eq!(lines[2], "next");
    }

    /// 재리뷰 N4: 잘린 뒤의 작은 조각도 모두 센다.
    #[test]
    fn small_chunks_after_the_cut_are_all_counted() {
        let chunk = vec![b'b'; 1024];
        let mut lines = Vec::new();
        let mut cutter = LineCutter::default();
        for _ in 0..40 {
            cutter.feed(&chunk, &mut |l: &str| lines.push(l.to_owned()));
        }
        assert!(cutter.held() <= MAX_STDERR_LINE, "{}", cutter.held());
        cutter.finish(&mut |l: &str| lines.push(l.to_owned()));
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1],
            format!(
                "… (the line above was cut: {} more bytes dropped)",
                40 * 1024 - MAX_STDERR_LINE
            )
        );
    }
}

fn folder_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

async fn write_loop(
    mut stdin: ChildStdin,
    mut outgoing: mpsc::UnboundedReceiver<Outgoing>,
    queued: Arc<AtomicUsize>,
) {
    while let Some(next) = outgoing.recv().await {
        match next {
            Outgoing::Frame(body, cost) => {
                let written = write_frame(&mut stdin, &body).await;
                drop(body);
                queued.fetch_sub(cost, Ordering::SeqCst);
                if let Err(e) = written {
                    tracing::debug!(target: "pw_lsp", %e, "cannot write to the language server");
                    break;
                }
            }
            Outgoing::Close => break,
        }
    }
}

async fn read_loop(shared: Arc<Shared>, stdout: ChildStdout) {
    let mut reader = FrameReader::new(stdout);
    loop {
        match reader.read_frame().await {
            Ok(Some(body)) => shared.dispatch(&body),
            Ok(None) => {
                shared.close("the language server closed its output");
                break;
            }
            Err(e) => {
                shared.violate(format!("protocol error: {e}"));
                break;
            }
        }
    }
}

async fn stderr_loop(shared: Arc<Shared>, mut stderr: ChildStderr) {
    let mut cutter = LineCutter::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut emit = |line: &str| {
        tracing::trace!(target: "pw_lsp", server = %shared.server_id, stderr = %line);
        if let Some(log) = &shared.log {
            log.line(Line::Stderr, line);
        }
    };
    loop {
        match stderr.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => cutter.feed(&buf[..n], &mut emit),
            Err(e) => {
                tracing::debug!(target: "pw_lsp", server = %shared.server_id, %e, "stopped reading the language server's stderr");
                break;
            }
        }
    }
    cutter.finish(&mut emit);
}

/// stderr 바이트를 줄로 가른다(`\n` · `\r` — 진행 표시의 `\r` 도 줄 끝으로 본다). 줄마다 [`MAX_STDERR_LINE`] 바이트까지만 쥔다:
/// 넘으면 그 앞부분을 곧바로 한 줄로 내고(줄바꿈이 끝내 오지 않아도 무엇이 왔는지 남게) 나머지는 줄 끝까지 세기만 한 뒤 「잘림」
/// 한 줄을 더 낸다.
#[derive(Debug, Default)]
pub(crate) struct LineCutter {
    line: Vec<u8>,
    cut: usize,
}

impl LineCutter {
    pub(crate) fn feed(&mut self, mut bytes: &[u8], emit: &mut impl FnMut(&str)) {
        while !bytes.is_empty() {
            let end = bytes.iter().position(|&b| b == b'\n' || b == b'\r');
            let (part, rest) = match end {
                Some(at) => (&bytes[..at], Some(&bytes[at + 1..])),
                None => (bytes, None),
            };
            let room = MAX_STDERR_LINE - self.line.len();
            if self.cut > 0 {
                // 이미 잘린 줄: 줄 끝까지 세기만 한다(재리뷰 N4 — 작은 조각이 `room` 안이라 쌓이면 수가 적게 나온다).
                self.cut += part.len();
            } else if part.len() <= room {
                self.line.extend_from_slice(part);
            } else {
                self.line.extend_from_slice(&part[..room]);
                let head = String::from_utf8_lossy(&self.line).into_owned();
                emit(&format!("{head} …"));
                self.line.clear();
                self.cut = part.len() - room;
            }
            match rest {
                Some(rest) => {
                    self.end_line(emit);
                    bytes = rest;
                }
                None => break,
            }
        }
    }

    fn end_line(&mut self, emit: &mut impl FnMut(&str)) {
        if self.cut > 0 {
            emit(&format!(
                "… (the line above was cut: {} more bytes dropped)",
                self.cut
            ));
        } else if !self.line.is_empty() {
            emit(&String::from_utf8_lossy(&self.line));
        }
        self.line.clear();
        self.cut = 0;
    }

    pub(crate) fn finish(&mut self, emit: &mut impl FnMut(&str)) {
        self.end_line(emit);
    }

    /// 쥔 바이트(시험).
    #[cfg(test)]
    fn held(&self) -> usize {
        self.line.capacity()
    }
}

async fn wait_loop(shared: Arc<Shared>, mut child: Child, exit: watch::Sender<Option<Exit>>) {
    let status = child.wait().await;
    // 서버가 끝났다 — 그룹에 남은 자손(빌드 · 데몬)을 끝내고, 그 뒤로는 신호를 보내지 않는다.
    shared.kill_tree();
    shared.reaped.store(true, Ordering::SeqCst);
    // 그룹을 끝냈다 — 강제 종료 뒤 거두기의 기록을 지운다.
    drop(lock(&shared.record).take());
    let code = status
        .as_ref()
        .ok()
        .and_then(std::process::ExitStatus::code);
    let reason = if let Some(why) = lock(&shared.killed).clone() {
        ExitReason::Killed(why)
    } else if shared.stopping.load(Ordering::SeqCst) {
        ExitReason::Stopped
    } else {
        ExitReason::Exited
    };
    shared.close(&match &reason {
        ExitReason::Killed(why) => why.clone(),
        ExitReason::Stopped => "the language server was stopped".to_owned(),
        ExitReason::Exited => format!("the language server exited ({status:?})"),
    });
    shared.note(&format!("exited: {status:?} ({reason:?})"));
    if reason == ExitReason::Exited {
        tracing::warn!(target: "pw_lsp", server = %shared.server_id, instance = %shared.instance, ?status, "language server exited on its own");
    }
    let done = Exit { code, reason };
    let _ = exit.send(Some(done.clone()));
    shared.emit(ServerEvent::Exited(done));
}
