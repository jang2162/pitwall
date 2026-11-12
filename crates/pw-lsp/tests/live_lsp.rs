//! 실 서버 시험(15-m4-plan.md M4-01 ⑦ · CI `lsp-live`): 서버마다 `initialize` → 기대 능력(scripts/lsp-live.toml 의 `[capabilities]`) →
//! `shutdown`. 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다 — `src/live.rs` · `live_cfg.rs` 머리 주석.

mod common;

#[cfg(any(
    pw_live_lsp_rust_analyzer,
    pw_live_lsp_typescript_language_server,
    pw_live_lsp_basedpyright,
    pw_live_lsp_ruff,
    pw_live_lsp_jdtls,
    pw_live_lsp_kotlin_lsp
))]
#[allow(
    dead_code,
    reason = "the document sync and watch helpers are for rust-analyzer and the TypeScript server"
)]
mod live {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::time::Duration;

    use pw_lsp::live::LiveServer;
    use pw_lsp::log::{ServerLog, lsp_log_dir};
    use pw_lsp::{InitializeOptions, LanguageServer, LaunchSpec, ServerOptions};
    use pw_runtime::RuntimeEnv;

    use super::common;

    /// 문서 동기화 끝까지(15-m4-plan.md M4-05 live): 픽스처를 임시 폴더에 복사해 서버 관리자로 연다 → 진행이 `Task` 로 나왔다
    /// 끝난다 → 요청 길로 사용처를 묻는다(앵커 표의 「`scale` 의 사용처 다섯」) → 편집 · 저장 · 닫기 → 서버 로그에 오류가 없다.
    ///
    /// `hide_rust_src`(rust-analyzer 만): 표준 라이브러리 소스(rust-src)를 가린 채 돈다 — 서버 환경(`[lsp.rust-analyzer.env]`)의
    /// `RUST_SRC_PATH` 를 `core` 폴더 하나만 든 빈 폴더로(rust-analyzer 는 `core` 가 있는 폴더만 받고, 그 안에 크레이트가 없으니
    /// 표준 라이브러리 크레이트가 하나도 없다 — rust-src 가 없는 툴체인과 같은 분석). 설정 `cargo.sysrootSrc` 는 `workspace/
    /// configuration` 으로 가도 이 판(2026-10-26)이 기동 때 쓰지 않아 가리지 못한다. 가려졌는지는 임시 복사본에 더한 파일
    /// (`src/m4_05_std.rs`)의 `String` 정의가 풀리지 않는 것으로 본다. 가리지 않은 rust 실행은 rust-src 가 있는 기기에서 그 정의가
    /// 풀리는 것을 본다(대조 — M4-05 리뷰 L4).
    #[cfg(unix)]
    pub async fn document_sync(
        server: LiveServer,
        fixture: &str,
        file: &str,
        language: &str,
        hide_rust_src: bool,
    ) {
        use std::collections::BTreeSet;

        use pw_core::Filter;
        use pw_core::bus::{Received, Scope};
        use pw_core::task::{TOPIC_FINISHED, TOPIC_PROGRESS};
        use pw_lsp::methods::LspInstanceState;
        use serde_json::{Value, json};

        use super::common::env::{Env, EnvOptions};

        let env = Env::new(EnvOptions {
            // cargo · node 를 찾도록 시험 프로세스의 환경을 넘긴다.
            isolated_shell: false,
            ..EnvOptions::default()
        });
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        copy_dir(&from, &env.root);
        let std_probe = (fixture == "rust").then(|| {
            // 이 행 몫의 임시 파일(픽스처 규칙: 임시 복사본의 lib.rs 끝에 한 줄).
            let probe = env.file(
                "src/m4_05_std.rs",
                "//! rust-src 탐침: 표준 라이브러리 항목의 정의.\npub fn text() -> String {\n    /*@std.string*/String::new()\n}\n",
            );
            let lib = env.root.join("src/lib.rs");
            let mut text = std::fs::read_to_string(&lib).unwrap();
            text.push_str("pub mod m4_05_std;\n");
            std::fs::write(&lib, text).unwrap();
            probe
        });
        env.configure(
            server.server_id(),
            &server.program(),
            &server
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        );
        if hide_rust_src {
            let empty = env.base.join("no-rust-src");
            std::fs::create_dir_all(empty.join("core")).unwrap();
            let settings = env.paths.config.join("settings.toml");
            let mut text = std::fs::read_to_string(&settings).unwrap();
            text.push_str(&format!(
                "[lsp.rust-analyzer.env]\nRUST_SRC_PATH = \"{}\"\n",
                empty.display()
            ));
            std::fs::write(&settings, text).unwrap();
            env.global.reload().unwrap();
        }
        let mut events = env
            .bus
            .subscribe(Filter::all().topics([TOPIC_PROGRESS, TOPIC_FINISHED]));
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

        // 진행: 하나 이상 나왔다가 모두 끝난다(서버가 색인을 마친다).
        let mut started = BTreeSet::new();
        let mut finished = BTreeSet::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        // 서버는 단계마다 진행을 따로 낸다(rust-analyzer: 가져오기 → 색인 → 검사) — 모두 끝나고 3 s 동안 새 진행이 없을 때까지.
        let quiet = Duration::from_secs(3);
        loop {
            let settled = !started.is_empty()
                && started.is_subset(&finished)
                && env.tasks.running(Some(&env.id)).is_empty();
            let wait = if settled {
                (tokio::time::Instant::now() + quiet).min(deadline)
            } else {
                deadline
            };
            let Ok(Some(Received::Event(event))) =
                tokio::time::timeout_at(wait, events.recv()).await
            else {
                assert!(
                    settled,
                    "progress did not finish: started {started:?}, finished {finished:?}"
                );
                break;
            };
            assert_eq!(event.scope, Scope::Project(env.id.clone()));
            let id = event.payload["taskId"].as_u64().unwrap();
            if event.topic == TOPIC_PROGRESS {
                started.insert(id);
            } else {
                assert_eq!(event.payload["status"], "completed", "{}", event.payload);
                finished.insert(id);
            }
        }
        eprintln!(
            "{}: {} progress task(s) came and went",
            server.executable(),
            started.len()
        );

        // 요청 길: 앵커 `call.scale-1` 에서 사용처(선언 포함) — 정의 · import · 호출 셋.
        let anchor = "/*@call.scale-1*/";
        let at = text.find(anchor).unwrap() + anchor.len();
        let offset = text[..at].encode_utf16().count();
        let mut found = Vec::new();
        for _ in 0..50 {
            let refs = win
                .call(
                    "test.lsp.feature",
                    json!({ "docId": doc, "feature": "references", "method": "textDocument/references",
                            "offset": offset, "version": 0, "includeDeclaration": true }),
                )
                .await;
            // 색인이 바뀌는 동안은 `ContentModified` 가 온다(관리자가 한 번 다시 물은 뒤 5002) — 조금 뒤 다시.
            let refs = match refs {
                Err(e) if e.data.as_ref().is_some_and(|d| d["code"] == -32801) => Value::Null,
                other => other.unwrap(),
            };
            found = refs["result"].as_array().cloned().unwrap_or_default();
            if found.len() >= 5 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let files: BTreeSet<String> = found
            .iter()
            .filter_map(|l| l["uri"].as_str())
            .map(|u| u.rsplit('/').next().unwrap().to_owned())
            .collect();
        assert_eq!(found.len(), 5, "{found:?}");
        assert_eq!(files.len(), 2, "{files:?}");

        if let Some(probe) = &std_probe {
            let probe_text = std::fs::read_to_string(probe).unwrap();
            let probe_doc = win.open(&env, probe, language, &probe_text).await["docId"]
                .as_str()
                .unwrap()
                .to_owned();
            let anchor = "/*@std.string*/";
            let at = probe_text.find(anchor).unwrap() + anchor.len();
            let offset = probe_text[..at].encode_utf16().count();
            let mut targets = Vec::new();
            for _ in 0..50 {
                let found = win
                    .call(
                        "test.lsp.feature",
                        json!({ "docId": probe_doc, "feature": "definition", "method": "textDocument/definition",
                                "offset": offset, "version": 0 }),
                    )
                    .await;
                let found = match found {
                    Err(e) if e.data.as_ref().is_some_and(|d| d["code"] == -32801) => Value::Null,
                    other => other.unwrap()["result"].clone(),
                };
                targets = match found {
                    Value::Array(list) => list,
                    Value::Null => Vec::new(),
                    one => vec![one],
                };
                if !targets.is_empty() || hide_rust_src {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            eprintln!("String goes to {targets:?}");
            if hide_rust_src {
                assert!(targets.is_empty(), "rust-src was not hidden: {targets:?}");
            } else if rust_src_installed() {
                assert!(
                    targets.iter().any(|t| t.to_string().contains("string.rs")),
                    "{targets:?}"
                );
            }
            win.call("lsp.document.close", json!({ "docId": probe_doc }))
                .await
                .unwrap();
        }

        // 편집 · 저장 · 닫기.
        let edit = json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "// 편집 🦀\n" }] });
        assert_eq!(
            win.call("lsp.document.change", edit).await.unwrap()["off"],
            Value::Null
        );
        win.call("lsp.document.save", json!({ "docId": doc }))
            .await
            .unwrap();
        win.call("lsp.document.close", json!({ "docId": doc }))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            env.state_of(server.server_id()),
            Some(LspInstanceState::Ready)
        );

        // 서버 로그(`$STATE/logs/lsp/<서버>-…`)에 오류 수준의 메시지 · 패닉이 없다.
        let logs = env.paths.state.join("logs/lsp");
        let mut log = String::new();
        for entry in std::fs::read_dir(&logs).unwrap().flatten() {
            log.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
        assert!(!log.is_empty());
        let errors: Vec<&str> = log
            .lines()
            .filter(|l| {
                l.contains("window/logMessage [1]")
                    || l.contains("window/showMessage [1]")
                    || l.contains("panicked")
                    || l.contains("killed:")
            })
            .collect();
        assert!(errors.is_empty(), "{errors:#?}");
        win.session.close();
        assert!(
            common::eventually(Duration::from_secs(3), || env
                .manager
                .pids(&env.id)
                .is_empty())
            .await
        );
    }

    /// 파일 감시 전달(15-m4-plan.md M4-05b live — 메인 판정 2026-11-02): 임시 복사본의 `lib.rs` 가 아직 디스크에 없는 모듈
    /// (`m4_05b_late`)을 선언하고 탐침(`src/m4_05b_watch.rs`)이 그 함수를 부른다. 색인이 끝난 뒤 탐침의 정의는 풀리지 않는다 → 그
    /// 파일을 디스크에 만들면 실제 OS 감시(`pw_fs`) → `fs.changed` → 관리자 → `workspace/didChangeWatchedFiles` 로 서버가 알게 되어
    /// 정의가 그 파일로 풀린다. rust-analyzer 는 클라이언트가 감시 동적 등록을 알리면 스스로 감시하지 않는다(`files.watcher` 기본
    /// `client`) — 전달이 없으면 끝까지 풀리지 않는다.
    #[cfg(unix)]
    pub async fn watched_files(server: LiveServer) {
        use pw_core::Filter;
        use pw_core::task::{TOPIC_FINISHED, TOPIC_PROGRESS};
        use pw_lsp::methods::LspInstanceState;
        use serde_json::{Value, json};

        use super::common::env::{Env, EnvOptions};

        let env = Env::new(EnvOptions {
            isolated_shell: false,
            files: true,
            ..EnvOptions::default()
        });
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust"),
            &env.root,
        );
        // 이 행 몫의 임시 파일(픽스처 규칙: 임시 복사본의 lib.rs 끝에 줄을 더한다).
        let probe = env.file(
            "src/m4_05b_watch.rs",
            "//! 파일 감시 탐침: 디스크에 늦게 생기는 모듈의 함수를 부른다.\npub fn probe() -> u32 {\n    crate::m4_05b_late::/*@watch.late*/late()\n}\n",
        );
        let lib = env.root.join("src/lib.rs");
        let mut text = std::fs::read_to_string(&lib).unwrap();
        text.push_str("pub mod m4_05b_watch;\npub mod m4_05b_late;\n");
        std::fs::write(&lib, text).unwrap();
        env.configure(
            server.server_id(),
            &server.program(),
            &server
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        );
        let _lease = env.watch_project().await;
        let mut events = env
            .bus
            .subscribe(Filter::all().topics([TOPIC_PROGRESS, TOPIC_FINISHED]));
        let probe_text = std::fs::read_to_string(&probe).unwrap();
        let mut win = env.window();
        let opened = win.open(&env, &probe, "Rust", &probe_text).await;
        let doc = opened["docId"].as_str().unwrap().to_owned();
        let ready = common::eventually(Duration::from_secs(60), || {
            env.state_of(server.server_id()) == Some(LspInstanceState::Ready)
        })
        .await;
        assert!(ready, "{:?}", env.states());
        settle_progress(&env, &mut events).await;

        let anchor = "/*@watch.late*/";
        let at = probe_text.find(anchor).unwrap() + anchor.len();
        let offset = probe_text[..at].encode_utf16().count();
        let mut definition = async || -> Vec<Value> {
            let found = win
                .call(
                    "test.lsp.feature",
                    json!({ "docId": doc, "feature": "definition", "method": "textDocument/definition",
                            "offset": offset, "version": 0 }),
                )
                .await;
            match found {
                Err(e) if e.data.as_ref().is_some_and(|d| d["code"] == -32801) => Vec::new(),
                other => match other.unwrap()["result"].clone() {
                    Value::Array(list) => list,
                    Value::Null => Vec::new(),
                    one => vec![one],
                },
            }
        };
        let before = definition().await;
        assert!(
            before.is_empty(),
            "resolved before the file existed: {before:?}"
        );

        let late = env.file("src/m4_05b_late.rs", "pub fn late() -> u32 {\n    7\n}\n");
        let started = std::time::Instant::now();
        let mut targets = Vec::new();
        while started.elapsed() < Duration::from_secs(60) {
            targets = definition().await;
            if !targets.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        eprintln!(
            "late() goes to {targets:?} {:?} after the file appeared",
            started.elapsed()
        );
        let uri = pw_lsp::uri::file_uri(&late).unwrap();
        assert!(
            targets
                .iter()
                .any(|t| t["uri"] == uri || t["targetUri"] == uri),
            "{targets:?}"
        );
        win.session.close();
        assert!(
            common::eventually(Duration::from_secs(3), || env
                .manager
                .pids(&env.id)
                .is_empty())
            .await
        );
    }

    /// 진행이 하나 이상 나왔다가 모두 끝나고 3 s 동안 새 진행이 없을 때까지 기다린다(서버가 색인을 마친다).
    #[cfg(unix)]
    async fn settle_progress(
        env: &super::common::env::Env,
        events: &mut pw_core::bus::Subscription,
    ) {
        use std::collections::BTreeSet;

        use pw_core::bus::{Received, Scope};
        use pw_core::task::TOPIC_PROGRESS;

        let mut started = BTreeSet::new();
        let mut finished = BTreeSet::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        let quiet = Duration::from_secs(3);
        loop {
            let settled = !started.is_empty()
                && started.is_subset(&finished)
                && env.tasks.running(Some(&env.id)).is_empty();
            let wait = if settled {
                (tokio::time::Instant::now() + quiet).min(deadline)
            } else {
                deadline
            };
            let Ok(Some(Received::Event(event))) =
                tokio::time::timeout_at(wait, events.recv()).await
            else {
                assert!(
                    settled,
                    "progress did not finish: started {started:?}, finished {finished:?}"
                );
                return;
            };
            assert_eq!(event.scope, Scope::Project(env.id.clone()));
            let id = event.payload["taskId"].as_u64().unwrap();
            if event.topic == TOPIC_PROGRESS {
                started.insert(id);
            } else {
                assert_eq!(event.payload["status"], "completed", "{}", event.payload);
                finished.insert(id);
            }
        }
    }

    /// 시험 프로세스의 `rustc` 툴체인에 rust-src 가 있는가.
    #[cfg(unix)]
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

    #[cfg(unix)]
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

    /// scripts/lsp-live.toml 의 `[capabilities]` 에서 그 서버의 기대 능력.
    fn expected_capabilities(server: LiveServer) -> Vec<String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/lsp-live.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        let table: toml::Table = text.parse().unwrap();
        table["capabilities"][server.name()]
            .as_array()
            .unwrap_or_else(|| panic!("{} has no [capabilities] {}", path.display(), server.name()))
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }

    pub async fn initialize_and_shutdown(server: LiveServer) {
        let p = common::project();
        // 빈 작업 공간이면 충분하다 — 픽스처 프로젝트(crates/pw-lsp/tests/fixtures/)는 M4-05 의 몫이다.
        let log =
            ServerLog::open(&lsp_log_dir(&p.state), server.server_id(), "live", true).unwrap();
        // 서버를 돌리는 런타임(정의의 `requires` — jdtls 의 JDK 25, basedpyright 의 python)을 서버 관리자와 같은 함수로 찾아 넣는다
        // (PATH 맨 앞 · java 면 `JAVA_HOME` — `ServerSetup::with_runtime` 과 같은 규칙).
        let mut env = RuntimeEnv::process();
        let extensions =
            pw_ext::Registry::from_roots(&semver::Version::new(0, 0, 0), pw_ext::builtin::roots());
        let (_, def) = extensions
            .language_server(server.server_id())
            .unwrap_or_else(|| panic!("no bundled definition {}", server.server_id()));
        for req in &def.requires {
            let found = pw_lsp::requires::find_runtime(req, &env).unwrap_or_else(|| {
                panic!(
                    "{}: no runtime for {:?} {} on this machine",
                    server.executable(),
                    req.runtime,
                    req.version
                )
            });
            let mut dirs = vec![found.bin.clone()];
            dirs.extend(std::env::split_paths(&env.path).filter(|d| *d != found.bin));
            env.path = std::env::join_paths(dirs).unwrap();
            if req.runtime == pw_ext::LspServerRuntime::Java {
                env.vars
                    .insert("JAVA_HOME".into(), found.home.clone().into_os_string());
            }
        }
        let mut extra_env = BTreeMap::new();
        // jdtls 의 작업 공간 색인 폴더(덮어쓰기 정의의 런처가 읽는다)를 시험 폴더 안에.
        extra_env.insert(
            "JDTLS_DATA".into(),
            p.base.join("jdtls-data").into_os_string(),
        );
        let spec = LaunchSpec {
            server_id: server.server_id().to_owned(),
            program: server.program(),
            args: server.args(),
            cwd: p.root.clone(),
            env,
            extra_env,
        };
        let lsp = LanguageServer::spawn(
            &p.grant,
            spec,
            ServerOptions {
                instance: "live".into(),
                log: Some(log),
                ..ServerOptions::default()
            },
        )
        .await
        .unwrap();
        let pid = lsp.pid().unwrap();
        // 시한은 서버 관리자와 같이 정의의 `initialize_timeout`(jdtls · kotlin-lsp 60 s, 나머지 10 s).
        let mut init = InitializeOptions::new(&p.root);
        init.timeout = def.initialize_timeout;
        let result = lsp
            .initialize(init)
            .await
            .unwrap_or_else(|e| panic!("{} initialize: {e}", server.executable()));
        let info = result
            .server_info
            .map(|i| format!("{} {:?}", i.name, i.version));
        eprintln!("{}: {info:?}", server.executable());
        let advertised = lsp.capabilities_json();
        let missing: Vec<String> = expected_capabilities(server)
            .into_iter()
            .filter(|key| {
                advertised
                    .get(key)
                    .is_none_or(|v| v.is_null() || *v == false)
            })
            .collect();
        assert!(
            missing.is_empty(),
            "{} does not advertise {missing:?} (scripts/lsp-live.toml [capabilities]); it sent {advertised}",
            server.executable()
        );
        let stopped = lsp.shutdown().await;
        assert!(stopped.answered_shutdown, "{stopped:?}");
        assert!(stopped.elapsed < Duration::from_secs(2), "{stopped:?}");
        #[cfg(unix)]
        assert!(
            common::eventually(Duration::from_secs(1), || common::group_members(pid)
                .is_empty())
            .await,
            "left in the group: {:?}",
            common::group_members(pid)
        );
        #[cfg(not(unix))]
        let _ = pid;
    }
}

#[cfg(pw_live_lsp_rust_analyzer)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_initialize_and_shutdown() {
    live::initialize_and_shutdown(pw_lsp::live::LiveServer::RustAnalyzer).await;
}

#[cfg(pw_live_lsp_typescript_language_server)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_initialize_and_shutdown() {
    live::initialize_and_shutdown(pw_lsp::live::LiveServer::TypescriptLanguageServer).await;
}

#[cfg(all(unix, pw_live_lsp_rust_analyzer))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_document_sync() {
    live::document_sync(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        "src/calls.rs",
        "Rust",
        false,
    )
    .await;
}

/// 파일 감시 전달(M4-05b): 디스크에 새로 생긴 모듈 파일을 감시 전달로 알게 되어 정의가 풀린다.
#[cfg(all(unix, pw_live_lsp_rust_analyzer))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_watched_files() {
    live::watched_files(pw_lsp::live::LiveServer::RustAnalyzer).await;
}

/// 픽스처가 rust-src 없이도 돈다(fixtures/README.md 「rust-src 없이도 돈다」 · 계획 리뷰 H2 · M4-05 리뷰 L4).
#[cfg(all(unix, pw_live_lsp_rust_analyzer))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_rust_analyzer_fixture_without_rust_src() {
    live::document_sync(
        pw_lsp::live::LiveServer::RustAnalyzer,
        "rust",
        "src/calls.rs",
        "Rust",
        true,
    )
    .await;
}

#[cfg(all(unix, pw_live_lsp_typescript_language_server))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_typescript_language_server_document_sync() {
    live::document_sync(
        pw_lsp::live::LiveServer::TypescriptLanguageServer,
        "typescript",
        "src/calls.ts",
        "TypeScript",
        false,
    )
    .await;
}

#[cfg(pw_live_lsp_basedpyright)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_basedpyright_initialize_and_shutdown() {
    live::initialize_and_shutdown(pw_lsp::live::LiveServer::Basedpyright).await;
}

#[cfg(pw_live_lsp_ruff)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_ruff_initialize_and_shutdown() {
    live::initialize_and_shutdown(pw_lsp::live::LiveServer::Ruff).await;
}

#[cfg(pw_live_lsp_jdtls)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_jdtls_initialize_and_shutdown() {
    live::initialize_and_shutdown(pw_lsp::live::LiveServer::Jdtls).await;
}

#[cfg(pw_live_lsp_kotlin_lsp)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_lsp_kotlin_lsp_initialize_and_shutdown() {
    live::initialize_and_shutdown(pw_lsp::live::LiveServer::KotlinLsp).await;
}
