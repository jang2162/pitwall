//! 포맷의 실 서버 시험(15-m4-plan.md M4-13 live): 픽스처의 흐트러진 서식 파일(앵커 `format.messy` — `src/messy.{rs,ts}`)을
//! `lsp.format` 으로 포맷한 편집을 웹뷰 적용기와 같은 규칙(LSP 위치 → 오프셋, 뒤에서부터)으로 문서에 넣으면 —
//! rust-analyzer 는 툴체인의 `rustfmt` 결과와 같아진다(rust-analyzer 가 `rustfmt` 를 부른다 — **없으면 「rustfmt 없음」 으로
//! 실패한다**), TS 서버는 그 서버의 포맷 결과(다시 포맷하면 바꿀 것이 없다)가 되고 줄 하나를 견본으로 본다. 두 서버 모두 넣은 뒤
//! 다시 포맷하면 편집이 없다. 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs` 머리 주석).

#![cfg(unix)]

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
mod live {
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use pw_lsp::edit::LspTextEdit;
    use pw_lsp::live::LiveServer;
    use pw_lsp::methods::LspInstanceState;
    use serde_json::json;

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

    /// LSP 위치(줄 · UTF-16) → 바이트 오프셋(`\n` 줄 — 픽스처는 LF). 줄 끝을 넘는 글자는 줄 끝, 없는 줄은 끝(LSP 규칙).
    fn offset(text: &str, line: u32, character: u32) -> usize {
        let mut start = 0;
        for _ in 0..line {
            match text[start..].find('\n') {
                Some(i) => start += i + 1,
                None => return text.len(),
            }
        }
        let mut units = 0;
        for (i, c) in text[start..].char_indices() {
            if units >= character as usize || c == '\n' {
                return start + i;
            }
            units += c.len_utf16();
        }
        text.len()
    }

    /// 편집을 넣은 글(뒤에서부터 — 웹뷰 적용기 `textChanges` 와 같은 규칙).
    fn apply(text: &str, edits: &[LspTextEdit]) -> String {
        let mut spans: Vec<(usize, usize, usize, &str)> = edits
            .iter()
            .enumerate()
            .map(|(i, e)| {
                (
                    offset(text, e.range.start.line, e.range.start.character),
                    offset(text, e.range.end.line, e.range.end.character),
                    i,
                    e.new_text.as_str(),
                )
            })
            .collect();
        spans.sort_by_key(|s| std::cmp::Reverse((s.0, s.2)));
        let mut out = text.to_owned();
        for (from, to, _, insert) in spans {
            out.replace_range(from..to, insert);
        }
        out
    }

    /// 포맷을 묻는다 — 서버가 색인하는 동안의 `ContentModified` · 시작 중 5001 은 다시 묻는다(최대 30 s).
    async fn format(
        win: &mut Conn,
        doc: &str,
        version: u32,
        server: &LiveServer,
    ) -> Vec<LspTextEdit> {
        let mut last = None;
        for _ in 0..150 {
            match win
                .call(
                    "lsp.format",
                    json!({ "docId": doc, "version": version, "options": { "tabSize": 4, "insertSpaces": true } }),
                )
                .await
            {
                Ok(answer) => {
                    assert_eq!(answer["staleVersion"], false, "{answer}");
                    assert_eq!(answer["server"], server.server_id(), "{answer}");
                    return serde_json::from_value(answer["edits"].clone()).unwrap();
                }
                Err(e) => last = Some(e),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("{}: format never answered: {last:?}", server.executable());
    }

    /// 툴체인의 rustfmt 가 낸 글. 없으면 「rustfmt 없음」 으로 실패한다.
    fn rustfmt(dir: &Path, text: &str) -> String {
        let mut child = Command::new("rustfmt")
            .args(["--edition", "2021", "--emit", "stdout", "--quiet"])
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| {
                panic!("rustfmt 없음 — rust-analyzer 의 포맷은 툴체인의 rustfmt 를 쓴다: {e}")
            });
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "rustfmt 없음 또는 실패: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    pub async fn format_the_messy_file(server: LiveServer, fixture: &str, ext: &str) {
        let env = Env::new(EnvOptions {
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
        let path = env.root.join(format!("src/messy.{ext}"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("/*@format.messy*/"),
            "the fixture lost its anchor"
        );
        let language = if ext == "rs" { "Rust" } else { "TypeScript" };
        let mut win = env.window();
        let doc = win.open(&env, &path, language, &text).await["docId"]
            .as_str()
            .unwrap()
            .to_owned();
        let ready = common::eventually(Duration::from_secs(60), || {
            env.state_of(server.server_id()) == Some(LspInstanceState::Ready)
        })
        .await;
        assert!(ready, "{:?}", env.states());

        let edits = format(&mut win, &doc, 0, &server).await;
        assert!(!edits.is_empty(), "{}: no edits", server.executable());
        let after = apply(&text, &edits);
        eprintln!("{}: {} edits\n{after}", server.executable(), edits.len());
        if ext == "rs" {
            assert_eq!(after, rustfmt(&env.root, &text), "differs from rustfmt");
        } else {
            assert!(
                after.contains("    let c = a + b;\n"),
                "typescript-language-server: {after}"
            );
            assert!(
                after.contains(
                    "export function /*@format.messy*/messy(a: number, b: number): number {"
                ),
                "typescript-language-server: {after}"
            );
        }

        // 넣은 글을 문서로 보내고 다시 포맷하면 바꿀 것이 없다(그 서버의 포맷 결과다).
        let len = text.encode_utf16().count() as u32;
        win.call(
            "lsp.document.change",
            json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": len, "insert": after }] }),
        )
        .await
        .unwrap();
        let again = format(&mut win, &doc, 1, &server).await;
        assert_eq!(
            apply(&after, &again),
            after,
            "{}: formatting twice changes the text: {again:?}",
            server.executable()
        );
        win.session.close();
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_format_matches_rustfmt() {
    live::format_the_messy_file(pw_lsp::live::LiveServer::RustAnalyzer, "rust", "rs").await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_format_the_messy_file() {
    live::format_the_messy_file(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        "ts",
    )
    .await;
}
