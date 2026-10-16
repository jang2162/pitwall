//! 앱 쪽 로그 · 진단 연결. 본체는 `crates/pw-diag`.

use std::sync::Arc;

use pw_core::config::{AppPaths, ConfigStore};
use pw_diag::{Diagnostics, LogConfig};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_clipboard_manager::ClipboardExt;

/// `$STATE/logs` 에 로그를 켜고 패닉 훅을 건다. 로그 디렉터리를 만들지 못해도 앱은 뜬다 — 파일 로그 없이 stderr 에만 알린다.
pub fn init(paths: &AppPaths) -> Arc<Diagnostics> {
    let dir = paths.state.join("logs");
    let mut config = LogConfig::new(&dir).level_from_env();
    config.stderr = cfg!(debug_assertions);
    if let Err(e) = pw_diag::init(&config) {
        eprintln!("로그를 켜지 못했다({}): {e}", dir.display());
    }
    pw_diag::install_panic_hook();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        "pitwall starting"
    );
    Arc::new(Diagnostics::new(dir, env!("CARGO_PKG_VERSION")))
}

/// 크래시 리포트를 켜는 전역 설정 키(`settings.toml` 의 `[diagnostics] crash_reports`). 내장 기본값은 false
/// (`crates/pw-core/src/config/source.rs` 의 `BUILTIN_SETTINGS`).
pub const CRASH_REPORTS_KEY: &str = "diagnostics.crash_reports";

/// 크래시 리포트를 켰는가. opt-in(D22)이라 확실하지 않으면 꺼짐이다 — 키가 없거나 bool 이 아니면 false, 마지막 읽기가
/// 실패해 이전 설정으로 버티는 중이어도(설정 파일이 깨졌다) false.
pub fn crash_reports_enabled(global: &ConfigStore) -> bool {
    global.fault().is_none()
        && global
            .snapshot()
            .get("settings", CRASH_REPORTS_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

/// 메뉴의 「진단 정보 복사」. 가린 진단 텍스트를 클립보드에 쓴다.
pub fn copy_to_clipboard<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let diagnostics = app.state::<Arc<Diagnostics>>();
    let text = diagnostics.collect();
    app.clipboard()
        .write_text(text)
        .map_err(|e| e.to_string())?;
    tracing::info!("copied diagnostics to the clipboard");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pw_core::bus::DEFAULT_CAPACITY;
    use pw_core::config::ConfigSource;
    use pw_core::{EventBus, Scope};

    use super::*;

    fn store_with(settings: Option<&str>) -> (tempfile::TempDir, ConfigStore) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        if let Some(text) = settings {
            fs::create_dir_all(&paths.config).unwrap();
            fs::write(paths.config.join("settings.toml"), text).unwrap();
        }
        let store = ConfigStore::open(
            ConfigSource::global(&paths),
            EventBus::new(DEFAULT_CAPACITY),
            Scope::Host,
        );
        (tmp, store)
    }

    #[test]
    fn crash_reports_default_to_off() {
        let (_tmp, store) = store_with(None);
        assert!(!crash_reports_enabled(&store));
    }

    #[test]
    fn crash_reports_follow_the_global_setting() {
        let (_tmp, store) = store_with(Some("schema = 1\n[diagnostics]\ncrash_reports = true\n"));
        assert!(crash_reports_enabled(&store));
        let (_tmp, store) =
            store_with(Some("schema = 1\n[diagnostics]\ncrash_reports = \"yes\"\n"));
        assert!(!crash_reports_enabled(&store), "bool 이 아니면 꺼짐");
    }

    #[test]
    fn a_broken_settings_file_turns_crash_reports_off() {
        let (tmp, store) = store_with(Some("schema = 1\n[diagnostics]\ncrash_reports = true\n"));
        assert!(crash_reports_enabled(&store));
        // 다시 읽기가 실패하면 스토어는 이전 값(true)을 유지한다. 그래도 꺼짐으로 본다.
        fs::write(
            AppPaths::under(tmp.path()).config.join("settings.toml"),
            "schema = 1\n[diagnostics\n",
        )
        .unwrap();
        assert!(store.reload().is_err());
        assert!(!crash_reports_enabled(&store));
        // 처음부터 깨져 있으면 내장 기본값(false)으로 뜬다.
        let (_tmp, store) = store_with(Some("crash_reports = = true"));
        assert!(!crash_reports_enabled(&store));
    }
}
