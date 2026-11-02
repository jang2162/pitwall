//! 내비게이션 기능의 실 서버 시험(15-m4-plan.md M4-08 live): 픽스처를 임시 폴더에 복사해 이 행의 파일(`m4_08_nav`)의 호출 자리에서
//! 정의가 기대 파일 · 줄로 가고, 사용처(선언 포함)가 그 파일 안에 셋이며, rust-analyzer 에서 `Vec` 의 정의가 표준 라이브러리
//! 소스(루트 밖)를 읽기 토큰으로 읽기 전용으로 연다. rust-src 가 없으면 그 시험은 「rust-src 없음」 으로 실패한다(건너뛰지 않는다).
//! 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다(`src/live.rs` 머리 주석).

#![cfg(unix)]

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
mod live {
    use std::path::{Path, PathBuf};
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

    /// 시험 프로세스의 `rustc` 툴체인에 rust-src 가 있는가.
    fn rust_src_installed() -> bool {
        std::process::Command::new("rustc")
            .args(["--print", "sysroot"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .is_some_and(|root| {
                Path::new(&root)
                    .join("lib/rustlib/src/rust/library")
                    .is_dir()
            })
    }

    /// 앵커 `/*@name*/` 바로 뒤 글자의 UTF-16 오프셋과 그 (줄, 열)(1 부터).
    fn anchor(text: &str, name: &str) -> (u32, u32, u32) {
        let mark = format!("/*@{name}*/");
        let at = text
            .find(&mark)
            .unwrap_or_else(|| panic!("no anchor {mark}"))
            + mark.len();
        let before = &text[..at];
        let line = before.matches('\n').count() as u32 + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = before[line_start..].encode_utf16().count() as u32 + 1;
        (before.encode_utf16().count() as u32, line, column)
    }

    /// 위치 목록이 빈 동안(색인 중) 다시 묻는다(최대 180 s).
    async fn locations(win: &mut Conn, method: &str, params: Value) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            match win.call(method, params.clone()).await {
                Ok(got)
                    if got["staleVersion"] == false
                        && got["locations"].as_array().is_some_and(|l| !l.is_empty()) =>
                {
                    return got;
                }
                other => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "{method} never answered: {other:?}"
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    pub async fn navigation(server: LiveServer, fixture: &str, file: &str, language: &str) {
        let rust = fixture == "rust";
        if rust {
            assert!(
                rust_src_installed(),
                "rust-src 없음: `rustup component add rust-src` 뒤 다시 돌린다(이 시험은 표준 라이브러리 소스를 연다)"
            );
        }
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
            text.push_str("pub mod m4_08_nav;\n");
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
        let real: PathBuf = std::fs::canonicalize(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut win = env.window();
        let opened = win.open(&env, &path, language, &text).await;
        let doc = opened["docId"].as_str().unwrap().to_owned();
        let ready = common::eventually(Duration::from_secs(60), || {
            env.state_of(server.server_id()) == Some(LspInstanceState::Ready)
        })
        .await;
        assert!(ready, "{:?}", env.states());

        // 호출 자리에서 정의로: 기대 파일 · 줄(이 파일의 `nav.def`).
        let (call, _, _) = anchor(&text, "nav.call-1");
        let (_, def_line, def_column) = anchor(&text, "nav.def");
        let got = locations(
            &mut win,
            "lsp.definition",
            json!({ "docId": doc, "offset": call }),
        )
        .await;
        eprintln!("{}: definition {}", server.executable(), got["locations"]);
        let first = &got["locations"][0];
        assert_eq!(first["path"], real.display().to_string(), "{got}");
        assert_eq!(first["token"], Value::Null);
        assert_eq!(first["line"], def_line, "{got}");
        assert_eq!(first["column"], def_column, "{got}");
        assert!(
            first["preview"]
                .as_str()
                .is_some_and(|p| p.contains("greet")),
            "{got}"
        );

        // 사용처(선언 포함): 이 파일 안에 셋 — 정의 · 호출 둘.
        let got = locations(
            &mut win,
            "lsp.references",
            json!({ "docId": doc, "offset": call, "includeDeclaration": true }),
        )
        .await;
        let mine: Vec<u64> = got["locations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["path"] == real.display().to_string())
            .map(|l| l["line"].as_u64().unwrap())
            .collect();
        assert_eq!(mine.len(), 3, "{got}");

        if rust {
            // `Vec` 의 정의는 표준 라이브러리 소스 — 루트 밖이라 토큰으로만, 읽기 전용으로 읽힌다.
            let (vec_at, _, _) = anchor(&text, "nav.vec");
            let got = locations(
                &mut win,
                "lsp.definition",
                json!({ "docId": doc, "offset": vec_at }),
            )
            .await;
            eprintln!("{}: Vec goes to {}", server.executable(), got["locations"]);
            let loc = &got["locations"][0];
            let token = loc["token"]
                .as_str()
                .unwrap_or_else(|| panic!("the standard library is outside the root: {got}"));
            assert!(
                loc["path"].as_str().unwrap().contains("alloc/src/vec"),
                "{got}"
            );
            let read = win
                .call("lsp.document.read", json!({ "token": token }))
                .await
                .unwrap();
            assert!(
                read["text"].as_str().unwrap().contains("pub struct Vec"),
                "the token reads the standard library source"
            );
            assert_eq!(read["server"], server.server_id());
            // 그 문서 안의 hover 도 같은 서버로 된다(정의 자리).
            let line = loc["line"].as_u64().unwrap() as usize;
            let column = loc["column"].as_u64().unwrap() as usize;
            let source = read["text"].as_str().unwrap();
            let offset: usize = source
                .split_inclusive('\n')
                .take(line - 1)
                .map(|l| l.encode_utf16().count())
                .sum::<usize>()
                + column
                - 1;
            let hover = win
                .call("lsp.hover", json!({ "token": token, "offset": offset }))
                .await
                .unwrap();
            assert!(
                hover["hover"]["value"]
                    .as_str()
                    .is_some_and(|v| v.contains("Vec")),
                "{hover}"
            );
        }
        win.session.close();
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_navigation() {
    live::navigation(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        "src/m4_08_nav.rs",
        "Rust",
    )
    .await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_navigation() {
    live::navigation(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        "src/m4_08_nav.ts",
        "TypeScript",
    )
    .await;
}
