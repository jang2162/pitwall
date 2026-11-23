//! 링크 틀(18-m7-plan M7-03 ② · 메인 판정 M7-P2): `pw-wire` 의 `Content-Length` 틀 하나에 JSON 한 건 —
//! `{ "ch": <채널>, "kind": <종류>, "body": <본문> }`.
//!
//! - 채널 [`CONTROL`](0)은 링크 자체의 것(핸드셰이크 · 하트비트), 1 이상은 논리 연결(`Host::connect` 하나)이다.
//! - 틀의 상한 · 머리 규칙은 `pw-wire` 그대로다. 틀 오류(상한 초과 · 잘린 틀)와 **모르는 종류 · JSON 이 아닌 본문은 링크를 끊는다**
//!   — 링크의 상대는 우리가 만든 프로그램 하나뿐이라 모르는 것을 건너뛸 까닭이 없고, 건너뛰면 틀 경계가 어긋난 채 RPC 가 지난다.

use std::fmt;
use std::io;

use pw_wire::{FrameReader, write_frame};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::transport::{BoxRead, BoxWrite, LinkStream};

/// 링크 자체의 채널.
pub const CONTROL: u32 = 0;

/// 증명 전(첫 틀 · 핸드셰이크) 틀 본문의 상한(18-m7-plan M7-04 — 출발값). 상대를 아직 모르는 동안 64 MiB 본문 하나로 메모리를
/// 잡지 못하게 한다. 증명 뒤에는 링크 틀 상한(`pw_wire::MAX_BODY_BYTES`)으로 올린다. 핸드셰이크 틀은 1 KiB 안팎이다.
pub const HANDSHAKE_MAX_FRAME: usize = 64 * 1024;

/// 틀의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// 핸드셰이크의 첫 틀(양쪽). 데몬 소켓은 첫 틀의 종류로 링크와 좁은 명령(M7-04)을 가른다.
    Hello,
    /// Desktop 의 도전 서명.
    Auth,
    /// Relay 가 Desktop 키를 받았다 — 본문은 그 기기의 role.
    Welcome,
    /// 핸드셰이크 거절. 보낸 쪽은 곧 끊는다.
    Reject,
    /// 하트비트(양쪽 — [`crate::LinkLimits::ping_every`]).
    Ping,
    Pong,
    /// Desktop → Relay: 채널 하나를 연다(본문은 호출자의 주장).
    Connect,
    /// Relay → Desktop: 채널을 받았다.
    Accepted,
    /// Relay → Desktop: 채널을 받지 않았다(본문은 까닭).
    Refused,
    /// 채널의 JSON-RPC 메시지 하나.
    Msg,
    /// 채널을 닫는다(양쪽).
    Close,
    /// 받는 쪽이 보내는 쪽에 더 보내도 되는 바이트를 준다(흐름 제어).
    Credit,
    /// 보내는 쪽이 채널 창보다 큰 틀 하나를 보내려고 창을 더 청한다(본문은 그 틀의 바이트). 받는 쪽은 링크 받기 예산에서 빌려 주거나
    /// 그 채널을 닫는다.
    Want,
    /// 받은 번호 있는 틀의 수(본문 `seq` — 이어 붙기의 다시 보낼 줄을 비운다, 18-m7-plan M7-09 ①).
    Ack,
    /// 링크를 일부러 닫는다 — 받는 쪽은 이어 붙기를 기다리지 않고 닫는다.
    Bye,
    /// Desktop → Relay: 채널 하나를 포트 포워딩으로 연다(본문 [`crate::forward::ForwardTo`] — 18-m7-plan M7-22). 답은 `connect` 와
    /// 같이 `accepted` · `refused` 이고, 그 뒤 채널의 `msg` 는 TCP 바이트다([`crate::forward`]). Relay 는 Desktop 쪽으로 보내지 않는다.
    Forward,
    /// 코드 페어링의 PAKE 메시지(양쪽 — `crate::pair`, Noise 세션의 첫 틀에서만).
    Pair,
    /// 코드 페어링의 키 확인(양쪽).
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkFrame {
    #[serde(rename = "ch")]
    pub channel: u32,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub body: Value,
}

impl LinkFrame {
    pub fn new(channel: u32, kind: Kind, body: Value) -> Self {
        Self {
            channel,
            kind,
            body,
        }
    }

    pub fn control(kind: Kind, body: Value) -> Self {
        Self::new(CONTROL, kind, body)
    }

    /// 틀 본문의 바이트. 값은 우리가 만든 것이라 직렬화가 실패하지 않는다.
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum FrameError {
    /// `Content-Length` 틀 오류(상한 초과 · 머리 · 잘림 · 읽기).
    Wire(pw_wire::FrameError),
    /// 본문이 링크 틀이 아니다(JSON 이 아니거나 모르는 종류).
    Malformed(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wire(e) => write!(f, "link frame: {e}"),
            Self::Malformed(e) => write!(f, "malformed link frame: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// 틀을 하나씩 읽는다.
pub struct FramedRead {
    inner: FrameReader<BoxRead>,
}

impl FramedRead {
    /// 다음 틀과 그 본문의 바이트 수. 틀 사이에서 스트림이 끝나면 `None`.
    pub async fn read(&mut self) -> Result<Option<(LinkFrame, usize)>, FrameError> {
        let Some(body) = self.inner.read_frame().await.map_err(FrameError::Wire)? else {
            return Ok(None);
        };
        let frame = serde_json::from_slice::<LinkFrame>(&body)
            .map_err(|e| FrameError::Malformed(e.to_string()))?;
        Ok(Some((frame, body.len())))
    }

    /// 다음 틀을 JSON 값으로(링크 틀의 꼴을 보지 않는다). 데몬은 첫 틀의 `kind` 로 링크와 좁은 명령을 가른다(M7-04) — 좁은 명령의
    /// 종류는 [`Kind`] 에 없다. JSON 이 아니면 [`FrameError::Malformed`].
    pub async fn read_value(&mut self) -> Result<Option<(Value, usize)>, FrameError> {
        let Some(body) = self.inner.read_frame().await.map_err(FrameError::Wire)? else {
            return Ok(None);
        };
        let value = serde_json::from_slice::<Value>(&body)
            .map_err(|e| FrameError::Malformed(e.to_string()))?;
        Ok(Some((value, body.len())))
    }

    /// 다음 틀부터의 본문 상한(`pw_wire::MAX_BODY_BYTES` 위로는 못 올린다).
    pub fn set_max_body(&mut self, max_body: usize) {
        self.inner.set_max_body(max_body);
    }

    pub fn max_body(&self) -> usize {
        self.inner.max_body()
    }

    /// [`Self::read`] 와 같되 본문을 풀지 않는다(원시 JSON 그대로 — 꼴만 확인한다).
    pub(crate) async fn read_raw(&mut self) -> Result<Option<(RawFrame, usize)>, FrameError> {
        let Some(body) = self.inner.read_frame().await.map_err(FrameError::Wire)? else {
            return Ok(None);
        };
        let frame = serde_json::from_slice::<RawFrame>(&body)
            .map_err(|e| FrameError::Malformed(e.to_string()))?;
        Ok(Some((frame, body.len())))
    }
}

/// 본문을 풀지 않은 틀. 채널 메시지는 받는 쪽이 꺼낼 때까지 이 꼴로 둔다 — 푼 `serde_json::Value` 는 와이어 바이트보다 몇 배 커서
/// (원소 하나에 수십 바이트) 받기 예산이 메모리 상한이 되지 못한다.
#[derive(Debug, Deserialize)]
pub(crate) struct RawFrame {
    #[serde(rename = "ch")]
    pub channel: u32,
    pub kind: Kind,
    #[serde(default)]
    pub body: Option<Box<RawValue>>,
}

impl RawFrame {
    /// 본문을 푼 틀(링크 틀 — 작다).
    pub(crate) fn into_frame(self) -> Result<LinkFrame, String> {
        let body = match self.body {
            Some(raw) => serde_json::from_str(raw.get()).map_err(|e| e.to_string())?,
            None => Value::Null,
        };
        Ok(LinkFrame::new(self.channel, self.kind, body))
    }
}

/// 틀을 써 내보낸다.
pub struct FramedWrite {
    pub(crate) inner: BoxWrite,
}

impl FramedWrite {
    pub async fn write(&mut self, frame: &LinkFrame) -> io::Result<()> {
        self.write_encoded(&frame.encode()).await
    }

    /// [`LinkFrame::encode`] 한 바이트를 그대로 쓴다.
    pub async fn write_encoded(&mut self, body: &[u8]) -> io::Result<()> {
        write_frame(&mut self.inner, body).await
    }
}

/// 스트림 하나를 틀 단위로.
pub struct Framed {
    pub read: FramedRead,
    pub write: FramedWrite,
}

impl Framed {
    /// 틀 상한이 링크 틀 상한(`pw_wire::MAX_BODY_BYTES`)인 스트림 — 이미 증명된 상대와의 것.
    pub fn new(stream: LinkStream) -> Self {
        Self::with_max_body(stream, pw_wire::MAX_BODY_BYTES)
    }

    /// 아직 증명하지 않은 상대의 스트림 — 틀 상한 [`HANDSHAKE_MAX_FRAME`]. 증명 뒤에 링크가 올린다.
    pub fn handshake(stream: LinkStream) -> Self {
        Self::with_max_body(stream, HANDSHAKE_MAX_FRAME)
    }

    fn with_max_body(stream: LinkStream, max_body: usize) -> Self {
        Self {
            read: FramedRead {
                inner: FrameReader::with_max_body(stream.read, max_body),
            },
            write: FramedWrite {
                inner: stream.write,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_format() {
        let frame = LinkFrame::new(3, Kind::Msg, json!({"jsonrpc": "2.0", "id": 1}));
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            json!({"ch": 3, "kind": "msg", "body": {"jsonrpc": "2.0", "id": 1}})
        );
        assert_eq!(
            serde_json::to_value(LinkFrame::control(Kind::Ping, Value::Null)).unwrap(),
            json!({"ch": 0, "kind": "ping"})
        );
        assert!(serde_json::from_value::<LinkFrame>(json!({"ch": 0, "kind": "shell"})).is_err());
    }
}
