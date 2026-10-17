// 릴리스 빌드에서 Windows 가 콘솔 창을 함께 띄우지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_core::{HostRouter, LocalHost, Scope};
use pw_diag::Diagnostics;
use pw_ext::{LoadOptions, Registry};

mod cli;
mod config_log;
mod diag;
mod instance;
mod ipc;
mod launch;
mod menu;
mod shell_command;
mod windows;

/// 데스크톱 프로세스 안의 Host 의 id. 프론트엔드가 프로젝트 없이 연결할 때 이 id 로 고른다(src/host-demo.tsx).
const LOCAL_HOST_ID: &str = "local";

/// 앱이 쓰는 LocalHost. 생성물과 등록된 메서드를 맞춰 보는 테스트도 이것을 쓴다.
fn local_host(
    paths: AppPaths,
    diagnostics: Arc<Diagnostics>,
    extensions: Arc<Registry>,
) -> LocalHost {
    LocalHost::builder(LOCAL_HOST_ID)
        .config(paths)
        .methods(move |router| pw_diag::methods::register(router, diagnostics))
        .methods(move |router| pw_ext::methods::register(router, extensions))
        .build()
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

/// `pitwall mcp`(M0-17). 앱을 띄우지 않고 단일 인스턴스로 전달하지도 않는다. stdout 은 MCP 의 stdio 가 쓰므로 비워 둔다.
fn run_mcp() -> ! {
    eprintln!("pitwall mcp: not implemented yet");
    std::process::exit(2);
}

/// 단일 인스턴스 키(instance.rs). debug 빌드만 빌드 폴더와 `$DATA` 로 가른다.
fn instance_identifier(base: &str, paths: &AppPaths) -> String {
    if !cfg!(debug_assertions) {
        return instance::identifier(base, None);
    }
    // 아직 없는 `$DATA` 도 나중에 만들어진 뒤와 같은 값이 나오게 레지스트리와 같은 정규화를 쓴다.
    let data =
        pw_core::registry::path::normalize(&paths.data).unwrap_or_else(|_| paths.data.clone());
    instance::identifier(
        base,
        Some((std::path::Path::new(env!("CARGO_MANIFEST_DIR")), &data)),
    )
}

fn main() {
    // 서브커맨드는 무엇보다 먼저 가른다 — 로그 · 확장 · Host 를 만들지 않고, 단일 인스턴스 플러그인에도 닿지 않는다.
    if std::env::args_os().nth(1).is_some_and(|a| a == "mcp") {
        run_mcp();
    }
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
    let host = Arc::new(local_host(
        paths.clone(),
        Arc::clone(&diagnostics),
        extensions,
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
    let registry = host.registry().cloned();
    let config_events = config_log::subscribe(host.bus());
    let router = Arc::new(HostRouter::new());
    router
        .add_host(host.clone())
        .expect("빈 라우터에 Host 를 처음 등록한다");

    let mut context = tauri::generate_context!();
    let identifier = instance_identifier(&context.config().identifier, &paths);
    tracing::info!(%identifier, "single-instance key");
    context.config_mut().identifier = identifier;

    // 단일 인스턴스 플러그인은 가장 먼저 등록한다 — 두 번째 실행은 이 플러그인의 setup 에서 인자와 cwd 를 기존 앱에 넘기고
    // 종료하므로 그 뒤의 플러그인 · setup 이 돌지 않는다.
    ipc::install(tauri::Builder::default(), router)
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            launch::dispatch(app, argv, cwd)
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(launch::Launch::new(registry.clone()))
        .manage(windows::PendingOpens::default())
        .manage(diagnostics)
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .setup(move |app| {
            tauri::async_runtime::spawn(config_log::run(config_events));
            // 레지스트리를 미리 열어 상태를 로그에 남긴다(정상이 아니면 경고 — 손상 · 잠김 · 읽기 전용). 열기는 잠긴 파일을 최대 2초 기다리고
            // 무결성 검사를 하므로 창을 띄우는 스레드가 아니라 블로킹 스레드에서 한다.
            if let Some(registry) = registry {
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
            // 첫 실행의 인자도 두 번째 실행과 같은 길로 연다(launch.rs 머리 주석).
            let (argv, cwd) = launch::invocation_from_env();
            launch::dispatch(app.handle(), argv, cwd);
            Ok(())
        })
        .run(context)
        .expect("Tauri 앱을 시작하지 못했다");
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

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
