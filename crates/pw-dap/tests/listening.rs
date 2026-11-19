//! 디버그 대상이 띄운 어댑터에 붙는 길(`Transport::Listening` — 17-m6-plan.md M6-19b, 메인 판정): 붙는 상대를 믿지 않는다.
//! loopback 이 아닌 주소에는 붙지 않고, 그 세션의 `runInTerminal` 은 거절하며, `startDebugging` 의 자식은 같은 주소에만 붙는다.
//! 상대가 다른 사용자의 프로세스인 경우는 시험 기기에 다른 사용자가 없어 여기서 보지 않는다(`pw-dap` `peer` 의 단위 시험이 우리
//! 연결을 찾는 것만 본다).

// 가짜 어댑터를 따로 띄워 둔다(대상이 띄운 어댑터의 흉내) — unix 만(다른 통합 시험과 같다).
#![cfg(unix)]

mod common;

use std::net::{SocketAddr, TcpListener};
use std::process::{Child, Command, Stdio};

use common::*;
use pw_dap::fake::{Action, Scenario};
use pw_dap::protocol::StartMode;
use pw_dap::{LaunchError, StartError, StartRequest, Transport};
use serde_json::{Map, Value, json};

/// 디버그 대상이 이미 띄운 어댑터(가짜)가 127.0.0.1 의 빈 포트에서 듣는다. 버리면 끝낸다.
struct TargetAdapter {
    child: Child,
    addr: SocketAddr,
}

impl Drop for TargetAdapter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn target_adapter(p: &Project, scenario: Scenario) -> TargetAdapter {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let spec = p.adapter(scenario, Transport::Stdio);
    let child = Command::new(&spec.program)
        .args(&spec.args)
        .args(["--port", &port.to_string(), "--host", "127.0.0.1"])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let until = std::time::Instant::now() + SOON;
    while std::net::TcpStream::connect(addr).is_err() {
        assert!(std::time::Instant::now() < until, "the fake never listened");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    TargetAdapter { child, addr }
}

fn attach(arguments: Value) -> StartRequest {
    StartRequest {
        mode: StartMode::Attach,
        arguments: match arguments {
            Value::Object(map) => map,
            _ => Map::new(),
        },
        breakpoints: Default::default(),
    }
}

/// 상대가 `attach` 를 받으면 `runInTerminal` 을 건다 — 거절되고 실행 관리자에는 아무것도 가지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_target_started_adapter_cannot_run_in_terminal() {
    let p = project();
    let target = target_adapter(
        &p,
        Scenario::default().on(
            "attach",
            vec![Action::Request {
                command: "runInTerminal".into(),
                arguments: json!({
                    "kind": "integrated",
                    "cwd": path_str(&p.root),
                    "args": ["sh", "-c", "echo pwned"],
                }),
                unless_arg: None,
            }],
        ),
    );
    let mut adapter = p.adapter(Scenario::default(), Transport::Stdio);
    adapter.transport = Transport::Listening { addr: target.addr };
    let (started, _events, terminals) =
        try_start(p.grant(), adapter, attach(json!({ "justMyCode": true }))).await;
    let (debug, _) = started.unwrap_or_else(|e| panic!("start failed: {e}"));
    assert!(
        eventually(SOON, || !p.responses_to("runInTerminal").is_empty()).await,
        "no response to runInTerminal"
    );
    let response = p.responses_to("runInTerminal").remove(0);
    assert_eq!(response["success"], false, "{response}");
    assert!(
        response["message"]
            .as_str()
            .is_some_and(|m| m.contains("started by the debug target")),
        "{response}"
    );
    assert!(terminals.runs.lock().unwrap().is_empty());
    assert_eq!(debug.root().pid(), None, "Pitwall started no adapter");
    debug.disconnect(None).await;
}

/// 자식 세션(`startDebugging`)은 같은 상대 주소에 연결을 하나 더 연다 — 어댑터를 띄우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_session_connects_to_the_same_address() {
    let p = project();
    let target = target_adapter(
        &p,
        Scenario {
            capabilities: json!({ "supportsConfigurationDoneRequest": true }),
            ..Scenario::default()
        }
        .on(
            "attach",
            vec![Action::Request {
                command: "startDebugging".into(),
                arguments: json!({ "request": "attach", "configuration": { "__child": true } }),
                unless_arg: Some("__child".into()),
            }],
        ),
    );
    let mut adapter = p.adapter(Scenario::default(), Transport::Stdio);
    adapter.transport = Transport::Listening { addr: target.addr };
    let (started, _events, _terminals) = try_start(p.grant(), adapter, attach(json!({}))).await;
    let (debug, _) = started.unwrap_or_else(|e| panic!("start failed: {e}"));
    assert!(
        eventually(SOON, || p
            .responses_to("startDebugging")
            .first()
            .is_some_and(|r| r["success"] == true))
        .await,
        "{:?}",
        p.responses_to("startDebugging")
    );
    // 연결 번호는 가짜 어댑터가 받은 차례다(준비 확인의 빈 연결이 0 번) — 자식은 뿌리와 다른 연결로 같은 상대에 붙었다.
    let attaches: Vec<(bool, Value)> = p
        .records()
        .into_iter()
        .filter(|r| r["msg"]["command"] == "attach")
        .map(|r| (r["msg"]["arguments"]["__child"] == true, r["conn"].clone()))
        .collect();
    assert_eq!(attaches.len(), 2, "{attaches:?}");
    assert_ne!(attaches[0].1, attaches[1].1, "{attaches:?}");
    assert!(attaches.iter().any(|(child, _)| *child), "{attaches:?}");
    assert_eq!(debug.sessions().len(), 2);
    assert!(debug.sessions().iter().all(|s| s.pid().is_none()));
    debug.disconnect(None).await;
}

/// loopback 이 아닌 주소에는 붙지 않는다(연결을 시도하지도 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_loopback_address_is_refused() {
    let p = project();
    let mut adapter = p.adapter(Scenario::default(), Transport::Stdio);
    // TEST-NET-1(RFC 5737) — 아무도 없는 주소다.
    adapter.transport = Transport::Listening {
        addr: ([192, 0, 2, 1], 5678).into(),
    };
    let (started, _events, _terminals) = try_start(p.grant(), adapter, attach(json!({}))).await;
    match started {
        Err(StartError::Launch(LaunchError::NotLoopback(addr))) => {
            assert_eq!(addr, ([192, 0, 2, 1], 5678).into());
        }
        Err(e) => panic!("expected NotLoopback, got {e}"),
        Ok(_) => panic!("connected to a non-loopback address"),
    }
}

/// 붙은 연결의 반대쪽을 이 사용자의 프로세스에서 찾지 못하면 DAP 를 보내지 않고 거절한다. macOS 는 `accept` 하지 않은 연결이 어느
/// 프로세스의 fd 에도 없으므로 「받지 않는 상대」 로 그 갈래를 본다(다른 사용자의 프로세스 흉내 — 시험 기기에 다른 사용자가 없다).
/// Linux 는 받기 전에도 듣는 소켓의 uid 로 보이므로 이 흉내가 되지 않는다.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_no_process_of_ours_holds_is_refused() {
    let p = project();
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut adapter = p.adapter(Scenario::default(), Transport::Stdio);
    adapter.transport = Transport::Listening {
        addr: silent.local_addr().unwrap(),
    };
    let (started, _events, _terminals) = try_start(p.grant(), adapter, attach(json!({}))).await;
    match started {
        Err(StartError::Launch(LaunchError::Impostor(why))) => {
            assert!(why.contains("no process of this user"), "{why}");
        }
        Err(e) => panic!("expected Impostor, got {e}"),
        Ok(_) => panic!("talked to a peer nobody of ours holds"),
    }
    drop(silent);
}
