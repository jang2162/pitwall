//! 원격의 열기 요청을 붙은 Desktop 으로 넘기는 자리(18-m7-plan M7-15 ② · 결정 M7-D3 — 06 D75). 창이 없는 Host(Relay)는
//! [`Desks`] 를 [`super::OpenRequests::attach`] 로 여는 쪽에 붙인다 — `pitwall-relay open <경로>` 와 에이전트의 `project_open` 이
//! 같은 길을 지난다.
//!
//! - **듣기**: 붙은 Desktop 은 Host 범위 연결로 `open.listen` 스트림을 열어 둔다. 첫 항목 [`OpenEvent::Listening`] 이 그 듣기의
//!   표(`listener`)다 — 답(`open.answer`)은 그 표를 실어야 받는다(같은 사용자의 다른 연결이 남의 질문에 답하지 못하게).
//! - **행선지**(메인 판정 2026-11-23): 듣는 owner Desktop 가운데 **마지막으로 사람 입력이 있던 것** 하나. 요청마다 듣는 것 전부에
//!   [`OpenEvent::Query`] 를 보내 「입력 뒤 지난 시간」 만 받는다([`QUERY_TIMEOUT`]) — Desktop 은 창 목록 · 프로젝트 이름을 원격에
//!   보내지 않는다. 가장 작은 값을 낸 Desktop 에 [`OpenEvent::Open`] 을 보낸다. 그 Desktop 에 그 묶음 창이 없으면 Desktop 이 창을
//!   앞으로 가져오지 않고 연다. 듣는 Desktop 이 없거나 아무도 답하지 않으면 거절한다(원격에 그 까닭을 찍는다).
//! - **누가 듣나**: `open.listen` · `open.answer` 는 owner 의 **Host 범위** 연결만 받는다 — 링크에서는 증명된 owner 기기의 링크
//!   (M7-02 ② 의 role 자르기로 다른 기기는 role 에서 걸린다). 프로젝트 창 · 파일 창 · 에이전트(MCP) 연결은 거절한다.
//! - Desktop 의 Host 에도 같은 메서드가 있다(같은 조립 — M7-01). Desktop 은 창을 직접 열므로 그 스트림에는 아무것도 오지 않는다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use pw_rpc::{ConnectionScope, Ctx, Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use super::{OpenRequest, OpenedIn, Opener};

/// 「입력 뒤 지난 시간」 을 기다리는 시간.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
/// 고른 Desktop 이 열었다고 답하기를 기다리는 시간.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// 듣기 하나의 보내기 줄 길이. 차면 그 Desktop 은 이번 요청에서 빠진다(답하지 않는 Desktop 이 요청을 붙잡지 않게).
const QUEUE: usize = 16;

/// 거절의 까닭(원격 셸 · 에이전트가 읽는 영어).
pub const NO_DESKTOP: &str = "no Pitwall desktop is connected to this machine to open it";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OpenListenParams {}

/// `open.listen` 의 항목.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OpenEvent {
    /// 듣기가 섰다. `listener` 를 답에 싣는다.
    Listening { listener: String },
    /// 열기 요청이 왔다 — 사람 입력 뒤 지난 시간을 [`OpenAnswer::Idle`] 로 답한다.
    Query {
        #[specta(type = pw_rpc::JsNumber)]
        request_id: u64,
    },
    /// 이 Desktop 이 연다. 그 묶음 창의 이 체크아웃(`worktree` 가 없으면 메인 체크아웃) 탭으로 — 활성 체크아웃은 바꾸지 않는다.
    /// 답은 [`OpenAnswer::Opened`] · [`OpenAnswer::Failed`].
    Open {
        #[specta(type = pw_rpc::JsNumber)]
        request_id: u64,
        /// 이 머신 레지스트리의 프로젝트 id.
        project_id: String,
        /// 워크트리 꼭대기(이 머신의 절대 경로). 메인 체크아웃이면 `null`.
        worktree: Option<String>,
        /// 열 파일의 실제 경로(프로젝트 루트 안). 프로젝트만 열면 `null`.
        path: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
}

/// Desktop 의 답.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OpenAnswer {
    /// 마지막 사람 입력 뒤 지난 밀리초.
    Idle {
        #[specta(type = pw_rpc::JsNumber)]
        idle_ms: u64,
    },
    Opened {
        opened_in: OpenedIn,
    },
    /// 열지 못했다(원격에 찍힌다 — 짧은 영어).
    Failed {
        message: String,
    },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OpenAnswerParams {
    pub listener: String,
    #[specta(type = pw_rpc::JsNumber)]
    pub request_id: u64,
    pub answer: OpenAnswer,
}

rpc_method! {
    /// 이 Host 의 열기 요청을 받는다(창이 없는 Host — Relay 에 붙은 Desktop). owner 의 Host 범위 연결만.
    pub OpenListen: "open.listen" {
        params: OpenListenParams,
        stream: OpenEvent,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// `open.listen` 으로 받은 질문 · 열기 요청에 답한다. owner 의 Host 범위 연결만.
    pub OpenAnswerMethod: "open.answer" {
        params: OpenAnswerParams,
        result: (),
        tier: Read,
        roles: [Owner],
    }
}

/// 기다리는 답의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Query,
    Open,
}

struct Pending {
    phase: Phase,
    reply: oneshot::Sender<OpenAnswer>,
}

struct Listener {
    token: String,
    events: mpsc::Sender<OpenEvent>,
}

#[derive(Default)]
struct State {
    listeners: HashMap<u64, Listener>,
    /// (듣기 표, 요청 id) → 기다리는 답.
    pending: HashMap<(String, u64), Pending>,
}

/// 붙은 Desktop 들의 듣기. 복제하면 같은 것을 가리킨다.
#[derive(Clone, Default)]
pub struct Desks {
    state: Arc<Mutex<State>>,
    next: Arc<AtomicU64>,
}

/// 열기의 실패.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeskError {
    /// 듣는 Desktop 이 없다(또는 아무도 답하지 않았다).
    NoDesktop,
    /// 고른 Desktop 이 열지 못했다 · 답하지 않았다.
    Failed(String),
}

impl std::fmt::Display for DeskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDesktop => f.write_str(NO_DESKTOP),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// 듣기 하나가 사는 동안. 버리면 표에서 빠지고 그 표로 기다리던 답을 버린다.
pub struct Listening {
    desks: Desks,
    id: u64,
    token: String,
    events: mpsc::Receiver<OpenEvent>,
}

impl Listening {
    pub fn token(&self) -> &str {
        &self.token
    }

    pub async fn recv(&mut self) -> Option<OpenEvent> {
        self.events.recv().await
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        let mut state = self.desks.lock();
        state.listeners.remove(&self.id);
        state.pending.retain(|(token, _), _| *token != self.token);
    }
}

impl Desks {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 지금 듣는 Desktop 수.
    pub fn listeners(&self) -> usize {
        self.lock().listeners.len()
    }

    /// 듣기를 연다.
    pub fn listen(&self) -> Listening {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let token = super::token();
        let (tx, rx) = mpsc::channel(QUEUE);
        self.lock().listeners.insert(
            id,
            Listener {
                token: token.clone(),
                events: tx,
            },
        );
        Listening {
            desks: self.clone(),
            id,
            token,
            events: rx,
        }
    }

    /// 듣기 `token` 의 답. 그 표로 기다리는 그 요청이 없거나(끝났다 · 남의 것) 꼴이 맞지 않으면 `false`.
    pub fn answer(&self, token: &str, request_id: u64, answer: OpenAnswer) -> bool {
        let mut state = self.lock();
        let key = (token.to_owned(), request_id);
        let fits = state.pending.get(&key).is_some_and(|p| {
            matches!(
                (p.phase, &answer),
                (Phase::Query, OpenAnswer::Idle { .. })
                    | (
                        Phase::Open,
                        OpenAnswer::Opened { .. } | OpenAnswer::Failed { .. }
                    )
            )
        });
        if !fits {
            return false;
        }
        let pending = state.pending.remove(&key).expect("checked above");
        pending.reply.send(answer).is_ok()
    }

    /// 보내고 답을 기다릴 자리를 만든다. 보내지 못하면(줄이 참 · 듣기가 닫힘) `None`.
    fn ask(
        &self,
        token: &str,
        events: &mpsc::Sender<OpenEvent>,
        request_id: u64,
        phase: Phase,
        event: OpenEvent,
    ) -> Option<oneshot::Receiver<OpenAnswer>> {
        let (tx, rx) = oneshot::channel();
        self.lock()
            .pending
            .insert((token.to_owned(), request_id), Pending { phase, reply: tx });
        if events.try_send(event).is_err() {
            self.lock().pending.remove(&(token.to_owned(), request_id));
            return None;
        }
        Some(rx)
    }

    fn forget(&self, token: &str, request_id: u64) {
        self.lock().pending.remove(&(token.to_owned(), request_id));
    }

    /// 요청 하나를 마지막 입력 Desktop 에 넘기고 그 답을 기다린다.
    pub async fn open(&self, request: &OpenRequest) -> Result<OpenedIn, DeskError> {
        let listeners: Vec<(String, mpsc::Sender<OpenEvent>)> = self
            .lock()
            .listeners
            .values()
            .map(|l| (l.token.clone(), l.events.clone()))
            .collect();
        if listeners.is_empty() {
            return Err(DeskError::NoDesktop);
        }
        let request_id = self.next.fetch_add(1, Ordering::Relaxed);
        let asked: Vec<_> = listeners
            .iter()
            .filter_map(|(token, events)| {
                self.ask(
                    token,
                    events,
                    request_id,
                    Phase::Query,
                    OpenEvent::Query { request_id },
                )
                .map(|rx| (token.clone(), events.clone(), rx))
            })
            .collect();
        let deadline = tokio::time::Instant::now() + QUERY_TIMEOUT;
        let mut best: Option<(u64, String, mpsc::Sender<OpenEvent>)> = None;
        for (token, events, rx) in asked {
            let answer = tokio::time::timeout_at(deadline, rx).await;
            self.forget(&token, request_id);
            if let Ok(Ok(OpenAnswer::Idle { idle_ms })) = answer
                && best.as_ref().is_none_or(|(b, ..)| idle_ms < *b)
            {
                best = Some((idle_ms, token, events));
            }
        }
        let Some((_, token, events)) = best else {
            return Err(DeskError::NoDesktop);
        };
        let open = OpenEvent::Open {
            request_id,
            project_id: request.project_id.clone(),
            worktree: request.worktree.clone(),
            path: request.file.as_ref().map(|f| f.display().to_string()),
            line: request.line,
            column: request.column,
        };
        let rx = self
            .ask(&token, &events, request_id, Phase::Open, open)
            .ok_or(DeskError::NoDesktop)?;
        let answer = tokio::time::timeout(OPEN_TIMEOUT, rx).await;
        self.forget(&token, request_id);
        match answer {
            Ok(Ok(OpenAnswer::Opened { opened_in })) => Ok(opened_in),
            Ok(Ok(OpenAnswer::Failed { message })) => Err(DeskError::Failed(message)),
            Ok(Ok(OpenAnswer::Idle { .. })) | Ok(Err(_)) => Err(DeskError::Failed(
                "the Pitwall desktop stopped listening before opening it".to_owned(),
            )),
            Err(_) => Err(DeskError::Failed(
                "the Pitwall desktop did not open it in time".to_owned(),
            )),
        }
    }
}

/// 창이 없는 Host 의 여는 쪽. 블로킹 스레드에서 불린다(`super::register` 의 `spawn_blocking`).
impl Opener for Desks {
    fn open(&self, request: &OpenRequest) -> Result<OpenedIn, String> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "the relay cannot wait for a desktop here".to_owned())?;
        handle
            .block_on(Desks::open(self, request))
            .map_err(|e| e.to_string())
    }
}

/// owner 의 Host 범위 연결인가(role 은 라우터가 본다).
fn host_scope(ctx: &Ctx) -> Result<(), RpcError> {
    if *ctx.scope() == ConnectionScope::Host {
        Ok(())
    } else {
        Err(RpcError::new(
            code::PERMISSION_DENIED,
            "only a desktop's host connection listens for open requests",
        ))
    }
}

/// `open.listen` · `open.answer` 를 붙인다.
pub fn register(router: &mut Router, desks: Desks) {
    let d = desks.clone();
    router.stream(OpenListen, move |ctx, _params: OpenListenParams, sink| {
        let desks = d.clone();
        async move {
            host_scope(&ctx)?;
            let mut listening = desks.listen();
            sink.send(OpenEvent::Listening {
                listener: listening.token().to_owned(),
            })
            .await?;
            while let Some(event) = listening.recv().await {
                sink.send(event).await?;
            }
            Ok(())
        }
    });
    router.unary(OpenAnswerMethod, move |ctx, params: OpenAnswerParams| {
        let desks = desks.clone();
        async move {
            host_scope(&ctx)?;
            if desks.answer(&params.listener, params.request_id, params.answer) {
                Ok(())
            } else {
                Err(RpcError::new(
                    code::NOT_FOUND,
                    "no such open request (already answered or closed)",
                ))
            }
        }
    });
}

#[cfg(test)]
mod tests;
