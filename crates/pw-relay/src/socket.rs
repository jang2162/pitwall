//! 데몬 소켓(18-m7-plan M7-04 ①). 규칙은 MCP 로컬 소켓(`pw_mcp::socket`)과 같다 — 네트워크 포트를 열지 않고, 그 사용자만 붙는다:
//!
//! 1. 소켓은 그 사용자 전용 폴더에 둔다. 폴더가 없으면 0700 으로 만들고, **있는 폴더가 0700 이 아니거나 다른 사용자의 것이거나
//!    심링크면 소켓을 열지 않는다**(MCP 소켓은 넓은 권한을 고쳐 쓰지만, Relay 는 남이 미리 만들어 둔 자리일 수 있어 고치지 않고
//!    멈춘다 — 사람이 고친다). 붙는 쪽(`proxy`)도 같은 폴더 검사를 통과한 소켓에만 붙는다.
//! 2. 소켓 파일은 0600.
//! 3. 연결마다 상대 uid 가 이 프로세스와 같아야 한다(`pw_mcp::socket::peer_allowed`) — 양쪽 다 본다. 같은 uid 의 다른 프로세스는
//!    여기서 막히지 않는다 — 링크는 기기 키 증명(M7-03)이 막고, 좁은 명령은 role · 범위를 얻지 못한다(`crate::command`).
//!
//! **자리**: `$STATE/relay/<채널>-<판>.sock`(채널 · 판마다 따로 — 18-m7-plan M7-07 ①: 다른 판 · 다른 채널의 데몬에는 붙지 않고 제
//! 판의 데몬을 따로 띄운다). `sun_path` 한도를 넘으면 `<임시 폴더>/pitwall-relay-<uid>/<해시>-<채널>-<판>.sock`. 데몬과 `proxy` 가
//! 같은 규칙으로 고른다. **한 판에 데몬 하나** — 같은 자리의 `.lock` 을 `flock` 으로 쥔 데몬만 소켓을 연다.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use pw_core::config::AppPaths;
use pw_mcp::socket::{MAX_PATH, check_endpoint, peer_allowed};
use tokio::net::{UnixListener, UnixStream};

pub use pw_mcp::socket::current_uid;

/// 판 이름을 파일 이름에 쓸 수 있는 글자로(그 밖의 글자는 `_`).
pub fn version_slug(version: &str) -> String {
    version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 그 (릴리스 채널, 판)의 데몬 소켓 자리. `paths` 는 Relay 의 앱 경로, `temp` 는 [`std::env::temp_dir`]. 채널과 판이 둘 다 이름에
/// 든다 — 같은 판 문자열의 다른 채널(`dev` 와 `stable` 의 `1.0.0`)도 데몬을 나누지 않는다(M7-04 리뷰 권고 8).
pub fn endpoint(
    paths: &AppPaths,
    channel: &str,
    version: &str,
    temp: &Path,
    uid: u32,
) -> io::Result<PathBuf> {
    let slug = format!("{}-{}", version_slug(channel), version_slug(version));
    let primary = paths.state.join(format!("{slug}.sock"));
    if primary.as_os_str().len() <= MAX_PATH {
        return Ok(primary);
    }
    let fallback = temp.join(format!("pitwall-relay-{uid}")).join(format!(
        "{:016x}-{slug}.sock",
        fnv(paths.state.as_os_str().as_encoded_bytes())
    ));
    if fallback.as_os_str().len() <= MAX_PATH {
        Ok(fallback)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "no socket path fits in {MAX_PATH} bytes (state: {})",
                paths.state.display()
            ),
        ))
    }
}

/// 소켓 폴더를 이 사용자 전용으로 만들거나 확인한다(데몬 쪽). 있는 폴더는 고치지 않는다 — 0700 이 아니면 거절한다.
pub fn private_dir(dir: &Path, uid: u32) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(insecure(dir, "is not a directory (or is a symlink)"));
    }
    if meta.uid() != uid {
        return Err(insecure(dir, "belongs to another user"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(insecure(
            dir,
            "is open to other users (make it 0700 or remove it)",
        ));
    }
    Ok(())
}

fn insecure(path: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} {why}", path.display()),
    )
}

/// 그 판의 데몬 자리를 쥔다. 다른 데몬이 쥐고 있으면 `Ok(None)`. 쥔 동안 [`DaemonLock`] 을 놓지 않는다.
pub fn lock(endpoint: &Path, uid: u32) -> io::Result<Option<DaemonLock>> {
    let dir = endpoint.parent().unwrap_or(Path::new("/"));
    private_dir(dir, uid)?;
    let path = endpoint.with_extension("lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    // SAFETY: 열린 파일의 fd 다. flock 은 메모리를 건드리지 않는다.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if locked == 0 {
        return Ok(Some(DaemonLock { _file: file }));
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(e)
    }
}

/// 그 판의 데몬 자리를 누가 쥐고 있는가 — 「데몬 없음」 을 확실히 하려고 본다(`status`). 붙기가 거절(`ConnectionRefused`)돼도
/// 데몬이 없다는 뜻만은 아니다: macOS 는 받는 줄(listen backlog)이 찬 소켓에도 같은 오류를 낸다(CI 열째 판 조사). 자리 파일이
/// 없으면 `Ok(false)`. 공유 잠금을 잠깐 잡아 보고 곧바로 놓는다 — 그 사이에 뜨는 데몬은 자리를 못 쥐고 물러난다(붙는 쪽이 다시 띄운다).
pub fn held(endpoint: &Path) -> io::Result<bool> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = endpoint.with_extension("lock");
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    // SAFETY: 열린 파일의 fd 다. flock 은 메모리를 건드리지 않는다.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if locked == 0 {
        // 파일을 닫으면 잠금도 놓인다.
        return Ok(false);
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::WouldBlock {
        Ok(true)
    } else {
        Err(e)
    }
}

/// 쥔 데몬 자리. 버리면(프로세스가 끝나면) 놓인다.
pub struct DaemonLock {
    _file: File,
}

/// 소켓을 연다. [`lock`] 을 쥔 데몬만 부른다 — 남은 소켓 파일(죽은 데몬의 것)은 지우고 새로 연다. 소켓이 아닌 파일 · 심링크는
/// 지우지 않고 오류다.
pub fn bind(path: &Path, uid: u32, _lock: &DaemonLock) -> io::Result<Listener> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    private_dir(dir, uid)?;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path)?,
        Ok(_) => return Err(insecure(path, "exists and is not a socket")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let inner = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let ino = std::fs::symlink_metadata(path)?.ino();
    Ok(Listener {
        inner,
        path: path.to_path_buf(),
        ino,
        uid,
    })
}

/// 열린 데몬 소켓. 버리면 파일을 지운다(그사이 다른 데몬이 같은 이름으로 다시 열었으면 그쪽 파일이라 두고).
pub struct Listener {
    inner: UnixListener,
    path: PathBuf,
    ino: u64,
    uid: u32,
}

impl Listener {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 다음 연결. 상대 uid 가 다르면(또는 읽지 못하면) 그 연결은 버리고 `Err`.
    pub async fn accept(&self) -> io::Result<UnixStream> {
        let (stream, _) = self.inner.accept().await?;
        peer_allowed(stream.peer_cred().map(|c| c.uid()), self.uid)?;
        Ok(stream)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.ino) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// 데몬 소켓에 붙는다(`proxy` · 좁은 명령). 폴더 · 소켓 파일 · 상대 uid 를 본다.
pub async fn connect(path: &Path, uid: u32) -> io::Result<UnixStream> {
    check_endpoint(path, uid)?;
    let stream = UnixStream::connect(path).await?;
    peer_allowed(stream.peer_cred().map(|c| c.uid()), uid)?;
    Ok(stream)
}

/// 실행마다 같은 값(두 프로세스가 같은 이름을 계산한다) — FNV-1a.
fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(root: &Path) -> AppPaths {
        crate::paths::relay_paths(&AppPaths::under(root))
    }

    /// 다른 uid 의 폴더 · 0700 이 아닌 폴더 · 심링크 폴더면 소켓을 열지 않는다(18-m7-plan M7-04 시험 ③). 고치지도 않는다.
    #[test]
    fn the_socket_is_not_opened_in_an_unsafe_folder() {
        let dir = tempfile::tempdir().unwrap();
        let uid = current_uid();

        let open = dir.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = lock(&open.join("1.0.0.sock"), uid)
            .err()
            .expect("0755 folder");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert_eq!(
            std::fs::metadata(&open).unwrap().mode() & 0o777,
            0o755,
            "the folder was changed"
        );
        assert!(!open.join("1.0.0.lock").exists());

        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = lock(&link.join("1.0.0.sock"), uid)
            .err()
            .expect("symlink folder");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");

        // 다른 사용자의 폴더(이 사용자 폴더를 다른 uid 로 묻는다 — 시험은 root 가 아니다).
        let err = lock(&real.join("1.0.0.sock"), uid + 1)
            .err()
            .expect("another user");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");

        // 안전한 폴더에서는 열리고 0600 이다.
        let held = lock(&real.join("1.0.0.sock"), uid).unwrap().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let listener = bind(&real.join("1.0.0.sock"), uid, &held).unwrap();
            let mode = std::fs::metadata(listener.path()).unwrap().mode() & 0o777;
            assert_eq!(mode, 0o600);
        });
    }

    /// 한 판에 데몬 하나 — 자리를 쥔 동안 같은 판의 둘째는 `None`, 다른 판은 따로 쥔다.
    #[test]
    fn one_daemon_holds_a_version() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let uid = current_uid();
        let temp = std::env::temp_dir();
        let a = endpoint(&paths, "dev", "1.0.0", &temp, uid).unwrap();
        let b = endpoint(&paths, "dev", "1.1.0", &temp, uid).unwrap();
        assert_ne!(a, b);
        let held = lock(&a, uid).unwrap().expect("first");
        assert!(lock(&a, uid).unwrap().is_none());
        assert!(super::held(&a).unwrap(), "a held place is held");
        assert!(!super::held(&b).unwrap(), "no lock file is not held");
        let other = lock(&b, uid).unwrap();
        assert!(other.is_some());
        drop(held);
        assert!(!super::held(&a).unwrap(), "a let-go place is free");
        assert!(lock(&a, uid).unwrap().is_some());
    }

    /// 붙는 쪽도 폴더를 본다 — 0700 이 아닌 폴더의 소켓에는 (받는 쪽이 있어도) 붙지 않는다(리뷰 권고 2 — E4).
    #[tokio::test]
    async fn connect_refuses_a_socket_in_an_open_folder() {
        let dir = tempfile::tempdir().unwrap();
        let uid = current_uid();
        let open = dir.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = open.join("1.0.0.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let err = connect(&path, uid)
            .await
            .expect_err("connected through an open folder");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");

        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(connect(&path, uid).await.is_ok());
    }

    /// 판 · 채널이 다르면 소켓이 다르다 — 다른 판 데몬에는 붙지 않는다(18-m7-plan M7-07 시험 ①, M7-04 리뷰 권고 8). 판이나 채널을
    /// 이름에서 빼면 실패한다. 짧은 자리와 임시 폴더 대체 자리 둘 다.
    #[test]
    fn each_channel_and_version_has_its_own_socket() {
        let dir = tempfile::tempdir().unwrap();
        let temp = Path::new("/tmp");
        for paths in [
            paths(dir.path()),
            paths(&Path::new("/tmp").join("d".repeat(120))),
        ] {
            let all = [
                endpoint(&paths, "dev", "1.0.0", temp, 501).unwrap(),
                endpoint(&paths, "dev", "1.1.0", temp, 501).unwrap(),
                endpoint(&paths, "stable", "1.0.0", temp, 501).unwrap(),
                endpoint(&paths, "stable", "1.1.0", temp, 501).unwrap(),
            ];
            let distinct: std::collections::BTreeSet<_> = all.iter().collect();
            assert_eq!(distinct.len(), all.len(), "{all:?}");
            assert_eq!(
                all[0],
                endpoint(&paths, "dev", "1.0.0", temp, 501).unwrap(),
                "the same build finds the same socket"
            );
        }
    }

    #[test]
    fn a_deep_state_falls_back_to_the_temp_folder() {
        let deep = Path::new("/tmp").join("d".repeat(120));
        let paths = paths(&deep);
        let path = endpoint(&paths, "dev", "1.0.0", Path::new("/tmp"), 501).unwrap();
        assert!(path.starts_with("/tmp/pitwall-relay-501"), "{path:?}");
        assert!(path.to_str().unwrap().ends_with("-dev-1.0.0.sock"));
        assert_eq!(version_slug("1.0.0/../x y"), "1.0.0_.._x_y");
    }
}
