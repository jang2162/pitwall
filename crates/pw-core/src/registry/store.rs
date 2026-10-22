//! `$DATA/state.db` 의 연결 · 스키마 버전 · 마이그레이션 · 열기 실패 처리.
//!
//! 스키마 버전은 SQLite 의 `PRAGMA user_version` 이다. [`MIGRATIONS`] 의 `i` 번째 단계가 버전 `i` 를 `i + 1` 로 올리고,
//! 단계마다 `BEGIN IMMEDIATE` 트랜잭션 하나라 실패하면 그 단계 전으로 돌아간다. 단계는 지우거나 고치지 않고 뒤에 더하기만 한다
//! — 읽기 전용으로 여는 새 스키마 파일을 이 앱의 질의로 읽을 수 있다는 전제가 여기에 걸려 있다.
//!
//! 여러 프로세스(워크트리마다 띄운 앱, 이후 CLI · relay)가 같은 파일을 쓴다. 그래서 쓰기는 모두 `BEGIN IMMEDIATE` 다.
//! `BEGIN`(deferred)으로 읽고 나서 쓰기로 올라가려 하면, 다른 연결이 이미 쓰기 잠금을 쥔 채 커밋을 기다리는 경우 SQLite 가
//! 교착을 피하려고 busy handler 를 부르지 않고 바로 `database is locked` 를 돌려준다 — [`BUSY_TIMEOUT`] 이 소용없어진다.
//!
//! 열기 정책은 [`StorageStatus`] 의 변형마다 적었다. 앱은 어느 경우에도 뜬다.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

/// 레지스트리 파일 이름(`$DATA/` 아래).
pub const FILE_NAME: &str = "state.db";
/// 다른 프로세스가 쓰는 중일 때 기다리는 시간. 넘으면 잠김으로 본다.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
/// 잠김으로 메모리에 떨어진 뒤 파일을 다시 열어 보는 간격과, 그 재시도 열기가 기다리는 시간.
/// 잠김이 이어지는 동안 레지스트리 호출마다 [`BUSY_TIMEOUT`] 을 다시 기다리지 않게 한다(호출은 뮤텍스로 줄을 선다).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);
pub const RETRY_BUSY_TIMEOUT: Duration = Duration::from_millis(250);

type Migration = fn(&Transaction<'_>) -> rusqlite::Result<()>;

#[cfg(test)]
thread_local! {
    /// 시험: 이 스레드의 마이그레이션이 쓰기 잠금을 기다리다 시간을 넘긴 횟수(벽시계 대신 기다림을 센다).
    static LOCK_TIMEOUTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// 버전 `i` → `i + 1`. 길이가 곧 이 앱이 아는 최신 버전이다.
pub const MIGRATIONS: &[Migration] = &[v0_to_v1, v1_to_v2];

/// 레지스트리가 읽는 질의. 읽기 전용으로 연 새 스키마 파일을 이 질의로 읽을 수 있는지 열 때 미리 본다(`prepare` 가 열 이름을 검사한다).
pub const SELECT_PROJECTS: &str =
    "SELECT id, name, host, favorite, config_mode, slug, created_at, opened_at
     FROM projects WHERE ?1 IS NULL OR id = ?1";
pub const SELECT_ROOTS: &str = "SELECT project_id, path FROM project_roots
     WHERE ?1 IS NULL OR project_id = ?1 ORDER BY project_id, position";
pub const SELECT_TAGS: &str = "SELECT project_id, tag FROM project_tags
     WHERE ?1 IS NULL OR project_id = ?1 ORDER BY project_id, tag";
pub const SELECT_TRUST: &str = "SELECT trusted FROM project_trust WHERE project_id = ?1";

pub fn supported_version() -> u32 {
    MIGRATIONS.len() as u32
}

/// v1: 프로젝트 · 루트(멀티 루트 F1-6 을 위해 표를 나눈다) · 태그.
fn v0_to_v1(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "CREATE TABLE projects (
             id          TEXT PRIMARY KEY,
             name        TEXT NOT NULL,
             host        TEXT NOT NULL,
             favorite    INTEGER NOT NULL DEFAULT 0,
             config_mode TEXT NOT NULL CHECK (config_mode IN ('repository', 'personal')),
             slug        TEXT NOT NULL UNIQUE,
             created_at  INTEGER NOT NULL,
             opened_at   INTEGER
         );
         CREATE TABLE project_roots (
             project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
             position   INTEGER NOT NULL,
             path       TEXT NOT NULL,
             PRIMARY KEY (project_id, position),
             UNIQUE (path)
         );
         CREATE TABLE project_tags (
             project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
             tag        TEXT NOT NULL,
             PRIMARY KEY (project_id, tag)
         );
         CREATE INDEX projects_opened_at ON projects(opened_at);",
    )
}

/// v2: 프로젝트 신뢰(F1-7 최소판, [`super::trust`]). 행이 없으면 아직 묻지 않은 프로젝트다 — v1 의 기존 프로젝트는 행 없이
/// 올라오므로 미신뢰다.
fn v1_to_v2(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "CREATE TABLE project_trust (
             project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
             trusted    INTEGER NOT NULL CHECK (trusted IN (0, 1)),
             decided_at INTEGER NOT NULL
         );",
    )
}

/// 레지스트리 저장소의 상태. `ok` · `recovered` 만 쓸 수 있다 — 나머지에서 쓰기는 오류로 거절한다(쓴 것이 종료 때 사라지지 않게).
/// 정상이 아니면 웰컴 화면(M0-12)이 `reason` 을 배너로 보여 준다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StorageStatus {
    /// 파일을 정상으로 열었다.
    Ok { path: String },
    /// 손상(SQLite 파일이 아님 · 무결성 검사 실패)된 파일을 `backup` 으로 옮기고 빈 레지스트리로 새로 만들었다.
    Recovered {
        path: String,
        backup: String,
        reason: String,
    },
    /// 파일을 읽기만 한다: 이 앱보다 새 스키마(앱을 내린 경우), 또는 읽기 전용 파일 · 폴더. 목록 · 해석은 되고 쓰기는 거절한다.
    ReadOnly { path: String, reason: String },
    /// 파일을 쓸 수도 읽을 수도 없어 빈 메모리 DB 로 떴다. 파일은 건드리지 않았다. 목록은 비어 보이고 쓰기는 거절한다.
    /// 잠김이 이유면 5초(`RETRY_INTERVAL`)마다 파일을 다시 열어 본다.
    Memory { path: String, reason: String },
}

impl StorageStatus {
    /// 쓸 수 없는 이유. 쓸 수 있으면 `None`.
    pub fn unwritable_reason(&self) -> Option<&str> {
        match self {
            StorageStatus::Ok { .. } | StorageStatus::Recovered { .. } => None,
            StorageStatus::ReadOnly { reason, .. } | StorageStatus::Memory { reason, .. } => {
                Some(reason)
            }
        }
    }
}

pub struct Opened {
    pub conn: Connection,
    pub status: StorageStatus,
    /// 잠김으로 메모리에 떨어졌으면 그 시각. 레지스트리가 [`RETRY_INTERVAL`] 뒤에 파일을 다시 열어 본다 — 메모리 DB 에는
    /// 쓰기를 받지 않으므로 갈아탈 때 잃는 것이 없다.
    pub locked_at: Option<Instant>,
}

/// `file` 을 열고 최신 스키마로 올린다. 실패해도 읽을 수 있는 연결을 돌려준다.
pub fn open(file: &Path, busy: Duration) -> Opened {
    open_with(file, busy, MIGRATIONS)
}

pub fn open_with(file: &Path, busy: Duration, migrations: &[Migration]) -> Opened {
    let display = file.display().to_string();
    match try_open(file, busy, migrations) {
        Ok(Ready::Writable(conn)) => Opened {
            conn,
            status: StorageStatus::Ok { path: display },
            locked_at: None,
        },
        Ok(Ready::ReadOnly(conn, reason)) => Opened {
            conn,
            status: StorageStatus::ReadOnly {
                path: display,
                reason,
            },
            locked_at: None,
        },
        Err(Failure::Corrupt(reason)) => match recover(file, busy, migrations) {
            Ok((conn, backup)) => Opened {
                conn,
                status: StorageStatus::Recovered {
                    path: display,
                    backup: backup.display().to_string(),
                    reason,
                },
                locked_at: None,
            },
            Err(e) => memory(display, format!("{reason}; recovery failed: {e}"), None),
        },
        Err(Failure::Busy(reason)) => memory(display, reason, Some(Instant::now())),
        Err(Failure::Other(reason)) => memory(display, reason, None),
    }
}

/// [`open_read_only`] 로 연 파일의 `readOnly` 이유.
pub const READ_ONLY_BY_REQUEST: &str =
    "opened read-only by a process that does not own the registry (`pitwall mcp` without the app)";

/// 읽기만 하는 프로세스(앱 없이 뜬 `pitwall mcp`)의 열기. **마이그레이션 · 손상 복구(파일 옮기기)를 하지 않는다** — 레지스트리를
/// 소유한 앱의 일이다. 행을 쓰지 않지만(`query_only`) SQLite 의 WAL 사이드카는 다룬다: 연 동안 `-wal` · `-shm` 을 두고, 앱이
/// 비정상 종료로 남긴 `-wal` 은 마지막 연결로 닫힐 때 파일에 옮긴다(이미 커밋된 쓰기라 논리 내용은 같다, 05-mcp 「로컬 소켓과
/// 헤드리스」). 그래서 [`open`] 과 달리:
/// - 파일이 없으면 빈 메모리 DB(`memory`). 폴더를 만들지 않는다.
/// - 손상 · 이 앱보다 낮은 스키마(올려야 읽힌다)도 `memory` 다. 파일은 그대로 둔다.
/// - 그 밖에는 `readOnly`(이유 [`READ_ONLY_BY_REQUEST`], 앱보다 새 스키마면 그 이유).
pub fn open_read_only(file: &Path, busy: Duration) -> Opened {
    let display = file.display().to_string();
    if !file.is_file() {
        return memory(display, "the registry file does not exist yet".into(), None);
    }
    match try_open_read_only(file, busy) {
        Ok((conn, reason)) => Opened {
            conn,
            status: StorageStatus::ReadOnly {
                path: display,
                reason,
            },
            locked_at: None,
        },
        Err(Failure::Busy(reason)) => memory(display, reason, Some(Instant::now())),
        Err(e) => memory(display, e.into_reason(), None),
    }
}

fn try_open_read_only(file: &Path, busy: Duration) -> Result<(Connection, String), Failure> {
    // 쓰기 가능한 연결로 열고 쓰지 않는다(질의는 [`read_only`] 의 `query_only` 가 막는다). `SQLITE_OPEN_READ_ONLY` 로 WAL 파일을
    // 열면, 앱이 닫혀 `-wal` · `-shm` 이 없을 때 그 둘을 만들고 닫으면서 지우지 못해 남긴다 — 쓸 수 있는 연결은 마지막으로 닫힐
    // 때 걷는다. 만들지 않는 플래그라 파일이 없으면 만들지 않는다. 앱이 비정상 종료로 남긴 `-wal` 이 있으면 마지막 연결로 닫히며
    // 그 내용(이미 커밋된 쓰기)을 파일에 옮긴다.
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(file, flags)?;
    conn.busy_timeout(busy)?;
    let (conn, check) = match quick_check(&conn) {
        Ok(check) => (conn, check),
        Err(e) if unwritable_error(&e) => {
            drop(conn);
            open_unwritable(file, busy)?
        }
        Err(e) => return Err(e.into()),
    };
    if check != "ok" {
        return Err(Failure::Corrupt(format!("integrity check: {check}")));
    }
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let latest = supported_version();
    let reason = if version > latest {
        format!("schema version {version} is newer than this app supports ({latest})")
    } else if version < latest {
        return Err(Failure::Other(format!(
            "schema version {version} needs migrating by the app ({latest})"
        )));
    } else {
        READ_ONLY_BY_REQUEST.to_owned()
    };
    match read_only(conn, reason)? {
        Ready::ReadOnly(conn, reason) => Ok((conn, reason)),
        Ready::Writable(_) => unreachable!("read_only 은 ReadOnly 만 돌려준다"),
    }
}

enum Ready {
    Writable(Connection),
    ReadOnly(Connection, String),
}

enum Failure {
    Corrupt(String),
    Busy(String),
    Other(String),
}

impl Failure {
    fn into_reason(self) -> String {
        match self {
            Failure::Corrupt(e) | Failure::Busy(e) | Failure::Other(e) => e,
        }
    }
}

impl From<rusqlite::Error> for Failure {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => {
                Failure::Corrupt(e.to_string())
            }
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => {
                Failure::Busy(e.to_string())
            }
            _ => Failure::Other(e.to_string()),
        }
    }
}

fn try_open(file: &Path, busy: Duration, migrations: &[Migration]) -> Result<Ready, Failure> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Failure::Other(format!("{}: {e}", dir.display())))?;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(file, flags)?;
    prepare(&conn, busy)?;
    // 파일이 SQLite 가 아니면 여기서 처음 드러난다(열기 자체는 게으르다). b-tree 가 깨진 경우는 오류가 아니라 문장으로 온다.
    let (mut conn, check) = match quick_check(&conn) {
        Ok(check) => (conn, check),
        // 쓰기 금지 폴더의 WAL 파일. 읽기만 하는 연결로 다시 열면 아래 `unwritable` 이 읽기 전용으로 돌린다.
        Err(e) if unwritable_error(&e) => {
            drop(conn);
            open_unwritable(file, busy)?
        }
        Err(e) => return Err(e.into()),
    };
    if check != "ok" {
        return Err(Failure::Corrupt(format!("integrity check: {check}")));
    }
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let latest = migrations.len() as u32;
    if version > latest {
        // 새 앱이 만든 파일이다. 쓰면 새 앱이 그 파일을 못 믿게 되므로 읽기만 한다.
        return read_only(
            conn,
            format!("schema version {version} is newer than this app supports ({latest})"),
        );
    }
    if let Some(reason) = unwritable(&conn, file) {
        if version < latest {
            return Err(Failure::Other(format!(
                "{reason}; it needs migrating from version {version}"
            )));
        }
        return read_only(conn, reason);
    }
    use_wal(&conn);
    migrate(&mut conn, migrations)?;
    Ok(Ready::Writable(conn))
}

/// 쓰는 연결은 WAL 로, 커밋마다 디스크 동기화하지 않고(`synchronous = NORMAL`) 쓴다(03-architecture 「state.db」).
///
/// 이유는 커밋이 쓰기 잠금을 쥐는 시간이다. 쉬지 않고 쓰는 프로세스는 잠금을 놓자마자 다시 잡으므로, busy 폴링으로 자다 깨는
/// 다른 프로세스는 잠금이 비는 짧은 틈을 놓친다 — 커밋이 몇 ms 를 넘으면 2초 안에 끼어들지 못한다(트랜잭션 안에 지연을 넣어 잰
/// 문턱: 2ms 통과, 5ms 가끔 실패, 10ms 이상 늘 실패). Windows CI 에서 커밋은 rollback 저널로 약 70ms, WAL + 기본 `FULL` 로도
/// 약 28ms 였고, 시간의 대부분이 커밋마다의 디스크 동기화다. WAL 의 `NORMAL` 은 체크포인트 때만 동기화한다 — 파일이 깨지지는
/// 않고, 전원이 나가면 마지막 커밋 몇 개를 잃을 수 있다. 레지스트리는 다시 등록하면 되는 데이터라 받아들인다. rollback 저널에서는
/// `NORMAL` 이 드물게 파일을 깨뜨릴 수 있어(SQLite 문서) WAL 로 바뀐 연결에만 쓴다. WAL 은 로컬 파일 시스템을 전제한다(`$DATA` 는
/// 로컬).
///
/// 모드는 파일 머리에 남아 한 번 바꾸면 다음부터는 그대로다 — 이 앱의 옛 빌드가 연 연결도 다음 접근에서 WAL 로 따라온다. 다른
/// 연결이 쓰는 중이라 이번에 못 바꾸면 rollback 저널(`FULL`)로 쓰고 다음 열기에서 다시 해 본다. 열기를 실패시키지 않는다.
fn use_wal(conn: &Connection) {
    let mode =
        conn.pragma_update_and_check(None, "journal_mode", "wal", |row| row.get::<_, String>(0));
    if mode.is_ok_and(|mode| mode.eq_ignore_ascii_case("wal")) {
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    }
}

fn quick_check(conn: &Connection) -> rusqlite::Result<String> {
    conn.query_row("PRAGMA quick_check", [], |row| row.get(0))
}

/// 쓰기 금지 파일 · 폴더 때문에 이 연결로 파일을 읽지 못했는가. WAL 파일은 `-shm` 을 만들 수 없으면 쓰기 연결로 읽히지 않는다.
fn unwritable_error(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(ErrorCode::ReadOnly | ErrorCode::CannotOpen)
    )
}

/// 쓰기 연결로 읽지 못한 파일을 읽기만 하는 연결로 연다. 먼저 `SQLITE_OPEN_READ_ONLY`(앱이 열고 있어 `-wal` · `-shm` 이 있으면
/// 이것으로 읽힌다), 그것도 안 되면 바뀌지 않는 파일(`immutable`)로 연다 — `-shm` 을 만들 수 없는 폴더에서 `-wal` 이 없는 WAL
/// 파일은 SQLite 가 달리 읽지 못한다. 이 사용자는 그 폴더에 `-wal` 을 만들 수 없어 이 사용자의 프로세스가 연 동안 파일을 바꾸지
/// 않는다. 폴더 소유자 같은 다른 사용자가 같은 파일에 쓰면 이 연결은 잠금 없이 읽으므로 낡은 결과나 깨진 읽기(`SQLITE_CORRUPT`)를
/// 받을 수 있다 — 같은 `$DATA` 를 다른 사용자와 나눠 쓰는 경우뿐이고, 깨진 읽기로 손상 판정이 나도 쓰기 금지 폴더라 복구의
/// 파일 옮기기가 실패해 `memory` 로 떨어질 뿐 파일은 그대로다.
fn open_unwritable(file: &Path, busy: Duration) -> Result<(Connection, String), Failure> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let read_only = Connection::open_with_flags(file, flags).and_then(|conn| {
        conn.busy_timeout(busy)?;
        let check = quick_check(&conn)?;
        Ok((conn, check))
    });
    match read_only {
        Err(e) if unwritable_error(&e) => {
            let conn = Connection::open_with_flags(
                immutable_uri(file),
                flags | OpenFlags::SQLITE_OPEN_URI,
            )?;
            let check = quick_check(&conn)?;
            Ok((conn, check))
        }
        other => Ok(other?),
    }
}

/// `file:` URI(`?immutable=1`). 경로의 `%` · `?` · `#` 은 URI 에서 뜻이 있어 퍼센트로 적는다. Windows 의 `C:\a` 는 `file:/C:/a` 다.
fn immutable_uri(file: &Path) -> String {
    let path = file.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file:");
    if !path.starts_with('/') {
        uri.push('/');
    }
    for c in path.chars() {
        match c {
            '%' => uri.push_str("%25"),
            '?' => uri.push_str("%3F"),
            '#' => uri.push_str("%23"),
            c => uri.push(c),
        }
    }
    uri.push_str("?immutable=1");
    uri
}

/// 파일이나 폴더가 이 프로세스에게 쓰기 금지인가. 폴더가 막히면 저널 파일을 못 만들어 첫 쓰기에서야 실패하므로 미리 본다.
/// 폴더는 권한 비트가 아니라 **실제로 파일을 만들어 보고** 판정한다 — 쓰기 비트는 있어도 이 사용자에게는 막힌 폴더(다른 사용자 소유),
/// 읽기 전용으로 마운트된 볼륨, 쓰기를 막지 않는 Windows 폴더 읽기 전용 속성을 모두 같은 방법으로 가린다.
fn unwritable(conn: &Connection, file: &Path) -> Option<String> {
    // 폴더를 먼저 본다 — 쓰기 금지 폴더의 WAL 파일은 읽기만 하는 연결로 다시 열었으므로 그 연결은 언제나 읽기 전용이다.
    unwritable_folder(file).or_else(|| {
        conn.is_readonly(rusqlite::MAIN_DB)
            .unwrap_or(false)
            .then(|| "the file is read-only".to_owned())
    })
}

fn unwritable_folder(file: &Path) -> Option<String> {
    static PROBES: AtomicU64 = AtomicU64::new(0);
    let dir = file.parent()?;
    // 같은 폴더를 여는 다른 프로세스 · 스레드와 이름이 겹치지 않게 pid 와 프로세스 안 순번을 붙인다.
    let probe = dir.join(format!(
        ".{FILE_NAME}.probe-{}-{}",
        std::process::id(),
        PROBES.fetch_add(1, Ordering::Relaxed)
    ));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            None
        }
        Err(e) => Some(format!("{} is not writable: {e}", dir.display())),
    }
}

fn read_only(conn: Connection, reason: String) -> Result<Ready, Failure> {
    conn.pragma_update(None, "query_only", true)?;
    for sql in [SELECT_PROJECTS, SELECT_ROOTS, SELECT_TAGS, SELECT_TRUST] {
        if let Err(e) = conn.prepare(sql) {
            return Err(Failure::Other(format!("{reason}; cannot read it: {e}")));
        }
    }
    Ok(Ready::ReadOnly(conn, reason))
}

fn prepare(conn: &Connection, busy: Duration) -> rusqlite::Result<()> {
    conn.busy_timeout(busy)?;
    conn.pragma_update(None, "foreign_keys", true)
}

/// 올릴 것이 있을 때만 단계마다 쓰기 잠금을 먼저 잡고 그 안에서 버전을 다시 읽는다. 다른 프로세스가 같은 새 파일을 동시에 처음
/// 열어 먼저 올렸으면 그 단계를 건너뛴다(밖에서 읽은 버전으로 돌리면 `table projects already exists` 로 실패한다).
/// 이미 최신이면 잠금 없이 끝낸다 — 최신 파일을 여는 것이 다른 프로세스의 쓰기를 기다리지 않게.
fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<(), Failure> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current as usize >= migrations.len() {
        return Ok(());
    }
    for (from, step) in migrations.iter().enumerate() {
        let from = from as u32;
        let refused = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
            Ok(tx) => {
                let version: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
                if version <= from {
                    step(&tx)?;
                    tx.pragma_update(None, "user_version", from + 1)?;
                    tx.commit()?;
                }
                continue;
            }
            Err(e) => e,
        };
        // 쓰기 잠금을 기다리다 시간을 넘겼다. 같은 새 파일을 동시에 처음 연 다른 연결이 그 사이 이 단계를 올리고 곧바로 쓰기를
        // 이어 가면 쓰기 잠금이 비는 틈은 짧아 기다리는 쪽이 계속 진다(디스크 동기화가 느린 Windows 에서 늘 그랬다). 읽기는 그
        // 틈에도 되므로 버전을 다시 읽어, 이미 올라갔으면 잠금 없이 넘어간다. 다른 연결이 모든 단계를 올렸으면 남은 단계마다
        // 잠금을 다시 기다리지 않고 곧바로 끝낸다.
        if refused.sqlite_error_code() == Some(ErrorCode::DatabaseBusy) {
            #[cfg(test)]
            LOCK_TIMEOUTS.set(LOCK_TIMEOUTS.get() + 1);
            let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if version as usize >= migrations.len() {
                return Ok(());
            }
            if version > from {
                continue;
            }
        }
        return Err(refused.into());
    }
    Ok(())
}

/// 손상된 파일과 그 저널(`-journal` · `-wal` · `-shm`)을 옮기고 새로 만든다. 옮긴 이름은 겹치면 `.1`, `.2` … 를 붙인다.
fn recover(
    file: &Path,
    busy: Duration,
    migrations: &[Migration],
) -> Result<(Connection, PathBuf), String> {
    let backup = free_name(&with_suffix(file, ".corrupt"));
    fs::rename(file, &backup).map_err(|e| format!("{}: {e}", file.display()))?;
    for journal in ["-journal", "-wal", "-shm"] {
        let from = with_suffix(file, journal);
        if from.exists() {
            let _ = fs::rename(&from, with_suffix(&backup, journal));
        }
    }
    match try_open(file, busy, migrations) {
        Ok(Ready::Writable(conn)) => Ok((conn, backup)),
        Ok(Ready::ReadOnly(_, reason)) => Err(reason),
        Err(e) => Err(e.into_reason()),
    }
}

fn memory(path: String, mut reason: String, locked_at: Option<Instant>) -> Opened {
    let mut conn = Connection::open_in_memory().expect("메모리 DB 는 열린다");
    // 빈 메모리 DB 에서도 마이그레이션이 실패하면 단계 자체의 버그다. 그래도 앱은 띄운다 — 표가 없으니 이후 읽기가
    // 오류를 돌려줄 뿐이고, 이유는 상태에 남는다.
    let ready = prepare(&conn, Duration::ZERO)
        .map_err(Failure::from)
        .and_then(|()| migrate(&mut conn, MIGRATIONS));
    if let Err(e) = ready {
        reason.push_str(&format!("; in-memory fallback failed: {}", e.into_reason()));
    }
    Opened {
        conn,
        status: StorageStatus::Memory { path, reason },
        locked_at,
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn free_name(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    (1u32..)
        .map(|n| with_suffix(path, &format!(".{n}")))
        .find(|p| !p.exists())
        .expect("빈 이름이 있다")
}

#[cfg(test)]
mod tests;
