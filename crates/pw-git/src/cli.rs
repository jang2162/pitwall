//! git CLI 실행기(결정 M2-D8). 쓰기(ref · index · 작업 트리를 바꾸는 모든 것, 결정 M2-D1)와 gix 로 못 하는 읽기가 이것으로 돈다.
//!
//! - 실행 파일: 전역 설정 `[git] path`(절대 경로나 이름)가 있으면 그것, 없으면 `git`. 이름은 런타임 환경의 PATH 에서 찾는다
//!   (`pw_open_with::launch::find_program`). 런타임 환경은 `pw_runtime::Runtime::env` 가 정한다 — 신뢰 전에는 로그인 셸만(D35).
//! - 버전: 쓰기 전에 `git --version` 을 읽어 [`MIN_VERSION`] 미만이거나 읽지 못하면 쓰기를 거절한다(`crate::Git::command`). 그
//!   판정 · 알림 한 번은 `lib.rs` 의 몫이고, 여기는 버전을 읽고 비교하는 것까지다.
//! - 환경: 런타임 환경 전체 + [`child_env`] 의 고정 변수. 저장소 위치를 바꾸는 변수([`REPO_LOCATION_VARS`])는 뺀다 — 앱을 git 훅
//!   안에서 띄웠으면 `GIT_DIR` 이 남아 해석한 저장소와 다른 저장소를 건드린다.
//! - 출력: 기계 형식(`--porcelain` · `-z`)만 해석한다. 사람용 출력은 언어 · 버전마다 달라진다. 예외는 `git --version` 의 첫
//!   줄뿐이다(형식이 고정이고 번역되지 않는다). stderr 는 해석하지 않고 원문 그대로 사람에게 보인다.
//! - 자식은 새 세션(unix `setsid`)이라 제어 터미널이 없다 — `ssh` 가 터미널에서 암호를 묻느라 멈추지 않는다. 취소는 그 세션의
//!   프로세스 그룹 전체에 SIGTERM, [`TERM_GRACE`] 뒤 SIGKILL 이다. SIGTERM 을 먼저 주는 것은 git 이 그 신호에서 `index.lock` 등
//!   잠금 파일을 지우고 끝나기 때문이다(SIGKILL 이면 잠금이 남아 다음 git 이 실패한다). Windows 는 자식만 끝낸다.
//! - 인증: askpass 도우미는 M2-06 이 만든다. 여기는 그 경로를 받는 자리([`child_env`] 의 `askpass`)만 있다.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_runtime::RuntimeEnv;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

/// 쓰기를 허락하는 가장 낮은 git 버전(02-tech-stack.md 「Git: gitoxide + git CLI 하이브리드」).
pub const MIN_VERSION: GitVersion = GitVersion {
    major: 2,
    minor: 40,
    patch: 0,
};

/// 전역 설정(`settings`)에서 git 실행 파일을 덮는 키.
pub const SETTINGS_KEY: &str = "git.path";

/// 취소할 때 SIGTERM 뒤 SIGKILL 까지 기다리는 시간.
pub const TERM_GRACE: Duration = Duration::from_millis(300);

/// git 이 끝난 뒤 파이프의 나머지를 기다리는 시간. 훅이 띄운 프로세스가 파이프를 쥐고 있어도 그 뒤로는 기다리지 않는다.
pub const EXIT_GRACE: Duration = Duration::from_millis(200);

/// `git --version` 을 기다리는 시간.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// 런타임 환경에서 빼는 변수: git 이 찾을 저장소 · index · 객체의 자리를 바꾼다.
pub const REPO_LOCATION_VARS: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
];

/// 모든 명령에 덮는 변수. 터미널 프롬프트를 끄고(인증은 askpass 로만), 편집기를 띄우지 않는다 — 메시지가 없는 커밋은 「빈
/// 메시지」 로 실패하고, merge · rebase 는 기본 메시지를 그대로 쓴다.
pub const FIXED_VARS: [(&str, &str); 4] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_EDITOR", ":"),
    ("GIT_SEQUENCE_EDITOR", ":"),
    ("GIT_MERGE_AUTOEDIT", "no"),
];

/// 읽는 명령에만 덮는 변수. `git status` 등이 index 를 몰래 새로 쓰지(잠금 파일을 만들지) 않게 한다 — 사람이 터미널에서 돌리는
/// git 과 잠금으로 부딪히지 않는다.
pub const READ_VARS: [(&str, &str); 1] = [("GIT_OPTIONAL_LOCKS", "0")];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    /// `git --version` 의 출력(`git version 2.54.0 (Apple Git-157)`, `git version 2.45.1.windows.1`). 패치가 없으면 0 이다.
    pub fn parse(output: &str) -> Option<Self> {
        let rest = output.lines().next()?.trim().strip_prefix("git version ")?;
        let mut parts = rest
            .split(|c: char| !c.is_ascii_digit())
            .take_while(|p| !p.is_empty());
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 명령이 저장소를 바꾸는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// 자식 환경 전체(PATH 포함). `askpass` 는 M2-06 의 도우미 경로다.
pub fn child_env(
    base: &RuntimeEnv,
    access: Access,
    askpass: Option<&Path>,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = base
        .vars
        .iter()
        .filter(|(name, _)| !REPO_LOCATION_VARS.iter().any(|v| same_name(name, v)))
        .filter(|(name, _)| !FIXED_VARS.iter().any(|(v, _)| same_name(name, v)))
        .filter(|(name, _)| !READ_VARS.iter().any(|(v, _)| same_name(name, v)))
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect();
    env.push(("PATH".into(), base.path.clone()));
    let read = match access {
        Access::Read => &READ_VARS[..],
        Access::Write => &[],
    };
    for (name, value) in FIXED_VARS.iter().chain(read) {
        env.push((name.into(), value.into()));
    }
    if let Some(askpass) = askpass {
        env.retain(|(n, _)| {
            !["GIT_ASKPASS", "SSH_ASKPASS", "SSH_ASKPASS_REQUIRE"]
                .iter()
                .any(|v| same_name(n, v))
        });
        env.push(("GIT_ASKPASS".into(), askpass.into()));
        env.push(("SSH_ASKPASS".into(), askpass.into()));
        env.push(("SSH_ASKPASS_REQUIRE".into(), "force".into()));
    }
    env
}

/// Windows 의 환경 변수 이름은 대소문자를 가리지 않는다.
fn same_name(name: &OsStr, other: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(other)
    } else {
        name == other
    }
}

/// 띄울 준비가 된 git 명령. `crate::Git::command` 가 만든다.
#[derive(Debug, Clone)]
pub struct GitCommand {
    program: PathBuf,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
    access: Access,
    args: Vec<OsString>,
}

/// 끝난 git 의 출력.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    /// `-z` 출력의 레코드(NUL 로 나눈 것). 끝의 빈 레코드는 없다.
    pub fn records(&self) -> impl Iterator<Item = &[u8]> {
        let body = self.stdout.strip_suffix(b"\0").unwrap_or(&self.stdout);
        body.split(|&b| b == 0).filter(move |_| !body.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// 띄우지 못했다.
    Spawn(String),
    /// 부른 쪽이 취소했다(프로세스 그룹째 끝냈다).
    Cancelled,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "cannot run git: {e}"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl GitCommand {
    pub(crate) fn new(
        program: PathBuf,
        cwd: PathBuf,
        env: Vec<(OsString, OsString)>,
        access: Access,
    ) -> Self {
        Self {
            program,
            cwd,
            env,
            access,
            args: Vec::new(),
        }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn access(&self) -> Access {
        self.access
    }

    /// 자식 환경 전체.
    pub fn env(&self) -> &[(OsString, OsString)] {
        &self.env
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// 띄워 끝날 때까지 기다린다. `cancel` 이 오면 프로세스 그룹을 끝내고 [`RunError::Cancelled`]. 이 future 를 끝나기 전에
    /// 버려도 그룹에 SIGTERM 을 보낸다.
    pub async fn output(self, cancel: &CancellationToken) -> Result<Output, RunError> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .env_clear()
            .envs(self.env.iter().map(|(n, v)| (n, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        // SAFETY: fork 와 exec 사이에서 async-signal-safe 인 setsid(2) 하나만 부른다.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|e| RunError::Spawn(e.to_string()))?;
        let mut guard = GroupGuard(child.id());
        let stdout = collect(child.stdout.take());
        let stderr = collect(child.stderr.take());
        let status = tokio::select! {
            status = child.wait() => status.map_err(|e| RunError::Spawn(e.to_string()))?,
            () = cancel.cancelled() => {
                terminate(&mut child, guard.0.take()).await;
                return Err(RunError::Cancelled);
            }
        };
        guard.0 = None;
        let (stdout, stderr) = tokio::join!(stdout.finish(), stderr.finish());
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

/// 버려질 때 아직 끝나지 않은 프로세스 그룹에 SIGTERM 을 보낸다(기다리지 않는다). 정상으로 끝났거나 이미 끝냈으면 `None` 이다.
struct GroupGuard(Option<u32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            signal_group(pid, Signal::Term);
        }
    }
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

/// 세션(= 프로세스 그룹, `setsid` 로 자식 pid 가 그룹 id 다)에 신호를 보낸다. 이미 없으면 아무 일도 없다.
#[cfg(unix)]
fn signal_group(pid: u32, signal: Signal) {
    let Ok(pgid) = libc::pid_t::try_from(pid) else {
        return;
    };
    let signal = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // SAFETY: killpg(2) 는 신호를 보낼 뿐이다.
    unsafe {
        libc::killpg(pgid, signal);
    }
}

#[cfg(not(unix))]
fn signal_group(_pid: u32, _signal: Signal) {}

/// 취소: 그룹에 SIGTERM, git 이 [`TERM_GRACE`] 안에 끝나지 않으면 SIGKILL. 그 뒤 그룹에 남은 것(SIGTERM 을 무시한 훅의 자식 등)도
/// SIGKILL 로 끝낸다. 자식은 거둔다.
async fn terminate(child: &mut tokio::process::Child, pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    if cfg!(unix) {
        signal_group(pid, Signal::Term);
        let _ = tokio::time::timeout(TERM_GRACE, child.wait()).await;
        signal_group(pid, Signal::Kill);
    } else {
        let _ = child.start_kill();
    }
    let _ = child.wait().await;
}

/// 파이프 하나를 끝까지 읽어 모으는 작업. 읽은 데까지는 언제든 꺼낼 수 있다.
struct Collector {
    buf: Arc<Mutex<Vec<u8>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

fn collect(pipe: Option<impl AsyncRead + Unpin + Send + 'static>) -> Collector {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let task = pipe.map(|mut pipe| {
        let buf = Arc::clone(&buf);
        tokio::spawn(async move {
            let mut chunk = vec![0u8; 64 * 1024];
            while let Ok(n @ 1..) = pipe.read(&mut chunk).await {
                buf.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(&chunk[..n]);
            }
        })
    });
    Collector { buf, task }
}

impl Collector {
    /// git 이 끝난 뒤에 부른다. 파이프가 [`EXIT_GRACE`] 안에 닫히지 않으면(훅이 띄운 프로세스가 쥐고 있다) 읽은 데까지다.
    async fn finish(self) -> Vec<u8> {
        if let Some(mut task) = self.task
            && tokio::time::timeout(EXIT_GRACE, &mut task).await.is_err()
        {
            task.abort();
        }
        std::mem::take(&mut *self.buf.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
