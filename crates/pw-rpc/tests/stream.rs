//! 스트림: credit 백프레셔, 종료 상태(completed · failed · cancelled).

mod common;

use common::{APP_ERROR, Harness, base_router};
use std::sync::Arc;

use pw_rpc::protocol::StreamCancelParams;
use pw_rpc::protocol::{
    STREAM_CHUNK, STREAM_END, StreamChunk, StreamEnd, StreamId, StreamOpened, StreamOutcome,
};
use pw_rpc::{Role, RpcError, StreamSink, code, rpc_method};
use serde_json::json;
use tokio::sync::{Notify, oneshot};

async fn open_count(h: &mut Harness, n: u32) -> StreamId {
    h.request(1, "test.count", json!({"n": n}));
    h.next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id
}

fn credit(h: &Harness, stream_id: StreamId, credit: u32) {
    h.notify(
        "stream.credit",
        json!({"streamId": stream_id, "credit": credit}),
    );
}

async fn chunk(h: &mut Harness, stream_id: StreamId) -> u32 {
    let chunk: StreamChunk = h.next_notification(STREAM_CHUNK).await;
    assert_eq!(chunk.stream_id, stream_id);
    serde_json::from_value(chunk.data).unwrap()
}

#[tokio::test(start_paused = true)]
async fn server_sends_only_as_many_chunks_as_credit_allows() {
    let mut h = Harness::new(base_router(), Role::Owner);
    let id = open_count(&mut h, 5).await;

    // 초기 credit 은 0 이다.
    h.assert_quiet().await;

    credit(&h, id, 2);
    assert_eq!(chunk(&mut h, id).await, 0);
    assert_eq!(chunk(&mut h, id).await, 1);
    // 핸들러는 보낼 것이 3개 더 있지만 credit 이 바닥났다.
    h.assert_quiet().await;

    credit(&h, id, 3);
    for expected in 2..5 {
        assert_eq!(chunk(&mut h, id).await, expected);
    }
    let end: StreamEnd = h.next_notification(STREAM_END).await;
    assert_eq!(
        end,
        StreamEnd {
            stream_id: id,
            outcome: StreamOutcome::Completed
        }
    );
}

#[tokio::test(start_paused = true)]
async fn unused_credit_carries_over_and_nothing_follows_the_end() {
    let mut h = Harness::new(base_router(), Role::Owner);
    let id = open_count(&mut h, 2).await;
    credit(&h, id, 10);
    assert_eq!(chunk(&mut h, id).await, 0);
    assert_eq!(chunk(&mut h, id).await, 1);
    let end: StreamEnd = h.next_notification(STREAM_END).await;
    assert_eq!(end.outcome, StreamOutcome::Completed);
    // 끝난 스트림에 대한 제어는 조용히 무시된다.
    credit(&h, id, 1);
    h.notify("stream.cancel", json!({"streamId": id}));
    h.assert_quiet().await;
}

#[tokio::test(start_paused = true)]
async fn handler_error_ends_the_stream_as_failed() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "test.count", json!({"n": 5, "fail_after": 0}));
    let id = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    credit(&h, id, 5);
    assert_eq!(chunk(&mut h, id).await, 0);
    let end: StreamEnd = h.next_notification(STREAM_END).await;
    match end.outcome {
        StreamOutcome::Failed { error } => assert_eq!(error.code, APP_ERROR),
        other => panic!("expected failed, got {other:?}"),
    }
    h.assert_quiet().await;
}

#[tokio::test(start_paused = true)]
async fn concurrent_streams_have_independent_credit() {
    let mut h = Harness::new(base_router(), Role::Owner);
    h.request(1, "test.count", json!({"n": 3}));
    let a = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    h.request(2, "test.count", json!({"n": 3}));
    let b = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    assert_ne!(a, b);

    credit(&h, b, 1);
    assert_eq!(chunk(&mut h, b).await, 0);
    h.assert_quiet().await;
}

#[test]
fn stream_end_wire_format_names_the_status() {
    let failed = StreamEnd {
        stream_id: 3,
        outcome: StreamOutcome::Failed {
            error: pw_rpc::RpcError::new(7, "x"),
        },
    };
    assert_eq!(
        serde_json::to_value(&failed).unwrap(),
        json!({"streamId": 3, "status": "failed", "error": {"code": 7, "message": "x"}})
    );
    let cancelled = StreamEnd {
        stream_id: 3,
        outcome: StreamOutcome::Cancelled,
    };
    assert_eq!(
        serde_json::to_value(&cancelled).unwrap(),
        json!({"streamId": 3, "status": "cancelled"})
    );
}

rpc_method! {
    /// async 본문에서 패닉한다.
    PanicStream: "test.panicStream" { params: (), stream: u32, tier: Read }

    /// sink 를 떼어 낸 태스크로 옮기고, `go` 가 오면 끝난다.
    Detached: "test.detached" { params: (), stream: u32, tier: Read }

    /// sink 를 떼어 낸 태스크로 옮기고 끝나지 않는다.
    DetachedForever: "test.detachedForever" { params: (), stream: u32, tier: Read }
}

fn boom() -> Result<(), RpcError> {
    panic!("stream handler panic for test")
}

#[tokio::test(start_paused = true)]
async fn handler_panic_ends_the_stream_as_failed() {
    let mut router = base_router();
    router.stream(PanicStream, |_ctx, (), _sink: StreamSink<u32>| async move {
        boom()
    });
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.panicStream", serde_json::Value::Null);
    let id = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    let end: StreamEnd = h.next_notification(STREAM_END).await;
    assert_eq!(end.stream_id, id);
    match end.outcome {
        StreamOutcome::Failed { error } => assert_eq!(error.code, code::INTERNAL_ERROR),
        other => panic!("expected failed, got {other:?}"),
    }
}

/// 떼어 낸 sink 의 전송 시점(`gate`)과 그 결과를 쥔 탐침.
struct DetachProbe {
    go: Arc<Notify>,
    gate: Arc<Notify>,
    started: Arc<Notify>,
    result: oneshot::Receiver<Result<(), RpcError>>,
}

fn detached_router() -> (pw_rpc::Router, DetachProbe) {
    let (go, gate, started) = (
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
    );
    let (tx, result) = oneshot::channel();
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
    let mut router = base_router();
    for (method, finishes) in [("detached", true), ("forever", false)] {
        let (go, gate, started, tx) = (go.clone(), gate.clone(), started.clone(), tx.clone());
        let handler = move |_ctx, (), sink: StreamSink<u32>| {
            let (go, gate, started, tx) = (go.clone(), gate.clone(), started.clone(), tx.clone());
            async move {
                tokio::spawn(async move {
                    gate.notified().await;
                    let sent = sink.send(7).await;
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(sent);
                    }
                });
                started.notify_one();
                if finishes {
                    go.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
                Ok(())
            }
        };
        match method {
            "detached" => router.stream(Detached, handler),
            _ => router.stream(DetachedForever, handler),
        };
    }
    (
        router,
        DetachProbe {
            go,
            gate,
            started,
            result,
        },
    )
}

#[tokio::test(start_paused = true)]
async fn detached_sink_cannot_send_after_the_stream_completed() {
    let (router, probe) = detached_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.detached", serde_json::Value::Null);
    let id = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    // 떼어 낸 send 가 쓸 credit 이 남아 있게 한다.
    credit(&h, id, 5);
    probe.started.notified().await;
    probe.go.notify_one();
    let end: StreamEnd = h.next_notification(STREAM_END).await;
    assert_eq!(end.outcome, StreamOutcome::Completed);

    probe.gate.notify_one();
    let sent = probe.result.await.unwrap();
    assert_eq!(sent.unwrap_err().code, code::CANCELLED);
    h.assert_quiet().await;
}

#[tokio::test(start_paused = true)]
async fn detached_sink_cannot_send_after_the_stream_was_cancelled() {
    let (router, probe) = detached_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.detachedForever", serde_json::Value::Null);
    let id = h
        .next_response()
        .await
        .result_as::<StreamOpened>()
        .unwrap()
        .stream_id;
    credit(&h, id, 5);
    probe.started.notified().await;
    h.notify(
        "stream.cancel",
        serde_json::to_value(StreamCancelParams { stream_id: id }).unwrap(),
    );
    let end: StreamEnd = h.next_notification(STREAM_END).await;
    assert_eq!(end.outcome, StreamOutcome::Cancelled);

    probe.gate.notify_one();
    let sent = probe.result.await.unwrap();
    assert_eq!(sent.unwrap_err().code, code::CANCELLED);
    h.assert_quiet().await;
}

#[tokio::test(start_paused = true)]
async fn detached_sink_waiting_for_credit_is_woken_with_an_error_when_the_stream_ends() {
    let (router, probe) = detached_router();
    let mut h = Harness::new(router, Role::Owner);
    h.request(1, "test.detached", serde_json::Value::Null);
    h.next_response().await.result_as::<StreamOpened>().unwrap();
    probe.started.notified().await;
    // credit 을 주지 않는다. 떼어 낸 send 는 credit 을 기다리며 멈춘다.
    probe.gate.notify_one();
    tokio::task::yield_now().await;
    probe.go.notify_one();
    let end: StreamEnd = h.next_notification(STREAM_END).await;
    assert_eq!(end.outcome, StreamOutcome::Completed);

    let sent = tokio::time::timeout(std::time::Duration::from_secs(1), probe.result)
        .await
        .expect("detached send was never woken")
        .unwrap();
    assert_eq!(sent.unwrap_err().code, code::CANCELLED);
    h.assert_quiet().await;
}
