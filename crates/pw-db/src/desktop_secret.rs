//! 「Desktop 에 묻는」 비밀 저장소 — 원격 Relay 의 [`SecretStore`](18-m7-plan M7-12 ② · 결정 D47).
//!
//! Relay 는 비밀을 디스크 · 키체인에 두지 않는다(메인 판정 M7-P5). 비밀은 링크로 붙은 Desktop 의 키체인에 (머신 id, 멤버 프로젝트
//! id, 연결 id) 열쇠로 있고, Relay 는 비밀이 필요할 때마다 그 Desktop 에 청한다(역요청):
//!
//! - 붙은 Desktop 이 owner · Host 범위 연결로 `secret.requests`(스트림)를 연다. Relay 가 비밀을 읽거나 쓰거나 지울 때마다 그 스트림에
//!   청([`SecretRequest`])을 보내고, Desktop 이 **같은 연결**로 `secret.answer` 를 보낸다(다른 연결의 답은 받지 않는다). 여러
//!   Desktop 이 열었으면 모두에게 보내고 처음 온 답(값 · 없음 · 했음 · 실패)을 쓴다 — 모두 거절하면 거절이다.
//! - 청에는 그 비밀을 부른 창의 요청([`SecretRequestCall`] — 창 label · 요청 id)이 실린다. Desktop 은 **그 요청이 지금 답을 기다리는 중이고 비밀이 필요한 메서드일 때 한 번만** 답한다(`pw_remote::secrets`). 열쇠의 머신 id 는
//!   Relay 가 대는 것이 아니라 그 링크가 증명한 것이다 — 다른 머신 id 의 링크는 남의 비밀을 받지 못한다(계획 리뷰 F1).
//! - 에이전트의 증거로 연 연결([`SecretAsker::Agent`])이나 세운 곳이 없는 흐름(`None`)에서는 묻지 않는다(D48 · D60 — 에이전트의 DB
//!   툴은 사람이 연 연결만 쓴다).
//! - 받은 비밀은 이 저장소에 두지 않는다 — 부른 쪽(연결을 여는 드라이버)에 넘기고 버린다. 연결이 사는 동안은 드라이버가 쥐고, 그 DB
//!   세션이 끝나면 함께 버려진다. 로그에는 계정 · 까닭만 쓴다.
//! - 답을 [`WAIT`] 안에 받지 못하거나 붙은 Desktop 이 없으면 「쓸 수 없음」(키체인 잠김과 같은 취급)이다.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_db_types::Secret;
use pw_rpc::{ConnectionScope, Ctx, Router, RpcError, SessionId, code, rpc_method};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::lock;
use crate::secret::{DbSecretKey, SecretAsker, SecretCall, SecretStore, StoreError, current_asker};

/// Desktop 의 답을 기다리는 시한. 사람이 누른 흐름 안의 청이라 Desktop 은 곧바로 답한다(키체인 읽기 하나).
pub const WAIT: Duration = Duration::from_secs(20);

/// 이 저장소가 Host 에 내는 메서드 — Relay 의 Host 에만 있다(Desktop 의 Host 에는 없다).
pub const METHODS: [&str; 2] = ["secret.requests", "secret.answer"];

/// 청의 종류.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SecretRequestOp {
    /// 연결을 열려고 비밀을 읽는다.
    Read,
    /// 그 창이 넣은 비밀(`db.secret.set`)을 키체인에 둔다. `item` 은 값과 대상의 JSON(`crate::secret` 의 키체인 항목)이다.
    Store {
        #[specta(type = String)]
        #[schemars(with = "String")]
        item: Secret,
    },
    /// 지운다(`db.secret.clear` · 연결 지우기).
    Remove,
}

impl fmt::Debug for SecretRequestOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Read => "Read",
            Self::Store { .. } => "Store(***)",
            Self::Remove => "Remove",
        })
    }
}

/// Relay → Desktop 청 하나(`secret.requests` 의 항목).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequest {
    /// 이 비밀을 부른 창의 요청 — Desktop 은 그 요청이 지금 기다리는 중이고 그 메서드가 이 청을 필요로 할 때만 답한다.
    pub call: SecretRequestCall,
    /// 답(`secret.answer`)에 싣는 id. 받는 Desktop 마다 다르다.
    #[specta(type = pw_rpc::JsNumber)]
    pub id: u64,
    /// 그 머신(Relay) 레지스트리의 프로젝트 id.
    pub project_id: String,
    pub connection_id: String,
    pub key: DbSecretKey,
    pub op: SecretRequestOp,
}

/// 청을 부른 창의 요청(`crate::secret::SecretCall`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequestCall {
    /// 그 연결의 창 label(링크로 온 연결이면 `<기기 id>/<label>`).
    pub window: String,
    /// 그 요청의 JSON-RPC id(JSON 글 — `7` · `"a"`).
    pub request: String,
}

/// Desktop 의 답.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SecretAnswer {
    /// 답하지 않는다(그 창이 그 연결로 기다리는 요청이 없다 등).
    Declined,
    /// 키체인에 없다.
    Missing,
    /// 읽은 항목(값과 대상의 JSON).
    Value {
        #[specta(type = String)]
        #[schemars(with = "String")]
        item: Secret,
    },
    /// 두었다 · 지웠다.
    Done,
    /// 키체인을 쓸 수 없다. 값을 싣지 않는다.
    Failed { message: String },
}

impl fmt::Debug for SecretAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declined => f.write_str("Declined"),
            Self::Missing => f.write_str("Missing"),
            Self::Value { .. } => f.write_str("Value(***)"),
            Self::Done => f.write_str("Done"),
            Self::Failed { message } => write!(f, "Failed({message})"),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SecretAnswerParams {
    #[specta(type = pw_rpc::JsNumber)]
    pub id: u64,
    pub answer: SecretAnswer,
}

rpc_method! {
    /// 붙은 Desktop 이 이 머신(Relay)의 비밀 청을 받는 스트림. owner · Host 범위 연결만. 청마다 같은 연결로 `secret.answer` 를 보낸다.
    /// Relay 의 Host 에만 있다.
    pub SecretRequests: "secret.requests" {
        params: (),
        stream: SecretRequest,
        tier: Read,
        roles: [Owner],
    }

    /// `secret.requests` 로 받은 청의 답. 그 청을 받은 연결의 답만 받는다. Relay 의 Host 에만 있다.
    pub SecretAnswerMethod: "secret.answer" {
        params: SecretAnswerParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

/// 청 하나의 끝.
enum Reply {
    Answer(SecretAnswer),
    /// 받은 Desktop 이 모두 거절했다.
    Declined,
}

struct Ask {
    outstanding: HashSet<u64>,
    reply: std_mpsc::Sender<Reply>,
}

#[derive(Default)]
struct State {
    next: u64,
    /// 스트림을 연 세션 → 그 스트림으로 보낼 것.
    pollers: HashMap<SessionId, mpsc::UnboundedSender<SecretRequest>>,
    asks: HashMap<u64, Ask>,
    /// 받는 쪽마다의 청 id → (청, 받은 세션).
    deliveries: HashMap<u64, (u64, SessionId)>,
}

impl State {
    fn next(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// 받은 쪽 하나가 거절했다(또는 사라졌다). 모두 거절했으면 그 청을 끝낸다.
    fn decline(&mut self, delivery: u64) {
        let Some((ask, _)) = self.deliveries.remove(&delivery) else {
            return;
        };
        let done = self.asks.get_mut(&ask).is_some_and(|a| {
            a.outstanding.remove(&delivery);
            a.outstanding.is_empty()
        });
        if done && let Some(ask) = self.asks.remove(&ask) {
            let _ = ask.reply.send(Reply::Declined);
        }
    }

    /// 청을 끝낸다(남은 받는 쪽의 id 도 걷는다).
    fn finish(&mut self, ask: u64) -> Option<Ask> {
        let ask = self.asks.remove(&ask)?;
        for delivery in &ask.outstanding {
            self.deliveries.remove(delivery);
        }
        Some(ask)
    }
}

/// 원격 Relay 의 비밀 저장소. 복제하면 같은 것을 본다.
#[derive(Clone)]
pub struct DesktopSecrets {
    state: Arc<Mutex<State>>,
    wait: Duration,
}

impl Default for DesktopSecrets {
    fn default() -> Self {
        Self::new()
    }
}

/// 계정(`<프로젝트 id>/<연결 id>/<키>` — [`crate::secret::account`])을 가른다.
fn parse_account(account: &str) -> Option<(String, String, DbSecretKey)> {
    let mut parts = account.split('/');
    let (project, connection, key) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || project.is_empty() || connection.is_empty() {
        return None;
    }
    let key = DbSecretKey::ALL.into_iter().find(|k| k.id() == key)?;
    Some((project.to_owned(), connection.to_owned(), key))
}

impl DesktopSecrets {
    pub fn new() -> Self {
        Self::with_wait(WAIT)
    }

    /// 기다리는 시한을 바꾼다(시험).
    pub fn with_wait(wait: Duration) -> Self {
        Self {
            state: Arc::default(),
            wait,
        }
    }

    /// 지금 청을 받는 Desktop 연결의 수.
    pub fn desktops(&self) -> usize {
        lock(&self.state).pollers.len()
    }

    /// 청을 받는 쪽을 붙인다(`secret.requests` 하나). 돌려준 수신기를 버리면 떨어진다.
    fn listen(&self, session: SessionId) -> Listener {
        let (tx, rx) = mpsc::unbounded_channel();
        let old = lock(&self.state).pollers.insert(session, tx);
        if old.is_some() {
            tracing::info!(target: "pw_db", ?session, "a desktop reopened its secret requests");
        }
        Listener {
            secrets: self.clone(),
            session,
            rx,
        }
    }

    fn unlisten(&self, session: SessionId) {
        let mut state = lock(&self.state);
        state.pollers.remove(&session);
        let gone: Vec<u64> = state
            .deliveries
            .iter()
            .filter(|(_, (_, s))| *s == session)
            .map(|(d, _)| *d)
            .collect();
        for delivery in gone {
            state.decline(delivery);
        }
    }

    /// 답을 받는다. 그 청을 받은 세션의 답이 아니면 `false`(없는 청과 같다).
    fn answer(&self, session: SessionId, id: u64, answer: SecretAnswer) -> bool {
        let mut state = lock(&self.state);
        match state.deliveries.get(&id) {
            Some((_, s)) if *s == session => {}
            _ => return false,
        }
        if answer == SecretAnswer::Declined {
            state.decline(id);
            return true;
        }
        let Some((ask, _)) = state.deliveries.get(&id).copied() else {
            return false;
        };
        if let Some(ask) = state.finish(ask) {
            let _ = ask.reply.send(Reply::Answer(answer));
        }
        true
    }

    /// 붙은 Desktop 들에 묻고 답을 기다린다(블로킹). 붙은 Desktop 이 없으면 `None`.
    fn ask(
        &self,
        account: &str,
        call: &SecretCall,
        op: SecretRequestOp,
    ) -> Result<Reply, StoreError> {
        let (project_id, connection_id, key) = parse_account(account)
            .ok_or_else(|| StoreError(format!("not a db secret account: {account}")))?;
        let (reply, answer) = std_mpsc::channel();
        let ask = {
            let mut state = lock(&self.state);
            let ask = state.next();
            let sessions: Vec<SessionId> = state.pollers.keys().copied().collect();
            let mut outstanding = HashSet::new();
            for session in sessions {
                let id = state.next();
                let request = SecretRequest {
                    call: SecretRequestCall {
                        window: call.window.clone(),
                        request: call.request.clone(),
                    },
                    id,
                    project_id: project_id.clone(),
                    connection_id: connection_id.clone(),
                    key,
                    op: op.clone(),
                };
                let sent = state
                    .pollers
                    .get(&session)
                    .is_some_and(|tx| tx.send(request).is_ok());
                if sent {
                    outstanding.insert(id);
                    state.deliveries.insert(id, (ask, session));
                } else {
                    state.pollers.remove(&session);
                }
            }
            if outstanding.is_empty() {
                tracing::info!(target: "pw_db", %account, "no desktop is attached to answer a secret request");
                return Err(StoreError(
                    "no desktop is attached to this machine to read its keychain".to_owned(),
                ));
            }
            state.asks.insert(ask, Ask { outstanding, reply });
            ask
        };
        match answer.recv_timeout(self.wait) {
            Ok(reply) => Ok(reply),
            Err(_) => {
                lock(&self.state).finish(ask);
                tracing::info!(target: "pw_db", %account, "the desktop did not answer a secret request in time");
                Err(StoreError(
                    "the desktop did not answer the secret request in time".to_owned(),
                ))
            }
        }
    }
}

/// 창의 흐름이고 그 비밀을 부른 요청을 알 때만 그 요청. 에이전트 · 세운 곳 없는 흐름 · 요청을 모르는 흐름은 `None`(묻지 않는다).
fn window_call() -> Option<SecretCall> {
    match current_asker() {
        Some(SecretAsker::Window(call)) => call,
        _ => None,
    }
}

fn outside_a_request() -> StoreError {
    StoreError("not in a window's request — the desktop is not asked".to_owned())
}

impl SecretStore for DesktopSecrets {
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError> {
        let Some(call) = window_call() else {
            tracing::info!(target: "pw_db", %account, "not asking the desktop for a secret outside a window's request");
            return Ok(None);
        };
        match self.ask(account, &call, SecretRequestOp::Read)? {
            Reply::Answer(SecretAnswer::Value { item }) => Ok(Some(item)),
            Reply::Answer(SecretAnswer::Missing) => Ok(None),
            Reply::Answer(SecretAnswer::Failed { message }) => Err(StoreError(message)),
            Reply::Declined | Reply::Answer(SecretAnswer::Declined) => {
                tracing::info!(target: "pw_db", %account, "the desktop declined a secret request");
                Ok(None)
            }
            Reply::Answer(SecretAnswer::Done) => Err(StoreError(
                "the desktop answered a read with `done`".to_owned(),
            )),
        }
    }

    fn set(&self, account: &str, value: &Secret) -> Result<(), StoreError> {
        let op = SecretRequestOp::Store {
            item: value.clone(),
        };
        let call = window_call().ok_or_else(outside_a_request)?;
        match self.ask(account, &call, op)? {
            Reply::Answer(SecretAnswer::Done) => Ok(()),
            Reply::Answer(SecretAnswer::Failed { message }) => Err(StoreError(message)),
            _ => Err(StoreError(
                "the desktop did not store the secret".to_owned(),
            )),
        }
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        let call = window_call().ok_or_else(outside_a_request)?;
        match self.ask(account, &call, SecretRequestOp::Remove)? {
            Reply::Answer(SecretAnswer::Done | SecretAnswer::Missing) => Ok(()),
            Reply::Answer(SecretAnswer::Failed { message }) => Err(StoreError(message)),
            _ => Err(StoreError(
                "the desktop did not remove the secret".to_owned(),
            )),
        }
    }

    fn register(self: Arc<Self>, router: &mut Router) {
        register(router, &self);
    }
}

/// `secret.requests` 하나가 받는 것. 버리면 그 Desktop 이 떨어진다(받은 청은 거절로 센다).
struct Listener {
    secrets: DesktopSecrets,
    session: SessionId,
    rx: mpsc::UnboundedReceiver<SecretRequest>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.secrets.unlisten(self.session);
    }
}

/// owner · Host 범위 연결만(붙은 Desktop 자신의 연결 — 프로젝트 창 · 에이전트의 연결은 아니다).
fn host_scope(ctx: &Ctx) -> Result<(), RpcError> {
    match ctx.scope() {
        ConnectionScope::Host => Ok(()),
        _ => Err(RpcError::new(
            code::PERMISSION_DENIED,
            "secret requests go to the attached desktop's host connection",
        )),
    }
}

/// `secret.requests` · `secret.answer` 를 붙인다.
pub fn register(router: &mut Router, secrets: &DesktopSecrets) {
    let s = secrets.clone();
    router.stream(SecretRequests, move |ctx, (), sink| {
        let s = s.clone();
        async move {
            host_scope(&ctx)?;
            let mut listener = s.listen(ctx.session());
            while let Some(request) = listener.rx.recv().await {
                sink.send(request).await?;
            }
            Ok(())
        }
    });
    let s = secrets.clone();
    router.unary(SecretAnswerMethod, move |ctx, p: SecretAnswerParams| {
        let s = s.clone();
        async move {
            host_scope(&ctx)?;
            if s.answer(ctx.session(), p.id, p.answer) {
                Ok(())
            } else {
                Err(RpcError::new(
                    code::NOT_FOUND,
                    "no such open secret request on this connection",
                ))
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_parsed_strictly() {
        assert_eq!(
            parse_account("p1/prod/password"),
            Some(("p1".into(), "prod".into(), DbSecretKey::Password))
        );
        for bad in [
            "p1/prod",
            "p1/prod/token",
            "m/p1/prod/password",
            "/prod/password",
        ] {
            assert_eq!(parse_account(bad), None, "{bad}");
        }
    }

    #[test]
    fn debug_hides_secret_values() {
        let op = SecretRequestOp::Store {
            item: Secret::new("hunter2"),
        };
        let answer = SecretAnswer::Value {
            item: Secret::new("hunter2"),
        };
        let text = format!("{op:?} {answer:?}");
        assert!(!text.contains("hunter2"), "{text}");
    }
}
