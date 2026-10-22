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
    fn add_color(tx: &Transaction<'_>) -> rusqlite::Result<()> {
        tx.execute_batch("ALTER TABLE projects ADD COLUMN color TEXT")
    }
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    drop(open_with(&file, BUSY_TIMEOUT, &[v0_to_v1]));
    let steps: &[Migration] = &[v0_to_v1, add_color];
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

/// 새 파일을 동시에 처음 연 연결은 모두 WAL + `NORMAL` 로 쓴다. 한 연결이 WAL 로 바꾸는 동안 다른 연결의 바꾸기는 곧바로
/// `database is locked` 로 거절되고, 그 연결은 다른 연결이 바꾼 WAL 로 따라간다. 열기를 마칠 때 모드를 다시 보지 않으면 `FULL` 로
/// 남는다 — 커밋마다 동기화해 Windows 에서 동시 쓰기를 굶긴다(레지스트리의 `concurrent_writers_on_one_file_wait_instead_of_failing`).
/// 다시 보지 않는 코드에서 넷 중 하나 이상이 `FULL` 로 남는 라운드가 절반쯤이라 라운드를 되풀이한다.
#[test]
fn concurrent_first_opens_all_commit_in_wal_without_a_sync_each() {
    const OPENERS: usize = 4;
    for round in 0..30 {
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
                    (journal_mode(&opened.conn), synchronous(&opened.conn))
                })
            })
            .collect();
        let modes: Vec<(String, u32)> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(
            modes,
            vec![("wal".to_owned(), NORMAL); OPENERS],
            "round {round}"
        );
    }
}

/// 최신 파일을 여는 것은 쓰기가 아니다 — 다른 연결이 쓰기 트랜잭션을 쥐고 있어도 기다리지 않고 연다.
#[test]
fn opening_a_current_file_does_not_wait_for_another_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    drop(open(&file, BUSY_TIMEOUT));
    let holder = Connection::open(&file).unwrap();
    holder
        .execute_batch(
            "BEGIN IMMEDIATE; INSERT INTO projects (id, name, host, config_mode, slug, created_at)
             VALUES ('p', 'p', 'local', 'repository', 'p', 0);",
        )
        .unwrap();
    let started = Instant::now();
    let opened = open(&file, BUSY_TIMEOUT);
    let took = started.elapsed();
    holder.execute_batch("ROLLBACK").unwrap();
    assert!(
        matches!(opened.status, StorageStatus::Ok { .. }),
        "{:?}",
        opened.status
    );
    // 쓰기 잠금을 기다렸다면 BUSY_TIMEOUT(2초) 근처다. 느린 CI 를 감안해 넉넉히 잡는다.
    assert!(took < Duration::from_millis(500), "{took:?}");
}

/// 실패한 단계는 통째로 되돌린다 — 버전도 표도 그 단계 전 그대로다.
#[test]
fn failing_step_rolls_back_and_leaves_the_file_alone() {
    fn half_then_fail(tx: &Transaction<'_>) -> rusqlite::Result<()> {
        tx.execute_batch("CREATE TABLE half_done (x INTEGER); SELECT * FROM no_such_table")
    }
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    drop(open_with(&file, BUSY_TIMEOUT, &[v0_to_v1]));
    let opened = open_with(&file, BUSY_TIMEOUT, &[v0_to_v1, half_then_fail]);
    assert!(
        matches!(opened.status, StorageStatus::Memory { .. }),
        "{:?}",
        opened.status
    );
    assert_eq!(
        opened.locked_at, None,
        "잠김이 아니면 다시 열어 보지 않는다"
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

/// 다른 프로세스가 파일을 통째로 잠근 상태. WAL 에서는 쓰기 트랜잭션이 읽기를 막지 않으므로 `locking_mode = EXCLUSIVE` 로 파일
/// 잠금을 쥔다. 잠금은 연결을 닫아야 풀린다.
fn lock_file(file: &Path) -> Connection {
    let holder = Connection::open(file).unwrap();
    holder
        .execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE")
        .unwrap();
    holder
}

#[test]
fn locked_file_falls_back_to_memory_and_asks_for_a_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    drop(open(&file, BUSY_TIMEOUT));
    let holder = lock_file(&file);
    let opened = open(&file, Duration::from_millis(50));
    let StorageStatus::Memory { reason, .. } = &opened.status else {
        panic!("{:?}", opened.status);
    };
    assert!(reason.contains("locked"), "{reason}");
    assert!(opened.locked_at.is_some());
    drop(holder);
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
        matches!(&status, StorageStatus::ReadOnly { reason, .. } if reason.contains("is not writable")),
        "{status:?}"
    );
}

fn journal_mode(conn: &Connection) -> String {
    conn.pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap()
}

/// `PRAGMA synchronous` 의 값.
const NORMAL: u32 = 1;
const FULL: u32 = 2;

fn synchronous(conn: &Connection) -> u32 {
    conn.pragma_query_value(None, "synchronous", |row| row.get(0))
        .unwrap()
}

fn insert(conn: &Connection, id: &str) {
    conn.execute(
        "INSERT INTO projects (id, name, host, config_mode, slug, created_at)
         VALUES (?1, ?1, 'local', 'repository', ?1, 0)",
        [id],
    )
    .unwrap();
}

fn count(conn: &Connection) -> u32 {
    conn.query_row("SELECT count(*) FROM projects", [], |row| row.get(0))
        .unwrap()
}

/// 폴더의 파일 이름들(정렬). 읽기 전용 열기가 `-wal` · `-shm` 을 만들지 않는지 본다.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn writable_opens_use_wal() {
    let tmp = tempfile::tempdir().unwrap();
    let opened = open(&tmp.path().join("state.db"), BUSY_TIMEOUT);
    assert!(matches!(opened.status, StorageStatus::Ok { .. }));
    assert_eq!(journal_mode(&opened.conn), "wal");
    assert_eq!(
        synchronous(&opened.conn),
        NORMAL,
        "WAL 은 커밋마다 동기화하지 않는다"
    );
}

/// 이 앱의 옛 빌드가 만든 rollback 저널 파일은 처음 열 때 WAL 로 바뀐다. 옛 빌드가 같은 파일을 열어 둔 채여도 깨지지 않는다 —
/// 그 연결은 다음 접근에서 WAL 로 따라오고, 읽기 트랜잭션 중이라 이번에 못 바꾸면 rollback 으로 쓰고 다음 열기에서 바꾼다.
#[test]
fn a_rollback_file_switches_to_wal_while_an_older_build_keeps_using_it() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    drop(open(&file, BUSY_TIMEOUT));
    let old = Connection::open(&file).unwrap();
    old.busy_timeout(BUSY_TIMEOUT).unwrap();
    assert_eq!(
        old.pragma_update_and_check(None, "journal_mode", "delete", |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "delete"
    );
    insert(&old, "before");

    // 옛 연결이 읽기 트랜잭션 중: 이번에는 못 바꾸지만 열기는 성공하고 쓸 수 있다.
    old.execute_batch("BEGIN; SELECT count(*) FROM projects;")
        .unwrap();
    let opened = open(&file, Duration::from_millis(50));
    assert!(
        matches!(opened.status, StorageStatus::Ok { .. }),
        "{:?}",
        opened.status
    );
    assert_eq!(journal_mode(&opened.conn), "delete");
    assert_eq!(
        synchronous(&opened.conn),
        FULL,
        "rollback 저널에서는 NORMAL 을 쓰지 않는다"
    );
    old.execute_batch("COMMIT").unwrap();
    insert(&opened.conn, "while-old-read");
    drop(opened);

    // 옛 연결이 열려 있지만 쉬는 중: 바꾼다. 옛 연결은 그대로 읽고 쓴다.
    let opened = open(&file, BUSY_TIMEOUT);
    assert!(matches!(opened.status, StorageStatus::Ok { .. }));
    assert_eq!(journal_mode(&opened.conn), "wal");
    insert(&old, "old-after-switch");
    insert(&opened.conn, "new-after-switch");
    assert_eq!(count(&old), 4);
    assert_eq!(count(&opened.conn), 4);
    assert_eq!(journal_mode(&old), "wal");
}

/// 앱 없이 뜬 `pitwall mcp` 의 읽기 전용 열기(`SQLITE_OPEN_READ_ONLY`)가 WAL 파일을 읽는다:
/// (a) 앱이 열고 있어 `-wal` · `-shm` 이 있을 때 — 체크포인트 전의 쓰기까지 보인다.
/// (b) 앱이 닫혀 `-wal` · `-shm` 이 없을 때 — 파일을 바로 읽고 `-wal` · `-shm` 을 만들지 않는다.
#[test]
fn read_only_opens_read_wal_files_with_and_without_the_app() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");

    let app = open(&file, BUSY_TIMEOUT);
    insert(&app.conn, "a");
    assert!(tmp.path().join("state.db-wal").exists());
    let reader = open_read_only(&file, BUSY_TIMEOUT);
    assert!(
        matches!(reader.status, StorageStatus::ReadOnly { .. }),
        "(a) {:?}",
        reader.status
    );
    assert_eq!(count(&reader.conn), 1, "(a) 체크포인트 전의 쓰기도 보인다");
    insert(&app.conn, "b");
    assert_eq!(count(&reader.conn), 2, "(a) 열린 뒤의 쓰기도 보인다");
    drop(reader);
    drop(app);

    let before = entries(tmp.path());
    assert_eq!(before, ["state.db"], "앱이 닫히면 -wal · -shm 이 걷힌다");
    let reader = open_read_only(&file, BUSY_TIMEOUT);
    assert!(
        matches!(reader.status, StorageStatus::ReadOnly { .. }),
        "(b) {:?}",
        reader.status
    );
    assert_eq!(count(&reader.conn), 2);
    drop(reader);
    assert_eq!(entries(tmp.path()), before, "(b) 아무 파일도 만들지 않는다");
}

/// (c) `$DATA` 폴더가 쓰기 금지일 때 — 앱이 닫혀 있든(파일만) 열고 있든(`-wal` · `-shm` 있음) 읽는다.
/// root 처럼 권한을 무시하는 계정이면 확인할 것이 없다.
#[cfg(unix)]
#[test]
fn read_only_opens_read_wal_files_in_a_read_only_folder() {
    use std::os::unix::fs::PermissionsExt;

    let set_mode = |path: &Path, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    let file = dir.join("state.db");
    let app = open(&file, BUSY_TIMEOUT);
    insert(&app.conn, "a");
    drop(app);

    set_mode(&dir, 0o555);
    if fs::write(dir.join("probe"), "").is_ok() {
        set_mode(&dir, 0o755);
        return;
    }
    let closed = open_read_only(&file, BUSY_TIMEOUT);
    let closed_count =
        matches!(closed.status, StorageStatus::ReadOnly { .. }).then(|| count(&closed.conn));
    let closed_status = closed.status.clone();
    drop(closed);
    // 앱 자신의 열기도 같은 폴더에서 읽기 전용으로 뜬다(메모리로 빠지지 않는다).
    let app_status = open(&file, BUSY_TIMEOUT).status;
    let after = entries(&dir);
    set_mode(&dir, 0o755);
    assert_eq!(
        closed_count,
        Some(1),
        "(c) 앱이 닫혀 있을 때: {closed_status:?}"
    );
    assert!(
        matches!(&app_status, StorageStatus::ReadOnly { reason, .. } if reason.contains("is not writable")),
        "{app_status:?}"
    );
    assert_eq!(after, ["state.db"]);

    let app = open(&file, BUSY_TIMEOUT);
    insert(&app.conn, "b");
    set_mode(&dir, 0o555);
    let running = open_read_only(&file, BUSY_TIMEOUT);
    let running_count =
        matches!(running.status, StorageStatus::ReadOnly { .. }).then(|| count(&running.conn));
    let running_status = running.status.clone();
    drop(running);
    set_mode(&dir, 0o755);
    drop(app);
    assert_eq!(
        running_count,
        Some(2),
        "(c) 앱이 열고 있을 때: {running_status:?}"
    );
}

/// 같은 새 파일을 먼저 연 다른 프로세스가 마이그레이션하고 곧바로 쓰기를 이어 가면, 이 열기는 마이그레이션 단계의 쓰기 잠금을 끝내
/// 못 잡는다. 그 사이 단계가 이미 올라갔으면 실패가 아니다 — 메모리로 빠지지 않고 연다(Windows CI 에서 레지스트리 넷 중 셋이 이렇게
/// 빠졌다). 다른 연결은 열기가 버전 0 을 읽고 쓰기 잠금을 기다리는 동안 커밋하고, 놓자마자 다시 쓰기 잠금을 쥔다 — 폴링으로
/// 기다리는 열기는 그 틈을 놓친다.
#[test]
fn a_migration_finished_by_another_writer_while_waiting_is_not_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    let mut other = Connection::open(&file).unwrap();
    let busy = Duration::from_millis(300);
    let tx = other
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let writer = {
        let file = file.clone();
        std::thread::spawn(move || open(&file, busy))
    };
    // 열기는 버전 0 을 읽고 마이그레이션 단계의 쓰기 잠금을 300ms 까지 기다리는 중이다. 다른 연결은 같은 앱이라 모든 단계를
    // 올린다 — 열기는 단계마다 쓰기 잠금을 못 잡지만 그때마다 버전을 다시 읽어 넘어간다.
    std::thread::sleep(Duration::from_millis(150));
    for step in MIGRATIONS {
        step(&tx).unwrap();
    }
    tx.pragma_update(None, "user_version", supported_version())
        .unwrap();
    tx.commit().unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let opened = writer.join().unwrap();
    other.execute_batch("ROLLBACK").unwrap();
    assert!(
        matches!(opened.status, StorageStatus::Ok { .. }),
        "{:?}",
        opened.status
    );
    assert_eq!(version(&opened.conn), supported_version());
}

/// 다른 연결이 모든 단계를 올리고 쓰기 잠금을 쥐고 있으면 열기는 잠금을 한 번만 기다린다 — 다시 읽은 버전이 최신이면 남은 단계마다
/// 다시 기다리지 않는다(단계가 늘수록 경합 중 열기가 단계 수만큼 [`BUSY_TIMEOUT`] 을 기다리지 않게). 판정은 걸린 시간이 아니라
/// 기다리다 시간을 넘긴 횟수다 — 시간은 느린 러너에서 늘어난다(macOS CI 에서 한 번 기다린 열기가 3.2초 걸렸다). 다른 연결이
/// 커밋하고 다시 잠그는 틈에 열기가 잠금을 잡으면 한 번도 기다리지 않아 이 성질을 보지 못하므로, 그때는 다시 한다.
#[test]
fn an_open_racing_a_finished_migration_waits_for_the_lock_once() {
    assert!(
        MIGRATIONS.len() >= 2,
        "단계가 둘 이상이어야 이 시험이 뜻이 있다"
    );
    for _ in 0..3 {
        let timeouts = race_a_finished_migration();
        assert!(timeouts <= 1, "waited for the lock {timeouts} times");
        if timeouts == 1 {
            return;
        }
    }
    panic!("the open never had to wait for the lock");
}

/// 열기가 버전 0 을 읽고 쓰기 잠금을 기다리는 동안 다른 연결이 모든 단계를 올리고, 놓자마자 다시 쓰기 잠금을 쥔다. 열기가 쓰기
/// 잠금을 기다리다 시간을 넘긴 횟수를 준다.
fn race_a_finished_migration() -> u32 {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("state.db");
    let mut other = Connection::open(&file).unwrap();
    let busy = Duration::from_secs(2);
    let tx = other
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let opener = {
        let file = file.clone();
        std::thread::spawn(move || (open(&file, busy), LOCK_TIMEOUTS.get()))
    };
    std::thread::sleep(Duration::from_millis(300));
    for step in MIGRATIONS {
        step(&tx).unwrap();
    }
    tx.pragma_update(None, "user_version", supported_version())
        .unwrap();
    tx.commit().unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (opened, timeouts) = opener.join().unwrap();
    other.execute_batch("ROLLBACK").unwrap();
    assert!(
        matches!(opened.status, StorageStatus::Ok { .. }),
        "{:?}",
        opened.status
    );
    timeouts
}

/// 비정상 종료로 `-wal` · `-shm` 이 남은 파일이 손상이면: 검사 연결이 닫히며 남은 WAL 프레임을 손상된 파일에 옮기고 사이드카를
/// [`corrupt_crash_image`] 에서 WAL 프레임에만 있는 행의 id.
const FRAME_ONLY_ID: &str = "row-only-in-the-wal-frames";

/// 앱이 연 채로 비정상 종료한 상태의 손상 파일(`state.db` · `-wal` · `-shm`)을 `crashed/` 에 만든다. 메인 파일의 1페이지 뒤
/// (WAL 에 없는 색인 페이지들)가 깨져 있다.
fn corrupt_crash_image(tmp: &Path) -> PathBuf {
    let live = tmp.join("live");
    let crashed = tmp.join("crashed");
    fs::create_dir_all(&crashed).unwrap();
    let app = open(&live.join("state.db"), BUSY_TIMEOUT);
    insert(&app.conn, "a");
    // 표 · 색인 페이지를 메인 파일로 옮겨 두고 한 번 더 쓴다 — WAL 에는 바뀐 페이지만 남는다.
    app.conn
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    insert(&app.conn, FRAME_ONLY_ID);
    // 앱이 연 채로 떠 둔 것 = 비정상 종료 이미지. 열린 파일은 공유 모드가 맞는 `fs::read` 로 읽는다(Windows).
    for name in ["state.db", "state.db-wal", "state.db-shm"] {
        fs::write(crashed.join(name), fs::read(live.join(name)).unwrap()).unwrap();
    }
    drop(app);
    let file = crashed.join("state.db");
    // 1페이지 뒤(WAL 에 없는 색인 페이지들)를 깨뜨린다.
    let mut bytes = fs::read(&file).unwrap();
    assert!(
        bytes.len() > 4096,
        "체크포인트로 페이지가 메인 파일에 있어야 한다"
    );
    bytes[4096..].fill(0x5a);
    fs::write(&file, bytes).unwrap();

    file
}

/// 지운 뒤 파일을 옮긴다 — 프레임은 `.corrupt` 안에 남고, 옮긴 자리에 사이드카가 따로 남지 않는다.
#[test]
fn a_corrupt_file_left_with_a_wal_is_moved_aside_with_its_frames() {
    let tmp = tempfile::tempdir().unwrap();
    let file = corrupt_crash_image(tmp.path());
    let crashed = file.parent().unwrap().to_path_buf();
    let opened = open(&file, BUSY_TIMEOUT);
    let StorageStatus::Recovered { backup, .. } = &opened.status else {
        panic!("{:?}", opened.status);
    };
    assert_eq!(Path::new(backup), crashed.join("state.db.corrupt"));
    assert_eq!(count(&opened.conn), 0);
    drop(opened);
    assert_eq!(entries(&crashed), ["state.db", "state.db.corrupt"]);
    let moved = fs::read(crashed.join("state.db.corrupt")).unwrap();
    assert!(
        moved
            .windows(FRAME_ONLY_ID.len())
            .any(|w| w == FRAME_ONLY_ID.as_bytes()),
        "WAL 에만 있던 쓰기가 옮긴 파일 안에 있다"
    );
}

/// 다른 프로세스가 손상 파일을 열고 있어 `-wal` · `-shm` 이 남아 있으면 파일과 함께 옮긴다 — 새 파일 옆에 옛 WAL 이 남아 새
/// 파일에 섞이지 않게. Windows 는 SQLite 가 공유 삭제 없이 연 파일을 옮길 수 없어(복구가 실패해 `memory` 로) 이 경우가 없다.
#[cfg(unix)]
#[test]
fn a_corrupt_file_another_process_holds_is_moved_with_its_sidecars() {
    let tmp = tempfile::tempdir().unwrap();
    let file = corrupt_crash_image(tmp.path());
    let crashed = file.parent().unwrap().to_path_buf();
    let other = Connection::open(&file).unwrap();
    let _: u32 = other
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();

    let opened = open(&file, BUSY_TIMEOUT);
    assert!(
        matches!(opened.status, StorageStatus::Recovered { .. }),
        "{:?}",
        opened.status
    );
    let names = entries(&crashed);
    assert!(
        names.contains(&"state.db.corrupt-wal".to_owned()),
        "{names:?}"
    );
    assert!(
        names.contains(&"state.db.corrupt-shm".to_owned()),
        "{names:?}"
    );
    assert_eq!(count(&opened.conn), 0, "새 파일은 옛 WAL 을 읽지 않는다");
    drop(other);
}
