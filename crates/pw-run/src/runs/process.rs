//! 실행의 종류 「프로세스」 — PTY 로 띄운 프로그램(메인 판정 M6-P2). 출력은 실행의 링 버퍼로 곧바로 옮긴다(소비자를 기다리지
//! 않는다 — 버퍼가 차면 앞을 버리므로 프로그램이 쓰기에서 멈추지 않는다).
//!
//! 끝: 프로그램(세션 리더)이 끝나면 남은 출력을 받고, 세션에 남은 프로세스(리더가 띄우고 두고 간 것)를 같은 중지 방법으로 끝낸 뒤
//! 「끝났다」 를 알린다 — 실행이 끝났는데 그 자식이 포트를 쥐고 남는 일이 없다. 중지는 세션 전체에 [`Ending`] 의 신호를 보낸다
//! (`crate::tree::end_sessions` — 그룹이 아니라 세션 id 로 찾으므로 자식이 따로 만든 프로세스 그룹도 닿고, 세션을 떠난 자손도
//! 따라간다).
//!
//! # 작업 폴더(M6-02 ⑦ · 리뷰 B1)
//!
//! 작업 폴더는 판정 때 정규화한 경로다. 그 뒤(로그인 셸 · mise 를 기다리는 동안 등) 경로의 한 구성 요소가 심링크로 바뀌거나 폴더가
//! 사라지면 다른 곳에서 돌 수 있다 — portable-pty 는 작업 폴더가 디렉터리가 아니면 **조용히 `$HOME` 에서 띄운다**. 열어 둔 폴더
//! 핸들로 띄울 수는 없다: portable-pty 는 작업 폴더를 경로로만 받고(`CommandBuilder::cwd`) 자식의 `pre_exec` 를 열지 않아
//! `fchdir` 를 넣을 자리가 없고, macOS 는 `chdir("/dev/fd/N")` 가 `ENOTDIR` 이다. 그래서 두 번 본다:
//!
//! 1. **띄우기 바로 앞** — 폴더를 열어 그 핸들의 실제 경로(macOS `F_GETPATH`, Linux `/proc/self/fd`)가 정규화한 경로와 같은지 보고,
//!    그 폴더의 (장치, inode)를 쥔다([`pin`]).
//! 2. **띄운 뒤, 프로그램이 돌기 전** — 프로그램을 `/bin/sh -c 'IFS= read -r _ && exec "$@"'` 로 감싸 띄운다. 띄우기가 돌아오면
//!    자식은 이미 `chdir` · `exec` 를 마쳤고 셸은 PTY 에서 한 줄을 기다린다. 그동안 그 프로세스의 실제 작업 폴더(macOS
//!    `proc_pidinfo`, Linux `/proc/<pid>/cwd`)의 (장치, inode)가 쥔 것과 같은지 본다. 같으면 에코를 끈 채 줄바꿈 하나를 넣어
//!    그 자리에서 프로그램을 `exec` 하게 하고(같은 pid — 그 줄은 출력에 남지 않는다), 다르면(바꿔치기 · `$HOME` 으로 떨어짐)
//!    SIGKILL 로 끝내고 실패한다 — 사람의 프로그램은 한 줄도 돌지 않는다. `kill -STOP $$` 로 멈췄다 SIGCONT 로 잇는 길은 쓰지
//!    않는다 — macOS 에서 수백 번에 한 번 SIGCONT 뒤에도 깨지 않고 잠든 채 남았다(실측).
//!
//! macOS · Linux 밖의 유닉스는 프로세스의 작업 폴더를 읽지 못해 1 만 한다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_rpc::RpcError;

use crate::pty::{CHUNK, Ending, Pty, Spec};

use super::{RunExit, RunKind, RunOutput, cwd_changed};

/// [`RunKind::kind`].
pub const KIND: &str = "process";

/// 리더가 끝난 뒤 남은 출력을 기다리는 시간(터미널의 `DRAIN_AFTER_EXIT` 와 같다). 세션을 떠난 데몬이 PTY 를 쥐고 있으면 EOF 가
/// 오지 않는다.
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(200);

/// 띄울 프로세스.
#[derive(Clone)]
pub struct ProcessSpec {
    /// 첫째가 프로그램(맨 이름이면 `env` 의 `PATH` 로 찾는다).
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// 자식의 환경 전체.
    pub env: Vec<(OsString, OsString)>,
    pub cols: u16,
    pub rows: u16,
    /// 중지의 방법.
    pub ending: Ending,
}

/// argv 원문 · env 값을 보이지 않는다(감사 · 로그에 명령줄 원문과 env 값을 남기지 않는다 — M5 의 규약).
impl std::fmt::Debug for ProcessSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessSpec")
            .field("argc", &self.argv.len())
            .field("cwd", &self.cwd)
            .field(
                "envKeys",
                &self.env.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field("size", &(self.cols, self.rows))
            .field("ending", &self.ending)
            .finish()
    }
}

/// 프로세스를 띄우는 것. 시험은 가짜를 넣어 불린 수를 센다(미신뢰 프로젝트는 0).
pub trait ProcessLauncher: Send + Sync + 'static {
    /// 띄우고 출력 · 끝을 `output` 으로 보낸다. tokio 런타임 안에서 불린다.
    fn launch(&self, spec: &ProcessSpec, output: RunOutput) -> Result<Arc<dyn RunKind>, RpcError>;
}

/// 프로그램을 감싸는 셸 글(머리 주석 「작업 폴더」 2). `$0` 다음이 프로그램의 argv 다.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const HOLD: &str = "IFS= read -r _ && exec \"$@\"";

/// PTY 로 띄운다.
pub struct PtyLauncher;

impl ProcessLauncher for PtyLauncher {
    fn launch(&self, spec: &ProcessSpec, output: RunOutput) -> Result<Arc<dyn RunKind>, RpcError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| RpcError::internal("runs must be started inside the async runtime"))?;
        #[cfg(unix)]
        let pinned = pin(&spec.cwd)?;
        #[cfg(all(test, unix))]
        hooks::before_spawn(&spec.cwd);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let argv: Vec<OsString> = ["/bin/sh", "-c", HOLD, "pitwall-run"]
            .into_iter()
            .map(OsString::from)
            .chain(spec.argv.iter().map(OsString::from))
            .collect();
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let argv: Vec<OsString> = spec.argv.iter().map(OsString::from).collect();
        let pty = Arc::new(
            Pty::spawn(&Spec {
                argv,
                cwd: spec.cwd.clone(),
                env: spec.env.clone(),
                cols: spec.cols,
                rows: spec.rows,
            })
            .map_err(RpcError::internal)?,
        );
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        release(&pty, &spec.cwd, pinned)?;
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
        let _ = pinned;
        let process = Arc::new(ProcessRun {
            pid: pty.pid(),
            pty: Mutex::new(Some(Arc::clone(&pty))),
            ending: spec.ending,
        });
        runtime.spawn(pump(pty, Arc::clone(&process), output));
        Ok(process)
    }
}

/// 머리 주석 「작업 폴더」 1: `cwd` 를 열어 그 실제 경로가 `cwd` 그대로인지 보고 그 폴더의 (장치, inode)를 준다.
#[cfg(unix)]
fn pin(cwd: &Path) -> Result<(u64, u64), RpcError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(cwd)
        .map_err(|e| cwd_changed(format!("the run's working directory cannot be opened: {e}")))?;
    let real = real_path(&dir, cwd);
    if real.as_deref() != Some(cwd) {
        return Err(cwd_changed(
            "the run's working directory changed since it was resolved".to_owned(),
        ));
    }
    let meta = dir
        .metadata()
        .map_err(|e| cwd_changed(format!("the run's working directory cannot be read: {e}")))?;
    Ok(crate::tree::dir_identity(&meta))
}

/// 열린 폴더의 실제 경로.
#[cfg(target_os = "macos")]
fn real_path(dir: &std::fs::File, _cwd: &Path) -> Option<PathBuf> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;
    let mut buf = vec![0u8; usize::try_from(libc::PATH_MAX).ok()? + 1];
    // SAFETY: F_GETPATH 는 MAXPATHLEN 바이트 버퍼에 NUL 로 끝나는 경로를 쓴다.
    if unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) } == -1 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
}

#[cfg(target_os = "linux")]
fn real_path(dir: &std::fs::File, _cwd: &Path) -> Option<PathBuf> {
    use std::os::fd::AsRawFd as _;
    std::fs::read_link(format!("/proc/self/fd/{}", dir.as_raw_fd())).ok()
}

/// 핸들의 경로를 읽지 못하는 유닉스: 지금 경로를 정규화한 것.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn real_path(_dir: &std::fs::File, cwd: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(cwd).ok()
}

/// 머리 주석 「작업 폴더」 2: 감싼 셸의 작업 폴더가 `pinned` 인지 보고 한 줄을 넣어 프로그램을 돌린다. 아니면 끝내고 실패한다.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn release(pty: &Pty, cwd: &Path, pinned: (u64, u64)) -> Result<(), RpcError> {
    let Some(pid) = pty.pid().and_then(|p| libc::pid_t::try_from(p).ok()) else {
        pty.terminate();
        return Err(RpcError::internal("the run has no process id"));
    };
    if crate::tree::cwd_identity(pid) != Some(pinned) {
        kill_now(pty, pid);
        tracing::warn!(target: "pw_run", cwd = %cwd.display(), "the run's process landed in another working directory; ended it");
        return Err(cwd_changed(
            "the run's working directory changed while it was being started".to_owned(),
        ));
    }
    pty.send_unechoed(b"\n").map_err(|e| {
        kill_now(pty, pid);
        RpcError::internal(e)
    })
}

/// 기다리는 셸을 곧바로 끝낸다(프로그램을 `exec` 하지 않는다).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn kill_now(pty: &Pty, pid: libc::pid_t) {
    // SAFETY: 위와 같다.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    pty.terminate();
}

/// 시험이 「띄우기 바로 앞」 과 「띄우기」 사이에 파일 시스템을 바꾸는 자리(작업 폴더마다 한 번).
#[cfg(all(test, unix))]
pub(crate) mod hooks {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};

    type Hook = Box<dyn FnOnce() + Send>;

    static BEFORE_SPAWN: Mutex<Vec<(PathBuf, Hook)>> = Mutex::new(Vec::new());

    pub(crate) fn on_spawn(cwd: &Path, hook: impl FnOnce() + Send + 'static) {
        BEFORE_SPAWN
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((cwd.to_path_buf(), Box::new(hook)));
    }

    pub(crate) fn before_spawn(cwd: &Path) {
        let hooks: Vec<Hook> = {
            let mut all = BEFORE_SPAWN.lock().unwrap_or_else(PoisonError::into_inner);
            all.extract_if(.., |(path, _)| path == cwd)
                .map(|(_, hook)| hook)
                .collect()
        };
        for hook in hooks {
            hook();
        }
    }
}

struct ProcessRun {
    pid: Option<u32>,
    /// 끝나면 비운다 — 끝난 실행이 목록에 남는 동안 PTY 의 fd 를 쥐지 않게.
    pty: Mutex<Option<Arc<Pty>>>,
    ending: Ending,
}

impl ProcessRun {
    fn pty(&self) -> Option<Arc<Pty>> {
        self.pty
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl RunKind for ProcessRun {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn input(&self, data: Vec<u8>) -> Result<(), String> {
        if self.pty().is_some_and(|pty| pty.write(data)) {
            Ok(())
        } else {
            Err("the run is not running".to_owned())
        }
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self.pty() {
            Some(pty) => pty.resize(cols, rows),
            None => Ok(()),
        }
    }

    fn stop(&self) {
        if let Some(pty) = self.pty() {
            pty.end(self.ending);
        }
    }
}

/// 출력을 끝까지 옮기고 끝을 알린다(머리 주석 「끝」).
async fn pump(pty: Arc<Pty>, process: Arc<ProcessRun>, output: RunOutput) {
    let mut exited = false;
    loop {
        let next = if exited {
            tokio::time::timeout(DRAIN_AFTER_EXIT, pty.peek(CHUNK))
                .await
                .unwrap_or(None)
        } else {
            tokio::select! {
                next = pty.peek(CHUNK) => next,
                _ = pty.exited() => {
                    exited = true;
                    continue;
                }
            }
        };
        let Some(bytes) = next else {
            break;
        };
        output.write(&bytes);
        pty.consume(bytes.len());
    }
    let exit = pty.exit().await;
    // 리더가 끝났으면 세션에 남은 것도 끝낸다. 중지 중이면 이미 시작한 끝내기가 이어진다(두 번째 `end` 는 아무것도 하지 않는다).
    pty.end(process.ending);
    pty.ended().await;
    // 세션을 떠난 데몬이 PTY 를 쥐고 있으면 읽기 스레드가 EOF 를 못 본다 — 닫아 그 스레드도 끝낸다.
    pty.close_output();
    process
        .pty
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    drop(pty);
    output.exited(RunExit {
        code: exit.code,
        signal: exit.signal,
    });
}
