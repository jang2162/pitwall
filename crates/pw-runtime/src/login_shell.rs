//! 로그인 셸의 환경(03 §5-1 4번 「로그인 셸」, Orca 의 PATH 가져오기와 같은 방식).
//!
//! 어떻게: `<셸> -l -i -c "printf 표식; /usr/bin/env -0; printf 표식"` 을 돌려 두 표식 사이의 환경 전체를 읽는다. 셸 시작 파일이
//! stdout 에 찍는 잡음(인사말, 프롬프트 도구의 출력)은 표식 바깥이라 버려진다. `env -0` 이라 여러 줄짜리 값이 있어도 갈라 읽는다.
//! 셸 고유 변수는 버린다([`crate::env::EXCLUDED`]). 자식은 `setsid` 로 새 세션이 되어 제어 터미널이 없다 — 대화형 셸이 터미널을
//! 잡으려다 멈추지(SIGTTOU) 않게. stdin 은 `/dev/null`, stderr 는 버린다. [`RESOLVING_ENV`] 를 줘 시작 파일이 무거운 일을 건너뛸
//! 수 있게 한다.
//!
//! 읽기는 부른 스레드에서 `poll(2)` 로 시간 제한까지만 한다. 둘째 표식이 보이거나 시간을 넘기면 파이프를 닫고 끝낸다 — 시작
//! 파일이 띄운 다른 세션의 프로세스가 stdout 을 쥐고 있어도 EOF 를 기다리거나 읽기 스레드를 남기지 않는다. 출력이
//! [`MAX_OUTPUT`] 을 넘으면 실패다.
//!
//! 시간 제한([`DEFAULT_TIMEOUT`])을 넘기면 셸의 세션(프로세스 그룹)을 통째로 죽인다. 넘기거나, 셸을 못 띄우거나, 표식 · PATH 를
//! 찾지 못하면 실패다 — 부른 쪽([`crate::Runtime`])이 앱 프로세스의 환경으로 물러난다. 실패는 남기지 않아 다음 요청에서 다시
//! 시도하되, **시간 초과만** [`TIMEOUT_COOLDOWN`] 동안 다시 돌리지 않고 물러난 결과를 곧바로 준다 — 시작 파일이 대화형 셸에서
//! `exec fish` · `exec tmux` 처럼 멈추면 요청(새 터미널 탭)마다 5초를 기다리게 된다. 「환경 다시 읽기」 는 냉각을 푼다.
//!
//! 셸: `$SHELL`, 없으면 사용자 계정(`getpwuid_r`)의 셸. Windows 에는 로그인 셸이 없어 언제나 실패(프로세스 환경)다.
//!
//! 언제: 처음 쓰일 때 — 앱 시작 경로를 막지 않는다(셸 시작은 수백 ms 가 걸리기도 한다). 성공하면 「환경 다시 읽기」
//! ([`LoginShell::invalidate`]) 전까지 남긴다. 셸은 디렉터리와 무관하게 한 번 돈다(시작 파일은 사용자 단위다).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::cache::Cache;
use crate::env::RuntimeEnv;
use crate::methods::RuntimeSource;
use crate::provider::{Layer, RuntimeProvider};

/// 셸이 환경을 찍기까지 기다리는 시간.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// 시간 초과 뒤 셸을 다시 돌리지 않는 시간.
pub const TIMEOUT_COOLDOWN: Duration = Duration::from_secs(30);

/// 셸 출력(시작 파일의 잡음 포함)의 상한. 넘으면 실패다.
pub const MAX_OUTPUT: usize = 64 * 1024 * 1024;

/// 셸에 주는 표식(값 `1`). 시작 파일이 환경을 가져가는 실행에서 무거운 일(플러그인 갱신 등)을 건너뛰게 할 수 있다.
pub const RESOLVING_ENV: &str = "PITWALL_RESOLVING_ENVIRONMENT";

/// 셸에서 환경을 읽지 못한 이유.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellError {
    pub reason: String,
    /// 시간 제한 안에 환경을 찍지 않았다(냉각 대상).
    pub timed_out: bool,
}

impl ShellError {
    fn other(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            timed_out: false,
        }
    }
}

type ShellResult = Result<Layer, ShellError>;

#[derive(Default)]
struct Cooling {
    /// 이때까지는 이 결과(시간 초과)를 곧바로 준다.
    until: Option<(Instant, Arc<ShellResult>)>,
    /// [`LoginShell::invalidate`] 마다 하나씩 는다. 무효화 전에 시작한 시도는 냉각을 걸지 않는다.
    epoch: u64,
}

/// 로그인 셸 제공자(`"shell"`). 언제나 쓰이고, 앱 프로세스의 환경 대신 셸이 만든 환경 전체를 준다.
pub struct LoginShell {
    shell: Option<PathBuf>,
    timeout: Duration,
    cooldown: Duration,
    cache: Cache<(), ShellResult>,
    cooling: Mutex<Cooling>,
}

impl LoginShell {
    pub const ID: &'static str = "shell";

    /// `shell` 이 없으면 언제나 실패한다. 냉각은 [`TIMEOUT_COOLDOWN`].
    pub fn new(shell: Option<PathBuf>, timeout: Duration) -> Self {
        Self {
            shell,
            timeout,
            cooldown: TIMEOUT_COOLDOWN,
            cache: Cache::default(),
            cooling: Mutex::default(),
        }
    }

    /// 이 사용자의 셸([`user_shell`])과 [`DEFAULT_TIMEOUT`].
    pub fn for_this_user() -> Self {
        Self::new(user_shell(), DEFAULT_TIMEOUT)
    }

    /// 시간 초과 뒤의 냉각 시간(시험).
    #[must_use]
    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = cooldown;
        self
    }

    fn cooling(&self) -> MutexGuard<'_, Cooling> {
        self.cooling.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 셸 환경. 성공한 것이 남아 있으면 그것, 시간 초과의 냉각 중이면 그 실패, 아니면 지금 셸을 돌린다(최대 시간 제한 남짓
    /// 막힌다).
    pub fn get(&self) -> Arc<ShellResult> {
        let epoch = {
            let cooling = self.cooling();
            if let Some((until, result)) = &cooling.until
                && Instant::now() < *until
            {
                return Arc::clone(result);
            }
            cooling.epoch
        };
        let result = self
            .cache
            .get_or_compute(&(), || self.read_logged(), Result::is_ok);
        if matches!(&*result, Err(e) if e.timed_out) {
            let mut cooling = self.cooling();
            let cooling_now = cooling
                .until
                .as_ref()
                .is_some_and(|(until, _)| Instant::now() < *until);
            if cooling.epoch == epoch && !cooling_now {
                cooling.until = Some((Instant::now() + self.cooldown, Arc::clone(&result)));
                tracing::warn!(
                    target: "pw_runtime",
                    cooldown = ?self.cooldown,
                    "the login shell timed out; using the process environment without retrying for a while"
                );
            }
        }
        result
    }

    /// 성공한 셸 환경이 남아 있는가.
    pub fn is_ready(&self) -> bool {
        self.cache.contains(&())
    }

    /// 남긴 환경과 시간 초과의 냉각을 잊는다. 다음 [`Self::get`] 이 셸을 다시 돌린다.
    pub fn invalidate(&self) {
        {
            let mut cooling = self.cooling();
            cooling.until = None;
            cooling.epoch += 1;
        }
        self.cache.clear();
    }

    fn read_logged(&self) -> ShellResult {
        let started = Instant::now();
        let read = match &self.shell {
            Some(shell) => read_env(shell, self.timeout).map(|env| Layer {
                env,
                source: RuntimeSource::LoginShell {
                    shell: shell.display().to_string(),
                },
            }),
            None => Err(ShellError::other("no login shell")),
        };
        let elapsed = started.elapsed();
        match &read {
            Ok(layer) => tracing::info!(
                target: "pw_runtime",
                shell = %self.shell.as_deref().unwrap_or(Path::new("")).display(),
                ?elapsed,
                path = %layer.env.path.to_string_lossy(),
                vars = layer.env.vars.len(),
                "environment from the login shell"
            ),
            Err(e) => tracing::warn!(
                target: "pw_runtime",
                reason = %e.reason, ?elapsed,
                "using the process environment; the login shell environment is unavailable"
            ),
        }
        read
    }
}

impl RuntimeProvider for LoginShell {
    fn id(&self) -> &'static str {
        Self::ID
    }

    /// 사용자 자신의 시작 파일만 돈다(셸은 프로젝트 폴더에서 돌지 않는다).
    fn runs_project_code(&self) -> bool {
        false
    }

    fn applies_to(&self, _dir: &Path, _base: &RuntimeEnv) -> bool {
        true
    }

    fn resolve(&self, _dir: &Path, _base: &RuntimeEnv) -> Result<Layer, String> {
        (*self.get()).clone().map_err(|e| e.reason)
    }
}

/// 사용자의 로그인 셸: `$SHELL`(절대 경로일 때), 없으면 계정 정보의 셸.
pub fn user_shell() -> Option<PathBuf> {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(account_shell)
}

/// `env -0` 출력에서 두 `marker` 사이의 변수들(`이름=값`). 표식 바깥(시작 파일의 잡음)은 보지 않는다. PATH 가 없거나 비어 있으면
/// 셸이 환경을 찍지 못한 것으로 본다.
pub fn parse_env_output(output: &[u8], marker: &[u8]) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut scanner = Scanner::new(marker, usize::MAX);
    match scanner.push(output) {
        Scan::Found(body) => parse_vars(&output[body]),
        Scan::More | Scan::TooLarge => None,
    }
}

/// 두 표식 사이의 `env -0` 출력을 변수들로. PATH 가 없거나 비어 있으면 `None`.
fn parse_vars(body: &[u8]) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let vars: Vec<(Vec<u8>, Vec<u8>)> = body
        .split(|&b| b == 0)
        .filter_map(|entry| {
            let eq = entry.iter().position(|&b| b == b'=')?;
            (eq > 0).then(|| (entry[..eq].to_vec(), entry[eq + 1..].to_vec()))
        })
        .collect();
    vars.iter()
        .any(|(name, value)| name == b"PATH" && !value.is_empty())
        .then_some(vars)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 조각으로 들어오는 출력에서 두 표식을 찾는다. 조각마다 새로 들어온 구간(표식이 조각 경계에 걸칠 수 있어 표식 길이 − 1 만큼
/// 앞부터)만 훑고 첫 표식 위치는 기억한다 — 시작 파일의 잡음이 길어도 출력 길이에 선형이다.
struct Scanner {
    marker: Vec<u8>,
    out: Vec<u8>,
    /// 첫 표식 바로 뒤.
    body_start: Option<usize>,
    limit: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Scan {
    More,
    /// `out` 안에서 두 표식 사이.
    Found(std::ops::Range<usize>),
    /// 둘째 표식을 찾기 전에 출력이 상한을 넘었다.
    TooLarge,
}

impl Scanner {
    fn new(marker: &[u8], limit: usize) -> Self {
        Self {
            marker: marker.to_vec(),
            out: Vec::new(),
            body_start: None,
            limit,
        }
    }

    fn push(&mut self, data: &[u8]) -> Scan {
        let from = self.out.len().saturating_sub(self.marker.len() - 1);
        self.out.extend_from_slice(data);
        if self.body_start.is_none() {
            self.body_start =
                find(&self.out[from..], &self.marker).map(|p| from + p + self.marker.len());
        }
        if let Some(start) = self.body_start {
            let from = from.max(start);
            if let Some(p) = find(&self.out[from..], &self.marker) {
                return Scan::Found(start..from + p);
            }
        }
        if self.out.len() > self.limit {
            return Scan::TooLarge;
        }
        Scan::More
    }
}

/// 시작 파일이 우연히 찍을 수 없는 표식. 셸이 따로 해석하지 않는 글자(영숫자 · `_`)만 쓴다.
#[cfg(unix)]
fn marker() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("__PITWALL_ENV_{:x}_{nanos:x}__", std::process::id())
}

/// `shell` 을 로그인 · 대화형으로 돌려 환경을 읽는다.
#[cfg(unix)]
pub fn read_env(shell: &Path, timeout: Duration) -> Result<RuntimeEnv, ShellError> {
    read_env_limited(shell, timeout, MAX_OUTPUT)
}

/// 읽은 결과.
#[cfg(unix)]
enum Read {
    Found(Vec<u8>),
    TimedOut,
    TooLarge,
    /// EOF 나 읽기 오류로 둘째 표식 없이 끝났다.
    Ended,
}

#[cfg(unix)]
fn read_env_limited(
    shell: &Path,
    timeout: Duration,
    limit: usize,
) -> Result<RuntimeEnv, ShellError> {
    use std::io::Read as _;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let marker = marker();
    let script = format!("printf '%s' {marker}; /usr/bin/env -0; printf '%s' {marker}");
    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c", &script])
        .env(RESOLVING_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
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
        .map_err(|e| ShellError::other(format!("cannot start {}: {e}", shell.display())))?;
    let mut stdout = child.stdout.take().expect("stdout 은 piped 다");
    let deadline = Instant::now() + timeout;
    let mut scanner = Scanner::new(marker.as_bytes(), limit);
    let mut chunk = vec![0u8; 64 * 1024];
    // 둘째 표식이 보이면 곧바로 끝낸다 — 시작 파일이 띄운 백그라운드 프로세스가 stdout 을 쥐고 있어도 EOF 를 기다리지 않게.
    let read = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Read::TimedOut;
        }
        let mut fd = libc::pollfd {
            fd: stdout.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis =
            libc::c_int::try_from(remaining.as_millis().max(1)).unwrap_or(libc::c_int::MAX);
        // SAFETY: pollfd 하나를 가리키는 유효한 포인터와 개수 1 을 넘긴다.
        let ready = unsafe { libc::poll(&raw mut fd, 1, millis) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break Read::Ended;
        }
        if ready == 0 {
            continue;
        }
        // poll 이 읽을 것(또는 EOF)이 있다고 했으므로 막히지 않는다.
        match stdout.read(&mut chunk) {
            Ok(0) => break Read::Ended,
            Ok(n) => match scanner.push(&chunk[..n]) {
                Scan::Found(body) => break Read::Found(scanner.out[body].to_vec()),
                Scan::TooLarge => break Read::TooLarge,
                Scan::More => {}
            },
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break Read::Ended,
        }
    };
    // 파이프의 읽는 쪽을 닫는다. 남은 쓰는 쪽은 다음 쓰기에서 SIGPIPE 를 받는다.
    drop(stdout);
    let pid = child.id();
    // 환경을 찍은 셸은 곧 끝난다 — 조금 기다려 준다. 시간을 넘긴 셸은 기다리지 않는다.
    let grace = if matches!(read, Read::Found(_)) {
        Duration::from_millis(500)
    } else {
        Duration::ZERO
    };
    let grace_until = Instant::now() + grace;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < grace_until => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => break false,
        }
    };
    if !exited {
        // 시간을 넘겼거나, 환경을 찍고도 끝나지 않는 셸. 세션째 끝낸다 — setsid 로 자식 pid 가 그룹 id 다.
        // SAFETY: killpg(2) 는 신호를 보낼 뿐이다.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    // 죽였든 스스로 끝났든 거둔다(좀비를 남기지 않는다).
    let status = child.wait().ok();
    let status = || status.map_or_else(|| "unknown".to_owned(), |s| s.to_string());
    let body = match read {
        Read::Found(body) => body,
        Read::TimedOut => {
            return Err(ShellError {
                reason: format!(
                    "{} did not print its environment within {timeout:?}",
                    shell.display()
                ),
                timed_out: true,
            });
        }
        Read::TooLarge => {
            return Err(ShellError::other(format!(
                "{} printed more than {limit} bytes before its environment",
                shell.display()
            )));
        }
        Read::Ended => {
            return Err(ShellError::other(format!(
                "{} printed no PATH ({})",
                shell.display(),
                status()
            )));
        }
    };
    // 끝 상태보다 출력을 먼저 본다 — 대화형 셸은 시작 파일의 마지막 명령이 실패해도 환경은 이미 찍었다.
    let vars = parse_vars(&body).ok_or_else(|| {
        ShellError::other(format!(
            "{} printed no PATH ({})",
            shell.display(),
            status()
        ))
    })?;
    Ok(RuntimeEnv::from_vars(vars.into_iter().map(|(k, v)| {
        (OsString::from_vec(k), OsString::from_vec(v))
    })))
}

#[cfg(not(unix))]
pub fn read_env(_shell: &Path, _timeout: Duration) -> Result<RuntimeEnv, ShellError> {
    Err(ShellError::other(
        "there is no login shell on this platform",
    ))
}

#[cfg(unix)]
fn account_shell() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;

    let mut buf = vec![0 as libc::c_char; 4096];
    // SAFETY: passwd 는 C 구조체라 0 으로 채운 값이 유효하다. getpwuid_r(3) 는 `buf` 안에 문자열을 쓰고 `result` 로 성공을 알린다 —
    // `pw_shell` 은 `buf` 가 살아 있는 동안만 읽는다.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut result = std::ptr::null_mut();
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if rc != 0 || result.is_null() || pwd.pw_shell.is_null() {
            return None;
        }
        let shell = PathBuf::from(OsStr::from_bytes(CStr::from_ptr(pwd.pw_shell).to_bytes()));
        shell.is_absolute().then_some(shell)
    }
}

#[cfg(not(unix))]
fn account_shell() -> Option<PathBuf> {
    None
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use super::*;

    /// 셸 흉내: `-l -i -c <스크립트>` 를 받아 `body` 를 돈다. `eval "$4"` 가 받은 스크립트를 돌린다.
    pub(crate) fn fake_shell(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-shell");
        // 같은 이름으로 다시 쓸 때 돌고 있는 셸이 반쯤 쓴 파일을 읽지 않게 옆에 쓰고 옮긴다.
        let tmp = dir.join("fake-shell.tmp");
        std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        path
    }

    #[test]
    fn reads_the_environment_between_the_markers_despite_startup_noise() {
        let dir = tempfile::tempdir().unwrap();
        // 표식 앞뒤의 잡음, NUL 이 든 잡음, 잡음 속의 가짜 `PATH=` 도 버린다. 값의 공백 · 개행은 그대로다.
        let shell = fake_shell(
            dir.path(),
            r#"echo "Welcome! PATH=/wrong"
printf 'noise\0PATH=/also-wrong\0'
PATH="/opt/my tools/bin:/usr/bin:/bin"; export PATH
JAVA_HOME="/opt/jdk 21"; export JAVA_HOME
MULTI="line one
line two"; export MULTI
[ "$PITWALL_RESOLVING_ENVIRONMENT" = 1 ] || exit 3
eval "$4"
echo "bye PATH=/late""#,
        );
        let env = read_env(&shell, Duration::from_secs(5)).unwrap();
        assert_eq!(env.path, OsString::from("/opt/my tools/bin:/usr/bin:/bin"));
        assert_eq!(
            env.get("JAVA_HOME"),
            Some(std::ffi::OsStr::new("/opt/jdk 21"))
        );
        assert_eq!(
            env.get("MULTI"),
            Some(std::ffi::OsStr::new("line one\nline two"))
        );
        // 셸 고유 변수와 표식은 넘기지 않는다(sh 는 PWD 를 정한다).
        for name in ["PWD", "SHLVL", "_", RESOLVING_ENV] {
            assert_eq!(env.get(name), None, "{name}");
        }
    }

    #[test]
    fn a_shell_that_hangs_fails_within_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let shell = fake_shell(
            dir.path(),
            &format!("sleep 30 & echo $! > '{}'; wait", pid_file.display()),
        );
        let started = Instant::now();
        let read = read_env(&shell, Duration::from_millis(300));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        let Err(e) = read else { panic!("{read:?}") };
        assert!(e.timed_out, "{e:?}");
        assert!(
            e.reason.contains("did not print its environment within"),
            "{e:?}"
        );
        // 셸이 띄운 손주(sleep)까지 세션째 죽였다.
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        // SAFETY: 신호 0 은 존재만 묻는다.
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "grandchild {pid} survived"
        );
    }

    /// 환경을 찍은 뒤에도 stdout 을 쥔 백그라운드 프로세스가 있어도 시간 제한까지 기다리지 않는다.
    #[test]
    fn does_not_wait_for_background_processes_that_hold_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "sleep 30 &\neval \"$4\"");
        let started = Instant::now();
        let read = read_env(&shell, Duration::from_secs(4));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(read.is_ok(), "{read:?}");
    }

    #[test]
    fn a_failing_or_missing_shell_fails_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "echo oops; exit 1");
        let read = read_env(&shell, Duration::from_secs(5));
        assert!(
            matches!(&read, Err(e) if e.reason.contains("printed no PATH") && !e.timed_out),
            "{read:?}"
        );
        let missing = dir.path().join("no-such-shell");
        let read = read_env(&missing, Duration::from_secs(5));
        assert!(
            matches!(&read, Err(e) if e.reason.contains("cannot start") && !e.timed_out),
            "{read:?}"
        );
        let none = LoginShell::new(None, Duration::from_secs(5));
        assert_eq!(*none.get(), Err(ShellError::other("no login shell")));
    }

    /// 표식이 조각 경계에 걸쳐도 찾는다(한 바이트씩 넣어도). 첫 표식만 보였을 때는 더 기다린다.
    #[test]
    fn the_scanner_finds_markers_split_across_chunks() {
        let output = b"noise MK noise MKA=1\0PATH=/a\0MK tail";
        let mut scanner = Scanner::new(b"MK", usize::MAX);
        let mut found = None;
        for b in output {
            match scanner.push(std::slice::from_ref(b)) {
                Scan::More => {}
                other => {
                    found = Some(other);
                    break;
                }
            }
        }
        // 첫 표식은 잡음 속의 첫 `MK` 다 — 둘째 표식은 그 뒤의 첫 `MK`.
        assert_eq!(found, Some(Scan::Found(8..15)));
        assert_eq!(&scanner.out[8..15], b" noise ");
        let mut scanner = Scanner::new(b"MARK", 10);
        assert_eq!(scanner.push(b"MARK1234"), Scan::More);
        assert_eq!(scanner.push(b"567"), Scan::TooLarge);
    }

    /// 표식 앞의 잡음이 4 MiB 여도 시간 안에 읽는다(출력 길이에 선형).
    #[test]
    fn a_noisy_startup_file_does_not_time_out() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "head -c 4194304 /dev/zero | tr '\\0' x\nPATH=/x:/usr/bin:/bin; export PATH\neval \"$4\"",
        );
        let started = Instant::now();
        let read = read_env(&shell, Duration::from_secs(5));
        assert!(read.is_ok(), "{read:?} after {:?}", started.elapsed());
        assert_eq!(read.unwrap().path, OsString::from("/x:/usr/bin:/bin"));
    }

    /// 출력이 상한을 넘으면 시간 초과를 기다리지 않고 실패한다(냉각 대상이 아니다).
    #[test]
    fn output_over_the_limit_fails() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "head -c 2097152 /dev/zero | tr '\\0' x\neval \"$4\"",
        );
        let read = read_env_limited(&shell, Duration::from_secs(5), 1024 * 1024);
        assert!(
            matches!(&read, Err(e) if e.reason.contains("printed more than 1048576 bytes") && !e.timed_out),
            "{read:?}"
        );
    }

    #[test]
    fn parse_needs_both_markers_and_a_non_empty_path() {
        let pairs = |v: Vec<(Vec<u8>, Vec<u8>)>| -> Vec<(String, String)> {
            v.into_iter()
                .map(|(k, v)| (String::from_utf8(k).unwrap(), String::from_utf8(v).unwrap()))
                .collect()
        };
        assert_eq!(
            parse_env_output(b"xMKA=1=2\0PATH=/a:/b\0junk\0MKy", b"MK").map(pairs),
            Some(vec![
                ("A".to_owned(), "1=2".to_owned()),
                ("PATH".to_owned(), "/a:/b".to_owned())
            ])
        );
        assert_eq!(parse_env_output(b"MKPATH=/a\0", b"MK"), None);
        assert_eq!(parse_env_output(b"MKPATH=\0MK", b"MK"), None);
        assert_eq!(parse_env_output(b"MKA=1\0MK", b"MK"), None);
        assert_eq!(parse_env_output(b"PATH=/a\0", b"MK"), None);
    }
}
