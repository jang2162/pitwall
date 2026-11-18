//! 디버그 세션의 실행 허가와 역요청의 판정(17-m6-plan.md M6-15 ③).
//!
//! 어댑터를 띄우는 것도, 어댑터가 `runInTerminal` 로 디버그 대상을 띄워 달라는 것도, `startDebugging` 으로 어댑터를 하나 더 띄우는
//! 것도 프로세스 실행이다. 그래서 세션을 여는 함수([`crate::DebugSession::start`])는 [`DebugGrant`] 를 받고, [`DebugGrant`] 는
//! [`debug_grant`] 만 만든다 — 두 판정을 다 지났다는 증거다(`pw-lsp` 의 `LspGrant` 와 같은 꼴, 결정 M4-D8):
//!
//! 1. **실행의 연결 범위** — `pw_core::registry::exec::exec_grant`(`Launch::Process`). 그 프로젝트 창의 연결만 띄운다. 다른 프로젝트
//!    창 · 파일 창 · Host 연결 · 에이전트 범위(`ConnectionScope::Agent`)는 거절이다(03 「실행의 연결 범위」).
//! 2. **신뢰** — `Registry::is_trusted`. 미신뢰 프로젝트(워크트리면 메인 저장소 프로젝트의 신뢰)는 `PERMISSION_DENIED`
//!    (`data.reason = "untrusted"`).
//!
//! 이 모듈 밖에서는 만들 수 없다:
//!
//! ```compile_fail
//! # fn f(exec: pw_core::registry::exec::ExecGrant, root: &std::path::Path) {
//! let _ = pw_dap::DebugGrant::new(exec, root);
//! # }
//! ```
//!
//! 허가는 세션 나무에 묶이고, 역요청마다 다시 본다([`DebugGrant::check_run_in_terminal`] · [`DebugGrant::check_start_child`]):
//!
//! - 세션이 끝나면(끊기 시작 · 어댑터 죽음) 허가를 거둔다 — 그 뒤의 역요청은 거절한다.
//! - 신뢰를 거두면 거절한다(레지스트리를 다시 읽는다 — 쥐고 있던 허가로는 신뢰를 거둔 뒤 띄우지 못한다).
//! - `runInTerminal` 의 `cwd` 는 그 워크트리의 프로젝트 루트 안이어야 한다(심볼릭 링크를 푼 실제 경로로 본다). 비어 있으면 루트다.
//! - `kind = "external"`(OS 의 터미널 창)은 받지 않는다 — 실행 관리자의 PTY 로만 띄운다.
//! - `argsCanBeInterpretedByShell` 은 받지 않는다 — 알리지 않았고(`supportsArgsCanBeInterpretedByShell = false`), 셸 없이 argv 로
//!   띄운다.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pw_core::registry::exec::{ExecGrant, Launch, exec_grant};
use pw_core::registry::path::{WorktreeRootError, simplify, worktree_root};
use pw_core::registry::{Registry, RegistryError};
use pw_rpc::{ConnectionScope, RpcError, code};
use serde_json::json;

use crate::protocol::RunInTerminalRequestArguments;
use crate::terminal::TerminalRun;

/// 세션 나무 하나의 실행 허가. [`debug_grant`] 만 만든다(머리 주석).
#[derive(Clone)]
pub struct DebugGrant {
    inner: Arc<Inner>,
}

struct Inner {
    exec: ExecGrant,
    /// 그 워크트리의 프로젝트 루트(실제 경로).
    root: PathBuf,
    projects: Registry,
    revoked: AtomicBool,
}

impl std::fmt::Debug for DebugGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugGrant")
            .field("project_id", &self.project_id())
            .field("root", &self.inner.root)
            .field("revoked", &self.revoked())
            .finish_non_exhaustive()
    }
}

/// 미신뢰 프로젝트를 거절하는 오류(`PERMISSION_DENIED`, `data.reason = "untrusted"`).
pub fn untrusted(project_id: &str) -> RpcError {
    RpcError::new(
        code::PERMISSION_DENIED,
        "trust the project before debugging it",
    )
    .with_data(json!({ "reason": "untrusted", "projectId": project_id }))
}

/// 이 연결이 `project_id`(활성 워크트리 `worktree`)에서 디버그 세션을 열어도 되는가(머리 주석의 두 판정). 모르는 프로젝트는
/// `NOT_FOUND`, 그 프로젝트의 것이 아닌 워크트리는 invalid params.
pub fn debug_grant(
    scope: &ConnectionScope,
    projects: &Registry,
    project_id: &str,
    worktree: Option<&str>,
) -> Result<DebugGrant, RpcError> {
    let exec = exec_grant(scope, project_id, &Launch::Process)?;
    let project = projects.get(project_id).map_err(registry_error)?;
    if !projects.is_trusted(project_id) {
        tracing::info!(target: "pw_dap", project = project_id, "debug session refused: the project is not trusted");
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
    Ok(DebugGrant {
        inner: Arc::new(Inner {
            exec,
            root,
            projects: projects.clone(),
            revoked: AtomicBool::new(false),
        }),
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

/// 심볼릭 링크를 푼 실제 경로(Windows 의 `\\?\` 머리는 걷는다 — 레지스트리의 경로와 같은 꼴).
fn real_path(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok().map(simplify)
}

impl DebugGrant {
    pub fn project_id(&self) -> &str {
        self.inner.exec.project_id()
    }

    /// 실행 관리자에 넘길 허가(`Terminals::run_in_terminal`).
    pub fn exec(&self) -> &ExecGrant {
        &self.inner.exec
    }

    /// 루트(실제 경로).
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// 거둔다(세션 나무가 끝난다). 되돌리지 않는다.
    pub fn revoke(&self) {
        self.inner.revoked.store(true, Ordering::SeqCst);
    }

    pub fn revoked(&self) -> bool {
        self.inner.revoked.load(Ordering::SeqCst)
    }

    /// 지금도 신뢰하는가(레지스트리를 다시 읽는다 — 블로킹이지만 짧다).
    pub fn still_trusted(&self) -> bool {
        self.inner.projects.is_trusted(self.project_id())
    }

    /// 경로가 루트 안인가(실제 경로로 — 없는 경로는 아니다).
    pub fn contains(&self, path: &Path) -> bool {
        real_path(path).is_some_and(|real| real.starts_with(&self.inner.root))
    }

    /// 세션 나무가 아직 역요청을 받아도 되는가(끝남 · 거둠 · 신뢰).
    fn check_alive(&self, session_ending: bool) -> Result<(), String> {
        if session_ending || self.revoked() {
            return Err("the debug session has ended".to_owned());
        }
        if !self.still_trusted() {
            return Err(format!(
                "project `{}` is no longer trusted",
                self.project_id()
            ));
        }
        Ok(())
    }

    /// `startDebugging` 으로 자식 세션(stdio 어댑터면 프로세스 하나 더)을 열어도 되는가. `parent_ending` 은 그 역요청을 건 세션이
    /// 끊는 중인가.
    pub fn check_start_child(&self, parent_ending: bool) -> Result<(), String> {
        self.check_alive(parent_ending)
    }

    /// `runInTerminal` 을 띄워도 되는가(머리 주석의 규칙). 되면 실행 관리자에 넘길 것, 아니면 어댑터에 돌려줄 까닭(영어 — 어댑터가
    /// 사람에게 보일 수 있다).
    pub fn check_run_in_terminal(
        &self,
        session_ending: bool,
        args: RunInTerminalRequestArguments,
    ) -> Result<TerminalRun, String> {
        self.check_alive(session_ending)?;
        if args.kind.as_deref().is_some_and(|k| k != "integrated") {
            return Err(format!(
                "runInTerminal kind `{}` is not supported; only the integrated terminal is",
                args.kind.as_deref().unwrap_or_default()
            ));
        }
        if args.args_can_be_interpreted_by_shell == Some(true) {
            return Err("arguments interpreted by a shell are not supported".to_owned());
        }
        if args.args.first().is_none_or(String::is_empty) {
            return Err("runInTerminal needs a command".to_owned());
        }
        let cwd = if args.cwd.is_empty() {
            self.inner.root.clone()
        } else {
            let cwd = PathBuf::from(&args.cwd);
            if !cwd.is_absolute() {
                return Err(format!(
                    "the working directory {:?} is not an absolute path",
                    args.cwd
                ));
            }
            match real_path(&cwd) {
                Some(real) if real.starts_with(&self.inner.root) => real,
                _ => {
                    return Err(format!(
                        "the working directory {:?} is outside the project root",
                        args.cwd
                    ));
                }
            }
        };
        Ok(TerminalRun {
            title: args.title,
            cwd,
            args: args.args,
            env: args.env.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
    use pw_core::config::AppPaths;
    use pw_core::registry::NewProject;

    struct Fixture {
        /// 시험이 쓰는 경로의 바탕(실제 경로 — macOS 의 `/var` → `/private/var` 를 푼 것).
        base: PathBuf,
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
            fs::create_dir_all(root.join("sub")).unwrap();
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
            base,
            _tmp: tmp,
            projects,
            trusted,
            untrusted,
            root,
        }
    }

    fn window(id: &str) -> ConnectionScope {
        ConnectionScope::Project(id.to_owned())
    }

    fn denied(result: Result<DebugGrant, RpcError>) -> Option<RpcError> {
        result.err().filter(|e| e.code == code::PERMISSION_DENIED)
    }

    /// F3: 신뢰한 프로젝트의 창만 허가를 받는다 — 미신뢰 · 에이전트 범위 · 다른 프로젝트 창 · Host 연결은 만들지 못한다.
    #[test]
    fn only_a_trusted_projects_own_window_gets_a_grant() {
        let f = fixture();
        let ok = debug_grant(&window(&f.trusted), &f.projects, &f.trusted, None).unwrap();
        assert_eq!(ok.root(), f.root);
        let untrusted = denied(debug_grant(
            &window(&f.untrusted),
            &f.projects,
            &f.untrusted,
            None,
        ))
        .unwrap();
        assert_eq!(untrusted.data.unwrap()["reason"], "untrusted");
        assert!(
            denied(debug_grant(
                &ConnectionScope::Agent,
                &f.projects,
                &f.trusted,
                None
            ))
            .is_some()
        );
        assert!(
            denied(debug_grant(
                &window(&f.untrusted),
                &f.projects,
                &f.trusted,
                None
            ))
            .is_some()
        );
        assert!(
            denied(debug_grant(
                &ConnectionScope::Host,
                &f.projects,
                &f.trusted,
                None
            ))
            .is_some()
        );
        let missing = debug_grant(&window("nope"), &f.projects, "nope", None).unwrap_err();
        assert_eq!(missing.code, code::NOT_FOUND);
    }

    fn grant(f: &Fixture) -> DebugGrant {
        debug_grant(&window(&f.trusted), &f.projects, &f.trusted, None).unwrap()
    }

    fn run(cwd: &str) -> RunInTerminalRequestArguments {
        RunInTerminalRequestArguments {
            cwd: cwd.to_owned(),
            args: vec!["node".into(), "app.js".into()],
            ..RunInTerminalRequestArguments::default()
        }
    }

    #[test]
    fn a_run_inside_the_root_passes_and_the_rest_is_refused() {
        let f = fixture();
        let g = grant(&f);
        let ok = g
            .check_run_in_terminal(false, run(f.root.join("sub").to_str().unwrap()))
            .unwrap();
        assert_eq!(ok.cwd, f.root.join("sub"));
        assert_eq!(ok.args, ["node", "app.js"]);
        assert_eq!(
            g.check_run_in_terminal(false, run("")).unwrap().cwd,
            g.root()
        );
        // 루트 밖 · 상대 경로 · `..` 로 나가기 · 없는 경로.
        for cwd in [
            f.base.to_str().unwrap().to_owned(),
            "sub".to_owned(),
            f.root.join("sub/../..").to_str().unwrap().to_owned(),
            f.root.join("nope").to_str().unwrap().to_owned(),
        ] {
            assert!(g.check_run_in_terminal(false, run(&cwd)).is_err(), "{cwd}");
        }
        let external = RunInTerminalRequestArguments {
            kind: Some("external".into()),
            ..run("")
        };
        assert!(g.check_run_in_terminal(false, external).is_err());
        let shell = RunInTerminalRequestArguments {
            args_can_be_interpreted_by_shell: Some(true),
            ..run("")
        };
        assert!(g.check_run_in_terminal(false, shell).is_err());
        let empty = RunInTerminalRequestArguments {
            args: vec![],
            ..run("")
        };
        assert!(g.check_run_in_terminal(false, empty).is_err());
        // 세션이 끝나면 거절.
        assert!(g.check_run_in_terminal(true, run("")).is_err());
        g.revoke();
        assert!(g.check_run_in_terminal(false, run("")).is_err());
    }

    /// R4: 루트를 실제 경로로 만들어, 글자 비교로 바꾸면 이 시험이 링크 때문에 실패하게 한다.
    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_root_is_outside() {
        let f = fixture();
        std::os::unix::fs::symlink(&f.base, f.root.join("out")).unwrap();
        let g = grant(&f);
        assert!(
            g.check_run_in_terminal(false, run(f.root.join("out").to_str().unwrap()))
                .is_err()
        );
        // 링크가 아닌 하위 폴더는 들어간다(같은 꼴의 경로가 받아지는 짝).
        assert!(
            g.check_run_in_terminal(false, run(f.root.join("sub").to_str().unwrap()))
                .is_ok()
        );
    }

    /// F4 · F3: `startDebugging` 은 끊는 중 · 거둔 허가 · 신뢰를 거둔 프로젝트에서 거절한다 — 셋을 따로 본다.
    #[test]
    fn a_child_session_needs_a_live_trusted_grant() {
        let f = fixture();
        let g = grant(&f);
        assert!(g.check_start_child(false).is_ok());
        assert!(g.check_start_child(true).is_err());
        f.projects.set_trust(&f.trusted, false).unwrap();
        assert!(g.check_start_child(false).is_err());
        assert!(g.check_run_in_terminal(false, run("")).is_err());
        f.projects.set_trust(&f.trusted, true).unwrap();
        assert!(g.check_start_child(false).is_ok());
        g.revoke();
        assert!(g.check_start_child(false).is_err());
    }
}
