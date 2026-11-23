//! Noise 세션(18-m7-plan M7-23 ① · 08 「보안 모델」): 직접 연결(QUIC — M7-24)과 코드 페어링은 이 위에서만 돈다.
//!
//! - 패턴은 [`PATTERN`](`Noise_XX_25519_ChaChaPoly_BLAKE2s`). 양쪽의 정적 키는 **기기 Ed25519 키를 X25519 로 바꾼 것**이다
//!   (`ed25519-dalek` 의 `to_scalar_bytes` · `to_montgomery` — 기기 키 하나로 서명과 Noise 를 함께 한다. 서명과 DH 에 한 키를 쓰는
//!   것의 안전성은 eprint 2021/509). XX 라 처음 붙는 상대의 키도 핸드셰이크가 증명한다.
//! - Noise 위 링크 프로토콜은 M7-03 그대로다(`hello` · `auth` · `welcome`). 다만 **안쪽 `hello` 의 Ed25519 키가 Noise 상대 정적 키와
//!   같아야** 링크를 받는다([`Bound`] — `dial` · `serve` 가 본다). 그래서 털린 Relay 가 다른 Relay 의 `hello` 를 그대로 중계해도
//!   (M7-03 리뷰 권고 3) Desktop 은 Noise 상대(중계자)와 다른 키의 `hello` 를 거절한다 — 중계자는 세션 키가 없어 틀을 끼워 넣지 못한다.
//! - 코드 페어링(`crate::pair`)의 PAKE 대본은 [`Bound::hash`](Noise 핸드셰이크 해시)에 묶인다 — 다른 세션으로 옮겨 쓰지 못한다.
//! - 전송 틀: Noise 메시지 하나 = 2바이트 길이(빅 엔디언) + 메시지(65535 바이트까지). 링크 틀(최대 64 MiB)은 조각으로 나눠 실린다.
//!   복호가 한 번이라도 틀리면 그 전송을 닫는다.
//! - SSH 경로(`pitwall-relay proxy` 의 stdio)는 이 층을 쓰지 않는다 — ssh 가 이미 암호화 · 호스트 인증을 하고, 기기 키는 M7-03 의
//!   도전 서명으로 증명한다.

use std::io;
use std::sync::Arc;

use pw_core::BoxFuture;
use snow::{Builder, HandshakeState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

use crate::device::{DeviceKey, PublicKey};
use crate::transport::{BoxRead, BoxWrite, LinkStream, Transport};

/// Noise 패턴.
pub const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// 프롤로그(다른 프로토콜의 Noise 세션과 섞이지 않게 — 링크 프로토콜 판을 함께 싣는다).
fn prologue() -> Vec<u8> {
    let mut out = b"pitwall-link-noise\0".to_vec();
    out.extend_from_slice(&crate::handshake::PROTOCOL.to_be_bytes());
    out
}

/// Noise 메시지의 상한(Noise 규격).
const MAX_MESSAGE: usize = 65535;
/// AEAD 꼬리표.
const TAG: usize = 16;
/// 한 메시지에 싣는 평문의 상한.
const CHUNK: usize = MAX_MESSAGE - TAG;
/// [`SecuredTransport`] 의 Noise 핸드셰이크 시한(받는 쪽은 `LinkLimits::dead_after` 안 — `crate::relay::serve_noise`).
pub const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// 평문 쪽 duplex 의 한 방향 버퍼.
const PIPE: usize = 256 * 1024;

/// Noise 세션이 증명한 상대와 그 세션.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    /// 상대의 정적 X25519 공개키.
    pub peer_static: [u8; 32],
    /// 핸드셰이크 해시(세션마다 다르다 — PAKE 대본에 묶는다).
    pub hash: [u8; 32],
}

impl Bound {
    /// `key`(안쪽 `hello` 의 Ed25519 키)가 이 세션의 상대 정적 키인가.
    pub fn is_peer(&self, key: &PublicKey) -> bool {
        key.x25519().is_some_and(|x| x == self.peer_static)
    }
}

impl std::fmt::Debug for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Bound(..)")
    }
}

fn builder(key: &DeviceKey) -> io::Result<(Builder<'static>, Zeroizing<[u8; 32]>)> {
    let params = PATTERN
        .parse()
        .map_err(|e: snow::Error| io::Error::other(e.to_string()))?;
    Ok((Builder::new(params), key.x25519_secret()))
}

fn noise_err(e: snow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("noise: {e}"))
}

async fn write_message(write: &mut BoxWrite, message: &[u8]) -> io::Result<()> {
    let len = u16::try_from(message.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "noise message too long"))?;
    write.write_all(&len.to_be_bytes()).await?;
    write.write_all(message).await?;
    write.flush().await
}

/// 다음 메시지. 메시지 사이에서 스트림이 끝나면 `None`.
async fn read_message(read: &mut BoxRead, buf: &mut Vec<u8>) -> io::Result<Option<()>> {
    let mut len = [0u8; 2];
    match read.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    buf.resize(usize::from(u16::from_be_bytes(len)), 0);
    read.read_exact(buf).await?;
    Ok(Some(()))
}

async fn expect_message(read: &mut BoxRead, buf: &mut Vec<u8>) -> io::Result<()> {
    read_message(read, buf).await?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "closed during the noise handshake",
        )
    })
}

/// 붙는 쪽(Desktop): `stream` 위에서 Noise XX 를 하고 암호화된 스트림을 돌려준다. 시한은 부르는 쪽이 건다.
pub async fn initiate(stream: LinkStream, key: &DeviceKey) -> io::Result<LinkStream> {
    let (builder, secret) = builder(key)?;
    let prologue = prologue();
    let state = builder
        .local_private_key(secret.as_ref())
        .and_then(|b| b.prologue(&prologue))
        .and_then(Builder::build_initiator)
        .map_err(noise_err)?;
    run(stream, state, true).await
}

/// 받는 쪽(Relay).
pub async fn respond(stream: LinkStream, key: &DeviceKey) -> io::Result<LinkStream> {
    let (builder, secret) = builder(key)?;
    let prologue = prologue();
    let state = builder
        .local_private_key(secret.as_ref())
        .and_then(|b| b.prologue(&prologue))
        .and_then(Builder::build_responder)
        .map_err(noise_err)?;
    run(stream, state, false).await
}

async fn run(
    stream: LinkStream,
    mut state: HandshakeState,
    initiator: bool,
) -> io::Result<LinkStream> {
    let LinkStream {
        mut read,
        mut write,
        ..
    } = stream;
    let mut out = vec![0u8; MAX_MESSAGE];
    let mut inbound = Vec::new();
    let mut scratch = vec![0u8; MAX_MESSAGE];
    // XX: → e · ← e, ee, s, es · → s, se. 페이로드는 비운다(안쪽 링크 핸드셰이크가 판 · 기기 id 를 싣는다).
    let mut my_turn = initiator;
    while !state.is_handshake_finished() {
        if my_turn {
            let n = state.write_message(&[], &mut out).map_err(noise_err)?;
            write_message(&mut write, &out[..n]).await?;
        } else {
            expect_message(&mut read, &mut inbound).await?;
            state
                .read_message(&inbound, &mut scratch)
                .map_err(noise_err)?;
        }
        my_turn = !my_turn;
    }
    let peer_static: [u8; 32] = state
        .get_remote_static()
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "noise: no remote static key"))?;
    let hash: [u8; 32] = state
        .get_handshake_hash()
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "noise: bad handshake hash"))?;
    let transport = Arc::new(state.into_stateless_transport_mode().map_err(noise_err)?);

    let (ours, theirs) = tokio::io::duplex(PIPE);
    let (mut plain_read, mut plain_write) = tokio::io::split(ours);
    // 평문 → 암호문(보내기).
    {
        let transport = Arc::clone(&transport);
        tokio::spawn(async move {
            let mut nonce = 0u64;
            let mut chunk = vec![0u8; CHUNK];
            let mut message = vec![0u8; MAX_MESSAGE];
            loop {
                let n = match plain_read.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let Ok(len) = transport.write_message(nonce, &chunk[..n], &mut message) else {
                    break;
                };
                nonce += 1;
                if write_message(&mut write, &message[..len]).await.is_err() {
                    break;
                }
            }
            let _ = write.shutdown().await;
        });
    }
    // 암호문 → 평문(받기). 복호가 틀리면 닫는다.
    tokio::spawn(async move {
        let mut nonce = 0u64;
        let mut message = Vec::new();
        let mut plain = vec![0u8; MAX_MESSAGE];
        while let Ok(Some(())) = read_message(&mut read, &mut message).await {
            let Ok(n) = transport.read_message(nonce, &message, &mut plain) else {
                tracing::info!(target: "pw_link", "dropped a noise session: a message did not decrypt");
                break;
            };
            nonce += 1;
            if plain_write.write_all(&plain[..n]).await.is_err() {
                break;
            }
        }
        let _ = plain_write.shutdown().await;
    });

    let mut secured = LinkStream::split(theirs);
    secured.bound = Some(Bound { peer_static, hash });
    Ok(secured)
}

/// 전송 위에 Noise 를 씌운 전송(직접 연결 — M7-24 의 `Quic` 을 감싼다). 여는 때마다 이 기기 키로 Noise XX 를 한다. `RemoteHost`
/// 가 다시 붙을 때도 같은 길이다.
pub struct SecuredTransport<T> {
    inner: T,
    key: Arc<DeviceKey>,
}

impl<T: Transport> SecuredTransport<T> {
    pub fn new(inner: T, key: Arc<DeviceKey>) -> Self {
        Self { inner, key }
    }
}

impl<T: Transport> Transport for SecuredTransport<T> {
    fn describe(&self) -> String {
        format!("noise over {}", self.inner.describe())
    }

    fn open(&self) -> BoxFuture<'_, io::Result<LinkStream>> {
        Box::pin(async move {
            let stream = self.inner.open().await?;
            tokio::time::timeout(HANDSHAKE_TIMEOUT, initiate(stream, &self.key))
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "the noise handshake timed out")
                })?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 두 끝이 Noise 를 마치면 서로의 기기 키를 상대 정적 키로 보고, 같은 핸드셰이크 해시를 가지며, 큰 바이트도 그대로 오간다.
    #[tokio::test]
    async fn both_ends_see_each_others_device_key_and_carry_large_writes() {
        let (desktop, relay) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        let (a, b) = tokio::io::duplex(4096);
        let (left, right) = tokio::join!(
            initiate(LinkStream::split(a), &desktop),
            respond(LinkStream::split(b), &relay)
        );
        let (mut left, mut right) = (left.unwrap(), right.unwrap());
        let (lb, rb) = (left.bound.unwrap(), right.bound.unwrap());
        assert!(lb.is_peer(&relay.public()));
        assert!(rb.is_peer(&desktop.public()));
        assert!(!lb.is_peer(&desktop.public()));
        assert_eq!(lb.hash, rb.hash);

        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let sent = big.clone();
        let writer = tokio::spawn(async move {
            left.write.write_all(&sent).await.unwrap();
            left.write.shutdown().await.unwrap();
            left
        });
        let mut got = Vec::new();
        right.read.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, big);
        writer.await.unwrap();
    }

    /// 전송 위의 바이트를 한 자리 바꾸면 받는 쪽은 그 세션을 닫는다(끼워 넣기 · 바꿔치기 없음).
    #[tokio::test]
    async fn a_tampered_message_closes_the_session() {
        let (desktop, relay) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        // Desktop ↔ (가운데: 핸드셰이크 뒤 첫 암호문의 한 바이트를 뒤집는다) ↔ Relay.
        let (a, mid_a) = tokio::io::duplex(1 << 20);
        let (mid_b, b) = tokio::io::duplex(1 << 20);
        let (mut ma_r, mut ma_w) = tokio::io::split(mid_a);
        let (mut mb_r, mut mb_w) = tokio::io::split(mid_b);
        tokio::spawn(async move {
            // Desktop → Relay 방향: 메시지 단위로 옮기고, 셋째(그 방향의 핸드셰이크 둘 뒤 첫 전송 메시지)를 바꾼다.
            let mut count = 0;
            loop {
                let mut len = [0u8; 2];
                if ma_r.read_exact(&mut len).await.is_err() {
                    break;
                }
                let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
                ma_r.read_exact(&mut msg).await.unwrap();
                count += 1;
                if count == 3 {
                    msg[0] ^= 1;
                }
                mb_w.write_all(&len).await.unwrap();
                mb_w.write_all(&msg).await.unwrap();
            }
        });
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut mb_r, &mut ma_w).await;
        });
        let (left, right) = tokio::join!(
            initiate(LinkStream::split(a), &desktop),
            respond(LinkStream::split(b), &relay)
        );
        let (mut left, mut right) = (left.unwrap(), right.unwrap());
        left.write.write_all(b"hello").await.unwrap();
        left.write.flush().await.unwrap();
        let mut got = Vec::new();
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            right.read.read_to_end(&mut got),
        )
        .await
        .expect("the session did not close");
        assert!(read.is_ok());
        assert!(got.is_empty(), "a tampered message reached the link");
    }
}
