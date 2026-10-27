//! 프로젝트 안에서 프로세스를 띄워도 되는가(Exec 등급 RPC 의 연결 범위, 03-architecture.md 「연결 범위」).
//!
//! 판정은 [`exec_grant`] 한 곳이고, 프로세스를 띄우는 함수(`pw_run` 의 터미널, `pw_open_with` 의 실행)는 그 결과인 [`ExecGrant`]
//! 를 받아야 한다 — 이 모듈 밖에서는 만들 수 없으므로 판정을 건너뛰고는 컴파일되지 않는다. 범위는 요청 값이 아니라 연결이 정한
//! 것이다(`fs.*` · git 쓰기와 같은 자리, [`super::state::scope_may_write`]).
//!
//! - 그 프로젝트의 창(`ConnectionScope::Project(그 id)`)은 무엇이든 띄운다. 작업 디렉터리가 그 프로젝트(활성 워크트리 포함) 안인지는
//!   띄우는 쪽이 이미 본다.
//! - 다른 프로젝트 창 · 파일 창은 아무것도 띄우지 못한다.
//! - Host 연결(웰컴 · 설정 창)은 하나만 된다: 웰컴 창의 「다음에서 열기」 — 등록된 외부 도구로 등록된 프로젝트 루트 그대로를 여는
//!   것. 워크트리 · 파일 · 줄 · 열을 넘기거나 목록에 없는 도구를 고르면 거절이다. 웹뷰는 도구 id 만 고르고 명령은 설정이 정한다.

use pw_rpc::{ConnectionScope, RpcError, code};

use super::state::scope_may_write;

/// 띄우려는 것.
pub enum Launch<'a> {
    /// 셸 · 명령(`terminal.create`). 그 프로젝트의 창만.
    Process,
    /// 외부 도구로 열기(`openWith.run`).
    OpenWith(OpenWithLaunch<'a>),
}

/// `openWith.run` 이 받은 것. Host 연결에는 이것이 등록된 프로젝트 루트를 등록된 도구로 여는 것뿐인지 본다.
pub struct OpenWithLaunch<'a> {
    /// 고른 도구. 없으면 설정의 기본 도구(목록에 있는 것만 고른다).
    pub tool: Option<&'a str>,
    /// 이 OS 의 도구 목록(확장 + 전역 설정)에 있는 id 인가.
    pub registered: &'a dyn Fn(&str) -> bool,
    pub worktree: Option<&'a str>,
    pub file: Option<&'a str>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// [`exec_grant`] 를 지난 허가. 띄울 프로젝트는 이것이 정한다([`Self::project_id`]).
#[derive(Debug)]
pub struct ExecGrant {
    project_id: String,
}

impl ExecGrant {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

/// 이 연결이 `project_id` 안에서 `launch` 를 띄워도 되는가(머리 주석). 거절은 `PERMISSION_DENIED` 이고 아무것도 띄우지 않는다.
pub fn exec_grant(
    scope: &ConnectionScope,
    project_id: &str,
    launch: &Launch<'_>,
) -> Result<ExecGrant, RpcError> {
    let granted = || ExecGrant {
        project_id: project_id.to_owned(),
    };
    if scope_may_write(scope, project_id) {
        return Ok(granted());
    }
    let reason = match (scope, launch) {
        (ConnectionScope::Host, Launch::OpenWith(open)) => match host_open(open) {
            None => return Ok(granted()),
            Some(reason) => reason,
        },
        _ => "only the project's own window may run processes in it",
    };
    Err(RpcError::new(code::PERMISSION_DENIED, reason)
        .with_data(serde_json::json!({ "projectId": project_id })))
}

/// Host 연결의 「다음에서 열기」 가 넘으면 안 되는 것. 없으면 `None`.
fn host_open(open: &OpenWithLaunch<'_>) -> Option<&'static str> {
    if open.worktree.is_some()
        || open.file.is_some()
        || open.line.is_some()
        || open.column.is_some()
    {
        return Some("a host connection opens only the registered project root");
    }
    if open.tool.is_some_and(|tool| !(open.registered)(tool)) {
        return Some("a host connection opens only with a registered tool");
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pw_rpc::{Caller, Role};

    use super::*;

    fn open(tool: Option<&str>) -> OpenWithLaunch<'_> {
        OpenWithLaunch {
            tool,
            registered: &|id| id == "fake",
            worktree: None,
            file: None,
            line: None,
            column: None,
        }
    }

    fn denied(result: Result<ExecGrant, RpcError>) -> bool {
        matches!(result, Err(e) if e.code == code::PERMISSION_DENIED)
    }

    #[test]
    fn the_project_window_runs_anything_in_its_project() {
        let scope = ConnectionScope::Project("p1".into());
        let grant = exec_grant(&scope, "p1", &Launch::Process).unwrap();
        assert_eq!(grant.project_id(), "p1");
        let with_file = OpenWithLaunch {
            worktree: Some("/w"),
            file: Some("/w/a.txt"),
            line: Some(3),
            ..open(Some("anything"))
        };
        assert!(exec_grant(&scope, "p1", &Launch::OpenWith(with_file)).is_ok());
    }

    #[test]
    fn other_project_and_file_windows_run_nothing() {
        let file = Caller::file(Role::Owner, PathBuf::from("/p1/a.txt")).scope;
        for scope in [ConnectionScope::Project("p2".into()), file] {
            assert!(denied(exec_grant(&scope, "p1", &Launch::Process)));
            assert!(denied(exec_grant(
                &scope,
                "p1",
                &Launch::OpenWith(open(Some("fake")))
            )));
        }
    }

    #[test]
    fn a_host_connection_only_opens_the_registered_root_with_a_registered_tool() {
        let host = ConnectionScope::Host;
        assert!(denied(exec_grant(&host, "p1", &Launch::Process)));
        assert!(exec_grant(&host, "p1", &Launch::OpenWith(open(Some("fake")))).is_ok());
        assert!(exec_grant(&host, "p1", &Launch::OpenWith(open(None))).is_ok());
        assert!(denied(exec_grant(
            &host,
            "p1",
            &Launch::OpenWith(open(Some("evil")))
        )));
        let beyond_root = [
            OpenWithLaunch {
                worktree: Some("/w"),
                ..open(None)
            },
            OpenWithLaunch {
                file: Some("/p1/a.txt"),
                ..open(None)
            },
            OpenWithLaunch {
                line: Some(1),
                ..open(None)
            },
            OpenWithLaunch {
                column: Some(1),
                ..open(None)
            },
        ];
        for launch in beyond_root {
            assert!(denied(exec_grant(&host, "p1", &Launch::OpenWith(launch))));
        }
    }
}
