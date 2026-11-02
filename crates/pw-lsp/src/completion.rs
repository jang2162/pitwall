//! 자동완성 · 시그니처 도움말의 Host 쪽(15-m4-plan.md M4-11): 웹뷰가 문서(`docId` · `version`)와 UTF-16 오프셋으로 묻고, Host 가
//! 요청 길(`LspManager::request` — 결정 M4-D12 의 라우팅 · `staleVersion`)로 서버에 물어 위치를 오프셋으로 바꿔 돌려준다.
//!
//! - `lsp.completion`: `textDocument/completion`. 목록은 서버 순서로 [`ITEM_LIMIT`] 개까지(넘으면 `truncated` — 창의 거르기는 받은
//!   것만 보므로 웹뷰는 그 목록을 「다시 물어야 하는 목록」으로 다룬다). `CompletionList.itemDefaults`(`editRange` ·
//!   `insertTextFormat` · `commitCharacters` · `data`)는 항목마다 채워 보낸다. 항목의 `textEdit`(삽입 · 바꾸기 범위)와
//!   `additionalTextEdits` 는 오프셋으로, 서버가 준 원래 항목(`raw`)은 resolve 에 되돌려 보내려고 함께 싣는다.
//! - `lsp.completion.resolve`: `completionItem/resolve` — 문서 · `additionalTextEdits`(자동 import)를 늦게 받는다.
//! - `lsp.completion.execute`: 받아들인 항목의 `command` 를 `workspace/executeCommand` 로(결정 M4-D15). 보내는 것은 **이 Host 가
//!   같은 문서 · 같은 버전의 완성 답으로 내준 명령**이고 **그 서버가 `executeCommandProvider.commands` 로 알린 것**뿐이다 — 아니면
//!   거절하고 보내지 않는다(메인 판정 2026-11-02). 명령 끝에 서버가 거는 `workspace/applyEdit` 은 M4-12 의 처리기 · 받는 창이 맡는다 —
//!   그 전에는 클라이언트 능력에 `workspace.applyEdit` 이 없어 서버가 보내지 않는다.
//! - `lsp.signatureHelp`: `textDocument/signatureHelp`. 매개변수 이름(문자열 꼴)은 시그니처 글 안의 UTF-16 범위로 바꾼다.
//!
//! 서버가 준 글(문서 · 시그니처)은 [`MARKUP_LIMIT`] 에서 자른다 — 웹뷰는 문자열(또는 HTML 을 끈 Markdown)로만 그린다.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use lsp_types::Range;
use pw_ext::LspFeature;
use pw_rpc::{Ctx, JsonValue, Router, RpcError, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::manager::{DocView, FeatureRequest, LspManager, code};

/// 완성 목록 상한. 서버가 더 주면 서버 순서로 자른다.
pub const ITEM_LIMIT: usize = 200;
/// 서버가 준 글(문서 · 세부) 하나의 상한(UTF-8 바이트). 넘으면 자르고 `…` 를 붙인다.
pub const MARKUP_LIMIT: usize = 32 * 1024;
/// 명령을 기억하는 문서 수(오래된 것부터 잊는다).
const REMEMBERED_DOCS: usize = 64;

// ---- 형 -----------------------------------------------------------------------------------------------------------------------

/// 완성을 연 까닭(LSP `CompletionTriggerKind`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspCompletionTriggerKind {
    /// 사람이 불렀거나(`⌃Space`) 낱말을 쳤다.
    Invoked,
    /// 서버의 트리거 문자를 쳤다.
    Character,
    /// 앞 목록이 `isIncomplete` 였다 — 글자마다 다시 묻는다.
    Incomplete,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionTrigger {
    pub kind: LspCompletionTriggerKind,
    /// `character` 일 때 친 글자.
    #[serde(default)]
    pub character: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionParams {
    pub doc_id: String,
    /// 웹뷰가 위치를 잰 문서 버전.
    pub version: u32,
    /// 커서(UTF-16 오프셋).
    pub offset: u32,
    pub trigger: LspCompletionTrigger,
}

/// 문서 안의 범위(UTF-16 오프셋, 요청을 보낼 때의 문서 기준).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspOffsetRange {
    pub from: u32,
    pub to: u32,
}

/// 편집 하나(오프셋).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspOffsetEdit {
    pub from: u32,
    pub to: u32,
    pub new_text: String,
}

/// 완성 항목의 본 편집: 넣을 글과 삽입 · 바꾸기 범위(`TextEdit` 이면 둘이 같다).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionEdit {
    pub new_text: String,
    pub insert: LspOffsetRange,
    pub replace: LspOffsetRange,
}

/// 서버가 준 글(`MarkupContent` · 문자열).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspMarkup {
    /// Markdown 인가(아니면 그대로의 글).
    pub markdown: bool,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionItem {
    pub label: String,
    /// `labelDetails.detail`(이름 바로 뒤 — 매개변수 등).
    pub label_detail: Option<String>,
    /// `labelDetails.description`(오른쪽 — 모듈 · 타입).
    pub label_description: Option<String>,
    /// `CompletionItemKind`(1 ~ 25).
    pub kind: Option<u32>,
    pub detail: Option<String>,
    pub documentation: Option<LspMarkup>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
    pub preselect: bool,
    pub deprecated: bool,
    /// 본 편집. 없으면 `insertText`(없으면 `label`)를 커서 앞 낱말 자리에 넣는다.
    pub edit: Option<LspCompletionEdit>,
    pub insert_text: Option<String>,
    /// `insertTextFormat = 2` — 넣을 글이 LSP 스니펫이다.
    pub snippet: bool,
    /// `additionalTextEdits`(자동 import 등).
    pub additional_edits: Vec<LspOffsetEdit>,
    pub commit_characters: Option<Vec<String>>,
    /// 받아들인 뒤 부를 명령(`Command` 그대로).
    #[specta(type = Option<JsonValue>)]
    pub command: Option<Value>,
    /// 서버가 준 항목(`itemDefaults.data` 를 채운 것) — `lsp.completion.resolve` 에 그대로 되돌린다.
    #[specta(type = JsonValue)]
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionList {
    pub server: String,
    /// 요청 사이에 문서가 바뀌었다 — 버린다.
    pub stale_version: bool,
    /// 서버가 목록이 모자란다고 했다(글자마다 다시 묻는다).
    pub is_incomplete: bool,
    /// 서버가 [`ITEM_LIMIT`] 보다 많이 주어 잘랐다.
    pub truncated: bool,
    pub items: Vec<LspCompletionItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionResolveParams {
    pub doc_id: String,
    pub version: u32,
    /// `LspCompletionItem.raw`.
    #[specta(type = JsonValue)]
    pub item: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionResolved {
    pub server: String,
    pub stale_version: bool,
    pub item: LspCompletionItem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionExecuteParams {
    pub doc_id: String,
    /// 그 항목을 받은 완성 답의 문서 버전.
    pub version: u32,
    /// 항목의 `command`(`{ title, command, arguments? }`).
    #[specta(type = JsonValue)]
    pub command: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCompletionExecuted {
    pub server: String,
    #[specta(type = JsonValue)]
    pub result: Value,
}

/// 시그니처 도움말을 연 까닭(LSP `SignatureHelpTriggerKind`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspSignatureTriggerKind {
    /// 사람이 불렀다(`⌘P`).
    Invoked,
    /// 트리거 · 다시 트리거 문자를 쳤다.
    Character,
    /// 열린 동안 글이 바뀌었거나 커서가 움직였다.
    ContentChange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspSignatureHelpParams {
    pub doc_id: String,
    pub version: u32,
    pub offset: u32,
    pub trigger_kind: LspSignatureTriggerKind,
    #[serde(default)]
    pub trigger_character: Option<String>,
    /// 이미 보이는 도움말이 있다.
    pub is_retrigger: bool,
    /// 보이는 도움말(앞 답의 `raw`) — 서버가 고른 시그니처를 이어 간다.
    #[serde(default)]
    #[specta(type = Option<JsonValue>)]
    pub active: Option<Value>,
}

/// 매개변수 하나: 시그니처 글 안의 범위(UTF-16).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspSignatureParameter {
    pub from: u32,
    pub to: u32,
    pub documentation: Option<LspMarkup>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspSignature {
    pub label: String,
    pub documentation: Option<LspMarkup>,
    pub parameters: Vec<LspSignatureParameter>,
    /// 지금 인자(시그니처의 것, 없으면 도움말의 것). 범위 밖이면 없다.
    pub active_parameter: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspSignatureHelp {
    pub signatures: Vec<LspSignature>,
    pub active_signature: u32,
    /// 서버의 답 그대로(다시 물을 때 `active` 로 되돌린다).
    #[specta(type = JsonValue)]
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspSignatureHelpAnswer {
    pub server: String,
    pub stale_version: bool,
    /// 도움말. 서버가 없다고 했거나(커서가 호출 밖) 시그니처가 비었으면 없다.
    pub help: Option<LspSignatureHelp>,
}

rpc_method! {
    /// 자동완성(`textDocument/completion`). 위치 · 편집 범위는 UTF-16 오프셋(요청 때 문서 기준), 목록은 서버 순서로 200개까지.
    /// 요청 사이 문서가 바뀌었으면 `staleVersion`. 맡을 서버가 없으면 5001.
    pub LspCompletionGet: "lsp.completion" {
        params: LspCompletionParams,
        result: LspCompletionList,
        tier: Read,
        roles: [Owner],
    }

    /// 완성 항목 하나를 서버에 다시 물어(`completionItem/resolve`) 문서 · 자동 import 편집을 받는다.
    pub LspCompletionResolve: "lsp.completion.resolve" {
        params: LspCompletionResolveParams,
        result: LspCompletionResolved,
        tier: Read,
        roles: [Owner],
    }

    /// 받아들인 완성 항목의 명령을 서버에 보낸다(`workspace/executeCommand` — 결정 M4-D15). 같은 문서 · 버전의 완성 답으로 내준
    /// 명령이고 그 서버가 알린 명령일 때만 — 아니면 permission denied 로 보내지 않는다.
    pub LspCompletionExecute: "lsp.completion.execute" {
        params: LspCompletionExecuteParams,
        result: LspCompletionExecuted,
        tier: Read,
        roles: [Owner],
    }

    /// 시그니처 도움말(`textDocument/signatureHelp`). 매개변수는 시그니처 글 안의 UTF-16 범위.
    pub LspSignatureHelpGet: "lsp.signatureHelp" {
        params: LspSignatureHelpParams,
        result: LspSignatureHelpAnswer,
        tier: Read,
        roles: [Owner],
    }
}

// ---- 변환 ---------------------------------------------------------------------------------------------------------------------

/// `s` 를 `limit` 바이트 안(글자 경계)으로 자른다.
fn clip(mut s: String, limit: usize) -> String {
    if s.len() <= limit {
        return s;
    }
    let mut at = limit;
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    s.truncate(at);
    s.push('…');
    s
}

fn string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// `MarkupContent` · `MarkedString` · 문자열 → 글. 빈 글은 없음.
pub fn markup(v: &Value) -> Option<LspMarkup> {
    let (markdown, value) = match v {
        Value::String(s) => (false, s.clone()),
        Value::Object(o) if o.contains_key("kind") => (
            o.get("kind").and_then(Value::as_str) == Some("markdown"),
            o.get("value").and_then(Value::as_str)?.to_owned(),
        ),
        // 옛 `MarkedString { language, value }`.
        Value::Object(o) => {
            let value = o.get("value").and_then(Value::as_str)?;
            let language = o.get("language").and_then(Value::as_str).unwrap_or("");
            (true, format!("```{language}\n{value}\n```"))
        }
        _ => return None,
    };
    (!value.is_empty()).then(|| LspMarkup {
        markdown,
        value: clip(value, MARKUP_LIMIT),
    })
}

fn range_of(view: &DocView, v: &Value) -> Option<LspOffsetRange> {
    let range: Range = serde_json::from_value(v.clone()).ok()?;
    let from = view.offset(range.start);
    let to = view.offset(range.end);
    Some(LspOffsetRange {
        from: from.min(to),
        to: from.max(to),
    })
}

fn edit_of(view: &DocView, v: &Value) -> Option<LspOffsetEdit> {
    let range = range_of(view, v.get("range")?)?;
    Some(LspOffsetEdit {
        from: range.from,
        to: range.to,
        new_text: string(v.get("newText")?)?,
    })
}

/// 항목에 `itemDefaults` 를 채운다(항목에 없는 칸만 — LSP 3.17).
fn with_defaults(mut item: Value, defaults: Option<&Value>) -> Value {
    let (Some(defaults), Some(obj)) = (defaults, item.as_object_mut()) else {
        return item;
    };
    for key in [
        "commitCharacters",
        "insertTextFormat",
        "insertTextMode",
        "data",
    ] {
        if !obj.contains_key(key)
            && let Some(v) = defaults.get(key)
        {
            obj.insert(key.to_owned(), v.clone());
        }
    }
    if !obj.contains_key("textEdit")
        && let Some(range) = defaults.get("editRange")
    {
        let new_text = obj
            .get("textEditText")
            .or_else(|| obj.get("insertText"))
            .or_else(|| obj.get("label"))
            .cloned()
            .unwrap_or(Value::String(String::new()));
        let edit = if range.get("insert").is_some() {
            json!({ "newText": new_text, "insert": range["insert"], "replace": range["replace"] })
        } else {
            json!({ "newText": new_text, "range": range })
        };
        obj.insert("textEdit".to_owned(), edit);
    }
    item
}

/// 서버의 `CompletionItem`(기본값을 채운 것) → 웹뷰의 항목.
pub fn completion_item(view: &DocView, raw: Value) -> Option<LspCompletionItem> {
    let label = string(raw.get("label")?)?;
    let edit = raw.get("textEdit").and_then(|e| {
        let new_text = string(e.get("newText")?)?;
        if let Some(range) = e.get("range") {
            let range = range_of(view, range)?;
            Some(LspCompletionEdit {
                new_text,
                insert: range,
                replace: range,
            })
        } else {
            Some(LspCompletionEdit {
                new_text,
                insert: range_of(view, e.get("insert")?)?,
                replace: range_of(view, e.get("replace")?)?,
            })
        }
    });
    let details = raw.get("labelDetails");
    let deprecated = raw.get("deprecated").and_then(Value::as_bool) == Some(true)
        || raw
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|t| t.iter().any(|t| t.as_u64() == Some(1)));
    Some(LspCompletionItem {
        label,
        label_detail: details.and_then(|d| d.get("detail")).and_then(string),
        label_description: details.and_then(|d| d.get("description")).and_then(string),
        kind: raw
            .get("kind")
            .and_then(Value::as_u64)
            .and_then(|k| u32::try_from(k).ok()),
        detail: raw
            .get("detail")
            .and_then(string)
            .map(|d| clip(d, MARKUP_LIMIT)),
        documentation: raw.get("documentation").and_then(markup),
        filter_text: raw.get("filterText").and_then(string),
        sort_text: raw.get("sortText").and_then(string),
        preselect: raw.get("preselect").and_then(Value::as_bool) == Some(true),
        deprecated,
        edit,
        insert_text: raw.get("insertText").and_then(string),
        snippet: raw.get("insertTextFormat").and_then(Value::as_u64) == Some(2),
        additional_edits: raw
            .get("additionalTextEdits")
            .and_then(Value::as_array)
            .map(|edits| edits.iter().filter_map(|e| edit_of(view, e)).collect())
            .unwrap_or_default(),
        commit_characters: raw
            .get("commitCharacters")
            .and_then(Value::as_array)
            .map(|c| c.iter().filter_map(string).collect()),
        command: raw
            .get("command")
            .filter(|c| c.get("command").is_some_and(Value::is_string))
            .cloned(),
        raw,
    })
}

/// `textDocument/completion` 의 답 → 목록(서버 순서로 [`ITEM_LIMIT`] 까지).
pub fn completion_list(view: &DocView, result: &Value) -> (Vec<LspCompletionItem>, bool, bool) {
    let (items, incomplete, defaults) = match result {
        Value::Array(items) => (items.as_slice(), false, None),
        Value::Object(list) => (
            list.get("items")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice),
            list.get("isIncomplete").and_then(Value::as_bool) == Some(true),
            list.get("itemDefaults"),
        ),
        _ => (&[][..], false, None),
    };
    let truncated = items.len() > ITEM_LIMIT;
    let items = items
        .iter()
        .take(ITEM_LIMIT)
        .filter_map(|item| completion_item(view, with_defaults(item.clone(), defaults)))
        .collect();
    (items, incomplete, truncated)
}

/// `label` 안 UTF-8 바이트 위치 → UTF-16 위치.
fn utf16_at(label: &str, byte: usize) -> u32 {
    u32::try_from(label[..byte].encode_utf16().count()).unwrap_or(u32::MAX)
}

/// 시그니처의 매개변수: 이름(문자열)이면 시그니처 글에서 앞 매개변수 뒤부터 찾고, `[시작, 끝]` 이면 그대로(UTF-16).
fn parameters(label: &str, params: &[Value]) -> Vec<LspSignatureParameter> {
    let len16 = u32::try_from(label.encode_utf16().count()).unwrap_or(u32::MAX);
    let mut after = 0usize;
    params
        .iter()
        .map(|p| {
            let documentation = p.get("documentation").and_then(markup);
            let (from, to) = match p.get("label") {
                Some(Value::String(name)) if !name.is_empty() => {
                    label[after..].find(name.as_str()).map_or((0, 0), |at| {
                        let start = after + at;
                        after = start + name.len();
                        (utf16_at(label, start), utf16_at(label, after))
                    })
                }
                Some(Value::Array(pair)) => {
                    let n = |i: usize| {
                        pair.get(i)
                            .and_then(Value::as_u64)
                            .and_then(|v| u32::try_from(v).ok())
                            .unwrap_or(0)
                            .min(len16)
                    };
                    (n(0).min(n(1)), n(0).max(n(1)))
                }
                _ => (0, 0),
            };
            LspSignatureParameter {
                from,
                to,
                documentation,
            }
        })
        .collect()
}

/// `textDocument/signatureHelp` 의 답 → 도움말. 시그니처가 없으면 없음.
pub fn signature_help(result: &Value) -> Option<LspSignatureHelp> {
    let signatures: Vec<LspSignature> = result
        .get("signatures")?
        .as_array()?
        .iter()
        .filter_map(|s| {
            let label = clip(string(s.get("label")?)?, MARKUP_LIMIT);
            let parameters = s
                .get("parameters")
                .and_then(Value::as_array)
                .map(|p| parameters(&label, p))
                .unwrap_or_default();
            let active = s
                .get("activeParameter")
                .or_else(|| result.get("activeParameter"))
                .and_then(Value::as_u64)
                .and_then(|a| u32::try_from(a).ok())
                .filter(|&a| (a as usize) < parameters.len());
            Some(LspSignature {
                documentation: s.get("documentation").and_then(markup),
                label,
                parameters,
                active_parameter: active,
            })
        })
        .collect();
    if signatures.is_empty() {
        return None;
    }
    let active_signature = result
        .get("activeSignature")
        .and_then(Value::as_u64)
        .and_then(|a| u32::try_from(a).ok())
        .filter(|&a| (a as usize) < signatures.len())
        .unwrap_or(0);
    Some(LspSignatureHelp {
        signatures,
        active_signature,
        raw: result.clone(),
    })
}

// ---- 명령 기억 ----------------------------------------------------------------------------------------------------------------

/// 문서마다 마지막 완성 답의 버전과 그 답으로 내준 명령들.
#[derive(Default)]
struct Offered {
    docs: HashMap<String, (u32, Vec<Value>)>,
    order: VecDeque<String>,
}

impl Offered {
    fn remember(&mut self, doc_id: &str, version: u32, commands: Vec<Value>, replace: bool) {
        if !self.docs.contains_key(doc_id) {
            self.order.push_back(doc_id.to_owned());
            while self.order.len() > REMEMBERED_DOCS {
                if let Some(old) = self.order.pop_front() {
                    self.docs.remove(&old);
                }
            }
        }
        let entry = self
            .docs
            .entry(doc_id.to_owned())
            .or_insert_with(|| (version, Vec::new()));
        if replace || entry.0 != version {
            *entry = (version, commands);
        } else {
            entry.1.extend(commands);
        }
    }

    fn offered(&self, doc_id: &str, version: u32, command: &Value) -> bool {
        self.docs
            .get(doc_id)
            .is_some_and(|(v, commands)| *v == version && commands.contains(command))
    }
}

fn denied(message: &str) -> RpcError {
    RpcError::new(rpc_code::PERMISSION_DENIED, message.to_owned())
}

// ---- 처리기 ---------------------------------------------------------------------------------------------------------------------

async fn complete(
    m: &LspManager,
    offered: &Mutex<Offered>,
    ctx: &Ctx,
    p: LspCompletionParams,
) -> Result<LspCompletionList, RpcError> {
    let answer = m
        .request(
            ctx,
            FeatureRequest {
                doc_id: &p.doc_id,
                feature: LspFeature::Completion,
                method: "textDocument/completion",
                expected_version: Some(p.version),
                timeout: None,
            },
            |view| {
                let mut context = json!({ "triggerKind": match p.trigger.kind {
                    LspCompletionTriggerKind::Invoked => 1,
                    LspCompletionTriggerKind::Character => 2,
                    LspCompletionTriggerKind::Incomplete => 3,
                }});
                if p.trigger.kind == LspCompletionTriggerKind::Character
                    && let Some(c) = &p.trigger.character
                {
                    context["triggerCharacter"] = json!(c);
                }
                Ok(json!({
                    "textDocument": view.text_document(),
                    "position": view.position(p.offset)?,
                    "context": context,
                }))
            },
        )
        .await?;
    let (items, is_incomplete, truncated) = completion_list(&answer.doc, &answer.result);
    if !answer.stale_version {
        let commands = items.iter().filter_map(|i| i.command.clone()).collect();
        offered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remember(&p.doc_id, answer.doc.version, commands, true);
    }
    Ok(LspCompletionList {
        server: answer.server,
        stale_version: answer.stale_version,
        is_incomplete,
        truncated,
        items,
    })
}

async fn resolve(
    m: &LspManager,
    offered: &Mutex<Offered>,
    ctx: &Ctx,
    p: LspCompletionResolveParams,
) -> Result<LspCompletionResolved, RpcError> {
    if !p.item.is_object() {
        return Err(RpcError::invalid_params("item must be a completion item"));
    }
    let item = p.item.clone();
    let answer = m
        .request(
            ctx,
            FeatureRequest {
                doc_id: &p.doc_id,
                feature: LspFeature::Completion,
                method: "completionItem/resolve",
                expected_version: Some(p.version),
                timeout: None,
            },
            move |_| Ok(item.clone()),
        )
        .await?;
    // 서버가 빈 답을 주면 원래 항목 그대로다.
    let raw = if answer.result.is_object() {
        answer.result
    } else {
        p.item
    };
    let item = completion_item(&answer.doc, raw)
        .ok_or_else(|| RpcError::new(code::SERVER_ERROR, "the server resolved a malformed item"))?;
    if !answer.stale_version
        && let Some(command) = &item.command
    {
        offered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remember(&p.doc_id, answer.doc.version, vec![command.clone()], false);
    }
    Ok(LspCompletionResolved {
        server: answer.server,
        stale_version: answer.stale_version,
        item,
    })
}

async fn execute(
    m: &LspManager,
    offered: &Mutex<Offered>,
    ctx: &Ctx,
    p: LspCompletionExecuteParams,
) -> Result<LspCompletionExecuted, RpcError> {
    let Some(name) = p.command.get("command").and_then(Value::as_str) else {
        return Err(RpcError::invalid_params("command must have a command name"));
    };
    if !offered
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .offered(&p.doc_id, p.version, &p.command)
    {
        return Err(denied(
            "this command was not offered by a completion of this document version",
        ));
    }
    // 그 서버가 알린 명령인가 — 요청 길과 같은 라우팅으로 고른 서버의 `executeCommandProvider`(없는 문서 · 다른 창은 not found).
    let routes = m.feature_routes(
        ctx.session(),
        &p.doc_id,
        &[(LspFeature::Completion, "workspace/executeCommand")],
    )?;
    let Some((_, _, options)) = routes.first() else {
        return Err(denied(
            "no running language server runs commands for this document",
        ));
    };
    let known = options
        .get("commands")
        .and_then(Value::as_array)
        .is_some_and(|c| c.iter().any(|c| c.as_str() == Some(name)));
    if !known {
        return Err(denied("the language server does not offer this command"));
    }
    let mut body = json!({ "command": name });
    if let Some(arguments) = p.command.get("arguments") {
        body["arguments"] = arguments.clone();
    }
    let answer = m
        .request(
            ctx,
            FeatureRequest {
                doc_id: &p.doc_id,
                feature: LspFeature::Completion,
                method: "workspace/executeCommand",
                expected_version: None,
                timeout: None,
            },
            move |_| Ok(body.clone()),
        )
        .await?;
    Ok(LspCompletionExecuted {
        server: answer.server,
        result: answer.result,
    })
}

async fn signature(
    m: &LspManager,
    ctx: &Ctx,
    p: LspSignatureHelpParams,
) -> Result<LspSignatureHelpAnswer, RpcError> {
    let answer = m
        .request(
            ctx,
            FeatureRequest {
                doc_id: &p.doc_id,
                feature: LspFeature::SignatureHelp,
                method: "textDocument/signatureHelp",
                expected_version: Some(p.version),
                timeout: None,
            },
            |view| {
                let mut context = json!({
                    "triggerKind": match p.trigger_kind {
                        LspSignatureTriggerKind::Invoked => 1,
                        LspSignatureTriggerKind::Character => 2,
                        LspSignatureTriggerKind::ContentChange => 3,
                    },
                    "isRetrigger": p.is_retrigger,
                });
                if let Some(c) = &p.trigger_character {
                    context["triggerCharacter"] = json!(c);
                }
                if let Some(active) = &p.active {
                    context["activeSignatureHelp"] = active.clone();
                }
                Ok(json!({
                    "textDocument": view.text_document(),
                    "position": view.position(p.offset)?,
                    "context": context,
                }))
            },
        )
        .await?;
    Ok(LspSignatureHelpAnswer {
        server: answer.server,
        stale_version: answer.stale_version,
        help: signature_help(&answer.result),
    })
}

/// 핸들러를 붙인다(`crate::register` 가 부른다).
pub fn register(router: &mut Router, manager: &LspManager) {
    let offered = Arc::new(Mutex::new(Offered::default()));
    {
        let (m, offered) = (manager.clone(), Arc::clone(&offered));
        router.unary(LspCompletionGet, move |ctx, p: LspCompletionParams| {
            let (m, offered) = (m.clone(), Arc::clone(&offered));
            async move { complete(&m, &offered, &ctx, p).await }
        });
    }
    {
        let (m, offered) = (manager.clone(), Arc::clone(&offered));
        router.unary(
            LspCompletionResolve,
            move |ctx, p: LspCompletionResolveParams| {
                let (m, offered) = (m.clone(), Arc::clone(&offered));
                async move { resolve(&m, &offered, &ctx, p).await }
            },
        );
    }
    {
        let (m, offered) = (manager.clone(), Arc::clone(&offered));
        router.unary(
            LspCompletionExecute,
            move |ctx, p: LspCompletionExecuteParams| {
                let (m, offered) = (m.clone(), Arc::clone(&offered));
                async move { execute(&m, &offered, &ctx, p).await }
            },
        );
    }
    let m = manager.clone();
    router.unary(
        LspSignatureHelpGet,
        move |ctx, p: LspSignatureHelpParams| {
            let m = m.clone();
            async move { signature(&m, &ctx, p).await }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_takes_markup_content_marked_strings_and_plain_strings() {
        assert_eq!(
            markup(&json!({ "kind": "markdown", "value": "**a**" })),
            Some(LspMarkup {
                markdown: true,
                value: "**a**".into()
            })
        );
        assert_eq!(
            markup(&json!("plain <b>")),
            Some(LspMarkup {
                markdown: false,
                value: "plain <b>".into()
            })
        );
        assert_eq!(
            markup(&json!({ "language": "rust", "value": "fn a()" })).map(|m| m.value),
            Some("```rust\nfn a()\n```".into())
        );
        assert_eq!(markup(&json!({ "kind": "plaintext", "value": "" })), None);
        let long = markup(&json!("가".repeat(MARKUP_LIMIT))).unwrap().value;
        assert!(long.len() <= MARKUP_LIMIT + '…'.len_utf8());
        assert!(long.ends_with('…'));
    }

    #[test]
    fn parameters_find_names_in_order_and_take_utf16_offsets() {
        // 이름이 같은 글자가 앞에 나와도 앞 매개변수 뒤부터 찾는다. 한글은 UTF-16 한 칸이다.
        let label = "fn 넓이(w: u32, h: u32, w2: u32)";
        let p = parameters(
            label,
            &[
                json!({ "label": "w: u32" }),
                json!({ "label": "h: u32" }),
                json!({ "label": [22, 29] }),
            ],
        );
        let slice = |p: &LspSignatureParameter| {
            label
                .encode_utf16()
                .skip(p.from as usize)
                .take((p.to - p.from) as usize)
                .collect::<Vec<u16>>()
        };
        assert_eq!(String::from_utf16(&slice(&p[0])).unwrap(), "w: u32");
        assert_eq!(String::from_utf16(&slice(&p[1])).unwrap(), "h: u32");
        assert_eq!(String::from_utf16(&slice(&p[2])).unwrap(), "w2: u32");
    }

    #[test]
    fn signature_help_clamps_the_active_signature_and_parameter() {
        let help = signature_help(&json!({
            "signatures": [
                { "label": "f(a, b)", "parameters": [{ "label": "a" }, { "label": "b" }] },
                { "label": "f(a)", "parameters": [{ "label": "a" }], "activeParameter": 0 },
            ],
            "activeSignature": 7,
            "activeParameter": 1,
        }))
        .unwrap();
        assert_eq!(help.active_signature, 0);
        assert_eq!(help.signatures[0].active_parameter, Some(1));
        assert_eq!(help.signatures[1].active_parameter, Some(0));
        let out = signature_help(&json!({
            "signatures": [{ "label": "f(a)", "parameters": [{ "label": "a" }] }],
            "activeParameter": 3,
        }))
        .unwrap();
        assert_eq!(out.signatures[0].active_parameter, None);
        assert_eq!(signature_help(&json!({ "signatures": [] })), None);
        assert_eq!(signature_help(&Value::Null), None);
    }

    #[test]
    fn item_defaults_fill_only_missing_fields() {
        let defaults = json!({
            "editRange": { "insert": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 3 } },
                           "replace": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 5 } } },
            "insertTextFormat": 2,
            "data": { "d": 1 },
        });
        let filled = with_defaults(
            json!({ "label": "x", "insertText": "x($1)" }),
            Some(&defaults),
        );
        assert_eq!(filled["textEdit"]["newText"], "x($1)");
        assert_eq!(filled["textEdit"]["replace"]["end"]["character"], 5);
        assert_eq!(filled["insertTextFormat"], 2);
        assert_eq!(filled["data"], json!({ "d": 1 }));
        let own = with_defaults(
            json!({ "label": "y", "insertTextFormat": 1, "data": 9 }),
            Some(&defaults),
        );
        assert_eq!(own["insertTextFormat"], 1);
        assert_eq!(own["data"], 9);
    }

    #[test]
    fn offered_commands_are_kept_per_document_and_version() {
        let mut o = Offered::default();
        let c = json!({ "title": "t", "command": "x" });
        o.remember("d1", 3, vec![c.clone()], true);
        assert!(o.offered("d1", 3, &c));
        assert!(!o.offered("d1", 4, &c));
        assert!(!o.offered("d2", 3, &c));
        o.remember("d1", 5, vec![], true);
        assert!(!o.offered("d1", 3, &c));
        for i in 0..=REMEMBERED_DOCS {
            o.remember(&format!("n{i}"), 1, vec![c.clone()], true);
        }
        assert!(!o.offered("d1", 5, &c), "the oldest document is forgotten");
    }
}
