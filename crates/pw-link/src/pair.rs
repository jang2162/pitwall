//! 6자리 코드 페어링(18-m7-plan M7-23 ② · 08 「페어링」): 수동 배치한 Relay 에 Desktop(또는 다른 기기)을 SSH 없이 페어링한다.
//!
//! ```text
//! (Noise XX 세션 — crate::noise. 그 밖의 전송에서는 받지 않는다)
//! Desktop → Relay  pair    { protocol, app, releaseChannel, device, key, name, spake }
//! Relay → Desktop  pair    { key, spake }                 (또는 reject — code · version · protocol)
//! Desktop → Relay  confirm { mac }
//! Relay → Desktop  confirm { mac, role }                  (또는 reject — code)
//! ```
//!
//! - **코드는 PAKE(SPAKE2 — `spake2`) 의 암호로만 쓴다**(메인 판정): 코드를 Noise psk 나 `HMAC(code, …)` 로 쓰면 처음 붙는 능동
//!   중간자가 받은 값으로 10^6 경우를 오프라인으로 풀어 5분 안에 진짜 Relay 에 자기 키를 넣는다. SPAKE2 는 연결 하나에 추측 하나만
//!   준다.
//! - **대본은 Noise 세션에 묶인다**: SPAKE2 의 양쪽 identity 에 Noise 핸드셰이크 해시를 넣고, 키 확인 MAC 도 그 해시 위에 한다 — 다른
//!   세션으로 옮긴 메시지는 키가 맞지 않는다. `pair.key`(양쪽 Ed25519 키)는 Noise 상대 정적 키와 같아야 한다 — 그래서 페어링되는
//!   키는 Noise 로 증명된 키다(중간자는 자기 키만 넣을 수 있는데, 그러려면 코드를 알아야 한다).
//! - **키 확인은 두 방향**이고 Desktop 이 먼저 낸다 — Relay 는 상대가 코드를 안다는 것을 본 뒤에만 자기 MAC 을 보낸다.
//! - **시도 수는 Relay 전체에서 그 코드에 대해 센다**([`CodeBook::attempt`] — 대기 파일의 시도 수). 시도는 Relay 가 SPAKE2 답을
//!   보내기 **전에** 센다 — 답을 받고 확인을 보내지 않고 끊는 연결도 한 번이다. [`MAX_ATTEMPTS`] 에 닿거나 [`CODE_TTL`] 이 지나거나 한
//!   번 성공하면 그 코드는 끝이다.
//! - 코드는 로그 · 감사 · `Debug` · 에이전트가 읽는 글에 싣지 않는다([`PairingCode`] 의 `Debug` 는 가린다).

use std::io;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use pw_rpc::Role;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use zeroize::Zeroizing;

use crate::desktop::DesktopIdentity;
use crate::device::{DeviceId, MachineId, PublicKey};
use crate::frame::{CONTROL, Framed, Kind, LinkFrame};
use crate::handshake::{Reject, RejectReason, Versions};
use crate::noise::Bound;
use crate::transport::LinkStream;

/// 코드가 사는 시간(08 「보안 모델」).
pub const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// 코드 하나에 받는 시도의 수(틀린 코드 세 번이면 버린다 — 18-m7-plan M7-23 시험 ②).
pub const MAX_ATTEMPTS: u32 = 3;
/// 자릿수.
pub const CODE_DIGITS: usize = 6;

/// 6자리 코드. `Debug` · `Display` 는 가린다 — 사람에게 보일 때만 [`Self::reveal`].
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode(Zeroizing<String>);

impl PairingCode {
    /// OS 난수로 새 코드(000000 ~ 999999 고르게).
    pub fn generate() -> io::Result<Self> {
        // 2^32 를 10^6 로 나눈 나머지가 치우치지 않게 위쪽을 버린다.
        const LIMIT: u32 = u32::MAX - (u32::MAX % 1_000_000);
        loop {
            let mut bytes = [0u8; 4];
            getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
            let n = u32::from_le_bytes(bytes);
            if n < LIMIT {
                return Ok(Self(Zeroizing::new(format!("{:06}", n % 1_000_000))));
            }
        }
    }

    /// 사람이 친 글(공백 · `-` 는 뺀다). 6자리 숫자가 아니면 `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let digits: Zeroizing<String> = Zeroizing::new(
            text.chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .collect(),
        );
        (digits.len() == CODE_DIGITS && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| Self(digits))
    }

    /// 사람에게 보일 글(터미널 · 기기 화면). 로그 · 감사에 넘기지 않는다.
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingCode(..)")
    }
}

/// 시도 하나에 쓸 대기 코드.
pub struct Pending {
    pub code: PairingCode,
    /// 코드를 만든 사람이 고른 role — 페어링되는 기기는 이 role 을 받는다.
    pub role: Role,
    /// 코드를 만든 사람이 준 이름(없으면 기기가 댄 이름).
    pub name: Option<String>,
}

/// Relay 의 대기 코드(데몬은 `$DATA/relay/pairing-code.json` — `pw_relay::codes`).
pub trait CodeBook: Send + Sync {
    /// 대기 코드에 시도 하나를 쓴다 — 쓴 뒤의 시도 수가 [`MAX_ATTEMPTS`] 에 닿으면 그 코드를 버린다(이번 시도는 그 코드로 한다). 코드가
    /// 없거나 만료됐거나 시도를 다 썼으면 `None`.
    fn attempt(&self) -> io::Result<Option<Pending>>;
    /// 이번 시도가 맞았다 — 코드를 버리고 `key` 를 `role` 로 페어링 표에 넣는다. 그 사이에 코드가 바뀌었으면(새 코드 · 해제) 오류.
    fn complete(
        &self,
        code: &PairingCode,
        key: &PublicKey,
        role: Role,
        name: &str,
    ) -> io::Result<()>;
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairHello {
    #[serde(flatten)]
    versions: Versions,
    device: DeviceId,
    key: PublicKey,
    #[serde(default)]
    name: String,
    spake: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PairReply {
    key: PublicKey,
    spake: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Confirm {
    mac: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<Role>,
}

fn identities(hash: &[u8; 32]) -> (Identity, Identity) {
    let with = |who: &[u8]| {
        let mut id = b"pitwall-pair\0".to_vec();
        id.extend_from_slice(who);
        id.extend_from_slice(hash);
        Identity::new(&id)
    };
    (with(b"desktop\0"), with(b"relay\0\0\0"))
}

fn mac(key: &[u8], who: &[u8], hash: &[u8; 32]) -> Hmac<Sha256> {
    // 키 길이에 제한이 없다(HMAC) — 실패하지 않는다.
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("hmac takes any key length");
    mac.update(b"pitwall-pair-confirm\0");
    mac.update(who);
    mac.update(hash);
    mac
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    STANDARD.decode(text).ok()
}

/// 페어링이 서지 않은 까닭(데몬 로그 — 코드 · 키 원문은 싣지 않는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairRefused {
    Io(String),
    Rejected(RejectReason),
}

impl std::fmt::Display for PairRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "pairing i/o: {e}"),
            Self::Rejected(reason) => write!(f, "refused a pairing: {reason:?}"),
        }
    }
}

async fn reject(framed: &mut Framed, reason: RejectReason, ours: Option<&Versions>) -> PairRefused {
    let _ = framed
        .write
        .write(&LinkFrame::control(
            Kind::Reject,
            json!(Reject {
                reason,
                versions: ours.cloned(),
            }),
        ))
        .await;
    tracing::info!(target: "pw_link", ?reason, "refused a pairing");
    PairRefused::Rejected(reason)
}

/// Relay 쪽: Noise 세션 위에서 첫 틀이 `pair` 인 연결(`crate::relay::serve_noise` 가 부른다). 맞으면 그 기기를 페어링 표에 넣고
/// 끝난다 — 링크는 열지 않는다(기기는 다시 붙어 M7-03 핸드셰이크를 한다).
pub(crate) async fn serve_pair(
    mut framed: Framed,
    first: LinkFrame,
    bound: Bound,
    relay_key: &crate::device::DeviceKey,
    ours: &Versions,
    codes: &dyn CodeBook,
) -> Result<(DeviceId, Role), PairRefused> {
    let Ok(hello) = serde_json::from_value::<PairHello>(first.body) else {
        return Err(reject(&mut framed, RejectReason::Protocol, None).await);
    };
    if hello.versions != *ours {
        return Err(reject(&mut framed, RejectReason::Version, Some(ours)).await);
    }
    // 페어링되는 키는 이 Noise 세션이 증명한 키다.
    if hello.device != hello.key.id() || !bound.is_peer(&hello.key) {
        return Err(reject(&mut framed, RejectReason::Protocol, None).await);
    }
    let Some(spake_a) = unb64(&hello.spake) else {
        return Err(reject(&mut framed, RejectReason::Protocol, None).await);
    };
    // 시도는 답을 보내기 전에 센다.
    let pending = match codes.attempt() {
        Ok(Some(pending)) => pending,
        Ok(None) => return Err(reject(&mut framed, RejectReason::Code, None).await),
        Err(e) => {
            tracing::warn!(target: "pw_link", %e, "cannot read the pending pairing code");
            return Err(reject(&mut framed, RejectReason::Code, None).await);
        }
    };
    let (id_a, id_b) = identities(&bound.hash);
    let (state, spake_b) = Spake2::<Ed25519Group>::start_b(
        &Password::new(pending.code.reveal().as_bytes()),
        &id_a,
        &id_b,
    );
    let Ok(key) = state.finish(&spake_a).map(Zeroizing::new) else {
        return Err(reject(&mut framed, RejectReason::Code, None).await);
    };
    let reply = PairReply {
        key: relay_key.public(),
        spake: b64(&spake_b),
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Pair, json!(reply)))
        .await
        .map_err(|e| PairRefused::Io(e.to_string()))?;

    let frame = match framed.read.read().await {
        Ok(Some((frame, _))) => frame,
        Ok(None) => return Err(PairRefused::Io("closed before the confirmation".to_owned())),
        Err(e) => return Err(PairRefused::Io(e.to_string())),
    };
    let confirm = (frame.kind == Kind::Confirm && frame.channel == CONTROL)
        .then(|| serde_json::from_value::<Confirm>(frame.body).ok())
        .flatten()
        .and_then(|c| unb64(&c.mac));
    let Some(their_mac) = confirm else {
        return Err(reject(&mut framed, RejectReason::Protocol, None).await);
    };
    // 상수 시간 비교(`verify_slice`).
    if mac(&key, b"desktop", &bound.hash)
        .verify_slice(&their_mac)
        .is_err()
    {
        return Err(reject(&mut framed, RejectReason::Code, None).await);
    }
    let name = pending
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| crate::pairing::valid_name(&hello.name).then(|| hello.name.clone()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "device".to_owned());
    if let Err(e) = codes.complete(&pending.code, &hello.key, pending.role, &name) {
        tracing::warn!(target: "pw_link", %e, "cannot write the paired device");
        return Err(reject(&mut framed, RejectReason::Code, None).await);
    }
    let ours_mac = mac(&key, b"relay", &bound.hash).finalize().into_bytes();
    let _ = framed
        .write
        .write(&LinkFrame::control(
            Kind::Confirm,
            json!(Confirm {
                mac: b64(&ours_mac),
                role: Some(pending.role),
            }),
        ))
        .await;
    tracing::info!(target: "pw_link", device = %hello.device, role = ?pending.role, "paired a device by code");
    Ok((hello.device, pending.role))
}

/// 코드로 페어링한 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodePaired {
    /// 그 Relay 의 머신 id(Noise 로 증명된 Relay 키의 지문) — 부르는 쪽이 그 호스트 항목에 기억한다.
    pub machine: MachineId,
    pub relay_key: PublicKey,
    pub role: Role,
}

/// 코드 페어링이 되지 않은 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    /// Noise 세션이 아닌 스트림(SSH 링크 · 데몬 소켓) — 코드 페어링은 Noise 위에서만 한다.
    NotSecured,
    /// 6자리 숫자가 아니다.
    BadCode,
    Io(String),
    Timeout,
    Protocol(String),
    VersionMismatch {
        theirs: Option<Versions>,
    },
    /// Relay 가 받지 않았다(`Code` — 틀린 · 만료된 · 다 쓴 · 없는 코드).
    Rejected(RejectReason),
    /// Relay 의 키 확인이 틀렸다 — 코드를 모르는 상대(중간자).
    RelayNotProven,
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSecured => f.write_str("code pairing needs a noise session"),
            Self::BadCode => f.write_str("the pairing code is six digits"),
            Self::Io(e) => write!(f, "pairing i/o: {e}"),
            Self::Timeout => f.write_str("the pairing timed out"),
            Self::Protocol(e) => write!(f, "pairing protocol error: {e}"),
            Self::VersionMismatch { .. } => f.write_str("the relay runs another version"),
            Self::Rejected(reason) => write!(f, "the relay refused the pairing: {reason:?}"),
            Self::RelayNotProven => f.write_str("the relay did not prove the code"),
        }
    }
}

impl std::error::Error for PairError {}

/// Desktop 쪽: Noise 세션(`crate::noise::initiate`) 위에서 코드로 페어링한다. 끝나면 스트림을 닫는다 — 링크는 그 뒤 따로 연다
/// (`dial`, `Expect::Machine(CodePaired::machine)`).
pub async fn pair_with_code(
    stream: LinkStream,
    me: &DesktopIdentity,
    code: &str,
    name: &str,
    timeout: Duration,
) -> Result<CodePaired, PairError> {
    let Some(bound) = stream.bound else {
        return Err(PairError::NotSecured);
    };
    let code = PairingCode::parse(code).ok_or(PairError::BadCode)?;
    let mut framed = Framed::handshake(stream);
    tokio::time::timeout(timeout, pair_inner(&mut framed, bound, me, &code, name))
        .await
        .map_err(|_| PairError::Timeout)?
}

async fn read_reply(framed: &mut Framed) -> Result<LinkFrame, PairError> {
    match framed.read.read().await {
        Ok(Some((frame, _))) if frame.channel == CONTROL => Ok(frame),
        Ok(Some(_)) => Err(PairError::Protocol(
            "a frame off the control channel".to_owned(),
        )),
        Ok(None) => Err(PairError::Io("the relay closed the pairing".to_owned())),
        Err(e) => Err(PairError::Protocol(e.to_string())),
    }
}

fn refused(frame: LinkFrame) -> PairError {
    match serde_json::from_value::<Reject>(frame.body) {
        Ok(Reject {
            reason: RejectReason::Version,
            versions,
        }) => PairError::VersionMismatch { theirs: versions },
        Ok(reject) => PairError::Rejected(reject.reason),
        Err(e) => PairError::Protocol(format!("bad reject: {e}")),
    }
}

async fn pair_inner(
    framed: &mut Framed,
    bound: Bound,
    me: &DesktopIdentity,
    code: &PairingCode,
    name: &str,
) -> Result<CodePaired, PairError> {
    let io = |e: std::io::Error| PairError::Io(e.to_string());
    let (id_a, id_b) = identities(&bound.hash);
    let (state, spake_a) =
        Spake2::<Ed25519Group>::start_a(&Password::new(code.reveal().as_bytes()), &id_a, &id_b);
    let my_key = me.key.public();
    let hello = PairHello {
        versions: me.versions.clone(),
        device: my_key.id(),
        key: my_key,
        name: name.to_owned(),
        spake: b64(&spake_a),
    };
    framed
        .write
        .write(&LinkFrame::control(Kind::Pair, json!(hello)))
        .await
        .map_err(io)?;
    let frame = read_reply(framed).await?;
    let reply = match frame.kind {
        Kind::Pair => serde_json::from_value::<PairReply>(frame.body)
            .map_err(|e| PairError::Protocol(format!("bad pair reply: {e}")))?,
        Kind::Reject => return Err(refused(frame)),
        other => return Err(PairError::Protocol(format!("expected pair, got {other:?}"))),
    };
    // Relay 의 Ed25519 키는 이 Noise 세션의 상대여야 한다 — 그 키의 지문이 머신 id 가 된다.
    if !bound.is_peer(&reply.key) {
        return Err(PairError::Protocol(
            "the relay's key is not the noise peer".to_owned(),
        ));
    }
    let spake_b = unb64(&reply.spake).ok_or_else(|| PairError::Protocol("bad spake".to_owned()))?;
    let key = Zeroizing::new(
        state
            .finish(&spake_b)
            .map_err(|_| PairError::Protocol("bad spake".to_owned()))?,
    );
    let my_mac = mac(&key, b"desktop", &bound.hash).finalize().into_bytes();
    framed
        .write
        .write(&LinkFrame::control(
            Kind::Confirm,
            json!(Confirm {
                mac: b64(&my_mac),
                role: None,
            }),
        ))
        .await
        .map_err(io)?;
    let frame = read_reply(framed).await?;
    let confirm = match frame.kind {
        Kind::Confirm => serde_json::from_value::<Confirm>(frame.body)
            .map_err(|e| PairError::Protocol(format!("bad confirm: {e}")))?,
        Kind::Reject => return Err(refused(frame)),
        other => {
            return Err(PairError::Protocol(format!(
                "expected confirm, got {other:?}"
            )));
        }
    };
    let proven = unb64(&confirm.mac)
        .is_some_and(|m| mac(&key, b"relay", &bound.hash).verify_slice(&m).is_ok());
    let (true, Some(role)) = (proven, confirm.role) else {
        return Err(PairError::RelayNotProven);
    };
    Ok(CodePaired {
        machine: reply.key.id(),
        relay_key: reply.key,
        role,
    })
}
