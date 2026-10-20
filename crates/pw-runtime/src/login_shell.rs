//! 로그인 셸의 환경(03 §5-1 4번 「로그인 셸」, Orca 의 PATH 가져오기와 같은 방식).
//!
//! 어떻게: `<셸> -l -i -c "printf 표식; /usr/bin/env -0; printf 표식"` 을 돌려 두 표식 사이의 환경 전체를 읽는다. 셸 시작 파일이
//! stdout 에 찍는 잡음(인사말, 프롬프트 도구의 출력)은 표식 바깥이라 버려진다. `env -0` 이라 여러 줄짜리 값이 있어도 갈라 읽는다.
//! 셸 고유 변수는 버린다([`crate::env::EXCLUDED`]). 자식은 `setsid` 로 새 세션이 되어 제어 터미널이 없다 — 대화형 셸이 터미널을
//! 잡으려다 멈추지(SIGTTOU) 않게. stdin 은 `/dev/null`, stderr 는 버린다. [`RESOLVING_ENV`] 를 줘 시작 파일이 무거운 일을 건너뛸
//! 수 있게 한다.
//!
//! 시간 제한([`DEFAULT_TIMEOUT`])을 넘기면 셸의 세션(프로세스 그룹)을 통째로 죽인다. 넘기거나, 셸을 못 띄우거나, 표식 · PATH 를
//! 찾지 못하면 실패다 — 부른 쪽([`crate::Runtime`])이 앱 프로세스의 환경으로 물러나고, 실패는 남기지 않아 다음 요청에서 다시
//! 시도한다.
//!
//! 셸: `$SHELL`, 없으면 사용자 계정(`getpwuid_r`)의 셸. Windows 에는 로그인 셸이 없어 언제나 실패(프로세스 환경)다.
//!
//! 언제: 처음 쓰일 때 — 앱 시작 경로를 막지 않는다(셸 시작은 수백 ms 가 걸리기도 한다). 성공하면 「환경 다시 읽기」
//! ([`LoginShell::invalidate`]) 전까지 남긴다. 셸은 디렉터리와 무관하게 한 번 돈다(시작 파일은 사용자 단위다).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cache::Cache;
use crate::env::RuntimeEnv;
use crate::methods::RuntimeSource;
use crate::provider::{Layer, RuntimeProvider};

/// 셸이 환경을 찍기까지 기다리는 시간.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// 셸에 주는 표식(값 `1`). 시작 파일이 환경을 가져가는 실행에서 무거운 일(플러그인 갱신 등)을 건너뛰게 할 수 있다.
pub const RESOLVING_ENV: &str = "PITWALL_RESOLVING_ENVIRONMENT";

/// 로그인 셸 제공자(`"shell"`). 언제나 쓰이고, 앱 프로세스의 환경 대신 셸이 만든 환경 전체를 준다.
pub struct LoginShell {
    shell: Option<PathBuf>,
    timeout: Duration,
    cache: Cache<(), Result<Layer, String>>,
}

impl LoginShell {
    pub const ID: &'static str = "shell";

    /// `shell` 이 없으면 언제나 실패한다.
    pub fn new(shell: Option<PathBuf>, timeout: Duration) -> Self {
        Self {
            shell,
            timeout,
            cache: Cache::default(),
        }
    }

    /// 이 사용자의 셸([`user_shell`])과 [`DEFAULT_TIMEOUT`].
    pub fn for_this_user() -> Self {
        Self::new(user_shell(), DEFAULT_TIMEOUT)
    }

    /// 셸 환경. 성공한 것이 남아 있으면 그것, 아니면 지금 셸을 돌린다(최대 시간 제한 남짓 막힌다).
    pub fn get(&self) -> std::sync::Arc<Result<Layer, String>> {
        self.cache
            .get_or_compute(&(), || self.read_logged(), Result::is_ok)
    }

    /// 성공한 셸 환경이 남아 있는가.
    pub fn is_ready(&self) -> bool {
        self.cache.contains(&())
    }

    /// 남긴 환경을 잊는다. 다음 [`Self::get`] 이 셸을 다시 돌린다.
    pub fn invalidate(&self) {
        self.cache.clear();
    }

    fn read_logged(&self) -> Result<Layer, String> {
        let started = std::time::Instant::now();
        let read = match &self.shell {
            Some(shell) => read_env(shell, self.timeout).map(|env| Layer {
                env,
                source: RuntimeSource::LoginShell {
                    shell: shell.display().to_string(),
                },
            }),
            None => Err("no login shell".to_owned()),
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
            Err(reason) => tracing::warn!(
                target: "pw_runtime",
                %reason, ?elapsed,
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
        (*self.get()).clone()
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
    let start = find(output, marker)? + marker.len();
    let end = start + find(&output[start..], marker)?;
    let vars: Vec<(Vec<u8>, Vec<u8>)> = output[start..end]
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
pub fn read_env(shell: &Path, timeout: Duration) -> Result<RuntimeEnv, String> {
    use std::io::Read;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

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
        .map_err(|e| format!("cannot start {}: {e}", shell.display()))?;
    let mut stdout = child.stdout.take().expect("stdout 은 piped 다");
    let (tx, rx) = mpsc::channel();
    let wanted = marker.clone().into_bytes();
    // 둘째 표식이 보이면 곧바로 돌려준다 — 시작 파일이 띄운 백그라운드 프로세스가 stdout 을 쥐고 있어도 EOF 를 기다리지 않게.
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    out.extend_from_slice(&chunk[..n]);
                    if parse_env_output(&out, &wanted).is_some() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = tx.send(out);
    });
    let pid = child.id();
    let output = rx.recv_timeout(timeout);
    // 환경을 찍은 셸은 곧 끝난다 — 조금 기다려 준다. 시간을 넘긴 셸은 기다리지 않는다.
    let grace = if output.is_ok() {
        Duration::from_millis(500)
    } else {
        Duration::ZERO
    };
    let deadline = std::time::Instant::now() + grace;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if std::time::Instant::now() < deadline => {
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
    let Ok(output) = output else {
        return Err(format!(
            "{} did not print its environment within {timeout:?}",
            shell.display()
        ));
    };
    // 끝 상태보다 출력을 먼저 본다 — 대화형 셸은 시작 파일의 마지막 명령이 실패해도 환경은 이미 찍었다.
    let vars = parse_env_output(&output, marker.as_bytes()).ok_or_else(|| {
        let status = status.map_or_else(|| "unknown".to_owned(), |s| s.to_string());
        format!("{} printed no PATH ({status})", shell.display())
    })?;
    Ok(RuntimeEnv::from_vars(vars.into_iter().map(|(k, v)| {
        (OsString::from_vec(k), OsString::from_vec(v))
    })))
}

#[cfg(not(unix))]
pub fn read_env(_shell: &Path, _timeout: Duration) -> Result<RuntimeEnv, String> {
    Err("there is no login shell on this platform".to_owned())
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
        let Err(reason) = read else {
            panic!("{read:?}")
        };
        assert!(
            reason.contains("did not print its environment within"),
            "{reason}"
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
            matches!(&read, Err(reason) if reason.contains("printed no PATH")),
            "{read:?}"
        );
        let missing = dir.path().join("no-such-shell");
        let read = read_env(&missing, Duration::from_secs(5));
        assert!(
            matches!(&read, Err(reason) if reason.contains("cannot start")),
            "{read:?}"
        );
        let none = LoginShell::new(None, Duration::from_secs(5));
        assert_eq!(*none.get(), Err("no login shell".to_owned()));
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
