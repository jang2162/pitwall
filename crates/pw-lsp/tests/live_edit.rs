//! 편집 기능의 실 서버 시험(15-m4-plan.md M4-12 live): 픽스처의 두 파일에 걸친 rename(앵커 `rename.m4-12` — 정의 하나 · import
//! 하나 · 호출 둘)의 편집이 파일 둘 · 편집 넷이고, 그 편집을 웹뷰 적용기와 같은 규칙(LSP 위치 → 오프셋, 뒤에서부터)으로 디스크에
//! 쓰면 옛 이름이 남지 않는다. 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs` 머리 주석).
//! 미사용 import 의 「빠른 수정」 live 시험은 진단 문맥(M4-06)이 병합된 뒤에 단다(계획 M4-12 행).

#![cfg(unix)]

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
mod live {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::time::Duration;

    use pw_lsp::edit::{LspFileChange, LspWorkspaceEdit};
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

    /// LSP 위치(줄 · UTF-16) → 바이트 오프셋(`\n` 줄 — 픽스처는 LF).
    fn offset(text: &str, line: u32, character: u32) -> usize {
        let start: usize = text
            .split_inclusive('\n')
            .take(line as usize)
            .map(str::len)
            .sum();
        let rest = &text[start..];
        let mut units = 0;
        for (i, c) in rest.char_indices() {
            if units >= character as usize || c == '\n' {
                return start + i;
            }
            units += c.len_utf16();
        }
        text.len()
    }

    /// 편집을 디스크에 적용한다(파일마다 뒤에서부터 — 웹뷰 적용기 `textChanges` 와 같은 규칙).
    fn write(edit: &LspWorkspaceEdit) {
        for change in &edit.changes {
            let LspFileChange::Text { path, edits, .. } = change else {
                panic!("rename made a file operation: {change:?}");
            };
            let mut text = std::fs::read_to_string(path).unwrap();
            let mut spans: Vec<(usize, usize, &str)> = edits
                .iter()
                .map(|e| {
                    (
                        offset(&text, e.range.start.line, e.range.start.character),
                        offset(&text, e.range.end.line, e.range.end.character),
                        e.new_text.as_str(),
                    )
                })
                .collect();
            spans.sort_by_key(|s| std::cmp::Reverse(s.0));
            for (from, to, insert) in spans {
                text.replace_range(from..to, insert);
            }
            std::fs::write(path, text).unwrap();
        }
    }

    pub async fn rename_across_two_files(server: LiveServer, fixture: &str, ext: &str) {
        let env = Env::new(EnvOptions {
            isolated_shell: false,
            ..EnvOptions::default()
        });
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        copy_dir(&from, &env.root);
        if ext == "rs" {
            let lib = env.root.join("src/lib.rs");
            let mut text = std::fs::read_to_string(&lib).unwrap();
            text.push_str("pub mod m4_12_target;\npub mod m4_12_user;\n");
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
        let target = env.root.join(format!("src/m4_12_target.{ext}"));
        let user = env.root.join(format!("src/m4_12_user.{ext}"));
        let text = std::fs::read_to_string(&target).unwrap();
        let language = if ext == "rs" { "Rust" } else { "TypeScript" };
        let mut win = env.window();
        let doc = win.open(&env, &target, language, &text).await["docId"]
            .as_str()
            .unwrap()
            .to_owned();
        let ready = common::eventually(Duration::from_secs(60), || {
            env.state_of(server.server_id()) == Some(LspInstanceState::Ready)
        })
        .await;
        assert!(ready, "{:?}", env.states());
        let anchor = "/*@rename.m4-12*/";
        let at = text.find(anchor).unwrap() + anchor.len();
        let offset = text[..at].encode_utf16().count();

        // 색인이 끝날 때까지(파일 둘 · 편집 넷이 될 때까지) 다시 묻는다. 색인이 바뀌는 동안은 `ContentModified`(5002 의 -32801)다.
        let mut edit = LspWorkspaceEdit::default();
        let mut prepared = Value::Null;
        for _ in 0..150 {
            prepared = match win
                .call(
                    "lsp.rename.prepare",
                    json!({ "docId": doc, "version": 0, "offset": offset }),
                )
                .await
            {
                Ok(p) => p,
                Err(_) => Value::Null,
            };
            let answer = win
                .call(
                    "lsp.rename",
                    json!({ "docId": doc, "version": 0, "offset": offset, "newName": "relabelled" }),
                )
                .await;
            if let Ok(answer) = answer {
                edit = serde_json::from_value(answer["edit"].clone()).unwrap();
                let total: usize = edit
                    .changes
                    .iter()
                    .map(|c| match c {
                        LspFileChange::Text { edits, .. } => edits.len(),
                        _ => 0,
                    })
                    .sum();
                if edit.changes.len() >= 2 && total >= 4 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        eprintln!("{}: prepare {prepared}", server.executable());
        assert_eq!(
            prepared["target"]["kind"],
            "range",
            "{}: {prepared}",
            server.executable()
        );
        let per_file: BTreeMap<String, usize> = edit
            .changes
            .iter()
            .map(|c| match c {
                LspFileChange::Text { path, edits, .. } => (
                    Path::new(path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    edits.len(),
                ),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            per_file,
            BTreeMap::from([
                (format!("m4_12_target.{ext}"), 1),
                (format!("m4_12_user.{ext}"), 3),
            ]),
            "{}: {edit:?}",
            server.executable()
        );
        write(&edit);
        for path in [&target, &user] {
            let after = std::fs::read_to_string(path).unwrap();
            assert!(
                !after.replace(anchor, "").contains("relabel(") && after.contains("relabelled"),
                "{}: {after}",
                path.display()
            );
        }
        let user_text = std::fs::read_to_string(&user).unwrap();
        assert!(
            user_text.contains("relabelled(relabelled(x))"),
            "{user_text}"
        );
        win.session.close();
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_rename_across_two_files() {
    live::rename_across_two_files(pw_lsp::live::LiveServer::RustAnalyzer, "rust", "rs").await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_rename_across_two_files() {
    live::rename_across_two_files(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        "ts",
    )
    .await;
}
