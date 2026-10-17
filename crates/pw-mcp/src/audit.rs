//! MCP 호출 감사 로그(`docs/design/05-mcp.md` §3): `$STATE/mcp/audit.db`(SQLite) 한 파일.
//!
//! - **모든 `tools/call`** 을 한 행으로 남긴다 — 허용 · 실패 · 거절(노출 안 된 툴, 등급 정책) 모두. 세션 시작은 남기지 않는다.
//! - 프로젝트를 정하지 않는 호출(`project_list`)은 `project_id` 가 `NULL` 이다. 프로젝트마다 파일을 두지 않는 이유가 이것이다 —
//!   어느 프로젝트에도 속하지 않는 호출이 있고, 「에이전트 활동」 패널은 `project_id` 로 거른다.
//! - 결과 **내용은 남기지 않는다**(크기와 상태만). 인자는 [`summarize`] 의 요약만 남긴다.
//! - 여러 프로세스(앱, 앱 없이 뜬 `pitwall mcp` 여럿)가 같은 파일에 쓴다. 쓰기는 `BEGIN IMMEDIATE` 와 2초 대기다.
//! - 기록이 실패해도 툴 호출은 실패시키지 않는다 — 경고 로그를 남긴다. 감사 로그가 깨졌다고 조회 툴이 멈추면 안 되고,
//!   지금 툴은 모두 `read` 다. 쓰기 등급 툴을 열 때 「기록 실패면 거절」로 바꿀지 다시 정한다.
//! - 보관 기간 · 지우기는 아직 없다(감사 로그 UI 와 함께).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::{Map, Value};

/// `$STATE` 아래 폴더와 파일 이름.
pub const DIR: &str = "mcp";
pub const FILE_NAME: &str = "audit.db";
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// 인자 요약의 한도.
pub const MAX_STRING: usize = 80;
pub const MAX_ITEMS: usize = 8;
pub const MAX_DEPTH: usize = 3;
pub const MAX_SUMMARY: usize = 1024;
/// 값을 지우는 키의 단어(camelCase · snake_case · kebab-case 로 가른 단어 하나가 이것과 같으면). 비교는 소문자로.
pub const SECRET_WORDS: &[&str] = &[
    "password",
    "passwd",
    "pass",
    "passphrase",
    "secret",
    "token",
    "key",
    "apikey",
    "auth",
    "authorization",
    "credential",
    "credentials",
    "cookie",
    "private",
    "otp",
];
pub const REDACTED: &str = "[redacted]";

/// 버전 `i` → `i + 1`.
const MIGRATIONS: &[&str] = &["CREATE TABLE calls (
         id          INTEGER PRIMARY KEY,
         at          INTEGER NOT NULL,
         session     TEXT NOT NULL,
         client      TEXT,
         project_id  TEXT,
         tool        TEXT NOT NULL,
         args        TEXT NOT NULL,
         outcome     TEXT NOT NULL CHECK (outcome IN ('ok', 'error', 'denied')),
         detail      TEXT,
         result_size INTEGER,
         duration_ms INTEGER NOT NULL
     );
     CREATE INDEX calls_project_at ON calls(project_id, at);"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// 툴은 불렸고 실패했다(RPC 오류).
    Error,
    /// 부르지 않았다(노출 안 된 툴, 등급 정책).
    Denied,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Denied => "denied",
        }
    }
}

/// 한 호출의 기록.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub session: String,
    /// `initialize` 의 clientInfo(`이름 버전`).
    pub client: Option<String>,
    pub project_id: Option<String>,
    pub tool: String,
    /// [`summarize`] 결과.
    pub args: String,
    pub outcome: Outcome,
    /// 오류 · 거절 이유.
    pub detail: Option<String>,
    /// 결과 JSON 의 바이트 수(성공일 때).
    pub result_size: Option<usize>,
    pub duration: Duration,
}

/// 감사 로그 파일. 처음 쓸 때 연다.
pub struct AuditLog {
    file: PathBuf,
    conn: Mutex<Option<Connection>>,
}

impl AuditLog {
    /// `$STATE/mcp/audit.db`.
    pub fn under_state(state: &Path) -> Self {
        Self::at(state.join(DIR).join(FILE_NAME))
    }

    pub fn at(file: PathBuf) -> Self {
        Self {
            file,
            conn: Mutex::new(None),
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// 한 행을 쓴다. 디스크 IO 를 하므로 비동기 문맥에서는 `spawn_blocking` 으로 부른다.
    pub fn record(&self, entry: &Entry) -> Result<(), String> {
        let mut guard = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            *guard = Some(open(&self.file).map_err(|e| format!("{}: {e}", self.file.display()))?);
        }
        let conn = guard.as_mut().expect("방금 채웠다");
        let result = insert(conn, entry);
        if result.is_err() {
            // 연결이 상한 경우(파일이 지워짐 등) 다음 기록에서 다시 연다.
            *guard = None;
        }
        result.map_err(|e| format!("{}: {e}", self.file.display()))
    }
}

fn open(file: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| {
            rusqlite::Error::InvalidPath(PathBuf::from(format!("{}: {e}", dir.display())))
        })?;
    }
    let mut conn = Connection::open(file)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if (current as usize) < MIGRATIONS.len() {
        for (from, step) in MIGRATIONS.iter().enumerate() {
            // 다른 프로세스가 먼저 올렸으면 잠금 안에서 다시 읽은 버전으로 건너뛴다(레지스트리와 같은 규칙).
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let version: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if version as usize > from {
                continue;
            }
            tx.execute_batch(step)?;
            tx.pragma_update(None, "user_version", (from + 1) as u32)?;
            tx.commit()?;
        }
    }
    Ok(conn)
}

fn insert(conn: &mut Connection, e: &Entry) -> rusqlite::Result<()> {
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO calls (at, session, client, project_id, tool, args, outcome, detail, result_size, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            at,
            e.session,
            e.client,
            e.project_id,
            e.tool,
            e.args,
            e.outcome.as_str(),
            e.detail,
            e.result_size.map(|n| n as i64),
            e.duration.as_millis() as i64,
        ],
    )?;
    tx.commit()
}

/// 인자 요약. 감사 로그는 오래 남고 UI 에 보이므로 인자를 그대로 쓰지 않는다:
///
/// 1. 키를 단어로 갈라(camelCase · `_` · `-`) [`SECRET_WORDS`] 의 단어가 있으면 값을 [`REDACTED`] 로 바꾼다(`apiKey`,
///    `db_password`, `authToken`). 값의 종류와 상관없이 통째로 바꾼다.
/// 2. 글자열은 [`MAX_STRING`] 글자까지, 배열은 [`MAX_ITEMS`] 개까지(나머지는 개수만), 깊이는 [`MAX_DEPTH`] 까지.
/// 3. 결과 JSON 이 [`MAX_SUMMARY`] 바이트를 넘으면 자르고 `…` 를 붙인다.
///
/// 키 이름으로 못 거르는 비밀(값 안의 토큰)은 남을 수 있다. 그래서 비밀을 인자로 받는 툴을 만들지 않는다는 설계 규칙
/// (`db_connection_upsert` 는 비밀번호를 툴 인자로 받지 않는다)이 첫 번째 방어선이고, 이것은 두 번째다.
pub fn summarize(args: &Value) -> String {
    let text = summarize_value(args, 0).to_string();
    if text.len() <= MAX_SUMMARY {
        return text;
    }
    let mut end = MAX_SUMMARY;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn summarize_value(value: &Value, depth: usize) -> Value {
    match value {
        Value::String(s) => Value::String(truncate(s)),
        Value::Array(items) if depth >= MAX_DEPTH => {
            Value::String(format!("[{} items]", items.len()))
        }
        Value::Object(map) if depth >= MAX_DEPTH => {
            Value::String(format!("{{{} keys}}", map.len()))
        }
        Value::Array(items) => {
            let mut out: Vec<Value> = items
                .iter()
                .take(MAX_ITEMS)
                .map(|v| summarize_value(v, depth + 1))
                .collect();
            if items.len() > MAX_ITEMS {
                out.push(Value::String(format!("… {} more", items.len() - MAX_ITEMS)));
            }
            Value::Array(out)
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = if secret_key(k) {
                        Value::String(REDACTED.into())
                    } else {
                        summarize_value(v, depth + 1)
                    };
                    (k.clone(), v)
                })
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

fn truncate(s: &str) -> String {
    match s.char_indices().nth(MAX_STRING) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_owned(),
    }
}

fn secret_key(key: &str) -> bool {
    words(key)
        .iter()
        .any(|w| SECRET_WORDS.contains(&w.as_str()))
}

/// `apiKey` → [api, key], `db_password` → [db, password], `X-Auth-Token` → [x, auth, token].
fn words(key: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in key.chars() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn secret_keys_are_redacted_by_word() {
        let summary: Value = serde_json::from_str(&summarize(&json!({
            "projectId": "p1",
            "apiKey": "sk-123",
            "db_password": {"nested": true},
            "X-Auth-Token": "t",
            "keymap": "default",
            "monkey": "banana",
        })))
        .unwrap();
        assert_eq!(
            summary,
            json!({
                "projectId": "p1",
                "apiKey": REDACTED,
                "db_password": REDACTED,
                "X-Auth-Token": REDACTED,
                "keymap": "default",
                "monkey": "banana",
            })
        );
    }

    #[test]
    fn long_values_are_cut() {
        let long = "가".repeat(MAX_STRING + 5);
        let summary: Value = serde_json::from_str(&summarize(&json!({
            "s": long,
            "list": (0..20).collect::<Vec<_>>(),
            "deep": {"a": {"b": {"c": [1, 2]}}},
        })))
        .unwrap();
        assert_eq!(
            summary["s"].as_str().unwrap().chars().count(),
            MAX_STRING + 1
        );
        assert_eq!(summary["list"].as_array().unwrap().len(), MAX_ITEMS + 1);
        assert_eq!(summary["list"][MAX_ITEMS], json!("… 12 more"));
        assert_eq!(summary["deep"]["a"]["b"], json!("{1 keys}"));
        let huge = summarize(
            &json!({"k": (0..2000).map(|i| i.to_string()).collect::<Vec<_>>().join(",")}),
        );
        assert!(huge.len() <= MAX_SUMMARY + "…".len());
    }

    #[test]
    fn records_allowed_and_denied_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::under_state(tmp.path());
        let entry = Entry {
            session: "s1".into(),
            client: Some("claude-code 2.1".into()),
            project_id: None,
            tool: "project_list".into(),
            args: "{}".into(),
            outcome: Outcome::Ok,
            detail: None,
            result_size: Some(42),
            duration: Duration::from_millis(3),
        };
        log.record(&entry).unwrap();
        log.record(&Entry {
            outcome: Outcome::Denied,
            detail: Some("tool is not exposed".into()),
            project_id: Some("p1".into()),
            ..entry.clone()
        })
        .unwrap();
        let conn = Connection::open(log.file()).unwrap();
        let rows: Vec<(Option<String>, String, Option<String>)> = conn
            .prepare("SELECT project_id, outcome, detail FROM calls ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows,
            [
                (None, "ok".into(), None),
                (
                    Some("p1".into()),
                    "denied".into(),
                    Some("tool is not exposed".into())
                ),
            ]
        );
    }
}
