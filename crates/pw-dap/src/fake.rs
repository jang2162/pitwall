//! 가짜 디버그 어댑터의 시나리오 형식(17-m6-plan.md M6-15 시험 — `pw-lsp` 의 `fake` 와 같은 방식). 어댑터 자체는
//! `src/bin/pw-dap-fake.rs`(시험 전용 바이너리 — 배포물에 들지 않는다)이고, 시험은 [`Scenario`] 를 JSON 파일로 써서 그 경로를 첫
//! 인자로 준다. `--port N --host H` 를 더 주면 stdio 대신 `H:N` 에서 연결을 받는다(연결마다 같은 시나리오로 답한다).
//!
//! ```json
//! {
//!   "capabilities": { "supportsConfigurationDoneRequest": true },
//!   "replies": {
//!     "launch": { "after": ["configurationDone"] },
//!     "threads": { "body": { "threads": [] }, "delayMs": 50 },
//!     "evaluate": { "error": "notStopped" }
//!   },
//!   "on": { "launch": [{ "do": "request", "command": "runInTerminal", "arguments": {} }] },
//!   "initializedAfter": "launch",
//!   "requireInitialized": ["setBreakpoints", "configurationDone"],
//!   "record": "/tmp/received.jsonl"
//! }
//! ```
//!
//! - **답**(`replies`, 명령 이름별): `body`(없으면 싣지 않는다) 또는 `error`(실패 응답의 `message`), `delayMs`, `after`(그 명령들을
//!   받은 뒤에 답 — 그사이 다른 메시지도 읽는다), `noReply`, `then`(답한 뒤 할 일). 없는 명령의 기본 답: `initialize` →
//!   `capabilities`, `setBreakpoints` · `setFunctionBreakpoints` → 받은 것마다 `verified` 인 브레이크포인트, `threads` → 스레드 하나(`main`),
//!   `disconnect` → 답한 뒤 끝난다(TCP 면 첫 연결의
//!   `disconnect` 에서), 그 밖 → 본문 없는 성공.
//! - **`initialized` 이벤트**: `initializedAfter` 의 명령을 받으면 보낸다(기본 `initialize` — 그 답 바로 뒤에. 다른 명령이면 받은
//!   그 자리에서, 답보다 먼저). `"never"` 면 보내지 않는다.
//! - **순서 검사**(`requireInitialized`): 그 명령이 `initialized` 를 보내기 전에 오면 `{"violation": …}` 을 기록하고 코드 97 로
//!   끝난다 — 순서를 어긴 클라이언트는 시작에 실패한다.
//! - **할 일**(`on` — 받은 요청의 명령 이름별, 답보다 먼저 · `start` — 뜨자마자): [`Action`] 의 목록을 차례로 — 이벤트, 역요청
//!   (`seq` 는 이어서), stderr 한 줄, 틀 없는 바이트, 틀에 담은 아무 본문(JSON 이 아닌 본문 시험), 멈추기, 끝내기(죽기). 역요청의
//!   `unlessArg` 는 그 할 일을 부른 요청의 인자에 그 키가 있으면 건너뛴다(자식 세션이 다시 자식을 열지 않게).
//! - **기록**(`record`): 뜨면 `{"argv": [...], "pid": n}` 한 줄, 받은 메시지마다 `{"conn": k, "msg": {...}}` 한 줄(클라이언트의
//!   요청 · 역요청에 대한 응답 그대로).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scenario {
    /// `initialize` 응답의 본문.
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub replies: BTreeMap<String, Reply>,
    #[serde(default)]
    pub on: BTreeMap<String, Vec<Action>>,
    #[serde(default)]
    pub start: Vec<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialized_after: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub require_initialized: Vec<String>,
    #[serde(default)]
    pub record: Option<PathBuf>,
}

/// 요청 하나의 답.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub delay_ms: u64,
    /// 이 명령들을 모두 받은 뒤에 답한다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(default)]
    pub no_reply: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub then: Vec<Action>,
}

/// 가짜 어댑터가 하는 일 하나.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    Event {
        event: String,
        #[serde(default)]
        body: Value,
    },
    /// 역요청. 응답은 `record` 에 남는다.
    Request {
        command: String,
        #[serde(default)]
        arguments: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unless_arg: Option<String>,
    },
    Stderr {
        text: String,
    },
    /// 틀 없이 그대로(틀 상한 · 깨진 틀 시험).
    Raw {
        text: String,
    },
    /// 틀에 담아 그대로(DAP 메시지가 아닌 본문 시험).
    Body {
        text: String,
    },
    Sleep {
        ms: u64,
    },
    /// 곧바로 이 코드로 끝난다(죽음).
    Exit {
        code: i32,
    },
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

    pub fn reply(mut self, command: &str, reply: Reply) -> Self {
        self.replies.insert(command.to_owned(), reply);
        self
    }

    pub fn on(mut self, command: &str, actions: Vec<Action>) -> Self {
        self.on
            .entry(command.to_owned())
            .or_default()
            .extend(actions);
        self
    }
}

impl Reply {
    pub fn body(body: Value) -> Self {
        Self {
            body: Some(body),
            ..Self::default()
        }
    }

    pub fn error(message: &str) -> Self {
        Self {
            error: Some(message.to_owned()),
            ..Self::default()
        }
    }

    pub fn silent() -> Self {
        Self {
            no_reply: true,
            ..Self::default()
        }
    }

    /// `commands` 를 모두 받은 뒤에 답한다.
    pub fn after(mut self, commands: &[&str]) -> Self {
        self.after = commands.iter().map(|&c| c.to_owned()).collect();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_example_reads() {
        let text = r#"{
          "capabilities": { "supportsConfigurationDoneRequest": true },
          "replies": {
            "launch": { "after": ["configurationDone"] },
            "threads": { "body": { "threads": [] }, "delayMs": 50 },
            "evaluate": { "error": "notStopped" }
          },
          "on": { "launch": [{ "do": "request", "command": "runInTerminal", "arguments": {} }] },
          "initializedAfter": "launch",
          "requireInitialized": ["setBreakpoints", "configurationDone"],
          "record": "/tmp/received.jsonl"
        }"#;
        let s: Scenario = serde_json::from_str(text).unwrap();
        assert_eq!(s.replies["launch"].after, ["configurationDone"]);
        assert_eq!(s.replies["threads"].delay_ms, 50);
        assert_eq!(s.initialized_after.as_deref(), Some("launch"));
        assert!(
            matches!(&s.on["launch"][0], Action::Request { command, .. } if command == "runInTerminal")
        );
        assert!(serde_json::from_str::<Scenario>(r#"{"capabilites": {}}"#).is_err());
        assert!(serde_json::from_str::<Action>(r#"{"do": "explode"}"#).is_err());
    }
}
