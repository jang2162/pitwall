//! 파일 범위 호출자(`Caller::file`, 단일 파일 창): `file_scope` 로 선언한 메서드만 부르고, 이벤트는 Host 범위와 자기 파일의
//! 범위만 듣는다.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::{Harness, base_router, err_code};
use pw_rpc::{Caller, EventHub, Role, Session, code, rpc_method};
use serde_json::json;

rpc_method! {
    /// 이 연결이 닿는 파일(파일 범위 호출자가 아니면 `null`).
    pub ScopeOf: "test.scopeOf" {
        params: (),
        result: Option<String>,
        tier: Read,
        file_scope: true,
    }
}

fn file_harness(path: &str) -> Harness {
    let mut router = base_router();
    router.unary(ScopeOf, |ctx, ()| {
        let path = ctx.file_scope().map(|f| f.path().display().to_string());
        async move { Ok(path) }
    });
    let events = EventHub::new();
    let (session, rx) = Session::new(
        Arc::new(router),
        events.clone(),
        Caller::file(Role::Owner, PathBuf::from(path)),
    );
    Harness {
        session,
        rx,
        events,
    }
}

#[tokio::test]
async fn a_file_caller_reaches_only_methods_declared_for_the_file_scope() {
    let mut h = file_harness("/x/a.txt");
    h.request(1, "test.scopeOf", json!(null));
    assert_eq!(
        h.next_response().await.outcome.unwrap(),
        json!("/x/a.txt"),
        "the handler sees the transport's file"
    );
    // owner 가 부를 수 있는 메서드여도 파일 범위로 선언하지 않았으면 거절한다.
    for (id, method, params) in [
        (2, "test.echo", json!({"text": "hi"})),
        (3, "test.writeSetting", json!(null)),
        (4, "test.count", json!({"n": 1})),
    ] {
        h.request(id, method, params);
        let response = h.next_response().await;
        assert_eq!(err_code(&response), code::PERMISSION_DENIED, "{method}");
        assert_eq!(
            response.outcome.unwrap_err().data,
            Some(json!({"method": method, "scope": "file"}))
        );
    }
}

#[tokio::test]
async fn a_file_caller_subscribes_to_the_host_and_its_own_file_only() {
    let mut h = file_harness("/x/a.txt");
    h.request(
        1,
        "events.subscribe",
        json!({"projectId": "p1", "topics": ["fs.changed"]}),
    );
    assert_eq!(err_code(&h.next_response().await), code::PERMISSION_DENIED);
    // 다른 파일의 범위도 남의 것이다.
    h.request(
        2,
        "events.subscribe",
        json!({"projectId": "file:/x/b.txt", "topics": ["fs.changed"]}),
    );
    assert_eq!(err_code(&h.next_response().await), code::PERMISSION_DENIED);
    h.events
        .publish("p1", "fs.changed", &json!({"paths": ["/p1/secret"]}))
        .unwrap();
    h.assert_quiet().await;

    h.request(
        3,
        "events.subscribe",
        json!({"projectId": "file:/x/a.txt", "topics": ["fs.changed"]}),
    );
    assert!(h.next_response().await.outcome.is_ok());
    h.request(4, "events.subscribe", json!({"topics": ["config.changed"]}));
    assert!(h.next_response().await.outcome.is_ok());
    h.events
        .publish(
            "file:/x/a.txt",
            "fs.changed",
            &json!({"paths": ["/x/a.txt"]}),
        )
        .unwrap();
    let note = h.next().await.into_notification().expect("an event");
    assert_eq!(note.params["projectId"], "file:/x/a.txt");
}

/// 파일 범위가 없는 호출자는 그대로다 — `file_scope` 로 선언한 메서드도 부르고 핸들러는 범위를 보지 못한다.
#[tokio::test]
async fn an_ordinary_caller_is_unaffected() {
    let mut router = base_router();
    router.unary(ScopeOf, |ctx, ()| {
        let path = ctx.file_scope().map(|f| f.path().display().to_string());
        async move { Ok(path) }
    });
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.scopeOf", json!(null));
    assert_eq!(h.next_response().await.outcome.unwrap(), json!(null));
    h.request(2, "test.echo", json!({"text": "hi"}));
    assert!(h.next_response().await.outcome.is_ok());
    h.request(
        3,
        "events.subscribe",
        json!({"projectId": "p1", "topics": ["fs.changed"]}),
    );
    assert!(h.next_response().await.outcome.is_ok());
}
