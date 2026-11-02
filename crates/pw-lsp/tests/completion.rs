//! 자동완성 · 시그니처 도움말의 Host 쪽(15-m4-plan.md M4-11)을 가짜 서버로 끝까지: 위치 · 편집 범위가 UTF-16 오프셋으로 오고,
//! `itemDefaults` 가 항목마다 채워지고, 목록이 서버 순서로 200 개에서 잘리고, 요청 사이 편집이면 `staleVersion` 이다. 받아들인 항목의
//! 명령(`workspace/executeCommand`)은 같은 문서 · 버전의 답으로 내준 것이고 서버가 알린 것일 때만 서버에 간다(메인 판정 2026-11-02).

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use pw_lsp::fake::{Reply, Scenario};
use pw_lsp::methods::LspInstanceState;
use pw_rpc::code;
use serde_json::{Value, json};

use common::env::{Env, EnvOptions};
use common::received;

fn doc_id(opened: &Value) -> String {
    opened["docId"].as_str().unwrap().to_owned()
}

fn methods_of(record: &Path, method: &str) -> Vec<Value> {
    received(record)
        .into_iter()
        .filter(|m| m["method"] == method)
        .collect()
}

fn pos(line: u32, character: u32) -> Value {
    json!({ "line": line, "character": character })
}

fn range(a: (u32, u32), b: (u32, u32)) -> Value {
    json!({ "start": pos(a.0, a.1), "end": pos(b.0, b.1) })
}

const COMMAND: &str = "fake.applyImport";

fn scenario(completion: Reply) -> Scenario {
    Scenario {
        capabilities: json!({
            "textDocumentSync": 2,
            "completionProvider": { "triggerCharacters": ["."], "resolveProvider": true },
            "signatureHelpProvider": { "triggerCharacters": ["("], "retriggerCharacters": [","] },
            "executeCommandProvider": { "commands": [COMMAND] },
        }),
        ..Scenario::default()
    }
    .reply("textDocument/completion", completion)
}

/// 둘째 줄의 한글 뒤 `.` 에서 완성을 묻는 문서: `let 값 = r.ar;` — 줄 0 은 `// 🦀` (UTF-16 다섯 칸 + 줄바꿈).
const TEXT: &str = "// 🦀\nlet 값 = r.ar;\n";

async fn ready(env: &Env, win: &mut common::env::Conn) -> String {
    let doc = doc_id(
        &win.open(env, &env.file("src/main.rs", TEXT), "Rust", TEXT)
            .await,
    );
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    doc
}

fn complete_at(doc: &str, version: u32, offset: u32) -> Value {
    json!({ "docId": doc, "version": version, "offset": offset, "trigger": { "kind": "character", "character": "." } })
}

/// 줄 1 의 `.` 바로 뒤(UTF-16 오프셋 6 + 10).
const AFTER_DOT: u32 = 6 + 10;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_items_come_with_offsets_defaults_and_the_server_order() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        scenario(Reply::result(json!({
            "isIncomplete": true,
            "itemDefaults": {
                "editRange": { "insert": range((1, 10), (1, 10)), "replace": range((1, 10), (1, 12)) },
                "insertTextFormat": 2,
                "data": { "from": "defaults" },
            },
            "items": [
                { "label": "area", "kind": 2, "insertText": "area()$0", "sortText": "b",
                  "labelDetails": { "detail": "()", "description": "Shape" },
                  "documentation": { "kind": "markdown", "value": "**넓이**" },
                  "additionalTextEdits": [{ "range": range((0, 5), (0, 5)), "newText": "\nuse a::Shape;" }],
                  "command": { "title": "import", "command": COMMAND, "arguments": [1] } },
                { "label": "width", "kind": 5, "filterText": "w", "insertTextFormat": 1, "data": 7,
                  "textEdit": { "newText": "width", "range": range((1, 9), (1, 10)) } },
            ],
        }))),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    let list = win
        .call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap();
    assert_eq!(list["staleVersion"], false);
    assert_eq!(list["isIncomplete"], true);
    assert_eq!(list["truncated"], false);
    let area = &list["items"][0];
    assert_eq!(area["label"], "area");
    assert_eq!(area["labelDetail"], "()");
    assert_eq!(area["labelDescription"], "Shape");
    assert_eq!(area["snippet"], true);
    // 기본 편집 범위(`editRange`)가 오프셋으로: 삽입 16..16, 바꾸기 16..18(`ar`).
    assert_eq!(
        area["edit"],
        json!({ "newText": "area()$0", "insert": { "from": 16, "to": 16 }, "replace": { "from": 16, "to": 18 } })
    );
    // 줄 0 의 `🦀` 뒤(UTF-16 5).
    assert_eq!(
        area["additionalEdits"],
        json!([{ "from": 5, "to": 5, "newText": "\nuse a::Shape;" }])
    );
    assert_eq!(
        area["documentation"],
        json!({ "markdown": true, "value": "**넓이**" })
    );
    assert_eq!(area["raw"]["data"], json!({ "from": "defaults" }));
    let width = &list["items"][1];
    assert_eq!(width["filterText"], "w");
    assert_eq!(width["snippet"], false);
    assert_eq!(width["raw"]["data"], 7);
    assert_eq!(width["edit"]["insert"], json!({ "from": 15, "to": 16 }));
    // 서버가 받은 것: 위치(LSP 줄 · 글자)와 트리거.
    let asked = &methods_of(&record, "textDocument/completion")[0]["params"];
    assert_eq!(asked["position"], pos(1, 10));
    assert_eq!(
        asked["context"],
        json!({ "triggerKind": 2, "triggerCharacter": "." })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_list_is_cut_at_200_in_the_server_order() {
    let env = Env::new(EnvOptions::default());
    let items: Vec<Value> = (0..250)
        .map(|i| json!({ "label": format!("item{i:03}"), "sortText": format!("{:03}", 999 - i) }))
        .collect();
    env.fake("rust-analyzer", scenario(Reply::result(json!(items))));
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    let list = win
        .call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap();
    let labels: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels.len(), 200);
    assert_eq!(labels[0], "item000");
    assert_eq!(labels[199], "item199");
    assert_eq!(list["truncated"], true);
    assert_eq!(list["isIncomplete"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_while_the_server_thinks_marks_the_list_stale() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        scenario(Reply::result(json!([{ "label": "area" }])).delayed(300)),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    let asked = win.send("lsp.completion", complete_at(&doc, 0, AFTER_DOT));
    tokio::time::sleep(Duration::from_millis(100)).await;
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 16, "to": 16, "insert": "a" }] }),
    )
    .await
    .unwrap();
    let list = win.response(asked).await.unwrap();
    assert_eq!(list["staleVersion"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_brings_late_documentation_and_import_edits() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        scenario(Reply::result(json!([{ "label": "area", "data": 1 }]))).reply(
            "completionItem/resolve",
            Reply::result(json!({
                "label": "area", "data": 1,
                "documentation": "넓이를 돌려준다 <script>",
                "additionalTextEdits": [{ "range": range((1, 0), (1, 0)), "newText": "use a::area;\n" }],
            })),
        ),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    let list = win
        .call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap();
    let resolved = win
        .call(
            "lsp.completion.resolve",
            json!({ "docId": doc, "version": 0, "item": list["items"][0]["raw"] }),
        )
        .await
        .unwrap();
    assert_eq!(resolved["staleVersion"], false);
    assert_eq!(
        resolved["item"]["documentation"],
        json!({ "markdown": false, "value": "넓이를 돌려준다 <script>" })
    );
    assert_eq!(
        resolved["item"]["additionalEdits"],
        json!([{ "from": 6, "to": 6, "newText": "use a::area;\n" }])
    );
    // 서버는 받은 항목 그대로를 다시 받는다(`data` 포함).
    assert_eq!(
        methods_of(&record, "completionItem/resolve")[0]["params"],
        json!({ "label": "area", "data": 1 })
    );
}

/// 받아들인 항목의 명령: 같은 문서 · 버전의 답으로 내준 명령이고 서버가 알린 것만 서버에 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_offered_commands_the_server_announced_reach_execute_command() {
    let env = Env::new(EnvOptions::default());
    let known = json!({ "title": "import", "command": COMMAND, "arguments": ["a", 1] });
    let unknown = json!({ "title": "run", "command": "fake.notAnnounced" });
    let record = env.fake(
        "rust-analyzer",
        scenario(Reply::result(json!([
            { "label": "area", "command": known },
            { "label": "width", "command": unknown },
        ])))
        .reply("workspace/executeCommand", Reply::result(json!("done"))),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    win.call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap();
    let execute = |version: u32, command: &Value| json!({ "docId": doc, "version": version, "command": command });
    // 서버가 알리지 않은 명령은 답으로 내주었어도 보내지 않는다.
    let refused = win
        .call("lsp.completion.execute", execute(0, &unknown))
        .await
        .unwrap_err();
    assert_eq!(refused.code, code::PERMISSION_DENIED);
    // 다른 버전의 답 · 답으로 내주지 않은 명령(인자가 다름)도 보내지 않는다.
    let stale = win
        .call("lsp.completion.execute", execute(1, &known))
        .await
        .unwrap_err();
    assert_eq!(stale.code, code::PERMISSION_DENIED);
    let forged = json!({ "title": "import", "command": COMMAND, "arguments": ["/etc/passwd"] });
    let forged = win
        .call("lsp.completion.execute", execute(0, &forged))
        .await
        .unwrap_err();
    assert_eq!(forged.code, code::PERMISSION_DENIED);
    // 다른 창(연결)은 이 문서를 모른다.
    let mut other = env.window();
    let not_found = other
        .call("lsp.completion.execute", execute(0, &known))
        .await
        .unwrap_err();
    assert_eq!(not_found.code, code::NOT_FOUND);
    assert!(methods_of(&record, "workspace/executeCommand").is_empty());
    // 내준 것 · 알린 것이면 간다.
    let done = win
        .call("lsp.completion.execute", execute(0, &known))
        .await
        .unwrap();
    assert_eq!(done["result"], "done");
    let sent = methods_of(&record, "workspace/executeCommand");
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["params"],
        json!({ "command": COMMAND, "arguments": ["a", 1] })
    );
}

/// 미신뢰 프로젝트에서는 서버가 뜨지 않는다 — 완성은 맡을 서버 없음(5001), 명령은 보내지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_completes_nothing_and_runs_no_command() {
    let env = Env::new(EnvOptions {
        trusted: false,
        ..EnvOptions::default()
    });
    let record = env.fake("rust-analyzer", scenario(Reply::result(json!([]))));
    let mut win = env.window();
    let opened = win
        .open(&env, &env.file("src/main.rs", TEXT), "Rust", TEXT)
        .await;
    let doc = doc_id(&opened);
    let none = win
        .call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap_err();
    assert_eq!(none.code, pw_lsp::manager::code::NO_SERVER);
    let refused = win
        .call(
            "lsp.completion.execute",
            json!({ "docId": doc, "version": 0, "command": { "title": "x", "command": COMMAND } }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, code::PERMISSION_DENIED);
    assert!(received(&record).is_empty(), "no server was started");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signature_help_marks_parameters_in_the_label_and_passes_the_context() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        scenario(Reply::result(json!([]))).reply(
            "textDocument/signatureHelp",
            Reply::result(json!({
                "signatures": [{
                    "label": "fn scale(넓이: u32, height: u32)",
                    "parameters": [{ "label": "넓이: u32" }, { "label": [18, 29], "documentation": "높이" }],
                }],
                "activeSignature": 0,
                "activeParameter": 1,
            })),
        ),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    let answer = win
        .call(
            "lsp.signatureHelp",
            json!({ "docId": doc, "version": 0, "offset": AFTER_DOT, "triggerKind": "character",
                    "triggerCharacter": ",", "isRetrigger": true, "active": { "signatures": [], "activeSignature": 0 } }),
        )
        .await
        .unwrap();
    let signature = &answer["help"]["signatures"][0];
    assert_eq!(signature["activeParameter"], 1);
    assert_eq!(
        signature["parameters"],
        json!([
            { "from": 9, "to": 16, "documentation": null },
            { "from": 18, "to": 29, "documentation": { "markdown": false, "value": "높이" } },
        ])
    );
    assert_eq!(
        methods_of(&record, "textDocument/signatureHelp")[0]["params"]["context"],
        json!({ "triggerKind": 2, "triggerCharacter": ",", "isRetrigger": true,
                "activeSignatureHelp": { "signatures": [], "activeSignature": 0 } })
    );
    // 서버가 `null` 이면(호출 밖) 도움말이 없다.
    let env2 = Env::new(EnvOptions::default());
    env2.fake(
        "rust-analyzer",
        scenario(Reply::result(json!([])))
            .reply("textDocument/signatureHelp", Reply::result(Value::Null)),
    );
    let mut win2 = env2.window();
    let doc2 = ready(&env2, &mut win2).await;
    let none = win2
        .call(
            "lsp.signatureHelp",
            json!({ "docId": doc2, "version": 0, "offset": 0, "triggerKind": "invoked", "isRetrigger": false }),
        )
        .await
        .unwrap();
    assert_eq!(none["help"], Value::Null);
}
