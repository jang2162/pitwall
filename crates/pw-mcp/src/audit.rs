//! MCP 호출 감사 로그(`docs/design/05-mcp.md` §3): `$STATE/mcp/audit.db`(SQLite) 한 파일.
//!
//! - **모든 `tools/call`** 을 한 행으로 남긴다 — 허용 · 실패 · 거절(노출 안 된 툴, 등급 정책) 모두. 세션 시작은 남기지 않는다.
//! - 프로젝트를 정하지 않는 호출(`project_list`)은 `project_id` 가 `NULL` 이다. 프로젝트마다 파일을 두지 않는 이유가 이것이다 —
//!   어느 프로젝트에도 속하지 않는 호출이 있고, 「에이전트 활동」 패널은 `project_id` 로 거른다.
//! - 결과 **내용은 남기지 않는다**(크기와 상태만). 인자는 [`summarize`] 의 요약만 남긴다.
//! - 여러 프로세스(앱, 앱 없이 뜬 `pitwall mcp` 여럿)가 같은 파일에 쓴다. 쓰기는 `BEGIN IMMEDIATE` 와 2초 대기다.
//!   저장은 WAL · `synchronous = FULL`([`use_wal`]), 한 프로세스 안에서는 연결 하나를 함께 쓴다([`AuditLog`]).
//!   앞 판 파일의 판 올림은 프로세스 사이에서도 한 번에 하나다([`upgrade_lock`]).
//! - **`read` 호출은 끝난 뒤 한 번 쓰고**, 기록이 실패해도 호출은 실패시키지 않는다 — 경고 로그를 남긴다(감사 로그가 깨졌다고
//!   조회 툴이 멈추면 안 된다). **`write` 이상과 승인을 지난 호출은 실행 전에 행을 `started` 로 쓰고([`AuditLog::begin`]) 못
//!   쓰면 실행하지 않는다**(16-m5-plan 결정 M5-D11 — 쓰기의 흔적이 없으면 사람이 무엇이 바뀌었는지 뒤에 볼 길이 없다). 끝나면 **같은 행**을
//!   고친다([`AuditLog::finish`]). 끝을 못 쓴 행(그 사이 프로세스가 죽음)은 `started` 로 남는다 — 화면은 「끝을 모름」 이다.
//! - **보관**(결정 M5-D10): 기본 [`DEFAULT_RETENTION_DAYS`](90)일, `[mcp.audit] retention_days`(0 은 지우지 않음).
//!   **`[mcp.audit]` 은 전역 설정에서만 읽는다**([`retention_days`] 는 전역 `[mcp]` 만 받는다) — 감사 파일이 하나라
//!   프로젝트마다 다른 기간을 둘 자리가 없고, 커밋된 `project.toml` 이 먹으면 저장소를 클론해 연 것만으로 기록이 지워진다.
//!   프로젝트 설정의 그 표는 효과가 없고 경고를 남긴다([`project_table_warning`] — 프로젝트 `[mcp]` 를 읽는
//!   `exposure::Exposure::from_layers` 가 낸다). 앱이 시작할 때와 하루에 한 번 지운다([`keep_pruned`]). 사람의 화면은 한
//!   프로젝트의 행을 지운다([`AuditLog::clear`] — `mcp.audit.clear`).
//! - **행 id 는 다시 쓰이지 않는다**(둘째 판의 `AUTOINCREMENT`): 보관 정리 · 지우기가 가장 큰 id 까지 지워도 새 행은 더 큰 id 다.
//!   `mcp.audit.list` 의 커서와 `mcp.activity` 의 `id` 가 행 id 라, 다시 쓰이면 다른 행을 가리킨다.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::{Map, Value};

/// `$STATE` 아래 폴더와 파일 이름.
pub const DIR: &str = "mcp";
pub const FILE_NAME: &str = "audit.db";
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// 판 올림 잠금([`upgrade_lock`])을 기다리는 상한 — 판 올림 단계마다 busy 대기 한 번(2초 × 단계 4 = 8초).
///
/// busy 2초는 문장 하나가 SQLite 잠금을 기다리는 상한이고 그대로다. 이것은 다른 문제 — 다른 프로세스의 판 올림 **전체**를 기다리는
/// 상한이다. 쥔 쪽은 단계마다 쓰기 잠금을 busy 2초까지 기다릴 수 있으므로, 잠금 없이 단계마다 다투던 때 한 연결이 기다릴 수
/// 있던 만큼(단계마다 2초)을 한 번에 준다. 넘기면 열기가 실패한다 — 쓰기 툴은 감사 불가(`audit_unavailable`)로 거절되고(닫힌 쪽)
/// 다음 열기에서 다시 해 본다. 상한이 없으면 잠금을 쥔 채 멈춘 프로세스(디버거 정지 등) 하나에 이 프로세스의 감사 기록이 모두
/// 매달린다(연결 뮤텍스를 쥔 채 연다).
const UPGRADE_LOCK_TIMEOUT: Duration = BUSY_TIMEOUT.saturating_mul(MIGRATIONS.len() as u32);

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
/// (16-m5-plan.md M5-07 리뷰 F1). 명령줄 원문(`command` · `args` — `open_with_upsert` 의 argv, `run_config_upsert` 의 셸 명령 ·
/// 인자)과 env 의 값(`env` — 구성의 `env` 표 · `envFile` 의 경로까지)도 같다(17-m6-plan M6-13, 메인 판정 2026-11-18). 비밀 키이기도
/// 하면 [`REDACTED`] 가 앞선다(글자 수도 남기지 않는다). 디버그 대상 안에서 평가되는 식(`condition` · `hitCondition` ·
/// `logMessage` — `breakpoint_set_expr`, 17-m6-plan M6-20)도 같다.
pub const CONTENT_WORDS: &[&str] = &["sql", "command", "args", "env", "condition", "log"];

/// `[mcp.audit] retention_days` 가 없을 때의 보관 일수(결정 M5-D10).
pub const DEFAULT_RETENTION_DAYS: u32 = 90;
/// 전역 `[mcp]` 안의 보관 표 이름(`[mcp.audit]`)과 그 키.
pub const SETTINGS_TABLE: &str = "audit";
pub const RETENTION_KEY: &str = "retention_days";
/// 보관 정리의 간격. 앱이 시작할 때 한 번 지우고 그 뒤 이 간격마다([`keep_pruned`]).
pub const PRUNE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 버전 `i` → `i + 1`. 판을 올리는 단계는 한 트랜잭션이다(`open`).
///
/// - 둘째 판: 결과에 `started`(선기록 — 실행 전에 쓴 행), 승인 방식 · 계획 해시 칸, 시각 색인(보관 정리). id 는
///   `AUTOINCREMENT` — 지운 가장 큰 id 를 다시 쓰지 않는다. SQLite 는 열의 `CHECK` · `AUTOINCREMENT` 를 고치지 못하므로 표를 새로
///   만들어 행을 옮긴다(id 그대로 — `sqlite_sequence` 가 가장 큰 id 에서 이어진다).
/// - 셋째 판: 승인 방식 `policy`(정책 자동 — 17-m6-plan M6-10 ③). `CHECK` 를 고치려고 표를 새로 만들어 행을 옮긴다. 옮기기 전
///   표의 `sqlite_sequence` 값(지운 가장 큰 id 까지 센 값)을 새 표로 넘긴다 — 옮긴 행의 가장 큰 id 로 되돌아가지 않는다.
/// - 넷째 판: 전송 칸 `transport`(18-m7-plan M7-25 ⑥ — 메인 판정 2026-11-24). `ALTER TABLE ... ADD COLUMN` 한 줄이라 행 · id 를
///   옮기지 않는다. 판 올림은 다른 판과 같이 한 트랜잭션이다(실패하면 셋째 판 그대로).
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE calls (
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
     CREATE INDEX calls_project_at ON calls(project_id, at);",
    "CREATE TABLE calls_v2 (
         id          INTEGER PRIMARY KEY AUTOINCREMENT,
         at          INTEGER NOT NULL,
         session     TEXT NOT NULL,
         client      TEXT,
         project_id  TEXT,
         tool        TEXT NOT NULL,
         args        TEXT NOT NULL,
         outcome     TEXT NOT NULL CHECK (outcome IN ('ok', 'error', 'denied', 'started')),
         detail      TEXT,
         result_size INTEGER,
         duration_ms INTEGER NOT NULL,
         approval    TEXT CHECK (approval IN ('auto', 'once', 'session', 'window')),
         plan_hash   TEXT
     );
     INSERT INTO calls_v2 (id, at, session, client, project_id, tool, args, outcome, detail, result_size, duration_ms)
         SELECT id, at, session, client, project_id, tool, args, outcome, detail, result_size, duration_ms FROM calls;
     DROP TABLE calls;
     ALTER TABLE calls_v2 RENAME TO calls;
     CREATE INDEX calls_project_at ON calls(project_id, at);
     CREATE INDEX calls_at ON calls(at);",
    "CREATE TABLE calls_v3 (
         id          INTEGER PRIMARY KEY AUTOINCREMENT,
         at          INTEGER NOT NULL,
         session     TEXT NOT NULL,
         client      TEXT,
         project_id  TEXT,
         tool        TEXT NOT NULL,
         args        TEXT NOT NULL,
         outcome     TEXT NOT NULL CHECK (outcome IN ('ok', 'error', 'denied', 'started')),
         detail      TEXT,
         result_size INTEGER,
         duration_ms INTEGER NOT NULL,
         approval    TEXT CHECK (approval IN ('auto', 'once', 'session', 'window', 'policy')),
         plan_hash   TEXT
     );
     INSERT INTO calls_v3 SELECT id, at, session, client, project_id, tool, args, outcome, detail, result_size,
         duration_ms, approval, plan_hash FROM calls;
     DELETE FROM sqlite_sequence WHERE name = 'calls_v3';
     INSERT INTO sqlite_sequence (name, seq) SELECT 'calls_v3', seq FROM sqlite_sequence WHERE name = 'calls';
     DROP TABLE calls;
     ALTER TABLE calls_v3 RENAME TO calls;
     CREATE INDEX calls_project_at ON calls(project_id, at);
     CREATE INDEX calls_at ON calls(at);",
    // 넷째 판: 세션의 전송(`socket` · `stdio` · `http` · `relay-socket` — 18-m7-plan M7-25 ⑥). 옛 행은 `NULL`(모름)이다.
    "ALTER TABLE calls ADD COLUMN transport TEXT;",
];

/// 지금 판(`PRAGMA user_version`).
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

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
    /// 실행 전에 쓴 행이고 끝이 아직 쓰이지 않았다([`AuditLog::begin`]). 호출이 도는 중이거나, 끝을 쓰기 전에 프로세스가
    /// 죽었다 — 화면은 「끝을 모름」 이다. 툴 결과의 표지(`marker`)에는 나오지 않는다.
    Started,
}

/// 짧은 이름 — 이 크레이트와 앱이 쓴다. TS 생성물의 이름은 [`AuditOutcome`] 이다.
pub type Outcome = AuditOutcome;

impl AuditOutcome {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Outcome::Ok),
            "error" => Some(Outcome::Error),
            "denied" => Some(Outcome::Denied),
            "started" => Some(Outcome::Started),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Denied => "denied",
            Outcome::Started => "started",
        }
    }
}

/// 승인을 지난 호출이 어떻게 허용됐는가(05-mcp.md 「권한 모델」). 승인 경로(16-m5-plan M5-12)가 채운다 — 승인을 지나지 않은
/// 호출(`read`, 거절)은 비어 있다.
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
pub enum AuditApproval {
    /// `auto_approve` 로 묻지 않았다.
    Auto,
    /// 사람이 [이번만 허용].
    Once,
    /// 사람이 [이 세션 동안 허용] — 앞서 허용한 것으로 묻지 않은 호출도 이것이다.
    Session,
    /// CLI 호출의 「이 프로젝트 창이 열려 있는 동안 CLI 허용」(결정 M5-D5).
    Window,
    /// 툴이 05 「권한 모델」 의 규칙(사람이 본 실행 구성 · `sql` 구성의 조회만)으로 묻지 않았다 — 계획의 `policyAuto`(17-m6-plan
    /// M6-10 ③).
    Policy,
}

impl AuditApproval {
    fn parse(s: &str) -> Option<Self> {
        [
            Self::Auto,
            Self::Once,
            Self::Session,
            Self::Window,
            Self::Policy,
        ]
        .into_iter()
        .find(|a| a.as_str() == s)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Once => "once",
            Self::Session => "session",
            Self::Window => "window",
            Self::Policy => "policy",
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
    /// 승인 방식(승인을 지난 호출만 — M5-12).
    pub approval: Option<AuditApproval>,
    /// 승인이 묶인 계획의 해시(결정 M5-D30 — M5-12).
    pub plan_hash: Option<String>,
    /// 세션의 전송(`crate::Services::transport` — `socket` · `stdio` · `http` · `relay-socket`, 18-m7-plan M7-25 ⑥).
    pub transport: Option<String>,
}

/// 감사 로그 파일. 처음 쓸 때 연다.
///
/// 한 프로세스 안에서 같은 파일의 [`AuditLog`] 는 SQLite 연결 하나를 함께 쓴다 — 앱 하나에 소켓 · HTTP 세션의 것과 Host 의
/// 정리 · 목록용(`pw-host`)이 따로 있다. 연결이 따로면 같은 프로세스의 쓰기끼리 파일 잠금을 다투고, 잠금은 줄을 서지 않고
/// busy 폴링으로 잡으므로 커밋이 느린 기기(Windows CI 의 rollback 저널 커밋 약 70 ms)에서는 2초 안에 못 끼어든 쪽의 선기록이
/// 실패해 그 쓰기 호출이 거절됐다(CI run 45316714445). 한 연결의 Mutex 는 같은 프로세스의 쓰기를 차례로 세운다. 다툼은 프로세스
/// 사이(앱과 헤드리스 `pitwall mcp` · `pitwall tool`)에만 남는다. 같은 파일인지는 경로 글로 가른다(호출하는 쪽은 모두
/// [`Self::under_state`] 의 `$STATE` 를 쓴다).
pub struct AuditLog {
    file: PathBuf,
    conn: Arc<Mutex<Option<Connection>>>,
}

/// 이 프로세스에서 열린 감사 로그 파일과 그 연결 칸. 모든 [`AuditLog`] 가 버려지면 칸도 버려진다(연결이 닫힌다).
type Slot = Arc<Mutex<Option<Connection>>>;
type WeakSlot = std::sync::Weak<Mutex<Option<Connection>>>;
static OPEN: Mutex<Vec<(PathBuf, WeakSlot)>> = Mutex::new(Vec::new());

fn shared_slot(file: &Path) -> Slot {
    let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
    open.retain(|(_, slot)| slot.strong_count() > 0);
    if let Some(slot) = open
        .iter()
        .find(|(path, _)| path == file)
        .and_then(|(_, slot)| slot.upgrade())
    {
        return slot;
    }
    let slot: Slot = Arc::new(Mutex::new(None));
    open.push((file.to_path_buf(), Arc::downgrade(&slot)));
    slot
}

impl AuditLog {
    /// `$STATE/mcp/audit.db`.
    pub fn under_state(state: &Path) -> Self {
        Self::at(state.join(DIR).join(FILE_NAME))
    }

    /// 이 프로세스의 같은 파일 로그와 연결을 함께 쓴다.
    pub fn at(file: PathBuf) -> Self {
        let conn = shared_slot(&file);
        Self { file, conn }
    }

    /// 연결을 함께 쓰지 않는 로그 — 다른 프로세스의 것처럼 따로 잠금을 다툰다. 시험이 프로세스 여럿을 흉내 낼 때만 쓴다.
    #[doc(hidden)]
    pub fn separate(file: PathBuf) -> Self {
        Self {
            file,
            conn: Arc::new(Mutex::new(None)),
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// 끝난 호출 한 행을 쓰고 그 행의 id(`calls.id` — `mcp.audit.list` 의 `id`)를 돌려준다. 디스크 IO 를 하므로 비동기 문맥에서
    /// 부르지 않는다 — 세션은 기록 스레드(`server.rs` `audit_writer`)에서 부른다.
    pub fn record(&self, entry: &Entry) -> Result<i64, String> {
        self.write(true, |conn| insert(conn, entry, entry.outcome))
            .map(|id| id.expect("만들며 열었다"))
    }

    /// **선기록**: 실행 전의 호출을 결과 [`Outcome::Started`] 로 쓰고 그 행의 id 를 돌려준다(`entry` 의 결과 · 크기 · 소요는
    /// 보지 않는다 — 결과 크기는 비고 소요는 0). 끝나면 같은 id 로 [`Self::finish`] 를 부른다. 실패하면 그 호출은 돌리지
    /// 않는다(결정 M5-D11). 디스크 IO 를 한다.
    pub fn begin(&self, entry: &Entry) -> Result<i64, String> {
        let pending = Entry {
            result_size: None,
            duration: Duration::ZERO,
            ..entry.clone()
        };
        self.write(true, |conn| insert(conn, &pending, Outcome::Started))
            .map(|id| id.expect("만들며 열었다"))
    }

    /// [`Self::begin`] 이 쓴 행 `id` 를 끝난 결과 · 상세 · 크기 · 소요로 고친다(승인 방식 · 해시는 `entry` 에 있으면 고친다).
    /// 행 수는 늘지 않는다. 그 행이 그 세션의 `started` 행이 아니면(지워짐 — 보관 정리 · `mcp.audit.clear`) 아무것도 고치지
    /// 않고 `false`. 디스크 IO 를 한다.
    pub fn finish(&self, id: i64, entry: &Entry) -> Result<bool, String> {
        self.write(false, |conn| update(conn, id, entry))
            .map(|changed| changed.unwrap_or(false))
    }

    /// 보관 기간이 지난 행(시각이 `days` 일보다 앞선 것)을 지우고 지운 수를 돌려준다. `days` 가 0 이면 지우지 않는다. 파일이
    /// 없으면 만들지 않는다. 디스크 IO 를 한다.
    pub fn prune(&self, days: u32) -> Result<usize, String> {
        if days == 0 {
            return Ok(0);
        }
        let cutoff = now_millis().saturating_sub(i64::from(days).saturating_mul(DAY_MS));
        self.write(false, |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let n = tx.execute("DELETE FROM calls WHERE at < ?1", params![cutoff])?;
            tx.commit()?;
            Ok(n)
        })
        .map(Option::unwrap_or_default)
    }

    /// 그 프로젝트의 행을 모두 지우고 지운 수를 돌려준다(`mcp.audit.clear`). 프로젝트 없는 행 · 다른 프로젝트의 행은 남는다.
    /// 파일이 없으면 만들지 않는다. 디스크 IO 를 한다.
    pub fn clear(&self, project_id: &str) -> Result<usize, String> {
        self.write(false, |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let n = tx.execute(
                "DELETE FROM calls WHERE project_id = ?1",
                params![project_id],
            )?;
            tx.commit()?;
            Ok(n)
        })
        .map(Option::unwrap_or_default)
    }

    /// 쓰기. `create` 가 아니고 파일이 아직 없으면 만들지 않고 `None`.
    fn write<T>(
        &self,
        create: bool,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> Result<Option<T>, String> {
        let mut guard = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            if !create && !self.file.exists() {
                return Ok(None);
            }
            *guard = Some(open(&self.file).map_err(|e| format!("{}: {e}", self.file.display()))?);
        }
        let conn = guard.as_mut().expect("방금 채웠다");
        let result = f(conn);
        if result.is_err() {
            // 연결이 상한 경우(파일이 지워짐 등) 다음 기록에서 다시 연다.
            *guard = None;
        }
        result
            .map(Some)
            .map_err(|e| format!("{}: {e}", self.file.display()))
    }
}

/// 전역 `[mcp]` 표(`settings.toml` 의 `mcp`)에서 보관 일수를 읽는다. **전역 설정만 받는다** — 프로젝트 설정의 `[mcp.audit]` 은
/// 효과가 없다(결정 M5-D10). 값이 없으면 [`DEFAULT_RETENTION_DAYS`], 0 은 지우지 않음. 0 이상의 정수가 아니면 기본값을 쓰고
/// 경고 문장을 함께 돌려준다 — 잘못 쓴 값으로 기록이 지워지는 쪽보다 남는 쪽이 낫다.
pub fn retention_days(global_mcp: Option<&Value>) -> (u32, Option<String>) {
    let value = global_mcp
        .and_then(|mcp| mcp.get(SETTINGS_TABLE))
        .and_then(|audit| audit.get(RETENTION_KEY));
    match value {
        None => (DEFAULT_RETENTION_DAYS, None),
        Some(v) => match v.as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(days) => (days, None),
            None => (
                DEFAULT_RETENTION_DAYS,
                Some(format!(
                    "mcp.audit.retention_days must be a whole number of days (0 keeps everything), found {v}; \
                     using {DEFAULT_RETENTION_DAYS}"
                )),
            ),
        },
    }
}

/// 프로젝트 설정의 `[mcp]`(공유 `project.toml` · `.local.toml` · 개인 전용 `project.toml` 을 합친 것)에 `[mcp.audit]` 이
/// 있으면 그것이 효과가 없다는 경고 문장. 그 표는 전역 설정에서만 읽는다(결정 M5-D10).
pub fn project_table_warning(project_mcp: Option<&Value>) -> Option<String> {
    project_mcp?.get(SETTINGS_TABLE).map(|_| {
        "mcp.audit in the project settings has no effect; the audit log retention is read only from \
         the global settings (settings.toml)"
            .to_owned()
    })
}

/// 전역 설정의 보관 일수로 지금 한 번 지운다. 잘못된 값의 경고 · 결과를 로그에 남긴다. 디스크 IO 를 한다.
pub fn prune_by_settings(log: &AuditLog, global: &pw_core::config::ConfigStore) -> usize {
    let snapshot = global.snapshot();
    let (days, warning) = retention_days(snapshot.get("settings", crate::exposure::KEY));
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
    match log.prune(days) {
        Ok(n) => {
            if n > 0 {
                tracing::info!(deleted = n, days, "pruned the mcp audit log");
            }
            n
        }
        Err(e) => {
            tracing::warn!(%e, "cannot prune the mcp audit log");
            0
        }
    }
}

/// 보관 정리: 지금 한 번, 그 뒤 [`PRUNE_EVERY`] 마다 [`prune_by_settings`](설정은 그때마다 다시 읽는다). `cancel` 이 취소되면
/// 끝난다. 앱이 시작할 때 띄운다(헤드리스 `pitwall mcp` · `pitwall tool` 은 띄우지 않는다 — 짧게 사는 프로세스다).
pub async fn keep_pruned(
    log: Arc<AuditLog>,
    global: pw_core::config::ConfigStore,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        let (log_now, global_now) = (Arc::clone(&log), global.clone());
        if let Err(e) =
            tokio::task::spawn_blocking(move || prune_by_settings(&log_now, &global_now)).await
        {
            tracing::warn!(%e, "the mcp audit log prune task failed");
        }
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(PRUNE_EVERY) => {}
        }
    }
}

/// 쓰인 감사 행 하나(`mcp.audit.list` 의 원소). 열은 [`Entry`] 와 같고 `id` · `at` 이 더 있다.
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
    /// 걸린 시간. 결과가 `started` 면 0 이다(아직 · 끝을 모름).
    #[specta(type = pw_rpc::JsNumber)]
    pub duration_ms: i64,
    /// 승인 방식. 승인을 지나지 않은 호출은 `null`.
    pub approval: Option<AuditApproval>,
    /// 승인이 묶인 계획의 해시. 승인을 지나지 않은 호출은 `null`.
    pub plan_hash: Option<String>,
    /// 세션의 전송(`socket` · `stdio` · `http` · `relay-socket`). 넷째 판 전에 쓴 행은 `null`.
    pub transport: Option<String>,
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
                "SELECT id, at, session, client, project_id, tool, args, outcome, detail, result_size, duration_ms,
                        approval, plan_hash, transport
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
                        let approval: Option<String> = row.get(11)?;
                        Ok(AuditRow {
                            id: row.get(0)?,
                            at: row.get(1)?,
                            session: row.get(2)?,
                            client: row.get(3)?,
                            project_id: row.get(4)?,
                            tool: row.get(5)?,
                            args: row.get(6)?,
                            // 스키마의 CHECK 가 값을 묶는다. 다른 값(뒤 판이 더한 값을 앞 판이 읽음)은 오류로 본다.
                            outcome: Outcome::parse(&outcome).unwrap_or(Outcome::Error),
                            detail: row.get(8)?,
                            result_size: row.get(9)?,
                            duration_ms: row.get(10)?,
                            approval: approval.as_deref().and_then(AuditApproval::parse),
                            plan_hash: row.get(12)?,
                            transport: row.get(13)?,
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
    open_waiting(file, UPGRADE_LOCK_TIMEOUT)
}

/// [`open`] — 판 올림 잠금을 `wait` 까지 기다린다(시험이 짧게 준다).
fn open_waiting(file: &Path, wait: Duration) -> rusqlite::Result<Connection> {
    let io = |path: &Path, e: std::io::Error| {
        rusqlite::Error::InvalidPath(PathBuf::from(format!("{}: {e}", path.display())))
    };
    if let Some(dir) = file.parent() {
        private_dir(dir).map_err(|e| io(dir, e))?;
    }
    private_file(file).map_err(|e| io(file, e))?;
    let mut conn = Connection::open(file)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    use_wal(&conn);
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if (current as usize) < MIGRATIONS.len() {
        let _upgrading = upgrade_lock(file, wait).map_err(|e| io(&upgrade_lock_path(file), e))?;
        // 기다린 사이 다른 프로세스가 올렸으면 쓰기 잠금 없이 여기서 끝난다.
        let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        for (from, step) in MIGRATIONS.iter().enumerate().skip(current as usize) {
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
    // 연결마다의 설정이라 열 때마다 건다. WAL 의 기본(`FULL`)이지만 빌드 설정에 기대지 않는다.
    conn.pragma_update(None, "synchronous", "FULL")?;
    Ok(conn)
}

/// 판 올림 잠금 파일(`audit.db-upgrade`). 내용은 없다 — 지우지 않고 둔다(지우면 기다리던 쪽과 새로 연 쪽이 다른 파일을 잠근다).
fn upgrade_lock_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push("-upgrade");
    PathBuf::from(name)
}

/// 판 올림을 프로세스 사이에서 한 번에 하나만 하게 OS 파일 잠금을 쥔다(돌려준 파일을 버리면 풀린다, 프로세스가 죽어도 풀린다).
///
/// 앞 판 파일을 여러 프로세스(앱이 꺼진 동안 함께 뜬 헤드리스 `pitwall mcp` 여럿, 넘기기 순간의 헤드리스와 앱)가 처음 함께 열면,
/// 판마다의 `BEGIN IMMEDIATE` 를 모두가 busy 폴링으로 다퉜다 — 표를 새로 만들어 행을 옮기는 올림은 쓰기 잠금을 오래 쥐어, 커밋이
/// 느린 Windows 에서 2초 안에 못 끼어든 연결의 열기가 `database is locked` 로 실패했다(CI run 45519711273). 잠금은 줄을 세우고,
/// 기다린 쪽은 판을 다시 읽어 이미 올랐으면 쓰기 잠금 없이 넘어간다. 평상 쓰기(올림이 필요 없는 열기)는 이 잠금을 쥐지 않는다.
/// 잠금 안에서도 판마다 `IMMEDIATE` 로 다시 읽는다 — 이 잠금을 모르는 판의 프로세스와도 맞게. 권고 잠금이라 이 잠금을 아는
/// 코드끼리만 줄을 선다. `wait` 안에 못 쥐면 `TimedOut`([`UPGRADE_LOCK_TIMEOUT`]).
fn upgrade_lock(file: &Path, wait: Duration) -> std::io::Result<fs::File> {
    let path = upgrade_lock_path(file);
    private_file(&path)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(fs::TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("another process held the upgrade lock for {wait:?}"),
                ));
            }
            Err(fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// 감사 로그는 WAL 저널에 `synchronous = FULL` 로 쓴다(05-mcp 「감사 로그」).
///
/// 내구성은 rollback 저널 때와 같다 — 커밋마다 디스크에 동기화하므로 커밋된 선기록은 전원이 나가도 남는다(선기록은 「쓰기 툴이 돌기
/// 전에 그 호출이 기록에 남는다」 는 보장이라 레지스트리(state.db)의 `NORMAL` 을 쓰지 않는다). 바뀌는 것은 다툼이다: WAL 에서는
/// 읽기가 쓰기를 막지 않고, 커밋이 저널 파일을 만들고 지우지 않아 쓰기 잠금을 쥐는 시간이 준다 — 프로세스 여럿(앱과 헤드리스
/// `pitwall mcp` · `pitwall tool`)이 같은 파일에 쓸 때 busy 폴링으로 2초 안에 못 끼어드는 일이 준다(CI run 45316714445 의 Windows).
/// `-wal` · `-shm` 은 SQLite 가 본 파일과 같은 권한(0600)으로 만든다. WAL 은 로컬 파일 시스템을 전제한다(`$STATE` 는 로컬).
///
/// 모드는 파일 머리에 남는다 — 옛 판(rollback)의 파일도 처음 연 연결이 바꾸고, 다른 연결은 다음 트랜잭션부터 따른다. 다른 연결이
/// 쓰는 중이라 이번에 못 바꾸면(바꾸기는 busy 대기 없이 거절될 수 있다) 이 연결은 rollback 으로 쓰고 다음 열기에서 다시 해 본다 —
/// 열기를 실패시키지 않는다(`pw_core::registry::store` 의 `use_wal` 과 같은 규칙).
fn use_wal(conn: &Connection) {
    let _ =
        conn.pragma_update_and_check(None, "journal_mode", "wal", |row| row.get::<_, String>(0));
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

fn insert(conn: &mut Connection, e: &Entry, outcome: Outcome) -> rusqlite::Result<i64> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO calls (at, session, client, project_id, tool, args, outcome, detail, result_size, duration_ms,
                            approval, plan_hash, transport)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            now_millis(),
            e.session,
            e.client,
            e.project_id,
            clip(&e.tool, MAX_TOOL),
            clip(&e.args, MAX_SUMMARY),
            outcome.as_str(),
            e.detail.as_deref().map(|d| clip(d, MAX_DETAIL)),
            e.result_size.map(|n| n as i64),
            e.duration.as_millis() as i64,
            e.approval.map(AuditApproval::as_str),
            e.plan_hash.as_deref().map(|h| clip(h, MAX_DETAIL)),
            e.transport.as_deref().map(|t| clip(t, MAX_TOOL)),
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

/// [`AuditLog::finish`]: 그 세션의 `started` 행만 고친다 — 다른 세션 · 이미 끝난 행을 덮지 않는다.
fn update(conn: &mut Connection, id: i64, e: &Entry) -> rusqlite::Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE calls
         SET outcome = ?1, detail = ?2, result_size = ?3, duration_ms = ?4,
             approval = COALESCE(?5, approval), plan_hash = COALESCE(?6, plan_hash)
         WHERE id = ?7 AND session = ?8 AND outcome = 'started'",
        params![
            e.outcome.as_str(),
            e.detail.as_deref().map(|d| clip(d, MAX_DETAIL)),
            e.result_size.map(|n| n as i64),
            e.duration.as_millis() as i64,
            e.approval.map(AuditApproval::as_str),
            e.plan_hash.as_deref().map(|h| clip(h, MAX_DETAIL)),
            id,
            e.session,
        ],
    )?;
    tx.commit()?;
    Ok(changed == 1)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
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

    /// 디버그 식(`condition` · `hitCondition` · `logMessage` — 17-m6-plan M6-20)도 원문 키다. 단어로 견주므로 `login` ·
    /// `catalog` · `logic` · `conditional` 처럼 한 단어 안에 든 것은 아니다.
    #[test]
    fn breakpoint_expressions_keep_only_their_length() {
        let summary: Value = serde_json::from_str(&summarize(&json!({
            "condition": "secret.len() > 3",
            "hitCondition": ">= 2",
            "logMessage": "token={token}",
            "login": "kept",
            "catalog": "kept",
            "logic": "kept",
            "conditional": "kept",
            "line": 4,
        })))
        .unwrap();
        assert_eq!(
            summary,
            json!({
                "condition": "[16 chars]",
                "hitCondition": "[4 chars]",
                "logMessage": "[13 chars]",
                "login": "kept",
                "catalog": "kept",
                "logic": "kept",
                "conditional": "kept",
                "line": 4,
            })
        );
    }

    /// 명령줄 원문 · env 의 값(`command` · `args` · `env` — 17-m6-plan M6-13)도 원문 키다: 실행 구성 쓰기 툴의 셸 명령 · 인자 ·
    /// env 표 · `envFile`, 외부 도구의 argv 가 감사 행에 남지 않는다. `environment` 처럼 한 단어 안에 든 것은 아니다.
    #[test]
    fn command_lines_and_env_values_keep_only_their_length() {
        let summary: Value = serde_json::from_str(&summarize(&json!({
            "id": "dev",
            "config": {
                "name": "Dev",
                "type": "shell",
                "command": "deploy --host prod-db-7",
                "args": ["--bin", "secret-bin"],
                "env": {"NODE_ENV": "development"},
                "envFile": [".env.local"],
                "environment": "kept",
            },
        })))
        .unwrap();
        assert_eq!(
            summary,
            json!({
                "id": "dev",
                "config": {
                    "name": "Dev",
                    "type": "shell",
                    "command": "[23 chars]",
                    "args": "[not shown]",
                    "env": "[not shown]",
                    "envFile": "[not shown]",
                    "environment": "kept",
                },
            })
        );
        let argv = summarize(&json!({"id": "zed", "command": ["zed", "--wait", "{file}"]}));
        assert_eq!(argv, r#"{"command":"[not shown]","id":"zed"}"#);
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
            approval: None,
            plan_hash: None,
            transport: None,
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
            approval: None,
            plan_hash: None,
            transport: None,
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

    fn entry(tool: &str, project: Option<&str>) -> Entry {
        Entry {
            session: "s".into(),
            client: None,
            project_id: project.map(str::to_owned),
            tool: tool.into(),
            args: "{}".into(),
            outcome: Outcome::Ok,
            detail: None,
            result_size: None,
            duration: Duration::from_millis(1),
            approval: None,
            plan_hash: None,
            transport: None,
        }
    }

    fn version(file: &Path) -> u32 {
        Connection::open(file)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap()
    }

    fn all(log: &AuditLog) -> Vec<AuditRow> {
        let (mut rows, _) = log
            .list(&Query {
                limit: 10_000,
                ..Query::default()
            })
            .unwrap();
        rows.reverse();
        rows
    }

    /// 첫째 판 파일(M5-10 까지의 앱이 쓴 것)을 열면 둘째 판으로 올라가고 행 · id 가 그대로 남는다. 올린 뒤에는 `started` 와 승인
    /// 칸을 쓸 수 있다.
    #[test]
    fn a_first_version_file_is_upgraded_and_keeps_its_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("audit.db");
        {
            let conn = Connection::open(&file).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            for (id, tool, outcome) in [(3, "project_list", "ok"), (7, "nope", "denied")] {
                conn.execute(
                    "INSERT INTO calls (id, at, session, client, project_id, tool, args, outcome, detail, result_size, duration_ms)
                     VALUES (?1, 1000, 's0', 'old 1', 'p1', ?2, '{}', ?3, NULL, 5, 2)",
                    params![id, tool, outcome],
                )
                .unwrap();
            }
        }
        let log = AuditLog::at(file.clone());
        let rows = all(&log);
        assert_eq!(version(&file), SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 4);
        let kept: Vec<(i64, &str, Outcome)> = rows
            .iter()
            .map(|r| (r.id, r.tool.as_str(), r.outcome))
            .collect();
        assert_eq!(
            kept,
            [
                (3, "project_list", Outcome::Ok),
                (7, "nope", Outcome::Denied)
            ]
        );
        assert_eq!(rows[0].client.as_deref(), Some("old 1"));
        assert_eq!(rows[0].result_size, Some(5));
        assert_eq!(
            (rows[0].approval, rows[0].plan_hash.as_deref()),
            (None, None)
        );
        // 새 판의 값을 쓸 수 있고, id 는 옮긴 가장 큰 id 뒤에서 이어진다.
        let started = log
            .begin(&Entry {
                approval: Some(AuditApproval::Once),
                plan_hash: Some("sha256:abc".into()),
                ..entry("fake_write", Some("p1"))
            })
            .unwrap();
        assert_eq!(started, 8);
        let row = all(&log).pop().unwrap();
        assert_eq!(row.outcome, Outcome::Started);
        assert_eq!(row.approval, Some(AuditApproval::Once));
        assert_eq!(row.plan_hash.as_deref(), Some("sha256:abc"));
        // 다시 열어도 판을 또 올리지 않는다.
        drop(log);
        assert_eq!(all(&AuditLog::at(file.clone())).len(), 3);
        assert_eq!(version(&file), SCHEMA_VERSION);
    }

    /// 셋째 판 파일(M7-25 전의 앱이 쓴 것)을 열면 넷째 판으로 올라가 행 · id · 승인 칸이 그대로 남고 옛 행의 전송은 `null` 이다.
    /// 새 행은 전송을 쓴다(18-m7-plan M7-25 ⑥).
    #[test]
    fn a_third_version_file_keeps_its_rows_and_takes_the_transport() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("audit.db");
        {
            let conn = Connection::open(&file).unwrap();
            for step in &MIGRATIONS[..3] {
                conn.execute_batch(step).unwrap();
            }
            conn.pragma_update(None, "user_version", 3).unwrap();
            for (id, approval) in [(4, Some("policy")), (7, None)] {
                conn.execute(
                    "INSERT INTO calls (id, at, session, client, tool, args, outcome, duration_ms, approval)
                     VALUES (?1, 1000, 's0', 'c 1', 't', '{}', 'ok', 3, ?2)",
                    params![id, approval],
                )
                .unwrap();
            }
        }
        let log = AuditLog::at(file.clone());
        let rows = all(&log);
        assert_eq!(version(&file), SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 4);
        type Kept<'a> = (i64, Option<AuditApproval>, Option<&'a str>, Option<&'a str>);
        let kept: Vec<Kept> = rows
            .iter()
            .map(|r| {
                (
                    r.id,
                    r.approval,
                    r.client.as_deref(),
                    r.transport.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            kept,
            [
                (4, Some(AuditApproval::Policy), Some("c 1"), None),
                (7, None, Some("c 1"), None),
            ]
        );
        log.record(&Entry {
            transport: Some("http".into()),
            ..entry("project_list", None)
        })
        .unwrap();
        let last = all(&log).pop().unwrap();
        assert_eq!((last.id, last.transport.as_deref()), (8, Some("http")));
    }

    /// 넷째 판 올림이 실패하면(여기서는 전송 칸이 이미 있어 `ADD COLUMN` 이 실패) 판 · 행이 셋째 판 그대로다 — 판 올림과 그 판의
    /// 단계가 한 트랜잭션이다. 열기는 오류이고 파일을 고치지 않는다.
    #[test]
    fn a_failed_fourth_version_step_leaves_the_third_version() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("audit.db");
        {
            let conn = Connection::open(&file).unwrap();
            for step in &MIGRATIONS[..3] {
                conn.execute_batch(step).unwrap();
            }
            conn.execute(
                "INSERT INTO calls (id, at, session, tool, args, outcome, duration_ms)
                 VALUES (5, 1000, 's0', 't', '{}', 'ok', 0)",
                [],
            )
            .unwrap();
            // 넷째 판의 단계를 막는다: 같은 이름 칸을 미리 둔다.
            conn.execute_batch("ALTER TABLE calls ADD COLUMN transport TEXT;")
                .unwrap();
            conn.pragma_update(None, "user_version", 3).unwrap();
        }
        let log = AuditLog::at(file.clone());
        assert!(
            log.list(&Query {
                limit: 10,
                ..Query::default()
            })
            .is_err()
        );
        assert!(log.record(&entry("t", None)).is_err());
        assert_eq!(version(&file), 3, "실패한 판 올림은 판을 올리지 않는다");
        let conn = Connection::open(&file).unwrap();
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM calls")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(ids, [5]);
    }

    /// 둘째 판 파일(M5 의 앱이 쓴 것)을 열면 셋째 판으로 올라가 행 · 승인 칸이 그대로 남고, 승인 방식 `policy` 를 쓸 수 있다.
    /// 올리기 전에 지운 가장 큰 id 도 다시 쓰지 않는다(`sqlite_sequence` 를 넘긴다).
    #[test]
    fn a_second_version_file_is_upgraded_and_takes_the_policy_approval() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("audit.db");
        {
            let conn = Connection::open(&file).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute_batch(MIGRATIONS[1]).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
            for (id, approval) in [(3, Some("once")), (9, Some("window")), (12, None)] {
                conn.execute(
                    "INSERT INTO calls (id, at, session, tool, args, outcome, duration_ms, approval, plan_hash)
                     VALUES (?1, 1000, 's0', 't', '{}', 'ok', 0, ?2, ?3)",
                    params![id, approval, approval.map(|_| "sha256:h")],
                )
                .unwrap();
            }
            conn.execute("DELETE FROM calls WHERE id = 12", []).unwrap();
            assert!(
                conn.execute(
                    "INSERT INTO calls (at, session, tool, args, outcome, duration_ms, approval)
                     VALUES (1, 's', 't', '{}', 'ok', 0, 'policy')",
                    [],
                )
                .is_err(),
                "둘째 판은 policy 를 받지 않는다"
            );
        }
        let log = AuditLog::at(file.clone());
        let rows = all(&log);
        assert_eq!(version(&file), SCHEMA_VERSION);
        let kept: Vec<(i64, Option<AuditApproval>, Option<&str>)> = rows
            .iter()
            .map(|r| (r.id, r.approval, r.plan_hash.as_deref()))
            .collect();
        assert_eq!(
            kept,
            [
                (3, Some(AuditApproval::Once), Some("sha256:h")),
                (9, Some(AuditApproval::Window), Some("sha256:h")),
            ]
        );
        let id = log
            .begin(&Entry {
                approval: Some(AuditApproval::Policy),
                plan_hash: Some("sha256:p".into()),
                ..entry("run_start", Some("p1"))
            })
            .unwrap();
        assert_eq!(id, 13, "지운 가장 큰 id(12) 뒤에서 이어진다");
        assert_eq!(
            all(&log).pop().unwrap().approval,
            Some(AuditApproval::Policy)
        );
    }

    /// 한 프로세스 안의 같은 파일 로그(앱의 소켓 · HTTP 세션과 Host 의 정리)는 연결 하나를 함께 쓴다 — 같은 프로세스의 쓰기가
    /// 파일 잠금을 다투지 않고 Mutex 로 줄을 선다. 다른 파일 · [`AuditLog::separate`] 는 따로다. 모두 버리면 칸도 버려진다.
    fn lock(slot: &Slot) -> std::sync::MutexGuard<'_, Option<Connection>> {
        slot.lock().unwrap()
    }

    /// 감사 로그는 WAL · `synchronous = FULL` 로 쓴다 — 옛 판(rollback 저널)의 첫째 판 파일도 열면 WAL 로 바뀌고 판이 오른다.
    /// 모드는 파일에 남아 다른 연결(다른 프로세스)도 WAL 로 본다.
    #[test]
    fn the_log_writes_with_wal_and_full_sync_and_moves_a_rollback_file_over() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("audit.db");
        {
            let conn = Connection::open(&file).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            let mode: String = conn
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "delete", "옛 판은 rollback 저널이다");
        }
        let log = AuditLog::separate(file.clone());
        log.record(&entry("t", None)).unwrap();
        {
            let guard = lock(&log.conn);
            let conn = guard.as_ref().unwrap();
            let mode: String = conn
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .unwrap();
            let sync: i64 = conn
                .pragma_query_value(None, "synchronous", |r| r.get(0))
                .unwrap();
            assert_eq!((mode.as_str(), sync), ("wal", 2), "WAL · FULL(2)");
        }
        assert_eq!(version(&file), SCHEMA_VERSION);
        let other: String = Connection::open(&file)
            .unwrap()
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(other, "wal");
    }

    /// WAL 의 곁 파일(`-wal` · `-shm`)도 본 파일처럼 이 사용자만 읽는다(0600) — 커밋 전의 행이 `-wal` 에 있다.
    #[cfg(unix)]
    #[test]
    fn the_wal_side_files_are_as_private_as_the_log() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::under_state(tmp.path());
        log.record(&entry("t", None)).unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{suffix}", log.file().display()));
            let mode = fs::metadata(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{}", path.display());
        }
    }

    #[test]
    fn logs_of_one_file_share_one_connection_in_a_process() {
        let tmp = tempfile::tempdir().unwrap();
        let a = AuditLog::under_state(tmp.path());
        let b = AuditLog::under_state(tmp.path());
        assert!(Arc::ptr_eq(&a.conn, &b.conn));
        assert!(!Arc::ptr_eq(
            &a.conn,
            &AuditLog::under_state(&tmp.path().join("other")).conn
        ));
        assert!(!Arc::ptr_eq(
            &a.conn,
            &AuditLog::separate(a.file().to_path_buf()).conn
        ));
        a.record(&entry("t", None)).unwrap();
        assert!(lock(&b.conn).is_some(), "b 는 a 가 연 연결을 본다");
        let weak = Arc::downgrade(&a.conn);
        drop((a, b));
        assert!(weak.upgrade().is_none(), "버린 로그의 연결이 남았다");
    }

    /// 여러 연결(프로세스 여럿 — 앱과 헤드리스 `pitwall mcp`)이 첫째 판 파일을 **동시에 처음 열어도** 모두 열리고 쓴다. 판
    /// 올림은 `user_version` 을 읽은 뒤 쓰므로 `BEGIN IMMEDIATE` 여야 한다 — `DEFERRED` 면 읽기 잠금을 쥔 채 쓰기 잠금을 기다리는
    /// 연결에 SQLite 가 바쁨 대기 없이 곧바로 `database is locked` 를 준다. 그 연결의 선기록이 실패하면 쓰기 호출이 거절된다.
    #[test]
    fn many_connections_upgrade_a_first_version_file_at_once() {
        const CONNECTIONS: usize = 8;
        const OLD_ROWS: i64 = 2000;
        for round in 0..20 {
            let tmp = tempfile::tempdir().unwrap();
            let file = tmp.path().join("audit.db");
            {
                let mut conn = Connection::open(&file).unwrap();
                conn.execute_batch(MIGRATIONS[0]).unwrap();
                conn.pragma_update(None, "user_version", 1).unwrap();
                let tx = conn.transaction().unwrap();
                for _ in 0..OLD_ROWS {
                    tx.execute(
                        "INSERT INTO calls (at, session, tool, args, outcome, duration_ms)
                         VALUES (1000, 's0', 't', '{}', 'ok', 0)",
                        [],
                    )
                    .unwrap();
                }
                tx.commit().unwrap();
            }
            let barrier = Arc::new(std::sync::Barrier::new(CONNECTIONS));
            let results: Vec<Result<(i64, i64), String>> = (0..CONNECTIONS)
                .map(|_| {
                    let (file, barrier) = (file.clone(), Arc::clone(&barrier));
                    std::thread::spawn(move || {
                        let log = AuditLog::separate(file);
                        barrier.wait();
                        Ok((
                            log.begin(&entry("fake_write", None))?,
                            log.record(&entry("t", None))?,
                        ))
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect();
            let failed: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
            assert!(failed.is_empty(), "round {round}: {failed:?}");
            assert_eq!(version(&file), SCHEMA_VERSION, "round {round}");
            let total: i64 = Connection::open(&file)
                .unwrap()
                .query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0))
                .unwrap();
            assert_eq!(total, OLD_ROWS + 2 * CONNECTIONS as i64, "round {round}");
        }
    }

    /// 판 올림은 판 올림 잠금을 쥔 쪽만 한다 — 다른 프로세스가 잠금을 쥔 동안 열기는 판을 올리지 않고 기다리고, 풀리면 판을 다시
    /// 읽어 올린다. 평상 열기(이미 지금 판)는 잠금을 기다리지 않는다.
    #[test]
    fn opening_a_first_version_file_waits_for_the_upgrade_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("audit.db");
        {
            let conn = Connection::open(&file).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        let held = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(upgrade_lock_path(&file))
            .unwrap();
        held.lock().unwrap();
        let opener = {
            let file = file.clone();
            std::thread::spawn(move || open(&file).map(drop).map_err(|e| e.to_string()))
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!opener.is_finished(), "잠금을 기다리지 않았다");
        assert_eq!(version(&file), 1, "잠금 밖에서 올렸다");
        drop(held);
        opener.join().unwrap().unwrap();
        assert_eq!(version(&file), SCHEMA_VERSION);

        let held = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(upgrade_lock_path(&file))
            .unwrap();
        held.lock().unwrap();
        open(&file).expect("지금 판 파일은 잠금 없이 열린다");
    }

    /// 판 올림 잠금을 쥔 채 놓지 않는 프로세스가 있어도 열기는 매달리지 않는다 — 시한 뒤 오류로 끝나고(선기록 실패 → 쓰기 툴은
    /// `audit_unavailable` 로 거절) 판은 그대로다. 잠금이 풀린 뒤의 열기는 올린다.
    #[test]
    fn a_held_upgrade_lock_fails_the_open_after_the_wait() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("audit.db");
        {
            let conn = Connection::open(&file).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        let held = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(upgrade_lock_path(&file))
            .unwrap();
        held.lock().unwrap();
        let opener = {
            let file = file.clone();
            std::thread::spawn(move || {
                open_waiting(&file, Duration::from_millis(200))
                    .map(drop)
                    .map_err(|e| e.to_string())
            })
        };
        let started = std::time::Instant::now();
        while !opener.is_finished() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "열기가 잠금에 매달렸다"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let err = opener.join().unwrap().expect_err("잠금을 쥔 채 열렸다");
        assert!(err.contains("upgrade lock"), "{err}");
        assert_eq!(version(&file), 1);
        drop(held);
        open(&file).unwrap();
        assert_eq!(version(&file), SCHEMA_VERSION);
    }

    /// 행 id 는 다시 쓰이지 않는다(R5): 가장 큰 id 를 지워도(보관 정리 · 지우기) 새 행은 더 큰 id 다 — 커서와 `mcp.activity` 가
    /// 다른 행을 가리키지 않는다. 첫째 판에서 올라온 파일도 같다.
    #[test]
    fn ids_are_not_reused_after_the_largest_rows_are_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::at(tmp.path().join("audit.db"));
        let ids: Vec<i64> = (0..3)
            .map(|_| log.record(&entry("t", Some("p1"))).unwrap())
            .collect();
        assert_eq!(log.clear("p1").unwrap(), 3);
        let next = log.record(&entry("t", Some("p1"))).unwrap();
        assert!(next > ids[2], "{next} after {ids:?}");

        let file = tmp.path().join("v1.db");
        {
            let conn = Connection::open(&file).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO calls (id, at, session, tool, args, outcome, duration_ms)
                 VALUES (41, 1000, 's', 't', '{}', 'ok', 0)",
                [],
            )
            .unwrap();
        }
        let upgraded = AuditLog::at(file);
        assert_eq!(upgraded.prune(1).unwrap(), 1, "1970 년의 행");
        assert_eq!(upgraded.record(&entry("t", None)).unwrap(), 42);
    }

    /// 끝 기록은 같은 행을 고친다 — 행 수가 늘지 않는다. 그 세션의 `started` 행만 고친다.
    #[test]
    fn finishing_updates_the_started_row_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::at(tmp.path().join("audit.db"));
        let id = log
            .begin(&Entry {
                approval: Some(AuditApproval::Session),
                plan_hash: Some("h1".into()),
                ..entry("fake_write", Some("p1"))
            })
            .unwrap();
        assert_eq!(all(&log)[0].outcome, Outcome::Started);
        let done = Entry {
            outcome: Outcome::Ok,
            detail: Some("wrote 1".into()),
            result_size: Some(12),
            duration: Duration::from_millis(30),
            ..entry("fake_write", Some("p1"))
        };
        assert!(log.finish(id, &done).unwrap());
        let rows = all(&log);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(
            (
                row.id,
                row.outcome,
                row.detail.as_deref(),
                row.result_size,
                row.duration_ms
            ),
            (id, Outcome::Ok, Some("wrote 1"), Some(12), 30)
        );
        // 시작 때 쓴 승인 방식 · 해시는 끝 기록이 비어 있어도 남는다.
        assert_eq!(row.approval, Some(AuditApproval::Session));
        assert_eq!(row.plan_hash.as_deref(), Some("h1"));
        // 이미 끝난 행 · 다른 세션의 행은 고치지 않는다.
        assert!(!log.finish(id, &done).unwrap());
        let other = log.begin(&entry("fake_write", None)).unwrap();
        assert!(
            !log.finish(
                other,
                &Entry {
                    session: "someone else".into(),
                    ..done.clone()
                }
            )
            .unwrap()
        );
        assert_eq!(all(&log)[1].outcome, Outcome::Started);
    }

    fn age(log: &AuditLog, id: i64, days: i64) {
        Connection::open(log.file())
            .unwrap()
            .execute(
                "UPDATE calls SET at = at - ?1 WHERE id = ?2",
                params![days * DAY_MS, id],
            )
            .unwrap();
    }

    /// 보관 정리는 기한이 지난 행만 지운다. 0 이면 지우지 않는다. 파일이 없으면 만들지 않는다.
    #[test]
    fn pruning_removes_only_rows_past_the_retention() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::at(tmp.path().join("audit.db"));
        assert_eq!(log.prune(90).unwrap(), 0);
        assert!(
            !log.file().exists(),
            "지울 것이 없으면 파일을 만들지 않는다"
        );
        let old = log.record(&entry("old", None)).unwrap();
        let edge = log.record(&entry("edge", Some("p1"))).unwrap();
        let started = log.begin(&entry("started_old", Some("p1"))).unwrap();
        let fresh = log.record(&entry("fresh", None)).unwrap();
        age(&log, old, 91);
        age(&log, edge, 89);
        age(&log, started, 100);
        assert_eq!(log.prune(0).unwrap(), 0, "0 은 지우지 않는다");
        assert_eq!(all(&log).len(), 4);
        assert_eq!(log.prune(90).unwrap(), 2);
        let left: Vec<i64> = all(&log).iter().map(|r| r.id).collect();
        assert_eq!(left, [edge, fresh]);
    }

    #[test]
    fn clearing_removes_only_that_projects_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let log = AuditLog::at(tmp.path().join("audit.db"));
        assert_eq!(log.clear("p1").unwrap(), 0);
        assert!(!log.file().exists());
        for project in [Some("p1"), None, Some("p2"), Some("p1")] {
            log.record(&entry("t", project)).unwrap();
        }
        assert_eq!(log.clear("p1").unwrap(), 2);
        let left: Vec<Option<String>> = all(&log).into_iter().map(|r| r.project_id).collect();
        assert_eq!(left, [None, Some("p2".to_owned())]);
    }

    #[test]
    fn retention_is_read_from_the_global_mcp_table() {
        assert_eq!(retention_days(None), (DEFAULT_RETENTION_DAYS, None));
        assert_eq!(
            retention_days(Some(&json!({"preset": "full"}))),
            (DEFAULT_RETENTION_DAYS, None)
        );
        assert_eq!(
            retention_days(Some(&json!({"audit": {"retention_days": 30}}))),
            (30, None)
        );
        assert_eq!(
            retention_days(Some(&json!({"audit": {"retention_days": 0}}))),
            (0, None)
        );
        for bad in [json!(-1), json!("30"), json!(1.5), json!(u64::MAX)] {
            let (days, warning) = retention_days(Some(&json!({"audit": {"retention_days": bad}})));
            assert_eq!(days, DEFAULT_RETENTION_DAYS, "{bad}");
            assert!(warning.unwrap().contains("retention_days"), "{bad}");
        }
    }
}
