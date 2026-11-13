//! MCP 세션의 관문(결정 M5-D27 — 16-m5-plan.md M5-02 ③).
//!
//! MCP 세션은 제 범위로 붙는다([`ConnectionScope::Agent`]). 창 범위가 필요한 판정([`super::state::scope_may_write`] ·
//! [`super::exec::exec_grant`] · `pw_db::gate::db_grant` · `pw_tools::install::tool_grant` · `pw_lsp::grant::lsp_grant`)은 이 범위를
//! 거절하므로, UI 가 쓰는 쓰기 · 실행 · DB RPC 에 `mcp: true` 를 잘못 달아도 창의 권한이 나가지 않는다. **툴 메서드**(MCP 세션이
//! 부르라고 만든 메서드)만 이 관문을 명시적으로 지나 [`AgentGrant`] 를 받는다 — 범위가 MCP 세션이고 그 프로젝트가 레지스트리에 있을
//! 때만이다. 이 모듈 밖에서는 만들 수 없다(`DbGrant` · `ExecGrant` 와 같은 꼴):
//!
//! ```compile_fail
//! let _ = pw_core::registry::agent::AgentGrant { project: todo!() };
//! ```
//!
//! 등급(`read` · `write` · `exec`)의 정책과 승인은 이 관문이 보지 않는다 — 그것은 `pw_mcp` 의 일이다. 신뢰(미신뢰 프로젝트의 제한)도
//! 각 툴의 밑 함수가 그대로 지난다.

use pw_rpc::{ConnectionScope, RpcError, code};

use super::methods::rpc_error;
use super::{Project, Registry};

/// [`agent_grant`] 를 지난 증거. 대상 프로젝트는 이것이 정한다([`Self::project`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGrant {
    project: Project,
}

impl AgentGrant {
    pub fn project_id(&self) -> &str {
        &self.project.id
    }

    /// 관문이 레지스트리에서 읽은 그 프로젝트.
    pub fn project(&self) -> &Project {
        &self.project
    }
}

/// MCP 세션의 연결(`ConnectionScope::Agent`)이 `project_id` 에 닿아도 되는가. Host · 창 · 파일 범위는 `PERMISSION_DENIED`
/// (`data.reason = "notAgentSession"`), 레지스트리에 없는 프로젝트는 레지스트리 오류([`rpc_error`] — `project.info` 와 같은 꼴).
///
/// 레지스트리를 디스크에서 읽으므로(동기) 핸들러는 블로킹 스레드에서 부른다.
pub fn agent_grant(
    scope: &ConnectionScope,
    projects: &Registry,
    project_id: &str,
) -> Result<AgentGrant, RpcError> {
    match scope {
        ConnectionScope::Agent => {}
        ConnectionScope::Host | ConnectionScope::Project(_) | ConnectionScope::File(_) => {
            return Err(not_agent_session(project_id));
        }
    }
    let project = projects.get(project_id).map_err(rpc_error)?;
    Ok(AgentGrant { project })
}

/// `PERMISSION_DENIED`(`data.reason = "notAgentSession"`).
pub fn not_agent_session(project_id: &str) -> RpcError {
    RpcError::new(
        code::PERMISSION_DENIED,
        "only an mcp session may call agent tools",
    )
    .with_data(serde_json::json!({ "reason": "notAgentSession", "projectId": project_id }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pw_rpc::{Caller, Role};

    use super::*;
    use crate::bus::{DEFAULT_CAPACITY, EventBus};
    use crate::config::AppPaths;
    use crate::registry::NewProject;

    struct Fixture {
        _tmp: tempfile::TempDir,
        registry: Registry,
        id: String,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work");
        std::fs::create_dir_all(&root).unwrap();
        let registry = Registry::new(
            AppPaths::under(&tmp.path().join("app")),
            None,
            EventBus::new(DEFAULT_CAPACITY),
        );
        let id = registry
            .create(NewProject {
                name: Some("alpha".into()),
                root,
                ..NewProject::default()
            })
            .unwrap()
            .id;
        Fixture {
            _tmp: tmp,
            registry,
            id,
        }
    }

    #[test]
    fn an_mcp_session_gets_a_registered_project() {
        let f = fixture();
        let grant = agent_grant(&ConnectionScope::Agent, &f.registry, &f.id).unwrap();
        assert_eq!(grant.project_id(), f.id);
        assert_eq!(grant.project().name, "alpha");
    }

    #[test]
    fn host_window_and_file_scopes_are_refused() {
        let f = fixture();
        let file = Caller::file(Role::Owner, PathBuf::from("/work/a.txt")).scope;
        for scope in [
            ConnectionScope::Host,
            ConnectionScope::Project(f.id.clone()),
            ConnectionScope::Project("other".into()),
            file,
        ] {
            let e = agent_grant(&scope, &f.registry, &f.id).unwrap_err();
            assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
            assert_eq!(e.data.unwrap()["reason"], "notAgentSession", "{scope:?}");
        }
    }

    #[test]
    fn an_unregistered_project_is_refused() {
        let f = fixture();
        let e = agent_grant(&ConnectionScope::Agent, &f.registry, "nope").unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }
}
