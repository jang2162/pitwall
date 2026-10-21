//! 제공자가 외부 도구(`mise env --json`)를 시간 제한 안에서 돌린다.
//!
//! 로그인 셸([`crate::login_shell`])과 같은 방식이다: 자식은 `setsid` 로 새 세션이 되어 제어 터미널이 없고, stdin 은 `/dev/null` 이다.
//! stdout · stderr 는 부른 스레드가 `poll(2)` 로 시간 제한까지만 읽는다. 시간을 넘기거나 출력이 상한을 넘으면 자식의 세션(프로세스
//! 그룹)을 통째로 죽인다. 자식이 끝난 뒤에도 그 자식이 띄운 프로세스가 파이프를 쥐고 있으면 [`EXIT_GRACE`] 만 더 읽고 끝낸다 —
//! EOF 를 기다리거나 읽기 스레드를 남기지 않는다.

use std::process::{Command, ExitStatus};
use std::time::Duration;

/// 자식이 끝난 뒤 파이프의 나머지를 기다리는 시간.
pub const EXIT_GRACE: Duration = Duration::from_millis(200);

/// 끝난 자식의 출력.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// 띄우지 못했다.
    Spawn(String),
    /// 시간 안에 끝나지 않았다(세션째 죽였다).
    TimedOut,
    /// stdout · stderr 를 합쳐 상한을 넘겼다(세션째 죽였다).
    TooLarge,
}

/// `command` 를 띄워 끝날 때까지(최대 `timeout`) 기다린다. stdin · stdout · stderr 와 세션은 여기서 정한다.
#[cfg(unix)]
pub fn run(mut command: Command, timeout: Duration, limit: usize) -> Result<Output, RunError> {
    use std::fs::File;
    use std::io::Read as _;
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::Instant;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let pid = child.id();
    let stdout = child.stdout.take().map(|p| File::from(OwnedFd::from(p)));
    let stderr = child.stderr.take().map(|p| File::from(OwnedFd::from(p)));
    // [stdout, stderr]. 닫힌(EOF · 오류) 파이프는 `None` 이다.
    let mut pipes = [stdout, stderr];
    let mut bufs = [Vec::new(), Vec::new()];
    let deadline = Instant::now() + timeout;
    let mut exited: Option<(ExitStatus, Instant)> = None;
    let mut chunk = vec![0u8; 64 * 1024];
    let outcome = loop {
        if exited.is_none()
            && let Ok(Some(status)) = child.try_wait()
        {
            exited = Some((status, Instant::now()));
        }
        let open: Vec<usize> = (0..2).filter(|&i| pipes[i].is_some()).collect();
        if let Some((_, at)) = exited
            && (open.is_empty() || at.elapsed() >= EXIT_GRACE)
        {
            break Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            break Err(RunError::TimedOut);
        }
        // 자식이 끝났는지 자주 보도록 짧게 기다린다.
        let wait = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(20));
        if open.is_empty() {
            // 파이프는 다 닫혔고 자식의 끝만 남았다.
            std::thread::sleep(wait);
            continue;
        }
        let mut fds: Vec<libc::pollfd> = open
            .iter()
            .filter_map(|&i| pipes[i].as_ref())
            .map(|p| libc::pollfd {
                fd: p.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        let millis = libc::c_int::try_from(wait.as_millis().max(1)).unwrap_or(libc::c_int::MAX);
        let nfds = libc::nfds_t::try_from(fds.len()).unwrap_or(2);
        // SAFETY: `fds` 의 유효한 pollfd 배열과 그 길이를 넘긴다.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), nfds, millis) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            // poll 자체가 안 되면 읽은 데까지로 끝낸다(자식은 아래에서 거둔다).
            break Ok(());
        }
        for (fd, &i) in fds.iter().zip(&open) {
            if fd.revents == 0 {
                continue;
            }
            let Some(pipe) = pipes[i].as_mut() else {
                continue;
            };
            // poll 이 읽을 것(또는 EOF · 오류)이 있다고 했으므로 막히지 않는다.
            match pipe.read(&mut chunk) {
                Ok(0) => pipes[i] = None,
                Ok(n) => bufs[i].extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => pipes[i] = None,
            }
        }
        if bufs[0].len() + bufs[1].len() > limit {
            break Err(RunError::TooLarge);
        }
    };
    drop(pipes);
    if outcome.is_err() || exited.is_none() {
        // 시간을 넘겼거나 상한을 넘겼다. 세션째 끝낸다 — setsid 로 자식 pid 가 그룹 id 다.
        // SAFETY: killpg(2) 는 신호를 보낼 뿐이다.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    // 죽였든 스스로 끝났든 거둔다(좀비를 남기지 않는다).
    let status = match exited {
        Some((status, _)) => status,
        None => child.wait().map_err(|e| RunError::Spawn(e.to_string()))?,
    };
    outcome?;
    let [stdout, stderr] = bufs;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Windows: 읽기 스레드 둘과 끝 확인으로 같은 일을 한다(세션 대신 자식만 죽인다).
#[cfg(not(unix))]
pub fn run(mut command: Command, timeout: Duration, limit: usize) -> Result<Output, RunError> {
    use std::io::Read;
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Instant;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    fn reader(mut pipe: impl Read + Send + 'static, limit: usize) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe
                .by_ref()
                .take(u64::try_from(limit).unwrap_or(u64::MAX) + 1)
                .read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        rx
    }
    let out = child.stdout.take().map(|p| reader(p, limit));
    let err = child.stderr.take().map(|p| reader(p, limit));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::TimedOut);
            }
        }
    };
    let collect = |rx: Option<mpsc::Receiver<Vec<u8>>>| {
        rx.and_then(|rx| rx.recv_timeout(EXIT_GRACE).ok())
            .unwrap_or_default()
    };
    let (stdout, stderr) = (collect(out), collect(err));
    if stdout.len() + stderr.len() > limit {
        return Err(RunError::TooLarge);
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Instant;

    use super::*;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", script]);
        c
    }

    #[test]
    fn collects_stdout_stderr_and_the_exit_code() {
        let out = run(
            sh("printf out; printf err >&2; exit 3"),
            Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
        assert_eq!(out.status.code(), Some(3));
    }

    /// 시간을 넘기면 세션째 죽인다 — 자식이 띄운 손자도 함께 사라진다.
    #[test]
    fn a_hanging_command_is_killed_with_its_session() {
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("pid");
        let started = Instant::now();
        let err = run(
            sh(&format!(
                "sleep 30 & echo $! > '{}'; wait",
                pidfile.display()
            )),
            Duration::from_millis(300),
            1024,
        )
        .unwrap_err();
        assert_eq!(err, RunError::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        // SAFETY: 신호 0 은 프로세스가 있는지만 본다.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(Instant::now() < deadline, "the grandchild {pid} survived");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 자식이 끝났으면 손자가 stdout 을 쥐고 있어도 기다리지 않는다.
    #[test]
    fn does_not_wait_for_a_grandchild_holding_stdout() {
        let started = Instant::now();
        let out = run(
            sh("(sleep 5; echo late) & printf done"),
            Duration::from_secs(4),
            1024,
        )
        .unwrap();
        assert_eq!(out.stdout, b"done");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn output_over_the_limit_fails() {
        let err = run(sh("head -c 100000 /dev/zero"), Duration::from_secs(5), 1000).unwrap_err();
        assert_eq!(err, RunError::TooLarge);
    }

    #[test]
    fn a_missing_program_fails_to_spawn() {
        let err = run(
            Command::new("/nonexistent/mise"),
            Duration::from_secs(1),
            1000,
        )
        .unwrap_err();
        assert!(matches!(err, RunError::Spawn(_)), "{err:?}");
    }
}
