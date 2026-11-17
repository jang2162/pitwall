//! 실 DB 로 에이전트의 `db_schema`(`db.tool.schema`) 왕복을 본다(16-m5-plan.md M5-06 의 live 완료 기준). 사람이 그 프로젝트
//! 창에서 연결을 연 뒤 MCP 세션 범위의 연결이 테이블 목록과 열 · 기본 키를 읽는다. `PW_TEST_PG_URL` · `PW_TEST_MYSQL_URL` 이 있을
//! 때만 목록에 생긴다(`pw_db_driver::live`, `mise run test-db -- pg mysql mariadb` — MariaDB 는 MySQL 시험을 그 URL 로 돈다).
//! SQLite 는 서버가 없어 `tests/agent.rs` 의 보통 시험이 실 드라이버로 본다.
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

/// 시험 URL 의 서버를 연결 `dev` 로 둔 Host 와 그 프로젝트 창, 그리고 URL 의 DB 이름.
async fn host(live: LiveDb, driver: Arc<dyn pw_db_driver::DbDriver>) -> (Fixture, Conn, String) {
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
    (f, w, database)
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

/// 사람이 연결을 열어 시험 테이블을 만들고, 에이전트가 `db_schema` 로 테이블 목록과 그 열 · 기본 키 · 인덱스를 읽은 뒤 사람이
/// 테이블을 지운다. 사람이 열기 전의 에이전트 호출은 `notOpenedByUser` 다.
async fn round_trip(live: LiveDb, driver: Arc<dyn pw_db_driver::DbDriver>, table: &str) {
    let (f, mut w, database) = host(live, driver).await;
    let mut agent = f.connect(ConnectionScope::Agent);
    let call = |extra: Value| {
        let mut params = json!({ "projectId": f.id, "connectionId": "dev" });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        params
    };
    let before = agent
        .call("db.tool.schema", call(json!({})))
        .await
        .unwrap_err();
    assert_eq!(
        before.data.as_ref().and_then(|d| d["reason"].as_str()),
        Some("notOpenedByUser")
    );

    let opened = w
        .call(
            "db.session.open",
            json!({ "projectId": f.id, "connectionId": "dev" }),
        )
        .await
        .unwrap();
    let session = opened["session"]["sessionId"].as_str().unwrap().to_owned();
    run_confirmed(&mut w, &session, &format!("DROP TABLE IF EXISTS {table}")).await;
    run_confirmed(
        &mut w,
        &session,
        &format!("CREATE TABLE {table} (id INT PRIMARY KEY, note VARCHAR(20))"),
    )
    .await;

    let found = agent
        .call(
            "db.tool.schema",
            call(json!({ "filter": table, "database": database })),
        )
        .await
        .unwrap();
    assert_eq!(found["level"], "objects");
    let names: Vec<&str> = found["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, [table]);
    let described = agent
        .call(
            "db.tool.schema",
            call(json!({ "object": table, "database": database })),
        )
        .await
        .unwrap();
    let columns: Vec<(&str, bool)> = described["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap(),
                c["primaryKey"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(columns, [("id", true), ("note", false)]);
    assert!(
        described["indexes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["primary"] == true),
        "{described}"
    );
    run_confirmed(&mut w, &session, &format!("DROP TABLE {table}")).await;
}

#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_db_schema_reads_tables_and_columns_for_an_agent() {
    round_trip(
        LiveDb::Pg,
        Arc::new(pw_db_driver_pg::PgDriver::new()),
        "m5_06_agent_probe",
    )
    .await;
}

#[cfg(pw_live_mysql)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_mysql_db_schema_reads_tables_and_columns_for_an_agent() {
    round_trip(
        LiveDb::Mysql,
        Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
        "m5_06_agent_probe",
    )
    .await;
}

/// 실 PG 로 `db_connection_test`(`db.tool.connectionTest`, 16-m5-plan.md M5-17 의 live 완료 기준) 왕복: 사람이 연 적 없는 연결은
/// 첫 호출이 서버에 붙지 않고 「이 연결 열기」 계획을 내고, 승인 표를 실은 둘째 호출이 실 서버에 붙어 판본을 돌려준다. 그 뒤 같은
/// 세션의 `db_schema` 가 그 연결을 쓰고, 다른 세션은 쓰지 못한다.
#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_db_connection_test_opens_an_approved_connection() {
    use pw_core::registry::plan::{APPROVAL_KEY, APPROVAL_REQUIRED};
    let (f, _w, _database) = host(LiveDb::Pg, Arc::new(pw_db_driver_pg::PgDriver::new())).await;
    let mut agent = f.connect(ConnectionScope::Agent);
    let params = json!({ "projectId": f.id, "connectionId": "dev" });
    let asked = agent
        .call("db.tool.connectionTest", params.clone())
        .await
        .unwrap_err();
    assert_eq!(asked.code, APPROVAL_REQUIRED, "{asked:?}");
    let plan = &asked.data.unwrap()["plan"];
    assert!(
        plan["summary"]
            .as_str()
            .unwrap()
            .starts_with("Open this DB connection for this agent session"),
        "{plan}"
    );
    let mut approved = params.clone();
    approved[APPROVAL_KEY] = json!({ "hash": plan["hash"] });
    let tested = agent
        .call("db.tool.connectionTest", approved)
        .await
        .unwrap();
    assert!(
        tested["version"].as_str().is_some_and(|v| !v.is_empty()),
        "{tested}"
    );
    assert_eq!(tested["tls"], "plaintext", "{tested}");
    assert_eq!(tested["approvedForSession"], true);
    let schemas = agent
        .call(
            "db.tool.schema",
            json!({ "projectId": f.id, "connectionId": "dev", "kind": "schema" }),
        )
        .await
        .unwrap();
    assert_eq!(schemas["level"], "schemas", "{schemas}");
    let mut other = f.connect(ConnectionScope::Agent);
    let refused = other
        .call(
            "db.tool.schema",
            json!({ "projectId": f.id, "connectionId": "dev" }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.data.as_ref().and_then(|d| d["reason"].as_str()),
        Some("notOpenedByUser")
    );
}

/// 실 PG 로 표지 기억의 영속(사용자 결정 2026-11-18 — M5-17 재리뷰 G2, M5-17b): 사람의 키체인에 비밀번호가 있는 연결이 한 번
/// 프로덕션으로 읽힌 뒤 에이전트가 자기 파일 툴로 `production` 을 지우고 앱이 다시 켜져도, `db_connection_test` 는 「이 연결 열기」
/// 승인을 청하지 않고 프로덕션으로 거절한다. 사람이 그 프로젝트 창에서 기억을 풀어야 평범한 대상이 되고, 그때는 승인 뒤 실 서버에
/// 붙는다.
#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_a_remembered_production_mark_holds_across_a_restart_until_the_person_forgets_it() {
    use pw_core::registry::plan::{APPROVAL_KEY, APPROVAL_REQUIRED};
    let (mut f, _w, _database) = host(LiveDb::Pg, Arc::new(pw_db_driver_pg::PgDriver::new())).await;
    let dev = f.db_file("dev.toml");
    let unmarked = std::fs::read_to_string(&dev).unwrap();
    std::fs::write(&dev, format!("{unmarked}production = true\n")).unwrap();
    let params = json!({ "projectId": f.id, "connectionId": "dev" });
    let reason = |e: &RpcError| {
        e.data
            .as_ref()
            .and_then(|d| d["reason"].as_str())
            .map(str::to_owned)
    };
    let mut agent = f.connect(ConnectionScope::Agent);
    let blocked = agent
        .call("db.tool.connectionTest", params.clone())
        .await
        .unwrap_err();
    assert_eq!(
        reason(&blocked).as_deref(),
        Some("productionBlocked"),
        "{blocked:?}"
    );

    // 에이전트가 자기 파일 툴로 표지를 지우고, 앱이 다시 켜진다.
    std::fs::write(&dev, &unmarked).unwrap();
    f.restart(|router, connections| {
        let mut drivers = pw_db::DriverRegistry::new();
        drivers
            .register(Arc::new(pw_db_driver_pg::PgDriver::new()))
            .unwrap();
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    let mut agent = f.connect(ConnectionScope::Agent);
    let blocked = agent
        .call("db.tool.connectionTest", params.clone())
        .await
        .unwrap_err();
    assert_eq!(
        reason(&blocked).as_deref(),
        Some("productionBlocked"),
        "{blocked:?}"
    );

    // 사람이 그 프로젝트 창에서 푼다 — 그 뒤에는 평범한 대상이라 승인 뒤 실 서버에 붙는다.
    let mut w = f.window();
    let forgot = w
        .call("db.connection.forgetMarks", params.clone())
        .await
        .unwrap();
    assert_eq!(forgot["remembered"]["production"], true, "{forgot}");
    let asked = agent
        .call("db.tool.connectionTest", params.clone())
        .await
        .unwrap_err();
    assert_eq!(asked.code, APPROVAL_REQUIRED, "{asked:?}");
    let mut approved = params.clone();
    approved[APPROVAL_KEY] = json!({ "hash": asked.data.unwrap()["plan"]["hash"] });
    let tested = agent
        .call("db.tool.connectionTest", approved)
        .await
        .unwrap();
    assert!(
        tested["version"].as_str().is_some_and(|v| !v.is_empty()),
        "{tested}"
    );
}
