// 릴리스 빌드에서 Windows 가 콘솔 창을 함께 띄우지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_core::{HostRouter, LocalHost, Scope};
use pw_diag::Diagnostics;
use pw_ext::{LoadOptions, Registry};
use pw_fs::Fs;
use pw_i18n::I18n;
use pw_open_with::{HostParts, OpenWith};
use pw_runtime::{LoginShell, Runtime, RuntimeHost};
use tauri::Manager as _;

mod cli;
mod clipboard;
mod config_log;
mod detach;
mod diag;
mod i18n;
mod instance;
mod ipc;
mod launch;
mod mcp;
mod menu;
mod perf;
mod shell_command;
mod trust;
mod update;
mod windows;

/// 데스크톱 프로세스 안의 Host 의 id. 프론트엔드가 프로젝트 없이 연결할 때 이 id 로 고른다(src/App.tsx 의 `LOCAL_HOST`).
const LOCAL_HOST_ID: &str = "local";

/// 앱이 쓰는 LocalHost. 생성물과 등록된 메서드를 맞춰 보는 테스트도 이것을 쓴다.
/// UI 언어(`i18n`)는 Host 를 만든 뒤 그 전역 설정을 붙인다.
fn local_host(
    paths: AppPaths,
    diagnostics: Arc<Diagnostics>,
    extensions: Arc<Registry>,
    i18n: Arc<I18n>,
) -> LocalHost {
    let methods = Arc::clone(&i18n);
    // 앱이 띄우는 프로세스의 환경. 로그인 셸은 처음 쓸 때 돈다 — 시작을 늦추지 않는다.
    // 미신뢰 프로젝트에서는 프로젝트 코드를 돌리는 제공자를 고르지 않는다(M1-05). 레지스트리는 Host 를 만든 뒤 붙인다.
    let trust = trust::TrustGate::default();
    let runtime = Arc::new(Runtime::new(LoginShell::for_this_user()).with_trust(trust.check()));
    let runtime_methods = Arc::clone(&runtime);
    let lang = Arc::new(pw_lang::methods::Lang::new(&extensions));
    let lang_methods = Arc::clone(&lang);
    let open_with = Arc::new(OpenWith::new(Arc::clone(&extensions), Arc::clone(&runtime)));
    let open_with_methods = Arc::clone(&open_with);
    let fs = Arc::new(Fs::new());
    let fs_methods = Arc::clone(&fs);
    let fs_watch = pw_fs::watch::Watch::new(Arc::clone(&fs));
    let fs_watch_methods = fs_watch.clone();
    // 「Orca 에서 Pitwall 열기」 안내의 검사(`cli.status`): 「Shell 명령 설치」 의 대상과 이 바이너리.
    let cli_check = Arc::new(pw_open_with::cli::CliCheck::new(
        Arc::clone(&runtime),
        shell_command::target_path().map_err(|e| e.to_string()),
        std::env::current_exe().map_err(|e| e.to_string()),
        shell_command::source_check(),
    ));
    // 내장 터미널(M1-08). 창이 닫히면 그 연결의 터미널 스트림이 버려져 셸의 프로세스 트리가 끝난다.
    let terminals = Arc::new(pw_run::Terminals::new(Arc::clone(&runtime)));
    let terminal_methods = Arc::clone(&terminals);
    let host = LocalHost::builder(LOCAL_HOST_ID)
        .config(paths)
        .methods(move |router| pw_diag::methods::register(router, diagnostics))
        .methods(move |router| pw_ext::methods::register(router, extensions))
        .methods(move |router| pw_i18n::methods::register(router, methods))
        .methods(move |router| pw_lang::methods::register(router, lang_methods))
        .methods(move |router| pw_open_with::methods::register(router, open_with_methods))
        .methods(move |router| pw_runtime::methods::register(router, runtime_methods))
        .methods(move |router| pw_fs::methods::register(router, fs_methods))
        .methods(move |router| pw_fs::watch::register(router, fs_watch_methods))
        .methods(move |router| pw_open_with::cli::register(router, cli_check))
        .methods(move |router| pw_run::methods::register(router, terminal_methods))
        .build();
    // 파일 감시(`fs.watch`)는 바뀐 경로를 버스로 낸다.
    fs_watch.attach(host.bus().clone());
    if let Some(config) = host.config() {
        i18n.attach_config(config.global.clone());
        // 외부 도구 실행(⌥F1 · ⌘⌥E)이 전역 설정 · 프로젝트 레지스트리를 쓴다.
        if let Some(projects) = host.registry() {
            // 파일 RPC(`fs.*`)는 요청의 프로젝트 루트를 레지스트리에서 찾는다.
            fs.attach(projects.clone());
            // 터미널은 전역 설정(`[terminal] shell`)과 레지스트리(프로젝트 · cwd)를 쓴다.
            terminals.attach(pw_run::HostParts {
                global: config.global.clone(),
                projects: projects.clone(),
            });
            open_with.attach(HostParts {
                paths: config.paths.clone(),
                global: config.global.clone(),
                projects: projects.clone(),
            });
            runtime.attach(RuntimeHost {
                projects: projects.clone(),
                bus: host.bus().clone(),
            });
            // 언어 판별(`lang.detect`)이 설정 매핑 · 프로젝트 레지스트리를 쓴다.
            lang.attach(pw_lang::methods::HostParts {
                paths: config.paths.clone(),
                global: config.global.clone(),
                projects: projects.clone(),
            });
            trust.attach(projects.clone());
            tauri::async_runtime::spawn(trust::follow(
                trust::subscribe(host.bus()),
                projects.clone(),
                runtime,
            ));
        }
    }
    host
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
    let host = Arc::new(local_host(
        paths.clone(),
        Arc::clone(&diagnostics),
        extensions,
        Arc::clone(&i18n),
    ));
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
        .manage(launch::Launch::new(host.registry().cloned()))
        .manage(windows::PendingOpens::default())
        .manage(windows::WebviewStore(webview_store))
        .manage(diagnostics)
        .manage(perf)
        .manage(update_settings)
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
            update::on_startup(app.handle(), auto_update);
            // 주 인스턴스만 MCP 소켓을 연다(떼어 띄우기 전의 프로세스 · 두 번째 실행은 여기 오지 않는다).
            #[cfg(unix)]
            mcp::serve_socket(host.clone(), paths.clone(), identifier.clone());
            Ok(())
        })
        .run(context)
        .expect("Tauri 앱을 시작하지 못했다");
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

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

    /// MCP 툴이 되는 메서드는 정확히 이 목록이다. 모든 메서드 크레이트(pw-core · pw-diag · pw-ext · pw-i18n)가 링크되는
    /// 자리라 어느 크레이트의 선언에 `mcp: true` 를 달아도 여기서 실패한다 — 툴을 늘리려면 이 목록을 함께 고친다.
    /// 앱 Host 와 헤드리스 Host 모두 목록의 모든 툴을 처리해야 한다(헤드리스가 목록에만 내고 못 부르는 툴이 없게).
    #[test]
    fn mcp_tools_are_exactly_these() {
        const TOOLS: [&str; 2] = ["project_info", "project_list"];
        let linked: Vec<String> = pw_mcp::Catalog::linked()
            .tools()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert_eq!(linked, TOOLS, "`mcp: true` 선언이 바뀌었다");

        let absent = std::env::temp_dir().join("pitwall-mcp-tools-test-absent");
        let paths = AppPaths::under(&absent);
        let app = Arc::new(local_host(
            paths.clone(),
            Arc::new(Diagnostics::new("unused-log-dir", "0.0.0")),
            Arc::new(Registry::default()),
            no_languages(),
        ));
        let headless = mcp::headless_host(&paths);
        for (label, host) in [("app", &app), ("headless", &headless)] {
            let names: Vec<String> = mcp::services(host, &paths, "test")
                .catalog
                .tools()
                .iter()
                .map(|t| t.name.clone())
                .collect();
            assert_eq!(names, TOOLS, "{label} Host 가 처리하지 않는 툴이 있다");
        }
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
        );
        let routed: BTreeSet<String> = host
            .router()
            .methods()
            .into_iter()
            .map(|m| m.name.to_owned())
            .collect();
        assert!(!routed.is_empty());
        assert_eq!(
            generated, routed,
            "`mise run gen` 을 돌리고, 메서드를 선언한 크레이트를 pw-rpc-gen 에 링크했는지 본다"
        );
    }
}
