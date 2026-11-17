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
//! 4. 등급 정책([`crate::policy`]).
//! 5. Host RPC. MCP 의 취소 알림이 오거나 세션이 끝나면 그 RPC 를 거둔다(`request.cancel` — [`crate::client`]).
//! 6. 응답 상한 [`MAX_RESPONSE_BYTES`] — 넘으면 「범위를 좁혀라」 오류(결정 M5-D34).
//!
//! 결과는 모두 `_meta` 에 결과 표지([`crate::marker`])를 단다.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use pw_core::config::{AppPaths, ConfigSource, ConfigStore, TOPIC_CHANGED};
use pw_core::registry::methods::{PathParams, ProjectIdParams};
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
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::audit::{AuditApproval, AuditLog, Entry, Outcome, summarize};
use crate::catalog::{Catalog, PROJECT_ID, Tool};
use crate::client::{CallError, HostClient};
use crate::exposure::{self, Exposure};
use crate::marker::{Mark, Reason};
use crate::policy::{self, Approval, Decision};
use crate::sessions::Sessions;

/// 툴 응답 한 건의 상한: 직렬화한 결과 JSON 의 바이트 수(결정 M5-D34 — 출발값이다). 툴 메서드는 이것을 넘지 않게 자르고
/// (`truncated` 와 다음 `cursor`), 그래도 넘은 응답은 세션이 「범위를 좁혀라」 오류로 바꾼다.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;

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

/// 헤드리스 세션의 서버 안내문(결정 M5-D3). 앱 세션에는 싣지 않는다. 넘겨 붙기(16-m5-plan M5-20)가 들어오면 그 행이 바꾼다.
pub const HEADLESS_NOTICE: &str = "The Pitwall app is not running, so only the core tools are available. \
Start the Pitwall app, then restart this MCP server to get the rest.";

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
    let result = running.waiting().await;
    tracing::debug!(?result, "mcp session ended");
    flush_audit(&audit).await;
    Ok(())
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
    id: String,
    /// 세션이 끝나면 취소된다([`serve`] 가 취소한다). 설정 감시 · 버스 구독 · list_changed 태스크가 여기에 묶인다.
    cancel: CancellationToken,
    client_info: Mutex<Option<String>>,
    /// 감사 기록 큐([`audit_writer`]). 세션의 호출 순서대로 쓰인다.
    audit: std::sync::mpsc::Sender<AuditJob>,
}

/// 감사 쓰기 스레드가 받는 일.
enum AuditJob {
    /// 끝난 호출의 새 행(`read` · 거절 · 선기록 없이 끝난 호출).
    Write(Entry),
    /// 선기록([`Session::begin`])한 행을 같은 id 로 고친다.
    Finish(i64, Entry),
    /// 앞서 받은 기록을 다 썼으면 알린다([`flush_audit`]).
    Flush(tokio::sync::oneshot::Sender<()>),
}

/// 지금까지 `audit` 에 넣은 기록이 다 쓰일 때까지 기다린다(큐는 차례대로 처리된다). 쓰기 스레드가 없으면 곧바로 돌아온다.
async fn flush_audit(audit: &std::sync::mpsc::Sender<AuditJob>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if audit.send(AuditJob::Flush(tx)).is_ok() {
        let _ = rx.await;
    }
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
        let exposure = exposure_feed(
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
                id,
                cancel,
                client_info: Mutex::new(None),
                audit,
            }),
        })
    }

    /// 지금 보이는 툴.
    fn visible(&self) -> Vec<&Tool> {
        let exposure = Arc::clone(&self.inner.exposure.borrow());
        self.inner
            .services
            .catalog
            .tools()
            .iter()
            .filter(|t| exposure.enabled(t))
            .collect()
    }

    fn to_mcp(&self, tool: &Tool) -> rmcp::model::Tool {
        let schema = tool.input_schema_for(self.inner.project.is_some());
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
        let tool = self.visible().into_iter().find(|t| t.name == name).cloned();
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
        if tool.project_scoped()
            && !args.contains_key(PROJECT_ID)
            && let Some(project) = &self.inner.project
        {
            args.insert(PROJECT_ID.into(), Value::String(project.id.clone()));
        }
        let project_id = args
            .get(PROJECT_ID)
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.fill_worktree(&tool, &mut args, project_id.as_deref());
        if tool.project_scoped()
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
                Err(e) => return Ok(self.call_failed(None, &name, &args, project_id, e, started)),
            }
        }
        let denied = match policy::decide(tool.tier, self.inner.services.approval) {
            Decision::Allow => None,
            // 승인 화면이 아직 없다. `Approval::Available` 을 넣는 쪽이 생기면 여기서 승인을 기다린다.
            Decision::Ask => Some((
                "approval is required and is not available yet".to_owned(),
                Reason::NeedsApproval,
            )),
            Decision::Deny(reason) => Some((
                reason,
                if self.headless() {
                    Reason::HeadlessWrite
                } else {
                    Reason::NeedsApproval
                },
            )),
        };
        if let Some((reason, code)) = denied {
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
        // 승인 경로(M5-12)는 위의 `Ask` 자리에서 답을 받은 뒤 승인 방식 · 계획 해시와 함께 이 단계로 온다.
        self.run(&tool, args, project_id, started, &cancel, None, None)
            .await
    }

    /// 정책을 지난 호출을 돌린다: `write` 이상이면 **선기록** — 실행 전에 감사 행을 `started` 로 쓰고([`Self::begin`] — 큐를
    /// 지나지 않고 기다린다) 못 쓰면 돌리지 않고 거절한다(결정 M5-D11). 그 뒤 Host RPC, 끝나면 같은 행을 결과 · 상세 · 크기 ·
    /// 소요로 고친다(큐로). `read` 는 끝난 뒤 새 행 하나다(큐로 — 실패는 경고만). 승인을 지난 호출은 `approval` · `plan_hash` 를
    /// 그 행에 남긴다.
    #[allow(clippy::too_many_arguments)] // 감사 행의 열과 호출 하나의 입력이다.
    async fn run(
        &self,
        tool: &Tool,
        args: Map<String, Value>,
        project_id: Option<String>,
        started: Instant,
        cancel: &CancellationToken,
        approval: Option<AuditApproval>,
        plan_hash: Option<String>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = tool.name.clone();
        let row = if tool.tier == Tier::Read {
            None
        } else {
            match self
                .begin(&name, &args, project_id.clone(), approval, plan_hash)
                .await
            {
                Ok(row) => Some(row),
                Err(e) => {
                    tracing::warn!(%e, tool = name, "cannot write the mcp audit row before a write call; refusing it");
                    let reason = format!(
                        "`{name}` changes something, and Pitwall could not write its audit log entry first, \
                         so it did not run; check that the Pitwall state directory is writable"
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
            Value::Object(args.clone())
        };
        // 클라이언트가 취소하거나 세션이 끝나면 Host RPC 를 거둔다 — `HostClient` 가 그 요청 id 로 `request.cancel` 을 보낸다.
        // 세션의 끝을 따로 보지 않는다: rmcp 의 요청 토큰(`cancel`)은 [`serve`] 가 `serve_with_ct` 에 넘긴 세션 토큰의 자손이라,
        // 세션 토큰이 취소되거나 전송이 닫혀 서비스 루프가 끝나면 함께 취소된다(rmcp 3.9 `serve_inner` 의 `serve_loop_ct`).
        match self
            .inner
            .client
            .call_until(tool.method, params, cancel)
            .await
        {
            Ok(result) => {
                let size = result.to_string().len();
                let detail = audit_detail(&result);
                if size > MAX_RESPONSE_BYTES {
                    let message = format!(
                        "the response is {size} bytes, over the {} KiB limit for one tool response; \
                         narrow the request (a smaller `limit`, a path, a filter) and call again",
                        MAX_RESPONSE_BYTES / 1024
                    );
                    self.audit(
                        row,
                        &name,
                        &args,
                        project_id,
                        Outcome::Error,
                        Some(with_detail(message.clone(), detail)),
                        Some(size),
                        started,
                    );
                    return Ok(failed(message, Mark::error(Some(Reason::TooLarge))));
                }
                self.audit(
                    row,
                    &name,
                    &args,
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
                Ok(marked(result, Mark::ok()))
            }
            Err(e) => Ok(self.call_failed(row, &name, &args, project_id, e, started)),
        }
    }

    /// Host RPC 가 실패한 호출(취소 포함)의 감사 행과 결과.
    fn call_failed(
        &self,
        row: Option<i64>,
        name: &str,
        args: &Map<String, Value>,
        project_id: Option<String>,
        error: CallError,
        started: Instant,
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
            _ => None,
        };
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
            Some(logged),
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
        let params = serde_json::to_value(ProjectIdParams {
            id: project_id.to_owned(),
        })
        .expect("ProjectIdParams 는 직렬화된다");
        let value = self
            .inner
            .client
            .call_until("project.get", params, cancel)
            .await?;
        let project: Project = serde_json::from_value(value).map_err(|e| CallError::Rpc {
            code: i64::from(pw_rpc::code::INTERNAL_ERROR),
            message: format!("unexpected project.get result: {e}"),
            data: None,
        })?;
        let services = &self.inner.services;
        let exposure = exposure::of_project(&services.paths, &services.global, &project);
        Ok(exposure.enabled(tool))
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
            tools: self.visible().into_iter().map(|t| self.to_mcp(t)).collect(),
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

/// 전역 · 프로젝트 설정의 `[mcp]` 를 읽고, `config.changed`(전역은 앱 버스, 프로젝트는 세션 버스)가 오면 다시 읽는다.
fn exposure_feed(
    global: ConfigStore,
    bus: &EventBus,
    project: Option<ProjectConfig>,
    cancel: CancellationToken,
) -> watch::Receiver<Arc<Exposure>> {
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
            // 값이 같으면 수신자를 깨우지 않는다.
            let next = read();
            tx.send_if_modified(|current| {
                if **current == *next {
                    false
                } else {
                    *current = next;
                    true
                }
            });
        }
    });
    rx
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

/// 선기록(16-m5-plan M5-11 ② — 결정 M5-D11). 지금 `write` 이상은 정책이 모두 거절하므로(승인 경로는 M5-12) 정책 뒤의 실행
/// 단계([`Session::run`])를 가짜 쓰기 툴로 직접 부른다 — M5-12 가 승인을 받은 뒤 부르는 자리다. 세션 · Host · 감사 파일은 진짜다.
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
            params: FakeWriteParams,
            result: FakeWriteResult,
            tier: Write,
            mcp: { group: "project", name: "fake_write" },
        }

        /// 시험용 쓰기 툴: 불리면 알리고 끝나지 않는다.
        pub HangingWrite: "mcptest.hangingWrite" {
            params: FakeWriteParams,
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
                        router.unary(FakeWrite, move |_ctx, p: FakeWriteParams| {
                            r.fetch_add(1, Ordering::SeqCst);
                            async move {
                                Ok::<_, RpcError>(FakeWriteResult {
                                    audit: format!("tag={}", p.tag),
                                })
                            }
                        });
                        router.unary(HangingWrite, move |_ctx, _p: FakeWriteParams| {
                            let e = Arc::clone(&e);
                            async move {
                                e.notify_one();
                                std::future::pending::<Result<FakeWriteResult, RpcError>>().await
                            }
                        });
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
                sessions: Sessions::new(),
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
                None,
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
}
