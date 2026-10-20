//! 문서 세션 RPC: `syntax.open` · `syntax.edit` · `syntax.highlights` · `syntax.close`. UI 전용이라 MCP 툴이 아니다.
//!
//! 위치는 모두 **UTF-16 코드 단위**다(웹뷰 CodeMirror 의 위치). 텍스트는 에디터가 가진 그대로(CodeMirror 는 줄바꿈을 `\n` 하나로
//! 센다 — `state.doc.toString()`) 보낸다.
//!
//! 순서: 웹뷰의 RPC 프레임은 보낸 순서대로 처리된다는 보장이 없다(데스크톱 IPC 의 `rpc_send` 는 비동기 명령이다). 그래서 편집마다
//! 버전을 붙인다 — 앞당겨 도착한 편집은 잠시 들고 있다가 차례가 오면 적용하고, 강조 결과에는 그것을 계산한 버전이 실린다.
//! `syntax.open` 의 응답을 받은 뒤에 편집을 보낸다.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Syntax;
use crate::token::SyntaxToken;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxOpenParams {
    /// 클라이언트가 정한 문서 id. 같은 id 로 다시 열면 앞 문서를 버리고 새로 연다(어긋났을 때 다시 맞추는 길).
    pub doc_id: String,
    /// Linguist 언어 이름(`lang.detect` 의 결과). 언어 확장의 `linguist` 와 대소문자 없이 맞춘다.
    pub language: String,
    pub text: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxOpenResult {
    /// 쓰는 언어 기여의 id(`[contributes.languages.<id>]`). `null` 이면 그 언어의 문법이 없다 — 문서를 열지 않았다(플레인
    /// 텍스트로 보인다). 열린 문서의 버전은 0 이다.
    pub grammar: Option<String>,
}

/// 바꾸기 하나: `from..to` 를 `insert` 로.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TextChange {
    pub from: u32,
    pub to: u32,
    pub insert: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxEditParams {
    pub doc_id: String,
    /// 이 편집을 적용한 뒤의 버전 = 앞 버전 + 1.
    pub version: u32,
    /// 차례로 적용한다 — 각 바꾸기의 위치는 앞 바꾸기를 적용한 뒤의 텍스트 기준이다. CodeMirror 의 `iterChanges(fromA, toA,
    /// fromB, _, inserted)` 는 `{ from: fromB, to: fromB + (toA - fromA), insert: inserted }` 로 옮긴다.
    pub changes: Vec<TextChange>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TextRange {
    pub from: u32,
    pub to: u32,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxHighlightsParams {
    pub doc_id: String,
    /// 강조할 범위(화면에 보이는 줄들). 문서 끝을 넘으면 끝에서 자른다.
    pub range: TextRange,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct HighlightSpan {
    pub from: u32,
    pub to: u32,
    /// 테마 토큰 이름은 `syntax.<token>` 이다(`docs/design/07-extensibility.md` 「3. 테마」).
    pub token: SyntaxToken,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxHighlightsResult {
    /// 이 구간들을 계산한 텍스트의 버전. 웹뷰의 지금 버전과 다르면 버린다(그 사이 편집이 있었다).
    pub version: u32,
    /// 겹치지 않고 앞에서부터 차례다. 토큰이 없는 곳(평문)은 빠진다.
    pub spans: Vec<HighlightSpan>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxCloseParams {
    pub doc_id: String,
}

rpc_method! {
    /// 문서를 연다(이미 있는 id 면 다시 연다). 문법을 처음 쓰면 여기서 올린다. 문법을 올리지 못하면 internal 오류다(그 문서는
    /// 닫힌다).
    pub SyntaxOpen: "syntax.open" {
        params: SyntaxOpenParams,
        result: SyntaxOpenResult,
        tier: Read,
    }
}

rpc_method! {
    /// 문서에 편집을 적용한다(다시 파싱은 다음 `syntax.highlights` 때 바뀐 곳만 한다). 열지 않은 문서는 not found,
    /// 이미 지난 버전 · 기다리는 편집이 너무 많으면 conflict(`data.version` 에 지금 버전 — 다시 연다), 범위가 텍스트 밖이면
    /// invalid params(아무것도 바꾸지 않는다).
    pub SyntaxEdit: "syntax.edit" {
        params: SyntaxEditParams,
        result: (),
        tier: Read,
    }
}

rpc_method! {
    /// 범위 안의 강조 구간. 열지 않은 문서는 not found, `from > to` 는 invalid params.
    pub SyntaxHighlights: "syntax.highlights" {
        params: SyntaxHighlightsParams,
        result: SyntaxHighlightsResult,
        tier: Read,
    }
}

rpc_method! {
    /// 문서를 닫는다. 없는 id 도 성공이다.
    pub SyntaxClose: "syntax.close" {
        params: SyntaxCloseParams,
        result: (),
        tier: Read,
    }
}

pub fn register(router: &mut Router, syntax: Arc<Syntax>) {
    let s = Arc::clone(&syntax);
    router.unary(SyntaxOpen, move |_ctx, params: SyntaxOpenParams| {
        // 문서를 넣는 일은 지금 하고(뒤따르는 편집이 찾을 수 있게), 문법 올리기만 블로킹 스레드로 보낸다.
        let opened = s.open(params);
        let s = Arc::clone(&s);
        async move {
            let Some(opened) = opened else {
                return Ok(SyntaxOpenResult { grammar: None });
            };
            tokio::task::spawn_blocking(move || s.prepare(opened))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
    let s = Arc::clone(&syntax);
    router.unary(SyntaxEdit, move |_ctx, params: SyntaxEditParams| {
        let result = s.edit(params);
        async move { result }
    });
    let s = Arc::clone(&syntax);
    router.unary(
        SyntaxHighlights,
        move |_ctx, params: SyntaxHighlightsParams| {
            let s = Arc::clone(&s);
            async move {
                tokio::task::spawn_blocking(move || s.highlights(&params))
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
            }
        },
    );
    router.unary(SyntaxClose, move |_ctx, params: SyntaxCloseParams| {
        syntax.close(&params.doc_id);
        async move { Ok(()) }
    });
}
