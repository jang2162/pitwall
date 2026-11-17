//! 실 DB 로 에이전트의 `db_execute`(`db.tool.execute`)를 본다(16-m5-plan.md M5-18 의 live 완료 기준): 승인 뒤 `INSERT` 왕복과,
//! 둘째 문장이 실패하면 첫 문장의 쓰기도 남지 않는 롤백. PG 는 트랜잭션 안에서 못 도는 문장(`CREATE INDEX CONCURRENTLY`)의 DB 오류가
//! 그대로 나오는 것, MySQL · MariaDB 는 암묵 커밋(DDL)이 「되돌릴 수 없다」 인 것 — DDL 앞의 쓰기는 롤백으로도 남는다. 승인은 세션
//! (`pw_mcp`)의 일이라 시험이 그 규약(첫 호출의 계획 해시를 승인 표로 싣는 둘째 호출)을 직접 한다. `PW_TEST_PG_URL` ·
//! `PW_TEST_MYSQL_URL` 이 있을 때만 목록에 생긴다(`mise run test-db -- pg mysql mariadb` — MariaDB 는 MySQL 시험을 그 URL 로 돈다).
//! SQLite 는 서버가 없어 `tests/execute_tool.rs` 의 보통 시험이 실 드라이버로 본다.
#![cfg(any(pw_live_pg, pw_live_mysql))]

mod common;

use std::sync::Arc;

use common::{Conn, Fixture};
use pw_core::registry::plan::{APPROVAL_KEY, APPROVAL_REQUIRED};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::live::{LiveDb, endpoint};
use pw_rpc::protocol::StreamOutcome;
use pw_rpc::{ConnectionScope, RpcError};
use serde_json::{Value, json};

/// 시험 URL 의 서버를 연결 `dev` 로 둔 Host 와 그 프로젝트 창(`tests/live_query.rs` 와 같다).
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

/// 사람이 콘솔에서 실행한다. 확인을 물으면 받은 해시로 한 번 확인한다.
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

/// 사람이 세션을 열고(「사람이 연 연결」) 시험 테이블에 행 셋을 만든다. 세션 id.
async fn prepare(w: &mut Conn, f: &Fixture, table: &str) -> String {
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
    session
}

/// 묻고(첫 호출 — 계획) 승인해(둘째 호출 — 그 해시) 돌린다.
async fn execute(agent: &mut Conn, f: &Fixture, sql: &str) -> (Value, Value) {
    let params = json!({ "projectId": f.id, "connectionId": "dev", "sql": sql });
    let asked = agent
        .call("db.tool.execute", params.clone())
        .await
        .expect_err("asks first");
    assert_eq!(asked.code, APPROVAL_REQUIRED, "{asked:?}");
    let plan = asked.data.unwrap()["plan"].clone();
    let mut approved = params;
    approved[APPROVAL_KEY] = json!({ "hash": plan["hash"] });
    let out = agent.call("db.tool.execute", approved).await.unwrap();
    (plan, out)
}

async fn ids(agent: &mut Conn, f: &Fixture, table: &str) -> Vec<Value> {
    let out = agent
        .call(
            "db.tool.query",
            json!({ "projectId": f.id, "connectionId": "dev", "sql": format!("SELECT id FROM {table} ORDER BY id") }),
        )
        .await
        .unwrap();
    out["results"][0]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[0].clone())
        .collect()
}

/// `INSERT` 왕복(커밋되어 다음 조회가 본다)과 롤백(둘째 문장의 키 중복 — 첫 문장의 `INSERT` 도 남지 않는다).
async fn insert_and_rollback(live: LiveDb, driver: Arc<dyn pw_db_driver::DbDriver>, table: &str) {
    let (f, mut w) = host(live, driver).await;
    let session = prepare(&mut w, &f, table).await;
    let mut agent = f.connect(ConnectionScope::Agent);
    let (plan, out) = execute(
        &mut agent,
        &f,
        &format!("INSERT INTO {table} (id, note) VALUES (4, 'four');\nUPDATE {table} SET note = 'uno' WHERE id = 1"),
    )
    .await;
    assert_eq!(plan["changes"][0]["kind"], "sql");
    assert_eq!(out["transaction"], "committed", "{out}");
    assert_eq!(out["results"][0]["rowsAffected"], 1, "{out}");
    assert_eq!(out["results"][1]["rowsAffected"], 1, "{out}");
    assert_eq!(
        ids(&mut agent, &f, table).await,
        [
            json!({"int": 1}),
            json!({"int": 2}),
            json!({"int": 3}),
            json!({"int": 4})
        ]
    );
    let (_, out) = execute(
        &mut agent,
        &f,
        &format!("INSERT INTO {table} (id, note) VALUES (5, 'five');\nINSERT INTO {table} (id, note) VALUES (1, 'dup')"),
    )
    .await;
    assert_eq!(out["transaction"], "rolledBack", "{out}");
    assert_eq!(out["error"]["index"], 1, "{out}");
    assert_eq!(out["results"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(
        ids(&mut agent, &f, table).await.len(),
        4,
        "the first INSERT was rolled back too"
    );
    run_confirmed(&mut w, &session, &format!("DROP TABLE {table}")).await;
}

#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_db_execute_inserts_and_rolls_back() {
    insert_and_rollback(
        LiveDb::Pg,
        Arc::new(pw_db_driver_pg::PgDriver::new()),
        "m5_18_execute_probe",
    )
    .await;
}

/// PG: 트랜잭션 안에서 못 도는 문장은 푸는 입력이 없고 DB 의 오류(SQLSTATE 25001)가 그대로 나온다. 앞 문장도 롤백된다.
#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_db_execute_returns_the_error_of_a_statement_that_needs_no_transaction() {
    let table = "m5_18_concurrently_probe";
    let (f, mut w) = host(LiveDb::Pg, Arc::new(pw_db_driver_pg::PgDriver::new())).await;
    let session = prepare(&mut w, &f, table).await;
    let mut agent = f.connect(ConnectionScope::Agent);
    let (_, out) = execute(
        &mut agent,
        &f,
        &format!("INSERT INTO {table} (id, note) VALUES (7, 'seven');\nCREATE INDEX CONCURRENTLY m5_18_idx ON {table} (note)"),
    )
    .await;
    assert_eq!(out["transaction"], "rolledBack", "{out}");
    assert_eq!(out["error"]["index"], 1, "{out}");
    assert_eq!(out["error"]["error"]["code"], "25001", "{out}");
    assert_eq!(ids(&mut agent, &f, table).await.len(), 3);
    run_confirmed(&mut w, &session, &format!("DROP TABLE {table}")).await;
}

#[cfg(pw_live_mysql)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_mysql_db_execute_inserts_and_rolls_back() {
    insert_and_rollback(
        LiveDb::Mysql,
        Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
        "m5_18_execute_probe",
    )
    .await;
}

/// MySQL · MariaDB: DDL 은 암묵 커밋이라 계획이 「되돌릴 수 없다」 를 보이고, 뒤 문장이 실패해 롤백해도 DDL 과 그 앞의 쓰기는 남는다.
/// DDL 뒤의 쓰기는 새 트랜잭션에 묶여 롤백된다.
#[cfg(pw_live_mysql)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_mysql_db_execute_shows_an_implicit_commit_as_irreversible() {
    let table = "m5_18_implicit_probe";
    let (f, mut w) = host(
        LiveDb::Mysql,
        Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
    )
    .await;
    let session = prepare(&mut w, &f, table).await;
    let mut agent = f.connect(ConnectionScope::Agent);
    let (plan, out) = execute(
        &mut agent,
        &f,
        &format!(
            "INSERT INTO {table} (id, note) VALUES (6, 'six');\nALTER TABLE {table} ADD COLUMN extra INT;\nINSERT INTO {table} (id, note) VALUES (8, 'eight');\nINSERT INTO {table} (id, note) VALUES (1, 'dup')"
        ),
    )
    .await;
    assert_eq!(plan["changes"][0]["irreversible"], true, "{plan}");
    assert_eq!(out["irreversible"], true, "{out}");
    assert_eq!(out["transaction"], "rolledBack", "{out}");
    assert_eq!(out["error"]["index"], 3, "{out}");
    let left = ids(&mut agent, &f, table).await;
    assert!(
        left.contains(&json!({"int": 6})),
        "committed by the DDL: {left:?}"
    );
    assert!(!left.contains(&json!({"int": 8})), "rolled back: {left:?}");
    run_confirmed(&mut w, &session, &format!("DROP TABLE {table}")).await;
}
