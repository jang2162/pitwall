//! M3-22 의 실 Redis 시험(14-m3-plan.md M3-22 완료 기준 ① ~ ⑤): 실제 드라이버(`pw-db-driver-redis`)를 관문 뒤에 붙여 `db.kv.*`
//! 를 RPC 세션 너머에서 부른다. `PW_TEST_REDIS_URL` 이 있을 때만 목록에 있다(`pw_db_driver::live`).
//!
//! 「서버에 닿지 않았다」 는 서버에 물어 판정한다 — `INFO commandstats` 의 호출 수(다른 시험이 그 명령을 서버에 보내지 않는 것만:
//! `FLUSHALL` · `KEYS`), 그 밖은 키가 그대로인지(`EXISTS` · `PTTL`), 연결은 드라이버의 `connect` 수.
//!
//! 시험마다 DB 번호를 따로 쓴다(nextest 는 시험마다 프로세스를 띄워 함께 돌린다). M3-09 의 드라이버 시험이 0 ~ 10 을 쓰므로 여기는
//! 11(키 100만 개 — 이미 심겨 있으면 다시 심지 않는다) · 12 ~ 15 다. 13 은 두 시험이 함께 쓴다 — 둘 다 DB 를 지우지 않고 자기 키만
//! 쓴다.

#![cfg(pw_live_redis)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Conn, Fixture, StreamMsg};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::live::{LiveDb, endpoint};
use pw_db_driver::{BoxFuture, Connected, DbConnection, DbDriver, DbResult, KvConnection};
use pw_db_driver_redis::RedisDriver;
use pw_db_types::{
    Capabilities, ConnectionSpec, DbValue, DriverKind, KvReply, Secret, SqlDialect, SslMode,
    Target, TlsSettings,
};
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

/// 실제 Redis 드라이버. 연결을 몇 번 열었는지 센다(미신뢰 · 다른 창이 서버에 붙지 않았는지).
struct Counted {
    inner: RedisDriver,
    connects: Arc<AtomicUsize>,
}

impl DbDriver for Counted {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn dialect(&self) -> Option<SqlDialect> {
        None
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.inner.connect(spec)
    }
}

struct Env {
    f: Fixture,
    _sessions: Arc<DbSessions>,
    connects: Arc<AtomicUsize>,
    db: u32,
}

/// `PW_TEST_REDIS_URL` 의 서버, DB `db` 에 붙는 연결 설정 `name.toml`(평문). 비밀번호가 있으면 키체인(가짜)으로.
fn config(name: &str, db: u32, extra: &str) -> String {
    let ep = endpoint(LiveDb::Redis, &LiveDb::Redis.url()).unwrap();
    let password = if ep.password.is_some() {
        "password = { secret_ref = \"keychain\" }\n"
    } else {
        ""
    };
    format!(
        "schema = 1\nname = \"{name}\"\ndriver = \"redis\"\nhost = \"{}\"\nport = {}\ndatabase = \"{db}\"\nsslmode = \"disable\"\n{password}{extra}",
        ep.host, ep.port
    )
}

fn env(db: u32) -> Env {
    let connects = Arc::new(AtomicUsize::new(0));
    let mut drivers = pw_db::DriverRegistry::new();
    drivers
        .register(Arc::new(Counted {
            inner: RedisDriver::new(),
            connects: Arc::clone(&connects),
        }))
        .unwrap();
    let mut slot = None;
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    f.write_db("r.toml", &config("live redis", db, ""));
    f.write_db(
        "ro.toml",
        &config("live redis ro", db, "read_only = true\n"),
    );
    Env {
        f,
        _sessions: slot.unwrap(),
        connects,
        db,
    }
}

impl Env {
    /// 그 프로젝트 창에서 `connection` 의 세션을 연다(비밀번호가 있으면 먼저 넣는다).
    async fn open(&self, w: &mut Conn, connection: &str) -> String {
        if let Some(password) = endpoint(LiveDb::Redis, &LiveDb::Redis.url())
            .unwrap()
            .password
        {
            w.set_password(&self.f.id, connection, &password)
                .await
                .unwrap();
        }
        let opened = w
            .call(
                "db.session.open",
                json!({ "projectId": self.f.id, "connectionId": connection }),
            )
            .await
            .unwrap();
        assert_eq!(opened["driver"]["id"], "redis");
        opened["session"]["sessionId"].as_str().unwrap().to_owned()
    }

    /// 시험 준비 · 판정용 연결(관문을 거치지 않는다 — 분류도 없다).
    async fn admin(&self) -> Box<dyn KvConnection> {
        admin(self.db).await
    }
}

async fn admin(db: u32) -> Box<dyn KvConnection> {
    let ep = endpoint(LiveDb::Redis, &LiveDb::Redis.url()).unwrap();
    let mut spec = ConnectionSpec::new(
        DriverKind::Redis.id(),
        Target::Network {
            host: ep.host,
            port: ep.port,
            connect_to: None,
        },
    );
    spec.tls = TlsSettings::mode(SslMode::Disable);
    spec.database = Some(db.to_string());
    spec.password = ep.password.map(Secret::new);
    match RedisDriver::new().connect(spec).await.unwrap().connection {
        DbConnection::Kv(conn) => conn,
        DbConnection::Sql(_) => unreachable!("redis is a kv driver"),
    }
}

async fn run(conn: &mut Box<dyn KvConnection>, text: &str) -> KvReply {
    conn.command(text.split_whitespace().map(str::to_owned).collect())
        .await
        .unwrap()
}

fn int(reply: &KvReply) -> i64 {
    match reply {
        KvReply::Int(DbValue::Int(n)) => serde_json::to_value(n).unwrap().as_i64().unwrap(),
        other => panic!("not an integer reply: {other:?}"),
    }
}

/// 서버가 그 명령을 몇 번 돌렸는가(`INFO commandstats` 의 `cmdstat_<name>:calls=`). 서버 전체의 수다.
async fn calls_of(conn: &mut Box<dyn KvConnection>, name: &str) -> u64 {
    let KvReply::Bulk(DbValue::Text(info)) = run(conn, "INFO commandstats").await else {
        panic!("INFO is a bulk string")
    };
    let prefix = format!("cmdstat_{name}:calls=");
    info.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|rest| rest.split(',').next())
        .map_or(0, |n| n.parse().unwrap())
}

fn command(session: &str, text: &str) -> Value {
    json!({ "sessionId": session, "argv": text.split_whitespace().collect::<Vec<_>>() })
}

fn denied(e: &RpcError, reason: &str) -> bool {
    e.code == rpc_code::PERMISSION_DENIED && e.data.as_ref().is_some_and(|d| d["reason"] == reason)
}

/// ① `db.kv.command` 의 `FLUSHALL` · `KEYS *` 는 거절이고 서버에 닿지 않는다(`INFO commandstats` 의 호출 수가 그대로, 키도 그대로).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_flushall_and_keys_are_refused_before_the_server() {
    let e = env(12);
    let mut admin = e.admin().await;
    run(&mut admin, "FLUSHDB").await;
    run(&mut admin, "SET keep 1").await;
    let (flushall, keys) = (
        calls_of(&mut admin, "flushall").await,
        calls_of(&mut admin, "keys").await,
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    for text in ["FLUSHALL", "flushall ASYNC", "KEYS *", "keys session:*"] {
        let err = w
            .call("db.kv.command", command(&s, text))
            .await
            .unwrap_err();
        assert!(denied(&err, "blockedCommand"), "{text}: {err:?}");
    }
    assert_eq!(
        calls_of(&mut admin, "flushall").await,
        flushall,
        "FLUSHALL reached the server"
    );
    assert_eq!(
        calls_of(&mut admin, "keys").await,
        keys,
        "KEYS reached the server"
    );
    assert_eq!(int(&run(&mut admin, "EXISTS keep").await), 1);
    // 허용 명령은 서버에 닿는다(판정이 서버의 수를 읽는다는 것의 대조).
    let before = calls_of(&mut admin, "dbsize").await;
    let reply = w
        .call("db.kv.command", command(&s, "DBSIZE"))
        .await
        .unwrap();
    assert_eq!(reply["int"]["int"], 1, "{reply}");
    assert_eq!(calls_of(&mut admin, "dbsize").await, before + 1);
}

/// ② `DEL` 은 `confirmed` 없이 부르면 3001 이고 서버에 닿지 않으며(키가 그대로), 받은 해시로 다시 부르면 지운다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_del_needs_a_confirmation_then_deletes() {
    let e = env(13);
    let mut admin = e.admin().await;
    // 이름 바꾸기 시험과 같은 DB 라 지우지 않고 자기 키만 쓴다.
    run(&mut admin, "SET victim 1").await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let err = w
        .call("db.kv.command", command(&s, "DEL victim"))
        .await
        .unwrap_err();
    assert_eq!(err.code, code::CONFIRM_REQUIRED, "{err:?}");
    let data = err.data.unwrap();
    assert_eq!(data["grade"], "unboundedWrite");
    assert_eq!(
        int(&run(&mut admin, "EXISTS victim").await),
        1,
        "DEL reached the server"
    );
    let reply = w
        .call(
            "db.kv.command",
            json!({ "sessionId": s, "argv": ["DEL", "victim"],
                    "confirmed": { "grade": data["grade"], "statementsHash": data["statementsHash"] } }),
        )
        .await
        .unwrap();
    assert_eq!(reply["int"]["int"], 1, "{reply}");
    assert_eq!(int(&run(&mut admin, "EXISTS victim").await), 0);
}

/// ③ read-only 연결의 `SET` 은 거절이고 키가 생기지 않는다. 읽기는 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_a_read_only_connection_refuses_set() {
    let e = env(14);
    let mut admin = e.admin().await;
    run(&mut admin, "FLUSHDB").await;
    run(&mut admin, "SET present 1").await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "ro").await;
    for params in [
        command(&s, "SET created 1"),
        json!({ "sessionId": s, "argv": ["SET", "created", "1"],
                "confirmed": { "grade": "destructive", "statementsHash": "x" } }),
    ] {
        let err = w.call("db.kv.command", params).await.unwrap_err();
        assert!(denied(&err, "readOnlyConnection"), "{err:?}");
    }
    let err = w
        .call(
            "db.kv.setTtl",
            json!({ "sessionId": s, "key": "present", "ttlMs": 60000 }),
        )
        .await
        .unwrap_err();
    assert!(denied(&err, "readOnlyConnection"), "{err:?}");
    assert_eq!(
        int(&run(&mut admin, "EXISTS created").await),
        0,
        "SET reached the server"
    );
    assert_eq!(int(&run(&mut admin, "PTTL present").await), -1);
    let reply = w
        .call("db.kv.command", command(&s, "GET present"))
        .await
        .unwrap();
    assert_eq!(reply["bulk"]["text"], "1", "{reply}");
}

/// ④ 다른 프로젝트 창 · 웰컴 · 파일 창의 `db.kv.*` 는 `PERMISSION_DENIED` 이고 서버에 닿지 않는다(키 · TTL 이 그대로, 새 연결
/// 없음). 미신뢰 프로젝트는 `untrusted` 이고 연결을 열지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_other_windows_and_untrusted_projects_reach_no_server() {
    let e = env(15);
    let mut admin = e.admin().await;
    run(&mut admin, "FLUSHDB").await;
    run(&mut admin, "SET victim 1").await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let connects = e.connects.load(Ordering::SeqCst);
    let file = e.f.root.join("a.txt");
    std::fs::write(&file, "").unwrap();
    for scope in [
        ConnectionScope::Project(e.f.other.clone()),
        ConnectionScope::Host,
        pw_rpc::Caller::file(pw_rpc::Role::Owner, file.clone()).scope,
    ] {
        let mut other = e.f.connect(scope.clone());
        for (method, params) in [
            ("db.kv.command", command(&s, "DEL victim")),
            (
                "db.kv.delete",
                json!({ "sessionId": s, "keys": ["victim"] }),
            ),
            (
                "db.kv.setTtl",
                json!({ "sessionId": s, "key": "victim", "ttlMs": 1000 }),
            ),
            (
                "db.kv.rename",
                json!({ "sessionId": s, "from": "victim", "to": "moved" }),
            ),
            ("db.kv.get", json!({ "sessionId": s, "key": "victim" })),
            (
                "db.session.open",
                json!({ "projectId": e.f.id, "connectionId": "r" }),
            ),
        ] {
            let err = other.call(method, params).await.unwrap_err();
            assert_eq!(
                err.code,
                rpc_code::PERMISSION_DENIED,
                "{scope:?} {method}: {err:?}"
            );
        }
    }
    assert_eq!(
        e.connects.load(Ordering::SeqCst),
        connects,
        "another window connected"
    );
    assert_eq!(int(&run(&mut admin, "EXISTS victim").await), 1);
    assert_eq!(int(&run(&mut admin, "PTTL victim").await), -1);
    assert_eq!(int(&run(&mut admin, "EXISTS moved").await), 0);

    // 신뢰를 거두면 열린 세션도 · 새 세션도 막힌다.
    e.f.projects.set_trust(&e.f.id, false).unwrap();
    let err = w
        .call(
            "db.kv.command",
            json!({ "sessionId": s, "argv": ["SET", "victim", "2"] }),
        )
        .await
        .unwrap_err();
    assert!(denied(&err, "untrusted"), "{err:?}");
    let err = w
        .call(
            "db.session.open",
            json!({ "projectId": e.f.id, "connectionId": "r" }),
        )
        .await
        .unwrap_err();
    assert!(denied(&err, "untrusted"), "{err:?}");
    assert_eq!(
        e.connects.load(Ordering::SeqCst),
        connects,
        "an untrusted project connected"
    );
    let KvReply::Bulk(DbValue::Text(value)) = run(&mut admin, "GET victim").await else {
        panic!("string")
    };
    assert_eq!(value, "1");
}

/// ⑤ 키 100만 개에서 스캔 스트림의 첫 키 묶음(1,000키 + 타입 · TTL · 크기)이 M3-09 ① 의 시간(200 ms) 안 — 스트림을 여는 요청부터
/// 첫 `keys` 항목까지. 다섯 번 재고 가장 빠른 것으로 판정한다(M3-09 ① 과 같은 방법).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_the_first_scan_chunk_of_a_million_keys_is_fast() {
    const KEYS: i64 = 1_000_000;
    const BATCH: i64 = 10_000;
    let e = env(11);
    let mut admin = e.admin().await;
    if int(&run(&mut admin, "DBSIZE").await) != KEYS {
        run(&mut admin, "FLUSHDB").await;
        for chunk in 0..KEYS / BATCH {
            let mut argv = vec!["MSET".to_owned()];
            for i in chunk * BATCH..(chunk + 1) * BATCH {
                argv.push(format!("k:{i}"));
                argv.push(i.to_string());
            }
            admin.command(argv).await.unwrap();
        }
    }
    assert_eq!(int(&run(&mut admin, "DBSIZE").await), KEYS);

    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        let stream = w
            .start("db.kv.scan", json!({ "sessionId": s, "count": 1000 }))
            .await
            .unwrap();
        w.credit(stream, 2);
        let StreamMsg::Chunk(accepted) = w.next(stream).await else {
            panic!("accepted")
        };
        assert_eq!(accepted["kind"], "accepted");
        let StreamMsg::Chunk(first) = w.next(stream).await else {
            panic!("keys")
        };
        best = best.min(started.elapsed());
        assert_eq!(first["kind"], "keys");
        let keys = first["keys"].as_array().unwrap();
        assert!(keys.len() >= 1_000, "{}", keys.len());
        assert!(
            keys.iter()
                .all(|k| k["kind"] == "string" && k["size"].is_u64())
        );
        w.cancel_stream(stream);
    }
    println!("first db.kv.scan chunk of {KEYS} keys: best of 5 {best:?}");
    assert!(best < Duration::from_millis(200), "{best:?}");
}

/// 전용 메서드가 실 서버에서: 값 쪽 읽기, TTL 바꾸기 · 지우기, 덮지 않는 이름 바꾸기의 `targetExists`, 덮는 이름 바꾸기는 확인 뒤에만.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_the_key_methods_change_the_server_only_through_the_gate() {
    let e = env(13);
    let mut admin = e.admin().await;
    // ② 와 같은 DB 라 지우지 않고 자기 키만 쓴다.
    run(&mut admin, "DEL m:a m:b").await;
    run(&mut admin, "SET m:a alpha").await;
    run(&mut admin, "SET m:b beta").await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;

    let page = w
        .call("db.kv.get", json!({ "sessionId": s, "key": "m:a" }))
        .await
        .unwrap();
    assert_eq!(page["value"]["string"]["text"], "alpha", "{page}");
    let set = w
        .call(
            "db.kv.setTtl",
            json!({ "sessionId": s, "key": "m:a", "ttlMs": 600000 }),
        )
        .await
        .unwrap();
    assert_eq!(set, true);
    assert!(int(&run(&mut admin, "PTTL m:a").await) > 0);
    let persisted = w
        .call("db.kv.persist", json!({ "sessionId": s, "key": "m:a" }))
        .await
        .unwrap();
    assert_eq!(persisted, true);
    assert_eq!(int(&run(&mut admin, "PTTL m:a").await), -1);

    let outcome = w
        .call(
            "db.kv.rename",
            json!({ "sessionId": s, "from": "m:a", "to": "m:b" }),
        )
        .await
        .unwrap();
    assert_eq!(outcome, "targetExists");
    let err = w
        .call(
            "db.kv.rename",
            json!({ "sessionId": s, "from": "m:a", "to": "m:b", "overwrite": true }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, code::CONFIRM_REQUIRED, "{err:?}");
    let KvReply::Bulk(DbValue::Text(b)) = run(&mut admin, "GET m:b").await else {
        panic!("string")
    };
    assert_eq!(b, "beta", "an unconfirmed rename overwrote the target");
    let data = err.data.unwrap();
    let outcome = w
        .call(
            "db.kv.rename",
            json!({ "sessionId": s, "from": "m:a", "to": "m:b", "overwrite": true,
                    "confirmed": { "grade": data["grade"], "statementsHash": data["statementsHash"] } }),
        )
        .await
        .unwrap();
    assert_eq!(outcome, "renamed");
    let KvReply::Bulk(DbValue::Text(b)) = run(&mut admin, "GET m:b").await else {
        panic!("string")
    };
    assert_eq!(b, "alpha");
    assert_eq!(int(&run(&mut admin, "EXISTS m:a").await), 0);
}
