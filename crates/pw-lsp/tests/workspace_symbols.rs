//! 작업 공간 심볼의 Host 쪽(15-m4-plan.md M4-16): `lsp.workspaceSymbol` 을 가짜 서버로 끝까지 — 물을 서버(이 창이 문서를 연 떠
//! 있는 서버만), 위치 · 종류 · 상한, 루트 밖 위치의 읽기 토큰(결정 M4-D13 — 내비게이션의 토큰 표).

#![cfg(unix)]

mod common;

use std::fs;
use std::path::Path;

use pw_lsp::fake::{Reply, Scenario};
use pw_lsp::methods::LspInstanceState;
use pw_rpc::ConnectionScope;
use serde_json::{Value, json};

use common::env::{Conn, Env, EnvOptions};
use common::{eventually, received};

fn caps() -> Value {
    json!({ "textDocumentSync": 2, "workspaceSymbolProvider": true })
}

fn uri(path: &Path) -> String {
    pw_lsp::uri::file_uri(path).unwrap()
}

fn symbol(name: &str, kind: u32, container: Option<&str>, uri: &str, line: u32) -> Value {
    json!({
        "name": name,
        "kind": kind,
        "containerName": container,
        "location": {
            "uri": uri,
            "range": { "start": { "line": line, "character": 4 }, "end": { "line": line, "character": 9 } }
        }
    })
}

/// 서버가 준비된 창 하나(그 창이 `src/main.rs` 를 연다).
async fn ready(env: &Env) -> Conn {
    let mut win = env.window();
    let text = "fn main() {}\n";
    win.open(env, &env.file("src/main.rs", text), "Rust", text)
        .await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    win
}

async fn search(
    win: &mut Conn,
    env: &Env,
    query: &str,
    extra: Value,
) -> Result<Value, pw_rpc::RpcError> {
    let mut params = json!({ "projectId": env.id, "query": query });
    if let (Value::Object(p), Value::Object(e)) = (&mut params, extra) {
        p.extend(e);
    }
    win.call("lsp.workspaceSymbol", params).await
}

/// 그 서버가 받은 `workspace/symbol` 요청 수.
fn asked(record: &Path) -> usize {
    received(record)
        .iter()
        .filter(|m| m["method"] == "workspace/symbol")
        .count()
}

/// 루트 안 위치는 실제 경로 · 루트 기준 상대 경로 · 1 부터 센 줄 · 열(토큰 없음)이고, 루트 밖 파일 · `jdt://` 는 이 창의 읽기 토큰과
/// 함께 온다 — 그 토큰으로 `lsp.document.read` 가 읽히고(내비게이션과 같은 표), 다른 창은 같은 토큰을 읽지 못한다. 서버가 준
/// 이름 · 컨테이너는 글자 그대로(상한에서 글자 경계로 자른다), 종류는 LSP 숫자(범위 밖이면 0)이고, 질의가 서버에 그대로 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symbols_come_back_with_paths_kinds_and_read_tokens_for_outside_locations() {
    let env = Env::new(EnvOptions::default());
    let inside = env.file("src/lib.rs", "pub struct Widget;\n");
    let outside = env.base.join("registry/dep/src/lib.rs");
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::write(&outside, "pub struct Outside;\n").unwrap();
    let long = "가".repeat(200);
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply(
            "workspace/symbol",
            Reply::result(json!([
                symbol("<b>Widget</b>", 23, Some("crate::ui"), &uri(&inside), 0),
                symbol("Outside", 5, None, &uri(&outside), 0),
                symbol("Jdt", 99, None, "jdt://contents/x/Y.class", 3),
                symbol(&long, 12, Some(&long), &uri(&inside), 2),
                { "kind": 5, "location": { "uri": uri(&inside) } },
            ])),
        ),
    );
    let mut win = ready(&env).await;
    let got = search(&mut win, &env, "wid", json!({})).await.unwrap();
    assert_eq!(got["servers"], json!(["rust-analyzer"]));
    assert_eq!(got["failed"], json!([]));
    assert_eq!(got["truncated"], json!(false));
    let symbols = got["symbols"].as_array().unwrap();
    assert_eq!(
        symbols.len(),
        4,
        "the nameless item is dropped: {symbols:?}"
    );

    let first = &symbols[0];
    assert_eq!(first["name"], "<b>Widget</b>");
    assert_eq!(first["kind"], 23);
    assert_eq!(first["container"], "crate::ui");
    assert_eq!(first["server"], "rust-analyzer");
    assert_eq!(first["language"], "Rust");
    assert_eq!(first["relative"], "src/lib.rs");
    assert_eq!(
        first["location"],
        json!({
            "path": inside.display().to_string(), "token": null, "line": 1, "column": 5,
            "endLine": 1, "endColumn": 10, "preview": null,
        })
    );

    let out = &symbols[1];
    assert_eq!(out["relative"], Value::Null);
    assert_eq!(out["location"]["path"], outside.display().to_string());
    let token = out["location"]["token"].as_str().unwrap().to_owned();
    let read = win
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap();
    assert_eq!(read["text"], "pub struct Outside;\n");
    let mut other = env.window();
    other
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap_err();

    let jdt = &symbols[2];
    assert_eq!(jdt["kind"], 0);
    assert_eq!(jdt["location"]["path"], "jdt://contents/x/Y.class");
    assert!(jdt["location"]["token"].is_string());
    assert_eq!(jdt["location"]["line"], 4);

    let clipped = symbols[3]["name"].as_str().unwrap();
    assert_eq!(clipped, "가".repeat(256 / 3));
    assert_eq!(symbols[3]["container"].as_str().unwrap(), clipped);

    let sent: Vec<Value> = received(&record)
        .into_iter()
        .filter(|m| m["method"] == "workspace/symbol")
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["params"]["query"], "wid");
}

/// `kinds` 로 거르고(클래스로 이동 — 함수를 뺀다) `limit` 에서 자르면 `truncated`. 빈 질의는 서버에 묻지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kinds_filter_limit_truncates_and_an_empty_query_asks_nothing() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    let items: Vec<Value> = (0..30)
        .map(|i| {
            let kind = if i % 2 == 0 { 12 } else { 23 };
            symbol(&format!("s{i}"), kind, None, &uri(&file), i)
        })
        .collect();
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply("workspace/symbol", Reply::result(Value::Array(items))),
    );
    let mut win = ready(&env).await;

    let types = search(&mut win, &env, "s", json!({ "kinds": [5, 10, 11, 23, 26] }))
        .await
        .unwrap();
    let kinds: Vec<u64> = types["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_u64().unwrap())
        .collect();
    assert_eq!(kinds, vec![23; 15]);
    assert_eq!(types["truncated"], json!(false));

    let cut = search(&mut win, &env, "s", json!({ "limit": 10 }))
        .await
        .unwrap();
    assert_eq!(cut["symbols"].as_array().unwrap().len(), 10);
    assert_eq!(cut["truncated"], json!(true));
    assert_eq!(cut["symbols"][9]["name"], "s9");

    let before = asked(&record);
    let empty = search(&mut win, &env, "  ", json!({})).await.unwrap();
    assert_eq!(empty["symbols"], json!([]));
    assert_eq!(asked(&record), before);
}

/// 서버가 오류로 답하면 그 서버는 `failed` 에 들고 결과는 비어 있다(RPC 는 실패하지 않는다 — 웹뷰가 인덱스 결과를 그대로 둔다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_server_is_listed_and_the_call_still_answers() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply(
            "workspace/symbol",
            Reply {
                error: Some(json!({ "code": -32603, "message": "boom" })),
                ..Reply::default()
            },
        ),
    );
    let mut win = ready(&env).await;
    let got = search(&mut win, &env, "x", json!({})).await.unwrap();
    assert_eq!(got["failed"], json!(["rust-analyzer"]));
    assert_eq!(got["servers"], json!([]));
    assert_eq!(got["symbols"], json!([]));
}

/// 서버가 `workspace/symbol` 을 알리지 않았으면 묻지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_without_the_capability_is_not_asked() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2 }),
            ..Scenario::default()
        },
    );
    let mut win = ready(&env).await;
    let got = search(&mut win, &env, "x", json!({})).await.unwrap();
    assert_eq!(got["servers"], json!([]));
    assert_eq!(asked(&record), 0);
}

/// 다른 창(같은 프로젝트지만 문서를 열지 않은 연결)에는 그 서버가 나오지 않는다 — 서버에 묻지도 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_without_a_document_in_the_server_gets_nothing() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply("workspace/symbol", Reply::result(json!([]))),
    );
    let _win = ready(&env).await;
    let mut other = env.window();
    let got = search(&mut other, &env, "x", json!({})).await.unwrap();
    assert_eq!(got["servers"], json!([]));
    assert_eq!(asked(&record), 0);
}

/// 다른 프로젝트: 그 프로젝트 창의 연결이 이 프로젝트를 물으면 거절(연결 범위), 자기 프로젝트를 물으면 이 프로젝트의 서버는 나오지
/// 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_projects_window_never_sees_these_servers() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply("workspace/symbol", Reply::result(json!([]))),
    );
    let _win = ready(&env).await;
    env.projects.set_trust(&env.other, true).unwrap();
    let mut foreign = env.connect(ConnectionScope::Project(env.other.clone()));
    foreign
        .call(
            "lsp.workspaceSymbol",
            json!({ "projectId": env.id, "query": "x" }),
        )
        .await
        .unwrap_err();
    let own = foreign
        .call(
            "lsp.workspaceSymbol",
            json!({ "projectId": env.other, "query": "x" }),
        )
        .await
        .unwrap();
    assert_eq!(own["servers"], json!([]));
    assert_eq!(asked(&record), 0);
}

/// 미신뢰가 되면 untrusted 로 거절하고 서버에 묻지 않는다(서버도 끝난다 — 결정 M4-D8).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_is_refused_without_asking_the_server() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply("workspace/symbol", Reply::result(json!([]))),
    );
    let mut win = ready(&env).await;
    env.projects.set_trust(&env.id, false).unwrap();
    let e = search(&mut win, &env, "x", json!({})).await.unwrap_err();
    assert_eq!(e.data.as_ref().unwrap()["reason"], "untrusted", "{e:?}");
    assert_eq!(asked(&record), 0);
    // 서버가 끝날 때까지 기다려도 묻지 않는다.
    assert!(
        eventually(std::time::Duration::from_secs(10), || env
            .state_of("rust-analyzer")
            .is_none_or(|s| !s.is_running()))
        .await
    );
    search(&mut win, &env, "x", json!({})).await.unwrap_err();
    assert_eq!(asked(&record), 0);
}
