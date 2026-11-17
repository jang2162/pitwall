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
/// 툴 이름 · 오류나 거절 이유의 최대 글자 수. 노출 안 된 툴 호출은 클라이언트가 보낸 이름을 그대로 남기므로 자른다.
pub const MAX_TOOL: usize = crate::catalog::MAX_NAME;
pub const MAX_DETAIL: usize = 512;
/// 값을 지우는 키의 단어와 그 자리의 글. 정본은 `pw_core::secret` 이다(설정 조회 툴 `settings_get` 과 한 벌).
pub use pw_core::secret::{REDACTED, SECRET_WORDS};

/// 값 대신 글자 수만 남기는 「원문 키」의 단어(키를 camelCase · `_` · `-` 로 가른 단어 하나가 이것과 같으면, 소문자로 견준다). SQL
/// 원문은 짧으면 리터럴 값까지 통째로 남으므로 남기지 않는다 — `db_query` · `db_execute` 는 감사 상세에 문장 해시를 따로 남긴다
/// (16-m5-plan.md M5-07 리뷰 F1). 비밀 키이기도 하면 [`REDACTED`] 가 앞선다(글자 수도 남기지 않는다).
pub const CONTENT_WORDS: &[&str] = &["sql"];

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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    Ok,
    /// 툴은 불렸고 실패했다(RPC 오류).
    Error,
    /// 부르지 않았다(노출 안 된 툴, 등급 정책).
    Denied,
}

/// 짧은 이름 — 이 크레이트와 앱이 쓴다. TS 생성물의 이름은 [`AuditOutcome`] 이다.
pub type Outcome = AuditOutcome;

impl AuditOutcome {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Outcome::Ok),
            "error" => Some(Outcome::Error),
            "denied" => Some(Outcome::Denied),
            _ => None,
        }
    }

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

    /// 한 행을 쓰고 그 행의 id(`calls.id` — `mcp.audit.list` 의 `id`)를 돌려준다. 디스크 IO 를 하므로 비동기 문맥에서 부르지
    /// 않는다 — 세션은 기록 스레드(`server.rs` `audit_writer`)에서 부른다.
    pub fn record(&self, entry: &Entry) -> Result<i64, String> {
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

/// 쓰인 감사 행 하나(`mcp.audit.list` 의 원소). 열은 [`Entry`] 와 같고 `id` · `at` 이 더 있다. M5-11 이 칸(승인 방식 · 계획
/// 해시)을 더할 자리다.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct AuditRow {
    /// 행 id. 쪽 넘기기의 커서이고 [`AuditLog::record`] 가 돌려준 값이다.
    #[specta(type = pw_rpc::JsNumber)]
    pub id: i64,
    /// 기록 시각(Unix 밀리초).
    #[specta(type = pw_rpc::JsNumber)]
    pub at: i64,
    pub session: String,
    pub client: Option<String>,
    pub project_id: Option<String>,
    pub tool: String,
    /// 인자 요약([`summarize`] 의 JSON 글).
    pub args: String,
    pub outcome: Outcome,
    pub detail: Option<String>,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub result_size: Option<i64>,
    #[specta(type = pw_rpc::JsNumber)]
    pub duration_ms: i64,
}

/// [`AuditLog::list`] 의 조건.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// 그 프로젝트의 행과 프로젝트 없는 행. `None` 이면 전부.
    pub project_id: Option<String>,
    /// 이 id 보다 앞의(오래된) 행만 — 쪽 넘기기.
    pub before: Option<i64>,
    pub limit: usize,
    pub outcome: Option<Outcome>,
    /// 툴 이름(정확히 같은 것).
    pub tool: Option<String>,
}

impl AuditLog {
    /// 최신부터 `query.limit` 행과 그 뒤에 행이 더 있는지. 파일이 없으면 만들지 않고 빈 목록이다. 디스크 IO 를 한다.
    pub fn list(&self, query: &Query) -> Result<(Vec<AuditRow>, bool), String> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, at, session, client, project_id, tool, args, outcome, detail, result_size, duration_ms
                 FROM calls
                 WHERE (?1 IS NULL OR project_id = ?1 OR project_id IS NULL)
                   AND (?2 IS NULL OR id < ?2)
                   AND (?3 IS NULL OR outcome = ?3)
                   AND (?4 IS NULL OR tool = ?4)
                 ORDER BY id DESC
                 LIMIT ?5",
            )?;
            let limit = i64::try_from(query.limit.saturating_add(1)).unwrap_or(i64::MAX);
            let rows = stmt
                .query_map(
                    params![
                        query.project_id,
                        query.before,
                        query.outcome.map(Outcome::as_str),
                        query.tool,
                        limit,
                    ],
                    |row| {
                        let outcome: String = row.get(7)?;
                        Ok(AuditRow {
                            id: row.get(0)?,
                            at: row.get(1)?,
                            session: row.get(2)?,
                            client: row.get(3)?,
                            project_id: row.get(4)?,
                            tool: row.get(5)?,
                            args: row.get(6)?,
                            // 스키마의 CHECK 가 값을 셋으로 묶는다. 다른 값(뒤 판이 더한 값을 앞 판이 읽음)은 오류로 본다.
                            outcome: Outcome::parse(&outcome).unwrap_or(Outcome::Error),
                            detail: row.get(8)?,
                            result_size: row.get(9)?,
                            duration_ms: row.get(10)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .map(|rows| {
            let mut rows = rows.unwrap_or_default();
            let more = rows.len() > query.limit;
            rows.truncate(query.limit);
            (rows, more)
        })
    }

    /// `since`(Unix 밀리초) 뒤의 툴별 호출 수. `project_id` 가 있으면 그 프로젝트의 행만. 파일이 없으면 빈 표다.
    pub fn tool_counts(
        &self,
        since: i64,
        project_id: Option<&str>,
    ) -> Result<std::collections::HashMap<String, u64>, String> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT tool, COUNT(*) FROM calls
                 WHERE at >= ?1 AND (?2 IS NULL OR project_id = ?2)
                 GROUP BY tool",
            )?;
            let counts = stmt
                .query_map(params![since, project_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .map(|r| r.map(|(tool, n)| (tool, u64::try_from(n).unwrap_or(0))))
                .collect::<rusqlite::Result<_>>()?;
            Ok(counts)
        })
        .map(Option::unwrap_or_default)
    }

    /// 읽기. 파일이 아직 없으면 만들지 않고 `None`.
    fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<Option<T>, String> {
        let mut guard = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            if !self.file.exists() {
                return Ok(None);
            }
            *guard = Some(open(&self.file).map_err(|e| format!("{}: {e}", self.file.display()))?);
        }
        let conn = guard.as_ref().expect("방금 채웠다");
        let result = f(conn);
        if result.is_err() {
            *guard = None;
        }
        result
            .map(Some)
            .map_err(|e| format!("{}: {e}", self.file.display()))
    }
}

fn open(file: &Path) -> rusqlite::Result<Connection> {
    let io = |path: &Path, e: std::io::Error| {
        rusqlite::Error::InvalidPath(PathBuf::from(format!("{}: {e}", path.display())))
    };
    if let Some(dir) = file.parent() {
        private_dir(dir).map_err(|e| io(dir, e))?;
    }
    private_file(file).map_err(|e| io(file, e))?;
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

/// 감사 로그는 인자 요약 · 프로젝트 · 클라이언트를 담는다. 같은 머신의 다른 사용자가 읽지 못하게 새로 만드는 폴더는 0700,
/// 파일은 0600 이다(SQLite 는 저널 파일을 본 파일과 같은 권한으로 만든다). 이미 있는 폴더의 권한은 바꾸지 않는다.
#[cfg(unix)]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn private_file(file: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(file)?;
    fs::set_permissions(file, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn private_file(_file: &Path) -> std::io::Result<()> {
    Ok(())
}

fn insert(conn: &mut Connection, e: &Entry) -> rusqlite::Result<i64> {
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
            clip(&e.tool, MAX_TOOL),
            clip(&e.args, MAX_SUMMARY),
            e.outcome.as_str(),
            e.detail.as_deref().map(|d| clip(d, MAX_DETAIL)),
            e.result_size.map(|n| n as i64),
            e.duration.as_millis() as i64,
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

/// 인자 요약. 감사 로그는 오래 남고 UI 에 보이므로 인자를 그대로 쓰지 않는다:
///
/// 1. 키를 단어로 갈라(camelCase · `_` · `-`) [`SECRET_WORDS`] 의 단어가 있으면 값을 [`REDACTED`] 로 바꾼다(`apiKey`,
///    `db_password`, `authToken`). 값의 종류와 상관없이 통째로 바꾼다.
/// 2. 비밀이 아닌 키 가운데 단어에 [`CONTENT_WORDS`] 가 있으면(`sql`) 값 대신 `[<글자 수> chars]` 를 남긴다(글자열이 아니면
///    `[not shown]`).
/// 3. 글자열은 [`MAX_STRING`] 글자까지, 배열은 [`MAX_ITEMS`] 개까지(나머지는 개수만), 깊이는 [`MAX_DEPTH`] 까지.
/// 4. 결과 JSON 이 [`MAX_SUMMARY`] 바이트를 넘으면 자르고 `…` 를 붙인다.
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
                    } else if content_key(k) {
                        Value::String(match v {
                            Value::String(text) => format!("[{} chars]", text.chars().count()),
                            _ => "[not shown]".to_owned(),
                        })
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

/// 키를 단어로 갈랐을 때(camelCase · 영숫자 아닌 글자) [`CONTENT_WORDS`] 의 단어가 있는가.
fn content_key(key: &str) -> bool {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in key.chars() {
        if !c.is_alphanumeric() {
            words.extend((!current.is_empty()).then(|| std::mem::take(&mut current)));
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        current.extend(c.to_lowercase());
    }
    words.extend((!current.is_empty()).then_some(current));
    words.iter().any(|w| CONTENT_WORDS.contains(&w.as_str()))
}

fn truncate(s: &str) -> String {
    clip(s, MAX_STRING)
}

/// `max` 글자까지 남기고 넘으면 `…` 를 붙인다.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_owned(),
    }
}

fn secret_key(key: &str) -> bool {
    pw_core::secret::is_secret_key(key)
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
            "APIToken": "t2",
            "api_keys": ["k"],
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
                "APIToken": REDACTED,
                "api_keys": REDACTED,
                "keymap": "default",
                "monkey": "banana",
            })
        );
    }

    /// 원문 키(`CONTENT_WORDS` — `sql`)는 짧아도 값 대신 글자 수만 남는다(16-m5-plan.md M5-07 리뷰 F1). 비밀 키이기도 하면
    /// `[redacted]` 가 앞선다. 단어로 견준다 — `mysql` · `sqlite` 처럼 한 단어 안에 든 것은 아니다.
    #[test]
    fn content_keys_keep_only_their_length_and_secrets_win() {
        let sql = "SELECT * FROM users WHERE ssn = '123-45-6789'";
        let summary: Value = serde_json::from_str(&summarize(&json!({
            "sql": sql,
            "nested": {"rawSql": "x"},
            "sql_lines": ["SELECT 1"],
            "sqlToken": "secret-and-content",
            "SQL_PASSWORD": "pw",
            "mysql": "kept",
            "sqlite": "kept",
            "file": "queries/a.sql",
        })))
        .unwrap();
        assert_eq!(
            summary,
            json!({
                "sql": format!("[{} chars]", sql.chars().count()),
                "nested": {"rawSql": "[1 chars]"},
                "sql_lines": "[not shown]",
                "sqlToken": REDACTED,
                "SQL_PASSWORD": REDACTED,
                "mysql": "kept",
                "sqlite": "kept",
                "file": "queries/a.sql",
            })
        );
        assert!(!summarize(&json!({ "sql": sql })).contains("123-45-6789"));
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

    /// 파일은 0600, 새로 만든 폴더는 0700. 클라이언트가 보낸 긴 툴 이름 · 긴 이유는 잘린다.
    #[cfg(unix)]
    #[test]
    fn the_file_is_private_and_long_fields_are_clipped() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::under_state(&tmp.path().join("state"));
        log.record(&Entry {
            session: "s".into(),
            client: None,
            project_id: None,
            tool: "x".repeat(10_000),
            args: "{}".into(),
            outcome: Outcome::Denied,
            detail: Some("y".repeat(10_000)),
            result_size: None,
            duration: Duration::ZERO,
        })
        .unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(log.file()), 0o600);
        assert_eq!(mode(log.file().parent().unwrap()), 0o700);
        let conn = Connection::open(log.file()).unwrap();
        let (tool, detail): (String, String) = conn
            .query_row("SELECT tool, detail FROM calls", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(tool.chars().count(), MAX_TOOL + 1);
        assert_eq!(detail.chars().count(), MAX_DETAIL + 1);
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
