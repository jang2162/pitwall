//! git CLI 실행기(결정 M2-D8). 쓰기(ref · index · 작업 트리를 바꾸는 모든 것, 결정 M2-D1)와 gix 로 못 하는 읽기가 이것으로 돈다.
//!
//! - 신뢰: 읽기는 신뢰한 프로젝트에서만 띄운다(`crate::Git::command`). git 은 읽는 명령에서도 저장소 설정이 정한 프로그램을 돌린다
//!   (`core.fsmonitor` 는 `git status` 만으로, `filter.<x>.clean` · `process` 는 stat 이 바뀐 파일에, `diff.external` · `textconv` 는
//!   diff 에) — 미신뢰 프로젝트에서 자동으로 도는 읽기가 그 코드를 돌리지 않게, 미신뢰면 읽기는 gix 로만 한다(03-architecture.md
//!   「보안」). 사람이 누른 쓰기는 신뢰와 상관없이 돈다 — 터미널에서 `git commit` 을 친 것과 같다(훅 · 필터 포함).
//!
//! - 실행 파일: 전역 설정 `[git] path`(절대 경로나 이름)가 있으면 그것, 없으면 `git`. 이름은 런타임 환경의 PATH 에서 찾는다
//!   (`pw_runtime::program::find_program`). 런타임 환경은 `pw_runtime::Runtime::env` 가 정한다 — 신뢰 전에는 로그인 셸만(D35).
//! - 버전: 쓰기 전에 `git --version` 을 읽어 [`MIN_VERSION`] 미만이거나 읽지 못하면 쓰기를 거절한다(`crate::Git::command`). 그
//!   판정 · 알림 한 번은 `lib.rs` 의 몫이고, 여기는 버전을 읽고 비교하는 것까지다.
//! - 환경: 런타임 환경 전체 + [`child_env`] 의 고정 변수. 저장소 위치 · 실행할 프로그램 · 명령의 뜻을 바꾸는 변수([`is_dropped`])는
//!   뺀다 — 앱을 git 훅 안에서 띄웠으면 `GIT_DIR` 이 남아 해석한 저장소와 다른 저장소를 건드린다.
//! - 출력: 기계 형식(`--porcelain` · `-z`)만 해석한다. 사람용 출력은 언어 · 버전마다 달라진다. 예외는 `git --version` 의 첫
//!   줄뿐이다(형식이 고정이고 번역되지 않는다). stderr 는 해석하지 않고 원문 그대로 사람에게 보인다.
//! - 자식은 새 세션(unix `setsid`)이라 제어 터미널이 없다 — `ssh` 가 터미널에서 암호를 묻느라 멈추지 않는다. 취소는 그 세션의
//!   프로세스 그룹 전체에 SIGTERM, [`TERM_GRACE`] 뒤 SIGKILL 이다. SIGTERM 을 먼저 주는 것은 git 이 그 신호에서 `index.lock` 등
//!   잠금 파일을 지우고 끝나기 때문이다(SIGKILL 이면 잠금이 남아 다음 git 이 실패한다). 취소 없이 future 를 버려도 같다(SIGKILL 은
//!   떼어 낸 스레드가 보낸다). Windows 는 자식을 콘솔 창 없이(`CREATE_NO_WINDOW`) 띄우고 Job Object 에 넣어, 취소 · 버림에서 그
//!   Job 의 프로세스 전체를 끝낸다(SIGTERM 에 해당하는 단계는 없다). 정상으로 끝나면 그룹 · Job 에 남은 것(git 이 떼어 띄운 `gc
//!   --auto` 등)은 건드리지 않는다.
//! - 입력: stdin 은 기본으로 비어 있다(null). [`GitCommand::stdin`] 으로 준 바이트만 넘기고 닫는다(`git apply --cached` 의 패치,
//!   `git commit -F -` 의 메시지).
//! - 인증: askpass 도우미는 쓰기 실행기(`crate::write::askpass`)가 만들어 쓰기 명령의 환경에 넣는다([`set_askpass`]).

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_runtime::RuntimeEnv;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
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

/// 런타임 환경에서 빼는 변수. 사람의 셸 · 앱을 띄운 git 훅 · 에이전트가 남긴 값이 해석한 저장소나 명령의 뜻을 바꾸지 않게 한다.
///
/// - git 이 저장소에 딸린 것으로 보는 변수 전부(`git rev-parse --local-env-vars`, git 2.54 의 15개)와 `GIT_NAMESPACE`: 저장소 · index ·
///   객체 · ref 의 자리, `-c` 로 넣은 설정(`GIT_CONFIG_PARAMETERS` · `GIT_CONFIG_COUNT` 와 [`DROPPED_PREFIXES`]).
/// - 띄울 프로그램을 바꾸는 것: `GIT_EXEC_PATH` · `GIT_EXTERNAL_DIFF` · `GIT_SSH*` · `GIT_ASKPASS`(askpass 는 M2-06 이 [`child_env`]
///   로 준다).
/// - 저장소 찾기를 바꾸는 것(gix 의 찾기와 갈라진다)과 기록 · pathspec 의 뜻을 바꾸는 것(커밋 시각, reflog 이유, quarantine,
///   `GIT_*_PATHSPECS`).
///
/// 사람의 설정 파일 자리(`GIT_CONFIG_GLOBAL` · `GIT_CONFIG_SYSTEM` · `GIT_CONFIG_NOSYSTEM`)와 신원(`GIT_AUTHOR_NAME` 등)은 둔다 —
/// 터미널의 git 과 같은 설정 · 신원을 쓴다.
pub const DROPPED_VARS: [&str; 32] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_EXEC_PATH",
    "GIT_EXTERNAL_DIFF",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_SSH_VARIANT",
    "GIT_ASKPASS",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_DATE",
    "GIT_REFLOG_ACTION",
    "GIT_QUARANTINE_PATH",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
];

/// 이름이 이것으로 시작하면 뺀다: `GIT_CONFIG_COUNT` 와 짝을 이루는 `-c` 설정(`GIT_CONFIG_KEY_<n>` · `GIT_CONFIG_VALUE_<n>`).
pub const DROPPED_PREFIXES: [&str; 2] = ["GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"];

/// 자식 환경에서 빼는 변수인가([`DROPPED_VARS`] · [`DROPPED_PREFIXES`]). 시험 도우미도 같은 판정으로 지운다.
pub fn is_dropped(name: &OsStr) -> bool {
    DROPPED_VARS.iter().any(|v| same_name(name, v))
        || name.to_str().is_some_and(|name| {
            DROPPED_PREFIXES.iter().any(|p| {
                name.get(..p.len())
                    .is_some_and(|head| same_name(OsStr::new(head), p))
            })
        })
}

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
        .filter(|(name, _)| !is_dropped(name))
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
        set_askpass(&mut env, askpass);
    }
    env
}

/// 환경의 askpass 를 `askpass` 로 바꾼다: git(`GIT_ASKPASS`)과 ssh(`SSH_ASKPASS`, 터미널이 있어도 쓰게 `SSH_ASKPASS_REQUIRE=force`)가
/// 암호 · 확인을 그 프로그램에 묻는다.
pub fn set_askpass(env: &mut Vec<(OsString, OsString)>, askpass: &Path) {
    env.retain(|(n, _)| {
        !["GIT_ASKPASS", "SSH_ASKPASS", "SSH_ASKPASS_REQUIRE"]
            .iter()
            .any(|v| same_name(n, v))
    });
    env.push(("GIT_ASKPASS".into(), askpass.into()));
    env.push(("SSH_ASKPASS".into(), askpass.into()));
    env.push(("SSH_ASKPASS_REQUIRE".into(), "force".into()));
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
    stdin: Option<Vec<u8>>,
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
            stdin: None,
        }
    }

    /// 자식 환경 전체를 바꾼다(쓰기 실행기가 askpass 를 넣는다).
    pub(crate) fn with_env(mut self, env: Vec<(OsString, OsString)>) -> Self {
        self.env = env;
        self
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

    /// 받은 인자.
    pub fn arguments(&self) -> &[OsString] {
        &self.args
    }

    /// git 의 stdin 으로 `input` 을 쓰고 닫는다. 주지 않으면 stdin 은 비어 있다(null). git 이 다 읽지 않고 끝나도(파이프가 닫혀도)
    /// 실패로 보지 않는다 — 결과는 git 의 종료 코드 · 출력으로 판정한다.
    #[must_use]
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// 띄워 끝날 때까지 기다린다. `cancel` 이 오면 프로세스 그룹을 끝내고 [`RunError::Cancelled`]. 이 future 를 끝나기 전에
    /// 버려도 그룹에 SIGTERM 을 보낸다.
    pub async fn output(self, cancel: &CancellationToken) -> Result<Output, RunError> {
        let mut running = self.spawn()?;
        let stdout = collect(running.stdout.take());
        running.finish(cancel, Some(stdout)).await
    }

    /// 띄우고 곧바로 돌려준다 — stdout 은 부른 쪽이 [`Running::stdout`] 으로 읽는다(출력이 끝나기 전에 처리하는 스트림 읽기,
    /// `git blame --incremental`). 부른 쪽이 읽지 않으면 파이프가 차서 git 이 멈춘다 — 받는 쪽이 느린 만큼 git 도 느려진다.
    /// stderr 는 모은다. [`Running`] 을 끝나기 전에 버리면 [`GitCommand::output`] 처럼 프로세스 그룹에 SIGTERM 을 보낸다.
    pub fn spawn(self) -> Result<Running, RunError> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .env_clear()
            .envs(self.env.iter().map(|(n, v)| (n, v)))
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
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
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|e| RunError::Spawn(e.to_string()))?;
        let guard = GroupGuard(Tree::of(&child));
        if let (Some(input), Some(mut pipe)) = (self.stdin, child.stdin.take()) {
            // 다 쓰면 닫는다(버려 EOF). 쓰기 오류(git 이 먼저 끝나 파이프가 닫힘)는 무시한다.
            tokio::spawn(async move {
                let _ = pipe.write_all(&input).await;
            });
        }
        let stdout = child.stdout.take();
        let stderr = collect(child.stderr.take());
        Ok(Running {
            child,
            guard,
            stdout,
            stderr,
        })
    }
}

/// 띄운 git([`GitCommand::spawn`]).
pub struct Running {
    child: tokio::process::Child,
    guard: GroupGuard,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Collector,
}

impl Running {
    /// git 의 stdout. 한 번만 꺼낼 수 있다.
    pub fn stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.stdout.take()
    }

    /// 끝날 때까지 기다린다. 결과의 `stdout` 은 비어 있다(부른 쪽이 읽었다). 취소는 [`GitCommand::output`] 과 같다.
    pub async fn wait(self, cancel: &CancellationToken) -> Result<Output, RunError> {
        self.finish(cancel, None).await
    }

    async fn finish(
        mut self,
        cancel: &CancellationToken,
        stdout: Option<Collector>,
    ) -> Result<Output, RunError> {
        let status = tokio::select! {
            status = self.child.wait() => status.map_err(|e| RunError::Spawn(e.to_string()))?,
            () = cancel.cancelled() => {
                terminate(&mut self.child, self.guard.0.take()).await;
                return Err(RunError::Cancelled);
            }
        };
        if let Some(tree) = self.guard.0.take() {
            tree.release();
        }
        let stdout = async {
            match stdout {
                Some(stdout) => stdout.finish().await,
                None => Vec::new(),
            }
        };
        let (stdout, stderr) = tokio::join!(stdout, self.stderr.finish());
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

/// 버려질 때 아직 끝나지 않은 프로세스 트리를 끝낸다(기다리지 않는다). unix 는 SIGTERM 을 보내고 [`TERM_GRACE`] 뒤 떼어 낸
/// 스레드가 SIGKILL 을 보낸다 — SIGTERM 을 무시하는 훅의 자식이 고아로 남지 않는다. 정상으로 끝났거나 이미 끝냈으면 `None` 이다.
struct GroupGuard(Option<Tree>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(tree) = self.0.take() {
            tree.abandon();
        }
    }
}

/// git 과 그것이 띄운 프로세스 전체(훅 · ssh · credential helper)의 손잡이. unix 는 git 의 세션(= 프로세스 그룹, `setsid` 로 git 의
/// pid 가 그룹 id 다).
#[cfg(unix)]
struct Tree(libc::pid_t);

#[cfg(unix)]
impl Tree {
    fn of(child: &tokio::process::Child) -> Option<Self> {
        child
            .id()
            .and_then(|pid| libc::pid_t::try_from(pid).ok())
            .map(Self)
    }

    /// 그룹에 신호를 보낸다. 이미 없으면 아무 일도 없다.
    fn signal(&self, signal: libc::c_int) {
        // SAFETY: killpg(2) 는 신호를 보낼 뿐이다.
        unsafe {
            libc::killpg(self.0, signal);
        }
    }

    fn term(&self) {
        self.signal(libc::SIGTERM);
    }

    fn kill(&self) {
        self.signal(libc::SIGKILL);
    }

    /// 정상으로 끝났다 — 그룹에 남은 것은 두고 손잡이만 놓는다.
    fn release(self) {}

    /// 기다리지 않고 끝낸다: SIGTERM, [`TERM_GRACE`] 뒤 SIGKILL(떼어 낸 스레드). 그 사이 그룹이 다 끝나면 그룹 id 가 다른 그룹에
    /// 다시 쓰일 수 있으나, 남은 프로세스가 하나라도 있으면 그 id 는 다시 쓰이지 않는다.
    fn abandon(self) {
        self.term();
        let spawned = std::thread::Builder::new()
            .name("pw-git-kill".to_owned())
            .spawn(move || {
                std::thread::sleep(TERM_GRACE);
                self.kill();
            });
        if let Err(e) = spawned {
            tracing::warn!(target: "pw_git", %e, "cannot start the kill thread; killing the git process group now");
        }
    }
}

/// Windows: git 을 넣은 Job Object. 넣기 전에 git 이 띄운 자식은 Job 밖이다 — git 이 자식을 띄우기 전(띄운 직후)에 넣는다. Job 에
/// 넣지 못하면(오래된 Windows 의 중첩 Job 제한 등) git 만 끝낸다.
#[cfg(windows)]
struct Tree(Option<job::Job>);

#[cfg(windows)]
impl Tree {
    fn of(child: &tokio::process::Child) -> Option<Self> {
        let job = child.raw_handle().and_then(|process| match job::Job::assign(process) {
            Ok(job) => Some(job),
            Err(e) => {
                tracing::warn!(target: "pw_git", %e, "cannot put git into a job object; cancelling ends git only");
                None
            }
        });
        Some(Self(job))
    }

    fn term(&self) {
        self.kill();
    }

    fn kill(&self) {
        if let Some(job) = &self.0 {
            job.terminate();
        }
    }

    fn release(self) {
        if let Some(job) = self.0 {
            job.release();
        }
    }

    /// Job 을 닫으면 그 안의 프로세스 전체가 끝난다(`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`).
    fn abandon(self) {}
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

    /// 닫히면 안의 프로세스를 모두 끝내는 Job. [`Job::release`] 로 그 제한을 풀고 닫을 수 있다.
    pub struct Job(HANDLE);

    // SAFETY: Job 핸들은 어느 스레드에서 써도 되는 커널 객체 핸들이다.
    unsafe impl Send for Job {}
    // SAFETY: `&self` 메서드는 커널 호출(`TerminateJobObject` · `SetInformationJobObject`) 하나씩이고, 커널이 같은 핸들의 동시 호출을
    // 직렬화한다. 취소가 `&Tree` 를 await 너머로 쥐므로 `GitCommand::output` 의 future 가 `Send` 이려면 필요하다(쓰기 실행기가 그
    // future 를 다른 태스크로 보낸다).
    unsafe impl Sync for Job {}

    impl Job {
        pub fn assign(process: RawHandle) -> std::io::Result<Self> {
            // SAFETY: 이름 없는 Job 을 만든다. 실패하면 null 이다.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Self(handle);
            job.set_kill_on_close(true)?;
            // SAFETY: 두 핸들 모두 살아 있다(`process` 는 tokio 의 Child 가 쥐고 있다).
            if unsafe { AssignProcessToJobObject(job.0, process as HANDLE) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }

        fn set_kill_on_close(&self, on: bool) -> std::io::Result<()> {
            // SAFETY: 0 으로 채운 구조체는 「제한 없음」 이다.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            if on {
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            // SAFETY: `info` 의 크기와 종류를 맞춰 넘긴다.
            let ok = unsafe {
                SetInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn terminate(&self) {
            // SAFETY: 살아 있는 Job 핸들이다. 실패(이미 빈 Job)는 무시한다.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }

        /// 남은 프로세스는 두고 닫는다.
        pub fn release(self) {
            if let Err(e) = self.set_kill_on_close(false) {
                tracing::warn!(target: "pw_git", %e, "cannot release the git job object; processes git left behind end with it");
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

#[cfg(not(any(unix, windows)))]
struct Tree;

#[cfg(not(any(unix, windows)))]
impl Tree {
    fn of(_child: &tokio::process::Child) -> Option<Self> {
        None
    }
    fn term(&self) {}
    fn kill(&self) {}
    fn release(self) {}
    fn abandon(self) {}
}

/// 취소: 트리에 SIGTERM, git 이 [`TERM_GRACE`] 안에 끝나지 않으면 SIGKILL. 그 뒤 트리에 남은 것(SIGTERM 을 무시한 훅의 자식 등)도
/// SIGKILL 로 끝낸다. git 만은 어느 플랫폼에서든 끝내고 거둔다.
async fn terminate(child: &mut tokio::process::Child, tree: Option<Tree>) {
    if let Some(tree) = &tree {
        tree.term();
        let _ = tokio::time::timeout(TERM_GRACE, child.wait()).await;
        tree.kill();
    }
    let _ = child.start_kill();
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
