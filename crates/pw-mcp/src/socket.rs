//! 실행 중인 앱의 MCP 로컬 소켓(unix). `pitwall mcp` 가 여기에 붙어 stdio 를 잇는다(`docs/design/05-mcp.md` §1-1).
//!
//! **그 사용자만 붙는다.** 네트워크 포트를 열지 않는다. 막는 것은 세 겹이다:
//! 1. 소켓은 그 사용자 전용 폴더(0700, 이 사용자 소유, 심링크 아님)에 둔다. 서버는 폴더를 만들거나 0700 으로 고치고,
//!    다른 사용자의 폴더 · 심링크면 소켓을 열지 않는다. 클라이언트는 같은 검사를 통과한 폴더의 소켓에만 붙는다.
//! 2. 소켓 파일은 0600. (macOS 는 소켓 파일 권한으로 connect 를 막지 않으므로 1 이 실제 방어선이다.)
//! 3. 연결마다 상대 프로세스의 uid(`getpeereid` / `SO_PEERCRED`)가 이 프로세스와 같아야 한다 — 양쪽 다 본다.
//!
//! **워크트리끼리 섞이지 않는다.** 소켓 이름은 단일 인스턴스 키(`apps/desktop/src-tauri/src/instance.rs` — debug 빌드는 빌드
//! 폴더 + `$DATA`, release 는 앱 identifier)와 `$STATE` 의 해시다. A 워크트리의 `pitwall mcp` 는 B 워크트리의 dev 앱이나
//! 사람의 정식 앱 소켓 이름을 계산하지 않는다.
//!
//! **위치**: `$STATE/run/<해시>.sock`. 그 경로가 `sun_path` 한도([`MAX_PATH`])를 넘으면(깊은 `PITWALL_STATE_DIR`)
//! `<임시 폴더>/pitwall-<uid>/<해시>.sock`(macOS 의 임시 폴더 `$TMPDIR` 는 사용자 전용이다). 양쪽이 같은 규칙으로 고른다.
//!
//! **이미 있는 소켓**: 붙어 보고 받으면 다른 앱이 쓰는 중이라 열지 않는다. 받지 않으면(앱이 죽고 남은 파일) 지우고 새로 연다.
//! 소켓이 아닌 파일 · 심링크는 지우지 않고 오류로 둔다.
//!
//! **핸드셰이크**: 연결 직후 클라이언트가 JSON 한 줄([`Hello`])을 보내고 서버가 한 줄([`Welcome`])로 답한다. 그 뒤로는
//! MCP stdio 바이트(줄 단위 JSON-RPC)가 그대로 흐른다.

use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;

use crate::server::{Services, SessionRequest, serve};

/// 소켓 경로의 최대 바이트 수. macOS `sun_path` 는 104바이트(끝의 NUL 포함), Linux 는 108. 작은 쪽에 맞춘다.
pub const MAX_PATH: usize = 103;
/// 핸드셰이크 한 줄의 최대 크기와 기다리는 시간.
pub const MAX_HELLO: usize = 64 * 1024;
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// [`Hello::protocol`].
pub const PROTOCOL: &str = "pitwall-mcp/1";

/// 클라이언트(`pitwall mcp`)가 처음 보내는 줄.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: String,
    #[serde(flatten)]
    pub request: SessionRequest,
}

/// 서버의 답. `ok` 가 아니면 서버가 연결을 닫는다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// 소켓 경로를 고른다. `key` 는 단일 인스턴스 키, `temp` 는 [`std::env::temp_dir`].
pub fn endpoint(key: &str, state: &Path, temp: &Path, uid: u32) -> io::Result<PathBuf> {
    let name = format!(
        "{:016x}.sock",
        fnv(&[key.as_bytes(), state.as_os_str().as_encoded_bytes()])
    );
    [state.join("run"), temp.join(format!("pitwall-{uid}"))]
        .into_iter()
        .map(|dir| dir.join(&name))
        .find(|path| path.as_os_str().len() <= MAX_PATH)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "no socket path fits in {MAX_PATH} bytes (state: {})",
                    state.display()
                ),
            )
        })
}

pub fn current_uid() -> u32 {
    // SAFETY: geteuid 는 실패하지 않고 상태를 바꾸지 않는다.
    unsafe { libc::geteuid() }
}

/// 소켓 폴더를 만들거나 고쳐 이 사용자만 들어오게 한다(서버 쪽).
pub fn prepare_dir(dir: &Path, uid: u32) -> io::Result<()> {
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
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// 붙기 전에 폴더와 소켓 파일을 본다(클라이언트 쪽). 고치지 않는다.
pub fn check_endpoint(path: &Path, uid: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(insecure(dir, "is not a directory (or is a symlink)"));
    }
    if meta.uid() != uid {
        return Err(insecure(dir, "belongs to another user"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(insecure(dir, "is open to other users"));
    }
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_socket() {
        return Err(insecure(path, "is not a socket"));
    }
    if meta.uid() != uid {
        return Err(insecure(path, "belongs to another user"));
    }
    Ok(())
}

fn insecure(path: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} {why}", path.display()),
    )
}

/// 소켓을 연다. 끝나면 [`Listener`] 를 버리면 파일이 지워진다.
pub fn bind(path: &Path, uid: u32) -> io::Result<Listener> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    prepare_dir(dir, uid)?;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("{} is served by another running app", path.display()),
                ));
            }
            // 받는 쪽이 없다 — 죽은 앱이 남긴 파일이다.
            std::fs::remove_file(path)?;
        }
        Ok(_) => return Err(insecure(path, "exists and is not a socket")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let ino = std::fs::symlink_metadata(path)?.ino();
    Ok(Listener {
        inner: listener,
        path: path.to_path_buf(),
        ino,
        uid,
    })
}

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
}

impl Drop for Listener {
    fn drop(&mut self) {
        // 그사이 다른 앱이 같은 이름으로 다시 열었으면 그쪽 파일이다. 지우지 않는다.
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.ino) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// 열려 있는 세션 수. 시험과 로그가 본다.
#[derive(Clone, Default)]
pub struct Connections(Arc<AtomicUsize>);

impl Connections {
    pub fn active(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

struct Counted(Connections);

impl Counted {
    fn new(c: &Connections) -> Self {
        c.0.fetch_add(1, Ordering::SeqCst);
        Self(c.clone())
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 연결을 받아 세션마다 [`serve`] 를 돌린다. `cancel` 이 취소되면 받기를 멈추고 열린 세션도 닫는다.
pub async fn accept_loop(
    listener: Listener,
    services: Arc<Services>,
    connections: Connections,
    cancel: CancellationToken,
) {
    loop {
        let stream = tokio::select! {
            () = cancel.cancelled() => return,
            accepted = listener.inner.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!(%e, "mcp socket accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        match stream.peer_cred().map(|c| c.uid()) {
            Ok(peer) if peer == listener.uid => {}
            Ok(peer) => {
                tracing::warn!(peer, "rejected an mcp socket connection from another user");
                continue;
            }
            Err(e) => {
                tracing::warn!(%e, "rejected an mcp socket connection without peer credentials");
                continue;
            }
        }
        let services = Arc::clone(&services);
        let counted = Counted::new(&connections);
        let cancel = cancel.child_token();
        tokio::spawn(async move {
            let _counted = counted;
            if let Err(e) = session(stream, services, cancel).await {
                tracing::warn!(%e, "mcp socket session failed");
            }
        });
    }
}

async fn session(
    stream: UnixStream,
    services: Arc<Services>,
    cancel: CancellationToken,
) -> io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let hello = tokio::time::timeout(HELLO_TIMEOUT, read_line(&mut read))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no handshake"))??;
    let hello = match serde_json::from_slice::<Hello>(&hello) {
        Ok(h) if h.protocol == PROTOCOL => h,
        Ok(h) => {
            return reject(
                &mut write,
                &format!("unsupported protocol `{}`", h.protocol),
            )
            .await;
        }
        Err(e) => return reject(&mut write, &format!("bad handshake: {e}")).await,
    };
    write_line(
        &mut write,
        &Welcome {
            ok: true,
            error: None,
        },
    )
    .await?;
    serve(services, hello.request, (read, write), cancel)
        .await
        .map_err(io::Error::other)
}

async fn reject(write: &mut tokio::net::unix::OwnedWriteHalf, error: &str) -> io::Result<()> {
    write_line(
        write,
        &Welcome {
            ok: false,
            error: Some(error.to_owned()),
        },
    )
    .await
}

/// `\n` 까지 읽는다(줄바꿈은 뺀다). [`MAX_HELLO`] 를 넘으면 오류.
pub async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(read: &mut R) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut limited = read.take(MAX_HELLO as u64 + 1);
    let n = limited.read_until(b'\n', &mut line).await?;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "closed before the handshake",
        ));
    }
    if line.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "handshake line too long",
        ));
    }
    line.pop();
    Ok(line)
}

pub async fn write_line<W: tokio::io::AsyncWrite + Unpin, T: Serialize>(
    write: &mut W,
    value: &T,
) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    write.write_all(&line).await?;
    write.flush().await
}

/// 앱에 붙는다(클라이언트 쪽): 폴더 · 파일 검사 → 연결 → 상대 uid → 핸드셰이크. 앱이 없으면 `NotFound` · `ConnectionRefused`.
pub async fn connect(path: &Path, uid: u32, request: SessionRequest) -> io::Result<UnixStream> {
    check_endpoint(path, uid)?;
    let mut stream = UnixStream::connect(path).await?;
    let peer = stream.peer_cred()?.uid();
    if peer != uid {
        return Err(insecure(path, "is served by another user"));
    }
    write_line(
        &mut stream,
        &Hello {
            protocol: PROTOCOL.into(),
            request,
        },
    )
    .await?;
    // 답 한 줄만 읽는다. BufReader 로 더 읽어 두면 그 뒤의 MCP 바이트를 잃으므로 한 바이트씩 읽는다(한 번뿐이다).
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let read = async {
        loop {
            let n = stream.read(&mut byte).await?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the app closed the handshake",
                ));
            }
            if byte[0] == b'\n' {
                return Ok::<_, io::Error>(());
            }
            line.push(byte[0]);
            if line.len() > MAX_HELLO {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "handshake reply too long",
                ));
            }
        }
    };
    tokio::time::timeout(HELLO_TIMEOUT, read)
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "the app did not answer the handshake",
            )
        })??;
    let welcome: Welcome = serde_json::from_slice(&line).map_err(io::Error::other)?;
    if !welcome.ok {
        return Err(io::Error::other(format!(
            "the app refused the session: {}",
            welcome.error.unwrap_or_default()
        )));
    }
    Ok(stream)
}

/// 실행마다 같은 값을 내야 하므로(두 프로세스가 같은 이름을 계산한다) 표준 라이브러리의 무작위 해시가 아닌 FNV-1a.
/// 조각 사이에 0 바이트를 넣어 `("a", "bc")` 와 `("ab", "c")` 를 가른다.
fn fnv(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, part) in parts.iter().enumerate() {
        let separator: &[u8] = if i > 0 { &[0] } else { &[] };
        for b in separator.iter().chain(part.iter()) {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}
