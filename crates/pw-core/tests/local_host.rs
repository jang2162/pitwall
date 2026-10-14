//! LocalHost 를 `Host` trait 의 연결로만 다룬다 — transport 가 보는 것과 같은 JSON 메시지로 왕복한다.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::DEFAULT_CAPACITY;
use pw_core::methods::{DemoTaskUpdate, HostInfo, Pong};
use pw_core::task::{TOPIC_FINISHED, TOPIC_PROGRESS, TaskFinishedEvent, TaskProgressEvent};
use pw_core::{
    Connection, Filter, Host, HostRouter, LocalHost, Received, Scope, Target, TaskProgress,
    TaskStatus,
};
use pw_rpc::protocol::{EventMessage, StreamEnd, StreamOpened, StreamOutcome};
use pw_rpc::{Caller, Role, code};
use serde_json::{Value, json};

struct Client {
    conn: Connection,
}

impl Client {
    async fn connect(host: &LocalHost, role: Role) -> Self {
        Self {
            conn: host.connect(Caller::new(role)).await.unwrap(),
        }
    }

    fn request(&self, id: i64, method: &str, params: Value) {
        self.conn
            .sink
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn notify(&self, method: &str, params: Value) {
        self.conn
            .sink
            .send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), self.conn.source.recv())
            .await
            .expect("timed out waiting for a frame")
            .expect("connection closed")
    }

    async fn result(&mut self, id: i64) -> Value {
        let frame = self.next().await;
        assert_eq!(frame["id"], json!(id), "{frame}");
        assert!(frame.get("error").is_none(), "{frame}");
        frame["result"].clone()
    }

    async fn notification(&mut self, method: &str) -> Value {
        let frame = self.next().await;
        assert_eq!(frame["method"], json!(method), "{frame}");
        frame["params"].clone()
    }
}

fn from<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

#[tokio::test]
async fn ping_and_info_round_trip_as_json() {
    let host = LocalHost::new("local");
    let mut c = Client::connect(&host, Role::Owner).await;

    c.request(1, "host.ping", json!({"nonce": "abc"}));
    let pong: Pong = from(c.result(1).await);
    assert_eq!(
        pong,
        Pong {
            nonce: "abc".into(),
            host_id: "local".into()
        }
    );

    c.request(2, "host.info", Value::Null);
    let info: HostInfo = from(c.result(2).await);
    assert_eq!(info.host_id, "local");
    assert_eq!(info.kind, "local");
    assert_eq!(info.os, std::env::consts::OS);
}

#[tokio::test]
async fn router_connects_a_project_to_its_host() {
    let host = Arc::new(LocalHost::new("local"));
    let router = HostRouter::new();
    router.add_host(host.clone()).unwrap();
    router.bind_project("p1".into(), "local").unwrap();

    let conn = router
        .connect(&Target::ProjectId("p1".into()), Caller::new(Role::Owner))
        .await
        .unwrap();
    let mut c = Client { conn };
    c.request(1, "host.ping", json!({}));
    let pong: Pong = from(c.result(1).await);
    assert_eq!(pong.host_id, "local");
}

#[tokio::test(start_paused = true)]
async fn demo_task_streams_progress_then_completion() {
    let host = LocalHost::new("local");
    let mut c = Client::connect(&host, Role::Owner).await;
    c.request(1, "host.demoTask", json!({"steps": 3, "intervalMs": 10}));
    let opened: StreamOpened = from(c.result(1).await);
    c.notify(
        "stream.credit",
        json!({"streamId": opened.stream_id, "credit": 100}),
    );

    let mut updates = Vec::new();
    loop {
        let chunk = c.notification("stream.chunk").await;
        let update: DemoTaskUpdate = from(chunk["data"].clone());
        let done = matches!(update, DemoTaskUpdate::Finished { .. });
        updates.push(update);
        if done {
            break;
        }
    }
    let dones: Vec<_> = updates
        .iter()
        .filter_map(|u| match u {
            DemoTaskUpdate::Progress { progress, .. } => Some(progress.done),
            DemoTaskUpdate::Finished { .. } => None,
        })
        .collect();
    assert_eq!(dones, [1, 2, 3]);
    assert!(matches!(
        updates.last(),
        Some(DemoTaskUpdate::Finished {
            status: TaskStatus::Completed,
            ..
        })
    ));
    let end: StreamEnd = from(c.notification("stream.end").await);
    assert_eq!(end.outcome, StreamOutcome::Completed);
}

/// 데모 작업을 열고 첫 진행 알림까지 받은 뒤, 그 작업의 종료 알림을 기다릴 구독과 함께 돌려준다.
async fn start_demo(host: &LocalHost, c: &mut Client) -> (u64, pw_core::Subscription) {
    let finished = host
        .bus()
        .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_FINISHED]));
    c.request(1, "host.demoTask", json!({"steps": 1000, "intervalMs": 10}));
    let opened: StreamOpened = from(c.result(1).await);
    c.notify(
        "stream.credit",
        json!({"streamId": opened.stream_id, "credit": 1}),
    );
    let chunk = c.notification("stream.chunk").await;
    assert!(matches!(
        from::<DemoTaskUpdate>(chunk["data"].clone()),
        DemoTaskUpdate::Progress { .. }
    ));
    (opened.stream_id, finished)
}

async fn finished_status(sub: &mut pw_core::Subscription) -> TaskStatus {
    let received = tokio::time::timeout(Duration::from_secs(5), sub.recv())
        .await
        .expect("task never finished");
    match received {
        Some(Received::Event(e)) => from::<TaskFinishedEvent>(e.payload.clone()).status,
        other => panic!("expected a finished event, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn cancelling_the_stream_cancels_the_task() {
    let host = LocalHost::new("local");
    let mut c = Client::connect(&host, Role::Owner).await;
    let (stream_id, mut finished) = start_demo(&host, &mut c).await;

    c.notify("stream.cancel", json!({"streamId": stream_id}));
    let end: StreamEnd = from(c.notification("stream.end").await);
    assert_eq!(end.outcome, StreamOutcome::Cancelled);
    assert_eq!(finished_status(&mut finished).await, TaskStatus::Cancelled);
}

#[tokio::test(start_paused = true)]
async fn closing_the_connection_cancels_the_task() {
    let host = LocalHost::new("local");
    let mut c = Client::connect(&host, Role::Owner).await;
    let (_stream_id, mut finished) = start_demo(&host, &mut c).await;

    drop(c.conn.sink);
    assert_eq!(finished_status(&mut finished).await, TaskStatus::Cancelled);
}

/// 스트림 핸들러가 credit 을 기다리는 사이 다른 작업의 진행 알림이 구독 상한을 넘게 쌓여 이 작업의 종료 알림이
/// 버려져도, 스트림은 작업 상태를 보고 끝나야 한다(리뷰 F1 재현).
#[tokio::test(start_paused = true)]
async fn demo_stream_ends_even_if_its_finish_event_is_dropped() {
    let host = LocalHost::new("local");
    let mut c = Client::connect(&host, Role::Owner).await;
    let mut finished = host
        .bus()
        .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_FINISHED]));
    c.request(1, "host.demoTask", json!({"steps": 1, "intervalMs": 10}));
    let opened: StreamOpened = from(c.result(1).await);
    // credit 을 주지 않으므로 핸들러는 첫 진행 청크를 보내려다 멈춘 채 작업이 끝난다.
    assert_eq!(finished_status(&mut finished).await, TaskStatus::Completed);

    for done in 0..(DEFAULT_CAPACITY as u64 + 44) {
        let other = TaskProgressEvent {
            task_id: 9999,
            title: "other".into(),
            progress: TaskProgress {
                done,
                ..TaskProgress::default()
            },
        };
        host.bus()
            .publish(Scope::Host, TOPIC_PROGRESS, &other)
            .unwrap();
    }
    c.notify(
        "stream.credit",
        json!({"streamId": opened.stream_id, "credit": 100}),
    );

    let mut last = None;
    let end = loop {
        let frame = c.next().await;
        match frame["method"].as_str() {
            Some("stream.chunk") => {
                last = Some(from::<DemoTaskUpdate>(frame["params"]["data"].clone()))
            }
            Some("stream.end") => break from::<StreamEnd>(frame["params"].clone()),
            _ => panic!("unexpected frame {frame}"),
        }
    };
    assert_eq!(end.outcome, StreamOutcome::Completed);
    assert!(matches!(
        last,
        Some(DemoTaskUpdate::Finished {
            status: TaskStatus::Completed,
            ..
        })
    ));
}

#[tokio::test]
async fn invalid_demo_params_are_rejected() {
    let host = LocalHost::new("local");
    let mut c = Client::connect(&host, Role::Owner).await;
    c.request(1, "host.demoTask", json!({"steps": 0, "intervalMs": 10}));
    let opened: StreamOpened = from(c.result(1).await);
    let end: StreamEnd = from(c.notification("stream.end").await);
    assert_eq!(end.stream_id, opened.stream_id);
    match end.outcome {
        StreamOutcome::Failed { error } => assert_eq!(error.code, code::INVALID_PARAMS),
        other => panic!("expected failure, got {other:?}"),
    }
}

#[tokio::test]
async fn project_events_reach_rpc_subscribers() {
    let host = LocalHost::new("local");
    let project = host
        .open_project("p1".into(), PathBuf::from("/tmp/p1"))
        .unwrap();
    let mut c = Client::connect(&host, Role::Readonly).await;
    c.request(
        1,
        "events.subscribe",
        json!({"projectId": "p1", "topics": ["fs.changed"]}),
    );
    c.result(1).await;

    assert_eq!(
        project
            .publish("fs.changed", &json!({"path": "a"}))
            .unwrap(),
        0
    );
    let event: EventMessage = from(c.notification("events.event").await);
    assert_eq!(event.project_id, "p1");
    assert_eq!(event.payload, json!({"path": "a"}));
}

#[tokio::test(start_paused = true)]
async fn closing_a_project_cancels_its_tasks_and_frees_the_id() {
    let host = LocalHost::new("local");
    let project = host
        .open_project("p1".into(), PathBuf::from("/tmp/p1"))
        .unwrap();
    assert_eq!(project.root(), PathBuf::from("/tmp/p1"));
    assert!(host.open_project("p1".into(), PathBuf::from("/x")).is_err());

    let mut handle = project.tasks().spawn("forever", |_ctx| async {
        std::future::pending::<()>().await;
        Ok(())
    });
    assert!(host.close_project("p1"));
    // 시간 제한: 취소가 전파되지 않는 회귀는 무한 대기가 아니라 실패로 끝나야 한다.
    let status = tokio::time::timeout(Duration::from_secs(5), handle.wait())
        .await
        .expect("project task was not cancelled");
    assert_eq!(status, TaskStatus::Cancelled);
    assert!(project.cancellation().is_cancelled());
    assert!(host.project("p1").is_none());
    assert!(
        host.open_project("p1".into(), PathBuf::from("/tmp/p1"))
            .is_ok()
    );
}
