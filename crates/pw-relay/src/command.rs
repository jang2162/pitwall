//! 데몬 소켓의 좁은 명령(18-m7-plan M7-04 — 계획 재리뷰 RB1). 연결의 첫 틀이 `hello` 면 링크(`pw_link::serve_proven`)이고, 첫 틀의
//! 종류가 [`NARROW_COMMANDS`] 에 있으면 좁은 명령이다. 그 밖의 첫 틀은 거절하고 끊는다.
//!
//! 좁은 명령은 **이 셋뿐**이다 — 원격 머신의 같은 사용자 프로세스(그 머신의 git askpass · `pitwall-relay open` · 상태 보기)가 기기 키
//! 없이 부르는 길이라, role · 범위를 얻거나 페어링 표를 바꾸는 명령을 두지 않는다. 좁은 명령의 연결은 요청 틀 하나에 답 틀 하나를
//! 보내고 닫는다 — 그 연결로는 RPC 도 링크 틀도 지나지 않는다.
//!
//! 틀은 링크와 같은 `Content-Length` 틀(첫 틀 상한 `pw_link::HANDSHAKE_MAX_FRAME`)에 JSON 하나:
//! 요청 `{"kind": "status", "body": …}` → 답 `{"kind": "result", "body": …}` 또는 `{"kind": "error", "body": {"message": …}}`.
//!
//! `open` · `askpass` 의 몸통은 M7-15 가 [`NarrowHandler`] 로 넣는다. 그 전에는 「아직 없음」 오류로 답한다.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 좁은 명령.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Command {
    /// 원격 터미널 · 에이전트가 그 머신의 파일을 Desktop 창에 연다(M7-15).
    Open,
    /// 원격 git 의 암호 질문을 붙은 Desktop 에 묻는다(M7-12 · M7-15).
    Askpass,
    /// 데몬의 판 · 붙은 링크 · 남은 일(이 행).
    Status,
}

/// 첫 틀의 종류 → 좁은 명령. 이 표에 넷째를 더하려면 그 명령이 role · 범위 · 페어링 표에 닿지 않는다는 것을 먼저 보인다.
pub const NARROW_COMMANDS: &[(&str, Command)] = &[
    ("askpass", Command::Askpass),
    ("open", Command::Open),
    ("status", Command::Status),
];

impl Command {
    /// 첫 틀의 `kind`.
    pub fn from_kind(kind: &str) -> Option<Self> {
        NARROW_COMMANDS
            .iter()
            .find(|(name, _)| *name == kind)
            .map(|(_, command)| *command)
    }

    pub fn name(self) -> &'static str {
        NARROW_COMMANDS
            .iter()
            .find(|(_, command)| *command == self)
            .map_or("?", |(name, _)| name)
    }
}

/// 좁은 명령의 답.
pub type Reply = Result<Value, String>;
pub type BoxReply<'a> = Pin<Box<dyn Future<Output = Reply> + Send + 'a>>;

/// `open` · `askpass` 의 몸통(M7-15 가 넣는다). `status` 는 데몬이 직접 답한다.
pub trait NarrowHandler: Send + Sync + 'static {
    fn open(&self, body: Value) -> BoxReply<'_>;
    fn askpass(&self, body: Value) -> BoxReply<'_>;
}

/// 아직 몸통이 없는 처리기 — 「이 Relay 에는 아직 없다」 로 답한다.
pub struct NotYet;

impl NarrowHandler for NotYet {
    fn open(&self, _body: Value) -> BoxReply<'_> {
        Box::pin(async { Err("`open` is not available on this relay yet".to_owned()) })
    }

    fn askpass(&self, _body: Value) -> BoxReply<'_> {
        Box::pin(async { Err("`askpass` is not available on this relay yet".to_owned()) })
    }
}

/// 첫 틀(요청).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub kind: String,
    #[serde(default)]
    pub body: Value,
}

/// 답 틀의 본문.
pub fn reply_frame(reply: &Reply) -> Value {
    match reply {
        Ok(body) => json!({ "kind": "result", "body": body }),
        Err(message) => json!({ "kind": "error", "body": { "message": message } }),
    }
}

/// `status` 의 답.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: String,
    pub release_channel: String,
    pub name: Option<String>,
    pub pid: u32,
    /// 데몬이 뜬 뒤 지난 시간(ms).
    pub uptime_ms: u64,
    /// 증명된 링크 수와 그중 owner 링크 수.
    pub links: usize,
    pub owner_links: usize,
    pub runs: usize,
    pub terminals: usize,
}

/// 데몬에 좁은 명령 하나를 보내고 답을 받는다(`pitwall-relay status` — M7-15 의 `askpass` · `open` 하위 명령도 이것을 쓴다).
pub async fn request(
    socket: &std::path::Path,
    uid: u32,
    command: Command,
    body: Value,
) -> std::io::Result<Reply> {
    let stream = crate::socket::connect(socket, uid).await?;
    request_on(pw_link::LinkStream::split(stream), command, body).await
}

/// [`request`] 의 몸통(이미 연 스트림).
pub async fn request_on(
    stream: pw_link::LinkStream,
    command: Command,
    body: Value,
) -> std::io::Result<Reply> {
    let mut framed = pw_link::frame::Framed::handshake(stream);
    let request = json!({ "kind": command.name(), "body": body });
    framed
        .write
        .write_encoded(&serde_json::to_vec(&request).map_err(std::io::Error::other)?)
        .await?;
    let Some((answer, _)) = framed
        .read
        .read_value()
        .await
        .map_err(std::io::Error::other)?
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "the relay daemon closed without an answer",
        ));
    };
    Ok(parse_reply(answer))
}

/// 답 틀을 푼다.
pub fn parse_reply(answer: Value) -> Reply {
    match answer.get("kind").and_then(Value::as_str) {
        Some("result") => Ok(answer.get("body").cloned().unwrap_or(Value::Null)),
        Some("error") => Err(answer["body"]["message"]
            .as_str()
            .unwrap_or("the relay refused the command")
            .to_owned()),
        _ => Err("the relay answered with a malformed frame".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 좁은 명령은 `open` · `askpass` · `status` 셋뿐이다(18-m7-plan M7-04 시험 ⑦ — 넷째를 더하면 이 시험이 실패한다). 링크 틀의
    /// 종류와 겹치지 않는다 — 겹치면 첫 틀로 가르지 못한다.
    #[test]
    fn the_narrow_commands_are_exactly_these() {
        let names: Vec<&str> = NARROW_COMMANDS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["askpass", "open", "status"]);
        for (name, command) in NARROW_COMMANDS {
            assert_eq!(Command::from_kind(name), Some(*command));
            assert_eq!(command.name(), *name);
            assert_eq!(serde_json::to_value(command).unwrap(), json!(name));
            let as_link =
                serde_json::from_value::<pw_link::frame::LinkFrame>(json!({"ch": 0, "kind": name}));
            assert!(as_link.is_err(), "`{name}` is also a link frame kind");
        }
        for not in ["hello", "connect", "msg", "pair", "auth", "welcome", ""] {
            assert_eq!(Command::from_kind(not), None, "{not}");
        }
    }
}
