//! 데몬의 받기(18-m7-plan M7-04 ①): 소켓에서 연결을 받아 첫 틀로 링크와 좁은 명령을 가른다.
//!
//! - **증명 전 연결 수 상한**([`DaemonParts::max_unproven`]): 연결은 받는 순간부터 링크가 증명될 때까지(또는 좁은 명령이 끝날 때까지)
//!   자리 하나를 쥔다. 자리가 없으면 그 연결은 곧바로 닫는다 — 증명하지 않는 연결로 데몬을 채우지 못하게. 네트워크에 열리는 전송
//!   (QUIC — M7-24) 전에 반드시 있어야 하는 것이다.
//! - **첫 틀 · 핸드셰이크 틀 상한** `pw_link::HANDSHAKE_MAX_FRAME`(64 KiB). 증명 뒤에만 링크 틀 상한으로 오른다(`pw_link`).
//! - 첫 틀은 [`FIRST_FRAME_TIMEOUT`] 안에 와야 한다.
//! - 증명된 링크는 [`Presence`] 에 센다 — 유휴 판정과 「붙은 owner 링크가 있는가」(M7-20 의 M7-D10 ④ 판정 — `crate::mcp::services`)가 본다.
//! - 열린 MCP 연결도 유휴 판정에 든다([`Daemon::count_agents`] — M7-20).
//! - **직접 연결(QUIC — M7-24)**([`Daemon::serve_direct`]): 받은 스트림은 `pw_link::serve_noise` 로만 잇는다(Noise 없이 링크 틀을
//!   받는 길 없음 · 좁은 명령은 받지 않는다). **소켓과 따로 된** 증명 전 자리([`MAX_DIRECT_UNPROVEN`] — 직접 연결이 소켓의 자리를 가져가지 못해 네트워크의 상대가 SSH
//!   `proxy` · 좁은 명령을 막지 못한다, M7-24 리뷰 F1)에 **주소(IP)별 상한**([`MAX_UNPROVEN_PER_ADDRESS`])을
//!   더 두고, 받은 때부터 증명(또는 코드 페어링)까지 **전체 시한 하나**([`DIRECT_PROOF_TIMEOUT`])를 건다 — Noise · 첫 틀 · 코드
//!   페어링마다 따로인 `dead_after`(M7-23 리뷰 R8)보다 먼저 끝난다. 한 주소가 자리를 채워 링크 · 대기 코드를 막지 못하게(R4).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_link::frame::{Framed, LinkFrame};
use pw_link::{LinkStream, RelayParts};
use pw_rpc::Role;
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio_util::sync::CancellationToken;

use crate::command::{Command, NarrowHandler, Reply, Request, Status, reply_frame};
use crate::config::IdleConfig;
use crate::idle::{Action, Busy, IdleClock};
use crate::socket::Listener;

/// 증명 전 연결의 수 상한(출발값). 사람의 Desktop 하나는 링크 하나를 연다 — 남는 자리는 좁은 명령(askpass 등)과 다시 붙기의 몫이다.
pub const MAX_UNPROVEN: usize = 16;
/// 첫 틀을 기다리는 시간.
pub const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// 직접 연결을 받은 때부터 링크가 증명될 때까지(코드 페어링이면 끝날 때까지)의 전체 시한(출발값 — M7-04 리뷰 권고 1).
pub const DIRECT_PROOF_TIMEOUT: Duration = Duration::from_secs(5);
/// 직접 연결 전체의 증명 전 자리(출발값). 소켓의 [`MAX_UNPROVEN`] 과 따로다(리뷰 F1).
pub const MAX_DIRECT_UNPROVEN: usize = 8;
/// 직접 연결에서 주소(IP) 하나가 쥘 수 있는 증명 전 자리(출발값). [`MAX_DIRECT_UNPROVEN`] 안에서 센다.
pub const MAX_UNPROVEN_PER_ADDRESS: usize = 4;

/// Host 에 남은 일(실행 · 터미널)과 언어 서버 끄기. 데몬은 `pw_host::Activity` 를 넣는다.
pub trait Activity: Send + Sync + 'static {
    fn runs(&self) -> usize;
    fn terminals(&self) -> usize;
    fn stop_language_servers(&self) -> Pin<Box<dyn Future<Output = usize> + Send + '_>>;
}

impl Activity for pw_host::Activity {
    fn runs(&self) -> usize {
        pw_host::Activity::runs(self)
    }

    fn terminals(&self) -> usize {
        pw_host::Activity::terminals(self)
    }

    fn stop_language_servers(&self) -> Pin<Box<dyn Future<Output = usize> + Send + '_>> {
        Box::pin(pw_host::Activity::stop_language_servers(self))
    }
}

/// 링크가 증명될 때 부르는 것([`Daemon::on_proven`]).
pub type ProvenHook = Arc<dyn Fn(&pw_link::Proven) + Send + Sync>;

/// 붙은(증명된) 링크의 수.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Links {
    pub all: usize,
    pub owner: usize,
}

/// 붙은 링크. 복제하면 같은 것을 본다. 「붙은 owner Desktop 이 있는가」(결정 M7-D10 ④ — Relay MCP 세션의 `write` 이상을 헤드리스처럼
/// 거절할지, M7-20)는 [`Self::has_owner`] 로 묻는다.
#[derive(Clone)]
pub struct Presence {
    links: Arc<watch::Sender<Links>>,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            links: Arc::new(watch::Sender::new(Links::default())),
        }
    }
}

impl Presence {
    pub fn links(&self) -> Links {
        *self.links.borrow()
    }

    /// 기기 키를 증명한 owner 링크가 하나라도 붙어 있는가.
    pub fn has_owner(&self) -> bool {
        self.links().owner > 0
    }

    /// 바뀔 때마다 깨는 수신기.
    pub fn watch(&self) -> watch::Receiver<Links> {
        self.links.subscribe()
    }

    fn attach(&self, role: Role) -> LinkGuard {
        let owner = role == Role::Owner;
        self.links.send_modify(|l| {
            l.all += 1;
            l.owner += usize::from(owner);
        });
        LinkGuard {
            presence: self.clone(),
            owner,
        }
    }
}

/// 링크 하나가 붙어 있는 동안. 버리면 센 것을 뺀다.
struct LinkGuard {
    presence: Presence,
    owner: bool,
}

impl Drop for LinkGuard {
    fn drop(&mut self) {
        let owner = self.owner;
        self.presence.links.send_modify(|l| {
            l.all -= 1;
            l.owner -= usize::from(owner);
        });
    }
}

/// 데몬을 이루는 것.
pub struct DaemonParts {
    /// 링크를 받는 데 드는 것(기기 키 · 판 · 페어링 표 · Host · 상한).
    pub link: RelayParts,
    pub activity: Arc<dyn Activity>,
    /// `open` · `askpass` 의 몸통(M7-15).
    pub narrow: Arc<dyn NarrowHandler>,
    pub idle: IdleConfig,
    /// `relay.toml` 의 `name`.
    pub name: Option<String>,
    pub max_unproven: usize,
}

/// 데몬이 끝난 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// `[idle] exit_after` 가 지났다.
    Idle,
    /// 끝내라는 신호를 받았다.
    Shutdown,
}

/// 데몬. 복제하면 같은 것을 본다.
#[derive(Clone)]
pub struct Daemon {
    shared: Arc<Shared>,
}

struct Shared {
    parts: DaemonParts,
    presence: Presence,
    unproven: Arc<Semaphore>,
    /// 직접 연결의 증명 전 자리(소켓과 따로).
    direct_unproven: Arc<Semaphore>,
    /// 지금 쥔 증명 전 자리 수(시험 · 로그).
    pending: AtomicUsize,
    started: std::time::Instant,
    /// 열린 MCP 연결(18-m7-plan M7-20 — [`Daemon::count_agents`]). 유휴 판정이 본다.
    agents: std::sync::OnceLock<pw_mcp::socket::Connections>,
    /// 링크가 증명될 때마다 부른다(기기 화면의 마지막 접속 — [`Daemon::on_proven`], M7-23).
    proven: std::sync::OnceLock<ProvenHook>,
    /// 직접 연결의 주소별 증명 전 자리 수.
    by_address: Mutex<std::collections::HashMap<std::net::IpAddr, usize>>,
    /// 직접 연결의 증명 전 시한(시험이 줄인다).
    direct_timeout: Mutex<Duration>,
}

impl Daemon {
    pub fn new(parts: DaemonParts) -> Self {
        let unproven = Arc::new(Semaphore::new(parts.max_unproven));
        Self {
            shared: Arc::new(Shared {
                parts,
                presence: Presence::default(),
                unproven,
                direct_unproven: Arc::new(Semaphore::new(MAX_DIRECT_UNPROVEN)),
                pending: AtomicUsize::new(0),
                started: std::time::Instant::now(),
                agents: std::sync::OnceLock::new(),
                proven: std::sync::OnceLock::new(),
                by_address: Mutex::default(),
                direct_timeout: Mutex::new(DIRECT_PROOF_TIMEOUT),
            }),
        }
    }

    pub fn presence(&self) -> Presence {
        self.shared.presence.clone()
    }

    /// MCP 소켓의 연결 수를 유휴 판정에 넣는다(18-m7-plan M7-20) — 원격 에이전트가 붙어 있는 동안은 링크가 없어도 끝나지 않는다.
    /// 두 번째부터는 무시한다.
    pub fn count_agents(&self, connections: pw_mcp::socket::Connections) {
        let _ = self.shared.agents.set(connections);
    }

    /// 링크가 증명될 때마다 `hook` 을 부른다(한 번만 붙는다). 데몬은 기기의 마지막 접속을 적는다(`crate::devices::record_seen`).
    pub fn on_proven(&self, hook: ProvenHook) {
        let _ = self.shared.proven.set(hook);
    }

    /// 지금 증명 전 자리를 쥔 연결 수.
    pub fn unproven(&self) -> usize {
        self.shared.pending.load(Ordering::SeqCst)
    }

    pub fn busy(&self) -> Busy {
        Busy {
            links: self.shared.presence.links().all,
            runs: self.shared.parts.activity.runs(),
            terminals: self.shared.parts.activity.terminals(),
            agents: self
                .shared
                .agents
                .get()
                .map_or(0, pw_mcp::socket::Connections::active),
        }
    }

    pub fn status(&self) -> Status {
        let links = self.shared.presence.links();
        let busy = self.busy();
        Status {
            version: crate::version().to_owned(),
            release_channel: crate::RELEASE_CHANNEL.to_owned(),
            name: self.shared.parts.name.clone(),
            pid: std::process::id(),
            uptime_ms: u64::try_from(self.shared.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            links: links.all,
            owner_links: links.owner,
            runs: busy.runs,
            terminals: busy.terminals,
        }
    }

    /// 소켓에서 받다가 유휴 시한이 지나거나 `shutdown` 이 취소되면 끝난다.
    pub async fn run(&self, listener: Listener, shutdown: CancellationToken) -> Ended {
        let ended = tokio::select! {
            () = shutdown.cancelled() => Ended::Shutdown,
            () = self.wait_idle() => Ended::Idle,
            () = self.accept_loop(&listener) => Ended::Shutdown,
        };
        // 붙은 링크에 `bye` 를 보낸다 — Desktop 이 끊김으로 보고 이어 붙기를 되풀이하지 않게(18-m7-plan M7-09 ①).
        self.shared.parts.link.resumes.close_all().await;
        ended
    }

    async fn accept_loop(&self, listener: &Listener) {
        loop {
            match listener.accept().await {
                Ok(stream) => {
                    self.accept(LinkStream::split(stream));
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    tracing::warn!(%e, "refused a relay socket connection");
                }
                Err(e) => {
                    tracing::warn!(%e, "relay socket accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// 유휴 시한이 지날 때 돌아온다. 그 사이 링크 없이 `stop_lsp_after` 가 지나면 언어 서버를 끈다.
    pub async fn wait_idle(&self) {
        let mut clock = IdleClock::new(self.shared.parts.idle, tokio::time::Instant::now());
        let mut tick = tokio::time::interval(clock.period());
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match clock.tick(tokio::time::Instant::now(), self.busy()) {
                Some(Action::Exit) => {
                    tracing::info!("nothing left on the relay; exiting");
                    return;
                }
                Some(Action::StopLanguageServers) => {
                    let stopped = self.shared.parts.activity.stop_language_servers().await;
                    if stopped > 0 {
                        tracing::info!(
                            stopped,
                            "no link for a while; stopped the language servers"
                        );
                    }
                }
                None => {}
            }
        }
    }

    /// 받은 연결 하나. 증명 전 자리가 없으면 곧바로 닫고 `false`.
    pub fn accept(&self, stream: LinkStream) -> bool {
        let Ok(permit) = Arc::clone(&self.shared.unproven).try_acquire_owned() else {
            tracing::warn!(
                limit = self.shared.parts.max_unproven,
                "too many unproven relay connections; closing a new one"
            );
            return false;
        };
        let slot = Slot::new(&self.shared, permit);
        let daemon = self.clone();
        tokio::spawn(async move { daemon.connection(stream, slot).await });
        true
    }

    async fn connection(&self, stream: LinkStream, slot: Slot) {
        let mut framed = Framed::handshake(stream);
        let first = match tokio::time::timeout(FIRST_FRAME_TIMEOUT, framed.read.read_value()).await
        {
            Ok(Ok(Some((value, _)))) => value,
            Ok(Ok(None)) => return,
            Ok(Err(e)) => {
                tracing::info!(%e, "a relay connection sent a bad first frame");
                return;
            }
            Err(_) => {
                tracing::info!("a relay connection sent no first frame in time");
                return;
            }
        };
        let kind = first
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if kind == "hello" {
            self.link(framed, first, slot).await;
        } else if let Some(command) = Command::from_kind(&kind) {
            // 요청 틀을 받았다 — 증명 전 자리를 놓는다. askpass · open 은 사람의 답을 오래 기다릴 수 있어 자리를 쥐면 링크가 붙지
            // 못한다. 기다리는 수는 처리기가 따로 센다(`crate::narrow::MAX_WAITING`).
            drop(slot);
            let Framed {
                mut read,
                mut write,
            } = framed;
            // 부른 쪽이 답을 받기 전에 끝나면(ssh 가 askpass 를 거둠 · 셸에서 끊음) 처리를 버린다 — 창의 다이얼로그가 닫힌다. 요청 뒤에
            // 더 온 틀은 읽어 버린다(좁은 명령의 연결은 요청 하나 · 답 하나다 — 어디에도 넘기지 않는다).
            let gone = async move { while let Ok(Some(_)) = read.read_value().await {} };
            let reply = tokio::select! {
                biased;
                reply = self.narrow(command, first) => reply,
                () = gone => return,
            };
            let _ = write
                .write_encoded(&serde_json::to_vec(&reply_frame(&reply)).unwrap_or_default())
                .await;
        } else {
            tracing::info!(kind = %kind.chars().take(32).collect::<String>(), "refused a relay connection with an unknown first frame");
            let reply: Reply =
                Err("the first frame is neither a link hello nor a relay command".to_owned());
            let _ = framed
                .write
                .write_encoded(&serde_json::to_vec(&reply_frame(&reply)).unwrap_or_default())
                .await;
        }
    }

    async fn narrow(&self, command: Command, first: Value) -> Reply {
        let body = match serde_json::from_value::<Request>(first) {
            Ok(request) => request.body,
            Err(e) => return Err(format!("bad request: {e}")),
        };
        let handler = &self.shared.parts.narrow;
        match command {
            Command::Status => Ok(json!(self.status())),
            Command::Open => handler.open(body).await,
            Command::Askpass => handler.askpass(body).await,
        }
    }

    /// 시험: 직접 연결의 증명 전 시한을 바꾼다.
    pub fn set_direct_timeout(&self, timeout: Duration) {
        *self
            .shared
            .direct_timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = timeout;
    }

    /// 지금 `ip` 가 쥔 직접 연결의 증명 전 자리 수.
    pub fn unproven_from(&self, ip: std::net::IpAddr) -> usize {
        self.shared
            .by_address
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&ip)
            .copied()
            .unwrap_or(0)
    }

    /// QUIC 엔드포인트 하나에서 받는다(엔드포인트가 닫히면 끝난다). `codes` 는 대기 코드(`crate::codes::FileCodeBook`) — 파일 잠금 ·
    /// `fsync` 를 하는 동기 호출이라 런타임 일꾼을 막지 않게 감싸서 부른다(M7-23 리뷰 R3).
    pub async fn serve_direct(
        &self,
        listener: Arc<pw_link::quic::QuicListener>,
        codes: Arc<dyn pw_link::CodeBook>,
    ) {
        let codes: Arc<dyn pw_link::CodeBook> = Arc::new(Blocking(codes));
        while let Some(incoming) = listener.next().await {
            let ip = incoming.remote().ip();
            let Some(slot) = self.direct_slot(ip) else {
                incoming.refuse();
                continue;
            };
            let (daemon, codes) = (self.clone(), Arc::clone(&codes));
            tokio::spawn(async move { daemon.direct(incoming, codes, slot).await });
        }
    }

    /// 시험: 주소 `ip` 의 직접 연결 증명 전 자리 하나를 쥔다(버리면 놓는다). 없으면 `None`. 이 기기 루프백 주소 하나로는 여러 주소를
    /// 만들 수 없어 둔 길이다(리뷰 F1).
    #[doc(hidden)]
    pub fn hold_direct_slot(&self, ip: std::net::IpAddr) -> Option<DirectSlot> {
        self.direct_slot(ip).map(DirectSlot)
    }

    /// 주소별 · 전체 증명 전 자리 하나. 없으면 `None`.
    fn direct_slot(&self, ip: std::net::IpAddr) -> Option<Slot> {
        let mut by = self
            .shared
            .by_address
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let held = by.get(&ip).copied().unwrap_or(0);
        if held >= MAX_UNPROVEN_PER_ADDRESS {
            tracing::warn!(
                limit = MAX_UNPROVEN_PER_ADDRESS,
                "one address holds too many unproven direct connections; refusing a new one"
            );
            return None;
        }
        let Ok(permit) = Arc::clone(&self.shared.direct_unproven).try_acquire_owned() else {
            tracing::warn!(
                limit = MAX_DIRECT_UNPROVEN,
                "too many unproven direct connections; refusing a new one"
            );
            return None;
        };
        by.insert(ip, held + 1);
        let mut slot = Slot::new(&self.shared, permit);
        slot.address = Some(ip);
        Some(slot)
    }

    async fn direct(
        &self,
        incoming: pw_link::quic::QuicIncoming,
        codes: Arc<dyn pw_link::CodeBook>,
        slot: Slot,
    ) {
        let limit = *self
            .shared
            .direct_timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let proven_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let attached: Arc<Mutex<Option<LinkGuard>>> = Arc::default();
        let serve = {
            let (proven_flag, held) = (Arc::clone(&proven_flag), Arc::clone(&attached));
            let presence = self.shared.presence.clone();
            let hook = self.shared.proven.get().cloned();
            let parts = &self.shared.parts.link;
            async move {
                let stream = incoming
                    .accept()
                    .await
                    .map_err(|e| pw_link::ServeError::Io(e.to_string()))?;
                pw_link::serve_noise(stream, parts, &*codes, move |proven| {
                    // 증명됐다 — 전체 시한을 풀고, 증명 전 자리(주소별 포함)를 놓고 붙은 링크로 센다.
                    proven_flag.store(true, Ordering::SeqCst);
                    drop(slot);
                    if let Some(hook) = hook {
                        hook(proven);
                    }
                    *held.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(presence.attach(proven.role));
                })
                .await
            }
        };
        tokio::pin!(serve);
        let result = match tokio::time::timeout(limit, &mut serve).await {
            Ok(result) => Some(result),
            Err(_) if proven_flag.load(Ordering::SeqCst) => Some(serve.await),
            // 시한 안에 증명하지 못했다 — 미래를 버리면 QUIC 연결이 닫히고 자리가 돌아온다.
            Err(_) => None,
        };
        attached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match result {
            None => tracing::info!("a direct connection did not prove itself in time"),
            Some(Ok(pw_link::Served::Link(reason))) => {
                tracing::info!(?reason, "a direct link closed");
            }
            Some(Ok(pw_link::Served::Paired { device, role })) => {
                tracing::info!(%device, ?role, "paired a device with a code over a direct connection");
            }
            Some(Err(e)) => tracing::info!(%e, "a direct link did not stand"),
        }
    }

    async fn link(&self, framed: Framed, first: Value, slot: Slot) {
        let Ok(first) = serde_json::from_value::<LinkFrame>(first) else {
            return;
        };
        let attached: Arc<Mutex<Option<LinkGuard>>> = Arc::default();
        let held = Arc::clone(&attached);
        let presence = self.shared.presence.clone();
        let hook = self.shared.proven.get().cloned();
        let result = pw_link::serve_proven(framed, first, &self.shared.parts.link, move |proven| {
            // 증명됐다 — 증명 전 자리를 놓고 붙은 링크로 센다.
            drop(slot);
            if let Some(hook) = hook {
                hook(proven);
            }
            *held.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(presence.attach(proven.role));
        })
        .await;
        attached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match result {
            Ok(reason) => tracing::info!(?reason, "a link closed"),
            Err(e) => tracing::info!(%e, "a link did not stand"),
        }
    }
}

/// 쥔 직접 연결 증명 전 자리([`Daemon::hold_direct_slot`]). 버리면 놓는다.
pub struct DirectSlot(#[allow(dead_code)] Slot);

/// 증명 전 자리 하나.
struct Slot {
    _permit: OwnedSemaphorePermit,
    shared: Arc<Shared>,
    /// 직접 연결이면 그 주소(주소별 자리를 함께 놓는다).
    address: Option<std::net::IpAddr>,
}

impl Slot {
    fn new(shared: &Arc<Shared>, permit: OwnedSemaphorePermit) -> Self {
        shared.pending.fetch_add(1, Ordering::SeqCst);
        Self {
            _permit: permit,
            shared: Arc::clone(shared),
            address: None,
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.shared.pending.fetch_sub(1, Ordering::SeqCst);
        if let Some(ip) = self.address {
            let mut by = self
                .shared
                .by_address
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(n) = by.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    by.remove(&ip);
                }
            }
        }
    }
}

/// 동기 대기 코드(파일 잠금 · `fsync`)를 런타임 일꾼을 막지 않고 부른다 — 여러 일꾼 런타임이면 `block_in_place`(M7-23 리뷰 R3).
struct Blocking(Arc<dyn pw_link::CodeBook>);

fn blocking<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(f),
        _ => f(),
    }
}

impl pw_link::CodeBook for Blocking {
    fn attempt(&self) -> std::io::Result<Option<pw_link::pair::Pending>> {
        blocking(|| self.0.attempt())
    }

    fn complete(
        &self,
        code: &pw_link::PairingCode,
        key: &pw_link::PublicKey,
        role: Role,
        name: &str,
    ) -> std::io::Result<()> {
        blocking(|| self.0.complete(code, key, role, name))
    }
}
