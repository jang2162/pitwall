//! 편집 기능의 Host 쪽(15-m4-plan.md M4-12)을 가짜 서버로 끝까지: rename 의 범위 · 편집, 코드 액션의 진단 문맥 · 선호 차례 ·
//! resolve, 서버 명령이 도는 동안에만 받는 `workspace/applyEdit`(결정 M4-D15 — 명령 밖 · 다른 서버 · 명령 뒤 · 창 닫힘은
//! `applied: false`).

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use pw_lsp::edit::code as edit_code;
use pw_lsp::fake::{Action, Reply, Scenario};
use pw_lsp::methods::LspInstanceState;
use pw_rpc::{Outbound, code};
use serde_json::{Value, json};

use common::env::{Conn, Env, EnvOptions};
use common::{eventually, received};

fn doc_id(opened: &Value) -> String {
    opened["docId"].as_str().unwrap().to_owned()
}

fn uri(path: &Path) -> String {
    pw_lsp::uri::file_uri(path).unwrap()
}

fn range(l1: u32, c1: u32, l2: u32, c2: u32) -> Value {
    json!({ "start": { "line": l1, "character": c1 }, "end": { "line": l2, "character": c2 } })
}

/// 가짜 서버가 받은 요청 `method` 의 마지막 `params`.
fn last_request(record: &Path, method: &str) -> Option<Value> {
    received(record)
        .into_iter()
        .rev()
        .find(|m| m["method"] == method)
        .map(|m| m["params"].clone())
}

/// 가짜 서버가 보낸 서버 → 클라이언트 요청 `id` 에 Pitwall 이 준 답(10 s 기다림).
async fn answer_to(record: &Path, id: &str) -> Value {
    let found = eventually(Duration::from_secs(10), || {
        received(record)
            .iter()
            .any(|m| m["id"] == id && m.get("method").is_none())
    })
    .await;
    assert!(found, "no answer to {id}: {:?}", received(record));
    received(record)
        .into_iter()
        .find(|m| m["id"] == id && m.get("method").is_none())
        .unwrap()["result"]
        .clone()
}

/// 창에 온 이벤트 `topic`(`limit` 동안).
async fn next_event(win: &mut Conn, topic: &str, limit: Duration) -> Option<Value> {
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

async fn ready(env: &Env, win: &mut Conn, rel: &str, language: &str, text: &str) -> String {
    let doc = doc_id(&win.open(env, &env.file(rel, text), language, text).await);
    let server = match language {
        "Python" => "basedpyright",
        _ => "rust-analyzer",
    };
    env.wait_state(server, LspInstanceState::Ready).await;
    doc
}

/// prepareRename 의 범위가 웹뷰 오프셋으로 오고, rename 의 편집은 경로 · 버전 · LSP 위치 그대로의 목록이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_gives_the_prepared_range_and_a_normalized_edit() {
    let env = Env::new(EnvOptions::default());
    let main = env.root.join("src/main.rs");
    let lib = env.root.join("src/lib.rs");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "renameProvider": { "prepareProvider": true } }),
            ..Scenario::default()
        }
        .reply(
            "textDocument/prepareRename",
            Reply::result(json!({ "range": range(1, 3, 1, 7), "placeholder": "main" })),
        )
        .reply(
            "textDocument/rename",
            Reply::result(json!({ "documentChanges": [
                { "textDocument": { "uri": uri(&main), "version": 0 },
                  "edits": [{ "range": range(1, 3, 1, 7), "newText": "start" }] },
                { "textDocument": { "uri": uri(&lib), "version": null },
                  "edits": [{ "range": range(0, 0, 0, 4), "newText": "start" }] },
            ]})),
        ),
    );
    let mut win = env.window();
    // 첫 줄은 한글(UTF-16 으로 3 글자) — 오프셋은 웹뷰 단위다.
    let doc = ready(
        &env,
        &mut win,
        "src/main.rs",
        "Rust",
        "// 한글\nfn main() {}\n",
    )
    .await;
    let prepared = win
        .call(
            "lsp.rename.prepare",
            json!({ "docId": doc, "version": 0, "offset": 9 }),
        )
        .await
        .unwrap();
    assert_eq!(
        prepared,
        json!({
            "server": "rust-analyzer",
            "staleVersion": false,
            "target": { "kind": "range", "from": 9, "to": 13, "placeholder": "main" },
        })
    );
    let answer = win
        .call(
            "lsp.rename",
            json!({ "docId": doc, "version": 0, "offset": 9, "newName": "start" }),
        )
        .await
        .unwrap();
    assert_eq!(answer["staleVersion"], false);
    assert_eq!(
        answer["edit"]["changes"],
        json!([
            { "kind": "text", "path": main, "version": 0, "foreign": false,
              "edits": [{ "range": range(1, 3, 1, 7), "newText": "start" }] },
            { "kind": "text", "path": lib, "version": null, "foreign": false,
              "edits": [{ "range": range(0, 0, 0, 4), "newText": "start" }] },
        ])
    );
    let sent = last_request(&record, "textDocument/rename").unwrap();
    assert_eq!(sent["newName"], "start");
    assert_eq!(sent["position"], json!({ "line": 1, "character": 3 }));
}

/// `prepareRename` 이 없으면 웹뷰가 낱말을 고르고(`default`), 서버가 거절하면 그 글로 `invalid`, `null` 이면 `invalid`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_rename_falls_back_or_reports_why_it_cannot_rename() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "renameProvider": true }),
            ..Scenario::default()
        },
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win, "src/main.rs", "Rust", "fn main() {}\n").await;
    let prepared = win
        .call(
            "lsp.rename.prepare",
            json!({ "docId": doc, "version": 0, "offset": 3 }),
        )
        .await
        .unwrap();
    assert_eq!(prepared["target"], json!({ "kind": "default" }));

    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "renameProvider": { "prepareProvider": true } }),
            ..Scenario::default()
        }
        .reply(
            "textDocument/prepareRename",
            Reply {
                error: Some(json!({ "code": -32803, "message": "No references found at position" })),
                ..Reply::default()
            },
        ),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win, "src/main.rs", "Rust", "fn main() {}\n").await;
    let prepared = win
        .call(
            "lsp.rename.prepare",
            json!({ "docId": doc, "version": 0, "offset": 0 }),
        )
        .await
        .unwrap();
    assert_eq!(
        prepared["target"],
        json!({ "kind": "invalid", "message": "No references found at position" })
    );
}

/// 루트 밖 스킴 · `.git` 안을 만드는 편집은 전체 거절(5120)이다 — 웹뷰에 아무것도 넘어가지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_touching_git_or_another_scheme_is_refused_whole() {
    let env = Env::new(EnvOptions::default());
    let main = env.root.join("src/main.rs");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "renameProvider": true }),
            ..Scenario::default()
        }
        .reply(
            "textDocument/rename",
            Reply::result(json!({ "documentChanges": [
                { "textDocument": { "uri": uri(&main), "version": 0 },
                  "edits": [{ "range": range(0, 3, 0, 7), "newText": "start" }] },
                { "kind": "create", "uri": uri(&env.root.join(".git/hooks/post-checkout")) },
            ]})),
        ),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win, "src/main.rs", "Rust", "fn main() {}\n").await;
    let e = win
        .call(
            "lsp.rename",
            json!({ "docId": doc, "version": 0, "offset": 3, "newName": "start" }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, edit_code::EDIT_REFUSED);
    assert_eq!(e.data.unwrap()["reason"], "git");
}

/// 부른 쪽이 준 진단은 그 문서 버전일 때만 서버의 `context.diagnostics` 로 간다. 선호 액션이 먼저다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_actions_carry_the_diagnostics_of_the_same_version_and_put_preferred_first() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "codeActionProvider": true }),
            ..Scenario::default()
        }
        .reply(
            "textDocument/codeAction",
            Reply::result(json!([
                { "title": "Extract", "kind": "refactor.extract" },
                { "title": "Remove unused import", "kind": "quickfix", "isPreferred": true },
            ])),
        ),
    );
    let mut win = env.window();
    let doc = ready(
        &env,
        &mut win,
        "src/main.rs",
        "Rust",
        "use std::fmt;\nfn main() {}\n",
    )
    .await;
    let diagnostic = json!({ "range": range(0, 4, 0, 12), "message": "unused import", "code": "unused_imports" });
    let list = win
        .call(
            "lsp.codeAction.list",
            json!({ "docId": doc, "version": 0, "from": 5, "to": 5,
                    "diagnostics": { "version": 0, "items": [diagnostic] } }),
        )
        .await
        .unwrap();
    let titles: Vec<&str> = list["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Remove unused import", "Extract"]);
    let sent = last_request(&record, "textDocument/codeAction").unwrap();
    assert_eq!(sent["context"]["diagnostics"], json!([diagnostic]));
    assert_eq!(sent["context"]["triggerKind"], 1);
    assert_eq!(sent["range"], range(0, 5, 0, 5));
    // 다른 버전의 진단은 버린다.
    win.call(
        "lsp.codeAction.list",
        json!({ "docId": doc, "version": 0, "from": 5, "to": 5, "trigger": "automatic",
                "diagnostics": { "version": 7, "items": [diagnostic] } }),
    )
    .await
    .unwrap();
    let sent = last_request(&record, "textDocument/codeAction").unwrap();
    assert_eq!(sent["context"]["diagnostics"], json!([]));
    assert_eq!(sent["context"]["triggerKind"], 2);
}

/// 진단 없이 물으면(`⌥↩` · 전구) Host 가 진단 저장(M4-06)의 범위 조회로 채운다 — 코드 액션을 맡은 서버가 낸 것이고 요청 문서
/// 버전의 것만(메인 판정 2026-11-02 — 나중에 병합한 M4-06 이 더했다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_actions_without_diagnostics_take_the_answering_servers_stored_ones() {
    let env = Env::new(EnvOptions::default());
    let file = env.root.join("app.py");
    let mut records = std::collections::HashMap::new();
    for server in ["basedpyright", "ruff"] {
        let diagnostic = json!({ "range": range(0, 0, 0, 1), "severity": 1, "message": format!("{server} says"), "code": "E1", "data": { "fix": server } });
        records.insert(
            server,
            env.fake(
                server,
                Scenario {
                    capabilities: json!({ "textDocumentSync": 2, "codeActionProvider": true }),
                    ..Scenario::default()
                }
                .on(
                    "textDocument/didOpen",
                    vec![Action::Notify {
                        method: "textDocument/publishDiagnostics".into(),
                        params: json!({ "uri": uri(&file), "version": 0, "diagnostics": [diagnostic] }),
                    }],
                )
                .reply("textDocument/codeAction", Reply::result(json!([]))),
            ),
        );
    }
    let mut win = env.window();
    let doc = ready(&env, &mut win, "app.py", "Python", "x = 1\n").await;
    // 두 서버의 진단이 다 쌓일 때까지.
    let stored = || {
        let mut win = env.window();
        let env = &env;
        let file = file.clone();
        async move {
            let list = win
                .call(
                    "lsp.diagnostics.list",
                    json!({ "projectId": env.id, "path": file, "limit": 10 }),
                )
                .await
                .unwrap();
            list["files"][0]["diagnostics"]
                .as_array()
                .map_or(0, Vec::len)
        }
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while stored().await < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "both servers' diagnostics never arrived"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let list = win
        .call(
            "lsp.codeAction.list",
            json!({ "docId": doc, "version": 0, "from": 0, "to": 0 }),
        )
        .await
        .unwrap();
    let server = list["server"].as_str().unwrap().to_owned();
    let other = if server == "ruff" {
        "basedpyright"
    } else {
        "ruff"
    };
    let sent = last_request(&records[server.as_str()], "textDocument/codeAction").unwrap();
    assert_eq!(
        sent["context"]["diagnostics"],
        json!([{ "range": range(0, 0, 0, 1), "severity": 1, "message": format!("{server} says"), "code": "E1", "data": { "fix": server } }]),
        "{sent}"
    );
    assert!(last_request(&records[other], "textDocument/codeAction").is_none());
    // 문서가 바뀐 뒤(버전 1) 저장된 진단은 지난 텍스트의 것이라 싣지 않는다.
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "y" }] }),
    )
    .await
    .unwrap();
    win.call(
        "lsp.codeAction.list",
        json!({ "docId": doc, "version": 1, "from": 0, "to": 0 }),
    )
    .await
    .unwrap();
    let sent = last_request(&records[server.as_str()], "textDocument/codeAction").unwrap();
    assert_eq!(sent["context"]["diagnostics"], json!([]), "{sent}");
}

/// 편집이 없는 액션은 서버가 `codeAction/resolve` 를 하면 물어 편집 · 명령을 받는다. `Command` 는 그대로 명령이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolving_asks_the_server_for_the_edit_and_keeps_commands() {
    let env = Env::new(EnvOptions::default());
    let main = env.root.join("src/main.rs");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "codeActionProvider": { "resolveProvider": true } }),
            ..Scenario::default()
        }
        .reply(
            "codeAction/resolve",
            Reply::result(json!({
                "title": "Fix", "data": 1,
                "edit": { "changes": { uri(&main): [{ "range": range(0, 0, 0, 2), "newText": "pub fn" }] } },
                "command": { "title": "Then", "command": "then.do" },
            })),
        ),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win, "src/main.rs", "Rust", "fn main() {}\n").await;
    let resolved = win
        .call(
            "lsp.codeAction.resolve",
            json!({ "docId": doc, "server": "rust-analyzer", "action": { "title": "Fix", "data": 1 } }),
        )
        .await
        .unwrap();
    assert_eq!(
        last_request(&record, "codeAction/resolve").unwrap(),
        json!({ "title": "Fix", "data": 1 })
    );
    assert_eq!(resolved["edit"]["changes"][0]["path"], json!(main));
    assert_eq!(resolved["command"]["command"], "then.do");
    let command = json!({ "title": "Run", "command": "run.it" });
    let resolved = win
        .call(
            "lsp.codeAction.resolve",
            json!({ "docId": doc, "server": "rust-analyzer", "action": command }),
        )
        .await
        .unwrap();
    assert_eq!(resolved["edit"], Value::Null);
    assert_eq!(resolved["command"], command);
}

fn apply_edit_request(path: &Path) -> Action {
    Action::Request {
        method: "workspace/applyEdit".into(),
        params: json!({ "label": "Fix it", "edit": { "changes": {
            uri(path): [{ "range": range(0, 0, 0, 0), "newText": "// fixed\n" }]
        }}}),
    }
}

/// 명령이 도는 동안 그 서버가 건 편집은 이 창에 이벤트로 넘어오고, 창의 답이 서버에 간다. 명령 밖(문서를 열 때 건 것)과 명령이
/// 끝난 뒤 늦게 온 것은 창에 넘기지 않고 `applied: false` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_edit_reaches_the_window_only_while_its_command_runs() {
    let env = Env::new(EnvOptions::default());
    let main = env.root.join("src/main.rs");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({
                "textDocumentSync": 2, "codeActionProvider": true,
                "executeCommandProvider": { "commands": ["fix.it"] },
            }),
            ..Scenario::default()
        }
        // 명령 밖: 문서를 열자마자 건다(fake-1).
        .on("textDocument/didOpen", vec![apply_edit_request(&main)])
        // 명령 안: 명령을 받으면 건다(fake-2). 명령의 답은 1 s 뒤, 그 뒤에 늦게 하나 더(fake-3).
        .on("workspace/executeCommand", vec![apply_edit_request(&main)])
        .reply(
            "workspace/executeCommand",
            Reply {
                result: Some(json!("done")),
                delay_ms: 1000,
                then: vec![apply_edit_request(&main)],
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
    let doc = ready(&env, &mut win, "src/main.rs", "Rust", "fn main() {}\n").await;
    let outside = answer_to(&record, "fake-1").await;
    assert_eq!(outside["applied"], false, "{outside}");

    let call = win.send(
        "lsp.command.execute",
        json!({ "docId": doc, "flowId": "f1", "server": "rust-analyzer", "command": { "title": "Fix", "command": "fix.it", "arguments": [1] } }),
    );
    let event = next_event(&mut win, "lsp.applyEdit", Duration::from_secs(10))
        .await
        .expect("the window gets the server's edit");
    assert_eq!(event["flowId"], "f1");
    assert_eq!(event["server"], "rust-analyzer");
    assert_eq!(event["label"], "Fix it");
    assert_eq!(event["edit"]["changes"][0]["path"], json!(main));
    win.call(
        "lsp.applyEdit.answer",
        json!({ "flowId": "f1", "requestId": event["requestId"], "applied": true }),
    )
    .await
    .unwrap();
    assert_eq!(answer_to(&record, "fake-2").await["applied"], true);
    let executed = win.response(call).await.unwrap();
    assert_eq!(
        executed,
        json!({ "server": "rust-analyzer", "result": "done" })
    );
    assert_eq!(
        last_request(&record, "workspace/executeCommand").unwrap(),
        json!({ "command": "fix.it", "arguments": [1] })
    );
    // 명령이 끝난 뒤 온 편집.
    assert_eq!(answer_to(&record, "fake-3").await["applied"], false);
    // 답할 편집이 없는 흐름에 답하면 not found.
    let e = win
        .call(
            "lsp.applyEdit.answer",
            json!({ "flowId": "f1", "requestId": 99, "applied": true }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
}

/// 다른 서버(같은 문서의 둘째 서버)가 명령 중에 건 편집은 창에 넘기지 않고 `applied: false`. 다른 연결은 그 흐름에 답하지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_servers_edit_during_a_command_is_not_applied() {
    let env = Env::new(EnvOptions::default());
    let app = env.root.join("app.py");
    let record = env.fake(
        "basedpyright",
        Scenario {
            capabilities: json!({
                "textDocumentSync": 2, "codeActionProvider": true,
                "executeCommandProvider": { "commands": ["fix.it"] },
            }),
            ..Scenario::default()
        }
        .on("workspace/executeCommand", vec![apply_edit_request(&app)])
        .reply(
            "workspace/executeCommand",
            Reply::result(json!(null)).delayed(1500),
        ),
    );
    let ruff = env.fake(
        "ruff",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2 }),
            ..Scenario::default()
        }
        .on("textDocument/didChange", vec![apply_edit_request(&app)]),
    );
    let mut win = env.window();
    win.call(
        "events.subscribe",
        json!({ "projectId": env.id, "topics": ["lsp.applyEdit"] }),
    )
    .await
    .unwrap();
    let doc = ready(&env, &mut win, "app.py", "Python", "x = 1\n").await;
    env.wait_state("ruff", LspInstanceState::Ready).await;
    let call = win.send(
        "lsp.command.execute",
        json!({ "docId": doc, "flowId": "f1", "server": "basedpyright", "command": { "title": "Fix", "command": "fix.it" } }),
    );
    let event = next_event(&mut win, "lsp.applyEdit", Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(event["server"], "basedpyright");
    // 명령이 도는 동안 문서를 고친다 → ruff 가 편집을 건다.
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "y" }] }),
    )
    .await
    .unwrap();
    assert_eq!(answer_to(&ruff, "fake-1").await["applied"], false);
    assert!(
        next_event(&mut win, "lsp.applyEdit", Duration::from_millis(300))
            .await
            .is_none(),
        "ruff's edit must not reach the window"
    );
    // 다른 창은 이 흐름의 편집에 답하지 못한다.
    let mut other = env.window();
    let e = other
        .call(
            "lsp.applyEdit.answer",
            json!({ "flowId": "f1", "requestId": event["requestId"], "applied": true }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    win.call(
        "lsp.applyEdit.answer",
        json!({ "flowId": "f1", "requestId": event["requestId"], "applied": false, "failureReason": "cancelled" }),
    )
    .await
    .unwrap();
    assert_eq!(
        answer_to(&record, "fake-1").await,
        json!({ "applied": false, "failureReason": "cancelled" })
    );
    win.response(call).await.unwrap();
}

/// 편집을 기다리는 동안 명령을 부른 창이 닫히면 `applied: false`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_window_answers_no() {
    let env = Env::new(EnvOptions::default());
    let main = env.root.join("src/main.rs");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({
                "textDocumentSync": 2, "codeActionProvider": true,
                "executeCommandProvider": { "commands": ["fix.it"] },
            }),
            ..Scenario::default()
        }
        .on("workspace/executeCommand", vec![apply_edit_request(&main)])
        .reply("workspace/executeCommand", Reply::silent()),
    );
    let mut win = env.window();
    win.call(
        "events.subscribe",
        json!({ "projectId": env.id, "topics": ["lsp.applyEdit"] }),
    )
    .await
    .unwrap();
    let doc = ready(&env, &mut win, "src/main.rs", "Rust", "fn main() {}\n").await;
    win.send(
        "lsp.command.execute",
        json!({ "docId": doc, "flowId": "f1", "server": "rust-analyzer", "command": { "title": "Fix", "command": "fix.it" } }),
    );
    next_event(&mut win, "lsp.applyEdit", Duration::from_secs(10))
        .await
        .unwrap();
    drop(win);
    assert_eq!(answer_to(&record, "fake-1").await["applied"], false);
}

/// 명령은 프로젝트 창에서만 돈다(웰컴 · 설정 창의 연결은 문서도 없다 — 연결 범위가 먼저 막는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commands_need_a_project_window() {
    let env = Env::new(EnvOptions::default());
    let mut host = env.connect(pw_rpc::ConnectionScope::Host);
    let e = host
        .call(
            "lsp.command.execute",
            json!({ "docId": "d1", "flowId": "f1", "server": "rust-analyzer", "command": { "title": "x", "command": "x" } }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
}

/// 같은 프로젝트의 다른 창이 연 파일은 서버의 텍스트가 그 창의 것이다 — 그 파일의 편집에 `foreign` 이 달린다. 같은 창이 연 파일 ·
/// 아무도 열지 않은 파일에는 달리지 않는다(결정 M4-D14 ① — 리뷰 H1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_marks_files_another_window_has_open() {
    let env = Env::new(EnvOptions::default());
    let main = env.root.join("src/main.rs");
    let theirs = env.file("src/theirs.rs", "pub fn old() {}\n");
    let mine = env.file("src/mine.rs", "pub fn old2() {}\n");
    let closed = env.file("src/closed.rs", "use crate::old;\n");
    let text = |path: &Path| {
        json!({ "textDocument": { "uri": uri(path), "version": null },
                "edits": [{ "range": range(0, 0, 0, 0), "newText": "// x\n" }] })
    };
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "renameProvider": true }),
            ..Scenario::default()
        }
        .reply(
            "textDocument/rename",
            Reply::result(json!({ "documentChanges": [
                text(&main), text(&theirs), text(&mine), text(&closed),
            ]})),
        ),
    );
    let mut a = env.window();
    let mut b = env.window();
    let doc = ready(&env, &mut a, "src/main.rs", "Rust", "fn main() {}\n").await;
    a.open(&env, &mine, "Rust", "pub fn old2() {}\n").await;
    // 창 B 가 연 채 고친다(미저장) — 서버의 `theirs.rs` 텍스트는 B 의 버퍼다.
    let other = doc_id(&b.open(&env, &theirs, "Rust", "pub fn old() {}\n").await);
    b.call(
        "lsp.document.change",
        json!({ "docId": other, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "added\n" }] }),
    )
    .await
    .unwrap();
    let answer = a
        .call(
            "lsp.rename",
            json!({ "docId": doc, "version": 0, "offset": 3, "newName": "start" }),
        )
        .await
        .unwrap();
    let marks: Vec<(String, bool)> = answer["edit"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let path = Path::new(c["path"].as_str().unwrap());
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                c["foreign"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        marks,
        [
            ("main.rs".to_owned(), false),
            ("theirs.rs".to_owned(), true),
            ("mine.rs".to_owned(), false),
            ("closed.rs".to_owned(), false),
        ]
    );
}

/// 같은 서버 id 의 두 인스턴스(형제 Cargo 작업 공간 둘): 한 인스턴스의 명령이 도는 동안 다른 인스턴스가 건 편집은 그 흐름의 창에
/// 가지 않고 `applied: false` 다(리뷰 M3 — 흐름은 인스턴스 열쇠 전체로 맞춘다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_from_another_instance_of_the_same_server_is_not_applied() {
    let env = Env::new(EnvOptions::default());
    env.file("a/Cargo.toml", "[package]\nname = \"a\"\n");
    env.file("b/Cargo.toml", "[package]\nname = \"b\"\n");
    let a_main = env.root.join("a/src/main.rs");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({
                "textDocumentSync": 2, "codeActionProvider": true,
                "executeCommandProvider": { "commands": ["fix.it"] },
            }),
            ..Scenario::default()
        }
        // 명령을 받은 인스턴스(a)가 건다.
        .on(
            "workspace/executeCommand",
            vec![apply_edit_request(&a_main)],
        )
        .reply(
            "workspace/executeCommand",
            Reply::result(json!(null)).delayed(3000),
        )
        // 문서가 바뀐 인스턴스(b)가 건다.
        .on("textDocument/didChange", vec![apply_edit_request(&a_main)]),
    );
    let mut win = env.window();
    win.call(
        "events.subscribe",
        json!({ "projectId": env.id, "topics": ["lsp.applyEdit"] }),
    )
    .await
    .unwrap();
    let doc_a = doc_id(
        &win.open(
            &env,
            &env.file("a/src/main.rs", "fn main() {}\n"),
            "Rust",
            "fn main() {}\n",
        )
        .await,
    );
    let doc_b = doc_id(
        &win.open(
            &env,
            &env.file("b/src/main.rs", "fn main() {}\n"),
            "Rust",
            "fn main() {}\n",
        )
        .await,
    );
    let both = eventually(Duration::from_secs(10), || {
        let states = env.states();
        states.len() == 2 && states.iter().all(|(_, s)| *s == LspInstanceState::Ready)
    })
    .await;
    assert!(both, "two rust-analyzer instances: {:?}", env.states());
    let call = win.send(
        "lsp.command.execute",
        json!({ "docId": doc_a, "flowId": "f1", "server": "rust-analyzer", "command": { "title": "Fix", "command": "fix.it" } }),
    );
    let event = next_event(&mut win, "lsp.applyEdit", Duration::from_secs(10))
        .await
        .expect("the window gets a's edit");
    win.call(
        "lsp.applyEdit.answer",
        json!({ "flowId": "f1", "requestId": event["requestId"], "applied": true }),
    )
    .await
    .unwrap();
    // 명령이 아직 도는 동안 b 의 문서를 고친다 → b 가 편집을 건다.
    win.call(
        "lsp.document.change",
        json!({ "docId": doc_b, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "y" }] }),
    )
    .await
    .unwrap();
    // 두 인스턴스가 같은 기록 파일에 쓴다 — 둘 다 첫 요청이 `fake-1` 이다. a 의 답(true) 다음에 b 의 답(false)이 온다.
    let answers = |record: &Path| -> Vec<Value> {
        received(record)
            .into_iter()
            .filter(|m| m["id"] == "fake-1" && m.get("method").is_none())
            .map(|m| m["result"].clone())
            .collect()
    };
    let two = eventually(Duration::from_secs(10), || answers(&record).len() == 2).await;
    assert!(two, "{:?}", answers(&record));
    assert_eq!(answers(&record)[0]["applied"], true);
    assert_eq!(answers(&record)[1]["applied"], false);
    assert!(
        next_event(&mut win, "lsp.applyEdit", Duration::from_millis(300))
            .await
            .is_none(),
        "b's edit must not reach the window"
    );
    win.response(call).await.unwrap();
}

/// resolve · 명령은 목록을 준 서버로만 간다 — 그 서버가 명령을 하지 않으면 5001 이고, 명령을 하는 다른 서버로 가지 않는다(리뷰 M4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commands_go_only_to_the_server_that_listed_the_action() {
    let env = Env::new(EnvOptions::default());
    // 정의 순서로 basedpyright 가 먼저다 — 명령만 하고 코드 액션은 ruff 가 준다.
    let pyright = env.fake(
        "basedpyright",
        Scenario {
            capabilities: json!({
                "textDocumentSync": 2,
                "executeCommandProvider": { "commands": ["fix.it"] },
            }),
            ..Scenario::default()
        }
        .reply(
            "workspace/executeCommand",
            Reply::result(json!("pyright ran it")),
        ),
    );
    env.fake(
        "ruff",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "codeActionProvider": true }),
            ..Scenario::default()
        }
        .reply(
            "textDocument/codeAction",
            Reply::result(
                json!([{ "title": "Fix", "command": { "title": "Fix", "command": "fix.it" } }]),
            ),
        ),
    );
    let mut win = env.window();
    let doc = ready(&env, &mut win, "app.py", "Python", "x = 1\n").await;
    env.wait_state("ruff", LspInstanceState::Ready).await;
    let list = win
        .call(
            "lsp.codeAction.list",
            json!({ "docId": doc, "version": 0, "from": 0, "to": 0 }),
        )
        .await
        .unwrap();
    assert_eq!(list["server"], "ruff");
    let action = list["actions"][0]["raw"].clone();
    let resolved = win
        .call(
            "lsp.codeAction.resolve",
            json!({ "docId": doc, "server": "ruff", "action": action }),
        )
        .await
        .unwrap();
    assert_eq!(resolved["server"], "ruff");
    let e = win
        .call(
            "lsp.command.execute",
            json!({ "docId": doc, "flowId": "f1", "server": "ruff", "command": resolved["command"] }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, pw_lsp::manager::code::NO_SERVER, "{e:?}");
    assert!(last_request(&pyright, "workspace/executeCommand").is_none());
    // 그 서버가 준 명령이면 그 서버로 간다.
    let ran = win
        .call(
            "lsp.command.execute",
            json!({ "docId": doc, "flowId": "f2", "server": "basedpyright", "command": { "title": "Fix", "command": "fix.it" } }),
        )
        .await
        .unwrap();
    assert_eq!(ran["result"], "pyright ran it");
}

/// 한 명령의 편집은 한 번에 하나다 — 앞 편집에 창이 답하기 전에 온 편집은 창에 넘기지 않고 곧바로 `applied: false`(리뷰 L4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_edit_while_the_first_waits_is_refused() {
    let env = Env::new(EnvOptions::default());
    let main = env.root.join("src/main.rs");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({
                "textDocumentSync": 2, "codeActionProvider": true,
                "executeCommandProvider": { "commands": ["fix.it"] },
            }),
            ..Scenario::default()
        }
        .on(
            "workspace/executeCommand",
            vec![apply_edit_request(&main), apply_edit_request(&main)],
        )
        .reply(
            "workspace/executeCommand",
            Reply::result(json!(null)).delayed(1500),
        ),
    );
    let mut win = env.window();
    win.call(
        "events.subscribe",
        json!({ "projectId": env.id, "topics": ["lsp.applyEdit"] }),
    )
    .await
    .unwrap();
    let doc = ready(&env, &mut win, "src/main.rs", "Rust", "fn main() {}\n").await;
    let call = win.send(
        "lsp.command.execute",
        json!({ "docId": doc, "flowId": "f1", "server": "rust-analyzer", "command": { "title": "Fix", "command": "fix.it" } }),
    );
    let event = next_event(&mut win, "lsp.applyEdit", Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(answer_to(&record, "fake-2").await["applied"], false);
    assert!(
        next_event(&mut win, "lsp.applyEdit", Duration::from_millis(300))
            .await
            .is_none()
    );
    win.call(
        "lsp.applyEdit.answer",
        json!({ "flowId": "f1", "requestId": event["requestId"], "applied": true }),
    )
    .await
    .unwrap();
    assert_eq!(answer_to(&record, "fake-1").await["applied"], true);
    win.response(call).await.unwrap();
}
