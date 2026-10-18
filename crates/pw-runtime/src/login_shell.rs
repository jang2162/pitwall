//! 로그인 셸의 PATH(03 §5-1 4번 「로그인 셸」, Orca 의 PATH 가져오기와 같은 방식).
//!
//! 어떻게: `<셸> -l -i -c "printf 표식; /usr/bin/env -0; printf 표식"` 을 돌려 두 표식 사이의 `PATH=` 를 읽는다. 셸 시작 파일이
//! stdout 에 찍는 잡음(인사말, 프롬프트 도구의 출력)은 표식 바깥이라 버려진다. `env -0` 이라 여러 줄짜리 값이 있어도 갈라 읽는다.
//! 자식은 `setsid` 로 새 세션이 되어 제어 터미널이 없다 — 대화형 셸이 터미널을 잡으려다 멈추지(SIGTTOU) 않게. stdin 은
//! `/dev/null`, stderr 는 버린다. [`RESOLVING_ENV`] 를 줘 시작 파일이 무거운 일을 건너뛸 수 있게 한다.
//!
//! 시간 제한([`DEFAULT_TIMEOUT`])을 넘기면 셸의 세션(프로세스 그룹)을 통째로 죽인다. 넘기거나, 셸을 못 띄우거나, 표식 · PATH 를
//! 찾지 못하면 이 프로세스의 PATH 로 물러난다. 무엇을 썼는지는 [`ResolvedPath::source`] 와 로그로 남긴다.
//!
//! 셸: `$SHELL`, 없으면 사용자 계정(`getpwuid_r`)의 셸. Windows 에는 로그인 셸이 없어 언제나 프로세스 PATH 다.
//!
//! 언제: 처음 쓰일 때 한 번([`LoginPath::get`]) — 앱 시작 경로를 막지 않는다(셸 시작은 수백 ms 가 걸리기도 한다). 쓸 것 같으면
//! [`LoginPath::prefetch`] 로 미리 시작해 둔다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};
use std::time::Duration;

/// 셸이 PATH 를 찍기까지 기다리는 시간.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// 셸에 주는 표식(값 `1`). 시작 파일이 PATH 를 가져가는 실행에서 무거운 일(플러그인 갱신 등)을 건너뛰게 할 수 있다.
pub const RESOLVING_ENV: &str = "PITWALL_RESOLVING_ENVIRONMENT";

/// PATH 를 어디서 얻었는가.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSource {
    LoginShell {
        shell: PathBuf,
    },
    /// 이 프로세스의 PATH 로 물러났다. 그 이유.
    Process {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    pub path: OsString,
    pub source: PathSource,
}

/// 셸에서 PATH 를 읽는다. 못 읽으면 `fallback`(이 프로세스의 PATH)으로 물러난다.
pub fn resolve(
    shell: Option<&Path>,
    timeout: Duration,
    fallback: Option<OsString>,
) -> ResolvedPath {
    let fallback = |reason: String| ResolvedPath {
        path: fallback.clone().unwrap_or_default(),
        source: PathSource::Process { reason },
    };
    let Some(shell) = shell else {
        return fallback("no login shell".to_owned());
    };
    match read_path(shell, timeout) {
        Ok(path) => ResolvedPath {
            path,
            source: PathSource::LoginShell {
                shell: shell.to_path_buf(),
            },
        },
        Err(reason) => fallback(reason),
    }
}

/// 사용자의 로그인 셸: `$SHELL`(절대 경로일 때), 없으면 계정 정보의 셸.
pub fn user_shell() -> Option<PathBuf> {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(account_shell)
}

/// `env -0` 출력에서 두 `marker` 사이의 `PATH=` 값. 표식 바깥(시작 파일의 잡음)은 보지 않는다. 비어 있으면 없는 것으로 본다.
pub fn parse_env_output(output: &[u8], marker: &[u8]) -> Option<Vec<u8>> {
    let start = find(output, marker)? + marker.len();
    let end = start + find(&output[start..], marker)?;
    output[start..end]
        .split(|&b| b == 0)
        .find_map(|entry| entry.strip_prefix(b"PATH="))
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
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

#[cfg(unix)]
fn read_path(shell: &Path, timeout: Duration) -> Result<OsString, String> {
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
    // PATH 를 찍은 셸은 곧 끝난다 — 조금 기다려 준다. 시간을 넘긴 셸은 기다리지 않는다.
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
        // 시간을 넘겼거나, PATH 를 찍고도 끝나지 않는 셸. 세션째 끝낸다 — setsid 로 자식 pid 가 그룹 id 다.
        // SAFETY: killpg(2) 는 신호를 보낼 뿐이다.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    // 죽였든 스스로 끝났든 거둔다(좀비를 남기지 않는다).
    let status = child.wait().ok();
    let Ok(output) = output else {
        return Err(format!(
            "{} did not print PATH within {timeout:?}",
            shell.display()
        ));
    };
    // 끝 상태보다 출력을 먼저 본다 — 대화형 셸은 시작 파일의 마지막 명령이 실패해도 PATH 는 이미 찍었다.
    parse_env_output(&output, marker.as_bytes())
        .map(OsString::from_vec)
        .ok_or_else(|| {
            let status = status.map_or_else(|| "unknown".to_owned(), |s| s.to_string());
            format!("{} printed no PATH ({status})", shell.display())
        })
}

#[cfg(not(unix))]
fn read_path(_shell: &Path, _timeout: Duration) -> Result<OsString, String> {
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

/// 앱 전체가 함께 쓰는 로그인 셸 PATH. 처음 [`Self::get`] 이 셸을 한 번 돌리고, 동시에 부른 쪽은 그 결과를 기다린다.
pub struct LoginPath {
    cell: OnceLock<ResolvedPath>,
    prefetch: Once,
    resolver: fn() -> ResolvedPath,
}

impl LoginPath {
    pub const fn new(resolver: fn() -> ResolvedPath) -> Self {
        Self {
            cell: OnceLock::new(),
            prefetch: Once::new(),
            resolver,
        }
    }

    /// PATH. 아직 없으면 지금 가져온다(최대 [`DEFAULT_TIMEOUT`] 남짓 막힌다).
    pub fn get(&self) -> &ResolvedPath {
        self.cell.get_or_init(self.resolver)
    }

    /// 백그라운드 스레드에서 가져오기를 시작한다. 이미 시작했거나 끝났으면 아무것도 하지 않는다.
    pub fn prefetch(&'static self) {
        self.prefetch.call_once(|| {
            let spawned = std::thread::Builder::new()
                .name("login-shell-path".to_owned())
                .spawn(move || {
                    self.get();
                });
            if let Err(e) = spawned {
                tracing::warn!(target: "pw_runtime", %e, "cannot prefetch the login shell PATH");
            }
        });
    }
}

/// 앱이 쓰는 것. 사용자의 셸과 이 프로세스의 PATH 로 [`resolve`] 하고 결과를 로그에 남긴다.
pub static LOGIN_PATH: LoginPath = LoginPath::new(resolve_for_this_user);

fn resolve_for_this_user() -> ResolvedPath {
    let started = std::time::Instant::now();
    let resolved = resolve(
        user_shell().as_deref(),
        DEFAULT_TIMEOUT,
        std::env::var_os("PATH"),
    );
    let elapsed = started.elapsed();
    let path = resolved.path.to_string_lossy();
    match &resolved.source {
        PathSource::LoginShell { shell } => tracing::info!(
            target: "pw_runtime",
            shell = %shell.display(), ?elapsed, %path, "PATH from the login shell"
        ),
        PathSource::Process { reason } => tracing::warn!(
            target: "pw_runtime",
            %reason, ?elapsed, %path, "using the process PATH; the login shell PATH is unavailable"
        ),
    }
    resolved
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use super::*;

    /// 셸 흉내: `-l -i -c <스크립트>` 를 받아 잡음을 찍고 PATH 를 바꾼 뒤 스크립트를 돌린다.
    fn fake_shell(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-shell");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn fallback() -> Option<OsString> {
        Some(OsString::from("/usr/bin:/bin"))
    }

    #[test]
    fn reads_path_between_the_markers_despite_startup_noise() {
        let dir = tempfile::tempdir().unwrap();
        // 표식 앞뒤의 잡음, NUL 이 든 잡음, 잡음 속의 가짜 `PATH=` 도 버린다. 값의 공백은 그대로다.
        let shell = fake_shell(
            dir.path(),
            r#"echo "Welcome! PATH=/wrong"
printf 'noise\0PATH=/also-wrong\0'
PATH="/opt/my tools/bin:/usr/bin:/bin"; export PATH
[ "$PITWALL_RESOLVING_ENVIRONMENT" = 1 ] || exit 3
eval "$4"
echo "bye PATH=/late""#,
        );
        let resolved = resolve(Some(&shell), Duration::from_secs(5), fallback());
        assert_eq!(
            resolved,
            ResolvedPath {
                path: OsString::from("/opt/my tools/bin:/usr/bin:/bin"),
                source: PathSource::LoginShell { shell },
            }
        );
    }

    #[test]
    fn a_shell_that_hangs_falls_back_within_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let shell = fake_shell(
            dir.path(),
            &format!("sleep 30 & echo $! > '{}'; wait", pid_file.display()),
        );
        let started = Instant::now();
        let resolved = resolve(Some(&shell), Duration::from_millis(300), fallback());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(resolved.path, OsString::from("/usr/bin:/bin"));
        let PathSource::Process { reason } = &resolved.source else {
            panic!("{resolved:?}");
        };
        assert!(reason.contains("did not print PATH within"), "{reason}");
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

    /// PATH 를 찍은 뒤에도 stdout 을 쥔 백그라운드 프로세스가 있어도 시간 제한까지 기다리지 않는다.
    #[test]
    fn does_not_wait_for_background_processes_that_hold_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "sleep 30 &\neval \"$4\"");
        let started = Instant::now();
        let resolved = resolve(Some(&shell), Duration::from_secs(4), fallback());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(
            matches!(resolved.source, PathSource::LoginShell { .. }),
            "{resolved:?}"
        );
    }

    #[test]
    fn a_failing_or_silent_shell_falls_back_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "echo oops; exit 1");
        let resolved = resolve(Some(&shell), Duration::from_secs(5), fallback());
        assert_eq!(resolved.path, OsString::from("/usr/bin:/bin"));
        assert!(
            matches!(&resolved.source, PathSource::Process { reason } if reason.contains("printed no PATH")),
            "{resolved:?}"
        );

        let missing = dir.path().join("no-such-shell");
        let resolved = resolve(Some(&missing), Duration::from_secs(5), fallback());
        assert!(
            matches!(&resolved.source, PathSource::Process { reason } if reason.contains("cannot start")),
            "{resolved:?}"
        );
        let resolved = resolve(None, Duration::from_secs(5), fallback());
        assert_eq!(
            resolved.source,
            PathSource::Process {
                reason: "no login shell".to_owned()
            }
        );
    }

    #[test]
    fn parse_needs_both_markers_and_a_non_empty_path() {
        assert_eq!(
            parse_env_output(b"xMKA=1\0PATH=/a:/b\0MKy", b"MK"),
            Some(b"/a:/b".to_vec())
        );
        assert_eq!(parse_env_output(b"MKPATH=/a\0", b"MK"), None);
        assert_eq!(parse_env_output(b"MKPATH=\0MK", b"MK"), None);
        assert_eq!(parse_env_output(b"PATH=/a\0", b"MK"), None);
    }

    /// 동시에 여러 번 불러도 셸은 한 번 돈다(첫 호출이 끝날 때까지 나머지가 기다린다).
    #[test]
    fn login_path_resolves_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        static ONCE: LoginPath = LoginPath::new(|| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
            ResolvedPath {
                path: OsString::from("/x"),
                source: PathSource::Process {
                    reason: "test".to_owned(),
                },
            }
        });
        ONCE.prefetch();
        let threads: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(|| ONCE.get().path.clone()))
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), OsString::from("/x"));
        }
        ONCE.prefetch();
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }
}
