//! 세션의 시작 순서 · 끝 · 틀 위반 · 잘못된 메시지 · 끊기(17-m6-plan.md M6-15 완료 기준 ① · ② · ⑤ · ⑧).

// 프로세스 그룹(`pgrep -g`)으로 어댑터가 끝났는지 본다 — unix 만(`pw-lsp` 의 통합 시험과 같다).
#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use pw_dap::fake::{Action, Reply, Scenario};
use pw_dap::protocol::{
    self, FunctionBreakpoint, SetBreakpointsArguments, Source, SourceBreakpoint,
};
use pw_dap::{
    EndReason, InitialBreakpoints, LaunchError, RequestError, SessionEventKind, StartError,
    Timeouts, Transport,
};
use serde_json::{Value, json};

fn capabilities() -> Value {
    json!({
        "supportsConfigurationDoneRequest": true,
        "supportsFunctionBreakpoints": true,
        "exceptionBreakpointFilters": [{ "filter": "uncaught", "label": "Uncaught exceptions" }],
    })
}

fn breakpoints(p: &Project) -> InitialBreakpoints {
    InitialBreakpoints {
        sources: vec![SetBreakpointsArguments {
            source: Source {
                path: Some(path_str(&p.root.join("src/main.rs"))),
                ..Source::default()
            },
            breakpoints: vec![
                SourceBreakpoint {
                    line: 3,
                    ..SourceBreakpoint::default()
                },
                SourceBreakpoint {
                    line: 7,
                    ..SourceBreakpoint::default()
                },
            ],
            source_modified: None,
        }],
        functions: vec![FunctionBreakpoint {
            name: "main".into(),
            ..FunctionBreakpoint::default()
        }],
        exception_filters: vec!["uncaught".into(), "never-announced".into()],
    }
}

/// 순서를 어기면 가짜 어댑터가 코드 97 로 끝나는 시나리오: `initialized` 는 `launch` 를 받고 한참 뒤에, `launch` 의 답은
/// `configurationDone` 뒤에(debugpy 의 꼴).
fn strict(capabilities: Value) -> Scenario {
    Scenario {
        capabilities,
        initialized_after: Some("launch".into()),
        require_initialized: vec![
            "setBreakpoints".into(),
            "setFunctionBreakpoints".into(),
            "setExceptionBreakpoints".into(),
            "configurationDone".into(),
        ],
        ..Scenario::default()
    }
    .on("launch", vec![Action::Sleep { ms: 300 }])
    .reply("launch", Reply::default().after(&["configurationDone"]))
}

/// ① `initialized` 이벤트 전에는 브레이크포인트를 보내지 않는다 — 순서를 어기면 가짜 어댑터가 실패해 시작이 실패한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakpoints_wait_for_the_initialized_event() {
    let p = project();
    let mut request = launch(json!({ "program": "app" }));
    request.breakpoints = breakpoints(&p);
    let s = start(&p, strict(capabilities()), Transport::Stdio, request).await;
    assert_eq!(
        p.commands(),
        [
            "initialize",
            "launch",
            "setBreakpoints",
            "setFunctionBreakpoints",
            "setExceptionBreakpoints",
            "configurationDone"
        ]
    );
    assert!(p.records().iter().all(|r| r.get("violation").is_none()));
    // 어댑터가 정한 브레이크포인트가 돌아온다.
    let lines: Vec<_> = s.configured.sources[0]
        .iter()
        .map(|b| (b.verified, b.line))
        .collect();
    assert_eq!(lines, [(true, Some(3)), (true, Some(7))]);
    // 알리지 않은 예외 필터는 보내지 않는다.
    assert_eq!(s.configured.exception_filters, ["uncaught"]);
    let sent = p
        .received()
        .into_iter()
        .find(|m| m["command"] == "setExceptionBreakpoints")
        .unwrap();
    assert_eq!(sent["arguments"]["filters"], json!(["uncaught"]));
    let init = p
        .received()
        .into_iter()
        .find(|m| m["command"] == "initialize")
        .unwrap();
    assert_eq!(init["arguments"]["adapterID"], "fake");
    assert_eq!(init["arguments"]["pathFormat"], "path");
    assert_eq!(init["arguments"]["supportsRunInTerminalRequest"], true);
    assert_eq!(init["arguments"]["supportsStartDebuggingRequest"], true);
    assert_eq!(
        init["arguments"]["supportsArgsCanBeInterpretedByShell"],
        false
    );
    s.debug.disconnect(None).await;
}

/// ① 의 반대편: `initialized` 를 `initialize` 응답 직후에 내는 어댑터(codelldb 의 꼴)도, `configurationDone` 을 알리지 않는
/// 어댑터도 시작한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_early_initialized_event_and_no_configuration_done_also_start() {
    let p = project();
    let mut request = launch(json!({}));
    request.breakpoints = breakpoints(&p);
    let s = start(&p, Scenario::default(), Transport::Stdio, request).await;
    // 함수 브레이크포인트 · 예외 필터 · configurationDone 을 알리지 않았으니 보내지 않는다.
    assert_eq!(p.commands(), ["initialize", "launch", "setBreakpoints"]);
    assert!(s.configured.functions.is_empty());
    s.debug.disconnect(None).await;
}

/// ① `launch` 가 실패로 답하면 시작이 실패하고 어댑터가 끝난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_launch_fails_the_start_and_ends_the_adapter() {
    let p = project();
    let scenario = Scenario::default().reply("launch", Reply::error("cannot find program"));
    let (started, _events, _t) = try_start(
        p.grant(),
        p.adapter(scenario, Transport::Stdio),
        launch(json!({})),
    )
    .await;
    match started {
        Err(StartError::Request("launch", RequestError::Adapter(e))) => {
            assert_eq!(e.message, "cannot find program");
        }
        other => panic!("{other:?}"),
    }
    let pid = p.records()[0]["pid"].as_u64().unwrap() as u32;
    assert!(eventually(SOON, || group_members(pid).is_empty()).await);
}

/// ② 본문 상한을 넘는 `Content-Length` 와 8 KiB 를 넘는 머리는 읽지 않고 그 어댑터를 죽인다 — 언어 서버와 같은 상한
/// (`pw_wire`)과 같은 규칙.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_over_the_limits_kills_the_adapter() {
    let over = pw_wire::MAX_BODY_BYTES + 1;
    for (name, raw) in [
        ("body", format!("Content-Length: {over}\r\n\r\n")),
        (
            "header",
            format!("X-Pad: {}\r\n", "a".repeat(pw_wire::MAX_HEADER_BYTES)),
        ),
    ] {
        let p = project();
        let scenario = Scenario::default()
            .reply("threads", Reply::silent())
            .on("threads", vec![Action::Raw { text: raw }]);
        // 끝내기의 물러남(`exit_grace`)을 길게 둔다 — 그래도 2 s 안에 그룹이 비면 위반 그 자리에서 죽인 것이다(리뷰 R1).
        let timeouts = Timeouts {
            exit_grace: Duration::from_secs(30),
            ..short_timeouts()
        };
        let (started, mut events, _t) = try_start_with(
            p.grant(),
            p.adapter(scenario, Transport::Stdio),
            launch(json!({})),
            timeouts,
        )
        .await;
        let (debug, _) = started.unwrap();
        let root = debug.root().clone();
        let pid = root.pid().unwrap();
        let started = Instant::now();
        let got = root.request::<protocol::Threads>(&protocol::Empty {}).await;
        assert!(
            matches!(&got, Err(RequestError::Closed(why)) if why.contains("larger than")),
            "{name}: {got:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(2), "{name}");
        let ended = next_event(&mut events, SOON, |e| match e.kind {
            SessionEventKind::Ended(ended) => Some(ended),
            _ => None,
        })
        .await
        .unwrap();
        assert!(
            matches!(&ended.reason, EndReason::Killed(why) if why.contains("larger than")),
            "{name}: {ended:?}"
        );
        assert!(
            eventually(Duration::from_secs(2), || group_members(pid).is_empty()).await,
            "{name}: the adapter was not killed on the spot"
        );
        assert!(debug.grant().revoked());
    }
}

/// ⑤ 어댑터가 죽으면 기다리던 요청이 실패하고, 세션이 끝나고 `Ended` 가 한 번 난다. 허가도 거둔다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_the_adapter_dies_the_session_ends_with_an_event() {
    let p = project();
    let scenario = Scenario::default()
        .reply("threads", Reply::silent())
        .on("threads", vec![Action::Exit { code: 3 }]);
    let mut s = start(&p, scenario, Transport::Stdio, launch(json!({}))).await;
    let root = s.debug.root().clone();
    let pid = root.pid().unwrap();
    let got = root.request::<protocol::Threads>(&protocol::Empty {}).await;
    assert!(matches!(got, Err(RequestError::Closed(_))), "{got:?}");
    let ended = next_event(&mut s.events, SOON, |e| match e.kind {
        SessionEventKind::Ended(ended) => Some((e.session, ended)),
        _ => None,
    })
    .await
    .unwrap();
    assert_eq!(ended.0, root.id());
    assert_eq!(ended.1.exit_code, Some(3));
    assert!(matches!(ended.1.reason, EndReason::Gone(_)), "{ended:?}");
    assert_eq!(root.ended(), Some(ended.1));
    assert!(s.debug.grant().revoked());
    assert!(eventually(SOON, || group_members(pid).is_empty()).await);
    // 한 번만.
    assert!(
        next_event(&mut s.events, Duration::from_millis(200), |e| {
            match e.kind {
                SessionEventKind::Ended(_) => Some(()),
                _ => None,
            }
        })
        .await
        .is_none()
    );
    let next = root.request::<protocol::Threads>(&protocol::Empty {}).await;
    assert!(matches!(next, Err(RequestError::Closed(_))));
}

/// 어댑터가 보내는 잘못된 메시지: JSON 이 아닌 본문 · 꼴이 틀린 메시지 · 모르는 이벤트 · 본문이 틀린 아는 이벤트는 그
/// 메시지만 버린다 — 세션은 계속 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_messages_and_unknown_events_are_dropped() {
    let p = project();
    let scenario = Scenario::default()
        .reply(
            "threads",
            Reply::body(json!({ "threads": [{ "id": 1, "name": "main" }] })),
        )
        .on(
            "threads",
            vec![
                Action::Body {
                    text: "not json".into(),
                },
                Action::Body {
                    text: r#"{"seq":1,"type":"event"}"#.into(),
                },
                Action::Event {
                    event: "weirdEvent".into(),
                    body: json!({}),
                },
                Action::Event {
                    event: "stopped".into(),
                    body: json!({ "threadId": 1 }),
                },
                Action::Event {
                    event: "output".into(),
                    body: json!({ "category": "stdout", "output": "after\n" }),
                },
            ],
        );
    let mut s = start(&p, scenario, Transport::Stdio, launch(json!({}))).await;
    let root = s.debug.root().clone();
    let threads = root
        .request::<protocol::Threads>(&protocol::Empty {})
        .await
        .unwrap();
    assert_eq!(threads.threads[0].name, "main");
    let first = next_event(&mut s.events, SOON, Some).await.unwrap();
    assert_eq!(first.session, root.id());
    assert!(
        matches!(&first.kind, SessionEventKind::Output(o) if o.output == "after\n"),
        "{first:?}"
    );
    assert_eq!(root.unreadable_messages(), 2);
    assert!(root.ended().is_none());
    s.debug.disconnect(None).await;
}

/// 끊기: `disconnect` 를 보내고 어댑터가 끝나며 `Ended { Stopped }`. 허가를 거둔다 — 같은 허가로 다시 시작하지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_ends_the_session_and_revokes_the_grant() {
    let p = project();
    let grant = p.grant();
    let (started, mut events, _t) = try_start(
        grant.clone(),
        p.adapter(Scenario::default(), Transport::Stdio),
        launch(json!({})),
    )
    .await;
    let (debug, _) = started.unwrap();
    let pid = debug.root().pid().unwrap();
    let ended = debug.disconnect(Some(true)).await;
    assert_eq!(ended.reason, EndReason::Stopped);
    let sent = p
        .received()
        .into_iter()
        .find(|m| m["command"] == "disconnect")
        .unwrap();
    assert_eq!(sent["arguments"]["terminateDebuggee"], true);
    assert!(
        next_event(&mut events, SOON, |e| match e.kind {
            SessionEventKind::Ended(ended) => Some(ended),
            _ => None,
        })
        .await
        .is_some_and(|e| e.reason == EndReason::Stopped)
    );
    assert!(eventually(SOON, || group_members(pid).is_empty()).await);
    assert!(grant.revoked());
    let (again, _, _) = try_start(
        grant,
        p.adapter(Scenario::default(), Transport::Stdio),
        launch(json!({})),
    )
    .await;
    assert!(matches!(again, Err(StartError::Refused(_))), "{again:?}");
}

/// 손잡이를 끊지 않고 버려도 어댑터가 트리째 끝난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_session_ends_the_adapter() {
    let p = project();
    let s = start(&p, Scenario::default(), Transport::Stdio, launch(json!({}))).await;
    let pid = s.debug.root().pid().unwrap();
    assert!(!group_members(pid).is_empty());
    drop(s.debug);
    assert!(eventually(SOON, || group_members(pid).is_empty()).await);
}

/// 어댑터의 작업 디렉터리가 허가의 루트 밖이면 띄우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adapter_outside_the_root_is_not_started() {
    let p = project();
    let mut adapter = p.adapter(Scenario::default(), Transport::Stdio);
    adapter.cwd = p.base.clone();
    let (started, _, _) = try_start(p.grant(), adapter, launch(json!({}))).await;
    assert!(
        matches!(started, Err(StartError::Refused(_))),
        "{started:?}"
    );
    assert!(p.records().is_empty());
}

/// ⑧ 포트 어댑터는 `127.0.0.1` 에 묶는 인자로 띄우고 그 주소로 붙는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_port_adapter_is_bound_to_the_loopback_address() {
    let p = project();
    let s = start(&p, Scenario::default(), tcp(), launch(json!({}))).await;
    let argv: Vec<String> = serde_json::from_value(p.records()[0]["argv"].clone()).unwrap();
    let host = argv.iter().position(|a| a == "--host").unwrap();
    assert_eq!(argv[host + 1], "127.0.0.1");
    let port = argv.iter().position(|a| a == "--port").unwrap();
    assert!(argv[port + 1].parse::<u16>().unwrap() > 0);
    assert_eq!(p.commands(), ["initialize", "launch"]);
    let threads = s
        .debug
        .root()
        .request::<protocol::Threads>(&protocol::Empty {})
        .await;
    assert!(threads.is_ok(), "{threads:?}");
    let pid = s.debug.root().pid().unwrap();
    s.debug.disconnect(None).await;
    assert!(eventually(SOON, || group_members(pid).is_empty()).await);
}

/// ⑧ `{host}` 자리가 없는 포트 인자 꼴은 띄우지 않는다(어댑터가 어느 주소에 묶는지 정하지 못한다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn port_arguments_without_a_host_start_nothing() {
    let p = project();
    let transport = Transport::Tcp {
        port_args: vec!["--port".into(), "{port}".into()],
    };
    let (started, _, _) = try_start(
        p.grant(),
        p.adapter(Scenario::default(), transport),
        launch(json!({})),
    )
    .await;
    assert!(
        matches!(started, Err(StartError::Launch(LaunchError::HostNotBound))),
        "{started:?}"
    );
    assert!(p.records().is_empty());
}
