//! RPC(tier Read): `index.symbols.search` · `index.file.symbols` · `index.status`. 입력은 struct, 결과는 쪽 단위다 — M5 의
//! `symbol_search` 툴(05-mcp.md)이 될 꼴이다(결정 M4-D21 — M4 에서는 `mcp: true` 를 달지 않는다).

use pw_rpc::{Router, rpc_method};
use serde::{Deserialize, Serialize};

use crate::SymbolIndex;
use crate::kind::IndexSymbolKind;

/// 결과 수의 기본값.
pub const DEFAULT_LIMIT: u32 = 100;
/// `limit` 의 상한.
pub const MAX_LIMIT: u32 = 1_000;
/// `offset` 의 상한(쪽을 넘기는 깊이).
pub const MAX_OFFSET: u32 = 100_000;
/// 질의의 최대 길이(글자). 매칭 비용이 질의 길이에 비례한다 — Readonly · Mobile 기기도 부르는 Read 메서드다.
pub const MAX_QUERY_CHARS: usize = 256;

/// 문서 안의 위치. 줄은 0 부터, `character` 는 줄 안의 UTF-16 코드 단위(LSP 의 기본 위치 인코딩과 같다).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexPosition {
    pub line: u32,
    pub character: u32,
}

/// `[start, end)` 범위(LSP `Range` 와 같은 꼴).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexRange {
    pub start: IndexPosition,
    pub end: IndexPosition,
}

impl IndexRange {
    pub(crate) fn from_span(span: [u32; 4]) -> Self {
        Self {
            start: IndexPosition {
                line: span[0],
                character: span[1],
            },
            end: IndexPosition {
                line: span[2],
                character: span[3],
            },
        }
    }
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexSymbolSearchParams {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기(`ActiveWorktree.top`). 있으면 그 워크트리 안의 프로젝트 루트를, 없으면 등록 루트를 찾는다
    /// (`fs.watch` · `search.files` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 찾을 것. 공백으로 가른 낱말이 모두 심볼 이름에 맞아야 한다(낱말마다 퍼지, 대소문자는 대문자가 있을 때만 가린다). 비어 있으면
    /// 경로 · 위치 순 앞에서부터. 글자 수 상한 [`MAX_QUERY_CHARS`].
    pub query: String,
    /// 이 종류만. 없으면 `impl` 을 뺀 모든 종류.
    #[serde(default)]
    pub kinds: Option<Vec<IndexSymbolKind>>,
    /// 결과 수. 기본 [`DEFAULT_LIMIT`], 1 ~ [`MAX_LIMIT`].
    #[serde(default)]
    pub limit: Option<u32>,
    /// 앞에서 건너뛸 수(쪽 넘기기). 기본 0, 상한 [`MAX_OFFSET`].
    #[serde(default)]
    pub offset: Option<u32>,
}

/// 맞는 심볼 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexSymbolMatch {
    pub name: String,
    pub kind: IndexSymbolKind,
    /// 바깥 심볼의 이름(클래스 안 메서드의 클래스 등). 없으면 `null`. 화면은 흐린 글자로 보인다.
    pub container: Option<String>,
    /// 파일의 절대 경로(루트 안 심링크를 지났으면 링크 쪽 경로 — `search.files` 와 같다).
    pub path: String,
    /// 그 루트(정규화한 실제 경로).
    pub root: String,
    /// 루트 기준 상대 경로(`/` 구분).
    pub relative: String,
    /// 언어 확장의 언어 id(`rust` · `typescript` …).
    pub language: String,
    /// 정의 전체.
    pub range: IndexRange,
    /// 이름. 고르면 이 자리로 간다.
    pub selection_range: IndexRange,
    /// `name` 안의 일치 글자 위치(UTF-16 코드 단위, 오름차순, 중복 없음).
    pub positions: Vec<u32>,
    /// 높을수록 위다. 같은 질의 안에서만 비교한다.
    pub score: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexSymbolSearch {
    /// 점수순(같으면 짧은 이름 · 경로 · 위치 순). `offset` 부터 `limit` 개.
    pub symbols: Vec<IndexSymbolMatch>,
    /// 맞은 심볼 수(`symbols` 는 그 `offset` 부터의 한 쪽).
    #[specta(type = pw_rpc::JsNumber)]
    pub matched: u64,
    /// 찾아본 심볼 수(종류로 거른 뒤).
    #[specta(type = pw_rpc::JsNumber)]
    pub indexed: u64,
    /// 색인이 없어 찾지 않은 루트(창이 감시하지 않는 루트 — 색인은 창이 `fs.watch` 로 보는 동안만 있다).
    pub unindexed: Vec<String>,
}

rpc_method! {
    /// 프로젝트의 심볼(함수 · 타입 · 메서드 …)을 이름으로 퍼지 매칭한다(심볼로 이동 · 클래스로 이동 · Search Everywhere 의 심볼 탭 —
    /// M4-16). 색인은 창이 `fs.watch` 로 보는 루트마다 메모리에 있고 파일 변경을 따라간다(`pw_index` 머리 주석). 처음 만드는 중이면 다
    /// 만들 때까지 기다리고, 그 작업이 취소되면 `CANCELLED` 다. 미신뢰 프로젝트에서도 돈다(코드를 돌리지 않는다 — 결정 M4-D8).
    pub IndexSymbolsSearchMethod: "index.symbols.search" {
        params: IndexSymbolSearchParams,
        result: IndexSymbolSearch,
        tier: Read,
    }
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexFileSymbolsParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 절대 경로. 프로젝트 루트(활성 워크트리 포함) 안이어야 한다.
    pub path: String,
    /// 저장하지 않은 문서 내용. 있으면 디스크 대신 이것을 파싱한다(편집 중 아웃라인 — M4-10). 1 MiB 를 넘으면 심볼이 없다.
    #[serde(default)]
    pub text: Option<String>,
}

/// 파일의 심볼 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexSymbol {
    pub name: String,
    pub kind: IndexSymbolKind,
    /// 바깥 심볼의 `symbols` 안 색인(앞쪽에 있다). 없으면 `null`. 이것으로 트리를 만든다(LSP `DocumentSymbol` 의 `children`).
    pub parent: Option<u32>,
    pub range: IndexRange,
    pub selection_range: IndexRange,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexFileSymbols {
    /// 언어 확장의 언어 id. 정의 쿼리가 있는 언어가 아니면 `null`(심볼도 없다).
    pub language: Option<String>,
    /// 문서 순서(정의 시작 위치, 같으면 바깥 것 먼저).
    pub symbols: Vec<IndexSymbol>,
}

rpc_method! {
    /// 파일 하나의 심볼(아웃라인 · 파일 구조 팝업의 대체 — 언어 서버가 없거나 준비 전 · 미신뢰일 때, M4-10). `text` 를 주면 그
    /// 내용을, 아니면 색인에 있는 것을(없으면 디스크에서 읽어) 쓴다.
    pub IndexFileSymbolsMethod: "index.file.symbols" {
        params: IndexFileSymbolsParams,
        result: IndexFileSymbols,
        tier: Read,
    }
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatusParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
}

/// 루트 하나의 색인 상태.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum IndexRootState {
    /// 창이 감시하지 않아 색인이 없다.
    None,
    /// 처음 만드는 중(작업 `task-title-indexing-symbols` 가 돈다). 요청은 다 만들 때까지 기다린다.
    Building,
    /// 다 만들었다(바뀐 파일을 다시 읽는 중일 수 있다 — 그동안은 옛 심볼로 답한다).
    Ready,
    /// 작업이 취소돼 버렸다. 다음 요청이 다시 만든다.
    Cancelled,
    /// 파일 목록 작업(`task-title-indexing-files`)이 취소돼 목록이 없다. 파일 목록을 다시 만들 때까지(파일 검색 `search.files` ·
    /// `rescan`) 이 루트는 찾지 않는다(`unindexed`).
    Suspended,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexRootStatus {
    pub root: String,
    pub state: IndexRootState,
    /// 심볼을 뽑은 파일 수(정의 쿼리가 있는 언어의 파일).
    #[specta(type = pw_rpc::JsNumber)]
    pub files: u64,
    #[specta(type = pw_rpc::JsNumber)]
    pub symbols: u64,
    /// 아직 다시 읽지 않은 파일 수.
    #[specta(type = pw_rpc::JsNumber)]
    pub pending: u64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub roots: Vec<IndexRootStatus>,
}

rpc_method! {
    /// 프로젝트의 루트마다 심볼 색인 상태. 기다리지 않는다.
    pub IndexStatusMethod: "index.status" {
        params: IndexStatusParams,
        result: IndexStatus,
        tier: Read,
    }
}

/// `index.*` 를 붙인다.
pub fn register(router: &mut Router, index: SymbolIndex) {
    let search = index.clone();
    router.unary(
        IndexSymbolsSearchMethod,
        move |_ctx, params: IndexSymbolSearchParams| {
            let index = search.clone();
            async move { index.search(params).await }
        },
    );
    let file = index.clone();
    router.unary(
        IndexFileSymbolsMethod,
        move |_ctx, params: IndexFileSymbolsParams| {
            let index = file.clone();
            async move { index.file_symbols(params).await }
        },
    );
    router.unary(IndexStatusMethod, move |_ctx, params: IndexStatusParams| {
        let index = index.clone();
        async move { index.status(params).await }
    });
}
