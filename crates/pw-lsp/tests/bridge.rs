//! 웹뷰 LSP 다리의 Host 쪽(15-m4-plan.md M4-07): `lsp.document.capabilities` 를 가짜 서버로 끝까지. 기동 중에는 빈 칸이다가 준비되면
//! 채워지고, 동적 등록 · 해제가 칸과 상태 이벤트에 반영되며, 고르는 규칙이 요청 길과 같다(결정 M4-D12).

#![cfg(unix)]

mod common;

use std::time::Duration;

use pw_core::Filter;
use pw_core::bus::Received;
use pw_lsp::fake::{Action, Reply, Scenario};
use pw_lsp::methods::{LspInstanceState, TOPIC_STATUS};
use pw_rpc::code;
use serde_json::{Value, json};

use common::env::{Env, EnvOptions};

fn doc_id(opened: &Value) -> String {
    opened["docId"].as_str().unwrap().to_owned()
}

async fn capabilities(win: &mut common::env::Conn, doc: &str) -> Value {
    win.call("lsp.document.capabilities", json!({ "docId": doc }))
        .await
        .unwrap()["features"]
        .clone()
}

/// 기동 중(`initialize` 를 기다림)에는 칸이 비어 있고, 준비되면 정적 능력이 칸에 든다. 옵션은 서버가 보낸 것 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_features_are_empty_while_starting_and_filled_when_ready() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        Scenario::default().reply(
            "initialize",
            Reply::result(json!({ "capabilities": {
                "textDocumentSync": 2,
                "hoverProvider": true,
                "completionProvider": { "triggerCharacters": [".", ":"] },
            }}))
            .delayed(400),
        ),
    );
    let mut win = env.window();
    let doc = doc_id(
        &win.open(&env, &env.file("src/main.rs", ""), "Rust", "fn main() {}")
            .await,
    );
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::Starting)
    );
    assert_eq!(capabilities(&mut win, &doc).await, json!({}));
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert_eq!(
        capabilities(&mut win, &doc).await,
        json!({
            "hover": { "server": "rust-analyzer", "options": true, "optionsTruncated": false },
            "completion": {
                "server": "rust-analyzer",
                "options": { "triggerCharacters": [".", ":"] },
                "optionsTruncated": false,
            },
        })
    );
}

/// 칸이 `want` 를 만족할 때까지(5 s) 다시 묻는다.
async fn until(win: &mut common::env::Conn, doc: &str, want: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let got = capabilities(win, doc).await;
        if want(&got) || tokio::time::Instant::now() > deadline {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 다음 `lsp.status` 이벤트(5 s).
async fn next_status(events: &mut pw_core::bus::Subscription) {
    let Ok(Some(Received::Event(event))) =
        tokio::time::timeout(Duration::from_secs(5), events.recv()).await
    else {
        panic!("no lsp.status");
    };
    assert_eq!(event.topic, TOPIC_STATUS);
}

/// 동적 등록이 칸에 들고(옵션은 등록 옵션), 해제하면 빠진다. 둘 다 `lsp.status` 이벤트를 낸다 — 웹뷰가 그 이벤트로 다시 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_registration_reaches_the_features_and_the_status_event() {
    let env = Env::new(EnvOptions::default());
    let register = Action::Request {
        method: "client/registerCapability".into(),
        params: json!({ "registrations": [
            { "id": "sig-1", "method": "textDocument/signatureHelp",
              "registerOptions": { "triggerCharacters": ["("] } }
        ]}),
    };
    let unregister = Action::Request {
        method: "client/unregisterCapability".into(),
        params: json!({ "unregisterations": [
            { "id": "sig-1", "method": "textDocument/signatureHelp" }
        ]}),
    };
    // 등록은 hover 답 뒤에, 해제는 편집(`didChange`)을 받으면 건다 — 시험이 차례를 고른다.
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "hoverProvider": true }),
            ..Scenario::default()
        }
        .reply(
            "textDocument/hover",
            Reply {
                result: Some(json!(null)),
                then: vec![register],
                ..Reply::default()
            },
        )
        .on("textDocument/didChange", vec![unregister]),
    );
    let mut win = env.window();
    let doc = doc_id(
        &win.open(&env, &env.file("src/main.rs", ""), "Rust", "fn main() {}")
            .await,
    );
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert!(
        capabilities(&mut win, &doc)
            .await
            .get("signature_help")
            .is_none()
    );

    let mut events = env.bus.subscribe(Filter::all().topics([TOPIC_STATUS]));
    win.call(
        "test.lsp.feature",
        json!({ "docId": doc, "feature": "hover", "method": "textDocument/hover", "offset": 0 }),
    )
    .await
    .unwrap();
    next_status(&mut events).await;
    let got = until(&mut win, &doc, |f| f.get("signature_help").is_some()).await;
    assert_eq!(
        got["signature_help"],
        json!({ "server": "rust-analyzer", "options": { "triggerCharacters": ["("] }, "optionsTruncated": false })
    );

    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": " " }] }),
    )
    .await
    .unwrap();
    next_status(&mut events).await;
    let got = until(&mut win, &doc, |f| f.get("signature_help").is_none()).await;
    assert!(got.get("signature_help").is_none(), "{got}");
    assert_eq!(got["hover"]["server"], "rust-analyzer");
}

/// 결정 M4-D12: 칸의 서버는 요청 길이 고르는 서버와 같다 — basedpyright 는 정의에서 포맷을 끈다, 포맷 칸은 ruff 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_features_follow_the_request_routing() {
    let env = Env::new(EnvOptions::default());
    let both =
        json!({ "textDocumentSync": 2, "hoverProvider": true, "documentFormattingProvider": true });
    for server in ["basedpyright", "ruff"] {
        env.fake(
            server,
            Scenario {
                capabilities: both.clone(),
                ..Scenario::default()
            },
        );
    }
    let mut win = env.window();
    let doc = doc_id(
        &win.open(&env, &env.file("app.py", ""), "Python", "x = 1\n")
            .await,
    );
    env.wait_state("basedpyright", LspInstanceState::Ready)
        .await;
    env.wait_state("ruff", LspInstanceState::Ready).await;
    let features = capabilities(&mut win, &doc).await;
    assert_eq!(features["hover"]["server"], "basedpyright");
    assert_eq!(features["formatting"]["server"], "ruff");
    assert!(
        features.get("diagnostics").is_none(),
        "diagnostics are gathered from every server"
    );
}

/// 다른 연결(창)의 문서 · 없는 문서는 not found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_windows_document_is_not_found() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "hoverProvider": true }),
            ..Scenario::default()
        },
    );
    let mut win = env.window();
    let doc = doc_id(
        &win.open(&env, &env.file("src/main.rs", ""), "Rust", "")
            .await,
    );
    let mut other = env.window();
    for id in [doc.as_str(), "nope"] {
        let e = other
            .call("lsp.document.capabilities", json!({ "docId": id }))
            .await
            .unwrap_err();
        assert_eq!(e.code, code::NOT_FOUND, "{id}");
    }
}

/// 동적 등록은 그 문서에 맞는 것(`documentSelector`)의 옵션을 쓴다 — 한 서버가 언어마다 다른 트리거를 등록해도(리뷰 L-5). 옵션 없는
/// 등록은 `true` 다(리뷰 L-4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_options_follow_the_document_selector() {
    let env = Env::new(EnvOptions::default());
    let register = Action::Request {
        method: "client/registerCapability".into(),
        params: json!({ "registrations": [
            { "id": "py", "method": "textDocument/completion",
              "registerOptions": { "documentSelector": [{ "language": "python" }], "triggerCharacters": ["#"] } },
            { "id": "rs", "method": "textDocument/completion",
              "registerOptions": { "documentSelector": [{ "language": "rust", "pattern": "**/*.rs" }], "triggerCharacters": ["."] } },
            { "id": "sig", "method": "textDocument/signatureHelp" }
        ]}),
    };
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2 }),
            ..Scenario::default()
        }
        .on("initialized", vec![register]),
    );
    let mut win = env.window();
    let doc = doc_id(
        &win.open(&env, &env.file("src/main.rs", ""), "Rust", "fn main() {}")
            .await,
    );
    let got = until(&mut win, &doc, |f| {
        f.get("completion").is_some() && f.get("signature_help").is_some()
    })
    .await;
    assert_eq!(
        got["completion"]["options"]["triggerCharacters"],
        json!(["."]),
        "{got}"
    );
    assert_eq!(got["signature_help"]["options"], json!(true), "{got}");
}
