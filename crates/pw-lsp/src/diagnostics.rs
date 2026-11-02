//! 진단(15-m4-plan.md M4-06): 서버들이 낸 진단을 (프로젝트, 파일, 서버)별로 모으고 웹뷰 · (M5 의) MCP 가 읽게 한다.
//!
//! **받기** — 두 길이다.
//! - push: 서버 알림 `textDocument/publishDiagnostics`(`LspManager::on_notification`).
//! - pull(LSP 3.17 `textDocument/diagnostic`, 서버가 지원하면 — 열린 문서만): 서버에 문서 텍스트를 보낼 때마다
//!   (`LspManager::on_synced`) 그 문서를 당긴다. 같은 문서의 새 버전이 오면 앞 pull 을 취소(`$/cancelRequest`)하고 그 답은 버린다.
//!   동시에 도는 pull 은 [`PULL_CONCURRENCY`] 개까지다. 서버의 `workspace/diagnostic/refresh`(`crate::handlers` 가 넘긴다)는 그
//!   서버에 열린 문서를 모두 다시 당기되, 몰려 와도 합친다([`REFRESH_SETTLE`] 동안 모으고, 당기는 동안 온 것은 끝난 뒤 한 번). rust-analyzer 는 pull 을 알리면
//!   열린 문서의 자체 진단을 pull 로만 주고(flycheck 는 push), 색인이 끝나면 refresh 를 보낸다.
//!   push 와 pull 은 따로 쌓는다(같은 서버가 둘 다 낸다 — rust-analyzer).
//!
//! **버리기** — 출처의 세대가 지난 것(끝난 서버가 늦게 보낸 알림 · 늦게 온 pull 답 — `LspManager::is_current`), 진단의 `version` 이
//! 서버에 보낸 그 문서의 버전보다 앞선 것(`LspManager::synced_version` — 지난 텍스트 기준), `file:` 이 아니거나 그 인스턴스의 작업
//! 공간 루트 밖 파일의 것. 상태가 바뀔 때마다(`LspManager::on_status` — 서버 끝 · 재기동 · 신뢰 해제 · 창 닫힘 · 문서 닫기) 지금
//! 세대가 아닌 출처의 진단과 서버에 더는 열려 있지 않은 문서의 pull 진단을 지운다.
//!
//! **상한**(망가진 서버가 진단을 쏟아부어도 Host · 웹뷰가 버틴다): 파일당 [`FILE_LIMIT`] 개, 프로젝트 [`PROJECT_LIMIT`] 개. 넘으면
//! 심각도 순(오류 먼저)으로 앞의 것만 두고 `truncated`. 진단 하나의 글도 자른다(메시지 [`MESSAGE_LIMIT`] 바이트 · 코드 · 출처
//! [`SHORT_LIMIT`] · 관련 정보 [`RELATED_LIMIT`] 개 · `data` [`DATA_LIMIT`] 바이트 — 넘는 `data` 는 버린다)고, 진단 하나를 통째로
//! [`DIAGNOSTIC_LIMIT`] 안에 둔다(넘으면 관련 정보를 버린다 — URI 가 [`HREF_LIMIT`] 를 넘는 관련 정보도 버린다). `codeDescription`
//! 링크는 `http(s)` 만.
//!
//! **내보내기** — 이벤트 [`TOPIC_DIAGNOSTICS`](경로가 바뀔 때마다 그 경로의 모든 서버 진단 — 프로젝트 범위), RPC
//! `lsp.diagnostics.list`(쪽 단위 · 입력 struct — M5 의 `diagnostics` 툴이 될 꼴, 결정 M4-D21 — `mcp: true` 는 달지 않는다) ·
//! `lsp.diagnostics.counts`. 위치는 LSP 그대로(줄 · UTF-16 열, 0 부터) — 웹뷰의 오프셋과 같은 단위다.
//!
//! **잠금 차례**: 저장소 잠금 → 관리자 잠금만 있다(관리자는 자기 잠금을 쥔 채 이 모듈을 부르지 않는다 — `on_synced` 의 받는 쪽은
//! 태스크만 띄운다). 그래서 넣기 · 지우기가 세대 판정과 한 잠금 안에서 일어나 늦은 진단이 지운 뒤에 남지 않는다.
//!
//! 코드 액션 요청의 진단 문맥(M4-12)은 [`DiagnosticStore::in_range`] 로 읽는다.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use pw_core::bus::Scope;
use pw_core::registry::exec::{Launch, exec_grant};
use pw_rpc::{JsonValue, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub use crate::edit::{LspPosition, LspRange};
use crate::manager::{InstanceKey, LspManager, NotificationSource};
use crate::message::code as lsp_code;
use crate::server::{RequestError, RequestOptions};
use crate::uri::{file_uri, uri_path};

/// 경로의 진단이 바뀌면 나가는 이벤트(프로젝트 범위). 내용은 [`LspDiagnosticsEvent`].
pub const TOPIC_DIAGNOSTICS: &str = "lsp.diagnostics";
/// 파일 하나(모든 서버를 합쳐)의 진단 상한.
pub const FILE_LIMIT: usize = 1_000;
/// 프로젝트 하나의 진단 상한.
pub const PROJECT_LIMIT: usize = 100_000;
/// 진단 메시지의 상한(UTF-8 바이트). 넘으면 글자 경계에서 자르고 `…` 를 붙인다.
pub const MESSAGE_LIMIT: usize = 4 * 1024;
/// 관련 정보 메시지의 상한(바이트).
pub const RELATED_MESSAGE_LIMIT: usize = 1024;
/// 코드 · 출처의 상한(바이트).
pub const SHORT_LIMIT: usize = 256;
/// `codeDescription.href` 의 상한(바이트). 넘으면 링크를 버린다.
pub const HREF_LIMIT: usize = 2048;
/// 진단 하나의 관련 정보 수 상한.
pub const RELATED_LIMIT: usize = 16;
/// 진단 하나의 상한(직렬화 바이트 — 관련 정보의 경로 · URI 까지). 관련 정보를 뺀 나머지는 칸마다의 상한으로 이 안에 든다 — 넘으면
/// 관련 정보를 뒤에서부터 버린다.
pub const DIAGNOSTIC_LIMIT: usize = 16 * 1024;
/// 진단 하나의 `data`(코드 액션이 되돌려 받는 값)의 상한(직렬화 바이트). 넘으면 버린다.
pub const DATA_LIMIT: usize = 4 * 1024;
/// 한 번에 도는 pull 의 상한(모든 서버 · 문서 합쳐).
pub const PULL_CONCURRENCY: usize = 8;
/// `workspace/diagnostic/refresh` 를 모으는 시간 — 이 안에 온 refresh 는 한 번으로 합친다.
pub const REFRESH_SETTLE: Duration = Duration::from_millis(20);
/// `lsp.diagnostics.list` 한 쪽의 상한.
pub const LIST_LIMIT: u32 = 5_000;

// ---- 형 ------------------------------------------------------------------------------------------------------------------------

/// 심각도. 차례는 오류가 먼저다(작을수록 심하다 — LSP 의 수와 같은 차례).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl LspSeverity {
    /// LSP 의 수(1 ~ 4). 없거나 모르는 수는 오류로 본다(VS Code 와 같다 — 명세는 클라이언트에 맡긴다).
    fn from_lsp(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_u64) {
            Some(2) => Self::Warning,
            Some(3) => Self::Information,
            Some(4) => Self::Hint,
            _ => Self::Error,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// 진단 꼬리표.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspDiagnosticTag {
    /// 쓰이지 않는 코드(흐리게).
    Unnecessary,
    /// 낡은 것(취소선).
    Deprecated,
}

/// 진단의 관련 정보(다른 자리 — 루트 밖일 수 있다. 여는 것은 M4-08 의 토큰 길로만).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspRelatedInformation {
    /// `file:` URI 면 그 경로.
    pub path: Option<String>,
    pub uri: String,
    pub range: LspRange,
    pub message: String,
}

/// 진단을 받은 길. 같은 서버가 둘 다 낸다(rust-analyzer — 자체 진단은 pull, flycheck 는 push). 한 서버의 진단은 길마다 따로 바뀐다 —
/// 웹뷰는 (서버, 길)마다 바꿔 끼운다.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspDiagnosticOrigin {
    /// `textDocument/publishDiagnostics`.
    Push,
    /// `textDocument/diagnostic`.
    Pull,
}

/// 진단 하나.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnostic {
    /// 낸 서버 id.
    pub server: String,
    /// 받은 길.
    pub origin: LspDiagnosticOrigin,
    /// 이 진단이 기준으로 한 서버 쪽 문서 버전(없으면 서버가 알리지 않았다 — 열지 않은 파일 등). LSP 의 `integer`(i32).
    pub version: Option<i32>,
    pub range: LspRange,
    pub severity: LspSeverity,
    pub message: String,
    /// 서버 · 도구의 진단 코드(`E0308` 등).
    pub code: Option<String>,
    /// 진단을 낸 도구(`rustc` · `ts` 등 — 서버가 알린 것).
    pub source: Option<String>,
    /// 코드 설명 링크(`http(s)` 만).
    pub code_href: Option<String>,
    pub tags: Vec<LspDiagnosticTag>,
    pub related: Vec<LspRelatedInformation>,
    /// 서버가 코드 액션 요청에서 되돌려 받는 값([`DATA_LIMIT`] 안일 때만).
    #[specta(type = Option<JsonValue>)]
    pub data: Option<Value>,
}

/// [`TOPIC_DIAGNOSTICS`] 의 내용: 경로 하나의 지금 진단 전부(모든 서버 — 심각도 · 위치 순).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsEvent {
    pub project_id: String,
    /// 그 진단을 낸 인스턴스의 워크트리(메인 체크아웃이면 없다).
    pub worktree: Option<String>,
    pub path: String,
    pub diagnostics: Vec<LspDiagnostic>,
    /// 상한에 걸려 일부만 실었다.
    pub truncated: bool,
}

/// `lsp.diagnostics.list` 의 입력(M5 의 `diagnostics` 툴이 될 꼴 — 결정 M4-D21).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsListParams {
    pub project_id: String,
    /// 활성 워크트리(없으면 메인 체크아웃).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 이 파일만(절대 경로).
    #[serde(default)]
    pub path: Option<String>,
    /// 이 심각도와 그보다 심한 것만.
    #[serde(default)]
    pub min_severity: Option<LspSeverity>,
    /// 앞 쪽의 `nextCursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 한 쪽의 진단 수(1 ~ [`LIST_LIMIT`]).
    pub limit: u32,
}

/// 파일 하나의 진단.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsFile {
    pub path: String,
    pub diagnostics: Vec<LspDiagnostic>,
    /// 이 파일의 진단이 상한에 걸려 일부만 있다.
    pub truncated: bool,
}

/// `lsp.diagnostics.list` 의 결과: 경로 차례로 파일들. 한 파일의 진단이 두 쪽에 걸칠 수 있다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsList {
    pub files: Vec<LspDiagnosticsFile>,
    /// 다음 쪽이 있으면 그 커서.
    pub next_cursor: Option<String>,
    /// 프로젝트의 진단이 상한에 걸렸다(어느 파일이든).
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsCountsParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
}

/// 심각도별 수(상태바).
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsCounts {
    pub error: u32,
    pub warning: u32,
    pub information: u32,
    pub hint: u32,
    /// 상한에 걸려 센 것보다 많다.
    pub truncated: bool,
}

rpc_method! {
    /// 프로젝트(활성 워크트리)의 진단을 쪽 단위로: 경로 차례, 파일 안은 심각도 · 위치 차례. 서버가 보낸 만큼만 — 열지 않은 파일은
    /// 서버가 스스로 보낸 것만 있다. 그 프로젝트 창의 연결만(미신뢰면 서버가 없어 빈 목록이다).
    pub LspDiagnosticsListGet: "lsp.diagnostics.list" {
        params: LspDiagnosticsListParams,
        result: LspDiagnosticsList,
        tier: Read,
        roles: [Owner],
    }

    /// 프로젝트(활성 워크트리)의 심각도별 진단 수(상태바의 `⚠ n ⛔ n`). 그 프로젝트 창의 연결만.
    pub LspDiagnosticsCountsGet: "lsp.diagnostics.counts" {
        params: LspDiagnosticsCountsParams,
        result: LspDiagnosticsCounts,
        tier: Read,
        roles: [Owner],
    }
}

pw_rpc::rpc_type!(LspDiagnosticsEvent);

// ---- 저장 ----------------------------------------------------------------------------------------------------------------------

use LspDiagnosticOrigin as Origin;

/// 지우기에서 출처 하나의 판정 열쇠: (서버, 작업 공간 루트, 워크트리, 세대).
type Judged = (String, PathBuf, Option<String>, u64);

/// 파일 하나의 진단을 낸 출처: (서버, 작업 공간 루트, 길).
type EntryKey = (String, PathBuf, Origin);

struct Entry {
    generation: u64,
    worktree: Option<String>,
    uri: String,
    items: Vec<LspDiagnostic>,
    truncated: bool,
    /// pull 의 `resultId`(다음 pull 의 `previousResultId`).
    result_id: Option<String>,
}

#[derive(Default)]
struct FileDiags {
    entries: BTreeMap<EntryKey, Entry>,
}

impl FileDiags {
    /// 모든 출처를 합친 진단(심각도 · 위치 차례, [`FILE_LIMIT`] 까지)과 잘렸는가.
    fn merged(&self) -> (Vec<LspDiagnostic>, bool) {
        let mut all: Vec<LspDiagnostic> = self
            .entries
            .values()
            .flat_map(|e| e.items.iter().cloned())
            .collect();
        all.sort_by_key(order);
        let mut truncated = self.entries.values().any(|e| e.truncated);
        if all.len() > FILE_LIMIT {
            all.truncate(FILE_LIMIT);
            truncated = true;
        }
        (all, truncated)
    }

    fn worktree(&self) -> Option<String> {
        self.entries
            .values()
            .next()
            .and_then(|e| e.worktree.clone())
    }
}

fn order(d: &LspDiagnostic) -> (LspSeverity, (u32, u32), (u32, u32)) {
    (d.severity, at(d.range.start), at(d.range.end))
}

/// 위치의 차례 열쇠(줄, 열). 위치 형은 편집 적용기(M4-12 — `crate::edit`)와 같은 것을 쓴다.
fn at(p: LspPosition) -> (u32, u32) {
    (p.line, p.character)
}

#[derive(Default)]
struct ProjectDiags {
    files: BTreeMap<String, FileDiags>,
    total: usize,
    /// 프로젝트 상한에 잘린 pull 항목(경로, 출처). 자리가 나면 다시 당긴다([`ProjectDiags::take_refills`]). push 는 서버에 다시
    /// 달라고 할 길이 없어 넣지 않는다 — 서버가 다시 보낼 때까지 `truncated` 로 남는다.
    cut_pulls: BTreeSet<(String, EntryKey)>,
}

impl ProjectDiags {
    /// 자리가 있으면 상한에 잘린 pull 을 모두 꺼낸다(다시 당길 출처 · URI). 다시 잘리면 넣기가 도로 넣는다.
    fn take_refills(&mut self, project_id: &str) -> Vec<(NotificationSource, String)> {
        if self.total >= PROJECT_LIMIT || self.cut_pulls.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut self.cut_pulls)
            .into_iter()
            .filter_map(|(path, key)| {
                let entry = self.files.get(&path)?.entries.get(&key)?;
                let (server, root, _) = key;
                Some((
                    NotificationSource {
                        key: InstanceKey {
                            project_id: project_id.to_owned(),
                            worktree: entry.worktree.clone(),
                            server,
                            root,
                        },
                        generation: entry.generation,
                    },
                    entry.uri.clone(),
                ))
            })
            .collect()
    }
}

#[derive(Default)]
struct State {
    projects: HashMap<String, ProjectDiags>,
}

/// 도는 pull 하나(같은 출처 · 문서의 새 pull 이 취소한다).
struct Pull {
    seq: u64,
    cancel: CancellationToken,
}

struct Inner {
    manager: LspManager,
    state: Mutex<State>,
    pulls: Mutex<HashMap<(InstanceKey, u64, String), Pull>>,
    next_pull: Mutex<u64>,
    /// 도는 refresh(출처 → 당기는 동안 또 왔는가).
    refreshing: Mutex<HashMap<(InstanceKey, u64), bool>>,
    permits: Arc<Semaphore>,
}

/// 진단 저장소. 복제는 같은 저장소를 가리킨다.
#[derive(Clone)]
pub struct DiagnosticStore {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 워크트리 글을 견줄 꼴로(끝의 `/` 를 뗀다).
fn normalize_worktree(worktree: Option<&str>) -> Option<String> {
    worktree.map(|w| {
        let trimmed = w.trim_end_matches('/');
        if trimmed.is_empty() { w } else { trimmed }.to_owned()
    })
}

/// 경로의 `.` · `..` 를 글자로 접는다(디스크를 보지 않는다 — 링크는 풀지 않는다).
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// 바이트 상한까지 글자 경계에서 자른다(넘으면 `…`).
fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn position(value: &Value) -> LspPosition {
    let n = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_u64)
            .map_or(0, |v| u32::try_from(v).unwrap_or(u32::MAX))
    };
    LspPosition {
        line: n("line"),
        character: n("character"),
    }
}

fn range(value: &Value) -> LspRange {
    let start = position(&value["start"]);
    let end = position(&value["end"]);
    // 거꾸로 온 범위는 바로 세운다.
    if at(end) < at(start) {
        LspRange {
            start: end,
            end: start,
        }
    } else {
        LspRange { start, end }
    }
}

/// 서버가 보낸 진단 하나를 상한 안의 꼴로. 범위가 없으면 버린다.
fn diagnostic(
    server: &str,
    origin: Origin,
    version: Option<i64>,
    raw: &Value,
) -> Option<LspDiagnostic> {
    let range_value = raw.get("range")?;
    let message = raw.get("message").and_then(Value::as_str).unwrap_or("");
    let code = match raw.get("code") {
        Some(Value::String(s)) => Some(clip(s, SHORT_LIMIT)),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    let code_href = raw
        .pointer("/codeDescription/href")
        .and_then(Value::as_str)
        .filter(|h| {
            h.len() <= HREF_LIMIT && (h.starts_with("https://") || h.starts_with("http://"))
        })
        .map(str::to_owned);
    let tags = raw
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|t| match t.as_u64() {
                    Some(1) => Some(LspDiagnosticTag::Unnecessary),
                    Some(2) => Some(LspDiagnosticTag::Deprecated),
                    _ => None,
                })
                .take(2)
                .collect()
        })
        .unwrap_or_default();
    let related: Vec<LspRelatedInformation> = raw
        .get("relatedInformation")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(RELATED_LIMIT)
                .filter_map(|item| {
                    // 자른 URI 는 다른 자리를 가리킨다 — 긴 것은 버린다(경로도 이 URI 에서 나오므로 그 길이 안이다).
                    let uri = item
                        .pointer("/location/uri")?
                        .as_str()
                        .filter(|u| u.len() <= HREF_LIMIT)?;
                    Some(LspRelatedInformation {
                        path: uri_path(uri).map(|p| p.to_string_lossy().into_owned()),
                        uri: uri.to_owned(),
                        range: range(item.pointer("/location/range")?),
                        message: clip(
                            item.get("message").and_then(Value::as_str).unwrap_or(""),
                            RELATED_MESSAGE_LIMIT,
                        ),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let data = raw
        .get("data")
        .filter(|d| serde_json::to_vec(d).is_ok_and(|v| v.len() <= DATA_LIMIT))
        .cloned();
    let mut diagnostic = LspDiagnostic {
        server: server.to_owned(),
        origin,
        version: version.and_then(|v| i32::try_from(v).ok()),
        range: range(range_value),
        severity: LspSeverity::from_lsp(raw.get("severity")),
        message: clip(message, MESSAGE_LIMIT),
        code,
        source: raw
            .get("source")
            .and_then(Value::as_str)
            .map(|s| clip(s, SHORT_LIMIT)),
        code_href,
        tags,
        related: Vec::new(),
        data,
    };
    // 진단 하나의 바이트 상한: 관련 정보를 뺀 크기에 관련 정보를 하나씩 더해 넘기 전까지만 둔다.
    let mut size = serialized_len(&diagnostic);
    for item in related {
        size += serialized_len(&item) + 1;
        if size > DIAGNOSTIC_LIMIT {
            break;
        }
        diagnostic.related.push(item);
    }
    Some(diagnostic)
}

fn serialized_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX / 2, |v| v.len())
}

/// 서버가 보낸 진단 목록을 상한 안의 꼴로: 읽을 수 있는 것만, 심각도 · 위치 차례, [`FILE_LIMIT`] 까지. (목록, 잘렸는가).
fn diagnostics(
    server: &str,
    origin: Origin,
    version: Option<i64>,
    raw: &Value,
) -> (Vec<LspDiagnostic>, bool) {
    let Some(items) = raw.as_array() else {
        return (Vec::new(), false);
    };
    // 상한을 크게 넘는 목록은 앞부분만 읽는다(심각도 순으로 고르려고 상한의 몇 배까지는 읽는다).
    let mut out: Vec<LspDiagnostic> = items
        .iter()
        .take(FILE_LIMIT * 4)
        .filter_map(|d| diagnostic(server, origin, version, d))
        .collect();
    out.sort_by_key(order);
    let truncated = items.len() > FILE_LIMIT * 4 || out.len() > FILE_LIMIT;
    out.truncate(FILE_LIMIT);
    (out, truncated)
}

impl DiagnosticStore {
    /// 저장소를 만들고 관리자에 받는 쪽을 단다(Host 에 하나 — [`register`] 가 부른다).
    pub fn attach(manager: &LspManager) -> Self {
        let store = Self {
            inner: Arc::new(Inner {
                manager: manager.clone(),
                state: Mutex::new(State::default()),
                pulls: Mutex::new(HashMap::new()),
                next_pull: Mutex::new(0),
                refreshing: Mutex::new(HashMap::new()),
                permits: Arc::new(Semaphore::new(PULL_CONCURRENCY)),
            }),
        };
        // 관리자가 이 저장소를 쥐므로(받는 쪽) 저장소는 약하게 가리킨다.
        let weak = Arc::downgrade(&store.inner);
        manager.on_notification("textDocument/publishDiagnostics", {
            let weak = weak.clone();
            move |source, params| {
                if let Some(inner) = weak.upgrade() {
                    Self { inner }.published(source, params);
                }
            }
        });
        manager.on_notification("workspace/diagnostic/refresh", {
            let weak = weak.clone();
            move |source, _| {
                if let Some(inner) = weak.upgrade() {
                    Self { inner }.refresh(source);
                }
            }
        });
        manager.on_synced({
            let weak = weak.clone();
            // 관리자 잠금 안 — 태스크만 띄운다(관리자의 읽기 함수는 그 태스크에서).
            move |source, uri, version| {
                if let Some(inner) = weak.upgrade() {
                    drop(Self { inner }.schedule_pull(source.clone(), uri.to_owned(), version));
                }
            }
        });
        manager.on_status(move |project_id| {
            if let Some(inner) = weak.upgrade() {
                Self { inner }.prune(project_id);
            }
        });
        store
    }

    // ---- push ----------------------------------------------------------------------------------------------------------------

    /// `textDocument/publishDiagnostics`.
    fn published(&self, source: &NotificationSource, params: &Value) {
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return;
        };
        let version = params.get("version").and_then(Value::as_i64);
        let raw = params.get("diagnostics").unwrap_or(&Value::Null);
        self.put(source, uri, Origin::Push, version, raw, None);
    }

    /// 출처 · 문서의 진단을 바꾼다. 지난 세대 · 지난 버전 · 루트 밖은 버린다.
    fn put(
        &self,
        source: &NotificationSource,
        uri: &str,
        origin: Origin,
        version: Option<i64>,
        raw: &Value,
        result_id: Option<String>,
    ) {
        // 디코드한 뒤 `..` 를 글자로 접고 견준다 — `..%2F` 는 URI 로는 한 조각이라 url 크레이트가 접지 않는다(M4-06 리뷰 L2).
        let Some(path) = uri_path(uri)
            .map(|p| lexical(&p))
            .filter(|p| p.starts_with(&source.key.root))
        else {
            return;
        };
        // 서버마다 URI 의 인코딩이 조금씩 다르다 — 관리자가 쥔 꼴(`file_uri`)로 맞춰 견준다.
        let Some(uri) = file_uri(&path) else {
            return;
        };
        let uri = uri.as_str();
        let path = path.to_string_lossy().into_owned();
        let (items, truncated) = diagnostics(&source.key.server, origin, version, raw);
        let refills = {
            let mut state = lock(&self.inner.state);
            // 잠금 차례: 저장소 → 관리자(머리 주석). 판정과 넣기가 한 잠금 안이라 지우기(`prune`)와 엇갈리지 않는다.
            if !self.inner.manager.is_current(source) {
                return;
            }
            if let Some(v) = version
                && self
                    .inner
                    .manager
                    .synced_version(source, uri)
                    .is_some_and(|sent| v < sent)
            {
                return;
            }
            let project = state
                .projects
                .entry(source.key.project_id.clone())
                .or_default();
            let key: EntryKey = (source.key.server.clone(), source.key.root.clone(), origin);
            let file = project.files.entry(path.clone()).or_default();
            let old = file.entries.remove(&key).map_or(0, |e| e.items.len());
            project.total -= old;
            let mut items = items;
            let mut truncated = truncated;
            let room = PROJECT_LIMIT.saturating_sub(project.total);
            let slot = (path.clone(), key.clone());
            project.cut_pulls.remove(&slot);
            if items.len() > room {
                items.truncate(room);
                truncated = true;
                if origin == Origin::Pull {
                    project.cut_pulls.insert(slot);
                }
            }
            project.total += items.len();
            if items.is_empty() && !truncated {
                if file.entries.is_empty() {
                    project.files.remove(&path);
                }
            } else {
                file.entries.insert(
                    key,
                    Entry {
                        generation: source.generation,
                        worktree: normalize_worktree(source.key.worktree.as_deref()),
                        uri: uri.to_owned(),
                        items,
                        truncated,
                        result_id,
                    },
                );
            }
            let event = event_of(&source.key.project_id, &path, project.files.get(&path))
                .unwrap_or_else(|| LspDiagnosticsEvent {
                    project_id: source.key.project_id.clone(),
                    worktree: normalize_worktree(source.key.worktree.as_deref()),
                    path: path.clone(),
                    diagnostics: Vec::new(),
                    truncated: false,
                });
            let refills = project.take_refills(&source.key.project_id);
            // 잠금 안에서 낸다 — 같은 경로의 이벤트가 상태가 바뀐 차례대로 나간다(잠금 밖이면 늦게 나간 옛 이벤트가 웹뷰의 마지막
            // 상태가 될 수 있다 — M4-06 리뷰 L1). 버스는 자기 잠금만 쓰고 이 저장소 · 관리자를 부르지 않는다.
            self.emit(&state, &[event]);
            refills
        };
        self.refill(refills);
    }

    /// 상한에 잘렸던 pull 을 다시 당긴다(그 문서가 아직 그 서버에 열려 있을 때만 — 동시 상한은 여느 pull 과 같다).
    fn refill(&self, refills: Vec<(NotificationSource, String)>) {
        for (source, uri) in refills {
            if let Some(version) = self.inner.manager.synced_version(&source, &uri) {
                drop(self.schedule_pull(source, uri, version));
            }
        }
    }

    // ---- pull ----------------------------------------------------------------------------------------------------------------

    /// 문서 텍스트를 서버에 보냈다(관리자 잠금 안): 그 문서를 당기는 태스크를 띄운다. 같은 출처 · 문서의 앞 pull 은 취소한다.
    fn schedule_pull(
        &self,
        source: NotificationSource,
        uri: String,
        version: i64,
    ) -> tokio::task::JoinHandle<()> {
        let cancel = CancellationToken::new();
        let seq = {
            let mut next = lock(&self.inner.next_pull);
            *next += 1;
            *next
        };
        let slot = (source.key.clone(), source.generation, uri.clone());
        if let Some(old) = lock(&self.inner.pulls).insert(
            slot.clone(),
            Pull {
                seq,
                cancel: cancel.clone(),
            },
        ) {
            old.cancel.cancel();
        }
        let this = self.clone();
        tokio::spawn(async move {
            this.pull(&source, &uri, version, &cancel).await;
            let mut pulls = lock(&this.inner.pulls);
            if pulls.get(&slot).is_some_and(|p| p.seq == seq) {
                pulls.remove(&slot);
            }
        })
    }

    async fn pull(
        &self,
        source: &NotificationSource,
        uri: &str,
        version: i64,
        cancel: &CancellationToken,
    ) {
        let permit = tokio::select! {
            permit = Arc::clone(&self.inner.permits).acquire_owned() => permit,
            () = cancel.cancelled() => return,
        };
        let Ok(_permit) = permit else {
            return;
        };
        let Some(server) = self.inner.manager.running_server(source) else {
            return;
        };
        if !server.supports("textDocument/diagnostic") {
            return;
        }
        let previous = self.result_id(source, uri);
        let mut params = json!({ "textDocument": { "uri": uri } });
        if let Some(identifier) = server
            .capabilities_json()
            .pointer("/diagnosticProvider/identifier")
            .and_then(Value::as_str)
        {
            params["identifier"] = Value::String(identifier.to_owned());
        }
        if let Some(previous) = previous {
            params["previousResultId"] = Value::String(previous);
        }
        let answer = server
            .request_raw(
                "textDocument/diagnostic",
                params,
                RequestOptions {
                    timeout: crate::server::DEFAULT_REQUEST_TIMEOUT,
                    cancel: cancel.clone(),
                },
            )
            .await;
        let report = match answer {
            Ok(report) => report,
            Err(RequestError::Cancelled) => return,
            Err(RequestError::Server(e))
                if e.code == lsp_code::SERVER_CANCELLED || e.code == lsp_code::CONTENT_MODIFIED =>
            {
                // 서버가 아직 준비되지 않았다(rust-analyzer 의 색인 중) — 준비되면 refresh 를 보낸다. 앞 진단은 그대로 둔다.
                tracing::debug!(target: "pw_lsp", server = %source.key.server, code = e.code, "diagnostic pull deferred by the server");
                return;
            }
            Err(e) => {
                tracing::debug!(target: "pw_lsp", server = %source.key.server, %e, "diagnostic pull failed");
                return;
            }
        };
        // 그사이 문서가 바뀌었으면(새 pull 이 간다) 이 답은 지난 텍스트 기준이다.
        if cancel.is_cancelled() || self.inner.manager.synced_version(source, uri) != Some(version)
        {
            return;
        }
        let result_id = report
            .get("resultId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match report.get("kind").and_then(Value::as_str) {
            Some("unchanged") => self.unchanged(source, uri, version, result_id),
            _ => self.put(
                source,
                uri,
                Origin::Pull,
                Some(version),
                report.get("items").unwrap_or(&Value::Null),
                result_id,
            ),
        }
    }

    /// 앞 pull 의 `resultId`.
    fn result_id(&self, source: &NotificationSource, uri: &str) -> Option<String> {
        let path = uri_path(uri)?.to_string_lossy().into_owned();
        let state = lock(&self.inner.state);
        let entry = state
            .projects
            .get(&source.key.project_id)?
            .files
            .get(&path)?
            .entries
            .get(&(
                source.key.server.clone(),
                source.key.root.clone(),
                Origin::Pull,
            ))?;
        (entry.generation == source.generation)
            .then(|| entry.result_id.clone())
            .flatten()
    }

    /// pull 이 「바뀌지 않음」 으로 답했다: 진단은 그대로, 버전 · `resultId` 만 새로.
    fn unchanged(
        &self,
        source: &NotificationSource,
        uri: &str,
        version: i64,
        result_id: Option<String>,
    ) {
        let Some(path) = uri_path(uri).map(|p| p.to_string_lossy().into_owned()) else {
            return;
        };
        {
            let mut state = lock(&self.inner.state);
            if !self.inner.manager.is_current(source) {
                return;
            }
            let Some(project) = state.projects.get_mut(&source.key.project_id) else {
                return;
            };
            let Some(file) = project.files.get_mut(&path) else {
                return;
            };
            let Some(entry) = file.entries.get_mut(&(
                source.key.server.clone(),
                source.key.root.clone(),
                Origin::Pull,
            )) else {
                return;
            };
            entry.result_id = result_id;
            for item in &mut entry.items {
                item.version = i32::try_from(version).ok();
            }
            if let Some(event) = event_of(&source.key.project_id, &path, Some(file)) {
                self.emit(&state, &[event]);
            }
        }
    }

    /// `workspace/diagnostic/refresh`: 그 서버에 열린 문서를 모두 다시 당긴다. 몰려 와도 합친다 — [`REFRESH_SETTLE`] 동안 모은 뒤 한
    /// 번 당기고, 당기는 동안 온 refresh 는 그 당기기가 끝난 뒤 한 번으로(출처마다 도는 당기기는 하나다).
    fn refresh(&self, source: &NotificationSource) {
        let slot = (source.key.clone(), source.generation);
        {
            let mut refreshing = lock(&self.inner.refreshing);
            if let Some(again) = refreshing.get_mut(&slot) {
                *again = true;
                return;
            }
            refreshing.insert(slot.clone(), false);
        }
        let this = self.clone();
        let source = source.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REFRESH_SETTLE).await;
                // 모으는 동안 온 것은 이번 당기기에 든다.
                if let Some(again) = lock(&this.inner.refreshing).get_mut(&slot) {
                    *again = false;
                }
                let pulls: Vec<_> = this
                    .inner
                    .manager
                    .synced_documents(&source)
                    .into_iter()
                    .map(|(uri, version)| this.schedule_pull(source.clone(), uri, version))
                    .collect();
                for pull in pulls {
                    let _ = pull.await;
                }
                let mut refreshing = lock(&this.inner.refreshing);
                if refreshing.get(&slot) == Some(&true) {
                    continue;
                }
                refreshing.remove(&slot);
                break;
            }
        });
    }

    // ---- 지우기 --------------------------------------------------------------------------------------------------------------

    /// 상태가 바뀌었다(관리자 잠금 밖): 지금 세대가 아닌 출처의 진단, 서버에 더는 열려 있지 않은 문서의 pull 진단을 지운다.
    fn prune(&self, project_id: &str) {
        let refills = {
            let mut state = lock(&self.inner.state);
            let Some(project) = state.projects.get_mut(project_id) else {
                return;
            };
            // 출처마다 한 번만 묻는다(파일이 많아도).
            let mut judged: HashMap<Judged, Option<HashSet<String>>> = HashMap::new();
            let manager = &self.inner.manager;
            let mut changed = Vec::new();
            let mut removed = 0;
            for (path, file) in &mut project.files {
                let before = file.entries.len();
                file.entries.retain(|(server, root, origin), entry| {
                    let open = judged
                        .entry((
                            server.clone(),
                            root.clone(),
                            entry.worktree.clone(),
                            entry.generation,
                        ))
                        .or_insert_with(|| {
                            let source = NotificationSource {
                                key: InstanceKey {
                                    project_id: project_id.to_owned(),
                                    worktree: entry.worktree.clone(),
                                    server: server.clone(),
                                    root: root.clone(),
                                },
                                generation: entry.generation,
                            };
                            manager.is_current(&source).then(|| {
                                manager
                                    .synced_documents(&source)
                                    .into_iter()
                                    .map(|(uri, _)| uri)
                                    .collect()
                            })
                        });
                    let keep = match open {
                        None => false,
                        Some(open) => *origin == Origin::Push || open.contains(&entry.uri),
                    };
                    if !keep {
                        removed += entry.items.len();
                    }
                    keep
                });
                if file.entries.len() != before {
                    changed.push(path.clone());
                }
            }
            project.total -= removed;
            let files = &project.files;
            project
                .cut_pulls
                .retain(|(path, key)| files.get(path).is_some_and(|f| f.entries.contains_key(key)));
            let mut events = Vec::new();
            for path in changed {
                let file = project.files.get(&path);
                let event = event_of(project_id, &path, file.filter(|f| !f.entries.is_empty()))
                    .unwrap_or_else(|| LspDiagnosticsEvent {
                        project_id: project_id.to_owned(),
                        worktree: file.and_then(FileDiags::worktree),
                        path: path.clone(),
                        diagnostics: Vec::new(),
                        truncated: false,
                    });
                if file.is_some_and(|f| f.entries.is_empty()) {
                    project.files.remove(&path);
                }
                events.push(event);
            }
            let refills = project.take_refills(project_id);
            self.emit(&state, &events);
            refills
        };
        self.refill(refills);
    }

    /// 이벤트를 낸다. 저장소 잠금을 쥔 채로만 부른다(`_held` — 차례가 상태가 바뀐 차례와 같게).
    fn emit(&self, _held: &MutexGuard<'_, State>, events: &[LspDiagnosticsEvent]) {
        let Some(bus) = self.inner.manager.bus() else {
            return;
        };
        for event in events {
            if let Err(e) = bus.publish(
                Scope::Project(event.project_id.clone()),
                TOPIC_DIAGNOSTICS,
                event,
            ) {
                tracing::warn!(target: "pw_lsp", %e, "cannot publish lsp.diagnostics");
            }
        }
    }

    // ---- 읽기 ----------------------------------------------------------------------------------------------------------------

    /// `lsp.diagnostics.list`.
    pub fn list(&self, params: &LspDiagnosticsListParams) -> Result<LspDiagnosticsList, RpcError> {
        let limit = params.limit.clamp(1, LIST_LIMIT) as usize;
        let (after_path, skip) = match &params.cursor {
            None => (None, 0),
            Some(cursor) => {
                let (path, skip): (String, usize) = serde_json::from_str(cursor)
                    .map_err(|_| RpcError::invalid_params("unreadable cursor"))?;
                (Some(path), skip)
            }
        };
        let worktree = normalize_worktree(params.worktree.as_deref());
        let state = lock(&self.inner.state);
        let Some(project) = state.projects.get(&params.project_id) else {
            return Ok(LspDiagnosticsList {
                files: Vec::new(),
                next_cursor: None,
                truncated: false,
            });
        };
        let project_truncated = project
            .files
            .values()
            .any(|f| f.entries.values().any(|e| e.truncated));
        let mut files = Vec::new();
        let mut taken = 0;
        let mut next_cursor = None;
        let start = after_path
            .clone()
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Included);
        for (path, file) in project
            .files
            .range::<String, _>((start, std::ops::Bound::Unbounded))
        {
            if params.path.as_ref().is_some_and(|p| p != path) || file.worktree() != worktree {
                continue;
            }
            let (mut items, truncated) = file.merged();
            if let Some(min) = params.min_severity {
                items.retain(|d| d.severity <= min);
            }
            let from = if after_path.as_ref() == Some(path) {
                skip
            } else {
                0
            };
            // 진단이 남지 않은 파일은 건너뛰되, 상한에 잘려 0개인 파일은 싣는다 — 받는 쪽(문제 툴 윈도우)은 파일마다의
            // `truncated` 로 머리 안내를 매번 계산한다(M4-06 리뷰 L4).
            if from >= items.len() && !(from == 0 && truncated) {
                continue;
            }
            let room = limit - taken;
            let end = (from + room).min(items.len());
            taken += end - from;
            files.push(LspDiagnosticsFile {
                path: path.clone(),
                diagnostics: items[from..end].to_vec(),
                truncated,
            });
            if end < items.len() {
                next_cursor = Some(serde_json::to_string(&(path, end)).unwrap_or_default());
                break;
            }
            if taken == limit {
                next_cursor = Some(serde_json::to_string(&(path, end)).unwrap_or_default());
                break;
            }
        }
        Ok(LspDiagnosticsList {
            files,
            next_cursor,
            truncated: project_truncated,
        })
    }

    /// `lsp.diagnostics.counts`.
    pub fn counts(&self, project_id: &str, worktree: Option<&str>) -> LspDiagnosticsCounts {
        let worktree = normalize_worktree(worktree);
        let state = lock(&self.inner.state);
        let mut counts = [0u32; 4];
        let mut truncated = false;
        if let Some(project) = state.projects.get(project_id) {
            for file in project.files.values() {
                if file.worktree() != worktree {
                    continue;
                }
                let (items, cut) = file.merged();
                truncated |= cut;
                for d in &items {
                    counts[d.severity.index()] += 1;
                }
            }
        }
        LspDiagnosticsCounts {
            error: counts[0],
            warning: counts[1],
            information: counts[2],
            hint: counts[3],
            truncated,
        }
    }

    /// 파일 `path` 의 진단 중 `range` 와 겹치는 것(코드 액션 요청의 문맥 — M4-12). 끝이 맞닿은 것도 겹친다고 본다(커서 자리).
    pub fn in_range(&self, project_id: &str, path: &Path, range: LspRange) -> Vec<LspDiagnostic> {
        let path = path.to_string_lossy();
        let state = lock(&self.inner.state);
        state
            .projects
            .get(project_id)
            .and_then(|p| p.files.get(path.as_ref()))
            .map(|f| {
                f.merged()
                    .0
                    .into_iter()
                    .filter(|d| {
                        at(d.range.start) <= at(range.end) && at(range.start) <= at(d.range.end)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn event_of(project_id: &str, path: &str, file: Option<&FileDiags>) -> Option<LspDiagnosticsEvent> {
    let file = file?;
    let (diagnostics, truncated) = file.merged();
    Some(LspDiagnosticsEvent {
        project_id: project_id.to_owned(),
        worktree: file.worktree(),
        path: path.to_owned(),
        diagnostics,
        truncated,
    })
}

/// 저장소를 만들어 붙이고 RPC 를 단다(`crate::register` 가 부른다). 저장소를 돌려준다 — M4-12 가 코드 액션의 문맥을 읽는다.
pub fn register(router: &mut Router, manager: &LspManager) -> DiagnosticStore {
    let store = DiagnosticStore::attach(manager);
    let s = store.clone();
    router.unary(
        LspDiagnosticsListGet,
        move |ctx, params: LspDiagnosticsListParams| {
            let s = s.clone();
            async move {
                // 그 프로젝트 창의 연결만 — 진단에는 파일 경로가 든다. 미신뢰여도 읽는다(서버가 없어 빈 목록).
                exec_grant(ctx.scope(), &params.project_id, &Launch::Process)?;
                s.list(&params)
            }
        },
    );
    let s = store.clone();
    router.unary(
        LspDiagnosticsCountsGet,
        move |ctx, params: LspDiagnosticsCountsParams| {
            let s = s.clone();
            async move {
                exec_grant(ctx.scope(), &params.project_id, &Launch::Process)?;
                Ok(s.counts(&params.project_id, params.worktree.as_deref()))
            }
        },
    );
    store
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagnostic_is_read_within_its_limits() {
        let raw = json!({
            "range": { "start": { "line": 3, "character": 9 }, "end": { "line": 3, "character": 2 } },
            "severity": 2,
            "code": 2322,
            "source": "s".repeat(SHORT_LIMIT + 10),
            "message": "가".repeat(MESSAGE_LIMIT),
            "codeDescription": { "href": "javascript:alert(1)" },
            "tags": [1, 2, 7],
            "relatedInformation": (0..40).map(|i| json!({
                "location": { "uri": "file:///x/a.rs", "range": { "start": { "line": i, "character": 0 }, "end": { "line": i, "character": 1 } } },
                "message": "m",
            })).collect::<Vec<_>>(),
            "data": { "big": "x".repeat(DATA_LIMIT) },
        });
        let d = diagnostic("s1", Origin::Push, Some(4), &raw).unwrap();
        assert_eq!(d.severity, LspSeverity::Warning);
        assert_eq!(d.code.as_deref(), Some("2322"));
        // 거꾸로 온 범위는 바로 선다.
        assert_eq!((d.range.start.character, d.range.end.character), (2, 9));
        assert!(d.message.len() <= MESSAGE_LIMIT && d.message.ends_with('…'));
        assert!(d.source.unwrap().len() <= SHORT_LIMIT);
        assert_eq!(d.code_href, None);
        assert_eq!(
            d.tags,
            [LspDiagnosticTag::Unnecessary, LspDiagnosticTag::Deprecated]
        );
        assert_eq!(d.related.len(), RELATED_LIMIT);
        assert_eq!(d.related[0].path.as_deref(), Some("/x/a.rs"));
        assert_eq!(d.data, None);
        // 심각도가 없으면 오류, 범위가 없으면 버린다.
        assert_eq!(
            diagnostic(
                "s1",
                Origin::Push,
                None,
                &json!({ "range": raw["range"], "message": "m" })
            )
            .unwrap()
            .severity,
            LspSeverity::Error
        );
        assert!(diagnostic("s1", Origin::Push, None, &json!({ "message": "m" })).is_none());
    }

    #[test]
    fn a_diagnostic_stays_within_its_byte_limit_with_related_paths() {
        let line = |i: u32| json!({ "start": { "line": i, "character": 0 }, "end": { "line": i, "character": 1 } });
        // 모든 칸을 상한까지 채우고, 관련 정보도 상한 길이의 URI · 메시지로 채운다. 맨 앞에는 아주 긴 URI(틀 상한 안 —
        // 자르기 전 URI 에서 경로를 만들면 그 길이 그대로 실린다).
        let long_dir = "d".repeat(64 * 1024);
        let mut related = vec![json!({
            "location": { "uri": format!("file:///{long_dir}/x.rs"), "range": line(0) },
            "message": "too long",
        })];
        let fit_dir = "p".repeat(HREF_LIMIT - "file:///".len() - "/x.rs".len());
        related.extend((0..40).map(|i| {
            json!({
                "location": { "uri": format!("file:///{fit_dir}/x.rs"), "range": line(i) },
                "message": "m".repeat(RELATED_MESSAGE_LIMIT),
            })
        }));
        let raw = json!({
            "range": line(1),
            "message": "가".repeat(MESSAGE_LIMIT),
            "code": "c".repeat(SHORT_LIMIT),
            "source": "s".repeat(SHORT_LIMIT),
            "codeDescription": { "href": format!("https://e.x/{}", "h".repeat(HREF_LIMIT - 12)) },
            "relatedInformation": related,
            "data": { "d": "x".repeat(DATA_LIMIT - 16) },
        });
        let d = diagnostic("s1", Origin::Push, Some(1), &raw).unwrap();
        assert!(d.code_href.is_some() && d.data.is_some());
        let size = serde_json::to_vec(&d).unwrap().len();
        assert!(size <= DIAGNOSTIC_LIMIT, "{size}");
        // 긴 URI 의 관련 정보는 버리고, 나머지는 상한 안에서 앞에서부터 둔다.
        assert!(!d.related.is_empty());
        assert!(d.related.len() < RELATED_LIMIT);
        assert!(
            d.related
                .iter()
                .all(|r| r.message.len() == RELATED_MESSAGE_LIMIT)
        );
        assert!(
            d.related
                .iter()
                .all(|r| r.uri.len() <= HREF_LIMIT && r.path.as_ref().unwrap().len() <= HREF_LIMIT)
        );
    }

    #[test]
    fn a_flood_keeps_the_most_severe_up_to_the_file_limit() {
        let at = |line: u32, severity: u64| {
            json!({
                "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 1 } },
                "severity": severity,
                "message": "m",
            })
        };
        let mut raw: Vec<Value> = (0..3_000).map(|i| at(i, 4)).collect();
        raw.push(at(5_000, 1));
        let (items, truncated) = diagnostics("s", Origin::Push, None, &Value::Array(raw));
        assert!(truncated);
        assert_eq!(items.len(), FILE_LIMIT);
        assert_eq!(items[0].severity, LspSeverity::Error);
    }
}
