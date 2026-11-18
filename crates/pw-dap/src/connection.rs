//! 어댑터와의 연결 하나(stdio 의 파이프 둘 또는 TCP 소켓 하나): 요청 표 · 읽기 · 쓰기.
//!
//! - **읽기** — `pw_wire` 의 틀로 본문을 가른다. 틀 오류(머리 · 본문 상한을 넘음 · 깨진 머리 · 틀 중간의 끝)는 어느 것이든 그
//!   어댑터를 트리째 죽이고 연결을 닫는다(`pw_wire` 머리 주석의 규칙 — 언어 서버와 같다). 본문이 DAP 메시지가 아니면 그 메시지만
//!   버리고 로그를 남긴다. 응답은 요청 표에서 기다리는 쪽에 넘기고, 이벤트와 역요청은 [`Inbound`] 로 세션에 넘긴다 — 받는 채널은
//!   크기가 있어 세션이 따라오지 못하면 읽기가 기다린다(어댑터의 쓰기가 막힌다 — Host 메모리가 늘지 않는다).
//! - **쓰기** — 보낼 틀이 [`MAX_QUEUED_BYTES`] 넘게 쌓이면(어댑터가 입력을 읽지 않는다) 망가진 것으로 보고 죽인다.
//! - **요청** — `seq` 는 연결마다 1 부터 오른다. 요청마다 시한이 있고, 시한이 지나거나 부른 쪽이 future 를 버리면 표에서 지운다(그
//!   뒤에 온 응답은 세기만 하고 버린다). DAP 의 `cancel` 은 보내지 않는다 — 지원하는 어댑터가 드물고, 시한을 넘긴 요청은 결과를
//!   버리면 된다.
//! - **닫기** — 처음 닫은 까닭이 남는다. 닫히면 대기 중인 요청은 모두 [`RequestError::Closed`] 로 끝나고 새 요청도 그렇다.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use pw_wire::{FrameReader, write_frame};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use crate::protocol::{self, ErrorResponse, Incoming};

/// 보낼 틀이 쌓일 수 있는 바이트. 넘으면 어댑터를 끝낸다.
pub const MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;
/// 틀 하나의 머리 몫.
const FRAME_OVERHEAD: usize = 64;
/// 세션으로 넘기는 이벤트 · 역요청의 채널 크기.
const INBOUND_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// 시한 안에 답이 없었다.
    Timeout,
    /// 어댑터가 실패로 답했다.
    Adapter(ErrorResponse),
    /// 연결이 닫혔다(까닭).
    Closed(String),
    /// 인자 · 응답 본문을 형으로 바꾸지 못했다.
    Decode(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("the debug adapter did not answer in time"),
            Self::Adapter(e) => write!(f, "the debug adapter answered with an error: {e}"),
            Self::Closed(why) => write!(f, "the debug adapter is gone: {why}"),
            Self::Decode(detail) => write!(f, "cannot decode: {detail}"),
        }
    }
}

impl std::error::Error for RequestError {}

/// 연결이 닫힌 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closed {
    /// 우리가 닫았다(끊기).
    Stopped,
    /// 어댑터가 연결을 닫았거나 끝났다(까닭).
    Gone(String),
    /// 프로토콜 위반으로 우리가 죽였다(까닭).
    Killed(String),
}

impl Closed {
    fn reason(&self) -> String {
        match self {
            Self::Stopped => "the debug session was stopped".to_owned(),
            Self::Gone(why) | Self::Killed(why) => why.clone(),
        }
    }
}

/// 연결이 세션에 넘기는 것.
#[derive(Debug)]
pub(crate) enum Inbound {
    Event {
        event: String,
        body: Value,
    },
    Request {
        seq: i64,
        command: String,
        arguments: Value,
    },
}

type Pending = HashMap<i64, oneshot::Sender<Result<Value, ErrorResponse>>>;

enum Outgoing {
    Frame(Vec<u8>, usize),
    /// 쓰는 쪽을 닫는다(TCP 면 어댑터가 EOF 를 본다).
    Close,
}

struct Shared {
    label: String,
    outgoing: mpsc::UnboundedSender<Outgoing>,
    queued: Arc<AtomicUsize>,
    seq: AtomicI64,
    pending: Mutex<Pending>,
    closed: watch::Sender<Option<Closed>>,
    /// 어댑터 프로세스를 트리째 죽인다(이미 열린 포트에 붙기만 했으면 아무것도 하지 않는다).
    kill: Box<dyn Fn() + Send + Sync>,
    reader: Mutex<Option<AbortHandle>>,
    late: AtomicU64,
    unreadable: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn send(&self, body: Vec<u8>) -> Result<(), RequestError> {
        if let Some(closed) = &*self.closed.borrow() {
            return Err(RequestError::Closed(closed.reason()));
        }
        let cost = body.capacity() + FRAME_OVERHEAD;
        let before = self.queued.fetch_add(cost, Ordering::SeqCst);
        if before + cost > MAX_QUEUED_BYTES {
            self.queued.fetch_sub(cost, Ordering::SeqCst);
            let reason = format!(
                "the debug adapter stopped reading its input ({} MiB waiting to be sent)",
                before >> 20
            );
            self.violate(reason.clone());
            return Err(RequestError::Closed(reason));
        }
        self.outgoing
            .send(Outgoing::Frame(body, cost))
            .map_err(|_| {
                self.queued.fetch_sub(cost, Ordering::SeqCst);
                RequestError::Closed("the connection is closed".to_owned())
            })
    }

    /// 닫는다(처음 까닭이 남는다): 새 요청을 막고, 대기 중인 요청을 모두 실패시키고, 쓰는 쪽과 읽기를 멈춘다.
    fn close(&self, closed: Closed) {
        let first = self.closed.send_if_modified(|now| {
            if now.is_none() {
                *now = Some(closed.clone());
                true
            } else {
                false
            }
        });
        if !first {
            return;
        }
        // 받는 쪽이 `Closed` 로 바꾼다(보낸 쪽이 사라짐).
        drop(std::mem::take(&mut *lock(&self.pending)));
        let _ = self.outgoing.send(Outgoing::Close);
        if let Some(reader) = lock(&self.reader).take() {
            reader.abort();
        }
    }

    /// 틀 · 프로토콜 위반: 어댑터를 트리째 죽이고 닫는다.
    fn violate(&self, reason: String) {
        tracing::warn!(target: "pw_dap", adapter = %self.label, %reason, "killing the debug adapter");
        self.close(Closed::Killed(reason));
        (self.kill)();
    }
}

/// 연결 하나의 손잡이(복제해 나눠 쥔다).
#[derive(Clone)]
pub(crate) struct Connection {
    shared: Arc<Shared>,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("label", &self.shared.label)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// 읽기 · 쓰기 태스크를 띄운다. 이벤트 · 역요청은 돌려준 받는 쪽으로 온다(읽기가 끝나면 닫힌다).
    pub(crate) fn open<R, W>(
        label: String,
        reader: R,
        writer: W,
        kill: Box<dyn Fn() + Send + Sync>,
    ) -> (Self, mpsc::Receiver<Inbound>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, outgoing_rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            label,
            outgoing,
            queued: Arc::clone(&queued),
            seq: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            closed: watch::channel(None).0,
            kill,
            reader: Mutex::new(None),
            late: AtomicU64::new(0),
            unreadable: AtomicU64::new(0),
        });
        let (inbound_tx, inbound) = mpsc::channel(INBOUND_CAPACITY);
        tokio::spawn(write_loop(writer, outgoing_rx, queued));
        let read = tokio::spawn(read_loop(Arc::clone(&shared), reader, inbound_tx));
        *lock(&shared.reader) = Some(read.abort_handle());
        // 읽기가 이미 끝났으면(곧바로 EOF) 손잡이는 쓸모없다 — 그대로 둬도 abort 는 아무 일도 하지 않는다.
        (Self { shared }, inbound)
    }

    /// 요청 하나. `arguments` 가 `null` 이면 싣지 않는다.
    pub(crate) async fn request(
        &self,
        command: &str,
        arguments: &Value,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        let seq = self.shared.seq.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        lock(&self.shared.pending).insert(seq, tx);
        let guard = InFlight {
            shared: &self.shared,
            seq: Some(seq),
        };
        self.shared
            .send(protocol::request(seq, command, arguments))?;
        let answer = tokio::time::timeout(timeout, rx).await;
        match answer {
            Err(_) => Err(RequestError::Timeout),
            Ok(Err(_)) => Err(RequestError::Closed(self.close_reason())),
            Ok(Ok(result)) => {
                guard.answered();
                result.map_err(RequestError::Adapter)
            }
        }
    }

    /// 역요청 `request_seq` 에 답한다.
    pub(crate) fn respond(&self, request_seq: i64, command: &str, result: &Result<Value, String>) {
        let seq = self.shared.seq.fetch_add(1, Ordering::SeqCst);
        let _ = self
            .shared
            .send(protocol::response(seq, request_seq, command, result));
    }

    pub(crate) fn close(&self, closed: Closed) {
        self.shared.close(closed);
    }

    pub(crate) fn violate(&self, reason: String) {
        self.shared.violate(reason);
    }

    pub(crate) fn closed(&self) -> Option<Closed> {
        self.shared.closed.borrow().clone()
    }

    pub(crate) fn close_reason(&self) -> String {
        self.closed()
            .map_or_else(|| "the connection is closed".to_owned(), |c| c.reason())
    }

    /// 닫힐 때까지 기다린다.
    pub(crate) async fn wait_closed(&self) -> Closed {
        let mut rx = self.shared.closed.subscribe();
        loop {
            if let Some(closed) = rx.borrow_and_update().clone() {
                return closed;
            }
            if rx.changed().await.is_err() {
                return Closed::Gone("the connection is closed".to_owned());
            }
        }
    }

    /// 표에 없어 버린 응답의 수.
    pub(crate) fn late_responses(&self) -> u64 {
        self.shared.late.load(Ordering::SeqCst)
    }

    /// DAP 메시지가 아니어서 버린 본문의 수.
    pub(crate) fn unreadable_messages(&self) -> u64 {
        self.shared.unreadable.load(Ordering::SeqCst)
    }
}

/// 요청 하나가 표에 있는 동안의 손잡이. 답을 받기 전에 버려지면 표에서 지운다.
struct InFlight<'a> {
    shared: &'a Shared,
    seq: Option<i64>,
}

impl InFlight<'_> {
    fn answered(mut self) {
        self.seq = None;
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Some(seq) = self.seq.take() {
            lock(&self.shared.pending).remove(&seq);
        }
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut outgoing: mpsc::UnboundedReceiver<Outgoing>,
    queued: Arc<AtomicUsize>,
) {
    while let Some(next) = outgoing.recv().await {
        match next {
            Outgoing::Frame(body, cost) => {
                let written = write_frame(&mut writer, &body).await;
                drop(body);
                queued.fetch_sub(cost, Ordering::SeqCst);
                if let Err(e) = written {
                    tracing::debug!(target: "pw_dap", %e, "cannot write to the debug adapter");
                    break;
                }
            }
            Outgoing::Close => break,
        }
    }
    let _ = writer.shutdown().await;
}

async fn read_loop<R: AsyncRead + Unpin>(
    shared: Arc<Shared>,
    reader: R,
    inbound: mpsc::Sender<Inbound>,
) {
    let mut frames = FrameReader::new(reader);
    loop {
        let body = match frames.read_frame().await {
            Ok(Some(body)) => body,
            Ok(None) => {
                shared.close(Closed::Gone(
                    "the debug adapter closed the connection".to_owned(),
                ));
                return;
            }
            Err(e) => {
                shared.violate(format!("protocol error: {e}"));
                return;
            }
        };
        let message = match protocol::parse(&body) {
            Ok(message) => message,
            Err(e) => {
                shared.unreadable.fetch_add(1, Ordering::SeqCst);
                tracing::debug!(target: "pw_dap", adapter = %shared.label, %e, "dropped an unreadable message");
                continue;
            }
        };
        let forward = match message {
            Incoming::Response {
                request_seq,
                result,
                ..
            } => {
                let waiter = lock(&shared.pending).remove(&request_seq);
                match waiter {
                    Some(waiter) => {
                        let _ = waiter.send(result);
                    }
                    None => {
                        shared.late.fetch_add(1, Ordering::SeqCst);
                        tracing::debug!(target: "pw_dap", adapter = %shared.label, request_seq, "dropped a late or unknown response");
                    }
                }
                continue;
            }
            Incoming::Event { event, body } => Inbound::Event { event, body },
            Incoming::Request {
                seq,
                command,
                arguments,
            } => Inbound::Request {
                seq,
                command,
                arguments,
            },
        };
        if inbound.send(forward).await.is_err() {
            // 세션이 없어졌다.
            return;
        }
    }
}
