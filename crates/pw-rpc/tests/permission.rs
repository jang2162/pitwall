//! 권한: 호출자의 role 이 메서드의 role 메타데이터에 없으면 핸들러를 부르지 않고 거부한다.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{Harness, base_router, err_code};
use pw_rpc::{Role, code, rpc_method};
use serde_json::{Value, json};

rpc_method! {
    /// 호출 횟수를 센다. owner 만.
    Touch: "test.touch" { params: (), result: (), tier: Write }
}

fn counting_router(calls: &Arc<AtomicUsize>) -> pw_rpc::Router {
    let mut router = base_router();
    let calls = Arc::clone(calls);
    router.unary(Touch, move |_ctx, ()| {
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    router
}

#[tokio::test(start_paused = true)]
async fn denied_call_is_rejected_without_running_the_handler() {
    let calls = Arc::new(AtomicUsize::new(0));
    for role in [Role::Readonly, Role::Mobile] {
        let mut h = Harness::new(counting_router(&calls), role);
        h.request(1, "test.touch", Value::Null);
        let res = h.next_response().await;
        assert_eq!(err_code(&res), code::PERMISSION_DENIED, "{role:?}");
        let data = res.outcome.unwrap_err().data.unwrap();
        assert_eq!(data["method"], "test.touch");
        assert_eq!(data["allowed"], json!(["owner"]));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let mut owner = Harness::new(counting_router(&calls), Role::Owner);
    owner.request(1, "test.touch", Value::Null);
    assert!(owner.next_response().await.outcome.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn role_matrix_follows_the_declarations() {
    // (method, owner, mobile, readonly)
    let matrix = [
        ("test.echo", true, true, true),           // read, 기본 role
        ("test.writeSetting", true, false, false), // write, 기본 role
        ("test.restart", true, true, false),       // exec, roles: [Owner, Mobile]
    ];
    for (method, owner, mobile, readonly) in matrix {
        for (role, allowed) in [
            (Role::Owner, owner),
            (Role::Mobile, mobile),
            (Role::Readonly, readonly),
        ] {
            let mut h = Harness::new(base_router(), role);
            let params = if method == "test.echo" {
                json!({"text": "x"})
            } else {
                Value::Null
            };
            h.request(1, method, params);
            let res = h.next_response().await;
            if allowed {
                assert!(res.outcome.is_ok(), "{method} {role:?}: {res:?}");
            } else {
                assert_eq!(err_code(&res), code::PERMISSION_DENIED, "{method} {role:?}");
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn denied_stream_request_opens_no_stream() {
    let mut h = Harness::new(base_router(), Role::Mobile);
    h.request(1, "test.dangerStream", Value::Null);
    assert_eq!(err_code(&h.next_response().await), code::PERMISSION_DENIED);
    // 열린 스트림이 있었다면 credit 을 받는 순간 청크가 나간다.
    h.notify("stream.credit", json!({"streamId": 1, "credit": 10}));
    h.assert_quiet().await;
}

#[tokio::test(start_paused = true)]
async fn denied_notification_is_silent() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut h = Harness::new(counting_router(&calls), Role::Readonly);
    h.notify("test.touch", Value::Null);
    h.assert_quiet().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
