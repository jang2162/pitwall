//! 세션: 연결 하나의 디스패치 상태 기계. 진행 중 요청 · 스트림 · 구독을 갖는다.

use std::collections::hash_map::Entry as MapEntry;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::error::RpcError;
use crate::events::EventHub;
use crate::message::{self, Id, Incoming, Outbound, Response, notification};
use crate::protocol::{
    CancelRequestParams, EVENTS_SUBSCRIBE, EVENTS_UNSUBSCRIBE, REQUEST_CANCEL, STREAM_CANCEL,
    STREAM_CREDIT, STREAM_END, StreamCancelParams, StreamCreditParams, StreamEnd, StreamId,
    StreamOpened, StreamOutcome, SubscribeParams, Subscribed, SubscriptionId, UnsubscribeParams,
};
use crate::router::{Caller, Ctx, Handler, RawSink, Router, StreamFn, StreamState, UnaryFn};

/// 잠금 중 패닉해도 표는 삽입 · 삭제 단위로만 바뀌어 일관성이 깨지지 않는다. 그래서 오염을 무시한다.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 연결 하나. transport 는 받은 메시지를 `handle_*` 로 넣고, `new` 가 돌려준 수신기에서 보낼 메시지를 꺼낸다.
///
/// `handle_*` 는 동기다 — 핸들러는 tokio 태스크로 돌리고, 제어 메시지(취소 · credit)는 반환 전에 적용된다.
/// 그러므로 tokio 런타임 안에서 불러야 한다. 세션을 버리면 [`Session::close`] 와 같다.
///
/// 진행 중인 unary 요청과 같은 id 로 다시 요청하면 `INVALID_REQUEST` 로 거부하는데, 그 거부 응답도 같은 id 로
/// 나가 클라이언트 쪽에서는 원 요청의 응답과 구분되지 않는다. id 를 겹치게 쓰는 것은 명세상 클라이언트 잘못이다.
/// 스트림 요청의 id 는 응답 즉시 끝나므로 중복 검사 대상이 아니다.
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    router: Arc<Router>,
    events: EventHub,
    caller: Caller,
    // 무한 채널이다. 스트림 청크는 credit 으로 이미 묶이고, 응답은 요청 수만큼만 나간다.
    // 이벤트에는 백프레셔가 없다 — 느린 소비자의 이벤트는 쌓인다.
    out: UnboundedSender<Outbound>,
    root: CancellationToken,
    requests: Mutex<HashMap<Id, CancellationToken>>,
    streams: Mutex<HashMap<StreamId, StreamEntry>>,
    next_stream: AtomicU64,
    subscriptions: Mutex<HashSet<SubscriptionId>>,
}

struct StreamEntry {
    state: Arc<StreamState>,
    cancel: CancellationToken,
}

impl Session {
    pub fn new(
        router: Arc<Router>,
        events: EventHub,
        caller: Caller,
    ) -> (Self, UnboundedReceiver<Outbound>) {
        let (out, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            router,
            events,
            caller,
            out,
            root: CancellationToken::new(),
            requests: Mutex::default(),
            streams: Mutex::default(),
            next_stream: AtomicU64::new(1),
            subscriptions: Mutex::default(),
        });
        (Self { inner }, rx)
    }

    /// 받은 텍스트 프레임 하나를 처리한다.
    pub fn handle_text(&self, text: &str) {
        match serde_json::from_str::<Value>(text) {
            Ok(value) => self.handle_value(value),
            Err(e) => self
                .inner
                .reply(Id::Null, Err(RpcError::parse_error(e.to_string()))),
        }
    }

    /// 받은 JSON 값 하나를 처리한다.
    pub fn handle_value(&self, value: Value) {
        // 배치는 응답을 배열로 모아 한 번에 돌려줘야 하는데, 스트림 · 취소와 섞이면 그 묶음의 경계가 없다.
        // 클라이언트(UI · MCP · Relay)는 모두 우리가 만드므로 배치를 쓰지 않는다.
        if value.is_array() {
            self.inner.reply(
                Id::Null,
                Err(RpcError::invalid_request(
                    "batch requests are not supported",
                )),
            );
            return;
        }
        match message::parse(value) {
            Ok(Some(incoming)) => self.inner.dispatch(incoming),
            Ok(None) => {}
            Err((id, e)) => self.inner.reply(id, Err(e)),
        }
    }

    /// 진행 중인 요청과 스트림을 모두 취소하고 구독을 걷는다. 두 번 불러도 된다.
    pub fn close(&self) {
        self.inner.root.cancel();
        let subs: Vec<_> = lock(&self.inner.subscriptions).drain().collect();
        for id in subs {
            self.inner.events.unsubscribe(id);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

impl Inner {
    fn reply(&self, id: Id, outcome: Result<Value, RpcError>) {
        // 수신기가 없으면 transport 가 닫힌 것이다. 보낼 곳이 없으므로 버린다.
        let _ = self.out.send(Outbound::Response(Response { id, outcome }));
    }

    fn ctx(&self, cancel: CancellationToken) -> Ctx {
        Ctx {
            caller: self.caller,
            cancel,
            events: self.events.clone(),
        }
    }

    fn dispatch(self: &Arc<Self>, m: Incoming) {
        match m.method.as_str() {
            REQUEST_CANCEL => self.control(m, |this, p: CancelRequestParams| {
                if let Some(token) = lock(&this.requests).get(&p.id) {
                    token.cancel();
                }
                Ok(())
            }),
            STREAM_CREDIT => self.control(m, |this, p: StreamCreditParams| {
                if let Some(stream) = lock(&this.streams).get(&p.stream_id) {
                    let credit = &stream.state.credit;
                    credit.add_permits(clamp_credit(credit.available_permits(), p.credit));
                }
                Ok(())
            }),
            STREAM_CANCEL => self.control(m, |this, p: StreamCancelParams| {
                if let Some(stream) = lock(&this.streams).get(&p.stream_id) {
                    stream.cancel.cancel();
                }
                Ok(())
            }),
            // 알림으로 온 구독은 id 를 돌려줄 곳이 없어 해제할 수도 없다. 받지 않는다.
            EVENTS_SUBSCRIBE if m.id.is_none() => {}
            EVENTS_SUBSCRIBE => self.control(m, |this, p: SubscribeParams| {
                // 확인과 등록을 같은 잠금 안에서 한다. close 는 토큰을 먼저 취소하고 이 잠금으로 걷으므로,
                // 어느 쪽이 먼저 잠그든 close 뒤에 구독이 남지 않는다.
                let mut subscriptions = lock(&this.subscriptions);
                if this.root.is_cancelled() {
                    return Err(RpcError::cancelled());
                }
                let id = this
                    .events
                    .subscribe(p.project_id, p.topics, this.out.clone());
                subscriptions.insert(id);
                Ok(Subscribed {
                    subscription_id: id,
                })
            }),
            EVENTS_UNSUBSCRIBE => self.control(m, |this, p: UnsubscribeParams| {
                // 다른 세션의 구독은 걷지 않는다.
                if lock(&this.subscriptions).remove(&p.subscription_id) {
                    this.events.unsubscribe(p.subscription_id);
                }
                Ok(())
            }),
            _ => self.call(m),
        }
    }

    fn control<P, R>(
        self: &Arc<Self>,
        m: Incoming,
        f: impl FnOnce(&Arc<Self>, P) -> Result<R, RpcError>,
    ) where
        P: DeserializeOwned,
        R: Serialize,
    {
        let result = serde_json::from_value::<P>(m.params)
            .map_err(|e| RpcError::invalid_params(e.to_string()))
            .and_then(|p| f(self, p))
            .and_then(|r| serde_json::to_value(r).map_err(|e| RpcError::internal(e.to_string())));
        if let Some(id) = m.id {
            self.reply(id, result);
        }
    }

    fn call(self: &Arc<Self>, m: Incoming) {
        let router = Arc::clone(&self.router);
        let Some(entry) = router.entry(&m.method) else {
            if let Some(id) = m.id {
                self.reply(id, Err(RpcError::method_not_found(&m.method)));
            }
            return;
        };
        if !entry.meta.permits(self.caller.role) {
            if let Some(id) = m.id {
                let denied =
                    RpcError::new(crate::error::code::PERMISSION_DENIED, "permission denied")
                        .with_data(json!({
                            "method": entry.meta.name,
                            "role": self.caller.role,
                            "allowed": entry.meta.roles,
                        }));
                self.reply(id, Err(denied));
            }
            return;
        }
        match (&entry.handler, m.id) {
            (Handler::Unary(handler), id) => self.call_unary(id, handler, m.params),
            (Handler::Stream(handler), Some(id)) => self.open_stream(id, handler, m.params),
            // 알림으로 온 스트림 요청은 streamId 를 돌려줄 곳이 없다.
            (Handler::Stream(_), None) => {}
        }
    }

    fn call_unary(self: &Arc<Self>, id: Option<Id>, handler: &UnaryFn, params: Value) {
        let token = self.root.child_token();
        if let Some(id) = &id {
            match lock(&self.requests).entry(id.clone()) {
                MapEntry::Occupied(_) => {
                    self.reply(
                        id.clone(),
                        Err(RpcError::invalid_request("duplicate request id")),
                    );
                    return;
                }
                MapEntry::Vacant(slot) => {
                    slot.insert(token.clone());
                }
            }
        }
        let fut = match invoke(|| handler(self.ctx(token.clone()), params)) {
            Ok(fut) => fut,
            Err(e) => {
                self.finish_request(id, Err(e));
                return;
            }
        };
        let mut guard = RequestGuard {
            inner: Arc::clone(self),
            id: Some(id),
        };
        tokio::spawn(async move {
            // biased: 취소를 먼저 본다. 취소된 뒤에는 핸들러를 한 번도 더 poll 하지 않고 버린다 —
            // 핸들러가 다음 await 에서 깨어날 준비가 돼 있어도 그 뒤의 코드는 실행되지 않는다.
            let result = tokio::select! {
                biased;
                () = token.cancelled() => Err(RpcError::cancelled()),
                result = fut => result,
            };
            guard.finish(result);
        });
    }

    fn finish_request(&self, id: Option<Id>, result: Result<Value, RpcError>) {
        if let Some(id) = id {
            lock(&self.requests).remove(&id);
            self.reply(id, result);
        }
    }

    fn open_stream(self: &Arc<Self>, id: Id, handler: &StreamFn, params: Value) {
        let token = self.root.child_token();
        let stream_id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(StreamState::new());
        let sink = RawSink {
            stream_id,
            state: Arc::clone(&state),
            out: self.out.clone(),
        };
        let fut = match invoke(|| handler(self.ctx(token.clone()), params, sink)) {
            Ok(fut) => fut,
            Err(e) => {
                self.reply(id, Err(e));
                return;
            }
        };
        lock(&self.streams).insert(
            stream_id,
            StreamEntry {
                state: Arc::clone(&state),
                cancel: token.clone(),
            },
        );
        // 응답을 태스크를 띄우기 전에 넣는다. 청크가 응답을 앞지르지 않는다.
        self.reply(
            id,
            serde_json::to_value(StreamOpened { stream_id })
                .map_err(|e| RpcError::internal(e.to_string())),
        );
        let mut guard = StreamGuard {
            inner: Arc::clone(self),
            stream_id,
            state,
            finished: false,
        };
        tokio::spawn(async move {
            // biased 인 이유는 call_unary 와 같다. credit 을 기다리던 send 도 함께 버려진다.
            let outcome = tokio::select! {
                biased;
                () = token.cancelled() => StreamOutcome::Cancelled,
                result = fut => match result {
                    Ok(()) => StreamOutcome::Completed,
                    Err(error) => StreamOutcome::Failed { error },
                },
            };
            guard.finish(outcome);
        });
    }

    fn finish_stream(&self, stream_id: StreamId, state: &StreamState, outcome: StreamOutcome) {
        lock(&self.streams).remove(&stream_id);
        {
            // StreamSink::send 와 같은 잠금. end 를 보낸 뒤에는 어떤 sink 도 청크를 보내지 못한다.
            let mut ended = lock(&state.ended);
            *ended = true;
            let _ = self
                .out
                .send(notification(STREAM_END, &StreamEnd { stream_id, outcome }));
        }
        // credit 을 기다리던 떼어 낸 send 를 깨워 오류로 끝낸다.
        state.credit.close();
    }
}

/// 핸들러의 동기 구간(future 를 만들기까지)을 부른다. 여기서 난 패닉은 transport 의 `handle_*` 호출자로
/// 새지 않고 그 요청의 `INTERNAL_ERROR` 가 된다. async 본문의 패닉은 `RequestGuard`/`StreamGuard` 가 받는다.
fn invoke<T>(f: impl FnOnce() -> Result<T, RpcError>) -> Result<T, RpcError> {
    catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(RpcError::internal("handler panicked")))
}

/// 더할 credit 을 semaphore 상한(`Semaphore::MAX_PERMITS`) 안으로 줄인다. 넘기면 `add_permits` 가 패닉한다.
/// 64비트에서는 상한이 2^61 이라 u32 credit 으로 닿기 어렵지만, 32비트 usize 에서는 2^29 라
/// `credit: u32::MAX` 한 번으로 닿는다.
fn clamp_credit(available: usize, requested: u32) -> usize {
    let room = Semaphore::MAX_PERMITS - available;
    usize::try_from(requested).unwrap_or(usize::MAX).min(room)
}

/// 핸들러가 패닉해 태스크가 풀려도 응답이 나가게 한다. 응답 없는 요청은 클라이언트를 영원히 기다리게 한다.
struct RequestGuard {
    inner: Arc<Inner>,
    // 바깥 Option 이 None 이면 이미 끝냈다. 안쪽 None 은 알림(응답 없음)이다.
    id: Option<Option<Id>>,
}

impl RequestGuard {
    fn finish(&mut self, result: Result<Value, RpcError>) {
        if let Some(id) = self.id.take() {
            self.inner.finish_request(id, result);
        }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.finish(Err(RpcError::internal("handler panicked")));
    }
}

/// `RequestGuard` 의 스트림판. 패닉해도 `stream.end`(failed) 가 나간다.
struct StreamGuard {
    inner: Arc<Inner>,
    stream_id: StreamId,
    state: Arc<StreamState>,
    finished: bool,
}

impl StreamGuard {
    fn finish(&mut self, outcome: StreamOutcome) {
        if !self.finished {
            self.finished = true;
            self.inner
                .finish_stream(self.stream_id, &self.state, outcome);
        }
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.finish(StreamOutcome::Failed {
            error: RpcError::internal("handler panicked"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_is_clamped_to_the_semaphore_limit() {
        let near_full = Semaphore::MAX_PERMITS - 3;
        assert_eq!(clamp_credit(near_full, 10), 3);
        assert_eq!(clamp_credit(0, 10), 10);

        // 실제 semaphore 에 더해도 패닉하지 않는다.
        let sem = Semaphore::new(near_full);
        sem.add_permits(clamp_credit(sem.available_permits(), u32::MAX));
        assert_eq!(sem.available_permits(), Semaphore::MAX_PERMITS);
    }
}
