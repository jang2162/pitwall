use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pw_rpc::{Caller, ConnectionScope, Role, code};
use serde_json::{Value, json};

use super::*;
use crate::config::AppPaths;
use crate::open::OpenRequests;
use crate::{Host, LocalHost};

fn request() -> OpenRequest {
    OpenRequest {
        project_id: "p1".into(),
        worktree: Some("/work/wt".into()),
        file: Some(PathBuf::from("/work/wt/README.md")),
        line: Some(3),
        column: None,
    }
}

/// 질문에는 `idle` 로, 열기에는 열었다고 답하는 Desktop 흉내. 받은 열기를 센다.
fn desktop(
    desks: &Desks,
    idle: Option<u64>,
) -> (Arc<Mutex<Vec<OpenEvent>>>, tokio::task::JoinHandle<()>) {
    let mut listening = desks.listen();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (desks, record) = (desks.clone(), Arc::clone(&seen));
    let task = tokio::spawn(async move {
        let token = listening.token().to_owned();
        while let Some(event) = listening.recv().await {
            match &event {
                OpenEvent::Query { request_id } => {
                    if let Some(idle_ms) = idle {
                        desks.answer(&token, *request_id, OpenAnswer::Idle { idle_ms });
                    }
                }
                OpenEvent::Open { request_id, .. } => {
                    desks.answer(
                        &token,
                        *request_id,
                        OpenAnswer::Opened {
                            opened_in: OpenedIn::NewWindow,
                        },
                    );
                }
                OpenEvent::Listening { .. } => {}
            }
            record.lock().unwrap().push(event);
        }
    });
    (seen, task)
}

fn opens(seen: &Mutex<Vec<OpenEvent>>) -> Vec<OpenEvent> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, OpenEvent::Open { .. }))
        .cloned()
        .collect()
}

/// 행선지는 듣는 Desktop 가운데 가장 최근에 사람 입력이 있던 것 하나다(결정 M7-D3, 메인 판정 2026-11-23) — 그 Desktop 에 창이
/// 있는지는 묻지 않는다. 더 오래 전에 입력이 있던 Desktop 에는 가지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desktop_with_the_most_recent_input_opens_it() {
    let desks = Desks::default();
    let (stale, _a) = desktop(&desks, Some(600_000));
    let (recent, _b) = desktop(&desks, Some(1_500));
    let (older, _c) = desktop(&desks, Some(90_000));
    let opened = desks.open(&request()).await;
    assert_eq!(opened, Ok(OpenedIn::NewWindow));
    assert!(opens(&stale).is_empty());
    assert!(opens(&older).is_empty());
    assert_eq!(
        opens(&recent),
        [OpenEvent::Open {
            request_id: match opens(&recent)[0] {
                OpenEvent::Open { request_id, .. } => request_id,
                _ => unreachable!(),
            },
            project_id: "p1".into(),
            worktree: Some("/work/wt".into()),
            path: Some("/work/wt/README.md".into()),
            line: Some(3),
            column: None,
        }]
    );
}

/// 듣는 Desktop 이 없으면 기다리지 않고 거절한다. 듣기만 하고 답하지 않는 Desktop 만 있어도 거절이다(질문 시간 뒤).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_desktop_the_open_is_refused() {
    let desks = Desks::default();
    let started = std::time::Instant::now();
    assert_eq!(desks.open(&request()).await, Err(DeskError::NoDesktop));
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(DeskError::NoDesktop.to_string(), NO_DESKTOP);

    let (silent, _task) = desktop(&desks, None);
    assert_eq!(desks.open(&request()).await, Err(DeskError::NoDesktop));
    assert!(opens(&silent).is_empty());
}

/// 답은 그 듣기의 표로만 받는다 — 다른 표 · 끝난 요청 · 꼴이 틀린 답은 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_need_the_listeners_token_and_a_question() {
    let desks = Desks::default();
    let mut listening = desks.listen();
    let token = listening.token().to_owned();
    let asking = tokio::spawn({
        let desks = desks.clone();
        async move { desks.open(&request()).await }
    });
    let Some(OpenEvent::Query { request_id }) = listening.recv().await else {
        panic!("no query");
    };
    assert!(!desks.answer(
        "0123456789abcdef0123456789abcdef",
        request_id,
        OpenAnswer::Idle { idle_ms: 0 }
    ));
    assert!(!desks.answer(&token, request_id + 1000, OpenAnswer::Idle { idle_ms: 0 }));
    assert!(!desks.answer(
        &token,
        request_id,
        OpenAnswer::Opened {
            opened_in: OpenedIn::ExistingWindow
        }
    ));
    assert!(desks.answer(&token, request_id, OpenAnswer::Idle { idle_ms: 5 }));
    assert!(!desks.answer(&token, request_id, OpenAnswer::Idle { idle_ms: 5 }));
    let Some(OpenEvent::Open { request_id, .. }) = listening.recv().await else {
        panic!("no open");
    };
    assert!(desks.answer(
        &token,
        request_id,
        OpenAnswer::Failed {
            message: "nope".into()
        }
    ));
    assert_eq!(asking.await.unwrap(), Err(DeskError::Failed("nope".into())));
    assert_eq!(desks.listeners(), 1);
    drop(listening);
    assert_eq!(desks.listeners(), 0);
}

struct Conn(crate::Connection, i64);

impl Conn {
    async fn open(host: &LocalHost, caller: Caller) -> Self {
        Self(host.connect(caller).await.unwrap(), 0)
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(10), self.0.source.recv())
            .await
            .expect("no frame in time")
            .expect("closed")
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.1 += 1;
        let id = self.1;
        self.0
            .sink
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let frame = self.next().await;
            if frame["id"] == json!(id) {
                return frame;
            }
        }
    }

    /// `open.listen` 을 연다. 서면 첫 항목(듣기 표)을, 거절되면 그 오류를.
    async fn listen(&mut self) -> Result<Value, Value> {
        let opened = self.call("open.listen", json!({})).await;
        if !opened["error"].is_null() {
            return Err(opened["error"].clone());
        }
        let stream = opened["result"]["streamId"].clone();
        self.0.sink.send(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream, "credit": 16}}));
        loop {
            let frame = self.next().await;
            match frame["method"].as_str() {
                Some("stream.chunk") => return Ok(frame["params"]["data"].clone()),
                Some("stream.end") => return Err(frame["params"]["error"].clone()),
                _ => {}
            }
        }
    }
}

/// `open.listen` · `open.answer` 는 owner 의 Host 범위 연결만 받는다 — 프로젝트 창 · 에이전트(MCP) 연결 · owner 가 아닌 role 은
/// 거절한다(메인 판정 2026-11-23 — 표 시험).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_an_owner_host_connection_listens_for_opens() {
    let tmp = tempfile::tempdir().unwrap();
    let host = LocalHost::builder("local")
        .config(AppPaths::under(tmp.path()))
        .open_requests(OpenRequests::default())
        .build();
    let cases = [
        (Caller::new(Role::Owner), true),
        (
            Caller {
                role: Role::Owner,
                scope: ConnectionScope::Project("p1".into()),
                window: None,
            },
            false,
        ),
        (Caller::agent(Role::Owner), false),
        (Caller::new(Role::Readonly), false),
        (Caller::new(Role::Mobile), false),
    ];
    for (caller, admitted) in cases {
        let what = format!("{caller:?}");
        let mut conn = Conn::open(&host, caller).await;
        let listened = conn.listen().await;
        assert_eq!(listened.is_ok(), admitted, "{what}: {listened:?}");
        let answered = conn
            .call(
                "open.answer",
                json!({"listener": "x", "requestId": 1, "answer": {"kind": "idle", "idleMs": 1}}),
            )
            .await;
        let code = answered["error"]["code"].as_i64();
        if admitted {
            assert_eq!(listened.unwrap()["kind"], "listening", "{what}");
            assert_eq!(code, Some(i64::from(code::NOT_FOUND)), "{what}: {answered}");
        } else {
            assert_ne!(code, Some(i64::from(code::NOT_FOUND)), "{what}: {answered}");
            assert!(code.is_some(), "{what}: {answered}");
        }
    }
}
