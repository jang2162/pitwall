//! Relay 쪽 끝: 받은 전송 하나에서 핸드셰이크를 하고, 증명된 Desktop 의 채널을 이 머신의 Host 연결로 잇는다(18-m7-plan M7-03 ·
//! M7-02 ②). 데몬 · 소켓은 M7-04 의 것이다 — 데몬은 첫 틀을 읽어 [`Kind::Hello`] 면 [`serve_framed`] 로 넘기고, 좁은 명령(`open` ·
//! `askpass` · `status`)은 자기가 처리한다.
//!
//! - **증명 없는 링크는 role 이 없다** — 판이 다르거나, 서명이 틀리거나(다른 Relay 에 묶인 서명 포함), 페어링 표에 없는 키면
//!   `reject` 를 보내고 끊는다. 그 전에 온 채널 열기 · 메시지는 하나도 Host 에 닿지 않는다(핸드셰이크 동안은 링크의 틀만 읽는다).
//! - 채널의 호출자는 [`crate::claim`] 이 링크 기기의 role 로 자른다.
//! - 페어링 표에서 그 기기가 빠지거나 role 이 바뀌면 링크를 끊는다([`CloseReason::PairingChanged`]) — 열린 구독 · 스트림은 role 을
//!   다시 보지 않으므로 끊는 것이 계약이다.

use std::fmt;
use std::sync::Arc;

use pw_core::Host;
use pw_rpc::Role;
use serde_json::{Value, json};

use crate::claim::{CallerClaim, admit};
use crate::device::{DeviceId, DeviceKey, PublicKey, random_challenge};
use crate::frame::{CONTROL, Framed, Kind, LinkFrame};
use crate::handshake::{
    Auth, Challenge, Hello, PROTOCOL, Reject, RejectReason, Signer, Versions, Welcome, transcript,
};
use crate::mux::{CloseReason, LinkLimits, Mux};
use crate::pairing::PairingTable;
use crate::transport::LinkStream;

/// 이 Relay 의 기기 키와 판.
pub struct RelayIdentity {
    pub key: Arc<DeviceKey>,
    pub versions: Versions,
}

/// 링크를 받는 데 드는 것.
#[derive(Clone)]
pub struct RelayParts {
    pub identity: Arc<RelayIdentity>,
    pub pairings: PairingTable,
    pub host: Arc<dyn Host>,
    pub limits: LinkLimits,
}

/// 링크가 서지 않은 까닭(데몬의 로그 — 키 · 서명 원문은 싣지 않는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeError {
    /// 핸드셰이크가 시한 안에 끝나지 않았다.
    Timeout,
    /// 첫 틀 전에 전송이 닫혔거나 읽지 못했다.
    Io(String),
    /// 거절했다. 판이 다르면 상대의 판.
    Rejected {
        reason: RejectReason,
        theirs: Option<Versions>,
    },
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("the link handshake timed out"),
            Self::Io(e) => write!(f, "link i/o: {e}"),
            Self::Rejected { reason, .. } => write!(f, "rejected the link: {reason:?}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// 증명을 마친 링크.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proven {
    pub device: DeviceId,
    pub key: PublicKey,
    pub role: Role,
}

/// 전송 하나를 받아 링크가 닫힐 때까지 돈다.
pub async fn serve(stream: LinkStream, parts: &RelayParts) -> Result<CloseReason, ServeError> {
    let mut framed = Framed::new(stream);
    let first = tokio::time::timeout(parts.limits.dead_after, framed.read.read())
        .await
        .map_err(|_| ServeError::Timeout)?;
    let first = match first {
        Ok(Some((frame, _))) => frame,
        Ok(None) => return Err(ServeError::Io("closed before the first frame".to_owned())),
        Err(e) => return Err(ServeError::Io(e.to_string())),
    };
    serve_framed(framed, first, parts).await
}

/// 데몬이 첫 틀을 이미 읽은 전송. 첫 틀이 `hello` 가 아니면 거절한다.
pub async fn serve_framed(
    mut framed: Framed,
    first: LinkFrame,
    parts: &RelayParts,
) -> Result<CloseReason, ServeError> {
    // 표를 핸드셰이크 **전에** 구독한다 — role 을 읽은 뒤에 구독하면 그 사이의 해제 · role 변경을 「이미 본 값」 으로 놓친다.
    let mut table = parts.pairings.watch();
    let proven = tokio::time::timeout(
        parts.limits.dead_after,
        handshake(&mut framed, first, parts),
    )
    .await
    .map_err(|_| ServeError::Timeout)??;
    tracing::info!(target: "pw_link", device = %proven.device, role = ?proven.role, "link proven");
    let (mux, mut inbox) = Mux::start(framed, parts.limits);
    let closed = mux.closed();

    // 페어링이 바뀌면 끊는다(핸드셰이크 뒤에 이미 바뀌었으면 `changed` 가 곧바로 깬다).
    let watcher = {
        let (mux, pairings, key, role) =
            (mux.clone(), parts.pairings.clone(), proven.key, proven.role);
        let closed = closed.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = closed.cancelled() => return,
                    changed = table.changed() => if changed.is_err() { return },
                }
                if pairings.role_of(&key) != Some(role) {
                    mux.shutdown(CloseReason::PairingChanged);
                    return;
                }
            }
        })
    };

    loop {
        let frame = tokio::select! {
            () = closed.cancelled() => break,
            frame = inbox.recv() => frame,
        };
        let Some(frame) = frame else { break };
        if frame.kind != Kind::Connect || frame.channel == CONTROL {
            mux.shutdown(CloseReason::Protocol(format!(
                "unexpected {:?} frame",
                frame.kind
            )));
            break;
        }
        connect(&mux, frame, &proven, &parts.host);
    }
    watcher.abort();
    closed.cancelled().await;
    Ok(mux.reason().unwrap_or(CloseReason::Local))
}

fn refuse(mux: &Mux, channel: u32, why: &str) {
    mux.send_control(&LinkFrame::new(
        channel,
        Kind::Refused,
        json!({ "reason": why }),
    ));
}

/// 채널 열기 하나. 호출자를 링크 기기의 role 로 자르고 Host 에 연결을 연다.
fn connect(mux: &Mux, frame: LinkFrame, proven: &Proven, host: &Arc<dyn Host>) {
    let channel = frame.channel;
    if mux.has_channel(channel) {
        refuse(mux, channel, "channel already open");
        return;
    }
    if mux.channel_count() >= mux.limits().max_channels {
        refuse(mux, channel, "too many channels on this link");
        return;
    }
    let caller = match serde_json::from_value::<CallerClaim>(frame.body)
        .map_err(|e| format!("bad caller: {e}"))
        .and_then(|claim| admit(claim, proven.role, &proven.device))
    {
        Ok(caller) => caller,
        Err(why) => {
            refuse(mux, channel, &why);
            return;
        }
    };
    let Some((mut rx, cancel)) = mux.add_channel(channel) else {
        return;
    };
    let (mux, host) = (mux.clone(), Arc::clone(host));
    tokio::spawn(async move {
        let connection = tokio::select! {
            () = cancel.cancelled() => return,
            connection = host.connect(caller) => connection,
        };
        let mut connection = match connection {
            Ok(connection) => connection,
            Err(e) => {
                mux.forget_channel(channel);
                refuse(&mux, channel, &e.to_string());
                return;
            }
        };
        mux.send_control(&LinkFrame::new(channel, Kind::Accepted, Value::Null));
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                inbound = rx.recv() => match inbound {
                    Some(message) => connection.sink.send(message),
                    None => break,
                },
                outbound = connection.source.recv() => match outbound {
                    Some(message) => {
                        if !mux.send_msg(channel, message) {
                            break;
                        }
                    }
                    None => {
                        mux.finish_channel(channel);
                        break;
                    }
                },
            }
        }
        // 연결을 버리면 Host 가 그 세션의 요청 · 스트림을 취소하고 구독을 걷는다.
        drop(connection);
    });
}

async fn read_control(framed: &mut Framed) -> Result<LinkFrame, ServeError> {
    match framed.read.read().await {
        Ok(Some((frame, _))) => Ok(frame),
        Ok(None) => Err(ServeError::Io("closed during the handshake".to_owned())),
        Err(e) => Err(ServeError::Io(e.to_string())),
    }
}

async fn reject(
    framed: &mut Framed,
    reason: RejectReason,
    ours: Option<&Versions>,
    theirs: Option<Versions>,
) -> ServeError {
    let body = Reject {
        reason,
        versions: ours.cloned(),
    };
    // 거절을 못 보내도 끊는 것은 같다.
    let _ = framed
        .write
        .write(&LinkFrame::control(Kind::Reject, json!(body)))
        .await;
    tracing::info!(target: "pw_link", ?reason, "rejected a link");
    ServeError::Rejected { reason, theirs }
}

async fn handshake(
    framed: &mut Framed,
    first: LinkFrame,
    parts: &RelayParts,
) -> Result<Proven, ServeError> {
    let ours = &parts.identity.versions;
    if first.kind != Kind::Hello || first.channel != CONTROL {
        return Err(reject(framed, RejectReason::Protocol, None, None).await);
    }
    let Ok(hello) = serde_json::from_value::<Hello>(first.body) else {
        return Err(reject(framed, RejectReason::Protocol, None, None).await);
    };
    // 판이 다르면 RPC 를 하나도 받지 않는다(M7-07). 이쪽 판을 알려 준다.
    if hello.versions != *ours {
        return Err(reject(
            framed,
            RejectReason::Version,
            Some(ours),
            Some(hello.versions),
        )
        .await);
    }
    if hello.device != hello.key.id() {
        return Err(reject(framed, RejectReason::Protocol, None, None).await);
    }
    let my_key = parts.identity.key.public();
    let my_challenge = Challenge(random_challenge().map_err(|e| ServeError::Io(e.to_string()))?);
    let signed = transcript(
        Signer::Relay,
        PROTOCOL,
        &hello.challenge,
        &my_challenge,
        &hello.key,
        &my_key,
    );
    let reply = Hello {
        versions: ours.clone(),
        device: my_key.id(),
        key: my_key,
        challenge: my_challenge,
        signature: Some(parts.identity.key.sign(&signed)),
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Hello, json!(reply)))
        .await
        .map_err(|e| ServeError::Io(e.to_string()))?;

    let frame = read_control(framed).await?;
    if frame.kind != Kind::Auth || frame.channel != CONTROL {
        return Err(reject(framed, RejectReason::Protocol, None, None).await);
    }
    let Ok(auth) = serde_json::from_value::<Auth>(frame.body) else {
        return Err(reject(framed, RejectReason::Protocol, None, None).await);
    };
    // 서명 글의 Relay 키는 이 Relay 의 것이어야 한다(채널 바인딩 — 계획 재리뷰 F-c). 다른 Relay 에 묶인 서명은 이 키로 만든 글과
    // 맞지 않는다. `auth.relayKey` 는 진단에만 쓴다.
    let text = transcript(
        Signer::Desktop,
        PROTOCOL,
        &hello.challenge,
        &my_challenge,
        &hello.key,
        &my_key,
    );
    if !hello.key.verifies(&text, &auth.signature) {
        if auth.relay_key != my_key {
            tracing::info!(target: "pw_link", "the desktop signed for another relay");
        }
        return Err(reject(framed, RejectReason::BadSignature, None, None).await);
    }
    // 증명된 키라도 페어링 표에 없으면 role 이 없다(M7-02 ②).
    let Some(role) = parts.pairings.role_of(&hello.key) else {
        return Err(reject(framed, RejectReason::Unpaired, None, None).await);
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Welcome, json!(Welcome { role })))
        .await
        .map_err(|e| ServeError::Io(e.to_string()))?;
    Ok(Proven {
        device: hello.device,
        key: hello.key,
        role,
    })
}
