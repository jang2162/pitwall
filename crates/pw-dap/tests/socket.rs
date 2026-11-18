//! 소켓 경로 어댑터와 어댑터 프로세스가 끝났을 때의 세션(17-m6-plan.md M6-15 ④ — 리뷰 F1 의 메인 판정).

// 프로세스 그룹(`pgrep -g`) · unix 소켓 · 폴더 권한으로 본다 — unix 만.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use common::*;
use pw_dap::fake::{Action, Reply, Scenario};
use pw_dap::{LaunchError, RequestError, SessionEventKind, StartError, Transport, protocol};
use serde_json::json;

/// 가짜 어댑터가 받은 `--socket` 의 경로.
fn socket_of(p: &Project) -> PathBuf {
    let argv: Vec<String> = serde_json::from_value(p.records()[0]["argv"].clone()).unwrap();
    let at = argv.iter().position(|a| a == "--socket").unwrap();
    PathBuf::from(&argv[at + 1])
}

/// 소켓 어댑터는 이 사용자만 들어가는 `0700` 폴더 안의 소켓으로 말하고, `startDebugging` 의 자식은 같은 소켓에 연결을 하나 더
/// 연다(프로세스는 하나). 끝나면 폴더를 지운다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_adapter_talks_through_a_private_folder() {
    let p = project();
    let scenario = Scenario::default().on(
        "launch",
        vec![Action::Request {
            command: "startDebugging".into(),
            arguments: json!({ "request": "launch", "configuration": { "__child": true } }),
            unless_arg: Some("__child".into()),
        }],
    );
    let mut s = start(&p, scenario, socket(), launch(json!({}))).await;
    let path = socket_of(&p);
    let folder = path.parent().unwrap().to_path_buf();
    assert_eq!(
        std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
        0o700,
        "{}",
        folder.display()
    );
    let root = s.debug.root().clone();
    let threads = root.request::<protocol::Threads>(&protocol::Empty {}).await;
    assert!(threads.is_ok(), "{threads:?}");
    let child = next_event(&mut s.events, SOON, |e| match e.kind {
        SessionEventKind::ChildStarted { child } => Some(child),
        _ => None,
    })
    .await
    .expect("no child session");
    assert_eq!(s.debug.session(child).unwrap().pid(), root.pid());
    assert!(
        eventually(SOON, || {
            p.records()
                .iter()
                .any(|r| r["conn"] == 1 && r["msg"]["arguments"]["__child"] == true)
        })
        .await
    );
    let pid = root.pid().unwrap();
    s.debug.disconnect(None).await;
    assert!(eventually(SOON, || group_members(pid).is_empty()).await);
    drop(s.debug);
    assert!(
        eventually(SOON, || !folder.exists()).await,
        "{}",
        folder.display()
    );
}

/// 경합: 어댑터가 아닌 프로세스(다른 프로세스 그룹)가 그 소켓을 먼저 받으면 붙지 않는다 — 시작이 실패하고 어댑터를 끝낸다. 그
/// 프로세스에는 아무 요청도 가지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_served_by_another_process_is_not_used() {
    let p = project();
    let scenario = Scenario {
        listen_elsewhere: true,
        ..Scenario::default()
    };
    let (started, _events, _t) =
        try_start(p.grant(), p.adapter(scenario, socket()), launch(json!({}))).await;
    let listener = p
        .records()
        .iter()
        .find_map(|r| r["listener"].as_u64())
        .expect("no impostor listener");
    match started {
        Err(StartError::Launch(LaunchError::Impostor(why))) => {
            assert!(why.contains("not the debug adapter"), "{why}");
        }
        other => panic!("{other:?}"),
    }
    let adapter = p.records()[0]["pid"].as_u64().unwrap() as u32;
    assert!(eventually(SOON, || group_members(adapter).is_empty()).await);
    assert!(p.received().is_empty(), "{:?}", p.received());
    kill(listener);
}

/// 리뷰 F1 (b): 어댑터 프로세스가 끝나면, 그 연결이 아직 열려 있어도(소켓을 물려받은 다른 프로세스가 쥐고 있다) 세션이 끝난다 —
/// 포트 · 소켓 둘 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_ends_when_its_adapter_exits_even_if_the_connection_stays() {
    for (name, transport) in [("tcp", tcp()), ("socket", socket())] {
        let p = project();
        let scenario = Scenario::default().reply("threads", Reply::silent()).on(
            "threads",
            vec![Action::Hold { seconds: 30 }, Action::Exit { code: 3 }],
        );
        let mut s = start(&p, scenario, transport, launch(json!({}))).await;
        let root = s.debug.root().clone();
        let got = tokio::time::timeout(
            Duration::from_secs(3),
            root.request::<protocol::Threads>(&protocol::Empty {}),
        )
        .await;
        let holder = p
            .records()
            .iter()
            .find_map(|r| r["holder"].as_u64())
            .expect("no holder");
        assert!(
            matches!(got, Ok(Err(RequestError::Closed(_)))),
            "{name}: {got:?}"
        );
        let ended = next_event(&mut s.events, Duration::from_secs(3), |e| match e.kind {
            SessionEventKind::Ended(ended) => Some(ended),
            _ => None,
        })
        .await;
        kill(holder);
        let ended = ended.unwrap_or_else(|| panic!("{name}: the session did not end"));
        assert!(
            matches!(&ended.reason, pw_dap::EndReason::Gone(why) if why.contains("exited")),
            "{name}: {ended:?}"
        );
        assert_eq!(ended.exit_code, Some(3), "{name}");
    }
}

/// 소켓 인자 꼴에 `{socket}` 이 없으면 띄우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socket_arguments_without_the_placeholder_start_nothing() {
    let p = project();
    let transport = Transport::Socket {
        socket_args: vec!["--socket".into()],
    };
    let (started, _, _) = try_start(
        p.grant(),
        p.adapter(Scenario::default(), transport),
        launch(json!({})),
    )
    .await;
    assert!(
        matches!(started, Err(StartError::Launch(LaunchError::SocketMissing))),
        "{started:?}"
    );
    assert!(p.records().is_empty());
}
