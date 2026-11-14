//! `project_open` 툴 메서드와 「열기 요청」 자리(`docs/design/16-m5-plan.md` M5-09 ③ · 결정 M5-D24, `docs/design/05-mcp.md`
//! 「프로젝트」). `read`(UI 에 영향을 주지만 비파괴라 승인이 없다 — 05 표의 `*`), 그룹 `project`.
//!
//! Host 에는 창이 없다. Host 는 판정(MCP 세션 · 레지스트리에 있는 프로젝트 · 루트 안 경로 · 빈도)만 하고, 여는 것은 데스크톱이
//! [`OpenRequests::attach`] 로 붙인 [`Opener`] 가 한다 — 데스크톱은 `pitwall <path>` 와 같은 길(창별 보류 큐 · `project://open`)로
//! 연다. 그 Host 를 만들 때 [`crate::LocalHostBuilder::open_requests`] 를 준 Host(앱)에만 이 메서드가 붙는다 — 헤드리스 Host 에는 없어
//! CLI 는 `needs_app` 으로 가른다(결정 M5-D3).
//!
//! - **창을 앞으로 가져오지 않는다**(결정 M5-D24): 그 프로젝트 창이 열려 있으면 그 안에서 탭만 열고, 없으면 활성화하지 않고 창을
//!   만든다. 승인 없는 툴이 사람의 화면 포커스를 가져가면 에이전트가 일하는 동안 사람의 작업이 되풀이해 끊긴다. 창이 생겨도 신뢰
//!   관문은 그대로다 — 미신뢰 프로젝트의 창은 제한 모드다.
//! - **경로**: 프로젝트 루트(워크트리를 주면 그 워크트리 안의 프로젝트 루트) 안만. 상대 경로는 그 루트 기준이다. 판정은 `fs.*` 와
//!   같다 — 레지스트리의 정규화([`crate::registry::path::normalize`])로 심링크를 푼 실제 경로가 루트 안이어야 한다.
//! - **빈도 제한**: Host 전체에 한 벌 — 이 Host 의 모든 에이전트 호출(MCP 세션 · `pitwall tool` 전부)을 합쳐 [`RATE_WINDOW`] 에
//!   [`RATE_LIMIT`] 번. 넘으면 `PERMISSION_DENIED`(`data.reason = "rateLimited"`, 결과 표지 `rate_limited`). 에이전트가 탭 · 창을
//!   수십 개 여는 것을 막는다. 세션마다 세지 않는다 — `pitwall tool` 은 호출마다 새 세션이라 세션 한도는 셸 반복 한 줄로 풀린다
//!   (메인 판정 2026-11-14, 16-m5-plan M5-09). 세는 것은 여는 데까지 간 호출이다(입력이 틀렸거나 앱이 아직 창을 열 수 없어 거절한
//!   호출은 세지 않는다). 값은 출발값이다.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use pw_rpc::{ConnectionScope, Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};

use crate::registry::Registry;
use crate::registry::agent::agent_grant;
use crate::registry::path::{WorktreeRootError, normalize, worktree_root};

/// 빈도 제한의 창.
pub const RATE_WINDOW: Duration = Duration::from_secs(10);
/// [`RATE_WINDOW`] 안에 이 Host 의 에이전트 호출을 모두 합쳐 열 수 있는 횟수.
pub const RATE_LIMIT: usize = 5;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOpenToolParams {
    pub project_id: String,
    /// 열 워크트리의 꼭대기(절대 경로). 비면 세션의 워크트리, 그것도 없으면 프로젝트의 메인 체크아웃. 창은 이 워크트리를 활성
    /// 워크트리로 연다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 열 파일. 상대 경로면 프로젝트 루트(워크트리를 정했으면 그 워크트리 안의 프로젝트 루트) 기준이다. 루트 밖(심링크로 밖을
    /// 가리키는 것 포함)은 거절한다. 폴더면 프로젝트만 연다. 없으면 프로젝트 창만.
    #[serde(default)]
    pub path: Option<String>,
    /// 커서를 둘 줄(1부터). `path` 가 있어야 한다.
    #[serde(default)]
    pub line: Option<u32>,
    /// 커서를 둘 열(1부터). `line` 이 있어야 한다.
    #[serde(default)]
    pub column: Option<u32>,
}

/// 요청이 열린 자리.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum OpenedIn {
    /// 이미 열린 그 프로젝트 창(앞으로 가져오지 않았다).
    ExistingWindow,
    /// 새로 만든 그 프로젝트 창(활성화하지 않았다).
    NewWindow,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOpenToolResult {
    pub opened_in: OpenedIn,
    /// 연 파일의 실제 경로(절대). 파일 없이 프로젝트만 열었으면 `null`.
    pub path: Option<String>,
}

rpc_method! {
    /// Pitwall 앱에서 프로젝트 · 파일 · 줄을 연다. 창을 앞으로 가져오지 않는다 — 그 프로젝트 창이 열려 있으면 그 안에 탭만 열고,
    /// 없으면 활성화하지 않고 창을 만든다. `path` 는 프로젝트 루트 안만(상대 경로는 루트 기준). 모든 에이전트
    /// 호출을 합쳐 10초에 5번까지(넘으면 `rateLimited`). 앱이 실행 중이어야 한다.
    pub ProjectOpenTool: "project.tool.open" {
        params: ProjectOpenToolParams,
        result: ProjectOpenToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "project", name: "project_open" },
    }
}

/// 판정을 지난 열기 요청. 데스크톱이 창의 보류 큐로 옮긴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(정규화한 절대 경로). 메인 체크아웃이면 `None`.
    pub worktree: Option<String>,
    /// 열 파일의 실제 경로(루트 안). 프로젝트만 열면 `None`.
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// 요청을 창으로 옮기는 쪽(데스크톱). 블로킹 스레드에서 부른다 — 창을 만들려면 메인 스레드를 기다린다.
pub trait Opener: Send + Sync + 'static {
    /// 창을 앞으로 가져오지 않고 연다. 그렇게 열 수 없으면 오류(사람에게 보일 글이 아니라 에이전트가 읽을 글).
    fn open(&self, request: &OpenRequest) -> Result<OpenedIn, String>;
}

/// 열기 요청 자리. 복제하면 같은 자리를 가리킨다. Host 를 만들 때 넘기고([`crate::LocalHostBuilder::open_requests`]), 창을 열 수
/// 있게 되면(앱의 setup) [`Self::attach`] 로 여는 쪽을 붙인다 — 그 전의 요청은 「앱이 아직 준비되지 않았다」 로 끝난다.
#[derive(Clone, Default)]
pub struct OpenRequests {
    opener: Arc<OnceLock<Arc<dyn Opener>>>,
    calls: Arc<Mutex<RateLimiter>>,
}

impl OpenRequests {
    /// 두 번째부터는 무시한다.
    pub fn attach(&self, opener: Arc<dyn Opener>) {
        let _ = self.opener.set(opener);
    }
}

/// Host 전체의 빈도 셈: 지난 [`RATE_WINDOW`] 안에 연 시각들.
#[derive(Default)]
struct RateLimiter {
    calls: VecDeque<Instant>,
}

impl RateLimiter {
    /// 지금 하나 더 열 수 있으면 세고 `Ok`, 아니면 다시 될 때까지 남은 시간.
    fn take(&mut self, now: Instant) -> Result<(), Duration> {
        while self
            .calls
            .front()
            .is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW)
        {
            self.calls.pop_front();
        }
        if self.calls.len() >= RATE_LIMIT {
            let oldest = self.calls.front().copied().unwrap_or(now);
            return Err(RATE_WINDOW.saturating_sub(now.duration_since(oldest)));
        }
        self.calls.push_back(now);
        Ok(())
    }
}

/// 핸들러를 붙인다(앱의 Host — [`crate::LocalHostBuilder::open_requests`]).
pub fn register(router: &mut Router, requests: OpenRequests, registry: Registry) {
    router.unary(
        ProjectOpenTool,
        move |ctx, params: ProjectOpenToolParams| {
            let (requests, registry) = (requests.clone(), registry.clone());
            let scope = ctx.scope().clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    let request = prepare(&scope, &registry, &params)?;
                    let opener = requests.opener.get().ok_or_else(|| {
                        RpcError::internal("the Pitwall app is not ready to open windows yet")
                    })?;
                    requests
                        .calls
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take(Instant::now())
                        .map_err(rate_limited)?;
                    let opened_in = opener.open(&request).map_err(RpcError::internal)?;
                    Ok(ProjectOpenToolResult {
                        opened_in,
                        path: request.file.map(|f| f.display().to_string()),
                    })
                })
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
            }
        },
    );
}

/// 관문 · 입력 판정을 지난 요청. 레지스트리 · 디스크(경로 정규화)를 읽는다.
pub fn prepare(
    scope: &ConnectionScope,
    registry: &Registry,
    params: &ProjectOpenToolParams,
) -> Result<OpenRequest, RpcError> {
    let grant = agent_grant(scope, registry, &params.project_id)?;
    let project = grant.project();
    let worktree = params.worktree.as_deref().filter(|w| !w.is_empty());
    let root = match worktree {
        None => project.root().to_path_buf(),
        Some(top) => worktree_root(project.root(), Path::new(top)).map_err(|e| match e {
            WorktreeRootError::Relative => {
                RpcError::invalid_params(format!("worktree `{top}` must be an absolute path"))
            }
            WorktreeRootError::NotOurs => RpcError::invalid_params(format!(
                "`{top}` is not a worktree of project `{}`",
                params.project_id
            )),
        })?,
    };
    let path = params.path.as_deref().filter(|p| !p.is_empty());
    if path.is_none() && params.line.is_some() {
        return Err(RpcError::invalid_params("`line` needs a `path`"));
    }
    if params.line.is_none() && params.column.is_some() {
        return Err(RpcError::invalid_params("`column` needs a `line`"));
    }
    if params.line == Some(0) || params.column == Some(0) {
        return Err(RpcError::invalid_params("`line` and `column` start at 1"));
    }
    let file = path.map(|p| inside(&root, p)).transpose()?;
    // 창은 워크트리 꼭대기를 받는다(`ActiveWorktree.top`). 정규화해 둔다 — 상대 경로는 위에서 거절됐다.
    let worktree = worktree
        .map(|top| normalize(Path::new(top)).map(|t| t.display().to_string()))
        .transpose()
        .map_err(|_| RpcError::invalid_params("worktree must be an absolute path"))?;
    Ok(OpenRequest {
        project_id: params.project_id.clone(),
        worktree,
        file: file.filter(|f| *f != root),
        line: params.line,
        column: params.column,
    })
}

/// `path` 의 실제 경로가 `root` 안이면 그 경로(`fs.*` 의 읽기 판정과 같다 — 심링크를 푼 뒤 구성 요소 단위 접두).
fn inside(root: &Path, path: &str) -> Result<PathBuf, RpcError> {
    let given = Path::new(path);
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else {
        root.join(given)
    };
    let outside = || RpcError::invalid_params(format!("`{path}` is outside the project root"));
    let real = normalize(&joined).map_err(|_| outside())?;
    let root = normalize(root).map_err(|_| outside())?;
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err(outside())
    }
}

/// 빈도를 넘었다. 결과 표지는 `denied` · `rate_limited` 다(`pw_mcp::marker`).
fn rate_limited(retry_after: Duration) -> RpcError {
    RpcError::new(
        code::PERMISSION_DENIED,
        format!(
            "project_open is rate limited: at most {RATE_LIMIT} calls per {} seconds for all agents together",
            RATE_WINDOW.as_secs()
        ),
    )
    .with_data(serde_json::json!({
        "reason": "rateLimited",
        "retryAfterMs": u64::try_from(retry_after.as_millis()).unwrap_or(u64::MAX),
    }))
}

#[cfg(test)]
mod tests;
