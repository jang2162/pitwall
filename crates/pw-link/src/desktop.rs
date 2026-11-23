//! Desktop 쪽 끝: 링크를 열고([`dial`]) 그 위에 `pw_core::Host` 를 세운다([`RemoteHost`] — 18-m7-plan M7-03 ⑤ ⑦).

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pw_core::{BoxFuture, Connection, FrameSink, FrameSource, Host, HostError, HostId};
use pw_rpc::{Caller, Role};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::claim::CallerClaim;
use crate::device::{DeviceKey, MachineId, PublicKey, random_challenge};
use crate::forward::ForwardTo;
use crate::frame::{CONTROL, Framed, Kind, LinkFrame};
use crate::handshake::{
    Auth, Challenge, Hello, PROTOCOL, Reject, RejectReason, Resume, ResumeToken, Signer, Versions,
    Welcome, transcript,
};
use crate::mux::{AddError, ChannelRx, CloseReason, LinkLimits, Mux, Phase};
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
    /// 전송을 잃고 이어 붙는 중(18-m7-plan M7-09 ①) — 채널 · 스트림은 그대로다. 시한([`LinkLimits::resume_within`]) 안에 붙으면
    /// `Connected`, 못 붙으면 `Disconnected`.
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
    /// 다음 이어 붙기 토큰(Relay 가 주지 않았으면 이어 붙지 않는다).
    token: Mutex<Option<ResumeToken>>,
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
    let (role, relay_key, welcome) =
        tokio::time::timeout(limits.dead_after, handshake(&mut framed, me, expect, None))
            .await
            .map_err(|_| DialError::Timeout)??;
    if welcome.resumed.is_some() {
        return Err(DialError::Protocol(
            "the relay resumed a link this desktop did not ask for".to_owned(),
        ));
    }
    framed.read.set_max_body(pw_wire::MAX_BODY_BYTES);
    let (mux, mut inbox) = if welcome.token.is_some() {
        Mux::start_resumable(framed, limits)
    } else {
        Mux::start(framed, limits)
    };
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
        token: Mutex::new(welcome.token),
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

pub(crate) async fn handshake(
    framed: &mut Framed,
    me: &DesktopIdentity,
    expect: &Expect,
    resume: Option<Resume>,
) -> Result<(Role, PublicKey, Welcome), DialError> {
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
    // 이어 붙기 토큰은 이 기기 키의 서명과 함께만 보낸다(Relay 도 서명을 본 뒤에만 읽는다).
    let auth = Auth {
        relay_key: theirs.key,
        signature: me.key.sign(&my_text),
        resume,
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
            Ok((welcome.role, theirs.key, welcome))
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

    /// 지금의 이어 붙기 토큰(시험).
    #[cfg(test)]
    pub(crate) fn resume_token(&self) -> Option<ResumeToken> {
        self.token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 전송을 잃고 이어 붙기를 기다리는가.
    pub fn is_suspended(&self) -> bool {
        *self.mux.phase().borrow() == Phase::Suspended
    }

    /// 새 전송으로 이 링크를 이어 붙는다(18-m7-plan M7-09 ①). 같은 Relay 키 · role 이어야 하고, Relay 가 토큰을 받지 않으면
    /// [`DialError::Rejected`]`(Resume)` — 그때 부른 쪽이 링크를 닫는다. 전송을 잃어 멈춰 있을 때만 부른다.
    pub async fn resume(&self, stream: LinkStream, me: &DesktopIdentity) -> Result<(), DialError> {
        let token = self
            .token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or(DialError::Rejected(RejectReason::Resume))?;
        let received = self.mux.received();
        let limits = *self.mux.limits();
        let mut framed = Framed::handshake(stream);
        let expect = Expect::Machine(self.machine.clone());
        let resume = Resume { token, received };
        let (role, _, welcome) = tokio::time::timeout(
            limits.dead_after,
            handshake(&mut framed, me, &expect, Some(resume)),
        )
        .await
        .map_err(|_| DialError::Timeout)??;
        let (Some(relay_received), Some(next)) = (welcome.resumed, welcome.token) else {
            return Err(DialError::Protocol(
                "the relay answered a resume with a new link".to_owned(),
            ));
        };
        if role != self.role {
            return Err(DialError::Rejected(RejectReason::Resume));
        }
        *self.token.lock().unwrap_or_else(PoisonError::into_inner) = Some(next);
        framed.read.set_max_body(pw_wire::MAX_BODY_BYTES);
        self.mux
            .resume(framed, relay_received)
            .map_err(DialError::Protocol)
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
        let (channel, rx, _cancel) = self
            .open_channel(Kind::Connect, json!(CallerClaim::from(caller)))
            .await?;
        Ok(Connection {
            sink: Box::new(ChannelSink {
                mux: self.mux.clone(),
                channel,
            }),
            source: Box::new(ChannelSource(rx)),
        })
    }

    /// 포트 포워딩 채널 하나를 열고(Relay 가 그 머신에서 `to` 로 TCP 를 연다 — 18-m7-plan M7-22) `tcp` 를 그 채널에 잇는다. 연결이
    /// 끝날 때까지 돈다. owner 링크가 아니면 열지 않는다(Relay 도 거절한다 — 메인 판정 M7-P15).
    pub async fn forward(&self, to: &ForwardTo, tcp: TcpStream) -> Result<(), HostError> {
        if self.role != Role::Owner {
            return Err(HostError::Refused(
                "port forwarding needs an owner device".to_owned(),
            ));
        }
        to.check().map_err(HostError::Refused)?;
        let (channel, rx, cancel) = self.open_channel(Kind::Forward, json!(to)).await?;
        crate::forward::pump(self.mux.clone(), channel, rx, cancel, tcp).await;
        Ok(())
    }

    /// 채널을 열고(`connect` · `forward`) Relay 의 답을 기다린다.
    async fn open_channel(
        &self,
        kind: Kind,
        body: Value,
    ) -> Result<(u32, ChannelRx, CancellationToken), HostError> {
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
        self.mux.send_control(&LinkFrame::new(channel, kind, body));
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
        Ok((channel, rx, cancel))
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

    /// 로컬에서 받은 `tcp` 연결 하나를 지금 링크로 원격의 `to` 에 잇는다(18-m7-plan M7-22 — [`DesktopLink::forward`]). 링크가
    /// 없으면 [`HostError::Disconnected`]. 링크가 이어 붙는 동안(M7-09)은 채널이 그대로라 연결도 이어진다.
    pub fn forward(
        &self,
        to: ForwardTo,
        tcp: TcpStream,
    ) -> BoxFuture<'static, Result<(), HostError>> {
        let link = self.link();
        let id = self.id.clone();
        Box::pin(async move {
            let link = link.ok_or(HostError::Disconnected(id))?;
            link.forward(&to, tcp).await
        })
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
        tokio::spawn(supervise(
            Arc::downgrade(&link),
            Arc::clone(&self.link),
            Arc::clone(&self.state),
            Arc::clone(&self.transport),
            Arc::clone(&self.identity),
        ));
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

/// 링크 하나를 지켜본다: 전송을 잃으면 `Reconnecting` 으로 두고 시한([`LinkLimits::resume_within`]) 안에서 새 전송으로 이어 붙기를
/// 되풀이한다(전송 열기 · 핸드셰이크가 실패하면 조금씩 늘려 기다린다). Relay 가 토큰을 받지 않거나 판 · 머신이 달라졌으면 링크를
/// 닫는다. 링크가 닫히면 `Disconnected`.
async fn supervise(
    watched: std::sync::Weak<DesktopLink>,
    slot: Arc<Mutex<Option<Arc<DesktopLink>>>>,
    state: Arc<watch::Sender<LinkState>>,
    transport: Arc<dyn Transport>,
    identity: Arc<DesktopIdentity>,
) {
    let (mut phase, closed, limits) = match watched.upgrade() {
        Some(link) => (link.mux.phase(), link.mux.closed(), *link.mux.limits()),
        None => return,
    };
    let current = |link: &Arc<DesktopLink>| {
        slot.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|now| Arc::ptr_eq(now, link))
    };
    loop {
        tokio::select! {
            () = closed.cancelled() => break,
            changed = phase.wait_for(|p| *p == Phase::Suspended) => if changed.is_err() { break },
        }
        let Some(link) = watched.upgrade() else {
            return;
        };
        if !current(&link) {
            return;
        }
        state.send_replace(LinkState::Reconnecting);
        let deadline = tokio::time::Instant::now() + limits.resume_within;
        let mut delay = std::time::Duration::from_millis(200);
        loop {
            if closed.is_cancelled() {
                break;
            }
            let attempt = async {
                let stream = transport
                    .open()
                    .await
                    .map_err(|e| DialError::Io(e.to_string()))?;
                link.resume(stream, &identity).await
            };
            let outcome = tokio::select! {
                () = closed.cancelled() => break,
                outcome = tokio::time::timeout_at(deadline, attempt) => outcome,
            };
            match outcome {
                Ok(Ok(())) => {
                    if current(&link) {
                        state.send_replace(LinkState::Connected {
                            machine: link.machine.clone(),
                            role: link.role,
                        });
                    }
                    break;
                }
                Ok(Err(e @ (DialError::Io(_) | DialError::Timeout))) => {
                    tracing::debug!(target: "pw_link", error = %e, "cannot resume the link yet");
                    let wait =
                        delay.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
                    tokio::select! {
                        () = closed.cancelled() => break,
                        () = tokio::time::sleep(wait) => {}
                    }
                    delay = (delay * 2).min(std::time::Duration::from_secs(5));
                }
                Ok(Err(e)) => {
                    tracing::info!(target: "pw_link", error = %e, "the link cannot resume");
                    link.mux.shutdown(CloseReason::Handshake(e.to_string()));
                    break;
                }
                // 시한이 지났다 — 링크의 시한 시계가 닫는다(같은 시한).
                Err(_) => {
                    link.mux.shutdown(CloseReason::HeartbeatTimeout);
                    break;
                }
            }
        }
        if closed.is_cancelled() {
            break;
        }
    }
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
