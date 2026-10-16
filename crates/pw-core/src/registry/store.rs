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
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

/// 레지스트리 파일 이름(`$DATA/` 아래).
pub const FILE_NAME: &str = "state.db";
/// 다른 프로세스가 쓰는 중일 때 기다리는 시간. 넘으면 잠김으로 본다.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

type Migration = fn(&Transaction<'_>) -> rusqlite::Result<()>;

/// 버전 `i` → `i + 1`. 길이가 곧 이 앱이 아는 최신 버전이다.
pub const MIGRATIONS: &[Migration] = &[v0_to_v1];

/// 레지스트리가 읽는 질의. 읽기 전용으로 연 새 스키마 파일을 이 질의로 읽을 수 있는지 열 때 미리 본다(`prepare` 가 열 이름을 검사한다).
pub const SELECT_PROJECTS: &str =
    "SELECT id, name, host, favorite, config_mode, slug, created_at, opened_at
     FROM projects WHERE ?1 IS NULL OR id = ?1";
pub const SELECT_ROOTS: &str = "SELECT project_id, path FROM project_roots
     WHERE ?1 IS NULL OR project_id = ?1 ORDER BY project_id, position";
pub const SELECT_TAGS: &str = "SELECT project_id, tag FROM project_tags
     WHERE ?1 IS NULL OR project_id = ?1 ORDER BY project_id, tag";

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
    /// 잠김이 이유면 다음 호출 때 파일을 다시 열어 본다.
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
    /// 다음 호출 때 파일을 다시 열어 볼 것인가(잠김으로 메모리에 떨어진 경우). 메모리 DB 에는 쓰기를 받지 않으므로
    /// 갈아탈 때 잃는 것이 없다.
    pub retry: bool,
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
            retry: false,
        },
        Ok(Ready::ReadOnly(conn, reason)) => Opened {
            conn,
            status: StorageStatus::ReadOnly {
                path: display,
                reason,
            },
            retry: false,
        },
        Err(Failure::Corrupt(reason)) => match recover(file, busy, migrations) {
            Ok((conn, backup)) => Opened {
                conn,
                status: StorageStatus::Recovered {
                    path: display,
                    backup: backup.display().to_string(),
                    reason,
                },
                retry: false,
            },
            Err(e) => memory(display, format!("{reason}; recovery failed: {e}"), false),
        },
        Err(Failure::Busy(reason)) => memory(display, reason, true),
        Err(Failure::Other(reason)) => memory(display, reason, false),
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
    let mut conn = Connection::open_with_flags(file, flags)?;
    prepare(&conn, busy)?;
    // 파일이 SQLite 가 아니면 여기서 처음 드러난다(열기 자체는 게으르다). b-tree 가 깨진 경우는 오류가 아니라 문장으로 온다.
    let check: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
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
    migrate(&mut conn, migrations)?;
    Ok(Ready::Writable(conn))
}

/// 파일이나 폴더가 쓰기 금지인가. 폴더가 막히면 저널 파일을 못 만들어 첫 쓰기에서야 실패하므로 미리 본다.
/// Windows 는 폴더의 읽기 전용 속성이 쓰기를 막지 않고(사용자 지정 아이콘 표시로 쓰인다) 판정에 쓰지 않는다.
fn unwritable(conn: &Connection, file: &Path) -> Option<String> {
    if conn.is_readonly(rusqlite::MAIN_DB).unwrap_or(false) {
        return Some("the file is read-only".into());
    }
    #[cfg(unix)]
    if let Some(dir) = file.parent()
        && fs::metadata(dir).is_ok_and(|m| m.permissions().readonly())
    {
        return Some(format!("{} is read-only", dir.display()));
    }
    #[cfg(not(unix))]
    let _ = file;
    None
}

fn read_only(conn: Connection, reason: String) -> Result<Ready, Failure> {
    conn.pragma_update(None, "query_only", true)?;
    for sql in [SELECT_PROJECTS, SELECT_ROOTS, SELECT_TAGS] {
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

/// 단계마다 쓰기 잠금을 먼저 잡고 그 안에서 버전을 다시 읽는다. 다른 프로세스가 같은 새 파일을 동시에 처음 열어 먼저 올렸으면
/// 그 단계를 건너뛴다(밖에서 읽은 버전으로 돌리면 `table projects already exists` 로 실패한다).
fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<(), Failure> {
    for (from, step) in migrations.iter().enumerate() {
        let from = from as u32;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > from {
            continue;
        }
        step(&tx)?;
        tx.pragma_update(None, "user_version", from + 1)?;
        tx.commit()?;
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

fn memory(path: String, mut reason: String, retry: bool) -> Opened {
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
        retry,
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
