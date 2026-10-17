//! 앱 쪽 UI 언어 연결. 본체는 `crates/pw-i18n`.
//!
//! 언어 설정(`settings.toml` 의 `ui.language`)이 바뀌면 전역 설정이 다시 읽히며 `config.changed` 가 나간다 — 손으로 고쳤든
//! `i18n.setLanguage` 로 바꿨든 같은 길이다. 웹뷰는 그 이벤트로 `i18n.get` 을 다시 부르고, 네이티브 메뉴는 여기서 받아
//! 글자를 바꾼다(테마 · 키맵의 재시작 없는 반영과 같은 이벤트 경로).

use std::sync::Arc;

use pw_core::config::TOPIC_CHANGED;
use pw_core::{EventBus, Filter, Scope, Subscription};
use pw_ext::Registry;
use pw_i18n::{Catalog, I18n, Localizer};
use tauri::{AppHandle, Runtime};

use crate::menu;

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

/// 언어가 바뀌면 네이티브 메뉴의 글자를 바꾼다.
pub fn follow_in_menu<R: Runtime>(app: &AppHandle<R>, i18n: Arc<I18n>, events: Subscription) {
    let app = app.clone();
    tauri::async_runtime::spawn(follow(i18n, events, move |l| {
        if let Err(error) = menu::relabel(&app, l) {
            tracing::error!(%error, "menu labels not changed to the new language");
        }
    }));
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
    use semver::Version;

    use super::*;

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
