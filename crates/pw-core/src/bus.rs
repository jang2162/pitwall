//! 이벤트 버스: 범위(Host · 프로젝트)와 토픽으로 거르는 발행/구독.
//!
//! 구독자마다 상한이 있는 큐를 둔다. 큐가 차면 **가장 오래된 이벤트를 버리고** 버린 수를 다음 수신에서
//! [`Received::Lagged`] 로 알린다. 그래서 느린 구독자가 있어도 `publish` 는 기다리지 않고, 메모리는
//! 구독자 수 × 상한을 넘지 않는다. 오래된 쪽을 버리는 이유: 진행률 · 상태 이벤트는 최신 값이 의미 있다.
//!
//! ⚠ 버려지지 않는 것은 **그 구독 큐 전체의** 가장 새 이벤트뿐이다. 구독이 여러 작업 · 토픽을 받으면, 소비자가
//! 멈춘 사이 다른 발행이 쌓여 특정 작업의 마지막 이벤트(완료 알림 등)도 버려질 수 있다. 「끝났는가」처럼 놓치면
//! 영원히 기다리게 되는 사실은 버스로 기다리지 말고 출처의 상태를 직접 본다(작업은 `TaskHandle::wait`).
//!
//! 이벤트는 같은 `publish` 안에서 pw-rpc [`EventHub`] 로도 나간다 — UI · MCP 의 `events.subscribe` 가 받는 길이다.
//! 프로젝트 범위는 그 `projectId` 로, Host 범위는 `projectId` 없이 구독한 쪽으로 간다. 그 쪽 전달은 pw-rpc 세션의 무한 outbound 큐에 들어가므로
//! 이 상한이 적용되지 않는다(pw-rpc `Session` 의 outbound 주석). transport 가 큐를 바로 비우는 동안만 묶인다.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use pw_rpc::EventHub;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Notify;

use crate::ProjectId;

/// 구독자 하나의 큐 상한 기본값.
pub const DEFAULT_CAPACITY: usize = 256;

/// 이벤트의 범위.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// 프로젝트에 속하지 않는 Host 전체의 이벤트. RPC 로는 `projectId` 없이 구독한 쪽에 나간다.
    Host,
    Project(ProjectId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub scope: Scope,
    pub topic: String,
    pub payload: Value,
}

/// 구독이 받는 것.
#[derive(Debug, Clone, PartialEq)]
pub enum Received {
    Event(Arc<Event>),
    /// 큐가 넘쳐 이 수만큼의 이벤트를 버렸다. 버려진 자리는 다음 이벤트의 바로 앞이다.
    Lagged(u64),
}

/// 무엇을 받을지. 기본값은 모든 범위 · 모든 토픽.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    scope: Option<Scope>,
    topics: Option<HashSet<String>>,
}

impl Filter {
    pub fn all() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// 토픽은 정확히 일치해야 한다(pw-rpc `events.subscribe` 와 같은 규칙).
    #[must_use]
    pub fn topics<I, S>(mut self, topics: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.topics = Some(topics.into_iter().map(Into::into).collect());
        self
    }

    fn matches(&self, scope: &Scope, topic: &str) -> bool {
        self.scope.as_ref().is_none_or(|s| s == scope)
            && self.topics.as_ref().is_none_or(|t| t.contains(topic))
    }
}

/// 이벤트 버스. 복제해도 같은 버스를 가리킨다.
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<BusInner>,
}

struct BusInner {
    capacity: usize,
    hub: Option<EventHub>,
    state: Mutex<BusState>,
}

#[derive(Default)]
struct BusState {
    next_id: u64,
    subscribers: HashMap<u64, Arc<Shared>>,
}

struct Shared {
    filter: Filter,
    queue: Mutex<Queue>,
    notify: Notify,
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Arc<Event>>,
    dropped: u64,
    closed: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // 잠금 안에서는 삽입 · 삭제만 하므로 패닉으로 오염돼도 표가 어긋나지 않는다.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl EventBus {
    /// 구독자마다 큐 상한이 `capacity` 인 버스. pw-rpc 로는 내보내지 않는다.
    ///
    /// # Panics
    /// `capacity` 가 0 이면 패닉한다.
    pub fn new(capacity: usize) -> Self {
        Self::build(capacity, None)
    }

    /// 이벤트를 `hub` 의 RPC 구독에도 내보내는 버스.
    ///
    /// # Panics
    /// `capacity` 가 0 이면 패닉한다.
    pub fn with_rpc_hub(capacity: usize, hub: EventHub) -> Self {
        Self::build(capacity, Some(hub))
    }

    fn build(capacity: usize, hub: Option<EventHub>) -> Self {
        assert!(capacity > 0, "event bus capacity must be positive");
        Self {
            inner: Arc::new(BusInner {
                capacity,
                hub,
                state: Mutex::default(),
            }),
        }
    }

    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// 이벤트를 맞는 구독 전부에 넣는다. 기다리지 않는다. 반환값은 이 버스 안에서 받은 구독 수.
    pub fn publish<T: Serialize>(
        &self,
        scope: Scope,
        topic: &str,
        payload: &T,
    ) -> serde_json::Result<usize> {
        let event = Arc::new(Event {
            scope,
            topic: topic.to_owned(),
            payload: serde_json::to_value(payload)?,
        });
        let mut delivered = 0;
        {
            let state = lock(&self.inner.state);
            for sub in state.subscribers.values() {
                if !sub.filter.matches(&event.scope, &event.topic) {
                    continue;
                }
                {
                    let mut queue = lock(&sub.queue);
                    if queue.items.len() == self.inner.capacity {
                        queue.items.pop_front();
                        queue.dropped += 1;
                    }
                    queue.items.push_back(Arc::clone(&event));
                }
                sub.notify.notify_one();
                delivered += 1;
            }
        }
        if let Some(hub) = &self.inner.hub {
            match &event.scope {
                Scope::Project(project) => hub.publish(project, &event.topic, &event.payload)?,
                Scope::Host => hub.publish_host(&event.topic, &event.payload)?,
            };
        }
        Ok(delivered)
    }

    pub fn subscribe(&self, filter: Filter) -> Subscription {
        let shared = Arc::new(Shared {
            filter,
            queue: Mutex::default(),
            notify: Notify::new(),
        });
        let mut state = lock(&self.inner.state);
        state.next_id += 1;
        let id = state.next_id;
        state.subscribers.insert(id, Arc::clone(&shared));
        Subscription {
            id,
            shared,
            bus: Arc::downgrade(&self.inner),
        }
    }

    pub fn subscriber_count(&self) -> usize {
        lock(&self.inner.state).subscribers.len()
    }
}

impl Drop for BusInner {
    fn drop(&mut self) {
        // 버스가 사라지면 기다리던 구독을 깨워 `None` 으로 끝낸다. 안 깨우면 영원히 기다린다.
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        for sub in state.subscribers.values() {
            lock(&sub.queue).closed = true;
            sub.notify.notify_one();
        }
    }
}

/// 구독 하나. 버리면 버스에서 빠진다.
pub struct Subscription {
    id: u64,
    shared: Arc<Shared>,
    bus: Weak<BusInner>,
}

impl Subscription {
    /// 다음 이벤트를 기다린다. 버스가 사라지고 큐가 비었으면 `None`.
    pub async fn recv(&mut self) -> Option<Received> {
        loop {
            if let Some(received) = self.try_recv() {
                return Some(received);
            }
            if lock(&self.shared.queue).closed {
                return None;
            }
            // notify_one 은 기다리는 쪽이 없으면 허가 하나를 남겨 두므로, 확인과 대기 사이에 들어온 발행을 놓치지 않는다.
            self.shared.notify.notified().await;
        }
    }

    /// 기다리지 않고 꺼낸다. 큐가 비었으면 `None`.
    pub fn try_recv(&mut self) -> Option<Received> {
        let mut queue = lock(&self.shared.queue);
        if queue.dropped > 0 {
            return Some(Received::Lagged(std::mem::take(&mut queue.dropped)));
        }
        queue.items.pop_front().map(Received::Event)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(bus) = self.bus.upgrade() {
            lock(&bus.state).subscribers.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pw_rpc::{Caller, Outbound, Role, Router, Session};
    use serde_json::json;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;

    fn project(id: &str) -> Scope {
        Scope::Project(id.to_owned())
    }

    fn event(received: Option<Received>) -> Arc<Event> {
        match received {
            Some(Received::Event(e)) => e,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn delivers_only_matching_scope_and_topic() {
        let bus = EventBus::new(8);
        let mut fs = bus.subscribe(Filter::all().scope(project("p1")).topics(["fs.changed"]));
        let mut everything = bus.subscribe(Filter::all());

        assert_eq!(bus.publish(project("p1"), "fs.changed", &1).unwrap(), 2);
        assert_eq!(bus.publish(project("p2"), "fs.changed", &2).unwrap(), 1);
        assert_eq!(
            bus.publish(project("p1"), "git.statusChanged", &3).unwrap(),
            1
        );
        assert_eq!(bus.publish(Scope::Host, "fs.changed", &4).unwrap(), 1);

        assert_eq!(event(fs.try_recv()).payload, json!(1));
        assert_eq!(fs.try_recv(), None);
        let all: Vec<_> = std::iter::from_fn(|| everything.try_recv())
            .map(|r| event(Some(r)).payload.clone())
            .collect();
        assert_eq!(all, [json!(1), json!(2), json!(3), json!(4)]);
    }

    #[test]
    fn slow_subscriber_keeps_only_the_newest_events_and_learns_how_many_were_dropped() {
        let bus = EventBus::new(3);
        let mut slow = bus.subscribe(Filter::all());
        for i in 0..10 {
            // 기다리지 않는다. 구독자가 하나도 꺼내지 않아도 발행이 막히지 않는다.
            bus.publish(Scope::Host, "tick", &i).unwrap();
        }
        assert_eq!(lock(&slow.shared.queue).items.len(), 3);
        assert_eq!(slow.try_recv(), Some(Received::Lagged(7)));
        let rest: Vec<_> = std::iter::from_fn(|| slow.try_recv())
            .map(|r| event(Some(r)).payload.clone())
            .collect();
        assert_eq!(rest, [json!(7), json!(8), json!(9)]);
    }

    #[test]
    fn slow_subscriber_does_not_affect_a_fast_one() {
        let bus = EventBus::new(2);
        let _slow = bus.subscribe(Filter::all());
        let mut fast = bus.subscribe(Filter::all());
        for i in 0..5 {
            bus.publish(Scope::Host, "tick", &i).unwrap();
            assert_eq!(event(fast.try_recv()).payload, json!(i));
        }
        assert_eq!(fast.try_recv(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn recv_wakes_on_publish() {
        let bus = EventBus::new(4);
        let mut sub = bus.subscribe(Filter::all());
        let publisher = bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            publisher.publish(Scope::Host, "late", &"x").unwrap();
        });
        assert_eq!(event(sub.recv().await).topic, "late");
    }

    #[tokio::test]
    async fn recv_ends_when_the_bus_is_gone() {
        let bus = EventBus::new(4);
        let mut sub = bus.subscribe(Filter::all());
        bus.publish(Scope::Host, "last", &()).unwrap();
        drop(bus);
        assert_eq!(event(sub.recv().await).topic, "last");
        assert_eq!(sub.recv().await, None);
    }

    #[test]
    fn dropping_a_subscription_removes_it() {
        let bus = EventBus::new(4);
        let sub = bus.subscribe(Filter::all());
        assert_eq!(bus.subscriber_count(), 1);
        drop(sub);
        assert_eq!(bus.subscriber_count(), 0);
        assert_eq!(bus.publish(Scope::Host, "x", &()).unwrap(), 0);
    }

    async fn rpc_subscribe(session: &Session, rx: &mut UnboundedReceiver<Outbound>, params: Value) {
        session.handle_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": "events.subscribe", "params": params,
        }));
        assert!(
            rx.recv()
                .await
                .unwrap()
                .into_response()
                .unwrap()
                .outcome
                .is_ok()
        );
    }

    fn next_event(rx: &mut UnboundedReceiver<Outbound>) -> pw_rpc::protocol::EventMessage {
        let note = rx.try_recv().unwrap().into_notification().unwrap();
        assert_eq!(note.method, pw_rpc::protocol::EVENTS_EVENT);
        note.params_as().unwrap()
    }

    #[tokio::test]
    async fn each_scope_reaches_only_rpc_subscribers_of_that_scope() {
        let hub = EventHub::new();
        let bus = EventBus::with_rpc_hub(4, hub.clone());
        let router = Arc::new(Router::new());
        let (in_project, mut project_rx) =
            Session::new(Arc::clone(&router), hub.clone(), Caller::new(Role::Owner));
        let (host, mut host_rx) = Session::new(router, hub, Caller::new(Role::Readonly));
        rpc_subscribe(
            &in_project,
            &mut project_rx,
            json!({"projectId": "p1", "topics": ["config.changed"]}),
        )
        .await;
        rpc_subscribe(&host, &mut host_rx, json!({"topics": ["config.changed"]})).await;

        bus.publish(Scope::Host, "config.changed", &"host").unwrap();
        bus.publish(project("p1"), "config.changed", &"p1").unwrap();

        let message = next_event(&mut project_rx);
        assert_eq!(
            (message.project_id.as_deref(), message.payload),
            (Some("p1"), json!("p1"))
        );
        assert!(project_rx.try_recv().is_err());
        let message = next_event(&mut host_rx);
        assert_eq!((message.project_id, message.payload), (None, json!("host")));
        assert!(host_rx.try_recv().is_err());
    }
}
