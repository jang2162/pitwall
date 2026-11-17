//! M5-19 의 관문 · 쪽 시험(16-m5-plan.md M5-19 행): Redis 조회 툴 `db_kv_scan`(`db.tool.kvScan`) · `db_kv_get`(`db.tool.kvGet`)을
//! 가짜 KV 드라이버(M3-09)로 실제 RPC 세션 너머에서 본다. MCP 세션은 그 연결 범위(`ConnectionScope::Agent`)로 붙인 RPC 연결이다 —
//! 툴 목록 · MCP 왕복은 데스크톱 크레이트의 `mcp_tests.rs`, 실 Redis 는 `kv_live.rs` 의 `live_redis_kv_tool_*`.
//!
//! 「서버에 닿지 않았다」 는 가짜 드라이버의 호출 기록(`FakeKvDriver::calls`)으로 판정한다.

mod common;

use std::sync::Arc;

use common::{Conn, Fixture, fake_worktree};
use pw_db::DbSessions;
use pw_db::kv_tool::{DbToolKvGetParams, DbToolKvScanParams};
use pw_db_driver::fake::FakeDriver;
use pw_db_driver::fake::kv::{FakeKvCall, FakeKvDriver, FakeKvValue};
use pw_db_driver::{BoxFuture, Connected, DbDriver, DbResult};
use pw_db_types::{Capabilities, ConnectionSpec, SqlDialect};
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

const CACHE: &str = "schema = 1\nname = \"cache\"\ndriver = \"redis\"\nhost = \"cache.example\"\ndatabase = \"3\"\n";

struct Env {
    f: Fixture,
    kv: FakeKvDriver,
}

fn env(trusted: bool) -> Env {
    let kv = FakeKvDriver::new();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(AsRedis(kv.clone()))).unwrap();
    drivers
        .register(Arc::new(FakeDriver::named(
            "postgres",
            SqlDialect::Postgres,
        )))
        .unwrap();
    let f = Fixture::build(trusted, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    f.write_db("cache.toml", CACHE);
    Env { f, kv }
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

    /// 사람이 그 프로젝트 창에서 Redis 연결의 세션을 연다(키 브라우저를 연 것과 같다).
    async fn human_opens(&self, connection: &str, worktree: Option<&str>) {
        let mut w = self.f.window();
        w.call(
            "db.session.open",
            json!({ "projectId": self.f.id, "worktree": worktree, "connectionId": connection }),
        )
        .await
        .unwrap();
    }

    fn params(&self, connection: &str, extra: Value) -> Value {
        let mut params = json!({ "projectId": self.f.id, "connectionId": connection });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        params
    }

    async fn scan(
        &self,
        agent: &mut Conn,
        connection: &str,
        extra: Value,
    ) -> Result<Value, RpcError> {
        agent
            .call("db.tool.kvScan", self.params(connection, extra))
            .await
    }

    async fn get(
        &self,
        agent: &mut Conn,
        connection: &str,
        extra: Value,
    ) -> Result<Value, RpcError> {
        agent
            .call("db.tool.kvGet", self.params(connection, extra))
            .await
    }

    fn connects(&self) -> usize {
        self.kv
            .calls()
            .iter()
            .filter(|c| matches!(c, FakeKvCall::Connect { .. }))
            .count()
    }

    fn seed(&self, db: u32, key: &str, value: FakeKvValue) {
        self.kv.insert(db, key.as_bytes(), value);
    }

    /// 끝까지 쪽을 넘겨 모은 키 이름(글)과 쪽 수.
    async fn scan_all(
        &self,
        agent: &mut Conn,
        connection: &str,
        extra: Value,
    ) -> (Vec<String>, usize) {
        let mut keys = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = extra.clone();
            if let Some(c) = &cursor {
                params["cursor"] = json!(c);
            }
            let page = self.scan(agent, connection, params).await.unwrap();
            pages += 1;
            for k in page["keys"].as_array().unwrap() {
                keys.push(k["key"].as_str().unwrap().to_owned());
            }
            match page["nextCursor"].as_str() {
                Some(c) => cursor = Some(c.to_owned()),
                None => return (keys, pages),
            }
            assert!(pages < 10_000, "the scan does not end");
        }
    }
}

/// 입력 스키마에 명령 칸이 없다 — 두 툴의 입력 칸은 정확히 이것뿐이다(명령 · `argv` · `command` 같은 칸이 생기면 실패한다).
#[test]
fn the_input_schemas_have_no_command_field() {
    let fields = |schema: schemars::Schema| {
        let value = schema.to_value();
        let mut names: Vec<String> = value["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        fields(schemars::schema_for!(DbToolKvScanParams)),
        [
            "connectionId",
            "cursor",
            "database",
            "limit",
            "match",
            "projectId",
            "worktree"
        ]
    );
    assert_eq!(
        fields(schemars::schema_for!(DbToolKvGetParams)),
        [
            "connectionId",
            "cursor",
            "database",
            "key",
            "limit",
            "projectId",
            "worktree"
        ]
    );
}

/// 사람이 연 적 없는 Redis 연결은 `notOpenedByUser` 이고 서버에 붙지 않는다. 사람이 키 브라우저(세션)를 한 번 연 뒤에는 두 툴이
/// 되고, 툴은 호출마다 자기 연결을 연다(창의 세션을 빌리지 않는다 — 그 DB 번호로). 창의 범위로 부르면 `notAgentSession`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_connection_the_user_never_opened_is_refused() {
    let e = env(true);
    e.seed(3, "user:1", FakeKvValue::String(b"ada".to_vec()));
    let mut agent = e.agent();
    refused(
        e.scan(&mut agent, "cache", json!({})).await,
        "notOpenedByUser",
    );
    refused(
        e.get(&mut agent, "cache", json!({ "key": "user:1" })).await,
        "notOpenedByUser",
    );
    assert_eq!(e.connects(), 0, "{:?}", e.kv.calls());

    e.human_opens("cache", None).await;
    let before = e.connects();
    let page = e.scan(&mut agent, "cache", json!({})).await.unwrap();
    assert_eq!(page["keys"][0]["key"], "user:1");
    assert_eq!(page["keys"][0]["kind"], "string");
    assert!(page.get("nextCursor").is_none_or(Value::is_null), "{page}");
    let value = e
        .get(&mut agent, "cache", json!({ "key": "user:1" }))
        .await
        .unwrap();
    assert_eq!(value["value"]["string"]["text"], "ada", "{value}");
    assert_eq!(value["kind"], "string");
    assert_eq!(e.connects(), before + 2, "one connection per call");
    assert!(
        e.kv.calls()
            .iter()
            .filter(|c| matches!(c, FakeKvCall::Connect { .. }))
            .all(|c| *c == FakeKvCall::Connect { db: 3 })
    );

    // `database` 입력은 그 DB 번호의 연결 하나다(`SELECT` 를 보내지 않는다).
    e.scan(&mut agent, "cache", json!({ "database": "5" }))
        .await
        .unwrap();
    assert_eq!(
        e.kv.calls()
            .iter()
            .rfind(|c| matches!(c, FakeKvCall::Connect { .. })),
        Some(&FakeKvCall::Connect { db: 5 })
    );

    let mut w = e.f.window();
    refused(
        w.call("db.tool.kvScan", e.params("cache", json!({}))).await,
        "notAgentSession",
    );
}

/// 미신뢰 프로젝트는 `untrusted`, SQL 연결은 `unsupported` 이고 SQL 툴을 가리킨다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_projects_and_sql_connections_are_refused() {
    let e = env(false);
    let mut agent = e.agent();
    refused(e.scan(&mut agent, "cache", json!({})).await, "untrusted");
    assert_eq!(e.connects(), 0);

    let e = env(true);
    e.f.write_db(
        "pg.toml",
        "schema = 1\nname = \"pg\"\ndriver = \"postgres\"\nhost = \"localhost\"\ndatabase = \"app\"\n",
    );
    let mut w = e.f.window();
    w.call(
        "db.schema.objects",
        json!({ "projectId": e.f.id, "connectionId": "pg", "kind": "table", "limit": 10 }),
    )
    .await
    .unwrap();
    let mut agent = e.agent();
    let error = e.scan(&mut agent, "pg", json!({})).await.unwrap_err();
    assert_eq!(error.code, pw_db::gate::code::DB_ERROR);
    assert_eq!(error.data.unwrap()["error"]["kind"], "unsupported");
    assert!(error.message.contains("db_query"), "{}", error.message);
}

const PROD: &str = "schema = 1\nname = \"prod cache\"\ndriver = \"redis\"\nhost = \"cache.example\"\ndatabase = \"0\"\nproduction = true\n";

/// 프로덕션 연결은 사람이 열었어도 조회를 거절한다. 같은 서버의 다른 DB 번호를 가리키는 연결도(사람이 열었어도) 프로덕션이다 —
/// Redis 의 표지 열쇠는 DB 번호를 뺀다(메인 판정 2026-11-17).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_production_redis_server_is_refused_on_every_database_number() {
    let e = env(true);
    e.f.write_db("prod.toml", PROD);
    e.human_opens("prod", None).await;
    e.human_opens("cache", None).await;
    let before = e.connects();
    let mut agent = e.agent();
    refused(
        e.scan(&mut agent, "prod", json!({})).await,
        "productionBlocked",
    );
    refused(
        e.get(&mut agent, "prod", json!({ "key": "k" })).await,
        "productionBlocked",
    );
    // 같은 서버의 DB 3(표지 없는 연결) — 서버 단위 표지.
    refused(
        e.scan(&mut agent, "cache", json!({})).await,
        "productionBlocked",
    );
    assert_eq!(e.connects(), before, "nothing reached the server");
    let list = agent
        .call("db.tool.connectionList", json!({ "projectId": e.f.id }))
        .await
        .unwrap();
    let cache = list["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "cache")
        .unwrap();
    assert_eq!(cache["production"], true, "{list}");
}

/// M5-06 리뷰가 넘긴 우회 — DB 번호로 표지를 비껴가기(공격자 입장의 재현). 사람이 연 표지 없는 dev 연결(DB 1)로 같은 서버의
/// 프로덕션 DB 0 을 겨눈다: `database` 입력의 다른 표기(`"00"`), 그리고 프로덕션 설정이 번호를 뺀(= 0) 경우의 `"0"`. 표지 열쇠에
/// 번호가 남아 있으면 둘 다 프로덕션 데이터를 읽는다. `"+0"` 같은 숫자 밖의 꼴은 입력에서 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_spelling_of_a_production_database_number_does_not_lift_the_mark() {
    let dev = "schema = 1\nname = \"dev\"\ndriver = \"redis\"\nhost = \"cache.example\"\ndatabase = \"1\"\n";
    for (prod, aim) in [
        (PROD.to_owned(), "00"),
        (PROD.replace("database = \"0\"\n", ""), "0"),
    ] {
        let e = env(true);
        e.f.write_db("prod.toml", &prod);
        e.f.write_db("dev.toml", dev);
        e.seed(0, "secret", FakeKvValue::String(b"production".to_vec()));
        e.human_opens("dev", None).await;
        let before = e.connects();
        let mut agent = e.agent();
        refused(
            e.scan(&mut agent, "dev", json!({ "database": aim })).await,
            "productionBlocked",
        );
        refused(
            e.get(
                &mut agent,
                "dev",
                json!({ "database": aim, "key": "secret" }),
            )
            .await,
            "productionBlocked",
        );
        assert_eq!(e.connects(), before, "{aim}: nothing reached the server");
        let bad = e
            .scan(&mut agent, "dev", json!({ "database": "+0" }))
            .await
            .unwrap_err();
        assert_eq!(bad.code, rpc_code::INVALID_PARAMS);
    }
}

/// 표지 없는 새 연결 설정(다른 id · 같은 서버 · 다른 번호 · 호스트의 대소문자만 다른 표기)을 워크트리에 만들어도 거절된다 —
/// 메인 체크아웃의 설정이 그 서버를 프로덕션으로 적고 있어 프로덕션 표지가 먼저 걸리고(사람이 연 적도 없다), 목록은 그 연결을
/// 프로덕션으로 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_unmarked_connection_to_the_production_server_is_not_usable() {
    let e = env(true);
    e.f.write_db("prod.toml", PROD);
    let wt = fake_worktree(&e.f.root, &e.f.base.join("p1-wt"));
    std::fs::create_dir_all(wt.join(".pitwall/db")).unwrap();
    std::fs::write(
        wt.join(".pitwall/db/sneaky.toml"),
        "schema = 1\nname = \"sneaky\"\ndriver = \"redis\"\nhost = \"CACHE.example\"\nport = 6379\ndatabase = \"9\"\n",
    )
    .unwrap();
    let wt = wt.to_string_lossy().into_owned();
    let mut agent = e.agent();
    let r = e
        .scan(&mut agent, "sneaky", json!({ "worktree": wt }))
        .await;
    refused(r, "productionBlocked");
    let list = agent
        .call(
            "db.tool.connectionList",
            json!({ "projectId": e.f.id, "worktree": wt }),
        )
        .await
        .unwrap();
    let sneaky = list["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "sneaky")
        .unwrap();
    assert_eq!(sneaky["production"], true, "{list}");
    assert_eq!(e.connects(), 0);
}

/// 키 1,200개를 쪽으로 넘겨 빠짐없이 모은다(쪽 안에 중복 없음), `match` 로 거른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_kv_scan_pages_through_1200_keys_and_filters_by_match() {
    let e = env(true);
    for i in 0..1_200 {
        e.seed(
            3,
            &format!("user:{i:04}"),
            FakeKvValue::String(b"x".to_vec()),
        );
    }
    for i in 0..30 {
        e.seed(
            3,
            &format!("order:{i}"),
            FakeKvValue::List(vec![b"a".to_vec()]),
        );
    }
    e.human_opens("cache", None).await;
    let mut agent = e.agent();
    let (mut keys, pages) = e
        .scan_all(
            &mut agent,
            "cache",
            json!({ "match": "user:*", "limit": 500 }),
        )
        .await;
    assert!(pages >= 3, "{pages}");
    let n = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!((n, keys.len()), (1_200, 1_200));
    let (orders, _) = e
        .scan_all(&mut agent, "cache", json!({ "match": "order:*" }))
        .await;
    assert_eq!(orders.len(), 30);
    assert!(orders.iter().all(|k| k.starts_with("order:")));
    assert!(e.kv.calls().iter().any(|c| matches!(
        c,
        FakeKvCall::Scan { pattern: Some(p), .. } if p == "order:*"
    )));
    // 틀린 커서 · 범위 밖 limit 은 서버에 가기 전에 거절한다.
    for bad in [
        json!({ "cursor": "abc" }),
        json!({ "limit": 0 }),
        json!({ "limit": 501 }),
    ] {
        let error = e.scan(&mut agent, "cache", bad).await.unwrap_err();
        assert_eq!(error.code, rpc_code::INVALID_PARAMS);
    }
}

/// 응답 상한(192 KiB): 이름이 긴 키가 많아 한 쪽에 다 실을 수 없으면 쪽이 나뉘고, 그래도 빠지는 키가 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_page_stays_within_the_response_budget_without_losing_keys() {
    let e = env(true);
    let long = "k".repeat(4 * 1024);
    for i in 0..120 {
        e.seed(
            3,
            &format!("{long}{i:03}"),
            FakeKvValue::String(b"x".to_vec()),
        );
    }
    e.human_opens("cache", None).await;
    let mut agent = e.agent();
    let first = e
        .scan(&mut agent, "cache", json!({ "limit": 500 }))
        .await
        .unwrap();
    assert!(first.to_string().len() <= pw_db::script::RESPONSE_BUDGET);
    assert!(first["nextCursor"].is_string(), "the page was split");
    let (mut keys, pages) = e
        .scan_all(&mut agent, "cache", json!({ "limit": 500 }))
        .await;
    assert!(pages >= 3, "{pages}");
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 120);
}

/// 값 한 쪽: 타입별, 쪽 넘기기, 없는 키는 `missing`, 응답 상한을 넘는 쪽은 원소를 줄이고 원소 하나가 넘치면 그 값만 잘린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_kv_get_reads_pages_by_type_within_the_response_budget() {
    let e = env(true);
    let members: Vec<Vec<u8>> = (0..250).map(|i| format!("m{i:03}").into_bytes()).collect();
    e.seed(3, "list", FakeKvValue::List(members));
    e.seed(
        3,
        "fat",
        FakeKvValue::Hash(
            (0..10)
                .map(|i| (format!("f{i}").into_bytes(), vec![b'v'; 60 * 1024]))
                .collect(),
        ),
    );
    e.seed(3, "huge", FakeKvValue::List(vec![vec![b'z'; 600 * 1024]]));
    e.human_opens("cache", None).await;
    let mut agent = e.agent();

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut extra = json!({ "key": "list", "limit": 100 });
        if let Some(c) = &cursor {
            extra["cursor"] = json!(c);
        }
        let page = e.get(&mut agent, "cache", extra).await.unwrap();
        assert_eq!(
            (&page["kind"], &page["total"]),
            (&json!("list"), &json!(250))
        );
        for v in page["value"]["list"].as_array().unwrap() {
            seen.push(v["text"].as_str().unwrap().to_owned());
        }
        match page["nextCursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 250);
    assert_eq!(seen[249], "m249");

    let missing = e
        .get(&mut agent, "cache", json!({ "key": "nope" }))
        .await
        .unwrap();
    assert_eq!(missing["value"], "missing");
    assert!(missing["kind"].is_null());

    // 10 × 60 KiB 의 hash — 한 쪽에 다 실을 수 없어 원소 수가 줄고 값은 그대로다.
    let fat = e
        .get(&mut agent, "cache", json!({ "key": "fat", "limit": 100 }))
        .await
        .unwrap();
    assert!(fat.to_string().len() <= pw_db::script::RESPONSE_BUDGET);
    let fields = fat["value"]["hash"].as_array().unwrap();
    assert!(!fields.is_empty() && fields.len() < 10, "{}", fields.len());
    assert_eq!(fat["truncated"], false);
    assert!(fat["nextCursor"].is_string());
    assert_eq!(
        fields[0]["value"]["text"].as_str().unwrap().len(),
        60 * 1024
    );

    // 원소 하나가 600 KiB — 그 값만 앞부분으로 잘리고 표지가 남는다.
    let huge = e
        .get(&mut agent, "cache", json!({ "key": "huge" }))
        .await
        .unwrap();
    assert!(huge.to_string().len() <= pw_db::script::RESPONSE_BUDGET);
    assert_eq!(huge["truncated"], true);
    assert_eq!(
        huge["value"]["list"][0]["truncated"]["size"],
        600 * 1024,
        "{}",
        &huge.to_string()[..200]
    );

    // 틀린 hex 키는 서버에 가기 전에 거절한다.
    let error = e
        .get(&mut agent, "cache", json!({ "key": { "hex": "0xf" } }))
        .await
        .unwrap_err();
    assert_eq!(error.code, rpc_code::INVALID_PARAMS);
}

/// 분류기에 더한 `MEMORY USAGE <key>` 는 콘솔(`db.kv.command`)에서도 같은 판정이다 — 읽기로 서버에 가고, 다른 `MEMORY` 하위
/// 명령은 서버에 가기 전에 `blockedCommand` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_console_classifies_memory_usage_like_the_tool() {
    let e = env(true);
    let mut w = e.f.window();
    let opened = w
        .call(
            "db.session.open",
            json!({ "projectId": e.f.id, "connectionId": "cache" }),
        )
        .await
        .unwrap();
    let session = opened["session"]["sessionId"].as_str().unwrap().to_owned();
    w.call(
        "db.kv.command",
        json!({ "sessionId": session, "argv": ["MEMORY", "USAGE", "k"] }),
    )
    .await
    .unwrap();
    for argv in [
        json!(["MEMORY", "STATS"]),
        json!(["MEMORY", "USAGE", "k", "SAMPLES", "0"]),
        json!(["MEMORY", "DOCTOR"]),
    ] {
        let error = w
            .call(
                "db.kv.command",
                json!({ "sessionId": session, "argv": argv }),
            )
            .await
            .unwrap_err();
        refused(Err(error), "blockedCommand");
    }
    let commands = e.kv.commands();
    assert_eq!(
        commands,
        vec![vec![
            "MEMORY".to_owned(),
            "USAGE".to_owned(),
            "k".to_owned()
        ]]
    );
}

/// 앞 묶음의 마지막 키를 다음 묶음 앞에 다시 붙여 주는 KV 연결 — Redis `SCAN` 이 재해시 중에 같은 키를 두 번 주는 것을 흉내 낸다.
struct Repeating {
    inner: Box<dyn pw_db_driver::KvConnection>,
    last: Vec<pw_db_types::KvKeyInfo>,
}

impl pw_db_driver::KvConnection for Repeating {
    fn interrupter(&self) -> Arc<dyn pw_db_driver::Interrupt> {
        self.inner.interrupter()
    }

    fn scan(
        &mut self,
        request: pw_db_types::KvScanRequest,
    ) -> BoxFuture<'_, DbResult<pw_db_types::KvScanPage>> {
        Box::pin(async move {
            let mut page = self.inner.scan(request).await?;
            // 앞 묶음의 마지막 키 하나를 다시 준다(쪽 안에 들 만큼 작게 — 묶음 전체를 되풀이하면 `limit` 을 넘어 다음 쪽으로 넘어간다).
            let again = self.last.last().cloned();
            self.last = page.keys.clone();
            page.keys.splice(0..0, again);
            Ok(page)
        })
    }

    fn key_info(
        &mut self,
        key: pw_db_types::KvKey,
    ) -> BoxFuture<'_, DbResult<Option<pw_db_types::KvKeyInfo>>> {
        self.inner.key_info(key)
    }

    fn read(
        &mut self,
        key: pw_db_types::KvKey,
        range: pw_db_types::KvRange,
    ) -> BoxFuture<'_, DbResult<pw_db_types::KvValuePage>> {
        self.inner.read(key, range)
    }

    fn set_ttl(&mut self, key: pw_db_types::KvKey, ttl_ms: u64) -> BoxFuture<'_, DbResult<bool>> {
        self.inner.set_ttl(key, ttl_ms)
    }

    fn persist(&mut self, key: pw_db_types::KvKey) -> BoxFuture<'_, DbResult<bool>> {
        self.inner.persist(key)
    }

    fn delete(&mut self, keys: Vec<pw_db_types::KvKey>) -> BoxFuture<'_, DbResult<u64>> {
        self.inner.delete(keys)
    }

    fn rename(
        &mut self,
        from: pw_db_types::KvKey,
        to: pw_db_types::KvKey,
        overwrite: bool,
    ) -> BoxFuture<'_, DbResult<pw_db_types::KvRenameOutcome>> {
        self.inner.rename(from, to, overwrite)
    }

    fn command(&mut self, argv: Vec<String>) -> BoxFuture<'_, DbResult<pw_db_types::KvReply>> {
        self.inner.command(argv)
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, DbResult<()>> {
        self.inner.close()
    }
}

struct RepeatingRedis(FakeKvDriver);

impl DbDriver for RepeatingRedis {
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
        Box::pin(async move {
            let mut connected = self.0.connect(spec).await?;
            if let pw_db_driver::DbConnection::Kv(inner) = connected.connection {
                connected.connection = pw_db_driver::DbConnection::Kv(Box::new(Repeating {
                    inner,
                    last: Vec::new(),
                }));
            }
            Ok(connected)
        })
    }
}

/// `SCAN` 이 같은 키를 두 번 주어도(앞 묶음의 키가 다음 묶음에 다시 온다) 한 쪽 안에서는 한 번만 싣는다. 끝까지 모으면 키가 빠지지
/// 않는다(쪽 사이의 중복은 그대로 준다 — 툴 설명).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_repeats_are_dropped_within_a_page() {
    let kv = FakeKvDriver::new();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers
        .register(Arc::new(RepeatingRedis(kv.clone())))
        .unwrap();
    let f = Fixture::build(true, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    f.write_db("cache.toml", CACHE);
    for i in 0..300 {
        kv.insert(
            3,
            format!("k:{i:03}").as_bytes(),
            FakeKvValue::String(b"x".to_vec()),
        );
    }
    let e = Env { f, kv };
    e.human_opens("cache", None).await;
    let mut agent = e.agent();
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut extra = json!({ "limit": 100 });
        if let Some(c) = &cursor {
            extra["cursor"] = json!(c);
        }
        let page = e.scan(&mut agent, "cache", extra).await.unwrap();
        let mut keys: Vec<String> = page["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["key"].as_str().unwrap().to_owned())
            .collect();
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n, "a key twice within one page");
        all.extend(keys);
        match page["nextCursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 300);
}

/// 리뷰 R1: 프로덕션 표지가 사용자를 적지 않은 Redis 연결에 있으면, 같은 서버를 `user = ""` · `user = "default"` 로 가리키는
/// 사람이 연 연결도 프로덕션이다(셋은 같은 `default` 사용자다). 다른 사용자는 다른 대상이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_user_spellings_share_the_production_mark() {
    let base = "schema = 1\ndriver = \"redis\"\nhost = \"cache.example\"\ndatabase = \"1\"\n";
    for (marked, other) in [
        ("", "user = \"\"\n"),
        ("", "user = \"default\"\n"),
        ("user = \"default\"\n", ""),
        ("user = \"\"\n", "user = \"default\"\n"),
    ] {
        let e = env(true);
        e.f.write_db(
            "prod.toml",
            &format!("{base}name = \"prod\"\n{marked}production = true\n"),
        );
        e.f.write_db("dev.toml", &format!("{base}name = \"dev\"\n{other}"));
        e.f.write_db(
            "reader.toml",
            &format!("{base}name = \"reader\"\nuser = \"reader\"\n"),
        );
        e.human_opens("dev", None).await;
        e.human_opens("reader", None).await;
        let mut agent = e.agent();
        refused(
            e.scan(&mut agent, "dev", json!({})).await,
            "productionBlocked",
        );
        e.scan(&mut agent, "reader", json!({})).await.unwrap();
    }
}

/// 키 이름 하나가 응답 예산보다 크면(예산 계산이 불가능한 꼴) 그 쪽은 거절되지만 `data.nextCursor` 로 그 뒤 키를 이어 읽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_name_larger_than_the_response_is_skipped_by_the_cursor() {
    let e = env(true);
    e.seed(
        3,
        &format!("a{}", "k".repeat(200 * 1024)),
        FakeKvValue::String(b"x".to_vec()),
    );
    for i in 0..5 {
        e.seed(3, &format!("b{i}"), FakeKvValue::String(b"x".to_vec()));
    }
    e.human_opens("cache", None).await;
    let mut agent = e.agent();
    let error = e.scan(&mut agent, "cache", json!({})).await.unwrap_err();
    assert_eq!(error.code, rpc_code::INVALID_PARAMS);
    let data = error.data.unwrap();
    assert_eq!(data["reason"], "tooLarge");
    let cursor = data["nextCursor"]
        .as_str()
        .expect("a next cursor")
        .to_owned();
    let (rest, _) = {
        let mut keys = Vec::new();
        let mut cursor = Some(cursor);
        let mut pages = 0;
        while let Some(c) = cursor.take() {
            let page = e
                .scan(&mut agent, "cache", json!({ "cursor": c }))
                .await
                .unwrap();
            pages += 1;
            for k in page["keys"].as_array().unwrap() {
                keys.push(k["key"].as_str().unwrap().to_owned());
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
        }
        (keys, pages)
    };
    assert_eq!(rest, ["b0", "b1", "b2", "b3", "b4"]);
}
