//! 이벤트 구독/해제: 프로젝트와 토픽이 맞는 구독에만 전달하고, 해제 뒤에는 보내지 않는다.
//! 토픽 role: 선언이 없거나 호출자의 role 에 열려 있지 않은 토픽은 구독할 수 없다.

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
            project_id: Some("p1".into()),
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

#[tokio::test(start_paused = true)]
async fn subscribing_without_a_project_receives_host_events_only() {
    let mut h = Harness::new(base_router(), Role::Readonly);
    h.request(1, "events.subscribe", json!({"topics": ["config.changed"]}));
    let sub = h
        .next_response()
        .await
        .result_as::<Subscribed>()
        .unwrap()
        .subscription_id;

    assert_eq!(h.events.publish("p1", "config.changed", &1).unwrap(), 0);
    assert_eq!(h.events.publish_host("config.changed", &2).unwrap(), 1);
    let event: EventMessage = h.next_notification(EVENTS_EVENT).await;
    assert_eq!(
        event,
        EventMessage {
            subscription_id: sub,
            project_id: None,
            topic: "config.changed".into(),
            payload: json!(2),
        }
    );
    assert_eq!(h.events.publish_host("fs.changed", &()).unwrap(), 0);
    h.assert_quiet().await;
}

#[tokio::test(start_paused = true)]
async fn a_project_subscription_does_not_receive_host_events() {
    let mut h = Harness::new(base_router(), Role::Owner);
    subscribe(&mut h, 1, "p1", &["config.changed"]).await;
    assert_eq!(h.events.publish_host("config.changed", &()).unwrap(), 0);
    h.assert_quiet().await;
}

#[tokio::test]
async fn malformed_subscribe_is_invalid_params() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "events.subscribe", json!({"projectId": "p1"}));
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

fn denied_topics(response: &pw_rpc::Response) -> Vec<String> {
    let Err(error) = &response.outcome else {
        panic!("expected an error, got {:?}", response.outcome);
    };
    assert_eq!(error.code, code::PERMISSION_DENIED);
    serde_json::from_value(error.data.as_ref().expect("data")["topics"].clone()).unwrap()
}

/// 선언이 없는 토픽은 owner 도 구독할 수 없다 — 새 토픽이 말없이 열리지 않는다. 등록도 남지 않는다.
#[tokio::test(start_paused = true)]
async fn undeclared_topic_cannot_be_subscribed() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(
        1,
        "events.subscribe",
        json!({"projectId": "p1", "topics": ["test.undeclared"]}),
    );
    assert_eq!(denied_topics(&h.next_response().await), ["test.undeclared"]);
    assert_eq!(h.events.subscription_count(), 0);
    assert_eq!(h.events.publish("p1", "test.undeclared", &()).unwrap(), 0);
    h.assert_quiet().await;
}

/// 토픽 하나라도 막히면 구독 전체를 거절한다 — 열린 토픽만 골라 받는 구독이 생기지 않는다.
#[tokio::test(start_paused = true)]
async fn one_denied_topic_rejects_the_whole_subscription() {
    let mut h = Harness::new(base_router(), Role::Readonly);
    h.request(
        1,
        "events.subscribe",
        json!({"projectId": "p1", "topics": ["fs.changed", "test.ownerOnly", "test.undeclared"]}),
    );
    assert_eq!(
        denied_topics(&h.next_response().await),
        ["test.ownerOnly", "test.undeclared"]
    );
    assert_eq!(h.events.subscription_count(), 0);
    assert_eq!(h.events.publish("p1", "fs.changed", &()).unwrap(), 0);
}

/// owner 전용 토픽은 owner 만 — mobile · readonly 는 거절된다.
#[tokio::test(start_paused = true)]
async fn owner_only_topic_follows_the_callers_role() {
    for role in [Role::Mobile, Role::Readonly] {
        let mut h = Harness::new(base_router(), role);
        h.request(1, "events.subscribe", json!({"topics": ["test.ownerOnly"]}));
        assert_eq!(
            denied_topics(&h.next_response().await),
            ["test.ownerOnly"],
            "{role:?}"
        );
        assert_eq!(h.events.subscription_count(), 0);
    }
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "events.subscribe", json!({"topics": ["test.ownerOnly"]}));
    assert!(h.next_response().await.outcome.is_ok());
    assert_eq!(h.events.publish_host("test.ownerOnly", &()).unwrap(), 1);
}

#[test]
fn declared_topics_are_listed_by_name() {
    let listed: Vec<_> = pw_rpc::events().iter().map(|m| (m.name, m.roles)).collect();
    let all = pw_rpc::RoleSet::EMPTY
        .with(Role::Owner)
        .with(Role::Mobile)
        .with(Role::Readonly);
    assert_eq!(
        listed,
        [
            ("config.changed", all),
            ("fs.changed", all),
            ("git.statusChanged", all),
            ("test.ownerOnly", pw_rpc::RoleSet::EMPTY.with(Role::Owner)),
        ]
    );
}
