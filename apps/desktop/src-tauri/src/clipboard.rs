//! 웹뷰의 「복사」 버튼(설정 창의 「Orca 에서 Pitwall 열기」 등)이 클립보드에 글자를 쓴다. 웹뷰의 `navigator.clipboard` 는
//! 보안 문맥 · 사용자 동작 조건이 플랫폼 웹뷰마다 달라, 메뉴의 「진단 정보 복사」(diag.rs)와 같은 클립보드 플러그인을 쓴다.

use tauri::{AppHandle, Runtime};
use tauri_plugin_clipboard_manager::ClipboardExt;

#[tauri::command]
pub fn write_clipboard<R: Runtime>(app: AppHandle<R>, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}
