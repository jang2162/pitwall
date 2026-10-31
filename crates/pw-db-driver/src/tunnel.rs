//! SSH 터널(14-m3-plan.md M3-11, 결정 M3-D11): 시스템 `ssh -N -L`.
//!
//! - **ssh 는 사람의 것을 그대로**: 실행 파일 · 환경(PATH · `SSH_AUTH_SOCK` 등)은 부르는 쪽([`Launch`])이 그 프로젝트의 런타임
//!   환경에서 준다. 사람의 `~/.ssh/config` · 에이전트 · ProxyJump · known_hosts 를 ssh 가 그대로 따른다.
//! - **argv 는 Pitwall 이 고정한다**([`ssh_args`]): `-N -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 -o ControlPath=none
//!   -L 127.0.0.1:<로컬 포트>:<DB 호스트>:<DB 포트> -- <목적지>`. 설정에서 오는 값은 목적지와 DB 호스트 · 포트뿐이고, 허용
//!   글자 목록([`check_destination`] · [`check_db_host`])을 벗어나면 띄우기 전에 거절한다(옵션 주입과, 사람의 `ProxyCommand …%h`
//!   를 거쳐 셸 메타문자가 명령이 되는 것을 막는다 — 설정 단계의 검사(`pw_db::config`)와 겹방어). `ControlPath=none` 은 사람의 ControlMaster 를 쓰지도 만들지도 않게 한다 — 마스터가 있으면
//!   ssh 가 포워드를 마스터에 맡기고 곧바로 끝나, 포워드가 Pitwall 이 끌 수 없는 마스터에 남는다(재현 — 14 M3-11 칸).
//! - **로컬 포트**: `127.0.0.1:0` 바인드로 고른 뒤 넘긴다. 그 사이 다른 프로세스가 가져가면 ssh 가 `ExitOnForwardFailure` 로 끝나고,
//!   다른 포트로 한 번 더 띄운다.
//! - **준비됨**: 로컬 포트에 TCP 가 붙고, 그 LISTEN 소켓이 **띄운 ssh 프로세스의 것**일 때다([`listener_owned_by`] — macOS
//!   `lsof`, Linux `/proc`, Windows `GetExtendedTcpTable`). ssh 는 인증이 끝난 뒤에야 `-L` 을 바인드하므로 그 사이 남이 포트를
//!   잡으면 TCP 는 붙는다 — 그것을 준비됨으로 보면 드라이버가 비밀번호를 남의 리스너로 보낸다. 제한 시간 [`READY_TIMEOUT`] 은
//!   askpass 질문이 열려 있는 동안 멈춘다(사람이 비밀번호를 치는 시간).
//! - **수명**: 터널은 [`TunnelKey`](프로젝트, 목적지, DB 호스트, 포트)마다 하나이고 그것을 쓰는 연결이 [`TunnelLease`] 로 함께 쓴다.
//!   마지막 리스가 떨어지면 ssh 의 프로세스 그룹째 끝낸다(SIGTERM, [`STOP_GRACE`] 뒤 SIGKILL). ssh 는 `setsid` 로 자기 세션 ·
//!   그룹을 갖는다 — 그 그룹만 끝낸다. ssh 가 스스로 죽으면 [`TunnelLease::lost`] 가 깨어나고(연결을 「끊김」 으로 알리는 것은
//!   부르는 쪽 — `pw_db::session`), 다시 붙는 것은 사람이 누를 때다(다음 [`Tunnels::open`] 이 새로 띄운다).
//! - **앱이 끝날 때**: [`shutdown`] 이 남은 ssh 그룹을 모두 끝낸다(데스크톱의 `RunEvent::Exit`). **강제로 끝나면**(SIGKILL ·
//!   크래시) `shutdown` 이 돌지 않는다 — 띄운 ssh 마다 상태 디렉터리에 기록을 두고([`Tunnels::recording`]) 다음 시작 때
//!   [`reap_orphans`] 가 (pid, 시작 시각, argv)가 모두 맞는 것만 끈다. Linux 는 ssh 에 `PR_SET_PDEATHSIG` 도 건다.
//! - askpass(비밀번호 · 패스프레이즈 · 모르는 호스트 키)는 부르는 쪽이 [`AskpassGuard`] 로 환경을 준다(M2-06 의 길 — `pw_git`).
//!   없으면 ssh 는 묻지 못하고 실패한다(제어 터미널이 없다).

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use pw_db_types::{DbError, DbErrorKind, SocketTarget};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::DbResult;

/// 「준비됨」 까지의 제한 시간(askpass 질문이 열린 시간은 세지 않는다).
pub const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// 끝낼 때 SIGTERM 뒤 SIGKILL 까지 기다리는 시간.
pub const STOP_GRACE: Duration = Duration::from_millis(300);
/// 준비 확인 간격.
const PROBE_EVERY: Duration = Duration::from_millis(50);
/// 오류 글에 싣는 ssh stderr 의 끝부분(바이트).
const STDERR_LIMIT: usize = 4096;
/// 로컬 끝은 늘 루프백이다.
pub const LOOPBACK: &str = "127.0.0.1";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 호스트 이름 · 사용자 이름에 받는 글자: `[A-Za-z0-9._-]` 이고 `-` 로 시작하지 않는다.
///
/// 허용 목록인 까닭: OpenSSH 9.6 미만(예: Windows 10 내장 8.1p1)은 목적지의 셸 메타문자를 거르지 않는다(CVE-2023-51385) — 사람의
/// 설정에 `ProxyCommand … %h` · `Match exec "… %h"` 가 있으면 커밋된 `host_ref` 의 `$(…)` · `;` · 백틱이 셸 명령이 된다. 금지
/// 목록(`-` · 공백 · 제어 문자)으로는 그 글자들을 다 셀 수 없다.
fn name_chars(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// IPv6 주소 글자: `[0-9A-Fa-f:.%]` 이고 `:` 를 담는다.
fn ipv6_chars(value: &str) -> bool {
    value.contains(':')
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.' | b'%'))
}

/// 터널 목적지(`ssh_tunnel.host_ref`, 결정 M3-D11)의 검사. 받는 꼴은 둘이다:
///
/// - `~/.ssh/config` 의 호스트 이름(별칭): [`name_chars`]. `host:port` · `user@host` 꼴은 받지 않는다 — ssh 는 `host:port` 를
///   호스트 이름으로 읽는다.
/// - `ssh://[user@]host[:port]`: 사용자 · 호스트는 [`name_chars`], 호스트가 `[…]` 이면 그 안은 [`ipv6_chars`], 포트는 1 ~ 65535.
///
/// 거절하면 까닭(값은 담지 않는다)을 돌려준다.
pub fn check_destination(host_ref: &str) -> Result<(), &'static str> {
    let Some(rest) = host_ref.strip_prefix("ssh://") else {
        return if name_chars(host_ref) {
            Ok(())
        } else {
            Err(
                "must be a host name from ~/.ssh/config (letters, digits, `.`, `_`, `-`) or `ssh://[user@]host[:port]`",
            )
        };
    };
    let uri = "is not `ssh://[user@]host[:port]` (user and host take letters, digits, `.`, `_`, `-`; an IPv6 host goes in `[…]`)";
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    let host_port = match authority.split_once('@') {
        Some((user, host_port)) if name_chars(user) => host_port,
        Some(_) => return Err(uri),
        None => authority,
    };
    let (host_ok, port) = match host_port.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((host, "")) => (ipv6_chars(host), None),
            Some((host, rest)) => match rest.strip_prefix(':') {
                Some(port) => (ipv6_chars(host), Some(port)),
                None => (false, None),
            },
            None => (false, None),
        },
        None => match host_port.split_once(':') {
            Some((host, port)) => (name_chars(host), Some(port)),
            None => (name_chars(host_port), None),
        },
    };
    let port_ok = port.is_none_or(|p| {
        !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u16>().is_ok_and(|p| p > 0)
    });
    if host_ok && port_ok { Ok(()) } else { Err(uri) }
}

/// `-L` 로 가는 DB 호스트의 검사: 호스트 이름([`name_chars`]) 또는 IPv6 주소(대괄호는 있어도 없어도 — [`ssh_args`] 가 감싼다).
pub fn check_db_host(host: &str) -> Result<(), &'static str> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if name_chars(host) || ipv6_chars(bare) {
        Ok(())
    } else {
        Err("must be a host name (letters, digits, `.`, `_`, `-`) or an IPv6 address")
    }
}

/// ssh 의 인자(실행 파일 뒤). 머리 주석의 고정 argv.
pub fn ssh_args(
    local_port: u16,
    db_host: &str,
    db_port: u16,
    destination: &str,
) -> DbResult<Vec<OsString>> {
    let refuse = |what: &str, why: &str| {
        DbError::new(DbErrorKind::Invalid, format!("ssh tunnel {what} {why}"))
    };
    check_destination(destination).map_err(|why| refuse("destination", why))?;
    check_db_host(db_host).map_err(|why| refuse("db host", why))?;
    if db_port == 0 {
        return Err(DbError::new(
            DbErrorKind::Invalid,
            "ssh tunnel db port must not be 0",
        ));
    }
    // `-L` 의 IPv6 주소는 대괄호로 감싼다.
    let host = if db_host.contains(':') && !db_host.starts_with('[') {
        format!("[{db_host}]")
    } else {
        db_host.to_owned()
    };
    Ok([
        "-N",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "ControlPath=none",
        "-L",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([
        OsString::from(format!("{LOOPBACK}:{local_port}:{host}:{db_port}")),
        OsString::from("--"),
        OsString::from(destination),
    ])
    .collect())
}

/// 터널 하나의 키. 같은 키의 연결은 터널 하나를 함께 쓴다.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TunnelKey {
    pub project: String,
    /// `ssh_tunnel.host_ref` 그대로(`~/.ssh/config` 의 호스트 이름 또는 `ssh://[user@]host[:port]`).
    pub destination: String,
    pub db_host: String,
    pub db_port: u16,
}

/// ssh 에 줄 askpass(M2-06). 환경(`SSH_ASKPASS` 등)과 「지금 사람에게 묻는 중인가」, 그리고 터널이 준비될 때까지 쥐고 있을 것(그
/// 토큰 표의 항목 등 — 버리면 묻기가 끝난다).
pub struct AskpassGuard {
    env: Vec<(OsString, OsString)>,
    asking: Box<dyn Fn() -> bool + Send + Sync>,
    _keep: Box<dyn std::any::Any + Send + Sync>,
}

impl AskpassGuard {
    pub fn new(
        env: Vec<(OsString, OsString)>,
        asking: impl Fn() -> bool + Send + Sync + 'static,
        keep: impl std::any::Any + Send + Sync,
    ) -> Self {
        Self {
            env,
            asking: Box::new(asking),
            _keep: Box::new(keep),
        }
    }

    fn asking(&self) -> bool {
        (self.asking)()
    }
}

/// ssh 를 띄울 것: 실행 파일(런타임 환경의 PATH 에서 찾은 `ssh`), 자식 환경 전체(앱 프로세스의 환경은 물려주지 않는다), askpass.
pub struct Launch {
    pub program: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    pub askpass: Option<AskpassGuard>,
}

/// 터널을 쓰는 동안 쥐는 것. 마지막 리스를 버리면 ssh 가 끝난다.
#[derive(Clone)]
pub struct TunnelLease {
    live: Arc<Live>,
}

impl std::fmt::Debug for TunnelLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelLease")
            .field("pid", &self.live.pid)
            .field("port", &self.live.port)
            .finish()
    }
}

impl TunnelLease {
    /// 드라이버가 붙을 주소(`127.0.0.1:<로컬 포트>`).
    pub fn local(&self) -> SocketTarget {
        SocketTarget {
            host: LOOPBACK.to_owned(),
            port: self.live.port,
        }
    }

    /// ssh 의 pid(프로세스 그룹 id 와 같다 — `setsid`).
    pub fn pid(&self) -> u32 {
        self.live.pid
    }

    /// ssh 가 끝나면(스스로 죽음 · 마지막 리스를 버림 · [`shutdown`]) 깨어난다. 돌려준 토큰을 취소해도 터널은 끝나지 않는다.
    pub fn lost(&self) -> CancellationToken {
        self.live.lost.child_token()
    }
}

/// 떠 있는 ssh 하나. 버리면(마지막 리스) 감시 태스크가 그룹을 끝낸다.
struct Live {
    pid: u32,
    port: u16,
    lost: CancellationToken,
    stop: CancellationToken,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

type Slot = Arc<tokio::sync::Mutex<Weak<Live>>>;
type PortPicker = Arc<dyn Fn() -> std::io::Result<u16> + Send + Sync>;

/// 앱의 터널들. 복제하면 같은 표를 가리킨다.
#[derive(Clone)]
pub struct Tunnels {
    inner: Arc<Inner>,
}

struct Inner {
    slots: Mutex<HashMap<TunnelKey, Slot>>,
    ready_timeout: Duration,
    pick_port: PortPicker,
    /// 띄운 ssh 의 기록을 둘 폴더([`orphans`]). 없으면 적지 않는다.
    records: Option<PathBuf>,
}

impl Default for Tunnels {
    fn default() -> Self {
        Self::new()
    }
}

impl Tunnels {
    pub fn new() -> Self {
        Self::with_options(READY_TIMEOUT, Arc::new(free_port), None)
    }

    /// 띄운 ssh 마다 `dir` 에 기록을 둔다 — 앱이 강제로 끝나 남은 ssh 를 다음 시작 때 [`reap_orphans`] 가 끈다. 데스크톱은 이것을
    /// 쓴다(`dir` 은 상태 디렉터리 아래).
    pub fn recording(dir: PathBuf) -> Self {
        Self::with_options(READY_TIMEOUT, Arc::new(free_port), Some(dir))
    }

    fn with_options(
        ready_timeout: Duration,
        pick_port: PortPicker,
        records: Option<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: Mutex::default(),
                ready_timeout,
                pick_port,
                records,
            }),
        }
    }

    /// 준비 제한 시간을 바꾼다(시험).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_ready_timeout(ready_timeout: Duration) -> Self {
        Self::with_options(ready_timeout, Arc::new(free_port), None)
    }

    /// 로컬 포트 고르기를 바꾼다(시험 — 경합 재현).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_port_picker(
        pick_port: impl Fn() -> std::io::Result<u16> + Send + Sync + 'static,
    ) -> Self {
        Self::with_options(READY_TIMEOUT, Arc::new(pick_port), None)
    }

    /// 지금 떠 있는 터널 수(시험).
    pub fn live_count(&self) -> usize {
        lock(&self.inner.slots)
            .values()
            .filter(|slot| {
                slot.try_lock().map_or(true, |live| {
                    live.upgrade().is_some_and(|l| !l.lost.is_cancelled())
                })
            })
            .count()
    }

    /// 지금 떠 있는 ssh 의 pid(시험 · 진단).
    pub fn pids(&self) -> Vec<u32> {
        lock(&self.inner.slots)
            .values()
            .filter_map(|slot| {
                slot.try_lock()
                    .ok()
                    .and_then(|live| live.upgrade())
                    .filter(|l| !l.lost.is_cancelled())
                    .map(|l| l.pid)
            })
            .collect()
    }

    /// `key` 의 터널을 쓴다. 떠 있으면 함께 쓰고, 없거나 끝났으면 `launch` 로 새로 띄워 준비될 때까지 기다린다. 이 future 를
    /// 버리면 띄우던 ssh 는 끝낸다.
    pub async fn open<F, Fut>(&self, key: TunnelKey, launch: F) -> DbResult<TunnelLease>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = DbResult<Launch>>,
    {
        let slot = {
            let mut slots = lock(&self.inner.slots);
            // 끝난 터널의 자리는 치운다(쓰는 중이거나 띄우는 중인 자리는 둔다).
            slots.retain(|_, slot| {
                Arc::strong_count(slot) > 1
                    || slot.try_lock().map_or(true, |live| live.strong_count() > 0)
            });
            Arc::clone(slots.entry(key.clone()).or_default())
        };
        let mut current = slot.lock().await;
        if let Some(live) = current.upgrade()
            && !live.lost.is_cancelled()
        {
            return Ok(TunnelLease { live });
        }
        let launch = launch().await?;
        let live = start(&key, launch, &self.inner).await?;
        *current = Arc::downgrade(&live);
        Ok(TunnelLease { live })
    }
}

/// 빈 로컬 포트(`127.0.0.1:0` 바인드로 고르고 놓는다).
fn free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind((LOOPBACK, 0))?;
    Ok(listener.local_addr()?.port())
}

/// 한 번 띄운 결과.
enum Attempt {
    Ready(Arc<Live>),
    /// 남이 로컬 포트를 잡았다 — 다른 포트로 다시.
    PortTaken,
    Failed(DbError),
}

async fn start(key: &TunnelKey, launch: Launch, inner: &Inner) -> DbResult<Arc<Live>> {
    for attempt in 0..2 {
        let port = (inner.pick_port)().map_err(|e| {
            DbError::new(
                DbErrorKind::Connect,
                format!("cannot pick a local port for the ssh tunnel: {e}"),
            )
        })?;
        match spawn_until_ready(
            key,
            &launch,
            port,
            inner.ready_timeout,
            inner.records.as_deref(),
        )
        .await
        {
            Attempt::Ready(live) => return Ok(live),
            Attempt::PortTaken if attempt == 0 => {
                tracing::info!(target: "pw_db", port, "another process took the tunnel's local port; trying another port");
            }
            Attempt::PortTaken => {
                return Err(DbError::new(
                    DbErrorKind::Connect,
                    format!(
                        "another process took the ssh tunnel's local port twice (last {LOOPBACK}:{port})"
                    ),
                ));
            }
            Attempt::Failed(e) => return Err(e),
        }
    }
    unreachable!("the loop returns on the second attempt")
}

/// 띄운 ssh 의 프로세스 그룹. 버리면(준비 전에 실패 · 취소) 그룹째 SIGKILL — 자식을 거두기 전에 보내므로 그룹 id 가 다른
/// 프로세스의 것이 되지 않는다(아래 선언 순서: 이것이 `Child` 보다 먼저 버려진다).
struct Group {
    pid: u32,
    armed: bool,
    /// 강제 종료 뒤 정리용 기록(버리면 지운다 — 신호를 보낸 뒤다). 준비되면 감시 태스크로 옮긴다.
    record: Option<orphans::Record>,
}

impl Drop for Group {
    fn drop(&mut self) {
        if self.armed {
            signal_group(self.pid, Signal::Kill);
            forget_group(self.pid);
        }
    }
}

async fn spawn_until_ready(
    key: &TunnelKey,
    launch: &Launch,
    port: u16,
    ready_timeout: Duration,
    records: Option<&std::path::Path>,
) -> Attempt {
    let args = match ssh_args(port, &key.db_host, key.db_port, &key.destination) {
        Ok(args) => args,
        Err(e) => return Attempt::Failed(e),
    };
    let mut command = Command::new(&launch.program);
    command
        .args(&args)
        .env_clear()
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(askpass) = &launch.askpass {
        command.envs(askpass.env.iter().map(|(k, v)| (k, v)));
    }
    #[cfg(unix)]
    {
        #[cfg(target_os = "linux")]
        let parent = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
        // SAFETY: fork 와 exec 사이에서 async-signal-safe 인 setsid(2) · prctl(2) · getppid(2) 만 부른다.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // Linux: 띄운 스레드가 끝나면(앱이 죽으면 — tokio 의 일꾼 스레드는 런타임과 함께 끝난다) ssh 에 SIGTERM. 이 줄 앞에서
                // 앱이 이미 죽었으면 부모가 바뀌었다 — 띄우지 않는다.
                #[cfg(target_os = "linux")]
                {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        return Err(std::io::Error::other(
                            "the app ended while ssh was starting",
                        ));
                    }
                }
                Ok(())
            });
        }
    }
    let mut child: Child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Attempt::Failed(DbError::new(
                DbErrorKind::Connect,
                format!("cannot start ssh ({}): {e}", launch.program.display()),
            ));
        }
    };
    let Some(pid) = child.id() else {
        return Attempt::Failed(DbError::new(DbErrorKind::Connect, "ssh exited at once"));
    };
    remember_group(pid);
    let record = records.and_then(|dir| orphans::Record::write(dir, pid, &args));
    let mut group = Group {
        pid,
        armed: true,
        record,
    };
    tracing::info!(target: "pw_db", pid, port, project = %key.project, destination = %key.destination, "ssh tunnel starting");
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let reader = child.stderr.take().map(|mut pipe| {
        let stderr = Arc::clone(&stderr);
        tokio::spawn(async move {
            let mut chunk = [0u8; 1024];
            while let Ok(n) = pipe.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                let mut buf = lock(&stderr);
                buf.extend_from_slice(&chunk[..n]);
                let over = buf.len().saturating_sub(STDERR_LIMIT);
                buf.drain(..over);
            }
        })
    });

    let mut deadline = Instant::now() + ready_timeout;
    // 남의 리스너를 봤는가. 보고도 계속 찔러 소유 확인만으로 판정한다 — 남이 와일드카드(`0.0.0.0:<포트>`)를 잡았으면 macOS 는 ssh 가
    // 더 좁은 `127.0.0.1:<포트>` 를 바인드하고 그 주소로 오는 연결은 ssh 가 받는다(ssh 는 끝나지 않는다). 루프백을 남이 잡았으면 ssh
    // 가 바인드하지 못하고 끝나며, 그때 이것이 「포트를 뺏겼다」 의 근거가 된다.
    let mut foreign = false;
    let mut last = Instant::now();
    let outcome = loop {
        tokio::select! {
            status = child.wait() => {
                if let Some(reader) = reader {
                    let _ = tokio::time::timeout(Duration::from_millis(200), reader).await;
                }
                let text = String::from_utf8_lossy(&lock(&stderr)).into_owned();
                let taken = foreign
                    || ["Address already in use", "cannot listen to port", "Could not request local forwarding"]
                        .iter()
                        .any(|m| text.contains(m));
                forget_group(pid);
                group.armed = false;
                tracing::info!(target: "pw_db", pid, port, ?status, taken, "ssh exited before the tunnel was ready");
                if taken {
                    return Attempt::PortTaken;
                }
                return Attempt::Failed(exit_error(&text, status.ok()));
            }
            () = tokio::time::sleep(PROBE_EVERY) => {}
        }
        let now = Instant::now();
        if launch.askpass.as_ref().is_some_and(AskpassGuard::asking) {
            // 사람에게 묻는 동안은 제한 시간을 세지 않는다.
            deadline += now - last;
        }
        last = now;
        if now >= deadline {
            break Err(DbError::new(
                DbErrorKind::Timeout,
                format!(
                    "the ssh tunnel to `{}` was not ready within {} s",
                    key.destination,
                    ready_timeout.as_secs()
                ),
            ));
        }
        let connected = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::net::TcpStream::connect((LOOPBACK, port)),
        )
        .await;
        if !matches!(connected, Ok(Ok(_))) {
            continue;
        }
        match listener_owned_by(pid, port).await {
            Owner::Ours => break Ok(()),
            Owner::Other => {
                if !foreign {
                    tracing::warn!(target: "pw_db", pid, port, "another process listens on the tunnel's local port; waiting until ssh listens or gives up");
                }
                foreign = true;
            }
        }
    };
    if let Err(e) = outcome {
        tracing::info!(target: "pw_db", pid, port, "ssh tunnel was not ready; ending its process group");
        return Attempt::Failed(e);
    }
    group.armed = false;
    let live = Arc::new(Live {
        pid,
        port,
        lost: CancellationToken::new(),
        stop: CancellationToken::new(),
    });
    tokio::spawn(watch(
        child,
        pid,
        live.stop.clone(),
        live.lost.clone(),
        group.record.take(),
    ));
    tracing::info!(target: "pw_db", pid, port, project = %key.project, destination = %key.destination, "ssh tunnel ready");
    Attempt::Ready(live)
}

/// ssh 가 끝난 이유를 오류로. 문장은 ssh 의 stderr(끝부분) — 제어 문자는 뺀다.
fn exit_error(stderr: &str, status: Option<std::process::ExitStatus>) -> DbError {
    let text: String = stderr
        .trim()
        .chars()
        .map(|c| {
            if c == '\n' {
                '\n'
            } else if c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let kind =
        if text.contains("Permission denied") || text.contains("Host key verification failed") {
            DbErrorKind::Auth
        } else {
            DbErrorKind::Connect
        };
    let status = status.map_or_else(|| "unknown status".to_owned(), |s| s.to_string());
    let message = if text.is_empty() {
        format!("ssh exited before the tunnel was ready ({status})")
    } else {
        format!("ssh exited before the tunnel was ready ({status}): {text}")
    };
    DbError::new(kind, message)
}

/// 준비된 ssh 를 지켜본다: 스스로 끝나면 `lost`, 마지막 리스가 떨어지면(`stop`) 그룹째 끝내고 거둔다.
async fn watch(
    mut child: Child,
    pid: u32,
    stop: CancellationToken,
    lost: CancellationToken,
    record: Option<orphans::Record>,
) {
    tokio::select! {
        status = child.wait() => {
            tracing::info!(target: "pw_db", pid, ?status, "ssh tunnel process ended by itself");
        }
        () = stop.cancelled() => {
            signal_group(pid, Signal::Term);
            if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_err() {
                signal_group(pid, Signal::Kill);
                let _ = child.wait().await;
            }
            tracing::info!(target: "pw_db", pid, "ssh tunnel closed (no connection uses it)");
        }
    }
    forget_group(pid);
    drop(record);
    lost.cancel();
}

// --- 프로세스 그룹 ---------------------------------------------------------------------------------------------

/// 떠 있는 ssh 그룹(앱 종료 정리용).
static GROUPS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

fn remember_group(pid: u32) {
    lock(&GROUPS).insert(pid);
}

fn forget_group(pid: u32) {
    lock(&GROUPS).remove(&pid);
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

/// ssh 의 프로세스 그룹(`setsid` — pid 가 그룹 id)에 신호를 보낸다. 부르는 쪽이 아직 거두지 않은 자식에게만 보낸다.
#[cfg(unix)]
fn signal_group(pid: u32, signal: Signal) {
    let signal = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: killpg(2) 는 신호를 보낼 뿐이다.
        unsafe {
            libc::killpg(pid, signal);
        }
    }
}

/// Windows 는 그룹이 없다 — ssh 하나를 끝낸다(`kill_on_drop` 과 감시 태스크가 거둔다).
#[cfg(not(unix))]
fn signal_group(pid: u32, _signal: Signal) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// 앱이 끝날 때: 남은 ssh 그룹을 모두 끝낸다(SIGTERM, [`STOP_GRACE`] 안에 남은 것은 SIGKILL). 거두는 것은 감시 태스크 · 프로세스
/// 종료가 한다.
pub fn shutdown() {
    let groups: Vec<u32> = lock(&GROUPS).iter().copied().collect();
    if groups.is_empty() {
        return;
    }
    tracing::info!(target: "pw_db", ?groups, "ending ssh tunnels before the app exits");
    for &pid in &groups {
        signal_group(pid, Signal::Term);
    }
    let deadline = std::time::Instant::now() + STOP_GRACE;
    while std::time::Instant::now() < deadline && groups.iter().any(|&pid| alive(pid)) {
        std::thread::sleep(Duration::from_millis(10));
    }
    for &pid in &groups {
        if alive(pid) {
            signal_group(pid, Signal::Kill);
        }
    }
}

/// 프로세스가 있는가(거두지 않은 좀비도 있다고 본다).
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    libc::pid_t::try_from(pid).is_ok_and(|pid| {
        // SAFETY: kill(2) 에 신호 0 은 확인만 한다.
        unsafe { libc::kill(pid, 0) == 0 }
    })
}

#[cfg(not(unix))]
pub fn alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
}

// --- 리스너의 주인 ------------------------------------------------------------------------------------------------

/// 로컬 포트의 LISTEN 소켓이 누구 것인가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Ours,
    Other,
}

/// `127.0.0.1:<port>` 의 LISTEN 소켓이 프로세스 `pid` 의 것인가. 확인할 수 없으면(도구가 없음 등) 경고를 남기고 `Ours` 로 본다 —
/// 그때의 판정은 14 의 처음 정의(TCP 가 붙는 것)와 같다.
pub async fn listener_owned_by(pid: u32, port: u16) -> Owner {
    let checked = tokio::task::spawn_blocking(move || owner_of(pid, port))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match checked {
        Ok(true) => Owner::Ours,
        Ok(false) => Owner::Other,
        Err(e) => {
            tracing::warn!(target: "pw_db", pid, port, %e, "cannot tell who listens on the tunnel's local port; trusting the TCP connect");
            Owner::Ours
        }
    }
}

#[cfg(target_os = "macos")]
fn owner_of(pid: u32, port: u16) -> Result<bool, String> {
    // `-a`: 조건을 모두(그 pid 의, 그 주소의, LISTEN). 맞는 것이 없으면 종료 코드 1.
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &pid.to_string(),
            &format!("-iTCP@{LOOPBACK}:{port}"),
            "-sTCP:LISTEN",
            "-t",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("lsof: {e}"))?;
    Ok(output.status.success() && !output.stdout.trim_ascii().is_empty())
}

#[cfg(target_os = "linux")]
fn owner_of(pid: u32, port: u16) -> Result<bool, String> {
    // `/proc/net/tcp` 의 `0100007F:<포트 16진수>` · 상태 `0A`(LISTEN) 줄의 inode 가 그 프로세스의 fd 가 가리키는 소켓인가.
    let table =
        std::fs::read_to_string("/proc/net/tcp").map_err(|e| format!("/proc/net/tcp: {e}"))?;
    let local = format!("0100007F:{port:04X}");
    let inodes: Vec<String> = table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            (fields.get(1) == Some(&local.as_str()) && fields.get(3) == Some(&"0A"))
                .then(|| fields.get(9).map(|s| (*s).to_owned()))
                .flatten()
        })
        .collect();
    if inodes.is_empty() {
        return Ok(false);
    }
    let fds =
        std::fs::read_dir(format!("/proc/{pid}/fd")).map_err(|e| format!("/proc/{pid}/fd: {e}"))?;
    Ok(fds.flatten().any(|fd| {
        std::fs::read_link(fd.path()).is_ok_and(|target| {
            inodes
                .iter()
                .any(|inode| target.as_os_str() == format!("socket:[{inode}]").as_str())
        })
    }))
}

#[cfg(windows)]
fn owner_of(pid: u32, port: u16) -> Result<bool, String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let mut size: u32 = 0;
    // SAFETY: 크기만 묻는다(버퍼 없음).
    unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            u32::from(AF_INET),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
    }
    let mut buf = vec![0u8; size as usize + 1024];
    let mut size = buf.len() as u32;
    // SAFETY: `buf` 의 크기를 `size` 로 넘긴다.
    let status = unsafe {
        GetExtendedTcpTable(
            buf.as_mut_ptr().cast(),
            &mut size,
            0,
            u32::from(AF_INET),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if status != 0 {
        return Err(format!("GetExtendedTcpTable failed ({status})"));
    }
    // SAFETY: 성공한 호출이 버퍼 앞에 MIB_TCPTABLE_OWNER_PID 를 채웠다. 행은 그 뒤에 dwNumEntries 개다.
    let rows = unsafe {
        let table = &*buf.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
        std::slice::from_raw_parts(
            table.table.as_ptr().cast::<MIB_TCPROW_OWNER_PID>(),
            table.dwNumEntries as usize,
        )
    };
    let loopback = u32::from_ne_bytes([127, 0, 0, 1]);
    Ok(rows.iter().any(|row| {
        // 포트는 네트워크 순서로 아래 16비트에 있다.
        let row_port = u16::from_be(row.dwLocalPort as u16);
        row_port == port && row.dwLocalAddr == loopback && row.dwOwningPid == pid
    }))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn owner_of(_pid: u32, _port: u16) -> Result<bool, String> {
    Err("no listener owner check on this platform".to_owned())
}

mod orphans;
pub use orphans::reap_orphans;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

#[cfg(test)]
mod tests;
