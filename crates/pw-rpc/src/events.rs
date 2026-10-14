//! 이벤트 구독 허브. 세션들의 `events.subscribe` 를 모아 두고 `publish` 를 맞는 구독에 나눠 준다.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::message::{Outbound, notification};
use crate::protocol::{EVENTS_EVENT, EventMessage, SubscriptionId};
use crate::session::lock;

/// 여러 세션이 공유하는 구독 표. 복제해도 같은 표를 가리킨다.
///
/// 전달은 `publish` 안에서 동기적으로 끝난다. 그래서 `events.unsubscribe` 의 응답이 나간 뒤에
/// 발행된 이벤트는 그 구독으로 절대 가지 않는다 — 구독 해제가 비동기 태스크 정리를 기다리지 않는다.
#[derive(Clone, Default)]
pub struct EventHub {
    state: Arc<Mutex<HubState>>,
}

#[derive(Default)]
struct HubState {
    next_id: SubscriptionId,
    subscribers: HashMap<SubscriptionId, Subscriber>,
}

struct Subscriber {
    project_id: String,
    topics: HashSet<String>,
    out: UnboundedSender<Outbound>,
}

impl EventHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// `project_id` 의 `topic` 이벤트를 맞는 구독 전부에 보낸다. 반환값은 전달한 구독 수.
    ///
    /// 연결이 끊긴 세션의 구독은 여기서 걷는다.
    pub fn publish<T: Serialize>(
        &self,
        project_id: &str,
        topic: &str,
        payload: &T,
    ) -> serde_json::Result<usize> {
        let payload = serde_json::to_value(payload)?;
        let mut state = lock(&self.state);
        let mut delivered = 0;
        let mut dead = Vec::new();
        for (id, sub) in &state.subscribers {
            if sub.project_id != project_id || !sub.topics.contains(topic) {
                continue;
            }
            let message = EventMessage {
                subscription_id: *id,
                project_id: project_id.to_owned(),
                topic: topic.to_owned(),
                payload: payload.clone(),
            };
            if sub.out.send(notification(EVENTS_EVENT, &message)).is_ok() {
                delivered += 1;
            } else {
                dead.push(*id);
            }
        }
        for id in dead {
            state.subscribers.remove(&id);
        }
        Ok(delivered)
    }

    /// 현재 구독 수. 세션 정리가 구독을 걷었는지 확인하는 용도.
    pub fn subscription_count(&self) -> usize {
        lock(&self.state).subscribers.len()
    }

    pub(crate) fn subscribe(
        &self,
        project_id: String,
        topics: Vec<String>,
        out: UnboundedSender<Outbound>,
    ) -> SubscriptionId {
        let mut state = lock(&self.state);
        state.next_id += 1;
        let id = state.next_id;
        state.subscribers.insert(
            id,
            Subscriber {
                project_id,
                topics: topics.into_iter().collect(),
                out,
            },
        );
        id
    }

    pub(crate) fn unsubscribe(&self, id: SubscriptionId) {
        lock(&self.state).subscribers.remove(&id);
    }
}
