//! 실 서버 시험(15-m4-plan.md M4-01 ⑦ · CI `lsp-live`): 서버마다 `initialize` → 기대 능력(scripts/lsp-live.toml 의 `[capabilities]`) →
//! `shutdown`. 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 시험이 목록에 없다 — `src/live.rs` · `live_cfg.rs` 머리 주석.

mod common;

#[cfg(any(pw_live_lsp_rust_analyzer, pw_live_lsp_typescript_language_server))]
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
    #[cfg(unix)]
    pub async fn document_sync(server: LiveServer, fixture: &str, file: &str, language: &str) {
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
        env.configure(
            server.server_id(),
            &server.program(),
            &server
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        );
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
        let spec = LaunchSpec {
            server_id: server.server_id().to_owned(),
            program: server.program(),
            args: server.args(),
            cwd: p.root.clone(),
            env: RuntimeEnv::process(),
            extra_env: BTreeMap::new(),
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
        let result = lsp
            .initialize(InitializeOptions::new(&p.root))
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
    )
    .await;
}
