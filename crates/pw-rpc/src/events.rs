//! 이벤트 구독 허브. 세션들의 `events.subscribe` 를 모아 두고 `publish` 를 맞는 구독에 나눠 준다.
//!
//! 구독의 범위는 둘이다: 프로젝트 하나(`projectId`)이거나 Host 전체(`projectId` 없음). 프로젝트 구독은 Host 범위
//! 이벤트를 받지 않고 Host 구독은 프로젝트 이벤트를 받지 않는다 — 둘 다 필요한 창은 구독을 둘 연다.
//! 구독할 수 있는 role 은 범위와 무관하게 같다(내장 프로토콜은 모든 role 이 쓴다, `crate::protocol` 머리 주석).
//!
//! ⚠ 토픽별 role 검사가 없으므로 **이벤트 payload 는 Read 등급 정보만 싣는다** — readonly · mobile 기기도 구독하면 받는다.
//! Owner 전용 정보(다른 프로젝트의 경로, relay 페어링 상태 등)를 실을 토픽이 생기면 그 전에 토픽별 role 메타(메서드의
//! `roles` 와 같은 모양)를 더한다. relay 연결 전에 구현한다(`docs/design/08-remote-mobile.md` 「RPC」 행).

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
    /// `None` 이면 Host 범위.
    project_id: Option<String>,
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
        self.publish_scoped(Some(project_id), topic, payload)
    }

    /// Host 범위(프로젝트 없음)의 `topic` 이벤트를 `projectId` 없이 구독한 쪽에 보낸다. 반환값은 전달한 구독 수.
    pub fn publish_host<T: Serialize>(
        &self,
        topic: &str,
        payload: &T,
    ) -> serde_json::Result<usize> {
        self.publish_scoped(None, topic, payload)
    }

    fn publish_scoped<T: Serialize>(
        &self,
        project_id: Option<&str>,
        topic: &str,
        payload: &T,
    ) -> serde_json::Result<usize> {
        let payload = serde_json::to_value(payload)?;
        let mut state = lock(&self.state);
        let mut delivered = 0;
        let mut dead = Vec::new();
        for (id, sub) in &state.subscribers {
            if sub.project_id.as_deref() != project_id || !sub.topics.contains(topic) {
                continue;
            }
            let message = EventMessage {
                subscription_id: *id,
                project_id: project_id.map(str::to_owned),
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
        project_id: Option<String>,
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
