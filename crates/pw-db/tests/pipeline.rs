//! M3-05 의 완료 기준(14-m3-plan.md M3-05 행): 가짜 드라이버로 실행 파이프라인 · 세션 · 관문을 실제 RPC 세션 너머에서 본다.
//! ① credit 백프레셔(첫 청크 50 ms 는 `first_rows_chunk_of_100k_arrives_within_50ms`, release) ② 행 상한 끊기 ③ read-only
//! ④ 확인 해시 ⑤ 연결 범위 · 신뢰 ⑥ 취소 ⑦ 창 닫기 ⑧ 쪽 단위 실행(⑨ 는 `gate` 의 `compile_fail` doctest). 그 밖에 트랜잭션 제어
//! 옮기기, 드라이버가 보고하는 상태 변화(암묵 커밋 · 전체 롤백 · 숨은 블록 · 끊김), 설정 바뀜.

mod common;

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Conn, Fixture, StreamMsg};
use pw_core::bus::{Filter, Received};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::fake::{FakeCall, FakeDriver, FakeEnding, FakeResponse};
use pw_db_types::{DbError, DbErrorKind, SqlDialect};
use pw_rpc::protocol::StreamOutcome;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

struct Env {
    f: Fixture,
    sessions: Arc<DbSessions>,
    pg: FakeDriver,
    my: FakeDriver,
    lite: FakeDriver,
}

const DEV: &str = "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"localhost\"\n";

fn env(trusted: bool) -> Env {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let my = FakeDriver::named("mysql", SqlDialect::Mysql);
    let lite = FakeDriver::named("sqlite", SqlDialect::Sqlite);
    lite.interrupt_rolls_back(true);
    let mut drivers = pw_db::DriverRegistry::new();
    for d in [&pg, &my, &lite] {
        drivers.register(Arc::new(d.clone())).unwrap();
    }
    let mut slot = None;
    let f = Fixture::build(trusted, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    let sessions = slot.unwrap();
    f.write_db("dev.toml", DEV);
    f.write_db(
        "prod.toml",
        "schema = 1\nname = \"prod db\"\ndriver = \"postgres\"\nhost = \"localhost\"\nproduction = true\n",
    );
    f.write_db(
        "ro.toml",
        "schema = 1\nname = \"ro\"\ndriver = \"postgres\"\nhost = \"localhost\"\nread_only = true\n",
    );
    f.write_db(
        "my.toml",
        "schema = 1\nname = \"my\"\ndriver = \"mysql\"\nhost = \"localhost\"\n",
    );
    fs::write(f.root.join("data.db"), b"").unwrap();
    f.write_db(
        "lite.toml",
        "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"data.db\"\n",
    );
    Env {
        f,
        sessions,
        pg,
        my,
        lite,
    }
}

impl Env {
    /// 이 프로젝트 창의 label(데스크톱의 `project-<id>`).
    fn label(&self) -> String {
        format!("project-{}", self.f.id)
    }

    /// `label` 창의 연결(새로고침한 창은 같은 label 로 새 연결을 연다).
    fn window_in(&self, label: &str) -> Conn {
        self.f
            .connect_in(ConnectionScope::Project(self.f.id.clone()), Some(label))
    }

    /// `tab` 탭의 세션을 연다.
    async fn open_tab(&self, w: &mut Conn, connection: &str, tab: &str) -> String {
        let opened = w
            .call(
                "db.session.open",
                json!({ "projectId": self.f.id, "connectionId": connection, "tabId": tab }),
            )
            .await
            .unwrap();
        opened["session"]["sessionId"].as_str().unwrap().to_owned()
    }

    async fn open(&self, w: &mut Conn, connection: &str) -> String {
        let opened = w
            .call(
                "db.session.open",
                json!({ "projectId": self.f.id, "connectionId": connection }),
            )
            .await
            .unwrap();
        opened["session"]["sessionId"].as_str().unwrap().to_owned()
    }
}

fn exec(session: &str, sql: &str) -> Value {
    json!({ "sessionId": session, "sql": sql })
}

/// 실행해 끝까지 모은다. 관문이 거절하면 그 오류.
async fn run(w: &mut Conn, params: Value) -> Result<Vec<Value>, RpcError> {
    let stream = w.start("db.query.execute", params).await?;
    let (items, outcome) = w.collect(stream).await;
    match outcome {
        StreamOutcome::Completed => Ok(items),
        StreamOutcome::Failed { error } => Err(error),
        StreamOutcome::Cancelled => panic!("cancelled"),
    }
}

fn executed(driver: &FakeDriver) -> Vec<String> {
    driver
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            FakeCall::Execute(sql) => Some(sql),
            _ => None,
        })
        .collect()
}

fn denied(e: &RpcError) -> bool {
    e.code == rpc_code::PERMISSION_DENIED
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

/// ① credit 백프레셔: 창이 credit 을 주지 않으면 드라이버가 더 읽지 않는다(읽은 행 ≤ 보낸 행 + 한 청크). credit 을 주면 다시 읽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_credit_the_driver_reads_no_further() {
    let e = env(true);
    e.pg.respond("FROM big", FakeResponse::rows(100_000));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let stream = w
        .start("db.query.execute", exec(&s, "SELECT * FROM big"))
        .await
        .unwrap();
    // credit 0: 아무것도 읽지 않는다.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(e.pg.rows_read(), 0);
    // accepted · started · columns · rows 하나.
    w.credit(stream, 4);
    let mut sent = 0;
    for _ in 0..4 {
        let StreamMsg::Chunk(item) = w.next(stream).await else {
            panic!("ended")
        };
        if item["kind"] == "rows" {
            sent += item["rows"].as_array().unwrap().len() as u64;
        }
    }
    assert_eq!(sent, 500, "the first chunk is the first screen");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let read = e.pg.rows_read();
    assert!(read <= sent + 1000, "read {read} rows for {sent} sent");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(e.pg.rows_read(), read, "no credit, no further reads");
    let (items, outcome) = w.collect(stream).await;
    assert_eq!(outcome, StreamOutcome::Completed);
    let rows: usize = items
        .iter()
        .filter(|i| i["kind"] == "rows")
        .map(|i| i["rows"].as_array().unwrap().len())
        .sum();
    assert_eq!(sent as usize + rows, 100_000);
    let done = items.last().unwrap();
    assert_eq!(done["kind"], "done");
    assert_eq!(done["truncated"], false);
}

/// ① 의 시간 기준(release, `mise run test-release`): 10만 행 결과의 첫 `rows` 청크가 요청 뒤 50 ms 안.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "release timing — mise run test-release"]
async fn first_rows_chunk_of_100k_arrives_within_50ms() {
    let e = env(true);
    e.pg.respond("FROM big", FakeResponse::rows(100_000));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    // 데우기(첫 분류 스레드 · 첫 연결 경로).
    run(&mut w, exec(&s, "SELECT 1")).await.unwrap();
    let mut worst = Duration::ZERO;
    for _ in 0..5 {
        let asked = Instant::now();
        let stream = w
            .start("db.query.execute", exec(&s, "SELECT * FROM big"))
            .await
            .unwrap();
        w.credit(stream, 1_000_000);
        loop {
            let StreamMsg::Chunk(item) = w.next(stream).await else {
                panic!("ended")
            };
            if item["kind"] == "rows" {
                break;
            }
        }
        worst = worst.max(asked.elapsed());
        w.cancel_stream(stream);
        while let StreamMsg::Chunk(_) = w.next(stream).await {}
        // 세션이 다음 실행을 받을 때까지(취소 정리).
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    eprintln!("first rows chunk: worst {worst:?}");
    assert!(worst < Duration::from_millis(50), "{worst:?}");
}

/// ② 상한 + 1 행이면 `truncated` 이고 「커서 버리기」 가 불렸고 「닫기」 는 아니다. 수동 커밋 세션의 쓰기 → 상한 넘는 조회 → 커밋이면
/// 쓰기가 커밋돼 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_row_limit_discards_the_cursor_and_keeps_the_transaction() {
    let e = env(true);
    e.sessions.set_grid_max_rows(Some(1000));
    e.pg.respond("FROM big", FakeResponse::rows(1001));
    e.pg.respond("FROM fits", FakeResponse::rows(1000));
    e.pg.respond("INSERT", FakeResponse::write(1));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;

    let items = run(&mut w, exec(&s, "SELECT * FROM fits")).await.unwrap();
    assert_eq!(items.last().unwrap()["truncated"], false);

    let mode = w
        .call(
            "db.session.setMode",
            json!({ "sessionId": s, "mode": "manual" }),
        )
        .await
        .unwrap();
    assert_eq!(mode["mode"], "manual");
    run(&mut w, exec(&s, "INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let items = run(&mut w, exec(&s, "SELECT * FROM big")).await.unwrap();
    let rows: usize = items
        .iter()
        .filter(|i| i["kind"] == "rows")
        .map(|i| i["rows"].as_array().unwrap().len())
        .sum();
    assert_eq!(rows, 1000);
    assert_eq!(items.last().unwrap()["truncated"], true, "{items:?}");
    assert!(
        e.pg.calls_of(0)
            .contains(&FakeCall::DiscardCursor { drained: false })
    );
    assert!(!e.pg.calls_of(0).contains(&FakeCall::Close));
    assert!(e.pg.committed().is_empty(), "still in the transaction");
    let info = w
        .call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap();
    assert_eq!(info["state"]["kind"], "idle");
    assert_eq!(e.pg.committed(), ["INSERT INTO t VALUES (1)"]);
    // 수동 커밋은 첫 문장 앞에서 트랜잭션을 열었다(한 번).
    assert_eq!(e.pg.count(&FakeCall::Begin { read_only: false }), 1);
}

/// 받은 셀 바이트 합(Host 가 세는 규칙 그대로).
fn held_bytes(items: &[Value]) -> u64 {
    items
        .iter()
        .filter(|i| i["kind"] == "rows")
        .flat_map(|i| i["rows"].as_array().unwrap().iter())
        .flat_map(|row| row.as_array().unwrap().iter())
        .map(|cell| pw_db::execute::cell_bytes(&serde_json::from_value(cell.clone()).unwrap()))
        .sum()
}

fn rows_of(items: &[Value]) -> usize {
    items
        .iter()
        .filter(|i| i["kind"] == "rows")
        .map(|i| i["rows"].as_array().unwrap().len())
        .sum()
}

/// 그리드 보유 바이트 상한(`[db] grid_max_bytes`, M3-16 리뷰): 행 상한과 바이트 상한 중 먼저 닿는 쪽에서 커서를 버린다 — 연결 ·
/// 트랜잭션은 그대로. 바이트 상한에서 멈추면 받은 바이트는 상한 이상이고 한 행 이상 넘지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_byte_limit_or_the_row_limit_stops_the_result_whichever_comes_first() {
    let e = env(true);
    e.pg.respond("FROM big", FakeResponse::rows(5000));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;

    // 바이트가 먼저: 행 상한 1000 · 바이트 상한 2000(한 행 ≈ 14 바이트 — 약 150행에서 닿는다).
    e.sessions.set_grid_max_rows(Some(1000));
    e.sessions.set_grid_max_bytes(Some(2000));
    let items = run(&mut w, exec(&s, "SELECT * FROM big")).await.unwrap();
    assert_eq!(items.last().unwrap()["truncated"], true, "{items:?}");
    let (rows, bytes) = (rows_of(&items), held_bytes(&items));
    assert!(rows < 1000, "{rows}");
    assert!((2000..2000 + 32).contains(&bytes), "{bytes}");
    assert!(
        e.pg.calls_of(0)
            .contains(&FakeCall::DiscardCursor { drained: false })
    );
    assert!(!e.pg.calls_of(0).contains(&FakeCall::Close));

    // 행이 먼저: 같은 결과에 행 상한 100 · 바이트 상한 64 MiB(기본).
    e.sessions.set_grid_max_rows(Some(100));
    e.sessions.set_grid_max_bytes(None);
    assert_eq!(e.sessions.grid_max_bytes(), 64 * 1024 * 1024);
    let items = run(&mut w, exec(&s, "SELECT * FROM big")).await.unwrap();
    assert_eq!(items.last().unwrap()["truncated"], true, "{items:?}");
    assert_eq!(rows_of(&items), 100);
    assert!(held_bytes(&items) < 2000);

    // 둘 다 넉넉하면 끝까지.
    e.sessions.set_grid_max_rows(Some(5000));
    e.sessions.set_grid_max_bytes(Some(1_000_000));
    let items = run(&mut w, exec(&s, "SELECT * FROM big")).await.unwrap();
    assert_eq!(items.last().unwrap()["truncated"], false, "{items:?}");
    assert_eq!(rows_of(&items), 5000);
}

/// 상한은 전역 `settings.toml` 의 `[db] grid_max_rows` · `grid_max_bytes` 를 읽는다(1 이상의 정수가 아니면 기본값).
#[test]
fn the_grid_limits_come_from_the_global_settings() {
    use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
    use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
    let sessions = |settings: &str| {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(paths.config.join("settings.toml"), settings).unwrap();
        let global = ConfigStore::open(
            ConfigSource::global(&paths),
            EventBus::new(DEFAULT_CAPACITY),
            Scope::Host,
        );
        let e = env(true);
        e.sessions.attach_settings(global);
        (e.sessions.grid_max_rows(), e.sessions.grid_max_bytes())
    };
    assert_eq!(
        sessions("schema = 1\n[db]\ngrid_max_rows = 500\ngrid_max_bytes = 1048576\n"),
        (500, 1_048_576)
    );
    assert_eq!(
        sessions("schema = 1\n[db]\ngrid_max_bytes = 0\n"),
        (100_000, 64 * 1024 * 1024)
    );
    assert_eq!(sessions("schema = 1\n"), (100_000, 64 * 1024 * 1024));
}

/// ③ read-only 연결의 `DELETE` · 파싱 실패 · `SET` 문장은 드라이버에 닿지 않는다. 조회는 읽기 전용 트랜잭션 안에서 돌고 끝나면 롤백한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_connections_keep_writes_from_the_driver() {
    let e = env(true);
    e.pg.respond("FROM t", FakeResponse::rows(3));
    let mut w = e.f.window();
    let s = e.open(&mut w, "ro").await;
    for sql in [
        "DELETE FROM t WHERE id = 1",
        "SELEC oops",
        "SET transaction_read_only = off",
        "SET TRANSACTION READ WRITE",
        "RESET ALL",
        "SELECT 1; DELETE FROM t",
    ] {
        let err = run(&mut w, exec(&s, sql)).await.unwrap_err();
        assert!(denied(&err), "{sql}: {err:?}");
        assert_eq!(reason(&err), Some("readOnlyConnection"));
    }
    assert!(executed(&e.pg).is_empty(), "{:?}", e.pg.calls());
    let items = run(&mut w, exec(&s, "SELECT * FROM t")).await.unwrap();
    assert_eq!(items.last().unwrap()["kind"], "done");
    let calls = e.pg.calls_of(0);
    let begin = calls
        .iter()
        .position(|c| *c == FakeCall::Begin { read_only: true });
    let execute = calls.iter().position(|c| matches!(c, FakeCall::Execute(_)));
    let rollback = calls.iter().rposition(|c| *c == FakeCall::Rollback);
    assert!(begin < execute && execute < rollback, "{calls:?}");
}

/// ④ 확인 해시: 범위 없는 쓰기는 `confirmed` 없이 3001 이고 드라이버에 닿지 않는다. 등급이 낮거나 한 글자 다른 SQL · 다른 세션의
/// 확인도 거절, 프로덕션 파괴는 이름이 틀리면 거절. 맞는 확인은 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmations_bind_to_the_sql_the_session_and_the_name() {
    let e = env(true);
    e.pg.respond("DELETE", FakeResponse::write(5));
    e.pg.respond("DROP", FakeResponse::write(0));
    let mut w = e.f.window();
    let s1 = e.open(&mut w, "dev").await;
    let s2 = e.open(&mut w, "dev").await;

    let asked = run(&mut w, exec(&s1, "DELETE FROM t")).await.unwrap_err();
    assert_eq!(asked.code, code::CONFIRM_REQUIRED);
    let data = asked.data.clone().unwrap();
    assert_eq!(data["grade"], "unboundedWrite");
    assert!(executed(&e.pg).is_empty());
    let confirmed = json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });

    // 한 글자 바꾼 SQL · 다른 세션 · 낮은 등급.
    for (session, sql, conf) in [
        (&s1, "DELETE FROM u", confirmed.clone()),
        (&s2, "DELETE FROM t", confirmed.clone()),
        (
            &s1,
            "DELETE FROM t",
            json!({ "grade": "write", "statementsHash": data["statementsHash"] }),
        ),
    ] {
        let err = run(
            &mut w,
            json!({ "sessionId": session, "sql": sql, "confirmed": conf }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, code::CONFIRM_REQUIRED, "{sql}");
        assert!(err.data.unwrap()["rejected"].is_string());
    }
    assert!(executed(&e.pg).is_empty());

    // db.sql.classify 가 같은 해시를 준다(창이 미리 보이는 확인).
    let classified = w
        .call(
            "db.sql.classify",
            json!({ "projectId": e.f.id, "sessionId": s1, "sql": "DELETE FROM t" }),
        )
        .await
        .unwrap();
    assert_eq!(classified["statementsHash"], data["statementsHash"]);

    let items = run(
        &mut w,
        json!({ "sessionId": s1, "sql": "DELETE FROM t", "confirmed": confirmed }),
    )
    .await
    .unwrap();
    assert_eq!(items.last().unwrap()["rowsAffected"], 5);
    assert_eq!(executed(&e.pg), ["DELETE FROM t"]);

    // 프로덕션 파괴: 이름을 쳐야 한다.
    let p = e.open(&mut w, "prod").await;
    let asked = run(&mut w, exec(&p, "DROP TABLE t")).await.unwrap_err();
    let data = asked.data.unwrap();
    assert_eq!(data["nameRequired"], true);
    for name in [json!(null), json!("prod"), json!("dev")] {
        let err = run(
            &mut w,
            json!({ "sessionId": p, "sql": "DROP TABLE t",
                    "confirmed": { "grade": "destructive", "statementsHash": data["statementsHash"], "name": name } }),
        )
        .await
        .unwrap_err();
        assert_eq!(reason_rejected(&err), Some("nameMismatch"), "{name}");
    }
    run(
        &mut w,
        json!({ "sessionId": p, "sql": "DROP TABLE t",
                "confirmed": { "grade": "destructive", "statementsHash": data["statementsHash"], "name": "prod db" } }),
    )
    .await
    .unwrap();
    // 프로덕션 연결은 범위 있는 쓰기도 확인한다.
    let err = run(&mut w, exec(&p, "UPDATE t SET a = 1 WHERE id = 1"))
        .await
        .unwrap_err();
    assert_eq!(err.code, code::CONFIRM_REQUIRED);
}

fn reason_rejected(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("rejected")?.as_str()
}

/// 관문은 요청이 아니라 연결의 방언으로 분류한다: MySQL 연결의 `/*! DROP TABLE t */` 는 파괴, PG 연결에서는 조회다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gate_classifies_with_the_connection_dialect() {
    let e = env(true);
    let mut w = e.f.window();
    let my = e.open(&mut w, "my").await;
    let pg = e.open(&mut w, "dev").await;
    let sql = "/*! DROP TABLE t */ SELECT 1";
    let err = run(&mut w, exec(&my, sql)).await.unwrap_err();
    assert_eq!(err.code, code::CONFIRM_REQUIRED);
    assert_eq!(err.data.unwrap()["grade"], "destructive");
    assert!(executed(&e.my).is_empty());
    run(&mut w, exec(&pg, sql)).await.unwrap();
    // 분류 RPC 도 연결의 방언을 쓰고, `dialect` 는 연결이 없을 때만.
    let by_connection = w
        .call(
            "db.sql.classify",
            json!({ "projectId": e.f.id, "connectionId": "my", "dialect": "postgres", "sql": sql }),
        )
        .await
        .unwrap();
    assert_eq!(by_connection["grade"], "destructive");
    let by_dialect = w
        .call(
            "db.sql.classify",
            json!({ "projectId": e.f.id, "dialect": "postgres", "sql": sql }),
        )
        .await
        .unwrap();
    assert_eq!(by_dialect["grade"], "read");
}

/// ⑤ 다른 프로젝트 창 · 웰컴 · 파일 창의 세션 · 실행 RPC 는 `PERMISSION_DENIED` 이고 드라이버가 불리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_windows_cannot_open_or_drive_sessions() {
    let e = env(true);
    e.pg.respond("FROM slow", FakeResponse::rows(10).hanging());
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let stream = w
        .start("db.query.execute", exec(&s, "SELECT * FROM slow"))
        .await
        .unwrap();
    w.credit(stream, 3);
    let StreamMsg::Chunk(accepted) = w.next(stream).await else {
        panic!()
    };
    let execution = accepted["executionId"].as_str().unwrap().to_owned();
    // started · columns 뒤 실행 태스크가 `fetch` 에서 멈출 때까지 — 그 뒤로는 이 창이 부르는 것이 없다.
    w.next(stream).await;
    w.next(stream).await;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !e
        .pg
        .calls()
        .iter()
        .any(|c| matches!(c, FakeCall::Fetch { .. }))
    {
        assert!(Instant::now() < deadline, "the run never reached fetch");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let before = e.pg.calls();

    let file = e.f.root.join("a.sql");
    fs::write(&file, "").unwrap();
    let scopes = [
        ConnectionScope::Project(e.f.other.clone()),
        ConnectionScope::Host,
        pw_rpc::Caller::file(pw_rpc::Role::Owner, file).scope,
    ];
    for scope in scopes {
        let mut other = e.f.connect(scope.clone());
        let id = &e.f.id;
        for (method, params) in [
            (
                "db.session.open",
                json!({ "projectId": id, "connectionId": "dev" }),
            ),
            (
                "db.query.page",
                json!({ "projectId": id, "connectionId": "dev", "sql": "SELECT 1", "limit": 5 }),
            ),
            ("db.session.commit", json!({ "sessionId": s })),
            ("db.session.rollback", json!({ "sessionId": s })),
            (
                "db.session.setMode",
                json!({ "sessionId": s, "mode": "manual" }),
            ),
            ("db.session.close", json!({ "sessionId": s })),
            ("db.query.cancel", json!({ "executionId": execution })),
            (
                "db.connection.test",
                json!({ "projectId": id, "connectionId": "dev" }),
            ),
        ] {
            let err = other.call(method, params).await.unwrap_err();
            assert!(denied(&err), "{scope:?} {method}: {err:?}");
        }
        let err = match other.start("db.query.execute", exec(&s, "SELECT 1")).await {
            Err(err) => err,
            Ok(stream) => match other.collect(stream).await.1 {
                StreamOutcome::Failed { error } => error,
                outcome => panic!("{outcome:?}"),
            },
        };
        assert!(denied(&err), "{scope:?} execute: {err:?}");
    }
    assert_eq!(e.pg.calls(), before, "no driver call from another window");
    // 그 창의 실행은 그대로 돈다.
    w.call("db.query.cancel", json!({ "executionId": execution }))
        .await
        .unwrap();
}

/// ⑤ 미신뢰 프로젝트의 `db.session.open` · `db.connection.test` 는 `untrusted`. 신뢰를 거두면 열린 세션의 실행도 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_projects_open_nothing() {
    let e = env(false);
    let mut w = e.f.window();
    for (method, params) in [
        (
            "db.session.open",
            json!({ "projectId": e.f.id, "connectionId": "dev" }),
        ),
        (
            "db.connection.test",
            json!({ "projectId": e.f.id, "connectionId": "dev" }),
        ),
        (
            "db.query.page",
            json!({ "projectId": e.f.id, "connectionId": "dev", "sql": "SELECT 1", "limit": 1 }),
        ),
    ] {
        let err = w.call(method, params).await.unwrap_err();
        assert_eq!(reason(&err), Some("untrusted"), "{method}");
    }
    assert!(e.pg.calls().is_empty());

    e.f.projects.set_trust(&e.f.id, true).unwrap();
    let s = e.open(&mut w, "dev").await;
    let server = w
        .call(
            "db.connection.test",
            json!({ "projectId": e.f.id, "connectionId": "dev" }),
        )
        .await
        .unwrap();
    assert_eq!(server["version"], "fake 1.0");
    e.f.projects.set_trust(&e.f.id, false).unwrap();
    let err = run(&mut w, exec(&s, "SELECT 1")).await.unwrap_err();
    assert_eq!(reason(&err), Some("untrusted"));
    assert!(executed(&e.pg).is_empty());
}

/// ⑥ 취소 뒤 1 s 안에 스트림이 `error { cancelled }` 로 끝나고 드라이버 인터럽트가 불렸다. 세션은 다음 실행을 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_interrupts_the_driver_and_ends_the_stream() {
    let e = env(true);
    e.pg.respond("FROM slow", FakeResponse::rows(10).hanging());
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let stream = w
        .start("db.query.execute", exec(&s, "SELECT * FROM slow"))
        .await
        .unwrap();
    w.credit(stream, 100);
    let StreamMsg::Chunk(accepted) = w.next(stream).await else {
        panic!()
    };
    // started · columns 까지 온 뒤(드라이버가 fetch 에서 멈췄다).
    w.next(stream).await;
    w.next(stream).await;
    let asked = Instant::now();
    w.call(
        "db.query.cancel",
        json!({ "executionId": accepted["executionId"] }),
    )
    .await
    .unwrap();
    let StreamMsg::Chunk(error) = w.next(stream).await else {
        panic!("no error item")
    };
    assert_eq!(error["kind"], "error");
    assert_eq!(error["error"]["kind"], "cancelled");
    assert!(matches!(
        w.next(stream).await,
        StreamMsg::End(StreamOutcome::Completed)
    ));
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "{:?}",
        asked.elapsed()
    );
    assert_eq!(e.pg.count(&FakeCall::Interrupt), 1);
    run(&mut w, exec(&s, "SELECT 1")).await.unwrap();

    // 스트림 취소(창이 스트림을 버림)도 같다.
    let stream = w
        .start("db.query.execute", exec(&s, "SELECT * FROM slow"))
        .await
        .unwrap();
    w.credit(stream, 3);
    for _ in 0..3 {
        w.next(stream).await;
    }
    w.cancel_stream(stream);
    assert!(matches!(
        w.next(stream).await,
        StreamMsg::End(StreamOutcome::Cancelled)
    ));
    let deadline = Instant::now() + Duration::from_secs(1);
    while e.pg.count(&FakeCall::Interrupt) < 2 {
        assert!(
            Instant::now() < deadline,
            "the stream cancel did not interrupt"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // 세션 잠금이 풀린다(정리 뒤).
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match run(&mut w, exec(&s, "SELECT 1")).await {
            Ok(_) => break,
            Err(err) if reason(&err) == Some("busy") && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => panic!("{err:?}"),
        }
    }
}

/// ⑦ 창(RPC 연결)을 닫으면 그 창의 세션이 모두 닫힌다 — 다른 창의 세션은 그대로.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_window_closes_its_sessions() {
    let e = env(true);
    let mut w = e.f.window();
    let mut keep = e.f.window();
    e.open(&mut w, "dev").await;
    e.open(&mut w, "my").await;
    let kept = e.open(&mut keep, "dev").await;
    assert_eq!(e.sessions.session_count(), 3);
    drop(w);
    let deadline = Instant::now() + Duration::from_secs(2);
    while e.sessions.session_count() > 1 {
        assert!(Instant::now() < deadline, "sessions left open");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(e.pg.count(&FakeCall::Close), 1);
    assert_eq!(e.my.count(&FakeCall::Close), 1);
    run(&mut keep, exec(&kept, "SELECT 1")).await.unwrap();
}

/// ⑧ `db.query.page`: 쓰기 · 파싱 실패 문장 거절(연결도 열지 않는다), 읽기 전용 트랜잭션 안에서 돌고, `limit` 넘는 행을 돌려주지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pages_are_read_only_and_limited() {
    let e = env(true);
    e.pg.respond("FROM t", FakeResponse::rows(50));
    let mut w = e.f.window();
    let page = |sql: &str, limit: u32| json!({ "projectId": e.f.id, "connectionId": "dev", "sql": sql, "limit": limit });
    for sql in [
        "DELETE FROM t WHERE id = 1",
        "SELEC oops",
        "SET a = 1",
        "SELECT 1; SELECT 2",
        "BEGIN",
    ] {
        let err = w.call("db.query.page", page(sql, 5)).await.unwrap_err();
        assert!(
            denied(&err) || err.code == rpc_code::INVALID_PARAMS,
            "{sql}: {err:?}"
        );
    }
    assert!(e.pg.calls().is_empty(), "{:?}", e.pg.calls());
    let got = w
        .call("db.query.page", page("SELECT * FROM t", 20))
        .await
        .unwrap();
    assert_eq!(got["rows"].as_array().unwrap().len(), 20);
    assert_eq!(got["truncated"], true);
    assert_eq!(got["columns"].as_array().unwrap().len(), 2);
    let calls = e.pg.calls_of(0);
    assert_eq!(calls[1], FakeCall::Begin { read_only: true }, "{calls:?}");
    assert!(calls.contains(&FakeCall::Rollback));
    assert_eq!(calls.last(), Some(&FakeCall::Close));
    assert!(e.pg.rows_read() <= 21);
    let all = w
        .call("db.query.page", page("SELECT * FROM t", 50))
        .await
        .unwrap();
    assert_eq!(all["rows"].as_array().unwrap().len(), 50);
    assert_eq!(all["truncated"], false);
    assert_eq!(
        w.call("db.query.page", page("SELECT 1", 0))
            .await
            .unwrap_err()
            .code,
        rpc_code::INVALID_PARAMS
    );
}

/// 사람이 친 `BEGIN` · `COMMIT` · `ROLLBACK` 은 드라이버 메서드로 옮긴다 — 드라이버는 이 문장을 SQL 로 받지 않는다. `PREPARE
/// TRANSACTION` 은 아무것도 돌리기 전에 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transaction_statements_become_driver_methods() {
    let e = env(true);
    e.pg.respond("INSERT", FakeResponse::write(1));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    run(
        &mut w,
        exec(
            &s,
            "BEGIN; INSERT INTO t VALUES (1); COMMIT AND CHAIN; INSERT INTO t VALUES (2); ROLLBACK",
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        executed(&e.pg),
        ["INSERT INTO t VALUES (1)", "INSERT INTO t VALUES (2)"]
    );
    assert_eq!(e.pg.committed(), ["INSERT INTO t VALUES (1)"]);
    let calls = e.pg.calls_of(0);
    assert_eq!(
        calls
            .iter()
            .filter(|c| matches!(c, FakeCall::Begin { .. }))
            .count(),
        2
    );
    assert!(calls.contains(&FakeCall::Commit) && calls.contains(&FakeCall::Rollback));
    let Received::Event(event) = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.payload["session"]["state"]["kind"], "idle");

    let err = run(
        &mut w,
        exec(&s, "INSERT INTO t VALUES (3); PREPARE TRANSACTION 'x'"),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, rpc_code::INVALID_PARAMS);
    assert_eq!(executed(&e.pg).len(), 2, "nothing ran");
}

/// 수동 커밋 탭: 쉬어도 저절로 롤백하지 않고, 트랜잭션이 열려 있으면 자동으로 바꾸지 않는다. 실패한 트랜잭션의 커밋은 롤백 + 알림.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_transactions_end_only_by_the_human() {
    let e = env(true);
    e.pg.respond("INSERT", FakeResponse::write(1));
    e.pg.respond(
        "BAD",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "boom").with_code("23505")),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    w.call(
        "db.session.setMode",
        json!({ "sessionId": s, "mode": "manual" }),
    )
    .await
    .unwrap();
    run(&mut w, exec(&s, "INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let err = w
        .call(
            "db.session.setMode",
            json!({ "sessionId": s, "mode": "auto" }),
        )
        .await
        .unwrap_err();
    assert_eq!(reason(&err), Some("transactionOpen"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!e.pg.calls().contains(&FakeCall::Rollback));

    // PG: 오류 뒤 실패한 트랜잭션 — 커밋하면 롤백되고 알린다.
    let items = run(&mut w, exec(&s, "UPDATE BAD SET a = 1 WHERE id = 1"))
        .await
        .unwrap();
    assert_eq!(items.last().unwrap()["kind"], "error");
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    let err = w
        .call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap_err();
    assert_eq!(err.code, code::DB_ERROR);
    let data = err.data.unwrap();
    assert_eq!(data["error"]["rolledBack"], true);
    assert_eq!(data["session"]["state"]["kind"], "idle");
    let Received::Event(event) = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.payload["notice"]["kind"], "rolledBack");
    assert!(e.pg.committed().is_empty());
}

/// MySQL 암묵 커밋 문장은 열린 트랜잭션에서 확인을 받는다(효과 `implicitCommit`). 실행 뒤 세션은 idle 이고 「암묵 커밋」 을 알린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_implicit_commits_are_confirmed_and_reported() {
    let e = env(true);
    e.my.respond("INSERT", FakeResponse::write(1));
    e.my.respond(
        "LOCK",
        FakeResponse::write(0).ending(FakeEnding::ImplicitCommit),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "my").await;
    // 열린 트랜잭션이 없는 자동 커밋 탭에서는 쓰기 등급 그대로 돈다.
    run(&mut w, exec(&s, "LOCK TABLES t WRITE")).await.unwrap();
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let asked = run(&mut w, exec(&s, "LOCK TABLES t WRITE"))
        .await
        .unwrap_err();
    assert_eq!(asked.code, code::CONFIRM_REQUIRED);
    let data = asked.data.unwrap();
    assert_eq!(data["effects"], json!(["implicitCommit"]));
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    run(
        &mut w,
        json!({ "sessionId": s, "sql": "LOCK TABLES t WRITE",
                "confirmed": { "grade": data["grade"], "statementsHash": data["statementsHash"] } }),
    )
    .await
    .unwrap();
    assert_eq!(
        e.my.committed(),
        [
            "LOCK TABLES t WRITE",
            "INSERT INTO t VALUES (1)",
            "LOCK TABLES t WRITE"
        ]
    );
    let Received::Event(event) = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.payload["notice"]["kind"], "committedImplicitly");
    assert_eq!(event.payload["session"]["state"]["kind"], "idle");

    // `SET` 목록 뒤쪽의 `autocommit` 도 암묵 커밋이다 — 확인 없이 돌지 않는다(M3-05 리뷰 L1).
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (2)"))
        .await
        .unwrap();
    let asked = run(&mut w, exec(&s, "SET @a = 1, autocommit = 1"))
        .await
        .unwrap_err();
    assert_eq!(asked.code, code::CONFIRM_REQUIRED);
    assert_eq!(asked.data.unwrap()["effects"], json!(["implicitCommit"]));
    assert!(!executed(&e.my).iter().any(|sql| sql.starts_with("SET")));
}

/// 다음 `db.sessionChanged` 의 알림(알림 없는 이벤트는 건너뛴다).
async fn next_notice(events: &mut pw_core::bus::Subscription) -> Value {
    loop {
        let received = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("no db.sessionChanged")
            .unwrap();
        let Received::Event(event) = received else {
            panic!("lagged")
        };
        if !event.payload["notice"].is_null() {
            return event.payload.clone();
        }
    }
}

/// 드라이버가 되돌렸다고 말하지 않은 채 트랜잭션이 오류로 끝나면(MySQL 의 모르는 전체 롤백 코드 — 3101 등) 롤백이라고 단정하지
/// 않는다 — 서버에 물어 끝난 것을 확인하고 「끝남, 커밋인지 롤백인지 모름」 으로 알린다(M3-05 리뷰 M1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_transaction_that_ends_without_saying_how_is_reported_as_ended() {
    let e = env(true);
    e.my.respond("INSERT", FakeResponse::write(1));
    // 드라이버가 `rolled_back` 을 싣지 않는 전체 롤백: 트랜잭션이 끝났다는 것만 상태로 보인다.
    e.my.respond(
        "UPDATE",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "plugin rollback").with_code("3101"))
            .ending(FakeEnding::RollBackSilently),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "my").await;
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    let items = run(&mut w, exec(&s, "UPDATE t SET a = 1 WHERE id = 1"))
        .await
        .unwrap();
    let error = items.last().unwrap();
    assert_eq!(error["kind"], "error");
    assert_ne!(error["error"]["rolledBack"], true, "{error}");
    let payload = next_notice(&mut events).await;
    assert_eq!(
        payload["notice"],
        json!({ "kind": "transactionEnded", "message": "plugin rollback" })
    );
    assert_eq!(payload["session"]["state"]["kind"], "idle");
    assert_eq!(
        e.my.count(&FakeCall::ReadTransactionState),
        1,
        "asked the server"
    );
}

/// 리뷰어의 재현(M3-05 리뷰 M1): MySQL 수동 트랜잭션의 `INSERT` 뒤 `CALL p()` 가 안에서 커밋한 뒤 실패하거나, `EXECUTE` 한 DDL 이
/// 암묵 커밋한 뒤 실패하면 앞의 쓰기는 커밋됐다 — 「롤백됨」 이라고 알리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_that_commits_and_then_fails_is_not_reported_rolled_back() {
    for (sql, code) in [("CALL p()", "1644"), ("EXECUTE s", "1050")] {
        let e = env(true);
        e.my.respond("INSERT", FakeResponse::write(1));
        e.my.respond(
            sql,
            FakeResponse::error(DbError::new(DbErrorKind::Server, "failed").with_code(code))
                .ending(FakeEnding::ImplicitCommit),
        );
        let mut w = e.f.window();
        let s = e.open(&mut w, "my").await;
        w.call(
            "db.session.setMode",
            json!({ "sessionId": s, "mode": "manual" }),
        )
        .await
        .unwrap();
        run(&mut w, exec(&s, "INSERT INTO t VALUES (1)"))
            .await
            .unwrap();
        let mut events =
            e.f.bus
                .subscribe(Filter::all().topics(["db.sessionChanged"]));
        let items = run(&mut w, exec(&s, sql)).await.unwrap();
        let error = items.last().unwrap();
        assert_eq!(error["error"]["code"], code, "{sql}");
        assert_ne!(error["error"]["rolledBack"], true, "{sql}: {error}");
        let payload = next_notice(&mut events).await;
        assert_eq!(payload["notice"]["kind"], "transactionEnded", "{sql}");
        assert_eq!(payload["session"]["state"]["kind"], "idle", "{sql}");
        assert_eq!(e.my.committed(), ["INSERT INTO t VALUES (1)"], "{sql}");
    }
}

/// 오류 뒤 서버에 상태를 묻지 못하면 「알 수 없음」 이고 세션은 마지막으로 안 상태(열린 트랜잭션) 그대로다. 묻다가 연결이 끊겼으면
/// 「끊김」 이되, 커밋할 수 있는 문장(`CALL`)이었으면 결과를 모른다고 알리고 그냥 쓰기였으면 DB 가 롤백했다고 알린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_state_after_an_error_is_shown_as_unknown() {
    let e = env(true);
    e.my.respond("INSERT", FakeResponse::write(1));
    e.my.respond(
        "UPDATE",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "duplicate").with_code("1062")),
    );
    e.my.respond(
        "CALL",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "failed").with_code("1644")),
    );
    let mut w = e.f.window();
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    // 세션마다 `BEGIN; INSERT` 로 트랜잭션을 연 뒤 `sql` 이 실패하고, 서버에 묻기는 `read` 로 실패한다.
    for (sql, read, notice, state) in [
        (
            "UPDATE t SET a = 1 WHERE id = 1",
            DbError::new(DbErrorKind::Timeout, "no answer"),
            json!({ "kind": "transactionUnknown", "message": "no answer" }),
            "inTransaction",
        ),
        (
            "CALL p()",
            DbError::new(DbErrorKind::Disconnected, "gone"),
            json!({ "kind": "connectionLost", "hadTransaction": true, "outcomeUnknown": true }),
            "disconnected",
        ),
        (
            "UPDATE t SET a = 1 WHERE id = 1",
            DbError::new(DbErrorKind::Disconnected, "gone"),
            json!({ "kind": "connectionLost", "hadTransaction": true, "outcomeUnknown": false }),
            "disconnected",
        ),
    ] {
        e.my.fail_state_reads(None);
        let s = e.open(&mut w, "my").await;
        run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
            .await
            .unwrap();
        e.my.fail_state_reads(Some(read));
        run(&mut w, exec(&s, sql)).await.unwrap();
        let payload = next_notice(&mut events).await;
        assert_eq!(payload["notice"], notice, "{sql}");
        assert_eq!(payload["session"]["state"]["kind"], state, "{sql}");
    }
}

/// 실행과 실행 사이에 트랜잭션이 끝났으면(정리 예산을 넘겨 버려진 문장이 나중에 롤백 — M3-05 리뷰 L2) [커밋] 은 성공으로 답하지
/// 않고, 다음 실행은 「암묵 커밋」 이 아니라 「끝남, 모름」 으로 알린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_transaction_that_ended_between_runs_is_not_reported_committed() {
    let e = env(true);
    e.lite.respond("INSERT", FakeResponse::write(1));
    let mut w = e.f.window();
    let s = e.open(&mut w, "lite").await;
    w.call(
        "db.session.setMode",
        json!({ "sessionId": s, "mode": "manual" }),
    )
    .await
    .unwrap();
    run(&mut w, exec(&s, "INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    e.lite.end_transaction_behind(0);
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    let err = w
        .call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap_err();
    assert_eq!(err.code, code::DB_ERROR);
    assert_eq!(
        err.data.as_ref().unwrap()["session"]["state"]["kind"],
        "idle"
    );
    assert_eq!(e.lite.count(&FakeCall::Commit), 0, "nothing to commit");
    let payload = next_notice(&mut events).await;
    assert_eq!(payload["notice"]["kind"], "transactionEnded");
    assert!(e.lite.committed().is_empty());

    // 다음 실행 앞에서 끝난 것도 같다.
    run(&mut w, exec(&s, "INSERT INTO t VALUES (2)"))
        .await
        .unwrap();
    e.lite.end_transaction_behind(0);
    run(&mut w, exec(&s, "SELECT 1")).await.unwrap();
    let payload = next_notice(&mut events).await;
    assert_eq!(payload["notice"]["kind"], "transactionEnded");
}

/// PG 의 숨은 블록: 자동 커밋 문장 뒤 연결이 트랜잭션 안이면 세션이 그것을 보이고 알린다(몰래 커밋 · 롤백하지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hidden_transaction_block_is_shown_and_kept() {
    let e = env(true);
    e.pg.respond(
        "HIDDEN",
        FakeResponse::write(1).ending(FakeEnding::OpenBlock),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    run(&mut w, exec(&s, "INSERT INTO HIDDEN VALUES (1)"))
        .await
        .unwrap();
    let Received::Event(event) = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.payload["notice"]["kind"], "transactionOpened");
    assert_eq!(event.payload["session"]["state"]["kind"], "inTransaction");
    assert!(e.pg.committed().is_empty());
    w.call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap();
    assert_eq!(e.pg.committed(), ["INSERT INTO HIDDEN VALUES (1)"]);
}

/// SQLite 수동 트랜잭션 안의 쓰기 취소는 트랜잭션 전체를 되돌린다 — 확인 없이는 3002, 확인하면 취소하고 「롤백됨」 을 알린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_sqlite_write_in_a_transaction_asks_first() {
    let e = env(true);
    e.lite.respond("INSERT", FakeResponse::write(1));
    e.lite.respond("RETURNING", FakeResponse::rows(5).hanging());
    let mut w = e.f.window();
    let s = e.open(&mut w, "lite").await;
    w.call(
        "db.session.setMode",
        json!({ "sessionId": s, "mode": "manual" }),
    )
    .await
    .unwrap();
    run(&mut w, exec(&s, "INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let stream = w
        .start(
            "db.query.execute",
            exec(&s, "UPDATE t SET a = 1 WHERE id = 1 RETURNING *"),
        )
        .await
        .unwrap();
    w.credit(stream, 100);
    let StreamMsg::Chunk(accepted) = w.next(stream).await else {
        panic!()
    };
    w.next(stream).await;
    w.next(stream).await;
    // 쓰기 문장이 드라이버의 `fetch` 에서 돌 때 취소한다(그 전에 온 취소는 아무것도 쓰지 않은 문장을 멈출 뿐이다).
    let deadline = Instant::now() + Duration::from_secs(2);
    while !e
        .lite
        .calls()
        .iter()
        .any(|c| matches!(c, FakeCall::Fetch { .. }))
    {
        assert!(Instant::now() < deadline, "the run never reached fetch");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let execution = accepted["executionId"].clone();
    let err = w
        .call("db.query.cancel", json!({ "executionId": execution }))
        .await
        .unwrap_err();
    assert_eq!(err.code, code::CANCEL_ROLLS_BACK);
    assert_eq!(e.lite.count(&FakeCall::Interrupt), 0);
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    w.call(
        "db.query.cancel",
        json!({ "executionId": execution, "confirmRollback": true }),
    )
    .await
    .unwrap();
    let StreamMsg::Chunk(error) = w.next(stream).await else {
        panic!()
    };
    assert_eq!(error["error"]["rolledBack"], true, "{error}");
    let Received::Event(event) = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.payload["notice"]["kind"], "rolledBack");
    assert_eq!(event.payload["session"]["state"]["kind"], "idle");
    assert!(e.lite.committed().is_empty());
}

/// 연결이 끊기면 세션은 `disconnected` 이고 열린 트랜잭션이 사라졌다고 알린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_connection_is_reported() {
    let e = env(true);
    e.pg.respond("INSERT", FakeResponse::write(1));
    e.pg.respond(
        "GONE",
        FakeResponse::error(DbError::new(
            DbErrorKind::Disconnected,
            "server closed the connection",
        )),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    run(&mut w, exec(&s, "SELECT GONE")).await.unwrap();
    let Received::Event(event) = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(
        event.payload["notice"],
        json!({ "kind": "connectionLost", "hadTransaction": true, "outcomeUnknown": false })
    );
    assert_eq!(event.payload["session"]["state"]["kind"], "disconnected");
    let err = run(&mut w, exec(&s, "SELECT 1")).await.unwrap_err();
    assert_eq!(err.code, code::DB_ERROR);
}

/// COMMIT 도중 연결이 끊기면 결과를 모른다 — 「DB 가 롤백했다」 고 단정하지 않는다(M3-05 리뷰 L3). [커밋] 버튼 · `COMMIT` 문장 둘 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_lost_during_commit_leaves_the_outcome_unknown() {
    for by_statement in [false, true] {
        let e = env(true);
        e.pg.respond("INSERT", FakeResponse::write(1));
        let mut w = e.f.window();
        let s = e.open(&mut w, "dev").await;
        run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
            .await
            .unwrap();
        e.pg.fail_commit(Some(DbError::new(
            DbErrorKind::Disconnected,
            "server closed the connection",
        )));
        let mut events =
            e.f.bus
                .subscribe(Filter::all().topics(["db.sessionChanged"]));
        if by_statement {
            let items = run(&mut w, exec(&s, "COMMIT")).await.unwrap();
            assert_ne!(items.last().unwrap()["error"]["rolledBack"], true);
        } else {
            let err = w
                .call("db.session.commit", json!({ "sessionId": s }))
                .await
                .unwrap_err();
            assert_ne!(err.data.unwrap()["error"]["rolledBack"], true);
        }
        let payload = next_notice(&mut events).await;
        assert_eq!(
            payload["notice"],
            json!({ "kind": "connectionLost", "hadTransaction": true, "outcomeUnknown": true }),
            "by statement: {by_statement}"
        );
    }
}

/// 설정이 바뀐 연결은 저절로 다시 붙지 않고 `configChanged` — 새 실행은 받지 않고 커밋 · 롤백은 받는다. 감시가 듣지 못하는 곳(메인
/// 체크아웃의 `.local.toml` 등)의 변경도 실행 전에 설정을 다시 읽어 잡는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_config_stops_new_runs_but_not_the_open_transaction() {
    let e = env(true);
    e.pg.respond("INSERT", FakeResponse::write(1));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    // 개인 파일로 read_only 를 켠다(파일 감시는 돌지 않는다).
    e.f.write_db("dev.local.toml", "read_only = true\n");
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    let err = run(&mut w, exec(&s, "SELECT 1")).await.unwrap_err();
    assert_eq!(reason(&err), Some("configChanged"));
    let Received::Event(event) = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.payload["notice"]["kind"], "configChanged");
    assert_eq!(event.payload["session"]["state"]["kind"], "configChanged");
    assert!(event.payload["session"]["state"]["transaction"]["since"].is_u64());
    assert_eq!(executed(&e.pg), ["INSERT INTO t VALUES (1)"]);
    w.call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap();
    assert_eq!(e.pg.committed(), ["INSERT INTO t VALUES (1)"]);
}

/// 실행 중 세션에 다른 실행 · 커밋은 `busy`. 닫기는 실행을 멈추고 연결을 닫는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_session_is_busy_until_it_finishes_or_closes() {
    let e = env(true);
    e.pg.respond("FROM slow", FakeResponse::rows(10).hanging());
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let stream = w
        .start("db.query.execute", exec(&s, "SELECT * FROM slow"))
        .await
        .unwrap();
    w.credit(stream, 100);
    for _ in 0..3 {
        w.next(stream).await;
    }
    let err = w
        .call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap_err();
    assert_eq!(reason(&err), Some("busy"));
    let err = run(&mut w, exec(&s, "SELECT 1")).await.unwrap_err();
    assert_eq!(reason(&err), Some("busy"));
    w.call("db.session.close", json!({ "sessionId": s }))
        .await
        .unwrap();
    assert_eq!(e.pg.count(&FakeCall::Interrupt), 1);
    assert!(e.pg.calls_of(0).contains(&FakeCall::Close));
    assert_eq!(e.sessions.session_count(), 0);
    let err = w
        .call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc_code::NOT_FOUND);
}

/// 숨은 블록 안에서 오류가 나면(PG 25P02 — 롤백만 된다) 세션은 `failed` 를 보이고, 커밋하면 롤백되며 그것을 알린다(M3-06 이월의
/// 파이프라인 쪽 — PG 실기는 live 시험의 몫).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_hidden_block_shows_failed_and_rolls_back_on_commit() {
    let e = env(true);
    e.pg.respond(
        "HIDDEN",
        FakeResponse::write(1).ending(FakeEnding::OpenBlock),
    );
    e.pg.respond(
        "BAD",
        FakeResponse::error(
            DbError::new(DbErrorKind::Server, "division by zero").with_code("22012"),
        ),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    run(&mut w, exec(&s, "INSERT INTO HIDDEN VALUES (1)"))
        .await
        .unwrap();
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    let items = run(&mut w, exec(&s, "SELECT 1/0 FROM BAD")).await.unwrap();
    assert_eq!(items.last().unwrap()["kind"], "error");
    let Received::Event(event) = events.recv().await.unwrap() else {
        panic!()
    };
    assert_eq!(event.payload["session"]["state"]["kind"], "failed");
    // 실패한 블록의 다음 문장은 서버가 거절한다(25P02) — 세션은 그대로 failed.
    let items = run(&mut w, exec(&s, "SELECT 2")).await.unwrap();
    assert_eq!(items.last().unwrap()["error"]["code"], "25P02");
    let err = w
        .call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap_err();
    assert_eq!(err.data.unwrap()["error"]["rolledBack"], true);
    assert!(e.pg.committed().is_empty());
}

/// 확인은 그것이 허락한 실행 한 번에 쓰이고 끝난다 — 같은 `confirmed` 를 다시 보내거나, 확인을 받은 뒤 다른 실행이 끼면 다시
/// 묻는다(M3-05 리뷰 L4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmation_is_spent_by_the_run_it_allowed() {
    let e = env(true);
    e.pg.respond("DELETE", FakeResponse::write(5));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let asked = run(&mut w, exec(&s, "DELETE FROM t")).await.unwrap_err();
    let data = asked.data.unwrap();
    let confirmed = json!({ "sessionId": s, "sql": "DELETE FROM t",
        "confirmed": { "grade": data["grade"], "statementsHash": data["statementsHash"] } });
    run(&mut w, confirmed.clone()).await.unwrap();
    let again = run(&mut w, confirmed).await.unwrap_err();
    assert_eq!(again.code, code::CONFIRM_REQUIRED);
    assert_eq!(reason_rejected(&again), Some("hashMismatch"));
    assert_eq!(executed(&e.pg), ["DELETE FROM t"]);

    // 미리 받은 확인(`db.sql.classify`) 뒤에 다른 실행이 끼면 그 확인은 낡았다.
    let classified = w
        .call(
            "db.sql.classify",
            json!({ "projectId": e.f.id, "sessionId": s, "sql": "DELETE FROM t" }),
        )
        .await
        .unwrap();
    run(&mut w, exec(&s, "SELECT 1")).await.unwrap();
    let stale = run(
        &mut w,
        json!({ "sessionId": s, "sql": "DELETE FROM t",
            "confirmed": { "grade": classified["grade"], "statementsHash": classified["statementsHash"] } }),
    )
    .await
    .unwrap_err();
    assert_eq!(reason_rejected(&stale), Some("hashMismatch"));
    assert_eq!(executed(&e.pg), ["DELETE FROM t", "SELECT 1"]);
}

/// 실행마다 설정을 다시 보는 것은 설정 파일의 수정 시각 · 크기가 바뀌었을 때만 읽는다(M3-05 리뷰 L5): 크기와 수정 시각을 그대로
/// 둔 고침은 읽지 않고, 수정 시각이 바뀌면 읽어 `configChanged` 로 잡는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_config_recheck_reads_the_files_only_when_they_change() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    run(&mut w, exec(&s, "SELECT 1")).await.unwrap();
    let path = e.f.db_file("dev.toml");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    // 같은 크기의 다른 이름, 수정 시각은 되돌린다.
    e.f.edit_db("dev.toml", "name = \"dev\"", "name = \"deX\"");
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    run(&mut w, exec(&s, "SELECT 1")).await.unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified + Duration::from_secs(2))
        .unwrap();
    let err = run(&mut w, exec(&s, "SELECT 1")).await.unwrap_err();
    assert_eq!(reason(&err), Some("configChanged"));
}

/// L5 의 비용(release, `mise run test-release`): 연결 파일이 7개든 207개든 실행 한 번의 비용이 같다 — 다시 읽기가 그 연결의 파일
/// 자리만 본다(리뷰 측정: 디버그 빌드에서 0.49 ms → 8.0 ms 로 늘었다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "release timing — mise run test-release"]
async fn the_config_recheck_cost_does_not_grow_with_connection_files() {
    const RUNS: u32 = 200;
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    run(&mut w, exec(&s, "SELECT 1")).await.unwrap();
    let t = Instant::now();
    for _ in 0..RUNS {
        run(&mut w, exec(&s, "SELECT 1")).await.unwrap();
    }
    let few = t.elapsed() / RUNS;
    for i in 0..200 {
        e.f.write_db(
            &format!("c{i}.toml"),
            &format!("schema = 1\nname = \"c{i}\"\ndriver = \"postgres\"\nhost = \"h{i}\"\n"),
        );
    }
    let t = Instant::now();
    for _ in 0..RUNS {
        run(&mut w, exec(&s, "SELECT 1")).await.unwrap();
    }
    let many = t.elapsed() / RUNS;
    eprintln!("per run: 7 connection files {few:?}, 207 connection files {many:?}");
    assert!(
        many < few * 2 + Duration::from_micros(200),
        "{few:?} -> {many:?}"
    );
}

/// 새로고침 · 웹뷰 재시작(M3-05 리뷰 M2): 창의 연결이 닫혀도 열린 트랜잭션이 있는 세션은 닫지 않고 보존한다(알림 `unattended`) —
/// 같은 창의 같은 탭이 새 연결로 `db.session.resume` 하면 「열린 트랜잭션」 그대로 이어지고 커밋할 수 있다. 열린 트랜잭션이 없는
/// 세션은 그대로 닫힌다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reloaded_window_takes_its_open_transaction_back() {
    let e = env(true);
    e.pg.respond("INSERT", FakeResponse::write(1));
    let label = e.label();
    let mut w = e.window_in(&label);
    let s = e.open_tab(&mut w, "dev", "tab-1").await;
    e.open_tab(&mut w, "dev", "tab-2").await;
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    drop(w);
    let payload = next_notice(&mut events).await;
    assert_eq!(payload["notice"]["kind"], "unattended");
    assert!(payload["notice"]["until"].is_u64());
    assert_eq!(payload["session"]["sessionId"], s.as_str());
    let deadline = Instant::now() + Duration::from_secs(2);
    while e.sessions.session_count() > 1 {
        assert!(
            Instant::now() < deadline,
            "the idle tab's session stayed open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(e.pg.count(&FakeCall::Rollback), 0);
    assert_eq!(e.pg.count(&FakeCall::Close), 1, "only the idle tab's");

    let mut w = e.window_in(&label);
    let resumed = w
        .call(
            "db.session.resume",
            json!({ "projectId": e.f.id, "tabId": "tab-1" }),
        )
        .await
        .unwrap();
    assert_eq!(resumed["session"]["sessionId"], s.as_str());
    assert_eq!(resumed["session"]["state"]["kind"], "inTransaction");
    assert_eq!(resumed["name"], "dev");
    // 이미 이어받았으니 다시 찾지 않는다.
    let again = w
        .call(
            "db.session.resume",
            json!({ "projectId": e.f.id, "tabId": "tab-1" }),
        )
        .await
        .unwrap();
    assert!(again.is_null());
    w.call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap();
    assert_eq!(e.pg.committed(), ["INSERT INTO t VALUES (1)"]);
}

/// 보존한 세션은 그 창의 그 탭만 이어받는다 — 같은 프로젝트의 다른 창 · 다른 탭은 `null`, 다른 프로젝트 창 · Host 범위 연결은
/// permission denied(결정 M3-D18).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_windows_and_projects_cannot_take_a_kept_session() {
    let e = env(true);
    e.pg.respond("INSERT", FakeResponse::write(1));
    let label = e.label();
    let mut w = e.window_in(&label);
    let s = e.open_tab(&mut w, "dev", "tab-1").await;
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    drop(w);
    assert_eq!(
        next_notice(&mut events).await["notice"]["kind"],
        "unattended"
    );
    let resume = json!({ "projectId": e.f.id, "tabId": "tab-1" });

    let mut other_window = e.window_in("project-elsewhere");
    let got = other_window
        .call("db.session.resume", resume.clone())
        .await
        .unwrap();
    assert!(got.is_null(), "another window");
    let mut same = e.window_in(&label);
    let got = same
        .call(
            "db.session.resume",
            json!({ "projectId": e.f.id, "tabId": "tab-2" }),
        )
        .await
        .unwrap();
    assert!(got.is_null(), "another tab");
    let other_label = format!("project-{}", e.f.other);
    for mut c in [
        e.f.connect_in(
            ConnectionScope::Project(e.f.other.clone()),
            Some(&other_label),
        ),
        e.f.connect_in(ConnectionScope::Host, Some(&label)),
    ] {
        let err = c
            .call("db.session.resume", resume.clone())
            .await
            .unwrap_err();
        assert!(denied(&err), "{err:?}");
    }
    let got = same.call("db.session.resume", resume).await.unwrap();
    assert_eq!(got["session"]["sessionId"], s.as_str());
}

/// 보존 시간 안에 아무도 붙지 않으면 롤백하고 닫는다 — 몰래가 아니다: 알림(`notification.show`)과, 다음에 그 프로젝트 창이 열릴 때
/// 읽는 기록(`db.session.unattendedRollbacks`)을 남긴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unattended_transaction_is_rolled_back_and_recorded() {
    let e = env(true);
    e.sessions
        .set_unattended_keep(Some(Duration::from_millis(300)));
    e.pg.respond("INSERT", FakeResponse::write(1));
    let label = e.label();
    let mut w = e.window_in(&label);
    let s = e.open_tab(&mut w, "dev", "tab-1").await;
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let mut shown =
        e.f.bus
            .subscribe(Filter::all().topics(["notification.show"]));
    let mut events =
        e.f.bus
            .subscribe(Filter::all().topics(["db.sessionChanged"]));
    drop(w);
    assert_eq!(
        next_notice(&mut events).await["notice"]["kind"],
        "unattended"
    );
    let payload = next_notice(&mut events).await;
    assert_eq!(payload["notice"]["kind"], "unattendedRolledBack");
    assert_eq!(payload["session"]["state"]["kind"], "disconnected");
    let Received::Event(event) = tokio::time::timeout(Duration::from_secs(5), shown.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        event.payload,
        json!({ "level": "warning", "message": "db-unattended-transaction-rolled-back",
                "args": { "connection": "dev", "minutes": "1" } })
    );
    assert_eq!(event.scope, pw_core::bus::Scope::Project(e.f.id.clone()));
    assert_eq!(e.pg.count(&FakeCall::Rollback), 1);
    assert!(e.pg.committed().is_empty());
    assert_eq!(e.sessions.session_count(), 0);

    let mut w = e.window_in(&label);
    let got = w
        .call(
            "db.session.resume",
            json!({ "projectId": e.f.id, "tabId": "tab-1" }),
        )
        .await
        .unwrap();
    assert!(got.is_null());
    let records = w
        .call(
            "db.session.unattendedRollbacks",
            json!({ "projectId": e.f.id }),
        )
        .await
        .unwrap();
    let [record] = records.as_array().unwrap().as_slice() else {
        panic!("{records}")
    };
    assert_eq!(record["connectionId"], "dev");
    assert_eq!(record["tabId"], "tab-1");
    assert_eq!(record["keptMinutes"], 1);
    assert!(record["since"].is_u64() && record["rolledBackAt"].is_u64());
    assert!(record["rollbackError"].is_null());
    let mut other = e.f.connect(ConnectionScope::Project(e.f.other.clone()));
    let err = other
        .call(
            "db.session.unattendedRollbacks",
            json!({ "projectId": e.f.id }),
        )
        .await
        .unwrap_err();
    assert!(denied(&err));
}

/// 창 · 탭을 모르는 연결(원격 등)의 열린 트랜잭션은 보존할 열쇠가 없어 곧바로 롤백하되, 알리고 기록한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_window_without_a_tab_rolls_back_at_once_and_says_so() {
    let e = env(true);
    e.pg.respond("INSERT", FakeResponse::write(1));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    run(&mut w, exec(&s, "BEGIN; INSERT INTO t VALUES (1)"))
        .await
        .unwrap();
    let mut shown =
        e.f.bus
            .subscribe(Filter::all().topics(["notification.show"]));
    drop(w);
    let Received::Event(event) = tokio::time::timeout(Duration::from_secs(5), shown.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        event.payload["message"],
        "db-closed-transaction-rolled-back"
    );
    assert_eq!(event.payload["args"], json!({ "connection": "dev" }));
    assert_eq!(e.pg.count(&FakeCall::Rollback), 1);
    let mut w = e.f.window();
    let records = w
        .call(
            "db.session.unattendedRollbacks",
            json!({ "projectId": e.f.id }),
        )
        .await
        .unwrap();
    assert_eq!(records[0]["keptMinutes"], 0);
    assert!(records[0]["tabId"].is_null());
}
