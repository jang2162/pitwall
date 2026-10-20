//! Host 가 창에 알림을 내는 길(`docs/design/10-ui.md` §2.8 「빈 상태 · 오류 상태」, F11-5): 버스 토픽 [`TOPIC_SHOW`].
//!
//! 창(`apps/desktop/src/notifications`)이 Host 범위와 자기 프로젝트 범위의 이 토픽을 구독해 토스트 · 알림 기록으로 띄운다. Host 는
//! 문장을 만들지 않고 Fluent 메시지 id 와 인자를 보낸다 — 원격 창은 Host 와 다른 언어일 수 있다. 본문은 원문(경로 · 오류 메시지)이라
//! 번역하지 않는다.
//!
//! 알림은 창이 떠 있을 때만 보인다(버스는 지난 이벤트를 남기지 않는다). 놓치면 안 되는 상태는 상태바 등 출처를 읽는 길을 따로 둔다.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bus::{EventBus, Scope};

pub const TOPIC_SHOW: &str = "notification.show";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// [`TOPIC_SHOW`] 의 내용.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub level: Level,
    /// 제목의 Fluent 메시지 id(`extensions/i18n-default/i18n/*/project.ftl`). 창이 자기 언어로 만든다.
    pub message: String,
    /// 그 메시지의 인자.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
    /// 원문 그대로 보이는 본문. 번역하지 않는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

impl Notification {
    pub fn new(level: Level, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            args: BTreeMap::new(),
            body: None,
        }
    }

    #[must_use]
    pub fn arg(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.args.insert(name.into(), value.into());
        self
    }

    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// `scope` 의 창들에 낸다. 반환값은 버스 안에서 받은 구독 수(RPC 구독은 세지 않는다).
    pub fn show(&self, bus: &EventBus, scope: Scope) -> usize {
        // 이 모듈의 직렬화 가능한 구조체라 실패하지 않는다.
        bus.publish(scope, TOPIC_SHOW, self).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{DEFAULT_CAPACITY, Filter, Received};

    #[test]
    fn show_publishes_the_message_id_and_args_in_the_scope() {
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let mut sub = bus.subscribe(Filter::all().topics([TOPIC_SHOW]));
        let scope = Scope::Project("p".into());
        Notification::new(Level::Warning, "fs-watch-limit")
            .arg("root", "/r")
            .body("detail")
            .show(&bus, scope.clone());
        let Some(Received::Event(event)) = sub.try_recv() else {
            panic!("expected an event");
        };
        assert_eq!(event.scope, scope);
        assert_eq!(
            event.payload,
            serde_json::json!({
                "level": "warning",
                "message": "fs-watch-limit",
                "args": { "root": "/r" },
                "body": "detail",
            })
        );
    }
}
