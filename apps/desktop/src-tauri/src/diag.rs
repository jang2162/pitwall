//! 앱 쪽 로그 · 진단 연결. 본체는 `crates/pw-diag`.

use std::sync::Arc;

use pw_core::config::AppPaths;
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
