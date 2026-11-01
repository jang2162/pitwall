//! 서버 프로세스를 띄우고 프로세스 트리째 끝낸다.
//!
//! - unix: 서버를 새 프로세스 그룹의 리더로 띄운다(`process_group(0)` — 그룹 id 가 서버의 pid). 서버가 띄운 것(`cargo check` ·
//!   proc-macro 서버 · Gradle 데몬 · tsserver)은 그 그룹에 든다. 끝낼 때 `killpg(SIGKILL)`. 스스로 `setsid` 로 떠난 데몬은 건드리지
//!   않는다(그 데몬의 정리는 그것을 띄운 서버의 몫이다).
//! - Windows: 서버를 Job Object(닫히면 안의 프로세스를 모두 끝냄)에 넣고, 콘솔 창 없이(`CREATE_NO_WINDOW`) 띄운다. 넣기 전에 서버가
//!   띄운 자식은 Job 밖이다 — 띄운 직후 넣는다. 넣지 못하면 서버만 끝낸다(pw-git 의 `cli::Tree` 와 같은 방식).
//!
//! 자식은 앱 프로세스의 환경을 물려받지 않고 [`LaunchSpec::env`](런타임 환경)만 받는다.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;

use pw_runtime::RuntimeEnv;
use tokio::process::{Child, Command};

/// 띄울 서버.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// 서버 id(정의의 id — 로그 파일 이름 · 로그 줄에 쓴다).
    pub server_id: String,
    /// 실행 파일. 해석(M4-02)이 끝난 절대 경로를 준다 — 이름만 주면 [`Self::env`] 의 PATH 로 찾는다.
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// 작업 디렉터리(작업 공간 루트).
    pub cwd: PathBuf,
    /// 그 프로젝트 · 디렉터리의 런타임 환경(`LspGrant::runtime_env` — D26). 자식은 이것만 받는다.
    pub env: RuntimeEnv,
    /// 런타임 환경 위에 덮을 변수(정의 · 설정의 `env` — M4-02).
    pub extra_env: BTreeMap<OsString, OsString>,
}

/// 서버와 그 자손 전체의 손잡이. 복제해 여러 곳(읽기 태스크 · 종료)이 쥔다.
#[derive(Debug)]
pub(crate) struct ProcessTree {
    #[cfg(unix)]
    group: libc::pid_t,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl ProcessTree {
    /// 트리 전체를 곧바로 끝낸다. 이미 없으면 아무 일도 없다.
    pub(crate) fn kill(&self) {
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

/// `spec` 을 띄운다. stdin · stdout · stderr 는 파이프다.
pub(crate) fn spawn(spec: &LaunchSpec) -> io::Result<(Child, ProcessTree)> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .env("PATH", &spec.env.path)
        .envs(&spec.env.vars)
        .envs(&spec.extra_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 손잡이를 잃으면(패닉 등) 서버라도 끝낸다. 트리는 `LanguageServer` 의 Drop 이 끝낸다.
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
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
        .ok_or_else(|| io::Error::other("the server exited before it could be tracked"))?;
    Ok(ProcessTree { group: pid })
}

#[cfg(windows)]
fn tree_of(child: &Child) -> io::Result<ProcessTree> {
    let job = child
        .raw_handle()
        .and_then(|process| match job::Job::assign(process) {
            Ok(job) => Some(job),
            Err(e) => {
                tracing::warn!(target: "pw_lsp", %e, "cannot put the language server into a job object; stopping ends the server only");
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
