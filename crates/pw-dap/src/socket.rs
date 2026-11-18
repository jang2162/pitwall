//! 소켓 경로로 말하는 어댑터의 자리와 붙기(17-m6-plan.md M6-15 ④ — 리뷰 F1, 메인 판정: 포트 대신 소켓 경로).
//!
//! 포트(`127.0.0.1`)는 다른 기기만 막는다 — 같은 기기의 다른 사용자는 그 포트에 붙을 수 있고, Pitwall 이 빈 포트를 고른 뒤 어댑터가
//! 묶기 전에 다른 프로세스가 먼저 묶으면 Pitwall 이 그 상대에 붙는다. 소켓 경로는 둘 다 막는다:
//!
//! - **unix** — 세션 나무마다 임시 폴더 안에 **새로** 만든 `0700` 폴더(이미 있으면 만들지 않고 실패한다 — 남이 미리 만든 폴더를 쓰지
//!   않는다) 안의 소켓. 다른 사용자는 그 폴더에 들어오지도, 소켓을 미리 만들지도 못한다. 붙은 뒤 상대가 어댑터의 프로세스 그룹
//!   (어댑터 pid 가 그룹 id)이고 같은 사용자인지 다시 본다 — 아니면 붙지 않는다.
//! - **Windows** — `\\.\pipe\pitwall-dap-<임의 값>` named pipe. 파이프 이름 공간은 기기 전체라 폴더 권한 같은 울타리가 없다 — 붙은 뒤
//!   그 파이프의 서버 프로세스(`GetNamedPipeServerProcessId`)가 어댑터 pid 인지 본다. 이 길은 CI 의 Windows 빌드로만 확인했다.

use std::io;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncRead, AsyncWrite};

/// 읽기 · 쓰기 반쪽.
pub(crate) type Halves = (
    Box<dyn AsyncRead + Unpin + Send>,
    Box<dyn AsyncWrite + Unpin + Send>,
);

/// 세션 나무 하나의 소켓 자리. 버리면 폴더째 지운다(unix).
#[derive(Debug)]
pub(crate) struct SocketPath {
    path: PathBuf,
    #[cfg(unix)]
    dir: PathBuf,
}

impl SocketPath {
    /// 새 자리를 만든다.
    pub(crate) fn new() -> io::Result<Self> {
        #[cfg(unix)]
        {
            let base = std::env::temp_dir();
            let mut last = None;
            for _ in 0..8 {
                match Self::in_dir(base.join(format!("pw-dap-{}", unique()))) {
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
                    other => return other,
                }
            }
            Err(last.unwrap_or_else(|| io::Error::other("cannot make a socket folder")))
        }
        #[cfg(windows)]
        {
            Ok(Self {
                path: PathBuf::from(format!(r"\\.\pipe\pitwall-dap-{}", unique())),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(io::Error::other("socket adapters are not supported here"))
        }
    }

    /// `dir` 을 `0700` 으로 **새로** 만들고 그 안의 소켓 자리(이미 있으면 `AlreadyExists`).
    #[cfg(unix)]
    pub(crate) fn in_dir(dir: PathBuf) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        // umask 는 권한을 줄이기만 하지만, 뜻을 그대로 남기려고 한 번 더 정한다.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            path: dir.join("dap.sock"),
            dir,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// 이름에 넣을 임의 값(맞히기 어렵게 — 프로세스 · 시각 · 차례를 섞은 해시 둘).
fn unique() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut parts = [0u64; 2];
    for part in &mut parts {
        let mut h = RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        h.write_u128(nanos);
        h.write_u64(n);
        *part = h.finish();
    }
    format!("{:016x}{:016x}", parts[0], parts[1])
}

/// 붙기 한 번의 결과.
pub(crate) enum Attempt {
    Connected(Halves),
    /// 아직 열리지 않았다(다시 해 본다).
    NotYet(io::Error),
    /// 열렸으나 상대가 어댑터가 아니다(다시 하지 않는다).
    Impostor(String),
}

/// `path` 에 한 번 붙어 본다. 상대는 `adapter_pid` 의 어댑터여야 한다(머리 주석).
pub(crate) async fn connect_once(path: &Path, adapter_pid: Option<u32>) -> Attempt {
    #[cfg(unix)]
    {
        let stream = match tokio::net::UnixStream::connect(path).await {
            Ok(stream) => stream,
            Err(e) => return Attempt::NotYet(e),
        };
        if let Err(why) = check_unix_peer(&stream, adapter_pid) {
            return Attempt::Impostor(why);
        }
        let (read, write) = stream.into_split();
        Attempt::Connected((Box::new(read), Box::new(write)))
    }
    #[cfg(windows)]
    {
        let client = match tokio::net::windows::named_pipe::ClientOptions::new().open(path) {
            Ok(client) => client,
            Err(e) => return Attempt::NotYet(e),
        };
        if let Err(why) = check_pipe_server(&client, adapter_pid) {
            return Attempt::Impostor(why);
        }
        let (read, write) = tokio::io::split(client);
        Attempt::Connected((Box::new(read), Box::new(write)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, adapter_pid);
        Attempt::Impostor("socket adapters are not supported here".to_owned())
    }
}

/// 상대가 같은 사용자이고 어댑터의 프로세스 그룹 안인가.
#[cfg(unix)]
fn check_unix_peer(
    stream: &tokio::net::UnixStream,
    adapter_pid: Option<u32>,
) -> Result<(), String> {
    let cred = stream
        .peer_cred()
        .map_err(|e| format!("cannot read the socket peer: {e}"))?;
    // SAFETY: geteuid(2) 는 늘 성공하고 메모리를 건드리지 않는다.
    let me = unsafe { libc::geteuid() };
    if cred.uid() != me {
        return Err(format!(
            "the socket is served by another user (uid {})",
            cred.uid()
        ));
    }
    let (Some(peer), Some(adapter)) = (cred.pid(), adapter_pid) else {
        return Err("cannot tell which process serves the socket".to_owned());
    };
    // SAFETY: getpgid(2) 는 그 pid 의 그룹 id 를 읽을 뿐이다(없으면 -1).
    let group = unsafe { libc::getpgid(peer) };
    if i64::from(group) != i64::from(adapter) {
        return Err(format!(
            "the socket is served by process {peer}, which is not the debug adapter {adapter}"
        ));
    }
    Ok(())
}

/// 파이프의 서버 프로세스가 어댑터인가.
#[cfg(windows)]
fn check_pipe_server(
    client: &tokio::net::windows::named_pipe::NamedPipeClient,
    adapter_pid: Option<u32>,
) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    let mut server: u32 = 0;
    // SAFETY: 살아 있는 파이프 핸들과 u32 하나를 넘긴다.
    let ok = unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as _, &mut server) };
    if ok == 0 {
        return Err(format!(
            "cannot read the pipe's server process: {}",
            io::Error::last_os_error()
        ));
    }
    match adapter_pid {
        Some(adapter) if adapter == server => Ok(()),
        _ => Err(format!(
            "the pipe is served by process {server}, which is not the debug adapter"
        )),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// 소켓 폴더는 이 사용자만 들어가는 `0700` 이고, 이미 있는 폴더(남이 미리 만든 것)는 쓰지 않는다. 버리면 지운다.
    #[test]
    fn the_socket_folder_is_private_and_never_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("taken");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = SocketPath::in_dir(dir.clone()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o777,
            "a folder someone else made is left alone"
        );
        let fresh = SocketPath::new().unwrap();
        let folder = fresh.path().parent().unwrap().to_path_buf();
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // unix 소켓 경로 상한(macOS 104 바이트) 안.
        assert!(
            fresh.path().as_os_str().len() < 104,
            "{}",
            fresh.path().display()
        );
        drop(fresh);
        assert!(!folder.exists());
    }
}
