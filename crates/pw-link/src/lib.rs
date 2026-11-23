//! `pw-link`: Desktop 과 원격 머신의 Relay 사이의 링크(18-m7-plan M7-03 · M7-02 ② — `docs/design/08-remote-mobile.md`).
//!
//! 원격 기능은 새 기능이 아니라 이미 있는 Host RPC 를 다른 머신의 Host 로 보내는 것이다(03-architecture.md 「전체 구조」). 링크는
//! 그 길이다:
//!
//! - [`transport`]: 바이트 스트림 하나를 연다(`Ssh` — M7-06, 시험용 [`transport::memory`], `Quic` — M7-24).
//! - [`frame`]: `pw-wire` 의 `Content-Length` 틀에 JSON 머리(`LinkFrame { channel, kind, body }`).
//! - [`handshake`]: 판 · 기기 id · 양쪽 도전 서명(채널 바인딩). 판이 다르거나 증명이 없으면 RPC 를 하나도 받지 않고 끊는다.
//! - [`device`] · [`pairing`]: 기기 키(Desktop 은 키체인, Relay 는 `$DATA/relay/device.key`)와 Relay 의 페어링 표.
//! - 다중화(`mux`): 링크 하나에 채널 여럿 — 채널 수 상한, 채널마다 흐름 제어 · 보내기 버퍼 상한, 하트비트.
//! - [`desktop`]: [`dial`] · [`RemoteHost`](`pw_core::Host` 를 구현해 `HostRouter` 에 붙는다) · 링크 상태.
//! - [`relay`]: [`serve`] — 증명된 Desktop 의 채널을 이 머신의 Host 연결로 잇고, 호출자를 링크 기기의 role 로 자른다.
//! - 이어 붙기(M7-09 ①): 전송을 잃은 링크는 시한 동안 채널을 쥐고, Desktop([`RemoteHost`])이 같은 링크의 토큰으로 다시 붙으면 잃은
//!   틀 없이 이어 간다(`mux` 머리 주석 「이어 붙기」).
//! - `ssh` · `install`(기능 `bootstrap` — Desktop 만): 시스템 `ssh` 로 원격 명령 · 링크 전송, Relay 설치 · SSH 자동 페어링(M7-06).
//! - `upgrade`(기능 `bootstrap`): 판 바꾸기 · 정리 — 옛 판 데몬 조사 · [옛 판 끝내기] · 최근 두 판만 남기기(M7-07).

mod claim;
pub mod desktop;
pub mod device;
pub mod frame;
pub mod handshake;
mod mux;
pub mod pairing;
pub mod relay;
pub mod transport;

#[cfg(all(unix, feature = "bootstrap", any(test, feature = "test-support")))]
pub mod fake_ssh;
#[cfg(feature = "bootstrap")]
pub mod install;
#[cfg(feature = "bootstrap")]
pub mod remote_hosts;
#[cfg(feature = "bootstrap")]
pub mod ssh;
#[cfg(feature = "bootstrap")]
pub mod upgrade;

pub use desktop::{DesktopIdentity, DesktopLink, DialError, Expect, LinkState, RemoteHost, dial};
pub use device::{
    DeviceId, DeviceKey, DeviceKeyStore, KeychainKeyStore, MachineId, PublicKey, RelayKeyFile,
};
pub use frame::HANDSHAKE_MAX_FRAME;
pub use handshake::{PROTOCOL, RejectReason, Versions};
pub use mux::{CloseReason, LinkLimits};
pub use pairing::{Pairing, PairingTable};
pub use relay::{
    Proven, RelayIdentity, RelayParts, Resumes, ServeError, serve, serve_framed, serve_proven,
};
pub use transport::{LinkStream, Transport};

#[cfg(test)]
mod tests;
