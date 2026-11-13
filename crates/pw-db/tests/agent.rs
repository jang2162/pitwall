//! M5-06 의 완료 기준(16-m5-plan.md M5-06 행): 에이전트의 DB 관문(`pw_db::gate::db_grant_for_agent`)과 툴 메서드
//! `db_connection_list`(`db.tool.connectionList`) · `db_schema`(`db.tool.schema`)를 가짜 드라이버로 실제 RPC 세션 너머에서 본다.
//! MCP 세션은 그 연결 범위(`ConnectionScope::Agent`)로 붙인 RPC 연결이다 — MCP 왕복 자체는 데스크톱 크레이트의 `mcp_tests.rs`.
//!
//! 「사람이 연 연결」 은 그 프로젝트 창의 연결(`ConnectionScope::Project`)이 `db.schema.objects` · `db.session.open` 으로 연 것이다.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Conn, Fixture, fake_worktree};
use pw_db::DbSessions;
use pw_db::schema::IDLE_CLOSE;
use pw_db_driver::fake::kv::FakeKvDriver;
use pw_db_driver::fake::{FakeCall, FakeDriver, FakeTable};
use pw_db_driver::{BoxFuture, Connected, DbDriver, DbResult};
use pw_db_types::{
    Capabilities, ColumnInfo, ConnectionSpec, ForeignKeyInfo, IndexInfo, ObjectInfo, ObjectKind,
    ObjectRef, SqlDialect,
};
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

/// 가짜 KV 드라이버를 설정 파일의 `driver = "redis"` 자리에 끼운다(`tests/kv.rs` 와 같다).
struct AsRedis(FakeKvDriver);

impl DbDriver for AsRedis {
    fn id(&self) -> &str {
        "redis"
    }

    fn dialect(&self) -> Option<SqlDialect> {
        None
    }

    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        self.0.connect(spec)
    }
}

struct Env {
    f: Fixture,
    sessions: Arc<DbSessions>,
    pg: FakeDriver,
}

const DEV: &str =
    "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"localhost\"\ndatabase = \"app\"\n";
const PROD: &str = "schema = 1\nname = \"prod\"\ndriver = \"postgres\"\nhost = \"db.prod.example\"\ndatabase = \"app\"\nuser = \"app\"\nproduction = true\n";
/// [`PROD`] 와 같은 대상을 표지 없이 가리키는 다른 연결.
const SNEAKY: &str = "schema = 1\nname = \"sneaky\"\ndriver = \"postgres\"\nhost = \"DB.prod.example\"\nport = 5432\ndatabase = \"app\"\nuser = \"app\"\n";

fn env(trusted: bool) -> Env {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(pg.clone())).unwrap();
    drivers
        .register(Arc::new(AsRedis(FakeKvDriver::new())))
        .unwrap();
    let mut slot = None;
    let f = Fixture::build(trusted, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    f.write_db("dev.toml", DEV);
    pg.set_objects(vec![table("users"), table("orders")]);
    Env {
        f,
        sessions: slot.unwrap(),
        pg,
    }
}

fn table(name: &str) -> ObjectInfo {
    ObjectInfo {
        name: name.to_owned(),
        kind: ObjectKind::Table,
        comment: None,
    }
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

#[track_caller]
fn refused(result: Result<Value, RpcError>, why: &str) {
    let e = result.expect_err("refused");
    assert_eq!(
        (e.code, reason(&e)),
        (rpc_code::PERMISSION_DENIED, Some(why)),
        "{e:?}"
    );
}

impl Env {
    fn agent(&self) -> Conn {
        self.f.connect(ConnectionScope::Agent)
    }

    /// 연결 워크트리 `p1-wt` 를 만들고 그 꼭대기를 돌려준다. `files` 는 그 워크트리의 `.pitwall/db/` 에 쓴다.
    fn worktree(&self, files: &[(&str, &str)]) -> String {
        let wt = fake_worktree(&self.f.root, &self.f.base.join("p1-wt"));
        std::fs::create_dir_all(wt.join(".pitwall/db")).unwrap();
        for (name, text) in files {
            std::fs::write(wt.join(".pitwall/db").join(name), text).unwrap();
        }
        wt.to_string_lossy().into_owned()
    }

    /// 사람이 그 프로젝트 창의 탐색기에서 연결을 펼친다(메타데이터용 연결을 연다).
    async fn human_opens(&self, connection: &str, worktree: Option<&str>) {
        let mut w = self.f.window();
        w.call(
            "db.schema.objects",
            json!({ "projectId": self.f.id, "worktree": worktree, "connectionId": connection,
                    "kind": "table", "limit": 500 }),
        )
        .await
        .unwrap();
    }

    async fn schema(
        &self,
        agent: &mut Conn,
        connection: &str,
        worktree: Option<&str>,
        extra: Value,
    ) -> Result<Value, RpcError> {
        let mut params =
            json!({ "projectId": self.f.id, "worktree": worktree, "connectionId": connection });
        if let Value::Object(extra) = extra {
            params.as_object_mut().unwrap().extend(extra);
        }
        agent.call("db.tool.schema", params).await
    }

    async fn list(&self, agent: &mut Conn, worktree: Option<&str>) -> Value {
        agent
            .call(
                "db.tool.connectionList",
                json!({ "projectId": self.f.id, "worktree": worktree }),
            )
            .await
            .unwrap()
    }
}

fn entry<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("{id} not in {list}"))
}

/// 창이 연 적 없는 연결은 `notOpenedByUser` 이고 드라이버에 닿지 않는다. 창이 한 번 연 뒤에는 되고, 창을 닫아도(DB 탭을 닫아도)
/// 앱을 끌 때까지 된다. 창이 연 메타데이터용 연결이 살아 있으면 그것을 쓴다(새로 열지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_schema_needs_a_connection_the_user_opened() {
    let e = env(true);
    let mut agent = e.agent();
    refused(
        e.schema(&mut agent, "dev", None, json!({})).await,
        "notOpenedByUser",
    );
    assert_eq!(
        e.pg.count(&FakeCall::Connect),
        0,
        "the driver was not reached"
    );
    let list = e.list(&mut agent, None).await;
    assert_eq!(entry(&list, "dev")["usable"], false);
    assert_eq!(entry(&list, "dev")["unusableReason"], "notOpenedByUser");

    let mut w = e.f.window();
    w.call(
        "db.schema.objects",
        json!({ "projectId": e.f.id, "connectionId": "dev", "kind": "view", "limit": 10 }),
    )
    .await
    .unwrap();
    assert_eq!(e.sessions.schema_state().connections_opened(), 1);
    let tables = e.schema(&mut agent, "dev", None, json!({})).await.unwrap();
    assert_eq!(tables["level"], "objects");
    let names: Vec<&str> = tables["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["users", "orders"]);
    assert_eq!(
        e.sessions.schema_state().connections_opened(),
        1,
        "the window's metadata connection was reused"
    );
    let list = e.list(&mut agent, None).await;
    assert_eq!(entry(&list, "dev")["usable"], true);

    drop(w);
    let again = e
        .schema(&mut agent, "dev", None, json!({ "filter": "ord" }))
        .await
        .unwrap();
    assert_eq!(again["items"][0]["name"], "orders");
}

/// 메인 창에서 연 뒤 워크트리의 같은 id 가 다른 대상이면 `notOpenedByUser`(같은 대상이면 된다). 그 뒤 메인의 설정 파일이
/// 대상을 바꾸면 다시 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_id_on_another_target_is_not_opened() {
    let e = env(true);
    e.human_opens("dev", None).await;
    let elsewhere = DEV.replace("localhost", "other.example");
    let wt = e.worktree(&[("dev.toml", &elsewhere), ("same.toml", DEV)]);
    let mut agent = e.agent();
    refused(
        e.schema(&mut agent, "dev", Some(&wt), json!({})).await,
        "notOpenedByUser",
    );
    e.schema(&mut agent, "dev", None, json!({})).await.unwrap();
    // 같은 대상이어도 연결 id 가 다르면 사람이 연 연결이 아니다.
    refused(
        e.schema(&mut agent, "same", Some(&wt), json!({})).await,
        "notOpenedByUser",
    );
    std::fs::write(e.f.base.join("p1-wt/.pitwall/db/dev.toml"), DEV).unwrap();
    e.schema(&mut agent, "dev", Some(&wt), json!({}))
        .await
        .unwrap();

    e.f.edit_db("dev.toml", "localhost", "other.example");
    refused(
        e.schema(&mut agent, "dev", None, json!({})).await,
        "notOpenedByUser",
    );
    e.f.edit_db("dev.toml", "database = \"app\"", "database = \"other\"");
    refused(
        e.schema(&mut agent, "dev", None, json!({})).await,
        "notOpenedByUser",
    );
}

/// 미신뢰 프로젝트는 `untrusted`. 목록은 준다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_is_refused_but_listed() {
    let e = env(false);
    let mut agent = e.agent();
    refused(
        e.schema(&mut agent, "dev", None, json!({})).await,
        "untrusted",
    );
    let list = e.list(&mut agent, None).await;
    assert_eq!(list["trusted"], false);
    let dev = entry(&list, "dev");
    assert_eq!(
        (&dev["state"], &dev["usable"], &dev["unusableReason"]),
        (&json!("untrusted"), &json!(false), &json!("untrusted"))
    );
    assert_eq!(dev["host"], "localhost");
}

/// 프로덕션 연결은 사람이 열었어도 거절한다(`productionBlocked`). 목록도 그렇게 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_production_connection_is_refused() {
    let e = env(true);
    e.f.write_db("prod.toml", PROD);
    e.human_opens("prod", None).await;
    let mut agent = e.agent();
    refused(
        e.schema(&mut agent, "prod", None, json!({})).await,
        "productionBlocked",
    );
    let list = e.list(&mut agent, None).await;
    let prod = entry(&list, "prod");
    assert_eq!(
        (
            &prod["production"],
            &prod["usable"],
            &prod["unusableReason"]
        ),
        (&json!(true), &json!(false), &json!("productionBlocked"))
    );
}

/// 메인 창에서 프로덕션 연결을 연 뒤 워크트리의 같은 id 에서 `production` 을 지워도 거절된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_production_in_a_worktree_does_not_lift_it() {
    let e = env(true);
    e.f.write_db("prod.toml", PROD);
    e.human_opens("prod", None).await;
    let unmarked = PROD.replace("production = true\n", "");
    let wt = e.worktree(&[("prod.toml", &unmarked)]);
    e.human_opens("prod", Some(&wt)).await;
    let mut agent = e.agent();
    refused(
        e.schema(&mut agent, "prod", Some(&wt), json!({})).await,
        "productionBlocked",
    );
}

/// 프로덕션으로 연 대상을 표지 없는 새 연결 id 로 가리켜도 거절되고 목록이 그 연결을 프로덕션으로 보인다 — 메인 체크아웃의
/// 설정에서 그 연결을 지운 뒤에도(이번 실행에서 본 표지는 대상에 남는다). 사람이 새 연결을 열었어도 그렇다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_unmarked_id_on_a_production_target_is_still_production() {
    let e = env(true);
    e.f.write_db("prod.toml", PROD);
    e.human_opens("prod", None).await;
    std::fs::remove_file(e.f.db_file("prod.toml")).unwrap();
    let wt = e.worktree(&[("sneaky.toml", SNEAKY)]);
    e.human_opens("sneaky", Some(&wt)).await;
    let mut agent = e.agent();
    refused(
        e.schema(&mut agent, "sneaky", Some(&wt), json!({})).await,
        "productionBlocked",
    );
    let list = e.list(&mut agent, Some(&wt)).await;
    let sneaky = entry(&list, "sneaky");
    assert_eq!(
        (&sneaky["production"], &sneaky["unusableReason"]),
        (&json!(true), &json!("productionBlocked"))
    );
}

/// 사람이 연 적 없어도, 메인 체크아웃의 설정이 프로덕션으로 적은 대상을 워크트리의 표지 없는 새 연결로 가리키면 거절되고 목록이
/// 그 연결을 프로덕션으로 보인다 — 에이전트의 DB 툴 호출이 메인 체크아웃의 설정도 읽는다. 둘 다 그 호출이 처음 읽는 때다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_main_checkout_marks_a_target_nobody_opened() {
    for via_list in [false, true] {
        let e = env(true);
        e.f.write_db("prod.toml", PROD);
        let wt = e.worktree(&[("sneaky.toml", SNEAKY)]);
        // 사람은 워크트리 창에서 새 연결만 연다 — 메인 체크아웃의 설정은 아무도 읽지 않았다.
        e.human_opens("sneaky", Some(&wt)).await;
        let mut agent = e.agent();
        if via_list {
            let list = e.list(&mut agent, Some(&wt)).await;
            let sneaky = entry(&list, "sneaky");
            assert_eq!(
                (&sneaky["production"], &sneaky["unusableReason"]),
                (&json!(true), &json!("productionBlocked"))
            );
        } else {
            refused(
                e.schema(&mut agent, "sneaky", Some(&wt), json!({})).await,
                "productionBlocked",
            );
        }
    }
}

/// 사람이 연 뒤 `production` 을 더하면 곧바로 거절된다. read-only 표지도 조여지기만 한다 — 설정에서 지워도 목록은 read-only 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adding_a_mark_applies_at_once_and_removing_it_does_not() {
    let e = env(true);
    e.f.write_db(
        "ro.toml",
        &DEV.replace("name = \"dev\"", "name = \"ro\"\nread_only = true"),
    );
    e.human_opens("dev", None).await;
    let mut agent = e.agent();
    e.schema(&mut agent, "dev", None, json!({})).await.unwrap();
    e.f.edit_db(
        "dev.toml",
        "database = \"app\"\n",
        "database = \"app\"\nproduction = true\n",
    );
    refused(
        e.schema(&mut agent, "dev", None, json!({})).await,
        "productionBlocked",
    );

    let list = e.list(&mut agent, None).await;
    assert_eq!(entry(&list, "ro")["readOnly"], true);
    e.f.edit_db("ro.toml", "read_only = true\n", "");
    let list = e.list(&mut agent, None).await;
    assert_eq!(entry(&list, "ro")["readOnly"], true, "still read-only");
}

/// UI 의 RPC `db.schema.objects` 는 MCP 세션 범위에서 여전히 `notProjectWindow` 이고, 툴 메서드는 창 범위를 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ui_rpcs_still_refuse_an_mcp_session_and_the_tools_refuse_a_window() {
    let e = env(true);
    e.human_opens("dev", None).await;
    let mut agent = e.agent();
    refused(
        agent
            .call(
                "db.schema.objects",
                json!({ "projectId": e.f.id, "connectionId": "dev", "kind": "table", "limit": 10 }),
            )
            .await,
        "notProjectWindow",
    );
    let mut w = e.f.window();
    refused(
        w.call(
            "db.tool.schema",
            json!({ "projectId": e.f.id, "connectionId": "dev" }),
        )
        .await,
        "notAgentSession",
    );
    refused(
        w.call("db.tool.connectionList", json!({ "projectId": e.f.id }))
            .await,
        "notAgentSession",
    );
}

/// 객체 1,200개를 쪽으로 넘긴다(기본 100, 상한 500). 서버 쪽 필터.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_schema_pages_through_1200_objects() {
    let e = env(true);
    e.pg.set_objects((0..1200).map(|i| table(&format!("t{i:04}"))).collect());
    e.human_opens("dev", None).await;
    let mut agent = e.agent();
    let first = e.schema(&mut agent, "dev", None, json!({})).await.unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 100);
    let mut seen = 0;
    let mut cursor = Value::Null;
    let mut pages = 0;
    loop {
        let page = e
            .schema(
                &mut agent,
                "dev",
                None,
                json!({ "limit": 500, "cursor": cursor }),
            )
            .await
            .unwrap();
        seen += page["items"].as_array().unwrap().len();
        pages += 1;
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!((seen, pages), (1200, 3));
    let found = e
        .schema(&mut agent, "dev", None, json!({ "filter": "t119" }))
        .await
        .unwrap();
    assert_eq!(found["items"].as_array().unwrap().len(), 10);
    let e2 = e
        .schema(&mut agent, "dev", None, json!({ "limit": 501 }))
        .await
        .unwrap_err();
    assert_eq!(e2.code, rpc_code::INVALID_PARAMS);
    // 단계 목록의 스키마 단계(가짜 드라이버는 `public` 하나 — DB 단계는 live 시험이 본다).
    let schemas = e
        .schema(&mut agent, "dev", None, json!({ "kind": "schema" }))
        .await
        .unwrap();
    assert_eq!(
        (&schemas["level"], &schemas["items"][0]["name"]),
        (&json!("schemas"), &json!("public"))
    );
}

/// `object` 를 주면 열(기본 키 표시) · 인덱스 · FK, `detail: "ddl"` 이면 DDL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_schema_describes_one_object() {
    let e = env(true);
    let column = |name: &str, nullable| ColumnInfo {
        name: name.into(),
        db_type: "int4".into(),
        nullable,
        default: None,
        comment: None,
    };
    e.pg.set_table(
        "users",
        FakeTable {
            columns: vec![column("id", false), column("team_id", true)],
            indexes: vec![IndexInfo {
                name: "users_pkey".into(),
                columns: vec!["id".into()],
                primary: true,
                unique: true,
                partial: false,
            }],
            foreign_keys: vec![ForeignKeyInfo {
                name: "users_team".into(),
                columns: vec!["team_id".into()],
                references: ObjectRef {
                    database: None,
                    schema: Some("public".into()),
                    name: "teams".into(),
                },
                referenced_columns: vec!["id".into()],
            }],
            ddl: "CREATE TABLE users (...)".into(),
        },
    );
    e.human_opens("dev", None).await;
    let mut agent = e.agent();
    let users = e
        .schema(&mut agent, "dev", None, json!({ "object": "public.users" }))
        .await
        .unwrap();
    assert_eq!(users["level"], "object");
    assert_eq!(users["object"]["schema"], "public");
    assert_eq!(users["object"]["name"], "users");
    assert_eq!(users["columns"][0]["name"], "id");
    assert_eq!(users["columns"][0]["type"], "int4");
    assert_eq!(users["columns"][0]["primaryKey"], true);
    assert_eq!(users["columns"][1]["primaryKey"], false);
    assert_eq!(users["columns"][1]["nullable"], true);
    assert_eq!(users["indexes"][0]["name"], "users_pkey");
    assert_eq!(users["foreignKeys"][0]["references"]["name"], "teams");
    let ddl = e
        .schema(
            &mut agent,
            "dev",
            None,
            json!({ "object": "users", "schema": "public", "detail": "ddl" }),
        )
        .await
        .unwrap();
    assert_eq!(
        (&ddl["level"], &ddl["ddl"]),
        (&json!("ddl"), &json!("CREATE TABLE users (...)"))
    );
}

/// Redis 연결은 사람이 열었어도 `unsupported` 이고 `db_kv_scan` 을 가리킨다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_connection_is_unsupported() {
    let e = env(true);
    e.f.write_db(
        "cache.toml",
        "schema = 1\nname = \"cache\"\ndriver = \"redis\"\nhost = \"localhost\"\ndatabase = \"3\"\n",
    );
    let mut w = e.f.window();
    w.call(
        "db.session.open",
        json!({ "projectId": e.f.id, "connectionId": "cache" }),
    )
    .await
    .unwrap();
    let mut agent = e.agent();
    let error = e
        .schema(&mut agent, "cache", None, json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.code, pw_db::gate::code::DB_ERROR);
    assert_eq!(error.data.unwrap()["error"]["kind"], "unsupported");
    assert!(error.message.contains("db_kv_scan"), "{}", error.message);
}

/// `db_connection_list` 에는 비밀이 없다(키체인 비밀 · env 변수의 값).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_connection_list_has_no_secrets() {
    let e = env(true);
    e.f.write_db(
        "kc.toml",
        "schema = 1\nname = \"kc\"\ndriver = \"postgres\"\nhost = \"localhost\"\nuser = \"app\"\npassword = { secret_ref = \"keychain\" }\n",
    );
    e.f.write_db(
        "envpw.toml",
        "schema = 1\nname = \"envpw\"\ndriver = \"postgres\"\nhost = \"localhost\"\npassword = { env = \"PGPASSWORD_TEST\" }\n",
    );
    e.f.env.set("PGPASSWORD_TEST", "env-secret-value-1");
    let mut w = e.f.window();
    let shown = w
        .call(
            "db.connection.get",
            json!({ "projectId": e.f.id, "connectionId": "kc" }),
        )
        .await
        .unwrap();
    w.call(
        "db.secret.set",
        json!({ "projectId": e.f.id, "connectionId": "kc", "key": "password",
                "target": common::target_of(&shown["config"]), "value": "keychain-secret-value-2" }),
    )
    .await
    .unwrap();
    let mut agent = e.agent();
    let list = e.list(&mut agent, None).await;
    let text = list.to_string();
    for secret in ["env-secret-value-1", "keychain-secret-value-2"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    assert_eq!(entry(&list, "kc")["state"], "ready");
    assert_eq!(entry(&list, "envpw")["state"], "approvalRequired");
}

/// 에이전트 세션을 열고 닫기를 두 번 해도 툴이 연 메타데이터용 연결은 한 번만 열리고(세션이 닫혀도 두므로), 유휴 시간이 지나면
/// 닫힌다(가짜 시계).
#[tokio::test(start_paused = true)]
async fn a_tool_metadata_connection_outlives_the_session_until_idle() {
    let e = env(true);
    // 사람이 열고 창을 닫는다 — 창이 연 메타데이터용 연결은 창과 함께 닫힌다.
    e.human_opens("dev", None).await;
    wait(|| e.pg.count(&FakeCall::Close) == 1).await;
    assert_eq!(e.sessions.schema_state().connections_opened(), 1);

    for filter in ["us", "or"] {
        let mut agent = e.agent();
        e.schema(&mut agent, "dev", None, json!({ "filter": filter }))
            .await
            .unwrap();
        drop(agent);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert_eq!(
        e.sessions.schema_state().connections_opened(),
        2,
        "one metadata connection for both sessions"
    );
    assert_eq!(e.pg.count(&FakeCall::Close), 1, "still open");
    assert_eq!(e.sessions.schema_state().open_links(), 1);

    tokio::time::sleep(IDLE_CLOSE + Duration::from_secs(1)).await;
    wait(|| e.pg.count(&FakeCall::Close) == 2).await;
    assert_eq!(e.sessions.schema_state().open_links(), 0);
}

/// 가짜 시계에서: 조건이 설 때까지 다른 태스크에 차례를 준다.
async fn wait(done: impl Fn() -> bool) {
    for _ in 0..1000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the condition did not hold");
}

/// 실 SQLite 드라이버로 `db_schema` 왕복: 사람이 연 뒤 테이블 목록과 열 · 기본 키. SQLite 는 서버가 없어 live 가 아닌 보통
/// 시험이다(PG · MySQL · MariaDB 는 `tests/live_agent.rs`). 상대 경로의 파일은 그 워크트리 안의 파일이라 워크트리의 같은 id 는
/// 다른 대상이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_schema_round_trip_on_real_sqlite() {
    let mut drivers = pw_db::DriverRegistry::new();
    drivers
        .register(Arc::new(pw_db_driver_sqlite::SqliteDriver::new()))
        .unwrap();
    let f = Fixture::build(true, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    std::fs::write(f.root.join("app.db"), "").unwrap();
    let lite = "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n";
    f.write_db("lite.toml", lite);
    let mut w = f.window();
    let opened = w
        .call(
            "db.session.open",
            json!({ "projectId": f.id, "connectionId": "lite" }),
        )
        .await
        .unwrap();
    let session = opened["session"]["sessionId"].as_str().unwrap().to_owned();
    let params = json!({ "sessionId": session,
        "sql": "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)" });
    if let Err(asked) = execute(&mut w, params.clone()).await {
        assert_eq!(asked.code, pw_db::gate::code::CONFIRM_REQUIRED, "{asked:?}");
        let data = asked.data.unwrap();
        let mut params = params;
        params["confirmed"] =
            json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });
        execute(&mut w, params).await.unwrap();
    }

    let mut agent = f.connect(ConnectionScope::Agent);
    let call = |extra: Value| {
        let mut params = json!({ "projectId": f.id, "connectionId": "lite" });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        params
    };
    let tables = agent.call("db.tool.schema", call(json!({}))).await.unwrap();
    assert_eq!(tables["items"][0]["name"], "users");
    let users = agent
        .call("db.tool.schema", call(json!({ "object": "users" })))
        .await
        .unwrap();
    assert_eq!(users["columns"][0]["name"], "id");
    assert_eq!(users["columns"][0]["primaryKey"], true);
    assert_eq!(users["columns"][1]["nullable"], false);

    // 워크트리의 같은 설정(`path = "app.db"`)은 그 워크트리 안의 다른 파일이다.
    let wt = fake_worktree(&f.root, &f.base.join("p1-wt"));
    std::fs::create_dir_all(wt.join(".pitwall/db")).unwrap();
    std::fs::write(wt.join(".pitwall/db/lite.toml"), lite).unwrap();
    std::fs::write(wt.join("app.db"), "").unwrap();
    refused(
        agent
            .call(
                "db.tool.schema",
                call(json!({ "worktree": wt.to_string_lossy() })),
            )
            .await,
        "notOpenedByUser",
    );
}

async fn execute(w: &mut Conn, params: Value) -> Result<(), RpcError> {
    let stream = w.start("db.query.execute", params).await?;
    match w.collect(stream).await.1 {
        pw_rpc::protocol::StreamOutcome::Completed => Ok(()),
        pw_rpc::protocol::StreamOutcome::Failed { error } => Err(error),
        pw_rpc::protocol::StreamOutcome::Cancelled => panic!("cancelled"),
    }
}

/// 파일 감시가 다시 읽은 설정(`Connections::refresh`)도 표지를 더한다 — 그사이 `production` 을 달았다가 지워도, 다음 목록이
/// 그 대상을 프로덕션으로 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watched_change_also_marks_the_target() {
    let e = env(true);
    let mut agent = e.agent();
    let list = e.list(&mut agent, None).await;
    assert_eq!(entry(&list, "dev")["production"], false);
    e.f.edit_db(
        "dev.toml",
        "database = \"app\"\n",
        "database = \"app\"\nproduction = true\n",
    );
    e.f.connections.refresh(&e.f.id, None);
    e.f.edit_db("dev.toml", "production = true\n", "");
    let list = e.list(&mut agent, None).await;
    assert_eq!(entry(&list, "dev")["production"], true);
}
