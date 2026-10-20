//! 앱 쪽 UI 언어 연결. 본체는 `crates/pw-i18n`.
//!
//! 언어 설정(`settings.toml` 의 `ui.language`)이 바뀌면 전역 설정이 다시 읽히며 `config.changed` 가 나간다 — 손으로 고쳤든
//! `i18n.setLanguage` 로 바꿨든 같은 길이다. 웹뷰는 그 이벤트로 `i18n.get` 을 다시 부르고, 네이티브 메뉴와 OS 창 제목은
//! 여기서 받아 글자를 바꾼다(테마 · 키맵의 재시작 없는 반영과 같은 이벤트 경로).

use std::sync::Arc;

use pw_core::config::TOPIC_CHANGED;
use pw_core::{EventBus, Filter, Scope, Subscription};
use pw_ext::Registry;
use pw_i18n::{Catalog, I18n, Localizer};
use tauri::{AppHandle, Manager, Runtime};

use crate::{menu, windows};

/// 확장 레지스트리의 번역과 OS 언어. 전역 설정은 Host 를 만든 뒤 [`I18n::attach_config`] 로 붙인다.
pub fn init(registry: &Registry) -> Arc<I18n> {
    let os = I18n::system_locales();
    let i18n = I18n::new(Catalog::from_registry(registry), os.clone());
    tracing::info!(
        languages = ?i18n.catalog().locales(),
        os = ?os,
        "ui languages loaded"
    );
    Arc::new(i18n)
}

/// 전역 설정이 다시 읽힌 것. Host 를 만든 직후, 파일 감시를 켜기 전에 연다 — 그 사이의 변경을 놓치지 않게.
pub fn subscribe(bus: &EventBus) -> Subscription {
    bus.subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_CHANGED]))
}

/// 고른 언어가 바뀔 때마다 `on_change` 를 부른다(처음 언어로는 부르지 않는다). 구독이 닫히면 끝난다.
pub async fn follow(
    i18n: Arc<I18n>,
    mut events: Subscription,
    on_change: impl Fn(&Localizer) + Send + 'static,
) {
    let mut current = i18n.locales();
    tracing::info!(locales = ?current, "ui language");
    // 넘친 이벤트(`Lagged`)도 다시 본다 — 그 안에 설정 변경이 있었을 수 있다.
    while events.recv().await.is_some() {
        let next = i18n.locales();
        if next == current {
            continue;
        }
        tracing::info!(locales = ?next, "ui language changed");
        current = next;
        on_change(&i18n.localizer());
    }
}

/// 언어가 바뀌었을 때 고칠 네이티브 UI. 앱은 [`AppUi`] 를 쓰고, 시험은 창 제목 쓰기를 기록하는 구현을 넘긴다(Tauri 의 mock
/// 런타임은 창 제목을 저장하지 않는다 — `title()` 이 늘 빈 문자열이다).
pub trait NativeUi {
    fn relabel_menu(&self, l: &Localizer) -> Result<(), String>;
    /// 열린 창의 label.
    fn window_labels(&self) -> Vec<String>;
    fn set_window_title(&self, label: &str, title: &str) -> Result<(), String>;
}

pub struct AppUi<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> NativeUi for AppUi<R> {
    fn relabel_menu(&self, l: &Localizer) -> Result<(), String> {
        menu::relabel(&self.0, l)
    }

    fn window_labels(&self) -> Vec<String> {
        self.0.webview_windows().into_keys().collect()
    }

    fn set_window_title(&self, label: &str, title: &str) -> Result<(), String> {
        let window = self
            .0
            .get_webview_window(label)
            .ok_or_else(|| "window closed".to_owned())?;
        window.set_title(title).map_err(|e| e.to_string())
    }
}

/// 네이티브 메뉴의 글자와 열린 창의 OS 제목을 `l` 의 언어로 바꾼다. 새로 여는 창은 만들 때 그 언어로 제목을 단다
/// (`windows::build_at`).
pub fn apply_language(ui: &impl NativeUi, l: &Localizer) {
    if let Err(error) = ui.relabel_menu(l) {
        tracing::error!(%error, "menu labels not changed to the new language");
    }
    if let Err(error) = windows::retitle(ui.window_labels(), l, |label, title| {
        ui.set_window_title(label, title)
    }) {
        tracing::error!(%error, "window titles not changed to the new language");
    }
}

/// 언어가 바뀔 때마다 [`apply_language`] 를 부른다.
pub fn follow_in_native_ui<R: Runtime>(app: &AppHandle<R>, i18n: Arc<I18n>, events: Subscription) {
    let ui = AppUi(app.clone());
    tauri::async_runtime::spawn(follow(i18n, events, move |l| apply_language(&ui, l)));
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
    use semver::Version;

    use super::*;

    /// 메뉴는 그대로 부르고 창 제목 쓰기만 기록한다. 창 목록은 mock 앱의 것이다.
    struct RecordingUi<R: Runtime> {
        app: AppUi<R>,
        menu_calls: Mutex<usize>,
        titles: Mutex<Vec<(String, String)>>,
    }

    impl<R: Runtime> NativeUi for RecordingUi<R> {
        fn relabel_menu(&self, _l: &Localizer) -> Result<(), String> {
            *self.menu_calls.lock().unwrap() += 1;
            Ok(())
        }

        fn window_labels(&self) -> Vec<String> {
            self.app.window_labels()
        }

        fn set_window_title(&self, label: &str, title: &str) -> Result<(), String> {
            self.titles
                .lock()
                .unwrap()
                .push((label.to_owned(), title.to_owned()));
            Ok(())
        }
    }

    /// 언어를 바꾸면 메뉴 글자와 함께 프로젝트 · 설정 창의 OS 제목이 그 언어로 바뀐다. 모르는 창(label `main`)은 건드리지 않는다.
    #[test]
    fn a_language_change_retitles_the_open_windows() {
        let app = tauri::test::mock_app();
        app.manage(crate::windows::PendingOpens::default());
        crate::windows::open_project(app.handle(), "ab12", &Default::default()).unwrap();
        crate::windows::open_settings(app.handle()).unwrap();
        tauri::WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
            .build()
            .unwrap();
        let ui = RecordingUi {
            app: AppUi(app.handle().clone()),
            menu_calls: Mutex::new(0),
            titles: Mutex::new(Vec::new()),
        };
        let registry = Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots());
        let catalog = Catalog::from_registry(&registry);

        let titles = |locales: &[&str]| {
            ui.titles.lock().unwrap().clear();
            let locales: Vec<String> = locales.iter().map(|&l| l.to_owned()).collect();
            apply_language(&ui, &catalog.localizer(&locales));
            let mut titles = ui.titles.lock().unwrap().clone();
            titles.sort();
            titles
        };
        let pairs = |p: &str, s: &str| {
            vec![
                ("project-ab12".to_owned(), p.to_owned()),
                ("settings".to_owned(), s.to_owned()),
            ]
        };
        assert_eq!(titles(&["ko", "en"]), pairs("프로젝트", "설정"));
        assert_eq!(titles(&["en"]), pairs("Project", "Settings"));
        assert_eq!(*ui.menu_calls.lock().unwrap(), 2);
    }

    /// 언어 설정을 바꾸면(`i18n.setLanguage` 와 같은 함수) `config.changed` 를 받아 새 언어로 한 번 알린다. 언어와 무관한
    /// 설정 변경에는 알리지 않는다.
    #[tokio::test]
    async fn follows_a_language_change_once_and_ignores_other_config_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("settings.toml"), "schema = 1\n").unwrap();
        let bus = EventBus::new(pw_core::bus::DEFAULT_CAPACITY);
        let store = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let registry = Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots());
        let i18n = Arc::new(I18n::new(
            Catalog::from_registry(&registry),
            vec!["en-US".into()],
        ));
        i18n.attach_config(store.clone());

        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let record = Arc::clone(&seen);
        let task = tokio::spawn(follow(Arc::clone(&i18n), subscribe(&bus), move |l| {
            record.lock().unwrap().push(l.text("menu-file"));
        }));

        std::fs::write(
            paths.config.join("settings.toml"),
            "schema = 1\n[diagnostics]\ncrash_reports = true\n",
        )
        .unwrap();
        store.reload().unwrap();
        let i = Arc::clone(&i18n);
        tokio::task::spawn_blocking(move || i.set_language("ko").unwrap())
            .await
            .unwrap();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while seen.lock().unwrap().is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "언어 변경을 받지 못했다"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // 같은 변경으로 두 번 알리지 않는지 잠깐 더 본다.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        task.abort();
        assert_eq!(*seen.lock().unwrap(), ["파일"]);
    }
}
