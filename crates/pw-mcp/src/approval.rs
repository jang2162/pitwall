//! 승인 요청의 라우팅(16-m5-plan.md M5-12 ③ ④, 결정 M5-D29 · M5-D5 — 05-mcp.md 「감사 로그」 의 「승인 요청 라우팅」).
//!
//! 창은 떠 있는 동안 `mcp.approvals { projectId? }` 스트림을 열어 둔다([`Approvals::listen`] — `git.prompts` 와 같은 꼴). 세션이
//! 승인을 청하면([`Approvals::ask`]) 허브가 요청을 보일 창을 고르고, 창은 다이얼로그를 띄워 `mcp.approval.answer` 로 답한다
//! ([`Approvals::answer`]). 답 · 시한 초과 · 묻던 쪽이 그만둠(세션 끊김 · 호출 취소 — future 를 버림)이면 요청을 거두고
//! 보이던 창에 닫힘([`McpApprovalEvent::Closed`])을 보낸다.
//!
//! - **프로젝트 요청**은 그 프로젝트 창(연결 범위가 그 프로젝트 — `connection_may_write` 와 같은 판정)에만 보이고 그 창만 답한다.
//!   듣는 그 프로젝트 창이 없으면 **기다리지 않고** 거절한다([`AskError::NoWindow`]). 묻는 도중 마지막 창이 떠나면
//!   [`AskError::WindowLeft`].
//! - **프로젝트 없는 요청**(`project_register` · `open_with_upsert`)은 가장 최근에 앞에 있던 창 하나에 보인다([`Approvals::focused`]
//!   — 앱이 창 포커스를 알린다. 포커스를 모르면 가장 최근에 듣기 시작한 창). 요청에 「이 프로젝트의 것이 아니다」 표지
//!   (`notThisProject`)가 실린다. 그 창의 연결만 답한다. 그 창이 떠나면 다음 창으로 옮기고, 창이 하나도 없으면 거절한다.
//! - **CLI 허용**(결정 M5-D5): CLI 요청에 [`McpApprovalAnswer::Window`] 로 답하면 「그 프로젝트 창이 열려 있는 동안 CLI 로 온
//!   호출 전부」 에 그 툴(계획이 열쇠를 정하면 그 툴의 그 열쇠 — [`Allowance`])을 묻지 않는다([`Approvals::cli_allowed`]). 그
//!   프로젝트를 듣는 창이 모두 떠나면 사라진다.
//! - **프로젝트 없는 요청의 후보는 창의 연결뿐이다** — 창 식별자([`Approvals::listen`] 의 `window`)가 없는 연결(창 없는 Host
//!   범위 연결 등)에는 보내지 않는다. 창이 사라지면 앱이 [`Approvals::window_closed`] 로 포커스 순서에서 뺀다.
//! - 한 창의 대기는 id 순서로 보인다(화면이 차례로 띄운다).
//! - **세션당 대기 상한**([`MAX_PENDING_PER_SESSION`] — 16-m5-plan M5-14): 한 MCP 세션의 대기가 상한에 닿아 있으면 새 요청은
//!   창에 보이지 않고 곧바로 거절된다([`AskError::Backlog`]). 에이전트가 승인 요청을 쌓아 화면과 시한 대기를 채우지 못하게 —
//!   앞의 요청 하나가 끝나면 다시 받는다. 다른 세션에는 닿지 않는다.
//! - 앱이 끝날 때 [`Approvals::shutdown`] 이 대기를 모두 거둔다.
//!
//! - **붙은 사람이 없는 Host**(18-m7-plan M7-20 — 결정 M7-D10 ④): 창이 없는 Host(Relay)는 [`Approvals::require_attendance`] 로
//!   「붙은 owner Desktop 이 있는가」 를 건다. 그것이 거짓인 동안 세션은 `write` 이상을 헤드리스처럼 거절한다 — `auto_approve` ·
//!   정책 자동도 돌지 않는다([`Approvals::attended`], 판정은 `crate::policy::decide` · `crate::server`). 걸지 않은 Host(Desktop)는 늘 참이다.
//!
//! 에이전트 범위 연결(MCP 세션)은 듣지도 답하지도 못한다 — `mcp.*` 의 문(`methods::WindowsOnly`)이 거절한다. 에이전트가 자기
//! 요청에 스스로 답하는 길이 없다.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pw_core::registry::plan::ToolPlan;
use pw_core::registry::state::scope_may_write;
use pw_rpc::{ConnectionScope, SessionId, Tier};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

/// 사람의 답.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum McpApprovalAnswer {
    /// [거절].
    Deny,
    /// [이번만 허용].
    Once,
    /// [이 세션 동안 허용] — 그 세션 · 그 프로젝트 · 그 툴(계획이 열쇠를 정하면 그 툴의 그 열쇠).
    Session,
    /// CLI 요청의 [이 프로젝트 창이 열려 있는 동안 CLI 허용](결정 M5-D5).
    Window,
    /// 다른 머신을 겨눈 `write` · `exec` 의 [이 세션 동안 이 머신 허용](18-m7-plan M7-18 — D57 ⑤). 그 세션의 그 머신 `write` ·
    /// `exec` 에 닿고, 미신뢰 대상 · `dangerous` · 사람이 띄운 실행 · 호출마다 청하는 승인에는 닿지 않는다. CLI 요청이면
    /// [이 프로젝트 창이 열려 있는 동안 CLI 에 이 머신 허용](D61 의 꼴)이다.
    Machine,
}

/// 창에 보이는 승인 요청.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpApprovalRequest {
    /// 이 앱 안에서 요청마다 다르다. 답할 때 쓴다.
    #[specta(type = pw_rpc::JsNumber)]
    pub id: u64,
    /// 대상 프로젝트. 프로젝트 없는 쓰기면 `null`.
    pub project_id: Option<String>,
    /// 다른 머신을 겨눈 호출이면 그 머신 id(`checkout` 입력 — 18-m7-plan M7-18). 이 머신이면 `null`. 화면은 이 id 를 Desktop 의
    /// 별명으로 보인다(Relay 가 댄 이름이 아니다 — M7-P19).
    pub machine: Option<String>,
    /// 이 요청은 받은 창의 프로젝트 것이 아니다(프로젝트 없는 요청 — 가장 최근에 앞에 있던 창에 뜬다. 결정 M5-D29).
    pub not_this_project: bool,
    /// 요청한 MCP 세션(감사 행의 `session`).
    pub session: String,
    /// 요청 에이전트(`이름 버전`). 에이전트가 적은 글이다 — 화면이 거른다.
    pub client: Option<String>,
    /// CLI(`pitwall-cli`)로 온 호출이다 — 셋째 답이 [`McpApprovalAnswer::Window`] 다.
    pub cli: bool,
    pub tool: String,
    /// 툴의 선언 등급.
    pub tier: Tier,
    /// 툴이 만든 계획(요약 · 바뀔 것 · 해시).
    pub plan: ToolPlan,
    /// 이 요청이 받는 답. `dangerous` · 기억하지 않는 계획(`open_with_upsert`)은 [거절] · [이번만 허용] 둘이다.
    pub answers: Vec<McpApprovalAnswer>,
    /// 시한(초).
    #[specta(type = pw_rpc::JsNumber)]
    pub timeout_secs: u64,
    /// 시한이 끝나는 시각(Unix 밀리초).
    #[specta(type = pw_rpc::JsNumber)]
    pub expires_at: i64,
}

/// `mcp.approvals` 스트림의 항목.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum McpApprovalEvent {
    /// 새 요청(또는 듣기 시작할 때 아직 대기 중인 요청).
    Open { request: Box<McpApprovalRequest> },
    /// 그 요청을 거뒀다(답 · 시한 초과 · 세션 끊김 · 호출 취소 · 앱 종료). 떠 있는 다이얼로그를 내린다.
    Closed {
        #[specta(type = pw_rpc::JsNumber)]
        id: u64,
    },
}

/// 허용이 기억하는 단위(프로젝트를 뺀 부분): 툴 이름과 계획이 정한 열쇠(`Remember::Key` — 없으면 그 툴 전부)와 계획 등급. 툴
/// 이름이 늘 들어 있어 한 툴의 허용이 같은 열쇠를 내는 다른 툴에 닿지 않는다. 계획 등급이 들어 있어 계획 등급을 낮추는 툴
/// (`catalog::LOWERED_PLAN_TIERS`)의 낮은 계획에 준 허용이 같은 툴의 높은 계획에 닿지 않는다(17-m6-plan M6-10 — 등급이 같을 때만
/// 닿는다).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Allowance {
    pub tool: String,
    pub key: Option<String>,
    pub tier: Tier,
}

/// 세션이 청하는 것. [`Approvals::ask`].
#[derive(Debug, Clone)]
pub struct ApprovalAsk {
    pub project_id: Option<String>,
    /// 다른 머신을 겨눈 호출이면 그 머신 id([`McpApprovalRequest::machine`]).
    pub machine: Option<String>,
    pub session: String,
    pub client: Option<String>,
    pub cli: bool,
    pub tool: String,
    pub tier: Tier,
    pub plan: ToolPlan,
    pub answers: Vec<McpApprovalAnswer>,
    /// [`McpApprovalAnswer::Window`] 로 답하면 기억할 것.
    pub remember: Option<Allowance>,
    /// CLI 요청을 [`McpApprovalAnswer::Machine`] 으로 답하면 기억할 머신 id(창이 열려 있는 동안 CLI 에 이 머신 허용).
    pub remember_machine: Option<String>,
    pub timeout: Duration,
}

/// 한 MCP 세션(감사 행의 `session`)이 한꺼번에 기다리게 할 수 있는 승인 요청 수. 넘는 요청은 [`AskError::Backlog`].
pub const MAX_PENDING_PER_SESSION: usize = 8;

/// 답 없이 끝난 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskError {
    /// 그 세션의 대기가 이미 [`MAX_PENDING_PER_SESSION`] 개다 — 창에 보이지 않았다.
    Backlog,
    /// 요청을 보일 창이 없다(그 프로젝트 창 · 프로젝트 없는 요청이면 아무 창).
    NoWindow,
    /// 묻는 도중 답할 창이 모두 떠났다.
    WindowLeft,
    /// 시한 안에 답이 없었다.
    TimedOut,
    /// 앱이 끝난다.
    Shutdown,
}

/// 답을 받지 않은 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerError {
    /// 그런 대기 요청이 없다(이미 답했거나 거뒀다).
    NotFound,
    /// 그 요청에 답할 창의 연결이 아니다.
    NotYours,
    /// 그 요청이 받지 않는 답이다.
    NotOffered,
}

/// 창의 연결 하나(Host 의 RPC 세션). 프로젝트 없는 요청은 그것을 받은 연결만 답한다.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Connection(String);

impl Connection {
    /// 그 RPC 세션.
    pub fn of(session: SessionId) -> Self {
        Self(format!("{session:?}"))
    }

    /// 이름으로(시험 · 핸들러 밖).
    pub fn named(name: &str) -> Self {
        Self(name.to_owned())
    }
}

/// 답하는 연결.
#[derive(Debug, Clone)]
pub struct Answerer<'a> {
    pub scope: &'a ConnectionScope,
    pub connection: Connection,
}

/// 승인 허브. 복제하면 같은 표다.
#[derive(Clone, Default)]
pub struct Approvals {
    inner: Arc<Shared>,
}

/// 「붙은 사람이 있는가」([`Approvals::require_attendance`]).
pub type Attendance = Arc<dyn Fn() -> bool + Send + Sync>;

/// 프로젝트 없는 요청을 보일 Desktop 고르기([`Approvals::choose_front_by`]): 마지막으로 사람 입력이 있던 Desktop 의 기기 id. 모르면
/// `None`(그 요청은 거절된다).
pub type FrontDesktop = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>
        + Send
        + Sync,
>;

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// 걸리지 않았으면 늘 붙은 것으로 본다(Desktop — 창이 이 프로세스 안에 있다).
    attendance: Mutex<Option<Attendance>>,
    /// 걸리지 않았으면 프로젝트 없는 요청은 포커스 순서로 고른다(Desktop).
    front_desktop: Mutex<Option<FrontDesktop>>,
    /// 승인을 지나는 중인 호출 수([`Approvals::hold`]) — 끝낼 때 그 호출들이 감사 기록을 큐에 넣을 때까지 기다린다.
    holding: AtomicUsize,
}

#[derive(Default)]
struct State {
    next_request: u64,
    next_listener: u64,
    next_focus: u64,
    listeners: BTreeMap<u64, Listening>,
    /// 창 식별자(웹뷰 label) → 앞에 온 순번.
    focus: HashMap<String, u64>,
    pending: BTreeMap<u64, Pending>,
    /// CLI 허용: (프로젝트, 열쇠).
    cli: BTreeSet<(String, Allowance)>,
    /// CLI 의 머신 허용: (프로젝트, 머신 id).
    cli_machines: BTreeSet<(String, String)>,
    closed: bool,
}

struct Listening {
    project: Option<String>,
    window: Option<String>,
    connection: Connection,
    tx: mpsc::UnboundedSender<McpApprovalEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Route {
    /// 그 프로젝트를 듣는 창 전부.
    Project(String),
    /// 그 듣기 하나(프로젝트 없는 요청).
    Listener(u64),
}

struct Pending {
    request: McpApprovalRequest,
    route: Route,
    remember: Option<Allowance>,
    remember_machine: Option<String>,
    reply: Option<oneshot::Sender<Result<McpApprovalAnswer, AskError>>>,
}

impl State {
    fn live(&self) -> impl Iterator<Item = (&u64, &Listening)> {
        self.listeners.iter().filter(|(_, l)| !l.tx.is_closed())
    }

    fn hears(&self, project: &str) -> bool {
        self.live()
            .any(|(_, l)| l.project.as_deref() == Some(project))
    }

    /// 프로젝트 없는 요청을 보일 창: 가장 최근에 앞에 온 창의 가장 최근 듣기, 포커스를 모르면 가장 최근 듣기. 창 식별자가 없는
    /// 연결은 후보가 아니다. Relay 에서는 창 식별자가 붙은 Desktop 의 label(`<기기 id>/<label>` — `pw_link::link_window`)이고 창
    /// 이벤트가 오지 않아([`Approvals::focused`] 를 부르는 자리가 없다) 늘 「가장 최근 듣기」 다(18-m7-plan M7-11).
    fn front(&self) -> Option<u64> {
        self.live()
            .filter(|(_, l)| l.window.is_some())
            .max_by_key(|(id, l)| {
                let focus = l
                    .window
                    .as_ref()
                    .and_then(|w| self.focus.get(w))
                    .copied()
                    .unwrap_or(0);
                (focus, **id)
            })
            .map(|(id, _)| *id)
    }

    /// 기기 `device` 의 창(링크의 label `<기기 id>/<label>`)으로 듣는 것 가운데 가장 최근 것.
    fn front_of(&self, device: &str) -> Option<u64> {
        self.live()
            .filter(|(_, l)| {
                l.window
                    .as_deref()
                    .and_then(|w| w.rsplit_once('/'))
                    .is_some_and(|(d, _)| d == device)
            })
            .map(|(id, _)| *id)
            .max()
    }

    fn send(&mut self, route: &Route, event: &McpApprovalEvent) {
        match route {
            Route::Project(project) => {
                for l in self.listeners.values() {
                    if l.project.as_deref() == Some(project.as_str()) {
                        let _ = l.tx.send(event.clone());
                    }
                }
            }
            Route::Listener(id) => {
                if let Some(l) = self.listeners.get(id) {
                    let _ = l.tx.send(event.clone());
                }
            }
        }
    }
}

impl Approvals {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// 창 하나가 듣기 시작한다. `project` 는 그 창의 프로젝트(프로젝트 창이면 — 범위 판정은 부르는 쪽이 했다), `window` 는 창
    /// 식별자(포커스 순서에 쓴다), `connection` 은 그 연결(프로젝트 없는 요청에 답할 수 있는지 가른다). 아직 대기 중인 그 창의
    /// 요청이 먼저 온다. 돌려준 것을 버리면 듣기를 그만둔다.
    pub fn listen(
        &self,
        project: Option<&str>,
        window: Option<&str>,
        connection: Connection,
    ) -> Listener {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.lock();
        state.next_listener += 1;
        let id = state.next_listener;
        for pending in state.pending.values() {
            if matches!(&pending.route, Route::Project(p) if Some(p.as_str()) == project) {
                let _ = tx.send(McpApprovalEvent::Open {
                    request: Box::new(pending.request.clone()),
                });
            }
        }
        state.listeners.insert(
            id,
            Listening {
                project: project.map(str::to_owned),
                window: window.map(str::to_owned),
                connection,
                tx,
            },
        );
        Listener {
            approvals: self.clone(),
            id,
            rx,
        }
    }

    /// 창이 없는 Host(Relay)가 「붙은 owner Desktop 이 있는가」 를 건다(결정 M7-D10 ④). 거짓인 동안 세션은 `write` 이상을 헤드리스처럼
    /// 거절한다 — 승인을 물을 사람이 없는 동안 `auto_approve` · 정책 자동으로 쓰기가 돌지 않게(로컬의 D61 — 앱이 꺼져 있으면 코어 툴만).
    pub fn require_attendance(&self, attended: Attendance) {
        *self
            .inner
            .attendance
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(attended);
    }

    /// 붙은 사람이 있는가. [`Self::require_attendance`] 를 걸지 않았으면 참.
    pub fn attended(&self) -> bool {
        let attendance = self
            .inner
            .attendance
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        attendance.is_none_or(|attended| attended())
    }

    /// 창이 없는 Host(Relay)가 프로젝트 없는 요청의 행선지를 「마지막으로 사람 입력이 있던 owner Desktop」 으로 건다(결정 M7-D3 —
    /// 열기 요청과 같은 길, `pw_core::open::desks`). 건 뒤에는 요청마다 그 Desktop 의 기기 id 를 묻고, 창 label 이 그 기기의 것
    /// (`<기기 id>/<label>` — 링크의 label)인 듣기 가운데 가장 최근 것 하나에 보인다. 모르거나 그 Desktop 에 듣는 창이 없으면 거절이다
    /// — 다른 Desktop 으로 떨어지지 않는다. 입력 시각은 Desktop 이 내고 Relay 는 비교만 한다.
    pub fn choose_front_by(&self, front: FrontDesktop) {
        *self
            .inner
            .front_desktop
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(front);
    }

    /// 창 `window` 가 앞으로 왔다(앱의 창 이벤트).
    pub fn focused(&self, window: &str) {
        let mut state = self.lock();
        state.next_focus += 1;
        let seq = state.next_focus;
        state.focus.insert(window.to_owned(), seq);
    }

    /// 창 `window` 가 사라졌다 — 포커스 순서에서 뺀다.
    pub fn window_closed(&self, window: &str) {
        self.lock().focus.remove(window);
    }

    /// 그 프로젝트 창이 열려 있는 동안의 CLI 허용이 있는가.
    pub fn cli_allowed(&self, project: &str, allowance: &Allowance) -> bool {
        self.lock()
            .cli
            .contains(&(project.to_owned(), allowance.clone()))
    }

    /// 그 프로젝트 창이 열려 있는 동안의 CLI 머신 허용이 있는가.
    pub fn cli_machine_allowed(&self, project: &str, machine: &str) -> bool {
        self.lock()
            .cli_machines
            .contains(&(project.to_owned(), machine.to_owned()))
    }

    /// 그 프로젝트를 듣는 창이 있는가 — 다른 머신을 겨누는 호출은 묶음 창이 열려 있어야 한다(05 「체크아웃」).
    pub fn hears(&self, project: &str) -> bool {
        self.lock().hears(project)
    }

    /// 대기 중인 요청 수.
    pub fn pending(&self) -> usize {
        self.lock().pending.len()
    }

    /// 듣는 창(연결) 수.
    pub fn listeners(&self) -> usize {
        self.lock().live().count()
    }

    /// 창에 묻고 답을 기다린다. 보일 창이 없으면 기다리지 않는다. 이 future 를 버리면 요청을 거둔다(창에 닫힘이 간다).
    pub async fn ask(&self, ask: ApprovalAsk) -> Result<McpApprovalAnswer, AskError> {
        // 프로젝트 없는 요청이고 행선지를 고르는 길이 걸렸으면(Relay) 그 Desktop 을 먼저 묻는다 — 묻는 동안 잠그지 않는다.
        let chooser = match ask.project_id {
            Some(_) => None,
            None => self
                .inner
                .front_desktop
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        };
        let desktop = match chooser {
            Some(choose) => Some(choose().await.ok_or(AskError::NoWindow)?),
            None => None,
        };
        let (rx, _closing) = {
            let mut state = self.lock();
            if state.closed {
                return Err(AskError::Shutdown);
            }
            let route = match (&ask.project_id, &desktop) {
                (Some(project), _) if state.hears(project) => Route::Project(project.clone()),
                (Some(_), _) => return Err(AskError::NoWindow),
                (None, Some(device)) => {
                    Route::Listener(state.front_of(device).ok_or(AskError::NoWindow)?)
                }
                (None, None) => Route::Listener(state.front().ok_or(AskError::NoWindow)?),
            };
            let waiting = state
                .pending
                .values()
                .filter(|p| p.reply.is_some() && p.request.session == ask.session)
                .count();
            if waiting >= MAX_PENDING_PER_SESSION {
                return Err(AskError::Backlog);
            }
            state.next_request += 1;
            let id = state.next_request;
            let request = McpApprovalRequest {
                id,
                not_this_project: ask.project_id.is_none(),
                project_id: ask.project_id,
                machine: ask.machine,
                session: ask.session,
                client: ask.client,
                cli: ask.cli,
                tool: ask.tool,
                tier: ask.tier,
                plan: ask.plan,
                answers: ask.answers,
                timeout_secs: ask.timeout.as_secs(),
                expires_at: expires_at(ask.timeout),
            };
            state.send(
                &route,
                &McpApprovalEvent::Open {
                    request: Box::new(request.clone()),
                },
            );
            let (tx, rx) = oneshot::channel();
            state.pending.insert(
                id,
                Pending {
                    request,
                    route,
                    remember: ask.remember,
                    remember_machine: ask.remember_machine,
                    reply: Some(tx),
                },
            );
            (
                rx,
                Closing {
                    approvals: self.clone(),
                    id,
                },
            )
        };
        match tokio::time::timeout(ask.timeout, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(AskError::WindowLeft),
            Err(_) => Err(AskError::TimedOut),
        }
    }

    /// 창의 답. 그 요청에 답할 연결이고 그 요청이 받는 답이면 묻던 쪽에 넘긴다.
    pub fn answer(
        &self,
        by: &Answerer<'_>,
        id: u64,
        answer: McpApprovalAnswer,
    ) -> Result<(), AnswerError> {
        self.answer_seen(by, id, answer).map(|_| ())
    }

    /// [`Self::answer`] 인데, 허용(거절이 아닌 답)이 묻던 쪽에 닿았고 그 요청이 프로젝트의 것이면 그 프로젝트와 계획의 「사람이 본
    /// 구성」 단계([`pw_core::registry::plan::ToolPlan::seen`])를 돌려준다 — `mcp.approval.answer` 가 그 창의 연결 범위로 기록한다
    /// (17-m6-plan M6-14 ③). 묻던 쪽이 이미 그만뒀으면(끊긴 세션 — 돌지 않는다) 돌려주지 않는다.
    pub fn answer_seen(
        &self,
        by: &Answerer<'_>,
        id: u64,
        answer: McpApprovalAnswer,
    ) -> Result<Option<(String, Vec<pw_core::registry::plan::PlanSeen>)>, AnswerError> {
        let mut state = self.lock();
        let Some(pending) = state.pending.get(&id) else {
            return Err(AnswerError::NotFound);
        };
        let may = match &pending.route {
            Route::Project(project) => scope_may_write(by.scope, project),
            Route::Listener(listener) => state
                .listeners
                .get(listener)
                .is_some_and(|l| l.connection == by.connection),
        };
        if !may {
            return Err(AnswerError::NotYours);
        }
        if !pending.request.answers.contains(&answer) {
            return Err(AnswerError::NotOffered);
        }
        let remember = match (&pending.route, &pending.remember, answer) {
            (Route::Project(project), Some(allowance), McpApprovalAnswer::Window) => {
                Some((project.clone(), allowance.clone()))
            }
            _ => None,
        };
        let remember_machine = match (&pending.route, &pending.remember_machine, answer) {
            (Route::Project(project), Some(machine), McpApprovalAnswer::Machine) => {
                Some((project.clone(), machine.clone()))
            }
            _ => None,
        };
        // 다른 머신을 겨눈 계획의 「사람이 본 구성」 은 이 머신의 기록이 아니다 — 그 실행이 도는 머신의 구성 열쇠 · 해시다. 이 머신
        // 프로젝트(묶음의 이 머신 멤버 창)의 `run_seen` 에 쓰면 이 머신의 같은 열쇠 기록을 덮고, 털린 Relay 가 실은 해시로 이 머신
        // 구성이 정책 자동이 된다(M7-D9 — 다른 머신 호출에 정책 자동 미적용, 리뷰 F2). 그 머신의 기록은 그 머신 Relay 의 몫(M7-20).
        let seen = match (&pending.route, answer) {
            (_, McpApprovalAnswer::Deny) => None,
            _ if pending.request.machine.is_some() => None,
            (Route::Project(project), _) if !pending.request.plan.seen.is_empty() => {
                Some((project.clone(), pending.request.plan.seen.clone()))
            }
            _ => None,
        };
        let pending = state.pending.get_mut(&id).expect("위에서 찾았다");
        let reply = pending.reply.take().ok_or(AnswerError::NotFound)?;
        if reply.send(Ok(answer)).is_err() {
            // 묻던 쪽이 막 그만뒀다 — 허용을 기억하지 않는다.
            return Err(AnswerError::NotFound);
        }
        if let Some(entry) = remember {
            state.cli.insert(entry);
        }
        if let Some(entry) = remember_machine {
            state.cli_machines.insert(entry);
        }
        Ok(seen)
    }

    /// 승인을 지나는 호출 하나 — 버릴 때까지 [`Self::shutdown_and_wait`] 가 기다린다.
    pub fn hold(&self) -> Hold {
        self.inner.holding.fetch_add(1, Ordering::SeqCst);
        Hold {
            approvals: self.clone(),
        }
    }

    /// 앱이 끝난다: 대기를 모두 거두고(묻던 쪽은 [`AskError::Shutdown`]) 새 요청을 받지 않는다.
    pub fn shutdown(&self) {
        let mut state = self.lock();
        state.closed = true;
        for pending in state.pending.values_mut() {
            if let Some(reply) = pending.reply.take() {
                let _ = reply.send(Err(AskError::Shutdown));
            }
        }
    }

    /// [`Self::shutdown`] 뒤 승인을 지나던 호출이 모두 끝날 때까지(감사 기록을 큐에 넣을 때까지) 기다린다 — `deadline` 까지.
    /// 블로킹이다(앱 종료 길).
    pub fn shutdown_and_wait(&self, deadline: Instant) -> bool {
        self.shutdown();
        while self.inner.holding.load(Ordering::SeqCst) > 0 {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }
}

/// [`Approvals::hold`] 의 가드.
pub struct Hold {
    approvals: Approvals,
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.approvals.inner.holding.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 요청이 끝나면(답 · 시한 · future 를 버림) 표에서 빼고 보이던 창에 닫힘을 보낸다.
struct Closing {
    approvals: Approvals,
    id: u64,
}

impl Drop for Closing {
    fn drop(&mut self) {
        let mut state = self.approvals.lock();
        if let Some(pending) = state.pending.remove(&self.id) {
            state.send(&pending.route, &McpApprovalEvent::Closed { id: self.id });
        }
    }
}

/// 한 창의 듣기. 버리면 빠진다.
pub struct Listener {
    approvals: Approvals,
    id: u64,
    rx: mpsc::UnboundedReceiver<McpApprovalEvent>,
}

impl Listener {
    pub async fn recv(&mut self) -> Option<McpApprovalEvent> {
        self.rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<McpApprovalEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let mut state = self.approvals.lock();
        let Some(left) = state.listeners.remove(&self.id) else {
            return;
        };
        // 그 프로젝트를 듣는 창이 모두 떠났다 — 그 프로젝트의 대기에 답할 창이 없고 CLI 허용도 사라진다.
        if let Some(project) = &left.project
            && !state.hears(project)
        {
            state.cli.retain(|(p, _)| p != project);
            state.cli_machines.retain(|(p, _)| p != project);
            for pending in state.pending.values_mut() {
                if pending.route == Route::Project(project.clone())
                    && let Some(reply) = pending.reply.take()
                {
                    let _ = reply.send(Err(AskError::WindowLeft));
                }
            }
        }
        // 이 창에 보이던 프로젝트 없는 요청은 다음 창으로 옮긴다. 창이 없으면 거절.
        let moved: Vec<u64> = state
            .pending
            .iter()
            .filter(|(_, p)| p.route == Route::Listener(self.id))
            .map(|(id, _)| *id)
            .collect();
        for id in moved {
            match state.front() {
                Some(next) => {
                    let route = Route::Listener(next);
                    let pending = state.pending.get_mut(&id).expect("위에서 골랐다");
                    pending.route = route.clone();
                    let event = McpApprovalEvent::Open {
                        request: Box::new(pending.request.clone()),
                    };
                    state.send(&route, &event);
                }
                None => {
                    let pending = state.pending.get_mut(&id).expect("위에서 골랐다");
                    if let Some(reply) = pending.reply.take() {
                        let _ = reply.send(Err(AskError::WindowLeft));
                    }
                }
            }
        }
    }
}

fn expires_at(timeout: Duration) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    i64::try_from(now + timeout.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> ToolPlan {
        ToolPlan::builder("test.write", Tier::Write, &serde_json::json!({})).build()
    }

    fn ask(project: Option<&str>, cli: bool) -> ApprovalAsk {
        ApprovalAsk {
            project_id: project.map(str::to_owned),
            machine: None,
            remember_machine: None,
            session: "s1".into(),
            client: Some("agent 1".into()),
            cli,
            tool: "fake_write".into(),
            tier: Tier::Write,
            plan: plan(),
            answers: vec![
                McpApprovalAnswer::Deny,
                McpApprovalAnswer::Once,
                if cli {
                    McpApprovalAnswer::Window
                } else {
                    McpApprovalAnswer::Session
                },
            ],
            remember: Some(fake_write()),
            timeout: Duration::from_secs(300),
        }
    }

    fn fake_write() -> Allowance {
        Allowance {
            tool: "fake_write".into(),
            key: None,
            tier: Tier::Write,
        }
    }

    /// 실패가 멈춤이 아니라 실패로 나오게 시한을 둔다.
    async fn within<F: Future>(f: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(10), f)
            .await
            .expect("timed out")
    }

    fn conn(n: u64) -> Connection {
        Connection::named(&n.to_string())
    }

    async fn opened(listener: &mut Listener) -> McpApprovalRequest {
        match within(listener.recv()).await {
            Some(McpApprovalEvent::Open { request }) => *request,
            other => panic!("{other:?}"),
        }
    }

    /// 리뷰 F2: 다른 머신을 겨눈 계획의 「사람이 본 구성」 은 허용해도 돌려주지 않는다 — 이 머신 프로젝트의 `run_seen` 에 쓰이지
    /// 않는다. 이 머신 계획은 그대로 돌려준다(대조).
    #[tokio::test]
    async fn a_remote_plans_seen_configuration_is_not_recorded_here() {
        let approvals = Approvals::default();
        let scope = ConnectionScope::Project("p".into());
        let by = Answerer {
            scope: &scope,
            connection: conn(1),
        };
        let mut listener = approvals.listen(Some("p"), Some("w"), conn(1));
        for machine in [None, Some("SHA256:mini")] {
            let mut request = ask(Some("p"), false);
            request.machine = machine.map(str::to_owned);
            request.plan = ToolPlan::builder("test.run", Tier::Exec, &serde_json::json!({}))
                .seen("dev", "sha256:seen")
                .build();
            let asking = tokio::spawn({
                let approvals = approvals.clone();
                async move { approvals.ask(request).await }
            });
            let shown = opened(&mut listener).await;
            let seen = approvals
                .answer_seen(&by, shown.id, McpApprovalAnswer::Once)
                .unwrap();
            match machine {
                None => assert_eq!(
                    seen.map(|(p, s)| (p, s.len())),
                    Some(("p".to_owned(), 1)),
                    "이 머신 계획은 기록한다(대조)"
                ),
                Some(_) => assert_eq!(
                    seen, None,
                    "다른 머신 계획의 seen 을 이 머신에 기록하려 했다"
                ),
            }
            assert_eq!(within(asking).await.unwrap(), Ok(McpApprovalAnswer::Once));
            let _ = within(listener.recv()).await;
        }
    }

    #[tokio::test]
    async fn no_window_of_the_project_is_an_immediate_refusal() {
        let approvals = Approvals::default();
        assert_eq!(
            within(approvals.ask(ask(Some("p"), false))).await,
            Err(AskError::NoWindow)
        );
        // 다른 프로젝트의 창 · 프로젝트 없는 창은 그 프로젝트의 요청을 받지 않는다.
        let _other = approvals.listen(Some("q"), Some("w-q"), conn(1));
        let _welcome = approvals.listen(None, Some("welcome"), conn(2));
        assert_eq!(
            within(approvals.ask(ask(Some("p"), false))).await,
            Err(AskError::NoWindow)
        );
        // 프로젝트 없는 요청은 창이 하나도 없으면 거절이다.
        let empty = Approvals::default();
        assert_eq!(empty.ask(ask(None, false)).await, Err(AskError::NoWindow));
    }

    #[tokio::test]
    async fn only_the_project_window_answers_and_the_answer_must_be_offered() {
        let approvals = Approvals::default();
        let mut window = approvals.listen(Some("p"), Some("w-p"), conn(1));
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(Some("p"), false)).await })
        };
        let request = opened(&mut window).await;
        assert_eq!(request.project_id.as_deref(), Some("p"));
        assert!(!request.not_this_project);
        let other = ConnectionScope::Project("q".into());
        let host = ConnectionScope::Host;
        let agent = ConnectionScope::Agent;
        for scope in [&other, &host, &agent] {
            assert_eq!(
                approvals.answer(
                    &Answerer {
                        scope,
                        connection: conn(1)
                    },
                    request.id,
                    McpApprovalAnswer::Once
                ),
                Err(AnswerError::NotYours),
                "{scope:?}"
            );
        }
        let mine = ConnectionScope::Project("p".into());
        let by = Answerer {
            scope: &mine,
            connection: conn(9),
        };
        assert_eq!(
            approvals.answer(&by, request.id, McpApprovalAnswer::Window),
            Err(AnswerError::NotOffered)
        );
        approvals
            .answer(&by, request.id, McpApprovalAnswer::Session)
            .unwrap();
        assert_eq!(
            within(asking).await.unwrap(),
            Ok(McpApprovalAnswer::Session)
        );
        assert_eq!(
            within(window.recv()).await,
            Some(McpApprovalEvent::Closed { id: request.id })
        );
        assert_eq!(
            approvals.answer(&by, request.id, McpApprovalAnswer::Once),
            Err(AnswerError::NotFound),
            "이미 답했다"
        );
        assert_eq!(approvals.pending(), 0);
    }

    /// 세션당 대기 상한(16-m5-plan M5-14): 그 세션의 아홉째 요청은 창에 보이지 않고 곧바로 거절되고 앞 여덟은 그대로 기다린다. 하나가
    /// 끝나면 다시 받는다. 다른 세션의 요청은 상한에 닿지 않는다.
    #[tokio::test]
    async fn a_session_cannot_pile_up_more_than_the_pending_limit() {
        let approvals = Approvals::default();
        let mut window = approvals.listen(Some("p"), Some("w-p"), conn(1));
        let mut asking = Vec::new();
        for _ in 0..MAX_PENDING_PER_SESSION {
            let approvals = approvals.clone();
            asking.push(tokio::spawn(async move {
                approvals.ask(ask(Some("p"), false)).await
            }));
        }
        let mut shown = Vec::new();
        for _ in 0..MAX_PENDING_PER_SESSION {
            shown.push(opened(&mut window).await);
        }
        assert_eq!(approvals.pending(), MAX_PENDING_PER_SESSION);
        assert_eq!(
            within(approvals.ask(ask(Some("p"), false))).await,
            Err(AskError::Backlog)
        );
        assert!(
            window.try_recv().is_none(),
            "넘친 요청은 창에 보이지 않는다"
        );
        assert_eq!(
            approvals.pending(),
            MAX_PENDING_PER_SESSION,
            "앞 여덟은 그대로다"
        );
        assert!(asking.iter().all(|t| !t.is_finished()));
        // 다른 세션은 상한과 무관하다.
        let other = {
            let approvals = approvals.clone();
            tokio::spawn(async move {
                approvals
                    .ask(ApprovalAsk {
                        session: "s2".into(),
                        ..ask(Some("p"), false)
                    })
                    .await
            })
        };
        let other_request = opened(&mut window).await;
        assert_eq!(other_request.session, "s2");
        // 하나에 답하면 그 세션이 다시 묻는다.
        let mine = ConnectionScope::Project("p".into());
        let by = Answerer {
            scope: &mine,
            connection: conn(1),
        };
        approvals
            .answer(&by, shown[0].id, McpApprovalAnswer::Deny)
            .unwrap();
        assert_eq!(
            within(asking.remove(0)).await.unwrap(),
            Ok(McpApprovalAnswer::Deny)
        );
        assert_eq!(
            within(window.recv()).await,
            Some(McpApprovalEvent::Closed { id: shown[0].id })
        );
        let again = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(Some("p"), false)).await })
        };
        let request = opened(&mut window).await;
        assert_eq!(request.session, "s1");
        approvals.shutdown();
        assert_eq!(within(again).await.unwrap(), Err(AskError::Shutdown));
        assert_eq!(within(other).await.unwrap(), Err(AskError::Shutdown));
    }

    /// 시한은 가짜 시계(tokio 의 멈춘 시간)로 본다.
    #[tokio::test(start_paused = true)]
    async fn an_unanswered_request_times_out_and_is_withdrawn() {
        let approvals = Approvals::default();
        let mut window = approvals.listen(Some("p"), None, conn(1));
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(Some("p"), false)).await })
        };
        let request = opened(&mut window).await;
        assert_eq!(request.timeout_secs, 300);
        tokio::time::advance(Duration::from_secs(299)).await;
        assert_eq!(approvals.pending(), 1);
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(within(asking).await.unwrap(), Err(AskError::TimedOut));
        assert_eq!(
            within(window.recv()).await,
            Some(McpApprovalEvent::Closed { id: request.id })
        );
    }

    /// 묻던 future 를 버리면(세션 끊김 · 호출 취소) 요청이 거둬지고, 그 뒤의 [허용] 은 받지 않는다.
    #[tokio::test]
    async fn dropping_the_ask_withdraws_it_and_a_late_answer_is_refused() {
        let approvals = Approvals::default();
        let mut window = approvals.listen(Some("p"), None, conn(1));
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(Some("p"), true)).await })
        };
        let request = opened(&mut window).await;
        asking.abort();
        let _ = asking.await;
        assert_eq!(
            within(window.recv()).await,
            Some(McpApprovalEvent::Closed { id: request.id })
        );
        let mine = ConnectionScope::Project("p".into());
        assert_eq!(
            approvals.answer(
                &Answerer {
                    scope: &mine,
                    connection: conn(1)
                },
                request.id,
                McpApprovalAnswer::Window
            ),
            Err(AnswerError::NotFound)
        );
        assert!(
            !approvals.cli_allowed("p", &fake_write()),
            "거둔 요청의 CLI 허용을 기억했다"
        );
    }

    #[tokio::test]
    async fn the_cli_allowance_lasts_while_a_project_window_listens() {
        let approvals = Approvals::default();
        let mut first = approvals.listen(Some("p"), Some("w1"), conn(1));
        let second = approvals.listen(Some("p"), Some("w1"), conn(2));
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(Some("p"), true)).await })
        };
        let request = opened(&mut first).await;
        let mine = ConnectionScope::Project("p".into());
        approvals
            .answer(
                &Answerer {
                    scope: &mine,
                    connection: conn(1),
                },
                request.id,
                McpApprovalAnswer::Window,
            )
            .unwrap();
        assert_eq!(within(asking).await.unwrap(), Ok(McpApprovalAnswer::Window));
        assert!(approvals.cli_allowed("p", &fake_write()));
        assert!(!approvals.cli_allowed(
            "p",
            &Allowance {
                tool: "other_tool".into(),
                key: None,
                tier: Tier::Write,
            }
        ));
        assert!(
            !approvals.cli_allowed(
                "p",
                &Allowance {
                    tier: Tier::Exec,
                    ..fake_write()
                }
            ),
            "다른 계획 등급"
        );
        assert!(!approvals.cli_allowed("q", &fake_write()));
        drop(first);
        assert!(
            approvals.cli_allowed("p", &fake_write()),
            "아직 그 프로젝트 창이 듣는다"
        );
        drop(second);
        assert!(
            !approvals.cli_allowed("p", &fake_write()),
            "창을 닫으면 사라진다"
        );
    }

    #[tokio::test]
    async fn closing_the_last_project_window_ends_its_requests() {
        let approvals = Approvals::default();
        let window = approvals.listen(Some("p"), None, conn(1));
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(Some("p"), false)).await })
        };
        while approvals.pending() == 0 {
            tokio::task::yield_now().await;
        }
        drop(window);
        assert_eq!(within(asking).await.unwrap(), Err(AskError::WindowLeft));
    }

    /// 프로젝트 없는 요청은 가장 최근에 앞에 온 창 하나에 「이 프로젝트의 것이 아니다」 로 보이고 그 창의 연결만 답한다.
    #[tokio::test]
    async fn a_projectless_request_goes_to_the_front_window_only() {
        let approvals = Approvals::default();
        let mut a = approvals.listen(Some("p"), Some("w-a"), conn(1));
        let mut b = approvals.listen(None, Some("w-b"), conn(2));
        // 포커스를 모르면 가장 최근에 듣기 시작한 창(b). a 가 앞으로 오면 a.
        approvals.focused("w-b");
        approvals.focused("w-a");
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(None, false)).await })
        };
        let request = opened(&mut a).await;
        assert!(request.not_this_project);
        assert_eq!(request.project_id, None);
        assert!(b.try_recv().is_none(), "다른 창에 보였다");
        let host = ConnectionScope::Host;
        assert_eq!(
            approvals.answer(
                &Answerer {
                    scope: &host,
                    connection: conn(2)
                },
                request.id,
                McpApprovalAnswer::Once
            ),
            Err(AnswerError::NotYours)
        );
        // 그 창이 떠나면 다음 창으로 옮긴다.
        drop(a);
        let moved = opened(&mut b).await;
        assert_eq!(moved.id, request.id);
        approvals
            .answer(
                &Answerer {
                    scope: &host,
                    connection: conn(2),
                },
                request.id,
                McpApprovalAnswer::Deny,
            )
            .unwrap();
        assert_eq!(within(asking).await.unwrap(), Ok(McpApprovalAnswer::Deny));
    }

    #[tokio::test]
    async fn without_focus_the_latest_listener_gets_the_projectless_request() {
        let approvals = Approvals::default();
        let mut older = approvals.listen(None, Some("w-old"), conn(1));
        let mut newer = approvals.listen(Some("p"), Some("w-new"), conn(2));
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(None, false)).await })
        };
        opened(&mut newer).await;
        assert!(older.try_recv().is_none());
        drop(newer);
        drop(older);
        assert_eq!(within(asking).await.unwrap(), Err(AskError::WindowLeft));
    }

    #[tokio::test]
    async fn shutdown_withdraws_everything() {
        let approvals = Approvals::default();
        let _window = approvals.listen(Some("p"), None, conn(1));
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move {
                let _hold = approvals.hold();
                approvals.ask(ask(Some("p"), false)).await
            })
        };
        while approvals.pending() == 0 {
            tokio::task::yield_now().await;
        }
        let waiter = {
            let approvals = approvals.clone();
            std::thread::spawn(move || {
                approvals.shutdown_and_wait(Instant::now() + Duration::from_secs(5))
            })
        };
        assert_eq!(within(asking).await.unwrap(), Err(AskError::Shutdown));
        assert!(waiter.join().unwrap());
        assert_eq!(
            within(approvals.ask(ask(Some("p"), false))).await,
            Err(AskError::Shutdown)
        );
    }

    /// 창 식별자가 없는 연결(창 없는 Host 범위 연결 등)은 프로젝트 없는 요청의 후보가 아니다.
    #[tokio::test]
    async fn a_connection_without_a_window_never_gets_a_projectless_request() {
        let approvals = Approvals::default();
        let mut windowless = approvals.listen(None, None, conn(1));
        assert_eq!(
            within(approvals.ask(ask(None, false))).await,
            Err(AskError::NoWindow),
            "창이 없는 연결만 있다"
        );
        let mut window = approvals.listen(Some("p"), Some("w-p"), conn(2));
        let later = approvals.listen(None, None, conn(3));
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(None, false)).await })
        };
        let request = opened(&mut window).await;
        assert!(request.not_this_project);
        assert!(windowless.try_recv().is_none());
        drop(later);
        // 창이 떠나면 창 없는 연결로 옮기지 않고 거절한다.
        drop(window);
        assert_eq!(within(asking).await.unwrap(), Err(AskError::WindowLeft));
        assert!(windowless.try_recv().is_none());
    }

    /// 사라진 창은 포커스 순서에서 빠진다 — 같은 식별자의 듣기가 남아 있어도 앞에 온 순번을 잃는다.
    #[tokio::test]
    async fn a_closed_window_leaves_the_focus_order() {
        let approvals = Approvals::default();
        let mut a = approvals.listen(None, Some("w-a"), conn(1));
        let mut b = approvals.listen(None, Some("w-b"), conn(2));
        approvals.focused("w-b");
        approvals.focused("w-a");
        approvals.window_closed("w-a");
        let asking = {
            let approvals = approvals.clone();
            tokio::spawn(async move { approvals.ask(ask(None, false)).await })
        };
        let request = opened(&mut b).await;
        assert!(a.try_recv().is_none(), "닫힌 창의 순번이 남았다");
        drop(b);
        let moved = opened(&mut a).await;
        assert_eq!(moved.id, request.id);
        drop(a);
        assert_eq!(within(asking).await.unwrap(), Err(AskError::WindowLeft));
    }

    /// [이번만 허용] · [거절] 은 CLI 허용으로 기억하지 않는다 — [창이 열려 있는 동안 CLI 허용] 만.
    #[tokio::test]
    async fn only_the_window_answer_is_remembered_for_the_cli() {
        let approvals = Approvals::default();
        let mut window = approvals.listen(Some("p"), Some("w-p"), conn(1));
        let scope = ConnectionScope::Project("p".into());
        for answer in [McpApprovalAnswer::Once, McpApprovalAnswer::Deny] {
            let asking = {
                let approvals = approvals.clone();
                tokio::spawn(async move { approvals.ask(ask(Some("p"), true)).await })
            };
            let request = opened(&mut window).await;
            approvals
                .answer(
                    &Answerer {
                        scope: &scope,
                        connection: conn(1),
                    },
                    request.id,
                    answer,
                )
                .unwrap();
            assert_eq!(within(asking).await.unwrap(), Ok(answer));
            assert_eq!(
                within(window.recv()).await,
                Some(McpApprovalEvent::Closed { id: request.id })
            );
            assert!(
                !approvals.cli_allowed("p", &fake_write()),
                "{answer:?} 를 CLI 허용으로 기억했다"
            );
        }
    }
}
