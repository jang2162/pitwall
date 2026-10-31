//! M3-22 의 관문 시험(14-m3-plan.md M3-22 행): 가짜 KV 드라이버(M3-09)로 `db.kv.*` 를 실제 RPC 세션 너머에서 본다. 「서버에 닿지
//! 않았다」 는 가짜 드라이버의 호출 기록(`FakeKvDriver::calls` · `commands`)으로 판정한다 — 가짜의 거절에 기대지 않는다.
//! 실 Redis 로 같은 완료 기준을 보는 시험은 `kv_live.rs`(`live_redis_*`).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Conn, Fixture, StreamMsg};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::fake::kv::{FakeKvCall, FakeKvDriver, FakeKvValue};
use pw_db_driver::{BoxFuture, Connected, DbDriver, DbResult};
use pw_db_types::{Capabilities, ConnectionSpec, KvKey, SqlDialect};
use pw_rpc::protocol::StreamOutcome;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

/// 가짜 KV 드라이버를 설정 파일의 `driver = "redis"` 자리에 끼운다.
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
    kv: FakeKvDriver,
}

const CACHE: &str =
    "schema = 1\nname = \"cache\"\ndriver = \"redis\"\nhost = \"localhost\"\ndatabase = \"3\"\n";

fn env(trusted: bool) -> Env {
    let kv = FakeKvDriver::new();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(AsRedis(kv.clone()))).unwrap();
    let mut slot = None;
    let f = Fixture::build(trusted, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    f.write_db("cache.toml", CACHE);
    f.write_db(
        "ro.toml",
        "schema = 1\nname = \"ro\"\ndriver = \"redis\"\nhost = \"localhost\"\ndatabase = \"3\"\nread_only = true\n",
    );
    f.write_db(
        "prod.toml",
        "schema = 1\nname = \"prod cache\"\ndriver = \"redis\"\nhost = \"localhost\"\ndatabase = \"3\"\nproduction = true\n",
    );
    Env {
        f,
        sessions: slot.unwrap(),
        kv,
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

    fn seed(&self, key: &str, value: &str) {
        self.kv.insert(
            3,
            key.as_bytes(),
            FakeKvValue::String(value.as_bytes().to_vec()),
        );
    }

    /// 연결 · 닫기를 뺀 드라이버 호출(관문을 지나 「서버에 닿은」 것).
    fn reached(&self) -> Vec<FakeKvCall> {
        self.kv
            .calls()
            .into_iter()
            .filter(|c| !matches!(c, FakeKvCall::Connect { .. } | FakeKvCall::Close))
            .collect()
    }
}

fn argv(text: &str) -> Value {
    json!(text.split_whitespace().collect::<Vec<_>>())
}

fn command(session: &str, text: &str) -> Value {
    json!({ "sessionId": session, "argv": argv(text) })
}

fn denied(e: &RpcError, reason: &str) -> bool {
    e.code == rpc_code::PERMISSION_DENIED && e.data.as_ref().is_some_and(|d| d["reason"] == reason)
}

/// 다른 창의 거절: 프로젝트 · Host 범위는 관문의 `notProjectWindow`, 파일 창은 라우터가 먼저 막는다(파일 범위 메서드가 아니다).
fn refused(scope: &ConnectionScope, e: &RpcError) -> bool {
    match scope {
        ConnectionScope::File(_) => e.code == rpc_code::PERMISSION_DENIED,
        _ => denied(e, "notProjectWindow"),
    }
}

fn confirm_data(e: &RpcError) -> Value {
    assert_eq!(e.code, code::CONFIRM_REQUIRED, "{e:?}");
    e.data.clone().unwrap()
}

/// 받은 확인 요청 그대로의 `confirmed`.
fn confirmed_from(data: &Value) -> Value {
    json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] })
}

async fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// 완료 기준 ①: `FLUSHALL` · `KEYS *` 와 연결을 붙잡거나 상태를 바꾸는 명령, 큰 `COUNT` 의 스캔은 거절이고 드라이버에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_commands_are_refused_before_the_driver() {
    let e = env(true);
    e.seed("a", "1");
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    for (text, block) in [
        ("FLUSHALL", "notAllowed"),
        ("flushdb", "notAllowed"),
        ("KEYS *", "notAllowed"),
        ("SELECT 2", "notAllowed"),
        ("CONFIG SET maxmemory 1", "notAllowed"),
        ("MONITOR", "notAllowed"),
        ("BLPOP q 0", "blocking"),
        ("XREAD BLOCK 0 STREAMS s $", "blocking"),
        ("SCAN 0 COUNT 20000", "scanCount"),
        ("PITWALL.NOSUCH x", "notAllowed"),
    ] {
        let err = w
            .call("db.kv.command", command(&s, text))
            .await
            .unwrap_err();
        assert!(denied(&err, "blockedCommand"), "{text}: {err:?}");
        assert_eq!(err.data.as_ref().unwrap()["block"], block, "{text}");
    }
    // 확인을 실어도 거절이다.
    let err = w
        .call(
            "db.kv.command",
            json!({ "sessionId": s, "argv": ["FLUSHALL"],
                    "confirmed": { "grade": "destructive", "statementsHash": "x" } }),
        )
        .await
        .unwrap_err();
    assert!(denied(&err, "blockedCommand"), "{err:?}");
    assert_eq!(e.reached(), vec![], "a refused command reached the driver");
    assert!(e.kv.get(3, b"a").is_some());
    // 허용 명령은 돈다.
    let reply = w.call("db.kv.command", command(&s, "GET a")).await.unwrap();
    assert_eq!(reply["bulk"]["text"], "1", "{reply}");
    assert_eq!(
        e.kv.commands(),
        vec![vec!["GET".to_owned(), "a".to_owned()]]
    );
}

/// 완료 기준 ②: `DEL` 은 확인 없이 3001 이고 드라이버에 닿지 않는다. 받은 해시로 다시 부르면 지운다. 해시는 그 명령 · 그 세션에만
/// 맞고 한 번만 쓰인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn del_needs_a_confirmation_bound_to_the_command() {
    let e = env(true);
    e.seed("a", "1");
    e.seed("b", "2");
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    let other = e.open(&mut w, "cache").await;

    let err = w
        .call("db.kv.command", command(&s, "DEL a"))
        .await
        .unwrap_err();
    let data = confirm_data(&err);
    assert_eq!(data["grade"], "unboundedWrite");
    assert_eq!(data["action"], "command");
    assert_eq!(data["argv"], json!(["DEL", "a"]));
    assert_eq!(data["rejected"], Value::Null);
    assert_eq!(e.reached(), vec![], "DEL reached the driver unconfirmed");
    let confirmed = confirmed_from(&data);

    // 다른 키 · 다른 세션 · 낮은 등급에는 맞지 않는다.
    for (session, text, grade, why) in [
        (&s, "DEL b", "unboundedWrite", "hashMismatch"),
        (&other, "DEL a", "unboundedWrite", "hashMismatch"),
        (&s, "DEL a", "write", "gradeTooLow"),
    ] {
        let mut c = confirmed.clone();
        c["grade"] = json!(grade);
        let err = w
            .call(
                "db.kv.command",
                json!({ "sessionId": session, "argv": argv(text), "confirmed": c }),
            )
            .await
            .unwrap_err();
        assert_eq!(confirm_data(&err)["rejected"], why, "{text} on {session}");
    }
    assert_eq!(e.reached(), vec![]);

    // 받은 해시 그대로면 지운다.
    let reply = w
        .call(
            "db.kv.command",
            json!({ "sessionId": s, "argv": ["DEL", "a"], "confirmed": confirmed }),
        )
        .await
        .unwrap();
    assert_eq!(reply["int"]["int"], 1, "{reply}");
    assert!(e.kv.get(3, b"a").is_none());
    assert!(e.kv.get(3, b"b").is_some());
    // 같은 확인을 다시 보내면 쓰지 않는다(한 번만 쓰인다).
    e.seed("a", "again");
    let err = w
        .call(
            "db.kv.command",
            json!({ "sessionId": s, "argv": ["DEL", "a"], "confirmed": confirmed }),
        )
        .await
        .unwrap_err();
    assert_eq!(confirm_data(&err)["rejected"], "hashMismatch");
    assert!(e.kv.get(3, b"a").is_some());
    assert_eq!(e.kv.commands().len(), 1);
}

/// 읽기는 확인을 쓰지 않는다 — 확인 다이얼로그가 떠 있는 동안 같은 세션의 키 트리가 읽어도 받은 확인이 그대로 맞는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_between_do_not_spend_a_confirmation() {
    let e = env(true);
    e.seed("a", "1");
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    let err = w
        .call("db.kv.delete", json!({ "sessionId": s, "keys": ["a"] }))
        .await
        .unwrap_err();
    let confirmed = confirmed_from(&confirm_data(&err));
    w.call("db.kv.get", json!({ "sessionId": s, "key": "a" }))
        .await
        .unwrap();
    w.call("db.kv.command", command(&s, "GET a")).await.unwrap();
    let deleted = w
        .call(
            "db.kv.delete",
            json!({ "sessionId": s, "keys": ["a"], "confirmed": confirmed }),
        )
        .await
        .unwrap();
    assert_eq!(deleted["deleted"], 1);
}

/// `db.kv.delete` 와 덮는 `db.kv.rename` 은 확인이 필요하다. 덮지 않는 이름 바꾸기는 대상이 있으면 바꾸지 않고 `targetExists`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_and_overwriting_rename_need_a_confirmation() {
    let e = env(true);
    e.seed("a", "1");
    e.seed("b", "2");
    e.kv.insert(3, &[0xff, 0x01], FakeKvValue::String(b"bin".to_vec()));
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;

    let hex = json!({ "hex": "0xff01" });
    let err = w
        .call(
            "db.kv.delete",
            json!({ "sessionId": s, "keys": ["a", hex] }),
        )
        .await
        .unwrap_err();
    let data = confirm_data(&err);
    assert_eq!(data["action"], "delete");
    assert_eq!(data["argv"], json!(["DEL", "a", "0xff01"]));
    assert_eq!(e.reached(), vec![]);
    let deleted = w
        .call(
            "db.kv.delete",
            json!({ "sessionId": s, "keys": ["a", hex], "confirmed": confirmed_from(&data) }),
        )
        .await
        .unwrap();
    assert_eq!(deleted["deleted"], 2);
    assert!(e.kv.get(3, &[0xff, 0x01]).is_none());

    // 덮지 않는 이름 바꾸기: 쓰기 — 확인 없이 돌고, 대상이 있으면 그대로 둔다.
    e.seed("c", "3");
    let outcome = w
        .call(
            "db.kv.rename",
            json!({ "sessionId": s, "from": "c", "to": "b" }),
        )
        .await
        .unwrap();
    assert_eq!(outcome, "targetExists");
    // 덮는 이름 바꾸기: 확인 전에는 드라이버에 닿지 않는다.
    let before = e.reached().len();
    let err = w
        .call(
            "db.kv.rename",
            json!({ "sessionId": s, "from": "c", "to": "b", "overwrite": true }),
        )
        .await
        .unwrap_err();
    let data = confirm_data(&err);
    assert_eq!(data["action"], "rename");
    assert_eq!(data["argv"], json!(["RENAME", "c", "b"]));
    assert_eq!(e.reached().len(), before);
    // 덮지 않는 것에 받은 확인은 덮는 것에 맞지 않는다(같은 키라도).
    let outcome = w
        .call(
            "db.kv.rename",
            json!({ "sessionId": s, "from": "c", "to": "b", "overwrite": true,
                    "confirmed": confirmed_from(&data) }),
        )
        .await
        .unwrap();
    assert_eq!(outcome, "renamed");
    assert_eq!(e.kv.get(3, b"b"), Some(FakeKvValue::String(b"3".to_vec())));

    // 틀린 hex 는 연결에 닿기 전에 거절이다.
    let err = w
        .call(
            "db.kv.delete",
            json!({ "sessionId": s, "keys": [{ "hex": "0xf" }] }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc_code::INVALID_PARAMS);
}

/// 완료 기준 ③: read-only 연결은 읽기만 — `SET` 과 전용 쓰기 메서드는 드라이버에 닿기 전에 거절이다(확인을 실어도).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_connection_runs_reads_only() {
    let e = env(true);
    e.seed("a", "1");
    let mut w = e.f.window();
    let s = e.open(&mut w, "ro").await;
    let confirmed = json!({ "grade": "destructive", "statementsHash": "x" });
    for (method, params) in [
        ("db.kv.command", command(&s, "SET a 2")),
        ("db.kv.command", command(&s, "DEL a")),
        (
            "db.kv.command",
            json!({ "sessionId": s, "argv": ["SET", "a", "2"], "confirmed": confirmed }),
        ),
        (
            "db.kv.setTtl",
            json!({ "sessionId": s, "key": "a", "ttlMs": 1000 }),
        ),
        ("db.kv.persist", json!({ "sessionId": s, "key": "a" })),
        (
            "db.kv.delete",
            json!({ "sessionId": s, "keys": ["a"], "confirmed": confirmed }),
        ),
        (
            "db.kv.rename",
            json!({ "sessionId": s, "from": "a", "to": "b" }),
        ),
    ] {
        let err = w.call(method, params).await.unwrap_err();
        assert!(denied(&err, "readOnlyConnection"), "{method}: {err:?}");
    }
    assert_eq!(e.reached(), vec![], "a write reached the driver");
    assert_eq!(e.kv.get(3, b"a"), Some(FakeKvValue::String(b"1".to_vec())));
    // 읽기는 돈다.
    w.call("db.kv.command", command(&s, "GET a")).await.unwrap();
    let page = w
        .call("db.kv.get", json!({ "sessionId": s, "key": "a" }))
        .await
        .unwrap();
    assert_eq!(page["value"]["string"]["text"], "1", "{page}");
}

/// 프로덕션 연결은 조회 밖을 모두 확인한다(결정 M3-D20) — 확인이 필요 없는 쓰기(`SET` · TTL)도.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_production_connection_confirms_every_write() {
    let e = env(true);
    e.seed("a", "1");
    let mut w = e.f.window();
    let s = e.open(&mut w, "prod").await;
    let err = w
        .call("db.kv.command", command(&s, "SET a 2"))
        .await
        .unwrap_err();
    let data = confirm_data(&err);
    assert_eq!(data["grade"], "write");
    assert_eq!(data["nameRequired"], false);
    let err = w
        .call(
            "db.kv.setTtl",
            json!({ "sessionId": s, "key": "a", "ttlMs": 5000 }),
        )
        .await
        .unwrap_err();
    let ttl = confirm_data(&err);
    assert_eq!(ttl["action"], "setTtl");
    assert_eq!(e.reached(), vec![]);
    w.call(
        "db.kv.setTtl",
        json!({ "sessionId": s, "key": "a", "ttlMs": 5000, "confirmed": confirmed_from(&ttl) }),
    )
    .await
    .unwrap();
    assert_eq!(
        e.reached(),
        vec![FakeKvCall::SetTtl(KvKey::Text("a".into()), 5000)]
    );
    // 읽기는 그대로.
    w.call("db.kv.command", command(&s, "GET a")).await.unwrap();
}

/// 자동 커밋 연결의 쓰기는 확인 없이 돈다. TTL 은 1 이상이어야 한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_writes_run_without_a_confirmation() {
    let e = env(true);
    e.seed("a", "1");
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    w.call("db.kv.command", command(&s, "SET a 2"))
        .await
        .unwrap();
    assert_eq!(e.kv.get(3, b"a"), Some(FakeKvValue::String(b"2".to_vec())));
    let set = w
        .call(
            "db.kv.setTtl",
            json!({ "sessionId": s, "key": "a", "ttlMs": 9000 }),
        )
        .await
        .unwrap();
    assert_eq!(set, true);
    let dropped = w
        .call("db.kv.persist", json!({ "sessionId": s, "key": "a" }))
        .await
        .unwrap();
    assert_eq!(dropped, true);
    let err = w
        .call(
            "db.kv.setTtl",
            json!({ "sessionId": s, "key": "a", "ttlMs": 0 }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc_code::INVALID_PARAMS);
}

/// 완료 기준 ④: 다른 프로젝트 창 · 웰컴(Host 범위) · 파일 창은 세션 id 를 알아도 `db.kv.*` 를 쓸 수 없고 드라이버에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_windows_cannot_use_a_kv_session() {
    let e = env(true);
    e.seed("a", "1");
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    let file = e.f.root.join("a.txt");
    std::fs::write(&file, "").unwrap();
    for scope in [
        ConnectionScope::Project(e.f.other.clone()),
        ConnectionScope::Host,
        pw_rpc::Caller::file(pw_rpc::Role::Owner, file.clone()).scope,
    ] {
        let mut other = e.f.connect(scope.clone());
        for (method, params) in [
            ("db.kv.get", json!({ "sessionId": s, "key": "a" })),
            ("db.kv.command", command(&s, "GET a")),
            (
                "db.kv.setTtl",
                json!({ "sessionId": s, "key": "a", "ttlMs": 1 }),
            ),
            ("db.kv.persist", json!({ "sessionId": s, "key": "a" })),
            ("db.kv.delete", json!({ "sessionId": s, "keys": ["a"] })),
            (
                "db.kv.rename",
                json!({ "sessionId": s, "from": "a", "to": "b" }),
            ),
            (
                "db.session.open",
                json!({ "projectId": e.f.id, "connectionId": "cache" }),
            ),
        ] {
            let err = other.call(method, params).await.unwrap_err();
            assert!(refused(&scope, &err), "{scope:?} {method}: {err:?}");
        }
        let err = match other.start("db.kv.scan", json!({ "sessionId": s })).await {
            Err(err) => err,
            Ok(stream) => match other.collect(stream).await.1 {
                StreamOutcome::Failed { error } => error,
                outcome => panic!("{outcome:?}"),
            },
        };
        assert!(refused(&scope, &err), "{scope:?} scan: {err:?}");
    }
    assert_eq!(e.reached(), vec![], "another window reached the driver");
}

/// 완료 기준 ④: 미신뢰 프로젝트는 KV 세션을 열지 못하고(`untrusted`, 연결하지 않는다), 신뢰를 거두면 열린 세션의 `db.kv.*` 도
/// 드라이버에 닿기 전에 `untrusted` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_reaches_no_server() {
    let e = env(false);
    let mut w = e.f.window();
    let err = w
        .call(
            "db.session.open",
            json!({ "projectId": e.f.id, "connectionId": "cache" }),
        )
        .await
        .unwrap_err();
    assert!(denied(&err, "untrusted"), "{err:?}");
    assert_eq!(e.kv.calls(), vec![], "an untrusted project connected");

    e.f.projects.set_trust(&e.f.id, true).unwrap();
    e.seed("a", "1");
    let s = e.open(&mut w, "cache").await;
    e.f.projects.set_trust(&e.f.id, false).unwrap();
    for (method, params) in [
        ("db.kv.get", json!({ "sessionId": s, "key": "a" })),
        ("db.kv.command", command(&s, "GET a")),
        ("db.kv.delete", json!({ "sessionId": s, "keys": ["a"] })),
    ] {
        let err = w.call(method, params).await.unwrap_err();
        assert!(denied(&err, "untrusted"), "{method}: {err:?}");
    }
    let stream = w.start("db.kv.scan", json!({ "sessionId": s })).await;
    let err = match stream {
        Err(err) => err,
        Ok(stream) => match w.collect(stream).await.1 {
            StreamOutcome::Failed { error } => error,
            outcome => panic!("{outcome:?}"),
        },
    };
    assert!(denied(&err, "untrusted"), "scan: {err:?}");
    assert_eq!(e.reached(), vec![]);
}

/// 연결 설정이 바뀐 세션은 `configChanged` 로 거절한다 — 저절로 다시 붙지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_config_stops_the_session() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    e.f.edit_db("cache.toml", "database = \"3\"", "database = \"4\"");
    let err = w
        .call("db.kv.command", command(&s, "PING"))
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc_code::CONFLICT);
    assert_eq!(err.data.unwrap()["reason"], "configChanged");
    assert_eq!(e.reached(), vec![]);
}

/// 스캔 스트림: `accepted` → 묶음마다 `keys` → `done`, 패턴을 드라이버에 넘긴다. `count` 는 1 ~ 10,000.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_scan_streams_batches_until_the_keyspace_ends() {
    let e = env(true);
    for i in 0..2_500 {
        e.seed(&format!("session:{i}"), "x");
    }
    e.seed("other", "y");
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    let stream = w
        .start(
            "db.kv.scan",
            json!({ "sessionId": s, "pattern": "session:*", "count": 1000 }),
        )
        .await
        .unwrap();
    let (items, outcome) = w.collect(stream).await;
    assert!(matches!(outcome, StreamOutcome::Completed), "{outcome:?}");
    assert_eq!(items[0]["kind"], "accepted");
    assert!(items[0]["executionId"].as_str().is_some());
    let batches: Vec<&Value> = items.iter().filter(|i| i["kind"] == "keys").collect();
    let keys: usize = batches
        .iter()
        .map(|b| b["keys"].as_array().unwrap().len())
        .sum();
    assert_eq!(keys, 2_500);
    assert!(batches.len() >= 3, "{}", batches.len());
    let done = items.last().unwrap();
    assert_eq!(done["kind"], "done");
    assert_eq!(done["keys"], 2_500);
    assert!(e.kv.calls().iter().any(|c| matches!(
        c,
        FakeKvCall::Scan { pattern: Some(p), .. } if p == "session:*"
    )));
    for count in [0, 10_001] {
        let err = w
            .start("db.kv.scan", json!({ "sessionId": s, "count": count }))
            .await;
        let err = match err {
            Err(err) => err,
            Ok(stream) => match w.collect(stream).await.1 {
                StreamOutcome::Failed { error } => error,
                outcome => panic!("{outcome:?}"),
            },
        };
        assert_eq!(err.code, rpc_code::INVALID_PARAMS, "count {count}");
    }
}

/// 스캔은 `db.query.cancel { executionId }` 로 멈춘다(credit 을 기다리는 중에도). 다른 프로젝트 창 · 웰컴은 그 실행을 끌 수 없다
/// (M3-D18). 멈춘 뒤 같은 세션은 그대로 쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_stops_on_db_query_cancel_from_its_own_window_only() {
    let e = env(true);
    for i in 0..3_000 {
        e.seed(&format!("k{i}"), "x");
    }
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    let stream = w
        .start("db.kv.scan", json!({ "sessionId": s, "count": 100 }))
        .await
        .unwrap();
    w.credit(stream, 2);
    let StreamMsg::Chunk(accepted) = w.next(stream).await else {
        panic!("accepted")
    };
    let execution = accepted["executionId"].as_str().unwrap().to_owned();
    let StreamMsg::Chunk(first) = w.next(stream).await else {
        panic!("keys")
    };
    assert_eq!(first["kind"], "keys");

    for scope in [
        ConnectionScope::Project(e.f.other.clone()),
        ConnectionScope::Host,
    ] {
        let mut other = e.f.connect(scope.clone());
        let err = other
            .call("db.query.cancel", json!({ "executionId": execution }))
            .await
            .unwrap_err();
        assert!(denied(&err, "notProjectWindow"), "{scope:?}: {err:?}");
    }
    // 그 스캔은 그대로 이어진다.
    w.credit(stream, 1);
    let StreamMsg::Chunk(next) = w.next(stream).await else {
        panic!("keys after a refused cancel")
    };
    assert_eq!(next["kind"], "keys");
    assert!(!e.kv.calls().contains(&FakeKvCall::Interrupt));

    // 그 창의 취소: credit 을 기다리던 스트림이 곧바로 끝난다.
    let started = Instant::now();
    w.call("db.query.cancel", json!({ "executionId": execution }))
        .await
        .unwrap();
    let outcome = loop {
        match w.next(stream).await {
            StreamMsg::Chunk(_) => {}
            StreamMsg::End(outcome) => break outcome,
        }
    };
    assert!(started.elapsed() < Duration::from_secs(1));
    match outcome {
        StreamOutcome::Failed { error } => assert_eq!(error.code, rpc_code::CANCELLED),
        outcome => panic!("{outcome:?}"),
    }
    assert!(
        e.kv.calls().contains(&FakeKvCall::Interrupt),
        "no driver interrupt"
    );
    let scans =
        e.kv.calls()
            .iter()
            .filter(|c| matches!(c, FakeKvCall::Scan { .. }))
            .count();
    assert!(
        scans < 30,
        "the scan went on after the cancel: {scans} batches"
    );
    // 세션은 산다.
    w.call("db.kv.command", command(&s, "PING")).await.unwrap();
}

/// 단항 요청은 요청 취소(`request.cancel`)가 드라이버를 인터럽트하고, 같은 세션의 다음 요청이 곧바로 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_cancel_interrupts_a_hanging_command() {
    let e = env(true);
    e.kv.hang_on("GET");
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    w.session.handle_value(json!({
        "jsonrpc": "2.0", "id": 900, "method": "db.kv.command", "params": command(&s, "GET a"),
    }));
    wait_until("the GET to reach the driver", || {
        e.kv.commands().iter().any(|c| c[0] == "GET")
    })
    .await;
    let started = Instant::now();
    w.session.handle_value(json!({
        "jsonrpc": "2.0", "method": "request.cancel", "params": { "id": 900 },
    }));
    wait_until("the driver interrupt", || {
        e.kv.calls().contains(&FakeKvCall::Interrupt)
    })
    .await;
    // 연결 잠금이 풀려 다음 요청이 곧바로 돈다.
    let pong = w.call("db.kv.command", command(&s, "PING")).await.unwrap();
    assert_eq!(pong["status"], "PONG");
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// 창이 닫히면 그 창의 KV 세션은 닫히고, 도는 요청은 응답 타임아웃을 기다리지 않고 곧바로 인터럽트된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_window_interrupts_a_hanging_command_at_once() {
    let e = env(true);
    e.kv.hang_on("GET");
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    w.session.handle_value(json!({
        "jsonrpc": "2.0", "id": 901, "method": "db.kv.command", "params": command(&s, "GET a"),
    }));
    wait_until("the GET to reach the driver", || {
        e.kv.commands().iter().any(|c| c[0] == "GET")
    })
    .await;
    assert_eq!(e.sessions.session_count(), 1);
    let started = Instant::now();
    drop(w);
    wait_until("the session to close", || e.sessions.session_count() == 0).await;
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(e.kv.calls().contains(&FakeKvCall::Interrupt));
    wait_until("the connection to close", || {
        e.kv.calls().contains(&FakeKvCall::Close)
    })
    .await;
}

/// 창이 닫히면 credit 을 기다리던 스캔도 끝나고 세션이 닫힌다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_window_ends_a_waiting_scan() {
    let e = env(true);
    for i in 0..500 {
        e.seed(&format!("k{i}"), "x");
    }
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    let stream = w
        .start("db.kv.scan", json!({ "sessionId": s, "count": 10 }))
        .await
        .unwrap();
    w.credit(stream, 2);
    let _ = w.next(stream).await;
    let _ = w.next(stream).await;
    drop(w);
    wait_until("the session to close", || e.sessions.session_count() == 0).await;
    let scans =
        e.kv.calls()
            .iter()
            .filter(|c| matches!(c, FakeKvCall::Scan { .. }))
            .count();
    assert!(
        scans < 10,
        "the scan went on after the window closed: {scans}"
    );
}

/// 값 읽기: 쪽 단위(`range`), 키가 없으면 `missing`. SQL 갈래 세션에는 쓰지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_reads_a_page_of_a_value() {
    let e = env(true);
    e.kv.insert(
        3,
        b"h",
        FakeKvValue::List((0..10).map(|i| format!("v{i}").into_bytes()).collect()),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "cache").await;
    let page = w
        .call(
            "db.kv.get",
            json!({ "sessionId": s, "key": "h", "range": { "count": 4 } }),
        )
        .await
        .unwrap();
    assert_eq!(page["value"]["list"].as_array().unwrap().len(), 4);
    assert_eq!(page["total"], 10);
    let next = page["nextCursor"].clone();
    let page = w
        .call(
            "db.kv.get",
            json!({ "sessionId": s, "key": "h", "range": { "cursor": next, "count": 100 } }),
        )
        .await
        .unwrap();
    assert_eq!(page["value"]["list"].as_array().unwrap().len(), 6);
    let missing = w
        .call("db.kv.get", json!({ "sessionId": s, "key": "nope" }))
        .await
        .unwrap();
    assert_eq!(missing["value"], "missing");
    let err = w
        .call(
            "db.kv.get",
            json!({ "sessionId": s, "key": "h", "range": { "count": 0 } }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc_code::INVALID_PARAMS);
}
