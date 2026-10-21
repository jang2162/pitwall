use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use pw_i18n::{I18n, Localizer};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, EventTarget, Manager, Runtime, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

/// 네이티브 대화상자의 부모 창: 포커스된 창, 없으면 아무 창. 부모 없는 대화상자는 앱이 활성화되지 않았으면 화면에 오르지
/// 않았다(macOS, 창 목록에 오프스크린으로만 생김) — 창이 있으면 그 창의 시트로 띄운다.
pub fn dialog_parent<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    let windows = app.webview_windows();
    windows
        .values()
        .find(|w| w.is_focused().unwrap_or(false))
        .or_else(|| windows.values().next())
        .cloned()
}

/// 창 종류. 프론트엔드는 하나의 엔트리에서 `?window=` 쿼리로 종류를 받는다(src/window-kind.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Welcome,
    Project,
    Settings,
    /// 단일 파일 뷰어 창(M1-24). 프로젝트 없이 파일 하나를 열고, 그 창의 Host 연결은 그 파일에만 닿는다([`FileWindows`]).
    File,
}

impl WindowKind {
    pub fn query_value(self) -> &'static str {
        match self {
            WindowKind::Welcome => "welcome",
            WindowKind::Project => "project",
            WindowKind::Settings => "settings",
            WindowKind::File => "file",
        }
    }

    pub fn url(self) -> WebviewUrl {
        self.url_with(&[])
    }

    /// `?window=` 뒤에 쿼리 인자를 더 붙인 주소. 값은 퍼센트 인코딩한다(프론트엔드는 `URLSearchParams` 로 읽는다).
    pub fn url_with(self, params: &[(&str, &str)]) -> WebviewUrl {
        let mut url = format!("index.html?window={}", self.query_value());
        for (key, value) in params {
            url.push('&');
            url.push_str(key);
            url.push('=');
            url.push_str(&encode_query_value(value));
        }
        WebviewUrl::App(url.into())
    }

    /// OS 창 제목(창 전환기 · Window 메뉴 · Dock 에 보인다). 웰컴 창은 앱 이름이라 번역하지 않는다
    /// (docs/design/07-extensibility.md 「UI 언어 (i18n)」 표의 「번역하지 않는 것」 행). 웹뷰의 제목 줄과 같은 메시지다
    /// (`src/i18n/app-strings.ts` 의 `windowTitles`).
    ///
    /// 단일 파일 창은 그 파일의 이름이라(번역하지 않는다) 이 값이 없다 — [`open_file`] 이 단다.
    pub fn title(self, l: &Localizer) -> Option<String> {
        match self {
            WindowKind::Welcome => Some("Pitwall".to_owned()),
            WindowKind::Project => Some(l.text("window-title-project")),
            WindowKind::Settings => Some(l.text("window-title-settings")),
            WindowKind::File => None,
        }
    }

    /// 창 label 의 종류. 이 모듈이 만든 창이 아니면 `None`.
    pub fn of_label(label: &str) -> Option<Self> {
        match label {
            WELCOME_LABEL => Some(WindowKind::Welcome),
            SETTINGS_LABEL => Some(WindowKind::Settings),
            _ if label.starts_with(PROJECT_LABEL_PREFIX) => Some(WindowKind::Project),
            _ if label.starts_with(FILE_LABEL_PREFIX) => Some(WindowKind::File),
            _ => None,
        }
    }

    fn inner_size(self) -> (f64, f64) {
        match self {
            WindowKind::Welcome => (800.0, 520.0),
            WindowKind::Project => (1280.0, 800.0),
            WindowKind::Settings => (900.0, 640.0),
            WindowKind::File => (960.0, 720.0),
        }
    }
}

/// RFC 3986 의 unreserved 문자만 그대로 두고 나머지 바이트는 `%XX` 로. 공백도 `+` 가 아니라 `%20` 이다
/// (`URLSearchParams` 는 `+` 를 공백으로 읽으므로 `+` 자체도 인코딩해야 한다).
fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 웹뷰 데이터 저장소 id(instance.rs 의 `webview_store_id`). 기본 `$DATA` 로 뜬 앱은 `None` — 번들의 기본 저장소를 쓴다.
pub struct WebviewStore(pub Option<[u8; 16]>);

pub const WELCOME_LABEL: &str = "welcome";
pub const SETTINGS_LABEL: &str = "settings";

/// 프로젝트 창에 「꺼낼 열기 요청이 있다」를 알리는 이벤트. payload 는 없다 — 요청은 [`PendingOpens`] 에 쌓이고 창이
/// [`take_pending_project_opens`] 로 꺼낸다(`src/project-open.ts`).
///
/// 이벤트에 요청을 실으면 막 만든 창이 잃는다: Tauri 는 JS 리스너가 등록된 웹뷰에만 이벤트를 보내고 쌓아 두지 않는다.
/// 창은 마운트할 때 리스너를 먼저 걸고 큐를 꺼내므로, 리스너가 없던 사이에 온 요청은 그 꺼내기가 받는다.
pub const PROJECT_OPEN_EVENT: &str = "project://open";

/// 프로젝트 창 label → 그 창이 아직 꺼내지 않은 열기 요청(온 순서). 새 창의 첫 요청도 여기로 간다(쿼리로 싣지 않는다 —
/// 경로가 하나다).
#[derive(Default)]
pub struct PendingOpens {
    queues: Mutex<HashMap<String, Vec<ProjectOpen>>>,
    /// [`open_project`] 한 번(창 확인 → 큐 → 창 만들기)을 통째로 직렬화한다. 같은 프로젝트로 두 요청이 동시에 오면 둘 다
    /// 「창 없음」을 보고 둘째의 큐 초기화가 첫째의 요청을 지우기 때문이다. 큐 잠금과 따로 둔다 — 창 만들기는 메인 스레드를
    /// 기다리는데, 메인 스레드에서 도는 동기 명령 [`take_pending_project_opens`] 가 이 잠금을 기다리면 교착한다.
    /// 그래서 `open_project` 는 메인 스레드에서 부르지 않는다(지금은 블로킹 스레드에서만 부른다).
    opening: Mutex<()>,
}

impl PendingOpens {
    fn queues(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<ProjectOpen>>> {
        self.queues.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, label: &str, open: ProjectOpen) {
        self.queues()
            .entry(label.to_owned())
            .or_default()
            .push(open);
    }

    /// 새로 만드는 창의 큐. 같은 label 의 닫힌 창이 꺼내지 않고 남긴 요청은 버린다.
    fn reset(&self, label: &str, first: Option<ProjectOpen>) {
        self.queues()
            .insert(label.to_owned(), first.into_iter().collect());
    }

    pub fn take(&self, label: &str) -> Vec<ProjectOpen> {
        self.queues().remove(label).unwrap_or_default()
    }
}

/// 프로젝트 창이 마운트할 때와 [`PROJECT_OPEN_EVENT`] 를 받을 때 부른다. 부른 창의 요청만 꺼낸다(꺼내면 비운다).
#[tauri::command]
pub fn take_pending_project_opens<R: Runtime>(
    window: WebviewWindow<R>,
    pending: State<'_, PendingOpens>,
) -> Vec<ProjectOpen> {
    pending.take(window.label())
}

/// 프로젝트 창에서 열 것. 모두 비면 창만 앞으로 가져온다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOpen {
    /// 활성 워크트리의 꼭대기(`ActiveWorktree::top`). 등록 안 된 워크트리 경로로 왔을 때(F2-5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// 열 파일의 절대 경로. 창이 고정 탭으로 열고 `line` · `col`(1부터)에 커서를 둔다(`src/editor/editor-workspace.ts`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
}

impl ProjectOpen {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

const PROJECT_LABEL_PREFIX: &str = "project-";

/// 프로젝트 창의 label. 프로젝트마다 창이 하나라(03-architecture §3) label 이 곧 「그 프로젝트의 창」이다.
pub fn project_label(project_id: &str) -> String {
    format!("{PROJECT_LABEL_PREFIX}{project_id}")
}

/// 지금 UI 언어. 언어 상태가 없으면(시험의 mock 앱) 빈 카탈로그라 메시지 id 가 그대로 나온다.
fn localizer<R: Runtime>(app: &AppHandle<R>) -> Localizer {
    match app.try_state::<Arc<I18n>>() {
        Some(i18n) => i18n.localizer(),
        None => pw_i18n::Catalog::default().localizer(&[]),
    }
}

/// 열린 창(`labels`)의 OS 제목을 `l` 의 언어로 바꾼다(언어 전환, `src/i18n.rs`). 이 모듈이 만든 창이 아니면 건너뛴다. 제목은
/// `set_title(label, 제목)` 으로 단다 — 앱은 그 창의 `set_title` 을 넘긴다. 한 창이 실패해도 나머지는 바꾼다.
pub fn retitle(
    labels: impl IntoIterator<Item = String>,
    l: &Localizer,
    mut set_title: impl FnMut(&str, &str) -> Result<(), String>,
) -> Result<(), String> {
    let errors: Vec<String> = labels
        .into_iter()
        .filter_map(|label| {
            let title = WindowKind::of_label(&label)?.title(l)?;
            set_title(&label, &title)
                .err()
                .map(|e| format!("{label}: {e}"))
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

// 아래 open_* 를 Tauri command 에서 부를 때는 async command 로 만든다. Windows 에서 동기 command·이벤트 핸들러
// 안에서 창을 만들면 deadlock 난다(tauri 2.12 WebviewWindowBuilder::new 문서 「Known issues」). 메뉴 이벤트 경로는
// WebView2 콜백 밖이라 해당하지 않는다고 보지만 Windows 에서 확인하지 않았다.
pub fn open_welcome<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    open_single(app, WindowKind::Welcome, WELCOME_LABEL)
}

/// 설정 창은 하나뿐이다. 이미 열려 있으면 새로 만들지 않고 앞으로 가져온다.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    open_single(app, WindowKind::Settings, SETTINGS_LABEL)
}

/// 웹뷰의 `settings.open` 액션(src/keymap/actions.ts). 창을 만드므로 async command 다(위 주석).
#[tauri::command]
pub async fn open_settings_window<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    open_settings(&app).map(drop).map_err(|e| e.to_string())
}

/// 프로젝트 창을 연다. `open` 은 그 창의 [`PendingOpens`] 큐에 넣는다. 창이 이미 있으면 앞으로 가져오고
/// [`PROJECT_OPEN_EVENT`] 로 알린다. 없으면 `?window=project&project=<id>` 창을 만든다(창은 마운트할 때 큐를 꺼낸다).
/// 프로젝트가 레지스트리에 있는지는 부르는 쪽이 본다(`launch::open_project`).
pub fn open_project<R: Runtime>(
    app: &AppHandle<R>,
    project_id: &str,
    open: &ProjectOpen,
) -> tauri::Result<WebviewWindow<R>> {
    let label = project_label(project_id);
    let pending = app.state::<PendingOpens>();
    let _opening = pending
        .opening
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let existing = focus(app, &label)?;
    #[cfg(test)]
    tests::after_focus_check();
    if let Some(window) = existing {
        if !open.is_empty() {
            pending.push(&label, open.clone());
            app.emit_to(EventTarget::webview_window(&label), PROJECT_OPEN_EVENT, ())?;
        }
        return Ok(window);
    }
    pending.reset(&label, (!open.is_empty()).then(|| open.clone()));
    let mut query = vec![("project", project_id)];
    query.extend(crate::perf::project_query(app));
    build_at(
        app,
        WindowKind::Project,
        &label,
        WindowKind::Project.url_with(&query),
    )
}

const FILE_LABEL_PREFIX: &str = "file-";

/// 열린 단일 파일 창: label → 그 창의 파일(정규화한 실제 경로). 같은 파일의 창은 하나다 — 같은 실제 경로로 다시 열면 그 창을
/// 앞으로 가져온다. 창의 Host 연결(ipc.rs `rpc_connect`)은 여기 적힌 파일 하나로 범위를 정한다 — 웹뷰가 보낸 경로가 아니다.
/// 창이 사라지면 지운다([`FileWindows::forget`]).
#[derive(Default)]
pub struct FileWindows {
    state: Mutex<FileWindowsState>,
    /// [`open_file`] 한 번을 통째로 직렬화한다([`PendingOpens::opening`] 과 같은 이유 — 같은 파일로 동시에 온 두 요청이 창을
    /// 둘 만들지 않게).
    opening: Mutex<()>,
}

#[derive(Default)]
struct FileWindowsState {
    files: HashMap<String, PathBuf>,
    next: u64,
}

impl FileWindows {
    fn state(&self) -> std::sync::MutexGuard<'_, FileWindowsState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 그 창의 파일. 단일 파일 창이 아니거나 이미 닫혔으면 `None`.
    pub fn file_of(&self, label: &str) -> Option<PathBuf> {
        self.state().files.get(label).cloned()
    }

    /// 그 파일의 창 label.
    pub fn label_of(&self, file: &Path) -> Option<String> {
        self.state()
            .files
            .iter()
            .find(|(_, f)| f.as_path() == file)
            .map(|(label, _)| label.clone())
    }

    /// 새 창의 label 을 정해 그 파일을 적는다. label 은 다시 쓰이지 않는다 — 닫힌 창의 label 로 늦게 온 연결이 새 창의 파일을
    /// 얻지 못하게.
    fn assign(&self, file: PathBuf) -> String {
        let mut state = self.state();
        state.next += 1;
        let label = format!("{FILE_LABEL_PREFIX}{}", state.next);
        state.files.insert(label.clone(), file);
        label
    }

    /// 창이 사라졌다(ipc.rs 의 `on_window_event`).
    pub fn forget(&self, label: &str) {
        self.state().files.remove(label);
    }
}

/// 단일 파일 창을 연다(M1-24). `file` 은 정규화한 실제 경로다(링크면 그 대상 — 같은 파일 = 같은 실제 경로). 그 파일의 창이 이미
/// 있으면 앞으로 가져오고 `open`(줄 · 열)을 그 창의 [`PendingOpens`] 큐로 보낸다. 없으면 `?window=file&path=<file>` 창을 만든다.
/// 창은 마운트할 때 그 파일을 탭으로 열고 큐를 꺼낸다(프로젝트 창과 같은 `project://open` 길).
pub fn open_file<R: Runtime>(
    app: &AppHandle<R>,
    file: &Path,
    open: &ProjectOpen,
) -> tauri::Result<WebviewWindow<R>> {
    let windows = app.state::<FileWindows>();
    let _opening = windows
        .opening
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let pending = app.state::<PendingOpens>();
    if let Some(label) = windows.label_of(file) {
        if let Some(window) = focus(app, &label)? {
            if !open.is_empty() {
                pending.push(&label, open.clone());
                app.emit_to(EventTarget::webview_window(&label), PROJECT_OPEN_EVENT, ())?;
            }
            return Ok(window);
        }
        // 창은 사라졌는데 아직 지우지 않았다(닫히는 중).
        windows.forget(&label);
    }
    let label = windows.assign(file.to_path_buf());
    pending.reset(&label, (!open.is_empty()).then(|| open.clone()));
    let path = file.display().to_string();
    let built = build_titled(
        app,
        WindowKind::File,
        &label,
        WindowKind::File.url_with(&[("path", &path)]),
        &file_window_title(file),
    );
    if built.is_err() {
        windows.forget(&label);
    }
    built
}

/// 단일 파일 창의 OS 제목: 파일 이름(웹뷰의 제목 줄과 같다, `src/file-window/file-window.tsx`).
fn file_window_title(file: &Path) -> String {
    file.file_name().map_or_else(
        || file.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// 그 label 의 창이 있으면 앞으로 가져와 돌려준다.
fn focus<R: Runtime>(app: &AppHandle<R>, label: &str) -> tauri::Result<Option<WebviewWindow<R>>> {
    let Some(window) = app.get_webview_window(label) else {
        return Ok(None);
    };
    window.unminimize()?;
    window.show()?;
    window.set_focus()?;
    #[cfg(test)]
    tests::focused(label);
    Ok(Some(window))
}

fn open_single<R: Runtime>(
    app: &AppHandle<R>,
    kind: WindowKind,
    label: &str,
) -> tauri::Result<WebviewWindow<R>> {
    if let Some(window) = focus(app, label)? {
        return Ok(window);
    }
    build_at(app, kind, label, kind.url())
}

fn build_at<R: Runtime>(
    app: &AppHandle<R>,
    kind: WindowKind,
    label: &str,
    url: WebviewUrl,
) -> tauri::Result<WebviewWindow<R>> {
    let title = kind.title(&localizer(app)).unwrap_or_default();
    build_titled(app, kind, label, url, &title)
}

fn build_titled<R: Runtime>(
    app: &AppHandle<R>,
    kind: WindowKind,
    label: &str,
    url: WebviewUrl,
    title: &str,
) -> tauri::Result<WebviewWindow<R>> {
    let (width, height) = kind.inner_size();
    let builder = WebviewWindowBuilder::new(app, label, url)
        .title(title)
        .inner_size(width, height)
        // 웹뷰 설정으로 끌 수 있는 기본 동작은 여기서 끈다. 설정이 없는 것은 src/webview-guards.ts 가 막는다.
        // - 줌: Windows 는 WebView2 의 IsZoomControlEnabled·IsPinchZoomEnabled 가 꺼진다.
        //   macOS·Linux 는 true 일 때만 줌 폴리필을 넣고, WKWebView 의 핀치 확대(allowsMagnification)는 기본이 꺼짐이다.
        // - 스와이프 뒤로/앞으로: wry 의 back_forward_navigation_gestures 기본값(false)이 세 OS 모두 끈다.
        .zoom_hotkeys_enabled(false);
    // 기본이 아닌 `$DATA` 로 뜬 앱은 localStorage 를 그 `$DATA` 의 저장소에 둔다(instance.rs 머리 주석). macOS 14+ 만 —
    // 그보다 낮은 macOS 와 다른 OS 에서는 wry 가 무시한다.
    let builder = match app.try_state::<WebviewStore>().and_then(|store| store.0) {
        Some(id) => builder.data_store_identifier(id),
        None => builder,
    };

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
    use std::sync::{Arc, Condvar};
    use std::time::Duration;

    use super::*;

    type Hook = Arc<dyn Fn() + Send + Sync>;

    /// [`open_project`] 가 창을 확인한 직후에 부르는 시험용 자리. 동시 요청 시험이 두 스레드를 거기서 만나게 한다.
    static AFTER_FOCUS_CHECK: Mutex<Option<Hook>> = Mutex::new(None);

    pub(super) fn after_focus_check() {
        let hook = AFTER_FOCUS_CHECK.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    thread_local! {
        /// 이 스레드에서 [`focus`] 가 앞으로 가져온 창의 label. mock 런타임은 포커스를 기억하지 않아 여기 적는다.
        static FOCUSED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    pub(super) fn focused(label: &str) {
        FOCUSED.with(|f| f.borrow_mut().push(label.to_owned()));
    }

    fn take_focused() -> Vec<String> {
        FOCUSED.with(|f| std::mem::take(&mut *f.borrow_mut()))
    }

    #[test]
    fn query_values_match_frontend_window_kinds() {
        // src/window-kind.ts 의 WindowKind 와 같은 문자열이어야 한다. 다르면 프론트엔드가 웰컴 창으로 떨어진다.
        let values: Vec<_> = [
            WindowKind::Welcome,
            WindowKind::Project,
            WindowKind::Settings,
            WindowKind::File,
        ]
        .map(WindowKind::query_value)
        .into();
        assert_eq!(values, ["welcome", "project", "settings", "file"]);
    }

    fn builtin_localizer(locales: &[&str]) -> Localizer {
        let registry =
            pw_ext::Registry::from_roots(&semver::Version::new(0, 0, 0), pw_ext::builtin::roots());
        let locales: Vec<String> = locales.iter().map(|&l| l.to_owned()).collect();
        pw_i18n::Catalog::from_registry(&registry).localizer(&locales)
    }

    /// OS 창 제목은 UI 언어의 메시지다. 웰컴 창은 앱 이름 그대로다.
    #[test]
    fn window_titles_follow_the_ui_language() {
        let kinds = [
            WindowKind::Welcome,
            WindowKind::Project,
            WindowKind::Settings,
        ];
        let ko = builtin_localizer(&["ko", "en"]);
        let en = builtin_localizer(&["en"]);
        assert_eq!(
            kinds.map(|k| k.title(&ko).unwrap()),
            ["Pitwall", "프로젝트", "설정"]
        );
        assert_eq!(
            kinds.map(|k| k.title(&en).unwrap()),
            ["Pitwall", "Project", "Settings"]
        );
        // 단일 파일 창의 제목은 파일 이름이라 언어를 따르지 않는다.
        assert_eq!(WindowKind::File.title(&ko), None);
    }

    /// 언어를 바꿀 때 제목을 고칠 창을 label 로 고른다.
    #[test]
    fn window_kind_is_read_back_from_its_label() {
        assert_eq!(
            WindowKind::of_label(WELCOME_LABEL),
            Some(WindowKind::Welcome)
        );
        assert_eq!(
            WindowKind::of_label(SETTINGS_LABEL),
            Some(WindowKind::Settings)
        );
        assert_eq!(
            WindowKind::of_label(&project_label("ab12")),
            Some(WindowKind::Project)
        );
        assert_eq!(WindowKind::of_label("file-3"), Some(WindowKind::File));
        assert_eq!(WindowKind::of_label("main"), None);
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

    fn labels<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
        app.webview_windows().into_keys().collect()
    }

    #[test]
    fn settings_window_is_opened_once_and_reused() {
        let app = tauri::test::mock_app();
        let first = open_settings(app.handle()).unwrap();
        let second = open_settings(app.handle()).unwrap();
        assert_eq!(first.label(), SETTINGS_LABEL);
        assert_eq!(second.label(), SETTINGS_LABEL);
        assert_eq!(labels(app.handle()), [SETTINGS_LABEL]);
    }

    #[test]
    fn welcome_window_is_opened_once_and_reused() {
        let app = tauri::test::mock_app();
        open_welcome(app.handle()).unwrap();
        open_welcome(app.handle()).unwrap();
        assert_eq!(labels(app.handle()), [WELCOME_LABEL]);
    }

    #[test]
    fn query_values_are_percent_encoded() {
        match WindowKind::Project.url_with(&[("project", "ab12"), ("file", "/a b/c+d&e=f/한.rs")])
        {
            WebviewUrl::App(path) => assert_eq!(
                path.to_str(),
                Some(
                    "index.html?window=project&project=ab12\
                     &file=%2Fa%20b%2Fc%2Bd%26e%3Df%2F%ED%95%9C.rs"
                )
            ),
            other => panic!("앱 내부 URL 이어야 한다: {other:?}"),
        }
    }

    fn app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        app.manage(PendingOpens::default());
        app.manage(FileWindows::default());
        app
    }

    fn take<R: Runtime>(app: &AppHandle<R>, window: &WebviewWindow<R>) -> Vec<ProjectOpen> {
        take_pending_project_opens(window.clone(), app.state())
    }

    fn file(path: &str, line: Option<u32>) -> ProjectOpen {
        ProjectOpen {
            file: Some(path.into()),
            line,
            ..ProjectOpen::default()
        }
    }

    #[test]
    fn a_project_window_is_opened_once_per_project() {
        let app = app();
        let first = open_project(app.handle(), "aaaa", &ProjectOpen::default()).unwrap();
        let again = open_project(app.handle(), "aaaa", &ProjectOpen::default()).unwrap();
        let other = open_project(app.handle(), "bbbb", &ProjectOpen::default()).unwrap();
        assert_eq!(first.label(), "project-aaaa");
        assert_eq!(again.label(), "project-aaaa");
        assert_eq!(other.label(), "project-bbbb");
        let mut labels = labels(app.handle());
        labels.sort();
        assert_eq!(labels, ["project-aaaa", "project-bbbb"]);
    }

    /// 프로젝트 스위처(⌘⇧P, src/search-everywhere/project-search.tsx)가 다른 프로젝트를 고르면 그 창이 이미 있을 때 새로 만들지
    /// 않고 그 창을 앞으로 가져온다 — 열기 요청(파일)이 없어도.
    #[test]
    fn opening_an_already_open_project_brings_that_window_forward() {
        let app = app();
        open_project(app.handle(), "aaaa", &ProjectOpen::default()).unwrap();
        open_project(app.handle(), "bbbb", &ProjectOpen::default()).unwrap();
        assert_eq!(
            take_focused(),
            Vec::<String>::new(),
            "새 창은 앞으로 가져온 것이 아니다"
        );
        let window = open_project(app.handle(), "aaaa", &ProjectOpen::default()).unwrap();
        assert_eq!(window.label(), "project-aaaa");
        assert_eq!(take_focused(), ["project-aaaa"]);
        let mut labels = labels(app.handle());
        labels.sort();
        assert_eq!(labels, ["project-aaaa", "project-bbbb"]);
        assert_eq!(
            take(app.handle(), &window),
            [],
            "빈 요청은 큐에 넣지 않는다"
        );
    }

    #[test]
    fn a_new_project_window_carries_only_its_project_and_queues_the_request() {
        let app = app();
        let open = ProjectOpen {
            worktree: Some("/wt".into()),
            file: Some("/wt/a.rs".into()),
            line: Some(3),
            col: Some(4),
        };
        let window = open_project(app.handle(), "aaaa", &open).unwrap();
        let url = window.url().unwrap();
        let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            query,
            [("window", "project"), ("project", "aaaa")].map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
        assert_eq!(take(app.handle(), &window), [open]);
        assert_eq!(take(app.handle(), &window), [], "꺼내면 비운다");
    }

    /// 측정 모드(perf.rs)면 프로젝트 창 주소에 `perf=1` 이 붙어 프론트엔드가 첫 프레임 표시를 남긴다.
    #[test]
    fn a_project_window_in_measurement_mode_asks_for_the_first_frame_mark() {
        let app = app();
        app.manage(crate::perf::PerfMarks::at(Some("/tmp/marks.jsonl".into())));
        let window = open_project(app.handle(), "aaaa", &ProjectOpen::default()).unwrap();
        let query: Vec<(String, String)> =
            window.url().unwrap().query_pairs().into_owned().collect();
        assert_eq!(
            query,
            [("window", "project"), ("project", "aaaa"), ("perf", "1")]
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
    }

    /// 창이 아직 리스너를 걸지 않았어도(막 만든 창) 요청이 사라지지 않는다 — 창은 마운트할 때 꺼낸다.
    #[test]
    fn requests_wait_in_order_until_the_window_takes_them() {
        let app = app();
        let window = open_project(app.handle(), "aaaa", &file("/r/a.rs", None)).unwrap();
        open_project(app.handle(), "aaaa", &file("/r/b.rs", Some(2))).unwrap();
        open_project(app.handle(), "aaaa", &ProjectOpen::default()).unwrap();
        let other = open_project(app.handle(), "bbbb", &ProjectOpen::default()).unwrap();
        assert_eq!(
            take(app.handle(), &other),
            [],
            "다른 창의 요청은 꺼내지 않는다"
        );
        assert_eq!(
            take(app.handle(), &window),
            [file("/r/a.rs", None), file("/r/b.rs", Some(2))]
        );
    }

    /// 창이 없는 프로젝트로 두 요청이 동시에 온다. 두 스레드는 창 확인 직후에 서로를 기다린다(최대 300ms) — 직렬화가 없으면
    /// 둘 다 「창 없음」을 본 채 만나 한쪽 요청이 사라지거나 창 만들기가 실패한다. 직렬화돼 있으면 둘째는 첫째가 창을 만든
    /// 뒤에야 확인에 닿으므로 첫째는 기다림이 끝나는 대로 혼자 가고, 둘째는 열린 창의 큐에 넣는다.
    #[test]
    fn two_simultaneous_requests_for_a_project_without_a_window_both_arrive() {
        let app = app();
        let met = Arc::new((Mutex::new(0usize), Condvar::new()));
        let hook_met = Arc::clone(&met);
        *AFTER_FOCUS_CHECK.lock().unwrap() = Some(Arc::new(move || {
            let (count, arrived) = &*hook_met;
            let mut count = count.lock().unwrap();
            *count += 1;
            arrived.notify_all();
            let _ = arrived
                .wait_timeout_while(count, Duration::from_millis(300), |n| *n < 2)
                .unwrap();
        }));
        let threads = ["/r/a.rs", "/r/b.rs"].map(|path| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                open_project(&handle, "aaaa", &file(path, None))
                    .map(drop)
                    .map_err(|e| e.to_string())
            })
        });
        let results = threads.map(|t| t.join().unwrap());
        *AFTER_FOCUS_CHECK.lock().unwrap() = None;
        assert!(results.iter().all(Result::is_ok), "{results:?}");
        assert_eq!(labels(app.handle()), ["project-aaaa"]);
        let window = app.get_webview_window("project-aaaa").unwrap();
        let mut got = take(app.handle(), &window);
        got.sort_by(|x, y| x.file.cmp(&y.file));
        assert_eq!(got, [file("/r/a.rs", None), file("/r/b.rs", None)]);
    }

    #[test]
    fn a_new_window_drops_what_a_closed_window_left_unclaimed() {
        let app = app();
        app.state::<PendingOpens>()
            .push("project-aaaa", file("/stale.rs", None));
        let window = open_project(app.handle(), "aaaa", &file("/r/a.rs", None)).unwrap();
        assert_eq!(take(app.handle(), &window), [file("/r/a.rs", None)]);
    }

    #[test]
    fn an_open_project_window_is_notified_only_when_something_was_queued() {
        use std::sync::Arc;

        use tauri::Listener;

        let app = app();
        let window = open_project(app.handle(), "aaaa", &ProjectOpen::default()).unwrap();
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&received);
        window.listen(PROJECT_OPEN_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_owned())
        });
        // 빈 요청은 창만 앞으로 가져온다.
        open_project(app.handle(), "aaaa", &ProjectOpen::default()).unwrap();
        open_project(app.handle(), "aaaa", &file("/r/a.rs", Some(7))).unwrap();
        assert_eq!(*received.lock().unwrap(), ["null"]);
        assert_eq!(take(app.handle(), &window), [file("/r/a.rs", Some(7))]);
        assert_eq!(labels(app.handle()), ["project-aaaa"]);
    }

    /// 같은 실제 경로는 한 창이다 — 다시 열면 그 창을 앞으로 가져오고 줄 · 열만 큐로 보낸다. 다른 파일은 새 창이다. 창의
    /// 파일은 label 로 찾는다(연결의 범위, ipc.rs).
    #[test]
    fn a_file_window_is_opened_once_per_real_path() {
        let app = app();
        let a = Path::new("/x/a.txt");
        let first = open_file(app.handle(), a, &ProjectOpen::default()).unwrap();
        let again = open_file(app.handle(), a, &file("/x/a.txt", Some(9))).unwrap();
        let other =
            open_file(app.handle(), Path::new("/x/b.txt"), &ProjectOpen::default()).unwrap();
        assert_eq!(first.label(), again.label());
        assert_ne!(first.label(), other.label());
        assert_eq!(WindowKind::of_label(first.label()), Some(WindowKind::File));
        let mut labels = labels(app.handle());
        labels.sort();
        assert_eq!(labels.len(), 2);
        let files = app.state::<FileWindows>();
        assert_eq!(files.file_of(first.label()).as_deref(), Some(a));
        assert_eq!(take(app.handle(), &first), [file("/x/a.txt", Some(9))]);
        let query: Vec<(String, String)> =
            first.url().unwrap().query_pairs().into_owned().collect();
        assert_eq!(
            query,
            [("window", "file"), ("path", "/x/a.txt")].map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
        assert_eq!(file_window_title(a), "a.txt");
    }

    /// 닫힌 창의 파일은 잊는다 — 같은 파일을 다시 열면 새 label 의 새 창이고, 옛 label 은 어떤 파일도 가리키지 않는다.
    #[test]
    fn a_closed_file_window_is_forgotten_and_its_label_is_not_reused() {
        let app = app();
        let a = Path::new("/x/a.txt");
        let first = open_file(app.handle(), a, &ProjectOpen::default()).unwrap();
        let old = first.label().to_owned();
        first.destroy().unwrap();
        app.state::<FileWindows>().forget(&old);
        let reopened = open_file(app.handle(), a, &ProjectOpen::default()).unwrap();
        assert_ne!(reopened.label(), old);
        assert_eq!(app.state::<FileWindows>().file_of(&old), None);
    }
}
