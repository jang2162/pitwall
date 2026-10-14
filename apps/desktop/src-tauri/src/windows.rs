use std::sync::atomic::{AtomicU32, Ordering};

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// 창 종류. 프론트엔드는 하나의 엔트리에서 `?window=` 쿼리로 종류를 받는다(src/window-kind.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Welcome,
    Project,
    Settings,
}

impl WindowKind {
    pub fn query_value(self) -> &'static str {
        match self {
            WindowKind::Welcome => "welcome",
            WindowKind::Project => "project",
            WindowKind::Settings => "settings",
        }
    }

    pub fn url(self) -> WebviewUrl {
        WebviewUrl::App(format!("index.html?window={}", self.query_value()).into())
    }

    fn title(self) -> &'static str {
        match self {
            WindowKind::Welcome => "Pitwall",
            WindowKind::Project => "Project",
            WindowKind::Settings => "Settings",
        }
    }

    fn inner_size(self) -> (f64, f64) {
        match self {
            WindowKind::Welcome => (800.0, 520.0),
            WindowKind::Project => (1280.0, 800.0),
            WindowKind::Settings => (900.0, 640.0),
        }
    }
}

pub const WELCOME_LABEL: &str = "welcome";
pub const SETTINGS_LABEL: &str = "settings";

static NEXT_PROJECT_ID: AtomicU32 = AtomicU32::new(1);

fn next_project_label() -> String {
    format!(
        "project-{}",
        NEXT_PROJECT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

pub fn open_welcome<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    open_single(app, WindowKind::Welcome, WELCOME_LABEL)
}

/// 설정 창은 하나뿐이다. 이미 열려 있으면 새로 만들지 않고 앞으로 가져온다.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    open_single(app, WindowKind::Settings, SETTINGS_LABEL)
}

/// 프로젝트 모델이 아직 없으므로(M0-09·M0-12) 부를 때마다 빈 프로젝트 창을 하나 더 연다.
pub fn open_project<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    build(app, WindowKind::Project, &next_project_label())
}

fn open_single<R: Runtime>(
    app: &AppHandle<R>,
    kind: WindowKind,
    label: &str,
) -> tauri::Result<WebviewWindow<R>> {
    if let Some(window) = app.get_webview_window(label) {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
        return Ok(window);
    }
    build(app, kind, label)
}

fn build<R: Runtime>(
    app: &AppHandle<R>,
    kind: WindowKind,
    label: &str,
) -> tauri::Result<WebviewWindow<R>> {
    let (width, height) = kind.inner_size();
    let builder = WebviewWindowBuilder::new(app, label, kind.url())
        .title(kind.title())
        .inner_size(width, height)
        // 웹뷰 설정으로 끌 수 있는 기본 동작은 여기서 끈다. 설정이 없는 것은 src/webview-guards.ts 가 막는다.
        // - 줌: Windows 는 WebView2 의 IsZoomControlEnabled·IsPinchZoomEnabled 가 꺼진다.
        //   macOS·Linux 는 true 일 때만 줌 폴리필을 넣고, WKWebView 의 핀치 확대(allowsMagnification)는 기본이 꺼짐이다.
        // - 스와이프 뒤로/앞으로: wry 의 back_forward_navigation_gestures 기본값(false)이 세 OS 모두 끈다.
        .zoom_hotkeys_enabled(false);

    // macOS 는 웹뷰를 타이틀바 자리까지 늘리고 신호등 버튼만 남긴다. 그 자리는 프론트엔드가 비워 둔다
    // (src/styles.css 의 .titlebar). 다른 OS 는 기본 장식을 쓴다.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_match_frontend_window_kinds() {
        // src/window-kind.ts 의 WindowKind 와 같은 문자열이어야 한다. 다르면 프론트엔드가 웰컴 창으로 떨어진다.
        let values: Vec<_> = [
            WindowKind::Welcome,
            WindowKind::Project,
            WindowKind::Settings,
        ]
        .map(WindowKind::query_value)
        .into();
        assert_eq!(values, ["welcome", "project", "settings"]);
    }

    #[test]
    fn url_points_at_single_entry_with_kind_query() {
        match WindowKind::Settings.url() {
            WebviewUrl::App(path) => {
                assert_eq!(path.to_str(), Some("index.html?window=settings"))
            }
            other => panic!("앱 내부 URL 이어야 한다: {other:?}"),
        }
    }

    #[test]
    fn project_labels_are_unique() {
        let first = next_project_label();
        let second = next_project_label();
        assert_ne!(first, second);
        assert!(first.starts_with("project-") && second.starts_with("project-"));
    }
}
