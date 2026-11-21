//! 이벤트 구독 허브. 세션들의 `events.subscribe` 를 모아 두고 `publish` 를 맞는 구독에 나눠 준다.
//!
//! 구독의 범위는 둘이다: 프로젝트 하나(`projectId`)이거나 Host 전체(`projectId` 없음). 프로젝트 구독은 Host 범위
//! 이벤트를 받지 않고 Host 구독은 프로젝트 이벤트를 받지 않는다 — 둘 다 필요한 창은 구독을 둘 연다.
//!
//! **토픽마다 구독할 수 있는 role 을 선언한다**([`rpc_event!`](crate::rpc_event) → [`EventMeta`]). `events.subscribe` 는
//! 토픽 하나라도 호출자의 role 에 열려 있지 않으면 구독 전체를 `PERMISSION_DENIED` 로 거절한다. **선언이 없는 토픽은 어느
//! role 도 구독할 수 없다** — 새 토픽이 말없이 열리지 않게(메서드의 `file_scope` 와 같은 방향). 토픽을 내는 크레이트가 그
//! 토픽 상수 옆에 선언 한 줄을 둔다. owner 전용 정보(다른 프로젝트의 경로, relay 페어링 상태 등)를 싣는 토픽은
//! `roles: [Owner]` 로 선언한다(`docs/design/18-m7-plan.md` M7-02).
//!
//! 발행(`publish`)은 선언을 보지 않는다 — 선언 없는 토픽을 내도 받을 구독이 없을 뿐이다.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::message::{Outbound, notification};
use crate::method::{Role, RoleSet};
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

/// 이벤트 토픽 하나의 선언: 이름과 구독할 수 있는 role. [`rpc_event!`](crate::rpc_event) 가 만든다.
#[derive(Debug, Clone, Copy, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EventMeta {
    /// `<module>.<event>`(`fs.changed`). 구독의 `topics` 와 정확히 일치해야 한다.
    pub name: &'static str,
    #[specta(type = Vec<Role>)]
    pub roles: RoleSet,
}

impl EventMeta {
    /// 선언 규칙 검사. `rpc_event!` 가 컴파일 타임에 부른다.
    pub const fn validate(&self) -> Result<(), &'static str> {
        let name = self.name.as_bytes();
        if name.is_empty() || name[0] == b'.' || name[name.len() - 1] == b'.' {
            return Err("event topic must be `<module>.<event>`");
        }
        let mut has_dot = false;
        let mut i = 0;
        while i < name.len() {
            if name[i] == b'.' {
                has_dot = true;
            }
            i += 1;
        }
        if !has_dot {
            return Err("event topic must be `<module>.<event>`");
        }
        if self.roles.is_empty() {
            return Err("event topic must allow at least one role");
        }
        Ok(())
    }

    /// 이 role 의 호출자가 이 토픽을 구독할 수 있는가.
    pub const fn permits(&self, role: Role) -> bool {
        self.roles.contains(role)
    }
}

inventory::collect!(EventMeta);

/// 링크된 모든 크레이트에서 선언된 이벤트 토픽을 이름순으로.
///
/// **링크된 크레이트의 선언만 보인다** — [`methods`](crate::methods) 와 같다. 토픽을 선언한 크레이트가 실행 파일에 링크되지
/// 않으면 그 토픽은 구독할 수 없다(fail-closed).
///
/// # Panics
/// 두 선언이 같은 토픽을 쓰면 패닉한다(프로그래밍 오류). 토픽은 그것을 내는 크레이트 한 곳에서만 선언한다.
pub fn events() -> Vec<&'static EventMeta> {
    let mut seen = HashMap::new();
    for meta in inventory::iter::<EventMeta> {
        let previous = seen.insert(meta.name, meta);
        assert!(
            previous.is_none(),
            "event topic `{}` declared twice",
            meta.name
        );
    }
    let mut all: Vec<_> = seen.into_values().collect();
    all.sort_by_key(|m| m.name);
    all
}

/// 토픽의 선언. 선언이 없으면 `None` — 그 토픽은 구독할 수 없다.
pub fn event(topic: &str) -> Option<&'static EventMeta> {
    static TABLE: std::sync::OnceLock<HashMap<&'static str, &'static EventMeta>> =
        std::sync::OnceLock::new();
    TABLE
        .get_or_init(|| events().into_iter().map(|m| (m.name, m)).collect())
        .get(topic)
        .copied()
}

/// `topics` 중 `role` 이 구독할 수 없는 것(선언이 없거나 role 이 열려 있지 않은 토픽). 비면 모두 구독할 수 있다.
pub(crate) fn denied_topics(topics: &[String], role: Role) -> Vec<String> {
    topics
        .iter()
        .filter(|t| !event(t).is_some_and(|m| m.permits(role)))
        .cloned()
        .collect()
}

/// 이벤트 토픽을 선언한다. 토픽을 내는 크레이트가 토픽 상수 옆에 한 줄로 둔다.
///
/// ```
/// pub const TOPIC_CHANGED: &str = "fs.changed";
/// pw_rpc::rpc_event!(TOPIC_CHANGED, roles: [Owner, Mobile, Readonly]);
///
/// fn main() {
///     assert!(pw_rpc::event("fs.changed").is_some_and(|m| m.permits(pw_rpc::Role::Readonly)));
/// }
/// ```
///
/// - 토픽은 `&'static str` 상수식(상수 경로 · 리터럴)이다. `roles` 는 [`Role`] 의 변형 이름이고 생략할 수 없다 —
///   payload 에 무엇이 실리는지는 토픽마다 고르는 것이다. Read 등급 정보만 싣는 토픽은 셋 모두로, owner 전용 정보를 싣는
///   토픽은 `[Owner]` 로 선언한다.
/// - 선언은 [`events`](crate::events) 레지스트리에 등록되고, 생성기가 TS 의 `TOPICS` 로 내보낸다.
/// - 이름 규칙 · role 규칙은 [`EventMeta::validate`] 가 컴파일 타임에 검사한다.
///
/// ```compile_fail
/// pw_rpc::rpc_event!("x.nobody", roles: []);
/// ```
///
/// ```compile_fail
/// pw_rpc::rpc_event!("nodot", roles: [Owner]);
/// ```
#[macro_export]
macro_rules! rpc_event {
    ($topic:expr, roles: [$($role:ident),* $(,)?] $(,)?) => {
        const _: () = {
            const META: $crate::EventMeta = $crate::EventMeta {
                name: $topic,
                roles: $crate::RoleSet::EMPTY $(.with($crate::Role::$role))*,
            };
            if let Err(reason) = META.validate() {
                panic!("{}", reason)
            }
        };
        $crate::__private::inventory::submit! {
            $crate::EventMeta {
                name: $topic,
                roles: $crate::RoleSet::EMPTY $(.with($crate::Role::$role))*,
            }
        }
    };
}
