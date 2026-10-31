use std::time::Duration;

use pw_db_types::{DbErrorKind, DbInt, ObjectKind, RiskGrade};

use super::*;
use crate::cancel::QueryCancel;

async fn sql(driver: &FakeDriver) -> Box<dyn SqlConnection> {
    match driver.connect(FakeDriver::spec()).await.unwrap().connection {
        DbConnection::Sql(conn) => conn,
        DbConnection::Kv(_) => panic!("the fake is an sql driver"),
    }
}

/// 시험 문장의 요청. 등급은 앞 낱말로(M3-03 의 분류기 대신): `SELECT` 는 조회, `BAD` 는 파괴(파싱 실패), 나머지는 쓰기.
fn stmt(sql: &str) -> ExecuteRequest {
    let grade = if sql.starts_with("SELECT") {
        RiskGrade::Read
    } else if sql.starts_with("BAD") {
        RiskGrade::Destructive
    } else {
        RiskGrade::Write
    };
    ExecuteRequest::new(sql, grade)
}

fn first_int(row: &Row) -> i64 {
    match &row[0] {
        DbValue::Int(DbInt::Number(n)) => *n,
        other => panic!("not a small int: {other:?}"),
    }
}

/// 완료 기준 ②: trait 객체로 1,000행이 순서대로 오고, 취소하면 인터럽트가 1회 기록된다.
#[tokio::test]
async fn a_boxed_connection_streams_1000_rows_in_order_and_one_cancel_interrupts_once() {
    let driver = FakeDriver::new();
    driver.respond("FROM big", FakeResponse::rows(1000));
    driver.respond("FROM slow", FakeResponse::rows(10).hanging());
    let mut conn: Box<dyn SqlConnection> = sql(&driver).await;

    let started = conn.execute(stmt("SELECT * FROM big")).await.unwrap();
    assert_eq!(started.columns.len(), 2);
    assert!(started.end.is_none(), "a row statement opens a cursor");
    let mut seen = Vec::new();
    loop {
        let batch = conn.fetch(FetchLimit::rows(128)).await.unwrap();
        seen.extend(batch.rows.iter().map(first_int));
        if let Some(end) = batch.end {
            assert_eq!(end.rows_affected, Some(1000));
            break;
        }
    }
    assert_eq!(seen, (0..1000).collect::<Vec<_>>());

    // 오래 도는 쿼리를 다른 태스크에서 취소한다. 두 번 취소해도 인터럽트는 한 번이다.
    conn.execute(stmt("SELECT * FROM slow")).await.unwrap();
    let cancel = QueryCancel::new(conn.interrupter(), Duration::from_secs(5));
    let canceller = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel().await.unwrap();
            cancel.cancel().await.unwrap();
        })
    };
    let result = cancel.run(conn.fetch(FetchLimit::rows(10))).await;
    canceller.await.unwrap();
    assert_eq!(result.unwrap_err().kind, DbErrorKind::Cancelled);
    assert_eq!(driver.count(&FakeCall::Interrupt), 1);
    // 연결은 그대로 쓸 수 있다.
    let again = conn.execute(stmt("SELECT * FROM big")).await;
    assert!(again.is_ok());
    assert_eq!(driver.count(&FakeCall::Close), 0);
}

#[tokio::test]
async fn the_cursor_is_read_only_as_far_as_fetched_and_discarding_keeps_the_transaction() {
    let driver = FakeDriver::new();
    driver.respond("SELECT", FakeResponse::rows(100_000));
    driver.respond("INSERT", FakeResponse::write(1));
    let mut conn = sql(&driver).await;

    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(stmt("INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    conn.execute(stmt("SELECT * FROM t")).await.unwrap();
    let batch = conn.fetch(FetchLimit::FIRST_SCREEN).await.unwrap();
    assert_eq!(batch.rows.len(), 500);
    // 더 달라고 하지 않으면 더 읽지 않는다.
    assert_eq!(driver.rows_read(), 500);
    conn.discard_cursor().await.unwrap();
    assert!(
        driver
            .calls()
            .contains(&FakeCall::DiscardCursor { drained: false })
    );
    assert_eq!(
        driver.rows_read(),
        500,
        "a read cursor is stopped, not drained"
    );
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    assert!(driver.committed().is_empty(), "nothing is committed yet");
    let after = conn.fetch(FetchLimit::rows(1)).await;
    assert_eq!(after.unwrap_err().kind, DbErrorKind::Invalid);
    conn.commit().await.unwrap();
    assert_eq!(driver.committed(), ["INSERT INTO t VALUES (1)"]);
    assert_eq!(driver.count(&FakeCall::Close), 0);
}

#[tokio::test]
async fn rolling_back_to_a_savepoint_keeps_the_earlier_writes() {
    let driver = FakeDriver::new();
    driver.respond("INSERT", FakeResponse::write(1));
    driver.respond("UPDATE", FakeResponse::write(1));
    let mut conn = sql(&driver).await;

    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(stmt("INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    conn.savepoint("pw_edit_1".into()).await.unwrap();
    conn.execute(stmt("UPDATE t SET a = 2")).await.unwrap();
    conn.rollback_to_savepoint("pw_edit_1".into())
        .await
        .unwrap();
    conn.commit().await.unwrap();
    assert_eq!(driver.committed(), ["INSERT INTO t VALUES (1)"]);

    // 트랜잭션 밖의 쓰기는 곧바로 커밋된다. 롤백한 트랜잭션의 쓰기는 버린다.
    conn.execute(stmt("UPDATE t SET a = 3")).await.unwrap();
    conn.begin(BeginOptions { read_only: true }).await.unwrap();
    conn.execute(stmt("INSERT INTO t VALUES (9)"))
        .await
        .unwrap();
    conn.rollback().await.unwrap();
    assert_eq!(
        driver.committed(),
        ["INSERT INTO t VALUES (1)", "UPDATE t SET a = 3"]
    );
    assert!(
        driver
            .calls()
            .contains(&FakeCall::Begin { read_only: true })
    );
}

#[tokio::test]
async fn errors_fail_the_transaction_until_rollback() {
    let driver = FakeDriver::new();
    driver.respond(
        "BAD",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "syntax")),
    );
    driver.respond("INSERT", FakeResponse::write(1));
    driver.respond(
        "FROM flaky",
        FakeResponse::rows(10).failing_after(3, DbError::new(DbErrorKind::Disconnected, "gone")),
    );
    let mut conn = sql(&driver).await;

    let flaky = async {
        conn.execute(stmt("SELECT * FROM flaky")).await?;
        conn.fetch(FetchLimit::rows(10)).await
    }
    .await;
    assert_eq!(flaky.unwrap_err().kind, DbErrorKind::Disconnected);
    assert_eq!(driver.rows_read(), 3);

    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(stmt("INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    assert!(conn.execute(stmt("BAD")).await.is_err());
    assert_eq!(conn.transaction_state(), TransactionState::Failed);
    let blocked = conn.execute(stmt("INSERT INTO t VALUES (2)")).await;
    assert_eq!(blocked.unwrap_err().code.as_deref(), Some("25P02"));
    // 실패한 트랜잭션의 commit 은 끝내되 롤백을 오류(`rolled_back`)로 알린다(trait 의 약속).
    let failed = conn.commit().await.unwrap_err();
    assert!(failed.rolled_back, "{failed:?}");
    assert_eq!(failed.code.as_deref(), Some("25P02"));
    assert!(
        driver.committed().is_empty(),
        "a failed transaction commits nothing"
    );
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
}

#[tokio::test]
async fn a_delayed_fetch_returns_at_once_on_interrupt_and_connect_can_fail() {
    let driver = FakeDriver::new();
    driver.respond(
        "SELECT",
        FakeResponse::rows(5).with_delay(Duration::from_secs(60)),
    );
    let mut conn = sql(&driver).await;
    conn.execute(stmt("SELECT 1")).await.unwrap();
    let interrupter = conn.interrupter();
    let started = std::time::Instant::now();
    let (result, ()) = tokio::join!(conn.fetch(FetchLimit::rows(5)), async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        interrupter.interrupt().await.unwrap();
    });
    assert_eq!(result.unwrap_err().kind, DbErrorKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));

    driver.fail_connect(DbError::new(
        DbErrorKind::Auth,
        "password authentication failed",
    ));
    let refused = driver.connect(FakeDriver::spec()).await;
    assert_eq!(refused.err().unwrap().kind, DbErrorKind::Auth);
    assert_eq!(driver.count(&FakeCall::Connect), 2);
}

#[tokio::test]
async fn objects_are_paged_and_filtered() {
    let driver = FakeDriver::new();
    driver.set_objects(
        (0..1000)
            .map(|i| ObjectInfo {
                name: format!("table_{i:04}"),
                kind: ObjectKind::Table,
                comment: None,
            })
            .collect(),
    );
    let mut conn = sql(&driver).await;
    let query = |cursor: Option<String>, filter: Option<&str>| ObjectQuery {
        database: None,
        schema: Some("public".into()),
        kind: ObjectKind::Table,
        filter: filter.map(str::to_owned),
        cursor,
        limit: 200,
    };
    let first = conn.objects(query(None, None)).await.unwrap();
    assert_eq!(first.items.len(), 200);
    assert_eq!(first.next_cursor.as_deref(), Some("200"));
    let second = conn.objects(query(first.next_cursor, None)).await.unwrap();
    assert_eq!(second.items[0].name, "table_0200");
    let filtered = conn.objects(query(None, Some("TABLE_099"))).await.unwrap();
    assert_eq!(filtered.items.len(), 10);
    assert_eq!(filtered.next_cursor, None);
    assert_eq!(driver.count(&FakeCall::Metadata("objects")), 3);
}

#[test]
fn capabilities_name_exactly_one_branch() {
    assert_eq!(FakeDriver::full_capabilities().validate(), Ok(()));
    // 둘 다인 갈래: KV 에 없는 기능을 모두 꺼서 갈래 검사만 걸리게 한다.
    let both = Capabilities {
        kv: true,
        transactions: false,
        savepoints: false,
        read_only_transaction: false,
        explain: false,
        schemas: false,
        ..FakeDriver::full_capabilities()
    };
    assert_eq!(
        both.validate(),
        Err("a driver is either sql or kv, not both")
    );
    let kv_with_tx = Capabilities {
        sql: false,
        kv: true,
        ..FakeDriver::full_capabilities()
    };
    assert!(kv_with_tx.validate().is_err());
    let neither = Capabilities {
        sql: false,
        ..FakeDriver::full_capabilities()
    };
    assert!(neither.validate().is_err());

    // 서버 커서 · 메타데이터도 SQL 갈래의 기능이다(리뷰 L7).
    let kv = Capabilities {
        sql: false,
        kv: true,
        server_cursor: false,
        cancel: true,
        transactions: false,
        savepoints: false,
        read_only_transaction: false,
        explain: false,
        metadata: false,
        schemas: false,
        multiple_databases: true,
        ssl_modes: Vec::new(),
    };
    assert_eq!(kv.validate(), Ok(()));
    for broken in [
        Capabilities {
            server_cursor: true,
            ..kv.clone()
        },
        Capabilities {
            metadata: true,
            ..kv.clone()
        },
    ] {
        assert!(broken.validate().is_err(), "{broken:?}");
    }
}

/// 리뷰 H1: 커서 버리기는 등급을 따른다. 조회가 아닌 문장(`INSERT … RETURNING`)을 상한에서 끊어도 쓰기는 남는다 —
/// 수동 커밋 탭의 사람이 커밋하면 그 쓰기가 들어간다.
#[tokio::test]
async fn discarding_a_writing_cursor_drains_it_and_keeps_the_write() {
    let driver = FakeDriver::new();
    driver.respond("RETURNING", FakeResponse::rows(100_000));
    let mut conn = sql(&driver).await;

    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(stmt(
        "INSERT INTO t SELECT g FROM generate_series(1, 100000) g RETURNING id",
    ))
    .await
    .unwrap();
    let first = conn.fetch(FetchLimit::FIRST_SCREEN).await.unwrap();
    assert_eq!(first.rows.len(), 500);
    conn.discard_cursor().await.unwrap();
    assert!(
        driver
            .calls()
            .contains(&FakeCall::DiscardCursor { drained: true }),
        "{:?}",
        driver.calls()
    );
    assert_eq!(
        driver.rows_read(),
        100_000,
        "the rest was read and thrown away"
    );
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    conn.commit().await.unwrap();
    assert_eq!(
        driver.committed(),
        ["INSERT INTO t SELECT g FROM generate_series(1, 100000) g RETURNING id"]
    );
}

/// 리뷰 M1: 트랜잭션 · 인터럽트는 연결마다 따로다(세션 둘 · 메타데이터용 연결을 한 가짜에서 연다).
#[tokio::test]
async fn connections_of_one_fake_keep_their_own_transaction_and_interrupt() {
    let driver = FakeDriver::new();
    driver.respond("INSERT", FakeResponse::write(1));
    driver.respond("FROM slow", FakeResponse::rows(10).hanging());
    // 지연이 있어야 fetch 가 인터럽트를 듣는 자리(기다림)를 지난다 — 남은 표식이 있으면 거기서 걸린다.
    driver.respond(
        "FROM quick",
        FakeResponse::rows(5).with_delay(Duration::from_millis(10)),
    );
    let mut a = sql(&driver).await;
    let mut b = sql(&driver).await;

    a.begin(BeginOptions::default()).await.unwrap();
    assert_eq!(b.transaction_state(), TransactionState::Idle);
    b.begin(BeginOptions::default()).await.unwrap();
    a.execute(stmt("INSERT INTO t VALUES (1)")).await.unwrap();
    b.commit().await.unwrap();
    assert!(
        driver.committed().is_empty(),
        "b's commit does not commit a's write"
    );
    assert_eq!(a.transaction_state(), TransactionState::InTransaction);
    a.commit().await.unwrap();
    assert_eq!(driver.committed(), ["INSERT INTO t VALUES (1)"]);
    assert_eq!(
        driver.calls_of(1),
        [
            FakeCall::Connect,
            FakeCall::Begin { read_only: false },
            FakeCall::Commit
        ]
    );

    // a 의 인터럽트는 b 의 멈춘 fetch 를 멈추지 않는다. b 의 것은 멈춘다.
    b.execute(stmt("SELECT * FROM slow")).await.unwrap();
    let a_interrupt = a.interrupter();
    let b_interrupt = b.interrupter();
    let (result, ()) = tokio::join!(b.fetch(FetchLimit::rows(10)), async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        a_interrupt.interrupt().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(driver.count(&FakeCall::Interrupt), 1);
        b_interrupt.interrupt().await.unwrap();
    });
    assert_eq!(result.unwrap_err().kind, DbErrorKind::Cancelled);

    // 도는 fetch 가 없을 때의 인터럽트는 아무것도 하지 않는다 — 다음 fetch 는 그대로 된다.
    a.execute(stmt("SELECT * FROM quick")).await.unwrap();
    a.interrupter().interrupt().await.unwrap();
    assert_eq!(a.fetch(FetchLimit::rows(10)).await.unwrap().rows.len(), 5);
}

/// 문장이 트랜잭션을 저절로 끝내거나 여는 흉내(`FakeEnding`)와 SQLite 식 취소 — 파이프라인(M3-05)이 세션 상태로 옮기는 드라이버 보고.
#[tokio::test]
async fn endings_report_implicit_commits_whole_rollbacks_and_hidden_blocks() {
    let driver = FakeDriver::named("mysql", SqlDialect::Mysql);
    assert_eq!(driver.id(), "mysql");
    assert_eq!(driver.dialect(), Some(SqlDialect::Mysql));
    driver.respond("INSERT", FakeResponse::write(1));
    driver.respond(
        "CREATE",
        FakeResponse::write(0).ending(FakeEnding::ImplicitCommit),
    );
    driver.respond(
        "DEADLOCK",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "deadlock").with_code("1213"))
            .ending(FakeEnding::RollBack),
    );
    driver.respond(
        "HIDDEN",
        FakeResponse::write(1).ending(FakeEnding::OpenBlock),
    );
    let mut conn = sql(&driver).await;

    // 암묵 커밋: 앞의 쓰기가 커밋되고 세션은 idle 이다.
    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(stmt("INSERT a")).await.unwrap();
    conn.execute(stmt("CREATE TABLE t (id int)")).await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert_eq!(driver.committed(), ["INSERT a", "CREATE TABLE t (id int)"]);

    // 전체 롤백: 오류가 `rolled_back` 을 싣고, 쓰기는 사라진다.
    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(stmt("INSERT b")).await.unwrap();
    let error = conn.execute(stmt("UPDATE DEADLOCK")).await.unwrap_err();
    assert!(error.rolled_back, "{error:?}");
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert!(!driver.committed().contains(&"INSERT b".to_owned()));

    // 알리지 않는 전체 롤백: 오류에는 `rolled_back` 이 없고 상태만 idle 이다.
    driver.respond(
        "QUIET",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "plugin rollback").with_code("3101"))
            .ending(FakeEnding::RollBackSilently),
    );
    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(stmt("INSERT q")).await.unwrap();
    let error = conn.execute(stmt("UPDATE QUIET")).await.unwrap_err();
    assert!(!error.rolled_back);
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert!(!driver.committed().contains(&"INSERT q".to_owned()));

    // 숨은 블록: 자동 커밋 문장 뒤 연결이 트랜잭션 안이다 — 쓰기는 커밋 전까지 남지 않는다.
    conn.execute(stmt("INSERT HIDDEN")).await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    assert!(!driver.committed().contains(&"INSERT HIDDEN".to_owned()));
    conn.commit().await.unwrap();
    assert!(driver.committed().contains(&"INSERT HIDDEN".to_owned()));

    // SQLite 식 취소: 트랜잭션 안의 문장을 멈추면 전체가 롤백된다.
    driver.interrupt_rolls_back(true);
    driver.respond("SLOW", FakeResponse::rows(5).hanging());
    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(stmt("INSERT c")).await.unwrap();
    conn.execute(stmt("UPDATE SLOW RETURNING *")).await.unwrap();
    let interrupt = conn.interrupter();
    let stop = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        interrupt.interrupt().await.unwrap();
    });
    let error = conn.fetch(FetchLimit::rows(5)).await.unwrap_err();
    stop.await.unwrap();
    assert_eq!(error.kind, DbErrorKind::Cancelled);
    assert!(error.rolled_back, "{error:?}");
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert!(!driver.committed().contains(&"INSERT c".to_owned()));
}
