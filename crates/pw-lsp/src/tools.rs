//! 코드 인텔리전스 MCP 툴 `diagnostics`(`docs/design/16-m5-plan.md` M5-08 ①, `docs/design/05-mcp.md` 「코드 인텔리전스」). `read`,
//! 그룹 `code`. 짝인 `symbol_search` 는 `pw_index::tools` 에 있다.
//!
//! 툴 메서드 규약(`crates/pw-mcp` 머리 주석)대로 UI 의 RPC 와 **같은 함수**를 부르는 얇은 선언이다 — 진단은 `lsp.diagnostics.list`
//! 가 읽는 저장소([`DiagnosticStore::list`]), 서버 요약은 `lsp.status.get` 의 함수(`LspManager::status` 를 워크트리로 거른 것).
//!
//! - **관문**: `agent_grant`(MCP 세션의 연결 범위 · 레지스트리에 있는 프로젝트, `pw_core::registry::agent`). UI 의 두 RPC 가 지나는
//!   `exec_grant`(그 프로젝트 창의 연결만)는 빌리지 않는다 — MCP 세션은 그 관문을 지나지 못한다. 없는 프로젝트는 그 관문의 오류
//!   (invalid params) 그대로다.
//! - **서버를 띄우지 않는다**(결정 M5-D18 · D52 — 미신뢰 · 창 범위 규칙): 창이 띄운 서버가 보낸 진단을 읽기만 한다. 그 프로젝트 창이
//!   없으면 서버도 없어 빈 목록과 빈 서버 요약이다. 미신뢰 프로젝트는 서버가 뜨지 않으므로 빈 목록과 까닭(`unavailable`)이다.
//! - **위치**: 저장소는 LSP 그대로(줄 · UTF-16 열, 0 부터)다. 툴은 줄 · 열을 1 부터 준다 — 에이전트 · 편집기 · 컴파일러가 쓰는 꼴.
//!   열은 UTF-16 코드 단위 그대로다.
//! - **응답 크기**: 세션은 256 KiB 를 넘는 응답을 「범위를 좁혀라」 오류로 바꾼다(`pw_mcp::server::MAX_RESPONSE_BYTES`). 툴은 그
//!   아래 [`RESPONSE_BUDGET`] 안에서 자르고 다음 `cursor` 를 알린다. 서버만 되돌려 받는 값(`data`)과 받은 길(push · pull) · 문서
//!   버전은 싣지 않는다.

use std::path::{Path, PathBuf};

use pw_core::registry::agent::{AgentGrant, agent_grant};
use pw_core::registry::path::worktree_root;
use pw_rpc::{ConnectionScope, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::diagnostics::{
    DiagnosticStore, LIST_LIMIT, LspDiagnostic, LspDiagnosticTag, LspDiagnosticsListParams,
    LspSeverity, lexical, list_cursor, parse_list_cursor,
};
use crate::grant::real_path;
use crate::manager::LspManager;
use crate::methods::LspInstanceStatus;

/// 툴이 응답에 싣는 결과 JSON 의 예산(바이트). 세션의 상한(256 KiB)에 봉투 · 이스케이프의 여유를 남긴 값이다 — 데스크톱 크레이트의
/// 시험이 상한보다 작은지 본다.
pub const RESPONSE_BUDGET: usize = 192 * 1024;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsToolParams {
    pub project_id: String,
    /// 볼 워크트리의 꼭대기(절대 경로). 비면 세션의 워크트리, 그것도 없으면 프로젝트의 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 이 파일만. 절대 경로나 프로젝트 루트(워크트리면 그 안의 프로젝트 루트) 기준 상대 경로(`src/lib.rs`).
    #[serde(default)]
    pub path: Option<String>,
    /// 이 심각도와 그보다 심한 것만(`warning` 이면 오류 · 경고).
    #[serde(default)]
    pub min_severity: Option<LspSeverity>,
    /// 앞 쪽의 `nextCursor`. 비면 처음부터.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 한 쪽의 진단 수, 1 ~ 5000(기본 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsToolResult {
    /// 이 프로젝트(워크트리)의 언어 서버 인스턴스와 상태 — 빈 목록이 「오류 없음」 인지 「서버가 아직 없다 · 준비 중이다」 인지 가른다.
    /// 비어 있으면 띄운 서버가 없다(Pitwall 창에서 그 언어의 파일을 열면 뜬다). `state.kind` 가 `ready` 가 아니면 답하지 않는 중이고,
    /// `progress` 가 있으면 서버가 색인 · 검사 중이라 진단이 더 올 수 있다.
    pub servers: Vec<LspInstanceStatus>,
    /// 경로 차례로 파일들. 파일 안은 심각도(오류 먼저) · 위치 차례. 한 파일의 진단이 두 쪽에 걸칠 수 있다.
    pub files: Vec<DiagnosticsToolFile>,
    /// 다음 쪽을 부를 `cursor`. 마지막 쪽이면 `null`.
    pub next_cursor: Option<String>,
    /// 서버가 진단을 쏟아부어 상한(파일당 1000 · 프로젝트 10만)에서 잘렸다 — 심한 것부터 남겼다.
    pub truncated: bool,
    /// 진단을 볼 수 없는 까닭(그러면 `files` 는 비어 있다).
    pub unavailable: Option<DiagnosticsUnavailable>,
}

/// 파일 하나의 진단.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsToolFile {
    /// 절대 경로.
    pub path: String,
    pub diagnostics: Vec<DiagnosticsToolItem>,
    /// 이 파일의 진단이 상한에 걸려 일부만 있다.
    pub truncated: bool,
}

/// 진단 하나. 줄 · 열은 1 부터, 열은 UTF-16 코드 단위.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsToolItem {
    pub severity: LspSeverity,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub message: String,
    /// 진단 코드(`E0308` 등).
    pub code: Option<String>,
    /// 진단을 낸 도구(`rustc` · `ts` 등 — 서버가 알린 것).
    pub source: Option<String>,
    /// 낸 언어 서버 id(`servers[].server`).
    pub server: String,
    /// 코드 설명 링크.
    pub code_href: Option<String>,
    /// `unnecessary`(쓰이지 않는 코드) · `deprecated`.
    pub tags: Vec<LspDiagnosticTag>,
    /// 관련된 다른 자리.
    pub related: Vec<DiagnosticsToolRelated>,
}

/// 진단의 관련 정보. 줄 · 열은 1 부터.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsToolRelated {
    /// 파일이면 그 절대 경로.
    pub path: Option<String>,
    /// 파일이 아니면(`path` 가 없으면) 서버가 준 URI.
    pub uri: Option<String>,
    pub line: u32,
    pub column: u32,
    pub message: String,
}

/// 진단을 볼 수 없는 까닭.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsUnavailable {
    pub reason: DiagnosticsUnavailableReason,
    /// 사람이 할 일을 담은 한 줄(영어).
    pub message: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticsUnavailableReason {
    /// 프로젝트를 신뢰하지 않아 언어 서버가 뜨지 않는다.
    Untrusted,
}

rpc_method! {
    /// Language server diagnostics (errors, warnings, hints) for a project, one page at a time: files in path order, each
    /// file's diagnostics most severe first. Filter by one file (`path`, absolute or relative to the project root) and by
    /// `minSeverity`. Lines and columns start at 1; columns count UTF-16 code units. Only what the language servers have
    /// published on their own: servers run while the project is open in a Pitwall window (they start when a file of that
    /// language is opened there), and many servers only report files that are open in the editor, so files nobody opened
    /// may be missing — an empty list is not proof the project builds. `servers` tells "no problems" apart from "no server
    /// yet / still starting / still indexing" (a `progress` means more diagnostics may come). This tool never starts a
    /// server. An untrusted project runs no language servers: the result is empty with `unavailable.reason = "untrusted"`.
    pub DiagnosticsTool: "lsp.tool.diagnostics" {
        params: DiagnosticsToolParams,
        result: DiagnosticsToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "code", name: "diagnostics" },
    }
}

/// 툴 메서드의 핸들러를 붙인다(`crate::register` 가 부른다 — 앱의 Host 만 언어 서버를 붙인다).
pub fn register(router: &mut Router, manager: &LspManager, store: DiagnosticStore) {
    let manager = manager.clone();
    router.unary(DiagnosticsTool, move |ctx, params| {
        let (manager, store, scope) = (manager.clone(), store.clone(), ctx.scope().clone());
        async move { call(&manager, &store, scope, params).await }
    });
}

async fn call(
    manager: &LspManager,
    store: &DiagnosticStore,
    scope: ConnectionScope,
    params: DiagnosticsToolParams,
) -> Result<DiagnosticsToolResult, RpcError> {
    let limit = params.limit.unwrap_or(pw_rpc::DEFAULT_LIST_LIMIT);
    if !(1..=LIST_LIMIT).contains(&limit) {
        return Err(RpcError::invalid_params(format!(
            "`limit` must be 1 to {LIST_LIMIT}"
        )));
    }
    let after = params
        .cursor
        .as_deref()
        .map(parse_list_cursor)
        .transpose()?;
    let projects = manager.projects()?;
    let (grant, trusted, path) = {
        let project_id = params.project_id.clone();
        let worktree = params.worktree.clone();
        let path = params.path.clone();
        blocking(move || {
            let grant = agent_grant(&scope, &projects, &project_id)?;
            let trusted = projects.is_trusted(&project_id);
            let path = path
                .map(|p| file_path(&grant, worktree.as_deref(), &p))
                .transpose()?;
            Ok((grant, trusted, path))
        })
        .await?
    };
    let servers = manager.status_in(grant.project_id(), params.worktree.as_deref());
    if !trusted {
        return Ok(DiagnosticsToolResult {
            servers,
            files: Vec::new(),
            next_cursor: None,
            truncated: false,
            unavailable: Some(DiagnosticsUnavailable {
                reason: DiagnosticsUnavailableReason::Untrusted,
                message: "the project is not trusted, so its language servers do not run; trust it in Pitwall to get diagnostics"
                    .to_owned(),
            }),
        });
    }
    let list = store.list(&LspDiagnosticsListParams {
        project_id: params.project_id,
        worktree: params.worktree,
        path,
        min_severity: params.min_severity,
        cursor: params.cursor,
        limit,
    })?;
    let mut budget = RESPONSE_BUDGET;
    let mut files = Vec::with_capacity(list.files.len());
    let mut next_cursor = list.next_cursor;
    let mut taken = 0usize;
    'files: for file in list.files {
        // 저장소의 쪽은 커서의 파일을 그 자리부터 싣는다 — 예산에서 끊을 때 이어질 자리를 같은 꼴로 만든다.
        let from = match &after {
            Some((path, skip)) if *path == file.path => *skip,
            _ => 0,
        };
        let mut out = DiagnosticsToolFile {
            path: file.path,
            diagnostics: Vec::with_capacity(file.diagnostics.len()),
            truncated: file.truncated,
        };
        if !take(&mut budget, &out) && taken > 0 {
            next_cursor = Some(list_cursor(&out.path, from));
            break;
        }
        for (n, d) in file.diagnostics.iter().enumerate() {
            let item = item(d);
            // 첫 진단은 예산과 상관없이 싣는다 — 쪽이 비면 다음 커서가 없어 나아가지 못한다.
            if !take(&mut budget, &item) && taken > 0 {
                next_cursor = Some(list_cursor(&out.path, from + n));
                if !out.diagnostics.is_empty() {
                    files.push(out);
                }
                break 'files;
            }
            out.diagnostics.push(item);
            taken += 1;
        }
        files.push(out);
    }
    Ok(DiagnosticsToolResult {
        servers,
        files,
        next_cursor,
        truncated: list.truncated,
        unavailable: None,
    })
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

/// 툴의 `path` → 저장소의 경로 열쇠(서버가 보낸 URI 를 글자로 접은 절대 경로 — 서버의 작업 공간 루트는 실제 경로라 그 아래다).
/// 상대 경로는 프로젝트 루트(워크트리면 그 안의 프로젝트 루트)의 실제 경로 아래로 놓는다. 디스크에 있으면 링크를 푼 실제 경로로.
fn file_path(grant: &AgentGrant, worktree: Option<&str>, path: &str) -> Result<String, RpcError> {
    let given = Path::new(path);
    let joined: PathBuf = if given.is_absolute() {
        given.to_path_buf()
    } else {
        let project = grant.project();
        let root = match worktree {
            None => project.root().to_path_buf(),
            Some(top) => worktree_root(project.root(), Path::new(top)).map_err(|_| {
                RpcError::invalid_params(format!(
                    "`{top}` is not a worktree of project `{}`",
                    project.id
                ))
            })?,
        };
        real_path(&root).unwrap_or(root).join(given)
    };
    let resolved = real_path(&joined).unwrap_or_else(|| lexical(&joined));
    Ok(resolved.to_string_lossy().into_owned())
}

/// 저장소의 진단(0 부터) → 툴의 진단(1 부터).
fn item(d: &LspDiagnostic) -> DiagnosticsToolItem {
    DiagnosticsToolItem {
        severity: d.severity,
        line: d.range.start.line + 1,
        column: d.range.start.character + 1,
        end_line: d.range.end.line + 1,
        end_column: d.range.end.character + 1,
        message: d.message.clone(),
        code: d.code.clone(),
        source: d.source.clone(),
        server: d.server.clone(),
        code_href: d.code_href.clone(),
        tags: d.tags.clone(),
        related: d
            .related
            .iter()
            .map(|r| DiagnosticsToolRelated {
                uri: r.path.is_none().then(|| r.uri.clone()),
                path: r.path.clone(),
                line: r.range.start.line + 1,
                column: r.range.start.character + 1,
                message: r.message.clone(),
            })
            .collect(),
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}
