//! 핸드셰이크(18-m7-plan M7-03 ③ ⑧ · M7-02 ②).
//!
//! ```text
//! Desktop → Relay  hello   { protocol, app, releaseChannel, device, key, challenge }
//! Relay → Desktop  hello   { protocol, app, releaseChannel, device, key, challenge, signature }   (또는 reject — 판 다름)
//! Desktop → Relay  auth    { relayKey, signature }
//! Relay → Desktop  welcome { role }                                                               (또는 reject)
//! ```
//!
//! - **판**: 링크 프로토콜 판 · 앱 판 · 릴리스 채널이 하나라도 다르면 RPC 를 하나도 받지 않고 끊는다(M7-07 — Desktop 과 정확히
//!   같은 판의 Relay 만 쓴다).
//! - **서명하는 글**([`transcript`]): (링크 프로토콜 판, Desktop 도전값, Relay 도전값, Desktop 공개키, Relay 공개키)에 누가
//!   서명하는지(`desktop` · `relay`)를 붙인 것. 양쪽 도전값이 매 연결 새 난수라 지난 연결의 서명을 다시 내밀 수 없다(재생).
//! - **채널 바인딩**(계획 재리뷰 F-c): Desktop 은 자기가 붙은 Relay 의 공개키를 서명 글에 넣고 `auth.relayKey` 로 밝힌다. Relay 는
//!   `relayKey` 가 **자기 키일 때만** 받는다 — 다른 Relay(A)가 이 Relay(B)의 도전값을 Desktop 에 건네 받아 온 서명은 A 의 키에
//!   묶여 있어 B 가 받지 않는다.
//! - **머신 id**(메인 판정 M7-P6 · 계획 리뷰 F1): Desktop 은 Relay 의 서명을 Relay 공개키로 확인하고, 그 키의 지문이 그 호스트에
//!   기억한 머신 id 와 다르면 서명하지 않고 끊는다.
//! - **role**: Relay 는 서명이 맞고 그 키가 페어링 표에 있을 때만 그 기기의 role 을 준다. 그 밖은 거절하고 끊는다 — 증명 없는 링크는
//!   role 이 없다.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use pw_rpc::Role;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::device::{DeviceId, PublicKey, Signature};

/// 링크 프로토콜 판. 틀 · 핸드셰이크 · 채널 규칙이 바뀌면 올린다.
pub const PROTOCOL: u32 = 1;

/// 서명 글의 머리(다른 프로토콜의 서명과 섞이지 않게).
const DOMAIN: &[u8] = b"pitwall-link-handshake\0";

/// 핸드셰이크에서 견주는 판.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Versions {
    /// 링크 프로토콜 판([`PROTOCOL`]).
    pub protocol: u32,
    /// 앱 판(Desktop 과 Relay 는 같은 판을 낸다).
    pub app: String,
    /// 릴리스 채널(`stable` · `preview` · `dev` 등).
    pub release_channel: String,
}

impl Versions {
    /// 이 빌드의 프로토콜 판에 앱 판 · 채널을 붙인다.
    pub fn new(app: impl Into<String>, release_channel: impl Into<String>) -> Self {
        Self {
            protocol: PROTOCOL,
            app: app.into(),
            release_channel: release_channel.into(),
        }
    }
}

/// 도전값 32바이트.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Challenge(pub [u8; 32]);

impl std::fmt::Debug for Challenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Challenge(..)")
    }
}

impl Serialize for Challenge {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Challenge {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = STANDARD
            .decode(text)
            .map_err(|_| serde::de::Error::custom("challenge is not base64"))?;
        Ok(Self(bytes.try_into().map_err(|_| {
            serde::de::Error::custom("challenge is not 32 bytes")
        })?))
    }
}

/// `hello` 의 본문(양쪽).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    #[serde(flatten)]
    pub versions: Versions,
    /// 보낸 기기의 id(공개키의 지문). `key` 의 지문과 다르면 거절한다.
    pub device: DeviceId,
    pub key: PublicKey,
    pub challenge: Challenge,
    /// Relay 만 — 그 연결의 서명 글([`transcript`] `relay`)에 한 서명.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
}

/// `auth` 의 본문(Desktop → Relay).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Auth {
    /// Desktop 이 서명 글에 넣은 Relay 공개키 — 자기가 붙었다고 믿는 Relay(채널 바인딩).
    pub relay_key: PublicKey,
    pub signature: Signature,
}

/// `welcome` 의 본문.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    pub role: Role,
}

/// 거절의 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RejectReason {
    /// 판이 다르다(링크 프로토콜 · 앱 · 릴리스 채널).
    Version,
    /// 핸드셰이크 틀의 꼴이 틀렸다(순서 · 본문 · 지문).
    Protocol,
    /// 서명이 틀렸다(다른 Relay 에 묶인 서명 포함).
    BadSignature,
    /// 페어링 표에 없는 기기 키.
    Unpaired,
}

/// `reject` 의 본문. 판이 다를 때는 보낸 쪽의 판을 싣는다(M7-07 이 사람에게 보인다).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reject {
    pub reason: RejectReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<Versions>,
}

/// 누가 서명하는가.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Signer {
    Desktop,
    Relay,
}

/// 서명하는 글.
pub(crate) fn transcript(
    signer: Signer,
    protocol: u32,
    desktop_challenge: &Challenge,
    relay_challenge: &Challenge,
    desktop_key: &PublicKey,
    relay_key: &PublicKey,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(DOMAIN.len() + 8 + 4 + 32 * 4);
    out.extend_from_slice(DOMAIN);
    out.extend_from_slice(match signer {
        Signer::Desktop => b"desktop\0",
        Signer::Relay => b"relay\0\0\0",
    });
    out.extend_from_slice(&protocol.to_be_bytes());
    out.extend_from_slice(&desktop_challenge.0);
    out.extend_from_slice(&relay_challenge.0);
    out.extend_from_slice(desktop_key.as_bytes());
    out.extend_from_slice(relay_key.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceKey;

    /// 두 서명자의 글은 다르다 — Relay 의 서명을 Desktop 의 것으로 내밀 수 없다.
    #[test]
    fn the_signers_sign_different_texts() {
        let (d, r) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        let (dc, rc) = (Challenge([1; 32]), Challenge([2; 32]));
        let desktop = transcript(
            Signer::Desktop,
            PROTOCOL,
            &dc,
            &rc,
            &d.public(),
            &r.public(),
        );
        let relay = transcript(Signer::Relay, PROTOCOL, &dc, &rc, &d.public(), &r.public());
        assert_ne!(desktop, relay);
        let sig = r.sign(&relay);
        assert!(r.public().verifies(&relay, &sig));
        assert!(!r.public().verifies(&desktop, &sig));
    }
}
