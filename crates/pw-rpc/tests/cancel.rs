//! 취소: 요청과 스트림을 id 로 취소하면 핸들러가 실제로 중단된다.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{Harness, base_router, err_code};
use pw_rpc::protocol::{
    STREAM_CHUNK, STREAM_END, StreamChunk, StreamEnd, StreamOpened, StreamOutcome,
};
use pw_rpc::{Id, Role, Router, RpcError, code, rpc_method};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};

rpc_method! {
    /// 시작을 알리고, 문이 열리면 플래그를 세운다.
    Gated: "test.gated" { params: (), result: (), tier: Read }

    /// 청크 하나를 보낸 뒤 문이 열리면 플래그를 세우고 하나 더 보낸다.
    GatedStream: "test.gatedStream" { params: (), stream: u32, tier: Read }
}

/// 핸들러가 문 앞에 도착할 때까지 기다린다. 요청이 거부돼 핸들러가 돌지 않으면 영원히 기다리지 않고 실패한다.
async fn wait_started(probe: &Probe) {
    timeout(Duration::from_secs(1), probe.started.notified())
        .await
        .expect("handler never started");
}

struct Probe {
    started: Arc<Notify>,
    gate: Arc<Notify>,
    reached: Arc<AtomicBool>,
}

fn gated_router() -> (Router, Probe) {
    let probe = Probe {
        started: Arc::new(Notify::new()),
        gate: Arc::new(Notify::new()),
        reached: Arc::new(AtomicBool::new(false)),
    };
    let mut router = base_router();
    let (started, gate, reached) = (
        probe.started.clone(),
        probe.gate.clone(),
        probe.reached.clone(),
    );
    router.unary(Gated, move |_ctx, ()| {
        let (started, gate, reached) = (started.clone(), gate.clone(), reached.clone());
        async move {
            started.notify_one();
            gate.notified().await;
            reached.store(true, Ordering::SeqCst);
            Ok::<_, RpcError>(())
        }
    });
    let (started, gate, reached) = (
        probe.started.clone(),
        probe.gate.clone(),
        probe.reached.clone(),
    );
    router.stream(GatedStream, move |_ctx, (), sink| {
        let (started, gate, reached) = (started.clone(), gate.clone(), reached.clone());
        async move {
            sink.send(0).await?;
            started.notify_one();
            gate.notified().await;
            reached.store(true, Ordering::SeqCst);
            sink.send(1).await
        }
    });
    (router, probe)
}

#[tokio::test]
async fn cancelled_request_stops_its_handler_and_answers_cancelled() {
    let (router, probe) = gated_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.gated", Value::Null);
    wait_started(&probe).await;

    h.notify("request.cancel", json!({"id": 1}));
    // 문을 연다. 핸들러가 버려지지 않았다면 깨어나 플래그를 세우고 성공으로 답한다.
    probe.gate.notify_one();

    let res = h.next_response().await;
    assert_eq!(
        (res.id.clone(), err_code(&res)),
        (Id::Number(1), code::CANCELLED)
    );
    tokio::task::yield_now().await;
    assert!(
        !probe.reached.load(Ordering::SeqCst),
        "handler ran past the cancel point"
    );
}

#[tokio::test(start_paused = true)]
async fn cancel_for_a_finished_or_unknown_request_is_ignored() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "test.echo", json!({"text": "x"}));
    assert!(h.next_response().await.outcome.is_ok());
    h.notify("request.cancel", json!({"id": 1}));
    h.notify("request.cancel", json!({"id": "never-sent"}));
    h.assert_quiet().await;

    // id 를 붙여 보낸 제어 메시지는 null 로 답한다.
    h.request(2, "request.cancel", json!({"id": 1}));
    assert_eq!(h.next_response().await.outcome, Ok(Value::Null));
}

#[tokio::test]
async fn cancelled_stream_stops_its_handler_and_ends_cancelled() {
    let (router, probe) = gated_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.gatedStream", Value::Null);
    let id = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    h.notify("stream.credit", json!({"streamId": id, "credit": 10}));
    let first: StreamChunk = h.next_notification(STREAM_CHUNK).await;
    assert_eq!(first.data, json!(0));
    wait_started(&probe).await;

    h.notify("stream.cancel", json!({"streamId": id}));
    probe.gate.notify_one();

    let end: StreamEnd = h.next_notification(STREAM_END).await;
    assert_eq!(
        end,
        StreamEnd {
            stream_id: id,
            outcome: StreamOutcome::Cancelled
        }
    );
    tokio::task::yield_now().await;
    assert!(
        !probe.reached.load(Ordering::SeqCst),
        "stream handler ran past the cancel point"
    );
}

#[tokio::test(start_paused = true)]
async fn stream_waiting_for_credit_can_be_cancelled() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "test.count", json!({"n": 3}));
    let id = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    h.notify("stream.cancel", json!({"streamId": id}));
    let end: StreamEnd = h.next_notification(STREAM_END).await;
    assert_eq!(end.outcome, StreamOutcome::Cancelled);
    // 취소 뒤 credit 을 줘도 아무것도 나가지 않는다.
    h.notify("stream.credit", json!({"streamId": id, "credit": 10}));
    h.assert_quiet().await;
}

#[tokio::test]
async fn closing_the_session_cancels_everything_in_flight() {
    let (router, probe) = gated_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.gated", Value::Null);
    wait_started(&probe).await;
    h.session.close();
    probe.gate.notify_one();
    let res = h.next_response().await;
    assert_eq!(err_code(&res), code::CANCELLED);
    tokio::task::yield_now().await;
    assert!(!probe.reached.load(Ordering::SeqCst));
}

/// 취소와 핸들러 완료가 같은 poll 에서 둘 다 준비된 경우에도 취소가 이긴다.
///
/// select 가 `biased` 가 아니면 준비된 갈래를 무작위로 고르므로 한 번에 1/2 확률로 핸들러가 이긴다.
/// 64번 되풀이해 그 경우를 놓칠 확률을 2^-64 로 만든다 — 결정적이지는 않지만 사실상 빠짐없이 잡는다.
#[tokio::test]
async fn cancel_wins_even_when_the_handler_became_ready_in_the_same_poll() {
    for round in 0..64 {
        let (router, probe) = gated_router();
        let mut h = Harness::new(router, Role::Owner);
        h.request(1, "test.gated", Value::Null);
        wait_started(&probe).await;
        h.notify("request.cancel", json!({"id": 1}));
        probe.gate.notify_one();
        let res = h.next_response().await;
        assert_eq!(err_code(&res), code::CANCELLED, "unary, round {round}");

        let (router, probe) = gated_router();
        let mut h = Harness::new(router, Role::Owner);
        h.request(1, "test.gatedStream", Value::Null);
        let id = h
            .next_response()
            .await
            .result_as::<StreamOpened>()
            .unwrap()
            .stream_id;
        h.notify("stream.credit", json!({"streamId": id, "credit": 10}));
        let _: StreamChunk = h.next_notification(STREAM_CHUNK).await;
        wait_started(&probe).await;
        h.notify("stream.cancel", json!({"streamId": id}));
        probe.gate.notify_one();
        let end: StreamEnd = h.next_notification(STREAM_END).await;
        assert_eq!(
            end.outcome,
            StreamOutcome::Cancelled,
            "stream, round {round}"
        );
    }
}

rpc_method! {
    /// 연결이 닫힐 때 취소되는 토큰을 남기고 곧바로 끝난다.
    Keep: "test.keep" { params: (), result: (), tier: Read }
}

/// `Ctx::connection_closed` 는 호출이 끝나도 살아 있다가 세션이 닫힐 때 취소된다 — 호출 하나의 취소나 다른 호출의 끝에는 반응하지 않는다.
#[tokio::test]
async fn the_connection_token_outlives_the_call_and_fires_when_the_session_closes() {
    let kept = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut router = base_router();
    let store = Arc::clone(&kept);
    router.unary(Keep, move |ctx, ()| {
        store.lock().unwrap().push(ctx.connection_closed());
        async move { Ok::<_, RpcError>(()) }
    });
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.keep", Value::Null);
    h.next_response().await.outcome.unwrap();
    h.request(2, "test.keep", Value::Null);
    h.notify("request.cancel", json!({"id": 2}));
    let _ = h.next_response().await;
    let tokens = kept.lock().unwrap().clone();
    assert_eq!(tokens.len(), 2);
    assert!(
        tokens.iter().all(|t| !t.is_cancelled()),
        "a finished or cancelled call leaves the connection token alone"
    );
    h.session.close();
    for token in &tokens {
        timeout(Duration::from_secs(1), token.cancelled())
            .await
            .expect("closing the session cancels the connection token");
    }
}
