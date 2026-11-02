//! 자동완성 · 시그니처 도움말의 실 서버 시험(15-m4-plan.md M4-11 live): 픽스처를 임시 폴더에 복사해 서버 관리자로 연 문서에서
//! `self.` · `obj.` 뒤 완성 후보에 기대 항목이 있고, 시그니처 도움말의 지금 인자가 그 자리의 인자이고, TS 의 자동 import 후보를
//! resolve 하면 import 줄이 온다. 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs` 머리 주석).

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
    use super::common::env::{Conn, Env, EnvOptions};

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

    /// 열린 픽스처 문서 하나.
    pub struct Opened {
        pub env: Env,
        pub win: Conn,
        pub doc: String,
        pub text: String,
    }

    impl Opened {
        /// 앵커(`/*@이름*/`) 바로 뒤의 UTF-16 오프셋.
        pub fn anchor(&self, name: &str) -> u32 {
            let marker = format!("/*@{name}*/");
            let at = self.text.find(&marker).unwrap() + marker.len();
            u32::try_from(self.text[..at].encode_utf16().count()).unwrap()
        }
    }

    /// 픽스처 `fixture` 를 복사하고(`extra_mod` 가 있으면 임시 `lib.rs` 끝에 그 모듈을 더해) `file` 을 열어 서버가 준비될 때까지.
    pub async fn open(
        server: LiveServer,
        fixture: &str,
        extra_mod: Option<&str>,
        file: &str,
        language: &str,
    ) -> Opened {
        let env = Env::new(EnvOptions {
            // cargo · node 를 찾도록 시험 프로세스의 환경을 넘긴다.
            isolated_shell: false,
            ..EnvOptions::default()
        });
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        copy_dir(&from, &env.root);
        if let Some(module) = extra_mod {
            let lib = env.root.join("src/lib.rs");
            let mut text = std::fs::read_to_string(&lib).unwrap();
            text.push_str(&format!("pub mod {module};\n"));
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
        Opened {
            env,
            win,
            doc,
            text,
        }
    }

    /// `ask` 를 `done` 이 참이 될 때까지(색인 중에는 빈 답 · `ContentModified` 가 온다 — 최대 90 s) 다시 묻는다.
    pub async fn until(
        win: &mut Conn,
        method: &str,
        params: Value,
        done: impl Fn(&Value) -> bool,
    ) -> Value {
        let mut last = Value::Null;
        for _ in 0..300 {
            match win.call(method, params.clone()).await {
                Ok(answer) if done(&answer) => return answer,
                Ok(answer) => last = answer,
                Err(e) => last = json!({ "error": e.message, "data": e.data }),
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        panic!("{method} never answered as expected: {last}");
    }

    pub fn labels(list: &Value) -> Vec<String> {
        list["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i["label"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `.` 바로 뒤에서 묻는 완성의 후보에 `want` 가 모두 든다.
    pub async fn members_after_dot(o: &mut Opened, anchor: &str, want: &[&str]) {
        let offset = o.anchor(anchor);
        let params = json!({ "docId": o.doc, "version": 0, "offset": offset,
                              "trigger": { "kind": "character", "character": "." } });
        let list = until(&mut o.win, "lsp.completion", params, |l| {
            let got = labels(l);
            want.iter().all(|w| {
                got.iter()
                    .any(|g| g == w || g.starts_with(&format!("{w}(")))
            })
        })
        .await;
        assert_eq!(list["staleVersion"], false);
        eprintln!("completion: {:?}", labels(&list));
    }

    /// 시그니처 도움말의 지금 인자가 `want`(그 시그니처 글 안의 조각)다.
    pub async fn active_parameter(o: &mut Opened, anchor: &str, want: &str) {
        let offset = o.anchor(anchor);
        let params = json!({ "docId": o.doc, "version": 0, "offset": offset,
                              "triggerKind": "invoked", "isRetrigger": false });
        let answer = until(&mut o.win, "lsp.signatureHelp", params, |a| {
            !a["help"].is_null()
        })
        .await;
        let help = &answer["help"];
        let signature = &help["signatures"][help["activeSignature"].as_u64().unwrap() as usize];
        let label: Vec<u16> = signature["label"]
            .as_str()
            .unwrap()
            .encode_utf16()
            .collect();
        let active =
            &signature["parameters"][signature["activeParameter"].as_u64().unwrap() as usize];
        let (from, to) = (
            active["from"].as_u64().unwrap() as usize,
            active["to"].as_u64().unwrap() as usize,
        );
        let got = String::from_utf16(&label[from..to]).unwrap();
        eprintln!("signature: {} — active {got:?}", signature["label"]);
        assert!(got.contains(want), "{got:?} in {}", signature["label"]);
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_completion_after_self() {
    let mut o = live::open(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        Some("m4_11_completion"),
        "src/m4_11_completion.rs",
        "Rust",
    )
    .await;
    live::members_after_dot(&mut o, "m4-11.self-members", &["reading", "limit"]).await;
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_signature_help_marks_the_argument() {
    let mut o = live::open(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        Some("m4_11_completion"),
        "src/m4_11_completion.rs",
        "Rust",
    )
    .await;
    live::active_parameter(&mut o, "m4-11.signature-second", "height").await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_completion_after_obj() {
    let mut o = live::open(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        None,
        "src/m4_11_completion.ts",
        "TypeScript",
    )
    .await;
    live::members_after_dot(&mut o, "m4-11.obj-members", &["reading", "limit"]).await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_signature_help_marks_the_argument() {
    let mut o = live::open(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        None,
        "src/calls.ts",
        "TypeScript",
    )
    .await;
    live::active_parameter(&mut o, "signature.scale", "width").await;
}

/// TS 의 자동 import: `scal` 끝에서 고른 `scale`(./shapes)을 resolve 하면 import 줄을 주고, 그 편집을 적용한 글에 import 가 생긴다.
#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_auto_import_adds_the_import_line() {
    use serde_json::json;

    let mut o = live::open(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        None,
        "src/m4_11_completion.ts",
        "TypeScript",
    )
    .await;
    // 표지 자리의 `0` 을 `scal` 로 바꾼다(버전 1).
    let at = o.anchor("m4-11.auto-import");
    o.win
        .call(
            "lsp.document.change",
            json!({ "docId": o.doc, "version": 1, "changes": [{ "from": at, "to": at + 1, "insert": "scal" }] }),
        )
        .await
        .unwrap();
    let mut text: Vec<u16> = o.text.encode_utf16().collect();
    text.splice(at as usize..at as usize + 1, "scal".encode_utf16());
    let offset = at + 4;
    let params =
        json!({ "docId": o.doc, "version": 1, "offset": offset, "trigger": { "kind": "invoked" } });
    // 이 파일은 `scale` 을 import 하지 않았으므로 `scale` 후보는 ./shapes 의 자동 import 다(TS 는 출처를 resolve 에서 준다).
    let pick = |list: &serde_json::Value| -> Option<serde_json::Value> {
        list["items"]
            .as_array()?
            .iter()
            .find(|i| i["label"] == "scale")
            .cloned()
    };
    let list = live::until(&mut o.win, "lsp.completion", params, |l| pick(l).is_some()).await;
    let item = pick(&list).unwrap();
    let resolved = o
        .win
        .call(
            "lsp.completion.resolve",
            json!({ "docId": o.doc, "version": 1, "item": item["raw"] }),
        )
        .await
        .unwrap();
    assert_eq!(resolved["staleVersion"], false);
    let resolved = &resolved["item"];
    let extra = resolved["additionalEdits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!extra.is_empty(), "no import edit: {resolved}");
    // 본 편집(없으면 `scal` 을 이름으로)과 자동 import 를 뒤에서부터 적용한다.
    let mut edits: Vec<(usize, usize, String)> = extra
        .iter()
        .map(|e| {
            (
                e["from"].as_u64().unwrap() as usize,
                e["to"].as_u64().unwrap() as usize,
                e["newText"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let main = &resolved["edit"];
    let (from, to, new_text) = if main.is_null() {
        (at as usize, offset as usize, "scale".to_owned())
    } else {
        (
            main["insert"]["from"].as_u64().unwrap() as usize,
            main["insert"]["to"].as_u64().unwrap() as usize,
            main["newText"].as_str().unwrap().to_owned(),
        )
    };
    edits.push((from, to, new_text));
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    for (from, to, insert) in edits {
        text.splice(from..to, insert.encode_utf16());
    }
    let result = String::from_utf16(&text).unwrap();
    eprintln!("after the auto import:\n{result}");
    assert!(
        result.contains("import { scale } from \"./shapes\";")
            || result.contains("import { scale } from './shapes';"),
        "{result}"
    );
    assert!(result.contains("*/scale;"), "{result}");
}
