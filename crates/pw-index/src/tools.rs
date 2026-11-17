//! 코드 인텔리전스 MCP 툴 `symbol_search`(`docs/design/16-m5-plan.md` M5-08 ②, `docs/design/05-mcp.md` 「코드 인텔리전스」). `read`,
//! 그룹 `code`. 짝인 `diagnostics` 는 `pw_lsp::tools` 에 있다.
//!
//! 툴 메서드 규약(`crates/pw-mcp` 머리 주석)대로 UI 의 RPC `index.symbols.search` 와 **같은 함수**([`SymbolIndex::search`])를 부르는
//! 얇은 선언이다. 화면용 칸(맞은 글자 위치 · 점수)은 빼고, 위치는 줄 · 열 1 부터다(`diagnostics` 와 같다 — 열은 UTF-16 코드 단위).
//!
//! - **관문**: `agent_grant`(MCP 세션의 연결 범위 · 레지스트리에 있는 프로젝트, `pw_core::registry::agent`). 없는 프로젝트는 그 관문의
//!   오류(invalid params) 그대로다.
//! - **색인이 있는 곳**: 창이 `fs.watch` 로 보는 루트에만 있다(`crate` 머리 주석 「수명」). 툴은 색인을 만들지 않는다 — 그 프로젝트 창이
//!   열려 있지 않으면 `unindexed` 에 루트가 들고 `hint` 가 「Pitwall 에서 프로젝트를 열어라」 를 알린다.
//! - **신뢰**: 파싱만 하므로 미신뢰 프로젝트에서도 찾는다(15-m4-plan.md 결정 M4-D8).
//! - **쪽**: 툴 규약의 `cursor` · `limit` · `nextCursor`(16-m5-plan.md M5-08 ② — 메인 판정 2026-11-14). 커서는 불투명 글자이고 안에
//!   다음 쪽의 시작 자리(`index.symbols.search` 의 `offset`)가 든다. 세션의 응답 상한(256 KiB) 아래 [`RESPONSE_BUDGET`] 에서 먼저
//!   끊으면 그 자리부터 다음 쪽이다.

use pw_core::registry::agent::agent_grant;
use pw_rpc::{ConnectionScope, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::SymbolIndex;
use crate::kind::IndexSymbolKind;
use crate::methods::{IndexSymbolMatch, IndexSymbolSearchParams, MAX_LIMIT};

/// `limit` 이 비었을 때(16-m5-plan.md M5-08 ②).
pub const DEFAULT_LIMIT: u32 = 50;
/// 툴이 응답에 싣는 결과 JSON 의 예산(바이트). 세션의 상한(256 KiB)에 봉투 · 이스케이프의 여유를 남긴 값이다 — 데스크톱 크레이트의
/// 시험이 상한보다 작은지 본다.
pub const RESPONSE_BUDGET: usize = 192 * 1024;
/// 커서 글자의 머리(불투명 — 에이전트가 수로 읽지 않게).
const CURSOR_PREFIX: &str = "o:";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSearchToolParams {
    pub project_id: String,
    /// Top of the worktree to read (absolute path). Empty means the session's worktree, or else the project's main
    /// checkout. invalid params if it is not a worktree of the project.
    #[serde(default)]
    pub worktree: Option<String>,
    /// What to look for. Every whitespace-separated word must match the symbol name (each word fuzzily — it matches
    /// when its letters appear in that order; case-sensitive only when the word has an uppercase letter). Empty means
    /// every symbol in path · position order. Up to 256 characters.
    pub query: String,
    /// Only these kinds. Without it, every kind except `impl`.
    #[serde(default)]
    pub kinds: Option<Vec<IndexSymbolKind>>,
    /// `nextCursor` from the previous page. Empty means from the start.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Symbols per page, 1 – 1000 (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSearchToolResult {
    /// Best match first (ties by shorter name · path · position).
    pub symbols: Vec<SymbolSearchToolMatch>,
    /// Number of matching symbols (across all pages).
    #[specta(type = pw_rpc::JsNumber)]
    pub matched: u64,
    /// The `cursor` for the next page. `null` on the last page.
    pub next_cursor: Option<String>,
    /// Roots not searched because they have no index (absolute paths). If not empty, results may be incomplete — see
    /// `hint`.
    pub unindexed: Vec<String>,
    /// What to do when `unindexed` is not empty (one line of English).
    pub hint: Option<String>,
}

/// One matching symbol. Lines and columns are from 1; columns are in UTF-16 code units.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SymbolSearchToolMatch {
    pub name: String,
    pub kind: IndexSymbolKind,
    /// Name of the enclosing symbol (the class of a method in a class, etc.).
    pub container: Option<String>,
    /// Absolute path of the file.
    pub path: String,
    /// Path relative to the project root (`/`-separated).
    pub relative: String,
    /// Language id (`rust` · `typescript` …).
    pub language: String,
    /// Line and column of the name.
    pub line: u32,
    pub column: u32,
    /// First and last line of the whole definition.
    pub start_line: u32,
    pub end_line: u32,
}

rpc_method! {
    /// Find symbols (functions, types, methods, fields, constants …) in a project by name. `query` is space-separated words
    /// that must all match the symbol name, each fuzzily (its letters in order; case-sensitive only if the word has an
    /// uppercase letter); results are best match first. Narrow with `kinds`. Lines and columns start at 1; columns count
    /// UTF-16 code units. The index is a tree-sitter parse of the project files and exists only while the project is open
    /// in a Pitwall window: roots without one are listed in `unindexed` with a `hint` — open the project in Pitwall and call
    /// again. Works in untrusted projects (nothing is run).
    pub SymbolSearchTool: "index.tool.symbolSearch" {
        params: SymbolSearchToolParams,
        result: SymbolSearchToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "code", name: "symbol_search" },
    }
}

/// 툴 메서드의 핸들러를 붙인다(`crate::methods::register` 가 부른다 — 앱의 Host 만 심볼 인덱스를 붙인다).
pub fn register(router: &mut Router, index: SymbolIndex) {
    router.unary(SymbolSearchTool, move |ctx, params| {
        let (index, scope) = (index.clone(), ctx.scope().clone());
        async move { call(&index, scope, params).await }
    });
}

async fn call(
    index: &SymbolIndex,
    scope: ConnectionScope,
    params: SymbolSearchToolParams,
) -> Result<SymbolSearchToolResult, RpcError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(RpcError::invalid_params(format!(
            "`limit` must be 1 to {MAX_LIMIT}"
        )));
    }
    let offset = params.cursor.as_deref().map(parse_cursor).transpose()?;
    let fs = std::sync::Arc::clone(&index.inner.fs);
    let project_id = params.project_id.clone();
    crate::blocking(move || agent_grant(&scope, fs.projects()?, &project_id)).await?;
    let offset = offset.unwrap_or(0);
    let found = index
        .search(IndexSymbolSearchParams {
            project_id: params.project_id,
            worktree: params.worktree,
            query: params.query,
            kinds: params.kinds,
            limit: Some(limit),
            offset: Some(offset),
        })
        .await?;
    let mut budget = RESPONSE_BUDGET;
    let mut symbols = Vec::with_capacity(found.symbols.len());
    for m in found.symbols {
        let item = item(m);
        // 첫 심볼은 예산과 상관없이 싣는다 — 쪽이 비면 나아가지 못한다.
        if !take(&mut budget, &item) && !symbols.is_empty() {
            break;
        }
        symbols.push(item);
    }
    let next = u64::from(offset) + symbols.len() as u64;
    let next_cursor = (next < found.matched).then(|| format!("{CURSOR_PREFIX}{next}"));
    let hint = (!found.unindexed.is_empty()).then(|| {
        "the symbol index exists only while the project is open in a Pitwall window; open the project in Pitwall and call again"
            .to_owned()
    });
    Ok(SymbolSearchToolResult {
        symbols,
        matched: found.matched,
        next_cursor,
        unindexed: found.unindexed,
        hint,
    })
}

fn parse_cursor(cursor: &str) -> Result<u32, RpcError> {
    cursor
        .strip_prefix(CURSOR_PREFIX)
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| RpcError::invalid_params("unreadable cursor"))
}

/// `value` 를 직렬화한 크기(배열의 쉼표 포함)만큼 예산을 쓴다. 남은 예산에 들지 않으면 쓰지 않고 `false`.
fn take(budget: &mut usize, value: &impl Serialize) -> bool {
    let size = serde_json::to_vec(value).map_or(usize::MAX, |v| v.len() + 1);
    if size > *budget {
        return false;
    }
    *budget -= size;
    true
}

/// 화면용 결과(0 부터 · 일치 위치 · 점수) → 툴의 결과(1 부터).
fn item(m: IndexSymbolMatch) -> SymbolSearchToolMatch {
    SymbolSearchToolMatch {
        line: m.selection_range.start.line + 1,
        column: m.selection_range.start.character + 1,
        start_line: m.range.start.line + 1,
        end_line: m.range.end.line + 1,
        name: m.name,
        kind: m.kind,
        container: m.container,
        path: m.path,
        relative: m.relative,
        language: m.language,
    }
}
