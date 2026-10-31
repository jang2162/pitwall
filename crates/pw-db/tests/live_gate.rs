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
            // M3-03b 리뷰 H1 · H2: 같은 함수를 다른 꼴로 — attribute notation 과 `U&"…"` 이름.
            format!("SELECT ({victim_pid}).pg_terminate_backend"),
            format!("SELECT 1 WHERE ({victim_pid}).pg_terminate_backend"),
            format!(
                "SELECT ('select to_tsvector(''a'') from (select pg_terminate_backend({victim_pid})) s').ts_stat"
            ),
            format!(r#"SELECT U&"query\005fto_xml"('{kill}', true, false, '')"#),
            format!(r#"SELECT (({victim_pid})).U&"pg\005fterminate\005fbackend""#),
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

    /// M3-03b 리뷰 M2: 보통 연결(운영 표시 없음)에서 `SET standard_conforming_strings = off; DELETE … WHERE name = 'a\'' OR true
    /// --'` 는 리뷰 때 확인 없이 돌아 세 행을 모두 지웠다. 이제 글 전체가 거절되고(`changesSqlReading`) 행 · 세션 설정이 그대로다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_pg_a_session_cannot_turn_standard_strings_off() {
        let (f, mut w) = host(LiveDb::Pg, Arc::new(pw_db_driver_pg::PgDriver::new())).await;
        let s = open(&f, &mut w, "dev").await;
        let table = format!("pw_live_scs_gate_{}", std::process::id());
        let exec = |sql: &str| json!({ "sessionId": s, "sql": sql });
        run_confirmed(&mut w, exec(&format!("DROP TABLE IF EXISTS {table}"))).await;
        run_confirmed(&mut w, exec(&format!("CREATE TABLE {table} (name text)"))).await;
        run(
            &mut w,
            exec(&format!("INSERT INTO {table} VALUES ('a'), ('b'), ('c')")),
        )
        .await
        .unwrap();
        for sql in [
            format!(
                r"SET standard_conforming_strings = off; DELETE FROM {table} WHERE name = 'a\'' OR true --'"
            ),
            "SELECT set_config('standard_conforming_strings', 'off', false)".to_owned(),
            "SET LOCAL standard_conforming_strings TO off".to_owned(),
            "UPDATE pg_settings SET setting = 'off' WHERE name = 'standard_conforming_strings'"
                .to_owned(),
        ] {
            let err = run(&mut w, exec(&sql)).await.unwrap_err();
            assert_eq!(err.code, rpc_code::PERMISSION_DENIED, "{sql}: {err:?}");
            assert_eq!(reason(&err), Some("changesSqlReading"), "{sql}: {err:?}");
        }
        let count = run(&mut w, exec(&format!("SELECT count(*) FROM {table}")))
            .await
            .unwrap();
        assert_eq!(first(&count)["int"], 3);
        let setting = run(&mut w, exec("SHOW standard_conforming_strings"))
            .await
            .unwrap();
        assert_eq!(first(&setting)["text"], "on");
        run_confirmed(&mut w, exec(&format!("DROP TABLE {table}"))).await;
    }

    /// 확인을 받아야 하면 받은 해시 · 등급으로 다시 보낸다(시험 준비 · 정리용 DDL).
    async fn run_confirmed(w: &mut Conn, params: Value) {
        match run(w, params.clone()).await {
            Ok(_) => {}
            Err(err) if err.code == code::CONFIRM_REQUIRED => {
                let data = err.data.unwrap();
                let mut again = params;
                again["confirmed"] =
                    json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });
                run(w, again).await.unwrap();
            }
            Err(err) => panic!("{err:?}"),
        }
    }
}

#[cfg(pw_live_mysql)]
mod mysql {
    use super::*;

    /// X3 · M3-03b 리뷰 M1 · M2: `'a\' OR 1 = 1 INTO @변수 -- '` 는 분류기의 두 읽기 가운데 뒤의 것(`NO_BACKSLASH_ESCAPES`)을 파서가
    /// 읽지 못한다 — 조회만 받는 호출(`queryOnly` · `db.query.page`)과 read-only 연결의 일반 실행이 거절하고 세션 변수는 그대로다.
    /// 세션에서 `NO_BACKSLASH_ESCAPES` 를 켜는 `SET` 은 거절된다(`changesSqlReading`). 서버 설정은 바꾸지 않는다(CONTRIBUTING.md
    /// 「실 DB 시험」) — 서버 전역에 켠 재현은 그 실행만의 컨테이너에서 했다(M3-03b 보고서).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_mysql_query_only_refuses_a_reading_the_classifier_could_not_read() {
        let (f, mut w) = host(
            LiveDb::Mysql,
            Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
        )
        .await;
        let s = open(&f, &mut w, "dev").await;
        let ro = open(&f, &mut w, "ro").await;
        let exec = |session: &str, sql: &str| json!({ "sessionId": session, "sql": sql });
        for sql in [
            "SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',NO_BACKSLASH_ESCAPES')",
            "SET sql_mode = 'ANSI_QUOTES'",
            "SET NAMES gbk",
            // M3-03b 재리뷰 N1: 수 값 — MySQL 은 `sql_mode` 를 비트 수(1048576 = NO_BACKSLASH_ESCAPES, 4 = ANSI_QUOTES)로,
            // 문자 집합을 번호(28 = gbk)로 받는다.
            "SET sql_mode = 1048576",
            "SET SESSION sql_mode = 4",
            "SET character_set_client = 28",
        ] {
            let err = run(&mut w, exec(&s, sql)).await.unwrap_err();
            assert_eq!(reason(&err), Some("changesSqlReading"), "{sql}: {err:?}");
        }
        let settings = run(
            &mut w,
            exec(
                &s,
                "SELECT CONCAT(@@SESSION.sql_mode, '|', @@SESSION.character_set_client)",
            ),
        )
        .await
        .unwrap();
        let settings = first(&settings)["text"].as_str().unwrap().to_owned();
        assert!(!settings.contains("NO_BACKSLASH_ESCAPES"), "{settings}");
        assert!(!settings.contains("ANSI_QUOTES"), "{settings}");
        assert!(settings.ends_with("|utf8mb4"), "{settings}");
        run(&mut w, exec(&s, "SET @pw_x = 'before'")).await.unwrap();
        let hidden = r"SELECT name FROM (SELECT 'a' AS name) AS v WHERE name = 'a\' OR 1 = 1 INTO @pw_x -- '";
        // 재리뷰의 재현(보통 연결의 한 번 실행 — 그때는 확인 없이 돌아 서버에 파일을 썼다)을 변수 꼴로: 글 전체가 돌지 않는다.
        let err = run(
            &mut w,
            exec(&s, &format!("SET sql_mode = 1048576; {hidden}")),
        )
        .await
        .unwrap_err();
        assert_eq!(reason(&err), Some("changesSqlReading"), "{err:?}");
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
        let err = run(&mut w, exec(&ro, hidden)).await.unwrap_err();
        assert_eq!(reason(&err), Some("readOnlyConnection"), "{err:?}");
        let read_x = exec(&s, "SELECT @pw_x");
        assert_eq!(first(&run(&mut w, read_x).await.unwrap())["text"], "before");
    }
}
