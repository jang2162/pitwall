// 릴리스 빌드에서 Windows 가 콘솔 창을 함께 띄우지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod menu;
mod windows;

fn main() {
    tauri::Builder::default()
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .setup(|app| {
            windows::open_welcome(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 앱을 시작하지 못했다");
}
