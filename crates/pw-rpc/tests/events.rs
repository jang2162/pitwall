//! 이벤트 구독/해제: 프로젝트와 토픽이 맞는 구독에만 전달하고, 해제 뒤에는 보내지 않는다.

mod common;

use std::sync::Arc;

use common::{Harness, base_router, err_code};
use pw_rpc::protocol::{EVENTS_EVENT, EventMessage, Subscribed};
use pw_rpc::{EventHub, Role, code};
use serde_json::json;

async fn subscribe(h: &mut Harness, id: i64, project: &str, topics: &[&str]) -> u64 {
    h.request(
        id,
        "events.subscribe",
        json!({"projectId": project, "topics": topics}),
    );
    h.next_response()
        .await
        .result_as::<Subscribed>()
        .unwrap()
        .subscription_id
}

#[tokio::test(start_paused = true)]
async fn subscriber_receives_matching_events_only() {
    let mut h = Harness::new(base_router(), Role::Readonly);
    let sub = subscribe(&mut h, 1, "p1", &["fs.changed", "git.statusChanged"]).await;

    assert_eq!(
        h.events
            .publish("p1", "fs.changed", &json!({"path": "a.rs"}))
            .unwrap(),
        1
    );
    let event: EventMessage = h.next_notification(EVENTS_EVENT).await;
    assert_eq!(
        event,
        EventMessage {
            subscription_id: sub,
            project_id: "p1".into(),
            topic: "fs.changed".into(),
            payload: json!({"path": "a.rs"}),
        }
    );

    assert_eq!(h.events.publish("p1", "lsp.diagnostics", &()).unwrap(), 0);
    assert_eq!(h.events.publish("p2", "fs.changed", &()).unwrap(), 0);
    h.assert_quiet().await;
}

#[tokio::test(start_paused = true)]
async fn unsubscribed_topic_is_no_longer_delivered() {
    let mut h = Harness::new(base_router(), Role::Owner);
    let sub = subscribe(&mut h, 1, "p1", &["fs.changed"]).await;
    h.request(2, "events.unsubscribe", json!({"subscriptionId": sub}));
    assert!(h.next_response().await.outcome.is_ok());

    assert_eq!(h.events.publish("p1", "fs.changed", &()).unwrap(), 0);
    h.assert_quiet().await;
    assert_eq!(h.events.subscription_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_session_cannot_remove_another_sessions_subscription() {
    let events = EventHub::new();
    let router = Arc::new(base_router());
    let mut a = Harness::with_events(Arc::clone(&router), events.clone(), Role::Owner);
    let mut b = Harness::with_events(router, events.clone(), Role::Owner);
    let sub = subscribe(&mut a, 1, "p1", &["fs.changed"]).await;

    b.request(1, "events.unsubscribe", json!({"subscriptionId": sub}));
    assert!(b.next_response().await.outcome.is_ok());

    assert_eq!(events.publish("p1", "fs.changed", &()).unwrap(), 1);
    let _: EventMessage = a.next_notification(EVENTS_EVENT).await;
    b.assert_quiet().await;
}

#[tokio::test]
async fn dropping_the_session_removes_its_subscriptions() {
    let events = EventHub::new();
    let mut h = Harness::with_events(Arc::new(base_router()), events.clone(), Role::Owner);
    subscribe(&mut h, 1, "p1", &["fs.changed"]).await;
    assert_eq!(events.subscription_count(), 1);
    drop(h);
    assert_eq!(events.subscription_count(), 0);
    assert_eq!(events.publish("p1", "fs.changed", &()).unwrap(), 0);
}

#[tokio::test]
async fn malformed_subscribe_is_invalid_params() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "events.subscribe", json!({"topics": ["fs.changed"]}));
    assert_eq!(err_code(&h.next_response().await), code::INVALID_PARAMS);
}

#[tokio::test]
async fn subscribe_after_close_is_rejected() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.session.close();
    h.request(
        1,
        "events.subscribe",
        json!({"projectId": "p1", "topics": ["fs.changed"]}),
    );
    assert_eq!(err_code(&h.next_response().await), code::CANCELLED);
    assert_eq!(h.events.subscription_count(), 0);
    assert_eq!(h.events.publish("p1", "fs.changed", &()).unwrap(), 0);
}
