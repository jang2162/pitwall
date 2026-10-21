//! 앱 명령(Tauri `invoke`, `#[tauri::command]`)의 창 종류별 허용 목록. 목록에 없는 명령은 어느 창에서도 거절한다.
//!
//! 앱 매니페스트가 없으면 Tauri 는 등록된 앱 명령을 모든 창에 연다 — capabilities(`capabilities/*.json`)는 플러그인 권한만
//! 가른다. 그래서 [`ipc::install`](crate::ipc::install) 이 invoke handler 를 [`allowed`] 로 감싼다: 부른 창의 label 을
//! [`WindowKind`] 로 읽어 그 종류에 열린 명령만 넘긴다. 모르는 label 의 창은 아무 명령도 부르지 못한다.
//!
//! 단일 파일 창(M1-24)이 웰컴 창의 명령으로 범위 밖 경로를 묻거나(`path_kind`) 다른 파일의 창을 만들지(`open_file_window`)
//! 못하게 하는 것이 이 목록의 이유다. 명령을 더하면 [`COMMANDS`] 에도 더한다 — 빠뜨리면 모든 창이 거절하고 시험이 잡는다.

use crate::windows::WindowKind;
use crate::windows::WindowKind::{File, Project, Settings, Welcome};

const ALL: &[WindowKind] = &[Welcome, Settings, Project, File];

/// 명령 이름과 그것을 부를 수 있는 창 종류. 각 창이 지금 쓰는 명령만 연다(웹뷰에서 부르는 곳은 머리 주석의 파일).
pub const COMMANDS: &[(&str, &[WindowKind])] = &[
    // 모든 창: Host 연결(src/tauri-transport.ts), 첫 프레임 측정(src/main.tsx), 키맵 액션(src/keymap/native.ts).
    ("rpc_page", ALL),
    ("rpc_connect", ALL),
    ("rpc_send", ALL),
    ("rpc_disconnect", ALL),
    ("perf_first_frame", ALL),
    ("perf_hidden", ALL),
    ("keymap_sync_menu", ALL),
    ("open_settings_window", ALL),
    ("open_project_window", ALL),
    ("window_close_cancelled", ALL),
    ("install_shell_command", ALL),
    // 웰컴 창의 S1 · S2(src/welcome/native.ts).
    ("open_file_window", &[Welcome]),
    ("take_pending_find_project", &[Welcome]),
    ("path_kind", &[Welcome]),
    // 설정 창(src/settings/native.ts).
    ("write_clipboard", &[Settings]),
    // 열기 요청을 받는 창(src/project-open.ts).
    ("take_pending_project_opens", &[Project, File]),
    // 프로젝트 창의 파일 트리 드롭(src/file-tree/tree-drop.ts).
    ("import_drop", &[Project]),
    // 단일 파일 창의 「프로젝트로 열기」(src/file-window/file-window.tsx).
    ("open_file_as_project", &[File]),
];

/// label 이 `label` 인 창이 `command` 를 부를 수 있는가.
pub fn allowed(label: &str, command: &str) -> bool {
    let Some(kind) = WindowKind::of_label(label) else {
        return false;
    };
    COMMANDS
        .iter()
        .any(|(name, kinds)| *name == command && kinds.contains(&kind))
}

/// 거절할 때 웹뷰가 받는 오류.
pub fn refusal(command: &str) -> String {
    format!("`{command}` is not allowed in this window")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::sync::Arc;

    use pw_core::{HostRouter, LocalHost};
    use serde_json::{Value, json};
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{
        INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
    };
    use tauri::webview::InvokeRequest;
    use tauri::{App, Manager, WebviewWindow, WebviewWindowBuilder};

    use super::*;
    use crate::launch::{FindProjectRequest, Launch};
    use crate::windows::{FileWindows, PendingOpens, ProjectOpen};

    const COMMON: &[&str] = &[
        "rpc_page",
        "rpc_connect",
        "rpc_send",
        "rpc_disconnect",
        "perf_first_frame",
        "perf_hidden",
        "keymap_sync_menu",
        "open_settings_window",
        "open_project_window",
        "window_close_cancelled",
        "install_shell_command",
    ];

    fn allowed_in(label: &str) -> BTreeSet<&'static str> {
        COMMANDS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| allowed(label, name))
            .collect()
    }

    fn common_and(extra: &[&'static str]) -> BTreeSet<&'static str> {
        COMMON.iter().chain(extra).copied().collect()
    }

    /// 각 창 종류에 열린 명령은 그 창이 지금 쓰는 것 그대로다. 모르는 label 의 창(`main`)은 아무것도 못 부른다.
    #[test]
    fn each_window_kind_may_call_only_the_commands_it_uses() {
        assert_eq!(
            allowed_in("welcome"),
            common_and(&["open_file_window", "take_pending_find_project", "path_kind"])
        );
        assert_eq!(allowed_in("settings"), common_and(&["write_clipboard"]));
        assert_eq!(
            allowed_in("project-ab12"),
            common_and(&["take_pending_project_opens", "import_drop"])
        );
        assert_eq!(
            allowed_in("file-1"),
            common_and(&["take_pending_project_opens", "open_file_as_project"])
        );
        assert!(allowed_in("main").is_empty());
        assert!(!allowed("welcome", "no_such_command"));
    }

    /// `generate_handler!` 에 등록한 명령(ipc.rs)과 이 목록이 같다 — 등록만 하고 목록에 없으면 모든 창이 거절하고, 목록에만 있는
    /// 이름은 낡은 것이다.
    #[test]
    fn every_registered_command_is_in_the_list() {
        let source = include_str!("ipc.rs");
        let start = source
            .find("generate_handler![")
            .expect("ipc.rs registers the commands")
            + "generate_handler![".len();
        let end = start + source[start..].find(']').unwrap();
        let registered: BTreeSet<&str> = source[start..end]
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(|path| path.rsplit("::").next().unwrap())
            .collect();
        let listed: BTreeSet<&str> = COMMANDS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            registered.len(),
            COMMANDS.len(),
            "a command is listed twice"
        );
        assert_eq!(registered, listed);
    }

    fn app() -> App<MockRuntime> {
        let router = Arc::new(HostRouter::new());
        router.add_host(Arc::new(LocalHost::new("local"))).unwrap();
        crate::ipc::install(mock_builder(), router)
            .manage(Launch::new(None))
            .manage(PendingOpens::default())
            .manage(FileWindows::default())
            .build(mock_context(noop_assets()))
            .unwrap()
    }

    fn invoke(
        webview: &WebviewWindow<MockRuntime>,
        cmd: &str,
        args: Value,
    ) -> Result<Value, Value> {
        get_ipc_response(
            webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                // 앱 페이지의 주소(ipc.rs 시험의 `invoke` 와 같은 분기).
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<Value>().unwrap())
    }

    fn refused(command: &str) -> Result<Value, Value> {
        Err(json!(refusal(command)))
    }

    /// 단일 파일 창은 웰컴 창의 명령을 부르지 못한다: 범위 밖 경로의 종류를 묻지 못하고, 다른 파일의 창을 만들지 못하고, 웰컴
    /// 창의 보류 S2 요청을 꺼내 비우지 못한다. 같은 명령이 웰컴 창에서는 그대로 돈다.
    #[test]
    fn a_file_window_cannot_call_the_welcome_commands() {
        let app = app();
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(fs::canonicalize(tmp.path()).unwrap());
        let mine = base.join("a.txt");
        let other = base.join("b.txt");
        fs::write(&mine, "a").unwrap();
        fs::write(&other, "b").unwrap();
        let file = crate::windows::open_file(app.handle(), &mine, &ProjectOpen::default()).unwrap();
        let request = FindProjectRequest {
            path: other.display().to_string(),
            line: None,
            col: None,
        };
        app.state::<Launch>().pending().replace(request.clone());

        let base_arg = json!({"path": base.display().to_string()});
        assert_eq!(
            invoke(&file, "path_kind", base_arg.clone()),
            refused("path_kind")
        );
        assert_eq!(
            invoke(
                &file,
                "open_file_window",
                json!({"path": other.display().to_string()})
            ),
            refused("open_file_window")
        );
        assert_eq!(app.webview_windows().len(), 1);
        assert_eq!(app.state::<FileWindows>().label_of(&other), None);
        assert_eq!(
            invoke(&file, "take_pending_find_project", json!({})),
            refused("take_pending_find_project")
        );
        assert_eq!(*app.state::<Launch>().pending(), Some(request.clone()));
        // 목록에 없는 명령은 handler 에 닿기 전에 거절한다.
        assert_eq!(
            invoke(&file, "no_such_command", json!({})),
            refused("no_such_command")
        );

        let welcome = WebviewWindowBuilder::new(&app, "welcome", Default::default())
            .build()
            .unwrap();
        assert_eq!(invoke(&welcome, "path_kind", base_arg), Ok(json!("dir")));
        assert_eq!(
            invoke(&welcome, "take_pending_find_project", json!({})),
            Ok(serde_json::to_value(&request).unwrap())
        );
    }
}
