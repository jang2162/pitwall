// 릴리스 빌드에서 Windows 가 콘솔 창을 함께 띄우지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use pw_core::{HostRouter, LocalHost};

mod ipc;
mod menu;
mod windows;

/// 데스크톱 프로세스 안의 Host 의 id. 프론트엔드가 프로젝트 없이 연결할 때 이 id 로 고른다(src/rpc-temp.ts).
const LOCAL_HOST_ID: &str = "local";

fn main() {
    let router = Arc::new(HostRouter::new());
    router
        .add_host(Arc::new(LocalHost::new(LOCAL_HOST_ID)))
        .expect("빈 라우터에 Host 를 처음 등록한다");

    ipc::install(tauri::Builder::default(), router)
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .setup(|app| {
            windows::open_welcome(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 앱을 시작하지 못했다");
}
