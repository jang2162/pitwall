//! 클라이언트 능력(`initialize` 에 보내는 것)과 서버 능력 조회(정적 + 동적 등록).
//!
//! **클라이언트 능력**(15-m4-plan.md M4-01 ②): 위치 인코딩은 `utf-16` 하나(결정 M4-D10), `window.workDoneProgress`,
//! `workspace.configuration`, `didChangeWatchedFiles` 동적 등록(`RelativePattern` 포함 — M4-05b), `publishDiagnostics` 의
//! `versionSupport` · `relatedInformation`, 그리고 `textDocument` 의 **M4 기능만** — 하지 않는 것(의미 토큰 · 인레이 힌트 · 코드 렌즈 ·
//! 호출/타입 계층 · 같은 심볼 강조 · 연결 편집 · 폴딩 · 입력 중 포맷 등, 15 §3)은 알리지 않는다. 알리면 서버가 그 일을 하느라 시간을
//! 쓰고 우리가 받지 않는 요청을 건다. 서버가 거는 요청 가운데 처리할 행이 아직 없는 것도 알리지 않는다 — `workspace.applyEdit` 은
//! M4-12, `window.showDocument` 는 M4-08 이 처리기를 달면서 여기 함께 켠다(M4-01 리뷰 L2). 기능 행이 자기 기능의 세부(스니펫 · resolve 등)를 바꿀 때는 [`client_capabilities_json`] 의 그
//! 칸만 고치고 이 모듈의 시험(알리지 않는 것 표)을 함께 본다.
//!
//! **서버 능력 조회**: [`supports`] 는 메서드 이름으로 묻는다 — 동적 등록(`client/registerCapability`)이 있으면 그것, 없으면
//! `initialize` 응답의 정적 능력(그 칸이 있고 `false` · `null` 이 아니면). 서버가 보낸 JSON 을 그대로 본다(서버마다 판이 달라
//! `lsp_types::ServerCapabilities` 로 읽히지 않는 칸이 있어도 조회는 된다).

use serde_json::{Value, json};

/// `initialize` 의 `capabilities`.
pub fn client_capabilities_json() -> Value {
    json!({
        "general": {
            "positionEncodings": ["utf-16"],
            "staleRequestSupport": {
                "cancel": true,
                "retryOnContentModified": []
            }
        },
        "window": {
            "workDoneProgress": true
        },
        "workspace": {
            "workspaceEdit": {
                "documentChanges": true,
                "resourceOperations": ["create", "rename", "delete"],
                "failureHandling": "abort"
            },
            "configuration": true,
            "workspaceFolders": true,
            "didChangeConfiguration": { "dynamicRegistration": false },
            "didChangeWatchedFiles": {
                "dynamicRegistration": true,
                "relativePatternSupport": true
            },
            "symbol": {
                "dynamicRegistration": false,
                "symbolKind": { "valueSet": symbol_kinds() }
            }
        },
        "textDocument": {
            "synchronization": {
                "dynamicRegistration": false,
                "didSave": true,
                "willSave": false,
                "willSaveWaitUntil": false
            },
            "publishDiagnostics": {
                "versionSupport": true,
                "relatedInformation": true,
                "codeDescriptionSupport": true,
                "dataSupport": true
            },
            "diagnostic": { "dynamicRegistration": true, "relatedDocumentSupport": false },
            "hover": { "dynamicRegistration": false, "contentFormat": ["markdown", "plaintext"] },
            "completion": {
                "dynamicRegistration": false,
                "contextSupport": true,
                "completionItem": {
                    "snippetSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "insertReplaceSupport": true,
                    "labelDetailsSupport": true,
                    "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] }
                },
                "completionList": { "itemDefaults": ["commitCharacters", "editRange", "insertTextFormat", "data"] }
            },
            "signatureHelp": {
                "dynamicRegistration": false,
                "contextSupport": true,
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true },
                    "activeParameterSupport": true
                }
            },
            "declaration": { "dynamicRegistration": false, "linkSupport": true },
            "definition": { "dynamicRegistration": false, "linkSupport": true },
            "typeDefinition": { "dynamicRegistration": false, "linkSupport": true },
            "implementation": { "dynamicRegistration": false, "linkSupport": true },
            "references": { "dynamicRegistration": false },
            "documentSymbol": {
                "dynamicRegistration": false,
                "hierarchicalDocumentSymbolSupport": true,
                "symbolKind": { "valueSet": symbol_kinds() }
            },
            "codeAction": {
                "dynamicRegistration": false,
                "isPreferredSupport": true,
                "disabledSupport": true,
                "dataSupport": true,
                "resolveSupport": { "properties": ["edit"] },
                "codeActionLiteralSupport": {
                    "codeActionKind": {
                        "valueSet": [
                            "", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                            "refactor.rewrite", "source", "source.organizeImports", "source.fixAll"
                        ]
                    }
                }
            },
            "formatting": { "dynamicRegistration": false },
            "rangeFormatting": { "dynamicRegistration": false },
            "rename": { "dynamicRegistration": false, "prepareSupport": true }
        }
    })
}

/// `SymbolKind` 1 ~ 26 전부.
fn symbol_kinds() -> Vec<u32> {
    (1..=26).collect()
}

/// [`client_capabilities_json`] 를 `lsp-types` 의 형으로.
pub fn client_capabilities() -> lsp_types::ClientCapabilities {
    serde_json::from_value(client_capabilities_json()).unwrap_or_default()
}

/// 메서드 → 그 메서드를 다루는 정적 능력 칸(`ServerCapabilities` 의 키). 문서 동기화 · 알림은 묻지 않는다.
pub const METHOD_CAPABILITIES: [(&str, &str); 19] = [
    ("textDocument/hover", "hoverProvider"),
    ("textDocument/completion", "completionProvider"),
    ("textDocument/signatureHelp", "signatureHelpProvider"),
    ("textDocument/declaration", "declarationProvider"),
    ("textDocument/definition", "definitionProvider"),
    ("textDocument/typeDefinition", "typeDefinitionProvider"),
    ("textDocument/implementation", "implementationProvider"),
    ("textDocument/references", "referencesProvider"),
    ("textDocument/documentSymbol", "documentSymbolProvider"),
    ("textDocument/codeAction", "codeActionProvider"),
    ("textDocument/formatting", "documentFormattingProvider"),
    (
        "textDocument/rangeFormatting",
        "documentRangeFormattingProvider",
    ),
    ("textDocument/rename", "renameProvider"),
    (
        "textDocument/prepareRename",
        "renameProvider.prepareProvider",
    ),
    ("textDocument/diagnostic", "diagnosticProvider"),
    ("workspace/symbol", "workspaceSymbolProvider"),
    ("workspace/executeCommand", "executeCommandProvider"),
    (
        "completionItem/resolve",
        "completionProvider.resolveProvider",
    ),
    ("codeAction/resolve", "codeActionProvider.resolveProvider"),
];

/// 정적 능력 `capabilities` 가 `method` 를 다루는가. 칸의 값이 있고 `false` · `null` 이 아니면 그렇다. 표에 없는 메서드는 `false`.
pub fn static_supports(capabilities: &Value, method: &str) -> bool {
    let Some((_, path)) = METHOD_CAPABILITIES.iter().find(|(m, _)| *m == method) else {
        return false;
    };
    let mut at = capabilities;
    for key in path.split('.') {
        match at.get(key) {
            Some(next) => at = next,
            None => return false,
        }
    }
    !matches!(at, Value::Null | Value::Bool(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_is_the_only_position_encoding() {
        let caps = client_capabilities_json();
        assert_eq!(caps["general"]["positionEncodings"], json!(["utf-16"]));
        assert_eq!(caps["window"]["workDoneProgress"], json!(true));
        assert_eq!(caps["workspace"]["configuration"], json!(true));
        assert_eq!(
            caps["workspace"]["didChangeWatchedFiles"]["dynamicRegistration"],
            json!(true)
        );
        let diagnostics = &caps["textDocument"]["publishDiagnostics"];
        assert_eq!(diagnostics["versionSupport"], json!(true));
        assert_eq!(diagnostics["relatedInformation"], json!(true));
        // lsp-types 의 형으로도 그대로 읽힌다(빠진 칸 없이).
        let typed = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(typed["general"]["positionEncodings"], json!(["utf-16"]));
        assert_eq!(
            typed["textDocument"]["documentSymbol"]["hierarchicalDocumentSymbolSupport"],
            json!(true)
        );
    }

    #[test]
    fn features_outside_m4_are_not_announced() {
        let caps = client_capabilities_json();
        let text_document = caps["textDocument"].as_object().unwrap();
        for absent in [
            "semanticTokens",
            "inlayHint",
            "codeLens",
            "callHierarchy",
            "typeHierarchy",
            "documentHighlight",
            "linkedEditingRange",
            "foldingRange",
            "selectionRange",
            "documentLink",
            "colorProvider",
            "onTypeFormatting",
            "moniker",
            "inlineValue",
        ] {
            assert!(!text_document.contains_key(absent), "{absent}");
        }
        // 처리할 행이 들어오기 전에는 서버의 편집 · 문서 열기 요청을 부르지 않는다(M4-12 · M4-08 이 켠다).
        assert!(caps["workspace"].get("applyEdit").is_none());
        assert!(caps["window"].get("showDocument").is_none());
        let workspace = caps["workspace"].as_object().unwrap();
        for absent in [
            "semanticTokens",
            "inlayHint",
            "codeLens",
            "inlineValue",
            "fileOperations",
        ] {
            assert!(!workspace.contains_key(absent), "{absent}");
        }
        let mut sent: Vec<&str> = text_document.keys().map(String::as_str).collect();
        sent.sort_unstable();
        assert_eq!(
            sent,
            [
                "codeAction",
                "completion",
                "declaration",
                "definition",
                "diagnostic",
                "documentSymbol",
                "formatting",
                "hover",
                "implementation",
                "publishDiagnostics",
                "rangeFormatting",
                "references",
                "rename",
                "signatureHelp",
                "synchronization",
                "typeDefinition",
            ]
        );
    }

    #[test]
    fn static_capabilities_are_read_by_method() {
        let caps = json!({
            "hoverProvider": true,
            "definitionProvider": false,
            "referencesProvider": {},
            "renameProvider": { "prepareProvider": true },
            "completionProvider": { "triggerCharacters": ["."] },
            "documentSymbolProvider": null
        });
        assert!(static_supports(&caps, "textDocument/hover"));
        assert!(!static_supports(&caps, "textDocument/definition"));
        assert!(static_supports(&caps, "textDocument/references"));
        assert!(static_supports(&caps, "textDocument/prepareRename"));
        assert!(static_supports(&caps, "textDocument/completion"));
        assert!(!static_supports(&caps, "completionItem/resolve"));
        assert!(!static_supports(&caps, "textDocument/documentSymbol"));
        assert!(!static_supports(&caps, "textDocument/formatting"));
        assert!(!static_supports(&caps, "textDocument/unknown"));
    }
}
