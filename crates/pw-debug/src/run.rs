//! 디버그 세션 = 실행 하나(메인 판정 M6-P15): 실행 관리자의 종류 갈래 「debug」([`RunKind`]) 와, 어댑터의 `runInTerminal` 을 그
//! 실행의 PTY 로 띄우는 것([`RunTerminals`]).
//!
//! - 콘솔 하나: 어댑터의 `output` 이벤트와 `runInTerminal` 로 띄운 디버그 대상의 PTY 출력이 같은 실행의 로그로 간다 — 디버그
//!   대상의 출력과 디버그가 한 탭이다.
//! - 입력 · 크기는 디버그 대상의 PTY 로(없으면 받지 않는다).
//! - 중지: 어댑터를 끊고(`launch` 면 대상도 끝내라고, `attach` 면 대상을 두고) 디버그 대상의 PTY 를 세션째 끝낸다. 실행의 끝은
//!   디버그 대상의 PTY 가 있으면 그 끝(그 프로세스 트리가 다 끝난 뒤 — 실행 관리자의 규칙), 없으면 끊기가 끝난 뒤다.
//! - 디버그 대상이 스스로 끝나면(PTY 의 끝) 실행이 끝나고, 어댑터도 끊는다.
//!
//! `runInTerminal` 의 판정은 셋이다: `pw-dap` 이 역요청마다 허가(창 범위 · 아직 신뢰 · 끝나지 않음)와 `cwd`(루트 안 실제 경로) ·
//! `kind`(external 거절)를 보고, 여기서 실행 관리자의 띄우기 규칙을 그대로 지난다 — 런타임 환경(D26) 위에 어댑터가 준 env, 셸 없이
//! argv, PTY(F7-3), 작업 폴더를 띄우기 바로 앞과 띄운 뒤 두 번 본다(`pw_run::runs::PtyLauncher` — M6-02 ⑦).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use pw_core::registry::exec::ExecGrant;
use pw_dap::protocol::Capabilities;
use pw_dap::terminal::TerminalFuture;
use pw_dap::{DebugSession, SessionId, TerminalRun, TerminalStarted, Terminals};
use pw_run::config::DebugMode;
use pw_run::pty::Ending;
use pw_run::runs::{ProcessLauncher, ProcessSpec, RunExit, RunId, RunKind, RunOutput};
use pw_runtime::Runtime;
use tokio::sync::Notify;

/// [`RunKind::kind`] — 실행 목록의 `kind`.
pub const KIND: &str = "debug";

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 디버그 세션 하나(실행 하나).
pub struct DebugRun {
    pub(crate) project_id: String,
    pub(crate) run_id: OnceLock<RunId>,
    pub(crate) adapter: String,
    pub(crate) mode: DebugMode,
    /// 그 워크트리의 프로젝트 루트(실제 경로) — 브레이크포인트의 상대 경로를 여기에 잇는다.
    pub(crate) root: PathBuf,
    pub(crate) output: RunOutput,
    pub(crate) session: OnceLock<Arc<DebugSession>>,
    pub(crate) capabilities: Mutex<Capabilities>,
    pub(crate) terminals: Mutex<Vec<Arc<dyn RunKind>>>,
    pub(crate) size: Mutex<(u16, u16)>,
    /// 끝내기를 시작하라(중지 · 어댑터의 `terminated` · 뿌리 세션의 끝).
    pub(crate) end: Notify,
    pub(crate) stopping: AtomicBool,
    pub(crate) finished: AtomicBool,
    /// 어댑터가 알린 디버그 대상의 종료 코드(`exited` 이벤트).
    pub(crate) debuggee_exit: Mutex<Option<i32>>,
    /// (세션, 어댑터의 브레이크포인트 id) → (상대 경로, 저장된 줄) — 어댑터가 `breakpoint` 이벤트로 줄을 옮기면 찾는다.
    pub(crate) ids: Mutex<HashMap<(SessionId, i64), (String, u32)>>,
}

impl DebugRun {
    pub(crate) fn new(
        project_id: String,
        adapter: String,
        mode: DebugMode,
        root: PathBuf,
        output: RunOutput,
        size: (u16, u16),
    ) -> Self {
        Self {
            project_id,
            run_id: OnceLock::new(),
            adapter,
            mode,
            root,
            output,
            session: OnceLock::new(),
            capabilities: Mutex::default(),
            terminals: Mutex::default(),
            size: Mutex::new(size),
            end: Notify::new(),
            stopping: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            debuggee_exit: Mutex::new(None),
            ids: Mutex::default(),
        }
    }

    pub fn run_id(&self) -> &str {
        self.run_id.get().map_or("", String::as_str)
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn session(&self) -> Option<&Arc<DebugSession>> {
        self.session.get()
    }

    /// 콘솔에 쓴다(줄바꿈은 터미널의 `\r\n` 으로).
    pub(crate) fn console(&self, text: &str) {
        let mut out = String::with_capacity(text.len() + 8);
        let mut prev = '\0';
        for c in text.chars() {
            if c == '\n' && prev != '\r' {
                out.push('\r');
            }
            out.push(c);
            prev = c;
        }
        self.output.write(out.as_bytes());
    }

    /// 디버그 대상의 PTY 들을 끝낸다. 하나라도 있었으면 참 — 그 PTY 의 끝이 실행의 끝을 알린다.
    pub(crate) fn stop_terminals(&self) -> bool {
        let terminals = lock(&self.terminals).clone();
        for t in &terminals {
            t.stop();
        }
        !terminals.is_empty()
    }
}

impl RunKind for DebugRun {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn pid(&self) -> Option<u32> {
        lock(&self.terminals)
            .first()
            .and_then(|t| t.pid())
            .or_else(|| self.session.get().and_then(|s| s.root().pid()))
    }

    fn input(&self, data: Vec<u8>) -> Result<(), String> {
        match lock(&self.terminals).first() {
            Some(t) => t.input(data),
            None => Err("this debug session has no program console to type into".to_owned()),
        }
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        *lock(&self.size) = (cols, rows);
        for t in lock(&self.terminals).iter() {
            t.resize(cols, rows)?;
        }
        Ok(())
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        // 영구 표지(permit) — 끝내기를 기다리는 쪽이 아직 없어도(시작 중) 잃지 않는다.
        self.end.notify_one();
    }
}

/// 어댑터의 `runInTerminal` 을 그 디버그 실행의 PTY 로 띄운다(머리 주석).
pub(crate) struct RunTerminals {
    pub(crate) run: Weak<DebugRun>,
    pub(crate) runtime: Arc<Runtime>,
    pub(crate) launcher: Arc<dyn ProcessLauncher>,
    pub(crate) grace: Duration,
}

impl Terminals for RunTerminals {
    fn run_in_terminal<'a>(&'a self, grant: &'a ExecGrant, run: TerminalRun) -> TerminalFuture<'a> {
        Box::pin(async move {
            let debug = self
                .run
                .upgrade()
                .ok_or_else(|| "the debug session has ended".to_owned())?;
            if debug.stopping.load(Ordering::SeqCst) || debug.finished.load(Ordering::SeqCst) {
                return Err("the debug session is ending".to_owned());
            }
            if grant.project_id() != debug.project_id {
                return Err("the debug grant is for another project".to_owned());
            }
            let runtime = Arc::clone(&self.runtime);
            let launcher = Arc::clone(&self.launcher);
            let grace = self.grace;
            let target = Arc::clone(&debug);
            let kind = tokio::task::spawn_blocking(move || {
                // 그 작업 폴더의 런타임 환경(D26 — 실행 관리자와 같은 조회) 위에 어댑터가 준 변수(`null` 은 지움).
                let mut env = target.runtime_env(&runtime, &run.cwd);
                for (name, value) in &run.env {
                    match (name.as_str(), value) {
                        ("PATH", Some(v)) => env.path = v.into(),
                        ("PATH", None) => env.path = Default::default(),
                        (_, Some(v)) => {
                            env.vars.insert(name.into(), v.into());
                        }
                        (_, None) => {
                            env.vars.remove(std::ffi::OsStr::new(name));
                        }
                    }
                }
                let (cols, rows) = *lock(&target.size);
                let spec = ProcessSpec {
                    argv: run.args,
                    cwd: run.cwd,
                    env: pw_run::shell::child_env(&env),
                    cols,
                    rows,
                    ending: Ending::Terminate { grace },
                };
                launcher
                    .launch(&spec, target.output.clone())
                    .map_err(|e| e.message)
            })
            .await
            .map_err(|e| e.to_string())??;
            let process_id = kind.pid();
            lock(&debug.terminals).push(kind);
            let run_id = debug.run_id().to_owned();
            tracing::info!(target: "pw_debug", run = %run_id, pid = ?process_id, "debug target started in the run console");
            Ok(TerminalStarted {
                process_id,
                shell_process_id: None,
            })
        })
    }
}

impl DebugRun {
    fn runtime_env(&self, runtime: &Runtime, cwd: &std::path::Path) -> pw_runtime::RuntimeEnv {
        runtime.env(&self.project_id, cwd).env.clone()
    }
}

/// 실행의 끝(디버그 대상의 PTY 가 없을 때): 어댑터가 알린 대상의 종료 코드, 없으면 어댑터의 것.
pub(crate) fn exit_of(run: &DebugRun, adapter: Option<i32>) -> RunExit {
    RunExit {
        code: lock(&run.debuggee_exit).or(adapter),
        signal: None,
    }
}
