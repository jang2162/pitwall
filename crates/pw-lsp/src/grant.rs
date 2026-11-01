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
//! 복제할 수 없다 — 허가를 쌓아 두었다가 나중에 다시 쓰는 길을 형으로 줄인다(아래 「신뢰를 거두면」 의 재확인과 함께):
//!
//! ```compile_fail
//! fn keep(grant: &pw_lsp::LspGrant) -> pw_lsp::LspGrant {
//!     grant.clone()
//! }
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
//! **루트에 묶인다**(M4-01 리뷰 M1): 허가는 그 프로젝트(활성 워크트리)의 루트를 정규화(심볼릭 링크를 푼 실제 경로)해 쥐고, 서버의
//! 작업 디렉터리(`LaunchSpec::cwd`)와 `initialize` 의 작업 공간 루트가 그 안일 때만 띄우고 보낸다([`LspGrant::contains`]) — 루트를
//! 고르는 쪽(M4-05 의 `root_markers` 위로 걷기)에 잘못이 있어도 미신뢰 폴더의 코드가 돌지 않는다.
//!
//! **신뢰를 거두면**(M4-01 리뷰 M2): 허가는 레지스트리를 함께 쥐고, 서버를 띄우는 그 자리에서 신뢰를 다시 본다
//! ([`LspGrant::still_trusted`]) — 쥐고 있던 허가로는 신뢰를 거둔 뒤 띄울 수 없다. 물러남 재기동 · `lsp.server.restart` · 설치 뒤
//! 기동은 그래도 매번 [`lsp_grant`] 를 다시 지난다(M4-05). 신뢰를 풀면 서버 관리자가 그 프로젝트의 서버를 끝낸다.

use std::path::{Path, PathBuf};

use pw_core::registry::exec::{Launch, exec_grant};
use pw_core::registry::path::{WorktreeRootError, simplify, worktree_root};
use pw_core::registry::{Registry, RegistryError};
use pw_rpc::{ConnectionScope, RpcError, code};
use pw_runtime::{RuntimeEnv, runtime::Runtime};
use serde_json::json;

/// [`lsp_grant`] 를 지난 허가: 이 프로젝트 · 워크트리의 루트 안에서 언어 서버를 띄워도 된다. 복제할 수 없다(머리 주석).
pub struct LspGrant {
    project_id: String,
    worktree: Option<String>,
    /// 정규화한 루트.
    root: PathBuf,
    projects: Registry,
}

impl std::fmt::Debug for LspGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspGrant")
            .field("project_id", &self.project_id)
            .field("worktree", &self.worktree)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl LspGrant {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// 활성 워크트리의 꼭대기(창이 준 그대로). 없으면 주 루트.
    pub fn worktree(&self) -> Option<&str> {
        self.worktree.as_deref()
    }

    /// 활성 워크트리 안의 프로젝트 루트(없으면 주 루트)를 정규화한 경로. 서버의 작업 공간 루트는 이 안에서 고른다(M4-05).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `path` 가 이 허가의 루트 안(루트 자신 포함)인가. 심볼릭 링크를 푼 실제 경로로 본다 — 없는 경로 · 읽지 못하는 경로는 밖이다.
    pub fn contains(&self, path: &Path) -> bool {
        real_path(path).is_some_and(|real| real.starts_with(&self.root))
    }

    /// 지금도 신뢰하는가(레지스트리를 다시 읽는다 — 블로킹이지만 짧다). 서버를 띄우는 자리가 부른다.
    pub fn still_trusted(&self) -> bool {
        self.projects.is_trusted(&self.project_id)
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
    let root = real_path(&root).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "cannot resolve the project root {}",
            root.display()
        ))
    })?;
    Ok(LspGrant {
        project_id: project_id.to_owned(),
        worktree: worktree.map(str::to_owned),
        root,
        projects: projects.clone(),
    })
}

/// 심볼릭 링크를 푼 실제 경로(Windows 의 `\\?\` 머리는 걷는다 — 레지스트리의 경로와 같은 꼴).
pub(crate) fn real_path(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok().map(simplify)
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

    /// 리뷰 M1: 허가는 자기 루트 안의 경로만 받는다 — 심볼릭 링크로 빠져나가는 경로도 밖이다.
    #[test]
    fn a_grant_contains_only_paths_under_its_real_root() {
        let f = fixture();
        let scope = ConnectionScope::Project(f.trusted.clone());
        let grant = lsp_grant(&scope, &f.projects, &f.trusted, None).unwrap();
        let inner = f.root.join("crates").join("a");
        fs::create_dir_all(&inner).unwrap();
        assert!(grant.contains(&f.root));
        assert!(grant.contains(&inner));
        assert!(grant.contains(&f.root.join("crates").join("..").join("crates")));
        let outside = f.root.parent().unwrap().join("untrusted");
        assert!(!grant.contains(&outside));
        assert!(!grant.contains(&f.root.join("..").join("untrusted")));
        assert!(!grant.contains(&f.root.join("missing")));
        // 이름이 루트로 시작할 뿐인 형제 폴더는 밖이다.
        let sibling = f.root.parent().unwrap().join("trusted-not");
        fs::create_dir_all(&sibling).unwrap();
        assert!(!grant.contains(&sibling));
        #[cfg(unix)]
        {
            let link = f.root.join("escape");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert!(!grant.contains(&link));
        }
    }

    /// 리뷰 M2: 허가를 쥔 채 신뢰를 거두면 `still_trusted` 가 거짓이다(서버를 띄우는 자리가 본다).
    #[test]
    fn a_held_grant_sees_trust_revoked() {
        let f = fixture();
        let scope = ConnectionScope::Project(f.trusted.clone());
        let grant = lsp_grant(&scope, &f.projects, &f.trusted, None).unwrap();
        assert!(grant.still_trusted());
        f.projects.set_trust(&f.trusted, false).unwrap();
        assert!(!grant.still_trusted());
    }
}
