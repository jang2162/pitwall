// 릴리스 빌드에서 Windows 가 콘솔 창을 함께 띄우지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_core::{HostRouter, LocalHost, Scope};
use pw_diag::Diagnostics;
use pw_ext::{LoadOptions, Registry};
use pw_fs::Fs;
use pw_i18n::I18n;
use tauri::Manager as _;

mod agent_open;
mod app_commands;
mod approval_notice;
mod cli;
mod clipboard;
mod config_log;
mod db;
mod debug_attention;
mod detach;
mod diag;
mod drop_import;
mod i18n;
mod instance;
mod ipc;
mod launch;
mod mcp;
#[cfg(test)]
mod mcp_tests;
mod menu;
mod perf;
#[cfg(feature = "perf-fake-db")]
mod perf_fake_db;
mod remote;
mod remote_open;
mod shell_command;
mod tool_cli;
mod update;
mod windows;

/// 데스크톱 프로세스 안의 Host 의 id. 프론트엔드가 프로젝트 없이 연결할 때 이 id 로 고른다(src/App.tsx 의 `LOCAL_HOST`).
const LOCAL_HOST_ID: &str = "local";

/// 시험이 쓰는 앱의 LocalHost — [`local_host_with`] 에 여는 쪽이 붙지 않은 열기 요청 자리를 준 것이다(`project_open` 은 목록에
/// 있고, 부르면 「앱이 아직 준비되지 않았다」 로 끝난다). 생성물과 등록된 메서드를 맞춰 보는 시험도 이것을 쓴다.
#[cfg(test)]
fn local_host(
    paths: AppPaths,
    diagnostics: Arc<Diagnostics>,
    extensions: Arc<Registry>,
    i18n: Arc<I18n>,
    fs: Arc<Fs>,
    askpass: pw_git::write::Askpass,
) -> LocalHost {
    local_host_with(
        paths,
        diagnostics,
        extensions,
        i18n,
        fs,
        askpass,
        pw_core::open::OpenRequests::default(),
        pw_mcp::sessions::Sessions::default(),
    )
    .0
}

/// [`local_host`] 와 같고 그 Host 의 DB 세션도 돌려준다 — 시험이 실행 길에 끼워 넣는 자리(`DbSessions::test_hooks`)를 쓴다.
/// `mcp_sessions` 는 시험의 MCP 세션들이 쓰는 등록부와 같은 것을 넘긴다 — 앱처럼 `mcp.approval.answer` RPC 가 그 승인 허브에 닿는다.
#[cfg(test)]
#[allow(clippy::too_many_arguments)] // local_host_with 와 같다.
fn local_host_and_db(
    paths: AppPaths,
    diagnostics: Arc<Diagnostics>,
    extensions: Arc<Registry>,
    i18n: Arc<I18n>,
    fs: Arc<Fs>,
    askpass: pw_git::write::Askpass,
    mcp_sessions: pw_mcp::sessions::Sessions,
) -> (LocalHost, Arc<pw_db::DbSessions>) {
    let (host, db_sessions, _) = local_host_parts(
        paths,
        diagnostics,
        extensions,
        i18n,
        fs,
        askpass,
        pw_core::open::OpenRequests::default(),
        mcp_sessions,
    );
    (host, db_sessions)
}

/// 앱이 쓰는 LocalHost.
/// UI 언어(`i18n`)는 Host 를 만든 뒤 그 전역 설정을 붙인다.
/// 파일 서비스(`fs`)는 Finder 드롭 들여오기(drop_import.rs)도 쓰므로 부르는 쪽이 만든다.
/// git askpass(`askpass`)도 부르는 쪽이 만든다 — 주 인스턴스가 된 뒤(setup)에 그 소켓을 연다.
/// 열기 요청 자리(`open_requests`, 에이전트의 `project_open`)도 부르는 쪽이 만든다 — 창을 여는 쪽(agent_open.rs)은 setup 에서 붙인다.
/// MCP 세션 등록부(`mcp_sessions`)도 부르는 쪽이 만든다 — `mcp.*` RPC 와 MCP 소켓(setup 에서 연다)이 같은 것을 쓴다.
/// 원격 호스트 관리자(`remote.*` — M7-08)를 함께 돌려준다 — 앱은 라우터 · SSH 백엔드를 붙인다(remote.rs).
#[allow(clippy::too_many_arguments)] // 부르는 쪽이 만들어 다른 자리(창 · 소켓)와 나눠 쓰는 부품들이다.
fn local_host_with(
    paths: AppPaths,
    diagnostics: Arc<Diagnostics>,
    extensions: Arc<Registry>,
    i18n: Arc<I18n>,
    fs: Arc<Fs>,
    askpass: pw_git::write::Askpass,
    open_requests: pw_core::open::OpenRequests,
    mcp_sessions: pw_mcp::sessions::Sessions,
) -> (LocalHost, Option<pw_remote::RemoteHosts>) {
    let (host, _, remote) = local_host_parts(
        paths,
        diagnostics,
        extensions,
        i18n,
        fs,
        askpass,
        open_requests,
        mcp_sessions,
    );
    (host, remote)
}

/// [`local_host_with`] 의 몸통 — 앱의 부품으로 공용 조립(`pw_host::assemble`)을 부른다. DB 세션은 시험([`local_host_and_db`])이
/// 받는다. 앱만의 부품: 비밀은 OS 키체인, `cli.status` 의 「Shell 명령 설치」 대상(shell_command.rs), 설정 창 판정(windows.rs),
/// 뒤에서 도는 일은 Tauri 의 tokio 런타임.
#[allow(clippy::too_many_arguments)] // local_host_with 와 같다.
fn local_host_parts(
    paths: AppPaths,
    diagnostics: Arc<Diagnostics>,
    extensions: Arc<Registry>,
    i18n: Arc<I18n>,
    fs: Arc<Fs>,
    askpass: pw_git::write::Askpass,
    open_requests: pw_core::open::OpenRequests,
    mcp_sessions: pw_mcp::sessions::Sessions,
) -> (
    LocalHost,
    Arc<pw_db::DbSessions>,
    Option<pw_remote::RemoteHosts>,
) {
    let assembled = pw_host::assemble(pw_host::HostParts {
        id: LOCAL_HOST_ID.into(),
        paths,
        diagnostics,
        extensions,
        i18n,
        fs,
        askpass,
        open_requests,
        mcp_sessions,
        // 비밀은 OS 키체인(서비스 `dev.pitwall.db`, M3-02).
        secrets: Arc::new(pw_db::secret::KeyringStore::new()),
        db_drivers: db::drivers(),
        cli: pw_host::CliParts {
            target: shell_command::target_path().map_err(|e| e.to_string()),
            app: std::env::current_exe().map_err(|e| e.to_string()),
            source: shell_command::source_check(),
        },
        settings_window: settings_window(),
        tokio: tauri::async_runtime::handle().inner().clone(),
    });
    (assembled.host, assembled.db_sessions, assembled.remote)
}

/// 창 라벨이 설정 창인가(`pw_host::HostParts::settings_window`) — 도구 설치(`tools.install` 등)는 설정 창 · 그 프로젝트 창만 한다.
fn settings_window() -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    Arc::new(|label: &str| {
        windows::WindowKind::of_label(label) == Some(windows::WindowKind::Settings)
    })
}

/// 내장 확장과 `$DATA/extensions/` 의 사용자 확장. 진단은 로그에 남는다(로깅을 켠 뒤에 부른다).
fn load_extensions(paths: &AppPaths) -> Arc<Registry> {
    let engine = env!("CARGO_PKG_VERSION")
        .parse()
        .expect("Cargo 패키지 버전은 semver 다");
    Arc::new(Registry::load(&LoadOptions::with_data_dir(
        engine,
        &paths.data,
    )))
}

/// `pitwall mcp`(mcp.rs). 앱을 띄우지 않고 단일 인스턴스로 전달하지도 않는다. stdout 은 MCP 의 stdio 가 쓴다.
/// 실행 중인 앱의 소켓은 그 앱과 같은 단일 인스턴스 키로 찾는다 — 다른 워크트리 · 다른 `$DATA` 의 앱에 붙지 않는다.
fn run_mcp() -> ! {
    let args: Vec<_> = std::env::args_os().skip(2).collect();
    let code = mcp::run(&args, |paths| {
        single_instance_key(&context().config().identifier, paths)
    });
    std::process::exit(code);
}

/// `pitwall tool`(tool_cli.rs). `pitwall mcp` 와 같은 길로 세션을 연다 — 같은 단일 인스턴스 키로 앱의 소켓을 찾고, 없으면 이
/// 프로세스 안의 헤드리스 Host 에 붙는다.
fn run_tool() -> ! {
    let args: Vec<_> = std::env::args_os().skip(2).collect();
    let code = tool_cli::run(&args, |paths| {
        single_instance_key(&context().config().identifier, paths)
    });
    std::process::exit(code);
}

/// Tauri 문맥. 앱과 `pitwall mcp` 가 같은 identifier 를 읽도록 한 자리에서 만든다(매크로가 자산을 한 번만 넣는다).
fn context() -> tauri::Context {
    tauri::generate_context!()
}

/// 단일 인스턴스 키(instance.rs): 빌드 종류별 키에 시험 접미사(`PITWALL_TEST_INSTANCE`, debug 만)를 붙인 것.
/// MCP 소켓 이름도 이 키로 정한다(mcp.rs). 앱 · 두 번째 실행 · `pitwall mcp` 가 모두 이것으로 계산한다.
fn single_instance_key(base: &str, paths: &AppPaths) -> String {
    instance::with_test_suffix(
        instance_identifier(base, paths, custom_data(paths).as_deref(), RELEASE),
        std::env::var(instance::TEST_KEY_ENV).ok().as_deref(),
        // `RELEASE` 와 따로 판정한다 — 그 상수가 틀려도 시험 바이너리는 시험 키를 쓴다.
        cfg!(debug_assertions),
    )
}

/// 이 빌드가 release 인가. 단일 인스턴스 키를 고른다([`instance_identifier`]). 시험(debug)이 이 값으로 dev 키를 보는지 지킨다.
const RELEASE: bool = !cfg!(debug_assertions);

/// 이 실행의 기본이 아닌 `$DATA`(instance.rs 의 `custom_data`). 기본 `$DATA` 로 떴으면 `None`.
fn custom_data(paths: &AppPaths) -> Option<std::path::PathBuf> {
    let default = AppPaths::default_data(&pw_core::config::PathEnv::current()).ok();
    instance::custom_data(&paths.data, default.as_deref())
}

/// 단일 인스턴스 키(instance.rs). debug 빌드는 빌드 폴더와 `$DATA` 로, release 빌드는 기본이 아닌 `$DATA`(`custom`)로만
/// 가른다.
fn instance_identifier(
    base: &str,
    paths: &AppPaths,
    custom: Option<&std::path::Path>,
    release: bool,
) -> String {
    if release {
        return instance::release_identifier(base, custom);
    }
    // 아직 없는 `$DATA` 도 나중에 만들어진 뒤와 같은 값이 나오게 레지스트리와 같은 정규화를 쓴다.
    let data =
        pw_core::registry::path::normalize(&paths.data).unwrap_or_else(|_| paths.data.clone());
    instance::identifier(
        base,
        Some((std::path::Path::new(env!("CARGO_MANIFEST_DIR")), &data)),
    )
}

/// git askpass 소켓의 경로: MCP 소켓과 같은 규칙(단일 인스턴스 키 + `$STATE`, `pw_mcp::socket::endpoint`)에 `/askpass` 를 붙인
/// 키로 정한다 — MCP 소켓과 다른 파일이고, 다른 데이터 폴더 · 다른 워크트리의 앱과 섞이지 않는다. 도우미는 이 경로를 git 의 환경
/// 변수로 받으므로 다시 계산하지 않는다.
#[cfg(unix)]
fn askpass_socket(identifier: &str, paths: &AppPaths) -> std::io::Result<std::path::PathBuf> {
    pw_mcp::socket::endpoint(
        &format!("{identifier}/askpass"),
        &paths.state,
        &std::env::temp_dir(),
        pw_mcp::socket::current_uid(),
    )
}

/// askpass 소켓을 연다(앱이 끝날 때까지). 열지 못하면 경고만 — git 은 askpass 없이 돌고 인증이 필요하면 실패 원문을 보인다.
#[cfg(unix)]
fn serve_askpass(askpass: &pw_git::write::Askpass, paths: &AppPaths, identifier: &str) {
    let (socket, exe) = match (askpass_socket(identifier, paths), std::env::current_exe()) {
        (Ok(socket), Ok(exe)) => (socket, exe),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(%e, "git askpass disabled");
            return;
        }
    };
    tauri::async_runtime::spawn(askpass.clone().serve(socket, exe));
}

/// 앱을 떼어 띄우고 이 프로세스는 끝낸다(detach.rs). 단일 인스턴스 소켓을 먼저 내려놓는다 — 자식이 아직 살아 있는 이
/// 프로세스를 「기존 앱」으로 보고 인자를 넘긴 뒤 끝나 버리지 않게. 띄우지 못하면 오류를 내고 0 이 아닌 코드로 끝난다
/// (소켓을 이미 내려놓았으므로 이 프로세스가 앱이 되면 안 된다).
fn detach_and_exit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> ! {
    tauri_plugin_single_instance::destroy(app);
    let result = detach::respawn();
    match &result {
        Ok(child) => tracing::info!(child, "detached the app from the launching process"),
        Err(e) => tracing::error!(%e, "cannot detach the app from the launching process"),
    }
    let (code, message) = detach::exit_after_respawn(&result);
    if let Some(message) = message {
        eprintln!("{message}");
    }
    std::process::exit(code);
}

/// 주 인스턴스가 된 앱의 시작(setup 본체). 첫 실행의 인자도 두 번째 실행과 같은 [`launch::dispatch`] 로 연다.
fn start<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    host: &LocalHost,
    config_events: pw_core::Subscription,
    (argv, cwd): (Vec<String>, String),
) {
    tauri::async_runtime::spawn(config_log::run(config_events));
    // 프로젝트 창이 연결해도 되는 체크아웃(그 창의 묶음 멤버)을 이 레지스트리로 가른다(ipc.rs `window_admits`, 18-m7-plan M7-11).
    // 시험의 mock 앱에는 transport 가 없다.
    if let (Some(registry), Some(ipc)) = (host.registry(), app.try_state::<ipc::IpcState>()) {
        ipc.use_registry(registry.clone());
    }
    if let Some(i18n) = app.try_state::<Arc<I18n>>() {
        i18n::follow_in_native_ui(app, Arc::clone(&i18n), i18n::subscribe(host.bus()));
    }
    // 레지스트리를 미리 열어 상태를 로그에 남긴다(정상이 아니면 경고 — 손상 · 잠김 · 읽기 전용). 열기는 잠긴 파일을 최대 2초 기다리고
    // 무결성 검사를 하므로 창을 띄우는 스레드가 아니라 블로킹 스레드에서 한다.
    if let Some(registry) = host.registry().cloned() {
        tauri::async_runtime::spawn_blocking(move || {
            let status = registry.status();
            if matches!(status, pw_core::registry::StorageStatus::Ok { .. }) {
                tracing::info!(?status, "project registry opened");
            } else {
                tracing::warn!(?status, "project registry is not using its file normally");
            }
        });
    }
    // 감시를 못 켜도 앱은 뜬다 — 설정은 시작할 때 읽은 값으로 남는다.
    if let Err(e) = host.watch_config(tauri::async_runtime::handle().inner()) {
        tracing::warn!(%e, "config watch disabled");
    }
    launch::dispatch(app, argv, cwd);
}

fn main() {
    // 서브커맨드는 무엇보다 먼저 가른다 — 로그 · 확장 · Host 를 만들지 않고, 단일 인스턴스 플러그인에도 닿지 않는다.
    if std::env::args_os().nth(1).is_some_and(|a| a == "mcp") {
        run_mcp();
    }
    // `pitwall tool …`: MCP 툴을 명령줄에서 부른다(tool_cli.rs). `mcp` 와 같다 — 앱을 띄우지 않고 단일 인스턴스로 전달하지 않는다.
    if std::env::args_os().nth(1).is_some_and(|a| a == "tool") {
        run_tool();
    }
    // `pitwall askpass <질문>`: 앱이 띄운 git 이 부르는 도우미(pw_git::write::askpass). 로그 · 경로 · 앱 무엇도 만들지 않는다.
    if std::env::args_os().nth(1).is_some_and(|a| a == "askpass") {
        let args: Vec<_> = std::env::args_os().skip(2).collect();
        std::process::exit(pw_git::write::askpass::client_main(&args));
    }
    let perf = perf::PerfMarks::from_env();
    perf.record("main", None, None);
    // 떼어 띄운 자식이면 표식을 지운다 — 앱이 띄우는 프로세스(터미널 등)가 물려받지 않게(detach.rs 머리 주석).
    let detached = std::env::var_os(detach::MARKER_ENV).is_some();
    // SAFETY: 아직 스레드를 띄우기 전이다(로그 · 런타임 · Tauri 모두 이 아래에서 시작한다).
    unsafe { std::env::remove_var(detach::MARKER_ENV) };
    // 경로 해석이 로깅보다 먼저다(로그 폴더가 `$STATE/logs`). 그래서 이 실패는 stderr 에만 남는다.
    let paths = match AppPaths::from_env() {
        Ok(paths) => paths,
        Err(e) => {
            // 패닉(Debug 형식, exit 101) 대신 변수 이름이 든 문장으로 끝낸다. 창을 띄우기 전이라 다이얼로그는 없다.
            eprintln!("pitwall: {e}");
            std::process::exit(2);
        }
    };
    let diagnostics = diag::init(&paths);
    let extensions = load_extensions(&paths);
    let i18n = i18n::init(&extensions);
    let fs = Arc::new(Fs::new());
    let askpass = pw_git::write::Askpass::default();
    let open_requests = pw_core::open::OpenRequests::default();
    let mcp_sessions = pw_mcp::sessions::Sessions::new();
    let (host, remote) = local_host_with(
        paths.clone(),
        Arc::clone(&diagnostics),
        extensions,
        Arc::clone(&i18n),
        Arc::clone(&fs),
        askpass.clone(),
        open_requests.clone(),
        mcp_sessions.clone(),
    );
    let host = Arc::new(host);
    // 「활성 기능」은 지금 Host 가 처리하는 메서드의 모듈 이름이다. 확장 · 설정 기능이 들어오면 그것으로 바꾼다.
    diagnostics.set_features(
        host.router()
            .methods()
            .iter()
            .filter_map(|m| m.name.split_once('.').map(|(module, _)| module.to_owned())),
    );
    if let Some(config) = host.config() {
        let global = config.global.clone();
        diagnostics.set_crash_reports(move || diag::crash_reports_enabled(&global));
        // 처음 읽기의 실패는 Host 를 만드는 중에 이벤트로 나가 아래 구독이 받지 못한다. 여기서 따로 남긴다.
        if let Some(fault) = config.global.fault() {
            config_log::log_fault(&Scope::Host, &fault);
        }
    }
    let update_settings = update::Settings(host.config().map(|c| c.global.clone()));
    let auto_update = update::auto_check(RELEASE, custom_data(&paths).is_some());
    let config_events = config_log::subscribe(host.bus());
    let router = Arc::new(HostRouter::new());
    router
        .add_host(host.clone())
        .expect("빈 라우터에 Host 를 처음 등록한다");
    // 붙은 원격 Host 는 그 머신 id 로 이 라우터에 더한다(M7-08).
    if let Some(remote) = &remote {
        remote.attach_router(Arc::clone(&router));
    }
    // 원격 셸의 `pitwall <파일>` 이 고를 「마지막 사람 입력」(remote_open.rs — M7-15).
    let last_input = Arc::new(remote_open::LastInput::default());
    let follow_router = Arc::clone(&router);
    // MCP 세션이 다른 머신을 겨누는 부품: 묶음 · 별명은 레지스트리, 링크는 원격 호스트 관리자, 연결은 이 라우터(M7-18).
    #[cfg(unix)]
    let mcp_machines: Option<Arc<dyn pw_mcp::checkout::Machines>> =
        host.registry().cloned().map(|registry| {
            Arc::new(mcp::machines::DesktopMachines::new(
                registry,
                remote.clone(),
                Arc::clone(&router),
            )) as Arc<dyn pw_mcp::checkout::Machines>
        });

    let mut context = context();
    let identifier = single_instance_key(&context.config().identifier, &paths);
    tracing::info!(%identifier, "single-instance key");
    context.config_mut().identifier = identifier.clone();
    let webview_store = custom_data(&paths).map(|data| instance::webview_store_id(&data));
    if let Some(id) = webview_store {
        tracing::info!(store = %instance::uuid_string(id), "webview data store");
    }

    // 단일 인스턴스 플러그인은 가장 먼저 등록한다 — 두 번째 실행은 이 플러그인의 setup 에서 인자와 cwd 를 기존 앱에 넘기고
    // 종료하므로 그 뒤의 플러그인 · setup 이 돌지 않는다.
    ipc::install(tauri::Builder::default(), router)
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            launch::dispatch(app, argv, cwd)
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // MCP 승인 요청의 OS 알림(approval_notice.rs, 16-m5-plan M5-14).
        .plugin(tauri_plugin_notification::init())
        .manage(approval_notice::ApprovalNotices::default())
        .manage(launch::Launch::new(host.registry().cloned()))
        .manage(windows::PendingOpens::default())
        .manage(windows::FileWindows::default())
        .manage(windows::WebviewStore(webview_store))
        .manage(drop_import::DropImport::new(fs, host.tasks().clone()))
        .manage(diagnostics)
        .manage(perf)
        .manage(update_settings)
        .manage(mcp_sessions.clone())
        .manage(Arc::clone(&last_input))
        // 메뉴(`menu::build`)가 지금 언어로 글자를 만든다 — 메뉴보다 먼저 등록한다.
        .manage(i18n)
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .setup(move |app| {
            // 여기까지 왔으면 기존 앱이 없다(있으면 단일 인스턴스 플러그인이 인자를 넘기고 끝냈다).
            if detach::should_detach(
                detach::parent_pid(),
                detached,
                detach::under_dev_server(
                    RELEASE,
                    std::env::var_os(detach::DEV_SERVER_ENV).is_some(),
                ),
            ) {
                detach_and_exit(app.handle());
            }
            start(
                app.handle(),
                &host,
                config_events,
                launch::invocation_from_env(),
            );
            perf::watch_parent(app.handle());
            // 에이전트의 `project_open` 은 이제부터 창을 연다(앞으로 가져오지 않는다 — agent_open.rs).
            open_requests.attach(Arc::new(agent_open::WindowOpener::new(
                app.handle().clone(),
            )));
            update::on_startup(app.handle(), auto_update);
            approval_notice::install(app.handle());
            // 주 인스턴스만 MCP 소켓을 연다(떼어 띄우기 전의 프로세스 · 두 번째 실행은 여기 오지 않는다).
            #[cfg(unix)]
            mcp::serve_socket(
                host.clone(),
                paths.clone(),
                identifier.clone(),
                mcp_sessions.clone(),
                mcp_machines.clone(),
            );
            // askpass 소켓도 주 인스턴스만 연다.
            #[cfg(unix)]
            serve_askpass(&askpass, &paths, &identifier);
            // 원격 호스트의 SSH 백엔드: 기기 키는 이 인스턴스 키의 키체인 항목, ssh 의 질문은 같은 askpass 로 창에(M7-08).
            if let Some(remote) = &remote {
                remote::attach(remote, &askpass, &paths, &identifier);
                // 붙은 원격 머신의 열기 요청을 묶음 창으로(remote_open.rs — M7-15 ②).
                if let Some(registry) = host.registry().cloned() {
                    tauri::async_runtime::spawn(remote_open::follow(
                        remote.clone(),
                        Arc::clone(&follow_router),
                        registry,
                        Arc::clone(&last_input),
                        Arc::new(remote_open::AppWindows(app.handle().clone())),
                    ));
                }
            }
            Ok(())
        })
        .build(context)
        .expect("Tauri 앱을 시작하지 못했다")
        .run(|app, event| {
            // Tauri 는 이 뒤에 `std::process::exit` 로 끝나 Drop 이 돌지 않는다. 터미널의 셸 · 그 자식(SIGHUP 을 무시하는 것 포함)을
            // 여기서 기다려 끝낸다(M1-08). 업데이트를 설치하고 창을 모두 닫은 것이면 그 뒤 새 버전으로 다시 시작한다.
            if let tauri::RunEvent::Exit = event {
                // MCP: 대기 중인 승인을 거두고 세션들의 감사 큐를 비운다 — 최대 2초 종료가 늦어질 수 있다(16-m5-plan M5-12).
                app.state::<pw_mcp::sessions::Sessions>()
                    .shutdown(std::time::Duration::from_secs(2));
                end_processes_on_exit();
                update::restart_if_installed(app);
            }
        });
}

/// 앱이 끝날 때(`RunEvent::Exit` — 뒤에 `std::process::exit` 가 와 Drop 이 돌지 않는다) 남은 자식 프로세스를 끝낸다: 터미널의 셸과
/// 실행(`run.*` — 중지 중인 것 포함, 17-m6-plan M6-02 · M6-06)은 `pw_run::shutdown`, DB 의 SSH 터널(M3-11)은 남은 ssh 프로세스 그룹.
/// 프로젝트 창을 닫을 때의 확인(M6-06 ⑥)은 창이 하고, 여기는 묻지 않는다 — 이미 닫기로 한 뒤다.
fn end_processes_on_exit() {
    pw_run::shutdown();
    pw_db_driver::tunnel::shutdown();
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use super::*;

    fn no_languages() -> Arc<I18n> {
        Arc::new(I18n::new(pw_i18n::Catalog::default(), Vec::new()))
    }

    fn host_under(dir: &std::path::Path) -> LocalHost {
        local_host(
            AppPaths::under(dir),
            Arc::new(Diagnostics::new(dir.join("logs"), "0.0.0")),
            Arc::new(Registry::default()),
            no_languages(),
            Arc::new(Fs::new()),
            pw_git::write::Askpass::default(),
        )
    }

    #[test]
    fn release_builds_with_the_default_data_share_one_single_instance_key() {
        let paths = AppPaths::under(std::path::Path::new("/somewhere"));
        assert_eq!(
            instance_identifier("dev.pitwall.desktop", &paths, None, true),
            "dev.pitwall.desktop"
        );
        let custom = instance_identifier(
            "dev.pitwall.desktop",
            &paths,
            Some(std::path::Path::new("/tmp/perf/data")),
            true,
        );
        assert!(custom.starts_with("dev.pitwall.desktop.data-"), "{custom}");
    }

    /// 이 빌드(시험은 debug)가 실제로 고르는 키. [`RELEASE`] 가 뒤집히면 워크트리끼리 다시 섞인다.
    #[test]
    fn this_debug_build_keys_by_build_and_data() {
        let a = AppPaths::under(std::path::Path::new("/data-a"));
        let b = AppPaths::under(std::path::Path::new("/data-b"));
        let key = instance_identifier("dev.pitwall.desktop", &a, None, RELEASE);
        assert!(key.starts_with("dev.pitwall.desktop.dev-"), "{key}");
        assert_ne!(
            key,
            instance_identifier("dev.pitwall.desktop", &b, None, RELEASE)
        );
    }

    /// askpass 소켓은 MCP 소켓과 다른 파일이고, 단일 인스턴스 키(다른 데이터 폴더 · 다른 빌드)나 `$STATE` 가 다르면 다른 경로다.
    #[cfg(unix)]
    #[test]
    fn askpass_sockets_are_per_instance_and_apart_from_the_mcp_socket() {
        let a = AppPaths::under(std::path::Path::new("/data-a"));
        let b = AppPaths::under(std::path::Path::new("/data-b"));
        let key_a = instance_identifier("dev.pitwall.desktop", &a, None, RELEASE);
        let key_b = instance_identifier("dev.pitwall.desktop", &b, None, RELEASE);
        let socket = askpass_socket(&key_a, &a).unwrap();
        assert_eq!(
            socket,
            askpass_socket(&key_a, &a).unwrap(),
            "같은 인스턴스는 같은 경로"
        );
        assert_ne!(
            socket,
            askpass_socket(&key_b, &b).unwrap(),
            "다른 데이터 폴더의 앱"
        );
        assert_ne!(
            socket,
            askpass_socket(&key_b, &a).unwrap(),
            "같은 `$STATE` 의 다른 키"
        );
        let mcp = pw_mcp::socket::endpoint(
            &key_a,
            &a.state,
            &std::env::temp_dir(),
            pw_mcp::socket::current_uid(),
        )
        .unwrap();
        assert_ne!(socket, mcp);
        assert_eq!(
            socket.parent(),
            mcp.parent(),
            "같은 사용자 전용 폴더(`$STATE/run`)"
        );
    }

    /// 인자를 준 첫 실행(setup 본체)이 두 번째 실행과 같은 열기 경로로 프로젝트 창을 연다.
    #[test]
    fn a_cold_start_with_a_path_opens_its_project_window() {
        use tauri::Manager;

        let tmp = tempfile::tempdir().unwrap();
        // Windows 의 canonicalize 는 verbatim 접두를 붙인다 — 레지스트리가 저장하는 꼴로 맞춘다.
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let host = host_under(&base.join("app"));
        let registry = host.registry().cloned().unwrap();
        let id = registry
            .create(pw_core::registry::NewProject {
                root: root.clone(),
                ..Default::default()
            })
            .unwrap()
            .id;
        let app = tauri::test::mock_app();
        app.manage(launch::Launch::new(Some(registry)));
        app.manage(windows::PendingOpens::default());
        app.manage(windows::FileWindows::default());
        let events = config_log::subscribe(host.bus());
        start(
            app.handle(),
            &host,
            events,
            (
                vec!["/usr/local/bin/pitwall".into(), "repo".into()],
                base.display().to_string(),
            ),
        );
        let label = windows::project_label(&id);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.get_webview_window(&label).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "프로젝트 창이 열리지 않았다"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.get_webview_window(windows::WELCOME_LABEL).is_none());
    }

    /// 생성물(`packages/rpc-client`)의 메서드 목록 == 앱의 Host 가 실제로 등록한 메서드 목록.
    ///
    /// 메서드를 선언한 크레이트를 pw-rpc-gen 에 링크하지 않으면 그 메서드는 생성물에서 오류 없이 빠지고 생성물 최신 검사도
    /// 통과한다(crates/pw-rpc-gen/src/main.rs 머리 주석). 그 빠뜨림을 잡는 곳이 여기다. 핸들러만 있고 생성물이 낡은 경우,
    /// 선언만 하고 핸들러를 안 붙인 경우도 함께 잡는다. 앱과 같은 크레이트 집합을 링크하므로 CI 의 세 OS 에서
    /// 「다른 크레이트의 선언 수집」도 이 테스트로 실증된다.
    /// 보지 않는 것: 어느 Host 에도 등록하지 않을 선언. 그런 선언을 허용하게 되면 비교를 「등록 ⊆ 생성」으로 낮춘다.
    #[test]
    fn generated_rpc_covers_the_router() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../packages/rpc-client/src/generated/schema.json"
        ))
        .unwrap();
        let generated: BTreeSet<String> = schema["methods"]
            .as_object()
            .expect("schema.json 의 methods 객체")
            .keys()
            .cloned()
            .collect();
        // 없는 폴더를 가리켜 실제 `$CONFIG` 를 읽지 않는다. 파일이 없으면 아무것도 쓰지 않는다.
        let host = local_host(
            AppPaths::under(&std::env::temp_dir().join("pitwall-generated-rpc-test-absent")),
            Arc::new(Diagnostics::new("unused-log-dir", "0.0.0")),
            Arc::new(Registry::default()),
            no_languages(),
            Arc::new(Fs::new()),
            pw_git::write::Askpass::default(),
        );
        let routed: BTreeSet<String> = host
            .router()
            .methods()
            .into_iter()
            .map(|m| m.name.to_owned())
            .collect();
        assert!(!routed.is_empty());
        // Relay 의 Host 에만 있는 메서드(「Desktop 에 묻는」 비밀 저장소의 역요청 — 18-m7-plan M7-12 — 와 기기 관리 `relay.*` —
        // M7-23). Desktop 의 Host 는 내지 않는다.
        let relay_only: BTreeSet<String> = pw_db::desktop_secret::METHODS
            .iter()
            .map(|m| (*m).to_owned())
            .chain(
                generated
                    .iter()
                    .filter(|m| m.starts_with("relay."))
                    .cloned(),
            )
            .collect();
        assert!(
            routed.is_disjoint(&relay_only),
            "the desktop host must not take secret requests"
        );
        let routed: BTreeSet<String> = routed.union(&relay_only).cloned().collect();
        assert_eq!(
            generated, routed,
            "`mise run gen` 을 돌리고, 메서드를 선언한 크레이트를 pw-rpc-gen 에 링크했는지 본다"
        );
    }

    /// 앱이 Host 에 넣는 설정 창 판정: 설정 창만 「설정 창」 이다 — 다른 창(웰컴 · 프로젝트 · 단일 파일 · 모르는 라벨)이 설정 창으로
    /// 읽히면 그 창에서 도구 설치가 열린다.
    #[test]
    fn only_the_settings_window_counts_as_the_settings_window() {
        let is_settings = settings_window();
        assert!(is_settings(windows::SETTINGS_LABEL));
        for label in [
            windows::WELCOME_LABEL.to_owned(),
            windows::project_label("abc"),
            "file-1".to_owned(),
            "settings-2".to_owned(),
            String::new(),
        ] {
            assert!(!is_settings(&label), "{label}");
        }
    }

    /// 생성물(`packages/rpc-client`)의 토픽 목록(`TOPICS`) ⊆ 앱이 링크한 토픽 선언(`pw_rpc::events()`).
    ///
    /// 토픽 선언(`rpc_event!`)도 메서드처럼 링크된 크레이트의 것만 보인다. 생성기는 선언한 크레이트를 링크하지만 앱이 그 크레이트를
    /// 링크하지 않으면 그 토픽은 앱에서 선언이 없는 것이 되어 `events.subscribe` 가 오류 없이 구독 전체를 거절한다(18-m7-plan
    /// M7-02). [`generated_rpc_covers_the_router`] 와 같은 까닭으로 앱 자리에서 본다.
    #[test]
    fn generated_topics_are_declared_in_the_app() {
        let generated = include_str!("../../../../packages/rpc-client/src/generated/rpc.ts");
        let block = generated
            .split_once("export const TOPICS")
            .expect("rpc.ts 의 TOPICS")
            .1
            .split_once("\n};")
            .expect("TOPICS 의 끝")
            .0;
        let topics: BTreeSet<&str> = block
            .lines()
            .filter_map(|line| line.trim().strip_prefix('"')?.split_once('"'))
            .map(|(name, _)| name)
            .collect();
        assert!(topics.len() > 10, "{topics:?}");
        let declared: BTreeSet<&str> = pw_rpc::events().into_iter().map(|e| e.name).collect();
        let missing: Vec<_> = topics.difference(&declared).collect();
        assert!(
            missing.is_empty(),
            "앱에 선언이 링크되지 않은 토픽 {missing:?} — 선언한 크레이트를 앱(pw-host)이 링크하는지 본다"
        );
    }

    /// JSON-RPC 연결 하나. 응답만 받고 그 사이의 알림(스트림 청크 · 이벤트)은 건너뛴다.
    struct Client(pw_core::Connection, i64);

    impl Client {
        async fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
            self.1 += 1;
            let id = self.1;
            self.0.sink.send(serde_json::json!(
                {"jsonrpc": "2.0", "id": id, "method": method, "params": params}
            ));
            loop {
                let frame = tokio::time::timeout(Duration::from_secs(10), self.0.source.recv())
                    .await
                    .unwrap_or_else(|_| panic!("{method}: no reply"))
                    .expect("connection closed");
                if frame["id"] == serde_json::json!(id) {
                    return frame;
                }
            }
        }
    }

    /// 응답의 결과: 성공이면 `None`, 오류면 그 코드.
    fn outcome(frame: &serde_json::Value) -> Option<i64> {
        frame
            .get("error")
            .map(|e| e["code"].as_i64().expect("error code"))
    }

    /// 프로젝트 창의 연결(프로젝트로 고른 연결 — 범위 `Project`)은 Host 로 고른 연결이 닿는 것에 모두 닿는다(M1-25). 앱의 Host
    /// 그대로(모든 메서드 크레이트) 두 범위로 연결해
    /// - 모든 메서드에 어느 params 타입으로도 읽히지 않는 params(`[[[]]]` — 구조체는 배열을 칸 순서로 읽으므로 첫 칸에서 실패한다.
    ///   객체 · 배열이 아닌 params 는 관문 앞에서 invalid request 다)를 보낸다: 디스패처의 관문(role · 범위)을 지나면 params 를 읽다가
    ///   invalid params 로 끝나므로, 두 범위의 답이 모두 invalid params 면 관문이 범위로 막는 메서드가 없다. 핸들러는 돌지 않는다
    ///   (셸 · 외부 도구 · 휴지통 같은 부작용 없음).
    /// - 핸들러까지 가는 대표 호출(fs · search · terminal · project · config · task · openWith)이 두 범위에서 같은 결과다. 셸을 띄우는
    ///   `terminal.create` · 로그인 셸을 읽는 `runtime.*` 는 위의 관문 확인으로 갈음한다 — 핸들러에서 범위를 보는 것은
    ///   `project.state.set` 뿐이다(`Ctx::scope` 를 읽는 곳).
    #[test]
    fn a_project_scoped_connection_reaches_everything_a_host_connection_does() {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
        let host = host_under(&base.join("app"));
        let methods: Vec<String> = host
            .router()
            .methods()
            .into_iter()
            .map(|m| m.name.to_owned())
            .collect();
        let id = host
            .registry()
            .unwrap()
            .create(pw_core::registry::NewProject {
                root: root.clone(),
                ..Default::default()
            })
            .unwrap()
            .id;
        let router = HostRouter::new();
        router.add_host(Arc::new(host)).unwrap();
        router.bind_project(id.clone(), LOCAL_HOST_ID).unwrap();
        let owner = || pw_rpc::Caller::new(pw_rpc::Role::Owner);
        tauri::async_runtime::block_on(async {
            let by_host = pw_core::Target::HostId(LOCAL_HOST_ID.into());
            let by_project = pw_core::Target::ProjectId(id.clone());
            let mut by_host = Client(router.connect(&by_host, owner()).await.unwrap(), 0);
            let mut by_project = Client(router.connect(&by_project, owner()).await.unwrap(), 0);
            let invalid = Some(i64::from(pw_rpc::code::INVALID_PARAMS));
            assert!(methods.len() > 50, "{methods:?}");
            for method in &methods {
                let params = serde_json::json!([[[]]]);
                assert_eq!(
                    outcome(&by_host.call(method, params.clone()).await),
                    invalid,
                    "host {method}"
                );
                assert_eq!(
                    outcome(&by_project.call(method, params).await),
                    invalid,
                    "project {method}"
                );
            }

            let file = root.join("a.txt").display().to_string();
            let dir = root.display().to_string();
            let calls = [
                (
                    "fs.list",
                    serde_json::json!({"projectId": id, "path": dir}),
                    true,
                ),
                (
                    "fs.stat",
                    serde_json::json!({"projectId": id, "path": file}),
                    true,
                ),
                (
                    "fs.read",
                    serde_json::json!({"projectId": id, "path": file}),
                    true,
                ),
                (
                    "fs.readBytes",
                    serde_json::json!({"projectId": id, "path": file, "offset": 0, "length": 4}),
                    true,
                ),
                (
                    "search.text",
                    serde_json::json!({"projectId": id, "paths": [dir], "query": "alpha"}),
                    true,
                ),
                (
                    "search.files",
                    serde_json::json!({"projectId": id, "query": "a"}),
                    false,
                ),
                (
                    "terminal.input",
                    serde_json::json!({"terminalId": 4_000_000_000_u32, "data": "x"}),
                    false,
                ),
                (
                    "terminal.resize",
                    serde_json::json!({"terminalId": 4_000_000_000_u32, "cols": 80, "rows": 24}),
                    false,
                ),
                (
                    "terminal.close",
                    serde_json::json!({"terminalId": 4_000_000_000_u32}),
                    false,
                ),
                ("project.get", serde_json::json!({"id": id}), true),
                ("project.info", serde_json::json!({"projectId": id}), true),
                ("project.trust", serde_json::json!({"id": id}), true),
                ("project.list", serde_json::Value::Null, true),
                (
                    "project.state.get",
                    serde_json::json!({"projectId": id}),
                    true,
                ),
                ("config.get", serde_json::json!({"projectId": id}), false),
                ("task.list", serde_json::json!({"projectId": id}), true),
                ("openWith.list", serde_json::json!({"projectId": id}), true),
            ];
            let denied = Some(i64::from(pw_rpc::code::PERMISSION_DENIED));
            for (method, params, ok) in calls {
                let host = by_host.call(method, params.clone()).await;
                let project = by_project.call(method, params).await;
                assert_eq!(outcome(&project), outcome(&host), "{method}: {project}");
                assert_ne!(outcome(&project), denied, "{method}: {project}");
                if ok {
                    assert_eq!(outcome(&project), None, "{method}: {project}");
                }
            }
            // 범위를 보는 단 하나: 상태 쓰기는 프로젝트로 연 연결만.
            let set = serde_json::json!({"projectId": id, "state": {"x": 1}});
            assert_eq!(
                outcome(&by_project.call("project.state.set", set.clone()).await),
                None
            );
            assert_eq!(
                outcome(&by_host.call("project.state.set", set).await),
                denied
            );
        });
    }

    /// 앱 종료 흉내(시험 [`exiting_the_app_ends_runs`] 가 이 시험 바이너리를 자식으로 띄울 때만 돈다): 실행 하나(SIGHUP · SIGTERM 을
    /// 무시하는 손자를 둔다)를 띄워 중지를 걸고 pid 를 찍은 뒤 [`end_processes_on_exit`] 를 부르고 Tauri 처럼 `std::process::exit` 로
    /// 끝난다.
    #[cfg(unix)]
    #[test]
    fn app_exit_probe() {
        use pw_run::pty::Ending;
        use pw_run::runs::{
            LaunchedBy, NewRun, ProcessLauncher, ProcessSpec, PtyLauncher, Relaunch, Runs,
        };

        if std::env::var("PW_APP_EXIT_PROBE").is_err() {
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let pids = runtime.block_on(async {
            let runs = Runs::new(Arc::new(pw_runtime::Runtime::new(
                pw_runtime::LoginShell::new(None, Duration::from_secs(1)),
            )));
            let spec = ProcessSpec {
                argv: [
                    "/bin/sh",
                    "-c",
                    "(trap '' HUP TERM; exec sleep 100) & echo BG=$!; echo DONE; wait",
                ]
                .map(str::to_owned)
                .to_vec(),
                cwd: std::fs::canonicalize(std::env::temp_dir()).unwrap(),
                env: vec![("PATH".into(), "/usr/bin:/bin".into())],
                cols: 80,
                rows: 24,
                ending: Ending::Terminate {
                    grace: Duration::from_secs(5),
                },
            };
            let info = runs
                .start_kind(
                    NewRun {
                        project_id: "p".into(),
                        config: "probe".into(),
                        name: "probe".into(),
                        launched_by: LaunchedBy::Window { window: None },
                        worktree: None,
                        cwd: Some(spec.cwd.clone()),
                        profile: None,
                        relaunch: Relaunch::None,
                        size: (80, 24),
                        restart_of: None,
                    },
                    |output| PtyLauncher.launch(&spec, output),
                )
                .unwrap();
            let run = runs.find("p", &info.run_id).unwrap();
            let text = loop {
                let text = run
                    .with_log(|log| String::from_utf8_lossy(&log.read(0, 4096).bytes).into_owned());
                if text.contains("DONE") {
                    break text;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            runs.stop("p", &info.run_id).unwrap();
            let background: u32 = text
                .split("BG=")
                .nth(1)
                .unwrap()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap();
            [background, info.pid.unwrap()]
        });
        println!("PIDS={},{}", pids[0], pids[1]);
        end_processes_on_exit();
        std::process::exit(0);
    }

    /// 17-m6-plan M6-06 완료 기준(Rust): 앱 종료 길([`end_processes_on_exit`] — `RunEvent::Exit` 가 부른다)이 실행을 끝낸다 — 중지
    /// 중이던 실행과 SIGHUP · SIGTERM 을 무시하는 손자까지 1초 안에 사라진다.
    #[cfg(unix)]
    #[test]
    fn exiting_the_app_ends_runs() {
        fn alive(pid: u32) -> bool {
            // SAFETY: 신호 0 은 존재만 본다.
            unsafe { libc::kill(libc::pid_t::try_from(pid).unwrap(), 0) == 0 }
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["tests::app_exit_probe", "--exact", "--nocapture"])
            .env("PW_APP_EXIT_PROBE", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let exited = std::time::Instant::now();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let pids: Vec<u32> = stdout
            .lines()
            .find_map(|l| l.strip_prefix("PIDS="))
            .unwrap_or_else(|| {
                panic!(
                    "no pids in {stdout:?} {:?}",
                    String::from_utf8_lossy(&output.stderr)
                )
            })
            .split(',')
            .map(|p| p.parse().unwrap())
            .collect();
        let left = loop {
            let left: Vec<u32> = pids.iter().copied().filter(|&p| alive(p)).collect();
            if left.is_empty() || exited.elapsed() > Duration::from_secs(1) {
                break left;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        for &pid in &left {
            // SAFETY: 이 시험이 띄운 프로세스다.
            unsafe { libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL) };
        }
        assert!(left.is_empty(), "{left:?} of {pids:?} outlived the app");
    }
}
