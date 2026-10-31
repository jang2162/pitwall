//! M3-20 의 설정 창 「DB」 페이지(14-m3-plan.md M3-20 행): Host 가 정본인 `[db]` 설정을 실제 RPC 세션 너머에서 본다.
//! 바꾼 `grid_max_rows` 가 `settings.toml` 에 쓰이고 다음 실행 · 다음 실행(앱을 다시 띄운 것)의 상한이 된다. 범위 판정은 Host 가
//! 하고(0 · 음수 · 아주 큰 값 · 소수 · 글은 쓰지 않는다), 손으로 고친 범위 밖 값은 읽을 때 기본값이다 — 어느 길로도 관문의 상한
//! (행 · 바이트)이 꺼지지 않는다. 셀 상한 · 연결 타임아웃 기본값 · Redis 응답 타임아웃은 드라이버 요청에 실린다.

mod common;

use std::fs;
use std::sync::Arc;

use common::{Conn, Fixture};
use pw_core::bus::Scope;
use pw_core::config::{ConfigSource, ConfigStore};
use pw_db::DbSessions;
use pw_db::config::Connections;
use pw_db_driver::fake::{FakeDriver, FakeResponse};
use pw_db_driver_sqlite::SqliteDriver;
use pw_db_types::SqlDialect;
use pw_rpc::protocol::StreamOutcome;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

struct Env {
    f: Fixture,
    sessions: Arc<DbSessions>,
    pg: FakeDriver,
    global: ConfigStore,
}

fn global_store(f: &Fixture) -> ConfigStore {
    fs::create_dir_all(&f.app.config).unwrap();
    ConfigStore::open(ConfigSource::global(&f.app), f.bus.clone(), Scope::Host)
}

fn env() -> Env {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(pg.clone())).unwrap();
    drivers.register(Arc::new(SqliteDriver::new())).unwrap();
    let mut slot = None;
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    let sessions = slot.unwrap();
    let global = global_store(&f);
    sessions.attach_settings(global.clone());
    f.write_db(
        "dev.toml",
        "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"localhost\"\n",
    );
    f.write_db(
        "slow.toml",
        "schema = 1\nname = \"slow\"\ndriver = \"postgres\"\nhost = \"localhost\"\nconnect_timeout = 20\n",
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
        global,
    }
}

impl Env {
    /// 설정 창(Host 범위)의 연결.
    fn settings_window(&self) -> Conn {
        self.f.connect(ConnectionScope::Host)
    }

    fn file(&self) -> std::path::PathBuf {
        self.f.app.config.join("settings.toml")
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

async fn set(w: &mut Conn, key: &str, value: Value) -> Result<Value, RpcError> {
    w.call("db.settings.set", json!({ "key": key, "value": value }))
        .await
}

fn item<'a>(view: &'a Value, key: &str) -> &'a Value {
    view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == key)
        .unwrap_or_else(|| panic!("no {key} in {view}"))
}

/// 실행해 끝까지 모은다.
async fn run(w: &mut Conn, session: &str, sql: &str) -> Vec<Value> {
    let stream = w
        .start(
            "db.query.execute",
            json!({ "sessionId": session, "sql": sql }),
        )
        .await
        .unwrap();
    let (items, outcome) = w.collect(stream).await;
    assert!(matches!(outcome, StreamOutcome::Completed), "{outcome:?}");
    items
}

fn rows(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .filter(|i| i["kind"] == "rows")
        .flat_map(|i| i["rows"].as_array().unwrap().clone())
        .collect()
}

fn done(items: &[Value]) -> &Value {
    items.iter().rev().find(|i| i["kind"] == "done").unwrap()
}

/// 완료 기준: 설정 페이지에서 바꾼 `grid_max_rows` 가 `settings.toml` 에 쓰이고(그 키만 — 주석 · 다른 키는 그대로) 다음 실행의
/// 상한이 되고, 앱을 다시 띄워(같은 설정 폴더로 새 설정 · 세션) 읽어도 그 값이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_written_grid_max_rows_reaches_settings_toml_and_the_next_run() {
    let e = env();
    fs::write(e.file(), "schema = 1\n# 내 설정\n[ui]\nlanguage = \"ko\"\n").unwrap();
    e.global.reload().unwrap();
    e.pg.respond("FROM t", FakeResponse::rows(10));
    let mut project = e.f.window();
    let s = e.open(&mut project, "dev").await;
    let before = run(&mut project, &s, "SELECT * FROM t").await;
    assert_eq!(rows(&before).len(), 10);

    let mut settings = e.settings_window();
    let view = set(&mut settings, "gridMaxRows", json!(5)).await.unwrap();
    assert_eq!(item(&view, "gridMaxRows")["value"], 5);
    assert_eq!(item(&view, "gridMaxRows")["configured"], true);
    assert_eq!(
        fs::read_to_string(e.file()).unwrap(),
        "schema = 1\n# 내 설정\n[ui]\nlanguage = \"ko\"\n\n[db]\ngrid_max_rows = 5\n"
    );

    let after = run(&mut project, &s, "SELECT * FROM t").await;
    assert_eq!(rows(&after).len(), 5);
    assert_eq!(done(&after)["truncated"], true);

    // 다음 실행: 같은 설정 폴더로 연 새 설정 · 연결 관리자 · 세션.
    let next = Arc::new(Connections::new(
        e.f.store.clone(),
        Arc::new(common::FakeEnv::default()),
    ));
    let sessions = DbSessions::new(next, pw_db::DriverRegistry::new());
    sessions.attach_settings(global_store(&e.f));
    assert_eq!(sessions.grid_max_rows(), 5);
    assert_eq!(e.sessions.grid_max_rows(), 5);

    // `null` 은 그 키를 지우고 기본값으로 돌아간다.
    let view = set(&mut settings, "gridMaxRows", Value::Null)
        .await
        .unwrap();
    assert_eq!(item(&view, "gridMaxRows")["value"], 100_000);
    assert_eq!(item(&view, "gridMaxRows")["configured"], false);
    assert_eq!(e.sessions.grid_max_rows(), 100_000);
    assert!(
        !fs::read_to_string(e.file())
            .unwrap()
            .contains("grid_max_rows")
    );
}

/// 창이 범위 밖 값(0 · 음수 · 아주 큰 값 · 소수 · 글 · 불)을 보내면 Host 가 쓰지 않고 `INVALID_PARAMS` 로 거절한다 — `data` 는
/// 창이 번역할 까닭. 파일은 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_window_cannot_turn_the_limits_off() {
    let e = env();
    let mut w = e.settings_window();
    for (key, value, reason) in [
        (
            "gridMaxRows",
            json!(0),
            json!({ "kind": "range", "min": 1, "max": 1_000_000 }),
        ),
        (
            "gridMaxRows",
            json!(-1),
            json!({ "kind": "range", "min": 1, "max": 1_000_000 }),
        ),
        (
            "gridMaxRows",
            json!(1_000_000_000_000_i64),
            json!({ "kind": "range", "min": 1, "max": 1_000_000 }),
        ),
        (
            "gridMaxRows",
            json!(u64::MAX),
            json!({ "kind": "notInteger" }),
        ),
        ("gridMaxRows", json!(1e9), json!({ "kind": "notInteger" })),
        ("gridMaxRows", json!(10.5), json!({ "kind": "notInteger" })),
        ("gridMaxRows", json!("100"), json!({ "kind": "notInteger" })),
        (
            "gridMaxBytes",
            json!(0),
            json!({ "kind": "range", "min": 1_048_576, "max": 268_435_456 }),
        ),
        (
            "gridMaxBytes",
            json!(i64::MAX),
            json!({ "kind": "range", "min": 1_048_576, "max": 268_435_456 }),
        ),
        (
            "cellMaxBytes",
            json!(0),
            json!({ "kind": "range", "min": 1024, "max": 16_777_216 }),
        ),
        (
            "connectTimeout",
            json!(0),
            json!({ "kind": "range", "min": 1, "max": 600 }),
        ),
        (
            "redisResponseTimeout",
            json!(601),
            json!({ "kind": "range", "min": 1, "max": 600 }),
        ),
        (
            "redisKeySeparator",
            json!(" "),
            json!({ "kind": "separator", "max": 8 }),
        ),
        (
            "redisKeySeparator",
            json!(""),
            json!({ "kind": "separator", "max": 8 }),
        ),
        ("redisKeySeparator", json!(1), json!({ "kind": "notText" })),
    ] {
        let err = set(&mut w, key, value.clone()).await.unwrap_err();
        assert_eq!(err.code, rpc_code::INVALID_PARAMS, "{key} {value}");
        assert_eq!(
            err.data,
            Some(json!({ "key": key, "reason": reason })),
            "{key} {value}"
        );
    }
    assert!(!e.file().exists(), "nothing was written");
    assert_eq!(e.sessions.grid_max_rows(), 100_000);
    assert_eq!(e.sessions.grid_max_bytes(), 64 * 1024 * 1024);

    // 범위 끝은 받는다.
    set(&mut w, "gridMaxRows", json!(1_000_000)).await.unwrap();
    set(&mut w, "gridMaxBytes", json!(268_435_456))
        .await
        .unwrap();
    assert_eq!(e.sessions.grid_max_rows(), 1_000_000);
    assert_eq!(e.sessions.grid_max_bytes(), 268_435_456);
}

/// 사람이 `settings.toml` 을 손으로 고쳐 범위 밖 값을 넣어도 상한은 꺼지지 않는다 — 읽을 때 기본값을 쓰고, 설정 페이지는 그
/// 까닭을 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hand_edited_limits_out_of_range_fall_back_to_the_defaults() {
    let e = env();
    let mut w = e.settings_window();
    for (rows, bytes) in [
        ("0", "0"),
        ("-5", "-1"),
        ("1000000000000000", "9223372036854775807"),
        ("\"all\"", "1.5"),
        ("1.0e9", "false"),
    ] {
        fs::write(
            e.file(),
            format!("schema = 1\n[db]\ngrid_max_rows = {rows}\ngrid_max_bytes = {bytes}\n"),
        )
        .unwrap();
        e.global.reload().unwrap();
        assert_eq!(e.sessions.grid_max_rows(), 100_000, "{rows}");
        assert_eq!(e.sessions.grid_max_bytes(), 64 * 1024 * 1024, "{bytes}");
        let view = w.call("db.settings.get", Value::Null).await.unwrap();
        let shown = item(&view, "gridMaxRows");
        assert_eq!(shown["value"], 100_000);
        assert_eq!(shown["configured"], true);
        assert!(shown["refused"].is_object(), "{shown}");
    }
}

/// 셀 상한(`cell_max_bytes`)은 실행의 드라이버 요청에 실린다 — 실제 SQLite 에서 그보다 긴 값이 잘려 온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cell_limit_reaches_the_driver() {
    let e = env();
    let mut project = e.f.window();
    let s = e.open(&mut project, "lite").await;
    let sql = "SELECT printf('%.*c', 5000, 'x') AS v";
    let whole = run(&mut project, &s, sql).await;
    assert_eq!(rows(&whole)[0][0]["text"].as_str().unwrap().len(), 5000);

    let mut settings = e.settings_window();
    set(&mut settings, "cellMaxBytes", json!(1024))
        .await
        .unwrap();
    let cut = run(&mut project, &s, sql).await;
    let cell = &rows(&cut)[0][0]["truncated"];
    assert_eq!(cell["size"], 5000, "{cut:?}");
    assert_eq!(cell["prefix"].as_str().unwrap().len(), 1024);
}

/// 연결 설정에 `connect_timeout` 이 없으면 전역 설정의 연결 타임아웃 기본값을, Redis 응답 타임아웃은 전역 설정을 드라이버 입력에
/// 싣는다. 연결 설정에 적힌 `connect_timeout` 이 먼저다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_timeouts_reach_the_connection_spec() {
    let e = env();
    let mut project = e.f.window();
    e.open(&mut project, "dev").await;
    let first = e.pg.connect_specs().pop().unwrap();
    assert_eq!(first.connect_timeout_ms, 10_000);
    assert_eq!(first.response_timeout_ms, Some(30_000));

    let mut settings = e.settings_window();
    set(&mut settings, "connectTimeout", json!(7))
        .await
        .unwrap();
    set(&mut settings, "redisResponseTimeout", json!(3))
        .await
        .unwrap();
    e.open(&mut project, "dev").await;
    let changed = e.pg.connect_specs().pop().unwrap();
    assert_eq!(changed.connect_timeout_ms, 7_000);
    assert_eq!(changed.response_timeout_ms, Some(3_000));

    e.open(&mut project, "slow").await;
    assert_eq!(
        e.pg.connect_specs().pop().unwrap().connect_timeout_ms,
        20_000
    );
}

/// 키 구분자는 글로 쓰이고 설정 페이지 · Redis 화면이 읽는 값이 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_key_separator_is_written_as_text() {
    let e = env();
    let mut w = e.settings_window();
    let view = w.call("db.settings.get", Value::Null).await.unwrap();
    assert_eq!(item(&view, "redisKeySeparator")["value"], ":");
    let view = set(&mut w, "redisKeySeparator", json!("::")).await.unwrap();
    assert_eq!(item(&view, "redisKeySeparator")["value"], "::");
    assert_eq!(
        fs::read_to_string(e.file()).unwrap(),
        "schema = 1\n\n[db]\nredis_key_separator = \"::\"\n"
    );
    let mut project = e.f.window();
    let seen = project.call("db.settings.get", Value::Null).await.unwrap();
    assert_eq!(item(&seen, "redisKeySeparator")["value"], "::");
}
