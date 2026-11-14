//! 실 DB 로 에이전트의 `db_query`(`db.tool.query`)를 본다(16-m5-plan.md M5-07 의 live 완료 기준): `sql` · `file` 왕복과, 분류를
//! 지난 부작용 함수(분류기가 모르는 사용자 함수가 쓰기를 한다)가 DB 수준 읽기 전용 트랜잭션에서 막히는 것. `PW_TEST_PG_URL` ·
//! `PW_TEST_MYSQL_URL` 이 있을 때만 목록에 생긴다(`mise run test-db -- pg mysql mariadb` — MariaDB 는 MySQL 시험을 그 URL 로 돈다).
//! SQLite 는 서버가 없어 `tests/query_tool.rs` 의 보통 시험이 실 드라이버로 본다.
#![cfg(any(pw_live_pg, pw_live_mysql))]

mod common;

use std::sync::Arc;

use common::{Conn, Fixture};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::live::{LiveDb, endpoint};
use pw_rpc::protocol::StreamOutcome;
use pw_rpc::{ConnectionScope, RpcError};
use serde_json::{Value, json};

/// 시험 URL 의 서버를 연결 `dev` 로 둔 Host 와 그 프로젝트 창(`tests/live_agent.rs` 와 같다).
async fn host(live: LiveDb, driver: Arc<dyn pw_db_driver::DbDriver>) -> (Fixture, Conn) {
    let url = live.url();
    let end = endpoint(live, &url).unwrap();
    let rest = url.split_once("://").unwrap().1;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let user = authority
        .rsplit_once('@')
        .map(|(info, _)| info.split(':').next().unwrap().to_owned())
        .unwrap();
    let database = path.split(['?', '#']).next().unwrap_or_default().to_owned();
    let id = driver.id().to_owned();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(driver).unwrap();
    let f = Fixture::build(true, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    f.write_db(
        "dev.toml",
        &format!(
            "schema = 1\nname = \"dev\"\ndriver = \"{id}\"\nhost = \"{}\"\nport = {}\nuser = \"{user}\"\ndatabase = \"{database}\"\nsslmode = \"disable\"\npassword = {{ secret_ref = \"keychain\" }}\n",
            end.host, end.port
        ),
    );
    let mut w = f.window();
    if let Some(password) = &end.password {
        w.set_password(&f.id, "dev", password).await.unwrap();
    }
    (f, w)
}

async fn run(w: &mut Conn, params: Value) -> Result<Vec<Value>, RpcError> {
    let stream = w.start("db.query.execute", params).await?;
    let (items, outcome) = w.collect(stream).await;
    match outcome {
        StreamOutcome::Completed => Ok(items),
        StreamOutcome::Failed { error } => Err(error),
        StreamOutcome::Cancelled => panic!("cancelled"),
    }
}

/// 실행해 끝까지 모은다. 확인을 물으면 받은 해시로 한 번 확인한다(창이 사람의 확인을 받은 흐름).
async fn run_confirmed(w: &mut Conn, session: &str, sql: &str) {
    let params = json!({ "sessionId": session, "sql": sql });
    match run(w, params.clone()).await {
        Ok(_) => {}
        Err(asked) if asked.code == code::CONFIRM_REQUIRED => {
            let data = asked.data.unwrap();
            let mut params = params;
            params["confirmed"] =
                json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });
            run(w, params).await.unwrap();
        }
        Err(e) => panic!("{sql}: {e:?}"),
    }
}

/// 사람이 세션을 열고 시험 테이블 · 행을 만든다(`setup` 은 그 뒤에 더 돌릴 문장들). 세션 id.
async fn prepare(w: &mut Conn, f: &Fixture, table: &str, setup: &[String]) -> String {
    let opened = w
        .call(
            "db.session.open",
            json!({ "projectId": f.id, "connectionId": "dev" }),
        )
        .await
        .unwrap();
    let session = opened["session"]["sessionId"].as_str().unwrap().to_owned();
    run_confirmed(w, &session, &format!("DROP TABLE IF EXISTS {table}")).await;
    run_confirmed(
        w,
        &session,
        &format!("CREATE TABLE {table} (id INT PRIMARY KEY, note VARCHAR(20))"),
    )
    .await;
    run_confirmed(
        w,
        &session,
        &format!("INSERT INTO {table} (id, note) VALUES (1, 'one'), (2, 'two'), (3, 'three')"),
    )
    .await;
    for sql in setup {
        run_confirmed(w, &session, sql).await;
    }
    session
}

fn query(f: &Fixture, extra: Value) -> Value {
    let mut params = json!({ "projectId": f.id, "connectionId": "dev" });
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    params
}

/// `sql`(두 문장 — 결과 둘, 행 상한)과 `file`(프로젝트 루트 기준 — 결과 둘) 왕복.
async fn round_trip(live: LiveDb, driver: Arc<dyn pw_db_driver::DbDriver>, table: &str) {
    let (f, mut w) = host(live, driver).await;
    let session = prepare(&mut w, &f, table, &[]).await;
    let mut agent = f.connect(ConnectionScope::Agent);
    let sql =
        format!("SELECT id, note FROM {table} ORDER BY id;\nSELECT count(*) AS n FROM {table}");
    let out = agent
        .call(
            "db.tool.query",
            query(&f, json!({ "sql": sql, "maxRows": 2 })),
        )
        .await
        .unwrap();
    assert_eq!(out["error"], Value::Null, "{out}");
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(results[0]["truncated"], true);
    assert_eq!(results[0]["rows"][1][1], json!({ "text": "two" }));
    assert_eq!(results[1]["line"], 2);

    std::fs::create_dir_all(f.root.join("queries")).unwrap();
    std::fs::write(
        f.root.join("queries/report.sql"),
        format!("-- report\nSELECT note FROM {table} WHERE id = 3;\nSELECT max(id) AS top FROM {table};\n"),
    )
    .unwrap();
    let out = agent
        .call(
            "db.tool.query",
            query(&f, json!({ "file": "queries/report.sql" })),
        )
        .await
        .unwrap();
    assert_eq!(out["error"], Value::Null, "{out}");
    assert_eq!(out["results"].as_array().unwrap().len(), 2);
    assert_eq!(out["results"][0]["rows"][0][0], json!({ "text": "three" }));
    assert_eq!(out["file"], "queries/report.sql");
    run_confirmed(&mut w, &session, &format!("DROP TABLE {table}")).await;
}

/// 분류기가 모르는 사용자 함수는 조회로 분류된다 — 그 함수가 쓰기를 하면 DB 수준 읽기 전용 트랜잭션이 막는다(문장 오류 `readOnly`,
/// 테이블은 그대로).
async fn read_only_blocks(
    live: LiveDb,
    driver: Arc<dyn pw_db_driver::DbDriver>,
    table: &str,
    function: &str,
    create: String,
) {
    let (f, mut w) = host(live, driver).await;
    let drop = format!("DROP FUNCTION IF EXISTS {function}");
    let session = prepare(&mut w, &f, table, &[drop.clone(), create]).await;
    let mut agent = f.connect(ConnectionScope::Agent);
    let out = agent
        .call(
            "db.tool.query",
            query(
                &f,
                json!({ "sql": format!("SELECT 1 AS first; SELECT {function}() AS bumped") }),
            ),
        )
        .await
        .unwrap();
    assert_eq!(out["results"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(out["results"][0]["grade"], "read");
    assert_eq!(out["error"]["index"], 1, "{out}");
    assert_eq!(out["error"]["error"]["kind"], "readOnly", "{out}");
    let after = agent
        .call(
            "db.tool.query",
            query(
                &f,
                json!({ "sql": format!("SELECT count(*) FROM {table}") }),
            ),
        )
        .await
        .unwrap();
    let count = &after["results"][0]["rows"][0][0];
    assert_eq!(count, &json!({ "int": 3 }), "{after}");
    run_confirmed(&mut w, &session, &drop).await;
    run_confirmed(&mut w, &session, &format!("DROP TABLE {table}")).await;
}

#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_db_query_round_trips_sql_and_file() {
    round_trip(
        LiveDb::Pg,
        Arc::new(pw_db_driver_pg::PgDriver::new()),
        "m5_07_query_probe",
    )
    .await;
}

#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_db_query_side_effect_function_hits_the_read_only_transaction() {
    let (table, function) = ("m5_07_bump_probe", "m5_07_bump");
    read_only_blocks(
        LiveDb::Pg,
        Arc::new(pw_db_driver_pg::PgDriver::new()),
        table,
        function,
        format!(
            "CREATE FUNCTION {function}() RETURNS int LANGUAGE sql AS $$ INSERT INTO {table} (id, note) VALUES (99, 'x') RETURNING 1 $$"
        ),
    )
    .await;
}

#[cfg(pw_live_mysql)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_mysql_db_query_round_trips_sql_and_file() {
    round_trip(
        LiveDb::Mysql,
        Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
        "m5_07_query_probe",
    )
    .await;
}

#[cfg(pw_live_mysql)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_mysql_db_query_side_effect_function_hits_the_read_only_transaction() {
    let (table, function) = ("m5_07_bump_probe", "m5_07_bump");
    read_only_blocks(
        LiveDb::Mysql,
        Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
        table,
        function,
        format!(
            "CREATE FUNCTION {function}() RETURNS INT DETERMINISTIC MODIFIES SQL DATA BEGIN INSERT INTO {table} (id, note) VALUES (99, 'x'); RETURN 1; END"
        ),
    )
    .await;
}
