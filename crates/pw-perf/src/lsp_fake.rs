//! LSP 측정용 가짜 언어 서버(`pw-perf-lsp`, src/bin/pw-perf-lsp.rs)의 동작. 06 「성능 목표」 의 LSP 행은 Pitwall 쪽 왕복만
//! 목표로 두므로(15-m4-plan.md 결정 M4-D19) 서버는 곧바로 답하고, 언제 무엇을 보냈는지를 벽시계와 함께 표지 파일(`stamps`)에
//! 남긴다 — 측정하는 쪽(`crate::lsp`)이 앱이 남긴 「그 프레임의 벽시계」 와 견준다(git 시나리오의 체크아웃 → 상태바와 같은 방법).
//!
//! 시험용 가짜(`pw-lsp` 의 `pw-lsp-fake`)를 쓰지 않는 까닭: 그쪽은 받은 알림마다 정해 둔 같은 알림을 보낼 뿐 문서 내용을 따라가지
//! 않고 보낸 시각을 남기지 않는다. 그 공유 부품을 넓히는 대신 측정 하네스 안에 둔다.
//!
//! - **문서**: `didOpen` · `didChange`(증분 — 범위는 UTF-16 글자)로 내용을 따라간다.
//! - **진단**: 문서를 열거나 바꿀 때마다 그 문서의 진단을 보낸다 — [`DIAG_MARK`] 가 든 줄마다 오류 하나(그 낱말 범위), 없으면 빈
//!   목록. 실제 서버처럼 키 입력마다 한 번씩 보낸다(타이핑 지연의 「LSP 켬」 에 든다). [`Config::problems`] 의 방아쇠 파일을 열면
//!   그 목록의 파일마다 진단 `per_file` 개를 보낸다(문제 툴 윈도우 — 진단 1만 개).
//! - **완성**: `textDocument/completion` 에 곧바로 [`Config::completion_items`] 개를 답한다.
//! - **정의**: `textDocument/definition` 의 n 번째 요청에 [`Config::definition_targets`] 의 `n % 길이` 번째 파일의
//!   [`Config::definition_line`] 줄을 답한다(다른 파일을 여는 것까지 재도록 파일을 돌려 쓴다).
//! - 그 밖의 요청은 `MethodNotFound`. `exit` 알림이나 stdin 의 끝에서 끝난다.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 이 낱말이 든 줄마다 오류 진단 하나.
pub const DIAG_MARK: &str = "perf_diag";
/// 진단 · 완성의 출처 이름.
pub const SOURCE: &str = "pw-perf-lsp";
/// 완성 항목 이름의 머리(`perfItem0` …).
pub const COMPLETION_LABEL: &str = "perfItem";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// 보낸 것의 표지(JSON 줄)를 덧붙일 파일.
    pub stamps: Option<PathBuf>,
    #[serde(default = "default_completion_items")]
    pub completion_items: usize,
    /// 정의 요청에 돌려 가며 답할 파일(`file://` URI).
    #[serde(default)]
    pub definition_targets: Vec<String>,
    /// 정의 위치의 줄(0 부터).
    #[serde(default)]
    pub definition_line: u32,
    #[serde(default)]
    pub problems: Option<Problems>,
}

fn default_completion_items() -> usize {
    50
}

/// 문제 툴 윈도우를 채울 진단.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Problems {
    /// 이 이름으로 끝나는 문서를 열면 보낸다.
    pub trigger: String,
    /// 진단을 실을 파일(`file://` URI).
    pub files: Vec<String>,
    pub per_file: usize,
}

/// 보낼 것 하나와 그때 남길 표지.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Message(Value),
    /// 보내기 직전에 표지 파일에 남길 줄(`epochMs` 는 보내는 쪽이 붙인다).
    Stamp(Value),
}

#[derive(Debug, Default)]
pub struct Server {
    pub config: Config,
    docs: HashMap<String, String>,
    definitions: usize,
    pub exit: bool,
}

impl Server {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// 받은 메시지 하나 → 보낼 것들(차례대로).
    pub fn handle(&mut self, message: &Value) -> Vec<Out> {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method, id) {
            (Some(method), Some(id)) => self.request(method, id, &params),
            (Some(method), None) => self.notification(method, &params),
            // 서버 → 클라이언트 요청의 답(보내지 않는다).
            _ => Vec::new(),
        }
    }

    fn request(&mut self, method: &str, id: Value, params: &Value) -> Vec<Out> {
        match method {
            "initialize" => vec![Out::Message(response(
                id,
                json!({
                    "capabilities": {
                        "textDocumentSync": { "openClose": true, "change": 2 },
                        "completionProvider": { "triggerCharacters": ["."], "resolveProvider": false },
                        "definitionProvider": true,
                    },
                    "serverInfo": { "name": SOURCE, "version": "0" },
                }),
            ))],
            "shutdown" => vec![Out::Message(response(id, Value::Null))],
            "textDocument/completion" => {
                let items: Vec<Value> = (0..self.config.completion_items)
                    .map(|k| {
                        json!({
                            "label": format!("{COMPLETION_LABEL}{k}"),
                            "kind": 2,
                            "detail": "fn() -> u64",
                            "sortText": format!("{k:04}"),
                        })
                    })
                    .collect();
                vec![
                    Out::Message(response(
                        id,
                        json!({ "isIncomplete": false, "items": items }),
                    )),
                    Out::Stamp(
                        json!({ "event": "completion", "items": self.config.completion_items }),
                    ),
                ]
            }
            "textDocument/definition" => {
                let targets = &self.config.definition_targets;
                if targets.is_empty() {
                    return vec![Out::Message(response(id, Value::Null))];
                }
                let n = self.definitions;
                self.definitions += 1;
                let uri = targets[n % targets.len()].clone();
                let line = self.config.definition_line;
                let from = uri_of(params);
                vec![
                    Out::Stamp(json!({ "event": "definition", "n": n, "uri": uri, "from": from })),
                    Out::Message(response(
                        id,
                        json!([{
                            "uri": uri,
                            "range": {
                                "start": { "line": line, "character": 7 },
                                "end": { "line": line, "character": 17 },
                            },
                        }]),
                    )),
                ]
            }
            _ => vec![Out::Message(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("{method} is not handled by {SOURCE}") },
            }))],
        }
    }

    fn notification(&mut self, method: &str, params: &Value) -> Vec<Out> {
        match method {
            "exit" => {
                self.exit = true;
                Vec::new()
            }
            "textDocument/didOpen" => {
                let doc = &params["textDocument"];
                let (Some(uri), Some(text)) = (doc["uri"].as_str(), doc["text"].as_str()) else {
                    return Vec::new();
                };
                let version = doc["version"].as_i64();
                self.docs.insert(uri.to_owned(), text.to_owned());
                let mut out = self.publish(uri, version);
                if let Some(problems) = &self.config.problems
                    && uri.ends_with(&problems.trigger)
                {
                    out.extend(problems_out(problems));
                }
                out
            }
            "textDocument/didChange" => {
                let doc = &params["textDocument"];
                let Some(uri) = doc["uri"].as_str() else {
                    return Vec::new();
                };
                let version = doc["version"].as_i64();
                let Some(text) = self.docs.get_mut(uri) else {
                    return Vec::new();
                };
                for change in params["contentChanges"].as_array().into_iter().flatten() {
                    apply(text, change);
                }
                self.publish(uri, version)
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.docs.remove(uri);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn publish(&self, uri: &str, version: Option<i64>) -> Vec<Out> {
        let Some(text) = self.docs.get(uri) else {
            return Vec::new();
        };
        let diagnostics = marks(text);
        let count = diagnostics.len();
        let mut params = json!({ "uri": uri, "diagnostics": diagnostics });
        if let Some(v) = version {
            params["version"] = json!(v);
        }
        vec![
            Out::Stamp(
                json!({ "event": "publish", "uri": uri, "version": version, "count": count }),
            ),
            Out::Message(notification("textDocument/publishDiagnostics", params)),
        ]
    }
}

fn uri_of(params: &Value) -> Option<String> {
    params["textDocument"]["uri"].as_str().map(str::to_owned)
}

fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

/// [`DIAG_MARK`] 가 든 줄마다 오류 진단 하나(그 낱말의 범위, UTF-16 글자).
pub fn marks(text: &str) -> Vec<Value> {
    text.split('\n')
        .enumerate()
        .filter_map(|(line, content)| {
            let at = content.find(DIAG_MARK)?;
            let start = content[..at].encode_utf16().count();
            let end = start + DIAG_MARK.encode_utf16().count();
            Some(json!({
                "range": {
                    "start": { "line": line, "character": start },
                    "end": { "line": line, "character": end },
                },
                "severity": 1,
                "source": SOURCE,
                "message": format!("perf diagnostic on line {}", line + 1),
            }))
        })
        .collect()
}

/// 문제 툴 윈도우용: 파일마다 진단 `per_file` 개(심각도를 돌려 가며).
fn problems_out(problems: &Problems) -> Vec<Out> {
    let mut out = Vec::with_capacity(problems.files.len() + 1);
    for uri in &problems.files {
        let diagnostics: Vec<Value> = (0..problems.per_file)
            .map(|k| {
                json!({
                    "range": {
                        "start": { "line": k, "character": 0 },
                        "end": { "line": k, "character": 4 },
                    },
                    "severity": (k % 4) + 1,
                    "source": SOURCE,
                    "message": format!("perf problem {k}"),
                })
            })
            .collect();
        out.push(Out::Message(notification(
            "textDocument/publishDiagnostics",
            json!({ "uri": uri, "diagnostics": diagnostics }),
        )));
    }
    out.push(Out::Stamp(json!({
        "event": "problems",
        "files": problems.files.len(),
        "count": problems.files.len() * problems.per_file,
    })));
    out
}

/// 증분 변경 하나를 적용한다(`range` 가 없으면 전체 바꾸기). 범위 밖이면 끝에 맞춘다.
pub fn apply(text: &mut String, change: &Value) {
    let Some(new) = change["text"].as_str() else {
        return;
    };
    let Some(range) = change.get("range").filter(|r| !r.is_null()) else {
        *text = new.to_owned();
        return;
    };
    let at = |p: &Value| {
        offset(
            text,
            p["line"].as_u64().unwrap_or(0) as usize,
            p["character"].as_u64().unwrap_or(0) as usize,
        )
    };
    let (from, to) = (at(&range["start"]), at(&range["end"]));
    let (from, to) = (from.min(to), from.max(to));
    text.replace_range(from..to, new);
}

/// (줄, UTF-16 글자) → 바이트 위치.
fn offset(text: &str, line: usize, character: usize) -> usize {
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(n) => start += n + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |n| start + n);
    let mut units = 0;
    for (i, c) in text[start..end].char_indices() {
        if units >= character {
            return start + i;
        }
        units += c.len_utf16();
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(line: u32, ch: u32, end_line: u32, end_ch: u32, text: &str) -> Value {
        json!({
            "range": {
                "start": { "line": line, "character": ch },
                "end": { "line": end_line, "character": end_ch },
            },
            "text": text,
        })
    }

    #[test]
    fn incremental_changes_follow_utf16_positions() {
        let mut text = "fn main() {\n    let 가 = 1;\n}\n".to_owned();
        // `가` 는 UTF-16 한 글자 — 그 뒤(8)에 넣는다.
        apply(&mut text, &change(1, 9, 1, 9, "x"));
        assert_eq!(text, "fn main() {\n    let 가x = 1;\n}\n");
        apply(&mut text, &change(0, 3, 0, 7, "start"));
        assert_eq!(text, "fn start() {\n    let 가x = 1;\n}\n");
        // 여러 줄을 지우기.
        apply(&mut text, &change(0, 12, 2, 0, ""));
        assert_eq!(text, "fn start() {}\n");
        apply(&mut text, &json!({ "text": "whole" }));
        assert_eq!(text, "whole");
    }

    fn messages(out: &[Out]) -> Vec<&Value> {
        out.iter()
            .filter_map(|o| match o {
                Out::Message(m) => Some(m),
                Out::Stamp(_) => None,
            })
            .collect()
    }

    fn stamps(out: &[Out]) -> Vec<&Value> {
        out.iter()
            .filter_map(|o| match o {
                Out::Stamp(s) => Some(s),
                Out::Message(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_marked_line_gets_a_diagnostic_after_each_change_and_loses_it_when_the_mark_goes() {
        let mut s = Server::new(Config::default());
        let uri = "file:///p/src/main.rs";
        let open = s.handle(&json!({
            "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": uri, "version": 0, "text": "a\nb\n" } },
        }));
        assert_eq!(stamps(&open)[0]["count"], 0);
        let typed = s.handle(&json!({
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 1 },
                "contentChanges": [change(1, 1, 1, 1, " perf_diag")],
            },
        }));
        let sent = messages(&typed)[0];
        assert_eq!(sent["method"], "textDocument/publishDiagnostics");
        assert_eq!(sent["params"]["version"], 1);
        let d = &sent["params"]["diagnostics"][0];
        assert_eq!(d["range"]["start"], json!({ "line": 1, "character": 2 }));
        assert_eq!(d["range"]["end"], json!({ "line": 1, "character": 11 }));
        assert_eq!(stamps(&typed)[0]["count"], 1);
        // 표지가 메시지보다 먼저다 — 보내기 직전의 시각.
        assert!(matches!(typed[0], Out::Stamp(_)));
        let removed = s.handle(&json!({
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [change(1, 1, 1, 11, "")],
            },
        }));
        assert_eq!(messages(&removed)[0]["params"]["diagnostics"], json!([]));
    }

    #[test]
    fn completion_definition_and_problems_answer_at_once() {
        let mut s = Server::new(Config {
            completion_items: 3,
            definition_targets: vec!["file:///p/t0.rs".into(), "file:///p/t1.rs".into()],
            definition_line: 40,
            problems: Some(Problems {
                trigger: "problems.rs".into(),
                files: vec!["file:///p/a.rs".into(), "file:///p/b.rs".into()],
                per_file: 5,
            }),
            ..Config::default()
        });
        let init = s.handle(&json!({ "id": 1, "method": "initialize", "params": {} }));
        let caps = &messages(&init)[0]["result"]["capabilities"];
        assert_eq!(
            caps["completionProvider"]["triggerCharacters"],
            json!(["."])
        );
        assert_eq!(caps["textDocumentSync"]["change"], 2);
        let done = s.handle(&json!({ "id": 2, "method": "textDocument/completion", "params": {} }));
        let items = messages(&done)[0]["result"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[2]["label"], "perfItem2");
        let targets: Vec<Value> = (0..3)
            .map(|n| {
                let out = s.handle(&json!({ "id": 10 + n, "method": "textDocument/definition", "params": { "textDocument": { "uri": "file:///p/m.rs" } } }));
                assert_eq!(stamps(&out)[0]["n"], n);
                messages(&out)[0]["result"][0]["uri"].clone()
            })
            .collect();
        assert_eq!(
            targets,
            ["file:///p/t0.rs", "file:///p/t1.rs", "file:///p/t0.rs"]
        );
        let opened = s.handle(&json!({
            "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": "file:///p/src/problems.rs", "version": 0, "text": "" } },
        }));
        let published: Vec<&Value> = messages(&opened)
            .into_iter()
            .filter(|m| {
                m["params"]["diagnostics"]
                    .as_array()
                    .is_some_and(|d| d.len() == 5)
            })
            .collect();
        assert_eq!(published.len(), 2);
        assert_eq!(stamps(&opened).last().unwrap()["count"], 10);
        let unknown = s.handle(&json!({ "id": 99, "method": "textDocument/hover", "params": {} }));
        assert_eq!(messages(&unknown)[0]["error"]["code"], -32601);
        assert!(s.handle(&json!({ "method": "exit" })).is_empty());
        assert!(s.exit);
    }
}
