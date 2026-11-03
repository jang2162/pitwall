//! 작업 공간 심볼의 실 서버 시험(15-m4-plan.md M4-16 live): 픽스처를 임시 폴더에 복사해 이 행의 파일(`m4_16_symbols`)을 열고
//! `lsp.workspaceSymbol` 이 그 파일의 심볼을 정의 줄(앵커 `sym.*` — rust-analyzer 는 이름 자리, TS 는 선언 줄의 처음)로 돌려주는지, 클래스로 이동의 종류(`kinds`)가 함수를 빼는지,
//! 그리고 **같은 파일의 심볼 인덱스(M4-15) 결과와 열쇠(루트 기준 상대 경로 · 줄 · 이름)가 같은지** 본다 — 웹뷰는 이 열쇠로 두 출처를
//! 한 줄로 합친다(apps/desktop/src/search-everywhere/symbol-search.ts `mergeSymbols`). 열쇠가 어긋나면 같은 심볼이 두 번 보인다.
//! 서버가 선언의 처음을 주는 꼴(데코레이터 클래스 · 이름이 다음 줄인 선언 — `m4_16_decorated.ts`)은 줄이 어긋나고, 서버 범위가
//! 인덱스의 이름 줄을 담는지를 본다(`declaration_starts` — 웹뷰는 이 포함으로 합친다).
//! 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs` 머리 주석).

#![cfg(unix)]

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
mod live {
    use std::path::Path;
    use std::sync::Arc;
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

    /// 앵커 `/*@name*/` 바로 뒤 글자의 (줄, 열)(1 부터, 열은 UTF-16).
    fn anchor(text: &str, name: &str) -> (u64, u64) {
        let mark = format!("/*@{name}*/");
        let at = text
            .find(&mark)
            .unwrap_or_else(|| panic!("no anchor {mark}"))
            + mark.len();
        let before = &text[..at];
        let line = before.matches('\n').count() as u64 + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = before[line_start..].encode_utf16().count() as u64 + 1;
        (line, column)
    }

    /// 서버가 `name` 을 내놓을 때까지(색인 중이면 빈 답) 다시 묻는다(최대 180 s).
    async fn symbols(win: &mut Conn, env: &Env, query: &str, kinds: Value, name: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            let got = win
                .call(
                    "lsp.workspaceSymbol",
                    json!({ "projectId": env.id, "query": query, "kinds": kinds }),
                )
                .await;
            if let Ok(got) = &got
                && got["symbols"]
                    .as_array()
                    .is_some_and(|s| s.iter().any(|s| s["name"] == name))
            {
                return got.clone();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "workspace/symbol never answered {name}: {got:?}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// 픽스처를 임시 폴더에 복사하고 `file` 을 창에서 연다(서버가 준비될 때까지). (환경, 창, 실제 경로, 글).
    async fn open(
        server: LiveServer,
        fixture: &str,
        file: &str,
        language: &str,
    ) -> (Env, Conn, std::path::PathBuf, String) {
        let rust = fixture == "rust";
        let env = Env::new(EnvOptions {
            // cargo · node 를 찾도록 시험 프로세스의 환경을 넘긴다.
            isolated_shell: false,
            ..EnvOptions::default()
        });
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        copy_dir(&from, &env.root);
        if rust {
            // 픽스처 규칙: 이 행의 파일이 크레이트에 들도록 임시 복사본의 lib.rs 끝에 한 줄.
            let lib = env.root.join("src/lib.rs");
            let mut text = std::fs::read_to_string(&lib).unwrap();
            text.push_str("pub mod m4_16_symbols;\n");
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
        let real = std::fs::canonicalize(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut win = env.window();
        win.open(&env, &path, language, &text).await;
        let ready = common::eventually(Duration::from_secs(60), || {
            env.state_of(server.server_id()) == Some(LspInstanceState::Ready)
        })
        .await;
        assert!(ready, "{:?}", env.states());
        (env, win, real, text)
    }

    /// 심볼 인덱스(M4-15)가 같은 글에서 뽑은 심볼.
    fn index_of(file: &str, text: &str) -> pw_index::methods::IndexFileSymbols {
        let fs = Arc::new(pw_fs::Fs::new());
        let extensions = Arc::new(pw_ext::Registry::from_roots(
            &semver::Version::new(0, 0, 0),
            pw_ext::builtin::roots(),
        ));
        pw_index::SymbolIndex::new(fs, extensions).parse_text(file, text)
    }

    pub async fn workspace_symbols(server: LiveServer, fixture: &str, file: &str, language: &str) {
        let rust = fixture == "rust";
        let (env, mut win, real, text) = open(server, fixture, file, language).await;

        // 심볼로 이동: 그 파일의 셋이 정의 이름 자리로.
        let got = symbols(&mut win, &env, "M416", Value::Null, "M416Gadget").await;
        eprintln!(
            "{}: workspace/symbol {}",
            server.executable(),
            got["symbols"]
        );
        assert_eq!(got["servers"], json!([server.server_id()]), "{got}");
        let mine: Vec<&Value> = got["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["location"]["path"] == real.display().to_string())
            .collect();
        let build = if rust {
            "m416_build_gadget"
        } else {
            "m416BuildGadget"
        };
        // rust-analyzer 는 기본 설정(`workspace.symbol.search.kind = "only_types"`)에서 타입만 준다 — 함수는 심볼 인덱스가 낸다(웹뷰가
        // 합친다). typescript-language-server 는 함수도 주고, 위치는 선언의 처음(1열)이다 — 이 파일의 선언은 이름과 같은 줄에서
        // 시작해 줄이 같고, 열은 웹뷰가 인덱스의 이름 자리로 고친다(symbol-search.ts `mergeSymbols`). 줄이 다른 꼴은 `declaration_starts`.
        let expected: &[(&str, &str)] = if rust {
            &[("M416Gadget", "sym.gadget"), ("M416Spin", "sym.spin")]
        } else {
            &[
                ("M416Gadget", "sym.gadget"),
                ("M416Spin", "sym.spin"),
                (build, "sym.build"),
            ]
        };
        for &(name, anchor_name) in expected {
            let found = mine
                .iter()
                .find(|s| s["name"] == name)
                .unwrap_or_else(|| panic!("{name} is not in {got}"));
            let (line, column) = anchor(&text, anchor_name);
            assert_eq!(found["location"]["line"], line, "{name}: {found}");
            if rust {
                assert_eq!(found["location"]["column"], column, "{name}: {found}");
            } else {
                assert!(
                    found["location"]["column"].as_u64().unwrap() <= column,
                    "{name}: {found}"
                );
            }
            assert_eq!(found["location"]["token"], Value::Null, "inside the root");
            assert_eq!(found["relative"], file, "{found}");
            assert_eq!(found["language"], language, "{found}");
        }

        // 클래스로 이동의 종류(웹뷰 symbol-kinds.ts 의 표와 같다): 함수는 빠지고 타입은 남는다.
        let types = symbols(
            &mut win,
            &env,
            "M416",
            json!([5, 10, 11, 23, 26]),
            "M416Gadget",
        )
        .await;
        let names: Vec<&str> = types["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["location"]["path"] == real.display().to_string())
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"M416Spin"), "{types}");
        assert!(!names.contains(&build), "{types}");

        // 심볼 인덱스가 같은 파일에서 뽑은 이름 자리와 열쇠가 같다 — 웹뷰가 한 줄로 합친다.
        let index = index_of(file, &text);
        for &(name, _) in expected {
            let from_index = index
                .symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("the index has no {name}: {:?}", index.symbols));
            let from_server = mine.iter().find(|s| s["name"] == name).unwrap();
            assert_eq!(
                (
                    file,
                    u64::from(from_index.selection_range.start.line) + 1,
                    name
                ),
                (
                    from_server["relative"].as_str().unwrap(),
                    from_server["location"]["line"].as_u64().unwrap(),
                    from_server["name"].as_str().unwrap(),
                ),
                "the index and the server must share the merge key for {name}"
            );
        }
        win.session.close();
    }

    /// 서버가 이름 자리가 아니라 **선언의 처음**(데코레이터 줄 · `class` 줄)을 주는 꼴(`m4_16_decorated.ts`): 서버 위치의 줄은 인덱스의
    /// 이름 줄과 다를 수 있지만 서버 범위가 그 줄을 담는다 — 웹뷰가 같은 파일 · 같은 이름의 두 줄을 이 포함으로 하나로 합친다
    /// (symbol-search.ts `mergeSymbols` 의 `covers`). 서버가 준 실제 답은 시험 출력(`--nocapture`)에 적는다 — 웹뷰 시험의 녹화
    /// (symbol-search.test.ts 「typescript-language-server」)와 견준다.
    pub async fn declaration_starts(server: LiveServer) {
        let file = "src/m4_16_decorated.ts";
        let (env, mut win, real, text) = open(server, "typescript", file, "TypeScript").await;
        let got = symbols(&mut win, &env, "M416", Value::Null, "M416Split").await;
        let index = index_of(file, &text);
        for (name, anchor_name) in [("M416Panel", "sym.decorated"), ("M416Split", "sym.split")] {
            let found = got["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == name && s["location"]["path"] == real.display().to_string())
                .unwrap_or_else(|| panic!("{name} is not in {got}"));
            let from_index = index
                .symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("the index has no {name}: {:?}", index.symbols));
            let name_line = u64::from(from_index.selection_range.start.line) + 1;
            let (line, end_line) = (
                found["location"]["line"].as_u64().unwrap(),
                found["location"]["endLine"].as_u64().unwrap(),
            );
            eprintln!(
                "{}: {name} server line {line}..{end_line} column {}, index name line {name_line} column {}",
                server.executable(),
                found["location"]["column"],
                from_index.selection_range.start.character + 1
            );
            assert_eq!(anchor(&text, anchor_name).0, name_line, "{name}");
            assert_eq!(found["relative"], file, "{found}");
            assert!(
                line <= name_line && name_line <= end_line,
                "{name}: the server range {line}..{end_line} must hold the index's name line {name_line} — {found}"
            );
        }
        win.session.close();
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_workspace_symbols() {
    live::workspace_symbols(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        "src/m4_16_symbols.rs",
        "Rust",
    )
    .await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_workspace_symbols() {
    live::workspace_symbols(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        "src/m4_16_symbols.ts",
        "TypeScript",
    )
    .await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_workspace_symbols_declaration_start() {
    live::declaration_starts(pw_lsp::live::LiveServer::TypescriptLanguageServer).await;
}
