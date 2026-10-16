use std::sync::{Arc, Barrier};

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

/// 여러 프로세스가 새 파일을 동시에 처음 열어도 아무도 메모리 DB 로 빠지지 않는다. 연결이 따로라 프로세스와 같은 잠금 경로를 탄다.
#[test]
fn concurrent_first_opens_all_succeed() {
    const OPENERS: usize = 4;
    for round in 0..10 {
        let tmp = tempfile::tempdir().unwrap();
        let file = Arc::new(tmp.path().join("state.db"));
        let barrier = Arc::new(Barrier::new(OPENERS));
        let threads: Vec<_> = (0..OPENERS)
            .map(|_| {
                let file = Arc::clone(&file);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let opened = open(&file, BUSY_TIMEOUT);
                    (opened.status, version(&opened.conn))
                })
            })
            .collect();
        for thread in threads {
            let (status, v) = thread.join().unwrap();
            assert!(
                matches!(status, StorageStatus::Ok { .. }),
                "round {round}: {status:?}"
            );
            assert_eq!(v, supported_version());
        }
    }
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
    assert!(!opened.retry, "잠김이 아니면 다시 열어 보지 않는다");
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

/// SQLite 가 오류 코드 없이 `quick_check` 의 문장으로만 알리는 손상(b-tree 안 rowid 순서가 뒤바뀜)도 손상으로 본다.
#[test]
fn integrity_check_failure_is_treated_as_corruption() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    let (root, page_size): (u32, usize) = {
        let conn = open(&file, BUSY_TIMEOUT).conn;
        conn.execute_batch(
            "CREATE TABLE t(x); INSERT INTO t(rowid, x) VALUES (1, 'a'), (2, 'b'), (3, 'c');",
        )
        .unwrap();
        let root = conn
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = 't'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let page_size: u32 = conn
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .unwrap();
        (root, page_size as usize)
    };
    let mut bytes = fs::read(&file).unwrap();
    let page = (root as usize - 1) * page_size;
    assert_eq!(bytes[page], 0x0d, "테이블 잎 페이지");
    let cell = |bytes: &[u8], i: usize| {
        let at = page + 8 + 2 * i;
        page + usize::from(u16::from_be_bytes([bytes[at], bytes[at + 1]]))
    };
    // 셀 = 페이로드 길이(1바이트 varint) · rowid(1바이트 varint) · 페이로드. 첫 셀과 끝 셀의 rowid 를 맞바꾼다.
    let (first, last) = (cell(&bytes, 0) + 1, cell(&bytes, 2) + 1);
    bytes.swap(first, last);
    fs::write(&file, &bytes).unwrap();

    let opened = open(&file, BUSY_TIMEOUT);
    let StorageStatus::Recovered { reason, .. } = &opened.status else {
        panic!("{:?}", opened.status);
    };
    assert!(reason.contains("out of order"), "{reason}");
}

#[test]
fn newer_schema_opens_read_only_without_touching_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    {
        let conn = open(&file, BUSY_TIMEOUT).conn;
        conn.execute(
            "INSERT INTO projects (id, name, host, config_mode, slug, created_at)
             VALUES ('p', 'kept', 'local', 'repository', 'kept', 0)",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", supported_version() + 5)
            .unwrap();
    }
    let before = fs::read(&file).unwrap();
    let opened = open(&file, BUSY_TIMEOUT);
    let StorageStatus::ReadOnly { reason, .. } = &opened.status else {
        panic!("{:?}", opened.status);
    };
    assert!(reason.contains("newer"), "{reason}");
    let name: String = opened
        .conn
        .query_row("SELECT name FROM projects", [], |row| row.get(0))
        .unwrap();
    assert_eq!(name, "kept", "읽기는 된다");
    assert!(opened.conn.execute("DELETE FROM projects", []).is_err());
    drop(opened);
    assert_eq!(fs::read(&file).unwrap(), before);
}

/// 새 스키마가 이 앱의 질의로 읽히지 않으면(열을 바꾼 경우 등) 그때 메모리로 간다.
#[test]
fn unreadable_newer_schema_falls_back_to_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    {
        let conn = Connection::open(&file).unwrap();
        conn.execute_batch("CREATE TABLE projects (other TEXT)")
            .unwrap();
        conn.pragma_update(None, "user_version", supported_version() + 1)
            .unwrap();
    }
    let opened = open(&file, BUSY_TIMEOUT);
    assert!(
        matches!(opened.status, StorageStatus::Memory { .. }),
        "{:?}",
        opened.status
    );
}

#[test]
fn locked_file_falls_back_to_memory_and_asks_for_a_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    drop(open(&file, BUSY_TIMEOUT));
    let holder = Connection::open(&file).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let opened = open(&file, Duration::from_millis(50));
    let StorageStatus::Memory { reason, .. } = &opened.status else {
        panic!("{:?}", opened.status);
    };
    assert!(reason.contains("locked"), "{reason}");
    assert!(opened.retry);
    holder.execute_batch("ROLLBACK").unwrap();
    // 잠김은 손상이 아니다 — 파일을 옮기지 않았다.
    assert!(!tmp.path().join("state.db.corrupt").exists());
    assert!(matches!(
        open(&file, BUSY_TIMEOUT).status,
        StorageStatus::Ok { .. }
    ));
}

/// 쓰기 금지 파일 · 폴더는 읽기 전용으로 알린다(`ok` 로 떠서 첫 쓰기에서야 실패하지 않게).
/// root 처럼 권한을 무시하는 계정이면 확인할 것이 없다.
#[cfg(unix)]
#[test]
fn read_only_file_or_folder_opens_read_only() {
    use std::os::unix::fs::PermissionsExt;

    let set_mode = |path: &Path, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    let file = dir.join("state.db");
    drop(open(&file, BUSY_TIMEOUT));

    set_mode(&file, 0o444);
    if fs::OpenOptions::new().write(true).open(&file).is_ok() {
        set_mode(&file, 0o644);
        return;
    }
    let status = open(&file, BUSY_TIMEOUT).status;
    set_mode(&file, 0o644);
    assert!(
        matches!(&status, StorageStatus::ReadOnly { reason, .. } if reason.contains("file is read-only")),
        "{status:?}"
    );

    set_mode(&dir, 0o555);
    let status = open(&file, BUSY_TIMEOUT).status;
    set_mode(&dir, 0o755);
    assert!(
        matches!(&status, StorageStatus::ReadOnly { reason, .. } if reason.contains("data is read-only")),
        "{status:?}"
    );
}
