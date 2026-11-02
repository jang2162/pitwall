//! 가짜 언어 서버의 시나리오 형식(15-m4-plan.md M4-01 ⑥). 서버 자체는 `src/bin/pw-lsp-fake.rs`(시험 전용 바이너리 — 배포물에 들지
//! 않는다)이고, 시험은 [`Scenario`] 를 JSON 파일로 써서 그 경로를 첫 인자로 준다:
//!
//! ```json
//! {
//!   "capabilities": { "hoverProvider": true },
//!   "replies": {
//!     "textDocument/hover": { "result": { "contents": "x" }, "delayMs": 50 },
//!     "shutdown": { "noReply": true }
//!   },
//!   "on": { "initialized": [{ "do": "notify", "method": "$/progress", "params": {} }] },
//!   "start": [{ "do": "stderr", "text": "booting" }],
//!   "record": "/tmp/received.jsonl",
//!   "ignoreExit": true
//! }
//! ```
//!
//! - **답**(`replies`, 메서드 이름별): `result`(없으면 `null`) 또는 `error`(`{code, message}`), `delayMs`(그만큼 뒤에 답 — 그사이
//!   다른 메시지도 읽는다), `noReply`(답하지 않는다), `then`(답한 뒤 할 일). `replies` 에 없는 요청의 기본 답: `initialize` →
//!   `{capabilities, serverInfo}`, `shutdown` → `null`, 그 밖 → `MethodNotFound`.
//! - **차례 답**(`sequence`, 메서드 이름별 답 목록): 그 메서드의 n 번째 요청(0 부터)은 목록의 n 번째 답, 다 쓰면 `replies` 를 따른다
//!   (같은 요청에 처음과 나중의 답이 달라야 할 때 — M4-06 의 pull `unchanged`).
//! - **할 일**(`on` — 받은 요청 · 알림의 메서드 이름별, `start` — 뜨자마자): [`Action`] 의 목록을 차례로 한다 — 알림 · 서버 → 클라이언트
//!   요청 보내기(id 는 `"fake-1"` 부터), stderr 한 줄, 틀 없이 바이트 그대로 쓰기(틀 상한 시험), 멈추기, 끝내기(죽기), 같은 프로세스
//!   그룹에 오래 사는 자식 띄우기, stderr · 서버 요청 쏟기와 stdin 읽기 멈추기(망가진 서버 — Host 메모리 상한 시험).
//! - **기록**(`record`): 받은 메시지마다 JSON 한 줄(클라이언트의 요청 · 알림 · 서버 요청에 대한 응답 그대로)을 덧붙인다. 띄운 자식의
//!   pid 는 `{"childPid": n}` 줄로 남는다.
//! - **끝**: `exit` 알림이나 stdin 의 끝에서 0 으로 끝난다. `ignoreExit` 면 둘 다 무시하고 죽을 때까지 돈다(SIGKILL 시험).
//!
//! 새 메서드의 응답 꼴이 필요하면 `replies` 로 대부분 된다. 가짜 서버에 새 동작이 필요하면 [`Action`] 에 변형을 더하고, 기존
//! 시나리오가 그대로 도는지 본다(15 「공유 부품의 주인」 — `deny_unknown_fields` 라 모르는 키는 시나리오 읽기에서 실패한다).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scenario {
    /// `initialize` 응답의 `capabilities`.
    #[serde(default)]
    pub capabilities: Value,
    /// `initialize` 응답의 `serverInfo`.
    #[serde(default)]
    pub server_info: Option<Value>,
    #[serde(default)]
    pub replies: BTreeMap<String, Reply>,
    /// 메서드마다 차례로 쓰는 답(다 쓰면 [`Self::replies`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sequence: BTreeMap<String, Vec<Reply>>,
    #[serde(default)]
    pub on: BTreeMap<String, Vec<Action>>,
    #[serde(default)]
    pub start: Vec<Action>,
    #[serde(default)]
    pub record: Option<PathBuf>,
    #[serde(default)]
    pub ignore_exit: bool,
}

/// 요청 하나의 답.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reply {
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<Value>,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default)]
    pub no_reply: bool,
    #[serde(default)]
    pub then: Vec<Action>,
}

/// 가짜 서버가 하는 일 하나.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    /// 알림을 보낸다.
    Notify {
        method: String,
        #[serde(default)]
        params: Value,
    },
    /// 서버 → 클라이언트 요청을 보낸다. 응답은 `record` 에 남는다.
    Request {
        method: String,
        #[serde(default)]
        params: Value,
    },
    /// stderr 에 한 줄.
    Stderr { text: String },
    /// 틀 없이 stdout 에 그대로(틀 상한 · 깨진 틀 시험).
    Raw { text: String },
    /// 멈춘다(다음 할 일까지).
    Sleep { ms: u64 },
    /// 곧바로 이 코드로 끝난다(죽음).
    Exit { code: i32 },
    /// 같은 프로세스 그룹에 `seconds` 동안 사는 자식(가짜 서버 자신의 `--sleep` — 환경을 물려받는다)을 띄운다(그룹째 끝내기 ·
    /// 거두기 시험). pid 는 `record` 에.
    SpawnChild { seconds: u64 },
    /// stderr 에 `bytes` 바이트의 `a` 를 쏟는다 — `newline` 이 거짓이면 줄바꿈 없이(stderr 상한 시험 — 리뷰 H1).
    StderrFlood { bytes: u64, newline: bool },
    /// 서버 → 클라이언트 요청 `method` 를 `count` 개 쏟는다. id 는 `id_bytes` 길이의 글이다(송신 큐 상한 시험 — 리뷰 H2).
    Flood {
        method: String,
        count: u64,
        id_bytes: usize,
    },
    /// 이 뒤로 stdin 을 읽지 않는다(파이프는 열어 둔 채 — 클라이언트의 송신이 막힌다).
    Hang,
}

impl Scenario {
    /// `dir/<name>.json` 에 쓰고 그 경로를 돌려준다.
    pub fn write(&self, dir: &Path, name: &str) -> io::Result<PathBuf> {
        let path = dir.join(format!("{name}.json"));
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&path, text)?;
        Ok(path)
    }

    pub fn read(path: &Path) -> io::Result<Self> {
        let text = std::fs::read(path)?;
        serde_json::from_slice(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn reply(mut self, method: &str, reply: Reply) -> Self {
        self.replies.insert(method.to_owned(), reply);
        self
    }

    /// `method` 의 요청에 차례로 `replies` 를 답한다(다 쓰면 [`Self::reply`] 의 답).
    pub fn sequence(mut self, method: &str, replies: Vec<Reply>) -> Self {
        self.sequence.insert(method.to_owned(), replies);
        self
    }

    pub fn on(mut self, method: &str, actions: Vec<Action>) -> Self {
        self.on
            .entry(method.to_owned())
            .or_default()
            .extend(actions);
        self
    }
}

impl Reply {
    pub fn result(value: Value) -> Self {
        Self {
            result: Some(value),
            ..Self::default()
        }
    }

    pub fn silent() -> Self {
        Self {
            no_reply: true,
            ..Self::default()
        }
    }

    pub fn delayed(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_documented_example_reads() {
        let text = r#"{
          "capabilities": { "hoverProvider": true },
          "replies": {
            "textDocument/hover": { "result": { "contents": "x" }, "delayMs": 50 },
            "shutdown": { "noReply": true }
          },
          "on": { "initialized": [{ "do": "notify", "method": "$/progress", "params": {} }] },
          "start": [{ "do": "stderr", "text": "booting" }],
          "record": "/tmp/received.jsonl",
          "ignoreExit": true
        }"#;
        let s: Scenario = serde_json::from_str(text).unwrap();
        assert_eq!(s.replies["textDocument/hover"].delay_ms, 50);
        assert!(s.replies["shutdown"].no_reply);
        assert_eq!(
            s.on["initialized"],
            vec![Action::Notify {
                method: "$/progress".into(),
                params: json!({})
            }]
        );
        assert!(s.ignore_exit);
        // 모르는 키는 읽기에서 실패한다 — 조용히 안 먹는 시나리오가 생기지 않게.
        assert!(serde_json::from_str::<Scenario>(r#"{"capabilites": {}}"#).is_err());
        assert!(serde_json::from_str::<Action>(r#"{"do": "explode"}"#).is_err());
    }
}
