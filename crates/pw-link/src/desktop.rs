//! Desktop 쪽 끝: 링크를 열고([`dial`]) 그 위에 `pw_core::Host` 를 세운다([`RemoteHost`] — 18-m7-plan M7-03 ⑤ ⑦).

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pw_core::{BoxFuture, Connection, FrameSink, FrameSource, Host, HostError, HostId};
use pw_rpc::{Caller, Role};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

use crate::claim::CallerClaim;
use crate::device::{DeviceKey, MachineId, PublicKey, random_challenge};
use crate::frame::{CONTROL, Framed, Kind, LinkFrame};
use crate::handshake::{
    Auth, Challenge, Hello, PROTOCOL, Reject, RejectReason, Signer, Versions, Welcome, transcript,
};
use crate::mux::{AddError, ChannelRx, CloseReason, LinkLimits, Mux};
use crate::transport::{LinkStream, Transport};

/// 이 Desktop 의 기기 키와 판.
pub struct DesktopIdentity {
    pub key: Arc<DeviceKey>,
    pub versions: Versions,
}

/// 붙을 Relay 의 머신 id 를 아는가(메인 판정 M7-P6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// 그 호스트 항목에 기억한 머신 id. 다른 키의 Relay 에는 붙지 않는다.
    Machine(MachineId),
    /// 처음 붙는 호스트 — 본 id 를 돌려주고 부르는 쪽(M7-08)이 기억한다. SSH 로 처음 설치할 때(M7-06)만 쓴다.
    FirstContact,
}

/// 링크를 열지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialError {
    Io(String),
    /// 핸드셰이크가 시한([`LinkLimits::dead_after`]) 안에 끝나지 않았다.
    Timeout,
    /// 상대가 링크 규칙을 어겼다.
    Protocol(String),
    /// 판이 다르다. 상대가 판을 알려 주지 않았으면 `theirs` 가 없다.
    VersionMismatch {
        ours: Versions,
        theirs: Option<Versions>,
    },
    /// 기억한 머신 id 와 다른 키의 Relay(SSH 호스트 키 변경과 같은 취급 — 사람에게 보인다). `found` 는 서명을 확인한 키의 지문이다.
    MachineMismatch {
        expected: MachineId,
        found: MachineId,
    },
    /// Relay 의 서명이 그 키 · 이 연결의 도전값과 맞지 않는다.
    BadRelaySignature,
    /// Relay 가 거절했다(서명 · 페어링).
    Rejected(RejectReason),
}

impl fmt::Display for DialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot open the link: {e}"),
            Self::Timeout => f.write_str("the link handshake timed out"),
            Self::Protocol(e) => write!(f, "link protocol error: {e}"),
            Self::VersionMismatch { ours, theirs } => match theirs {
                Some(theirs) => write!(
                    f,
                    "version mismatch: ours {}, theirs {}",
                    ours.app, theirs.app
                ),
                None => write!(f, "version mismatch: ours {}", ours.app),
            },
            Self::MachineMismatch { expected, found } => {
                write!(f, "machine id changed: expected {expected}, found {found}")
            }
            Self::BadRelaySignature => f.write_str("the relay did not prove its key"),
            Self::Rejected(reason) => write!(f, "the relay rejected this device: {reason:?}"),
        }
    }
}

impl std::error::Error for DialError {}

/// 링크 상태(18-m7-plan M7-03 ⑦). M7-08 이 `remote.*` 로 창에 보인다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LinkState {
    /// 아직 붙지 않았다.
    Idle,
    Connecting,
    Connected {
        machine: MachineId,
        role: Role,
    },
    /// 이어 붙는 중(M7-09 가 쓴다).
    Reconnecting,
    Disconnected {
        reason: CloseReason,
    },
    VersionMismatch {
        ours: Versions,
        theirs: Option<Versions>,
    },
    MachineMismatch {
        expected: MachineId,
        found: MachineId,
    },
}

/// Desktop 쪽 링크. 버리면 닫힌다.
pub struct DesktopLink {
    mux: Mux,
    role: Role,
    machine: MachineId,
    relay_key: PublicKey,
    next_channel: AtomicU32,
    pending: Pending,
}

/// 답을 기다리는 채널 열기.
enum Slot {
    Waiting(oneshot::Sender<Result<(), String>>),
    /// 답 전에 버려진 열기 — 채널은 닫았고, 늦게 온 답은 버린다.
    Abandoned,
}

type Pending = Arc<Mutex<HashMap<u32, Slot>>>;

fn take_pending(pending: &Pending, channel: u32) -> Option<Slot> {
    pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&channel)
}

/// 열기가 답 전에 버려지면(부른 쪽의 시한 · 취소) 채널을 닫는다 — 그러지 않으면 채널과 Relay 의 Host 연결이 링크가 죽을 때까지
/// 남아 채널 상한을 채운다.
struct OpenGuard<'a> {
    mux: &'a Mux,
    pending: &'a Pending,
    channel: u32,
    armed: bool,
}

impl Drop for OpenGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(self.channel, Slot::Abandoned);
        self.mux.close_channel(self.channel, "abandoned");
    }
}

/// 전송 위에서 핸드셰이크를 하고 링크를 연다. 판 · 머신 id · Relay 서명이 맞지 않으면 서명하지 않고 끊는다.
pub async fn dial(
    stream: LinkStream,
    me: &DesktopIdentity,
    expect: &Expect,
    limits: LinkLimits,
) -> Result<DesktopLink, DialError> {
    // Relay 를 증명하기 전에는 작은 틀만 받는다(Relay 쪽과 같은 규칙 — `HANDSHAKE_MAX_FRAME`).
    let mut framed = Framed::handshake(stream);
    let (role, relay_key) =
        tokio::time::timeout(limits.dead_after, handshake(&mut framed, me, expect))
            .await
            .map_err(|_| DialError::Timeout)??;
    framed.read.set_max_body(pw_wire::MAX_BODY_BYTES);
    let (mux, mut inbox) = Mux::start(framed, limits);
    let pending: Pending = Arc::default();
    {
        let (mux, pending) = (mux.clone(), Arc::clone(&pending));
        tokio::spawn(async move {
            while let Some(frame) = inbox.recv().await {
                let outcome = match frame.kind {
                    Kind::Accepted => Ok(()),
                    Kind::Refused => Err(frame.body["reason"]
                        .as_str()
                        .unwrap_or("refused")
                        .to_owned()),
                    other => {
                        mux.shutdown(CloseReason::Protocol(format!("unexpected {other:?} frame")));
                        return;
                    }
                };
                match take_pending(&pending, frame.channel) {
                    Some(Slot::Waiting(tx)) => {
                        let _ = tx.send(outcome);
                    }
                    Some(Slot::Abandoned) => {}
                    None => {
                        mux.shutdown(CloseReason::Protocol(
                            "an answer for no open request".to_owned(),
                        ));
                        return;
                    }
                }
            }
        });
    }
    Ok(DesktopLink {
        mux,
        role,
        machine: relay_key.id(),
        relay_key,
        next_channel: AtomicU32::new(1),
        pending,
    })
}

async fn read_control(framed: &mut Framed) -> Result<LinkFrame, DialError> {
    match framed.read.read().await {
        Ok(Some((frame, _))) if frame.channel == CONTROL => Ok(frame),
        Ok(Some((frame, _))) => Err(DialError::Protocol(format!(
            "a {:?} frame on channel {} during the handshake",
            frame.kind, frame.channel
        ))),
        Ok(None) => Err(DialError::Io("the relay closed the link".to_owned())),
        Err(e) => Err(DialError::Protocol(e.to_string())),
    }
}

fn rejected(frame: &LinkFrame, ours: &Versions) -> DialError {
    match serde_json::from_value::<Reject>(frame.body.clone()) {
        Ok(Reject {
            reason: RejectReason::Version,
            versions,
        }) => DialError::VersionMismatch {
            ours: ours.clone(),
            theirs: versions,
        },
        Ok(reject) => DialError::Rejected(reject.reason),
        Err(e) => DialError::Protocol(format!("bad reject: {e}")),
    }
}

async fn handshake(
    framed: &mut Framed,
    me: &DesktopIdentity,
    expect: &Expect,
) -> Result<(Role, PublicKey), DialError> {
    let io = |e: std::io::Error| DialError::Io(e.to_string());
    let ours = &me.versions;
    let my_key = me.key.public();
    let my_challenge = Challenge(random_challenge().map_err(io)?);
    let hello = Hello {
        versions: ours.clone(),
        device: my_key.id(),
        key: my_key,
        challenge: my_challenge,
        signature: None,
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Hello, json!(hello)))
        .await
        .map_err(io)?;

    let frame = read_control(framed).await?;
    let theirs = match frame.kind {
        Kind::Hello => serde_json::from_value::<Hello>(frame.body)
            .map_err(|e| DialError::Protocol(format!("bad hello: {e}")))?,
        Kind::Reject => return Err(rejected(&frame, ours)),
        other => {
            return Err(DialError::Protocol(format!(
                "expected hello, got {other:?}"
            )));
        }
    };
    if theirs.versions != *ours {
        return Err(DialError::VersionMismatch {
            ours: ours.clone(),
            theirs: Some(theirs.versions),
        });
    }
    let found = theirs.key.id();
    if theirs.device != found {
        return Err(DialError::Protocol(
            "the relay id is not its key".to_owned(),
        ));
    }
    let relay_text = transcript(
        Signer::Relay,
        PROTOCOL,
        &my_challenge,
        &theirs.challenge,
        &my_key,
        &theirs.key,
    );
    let proven = theirs
        .signature
        .as_ref()
        .is_some_and(|sig| theirs.key.verifies(&relay_text, sig));
    if !proven {
        return Err(DialError::BadRelaySignature);
    }
    // 머신 id(메인 판정 M7-P6): 그 호스트에 기억한 id 와 다른 키의 Relay 에는 서명하지 않는다. 비교는 Relay 의 서명을 확인한
    // **뒤**다 — 그래서 `MachineMismatch.found` 는 증명된 키의 지문이다(서명 없는 hello 의 키가 「새로 믿을」 id 가 되지 않게 —
    // M7-08 델타 리뷰 F-a). Relay 의 서명을 보는 것은 이 Desktop 이 아무것도 서명하지 않고 할 수 있다.
    if let Expect::Machine(expected) = expect
        && *expected != found
    {
        return Err(DialError::MachineMismatch {
            expected: expected.clone(),
            found,
        });
    }

    // 서명 글에 이 Relay 의 키를 넣는다(채널 바인딩 — 다른 Relay 가 이 서명을 옮겨 쓰지 못한다).
    let my_text = transcript(
        Signer::Desktop,
        PROTOCOL,
        &my_challenge,
        &theirs.challenge,
        &my_key,
        &theirs.key,
    );
    let auth = Auth {
        relay_key: theirs.key,
        signature: me.key.sign(&my_text),
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Auth, json!(auth)))
        .await
        .map_err(io)?;

    let frame = read_control(framed).await?;
    match frame.kind {
        Kind::Welcome => {
            let welcome = serde_json::from_value::<Welcome>(frame.body)
                .map_err(|e| DialError::Protocol(format!("bad welcome: {e}")))?;
            Ok((welcome.role, theirs.key))
        }
        Kind::Reject => Err(rejected(&frame, ours)),
        other => Err(DialError::Protocol(format!(
            "expected welcome, got {other:?}"
        ))),
    }
}

impl DesktopLink {
    /// Relay 가 이 기기에 준 role(페어링 표).
    pub fn role(&self) -> Role {
        self.role
    }

    /// 증명된 머신 id(Relay 기기 공개키의 지문).
    pub fn machine(&self) -> &MachineId {
        &self.machine
    }

    pub fn relay_key(&self) -> PublicKey {
        self.relay_key
    }

    /// 열린 채널 수.
    pub fn channel_count(&self) -> usize {
        self.mux.channel_count()
    }

    /// 링크를 닫는다.
    pub fn close(&self) {
        self.mux.shutdown(CloseReason::Local);
    }

    /// 링크가 닫힐 때까지 기다리고 그 까닭.
    pub async fn closed(&self) -> CloseReason {
        self.mux.closed().cancelled().await;
        self.mux.reason().unwrap_or(CloseReason::Local)
    }

    /// 채널 하나를 연다(`Host::connect` 하나). Relay 는 호출자의 주장을 링크 기기의 role 로 자른다(M7-02 ②).
    pub async fn open(&self, caller: &Caller) -> Result<Connection, HostError> {
        let disconnected = || HostError::Disconnected(self.machine.to_string());
        let channel = self.next_channel.fetch_add(1, Ordering::Relaxed);
        if channel == CONTROL || channel == u32::MAX {
            return Err(HostError::Refused(
                "no more channel ids on this link".to_owned(),
            ));
        }
        let (rx, cancel) = self.mux.add_channel(channel).map_err(|e| match e {
            AddError::Closed => disconnected(),
            AddError::Exists => HostError::Refused("channel id in use".to_owned()),
            AddError::Budget => {
                HostError::Refused("the link's receive budget is exhausted".to_owned())
            }
        })?;
        let (tx, answer) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(channel, Slot::Waiting(tx));
        let mut guard = OpenGuard {
            mux: &self.mux,
            pending: &self.pending,
            channel,
            armed: true,
        };
        self.mux.send_control(&LinkFrame::new(
            channel,
            Kind::Connect,
            json!(CallerClaim::from(caller)),
        ));
        let closed = self.mux.closed();
        let outcome = tokio::select! {
            outcome = answer => outcome.unwrap_or_else(|_| Err("link closed".to_owned())),
            () = closed.cancelled() => Err("link closed".to_owned()),
        };
        guard.armed = false;
        if let Err(why) = outcome {
            take_pending(&self.pending, channel);
            self.mux.forget_channel(channel);
            return Err(if self.mux.reason().is_some() {
                disconnected()
            } else {
                HostError::Refused(why)
            });
        }
        if cancel.is_cancelled() {
            return Err(disconnected());
        }
        Ok(Connection {
            sink: Box::new(ChannelSink {
                mux: self.mux.clone(),
                channel,
            }),
            source: Box::new(ChannelSource(rx)),
        })
    }
}

impl Drop for DesktopLink {
    fn drop(&mut self) {
        self.mux.shutdown(CloseReason::Local);
    }
}

/// 채널로 보내는 쪽. 버리면 채널을 닫는다 — Relay 의 Host 가 그 연결을 닫는다.
struct ChannelSink {
    mux: Mux,
    channel: u32,
}

impl FrameSink for ChannelSink {
    fn send(&self, frame: Value) {
        self.mux.send_msg(self.channel, frame);
    }
}

impl Drop for ChannelSink {
    fn drop(&mut self) {
        self.mux.close_channel(self.channel, "closed");
    }
}

struct ChannelSource(ChannelRx);

impl FrameSource for ChannelSource {
    fn recv(&mut self) -> BoxFuture<'_, Option<Value>> {
        Box::pin(self.0.recv())
    }
}

/// 원격 머신의 Host(18-m7-plan M7-03 ⑤). `HostRouter` 에 붙고, 그 프로젝트는 `Target::Member` 로 고른다. 링크는
/// [`RemoteHost::attach`] 가 열고(M7-08 이 부른다), 링크가 없으면 연결은 [`HostError::Disconnected`] 다.
pub struct RemoteHost {
    id: HostId,
    transport: Arc<dyn Transport>,
    identity: Arc<DesktopIdentity>,
    limits: LinkLimits,
    expect: Mutex<Expect>,
    link: Arc<Mutex<Option<Arc<DesktopLink>>>>,
    state: Arc<watch::Sender<LinkState>>,
}

impl RemoteHost {
    pub fn new(
        id: impl Into<HostId>,
        transport: Arc<dyn Transport>,
        identity: Arc<DesktopIdentity>,
        expect: Expect,
        limits: LinkLimits,
    ) -> Self {
        Self {
            id: id.into(),
            transport,
            identity,
            limits,
            expect: Mutex::new(expect),
            link: Arc::default(),
            state: Arc::new(watch::Sender::new(LinkState::Idle)),
        }
    }

    /// 링크 상태. 바뀔 때마다 깬다.
    pub fn state(&self) -> watch::Receiver<LinkState> {
        self.state.subscribe()
    }

    /// 지금 링크(붙어 있을 때).
    pub fn link(&self) -> Option<Arc<DesktopLink>> {
        self.link
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 전송을 열고 링크를 세운다. 처음 붙는 호스트면([`Expect::FirstContact`]) 본 머신 id 를 그 뒤로 기대한다 — 돌려준 id 를
    /// 부르는 쪽이 기억한다. 이미 붙어 있으면 옛 링크를 닫고 새로 붙는다.
    pub async fn attach(&self) -> Result<MachineId, DialError> {
        self.detach();
        self.state.send_replace(LinkState::Connecting);
        let expect = self
            .expect
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let result = async {
            let stream = self
                .transport
                .open()
                .await
                .map_err(|e| DialError::Io(e.to_string()))?;
            dial(stream, &self.identity, &expect, self.limits).await
        }
        .await;
        let link = match result {
            Ok(link) => Arc::new(link),
            Err(e) => {
                tracing::info!(target: "pw_link", host = %self.id, error = %e, "cannot attach the remote host");
                self.state.send_replace(match &e {
                    DialError::VersionMismatch { ours, theirs } => LinkState::VersionMismatch {
                        ours: ours.clone(),
                        theirs: theirs.clone(),
                    },
                    DialError::MachineMismatch { expected, found } => LinkState::MachineMismatch {
                        expected: expected.clone(),
                        found: found.clone(),
                    },
                    other => LinkState::Disconnected {
                        reason: CloseReason::Handshake(other.to_string()),
                    },
                });
                return Err(e);
            }
        };
        let machine = link.machine().clone();
        *self.expect.lock().unwrap_or_else(PoisonError::into_inner) =
            Expect::Machine(machine.clone());
        *self.link.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&link));
        self.state.send_replace(LinkState::Connected {
            machine: machine.clone(),
            role: link.role(),
        });
        let (slot, state) = (Arc::clone(&self.link), Arc::clone(&self.state));
        let watched = Arc::downgrade(&link);
        let closed = link.mux.closed();
        tokio::spawn(async move {
            closed.cancelled().await;
            let Some(link) = watched.upgrade() else {
                return;
            };
            let reason = link.mux.reason().unwrap_or(CloseReason::Local);
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            // 그 사이에 새 링크로 바뀌었으면 상태를 건드리지 않는다.
            if slot.as_ref().is_some_and(|now| Arc::ptr_eq(now, &link)) {
                *slot = None;
                state.send_replace(LinkState::Disconnected { reason });
            }
        });
        Ok(machine)
    }

    /// 링크를 닫는다.
    pub fn detach(&self) {
        let old = self
            .link
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(old) = old {
            old.close();
            self.state.send_replace(LinkState::Disconnected {
                reason: CloseReason::Local,
            });
        }
    }
}

impl Host for RemoteHost {
    fn id(&self) -> &str {
        &self.id
    }

    fn connect(&self, caller: Caller) -> BoxFuture<'_, Result<Connection, HostError>> {
        Box::pin(async move {
            let link = self
                .link()
                .ok_or_else(|| HostError::Disconnected(self.id.clone()))?;
            link.open(&caller).await
        })
    }

    fn is_remote(&self) -> bool {
        true
    }
}
