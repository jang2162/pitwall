//! Relay 쪽 끝: 받은 전송 하나에서 핸드셰이크를 하고, 증명된 Desktop 의 채널을 이 머신의 Host 연결로 잇는다(18-m7-plan M7-03 ·
//! M7-02 ②). 데몬 · 소켓은 M7-04 의 것이다 — 데몬은 첫 틀을 읽어 [`Kind::Hello`] 면 [`serve_framed`] 로 넘기고, 좁은 명령(`open` ·
//! `askpass` · `status`)은 자기가 처리한다.
//!
//! - **증명 없는 링크는 role 이 없다** — 판이 다르거나, 서명이 틀리거나(다른 Relay 에 묶인 서명 포함), 페어링 표에 없는 키면
//!   `reject` 를 보내고 끊는다. 그 전에 온 채널 열기 · 메시지는 하나도 Host 에 닿지 않는다(핸드셰이크 동안은 링크의 틀만 읽는다).
//! - 채널의 호출자는 [`crate::claim`] 이 링크 기기의 role 로 자른다.
//! - 페어링 표에서 그 기기가 빠지거나 role 이 바뀌면 링크를 끊는다([`CloseReason::PairingChanged`]) — 열린 구독 · 스트림은 role 을
//!   다시 보지 않으므로 끊는 것이 계약이다.
//! - **이어 붙기**(18-m7-plan M7-09 ①): 링크는 전송을 잃어도 [`crate::LinkLimits::resume_within`] 동안 채널을 쥐고([`Resumes`] 에
//!   그 링크의 토큰과 함께 남는다) 같은 기기가 토큰으로 다시 붙기를 기다린다. 이어 받은 전송의 `serve` 는 곧 [`CloseReason::Resumed`]
//!   로 끝나고, 링크는 그것을 처음 받은 `serve` 가 계속 쥔다(데몬의 링크 수가 겹치지 않게).

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use pw_core::Host;
use pw_rpc::Role;
use serde_json::{Value, json};

use crate::claim::{CallerClaim, admit};
use crate::device::{DeviceId, DeviceKey, PublicKey, random_challenge};
use crate::frame::{CONTROL, Framed, HANDSHAKE_MAX_FRAME, Kind, LinkFrame};
use crate::handshake::{
    Auth, Challenge, Hello, PROTOCOL, Reject, RejectReason, Resume, ResumeToken, Signer, Versions,
    Welcome, transcript,
};
use crate::mux::{AddError, CloseReason, LinkLimits, Mux};
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
    /// 이어 붙기를 기다리는 링크(데몬 수명 하나 — 복제는 같은 것을 본다).
    pub resumes: Resumes,
}

impl RelayParts {
    /// 빈 이어 붙기 등록부로.
    pub fn new(
        identity: Arc<RelayIdentity>,
        pairings: PairingTable,
        host: Arc<dyn Host>,
        limits: LinkLimits,
    ) -> Self {
        Self {
            identity,
            pairings,
            host,
            limits,
            resumes: Resumes::default(),
        }
    }
}

/// 이어 붙을 수 있는 링크들과 그 토큰(18-m7-plan M7-09 ①). 링크가 닫히면 빠진다.
#[derive(Clone, Default)]
pub struct Resumes(Arc<Mutex<Vec<Held>>>);

/// 링크 하나의 토큰들. `current` 는 상대가 가졌다고 확인된 토큰, `pending` 은 이어 붙기의 `welcome` 으로 건넸지만 상대가 받았는지
/// 아직 모르는 토큰이다 — 새 전송으로 상대의 틀이 오면 확정한다([`Resumes::confirm`]). 확정 전에 그 전송이 죽으면(welcome 을 잃음)
/// 상대는 옛 토큰으로 다시 오므로, `current` 는 확정 전까지 **한 번 더**(모두 두 번) 받는다. 확정되면 옛 토큰은 끝이다.
struct Held {
    id: u64,
    key: PublicKey,
    role: Role,
    current: ResumeToken,
    /// `current` 로 이어 붙은 수(0 · 1 · 2).
    uses: u8,
    pending: Option<ResumeToken>,
    mux: Mux,
}

impl Resumes {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Held>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn register(&self, key: PublicKey, role: Role, token: ResumeToken, mux: Mux) -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.lock().push(Held {
            id,
            key,
            role,
            current: token,
            uses: 0,
            pending: None,
            mux,
        });
        id
    }

    fn forget(&self, id: u64) {
        self.lock().retain(|h| h.id != id);
    }

    /// 그 기기 키의 링크 중 토큰이 맞는(상수 시간 비교) 것과 새로 건넬 토큰. 맞는 토큰은 확정된 토큰(`current` — 확정 전 한 번 더까지)
    /// 이거나 건넸지만 확정되지 않은 토큰(`pending` — 상대가 받았다는 증거라 그것이 `current` 가 된다)이다. role 이 다르거나 닫힌
    /// 링크, 쓸 수를 다 쓴 토큰이면 `None`.
    fn take(
        &self,
        key: &PublicKey,
        presented: &ResumeToken,
        role: Role,
    ) -> Option<(u64, Mux, ResumeToken)> {
        let next = ResumeToken::generate().ok()?;
        let mut held = self.lock();
        let mut found = None;
        // 맞는 것을 찾아도 끝까지 돌고 두 토큰을 다 견준다 — 비교의 수가 맞은 자리를 드러내지 않게.
        for (i, h) in held.iter().enumerate() {
            let current = h.current.matches(presented);
            let pending = h.pending.as_ref().is_some_and(|t| t.matches(presented));
            if h.key == *key && (current || pending) {
                found = Some((i, pending));
            }
        }
        let (i, was_pending) = found?;
        let h = &mut held[i];
        if h.role != role || h.mux.reason().is_some() {
            return None;
        }
        if was_pending {
            // 상대가 건넨 토큰을 받았다 — 그것이 확정된 토큰이고, 이번이 그 첫 사용이다.
            h.current = h.pending.take()?;
            h.uses = 1;
        } else {
            // 확정된 토큰은 한 번(보통의 이어 붙기), 건넨 토큰이 확정되지 않은 동안 한 번 더(그 welcome 을 잃음).
            let allowed = h.uses == 0 || (h.uses == 1 && h.pending.is_some());
            if !allowed {
                return None;
            }
            h.uses += 1;
        }
        h.pending = Some(next.clone());
        Some((h.id, h.mux.clone(), next))
    }

    /// 상대가 `token` 을 받았다(그것을 건넨 전송으로 상대의 틀이 왔다) — 확정하고 옛 토큰을 버린다.
    fn confirm(&self, id: u64, token: &ResumeToken) {
        let mut held = self.lock();
        if let Some(h) = held.iter_mut().find(|h| h.id == id)
            && h.pending.as_ref().is_some_and(|t| t.matches(token))
        {
            h.current = token.clone();
            h.uses = 0;
            h.pending = None;
        }
    }

    /// 링크를 모두 닫고 상대에게 `bye` 가 나갈 때까지(링크마다 1초 남짓까지) 기다린다 — 데몬이 끝날 때 부른다. 그러지 않으면
    /// Desktop 이 끊김으로 보고 이어 붙기를 되풀이하며, 그 전송(ssh 의 proxy)이 끝난 데몬을 다시 띄운다.
    pub async fn close_all(&self) {
        let muxes: Vec<Mux> = self.lock().iter().map(|h| h.mux.clone()).collect();
        for mux in &muxes {
            mux.shutdown(CloseReason::Local);
        }
        let all = async {
            for mux in &muxes {
                mux.drained().await;
            }
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), all).await;
    }

    /// 건넨 토큰이 모두 확정됐는가(시험).
    #[cfg(test)]
    pub(crate) fn settled(&self) -> bool {
        self.lock().iter().all(|h| h.pending.is_none())
    }

    /// 이어 붙기를 기다리는 링크의 수(시험 · 진단).
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
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

/// 전송 하나를 받아 링크가 닫힐 때까지 돈다. Noise 세션 위의 스트림이면(`crate::noise`) 안쪽 `hello` 의 키가 그 세션의 상대여야
/// 한다. 코드 페어링(`pair`)은 받지 않는다 — [`serve_noise`] 만 받는다.
pub async fn serve(stream: LinkStream, parts: &RelayParts) -> Result<CloseReason, ServeError> {
    let bound = stream.bound;
    let mut framed = Framed::handshake(stream);
    let first = tokio::time::timeout(parts.limits.dead_after, framed.read.read())
        .await
        .map_err(|_| ServeError::Timeout)?;
    let first = match first {
        Ok(Some((frame, _))) => frame,
        Ok(None) => return Err(ServeError::Io("closed before the first frame".to_owned())),
        Err(e) => return Err(ServeError::Io(e.to_string())),
    };
    serve_inner(framed, first, parts, bound, |_| {}).await
}

/// [`serve_noise`] 가 한 일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    /// 링크가 섰다가 닫혔다.
    Link(CloseReason),
    /// 코드로 기기 하나를 페어링했다(링크는 열지 않았다 — 그 기기는 다시 붙는다).
    Paired { device: DeviceId, role: Role },
}

/// 직접 연결(18-m7-plan M7-23 ① — QUIC 은 M7-24)의 받기: 받은 바이트 스트림에서 Noise XX 를 하고(이 Relay 의 기기 키), 그 위의 첫
/// 틀이 `hello` 면 M7-03 링크를([`serve_proven`] 과 같다 — 안쪽 `hello` 의 키가 Noise 상대여야 한다), `pair` 면 코드 페어링을
/// 한다(`crate::pair` — 코드 페어링은 이 길에서만 받는다).
pub async fn serve_noise(
    stream: LinkStream,
    parts: &RelayParts,
    codes: &dyn crate::pair::CodeBook,
    on_proven: impl FnOnce(&Proven) + Send,
) -> Result<Served, ServeError> {
    let limit = parts.limits.dead_after;
    let secured = tokio::time::timeout(limit, crate::noise::respond(stream, &parts.identity.key))
        .await
        .map_err(|_| ServeError::Timeout)?
        .map_err(|e| ServeError::Io(e.to_string()))?;
    let Some(bound) = secured.bound else {
        return Err(ServeError::Io("the noise session has no peer".to_owned()));
    };
    let mut framed = Framed::handshake(secured);
    let first = match tokio::time::timeout(limit, framed.read.read())
        .await
        .map_err(|_| ServeError::Timeout)?
    {
        Ok(Some((frame, _))) => frame,
        Ok(None) => return Err(ServeError::Io("closed before the first frame".to_owned())),
        Err(e) => return Err(ServeError::Io(e.to_string())),
    };
    if first.kind == Kind::Pair && first.channel == CONTROL {
        let paired = tokio::time::timeout(
            limit,
            crate::pair::serve_pair(
                framed,
                first,
                bound,
                &parts.identity.key,
                &parts.identity.versions,
                codes,
            ),
        )
        .await
        .map_err(|_| ServeError::Timeout)?;
        return match paired {
            Ok((device, role)) => Ok(Served::Paired { device, role }),
            Err(crate::pair::PairRefused::Rejected(reason)) => Err(ServeError::Rejected {
                reason,
                theirs: None,
            }),
            Err(crate::pair::PairRefused::Io(e)) => Err(ServeError::Io(e)),
        };
    }
    serve_inner(framed, first, parts, Some(bound), on_proven)
        .await
        .map(Served::Link)
}

/// 데몬이 첫 틀을 이미 읽은 전송. 첫 틀이 `hello` 가 아니면 거절한다.
pub async fn serve_framed(
    framed: Framed,
    first: LinkFrame,
    parts: &RelayParts,
) -> Result<CloseReason, ServeError> {
    serve_proven(framed, first, parts, |_| {}).await
}

/// [`serve_framed`] 와 같되 증명을 마친 순간 `on_proven` 을 부른다(링크가 닫힐 때까지 돌기 전에). 데몬(M7-04)은 여기서 그 연결을
/// 「증명 전 연결」 수에서 빼고 붙은 링크(owner 인가)를 센다.
///
/// 핸드셰이크 동안은 틀 상한이 [`HANDSHAKE_MAX_FRAME`] 이고(부른 쪽이 `Framed::new` 로 만들었어도 여기서 낮춘다), 증명 뒤에만 링크 틀
/// 상한으로 올린다.
pub async fn serve_proven(
    framed: Framed,
    first: LinkFrame,
    parts: &RelayParts,
    on_proven: impl FnOnce(&Proven) + Send,
) -> Result<CloseReason, ServeError> {
    serve_inner(framed, first, parts, None, on_proven).await
}

async fn serve_inner(
    mut framed: Framed,
    first: LinkFrame,
    parts: &RelayParts,
    bound: Option<crate::noise::Bound>,
    on_proven: impl FnOnce(&Proven) + Send,
) -> Result<CloseReason, ServeError> {
    framed.read.set_max_body(HANDSHAKE_MAX_FRAME);
    // 표를 핸드셰이크 **전에** 구독한다 — role 을 읽은 뒤에 구독하면 그 사이의 해제 · role 변경을 「이미 본 값」 으로 놓친다.
    let mut table = parts.pairings.watch();
    let (proven, resume) = tokio::time::timeout(
        parts.limits.dead_after,
        handshake(&mut framed, first, parts, bound),
    )
    .await
    .map_err(|_| ServeError::Timeout)??;
    tracing::info!(target: "pw_link", device = %proven.device, role = ?proven.role, resume = resume.is_some(), "link proven");
    if let Some(resume) = resume {
        return resume_link(framed, resume, &proven, parts, on_proven).await;
    }
    let token = ResumeToken::generate().map_err(|e| ServeError::Io(e.to_string()))?;
    write_welcome(&mut framed, proven.role, Some(token.clone()), None).await?;
    framed.read.set_max_body(pw_wire::MAX_BODY_BYTES);
    on_proven(&proven);
    let (mux, mut inbox) = Mux::start_resumable(framed, parts.limits);
    let held = parts
        .resumes
        .register(proven.key, proven.role, token, mux.clone());
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
        match frame.kind {
            Kind::Connect if frame.channel != CONTROL => {
                connect(&mux, frame, &proven, &parts.host);
            }
            Kind::Forward if frame.channel != CONTROL => forward(&mux, frame, &proven),
            _ => {
                mux.shutdown(CloseReason::Protocol(format!(
                    "unexpected {:?} frame",
                    frame.kind
                )));
                break;
            }
        }
    }
    watcher.abort();
    closed.cancelled().await;
    parts.resumes.forget(held);
    Ok(mux.reason().unwrap_or(CloseReason::Local))
}

async fn write_welcome(
    framed: &mut Framed,
    role: Role,
    token: Option<ResumeToken>,
    resumed: Option<u64>,
) -> Result<(), ServeError> {
    framed
        .write
        .write(&LinkFrame::control(
            Kind::Welcome,
            json!(Welcome {
                role,
                token,
                resumed
            }),
        ))
        .await
        .map_err(|e| ServeError::Io(e.to_string()))
}

/// 멈춘 링크를 이 전송으로 이어 준다. 토큰은 서명 · 페어링을 본 뒤에만 읽는다(`handshake`). 그 기기 키의 링크 토큰과 맞지 않거나
/// (모르는 · 이미 쓴 · 다른 기기의 토큰) role 이 바뀌었거나 링크가 닫혔으면 거절한다.
async fn resume_link(
    mut framed: Framed,
    resume: Resume,
    proven: &Proven,
    parts: &RelayParts,
    on_proven: impl FnOnce(&Proven) + Send,
) -> Result<CloseReason, ServeError> {
    let Some((id, mux, token)) = parts.resumes.take(&proven.key, &resume.token, proven.role) else {
        return Err(reject(&mut framed, RejectReason::Resume, None, None).await);
    };
    let Some(received) = mux.suspend_for_resume() else {
        return Err(reject(&mut framed, RejectReason::Resume, None, None).await);
    };
    write_welcome(
        &mut framed,
        proven.role,
        Some(token.clone()),
        Some(received),
    )
    .await?;
    framed.read.set_max_body(pw_wire::MAX_BODY_BYTES);
    on_proven(proven);
    // 이 전송으로 Desktop 의 틀이 오면(Desktop 은 welcome 을 읽은 뒤에만 이 전송에 쓴다 — 이어 붙자마자 `ack` 를 보낸다) 새 토큰을
    // 확정한다.
    let resumes = parts.resumes.clone();
    let confirmed = Box::new(move || resumes.confirm(id, &token));
    if let Err(why) = mux.resume_then(framed, resume.received, Some(confirmed)) {
        tracing::info!(target: "pw_link", %why, "cannot resume the link");
        mux.shutdown(CloseReason::Protocol(why));
    }
    Ok(CloseReason::Resumed)
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
    let (mut rx, cancel) = match mux.add_channel(channel) {
        Ok(opened) => opened,
        Err(AddError::Budget) => {
            refuse(mux, channel, "the link's receive budget is exhausted");
            return;
        }
        Err(AddError::Closed | AddError::Exists) => return,
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

/// 포트 포워딩 채널 하나(18-m7-plan M7-22 — [`crate::forward`]). **owner 링크만**(메인 판정 M7-P15): 다른 role 의 기기는
/// 거절하고 TCP 를 열지 않는다. 대상은 이 머신의 루프백만이다([`crate::forward::ForwardTo::check`] — 메인 판정 2026-11-24).
fn forward(mux: &Mux, frame: LinkFrame, proven: &Proven) {
    let channel = frame.channel;
    if proven.role != Role::Owner {
        refuse(mux, channel, "port forwarding needs an owner device");
        return;
    }
    if mux.has_channel(channel) {
        refuse(mux, channel, "channel already open");
        return;
    }
    if mux.channel_count() >= mux.limits().max_channels {
        refuse(mux, channel, "too many channels on this link");
        return;
    }
    let to = match serde_json::from_value::<crate::forward::ForwardTo>(frame.body)
        .map_err(|e| format!("bad forward target: {e}"))
        .and_then(|to| to.check().map(|()| to))
    {
        Ok(to) => to,
        Err(why) => {
            refuse(mux, channel, &why);
            return;
        }
    };
    let (rx, cancel) = match mux.add_channel(channel) {
        Ok(opened) => opened,
        Err(AddError::Budget) => {
            refuse(mux, channel, "the link's receive budget is exhausted");
            return;
        }
        Err(AddError::Closed | AddError::Exists) => return,
    };
    let mux = mux.clone();
    tokio::spawn(async move {
        // `check` 를 지난 대상은 루프백 주소다(이름을 풀지 않는다).
        let addrs = to.addrs().unwrap_or_default();
        let connecting = tokio::time::timeout(
            crate::forward::CONNECT_TIMEOUT,
            tokio::net::TcpStream::connect(addrs.as_slice()),
        );
        let tcp = tokio::select! {
            () = cancel.cancelled() => return,
            tcp = connecting => tcp,
        };
        let tcp = match tcp {
            Ok(Ok(tcp)) => tcp,
            Ok(Err(e)) => {
                mux.forget_channel(channel);
                refuse(
                    &mux,
                    channel,
                    &format!("cannot connect to {}:{}: {e}", to.host, to.port),
                );
                return;
            }
            Err(_) => {
                mux.forget_channel(channel);
                refuse(
                    &mux,
                    channel,
                    &format!("connecting to {}:{} timed out", to.host, to.port),
                );
                return;
            }
        };
        // 연 대상을 싣는다 — Desktop 은 청한 것과 다르면 받지 않는다(버그 · 판 어긋남을 잡는 확인, 보호가 아니다).
        mux.send_control(&LinkFrame::new(channel, Kind::Accepted, json!(to)));
        crate::forward::pump(mux, channel, rx, cancel, tcp).await;
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
    bound: Option<crate::noise::Bound>,
) -> Result<(Proven, Option<Resume>), ServeError> {
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
    // Noise 위에서는 안쪽 `hello` 의 키가 그 세션이 증명한 상대여야 한다(중계 끼워 넣기 — `crate::noise` 머리 주석).
    if bound.is_some_and(|b| !b.is_peer(&hello.key)) {
        tracing::info!(target: "pw_link", "the hello key is not the noise peer");
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
    Ok((
        Proven {
            device: hello.device,
            key: hello.key,
            role,
        },
        auth.resume,
    ))
}
