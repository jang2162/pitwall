//! 로그 · 진단 RPC 메서드와 그 핸들러.

use std::sync::Arc;

use pw_rpc::{Router, rpc_method};
use serde::{Deserialize, Serialize};

use crate::logging::UI_TARGET;
use crate::report::Diagnostics;

/// 한 번에 받는 UI 로그 항목 수. 넘는 항목은 버리고 버린 수를 경고로 남긴다.
pub const MAX_BATCH_ENTRIES: usize = 200;
/// UI 로그 항목 하나의 메시지 최대 길이(바이트). 넘으면 잘라 쓴다.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum UiLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct UiLogEntry {
    pub level: UiLogLevel,
    pub message: String,
    /// 어디서 남겼는지(모듈 이름, `window.error` 등). 경로나 사용자 입력을 넣지 않는다.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct UiLogBatch {
    pub entries: Vec<UiLogEntry>,
    /// 보내기 전에 웹뷰 쪽 대기열이 넘쳐 버린 항목 수.
    #[serde(default)]
    pub dropped: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsText {
    /// 경로 · 비밀 · 홈 디렉터리 이름을 가린 텍스트. 이슈 제보에 그대로 붙여 넣을 수 있다.
    pub text: String,
}

rpc_method! {
    /// UI(웹뷰)가 남긴 로그를 UI 로그 파일(`$STATE/logs/ui.<날짜>.log`)에 쓴다. 웹뷰는 항목을 모아 묶음으로 보낸다.
    /// 한 번에 200개, 메시지 하나에 8 KiB 까지 받고 넘는 것은 버리거나 자른다.
    pub LogWrite: "log.write" {
        params: UiLogBatch,
        result: (),
        tier: Write,
        file_scope: true,
    }

    /// 「진단 정보 복사」의 텍스트: 버전 · OS · 활성 기능 · 크래시 리포트 설정 · 최근 경고와 오류 로그.
    /// 경로 · 비밀 · 홈 디렉터리 이름은 가린다.
    pub DiagCollect: "diag.collect" {
        params: (),
        result: DiagnosticsText,
        tier: Read,
        roles: [Owner],
    }
}

/// 두 메서드의 핸들러를 붙인다.
pub fn register(router: &mut Router, diagnostics: Arc<Diagnostics>) {
    router
        .unary(LogWrite, |_ctx, batch| async move {
            write_ui_batch(batch);
            Ok(())
        })
        .unary(DiagCollect, move |_ctx, ()| {
            let diagnostics = Arc::clone(&diagnostics);
            async move {
                Ok(DiagnosticsText {
                    text: diagnostics.collect(),
                })
            }
        });
}

/// UI 로그 묶음을 [`UI_TARGET`] 이벤트로 낸다. 구독자가 UI 로그 파일로 보낸다.
pub fn write_ui_batch(batch: UiLogBatch) {
    let over = batch.entries.len().saturating_sub(MAX_BATCH_ENTRIES);
    for entry in batch.entries.into_iter().take(MAX_BATCH_ENTRIES) {
        // 줄바꿈을 이스케이프한다. 메시지 안의 `\n<시각> ERROR …` 가 파일에서 별개 항목으로 읽히면(진단 텍스트의
        // 「최근 오류」 등) 웹뷰가 코어 로그 항목을 위조할 수 있다.
        let message = escape_newlines(clip(&entry.message, MAX_MESSAGE_BYTES));
        let source = entry.source.as_deref().map(|s| clip(s, 128));
        match entry.level {
            UiLogLevel::Debug => tracing::debug!(target: UI_TARGET, source, "{message}"),
            UiLogLevel::Info => tracing::info!(target: UI_TARGET, source, "{message}"),
            UiLogLevel::Warn => tracing::warn!(target: UI_TARGET, source, "{message}"),
            UiLogLevel::Error => tracing::error!(target: UI_TARGET, source, "{message}"),
        }
    }
    let dropped = u64::from(batch.dropped) + over as u64;
    if dropped > 0 {
        tracing::warn!(target: UI_TARGET, dropped, "dropped UI log entries (queue or batch limit)");
    }
}

fn escape_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains(['\n', '\r']) {
        text.replace('\r', "\\r").replace('\n', "\\n").into()
    } else {
        text.into()
    }
}

fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pw_rpc::{Caller, EventHub, Outbound, Role, Session};
    use serde_json::{Value, json};

    use super::*;
    use crate::logging::{LogConfig, subscriber};
    use crate::rolling::tests::FakeClock;
    use crate::test_dir::TestDir;

    fn entry(level: UiLogLevel, message: &str) -> UiLogEntry {
        UiLogEntry {
            level,
            message: message.into(),
            source: Some("test".into()),
        }
    }

    /// 실제 RPC 세션으로 `log.write` 를 불러 UI 로그 파일에 닿는지 본다.
    #[tokio::test]
    async fn log_write_reaches_the_ui_log_file() {
        let dir = TestDir::new();
        let mut config = LogConfig::new(dir.path());
        config.clock = FakeClock::at("2026-10-16");
        let _guard = tracing::subscriber::set_default(subscriber(&config).unwrap());

        let mut router = Router::new();
        register(&mut router, Arc::new(Diagnostics::new(dir.path(), "0.0.0")));
        let (session, mut rx) =
            Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
        session.handle_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": "log.write",
            "params": {"entries": [
                {"level": "error", "message": "uncaught TypeError", "source": "window.error"},
                {"level": "info", "message": "mounted"}
            ], "dropped": 3}
        }));
        let reply = rx.recv().await.unwrap();
        let reply: Value = match reply {
            Outbound::Response(_) => serde_json::to_value(reply).unwrap(),
            other => panic!("expected a response, got {other:?}"),
        };
        assert_eq!(reply["result"], Value::Null, "{reply}");

        let ui = fs::read_to_string(dir.path().join("ui.2026-10-16.log")).unwrap();
        assert!(
            ui.contains("ERROR uncaught TypeError source=\"window.error\""),
            "{ui}"
        );
        assert!(ui.contains("INFO mounted"), "{ui}");
        assert!(
            ui.contains("WARN dropped UI log entries (queue or batch limit) dropped=3"),
            "{ui}"
        );
        let core = fs::read_to_string(dir.path().join("core.2026-10-16.log")).unwrap();
        assert!(!core.contains("uncaught TypeError"), "{core}");
    }

    #[test]
    fn oversized_batches_and_messages_are_capped() {
        let dir = TestDir::new();
        let mut config = LogConfig::new(dir.path());
        config.clock = FakeClock::at("2026-10-16");
        tracing::subscriber::with_default(subscriber(&config).unwrap(), || {
            let mut entries = vec![entry(UiLogLevel::Warn, &"가".repeat(MAX_MESSAGE_BYTES))];
            entries.extend(
                (0..MAX_BATCH_ENTRIES + 4).map(|i| entry(UiLogLevel::Info, &format!("n{i}"))),
            );
            write_ui_batch(UiLogBatch {
                entries,
                dropped: 0,
            });
        });
        let ui = fs::read_to_string(dir.path().join("ui.2026-10-16.log")).unwrap();
        let long = ui.lines().next().unwrap();
        assert!(long.len() < MAX_MESSAGE_BYTES + 200, "{}", long.len());
        assert!(ui.contains(&format!("n{}", MAX_BATCH_ENTRIES - 2)));
        assert!(
            !ui.contains(&format!("n{}", MAX_BATCH_ENTRIES - 1)),
            "201번째 항목부터 버린다"
        );
        assert!(ui.contains("dropped=5"), "{ui}");
    }

    #[test]
    fn newlines_in_ui_messages_cannot_forge_entries() {
        let dir = TestDir::new();
        let mut config = LogConfig::new(dir.path());
        config.clock = FakeClock::at("2026-10-16");
        tracing::subscriber::with_default(subscriber(&config).unwrap(), || {
            write_ui_batch(UiLogBatch {
                entries: vec![entry(
                    UiLogLevel::Info,
                    "ok\n2026-10-16T00:00:00.000001Z ERROR pw_core: forged\r\nend",
                )],
                dropped: 0,
            });
        });
        let ui = fs::read_to_string(dir.path().join("ui.2026-10-16.log")).unwrap();
        assert_eq!(ui.lines().count(), 1, "{ui}");
        assert!(
            ui.contains(r"ok\n2026-10-16T00:00:00.000001Z ERROR pw_core: forged\r\nend"),
            "{ui}"
        );
    }

    #[tokio::test]
    async fn diag_collect_returns_masked_text() {
        let dir = TestDir::new();
        let diag = Diagnostics::new(dir.path(), "9.9.9").with_home(Some("/Users/alice".into()));
        let mut router = Router::new();
        register(&mut router, Arc::new(diag));
        let (session, mut rx) =
            Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
        session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": "diag.collect"}));
        let reply = serde_json::to_value(rx.recv().await.unwrap()).unwrap();
        let text = reply["result"]["text"].as_str().unwrap();
        assert!(text.contains("version: 9.9.9"), "{text}");
    }
}
