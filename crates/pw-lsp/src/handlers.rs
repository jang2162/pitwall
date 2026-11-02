//! 서버 → 클라이언트 요청 처리기의 등록부(15-m4-plan.md §2 「공유 부품의 주인」 — 주인 M4-05).
//!
//! 인스턴스를 띄울 때 [`server_handlers`] 가 그 인스턴스의 처리기 표를 만든다. **기능 행은 아래 표에 한 줄씩 더한다**
//! (`workspace/applyEdit` 은 M4-12, `window/showDocument` 는 M4-08) — 처리기를 달면서 그 요청의 클라이언트 능력도 켠다
//! (`crate::capabilities` — 알리지 않은 요청은 서버가 보내지 않는다). 처리기가 없는 요청의 기본 처리는 `crate::server` 머리 주석.

use std::path::PathBuf;
use std::sync::Arc;

use pw_core::bus::EventBus;
use pw_core::registry::Registry;
use pw_lsp_servers::ServerConfig;
use serde_json::Value;

use crate::server::Handlers;

/// 처리기가 받는 인스턴스 문맥.
#[derive(Clone)]
pub struct HandlerContext {
    pub project_id: String,
    pub worktree: Option<String>,
    pub server: String,
    /// 작업 공간 루트(실제 경로).
    pub root: PathBuf,
    /// 띄울 때의 합친 설정(M4-02). 설정이 바뀌면 인스턴스는 `restartNeeded` 이고, 다시 띄울 때 새 값이 든다.
    pub config: Arc<ServerConfig>,
    /// 처리기가 웹뷰에 이벤트를 낼 버스(`window/showDocument` — M4-08). 관리자가 붙기 전에 만든 문맥이면 없다.
    pub bus: Option<EventBus>,
    /// 프로젝트 레지스트리(처리기가 신뢰를 다시 본다 — M4-08). 없으면 신뢰를 요구하는 처리기는 거절한다.
    pub projects: Option<Registry>,
}

/// 인스턴스 하나의 처리기 표.
pub(crate) fn server_handlers(cx: &HandlerContext) -> Handlers {
    let mut handlers = Handlers::new();
    configuration(&mut handlers, cx);
    // 기능 행이 여기에 한 줄씩: `workspace/applyEdit`(M4-12), `window/showDocument`(M4-08).
    crate::edit::apply_edit(&mut handlers, cx);
    crate::navigation::show_document(&mut handlers, cx);
    handlers
}

/// `workspace/configuration` — M4-02 의 합친 설정(`ServerConfig::settings`)에서 답한다.
fn configuration(handlers: &mut Handlers, cx: &HandlerContext) {
    let config = Arc::clone(&cx.config);
    handlers.on("workspace/configuration", move |params: Value| {
        let config = Arc::clone(&config);
        async move { Ok(configuration_answer(&config.settings, &params)) }
    });
}

/// `items` 마다 하나: `section` 이 없으면 설정 전체, 있으면 그 점 경로(`rust-analyzer.cargo`)의 값, 없으면 `null`(「설정 없음」 —
/// 서버가 기본값을 쓴다). `scopeUri` 는 보지 않는다 — 설정은 인스턴스(작업 공간 루트)마다 하나다.
pub fn configuration_answer(settings: &Value, params: &Value) -> Value {
    let items = params
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    Value::Array(
        items
            .iter()
            .map(|item| match item.get("section").and_then(Value::as_str) {
                None | Some("") => settings.clone(),
                Some(section) => section
                    .split('.')
                    .try_fold(settings, |value, key| value.get(key))
                    .cloned()
                    .unwrap_or(Value::Null),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn configuration_answers_by_section_path() {
        let settings = json!({ "rust-analyzer": { "cargo": { "features": "all" } }, "flag": true });
        let params = json!({ "items": [
            { "section": "rust-analyzer" },
            { "section": "rust-analyzer.cargo.features", "scopeUri": "file:///x" },
            { "section": "missing.key" },
            {},
        ]});
        assert_eq!(
            configuration_answer(&settings, &params),
            json!([
                { "cargo": { "features": "all" } },
                "all",
                null,
                settings,
            ])
        );
        assert_eq!(configuration_answer(&settings, &json!({})), json!([]));
    }
}
