//! 언어 서버 실행 허가(15-m4-plan.md M4-01 ③, 결정 M4-D8).
//!
//! 언어 서버는 프로젝트 코드를 돌린다(빌드 스크립트 · proc-macro · Gradle · tsserver 플러그인 · 프로젝트 인터프리터). 그래서 서버를 띄우는
//! 함수([`crate::server::LanguageServer::spawn`])는 [`LspGrant`] 를 받고, [`LspGrant`] 는 [`lsp_grant`] 만 만든다 — 두 판정을 다
//! 지났다는 증거다.
//!
//! 1. **실행의 연결 범위** — `pw_core::registry::exec::exec_grant`(`Launch::Process`). 그 프로젝트 창의 연결만 프로세스를 띄운다
//!    (`scope_may_write` 와 같은 판정 — 터미널 · 실행 · DB 세션과 같다).
//! 2. **신뢰** — `Registry::is_trusted`. 미신뢰 프로젝트(워크트리면 메인 저장소 프로젝트의 신뢰)는 `PERMISSION_DENIED`
//!    (`data.reason = "untrusted"`) — M3 의 `db_trust_gate` 와 같은 꼴.
//!
//! 이 모듈 밖에서는 만들 수 없다 — 판정을 건너뛰는 새 길은 컴파일되지 않는다:
//!
//! ```compile_fail
//! let _ = pw_lsp::grant::LspGrant { project_id: String::from("p"), worktree: None, root: std::path::PathBuf::new() };
//! ```
//!
//! ```compile_fail
//! // 허가 없이 띄우는 함수는 없다 — 첫 인자가 `&LspGrant` 다.
//! # async fn f(spec: pw_lsp::LaunchSpec, options: pw_lsp::ServerOptions) {
//! let _ = pw_lsp::LanguageServer::spawn(spec, options).await;
//! # }
//! ```
//!
//! 허가를 넘기면 컴파일된다(위 시험이 다른 까닭으로 실패하는 것이 아님을 보이는 짝):
//!
//! ```no_run
//! # async fn f(grant: &pw_lsp::LspGrant, spec: pw_lsp::LaunchSpec, options: pw_lsp::ServerOptions) {
//! let _ = pw_lsp::LanguageServer::spawn(grant, spec, options).await;
//! # }
//! ```
//!
//! 들고 있어도 신뢰가 이어진다는 뜻은 아니다 — 물러남 재기동 · `lsp.server.restart` · 설치 뒤 기동은 매번 [`lsp_grant`] 를 다시
//! 지난다(M4-05). 신뢰를 풀면 서버 관리자가 그 프로젝트의 서버를 끝낸다.

use std::path::{Path, PathBuf};

use pw_core::registry::exec::{Launch, exec_grant};
use pw_core::registry::path::{WorktreeRootError, worktree_root};
use pw_core::registry::{Registry, RegistryError};
use pw_rpc::{ConnectionScope, RpcError, code};
use pw_runtime::{RuntimeEnv, runtime::Runtime};
use serde_json::json;

/// [`lsp_grant`] 를 지난 허가: 이 프로젝트 · 워크트리에서 언어 서버를 띄워도 된다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspGrant {
    project_id: String,
    worktree: Option<String>,
    root: PathBuf,
}

impl LspGrant {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// 활성 워크트리의 꼭대기(창이 준 그대로). 없으면 주 루트.
    pub fn worktree(&self) -> Option<&str> {
        self.worktree.as_deref()
    }

    /// 활성 워크트리 안의 프로젝트 루트(없으면 주 루트). 서버의 작업 공간 루트는 이 안에서 고른다(M4-05).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 서버를 띄울 환경: 그 프로젝트 · 디렉터리의 런타임 환경(`Runtime::env` — D26). 로그인 셸을 기다릴 수 있다(블로킹).
    pub fn runtime_env(&self, runtime: &Runtime, dir: &Path) -> RuntimeEnv {
        runtime.env(&self.project_id, dir).env.clone()
    }
}

/// 미신뢰 프로젝트를 거절하는 오류(`PERMISSION_DENIED`, `data.reason = "untrusted"`).
pub fn untrusted(project_id: &str) -> RpcError {
    RpcError::new(
        code::PERMISSION_DENIED,
        "trust the project before starting its language servers",
    )
    .with_data(json!({ "reason": "untrusted", "projectId": project_id }))
}

/// 이 연결이 `project_id`(활성 워크트리 `worktree`)에서 언어 서버를 띄워도 되는가(머리 주석의 두 판정). 모르는 프로젝트는
/// `NOT_FOUND`, 그 프로젝트의 것이 아닌 워크트리는 invalid params.
pub fn lsp_grant(
    scope: &ConnectionScope,
    projects: &Registry,
    project_id: &str,
    worktree: Option<&str>,
) -> Result<LspGrant, RpcError> {
    exec_grant(scope, project_id, &Launch::Process)?;
    let project = projects.get(project_id).map_err(registry_error)?;
    if !projects.is_trusted(project_id) {
        tracing::info!(target: "pw_lsp", project = project_id, "language server refused: the project is not trusted");
        return Err(untrusted(project_id));
    }
    let root = match worktree {
        None => project.root().to_path_buf(),
        Some(top) => worktree_root(project.root(), Path::new(top)).map_err(|e| match e {
            WorktreeRootError::Relative => {
                RpcError::invalid_params(format!("worktree `{top}` must be an absolute path"))
            }
            WorktreeRootError::NotOurs => RpcError::invalid_params(format!(
                "`{top}` is not a worktree of project `{project_id}`"
            )),
        })?,
    };
    Ok(LspGrant {
        project_id: project_id.to_owned(),
        worktree: worktree.map(str::to_owned),
        root,
    })
}

fn registry_error(e: RegistryError) -> RpcError {
    match e {
        RegistryError::NotFound(id) => {
            RpcError::new(code::NOT_FOUND, format!("project `{id}` is not registered"))
        }
        other => RpcError::internal(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
    use pw_core::config::AppPaths;
    use pw_core::registry::NewProject;
    use pw_core::registry::path::simplify;
    use pw_rpc::{Caller, Role};

    use super::*;

    struct Fixture {
        _tmp: tempfile::TempDir,
        projects: Registry,
        trusted: String,
        untrusted: String,
        root: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let projects = Registry::new(
            AppPaths::under(&base.join("app")),
            None,
            EventBus::new(DEFAULT_CAPACITY),
        );
        let create = |name: &str| {
            let root = base.join(name);
            fs::create_dir_all(&root).unwrap();
            let id = projects
                .create(NewProject {
                    root: root.clone(),
                    ..NewProject::default()
                })
                .unwrap()
                .id;
            (id, root)
        };
        let (trusted, root) = create("trusted");
        let (untrusted, _) = create("untrusted");
        projects.set_trust(&trusted, true).unwrap();
        Fixture {
            _tmp: tmp,
            projects,
            trusted,
            untrusted,
            root,
        }
    }

    fn reason(e: &RpcError) -> Option<&str> {
        e.data.as_ref()?.get("reason")?.as_str()
    }

    #[test]
    fn the_trusted_projects_own_window_gets_a_grant_for_its_root() {
        let f = fixture();
        let scope = ConnectionScope::Project(f.trusted.clone());
        let grant = lsp_grant(&scope, &f.projects, &f.trusted, None).unwrap();
        assert_eq!(grant.project_id(), f.trusted);
        assert_eq!(grant.root(), f.root);
        assert_eq!(grant.worktree(), None);
    }

    #[test]
    fn an_untrusted_project_never_gets_a_grant() {
        let f = fixture();
        let scope = ConnectionScope::Project(f.untrusted.clone());
        let e = lsp_grant(&scope, &f.projects, &f.untrusted, None).unwrap_err();
        assert_eq!(e.code, code::PERMISSION_DENIED);
        assert_eq!(reason(&e), Some("untrusted"));
        // 신뢰를 거두면 같은 창이라도 다시 받지 못한다.
        let scope = ConnectionScope::Project(f.trusted.clone());
        f.projects.set_trust(&f.trusted, false).unwrap();
        let e = lsp_grant(&scope, &f.projects, &f.trusted, None).unwrap_err();
        assert_eq!(reason(&e), Some("untrusted"));
    }

    #[test]
    fn other_windows_get_no_grant_even_for_a_trusted_project() {
        let f = fixture();
        let file = Caller::file(Role::Owner, f.root.join("a.rs")).scope;
        for scope in [
            ConnectionScope::Host,
            ConnectionScope::Project(f.untrusted.clone()),
            file,
        ] {
            let e = lsp_grant(&scope, &f.projects, &f.trusted, None).unwrap_err();
            assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
            assert_ne!(reason(&e), Some("untrusted"), "{scope:?}");
        }
    }

    #[test]
    fn unknown_projects_and_foreign_worktrees_are_refused() {
        let f = fixture();
        let scope = ConnectionScope::Project("nope".to_owned());
        let e = lsp_grant(&scope, &f.projects, "nope", None).unwrap_err();
        assert_eq!(e.code, code::NOT_FOUND);
        let scope = ConnectionScope::Project(f.trusted.clone());
        let e = lsp_grant(&scope, &f.projects, &f.trusted, Some("relative")).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
        let elsewhere = f.root.parent().unwrap().join("untrusted");
        let e = lsp_grant(
            &scope,
            &f.projects,
            &f.trusted,
            Some(elsewhere.to_str().unwrap()),
        )
        .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }
}
