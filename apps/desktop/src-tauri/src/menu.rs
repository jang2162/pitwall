//! 네이티브 메뉴. 키맵 액션에 해당하는 항목은 id 가 액션 id 이고, 단축키(accelerator)는 웹뷰의 키맵 엔진이 병합한 결과를
//! `keymap_sync_menu` 로 받아 바꾼다(docs/design/10-ui.md §4.4 「네이티브 메뉴 동기화」). 병합은 웹뷰
//! (`packages/keymap`)에만 있다 — 메뉴는 결과만 받는다. 키맵 파일이 바뀌면 웹뷰가 Host 범위 `config.changed` 로 알고
//! 다시 보내므로 메뉴도 재시작 없이 바뀐다.
//!
//! 메뉴 항목을 고르면(단축키로 고른 것 포함): 네이티브가 할 수 있는 것(설정 창)은 여기서 하고, 웹뷰의 액션은 포커스된
//! 창에 [`MENU_ACTION_EVENT`] 로 액션 id 를 보낸다(`src/keymap/native.ts`).

use std::collections::HashMap;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, EventTarget, Manager, Runtime};

use crate::{diag, windows};

const NEW_PROJECT_WINDOW: &str = "new-project-window";
const COPY_DIAGNOSTICS: &str = "copy-diagnostics";
/// 키맵 액션 id 를 그대로 메뉴 id 로 쓰는 항목.
const SETTINGS_ACTION: &str = "settings.open";
const FIND_ACTION: &str = "action.find";

/// 메뉴에서 고른 웹뷰 액션을 포커스된 창에 보내는 이벤트. `src/keymap/native.ts` 의 `MENU_ACTION_EVENT`.
pub const MENU_ACTION_EVENT: &str = "keymap://run-action";

/// 키맵이 단축키를 정하는 메뉴 항목. `keymap_sync_menu` 가 이것만 바꾼다 — 메뉴에 없는 액션은 메뉴에 넣지 않는다.
pub struct KeymapMenuItems<R: Runtime> {
    items: Vec<MenuItem<R>>,
}

impl<R: Runtime> KeymapMenuItems<R> {
    /// 받은 accelerator 를 메뉴 항목에 건다. 받은 맵에 없는 항목은 그대로 둔다. `None` 이면 단축키 표시를 지운다.
    /// 반환값은 바꾼 항목의 id. 한 항목이 실패해도 나머지는 건다.
    pub fn apply(
        &self,
        accelerators: &HashMap<String, Option<String>>,
    ) -> Result<Vec<String>, String> {
        let mut applied = Vec::new();
        let mut errors = Vec::new();
        for item in &self.items {
            let id = item.id().as_ref();
            let Some(accelerator) = accelerators.get(id) else {
                continue;
            };
            match item.set_accelerator(accelerator.as_deref()) {
                Ok(()) => applied.push(id.to_owned()),
                Err(e) => errors.push(format!("{id}: {e}")),
            }
        }
        if errors.is_empty() {
            Ok(applied)
        } else {
            Err(errors.join("; "))
        }
    }
}

/// 웹뷰의 키맵 엔진이 병합한 단축키(액션 id → muda accelerator 또는 `null`)를 메뉴에 건다.
/// 창마다 같은 값을 보내므로 여러 번 불려도 결과는 같다.
/// 단위 시험이 없다 — muda 는 macOS 에서 메뉴 항목을 메인 스레드에서만 만들게 해 시험 스레드에서 패닉한다. 앱을 띄워
/// keymap.toml 을 고치고 메뉴의 단축키 표시가 바뀌는지로 확인한다.
#[tauri::command]
pub async fn keymap_sync_menu<R: Runtime>(
    app: AppHandle<R>,
    accelerators: HashMap<String, Option<String>>,
) -> Result<(), String> {
    app.state::<KeymapMenuItems<R>>()
        .apply(&accelerators)
        .map(drop)
}

// 처음 단축키는 기본 키맵(extensions/keymap-default/keymaps/default.toml)과 같게 둔다. 웹뷰가 뜨면 병합 결과로 덮는다.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    #[cfg(target_os = "macos")]
    let settings_key = "Cmd+Comma";
    #[cfg(not(target_os = "macos"))]
    let settings_key = "Ctrl+Alt+KeyS";
    let settings = MenuItem::with_id(app, SETTINGS_ACTION, "Settings…", true, Some(settings_key))?;
    let find_action = MenuItem::with_id(
        app,
        FIND_ACTION,
        "Find Action…",
        true,
        Some("CmdOrCtrl+Shift+KeyA"),
    )?;
    app.manage(KeymapMenuItems {
        items: vec![settings.clone(), find_action.clone()],
    });
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

    // macOS 는 제목이 「Help」인 메뉴를 도움말 메뉴로 잡아 검색 칸을 붙인다. 액션 찾기는 IntelliJ 처럼 Help 에 둔다.
    let help_menu = Submenu::with_items(
        app,
        "Help",
        true,
        &[
            &find_action,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                COPY_DIAGNOSTICS,
                "Copy Diagnostic Info",
                true,
                None::<&str>,
            )?,
        ],
    )?;

    let menu = Menu::with_items(app, &[&file_menu, &edit_menu, &window_menu, &help_menu])?;
    #[cfg(target_os = "macos")]
    menu.prepend(&app_menu)?;

    Ok(menu)
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let result = match event.id().as_ref() {
        NEW_PROJECT_WINDOW => windows::open_project(app)
            .map(drop)
            .map_err(|e| e.to_string()),
        SETTINGS_ACTION => windows::open_settings(app)
            .map(drop)
            .map_err(|e| e.to_string()),
        COPY_DIAGNOSTICS => diag::copy_to_clipboard(app),
        FIND_ACTION => run_in_focused_window(app, FIND_ACTION),
        _ => Ok(()),
    };
    if let Err(error) = result {
        tracing::error!(menu = ?event.id(), %error, "menu action failed");
    }
}

/// 웹뷰 액션을 포커스된 창에서 실행하게 한다. 포커스된 창이 없으면(macOS 에서 창을 모두 닫은 경우) 할 것이 없다.
fn run_in_focused_window<R: Runtime>(app: &AppHandle<R>, action: &str) -> Result<(), String> {
    let window = app
        .webview_windows()
        .into_values()
        .find(|w| w.is_focused().unwrap_or(false))
        .ok_or_else(|| "no focused window".to_owned())?;
    app.emit_to(
        EventTarget::webview_window(window.label()),
        MENU_ACTION_EVENT,
        action,
    )
    .map_err(|e| e.to_string())
}
