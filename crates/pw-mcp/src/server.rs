//! MCP 세션 하나: rmcp 서버 핸들러. 툴 호출을 Host 의 RPC 로 옮긴다.
//!
//! 세션은 Host 에 **owner** role 로 붙는다 — 에이전트는 이 사용자의 로컬 프로세스다(로컬 소켓 · stdio 만 받는다). 무엇을
//! 부를 수 있는지는 role 이 아니라 이 계층이 정한다: 노출([`crate::exposure`]) → 등급 정책([`crate::policy`]).
//! 호출 하나하나는 결과와 상관없이 감사 로그([`crate::audit`])에 남는다.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use pw_core::config::{AppPaths, ConfigSource, ConfigStore, TOPIC_CHANGED};
use pw_core::registry::methods::PathParams;
use pw_core::registry::{ConfigMode, Resolution};
use pw_core::{EventBus, Filter, HostRouter, Scope, Target};
use pw_rpc::{Caller, Role};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, SubscriptionFilter,
};
use rmcp::service::{MaybeSendFuture, NotificationContext, RequestContext, SubscriptionContext};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::audit::{AuditLog, Entry, Outcome, summarize};
use crate::catalog::{Catalog, PROJECT_ID, Tool};
use crate::client::HostClient;
use crate::exposure::{self, Exposure};
use crate::policy::{self, Approval, Decision};

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
    /// 로그에 남길 연결 종류(`socket` · `stdio`).
    pub transport: &'static str,
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

/// 세션 하나를 끝까지 돌린다. 클라이언트가 끊거나 `cancel` 이 취소되면 끝난다.
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
    let running = session
        .serve_with_ct(io, session_cancel.clone())
        .await
        .map_err(|e| ServeError::Initialize(e.to_string()))?;
    let result = running.waiting().await;
    tracing::debug!(?result, "mcp session ended");
    Ok(())
}

/// 세션의 프로젝트.
#[derive(Debug, Clone)]
struct SessionProject {
    id: String,
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
                Caller::new(Role::Owner),
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
        let project = resolved.map(|r| SessionProject { id: r.project.id });
        let id = session_id();
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

    async fn call(&self, request: CallToolRequestParams) -> Result<CallToolResult, ErrorData> {
        let started = Instant::now();
        let name = request.name.to_string();
        let mut args = request.arguments.unwrap_or_default();
        let tool = self.visible().into_iter().find(|t| t.name == name).cloned();
        let Some(tool) = tool else {
            self.audit(
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
                None,
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
        let denied = match policy::decide(tool.tier, self.inner.services.approval) {
            Decision::Allow => None,
            // 승인 화면이 아직 없다. `Approval::Available` 을 넣는 쪽이 생기면 여기서 승인을 기다린다.
            Decision::Ask => Some("approval is required and is not available yet".to_owned()),
            Decision::Deny(reason) => Some(reason),
        };
        if let Some(reason) = denied {
            self.audit(
                &name,
                &args,
                project_id,
                Outcome::Denied,
                Some(reason.clone()),
                None,
                started,
            );
            return Ok(CallToolResult::error(vec![ContentBlock::text(reason)]));
        }
        let params = if tool.unit_params {
            Value::Null
        } else {
            Value::Object(args.clone())
        };
        match self.inner.client.call(tool.method, params).await {
            Ok(result) => {
                let size = result.to_string().len();
                self.audit(
                    &name,
                    &args,
                    project_id,
                    Outcome::Ok,
                    None,
                    Some(size),
                    started,
                );
                Ok(if tool.output_schema.is_some() && result.is_object() {
                    CallToolResult::structured(result)
                } else {
                    CallToolResult::success(vec![ContentBlock::text(result.to_string())])
                })
            }
            Err(e) => {
                let message = e.to_string();
                self.audit(
                    &name,
                    &args,
                    project_id,
                    Outcome::Error,
                    Some(message.clone()),
                    None,
                    started,
                );
                Ok(CallToolResult::error(vec![ContentBlock::text(message)]))
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // 감사 로그 한 행의 열이다.
    fn audit(
        &self,
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
        };
        let log = Arc::clone(&self.inner.services.audit);
        // 응답을 기다리게 하지 않는다. 기록 실패는 경고로만 남는다(audit 머리 주석).
        tokio::task::spawn_blocking(move || {
            if let Err(e) = log.record(&entry) {
                tracing::warn!(%e, tool = entry.tool, "cannot write the mcp audit log");
            }
        });
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
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new("pitwall", env!("CARGO_PKG_VERSION")))
        .with_instructions(format!(
            "Pitwall IDE: project registry and settings that only the IDE knows.{project}"
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
        self.call(request).await.map(Into::into)
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
        let global_mcp = g.get("settings", exposure::KEY);
        let p = project.as_ref().map(|p| p.store.snapshot());
        let (mut exposure, mut warnings) = Exposure::from_layers(
            global_mcp,
            p.as_ref().and_then(|p| p.get("project", exposure::KEY)),
        );
        if let Some(source) = project.as_ref().and_then(|p| p.without_shared.as_ref()) {
            // 공유 레이어를 뺀 값을 다시 읽는다. 읽지 못하면(로컬 파일이 깨짐) 전역만을 상한으로 둔다 — 넓어지지 않는다.
            let local = source.load().ok();
            let (ceiling, _) = Exposure::from_layers(
                global_mcp,
                local.as_ref().and_then(|s| s.get("project", exposure::KEY)),
            );
            exposure = exposure.narrowed_by(ceiling);
        }
        for w in warnings.drain(..) {
            tracing::warn!("{w}");
        }
        Arc::new(exposure)
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
