//! MySQL · MariaDB 실 DB 시험(14-m3-plan.md M3-07 의 완료 기준 ① ~ ⑦ · ⑨). `PW_TEST_MYSQL_URL` 이 있을 때만 목록에 있다
//! (`pw_db_driver::live` 머리 주석). MariaDB 는 `scripts/db-live.mjs` 가 같은 시험을 `PW_TEST_MARIADB_URL` 로 한 번 더 돈다.
//! 시간 기준(1 s)은 CI `db-live` 잡이 `--release` 로 돌며 판정한다.
#![cfg(pw_live_mysql)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mysql_async::prelude::Queryable;
use pw_db_driver::{DbConnection, DbDriver, QueryCancel, SqlConnection};
use pw_db_driver_mysql::{MysqlDriver, tls};
use pw_db_types::{
    BeginOptions, DbErrorKind, DbInt, DbValue, ExecuteRequest, FetchLimit, ObjectKind, ObjectQuery,
    ObjectRef, RiskGrade, SocketTarget, SslMode, Target, TlsStatus, TransactionState,
};
use support::proxy::{self, Mode};
use support::*;

/// 서버의 그 연결이 아직 문장을 돌리고 있는가.
async fn running(admin: &mut mysql_async::Conn, id: u64) -> bool {
    let count: Option<u64> = admin
        .exec_first(
            "SELECT COUNT(*) FROM information_schema.processlist WHERE id = ? AND command IN ('Query', 'Execute')",
            (id,),
        )
        .await
        .unwrap();
    count == Some(1)
}

async fn connection_id(conn: &mut dyn SqlConnection) -> u64 {
    match scalar(conn, "SELECT CONNECTION_ID()").await {
        DbValue::Int(DbInt::Number(id)) => u64::try_from(id).unwrap(),
        other => panic!("CONNECTION_ID() gave {other:?}"),
    }
}

/// ① 100만 행을 1,000행 credit 으로 읽다 상한에서 버리면 서버에서 그 쿼리가 1 s 안에 사라지고 연결은 그대로다.
#[tokio::test]
async fn live_mysql_row_cap_kills_the_select_and_keeps_the_connection() {
    let (mut conn, _) = open(SslMode::Disable).await;
    let id = connection_id(conn.as_mut()).await;
    let mut admin = admin().await;
    conn.execute(ExecuteRequest::new(numbers(1_000_000), RiskGrade::Read))
        .await
        .unwrap();
    let mut read = 0;
    while read < 5_000 {
        let batch = conn.fetch(FetchLimit::rows(1_000)).await.unwrap();
        assert!(batch.end.is_none());
        read += batch.rows.len();
    }
    assert!(
        running(&mut admin, id).await,
        "the server is still sending the rest"
    );
    let started = Instant::now();
    conn.discard_cursor().await.unwrap();
    while running(&mut admin, id).await {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the query is still in PROCESSLIST"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    eprintln!(
        "row cap: the query left PROCESSLIST {:?} after the discard began",
        started.elapsed()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        connection_id(conn.as_mut()).await,
        id,
        "the same connection"
    );
    assert_eq!(scalar(conn.as_mut(), "SELECT 1 + 1").await, DbValue::int(2));
    conn.close().await.unwrap();
}

/// ② 수동 커밋 세션에서 `INSERT` → 상한 + 1 행 `SELECT`(끊김) → `COMMIT` 뒤 다른 연결에서 그 `INSERT` 가 보인다.
#[tokio::test]
async fn live_mysql_row_cap_in_a_manual_transaction_keeps_the_insert() {
    let table = unique("pw_live_tx");
    let mut admin = admin().await;
    admin
        .query_drop(format!("CREATE TABLE {table} (n INT PRIMARY KEY)"))
        .await
        .unwrap();
    let (mut conn, _) = open(SslMode::Disable).await;
    conn.begin(BeginOptions::default()).await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    write(conn.as_mut(), &format!("INSERT INTO {table} VALUES (7)")).await;

    let cap = 1_000;
    conn.execute(ExecuteRequest::new(numbers(1_000_000), RiskGrade::Read))
        .await
        .unwrap();
    let mut read = 0;
    while read <= cap {
        read += conn.fetch(FetchLimit::rows(500)).await.unwrap().rows.len();
    }
    conn.discard_cursor().await.unwrap();
    assert_eq!(
        conn.transaction_state(),
        TransactionState::InTransaction,
        "the cap keeps the transaction"
    );

    let before: Vec<u32> = admin.query(format!("SELECT n FROM {table}")).await.unwrap();
    assert!(before.is_empty(), "not committed yet");
    conn.commit().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    let after: Vec<u32> = admin.query(format!("SELECT n FROM {table}")).await.unwrap();
    assert_eq!(after, [7]);
    conn.close().await.unwrap();
    admin
        .query_drop(format!("DROP TABLE {table}"))
        .await
        .unwrap();
}

/// ③ `SELECT SLEEP(30)` 취소가 1 s 안에 끝나고(TLS 연결 — `KILL` 연결도 TLS), 서버에서도 멈추고, 연결은 그대로다.
#[tokio::test]
async fn live_mysql_cancel_stops_sleep_within_a_second() {
    let (mut conn, _) = open(SslMode::Require).await;
    let id = connection_id(conn.as_mut()).await;
    let cancel = QueryCancel::new(conn.interrupter(), Duration::from_secs(5));
    let canceller = cancel.clone();
    let fired = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let at = Instant::now();
        canceller.cancel().await.unwrap();
        at
    });
    let result = cancel
        .run(async {
            let started = conn
                .execute(ExecuteRequest::new("SELECT SLEEP(30)", RiskGrade::Read))
                .await?;
            assert!(started.end.is_none());
            conn.fetch(FetchLimit::rows(10)).await
        })
        .await;
    let returned = Instant::now();
    let fired = fired.await.unwrap();
    assert_eq!(result.unwrap_err().kind, DbErrorKind::Cancelled);
    let took = returned.duration_since(fired);
    eprintln!("cancel: the call returned {took:?} after the cancel");
    assert!(took < Duration::from_secs(1), "{took:?}");
    let mut admin = admin().await;
    assert!(
        !running(&mut admin, id).await,
        "the server stopped the query"
    );
    assert_eq!(
        scalar(conn.as_mut(), "SELECT 'still here'").await,
        DbValue::Text("still here".into())
    );
    conn.close().await.unwrap();
}

/// ④ 읽기 전용 트랜잭션에서 `INSERT` 가 DB 오류 1792 로 거절된다.
#[tokio::test]
async fn live_mysql_read_only_transaction_refuses_an_insert() {
    let table = unique("pw_live_ro");
    let mut admin = admin().await;
    admin
        .query_drop(format!("CREATE TABLE {table} (n INT)"))
        .await
        .unwrap();
    let (mut conn, _) = open(SslMode::Disable).await;
    conn.begin(BeginOptions { read_only: true }).await.unwrap();
    let refused = conn
        .execute(ExecuteRequest::new(
            format!("INSERT INTO {table} VALUES (1)"),
            RiskGrade::Write,
        ))
        .await
        .unwrap_err();
    assert_eq!(
        (refused.kind, refused.code.as_deref()),
        (DbErrorKind::ReadOnly, Some("1792"))
    );
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    conn.rollback().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    let count: Option<u64> = admin
        .query_first(format!("SELECT COUNT(*) FROM {table}"))
        .await
        .unwrap();
    assert_eq!(count, Some(0));
    conn.close().await.unwrap();
    admin
        .query_drop(format!("DROP TABLE {table}"))
        .await
        .unwrap();
}

/// ⑤ 값 표 — 텍스트 프로토콜(바인드 없음)과 바이너리 프로토콜(바인드 있음)이 같은 값을 낸다.
#[tokio::test]
async fn live_mysql_values_follow_the_value_rules() {
    let table = unique("pw_live_values");
    let (mut conn, version) = open(SslMode::Disable).await;
    write(
        conn.as_mut(),
        &format!(
            "CREATE TABLE {table} (b BIGINT, u BIGINT UNSIGNED, d DECIMAL(30,10), bl BLOB, j JSON, dt DATETIME(6), \
             t VARCHAR(20), f DOUBLE, fl FLOAT)"
        ),
    )
    .await;
    write(
        conn.as_mut(),
        &format!(
            "INSERT INTO {table} VALUES (5, 18446744073709551615, 12345678901234567890.1234567890, X'DEADBEEF', \
             '{{\"a\": [1, 2]}}', '2026-10-31 12:34:56.123456', 'héllo', 1.5, 0.1)"
        ),
    )
    .await;
    let json = if is_mariadb(&version) {
        // MariaDB 의 JSON 은 LONGTEXT 의 별칭이라 열 타입으로 가를 수 없다(`value` 머리 주석).
        DbValue::Text(r#"{"a": [1, 2]}"#.into())
    } else {
        DbValue::Json(r#"{"a": [1, 2]}"#.into())
    };
    let expected = vec![
        DbValue::Int(DbInt::Number(5)),
        DbValue::Int(DbInt::Text("18446744073709551615".into())),
        DbValue::Decimal("12345678901234567890.1234567890".into()),
        DbValue::bytes(&[0xde, 0xad, 0xbe, 0xef], pw_db_types::CellLimit::DEFAULT),
        json,
        DbValue::Datetime("2026-10-31 12:34:56.123456".into()),
        DbValue::Text("héllo".into()),
        DbValue::float(1.5),
        // 단정밀도 0.1 은 0.1 로(넓혀 보인 0.10000000149011612 가 아니다).
        DbValue::float(0.1),
    ];
    // 바인드가 있든 없든 준비 문장(바이너리 프로토콜)이다(`conn` 머리 주석).
    let select = format!("SELECT b, u, d, bl, j, dt, t, f, fl FROM {table}");
    let prepared = rows(conn.as_mut(), &select, RiskGrade::Read).await;
    assert_eq!(
        prepared,
        std::slice::from_ref(&expected),
        "{version} prepared without binds"
    );
    // 텍스트 프로토콜로 행이 오는 길: `HANDLER … READ` 는 MySQL 이 준비하지 않아(1295) 텍스트로 간다(MariaDB 는 준비한다).
    write(conn.as_mut(), &format!("HANDLER {table} OPEN")).await;
    let handler = rows(
        conn.as_mut(),
        &format!("HANDLER {table} READ FIRST"),
        RiskGrade::Read,
    )
    .await;
    write(conn.as_mut(), &format!("HANDLER {table} CLOSE")).await;
    assert_eq!(
        handler,
        std::slice::from_ref(&expected),
        "{version} HANDLER READ"
    );
    let mut bound = ExecuteRequest::new(format!("{select} WHERE b = ?"), RiskGrade::Read);
    bound.params = vec![DbValue::int(5)];
    assert_eq!(
        run(conn.as_mut(), bound).await,
        [expected],
        "{version} binary protocol"
    );

    let started = conn
        .execute(ExecuteRequest::new(select, RiskGrade::Read))
        .await
        .unwrap();
    let types: Vec<&str> = started.columns.iter().map(|c| c.db_type.as_str()).collect();
    assert_eq!(
        &types[..4],
        ["BIGINT", "BIGINT UNSIGNED", "DECIMAL", "BLOB"]
    );
    conn.discard_cursor().await.unwrap();
    write(conn.as_mut(), &format!("DROP TABLE {table}")).await;
    conn.close().await.unwrap();
}

fn tunnelled(mode: SslMode, host: &str, via: std::net::SocketAddr) -> pw_db_types::ConnectionSpec {
    let mut spec = spec(mode);
    let Target::Network { port, .. } = spec.target else {
        unreachable!()
    };
    spec.target = Target::Network {
        host: host.into(),
        port,
        connect_to: Some(SocketTarget {
            host: via.ip().to_string(),
            port: via.port(),
        }),
    };
    spec
}

fn with_test_ca() -> MysqlDriver {
    MysqlDriver::with_root_source(Arc::new(|| {
        let mut roots = tls::native_roots()?;
        roots.push(proxy::test_ca());
        Ok(roots)
    }))
}

/// ⑥ 시험 CA 를 루트 공급 자리에 넣으면 `verify-full` 로 붙고(터널 — TLS 이름은 설정의 DB 호스트), 빼면 거절된다.
#[tokio::test]
async fn live_mysql_test_ca_verifies_full_through_a_tunnel_and_is_refused_without() {
    let via = proxy::start(server_address(), Mode::Tls).await;
    let spec = tunnelled(SslMode::VerifyFull, "db.pitwall.test", via);

    let connected = with_test_ca().connect(spec.clone()).await.unwrap();
    assert_eq!(connected.server.tls, TlsStatus::Verified);
    let DbConnection::Sql(mut conn) = connected.connection else {
        unreachable!()
    };
    assert_eq!(scalar(conn.as_mut(), "SELECT 1").await, DbValue::int(1));
    conn.close().await.unwrap();

    let refused = MysqlDriver::new()
        .connect(spec.clone())
        .await
        .err()
        .unwrap();
    assert_eq!(refused.kind, DbErrorKind::Tls, "{refused}");

    // 이름이 다르면 거절한다. verify-ca(이름을 보지 않는다)는 이 드라이버가 받지 않는다 — 붙어 보지도 않고 거절한다.
    let wrong_name = tunnelled(SslMode::VerifyFull, "other.pitwall.test", via);
    let refused = with_test_ca().connect(wrong_name).await.err().unwrap();
    assert_eq!(refused.kind, DbErrorKind::Tls, "{refused}");
    let chain_only = tunnelled(SslMode::VerifyCa, "other.pitwall.test", via);
    let refused = with_test_ca().connect(chain_only).await.err().unwrap();
    assert_eq!(refused.kind, DbErrorKind::Invalid, "{refused}");
}

/// `prefer` 는 TLS 를 못 하는 서버에 평문으로(표지 「평문」), `allow` 는 평문을 거절하는 서버에 TLS 로 다시 붙는다.
#[tokio::test]
async fn live_mysql_prefer_and_allow_fall_back() {
    let no_tls = proxy::start(server_address(), Mode::NoTls).await;
    let connected = MysqlDriver::new()
        .connect(tunnelled(SslMode::Prefer, "db.pitwall.test", no_tls))
        .await
        .unwrap();
    assert_eq!(connected.server.tls, TlsStatus::Plaintext);
    connected.connection.close().await.unwrap();
    let refused = MysqlDriver::new()
        .connect(tunnelled(SslMode::Require, "db.pitwall.test", no_tls))
        .await;
    assert_eq!(refused.err().unwrap().kind, DbErrorKind::Tls);

    let tls_only = proxy::start(server_address(), Mode::RequireTls).await;
    let connected = MysqlDriver::new()
        .connect(tunnelled(SslMode::Allow, "db.pitwall.test", tls_only))
        .await
        .unwrap();
    assert_eq!(connected.server.tls, TlsStatus::Unverified);
    connected.connection.close().await.unwrap();
    let refused = MysqlDriver::new()
        .connect(tunnelled(SslMode::Disable, "db.pitwall.test", tls_only))
        .await;
    let refused = refused.err().unwrap();
    assert_eq!(
        (refused.kind, refused.code.as_deref()),
        (DbErrorKind::Tls, Some("3159"))
    );
}

/// ⑦ 픽스처 스키마의 목록(서버 쪽 필터 · 쪽) · 열 · 인덱스 · FK · DDL, 그리고 `EXPLAIN FORMAT=JSON`.
#[tokio::test]
async fn live_mysql_metadata_lists_the_fixture_schema() {
    let db = unique("pw_live_meta");
    let mut admin = admin().await;
    admin
        .query_drop(format!("CREATE DATABASE {db}"))
        .await
        .unwrap();
    for sql in [
        "CREATE TABLE parent (id INT PRIMARY KEY, name VARCHAR(20) NOT NULL, UNIQUE KEY ux_name (name))".to_owned(),
        "CREATE TABLE child (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, parent_id INT NOT NULL, \
         note TEXT COMMENT 'free text', KEY ix_parent (parent_id), \
         CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES parent (id)) COMMENT = 'child rows'"
            .to_owned(),
        "CREATE TABLE t_a (n INT)".to_owned(),
        "CREATE TABLE t_b (n INT)".to_owned(),
        "CREATE TABLE t_c (n INT)".to_owned(),
        "CREATE TABLE tax (n INT)".to_owned(),
        "CREATE VIEW v_child AS SELECT id, note FROM child".to_owned(),
    ] {
        admin.query_drop(format!("USE {db}")).await.unwrap();
        admin.query_drop(sql).await.unwrap();
    }
    let (mut conn, _) = open(SslMode::Disable).await;
    let query = |filter: Option<&str>, cursor: Option<String>, kind| ObjectQuery {
        database: Some(db.clone()),
        schema: None,
        kind,
        filter: filter.map(str::to_owned),
        cursor,
        limit: 2,
    };
    let mut names = Vec::new();
    let mut cursor = None;
    loop {
        let page = conn
            .objects(query(None, cursor, ObjectKind::Table))
            .await
            .unwrap();
        names.push(
            page.items
                .iter()
                .map(|o| o.name.clone())
                .collect::<Vec<_>>(),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        names,
        [
            vec!["child", "parent"],
            vec!["t_a", "t_b"],
            vec!["t_c", "tax"]
        ]
    );
    let filtered = conn
        .objects(ObjectQuery {
            limit: 10,
            ..query(Some("T_"), None, ObjectKind::Table)
        })
        .await
        .unwrap();
    let filtered: Vec<_> = filtered.items.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        filtered,
        ["t_a", "t_b", "t_c"],
        "`_` is a letter, not a wildcard (no `tax`); the case is ignored"
    );
    let views = conn
        .objects(query(None, None, ObjectKind::View))
        .await
        .unwrap();
    assert_eq!(
        views
            .items
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        ["v_child"]
    );
    let child_page = conn
        .objects(query(Some("child"), None, ObjectKind::Table))
        .await
        .unwrap();
    assert_eq!(child_page.items[0].comment.as_deref(), Some("child rows"));

    let child = ObjectRef {
        database: Some(db.clone()),
        schema: None,
        name: "child".into(),
    };
    let columns = conn.columns(child.clone()).await.unwrap();
    let column_names: Vec<_> = columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(column_names, ["id", "parent_id", "note"]);
    assert!(
        columns[0].db_type.starts_with("bigint") && columns[0].db_type.ends_with("unsigned"),
        "{columns:?}"
    );
    assert!(!columns[0].nullable && columns[2].nullable);
    assert_eq!(columns[2].comment.as_deref(), Some("free text"));

    let indexes = conn.indexes(child.clone()).await.unwrap();
    assert_eq!(indexes[0].name, "PRIMARY");
    assert!(indexes[0].primary && indexes[0].unique && indexes[0].columns == ["id"]);
    let by_parent = indexes.iter().find(|i| i.name == "ix_parent").unwrap();
    assert!(!by_parent.unique && by_parent.columns == ["parent_id"]);
    let parent = ObjectRef {
        name: "parent".into(),
        ..child.clone()
    };
    let unique_name = conn
        .indexes(parent)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.name == "ux_name")
        .unwrap();
    assert!(unique_name.unique && !unique_name.primary && !unique_name.partial);

    let keys = conn.foreign_keys(child.clone()).await.unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(
        (keys[0].name.as_str(), keys[0].columns.as_slice()),
        ("fk_parent", &["parent_id".to_owned()][..])
    );
    assert_eq!(
        keys[0].references,
        ObjectRef {
            database: Some(db.clone()),
            schema: None,
            name: "parent".into()
        }
    );
    assert_eq!(keys[0].referenced_columns, ["id"]);

    let ddl = conn.ddl(child.clone()).await.unwrap().text;
    assert!(
        ddl.starts_with("CREATE TABLE `child`") && ddl.contains("`fk_parent`"),
        "{ddl}"
    );
    let view_ddl = conn
        .ddl(ObjectRef {
            name: "v_child".into(),
            ..child
        })
        .await
        .unwrap()
        .text;
    assert!(
        view_ddl.contains(" VIEW ") && view_ddl.contains("`v_child`"),
        "{view_ddl}"
    );

    let databases = conn.databases().await.unwrap();
    assert!(databases.iter().any(|d| d.name == db));
    let plan = conn
        .explain(format!("SELECT * FROM {db}.child WHERE parent_id = 1"))
        .await
        .unwrap();
    assert!(
        plan.text.trim_start().starts_with('{') && plan.text.contains("query_block"),
        "{}",
        plan.text
    );

    conn.close().await.unwrap();
    admin
        .query_drop(format!("DROP DATABASE {db}"))
        .await
        .unwrap();
}

/// ⑨ 조회가 아닌 문장의 상한 끊기: 남은 행 100만 개를 끝까지 읽어 버린다(쓰기가 남는다). 그 시간을 재고, 그동안 취소하면
/// 어떻게 되는지 본다. MariaDB 는 `INSERT … RETURNING`, MySQL 은 `RETURNING` 이 없어 등급이 쓰기인 100만 행 조회로 읽어 버리는
/// 길만 잰다.
#[tokio::test]
async fn live_mysql_write_cap_drains_the_rest_and_can_be_cancelled() {
    let (mut conn, version) = open(SslMode::Disable).await;
    let mariadb = is_mariadb(&version);
    let table = unique("pw_live_returning");
    // 결과가 소켓 · 중간 버퍼에 다 들어가면 서버의 문장은 이미 끝나 취소가 닿을 곳이 없다 — 행마다 100바이트를 붙여(약 100 MB)
    // 읽어 버리는 동안에도 서버가 보내는 중이게 한다.
    let statement = |table: &str| {
        if mariadb {
            format!(
                "INSERT INTO {table} (id) SELECT id FROM ({}) AS g RETURNING id, REPEAT('x', 100) AS pad",
                numbers(1_000_000)
            )
        } else {
            numbers(1_000_000)
        }
    };
    let count = |table: String| async move {
        let mut admin = admin().await;
        let n: Option<u64> = admin
            .query_first(format!("SELECT COUNT(*) FROM {table}"))
            .await
            .unwrap();
        n.unwrap()
    };
    for t in [&table, &format!("{table}_cancel")] {
        write(
            conn.as_mut(),
            &format!("CREATE TABLE {t} (id INT PRIMARY KEY)"),
        )
        .await;
    }

    conn.execute(ExecuteRequest::new(statement(&table), RiskGrade::Write))
        .await
        .unwrap();
    assert_eq!(
        conn.fetch(FetchLimit::rows(1_000))
            .await
            .unwrap()
            .rows
            .len(),
        1_000
    );
    let started = Instant::now();
    conn.discard_cursor().await.unwrap();
    let drained = started.elapsed();
    eprintln!("{version}: draining the rest of 1,000,000 rows took {drained:?}");
    if mariadb {
        assert_eq!(
            count(table.clone()).await,
            1_000_000,
            "the drained write stays"
        );
    }

    let cancelled_table = format!("{table}_cancel");
    conn.execute(ExecuteRequest::new(
        statement(&cancelled_table),
        RiskGrade::Write,
    ))
    .await
    .unwrap();
    assert_eq!(
        conn.fetch(FetchLimit::rows(1_000))
            .await
            .unwrap()
            .rows
            .len(),
        1_000
    );
    let interrupter = conn.interrupter();
    let wait = drained / 4;
    let cancel = tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        let at = Instant::now();
        interrupter.interrupt().await.unwrap();
        at
    });
    let outcome = conn.discard_cursor().await;
    let returned = Instant::now();
    let fired = cancel.await.unwrap();
    eprintln!(
        "{version}: a cancel {wait:?} into the drain → {:?}, {:?} after the cancel",
        outcome.as_ref().map_err(|e| e.kind),
        returned.saturating_duration_since(fired)
    );
    assert_eq!(
        outcome.unwrap_err().kind,
        DbErrorKind::Cancelled,
        "the drain can be cancelled"
    );
    if mariadb {
        let kept = count(cancelled_table.clone()).await;
        eprintln!("{version}: rows of the cancelled INSERT … RETURNING left in the table: {kept}");
        assert_eq!(kept, 0, "a cancelled statement is rolled back");
    }
    assert_eq!(
        scalar(conn.as_mut(), "SELECT 3").await,
        DbValue::int(3),
        "the connection is usable"
    );
    for t in [table, cancelled_table] {
        write(conn.as_mut(), &format!("DROP TABLE {t}")).await;
    }
    conn.close().await.unwrap();
}

/// 세이브포인트까지만 되돌리고, `SET autocommit = 0` 의 암묵 트랜잭션도 상태로 보이며, 열린 트랜잭션 위의 `begin` 은 거절한다.
#[tokio::test]
async fn live_mysql_savepoints_and_the_transaction_state_follow_the_server() {
    let table = unique("pw_live_sp");
    let (mut conn, _) = open(SslMode::Disable).await;
    write(conn.as_mut(), &format!("CREATE TABLE {table} (n INT)")).await;
    conn.begin(BeginOptions::default()).await.unwrap();
    write(conn.as_mut(), &format!("INSERT INTO {table} VALUES (1)")).await;
    conn.savepoint("pw_edit".into()).await.unwrap();
    write(conn.as_mut(), &format!("INSERT INTO {table} VALUES (2)")).await;
    conn.rollback_to_savepoint("pw_edit".into()).await.unwrap();
    conn.release_savepoint("pw_edit".into()).await.unwrap();
    let again = conn.begin(BeginOptions::default()).await.unwrap_err();
    assert_eq!(
        again.kind,
        DbErrorKind::Invalid,
        "START TRANSACTION would commit silently"
    );
    conn.commit().await.unwrap();
    let kept = rows(
        conn.as_mut(),
        &format!("SELECT n FROM {table}"),
        RiskGrade::Read,
    )
    .await;
    assert_eq!(kept, [vec![DbValue::int(1)]]);

    write(conn.as_mut(), "SET autocommit = 0").await;
    write(conn.as_mut(), &format!("INSERT INTO {table} VALUES (3)")).await;
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    conn.rollback().await.unwrap();
    write(conn.as_mut(), "SET autocommit = 1").await;
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    write(conn.as_mut(), &format!("DROP TABLE {table}")).await;
    conn.close().await.unwrap();
}

/// `verify-ca` 는 이 드라이버가 받지 않는다(`tls` 머리 주석) — 실 서버를 가리켜도 붙어 보기 전에 까닭과 대안을 실어 거절한다.
#[tokio::test]
async fn live_mysql_verify_ca_is_refused_before_connecting() {
    let refused = MysqlDriver::new()
        .connect(spec(SslMode::VerifyCa))
        .await
        .err()
        .unwrap();
    assert_eq!(refused.kind, DbErrorKind::Invalid, "{refused}");
    assert!(refused.message.contains("verify-full"), "{refused}");
    // 같은 설정이 verify-ca 만 아니면 서버에 닿는다 — 거절은 서버가 아니라 드라이버의 것이다.
    let connected = MysqlDriver::new()
        .connect(spec(SslMode::Require))
        .await
        .unwrap();
    assert_eq!(connected.server.tls, TlsStatus::Unverified);
    connected.connection.close().await.unwrap();
}

/// 세션의 준비 · 닫기 횟수(`Com_stmt_prepare`, `Com_stmt_close`).
async fn statement_counters(conn: &mut dyn SqlConnection) -> (u64, u64) {
    let rows = rows(
        conn,
        "SHOW SESSION STATUS WHERE Variable_name IN ('Com_stmt_prepare', 'Com_stmt_close')",
        RiskGrade::Read,
    )
    .await;
    let value = |name: &str| {
        let row = rows
            .iter()
            .find(|r| r[0] == DbValue::Text(name.into()))
            .unwrap();
        match &row[1] {
            DbValue::Text(v) => v.parse::<u64>().unwrap(),
            other => panic!("{name} = {other:?}"),
        }
    };
    (value("Com_stmt_prepare"), value("Com_stmt_close"))
}

/// 나뉜 문장 하나 = 서버 문장 하나: 한 번에 보낸 `SELECT 1; DROP TABLE t` 는 서버가 거절하고 `t` 가 남는다. 준비 문장으로 받지
/// 않는 문장(MySQL 의 `CREATE PROCEDURE` · `USE`)은 그대로 돌고, 준비 문장은 문장마다 닫힌다(세션의 준비 · 닫기 수가 같다 — 준비에
/// 실패한 문장도 준비 수에 들어가므로, 늘 준비되는 문장만 세는 구간에서 잰다).
#[tokio::test]
async fn live_mysql_one_send_is_one_server_statement() {
    let table = unique("pw_live_keep");
    let procedure = unique("pw_live_proc");
    let mut admin = admin().await;
    admin
        .query_drop(format!("CREATE TABLE {table} (n INT)"))
        .await
        .unwrap();
    let (mut conn, _) = open(SslMode::Disable).await;

    let database = spec(SslMode::Disable).database.unwrap();
    // 서버가 준비하지 않는 문장(MySQL 1295)을 앞에 둔 꼴과 진단 문장 · 실행 주석 꼴까지 — 모두 1064 로 거절되어 텍스트로 가지 않는다.
    for hidden in [
        format!("SELECT 1; DROP TABLE {table}"),
        format!("CREATE PROCEDURE {procedure}() SELECT 1; DROP TABLE {table}"),
        format!("USE {database}; DROP TABLE {table}"),
        format!("BEGIN; DROP TABLE {table}"),
        format!("SAVEPOINT a; DROP TABLE {table}"),
        format!("LOCK TABLES {table} READ; DROP TABLE {table}"),
        format!("SHOW WARNINGS; DROP TABLE {table}"),
        format!("GET DIAGNOSTICS @n = NUMBER; DROP TABLE {table}"),
        format!("USE {database} /*! ; DROP TABLE {table} */"),
    ] {
        let refused = conn
            .execute(ExecuteRequest::new(hidden.clone(), RiskGrade::Read))
            .await
            .unwrap_err();
        assert_eq!(
            (refused.kind, refused.code.as_deref()),
            (DbErrorKind::Server, Some("1064")),
            "{hidden}: {refused}"
        );
    }
    let kept: Option<u64> = admin
        .query_first(format!("SELECT COUNT(*) FROM {table}"))
        .await
        .unwrap();
    assert_eq!(kept, Some(0), "the hidden DROP never ran");

    write(
        conn.as_mut(),
        &format!("CREATE PROCEDURE {procedure}() BEGIN SELECT 1 AS a; SELECT 2 AS b; END"),
    )
    .await;
    let first = rows(
        conn.as_mut(),
        &format!("CALL {procedure}()"),
        RiskGrade::Write,
    )
    .await;
    assert_eq!(first, [vec![DbValue::int(1)]], "the first result set");
    write(conn.as_mut(), &format!("USE {database}")).await;
    write(conn.as_mut(), &format!("DROP PROCEDURE {procedure}")).await;

    let before = statement_counters(conn.as_mut()).await;
    assert_eq!(
        rows(conn.as_mut(), &numbers(3), RiskGrade::Read)
            .await
            .len(),
        3
    );
    write(conn.as_mut(), &format!("INSERT INTO {table} VALUES (1)")).await;
    let started = conn
        .execute(ExecuteRequest::new(numbers(10), RiskGrade::Read))
        .await
        .unwrap();
    assert!(started.end.is_none());
    conn.discard_cursor().await.unwrap();
    conn.columns(ObjectRef {
        database: Some(database),
        schema: None,
        name: table.clone(),
    })
    .await
    .unwrap();

    // 앞 읽기는 자신의 준비를 세고 닫기를 세지 않았고, 뒤 읽기도 그렇다 — 그 사이 문장이 다 닫혔으면 두 차이가 같다.
    let after = statement_counters(conn.as_mut()).await;
    assert_eq!(
        after.0 - before.0,
        after.1 - before.1,
        "prepared statements left open: before {before:?}, after {after:?}"
    );
    conn.close().await.unwrap();
    admin
        .query_drop(format!("DROP TABLE {table}"))
        .await
        .unwrap();
}

/// `LOAD DATA LOCAL INFILE` 은 사람 기기의 파일을 읽지 않고 거절된다(서버가 허용해도). 연결은 그대로다.
#[tokio::test]
async fn live_mysql_local_infile_is_refused_without_reading_the_file() {
    let table = unique("pw_live_infile");
    // 쓰는 쪽이 없는 FIFO — 드라이버가 이 파일을 열기만 해도 막혀 아래 시간 상한에 걸린다(「읽지 않는다」 를 넘어 「열지 않는다」).
    let file = std::env::temp_dir().join(format!("{table}.fifo"));
    let made = std::process::Command::new("mkfifo")
        .arg(&file)
        .status()
        .unwrap();
    assert!(made.success(), "mkfifo {}", file.display());
    let mut admin = admin().await;
    let was: Option<String> = admin
        .query_first("SELECT @@GLOBAL.local_infile")
        .await
        .unwrap();
    // MySQL 8.4 은 서버 쪽 기본이 꺼져 있어 클라이언트에 묻기도 전에 거절한다 — 클라이언트의 거절을 보려고 켠다.
    admin
        .query_drop("SET GLOBAL local_infile = 1")
        .await
        .unwrap();
    admin
        .query_drop(format!("CREATE TABLE {table} (n INT)"))
        .await
        .unwrap();
    let (mut conn, _) = open(SslMode::Disable).await;
    let sql = format!(
        "LOAD DATA LOCAL INFILE '{}' INTO TABLE {table}",
        file.display()
    );
    let refused = tokio::time::timeout(
        Duration::from_secs(20),
        conn.execute(ExecuteRequest::new(sql, RiskGrade::Write)),
    )
    .await
    .expect("the refusal does not hang")
    .unwrap_err();
    admin
        .query_drop(format!(
            "SET GLOBAL local_infile = {}",
            was.as_deref().unwrap_or("0")
        ))
        .await
        .unwrap();
    assert_eq!(refused.kind, DbErrorKind::Invalid, "{refused}");
    assert!(refused.message.contains("LOCAL INFILE"), "{refused}");
    let loaded: Option<u64> = admin
        .query_first(format!("SELECT COUNT(*) FROM {table}"))
        .await
        .unwrap();
    assert_eq!(loaded, Some(0), "no line of the file reached the server");
    let usable = tokio::time::timeout(Duration::from_secs(20), scalar(conn.as_mut(), "SELECT 4"))
        .await
        .expect("the connection is still in step with the server");
    assert_eq!(usable, DbValue::int(4));
    conn.close().await.unwrap();
    admin
        .query_drop(format!("DROP TABLE {table}"))
        .await
        .unwrap();
    std::fs::remove_file(file).unwrap();
}

/// 교착(1213)은 트랜잭션 전체를 되돌린다 — 그 오류에 `rolled_back` 이 실리고 연결의 상태가 `Idle` 이 된다. 문장만 되돌리는 오류
/// (중복 키 1062)는 `rolled_back` 이 아니고 트랜잭션이 남는다.
#[tokio::test]
async fn live_mysql_a_deadlock_reports_the_whole_transaction_rolled_back() {
    let table = unique("pw_live_deadlock");
    let mut admin = admin().await;
    admin
        .query_drop(format!(
            "CREATE TABLE {table} (id INT PRIMARY KEY, n INT) ENGINE = InnoDB"
        ))
        .await
        .unwrap();
    admin
        .query_drop(format!("INSERT INTO {table} VALUES (1, 0), (2, 0)"))
        .await
        .unwrap();
    let (mut a, _) = open(SslMode::Disable).await;
    let (mut b, _) = open(SslMode::Disable).await;
    a.begin(BeginOptions::default()).await.unwrap();
    write(
        a.as_mut(),
        &format!("UPDATE {table} SET n = 1 WHERE id = 1"),
    )
    .await;
    b.begin(BeginOptions::default()).await.unwrap();
    write(
        b.as_mut(),
        &format!("UPDATE {table} SET n = 2 WHERE id = 2"),
    )
    .await;
    // a 가 b 의 행을 기다리는 동안 b 가 a 의 행을 청해 고리를 닫는다.
    let waits = format!("UPDATE {table} SET n = 1 WHERE id = 2");
    let a_task = tokio::spawn(async move {
        let result = a
            .execute(ExecuteRequest::new(waits, RiskGrade::Write))
            .await;
        (a, result)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let b_result = b
        .execute(ExecuteRequest::new(
            format!("UPDATE {table} SET n = 2 WHERE id = 1"),
            RiskGrade::Write,
        ))
        .await;
    let (a, a_result) = a_task.await.unwrap();
    let (mut victim, mut survivor, error) = match (a_result, b_result) {
        (Err(e), Ok(_)) => (a, b, e),
        (Ok(_), Err(e)) => (b, a, e),
        other => panic!("exactly one side is the deadlock victim: {other:?}"),
    };
    assert_eq!(error.code.as_deref(), Some("1213"), "{error}");
    assert!(error.rolled_back, "the whole transaction was rolled back");
    assert_eq!(victim.transaction_state(), TransactionState::Idle);
    assert_eq!(
        survivor.transaction_state(),
        TransactionState::InTransaction
    );
    survivor.commit().await.unwrap();

    victim.begin(BeginOptions::default()).await.unwrap();
    write(
        victim.as_mut(),
        &format!("UPDATE {table} SET n = 9 WHERE id = 1"),
    )
    .await;
    let duplicate = victim
        .execute(ExecuteRequest::new(
            format!("INSERT INTO {table} VALUES (1, 0)"),
            RiskGrade::Write,
        ))
        .await
        .unwrap_err();
    assert_eq!(duplicate.code.as_deref(), Some("1062"), "{duplicate}");
    assert!(!duplicate.rolled_back, "only the statement was rolled back");
    assert_eq!(victim.transaction_state(), TransactionState::InTransaction);
    victim.rollback().await.unwrap();
    victim.close().await.unwrap();
    survivor.close().await.unwrap();
    admin
        .query_drop(format!("DROP TABLE {table}"))
        .await
        .unwrap();
}

/// 암묵 커밋 문장(DDL · `LOCK TABLES` 등)은 실행 전에 열린 트랜잭션을 커밋한다 — 그 뒤 실패해도 앞의 쓰기는 커밋됐다. 드라이버는
/// 그것을 「되돌렸다」(`rolled_back`)고 하지 않고 상태를 `Idle` 로 보인다. 다른 연결에서 그 쓰기가 보인다.
#[tokio::test]
async fn live_mysql_an_implicit_commit_is_not_reported_as_a_rollback() {
    let table = unique("pw_live_implicit");
    let mut admin = admin().await;
    admin
        .query_drop(format!("CREATE TABLE {table} (n INT)"))
        .await
        .unwrap();
    let (mut conn, _) = open(SslMode::Disable).await;
    for (n, failing) in [
        (1, format!("CREATE TABLE {table} (a INT)")),
        (2, format!("DROP TABLE {table}_no_such")),
        (3, format!("LOCK TABLES {table}_no_such READ")),
    ] {
        conn.begin(BeginOptions::default()).await.unwrap();
        write(conn.as_mut(), &format!("INSERT INTO {table} VALUES ({n})")).await;
        let error = conn
            .execute(ExecuteRequest::new(failing.clone(), RiskGrade::Ddl))
            .await
            .unwrap_err();
        assert!(
            !error.rolled_back,
            "{failing}: committed, not rolled back: {error}"
        );
        assert_eq!(
            conn.transaction_state(),
            TransactionState::Idle,
            "{failing}: the implicit commit ended the transaction"
        );
        let seen: Option<u64> = admin
            .query_first(format!("SELECT COUNT(*) FROM {table} WHERE n = {n}"))
            .await
            .unwrap();
        assert_eq!(
            seen,
            Some(1),
            "{failing}: the INSERT before it is committed"
        );
    }
    conn.close().await.unwrap();
    admin
        .query_drop(format!("DROP TABLE {table}"))
        .await
        .unwrap();
}

/// 진단 문장은 사람의 앞 문장이 남긴 경고 · 오류를 보인다 — 드라이버의 준비 시도(1295)나 상태 읽기가 덮지 않는다. 붙인 문장은
/// 지금 경로로 가서 거절된다.
#[tokio::test]
async fn live_mysql_diagnostics_show_the_person_s_warnings_and_errors() {
    let table = unique("pw_live_diag");
    let (mut conn, _) = open(SslMode::Disable).await;
    let code = |rows: &[pw_db_types::Row]| rows.first().map(|r| r[1].clone());

    scalar(conn.as_mut(), "SELECT CAST('1x' AS SIGNED)").await;
    let warnings = rows(conn.as_mut(), "SHOW WARNINGS", RiskGrade::Read).await;
    assert_eq!(code(&warnings), Some(DbValue::int(1292)), "{warnings:?}");
    let count = scalar(conn.as_mut(), "show count(*) warnings;").await;
    assert_eq!(count, DbValue::int(1));

    let typo = conn
        .execute(ExecuteRequest::new("SELEC 1", RiskGrade::Read))
        .await
        .unwrap_err();
    assert_eq!(typo.code.as_deref(), Some("1064"));
    let errors = rows(conn.as_mut(), "SHOW ERRORS LIMIT 5", RiskGrade::Read).await;
    assert_eq!(code(&errors), Some(DbValue::int(1064)), "{errors:?}");
    write(
        conn.as_mut(),
        "GET DIAGNOSTICS CONDITION 1 @pw_m = MESSAGE_TEXT, @pw_e = MYSQL_ERRNO",
    )
    .await;
    assert_eq!(
        scalar(conn.as_mut(), "SELECT @pw_e").await,
        DbValue::int(1064)
    );

    // 트랜잭션 안의 오류 뒤에도(드라이버가 상태를 다시 읽는다) 사람의 오류가 남는다.
    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(ExecuteRequest::new("SELEC 2", RiskGrade::Read))
        .await
        .unwrap_err();
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    let errors = rows(conn.as_mut(), "SHOW ERRORS", RiskGrade::Read).await;
    assert_eq!(code(&errors), Some(DbValue::int(1064)), "{errors:?}");
    conn.rollback().await.unwrap();

    write(conn.as_mut(), &format!("CREATE TABLE {table} (n INT)")).await;
    let glued = conn
        .execute(ExecuteRequest::new(
            format!("SHOW WARNINGS; DROP TABLE {table}"),
            RiskGrade::Read,
        ))
        .await
        .unwrap_err();
    assert_eq!(glued.code.as_deref(), Some("1064"), "{glued}");
    write(conn.as_mut(), &format!("DROP TABLE {table}")).await;
    conn.close().await.unwrap();
}
