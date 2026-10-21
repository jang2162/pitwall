//! 요청/응답: 성공 · 알 수 없는 메서드 · 잘못된 params · 형식 오류 · 열거.

mod common;

use common::{APP_ERROR, EchoParams, Harness, base_router, err_code};
use pw_rpc::{Id, MethodKind, Role, Tier, code};
use serde_json::{Value, json};

#[tokio::test]
async fn unary_request_gets_its_result() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "test.echo", json!({"text": "hi"}));
    let res = h.next_response().await;
    assert_eq!(res.id, Id::Number(1));
    assert_eq!(
        res.result_as::<EchoParams>().unwrap(),
        EchoParams { text: "hi".into() }
    );
}

#[tokio::test]
async fn wire_format_round_trip() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.session
        .handle_text(r#"{"jsonrpc":"2.0","id":"a-1","method":"test.echo","params":{"text":"x"}}"#);
    let text = h.next().await.to_text();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({"jsonrpc": "2.0", "id": "a-1", "result": {"text": "x"}})
    );
}

#[tokio::test]
async fn unknown_method_is_method_not_found() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(2, "test.nope", json!({}));
    let res = h.next_response().await;
    assert_eq!(res.id, Id::Number(2));
    assert_eq!(err_code(&res), code::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn params_that_do_not_fit_the_input_type_are_invalid_params() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(3, "test.echo", json!({"txt": "typo"}));
    assert_eq!(err_code(&h.next_response().await), code::INVALID_PARAMS);
    h.request(4, "test.echo", json!({"text": 5}));
    assert_eq!(err_code(&h.next_response().await), code::INVALID_PARAMS);
    // 스트림 메서드도 params 가 틀리면 스트림을 열지 않고 오류로 답한다.
    h.request(5, "test.count", json!({"n": "five"}));
    assert_eq!(err_code(&h.next_response().await), code::INVALID_PARAMS);
}

#[tokio::test]
async fn malformed_frames_get_standard_errors() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.session.handle_text("{not json");
    let res = h.next_response().await;
    assert_eq!(
        (res.id.clone(), err_code(&res)),
        (Id::Null, code::PARSE_ERROR)
    );

    h.session.handle_text(r#"{"jsonrpc":"2.0","id":9}"#);
    let res = h.next_response().await;
    assert_eq!(
        (res.id.clone(), err_code(&res)),
        (Id::Number(9), code::INVALID_REQUEST)
    );

    h.session
        .handle_text(r#"[{"jsonrpc":"2.0","id":1,"method":"test.echo"}]"#);
    assert_eq!(err_code(&h.next_response().await), code::INVALID_REQUEST);
}

#[tokio::test(start_paused = true)]
async fn notifications_never_get_a_reply() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.notify("test.nope", json!({}));
    h.notify("test.echo", json!({"text": "x"}));
    h.notify("test.writeSetting", Value::Null);
    h.assert_quiet().await;
}

#[tokio::test]
async fn handler_errors_and_panics_become_error_responses() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "test.fail", Value::Null);
    let res = h.next_response().await;
    assert_eq!(err_code(&res), APP_ERROR);
    assert_eq!(res.outcome.unwrap_err().message, "boom");

    h.request(2, "test.panic", Value::Null);
    let res = h.next_response().await;
    assert_eq!(
        (res.id.clone(), err_code(&res)),
        (Id::Number(2), code::INTERNAL_ERROR)
    );
}

#[tokio::test(start_paused = true)]
async fn duplicate_in_flight_id_is_rejected() {
    let mut router = base_router();
    rpc_hang(&mut router);
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.hang", Value::Null);
    h.request(1, "test.hang", Value::Null);
    let res = h.next_response().await;
    assert_eq!(
        (res.id.clone(), err_code(&res)),
        (Id::Number(1), code::INVALID_REQUEST)
    );
    h.assert_quiet().await;
}

pw_rpc::rpc_method! {
    /// 끝나지 않는다.
    Hang: "test.hang" { params: (), result: (), tier: Read }
}

fn rpc_hang(router: &mut pw_rpc::Router) {
    router.unary(Hang, |_ctx, ()| {
        std::future::pending::<Result<(), pw_rpc::RpcError>>()
    });
}

#[test]
fn registered_methods_are_enumerable_with_their_metadata() {
    let router = base_router();
    let names: Vec<_> = router.methods().iter().map(|m| m.name).collect();
    assert_eq!(
        names,
        [
            "test.count",
            "test.dangerStream",
            "test.echo",
            "test.fail",
            "test.panic",
            "test.restart",
            "test.writeSetting"
        ]
    );
    let count = router
        .methods()
        .into_iter()
        .find(|m| m.name == "test.count")
        .unwrap();
    assert_eq!(count.kind, MethodKind::Stream);
    assert_eq!(count.tier, Tier::Read);
    assert_eq!(count.params, "CountParams");
    assert_eq!(count.output, "u32");
    assert_eq!(count.doc.trim(), "0 부터 n-1 까지 청크로 보낸다.");
    assert_eq!(count.module(), "test");

    let restart = router
        .methods()
        .into_iter()
        .find(|m| m.name == "test.restart")
        .unwrap();
    assert_eq!(
        serde_json::to_value(restart).unwrap(),
        json!({
            "name": "test.restart",
            "kind": "unary",
            "tier": "exec",
            "roles": ["owner", "mobile"],
            "params": "()",
            "output": "()",
            "doc": " mobile 에게도 허용한 조작.\n",
            "mcp": true,
            "fileScope": false,
        })
    );
}

#[test]
#[should_panic(expected = "registered twice")]
fn registering_a_method_twice_panics() {
    let mut router = base_router();
    router.unary(common::Echo, |_ctx, p: EchoParams| async move { Ok(p) });
}

pw_rpc::rpc_method! {
    /// future 를 만들기 전에 패닉한다.
    SyncPanic: "test.syncPanic" { params: (), result: (), tier: Read }

    /// 스트림판.
    SyncPanicStream: "test.syncPanicStream" { params: (), stream: u32, tier: Read }
}

type HandlerFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), pw_rpc::RpcError>> + Send>>;

fn panics_before_the_future(_ctx: pw_rpc::Ctx, _params: ()) -> HandlerFuture {
    panic!("sync-part panic for test")
}

fn stream_panics_before_the_future(
    _ctx: pw_rpc::Ctx,
    _params: (),
    _sink: pw_rpc::StreamSink<u32>,
) -> HandlerFuture {
    panic!("sync-part panic for test")
}

#[tokio::test(start_paused = true)]
async fn panic_in_the_synchronous_part_of_a_handler_is_an_internal_error() {
    let mut router = base_router();
    router
        .unary(SyncPanic, panics_before_the_future)
        .stream(SyncPanicStream, stream_panics_before_the_future);
    let mut h = Harness::new(router, Role::Owner);

    // 패닉이 transport 의 호출자로 새지 않는다.
    let escaped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.request(1, "test.syncPanic", Value::Null);
    }));
    assert!(escaped.is_ok(), "panic escaped handle_value");
    let res = h.next_response().await;
    assert_eq!(
        (res.id.clone(), err_code(&res)),
        (Id::Number(1), code::INTERNAL_ERROR)
    );

    // id 1 이 진행 중 맵에 남지 않았다.
    h.request(1, "test.echo", json!({"text": "again"}));
    assert_eq!(
        h.next_response().await.result_as::<EchoParams>().unwrap(),
        EchoParams {
            text: "again".into()
        }
    );

    h.request(2, "test.syncPanicStream", Value::Null);
    assert_eq!(err_code(&h.next_response().await), code::INTERNAL_ERROR);
    h.notify("stream.credit", json!({"streamId": 1, "credit": 10}));
    h.assert_quiet().await;
}

#[derive(Debug, Clone, Copy)]
struct HandMade;

impl pw_rpc::Method for HandMade {
    type Params = ();
    // rpc_method! 라면 컴파일되지 않을 선언: readonly 에게 write 를 연다.
    const META: pw_rpc::MethodMeta = pw_rpc::MethodMeta {
        name: "test.handMade",
        kind: MethodKind::Unary,
        tier: Tier::Write,
        roles: pw_rpc::RoleSet::EMPTY.with(Role::Readonly),
        params: "()",
        output: "()",
        doc: "",
        mcp: false,
        file_scope: false,
    };
}

impl pw_rpc::UnaryMethod for HandMade {
    type Result = ();
}

#[test]
#[should_panic(expected = "is invalid")]
fn registering_a_hand_made_invalid_method_panics() {
    let mut router = pw_rpc::Router::new();
    router.unary(HandMade, |_ctx, ()| async { Ok(()) });
}
