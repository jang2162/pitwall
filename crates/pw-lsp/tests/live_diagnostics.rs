//! 진단의 실 서버 시험(15-m4-plan.md M4-06 live): 픽스처를 임시 폴더에 복사해 이 행의 파일(`src/m4_06_diagnostics.*` — 앵커
//! `m4_06.type-error`)을 서버 관리자로 열면 타입 오류 하나가 그 줄 · 범위로 이벤트 · 목록에 온다. 진단 수는 그 파일 안으로만 본다
//! (fixtures/README.md 「자기 파일 안」). rust-analyzer 는 pull(3.17)을 알리므로 자체 진단(`source: rust-analyzer`)이 pull 로
//! 온다 — 이 시험이 pull 길의 live 확인이다. 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs`).

#![cfg(unix)]

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
mod live {
    use std::path::Path;
    use std::time::Duration;

    use pw_core::Filter;
    use pw_core::bus::Received;
    use pw_lsp::diagnostics::TOPIC_DIAGNOSTICS;
    use pw_lsp::live::LiveServer;
    use serde_json::{Value, json};

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

    /// 앵커 `/*@name*/` 바로 뒤의 (줄, UTF-16 열).
    fn anchor(text: &str, name: &str) -> (u64, u64) {
        let marker = format!("/*@{name}*/");
        let at = text.find(&marker).expect("anchor") + marker.len();
        let before = &text[..at];
        let line = before.matches('\n').count();
        let column = before[before.rfind('\n').map_or(0, |i| i + 1)..]
            .encode_utf16()
            .count();
        (line as u64, column as u64)
    }

    fn at(d: &Value, line: u64, start: u64, end: u64) -> bool {
        d["range"]
            == json!({
                "start": { "line": line, "character": start },
                "end": { "line": line, "character": end },
            })
    }

    /// `file` 을 열고 앵커 자리의 오류(길이 `width`)를 기다린다. `pulled` 면 서버 자체 진단(`source == server`)도 온 것을 본다.
    pub async fn type_error(
        server: LiveServer,
        fixture: &str,
        file: &str,
        language: &str,
        width: u64,
        pulled: Option<&str>,
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
        if fixture == "rust" {
            // 픽스처 규칙: 이 행의 파일을 임시 복사본의 lib.rs 끝에 한 줄로 크레이트에 넣는다.
            let lib = env.root.join("src/lib.rs");
            let mut text = std::fs::read_to_string(&lib).unwrap();
            text.push_str("pub mod m4_06_diagnostics;\n");
            std::fs::write(&lib, text).unwrap();
        }
        env.configure(
            server.server_id(),
            &server.program(),
            &server
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        );
        let mut events = env.bus.subscribe(Filter::all().topics([TOPIC_DIAGNOSTICS]));
        let path = env.root.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let (line, column) = anchor(&text, "m4_06.type-error");
        let mut win = env.window();
        win.open(&env, &path, language, &text).await;

        let wanted = path.to_string_lossy().into_owned();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
        let mut last = Value::Null;
        loop {
            let found = last.as_array().is_some_and(|ds| {
                ds.iter()
                    .any(|d| d["severity"] == "error" && at(d, line, column, column + width))
                    && pulled.is_none_or(|source| ds.iter().any(|d| d["source"] == source))
            });
            if found {
                break;
            }
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Some(Received::Event(event))) if event.payload["path"] == wanted.as_str() => {
                    last = event.payload["diagnostics"].clone();
                    eprintln!("{}: {last}", server.executable());
                }
                Ok(Some(_)) => {}
                _ => panic!(
                    "no error at {line}:{column}+{width} (pulled {pulled:?}) in {wanted}: {last}"
                ),
            }
        }
        // 목록 · 수도 같은 것을 본다(그 파일 안으로만).
        let list = win
            .call(
                "lsp.diagnostics.list",
                json!({ "projectId": env.id, "path": path, "limit": 1000 }),
            )
            .await
            .unwrap();
        let listed = list["files"][0]["diagnostics"].as_array().unwrap();
        assert!(
            listed
                .iter()
                .any(|d| d["severity"] == "error" && at(d, line, column, column + width)),
            "{list}"
        );
        // 이 파일의 오류는 모두 그 줄이다(타입 오류 하나).
        assert!(
            listed
                .iter()
                .filter(|d| d["severity"] == "error")
                .all(|d| d["range"]["start"]["line"] == line),
            "{list}"
        );
        let counts = win
            .call("lsp.diagnostics.counts", json!({ "projectId": env.id }))
            .await
            .unwrap();
        assert!(counts["error"].as_u64().unwrap() >= 1, "{counts}");
        win.session.close();
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_diagnostics() {
    live::type_error(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        "src/m4_06_diagnostics.rs",
        "Rust",
        7,
        Some("rust-analyzer"),
    )
    .await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_diagnostics() {
    live::type_error(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        "src/m4_06_diagnostics.ts",
        "TypeScript",
        5,
        None,
    )
    .await;
}
