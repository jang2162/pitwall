//! 포맷의 Host 쪽(15-m4-plan.md M4-13): `lsp.format` — 문서 전체(`textDocument/formatting`) 또는 선택 범위
//! (`textDocument/rangeFormatting`)의 `TextEdit[]` 을 서버에 묻고 검사해 웹뷰에 준다.
//!
//! **쓰지 않는다.** 포맷은 결정 M4-D14 의 「디스크에 바로」 대상이 아니다 — 웹뷰가 편집 적용기의 단일 문서 함수
//! (`apps/desktop/src/lsp/workspace-edit.ts` 의 `applyDocumentEdits`)로 문서에만 넣고(한 undo), 저장은 `⌘S` 다(D34). 포맷 시점은
//! `⌘⌥L` 하나다(결정 M4-D23 — 저장할 때 · 입력 중 포맷은 없다).
//!
//! **맡을 서버**는 결정 M4-D12 의 라우팅(요청 길의 `route` 하나 — 능력이 있고 정의 · 설정에서 `formatting` 을 끄지 않은 첫 서버,
//! Python 은 ruff — 결정 M4-D3)이다. 범위를 주면 그 서버가 `documentRangeFormattingProvider` 일 때만 범위로 묻고, 아니면 문서
//! 전체로 묻는다(다른 서버로 가지 않는다). 맡을 서버가 없으면 5001.
//!
//! **옵션**은 웹뷰가 준 `tabSize` · `insertSpaces` 둘만 보낸다(`trimTrailingWhitespace` 등은 보내지 않는다 — 계획 M4-13 행).
//!
//! **시한** [`FORMAT_TIMEOUT`](10 s): 넘으면 서버에 `$/cancelRequest` 를 보내고 5003 에 `timedOut: true` 를 단다 — 웹뷰는 문서를
//! 그대로 두고 알린다.
//!
//! **거절**([`code::FORMAT_REFUSED`] — 아무것도 넣지 않는다): 편집 수가 [`MAX_FORMAT_EDITS`] 를 넘거나 넣을 글의 합이
//! [`MAX_FORMAT_TEXT`] 를 넘음, 문서 밖 범위(요청을 보낼 때의 문서 기준 — 마지막 줄 뒤의 줄, 시작이 끝보다 뒤), 서로 겹치는 편집,
//! 꼴이 틀린 답, 대리쌍 가운데(한 글자의 두 UTF-16 단위 사이)를 가리키는 위치(`malformed`). 줄 끝을 넘는 글자는 문서 밖이 아니다 —
//! LSP 명세대로 줄 끝으로 맞춘다(웹뷰의 `lspOffset` 도 같다 — 같은 것은 이 줄 끝 맞춤뿐이다. 대리쌍 가운데는 `DocText::offset` 이
//! 글자 앞으로 당기지만 `lspOffset` 은 그 가운데에 넣으므로 거절해 둘이 다른 자리를 보지 않게 한다). 문서 끝 바로 뒤 줄의 0 글자
//! (`{ line: 줄 수, character: 0 }`)는 문서 끝으로 받는다 — 문서 전체를 바꾸는 편집의 끝을 그렇게 주는 서버가 있다. 낡은 답
//! (`stale_version`)은 검사하지 않고 빈 편집이다 — 웹뷰가 넣지 않는다.
//!
//! **진행 중의 답**(`not_ready`, M4-13 재리뷰 N1): LSP 의 포맷 답에는 어느 글 기준인지가 없어 버전으로는 못 잡는 낡음이 있다 —
//! rust-analyzer 는 작업 공간을 읽는 동안(`$/progress` 「Roots Scanned」) 받은 `didChange` 를 분석에 넣지 않은 채 옛 글로 포맷한다.
//! 서버 정의의 `format_unreliable_while_progress`(진행 제목, 정확히 같은 글)가 그런 진행을 적고, 요청을 보내기 전이나 답을 받은 뒤에
//! 그 인스턴스에 그 제목의 진행이 열려 있으면 답을 버린다(검사하지 않고 빈 편집 — 웹뷰는 「언어 서버가 아직 준비 중」). 보낼 때 열려
//! 있다가 답 전에 닫힌 것은 앞쪽이, 보낸 뒤에 열린 것은 뒤쪽이 잡는다 — rust-analyzer 는 그 진행의 begin 을 읽기를 시작하는 처리에서
//! 보내므로 그 사이에 받은 포맷의 답보다 늘 먼저 온다. 남는 틈: 진행 알림은 이벤트 채널을 거쳐 들어오므로(`open_progress_titles`)
//! 답 바로 앞의 begin 이 아직 안 들어왔을 수 있고, 보낸 뒤 열렸다 답 전에 닫힌 진행은 못 본다. 다른 진행(Indexing · cargo check
//! 등)은 포맷을 막지 않는다 — 그때의 답은 받은 편집을 반영한 글 기준이다.

use std::time::Duration;

use lsp_types::Position;
use pw_ext::LspFeature;
use pw_rpc::{Ctx, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::completion::LspOffsetRange;
use crate::document::DocText;
use crate::edit::{LspRange, LspTextEdit};
use crate::manager::{
    FeatureAnswer, FeatureRequest, InstanceKey, LspManager, code as manager_code,
};
use crate::server::RequestError;

/// 이 행의 오류 코드(`pw-lsp` 5001~ 의 칸 — 행마다 겹치지 않게 513x 를 쓴다).
pub mod code {
    /// 서버의 포맷 결과를 받지 않는다(전체 거절 — 문서는 그대로). `data`: `{ reason, server }` — `reason` 은 `tooMany` ·
    /// `tooLarge` · `outside` · `overlap` · `malformed`.
    pub const FORMAT_REFUSED: i32 = 5130;
}

/// 포맷 요청의 시한(계획 M4-13 행).
pub const FORMAT_TIMEOUT: Duration = Duration::from_secs(10);
/// 포맷 결과의 편집 수 상한. 편집 등급(5 MiB) 문서를 줄마다 고쳐도 이 안이다.
pub const MAX_FORMAT_EDITS: usize = 100_000;
/// 포맷 결과가 넣는 글(`newText`)의 합의 상한(UTF-8 바이트). 편집 등급 문서(≤ 5 MiB)를 통째로 바꾸는 답도 이 안이다.
pub const MAX_FORMAT_TEXT: usize = 16 * 1024 * 1024;

/// 서버에 보내는 포맷 옵션(`FormattingOptions` 의 두 칸만).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspFormatOptions {
    pub tab_size: u32,
    pub insert_spaces: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspFormatParams {
    pub doc_id: String,
    /// 웹뷰가 범위 · 옵션을 잰 문서 버전.
    pub version: u32,
    /// 선택 범위. 없으면 문서 전체.
    #[serde(default)]
    pub range: Option<LspOffsetRange>,
    pub options: LspFormatOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspFormatted {
    /// 포맷한 서버 id.
    pub server: String,
    /// 범위 포맷(`textDocument/rangeFormatting`)으로 물었다.
    pub range: bool,
    /// 요청 사이에 문서가 바뀌었다 — 웹뷰는 넣지 않는다.
    pub stale_version: bool,
    /// 보낼 때나 답할 때 서버가 「포맷 답을 믿지 않을 진행」(정의의 `format_unreliable_while_progress`)을 열고 있었다 — 옛 글 기준일
    /// 수 있어 웹뷰는 넣지 않고 「언어 서버가 아직 준비 중」 을 알린다. 편집은 빈 목록.
    pub not_ready: bool,
    /// 요청을 보낼 때의 문서 기준 편집(LSP 위치 그대로, 겹치지 않음 · 문서 안). 서버가 `null` 이거나 낡은 답(`stale_version` ·
    /// `not_ready`)이면 빈 목록.
    pub edits: Vec<LspTextEdit>,
}

rpc_method! {
    /// 문서(또는 선택 범위)의 포맷 편집. 쓰지 않는다 — 웹뷰가 문서에만 넣는다(저장은 `⌘S`). 맡을 서버가 없으면 5001, 서버 오류는
    /// 5002, 시한(10 s) · 서버 끝남은 5003(시한이면 `data.timedOut`), 받을 수 없는 결과(상한 · 문서 밖 · 겹침)는 5130.
    pub LspFormat: "lsp.format" {
        params: LspFormatParams,
        result: LspFormatted,
        tier: Read,
        roles: [Owner],
    }
}

fn refused(reason: &str, server: &str, message: impl Into<String>) -> RpcError {
    RpcError::new(code::FORMAT_REFUSED, message.into())
        .with_data(json!({ "reason": reason, "server": server }))
}

/// 서버의 `TextEdit[] | null` 을 검사한다(머리 주석의 「거절」). `text` 는 요청을 보낼 때의 문서다.
pub fn checked_edits(
    result: &Value,
    text: &DocText,
    server: &str,
) -> Result<Vec<LspTextEdit>, RpcError> {
    if result.is_null() {
        return Ok(Vec::new());
    }
    let list = result.as_array().ok_or_else(|| {
        refused(
            "malformed",
            server,
            "the formatter did not send a list of edits",
        )
    })?;
    if list.len() > MAX_FORMAT_EDITS {
        return Err(refused(
            "tooMany",
            server,
            format!(
                "the formatter sent {} edits (at most {MAX_FORMAT_EDITS})",
                list.len()
            ),
        ));
    }
    let mut edits = Vec::with_capacity(list.len());
    let mut size = 0usize;
    for item in list {
        let edit: LspTextEdit = serde_json::from_value(item.clone())
            .map_err(|_| refused("malformed", server, "the formatter sent a malformed edit"))?;
        size = size.saturating_add(edit.new_text.len());
        if size > MAX_FORMAT_TEXT {
            return Err(refused(
                "tooLarge",
                server,
                format!("the formatter's edits insert more than {MAX_FORMAT_TEXT} bytes"),
            ));
        }
        edits.push(edit);
    }
    // 문서 안인지 보고 오프셋으로 바꿔 겹침을 본다.
    let lines = text.line_count();
    let inside = |p: &crate::edit::LspPosition| {
        let line = p.line as usize;
        line < lines || (line == lines && p.character == 0)
    };
    let offset = |p: &crate::edit::LspPosition| text.offset(Position::new(p.line, p.character));
    // 줄 안의 글자가 대리쌍 가운데(두 UTF-16 단위 사이)인가. `DocText::offset` 은 그 자리를 글자 앞으로 당기지만 웹뷰의 적용기
    // (`lspOffset`)는 그 가운데에 넣어 외톨이 대리쌍을 만들므로, 받지 않는다(M4-13 리뷰 M2).
    let splits_pair = |p: &crate::edit::LspPosition| {
        if p.line as usize >= lines {
            return false;
        }
        let start = text.offset(Position::new(p.line, 0));
        let end = text.offset(Position::new(p.line, u32::MAX));
        let want = start + p.character as usize;
        want < end && text.byte_of(want).is_none()
    };
    let mut spans = Vec::with_capacity(edits.len());
    for (i, edit) in edits.iter().enumerate() {
        let LspRange { start, end } = edit.range;
        if !inside(&start) || !inside(&end) {
            return Err(refused(
                "outside",
                server,
                format!(
                    "the formatter edits line {} of a document with {lines} lines",
                    start.line.max(end.line)
                ),
            ));
        }
        if splits_pair(&start) || splits_pair(&end) {
            return Err(refused(
                "malformed",
                server,
                "the formatter sent a position inside a character (between the halves of a UTF-16 surrogate pair)",
            ));
        }
        let (from, to) = (offset(&start), offset(&end));
        if (start.line, start.character) > (end.line, end.character) || from > to {
            return Err(refused(
                "outside",
                server,
                "the formatter sent an edit that ends before it starts",
            ));
        }
        spans.push((from, to, i));
    }
    // 같은 자리의 넣기는 서버가 준 차례다(LSP 규칙) — 겹침은 앞 편집의 끝보다 앞에서 시작하는 것.
    spans.sort_unstable_by_key(|&(from, _, i)| (from, i));
    if spans.windows(2).any(|w| w[1].0 < w[0].1) {
        return Err(refused(
            "overlap",
            server,
            "the formatter sent overlapping edits",
        ));
    }
    Ok(edits)
}

/// 시한을 넘긴 5003 이면 `data.timedOut` 을 단다.
fn mark_timeout(e: RpcError) -> RpcError {
    let timed_out = e.code == manager_code::SERVER_UNAVAILABLE
        && e.data
            .as_ref()
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str)
            == Some(RequestError::Timeout.to_string().as_str());
    if !timed_out {
        return e;
    }
    let mut data = e.data.clone().unwrap_or_else(|| json!({}));
    data["timedOut"] = json!(true);
    RpcError::new(e.code, e.message.clone()).with_data(data)
}

/// `lsp.format`.
pub async fn format(
    m: &LspManager,
    ctx: &Ctx,
    p: LspFormatParams,
) -> Result<LspFormatted, RpcError> {
    let params = |view: &crate::manager::DocView, range: Option<LspOffsetRange>| {
        let mut body = json!({
            "textDocument": view.text_document(),
            "options": { "tabSize": p.options.tab_size, "insertSpaces": p.options.insert_spaces },
        });
        if let Some(r) = range {
            body["range"] = json!({ "start": view.position(r.from)?, "end": view.position(r.to)? });
        }
        Ok(body)
    };
    let request = |method| FeatureRequest {
        doc_id: &p.doc_id,
        feature: LspFeature::Formatting,
        method,
        expected_version: Some(p.version),
        timeout: Some(FORMAT_TIMEOUT),
    };
    // 문서 전체를 맡을 서버(결정 M4-D12). 범위를 주었고 그 서버가 범위 포맷을 하면 그 서버에만 범위로 묻는다.
    let routed = m
        .feature_route_keys(
            ctx.session(),
            &p.doc_id,
            &[(LspFeature::Formatting, "textDocument/formatting")],
            None,
        )?
        .into_iter()
        .next()
        .map(|(_, key, _)| key);
    let range_server = match (p.range.filter(|r| r.from < r.to), &routed) {
        (Some(range), Some(key)) => m
            .feature_route_keys(
                ctx.session(),
                &p.doc_id,
                &[(LspFeature::Formatting, "textDocument/rangeFormatting")],
                Some(&key.server),
            )
            .is_ok_and(|r| !r.is_empty())
            .then(|| (key.server.clone(), range)),
        _ => None,
    };
    // 보내기 전에 열린 진행(머리 주석 「진행 중의 답」).
    let before = routed
        .as_ref()
        .map(|key| (key.clone(), m.open_progress_titles(key)));
    let answer: FeatureAnswer = match &range_server {
        Some((server, range)) => {
            let range = *range;
            m.request_on(
                ctx,
                server,
                request("textDocument/rangeFormatting"),
                |view| params(view, Some(range)),
            )
            .await
        }
        None => {
            m.request(ctx, request("textDocument/formatting"), |view| {
                params(view, None)
            })
            .await
        }
    }
    .map_err(mark_timeout)?;
    let not_ready = unreliable_progress(m, &answer.key, before);
    // 낡은 답 · 진행 중의 답은 웹뷰가 넣지 않는다 — 검사하지 않고 빈 편집으로 답해, 「문서가 바뀌었다」 · 「준비 중」 대신 「거절」 이
    // 뜨지 않게 한다(M4-13 리뷰 L3).
    let edits = if answer.stale_version || not_ready {
        Vec::new()
    } else {
        checked_edits(&answer.result, &answer.doc.text, &answer.server)?
    };
    Ok(LspFormatted {
        server: answer.server,
        range: range_server.is_some(),
        stale_version: answer.stale_version,
        not_ready,
        edits,
    })
}

/// 답한 인스턴스 `key` 에 보내기 전(`before` — 그때 맡을 인스턴스와 그 열린 진행) 또는 지금 「포맷 답을 믿지 않을 진행」 이
/// 열려 있는가. 제목은 정확히 같아야 한다.
fn unreliable_progress(
    m: &LspManager,
    key: &InstanceKey,
    before: Option<(InstanceKey, Vec<String>)>,
) -> bool {
    let Some(config) = m.instance_config(key) else {
        return false;
    };
    let unreliable = &config.format_unreliable_while_progress;
    if unreliable.is_empty() {
        return false;
    }
    let before = before
        .filter(|(at, _)| at.server == key.server && at.root == key.root)
        .map(|(_, titles)| titles)
        .unwrap_or_default();
    before
        .iter()
        .chain(&m.open_progress_titles(key))
        .any(|title| unreliable.contains(title))
}

/// 핸들러를 붙인다(`crate::register` 가 부른다).
pub fn register(router: &mut Router, manager: &LspManager) {
    let m = manager.clone();
    router.unary(LspFormat, move |ctx, p| {
        let m = m.clone();
        async move { format(&m, &ctx, p).await }
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn edit(l1: u32, c1: u32, l2: u32, c2: u32, text: &str) -> Value {
        json!({ "range": { "start": { "line": l1, "character": c1 }, "end": { "line": l2, "character": c2 } }, "newText": text })
    }

    fn reason(e: &RpcError) -> String {
        assert_eq!(e.code, code::FORMAT_REFUSED, "{e:?}");
        e.data.as_ref().unwrap()["reason"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn edits_inside_the_document_pass_and_null_is_no_edit() {
        // "fn a() {}\n" 는 두 줄(둘째는 빈 줄)이다.
        let text = DocText::new("fn a() {}\n");
        assert_eq!(checked_edits(&Value::Null, &text, "s").unwrap(), vec![]);
        let edits = checked_edits(
            &json!([
                edit(0, 2, 0, 3, " "),
                edit(0, 99, 1, 0, "\n"),
                edit(2, 0, 2, 0, "")
            ]),
            &text,
            "s",
        )
        .unwrap();
        assert_eq!(edits.len(), 3);
        // 같은 자리의 넣기 둘은 겹침이 아니다.
        checked_edits(
            &json!([edit(0, 0, 0, 0, "a"), edit(0, 0, 0, 0, "b")]),
            &text,
            "s",
        )
        .unwrap();
    }

    #[test]
    fn edits_outside_the_document_or_overlapping_are_refused_whole() {
        let text = DocText::new("fn a() {}\n");
        assert_eq!(
            reason(&checked_edits(&json!([edit(3, 0, 3, 0, "x")]), &text, "s").unwrap_err()),
            "outside"
        );
        assert_eq!(
            reason(&checked_edits(&json!([edit(2, 1, 2, 1, "x")]), &text, "s").unwrap_err()),
            "outside"
        );
        assert_eq!(
            reason(&checked_edits(&json!([edit(0, 5, 0, 2, "x")]), &text, "s").unwrap_err()),
            "outside"
        );
        assert_eq!(
            reason(
                &checked_edits(
                    &json!([edit(0, 0, 0, 4, "x"), edit(0, 3, 0, 5, "y")]),
                    &text,
                    "s"
                )
                .unwrap_err()
            ),
            "overlap"
        );
        assert_eq!(
            reason(&checked_edits(&json!({ "edits": [] }), &text, "s").unwrap_err()),
            "malformed"
        );
        assert_eq!(
            reason(&checked_edits(&json!([{ "newText": "x" }]), &text, "s").unwrap_err()),
            "malformed"
        );
    }

    #[test]
    fn a_position_inside_a_surrogate_pair_is_refused_whole() {
        // `😀` 은 UTF-16 두 단위(글자 1 ~ 3)다 — 2 는 그 가운데.
        let text = DocText::new("a😀b\n");
        checked_edits(
            &json!([edit(0, 1, 0, 3, "x"), edit(0, 4, 0, 4, "y")]),
            &text,
            "s",
        )
        .unwrap();
        for bad in [
            edit(0, 2, 0, 2, "x"),
            edit(0, 0, 0, 2, "x"),
            edit(0, 2, 0, 4, "x"),
        ] {
            assert_eq!(
                reason(
                    &checked_edits(&json!([edit(0, 0, 0, 0, "ok"), bad]), &text, "s").unwrap_err()
                ),
                "malformed"
            );
        }
        // 줄 끝을 넘는 글자는 대리쌍 판정과 무관하게 줄 끝이다.
        checked_edits(&json!([edit(0, 99, 0, 99, "x")]), &text, "s").unwrap();
    }

    #[test]
    fn edits_over_the_limits_are_refused_whole() {
        let text = DocText::new("x\n");
        let many: Vec<Value> = (0..=MAX_FORMAT_EDITS)
            .map(|_| edit(0, 0, 0, 0, ""))
            .collect();
        assert_eq!(
            reason(&checked_edits(&Value::Array(many), &text, "s").unwrap_err()),
            "tooMany"
        );
        let half = "a".repeat(MAX_FORMAT_TEXT / 2 + 1);
        assert_eq!(
            reason(
                &checked_edits(
                    &json!([edit(0, 0, 0, 0, &half), edit(0, 1, 0, 1, &half)]),
                    &text,
                    "s"
                )
                .unwrap_err()
            ),
            "tooLarge"
        );
    }

    #[test]
    fn a_timeout_is_marked() {
        let timeout = RpcError::new(manager_code::SERVER_UNAVAILABLE, "x")
            .with_data(json!({ "server": "s", "reason": RequestError::Timeout.to_string() }));
        assert_eq!(mark_timeout(timeout).data.unwrap()["timedOut"], json!(true));
        let gone = RpcError::new(manager_code::SERVER_UNAVAILABLE, "x")
            .with_data(json!({ "server": "s", "reason": "the language server is gone: x" }));
        assert!(mark_timeout(gone).data.unwrap().get("timedOut").is_none());
    }
}
