//! MCP 세션 하나: rmcp 서버 핸들러. 툴 호출을 Host 의 RPC 로 옮긴다.
//!
//! 세션은 Host 에 **owner** role 로 붙는다 — 에이전트는 이 사용자의 로컬 프로세스다(로컬 소켓 · stdio 만 받는다). 무엇을
//! 부를 수 있는지는 role 이 아니라 이 계층이 정한다: 노출([`crate::exposure`]) → 등급 정책([`crate::policy`]).
//! 호출 하나하나는 결과와 상관없이 감사 로그([`crate::audit`])에 남는다.
//!
//! 호출 하나가 지나는 길(`docs/design/05-mcp.md` 「툴 레지스트리」 · 「툴 노출 설정」, 16-m5-plan M5-03):
//!
//! 1. 세션의 노출 — 보이지 않는 툴은 JSON-RPC invalid params(오류 `data` 에 결과 표지).
//! 2. 빈 `projectId` 는 세션 프로젝트로, 빈 `worktree` 는 세션의 활성 워크트리 꼭대기로 채운다(세션 프로젝트를 겨눌 때만).
//! 3. 대상 프로젝트의 노출 — `projectId` 가 세션 프로젝트가 아니면 그 프로젝트의 `[mcp]` 로도 판정한다(결정 M5-D28).
//! 4. 등급 정책([`crate::policy`]). 승인 경로가 없는 세션(헤드리스)의 `write` 이상은 여기서 **선언 등급으로** 거절한다 — 툴을
//!    부르기 전(계획을 만들기 전)이다.
//! 5. **승인**(16-m5-plan M5-12, 결정 M5-D30): 계획을 만드는 툴([`Tool::planned`] — `write` 이상은 모두)은 먼저 승인 표 없이
//!    부른다. 툴은 부작용 없이 계획을 실은 「승인 필요」(`pw_core::registry::plan::APPROVAL_REQUIRED`)로 답하고, 세션은
//!    정책 자동(계획의 `policyAuto` — 툴이 05 의 규칙으로 판정한 것) · `auto_approve` · 세션 허용 · CLI 허용을 보거나 그 프로젝트
//!    창에 묻는다([`crate::approval`]). 판정의 등급은 **계획 등급**이다 — 선언 등급은 상한이라 계획이 그보다 높거나 `dangerous`
//!    계획이 정책 자동을 달면 툴의 버그로 묻지도 돌리지도 않는다(17-m6-plan 결정 M6-P9). 허용이면 승인 표(해시)를 예약 키에
//!    실어 다시 부르고, 툴이 계획을 다시 만들어 해시가 다르면 다시 묻지 않고 거절한다(`plan_changed`). 「승인 필요」 오류는
//!    에이전트에게 그대로 나가지 않는다 — 묻지 못하면 정해진 거절 까닭이다. 에이전트가 보낸 예약 키는 맨 앞에서 버린다.
//!    계획이 승인을 청하지 않은 `read` 호출은 그 첫 호출의 결과가 곧 결과다.
//! 6. Host RPC(선기록 — 결정 M5-D11). MCP 의 취소 알림이 오거나 세션이 끝나면 그 RPC 를 거둔다(`request.cancel` —
//!    [`crate::client`]). 승인을 기다리는 동안 클라이언트가 끊으면(읽기가 끝남) 곧바로 요청을 거둔다 — rmcp 가 끊긴 뒤 5초 동안
//!    핸들러를 돌리는 사이(05 「호출 취소」)에 사람이 [허용] 을 눌러도 돌지 않는다. 선기록한 호출은 취소 뒤에도 Host 의 답을
//!    [`SETTLE_AFTER_CANCEL`] 까지 기다려 그 답으로 행을 끝낸다 — 되돌릴 수 없는 지점(`db_execute` 의 커밋)을 넘은 호출은 결과
//!    `ok`(상세에 [`CANCELLED_AFTER_THE_POINT`]), 취소가 닿기 전에 이미 끝난 호출은 그 결과(상세에
//!    [`CANCELLED_AFTER_IT_FINISHED`]), 답이 없으면 취소(상세에 [`CANCELLED_UNSETTLED`]) — 16-m5-plan M5-18b.
//! 7. 응답 상한 [`MAX_RESPONSE_BYTES`] — 넘으면 「범위를 좁혀라」 오류(결정 M5-D34).
//!
//! 결과는 모두 `_meta` 에 결과 표지([`crate::marker`])를 단다.

mod checkout_tools;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use pw_core::config::{AppPaths, ConfigSource, ConfigStore, TOPIC_CHANGED};
use pw_core::registry::methods::{PathParams, ProjectIdParams};
use pw_core::registry::plan::{
    APPROVAL_KEY, APPROVAL_REQUIRED, ApprovalRequired, ApprovalTicket, Remember, ToolPlan,
};
use pw_core::registry::{ConfigMode, Project, Resolution};
use pw_core::{EventBus, Filter, HostRouter, Scope, Target};
use pw_rpc::{Caller, Role, Tier};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, MetaObject, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    SubscriptionFilter,
};
use rmcp::service::{MaybeSendFuture, NotificationContext, RequestContext, SubscriptionContext};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::approval::{
    Allowance, ApprovalAsk, AskError, MAX_PENDING_PER_SESSION, McpApprovalAnswer,
};
use crate::audit::{AuditApproval, AuditLog, Entry, Outcome, summarize};
use crate::catalog::{Catalog, PROJECT_ID, Tool};
use crate::checkout::{self, CHECKOUT, CheckoutInput, Machines, RemoteAim};
use crate::client::{CallError, HostClient, Settled};
use crate::exposure::{self, Exposure};
use crate::marker::{Mark, Reason};
use crate::policy::{self, Approval, Decision, Policy};
use crate::sessions::Sessions;

/// 툴 응답 한 건의 상한: 직렬화한 결과 JSON 의 바이트 수(결정 M5-D34 — 출발값이다). 툴 메서드는 이것을 넘지 않게 자르고
/// (`truncated` 와 다음 `cursor`), 그래도 넘은 응답은 세션이 「범위를 좁혀라」 오류로 바꾼다.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// 선기록한 호출을 취소한 뒤 Host 의 답을 기다리는 시한(16-m5-plan M5-18b). rmcp 가 클라이언트가 끊긴 뒤 핸들러의 응답을 기다리는
/// 시간(rmcp 3.9 의 drain 5 s — 05-mcp.md 「호출 취소」)과 같다. 넘기면 결과를 모른다 — 감사 행은 취소이고 상세가
/// [`CANCELLED_UNSETTLED`] 다.
pub const SETTLE_AFTER_CANCEL: Duration = Duration::from_secs(5);

/// 취소 뒤 Host 가 결과로 답한 호출의 감사 상세 꼬리 — 되돌릴 수 없는 지점(`db_execute` 의 커밋)을 넘어 끝까지 돌았다.
pub const CANCELLED_AFTER_THE_POINT: &str =
    "cancelled by the client after the point of no return; it ran to the end";

/// 취소했지만 Host 가 그 취소를 받기 전에 이미 답한 호출의 감사 상세 꼬리 — 표지와 무관하게 끝까지 돌았다.
pub const CANCELLED_AFTER_IT_FINISHED: &str =
    "cancelled by the client after the host had already finished it";

/// 취소 뒤 [`SETTLE_AFTER_CANCEL`] 까지 답이 없던 호출의 감사 상세 꼬리 — 「취소됨」 과 달리 효과가 났는지 모른다.
pub const CANCELLED_UNSETTLED: &str = "no answer from the host within 5 s after the cancel; \
     whether it took effect (for db_execute, whether it committed) is unknown";

/// 목록 툴의 `limit` 기본값(결정 M5-D34) — 툴 메서드 규약의 값이라 정본은 `pw_rpc` 에 있다(기능 크레이트가 쓴다). 툴마다
/// 다르게 둘 수 있다(`git_log` · `symbol_search` 50, `git_status` 200).
pub use pw_rpc::DEFAULT_LIST_LIMIT;

/// 툴 결과(object)나 오류 `data`(object)의 최상위에 이 키로 글을 실으면 감사 행의 상세 칸으로 간다(「툴 메서드 규약」 —
/// `crates/pw-mcp` 머리 주석). 결과에 남으므로 에이전트에게도 보인다.
pub const AUDIT: &str = "audit";

/// 프로젝트 툴이 선택 입력으로 받는 워크트리 꼭대기 키(「체크아웃」). 비었으면 세션의 활성 워크트리로 채운다.
pub const WORKTREE: &str = "worktree";

/// 세션을 서비스하는 Host 가 무엇인가. 헤드리스는 연결 방식(stdio · 소켓 · 프로세스 안 duplex)이 아니라 Host 의 성질이다 —
/// 앱 없이 뜬 `pitwall mcp` 와 앱 없이 도는 CLI(16-m5-plan M5-04)가 만든 Host 는 코어 메서드만 처리하고 승인 경로가 없다.
/// 기본값을 두지 않는다 — [`Services`] 를 만드는 자리가 반드시 고른다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// 실행 중인 앱의 Host.
    App,
    /// 앱 없이 그 프로세스 안에 만든 Host(`pitwall mcp` 의 헤드리스, 앱 없는 CLI). 서버 안내문에 [`HEADLESS_NOTICE`] 를 싣고,
    /// `write` 이상의 거절 까닭은 `headless_write` 다.
    Headless,
}

/// CLI(`pitwall tool`)가 `initialize` 에 적는 클라이언트 이름. 이 이름의 호출은 승인의 셋째 답이 「창이 열려 있는 동안 CLI
/// 허용」 이다(결정 M5-D5 — 서버는 CLI 를 이 이름으로만 안다).
pub const CLI_CLIENT: &str = "pitwall-cli";

/// 헤드리스 세션의 서버 안내문(결정 M5-D3). 앱 세션에는 싣지 않는다. 앱이 뜨면 `pitwall mcp` 가 앱으로 넘겨 붙는다(16-m5-plan M5-20).
pub const HEADLESS_NOTICE: &str = "The Pitwall app is not running, so only the core tools are available. \
Start the Pitwall app and the rest of the tools appear in this session without a restart.";

/// 세션들이 함께 쓰는 것. 앱에서는 앱의 Host · 전역 설정 · 버스를, 앱 없이 뜬 `pitwall mcp` 는 자기가 만든 것을 넣는다.
pub struct Services {
    pub router: Arc<HostRouter>,
    /// 레지스트리(`project.*`)를 가진 Host.
    pub host_id: String,
    pub paths: AppPaths,
    /// 전역 설정(`$CONFIG/settings.toml` 의 `[mcp]`).
    pub global: ConfigStore,
    /// 설정 변경 이벤트(`config.changed`)를 받는 버스. 전역 설정 스토어가 내는 버스와 같아야 한다.
    pub bus: EventBus,
    pub audit: Arc<AuditLog>,
    pub catalog: Arc<Catalog>,
    pub approval: Approval,
    /// 로그에 남길 연결 종류(`socket` · `stdio`). 판정에는 쓰지 않는다 — 헤드리스인지는 [`Self::host_kind`].
    pub transport: &'static str,
    pub host_kind: HostKind,
    /// 열린 세션의 등록부(`mcp.sessions`) — 앱은 `mcp.*` RPC 와 같은 것을, 헤드리스는 자기 것을 넣는다.
    pub sessions: Sessions,
    /// 다른 머신 · 묶음(`checkout` 입력 — 18-m7-plan M7-18). Desktop 앱만 넣는다. `None`(헤드리스 · 시험)이면 다른 머신을
    /// 겨누는 호출을 모두 거절하고 스키마에 `checkout` 을 더하지 않는다.
    pub machines: Option<Arc<dyn Machines>>,
}

/// 세션을 여는 쪽이 알려 주는 것(`pitwall mcp [--project <path>]` 와 그 cwd).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionRequest {
    /// `--project`. 상대 경로면 `cwd` 기준.
    #[serde(default)]
    pub project: Option<PathBuf>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

impl SessionRequest {
    /// 프로젝트를 해석할 경로. `--project` 가 없으면 cwd(F2 와 같은 규칙).
    pub fn path(&self) -> Option<PathBuf> {
        match (&self.project, &self.cwd) {
            (Some(p), _) if p.is_absolute() => Some(p.clone()),
            (Some(p), Some(cwd)) => Some(cwd.join(p)),
            (Some(p), None) => Some(p.clone()),
            (None, cwd) => cwd.clone(),
        }
    }
}

#[derive(Debug)]
pub enum ServeError {
    Host(String),
    Initialize(String),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Host(e) => write!(f, "cannot connect to the Pitwall host: {e}"),
            ServeError::Initialize(e) => write!(f, "MCP session did not start: {e}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// 세션 하나를 끝까지 돌린다. 클라이언트가 끊거나 `cancel` 이 취소되면 끝난다. 돌아오기 전에 그때까지 큐에 넣은 감사 기록이 다
/// 쓰일 때까지 기다린다 — 쓰기 스레드([`audit_writer`])는 따로 돌아, 기다리지 않으면 세션 하나로 끝나는 프로세스(`pitwall tool` 의
/// 헤드리스 세션)가 기록을 쓰는 도중에 끝나 행을 잃는다. 클라이언트가 응답을 받은 호출은 응답 전에 큐에 들어가므로 모두 쓰인다.
/// 연결이 끊긴 뒤에도 도는 핸들러(05-mcp.md 「호출 취소」)의 기록은 이 기다림에 들지 않는다.
pub async fn serve<R, W>(
    services: Arc<Services>,
    request: SessionRequest,
    io: (R, W),
    cancel: CancellationToken,
) -> Result<(), ServeError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    // 세션이 어떻게 끝나든(정상 · 초기화 실패 · 패닉) 그 세션의 설정 감시 · 버스 구독 · list_changed 태스크 · Host 연결을 걷는다.
    let session_cancel = cancel.child_token();
    let _stop = session_cancel.clone().drop_guard();
    let session = Session::open(services, &request, session_cancel.clone()).await?;
    let audit = session.inner.audit.clone();
    // 클라이언트가 끊으면(읽기가 끝남) 곧바로 안다 — 승인 대기를 거둔다(rmcp 의 5초 drain 을 기다리지 않는다).
    let (reader, writer) = io;
    let io = (
        WatchEof {
            inner: reader,
            closed: session.inner.closed.clone(),
        },
        writer,
    );
    let running = session
        .serve_with_ct(io, session_cancel.clone())
        .await
        .map_err(|e| ServeError::Initialize(e.to_string()))?;
    // 세션 등록부(M5-10): 초기화를 마친 세션을 올리고 끝나면(가드가 버려지면) 내린다. 2025-11-25 까지의 클라이언트는 여기서 이름을 안다.
    let _registered = running.service().inner.services.sessions.register(
        &running.service().inner.id,
        running
            .peer()
            .peer_info()
            .map(|i| format!("{} {}", i.client_info.name, i.client_info.version)),
        running
            .service()
            .inner
            .project
            .as_ref()
            .map(|p| p.id.clone()),
        running.service().inner.services.transport,
    );
    // 앱이 끝날 때 이 세션의 감사 큐를 비운다(`Sessions::shutdown`).
    let flush_queue = audit.clone();
    let _flusher = running.service().inner.services.sessions.add_flusher(
        &running.service().inner.id,
        Arc::new(move |deadline| flush_audit_blocking(&flush_queue, deadline)),
    );
    let result = running.waiting().await;
    tracing::debug!(?result, "mcp session ended");
    flush_audit(&audit).await;
    Ok(())
}

/// 읽기 쪽이 끝나면(EOF · 오류) `closed` 를 취소한다.
struct WatchEof<R> {
    inner: R,
    closed: CancellationToken,
}

impl<R: AsyncRead + Unpin> AsyncRead for WatchEof<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        match &polled {
            Poll::Ready(Ok(())) if buf.filled().len() == before && buf.remaining() > 0 => {
                self.closed.cancel();
            }
            Poll::Ready(Err(_)) => self.closed.cancel(),
            _ => {}
        }
        polled
    }
}

/// 세션의 프로젝트.
#[derive(Debug, Clone)]
struct SessionProject {
    id: String,
    /// 세션을 연 경로가 미등록 연결 워크트리 안이면 그 꼭대기(`Resolution.worktree`). 빈 `worktree` 입력을 이것으로 채운다.
    worktree: Option<String>,
}

#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    services: Arc<Services>,
    client: HostClient,
    project: Option<SessionProject>,
    exposure: watch::Receiver<Arc<Exposure>>,
    /// 노출을 지금 다시 읽어 `exposure` 에 넣는다([`ExposureFeed::refresh`]).
    refresh_exposure: Arc<dyn Fn() + Send + Sync>,
    id: String,
    /// 세션이 끝나면 취소된다([`serve`] 가 취소한다). 설정 감시 · 버스 구독 · list_changed 태스크가 여기에 묶인다.
    cancel: CancellationToken,
    client_info: Mutex<Option<String>>,
    /// 감사 기록 큐([`audit_writer`]). 세션의 호출 순서대로 쓰인다.
    audit: std::sync::mpsc::Sender<AuditJob>,
    /// 클라이언트가 끊었거나 세션이 끝났다([`WatchEof`] · `cancel` 의 자식). 승인 대기는 이것이 취소되면 거둔다.
    closed: CancellationToken,
    /// 「이 세션 동안 허용」: (프로젝트, 툴 이름 · 계획의 열쇠). 다른 머신 호출의 열쇠에는 그 머신이 든다([`machine_key`]).
    allowed: Mutex<BTreeSet<(Option<String>, Allowance)>>,
    /// 「이 세션 동안 이 머신 허용」: (프로젝트 — 묶음의 이 머신 멤버, 머신 id)(18-m7-plan M7-18). 프로젝트가 들어 있어 한 창에서
    /// 준 허용이 다른 묶음의 같은 머신 멤버에 닿지 않는다(리뷰 F1 — CLI 판 `cli_machines` 와 같은 열쇠).
    machines_allowed: Mutex<BTreeSet<(String, String)>>,
    /// 머신마다의 연결(다른 머신만 — 이 머신은 `client`).
    pool: checkout::Pool,
}

/// 풀었지만 아직 연결을 잇지 않은 다른 머신 대상([`Session::aim`] → [`Session::link`]).
struct Unlinked {
    machine: String,
    project: String,
    routing: String,
    /// 이 머신 체크아웃이 없는 묶음(원격만)이다 — `routing` 이 묶음 id 이고, 노출 · 정책은 그 머신 Host 가 판정한다(`mcp.target` —
    /// 18-m7-plan M7-20, M7-11b 판정).
    remote_only: bool,
    /// 에이전트에게 보일 머신 이름.
    shown: String,
}

/// 승인을 지난 호출의 승인 방식과 계획 해시.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Approved {
    mode: AuditApproval,
    hash: String,
}

/// 승인을 받지 못한 까닭(에이전트에게 가는 글 · 표지).
struct Refusal {
    message: String,
    mark: Mark,
}

impl Refusal {
    fn denied(reason: Reason, message: String) -> Self {
        Self {
            message,
            mark: Mark::denied(reason),
        }
    }

    fn cancelled() -> Self {
        Self {
            message: Reason::Cancelled.as_str().to_owned(),
            mark: Mark::error(Some(Reason::Cancelled)),
        }
    }
}

/// 감사 쓰기 스레드가 받는 일.
enum AuditJob {
    /// 끝난 호출의 새 행(`read` · 거절 · 선기록 없이 끝난 호출).
    Write(Entry),
    /// 선기록([`Session::begin`])한 행을 같은 id 로 고친다.
    Finish(i64, Entry),
    /// 앞서 받은 기록을 다 썼으면 알린다([`flush_audit`]).
    Flush(tokio::sync::oneshot::Sender<()>),
    /// [`Self::Flush`] 의 블로킹 꼴([`flush_audit_blocking`] — 앱 종료 길).
    FlushSync(std::sync::mpsc::Sender<()>),
}

/// 지금까지 `audit` 에 넣은 기록이 다 쓰일 때까지 기다린다(큐는 차례대로 처리된다). 쓰기 스레드가 없으면 곧바로 돌아온다.
async fn flush_audit(audit: &std::sync::mpsc::Sender<AuditJob>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if audit.send(AuditJob::Flush(tx)).is_ok() {
        let _ = rx.await;
    }
}

/// [`flush_audit`] 의 블로킹 꼴: `deadline` 까지 기다린다. 다 썼으면(쓰기 스레드가 없어도) `true`.
fn flush_audit_blocking(audit: &std::sync::mpsc::Sender<AuditJob>, deadline: Instant) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    if audit.send(AuditJob::FlushSync(tx)).is_err() {
        return true;
    }
    let left = deadline.saturating_duration_since(Instant::now());
    !matches!(
        rx.recv_timeout(left),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    )
}

/// 세션 하나의 감사 기록을 **받은 순서대로** 쓰는 스레드를 띄우고 그 큐를 돌려준다. 호출마다 `spawn_blocking` 으로 쓰면 blocking
/// 풀의 스레드 배정과 잠금 획득 순서에 보장이 없어 행 순서(`id`)가 호출 순서와 갈린다. 응답은 큐에 넣기만 하고 기록을 기다리지
/// 않는다 — 세션이 끝날 때 [`serve`] 가 [`flush_audit`] 로 기다린다. 큐의 송신 쪽이 모두 버려지면 남은 기록을 쓰고 스레드가 끝난다.
fn audit_writer(
    log: Arc<AuditLog>,
    sessions: Sessions,
    session: &str,
) -> std::sync::mpsc::Sender<AuditJob> {
    let (tx, rx) = std::sync::mpsc::channel::<AuditJob>();
    let spawned = std::thread::Builder::new()
        .name("mcp-audit".to_owned())
        .spawn(move || {
            for job in rx {
                match job {
                    AuditJob::Write(entry) => {
                        match log.record(&entry) {
                            // 행이 실제로 쓰였을 때만 `mcp.activity` 를 낸다(M5-10).
                            Ok(row) => sessions.recorded(row, entry.project_id.as_deref()),
                            Err(e) => {
                                tracing::warn!(%e, tool = entry.tool, "cannot write the mcp audit log");
                            }
                        }
                    }
                    // 선기록한 행의 끝. 고치지 못하면 그 행은 `started`(끝을 모름)로 남는다 — 툴은 이미 돌았다.
                    AuditJob::Finish(row, entry) => match log.finish(row, &entry) {
                        // 같은 행이 바뀌었다 — 시작 때와 같은 id 로 `mcp.activity` 를 한 번 더 낸다.
                        Ok(true) => sessions.recorded(row, entry.project_id.as_deref()),
                        Ok(false) => tracing::warn!(
                            row,
                            tool = entry.tool,
                            "the started mcp audit row is gone; its end is not recorded"
                        ),
                        Err(e) => tracing::warn!(
                            %e,
                            row,
                            tool = entry.tool,
                            "cannot write the end of the mcp audit row; it stays started"
                        ),
                    },
                    AuditJob::Flush(done) => {
                        let _ = done.send(());
                    }
                    AuditJob::FlushSync(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
    if let Err(e) = spawned {
        // 받는 쪽이 없으니 보내기가 실패하고 그때마다 경고가 남는다(audit 머리 주석: 기록 실패는 경고로만).
        tracing::warn!(%e, session, "cannot start the mcp audit writer");
    }
    tx
}

impl Session {
    async fn open(
        services: Arc<Services>,
        request: &SessionRequest,
        cancel: CancellationToken,
    ) -> Result<Self, ServeError> {
        let conn = services
            .router
            .connect(
                &Target::HostId(services.host_id.clone()),
                Caller::agent(Role::Owner),
            )
            .await
            .map_err(|e| ServeError::Host(e.to_string()))?;
        let client = HostClient::new(conn);
        let resolved = match request.path() {
            None => None,
            Some(path) => resolve(&client, path).await,
        };
        let project_config = resolved
            .as_ref()
            .and_then(|r| ProjectConfig::open(&services.paths, r, &cancel));
        let ExposureFeed {
            receiver: exposure,
            refresh: refresh_exposure,
        } = exposure_feed(
            services.global.clone(),
            &services.bus,
            project_config,
            cancel.clone(),
        );
        let project = resolved.map(|r| SessionProject {
            id: r.project.id,
            worktree: r.worktree.map(|w| w.top),
        });
        let id = session_id();
        let audit = audit_writer(Arc::clone(&services.audit), services.sessions.clone(), &id);
        tracing::info!(
            session = %id,
            transport = services.transport,
            project = project.as_ref().map(|p| p.id.as_str()),
            "mcp session started"
        );
        Ok(Self {
            inner: Arc::new(Inner {
                services,
                client,
                project,
                exposure,
                refresh_exposure,
                id,
                client_info: Mutex::new(None),
                audit,
                closed: cancel.child_token(),
                allowed: Mutex::new(BTreeSet::new()),
                machines_allowed: Mutex::new(BTreeSet::new()),
                pool: checkout::Pool::default(),
                cancel,
            }),
        })
    }

    /// 지금 보이는 툴. 목록 · 호출마다 노출을 다시 읽는다 — 사람 레이어 상한(공유 레이어를 뺀 읽기)은 git 인덱스로 개인 파일인지를
    /// 가르는데, 인덱스만 바뀐 경우(`git add -f` · `git rm --cached`)는 `.pitwall` 감시의 `config.changed` 가 오지 않는다
    /// (17-m6-plan M6-12 ⑥ — m6-localpolicy 리뷰 R1). 바뀌었으면 `exposure` 에도 넣어 `tools/list_changed` 가 나간다.
    ///
    /// 다시 읽기는 설정 파일 · git 인덱스 · 객체 DB 를 읽으므로 블로킹 스레드에서 한다(async 워커를 막지 않게). 블로킹 풀이 그
    /// 일을 거두면(런타임이 끝나는 중) 마지막 값으로 판정한다.
    async fn visible(&self) -> Vec<&Tool> {
        let refresh = Arc::clone(&self.inner.refresh_exposure);
        let _ = tokio::task::spawn_blocking(move || refresh()).await;
        let exposure = Arc::clone(&self.inner.exposure.borrow());
        self.inner
            .services
            .catalog
            .tools()
            .iter()
            // 세션 툴은 다른 머신을 아는 앱 세션에만(카탈로그가 이미 그렇게 만든다 — 한 번 더 막는다).
            .filter(|t| {
                !t.is_session() || (self.inner.services.machines.is_some() && !self.headless())
            })
            .filter(|t| exposure.enabled(t))
            .collect()
    }

    fn to_mcp(&self, tool: &Tool) -> rmcp::model::Tool {
        let mut schema = tool.input_schema_for(self.inner.project.is_some());
        if self.offers_checkout(tool) {
            schema = checkout::with_checkout(schema);
        }
        let mut out = rmcp::model::Tool::new(
            tool.name.clone(),
            tool.description.clone(),
            Arc::new(schema),
        );
        if let Some(output) = &tool.output_schema {
            out = out.with_raw_output_schema(Arc::clone(output));
        }
        out.with_annotations(
            rmcp::model::ToolAnnotations::new().read_only(tool.tier == pw_rpc::Tier::Read),
        )
    }

    /// 세션이 헤드리스 Host 에 붙었는가([`HostKind`]).
    fn headless(&self) -> bool {
        self.inner.services.host_kind == HostKind::Headless
    }

    /// 승인을 물을 사람이 붙어 있는가(결정 M7-D10 ④ — Relay 만 거짓일 수 있다, [`crate::approval::Approvals::attended`]).
    fn attended(&self) -> bool {
        match &self.inner.services.approval {
            Approval::Route(route) => route.attended(),
            // 승인 경로가 없는 세션은 헤드리스인지로 가른다([`Self::headless`]) — 붙은 사람과 상관없다.
            Approval::Unavailable => true,
        }
    }

    /// 이 툴의 스키마에 `checkout` 을 더하는가: 다른 머신을 아는 앱 세션이고 툴이 `worktree` 를 받는다(결정 M5-D15 —
    /// `project_info` 처럼 체크아웃을 보지 않는 툴에는 달지 않는다).
    fn offers_checkout(&self, tool: &Tool) -> bool {
        self.inner.services.machines.is_some() && !self.headless() && takes_worktree(tool)
    }

    /// `cancel` 은 rmcp 가 그 요청에 건 취소 토큰이다 — 클라이언트의 취소 알림(`notifications/cancelled`)이 오거나 rmcp 의
    /// 서비스 루프가 끝나면 취소된다.
    async fn call(
        &self,
        request: CallToolRequestParams,
        cancel: CancellationToken,
    ) -> Result<CallToolResult, ErrorData> {
        let started = Instant::now();
        let name = request.name.to_string();
        let mut args = request.arguments.unwrap_or_default();
        // 승인 표의 자리는 세션만 싣는다 — 에이전트가 보낸 같은 키는 버린다(결정 M5-D30).
        if args.remove(APPROVAL_KEY).is_some() {
            tracing::warn!(tool = name, "dropped an approval key the agent sent");
        }
        let tool = self
            .visible()
            .await
            .into_iter()
            .find(|t| t.name == name)
            .cloned();
        let Some(tool) = tool else {
            self.audit(
                None,
                &name,
                &args,
                None,
                Outcome::Denied,
                Some("tool is not exposed".into()),
                None,
                started,
            );
            return Err(ErrorData::invalid_params(
                format!("unknown tool `{name}`"),
                Some(Value::Object(Mark::denied(Reason::NotExposed).to_map())),
            ));
        };
        // 세션 툴(`checkout_list` · `checkout_send`)은 Host 메서드가 아니다 — 세션이 처리한다(18-m7-plan M7-19).
        if tool.is_session() {
            return Ok(self.session_tool(&tool, args, started, &cancel).await);
        }
        // `checkout` 은 세션의 입력이다 — Host 로 가지 않는다(D57 ④).
        let checkout = checkout::take(&mut args);
        if tool.project_scoped()
            && !args.contains_key(PROJECT_ID)
            && let Some(project) = &self.inner.project
        {
            args.insert(PROJECT_ID.into(), Value::String(project.id.clone()));
        }
        let mut project_id = args
            .get(PROJECT_ID)
            .and_then(Value::as_str)
            .map(str::to_owned);
        let aimed = match checkout {
            Ok(checkout) => self.aim(&tool, checkout, &mut args, &mut project_id).await,
            Err(message) => Err(Refusal {
                message,
                mark: Mark::error(None),
            }),
        };
        let (unlinked, explicit) = match aimed {
            Ok(aimed) => aimed,
            Err(refusal) => {
                self.audit(
                    None,
                    &name,
                    &args,
                    project_id,
                    refusal.mark.outcome,
                    Some(refusal.message.clone()),
                    None,
                    started,
                );
                return Ok(failed(refusal.message, refusal.mark));
            }
        };
        if !explicit {
            self.fill_worktree(&tool, &mut args, project_id.as_deref());
        }
        // 이 머신 체크아웃이 없는 묶음(원격만)은 이 머신에 그 프로젝트의 설정이 없다 — 그 머신 Host 가 판정한다(붙은 뒤, 아래). 전역
        // 설정만으로 판정하지 않는다(프로젝트에서 막은 툴을 건너뛰게 된다).
        let remote_only = unlinked.as_ref().is_some_and(|u| u.remote_only);
        // 대상 프로젝트의 설정 · 노출 판정은 한 자리다 — 다른 머신을 겨눈 호출도 여기서 `project_id` 는 묶음의 이 머신 멤버(에이전트가
        // 고른 Desktop 쪽 프로젝트)다(리뷰 B1). 그 프로젝트가 끈 툴은 `checkout` 을 달아도 어디서도 돌지 않는다. 다른 머신 자신의 설정은
        // 이 세션이 읽지 않는다(그 머신의 로컬 규칙 — M7-20). 승인은 `auto_approve` 없이 묻는다.
        if tool.project_scoped()
            && !remote_only
            && let Some(target) = project_id.as_deref()
        {
            let refused = match self.target_config_unreadable(target, &cancel).await {
                Ok(refused) => refused,
                Err(cancelled) => Some(cancelled),
            };
            if let Some(refusal) = refused {
                self.audit(
                    None,
                    &name,
                    &args,
                    project_id,
                    refusal.mark.outcome,
                    Some(refusal.message.clone()),
                    None,
                    started,
                );
                return Ok(failed(refusal.message, refusal.mark));
            }
        }
        if tool.project_scoped()
            && !remote_only
            && let Some(target) = project_id.as_deref()
            && self.inner.project.as_ref().is_none_or(|p| p.id != target)
        {
            match self.target_exposes(&tool, target, &cancel).await {
                Ok(true) => {}
                Ok(false) => {
                    let reason = format!(
                        "`{name}` is turned off in the [mcp] settings of project `{target}`"
                    );
                    self.audit(
                        None,
                        &name,
                        &args,
                        project_id,
                        Outcome::Denied,
                        Some(reason.clone()),
                        None,
                        started,
                    );
                    return Ok(failed(reason, Mark::denied(Reason::NotExposed)));
                }
                Err(e) => {
                    return Ok(self.call_failed(None, &name, &args, project_id, e, started, None));
                }
            }
        }
        let remote = match unlinked {
            None => None,
            Some(target) => match self.link(target, &cancel).await {
                Ok(aim) => Some(aim),
                Err(refusal) => {
                    self.audit(
                        None,
                        &name,
                        &args,
                        project_id,
                        refusal.mark.outcome,
                        Some(refusal.message.clone()),
                        None,
                        started,
                    );
                    return Ok(failed(refusal.message, refusal.mark));
                }
            },
        };
        let remote_timeout = match (&remote, remote_only) {
            (Some(aim), true) => match self.remote_target(&tool, aim, &cancel).await {
                Ok(timeout) => Some(timeout),
                Err(refusal) => {
                    self.audit(
                        None,
                        &name,
                        &args,
                        project_id,
                        refusal.mark.outcome,
                        Some(refusal.message.clone()),
                        None,
                        started,
                    );
                    return Ok(failed(refusal.message, refusal.mark));
                }
            },
            _ => None,
        };
        if let Decision::Deny(reason) = policy::decide(tool.tier, &self.inner.services.approval) {
            // 붙은 owner Desktop 이 없는 Relay 는 헤드리스와 같다(결정 M7-D10 ④ · D61) — 표지도 같다.
            let code = if self.headless() || !self.attended() {
                Reason::HeadlessWrite
            } else {
                Reason::NeedsApproval
            };
            self.audit(
                None,
                &name,
                &args,
                project_id,
                Outcome::Denied,
                Some(reason.clone()),
                None,
                started,
            );
            return Ok(failed(reason, Mark::denied(code)));
        }
        if tool.planned {
            return self
                .planned(
                    &tool,
                    args,
                    project_id,
                    started,
                    &cancel,
                    remote.as_ref(),
                    remote_timeout,
                )
                .await;
        }
        // 카탈로그가 `write` 이상을 모두 `planned` 로 만든다 — 여기 오는 것은 계획 없는 `read` 툴뿐이다.
        self.run(
            &tool,
            args,
            project_id,
            started,
            &cancel,
            None,
            remote.as_ref(),
        )
        .await
    }

    /// 계획을 만드는 툴의 호출(머리 주석의 5단계). 첫 호출은 승인 표 없이 — 툴은 부작용 없이 계획만 만든다.
    async fn planned(
        &self,
        tool: &Tool,
        args: Map<String, Value>,
        project_id: Option<String>,
        started: Instant,
        cancel: &CancellationToken,
        remote: Option<&RemoteAim>,
        remote_timeout: Option<Duration>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = tool.name.clone();
        let first = self
            .client_for(remote)
            .call_until(tool.method, Value::Object(sent(&args)), cancel)
            .await;
        let plan = match first {
            // 이 호출에는 승인을 청하지 않았다(`read` 툴).
            Ok(result) if tool.tier == Tier::Read => {
                return Ok(
                    self.finished(None, &name, &args, project_id, tool, result, started, None)
                );
            }
            Ok(_) => {
                // 툴이 승인 표 없이 돌았다 — 규약 위반(프로그래밍 오류). 결과를 에이전트에게 주지 않는다.
                tracing::error!(tool = name, "a planned tool ran without an approval ticket");
                let message = format!(
                    "`{name}` ran without asking for approval in Pitwall; this is a Pitwall bug, please report it"
                );
                self.audit(
                    None,
                    &name,
                    &args,
                    project_id,
                    Outcome::Error,
                    Some(message.clone()),
                    None,
                    started,
                );
                return Ok(failed(message, Mark::error(None)));
            }
            Err(CallError::Rpc { code, data, .. }) if code == i64::from(APPROVAL_REQUIRED) => {
                match data.and_then(|d| serde_json::from_value::<ApprovalRequired>(d).ok()) {
                    Some(required) => match plan_problem(tool, &required.plan) {
                        None => required.plan,
                        // 툴이 규약을 어겼다(프로그래밍 오류) — 묻지 않고 돌리지 않는다.
                        Some(problem) => {
                            tracing::error!(
                                tool = name,
                                problem,
                                "a tool sent a plan that does not fit it"
                            );
                            let message = format!(
                                "`{name}` sent an approval plan that does not fit it ({problem}); this is a Pitwall bug, please report it"
                            );
                            self.audit(
                                None,
                                &name,
                                &args,
                                project_id,
                                Outcome::Error,
                                Some(message.clone()),
                                None,
                                started,
                            );
                            return Ok(failed(message, Mark::error(None)));
                        }
                    },
                    None => {
                        let message = format!("`{name}` sent a plan Pitwall cannot read");
                        self.audit(
                            None,
                            &name,
                            &args,
                            project_id,
                            Outcome::Error,
                            Some(message.clone()),
                            None,
                            started,
                        );
                        return Ok(failed(message, Mark::error(None)));
                    }
                }
            }
            Err(e) => return Ok(self.call_failed(None, &name, &args, project_id, e, started, None)),
        };
        // 앱이 끝날 때(`Sessions::shutdown`) 이 호출이 거절 기록을 큐에 넣을 때까지 기다리게 한다.
        let hold = match &self.inner.services.approval {
            Approval::Route(route) => Some(route.hold()),
            Approval::Unavailable => None,
        };
        let decided = match remote {
            Some(aim) => {
                self.approve_remote(tool, &plan, aim, remote_timeout, cancel)
                    .await
            }
            None => {
                self.approve(tool, &plan, project_id.as_deref(), cancel)
                    .await
            }
        };
        let approved = match decided {
            Ok(mode) => Approved {
                mode,
                hash: plan.hash,
            },
            Err(refusal) => {
                self.audit(
                    None,
                    &name,
                    &args,
                    project_id,
                    refusal.mark.outcome,
                    Some(refusal.message.clone()),
                    None,
                    started,
                );
                drop(hold);
                return Ok(failed(refusal.message, refusal.mark));
            }
        };
        drop(hold);
        self.run(
            tool,
            args,
            project_id,
            started,
            cancel,
            Some(approved),
            remote,
        )
        .await
    }

    /// 계획 하나의 승인 방식을 정한다: 정책 자동(`policyAuto`) → `auto_approve` → 세션 허용 · CLI 허용 → 창에 묻기. 등급은 계획
    /// 등급이다. `dangerous` 계획은 매번 묻고 기억하지 않는다. 묻는 동안 호출이 취소되거나 클라이언트가 끊으면 요청을 거둔다.
    async fn approve(
        &self,
        tool: &Tool,
        plan: &ToolPlan,
        project_id: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<AuditApproval, Refusal> {
        let name = &tool.name;
        let Approval::Route(route) = &self.inner.services.approval else {
            return Err(Refusal::denied(
                Reason::NeedsApproval,
                format!(
                    "`{name}` needs approval in the Pitwall app for this call, and this session has no approval route"
                ),
            ));
        };
        // 붙은 사람이 없는 동안(결정 M7-D10 ④)은 정책 자동 · `auto_approve` 보다 앞에서 거절한다 — 선언 등급으로 이미 거절했지만
        // (`policy::decide`) 그 뒤 계획을 받는 사이 마지막 Desktop 이 떠났을 수 있다.
        if !route.attended() {
            return Err(Refusal::denied(
                Reason::HeadlessWrite,
                policy::unattended(plan.tier),
            ));
        }
        // 판정은 계획 등급이다(17-m6-plan 결정 M6-P9). 선언 등급은 상한이다 — 계획이 그보다 높으면 `planned` 가 이미 툴의 버그로
        // 거절했다(`plan_problem`). 노출 · 헤드리스 거절 · `readOnly` 표지는 선언 등급으로 이미 정해졌다.
        let tier = plan.tier;
        let dangerous = tier == Tier::Dangerous;
        // 정책 자동: 툴이 05 의 규칙으로 묻지 않아도 된다고 판정했다(`dangerous` 계획의 표지는 `plan_problem` 이 거절했다). 승인 표를
        // 실어 다시 부르는 것 · 선기록 · 해시 재확인은 그대로다. 다른 머신을 겨누는 호출(M7 의 `checkout`)에는 `auto_approve` 와
        // 같이 적용하지 않는다 — M7 이 그 세션의 머신 허용 뒤에 얹을지 정한다(지금은 그런 호출이 없다). 이 갈래는 대상 프로젝트의
        // 설정(`policy_for`)보다 앞이라 설정이 정책 자동을 끌 수 없다 — 그런 설정(「정책 자동도 묻기」)을 더하면 이 순서를 바꾼다.
        if plan.rules.policy_auto {
            return Ok(AuditApproval::Policy);
        }
        let policy = self.policy_for(project_id, cancel).await?;
        if !dangerous && plan.rules.auto_approve && policy.auto_approves(tier) {
            return Ok(AuditApproval::Auto);
        }
        // 기억하는 단위는 늘 그 툴의 것이고 그 계획 등급의 것이다 — 같은 열쇠를 내는 다른 툴에도, 같은 툴의 다른 등급 계획(낮춘
        // `write` 계획에 준 허용 → `exec` 계획)에도 닿지 않는다.
        let key = if dangerous {
            None
        } else {
            match &plan.rules.remember {
                Remember::Tool => Some(Allowance {
                    tool: name.clone(),
                    key: None,
                    tier,
                }),
                Remember::Key { key } => Some(Allowance {
                    tool: name.clone(),
                    key: Some(key.clone()),
                    tier,
                }),
                Remember::Never => None,
            }
        };
        let cli = self.is_cli();
        if let Some(key) = &key {
            if self.session_allows(project_id, key) {
                return Ok(AuditApproval::Session);
            }
            if cli && project_id.is_some_and(|p| route.cli_allowed(p, key)) {
                return Ok(AuditApproval::Window);
            }
        }
        let third = key.as_ref().and_then(|_| {
            if cli {
                // CLI 허용은 그 프로젝트 창에 묶인다 — 프로젝트 없는 요청에는 없다.
                project_id.map(|_| McpApprovalAnswer::Window)
            } else {
                Some(McpApprovalAnswer::Session)
            }
        });
        let mut answers = vec![McpApprovalAnswer::Deny, McpApprovalAnswer::Once];
        answers.extend(third);
        let ask = ApprovalAsk {
            project_id: project_id.map(str::to_owned),
            machine: None,
            session: self.inner.id.clone(),
            client: self.client(),
            cli,
            tool: name.clone(),
            tier,
            plan: plan.clone(),
            answers,
            remember: if cli { key.clone() } else { None },
            remember_machine: None,
            timeout: policy.approval_timeout,
        };
        match self
            .ask_window(route, ask, name, project_id, cancel)
            .await?
        {
            McpApprovalAnswer::Session => {
                if let Some(key) = key {
                    self.inner
                        .allowed
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert((project_id.map(str::to_owned), key));
                }
                Ok(AuditApproval::Session)
            }
            McpApprovalAnswer::Window => Ok(AuditApproval::Window),
            // 이 머신 호출에는 [이 머신 허용] 을 싣지 않는다 — 허브가 싣지 않은 답을 거절한다(`AnswerError::NotOffered`).
            McpApprovalAnswer::Once | McpApprovalAnswer::Machine | McpApprovalAnswer::Deny => {
                Ok(AuditApproval::Once)
            }
        }
    }

    /// 창에 묻고 답을 기다린다. [거절] · 창 없음 · 시한 · 취소는 `Err`. 답과 끊김이 겹치면 돌리지 않는다.
    async fn ask_window(
        &self,
        route: &crate::approval::Approvals,
        ask: ApprovalAsk,
        name: &str,
        project_id: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<McpApprovalAnswer, Refusal> {
        let timeout = ask.timeout;
        let answered = tokio::select! {
            answered = route.ask(ask) => answered,
            () = cancel.cancelled() => return Err(Refusal::cancelled()),
            () = self.inner.closed.cancelled() => return Err(Refusal::cancelled()),
        };
        // 답과 끊김이 겹쳤으면 돌리지 않는다 — 끊긴 세션의 요청은 [허용] 을 받아도 돌지 않는다.
        if cancel.is_cancelled() || self.inner.closed.is_cancelled() {
            return Err(Refusal::cancelled());
        }
        let window = match project_id {
            Some(p) => format!("no Pitwall window of project `{p}` is open"),
            None => "no Pitwall window is open".to_owned(),
        };
        match answered {
            Ok(McpApprovalAnswer::Deny) => Err(Refusal::denied(
                Reason::ApprovalDenied,
                format!("the call to `{name}` was denied in Pitwall"),
            )),
            Ok(answer) => Ok(answer),
            Err(AskError::NoWindow) => Err(Refusal::denied(
                Reason::NeedsApproval,
                format!(
                    "`{name}` needs approval, and {window} to ask; open it in Pitwall and call again"
                ),
            )),
            Err(AskError::WindowLeft) => Err(Refusal::denied(
                Reason::NeedsApproval,
                format!(
                    "the Pitwall window closed before anyone answered the approval request for `{name}`"
                ),
            )),
            Err(AskError::TimedOut) => Err(Refusal::denied(
                Reason::ApprovalTimeout,
                format!(
                    "no one answered the approval request for `{name}` in Pitwall within {} seconds",
                    timeout.as_secs()
                ),
            )),
            Err(AskError::Backlog) => Err(Refusal::denied(
                Reason::ApprovalBacklog,
                format!(
                    "`{name}` was not shown for approval: this session already has {MAX_PENDING_PER_SESSION} approval requests waiting in Pitwall; wait for them to be answered and call again"
                ),
            )),
            Err(AskError::Shutdown) => Err(Refusal::cancelled()),
        }
    }

    /// 다른 머신을 겨눈 계획의 승인(05 「다른 머신을 겨누는 호출」 · 결정 M7-D9). `auto_approve` · 정책 자동은 적용하지 않는다 —
    /// 원격이 낸 계획의 `policyAuto` · `auto_approve` 를 넓히는 데 읽지 않는다(M7-P19). 「이 세션 동안 이 머신 허용」 은 신뢰한
    /// 대상의 `write` · `exec` 계획 가운데 `auto_approve` 가 닿을 수 있는 것(사람이 띄운 실행처럼 툴이 막은 것은 빼고 — D69)과
    /// 기억할 수 있는 것에만 닿는다. 미신뢰 대상 · `dangerous` 는 매번, 호출마다 청하는 승인(`read` 계획)은 지금 규칙(그 머신의
    /// 그 열쇠)대로 묻는다. 묻는 창은 묶음의 로컬 멤버 창이다.
    async fn approve_remote(
        &self,
        tool: &Tool,
        plan: &ToolPlan,
        aim: &RemoteAim,
        remote_timeout: Option<Duration>,
        cancel: &CancellationToken,
    ) -> Result<AuditApproval, Refusal> {
        let name = &tool.name;
        let Approval::Route(route) = &self.inner.services.approval else {
            return Err(Refusal::denied(
                Reason::NeedsApproval,
                format!(
                    "`{name}` needs approval in the Pitwall app for this call, and this session has no approval route"
                ),
            ));
        };
        if !route.attended() {
            return Err(Refusal::denied(
                Reason::HeadlessWrite,
                policy::unattended(plan.tier),
            ));
        }
        let tier = plan.tier;
        let writes = matches!(tier, Tier::Write | Tier::Exec);
        // 신뢰는 대상 머신의 것이다. 읽지 못하면 미신뢰로 본다(매번 묻는다 — 넓히지 않는 쪽).
        let trusted = writes && self.remote_trusted(aim, cancel).await?;
        let by_machine =
            writes && trusted && plan.rules.auto_approve && plan.rules.remember != Remember::Never;
        let cli = self.is_cli();
        let routing = aim.routing.as_str();
        if by_machine {
            // 감사의 승인 방식은 `session` 이다 — 이 세션 동안의 허용이고, 대상 머신은 같은 행의 인자 요약(`checkout`)에 있다.
            // 승인 방식 열의 `CHECK` 를 넓히려면 감사 파일을 새 판으로 옮겨야 한다(audit.rs 「셋째 판」 의 꼴) — 하지 않았다.
            if self.machine_allows(routing, &aim.machine) {
                return Ok(AuditApproval::Session);
            }
            if cli && route.cli_machine_allowed(routing, &aim.machine) {
                return Ok(AuditApproval::Window);
            }
        }
        let key = if tier == Tier::Read {
            match &plan.rules.remember {
                Remember::Tool => Some(machine_key(name, None, tier, &aim.machine)),
                Remember::Key { key } => Some(machine_key(name, Some(key), tier, &aim.machine)),
                Remember::Never => None,
            }
        } else {
            None
        };
        if let Some(key) = &key {
            if self.session_allows(Some(routing), key) {
                return Ok(AuditApproval::Session);
            }
            if cli && route.cli_allowed(routing, key) {
                return Ok(AuditApproval::Window);
            }
        }
        let mut answers = vec![McpApprovalAnswer::Deny, McpApprovalAnswer::Once];
        if by_machine {
            answers.push(McpApprovalAnswer::Machine);
        }
        if key.is_some() {
            answers.push(if cli {
                McpApprovalAnswer::Window
            } else {
                McpApprovalAnswer::Session
            });
        }
        // 승인 시한은 묶음의 이 머신 멤버의 정책이다. 원격만 묶음이면 그 머신 Host 가 판정한 그 프로젝트의 정책(`mcp.target`).
        let timeout = match remote_timeout {
            Some(timeout) => timeout,
            None => {
                self.policy_for(Some(routing), cancel)
                    .await?
                    .approval_timeout
            }
        };
        let ask = ApprovalAsk {
            project_id: Some(routing.to_owned()),
            machine: Some(aim.machine.clone()),
            session: self.inner.id.clone(),
            client: self.client(),
            cli,
            tool: name.clone(),
            tier,
            plan: plan.clone(),
            answers,
            remember: if cli { key.clone() } else { None },
            remember_machine: (cli && by_machine).then(|| aim.machine.clone()),
            timeout,
        };
        match self
            .ask_window(route, ask, name, Some(routing), cancel)
            .await?
        {
            McpApprovalAnswer::Machine => {
                if !cli {
                    self.inner
                        .machines_allowed
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert((routing.to_owned(), aim.machine.clone()));
                }
                Ok(if cli {
                    AuditApproval::Window
                } else {
                    AuditApproval::Session
                })
            }
            McpApprovalAnswer::Session => {
                if let Some(key) = key {
                    self.inner
                        .allowed
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert((Some(routing.to_owned()), key));
                }
                Ok(AuditApproval::Session)
            }
            McpApprovalAnswer::Window => Ok(AuditApproval::Window),
            McpApprovalAnswer::Once | McpApprovalAnswer::Deny => Ok(AuditApproval::Once),
        }
    }

    /// 대상 머신이 그 프로젝트를 신뢰하는가(그 머신의 `project.trust`). 읽지 못하면 `false`. 취소면 거절.
    async fn remote_trusted(
        &self,
        aim: &RemoteAim,
        cancel: &CancellationToken,
    ) -> Result<bool, Refusal> {
        let answer = aim
            .link
            .client
            .call_until(
                "project.trust",
                serde_json::json!({ "id": aim.project }),
                cancel,
            )
            .await;
        match answer {
            Ok(value) => Ok(
                serde_json::from_value::<pw_core::registry::trust::ProjectTrust>(value)
                    .is_ok_and(|t| t.trusted()),
            ),
            Err(CallError::Cancelled) => Err(Refusal::cancelled()),
            Err(e) => {
                tracing::warn!(%e, machine = aim.machine, "cannot read the trust of another machine's project; asking every time");
                Ok(false)
            }
        }
    }

    fn machine_allows(&self, project: &str, machine: &str) -> bool {
        self.inner
            .machines_allowed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(project.to_owned(), machine.to_owned()))
    }

    /// 호출을 보낼 연결: 다른 머신이면 그 머신 연결, 아니면 이 머신 Host.
    fn client_for<'a>(&'a self, remote: Option<&'a RemoteAim>) -> &'a HostClient {
        remote.map_or(&self.inner.client, |r| &r.link.client)
    }

    /// `checkout` 을 풀어 대상을 정한다(05 「체크아웃」). 돌려주는 것은 (다른 머신 대상, 이 머신 대상을 `checkout` 으로 명시했는가).
    /// - `checkout` 이 없으면: `projectId` 가 묶음 id 면 그 로컬 멤버로 바꾼다(없으면 거절). 나머지는 지금 그대로.
    /// - 이 머신을 고르면: 그 묶음의 로컬 멤버와 그 워크트리로 바꾼다.
    /// - 다른 머신을 고르면: 다른 머신을 아는 앱 세션 · 그 묶음 창이 열려 있음을 차례로 보고(붙는 것은 노출 판정 뒤 [`Self::link`]), 입력을 그 머신
    ///   프로젝트 id · 워크트리로 바꾼다. 감사 인자 요약에 남게 `checkout`(머신 id · 워크트리)을 입력에 다시 넣는다 — 보낼 때
    ///   떼어 낸다([`sent`]). `project_id` 는 승인 · 감사의 프로젝트(묶음의 로컬 멤버)가 된다.
    async fn aim(
        &self,
        tool: &Tool,
        checkout: Option<CheckoutInput>,
        args: &mut Map<String, Value>,
        project_id: &mut Option<String>,
    ) -> Result<(Option<Unlinked>, bool), Refusal> {
        let name = &tool.name;
        let machines = self.inner.services.machines.as_deref();
        let Some(checkout) = checkout else {
            // 묶음 id 를 이 머신 대상으로(체크아웃을 주지 않은 호출).
            if let (Some(machines), Some(pid)) = (machines, project_id.as_deref())
                && tool.project_scoped()
                && pw_core::registry::bundle::is_bundle_row_id(pid)
            {
                let bundle = bundle_of(machines, pid)?;
                let Some(local) = bundle.local_project() else {
                    return Err(Refusal {
                        message: format!(
                            "project `{pid}` has no checkout on this computer; give `checkout` with one of its machines"
                        ),
                        mark: Mark::error(None),
                    });
                };
                let local = local.to_owned();
                args.insert(PROJECT_ID.into(), Value::String(local.clone()));
                *project_id = Some(local);
            }
            return Ok((None, false));
        };
        let unavailable = |message: String| Refusal::denied(Reason::MachineUnavailable, message);
        let (Some(machines), false) = (machines, self.headless()) else {
            return Err(unavailable(
                "this session cannot target another checkout: only a session of the running Pitwall desktop app \
                 can (open the Pitwall app; a headless session, or a session on a remote machine's Pitwall relay, \
                 serves its own computer only)"
                    .to_owned(),
            ));
        };
        if !takes_worktree(tool) {
            return Err(Refusal {
                message: format!("`{name}` does not take `checkout`"),
                mark: Mark::error(None),
            });
        }
        let Some(pid) = project_id.clone() else {
            return Err(Refusal {
                message: format!("`{name}` with `checkout` needs `projectId`"),
                mark: Mark::error(None),
            });
        };
        let machine = if checkout::is_local(&checkout.machine) {
            checkout.machine.clone()
        } else {
            machines
                .machine_id(&checkout.machine)
                .ok_or_else(|| Refusal {
                    message: format!(
                        "unknown machine `{}`; project_list shows the machines of each project",
                        checkout.machine
                    ),
                    mark: Mark::error(None),
                })?
        };
        let bundle = bundle_of(machines, &pid)?;
        let Some(member) = bundle.member(&machine) else {
            return Err(Refusal {
                message: format!(
                    "project `{pid}` has no checkout on machine `{}`; project_list shows its machines",
                    checkout.machine
                ),
                mark: Mark::error(None),
            });
        };
        let given = match args.get(WORKTREE) {
            Some(Value::String(w)) if !w.is_empty() => Some(w.clone()),
            _ => None,
        };
        let worktree = match (checkout.worktree, given) {
            (Some(a), Some(b)) if a != b => {
                return Err(Refusal {
                    message: "`worktree` and `checkout.worktree` differ; give the worktree in `checkout` only"
                        .to_owned(),
                    mark: Mark::error(None),
                });
            }
            (a, b) => a.or(b),
        };
        match &worktree {
            Some(w) => {
                args.insert(WORKTREE.into(), Value::String(w.clone()));
            }
            None => {
                args.remove(WORKTREE);
            }
        }
        args.insert(PROJECT_ID.into(), Value::String(member.project_id.clone()));
        if checkout::is_local(&machine) {
            *project_id = Some(member.project_id.clone());
            return Ok((None, true));
        }
        let shown = machines
            .machine_name(&machine)
            .unwrap_or_else(|| machine.clone());
        // 승인을 물을 자리가 있어야 한다 — 그 묶음 창이 열려 있어야 한다(05 「체크아웃」). 창은 로컬 멤버 id 로, 이 머신 체크아웃이
        // 없는 묶음(원격만)의 창은 묶음 id 로 듣는다(M7-11b · M7-20). 묶음 id 는 프로젝트 id 와 겹치지 않는다(`b-` 접두).
        let remote_only = bundle.local_project().is_none();
        let routing = bundle
            .local_project()
            .map_or_else(|| bundle.id.clone(), str::to_owned);
        let hears = match &self.inner.services.approval {
            Approval::Route(route) => route.hears(&routing),
            Approval::Unavailable => false,
        };
        if !hears {
            return Err(unavailable(format!(
                "targeting machine `{shown}` needs the window of project `{}` open in Pitwall; open it and call again",
                bundle.name
            )));
        }
        args.insert(
            CHECKOUT.into(),
            checkout::audit_value(&machine, worktree.as_deref()),
        );
        *project_id = Some(routing.clone());
        Ok((
            Some(Unlinked {
                machine,
                project: member.project_id.clone(),
                routing,
                remote_only,
                shown,
            }),
            true,
        ))
    }

    /// 원격만 묶음의 대상을 그 머신 Host 에 묻는다(`mcp.target` — 그 프로젝트 설정으로 그 툴의 노출 · 승인 시한). 못 읽으면
    /// `project_config_unreadable`, 끈 툴이면 `not_exposed` 로 거절한다 — 그 머신이 답하지 못하면(옛 판 등) 넓히지 않고 거절한다.
    async fn remote_target(
        &self,
        tool: &Tool,
        aim: &RemoteAim,
        cancel: &CancellationToken,
    ) -> Result<Duration, Refusal> {
        let params = serde_json::json!({ "projectId": aim.project, "tool": tool.name });
        match aim
            .link
            .client
            .call_until("mcp.target", params, cancel)
            .await
        {
            Ok(value) => {
                let exposed = value.get("exposed").and_then(Value::as_bool) == Some(true);
                if !exposed {
                    return Err(Refusal::denied(
                        Reason::NotExposed,
                        format!(
                            "`{}` is turned off in the [mcp] settings of that machine's project",
                            tool.name
                        ),
                    ));
                }
                let secs = value
                    .get("approvalTimeoutSecs")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Refusal {
                        message: "that machine sent a target answer Pitwall cannot read".to_owned(),
                        mark: Mark::error(None),
                    })?;
                Ok(Duration::from_secs(secs))
            }
            Err(CallError::Cancelled) => Err(Refusal::cancelled()),
            Err(CallError::Rpc {
                code,
                message,
                data,
            }) if code == i64::from(pw_rpc::code::PERMISSION_DENIED)
                && data
                    .as_ref()
                    .and_then(|d| d.get("reason"))
                    .and_then(Value::as_str)
                    == Some(pw_core::config::mcp_policy::PROJECT_CONFIG_UNREADABLE) =>
            {
                Err(Refusal::denied(Reason::ProjectConfigUnreadable, message))
            }
            Err(e) => Err(Refusal::denied(
                Reason::MachineUnavailable,
                format!(
                    "that machine cannot tell whether `{}` may run on its project ({e}); refusing",
                    tool.name
                ),
            )),
        }
    }

    /// 풀린 다른 머신 대상에 연결을 잇는다(연결 풀 — 머신마다 하나). 노출 · 설정 판정을 지난 뒤에 부른다 — 노출이 끈 툴로는 그
    /// 머신에 붙지도 않는다.
    async fn link(
        &self,
        target: Unlinked,
        cancel: &CancellationToken,
    ) -> Result<RemoteAim, Refusal> {
        let Some(machines) = self.inner.services.machines.as_deref() else {
            return Err(Refusal::denied(
                Reason::MachineUnavailable,
                "this session cannot target another checkout".to_owned(),
            ));
        };
        let Unlinked {
            machine,
            project,
            routing,
            shown,
            remote_only: _,
        } = target;
        let link = tokio::select! {
            link = self.inner.pool.get(machines, &machine) => link,
            () = cancel.cancelled() => return Err(Refusal::cancelled()),
        }
        .map_err(|e| {
            Refusal::denied(
                Reason::MachineUnavailable,
                format!("cannot reach machine `{shown}`: {}", e.message()),
            )
        })?;
        Ok(RemoteAim {
            machine,
            project,
            routing,
            link,
        })
    }

    /// `project_list` 의 결과에 묶음을 싣는다(05 「체크아웃」 — `project_list` 는 묶음을 낸다): 묶음에 든 프로젝트에 `bundleId` ·
    /// `checkouts`(머신 id · Pitwall 의 이름 · 워크트리 없음 = 메인 체크아웃)를 더하고, 이 머신 체크아웃이 없는 묶음은 `id` 가 묶음 id 인
    /// 항목으로 뒤에 붙인다. 다른 머신을 모르는 세션은 그대로다.
    async fn with_bundles(&self, tool: &Tool, result: Value) -> Value {
        let Some(machines) = self.inner.services.machines.as_deref() else {
            return result;
        };
        if tool.method != "project.list" {
            return result;
        }
        let Value::Array(mut items) = result else {
            return result;
        };
        let bundles = match machines.bundles() {
            Ok(bundles) => bundles,
            Err(e) => {
                tracing::warn!(%e, "cannot read the bundles for project_list");
                return Value::Array(items);
            }
        };
        let checkouts = |b: &pw_core::registry::bundle::ProjectBundle| {
            Value::Array(
                b.members
                    .iter()
                    .map(|m| {
                        serde_json::json!({
                            "machine": m.machine,
                            "name": if checkout::is_local(&m.machine) {
                                Some(m.machine.clone())
                            } else {
                                machines.machine_name(&m.machine)
                            },
                        })
                    })
                    .collect(),
            )
        };
        for item in &mut items {
            let Some(object) = item.as_object_mut() else {
                continue;
            };
            let Some(id) = object.get("id").and_then(Value::as_str) else {
                continue;
            };
            if let Some(bundle) = bundles
                .iter()
                .find(|b| !b.implicit && b.local_project() == Some(id))
            {
                object.insert("bundleId".into(), Value::String(bundle.id.clone()));
                object.insert("checkouts".into(), checkouts(bundle));
            }
        }
        for bundle in bundles.iter().filter(|b| b.local_project().is_none()) {
            items.push(serde_json::json!({
                "id": bundle.id,
                "name": bundle.name,
                "bundleId": bundle.id,
                "checkouts": checkouts(bundle),
            }));
        }
        Value::Array(items)
    }

    /// 대상 프로젝트의 승인 정책(프로젝트 없는 호출은 전역). 프로젝트를 읽지 못하면 그 오류로 거절한다.
    async fn policy_for(
        &self,
        project_id: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Policy, Refusal> {
        let services = &self.inner.services;
        let Some(project_id) = project_id else {
            return Ok(policy::of_global(&services.global));
        };
        let project = self
            .project(project_id, cancel)
            .await
            .map_err(|e| match e {
                CallError::Cancelled => Refusal::cancelled(),
                e => Refusal {
                    message: e.to_string(),
                    mark: Mark::error(None),
                },
            })?;
        policy::of_project(&services.paths, &services.global, &project)
            .map_err(|e| Refusal::denied(Reason::ProjectConfigUnreadable, e.message()))
    }

    /// 대상 프로젝트의 설정 파일을 모두 읽을 수 있는가 — 못 읽으면 그 까닭(결과 `denied` · `project_config_unreadable`). 정책을
    /// 모르는 프로젝트에 닿는 호출은 등급과 상관없이 돌리지 않는다(fail-closed — `pw_core::config::mcp_policy` 머리 주석). 프로젝트를
    /// 찾지 못한 것(없는 id 등)은 여기서 가르지 않는다 — 툴 · 대상 노출 판정이 제 오류를 돌려준다.
    async fn target_config_unreadable(
        &self,
        project_id: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<Refusal>, Refusal> {
        let services = &self.inner.services;
        let project = match self.project(project_id, cancel).await {
            Ok(project) => project,
            Err(CallError::Cancelled) => return Err(Refusal::cancelled()),
            Err(_) => return Ok(None),
        };
        Ok(
            policy::of_project(&services.paths, &services.global, &project)
                .err()
                .map(|e| Refusal::denied(Reason::ProjectConfigUnreadable, e.message())),
        )
    }

    fn session_allows(&self, project_id: Option<&str>, key: &Allowance) -> bool {
        self.inner
            .allowed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(project_id.map(str::to_owned), key.clone()))
    }

    fn client(&self) -> Option<String> {
        self.inner
            .client_info
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// CLI(`pitwall-cli`)로 온 세션인가 — 클라이언트가 적은 이름으로만 안다(결정 M5-D5).
    fn is_cli(&self) -> bool {
        self.client()
            .is_some_and(|c| c.split(' ').next() == Some(CLI_CLIENT))
    }

    /// 정책 · 승인을 지난 호출을 돌린다: `write` 이상이거나 승인을 지난 호출(`approved` — 등급과 무관)이면 **선기록** — 실행
    /// 전에 감사 행을 `started` 로 쓰고([`Self::begin`] — 큐를 지나지 않고 기다린다) 못 쓰면 돌리지 않고 거절한다(결정 M5-D11).
    /// 그 뒤 Host RPC(승인을 지났으면 입력의 예약 키에 승인 표를 싣는다), 끝나면 같은 행을 결과 · 상세 · 크기 · 소요로 고친다
    /// (큐로). 승인 방식 · 계획 해시는 그 행에 남는다. 승인을 지나지 않은 `read` 는 끝난 뒤 새 행 하나다(큐로 — 실패는 경고만).
    #[allow(clippy::too_many_arguments)] // 호출 하나의 대상 · 승인 · 감사 열이다.
    async fn run(
        &self,
        tool: &Tool,
        args: Map<String, Value>,
        project_id: Option<String>,
        started: Instant,
        cancel: &CancellationToken,
        approved: Option<Approved>,
        remote: Option<&RemoteAim>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = tool.name.clone();
        // 선기록의 기준: `write` 이상이거나 승인을 지난 호출. 승인을 청하는 `read` 호출(「이 연결 열기」, 프로덕션 `ask` 의
        // 조회)도 승인을 지났으면 행을 먼저 쓰고 승인 칸을 그 행에 남긴다(메인 판정 2026-11-17 — 16-m5-plan M5-12 ⑧).
        let row = if tool.tier == Tier::Read && approved.is_none() {
            None
        } else {
            let (mode, hash) = approved.as_ref().map(|a| (a.mode, a.hash.clone())).unzip();
            match self
                .begin(&name, &args, project_id.clone(), mode, hash)
                .await
            {
                Ok(row) => Some(row),
                Err(e) => {
                    tracing::warn!(%e, tool = name, "cannot write the mcp audit row before the call; refusing it");
                    let reason = format!(
                        "`{name}` needs its audit log entry written before it runs, and Pitwall could not \
                         write it, so it did not run; check that the Pitwall state directory is writable"
                    );
                    self.audit(
                        None,
                        &name,
                        &args,
                        project_id,
                        Outcome::Denied,
                        Some(reason.clone()),
                        None,
                        started,
                    );
                    return Ok(failed(reason, Mark::denied(Reason::AuditUnavailable)));
                }
            }
        };
        let params = if tool.unit_params {
            Value::Null
        } else {
            let mut params = sent(&args);
            if let Some(approved) = &approved {
                params.insert(
                    APPROVAL_KEY.to_owned(),
                    serde_json::to_value(ApprovalTicket {
                        hash: approved.hash.clone(),
                    })
                    .expect("승인 표는 직렬화된다"),
                );
            }
            Value::Object(params)
        };
        // 클라이언트가 취소하거나 세션이 끝나면 Host RPC 를 거둔다 — `HostClient` 가 그 요청 id 로 `request.cancel` 을 보낸다.
        // 세션의 끝을 따로 보지 않는다: rmcp 의 요청 토큰(`cancel`)은 [`serve`] 가 `serve_with_ct` 에 넘긴 세션 토큰의 자손이라,
        // 세션 토큰이 취소되거나 전송이 닫혀 서비스 루프가 끝나면 함께 취소된다(rmcp 3.9 `serve_inner` 의 `serve_loop_ct`).
        //
        // 선기록한 호출은 취소 뒤에도 Host 의 답을 [`SETTLE_AFTER_CANCEL`] 까지 기다린다(16-m5-plan M5-18b): 「되돌릴 수 없는
        // 지점」(`pw_rpc::PointOfNoReturn` — `db_execute` 의 커밋)을 넘은 핸들러는 버려지지 않고 끝까지 돌아 답한다. 그 답으로 행을
        // 끝내 감사가 사실과 맞게 한다. 그 결과는 에이전트에게 가지 않을 수 있다 — rmcp 는 취소된 요청의 늦은 응답을 버린다(rmcp 3.9
        // `serve_inner` 의 `local_ct_pool`). 그래서 감사 행이 정본이다.
        let client = self.client_for(remote);
        let settled = if row.is_some() {
            client
                .call_until_settled(tool.method, params, cancel, SETTLE_AFTER_CANCEL)
                .await
        } else {
            Settled::Answered(client.call_until(tool.method, params, cancel).await)
        };
        // 표지 뒤의 꼬리 글은 Host 가 취소를 받은 뒤에 답한 호출에만 — 취소가 닿기 전에 끝난 호출(표지와 무관)은 다른 글이다.
        let after_cancel = match &settled {
            Settled::AnsweredAfterCancel(_) => Some(CANCELLED_AFTER_THE_POINT),
            Settled::FinishedBeforeCancel(_) => Some(CANCELLED_AFTER_IT_FINISHED),
            _ => None,
        };
        match settled {
            Settled::Answered(Ok(result)) => {
                let result = self.with_bundles(tool, result).await;
                Ok(self.finished(row, &name, &args, project_id, tool, result, started, None))
            }
            Settled::Answered(Err(e)) => {
                Ok(self.call_failed(row, &name, &args, project_id, e, started, None))
            }
            Settled::AnsweredAfterCancel(answer) | Settled::FinishedBeforeCancel(answer) => {
                tracing::warn!(
                    tool = name,
                    ok = answer.is_ok(),
                    note = after_cancel,
                    "the client cancelled a call that ran to the end"
                );
                Ok(match answer {
                    Ok(result) => self.finished(
                        row,
                        &name,
                        &args,
                        project_id,
                        tool,
                        result,
                        started,
                        after_cancel,
                    ),
                    Err(e) => {
                        self.call_failed(row, &name, &args, project_id, e, started, after_cancel)
                    }
                })
            }
            Settled::Cancelled => Ok(self.call_failed(
                row,
                &name,
                &args,
                project_id,
                CallError::Cancelled,
                started,
                None,
            )),
            Settled::Unsettled => {
                tracing::warn!(
                    tool = name,
                    "the client cancelled a call and the host did not answer in time; whether it took effect is unknown"
                );
                Ok(self.call_failed(
                    row,
                    &name,
                    &args,
                    project_id,
                    CallError::Cancelled,
                    started,
                    Some(CANCELLED_UNSETTLED),
                ))
            }
        }
    }

    /// Host RPC 가 성공한 호출의 감사 행과 결과(응답 상한을 본다). `note` 는 감사 상세 끝에 덧붙는다(취소 뒤의 답 —
    /// [`CANCELLED_AFTER_THE_POINT`]).
    #[allow(clippy::too_many_arguments)] // 감사 행의 열과 호출 하나의 결과다.
    fn finished(
        &self,
        row: Option<i64>,
        name: &str,
        args: &Map<String, Value>,
        project_id: Option<String>,
        tool: &Tool,
        result: Value,
        started: Instant,
        note: Option<&str>,
    ) -> CallToolResult {
        let size = result.to_string().len();
        let noted = audit_detail(&result).map(|d| with_detail(d.to_owned(), note));
        let detail = noted.as_deref().or(note);
        if size > MAX_RESPONSE_BYTES {
            let message = format!(
                "the response is {size} bytes, over the {} KiB limit for one tool response; \
                 narrow the request (a smaller `limit`, a path, a filter) and call again",
                MAX_RESPONSE_BYTES / 1024
            );
            self.audit(
                row,
                name,
                args,
                project_id,
                Outcome::Error,
                Some(with_detail(message.clone(), detail)),
                Some(size),
                started,
            );
            return failed(message, Mark::error(Some(Reason::TooLarge)));
        }
        self.audit(
            row,
            name,
            args,
            project_id,
            Outcome::Ok,
            detail.map(str::to_owned),
            Some(size),
            started,
        );
        let result = if tool.output_schema.is_some() && result.is_object() {
            CallToolResult::structured(result)
        } else {
            CallToolResult::success(vec![ContentBlock::text(result.to_string())])
        };
        marked(result, Mark::ok())
    }

    /// Host RPC 가 실패한 호출(취소 포함)의 감사 행과 결과. `note` 는 감사 상세 끝에 덧붙는다(취소 뒤의 답 · 시한까지 답이 없음 —
    /// [`CANCELLED_AFTER_THE_POINT`] · [`CANCELLED_UNSETTLED`]).
    #[allow(clippy::too_many_arguments)] // 감사 행의 열과 호출 하나의 결과다.
    fn call_failed(
        &self,
        row: Option<i64>,
        name: &str,
        args: &Map<String, Value>,
        project_id: Option<String>,
        error: CallError,
        started: Instant,
        note: Option<&str>,
    ) -> CallToolResult {
        let cancelled = match &error {
            CallError::Cancelled => true,
            // Host 가 그 RPC 를 거뒀다(Host 종료 등).
            CallError::Rpc { code, .. } => *code == i64::from(pw_rpc::code::CANCELLED),
            CallError::Closed => false,
        };
        // 툴의 관문이 거절했다(`PERMISSION_DENIED`) — 결과 `denied`, 까닭은 `data.reason`(`marker` 머리 주석).
        let refused = match &error {
            CallError::Rpc { code, data, .. }
                if *code == i64::from(pw_rpc::code::PERMISSION_DENIED) =>
            {
                Some(Reason::of_permission_denied(
                    data.as_ref()
                        .and_then(|d| d.get("reason"))
                        .and_then(Value::as_str),
                ))
            }
            // DB 툴의 표지 기억을 쓰거나 읽지 못해 돌리지 않았다(fail-closed) — 고장이 아니라 거절이다(감사 까닭으로 찾게).
            CallError::Rpc { code, data, .. }
                if *code == i64::from(pw_rpc::code::STORAGE_UNAVAILABLE) =>
            {
                Reason::of_storage_unavailable(
                    data.as_ref()
                        .and_then(|d| d.get("reason"))
                        .and_then(Value::as_str),
                )
            }
            _ => None,
        };
        // 승인 뒤 다시 만든 계획의 해시가 다르다(파일 · 워크트리 · 대상이 바뀜) — 다시 묻지 않고 거절한다. 「승인 필요」 오류와
        // 그 계획은 에이전트에게 그대로 보내지 않는다.
        let approval = match &error {
            CallError::Rpc { code, data, .. } if *code == i64::from(APPROVAL_REQUIRED) => Some(
                data.as_ref()
                    .and_then(|d| serde_json::from_value::<ApprovalRequired>(d.clone()).ok())
                    .is_some_and(|r| r.mismatch),
            ),
            _ => None,
        };
        if let Some(mismatch) = approval {
            let (message, mark) = if mismatch {
                (
                    format!(
                        "the plan of `{name}` changed after it was approved (a file, the worktree or the target \
                         changed), so it did not run; call again to ask for a new approval"
                    ),
                    Mark::denied(Reason::PlanChanged),
                )
            } else {
                (
                    format!("`{name}` needs approval in Pitwall for this call"),
                    Mark::denied(Reason::NeedsApproval),
                )
            };
            self.audit(
                row,
                name,
                args,
                project_id,
                mark.outcome,
                Some(message.clone()),
                None,
                started,
            );
            return failed(message, mark);
        }
        // 취소된 호출의 감사 행은 결과 `error` · 까닭 `cancelled` 다(결과 값을 늘리지 않는다).
        let (message, mark) = if cancelled {
            (
                Reason::Cancelled.as_str().to_owned(),
                Mark::error(Some(Reason::Cancelled)),
            )
        } else if let Some(reason) = refused {
            (error.to_string(), Mark::denied(reason))
        } else {
            (error.to_string(), Mark::error(None))
        };
        // 툴이 오류 `data` 에 감사 상세를 실었으면(「툴 메서드 규약」) 감사 행은 `data` 를 통째로 싣지 않고 오류 글 · 상세다 —
        // `data` 에는 서버의 오류 글(값이 섞인다)이 실릴 수 있다.
        let logged = match &error {
            CallError::Rpc {
                code,
                message: rpc_message,
                data,
            } => match data.as_ref().and_then(audit_detail) {
                Some(detail) if cancelled => with_detail(message.clone(), Some(detail)),
                Some(detail) => with_detail(format!("{rpc_message} ({code})"), Some(detail)),
                None => message.clone(),
            },
            _ => message.clone(),
        };
        self.audit(
            row,
            name,
            args,
            project_id,
            mark.outcome,
            Some(with_detail(logged, note)),
            None,
            started,
        );
        failed(message, mark)
    }

    /// 입력에 [`WORKTREE`] 가 있는 툴이 세션 프로젝트를 겨누고 그 값이 비었으면(없음 · `null` · 빈 글자열) 세션의 활성 워크트리
    /// 꼭대기로 채운다. 다른 프로젝트를 겨누면 채우지 않는다 — 세션의 워크트리는 세션 프로젝트의 것이다.
    fn fill_worktree(&self, tool: &Tool, args: &mut Map<String, Value>, project_id: Option<&str>) {
        let Some(project) = &self.inner.project else {
            return;
        };
        let Some(top) = &project.worktree else {
            return;
        };
        if project_id != Some(project.id.as_str()) || !takes_worktree(tool) {
            return;
        }
        let empty = match args.get(WORKTREE) {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            Some(_) => false,
        };
        if empty {
            args.insert(WORKTREE.into(), Value::String(top.clone()));
        }
    }

    /// 대상 프로젝트(세션 프로젝트가 아닌 `projectId`)의 노출 설정이 그 툴을 켰는가(결정 M5-D28). 세션의 노출은 이미 지났다 —
    /// 둘 다 켠 툴만 부른다. 그 프로젝트의 설정은 **디스크에 쓰지 않고** 읽는다(구버전 파일은 메모리에서만 올린다). 프로젝트를
    /// 찾지 못하면 그 오류를 호출의 오류로 돌려준다(없는 id 면 툴이 돌려줄 오류와 같다).
    async fn target_exposes(
        &self,
        tool: &Tool,
        project_id: &str,
        cancel: &CancellationToken,
    ) -> Result<bool, CallError> {
        let project = self.project(project_id, cancel).await?;
        let services = &self.inner.services;
        let exposure = exposure::of_project(&services.paths, &services.global, &project);
        Ok(exposure.enabled(tool))
    }

    /// 레지스트리의 그 프로젝트(`project.get`). 없는 id 면 툴이 돌려줄 오류와 같은 오류다.
    async fn project(
        &self,
        project_id: &str,
        cancel: &CancellationToken,
    ) -> Result<Project, CallError> {
        let params = serde_json::to_value(ProjectIdParams {
            id: project_id.to_owned(),
        })
        .expect("ProjectIdParams 는 직렬화된다");
        let value = self
            .inner
            .client
            .call_until("project.get", params, cancel)
            .await?;
        serde_json::from_value(value).map_err(|e| CallError::Rpc {
            code: i64::from(pw_rpc::code::INTERNAL_ERROR),
            message: format!("unexpected project.get result: {e}"),
            data: None,
        })
    }

    /// 끝난 호출의 감사 기록을 큐에 넣는다. `row` 가 있으면(선기록한 호출) 그 행을 고치고, 없으면 새 행이다.
    #[allow(clippy::too_many_arguments)] // 감사 로그 한 행의 열이다.
    fn audit(
        &self,
        row: Option<i64>,
        tool: &str,
        args: &Map<String, Value>,
        project_id: Option<String>,
        outcome: Outcome,
        detail: Option<String>,
        result_size: Option<usize>,
        started: Instant,
    ) {
        let entry = Entry {
            session: self.inner.id.clone(),
            client: self
                .inner
                .client_info
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            project_id,
            tool: tool.to_owned(),
            args: summarize(&Value::Object(args.clone())),
            outcome,
            detail,
            result_size,
            duration: started.elapsed(),
            // 선기록한 행이면 시작 때 쓴 값이 남는다(`AuditLog::finish` 는 비어 있는 값으로 덮지 않는다).
            approval: None,
            plan_hash: None,
        };
        let job = match row {
            Some(row) => AuditJob::Finish(row, entry),
            None => {
                // 세션 등록부의 호출 수 · 클라이언트(M5-10). 호출 하나가 감사 행 하나다 — 선기록한 호출은 시작 때 셌다.
                self.inner
                    .services
                    .sessions
                    .called(&entry.session, entry.client.as_deref());
                AuditJob::Write(entry)
            }
        };
        // 응답을 기다리게 하지 않는다 — 큐에 넣기만 한다. 기록 실패는 경고로만 남는다(audit 머리 주석).
        if self.inner.audit.send(job).is_err() {
            tracing::warn!(tool, "cannot queue the mcp audit log entry");
        }
    }

    /// 선기록(결정 M5-D11): 실행 전의 행을 `started` 로 쓰고 그 id 를 돌려준다. **큐를 지나지 않고 기다린다** — 쓰기 스레드에 넣고
    /// 응답을 기다리지 않는 다른 기록과 달리, 이것이 끝나야 툴이 돈다. 먼저 큐를 비운다 — 앞서 끝난 호출의 행이 이 행보다 앞
    /// id 를 받는다(행 순서가 호출 순서). 행이 쓰이면 호출 수를 세고 `mcp.activity` 를 낸다.
    async fn begin(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        project_id: Option<String>,
        approval: Option<AuditApproval>,
        plan_hash: Option<String>,
    ) -> Result<i64, String> {
        flush_audit(&self.inner.audit).await;
        let entry = Entry {
            session: self.inner.id.clone(),
            client: self
                .inner
                .client_info
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            project_id,
            tool: tool.to_owned(),
            args: summarize(&Value::Object(args.clone())),
            outcome: Outcome::Started,
            detail: None,
            result_size: None,
            duration: std::time::Duration::ZERO,
            approval,
            plan_hash,
        };
        let log = Arc::clone(&self.inner.services.audit);
        let (row, entry) = tokio::task::spawn_blocking(move || (log.begin(&entry), entry))
            .await
            .map_err(|e| format!("the audit write task failed: {e}"))?;
        let row = row?;
        let sessions = &self.inner.services.sessions;
        sessions.called(&entry.session, entry.client.as_deref());
        sessions.recorded(row, entry.project_id.as_deref());
        Ok(row)
    }
}

/// 툴이 보낸 계획이 그 툴의 것이 아닌 까닭(권고 1 — 16-m5-plan M5-12 리뷰): 다른 메서드의 계획이거나, 선언 등급보다 높은
/// 등급이거나(선언 등급은 상한 — 「등급은 그 툴이 도달할 수 있는 가장 높은 등급」, `pw-mcp` 머리 주석 「툴 메서드 규약」 · 17-m6-plan
/// 결정 M6-P9), 낮춤 목록(`catalog::LOWERED_PLAN_TIERS`)에 없는 낮은 등급이거나, `dangerous` 계획에 정책 자동을 단 것이다(`dangerous` 는 매번 묻는다 — 17-m6-plan M6-10 ③). 맞으면 `None`.
fn plan_problem(tool: &Tool, plan: &ToolPlan) -> Option<String> {
    if plan.method != tool.method {
        return Some(format!(
            "the plan is for `{}`, not `{}`",
            plan.method, tool.method
        ));
    }
    if plan.tier > tool.tier {
        return Some(format!(
            "the plan tier `{}` is above the declared tier `{}`",
            policy::tier_name(plan.tier),
            policy::tier_name(tool.tier)
        ));
    }
    // 낮춤은 목록에 적힌 툴 · 등급만(`catalog::LOWERED_PLAN_TIERS`) — 낮춘 계획에는 `auto_approve` · 세션 허용이 더 낮은 등급으로
    // 닿으므로, 툴이 말없이 낮추면 승인이 넓어진다.
    if plan.tier < tool.tier && !tool.lowered.contains(&plan.tier) {
        return Some(format!(
            "the plan tier `{}` is below the declared tier `{}`, and this tool may not lower it",
            policy::tier_name(plan.tier),
            policy::tier_name(tool.tier)
        ));
    }
    (plan.tier == Tier::Dangerous && plan.rules.policy_auto)
        .then(|| "a `dangerous` plan cannot skip the question by policy".to_owned())
}

/// 툴이 결과(또는 오류 `data`) 최상위에 실은 감사 상세 글(`audit` — 「툴 메서드 규약」). 감사 행의 상세 칸으로 간다(길이는
/// [`crate::audit::MAX_DETAIL`] 에서 잘린다).
fn audit_detail(value: &Value) -> Option<&str> {
    value.get(AUDIT).and_then(Value::as_str)
}

/// `message · detail`(상세가 없으면 `message`).
fn with_detail(message: String, detail: Option<&str>) -> String {
    match detail {
        Some(detail) => format!("{message} · {detail}"),
        None => message,
    }
}

impl ServerHandler for Session {
    fn get_info(&self) -> ServerConfig {
        let project = self
            .inner
            .project
            .as_ref()
            .map(|p| {
                format!(
                    " The session project is `{}`; project tools default to it.",
                    p.id
                )
            })
            .unwrap_or_default();
        let headless = if self.headless() {
            format!(" {HEADLESS_NOTICE}")
        } else {
            String::new()
        };
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new("pitwall", env!("CARGO_PKG_VERSION")))
        .with_instructions(format!(
            "Pitwall IDE: project registry and settings that only the IDE knows.{project}{headless}"
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self
                .visible()
                .await
                .into_iter()
                .map(|t| self.to_mcp(t))
                .collect(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // 2026-07-28 은 요청마다 `_meta` 로, 그 전 버전은 initialize 로 온다. `client_info` 가 둘 다 본다.
        if let Some(info) = context.client_info() {
            *self
                .inner
                .client_info
                .lock()
                .unwrap_or_else(PoisonError::into_inner) =
                Some(format!("{} {}", info.name, info.version));
        }
        self.call(request, context.ct.clone()).await.map(Into::into)
    }

    /// 2025-11-25 까지의 클라이언트: initialize 뒤 세션 내내 `tools/list_changed` 를 보낸다.
    fn on_initialized(
        &self,
        context: NotificationContext<RoleServer>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        let peer = context.peer.clone();
        let session = self.clone();
        async move {
            tokio::spawn(async move {
                let cancel = session.inner.cancel.clone();
                let mut changes = session.changes();
                loop {
                    tokio::select! {
                        () = cancel.cancelled() => return,
                        changed = changes.next() => if !changed { return },
                    }
                    if peer.notify_tool_list_changed().await.is_err() {
                        return;
                    }
                }
            });
        }
    }

    /// 2026-07-28 클라이언트: 알림은 `subscriptions/listen` 으로 연 구독으로만 간다. 툴 목록 변경만 받는다.
    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.supported_by(&self.get_info().capabilities))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let cancel = self.inner.cancel.clone();
        if context.accepted().tools_list_changed != Some(true) {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = context.cancelled() => {}
            }
            return Ok(());
        }
        let mut changes = self.changes();
        loop {
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                () = context.cancelled() => return Ok(()),
                changed = changes.next() => if !changed { return Ok(()) },
            }
            if context.sink().notify_tool_list_changed().await.is_err() {
                return Ok(());
            }
        }
    }
}

/// 보이는 툴 집합이 바뀔 때마다 깨어난다. 설정이 바뀌어도 보이는 툴이 같으면 깨지 않는다.
struct Changes {
    exposure: watch::Receiver<Arc<Exposure>>,
    catalog: Arc<Catalog>,
    shown: BTreeSet<String>,
}

impl Changes {
    /// 다음 변경까지 기다린다. 설정 감시가 끝났으면 `false`.
    async fn next(&mut self) -> bool {
        loop {
            if self.exposure.changed().await.is_err() {
                return false;
            }
            let now = self.exposure.borrow_and_update().visible(&self.catalog);
            if now != self.shown {
                self.shown = now;
                return true;
            }
        }
    }
}

impl Session {
    fn changes(&self) -> Changes {
        let mut exposure = self.inner.exposure.clone();
        let shown = exposure
            .borrow_and_update()
            .visible(&self.inner.services.catalog);
        Changes {
            exposure,
            catalog: Arc::clone(&self.inner.services.catalog),
            shown,
        }
    }
}

async fn resolve(client: &HostClient, path: PathBuf) -> Option<Resolution> {
    let params = serde_json::to_value(PathParams {
        path: path.display().to_string(),
    })
    .expect("PathParams 는 직렬화된다");
    match client.call("project.resolve", params).await {
        Ok(Value::Null) => {
            tracing::info!(path = %path.display(), "no registered project contains the mcp session path");
            None
        }
        Ok(value) => match serde_json::from_value::<Resolution>(value) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(%e, "unexpected project.resolve result");
                None
            }
        },
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "cannot resolve the mcp session project");
            None
        }
    }
}

/// 세션 프로젝트의 설정. **디스크에 쓰지 않고** 읽는다 — 세션을 열었다고 커밋된 `.pitwall/project.toml` 이 마이그레이션되거나
/// 저장소에 `.backup/` 이 생기면 안 된다(조회만 하는 에이전트가 저장소를 고친다). 이벤트는 세션 전용 버스로 낸다 — 앱의
/// 버스로 내면 앱이 연 같은 프로젝트의 설정 이벤트(`config.changed` · `config.error`)가 세션마다 한 번 더 나간다.
struct ProjectConfig {
    store: ConfigStore,
    bus: EventBus,
    /// 저장소 모드면 공유 레이어를 뺀 읽기. 커밋된 설정은 노출을 좁히기만 한다(`docs/design/05-mcp.md` 「툴 노출 설정」 절).
    /// 개인 전용 프로젝트의 `project.toml` 은 개인 설정이라 없다.
    without_shared: Option<ConfigSource>,
}

impl ProjectConfig {
    fn open(paths: &AppPaths, r: &Resolution, cancel: &CancellationToken) -> Option<Self> {
        let root = r
            .worktree
            .as_ref()
            .map_or_else(|| PathBuf::from(&r.root), |w| PathBuf::from(&w.root));
        let source = match r.project.config_source(paths, &root) {
            Ok(source) => source.without_writes(),
            Err(e) => {
                tracing::warn!(%e, "cannot read the project config for the mcp session");
                return None;
            }
        };
        let without_shared = (r.project.config_mode == ConfigMode::Repository)
            .then(|| source.clone().without_shared());
        let bus = EventBus::new(16);
        let store = ConfigStore::open(source, bus.clone(), Scope::Project(r.project.id.clone()));
        if let Err(e) = store.watch(
            &tokio::runtime::Handle::current(),
            pw_core::config::DEFAULT_DEBOUNCE,
            cancel.child_token(),
        ) {
            tracing::warn!(%e, "project config watch disabled for the mcp session");
        }
        Some(Self {
            store,
            bus,
            without_shared,
        })
    }
}

/// [`exposure_feed`] 의 결과: 노출의 수신자와, 지금 다시 읽어 넣는 함수.
struct ExposureFeed {
    receiver: watch::Receiver<Arc<Exposure>>,
    refresh: Arc<dyn Fn() + Send + Sync>,
}

/// 전역 · 프로젝트 설정의 `[mcp]` 를 읽고, `config.changed`(전역은 앱 버스, 프로젝트는 세션 버스)가 오면 다시 읽는다. 이벤트 없이
/// 바뀌는 것(git 인덱스)은 [`ExposureFeed::refresh`] 를 부르는 쪽(목록 · 호출)이 다시 읽는다.
fn exposure_feed(
    global: ConfigStore,
    bus: &EventBus,
    project: Option<ProjectConfig>,
    cancel: CancellationToken,
) -> ExposureFeed {
    let mut global_events = bus.subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_CHANGED]));
    let mut project_events = project
        .as_ref()
        .map(|p| p.bus.subscribe(Filter::all().topics([TOPIC_CHANGED])));
    let read = move || {
        let g = global.snapshot();
        let p = project.as_ref().map(|p| p.store.snapshot());
        Arc::new(exposure::layered(
            g.get("settings", exposure::KEY),
            p.as_ref().and_then(|p| p.get("project", exposure::KEY)),
            project.as_ref().and_then(|p| p.without_shared.as_ref()),
        ))
    };
    let (tx, rx) = watch::channel(read());
    // 값이 같으면 수신자를 깨우지 않는다.
    let refresh = Arc::new(move || {
        let next = read();
        tx.send_if_modified(|current| {
            if **current == *next {
                false
            } else {
                *current = next;
                true
            }
        });
    });
    let refresh_on_event = Arc::clone(&refresh);
    tokio::spawn(async move {
        loop {
            let project_changed = async {
                match project_events.as_mut() {
                    Some(events) => events.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = cancel.cancelled() => return,
                received = global_events.recv() => if received.is_none() { return },
                received = project_changed => if received.is_none() { return },
            }
            // 설정 · 인덱스 읽기는 블로킹 스레드에서(`Session::visible` 과 같다).
            let refresh = Arc::clone(&refresh_on_event);
            let _ = tokio::task::spawn_blocking(move || refresh()).await;
        }
    });
    ExposureFeed {
        receiver: rx,
        refresh,
    }
}

/// Host 로 보낼 입력: 세션의 입력(`checkout`)을 뗀다 — Host 는 `checkout` 을 모른다(D57 ④).
fn sent(args: &Map<String, Value>) -> Map<String, Value> {
    let mut sent = args.clone();
    sent.remove(CHECKOUT);
    sent
}

/// 다른 머신 호출의 세션 허용 열쇠: 머신을 열쇠에 넣어 이 머신 · 다른 머신의 같은 열쇠가 서로 닿지 않게 한다.
fn machine_key(tool: &str, key: Option<&str>, tier: Tier, machine: &str) -> Allowance {
    Allowance {
        tool: tool.to_owned(),
        key: Some(format!("@{machine}:{}", key.unwrap_or_default())),
        tier,
    }
}

/// Desktop 의 프로젝트 이름의 묶음. 모르는 이름(다른 머신의 프로젝트 id 등)은 거절한다.
fn bundle_of(
    machines: &dyn Machines,
    project_id: &str,
) -> Result<pw_core::registry::bundle::ProjectBundle, Refusal> {
    match machines.bundle(project_id) {
        Ok(Some(bundle)) => Ok(bundle),
        Ok(None) => Err(Refusal {
            message: format!(
                "`{project_id}` is not a project or bundle id of this Pitwall; another machine's project ids are not \
                 accepted (project_list lists the ids)"
            ),
            mark: Mark::error(None),
        }),
        Err(e) => Err(Refusal {
            message: format!("cannot read the projects of this Pitwall: {e}"),
            mark: Mark::error(None),
        }),
    }
}

/// 입력 스키마에 [`WORKTREE`] 가 있는가.
fn takes_worktree(tool: &Tool) -> bool {
    tool.input_schema_for(false)
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|p| p.contains_key(WORKTREE))
}

fn marked(mut result: CallToolResult, mark: Mark) -> CallToolResult {
    result.meta = Some(MetaObject(mark.to_map()));
    result
}

/// `isError` 결과(거절 · 툴 오류)와 그 표지.
fn failed(message: String, mark: Mark) -> CallToolResult {
    marked(
        CallToolResult::error(vec![ContentBlock::text(message)]),
        mark,
    )
}

/// 감사 로그에서 한 세션의 호출을 묶는 id: pid · 시작 시각(밀리초) · 프로세스 안 순번.
fn session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!(
        "{}-{millis}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// 선기록(16-m5-plan M5-11 ② — 결정 M5-D11). 정책 · 승인 뒤의 실행 단계([`Session::run`])를 가짜 쓰기 툴로 직접 부른다 —
/// 승인 경로(M5-12)가 답을 받은 뒤 부르는 자리다. 가짜 툴은 승인 표를 보지 않고 돈다(승인 경로의 시험은 `tests/approval.rs`).
/// 세션 · Host · 감사 파일은 진짜다.
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use pw_core::LocalHost;
    use pw_rpc::{RpcError, rpc_method};

    use super::*;

    #[derive(
        Debug,
        Clone,
        Default,
        PartialEq,
        Eq,
        serde::Serialize,
        serde::Deserialize,
        specta::Type,
        schemars::JsonSchema,
    )]
    #[serde(rename_all = "camelCase")]
    pub struct FakeWriteParams {
        #[serde(default)]
        pub tag: String,
    }

    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        serde::Serialize,
        serde::Deserialize,
        specta::Type,
        schemars::JsonSchema,
    )]
    pub struct FakeWriteResult {
        pub audit: String,
    }

    rpc_method! {
        /// 시험용 쓰기 툴: 불린 수를 세고 `tag` 를 감사 상세로 싣는다.
        pub FakeWrite: "mcptest.fakeWrite" {
            params: pw_core::registry::plan::Planned<FakeWriteParams>,
            result: FakeWriteResult,
            tier: Write,
            mcp: { group: "project", name: "fake_write" },
        }

        /// 시험용 조회 툴: 불린 수를 세고(쓰기 툴과 같은 수) `tag` 를 감사 상세로 싣는다. 승인을 청하는 `read` 호출(「이 연결 열기」
        /// 등)의 자리다.
        pub FakeRead: "mcptest.fakeRead" {
            params: FakeWriteParams,
            result: FakeWriteResult,
            tier: Read,
            mcp: { group: "project", name: "fake_read" },
        }

        /// 시험용 쓰기 툴: 불리면 알리고 끝나지 않는다.
        pub HangingWrite: "mcptest.hangingWrite" {
            params: pw_core::registry::plan::Planned<FakeWriteParams>,
            result: FakeWriteResult,
            tier: Write,
            mcp: { group: "project", name: "hanging_write" },
        }
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        paths: AppPaths,
        host: Arc<LocalHost>,
        runs: Arc<AtomicUsize>,
        entered: Arc<tokio::sync::Notify>,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let paths = AppPaths::under(tmp.path());
            let runs = Arc::new(AtomicUsize::new(0));
            let entered = Arc::new(tokio::sync::Notify::new());
            let (r, e) = (Arc::clone(&runs), Arc::clone(&entered));
            let host = Arc::new(
                LocalHost::builder("local")
                    .config(paths.clone())
                    .methods(move |router| {
                        let read_runs = Arc::clone(&r);
                        router.unary(FakeRead, move |_ctx, p: FakeWriteParams| {
                            read_runs.fetch_add(1, Ordering::SeqCst);
                            async move {
                                Ok::<_, RpcError>(FakeWriteResult {
                                    audit: format!("tag={}", p.tag),
                                })
                            }
                        });
                        router.unary(
                            FakeWrite,
                            move |_ctx, p: pw_core::registry::plan::Planned<FakeWriteParams>| {
                                r.fetch_add(1, Ordering::SeqCst);
                                async move {
                                    Ok::<_, RpcError>(FakeWriteResult {
                                        audit: format!("tag={}", p.params.tag),
                                    })
                                }
                            },
                        );
                        router.unary(
                            HangingWrite,
                            move |_ctx, _p: pw_core::registry::plan::Planned<FakeWriteParams>| {
                                let e = Arc::clone(&e);
                                async move {
                                    e.notify_one();
                                    std::future::pending::<Result<FakeWriteResult, RpcError>>()
                                        .await
                                }
                            },
                        );
                    })
                    .build(),
            );
            Self {
                _tmp: tmp,
                paths,
                host,
                runs,
                entered,
            }
        }

        /// 세션 하나. 감사 로그는 세션마다 따로 연다 — 프로세스 둘이 같은 파일에 쓰는 것과 같다(연결이 다르다).
        async fn session(&self) -> Session {
            self.session_with(Sessions::new()).await
        }

        /// [`Self::session`] 에 세션 등록부를 넣은 것(호출 수 · `mcp.activity` 를 보려고).
        async fn session_with(&self, sessions: Sessions) -> Session {
            let router = Arc::new(HostRouter::new());
            router.add_host(self.host.clone()).unwrap();
            let services = Arc::new(Services {
                router,
                host_id: "local".into(),
                paths: self.paths.clone(),
                global: self.host.config().unwrap().global.clone(),
                bus: self.host.bus().clone(),
                audit: Arc::new(AuditLog::under_state(&self.paths.state)),
                catalog: Arc::new(Catalog::linked().routed_by(self.host.router())),
                approval: Approval::Unavailable,
                transport: "test",
                host_kind: HostKind::App,
                sessions,
                machines: None,
            });
            Session::open(
                services,
                &SessionRequest::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap()
        }

        fn rows(&self) -> Vec<(String, String, String, Option<String>, i64)> {
            let conn = rusqlite::Connection::open(self.paths.state.join("mcp/audit.db")).unwrap();
            conn.prepare(
                "SELECT session, tool, outcome, detail, duration_ms FROM calls ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        }
    }

    /// 정책을 지난 쓰기 호출 하나 — 승인 경로(M5-12)가 답을 받은 뒤 부르는 자리.
    async fn write(session: &Session, name: &str, tag: &str) -> CallToolResult {
        approved(session, name, tag, None).await
    }

    /// 정책 · 승인을 지난 호출 하나 — 승인 방식 · 계획 해시를 함께 넘긴다(M5-12 가 승인 뒤 넘기는 값).
    async fn approved(
        session: &Session,
        name: &str,
        tag: &str,
        approval: Option<(AuditApproval, &str)>,
    ) -> CallToolResult {
        let tool = session
            .inner
            .services
            .catalog
            .tools()
            .iter()
            .find(|t| t.name == name)
            .unwrap()
            .clone();
        let mut args = Map::new();
        args.insert("tag".into(), Value::from(tag));
        session
            .run(
                &tool,
                args,
                None,
                Instant::now(),
                &CancellationToken::new(),
                approval.map(|(mode, hash)| Approved {
                    mode,
                    hash: hash.to_owned(),
                }),
                None,
            )
            .await
            .unwrap()
    }

    fn mark(result: &CallToolResult) -> Mark {
        Mark::read(&result.meta.as_ref().unwrap().0).unwrap()
    }

    /// 실행 전에 쓴 행을 끝난 뒤 같은 행으로 고친다 — 쓰기 호출 하나가 행 하나다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_write_call_is_recorded_before_it_runs_and_finished_in_the_same_row() {
        let f = Fixture::new();
        let session = f.session().await;
        let result = write(&session, "fake_write", "a").await;
        assert_eq!(mark(&result), Mark::ok(), "{result:?}");
        assert_eq!(f.runs.load(Ordering::SeqCst), 1);
        flush_audit(&session.inner.audit).await;
        let rows = f.rows();
        assert_eq!(rows.len(), 1, "선기록과 끝 기록이 한 행이다: {rows:?}");
        let (row_session, tool, outcome, detail, _) = &rows[0];
        assert_eq!(
            (
                row_session.as_str(),
                tool.as_str(),
                outcome.as_str(),
                detail.as_deref()
            ),
            (session.inner.id.as_str(), "fake_write", "ok", Some("tag=a"))
        );
        // 다음 호출도 행 하나씩이다.
        write(&session, "fake_write", "b").await;
        flush_audit(&session.inner.audit).await;
        assert_eq!(f.rows().len(), 2);
    }

    /// 감사 DB 를 쓸 수 없으면 쓰기 툴은 돌지 않고 거절된다(`audit_unavailable`). 조회 툴은 돈다 — 기록 실패는 경고만이다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_write_call_does_not_run_without_its_audit_row_and_a_read_call_still_does() {
        let f = Fixture::new();
        // 감사 폴더 자리에 파일을 두어 열지 못하게 한다.
        std::fs::create_dir_all(&f.paths.state).unwrap();
        std::fs::write(f.paths.state.join(crate::audit::DIR), "not a directory").unwrap();
        let session = f.session().await;
        let result = write(&session, "fake_write", "a").await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(mark(&result), Mark::denied(Reason::AuditUnavailable));
        assert_eq!(f.runs.load(Ordering::SeqCst), 0, "툴이 돌았다");

        let read = session
            .call(
                CallToolRequestParams::new("project_list"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(mark(&read), Mark::ok(), "{read:?}");
    }

    /// 끝을 쓰기 전에 호출이 사라지면(프로세스가 죽음) 그 행은 `started` 로 남는다 — 화면의 「끝을 모름」.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_call_whose_end_is_never_written_stays_started() {
        let f = Fixture::new();
        let session = f.session().await;
        let running = {
            let session = session.clone();
            tokio::spawn(async move { write(&session, "hanging_write", "x").await })
        };
        tokio::time::timeout(Duration::from_secs(10), f.entered.notified())
            .await
            .expect("the tool did not run");
        // 툴이 도는 동안 행은 이미 있다 — 실행 전에 썼다.
        let rows = f.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].2, "started");
        running.abort();
        let _ = running.await;
        flush_audit(&session.inner.audit).await;
        let rows = f.rows();
        assert_eq!(rows.len(), 1);
        let (_, tool, outcome, detail, duration) = &rows[0];
        assert_eq!(
            (
                tool.as_str(),
                outcome.as_str(),
                detail.as_deref(),
                *duration
            ),
            ("hanging_write", "started", None, 0)
        );
        let (listed, _) = AuditLog::under_state(&f.paths.state)
            .list(&crate::audit::Query {
                limit: 10,
                outcome: Some(Outcome::Started),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(listed.len(), 1);
    }

    /// 두 연결(프로세스 둘과 같다 — 감사 로그 연결이 따로다)이 동시에 쓰기 호출을 해도 끝 기록은 자기 행만 고친다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_sessions_writing_at_once_do_not_mix_their_rows() {
        const EACH: usize = 20;
        let f = Fixture::new();
        let (a, b) = (f.session().await, f.session().await);
        let mut tasks = Vec::new();
        for i in 0..EACH {
            for (session, prefix) in [(&a, "a"), (&b, "b")] {
                let session = session.clone();
                tasks.push(tokio::spawn(async move {
                    write(&session, "fake_write", &format!("{prefix}{i}")).await
                }));
            }
        }
        for task in tasks {
            assert_eq!(mark(&task.await.unwrap()), Mark::ok());
        }
        flush_audit(&a.inner.audit).await;
        flush_audit(&b.inner.audit).await;
        let conn = rusqlite::Connection::open(f.paths.state.join("mcp/audit.db")).unwrap();
        let rows: Vec<(String, String, String, Option<String>)> = conn
            .prepare("SELECT session, args, outcome, detail FROM calls")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.len(), EACH * 2);
        for (row_session, args, outcome, detail) in &rows {
            let tag = serde_json::from_str::<Value>(args).unwrap()["tag"]
                .as_str()
                .unwrap()
                .to_owned();
            let owner = if tag.starts_with('a') { &a } else { &b };
            assert_eq!(outcome, "ok", "{tag}");
            assert_eq!(detail.as_deref(), Some(format!("tag={tag}").as_str()));
            assert_eq!(row_session, &owner.inner.id, "{tag}");
        }
    }

    fn approval_columns(f: &Fixture) -> Vec<(String, String, Option<String>, Option<String>)> {
        let conn = rusqlite::Connection::open(f.paths.state.join("mcp/audit.db")).unwrap();
        conn.prepare("SELECT tool, outcome, approval, plan_hash FROM calls ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// 승인을 지난 `read` 호출(「이 연결 열기」 · 프로덕션 `ask` 의 조회 — 메인 판정 2026-11-17, 16-m5-plan M5-12 ⑧)도 선기록되고
    /// 승인 방식 · 계획 해시가 그 행에 남는다. 감사 행을 먼저 쓰지 못하면 돌지 않는다. 승인 없는 `read` 는 그대로(행 하나, 칸 없음).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_approved_read_call_is_recorded_first_and_keeps_its_approval() {
        let f = Fixture::new();
        let session = f.session().await;
        let result = approved(
            &session,
            "fake_read",
            "r",
            Some((AuditApproval::Once, "sha256:plan")),
        )
        .await;
        assert_eq!(mark(&result), Mark::ok(), "{result:?}");
        write(&session, "fake_read", "plain").await;
        flush_audit(&session.inner.audit).await;
        assert_eq!(
            approval_columns(&f),
            [
                (
                    "fake_read".to_owned(),
                    "ok".to_owned(),
                    Some("once".to_owned()),
                    Some("sha256:plan".to_owned())
                ),
                ("fake_read".to_owned(), "ok".to_owned(), None, None),
            ]
        );
        assert_eq!(f.runs.load(Ordering::SeqCst), 2);

        // 감사 DB 를 쓸 수 없으면 승인을 지난 `read` 도 돌지 않는다 — 승인 없는 `read` 는 돈다.
        let broken = Fixture::new();
        std::fs::create_dir_all(&broken.paths.state).unwrap();
        std::fs::write(
            broken.paths.state.join(crate::audit::DIR),
            "not a directory",
        )
        .unwrap();
        let session = broken.session().await;
        let refused = approved(
            &session,
            "fake_read",
            "r",
            Some((AuditApproval::Session, "sha256:plan")),
        )
        .await;
        assert_eq!(mark(&refused), Mark::denied(Reason::AuditUnavailable));
        assert_eq!(broken.runs.load(Ordering::SeqCst), 0, "툴이 돌았다");
        assert_eq!(
            mark(&write(&session, "fake_read", "plain").await),
            Mark::ok()
        );
        assert_eq!(broken.runs.load(Ordering::SeqCst), 1);
    }

    /// 선기록한 호출은 호출 수를 한 번 세고(시작 때), `mcp.activity` 를 같은 id 로 두 번(시작 행 · 끝 고침) 낸다. 선기록하지 않은
    /// `read` 는 한 번 세고 한 번 낸다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pre_recorded_call_counts_once_and_publishes_its_row_twice() {
        use pw_core::{Filter, Received};
        let f = Fixture::new();
        let sessions = Sessions::new();
        sessions.attach(f.host.bus().clone());
        let mut events = f
            .host
            .bus()
            .subscribe(Filter::all().topics([crate::sessions::TOPIC_ACTIVITY]));
        let session = f.session_with(sessions.clone()).await;
        let _registered = sessions.register(&session.inner.id, None, None, "test");

        write(&session, "fake_write", "w").await;
        flush_audit(&session.inner.audit).await;
        assert_eq!(sessions.list()[0].calls, 1, "쓰기 호출 하나는 한 번 센다");
        let ids = |events: &mut pw_core::Subscription| -> Vec<i64> {
            std::iter::from_fn(|| events.try_recv())
                .map(|r| match r {
                    Received::Event(e) => e.payload["id"].as_i64().unwrap(),
                    Received::Lagged(n) => panic!("lagged {n}"),
                })
                .collect()
        };
        let written: i64 = rusqlite::Connection::open(f.paths.state.join("mcp/audit.db"))
            .unwrap()
            .query_row("SELECT id FROM calls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ids(&mut events), [written, written], "시작과 끝, 같은 id");

        write(&session, "fake_read", "r").await;
        flush_audit(&session.inner.audit).await;
        assert_eq!(sessions.list()[0].calls, 2);
        assert_eq!(ids(&mut events).len(), 1, "선기록 없는 read 는 한 번");
    }
}
