//! 드라이버 시험(14-m3-plan.md M3-08 완료 기준 ① ~ ⑧ 과 그 밖의 동작). 실 DB 표지 없이 세 OS 에서 돈다.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pw_db_driver::{Budget, DbConnection, DbDriver, QueryCancel, SqlConnection};
use pw_db_types::{
    BeginOptions, CellLimit, ColumnInfo, ColumnMeta, ConnectionSpec, DbError, DbErrorKind, DbValue,
    DriverKind, ExecuteRequest, FetchLimit, ForeignKeyInfo, IndexInfo, ObjectKind, ObjectQuery,
    ObjectRef, RiskGrade, Row, StatementStarted, Target, TlsStatus, TransactionState,
};
use rusqlite::Connection;

use crate::SqliteDriver;
use crate::fixture::SqliteFixture;

/// 끝나지 않는 조회. 테이블을 읽지 않아 잠금을 쥐지 않는다.
const ENDLESS: &str =
    "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c";

/// 파일의 폴더를 허용 루트로 둔 설정.
fn spec(path: &Path) -> ConnectionSpec {
    let mut spec = ConnectionSpec::new(
        DriverKind::Sqlite.id(),
        Target::File {
            path: path.to_path_buf(),
        },
    );
    spec.allowed_roots = vec![real_parent(path)];
    spec
}

fn real_parent(path: &Path) -> PathBuf {
    path.parent().unwrap().canonicalize().unwrap()
}

async fn connect(spec: ConnectionSpec) -> Box<dyn SqlConnection> {
    match SqliteDriver.connect(spec).await.unwrap().connection {
        DbConnection::Sql(conn) => conn,
        DbConnection::Kv(_) => unreachable!("sqlite is an sql driver"),
    }
}

fn read(sql: &str) -> ExecuteRequest {
    ExecuteRequest::new(sql, RiskGrade::Read)
}

fn write(sql: &str) -> ExecuteRequest {
    ExecuteRequest::new(sql, RiskGrade::Write)
}

/// 문장 하나를 끝까지 돌려 행을 모은다.
async fn rows_of(
    conn: &mut dyn SqlConnection,
    request: ExecuteRequest,
) -> Result<Vec<Row>, DbError> {
    let started = conn.execute(request).await?;
    let mut rows = Vec::new();
    if started.end.is_some() {
        return Ok(rows);
    }
    loop {
        let batch = conn.fetch(FetchLimit::rows(1000)).await?;
        rows.extend(batch.rows);
        if batch.end.is_some() {
            return Ok(rows);
        }
    }
}

async fn query(conn: &mut dyn SqlConnection, sql: &str) -> Vec<Row> {
    rows_of(conn, read(sql)).await.unwrap()
}

async fn count(conn: &mut dyn SqlConnection, sql: &str) -> i64 {
    match query(conn, sql).await.as_slice() {
        [row] => match row.as_slice() {
            [DbValue::Int(pw_db_types::DbInt::Number(n))] => *n,
            other => panic!("not a count: {other:?}"),
        },
        other => panic!("not one row: {other:?}"),
    }
}

/// SQL 글자 상수(작은따옴표는 둘로).
fn literal(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

/// 완료 기준 ①: 끝나지 않는 재귀 CTE 를 취소하면 1 s 안에 `interrupted` 이고, tokio 워커 둘인 런타임에서 워커보다 많은 실행이
/// 돌아도 다른 연결의 질의가 막히지 않는다(연결마다 전용 스레드).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_an_endless_recursive_cte_is_interrupted_within_1s_without_blocking_others() {
    let fixture = SqliteFixture::file();
    let spec = spec(fixture.path().unwrap());
    let mut runs = Vec::new();
    for _ in 0..3 {
        let mut conn = connect(spec.clone()).await;
        let cancel = QueryCancel::new(conn.interrupter(), Budget::CONSOLE.cancel_cleanup);
        let run = cancel.clone();
        let task = tokio::spawn(async move {
            run.run(conn.execute(read(ENDLESS))).await.unwrap();
            let fetched = run.run(conn.fetch(FetchLimit::FIRST_SCREEN)).await;
            (fetched, conn)
        });
        runs.push((cancel, task));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut other = connect(spec.clone()).await;
    let answered = tokio::time::timeout(
        Duration::from_secs(1),
        count(&mut *other, "SELECT count(*) FROM customers"),
    )
    .await
    .expect("another connection answers while three queries spin");
    assert_eq!(answered, 3);

    for (cancel, task) in runs {
        assert!(!task.is_finished(), "the query was still running");
        let cancelled_at = Instant::now();
        cancel.cancel().await.unwrap();
        let (fetched, mut conn) = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the query ends within 1 s of the cancel")
            .unwrap();
        assert!(cancelled_at.elapsed() < Duration::from_secs(1));
        let error = fetched.unwrap_err();
        assert_eq!(error.kind, DbErrorKind::Cancelled, "{error:?}");
        assert_eq!(error.message, "interrupted");
        assert_eq!(
            count(&mut *conn, "SELECT count(*) FROM orders").await,
            3,
            "the connection still works"
        );
    }
}

/// 완료 기준 ②: 수동 커밋 세션에서 `INSERT` → 상한 + 1 행 조회를 상한에서 끊음 → `COMMIT` 뒤 다른 연결에서 그 행이 보인다.
#[tokio::test]
async fn a_manual_commit_insert_survives_a_cut_off_query_and_is_seen_after_commit() {
    let fixture = SqliteFixture::file();
    let spec = spec(fixture.path().unwrap());
    let mut conn = connect(spec.clone()).await;
    let mut other = connect(spec).await;

    conn.begin(BeginOptions::default()).await.unwrap();
    let inserted = conn
        .execute(write("INSERT INTO audit_log VALUES ('now', 'kept')"))
        .await
        .unwrap();
    assert_eq!(inserted.end.unwrap().rows_affected, Some(1));

    let limit = 100;
    conn.execute(read(&format!(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {}) SELECT i, message FROM n, audit_log WHERE message = 'kept'",
        limit + 1
    )))
    .await
    .unwrap();
    let batch = conn.fetch(FetchLimit::rows(limit)).await.unwrap();
    assert_eq!(batch.rows.len(), limit as usize);
    assert!(batch.end.is_none(), "one row is left");
    conn.discard_cursor().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);

    let kept = "SELECT count(*) FROM audit_log WHERE message = 'kept'";
    assert_eq!(count(&mut *other, kept).await, 0, "not committed yet");
    conn.commit().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert_eq!(count(&mut *other, kept).await, 1);
}

/// 완료 기준 ③: read-only 연결에서 `INSERT` · `CREATE TABLE` 이 거절되고 파일의 mtime · 내용이 그대로다. `query_only` 를 풀 수도 없다.
#[tokio::test]
async fn a_read_only_connection_refuses_writes_and_leaves_the_file_untouched() {
    let fixture = SqliteFixture::file();
    let path = fixture.path().unwrap();
    let modified = std::fs::metadata(path).unwrap().modified().unwrap();
    let bytes = std::fs::read(path).unwrap();
    let mut read_only = spec(path);
    read_only.read_only = true;
    let mut conn = connect(read_only).await;

    for sql in [
        "INSERT INTO audit_log VALUES ('x', 'y')",
        "CREATE TABLE t (x)",
    ] {
        let error = conn.execute(write(sql)).await.unwrap_err();
        assert_eq!(error.kind, DbErrorKind::ReadOnly, "{sql}: {error:?}");
    }
    let unlock = conn
        .execute(write("PRAGMA query_only = OFF"))
        .await
        .unwrap_err();
    assert!(
        unlock.message.contains("query_only cannot be changed"),
        "{unlock:?}"
    );
    assert_eq!(count(&mut *conn, "SELECT count(*) FROM customers").await, 3);
    conn.close().await.unwrap();

    assert_eq!(
        std::fs::metadata(path).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

/// 완료 기준 ④: `load_extension` 은 SQL 함수(authorizer)로도 C API(연결 설정)로도 꺼져 있다.
#[tokio::test]
async fn load_extension_is_turned_off() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    let error = rows_of(&mut *conn, read("SELECT load_extension('x')"))
        .await
        .unwrap_err();
    assert_eq!(
        error.message, "loading SQLite extensions is turned off",
        "{error:?}"
    );

    // 연결 설정 쪽: authorizer 를 떼어도 C API 는 꺼져 있다.
    let (raw, _guard) = crate::guard::open(&spec(fixture.path().unwrap())).unwrap();
    let mut enabled: std::ffi::c_int = -1;
    // SAFETY: 열린 연결의 포인터, -1 은 「바꾸지 않고 읽기만」 이다.
    let rc = unsafe {
        rusqlite::ffi::sqlite3_db_config(
            raw.handle(),
            rusqlite::ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION,
            -1 as std::ffi::c_int,
            &raw mut enabled,
        )
    };
    assert_eq!((rc, enabled), (rusqlite::ffi::SQLITE_OK, 0));
}

const LOCK_HOLDER: &str = "PW_SQLITE_TEST_HOLD_WRITE_LOCK";
const LOCK_HELD: &str = "pw-sqlite-test: write lock held";

/// 완료 기준 ⑤: 다른 프로세스가 쓰기 잠금을 쥔 파일에 쓰면 `busy_timeout`(5 s) 뒤 `busy` 다. 다른 프로세스는 이 시험 바이너리를
/// 이 시험 하나로 다시 띄운 것이다(환경 변수 [`LOCK_HOLDER`] 가 있으면 잠금을 쥐고 표준 입력이 닫힐 때까지 기다린다).
#[tokio::test]
async fn another_process_holding_the_write_lock_ends_in_busy_after_5s() {
    if let Ok(path) = std::env::var(LOCK_HOLDER) {
        hold_write_lock(Path::new(&path));
        return;
    }
    let fixture = SqliteFixture::file();
    let path = fixture.path().unwrap();
    let test_name = format!(
        "{}::another_process_holding_the_write_lock_ends_in_busy_after_5s",
        module_path!().split_once("::").unwrap().1
    );
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test_name, "--nocapture", "--test-threads", "1"])
        .env(LOCK_HOLDER, path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(holder.stdout.take().unwrap()).lines();
    assert!(
        // libtest 는 표지를 `test … ` 줄 뒤에 이어 찍는다.
        lines.any(|line| line.unwrap().contains(LOCK_HELD)),
        "the other process took the write lock"
    );

    let mut conn = connect(spec(path)).await;
    assert_eq!(
        count(&mut *conn, "SELECT count(*) FROM customers").await,
        3,
        "reads still go"
    );
    let started = Instant::now();
    let error = conn
        .execute(write("INSERT INTO audit_log VALUES ('x', 'y')"))
        .await
        .unwrap_err();
    let waited = started.elapsed();
    assert_eq!(error.kind, DbErrorKind::Busy, "{error:?}");
    assert!(
        (Duration::from_millis(4500)..Duration::from_secs(10)).contains(&waited),
        "busy after {waited:?}"
    );

    drop(holder.stdin.take());
    assert!(holder.wait().unwrap().success());
}

fn hold_write_lock(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "BEGIN IMMEDIATE; INSERT INTO audit_log VALUES ('held', 'by another process');",
    )
    .unwrap();
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{LOCK_HELD}").unwrap();
    stdout.flush().unwrap();
    // 부모가 표준 입력을 닫으면(시험이 끝나거나 죽으면) 잠금을 놓고 끝난다. 부모가 멈춰도 60 s 뒤에는 끝난다.
    let (closed, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read_to_end(&mut Vec::new());
        let _ = closed.send(());
    });
    let _ = wait.recv_timeout(Duration::from_secs(60));
}

/// 완료 기준 ⑥: 앞 4 KiB 를 덮은 파일은 여는 데서 오류이고, 드라이버는 그 뒤에도 다른 연결을 연다.
#[tokio::test]
async fn a_corrupt_file_fails_to_open_and_the_driver_keeps_answering() {
    let broken = SqliteFixture::file();
    let path = broken.path().unwrap();
    let mut bytes = std::fs::read(path).unwrap();
    assert!(bytes.len() > 4096);
    bytes[..4096].fill(0x5a);
    std::fs::write(path, bytes).unwrap();

    let error = SqliteDriver.connect(spec(path)).await.err().unwrap();
    assert_eq!(error.kind, DbErrorKind::Server, "{error:?}");
    assert_eq!(error.message, "file is not a database");
    assert_eq!(error.code.as_deref(), Some("26"));

    let healthy = SqliteFixture::file();
    let mut conn = connect(spec(healthy.path().unwrap())).await;
    assert_eq!(count(&mut *conn, "SELECT count(*) FROM orders").await, 3);
}

/// 완료 기준 ⑦: 루트 밖 `ATTACH` · `VACUUM INTO` 는 거절되고(파일도 생기지 않는다) 루트 안은 된다.
#[tokio::test]
async fn attach_and_vacuum_into_outside_the_roots_are_refused() {
    let fixture = SqliteFixture::file();
    let root = real_parent(fixture.path().unwrap());
    let outside = tempfile::tempdir().unwrap();
    let outside = outside.path().canonicalize().unwrap();
    let inside_db = root.join("inside.db");
    Connection::open(&inside_db)
        .unwrap()
        .execute_batch("CREATE TABLE x (a); INSERT INTO x VALUES (1);")
        .unwrap();
    let outside_db = outside.join("outside.db");
    std::fs::copy(&inside_db, &outside_db).unwrap();
    let mut conn = connect(spec(fixture.path().unwrap())).await;

    conn.execute(write(&format!("ATTACH {} AS inside", literal(&inside_db))))
        .await
        .unwrap();
    assert_eq!(count(&mut *conn, "SELECT count(*) FROM inside.x").await, 1);
    conn.execute(write("DETACH inside")).await.unwrap();

    let escaping = root
        .join("..")
        .join(outside.file_name().unwrap())
        .join("outside.db");
    let refused = [
        format!("ATTACH {} AS o", literal(&outside_db)),
        format!("ATTACH {} AS o", literal(&outside.join("new.db"))),
        format!("ATTACH {} AS o", literal(&escaping)),
        format!("VACUUM INTO {}", literal(&outside.join("copy.db"))),
    ];
    for sql in &refused {
        let error = conn.execute(write(sql)).await.unwrap_err();
        assert!(
            error
                .message
                .starts_with("attaching a file outside the project roots is not allowed"),
            "{sql}: {error:?}"
        );
    }
    let literal_only = conn
        .execute(write(&format!("ATTACH {} || '' AS o", literal(&inside_db))))
        .await
        .unwrap_err();
    assert_eq!(
        literal_only.message,
        "only a literal file name can be attached"
    );
    let uri = conn
        .execute(write("ATTACH 'file:other.db?mode=memory' AS o"))
        .await
        .unwrap_err();
    assert!(
        uri.message.starts_with("attaching a URI file name"),
        "{uri:?}"
    );
    assert!(!outside.join("new.db").exists());
    assert!(!outside.join("copy.db").exists());

    conn.execute(write(&format!(
        "VACUUM INTO {}",
        literal(&root.join("copy.db"))
    )))
    .await
    .unwrap();
    assert!(root.join("copy.db").exists());
    conn.execute(write("VACUUM")).await.unwrap();
}

/// 루트 안의 링크가 루트 밖을 가리키면 링크를 푼 실제 경로로 판정한다.
#[cfg(unix)]
#[tokio::test]
async fn attach_follows_links_before_judging_the_root() {
    let fixture = SqliteFixture::file();
    let root = real_parent(fixture.path().unwrap());
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("outside.db");
    Connection::open(&target)
        .unwrap()
        .execute_batch("CREATE TABLE x (a)")
        .unwrap();
    let link = root.join("link.db");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    let error = conn
        .execute(write(&format!("ATTACH {} AS o", literal(&link))))
        .await
        .unwrap_err();
    assert!(
        error.message.contains("outside the project roots"),
        "{error:?}"
    );
}

fn table(name: &str) -> ObjectRef {
    ObjectRef {
        database: None,
        schema: None,
        name: name.to_owned(),
    }
}

fn column(name: &str, db_type: &str, nullable: bool) -> ColumnInfo {
    ColumnInfo {
        name: name.to_owned(),
        db_type: db_type.to_owned(),
        nullable,
        default: None,
        comment: None,
    }
}

fn index(name: &str, columns: &[&str], primary: bool, unique: bool) -> IndexInfo {
    IndexInfo {
        name: name.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        primary,
        unique,
        partial: false,
    }
}

fn objects_query(kind: ObjectKind, limit: u32) -> ObjectQuery {
    ObjectQuery {
        database: None,
        schema: None,
        kind,
        filter: None,
        cursor: None,
        limit,
    }
}

fn names(page: &pw_db_types::ObjectPage) -> Vec<&str> {
    page.items.iter().map(|i| i.name.as_str()).collect()
}

/// 완료 기준 ⑧: 픽스처 스키마의 목록 · 열 · 인덱스 · FK · DDL 이 기대값이다.
#[tokio::test]
async fn the_fixture_schema_lists_and_ddl_are_the_expected_ones() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;

    let tables = conn
        .objects(objects_query(ObjectKind::Table, 500))
        .await
        .unwrap();
    assert_eq!(names(&tables), ["audit_log", "customers", "orders"]);
    assert_eq!(tables.next_cursor, None);
    let views = conn
        .objects(objects_query(ObjectKind::View, 500))
        .await
        .unwrap();
    assert_eq!(names(&views), ["big_orders"]);
    assert_eq!(views.items[0].kind, ObjectKind::View);
    for kind in [
        ObjectKind::Trigger,
        ObjectKind::Sequence,
        ObjectKind::Function,
    ] {
        assert!(
            conn.objects(objects_query(kind, 500))
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }
    let first = conn
        .objects(objects_query(ObjectKind::Table, 2))
        .await
        .unwrap();
    assert_eq!(names(&first), ["audit_log", "customers"]);
    assert_eq!(first.next_cursor.as_deref(), Some("customers"));
    let second = conn
        .objects(ObjectQuery {
            cursor: first.next_cursor,
            ..objects_query(ObjectKind::Table, 2)
        })
        .await
        .unwrap();
    assert_eq!(names(&second), ["orders"]);
    assert_eq!(second.next_cursor, None);
    let filtered = conn
        .objects(ObjectQuery {
            filter: Some("ORD".into()),
            ..objects_query(ObjectKind::Table, 500)
        })
        .await
        .unwrap();
    assert_eq!(names(&filtered), ["orders"]);
    let too_big = conn
        .objects(objects_query(ObjectKind::Table, 501))
        .await
        .unwrap_err();
    assert_eq!(too_big.kind, DbErrorKind::Invalid);

    assert_eq!(
        conn.columns(table("customers")).await.unwrap(),
        [
            column("id", "INTEGER", false),
            column("email", "TEXT", false),
            column("name", "TEXT", true),
            column("avatar", "BLOB", true),
        ]
    );
    assert_eq!(
        conn.columns(table("orders")).await.unwrap(),
        [
            column("id", "INTEGER", false),
            column("customer_id", "INTEGER", false),
            column("total", "NUMERIC", false),
            column("placed_at", "TEXT", true),
        ]
    );
    assert_eq!(
        conn.indexes(table("customers")).await.unwrap(),
        [
            index("PRIMARY", &["id"], true, true),
            index("sqlite_autoindex_customers_1", &["email"], false, true),
        ]
    );
    assert_eq!(
        conn.indexes(table("orders")).await.unwrap(),
        [
            index("PRIMARY", &["id"], true, true),
            index("orders_customer", &["customer_id"], false, false),
        ]
    );
    assert_eq!(
        conn.indexes(table("audit_log")).await.unwrap(),
        [],
        "no key"
    );
    assert_eq!(
        conn.foreign_keys(table("orders")).await.unwrap(),
        [ForeignKeyInfo {
            name: "fk_0".into(),
            columns: vec!["customer_id".into()],
            references: table("customers"),
            referenced_columns: vec!["id".into()],
        }]
    );
    assert_eq!(conn.foreign_keys(table("customers")).await.unwrap(), []);
    assert_eq!(
        conn.ddl(table("orders")).await.unwrap().text,
        "CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers(id),
    total NUMERIC NOT NULL,
    placed_at TEXT
);
CREATE INDEX orders_customer ON orders(customer_id);"
    );
    assert_eq!(
        conn.ddl(table("big_orders")).await.unwrap().text,
        "CREATE VIEW big_orders AS SELECT * FROM orders WHERE total > 100;"
    );
    for missing in [
        conn.columns(table("nope")).await.unwrap_err(),
        conn.ddl(table("nope")).await.unwrap_err(),
    ] {
        assert_eq!(missing.kind, DbErrorKind::Invalid, "{missing:?}");
    }
}

/// FK 가 열을 적지 않으면(`REFERENCES parent`) 부모의 기본 키를 가리킨다. 붙인 DB 의 객체는 `database` 로 고른다.
#[tokio::test]
async fn implicit_foreign_key_targets_and_attached_databases() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    conn.execute(write(
        "CREATE TABLE lines (order_id INTEGER REFERENCES orders, n INTEGER)",
    ))
    .await
    .unwrap();
    let lines = conn.foreign_keys(table("lines")).await.unwrap();
    assert_eq!(lines[0].referenced_columns, ["id"]);

    conn.execute(write("ATTACH '' AS scratch")).await.unwrap();
    conn.execute(write("CREATE TABLE scratch.notes (id INTEGER PRIMARY KEY)"))
        .await
        .unwrap();
    let scratch = ObjectQuery {
        database: Some("scratch".into()),
        ..objects_query(ObjectKind::Table, 500)
    };
    assert_eq!(names(&conn.objects(scratch).await.unwrap()), ["notes"]);
    let notes = ObjectRef {
        database: Some("scratch".into()),
        ..table("notes")
    };
    assert_eq!(
        conn.columns(notes).await.unwrap(),
        [column("id", "INTEGER", false)]
    );
}

/// 값 규칙: INTEGER 는 ±(2^53−1) 밖이면 문자열, REAL, TEXT, BLOB 은 hex, NULL. 열 메타의 타입은 선언 타입이다.
#[tokio::test]
async fn values_and_column_types_follow_the_rules() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    let started = conn
        .execute(read(
            "SELECT id, email, name, avatar, 1 + 1 AS two FROM customers ORDER BY id",
        ))
        .await
        .unwrap();
    let types: Vec<(&str, &str)> = started
        .columns
        .iter()
        .map(|c: &ColumnMeta| (c.name.as_str(), c.db_type.as_str()))
        .collect();
    assert_eq!(
        types,
        [
            ("id", "INTEGER"),
            ("email", "TEXT"),
            ("name", "TEXT"),
            ("avatar", "BLOB"),
            ("two", "")
        ]
    );
    let rows = conn.fetch(FetchLimit::rows(10)).await.unwrap();
    let limit = CellLimit::DEFAULT;
    assert_eq!(
        rows.rows[..2],
        [
            vec![
                DbValue::int(1),
                DbValue::text("ada@example.com", limit),
                DbValue::text("Ada", limit),
                DbValue::bytes(&[0xde, 0xad, 0xbe, 0xef], limit),
                DbValue::int(2),
            ],
            vec![
                DbValue::int(2),
                DbValue::text("grace@example.com", limit),
                DbValue::text("그레이스", limit),
                DbValue::Null,
                DbValue::int(2),
            ],
        ]
    );
    assert_eq!(
        rows.end.unwrap().rows_affected,
        None,
        "a select changes nothing"
    );
    assert_eq!(
        query(&mut *conn, "SELECT total FROM orders ORDER BY id").await,
        [
            vec![DbValue::float(250.5)],
            vec![DbValue::int(20)],
            vec![DbValue::int(9_007_199_254_740_993_i64)],
        ]
    );
    let mut small = read("SELECT name, avatar FROM customers WHERE id = 1");
    small.cell_limit = CellLimit(3);
    assert_eq!(
        rows_of(&mut *conn, small).await.unwrap(),
        [vec![
            DbValue::text("Ada", CellLimit(3)),
            DbValue::bytes(&[0xde, 0xad, 0xbe, 0xef], CellLimit(3)),
        ]]
    );
}

/// 행은 credit 만큼만 걷는다(행 수 · 바이트 중 먼저), 커서 없이 `fetch` 는 `Invalid`, 커서가 열린 채 다른 문장을 시작하면 커서를
/// 먼저 버린다.
#[tokio::test]
async fn fetch_walks_only_as_far_as_the_limit() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    let no_cursor = conn.fetch(FetchLimit::rows(1)).await.unwrap_err();
    assert_eq!(no_cursor.kind, DbErrorKind::Invalid);

    conn.execute(read("SELECT id FROM customers ORDER BY id"))
        .await
        .unwrap();
    let first = conn.fetch(FetchLimit::rows(2)).await.unwrap();
    assert_eq!(first.rows, [vec![DbValue::int(1)], vec![DbValue::int(2)]]);
    assert!(first.end.is_none());
    let rest = conn.fetch(FetchLimit::rows(2)).await.unwrap();
    assert_eq!(rest.rows, [vec![DbValue::int(3)]]);
    assert!(rest.end.is_some());

    conn.execute(read("SELECT email FROM customers ORDER BY id"))
        .await
        .unwrap();
    let by_bytes = conn
        .fetch(FetchLimit {
            max_rows: 100,
            max_bytes: 10,
        })
        .await
        .unwrap();
    assert_eq!(
        by_bytes.rows.len(),
        1,
        "one row is over 10 bytes on its own"
    );
    // 커서가 열린 채 다음 문장 — 앞 커서는 버려진다.
    assert_eq!(count(&mut *conn, "SELECT count(*) FROM orders").await, 3);
    assert_eq!(
        conn.fetch(FetchLimit::rows(1)).await.unwrap_err().kind,
        DbErrorKind::Invalid,
        "the earlier cursor is gone"
    );
}

/// 커서 버리기는 리셋이다 — `… RETURNING` 의 쓰기는 첫 걸음에 다 되므로 버려도 남는다.
#[tokio::test]
async fn discarding_a_returning_cursor_keeps_its_writes() {
    let fixture = SqliteFixture::file();
    let spec = spec(fixture.path().unwrap());
    let mut conn = connect(spec.clone()).await;
    conn.execute(write(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50) \
         INSERT INTO audit_log SELECT 'returned', 'row ' || i FROM n RETURNING message",
    ))
    .await
    .unwrap();
    assert_eq!(conn.fetch(FetchLimit::rows(1)).await.unwrap().rows.len(), 1);
    conn.discard_cursor().await.unwrap();
    let mut other = connect(spec).await;
    assert_eq!(
        count(
            &mut *other,
            "SELECT count(*) FROM audit_log WHERE at = 'returned'"
        )
        .await,
        50
    );
}

/// 읽기 전용 트랜잭션: `BEGIN` + `query_only` — 그 안의 쓰기 · `query_only` 풀기는 거절되고, 트랜잭션이 끝나면(롤백 · 사람의
/// `COMMIT`) 읽기 쓰기 연결로 돌아온다.
#[tokio::test]
async fn a_read_only_transaction_refuses_writes_until_it_ends() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    let insert = "INSERT INTO audit_log VALUES ('x', 'y')";

    conn.begin(BeginOptions { read_only: true }).await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    assert_eq!(
        conn.execute(write(insert)).await.unwrap_err().kind,
        DbErrorKind::ReadOnly
    );
    let unlock = conn
        .execute(write("PRAGMA query_only = 0"))
        .await
        .unwrap_err();
    assert!(unlock.message.contains("query_only"), "{unlock:?}");
    assert_eq!(count(&mut *conn, "SELECT count(*) FROM orders").await, 3);
    conn.rollback().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    conn.execute(write(insert)).await.unwrap();

    conn.begin(BeginOptions { read_only: true }).await.unwrap();
    conn.execute(write("COMMIT")).await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    conn.execute(write(insert)).await.unwrap();
    assert_eq!(count(&mut *conn, "SELECT count(*) FROM audit_log").await, 4);
}

/// 늘 끄는 것: `trusted_schema` 는 꺼져 있고, defensive 라 `writable_schema` 로 스키마를 고치지 못한다.
#[tokio::test]
async fn the_schema_is_untrusted_and_defensive() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    assert_eq!(count(&mut *conn, "PRAGMA trusted_schema").await, 0);
    let _ = conn.execute(write("PRAGMA writable_schema = ON")).await;
    let error = conn
        .execute(write(
            "UPDATE sqlite_schema SET sql = 'x' WHERE name = 'orders'",
        ))
        .await
        .unwrap_err();
    assert_eq!(error.kind, DbErrorKind::Server, "{error:?}");
    assert_eq!(
        count(
            &mut *conn,
            "SELECT count(*) FROM sqlite_schema WHERE sql = 'x'"
        )
        .await,
        0
    );
}

/// 세이브포인트까지만 되돌린다.
#[tokio::test]
async fn savepoints_roll_back_only_their_part() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(write("INSERT INTO audit_log VALUES ('a', 'kept')"))
        .await
        .unwrap();
    conn.savepoint("edit \"1\"".into()).await.unwrap();
    conn.execute(write("INSERT INTO audit_log VALUES ('b', 'undone')"))
        .await
        .unwrap();
    conn.rollback_to_savepoint("edit \"1\"".into())
        .await
        .unwrap();
    conn.release_savepoint("edit \"1\"".into()).await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    conn.commit().await.unwrap();
    assert_eq!(
        query(
            &mut *conn,
            "SELECT message FROM audit_log WHERE at IN ('a', 'b')"
        )
        .await,
        [vec![DbValue::text("kept", CellLimit::DEFAULT)]]
    );
    let unknown = conn.rollback_to_savepoint("nope".into()).await.unwrap_err();
    assert_eq!(unknown.kind, DbErrorKind::Server);
}

/// 바인드 값은 SQL 글에 잇지 않는다 — 따옴표 · 주석 글자가 그대로 들어간다. 바뀐 행 수는 DML 에만 있다.
#[tokio::test]
async fn bind_values_and_rows_affected() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    let tricky = "'; DROP TABLE orders; --";
    let mut insert = write("INSERT INTO customers (id, email, name, avatar) VALUES (?, ?, ?, ?)");
    insert.params = vec![
        DbValue::int(4),
        DbValue::text(tricky, CellLimit::DEFAULT),
        DbValue::Null,
        DbValue::bytes(&[0, 1, 0xff], CellLimit::DEFAULT),
    ];
    let done: StatementStarted = conn.execute(insert).await.unwrap();
    assert_eq!(done.end.unwrap().rows_affected, Some(1));
    assert_eq!(
        query(
            &mut *conn,
            "SELECT email, name, avatar FROM customers WHERE id = 4"
        )
        .await,
        [vec![
            DbValue::text(tricky, CellLimit::DEFAULT),
            DbValue::Null,
            DbValue::bytes(&[0, 1, 0xff], CellLimit::DEFAULT),
        ]]
    );
    assert_eq!(count(&mut *conn, "SELECT count(*) FROM orders").await, 3);

    let none = conn
        .execute(write("UPDATE orders SET total = 0 WHERE id = -1"))
        .await
        .unwrap();
    assert_eq!(none.end.unwrap().rows_affected, Some(0));
    let ddl = conn.execute(write("CREATE TABLE t (x)")).await.unwrap();
    assert_eq!(
        ddl.end.unwrap().rows_affected,
        None,
        "not the earlier update's count"
    );

    let mut short = read("SELECT ?, ?");
    short.params = vec![DbValue::int(1)];
    let wrong_count = conn.execute(short).await.unwrap_err();
    assert_eq!(wrong_count.kind, DbErrorKind::Invalid);
    let two = conn.execute(read("SELECT 1; SELECT 2")).await.unwrap_err();
    assert_eq!(two.kind, DbErrorKind::Invalid, "one statement at a time");
    let blank = conn.execute(read("  -- nothing")).await.unwrap();
    assert!(blank.columns.is_empty() && blank.end.is_some());
}

/// `EXPLAIN QUERY PLAN` 을 들여쓴 트리로. 문장은 실행하지 않는다.
#[tokio::test]
async fn explain_returns_the_query_plan_tree_without_running_the_statement() {
    let fixture = SqliteFixture::file();
    let mut conn = connect(spec(fixture.path().unwrap())).await;
    let plan = conn
        .explain("SELECT * FROM orders WHERE customer_id = 1".into())
        .await
        .unwrap();
    assert_eq!(plan.format, pw_db_types::PlanFormat::Text);
    assert_eq!(
        plan.text,
        "SEARCH orders USING INDEX orders_customer (customer_id=?)"
    );
    let compound = conn
        .explain("SELECT id FROM orders UNION SELECT id FROM customers".into())
        .await
        .unwrap();
    assert_eq!(
        compound.text,
        "MERGE (UNION)\n  LEFT\n    SCAN orders\n  RIGHT\n    SCAN customers"
    );
    conn.explain("DELETE FROM orders".into()).await.unwrap();
    assert_eq!(count(&mut *conn, "SELECT count(*) FROM orders").await, 3);
}

/// 연결을 여는 것: 없는 파일은 만들지 않고 오류, 상대 경로 · 네트워크 대상 · URI 이름은 거절. 지원 플래그는 갈래 검사를 지난다.
#[tokio::test]
async fn connecting_needs_an_existing_absolute_file() {
    let fixture = SqliteFixture::file();
    let missing = real_parent(fixture.path().unwrap()).join("missing.db");
    let error = SqliteDriver.connect(spec(&missing)).await.err().unwrap();
    assert_eq!(error.kind, DbErrorKind::Connect, "{error:?}");
    assert!(!missing.exists(), "the driver does not create files");

    let relative = SqliteDriver
        .connect(ConnectionSpec::new(
            "sqlite",
            Target::File {
                path: PathBuf::from("fixture.db"),
            },
        ))
        .await
        .err()
        .unwrap();
    assert_eq!(relative.kind, DbErrorKind::Invalid);
    let uri = SqliteDriver
        .connect(ConnectionSpec::new(
            "sqlite",
            Target::File {
                path: PathBuf::from("file:x.db"),
            },
        ))
        .await
        .err()
        .unwrap();
    assert_eq!(uri.kind, DbErrorKind::Invalid);
    let network = SqliteDriver
        .connect(ConnectionSpec::new(
            "sqlite",
            Target::Network {
                host: "db".into(),
                port: 1,
                connect_to: None,
            },
        ))
        .await
        .err()
        .unwrap();
    assert_eq!(network.kind, DbErrorKind::Invalid);

    let connected = SqliteDriver
        .connect(spec(fixture.path().unwrap()))
        .await
        .unwrap();
    assert_eq!(connected.server.tls, TlsStatus::NotApplicable);
    assert_eq!(
        connected.server.version.as_deref(),
        Some(rusqlite::version())
    );
    assert_eq!(SqliteDriver.capabilities().validate(), Ok(()));
    assert_eq!(SqliteDriver.id(), "sqlite");
    assert_eq!(
        SqliteDriver
            .test(spec(fixture.path().unwrap()))
            .await
            .unwrap()
            .tls,
        TlsStatus::NotApplicable
    );
}

/// 닫으면 열린 트랜잭션은 롤백된다. 손잡이를 놓으면 도는 문장이 멈춰 그 문장의 읽기 잠금이 풀린다.
#[tokio::test]
async fn closing_rolls_back_and_dropping_stops_the_running_statement() {
    let fixture = SqliteFixture::file();
    let spec = spec(fixture.path().unwrap());
    let mut conn = connect(spec.clone()).await;
    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(write("INSERT INTO audit_log VALUES ('x', 'gone')"))
        .await
        .unwrap();
    conn.close().await.unwrap();
    let mut other = connect(spec.clone()).await;
    assert_eq!(
        count(
            &mut *other,
            "SELECT count(*) FROM audit_log WHERE message = 'gone'"
        )
        .await,
        0
    );

    // 테이블을 읽으며 끝나지 않는 조회는 그동안 읽기 잠금을 쥔다 — 쓰기의 커밋이 그것을 기다린다.
    let mut reader = connect(spec.clone()).await;
    let task = tokio::spawn(async move {
        reader
            .execute(read(&format!("{ENDLESS}, customers")))
            .await
            .unwrap();
        let _ = reader.fetch(FetchLimit::rows(1)).await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    task.abort();
    let _ = task.await;
    let started = Instant::now();
    other
        .execute(write(
            "INSERT INTO audit_log VALUES ('x', 'after the drop')",
        ))
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}
