use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

use crate::windows;

const NEW_PROJECT_WINDOW: &str = "new-project-window";
const OPEN_SETTINGS: &str = "open-settings";

// 단축키는 키맵 엔진(M0-13)이 들어오면 거기서 동기화한다. 지금은 OS 관례(⌘,)만 둔다.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let settings = MenuItem::with_id(app, OPEN_SETTINGS, "Settings…", true, Some("CmdOrCtrl+,"))?;
    let new_project = MenuItem::with_id(
        app,
        NEW_PROJECT_WINDOW,
        "New Project Window",
        true,
        None::<&str>,
    )?;

    #[cfg(target_os = "macos")]
    let app_menu = Submenu::with_items(
        app,
        "Pitwall",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &new_project,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    // macOS 가 아니면 앱 메뉴가 없으므로 설정을 File 에 둔다.
    #[cfg(not(target_os = "macos"))]
    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &new_project,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    // macOS 에서 복사·붙여넣기 단축키는 Edit 메뉴 항목이 있어야 웹뷰에 전달된다.
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;

    let menu = Menu::with_items(app, &[&file_menu, &edit_menu, &window_menu])?;
    #[cfg(target_os = "macos")]
    menu.prepend(&app_menu)?;

    Ok(menu)
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let result = match event.id().as_ref() {
        NEW_PROJECT_WINDOW => windows::open_project(app).map(drop),
        OPEN_SETTINGS => windows::open_settings(app).map(drop),
        _ => Ok(()),
    };
    if let Err(error) = result {
        eprintln!("메뉴 {:?} 처리 실패: {error}", event.id());
    }
}
