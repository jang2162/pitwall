//! 웹뷰 LSP 다리의 실 서버 시험(15-m4-plan.md M4-07 live): 픽스처를 임시 폴더에 복사해 서버 관리자로 연 문서의 능력 칸
//! (`lsp.document.capabilities`)에 실제 서버의 완성 · 시그니처 트리거 문자가 든다 — 기능 행(M4-11)이 이 칸에서 읽는다. 표지
//! (`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs` 머리 주석).

#![cfg(unix)]

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
mod live {
    use std::path::Path;
    use std::time::Duration;

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

    pub async fn document_capabilities(
        server: LiveServer,
        fixture: &str,
        file: &str,
        language: &str,
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
        let features = win
            .call("lsp.document.capabilities", json!({ "docId": doc }))
            .await
            .unwrap()["features"]
            .clone();
        eprintln!("{}: {features}", server.executable());
        for feature in [
            "hover",
            "definition",
            "references",
            "completion",
            "signature_help",
        ] {
            assert_eq!(
                features[feature]["server"],
                server.server_id(),
                "{feature}: {features}"
            );
        }
        let triggers = |feature: &str| -> Vec<Value> {
            features[feature]["options"]["triggerCharacters"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        };
        assert!(triggers("completion").contains(&json!(".")), "{features}");
        assert!(
            triggers("signature_help").contains(&json!("(")),
            "{features}"
        );
        assert!(
            features
                .as_object()
                .unwrap()
                .values()
                .all(|f| f["optionsTruncated"] == false),
            "{features}"
        );
        win.session.close();
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_document_capabilities() {
    live::document_capabilities(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        "src/calls.rs",
        "Rust",
    )
    .await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_document_capabilities() {
    live::document_capabilities(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        "src/calls.ts",
        "TypeScript",
    )
    .await;
}
