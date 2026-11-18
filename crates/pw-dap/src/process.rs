//! 어댑터 프로세스를 띄우고 프로세스 트리째 끝낸다. `pw-lsp` 의 `process`(15-m4-plan.md M4-01 ④)와 같은 규칙이다 — 그쪽은 크레이트
//! 안(`pub(crate)`)이라 같은 꼴을 여기에 둔다(공용 크레이트 `pw-wire` 는 틀만 — 17-m6-plan.md 「공유 부품의 주인」).
//!
//! - unix: 어댑터를 새 프로세스 그룹의 리더로 띄운다(`process_group(0)`). 어댑터가 띄운 디버그 대상 · 도우미(lldb-server · node
//!   자식)는 그 그룹에 든다. 끝낼 때 `killpg(SIGKILL)`.
//! - Windows: Job Object(닫히면 안의 프로세스를 모두 끝냄)에 넣고 콘솔 창 없이 띄운다. 넣지 못하면 어댑터만 끝낸다.
//! - Linux: 띄우기는 프로세스 수명 동안 사는 스레드(`pw_spawn::on_launcher`)가 하고 `PR_SET_PDEATHSIG` 를 건다 — Host 가 죽으면
//!   어댑터도 끝난다.
//!
//! 자식은 앱 프로세스의 환경을 물려받지 않고 [`AdapterSpec::env`](런타임 환경)와 그 위에 덮을 변수만 받는다(03 「런타임」 — 모든
//! 프로세스 실행은 런타임 환경을 거친다).

use std::ffi::OsString;
use std::io;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;

use crate::adapter::AdapterSpec;

/// stderr(TCP 어댑터면 stdout 도) 한 줄에서 쥐는 바이트. 넘는 몫은 버린다.
pub const MAX_LOG_LINE: usize = 8 * 1024;

/// 어댑터 프로세스가 끝난 모양.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExit {
    /// 종료 코드. 신호로 끝났으면 없다.
    pub code: Option<i32>,
}

/// 떠 있는 어댑터 프로세스. TCP 어댑터면 그 프로세스에 붙은 세션 여럿(`startDebugging` 의 자식)이 나눠 쥔다.
#[derive(Debug)]
pub(crate) struct AdapterProcess {
    pid: Option<u32>,
    tree: ProcessTree,
    /// 기다리기 태스크가 거뒀다 — 그 뒤로는 그룹 id 가 다른 프로세스에 다시 쓰일 수 있어 신호를 보내지 않는다.
    reaped: Arc<AtomicBool>,
    exit: watch::Receiver<Option<ProcessExit>>,
}

/// 띄운 것: 프로세스와 (stdio 어댑터면) 그 stdin · stdout.
pub(crate) struct Spawned {
    pub process: Arc<AdapterProcess>,
    pub stdin: Option<tokio::process::ChildStdin>,
    pub stdout: Option<tokio::process::ChildStdout>,
}

impl AdapterProcess {
    /// `argv` 로 띄운다. `stdio` 면 stdin · stdout 이 프로토콜이고, 아니면 stdout 도 로그다.
    pub(crate) async fn spawn(
        spec: &AdapterSpec,
        args: Vec<OsString>,
        stdio: bool,
    ) -> io::Result<Spawned> {
        let spec = spec.clone();
        let (mut child, tree) = pw_spawn::on_launcher(move || spawn(&spec, &args, stdio)).await?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let label = pid.map_or_else(|| "?".to_owned(), |p| p.to_string());
        if let Some(stderr) = stderr {
            tokio::spawn(log_lines(stderr, label.clone(), "stderr"));
        }
        let stdout = if stdio {
            stdout
        } else {
            if let Some(stdout) = stdout {
                tokio::spawn(log_lines(stdout, label, "stdout"));
            }
            None
        };
        let (exit_tx, exit) = watch::channel(None);
        let reaped = Arc::new(AtomicBool::new(false));
        let process = Arc::new(Self {
            pid,
            tree,
            reaped: Arc::clone(&reaped),
            exit,
        });
        let tree = process.tree.clone_handle();
        tokio::spawn(async move {
            let status = child.wait().await;
            // 어댑터가 끝났다 — 그룹에 남은 자손(디버그 대상 · 도우미)을 끝내고, 그 뒤로는 신호를 보내지 않는다.
            if !reaped.load(Ordering::SeqCst) {
                tree.kill();
            }
            reaped.store(true, Ordering::SeqCst);
            let code = status
                .as_ref()
                .ok()
                .and_then(std::process::ExitStatus::code);
            tracing::debug!(target: "pw_dap", ?pid, ?status, "debug adapter exited");
            let _ = exit_tx.send(Some(ProcessExit { code }));
        });
        Ok(Spawned {
            process,
            stdin,
            stdout,
        })
    }

    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// 트리째 곧바로 끝낸다(거둔 뒤에는 아무것도 하지 않는다).
    pub(crate) fn kill(&self) {
        if !self.reaped.load(Ordering::SeqCst) {
            self.tree.kill();
        }
    }

    pub(crate) fn exited(&self) -> Option<ProcessExit> {
        self.exit.borrow().clone()
    }

    /// 끝날 때까지 기다린다.
    pub(crate) async fn wait(&self) -> ProcessExit {
        let mut exit = self.exit.clone();
        loop {
            if let Some(done) = exit.borrow_and_update().clone() {
                return done;
            }
            if exit.changed().await.is_err() {
                return ProcessExit { code: None };
            }
        }
    }
}

impl Drop for AdapterProcess {
    fn drop(&mut self) {
        // 손잡이를 모두 잃으면(세션이 버려짐) 트리째 끝낸다.
        self.kill();
    }
}

/// 줄마다 `tracing` 으로. 한 줄은 [`MAX_LOG_LINE`] 바이트까지.
async fn log_lines<R: AsyncRead + Unpin>(mut from: R, pid: String, stream: &'static str) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut line: Vec<u8> = Vec::new();
    let emit = |line: &mut Vec<u8>| {
        if !line.is_empty() {
            tracing::debug!(target: "pw_dap", pid = %pid, stream, line = %String::from_utf8_lossy(line));
            line.clear();
        }
    };
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                emit(&mut line);
            } else if line.len() < MAX_LOG_LINE {
                line.push(b);
            }
        }
    }
    emit(&mut line);
}

/// 어댑터와 그 자손 전체의 손잡이.
#[derive(Debug)]
pub(crate) struct ProcessTree {
    #[cfg(unix)]
    group: libc::pid_t,
    #[cfg(windows)]
    job: Option<Arc<job::Job>>,
}

impl ProcessTree {
    fn clone_handle(&self) -> Self {
        Self {
            #[cfg(unix)]
            group: self.group,
            #[cfg(windows)]
            job: self.job.clone(),
        }
    }

    fn kill(&self) {
        #[cfg(unix)]
        {
            // SAFETY: killpg(2) 는 신호를 보낼 뿐이다. 그룹이 비었으면 ESRCH 다.
            unsafe {
                libc::killpg(self.group, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
    }
}

/// 띄우기 스레드에서만 부른다(머리 주석).
fn spawn(spec: &AdapterSpec, args: &[OsString], stdio: bool) -> io::Result<(Child, ProcessTree)> {
    let mut command = Command::new(&spec.program);
    command
        .args(args)
        .current_dir(&spec.cwd)
        .env_clear()
        .env("PATH", &spec.env.path)
        .envs(&spec.env.vars)
        .envs(&spec.extra_env)
        .stdin(if stdio { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(target_os = "linux")]
    {
        // SAFETY: getpid(2) 는 메모리를 건드리지 않는다.
        let parent = unsafe { libc::getpid() };
        // SAFETY: fork 와 exec 사이에서 async-signal-safe 인 prctl(2) · getppid(2) 만 부른다.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::other(
                        "the host ended while the adapter was starting",
                    ));
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command.spawn()?;
    let tree = tree_of(&child)?;
    Ok((child, tree))
}

#[cfg(unix)]
fn tree_of(child: &Child) -> io::Result<ProcessTree> {
    let pid = child
        .id()
        .and_then(|pid| libc::pid_t::try_from(pid).ok())
        .ok_or_else(|| io::Error::other("the adapter exited before it could be tracked"))?;
    Ok(ProcessTree { group: pid })
}

#[cfg(windows)]
fn tree_of(child: &Child) -> io::Result<ProcessTree> {
    let job = child
        .raw_handle()
        .and_then(|process| match job::Job::assign(process) {
            Ok(job) => Some(Arc::new(job)),
            Err(e) => {
                tracing::warn!(target: "pw_dap", %e, "cannot put the debug adapter into a job object; stopping ends the adapter only");
                None
            }
        });
    Ok(ProcessTree { job })
}

#[cfg(not(any(unix, windows)))]
fn tree_of(_child: &Child) -> io::Result<ProcessTree> {
    Ok(ProcessTree {})
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::RawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// 닫히면 안의 프로세스를 모두 끝내는 Job.
    #[derive(Debug)]
    pub struct Job(HANDLE);

    // SAFETY: Job 핸들은 어느 스레드에서 써도 되는 커널 객체 핸들이다.
    unsafe impl Send for Job {}
    // SAFETY: `&self` 메서드는 커널 호출 하나씩이고 커널이 같은 핸들의 동시 호출을 직렬화한다.
    unsafe impl Sync for Job {}

    impl Job {
        pub fn assign(process: RawHandle) -> std::io::Result<Self> {
            // SAFETY: 이름 없는 Job 을 만든다. 실패하면 null 이다.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Self(handle);
            // SAFETY: 0 으로 채운 구조체는 「제한 없음」 이다.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `info` 의 크기와 종류를 맞춰 넘긴다.
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: 두 핸들 모두 살아 있다(`process` 는 tokio 의 Child 가 쥐고 있다).
            if unsafe { AssignProcessToJobObject(job.0, process as HANDLE) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }

        pub fn terminate(&self) {
            // SAFETY: 살아 있는 Job 핸들이다. 실패(이미 빈 Job)는 무시한다.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: 이 구조체만 이 핸들을 쥐고 있다.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
