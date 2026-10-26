//! 창에 묻기: askpass 입력과 미저장 문서 확인(13-m2-plan.md M2-06).
//!
//! 프로젝트 창은 떠 있는 동안 `git.prompts` 스트림을 열어 둔다(듣기, [`Prompts::listen`]). Host 가 물을 것이 생기면 그 프로젝트를
//! 듣는 창 전부에 질문을 보내고([`GitPromptEvent::Open`]), 창은 다이얼로그를 띄워 `git.prompt.answer` 로 답한다
//! ([`Prompts::answer`]). 처음 온 답이 그 질문의 답이다. 답을 받았거나 시간이 지났거나 묻던 쪽이 그만두면 듣는 창 전부에 닫힘
//! ([`GitPromptEvent::Closed`])을 보내 다른 창의 다이얼로그도 내린다. 나중에 연 창(새로 고침 등)은 듣기 시작할 때 아직 열린
//! 질문을 받는다.
//!
//! Host 는 듣는 창의 수를 안다 — 듣는 창이 없으면 기다리지 않는다([`PromptError::NoWindow`]). 묻는 동안 마지막 창이 듣기를 그만두면
//! (창을 닫음) 그 프로젝트의 열린 질문도 같은 답으로 끝난다. askpass 는 그때 실패하고, 미저장 확인은 확인할 창(미저장 문서)이
//! 없으므로 그대로 진행한다. 프로젝트 창은 프로젝트마다 하나라 「처음 온 답」 이 그 창의 답이다.
//!
//! **비밀**: askpass 의 답([`GitPromptAnswer::Askpass`] 의 `value`)은 로그 · 상태 · 알림 어디에도 남기지 않는다. 그 타입의
//! `Debug` 는 값을 가린다. 질문의 문장(git · ssh 가 준 것)도 로그에 쓰지 않는다 — URL 에 사용자 이름 등이 들어 있을 수 있다.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

/// askpass 질문을 기다리는 시간. 답이 없으면 askpass 가 실패하고 git 은 인증 실패로 끝난다.
pub const ASKPASS_TIMEOUT: Duration = Duration::from_secs(120);

/// 미저장 확인을 기다리는 시간. 답이 없으면 쓰기를 취소한다.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(600);

/// 창에 묻는 것.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitPromptRequest {
    /// git · ssh 가 묻는 것(암호 · 토큰 · 키 암호 · 호스트 키 확인). `text` 는 그 프로그램이 준 문장 그대로다 — 번역하지 않는다.
    /// 답은 비밀번호 칸으로 받는다.
    Askpass { text: String },
    /// 작업 트리를 바꾸는 쓰기를 하기 전이다. 창은 이 경로와 겹치는 미저장 문서가 있으면 확인하고(M1-13 의 확인 — 저장 · 저장 안
    /// 함 · 취소), 없으면 바로 `proceed` 로 답한다. `paths` 는 `workTree` 기준 상대 경로(`/` 구분)이고, `null` 이면 작업 트리
    /// 전체다(바뀔 경로를 미리 모르는 쓰기 — 진행 중 작업의 계속 · 중단 등).
    Unsaved {
        work_tree: String,
        paths: Option<Vec<String>>,
    },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPrompt {
    /// 이 Host 안에서 질문마다 다르다. 답할 때 쓴다.
    #[specta(type = pw_rpc::JsNumber)]
    pub prompt_id: u64,
    pub request: GitPromptRequest,
}

/// `git.prompts` 스트림의 항목.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitPromptEvent {
    /// 새 질문(또는 듣기 시작할 때 아직 열린 질문).
    Open { prompt: GitPrompt },
    /// 그 질문이 끝났다(답을 받음 · 시간 초과 · 묻던 쓰기가 끝남). 떠 있는 다이얼로그를 내린다.
    Closed {
        #[specta(type = pw_rpc::JsNumber)]
        prompt_id: u64,
    },
}

/// 창의 답. `askpass` 질문에는 `askpass` 나 `cancel`, `unsaved` 질문에는 `proceed` 나 `cancel` 만 받는다.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitPromptAnswer {
    /// 넣은 값. 비밀이다 — `Debug` 는 가린다.
    Askpass { value: String },
    /// 미저장 문서를 확인했다(저장했거나 저장하지 않기로 했다) — 쓰기를 한다.
    Proceed,
    /// 취소. askpass 면 git 이 인증 실패로 끝나고, 미저장 확인이면 쓰기를 하지 않는다.
    Cancel,
}

impl fmt::Debug for GitPromptAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Askpass { .. } => f.write_str("Askpass { value: <hidden> }"),
            Self::Proceed => f.write_str("Proceed"),
            Self::Cancel => f.write_str("Cancel"),
        }
    }
}

impl GitPromptAnswer {
    /// `request` 에 맞는 답인가.
    fn fits(&self, request: &GitPromptRequest) -> bool {
        matches!(
            (request, self),
            (
                GitPromptRequest::Askpass { .. },
                Self::Askpass { .. } | Self::Cancel
            ) | (
                GitPromptRequest::Unsaved { .. },
                Self::Proceed | Self::Cancel
            )
        )
    }
}

/// 묻기가 답 없이 끝난 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptError {
    /// 그 프로젝트를 듣는 창이 없다.
    NoWindow,
    /// 정한 시간 안에 답이 없었다.
    TimedOut,
}

/// 답을 받지 않은 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerError {
    /// 그 프로젝트에 그런 열린 질문이 없다(이미 답했거나 끝났다).
    NotFound,
    /// 질문의 종류에 맞지 않는 답이다.
    Mismatch,
}

/// 질문 표와 듣는 창. 복제하면 같은 표를 가리킨다.
#[derive(Clone, Default)]
pub struct Prompts {
    inner: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    next_prompt: u64,
    next_listener: u64,
    /// 프로젝트 → 듣는 창(듣기 id → 보낼 곳).
    listeners: HashMap<String, BTreeMap<u64, mpsc::UnboundedSender<GitPromptEvent>>>,
    /// 열린 질문. 묻는 쪽([`Prompts::ask`])이 살아 있는 동안만 있다.
    pending: BTreeMap<u64, Pending>,
}

struct Pending {
    project: String,
    prompt: GitPrompt,
    /// 첫 답을 보내면 비운다.
    reply: Option<oneshot::Sender<GitPromptAnswer>>,
}

impl State {
    fn broadcast(&mut self, project: &str, event: &GitPromptEvent) {
        if let Some(listeners) = self.listeners.get_mut(project) {
            listeners.retain(|_, tx| tx.send(event.clone()).is_ok());
        }
    }
}

impl Prompts {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `project` 를 듣기 시작한다. 아직 열린 그 프로젝트의 질문이 먼저 온다. 돌려준 것을 버리면 듣기를 그만둔다.
    pub fn listen(&self, project: &str) -> Listener {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.lock();
        for pending in state.pending.values().filter(|p| p.project == project) {
            let _ = tx.send(GitPromptEvent::Open {
                prompt: pending.prompt.clone(),
            });
        }
        state.next_listener += 1;
        let id = state.next_listener;
        state
            .listeners
            .entry(project.to_owned())
            .or_default()
            .insert(id, tx);
        Listener {
            prompts: self.clone(),
            project: project.to_owned(),
            id,
            rx,
        }
    }

    /// `project` 를 듣는 창의 수.
    pub fn listeners(&self, project: &str) -> usize {
        self.lock().listeners.get(project).map_or(0, |listeners| {
            listeners.values().filter(|tx| !tx.is_closed()).count()
        })
    }

    /// 열린 질문의 수(모든 프로젝트).
    pub fn pending(&self) -> usize {
        self.lock().pending.len()
    }

    /// `project` 를 듣는 창에 묻고 첫 답을 기다린다. 듣는 창이 없으면 기다리지 않는다. 이 future 를 버리면 질문을 닫는다.
    pub async fn ask(
        &self,
        project: &str,
        request: GitPromptRequest,
        timeout: Duration,
    ) -> Result<GitPromptAnswer, PromptError> {
        let (rx, _closing) = {
            let mut state = self.lock();
            if state
                .listeners
                .get(project)
                .is_none_or(|listeners| listeners.values().all(|tx| tx.is_closed()))
            {
                return Err(PromptError::NoWindow);
            }
            state.next_prompt += 1;
            let prompt = GitPrompt {
                prompt_id: state.next_prompt,
                request,
            };
            let (tx, rx) = oneshot::channel();
            state.broadcast(
                project,
                &GitPromptEvent::Open {
                    prompt: prompt.clone(),
                },
            );
            let id = prompt.prompt_id;
            state.pending.insert(
                id,
                Pending {
                    project: project.to_owned(),
                    prompt,
                    reply: Some(tx),
                },
            );
            (
                rx,
                Closing {
                    prompts: self.clone(),
                    id,
                },
            )
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => Ok(answer),
            // 답하지 않고 보내는 쪽을 버리는 것은 마지막 창이 듣기를 그만둘 때뿐이다([`Listener`] 의 drop).
            Ok(Err(_)) => Err(PromptError::NoWindow),
            Err(_) => Err(PromptError::TimedOut),
        }
    }

    /// 창의 답. 그 프로젝트의 열린 질문이고 종류에 맞으면 묻던 쪽에 넘긴다.
    pub fn answer(
        &self,
        project: &str,
        prompt_id: u64,
        answer: GitPromptAnswer,
    ) -> Result<(), AnswerError> {
        let mut state = self.lock();
        let Some(pending) = state
            .pending
            .get_mut(&prompt_id)
            .filter(|p| p.project == project)
        else {
            return Err(AnswerError::NotFound);
        };
        if !answer.fits(&pending.prompt.request) {
            return Err(AnswerError::Mismatch);
        }
        let reply = pending.reply.take().ok_or(AnswerError::NotFound)?;
        // 묻던 쪽이 막 그만뒀으면(받는 쪽이 없다) 답은 버려진다 — 닫힘은 그쪽이 보낸다.
        let _ = reply.send(answer);
        Ok(())
    }
}

/// 질문이 끝나면(답 · 시간 초과 · 묻던 future 를 버림) 표에서 빼고 듣는 창에 닫힘을 보낸다.
struct Closing {
    prompts: Prompts,
    id: u64,
}

impl Drop for Closing {
    fn drop(&mut self) {
        let mut state = self.prompts.lock();
        if let Some(pending) = state.pending.remove(&self.id) {
            state.broadcast(
                &pending.project,
                &GitPromptEvent::Closed { prompt_id: self.id },
            );
        }
    }
}

/// 한 창의 듣기. 버리면 듣는 창에서 빠진다.
pub struct Listener {
    prompts: Prompts,
    project: String,
    id: u64,
    rx: mpsc::UnboundedReceiver<GitPromptEvent>,
}

impl Listener {
    /// 다음 항목. 표가 사라지지 않는 한 끝나지 않는다.
    pub async fn recv(&mut self) -> Option<GitPromptEvent> {
        self.rx.recv().await
    }

    /// 기다리지 않고 꺼낸다.
    pub fn try_recv(&mut self) -> Option<GitPromptEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let mut state = self.prompts.lock();
        let Some(listeners) = state.listeners.get_mut(&self.project) else {
            return;
        };
        listeners.remove(&self.id);
        if !listeners.is_empty() {
            return;
        }
        state.listeners.remove(&self.project);
        // 마지막 창이 떠났다 — 그 프로젝트의 열린 질문에 답할 창이 없다. 보내는 쪽을 버리면 묻던 쪽이 「창 없음」 으로 끝난다.
        for pending in state.pending.values_mut() {
            if pending.project == self.project {
                pending.reply = None;
            }
        }
    }
}
