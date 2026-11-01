//! 웹뷰의 「복사」 버튼(설정 창의 「Orca 에서 Pitwall 열기」 등)이 클립보드에 글자를 쓴다. 웹뷰의 `navigator.clipboard` 는
//! 보안 문맥 · 사용자 동작 조건이 플랫폼 웹뷰마다 달라, 메뉴의 「진단 정보 복사」(diag.rs)와 같은 클립보드 플러그인을 쓴다.
//! 프로젝트 창에도 열려 있으므로(app_commands.rs) 한 번에 쓰는 글의 크기에 상한([`MAX_CLIPBOARD_BYTES`])을 둔다.

use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_clipboard_manager::ClipboardExt;

/// 한 번에 클립보드에 쓸 수 있는 글의 UTF-8 바이트 상한. 넘으면 쓰지 않고 [`ClipboardRefused::TooLarge`].
///
/// 가장 큰 글은 결과 그리드의 「다른 형식으로 복사」 다. 그리드는 셀 바이트 합을 `[db] grid_max_bytes`(기본 64 MiB, 설정할 수 있는
/// 최대 256 MiB — `pw_db::settings`)까지 들고, 형식이 글을 불린다: CSV 는 따옴표를 겹쳐 많아야 두 배 남짓, JSON 은 셀마다 열
/// 이름 · 들여쓰기를 되풀이하고 제어 문자를 `\u0001`(여섯 배)로 쓴다. 그 최대의 네 배(1 GiB)면 기본 설정의 그리드 전부는 어느
/// 형식이든(64 MiB × 6 = 384 MiB) 들어가고, 최대 설정에서도 CSV 는 들어간다. 그리드 설정의 범위를 넓히면 이 값도 다시 본다 —
/// 시험(`the_limit_is_four_times_the_largest_grid_byte_limit`)이 잡는다.
pub const MAX_CLIPBOARD_BYTES: usize = 1 << 30;

/// 쓰기를 거절한 까닭 — 웹뷰가 받는 오류(`{ code, … }`). 창은 `code` 로 번역하고(src/db/grid/clipboard.ts), `message` 는
/// 번역하지 않는 쪽(설정 창)이 그대로 보인다.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum ClipboardRefused {
    #[serde(rename_all = "camelCase")]
    TooLarge { max_bytes: usize, message: String },
    /// OS 클립보드가 쓰기를 실패했다.
    Failed { message: String },
}

#[tauri::command]
pub fn write_clipboard<R: Runtime>(
    app: AppHandle<R>,
    text: String,
) -> Result<(), ClipboardRefused> {
    write_within(&app, text, MAX_CLIPBOARD_BYTES)
}

/// `max` 바이트를 넘는 글은 클립보드에 닿기 전에 거절한다.
fn write_within<R: Runtime>(
    app: &AppHandle<R>,
    text: String,
    max: usize,
) -> Result<(), ClipboardRefused> {
    if text.len() > max {
        return Err(ClipboardRefused::TooLarge {
            max_bytes: max,
            message: format!(
                "the text is {} bytes, more than the clipboard limit of {max} bytes",
                text.len()
            ),
        });
    }
    app.clipboard()
        .write_text(text)
        .map_err(|e| ClipboardRefused::Failed {
            message: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use pw_db::settings::{DbSettingKey, validate};
    use serde_json::json;

    use super::*;

    /// 상한을 넘는 글은 클립보드(플러그인)에 닿지 않고 거절된다 — 시험 앱에는 클립보드 플러그인이 없어 닿으면 패닉이다. 오류는
    /// 창이 읽는 꼴(`code` · `maxBytes` · `message`)이다.
    #[test]
    fn text_over_the_limit_is_refused_before_the_clipboard() {
        let app = tauri::test::mock_app();
        let refused = write_within(app.handle(), "abcd".into(), 3).unwrap_err();
        assert_eq!(
            serde_json::to_value(&refused).unwrap(),
            json!({
                "code": "tooLarge",
                "maxBytes": 3,
                "message": "the text is 4 bytes, more than the clipboard limit of 3 bytes",
            })
        );
        // 바이트로 센다: 「가」 는 세 바이트다.
        assert!(matches!(
            write_within(app.handle(), "가a".into(), 3),
            Err(ClipboardRefused::TooLarge { .. })
        ));
    }

    /// 상한은 설정할 수 있는 그리드 바이트 상한 최대(`[db] grid_max_bytes`)의 네 배다.
    #[test]
    fn the_limit_is_four_times_the_largest_grid_byte_limit() {
        let largest = MAX_CLIPBOARD_BYTES / 4;
        assert!(validate(DbSettingKey::GridMaxBytes, &json!(largest)).is_ok());
        assert!(validate(DbSettingKey::GridMaxBytes, &json!(largest + 1)).is_err());
    }

    #[test]
    fn a_failed_write_keeps_the_plugin_message() {
        assert_eq!(
            serde_json::to_value(ClipboardRefused::Failed {
                message: "no clipboard".into()
            })
            .unwrap(),
            json!({ "code": "failed", "message": "no clipboard" })
        );
    }
}
