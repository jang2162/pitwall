//! 그리드 보유 상한(결정 M3-D7 의 행 상한 + M3-16 리뷰의 바이트 상한 `[db] grid_max_bytes`)을 실 PG 로 관문 너머에서 본다.
//! `PW_TEST_PG_URL` 이 있을 때만 목록에 생긴다(`pw_db_driver::live`, `mise run test-db -- pg`).
#![cfg(pw_live_pg)]

mod common;

use std::sync::Arc;

use common::Fixture;
use pw_db::DbSessions;
use pw_db_driver::live::{LiveDb, endpoint};
use pw_rpc::protocol::StreamOutcome;
use serde_json::{Value, json};

/// 시험 URL(`postgres://user:password@host:port/db`)의 연결을 `dev` 로 둔 Host. 비밀은 가짜 키체인에.
async fn host() -> (Fixture, Arc<DbSessions>, common::Conn, String) {
    let url = LiveDb::Pg.url();
    let end = endpoint(LiveDb::Pg, &url).unwrap();
    let rest = url.split_once("://").unwrap().1;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let user = authority
        .rsplit_once('@')
        .map(|(info, _)| info.split(':').next().unwrap().to_owned())
        .unwrap();
    let database = path.split('?').next().unwrap_or_default();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers
        .register(Arc::new(pw_db_driver_pg::PgDriver::new()))
        .unwrap();
    let mut slot = None;
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    let sessions = slot.unwrap();
    f.write_db(
        "dev.toml",
        &format!(
            "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"{}\"\nport = {}\nuser = \"{user}\"\ndatabase = \"{database}\"\nsslmode = \"disable\"\npassword = {{ secret_ref = \"keychain\" }}\n",
            end.host, end.port
        ),
    );
    let mut w = f.window();
    if let Some(password) = &end.password {
        w.set_password(&f.id, "dev", password).await.unwrap();
    }
    let opened = w
        .call(
            "db.session.open",
            json!({ "projectId": f.id, "connectionId": "dev" }),
        )
        .await
        .unwrap();
    let s = opened["session"]["sessionId"].as_str().unwrap().to_owned();
    (f, sessions, w, s)
}

async fn run(w: &mut common::Conn, session: &str, sql: &str) -> Vec<Value> {
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

fn rows(items: &[Value]) -> Vec<&Value> {
    items
        .iter()
        .filter(|i| i["kind"] == "rows")
        .flat_map(|i| i["rows"].as_array().unwrap().iter())
        .collect()
}

/// 1 MB 셀 200행: 바이트 상한(4 MiB)에서 멈추고(5행 — 4 MiB 를 처음 넘긴 행까지), 세션은 다음 실행을 그대로 받는다. 같은 결과에
/// 행 상한 3 이 먼저면 3행에서 멈춘다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_a_result_of_big_cells_stops_at_the_byte_limit() {
    let (_f, sessions, mut w, s) = host().await;
    let big = "SELECT repeat('x', 1000000) AS body FROM generate_series(1, 200)";

    sessions.set_grid_max_bytes(Some(4 * 1024 * 1024));
    let items = run(&mut w, &s, big).await;
    let done = items.last().unwrap();
    assert_eq!(done["kind"], "done", "{done}");
    assert_eq!(done["truncated"], true);
    let got = rows(&items);
    assert_eq!(got.len(), 5);
    assert!(
        got.iter()
            .all(|r| r[0]["text"].as_str().map(str::len) == Some(1_000_000))
    );

    // 행이 먼저.
    sessions.set_grid_max_rows(Some(3));
    let items = run(&mut w, &s, big).await;
    assert_eq!(items.last().unwrap()["truncated"], true);
    assert_eq!(rows(&items).len(), 3);

    // 커서만 버렸다 — 같은 세션이 다음 문장을 돌린다.
    sessions.set_grid_max_rows(None);
    sessions.set_grid_max_bytes(None);
    let items = run(&mut w, &s, "SELECT 1 AS one").await;
    assert_eq!(items.last().unwrap()["truncated"], false);
    assert_eq!(rows(&items)[0][0]["int"], 1);
}
