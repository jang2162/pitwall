//! 자동완성 · 시그니처 도움말의 Host 쪽(15-m4-plan.md M4-11)을 가짜 서버로 끝까지: 위치 · 편집 범위가 UTF-16 오프셋으로 오고,
//! `itemDefaults` 가 항목마다 채워지고, 목록이 서버 순서로 200 개에서 잘리고, 요청 사이 편집이면 `staleVersion` 이다. 받아들인 항목의
//! 명령(`workspace/executeCommand`)은 같은 문서 · 버전의 답으로 내준 것이고 서버가 알린 것일 때만 서버에 간다(메인 판정 2026-11-02).

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use pw_lsp::fake::{Reply, Scenario};
use pw_lsp::methods::LspInstanceState;
use pw_rpc::{Outbound, code};
use serde_json::{Value, json};

use common::env::{Env, EnvOptions};
use common::received;

fn doc_id(opened: &Value) -> String {
    opened["docId"].as_str().unwrap().to_owned()
}

/// 가짜 서버가 보낸 서버 → 클라이언트 요청 `id` 에 Pitwall 이 준 답(10 s 기다림).
async fn answer_to(record: &Path, id: &str) -> Value {
    let answered = |m: &Value| m["id"] == id && m.get("method").is_none();
    let found = common::eventually(Duration::from_secs(10), || {
        received(record).iter().any(answered)
    })
    .await;
    assert!(found, "no answer to {id}: {:?}", received(record));
    received(record).into_iter().find(answered).unwrap()["result"].clone()
}

/// 창에 온 이벤트 `topic`(`limit` 동안).
async fn next_event(win: &mut common::env::Conn, topic: &str, limit: Duration) -> Option<Value> {
    let is_event = |m: &Outbound| {
        matches!(m, Outbound::Notification(n)
            if n.method == pw_rpc::protocol::EVENTS_EVENT && n.params["topic"] == topic)
    };
    if let Some(at) = win.pending.iter().position(is_event)
        && let Outbound::Notification(n) = win.pending.remove(at)
    {
        return Some(n.params["payload"].clone());
    }
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let msg = tokio::time::timeout_at(deadline, win.rx.recv())
            .await
            .ok()??;
        if is_event(&msg) {
            let Outbound::Notification(n) = msg else {
                unreachable!()
            };
            return Some(n.params["payload"].clone());
        }
        win.pending.push(msg);
    }
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
    // 서버가 준 원래 항목은 Host 가 쥐고 웹뷰에는 답 안의 번호만 간다.
    assert_eq!(area["id"], 0);
    assert!(area.get("raw").is_none());
    let width = &list["items"][1];
    assert_eq!(width["filterText"], "w");
    assert_eq!(width["snippet"], false);
    assert_eq!(width["id"], 1);
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
            json!({ "docId": doc, "version": 0, "listVersion": 0, "id": list["items"][0]["id"] }),
        )
        .await
        .unwrap();
    assert_eq!(resolved["staleVersion"], false);
    assert_eq!(resolved["item"]["id"], 0);
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
    let execute = |version: u32, command: &Value| json!({ "docId": doc, "version": version, "flowId": "f1", "command": command });
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

/// resolve 는 Host 가 그 답으로 내준 항목만 서버에 보낸다 — 웹뷰가 지어낸 `command` 를 단 항목을 resolve 에 보내 「내준 명령」 으로
/// 만들 수 없다(리뷰 H-2 의 재현: resolve 답 `null` → 예전에는 웹뷰가 보낸 항목을 답으로 썼다). 서버가 받는 것은 Host 가 쥔 원래
/// 항목이므로 받은 항목을 되돌려 주는 서버여도 지어낸 명령이 돌아올 길이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_sends_only_items_the_host_offered_and_a_forged_command_never_runs() {
    let forged =
        json!({ "title": "import", "command": COMMAND, "arguments": ["/etc/passwd", "FORGED"] });
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        scenario(Reply::result(json!([
            { "label": "area", "data": 1 },
            { "label": "width", "data": 2 },
        ])))
        .reply("completionItem/resolve", Reply::result(Value::Null))
        .reply("workspace/executeCommand", Reply::result(json!("done"))),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    win.call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap();
    // 옛 꼴(웹뷰가 항목 JSON 을 보냄)은 받지 않는다 — 번호가 없으면 잘못된 인자다.
    let old = win
        .call(
            "lsp.completion.resolve",
            json!({ "docId": doc, "version": 0, "item": { "label": "nope", "command": forged } }),
        )
        .await
        .unwrap_err();
    assert_eq!(old.code, code::INVALID_PARAMS);
    // 내주지 않은 번호 · 다른 버전의 답은 서버에 묻지 않고 거절한다.
    for (list_version, id) in [(0, 2), (1, 0)] {
        let refused = win
            .call(
                "lsp.completion.resolve",
                json!({ "docId": doc, "version": 0, "listVersion": list_version, "id": id }),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code, code::PERMISSION_DENIED);
    }
    assert!(methods_of(&record, "completionItem/resolve").is_empty());
    // 내준 항목: 서버가 받는 것은 Host 가 쥔 원래 항목이고, 빈 답이면 그 원래 항목 그대로다.
    let resolved = win
        .call(
            "lsp.completion.resolve",
            json!({ "docId": doc, "version": 0, "listVersion": 0, "id": 1 }),
        )
        .await
        .unwrap();
    assert_eq!(
        methods_of(&record, "completionItem/resolve")[0]["params"],
        json!({ "label": "width", "data": 2 })
    );
    assert_eq!(resolved["item"]["label"], "width");
    assert_eq!(resolved["item"]["command"], Value::Null);
    let denied = win
        .call(
            "lsp.completion.execute",
            json!({ "docId": doc, "version": 0, "flowId": "f1", "command": forged }),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.code, code::PERMISSION_DENIED);
    assert!(methods_of(&record, "workspace/executeCommand").is_empty());
}

/// 내준 항목의 resolve 답이 준 명령은 그 항목의 것이다 — resolve 전에는 보내지 않고, 뒤에는 같은 답 버전으로 보낸다. 같은 항목을 두 번
/// resolve 해도 명령은 그 항목에 하나다(앞 답의 명령을 잊는 것은 단위 시험 `offered_items_and_commands_are_kept_per_document_and_version`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_from_the_resolve_answer_of_an_offered_item_runs() {
    let first = json!({ "title": "import", "command": COMMAND, "arguments": ["a", 1] });
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        scenario(Reply::result(json!([{ "label": "area", "data": 1 }])))
            .reply(
                "completionItem/resolve",
                Reply::result(json!({ "label": "area", "data": 1, "command": first })),
            )
            .reply("workspace/executeCommand", Reply::result(json!("done"))),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    win.call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap();
    let execute = json!({ "docId": doc, "version": 0, "flowId": "f1", "command": first });
    let before = win
        .call("lsp.completion.execute", execute.clone())
        .await
        .unwrap_err();
    assert_eq!(before.code, code::PERMISSION_DENIED, "not resolved yet");
    for _ in 0..2 {
        win.call(
            "lsp.completion.resolve",
            json!({ "docId": doc, "version": 0, "listVersion": 0, "id": 0 }),
        )
        .await
        .unwrap();
    }
    win.call("lsp.completion.execute", execute).await.unwrap();
    assert_eq!(
        methods_of(&record, "workspace/executeCommand")[0]["params"],
        json!({ "command": COMMAND, "arguments": ["a", 1] })
    );
}

/// 완성 항목의 명령은 M4-12 의 명령 길(`lsp.command.execute` 와 같은 `pw_lsp::edit::execute_command`)로 간다: 도는 동안 그 서버가
/// 건 `workspace/applyEdit` 은 이 창에 이벤트 `lsp.applyEdit`(흐름 id 실림)로 넘어오고 창의 답이 서버에 간다. 명령이 끝난 뒤 온 것은
/// `applied: false` 다(결정 M4-D15 — 메인 판정 2026-11-02: 나중에 병합하는 행이 두 길을 합친다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completion_command_hands_the_servers_edit_to_the_window_like_any_command() {
    let env = Env::new(EnvOptions::default());
    let main = env.root.join("src/main.rs");
    let edit_request = || pw_lsp::fake::Action::Request {
        method: "workspace/applyEdit".into(),
        params: json!({ "label": "Import", "edit": { "changes": {
            pw_lsp::uri::file_uri(&main).unwrap(): [{ "range": range((0, 0), (0, 0)), "newText": "use a::Shape;\n" }]
        }}}),
    };
    let command = json!({ "title": "import", "command": COMMAND, "arguments": ["a"] });
    let record = env.fake(
        "rust-analyzer",
        scenario(Reply::result(
            json!([{ "label": "Shape", "command": command }]),
        ))
        .on("workspace/executeCommand", vec![edit_request()])
        .reply(
            "workspace/executeCommand",
            Reply {
                result: Some(json!("done")),
                delay_ms: 500,
                then: vec![edit_request()],
                ..Reply::default()
            },
        ),
    );
    let mut win = env.window();
    win.call(
        "events.subscribe",
        json!({ "projectId": env.id, "topics": ["lsp.applyEdit"] }),
    )
    .await
    .unwrap();
    let doc = ready(&env, &mut win).await;
    win.call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap();
    let call = win.send(
        "lsp.completion.execute",
        json!({ "docId": doc, "version": 0, "flowId": "c1", "command": command }),
    );
    let event = next_event(&mut win, "lsp.applyEdit", Duration::from_secs(10))
        .await
        .expect("the window gets the server's edit");
    assert_eq!(event["flowId"], "c1");
    assert_eq!(event["server"], "rust-analyzer");
    assert_eq!(event["edit"]["changes"][0]["path"], json!(main));
    win.call(
        "lsp.applyEdit.answer",
        json!({ "flowId": "c1", "requestId": event["requestId"], "applied": true }),
    )
    .await
    .unwrap();
    assert_eq!(answer_to(&record, "fake-1").await["applied"], true);
    assert_eq!(
        win.response(call).await.unwrap(),
        json!({ "server": "rust-analyzer", "result": "done" })
    );
    // 명령이 끝난 뒤 온 편집은 창에 넘기지 않는다.
    assert_eq!(answer_to(&record, "fake-2").await["applied"], false);
    assert_eq!(
        methods_of(&record, "workspace/executeCommand")[0]["params"],
        json!({ "command": COMMAND, "arguments": ["a"] })
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
            json!({ "docId": doc, "version": 0, "flowId": "f1", "command": { "title": "x", "command": COMMAND } }),
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

/// 완성 명령은 완성 기능의 라우팅을 탄다 — 사람이 그 서버의 코드 액션만 꺼도(`features.disable = ["code_action"]`) 완성 항목의 명령은
/// 서버에 간다(재리뷰 N-1). 코드 액션의 명령 길(`lsp.command.execute`)은 꺼진 기능이라 5001 이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completion_command_runs_when_code_actions_are_turned_off() {
    let env = Env::new(EnvOptions::default());
    let known = json!({ "title": "import", "command": COMMAND, "arguments": ["a"] });
    let record = env.fake(
        "rust-analyzer",
        scenario(Reply::result(
            json!([{ "label": "area", "command": known }]),
        ))
        .reply("workspace/executeCommand", Reply::result(json!("done"))),
    );
    let settings = env.paths.config.join("settings.toml");
    let mut text = std::fs::read_to_string(&settings).unwrap();
    text.push_str("[lsp.rust-analyzer.features]\ndisable = [\"code_action\"]\n");
    std::fs::write(&settings, text).unwrap();
    env.global.reload().unwrap();
    let mut win = env.window();
    let doc = ready(&env, &mut win).await;
    let list = win
        .call("lsp.completion", complete_at(&doc, 0, AFTER_DOT))
        .await
        .unwrap();
    assert_eq!(list["items"][0]["label"], "area");
    let as_action = win
        .call(
            "lsp.command.execute",
            json!({ "docId": doc, "flowId": "f1", "server": "rust-analyzer", "command": known }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        as_action.code,
        pw_lsp::manager::code::NO_SERVER,
        "{as_action:?}"
    );
    assert!(methods_of(&record, "workspace/executeCommand").is_empty());
    let done = win
        .call(
            "lsp.completion.execute",
            json!({ "docId": doc, "version": 0, "flowId": "f2", "command": known }),
        )
        .await
        .unwrap();
    assert_eq!(done["result"], "done");
    assert_eq!(methods_of(&record, "workspace/executeCommand").len(), 1);
}
