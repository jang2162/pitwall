//! M3-10 의 완료 기준(14-m3-plan.md M3-10 행): 가짜 드라이버로 스키마 탐색 RPC · 캐시를 실제 RPC 세션 너머에서 본다.
//! ① 1만 개 목록의 첫 쪽은 드라이버를 한 번만 부르고 같은 쪽 두 번째는 캐시 ② DDL 실행 뒤 `db.schemaChanged` 가 오고 다음 조회가
//! 드라이버로 간다(③ 키 판정 표는 `schema::tests`) ④ 다른 창 · 미신뢰 프로젝트의 `db.schema.*` 는 거절되고 드라이버가 불리지 않는다.
//! 그 밖에 메타데이터용 연결이 사람의 세션과 갈라져 커밋된 스키마만 보는 것, 이벤트가 그 프로젝트 범위로만 나가는 것, 창을 닫으면
//! 메타데이터용 연결이 닫히는 것, 설정이 바뀌면 캐시를 버리는 것.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Conn, Fixture};
use pw_core::bus::{Filter, Received, Scope, Subscription};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::fake::{FakeCall, FakeDriver, FakeResponse, FakeTable};
use pw_db_types::{
    ColumnInfo, DbError, DbErrorKind, IndexInfo, ObjectInfo, ObjectKind, SqlDialect,
};
use pw_rpc::protocol::StreamOutcome;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

struct Env {
    f: Fixture,
    sessions: Arc<DbSessions>,
    pg: FakeDriver,
}

const DEV: &str = "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"localhost\"\n";

fn env(trusted: bool) -> Env {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(pg.clone())).unwrap();
    let mut slot = None;
    let f = Fixture::build(trusted, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    f.write_db("dev.toml", DEV);
    Env {
        f,
        sessions: slot.unwrap(),
        pg,
    }
}

fn tables(n: usize) -> Vec<ObjectInfo> {
    (0..n)
        .map(|i| ObjectInfo {
            name: format!("t{i:05}"),
            kind: ObjectKind::Table,
            comment: None,
        })
        .collect()
}

impl Env {
    fn objects(&self) -> Value {
        json!({ "projectId": self.f.id, "connectionId": "dev", "kind": "table", "limit": 500 })
    }

    fn table(&self, method_params: &str) -> Value {
        json!({ "projectId": self.f.id, "connectionId": "dev",
                "object": { "schema": "public", "name": method_params } })
    }

    fn metadata_calls(&self, what: &str) -> usize {
        self.pg.count(&FakeCall::Metadata(match what {
            "objects" => "objects",
            "columns" => "columns",
            "indexes" => "indexes",
            "ddl" => "ddl",
            _ => unreachable!(),
        }))
    }

    async fn open(&self, w: &mut Conn) -> String {
        let opened = w
            .call(
                "db.session.open",
                json!({ "projectId": self.f.id, "connectionId": "dev" }),
            )
            .await
            .unwrap();
        opened["session"]["sessionId"].as_str().unwrap().to_owned()
    }
}

/// 실행해 끝까지 모은다. 확인을 물으면 받은 해시로 한 번 확인한다(창이 사람의 확인을 받은 흐름).
async fn run_confirmed(w: &mut Conn, session: &str, sql: &str) -> Vec<Value> {
    let params = json!({ "sessionId": session, "sql": sql });
    match run(w, params.clone()).await {
        Ok(items) => items,
        Err(asked) if asked.code == code::CONFIRM_REQUIRED => {
            let data = asked.data.unwrap();
            let mut params = params;
            params["confirmed"] =
                json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });
            run(w, params).await.unwrap()
        }
        Err(e) => panic!("{sql}: {e:?}"),
    }
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

fn schema_events(env: &Env) -> Subscription {
    env.f.bus.subscribe(
        Filter::all()
            .scope(Scope::Project(env.f.id.clone()))
            .topics(["db.schemaChanged"]),
    )
}

async fn next_schema_change(events: &mut Subscription) -> Value {
    let received = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("no db.schemaChanged")
        .unwrap();
    let Received::Event(event) = received else {
        panic!("lagged")
    };
    event.payload.clone()
}

fn no_more(events: &mut Subscription) {
    assert!(
        events.try_recv().is_none(),
        "an unexpected db.schemaChanged"
    );
}

fn denied(e: &RpcError) -> bool {
    e.code == rpc_code::PERMISSION_DENIED
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

/// ① 1만 개 목록의 첫 쪽은 드라이버를 한 번만 부르고(서버 쪽 쪽 나누기 — 500개), 같은 쪽 두 번째는 캐시다. 다음 쪽 · 다른 필터는
/// 따로 읽는다. 메타데이터용 연결은 하나만 연다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_page_of_ten_thousand_reads_the_driver_once_then_the_cache() {
    let e = env(true);
    e.pg.set_objects(tables(10_000));
    let mut w = e.f.window();

    let first = w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 500);
    assert_eq!(first["items"][0]["name"], "t00000");
    assert_eq!(first["nextCursor"], "500");
    assert_eq!(e.metadata_calls("objects"), 1);

    let again = w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(
        e.metadata_calls("objects"),
        1,
        "the same page comes from the cache"
    );

    let mut next = e.objects();
    next["cursor"] = first["nextCursor"].clone();
    let second = w.call("db.schema.objects", next).await.unwrap();
    assert_eq!(second["items"][0]["name"], "t00500");
    let mut filtered = e.objects();
    filtered["filter"] = json!("t0999");
    let found = w.call("db.schema.objects", filtered).await.unwrap();
    assert_eq!(found["items"].as_array().unwrap().len(), 10);
    assert_eq!(e.metadata_calls("objects"), 3);
    assert_eq!(e.pg.count(&FakeCall::Connect), 1);
    assert_eq!(e.sessions.schema_state().connections_opened(), 1);
}

/// 쪽 크기 1 ~ 500 밖은 드라이버에 닿기 전에 거절한다. 그 밖 목록(열 등)도 `cursor` · `limit` 으로 자른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limits_are_checked_and_other_lists_are_paged() {
    let e = env(true);
    let columns: Vec<ColumnInfo> = (0..7)
        .map(|i| ColumnInfo {
            name: format!("c{i}"),
            db_type: "int".into(),
            nullable: true,
            default: None,
            comment: None,
        })
        .collect();
    e.pg.set_table(
        "wide",
        FakeTable {
            columns,
            ..FakeTable::default()
        },
    );
    let mut w = e.f.window();
    for limit in [0, 501] {
        let mut params = e.objects();
        params["limit"] = json!(limit);
        let err = w.call("db.schema.objects", params).await.unwrap_err();
        assert_eq!(err.code, rpc_code::INVALID_PARAMS, "{limit}");
    }
    assert!(e.pg.calls().is_empty());

    let mut params = e.table("wide");
    params["limit"] = json!(5);
    let first = w.call("db.schema.columns", params.clone()).await.unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 5);
    params["cursor"] = first["nextCursor"].clone();
    let rest = w.call("db.schema.columns", params).await.unwrap();
    assert_eq!(rest["items"][0]["name"], "c5");
    assert!(rest["nextCursor"].is_null());
    assert_eq!(
        e.metadata_calls("columns"),
        1,
        "both pages slice one cached list"
    );
}

/// ② 콘솔에서 DDL 문장이 끝나면 `db.schemaChanged` 가 오고 다음 조회는 드라이버로 간다. 실패한 DDL 도 같다(MySQL 은 실패한 DDL 도
/// 암묵 커밋한다). 조회 · 쓰기는 캐시를 버리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ddl_statement_drops_the_cache_and_says_so() {
    let e = env(true);
    e.pg.set_objects(tables(3));
    e.pg.respond("CREATE", FakeResponse::write(0));
    e.pg.respond("INSERT", FakeResponse::write(1));
    e.pg.respond(
        "ALTER",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "no such column").with_code("42703")),
    );
    let mut events = schema_events(&e);
    let mut w = e.f.window();
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 1);

    let s = e.open(&mut w).await;
    run_confirmed(&mut w, &s, "SELECT 1").await;
    run_confirmed(&mut w, &s, "INSERT INTO t00000 VALUES (1)").await;
    no_more(&mut events);
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(
        e.metadata_calls("objects"),
        1,
        "queries and writes keep the cache"
    );

    run_confirmed(&mut w, &s, "CREATE TABLE t9 (id int)").await;
    let changed = next_schema_change(&mut events).await;
    assert_eq!(
        changed,
        json!({ "projectId": e.f.id, "connectionId": "dev" })
    );
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 2);

    let items = run_confirmed(&mut w, &s, "ALTER TABLE t9 DROP COLUMN nope").await;
    assert_eq!(items.last().unwrap()["kind"], "error");
    next_schema_change(&mut events).await;
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 3);
}

/// 메타데이터용 연결은 사람의 세션과 따로라 커밋된 스키마만 본다: 수동 커밋 탭의 커밋 전 `CREATE TABLE` 은 그 DDL 의 무효화 뒤에도
/// 보이지 않고(가짜는 커밋 전에는 목록을 바꾸지 않는다), 그 탭의 `COMMIT`(메서드 · 문장 둘 다)이 캐시를 다시 버려 보인다. 메타데이터는
/// 세션의 연결에서 읽지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncommitted_ddl_shows_after_the_commit_drops_the_cache() {
    let e = env(true);
    e.pg.set_objects(tables(1));
    e.pg.respond("CREATE", FakeResponse::write(0));
    let mut events = schema_events(&e);
    let mut w = e.f.window();
    let s = e.open(&mut w).await;
    w.call(
        "db.session.setMode",
        json!({ "sessionId": s, "mode": "manual" }),
    )
    .await
    .unwrap();

    run_confirmed(&mut w, &s, "CREATE TABLE fresh (id int)").await;
    next_schema_change(&mut events).await;
    let before = w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(
        before["items"].as_array().unwrap().len(),
        1,
        "not committed yet"
    );
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 1);

    // 커밋하면 그 연결의 다른 연결에도 보인다.
    let mut committed = tables(1);
    committed.push(ObjectInfo {
        name: "fresh".into(),
        kind: ObjectKind::Table,
        comment: None,
    });
    e.pg.set_objects(committed.clone());
    w.call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap();
    next_schema_change(&mut events).await;
    let after = w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(after["items"][1]["name"], "fresh");
    assert_eq!(e.metadata_calls("objects"), 2);

    // SQL 로 친 `COMMIT`(파이프라인이 드라이버의 `commit` 으로 옮긴다)도 같다.
    run_confirmed(&mut w, &s, "CREATE TABLE fresh2 (id int)").await;
    next_schema_change(&mut events).await;
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 3);
    run_confirmed(&mut w, &s, "COMMIT").await;
    next_schema_change(&mut events).await;
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 4);

    // 세션의 연결(0번)은 메타데이터를 읽지 않았다.
    assert!(
        !e.pg
            .calls_of(0)
            .iter()
            .any(|c| matches!(c, FakeCall::Metadata(_)))
    );
    assert!(
        e.pg.calls_of(1)
            .iter()
            .all(|c| matches!(c, FakeCall::Connect | FakeCall::Metadata(_)))
    );
}

/// `db.schemaChanged` 는 그 프로젝트 범위로만 나간다 — 다른 프로젝트 · Host 범위 구독에는 오지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schema_changes_reach_only_the_project() {
    let e = env(true);
    let mut ours = schema_events(&e);
    let mut theirs = e.f.bus.subscribe(
        Filter::all()
            .scope(Scope::Project(e.f.other.clone()))
            .topics(["db.schemaChanged"]),
    );
    let mut host = e.f.bus.subscribe(
        Filter::all()
            .scope(Scope::Host)
            .topics(["db.schemaChanged"]),
    );
    let mut w = e.f.window();
    w.call(
        "db.schema.refresh",
        json!({ "projectId": e.f.id, "connectionId": "dev" }),
    )
    .await
    .unwrap();
    next_schema_change(&mut ours).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    no_more(&mut theirs);
    no_more(&mut host);
}

/// 수동 새로 고침은 캐시를 버리고 알린다. 연결을 열지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_manual_refresh_drops_the_cache() {
    let e = env(true);
    e.pg.set_objects(tables(3));
    let mut events = schema_events(&e);
    let mut w = e.f.window();
    w.call("db.schema.objects", e.objects()).await.unwrap();
    w.call(
        "db.schema.refresh",
        json!({ "projectId": e.f.id, "connectionId": "dev" }),
    )
    .await
    .unwrap();
    next_schema_change(&mut events).await;
    assert_eq!(e.pg.count(&FakeCall::Connect), 1);
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 2);
}

/// ④ 다른 프로젝트 창 · 웰컴(Host 범위) · 파일 창의 `db.schema.*` 는 `PERMISSION_DENIED` 이고 드라이버가 불리지 않는다 — 그 프로젝트
/// 창이 채운 캐시도 내주지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_windows_get_no_schema_even_from_the_cache() {
    let e = env(true);
    e.pg.set_objects(tables(3));
    let file = e.f.root.join("a.sql");
    std::fs::write(&file, "SELECT 1").unwrap();
    let file_scope = pw_rpc::Caller::file(pw_rpc::Role::Owner, file).scope;
    let calls = |e: &Env| {
        vec![
            (
                "db.schema.databases",
                json!({ "projectId": e.f.id, "connectionId": "dev" }),
            ),
            (
                "db.schema.schemas",
                json!({ "projectId": e.f.id, "connectionId": "dev" }),
            ),
            ("db.schema.objects", e.objects()),
            ("db.schema.columns", e.table("t00000")),
            ("db.schema.indexes", e.table("t00000")),
            ("db.schema.foreignKeys", e.table("t00000")),
            ("db.schema.ddl", e.table("t00000")),
            (
                "db.schema.refresh",
                json!({ "projectId": e.f.id, "connectionId": "dev" }),
            ),
        ]
    };
    let scopes = [
        ConnectionScope::Project(e.f.other.clone()),
        ConnectionScope::Host,
        file_scope,
    ];
    for scope in &scopes {
        let mut w = e.f.connect(scope.clone());
        for (method, params) in calls(&e) {
            let err = w.call(method, params).await.unwrap_err();
            assert!(denied(&err), "{method} from {scope:?}: {err:?}");
        }
    }
    assert!(e.pg.calls().is_empty());

    // 그 프로젝트 창이 캐시를 채운 뒤에도 같다.
    let mut ours = e.f.window();
    // 가짜 드라이버에는 DB 목록이 없다(「지원하지 않음」) — 나머지로 캐시를 채운다.
    for (method, params) in calls(&e).into_iter().skip(1) {
        ours.call(method, params).await.unwrap();
    }
    let before = e.pg.calls().len();
    for scope in &scopes {
        let mut w = e.f.connect(scope.clone());
        for (method, params) in calls(&e) {
            let err = w.call(method, params).await.unwrap_err();
            assert!(
                denied(&err),
                "{method} from {scope:?} after the cache filled"
            );
        }
    }
    assert_eq!(e.pg.calls().len(), before);
}

/// ④ 미신뢰 프로젝트의 `db.schema.*` 는 `untrusted` 이고 드라이버가 불리지 않는다. 신뢰했다가 거두면 캐시에 든 것도 내주지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_projects_get_no_schema_even_from_the_cache() {
    let e = env(false);
    e.pg.set_objects(tables(3));
    let mut w = e.f.window();
    for (method, params) in [
        (
            "db.schema.databases",
            json!({ "projectId": e.f.id, "connectionId": "dev" }),
        ),
        ("db.schema.objects", e.objects()),
        ("db.schema.columns", e.table("t00000")),
        ("db.schema.ddl", e.table("t00000")),
    ] {
        let err = w.call(method, params).await.unwrap_err();
        assert_eq!(reason(&err), Some("untrusted"), "{method}");
    }
    assert!(e.pg.calls().is_empty());

    e.f.projects.set_trust(&e.f.id, true).unwrap();
    w.call("db.schema.objects", e.objects()).await.unwrap();
    let before = e.pg.calls().len();
    e.f.projects.set_trust(&e.f.id, false).unwrap();
    let err = w.call("db.schema.objects", e.objects()).await.unwrap_err();
    assert_eq!(reason(&err), Some("untrusted"));
    assert_eq!(e.pg.calls().len(), before);
}

/// 메타데이터용 연결은 그것을 연 창의 RPC 연결이 닫히면 닫힌다(사람이 누를 때만 연결이 열려 있다 — 결정 M3-D13).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_window_closes_its_metadata_connection() {
    let e = env(true);
    e.pg.set_objects(tables(3));
    let mut w = e.f.window();
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.pg.count(&FakeCall::Close), 0);
    drop(w);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while e.pg.count(&FakeCall::Close) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the metadata connection stayed open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // 다시 연 창은 새 연결을 연다. 캐시는 그대로다(스키마는 바뀌지 않았다).
    let mut w = e.f.window();
    w.call("db.schema.columns", e.table("t00000"))
        .await
        .unwrap();
    assert_eq!(e.pg.count(&FakeCall::Connect), 2);
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 1);
}

/// 연결 설정이 바뀌면(다른 서버를 가리킬 수 있다) 그 설정으로 채운 캐시를 버리고 메타데이터용 연결을 다시 연다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_config_drops_the_cache_and_reconnects() {
    let e = env(true);
    e.pg.set_objects(tables(3));
    let mut w = e.f.window();
    w.call("db.schema.objects", e.objects()).await.unwrap();
    e.f.edit_db("dev.toml", "host = \"localhost\"", "host = \"127.0.0.1\"");
    w.call("db.schema.objects", e.objects()).await.unwrap();
    assert_eq!(e.metadata_calls("objects"), 2);
    assert_eq!(e.pg.count(&FakeCall::Connect), 2);
    assert_eq!(e.pg.count(&FakeCall::Close), 1);
}

/// 끊긴 메타데이터용 연결은 버리고 다음 조회가 다시 연다. 오류는 캐시에 남지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_metadata_connection_is_replaced() {
    let e = env(true);
    e.pg.set_objects(tables(3));
    let mut w = e.f.window();
    w.call("db.schema.objects", e.objects()).await.unwrap();
    e.pg.fail_metadata(Some(DbError::new(DbErrorKind::Disconnected, "gone")));
    let err = w
        .call("db.schema.columns", e.table("t00000"))
        .await
        .unwrap_err();
    assert_eq!(err.code, code::DB_ERROR);
    e.pg.fail_metadata(None);
    w.call("db.schema.columns", e.table("t00000"))
        .await
        .unwrap();
    assert_eq!(e.pg.count(&FakeCall::Connect), 2);
}

/// 스키마 단계가 있는 드라이버(PG)는 연결이 그 DB 의 카탈로그만 읽는다 — 다른 DB 를 가리키는 요청은 그 DB 로 연 연결에서 읽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_database_gets_its_own_metadata_connection() {
    let e = env(true);
    e.pg.set_objects(tables(3));
    let mut w = e.f.window();
    w.call("db.schema.objects", e.objects()).await.unwrap();
    let mut other = e.objects();
    other["database"] = json!("analytics");
    w.call("db.schema.objects", other.clone()).await.unwrap();
    w.call("db.schema.objects", other).await.unwrap();
    assert_eq!(e.pg.count(&FakeCall::Connect), 2);
    assert_eq!(e.metadata_calls("objects"), 2);
}

/// 인덱스 · DDL 도 객체마다 캐시한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn indexes_and_ddl_are_cached_per_object() {
    let e = env(true);
    e.pg.set_table(
        "users",
        FakeTable {
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    db_type: "int".into(),
                    nullable: false,
                    default: None,
                    comment: None,
                },
                ColumnInfo {
                    name: "email".into(),
                    db_type: "text".into(),
                    nullable: true,
                    default: None,
                    comment: None,
                },
            ],
            indexes: vec![IndexInfo {
                name: "users_email_key".into(),
                columns: vec!["email".into()],
                primary: false,
                unique: true,
                partial: false,
            }],
            ..FakeTable::default()
        },
    );
    let mut w = e.f.window();
    let indexes = w.call("db.schema.indexes", e.table("users")).await.unwrap();
    assert_eq!(indexes["items"][0]["name"], "users_email_key");
    assert_eq!(e.metadata_calls("indexes"), 1);
    w.call("db.schema.indexes", e.table("users")).await.unwrap();
    assert_eq!(e.metadata_calls("indexes"), 1);
    let ddl = w.call("db.schema.ddl", e.table("users")).await.unwrap();
    assert_eq!(ddl["text"], "");
    w.call("db.schema.ddl", e.table("users")).await.unwrap();
    assert_eq!(e.metadata_calls("ddl"), 1);
}
