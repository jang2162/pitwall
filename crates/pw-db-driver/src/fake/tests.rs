use std::time::Duration;

use pw_db_types::{DbErrorKind, DbInt, ObjectKind};

use super::*;
use crate::cancel::QueryCancel;

async fn sql(driver: &FakeDriver) -> Box<dyn SqlConnection> {
    match driver.connect(FakeDriver::spec()).await.unwrap().connection {
        DbConnection::Sql(conn) => conn,
        DbConnection::Kv(_) => panic!("the fake is an sql driver"),
    }
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

    let started = conn
        .execute(ExecuteRequest::sql("SELECT * FROM big"))
        .await
        .unwrap();
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
    conn.execute(ExecuteRequest::sql("SELECT * FROM slow"))
        .await
        .unwrap();
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
    let again = conn.execute(ExecuteRequest::sql("SELECT * FROM big")).await;
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
    conn.execute(ExecuteRequest::sql("INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    conn.execute(ExecuteRequest::sql("SELECT * FROM t"))
        .await
        .unwrap();
    let batch = conn.fetch(FetchLimit::FIRST_SCREEN).await.unwrap();
    assert_eq!(batch.rows.len(), 500);
    // 더 달라고 하지 않으면 더 읽지 않는다.
    assert_eq!(driver.rows_read(), 500);
    conn.discard_cursor().await.unwrap();
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
    conn.execute(ExecuteRequest::sql("INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    conn.savepoint("pw_edit_1".into()).await.unwrap();
    conn.execute(ExecuteRequest::sql("UPDATE t SET a = 2"))
        .await
        .unwrap();
    conn.rollback_to_savepoint("pw_edit_1".into())
        .await
        .unwrap();
    conn.commit().await.unwrap();
    assert_eq!(driver.committed(), ["INSERT INTO t VALUES (1)"]);

    // 트랜잭션 밖의 쓰기는 곧바로 커밋된다. 롤백한 트랜잭션의 쓰기는 버린다.
    conn.execute(ExecuteRequest::sql("UPDATE t SET a = 3"))
        .await
        .unwrap();
    conn.begin(BeginOptions { read_only: true }).await.unwrap();
    conn.execute(ExecuteRequest::sql("INSERT INTO t VALUES (9)"))
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
        conn.execute(ExecuteRequest::sql("SELECT * FROM flaky"))
            .await?;
        conn.fetch(FetchLimit::rows(10)).await
    }
    .await;
    assert_eq!(flaky.unwrap_err().kind, DbErrorKind::Disconnected);
    assert_eq!(driver.rows_read(), 3);

    conn.begin(BeginOptions::default()).await.unwrap();
    conn.execute(ExecuteRequest::sql("INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    assert!(conn.execute(ExecuteRequest::sql("BAD")).await.is_err());
    assert_eq!(conn.transaction_state(), TransactionState::Failed);
    let blocked = conn
        .execute(ExecuteRequest::sql("INSERT INTO t VALUES (2)"))
        .await;
    assert_eq!(blocked.unwrap_err().code.as_deref(), Some("25P02"));
    conn.commit().await.unwrap();
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
    conn.execute(ExecuteRequest::sql("SELECT 1")).await.unwrap();
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
}
