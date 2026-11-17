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

rpc_method! {
    /// 띄운 태스크가 문 1 뒤에 「되돌릴 수 없는 지점」 을 세우고(세웠는가가 결과), 문 2 뒤에 끝난다.
    Irreversible: "test.irreversible" { params: (), result: bool, tier: Read }

    /// 스트림 호출에서 표지를 세워 본다(청크 하나 — 섰는가).
    PointInStream: "test.pointInStream" { params: (), stream: bool, tier: Read }
}

/// [`Irreversible`] 의 관측 자리.
#[derive(Default)]
struct PointProbe {
    started: Notify,
    before: Notify,
    decided: Notify,
    after: Notify,
    passed: std::sync::Mutex<Option<bool>>,
    reached: AtomicBool,
}

fn point_router() -> (Router, Arc<PointProbe>) {
    let probe = Arc::new(PointProbe::default());
    let mut router = base_router();
    let p = Arc::clone(&probe);
    router.unary(Irreversible, move |ctx, ()| {
        let p = Arc::clone(&p);
        let point = ctx.point_of_no_return();
        async move {
            // 효과를 내는 쪽은 핸들러가 버려져도 도는 태스크다(`db_execute` 의 실행 태스크처럼).
            let task = tokio::spawn(async move {
                p.started.notify_one();
                p.before.notified().await;
                let passed = point.pass();
                *p.passed.lock().unwrap() = Some(passed);
                p.decided.notify_one();
                p.after.notified().await;
                p.reached.store(true, Ordering::SeqCst);
                passed
            });
            Ok::<_, RpcError>(task.await.unwrap())
        }
    });
    router.stream(PointInStream, move |ctx, (), sink| async move {
        sink.send(ctx.point_of_no_return().pass()).await
    });
    (router, probe)
}

async fn notified(n: &Notify, what: &str) {
    timeout(Duration::from_secs(1), n.notified())
        .await
        .unwrap_or_else(|_| panic!("{what} never happened"));
}

/// 「되돌릴 수 없는 지점」 을 세운 뒤의 취소는 핸들러를 버리지 않는다 — 끝까지 기다려 그 결과로 답한다(16-m5-plan M5-18 이월).
#[tokio::test]
async fn a_cancel_after_the_point_of_no_return_waits_for_the_handler_and_answers_its_result() {
    let (router, probe) = point_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.irreversible", Value::Null);
    notified(&probe.started, "start").await;
    probe.before.notify_one();
    notified(&probe.decided, "the point").await;
    assert_eq!(*probe.passed.lock().unwrap(), Some(true));

    h.notify("request.cancel", json!({"id": 1}));
    // 표지 뒤라 취소로 답하지 않는다 — 핸들러가 끝날 때까지 아무것도 나가지 않는다.
    assert!(
        timeout(Duration::from_millis(100), h.next()).await.is_err(),
        "answered before the handler finished"
    );
    probe.after.notify_one();
    let res = h.next_response().await;
    assert_eq!(res.id, Id::Number(1));
    assert_eq!(res.outcome, Ok(json!(true)));
    assert!(probe.reached.load(Ordering::SeqCst));
}

/// 표지를 세우기 전에 온 취소는 지금처럼 핸들러를 버리고 `cancelled` 로 답한다. 그 뒤 표지를 세우려 하면 `false` — 효과를 내지
/// 말고 되돌린다.
#[tokio::test]
async fn a_cancel_before_the_point_of_no_return_drops_the_handler_and_the_point_refuses() {
    let (router, probe) = point_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.irreversible", Value::Null);
    notified(&probe.started, "start").await;
    h.notify("request.cancel", json!({"id": 1}));
    let res = h.next_response().await;
    assert_eq!(err_code(&res), code::CANCELLED);

    probe.before.notify_one();
    notified(&probe.decided, "the point").await;
    assert_eq!(*probe.passed.lock().unwrap(), Some(false));
}

/// 세션이 닫혀도 표지 뒤의 핸들러는 끝까지 돈다(그 효과는 이미 정해졌다).
#[tokio::test]
async fn closing_the_session_after_the_point_of_no_return_lets_the_handler_finish() {
    let (router, probe) = point_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.irreversible", Value::Null);
    notified(&probe.started, "start").await;
    probe.before.notify_one();
    notified(&probe.decided, "the point").await;
    h.session.close();
    probe.after.notify_one();
    let res = h.next_response().await;
    assert_eq!(res.outcome, Ok(json!(true)));
    assert!(probe.reached.load(Ordering::SeqCst));
}

/// 스트림 호출의 표지는 서지 않는다 — 스트림은 취소되면 그대로 버려진다.
#[tokio::test]
async fn the_point_of_no_return_never_passes_in_a_stream() {
    let (router, _) = point_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.pointInStream", Value::Null);
    let id = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    h.notify("stream.credit", json!({"streamId": id, "credit": 1}));
    let chunk: StreamChunk = h.next_notification(STREAM_CHUNK).await;
    assert_eq!(chunk.data, json!(false));
}

rpc_method! {
    /// 버려질 때 느리게 정리하는 핸들러(정리가 끝나면 플래그를 세운다).
    SlowDrop: "test.slowDrop" { params: (), result: (), tier: Read }
}

/// 버려질 때 잠깐 걸려 정리하고 플래그를 세운다.
struct Cleanup(Arc<AtomicBool>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(100));
        self.0.store(true, Ordering::SeqCst);
    }
}

/// 표지 전의 취소는 핸들러를 **버린 뒤에** 답한다(리뷰 R12) — 취소 응답을 받은 쪽은 핸들러의 정리(`db_execute` 의 `CancelOnDrop`
/// 이 보내는 인터럽트 등)가 이미 끝났다고 믿어도 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_handler_is_dropped_before_the_cancel_is_answered() {
    let cleaned = Arc::new(AtomicBool::new(false));
    let started = Arc::new(Notify::new());
    let mut router = base_router();
    let (flag, start) = (Arc::clone(&cleaned), Arc::clone(&started));
    router.unary(SlowDrop, move |_ctx, ()| {
        let guard = Cleanup(Arc::clone(&flag));
        let start = Arc::clone(&start);
        async move {
            let _guard = guard;
            start.notify_one();
            std::future::pending::<()>().await;
            Ok::<_, RpcError>(())
        }
    });
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.slowDrop", Value::Null);
    timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("handler never started");
    h.notify("request.cancel", json!({"id": 1}));
    let res = h.next_response().await;
    assert_eq!(err_code(&res), code::CANCELLED);
    assert!(
        cleaned.load(Ordering::SeqCst),
        "the cancel was answered before the handler was dropped"
    );
}
