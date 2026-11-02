//! 포맷의 Host 쪽(15-m4-plan.md M4-13)을 가짜 서버로 끝까지: 옵션은 `tabSize` · `insertSpaces` 만, 선택이 있으면 범위 포맷(서버가
//! 할 때만 — 아니면 문서 전체), 결정 M4-D12 의 라우팅(정의 · 설정에서 `formatting` 을 끈 서버에는 가지 않는다), 시한(10 s)을 넘기면
//! 취소하고 `timedOut`, 문서 밖 · 겹침 결과는 전체 거절, 요청 사이의 편집은 `staleVersion`.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use pw_lsp::fake::{Reply, Scenario};
use pw_lsp::format::{FORMAT_TIMEOUT, code as format_code};
use pw_lsp::manager::code as manager_code;
use pw_lsp::methods::LspInstanceState;
use serde_json::{Value, json};

use common::env::{Conn, Env, EnvOptions};
use common::received;

fn doc_id(opened: &Value) -> String {
    opened["docId"].as_str().unwrap().to_owned()
}

fn range(l1: u32, c1: u32, l2: u32, c2: u32) -> Value {
    json!({ "start": { "line": l1, "character": c1 }, "end": { "line": l2, "character": c2 } })
}

fn edit(l1: u32, c1: u32, l2: u32, c2: u32, text: &str) -> Value {
    json!({ "range": range(l1, c1, l2, c2), "newText": text })
}

/// 가짜 서버가 받은 요청 `method` 의 `params` 들.
fn requests(record: &Path, method: &str) -> Vec<Value> {
    received(record)
        .into_iter()
        .filter(|m| m["method"] == method)
        .map(|m| m["params"].clone())
        .collect()
}

/// 증분 동기화 + `capabilities` 를 더한 가짜 서버.
fn server(capabilities: Value) -> Scenario {
    let mut caps = json!({ "textDocumentSync": 2 });
    for (k, v) in capabilities.as_object().unwrap() {
        caps[k] = v.clone();
    }
    Scenario {
        capabilities: caps,
        ..Scenario::default()
    }
}

/// 첫 줄은 한글(UTF-16 3 단위) — 오프셋은 웹뷰 단위다.
const RUST: &str = "// 한글\nfn  main( ){\nlet x=1;\n}\n";

async fn open_rust(env: &Env, win: &mut Conn) -> String {
    let doc = doc_id(
        &win.open(env, &env.file("src/main.rs", RUST), "Rust", RUST)
            .await,
    );
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    doc
}

fn format_params(doc: &str, range: Option<(u32, u32)>) -> Value {
    let mut p =
        json!({ "docId": doc, "version": 0, "options": { "tabSize": 4, "insertSpaces": true } });
    if let Some((from, to)) = range {
        p["range"] = json!({ "from": from, "to": to });
    }
    p
}

/// 문서 전체 포맷: 옵션은 `tabSize` · `insertSpaces` 만 가고(`trimTrailingWhitespace` 등은 없다), 서버의 편집이 그대로 온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_whole_document_is_formatted_with_only_the_two_options() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        server(json!({ "documentFormattingProvider": true })).reply(
            "textDocument/formatting",
            Reply::result(json!([edit(1, 2, 1, 4, " "), edit(2, 0, 2, 0, "    ")])),
        ),
    );
    let mut win = env.window();
    let doc = open_rust(&env, &mut win).await;
    let formatted = win
        .call("lsp.format", format_params(&doc, None))
        .await
        .unwrap();
    assert_eq!(
        formatted,
        json!({
            "server": "rust-analyzer",
            "range": false,
            "staleVersion": false,
            "edits": [edit(1, 2, 1, 4, " "), edit(2, 0, 2, 0, "    ")],
        })
    );
    let asked = requests(&record, "textDocument/formatting");
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0]["options"],
        json!({ "tabSize": 4, "insertSpaces": true })
    );
    assert!(asked[0].get("range").is_none());
}

/// 선택이 있고 서버가 범위 포맷을 하면 그 범위(선택과 같은 LSP 범위)로 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selection_is_formatted_as_that_range() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        server(
            json!({ "documentFormattingProvider": true, "documentRangeFormattingProvider": true }),
        )
        .reply(
            "textDocument/rangeFormatting",
            Reply::result(json!([edit(2, 5, 2, 5, " ")])),
        )
        .reply("textDocument/formatting", Reply::result(Value::Null)),
    );
    let mut win = env.window();
    let doc = open_rust(&env, &mut win).await;
    // 「let x=1;」 줄 전체: 오프셋 19 ~ 27(첫 줄 「// 한글\n」 은 6 단위, 둘째 줄 「fn  main( ){\n」 은 13 단위).
    let formatted = win
        .call("lsp.format", format_params(&doc, Some((19, 27))))
        .await
        .unwrap();
    assert_eq!(formatted["range"], true);
    assert_eq!(formatted["edits"], json!([edit(2, 5, 2, 5, " ")]));
    let asked = requests(&record, "textDocument/rangeFormatting");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["range"], range(2, 0, 2, 8));
    assert_eq!(
        asked[0]["options"],
        json!({ "tabSize": 4, "insertSpaces": true })
    );
    assert!(requests(&record, "textDocument/formatting").is_empty());
    // 빈 선택(from = to)은 선택이 아니다 — 문서 전체(이 서버의 `formatting` 은 `null` — 편집 없음).
    let whole = win
        .call("lsp.format", format_params(&doc, Some((19, 19))))
        .await
        .unwrap();
    assert_eq!(
        (whole["range"].clone(), whole["edits"].clone()),
        (json!(false), json!([]))
    );
    assert_eq!(requests(&record, "textDocument/rangeFormatting").len(), 1);
    assert_eq!(requests(&record, "textDocument/formatting").len(), 1);
}

/// 선택이 있어도 서버가 범위 포맷을 하지 않으면 문서 전체로 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selection_falls_back_to_the_whole_document_without_range_formatting() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        server(json!({ "documentFormattingProvider": true })).reply(
            "textDocument/formatting",
            Reply::result(json!([edit(1, 2, 1, 4, " ")])),
        ),
    );
    let mut win = env.window();
    let doc = open_rust(&env, &mut win).await;
    let formatted = win
        .call("lsp.format", format_params(&doc, Some((19, 27))))
        .await
        .unwrap();
    assert_eq!(formatted["range"], false);
    assert_eq!(requests(&record, "textDocument/formatting").len(), 1);
    assert!(requests(&record, "textDocument/rangeFormatting").is_empty());
}

/// Python 의 두 서버(basedpyright · ruff — 결정 M4-D3)가 모두 포맷 · 범위 포맷을 알리는 가짜.
fn python_servers(env: &Env) -> (std::path::PathBuf, std::path::PathBuf) {
    let both = || {
        server(
            json!({ "documentFormattingProvider": true, "documentRangeFormattingProvider": true }),
        )
        .reply(
            "textDocument/formatting",
            Reply::result(json!([edit(0, 0, 0, 0, "#")])),
        )
        .reply(
            "textDocument/rangeFormatting",
            Reply::result(json!([edit(0, 0, 0, 0, "#")])),
        )
    };
    (env.fake("basedpyright", both()), env.fake("ruff", both()))
}

async fn open_python(env: &Env, win: &mut Conn) -> String {
    let file = env.file("app.py", "");
    let doc = doc_id(&win.open(env, &file, "Python", "x  =  1\n").await);
    env.wait_state("basedpyright", LspInstanceState::Ready)
        .await;
    env.wait_state("ruff", LspInstanceState::Ready).await;
    doc
}

/// 결정 M4-D12 · D3: 두 서버가 모두 포맷을 알려도 정의가 `formatting` 을 끈 basedpyright(정의 순서로 첫 서버)에는 가지 않고
/// ruff 로 간다 — 문서 전체도 범위도.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn formatting_skips_a_server_that_turned_it_off() {
    let env = Env::new(EnvOptions::default());
    let (pyright, ruff) = python_servers(&env);
    let mut win = env.window();
    let doc = open_python(&env, &mut win).await;
    let whole = win
        .call("lsp.format", format_params(&doc, None))
        .await
        .unwrap();
    assert_eq!(whole["server"], "ruff");
    let ranged = win
        .call("lsp.format", format_params(&doc, Some((0, 7))))
        .await
        .unwrap();
    assert_eq!(
        (ranged["server"].clone(), ranged["range"].clone()),
        (json!("ruff"), json!(true))
    );
    assert!(requests(&pyright, "textDocument/formatting").is_empty());
    assert!(requests(&pyright, "textDocument/rangeFormatting").is_empty());
    assert_eq!(requests(&ruff, "textDocument/formatting").len(), 1);
    assert_eq!(requests(&ruff, "textDocument/rangeFormatting").len(), 1);
}

/// 설정 `[lsp.ruff.features] disable = ["formatting"]` 로 ruff 도 끄면 맡을 서버가 없다(5001) — 어느 서버도 묻지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn formatting_turned_off_in_settings_reaches_no_server() {
    let env = Env::new(EnvOptions::default());
    let (pyright, ruff) = python_servers(&env);
    let settings = env.paths.config.join("settings.toml");
    let mut text = std::fs::read_to_string(&settings).unwrap();
    text.push_str("[lsp.ruff.features]\ndisable = [\"formatting\"]\n");
    std::fs::write(&settings, text).unwrap();
    env.global.reload().unwrap();
    let mut win = env.window();
    let doc = open_python(&env, &mut win).await;
    for range in [None, Some((0, 7))] {
        let err = win
            .call("lsp.format", format_params(&doc, range))
            .await
            .unwrap_err();
        assert_eq!(err.code, manager_code::NO_SERVER, "{err:?}");
    }
    for record in [&pyright, &ruff] {
        assert!(requests(record, "textDocument/formatting").is_empty());
        assert!(requests(record, "textDocument/rangeFormatting").is_empty());
    }
}

/// 시한(10 s)을 넘기면 서버에 `$/cancelRequest` 를 보내고 5003 + `timedOut` 이다 — 웹뷰는 문서를 그대로 둔다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_formatter_slower_than_the_limit_is_cancelled_and_reported() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        server(json!({ "documentFormattingProvider": true })).reply(
            "textDocument/formatting",
            Reply::result(json!([edit(0, 0, 0, 0, "x")]))
                .delayed(FORMAT_TIMEOUT.as_millis() as u64 + 3_000),
        ),
    );
    let mut win = env.window();
    let doc = open_rust(&env, &mut win).await;
    let started = Instant::now();
    let err = win
        .call("lsp.format", format_params(&doc, None))
        .await
        .unwrap_err();
    let took = started.elapsed();
    assert_eq!(err.code, manager_code::SERVER_UNAVAILABLE, "{err:?}");
    assert_eq!(err.data.as_ref().unwrap()["timedOut"], true, "{err:?}");
    assert!(
        took >= FORMAT_TIMEOUT && took < FORMAT_TIMEOUT + Duration::from_secs(3),
        "{took:?}"
    );
    let asked = requests(&record, "textDocument/formatting");
    let cancelled = common::eventually(Duration::from_secs(5), || {
        requests(&record, "$/cancelRequest").len() == 1
    })
    .await;
    assert!(cancelled, "{:?}", received(&record));
    let id = received(&record)
        .into_iter()
        .find(|m| m["method"] == "textDocument/formatting")
        .unwrap()["id"]
        .clone();
    assert_eq!(requests(&record, "$/cancelRequest")[0]["id"], id);
    assert_eq!(asked.len(), 1);
}

/// 문서 밖 범위 · 겹치는 편집은 전체 거절(5130) — 웹뷰는 아무것도 넣지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edits_outside_the_document_or_overlapping_are_refused() {
    let env = Env::new(EnvOptions::default());
    let scenario = server(
        json!({ "documentFormattingProvider": true, "documentRangeFormattingProvider": true }),
    )
    .reply(
        "textDocument/formatting",
        Reply::result(json!([edit(1, 0, 1, 1, ""), edit(40, 0, 40, 0, "x")])),
    )
    .reply(
        "textDocument/rangeFormatting",
        Reply::result(json!([edit(1, 0, 1, 4, ""), edit(1, 2, 1, 6, "y")])),
    );
    env.fake("rust-analyzer", scenario);
    let mut win = env.window();
    let doc = open_rust(&env, &mut win).await;
    let outside = win
        .call("lsp.format", format_params(&doc, None))
        .await
        .unwrap_err();
    assert_eq!(outside.code, format_code::FORMAT_REFUSED, "{outside:?}");
    assert_eq!(outside.data.as_ref().unwrap()["reason"], "outside");
    let overlap = win
        .call("lsp.format", format_params(&doc, Some((19, 27))))
        .await
        .unwrap_err();
    assert_eq!(overlap.code, format_code::FORMAT_REFUSED, "{overlap:?}");
    assert_eq!(overlap.data.as_ref().unwrap()["reason"], "overlap");
}

/// 답이 오는 사이에 문서가 바뀌면 `staleVersion` 이다(웹뷰는 넣지 않는다). 낡은 답은 검사하지 않고 빈 편집이다 — 문서 밖 편집이
/// 섞여 있어도 「거절」(5130)이 아니라 「문서가 바뀌었다」 로 끝난다(M4-13 리뷰 L3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_while_formatting_marks_the_answer_stale() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        server(json!({ "documentFormattingProvider": true })).reply(
            "textDocument/formatting",
            Reply::result(json!([edit(1, 2, 1, 4, " "), edit(40, 0, 40, 0, "x")])).delayed(400),
        ),
    );
    let mut win = env.window();
    let doc = open_rust(&env, &mut win).await;
    let pending = win.send("lsp.format", format_params(&doc, None));
    tokio::time::sleep(Duration::from_millis(100)).await;
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "\n" }] }),
    )
    .await
    .unwrap();
    let stale = win.response(pending).await.unwrap();
    assert_eq!(stale["staleVersion"], true);
    assert_eq!(stale["edits"], json!([]));
}
