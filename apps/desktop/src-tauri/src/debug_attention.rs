//! 디버그 대상이 멈췄을 때의 창 주의 요청(17-m6-plan M6-18 ④). 승인 알림(`approval_notice.rs`)과 같은 길이다 — 웹뷰가 그 창의 디버그
//! 세션이 멈출 때마다(`debug.stopped`) [`debug_stopped_attention`] 을 부르고, 여기서 가른다:
//!
//! - 그 창이 앞에 있으면 아무것도 하지 않는다(멈춘 줄이 에디터에 보인다).
//! - 앞에 없으면 그 창의 주의를 청한다(macOS 는 Dock 아이콘 튀기기 — [`UserAttentionType::Critical`], 앱이 앞에 올 때까지). OS 알림은
//!   내지 않는다 — 스텝마다 멈추므로 쌓인다.

use tauri::{Runtime, UserAttentionType, WebviewWindow};

/// 창이 앞에 없으면 `attention` 을 부른다. 불렀으면 참.
pub fn stopped(in_front: bool, attention: impl FnOnce()) -> bool {
    if in_front {
        return false;
    }
    attention();
    true
}

/// 웹뷰: 이 창의 디버그 세션이 멈췄다(`src/debug/debug-native.ts`).
#[tauri::command]
pub async fn debug_stopped_attention<R: Runtime>(window: WebviewWindow<R>) -> Result<(), ()> {
    let in_front = window.is_focused().unwrap_or(false);
    stopped(in_front, || {
        if let Err(error) = window.request_user_attention(Some(UserAttentionType::Critical)) {
            tracing::warn!(%error, "could not ask for attention for a stopped debug session");
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn only_a_window_not_in_front_asks_for_attention() {
        let asked = Cell::new(0);
        assert!(!stopped(true, || asked.set(asked.get() + 1)));
        assert_eq!(asked.get(), 0);
        assert!(stopped(false, || asked.set(asked.get() + 1)));
        assert_eq!(asked.get(), 1);
    }
}
