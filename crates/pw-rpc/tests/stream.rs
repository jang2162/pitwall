//! 스트림: credit 백프레셔, 종료 상태(completed · failed · cancelled).

mod common;

use common::{APP_ERROR, Harness, base_router};
use pw_rpc::Role;
use pw_rpc::protocol::{
    STREAM_CHUNK, STREAM_END, StreamChunk, StreamEnd, StreamId, StreamOpened, StreamOutcome,
};
use serde_json::json;

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
