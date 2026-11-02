//! 문서 심볼(15-m4-plan.md M4-10): `lsp.symbols.document` 를 가짜 서버로 끝까지. 계층 답이 부모 색인 목록 + UTF-16 오프셋으로
//! 오고, 맡을 서버가 없으면(능력 없음 · 미신뢰) `5001` — 웹뷰는 그때 심볼 인덱스로 대신한다. 낡은 버전으로 물으면 `staleVersion`.

#![cfg(unix)]

mod common;

use pw_lsp::fake::{Reply, Scenario};
use pw_lsp::methods::LspInstanceState;
use serde_json::{Value, json};

use common::env::{Env, EnvOptions};

/// `fn 한() {}` 다음 줄의 `struct P { x: u8 }` — 첫 줄의 한글 한 글자가 UTF-16 오프셋에 그대로 든다.
const TEXT: &str = "fn 한() {}\nstruct P { x: u8 }\n";

fn r(sl: u32, sc: u32, el: u32, ec: u32) -> Value {
    json!({ "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } })
}

fn scenario() -> Scenario {
    Scenario {
        capabilities: json!({ "textDocumentSync": 2, "documentSymbolProvider": true }),
        ..Scenario::default()
    }
    .reply(
        "textDocument/documentSymbol",
        Reply::result(json!([
            { "name": "한", "detail": "fn()", "kind": 12, "range": r(0, 0, 0, 9), "selectionRange": r(0, 3, 0, 4) },
            { "name": "P", "kind": 23, "range": r(1, 0, 1, 18), "selectionRange": r(1, 7, 1, 8),
              "children": [
                { "name": "x", "detail": "u8", "kind": 8, "range": r(1, 11, 1, 16), "selectionRange": r(1, 11, 1, 12) }
              ] }
        ])),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hierarchical_answer_comes_back_as_a_parent_indexed_list_with_offsets() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", scenario());
    let mut win = env.window();
    let opened = win
        .open(&env, &env.file("src/main.rs", TEXT), "Rust", TEXT)
        .await;
    let doc = opened["docId"].as_str().unwrap().to_owned();
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let got = win
        .call(
            "lsp.symbols.document",
            json!({ "docId": doc, "version": 0 }),
        )
        .await
        .unwrap();
    assert_eq!(
        got,
        json!({
            "server": "rust-analyzer",
            "staleVersion": false,
            "truncated": false,
            "symbols": [
                { "name": "한", "detail": "fn()", "kind": 12, "deprecated": false, "parent": null,
                  "from": 0, "to": 9, "selectionFrom": 3, "selectionTo": 4 },
                { "name": "P", "detail": null, "kind": 23, "deprecated": false, "parent": null,
                  "from": 10, "to": 28, "selectionFrom": 17, "selectionTo": 18 },
                { "name": "x", "detail": "u8", "kind": 8, "deprecated": false, "parent": 1,
                  "from": 21, "to": 26, "selectionFrom": 21, "selectionTo": 22 },
            ],
        })
    );
    // 웹뷰가 계산한 버전이 지금 문서와 다르면 답에 `staleVersion` — 웹뷰는 버린다.
    let stale = win
        .call(
            "lsp.symbols.document",
            json!({ "docId": doc, "version": 7 }),
        )
        .await
        .unwrap();
    assert_eq!(stale["staleVersion"], true);
    win.session.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_without_document_symbols_answers_no_server() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "hoverProvider": true }),
            ..Scenario::default()
        },
    );
    let mut win = env.window();
    let opened = win
        .open(&env, &env.file("src/main.rs", TEXT), "Rust", TEXT)
        .await;
    let doc = opened["docId"].as_str().unwrap().to_owned();
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let err = win
        .call("lsp.symbols.document", json!({ "docId": doc }))
        .await
        .unwrap_err();
    assert_eq!(err.code, pw_lsp::manager::code::NO_SERVER);
    win.session.close();
}

/// 미신뢰 프로젝트는 서버를 띄우지 않으므로(결정 M4-D8) 문서 심볼은 `5001` 이다 — 웹뷰가 인덱스로 조용히 대신한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_answers_no_server_without_starting_one() {
    let env = Env::new(EnvOptions {
        trusted: false,
        ..EnvOptions::default()
    });
    env.fake("rust-analyzer", scenario());
    let mut win = env.window();
    let opened = win
        .open(&env, &env.file("src/main.rs", TEXT), "Rust", TEXT)
        .await;
    let doc = opened["docId"].as_str().unwrap().to_owned();
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::Untrusted)
    );
    let err = win
        .call("lsp.symbols.document", json!({ "docId": doc }))
        .await
        .unwrap_err();
    assert_eq!(err.code, pw_lsp::manager::code::NO_SERVER);
    assert_eq!(env.manager.spawn_count(), 0);
    win.session.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_windows_document_is_not_found() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", scenario());
    let mut win = env.window();
    let opened = win
        .open(&env, &env.file("src/main.rs", TEXT), "Rust", TEXT)
        .await;
    let doc = opened["docId"].as_str().unwrap().to_owned();
    let mut other = env.window();
    let err = other
        .call("lsp.symbols.document", json!({ "docId": doc }))
        .await
        .unwrap_err();
    assert_eq!(err.code, pw_rpc::code::NOT_FOUND);
    win.session.close();
    other.session.close();
}
