//! Host 연결 위의 작은 JSON-RPC 클라이언트: 요청을 보내고 같은 id 의 응답을 기다린다.
//!
//! 구독은 다루지 않는다 — 이벤트 프레임은 버린다. 스트림은 [`HostClient::stream`] 으로 받는다(세션 툴 `checkout_send` 가 원본의
//! `git.send.prepare` 를 받는다 — 18-m7-plan M7-19): 청크 하나를 꺼낼 때마다 credit 을 하나 더 준다(한 번에 하나만 와 있다).
//! 연결이 닫히면 기다리던 요청 · 스트림은 모두 [`CallError::Closed`] 로 끝난다.
//!
//! **취소**: 응답을 받기 전에 호출이 끝나면 — 넘긴 취소 토큰이 취소됐거나([`HostClient::call_until`]) 호출 future 가 버려졌으면 —
//! 그 요청 id 로 `request.cancel`(`pw_rpc::protocol`) 알림을 보낸다. Host 는 그 핸들러 future 를 버린다. MCP 의 취소 ·
//! CLI 의 Ctrl+C · `db_query` 의 취소 · 승인 대기 거두기가 모두 이 길을 쓴다(16-m5-plan M5-03).
//!
//! **취소 뒤 답 기다리기**([`HostClient::call_until_settled`] — 16-m5-plan M5-18b): 핸들러가 「되돌릴 수 없는 지점」
//! (`pw_rpc::PointOfNoReturn` — `db_execute` 의 커밋)을 넘은 뒤에는 Host 가 취소로 버리지 않고 끝까지 돌려 답한다. 그 결과를
//! 감사 행에 남기려고, 취소를 보낸 뒤에도 그 요청의 답을 시한까지 기다린다. Host 가 `cancelled` 로 답하면(지점 전이라 버려졌다)
//! 취소, 결과나 다른 오류로 답하면 그 답, 시한까지 답이 없으면 「모름」 이다. 결과로 답한 경우는 둘로 가른다: 취소를 id 를 붙인
//! `request.cancel` 요청으로 보내면 Host 는 그 취소를 적용한 자리에서 `null` 로 답하므로(`pw_rpc::protocol`), 그 확인이 답보다
//! **먼저** 왔으면 Host 가 취소를 받은 뒤에 끝낸 것 — 되돌릴 수 없는 지점을 넘은 호출이다. 답이 먼저 왔으면 Host 는 취소가 닿기
//! 전에 이미 끝냈다(표지와 무관하다). 순서는 펌프가 받은 차례 번호로 본다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_core::{Connection, FrameSink};
use pw_rpc::code as rpc_code;
use pw_rpc::protocol::{
    REQUEST_CANCEL, STREAM_CANCEL, STREAM_CHUNK, STREAM_CREDIT, STREAM_END, StreamOutcome,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// 받은 응답과 그 차례(펌프가 받은 순서 — 1부터).
type Delivery = (u64, Result<Value, CallError>);
type Pending = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Delivery>>>>>;
/// 열린 스트림(`streamId` → 청크 · 끝을 받는 쪽). 연결이 닫히면 `None` 으로 걷는다 — 받는 쪽은 [`CallError::Closed`] 로 끝난다.
type Streams = Arc<Mutex<Option<HashMap<u64, mpsc::UnboundedSender<StreamFrame>>>>>;

/// 스트림 하나에 온 것.
enum StreamFrame {
    Chunk(Value),
    End(StreamOutcome),
}

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
    /// Host 가 취소를 받은 **뒤에** 결과(또는 `cancelled` 가 아닌 오류)로 답했다 — 핸들러가 되돌릴 수 없는 지점을 넘어 버려지지
    /// 않고 끝까지 돌았다(`pw_rpc::PointOfNoReturn`).
    AnsweredAfterCancel(Result<Value, CallError>),
    /// 취소를 보냈지만 Host 는 그 취소가 닿기 **전에** 이미 답했다 — 표지와 무관하게 끝까지 돈 호출이다.
    FinishedBeforeCancel(Result<Value, CallError>),
    /// 취소했고 Host 가 그 핸들러를 버렸다(`cancelled` 로 답했다).
    Cancelled,
    /// 취소했고 시한까지 답이 없었다(또는 연결이 닫혔다) — 효과가 났는지 모른다.
    Unsettled,
}

pub struct HostClient {
    sink: Box<dyn FrameSink>,
    pending: Pending,
    streams: Streams,
    next_id: AtomicU64,
    pump: tokio::task::JoinHandle<()>,
}

impl HostClient {
    /// 받는 쪽을 펌프 태스크로 옮긴다. tokio 런타임 안에서 부른다.
    pub fn new(conn: Connection) -> Self {
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let streams: Streams = Arc::new(Mutex::new(Some(HashMap::new())));
        let mut source = conn.source;
        let table = Arc::clone(&pending);
        let open_streams = Arc::clone(&streams);
        let pump = tokio::spawn(async move {
            let mut seq = 0u64;
            while let Some(frame) = source.recv().await {
                if let Some(method) = frame.get("method").and_then(Value::as_str) {
                    if frame.get("id").is_none() {
                        route_stream(&open_streams, method, &frame);
                    }
                    continue;
                }
                let Some(id) = frame.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                seq += 1;
                let waiter = lock(&table).as_mut().and_then(|t| t.remove(&id));
                if let Some(waiter) = waiter {
                    let _ = waiter.send((seq, outcome(frame)));
                }
            }
            // 닫혔다. 기다리던 쪽을 모두 깨우고 이후 요청은 바로 실패하게 표를 걷는다.
            lock(&open_streams).take();
            if let Some(table) = lock(&table).take() {
                for (_, waiter) in table {
                    let _ = waiter.send((u64::MAX, Err(CallError::Closed)));
                }
            }
        });
        Self {
            sink: conn.sink,
            pending,
            streams,
            next_id: AtomicU64::new(1),
            pump,
        }
    }

    /// 연결이 닫혔는가(펌프가 표를 걷었다) — 세션의 연결 풀이 다시 열지 가른다.
    pub fn closed(&self) -> bool {
        lock(&self.pending).is_none()
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
                answer.map_or(Err(CallError::Closed), |(_, answer)| answer)
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
                return Settled::Answered(answer.map_or(Err(CallError::Closed), |(_, answer)| answer));
            }
            () = cancel.cancelled() => {}
        }
        // 표에서 걷지 않고 취소만 보낸다 — 그 요청의 답을 계속 받는다. 시한이 지나 이 함수가 끝나면 지킴이가 표에서 걷는다. 취소는
        // id 를 붙인 요청으로 보낸다 — Host 의 확인(`null`)이 답보다 먼저 오는가로 「취소 뒤에 끝냈다」 를 안다(머리 주석).
        let ack = self.send_cancel_request(id);
        let acked = |seq: u64| ack.as_ref().is_some_and(|ack| ack.received_before(seq));
        match tokio::time::timeout(grace, rx).await {
            Ok(Ok((seq, answer))) => {
                outstanding.answered = true;
                match answer {
                    Err(CallError::Rpc { code, .. }) if code == i64::from(rpc_code::CANCELLED) => {
                        Settled::Cancelled
                    }
                    Err(CallError::Closed) => Settled::Unsettled,
                    answer if acked(seq) => Settled::AnsweredAfterCancel(answer),
                    answer => Settled::FinishedBeforeCancel(answer),
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

    /// 스트림 메서드를 연다. 열기의 응답(`streamId`)을 받은 뒤 credit 을 하나 준다 — 청크는 [`HostStream::next`] 로 꺼낸다.
    /// `cancel` 은 열기까지만 본다(청크를 기다리는 쪽이 따로 거둔다 — 받는 쪽을 버리면 `stream.cancel` 을 보낸다).
    pub async fn stream(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<HostStream<'_>, CallError> {
        let opened = self.call_until(method, params, cancel).await?;
        let Some(stream_id) = opened.get("streamId").and_then(Value::as_u64) else {
            return Err(CallError::Rpc {
                code: i64::from(rpc_code::INTERNAL_ERROR),
                message: format!("`{method}` did not open a stream"),
                data: None,
            });
        };
        let (tx, rx) = mpsc::unbounded_channel();
        match lock(&self.streams).as_mut() {
            Some(table) => {
                table.insert(stream_id, tx);
            }
            None => return Err(CallError::Closed),
        }
        let stream = HostStream {
            client: self,
            stream_id,
            rx,
            done: false,
        };
        // 초기 credit 은 0 이다 — 표에 넣은 뒤에 주므로 청크가 표보다 먼저 오지 않는다.
        stream.credit();
        Ok(stream)
    }

    fn send_cancel(&self, id: u64) {
        self.sink.send(json!({
            "jsonrpc": "2.0",
            "method": REQUEST_CANCEL,
            "params": { "id": id },
        }));
    }

    /// `request.cancel` 을 id 를 붙인 요청으로 보낸다 — Host 는 취소를 적용한 자리에서 `null` 로 답한다. 연결이 닫혔으면 `None`.
    fn send_cancel_request(&self, id: u64) -> Option<CancelAck<'_>> {
        let ack_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).as_mut()?.insert(ack_id, tx);
        self.sink.send(json!({
            "jsonrpc": "2.0",
            "id": ack_id,
            "method": REQUEST_CANCEL,
            "params": { "id": id },
        }));
        Some(CancelAck {
            client: self,
            id: ack_id,
            rx: Mutex::new(rx),
        })
    }
}

/// 스트림 알림(`stream.chunk` · `stream.end`)을 그 스트림의 받는 쪽으로.
fn route_stream(streams: &Streams, method: &str, frame: &Value) {
    let params = frame.get("params");
    let Some(stream_id) = params
        .and_then(|p| p.get("streamId"))
        .and_then(Value::as_u64)
    else {
        return;
    };
    let mut table = lock(streams);
    let Some(table) = table.as_mut() else {
        return;
    };
    match method {
        STREAM_CHUNK => {
            let data = params
                .and_then(|p| p.get("data"))
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(tx) = table.get(&stream_id) {
                let _ = tx.send(StreamFrame::Chunk(data));
            }
        }
        STREAM_END => {
            let outcome = params
                .cloned()
                .and_then(|p| serde_json::from_value::<StreamOutcome>(p).ok())
                .unwrap_or(StreamOutcome::Cancelled);
            if let Some(tx) = table.remove(&stream_id) {
                let _ = tx.send(StreamFrame::End(outcome));
            }
        }
        _ => {}
    }
}

/// 열린 스트림 하나([`HostClient::stream`]). 끝까지 받기 전에 버리면 `stream.cancel` 을 보낸다.
pub struct HostStream<'a> {
    client: &'a HostClient,
    stream_id: u64,
    rx: mpsc::UnboundedReceiver<StreamFrame>,
    done: bool,
}

impl HostStream<'_> {
    /// 다음 청크. 정상으로 끝났으면 `None`, Host 의 실패 · 취소 · 연결 닫힘은 `Some(Err)`.
    pub async fn next(&mut self) -> Option<Result<Value, CallError>> {
        if self.done {
            return None;
        }
        let Some(frame) = self.rx.recv().await else {
            self.done = true;
            return Some(Err(CallError::Closed));
        };
        match frame {
            StreamFrame::Chunk(data) => {
                self.credit();
                Some(Ok(data))
            }
            StreamFrame::End(outcome) => {
                self.done = true;
                match outcome {
                    StreamOutcome::Completed => None,
                    StreamOutcome::Failed { error } => Some(Err(CallError::Rpc {
                        code: i64::from(error.code),
                        message: error.message,
                        data: error.data,
                    })),
                    StreamOutcome::Cancelled => Some(Err(CallError::Rpc {
                        code: i64::from(rpc_code::CANCELLED),
                        message: "cancelled".into(),
                        data: None,
                    })),
                }
            }
        }
    }

    fn credit(&self) {
        self.client.sink.send(json!({
            "jsonrpc": "2.0",
            "method": STREAM_CREDIT,
            "params": { "streamId": self.stream_id, "credit": 1 },
        }));
    }
}

impl Drop for HostStream<'_> {
    fn drop(&mut self) {
        let open = lock(&self.client.streams)
            .as_mut()
            .map(|table| table.remove(&self.stream_id));
        if !self.done && open.is_some() {
            self.client.sink.send(json!({
                "jsonrpc": "2.0",
                "method": STREAM_CANCEL,
                "params": { "streamId": self.stream_id },
            }));
        }
    }
}

/// id 를 붙여 보낸 취소의 확인. 버려지면 표에서 걷는다.
struct CancelAck<'a> {
    client: &'a HostClient,
    id: u64,
    rx: Mutex<oneshot::Receiver<Delivery>>,
}

impl CancelAck<'_> {
    /// 확인이 차례 `seq` 의 응답보다 먼저 왔는가. 펌프는 받은 차례대로 넘기므로, 그 응답을 받은 지금 확인이 없으면 확인이 뒤다.
    fn received_before(&self, seq: u64) -> bool {
        matches!(lock(&self.rx).try_recv(), Ok((ack, _)) if ack < seq)
    }
}

impl Drop for CancelAck<'_> {
    fn drop(&mut self) {
        if let Some(table) = lock(&self.client.pending).as_mut() {
            table.remove(&self.id);
        }
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

#[cfg(test)]
mod tests {
    //! 취소 뒤 답 기다리기([`HostClient::call_until_settled`])를 Host 프레임의 순서를 쥔 가짜 연결로 본다.

    use std::sync::Arc;
    use std::time::Duration;

    use pw_core::{BoxFuture, Connection, FrameSink, FrameSource};
    use serde_json::{Value, json};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
    use tokio_util::sync::CancellationToken;

    use super::{HostClient, Settled, lock};

    struct Sink(UnboundedSender<Value>);

    impl FrameSink for Sink {
        fn send(&self, frame: Value) {
            let _ = self.0.send(frame);
        }
    }

    struct Source(UnboundedReceiver<Value>);

    impl FrameSource for Source {
        fn recv(&mut self) -> BoxFuture<'_, Option<Value>> {
            Box::pin(self.0.recv())
        }
    }

    /// 가짜 Host: 클라이언트가 보낸 프레임을 받고(`sent`), Host 의 프레임을 보낸다(`host`).
    struct Fake {
        client: Arc<HostClient>,
        sent: UnboundedReceiver<Value>,
        host: Option<UnboundedSender<Value>>,
    }

    impl Fake {
        fn new() -> Self {
            let (to_host, sent) = unbounded_channel();
            let (host, from_host) = unbounded_channel();
            let client = Arc::new(HostClient::new(Connection {
                sink: Box::new(Sink(to_host)),
                source: Box::new(Source(from_host)),
            }));
            Self {
                client,
                sent,
                host: Some(host),
            }
        }

        async fn next_sent(&mut self) -> Value {
            tokio::time::timeout(Duration::from_secs(5), self.sent.recv())
                .await
                .expect("the client sent nothing")
                .unwrap()
        }

        fn answer(&self, frame: Value) {
            self.host.as_ref().unwrap().send(frame).unwrap();
        }

        /// 호출을 띄우고 취소한다. (호출 id, 취소 확인 요청의 id).
        async fn cancelled_call(
            &mut self,
            grace: Duration,
        ) -> (tokio::task::JoinHandle<Settled>, u64, u64) {
            let cancel = CancellationToken::new();
            let client = Arc::clone(&self.client);
            let token = cancel.clone();
            let call = tokio::spawn(async move {
                client
                    .call_until_settled("test.call", Value::Null, &token, grace)
                    .await
            });
            let request = self.next_sent().await;
            assert_eq!(request["method"], "test.call");
            cancel.cancel();
            let ack = self.next_sent().await;
            assert_eq!(ack["method"], "request.cancel", "{ack}");
            assert_eq!(ack["params"]["id"], request["id"]);
            (
                call,
                request["id"].as_u64().unwrap(),
                ack["id"]
                    .as_u64()
                    .expect("the cancel is a request with an id"),
            )
        }
    }

    const GRACE: Duration = Duration::from_secs(5);

    /// Host 가 취소를 받은 뒤(확인이 먼저) 결과로 답했다 — 되돌릴 수 없는 지점을 넘은 호출.
    #[tokio::test]
    async fn an_answer_after_the_cancel_ack_ran_past_the_point_of_no_return() {
        let mut f = Fake::new();
        let (call, id, ack) = f.cancelled_call(GRACE).await;
        f.answer(json!({ "jsonrpc": "2.0", "id": ack, "result": null }));
        f.answer(json!({ "jsonrpc": "2.0", "id": id, "result": 7 }));
        assert_eq!(
            call.await.unwrap(),
            Settled::AnsweredAfterCancel(Ok(json!(7)))
        );
    }

    /// Host 가 취소를 받기 전에(답이 먼저) 이미 끝냈다 — 표지와 무관하다.
    #[tokio::test]
    async fn an_answer_before_the_cancel_ack_finished_before_the_cancel() {
        let mut f = Fake::new();
        let (call, id, ack) = f.cancelled_call(GRACE).await;
        f.answer(json!({ "jsonrpc": "2.0", "id": id, "result": 7 }));
        f.answer(json!({ "jsonrpc": "2.0", "id": ack, "result": null }));
        assert_eq!(
            call.await.unwrap(),
            Settled::FinishedBeforeCancel(Ok(json!(7)))
        );
    }

    /// Host 가 핸들러를 버리고 `cancelled` 로 답했다.
    #[tokio::test]
    async fn a_cancelled_answer_is_a_cancel() {
        let mut f = Fake::new();
        let (call, id, ack) = f.cancelled_call(GRACE).await;
        f.answer(json!({ "jsonrpc": "2.0", "id": ack, "result": null }));
        f.answer(json!({ "jsonrpc": "2.0", "id": id,
            "error": { "code": pw_rpc::code::CANCELLED, "message": "cancelled" } }));
        assert_eq!(call.await.unwrap(), Settled::Cancelled);
    }

    /// 취소 뒤 기다리는 동안 Host 연결이 닫히면 효과가 났는지 모른다(리뷰 R3) — 취소가 아니다.
    #[tokio::test]
    async fn the_connection_closing_while_waiting_is_unsettled() {
        let mut f = Fake::new();
        let (call, _, ack) = f.cancelled_call(GRACE).await;
        f.answer(json!({ "jsonrpc": "2.0", "id": ack, "result": null }));
        f.host = None;
        assert_eq!(call.await.unwrap(), Settled::Unsettled);
    }

    /// 시한까지 답이 없으면 모른다. 그 요청과 취소 확인은 대기 표에서 걷힌다(늦은 답은 주인 없이 버려진다).
    #[tokio::test(start_paused = true)]
    async fn no_answer_in_time_is_unsettled_and_leaves_nothing_waiting() {
        let mut f = Fake::new();
        let (call, id, _) = f.cancelled_call(GRACE).await;
        assert_eq!(call.await.unwrap(), Settled::Unsettled);
        assert!(lock(&f.client.pending).as_ref().unwrap().is_empty());
        // 늦은 답이 와도 아무 일도 없다.
        f.answer(json!({ "jsonrpc": "2.0", "id": id, "result": 7 }));
        tokio::task::yield_now().await;
        assert!(lock(&f.client.pending).as_ref().unwrap().is_empty());
    }
}
