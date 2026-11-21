//! `pitwall-relay proxy`(08 「연결 · 재연결」 — 18-m7-plan M7-04 ②): SSH 세션의 stdio 를 데몬 소켓에 잇는다. 데몬이 없으면 띄우고
//! (`setsid` 로 SSH 세션과 떼어 — 세션이 끊겨도 데몬 · 실행이 남는다) 붙는다. 띄운 데몬이 [`SPAWN_DEADLINE`] 안에 받지 않으면 진단
//! 글과 함께 끝난다.
//!
//! stdout 은 링크 바이트다 — 진단은 stderr 에만 쓴다.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// 띄운 데몬이 소켓에서 받기까지 기다리는 시간.
pub const SPAWN_DEADLINE: Duration = Duration::from_secs(2);
const RETRY_EVERY: Duration = Duration::from_millis(20);

/// 데몬에 붙는다. 없으면 `spawn` 으로 띄우고 [`SPAWN_DEADLINE`] 동안 다시 붙어 본다.
pub async fn connect_or_spawn(
    socket: &Path,
    uid: u32,
    spawn: impl FnOnce() -> io::Result<()>,
) -> Result<UnixStream, String> {
    match crate::socket::connect(socket, uid).await {
        Ok(stream) => return Ok(stream),
        // 폴더 · 소켓이 안전하지 않으면 데몬을 띄워도 같다 — 곧바로 알린다.
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            return Err(format!("cannot use the relay socket: {e}"));
        }
        Err(_) => {}
    }
    spawn().map_err(|e| format!("cannot start the relay daemon: {e}"))?;
    let deadline = tokio::time::Instant::now() + SPAWN_DEADLINE;
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        match crate::socket::connect(socket, uid).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(RETRY_EVERY).await;
    }
    Err(format!(
        "the relay daemon did not accept on {} within {} s ({})",
        socket.display(),
        SPAWN_DEADLINE.as_secs(),
        last.map_or_else(|| "no attempt".to_owned(), |e| e.to_string())
    ))
}

/// 이 바이너리를 데몬으로 띄운다: 새 세션(`setsid`), stdio 는 `/dev/null`(로그는 파일로). 기다리지 않는다.
pub fn spawn_daemon(program: &Path, args: &[&str]) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: fork 뒤 exec 전에 async-signal-safe 함수(setsid) 하나만 부른다.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // 자식은 기다리지 않는다 — 데몬은 이 프로세스보다 오래 산다. 좀비는 setsid 뒤 init 이 거둔다(이 프로세스가 먼저 끝난다).
    command.spawn().map(drop)
}

/// stdin → 소켓, 소켓 → stdout. 데몬이 닫으면 끝난다. stdin 이 먼저 끝나면 소켓의 쓰기 쪽만 닫고(반닫기) 데몬이 남은 바이트를 다
/// 보내고 닫을 때까지 stdout 으로 흘린다.
pub async fn pipe(stream: UnixStream) -> io::Result<()> {
    pipe_io(stream, tokio::io::stdin(), tokio::io::stdout()).await
}

/// [`pipe`] 의 몸통(시험은 stdin · stdout 대신 다른 스트림을 준다).
pub async fn pipe_io(
    stream: UnixStream,
    mut input: impl tokio::io::AsyncRead + Unpin,
    mut output: impl tokio::io::AsyncWrite + Unpin,
) -> io::Result<()> {
    let (mut from_daemon, mut to_daemon) = stream.into_split();
    let down = async {
        tokio::io::copy(&mut from_daemon, &mut output).await?;
        output.flush().await
    };
    tokio::pin!(down);
    let up = async {
        tokio::io::copy(&mut input, &mut to_daemon).await?;
        to_daemon.shutdown().await
    };
    tokio::select! {
        result = &mut down => return result,
        result = up => result?,
    }
    down.await
}

/// 로그 파일 자리(진단 글에 싣는다).
pub fn log_hint(paths: &pw_core::config::AppPaths) -> PathBuf {
    crate::paths::logs_dir(paths)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// stdin 이 먼저 끝나도 데몬이 그 뒤에 보낸 바이트는 stdout 에 다 나간다(데몬은 요청을 끝까지 읽은 뒤에 답한다).
    #[tokio::test]
    async fn the_answer_after_stdin_ends_still_reaches_stdout() {
        let (proxy_side, mut daemon_side) = UnixStream::pair().unwrap();
        let daemon = tokio::spawn(async move {
            let mut request = Vec::new();
            daemon_side.read_to_end(&mut request).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            daemon_side.write_all(b"answer:").await.unwrap();
            daemon_side.write_all(&request).await.unwrap();
        });
        let mut output = Vec::new();
        pipe_io(proxy_side, &b"request"[..], &mut output)
            .await
            .unwrap();
        daemon.await.unwrap();
        assert_eq!(output, b"answer:request");
    }
}
