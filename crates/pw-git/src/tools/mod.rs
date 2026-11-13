//! git 조회 MCP 툴 메서드(`docs/design/16-m5-plan.md` M5-05, `docs/design/05-mcp.md` 「Git」): `git_status` · `git_log` ·
//! `git_diff` · `git_blame` · `git_branch_list`. 모두 `read`, 그룹 `git` 이고 쓰기 툴은 없다(결정 M5-D18 — 에이전트는 git CLI 를
//! 직접 쓴다).
//!
//! 툴 메서드 규약(`crates/pw-mcp` 머리 주석)대로 UI 의 RPC 와 **같은 함수**를 부르는 얇은 선언이다 — `git.status` · `git.refs` ·
//! `git.repo`([`crate::status::Status`] · [`crate::Git::repo`]), `git.logPage`([`crate::log::methods::page`]), `git.diff`
//! ([`crate::Git::diff`]), `git.blame`([`crate::blame::methods::prepare`] · [`crate::blame::methods::run`] — 스트림이라 툴이 될 수
//! 없어 줄 범위를 받는 unary 를 따로 둔다). 미신뢰 프로젝트의 읽기 규칙(git CLI · 저장소의 프로그램을 띄우지 않는다 —
//! 03-architecture.md 「보안」)은 그 함수들이 지킨다. 툴이 따로 판정하지 않는다.
//!
//! - **관문**: 모든 툴이 먼저 `agent_grant`(MCP 세션의 연결 범위 · 레지스트리에 있는 프로젝트, `pw_core::registry::agent`)를 지난다.
//!   UI 의 git 읽기 RPC 는 호출자를 보지 않지만 툴 메서드는 MCP 세션만 부른다. 없는 프로젝트는 그 관문의 오류(invalid params)
//!   그대로다.
//! - **응답 크기**: 세션은 256 KiB 를 넘는 응답을 「범위를 좁혀라」 오류로 바꾼다(`pw_mcp::server::MAX_RESPONSE_BYTES`). 툴은 그
//!   아래 [`RESPONSE_BUDGET`] 안에서 자르고 `truncated` 나 다음 `cursor` 를 알린다.

pub mod blame;
pub mod branches;
pub mod diff;
pub mod log;
pub mod status;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use pw_core::registry::agent::AgentGrant;
use pw_rpc::{ConnectionScope, Router, RpcError};
use serde::Serialize;

use crate::Git;
use crate::status::Status;

/// 툴이 응답에 싣는 결과 JSON 의 예산(바이트). 세션의 상한(256 KiB)에 봉투 · 이스케이프의 여유를 남긴 값이다 — 데스크톱 크레이트의
/// 시험이 상한보다 작은지 본다.
pub const RESPONSE_BUDGET: usize = 192 * 1024;

/// 툴 결과를 예산 안에서 채우는 셈.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    left: usize,
}

impl Budget {
    pub(crate) fn new() -> Self {
        Self {
            left: RESPONSE_BUDGET,
        }
    }

    /// `item` 을 직렬화한 크기(배열의 쉼표 포함)만큼 쓴다. 남은 예산에 들지 않으면 쓰지 않고 `false`.
    pub(crate) fn take(&mut self, item: &impl Serialize) -> bool {
        let size = serde_json::to_vec(item).map_or(usize::MAX, |v| v.len() + 1);
        if size > self.left {
            return false;
        }
        self.left -= size;
        true
    }
}

/// MCP 세션용 관문(블로킹 — 레지스트리를 디스크에서 읽는다). `agent_grant` 의 오류를 그대로 돌려준다.
pub(crate) async fn grant(
    git: &Arc<Git>,
    scope: ConnectionScope,
    project_id: &str,
) -> Result<AgentGrant, RpcError> {
    let git = Arc::clone(git);
    let project_id = project_id.to_owned();
    blocking(move || {
        pw_core::registry::agent::agent_grant(&scope, &git.host()?.projects, &project_id)
    })
    .await
}

pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 툴 메서드 다섯의 핸들러를 붙인다(`crate::status::register` 가 부른다 — 앱의 Host 만 git 을 붙인다).
pub fn register(router: &mut Router, status: Arc<Status>) {
    let s = Arc::clone(&status);
    router.unary(status::GitStatusTool, move |ctx, params| {
        let (s, scope) = (Arc::clone(&s), ctx.scope().clone());
        async move { status::call(&s, scope, params).await }
    });
    let s = Arc::clone(&status);
    router.unary(branches::GitBranchListTool, move |ctx, params| {
        let (s, scope) = (Arc::clone(&s), ctx.scope().clone());
        async move { branches::call(&s, scope, params).await }
    });
    let git = Arc::clone(status.git());
    let g = Arc::clone(&git);
    router.unary(log::GitLogTool, move |ctx, params| {
        let (g, scope) = (Arc::clone(&g), ctx.scope().clone());
        async move { log::call(&g, scope, params).await }
    });
    let g = Arc::clone(&git);
    router.unary(diff::GitDiffTool, move |ctx, params| {
        let (g, scope) = (Arc::clone(&g), ctx.scope().clone());
        async move { diff::call(&g, scope, params).await }
    });
    router.unary(blame::GitBlameTool, move |ctx, params| {
        let (g, scope) = (Arc::clone(&git), ctx.scope().clone());
        async move { blame::call(&g, scope, params).await }
    });
}
