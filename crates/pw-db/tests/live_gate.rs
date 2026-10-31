//! 관문 너머의 실 DB 로 분류기의 「조회인데 쓰기」 틈(M3-17 리뷰 X1 · X3)을 본다. `PW_TEST_PG_URL` · `PW_TEST_MYSQL_URL` 이 있을
//! 때만 목록에 생긴다(`pw_db_driver::live`, `mise run test-db -- pg mysql mariadb`).
#![cfg(any(pw_live_pg, pw_live_mysql))]

mod common;

use std::sync::Arc;

use common::{Conn, Fixture};
use pw_db::DbSessions;
use pw_db_driver::live::{LiveDb, endpoint};
use pw_rpc::RpcError;
use pw_rpc::protocol::StreamOutcome;
use serde_json::{Value, json};

/// 시험 URL 의 서버를 `dev`(보통) · `ro`(read-only) 두 연결로 둔 Host. 비밀은 가짜 키체인에.
async fn host(live: LiveDb, driver: Arc<dyn pw_db_driver::DbDriver>) -> (Fixture, Conn) {
    let url = live.url();
    let end = endpoint(live, &url).unwrap();
    let rest = url.split_once("://").unwrap().1;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let user = authority
        .rsplit_once('@')
        .map(|(info, _)| info.split(':').next().unwrap().to_owned())
        .unwrap();
    let database = path.split(['?', '#']).next().unwrap_or_default();
    let id = driver.id().to_owned();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(driver).unwrap();
    let f = Fixture::build(true, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    for (name, extra) in [("dev", ""), ("ro", "read_only = true\n")] {
        f.write_db(
            &format!("{name}.toml"),
            &format!(
                "schema = 1\nname = \"{name}\"\ndriver = \"{id}\"\nhost = \"{}\"\nport = {}\nuser = \"{user}\"\ndatabase = \"{database}\"\nsslmode = \"disable\"\npassword = {{ secret_ref = \"keychain\" }}\n{extra}",
                end.host, end.port
            ),
        );
    }
    let mut w = f.window();
    if let Some(password) = &end.password {
        for name in ["dev", "ro"] {
            w.set_password(&f.id, name, password).await.unwrap();
        }
    }
    (f, w)
}

async fn open(f: &Fixture, w: &mut Conn, connection: &str) -> String {
    let opened = w
        .call(
            "db.session.open",
            json!({ "projectId": f.id, "connectionId": connection }),
        )
        .await
        .unwrap();
    opened["session"]["sessionId"].as_str().unwrap().to_owned()
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

/// 첫 행의 첫 칸.
fn first(items: &[Value]) -> &Value {
    items
        .iter()
        .find(|i| i["kind"] == "rows")
        .map(|i| &i["rows"][0][0])
        .unwrap_or_else(|| panic!("no row: {items:?}"))
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

#[cfg(pw_live_pg)]
mod pg {
    use pw_db::gate::code;
    use pw_rpc::code as rpc_code;

    use super::*;

    /// read-only 연결에서 `query_to_xml('select pg_terminate_backend(<다른 세션>)')` 류를 돌리면 관문이 거절하고 그 세션은 살아 있다
    /// (X1 — PG 의 읽기 전용 트랜잭션은 안의 관리 함수를 막지 않으므로 분류가 첫 방어다). 보통 연결은 범위 없는 쓰기의 확인을 묻고
    /// (아무것도 돌지 않는다), 조회만 받는 호출은 `notAQuery` 다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_pg_sql_text_functions_do_not_end_another_session_from_a_read_only_connection() {
        let (f, mut w) = host(LiveDb::Pg, Arc::new(pw_db_driver_pg::PgDriver::new())).await;
        let victim = open(&f, &mut w, "dev").await;
        let pid_sql = json!({ "sessionId": victim, "sql": "SELECT pg_backend_pid()" });
        let victim_pid = first(&run(&mut w, pid_sql.clone()).await.unwrap())["int"]
            .as_i64()
            .unwrap();
        let ro = open(&f, &mut w, "ro").await;
        let dev = open(&f, &mut w, "dev").await;
        let kill = format!("select pg_terminate_backend({victim_pid})");
        for sql in [
            format!("SELECT query_to_xml('{kill}', true, false, '')"),
            format!("SELECT query_to_xml_and_xmlschema('{kill}', true, false, '')"),
            format!("SELECT count(*) FROM ts_stat('select to_tsvector(''a'') from ({kill}) s')"),
            format!(
                "SELECT ts_rewrite('a'::tsquery, 'select ''a''::tsquery, ''b''::tsquery from ({kill}) s')"
            ),
        ] {
            let err = run(&mut w, json!({ "sessionId": ro, "sql": sql }))
                .await
                .unwrap_err();
            assert_eq!(err.code, rpc_code::PERMISSION_DENIED, "{sql}: {err:?}");
            assert_eq!(reason(&err), Some("readOnlyConnection"), "{sql}: {err:?}");
            for session in [&ro, &dev] {
                let err = run(
                    &mut w,
                    json!({ "sessionId": session, "sql": sql, "queryOnly": true }),
                )
                .await
                .unwrap_err();
                assert_eq!(reason(&err), Some("notAQuery"), "{sql}: {err:?}");
            }
            let err = w
                .call(
                    "db.query.page",
                    json!({ "projectId": f.id, "connectionId": "ro", "sql": sql, "limit": 5 }),
                )
                .await
                .unwrap_err();
            assert_eq!(reason(&err), Some("notAQuery"), "{sql}: {err:?}");
            let err = run(&mut w, json!({ "sessionId": dev, "sql": sql }))
                .await
                .unwrap_err();
            assert_eq!(err.code, code::CONFIRM_REQUIRED, "{sql}: {err:?}");
            assert_eq!(err.data.as_ref().unwrap()["grade"], "unboundedWrite");
        }
        // 그 세션은 살아 있다 — 같은 백엔드가 다음 문장을 돌린다.
        let again = run(&mut w, pid_sql)
            .await
            .expect("the other session was ended");
        assert_eq!(first(&again)["int"].as_i64(), Some(victim_pid));
    }
}

#[cfg(pw_live_mysql)]
mod mysql {
    use super::*;

    /// X3: `NO_BACKSLASH_ESCAPES` 세션에서 `'a\' OR 1 = 1 INTO @변수 -- '` 는 분류기의 두 읽기 가운데 뒤의 것을 파서가 읽지 못한다 —
    /// 조회만 받는 호출(`queryOnly` · `db.query.page`)은 거절하고 세션 변수는 그대로다. 같은 글을 일반 실행하면 조회 등급으로 돌아
    /// 변수가 바뀐다 — 그 글이 실제로 서버에서 쓰기라는 대조이자 14 M3-03 칸의 남는 틈. `sql_mode` 는 이 시험의 세션에만 건다(서버
    /// 설정을 바꾸지 않는다 — CONTRIBUTING.md 「실 DB 시험」).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_mysql_query_only_refuses_a_reading_the_classifier_could_not_read() {
        let (f, mut w) = host(
            LiveDb::Mysql,
            Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
        )
        .await;
        let s = open(&f, &mut w, "dev").await;
        let exec = |sql: &str| json!({ "sessionId": s, "sql": sql });
        run(
            &mut w,
            exec("SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',NO_BACKSLASH_ESCAPES')"),
        )
        .await
        .unwrap();
        run(&mut w, exec("SET @pw_x = 'before'")).await.unwrap();
        let hidden = r"SELECT name FROM (SELECT 'a' AS name) AS v WHERE name = 'a\' OR 1 = 1 INTO @pw_x -- '";
        let err = run(
            &mut w,
            json!({ "sessionId": s, "sql": hidden, "queryOnly": true }),
        )
        .await
        .unwrap_err();
        assert_eq!(reason(&err), Some("notAQuery"), "{err:?}");
        assert_eq!(
            err.data.as_ref().unwrap()["statements"][0]["reasons"],
            json!(["unparsed"])
        );
        let err = w
            .call(
                "db.query.page",
                json!({ "projectId": f.id, "connectionId": "dev", "sql": hidden, "limit": 5 }),
            )
            .await
            .unwrap_err();
        assert_eq!(reason(&err), Some("notAQuery"), "{err:?}");
        let read_x = exec("SELECT @pw_x");
        assert_eq!(
            first(&run(&mut w, read_x.clone()).await.unwrap())["text"],
            "before"
        );
        run(&mut w, exec(hidden)).await.unwrap();
        assert_eq!(first(&run(&mut w, read_x).await.unwrap())["text"], "a");
    }
}
