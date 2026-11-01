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
