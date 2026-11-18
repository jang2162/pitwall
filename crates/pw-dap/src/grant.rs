//! 디버그 세션의 실행 허가와 역요청 `runInTerminal` 의 판정(17-m6-plan.md M6-15 ③).
//!
//! 어댑터를 띄우는 것도, 어댑터가 `runInTerminal` 로 디버그 대상을 띄워 달라는 것도 프로세스 실행이다. 둘 다 `debug.start` 가 받은
//! [`ExecGrant`](그 프로젝트 창 — 03 「실행의 연결 범위」)로만 한다. 허가는 세션 나무에 묶인다:
//!
//! - 세션이 끝나면(끊기 시작 · 어댑터 죽음) 허가를 거둔다 — 그 뒤의 `runInTerminal` 은 거절한다.
//! - `cwd` 는 그 워크트리의 프로젝트 루트 안이어야 한다(심볼릭 링크를 푼 실제 경로로 본다). 비어 있으면 루트다.
//! - `kind = "external"`(OS 의 터미널 창)은 받지 않는다 — 실행 관리자의 PTY 로만 띄운다.
//! - `argsCanBeInterpretedByShell` 은 받지 않는다 — 알리지 않았고(`supportsArgsCanBeInterpretedByShell = false`), 셸 없이 argv 로
//!   띄운다.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pw_core::registry::exec::ExecGrant;

use crate::protocol::RunInTerminalRequestArguments;
use crate::terminal::TerminalRun;

/// 세션 나무 하나의 실행 허가.
#[derive(Debug, Clone)]
pub struct DebugGrant {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    exec: ExecGrant,
    /// 그 워크트리의 프로젝트 루트(실제 경로).
    root: PathBuf,
    revoked: AtomicBool,
}

impl DebugGrant {
    /// `exec` 는 `debug.start` 를 부른 연결이 그 프로젝트에서 받은 허가, `root` 는 고른 워크트리의 프로젝트 루트다. 루트가 없으면
    /// 오류.
    pub fn new(exec: ExecGrant, root: &Path) -> std::io::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        Ok(Self {
            inner: Arc::new(Inner {
                exec,
                root,
                revoked: AtomicBool::new(false),
            }),
        })
    }

    pub fn project_id(&self) -> &str {
        self.inner.exec.project_id()
    }

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

    /// 경로가 루트 안인가(실제 경로로 — 없는 경로는 아니다).
    pub fn contains(&self, path: &Path) -> bool {
        std::fs::canonicalize(path).is_ok_and(|real| real.starts_with(&self.inner.root))
    }

    /// `runInTerminal` 을 띄워도 되는가(머리 주석의 규칙). 되면 실행 관리자에 넘길 것, 아니면 어댑터에 돌려줄 까닭(영어 — 어댑터가
    /// 사람에게 보일 수 있다).
    pub fn check_run_in_terminal(
        &self,
        session_ending: bool,
        args: RunInTerminalRequestArguments,
    ) -> Result<TerminalRun, String> {
        if session_ending || self.revoked() {
            return Err("the debug session has ended".to_owned());
        }
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
            match std::fs::canonicalize(&cwd) {
                Ok(real) if real.starts_with(&self.inner.root) => real,
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
    use pw_core::registry::exec::{Launch, exec_grant};
    use pw_rpc::ConnectionScope;

    use super::*;

    fn grant(root: &Path) -> DebugGrant {
        let exec = exec_grant(
            &ConnectionScope::Project("p1".into()),
            "p1",
            &Launch::Process,
        )
        .unwrap();
        DebugGrant::new(exec, root).unwrap()
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
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let g = grant(&root);
        let ok = g
            .check_run_in_terminal(false, run(root.join("sub").to_str().unwrap()))
            .unwrap();
        assert_eq!(ok.cwd, std::fs::canonicalize(root.join("sub")).unwrap());
        assert_eq!(ok.args, ["node", "app.js"]);
        assert_eq!(
            g.check_run_in_terminal(false, run("")).unwrap().cwd,
            g.root()
        );
        // 루트 밖 · 상대 경로 · `..` 로 나가기 · 없는 경로.
        for cwd in [
            tmp.path().to_str().unwrap().to_owned(),
            "sub".to_owned(),
            root.join("sub/../..").to_str().unwrap().to_owned(),
            root.join("nope").to_str().unwrap().to_owned(),
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

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_root_is_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(tmp.path(), root.join("out")).unwrap();
        let g = grant(&root);
        assert!(
            g.check_run_in_terminal(false, run(root.join("out").to_str().unwrap()))
                .is_err()
        );
    }
}
