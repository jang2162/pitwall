//! 편집 기능의 Host 쪽(15-m4-plan.md M4-12): rename(`lsp.rename.prepare` · `lsp.rename`), 코드 액션(`lsp.codeAction.list` ·
//! `lsp.codeAction.resolve`), 서버 명령(`lsp.command.execute`)과 그 처리 중에 서버가 거는 `workspace/applyEdit`.
//!
//! **편집을 쓰는 것은 웹뷰다.** 서버의 `WorkspaceEdit` 은 여기서 경로 · 연산의 목록([`LspWorkspaceEdit`])으로 고르게 바꿔 웹뷰에
//! 주고, 웹뷰의 적용기(`apps/desktop/src/lsp/workspace-edit.ts` — 결정 M4-D14)가 미리 확인 · 미리보기 · 쓰기 · 되돌리기를 한다. 열린
//! 문서(미저장 편집 · undo)와 `fs.changed` 를 아는 쪽이 웹뷰라서다. 위치는 서버가 준 LSP 위치(줄 · UTF-16 글자) 그대로 넘긴다 — 열지
//! 않은 파일의 위치는 디스크 텍스트 기준이라 Host 의 문서 텍스트로 바꿀 수 없고, 웹뷰가 그 파일을 읽은 텍스트로 바꾼다.
//!
//! **거절**(전체): `file:` 이 아닌 URI, 상대 경로, `.git` 안의 경로(파일 하나를 덮는 것으로 훅 · `core.fsmonitor` 를 거쳐 코드
//! 실행까지 간다 — `pw_fs::Fs::write_roots`), 상한([`MAX_EDIT_FILES`] · [`MAX_TEXT_EDITS`])을 넘는 편집. 루트 밖 경로는 웹뷰가 쓰기
//! 전에 `fs.stat` 으로 모두 확인한다(실제 경로 · 연결 범위 판정은 `fs.*` 의 것 하나다).
//!
//! **다른 창이 연 파일**(결정 M4-D14 ①): 같은 프로젝트의 창은 여럿일 수 있고 같은 URI 는 서버에 한 번 열린다 — 서버의 텍스트는
//! 주인 문서의 것이다(M4-05 `Synced`). 편집을 계산한 인스턴스가 어느 파일을 다른 연결(창)의 텍스트로 갖고 있으면 그 파일의
//! [`LspFileChange::Text`] 에 `foreign` 을 단다([`LspManager::foreign_paths`]). 웹뷰는 그런 파일이 끼면 아무것도 쓰지 않는다 — 그
//! 범위는 그 창의 미저장 텍스트 기준이라 디스크에 쓰면 어긋나고, 그 창의 버퍼 아래 디스크를 덮는다.
//!
//! **서버가 거는 편집(결정 M4-D15)**: `workspace/applyEdit` 은 사람이 고른 명령(`lsp.command.execute` — 코드 액션의 `command`,
//! M4-11 의 완성 항목 `command` 도 같은 길)이 **그 인스턴스**(프로젝트 · 서버 · 루트 — 같은 서버 id 의 다른 루트는 다른 인스턴스다)
//! 에서 도는 동안에만 받는다. 그 명령을 부른 창에 이벤트 [`TOPIC_APPLY_EDIT`] 로 넘기고 창의 답(`lsp.applyEdit.answer`)을
//! [`ANSWER_TIMEOUT`] 까지 기다린다. 명령 밖 · 다른 인스턴스 · 명령이 끝난 뒤 · 창이 닫힘 · 시한 넘김 · 같은 명령의 앞 편집이 아직
//! 답을 기다림은 모두 `applied: false` 이고 아무것도 바뀌지 않는다.
//!
//! **목록을 준 서버로만**: 코드 액션의 resolve · 명령은 목록(`lsp.codeAction.list`)을 준 서버(웹뷰가 `server` 로 돌려준다)에만
//! 간다 — 메서드마다 첫 서버를 고르는 라우팅(결정 M4-D12)으로 다른 서버가 그 액션의 `data` · 명령을 받지 않게. 그 서버가 resolve 를
//! 하지 않으면 받은 그대로, 명령을 하지 않으면 5001.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use pw_ext::LspFeature;
use pw_rpc::{
    ConnectionScope, Ctx, EventHub, JsonValue, Router, RpcError, SessionId, code as rpc_code,
    rpc_method,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::diagnostics::DiagnosticStore;
use crate::handlers::HandlerContext;
use crate::manager::{FeatureRequest, InstanceKey, LspManager, code as manager_code};
use crate::message::ResponseError;
use crate::server::Handlers;
use crate::uri::uri_path;

/// 이 행의 오류 코드(`pw-lsp` 5001~ 의 칸 — 행마다 겹치지 않게 512x 를 쓴다).
pub mod code {
    /// 서버의 편집을 받지 않는다(전체 거절). `data`: `{ reason, uri? }` — `reason` 은 `notFile` · `relative` · `git` · `tooLarge` ·
    /// `malformed`.
    pub const EDIT_REFUSED: i32 = 5120;
}

/// 편집 하나가 건드리는 파일(연산 포함) 상한.
pub const MAX_EDIT_FILES: usize = 2_000;
/// 편집 하나의 텍스트 바꾸기 수 상한.
pub const MAX_TEXT_EDITS: usize = 200_000;
/// 코드 액션 요청에 실어 보내는 진단 수 상한 — M4-06 의 파일당 진단 상한(1,000)과 같다.
pub const MAX_CONTEXT_DIAGNOSTICS: usize = 1_000;
/// 코드 액션 요청에 실어 보내는 진단의 직렬화 크기 상한.
pub const MAX_CONTEXT_DIAGNOSTICS_BYTES: usize = 1024 * 1024;
/// 코드 액션 목록의 항목 수 상한(넘는 것은 버린다).
pub const MAX_CODE_ACTIONS: usize = 200;
/// 코드 액션 항목 하나(서버가 준 JSON)의 크기 상한 — 넘으면 그 항목을 버린다(웹뷰가 그대로 돌려보내는 값이다).
pub const MAX_CODE_ACTION_BYTES: usize = 256 * 1024;
/// 서버 명령의 시한. 그 처리 중에 온 편집의 미리보기를 사람이 보는 시간을 담는다.
pub const EXECUTE_TIMEOUT: Duration = Duration::from_secs(300);
/// 창이 `workspace/applyEdit` 에 답하는 시한(미리보기 포함).
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);
/// 서버가 거는 편집을 창에 넘기는 이벤트(프로젝트 범위). 내용은 [`LspApplyEditEvent`].
pub const TOPIC_APPLY_EDIT: &str = "lsp.applyEdit";

// ---- 편집의 꼴 ---------------------------------------------------------------------------------------------------------------

/// LSP 위치(줄 · UTF-16 글자 — 결정 M4-D10). 서버가 준 그대로다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

/// 텍스트 바꾸기 하나(`TextEdit`). 한 파일의 바꾸기들은 모두 같은 텍스트(바꾸기 전) 기준이고 겹치지 않는다(LSP 규칙).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspTextEdit {
    pub range: LspRange,
    pub new_text: String,
}

/// 파일 하나의 바뀜. 목록의 차례대로 적용한다(LSP `documentChanges` 의 차례).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LspFileChange {
    /// 텍스트 바꾸기. `version` 은 서버가 이 편집을 계산한 문서 버전(`TextDocumentEdit.textDocument.version` — 열린 문서면 웹뷰가
    /// 보낸 버전이다). 없으면 서버가 디스크(또는 버전 없이 받은 텍스트)를 기준으로 했다.
    Text {
        path: String,
        version: Option<i32>,
        edits: Vec<LspTextEdit>,
        /// 편집을 계산한 서버가 이 파일을 다른 창(연결)의 문서 텍스트로 갖고 있다 — 웹뷰는 아무것도 쓰지 않는다(머리 주석).
        foreign: bool,
    },
    Create {
        path: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        from: String,
        to: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        path: String,
        recursive: bool,
        ignore_if_not_exists: bool,
    },
}

/// 고르게 바꾼 `WorkspaceEdit`.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspWorkspaceEdit {
    pub changes: Vec<LspFileChange>,
}

fn refused(reason: &str, message: impl Into<String>, uri: Option<&str>) -> RpcError {
    RpcError::new(code::EDIT_REFUSED, message.into())
        .with_data(json!({ "reason": reason, "uri": uri }))
}

/// `uri` → 절대 경로 글. `file:` 이 아니거나 상대거나 `.git` 안이면 거절.
fn edit_path(uri: &str) -> Result<String, RpcError> {
    let path = uri_path(uri).ok_or_else(|| {
        refused(
            "notFile",
            format!("the language server edits `{uri}`, which is not a file"),
            Some(uri),
        )
    })?;
    if !path.is_absolute() {
        return Err(refused(
            "relative",
            format!("`{uri}` is not an absolute path"),
            Some(uri),
        ));
    }
    if inside_git(&path) {
        return Err(refused(
            "git",
            format!("the language server edits `{}` inside .git", path.display()),
            Some(uri),
        ));
    }
    Ok(path.display().to_string())
}

/// 경로의 어느 마디가 `.git` 이다(대소문자 무시 — macOS · Windows 의 파일 시스템은 대개 가리지 않는다).
fn inside_git(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(name) => name.to_string_lossy().eq_ignore_ascii_case(".git"),
        _ => false,
    })
}

fn malformed(what: &str) -> RpcError {
    refused(
        "malformed",
        format!("the language server sent a malformed {what}"),
        None,
    )
}

fn text_edits(edits: &Value) -> Result<Vec<LspTextEdit>, RpcError> {
    let list = edits.as_array().ok_or_else(|| malformed("edit list"))?;
    list.iter()
        .map(|edit| {
            // `AnnotatedTextEdit` 도 같은 칸이다. 스니펫 편집(3.18 `SnippetTextEdit`)은 알리지 않아 오지 않는다.
            let range: LspRange =
                serde_json::from_value(edit.get("range").cloned().unwrap_or_default())
                    .map_err(|_| malformed("text edit range"))?;
            let new_text = edit
                .get("newText")
                .and_then(Value::as_str)
                .ok_or_else(|| malformed("text edit"))?
                .to_owned();
            Ok(LspTextEdit { range, new_text })
        })
        .collect()
}

fn flag(options: Option<&Value>, key: &str) -> bool {
    options
        .and_then(|o| o.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 서버의 `WorkspaceEdit`(JSON) → [`LspWorkspaceEdit`]. `null` 은 빈 편집. `documentChanges` 가 있으면 그것만 본다(LSP 규칙).
pub fn normalize_edit(edit: &Value) -> Result<LspWorkspaceEdit, RpcError> {
    let mut changes = Vec::new();
    if edit.is_null() {
        return Ok(LspWorkspaceEdit::default());
    }
    if let Some(list) = edit.get("documentChanges").filter(|v| !v.is_null()) {
        let list = list
            .as_array()
            .ok_or_else(|| malformed("documentChanges"))?;
        for item in list {
            let options = item.get("options");
            let uri = |key: &str| {
                item.get(key)
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("resource operation"))
                    .and_then(edit_path)
            };
            let change = match item.get("kind").and_then(Value::as_str) {
                Some("create") => LspFileChange::Create {
                    path: uri("uri")?,
                    overwrite: flag(options, "overwrite"),
                    ignore_if_exists: flag(options, "ignoreIfExists"),
                },
                Some("rename") => LspFileChange::Rename {
                    from: uri("oldUri")?,
                    to: uri("newUri")?,
                    overwrite: flag(options, "overwrite"),
                    ignore_if_exists: flag(options, "ignoreIfExists"),
                },
                Some("delete") => LspFileChange::Delete {
                    path: uri("uri")?,
                    recursive: flag(options, "recursive"),
                    ignore_if_not_exists: flag(options, "ignoreIfNotExists"),
                },
                Some(_) => return Err(malformed("resource operation")),
                None => {
                    let doc = item
                        .get("textDocument")
                        .ok_or_else(|| malformed("text document edit"))?;
                    let uri = doc
                        .get("uri")
                        .and_then(Value::as_str)
                        .ok_or_else(|| malformed("text document edit"))?;
                    LspFileChange::Text {
                        path: edit_path(uri)?,
                        version: doc
                            .get("version")
                            .and_then(Value::as_i64)
                            .and_then(|v| i32::try_from(v).ok()),
                        edits: text_edits(item.get("edits").unwrap_or(&Value::Null))?,
                        foreign: false,
                    }
                }
            };
            changes.push(change);
        }
    } else if let Some(map) = edit.get("changes").filter(|v| !v.is_null()) {
        let map = map.as_object().ok_or_else(|| malformed("changes"))?;
        // 차례가 없는 표다 — 경로 순으로 둔다(미리보기 · 시험이 같은 차례를 본다).
        let mut sorted = BTreeMap::new();
        for (uri, edits) in map {
            sorted.insert(edit_path(uri)?, text_edits(edits)?);
        }
        changes.extend(sorted.into_iter().map(|(path, edits)| LspFileChange::Text {
            path,
            version: None,
            edits,
            foreign: false,
        }));
    }
    let text_count: usize = changes
        .iter()
        .map(|c| match c {
            LspFileChange::Text { edits, .. } => edits.len(),
            _ => 0,
        })
        .sum();
    if changes.len() > MAX_EDIT_FILES || text_count > MAX_TEXT_EDITS {
        return Err(refused(
            "tooLarge",
            format!(
                "the edit touches {} files with {text_count} text edits (limits {MAX_EDIT_FILES} · {MAX_TEXT_EDITS})",
                changes.len()
            ),
            None,
        ));
    }
    Ok(LspWorkspaceEdit { changes })
}

/// 인스턴스 `key` 가 다른 창의 텍스트로 가진 파일에 `foreign` 을 단다(머리 주석 「다른 창이 연 파일」).
fn mark_foreign(
    m: &LspManager,
    key: &InstanceKey,
    session: SessionId,
    edit: &mut LspWorkspaceEdit,
) {
    let paths: Vec<PathBuf> = edit
        .changes
        .iter()
        .filter_map(|c| match c {
            LspFileChange::Text { path, .. } => Some(PathBuf::from(path)),
            _ => None,
        })
        .collect();
    let foreign = m.foreign_paths(key, session, paths.iter().map(PathBuf::as_path));
    for change in &mut edit.changes {
        if let LspFileChange::Text {
            path,
            foreign: mark,
            ..
        } = change
        {
            *mark = foreign.contains(Path::new(path.as_str()));
        }
    }
}

// ---- rename ------------------------------------------------------------------------------------------------------------------

/// 문서의 한 자리(웹뷰의 UTF-16 오프셋 — 결정 M4-D10).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspRenamePrepareParams {
    pub doc_id: String,
    /// 웹뷰가 오프셋을 잰 문서 버전.
    pub version: u32,
    pub offset: u32,
}

/// `lsp.rename.prepare` 의 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspRenamePrepared {
    pub server: String,
    /// 묻는 사이 문서가 바뀌었다 — 버린다.
    pub stale_version: bool,
    pub target: LspRenameTarget,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LspRenameTarget {
    /// 바꿀 범위(웹뷰 오프셋)와 서버가 권한 처음 이름.
    Range {
        from: u32,
        to: u32,
        placeholder: Option<String>,
    },
    /// 서버가 범위를 정하지 않는다(`prepareRename` 이 없거나 `defaultBehavior`) — 웹뷰가 커서의 낱말을 쓴다.
    Default,
    /// 이 자리는 이름을 바꿀 수 없다. `message` 는 서버가 준 까닭(있으면).
    Invalid { message: Option<String> },
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspRenameParams {
    pub doc_id: String,
    pub version: u32,
    pub offset: u32,
    pub new_name: String,
}

/// 서버가 계산한 편집(rename · 코드 액션 resolve).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspEditAnswer {
    pub server: String,
    pub stale_version: bool,
    pub edit: LspWorkspaceEdit,
}

// ---- 코드 액션 ---------------------------------------------------------------------------------------------------------------

/// 코드 액션 요청에 실을 진단(부른 쪽이 준 서버의 `Diagnostic` 그대로). `version` 은 그 진단이 맞는 문서 버전 — 요청의 버전과
/// 다르면 버린다(위치가 어긋난 진단은 서버가 다른 수정을 고른다).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspContextDiagnostics {
    pub version: u32,
    #[specta(type = Vec<JsonValue>)]
    pub items: Vec<Value>,
}

/// 코드 액션을 부른 까닭(LSP `CodeActionTriggerKind`).
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
pub enum LspCodeActionTrigger {
    /// 사람이 불렀다(`⌥↩` · 「빠른 수정」).
    #[default]
    Invoked,
    /// 저절로(커서가 멈춘 뒤의 전구).
    Automatic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCodeActionListParams {
    pub doc_id: String,
    pub version: u32,
    /// 선택 범위(웹뷰 오프셋). 커서뿐이면 `from == to`.
    pub from: u32,
    pub to: u32,
    #[serde(default)]
    pub diagnostics: Option<LspContextDiagnostics>,
    /// 이 종류만(`quickfix` · `refactor` · `source` …). 없으면 모두.
    #[serde(default)]
    pub only: Option<Vec<String>>,
    #[serde(default)]
    pub trigger: LspCodeActionTrigger,
}

/// 코드 액션 하나(`CodeAction` 또는 `Command`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCodeActionItem {
    pub title: String,
    pub kind: Option<String>,
    pub is_preferred: bool,
    /// 서버가 지금은 못 한다고 한 까닭(`disabled.reason`). 목록에 흐리게 보인다.
    pub disabled: Option<String>,
    /// 서버가 준 그대로 — `lsp.codeAction.resolve` 에 돌려준다.
    #[specta(type = JsonValue)]
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCodeActionList {
    pub server: String,
    pub stale_version: bool,
    /// 선호 액션(`isPreferred`) 먼저, 나머지는 서버가 준 차례.
    pub actions: Vec<LspCodeActionItem>,
    /// [`MAX_CODE_ACTIONS`] · [`MAX_CODE_ACTION_BYTES`] 로 버린 것이 있다.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCodeActionResolveParams {
    pub doc_id: String,
    /// 목록을 준 서버([`LspCodeActionList::server`]) — 그 서버에만 묻는다.
    pub server: String,
    /// [`LspCodeActionItem::raw`] 그대로.
    #[specta(type = JsonValue)]
    pub action: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCodeActionResolved {
    pub server: String,
    /// 적용할 편집(없으면 명령만).
    pub edit: Option<LspWorkspaceEdit>,
    /// 편집 뒤 돌릴 명령(`Command` — `lsp.command.execute` 에 그대로).
    #[specta(type = Option<JsonValue>)]
    pub command: Option<Value>,
}

// ---- 서버 명령 · 서버가 거는 편집 --------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCommandExecuteParams {
    pub doc_id: String,
    /// 그 명령을 준 서버(코드 액션 목록 · 완성 항목의 서버) — 그 서버에만 보낸다.
    pub server: String,
    /// 이 명령의 흐름 id(웹뷰가 정한다). 그 처리 중에 서버가 건 편집의 이벤트에 실린다.
    pub flow_id: String,
    /// 서버가 준 `Command`(`{ title, command, arguments? }`).
    #[specta(type = JsonValue)]
    pub command: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspCommandExecuted {
    pub server: String,
    #[specta(type = JsonValue)]
    pub result: Value,
}

/// [`TOPIC_APPLY_EDIT`] 의 내용.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspApplyEditEvent {
    pub flow_id: String,
    pub request_id: u32,
    pub server: String,
    /// 서버가 붙인 이름(`label` — 되돌리기 알림 등).
    pub label: Option<String>,
    pub edit: LspWorkspaceEdit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspApplyEditAnswerParams {
    pub flow_id: String,
    pub request_id: u32,
    pub applied: bool,
    #[serde(default)]
    pub failure_reason: Option<String>,
}

rpc_method! {
    /// rename 의 대상: 서버의 `prepareRename` 이 준 범위 · 처음 이름. 서버가 `prepareRename` 을 하지 않으면 `default`(웹뷰가 커서의
    /// 낱말을 쓴다), 그 자리를 바꿀 수 없으면 `invalid`. 맡을 서버가 없으면 5001.
    pub LspRenamePrepare: "lsp.rename.prepare" {
        params: LspRenamePrepareParams,
        result: LspRenamePrepared,
        tier: Read,
        roles: [Owner],
    }

    /// rename 의 편집(`textDocument/rename`). 쓰지 않는다 — 웹뷰의 적용기가 미리 확인 · 미리보기 · 쓰기를 한다(결정 M4-D14). 서버가
    /// 이름을 거절하면 5002(서버의 글). 받을 수 없는 편집(루트 밖 스킴 · `.git` · 상한)은 5120.
    pub LspRename: "lsp.rename" {
        params: LspRenameParams,
        result: LspEditAnswer,
        tier: Read,
        roles: [Owner],
    }

    /// 범위(와 그 자리의 진단)의 코드 액션(`textDocument/codeAction`). 선호 액션이 먼저다.
    pub LspCodeActionListMethod: "lsp.codeAction.list" {
        params: LspCodeActionListParams,
        result: LspCodeActionList,
        tier: Read,
        roles: [Owner],
    }

    /// 코드 액션 하나의 편집 · 명령. 편집이 없고 서버가 `codeAction/resolve` 를 하면 묻는다.
    pub LspCodeActionResolve: "lsp.codeAction.resolve" {
        params: LspCodeActionResolveParams,
        result: LspCodeActionResolved,
        tier: Read,
        roles: [Owner],
    }

    /// 서버 명령(`workspace/executeCommand`)을 돌린다. 그동안 그 서버가 거는 `workspace/applyEdit` 만 이 창에 이벤트
    /// `lsp.applyEdit`(흐름 id 실림)로 넘어오고, 창은 `lsp.applyEdit.answer` 로 답한다(결정 M4-D15).
    pub LspCommandExecute: "lsp.command.execute" {
        params: LspCommandExecuteParams,
        result: LspCommandExecuted,
        tier: Write,
        roles: [Owner],
    }

    /// `lsp.applyEdit` 이벤트에 답한다(그 명령을 부른 연결만).
    pub LspApplyEditAnswer: "lsp.applyEdit.answer" {
        params: LspApplyEditAnswerParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

// ---- 흐름(명령이 도는 동안) --------------------------------------------------------------------------------------------------

/// 창이 `workspace/applyEdit` 에 준 답.
#[derive(Debug, Clone)]
struct Answer {
    applied: bool,
    failure_reason: Option<String>,
}

/// 도는 명령 하나.
struct Flow {
    id: String,
    /// 명령을 보낸 인스턴스(프로젝트 · 서버 · 루트로 견준다 — `InstanceKey` 의 같음).
    key: InstanceKey,
    /// 그 인스턴스가 다른 창의 텍스트로 가진 파일을 판정한다(`foreign`).
    manager: LspManager,
    session: SessionId,
    hub: EventHub,
    closed: CancellationToken,
    next: u32,
    waiting: HashMap<u32, oneshot::Sender<Answer>>,
}

/// 도는 명령들. 서버 → 클라이언트 처리기(인스턴스마다 만들어진다)와 RPC 처리기가 함께 본다.
static FLOWS: LazyLock<Mutex<Vec<Flow>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn flows() -> MutexGuard<'static, Vec<Flow>> {
    FLOWS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 명령이 끝나면(어떤 길로든) 흐름을 뺀다 — 기다리던 답의 송신자가 떨어져 처리기가 `applied: false` 로 끝난다.
struct FlowGuard(String, SessionId);

impl Drop for FlowGuard {
    fn drop(&mut self) {
        flows().retain(|f| !(f.id == self.0 && f.session == self.1));
    }
}

fn not_applied(reason: &str) -> Value {
    json!({ "applied": false, "failureReason": reason })
}

/// 서버 → 클라이언트 `workspace/applyEdit` 처리기(인스턴스마다 — `crate::handlers`).
pub(crate) fn apply_edit(handlers: &mut Handlers, cx: &HandlerContext) {
    // 이 인스턴스의 열쇠(`InstanceKey` 의 같음은 프로젝트 · 서버 · 루트다 — 워크트리는 보지 않는다).
    let key = InstanceKey {
        project_id: cx.project_id.clone(),
        worktree: cx.worktree.clone(),
        server: cx.server.clone(),
        root: cx.root.clone(),
    };
    handlers.on("workspace/applyEdit", move |params: Value| {
        let key = key.clone();
        async move { Ok::<_, ResponseError>(forward(&key, &params).await) }
    });
}

/// 인스턴스 `key` 의 명령이 돌고 있으면 그 창에 넘기고 답을 기다린다.
async fn forward(key: &InstanceKey, params: &Value) -> Value {
    let mut edit = match normalize_edit(params.get("edit").unwrap_or(&Value::Null)) {
        Ok(edit) => edit,
        Err(e) => return not_applied(&e.message),
    };
    let label = params
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let (id, session, manager) = {
        let flows = flows();
        // 같은 인스턴스의 명령이 둘 돌면 나중 것(창은 한 번에 하나만 돌린다).
        let Some(at) = flows.iter().rposition(|f| f.key == *key) else {
            return not_applied(
                "Pitwall applies a server's edit only while running a command the person chose",
            );
        };
        let flow = &flows[at];
        // 한 명령의 편집은 한 번에 하나 — 앞 편집에 창이 답하기 전에 온 것은 받지 않는다(쏟아붓기에 창의 미리보기가 줄줄이 뜨지 않게).
        if !flow.waiting.is_empty() {
            return not_applied("an earlier edit of this command is still waiting for the window");
        }
        (flow.id.clone(), flow.session, flow.manager.clone())
    };
    // 판정은 흐름 표의 잠금 밖에서(관리자의 잠금을 겹쳐 쥐지 않는다).
    mark_foreign(&manager, key, session, &mut edit);
    let (rx, closed, request_id) = {
        let mut flows = flows();
        let Some(flow) = flows
            .iter_mut()
            .find(|f| f.id == id && f.session == session && f.key == *key)
        else {
            return not_applied("the command ended before the window answered");
        };
        // 판정하는 사이 같은 명령의 다른 편집이 먼저 자리를 잡았다.
        if !flow.waiting.is_empty() {
            return not_applied("an earlier edit of this command is still waiting for the window");
        }
        flow.next += 1;
        let request_id = flow.next;
        let (tx, rx) = oneshot::channel();
        flow.waiting.insert(request_id, tx);
        let event = LspApplyEditEvent {
            flow_id: flow.id.clone(),
            request_id,
            server: key.server.clone(),
            label,
            edit,
        };
        if flow
            .hub
            .publish(&key.project_id, TOPIC_APPLY_EDIT, &event)
            .unwrap_or(0)
            == 0
        {
            flow.waiting.remove(&request_id);
            return not_applied("the window that ran the command is not listening");
        }
        (rx, flow.closed.clone(), request_id)
    };
    let answer = tokio::select! {
        answer = rx => return match answer {
            Ok(answer) => json!({ "applied": answer.applied, "failureReason": answer.failure_reason }),
            Err(_) => not_applied("the command ended before the window answered"),
        },
        () = closed.cancelled() => not_applied("the window closed"),
        () = tokio::time::sleep(ANSWER_TIMEOUT) => not_applied("the window did not answer in time"),
    };
    // 답 없이 끝났다 — 자리를 비운다(남겨 두면 같은 명령의 다음 편집이 「앞 편집이 기다리는 중」 으로 거절된다).
    if let Some(flow) = flows()
        .iter_mut()
        .find(|f| f.id == id && f.session == session)
    {
        flow.waiting.remove(&request_id);
    }
    answer
}

/// 연결이 프로젝트 창이면 그 프로젝트.
fn window_project(ctx: &Ctx) -> Result<String, RpcError> {
    match ctx.scope() {
        ConnectionScope::Project(id) => Ok(id.clone()),
        _ => Err(RpcError::new(
            rpc_code::PERMISSION_DENIED,
            "only a project window runs language server commands",
        )),
    }
}

// ---- RPC 처리 ----------------------------------------------------------------------------------------------------------------

/// 서버 오류를 그대로 넘기되 `RpcError` 의 `data.message`(서버의 글)를 꺼낸다.
fn server_message(e: &RpcError) -> Option<String> {
    (e.code == manager_code::SERVER_ERROR).then(|| {
        e.data
            .as_ref()
            .and_then(|d| d.get("message"))
            .and_then(Value::as_str)
            .unwrap_or(&e.message)
            .to_owned()
    })
}

async fn prepare_rename(
    m: &LspManager,
    ctx: &Ctx,
    p: LspRenamePrepareParams,
) -> Result<LspRenamePrepared, RpcError> {
    let routes = m.feature_routes(
        ctx.session(),
        &p.doc_id,
        &[
            (LspFeature::Rename, "textDocument/rename"),
            (LspFeature::Rename, "textDocument/prepareRename"),
        ],
    )?;
    let Some((_, rename_server, _)) = routes.first() else {
        return Err(RpcError::new(
            manager_code::NO_SERVER,
            "no running language server offers rename",
        ));
    };
    // 이름을 바꿀 서버가 `prepareRename` 도 해야 그 답을 쓴다(라우팅은 메서드마다라 다른 서버가 고를 수 있다).
    let prepares = routes
        .get(1)
        .is_some_and(|(_, server, _)| server == rename_server);
    if !prepares {
        return Ok(LspRenamePrepared {
            server: rename_server.clone(),
            stale_version: false,
            target: LspRenameTarget::Default,
        });
    }
    let answer = m
        .request(
            ctx,
            FeatureRequest {
                doc_id: &p.doc_id,
                feature: LspFeature::Rename,
                method: "textDocument/prepareRename",
                expected_version: Some(p.version),
                timeout: None,
            },
            |view| Ok(json!({ "textDocument": view.text_document(), "position": view.position(p.offset)? })),
        )
        .await;
    let answer = match answer {
        Ok(answer) => answer,
        Err(e) => {
            return match server_message(&e) {
                Some(message) => Ok(LspRenamePrepared {
                    server: rename_server.clone(),
                    stale_version: false,
                    target: LspRenameTarget::Invalid {
                        message: Some(message),
                    },
                }),
                None => Err(e),
            };
        }
    };
    let offset = |key: &str, range: &Value| -> Option<u32> {
        let position = serde_json::from_value(range.get(key)?.clone()).ok()?;
        Some(answer.doc.offset(position))
    };
    let result = &answer.result;
    let target = if result.is_null() {
        LspRenameTarget::Invalid { message: None }
    } else if result
        .get("defaultBehavior")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        LspRenameTarget::Default
    } else {
        // `Range` 또는 `{ range, placeholder }`.
        let range = result.get("range").unwrap_or(result);
        match (offset("start", range), offset("end", range)) {
            (Some(from), Some(to)) => LspRenameTarget::Range {
                from,
                to,
                placeholder: result
                    .get("placeholder")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            _ => LspRenameTarget::Invalid { message: None },
        }
    };
    Ok(LspRenamePrepared {
        server: answer.server,
        stale_version: answer.stale_version,
        target,
    })
}

async fn rename(m: &LspManager, ctx: &Ctx, p: LspRenameParams) -> Result<LspEditAnswer, RpcError> {
    let answer = m
        .request(
            ctx,
            FeatureRequest {
                doc_id: &p.doc_id,
                feature: LspFeature::Rename,
                method: "textDocument/rename",
                expected_version: Some(p.version),
                timeout: None,
            },
            |view| {
                Ok(json!({
                    "textDocument": view.text_document(),
                    "position": view.position(p.offset)?,
                    "newName": p.new_name,
                }))
            },
        )
        .await?;
    let mut edit = normalize_edit(&answer.result)?;
    mark_foreign(m, &answer.key, ctx.session(), &mut edit);
    Ok(LspEditAnswer {
        edit,
        server: answer.server,
        stale_version: answer.stale_version,
    })
}

/// 부른 쪽이 준 진단 — 그 문서 버전의 것이고 상한 안이면 그대로, 아니면 없다.
fn context_diagnostics(given: Option<LspContextDiagnostics>, version: u32) -> Vec<Value> {
    let Some(given) = given.filter(|d| d.version == version) else {
        return Vec::new();
    };
    let mut items = given.items;
    items.truncate(MAX_CONTEXT_DIAGNOSTICS);
    let size = serde_json::to_vec(&items).map_or(usize::MAX, |v| v.len());
    if size > MAX_CONTEXT_DIAGNOSTICS_BYTES {
        return Vec::new();
    }
    items
}

/// 서버의 코드 액션 목록 → 항목들(선호 먼저). 못 읽는 항목 · 큰 항목은 버린다.
pub fn code_action_items(result: &Value) -> (Vec<LspCodeActionItem>, bool) {
    let list = result.as_array().map(Vec::as_slice).unwrap_or_default();
    let mut truncated = false;
    let mut items: Vec<LspCodeActionItem> = list
        .iter()
        .filter_map(|raw| {
            let title = raw.get("title")?.as_str()?.to_owned();
            if serde_json::to_vec(raw).map_or(usize::MAX, |v| v.len()) > MAX_CODE_ACTION_BYTES {
                truncated = true;
                return None;
            }
            // `Command` 는 `command` 가 글이다 — 종류 · 선호가 없다.
            let is_command = raw.get("command").is_some_and(Value::is_string);
            Some(LspCodeActionItem {
                title,
                kind: (!is_command)
                    .then(|| raw.get("kind").and_then(Value::as_str).map(str::to_owned))
                    .flatten(),
                is_preferred: !is_command
                    && raw
                        .get("isPreferred")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                disabled: raw
                    .get("disabled")
                    .and_then(|d| d.get("reason"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                raw: raw.clone(),
            })
        })
        .collect();
    if items.len() > MAX_CODE_ACTIONS {
        items.truncate(MAX_CODE_ACTIONS);
        truncated = true;
    }
    // 선호 먼저(그 안에서는 서버 차례 — 안정 정렬).
    items.sort_by_key(|i| !i.is_preferred);
    (items, truncated)
}

/// 진단 저장(M4-06)에서 `view` 의 `from..to` 와 겹치는 진단 — `key` 의 서버가 낸 것이고 요청 문서 버전의 것만, 상한 안에서.
fn stored_diagnostics(
    store: &DiagnosticStore,
    key: &InstanceKey,
    view: &crate::manager::DocView,
    from: u32,
    to: u32,
) -> Result<Vec<Value>, RpcError> {
    let Some(path) = uri_path(&view.uri) else {
        return Ok(Vec::new());
    };
    let at = |offset| {
        view.position(offset).map(|p| LspPosition {
            line: p.line,
            character: p.character,
        })
    };
    let range = LspRange {
        start: at(from)?,
        end: at(to)?,
    };
    let version = i32::try_from(view.version).ok();
    let mut size = 0;
    Ok(store
        .in_range(&key.project_id, &path, range)
        .iter()
        .filter(|d| d.server == key.server && d.version.is_some() && d.version == version)
        .take(MAX_CONTEXT_DIAGNOSTICS)
        .map(crate::diagnostics::to_lsp)
        .take_while(|d| {
            size += serde_json::to_vec(d).map_or(usize::MAX / 2, |v| v.len());
            size <= MAX_CONTEXT_DIAGNOSTICS_BYTES
        })
        .collect())
}

async fn code_actions(
    m: &LspManager,
    store: &DiagnosticStore,
    ctx: &Ctx,
    p: LspCodeActionListParams,
) -> Result<LspCodeActionList, RpcError> {
    // `⌥↩` · 전구는 진단 없이 묻는다 — 그때는 진단 저장(M4-06)의 범위 조회로 채운다(메인 판정 2026-11-02 — 나중에 병합하는 쪽이
    // 더한다). 코드 액션을 맡을 서버가 낸 것만 그 서버에 넘기므로 그 서버로만 묻는다. 「빠른 수정」 처럼 부른 쪽이 준 진단은 그것만.
    let stored = match &p.diagnostics {
        Some(_) => None,
        None => m
            .feature_route_keys(
                ctx.session(),
                &p.doc_id,
                &[(LspFeature::CodeAction, "textDocument/codeAction")],
                None,
            )
            .ok()
            .and_then(|routes| routes.into_iter().next())
            .map(|(_, key, _)| key),
    };
    let diagnostics = context_diagnostics(p.diagnostics, p.version);
    let request = FeatureRequest {
        doc_id: &p.doc_id,
        feature: LspFeature::CodeAction,
        method: "textDocument/codeAction",
        expected_version: Some(p.version),
        timeout: None,
    };
    let params = |view: &crate::manager::DocView| {
        let diagnostics = match &stored {
            Some(key) => stored_diagnostics(store, key, view, p.from, p.to)?,
            None => diagnostics.clone(),
        };
        let mut context = json!({
            "diagnostics": diagnostics,
            "triggerKind": match p.trigger {
                LspCodeActionTrigger::Invoked => 1,
                LspCodeActionTrigger::Automatic => 2,
            },
        });
        if let Some(only) = &p.only {
            context["only"] = json!(only);
        }
        Ok(json!({
            "textDocument": view.text_document(),
            "range": { "start": view.position(p.from)?, "end": view.position(p.to)? },
            "context": context,
        }))
    };
    let answer = match &stored {
        Some(key) => m.request_on(ctx, &key.server, request, params).await?,
        None => m.request(ctx, request, params).await?,
    };
    let (actions, truncated) = code_action_items(&answer.result);
    Ok(LspCodeActionList {
        server: answer.server,
        stale_version: answer.stale_version,
        actions,
        truncated,
    })
}

async fn resolve_action(
    m: &LspManager,
    ctx: &Ctx,
    p: LspCodeActionResolveParams,
) -> Result<LspCodeActionResolved, RpcError> {
    // `Command` 자체 — 돌릴 명령뿐이다(`lsp.command.execute` 가 그 서버로 보낸다).
    if p.action.get("command").is_some_and(Value::is_string) {
        return Ok(LspCodeActionResolved {
            server: p.server,
            edit: None,
            command: Some(p.action),
        });
    }
    // 목록을 준 서버만 본다 — 그 서버가 resolve 를 하지 않으면 받은 그대로.
    let route = |method: &str| -> Result<Option<InstanceKey>, RpcError> {
        Ok(m.feature_route_keys(
            ctx.session(),
            &p.doc_id,
            &[(LspFeature::CodeAction, method)],
            Some(&p.server),
        )?
        .into_iter()
        .next()
        .map(|(_, key, _)| key))
    };
    let resolver = route("codeAction/resolve")?;
    let resolvable = resolver.is_some();
    // 편집의 「다른 창」 표지를 판정할 인스턴스(그 서버가 목록을 준 인스턴스).
    let mut key = match resolver {
        Some(key) => Some(key),
        None => route("textDocument/codeAction")?,
    };
    let mut action = p.action;
    if action.get("edit").is_none_or(Value::is_null) && resolvable {
        let answer = m
            .request_on(
                ctx,
                &p.server,
                FeatureRequest {
                    doc_id: &p.doc_id,
                    feature: LspFeature::CodeAction,
                    method: "codeAction/resolve",
                    expected_version: None,
                    timeout: None,
                },
                |_| Ok(action.clone()),
            )
            .await?;
        key = Some(answer.key);
        action = answer.result;
    }
    let edit = match action.get("edit").filter(|e| !e.is_null()) {
        Some(edit) => {
            let mut edit = normalize_edit(edit)?;
            if let Some(key) = &key {
                mark_foreign(m, key, ctx.session(), &mut edit);
            }
            Some(edit)
        }
        None => None,
    };
    Ok(LspCodeActionResolved {
        server: p.server,
        edit,
        command: action.get("command").filter(|c| !c.is_null()).cloned(),
    })
}

/// 서버 명령을 돌린다(`workspace/executeCommand`) — 도는 동안 그 서버가 거는 `workspace/applyEdit` 은 이 연결(창)에 흐름
/// `p.flow_id` 로 넘어간다(결정 M4-D15). 서버에 명령을 보내는 하나뿐인 길이다 — 완성 항목의 `command`(M4-11)도 이것을 부른다
/// (나중에 병합하는 쪽이 `lsp.completion.execute` 를 이 도우미로 합친다 — 메인 판정 2026-11-02).
pub async fn execute_command(
    m: &LspManager,
    ctx: &Ctx,
    p: LspCommandExecuteParams,
) -> Result<LspCommandExecuted, RpcError> {
    // 프로젝트 창만(흐름의 이벤트는 그 프로젝트 범위로 간다 — 인스턴스 열쇠의 프로젝트).
    window_project(ctx)?;
    let command = p
        .command
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("`command.command` must be a string"))?
        .to_owned();
    let arguments = p.command.get("arguments").cloned();
    // 그 명령을 준 서버의 인스턴스만 — 그 서버가 명령을 하지 않으면 5001(다른 서버로 보내지 않는다).
    let key = m
        .feature_route_keys(
            ctx.session(),
            &p.doc_id,
            &[(LspFeature::CodeAction, "workspace/executeCommand")],
            Some(&p.server),
        )?
        .into_iter()
        .next()
        .map(|(_, key, _)| key)
        .ok_or_else(|| {
            RpcError::new(
                manager_code::NO_SERVER,
                format!(
                    "the language server `{}` does not run commands for this document",
                    p.server
                ),
            )
        })?;
    {
        let mut flows = flows();
        if flows
            .iter()
            .any(|f| f.id == p.flow_id && f.session == ctx.session())
        {
            return Err(RpcError::invalid_params(format!(
                "the flow `{}` is already running",
                p.flow_id
            )));
        }
        flows.push(Flow {
            id: p.flow_id.clone(),
            key,
            manager: m.clone(),
            session: ctx.session(),
            hub: ctx.events().clone(),
            closed: ctx.connection_closed(),
            next: 0,
            waiting: HashMap::new(),
        });
    }
    let _guard = FlowGuard(p.flow_id.clone(), ctx.session());
    let answer = m
        .request_on(
            ctx,
            &p.server,
            FeatureRequest {
                doc_id: &p.doc_id,
                feature: LspFeature::CodeAction,
                method: "workspace/executeCommand",
                expected_version: None,
                timeout: Some(EXECUTE_TIMEOUT),
            },
            |_| {
                let mut params = json!({ "command": command });
                if let Some(arguments) = &arguments {
                    params["arguments"] = arguments.clone();
                }
                Ok(params)
            },
        )
        .await?;
    Ok(LspCommandExecuted {
        server: answer.server,
        result: answer.result,
    })
}

fn answer(ctx: &Ctx, p: LspApplyEditAnswerParams) -> Result<(), RpcError> {
    let mut flows = flows();
    let tx = flows
        .iter_mut()
        .find(|f| f.id == p.flow_id && f.session == ctx.session())
        .and_then(|f| f.waiting.remove(&p.request_id))
        .ok_or_else(|| {
            RpcError::new(
                rpc_code::NOT_FOUND,
                format!(
                    "no edit `{}` waits for an answer in the flow `{}`",
                    p.request_id, p.flow_id
                ),
            )
        })?;
    let _ = tx.send(Answer {
        applied: p.applied,
        failure_reason: p.failure_reason,
    });
    Ok(())
}

/// 핸들러를 붙인다(`crate::register` 가 부른다).
pub fn register(router: &mut Router, manager: &LspManager, diagnostics: DiagnosticStore) {
    let m = Arc::new(manager.clone());
    let at = Arc::clone(&m);
    router.unary(LspRenamePrepare, move |ctx, p| {
        let m = Arc::clone(&at);
        async move { prepare_rename(&m, &ctx, p).await }
    });
    let at = Arc::clone(&m);
    router.unary(LspRename, move |ctx, p| {
        let m = Arc::clone(&at);
        async move { rename(&m, &ctx, p).await }
    });
    let at = Arc::clone(&m);
    router.unary(LspCodeActionListMethod, move |ctx, p| {
        let m = Arc::clone(&at);
        let store = diagnostics.clone();
        async move { code_actions(&m, &store, &ctx, p).await }
    });
    let at = Arc::clone(&m);
    router.unary(LspCodeActionResolve, move |ctx, p| {
        let m = Arc::clone(&at);
        async move { resolve_action(&m, &ctx, p).await }
    });
    let at = Arc::clone(&m);
    router.unary(LspCommandExecute, move |ctx, p| {
        let m = Arc::clone(&at);
        async move { execute_command(&m, &ctx, p).await }
    });
    router.unary(
        LspApplyEditAnswer,
        move |ctx, p| async move { answer(&ctx, p) },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> LspPosition {
        LspPosition { line, character }
    }

    #[test]
    fn document_changes_keep_their_order_and_options() {
        let edit = json!({ "documentChanges": [
            { "kind": "create", "uri": "file:///p/new.rs", "options": { "ignoreIfExists": true } },
            { "textDocument": { "uri": "file:///p/a.rs", "version": 3 },
              "edits": [{ "range": { "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 2 } }, "newText": "x" }] },
            { "kind": "rename", "oldUri": "file:///p/a.rs", "newUri": "file:///p/b.rs" },
            { "kind": "delete", "uri": "file:///p/old", "options": { "recursive": true } },
        ]});
        assert_eq!(
            normalize_edit(&edit).unwrap().changes,
            [
                LspFileChange::Create {
                    path: "/p/new.rs".into(),
                    overwrite: false,
                    ignore_if_exists: true
                },
                LspFileChange::Text {
                    path: "/p/a.rs".into(),
                    version: Some(3),
                    edits: vec![LspTextEdit {
                        range: LspRange {
                            start: pos(0, 1),
                            end: pos(0, 2)
                        },
                        new_text: "x".into()
                    }],
                    foreign: false,
                },
                LspFileChange::Rename {
                    from: "/p/a.rs".into(),
                    to: "/p/b.rs".into(),
                    overwrite: false,
                    ignore_if_exists: false
                },
                LspFileChange::Delete {
                    path: "/p/old".into(),
                    recursive: true,
                    ignore_if_not_exists: false
                },
            ]
        );
        // `changes` 표는 경로 순, 버전 없음.
        let edit = json!({ "changes": {
            "file:///p/z.rs": [],
            "file:///p/a.rs": [{ "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } }, "newText": "" }],
        }});
        let paths: Vec<_> = normalize_edit(&edit)
            .unwrap()
            .changes
            .into_iter()
            .map(|c| match c {
                LspFileChange::Text { path, version, .. } => (path, version),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            paths,
            [("/p/a.rs".to_owned(), None), ("/p/z.rs".to_owned(), None)]
        );
        assert_eq!(
            normalize_edit(&Value::Null).unwrap(),
            LspWorkspaceEdit::default()
        );
    }

    #[test]
    fn edits_outside_files_or_inside_git_are_refused_whole() {
        let reason = |edit: Value| {
            let e = normalize_edit(&edit).unwrap_err();
            assert_eq!(e.code, code::EDIT_REFUSED);
            e.data.unwrap()["reason"].as_str().unwrap().to_owned()
        };
        let text = |uri: &str| json!({ "changes": { "file:///p/ok.rs": [], uri: [] } });
        assert_eq!(reason(text("jdt://contents/x")), "notFile");
        assert_eq!(reason(text("untitled:Untitled-1")), "notFile");
        assert_eq!(
            reason(
                json!({ "documentChanges": [{ "kind": "create", "uri": "file:///p/.git/hooks/pre-commit" }] })
            ),
            "git"
        );
        assert_eq!(
            reason(
                json!({ "documentChanges": [{ "kind": "rename", "oldUri": "file:///p/a", "newUri": "file:///p/.GIT/config" }] })
            ),
            "git"
        );
        assert_eq!(
            reason(json!({ "documentChanges": [{ "kind": "explode", "uri": "file:///p/a" }] })),
            "malformed"
        );
        assert_eq!(
            reason(json!({ "changes": { "file:///p/a": [{ "newText": "x" }] } })),
            "malformed"
        );
        let many: serde_json::Map<String, Value> = (0..=MAX_EDIT_FILES)
            .map(|i| (format!("file:///p/{i}.rs"), json!([])))
            .collect();
        assert_eq!(reason(json!({ "changes": many })), "tooLarge");
    }

    #[test]
    fn preferred_code_actions_come_first_and_commands_have_no_kind() {
        let (items, truncated) = code_action_items(&json!([
            { "title": "a", "kind": "refactor" },
            { "title": "cmd", "command": "do.it", "arguments": [] },
            { "title": "fix", "kind": "quickfix", "isPreferred": true },
            { "title": "off", "kind": "quickfix", "disabled": { "reason": "nope" } },
            { "no": "title" },
        ]));
        assert!(!truncated);
        let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["fix", "a", "cmd", "off"]);
        assert_eq!(items[2].kind, None);
        assert_eq!(items[3].disabled.as_deref(), Some("nope"));
        let big = json!([{ "title": "big", "data": "x".repeat(MAX_CODE_ACTION_BYTES) }]);
        assert_eq!(code_action_items(&big), (Vec::new(), true));
    }

    #[test]
    fn context_diagnostics_must_match_the_version_and_the_limits() {
        let given = |version, n: usize| LspContextDiagnostics {
            version,
            items: (0..n).map(|i| json!({ "message": i })).collect(),
        };
        assert_eq!(context_diagnostics(Some(given(3, 2)), 3).len(), 2);
        assert!(context_diagnostics(Some(given(2, 2)), 3).is_empty());
        assert!(context_diagnostics(None, 3).is_empty());
        assert_eq!(
            context_diagnostics(Some(given(3, MAX_CONTEXT_DIAGNOSTICS + 5)), 3).len(),
            MAX_CONTEXT_DIAGNOSTICS
        );
        let huge = LspContextDiagnostics {
            version: 3,
            items: vec![json!({ "message": "x".repeat(MAX_CONTEXT_DIAGNOSTICS_BYTES) })],
        };
        assert!(context_diagnostics(Some(huge), 3).is_empty());
    }
}
