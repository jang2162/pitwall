//! 다중화(18-m7-plan M7-03 ④): 링크 하나(Desktop, 머신) 위의 논리 연결 여럿. `Host::connect` 하나가 채널 하나다.
//!
//! - **흐름 제어**: 채널마다 받는 쪽이 창([`LinkLimits::window`] 바이트)을 주고, 꺼내 쓴 만큼 `credit` 으로 돌려준다(창의 반이
//!   모이거나 받아 둔 것을 다 꺼냈을 때). 보내는 쪽은 틀 하나가 **남은 창에 통째로 들어갈 때만** 그 채널의 틀을 내보낸다 — 한
//!   창(window)이 읽지 않아도 그 채널만 멈추고 링크의 다른 채널은 돈다. 남은 창을 넘는 틀을 보낸 상대(규칙을 어긴 상대)의 채널은
//!   받는 쪽이 닫는다(메인 판정 M7-P19 「받는 스트림은 credit · 상한으로」).
//! - **링크 받기 예산**: 받는 쪽이 내준 창(credit)의 합은 링크마다 [`LinkLimits::receive_budget`] 를 넘지 않는다 — 털린 상대가
//!   채널을 여럿 열어 이쪽 메모리를 채널 수 × 창만큼 채우지 못하게. 채널 하나를 열 때 기본 창([`LinkLimits::window`])을 예산에서
//!   잡고, 예산이 모자라면 그 채널을 열지 않는다. **창보다 큰 틀**(틀 상한 `pw-wire` 64 MiB 까지 — `fs.read` 의 큰 응답 등)은
//!   보내는 쪽이 `want` 로 그 틀만큼 창을 청하고, 받는 쪽은 예산이 남으면 빌려 주고(그 틀을 꺼내 쓰면 예산으로 돌아간다) 모자라면
//!   그 채널을 닫는다.
//! - **링크 틀 큐 상한**: `pong` · `credit` · 채널 응답처럼 이쪽이 내는 링크 틀이 [`LinkLimits::control_buffer`] 를 넘게 쌓이면
//!   (상대가 읽지 않으며 `ping` · 나쁜 채널 열기를 몰아 보냄) 링크를 끊는다 — 증명된 상대 하나가 이쪽 메모리를 끝없이 늘리지 못하게.
//! - **보내기 버퍼 상한**: 창이 없어 못 보낸 틀이 채널마다 [`LinkLimits::send_buffer`] 를 넘으면 그 채널을 닫는다(빈 버퍼에는 틀
//!   하나를 크기와 무관하게 받는다 — 큰 응답 하나로 채널이 닫히지 않게). 닫힌 채널은 Host 쪽에서 그 연결이 닫힌다(진행 중 요청 ·
//!   스트림 취소 · 구독 걷기 — `pw_core::host` 의 계약).
//! - **하트비트**: 양쪽이 [`LinkLimits::ping_every`] 마다 `ping` 을 보내고, [`LinkLimits::dead_after`] 동안 아무 틀도 받지 못하면
//!   링크를 끊긴 것으로 보고 전송을 닫는다(계획 재리뷰 F-g — M7-09 의 이어 붙기 시한이 여기서 시작된다). 쓰기가 막힌 전송(상대가
//!   읽지 않음)에서도 하트비트는 따로 돈다.
//! - 링크 자체의 채널(0)에는 `ping` · `pong` 과 핸드셰이크 뒤의 채널 열기 응답만 온다. 채널 0 의 `msg` 는 규칙 위반이라 링크를
//!   끊는다.
//! - **이어 붙기**(18-m7-plan M7-09 ① — 메인 판정 M7-P8, [`Mux::start_resumable`]): 하트비트 · 쓰기 · 읽기가 전송을 잃으면(끊김 ·
//!   응답 없음) 링크를 닫지 않고 **멈춘다** — 채널은 그대로 두고 보낼 것은 쌓는다(채널마다 [`LinkLimits::send_buffer`] · 링크마다
//!   [`LinkLimits::send_budget`] — 넘으면 그 채널만 닫는다, 시한 안에서도). [`LinkLimits::resume_within`] 안에 새 전송이 오면
//!   ([`Mux::resume`]) 잃은 틀을 다시 보내고 이어 간다. 시한이 지나면 링크를 닫는다(채널마다 보통 정리).
//!   - 링크 틀 중 `ping` · `pong` · `ack` · `bye` 를 뺀 것(채널 메시지 · 열기 · 응답 · 닫기 · `credit` · `want`)에는 보낸 순서의 번호가
//!     있다(와이어에 싣지 않고 양쪽이 센다). 받는 쪽은 받은 수를 `ack` 로 알려 주고(창의 반만큼 받을 때마다 · 하트비트마다), 보내는
//!     쪽은 `ack` 받지 못한 틀을 다시 보낼 줄([`LinkLimits::send_budget`] 상한 — 넘으면 상대가 받았다고 하지 않는 것이라 링크를 끊는다)에
//!     둔다. 이어 붙을 때 양쪽이 받은 수를 주고받고 그 뒤의 틀만 다시 보낸다 — 잃은 틀도 두 번 받는 틀도 없다.
//!   - 일부러 닫으면(이쪽이 닫음 · 규칙 위반 · 페어링 변경) 상대에게 `bye` 를 보내 상대가 시한 동안 기다리지 않게 한다. 틀 오류 ·
//!     규칙 위반은 멈추지 않고 닫는다.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use serde_json::value::RawValue;

use crate::frame::{
    CONTROL, FrameError, Framed, FramedRead, FramedWrite, Kind, LinkFrame, RawFrame,
};

/// 링크의 상한 · 시한. 값은 출발값이다(18-m7-plan M7-03 ④).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkLimits {
    /// 링크 하나의 채널 수 상한.
    pub max_channels: usize,
    /// 채널마다 받는 쪽이 기본으로 내주는 창(바이트). 이보다 큰 틀은 `want` 로 링크 받기 예산에서 빌린다. 보내는 쪽은 상대의
    /// 기본 창을 **자기 값으로 가정한다**(`want` 를 낼지 판정) — 양쪽은 핸드셰이크에서 앱 판이 정확히 같아야 붙고 이 값은 설정이
    /// 아니라 빌드에 박힌 기본값이라 같다. 이 값을 설정으로 열거나 판마다 바꾸면 창을 채널 열기(`connect` · `accepted`)에 실어야 한다.
    pub window: usize,
    /// 링크 하나에서 받는 쪽이 내준 창의 합의 상한(바이트). 기본은 채널 상한 × 기본 창(64 MiB)에 큰 틀 셋(64 MiB 씩)을 빌려 줄
    /// 자리를 더한 것이다.
    pub receive_budget: usize,
    /// 채널마다 창이 없어 쌓인 보낼 바이트의 상한. 넘으면 그 채널을 닫는다.
    pub send_buffer: usize,
    /// 링크 하나에서 창이 없어 못 보내고 쌓인 채널 메시지의 합의 상한(바이트). 넘으면 그 메시지의 채널을 닫는다 — `want` 에
    /// 답하지도 채널을 닫지도 않는 상대에게 채널마다 큰 틀이 쌓이지 않게.
    pub send_budget: usize,
    /// 이쪽이 내는 링크 틀(`pong` · `credit` · 채널 응답 · 닫기)이 못 나가고 쌓일 수 있는 바이트. 넘으면 링크를 끊는다.
    pub control_buffer: usize,
    /// 하트비트 간격.
    pub ping_every: Duration,
    /// 이만큼 아무 틀도 받지 못하면 링크가 끊긴 것이다. 핸드셰이크의 시한이기도 하다.
    pub dead_after: Duration,
    /// 이어 붙기 시한(18-m7-plan M7-09 ① — 출발값 2분). 전송을 잃은 링크는 이만큼 채널을 쥐고 다시 붙기를 기다린다.
    pub resume_within: Duration,
}

impl Default for LinkLimits {
    fn default() -> Self {
        Self {
            max_channels: 256,
            window: 256 * 1024,
            receive_budget: 256 * 1024 * 1024,
            send_buffer: 16 * 1024 * 1024,
            send_budget: 256 * 1024 * 1024,
            control_buffer: 256 * 1024,
            ping_every: Duration::from_secs(10),
            dead_after: Duration::from_secs(30),
            resume_within: Duration::from_secs(120),
        }
    }
}

/// 링크가 닫힌 까닭.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "camelCase")]
pub enum CloseReason {
    /// 이쪽이 닫았다.
    Local,
    /// 상대가 전송을 닫았다.
    PeerClosed,
    /// [`LinkLimits::dead_after`] 동안 아무 틀도 받지 못했다.
    HeartbeatTimeout,
    /// 틀 오류 · 규칙 위반(상한을 넘은 틀 포함). 값은 사람이 읽는 까닭.
    Protocol(String),
    /// 전송 읽기 · 쓰기 오류.
    Io(String),
    /// 그 기기의 페어링이 빠지거나 role 이 바뀌었다(Relay — M7-02 ②).
    PairingChanged,
    /// 핸드셰이크가 끝나지 않았다(Desktop 의 상태 — 판 · 머신 id 가 아닌 까닭).
    Handshake(String),
    /// 이 전송은 멈춘 링크를 이어 받았다(Relay — 링크는 그 링크를 처음 받은 `serve` 가 계속 쥔다, 18-m7-plan M7-09 ①).
    Resumed,
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("closed here"),
            Self::PeerClosed => f.write_str("the other side closed the link"),
            Self::HeartbeatTimeout => f.write_str("no heartbeat"),
            Self::Protocol(why) => write!(f, "link protocol error: {why}"),
            Self::Io(why) => write!(f, "link i/o error: {why}"),
            Self::PairingChanged => f.write_str("the device pairing changed"),
            Self::Handshake(why) => write!(f, "handshake failed: {why}"),
            Self::Resumed => f.write_str("handed to the link it resumed"),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CreditBody {
    bytes: u64,
}

/// 보낼 틀 하나. `windowed` 가 거짓이면(채널을 닫는 틀) 창과 무관하게 나간다.
struct Out {
    bytes: Vec<u8>,
    windowed: bool,
}

/// 받아 두고 아직 꺼내지 않은 채널 메시지: 원시 JSON 과 그 와이어 바이트 수. 꺼낼 때([`ChannelRx::recv`]) 푼다 — 받기 예산은 이
/// 원시 바이트를 센다.
type Held = (Box<RawValue>, usize);

struct Chan {
    out: VecDeque<Out>,
    out_bytes: usize,
    send_window: i64,
    /// `ready` 줄에 들어 있는가.
    queued: bool,
    /// 받은 메시지를 꺼내 쓰는 쪽. 채널을 닫으면 버려져 받는 쪽이 끝을 본다.
    inbound: mpsc::UnboundedSender<Held>,
    /// 상대가 창을 넘지 않고 더 보낼 수 있는 바이트.
    recv_remaining: i64,
    /// 꺼내 썼지만 아직 `credit` 으로 돌려주지 않은 바이트.
    recv_consumed: usize,
    /// 이 채널에 내준 창(남은 창 + 받아 두었거나 꺼냈지만 돌려주지 않은 바이트). 링크 받기 예산에서 잡은 양이다.
    grant: usize,
    /// 맨 앞 틀을 보내려고 `want` 를 보냈다.
    wanted: bool,
    cancel: CancellationToken,
}

/// 채널을 열지 못한 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddError {
    /// 링크가 닫혔다.
    Closed,
    /// 이미 열린 채널 번호다.
    Exists,
    /// 링크 받기 예산이 기본 창을 내줄 만큼 남지 않았다.
    Budget,
}

#[derive(Default)]
struct State {
    /// 번호 없는 링크 틀(`ping` · `pong` · `ack` · `bye`) — 다시 보내지 않는다.
    link: VecDeque<Vec<u8>>,
    control: VecDeque<Vec<u8>>,
    control_bytes: usize,
    channels: HashMap<u32, Chan>,
    ready: VecDeque<u32>,
    /// 열린 채널의 `grant` 합.
    budget_used: usize,
    /// 열린 채널의 `out_bytes` 합(보내기 예산).
    send_used: usize,
    shut: bool,
    /// 이어 붙을 수 있는 링크인가([`Mux::start_resumable`]).
    resumable: bool,
    /// 전송의 세대 — 이어 붙을 때마다 하나씩. 옛 전송의 읽기 · 쓰기 · 하트비트는 세대가 다르면 그만둔다.
    generation: u64,
    /// 지금 전송의 읽기 · 쓰기 · 하트비트를 멈춘다.
    stop: CancellationToken,
    /// 전송을 잃고 이어 붙기를 기다린다.
    suspended: bool,
    /// 번호 있는 틀을 보낸 수.
    sent: u64,
    /// 보냈지만 `ack` 받지 못한 번호 있는 틀(앞이 `sent - replay.len() + 1` 번).
    replay: VecDeque<Vec<u8>>,
    replay_bytes: usize,
    /// `replay` 에서 다음에 (다시) 보낼 자리. 이어 붙은 뒤에는 0 부터, 평소에는 `replay.len()`.
    resend: usize,
    /// 번호 있는 틀을 받은 수.
    received: u64,
    /// `ack` 를 보낸 뒤 받은 바이트.
    unacked_bytes: usize,
    /// 마지막으로 보낸 `ack`.
    acked: u64,
}

impl State {
    /// 채널을 걷고 그 창을 예산으로 돌린다.
    fn remove_channel(&mut self, channel: u32) -> Option<Chan> {
        let chan = self.channels.remove(&channel)?;
        self.budget_used -= chan.grant;
        self.send_used -= chan.out_bytes;
        Some(chan)
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Notify,
    limits: LinkLimits,
    closed: CancellationToken,
    reason: OnceLock<CloseReason>,
    last_rx: Mutex<Instant>,
    /// 받은 틀을 세고 다루는 동안 쥔다 — 이어 붙을 때 「받은 수」 를 읽는 사이에 옛 전송의 틀이 끼어들지 않게.
    rx_gate: Mutex<()>,
    /// 다중화가 다루지 않는 틀(채널 열기 응답 등)을 받는 쪽 — 전송이 바뀌어도 같다.
    inbox: mpsc::UnboundedSender<LinkFrame>,
    phase: watch::Sender<Phase>,
}

/// 링크의 단계.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Live,
    /// 전송을 잃고 이어 붙기를 기다린다.
    Suspended,
    Closed,
}

/// `ack` 본문.
#[derive(Serialize, Deserialize)]
struct AckBody {
    seq: u64,
}

/// 번호가 붙는 틀인가(다시 보내는 틀).
fn sequenced(kind: Kind) -> bool {
    !matches!(kind, Kind::Ping | Kind::Pong | Kind::Ack | Kind::Bye)
}

/// 이쪽 쓰기가 다음에 할 일.
enum Next {
    Frame(Vec<u8>),
    Idle,
    /// 이 세대의 전송이 아니다 — 그만둔다.
    Stale,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 다중화한 링크. 복제는 같은 링크를 가리킨다. 마지막 복제를 버려도 링크는 닫히지 않는다 — [`Mux::shutdown`] 으로 닫는다.
#[derive(Clone)]
pub(crate) struct Mux {
    shared: Arc<Shared>,
}

/// 채널의 받는 쪽. 꺼낼 때마다 상대에게 창을 돌려준다.
pub(crate) struct ChannelRx {
    rx: mpsc::UnboundedReceiver<Held>,
    mux: Mux,
    channel: u32,
}

impl ChannelRx {
    /// 다음 메시지. 채널이 닫히고 남은 것을 다 꺼냈으면 `None`.
    pub(crate) async fn recv(&mut self) -> Option<Value> {
        loop {
            let (raw, len) = self.rx.recv().await?;
            self.mux.consumed(self.channel, len, self.rx.is_empty());
            // 원시 JSON 은 받을 때 꼴을 확인했다 — 풀리지 않는 것(깊이 상한 등)은 버린다.
            match serde_json::from_str::<Value>(raw.get()) {
                Ok(value) => return Some(value),
                Err(e) => {
                    tracing::debug!(target: "pw_link", channel = self.channel, error = %e, "dropped an undecodable message");
                }
            }
        }
    }
}

impl Mux {
    /// 핸드셰이크를 마친 전송 위에서 읽기 · 쓰기 · 하트비트를 띄운다. 채널 열기 응답처럼 다중화가 다루지 않는 틀은 돌려준 수신기로
    /// 온다.
    pub(crate) fn start(
        framed: Framed,
        limits: LinkLimits,
    ) -> (Self, mpsc::UnboundedReceiver<LinkFrame>) {
        Self::start_with(framed, limits, false)
    }

    /// [`Self::start`] 과 같되 전송을 잃으면 닫지 않고 멈춰 [`Self::resume`] 을 기다린다(머리 주석 「이어 붙기」).
    pub(crate) fn start_resumable(
        framed: Framed,
        limits: LinkLimits,
    ) -> (Self, mpsc::UnboundedReceiver<LinkFrame>) {
        Self::start_with(framed, limits, true)
    }

    fn start_with(
        framed: Framed,
        limits: LinkLimits,
        resumable: bool,
    ) -> (Self, mpsc::UnboundedReceiver<LinkFrame>) {
        let (inbox_tx, inbox) = mpsc::unbounded_channel();
        let stop = CancellationToken::new();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                resumable,
                stop: stop.clone(),
                ..State::default()
            }),
            wake: Notify::new(),
            limits,
            closed: CancellationToken::new(),
            reason: OnceLock::new(),
            last_rx: Mutex::new(Instant::now()),
            rx_gate: Mutex::new(()),
            inbox: inbox_tx,
            phase: watch::Sender::new(Phase::Live),
        });
        let mux = Self { shared };
        mux.spawn_transport(framed, 0, stop);
        (mux, inbox)
    }

    fn spawn_transport(&self, framed: Framed, generation: u64, stop: CancellationToken) {
        let Framed { read, write } = framed;
        tokio::spawn(self.clone().reader(read, generation, stop.clone()));
        tokio::spawn(self.clone().writer(write, generation, stop.clone()));
        tokio::spawn(self.clone().heartbeat(generation, stop));
    }

    /// 링크의 단계. 바뀔 때마다 깬다.
    pub(crate) fn phase(&self) -> watch::Receiver<Phase> {
        self.shared.phase.subscribe()
    }

    /// 전송을 잃었다(끊김 · 쓰기 오류 · 하트비트). 이어 붙을 수 있는 링크면 멈추고 시한을 잰다, 아니면 닫는다. 그 세대의 전송이
    /// 아니면(이미 바뀜) 아무것도 하지 않는다.
    fn transport_lost(&self, generation: u64, reason: CloseReason) {
        let mut state = lock(&self.shared.state);
        if state.shut || state.generation != generation || state.suspended {
            return;
        }
        if !state.resumable {
            drop(state);
            self.shutdown(reason);
            return;
        }
        self.suspend_locked(&mut state, reason);
    }

    fn suspend_locked(&self, state: &mut State, reason: CloseReason) {
        state.suspended = true;
        state.stop.cancel();
        state.link.clear();
        let generation = state.generation;
        tracing::info!(target: "pw_link", %reason, "link transport lost; waiting for it to resume");
        self.shared.phase.send_replace(Phase::Suspended);
        let (mux, within) = (self.clone(), self.shared.limits.resume_within);
        tokio::spawn(async move {
            let closed = mux.closed();
            tokio::select! {
                () = closed.cancelled() => return,
                () = tokio::time::sleep(within) => {}
            }
            let expired = {
                let state = lock(&mux.shared.state);
                state.suspended && state.generation == generation
            };
            if expired {
                tracing::info!(target: "pw_link", "the link did not resume in time");
                mux.shutdown(reason);
            }
        });
    }

    /// 이어 붙기 앞: 지금 전송을 멈추고(아직 살아 있다고 보던 것이면 — 상대가 먼저 끊김을 알았다) 받은 번호 있는 틀의 수를 준다.
    /// 그 뒤로는 옛 전송의 틀을 받지 않는다. 닫힌 링크면 `None`.
    pub(crate) fn suspend_for_resume(&self) -> Option<u64> {
        let _gate = lock(&self.shared.rx_gate);
        let mut state = lock(&self.shared.state);
        if state.shut {
            return None;
        }
        if !state.suspended {
            self.suspend_locked(
                &mut state,
                CloseReason::Io("replaced by a resumed transport".to_owned()),
            );
        }
        Some(state.received)
    }

    /// 새 전송으로 이어 간다. `peer_received` 는 상대가 받은 번호 있는 틀의 수 — 그 뒤의 틀부터 다시 보낸다. 상대가 이쪽이 이미
    /// 버린 틀(`ack` 받은 앞)보다 덜 받았거나 보낸 것보다 많이 받았다면 이어 갈 수 없다(부른 쪽이 링크를 닫는다). 먼저
    /// [`Self::suspend_for_resume`] 이나 전송을 잃어 멈춰 있어야 한다.
    pub(crate) fn resume(&self, framed: Framed, peer_received: u64) -> Result<(), String> {
        let _gate = lock(&self.shared.rx_gate);
        let mut state = lock(&self.shared.state);
        if state.shut {
            return Err("the link is closed".to_owned());
        }
        if !state.suspended {
            return Err("the link is not waiting to resume".to_owned());
        }
        let first = state.sent - state.replay.len() as u64;
        if peer_received < first || peer_received > state.sent {
            return Err(format!(
                "the peer received {peer_received} frames; this side kept {first}..{}",
                state.sent
            ));
        }
        for _ in first..peer_received {
            if let Some(frame) = state.replay.pop_front() {
                state.replay_bytes -= frame.len();
            }
        }
        state.resend = 0;
        state.generation += 1;
        state.suspended = false;
        let stop = CancellationToken::new();
        state.stop = stop.clone();
        state.unacked_bytes = 0;
        let generation = state.generation;
        let resent = state.replay.len();
        drop(state);
        *lock(&self.shared.last_rx) = Instant::now();
        tracing::info!(target: "pw_link", resent, "link resumed");
        self.spawn_transport(framed, generation, stop);
        self.shared.phase.send_replace(Phase::Live);
        self.shared.wake.notify_one();
        Ok(())
    }

    /// 받은 번호 있는 틀의 수(이어 붙기의 핸드셰이크가 싣는다).
    pub(crate) fn received(&self) -> u64 {
        let _gate = lock(&self.shared.rx_gate);
        lock(&self.shared.state).received
    }

    pub(crate) fn limits(&self) -> &LinkLimits {
        &self.shared.limits
    }

    /// 링크가 닫히면 취소된다.
    pub(crate) fn closed(&self) -> CancellationToken {
        self.shared.closed.clone()
    }

    /// 닫힌 까닭(아직 열려 있으면 `None`).
    pub(crate) fn reason(&self) -> Option<CloseReason> {
        self.shared.reason.get().cloned()
    }

    /// 링크를 닫는다. 모든 채널이 끝나고 전송이 닫힌다. 두 번째부터는 아무것도 하지 않는다.
    pub(crate) fn shutdown(&self, reason: CloseReason) {
        if self.shared.reason.set(reason.clone()).is_err() {
            return;
        }
        tracing::debug!(target: "pw_link", %reason, "link closed");
        let channels = {
            let mut state = lock(&self.shared.state);
            state.shut = true;
            state.control.clear();
            state.control_bytes = 0;
            state.ready.clear();
            state.budget_used = 0;
            state.send_used = 0;
            state.replay.clear();
            state.replay_bytes = 0;
            std::mem::take(&mut state.channels)
        };
        for chan in channels.into_values() {
            chan.cancel.cancel();
        }
        self.shared.closed.cancel();
        self.shared.phase.send_replace(Phase::Closed);
        self.shared.wake.notify_one();
    }

    pub(crate) fn channel_count(&self) -> usize {
        lock(&self.shared.state).channels.len()
    }

    pub(crate) fn has_channel(&self, channel: u32) -> bool {
        lock(&self.shared.state).channels.contains_key(&channel)
    }

    /// 채널을 연다(받을 자리를 만든다). 기본 창을 링크 받기 예산에서 잡는다. 채널이 닫히면 돌려준 토큰이 취소된다.
    pub(crate) fn add_channel(
        &self,
        channel: u32,
    ) -> Result<(ChannelRx, CancellationToken), AddError> {
        let mut state = lock(&self.shared.state);
        if state.shut {
            return Err(AddError::Closed);
        }
        if channel == CONTROL || state.channels.contains_key(&channel) {
            return Err(AddError::Exists);
        }
        if state.budget_used + self.shared.limits.window > self.shared.limits.receive_budget {
            return Err(AddError::Budget);
        }
        state.budget_used += self.shared.limits.window;
        let (tx, rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let window = i64::try_from(self.shared.limits.window).unwrap_or(i64::MAX);
        state.channels.insert(
            channel,
            Chan {
                out: VecDeque::new(),
                out_bytes: 0,
                send_window: window,
                queued: false,
                inbound: tx,
                recv_remaining: window,
                recv_consumed: 0,
                grant: self.shared.limits.window,
                wanted: false,
                cancel: cancel.clone(),
            },
        );
        Ok((
            ChannelRx {
                rx,
                mux: self.clone(),
                channel,
            },
            cancel,
        ))
    }

    /// 링크 자체의 틀(채널 열기 · 응답 · 핸드셰이크 뒤 제어). 채널 틀보다 먼저 나간다.
    pub(crate) fn send_control(&self, frame: &LinkFrame) {
        self.push_control(frame.encode());
    }

    /// 링크 틀 하나를 큐에 넣는다. 큐가 [`LinkLimits::control_buffer`] 를 넘으면 링크를 끊는다.
    fn push_control(&self, bytes: Vec<u8>) {
        let mut state = lock(&self.shared.state);
        if state.shut {
            return;
        }
        if state.control_bytes + bytes.len() > self.shared.limits.control_buffer {
            drop(state);
            self.shutdown(CloseReason::Protocol(
                "the peer does not read our link frames".to_owned(),
            ));
            return;
        }
        state.control_bytes += bytes.len();
        state.control.push_back(bytes);
        drop(state);
        self.shared.wake.notify_one();
    }

    /// 채널에 메시지 하나를 보낸다. 채널이 없으면 버린다. 보내기 버퍼가 상한을 넘으면 그 채널을 닫고 거짓.
    pub(crate) fn send_msg(&self, channel: u32, body: Value) -> bool {
        let bytes = LinkFrame::new(channel, Kind::Msg, body).encode();
        let mut state = lock(&self.shared.state);
        let limit = self.shared.limits.send_buffer;
        let send_budget = self.shared.limits.send_budget;
        let State {
            channels,
            ready,
            send_used,
            ..
        } = &mut *state;
        let Some(chan) = channels.get_mut(&channel) else {
            return false;
        };
        if bytes.len() > pw_wire::MAX_BODY_BYTES {
            drop(state);
            tracing::debug!(target: "pw_link", channel, "a message larger than the frame limit; closing the channel");
            self.close_channel(channel, "message larger than the frame limit");
            return false;
        }
        if !chan.out.is_empty() && chan.out_bytes + bytes.len() > limit {
            drop(state);
            tracing::debug!(target: "pw_link", channel, "send buffer full; closing the channel");
            self.close_channel(channel, "send buffer full");
            return false;
        }
        if *send_used + bytes.len() > send_budget {
            drop(state);
            tracing::debug!(target: "pw_link", channel, "link send budget exhausted; closing the channel");
            self.close_channel(channel, "link send budget exhausted");
            return false;
        }
        *send_used += bytes.len();
        chan.out_bytes += bytes.len();
        chan.out.push_back(Out {
            bytes,
            windowed: true,
        });
        let wake = enqueue(chan, ready, channel, self.shared.limits.window);
        drop(state);
        if wake {
            self.shared.wake.notify_one();
        }
        true
    }

    /// 채널을 곧바로 닫는다 — 아직 못 보낸 틀은 버리고 상대에게 `close` 를 보낸다. 받는 쪽은 끝을 본다.
    pub(crate) fn close_channel(&self, channel: u32, reason: &str) {
        let removed = lock(&self.shared.state).remove_channel(channel);
        if let Some(chan) = removed {
            chan.cancel.cancel();
            self.push_control(
                LinkFrame::new(channel, Kind::Close, json!({ "reason": reason })).encode(),
            );
        }
    }

    /// 채널을 상대에게 알리지 않고 걷는다(열기가 거절됐을 때).
    pub(crate) fn forget_channel(&self, channel: u32) {
        let removed = lock(&self.shared.state).remove_channel(channel);
        if let Some(chan) = removed {
            chan.cancel.cancel();
        }
    }

    /// 채널을 순서대로 닫는다 — 앞서 넣은 메시지를 다 보낸 뒤 `close` 가 나간다(Host 쪽 연결이 끝났을 때 마지막 응답을 잃지 않게).
    pub(crate) fn finish_channel(&self, channel: u32) {
        let mut state = lock(&self.shared.state);
        let State {
            channels, ready, ..
        } = &mut *state;
        let Some(chan) = channels.get_mut(&channel) else {
            return;
        };
        chan.out.push_back(Out {
            bytes: LinkFrame::new(channel, Kind::Close, json!({ "reason": "ended" })).encode(),
            windowed: false,
        });
        let wake = enqueue(chan, ready, channel, self.shared.limits.window);
        drop(state);
        if wake {
            self.shared.wake.notify_one();
        }
    }

    /// 받는 쪽이 `len` 바이트를 꺼내 썼다. 창의 반을 넘게 모이면 상대에게 돌려준다.
    fn consumed(&self, channel: u32, len: usize, drained: bool) {
        let mut state = lock(&self.shared.state);
        let base = self.shared.limits.window;
        let half = (base / 2).max(1);
        let State {
            channels,
            budget_used,
            ..
        } = &mut *state;
        let Some(chan) = channels.get_mut(&channel) else {
            return;
        };
        chan.recv_consumed += len;
        // 빌려 준 창(기본 창을 넘는 몫)은 그 큰 틀을 꺼냈을 때 상대에게 돌려주지 않고 예산으로 돌린다. 그 전에 꺼낸 작은 틀의
        // 창은 그대로 돌려준다 — 빌린 창이 큰 틀이 오기 전에 예산으로 새면 큰 틀이 영영 들어가지 못한다.
        if len > base && chan.grant > base {
            let back = (chan.grant - base).min(chan.recv_consumed);
            chan.recv_consumed -= back;
            chan.grant -= back;
            *budget_used -= back;
        }
        // 받아 둔 것을 다 꺼냈으면 창을 모두 돌려준다 — 보내는 쪽은 틀이 통째로 들어갈 창을 기다리므로, 반이 안 모였다고 붙잡아
        // 두면 큰 틀 하나가 영영 나가지 못한다.
        if chan.recv_consumed == 0 || (chan.recv_consumed < half && !drained) {
            return;
        }
        let bytes = std::mem::take(&mut chan.recv_consumed);
        chan.recv_remaining = chan
            .recv_remaining
            .saturating_add(i64::try_from(bytes).unwrap_or(i64::MAX));
        drop(state);
        let frame = LinkFrame::new(
            channel,
            Kind::Credit,
            json!(CreditBody {
                bytes: bytes as u64
            }),
        );
        self.push_control(frame.encode());
    }

    /// 다음에 쓸 틀: 번호 없는 링크 틀 → 이어 붙은 뒤 다시 보낼 틀 → 링크의 틀 → 창이 남은 채널을 돌아가며 하나씩. 번호 있는 틀은
    /// 내보내기 전에 다시 보낼 줄에 넣는다(이어 붙을 수 있는 링크).
    fn next_outgoing(&self, generation: u64) -> Next {
        let mut state = lock(&self.shared.state);
        if state.generation != generation || state.suspended || state.shut {
            return Next::Stale;
        }
        if let Some(frame) = state.link.pop_front() {
            return Next::Frame(frame);
        }
        if state.resend < state.replay.len() {
            let frame = state.replay[state.resend].clone();
            state.resend += 1;
            return Next::Frame(frame);
        }
        let Some(frame) = self.take_sequenced(&mut state) else {
            return Next::Idle;
        };
        if state.resumable {
            state.sent += 1;
            state.replay_bytes += frame.len();
            state.replay.push_back(frame.clone());
            state.resend = state.replay.len();
            if state.replay_bytes > self.shared.limits.send_budget {
                drop(state);
                self.shutdown(CloseReason::Protocol(
                    "the peer does not acknowledge what it receives".to_owned(),
                ));
                return Next::Stale;
            }
        }
        Next::Frame(frame)
    }

    /// 다음 번호 있는 틀: 링크의 틀이 먼저, 그 다음 창이 남은 채널을 돌아가며 하나씩.
    fn take_sequenced(&self, state: &mut State) -> Option<Vec<u8>> {
        if let Some(frame) = state.control.pop_front() {
            state.control_bytes -= frame.len();
            return Some(frame);
        }
        let State {
            channels,
            ready,
            budget_used,
            send_used,
            control,
            control_bytes,
            ..
        } = state;
        let base = self.shared.limits.window;
        while let Some(channel) = ready.pop_front() {
            let Some(chan) = channels.get_mut(&channel) else {
                continue;
            };
            let Some(front) = chan.out.front() else {
                chan.queued = false;
                continue;
            };
            if !fits(front, chan.send_window) {
                // 기본 창보다 큰 틀은 창이 돌아와도 들어가지 않는다 — 그 틀만큼 창을 청한다(한 번).
                if front.bytes.len() > base && !chan.wanted {
                    chan.wanted = true;
                    let want = LinkFrame::new(
                        channel,
                        Kind::Want,
                        json!(CreditBody {
                            bytes: front.bytes.len() as u64
                        }),
                    )
                    .encode();
                    *control_bytes += want.len();
                    control.push_back(want);
                }
                chan.queued = false;
                if let Some(frame) = control.pop_front() {
                    *control_bytes -= frame.len();
                    return Some(frame);
                }
                continue;
            }
            let out = chan.out.pop_front()?;
            chan.wanted = false;
            if out.windowed {
                chan.out_bytes -= out.bytes.len();
                *send_used -= out.bytes.len();
                chan.send_window -= i64::try_from(out.bytes.len()).unwrap_or(i64::MAX);
            } else {
                // 순서대로 닫는 틀 — 다 보냈으니 채널을 걷는다.
                if let Some(chan) = channels.remove(&channel) {
                    *budget_used -= chan.grant;
                    *send_used -= chan.out_bytes;
                    chan.cancel.cancel();
                }
                return Some(out.bytes);
            }
            if sendable(chan, base) {
                ready.push_back(channel);
            } else {
                chan.queued = false;
            }
            return Some(out.bytes);
        }
        None
    }

    /// 번호 없는 링크 틀(`ping` · `pong` · `ack`)을 낸다. 멈춘 동안에는 버린다.
    fn push_link(&self, frame: &LinkFrame) {
        let mut state = lock(&self.shared.state);
        if state.shut || state.suspended {
            return;
        }
        if state.link.len() > 64 {
            // 상대가 읽지 않으며 `ping` 을 몰아 보낸다.
            drop(state);
            self.shutdown(CloseReason::Protocol(
                "the peer does not read our link frames".to_owned(),
            ));
            return;
        }
        state.link.push_back(frame.encode());
        drop(state);
        self.shared.wake.notify_one();
    }

    async fn writer(self, mut write: FramedWrite, generation: u64, stop: CancellationToken) {
        let closed = self.closed();
        loop {
            match self.next_outgoing(generation) {
                Next::Frame(bytes) => {
                    let result = tokio::select! {
                        biased;
                        () = stop.cancelled() => return,
                        () = closed.cancelled() => return self.bye(&mut write, &stop).await,
                        result = write.write_encoded(&bytes) => result,
                    };
                    if let Err(e) = result {
                        self.transport_lost(generation, CloseReason::Io(e.to_string()));
                        return;
                    }
                }
                Next::Idle => tokio::select! {
                    biased;
                    () = stop.cancelled() => return,
                    () = closed.cancelled() => return self.bye(&mut write, &stop).await,
                    () = self.shared.wake.notified() => {}
                },
                Next::Stale => {
                    if closed.is_cancelled() {
                        self.bye(&mut write, &stop).await;
                    }
                    return;
                }
            }
        }
    }

    /// 링크를 닫았다 — 상대가 이어 붙기를 기다리지 않게 `bye` 를 보낸다(전송이 막혔으면 잠깐만 기다린다).
    async fn bye(&self, write: &mut FramedWrite, stop: &CancellationToken) {
        if stop.is_cancelled() {
            return;
        }
        let bye = LinkFrame::control(Kind::Bye, Value::Null).encode();
        let _ = tokio::time::timeout(Duration::from_secs(1), write.write_encoded(&bye)).await;
    }

    async fn reader(self, mut read: FramedRead, generation: u64, stop: CancellationToken) {
        let closed = self.closed();
        loop {
            let result = tokio::select! {
                biased;
                () = closed.cancelled() => return,
                () = stop.cancelled() => return,
                result = read.read_raw() => result,
            };
            match result {
                Ok(Some((frame, len))) => {
                    let gate = lock(&self.shared.rx_gate);
                    if !self.count(generation, frame.kind, len) {
                        return;
                    }
                    *lock(&self.shared.last_rx) = Instant::now();
                    if let Err(why) = self.dispatch(frame, len) {
                        drop(gate);
                        self.shutdown(CloseReason::Protocol(why));
                        return;
                    }
                }
                Ok(None) => {
                    self.transport_lost(generation, CloseReason::PeerClosed);
                    return;
                }
                Err(FrameError::Wire(
                    e @ (pw_wire::FrameError::Truncated | pw_wire::FrameError::Io(_)),
                )) => {
                    self.transport_lost(generation, CloseReason::Io(e.to_string()));
                    return;
                }
                Err(e) => {
                    self.shutdown(CloseReason::Protocol(e.to_string()));
                    return;
                }
            }
        }
    }

    /// 받은 틀을 센다(번호 있는 틀). 이 세대의 전송이 아니면 거짓 — 그 틀은 버린다. 창의 반만큼 받았으면 `ack` 를 낸다.
    fn count(&self, generation: u64, kind: Kind, len: usize) -> bool {
        let mut state = lock(&self.shared.state);
        if state.generation != generation || state.suspended || state.shut {
            return false;
        }
        if !sequenced(kind) {
            return true;
        }
        state.received += 1;
        state.unacked_bytes += len;
        if state.resumable && state.unacked_bytes >= (self.shared.limits.window / 2).max(1) {
            Self::queue_ack(&mut state);
            drop(state);
            self.shared.wake.notify_one();
        }
        true
    }

    fn queue_ack(state: &mut State) {
        state.unacked_bytes = 0;
        state.acked = state.received;
        let ack = LinkFrame::control(
            Kind::Ack,
            json!(AckBody {
                seq: state.received
            }),
        );
        state.link.push_back(ack.encode());
    }

    /// 상대가 `seq` 번까지 받았다 — 다시 보낼 줄에서 뺀다.
    fn on_ack(&self, seq: u64) -> Result<(), String> {
        let mut state = lock(&self.shared.state);
        if seq > state.sent {
            return Err(format!("ack {seq} past the {} frames sent", state.sent));
        }
        let first = state.sent - state.replay.len() as u64;
        for _ in first..seq.max(first) {
            if let Some(frame) = state.replay.pop_front() {
                state.replay_bytes -= frame.len();
                state.resend = state.resend.saturating_sub(1);
            }
        }
        Ok(())
    }

    fn dispatch(&self, raw: RawFrame, len: usize) -> Result<(), String> {
        // 채널 메시지는 풀지 않고 받아 둔다.
        if raw.kind == Kind::Msg && raw.channel != CONTROL {
            let body = match raw.body {
                Some(body) => body,
                None => RawValue::from_string("null".to_owned()).map_err(|e| e.to_string())?,
            };
            self.on_msg(raw.channel, body, len);
            return Ok(());
        }
        let frame = raw.into_frame()?;
        match frame.kind {
            Kind::Ping => {
                self.push_link(&LinkFrame::control(Kind::Pong, Value::Null));
            }
            Kind::Pong => {}
            Kind::Ack => {
                let ack = serde_json::from_value::<AckBody>(frame.body)
                    .map_err(|e| format!("bad ack: {e}"))?;
                self.on_ack(ack.seq)?;
            }
            Kind::Bye => {
                self.shutdown(CloseReason::PeerClosed);
            }
            Kind::Msg => {
                return Err("a message on the link channel".to_owned());
            }
            Kind::Credit => {
                let credit = serde_json::from_value::<CreditBody>(frame.body)
                    .map_err(|e| format!("bad credit: {e}"))?;
                self.on_credit(frame.channel, credit.bytes);
            }
            Kind::Want => {
                let want = serde_json::from_value::<CreditBody>(frame.body)
                    .map_err(|e| format!("bad want: {e}"))?;
                self.on_want(frame.channel, want.bytes);
            }
            Kind::Close => {
                let removed = lock(&self.shared.state).remove_channel(frame.channel);
                if let Some(chan) = removed {
                    chan.cancel.cancel();
                }
            }
            _ => {
                let _ = self.shared.inbox.send(frame);
            }
        }
        Ok(())
    }

    fn on_msg(&self, channel: u32, body: Box<RawValue>, len: usize) {
        let mut state = lock(&self.shared.state);
        let Some(chan) = state.channels.get_mut(&channel) else {
            // 이미 닫은 채널에 오던 것.
            return;
        };
        // 틀은 남은 창에 통째로 들어가야 한다(보내는 쪽이 그렇게 보낸다) — 남은 창 1바이트에 큰 틀 하나를 받지 않는다.
        if i64::try_from(len).unwrap_or(i64::MAX) > chan.recv_remaining {
            drop(state);
            tracing::debug!(target: "pw_link", channel, "the peer overran the window; closing the channel");
            self.close_channel(channel, "flow control violated");
            return;
        }
        chan.recv_remaining -= i64::try_from(len).unwrap_or(i64::MAX);
        if chan.inbound.send((body, len)).is_err() {
            // 받는 쪽이 사라졌다.
            drop(state);
            self.close_channel(channel, "receiver gone");
        }
    }

    /// 상대가 기본 창보다 큰 틀 하나(`bytes`)를 보내려고 창을 청했다. 링크 받기 예산이 남으면 모자란 만큼 빌려 주고, 모자라면 그
    /// 채널을 닫는다. 틀 상한을 넘는 청은 받지 않는다(그 틀은 어차피 받지 않는다).
    fn on_want(&self, channel: u32, bytes: u64) {
        let mut state = lock(&self.shared.state);
        let budget = self.shared.limits.receive_budget;
        let State {
            channels,
            budget_used,
            ..
        } = &mut *state;
        let Some(chan) = channels.get_mut(&channel) else {
            return;
        };
        let wanted = usize::try_from(bytes).unwrap_or(usize::MAX);
        // 꺼냈지만 아직 돌려주지 않은 창도 함께 돌려준다.
        let flushed = std::mem::take(&mut chan.recv_consumed);
        let remaining = usize::try_from(chan.recv_remaining.max(0)).unwrap_or(0) + flushed;
        let need = wanted.saturating_sub(remaining);
        let refuse = wanted > pw_wire::MAX_BODY_BYTES || *budget_used + need > budget;
        if refuse {
            chan.recv_consumed = flushed;
            drop(state);
            tracing::debug!(target: "pw_link", channel, "cannot lend the window for a big frame; closing the channel");
            self.close_channel(channel, "receive budget exhausted");
            return;
        }
        chan.grant += need;
        *budget_used += need;
        let credit = flushed + need;
        chan.recv_remaining = chan
            .recv_remaining
            .saturating_add(i64::try_from(credit).unwrap_or(i64::MAX));
        drop(state);
        if credit > 0 {
            let frame = LinkFrame::new(
                channel,
                Kind::Credit,
                json!(CreditBody {
                    bytes: credit as u64
                }),
            );
            self.push_control(frame.encode());
        }
    }

    fn on_credit(&self, channel: u32, bytes: u64) {
        let mut state = lock(&self.shared.state);
        let State {
            channels, ready, ..
        } = &mut *state;
        let Some(chan) = channels.get_mut(&channel) else {
            return;
        };
        chan.send_window = chan
            .send_window
            .saturating_add(i64::try_from(bytes).unwrap_or(i64::MAX));
        let wake = enqueue(chan, ready, channel, self.shared.limits.window);
        drop(state);
        if wake {
            self.shared.wake.notify_one();
        }
    }

    async fn heartbeat(self, generation: u64, stop: CancellationToken) {
        let closed = self.closed();
        let limits = self.shared.limits;
        let mut tick =
            tokio::time::interval_at(Instant::now() + limits.ping_every, limits.ping_every);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                () = closed.cancelled() => return,
                () = stop.cancelled() => return,
                _ = tick.tick() => {}
            }
            let silent = lock(&self.shared.last_rx).elapsed();
            if silent >= limits.dead_after {
                self.transport_lost(generation, CloseReason::HeartbeatTimeout);
                return;
            }
            self.push_link(&LinkFrame::control(Kind::Ping, Value::Null));
            // 창의 반을 채우지 못한 채 받은 것도 하트비트마다 알린다 — 다시 보낼 줄이 비게.
            let mut state = lock(&self.shared.state);
            if state.resumable && !state.suspended && state.received > state.acked {
                Self::queue_ack(&mut state);
                drop(state);
                self.shared.wake.notify_one();
            }
        }
    }
}

/// 쓰는 쪽이 할 일이 있는가: 맨 앞 틀이 창에 들어가거나(닫는 틀 포함), 기본 창(`base`)보다 커서 아직 `want` 를 보내지 않았다.
fn sendable(chan: &Chan, base: usize) -> bool {
    chan.out.front().is_some_and(|front| {
        fits(front, chan.send_window)
            || (front.windowed && front.bytes.len() > base && !chan.wanted)
    })
}

/// 틀이 남은 창에 통째로 들어가는가(닫는 틀은 창과 무관하다).
fn fits(out: &Out, window: i64) -> bool {
    !out.windowed || i64::try_from(out.bytes.len()).unwrap_or(i64::MAX) <= window
}

/// 보낼 수 있으면 `ready` 줄에 넣는다. 새로 넣었으면 참(쓰는 쪽을 깨운다).
fn enqueue(chan: &mut Chan, ready: &mut VecDeque<u32>, channel: u32, base: usize) -> bool {
    if chan.queued || !sendable(chan, base) {
        return false;
    }
    chan.queued = true;
    ready.push_back(channel);
    true
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::value::RawValue;
    use serde_json::{Value, json};
    use tokio::sync::mpsc;

    use super::{Chan, LinkLimits, Mux};
    use crate::frame::{Framed, Kind, LinkFrame};
    use crate::transport::LinkStream;

    fn framed_pair() -> (Framed, Framed) {
        let (a, b) = tokio::io::duplex(1 << 20);
        (
            Framed::new(LinkStream::split(a)),
            Framed::new(LinkStream::split(b)),
        )
    }

    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..500 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{what}");
    }

    /// 받아 둔 메시지는 원시 JSON 바이트로 남는다 — 받기 예산(와이어 바이트)이 실제 메모리 상한이 되게(델타 리뷰 권고 1). 큐의 원소
    /// 타입을 푼 `Value` 로 되돌리면 이 시험이 컴파일되지 않는다.
    #[tokio::test]
    async fn held_messages_stay_as_raw_bytes() {
        fn held(chan: &Chan) -> &mpsc::UnboundedSender<(Box<RawValue>, usize)> {
            &chan.inbound
        }
        let (a, b) = framed_pair();
        let (a, _) = Mux::start(a, LinkLimits::default());
        let (b, _) = Mux::start(b, LinkLimits::default());
        let (mut rx, _) = a.add_channel(1).unwrap();
        b.add_channel(1).unwrap();
        // 원소마다 2 바이트인 배열 — 풀면 원소마다 수십 바이트가 된다.
        let zeros = Value::Array(vec![json!(0); 10_000]);
        let wire = serde_json::to_string(&zeros).unwrap();
        assert!(b.send_msg(1, zeros.clone()));
        let (raw, len) = rx.rx.recv().await.unwrap();
        assert_eq!(raw.get(), wire);
        assert!(len >= wire.len() && len < wire.len() + 64, "{len}");
        assert!(
            lock(&a)
                .channels
                .get(&1)
                .is_some_and(|c| !held(c).is_closed())
        );
    }

    fn lock(mux: &Mux) -> std::sync::MutexGuard<'_, super::State> {
        super::lock(&mux.shared.state)
    }

    /// 순서대로 닫은 채널(Host 연결이 끝나 마지막 응답 뒤 `close`)도 그 창을 예산으로 돌린다 — 새면 요청 몇 번 뒤 그 링크의 채널
    /// 열기가 모두 거절된다(델타 리뷰 권고 2).
    #[tokio::test]
    async fn finished_channels_return_their_window_to_the_budget() {
        let limits = LinkLimits {
            window: 64 * 1024,
            receive_budget: 192 * 1024,
            ..LinkLimits::default()
        };
        let (a, b) = framed_pair();
        let (a, _) = Mux::start(a, limits);
        let (b, _) = Mux::start(b, limits);
        for channel in 1..=10 {
            let _rx = a
                .add_channel(channel)
                .unwrap_or_else(|e| panic!("channel {channel}: {e:?}"));
            let _peer = b.add_channel(channel).unwrap();
            assert!(a.send_msg(channel, json!({"last": channel})));
            a.finish_channel(channel);
            until("the finished channel stayed open", || {
                a.channel_count() == 0
            })
            .await;
            until("the peer kept the channel", || b.channel_count() == 0).await;
        }
        assert_eq!(lock(&a).budget_used, 0);
        assert_eq!(lock(&a).send_used, 0);
    }

    /// 틀 상한을 넘는 `want` 는 예산이 남아도 빌려 주지 않고 그 채널을 닫는다.
    #[tokio::test]
    async fn a_want_past_the_frame_limit_closes_the_channel() {
        let (a, mut peer) = framed_pair();
        let (a, _) = Mux::start(a, LinkLimits::default());
        let _rx = a.add_channel(1).unwrap();
        let want = json!({"bytes": pw_wire::MAX_BODY_BYTES + 1});
        peer.write
            .write(&LinkFrame::new(1, Kind::Want, want))
            .await
            .unwrap();
        loop {
            let (frame, _) = peer.read.read().await.unwrap().expect("link closed");
            if frame.kind == Kind::Credit {
                panic!("lent a window for a frame past the limit");
            }
            if frame.kind == Kind::Close {
                assert_eq!(frame.channel, 1);
                break;
            }
        }
        assert_eq!(a.channel_count(), 0);
    }

    /// 창이 없어 못 보낸 메시지의 합은 링크 보내기 예산을 넘지 않는다 — `want` 에 답하지 않는 상대에게 채널마다 큰 틀이 쌓이지
    /// 않게(델타 리뷰 권고 4). 넘긴 메시지의 채널만 닫힌다.
    #[tokio::test]
    async fn queued_messages_stay_within_the_link_send_budget() {
        let limits = LinkLimits {
            window: 16 * 1024,
            send_budget: 100 * 1024,
            ..LinkLimits::default()
        };
        // 상대는 읽기만 하고 창을 돌려주지도 `want` 에 답하지도 않는다.
        let (a, mut peer) = framed_pair();
        let (a, _) = Mux::start(a, limits);
        tokio::spawn(async move { while let Ok(Some(_)) = peer.read.read().await {} });
        let _one = a.add_channel(1).unwrap();
        let _two = a.add_channel(2).unwrap();
        let big = json!("x".repeat(60 * 1024));
        assert!(a.send_msg(1, big.clone()));
        assert!(
            !a.send_msg(2, big),
            "the link held more than its send budget"
        );
        assert!(a.has_channel(1));
        assert!(!a.has_channel(2));
        assert!(lock(&a).send_used <= limits.send_budget);
    }
}
