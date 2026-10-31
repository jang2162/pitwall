//! askpass 도우미(결정 M2-D8 · D43): git · ssh 가 암호 · 토큰 · 키 암호 · 호스트 키 확인을 물으면 그 프로젝트 창의 입력 다이얼로그로
//! 묻는다.
//!
//! - **길**: 쓰기 명령의 환경에 `GIT_ASKPASS` · `SSH_ASKPASS`(`SSH_ASKPASS_REQUIRE=force`)로 도우미 스크립트를 준다
//!   (`crate::cli::set_askpass`). 스크립트는 `pitwall askpass "<질문>"` 을 부르고, 그 서브커맨드([`client_main`])가 앱의 askpass
//!   소켓에 붙어 질문을 넘긴다. 앱은 그 쓰기의 프로젝트를 듣는 창에 묻고([`super::prompt`]) 답을 돌려준다. 도우미는 답을 stdout
//!   에 한 줄로 쓰고 0 으로 끝난다 — git · ssh 가 읽는 꼴이다. 답이 없거나(시간 초과 · 취소 · 듣는 창 없음) 소켓에 닿지 못하면 아무것도
//!   쓰지 않고 1 로 끝난다 — git 은 인증 실패로 끝난다(`GIT_TERMINAL_PROMPT=0` 이라 터미널로 묻지 않는다). ssh 가
//!   `SSH_ASKPASS_PROMPT` 로 알려 주는 질문의 종류(`confirm` 예 · 아니오, `none` 알림만)는 [`AskpassHint`] 로 함께 넘긴다. 도우미가
//!   답을 받기 전에 끝나면(ssh 가 알림 도우미를 끝냄 등) 앱은 연결이 끊긴 것을 보고 그 질문을 닫는다.
//! - **어느 쓰기에**: 인증을 물을 수 있는 쓰기([`super::WriteRequest::askpass`])에만 준다. DB 의 SSH 터널(M3-11, `pw_db::tunnel`)도
//!   ssh 를 띄울 때 [`Askpass::session`] 으로 같은 길을 쓴다 — 질문은 그 터널을 연 프로젝트의 창으로 간다.
//! - **섞이지 않게**: 소켓은 앱(주 인스턴스)이 하나 열고, 경로는 데스크톱이 단일 인스턴스 키와 `$STATE` 로 정한다(MCP 소켓과 같은
//!   규칙, `apps/desktop/src-tauri/src/main.rs` 의 `askpass_socket`). 도우미는 경로를 다시 계산하지 않고 환경 변수([`SOCKET_ENV`])로
//!   받는다 — 그 git 을 띄운 앱의 소켓이다. 질문에는 쓰기 한 번마다 새로 만든 토큰([`TOKEN_ENV`])을 붙이고, 앱은 지금 도는 쓰기의
//!   토큰만 받는다(그 토큰이 프로젝트를 정한다). 다른 앱의 토큰 · 끝난 쓰기의 토큰은 거절한다.
//! - **그 사용자만**: 소켓 폴더는 그 사용자 전용(0700, 그 사용자 소유, 심링크 아님)이고 소켓 파일은 0600 이다. 연결마다 상대의
//!   uid 가 같아야 받는다. 도우미도 붙기 전에 폴더와 소켓을 본다. 토큰은 같은 사용자의 다른 프로세스를 막는 비밀이 아니다(그
//!   사용자는 git 의 환경을 읽을 수 있다) — 질문을 그 쓰기에 묶는 표다.
//! - **비밀**: 답은 소켓으로 도우미에게만 간다. 로그 · Task 진행 · 알림에 쓰지 않는다. 질문의 문장도 로그에 쓰지 않는다.
//! - Windows 는 아직 소켓이 없어 askpass 를 주지 않는다 — 인증이 필요한 쓰기는 git 의 실패 원문으로 끝난다(D38, Windows 출시 준비).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::prompt::{AskpassHint, Prompts};

/// 도우미가 붙을 소켓의 경로.
pub const SOCKET_ENV: &str = "PITWALL_ASKPASS_SOCKET";
/// 그 쓰기의 토큰.
pub const TOKEN_ENV: &str = "PITWALL_ASKPASS_TOKEN";
/// ssh 가 도우미에게 주는 질문의 종류(`confirm` · `none`, 없으면 값을 묻는다).
pub const SSH_PROMPT_ENV: &str = "SSH_ASKPASS_PROMPT";
/// 도우미와 앱이 주고받는 첫 줄의 `protocol`.
pub const PROTOCOL: &str = "pitwall-askpass/1";
/// 한 줄(요청 · 답)의 최대 크기.
pub const MAX_LINE: usize = 64 * 1024;
/// 붙은 도우미가 요청 줄을 보내기까지 기다리는 시간.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// 도우미가 답을 기다리는 시간. 앱의 질문 시간 제한([`super::prompt::ASKPASS_TIMEOUT`])보다 길다 — 앱이 멈춰도 도우미는 끝난다.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(180);

/// 도우미 → 앱.
#[cfg(unix)]
#[derive(serde::Serialize, serde::Deserialize)]
struct Request {
    protocol: String,
    token: String,
    prompt: String,
    #[serde(default)]
    hint: AskpassHint,
}

/// 앱 → 도우미. 둘 중 하나만 있다. 값이 비밀이라 `Debug` 를 두지 않는다.
#[cfg(unix)]
#[derive(serde::Serialize, serde::Deserialize)]
struct Reply {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// 앱의 askpass. 복제하면 같은 것을 가리킨다. 데스크톱은 주 인스턴스가 된 뒤 [`Askpass::serve`] 로 소켓을 연다 — 열기 전과 열지 못한
/// 앱의 쓰기는 askpass 없이 돈다.
#[derive(Clone, Default)]
pub struct Askpass {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    serving: Mutex<Option<Serving>>,
    sessions: Mutex<HashMap<String, Entry>>,
}

#[derive(Debug, Clone)]
struct Serving {
    socket: PathBuf,
    helper: PathBuf,
}

#[derive(Clone)]
#[cfg_attr(
    not(unix),
    allow(
        dead_code,
        reason = "only the unix socket reads the entries; Windows has no askpass socket yet (D38)"
    )
)]
struct Entry {
    project: String,
    prompts: Prompts,
    cancel: CancellationToken,
    timeout: Duration,
    /// 지금 창에 묻고 있는 질문 수([`Session::asking`]).
    asking: Arc<AtomicUsize>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Askpass {
    /// 열고 있는 소켓. 열기 전 · 열지 못했으면 `None`.
    pub fn socket(&self) -> Option<PathBuf> {
        lock(&self.inner.serving).as_ref().map(|s| s.socket.clone())
    }

    /// 도우미 스크립트(`GIT_ASKPASS` 의 값). 열기 전 · 열지 못했으면 `None`.
    pub fn helper(&self) -> Option<PathBuf> {
        lock(&self.inner.serving).as_ref().map(|s| s.helper.clone())
    }

    /// 지금 도는 쓰기의 수(토큰 표의 크기).
    pub fn sessions(&self) -> usize {
        lock(&self.inner.sessions).len()
    }

    /// 쓰기(또는 DB 터널의 ssh) 한 번의 askpass. 질문은 `project` 를 듣는 창으로 간다. 소켓을 열지 않았으면 `None`. 버리면 그
    /// 토큰의 질문을 끝낸다.
    pub fn session(&self, project: &str, prompts: &Prompts, timeout: Duration) -> Option<Session> {
        let serving = lock(&self.inner.serving).clone()?;
        let token = new_token();
        let cancel = CancellationToken::new();
        let asking = Arc::new(AtomicUsize::new(0));
        lock(&self.inner.sessions).insert(
            token.clone(),
            Entry {
                project: project.to_owned(),
                prompts: prompts.clone(),
                cancel: cancel.clone(),
                timeout,
                asking: Arc::clone(&asking),
            },
        );
        Some(Session {
            askpass: self.clone(),
            token,
            cancel,
            serving,
            asking,
        })
    }

    #[cfg(unix)]
    fn entry(&self, token: &str) -> Option<Entry> {
        lock(&self.inner.sessions).get(token).cloned()
    }

    /// 요청 하나에 답한다(소켓 연결 하나).
    #[cfg(unix)]
    async fn reply(&self, request: Request) -> Reply {
        let refuse = |error: &str| Reply {
            value: None,
            error: Some(error.to_owned()),
        };
        if request.protocol != PROTOCOL {
            return refuse("unsupported askpass protocol");
        }
        let Some(entry) = self.entry(&request.token) else {
            tracing::warn!(target: "pw_git", "askpass request for a write that is not running; refused");
            return refuse("this askpass request does not belong to a running git command");
        };
        entry.asking.fetch_add(1, Ordering::SeqCst);
        let _asked = Asking(Arc::clone(&entry.asking));
        let asked = entry.prompts.ask(
            &entry.project,
            super::GitPromptRequest::Askpass {
                text: super::askpass_text(&request.prompt),
                hint: request.hint,
            },
            entry.timeout,
        );
        let answer = tokio::select! {
            biased;
            () = entry.cancel.cancelled() => {
                return refuse("the git command ended");
            }
            answer = asked => answer,
        };
        match answer {
            Ok(super::GitPromptAnswer::Askpass { value }) => {
                tracing::debug!(target: "pw_git", project = entry.project, "askpass answered");
                Reply {
                    value: Some(value),
                    error: None,
                }
            }
            // ssh 의 confirm 은 빈 답이나 `yes` 를 「예」 로 본다.
            Ok(super::GitPromptAnswer::Proceed) => Reply {
                value: Some("yes".to_owned()),
                error: None,
            },
            Ok(super::GitPromptAnswer::Cancel) => {
                tracing::debug!(target: "pw_git", project = entry.project, "askpass cancelled in the window");
                refuse("cancelled")
            }
            Err(super::PromptError::NoWindow) => {
                tracing::info!(target: "pw_git", project = entry.project, "askpass: no window of the project is open; failing at once");
                refuse("no Pitwall window of this project is open to answer")
            }
            Err(super::PromptError::WindowLeft) => {
                tracing::info!(target: "pw_git", project = entry.project, "askpass: the project window closed while asking");
                refuse("the Pitwall window of this project closed before answering")
            }
            Err(super::PromptError::TimedOut) => {
                tracing::info!(target: "pw_git", project = entry.project, "askpass: no answer in time");
                refuse("no answer in time")
            }
        }
    }
}

/// 묻는 동안 센다(끝나면 뺀다).
#[cfg(unix)]
struct Asking(Arc<AtomicUsize>);

#[cfg(unix)]
impl Drop for Asking {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 쓰기 한 번의 askpass 표. 버리면 표에서 빠지고, 그 토큰으로 기다리던 질문이 끝난다.
pub struct Session {
    askpass: Askpass,
    token: String,
    cancel: CancellationToken,
    serving: Serving,
    asking: Arc<AtomicUsize>,
}

impl Session {
    /// 지금 이 토큰의 질문이 창에 떠 있는가(사람의 답을 기다린다). DB 터널은 그동안 준비 제한 시간을 세지 않는다.
    pub fn asking(&self) -> bool {
        self.asking.load(Ordering::SeqCst) > 0
    }

    /// 자식 환경에 도우미와 소켓 · 토큰을 넣는다.
    pub fn apply(&self, env: &mut Vec<(OsString, OsString)>) {
        crate::cli::set_askpass(env, &self.serving.helper);
        env.retain(|(name, _)| name != SOCKET_ENV && name != TOKEN_ENV);
        env.push((SOCKET_ENV.into(), self.serving.socket.clone().into()));
        env.push((TOKEN_ENV.into(), self.token.clone().into()));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        lock(&self.askpass.inner.sessions).remove(&self.token);
        self.cancel.cancel();
    }
}

/// 32자 16진수. 프로세스마다 OS 난수로 키를 받는 `RandomState`(SipHash) 두 개의 출력이다 — 다른 프로세스가 미리 맞힐 수 없다.
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (0..2u8)
        .map(|half| {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u8(half);
            hasher.write_u128(nanos);
            hasher.write_u32(std::process::id());
            format!("{:016x}", hasher.finish())
        })
        .collect()
}

/// `pitwall askpass <질문>` 의 본체. 종료 코드: 0 답을 stdout 에 썼다, 1 답이 없다(취소 · 아니오 · 시간 초과 · 앱에 닿지 못함), 2
/// git 이 띄운 것이 아니다(환경 변수가 없다). 로그를 켜지 않고 파일을 쓰지 않는다.
pub fn client_main(args: &[OsString]) -> i32 {
    use std::io::Write;

    let prompt = args
        .first()
        .map(|a| a.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (Some(socket), Some(token)) = (
        std::env::var_os(SOCKET_ENV),
        std::env::var_os(TOKEN_ENV).and_then(|t| t.into_string().ok()),
    ) else {
        eprintln!(
            "pitwall askpass: git runs this helper for Pitwall; {SOCKET_ENV} and {TOKEN_ENV} are not set"
        );
        return 2;
    };
    let hint = hint_of(std::env::var_os(SSH_PROMPT_ENV).as_deref());
    match request(Path::new(&socket), &token, &prompt, hint) {
        Ok(value) => {
            let mut out = std::io::stdout().lock();
            let written = out
                .write_all(value.as_bytes())
                .and_then(|()| out.write_all(b"\n"))
                .and_then(|()| out.flush());
            i32::from(written.is_err())
        }
        Err(e) => {
            eprintln!("pitwall askpass: {e}");
            1
        }
    }
}

/// `SSH_ASKPASS_PROMPT` 의 값으로 질문의 종류를 정한다(모르는 값 · 없음은 값을 묻는 것).
pub(crate) fn hint_of(value: Option<&std::ffi::OsStr>) -> AskpassHint {
    match value.and_then(std::ffi::OsStr::to_str) {
        Some("confirm") => AskpassHint::Confirm,
        Some("none") => AskpassHint::Notice,
        _ => AskpassHint::Secret,
    }
}

/// 앱의 소켓 `socket` 에 질문 하나를 보내고 답을 받는다(도우미 쪽, 블로킹).
#[cfg(unix)]
pub fn request(
    socket: &Path,
    token: &str,
    prompt: &str,
    hint: AskpassHint,
) -> std::io::Result<String> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;

    unix::check_endpoint(socket, unix::current_uid())?;
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    let mut line = serde_json::to_vec(&Request {
        protocol: PROTOCOL.to_owned(),
        token: token.to_owned(),
        prompt: prompt.to_owned(),
        hint,
    })
    .map_err(std::io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reply = Vec::new();
    BufReader::new(stream)
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut reply)?;
    let reply: Reply = serde_json::from_slice(&reply).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the app closed the connection without an answer",
        )
    })?;
    match (reply.value, reply.error) {
        (Some(value), _) => Ok(value),
        (None, error) => Err(std::io::Error::other(
            error.unwrap_or_else(|| "no answer".to_owned()),
        )),
    }
}

#[cfg(not(unix))]
pub fn request(
    _socket: &Path,
    _token: &str,
    _prompt: &str,
    _hint: AskpassHint,
) -> std::io::Result<String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "askpass is not available on this platform yet",
    ))
}

#[cfg(unix)]
impl Askpass {
    /// 소켓 `socket` 을 열고 그 옆에 도우미 스크립트(`pitwall` 실행 파일 `exe` 의 `askpass` 를 부른다)를 둔 뒤 연결을 받는다. 이
    /// future 가 도는 동안 열려 있고, 버리면 소켓 파일을 지우고 닫는다. 열지 못하면 경고를 남기고 끝난다.
    pub async fn serve(self, socket: PathBuf, exe: PathBuf) {
        let uid = unix::current_uid();
        let listener = match unix::bind(&socket, uid) {
            Ok(listener) => listener,
            Err(e) => {
                tracing::warn!(target: "pw_git", %e, socket = %socket.display(), "askpass socket disabled; git will not ask for passwords in the window");
                return;
            }
        };
        let helper = match unix::write_helper(&socket, &exe) {
            Ok(helper) => helper,
            Err(e) => {
                tracing::warn!(target: "pw_git", %e, "cannot write the askpass helper script; askpass disabled");
                return;
            }
        };
        tracing::info!(target: "pw_git", socket = %socket.display(), "askpass socket listening");
        *lock(&self.inner.serving) = Some(Serving { socket, helper });
        let _serving = ClearServing(self.clone());
        loop {
            let stream = match listener.accept().await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::warn!(target: "pw_git", %e, "askpass accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let askpass = self.clone();
            tokio::spawn(async move { askpass.connection(stream, uid).await });
        }
    }

    async fn connection(self, stream: tokio::net::UnixStream, uid: u32) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        if let Err(e) = unix::peer_allowed(stream.peer_cred().map(|c| c.uid()), uid) {
            tracing::warn!(target: "pw_git", %e, "askpass connection refused");
            return;
        }
        let (read, mut write) = stream.into_split();
        let mut line = Vec::new();
        let mut read = BufReader::new(read).take(MAX_LINE as u64);
        let request =
            match tokio::time::timeout(REQUEST_TIMEOUT, read.read_until(b'\n', &mut line)).await {
                Ok(Ok(_)) => serde_json::from_slice::<Request>(&line).ok(),
                Ok(Err(_)) | Err(_) => None,
            };
        // 도우미는 요청 한 줄 뒤에 아무것도 보내지 않고 답을 기다린다 — 읽기 쪽이 끝나면(EOF) 도우미가 끝난 것이다. 그때는 묻던
        // 질문을 버려(창의 다이얼로그를 닫고) 답하지 않는다.
        let mut read = read.into_inner();
        let helper_gone = async {
            let mut byte = [0u8; 1];
            while matches!(read.read(&mut byte).await, Ok(n) if n > 0) {}
        };
        let reply = match request {
            Some(request) => tokio::select! {
                reply = self.reply(request) => reply,
                () = helper_gone => return,
            },
            None => Reply {
                value: None,
                error: Some("malformed askpass request".to_owned()),
            },
        };
        let Ok(mut bytes) = serde_json::to_vec(&reply) else {
            return;
        };
        bytes.push(b'\n');
        let _ = write.write_all(&bytes).await;
        let _ = write.shutdown().await;
    }
}

#[cfg(not(unix))]
impl Askpass {
    /// 이 플랫폼에는 아직 소켓이 없다 — 열지 않고 끝난다.
    pub async fn serve(self, _socket: PathBuf, _exe: PathBuf) {
        tracing::info!(target: "pw_git", "askpass is not available on this platform; git will not ask for passwords in the window");
    }
}

/// [`Askpass::serve`] 가 끝나면(버려지면) 도우미를 더 주지 않는다.
#[cfg(unix)]
struct ClearServing(Askpass);

#[cfg(unix)]
impl Drop for ClearServing {
    fn drop(&mut self) {
        *lock(&self.0.inner.serving) = None;
    }
}

/// unix 소켓 · 도우미 파일. `pw-mcp` 의 MCP 소켓과 같은 규칙이다(그 사용자 전용 폴더 · 0600 소켓 · 상대 uid).
#[cfg(unix)]
pub mod unix {
    use std::io;
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    use std::path::{Path, PathBuf};

    use tokio::net::{UnixListener, UnixStream};

    pub fn current_uid() -> u32 {
        // SAFETY: geteuid 는 실패하지 않고 상태를 바꾸지 않는다.
        unsafe { libc::geteuid() }
    }

    fn insecure(path: &Path, why: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} {why}", path.display()),
        )
    }

    /// 폴더를 만들거나 고쳐 이 사용자만 들어오게 한다(앱 쪽).
    fn prepare_dir(dir: &Path, uid: u32) -> io::Result<()> {
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

    /// 붙기 전에 폴더와 소켓 파일을 본다(도우미 쪽). 고치지 않는다.
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

    /// 상대의 uid 가 이 사용자인가. 읽지 못한 연결도 거절한다.
    pub fn peer_allowed(peer: io::Result<u32>, uid: u32) -> io::Result<()> {
        match peer {
            Ok(peer) if peer == uid => Ok(()),
            Ok(peer) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("the peer is another user (uid {peer})"),
            )),
            Err(e) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("cannot read the peer credentials: {e}"),
            )),
        }
    }

    /// 열린 소켓. 버리면 파일을 지운다(그사이 다른 앱이 같은 이름으로 다시 열었으면 두고).
    pub struct Listener {
        inner: UnixListener,
        path: PathBuf,
        ino: u64,
    }

    impl Listener {
        pub async fn accept(&self) -> io::Result<UnixStream> {
            self.inner.accept().await.map(|(stream, _)| stream)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.ino) {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    /// 소켓을 연다. 받는 앱이 있는 소켓이면 열지 않는다(다른 앱이 쓰는 중). 받지 않는 소켓(죽은 앱이 남긴 것)은 지우고 연다. 소켓이
    /// 아닌 파일 · 심링크는 지우지 않고 오류로 둔다.
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
                std::fs::remove_file(path)?;
            }
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
        })
    }

    /// 소켓 옆(`<소켓 이름>.askpass`, 0700)에 `exec '<exe>' askpass "$@"` 스크립트를 쓴다. git · ssh 는 askpass 를 인자 없이 실행
    /// 파일 하나로 부르므로 서브커맨드를 붙일 자리가 이것이다. 임시 이름에 다 쓴 뒤 자리를 잡는다.
    pub fn write_helper(socket: &Path, exe: &Path) -> io::Result<PathBuf> {
        use std::io::Write;

        let helper = socket.with_extension("askpass");
        let partial = socket.with_extension("askpass.partial");
        let quoted = exe.to_string_lossy().replace('\'', r"'\''");
        let _ = std::fs::remove_file(&partial);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&partial)?;
        file.write_all(format!("#!/bin/sh\nexec '{quoted}' askpass \"$@\"\n").as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&partial, &helper)?;
        Ok(helper)
    }
}
