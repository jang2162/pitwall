//! `$DATA/state.db` 의 연결 · 스키마 버전 · 마이그레이션 · 열기 실패 처리.
//!
//! 스키마 버전은 SQLite 의 `PRAGMA user_version` 이다. [`MIGRATIONS`] 의 `i` 번째 단계가 버전 `i` 를 `i + 1` 로 올리고,
//! 단계마다 트랜잭션 하나라 실패하면 그 단계 전으로 돌아간다. 단계는 지우거나 고치지 않고 뒤에 더하기만 한다.
//!
//! 열기에 실패해도 앱은 뜬다([`StorageStatus`]).
//! - 손상(SQLite 파일이 아님 · 무결성 검사 실패): 파일을 `state.db.corrupt` 로 옮겨 두고 새로 만든다.
//! - 잠김 · 권한 · 앱보다 새 스키마 · 그 밖: 파일을 건드리지 않고 메모리 DB 로 뜬다. 그 세션의 변경은 남지 않는다.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction};
use serde::{Deserialize, Serialize};

/// 레지스트리 파일 이름(`$DATA/` 아래).
pub const FILE_NAME: &str = "state.db";
/// 다른 프로세스가 쓰는 중일 때 기다리는 시간. 넘으면 잠김으로 본다.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

type Migration = fn(&Transaction<'_>) -> rusqlite::Result<()>;

/// 버전 `i` → `i + 1`. 길이가 곧 이 앱이 아는 최신 버전이다.
pub const MIGRATIONS: &[Migration] = &[v0_to_v1];

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

/// 레지스트리 저장소의 상태. 앱은 `memory` 일 때 「프로젝트 목록을 저장하지 못한다」를 알린다.
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
    /// 손상된 파일을 `backup` 으로 옮기고 빈 레지스트리로 새로 만들었다.
    Recovered {
        path: String,
        backup: String,
        reason: String,
    },
    /// 파일을 쓸 수 없어 메모리 DB 로 떴다. 이번 세션의 변경은 저장되지 않는다. 파일은 건드리지 않았다.
    Memory { path: String, reason: String },
}

pub struct Opened {
    pub conn: Connection,
    pub status: StorageStatus,
}

/// `file` 을 열고 최신 스키마로 올린다. 실패해도 쓸 수 있는 연결을 돌려준다.
pub fn open(file: &Path, busy: Duration) -> Opened {
    open_with(file, busy, MIGRATIONS)
}

pub fn open_with(file: &Path, busy: Duration, migrations: &[Migration]) -> Opened {
    let display = file.display().to_string();
    match try_open(file, busy, migrations) {
        Ok(conn) => Opened {
            conn,
            status: StorageStatus::Ok { path: display },
        },
        Err(Failure::Corrupt(reason)) => match recover(file, busy, migrations) {
            Ok((conn, backup)) => Opened {
                conn,
                status: StorageStatus::Recovered {
                    path: display,
                    backup: backup.display().to_string(),
                    reason,
                },
            },
            Err(e) => memory(
                display,
                format!("{reason}; recovery failed: {e}"),
                migrations,
            ),
        },
        Err(Failure::Other(reason)) => memory(display, reason, migrations),
    }
}

enum Failure {
    Corrupt(String),
    Other(String),
}

impl From<rusqlite::Error> for Failure {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => {
                Failure::Corrupt(e.to_string())
            }
            _ => Failure::Other(e.to_string()),
        }
    }
}

fn try_open(file: &Path, busy: Duration, migrations: &[Migration]) -> Result<Connection, Failure> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Failure::Other(format!("{}: {e}", dir.display())))?;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut conn = Connection::open_with_flags(file, flags)?;
    prepare(&conn, busy)?;
    // 파일이 SQLite 가 아니면 여기서 처음 드러난다(열기 자체는 게으르다).
    let check: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(Failure::Corrupt(format!("integrity check: {check}")));
    }
    migrate(&mut conn, migrations)?;
    Ok(conn)
}

fn prepare(conn: &Connection, busy: Duration) -> rusqlite::Result<()> {
    conn.busy_timeout(busy)?;
    conn.pragma_update(None, "foreign_keys", true)
}

fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<(), Failure> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let latest = migrations.len() as u32;
    if version > latest {
        // 새 앱이 만든 파일이다. 읽어도 열 의미가 다를 수 있고, 쓰면 새 앱이 그 파일을 못 믿게 된다.
        return Err(Failure::Other(format!(
            "schema version {version} is newer than this app supports ({latest})"
        )));
    }
    for (from, step) in migrations.iter().enumerate().skip(version as usize) {
        let tx = conn.transaction()?;
        step(&tx)?;
        tx.pragma_update(None, "user_version", from as u32 + 1)?;
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
        Ok(conn) => Ok((conn, backup)),
        Err(Failure::Corrupt(e) | Failure::Other(e)) => Err(e),
    }
}

fn memory(path: String, mut reason: String, migrations: &[Migration]) -> Opened {
    let mut conn = Connection::open_in_memory().expect("메모리 DB 는 열린다");
    // 빈 메모리 DB 에서도 마이그레이션이 실패하면 단계 자체의 버그다. 그래도 앱은 띄운다 — 표가 없으니 이후 호출이
    // 오류를 돌려줄 뿐이고, 이유는 상태에 남는다.
    let prepared = prepare(&conn, Duration::ZERO).map_err(Failure::from);
    if let Err(Failure::Corrupt(e) | Failure::Other(e)) =
        prepared.and_then(|()| migrate(&mut conn, migrations))
    {
        reason.push_str(&format!("; in-memory fallback failed: {e}"));
    }
    Opened {
        conn,
        status: StorageStatus::Memory { path, reason },
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
mod tests {
    use super::*;

    fn version(conn: &Connection) -> u32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn new_file_is_created_at_the_latest_version() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("data/state.db");
        let opened = open(&file, BUSY_TIMEOUT);
        assert!(matches!(opened.status, StorageStatus::Ok { .. }));
        assert_eq!(version(&opened.conn), supported_version());
        assert!(file.exists());
    }

    /// 단계는 순서대로 한 번씩 돌고, 이미 지난 단계는 다시 돌지 않는다.
    #[test]
    fn migrations_run_in_order_and_only_once() {
        fn v1_to_v2(tx: &Transaction<'_>) -> rusqlite::Result<()> {
            tx.execute_batch("ALTER TABLE projects ADD COLUMN color TEXT")
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("state.db");
        drop(open(&file, BUSY_TIMEOUT));
        let steps: &[Migration] = &[v0_to_v1, v1_to_v2];
        let opened = open_with(&file, BUSY_TIMEOUT, steps);
        assert!(matches!(opened.status, StorageStatus::Ok { .. }));
        assert_eq!(version(&opened.conn), 2);
        opened
            .conn
            .execute("UPDATE projects SET color = 'red'", [])
            .unwrap();
        drop(opened);
        // 다시 열어도 v1_to_v2 가 또 돌지 않는다(돌면 같은 열을 두 번 더해 실패한다).
        let again = open_with(&file, BUSY_TIMEOUT, steps);
        assert!(
            matches!(again.status, StorageStatus::Ok { .. }),
            "{:?}",
            again.status
        );
    }

    /// 실패한 단계는 통째로 되돌린다 — 버전도 표도 그 단계 전 그대로다.
    #[test]
    fn failing_step_rolls_back_and_leaves_the_file_alone() {
        fn half_then_fail(tx: &Transaction<'_>) -> rusqlite::Result<()> {
            tx.execute_batch("CREATE TABLE half_done (x INTEGER); SELECT * FROM no_such_table")
        }
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("state.db");
        drop(open(&file, BUSY_TIMEOUT));
        let opened = open_with(&file, BUSY_TIMEOUT, &[v0_to_v1, half_then_fail]);
        assert!(
            matches!(opened.status, StorageStatus::Memory { .. }),
            "{:?}",
            opened.status
        );
        let conn = Connection::open(&file).unwrap();
        assert_eq!(version(&conn), 1);
        let leftover: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'half_done'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leftover, 0);
    }

    #[test]
    fn garbage_file_is_moved_aside_and_recreated() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("state.db");
        let garbage = b"this is not a sqlite database, just some bytes that fill a page".repeat(80);
        fs::write(&file, &garbage).unwrap();
        fs::write(tmp.path().join("state.db.corrupt"), b"older backup").unwrap();
        let opened = open(&file, BUSY_TIMEOUT);
        let StorageStatus::Recovered { backup, .. } = &opened.status else {
            panic!("{:?}", opened.status);
        };
        // 이미 있던 보관 파일은 덮지 않는다.
        assert_eq!(Path::new(backup), tmp.path().join("state.db.corrupt.1"));
        assert_eq!(fs::read(backup).unwrap(), garbage);
        assert_eq!(version(&opened.conn), supported_version());
    }

    #[test]
    fn newer_schema_opens_in_memory_without_touching_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("state.db");
        {
            let conn = Connection::open(&file).unwrap();
            conn.pragma_update(None, "user_version", supported_version() + 5)
                .unwrap();
        }
        let before = fs::read(&file).unwrap();
        let opened = open(&file, BUSY_TIMEOUT);
        assert!(
            matches!(opened.status, StorageStatus::Memory { .. }),
            "{:?}",
            opened.status
        );
        // 메모리 DB 는 쓸 수 있다.
        assert_eq!(version(&opened.conn), supported_version());
        drop(opened);
        assert_eq!(fs::read(&file).unwrap(), before);
    }

    #[test]
    fn locked_file_falls_back_to_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("state.db");
        let holder = Connection::open(&file).unwrap();
        holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let opened = open(&file, Duration::from_millis(50));
        let StorageStatus::Memory { reason, .. } = &opened.status else {
            panic!("{:?}", opened.status);
        };
        assert!(reason.contains("locked"), "{reason}");
        holder.execute_batch("ROLLBACK").unwrap();
        // 잠김은 손상이 아니다 — 파일을 옮기지 않았다.
        assert!(!tmp.path().join("state.db.corrupt").exists());
    }
}
