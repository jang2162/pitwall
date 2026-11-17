//! Host 연결 위의 작은 JSON-RPC 클라이언트: 요청을 보내고 같은 id 의 응답을 기다린다.
//!
//! 스트림 · 구독은 다루지 않는다(지금 툴은 모두 unary). 응답이 아닌 프레임(이벤트 · 스트림 청크)은 버린다.
//! 연결이 닫히면 기다리던 요청은 모두 [`CallError::Closed`] 로 끝난다.
//!
//! **취소**: 응답을 받기 전에 호출이 끝나면 — 넘긴 취소 토큰이 취소됐거나([`HostClient::call_until`]) 호출 future 가 버려졌으면 —
//! 그 요청 id 로 `request.cancel`(`pw_rpc::protocol`) 알림을 보낸다. Host 는 그 핸들러 future 를 버린다. MCP 의 취소 ·
//! CLI 의 Ctrl+C · `db_query` 의 취소 · 승인 대기 거두기가 모두 이 길을 쓴다(16-m5-plan M5-03).
//!
//! **취소 뒤 답 기다리기**([`HostClient::call_until_settled`] — 16-m5-plan M5-18b): 핸들러가 「되돌릴 수 없는 지점」
//! (`pw_rpc::PointOfNoReturn` — `db_execute` 의 커밋)을 넘은 뒤에는 Host 가 취소로 버리지 않고 끝까지 돌려 답한다. 그 결과를
//! 감사 행에 남기려고, 취소를 보낸 뒤에도 그 요청의 답을 시한까지 기다린다. Host 가 `cancelled` 로 답하면(지점 전이라 버려졌다)
//! 취소, 결과나 다른 오류로 답하면 그 답, 시한까지 답이 없으면 「모름」 이다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_core::{Connection, FrameSink};
use pw_rpc::code as rpc_code;
use pw_rpc::protocol::REQUEST_CANCEL;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

type Pending = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Result<Value, CallError>>>>>>;

#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// Host 가 돌려준 JSON-RPC 오류.
    Rpc {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    /// 연결이 닫혔다.
    Closed,
    /// 응답 전에 호출을 거뒀다(Host 에는 `request.cancel` 을 보냈다).
    Cancelled,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Rpc {
                code,
                message,
                data,
            } => {
                write!(f, "{message} ({code})")?;
                match data {
                    Some(Value::String(s)) => write!(f, ": {s}"),
                    Some(other) => write!(f, ": {other}"),
                    None => Ok(()),
                }
            }
            CallError::Closed => f.write_str("the Pitwall host connection closed"),
            CallError::Cancelled => f.write_str("cancelled"),
        }
    }
}

/// [`HostClient::call_until_settled`] 의 끝.
#[derive(Debug, Clone, PartialEq)]
pub enum Settled {
    /// 취소 전에 답을 받았다([`HostClient::call_until`] 의 결과와 같다).
    Answered(Result<Value, CallError>),
    /// 취소를 보낸 뒤 Host 가 결과(또는 `cancelled` 가 아닌 오류)로 답했다 — 핸들러가 버려지지 않고 끝까지 돌았다(되돌릴 수
    /// 없는 지점을 넘었거나, 취소가 닿기 전에 끝났다). 그 효과는 났다.
    AnsweredAfterCancel(Result<Value, CallError>),
    /// 취소했고 Host 가 그 핸들러를 버렸다(`cancelled` 로 답했다).
    Cancelled,
    /// 취소했고 시한까지 답이 없었다(또는 연결이 닫혔다) — 효과가 났는지 모른다.
    Unsettled,
}

pub struct HostClient {
    sink: Box<dyn FrameSink>,
    pending: Pending,
    next_id: AtomicU64,
    pump: tokio::task::JoinHandle<()>,
}

impl HostClient {
    /// 받는 쪽을 펌프 태스크로 옮긴다. tokio 런타임 안에서 부른다.
    pub fn new(conn: Connection) -> Self {
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let mut source = conn.source;
        let table = Arc::clone(&pending);
        let pump = tokio::spawn(async move {
            while let Some(frame) = source.recv().await {
                let Some(id) = frame.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                if frame.get("method").is_some() {
                    continue;
                }
                let waiter = lock(&table).as_mut().and_then(|t| t.remove(&id));
                if let Some(waiter) = waiter {
                    let _ = waiter.send(outcome(frame));
                }
            }
            // 닫혔다. 기다리던 쪽을 모두 깨우고 이후 요청은 바로 실패하게 표를 걷는다.
            if let Some(table) = lock(&table).take() {
                for (_, waiter) in table {
                    let _ = waiter.send(Err(CallError::Closed));
                }
            }
        });
        Self {
            sink: conn.sink,
            pending,
            next_id: AtomicU64::new(1),
            pump,
        }
    }

    /// 응답을 기다린다. 이 future 를 응답 전에 버리면 Host 에 `request.cancel` 을 보낸다.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        self.call_until(method, params, &CancellationToken::new())
            .await
    }

    /// [`Self::call`] 과 같고, `cancel` 이 응답보다 먼저 취소되면 Host 에 `request.cancel` 을 보내고 [`CallError::Cancelled`] 로
    /// 끝난다. 응답을 이미 받았으면 응답이 이긴다.
    pub async fn call_until(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match lock(&self.pending).as_mut() {
            Some(table) => {
                table.insert(id, tx);
            }
            None => return Err(CallError::Closed),
        }
        // 보내기 전에 지킴이를 세운다 — 이 뒤 어디서 future 가 버려져도 Host 쪽 핸들러를 거둔다.
        let mut outstanding = Outstanding {
            client: self,
            id,
            answered: false,
        };
        self.sink.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        tokio::select! {
            biased;
            answer = rx => {
                outstanding.answered = true;
                answer.unwrap_or(Err(CallError::Closed))
            }
            () = cancel.cancelled() => Err(CallError::Cancelled),
        }
    }

    /// [`Self::call_until`] 과 같고, `cancel` 이 응답보다 먼저 취소되면 `request.cancel` 을 보낸 뒤 그 요청의 답을 `grace` 까지
    /// 기다린다([`Settled`]). 감사 행을 먼저 쓴 호출(`crate::server` 의 선기록)이 쓴다 — 되돌릴 수 없는 지점을 넘은 호출의 결과를
    /// 그 행에 남기려고. 이 future 를 버리면 `call_until` 과 같다(표에서 걷고 취소를 보낸다 — 이미 보냈으면 한 번 더 가지만 Host 는
    /// 끝난 · 모르는 id 의 취소를 무시한다).
    pub async fn call_until_settled(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
        grace: Duration,
    ) -> Settled {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = oneshot::channel();
        match lock(&self.pending).as_mut() {
            Some(table) => {
                table.insert(id, tx);
            }
            None => return Settled::Answered(Err(CallError::Closed)),
        }
        let mut outstanding = Outstanding {
            client: self,
            id,
            answered: false,
        };
        self.sink.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        tokio::select! {
            biased;
            answer = &mut rx => {
                outstanding.answered = true;
                return Settled::Answered(answer.unwrap_or(Err(CallError::Closed)));
            }
            () = cancel.cancelled() => {}
        }
        // 표에서 걷지 않고 취소만 보낸다 — 그 요청의 답을 계속 받는다. 시한이 지나 이 함수가 끝나면 지킴이가 표에서 걷는다.
        self.send_cancel(id);
        match tokio::time::timeout(grace, rx).await {
            Ok(Ok(answer)) => {
                outstanding.answered = true;
                match answer {
                    Err(CallError::Rpc { code, .. }) if code == i64::from(rpc_code::CANCELLED) => {
                        Settled::Cancelled
                    }
                    Err(CallError::Closed) => Settled::Unsettled,
                    answer => Settled::AnsweredAfterCancel(answer),
                }
            }
            // 연결이 닫혔다(펌프가 표를 걷었다).
            Ok(Err(_)) => {
                outstanding.answered = true;
                Settled::Unsettled
            }
            Err(_) => {
                outstanding.forget();
                Settled::Unsettled
            }
        }
    }

    fn send_cancel(&self, id: u64) {
        self.sink.send(json!({
            "jsonrpc": "2.0",
            "method": REQUEST_CANCEL,
            "params": { "id": id },
        }));
    }
}

/// 응답을 받기 전에 버려지면 그 요청을 표에서 걷고 Host 에 `request.cancel` 을 보낸다.
struct Outstanding<'a> {
    client: &'a HostClient,
    id: u64,
    answered: bool,
}

impl Outstanding<'_> {
    /// 취소를 이미 보냈다 — 표에서 걷기만 한다(다시 보내지 않는다).
    fn forget(&mut self) {
        lock(&self.client.pending)
            .as_mut()
            .map(|table| table.remove(&self.id));
        self.answered = true;
    }
}

impl Drop for Outstanding<'_> {
    fn drop(&mut self) {
        if self.answered {
            return;
        }
        let closed = match lock(&self.client.pending).as_mut() {
            Some(table) => {
                table.remove(&self.id);
                false
            }
            None => true,
        };
        // 연결이 이미 닫혔으면 Host 쪽 세션이 그 핸들러를 모두 거뒀다. 보낼 곳도 없다.
        if !closed {
            self.client.send_cancel(self.id);
        }
    }
}

impl Drop for HostClient {
    fn drop(&mut self) {
        // sink 가 버려지면 Host 쪽 세션이 닫힌다(`pw_core::FrameSink` 계약). 펌프는 그 뒤 None 을 받고 끝나지만 기다리지 않는다.
        self.pump.abort();
    }
}

fn outcome(frame: Value) -> Result<Value, CallError> {
    if let Some(error) = frame.get("error") {
        return Err(CallError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_owned(),
            data: error.get("data").cloned(),
        });
    }
    Ok(frame.get("result").cloned().unwrap_or(Value::Null))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
