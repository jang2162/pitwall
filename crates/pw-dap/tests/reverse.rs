//! 역요청 `runInTerminal` · `startDebugging`(17-m6-plan.md M6-15 완료 기준 ③ · ④ · ⑦).

// 프로세스 그룹(`pgrep -g`)으로 어댑터가 끝났는지 본다 — unix 만(`pw-lsp` 의 통합 시험과 같다).
#![cfg(unix)]

mod common;

use common::*;
use pw_dap::fake::{Action, Scenario};
use pw_dap::{EndReason, SessionEventKind, Transport};
use serde_json::{Value, json};

fn run_in_terminal(arguments: Value) -> Action {
    Action::Request {
        command: "runInTerminal".into(),
        arguments,
        unless_arg: None,
    }
}

/// 가짜 어댑터가 `launch` 를 받으면 `runInTerminal` 을 건다.
fn asks_on_launch(arguments: Value) -> Scenario {
    Scenario::default().on("launch", vec![run_in_terminal(arguments)])
}

async fn response_to_run(p: &Project) -> Value {
    assert!(
        eventually(SOON, || !p.responses_to("runInTerminal").is_empty()).await,
        "no response to runInTerminal"
    );
    p.responses_to("runInTerminal").remove(0)
}

/// ③ `runInTerminal` 은 그 세션의 허가로 실행 관리자에 실행을 만들고, 그 pid 를 어댑터에 돌려준다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_in_terminal_creates_a_run_in_the_run_manager() {
    let p = project();
    let scenario = asks_on_launch(json!({
        "kind": "integrated",
        "title": "app",
        "cwd": path_str(&p.root.join("src")),
        "args": ["node", "app.js", "--flag"],
        "env": { "A": "1", "B": null },
    }));
    let s = start(&p, scenario, Transport::Stdio, launch(json!({}))).await;
    let response = response_to_run(&p).await;
    assert_eq!(response["success"], true, "{response}");
    assert_eq!(response["body"]["processId"], 4242);
    let runs = s.terminals.runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 1);
    let (project, run) = &runs[0];
    assert_eq!(project, &p.id);
    assert_eq!(run.cwd, p.root.join("src"));
    assert_eq!(run.args, ["node", "app.js", "--flag"]);
    assert_eq!(run.title.as_deref(), Some("app"));
    assert_eq!(run.env.get("A"), Some(&Some("1".to_owned())));
    assert_eq!(run.env.get("B"), Some(&None));
    s.debug.disconnect(None).await;
}

/// ⑦ 루트 밖 `cwd` 는 거절하고 아무것도 띄우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_in_terminal_outside_the_root_is_refused() {
    let p = project();
    let scenario = asks_on_launch(json!({ "cwd": path_str(&p.base), "args": ["sh"] }));
    let s = start(&p, scenario, Transport::Stdio, launch(json!({}))).await;
    let response = response_to_run(&p).await;
    assert_eq!(response["success"], false, "{response}");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("outside the project root")
    );
    assert!(s.terminals.runs.lock().unwrap().is_empty());
    s.debug.disconnect(None).await;
}

/// ⑦ `kind = "external"`(OS 터미널 창)은 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_in_terminal_of_an_external_kind_is_refused() {
    let p = project();
    let scenario = asks_on_launch(json!({
        "kind": "external",
        "cwd": path_str(&p.root),
        "args": ["node", "app.js"],
    }));
    let s = start(&p, scenario, Transport::Stdio, launch(json!({}))).await;
    let response = response_to_run(&p).await;
    assert_eq!(response["success"], false, "{response}");
    assert!(response["message"].as_str().unwrap().contains("external"));
    assert!(s.terminals.runs.lock().unwrap().is_empty());
    s.debug.disconnect(None).await;
}

/// ⑦ 세션이 끝난 뒤(끊는 중)의 `runInTerminal` 은 거절한다 — 허가는 세션에 묶인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_in_terminal_after_the_session_ended_is_refused() {
    let p = project();
    // `disconnect` 를 받으면 답하기 전에 `runInTerminal` 을 걸고 그 답을 기다린다.
    let scenario = Scenario::default().on(
        "disconnect",
        vec![
            run_in_terminal(json!({ "cwd": path_str(&p.root), "args": ["node", "app.js"] })),
            Action::Sleep { ms: 300 },
        ],
    );
    let s = start(&p, scenario, Transport::Stdio, launch(json!({}))).await;
    let ended = s.debug.disconnect(None).await;
    assert_eq!(ended.reason, EndReason::Stopped);
    let response = response_to_run(&p).await;
    assert_eq!(response["success"], false, "{response}");
    assert!(response["message"].as_str().unwrap().contains("ended"));
    assert!(s.terminals.runs.lock().unwrap().is_empty());
}

/// 자식 세션을 여는 시나리오: 뿌리가 `launch` 를 받으면 `startDebugging` 을 건다(자식의 `launch` 에는 `__child` 가 있어 다시
/// 걸지 않는다). 세션마다 `configurationDone` 에 `output` 을 낸다.
fn opens_a_child() -> Scenario {
    Scenario {
        capabilities: json!({ "supportsConfigurationDoneRequest": true }),
        ..Scenario::default()
    }
    .on(
        "launch",
        vec![Action::Request {
            command: "startDebugging".into(),
            arguments: json!({
                "request": "launch",
                "configuration": { "__child": true, "name": "worker" },
            }),
            unless_arg: Some("__child".into()),
        }],
    )
    .on(
        "configurationDone",
        vec![Action::Event {
            event: "output".into(),
            body: json!({ "output": "ready\n" }),
        }],
    )
}

async fn child_tree(transport: Transport) -> (Project, Started, pw_dap::SessionId) {
    let p = project();
    let mut s = start(
        &p,
        opens_a_child(),
        transport,
        launch(json!({ "name": "main" })),
    )
    .await;
    let root = s.debug.root().id();
    let child = next_event(&mut s.events, SOON, |e| match e.kind {
        SessionEventKind::ChildStarted { child } if e.session == root => Some(child),
        _ => None,
    })
    .await
    .expect("no child session");
    assert!(
        eventually(SOON, || p
            .responses_to("startDebugging")
            .first()
            .is_some_and(|r| r["success"] == true))
        .await,
        "{:?}",
        p.responses_to("startDebugging")
    );
    (p, s, child)
}

/// ④ `startDebugging`(stdio 어댑터): 같은 정의로 어댑터를 하나 더 띄워 자식 세션을 열고, 받은 구성으로 `launch` 한다. 세션
/// 나무에 들고, 자식의 이벤트는 자식 id 를 단다. 뿌리를 끊으면 자식도 끝난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_debugging_opens_a_child_session_with_its_own_adapter() {
    let (p, mut s, child_id) = child_tree(Transport::Stdio).await;
    let root = s.debug.root().clone();
    let child = s.debug.session(child_id).unwrap();
    assert_eq!(child.parent(), Some(root.id()));
    assert_eq!(s.debug.sessions().len(), 2);
    assert_ne!(child.pid(), root.pid());
    let launches: Vec<Value> = p
        .received()
        .into_iter()
        .filter(|m| m["command"] == "launch")
        .map(|m| m["arguments"].clone())
        .collect();
    assert!(
        launches.contains(&json!({ "__child": true, "name": "worker" })),
        "{launches:?}"
    );
    // 자식의 이벤트는 자식 id 로 온다.
    assert!(
        next_event(&mut s.events, SOON, |e| match e.kind {
            SessionEventKind::Output(o) if e.session == child_id => Some(o),
            _ => None,
        })
        .await
        .is_some_and(|o| o.output == "ready\n")
    );
    let (root_pid, child_pid) = (root.pid().unwrap(), child.pid().unwrap());
    s.debug.disconnect(None).await;
    assert!(child.ended().is_some());
    assert!(
        eventually(SOON, || group_members(root_pid).is_empty()
            && group_members(child_pid).is_empty())
        .await
    );
}

/// ④ `startDebugging`(TCP 어댑터 — js-debug 의 꼴): 같은 포트에 연결을 하나 더 연다(프로세스는 하나).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_debugging_on_a_port_adapter_connects_again() {
    let (p, s, child_id) = child_tree(tcp()).await;
    let root = s.debug.root().clone();
    let child = s.debug.session(child_id).unwrap();
    assert_eq!(child.pid(), root.pid());
    let child_launch = p
        .records()
        .into_iter()
        .find(|r| r["msg"]["command"] == "launch" && r["msg"]["arguments"]["__child"] == true)
        .unwrap();
    assert_eq!(child_launch["conn"], 1);
    let pid = root.pid().unwrap();
    s.debug.disconnect(None).await;
    assert!(child.ended().is_some());
    assert!(eventually(SOON, || group_members(pid).is_empty()).await);
}

/// 리뷰 F4: 끝난 뒤(끊는 중)의 `startDebugging` 은 거절하고 어댑터를 더 띄우지 않는다(⑦ 의 `runInTerminal` 짝).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_debugging_after_the_session_ended_is_refused() {
    let p = project();
    let scenario = Scenario::default().on(
        "disconnect",
        vec![
            Action::Request {
                command: "startDebugging".into(),
                arguments: json!({ "request": "launch", "configuration": { "__child": true } }),
                unless_arg: None,
            },
            Action::Sleep { ms: 300 },
        ],
    );
    let s = start(&p, scenario, Transport::Stdio, launch(json!({}))).await;
    s.debug.disconnect(None).await;
    assert!(
        eventually(SOON, || !p.responses_to("startDebugging").is_empty()).await,
        "no response to startDebugging"
    );
    let response = p.responses_to("startDebugging").remove(0);
    assert_eq!(response["success"], false, "{response}");
    // 띄운 어댑터는 뿌리 하나뿐이다.
    let started = p
        .records()
        .iter()
        .filter(|r| r.get("argv").is_some())
        .count();
    assert_eq!(started, 1);
    assert_eq!(s.debug.sessions().len(), 1);
}

/// 리뷰 F3: 세션 중 신뢰를 거두면 `runInTerminal` 도 `startDebugging` 도 거절한다 — 쥐고 있던 허가로는 띄우지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_after_trust_is_revoked_are_refused() {
    let p = project();
    let scenario = Scenario::default().on(
        "threads",
        vec![
            run_in_terminal(json!({ "cwd": path_str(&p.root), "args": ["node", "app.js"] })),
            Action::Request {
                command: "startDebugging".into(),
                arguments: json!({ "request": "launch", "configuration": { "__child": true } }),
                unless_arg: None,
            },
        ],
    );
    let s = start(&p, scenario, Transport::Stdio, launch(json!({}))).await;
    p.projects.set_trust(&p.id, false).unwrap();
    s.debug
        .root()
        .request::<pw_dap::protocol::Threads>(&pw_dap::protocol::Empty {})
        .await
        .unwrap();
    assert!(
        eventually(SOON, || !p.responses_to("runInTerminal").is_empty()
            && !p.responses_to("startDebugging").is_empty())
        .await
    );
    for command in ["runInTerminal", "startDebugging"] {
        let response = p.responses_to(command).remove(0);
        assert_eq!(response["success"], false, "{command}: {response}");
        assert!(
            response["message"]
                .as_str()
                .unwrap()
                .contains("no longer trusted"),
            "{command}: {response}"
        );
    }
    assert!(s.terminals.runs.lock().unwrap().is_empty());
    let started = p
        .records()
        .iter()
        .filter(|r| r.get("argv").is_some())
        .count();
    assert_eq!(started, 1);
    s.debug.disconnect(None).await;
}
