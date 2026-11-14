//! MCP 툴 `diagnostics`(16-m5-plan.md M5-08 ①)의 가짜 서버 시험: 창이 띄운 서버가 발행한 진단을 MCP 세션의 연결
//! (`ConnectionScope::Agent`)이 줄 · 열 1 부터로 읽고, 심각도로 거르고, 쪽을 넘기고, 상대 경로로 고른다. 서버가 없으면 빈 서버
//! 요약과 빈 목록, 미신뢰는 빈 목록과 까닭. 툴은 서버를 띄우지 않고, 창의 연결은 툴을 부르지 못한다. 바탕은 tests/diagnostics.rs 와
//! 같다(내장 번들에 설정 `[lsp.<id>] path` 로 가짜 서버를 꽂는다).

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::env::{Conn, Env, EnvOptions};
use pw_core::Filter;
use pw_core::bus::{Received, Subscription};
use pw_lsp::diagnostics::{MESSAGE_LIMIT, TOPIC_DIAGNOSTICS};
use pw_lsp::fake::{Action, Scenario};
use pw_lsp::methods::LspInstanceState;
use pw_lsp::tools::RESPONSE_BUDGET;
use pw_lsp::uri::file_uri;
use pw_rpc::{ConnectionScope, code};
use serde_json::{Value, json};

const TOOL: &str = "lsp.tool.diagnostics";

fn uri(path: &Path) -> String {
    file_uri(path).unwrap()
}

/// 진단 하나(줄 `line`, 열 `from` ~ `to` — LSP 의 0 부터).
fn diag_at(line: u32, from: u32, to: u32, severity: u64, message: &str) -> Value {
    json!({
        "range": { "start": { "line": line, "character": from }, "end": { "line": line, "character": to } },
        "severity": severity,
        "message": message,
    })
}

fn diag(line: u32, severity: u64, message: &str) -> Value {
    diag_at(line, 0, 1, severity, message)
}

fn publish(uri: &str, diagnostics: Vec<Value>) -> Action {
    Action::Notify {
        method: "textDocument/publishDiagnostics".into(),
        params: json!({ "uri": uri, "version": 0, "diagnostics": diagnostics }),
    }
}

fn incremental() -> Value {
    json!({ "textDocumentSync": 2 })
}

fn subscribe(env: &Env) -> Subscription {
    env.bus.subscribe(Filter::all().topics([TOPIC_DIAGNOSTICS]))
}

/// `path` 의 다음 진단 이벤트를 기다린다(시한 10 s).
async fn published(events: &mut Subscription, path: &Path) {
    let path = path.to_string_lossy();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let got = tokio::time::timeout_at(deadline, events.recv())
            .await
            .unwrap_or_else(|_| panic!("no lsp.diagnostics for {path}"));
        if let Some(Received::Event(event)) = got
            && event.payload["path"] == path.as_ref()
        {
            return;
        }
    }
}

/// MCP 세션의 연결.
fn agent(env: &Env) -> Conn {
    env.connect(ConnectionScope::Agent)
}

/// 쪽 하나의 (경로, 메시지) 들.
fn messages(page: &Value) -> Vec<(String, String)> {
    page["files"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| {
            let path = f["path"].as_str().unwrap().to_owned();
            f["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |d| (path.clone(), d["message"].as_str().unwrap().to_owned()))
        })
        .collect()
}

/// `src/a.rs`(셋)와 `src/b.rs`(하나)에 진단을 발행하는 가짜 서버를 꽂고 창에서 두 파일을 연다. 두 파일의 이벤트까지 기다린다.
async fn two_files(env: &Env) -> (Conn, std::path::PathBuf, std::path::PathBuf) {
    let a = env.file("src/a.rs", "a\nb\nc\nd\n");
    let b = env.file("src/b.rs", "x\n");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![
                publish(
                    &uri(&a),
                    vec![
                        diag(2, 4, "hint"),
                        diag(1, 2, "warning"),
                        diag_at(3, 4, 9, 1, "error"),
                    ],
                ),
                publish(&uri(&b), vec![diag(0, 1, "b error")]),
            ],
        ),
    );
    let mut events = subscribe(env);
    let mut win = env.window();
    win.open(env, &a, "Rust", "a\nb\nc\nd\n").await;
    published(&mut events, &a).await;
    published(&mut events, &b).await;
    (win, a, b)
}

/// 완료 기준: 가짜 서버가 발행한 진단 셋을 `diagnostics` 가 줄 · 열 1 부터로 돌려준다. 파일은 경로 차례, 파일 안은 심각도 차례.
/// 서버 요약에 그 서버가 준비된 것으로 있고, 까닭은 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_come_back_one_based_with_the_server_summary() {
    let env = Env::new(EnvOptions::default());
    let (_win, a, b) = two_files(&env).await;
    let mut agent = agent(&env);
    let page = agent
        .call(TOOL, json!({ "projectId": env.id }))
        .await
        .unwrap();
    assert_eq!(
        messages(&page),
        [
            (a.display().to_string(), "error".to_owned()),
            (a.display().to_string(), "warning".to_owned()),
            (a.display().to_string(), "hint".to_owned()),
            (b.display().to_string(), "b error".to_owned()),
        ]
    );
    let first = &page["files"][0]["diagnostics"][0];
    assert_eq!(
        (
            &first["line"],
            &first["column"],
            &first["endLine"],
            &first["endColumn"]
        ),
        (&json!(4), &json!(5), &json!(4), &json!(10)),
        "{first}"
    );
    assert_eq!(first["severity"], "error");
    assert_eq!(first["server"], "rust-analyzer");
    assert_eq!(page["files"][0]["diagnostics"][1]["line"], 2);
    assert_eq!(page["nextCursor"], Value::Null);
    assert_eq!(page["truncated"], false);
    assert_eq!(page["unavailable"], Value::Null);
    let servers = page["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1, "{page}");
    assert_eq!(servers[0]["server"], "rust-analyzer");
    assert_eq!(servers[0]["state"]["kind"], "ready");
    // 서버만 되돌려 받는 값 · 받은 길은 싣지 않는다.
    assert!(first.get("data").is_none() && first.get("origin").is_none());
}

/// 완료 기준: `minSeverity` 필터 — 그 심각도와 그보다 심한 것만.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn min_severity_keeps_the_more_severe() {
    let env = Env::new(EnvOptions::default());
    let (_win, _a, _b) = two_files(&env).await;
    let mut agent = agent(&env);
    let page = agent
        .call(
            TOOL,
            json!({ "projectId": env.id, "minSeverity": "warning" }),
        )
        .await
        .unwrap();
    let got: Vec<String> = messages(&page).into_iter().map(|(_, m)| m).collect();
    assert_eq!(got, ["error", "warning", "b error"]);
    let page = agent
        .call(TOOL, json!({ "projectId": env.id, "minSeverity": "error" }))
        .await
        .unwrap();
    let got: Vec<String> = messages(&page).into_iter().map(|(_, m)| m).collect();
    assert_eq!(got, ["error", "b error"]);
}

/// 완료 기준: 쪽 넘기기 — `limit` 만큼씩, 한 파일이 두 쪽에 걸쳐도 빠짐 · 겹침 없이 이어지고 마지막 쪽의 `nextCursor` 는 `null`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pages_follow_the_cursor_to_the_end() {
    let env = Env::new(EnvOptions::default());
    let (_win, _a, _b) = two_files(&env).await;
    let mut agent = agent(&env);
    let mut cursor = Value::Null;
    let mut pages = Vec::new();
    loop {
        let page = agent
            .call(
                TOOL,
                json!({ "projectId": env.id, "limit": 3, "cursor": cursor }),
            )
            .await
            .unwrap();
        pages.push(
            messages(&page)
                .into_iter()
                .map(|(_, m)| m)
                .collect::<Vec<_>>(),
        );
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
        assert!(pages.len() < 10, "{pages:?}");
    }
    assert_eq!(
        pages,
        [vec!["error", "warning", "hint"], vec!["b error"]],
        "{pages:?}"
    );
    let page = agent
        .call(TOOL, json!({ "projectId": env.id, "limit": 2 }))
        .await
        .unwrap();
    let next = agent
        .call(
            TOOL,
            json!({ "projectId": env.id, "limit": 2, "cursor": page["nextCursor"] }),
        )
        .await
        .unwrap();
    let got: Vec<String> = messages(&next).into_iter().map(|(_, m)| m).collect();
    assert_eq!(got, ["hint", "b error"]);
    assert_eq!(next["nextCursor"], Value::Null);
}

/// 완료 기준: `path` 는 프로젝트 루트 기준 상대 경로도 받는다(`.` · `..` 를 접는다). 절대 경로도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relative_path_picks_one_file() {
    let env = Env::new(EnvOptions::default());
    let (_win, _a, b) = two_files(&env).await;
    let mut agent = agent(&env);
    for path in [
        "src/b.rs".to_owned(),
        "./src/../src/b.rs".to_owned(),
        b.display().to_string(),
    ] {
        let page = agent
            .call(TOOL, json!({ "projectId": env.id, "path": path }))
            .await
            .unwrap();
        assert_eq!(
            messages(&page),
            [(b.display().to_string(), "b error".to_owned())],
            "{path}"
        );
    }
    let page = agent
        .call(TOOL, json!({ "projectId": env.id, "path": "src/none.rs" }))
        .await
        .unwrap();
    assert!(messages(&page).is_empty(), "{page}");
}

/// 완료 기준: 서버가 없으면(그 프로젝트 창이 아무것도 열지 않았다) 빈 서버 요약과 빈 목록이다. 툴은 서버를 띄우지 않는다 — 가짜
/// 서버를 꽂아 두어도 띄운 수가 0 이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_server_the_summary_and_list_are_empty_and_none_is_started() {
    let env = Env::new(EnvOptions::default());
    env.file("src/a.rs", "a\n");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        },
    );
    let mut agent = agent(&env);
    let page = agent
        .call(TOOL, json!({ "projectId": env.id, "path": "src/a.rs" }))
        .await
        .unwrap();
    assert_eq!(
        page,
        json!({ "servers": [], "files": [], "nextCursor": null, "truncated": false, "unavailable": null })
    );
    assert_eq!(env.manager.spawn_count(), 0);
}

/// 완료 기준: 미신뢰 프로젝트는 빈 목록과 까닭. 서버 요약은 창이 연 서버의 `untrusted` 를 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_gives_an_empty_list_and_the_reason() {
    let env = Env::new(EnvOptions {
        trusted: false,
        ..EnvOptions::default()
    });
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), vec![diag(0, 1, "error")])],
        ),
    );
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::Untrusted)
    );
    let mut agent = agent(&env);
    let page = agent
        .call(TOOL, json!({ "projectId": env.id }))
        .await
        .unwrap();
    assert_eq!(page["files"], json!([]));
    assert_eq!(page["unavailable"]["reason"], "untrusted", "{page}");
    assert_eq!(page["servers"][0]["state"]["kind"], "untrusted", "{page}");
}

/// 관문은 `agent_grant` 다: 창 · Host 의 연결은 툴을 부르지 못하고(`notAgentSession`), 없는 프로젝트는 그 관문의 오류 그대로
/// (invalid params). 다른 프로젝트의 진단은 섞이지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_an_mcp_session_calls_the_tool() {
    let env = Env::new(EnvOptions::default());
    let (mut win, _a, _b) = two_files(&env).await;
    let e = win
        .call(TOOL, json!({ "projectId": env.id }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
    assert_eq!(e.data.unwrap()["reason"], "notAgentSession");
    let mut host = env.connect(ConnectionScope::Host);
    let e = host
        .call(TOOL, json!({ "projectId": env.id }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
    let mut agent = agent(&env);
    let e = agent
        .call(TOOL, json!({ "projectId": "nope" }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    let other = agent
        .call(TOOL, json!({ "projectId": env.other }))
        .await
        .unwrap();
    assert_eq!(other["files"], json!([]));
    assert_eq!(other["servers"], json!([]));
    for bad in [json!(0), json!(5001)] {
        let e = agent
            .call(TOOL, json!({ "projectId": env.id, "limit": bad }))
            .await
            .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{bad}");
    }
}

/// 응답 예산: 긴 진단이 많으면 `limit` 보다 먼저 예산에서 끊고, 그 자리부터 다음 쪽이 이어진다(빠짐 · 겹침 없이).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_stops_at_the_response_budget_and_resumes_there() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    let long = "가".repeat(MESSAGE_LIMIT / 3 - 1);
    let count = 150;
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(
                &uri(&file),
                (0..count)
                    .map(|i| diag(i, 1, &format!("{i:03}{long}")))
                    .collect(),
            )],
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    published(&mut events, &file).await;
    let mut agent = agent(&env);
    let mut cursor = Value::Null;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = agent
            .call(
                TOOL,
                json!({ "projectId": env.id, "limit": 5000, "cursor": cursor }),
            )
            .await
            .unwrap();
        let size = serde_json::to_vec(&page).unwrap().len();
        assert!(size <= RESPONSE_BUDGET + 1024, "{size}");
        seen.extend(messages(&page).into_iter().map(|(_, m)| m[..3].to_owned()));
        pages += 1;
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
        assert!(pages < 20);
    }
    assert!(pages > 1, "the budget never cut a page");
    let want: Vec<String> = (0..count).map(|i| format!("{i:03}")).collect();
    assert_eq!(seen, want);
}
