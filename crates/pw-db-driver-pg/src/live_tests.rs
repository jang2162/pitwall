//! 실 PG 시험(14-m3-plan.md M3-06 의 완료 기준 ① ~ ⑦). `PW_TEST_PG_URL` 이 있을 때만 목록에 생긴다(`pw_db_driver::live`).
//!
//! TLS 시험은 시험 CA 로 서명한 두 번째 서버를 쓴다 — `scripts/db-tls.mjs` 가 띄우고 `PW_TEST_PG_TLS_URL` · `PW_TEST_TLS_CA` 를
//! 알린다(CI 는 `db-live` 잡의 단계, 로컬은 `mise run test-db` 가 도커로 띄운다). 없으면 그 시험은 실패한다 — 건너뛰지 않는다.
//!
//! 시험은 프로세스마다 따로 돌고(nextest) 같은 DB 를 함께 쓴다 — 테이블 · 스키마 이름은 시험마다 다르게 짓는다.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_db_driver::live::{LiveDb, wait_ready};
use pw_db_driver::{DbDriver, Interrupt, SqlConnection};
use pw_db_types::{
    BeginOptions, CellLimit, ConnectionSpec, DbErrorKind, DbValue, DriverKind, ExecuteRequest,
    FetchLimit, ObjectKind, ObjectQuery, ObjectRef, RiskGrade, Row, Secret, SocketTarget, SslMode,
    Target, TlsSettings, TlsStatus, TransactionState,
};

use crate::{PgConnection, PgDriver};

/// `postgres://user:password@host:port/db` → 설정. 시험 URL 만 읽는다(퍼센트 인코딩 없음).
fn spec_from(url: &str) -> ConnectionSpec {
    let endpoint = pw_db_driver::live::endpoint(LiveDb::Pg, url).unwrap();
    let rest = url.split_once("://").unwrap().1;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let user = authority
        .rsplit_once('@')
        .map(|(userinfo, _)| userinfo.split(':').next().unwrap().to_owned());
    let mut spec = ConnectionSpec::new(
        DriverKind::Postgres.id(),
        Target::Network {
            host: endpoint.host,
            port: endpoint.port,
            connect_to: None,
        },
    );
    spec.user = user;
    spec.password = endpoint.password.map(Secret::new);
    let database = path.split('?').next().unwrap_or_default();
    spec.database = (!database.is_empty()).then(|| database.to_owned());
    spec.tls = TlsSettings::mode(SslMode::Disable);
    spec
}

fn plain() -> ConnectionSpec {
    spec_from(&LiveDb::Pg.url())
}

fn tls_url() -> String {
    std::env::var("PW_TEST_PG_TLS_URL").expect(
        "PW_TEST_PG_TLS_URL is not set — start the TLS server with `node scripts/db-tls.mjs pg` (CONTRIBUTING.md 「명령」)",
    )
}

fn tls_ca() -> std::path::PathBuf {
    std::env::var_os("PW_TEST_TLS_CA")
        .expect("PW_TEST_TLS_CA is not set — `node scripts/db-tls.mjs pg` prints it")
        .into()
}

/// TLS 서버의 설정. 서버가 뜨기를 기다린다(CI 의 `docker run` 단계 바로 뒤일 수 있다).
fn tls_spec(mode: SslMode) -> ConnectionSpec {
    let url = tls_url();
    // 준비 확인은 이름의 첫 주소만 본다 — `localhost` 가 `::1` 로 풀리면 IPv4 에만 연 컨테이너를 못 본다.
    wait_ready(
        LiveDb::Pg,
        &url.replace("@localhost:", "@127.0.0.1:"),
        Duration::from_secs(120),
    )
    .unwrap();
    let mut spec = spec_from(&url);
    spec.tls = TlsSettings::mode(mode);
    spec
}

async fn open(spec: &ConnectionSpec) -> PgConnection {
    PgDriver::new().open(spec).await.unwrap().0
}

fn read(sql: &str) -> ExecuteRequest {
    ExecuteRequest::new(sql, RiskGrade::Read)
}

fn write(sql: &str) -> ExecuteRequest {
    ExecuteRequest::new(sql, RiskGrade::Write)
}

/// 문장을 끝까지 돌려 행을 모은다.
async fn all(conn: &mut PgConnection, request: ExecuteRequest) -> Vec<Row> {
    let started = conn.execute(request).await.unwrap();
    let mut rows = Vec::new();
    if started.end.is_some() {
        return rows;
    }
    loop {
        let batch = conn.fetch(FetchLimit::rows(10_000)).await.unwrap();
        rows.extend(batch.rows);
        if batch.end.is_some() {
            return rows;
        }
    }
}

async fn run(conn: &mut PgConnection, sql: &str) {
    all(conn, write(sql)).await;
}

async fn one(conn: &mut PgConnection, sql: &str) -> DbValue {
    let rows = all(conn, read(sql)).await;
    assert_eq!(rows.len(), 1, "{sql}");
    rows[0][0].clone()
}

fn int(value: &DbValue) -> i64 {
    match value {
        DbValue::Int(pw_db_types::DbInt::Number(n)) => *n,
        other => panic!("not a small integer: {other:?}"),
    }
}

fn text(value: &DbValue) -> &str {
    match value {
        DbValue::Text(t) => t,
        other => panic!("not text: {other:?}"),
    }
}

async fn backend_pid(conn: &mut PgConnection) -> i64 {
    int(&one(conn, "SELECT pg_backend_pid()").await)
}

/// 다른 연결에서 본 그 백엔드의 상태(`pg_stat_activity.state`).
async fn backend_state(observer: &mut PgConnection, pid: i64) -> String {
    let mut request = read("SELECT state FROM pg_stat_activity WHERE pid = $1");
    request.params = vec![DbValue::int(pid)];
    let rows = all(observer, request).await;
    text(&rows[0][0]).to_owned()
}

async fn count(conn: &mut PgConnection, sql: &str) -> i64 {
    int(&one(conn, sql).await)
}

/// ① 포털: 백만 행 문장을 1,000행 credit 으로 읽다 멈추면 서버는 나머지를 보내지 않고 기다린다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_a_portal_waits_for_credit() {
    let mut conn = open(&plain()).await;
    let mut observer = open(&plain()).await;
    let pid = backend_pid(&mut conn).await;

    let started = conn
        .execute(read("SELECT generate_series(1, 1000000) AS n"))
        .await
        .unwrap();
    assert_eq!(started.columns[0].db_type, "int4");
    let first = conn.fetch(FetchLimit::rows(1000)).await.unwrap();
    assert_eq!(first.rows.len(), 1000);
    assert!(first.end.is_none());
    tokio::time::sleep(Duration::from_millis(300)).await;
    // 서버는 그 문장을 「돌리는 중」 이 아니라 다음 Execute 를 기다린다.
    assert_eq!(
        backend_state(&mut observer, pid).await,
        "idle in transaction"
    );
    let activity = {
        let mut request = read("SELECT query FROM pg_stat_activity WHERE pid = $1");
        request.params = vec![DbValue::int(pid)];
        all(&mut observer, request).await
    };
    assert!(text(&activity[0][0]).contains("generate_series"));
    // 같은 포털에서 이어 읽는다(다시 실행하지 않는다).
    let next = conn.fetch(FetchLimit::rows(1000)).await.unwrap();
    assert_eq!(next.rows[0][0], DbValue::int(1001));
    assert_eq!(conn.transaction_state(), TransactionState::Idle);

    conn.discard_cursor().await.unwrap();
    // 자동 커밋 탭은 커서를 버리면 포털의 트랜잭션을 끝낸다.
    assert_eq!(backend_state(&mut observer, pid).await, "idle");
    assert_eq!(count(&mut conn, "SELECT 1").await, 1);

    // 서버가 실제로 만든 행 수: 시퀀스는 트랜잭션과 상관없이 다른 연결에 보인다. credit 만큼만 만들고, 조회의 커서를 버리면 멈춘다.
    run(&mut conn, "DROP SEQUENCE IF EXISTS pw_live_portal_seq").await;
    run(&mut conn, "CREATE SEQUENCE pw_live_portal_seq").await;
    conn.execute(read(
        "SELECT nextval('pw_live_portal_seq') FROM generate_series(1, 1000000)",
    ))
    .await
    .unwrap();
    assert_eq!(
        conn.fetch(FetchLimit::rows(1000)).await.unwrap().rows.len(),
        1000
    );
    let produced = count(&mut observer, "SELECT last_value FROM pw_live_portal_seq").await;
    assert!(
        produced <= 1001,
        "the server produced {produced} rows for 1,000 credits"
    );
    conn.discard_cursor().await.unwrap();
    let produced = count(&mut observer, "SELECT last_value FROM pw_live_portal_seq").await;
    assert!(
        produced <= 1001,
        "discarding a read let the server run on to {produced}"
    );
}

/// ② 수동 커밋 세션에서 쓰기 → 상한 + 1 행 조회를 상한에서 끊기 → 커밋하면 쓰기가 남는다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_a_manual_transaction_survives_the_row_limit() {
    let mut conn = open(&plain()).await;
    let mut other = open(&plain()).await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_manual_limit").await;
    run(&mut conn, "CREATE TABLE pw_live_manual_limit (id int)").await;

    conn.begin(BeginOptions::default()).await.unwrap();
    run(&mut conn, "INSERT INTO pw_live_manual_limit VALUES (42)").await;
    // 그리드 보유 행 상한(기본 100,000)까지 읽고 커서를 버린다 — 파이프라인(M3-05)이 하는 그대로.
    conn.execute(read("SELECT generate_series(1, 100001)"))
        .await
        .unwrap();
    let mut held = 0;
    let mut limit = FetchLimit::FIRST_SCREEN;
    while held < 100_000 {
        let batch = conn.fetch(limit).await.unwrap();
        assert!(batch.end.is_none());
        held += batch.rows.len();
        limit = FetchLimit::rows(u32::try_from(100_000 - held).unwrap().min(10_000));
    }
    assert_eq!(held, 100_000);
    conn.discard_cursor().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    assert_eq!(
        count(&mut other, "SELECT count(*) FROM pw_live_manual_limit").await,
        0
    );

    conn.commit().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert_eq!(
        count(
            &mut other,
            "SELECT count(*) FROM pw_live_manual_limit WHERE id = 42"
        )
        .await,
        1
    );
}

/// 자동 커밋의 `INSERT … RETURNING` 은 상한에서 끊겨도 쓰기가 남는다(포털을 닫고 커밋한다).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_an_autocommit_returning_write_survives_the_row_limit() {
    let mut conn = open(&plain()).await;
    let mut other = open(&plain()).await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_returning").await;
    run(&mut conn, "CREATE TABLE pw_live_returning (id int)").await;

    conn.execute(write(
        "INSERT INTO pw_live_returning SELECT generate_series(1, 5000) RETURNING id",
    ))
    .await
    .unwrap();
    let batch = conn.fetch(FetchLimit::rows(1000)).await.unwrap();
    assert_eq!(batch.rows.len(), 1000);
    conn.discard_cursor().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert_eq!(
        count(&mut other, "SELECT count(*) FROM pw_live_returning").await,
        5000
    );

    // 다 읽은 쓰기는 바뀐 행 수를 싣는다. 행을 돌려주지 않는 문장은 트랜잭션 없이 돈다 — `VACUUM` 이 된다.
    conn.execute(write(
        "DELETE FROM pw_live_returning WHERE id <= 10 RETURNING id",
    ))
    .await
    .unwrap();
    let rest = conn.fetch(FetchLimit::rows(100)).await.unwrap();
    assert_eq!(rest.rows.len(), 10);
    assert_eq!(rest.end.unwrap().rows_affected, Some(10));
    let vacuum = conn
        .execute(write("VACUUM pw_live_returning"))
        .await
        .unwrap();
    assert!(vacuum.end.is_some());

    // 조회가 아닌 문장의 커서를 버리면 멈추지 않고 끝까지 읽어 버린다 — 부작용이 문장 끝까지 돈다.
    run(&mut conn, "DROP SEQUENCE IF EXISTS pw_live_drain_seq").await;
    run(&mut conn, "CREATE SEQUENCE pw_live_drain_seq").await;
    conn.execute(write(
        "SELECT nextval('pw_live_drain_seq') FROM generate_series(1, 5000)",
    ))
    .await
    .unwrap();
    assert_eq!(
        conn.fetch(FetchLimit::rows(1000)).await.unwrap().rows.len(),
        1000
    );
    conn.discard_cursor().await.unwrap();
    assert_eq!(
        count(&mut other, "SELECT last_value FROM pw_live_drain_seq").await,
        5000
    );
}

/// ③ 자동 커밋: `pg_sleep(30)` 을 취소하면 1 s 안에 취소 오류이고 백엔드는 `idle` 이다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_cancel_stops_a_sleep_within_a_second() {
    let mut conn = open(&plain()).await;
    let mut observer = open(&plain()).await;
    let pid = backend_pid(&mut conn).await;
    let (elapsed, error) = sleep_and_cancel(&mut conn).await;
    assert_eq!(error, DbErrorKind::Cancelled);
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert_eq!(backend_state(&mut observer, pid).await, "idle");
    assert_eq!(
        count(&mut conn, "SELECT 1").await,
        1,
        "the connection stays usable"
    );
}

/// ③ 수동 트랜잭션: 취소 뒤 세션은 `failed`(롤백만 된다), 롤백하면 다시 쓴다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_cancel_in_a_manual_transaction_leaves_it_failed() {
    let mut conn = open(&plain()).await;
    let mut observer = open(&plain()).await;
    let pid = backend_pid(&mut conn).await;
    conn.begin(BeginOptions::default()).await.unwrap();
    let (elapsed, error) = sleep_and_cancel(&mut conn).await;
    assert_eq!(error, DbErrorKind::Cancelled);
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    assert_eq!(conn.transaction_state(), TransactionState::Failed);
    assert_eq!(
        backend_state(&mut observer, pid).await,
        "idle in transaction (aborted)"
    );
    let refused = conn.execute(read("SELECT 1")).await;
    assert_eq!(refused.unwrap_err().code.as_deref(), Some("25P02"));
    conn.rollback().await.unwrap();
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert_eq!(count(&mut conn, "SELECT 1").await, 1);
}

/// 서버에 묻는 트랜잭션 상태(`read_transaction_state` — 블록 판정)는 서버의 블록을 따라간다: 밖이면 `Idle`, 안이면
/// `InTransaction`, 오류로 깨진 블록이면 `Failed`. 물어도 트랜잭션은 그대로다(그 뒤 커밋한 쓰기가 남는다).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_reading_the_transaction_state_asks_the_server() {
    let table = format!("pw_live_state_{}", std::process::id());
    let mut conn = open(&plain()).await;
    run(&mut conn, &format!("CREATE TABLE {table} (n int)")).await;
    assert_eq!(
        conn.read_transaction_state().await.unwrap(),
        TransactionState::Idle
    );
    conn.begin(BeginOptions::default()).await.unwrap();
    run(&mut conn, &format!("INSERT INTO {table} VALUES (1)")).await;
    assert_eq!(
        conn.read_transaction_state().await.unwrap(),
        TransactionState::InTransaction
    );
    conn.commit().await.unwrap();
    assert_eq!(
        count(&mut conn, &format!("SELECT count(*) FROM {table}")).await,
        1
    );
    conn.begin(BeginOptions::default()).await.unwrap();
    let failed = conn.execute(read("SELECT 1 / 0")).await;
    let failed = match failed {
        Err(e) => e,
        Ok(_) => conn.fetch(FetchLimit::rows(1)).await.unwrap_err(),
    };
    assert_eq!(failed.code.as_deref(), Some("22012"));
    assert_eq!(
        conn.read_transaction_state().await.unwrap(),
        TransactionState::Failed
    );
    conn.rollback().await.unwrap();
    assert_eq!(
        conn.read_transaction_state().await.unwrap(),
        TransactionState::Idle
    );
    run(&mut conn, &format!("DROP TABLE {table}")).await;
}

/// `pg_sleep(30)` 을 시작하고 0.3 s 뒤 인터럽트. (인터럽트에서 오류까지의 시간, 오류 종류).
async fn sleep_and_cancel(conn: &mut PgConnection) -> (Duration, DbErrorKind) {
    conn.execute(read("SELECT pg_sleep(30)")).await.unwrap();
    let interrupter: Arc<dyn Interrupt> = conn.interrupter();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let at = Instant::now();
        interrupter.interrupt().await.unwrap();
        at
    });
    let error = conn.fetch(FetchLimit::rows(1)).await.unwrap_err();
    let ended = Instant::now();
    let at = canceller.await.unwrap();
    (ended.duration_since(at), error.kind)
}

/// ④ 읽기 전용 트랜잭션은 쓰기 · `setval` · `FOR UPDATE` 를 DB 오류 25006 으로 거절한다. `pg_terminate_backend` 는 PG 가 막지
/// 않는다 — 그것을 막는 것은 분류(M3-03)와 관문(M3-05)이다(이 시험이 그 사실을 고정한다).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_a_read_only_transaction_refuses_writes() {
    let mut conn = open(&plain()).await;
    let mut victim = open(&plain()).await;
    let mut observer = open(&plain()).await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_read_only").await;
    run(&mut conn, "CREATE TABLE pw_live_read_only (id int)").await;
    run(&mut conn, "DROP SEQUENCE IF EXISTS pw_live_read_only_seq").await;
    run(&mut conn, "CREATE SEQUENCE pw_live_read_only_seq").await;

    for (sql, grade) in [
        ("INSERT INTO pw_live_read_only VALUES (1)", RiskGrade::Write),
        (
            "SELECT setval('pw_live_read_only_seq', 5)",
            RiskGrade::Write,
        ),
        (
            "SELECT id FROM pw_live_read_only FOR UPDATE",
            RiskGrade::Write,
        ),
    ] {
        conn.begin(BeginOptions { read_only: true }).await.unwrap();
        let error = match conn.execute(ExecuteRequest::new(sql, grade)).await {
            Err(e) => e,
            Ok(started) if started.end.is_some() => panic!("{sql} ran in a read-only transaction"),
            Ok(_) => conn.fetch(FetchLimit::rows(10)).await.unwrap_err(),
        };
        assert_eq!(error.kind, DbErrorKind::ReadOnly, "{sql}: {error:?}");
        assert_eq!(error.code.as_deref(), Some("25006"), "{sql}");
        assert_eq!(conn.transaction_state(), TransactionState::Failed);
        conn.rollback().await.unwrap();
    }
    assert_eq!(
        count(&mut conn, "SELECT count(*) FROM pw_live_read_only").await,
        0
    );
    assert_eq!(
        count(&mut conn, "SELECT last_value FROM pw_live_read_only_seq").await,
        1
    );

    // 첫 문장의 `SET` 은 읽기 전용 트랜잭션을 푼다(스냅숏 전이라 PG 가 허락한다) — 드라이버는 막지 않는다. read-only 연결의
    // `SET` · `RESET` · `SET TRANSACTION` 은 관문(M3-05)이 거절한다(결정 M3-D6, 리뷰 M2). 이 시험은 그 사실을 고정한다.
    for lift in [
        "SET transaction_read_only = off",
        "SET TRANSACTION READ WRITE",
    ] {
        conn.begin(BeginOptions { read_only: true }).await.unwrap();
        run(&mut conn, lift).await;
        run(&mut conn, "INSERT INTO pw_live_read_only VALUES (9)").await;
        conn.rollback().await.unwrap();
    }

    let victim_pid = backend_pid(&mut victim).await;
    conn.begin(BeginOptions { read_only: true }).await.unwrap();
    let terminated = one(
        &mut conn,
        &format!("SELECT pg_terminate_backend({victim_pid})"),
    )
    .await;
    conn.rollback().await.unwrap();
    assert_eq!(
        terminated,
        DbValue::Bool(true),
        "PG does not refuse pg_terminate_backend in a read-only transaction"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut request = read("SELECT count(*) FROM pg_stat_activity WHERE pid = $1");
        request.params = vec![DbValue::int(victim_pid)];
        if int(&all(&mut observer, request).await[0][0]) == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the terminated backend is still there"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let error = victim.execute(read("SELECT 1")).await.unwrap_err();
    assert_eq!(error.kind, DbErrorKind::Disconnected, "{error:?}");
}

/// `BEGIN READ ONLY` 를 지나는 부작용 함수 표(코디네이터 요청 — M3-03 의 분류가 이것들을 조회로 매기면 안 된다). 결과는 서버 판
/// (CI 의 17.6)에서 본 사실이고 `--no-capture` 로 표를 찍는다. 끊거나 멈추는 대상은 이 시험이 연 연결뿐이다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_side_effect_functions_that_pass_a_read_only_transaction() {
    let mut conn = open(&plain()).await;
    let mut victim = open(&plain()).await;
    run(&mut conn, "DROP SEQUENCE IF EXISTS pw_live_probe_seq").await;
    run(&mut conn, "CREATE SEQUENCE pw_live_probe_seq").await;
    let victim_pid = backend_pid(&mut victim).await;
    let own_pid = backend_pid(&mut conn).await;
    // 서버 상태를 바꾸는 후보(복제 슬롯 · 오리진 · 백업)는 이 시험이 만든 이름만 쓰고 끝나면 치운다(`probe_cleanup`).
    probe_cleanup(&mut conn).await;
    run(
        &mut conn,
        "SELECT pg_create_physical_replication_slot('pw_live_probe_drop')",
    )
    .await;
    let probes = [
        format!("SELECT pg_cancel_backend({victim_pid})"),
        "SELECT pg_reload_conf()".to_owned(),
        "SELECT pg_rotate_logfile()".to_owned(),
        "SELECT pg_advisory_lock(424242)".to_owned(),
        "SELECT set_config('pitwall.probe', 'x', false)".to_owned(),
        "SELECT pg_notify('pitwall_probe', 'x')".to_owned(),
        "SELECT pg_stat_reset()".to_owned(),
        "SELECT pg_switch_wal()".to_owned(),
        "SELECT txid_current()".to_owned(),
        "SELECT lo_create(0)".to_owned(),
        "SELECT nextval('pw_live_probe_seq')".to_owned(),
        "SELECT setval('pw_live_probe_seq', 5)".to_owned(),
        // M3-03 리뷰(H2)의 후보.
        "SELECT pg_create_physical_replication_slot('pw_live_probe_new')".to_owned(),
        "SELECT pg_drop_replication_slot('pw_live_probe_drop')".to_owned(),
        "SELECT pg_create_logical_replication_slot('pw_live_probe_logical', 'pgoutput')".to_owned(),
        "SELECT pg_replication_origin_create('pw_live_probe')".to_owned(),
        "SELECT pg_create_restore_point('pw_live_probe')".to_owned(),
        "SELECT pg_backup_start('pw_live_probe', true)".to_owned(),
        "SELECT pg_wal_replay_pause()".to_owned(),
        "SELECT pg_promote()".to_owned(),
        "SELECT pg_stat_reset_shared('bgwriter')".to_owned(),
        format!("SELECT pg_log_backend_memory_contexts({own_pid})"),
        "SELECT pg_logical_emit_message(true, 'pw', 'x')".to_owned(),
        "SELECT pg_ls_dir('.')".to_owned(),
        "SELECT pg_stat_file('postgresql.conf')".to_owned(),
        "SELECT pg_current_xact_id()".to_owned(),
        "SELECT pg_file_write('/tmp/pw_live_probe', 'y', false)".to_owned(),
    ];
    let mut table = Vec::new();
    for sql in &probes {
        conn.begin(BeginOptions { read_only: true }).await.unwrap();
        let outcome = match conn
            .execute(ExecuteRequest::new(sql.clone(), RiskGrade::Write))
            .await
        {
            Err(e) => e.code.unwrap_or_else(|| format!("{:?}", e.kind)),
            Ok(_) => match conn.fetch(FetchLimit::rows(10)).await {
                Ok(_) => "passes".to_owned(),
                Err(e) => e.code.unwrap_or_else(|| format!("{:?}", e.kind)),
            },
        };
        conn.rollback().await.unwrap();
        table.push((
            sql.split('(')
                .next()
                .unwrap()
                .trim_start_matches("SELECT ")
                .to_owned(),
            outcome,
        ));
    }
    probe_cleanup(&mut conn).await;
    for (function, outcome) in &table {
        println!("{function:40} {outcome}");
    }
    let outcome = |name: &str| table.iter().find(|(f, _)| f == name).unwrap().1.as_str();
    for name in [
        "pg_cancel_backend",
        "pg_reload_conf",
        "pg_rotate_logfile",
        "pg_advisory_lock",
        "set_config",
        "pg_notify",
        "pg_stat_reset",
        "pg_switch_wal",
        "txid_current",
        "pg_create_physical_replication_slot",
        "pg_drop_replication_slot",
        "pg_replication_origin_create",
        "pg_create_restore_point",
        "pg_backup_start",
        "pg_stat_reset_shared",
        "pg_log_backend_memory_contexts",
        "pg_logical_emit_message",
        "pg_ls_dir",
        "pg_stat_file",
        "pg_current_xact_id",
    ] {
        assert_eq!(outcome(name), "passes", "{name}");
    }
    for name in ["lo_create", "nextval", "setval"] {
        assert_eq!(outcome(name), "25006", "{name}");
    }
    // 읽기 전용이 아니라 서버 상태가 막는다: 논리 슬롯은 `wal_level = logical`, 재생 정지 · 승격은 대기 서버에서만(55000).
    for name in [
        "pg_create_logical_replication_slot",
        "pg_wal_replay_pause",
        "pg_promote",
    ] {
        assert_eq!(outcome(name), "55000", "{name}");
    }
    // adminpack 은 PG 17 에 없다.
    assert_eq!(outcome("pg_file_write"), "42883");
    // 치웠다.
    assert_eq!(
        count(
            &mut conn,
            "SELECT count(*) FROM pg_replication_slots WHERE slot_name LIKE 'pw\\_live\\_probe%'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &mut conn,
            "SELECT count(*) FROM pg_replication_origin WHERE roname = 'pw_live_probe'"
        )
        .await,
        0
    );
}

/// 함수 표 시험이 만든 서버 상태를 치운다(없으면 아무것도 하지 않는다).
async fn probe_cleanup(conn: &mut PgConnection) {
    run(
        conn,
        "SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots WHERE slot_name LIKE 'pw\\_live\\_probe%'",
    )
    .await;
    run(
        conn,
        "SELECT pg_replication_origin_drop(roname) FROM pg_replication_origin WHERE roname = 'pw_live_probe'",
    )
    .await;
    // 이 세션에서 시작한 백업이 있으면 끝낸다(없으면 오류 — 무시).
    let _ = all_or_error(conn, "SELECT pg_backup_stop(false)").await;
}

async fn all_or_error(conn: &mut PgConnection, sql: &str) -> Result<(), pw_db_types::DbError> {
    let started = conn.execute(write(sql)).await?;
    if started.end.is_none() {
        conn.discard_cursor().await?;
    }
    Ok(())
}

/// ⑤ 값 표(결정 M3-D8).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_values_follow_the_value_rules() {
    let mut conn = open(&plain()).await;
    run(&mut conn, "SET TimeZone = 'UTC'").await;
    let rows = all(
        &mut conn,
        read(
            "SELECT 9007199254740992::int8, 9007199254740991::int8, 5::int8, numeric '0.1', '12.34'::money, \
                    '\\xdeadbeef'::bytea, '{\"b\": [1, 2], \"a\": 1}'::jsonb, timestamptz '2026-10-31 00:00:00+00', \
                    ARRAY[1, NULL, 3], 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'::uuid, NULL::text, \
                    1.5::float8, 0.1::float4, true, 'x'::varchar, interval '1 day 2 hours', inet '10.0.0.1/8', \
                    date '2026-10-31', ARRAY[[1, 2], [3, 4]], 'NaN'::float8",
        ),
    )
    .await;
    let row = &rows[0];
    let json = |i: usize| serde_json::to_value(&row[i]).unwrap();
    assert_eq!(json(0), serde_json::json!({ "int": "9007199254740992" }));
    assert_eq!(json(1), serde_json::json!({ "int": 9007199254740991_i64 }));
    assert_eq!(json(2), serde_json::json!({ "int": 5 }));
    assert_eq!(row[3], DbValue::Decimal("0.1".into()));
    match &row[4] {
        DbValue::Decimal(money) => assert!(money.contains("12.34"), "{money}"),
        other => panic!("money: {other:?}"),
    }
    assert_eq!(
        json(5),
        serde_json::json!({ "bytes": { "hex": "0xdeadbeef", "size": 4, "truncated": false } })
    );
    // jsonb 는 서버가 정규화한 글 그대로(파싱하지 않는다).
    assert_eq!(row[6], DbValue::Json("{\"a\": 1, \"b\": [1, 2]}".into()));
    assert_eq!(row[7], DbValue::Datetime("2026-10-31 00:00:00+00".into()));
    assert_eq!(
        row[8],
        DbValue::Array(vec![DbValue::int(1), DbValue::Null, DbValue::int(3)])
    );
    assert_eq!(
        row[9],
        DbValue::Uuid("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".into())
    );
    assert_eq!(row[10], DbValue::Null);
    assert_eq!(row[11], DbValue::float(1.5));
    assert_eq!(row[12], DbValue::float(0.1));
    assert_eq!(row[13], DbValue::Bool(true));
    assert_eq!(row[14], DbValue::Text("x".into()));
    assert_eq!(
        row[15],
        DbValue::other("1 day 02:00:00", "interval", CellLimit::DEFAULT)
    );
    assert_eq!(
        row[16],
        DbValue::other("10.0.0.1/8", "inet", CellLimit::DEFAULT)
    );
    assert_eq!(row[17], DbValue::Datetime("2026-10-31".into()));
    assert_eq!(
        row[18],
        DbValue::Array(vec![
            DbValue::Array(vec![DbValue::int(1), DbValue::int(2)]),
            DbValue::Array(vec![DbValue::int(3), DbValue::int(4)]),
        ])
    );
    assert_eq!(json(19), serde_json::json!({ "float": "NaN" }));

    // 로컬 numeric 디코더가 서버의 numeric_out 과 같은 글을 낸다.
    let numerics = all(
        &mut conn,
        read(
            "SELECT v, v::text FROM (VALUES (numeric '0'), ('0.00'), ('-0.00000005'), ('12345.678'), ('100000'), \
                    ('1e-20'), ('NaN'), ('Infinity'), ('-Infinity'), ('123456789012345678901234567890.123456789'), \
                    ('9007199254740993'), ('-1.10')) AS t(v)",
        ),
    )
    .await;
    assert_eq!(numerics.len(), 12);
    for row in &numerics {
        let DbValue::Decimal(decoded) = &row[0] else {
            panic!("{row:?}")
        };
        assert_eq!(decoded, text(&row[1]));
    }

    // 셀 상한: 넘는 텍스트 · 바이트는 앞부분과 원래 크기.
    let mut capped = read("SELECT repeat('a', 100), decode(repeat('ff', 100), 'hex')");
    capped.cell_limit = CellLimit(10);
    let rows = all(&mut conn, capped).await;
    let capped = serde_json::to_value(&rows[0]).unwrap();
    assert_eq!(capped[0]["truncated"]["size"], 100);
    assert_eq!(capped[0]["truncated"]["prefix"], "aaaaaaaaaa");
    assert_eq!(capped[1]["bytes"]["truncated"], true);
    assert_eq!(capped[1]["bytes"]["size"], 100);
}

/// ⑤ `timestamptz` 는 서버 세션 시간대의 텍스트 — 세션 `TimeZone` 을 바꾸면 따라 바뀐다(호스트 시간대가 아니다).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_timestamptz_follows_the_session_time_zone() {
    let mut conn = open(&plain()).await;
    let sql = "SELECT timestamptz '2026-10-31 00:00:00+00', ARRAY[timestamptz '2026-01-01 12:00:00+00', NULL]";
    run(&mut conn, "SET TimeZone = 'UTC'").await;
    let utc = all(&mut conn, read(sql)).await;
    assert_eq!(
        utc[0][0],
        DbValue::Datetime("2026-10-31 00:00:00+00".into())
    );
    run(&mut conn, "SET TimeZone = 'Asia/Seoul'").await;
    let seoul = all(&mut conn, read(sql)).await;
    assert_eq!(
        seoul[0][0],
        DbValue::Datetime("2026-10-31 09:00:00+09".into())
    );
    assert_eq!(
        seoul[0][1],
        DbValue::Array(vec![
            DbValue::Datetime("2026-01-01 21:00:00+09".into()),
            DbValue::Null
        ])
    );
    run(&mut conn, "SET DateStyle = 'SQL, DMY'").await;
    let sql_style = all(
        &mut conn,
        read("SELECT timestamptz '2026-10-31 00:00:00+00'"),
    )
    .await;
    assert_eq!(
        sql_style[0][0],
        DbValue::Datetime("31/10/2026 09:00:00 KST".into())
    );
}

/// 텍스트 출력 되돌려 받기가 실패해도 질의는 실패하지 않고 그 값만 「표시 불가」 — 트랜잭션 상태 · 포털은 그대로다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_a_failed_text_output_marks_only_those_values() {
    let mut conn = open(&plain()).await;
    let mut other = open(&plain()).await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_text_output").await;
    run(&mut conn, "CREATE TABLE pw_live_text_output (id int)").await;

    conn.begin(BeginOptions::default()).await.unwrap();
    run(&mut conn, "INSERT INTO pw_live_text_output VALUES (7)").await;
    conn.execute(read(
        "SELECT n, timestamptz '2026-10-31 00:00:00+00' + n * interval '1 hour' FROM generate_series(1, 10) AS n",
    ))
    .await
    .unwrap();
    conn.fail_text_output(true);
    let failed = conn.fetch(FetchLimit::rows(5)).await.unwrap();
    assert_eq!(failed.rows.len(), 5);
    assert_eq!(
        failed.rows[0][0],
        DbValue::int(1),
        "values that decode locally are kept"
    );
    match &failed.rows[0][1] {
        DbValue::Other(other) => {
            assert_eq!(other.type_name, "timestamptz");
            assert!(other.text.starts_with("(cannot display:"), "{}", other.text);
        }
        value => panic!("{value:?}"),
    }
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    conn.fail_text_output(false);
    let rest = conn.fetch(FetchLimit::rows(100)).await.unwrap();
    assert_eq!(
        rest.rows.len(),
        5,
        "the portal survived the failed text output"
    );
    assert_eq!(rest.rows[0][0], DbValue::int(6));
    assert!(matches!(rest.rows[0][1], DbValue::Datetime(_)));
    assert!(rest.end.is_some());
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    conn.commit().await.unwrap();
    assert_eq!(
        count(
            &mut other,
            "SELECT count(*) FROM pw_live_text_output WHERE id = 7"
        )
        .await,
        1
    );
}

/// 사람이 친 트랜잭션 문장은 거절하고(파이프라인이 메서드로 옮긴다), 세이브포인트 문장은 통과한다 — `ROLLBACK TO` 가 실패한
/// 트랜잭션을 되살린다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_transaction_statements_are_refused_and_savepoints_pass() {
    let mut conn = open(&plain()).await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_deferred").await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_savepoint").await;
    run(&mut conn, "CREATE TABLE pw_live_savepoint (id int)").await;
    for sql in [
        "BEGIN",
        "start transaction",
        "COMMIT AND CHAIN",
        "ROLLBACK AND CHAIN",
        "END",
        "ABORT",
        "PREPARE TRANSACTION 'x'",
        // PG 렉서와 같게 읽는다(M3-06 리뷰 H1): 서버는 이것들을 모두 BEGIN · COMMIT 으로 읽는다.
        "\x0bBEGIN",
        "\x0cBEGIN",
        "-- note\rBEGIN",
        ";BEGIN",
        "/* a /* b */ c */\x0b COMMIT",
    ] {
        let error = conn.execute(write(sql)).await.unwrap_err();
        assert_eq!(error.kind, DbErrorKind::Invalid, "{sql}");
        assert!(error.code.is_none(), "{sql} never reached the server");
        assert_eq!(conn.transaction_state(), TransactionState::Idle);
    }
    // 거르개가 지나보내는 「BEGIN 비슷한 것」 은 서버도 키워드가 아닌 식별자로 읽어 문법 오류다.
    for sql in ["BEGIN$x", "\u{3000}BEGIN", "BEGIN한"] {
        let error = conn.execute(write(sql)).await.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("42601"), "{sql}: {error:?}");
        assert_eq!(conn.transaction_state(), TransactionState::Idle);
    }

    conn.begin(BeginOptions::default()).await.unwrap();
    run(&mut conn, "INSERT INTO pw_live_savepoint VALUES (1)").await;
    run(&mut conn, "SAVEPOINT before_mistake").await;
    run(&mut conn, "INSERT INTO pw_live_savepoint VALUES (2)").await;
    let error = conn.execute(read("SELECT 1/0")).await;
    let error = match error {
        Err(e) => e,
        Ok(_) => conn.fetch(FetchLimit::rows(1)).await.unwrap_err(),
    };
    assert_eq!(error.code.as_deref(), Some("22012"));
    assert_eq!(conn.transaction_state(), TransactionState::Failed);
    run(&mut conn, "ROLLBACK TO SAVEPOINT before_mistake").await;
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    // trait 의 세이브포인트 메서드도 같은 길이다.
    conn.savepoint("edit".into()).await.unwrap();
    run(&mut conn, "INSERT INTO pw_live_savepoint VALUES (3)").await;
    conn.rollback_to_savepoint("edit".into()).await.unwrap();
    conn.release_savepoint("edit".into()).await.unwrap();
    run(&mut conn, "RELEASE SAVEPOINT before_mistake").await;
    conn.commit().await.unwrap();
    let ids = all(
        &mut conn,
        read("SELECT id FROM pw_live_savepoint ORDER BY id"),
    )
    .await;
    assert_eq!(ids, vec![vec![DbValue::int(1)]]);

    // COMMIT 이 실패하면(지연 제약) 서버가 트랜잭션 전체를 되돌렸다 — 오류가 `rolled_back` 을 싣고 세션은 idle 이다.
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_deferred").await;
    run(
        &mut conn,
        "ALTER TABLE pw_live_savepoint ADD PRIMARY KEY (id)",
    )
    .await;
    run(
        &mut conn,
        "CREATE TABLE pw_live_deferred (id int, parent int REFERENCES pw_live_savepoint (id) DEFERRABLE INITIALLY DEFERRED)",
    )
    .await;
    conn.begin(BeginOptions::default()).await.unwrap();
    run(&mut conn, "INSERT INTO pw_live_deferred VALUES (1, 999)").await;
    let error = conn.commit().await.unwrap_err();
    assert_eq!(error.code.as_deref(), Some("23503"), "{error:?}");
    assert!(error.rolled_back);
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert_eq!(
        count(&mut conn, "SELECT count(*) FROM pw_live_deferred").await,
        0
    );

    // 실패한 트랜잭션의 commit 은 성공이 아니다(M3-06 리뷰 H2): 서버는 COMMIT 에 롤백으로 답했다 — 오류 + rolled_back.
    let mut other = open(&plain()).await;
    conn.begin(BeginOptions::default()).await.unwrap();
    run(&mut conn, "INSERT INTO pw_live_deferred VALUES (2, NULL)").await;
    let error = match conn.execute(read("SELECT 1/0")).await {
        Err(e) => e,
        Ok(_) => conn.fetch(FetchLimit::rows(1)).await.unwrap_err(),
    };
    assert_eq!(error.code.as_deref(), Some("22012"));
    assert_eq!(conn.transaction_state(), TransactionState::Failed);
    let failed = conn.commit().await.unwrap_err();
    assert!(failed.rolled_back, "{failed:?}");
    assert_eq!(failed.code.as_deref(), Some("25P02"));
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    assert_eq!(
        count(&mut other, "SELECT count(*) FROM pw_live_deferred").await,
        0
    );
    assert_eq!(
        count(&mut conn, "SELECT 1").await,
        1,
        "the session is usable again"
    );
}

/// 교차 계약(M3-04): 나뉜 문장 하나 = 서버 문장 하나. 사람의 문장은 확장 프로토콜(Parse 하나에 한 문장)로만 간다 — 두 문장을 한
/// 번에 보내면 서버가 거절하고, 숨은 둘째 문장(`DROP`)은 돌지 않는다. 자동 커밋 · 수동 트랜잭션 · 실행 계획 모두 같다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_two_statements_in_one_execute_are_refused_by_the_server() {
    let mut conn = open(&plain()).await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_multi").await;
    run(&mut conn, "CREATE TABLE pw_live_multi (id int)").await;
    let sql = "SELECT 1; DROP TABLE pw_live_multi";
    for grade in [RiskGrade::Read, RiskGrade::Destructive] {
        let error = conn
            .execute(ExecuteRequest::new(sql, grade))
            .await
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("42601"), "{error:?}");
    }
    let error = conn
        .execute(write(
            "INSERT INTO pw_live_multi VALUES (1); DROP TABLE pw_live_multi",
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("42601"), "{error:?}");
    assert!(conn.explain(sql.into()).await.is_err());
    conn.begin(BeginOptions::default()).await.unwrap();
    let error = conn.execute(write(sql)).await.unwrap_err();
    assert_eq!(error.code.as_deref(), Some("42601"), "{error:?}");
    conn.rollback().await.unwrap();
    assert_eq!(
        count(&mut conn, "SELECT count(*) FROM pw_live_multi").await,
        0,
        "the table is still there"
    );
}

/// 실행 계획은 `EXPLAIN (FORMAT JSON)` 의 글.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_explain_returns_the_json_plan() {
    let mut conn = open(&plain()).await;
    let plan = conn
        .explain("SELECT * FROM generate_series(1, 10)".into())
        .await
        .unwrap();
    assert_eq!(plan.format, pw_db_types::PlanFormat::Json);
    assert!(plan.text.trim_start().starts_with('['), "{}", plan.text);
    assert!(plan.text.contains("\"Function Scan\""), "{}", plan.text);
}

/// 실행 계획은 문장을 돌리지 않는다(M3-15 — `EXPLAIN` 만, `ANALYZE` 없음): `DELETE` 의 계획을 받아도 행이 그대로이고, 사람이 친
/// `EXPLAIN ANALYZE …` · `ANALYZE …` 를 넘겨도 겹친 `EXPLAIN` 이 되어 서버가 문법 오류로 거절한다(파이프라인은 그 전에 거절한다).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_explain_never_runs_the_statement() {
    let mut conn = open(&plain()).await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_explain").await;
    run(&mut conn, "CREATE TABLE pw_live_explain (id int)").await;
    run(&mut conn, "INSERT INTO pw_live_explain VALUES (1), (2)").await;
    let plan = conn
        .explain("DELETE FROM pw_live_explain".into())
        .await
        .unwrap();
    assert!(plan.text.contains("\"Delete\""), "{}", plan.text);
    for nested in [
        "EXPLAIN ANALYZE DELETE FROM pw_live_explain",
        "EXPLAIN (ANALYZE) DELETE FROM pw_live_explain",
        "ANALYZE pw_live_explain",
    ] {
        let error = conn.explain(nested.into()).await.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("42601"), "{nested}: {error:?}");
    }
    assert_eq!(
        count(&mut conn, "SELECT count(*) FROM pw_live_explain").await,
        2
    );
    run(&mut conn, "DROP TABLE pw_live_explain").await;
}

/// 고른 스키마(M3-15 의 스키마 고르기)는 연결 시작 옵션의 `search_path` 다 — `SET` 을 보내지 않고, 따옴표 · 공백이 든 이름도 그대로
/// 지금 스키마가 된다. `public` 이 뒤에 남는다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_a_chosen_schema_is_the_current_schema() {
    let name = "pw live \"schema\"";
    let mut admin = open(&plain()).await;
    run(
        &mut admin,
        r#"DROP SCHEMA IF EXISTS "pw live ""schema""" CASCADE"#,
    )
    .await;
    run(&mut admin, r#"CREATE SCHEMA "pw live ""schema""""#).await;
    let mut spec = plain();
    spec.schema = Some(name.to_owned());
    let mut conn = open(&spec).await;
    assert_eq!(
        text(&one(&mut conn, "SELECT current_schema()::text").await),
        name
    );
    let path = one(&mut conn, "SHOW search_path").await;
    assert!(text(&path).ends_with("public"), "{path:?}");
    run(&mut admin, r#"DROP SCHEMA "pw live ""schema""" CASCADE"#).await;
}

/// 트랜잭션이 열린 채 닫으면 DB 가 롤백한다. 쉬는 연결은 풀로 돌아가 세션 상태를 지우고 다시 쓰인다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_closing_rolls_back_and_the_pool_resets_sessions() {
    let driver = PgDriver::new();
    let mut other = open(&plain()).await;
    run(&mut other, "DROP TABLE IF EXISTS pw_live_close").await;
    run(&mut other, "CREATE TABLE pw_live_close (id int)").await;

    let (mut conn, _) = driver.open(&plain()).await.unwrap();
    conn.begin(BeginOptions::default()).await.unwrap();
    run(&mut conn, "INSERT INTO pw_live_close VALUES (1)").await;
    let pid = backend_pid(&mut conn).await;
    Box::new(conn).close().await.unwrap();
    assert_eq!(
        count(&mut other, "SELECT count(*) FROM pw_live_close").await,
        0
    );
    // 그 연결은 풀에 돌아가지 않고 닫혔다.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut request = read("SELECT count(*) FROM pg_stat_activity WHERE pid = $1");
        request.params = vec![DbValue::int(pid)];
        let rows = all(&mut other, request).await;
        if int(&rows[0][0]) == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the backend in a transaction was not closed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (mut idle, _) = driver.open(&plain()).await.unwrap();
    let idle_pid = backend_pid(&mut idle).await;
    run(&mut idle, "SET TimeZone = 'Asia/Seoul'").await;
    run(&mut idle, "PREPARE pw_live_leak AS SELECT 1").await;
    Box::new(idle).close().await.unwrap();
    let (mut again, _) = driver.open(&plain()).await.unwrap();
    assert_eq!(
        backend_pid(&mut again).await,
        idle_pid,
        "the idle connection came back from the pool"
    );
    assert_ne!(
        text(&one(&mut again, "SHOW TimeZone").await),
        "Asia/Seoul",
        "the pool reset the session"
    );
    // 사람의 PREPARE 도 다음 탭에 넘어가지 않는다(`DISCARD ALL`, 리뷰 L2).
    let leaked = again
        .execute(write("EXECUTE pw_live_leak"))
        .await
        .unwrap_err();
    assert_eq!(leaked.code.as_deref(), Some("26000"), "{leaked:?}");
    // 풀이 지운 타입 조회 문장을 다시 만든다(꺼낼 때 `clear_type_cache`) — 모르는 타입의 열도 읽힌다.
    assert!(matches!(
        one(&mut again, "SELECT 'pg_class'::regclass").await,
        DbValue::Other(_)
    ));
    assert_eq!(again.transaction_state(), TransactionState::Idle);
}

/// 거르개가 놓친 숨은 `BEGIN` 이 서버에 블록을 열어도(M3-06 리뷰 H1 의 둘째 겹) 그 블록은 그 탭에 묶인다: 탭의 상태가 「트랜잭션
/// 열림」 이 되고, 닫으면 풀로 돌아가지 않고 DB 가 롤백한다 — 다른 탭의 자동 커밋이 앞 탭의 쓰기를 커밋하지 않는다(리뷰어 재현).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_a_hidden_transaction_block_stays_with_its_tab() {
    let driver = PgDriver::new();
    let mut observer = open(&plain()).await;
    run(&mut observer, "DROP TABLE IF EXISTS pw_live_hidden").await;
    run(&mut observer, "CREATE TABLE pw_live_hidden (id int)").await;

    let (mut a, _) = driver.open(&plain()).await.unwrap();
    let a_pid = backend_pid(&mut a).await;
    a.skip_statement_filter(true);
    run(&mut a, "BEGIN").await;
    assert_eq!(a.transaction_state(), TransactionState::InTransaction);
    run(&mut a, "INSERT INTO pw_live_hidden VALUES (1)").await;
    assert_eq!(
        count(&mut a, "SELECT count(*) FROM pw_live_hidden").await,
        1
    );
    assert_eq!(a.transaction_state(), TransactionState::InTransaction);
    assert_eq!(
        count(&mut observer, "SELECT count(*) FROM pw_live_hidden").await,
        0
    );
    Box::new(a).close().await.unwrap();

    let (mut b, _) = driver.open(&plain()).await.unwrap();
    assert_ne!(
        backend_pid(&mut b).await,
        a_pid,
        "the tab's open block did not go back to the pool"
    );
    assert_eq!(
        count(&mut b, "SELECT count(*) FROM pw_live_hidden").await,
        0
    );
    assert_eq!(b.transaction_state(), TransactionState::Idle);
    assert_eq!(
        count(&mut observer, "SELECT count(*) FROM pw_live_hidden").await,
        0,
        "nobody committed tab A's write"
    );

    // 사람이 그 블록을 커밋하면(콘솔의 커밋) 그때 남는다.
    b.skip_statement_filter(true);
    run(&mut b, "BEGIN").await;
    run(&mut b, "INSERT INTO pw_live_hidden VALUES (2)").await;
    b.commit().await.unwrap();
    assert_eq!(b.transaction_state(), TransactionState::Idle);
    assert_eq!(
        count(&mut observer, "SELECT count(*) FROM pw_live_hidden").await,
        1
    );
}

/// 사람의 트랜잭션 안에서 메타데이터는 풀의 다른 연결로, 실행 계획은 같은 연결의 세이브포인트 안에서 — 어느 쪽이 실패해도 사람의
/// 트랜잭션은 `Failed` 가 되지 않는다(M3-06 리뷰 M1).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_metadata_and_plans_leave_the_persons_transaction_alone() {
    let mut conn = open(&plain()).await;
    let mut observer = open(&plain()).await;
    run(&mut conn, "DROP TABLE IF EXISTS pw_live_meta_tx").await;
    conn.begin(BeginOptions::default()).await.unwrap();
    run(&mut conn, "CREATE TABLE pw_live_meta_tx (id int)").await;
    let query = ObjectQuery {
        database: None,
        schema: Some("public".into()),
        kind: ObjectKind::Table,
        filter: Some("pw_live_meta_tx".into()),
        cursor: None,
        limit: 10,
    };
    // 다른 연결은 아직 커밋되지 않은 테이블을 보지 못한다 — 메타데이터가 사람의 연결 밖에서 돌았다.
    assert!(conn.objects(query.clone()).await.unwrap().items.is_empty());
    // 다른 연결에서의 메타데이터 오류(없는 테이블)는 사람의 트랜잭션과 상관없다.
    let missing = ObjectRef {
        database: None,
        schema: Some("public".into()),
        name: "pw_live_meta_tx".into(),
    };
    assert!(conn.columns(missing).await.is_err());
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    // 실행 계획은 같은 연결이라 그 트랜잭션의 테이블을 본다. 문법 오류여도 트랜잭션은 산다.
    let plan = conn
        .explain("SELECT * FROM pw_live_meta_tx".into())
        .await
        .unwrap();
    assert!(plan.text.contains("pw_live_meta_tx"), "{}", plan.text);
    let error = conn.explain("SELEC 1".into()).await.unwrap_err();
    assert_eq!(error.code.as_deref(), Some("42601"));
    assert_eq!(conn.transaction_state(), TransactionState::InTransaction);
    run(&mut conn, "INSERT INTO pw_live_meta_tx VALUES (1)").await;
    conn.commit().await.unwrap();
    assert_eq!(
        count(&mut observer, "SELECT count(*) FROM pw_live_meta_tx").await,
        1
    );
    assert_eq!(conn.objects(query).await.unwrap().items.len(), 1);
}

/// 바이너리 send 함수가 없는 타입(`aclitem`)의 열은 서버가 거절한다 — 오류가 그 열 · 타입과 `::text` 힌트를 싣는다(리뷰 M3).
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_a_column_without_binary_output_names_itself() {
    let mut conn = open(&plain()).await;
    let sql = "SELECT relname, relacl FROM pg_class WHERE relacl IS NOT NULL LIMIT 1";
    let error = match conn.execute(read(sql)).await {
        Err(e) => e,
        Ok(_) => conn.fetch(FetchLimit::rows(1)).await.unwrap_err(),
    };
    assert_eq!(error.code.as_deref(), Some("42883"), "{error:?}");
    assert!(
        error.message.contains("column relacl (type aclitem[])"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("\"relacl\"::text"),
        "{}",
        error.message
    );
    assert_eq!(conn.transaction_state(), TransactionState::Idle);
    let rows = all(
        &mut conn,
        read("SELECT relname, relacl::text FROM pg_class WHERE relacl IS NOT NULL LIMIT 1"),
    )
    .await;
    assert_eq!(rows.len(), 1);
    conn.begin(BeginOptions::default()).await.unwrap();
    let error = match conn.execute(read(sql)).await {
        Err(e) => e,
        Ok(_) => conn.fetch(FetchLimit::rows(1)).await.unwrap_err(),
    };
    assert!(error.message.contains("relacl"), "{}", error.message);
    conn.rollback().await.unwrap();
}

/// ⑥ `verify-full`: OS 루트에 없는 CA 의 서버는 거절, 시험 CA 를 루트 자리(`root_cert`)에 넣으면 「검증함」 으로 붙는다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_tls_verify_full_trusts_only_the_given_roots() {
    let refused = PgDriver::new()
        .test(tls_spec(SslMode::VerifyFull))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, DbErrorKind::Tls, "{refused:?}");

    let mut trusted = tls_spec(SslMode::VerifyFull);
    trusted.tls.root_cert = Some(tls_ca());
    let server = PgDriver::new().test(trusted.clone()).await.unwrap();
    assert_eq!(server.tls, TlsStatus::Verified);
    assert!(server.version.unwrap().starts_with("17."));
    let (mut conn, server) = PgDriver::new().open(&trusted).await.unwrap();
    assert_eq!(server.tls, TlsStatus::Verified);
    assert_eq!(
        one(
            &mut conn,
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()"
        )
        .await,
        DbValue::Bool(true)
    );

    // `verify-ca` 는 사슬을 본다 — 시험 CA 를 루트에 넣지 않으면 거절한다(이름 오류만 걸러 내지 않는다). 이름만 틀린 경우는 터널
    // 시험(`live_pg_tls_through_a_tunnel_checks_the_db_host_name`)이 본다.
    let chain_only = |root: bool| {
        let mut spec = tls_spec(SslMode::VerifyCa);
        spec.tls.root_cert = root.then(tls_ca);
        spec
    };
    assert_eq!(
        PgDriver::new().test(chain_only(true)).await.unwrap().tls,
        TlsStatus::Unverified
    );
    let error = PgDriver::new().test(chain_only(false)).await.unwrap_err();
    assert_eq!(error.kind, DbErrorKind::Tls, "{error:?}");
}

/// ⑥ `require` · `prefer` · `allow` 는 「검증하지 않음」, `disable` 은 「평문」. `allow` 는 평문이 거절되면 TLS 로 다시 붙는다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_tls_modes_report_what_they_verified() {
    let status = |mode| async move { PgDriver::new().test(tls_spec(mode)).await.unwrap().tls };
    assert_eq!(status(SslMode::Require).await, TlsStatus::Unverified);
    assert_eq!(status(SslMode::Prefer).await, TlsStatus::Unverified);
    assert_eq!(status(SslMode::Disable).await, TlsStatus::Plaintext);
    assert_eq!(status(SslMode::Allow).await, TlsStatus::Plaintext);

    let mut tls_only = tls_spec(SslMode::Allow);
    tls_only.user = Some("pitwall_tls_only".into());
    let (mut conn, server) = PgDriver::new().open(&tls_only).await.unwrap();
    assert_eq!(server.tls, TlsStatus::Unverified, "allow fell back to TLS");
    assert_eq!(
        text(&one(&mut conn, "SELECT current_user::text").await),
        "pitwall_tls_only"
    );
    tls_only.tls = TlsSettings::mode(SslMode::Disable);
    let refused = PgDriver::new().test(tls_only).await.unwrap_err();
    assert_eq!(refused.kind, DbErrorKind::Auth, "{refused:?}");

    // TLS 를 못 하는 서버(평문 PG)에 `require` 면 붙지 않는다.
    let mut plain_require = plain();
    plain_require.tls = TlsSettings::mode(SslMode::Require);
    let error = PgDriver::new().test(plain_require).await.unwrap_err();
    assert_eq!(error.kind, DbErrorKind::Tls, "{error:?}");
}

/// 터널 너머 TLS: TCP 는 `connect_to`(`hostaddr`)로, 이름 검증은 설정의 DB `host` 로.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_tls_through_a_tunnel_checks_the_db_host_name() {
    let base = tls_spec(SslMode::VerifyFull);
    let Target::Network { port, .. } = base.target else {
        unreachable!()
    };
    let tunnelled = |host: &str, mode| {
        let mut spec = base.clone();
        spec.tls = TlsSettings::mode(mode);
        spec.tls.root_cert = Some(tls_ca());
        spec.target = Target::Network {
            host: host.into(),
            port: 5432,
            connect_to: Some(SocketTarget {
                host: "127.0.0.1".into(),
                port,
            }),
        };
        spec
    };
    let (mut conn, server) = PgDriver::new()
        .open(&tunnelled("localhost", SslMode::VerifyFull))
        .await
        .unwrap();
    assert_eq!(server.tls, TlsStatus::Verified);
    assert_eq!(one(&mut conn, "SELECT 1").await, DbValue::int(1));
    // 이름이 인증서와 다른 DB 호스트(풀 수 없는 이름 — TCP 는 터널로 간다)는 `verify-full` 이 거절한다.
    let error = PgDriver::new()
        .test(tunnelled("db.pitwall.invalid", SslMode::VerifyFull))
        .await
        .unwrap_err();
    assert_eq!(error.kind, DbErrorKind::Tls, "{error:?}");
    let chain_only = PgDriver::new()
        .test(tunnelled("db.pitwall.invalid", SslMode::VerifyCa))
        .await
        .unwrap();
    assert_eq!(chain_only.tls, TlsStatus::Unverified);
}

/// ⑦ 픽스처 스키마의 테이블 · 열 · 인덱스 · FK · DDL.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_metadata_reads_the_fixture_schema() {
    let mut conn = open(&plain()).await;
    run(&mut conn, "DROP SCHEMA IF EXISTS pw_live_meta CASCADE").await;
    for sql in [
        "CREATE SCHEMA pw_live_meta",
        "CREATE TABLE pw_live_meta.parent (id int PRIMARY KEY, code text NOT NULL UNIQUE)",
        "CREATE TABLE pw_live_meta.child (\
            id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
            parent_id int NOT NULL REFERENCES pw_live_meta.parent (id), \
            note text NOT NULL DEFAULT 'n/a', \
            amount numeric(10, 2) CHECK (amount >= 0))",
        "CREATE INDEX child_note_idx ON pw_live_meta.child (lower(note)) WHERE note <> ''",
        "CREATE UNIQUE INDEX child_parent_note_idx ON pw_live_meta.child (parent_id, note) INCLUDE (amount)",
        "COMMENT ON TABLE pw_live_meta.child IS 'child''s rows'",
        "COMMENT ON COLUMN pw_live_meta.child.note IS 'free text'",
        "CREATE VIEW pw_live_meta.child_view AS SELECT id, note FROM pw_live_meta.child",
        "CREATE SEQUENCE pw_live_meta.counter START 10",
        "CREATE FUNCTION pw_live_meta.twice(x int) RETURNS int LANGUAGE sql IMMUTABLE AS 'SELECT x * 2'",
        "CREATE PROCEDURE pw_live_meta.touch() LANGUAGE sql AS 'SELECT 1'",
        "CREATE FUNCTION pw_live_meta.stamp() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RETURN NEW; END'",
        "CREATE TRIGGER child_stamp BEFORE INSERT ON pw_live_meta.child FOR EACH ROW EXECUTE FUNCTION pw_live_meta.stamp()",
    ] {
        run(&mut conn, sql).await;
    }
    let schema = Some("pw_live_meta".to_owned());
    let names =
        |page: pw_db_types::ObjectPage| page.items.into_iter().map(|i| i.name).collect::<Vec<_>>();
    let query = |kind, filter: Option<&str>| ObjectQuery {
        database: None,
        schema: schema.clone(),
        kind,
        filter: filter.map(str::to_owned),
        cursor: None,
        limit: 50,
    };
    assert_eq!(
        names(conn.objects(query(ObjectKind::Table, None)).await.unwrap()),
        ["child", "parent"]
    );
    let tables = conn
        .objects(query(ObjectKind::Table, Some("CHI")))
        .await
        .unwrap();
    assert_eq!(tables.items[0].comment.as_deref(), Some("child's rows"));
    assert_eq!(tables.items.len(), 1);
    assert_eq!(
        names(
            conn.objects(query(ObjectKind::Table, Some("%")))
                .await
                .unwrap()
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        names(conn.objects(query(ObjectKind::View, None)).await.unwrap()),
        ["child_view"]
    );
    // 식별 열의 시퀀스도 시퀀스다.
    assert_eq!(
        names(
            conn.objects(query(ObjectKind::Sequence, None))
                .await
                .unwrap()
        ),
        ["child_id_seq", "counter"]
    );
    assert_eq!(
        names(
            conn.objects(query(ObjectKind::Function, None))
                .await
                .unwrap()
        ),
        ["stamp", "twice"]
    );
    assert_eq!(
        names(
            conn.objects(query(ObjectKind::Procedure, None))
                .await
                .unwrap()
        ),
        ["touch"]
    );
    assert_eq!(
        names(
            conn.objects(query(ObjectKind::Trigger, None))
                .await
                .unwrap()
        ),
        ["child_stamp"]
    );
    let schemas = conn.schemas(None).await.unwrap();
    assert!(schemas.iter().any(|s| s.name == "pw_live_meta"));
    assert!(
        conn.databases()
            .await
            .unwrap()
            .iter()
            .any(|d| d.name == "pitwall")
    );

    let child = ObjectRef {
        database: None,
        schema: schema.clone(),
        name: "child".into(),
    };
    let columns = conn.columns(child.clone()).await.unwrap();
    let summary: Vec<_> = columns
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.db_type.as_str(),
                c.nullable,
                c.default.as_deref(),
                c.comment.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("id", "bigint", false, None, None),
            ("parent_id", "integer", false, None, None),
            (
                "note",
                "text",
                false,
                Some("'n/a'::text"),
                Some("free text")
            ),
            ("amount", "numeric(10,2)", true, None, None),
        ]
    );
    let indexes = conn.indexes(child.clone()).await.unwrap();
    let summary: Vec<_> = indexes
        .iter()
        .map(|i| {
            (
                i.name.as_str(),
                i.columns.clone(),
                i.primary,
                i.unique,
                i.partial,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "child_note_idx",
                vec!["lower(note)".to_owned()],
                false,
                false,
                true
            ),
            (
                "child_parent_note_idx",
                vec!["parent_id".to_owned(), "note".to_owned()],
                false,
                true,
                false
            ),
            ("child_pkey", vec!["id".to_owned()], true, true, false),
        ]
    );
    let keys = conn.foreign_keys(child.clone()).await.unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "child_parent_id_fkey");
    assert_eq!(keys[0].columns, ["parent_id"]);
    assert_eq!(keys[0].references.schema.as_deref(), Some("pw_live_meta"));
    assert_eq!(keys[0].references.name, "parent");
    assert_eq!(keys[0].references.database.as_deref(), Some("pitwall"));
    assert_eq!(keys[0].referenced_columns, ["id"]);

    let ddl = conn.ddl(child.clone()).await.unwrap().text;
    assert_eq!(
        ddl,
        "CREATE TABLE pw_live_meta.child (\n    \
            id bigint GENERATED ALWAYS AS IDENTITY NOT NULL,\n    \
            parent_id integer NOT NULL,\n    \
            note text DEFAULT 'n/a'::text NOT NULL,\n    \
            amount numeric(10,2),\n    \
            CONSTRAINT child_pkey PRIMARY KEY (id),\n    \
            CONSTRAINT child_amount_check CHECK (amount >= 0::numeric),\n    \
            CONSTRAINT child_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES pw_live_meta.parent(id)\n\
         );\n\
         CREATE INDEX child_note_idx ON pw_live_meta.child USING btree (lower(note)) WHERE (note <> ''::text);\n\
         CREATE UNIQUE INDEX child_parent_note_idx ON pw_live_meta.child USING btree (parent_id, note) INCLUDE (amount);\n\
         COMMENT ON TABLE pw_live_meta.child IS 'child''s rows';\n\
         COMMENT ON COLUMN pw_live_meta.child.note IS 'free text';\n"
    );
    let at = |name: &str| ObjectRef {
        database: None,
        schema: schema.clone(),
        name: name.into(),
    };
    let view = conn.ddl(at("child_view")).await.unwrap().text;
    assert_eq!(
        view,
        "CREATE OR REPLACE VIEW pw_live_meta.child_view AS\n SELECT id,\n    note\n   FROM pw_live_meta.child;\n"
    );
    let sequence = conn.ddl(at("counter")).await.unwrap().text;
    assert_eq!(
        sequence,
        "CREATE SEQUENCE pw_live_meta.counter AS bigint INCREMENT BY 1 MINVALUE 1 MAXVALUE 9223372036854775807 START WITH 10 CACHE 1 NO CYCLE;\n"
    );
    let function = conn.ddl(at("twice")).await.unwrap().text;
    assert!(
        function.starts_with("CREATE OR REPLACE FUNCTION pw_live_meta.twice(x integer)"),
        "{function}"
    );
    let trigger = conn.ddl(at("child_stamp")).await.unwrap().text;
    assert_eq!(
        trigger,
        "CREATE TRIGGER child_stamp BEFORE INSERT ON pw_live_meta.child FOR EACH ROW EXECUTE FUNCTION pw_live_meta.stamp();"
    );
    // 다른 DB 의 메타데이터는 이 연결로 읽지 않는다.
    let elsewhere = ObjectRef {
        database: Some("postgres".into()),
        ..child
    };
    assert_eq!(
        conn.columns(elsewhere).await.unwrap_err().kind,
        DbErrorKind::Invalid
    );
    run(&mut conn, "DROP SCHEMA pw_live_meta CASCADE").await;
}

/// ⑦ 테이블 1만 개 스키마에서 첫 쪽(200)이 200 ms 안. 다음 쪽 커서가 이어진다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_the_first_page_of_ten_thousand_tables_is_fast() {
    let mut conn = open(&plain()).await;
    drop_many(&mut conn).await;
    run(&mut conn, "CREATE SCHEMA pw_live_many").await;
    // 한 트랜잭션에 많이 만들면 공유 잠금 표(`max_locks_per_transaction` × 연결 수)가 넘쳐 함께 도는 시험까지 실패한다 — 100 개씩.
    for batch in 0..100 {
        run(
            &mut conn,
            &format!(
                "DO $$ BEGIN FOR i IN {}..{} LOOP \
                   EXECUTE format('CREATE TABLE pw_live_many.t%s (id int)', lpad(i::text, 5, '0')); \
                 END LOOP; END $$",
                batch * 100 + 1,
                batch * 100 + 100
            ),
        )
        .await;
    }
    let mut fresh = open(&plain()).await;
    let query = ObjectQuery {
        database: None,
        schema: Some("pw_live_many".into()),
        kind: ObjectKind::Table,
        filter: None,
        cursor: None,
        limit: 200,
    };
    let started = Instant::now();
    let first = fresh.objects(query.clone()).await.unwrap();
    let elapsed = started.elapsed();
    println!("first page of 10,000 tables: {elapsed:?}");
    assert_eq!(first.items.len(), 200);
    assert_eq!(first.items[0].name, "t00001");
    assert!(elapsed < Duration::from_millis(200), "{elapsed:?}");
    let second = fresh
        .objects(ObjectQuery {
            cursor: first.next_cursor.clone(),
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(second.items[0].name, "t00201");
    let last = fresh
        .objects(ObjectQuery {
            cursor: Some("9900".into()),
            ..query
        })
        .await
        .unwrap();
    assert_eq!(last.items.len(), 100);
    assert_eq!(last.next_cursor, None);
    drop_many(&mut conn).await;
}

/// `pw_live_many` 를 100 개씩 지운다 — 한 번에 `DROP SCHEMA … CASCADE` 하면 1만 개의 잠금이 공유 잠금 표를 넘친다.
async fn drop_many(conn: &mut PgConnection) {
    loop {
        let left = count(
            conn,
            "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'pw_live_many' AND c.relkind = 'r'",
        )
        .await;
        if left == 0 {
            break;
        }
        run(
            conn,
            "DO $$ DECLARE t record; BEGIN \
               FOR t IN SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                        WHERE n.nspname = 'pw_live_many' AND c.relkind = 'r' LIMIT 100 LOOP \
                 EXECUTE format('DROP TABLE pw_live_many.%I', t.relname); \
               END LOOP; END $$",
        )
        .await;
    }
    run(conn, "DROP SCHEMA IF EXISTS pw_live_many").await;
}

/// 비용(코디네이터 조건): `timestamptz` 열이 있는 10만 행에서 텍스트 출력 되돌려 받기가 첫 청크 · 전체를 얼마나 늘리는가.
/// 기준은 결정 M3-D17 의 첫 화면 500 ms. 수치는 `--no-capture` 로 본다.
#[tokio::test(flavor = "multi_thread")]
async fn live_pg_text_output_cost_on_a_hundred_thousand_rows() {
    let mut conn = open(&plain()).await;
    let mut measure = async |sql: &str| {
        let started = Instant::now();
        conn.execute(read(sql)).await.unwrap();
        let first = conn.fetch(FetchLimit::FIRST_SCREEN).await.unwrap();
        let first_at = started.elapsed();
        let mut rows = first.rows.len();
        let mut end = first.end;
        while end.is_none() {
            let batch = conn.fetch(FetchLimit::rows(5000)).await.unwrap();
            rows += batch.rows.len();
            end = batch.end;
        }
        assert_eq!(rows, 100_000);
        (first_at, started.elapsed())
    };
    let base = measure("SELECT n, n::int8 AS m FROM generate_series(1, 100000) AS n").await;
    let stamped = measure(
        "SELECT n, timestamptz '2026-01-01 00:00:00+00' + n * interval '1 second' AS t FROM generate_series(1, 100000) AS n",
    )
    .await;
    println!(
        "100k rows — int8: first chunk {:?}, all {:?}; timestamptz: first chunk {:?}, all {:?}",
        base.0, base.1, stamped.0, stamped.1
    );
    assert!(stamped.0 < Duration::from_millis(500), "{:?}", stamped.0);
}

/// 리터럴 왕복(M3-16 — 「INSERT 문으로 복사」 의 리터럴, `pw_db_sql::literal`): 무작위 값을 리터럴 `INSERT` 와 바인드 `INSERT`
/// 로 한 행씩 넣고 다시 읽은 두 값이 같은지(그리고 표현이 그대로인 타입은 원래 값과도) — `standard_conforming_strings` 가 켜진
/// 세션과 꺼진 세션 둘 다. 시드는 `PW_LITERAL_SEED` 로 고정한다.
#[tokio::test]
async fn live_pg_literals_round_trip_under_both_string_settings() {
    use pw_db_sql::literal::{insert_text, testing};
    use pw_db_types::SqlDialect;

    let (mut rng, seed) = testing::Rng::from_env();
    let samples = testing::samples(SqlDialect::Postgres, &mut rng, 30);
    let mut conn = open(&plain()).await;
    for (mode, setting) in [("on", "on"), ("off", "off")] {
        run(
            &mut conn,
            &format!("SET standard_conforming_strings = {setting}"),
        )
        .await;
        run(&mut conn, "SET escape_string_warning = off").await;
        for (i, s) in samples.iter().enumerate() {
            let table = format!("lit_{}_{mode}_{i}", std::process::id());
            run(
                &mut conn,
                &format!("CREATE TEMP TABLE {table} (k int, v {})", s.column_type),
            )
            .await;
            let object = ObjectRef {
                database: None,
                schema: None,
                name: table.clone(),
            };
            let cols = ["k".to_owned(), "v".to_owned()];
            let insert = insert_text(
                SqlDialect::Postgres,
                Some(&object),
                &cols,
                &[vec![DbValue::int(1), s.value.clone()]],
            )
            .unwrap_or_else(|e| panic!("seed {seed}: {e:?} for {:?}", s.value));
            run(&mut conn, insert.trim_end()).await;
            let mut bound = write(&format!("INSERT INTO {table} (k, v) VALUES (2, $1)"));
            bound.params = vec![s.value.clone()];
            all(&mut conn, bound).await;
            let rows = all(
                &mut conn,
                read(&format!("SELECT v FROM {table} ORDER BY k")),
            )
            .await;
            assert_eq!(
                rows[0][0], rows[1][0],
                "seed {seed}, scs {mode}, {}: literal {insert:?} read back differently from the bound value {:?}",
                s.column_type, s.value
            );
            if s.exact {
                assert_eq!(
                    rows[0][0], s.value,
                    "seed {seed}, scs {mode}, {}: literal {insert:?}",
                    s.column_type
                );
            }
            run(&mut conn, &format!("DROP TABLE {table}")).await;
        }
    }
    eprintln!(
        "pg literal round trip: {} samples × 2 settings, seed {seed}",
        samples.len()
    );
}
