//! 진단의 실 서버 시험(15-m4-plan.md M4-06 live): 픽스처를 임시 폴더에 복사해 이 행의 파일(`src/m4_06_diagnostics.*` — 앵커
//! `m4_06.type-error`)을 서버 관리자로 열면 타입 오류 하나가 그 줄 · 범위로 이벤트 · 목록에 온다. 진단 수는 그 파일 안으로만 본다
//! (fixtures/README.md 「자기 파일 안」). rust-analyzer 는 pull(3.17)을 알리므로 자체 진단(`source: rust-analyzer`)이 pull 로
//! 온다 — 이 시험이 pull 길의 live 확인이다. 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs`).
//!
//! typescript-language-server 는 진단에 `version` 을 싣지 않는다 — 그 진단이 「버전 모름」 으로 코드 액션 문맥에 실려 빠른 수정이
//! 나오는지도 본다(M4-06 재리뷰 N1, 픽스처 `src/unused.ts` 의 앵커 `unused.rect` — TS6133).

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

    pub fn copy_dir(from: &Path, to: &Path) {
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
    pub fn anchor(text: &str, name: &str) -> (u64, u64) {
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
                    // rust-analyzer 의 flycheck(rustc) 진단은 `data.rendered` 를 싣는다 — Host 의 `data` 상한(4 KiB)이 실제 진단의
                    // data 를 버리지 않는지 본다(버리면 「빠른 수정」 이 그것을 서버에 돌려주지 못한다 — M4-12).
                    && (fixture != "rust"
                        || ds
                            .iter()
                            .any(|d| d["source"] == "rustc" && !d["data"].is_null()))
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

/// `⌥↩`(진단 없이 — Host 가 진단 저장으로 채운다)과 「빠른 수정」(팝업이 버전 모름 `null` 로 준 진단) 모두 TS 의 빠른 수정을 받는다.
/// typescript-language-server 는 진단의 오류 코드로만 빠른 수정을 계산하므로, 문맥에 진단이 안 실리면 리팩터만 나온다.
#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_quick_fix_without_a_diagnostic_version() {
    use std::path::Path;
    use std::time::Duration;

    use common::env::{Env, EnvOptions};
    use pw_core::Filter;
    use pw_core::bus::Received;
    use pw_lsp::diagnostics::TOPIC_DIAGNOSTICS;
    use pw_lsp::live::LiveServer;
    use serde_json::{Value, json};

    let server = LiveServer::TypescriptLanguageServer;
    let env = Env::new(EnvOptions {
        isolated_shell: false,
        ..EnvOptions::default()
    });
    live::copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/typescript"),
        &env.root,
    );
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
    let path = env.root.join("src/unused.ts");
    let text = std::fs::read_to_string(&path).unwrap();
    let (line, column) = live::anchor(&text, "unused.rect");
    let mut win = env.window();
    let opened = win.open(&env, &path, "TypeScript", &text).await;
    let doc = opened["docId"].as_str().unwrap().to_owned();

    let wanted = path.to_string_lossy().into_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
    let unused = loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(Received::Event(event))) if event.payload["path"] == wanted.as_str() => {
                eprintln!("{}: {}", server.executable(), event.payload["diagnostics"]);
                if let Some(d) = event.payload["diagnostics"]
                    .as_array()
                    .and_then(|ds| ds.iter().find(|d| d["code"] == "6133"))
                {
                    break d.clone();
                }
            }
            Ok(Some(_)) => {}
            _ => panic!("no TS6133 in {wanted}"),
        }
    };
    // 이 서버는 버전을 싣지 않는다 — 이 시험이 보는 경우가 맞는지.
    assert!(unused["version"].is_null(), "{unused}");
    assert_eq!(unused["range"]["start"]["line"], line, "{unused}");

    let offset = |line: u64, character: u64| -> u64 {
        let start: usize = text
            .split_inclusive('\n')
            .take(usize::try_from(line).unwrap())
            .map(|l| l.encode_utf16().count())
            .sum();
        start as u64 + character
    };
    let from = offset(line, column);
    let to = from + 4;
    let quick_fixes = |list: &Value| -> Vec<String> {
        list["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| {
                a["kind"]
                    .as_str()
                    .is_some_and(|k| k.starts_with("quickfix"))
            })
            .map(|a| a["title"].as_str().unwrap_or_default().to_owned())
            .collect()
    };
    // `⌥↩`: 진단 없이.
    let invoked = win
        .call(
            "lsp.codeAction.list",
            json!({ "docId": doc, "version": 0, "from": from, "to": to }),
        )
        .await
        .unwrap();
    assert!(!quick_fixes(&invoked).is_empty(), "{invoked}");
    // 「빠른 수정」: 팝업의 LSP 진단(`lspDiagnostic` 꼴)을 버전 모름으로.
    let items = json!([{
        "range": unused["range"], "severity": 1, "message": unused["message"],
        "code": unused["code"], "source": unused["source"],
    }]);
    let quick = win
        .call(
            "lsp.codeAction.list",
            json!({ "docId": doc, "version": 0, "from": from, "to": to, "only": ["quickfix"],
                    "diagnostics": { "version": null, "items": items } }),
        )
        .await
        .unwrap();
    assert!(!quick_fixes(&quick).is_empty(), "{quick}");
    eprintln!(
        "quick fixes: invoked {:?}, quick {:?}",
        quick_fixes(&invoked),
        quick_fixes(&quick)
    );
    win.session.close();
}
