//! Host 연결 위의 작은 JSON-RPC 클라이언트: 요청을 보내고 같은 id 의 응답을 기다린다.
//!
//! 스트림 · 구독은 다루지 않는다(지금 툴은 모두 unary). 응답이 아닌 프레임(이벤트 · 스트림 청크)은 버린다.
//! 연결이 닫히면 기다리던 요청은 모두 [`CallError::Closed`] 로 끝난다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pw_core::{Connection, FrameSink};
use serde_json::{Value, json};
use tokio::sync::oneshot;

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
        }
    }
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

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match lock(&self.pending).as_mut() {
            Some(table) => {
                table.insert(id, tx);
            }
            None => return Err(CallError::Closed),
        }
        self.sink.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        rx.await.unwrap_or(Err(CallError::Closed))
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
