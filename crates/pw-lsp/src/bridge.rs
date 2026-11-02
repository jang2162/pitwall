//! 웹뷰 LSP 다리(15-m4-plan.md M4-07)의 Host 쪽: 문서의 기능마다 그 기능을 맡을 서버와 그 서버의 능력을 웹뷰에 준다
//! (`lsp.document.capabilities`). 웹뷰의 `EditorFeatureContext.lsp` 가 이것을 들고, 기능 행은 그 칸으로 기능을 켜고 끄거나
//! 트리거 문자(완성 · 시그니처) 같은 옵션을 읽는다.
//!
//! 고르는 규칙은 요청 길과 같다([`LspManager::feature_routes`] — 결정 M4-D12). 진단은 넣지 않는다 — 진단은 한 서버가 아니라 모든
//! 서버에서 모은다(M4-06 이 Host 에서 한다). 서버가 아직 뜨지 않았으면(`starting`) 칸이 비어 있다 — 웹뷰는 `lsp.status` 이벤트마다
//! 다시 묻는다. 동적 등록 · 해제도 그 이벤트를 낸다(관리자의 `CapabilitiesChanged`).

use std::collections::BTreeMap;

use pw_ext::LspFeature;
use pw_rpc::{JsonValue, Router, RpcError, SessionId, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manager::LspManager;
use crate::methods::LspDocumentRef;

/// 기능 하나의 능력 옵션이 이 크기(직렬화한 JSON 바이트)를 넘으면 옵션을 비우고 `optionsTruncated` 를 단다. 서버가 보낸 값을
/// 그대로 웹뷰에 옮기므로 서버 하나가 창의 RPC 를 크게 만들지 못하게 한다(완성의 트리거 문자 · 코드 액션 종류 목록은 1 KiB 안이다).
pub const OPTIONS_LIMIT: usize = 16 * 1024;

/// 칸에 넣는 기능과 그 기능을 맡을 서버를 고를 때 보는 메서드.
pub const FEATURE_METHODS: [(LspFeature, &str); 11] = [
    (LspFeature::Hover, "textDocument/hover"),
    (LspFeature::Definition, "textDocument/definition"),
    (LspFeature::Implementation, "textDocument/implementation"),
    (LspFeature::References, "textDocument/references"),
    (LspFeature::DocumentSymbol, "textDocument/documentSymbol"),
    (LspFeature::WorkspaceSymbol, "workspace/symbol"),
    (LspFeature::Completion, "textDocument/completion"),
    (LspFeature::SignatureHelp, "textDocument/signatureHelp"),
    (LspFeature::Rename, "textDocument/rename"),
    (LspFeature::CodeAction, "textDocument/codeAction"),
    (LspFeature::Formatting, "textDocument/formatting"),
];

/// 기능 하나를 맡을 서버와 그 능력.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspFeatureCapability {
    /// 맡을 서버 id.
    pub server: String,
    /// 서버가 알린 능력: 그 메서드의 동적 등록 옵션, 없으면 정적 능력 칸 그대로(`true` 또는 옵션 객체 — `completionProvider` 의
    /// `triggerCharacters` 등). [`OPTIONS_LIMIT`] 를 넘으면 `{}`.
    #[specta(type = JsonValue)]
    pub options: Value,
    /// 옵션이 [`OPTIONS_LIMIT`] 를 넘어 비웠다.
    pub options_truncated: bool,
}

/// `lsp.document.capabilities` 의 결과.
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentCapabilities {
    /// 맡을 서버가 있는 기능만(진단은 넣지 않는다). 키는 `LspFeature` 이름(`hover` · `completion` …).
    pub features: BTreeMap<LspFeature, LspFeatureCapability>,
}

rpc_method! {
    /// 문서의 기능마다 그 기능을 맡을 서버와 그 서버가 알린 능력(웹뷰의 기능 칸). 고르는 규칙은 기능 요청과 같다(능력이 있고
    /// 끄지 않은, 떠서 그 문서를 받은 첫 서버 — 결정 M4-D12). 진단은 넣지 않는다. 서버를 띄우지 않는다 — 아직 뜨지 않았으면 빈
    /// 칸이고 `lsp.status` 이벤트 뒤 다시 묻는다. 없는 문서 · 다른 연결의 문서는 not found.
    pub LspDocumentCapabilitiesGet: "lsp.document.capabilities" {
        params: LspDocumentRef,
        result: LspDocumentCapabilities,
        tier: Read,
        roles: [Owner],
    }
}

/// 옵션을 칸에 넣는다: `null` 은 없는 것과 같아 넣지 않고, 상한을 넘으면 비운다.
fn capability(server: String, options: Value) -> LspFeatureCapability {
    let size = serde_json::to_vec(&options).map_or(usize::MAX, |v| v.len());
    let truncated = size > OPTIONS_LIMIT;
    LspFeatureCapability {
        server,
        options: if truncated {
            Value::Object(serde_json::Map::new())
        } else {
            options
        },
        options_truncated: truncated,
    }
}

/// 문서의 능력 칸.
pub fn document_capabilities(
    manager: &LspManager,
    session: SessionId,
    doc_id: &str,
) -> Result<LspDocumentCapabilities, RpcError> {
    let routes = manager.feature_routes(session, doc_id, &FEATURE_METHODS)?;
    Ok(LspDocumentCapabilities {
        features: routes
            .into_iter()
            .map(|(feature, server, options)| (feature, capability(server, options)))
            .collect(),
    })
}

/// 핸들러를 붙인다(`crate::register` 가 부른다).
pub fn register(router: &mut Router, manager: &LspManager) {
    let m = manager.clone();
    router.unary(
        LspDocumentCapabilitiesGet,
        move |ctx, params: LspDocumentRef| {
            let m = m.clone();
            async move { document_capabilities(&m, ctx.session(), &params.doc_id) }
        },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn options_over_the_limit_are_emptied() {
        let small = capability("s".into(), json!({ "triggerCharacters": ["."] }));
        assert!(!small.options_truncated);
        assert_eq!(small.options, json!({ "triggerCharacters": ["."] }));
        let big = capability(
            "s".into(),
            json!({ "triggerCharacters": ["x".repeat(OPTIONS_LIMIT)] }),
        );
        assert!(big.options_truncated);
        assert_eq!(big.options, json!({}));
    }
}
