//! 네이티브 메뉴. 키맵 액션에 해당하는 항목은 id 가 액션 id 이고, 단축키(accelerator)는 웹뷰의 키맵 엔진이 병합한 결과를
//! `keymap_sync_menu` 로 받아 바꾼다(docs/design/10-ui.md §4.4 「네이티브 메뉴 동기화」). 병합은 웹뷰
//! (`packages/keymap`)에만 있다 — 메뉴는 결과만 받는다. 키맵 파일이 바뀌면 웹뷰가 Host 범위 `config.changed` 로 알고
//! 다시 보내므로 메뉴도 재시작 없이 바뀐다.
//!
//! 메뉴 항목을 고르면(단축키로 고른 것 포함): 키맵 액션은 포커스된 창에 [`MENU_ACTION_EVENT`] 로 액션 id 를 보내
//! 웹뷰가 실행한다(`src/keymap/native.ts`). 설정 창처럼 네이티브가 할 수 있는 것도 포커스된 창이 있으면 웹뷰로 보낸다 —
//! 웹뷰가 「키로 방금 실행한 액션이 같은 눌림의 accelerator 로 또 온 것」을 버리는 가드(`src/keymap/keymap-provider.tsx`
//! 의 `MENU_DUPLICATE_MS`)를 같이 타게. 포커스된 창이 없을 때(macOS 에서 창을 모두 닫음)만 여기서 직접 한다.
//!
//! 메뉴 문자열은 UI 언어(`crates/pw-i18n`, `extensions/i18n-default/i18n/*/menu.ftl`)로 만든다. 언어가 바뀌면
//! [`relabel`] 이 항목의 글자만 바꾼다(`src/i18n.rs`) — 메뉴를 다시 만들지 않으므로 동기화한 단축키가 남는다.
//!
//! ⚠ 메뉴에 넣는 액션은 두 번 실행돼도 결과가 같아야 한다(열기 · 보이기). 가드는 짧은 시간 창만 본다. 토글 액션을 메뉴에
//! 넣으려면 그 전에 키 경로와 메뉴 경로가 한 번만 오는지 플랫폼마다 실측하거나 더 단단한 중복 실행 가드를 둔다.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use pw_i18n::{FluentArgs, I18n, Localizer};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, EventTarget, Manager, Runtime};

use crate::{diag, windows};

/// 프로젝트 창은 프로젝트에 묶여 열리므로(`launch::open_project`) 빈 프로젝트 창 대신 웰컴 창(새 프로젝트 · 폴더 열기)을 연다.
const WELCOME_WINDOW: &str = "welcome-window";
const COPY_DIAGNOSTICS: &str = "copy-diagnostics";
/// 키맵 액션 id 를 그대로 메뉴 id 로 쓰는 항목.
const SETTINGS_ACTION: &str = "settings.open";
const FIND_ACTION: &str = "action.find";
/// 「Shell 명령 설치」(F2-1). macOS 앱 메뉴에만 있다.
#[cfg(target_os = "macos")]
const CLI_INSTALL_ACTION: &str = "cli.install";

/// 메뉴에서 고른 웹뷰 액션을 포커스된 창에 보내는 이벤트. `src/keymap/native.ts` 의 `MENU_ACTION_EVENT`.
pub const MENU_ACTION_EVENT: &str = "keymap://run-action";

/// 키맵 액션 메뉴 항목의 메시지 id.
fn action_label(action: &str) -> Option<&'static str> {
    Some(match action {
        SETTINGS_ACTION => "menu-settings",
        FIND_ACTION => "menu-find-action",
        #[cfg(target_os = "macos")]
        CLI_INSTALL_ACTION => "menu-install-shell-command",
        _ => return None,
    })
}

/// 메뉴 문자열. `$app` 은 앱 이름이다.
fn label<R: Runtime>(app: &AppHandle<R>, l: &Localizer, id: &str) -> String {
    let mut args = FluentArgs::new();
    args.set("app", app.package_info().name.clone());
    l.format(id, Some(&args))
}

/// 키맵 액션이 아닌 메뉴 항목 · 하위 메뉴와 그 메시지 id. 언어가 바뀌면 [`relabel`] 이 글자를 바꾼다.
enum Labeled<R: Runtime> {
    Submenu(Submenu<R>),
    Item(MenuItem<R>),
    Predefined(PredefinedMenuItem<R>),
}

impl<R: Runtime> Labeled<R> {
    fn set_text(&self, text: &str) -> tauri::Result<()> {
        match self {
            Labeled::Submenu(m) => m.set_text(text),
            Labeled::Item(i) => i.set_text(text),
            Labeled::Predefined(i) => i.set_text(text),
        }
    }
}

pub struct MenuLabels<R: Runtime> {
    entries: Vec<(Labeled<R>, &'static str)>,
}

/// 메뉴의 글자를 `l` 의 언어로 바꾼다. 한 항목이 실패해도 나머지는 바꾼다. 단위 시험이 없다 — muda 는 macOS 에서 메뉴 항목을
/// 메인 스레드에서만 만들게 해 시험 스레드에서 패닉한다([`keymap_sync_menu`] 와 같다). 앱에서 언어를 바꿔 확인한다.
pub fn relabel<R: Runtime>(app: &AppHandle<R>, l: &Localizer) -> Result<(), String> {
    let mut errors = Vec::new();
    if let Some(labels) = app.try_state::<MenuLabels<R>>() {
        for (entry, id) in &labels.entries {
            if let Err(e) = entry.set_text(&label(app, l, id)) {
                errors.push(format!("{id}: {e}"));
            }
        }
    }
    if let Some(items) = app.try_state::<KeymapMenuItems<R>>() {
        let entries = items.entries.lock().unwrap_or_else(PoisonError::into_inner);
        for (_, item) in entries.iter() {
            let Some(id) = action_label(item.id().as_ref()) else {
                continue;
            };
            if let Err(e) = item.set_text(label(app, l, id)) {
                errors.push(format!("{id}: {e}"));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// 키맵이 단축키를 정하는 메뉴 항목과 그 항목이 든 하위 메뉴. `keymap_sync_menu` 가 이것만 바꾼다 — 메뉴에 없는 액션은
/// 메뉴에 넣지 않는다.
pub struct KeymapMenuItems<R: Runtime> {
    entries: Mutex<Vec<(Submenu<R>, MenuItem<R>)>>,
}

impl<R: Runtime> KeymapMenuItems<R> {
    /// 받은 accelerator 로 메뉴 항목을 바꾼다. 받은 맵에 없는 항목은 그대로 둔다. `None` 이면 단축키 표시를 지운다.
    /// 반환값은 바꾼 항목의 id. 한 항목이 실패해도 나머지는 바꾼다.
    ///
    /// 항목의 단축키를 고치지 않고 **같은 자리에 새 항목을 만들어 바꿔 끼운다.** muda 0.20 의 macOS `set_accelerator(None)`
    /// 은 아무것도 하지 않아 단축키를 지울 수 없다(unbind 한 키가 메뉴에 남았다). 새 항목에는 `set_accelerator` 로 키를
    /// 건다 — `MenuItem::with_id` 는 읽지 못한 accelerator 를 오류 없이 버리므로 여기서 실패를 받는다.
    pub fn apply(
        &self,
        accelerators: &HashMap<String, Option<String>>,
    ) -> Result<Vec<String>, String> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let mut applied = Vec::new();
        let mut errors = Vec::new();
        for (parent, item) in entries.iter_mut() {
            let id = item.id().as_ref().to_owned();
            let Some(accelerator) = accelerators.get(&id) else {
                continue;
            };
            match replace(parent, item, accelerator.as_deref()) {
                Ok(fresh) => {
                    *item = fresh;
                    applied.push(id);
                }
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

fn replace<R: Runtime>(
    parent: &Submenu<R>,
    item: &MenuItem<R>,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<R>> {
    let fresh = MenuItem::with_id(
        item.app_handle(),
        item.id().clone(),
        item.text()?,
        item.is_enabled()?,
        None::<&str>,
    )?;
    if accelerator.is_some() {
        fresh.set_accelerator(accelerator)?;
    }
    let position = parent
        .items()?
        .iter()
        .position(|i| i.id() == item.id())
        .unwrap_or(0);
    parent.remove(item)?;
    parent.insert(&fresh, position)?;
    Ok(fresh)
}

/// 웹뷰의 키맵 엔진이 병합한 단축키(액션 id → muda accelerator 또는 `null`)를 메뉴에 건다.
/// 창마다 같은 값을 보내므로 여러 번 불려도 결과는 같다. 여기 단축키를 거는 액션은 두 번 실행돼도 결과가 같아야 한다(머리 주석).
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
// 글자는 지금 UI 언어로 만든다(`I18n` 은 앱 상태로 먼저 등록돼 있다, main.rs).
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let l = app.state::<std::sync::Arc<I18n>>().localizer();
    let t = |id: &str| label(app, &l, id);
    let mut labels: Vec<(Labeled<R>, &'static str)> = Vec::new();
    // 하위 메뉴 · 항목을 만들며 메시지 id 를 함께 남긴다.
    let mut item = |id: &'static str, kind: Labeled<R>| labels.push((kind, id));

    #[cfg(target_os = "macos")]
    let settings_key = "Cmd+Comma";
    #[cfg(not(target_os = "macos"))]
    let settings_key = "Ctrl+Alt+KeyS";
    let settings = MenuItem::with_id(
        app,
        SETTINGS_ACTION,
        t("menu-settings"),
        true,
        Some(settings_key),
    )?;
    let find_action = MenuItem::with_id(
        app,
        FIND_ACTION,
        t("menu-find-action"),
        true,
        Some("CmdOrCtrl+Shift+KeyA"),
    )?;
    let welcome = MenuItem::with_id(
        app,
        WELCOME_WINDOW,
        t("menu-welcome-window"),
        true,
        None::<&str>,
    )?;
    item("menu-welcome-window", Labeled::Item(welcome.clone()));
    #[cfg(target_os = "macos")]
    let cli_install = MenuItem::with_id(
        app,
        CLI_INSTALL_ACTION,
        t("menu-install-shell-command"),
        true,
        None::<&str>,
    )?;
    let predefined = |id: &'static str, made: PredefinedMenuItem<R>| (id, made);
    let close_window = predefined(
        "menu-close-window",
        PredefinedMenuItem::close_window(app, Some(&t("menu-close-window")))?,
    );
    #[cfg(target_os = "macos")]
    let app_items = [
        predefined(
            "menu-about",
            PredefinedMenuItem::about(app, Some(&t("menu-about")), None)?,
        ),
        predefined(
            "menu-services",
            PredefinedMenuItem::services(app, Some(&t("menu-services")))?,
        ),
        predefined(
            "menu-hide",
            PredefinedMenuItem::hide(app, Some(&t("menu-hide")))?,
        ),
        predefined(
            "menu-hide-others",
            PredefinedMenuItem::hide_others(app, Some(&t("menu-hide-others")))?,
        ),
        predefined(
            "menu-show-all",
            PredefinedMenuItem::show_all(app, Some(&t("menu-show-all")))?,
        ),
        predefined(
            "menu-quit",
            PredefinedMenuItem::quit(app, Some(&t("menu-quit")))?,
        ),
    ];
    #[cfg(not(target_os = "macos"))]
    let quit = predefined(
        "menu-quit",
        PredefinedMenuItem::quit(app, Some(&t("menu-quit")))?,
    );

    #[cfg(target_os = "macos")]
    let app_menu = {
        let [about, services, hide, hide_others, show_all, quit] = &app_items;
        Submenu::with_items(
            app,
            t("menu-app"),
            true,
            &[
                &about.1,
                &PredefinedMenuItem::separator(app)?,
                &settings,
                &cli_install,
                &PredefinedMenuItem::separator(app)?,
                &services.1,
                &PredefinedMenuItem::separator(app)?,
                &hide.1,
                &hide_others.1,
                &show_all.1,
                &PredefinedMenuItem::separator(app)?,
                &quit.1,
            ],
        )?
    };

    #[cfg(target_os = "macos")]
    let file_menu = Submenu::with_items(
        app,
        t("menu-file"),
        true,
        &[
            &welcome,
            &PredefinedMenuItem::separator(app)?,
            &close_window.1,
        ],
    )?;
    // macOS 가 아니면 앱 메뉴가 없으므로 설정을 File 에 둔다.
    #[cfg(not(target_os = "macos"))]
    let file_menu = Submenu::with_items(
        app,
        t("menu-file"),
        true,
        &[
            &welcome,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &close_window.1,
            &quit.1,
        ],
    )?;

    // macOS 에서 복사·붙여넣기 단축키는 Edit 메뉴 항목이 있어야 웹뷰에 전달된다.
    let edit_items = [
        predefined(
            "menu-undo",
            PredefinedMenuItem::undo(app, Some(&t("menu-undo")))?,
        ),
        predefined(
            "menu-redo",
            PredefinedMenuItem::redo(app, Some(&t("menu-redo")))?,
        ),
        predefined(
            "menu-cut",
            PredefinedMenuItem::cut(app, Some(&t("menu-cut")))?,
        ),
        predefined(
            "menu-copy",
            PredefinedMenuItem::copy(app, Some(&t("menu-copy")))?,
        ),
        predefined(
            "menu-paste",
            PredefinedMenuItem::paste(app, Some(&t("menu-paste")))?,
        ),
        predefined(
            "menu-select-all",
            PredefinedMenuItem::select_all(app, Some(&t("menu-select-all")))?,
        ),
    ];
    let edit_menu = {
        let [undo, redo, cut, copy, paste, select_all] = &edit_items;
        Submenu::with_items(
            app,
            t("menu-edit"),
            true,
            &[
                &undo.1,
                &redo.1,
                &PredefinedMenuItem::separator(app)?,
                &cut.1,
                &copy.1,
                &paste.1,
                &select_all.1,
            ],
        )?
    };

    let window_items = [
        predefined(
            "menu-minimize",
            PredefinedMenuItem::minimize(app, Some(&t("menu-minimize")))?,
        ),
        predefined(
            "menu-maximize",
            PredefinedMenuItem::maximize(app, Some(&t("menu-maximize")))?,
        ),
        predefined(
            "menu-fullscreen",
            PredefinedMenuItem::fullscreen(app, Some(&t("menu-fullscreen")))?,
        ),
    ];
    let window_menu = {
        let [minimize, maximize, fullscreen] = &window_items;
        Submenu::with_items(
            app,
            t("menu-window"),
            true,
            &[&minimize.1, &maximize.1, &fullscreen.1],
        )?
    };

    let copy_diagnostics = MenuItem::with_id(
        app,
        COPY_DIAGNOSTICS,
        t("menu-copy-diagnostics"),
        true,
        None::<&str>,
    )?;
    item(
        "menu-copy-diagnostics",
        Labeled::Item(copy_diagnostics.clone()),
    );
    // 액션 찾기는 IntelliJ 처럼 Help 에 둔다.
    let help_menu = Submenu::with_items(
        app,
        t("menu-help"),
        true,
        &[
            &find_action,
            &PredefinedMenuItem::separator(app)?,
            &copy_diagnostics,
        ],
    )?;
    // macOS 는 제목이 영어 「Help」인 메뉴만 스스로 도움말 메뉴로 잡아 검색 칸을 붙인다. 번역된 제목에서도 붙게 직접 알린다.
    #[cfg(target_os = "macos")]
    help_menu.set_as_help_menu_for_nsapp()?;

    #[cfg(target_os = "macos")]
    {
        item("menu-app", Labeled::Submenu(app_menu.clone()));
        for (id, made) in app_items {
            item(id, Labeled::Predefined(made));
        }
    }
    #[cfg(not(target_os = "macos"))]
    item(quit.0, Labeled::Predefined(quit.1));
    item("menu-file", Labeled::Submenu(file_menu.clone()));
    item(close_window.0, Labeled::Predefined(close_window.1));
    item("menu-edit", Labeled::Submenu(edit_menu.clone()));
    item("menu-window", Labeled::Submenu(window_menu.clone()));
    item("menu-help", Labeled::Submenu(help_menu.clone()));
    for (id, made) in edit_items.into_iter().chain(window_items) {
        item(id, Labeled::Predefined(made));
    }
    app.manage(MenuLabels { entries: labels });

    #[cfg(target_os = "macos")]
    let settings_parent = app_menu.clone();
    #[cfg(not(target_os = "macos"))]
    let settings_parent = file_menu.clone();
    #[allow(unused_mut)]
    let mut entries = vec![
        (settings_parent.clone(), settings),
        (help_menu.clone(), find_action),
    ];
    #[cfg(target_os = "macos")]
    entries.push((settings_parent, cli_install));
    app.manage(KeymapMenuItems {
        entries: Mutex::new(entries),
    });

    let menu = Menu::with_items(app, &[&file_menu, &edit_menu, &window_menu, &help_menu])?;
    #[cfg(target_os = "macos")]
    menu.prepend(&app_menu)?;

    Ok(menu)
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let result = match event.id().as_ref() {
        WELCOME_WINDOW => windows::open_welcome(app)
            .map(drop)
            .map_err(|e| e.to_string()),
        #[cfg(target_os = "macos")]
        CLI_INSTALL_ACTION => run_in_focused_window(app, CLI_INSTALL_ACTION).or_else(|_| {
            crate::shell_command::install_in_background(app);
            Ok(())
        }),
        SETTINGS_ACTION => run_in_focused_window(app, SETTINGS_ACTION).or_else(|_| {
            windows::open_settings(app)
                .map(drop)
                .map_err(|e| e.to_string())
        }),
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
