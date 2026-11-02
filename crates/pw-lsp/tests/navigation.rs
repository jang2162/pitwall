//! 내비게이션 기능의 Host 쪽(15-m4-plan.md M4-08): hover · 정의 · 사용처 RPC, 루트 밖 문서의 읽기 토큰(결정 M4-D13), 서버의
//! `window/showDocument` 를 가짜 서버로 끝까지.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use pw_core::Filter;
use pw_core::bus::Received;
use pw_lsp::fake::{Action, Reply, Scenario};
use pw_lsp::methods::LspInstanceState;
use pw_lsp::navigation::TOPIC_SHOW_DOCUMENT;
use pw_rpc::code;
use serde_json::{Value, json};

use common::env::{Conn, Env, EnvOptions};
use common::received;

const CAPS: &str = r#"{ "textDocumentSync": 2, "hoverProvider": true, "definitionProvider": true,
    "declarationProvider": true, "typeDefinitionProvider": true, "implementationProvider": true,
    "referencesProvider": true }"#;

fn caps() -> Value {
    serde_json::from_str(CAPS).unwrap()
}

fn uri(path: &Path) -> String {
    pw_lsp::uri::file_uri(path).unwrap()
}

fn range(line: u32, from: u32, to: u32) -> Value {
    json!({ "start": { "line": line, "character": from }, "end": { "line": line, "character": to } })
}

fn doc_id(opened: &Value) -> String {
    opened["docId"].as_str().unwrap().to_owned()
}

/// 서버가 준비된 창 하나와 그 문서(`src/main.rs`).
async fn ready(env: &Env, text: &str) -> (Conn, String) {
    let mut win = env.window();
    let doc = doc_id(
        &win.open(env, &env.file("src/main.rs", text), "Rust", text)
            .await,
    );
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    (win, doc)
}

async fn at(
    win: &mut Conn,
    method: &str,
    doc: &str,
    offset: u32,
) -> Result<Value, pw_rpc::RpcError> {
    win.call(method, json!({ "docId": doc, "offset": offset }))
        .await
}

// ---- hover -------------------------------------------------------------------------------------------------------------------

/// hover 의 세 꼴(MarkupContent · MarkedString · 그 배열)이 본문 하나가 되고, 범위는 문서의 UTF-16 오프셋이다. 서버가 보낸 글은
/// 그대로 싣는다(HTML 을 걷거나 해석하지 않는다 — 웹뷰가 HTML 을 끈 렌더로 그린다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hover_turns_every_form_into_one_body_with_document_offsets() {
    let env = Env::new(EnvOptions::default());
    let body = "**main** <img src=x onerror=alert(1)> [x](javascript:alert(1))";
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply(
            "textDocument/hover",
            Reply::result(json!({
                "contents": { "kind": "markdown", "value": body },
                "range": range(1, 4, 8),
            })),
        ),
    );
    let (mut win, doc) = ready(&env, "// 한글\nfn main() {}\n").await;
    let got = at(&mut win, "lsp.hover", &doc, 15).await.unwrap();
    assert_eq!(got["staleVersion"], json!(false));
    // 첫 줄은 「// 한글」 5 UTF-16 + 줄바꿈 → 둘째 줄의 4 는 오프셋 10.
    assert_eq!(
        got["hover"],
        json!({ "kind": "markdown", "value": body, "from": 10, "to": 14 })
    );

    for (contents, want) in [
        (
            json!("plain *md*"),
            json!({ "kind": "markdown", "value": "plain *md*" }),
        ),
        (
            json!([{ "language": "rust", "value": "fn main()" }, "docs"]),
            json!({ "kind": "markdown", "value": "```rust\nfn main()\n```\n\ndocs" }),
        ),
        (
            json!({ "kind": "plaintext", "value": "<b>not html</b>" }),
            json!({ "kind": "plaintext", "value": "<b>not html</b>" }),
        ),
    ] {
        let env = Env::new(EnvOptions::default());
        env.fake(
            "rust-analyzer",
            Scenario {
                capabilities: caps(),
                ..Scenario::default()
            }
            .reply(
                "textDocument/hover",
                Reply::result(json!({ "contents": contents })),
            ),
        );
        let (mut win, doc) = ready(&env, "fn main() {}").await;
        let got = at(&mut win, "lsp.hover", &doc, 3).await.unwrap();
        let mut want = want;
        want["from"] = Value::Null;
        want["to"] = Value::Null;
        assert_eq!(got["hover"], want);
    }
}

/// 빈 hover(`null` · 빈 글)는 본문이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_hover_has_no_body() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply(
            "textDocument/hover",
            Reply::result(json!({ "contents": "  " })),
        ),
    );
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    let got = at(&mut win, "lsp.hover", &doc, 3).await.unwrap();
    assert_eq!(got["hover"], Value::Null);
}

// ---- ① 정의 · 사용처 ----------------------------------------------------------------------------------------------------------

/// ① 정의의 답(Location · LocationLink 배열)이 절대 경로 · 1 부터 센 줄 · 열과 그 줄의 글이 되고, 루트 안이면 토큰이 없다. 사용처는
/// `includeDeclaration` 을 서버에 그대로 보낸다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn definitions_and_references_come_back_as_absolute_lines_with_previews() {
    let env = Env::new(EnvOptions::default());
    let lib = env.file(
        "src/lib.rs",
        "// lib\npub fn helper() {}\npub fn other() {}\n",
    );
    let lib_uri = uri(&lib);
    let main_text = "fn main() {\n    helper();\n}\n";
    let main = env.root.join("src/main.rs");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply(
            "textDocument/definition",
            Reply::result(json!([{
                "originSelectionRange": range(1, 4, 10),
                "targetUri": lib_uri,
                "targetRange": range(1, 0, 18),
                "targetSelectionRange": range(1, 7, 13),
            }])),
        )
        .reply(
            "textDocument/references",
            Reply::result(json!([
                { "uri": lib_uri, "range": range(1, 7, 13) },
                { "uri": uri(&main), "range": range(1, 4, 10) },
                { "uri": lib_uri, "range": range(2, 7, 12) },
            ])),
        ),
    );
    let (mut win, doc) = ready(&env, main_text).await;
    let got = at(&mut win, "lsp.definition", &doc, 18).await.unwrap();
    let lib_real = std::fs::canonicalize(&lib).unwrap();
    assert_eq!(
        got["locations"],
        json!([{
            "path": lib_real.display().to_string(), "token": null, "line": 2, "column": 8,
            "endLine": 2, "endColumn": 14, "preview": "pub fn helper() {}",
        }])
    );
    assert_eq!(got["server"], "rust-analyzer");

    // 열린 문서의 줄 미리보기는 Host 의 글이다(디스크가 아니다) — 둘째 줄 앞에 저장하지 않은 편집.
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 12, "to": 12, "insert": "/*a*/" }] }),
    )
    .await
    .unwrap();
    let got = win
        .call(
            "lsp.references",
            json!({ "docId": doc, "offset": 18, "includeDeclaration": false }),
        )
        .await
        .unwrap();
    let lines: Vec<(u64, &str)> = got["locations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["line"].as_u64().unwrap(), l["preview"].as_str().unwrap()))
        .collect();
    assert_eq!(
        lines,
        [
            (2, "pub fn helper() {}"),
            (2, "/*a*/    helper();"),
            (3, "pub fn other() {}"),
        ]
    );
    let sent = received(&record);
    let references = sent
        .iter()
        .find(|m| m["method"] == "textDocument/references")
        .unwrap();
    assert_eq!(
        references["params"]["context"],
        json!({ "includeDeclaration": false })
    );
    assert_eq!(
        references["params"]["position"],
        json!({ "line": 1, "character": 6 })
    );
}

/// 선언 · 타입 정의 · 구현은 각자의 LSP 메서드로 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declaration_type_definition_and_implementation_use_their_methods() {
    let env = Env::new(EnvOptions::default());
    let lib = env.file("src/lib.rs", "a\nb\nc\nd\n");
    let mut scenario = Scenario {
        capabilities: caps(),
        ..Scenario::default()
    };
    for (method, line) in [
        ("textDocument/declaration", 0),
        ("textDocument/typeDefinition", 1),
        ("textDocument/implementation", 2),
    ] {
        scenario = scenario.reply(
            method,
            Reply::result(json!({ "uri": uri(&lib), "range": range(line, 0, 1) })),
        );
    }
    env.fake("rust-analyzer", scenario);
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    for (rpc, want) in [
        ("lsp.declaration", "a"),
        ("lsp.typeDefinition", "b"),
        ("lsp.implementation", "c"),
    ] {
        let got = at(&mut win, rpc, &doc, 3).await.unwrap();
        assert_eq!(got["locations"][0]["preview"], want, "{rpc}");
    }
}

// ---- ② 루트 밖 문서 ------------------------------------------------------------------------------------------------------------

/// 루트 밖 파일을 가리키는 정의 결과(바깥 폴더 · 루트 안 링크가 밖을 가리킴).
fn outside_scenario(targets: &[(&Path, u32)]) -> Scenario {
    Scenario {
        capabilities: caps(),
        ..Scenario::default()
    }
    .reply(
        "textDocument/definition",
        Reply::result(Value::Array(
            targets
                .iter()
                .map(|(p, line)| json!({ "uri": uri(p), "range": range(*line, 0, 3) }))
                .collect(),
        )),
    )
    .reply(
        "textDocument/hover",
        Reply::result(json!({ "contents": "from the same server" })),
    )
}

fn outside_file(env: &Env, rel: &str, text: &str) -> std::path::PathBuf {
    let path = env.base.join("outside").join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    std::fs::canonicalize(path).unwrap()
}

/// ② 루트 밖 위치는 토큰과 함께 오고, 토큰으로만 읽힌다 — 토큰 문서 안의 hover 는 같은 서버로 간다. 토큰 없이(아무 경로를 토큰
/// 자리에) · 다른 창의 연결로는 읽을 수 없고, 토큰은 쓰기 RPC 에 쓰이지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outside_locations_are_read_only_through_this_windows_token() {
    let env = Env::new(EnvOptions::default());
    let vec_rs = outside_file(&env, "std/vec.rs", "pub struct Vec;\n");
    let record = env.fake("rust-analyzer", outside_scenario(&[(&vec_rs, 0)]));
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    let got = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
    let loc = &got["locations"][0];
    assert_eq!(loc["path"], vec_rs.display().to_string());
    assert_eq!(loc["preview"], "pub struct Vec;");
    let token = loc["token"]
        .as_str()
        .expect("an outside location carries a token")
        .to_owned();
    // 같은 위치를 다시 받으면 같은 토큰이다.
    let again = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
    assert_eq!(again["locations"][0]["token"], token);

    let read = win
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap();
    assert_eq!(read["text"], "pub struct Vec;\n");
    assert_eq!(read["server"], "rust-analyzer");
    assert_eq!(read["path"], vec_rs.display().to_string());

    // 그 문서 안의 hover 는 같은 서버로(문서 없이 — URI 는 루트 밖 파일).
    let hover = win
        .call("lsp.hover", json!({ "token": token, "offset": 4 }))
        .await
        .unwrap();
    assert_eq!(hover["hover"]["value"], "from the same server");
    let asked = received(&record)
        .into_iter()
        .rfind(|m| m["method"] == "textDocument/hover")
        .unwrap();
    assert_eq!(asked["params"]["textDocument"]["uri"], uri(&vec_rs));

    // 토큰 없이: 아무 경로 · 지어낸 토큰은 not found.
    for bogus in [vec_rs.display().to_string(), "0".repeat(32), uri(&vec_rs)] {
        let e = win
            .call("lsp.document.read", json!({ "token": bogus }))
            .await
            .unwrap_err();
        assert_eq!(e.code, code::NOT_FOUND, "{bogus}");
    }
    // 다른 창(같은 프로젝트의 다른 연결, 같은 인스턴스에 문서를 열었다)은 이 창의 토큰을 읽지 못한다.
    let mut other = env.window();
    other
        .open(&env, &env.file("src/other.rs", ""), "Rust", "")
        .await;
    let e = other
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    let e = other
        .call("lsp.hover", json!({ "token": token, "offset": 0 }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    // 그 인스턴스에 문서를 연 적이 없는 창도 읽지 못한다(토큰의 연결 묶음과 인스턴스 허가가 함께 지킨다).
    let mut third = env.window();
    let e = third
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap_err();
    assert!(
        [code::NOT_FOUND, code::PERMISSION_DENIED].contains(&e.code),
        "{e:?}"
    );
    // 쓰기 길은 없다: 문서 RPC 는 토큰을 문서로 받지 않는다.
    let e = win
        .call(
            "lsp.document.change",
            json!({ "docId": token, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "x" }] }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    assert_eq!(
        std::fs::read_to_string(&vec_rs).unwrap(),
        "pub struct Vec;\n"
    );
}

/// ② 루트 안의 링크가 루트 밖을 가리키면 루트 밖이다(토큰). 토큰을 만든 뒤 그 링크가 다른 파일로 바뀌면 읽기를 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_out_of_the_root_is_outside_and_a_retargeted_link_is_refused() {
    let env = Env::new(EnvOptions::default());
    let target = outside_file(&env, "secret.rs", "pub fn secret() {}\n");
    let other = outside_file(&env, "other.rs", "pub fn other() {}\n");
    let link = env.root.join("src/linked.rs");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    env.fake("rust-analyzer", outside_scenario(&[(&link, 0)]));
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    let got = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
    let token = got["locations"][0]["token"]
        .as_str()
        .expect("a link out of the root is outside")
        .to_owned();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&other, &link).unwrap();
    let e = win
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
}

/// ② FIFO 를 가리키는 위치의 토큰은 읽기를 거절한다 — 멈추지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_for_a_fifo_is_refused_without_blocking() {
    let env = Env::new(EnvOptions::default());
    let dir = env.base.join("outside");
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("pipe.rs");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let fifo = std::fs::canonicalize(fifo).unwrap();
    let folder = std::fs::canonicalize(&dir).unwrap();
    env.fake(
        "rust-analyzer",
        outside_scenario(&[(&fifo, 0), (&folder, 0)]),
    );
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    let got = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
    for loc in got["locations"].as_array().unwrap() {
        assert_eq!(loc["preview"], Value::Null);
        let token = loc["token"].as_str().unwrap();
        let read = tokio::time::timeout(
            Duration::from_secs(3),
            win.call("lsp.document.read", json!({ "token": token })),
        )
        .await
        .expect("reading a FIFO or a folder must not block");
        let e = read.unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{loc}");
    }
}

/// ② 신뢰를 풀면 그 프로젝트의 토큰은 읽히지 않고, 토큰을 준 인스턴스가 멈춰도(다른 서버로는 가지 않는다) 읽히지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokens_die_with_trust_and_with_their_instance() {
    for revoke in [true, false] {
        let env = Env::new(EnvOptions {
            // 신뢰 해제가 서버를 끝내지 않게 — 떠 있는 서버가 있어도 토큰이 신뢰를 다시 보는지 본다.
            follow: false,
            ..EnvOptions::default()
        });
        let file = outside_file(&env, "lib.rs", "x\n");
        env.fake("rust-analyzer", outside_scenario(&[(&file, 0)]));
        let (mut win, doc) = ready(&env, "fn main() {}").await;
        let got = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
        let token = got["locations"][0]["token"].as_str().unwrap().to_owned();
        win.call("lsp.document.read", json!({ "token": token }))
            .await
            .unwrap();
        if revoke {
            env.projects.set_trust(&env.id, false).unwrap();
            assert_eq!(env.state_of("rust-analyzer"), Some(LspInstanceState::Ready));
        } else {
            let root = env.manager.status(&env.id).instances[0].root.clone();
            win.call(
                "lsp.server.stop",
                json!({ "projectId": env.id, "server": "rust-analyzer", "root": root }),
            )
            .await
            .unwrap();
        }
        for (method, params) in [
            ("lsp.document.read", json!({ "token": token })),
            ("lsp.hover", json!({ "token": token, "offset": 0 })),
        ] {
            let e = win.call(method, params).await.unwrap_err();
            assert!(
                [code::PERMISSION_DENIED, 5001].contains(&e.code),
                "revoke={revoke} {method}: {e:?}"
            );
        }
    }
}

/// ② `jdt://` 는 디스크로 풀지 않고 그 인스턴스의 `java/classFileContents` 로만 읽는다. 그 밖의 스킴은 읽지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jdt_documents_come_from_their_server_and_other_schemes_are_not_read() {
    let env = Env::new(EnvOptions::default());
    let jdt = "jdt://contents/java.base/java.util/ArrayList.class?=x";
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: caps(),
            ..Scenario::default()
        }
        .reply(
            "textDocument/definition",
            Reply::result(json!([
                { "uri": jdt, "range": range(3, 0, 5) },
                { "uri": "untitled:Untitled-1", "range": range(0, 0, 1) },
            ])),
        )
        .reply(
            "java/classFileContents",
            Reply::result(json!("package java.util;\npublic class ArrayList {}\n")),
        ),
    );
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    let got = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
    assert_eq!(got["locations"][0]["path"], jdt);
    let token = got["locations"][0]["token"].as_str().unwrap();
    let read = win
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap();
    assert_eq!(
        read["text"],
        "package java.util;\npublic class ArrayList {}\n"
    );
    assert_eq!(read["path"], jdt);
    let asked: Vec<Value> = received(&record)
        .into_iter()
        .filter(|m| m["method"] == "java/classFileContents")
        .collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["params"], json!({ "uri": jdt }));
    let untitled = got["locations"][1]["token"].as_str().unwrap();
    let e = win
        .call("lsp.document.read", json!({ "token": untitled }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

/// 연결이 닫히면 그 연결의 토큰이 지워진다 — 같은 창이 새로 붙어도(새 연결) 옛 토큰은 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_connection_drops_its_tokens() {
    let env = Env::new(EnvOptions::default());
    let file = outside_file(&env, "lib.rs", "x\n");
    env.fake("rust-analyzer", outside_scenario(&[(&file, 0)]));
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    let got = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
    let token = got["locations"][0]["token"].as_str().unwrap().to_owned();
    drop(win);
    let mut next = env.window();
    next.open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let e = next
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
}

// ---- ③ window/showDocument ----------------------------------------------------------------------------------------------------

/// hover 에 답한 뒤 `window/showDocument` 를 `params` 로 건다.
fn show_after_hover(params: Vec<Value>) -> Scenario {
    Scenario {
        capabilities: caps(),
        ..Scenario::default()
    }
    .reply(
        "textDocument/hover",
        Reply {
            result: Some(Value::Null),
            then: params
                .into_iter()
                .map(|params| Action::Request {
                    method: "window/showDocument".into(),
                    params,
                })
                .collect(),
            ..Reply::default()
        },
    )
}

/// 가짜 서버가 받은 `window/showDocument` 의 답들(보낸 차례).
async fn show_answers(record: &Path, count: usize) -> Vec<Value> {
    let ok = common::eventually(Duration::from_secs(5), || {
        received(record)
            .iter()
            .filter(|m| m.get("method").is_none() && m.get("id").is_some())
            .count()
            >= count
    })
    .await;
    assert!(
        ok,
        "the client did not answer {count} showDocument requests"
    );
    received(record)
        .into_iter()
        .filter(|m| {
            m.get("method").is_none() && m["id"].as_str().is_some_and(|id| id.starts_with("fake-"))
        })
        .map(|m| m["result"]["success"].clone())
        .collect()
}

/// ③ 루트 안 파일만 받아 그 창에 이벤트를 낸다. 응답에 없던 루트 밖 경로 · `..` 로 벗어난 경로 · 밖을 가리키는 링크 · `external` ·
/// 다른 스킴은 `false` 이고 이벤트가 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_document_opens_only_files_inside_the_root() {
    let env = Env::new(EnvOptions::default());
    let inside = env.file("src/lib.rs", "a\nb\n");
    let outside = outside_file(&env, "secret.rs", "s\n");
    let link = env.root.join("src/out.rs");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let escaping = format!("{}/src/../../outside/secret.rs", uri(&env.root));
    let requests = vec![
        json!({ "uri": uri(&outside) }),
        json!({ "uri": escaping }),
        json!({ "uri": uri(&link) }),
        json!({ "uri": uri(&inside), "external": true }),
        json!({ "uri": "https://example.com/" , "external": true }),
        json!({ "uri": "jdt://contents/x" }),
        json!({ "uri": uri(&inside), "takeFocus": true, "selection": range(1, 0, 1) }),
    ];
    let count = requests.len();
    let record = env.fake("rust-analyzer", show_after_hover(requests));
    let mut events = env
        .bus
        .subscribe(Filter::all().topics([TOPIC_SHOW_DOCUMENT]));
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    at(&mut win, "lsp.hover", &doc, 0).await.unwrap();
    assert_eq!(
        show_answers(&record, count).await,
        [false, false, false, false, false, false, true]
    );
    let Ok(Some(Received::Event(event))) =
        tokio::time::timeout(Duration::from_secs(5), events.recv()).await
    else {
        panic!("no showDocument event");
    };
    assert_eq!(event.scope, pw_core::bus::Scope::Project(env.id.clone()));
    let real = std::fs::canonicalize(&inside).unwrap();
    assert_eq!(
        event.payload,
        json!({ "path": real.display().to_string(), "worktree": null, "line": 2, "column": 1,
                "takeFocus": true, "server": "rust-analyzer" })
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), events.recv())
            .await
            .is_err(),
        "only the file inside the root opens"
    );
}

/// ③ 미신뢰 프로젝트에서는(떠 있던 서버가 남아 있어도) 받지 않고 이벤트를 내지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_document_is_refused_while_untrusted() {
    let env = Env::new(EnvOptions {
        follow: false,
        ..EnvOptions::default()
    });
    let inside = env.file("src/lib.rs", "a\n");
    let record = env.fake(
        "rust-analyzer",
        show_after_hover(vec![json!({ "uri": uri(&inside) })]),
    );
    let mut events = env
        .bus
        .subscribe(Filter::all().topics([TOPIC_SHOW_DOCUMENT]));
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    // 신뢰 해제를 따르지 않는 관리자라 서버가 떠 있고 hover 도 간다 — 그 뒤의 showDocument 를 처리기가 거절해야 한다.
    env.projects.set_trust(&env.id, false).unwrap();
    at(&mut win, "lsp.hover", &doc, 0).await.unwrap();
    assert_eq!(show_answers(&record, 1).await, [false]);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), events.recv())
            .await
            .is_err()
    );
}

/// ② 토큰 문서는 편집 등급 상한(5 MiB)까지만 읽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_document_over_the_edit_tier_is_refused() {
    let env = Env::new(EnvOptions::default());
    let big = outside_file(
        &env,
        "big.rs",
        &"a".repeat(usize::try_from(pw_lsp::navigation::READ_LIMIT).unwrap() + 1),
    );
    let fits = outside_file(
        &env,
        "fits.rs",
        &"a".repeat(usize::try_from(pw_lsp::navigation::READ_LIMIT).unwrap()),
    );
    env.fake("rust-analyzer", outside_scenario(&[(&big, 0), (&fits, 0)]));
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    let got = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
    let token = |i: usize| got["locations"][i]["token"].as_str().unwrap().to_owned();
    let e = win
        .call("lsp.document.read", json!({ "token": token(0) }))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert_eq!(e.data.unwrap()["reason"], "tooLarge");
    let read = win
        .call("lsp.document.read", json!({ "token": token(1) }))
        .await
        .unwrap();
    assert_eq!(
        read["text"].as_str().unwrap().len() as u64,
        pw_lsp::navigation::READ_LIMIT
    );
}

/// ② 토큰을 준 인스턴스가 다시 뜨는 중(`starting`)이면 토큰을 쓰지 못한다 — 한 번 읽어 둔 글도 내주지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_waits_for_its_restarting_instance() {
    let env = Env::new(EnvOptions::default());
    let file = outside_file(&env, "lib.rs", "x\n");
    env.fake(
        "rust-analyzer",
        outside_scenario(&[(&file, 0)]).reply(
            "initialize",
            Reply::result(json!({ "capabilities": caps() })).delayed(1500),
        ),
    );
    let (mut win, doc) = ready(&env, "fn main() {}").await;
    let got = at(&mut win, "lsp.definition", &doc, 3).await.unwrap();
    let token = got["locations"][0]["token"].as_str().unwrap().to_owned();
    win.call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap();
    let root = env.manager.status(&env.id).instances[0].root.clone();
    win.call(
        "lsp.server.restart",
        json!({ "projectId": env.id, "server": "rust-analyzer", "root": root }),
    )
    .await
    .unwrap();
    // 새 서버가 떠서(프로세스가 있다) `initialize` 를 기다리는 동안 — 서버는 있지만 아직 준비되지 않았다.
    let spawned = common::eventually(Duration::from_secs(5), || {
        env.manager.spawn_count() == 2 && !env.manager.pids(&env.id).is_empty()
    })
    .await;
    assert!(spawned);
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::Starting)
    );
    let e = win
        .call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap_err();
    assert_eq!(e.code, 5001, "{e:?}");
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    win.call("lsp.document.read", json!({ "token": token }))
        .await
        .unwrap();
}
