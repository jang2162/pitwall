//! 포맷의 실 서버 시험(15-m4-plan.md M4-13 live): 픽스처의 흐트러진 서식 파일(앵커 `format.messy` — `src/messy.{rs,ts}`)을
//! `lsp.format` 으로 포맷한 편집을 웹뷰 적용기와 같은 규칙(LSP 위치 → 오프셋, 뒤에서부터)으로 문서에 넣으면 —
//! rust-analyzer 는 툴체인의 `rustfmt` 결과와 같아진다(rust-analyzer 가 `rustfmt` 를 부른다 — **없으면 「rustfmt 없음」 으로
//! 실패한다**), TS 서버는 그 서버의 포맷 결과(다시 포맷하면 바꿀 것이 없다)가 되고 줄 하나를 견본으로 본다. 두 서버 모두 넣은 뒤
//! 다시 포맷하면 편집이 없다. TS 서버는 2칸 들여쓰기 파일(앵커 `format.two-space`)을 2칸으로 포맷하면 글이 그대로다(웹뷰의
//! 들여쓰기 판정이 그 파일에 2 를 낸다 — M4-13 리뷰 H1). rust-analyzer 를 띄운 직후부터 편집과 포맷을 잇달아 해도 웹뷰가 넣는 답
//! (Host 가 `staleVersion` · `notReady` 를 달지 않은 답 — `notReady` 는 정의의 「포맷 답을 믿지 않을 진행」 `Roots Scanned` 이
//! 요청 왕복 중에 한 번이라도 열려 있던 답, `crates/pw-lsp/src/format.rs` 「진행 중의 답」)은 늘 그 버전 글의 `rustfmt` 결과가 된다(M4-13 재리뷰 N1 — 작업 공간을 읽는
//! 동안의 답은 옛 글 기준이다). 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다
//! (`src/live.rs` 머리 주석).

#![cfg(unix)]

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
mod live {
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use pw_lsp::edit::LspTextEdit;
    use pw_lsp::live::LiveServer;
    use pw_lsp::methods::LspInstanceState;
    use pw_rpc::RpcError;
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

    /// 포맷을 한 번 묻는다(공백 `tab_size` 칸). 웹뷰(format.ts)가 넣지 않는 답 — Host 가 낡았다(`staleVersion`)거나 서버가 아직
    /// 준비 중(`notReady` — 포맷 답을 믿지 않을 진행 중)이라고 한 답 — 은 `Ok(Err(까닭))`.
    async fn format_once(
        win: &mut Conn,
        doc: &str,
        version: u32,
        server: &LiveServer,
        tab_size: u32,
    ) -> Result<Result<Vec<LspTextEdit>, String>, RpcError> {
        let answer = win
            .call(
                "lsp.format",
                json!({ "docId": doc, "version": version, "options": { "tabSize": tab_size, "insertSpaces": true } }),
            )
            .await?;
        assert_eq!(answer["server"], server.server_id(), "{answer}");
        if answer["staleVersion"] == true {
            return Ok(Err("stale".into()));
        }
        if answer["notReady"] == true {
            return Ok(Err("notReady".into()));
        }
        Ok(Ok(serde_json::from_value(answer["edits"].clone()).unwrap()))
    }

    /// 웹뷰가 넣을 포맷 답을 받을 때까지 다시 묻는다(최대 30 s) — 서버가 색인하는 동안의 `ContentModified` · 시작 중 5001 · 낡은 답 ·
    /// 준비 중의 답.
    async fn format(
        win: &mut Conn,
        doc: &str,
        version: u32,
        server: &LiveServer,
        tab_size: u32,
    ) -> Vec<LspTextEdit> {
        let mut last: Option<Value> = None;
        for _ in 0..150 {
            match format_once(win, doc, version, server, tab_size).await {
                Ok(Ok(edits)) => return edits,
                Ok(Err(reason)) => last = Some(json!(format!("not taken: {reason}"))),
                Err(e) => last = Some(json!(e.to_string())),
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

        let edits = format(&mut win, &doc, 0, &server, 4).await;
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
        let again = format(&mut win, &doc, 1, &server, 4).await;
        assert_eq!(
            apply(&after, &again),
            after,
            "{}: formatting twice changes the text: {again:?}",
            server.executable()
        );
        win.session.close();
    }

    /// 2칸 들여쓰기 · 여러 줄 JSDoc 파일(앵커 `format.two-space` — `src/m4_13_two_space.ts`)을 웹뷰의 들여쓰기 판정이 내는 옵션
    /// (공백 2칸 — `apps/desktop/src/lsp/format.test.ts` 가 같은 파일로 본다)으로 포맷하면 글이 그대로다(M4-13 리뷰 H1 — 판정이 1 을
    /// 내면 TS 서버는 파일 전체를 1칸으로 다시 들여쓴다).
    pub async fn keep_two_spaces(server: LiveServer) {
        let env = Env::new(EnvOptions {
            isolated_shell: false,
            ..EnvOptions::default()
        });
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/typescript");
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
        let path = env.root.join("src/m4_13_two_space.ts");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("/*@format.two-space*/"),
            "the fixture lost its anchor"
        );
        let mut win = env.window();
        let doc = win.open(&env, &path, "TypeScript", &text).await["docId"]
            .as_str()
            .unwrap()
            .to_owned();
        let ready = common::eventually(Duration::from_secs(60), || {
            env.state_of(server.server_id()) == Some(LspInstanceState::Ready)
        })
        .await;
        assert!(ready, "{:?}", env.states());
        let edits = format(&mut win, &doc, 0, &server, 2).await;
        assert_eq!(
            apply(&text, &edits),
            text,
            "{}: a two-space file changed with tabSize 2: {edits:?}",
            server.executable()
        );
        win.session.close();
    }

    /// rust-analyzer 를 띄운 직후(작업 공간을 읽는 동안)부터 버전마다 글 전체를 「포맷된 글 + `// vN`」 / 「흐트러진 글 + `// vN`」
    /// 으로 번갈아 바꾸고 곧바로 포맷한다(M4-13 재리뷰의 탐침 꼴). 웹뷰가 넣는 답은 모두 그 버전 글의 `rustfmt` 결과를 내야 한다 —
    /// `notReady` 가 없으면 첫 답 뒤 1 s 안쯤에 옛 글 기준 편집이 「낡지 않음」 으로 온다(재리뷰: 1,633 답 중 4). 첫 답 뒤 6 s 가
    /// 지나고 처음 받은 답 뒤 3 s 가 지날 때까지 돈다(전체 상한 180 s). 처음 받은 답까지의 시간을 출력한다.
    pub async fn format_right_after_start(server: LiveServer) {
        let env = Env::new(EnvOptions {
            isolated_shell: false,
            ..EnvOptions::default()
        });
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust");
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
        let path = env.root.join("src/messy.rs");
        let messy = std::fs::read_to_string(&path).unwrap();
        assert!(
            messy.contains("/*@format.messy*/"),
            "the fixture lost its anchor"
        );
        let formatted = rustfmt(&env.root, &messy);
        assert_ne!(formatted, messy, "the fixture is already formatted");
        // 끝에 붙인 주석 한 줄은 rustfmt 가 그대로 둔다 — 두 꼴 모두 `formatted + 주석` 이 맞는 결과다.
        let versioned = |v: u32| {
            let base = if v % 2 == 0 { &messy } else { &formatted };
            (format!("{base}// v{v}\n"), format!("{formatted}// v{v}\n"))
        };
        assert_eq!(rustfmt(&env.root, &versioned(2).0), versioned(2).1);
        assert_eq!(rustfmt(&env.root, &versioned(3).0), versioned(3).1);

        let mut win = env.window();
        let doc = win.open(&env, &path, "Rust", &messy).await["docId"]
            .as_str()
            .unwrap()
            .to_owned();
        let started = Instant::now();
        let mut current = messy.clone();
        let (mut version, mut accepted, mut errors) = (0_u32, 0_u32, 0_u32);
        let mut dropped = std::collections::BTreeMap::<String, u32>::new();
        let mut first_answer: Option<Instant> = None;
        let mut first_taken: Option<Instant> = None;
        let mut wrong = Vec::new();
        while first_answer.is_none_or(|t| t.elapsed() < Duration::from_secs(6))
            || first_taken.is_none_or(|t| t.elapsed() < Duration::from_secs(3))
        {
            assert!(
                started.elapsed() < Duration::from_secs(180),
                "{}: no format answer in 180 s ({errors} errors, {:?})",
                server.executable(),
                env.states()
            );
            version += 1;
            let (text, want) = versioned(version);
            let len = current.encode_utf16().count() as u32;
            win.call(
                "lsp.document.change",
                json!({ "docId": doc, "version": version, "changes": [{ "from": 0, "to": len, "insert": text }] }),
            )
            .await
            .unwrap();
            current = text;
            match format_once(&mut win, &doc, version, &server, 4).await {
                Ok(Ok(edits)) => {
                    first_answer.get_or_insert_with(Instant::now);
                    first_taken.get_or_insert_with(Instant::now);
                    accepted += 1;
                    let got = apply(&current, &edits);
                    if got != want {
                        wrong.push((version, started.elapsed(), edits.len()));
                    }
                }
                Ok(Err(reason)) => {
                    first_answer.get_or_insert_with(Instant::now);
                    *dropped.entry(reason).or_default() += 1;
                }
                Err(_) => {
                    errors += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        eprintln!(
            "{}: {version} versions — {accepted} answers taken, dropped {dropped:?}, {errors} errors, {} wrong; first answer {:?}, first taken {:?} after the open",
            server.executable(),
            wrong.len(),
            first_answer.map(|t| t - started),
            first_taken.map(|t| t - started),
        );
        assert!(
            accepted > 0,
            "{}: no answer was taken (dropped {dropped:?})",
            server.executable()
        );
        assert!(
            wrong.is_empty(),
            "{}: answers on old text (version, elapsed, edits): {wrong:?}",
            server.executable()
        );
        win.session.close();
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_format_right_after_start_never_lands_on_old_text() {
    live::format_right_after_start(pw_lsp::live::LiveServer::RustAnalyzer).await;
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

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_format_keeps_two_spaces() {
    live::keep_two_spaces(pw_lsp::live::LiveServer::TypescriptLanguageServer).await;
}
