//! 서버 관리자(15-m4-plan.md M4-05, 결정 M4-D8 · D10 · D11 · D12): 인스턴스 · 문서 동기화 · 수명 · 요청 길.
//!
//! **인스턴스** — (프로젝트, 워크트리, 서버 id, 작업 공간 루트)마다 하나. 루트는 `crate::root`(root_markers 가 맞는 가장 위 폴더).
//! 그 언어의 문서를 처음 열 때 띄운다(지연 기동). 한 언어에 서버가 여럿이면 모두 띄우고 모두에 동기화한다(결정 M4-D12).
//!
//! **띄우기**(`start`) — 매번 [`lsp_grant`] 를 다시 지난다(연 연결의 범위 · 닫힘 토큰으로 — 물러남 재기동 · `lsp.server.restart`
//! 도 같다). 미신뢰면 `untrusted` 로 두고 띄우지 않는다(결정 M4-D8). 해석 · 합친 설정은 `pw_lsp_servers::LspServers::setup`,
//! 서버 런타임은 `crate::requires`(결정 M4-D6 — 찾으면 `ServerSetup::with_runtime`, 못 찾으면 `runtimeMissing`). 이 둘은 로그인 셸
//! · 실행 파일을 기다리므로 블로킹 스레드에서 구한다. 서버 프로세스의 fork 는 `crate::process` 의 전용 스레드가 한다 — Linux 의
//! `PR_SET_PDEATHSIG` 는 fork 한 스레드에 묶이고 런타임의 스레드는 끝날 수 있다(M4-01 재리뷰 N3 · M4-05 리뷰 M4). 띄운 서버는
//! `initialize` 를 기다리는 동안에도 인스턴스가 쥔다 — 그사이 신뢰 해제 · 창 닫힘 · restart · stop 이 곧바로 끝낸다(리뷰 H1).
//! 이벤트 채널은 서버마다 따로 만든다(N2 — 나누면 채널을 채운 서버가 아니라 그때 알림을 낸 서버가 끝난다).
//!
//! **수명**:
//! - 인스턴스의 마지막 문서가 닫히면 유휴 시한(`[lsp] idle_shutdown_minutes`, 기본 10분) 뒤 끝낸다. 그사이 다시 열면 그대로 쓴다.
//! - **연결(창)이 닫히면** 그 연결이 연 문서를 모두 닫고, 연 연결이 하나도 남지 않은 인스턴스는 곧바로 끝낸다(유휴 시한 없이).
//!   문서를 열거나 `lsp.server.restart` · `stop` 을 부른 연결은 모두 그 닫힘을 지켜본다(리뷰 M1).
//!   같은 창의 새 연결(새로고침)이 이어받지 않는다 — 새 연결이 문서를 다시 열면 다시 띄운다. 그래서 창을 닫으면 그 창이 띄운
//!   서버가 2 s(`LanguageServer::shutdown` 의 상한) 안에 모두 끝난다.
//! - 서버가 스스로 죽으면 지수 물러남(기본 1 s 부터 두 배, 30 s 까지)으로 다시 띄우되 3분 안에 5번이면 `crashed` 로 멈춘다.
//!   다시 띄울 때도 허가를 다시 받는다 — 그 인스턴스를 연 연결 중 열린 것으로.
//! - 신뢰를 풀면(`project.changed` 의 `trust`) 그 신뢰를 따르는 프로젝트의 서버를 곧바로 트리째 끝내고 `untrusted` 로 둔다.
//! - 설정이 바뀌면(`config.changed`) 합친 설정이 달라진 인스턴스를 `restartNeeded`, 런타임 환경이 바뀌면(`runtime.changed`)
//!   `restartSuggested` 로 둔다 — 저절로 다시 띄우지 않는다.
//!
//! **문서 동기화**(결정 M4-D10) — Host 가 문서마다 텍스트(`crate::document`)를 쥐고, 떠 있는 서버에 `didOpen` · `didChange`(서버가
//! `Incremental` 이면 증분, 아니면 전체) · `didSave` · `didClose` 를 보낸다. 보내기는 잠금 안에서 곧바로 송신 큐에 넣는다 — 그래서
//! 요청 길의 요청이 그보다 앞선 편집을 앞지르지 않는다. 편집 등급 밖 문서는 열지 않는다(`off`). 같은 파일을 여러 연결(창)이
//! 열어도 서버에는 한 번 열린다(`Synced` — 리뷰 M2).
//!
//! **요청 길** — [`LspManager::request`] 하나. 기능 행은 서버에 직접 닿지 않는다(15 「공유 부품의 주인」).
//!
//! **파일 감시 전달**(M4-05b) — 떠 있는 서버마다 자리(`crate::watched::Feed`)를 두고 `fs.changed` 와 파일 목록 인덱스의 다시 만든
//! 목록을 그 서버가 등록한 글롭대로 보낸다. 규칙과 상한은 `crate::watched` 머리 주석.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use lsp_types::{
    Position, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncSaveOptions,
};
use pw_core::bus::{EventBus, Received, Scope, Subscription};
use pw_core::config::ConfigStore;
use pw_core::registry::path::worktree_root;
use pw_core::registry::{ChangeKind, ProjectChanged, Registry};
use pw_core::task::TaskSpawner;
use pw_core::{Filter, registry};
use pw_ext::LspFeature;
use pw_lsp_servers::{LspResolution, LspResolveStep, LspServers, ServerConfig};
use pw_rpc::{ConnectionScope, Ctx, RpcError, SessionId, code as rpc_code};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::document::{DocText, LspTextChange, OverLimit};
use crate::grant::{LspGrant, lsp_grant, real_path};
use crate::handlers::{HandlerContext, server_handlers};
use crate::ledger::{Ledger, ledger_dir};
use crate::log::{ServerLog, lsp_log_dir};
use crate::message::code as lsp_code;
use crate::methods::{
    LspDocumentChangeParams, LspDocumentChangeResult, LspDocumentOff, LspDocumentOpenParams,
    LspDocumentOpened, LspInstanceState, LspInstanceStatus, LspServerControlParams, LspStatus,
    TOPIC_STATUS,
};
use crate::process::LaunchSpec;
use crate::progress::Tracker;
use crate::requires::{find_runtime, runtime_name};
use crate::server::{
    ExitReason, InitializeOptions, LanguageServer, RequestError, RequestOptions, ServerEvent,
    ServerOptions,
};
use crate::uri::file_uri;
use crate::watched::{Event as WatchEvent, Feed, FeedHost, FeedObserver, Gate, WatchLimits};

/// 서버 관리자의 애플리케이션 오류 코드(`pw-lsp` 5001~ — 15 「공유 부품의 주인」 의 오류 코드 칸). 기능 행은 이 범위에 더한다.
pub mod code {
    /// 그 문서의 서버 중 이 기능을 맡을 떠 있는 서버가 없다(꺼짐 · 능력 없음 · 아직 뜨는 중 · 미신뢰 등). `data`:
    /// `{ feature, instances: [LspInstanceStatus] }`.
    pub const NO_SERVER: i32 = 5001;
    /// 서버가 오류로 답했다(`ContentModified` · `ServerCancelled` 는 한 번 다시 물은 뒤). `data`: `{ server, code, message }`.
    pub const SERVER_ERROR: i32 = 5002;
    /// 서버가 시한 안에 답하지 않았거나 끝났다. `data`: `{ server, reason }`.
    pub const SERVER_UNAVAILABLE: i32 = 5003;
}

/// 문서 하나가 먼저 받아 둘 수 있는 앞당겨 온 편집 수(`syntax.edit` 와 같은 규칙 — 넘으면 conflict).
const MAX_EARLY_EDITS: usize = 64;

/// 관리자의 시간 · 크기 값. 시험이 줄인다.
#[derive(Debug, Clone)]
pub struct ManagerOptions {
    /// 마지막 문서를 닫은 뒤 끝내기까지. `None` 이면 설정 `[lsp] idle_shutdown_minutes`(기본 10분).
    pub idle_shutdown: Option<Duration>,
    /// 이 시간 안에 [`Self::crash_limit`] 번 죽으면 `crashed`.
    pub crash_window: Duration,
    pub crash_limit: usize,
    /// 첫 물러남. 죽을 때마다 두 배, [`Self::backoff_max`] 까지.
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    /// 서버마다의 이벤트 채널 크기. 받는 쪽은 곧바로 비운다 — 차면 `$/progress` 는 버리고 그 밖이면 그 서버가 끝난다.
    pub event_capacity: usize,
    /// 서버 로그에 프로토콜(보낸 · 받은 메시지)도 적는가.
    pub trace: bool,
    /// 파일 감시 전달의 상한(M4-05b).
    pub watch: WatchLimits,
}

impl Default for ManagerOptions {
    fn default() -> Self {
        Self {
            idle_shutdown: None,
            crash_window: Duration::from_secs(180),
            crash_limit: 5,
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            event_capacity: 4096,
            trace: false,
            watch: WatchLimits::default(),
        }
    }
}

/// 기본 유휴 시한(분).
pub const DEFAULT_IDLE_SHUTDOWN_MINUTES: u64 = 10;

/// Host 를 만든 뒤에 붙이는 것.
#[derive(Clone)]
pub struct HostParts {
    pub projects: Registry,
    pub bus: EventBus,
    /// 진행(`$/progress`)을 작업으로 띄운다.
    pub tasks: TaskSpawner,
    /// 전역 설정(`[lsp] idle_shutdown_minutes`).
    pub global: ConfigStore,
    /// `$STATE` — 서버 로그(`logs/lsp/`)와 강제 종료 뒤 거두기의 기록부(`lsp/running/`).
    pub state_dir: PathBuf,
    /// 파일 목록 인덱스 — 다시 만든 목록으로 감시 글롭에 맞는 파일을 다시 알린다(M4-05b). 없으면 증분(`fs.changed`)만 보낸다.
    pub files: Option<pw_fs::index::FileIndex>,
}

struct Host {
    parts: HostParts,
    ledger: Arc<Ledger>,
    log_dir: PathBuf,
}

/// 인스턴스의 열쇠. 같음 · 차례는 (프로젝트, 서버, 루트)만 본다 — 루트는 실제 경로라 워크트리를 정하고, `worktree` 는 창이 준 글
/// 그대로라 같은 워크트리를 `/x/wt` · `/x/wt/` 처럼 달리 적을 수 있다(그래도 인스턴스는 하나 — M4-05 리뷰 L5). `worktree` 는 처음
/// 연 요청의 것을 쥐고 허가 · 설정을 구할 때 넘긴다.
#[derive(Debug, Clone)]
pub struct InstanceKey {
    pub project_id: String,
    pub worktree: Option<String>,
    pub server: String,
    /// 작업 공간 루트(실제 경로).
    pub root: PathBuf,
}

impl InstanceKey {
    fn identity(&self) -> (&str, &str, &Path) {
        (&self.project_id, &self.server, &self.root)
    }
}

impl PartialEq for InstanceKey {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity()
    }
}

impl Eq for InstanceKey {}

impl std::hash::Hash for InstanceKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.identity().hash(state);
    }
}

impl PartialOrd for InstanceKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InstanceKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.identity().cmp(&other.identity())
    }
}

/// 인스턴스의 세대 표지(M4-08 리뷰 M1 — [`LspManager::instance_grant`]). 인스턴스를 다시 띄우거나 끝내면(`generation`),
/// 표에서 빠졌다 다시 생기면(`born`), 그 프로젝트의 신뢰가 바뀌거나 미신뢰가 관찰되면(`trust`) 달라진다. 루트 밖 읽기 토큰은
/// 발급 때의 표지에 묶여, 표지가 달라지면 다시 살아나지 않는다(신뢰를 다시 걸거나 서버를 다시 띄워도).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceEpoch {
    born: u64,
    generation: u64,
    trust: u64,
}

/// 연결이 문서를 연 인스턴스 하나([`LspManager::session_instances`]).
#[derive(Debug, Clone)]
pub struct SessionInstance {
    pub key: InstanceKey,
    /// 목록을 만들 때의 세대 표지 — [`LspManager::request_instance`] 의 `expected` 로 넘긴다.
    pub epoch: InstanceEpoch,
    /// 그 연결이 그 인스턴스에 연 문서 하나의 언어 — 언어 확장의 Linguist 이름(웹뷰의 문서 언어와 같은 꼴, 루트 밖 위치를 연 외부
    /// 소스 탭의 언어). 확장에 없는 언어면 LSP 의 `languageId`.
    pub language: String,
}

/// `window/showDocument` 을 보일 창([`LspManager::show_document_window`]).
#[derive(Debug, Clone)]
pub(crate) struct ShowWindow {
    /// 그 연결을 연 창의 label. 모르면(창이 아닌 연결) `None`.
    pub window: Option<String>,
}

/// 관리자를 약하게 가리킨다(인스턴스의 처리기가 쥔다 — [`LspManager::downgrade`]).
#[derive(Clone)]
pub struct WeakLspManager(std::sync::Weak<Inner>);

impl WeakLspManager {
    pub fn upgrade(&self) -> Option<LspManager> {
        self.0.upgrade().map(|inner| LspManager { inner })
    }
}

/// 알림을 받는 쪽이 아는 출처.
#[derive(Debug, Clone)]
pub struct NotificationSource {
    pub key: InstanceKey,
    /// 그 서버를 띄운 시도의 세대. 서버를 내려놓으면(끝 · 재기동 · 신뢰 해제) 인스턴스의 세대가 올라 이 출처는 지난 것이 된다 —
    /// 끝난 서버가 늦게 보낸 알림을 받는 쪽이 [`LspManager::is_current`] 로 가른다(M4-06).
    pub generation: u64,
}

type Listener = Arc<dyn Fn(&NotificationSource, &Value) + Send + Sync>;
/// 상태가 바뀌어 `lsp.status` 를 낼 때(프로젝트 id). 잠금 밖에서 불린다.
type StatusListener = Arc<dyn Fn(&str) + Send + Sync>;
/// 서버에 문서 텍스트(`didOpen` · `didChange`)를 보냈을 때(출처, URI, 서버 쪽 버전). **관리자 잠금 안에서** 불린다.
type SyncListener = Arc<dyn Fn(&NotificationSource, &str, i64) + Send + Sync>;
/// 인스턴스 하나의 [`SyncListener`] 들을 그 출처로 부르는 것.
type SyncHook = Arc<dyn Fn(&str, i64) + Send + Sync>;

/// 문서를 연 연결.
#[derive(Clone)]
struct Holder {
    scope: ConnectionScope,
    closed: CancellationToken,
    /// 그 연결을 연 창의 label([`Ctx::window`]) — `window/showDocument` 를 보일 창(M4-08 리뷰 M2).
    window: Option<String>,
    /// 이 창에 보낸 `window/showDocument` 의 시각(빈도 상한 — [`LspManager::show_document_window`]).
    shown: VecDeque<Instant>,
}

impl Holder {
    fn of(ctx: &Ctx) -> Self {
        Self {
            scope: ctx.scope().clone(),
            closed: ctx.connection_closed(),
            window: ctx.window().map(str::to_owned),
            shown: VecDeque::new(),
        }
    }
}

/// 연결을 문서 · 서버를 쥔 쪽으로 적는다(`open` · `lsp.server.restart` · `lsp.server.stop` — M4-05 리뷰 M1). 처음 적은 연결이면 그
/// 닫힘 토큰을 돌려준다 — 부른 쪽이 잠금 밖에서 `LspManager::watch_close` 로 지켜본다(연결마다 한 번).
fn hold(sessions: &mut HashMap<SessionId, Holder>, ctx: &Ctx) -> Option<CancellationToken> {
    match sessions.entry(ctx.session()) {
        std::collections::hash_map::Entry::Occupied(_) => None,
        std::collections::hash_map::Entry::Vacant(slot) => {
            Some(slot.insert(Holder::of(ctx)).closed.clone())
        }
    }
}

struct Doc {
    session: SessionId,
    project_id: String,
    uri: String,
    language_id: String,
    version: u32,
    text: Arc<DocText>,
    /// 정의 순서(결정 M4-D12).
    instances: Vec<InstanceKey>,
    early: BTreeMap<u32, Vec<LspTextChange>>,
    diverged: bool,
}

/// 서버가 원하는 동기화.
#[derive(Debug, Clone, Copy)]
struct SyncKind {
    change: TextDocumentSyncKind,
    /// `didSave` 를 보내는가 · 텍스트를 넣는가.
    save: Option<bool>,
}

/// 서버에 열어 둔 URI 하나(M4-05 리뷰 M2). 같은 파일을 여러 연결(창)이 열어도 서버에는 `didOpen` 한 번이고, 그 문서들을 `refs` 로
/// 세어 마지막 문서가 닫힐 때만 `didClose` 한다. 서버의 텍스트는 `owner` 문서의 것이다 — 다른 문서가 편집 · 요청 · 저장하면 그
/// 문서가 주인이 되고(텍스트가 다르면 전체 텍스트 `didChange`), 버전은 문서와 상관없이 한 줄로 오른다.
struct Synced {
    owner: String,
    refs: BTreeSet<String>,
    /// 서버에 보낸 마지막 버전.
    version: i64,
    /// 텍스트를 보낸 뒤 부를 것(`LspManager::on_synced` — 인스턴스의 것).
    hook: Option<SyncHook>,
}

impl Synced {
    /// 다음 버전: 문서의 버전(`wanted`)을 따르되 늘 오른다 — 연결이 하나면 웹뷰의 버전 그대로다.
    fn next_version(&mut self, wanted: u32) -> i64 {
        self.version = (self.version + 1).max(i64::from(wanted));
        self.version
    }

    /// 서버에 지금 버전의 텍스트를 보냈다고 알린다(잠금 안 — 받는 쪽은 태스크에 넣기만).
    fn sent(&self, uri: &str) {
        if let Some(hook) = &self.hook {
            hook(uri, self.version);
        }
    }
}

/// 인스턴스에서 내려놓은 서버. 끝내는 것은 내려놓은 쪽이다.
struct Taken {
    server: Arc<LanguageServer>,
    /// `initialize` 의 답을 받았는가. 아니면(기동 중) 기다리지 않고 트리째 끝낸다 — LSP 는 `initialize` 의 답 전에 `shutdown` 을
    /// 보내지 못하고, 답을 미루는 서버(JVM · 고장 · 악의)가 시한(10 s)을 버는 길이 된다(M4-05 리뷰 H1).
    initialized: bool,
}

impl Taken {
    /// 끝낸다: 떠 있던 서버는 `shutdown`(상한 2 s), 기동 중이던 서버는 곧바로 트리째.
    async fn end(self) {
        if self.initialized {
            self.server.shutdown().await;
        } else {
            self.server.kill();
        }
    }
}

struct Instance {
    /// 정의 순서(요청을 맡을 서버를 고른다).
    order: usize,
    state: LspInstanceState,
    /// 띄운 서버. `initialize` 를 기다리는 동안(`starting`)에도 여기 있다 — 신뢰 해제 · 창 닫힘 · restart · stop 이 그 서버에 닿는다
    /// (M4-05 리뷰 H1). 동기화 · 요청은 `state.is_running()` 일 때만 간다.
    server: Option<Arc<LanguageServer>>,
    /// 띄우기 · 끝내기마다 오른다 — 지난 시도의 결과를 버린다. 관리자 전체의 단조 증가 수([`Instance::bump`])에서 받는다: 창을
    /// 닫으면 인스턴스가 지워지므로 인스턴스마다 0 부터 세면 같은 키로 다시 연 인스턴스가 끝나는 옛 서버와 같은 세대가 된다(M4-06
    /// 리뷰 M1 — 옛 서버의 늦은 알림 · 답이 새 서버의 것으로 받아들여진다).
    generation: u64,
    /// 이 인스턴스 값을 만든 차례(관리자 전체에서 하나씩 오른다). 세대 표지([`InstanceEpoch`])는 이것과 함께 본다(M4-08). `generation`
    /// 도 이제 관리자 전체에서 오르므로(M4-06 리뷰 M1) 다시 생긴 인스턴스와 겹치지 않는다 — 표지는 그대로 둘을 함께 본다.
    born: u64,
    /// 가장 최근에 이 서버를 쓴 연결(문서 열기 · 요청 — `window/showDocument` 를 보일 창, M4-08 리뷰 M2).
    last_user: Option<SessionId>,
    generations: Arc<AtomicU64>,
    docs: BTreeSet<String>,
    /// 지금 서버에 열어 둔 URI(→ 그 URI 를 연 문서들).
    synced: BTreeMap<String, Synced>,
    holders: BTreeSet<SessionId>,
    crashes: VecDeque<Instant>,
    idle: Option<(u64, CancellationToken)>,
    config: Option<Arc<ServerConfig>>,
    /// 실행 파일을 Pitwall 설치본(`$DATA/tools`)에서 찾았으면 그 Mason 패키지(도구 제거 전에 멈출 서버를 고른다).
    installed: Option<String>,
    sync: SyncKind,
    progress: Tracker,
    /// 프로젝트(활성 워크트리면 그 워크트리) 루트의 실제 경로 — 감시 전달은 이 루트의 `fs.changed` 만 받는다.
    scope: PathBuf,
    /// 떠 있는 서버로 가는 파일 감시 전달의 자리(`ready` 때 만들고 서버를 내려놓을 때 닫는다).
    feed: Option<Arc<Feed>>,
    /// 이 세대의 [`SyncHook`](떠서 `ready` 가 될 때 단다).
    sync_hook: Option<SyncHook>,
}

impl Instance {
    fn new(order: usize, scope: PathBuf, generations: Arc<AtomicU64>) -> Self {
        Self {
            order,
            scope,
            feed: None,
            state: LspInstanceState::Starting,
            server: None,
            born: {
                static BORN: AtomicU64 = AtomicU64::new(0);
                BORN.fetch_add(1, Ordering::Relaxed)
            },
            last_user: None,
            generation: generations.fetch_add(1, Ordering::Relaxed) + 1,
            generations,
            docs: BTreeSet::new(),
            synced: BTreeMap::new(),
            holders: BTreeSet::new(),
            crashes: VecDeque::new(),
            idle: None,
            config: None,
            installed: None,
            sync: SyncKind {
                change: TextDocumentSyncKind::FULL,
                save: None,
            },
            progress: Tracker::default(),
            sync_hook: None,
        }
    }

    /// 세대를 올린다 — 관리자 전체에서 한 번도 쓰지 않은 수로.
    fn bump(&mut self) {
        self.generation = self.generations.fetch_add(1, Ordering::Relaxed) + 1;
    }

    /// 서버를 내려놓는다(끝내는 것은 부른 쪽 — [`Taken::end`]).
    fn take_server(&mut self, state: LspInstanceState) -> Option<Taken> {
        let initialized = self.state.is_running();
        self.bump();
        self.synced.clear();
        self.progress.end_all();
        self.sync_hook = None;
        self.state = state;
        self.idle = None;
        if let Some(feed) = self.feed.take() {
            feed.close();
        }
        self.server.take().map(|server| Taken {
            server,
            initialized,
        })
    }

    /// 문서 `doc_id` 가 지금 서버에 동기화돼 있는가(그 URI 를 연 문서 중 하나인가).
    fn is_synced(&self, doc_id: &str, uri: &str) -> bool {
        self.synced
            .get(uri)
            .is_some_and(|s| s.refs.contains(doc_id))
    }
}

#[derive(Default)]
struct State {
    docs: HashMap<String, Doc>,
    instances: BTreeMap<InstanceKey, Instance>,
    sessions: HashMap<SessionId, Holder>,
    next_doc: u64,
    next_idle: u64,
    /// 프로젝트(인스턴스의 `project_id`)마다 신뢰 세대 — 신뢰가 바뀌었다는 알림을 받거나 미신뢰를 본 때마다 오른다(M4-08 리뷰 M1).
    trust_epochs: HashMap<String, u64>,
}

struct Inner {
    servers: Arc<LspServers>,
    extensions: Arc<pw_ext::Registry>,
    options: ManagerOptions,
    host: OnceLock<Host>,
    state: Mutex<State>,
    spawns: AtomicU64,
    /// 인스턴스 세대의 원천(관리자 수명 안에서 단조 증가 — [`Instance::generation`]).
    generations: Arc<AtomicU64>,
    listeners: RwLock<HashMap<String, Vec<Listener>>>,
    status_listeners: RwLock<Vec<StatusListener>>,
    sync_listeners: RwLock<Vec<SyncListener>>,
}

/// [`LspManager::subscribe`] 의 구독 둘(제어 · 파일 바뀜). [`LspManager::follow`] 에 넘긴다.
pub struct ManagerEvents {
    control: Subscription,
    files: Subscription,
}

/// 서버 관리자. 복제는 같은 관리자를 가리킨다.
#[derive(Clone)]
pub struct LspManager {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

fn not_found(doc_id: &str) -> RpcError {
    RpcError::new(rpc_code::NOT_FOUND, format!("no open document `{doc_id}`"))
}

fn conflict(message: String, version: u32) -> RpcError {
    RpcError::new(rpc_code::CONFLICT, message).with_data(json!({ "version": version }))
}

/// LSP 의 `languageId`(확장의 언어 id 와 다른 것만).
fn lsp_language_id(id: &str) -> String {
    match id {
        "tsx" => "typescriptreact".to_owned(),
        "jsx" => "javascriptreact".to_owned(),
        other => other.to_owned(),
    }
}

impl LspManager {
    pub fn new(
        servers: Arc<LspServers>,
        extensions: Arc<pw_ext::Registry>,
        options: ManagerOptions,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                servers,
                extensions,
                options,
                host: OnceLock::new(),
                state: Mutex::new(State::default()),
                spawns: AtomicU64::new(0),
                generations: Arc::new(AtomicU64::new(0)),
                listeners: RwLock::new(HashMap::new()),
                status_listeners: RwLock::new(Vec::new()),
                sync_listeners: RwLock::new(Vec::new()),
            }),
        }
    }

    /// Host 를 붙인다(두 번째부터는 무시). 그 뒤 [`Self::subscribe`] 의 구독으로 [`Self::follow`] 를 돌린다.
    pub fn attach(&self, parts: HostParts) {
        let ledger = Ledger::new(ledger_dir(&parts.state_dir));
        let log_dir = lsp_log_dir(&parts.state_dir);
        let _ = self.inner.host.set(Host {
            parts,
            ledger,
            log_dir,
        });
    }

    fn host(&self) -> Result<&Host, RpcError> {
        self.inner.host.get().ok_or_else(|| {
            RpcError::internal("the language server manager is not attached to a host")
        })
    }

    /// Host 가 강제로 끝난 뒤 남은 서버 그룹을 거둔다(M4-01 의 기록부 판정 그대로 — `crate::ledger`). Host 를 시작할 때 한 번,
    /// 블로킹 스레드에서 부른다. 붙기 전이면 0.
    pub fn reap_orphans(&self) -> usize {
        self.inner
            .host
            .get()
            .map_or(0, |host| host.ledger.reap_orphans())
    }

    /// 지금까지 띄운 서버 프로세스 수(시험 · 진단).
    pub fn spawn_count(&self) -> u64 {
        self.inner.spawns.load(Ordering::SeqCst)
    }

    /// 서버 알림 `method` 를 받는다(`textDocument/publishDiagnostics` 등 — 기능 행이 단다). 받는 함수는 서버의 이벤트 태스크에서
    /// 곧바로 불린다 — 오래 걸리는 일은 채널에 넣기만 한다.
    pub fn on_notification(
        &self,
        method: &str,
        listener: impl Fn(&NotificationSource, &Value) + Send + Sync + 'static,
    ) {
        self.inner
            .listeners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(method.to_owned())
            .or_default()
            .push(Arc::new(listener));
    }

    /// 상태가 바뀌어 `lsp.status` 를 낼 때마다 그 프로젝트 id 로 부른다(서버 뜸 · 끝 · 재기동 · 신뢰 해제 · 창 닫힘 — M4-06 이
    /// 지난 세대의 진단을 지운다). 관리자 잠금 밖에서 불리므로 받는 쪽이 관리자의 읽기 함수를 불러도 된다.
    pub fn on_status(&self, listener: impl Fn(&str) + Send + Sync + 'static) {
        self.inner
            .status_listeners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::new(listener));
    }

    /// 떠 있는 서버에 문서 텍스트(`didOpen` · `didChange`)를 보낼 때마다 (출처, URI, 서버 쪽 버전)으로 부른다(M4-06 의 진단 pull).
    /// **관리자 잠금 안에서** 불린다 — 받는 쪽은 태스크 · 채널에 넣기만 하고, 관리자의 읽기 함수는 그 태스크에서 부른다(같은
    /// 스레드에서 부르면 교착).
    pub fn on_synced(
        &self,
        listener: impl Fn(&NotificationSource, &str, i64) + Send + Sync + 'static,
    ) {
        self.inner
            .sync_listeners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::new(listener));
    }

    /// 인스턴스 하나의 [`SyncHook`]. 인스턴스가 쥐므로 관리자를 약하게 가리킨다.
    fn sync_hook(&self, key: &InstanceKey, generation: u64) -> SyncHook {
        let inner = Arc::downgrade(&self.inner);
        let source = NotificationSource {
            key: key.clone(),
            generation,
        };
        Arc::new(move |uri, version| {
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let listeners = inner
                .sync_listeners
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            for listener in listeners {
                listener(&source, uri, version);
            }
        })
    }

    /// 알림 `method` 의 받는 쪽들을 부른다(서버의 알림 · 처리기가 넘긴 알림성 요청).
    fn notify_listeners(&self, method: &str, source: &NotificationSource, params: &Value) {
        let listeners = self
            .inner
            .listeners
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(method)
            .cloned()
            .unwrap_or_default();
        for listener in listeners {
            listener(source, params);
        }
    }

    /// 출처가 지금 인스턴스의 세대이고 그 서버를 쥐고 있는가(끝난 서버의 늦은 알림 · 답을 버린다 — M4-06).
    pub fn is_current(&self, source: &NotificationSource) -> bool {
        lock(&self.inner.state)
            .instances
            .get(&source.key)
            .is_some_and(|i| i.generation == source.generation && i.server.is_some())
    }

    /// 출처의 서버가 지금 세대로 떠서 동기화 · 요청을 받는 중이면 그 서버(M4-06 의 진단 pull — 요청 길의 라우팅과 따로, 진단은
    /// 모든 서버에 묻는다).
    pub fn running_server(&self, source: &NotificationSource) -> Option<Arc<LanguageServer>> {
        let state = lock(&self.inner.state);
        let instance = state.instances.get(&source.key)?;
        if instance.generation != source.generation || !instance.state.is_running() {
            return None;
        }
        instance.server.clone()
    }

    /// 출처의 서버에 지금 열려 있는 URI 와 그 서버 쪽 버전(`Synced::version`). 지난 출처면 빈 목록.
    pub fn synced_documents(&self, source: &NotificationSource) -> Vec<(String, i64)> {
        let state = lock(&self.inner.state);
        state
            .instances
            .get(&source.key)
            .filter(|i| i.generation == source.generation)
            .map(|i| {
                i.synced
                    .iter()
                    .map(|(uri, s)| (uri.clone(), s.version))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 출처의 서버에 열린 URI 의 서버 쪽 버전(M4-05 재리뷰 N3 — 진단의 `version` 을 이것과 견준다). 열려 있지 않거나 지난 출처면
    /// `None`.
    pub fn synced_version(&self, source: &NotificationSource, uri: &str) -> Option<i64> {
        let state = lock(&self.inner.state);
        let instance = state
            .instances
            .get(&source.key)
            .filter(|i| i.generation == source.generation)?;
        instance.synced.get(uri).map(|s| s.version)
    }

    /// Host 의 이벤트 버스(붙기 전이면 `None`).
    pub fn bus(&self) -> Option<EventBus> {
        self.inner.host.get().map(|h| h.parts.bus.clone())
    }

    /// 그 언어(Linguist 이름 또는 확장의 언어 id)를 맡는 서버 정의: (확장 언어 id, [(정의 순서, 서버 id, root_markers)]).
    fn language_servers(&self, language: &str) -> (String, Vec<(usize, String, Vec<String>)>) {
        let id = self
            .inner
            .extensions
            .languages()
            .find(|(_, l)| l.id == language || l.linguist.eq_ignore_ascii_case(language))
            .map_or_else(|| language.to_ascii_lowercase(), |(_, l)| l.id.clone());
        let defs = self
            .inner
            .servers
            .definitions()
            .enumerate()
            .filter(|(_, (_, def))| def.languages.iter().any(|l| l.name == id))
            .map(|(order, (_, def))| (order, def.id.clone(), def.root_markers.clone()))
            .collect();
        (id, defs)
    }

    // ---- 문서 동기화 --------------------------------------------------------------------------------------------------------

    /// `lsp.document.open`.
    pub async fn open(
        &self,
        ctx: &Ctx,
        params: LspDocumentOpenParams,
    ) -> Result<LspDocumentOpened, RpcError> {
        let host = self.host()?;
        let projects = host.parts.projects.clone();
        let (project_id, worktree) = (params.project_id.clone(), params.worktree.clone());
        // 허가 판정: 그 프로젝트 창의 연결인가, 신뢰하는가. 띄우는 자리(`start`)가 다시 받는다 — 여기서는 판정만.
        let untrusted = match lsp_grant(
            ctx.scope(),
            ctx.connection_closed(),
            &projects,
            &project_id,
            worktree.as_deref(),
        ) {
            Ok(_) => false,
            Err(e) if reason(&e) == Some("untrusted") => true,
            Err(e) => return Err(e),
        };
        let path = PathBuf::from(&params.path);
        let root = tokio::task::spawn_blocking({
            let (project_id, worktree) = (project_id.clone(), worktree.clone());
            move || project_root(&projects, &project_id, worktree.as_deref())
        })
        .await
        .map_err(|e| RpcError::internal(e.to_string()))??;
        let dir = path
            .parent()
            .filter(|_| path.is_absolute())
            .and_then(real_path)
            .filter(|dir| dir.starts_with(&root))
            .ok_or_else(|| {
                RpcError::invalid_params(format!(
                    "{} is not inside the project root {}",
                    path.display(),
                    root.display()
                ))
            })?;
        let (language, defs) = self.language_servers(&params.language);
        let off = |off| LspDocumentOpened {
            doc_id: String::new(),
            off: Some(off),
            instances: Vec::new(),
        };
        if defs.is_empty() {
            return Ok(off(LspDocumentOff::NoServer));
        }
        let text = DocText::new(params.text);
        match text.over_limit() {
            Some(OverLimit::TooLarge) => return Ok(off(LspDocumentOff::TooLarge)),
            Some(OverLimit::LongLine) => return Ok(off(LspDocumentOff::LongLine)),
            None => {}
        }
        let uri = file_uri(&dir.join(path.file_name().unwrap_or_default()))
            .ok_or_else(|| RpcError::invalid_params("the document path is not absolute"))?;
        let keys: Vec<(usize, InstanceKey)> = defs
            .into_iter()
            .map(|(order, server, markers)| {
                let root = crate::root::workspace_root(&root, &dir, &markers);
                (
                    order,
                    InstanceKey {
                        project_id: project_id.clone(),
                        worktree: worktree.clone(),
                        server,
                        root,
                    },
                )
            })
            .collect();
        let session = ctx.session();
        let mut starts = Vec::new();
        let (doc_id, watch, instances) = {
            let mut state = lock(&self.inner.state);
            let state = &mut *state;
            let watch = hold(&mut state.sessions, ctx);
            // 같은 연결이 같은 파일을 다시 열면 앞 문서를 닫고 연다(`syntax.open` 의 「다시 연다」 와 같은 뜻). 다른 연결이 연 같은
            // 파일은 서버에 한 번만 열린다(`Synced`).
            let reopened: Vec<String> = state
                .docs
                .iter()
                .filter(|(_, d)| d.session == session && d.uri == uri)
                .map(|(id, _)| id.clone())
                .collect();
            for old in reopened {
                self.remove_doc(state, &old);
            }
            state.next_doc += 1;
            let doc_id = format!("lsp-{}", state.next_doc);
            let doc = Doc {
                session,
                project_id: project_id.clone(),
                uri,
                language_id: lsp_language_id(&language),
                version: params.version,
                text: Arc::new(text),
                instances: keys.iter().map(|(_, k)| k.clone()).collect(),
                early: BTreeMap::new(),
                diverged: false,
            };
            state.docs.insert(doc_id.clone(), doc);
            let State {
                docs, instances, ..
            } = state;
            let doc = &docs[&doc_id];
            for (order, key) in &keys {
                let created = !instances.contains_key(key);
                let instance = instances.entry(key.clone()).or_insert_with(|| {
                    Instance::new(*order, root.clone(), Arc::clone(&self.inner.generations))
                });
                instance.docs.insert(doc_id.clone());
                instance.holders.insert(session);
                instance.last_user = Some(session);
                // 유휴 시한을 거둔다(그 태스크를 끝낸다).
                if let Some((_, idle)) = instance.idle.take() {
                    idle.cancel();
                }
                if instance.state.is_running()
                    && let Some(server) = instance.server.clone()
                {
                    sync_open(&server, instance, &doc_id, doc, docs);
                } else if untrusted {
                    if instance.server.is_none() {
                        instance.bump();
                        instance.state = LspInstanceState::Untrusted;
                    }
                } else if created || retry_on_open(&instance.state) {
                    instance.bump();
                    instance.state = LspInstanceState::Starting;
                    starts.push((key.clone(), instance.generation));
                }
            }
            let statuses = keys
                .iter()
                .filter_map(|(_, k)| instances.get(k).map(|i| status_of(k, i)))
                .collect();
            (doc_id, watch, statuses)
        };
        if let Some(closed) = watch {
            self.watch_close(session, closed);
        }
        let holder = Holder::of(ctx);
        for (key, generation) in starts {
            self.spawn_start(key, generation, holder.clone());
        }
        self.publish(&project_id);
        Ok(LspDocumentOpened {
            doc_id,
            off: None,
            instances,
        })
    }

    /// `lsp.document.change`.
    pub fn change(
        &self,
        ctx: &Ctx,
        params: LspDocumentChangeParams,
    ) -> Result<LspDocumentChangeResult, RpcError> {
        let mut idle = Vec::new();
        let mut off_project = None;
        let result = {
            let mut state = lock(&self.inner.state);
            let State {
                docs, instances, ..
            } = &mut *state;
            let doc = docs
                .get_mut(&params.doc_id)
                .filter(|d| d.session == ctx.session())
                .ok_or_else(|| not_found(&params.doc_id))?;
            if doc.diverged {
                return Err(conflict(
                    "an earlier edit was dropped; reopen the document".to_owned(),
                    doc.version,
                ));
            }
            if params.version <= doc.version {
                return Err(conflict(
                    format!(
                        "edit version {} is not after the document version {}",
                        params.version, doc.version
                    ),
                    doc.version,
                ));
            }
            if params.version > doc.version + 1 {
                if doc.early.len() >= MAX_EARLY_EDITS {
                    return Err(conflict(
                        format!(
                            "edit {} arrived but edit {} never did",
                            params.version,
                            doc.version + 1
                        ),
                        doc.version,
                    ));
                }
                doc.early.insert(params.version, params.changes);
                return Ok(LspDocumentChangeResult { off: None });
            }
            let events = Arc::make_mut(&mut doc.text)
                .apply(&params.changes)
                .map_err(RpcError::invalid_params)?;
            doc.version = params.version;
            let mut batches = vec![(doc.version, events)];
            while let Some(changes) = doc.early.remove(&(doc.version + 1)) {
                match Arc::make_mut(&mut doc.text).apply(&changes) {
                    Ok(events) => {
                        doc.version += 1;
                        batches.push((doc.version, events));
                    }
                    Err(e) => {
                        tracing::warn!(target: "pw_lsp", doc = %params.doc_id, "dropping an early edit: {e}");
                        doc.early.clear();
                        doc.diverged = true;
                    }
                }
            }
            // 등급을 넘는 편집은 서버에 보내지 않고 문서를 닫는다(결정 M1-D9 · M4-D10).
            match doc.text.over_limit() {
                None => {
                    send_change(&params.doc_id, doc, instances, &batches);
                    LspDocumentChangeResult { off: None }
                }
                Some(over) => {
                    let doc_id = params.doc_id.clone();
                    off_project = Some(doc.project_id.clone());
                    idle = self.remove_doc(&mut state, &doc_id);
                    LspDocumentChangeResult {
                        off: Some(match over {
                            OverLimit::TooLarge => LspDocumentOff::TooLarge,
                            OverLimit::LongLine => LspDocumentOff::LongLine,
                        }),
                    }
                }
            }
        };
        for key in idle {
            self.schedule_idle(key);
        }
        if let Some(project) = off_project {
            self.publish(&project);
        }
        Ok(result)
    }

    /// `lsp.document.save`.
    pub fn save(&self, ctx: &Ctx, doc_id: &str) -> Result<(), RpcError> {
        let mut state = lock(&self.inner.state);
        let State {
            docs, instances, ..
        } = &mut *state;
        let doc = docs
            .get(doc_id)
            .filter(|d| d.session == ctx.session())
            .ok_or_else(|| not_found(doc_id))?;
        for key in &doc.instances {
            let Some(instance) = instances.get_mut(key) else {
                continue;
            };
            let (Some(server), Some(include_text)) = (instance.server.clone(), instance.sync.save)
            else {
                continue;
            };
            let sync = instance.sync;
            let Some(synced) = instance
                .synced
                .get_mut(&doc.uri)
                .filter(|s| s.refs.contains(doc_id))
            else {
                continue;
            };
            // 저장한 텍스트가 서버의 텍스트다.
            take_over(&server, sync, synced, doc_id, doc, docs);
            let mut params = json!({ "textDocument": { "uri": doc.uri } });
            if include_text {
                params["text"] = Value::String(doc.text.as_str().to_owned());
            }
            let _ = server.notify_raw("textDocument/didSave", params);
        }
        Ok(())
    }

    /// `lsp.document.close`. 없는 문서 · 다른 연결의 문서는 아무것도 하지 않는다.
    pub fn close(&self, ctx: &Ctx, doc_id: &str) {
        let (idle, project) = {
            let mut state = lock(&self.inner.state);
            match state.docs.get(doc_id) {
                Some(doc) if doc.session == ctx.session() => {
                    let project = doc.project_id.clone();
                    (self.remove_doc(&mut state, doc_id), Some(project))
                }
                _ => (Vec::new(), None),
            }
        };
        for key in idle {
            self.schedule_idle(key);
        }
        if let Some(project) = project {
            self.publish(&project);
        }
    }

    /// 문서를 뺀다. 그 URI 를 연 마지막 문서면 서버에 `didClose`, 다른 연결의 문서가 남았으면 그 문서가 서버 텍스트의 주인이 된다.
    /// 마지막 문서를 잃은 인스턴스(유휴 시한을 걸 것)를 돌려준다.
    fn remove_doc(&self, state: &mut State, doc_id: &str) -> Vec<InstanceKey> {
        let Some(doc) = state.docs.remove(doc_id) else {
            return Vec::new();
        };
        let mut emptied = Vec::new();
        for key in &doc.instances {
            let Some(instance) = state.instances.get_mut(key) else {
                continue;
            };
            instance.docs.remove(doc_id);
            let sync = instance.sync;
            if let Some(synced) = instance.synced.get_mut(&doc.uri)
                && synced.refs.remove(doc_id)
            {
                let server = instance.server.clone();
                if synced.refs.is_empty() {
                    instance.synced.remove(&doc.uri);
                    if let Some(server) = server {
                        let _ = server.notify_raw(
                            "textDocument/didClose",
                            json!({ "textDocument": { "uri": doc.uri } }),
                        );
                    }
                } else if synced.owner == doc_id
                    && let Some(next) = synced.refs.first().cloned()
                    && let Some(next_doc) = state.docs.get(&next)
                {
                    synced.owner = next.clone();
                    if next_doc.text.as_str() != doc.text.as_str()
                        && let Some(server) = server
                    {
                        full_change(&server, sync, synced, next_doc);
                    }
                }
            }
            if instance.docs.is_empty() {
                emptied.push(key.clone());
            }
        }
        emptied
    }

    // ---- 수명 ---------------------------------------------------------------------------------------------------------------

    /// 유휴 시한(설정 `[lsp] idle_shutdown_minutes`).
    fn idle_duration(&self) -> Duration {
        if let Some(d) = self.inner.options.idle_shutdown {
            return d;
        }
        let minutes = self
            .inner
            .host
            .get()
            .and_then(|h| {
                h.parts
                    .global
                    .snapshot()
                    .get("settings", pw_lsp_servers::settings::SETTINGS_KEY)
                    .and_then(|lsp| lsp.get("idle_shutdown_minutes"))
                    .and_then(Value::as_u64)
            })
            .unwrap_or(DEFAULT_IDLE_SHUTDOWN_MINUTES);
        Duration::from_secs(minutes.saturating_mul(60))
    }

    /// 문서가 없는 인스턴스에 유휴 시한을 건다. 그사이 문서가 열리면 `open` 이 거둔다.
    fn schedule_idle(&self, key: InstanceKey) {
        let duration = self.idle_duration();
        let token = CancellationToken::new();
        let id = {
            let mut state = lock(&self.inner.state);
            state.next_idle += 1;
            let id = state.next_idle;
            let Some(instance) = state.instances.get_mut(&key) else {
                return;
            };
            if !instance.docs.is_empty() {
                return;
            }
            instance.idle = Some((id, token.clone()));
            id
        };
        let this = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(duration) => this.idle_expired(&key, id).await,
                () = token.cancelled() => {}
            }
        });
    }

    async fn idle_expired(&self, key: &InstanceKey, id: u64) {
        let server = {
            let mut state = lock(&self.inner.state);
            let Some(instance) = state.instances.get(key) else {
                return;
            };
            if !instance.docs.is_empty() || instance.idle.as_ref().map(|(i, _)| *i) != Some(id) {
                return;
            }
            let mut instance = state.instances.remove(key).expect("checked above");
            instance.take_server(LspInstanceState::Stopped)
        };
        tracing::info!(target: "pw_lsp", project = %key.project_id, server = %key.server, root = %key.root.display(), "language server idle; stopping it");
        self.publish(&key.project_id);
        if let Some(server) = server {
            server.end().await;
        }
    }

    /// 연결이 닫혔다: 그 연결의 문서를 닫고, 연 연결이 남지 않은 인스턴스를 곧바로 끝낸다.
    fn session_closed(&self, session: SessionId) {
        let mut stopping = Vec::new();
        let mut idle = Vec::new();
        let mut projects = BTreeSet::new();
        {
            let mut state = lock(&self.inner.state);
            state.sessions.remove(&session);
            let owned: Vec<String> = state
                .docs
                .iter()
                .filter(|(_, d)| d.session == session)
                .map(|(id, _)| id.clone())
                .collect();
            for doc_id in owned {
                if let Some(doc) = state.docs.get(&doc_id) {
                    projects.insert(doc.project_id.clone());
                }
                idle.extend(self.remove_doc(&mut state, &doc_id));
            }
            let orphans: Vec<InstanceKey> = state
                .instances
                .iter_mut()
                .filter_map(|(key, instance)| {
                    instance
                        .holders
                        .remove(&session)
                        .then_some(())
                        .filter(|()| instance.holders.is_empty())
                        .map(|()| key.clone())
                })
                .collect();
            for key in orphans {
                if let Some(mut instance) = state.instances.remove(&key) {
                    projects.insert(key.project_id.clone());
                    if let Some(server) = instance.take_server(LspInstanceState::Stopped) {
                        stopping.push((key, server));
                    }
                }
            }
        }
        idle.retain(|key| !stopping.iter().any(|(k, _)| k == key));
        for (key, server) in stopping {
            tracing::info!(target: "pw_lsp", project = %key.project_id, server = %key.server, "the window's connection closed; stopping its language server");
            tokio::spawn(server.end());
        }
        for key in idle {
            self.schedule_idle(key);
        }
        for project in projects {
            self.publish(&project);
        }
    }

    /// 연결의 닫힘을 지켜본다 — 닫히면 [`Self::session_closed`]. [`hold`] 가 처음 적은 연결에 한 번.
    fn watch_close(&self, session: SessionId, closed: CancellationToken) {
        let this = self.clone();
        tokio::spawn(async move {
            closed.cancelled().await;
            this.session_closed(session);
        });
    }

    /// 띄우기를 태스크로 보낸다(fork 는 `crate::process` 의 전용 스레드가 한다).
    fn spawn_start(&self, key: InstanceKey, generation: u64, holder: Holder) {
        let this = self.clone();
        tokio::spawn(async move { this.start(key, generation, holder).await });
    }

    /// 지금 시도(`generation`)가 그 인스턴스의 것이면 상태를 바꾸고 알린다.
    fn set_state(&self, key: &InstanceKey, generation: u64, next: LspInstanceState) -> bool {
        let changed = {
            let mut state = lock(&self.inner.state);
            match state.instances.get_mut(key) {
                Some(instance) if instance.generation == generation => {
                    instance.state = next;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.publish(&key.project_id);
        }
        changed
    }

    fn current(&self, key: &InstanceKey, generation: u64) -> bool {
        lock(&self.inner.state)
            .instances
            .get(key)
            .is_some_and(|i| i.generation == generation)
    }

    async fn start(&self, key: InstanceKey, generation: u64, holder: Holder) {
        let Ok(host) = self.host() else {
            return;
        };
        let projects = host.parts.projects.clone();
        // 허가를 다시 받는다 — 연 연결의 범위 · 닫힘 토큰으로(결정 M4-D8, M4-01 재리뷰 N5).
        let grant = match lsp_grant(
            &holder.scope,
            holder.closed.clone(),
            &projects,
            &key.project_id,
            key.worktree.as_deref(),
        ) {
            Ok(grant) => grant,
            Err(e) => {
                let next = if reason(&e) == Some("untrusted") {
                    LspInstanceState::Untrusted
                } else {
                    LspInstanceState::Failed { message: e.message }
                };
                self.set_state(&key, generation, next);
                return;
            }
        };
        // 해석 · 합친 설정 · 서버 런타임 — 로그인 셸 · 실행 파일을 기다리므로 블로킹 스레드에서.
        let servers = Arc::clone(&self.inner.servers);
        let k = key.clone();
        let prepared = tokio::task::spawn_blocking(move || prepare(&servers, &k)).await;
        let prepared = match prepared {
            Ok(Ok(p)) => p,
            Ok(Err(next)) => {
                self.set_state(&key, generation, next);
                return;
            }
            Err(e) => {
                self.set_state(
                    &key,
                    generation,
                    LspInstanceState::Failed {
                        message: e.to_string(),
                    },
                );
                return;
            }
        };
        // 설치본에서 찾았으면 띄우기 전에 적는다 — 그사이(기동 중 포함) 도구를 지우면 `stop_installed` 가 이 시도를 버리고 띄운
        // 서버를 끝낸다(재리뷰 N1).
        {
            let mut state = lock(&self.inner.state);
            match state.instances.get_mut(&key) {
                Some(instance) if instance.generation == generation => {
                    instance.installed = prepared.installed.clone();
                }
                _ => return,
            }
        }
        let prepared_installed = prepared.installed.clone();
        let instance_name = instance_name(&key);
        let log = ServerLog::open(
            &host.log_dir,
            &key.server,
            &instance_name,
            self.inner.options.trace,
        )
        .map_err(|e| tracing::warn!(target: "pw_lsp", %e, "cannot open the language server log"))
        .ok();
        let (events_tx, events_rx) = mpsc::channel(self.inner.options.event_capacity.max(1));
        let config = Arc::new(prepared.config);
        let handlers = server_handlers(&HandlerContext {
            project_id: key.project_id.clone(),
            worktree: key.worktree.clone(),
            server: key.server.clone(),
            root: key.root.clone(),
            config: Arc::clone(&config),
            bus: Some(host.parts.bus.clone()),
            projects: Some(host.parts.projects.clone()),
            manager: Some(self.downgrade()),
            forward: {
                let this = Arc::downgrade(&self.inner);
                let source = NotificationSource {
                    key: key.clone(),
                    generation,
                };
                Arc::new(move |method: &str, params: &Value| {
                    if let Some(inner) = this.upgrade() {
                        LspManager { inner }.notify_listeners(method, &source, params);
                    }
                })
            },
        });
        let spawned = LanguageServer::spawn(
            &grant,
            prepared.spec,
            ServerOptions {
                instance: instance_name,
                log,
                handlers,
                events: Some(events_tx),
                ledger: Some(Arc::clone(&host.ledger)),
            },
        )
        .await;
        drop(grant);
        let server = match spawned {
            Ok(server) => Arc::new(server),
            Err(e) => {
                let next = if e.kind() == std::io::ErrorKind::PermissionDenied
                    && !projects.is_trusted(&key.project_id)
                {
                    LspInstanceState::Untrusted
                } else {
                    LspInstanceState::Failed {
                        message: format!("cannot start {}: {e}", key.server),
                    }
                };
                self.set_state(&key, generation, next);
                return;
            }
        };
        self.inner.spawns.fetch_add(1, Ordering::SeqCst);
        // `initialize` 를 기다리는 동안에도 인스턴스가 서버를 쥔다(`starting` 그대로) — 그사이의 신뢰 해제 · 창 닫힘 · restart ·
        // stop 이 이 서버를 곧바로 끝낸다(M4-05 리뷰 H1). 그새 시도가 바뀌었으면(세대) 띄운 것을 버린다.
        let placed = {
            let mut state = lock(&self.inner.state);
            match state.instances.get_mut(&key) {
                Some(instance) if instance.generation == generation => {
                    instance.server = Some(Arc::clone(&server));
                    true
                }
                _ => false,
            }
        };
        if !placed {
            server.kill();
            return;
        }
        self.watch_events(&key, generation, &server, events_rx);
        let mut init = InitializeOptions::new(&key.root);
        init.initialization_options = Some(prepared.initialization_options);
        match server.initialize(init).await {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(target: "pw_lsp", project = %key.project_id, server = %key.server, %e, "language server failed to initialize");
                self.failed(&key, generation, Some(&server));
                return;
            }
        }
        let sync = sync_of(&server.capabilities());
        let installed = prepared_installed;
        let mut feeding = None;
        let installed = {
            let mut state = lock(&self.inner.state);
            let State {
                docs, instances, ..
            } = &mut *state;
            match instances.get_mut(&key) {
                Some(instance)
                    if instance.generation == generation
                        && instance
                            .server
                            .as_ref()
                            .is_some_and(|s| Arc::ptr_eq(s, &server)) =>
                {
                    instance.state = LspInstanceState::Ready;
                    instance.config = Some(config);
                    instance.installed = installed;
                    instance.sync = sync;
                    let feed = Feed::new(self.inner.options.watch.max_pending);
                    instance.feed = Some(Arc::clone(&feed));
                    feeding = Some((feed, instance.scope.clone()));
                    instance.sync_hook = Some(self.sync_hook(&key, generation));
                    for doc_id in instance.docs.clone() {
                        if let Some(doc) = docs.get(&doc_id) {
                            sync_open(&server, instance, &doc_id, doc, docs);
                        }
                    }
                    true
                }
                _ => false,
            }
        };
        if installed {
            tracing::info!(target: "pw_lsp", project = %key.project_id, server = %key.server, root = %key.root.display(), "language server ready");
            if let Some((feed, scope)) = feeding {
                self.spawn_feed(&key, generation, &server, feed, scope);
            }
            self.publish(&key.project_id);
            let this = self.clone();
            let watched = Arc::clone(&server);
            tokio::spawn(async move {
                let exit = watched.exited().await;
                if exit.reason != ExitReason::Stopped {
                    this.failed(&key, generation, Some(&watched));
                }
            });
        }
        // 아니면 그사이 끝내라는 말이 왔다(창 닫힘 · 신뢰 해제 · 다시 시작 · 멈춤) — 서버를 내려놓은 쪽이 이미 끝냈다.
    }

    /// 떠 있는 서버의 파일 감시 전달을 띄운다: 파일 목록 인덱스의 다시 만든 목록을 받는 관찰자를 붙이고, 자리를 비우는 태스크가
    /// 끝날 때(자리 닫힘 · 서버 끝) 뗀다.
    fn spawn_feed(
        &self,
        key: &InstanceKey,
        generation: u64,
        server: &Arc<LanguageServer>,
        feed: Arc<Feed>,
        scope: PathBuf,
    ) {
        let files = self.inner.host.get().and_then(|h| h.parts.files.clone());
        let observation = files.as_ref().map(|files| {
            let id = files.observe_queued(Arc::new(FeedObserver {
                project_id: key.project_id.clone(),
                root: scope.clone(),
                feed: Arc::downgrade(&feed),
            }));
            feed.set_observer(id);
            id
        });
        let link = FeedLink {
            manager: self.clone(),
            key: key.clone(),
            generation,
        };
        let run = crate::watched::run(
            link,
            key.server.clone(),
            scope,
            key.root.clone(),
            Arc::downgrade(server),
            feed,
            self.inner.options.watch.clone(),
        );
        tokio::spawn(async move {
            run.await;
            if let (Some(files), Some(id)) = (files, observation) {
                files.unobserve(id);
            }
        });
    }

    /// 그 프로젝트에 파일 감시 전달의 자리가 있는가(없으면 `fs.changed` 를 읽지도 않는다 — 리뷰 L1).
    fn has_feeds(&self, project_id: &str) -> bool {
        lock(&self.inner.state)
            .instances
            .iter()
            .any(|(k, i)| k.project_id == project_id && i.feed.is_some())
    }

    /// `fs.changed` 하나를 그 루트의 떠 있는 서버들의 자리에 쌓는다(`follow` 가 부른다). `rescan` 이면 경로가 없다 — 파일 목록
    /// 인덱스가 다시 만든 목록으로 다시 알린다(`crate::watched` 머리 주석). 쌓기가 넘친 자리는 그 자리에만 인덱스의 지금 목록을 다시
    /// 청해 메운다(리뷰 H1 — 버린 바뀜을 조용히 잃지 않게, [`Self::refill`]).
    pub fn fs_changed(&self, project_id: &str, changed: &pw_fs::watch::FsChanged) {
        if changed.paths.is_empty() {
            return;
        }
        let root = Path::new(&changed.root);
        let feeds: Vec<Arc<Feed>> = lock(&self.inner.state)
            .instances
            .iter()
            .filter(|(k, i)| k.project_id == project_id && i.scope == root)
            .filter_map(|(_, i)| i.feed.clone())
            .collect();
        if feeds.is_empty() {
            return;
        }
        let events: Vec<WatchEvent> = changed
            .paths
            .iter()
            .zip(&changed.kinds)
            .map(|(path, kind)| WatchEvent {
                path: PathBuf::from(path),
                change: (*kind).into(),
            })
            .collect();
        let overflowed: Vec<Arc<Feed>> = feeds.into_iter().filter(|f| f.push(&events)).collect();
        if !overflowed.is_empty() {
            tracing::warn!(target: "pw_lsp", root = %changed.root, servers = overflowed.len(), "file changes overflowed; announcing the folder again");
            self.refill(project_id, root, &overflowed);
        }
    }

    /// 관리자의 파일 바뀜 구독이 넘쳤다(`Lagged` — 어느 루트의 `fs.changed` 를 잃었는지 모른다): 떠 있는 서버의 자리마다 그 루트의
    /// 지금 목록을 다시 청해 메운다([`Self::refill`]).
    pub fn fs_lagged(&self) {
        let feeds: Vec<(String, PathBuf, Arc<Feed>)> = lock(&self.inner.state)
            .instances
            .iter()
            .filter_map(|(k, i)| Some((k.project_id.clone(), i.scope.clone(), i.feed.clone()?)))
            .collect();
        for (project_id, root, feed) in feeds {
            self.refill(&project_id, &root, &[feed]);
        }
    }

    /// 그 자리들에 그 루트의 지금 목록을 다시 보내게 한다: 파일 목록 인덱스에 그 자리의 관찰자 하나에만 지금 목록을 청한다
    /// (`FileIndex::replay` — 다시 만들지 않는다). 그 목록(`rebuilt`)으로 버린 바뀜을 메운다(지운 파일은 목록에 없어 메우지 못한다).
    /// 같은 루트를 보는 다른 관찰자(심볼 인덱스 · 다른 서버)와 버스의 `fs.changed` 구독자(런타임 · git · 트리 · 열린 문서)는 움직이지
    /// 않는다 — 버스에 `rescan` 을 내면 런타임이 그 프로젝트의 서버 전부를 「다시 시작 권함」 으로 바꾸는 등 모두가 움직인다(재리뷰
    /// R1). 인덱스가 없거나 그 루트를 열지 않았으면 메우지 못한다(경고).
    fn refill(&self, project_id: &str, root: &Path, feeds: &[Arc<Feed>]) {
        let files = self.inner.host.get().and_then(|h| h.parts.files.clone());
        for feed in feeds {
            let asked = match (&files, feed.observer()) {
                (Some(files), Some(id)) => files.replay(id, project_id, root),
                _ => false,
            };
            if !asked {
                feed.refill_failed();
                tracing::warn!(target: "pw_lsp", root = %root.display(), "there is no file list to announce the folder again; some file changes are lost");
            }
        }
    }

    /// 서버의 이벤트를 받는 태스크(서버마다 따로).
    fn watch_events(
        &self,
        key: &InstanceKey,
        generation: u64,
        server: &Arc<LanguageServer>,
        mut events: mpsc::Receiver<ServerEvent>,
    ) {
        let this = self.clone();
        let key = key.clone();
        let weak = Arc::downgrade(server);
        tokio::spawn(async move {
            let source = NotificationSource {
                key: key.clone(),
                generation,
            };
            while let Some(event) = events.recv().await {
                match event {
                    ServerEvent::Notification { method, params } if method == "$/progress" => {
                        this.progress(&key, generation, &params, weak.clone());
                    }
                    ServerEvent::Notification { method, params } => {
                        this.notify_listeners(&method, &source, &params);
                    }
                    // 동적 등록 · 해제 — 웹뷰의 능력 칸(M4-07 `lsp.document.capabilities`)이 상태 이벤트를 받고 다시 묻는다.
                    ServerEvent::CapabilitiesChanged => this.publish(&key.project_id),
                    ServerEvent::ShowMessageRequest { .. } => {}
                    ServerEvent::Exited(_) => break,
                }
            }
        });
    }

    fn progress(
        &self,
        key: &InstanceKey,
        generation: u64,
        params: &Value,
        server: std::sync::Weak<LanguageServer>,
    ) {
        let Ok(host) = self.host() else {
            return;
        };
        let changed =
            {
                let mut state = lock(&self.inner.state);
                match state.instances.get_mut(key) {
                    Some(instance) if instance.generation == generation => instance
                        .progress
                        .handle(params, &host.parts.tasks, &key.project_id, server),
                    _ => false,
                }
            };
        if changed {
            self.publish(&key.project_id);
        }
    }

    /// 띄운 서버가 죽었거나 `initialize` 에 실패했다: 죽음을 세고 물러남으로 다시 띄우거나 `crashed` 로 멈춘다. `watched` 가 있으면
    /// 그 서버가 지금 인스턴스의 서버일 때만.
    fn failed(&self, key: &InstanceKey, generation: u64, watched: Option<&Arc<LanguageServer>>) {
        let options = &self.inner.options;
        let retry = {
            let mut state = lock(&self.inner.state);
            let State {
                instances,
                sessions,
                ..
            } = &mut *state;
            let Some(instance) = instances.get_mut(key) else {
                return;
            };
            if instance.generation != generation {
                return;
            }
            if let Some(watched) = watched
                && !instance
                    .server
                    .as_ref()
                    .is_some_and(|s| Arc::ptr_eq(s, watched))
            {
                return;
            }
            let now = Instant::now();
            instance.crashes.push_back(now);
            while instance
                .crashes
                .front()
                .is_some_and(|t| now.duration_since(*t) > options.crash_window)
            {
                instance.crashes.pop_front();
            }
            if instance.crashes.len() >= options.crash_limit {
                // 죽은 서버거나 `initialize` 에 실패한(시한을 넘긴 — 아직 살아 있을 수 있는) 서버다.
                if let Some(taken) = instance.take_server(LspInstanceState::Crashed) {
                    taken.server.kill();
                }
                tracing::warn!(target: "pw_lsp", project = %key.project_id, server = %key.server, deaths = instance.crashes.len(), "language server keeps dying; not starting it again");
                None
            } else {
                let deaths = instance.crashes.len();
                if let Some(taken) = instance.take_server(LspInstanceState::Starting) {
                    taken.server.kill();
                }
                let holder = instance
                    .holders
                    .iter()
                    .filter_map(|s| sessions.get(s))
                    .find(|h| !h.closed.is_cancelled())
                    .cloned();
                let exp = u32::try_from(deaths.saturating_sub(1))
                    .unwrap_or(u32::MAX)
                    .min(16);
                let delay = options
                    .backoff_base
                    .saturating_mul(1 << exp)
                    .min(options.backoff_max);
                Some((instance.generation, holder, delay))
            }
        };
        self.publish(&key.project_id);
        let Some((next, holder, delay)) = retry else {
            return;
        };
        let Some(holder) = holder else {
            return;
        };
        tracing::info!(target: "pw_lsp", project = %key.project_id, server = %key.server, ?delay, "language server died; starting it again after a pause");
        let this = self.clone();
        let key = key.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if this.current(&key, next) {
                this.start(key, next, holder).await;
            }
        });
    }

    /// `lsp.server.restart`.
    pub async fn restart(
        &self,
        ctx: &Ctx,
        params: LspServerControlParams,
    ) -> Result<LspInstanceStatus, RpcError> {
        let host = self.host()?;
        let key = self.control_key(&params);
        let grant = lsp_grant(
            ctx.scope(),
            ctx.connection_closed(),
            &host.parts.projects,
            &params.project_id,
            params.worktree.as_deref(),
        );
        let untrusted = match &grant {
            Ok(_) => false,
            Err(e) if reason(e) == Some("untrusted") => true,
            Err(e) => return Err(e.clone()),
        };
        drop(grant);
        let session = ctx.session();
        let holder = Holder::of(ctx);
        let (old, generation, watch) = {
            let mut state = lock(&self.inner.state);
            let State {
                instances,
                sessions,
                ..
            } = &mut *state;
            let instance = instances.get_mut(&key).ok_or_else(|| {
                RpcError::new(rpc_code::NOT_FOUND, "no such language server instance")
            })?;
            instance.crashes.clear();
            instance.holders.insert(session);
            // 문서를 연 적 없는 연결이어도 그 닫힘을 지켜본다 — 이 연결이 쥔 서버가 창보다 오래 살지 않게(M4-05 리뷰 M1).
            let watch = hold(sessions, ctx);
            let next = if untrusted {
                LspInstanceState::Untrusted
            } else {
                LspInstanceState::Starting
            };
            (instance.take_server(next), instance.generation, watch)
        };
        if let Some(closed) = watch {
            self.watch_close(session, closed);
        }
        if let Some(old) = old {
            old.end().await;
        }
        if !untrusted {
            self.spawn_start(key.clone(), generation, holder);
        }
        self.publish(&key.project_id);
        self.instance_status(&key)
    }

    /// `lsp.server.stop`.
    pub async fn stop(
        &self,
        ctx: &Ctx,
        params: LspServerControlParams,
    ) -> Result<LspInstanceStatus, RpcError> {
        let host = self.host()?;
        let key = self.control_key(&params);
        match lsp_grant(
            ctx.scope(),
            ctx.connection_closed(),
            &host.parts.projects,
            &params.project_id,
            params.worktree.as_deref(),
        ) {
            Ok(_) => {}
            Err(e) if reason(&e) == Some("untrusted") => {}
            Err(e) => return Err(e),
        }
        let (old, watch) = {
            let mut state = lock(&self.inner.state);
            let State {
                instances,
                sessions,
                ..
            } = &mut *state;
            let instance = instances.get_mut(&key).ok_or_else(|| {
                RpcError::new(rpc_code::NOT_FOUND, "no such language server instance")
            })?;
            (
                instance.take_server(LspInstanceState::Stopped),
                hold(sessions, ctx),
            )
        };
        if let Some(closed) = watch {
            self.watch_close(ctx.session(), closed);
        }
        self.publish(&key.project_id);
        if let Some(old) = old {
            old.end().await;
        }
        self.instance_status(&key)
    }

    /// 도구 제거(M4-04 `ToolUsers`) 전에 Mason 패키지 `package` 의 설치본으로 뜬 서버를 모두 끝낸다(끝날 때까지 기다린다). 그
    /// 인스턴스는 `notInstalled` 로 둔다 — 다음 open 이 다시 해석한다(PATH 등 다른 곳의 실행 파일이 있으면 그것으로 뜬다).
    pub async fn stop_installed(&self, package: &str) {
        let (servers, projects) = {
            let mut state = lock(&self.inner.state);
            let mut servers = Vec::new();
            let mut projects = BTreeSet::new();
            for (key, instance) in &mut state.instances {
                if instance.installed.as_deref() != Some(package) {
                    continue;
                }
                instance.installed = None;
                projects.insert(key.project_id.clone());
                if let Some(server) = instance.take_server(LspInstanceState::NotInstalled {
                    mason: Some(package.to_owned()),
                }) {
                    servers.push(server);
                }
            }
            (servers, projects)
        };
        for project in projects {
            self.publish(&project);
        }
        for taken in servers {
            tracing::info!(target: "pw_lsp", package, server = taken.server.server_id(), "stopping a language server before its tool is removed");
            taken.end().await;
        }
    }

    fn control_key(&self, params: &LspServerControlParams) -> InstanceKey {
        InstanceKey {
            project_id: params.project_id.clone(),
            worktree: params.worktree.clone(),
            server: params.server.clone(),
            root: PathBuf::from(&params.root),
        }
    }

    fn instance_status(&self, key: &InstanceKey) -> Result<LspInstanceStatus, RpcError> {
        lock(&self.inner.state)
            .instances
            .get(key)
            .map(|i| status_of(key, i))
            .ok_or_else(|| RpcError::new(rpc_code::NOT_FOUND, "no such language server instance"))
    }

    // ---- 상태 ---------------------------------------------------------------------------------------------------------------

    /// `lsp.status.get`.
    pub fn status(&self, project_id: &str) -> LspStatus {
        let state = lock(&self.inner.state);
        LspStatus {
            project_id: project_id.to_owned(),
            instances: state
                .instances
                .iter()
                .filter(|(k, _)| k.project_id == project_id)
                .map(|(k, i)| status_of(k, i))
                .collect(),
        }
    }

    /// 인스턴스의 서버 pid(시험 · 진단).
    pub fn pids(&self, project_id: &str) -> Vec<u32> {
        self.pids_of(project_id, None)
    }

    /// 그 프로젝트에서 서버 `server`(없으면 전부)의 서버 pid(시험 · 진단).
    pub fn pids_of(&self, project_id: &str, server: Option<&str>) -> Vec<u32> {
        lock(&self.inner.state)
            .instances
            .iter()
            .filter(|(k, _)| k.project_id == project_id)
            .filter(|(k, _)| server.is_none_or(|s| s == k.server))
            .filter_map(|(_, i)| i.server.as_ref()?.pid())
            .collect()
    }

    fn publish(&self, project_id: &str) {
        let listeners = self
            .inner
            .status_listeners
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for listener in listeners {
            listener(project_id);
        }
        let Some(host) = self.inner.host.get() else {
            return;
        };
        let status = self.status(project_id);
        if let Err(e) =
            host.parts
                .bus
                .publish(Scope::Project(project_id.to_owned()), TOPIC_STATUS, &status)
        {
            tracing::warn!(target: "pw_lsp", %e, "cannot publish lsp.status");
        }
    }

    // ---- 버스 --------------------------------------------------------------------------------------------------------------

    /// [`Self::follow`] 가 받는 구독 둘: 제어(레지스트리 변경(신뢰) · `runtime.changed` · `config.changed`)와 파일 바뀜
    /// (`fs.changed` — M4-05b). 따로 구독한다 — 많이 오는 파일 바뀜이 넘쳐도 제어의 처리(신뢰 · 설정 다시 보기)가 돌지 않고, 제어
    /// 처리가 오래 걸려도 파일 바뀜이 넘치지 않게(리뷰 L1). 버스의 큐는 구독마다 따로다.
    pub fn subscribe(bus: &EventBus) -> ManagerEvents {
        ManagerEvents {
            control: bus.subscribe(Filter::all().topics([
                registry::TOPIC_CHANGED,
                pw_runtime::runtime::TOPIC_CHANGED,
                pw_core::config::TOPIC_CHANGED,
            ])),
            files: bus.subscribe(Filter::all().topics([pw_fs::watch::TOPIC_CHANGED])),
        }
    }

    /// 구독이 닫힐 때까지 신뢰 해제 · 런타임 변경 · 설정 변경을 인스턴스에 반영하고, 파일 바뀜을 서버에 전한다(둘은 따로 돈다).
    pub async fn follow(self, events: ManagerEvents) {
        let ManagerEvents { control, files } = events;
        tokio::join!(
            self.clone().follow_control(control),
            self.follow_files(files)
        );
    }

    async fn follow_files(self, mut files: Subscription) {
        while let Some(received) = files.recv().await {
            match received {
                Received::Event(event) => {
                    let Scope::Project(project) = &event.scope else {
                        continue;
                    };
                    if !self.has_feeds(project) {
                        continue;
                    }
                    if let Ok(changed) =
                        serde_json::from_value::<pw_fs::watch::FsChanged>(event.payload.clone())
                    {
                        self.fs_changed(project, &changed);
                    }
                }
                Received::Lagged(n) => {
                    // 버스의 큐는 구독마다 따로 넘친다 — 이 구독만 넘쳤으면 파일 목록 인덱스는 그 바뀜을 적용했다. 자리마다 그
                    // 루트의 지금 목록을 다시 청해 메운다(지운 파일은 메우지 못한다).
                    tracing::warn!(target: "pw_lsp", dropped = n, "missed file change events; announcing the watched folders again");
                    self.fs_lagged();
                }
            }
        }
    }

    async fn follow_control(self, mut events: Subscription) {
        while let Some(received) = events.recv().await {
            match received {
                Received::Event(event) if event.topic == registry::TOPIC_CHANGED => {
                    if let Ok(ProjectChanged {
                        kind: ChangeKind::Trust,
                        project_id,
                    }) = serde_json::from_value(event.payload.clone())
                    {
                        self.trust_changed(&project_id).await;
                        self.recheck_trust().await;
                    }
                }
                Received::Event(event) if event.topic == pw_runtime::runtime::TOPIC_CHANGED => {
                    let project = match &event.scope {
                        Scope::Project(p) => Some(p.clone()),
                        Scope::Host => None,
                    };
                    self.runtime_changed(project.as_deref());
                }
                Received::Event(event) if event.topic == pw_core::config::TOPIC_CHANGED => {
                    self.config_changed().await;
                }
                Received::Event(_) => {}
                Received::Lagged(n) => {
                    tracing::warn!(target: "pw_lsp", dropped = n, "missed events; checking trust and settings again");
                    // 놓친 것이 신뢰 변경일 수 있다 — 모든 프로젝트의 신뢰 세대를 올린다(토큰을 잃는 쪽으로).
                    let projects: Vec<String> = lock(&self.inner.state)
                        .instances
                        .keys()
                        .map(|k| k.project_id.clone())
                        .collect();
                    self.bump_trust(projects);
                    self.recheck_trust().await;
                    self.config_changed().await;
                }
            }
        }
    }

    /// 신뢰를 잃은 프로젝트의 서버를 곧바로 트리째 끝낸다(결정 M4-D8).
    pub async fn recheck_trust(&self) {
        let Ok(host) = self.host() else {
            return;
        };
        let projects = host.parts.projects.clone();
        let ids: BTreeSet<String> = lock(&self.inner.state)
            .instances
            .keys()
            .map(|k| k.project_id.clone())
            .collect();
        let untrusted: BTreeSet<String> = tokio::task::spawn_blocking(move || {
            ids.into_iter()
                .filter(|id| !projects.is_trusted(id))
                .collect()
        })
        .await
        .unwrap_or_default();
        if untrusted.is_empty() {
            return;
        }
        let killed: Vec<(InstanceKey, Taken)> = {
            let mut state = lock(&self.inner.state);
            let mut killed = Vec::new();
            for (key, instance) in &mut state.instances {
                if !untrusted.contains(&key.project_id)
                    || instance.state == LspInstanceState::Untrusted
                {
                    continue;
                }
                // 뜨는 중이던 시도도 버린다 — 세대가 올라 그 결과가 버려진다.
                if let Some(server) = instance.take_server(LspInstanceState::Untrusted) {
                    killed.push((key.clone(), server));
                }
            }
            killed
        };
        for (key, taken) in &killed {
            tracing::info!(target: "pw_lsp", project = %key.project_id, server = %key.server, "project trust revoked; ending its language server");
            taken.server.kill();
        }
        for project in untrusted {
            self.publish(&project);
        }
    }

    fn runtime_changed(&self, project: Option<&str>) {
        let mut projects = BTreeSet::new();
        {
            let mut state = lock(&self.inner.state);
            for (key, instance) in &mut state.instances {
                if project.is_some_and(|p| p != key.project_id) {
                    continue;
                }
                if instance.state == LspInstanceState::Ready {
                    instance.state = LspInstanceState::RestartSuggested;
                    projects.insert(key.project_id.clone());
                }
            }
        }
        for project in projects {
            self.publish(&project);
        }
    }

    async fn config_changed(&self) {
        let groups: BTreeSet<(String, Option<String>)> = lock(&self.inner.state)
            .instances
            .iter()
            .filter(|(_, i)| i.config.is_some() && i.state.is_running())
            .map(|(k, _)| (k.project_id.clone(), k.worktree.clone()))
            .collect();
        for (project, worktree) in groups {
            let servers = Arc::clone(&self.inner.servers);
            let (p, w) = (project.clone(), worktree.clone());
            let Ok(Ok(configs)) =
                tokio::task::spawn_blocking(move || servers.configs(&p, w.as_deref())).await
            else {
                continue;
            };
            let changed = {
                let mut state = lock(&self.inner.state);
                let mut changed = false;
                for (key, instance) in &mut state.instances {
                    if key.project_id != project || key.worktree != worktree {
                        continue;
                    }
                    let (Some(running), Some(now)) = (&instance.config, configs.get(&key.server))
                    else {
                        continue;
                    };
                    if **running != *now && instance.state.is_running() {
                        instance.state = LspInstanceState::RestartNeeded;
                        changed = true;
                    }
                }
                changed
            };
            if changed {
                self.publish(&project);
            }
        }
    }

    // ---- 요청 길 -----------------------------------------------------------------------------------------------------------

    /// 기능 행이 서버에 닿는 하나뿐인 길(`request(doc, feature, params)`): 그 문서의 서버 중 기능을 맡을 서버를 고르고(능력이 있고
    /// 정의 · 설정에서 끄지 않은 첫 서버 — 결정 M4-D12), `params` 로 매개변수를 만들어(위치 변환은 [`DocView`]) 묻는다. 서버 오류
    /// `ContentModified` · `ServerCancelled` 는 지금 문서로 한 번 다시 묻는다. 요청이 서버에 간 뒤 문서가 바뀌었거나 부른 쪽이
    /// 계산한 버전(`expected_version`)이 문서와 다르면 결과에 `stale_version`. 부른 쪽의 RPC 가 취소되면 `$/cancelRequest` 가 간다.
    pub async fn request(
        &self,
        ctx: &Ctx,
        request: FeatureRequest<'_>,
        params: impl Fn(&DocView) -> Result<Value, RpcError>,
    ) -> Result<FeatureAnswer, RpcError> {
        self.request_via(ctx, None, request, params).await
    }

    /// [`Self::request`] 와 같은 길(같은 문서 주인 · 라우팅 규칙 · 재시도 · `stale_version`)이되 그 문서의 서버 중 id 가 `server` 인
    /// 인스턴스에만 묻는다. 그 인스턴스가 그 메서드를 맡지 못하면(떠 있지 않음 · 끔 · 능력 없음) 5001 — 다른 서버로 가지 않는다
    /// (M4-12: 코드 액션의 resolve · 명령은 목록을 준 서버로만).
    pub async fn request_on(
        &self,
        ctx: &Ctx,
        server: &str,
        request: FeatureRequest<'_>,
        params: impl Fn(&DocView) -> Result<Value, RpcError>,
    ) -> Result<FeatureAnswer, RpcError> {
        self.request_via(ctx, Some(server), request, params).await
    }

    async fn request_via(
        &self,
        ctx: &Ctx,
        only: Option<&str>,
        request: FeatureRequest<'_>,
        params: impl Fn(&DocView) -> Result<Value, RpcError>,
    ) -> Result<FeatureAnswer, RpcError> {
        let mut retried = false;
        loop {
            let (server, picked, view) = self.pick(ctx.session(), &request, only)?;
            let server_id = picked.key.server.clone();
            let body = params(&view)?;
            let options = RequestOptions {
                timeout: request
                    .timeout
                    .unwrap_or(crate::server::DEFAULT_REQUEST_TIMEOUT),
                cancel: ctx.cancellation().clone(),
            };
            match server.request_raw(request.method, body, options).await {
                Ok(result) => {
                    let now = self.doc_version(request.doc_id);
                    // 같은 파일을 연 다른 연결이 그사이 서버의 텍스트를 바꿨어도(`Synced`) 답은 다른 텍스트 기준이다(재리뷰 N2).
                    let stale = now != Some(view.version)
                        || request.expected_version.is_some_and(|v| v != view.version)
                        || self.server_text_moved(&picked, request.doc_id);
                    return Ok(FeatureAnswer {
                        result,
                        key: picked.key.clone(),
                        server: server_id,
                        stale_version: stale,
                        doc: view,
                    });
                }
                Err(RequestError::Server(e))
                    if !retried
                        && (e.code == lsp_code::CONTENT_MODIFIED
                            || e.code == lsp_code::SERVER_CANCELLED) =>
                {
                    retried = true;
                }
                Err(RequestError::Server(e)) => {
                    return Err(
                        RpcError::new(code::SERVER_ERROR, e.message.clone()).with_data(
                            json!({ "server": server_id, "code": e.code, "message": e.message }),
                        ),
                    );
                }
                Err(RequestError::Cancelled) => {
                    return Err(RpcError::new(
                        rpc_code::CANCELLED,
                        "the request was cancelled",
                    ));
                }
                Err(
                    e
                    @ (RequestError::Timeout | RequestError::Closed(_) | RequestError::Refused(_)),
                ) => {
                    return Err(RpcError::new(code::SERVER_UNAVAILABLE, e.to_string())
                        .with_data(json!({ "server": server_id, "reason": e.to_string() })));
                }
                Err(RequestError::Decode(e)) => return Err(RpcError::internal(e)),
            }
        }
    }

    /// 요청을 보낸 뒤 서버의 그 URI 텍스트가 바뀌었는가(주인이 바뀌었거나 버전이 올랐거나 닫혔다).
    fn server_text_moved(&self, picked: &Picked, doc_id: &str) -> bool {
        let state = lock(&self.inner.state);
        state
            .instances
            .get(&picked.key)
            .and_then(|i| i.synced.get(&picked.uri))
            .is_none_or(|s| s.owner != doc_id || s.version != picked.version)
    }

    fn doc_version(&self, doc_id: &str) -> Option<u32> {
        lock(&self.inner.state).docs.get(doc_id).map(|d| d.version)
    }

    /// 기능을 맡을 서버와 지금 문서.
    fn pick(
        &self,
        session: SessionId,
        request: &FeatureRequest<'_>,
        only: Option<&str>,
    ) -> Result<(Arc<LanguageServer>, Picked, DocView), RpcError> {
        let mut state = lock(&self.inner.state);
        let State {
            docs, instances, ..
        } = &mut *state;
        let doc = docs
            .get(request.doc_id)
            .filter(|d| d.session == session)
            .ok_or_else(|| not_found(request.doc_id))?;
        let chosen = route_on(
            request.doc_id,
            doc,
            instances,
            request.feature,
            request.method,
            only,
        )
        .map(|(key, server)| (Arc::clone(server), key.clone()));
        let Some((server, key)) = chosen else {
            let instances: Vec<LspInstanceStatus> = candidates(doc, instances)
                .iter()
                .map(|(k, i)| status_of(k, i))
                .collect();
            return Err(RpcError::new(
                code::NO_SERVER,
                format!(
                    "no running language server offers {}",
                    request.feature.key()
                ),
            )
            .with_data(json!({ "feature": request.feature, "instances": instances })));
        };
        // 다른 연결(창)이 같은 파일을 연 채 편집했으면 서버의 텍스트가 그 문서의 것이다 — 이 문서로 맞춘 뒤 묻는다(위치는 이 문서
        // 기준이다).
        let mut sent = 0;
        if let Some(instance) = instances.get_mut(&key) {
            instance.last_user = Some(session);
            if let Some(synced) = instance.synced.get_mut(&doc.uri) {
                take_over(&server, instance.sync, synced, request.doc_id, doc, docs);
                sent = synced.version;
            }
        }
        Ok((
            server,
            Picked {
                key,
                uri: doc.uri.clone(),
                version: sent,
            },
            DocView {
                uri: doc.uri.clone(),
                version: doc.version,
                text: Arc::clone(&doc.text),
            },
        ))
    }
}

impl LspManager {
    /// 인스턴스 `key` 의 허가를 다시 받는다(M4-08 — 루트 밖 문서의 토큰 길, 메인 판정 2026-11-02): `lsp_grant`(그 프로젝트 창의
    /// 연결인가 · 지금 신뢰하는가)를 지나고, 허가의 루트가 인스턴스의 작업 공간 루트를 품고, 그 인스턴스가 떠 있고, 부른 연결이 그
    /// 인스턴스에 문서를 열었거나 그 인스턴스를 재시작 · 정지한 적이 있을(`holders`) 때만. 아니면 permission denied · untrusted ·
    /// not found(`NO_SERVER`). 허가와 함께 지금의 세대 표지([`InstanceEpoch`])를 돌려준다 — 부른 쪽이 발급 때의 표지와 견준다.
    /// 미신뢰를 보면 그 프로젝트의 신뢰 세대를 올린다(신뢰를 다시 걸어도 그 전의 표지는 맞지 않는다).
    pub fn instance_grant(
        &self,
        ctx: &Ctx,
        key: &InstanceKey,
    ) -> Result<(LspGrant, InstanceEpoch), RpcError> {
        let host = self.host()?;
        let grant = lsp_grant(
            ctx.scope(),
            ctx.connection_closed(),
            &host.parts.projects,
            &key.project_id,
            key.worktree.as_deref(),
        )
        .inspect_err(|e| {
            if reason(e) == Some("untrusted") {
                self.bump_trust([key.project_id.clone()]);
            }
        })?;
        if !key.root.starts_with(grant.root()) {
            return Err(RpcError::new(
                rpc_code::PERMISSION_DENIED,
                "the language server instance is not under this window's root",
            ));
        }
        let state = lock(&self.inner.state);
        let instance = state
            .instances
            .get(key)
            .filter(|i| i.state.is_running() && i.server.is_some())
            .ok_or_else(|| {
                RpcError::new(code::NO_SERVER, "the language server is not running")
                    .with_data(json!({ "server": key.server }))
            })?;
        if !instance.holders.contains(&ctx.session()) {
            return Err(RpcError::new(
                rpc_code::PERMISSION_DENIED,
                "this connection has no document in that language server",
            ));
        }
        let epoch = InstanceEpoch {
            born: instance.born,
            generation: instance.generation,
            trust: state
                .trust_epochs
                .get(&key.project_id)
                .copied()
                .unwrap_or(0),
        };
        drop(state);
        Ok((grant, epoch))
    }

    /// 문서 없이 프로젝트의 서버들에 묻는 기능(`workspace/symbol` — M4-16)이 물을 인스턴스들. `lsp_grant`(그 프로젝트 창의
    /// 연결인가 · 지금 신뢰하는가 — 미신뢰면 신뢰 세대를 올리고 untrusted)를 지난 뒤, 그 허가의 루트 아래 그 프로젝트의 인스턴스 중
    /// 떠 있고 · `method` 를 다루고 · 정의 · 설정에서 `feature` 를 끄지 않았고 · **부른 연결이 그 인스턴스에 문서를 연**(그 인스턴스
    /// `docs` 의 문서 주인 세션 — 15 M4-08 ⓔ, 재시작 · 정지만 한 연결은 아니다) 것을 정의 순서로 준다. 하나하나에 지금의 세대
    /// 표지와 그 연결이 연 문서 하나의 언어(Linguist 이름)를 싣는다 — 부른 쪽은 [`Self::request_instance`] 에 그 표지를 넘긴다. 잠금은 표를 한
    /// 번 훑는 동안만 쥔다. 서버를 띄우지 않는다.
    pub fn session_instances(
        &self,
        ctx: &Ctx,
        project_id: &str,
        worktree: Option<&str>,
        feature: LspFeature,
        method: &str,
    ) -> Result<(PathBuf, Vec<SessionInstance>), RpcError> {
        let host = self.host()?;
        let grant = lsp_grant(
            ctx.scope(),
            ctx.connection_closed(),
            &host.parts.projects,
            project_id,
            worktree,
        )
        .inspect_err(|e| {
            if reason(e) == Some("untrusted") {
                self.bump_trust([project_id.to_owned()]);
            }
        })?;
        let root = grant.root().to_path_buf();
        drop(grant);
        let session = ctx.session();
        let state = lock(&self.inner.state);
        let trust = state.trust_epochs.get(project_id).copied().unwrap_or(0);
        let mut found: Vec<(usize, SessionInstance)> = state
            .instances
            .iter()
            .filter(|(key, _)| key.project_id == project_id && key.root.starts_with(&root))
            .filter_map(|(key, instance)| {
                let server = instance.server.as_ref()?;
                if !instance.state.is_running()
                    || instance
                        .config
                        .as_ref()
                        .is_some_and(|c| c.disabled_features.contains(&feature))
                    || !server.supports(method)
                {
                    return None;
                }
                let language = instance
                    .docs
                    .iter()
                    .filter_map(|id| state.docs.get(id))
                    .find(|doc| doc.session == session)?
                    .language_id
                    .clone();
                Some((
                    instance.order,
                    SessionInstance {
                        key: key.clone(),
                        epoch: InstanceEpoch {
                            born: instance.born,
                            generation: instance.generation,
                            trust,
                        },
                        language,
                    },
                ))
            })
            .collect();
        drop(state);
        found.sort_by_key(|(order, _)| *order);
        // LSP 의 `languageId` → 언어 확장의 Linguist 이름(웹뷰의 문서 언어와 같은 꼴 — 외부 소스 탭의 강조).
        for (_, found) in &mut found {
            if let Some((_, l)) = self
                .inner
                .extensions
                .languages()
                .find(|(_, l)| lsp_language_id(&l.id) == found.language)
            {
                found.language = l.linguist.clone();
            }
        }
        Ok((root, found.into_iter().map(|(_, i)| i).collect()))
    }

    /// 프로젝트들의 신뢰 세대를 올린다(머리의 [`InstanceEpoch`]).
    fn bump_trust(&self, projects: impl IntoIterator<Item = String>) {
        let mut state = lock(&self.inner.state);
        for project in projects {
            *state.trust_epochs.entry(project).or_default() += 1;
        }
    }

    /// 신뢰를 가진 프로젝트 `owner` 의 신뢰가 바뀌었다는 알림: 그 신뢰를 따르는 프로젝트(워크트리 프로젝트 포함)의 신뢰 세대를
    /// 올린다. 해제든 부여든 올린다 — 해제와 부여가 잇달아 와 서버를 끝내기 전에 신뢰가 돌아왔어도 그 전의 토큰은 살아나지 않는다.
    async fn trust_changed(&self, owner: &str) {
        let Ok(host) = self.host() else {
            return;
        };
        let projects = host.parts.projects.clone();
        let owner_id = owner.to_owned();
        let followers = tokio::task::spawn_blocking(move || projects.trust_followers(&owner_id))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_else(|| vec![owner.to_owned()]);
        self.bump_trust(followers);
    }

    /// `window/showDocument` 을 보일 창(M4-08 리뷰 M2): 인스턴스 `key` 에 지금 문서를 연 연결 중 가장 최근에 그 서버를 쓴 것(그것이
    /// 문서를 닫았으면 문서를 연 연결 중 하나), 닫히지 않은 것. 그 연결에 `period` 안에 `burst` 번을 이미 보였으면 `None`(버린다
    /// — 서버가 창을 마구 움직이지 못하게). 문서를 연 연결이 없어도 `None`.
    pub(crate) fn show_document_window(
        &self,
        key: &InstanceKey,
        burst: usize,
        period: Duration,
    ) -> Option<ShowWindow> {
        let now = Instant::now();
        let mut state = lock(&self.inner.state);
        let State {
            docs,
            instances,
            sessions,
            ..
        } = &mut *state;
        let instance = instances.get(key)?;
        let openers: BTreeSet<SessionId> = instance
            .docs
            .iter()
            .filter_map(|d| docs.get(d))
            .map(|d| d.session)
            .filter(|s| sessions.get(s).is_some_and(|h| !h.closed.is_cancelled()))
            .collect();
        let session = instance
            .last_user
            .filter(|s| openers.contains(s))
            .or_else(|| openers.iter().next().copied())?;
        let holder = sessions.get_mut(&session)?;
        while holder
            .shown
            .front()
            .is_some_and(|t| now.duration_since(*t) >= period)
        {
            holder.shown.pop_front();
        }
        if holder.shown.len() >= burst {
            return None;
        }
        holder.shown.push_back(now);
        Some(ShowWindow {
            window: holder.window.clone(),
        })
    }

    /// 인스턴스 `key` 에 지금 열린 진행(`$/progress`)의 제목 모두 — 상태(`LspInstanceStatus::progress`)의 가장 최근 하나가 아니라.
    /// 인스턴스가 없으면 빈 목록. 포맷(M4-13)이 서버 정의의 「포맷 답을 믿지 않을 진행」 과 견준다.
    ///
    /// 진행 알림은 서버의 이벤트 채널을 거쳐 여기에 들어오고 요청의 답은 곧바로 돌아오므로, 서버가 답 바로 앞에 보낸 begin 은 그
    /// 답을 받은 순간 아직 안 들어와 있을 수 있다(그 사이는 이벤트 처리 한 번).
    pub fn open_progress_titles(&self, key: &InstanceKey) -> Vec<String> {
        let state = lock(&self.inner.state);
        state
            .instances
            .get(key)
            .map(|i| i.progress.titles())
            .unwrap_or_default()
    }

    /// 인스턴스 `key` 를 띄운 설정(정의 위에 설정을 합친 것). 떠 있지 않으면 `None`.
    pub fn instance_config(&self, key: &InstanceKey) -> Option<Arc<ServerConfig>> {
        let state = lock(&self.inner.state);
        state.instances.get(key).and_then(|i| i.config.clone())
    }

    /// 처리기가 쥘 약한 참조(서버 → 처리기 → 관리자로 순환하지 않게).
    pub(crate) fn downgrade(&self) -> WeakLspManager {
        WeakLspManager(Arc::downgrade(&self.inner))
    }

    /// 문서 없이 인스턴스 `key` 의 서버에 묻는다(M4-08 — 루트 밖 문서 안의 hover · 정의, `jdt://` 의 `java/classFileContents`).
    /// 허가는 [`Self::instance_grant`] 그대로(부를 때마다 다시 본다). 오류의 꼴은 [`Self::request`] 와 같다(`ContentModified` ·
    /// `ServerCancelled` 는 한 번 다시 묻는다).
    pub async fn request_instance(
        &self,
        ctx: &Ctx,
        key: &InstanceKey,
        expected: Option<InstanceEpoch>,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, RpcError> {
        let mut retried = false;
        loop {
            let (_, epoch) = self.instance_grant(ctx, key)?;
            if expected.is_some_and(|e| e != epoch) {
                return Err(RpcError::new(
                    rpc_code::NOT_FOUND,
                    "the language server instance is not the one that was asked before",
                ));
            }
            let server = {
                let mut state = lock(&self.inner.state);
                let instance = state.instances.get_mut(key);
                let server = instance.and_then(|i| {
                    i.last_user = Some(ctx.session());
                    i.server.clone()
                });
                server.ok_or_else(|| {
                    RpcError::new(code::NO_SERVER, "the language server is not running")
                })?
            };
            let options = RequestOptions {
                timeout: timeout.unwrap_or(crate::server::DEFAULT_REQUEST_TIMEOUT),
                cancel: ctx.cancellation().clone(),
            };
            match server.request_raw(method, params.clone(), options).await {
                Ok(result) => return Ok(result),
                Err(RequestError::Server(e))
                    if !retried
                        && (e.code == lsp_code::CONTENT_MODIFIED
                            || e.code == lsp_code::SERVER_CANCELLED) =>
                {
                    retried = true;
                }
                Err(RequestError::Server(e)) => {
                    return Err(
                        RpcError::new(code::SERVER_ERROR, e.message.clone()).with_data(
                            json!({ "server": key.server, "code": e.code, "message": e.message }),
                        ),
                    );
                }
                Err(RequestError::Cancelled) => {
                    return Err(RpcError::new(
                        rpc_code::CANCELLED,
                        "the request was cancelled",
                    ));
                }
                Err(
                    e
                    @ (RequestError::Timeout | RequestError::Closed(_) | RequestError::Refused(_)),
                ) => {
                    return Err(RpcError::new(code::SERVER_UNAVAILABLE, e.to_string())
                        .with_data(json!({ "server": key.server, "reason": e.to_string() })));
                }
                Err(RequestError::Decode(e)) => return Err(RpcError::internal(e)),
            }
        }
    }
}

impl LspManager {
    /// 문서의 기능마다 그 기능을 맡을 서버와 그 서버가 알린 능력(`routes` 의 (기능, 메서드)마다 — 웹뷰 다리 M4-07 의
    /// `lsp.document.capabilities`). 고르는 규칙은 요청 길(`request`)과 같은 [`route`] 다 — 떠서 그 문서를 동기화한, 능력이
    /// 있고 끄지 않은 첫 서버(결정 M4-D12). 능력은 그 메서드의 첫 동적 등록의 옵션, 없으면 정적 능력 칸(`hoverProvider` 등)
    /// 그대로다. 맡을 서버가 없는 기능은 빠진다. 서버를 띄우지 않는다. 없는 문서 · 다른 연결의 문서는 not found.
    pub fn feature_routes(
        &self,
        session: SessionId,
        doc_id: &str,
        routes: &[(LspFeature, &str)],
    ) -> Result<Vec<(LspFeature, String, Value)>, RpcError> {
        Ok(self
            .feature_route_keys(session, doc_id, routes, None)?
            .into_iter()
            .map(|(feature, key, options)| (feature, key.server, options))
            .collect())
    }

    /// [`Self::feature_routes`] 와 같되 서버 id 대신 인스턴스 열쇠 전체를 돌려주고, `only` 가 있으면 id 가 그것인 인스턴스만 본다
    /// (M4-12: 명령의 흐름이 「그 인스턴스」 를 쥐고, resolve · 명령은 목록을 준 서버로만 간다).
    pub fn feature_route_keys(
        &self,
        session: SessionId,
        doc_id: &str,
        routes: &[(LspFeature, &str)],
        only: Option<&str>,
    ) -> Result<Vec<(LspFeature, InstanceKey, Value)>, RpcError> {
        let state = lock(&self.inner.state);
        let doc = state
            .docs
            .get(doc_id)
            .filter(|d| d.session == session)
            .ok_or_else(|| not_found(doc_id))?;
        Ok(routes
            .iter()
            .filter_map(|&(feature, method)| {
                let (key, server) = route_on(doc_id, doc, &state.instances, feature, method, only)?;
                // 동적 등록은 그 문서에 맞는 것(`documentSelector`)만 본다 — 한 서버가 언어마다 다른 옵션을 등록할 수 있다(리뷰 L-5).
                let options = server
                    .registrations(method)
                    .into_iter()
                    .filter_map(|r| r.register_options)
                    .find(|o| selects(o, doc))
                    .unwrap_or_else(|| static_capability(&server.capabilities_json(), method));
                Some((feature, key.clone(), options))
            })
            .collect())
    }
}

impl LspManager {
    /// 인스턴스 `key` 가 `paths` 중 어느 것을 **다른 연결(창)의 문서 텍스트로** 갖고 있나(M4-12 — 결정 M4-D14 ①). 같은 URI 는 서버에
    /// 한 번 열리고 그 텍스트는 주인 문서의 것이라(`Synced::owner`), 주인이 `session` 밖이면 서버가 그 파일에 준 편집의 범위는 그 창의
    /// (미저장일 수 있는) 텍스트 기준이다 — 이 창이 디스크에 쓰면 범위가 어긋나고 그 창의 버퍼 아래 디스크를 덮는다. 아무도 열지
    /// 않은 파일(서버는 디스크를 봤다) · 이 연결이 주인인 파일은 들지 않는다. 판정만 한다(잠금은 표를 한 번 훑는 동안만).
    pub fn foreign_paths<'p>(
        &self,
        key: &InstanceKey,
        session: SessionId,
        paths: impl IntoIterator<Item = &'p Path>,
    ) -> BTreeSet<PathBuf> {
        let foreign: BTreeSet<PathBuf> = {
            let state = lock(&self.inner.state);
            let Some(instance) = state.instances.get(key) else {
                return BTreeSet::new();
            };
            instance
                .synced
                .iter()
                .filter(|(_, s)| {
                    state
                        .docs
                        .get(&s.owner)
                        .is_none_or(|d| d.session != session)
                })
                .filter_map(|(uri, _)| crate::uri::uri_path(uri))
                .collect()
        };
        paths
            .into_iter()
            .filter(|p| foreign.contains(*p))
            .map(Path::to_path_buf)
            .collect()
    }
}

/// 문서를 맡는 인스턴스들(정의 순서 — 결정 M4-D12).
fn candidates<'a>(
    doc: &Doc,
    instances: &'a BTreeMap<InstanceKey, Instance>,
) -> Vec<(&'a InstanceKey, &'a Instance)> {
    let mut candidates: Vec<(&InstanceKey, &Instance)> = doc
        .instances
        .iter()
        .filter_map(|k| instances.get_key_value(k))
        .collect();
    candidates.sort_by_key(|(_, i)| i.order);
    candidates
}

/// 기능을 맡을 서버(결정 M4-D12): 떠서 그 문서를 동기화했고, 정의 · 설정에서 그 기능을 끄지 않았고, `method` 를 다루는 첫
/// 인스턴스. 요청 길과 웹뷰의 능력 칸(`feature_routes`)이 함께 쓰는 하나뿐인 규칙이다.
///
/// `only` 이 있으면 서버 id 가 그것인 인스턴스만 본다(한 문서에서 서버 id 하나는 인스턴스 하나다 — 루트는 문서마다 하나로 정해진다).
fn route_on<'a>(
    doc_id: &str,
    doc: &Doc,
    instances: &'a BTreeMap<InstanceKey, Instance>,
    feature: LspFeature,
    method: &str,
    only: Option<&str>,
) -> Option<(&'a InstanceKey, &'a Arc<LanguageServer>)> {
    candidates(doc, instances)
        .into_iter()
        .filter(|(key, _)| only.is_none_or(|s| key.server == s))
        .find_map(|(key, instance)| {
            let server = instance.server.as_ref()?;
            if !instance.state.is_running() || !instance.is_synced(doc_id, &doc.uri) {
                return None;
            }
            if instance
                .config
                .as_ref()
                .is_some_and(|c| c.disabled_features.contains(&feature))
            {
                return None;
            }
            server.supports(method).then_some((key, server))
        })
}

/// 등록 옵션의 `documentSelector` 가 문서에 맞는가. 없거나 `null` 이면 모든 문서다. 거르개(`DocumentFilter`)의 `language` ·
/// `scheme` · `pattern`(글롭) 중 적힌 것이 모두 맞는 거르개가 하나라도 있으면 맞다. 읽을 수 없는 거르개는 맞지 않는다.
fn selects(options: &Value, doc: &Doc) -> bool {
    let Some(filters) = options.get("documentSelector").filter(|v| !v.is_null()) else {
        return true;
    };
    let Some(filters) = filters.as_array() else {
        return false;
    };
    let path = crate::uri::uri_path(&doc.uri);
    filters.iter().any(|filter| {
        let field = |key: &str| filter.get(key).and_then(Value::as_str);
        field("language").is_none_or(|l| l == doc.language_id)
            && field("scheme").is_none_or(|s| doc.uri.starts_with(&format!("{s}:")))
            && field("pattern").is_none_or(|p| {
                globset::Glob::new(p)
                    .ok()
                    .zip(path.as_ref())
                    .is_some_and(|(g, path)| g.compile_matcher().is_match(path))
            })
    })
}

/// 정적 능력에서 `method` 의 칸(`METHOD_CAPABILITIES` 의 경로 그대로 — `hoverProvider` 등). 없으면 `null`.
fn static_capability(capabilities: &Value, method: &str) -> Value {
    let Some((_, path)) = crate::capabilities::METHOD_CAPABILITIES
        .iter()
        .find(|(m, _)| *m == method)
    else {
        return Value::Null;
    };
    path.split('.')
        .try_fold(capabilities, |at, key| at.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

/// 요청을 맡은 서버 쪽: 인스턴스, URI, 요청을 보낼 때 서버에 보낸 마지막 버전(`Synced::version`).
struct Picked {
    key: InstanceKey,
    uri: String,
    version: i64,
}

/// 요청 하나(기능 행이 채운다).
#[derive(Debug, Clone)]
pub struct FeatureRequest<'a> {
    pub doc_id: &'a str,
    /// 기능(라우팅 · 끄기 판정 — 결정 M4-D12).
    pub feature: LspFeature,
    /// LSP 메서드(`textDocument/hover`).
    pub method: &'a str,
    /// 웹뷰가 위치를 계산한 문서 버전. 요청을 보낼 때의 문서와 다르면 결과에 `stale_version`.
    pub expected_version: Option<u32>,
    /// 시한. 없으면 기본(10 s).
    pub timeout: Option<Duration>,
}

/// 요청을 보낼 때의 문서(위치 변환에 쓴다).
#[derive(Debug, Clone)]
pub struct DocView {
    pub uri: String,
    pub version: u32,
    pub text: Arc<DocText>,
}

impl DocView {
    /// `{ "uri": … }`.
    pub fn text_document(&self) -> Value {
        json!({ "uri": self.uri })
    }

    /// 웹뷰의 UTF-16 오프셋 → LSP 위치. 텍스트 밖이면 invalid params.
    pub fn position(&self, offset: u32) -> Result<Position, RpcError> {
        self.text.position(offset as usize).ok_or_else(|| {
            RpcError::invalid_params(format!(
                "offset {offset} is outside the document (length {})",
                self.text.len_utf16()
            ))
        })
    }

    /// LSP 위치 → 웹뷰의 UTF-16 오프셋(줄 밖은 줄 끝으로).
    pub fn offset(&self, position: Position) -> u32 {
        u32::try_from(self.text.offset(position)).unwrap_or(u32::MAX)
    }
}

/// 요청의 답.
#[derive(Debug, Clone)]
pub struct FeatureAnswer {
    pub result: Value,
    /// 답한 인스턴스(M4-12 — 편집이 건드리는 파일을 그 인스턴스가 다른 창의 텍스트로 갖고 있는지 [`LspManager::foreign_paths`] 로 본다.
    /// M4-08 — 루트 밖 문서(결정 M4-D13)가 그 문서 안의 요청을 같은 서버에 묻는다, [`LspManager::request_instance`]).
    pub key: InstanceKey,
    /// 답한 서버 id.
    pub server: String,
    /// 요청 사이에 문서가 바뀌었다 — 위치를 그대로 쓰면 어긋날 수 있다.
    pub stale_version: bool,
    /// 요청을 보낼 때의 문서(결과의 위치를 오프셋으로 바꾼다).
    pub doc: DocView,
}

/// 띄울 준비가 된 것.
struct Prepared {
    spec: LaunchSpec,
    config: ServerConfig,
    initialization_options: Value,
    /// 설치본에서 찾았으면 그 Mason 패키지.
    installed: Option<String>,
}

/// 파일 감시 전달의 태스크가 관리자에게 묻는 손잡이(그 인스턴스 · 시도).
struct FeedLink {
    manager: LspManager,
    key: InstanceKey,
    generation: u64,
}

impl FeedHost for FeedLink {
    async fn gate(&self) -> Gate {
        let Some(host) = self.manager.inner.host.get() else {
            return Gate::Gone;
        };
        let projects = host.parts.projects.clone();
        let project = self.key.project_id.clone();
        // 보내기 묶음마다 신뢰를 다시 본다 — 신뢰 해제는 서버를 끝내지만(`recheck_trust`) 그 알림을 받기 전의 틈에도 보내지 않게.
        let trusted = tokio::task::spawn_blocking(move || projects.is_trusted(&project))
            .await
            .unwrap_or(false);
        if !trusted {
            return Gate::Hold;
        }
        let state = lock(&self.manager.inner.state);
        match state.instances.get(&self.key) {
            Some(instance)
                if instance.generation == self.generation && instance.state.is_running() =>
            {
                Gate::Open(instance.synced.keys().cloned().collect())
            }
            _ => Gate::Gone,
        }
    }
}

/// 해석 · 서버 런타임(블로킹). 띄울 수 없으면 그 상태.
fn prepare(servers: &LspServers, key: &InstanceKey) -> Result<Prepared, LspInstanceState> {
    let mut setup = servers
        .setup(
            &key.project_id,
            key.worktree.as_deref(),
            &key.server,
            &key.root,
        )
        .map_err(|e| LspInstanceState::Failed { message: e.message })?;
    let (program, installed) = match &setup.resolution {
        LspResolution::Found { path, step, .. } => (
            path.clone(),
            (*step == LspResolveStep::Installed)
                .then(|| setup.definition.mason.clone())
                .flatten(),
        ),
        LspResolution::Disabled => return Err(LspInstanceState::Disabled),
        LspResolution::NotInstalled { mason, .. } => {
            return Err(LspInstanceState::NotInstalled {
                mason: mason.clone(),
            });
        }
        LspResolution::InvalidPath { path, reason, .. } => {
            return Err(LspInstanceState::Failed {
                message: format!("the configured path {path} cannot be used: {reason}"),
            });
        }
    };
    if !setup.config.enabled {
        return Err(LspInstanceState::Disabled);
    }
    for req in setup.definition.requires.clone() {
        match find_runtime(&req, &setup.env) {
            Some(found) => setup.with_runtime(&found),
            None => {
                return Err(LspInstanceState::RuntimeMissing {
                    tool: runtime_name(req.runtime).to_owned(),
                    needed: req.version.to_string(),
                });
            }
        }
    }
    Ok(Prepared {
        spec: LaunchSpec {
            server_id: key.server.clone(),
            program,
            args: setup.config.args.iter().map(Into::into).collect(),
            cwd: key.root.clone(),
            env: setup.env.clone(),
            extra_env: BTreeMap::new(),
        },
        initialization_options: setup.initialization_options.clone(),
        config: setup.config,
        installed,
    })
}

/// 이미 있는 인스턴스에 문서를 여는 김에 다시 띄워 볼 상태인가 — 띄우지 못한 까닭(미신뢰 · 미설치 · 런타임 없음 · 꺼짐 · 실패)이
/// 그새 풀렸을 수 있다. 뜨는 중 · 떠 있음 · 사람이 멈춤 · 죽음 반복(`crashed`)은 아니다.
fn retry_on_open(state: &LspInstanceState) -> bool {
    !state.is_running()
        && !matches!(
            state,
            LspInstanceState::Starting | LspInstanceState::Crashed | LspInstanceState::Stopped
        )
}

fn status_of(key: &InstanceKey, instance: &Instance) -> LspInstanceStatus {
    LspInstanceStatus {
        server: key.server.clone(),
        root: key.root.display().to_string(),
        state: instance.state.clone(),
        progress: instance.progress.latest(),
    }
}

/// 로그 파일 이름의 인스턴스 칸: 프로젝트 id 와 루트의 짧은 해시(같은 루트면 다시 띄워도 같은 파일).
fn instance_name(key: &InstanceKey) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.root.hash(&mut hasher);
    key.worktree.hash(&mut hasher);
    format!("{}-{:08x}", key.project_id, hasher.finish() as u32)
}

fn sync_of(caps: &lsp_types::ServerCapabilities) -> SyncKind {
    match &caps.text_document_sync {
        None => SyncKind {
            change: TextDocumentSyncKind::NONE,
            save: None,
        },
        Some(TextDocumentSyncCapability::Kind(kind)) => SyncKind {
            change: *kind,
            save: None,
        },
        Some(TextDocumentSyncCapability::Options(options)) => SyncKind {
            change: options.change.unwrap_or(TextDocumentSyncKind::NONE),
            save: options.save.as_ref().map(|save| match save {
                TextDocumentSyncSaveOptions::Supported(_) => false,
                TextDocumentSyncSaveOptions::SaveOptions(o) => o.include_text.unwrap_or(false),
            }),
        },
    }
}

fn did_open(server: &LanguageServer, doc: &Doc) {
    let _ = server.notify_raw(
        "textDocument/didOpen",
        json!({ "textDocument": {
            "uri": doc.uri,
            "languageId": doc.language_id,
            "version": doc.version,
            "text": doc.text.as_str(),
        }}),
    );
}

/// 문서를 그 서버에 연다(`Synced`). 그 URI 가 이미 열려 있으면(다른 연결의 문서) `didOpen` 없이 세고 서버의 텍스트를 이 문서로
/// 맞춘다 — 서버는 같은 URI 의 `didOpen` 을 겹쳐 받지 않는다(M4-05 리뷰 M2).
fn sync_open(
    server: &LanguageServer,
    instance: &mut Instance,
    doc_id: &str,
    doc: &Doc,
    docs: &HashMap<String, Doc>,
) {
    let sync = instance.sync;
    match instance.synced.get_mut(&doc.uri) {
        Some(synced) => {
            synced.refs.insert(doc_id.to_owned());
            take_over(server, sync, synced, doc_id, doc, docs);
        }
        None => {
            did_open(server, doc);
            let synced = Synced {
                owner: doc_id.to_owned(),
                refs: BTreeSet::from([doc_id.to_owned()]),
                version: i64::from(doc.version),
                hook: instance.sync_hook.clone(),
            };
            synced.sent(&doc.uri);
            instance.synced.insert(doc.uri.clone(), synced);
        }
    }
}

/// 서버의 텍스트를 문서 `doc_id` 의 것으로 맞춘다(주인을 바꾼다). 앞 주인과 텍스트가 같으면 보내지 않는다.
fn take_over(
    server: &LanguageServer,
    sync: SyncKind,
    synced: &mut Synced,
    doc_id: &str,
    doc: &Doc,
    docs: &HashMap<String, Doc>,
) {
    if synced.owner == doc_id {
        return;
    }
    let same = docs
        .get(&synced.owner)
        .is_some_and(|owner| owner.text.as_str() == doc.text.as_str());
    synced.owner = doc_id.to_owned();
    if !same {
        full_change(server, sync, synced, doc);
    }
}

/// 문서의 전체 텍스트를 `didChange` 로 보낸다(`Incremental` 서버도 범위 없는 변경을 받는다). 동기화 `None` 서버에는 보내지 않는다.
fn full_change(server: &LanguageServer, sync: SyncKind, synced: &mut Synced, doc: &Doc) {
    if sync.change == TextDocumentSyncKind::NONE {
        return;
    }
    let version = synced.next_version(doc.version);
    let _ = server.notify_raw(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": doc.uri, "version": version },
            "contentChanges": [{ "text": doc.text.as_str() }],
        }),
    );
    synced.sent(&doc.uri);
}

/// 문서의 편집들(버전, 증분 변경)을 그 문서를 연 서버들에 보낸다(잠금 안 — 머리 주석 「문서 동기화」). `Full` 서버에는 마지막
/// 버전의 전체 텍스트 한 번. 서버의 텍스트가 다른 연결의 문서 것이었으면(`Synced::owner`) 이 문서의 전체 텍스트로 맞춘다.
fn send_change(
    doc_id: &str,
    doc: &Doc,
    instances: &mut BTreeMap<InstanceKey, Instance>,
    batches: &[(u32, Vec<lsp_types::TextDocumentContentChangeEvent>)],
) {
    for key in &doc.instances {
        let Some(instance) = instances.get_mut(key) else {
            continue;
        };
        let Some(server) = instance.server.clone() else {
            continue;
        };
        let sync = instance.sync;
        let Some(synced) = instance
            .synced
            .get_mut(&doc.uri)
            .filter(|s| s.refs.contains(doc_id))
        else {
            continue;
        };
        if synced.owner != doc_id {
            synced.owner = doc_id.to_owned();
            full_change(&server, sync, synced, doc);
            continue;
        }
        if sync.change == TextDocumentSyncKind::INCREMENTAL {
            for (version, events) in batches {
                let version = synced.next_version(*version);
                let _ = server.notify_raw(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": doc.uri, "version": version },
                        "contentChanges": serde_json::to_value(events).unwrap_or(Value::Null),
                    }),
                );
            }
            synced.sent(&doc.uri);
        } else {
            full_change(&server, sync, synced, doc);
        }
    }
}

/// 프로젝트(활성 워크트리면 그 안)의 루트 실제 경로 — `lsp_grant` 와 같은 계산(미신뢰여도 인스턴스 열쇠를 만든다).
fn project_root(
    projects: &Registry,
    project_id: &str,
    worktree: Option<&str>,
) -> Result<PathBuf, RpcError> {
    let project = projects
        .get(project_id)
        .map_err(|e| RpcError::new(rpc_code::NOT_FOUND, e.to_string()))?;
    let root = match worktree {
        None => project.root().to_path_buf(),
        Some(top) => worktree_root(project.root(), Path::new(top)).map_err(|_| {
            RpcError::invalid_params(format!("`{top}` is not a worktree of `{project_id}`"))
        })?,
    };
    real_path(&root).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "cannot resolve the project root {}",
            root.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 리뷰 L5: 같은 워크트리를 다르게 적어도(끝의 `/`) 루트(실제 경로)가 같으면 같은 인스턴스다. 루트가 다르면 다르다.
    #[test]
    fn instance_keys_ignore_how_the_worktree_was_written() {
        let key = |worktree: Option<&str>, root: &str| InstanceKey {
            project_id: "p".into(),
            worktree: worktree.map(str::to_owned),
            server: "rust-analyzer".into(),
            root: PathBuf::from(root),
        };
        let mut instances = BTreeMap::new();
        instances.insert(key(Some("/x/wt"), "/x/wt/crate"), 1);
        assert_eq!(instances.get(&key(Some("/x/wt/"), "/x/wt/crate")), Some(&1));
        assert_eq!(instances.get(&key(None, "/x/wt/crate")), Some(&1));
        assert_eq!(instances.get(&key(Some("/x/wt"), "/x/main/crate")), None);
        let mut set = std::collections::HashSet::new();
        set.insert(key(Some("/x/wt"), "/x/wt/crate"));
        assert!(set.contains(&key(Some("/x/wt/"), "/x/wt/crate")));
    }
}
