//! 문서 심볼의 실 서버 시험(15-m4-plan.md M4-10 live): 픽스처를 임시 폴더에 복사해 서버 관리자로 연 문서에
//! `lsp.symbols.document` 를 물으면 그 파일(`m4_10_outline.*`)의 함수 · 타입 이름이 트리(부모 색인)에 든다. 표지
//! (`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs` 머리 주석).

#![cfg(unix)]

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
mod live {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use pw_lsp::live::LiveServer;
    use pw_lsp::methods::LspInstanceState;
    use serde_json::{Value, json};

    use super::common;
    use super::common::env::{Env, EnvOptions};

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// `(이름, 부모 이름)` 이 모두 트리에 있는지 본다. 부모가 `None` 이면 맨 위다.
    pub async fn document_symbols(
        server: LiveServer,
        fixture: &str,
        file: &str,
        language: &str,
        want: &[(&str, Option<&str>)],
    ) {
        let env = Env::new(EnvOptions {
            // cargo · node 를 찾도록 시험 프로세스의 환경을 넘긴다.
            isolated_shell: false,
            ..EnvOptions::default()
        });
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        copy_dir(&from, &env.root);
        env.configure(
            server.server_id(),
            &server.program(),
            &server
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        );
        let path = env.root.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut win = env.window();
        let opened = win.open(&env, &path, language, &text).await;
        let doc = opened["docId"].as_str().unwrap().to_owned();
        let ready = common::eventually(Duration::from_secs(60), || {
            env.state_of(server.server_id()) == Some(LspInstanceState::Ready)
        })
        .await;
        assert!(ready, "{:?}", env.states());
        // 서버가 프로젝트를 읽는 동안은 빈 답일 수 있다 — 바라는 이름이 다 들 때까지(60 s) 다시 묻는다.
        let deadline = Instant::now() + Duration::from_secs(60);
        let (pairs, got) = loop {
            let got = win
                .call("lsp.symbols.document", json!({ "docId": doc }))
                .await
                .unwrap();
            let symbols = got["symbols"].as_array().cloned().unwrap_or_default();
            let name = |i: &Value| symbols[i.as_u64().unwrap() as usize]["name"].clone();
            let pairs: Vec<(Value, Value)> = symbols
                .iter()
                .map(|s| (s["name"].clone(), name_or_null(&s["parent"], &name)))
                .collect();
            let all = want
                .iter()
                .all(|(n, p)| pairs.contains(&(json!(n), p.map_or(Value::Null, |p| json!(p)))));
            if all || Instant::now() > deadline {
                break (pairs, got);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        eprintln!("{}: {pairs:?}", server.executable());
        for (n, p) in want {
            assert!(
                pairs.contains(&(json!(n), p.map_or(Value::Null, |p| json!(p)))),
                "{n} under {p:?}: {got}"
            );
        }
        assert_eq!(got["server"], server.server_id());
        assert_eq!(got["truncated"], false);
        // 범위는 문서 안이고 이름 자리는 범위 안이다.
        let len = text.encode_utf16().count() as u64;
        for s in got["symbols"].as_array().unwrap() {
            let at = |k: &str| s[k].as_u64().unwrap();
            assert!(at("from") <= at("selectionFrom"), "{s}");
            assert!(at("selectionTo") <= at("to") && at("to") <= len, "{s}");
        }
        win.session.close();
    }

    fn name_or_null(parent: &Value, name: &dyn Fn(&Value) -> Value) -> Value {
        if parent.is_null() {
            Value::Null
        } else {
            name(parent)
        }
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_document_symbols() {
    live::document_symbols(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        "src/m4_10_outline.rs",
        "Rust",
        &[
            ("OutlineBox", None),
            ("outline_width", Some("OutlineBox")),
            ("OutlineColor", None),
            ("OutlineRed", Some("OutlineColor")),
            ("outline_area", Some("impl OutlineBox")),
            ("outline_entry", None),
            ("outline_inner", None),
            ("outline_nested", Some("outline_inner")),
        ],
    )
    .await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_document_symbols() {
    live::document_symbols(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        "src/m4_10_outline.ts",
        "TypeScript",
        &[
            ("OutlineShape", None),
            ("OutlineBox", None),
            ("outlineArea", Some("OutlineBox")),
            ("OutlineColor", None),
            ("OutlineRed", Some("OutlineColor")),
            ("OutlineAlias", None),
            ("outlineEntry", None),
        ],
    )
    .await;
}
