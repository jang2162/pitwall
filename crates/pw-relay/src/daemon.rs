//! 데몬의 받기(18-m7-plan M7-04 ①): 소켓에서 연결을 받아 첫 틀로 링크와 좁은 명령을 가른다.
//!
//! - **증명 전 연결 수 상한**([`DaemonParts::max_unproven`]): 연결은 받는 순간부터 링크가 증명될 때까지(또는 좁은 명령이 끝날 때까지)
//!   자리 하나를 쥔다. 자리가 없으면 그 연결은 곧바로 닫는다 — 증명하지 않는 연결로 데몬을 채우지 못하게. 네트워크에 열리는 전송
//!   (QUIC — M7-24) 전에 반드시 있어야 하는 것이다.
//! - **첫 틀 · 핸드셰이크 틀 상한** `pw_link::HANDSHAKE_MAX_FRAME`(64 KiB). 증명 뒤에만 링크 틀 상한으로 오른다(`pw_link`).
//! - 첫 틀은 [`FIRST_FRAME_TIMEOUT`] 안에 와야 한다.
//! - 증명된 링크는 [`Presence`] 에 센다 — 유휴 판정과 「붙은 owner 링크가 있는가」(M7-20 의 M7-D10 ④ 판정 — `crate::mcp::services`)가 본다.
//! - 열린 MCP 연결도 유휴 판정에 든다([`Daemon::count_agents`] — M7-20).

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
    /// 지금 쥔 증명 전 자리 수(시험 · 로그).
    pending: AtomicUsize,
    started: std::time::Instant,
    /// 열린 MCP 연결(18-m7-plan M7-20 — [`Daemon::count_agents`]). 유휴 판정이 본다.
    agents: std::sync::OnceLock<pw_mcp::socket::Connections>,
}

impl Daemon {
    pub fn new(parts: DaemonParts) -> Self {
        let unproven = Arc::new(Semaphore::new(parts.max_unproven));
        Self {
            shared: Arc::new(Shared {
                parts,
                presence: Presence::default(),
                unproven,
                pending: AtomicUsize::new(0),
                started: std::time::Instant::now(),
                agents: std::sync::OnceLock::new(),
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

    async fn link(&self, framed: Framed, first: Value, slot: Slot) {
        let Ok(first) = serde_json::from_value::<LinkFrame>(first) else {
            return;
        };
        let attached: Arc<Mutex<Option<LinkGuard>>> = Arc::default();
        let held = Arc::clone(&attached);
        let presence = self.shared.presence.clone();
        let result = pw_link::serve_proven(framed, first, &self.shared.parts.link, move |proven| {
            // 증명됐다 — 증명 전 자리를 놓고 붙은 링크로 센다.
            drop(slot);
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

/// 증명 전 자리 하나.
struct Slot {
    _permit: OwnedSemaphorePermit,
    shared: Arc<Shared>,
}

impl Slot {
    fn new(shared: &Arc<Shared>, permit: OwnedSemaphorePermit) -> Self {
        shared.pending.fetch_add(1, Ordering::SeqCst);
        Self {
            _permit: permit,
            shared: Arc::clone(shared),
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.shared.pending.fetch_sub(1, Ordering::SeqCst);
    }
}
