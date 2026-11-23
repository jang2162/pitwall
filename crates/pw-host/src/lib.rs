//! Host 조립(18-m7-plan M7-01, 메인 판정 M7-P1). 기능 크레이트들의 메서드 등록과 그 사이의 붙임(레지스트리 · 버스 · 작업 ·
//! 설정)을 한 자리에서 해 [`LocalHost`] 하나를 만든다. Desktop(`apps/desktop/src-tauri`)과 Relay 가 같은 조립을 쓰고 부품
//! ([`HostParts`])만 다르게 넣는다 — 조립이 두 벌이면 Relay 의 메서드 목록이 말없이 갈린다.
//!
//! 이 크레이트는 tauri 를 모른다(`tests/deps.rs` 가 의존 그래프로 지킨다). 뒤에서 도는 일은 부품의 tokio 핸들([`HostParts::tokio`])에
//! 띄우고, 창이 하는 판정(설정 창인가)과 앱 전용 값(「Shell 명령 설치」 의 대상)은 부르는 쪽이 부품으로 준다.

use std::path::PathBuf;
use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_core::{HostId, LocalHost};
use pw_diag::Diagnostics;
use pw_ext::Registry;
use pw_fs::Fs;
use pw_i18n::I18n;
use pw_open_with::OpenWith;
use pw_open_with::cli::CliSource;
use pw_runtime::{LoginShell, Runtime, RuntimeHost};

pub mod db;
pub mod trust;

/// 비밀 저장소(DB 연결의 비밀번호 등). Desktop 은 OS 키체인(`pw_db::secret::KeyringStore`), Relay 는 링크의 Desktop 에 묻는
/// 구현(M7-12)을 넣는다. trait 의 자리는 `pw-db` 다 — 그 크레이트가 비밀을 읽으므로 여기 두면 의존이 거꾸로 돈다.
pub use pw_db::secret::SecretStore;

/// 「Orca 에서 Pitwall 열기」 안내의 검사(`cli.status`)가 보는 값 — 「Shell 명령 설치」 의 대상과 이 바이너리. 설치는 앱의 일이라
/// 부르는 쪽이 계산해 넘긴다(`pw_open_with::cli::CliCheck::new`).
pub struct CliParts {
    /// 설치 대상 경로(`/usr/local/bin/pitwall` 등). 계산하지 못했으면 그 까닭.
    pub target: Result<PathBuf, String>,
    /// 지금 도는 바이너리.
    pub app: Result<PathBuf, String>,
    /// 설치의 원본이 될 수 있는가(임시 위치 · 개발 빌드면 아니다).
    pub source: CliSource,
}

/// 부르는 쪽이 만들어 넘기는 부품. 여기 든 것은 Host 밖(창 · 소켓 · 앱 메뉴)과 나눠 쓰거나, 어디서 뜨느냐(Desktop · Relay)에 따라
/// 다른 것이다.
pub struct HostParts {
    /// Host 의 id(Desktop 은 `local`).
    pub id: HostId,
    pub paths: AppPaths,
    pub diagnostics: Arc<Diagnostics>,
    pub extensions: Arc<Registry>,
    /// UI 언어. Host 를 만든 뒤 그 전역 설정을 붙인다.
    pub i18n: Arc<I18n>,
    /// 파일 서비스. Desktop 은 Finder 드롭 들여오기에도 쓴다.
    pub fs: Arc<Fs>,
    /// git askpass. Desktop 은 주 인스턴스가 된 뒤 그 소켓을 연다.
    pub askpass: pw_git::write::Askpass,
    /// 열기 요청 자리(에이전트의 `project_open`). 창을 여는 쪽은 부르는 쪽이 나중에 붙인다.
    pub open_requests: pw_core::open::OpenRequests,
    /// MCP 세션 등록부 — `mcp.*` RPC 와 MCP 소켓이 같은 것을 쓴다.
    pub mcp_sessions: pw_mcp::sessions::Sessions,
    /// DB 연결의 비밀 저장소.
    pub secrets: Arc<dyn SecretStore>,
    /// DB 드라이버 등록부. 보통 [`db::drivers`].
    pub db_drivers: pw_db::DriverRegistry,
    /// `cli.status` 가 보는 값.
    pub cli: CliParts,
    /// 창 라벨이 설정 창인가 — 도구 설치(`tools.install` 등)는 설정 창 · 그 프로젝트 창만 한다.
    pub settings_window: pw_tools::install::SettingsWindow,
    /// 뒤에서 도는 일(색인 붙이기 · 파일 따라가기 · 감사 정리 등)을 띄울 tokio 런타임.
    pub tokio: tokio::runtime::Handle,
}

/// 조립한 Host 와, 시험이 실행 길에 끼워 넣는 자리(`DbSessions::test_hooks`)를 쓰는 DB 세션.
pub struct Assembled {
    pub host: LocalHost,
    pub db_sessions: Arc<pw_db::DbSessions>,
    /// Host 에 남은 일(실행 · 터미널)과 언어 서버. Relay 데몬의 유휴 판정이 본다(18-m7-plan M7-04 ④).
    pub activity: Activity,
    /// 원격 호스트 관리자(`remote.*` — 18-m7-plan M7-08). Desktop 만 `Some` 이다. 앱이 setup 에서 SSH 백엔드(`pw_remote::ssh`) ·
    /// `HostRouter` 를 붙인다 — 붙이기 전에는 목록이 비고 붙기를 할 수 없다.
    pub remote: Option<pw_remote::RemoteHosts>,
}

/// Host 에 남은 일. 복제하면 같은 것을 본다.
#[derive(Clone)]
pub struct Activity {
    terminals: Arc<pw_run::Terminals>,
    runs: pw_run::runs::Runs,
    lsp: pw_lsp::LspManager,
}

impl Activity {
    /// 끝나지 않은 실행(디버그 세션 포함 — 실행 하나다)의 수.
    pub fn runs(&self) -> usize {
        self.runs.live_count()
    }

    /// 열린 터미널의 수.
    pub fn terminals(&self) -> usize {
        self.terminals.count()
    }

    /// 떠 있는 언어 서버를 모두 끝낸다(끝낸 수).
    pub async fn stop_language_servers(&self) -> usize {
        self.lsp.stop_all().await
    }
}

/// 어느 앱의 Host 인가. 메서드 목록이 갈리는 곳은 여기 하나다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Desktop,
    Relay,
}

/// Relay 의 Host 에서 걷어 내는 메서드 갈래(18-m7-plan M7-04).
/// - `bundle.` — 묶음은 Desktop 의 것이다. 묶음 저장은 사람의 Desktop 레지스트리에 있고, 원격 머신의 레지스트리에 묶음을 만들
///   까닭이 없다(M7-10 리뷰 권고 6).
/// - `openWith.` — 「다른 앱으로 열기」 는 그 머신에서 GUI 프로그램을 띄운다. 창 없는 원격 머신에서는 뜻이 없다(M7-04 리뷰 권고 4).
/// - `cli.` — 「Shell 명령 설치」 안내(`cli.status`)는 Desktop 앱의 것이다. Relay 는 설치할 Shell 명령이 없다.
/// - `remote.` — 원격 호스트 · 연결 관리(`pw_remote` — 18-m7-plan M7-08)는 Desktop 의 것이다. Relay 는 다른 머신에 붙지 않는다.
pub const RELAY_EXCLUDED_PREFIXES: &[&str] = &["bundle.", "cli.", "openWith.", "remote."];

/// 부품으로 Host 를 조립한다. 메서드를 모두 등록하고, Host 의 설정 · 레지스트리가 있으면 그것을 기능들에 붙인다.
pub fn assemble(parts: HostParts) -> Assembled {
    assemble_as(parts, Flavor::Desktop)
}

/// Relay 데몬(`pitwall-relay` — 18-m7-plan M7-04)의 Host. [`assemble`] 과 같은 조립에서 [`RELAY_EXCLUDED_PREFIXES`] 만 뺀다.
pub fn assemble_relay(parts: HostParts) -> Assembled {
    assemble_as(parts, Flavor::Relay)
}

fn assemble_as(parts: HostParts, flavor: Flavor) -> Assembled {
    let HostParts {
        id,
        paths,
        diagnostics,
        extensions,
        i18n,
        fs,
        askpass,
        open_requests,
        mcp_sessions,
        secrets,
        db_drivers,
        cli,
        settings_window,
        tokio: runtime_handle,
    } = parts;
    let methods = Arc::clone(&i18n);
    // 앱이 띄우는 프로세스의 환경. 로그인 셸은 처음 쓸 때 돈다 — 시작을 늦추지 않는다.
    // 미신뢰 프로젝트에서는 프로젝트 코드를 돌리는 제공자를 고르지 않는다(M1-05). 레지스트리는 Host 를 만든 뒤 붙인다.
    let trust = trust::TrustGate::default();
    // 신뢰한 프로젝트의 mise 설정이 고정한 도구 버전을 로그인 셸 환경 위에 덮는다(M1-16).
    let runtime = Arc::new(
        Runtime::new(LoginShell::for_this_user())
            .with_provider(Arc::new(pw_runtime::Mise::default()))
            .with_trust(trust.check()),
    );
    let runtime_methods = Arc::clone(&runtime);
    let lang = Arc::new(pw_lang::methods::Lang::new(&extensions));
    let lang_methods = Arc::clone(&lang);
    let open_with = Arc::new(OpenWith::new(Arc::clone(&extensions), Arc::clone(&runtime)));
    let open_with_methods = Arc::clone(&open_with);
    // 언어 서버 정의 · 해석 · 설정(`lsp.servers.list`, M4-02). 설치본 찾기는 설치기(M4-04)가 붙인다.
    let lsp_servers = Arc::new(pw_lsp_servers::LspServers::new(
        Arc::clone(&extensions),
        Arc::clone(&runtime),
    ));
    let lsp_servers_methods = Arc::clone(&lsp_servers);
    // 언어 서버 관리자(M4-05): 그 언어의 문서를 처음 열 때 서버를 띄우고, 창이 닫히면 그 창이 띄운 서버를 끝낸다.
    let lsp = pw_lsp::LspManager::new(
        Arc::clone(&lsp_servers),
        Arc::clone(&extensions),
        pw_lsp::ManagerOptions::default(),
    );
    let lsp_methods = lsp.clone();
    let lsp_activity = lsp.clone();
    // 도구 registry(`tools.registry.*`, M4-03): 내장 스냅숏으로 시작하고, 처음 불릴 때 캐시를 읽고 가져오기를 뒤에서 한다.
    let tools_registry = Arc::new(pw_tools::ToolsRegistry::new());
    let tools_registry_methods = Arc::clone(&tools_registry);
    // 도구 설치기(`tools.install` · `update` · `rollback` · `uninstall` · `list` · `updates` · `log`, M4-04). `$DATA/tools` 에 설치하고
    // 설치본 찾기(`ToolLocator`)를 언어 서버 해석에 붙인다. 설치는 설정 창 · 그 프로젝트 창만 한다.
    let tools_installer = Arc::new(pw_tools::ToolsInstaller::new(Arc::clone(&tools_registry)));
    let tools_installer_methods = Arc::clone(&tools_installer);
    let fs_methods = Arc::clone(&fs);
    let fs_watch = pw_fs::watch::Watch::new(Arc::clone(&fs));
    let fs_watch_methods = fs_watch.clone();
    // 파일 목록 인덱스(`search.files`)는 감시하는 루트마다 목록을 둔다.
    let fs_index = pw_fs::index::FileIndex::new(Arc::clone(&fs));
    let fs_index_methods = fs_index.clone();
    // 언어 서버의 파일 감시 전달(M4-05b)이 다시 만든 목록을 받는다.
    let fs_index_lsp = fs_index.clone();
    fs_watch.attach_index(fs_index.clone());
    // 심볼 인덱스(`index.*`, M4-15)는 파일 목록 인덱스의 목록을 따라간다(같은 수명 — 감시하는 루트마다).
    let symbol_index = pw_index::SymbolIndex::new(Arc::clone(&fs), Arc::clone(&extensions));
    fs_index.observe(symbol_index.observer());
    let symbol_index_methods = symbol_index.clone();
    // 「Orca 에서 Pitwall 열기」 안내의 검사(`cli.status`): 「Shell 명령 설치」 의 대상과 이 바이너리.
    let cli_check = Arc::new(pw_open_with::cli::CliCheck::new(
        Arc::clone(&runtime),
        cli.target,
        cli.app,
        cli.source,
    ));
    // 내장 터미널(M1-08). 창이 닫히면 그 연결의 터미널 스트림이 버려져 셸의 프로세스 트리가 끝난다.
    let terminals = Arc::new(pw_run::Terminals::new(Arc::clone(&runtime)));
    let terminal_methods = Arc::clone(&terminals);
    let terminals_activity = Arc::clone(&terminals);
    // 실행 구성(`run.config.*`, M6-01). `.pitwall/run/*.toml` 을 읽고 쓴다.
    let run_configs = Arc::new(pw_run::config::methods::RunConfigs::new());
    let run_config_methods = Arc::clone(&run_configs);
    // 실행 관리자(`run.*`, M6-02). 실행은 연결이 아니라 Host 의 것이다 — 창을 다시 불러와도 · 에이전트가 띄웠어도 같은 실행을 본다.
    // 프로젝트 창이 사라지면 그 프로젝트의 실행을 끝내고(ipc.rs), 앱이 끝날 때는 `pw_run::shutdown` 이 함께 끝낸다.
    let runs = pw_run::runs::Runs::new(Arc::clone(&runtime));
    let run_methods = runs.clone();
    let runs_activity = runs.clone();
    // 디버그 세션 · 브레이크포인트(`debug.*`, M6-17). 디버그 세션은 실행 하나다(실행 관리자의 종류 `debug`) — 어댑터 정의 · 해석은
    // 언어 서버와 같은 곳(`LspServers`, M6-16)을 쓴다. 끝내기는 실행의 끝내기를 따른다(창 닫기 · 앱 끝).
    let debugger = pw_debug::Debugger::new(
        runs.clone(),
        Arc::clone(&runtime),
        Arc::clone(&lsp_servers) as Arc<dyn pw_debug::Adapters>,
    );
    // 언어 서버가 여는 어댑터(java-debug — jdtls 의 `vscode.java.startDebugSession`, M6-19)는 서버 관리자에 명령을 보낸다. 관리자는
    // 그 세션의 프로젝트 · 루트에 떠 있는 인스턴스에만 보낸다.
    debugger.attach_language_servers(Arc::new(lsp.clone()) as Arc<dyn pw_lsp_servers::LspCommands>);
    let debugger_methods = debugger.clone();
    // 구문 강조(`syntax.*`). 문법 엔진은 처음 문서를 열 때 만든다.
    let syntax = Arc::new(pw_syntax::Syntax::new(Arc::clone(&extensions)));
    // git(`git.*`, M2). 읽기는 gix, 쓰기는 런타임 환경으로 띄우는 git CLI 다.
    let git = Arc::new(pw_git::Git::new(Arc::clone(&runtime)));
    let git_methods = Arc::clone(&git);
    // git 쓰기 실행기 · 진행 중 작업 · 창 질의(`git.operation*` · `git.prompts`, M2-06). 쓰기는 Host 의 Task 로 돈다.
    let git_write = Arc::new(pw_git::write::GitWrite::new(
        Arc::clone(&git),
        askpass,
        Arc::clone(&fs),
    ));
    let git_write_methods = Arc::clone(&git_write);
    // 충돌 데이터 · 해결(`git.conflicts` · `git.conflict.*`, M2-11). 해결은 쓰기 실행기로 돈다.
    let git_conflict_methods = Arc::clone(&git_write);
    // merge · cherry-pick · revert · rebase · reset(`git.merge` 등, M2-10). 쓰기 실행기로 돈다.
    let git_history_methods = Arc::clone(&git_write);
    // 커밋(`git.commit`, M2-08). 쓰기 실행기로 돈다.
    let git_commit_methods = Arc::clone(&git_write);
    // 체크아웃 · 브랜치 · 태그 · fetch · pull · push(`git.checkout` · `git.branch.*` · `git.tag.create` · `git.fetch` · `git.pull` ·
    // `git.push`, M2-09). 쓰기 실행기로 돈다.
    let git_branch_methods = Arc::clone(&git_write);
    let git_remote_methods = Arc::clone(&git_write);
    // 스테이징 · 되돌리기 · stash(`git.stage*` · `git.stash.*`, M2-07). 쓰기 실행기로 돈다.
    let git_stage_methods = Arc::clone(&git_write);
    // 작업 트리 상태 · ref(`git.status` · `git.refs`)와 저장소 감시(`git.watch`, M2-02).
    let git_status = Arc::new(pw_git::status::Status::new(Arc::clone(&git)));
    let git_status_methods = Arc::clone(&git_status);
    let git_watch = pw_git::watch::GitWatch::new(Arc::clone(&git));
    // DB 연결 설정 · 비밀 · 신뢰 게이트(`db.connection.*` · `db.secret.*`, M3-02). 비밀은 OS 키체인, env 참조는 런타임 환경.
    let db_connections = db::connections(&runtime, secrets);
    let db_connection_methods = Arc::clone(&db_connections);
    let db_ssh_host_methods = Arc::clone(&db_connections);
    // DB 세션 · 실행 파이프라인(`db.session.*` · `db.query.*` · `db.sql.classify` · `db.connection.test`, M3-05).
    let db_sessions = db::sessions(&db_connections, db_drivers.clone());
    // SSH 터널(M3-11) — git 쓰기와 같은 askpass · 창 질의를 쓴다.
    db::attach_tunnels(
        &db_sessions,
        &runtime,
        git_write.askpass(),
        git_write.prompts(),
        paths.state.join("db-ssh-tunnels"),
    );
    let db_session_methods = Arc::clone(&db_sessions);
    // 실행 구성의 `sql` 종류(`run.sql.start`, M6-09): 프로세스 없이 DB 실행 경로로, 실행 관리자의 실행 하나로 돈다.
    let sql_run_sessions = Arc::clone(&db_sessions);
    let sql_runs = runs.clone();
    let db_sessions_out = Arc::clone(&db_sessions);
    // MCP 상태(`mcp.sessions` · `mcp.tools.list` · `mcp.audit.list`, M5-10). 카탈로그 · 감사 로그는 Host 를 만든 뒤 붙인다.
    let mcp_status = Arc::new(pw_mcp::methods::Mcp::new(mcp_sessions));
    let mcp_status_methods = Arc::clone(&mcp_status);
    // 원격 호스트 · 연결 관리(`remote.*`, M7-08). Relay 는 아래에서 걷어 낸다(`RELAY_EXCLUDED_PREFIXES`).
    let remote = pw_remote::RemoteHosts::default();
    let remote_methods = remote.clone();
    let host = LocalHost::builder(id)
        .config(paths)
        .open_requests(open_requests)
        .methods(move |router| pw_diag::methods::register(router, diagnostics))
        .methods(move |router| pw_ext::methods::register(router, extensions))
        .methods(move |router| pw_i18n::methods::register(router, methods))
        .methods(move |router| pw_lang::methods::register(router, lang_methods))
        .methods(move |router| pw_open_with::methods::register(router, open_with_methods))
        .methods(move |router| pw_lsp_servers::methods::register(router, lsp_servers_methods))
        .methods(move |router| pw_lsp::register(router, &lsp_methods))
        .methods(move |router| pw_runtime::methods::register(router, runtime_methods))
        .methods(move |router| pw_fs::methods::register(router, fs_methods))
        .methods(move |router| pw_fs::watch::register(router, fs_watch_methods))
        .methods(move |router| pw_fs::index::register(router, fs_index_methods))
        .methods(move |router| pw_index::register(router, symbol_index_methods))
        .methods(move |router| pw_open_with::cli::register(router, cli_check))
        .methods(move |router| pw_run::methods::register(router, terminal_methods))
        .methods(move |router| pw_run::config::methods::register(router, run_config_methods))
        .methods(move |router| pw_run::runs::methods::register(router, run_methods))
        .methods(move |router| pw_debug::methods::register(router, debugger_methods))
        .methods(move |router| pw_syntax::methods::register(router, syntax))
        .methods(move |router| pw_git::methods::register(router, git_methods))
        .methods(move |router| pw_git::write::register(router, git_write_methods))
        .methods(move |router| pw_git::conflict::methods::register(router, git_conflict_methods))
        .methods(move |router| pw_git::history::register(router, git_history_methods))
        .methods(move |router| pw_git::commit::register(router, git_commit_methods))
        .methods(move |router| pw_git::branch::register(router, git_branch_methods))
        .methods(move |router| pw_git::remote::register(router, git_remote_methods))
        .methods(move |router| pw_git::stage::register(router, git_stage_methods))
        .methods(move |router| pw_git::status::register(router, git_status_methods))
        .methods(move |router| pw_git::watch::register(router, git_watch))
        .methods(pw_db_sql::split::register)
        .methods(pw_db_sql::literal::register)
        .methods(pw_db_sql::kv_command::register)
        .methods(move |router| pw_db::config::register(router, db_connection_methods))
        .methods(move |router| pw_db::register(router, db_session_methods))
        .methods(move |router| pw_db::run_sql::register(router, sql_run_sessions, sql_runs))
        .methods(move |router| pw_db::registry::register(router, db_drivers))
        .methods(move |router| pw_db::ssh_hosts::register(router, db_ssh_host_methods))
        .methods(move |router| pw_tools::methods::register(router, tools_registry_methods))
        .methods(move |router| {
            pw_tools::install::methods::register(router, tools_installer_methods)
        })
        .methods(move |router| pw_mcp::methods::register(router, mcp_status_methods))
        .methods(move |router| pw_remote::methods::register(router, &remote_methods))
        .methods(move |router| {
            if flavor == Flavor::Relay {
                for prefix in RELAY_EXCLUDED_PREFIXES {
                    router.remove_prefix(prefix);
                }
            }
        })
        .build();
    // 파일 감시(`fs.watch`)는 바뀐 경로를 버스로 낸다.
    fs_watch.attach(host.bus().clone());
    git_write.attach(host.tasks().clone());
    // 인덱스는 버스의 `fs.changed` 로 목록을 갱신하고, 목록을 만드는 동안을 작업으로 보인다.
    runtime_handle.spawn(fs_index.attach(host.bus(), host.tasks().clone()));
    // 심볼 색인을 처음 만드는 동안을 작업으로 보인다. 런타임 안에서 붙인다 — 목록은 런타임 밖 스레드에서도 오므로 그 런타임을
    // 붙잡아 둔다.
    let symbol_tasks = host.tasks().clone();
    runtime_handle.spawn(async move { symbol_index.attach(symbol_tasks) });
    // 큰 저장소의 전체 status 는 작업으로 보인다.
    git_status.attach(host.tasks().clone());
    if let Some(config) = host.config() {
        i18n.attach_config(config.global.clone());
        // registry 캐시(`$CACHE/registry/`)와 설정 `[tools] registry`.
        tools_registry.attach(config.paths.cache.clone(), config.global.clone());
        // 외부 도구 실행(⌥F1 · ⌘⌥E)이 전역 설정 · 프로젝트 레지스트리를 쓴다.
        if let Some(projects) = host.registry() {
            // MCP 상태: 세션 이벤트는 Host 버스로, 툴 목록은 세션과 같은 카탈로그(이 Host 가 처리하는 툴)로.
            mcp_status.sessions().attach(host.bus().clone());
            let mcp_audit = Arc::new(pw_mcp::audit::AuditLog::under_state(&config.paths.state));
            mcp_status.attach(pw_mcp::methods::HostParts {
                paths: config.paths.clone(),
                global: config.global.clone(),
                projects: projects.clone(),
                catalog: Arc::new(pw_mcp::Catalog::linked().routed_by(host.router())),
                audit: Arc::clone(&mcp_audit),
            });
            // 감사 로그의 보관 정리(16-m5-plan M5-11 — 결정 M5-D10): 지금 한 번, 그 뒤 하루에 한 번. 보관 일수는 전역 설정의
            // `[mcp.audit] retention_days` 만 읽는다. 앱이 끝나면 함께 끝난다.
            runtime_handle.spawn(pw_mcp::audit::keep_pruned(
                mcp_audit,
                config.global.clone(),
                tokio_util::sync::CancellationToken::new(),
            ));
            // 파일 RPC(`fs.*`)는 요청의 프로젝트 루트를 레지스트리에서 찾는다.
            fs.attach(projects.clone());
            // 터미널은 전역 설정(`[terminal] shell`)과 레지스트리(프로젝트 · cwd)를 쓴다.
            terminals.attach(pw_run::HostParts {
                global: config.global.clone(),
                projects: projects.clone(),
            });
            // 실행 구성은 설정 경로(개인 전용 프로젝트의 폴더)와 레지스트리를 쓴다.
            run_configs.attach(pw_run::config::methods::HostParts {
                paths: config.paths.clone(),
                projects: projects.clone(),
            });
            // 프로세스를 띄우는 태스크 감지(mise · gradle · maven — M6-05)는 신뢰한 프로젝트에서 그 런타임 환경으로 돌고, 「태스크
            // 읽기」 는 Host 작업(진행 · 취소)이다.
            run_configs.attach_detect(pw_run::config::methods::DetectHost::new(
                Arc::clone(&runtime),
                host.tasks().clone(),
            ));
            // 감지 출처(`package.json` · `Makefile` …)가 바뀌면 그 프로젝트의 감지만 다시 하고 달라졌으면 `config.changed` 를 낸다.
            runtime_handle.spawn(pw_run::detect::follow_files(
                pw_run::detect::subscribe_files(host.bus()),
                run_configs.seen(),
                host.bus().clone(),
            ));
            // 실행은 구성을 읽고(설정 경로 · 레지스트리) `run.started` · `run.exited` 를 버스로 낸다.
            runs.attach(pw_run::runs::HostParts {
                paths: config.paths.clone(),
                projects: projects.clone(),
                bus: host.bus().clone(),
            });
            // 화면에서 저장한 실행 구성은 창의 실행과 같은 풀기로 「사람이 본 구성」 이 된다(M6-07 ④).
            run_configs.attach_runs(runs.clone());
            // 디버그는 실행 구성 · 레지스트리 · 앱 데이터(`breakpoints.json`)와 이벤트 버스를 쓴다.
            debugger.attach(pw_debug::HostParts {
                paths: config.paths.clone(),
                projects: projects.clone(),
                bus: host.bus().clone(),
            });
            // git 은 전역 설정(`[git] path`) · 레지스트리를 쓰고, 쓰기를 끈 알림을 버스로 낸다.
            git.attach(pw_git::HostParts {
                global: config.global.clone(),
                projects: projects.clone(),
                bus: host.bus().clone(),
            });
            open_with.attach(pw_open_with::HostParts {
                paths: config.paths.clone(),
                global: config.global.clone(),
                projects: projects.clone(),
            });
            lsp_servers.attach(pw_lsp_servers::HostParts {
                paths: config.paths.clone(),
                global: config.global.clone(),
                projects: projects.clone(),
            });
            tools_installer.attach(pw_tools::install::InstallerHost {
                tools_dir: config.paths.data.join("tools"),
                state_dir: config.paths.state.clone(),
                tasks: host.tasks().clone(),
                toolchains: Arc::new(pw_tools::install::RuntimeToolchains {
                    runtime: Arc::clone(&runtime),
                    projects: projects.clone(),
                }),
                settings_window,
            });
            if let Some(locator) = tools_installer.locator() {
                lsp_servers.set_tools(Arc::new(locator));
            }
            // 서버 관리자: 진행은 작업으로, 상태 · 신뢰 해제 · 런타임 · 설정 변경은 버스로. 시작할 때 지난 Host 가 강제로 끝나며
            // 남긴 서버 그룹을 거둔다(기록부 판정 — `pw_lsp::ledger`).
            lsp.attach(pw_lsp::HostParts {
                projects: projects.clone(),
                bus: host.bus().clone(),
                tasks: host.tasks().clone(),
                global: config.global.clone(),
                state_dir: config.paths.state.clone(),
                files: Some(fs_index_lsp.clone()),
            });
            runtime_handle.spawn(
                lsp.clone()
                    .follow(pw_lsp::LspManager::subscribe(host.bus())),
            );
            // 도구를 지우기 전에 그 설치본으로 뜬 언어 서버를 멈춘다(M4-04 의 `ToolUsers`).
            tools_installer.set_users(Arc::new(LspToolUsers(lsp.clone())));
            let reaper = lsp.clone();
            runtime_handle.spawn_blocking(move || {
                let reaped = reaper.reap_orphans();
                if reaped > 0 {
                    tracing::info!(reaped, "ended language servers left by an earlier run");
                }
            });
            runtime.attach(RuntimeHost {
                projects: projects.clone(),
                bus: host.bus().clone(),
                paths: config.paths.clone(),
            });
            // 버전 파일(`mise.toml` 등)이 바뀌면 그 프로젝트의 런타임 환경을 다시 계산한다(파일 감시 `fs.changed`).
            runtime_handle.spawn(pw_runtime::follow_files(
                pw_runtime::subscribe_files(host.bus()),
                Arc::clone(&runtime),
            ));
            // 언어 판별(`lang.detect`)이 설정 매핑 · 프로젝트 레지스트리를 쓴다.
            lang.attach(pw_lang::methods::HostParts {
                paths: config.paths.clone(),
                global: config.global.clone(),
                projects: projects.clone(),
            });
            // DB 연결 설정은 레지스트리(프로젝트 루트 · 신뢰)를 쓰고, `.pitwall/` 이 바뀌면(파일 감시) `config.changed` 를 낸다.
            db_connections.attach(pw_db::config::HostParts {
                paths: config.paths.clone(),
                projects: projects.clone(),
                bus: host.bus().clone(),
            });
            runtime_handle.spawn(pw_db::config::follow_files(
                pw_db::config::subscribe_files(host.bus()),
                db_connections,
            ));
            // 세션은 전역 설정의 `[db]` 항목(그리드 보유 행 · 바이트 상한, 셀 상한, 타임아웃 등 — `pw_db::settings`)을 읽고, 연결
            // 설정이 바뀌면(`config.changed`) 그 세션을 「설정 바뀜」 으로 둔다.
            db_sessions.attach_settings(config.global.clone());
            runtime_handle.spawn(pw_db::session::follow_config(
                pw_db::session::subscribe_config(host.bus()),
                db_sessions,
            ));
            trust.attach(projects.clone());
            runtime_handle.spawn(trust::follow(
                trust::subscribe(host.bus()),
                projects.clone(),
                runtime,
            ));
        }
    }
    let remote = (flavor == Flavor::Desktop).then(|| {
        remote.attach_host(
            host.registry().cloned(),
            host.bus().clone(),
            host.config().map(|c| c.global.clone()),
        );
        remote
    });
    Assembled {
        host,
        remote,
        db_sessions: db_sessions_out,
        activity: Activity {
            terminals: terminals_activity,
            runs: runs_activity,
            lsp: lsp_activity,
        },
    }
}

/// 도구 제거(`tools.uninstall`) 전에 그 패키지의 설치본으로 뜬 언어 서버를 끝낸다(`pw_tools::install::ToolUsers` — M4-05).
struct LspToolUsers(pw_lsp::LspManager);

impl pw_tools::install::ToolUsers for LspToolUsers {
    fn stop(
        &self,
        package: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let package = package.to_owned();
        Box::pin(async move { self.0.stop_installed(&package).await })
    }
}
