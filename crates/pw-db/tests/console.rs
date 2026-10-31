//! M3-15(SQL 콘솔)이 `pw-db` 에 더한 것: 실행 계획 `db.query.explain` 과 세션을 열 때의 스키마 고르기(`db.session.open { schema }`).
//! 가짜 드라이버로 실제 RPC 세션 너머에서 본다 — 계획은 실행과 같은 관문(세션 범위 · 신뢰 · read-only · 등급 확인)을 지나고 `ANALYZE`
//! 를 받지 않으며, 고른 스키마는 드라이버의 연결 설정까지 가고 스키마가 없는 드라이버면 연결을 열지 않는다.

mod common;

use std::sync::Arc;

use common::{Conn, Fixture};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::fake::{FakeCall, FakeDriver, FakeResponse};
use pw_db_types::{Capabilities, SqlDialect};
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

struct Env {
    f: Fixture,
    _sessions: Arc<DbSessions>,
    pg: FakeDriver,
    my: FakeDriver,
}

fn env() -> Env {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    // MySQL 은 DB 가 곧 스키마다 — 스키마 단계도 계획도 없는 드라이버로 흉내 낸다.
    let my = FakeDriver::named_with(
        "mysql",
        SqlDialect::Mysql,
        Capabilities {
            schemas: false,
            explain: false,
            ..FakeDriver::full_capabilities()
        },
    );
    let mut drivers = pw_db::DriverRegistry::new();
    for d in [&pg, &my] {
        drivers.register(Arc::new(d.clone())).unwrap();
    }
    let mut slot = None;
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    for (file, extra) in [
        ("dev", ""),
        ("prod", "production = true\n"),
        ("ro", "read_only = true\n"),
    ] {
        f.write_db(
            &format!("{file}.toml"),
            &format!(
                "schema = 1\nname = \"{file} db\"\ndriver = \"postgres\"\nhost = \"localhost\"\n{extra}"
            ),
        );
    }
    f.write_db(
        "my.toml",
        "schema = 1\nname = \"my\"\ndriver = \"mysql\"\nhost = \"localhost\"\n",
    );
    Env {
        f,
        _sessions: slot.unwrap(),
        pg,
        my,
    }
}

impl Env {
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

fn explain(session: &str, sql: &str) -> Value {
    json!({ "sessionId": session, "sql": sql })
}

fn explained(driver: &FakeDriver) -> Vec<String> {
    driver
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            FakeCall::Explain(sql) => Some(sql),
            _ => None,
        })
        .collect()
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

/// 고른 문장 하나의 원문만 드라이버의 계획으로 간다 — 편집기 전체 글은 가지 않는다. 수동 커밋 탭이어도 트랜잭션을 열지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explain_plans_only_the_chosen_statement_without_opening_a_transaction() {
    let e = env();
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    w.call(
        "db.session.setMode",
        json!({ "sessionId": s, "mode": "manual" }),
    )
    .await
    .unwrap();
    let plan = w
        .call(
            "db.query.explain",
            json!({ "sessionId": s, "sql": "SELECT 1;\nSELECT * FROM t WHERE a = 1;\nDELETE FROM t", "statements": [1] }),
        )
        .await
        .unwrap();
    assert_eq!(plan["format"], "json");
    assert!(plan["text"].as_str().unwrap().contains("Seq Scan"));
    assert_eq!(explained(&e.pg), vec!["SELECT * FROM t WHERE a = 1"]);
    let calls = e.pg.calls();
    assert!(
        !calls
            .iter()
            .any(|c| matches!(c, FakeCall::Begin { .. } | FakeCall::Execute(_))),
        "{calls:?}"
    );
    let info = w
        .call("db.session.rollback", json!({ "sessionId": s }))
        .await
        .unwrap();
    assert_eq!(info["state"]["kind"], "idle");
}

/// `ANALYZE` 는 없다: 계획 · 통계 문장(`EXPLAIN ANALYZE` · `EXPLAIN` · `ANALYZE` · `DESCRIBE`)과 트랜잭션 제어 문장, 둘 이상의
/// 문장은 드라이버에 닿기 전에 invalid params 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explain_refuses_analyze_and_anything_but_one_plain_statement() {
    let e = env();
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    for sql in [
        "EXPLAIN ANALYZE DELETE FROM t",
        "explain (analyze, buffers) delete from t",
        "/* x */ EXPLAIN SELECT 1",
        "ANALYZE t",
        "DESCRIBE t",
        "BEGIN",
        "COMMIT",
        "SELECT 1; SELECT 2",
    ] {
        let err = w
            .call("db.query.explain", explain(&s, sql))
            .await
            .unwrap_err();
        assert_eq!(err.code, rpc_code::INVALID_PARAMS, "{sql}: {err:?}");
    }
    let err = w
        .call("db.query.explain", explain(&s, "BEGIN"))
        .await
        .unwrap_err();
    assert_eq!(reason(&err), Some("transactionStatement"));
    assert!(
        e.pg.calls()
            .iter()
            .all(|c| !matches!(c, FakeCall::Explain(_)))
    );
}

/// 등급 정책은 실행과 같다: 범위 없는 쓰기는 3001(드라이버에 닿지 않음), 받은 해시를 실은 확인 · `db.sql.classify` 가 미리 준
/// 해시면 계획을 준다. 프로덕션의 쓰기도 확인, 파괴는 이름까지.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explain_asks_for_the_same_confirmation_as_running() {
    let e = env();
    let mut w = e.f.window();
    let dev = e.open(&mut w, "dev").await;
    let asked = w
        .call("db.query.explain", explain(&dev, "DELETE FROM t"))
        .await
        .unwrap_err();
    assert_eq!(asked.code, code::CONFIRM_REQUIRED);
    assert!(explained(&e.pg).is_empty());
    let data = asked.data.unwrap();
    let confirmed = json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });
    w.call(
        "db.query.explain",
        json!({ "sessionId": dev, "sql": "DELETE FROM t", "confirmed": confirmed }),
    )
    .await
    .unwrap();
    assert_eq!(explained(&e.pg), vec!["DELETE FROM t"]);
    // 창이 미리 받은 해시(분류 + 세션)도 같은 실행 하나에 맞는다.
    let classified = w
        .call(
            "db.sql.classify",
            json!({ "projectId": e.f.id, "sessionId": dev, "sql": "UPDATE t SET a = 1" }),
        )
        .await
        .unwrap();
    w.call(
        "db.query.explain",
        json!({ "sessionId": dev, "sql": "UPDATE t SET a = 1",
                "confirmed": { "grade": classified["grade"], "statementsHash": classified["statementsHash"] } }),
    )
    .await
    .unwrap();

    let prod = e.open(&mut w, "prod").await;
    let asked = w
        .call(
            "db.query.explain",
            explain(&prod, "INSERT INTO t VALUES (1)"),
        )
        .await
        .unwrap_err();
    assert_eq!(asked.code, code::CONFIRM_REQUIRED);
    let asked = w
        .call("db.query.explain", explain(&prod, "DROP TABLE t"))
        .await
        .unwrap_err();
    let data = asked.data.unwrap();
    assert_eq!(data["nameRequired"], true);
    let wrong = w
        .call(
            "db.query.explain",
            json!({ "sessionId": prod, "sql": "DROP TABLE t",
                    "confirmed": { "grade": "destructive", "statementsHash": data["statementsHash"], "name": "prod" } }),
        )
        .await
        .unwrap_err();
    assert_eq!(wrong.code, code::CONFIRM_REQUIRED);
    assert_eq!(explained(&e.pg).len(), 2);
}

/// read-only 연결: 조회 밖은 거절(드라이버에 닿지 않음), 조회의 계획은 읽기 전용 트랜잭션 안에서 받고 그 트랜잭션을 닫는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explain_on_a_read_only_connection_stays_inside_a_read_only_transaction() {
    let e = env();
    let mut w = e.f.window();
    let s = e.open(&mut w, "ro").await;
    for sql in ["DELETE FROM t WHERE id = 1", "SELEC oops"] {
        let err = w
            .call("db.query.explain", explain(&s, sql))
            .await
            .unwrap_err();
        assert_eq!(err.code, rpc_code::PERMISSION_DENIED, "{sql}");
        assert_eq!(reason(&err), Some("readOnlyConnection"));
    }
    assert!(explained(&e.pg).is_empty());
    w.call("db.query.explain", explain(&s, "SELECT * FROM t"))
        .await
        .unwrap();
    let calls = e.pg.calls_of(0);
    let begin = calls
        .iter()
        .position(|c| *c == FakeCall::Begin { read_only: true });
    let plan = calls.iter().position(|c| matches!(c, FakeCall::Explain(_)));
    let rollback = calls.iter().rposition(|c| *c == FakeCall::Rollback);
    assert!(
        begin.is_some() && begin < plan && plan < rollback,
        "{calls:?}"
    );
}

/// 범위 · 신뢰: 다른 프로젝트 창은 남의 세션의 계획을 받지 못하고, 신뢰를 거두면 열린 세션의 계획도 거절한다. 계획이 없는 드라이버는
/// 3003 unsupported.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explain_keeps_to_the_project_window_and_trust() {
    let e = env();
    e.pg.respond("FROM t", FakeResponse::rows(1));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let mut other = e.f.connect(ConnectionScope::Project(e.f.other.clone()));
    let err = other
        .call("db.query.explain", explain(&s, "SELECT * FROM t"))
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc_code::PERMISSION_DENIED);

    let my = e.open(&mut w, "my").await;
    let err = w
        .call("db.query.explain", explain(&my, "SELECT 1"))
        .await
        .unwrap_err();
    assert_eq!(err.code, code::DB_ERROR, "{err:?}");
    assert_eq!(err.data.unwrap()["error"]["kind"], "unsupported");
    assert!(
        e.my.calls()
            .iter()
            .all(|c| !matches!(c, FakeCall::Explain(_)))
    );

    e.f.projects.set_trust(&e.f.id, false).unwrap();
    let err = w
        .call("db.query.explain", explain(&s, "SELECT * FROM t"))
        .await
        .unwrap_err();
    assert_eq!(reason(&err), Some("untrusted"));
    assert!(explained(&e.pg).is_empty());
}

/// 고른 DB · 스키마는 드라이버의 연결 설정까지 간다. 스키마가 없는 드라이버에 스키마를 주면 연결을 열지 않고 invalid params, 제어
/// 문자가 든 이름도 invalid params.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chosen_schema_reaches_the_driver_or_is_refused() {
    let e = env();
    let mut w = e.f.window();
    w.call(
        "db.session.open",
        json!({ "projectId": e.f.id, "connectionId": "dev", "database": "billing", "schema": "ledger" }),
    )
    .await
    .unwrap();
    let spec = e.pg.connect_specs().pop().unwrap();
    assert_eq!(spec.database.as_deref(), Some("billing"));
    assert_eq!(spec.schema.as_deref(), Some("ledger"));
    e.open(&mut w, "dev").await;
    assert_eq!(e.pg.connect_specs().pop().unwrap().schema, None);

    let err = w
        .call(
            "db.session.open",
            json!({ "projectId": e.f.id, "connectionId": "my", "schema": "ledger" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc_code::INVALID_PARAMS, "{err:?}");
    assert!(e.my.calls().is_empty(), "{:?}", e.my.calls());
    for schema in ["", "a\nb"] {
        let err = w
            .call(
                "db.session.open",
                json!({ "projectId": e.f.id, "connectionId": "dev", "schema": schema }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, rpc_code::INVALID_PARAMS, "{schema:?}");
    }
}
