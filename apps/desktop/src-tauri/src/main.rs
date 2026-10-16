// 릴리스 빌드에서 Windows 가 콘솔 창을 함께 띄우지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_core::{HostRouter, LocalHost};
use pw_diag::Diagnostics;

mod diag;
mod ipc;
mod menu;
mod windows;

/// 데스크톱 프로세스 안의 Host 의 id. 프론트엔드가 프로젝트 없이 연결할 때 이 id 로 고른다(src/host-demo.tsx).
const LOCAL_HOST_ID: &str = "local";

/// 앱이 쓰는 LocalHost. 생성물과 등록된 메서드를 맞춰 보는 테스트도 이것을 쓴다.
fn local_host(paths: AppPaths, diagnostics: Arc<Diagnostics>) -> LocalHost {
    LocalHost::builder(LOCAL_HOST_ID)
        .config(paths)
        .methods(move |router| pw_diag::methods::register(router, diagnostics))
        .build()
}

fn main() {
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
    let host = Arc::new(local_host(paths, Arc::clone(&diagnostics)));
    // 「활성 기능」은 지금 Host 가 처리하는 메서드의 모듈 이름이다. 확장 · 설정 기능이 들어오면 그것으로 바꾼다.
    diagnostics.set_features(
        host.router()
            .methods()
            .iter()
            .filter_map(|m| m.name.split_once('.').map(|(module, _)| module.to_owned())),
    );
    let router = Arc::new(HostRouter::new());
    router
        .add_host(host.clone())
        .expect("빈 라우터에 Host 를 처음 등록한다");

    ipc::install(tauri::Builder::default(), router)
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(diagnostics)
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .setup(move |app| {
            // 감시를 못 켜도 앱은 뜬다 — 설정은 시작할 때 읽은 값으로 남는다.
            if let Err(e) = host.watch_config(tauri::async_runtime::handle().inner()) {
                eprintln!("config watch disabled: {e}");
            }
            windows::open_welcome(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
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
