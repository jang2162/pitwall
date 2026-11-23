//! HTTP 세션 하나 = 프로세스 안 duplex 하나에 도는 [`crate::server::serve`] 하나. 이 모듈은 JSON-RPC 줄을 옮기기만 한다:
//!
//! - 클라이언트가 POST 한 메시지는 파싱한 값을 다시 직렬화해 한 줄로 쓴다(본문에 줄바꿈을 넣어 다른 메시지를 끼우지 못한다).
//! - 세션이 쓴 줄 가운데 응답(`id` 가 있고 `method` 가 없음)은 그 id 를 기다리는 POST 로, 나머지(서버가 먼저 보내는 알림)는 열린
//!   `GET` SSE 스트림으로 간다. 스트림이 없으면 버린다(`tools/list_changed` 는 다음 `tools/list` 가 따라잡는다).
//! - 답을 기다리던 POST 가 답 전에 버려지면(클라이언트가 끊음) `notifications/cancelled` 를 세션에 써 그 호출을 거둔다 — stdio 의
//!   취소와 같은 길(05-mcp.md 「호출 취소」).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Instant;

use bytes::Bytes;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::connections::{Connections, Counted};
use crate::server::{Services, SessionRequest, serve};

/// duplex 한쪽의 버퍼. 본문 상한보다 크게 — 한 요청을 쓰는 동안 세션이 읽기를 기다리지 않게.
const DUPLEX: usize = super::MAX_BODY * 2;
/// SSE 스트림에 쌓아 두는 메시지 수. 넘으면 버린다(느린 클라이언트가 세션을 막지 않게).
const STREAM_QUEUE: usize = 64;

/// 열린 HTTP 세션들.
#[derive(Default)]
pub(super) struct Table {
    live: Mutex<BTreeMap<String, Arc<HttpSession>>>,
}

pub(super) enum Opened {
    /// 세션이 열렸다 — `initialize` 의 응답.
    Ready(Arc<HttpSession>, Value),
    /// 세션이 `initialize` 에 오류로 답했거나 답하지 않았다(세션은 닫았다).
    Refused(Option<Value>),
    /// [`super::MAX_SESSIONS`] 를 넘는다.
    Full,
}

pub(super) enum Asked {
    Answer(Value),
    /// 그 id 를 이미 기다린다.
    Duplicate,
    /// [`super::MAX_PENDING`] 를 넘는다.
    Busy,
    /// 답 전에 세션이 닫혔다.
    Closed,
}

impl Table {
    pub(super) fn len(&self) -> usize {
        self.live().len()
    }

    pub(super) fn get(&self, id: &str) -> Option<Arc<HttpSession>> {
        let session = self.live().get(id).cloned()?;
        session.touch();
        Some(session)
    }

    fn live(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<HttpSession>>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 새 세션을 열고 `initialize` 를 보낸다. 성공하면 표에 올리고 세운다(`connections`).
    pub(super) async fn open(
        self: &Arc<Self>,
        services: &Arc<Services>,
        connections: &Connections,
        initialize: Value,
        cancel: &CancellationToken,
        limits: super::Limits,
    ) -> Opened {
        if self.len() >= limits.max_sessions {
            return Opened::Full;
        }
        let Ok(id) = session_id() else {
            return Opened::Refused(None);
        };
        let (ours, theirs) = tokio::io::duplex(DUPLEX);
        let (read, write) = tokio::io::split(theirs);
        let session_cancel = cancel.child_token();
        let serving = session_cancel.clone();
        let services = Arc::clone(services);
        tokio::spawn(async move {
            if let Err(e) = serve(services, SessionRequest::default(), (read, write), serving).await
            {
                tracing::warn!(%e, "mcp http session failed");
            }
        });
        let (from_session, to_session) = tokio::io::split(ours);
        let session = Arc::new(HttpSession {
            id: id.clone(),
            writer: tokio::sync::Mutex::new(to_session),
            pending: Mutex::new(HashMap::new()),
            stream: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            cancel: session_cancel,
            counted: Mutex::new(None),
        });
        tokio::spawn(read_session(
            Arc::downgrade(&session),
            Arc::downgrade(self),
            from_session,
        ));
        let answer = tokio::time::timeout(limits.body_timeout, session.ask(initialize)).await;
        match answer {
            Ok(Asked::Answer(answer)) if answer.get("result").is_some() => {
                *session
                    .counted
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(Counted::new(connections));
                let mut live = self.live();
                if live.len() >= limits.max_sessions {
                    drop(live);
                    session.close();
                    return Opened::Full;
                }
                live.insert(id, Arc::clone(&session));
                drop(live);
                Opened::Ready(session, answer)
            }
            Ok(Asked::Answer(answer)) => {
                session.close();
                Opened::Refused(Some(answer))
            }
            _ => {
                session.close();
                Opened::Refused(None)
            }
        }
    }

    /// 세션을 닫고 표에서 뺀다.
    pub(super) fn close(&self, id: &str) -> bool {
        let removed = self.live().remove(id);
        match removed {
            Some(session) => {
                session.close();
                true
            }
            None => false,
        }
    }

    /// 모두 닫는다(토큰을 새로 만듦 · 끔).
    pub(super) fn close_all(&self) {
        let all: Vec<Arc<HttpSession>> = std::mem::take(&mut *self.live()).into_values().collect();
        for session in all {
            session.close();
        }
    }

    /// 기다리는 요청도 스트림도 없이 `idle` 이 지난 세션을 닫는다.
    pub(super) fn close_idle(&self, idle: std::time::Duration) {
        let stale: Vec<String> = self
            .live()
            .iter()
            .filter(|(_, s)| s.idle_for() >= idle && s.quiet())
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.close(&id);
        }
    }
}

pub(super) struct HttpSession {
    id: String,
    writer: tokio::sync::Mutex<WriteHalf<DuplexStream>>,
    /// 답을 기다리는 요청(JSON-RPC id 를 직렬화한 글 → 답을 받을 곳).
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    /// 열린 `GET` SSE 스트림(그 스트림의 번호, 보낼 곳).
    stream: Mutex<Option<(u64, mpsc::Sender<Bytes>)>>,
    last_used: Mutex<Instant>,
    /// 취소하면 `serve` 가 끝난다.
    cancel: CancellationToken,
    /// 세션 수 세기(`initialize` 를 마친 뒤부터).
    counted: Mutex<Option<Counted>>,
}

impl HttpSession {
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    fn touch(&self) {
        *self
            .last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    fn idle_for(&self) -> std::time::Duration {
        self.last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
    }

    fn quiet(&self) -> bool {
        self.pending_map().is_empty()
            && self
                .stream
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none()
    }

    fn pending_map(&self) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<Value>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self) {
        self.cancel.cancel();
        self.pending_map().clear();
        self.stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.counted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    pub(super) fn closed(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// 알림 · 클라이언트의 응답을 쓴다(답을 기다리지 않는다).
    pub(super) async fn send(&self, message: &Value) -> bool {
        self.write(message).await
    }

    async fn write(&self, message: &Value) -> bool {
        let Ok(mut line) = serde_json::to_vec(message) else {
            return false;
        };
        line.push(b'\n');
        let mut writer = self.writer.lock().await;
        tokio::select! {
            () = self.cancel.cancelled() => false,
            written = writer.write_all(&line) => written.is_ok(),
        }
    }

    /// 요청을 쓰고 그 id 의 답을 기다린다. 답 전에 이 future 가 버려지면 그 호출을 취소한다.
    pub(super) async fn ask(self: &Arc<Self>, request: Value) -> Asked {
        let Some(id) = request.get("id").cloned() else {
            return Asked::Closed;
        };
        let key = id.to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_map();
            if pending.contains_key(&key) {
                return Asked::Duplicate;
            }
            if pending.len() >= super::MAX_PENDING {
                return Asked::Busy;
            }
            pending.insert(key.clone(), tx);
        }
        let mut guard = Unanswered {
            session: Arc::clone(self),
            key,
            id: Some(id),
        };
        if !self.write(&request).await {
            return Asked::Closed;
        }
        let answer = tokio::select! {
            () = self.cancel.cancelled() => Asked::Closed,
            answer = rx => answer.map_or(Asked::Closed, Asked::Answer),
        };
        if matches!(answer, Asked::Answer(_)) {
            guard.id = None;
        }
        self.touch();
        answer
    }

    /// `GET` 의 SSE 스트림을 연다. 이미 열려 있으면 `None`(하나만).
    pub(super) fn open_stream(self: &Arc<Self>) -> Option<(mpsc::Receiver<Bytes>, StreamGuard)> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut slot = self.stream.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() || self.closed() {
            return None;
        }
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(STREAM_QUEUE);
        *slot = Some((number, tx));
        Some((
            rx,
            StreamGuard {
                session: Arc::downgrade(self),
                number,
            },
        ))
    }

    fn deliver(&self, line: &[u8], message: &Value) {
        let is_response = message.get("method").is_none() && message.get("id").is_some();
        if is_response {
            let key = message["id"].to_string();
            if let Some(tx) = self.pending_map().remove(&key) {
                let _ = tx.send(message.clone());
            }
            return;
        }
        let stream = self
            .stream
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|(_, tx)| tx.clone());
        if let Some(tx) = stream {
            let mut event = Vec::with_capacity(line.len() + 24);
            event.extend_from_slice(b"event: message\ndata: ");
            event.extend_from_slice(line);
            event.extend_from_slice(b"\n\n");
            let _ = tx.try_send(Bytes::from(event));
        }
    }
}

/// 답 없이 버려진 요청: 기다림을 지우고 세션에 그 호출의 취소를 쓴다.
struct Unanswered {
    session: Arc<HttpSession>,
    key: String,
    id: Option<Value>,
}

impl Drop for Unanswered {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        self.session.pending_map().remove(&self.key);
        if self.session.closed() {
            return;
        }
        let session = Arc::clone(&self.session);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                session
                    .send(&json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/cancelled",
                        "params": { "requestId": id, "reason": "the http request was closed" },
                    }))
                    .await;
            });
        }
    }
}

/// SSE 스트림이 끝나면(클라이언트가 끊음) 그 세션의 스트림 자리를 비운다.
pub(super) struct StreamGuard {
    session: Weak<HttpSession>,
    number: u64,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if let Some(session) = self.session.upgrade() {
            let mut slot = session
                .stream
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|(n, _)| *n == self.number) {
                *slot = None;
            }
            drop(slot);
            session.touch();
        }
    }
}

/// 세션이 쓴 줄을 읽어 나눈다. 세션이 끝나면(EOF) 표에서 뺀다.
async fn read_session(
    session: Weak<HttpSession>,
    table: Weak<Table>,
    from_session: ReadHalf<DuplexStream>,
) {
    let mut reader = BufReader::new(from_session);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Some(session) = session.upgrade() else {
            break;
        };
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        match serde_json::from_slice::<Value>(text) {
            Ok(message) => session.deliver(text, &message),
            Err(_) => tracing::debug!("the mcp session wrote a line that is not json"),
        }
    }
    if let Some(session) = session.upgrade() {
        session.close();
        if let Some(table) = table.upgrade() {
            table.close(&session.id);
        }
    }
}

/// OS 난수 32바이트의 16진 글.
fn session_id() -> std::io::Result<String> {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    }))
}
