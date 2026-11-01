//! 도구 설치의 허가(결정 M4-D18 — 15-m4-plan.md M4-04 ⑥).
//!
//! 설치 · 업데이트 · 되돌리기 · 제거 RPC 는 tier exec(owner)이고, 연결 범위는 **설정 창의 Host 연결** 또는 **프로젝트 창의 그 프로젝트
//! 연결**만 받는다. 파일 창 · 웰컴 창 · 창이 없는 Host 연결(MCP 세션 — `pw-mcp` 는 `Caller(Owner, Host, window = None)` 로 붙는다 ·
//! 원격)은 `PERMISSION_DENIED` 다. 어느 창이 설정 창인지는 데스크톱이 창 label 판정 함수로 넘긴다(이 크레이트는 창 label 을 모른다).
//!
//! [`ToolGrant`] 는 [`tool_grant`] 만 만든다. 설치 함수는 이것을 받아야 하고, 이 모듈 밖에서는 만들 수 없다:
//!
//! ```compile_fail
//! let _ = pw_tools::install::ToolGrant { project: None };
//! ```
//!
//! 설치가 npm · python 을 띄우는 것은 이 허가의 일부다(실행의 연결 범위 `exec_grant` 의 Host 예외를 넓히지 않고 따로 둔다).

use pw_rpc::{ConnectionScope, RpcError, code};
use serde_json::json;

/// 판정을 지난 증거. npm · python 을 찾을 런타임 환경은 이것이 정한다(프로젝트 창이면 그 프로젝트, 설정 창이면 로그인 셸).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGrant {
    project: Option<String>,
}

impl ToolGrant {
    /// 프로젝트 창이면 그 프로젝트 id, 설정 창이면 `None`.
    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
}

/// 이 연결이 도구를 설치 · 업데이트 · 되돌리기 · 제거해도 되는가. `settings_window` 는 창 label 이 설정 창인지 답한다.
pub fn tool_grant(
    scope: &ConnectionScope,
    window: Option<&str>,
    settings_window: &dyn Fn(&str) -> bool,
) -> Result<ToolGrant, RpcError> {
    let reason = match scope {
        ConnectionScope::Project(id) => {
            return Ok(ToolGrant {
                project: Some(id.clone()),
            });
        }
        ConnectionScope::Host => match window {
            Some(label) if settings_window(label) => return Ok(ToolGrant { project: None }),
            Some(_) => "notSettingsWindow",
            None => "noWindow",
        },
        ConnectionScope::File(_) => "fileWindow",
    };
    tracing::info!(target: "pw_tools", reason, "tool install refused for this connection");
    Err(RpcError::new(
        code::PERMISSION_DENIED,
        "only the settings window or a project window may install tools",
    )
    .with_data(json!({ "reason": reason })))
}
