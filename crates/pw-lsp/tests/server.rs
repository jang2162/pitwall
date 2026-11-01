//! 가짜 서버(`pw-lsp-fake`)로 본 연결 · 수명(15-m4-plan.md M4-01 완료 기준 ① ~ ⑤ · ⑦).

mod common;

use std::time::{Duration, Instant};

use pw_lsp::fake::{Action, Reply, Scenario};
use pw_lsp::message::code;
#[cfg(unix)]
use pw_lsp::server::{EXIT_GRACE, SHUTDOWN_TIMEOUT, STOP_BUDGET};
use pw_lsp::{ExitReason, InitializeOptions, RequestError, RequestOptions, ServerEvent};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use common::{eventually, next_event, project, received};

const SOON: Duration = Duration::from_secs(5);

fn method_of(v: &Value) -> Option<&str> {
    v.get("method").and_then(Value::as_str)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_stores_capabilities_and_sends_initialized() {
    let p = project();
    let scenario = Scenario {
        capabilities: json!({ "hoverProvider": true, "definitionProvider": false }),
        ..Scenario::default()
    };
    let f = p.fake("init", scenario, false).await;
    let result = f
        .server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    assert_eq!(result.server_info.unwrap().name, "pw-lsp-fake");
    assert!(f.server.supports("textDocument/hover"));
    assert!(!f.server.supports("textDocument/definition"));
    assert!(
        eventually(SOON, || {
            received(&f.record)
                .iter()
                .any(|m| method_of(m) == Some("initialized"))
        })
        .await
    );
    let init = received(&f.record)
        .into_iter()
        .find(|m| method_of(m) == Some("initialize"))
        .unwrap();
    let params = &init["params"];
    assert_eq!(
        params["capabilities"]["general"]["positionEncodings"],
        json!(["utf-16"])
    );
    assert_eq!(params["processId"], json!(std::process::id()));
    let uri = params["rootUri"].as_str().unwrap();
    assert!(
        uri.starts_with("file:///") && uri.ends_with("/project"),
        "{uri}"
    );
    assert_eq!(params["workspaceFolders"][0]["uri"], json!(uri));
    let stopped = f.server.shutdown().await;
    assert!(stopped.answered_shutdown);
    assert!(!stopped.killed);
    assert_eq!(f.server.exit_status().unwrap().reason, ExitReason::Stopped);
}

/// ② 요청을 보낸 뒤 취소하면 `$/cancelRequest` 가 나가고, 늦게 온 응답은 버린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_request_sends_cancel_and_drops_the_late_answer() {
    let p = project();
    let scenario = Scenario::default()
        .reply("slow/one", Reply::result(json!("late")).delayed(400))
        .reply("fast/one", Reply::result(json!("fast")));
    let f = p.fake("cancel", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();

    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });
    let got = f
        .server
        .request_raw("slow/one", Value::Null, RequestOptions::cancel(token))
        .await;
    assert_eq!(got, Err(RequestError::Cancelled));
    let slow_id = received(&f.record)
        .into_iter()
        .find(|m| method_of(m) == Some("slow/one"))
        .unwrap()["id"]
        .clone();
    assert!(
        eventually(SOON, || {
            received(&f.record)
                .iter()
                .any(|m| method_of(m) == Some("$/cancelRequest") && m["params"]["id"] == slow_id)
        })
        .await
    );
    // 늦은 응답이 와도 다음 요청은 제 답을 받고, 늦은 것은 버린 수로만 남는다.
    assert!(eventually(SOON, || f.server.late_responses() == 1).await);
    let fast = f
        .server
        .request_raw("fast/one", Value::Null, RequestOptions::default())
        .await;
    assert_eq!(fast, Ok(json!("fast")));
    assert_eq!(f.server.late_responses(), 1);

    // 부른 쪽이 기다리던 future 를 버려도 같다.
    let dropped = tokio::time::timeout(
        Duration::from_millis(50),
        f.server
            .request_raw("slow/one", Value::Null, RequestOptions::default()),
    )
    .await;
    assert!(dropped.is_err());
    assert!(
        eventually(SOON, || {
            received(&f.record)
                .iter()
                .filter(|m| method_of(m) == Some("$/cancelRequest"))
                .count()
                == 2
        })
        .await
    );
    f.server.shutdown().await;
}

/// 요청마다 시한: 넘으면 `Timeout` 이고 `$/cancelRequest` 를 보낸다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_past_its_deadline_times_out_and_is_cancelled() {
    let p = project();
    let scenario = Scenario::default().reply("never/answers", Reply::silent());
    let f = p.fake("deadline", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    let started = Instant::now();
    let got = f
        .server
        .request_raw(
            "never/answers",
            Value::Null,
            RequestOptions::timeout(Duration::from_millis(200)),
        )
        .await;
    assert_eq!(got, Err(RequestError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        eventually(SOON, || {
            received(&f.record)
                .iter()
                .any(|m| method_of(m) == Some("$/cancelRequest"))
        })
        .await
    );
    assert_eq!(
        RequestOptions::default().timeout,
        Duration::from_secs(10),
        "the default request deadline"
    );
    f.server.shutdown().await;
}

/// ③ `initialize` 에 10 s 답하지 않으면 시한 오류이고 프로세스가 없다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_initialize_unanswered_for_10_s_times_out_and_leaves_no_process() {
    let p = project();
    let scenario = Scenario::default().reply("initialize", Reply::silent());
    let f = p.fake("init-timeout", scenario, false).await;
    let pid = f.server.pid().unwrap();
    assert!(common::alive(pid));
    let started = Instant::now();
    let options = InitializeOptions::new(&p.root);
    assert_eq!(options.timeout, Duration::from_secs(10));
    let got = f.server.initialize(options).await;
    let waited = started.elapsed();
    assert_eq!(got.unwrap_err(), RequestError::Timeout);
    assert!(
        waited >= Duration::from_secs(10) && waited < Duration::from_secs(13),
        "{waited:?}"
    );
    assert!(!common::alive(pid), "the server process is still there");
    assert!(common::group_members(pid).is_empty());
    assert_eq!(f.server.exit_status().unwrap().reason, ExitReason::Stopped);
}

/// ④ `shutdown` 에 답하지 않고 `exit` 도 무시하는 서버가 2 s 안에 그룹째 끝난다(자식까지 — `pgrep -g`).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_ignoring_shutdown_ends_with_its_children_within_2_s() {
    let p = project();
    let scenario = Scenario {
        start: vec![Action::SpawnChild { seconds: 60 }],
        ignore_exit: true,
        ..Scenario::default()
    }
    .reply("shutdown", Reply::silent());
    let f = p.fake("stubborn", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    let pid = f.server.pid().unwrap();
    let child = received(&f.record)
        .iter()
        .find_map(|m| m.get("childPid").and_then(Value::as_u64))
        .unwrap();
    let child = u32::try_from(child).unwrap();
    let members = common::group_members(pid);
    assert!(
        members.contains(&pid) && members.contains(&child),
        "{members:?}"
    );

    let stopped = f.server.shutdown().await;
    assert!(!stopped.answered_shutdown);
    assert!(stopped.killed);
    assert!(stopped.elapsed < STOP_BUDGET, "{:?}", stopped.elapsed);
    assert!(stopped.elapsed >= SHUTDOWN_TIMEOUT + EXIT_GRACE);
    // SIGKILL 뒤 자식을 init 이 거두는 틈만 기다린다.
    assert!(
        eventually(Duration::from_millis(500), || common::group_members(pid)
            .is_empty())
        .await,
        "left in the group: {:?}",
        common::group_members(pid)
    );
    assert!(!common::alive(pid));
    assert!(!common::alive(child));
    // 끝내는 데 쓰인 메시지 순서: shutdown → exit.
    let methods: Vec<String> = received(&f.record)
        .iter()
        .filter_map(|m| method_of(m).map(str::to_owned))
        .collect();
    let at = |name: &str| methods.iter().position(|m| m == name).unwrap();
    assert!(at("shutdown") < at("exit"), "{methods:?}");
}

/// 서버가 스스로 죽으면 대기 중인 요청이 곧바로 실패하고 이벤트가 나간다. 그룹에 남은 자식도 끝난다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dying_server_fails_pending_requests_and_takes_its_children() {
    let p = project();
    let scenario = Scenario::default().reply("will/die", Reply::silent()).on(
        "will/die",
        vec![
            Action::SpawnChild { seconds: 60 },
            Action::Sleep { ms: 50 },
            Action::Exit { code: 3 },
        ],
    );
    let mut f = p.fake("dies", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    let pid = f.server.pid().unwrap();
    let got = f
        .server
        .request_raw("will/die", Value::Null, RequestOptions::default())
        .await;
    assert!(matches!(got, Err(RequestError::Closed(_))), "{got:?}");
    let exit = next_event(&mut f.events, SOON, |e| match e {
        ServerEvent::Exited(exit) => Some(exit),
        _ => None,
    })
    .await
    .unwrap();
    assert_eq!(exit.code, Some(3));
    assert_eq!(exit.reason, ExitReason::Exited);
    assert!(eventually(SOON, || common::group_members(pid).is_empty()).await);
    let next = f
        .server
        .request_raw("x", Value::Null, RequestOptions::default())
        .await;
    assert!(matches!(next, Err(RequestError::Closed(_))));
}

/// ① 본문 상한 + 1 을 알린 틀은 그 서버를 죽이고 대기 중인 요청을 오류로 끝낸다. 다른 서버(Host)는 계속 응답한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_over_the_body_limit_kills_that_server_only() {
    let p = project();
    let over = pw_lsp::transport::MAX_BODY_BYTES + 1;
    let scenario = Scenario::default().reply("big/one", Reply::silent()).on(
        "big/one",
        vec![Action::Raw {
            text: format!("Content-Length: {over}\r\n\r\n"),
        }],
    );
    let mut f = p.fake("oversize", scenario, false).await;
    let healthy = p.fake("healthy", Scenario::default(), false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    healthy
        .server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    let started = Instant::now();
    let got = f
        .server
        .request_raw("big/one", Value::Null, RequestOptions::default())
        .await;
    assert!(
        matches!(&got, Err(RequestError::Closed(why)) if why.contains("larger than")),
        "{got:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    let exit = next_event(&mut f.events, SOON, |e| match e {
        ServerEvent::Exited(exit) => Some(exit),
        _ => None,
    })
    .await
    .unwrap();
    assert!(matches!(exit.reason, ExitReason::Killed(ref why) if why.contains("larger than")));
    // 다른 서버는 그대로 답한다.
    let answer = healthy
        .server
        .request_raw("shutdown", Value::Null, RequestOptions::default())
        .await;
    assert_eq!(answer, Ok(Value::Null));
    healthy.server.shutdown().await;
}

/// ⑤ 동적 등록 · 해제 뒤 능력 조회가 바뀐다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_registration_changes_what_the_server_supports() {
    let p = project();
    let register = json!({ "registrations": [
        { "id": "fmt-1", "method": "textDocument/formatting", "registerOptions": { "documentSelector": null } },
        { "id": "watch-1", "method": "workspace/didChangeWatchedFiles",
          "registerOptions": { "watchers": [{ "globPattern": "**/*.rs" }] } }
    ]});
    let scenario = Scenario::default()
        .on(
            "initialized",
            vec![Action::Request {
                method: "client/registerCapability".into(),
                params: register,
            }],
        )
        .on(
            "fake/unregister",
            vec![Action::Request {
                method: "client/unregisterCapability".into(),
                params: json!({ "unregisterations": [
                    { "id": "fmt-1", "method": "textDocument/formatting" }
                ]}),
            }],
        );
    let mut f = p.fake("dynamic", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    assert!(!f.server.supports("textDocument/formatting"));
    next_event(&mut f.events, SOON, |e| {
        (e == ServerEvent::CapabilitiesChanged).then_some(())
    })
    .await
    .unwrap();
    assert!(f.server.supports("textDocument/formatting"));
    let watchers = f.server.registrations("workspace/didChangeWatchedFiles");
    assert_eq!(watchers.len(), 1);
    assert_eq!(
        watchers[0].register_options.as_ref().unwrap()["watchers"][0]["globPattern"],
        json!("**/*.rs")
    );
    f.server.notify_raw("fake/unregister", json!({})).unwrap();
    next_event(&mut f.events, SOON, |e| {
        (e == ServerEvent::CapabilitiesChanged).then_some(())
    })
    .await
    .unwrap();
    assert!(!f.server.supports("textDocument/formatting"));
    assert_eq!(
        f.server
            .registrations("workspace/didChangeWatchedFiles")
            .len(),
        1
    );
    // 서버는 두 요청 모두에 `null` 답을 받았다.
    assert!(
        eventually(SOON, || {
            received(&f.record)
                .iter()
                .filter(|m| {
                    m.get("id")
                        .is_some_and(|id| id.as_str().is_some_and(|s| s.starts_with("fake-")))
                })
                .filter(|m| m.get("result") == Some(&Value::Null))
                .count()
                == 2
        })
        .await
    );
    f.server.shutdown().await;
}

/// ④(서버 → 클라이언트 요청) 이 행이 답하는 것, 기능 행이 단 처리기, 모르는 요청의 `MethodNotFound`. 로그 · 알림은 로그로.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_requests_are_answered_and_messages_are_routed() {
    let p = project();
    let request = |method: &str, params: Value| Action::Request {
        method: method.into(),
        params,
    };
    let scenario = Scenario::default().on(
        "initialized",
        vec![
            request("window/workDoneProgress/create", json!({ "token": "t1" })),
            request(
                "window/showMessageRequest",
                json!({ "type": 2, "message": "pick one", "actions": [{ "title": "A" }] }),
            ),
            request(
                "workspace/configuration",
                json!({ "items": [{ "section": "x" }] }),
            ),
            request("custom/handled", json!({ "n": 2 })),
            request("unknown/thing", json!({})),
            Action::Notify {
                method: "window/logMessage".into(),
                params: json!({ "type": 3, "message": "logged line" }),
            },
            Action::Notify {
                method: "$/progress".into(),
                params: json!({ "token": "t1", "value": { "kind": "begin", "title": "Indexing" } }),
            },
        ],
    );
    let p2 = &p;
    let record = p2.base.join("routing.jsonl");
    let mut scenario = scenario;
    scenario.record = Some(record.clone());
    let path = scenario.write(&p.base, "routing").unwrap();
    let log_dir = pw_lsp::log::lsp_log_dir(&p.state);
    let log = pw_lsp::log::ServerLog::open(&log_dir, "fake", "routing", false).unwrap();
    let (tx, mut events) = tokio::sync::mpsc::channel(1024);
    let mut handlers = pw_lsp::Handlers::new();
    handlers.on("custom/handled", |params: Value| async move {
        Ok(json!({ "doubled": params["n"].as_i64().unwrap_or(0) * 2 }))
    });
    let server = pw_lsp::LanguageServer::spawn(
        &p.grant,
        common::fake_spec(&p.root, &path),
        pw_lsp::ServerOptions {
            instance: "routing".into(),
            log: Some(log),
            handlers,
            events: Some(tx),
            ledger: None,
        },
    )
    .await
    .unwrap();
    server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    // 서버 요청은 따로 도는 태스크가 답하므로 알림과 순서가 섞일 수 있다 — 둘 다 올 때까지 모은다.
    let (mut alert, mut progress) = (None, None);
    next_event(&mut events, SOON, |e| {
        match e {
            ServerEvent::ShowMessageRequest { kind, message } => alert = Some((kind, message)),
            ServerEvent::Notification { method, params } if method == "$/progress" => {
                progress = Some(params);
            }
            _ => {}
        }
        (alert.is_some() && progress.is_some()).then_some(())
    })
    .await
    .unwrap();
    assert_eq!(alert, Some((2, "pick one".to_owned())));
    assert_eq!(progress.unwrap()["token"], json!("t1"));
    let answers = || {
        received(&record)
            .into_iter()
            .filter(|m| m.get("method").is_none() && m.get("id").is_some())
            .collect::<Vec<_>>()
    };
    assert!(
        eventually(SOON, || answers().len() == 5).await,
        "{:?}",
        answers()
    );
    let by_id = |id: &str| {
        answers()
            .into_iter()
            .find(|m| m["id"] == json!(id))
            .unwrap()
    };
    assert_eq!(by_id("fake-1")["result"], Value::Null);
    assert_eq!(by_id("fake-2")["result"], Value::Null);
    // 처리기가 달리기 전의 설정 요청은 항목마다 `null`(리뷰 L2), 모르는 요청은 `MethodNotFound`.
    assert_eq!(by_id("fake-3")["result"], json!([null]));
    assert_eq!(
        by_id("fake-5")["error"]["code"],
        json!(code::METHOD_NOT_FOUND)
    );
    assert_eq!(by_id("fake-4")["result"], json!({ "doubled": 4 }));
    server.shutdown().await;
    let log = std::fs::read_dir(&log_dir)
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(log.contains("window/logMessage [3] logged line"), "{log}");
}

/// ⑦ 서버의 stderr 가 `$STATE/logs/lsp/<서버 id>-<인스턴스>.<날짜>.log` 로 간다. 프로토콜 기록은 켰을 때만.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stderr_goes_to_the_server_log_file() {
    let p = project();
    let scenario = Scenario {
        start: vec![Action::Stderr {
            text: "hello from the fake server".into(),
        }],
        ..Scenario::default()
    };
    let f = p.fake("stderr-1", scenario.clone(), false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    f.server.shutdown().await;
    let traced = p.fake("stderr-2", scenario, true).await;
    traced
        .server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    traced.server.shutdown().await;
    let dir = &f.log_dir;
    assert_eq!(*dir, p.state.join("logs").join("lsp"));
    let read = |instance: &str| {
        let file = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|path| {
                let name = path.file_name().unwrap().to_str().unwrap();
                name.starts_with(&format!("fake-{instance}.")) && name.ends_with(".log")
            })
            .unwrap_or_else(|| panic!("no log file for {instance} in {}", dir.display()));
        std::fs::read_to_string(file).unwrap()
    };
    let plain = read("stderr-1");
    assert!(
        plain.contains(" stderr hello from the fake server"),
        "{plain}"
    );
    assert!(!plain.contains("\"initialize\""), "{plain}");
    let traced = read("stderr-2");
    assert!(traced.contains(" stderr hello from the fake server"));
    assert!(traced.contains("--> {\"id\":1"), "{traced}");
    assert!(traced.contains("<-- {"), "{traced}");
}

/// 손잡이를 버리면 서버를 트리째 끝낸다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_handle_kills_the_tree() {
    let p = project();
    let scenario = Scenario {
        start: vec![Action::SpawnChild { seconds: 60 }],
        ignore_exit: true,
        ..Scenario::default()
    };
    let f = p.fake("dropped", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    let pid = f.server.pid().unwrap();
    assert!(eventually(SOON, || common::group_members(pid).len() == 2).await);
    drop(f);
    assert!(eventually(SOON, || common::group_members(pid).is_empty()).await);
}
