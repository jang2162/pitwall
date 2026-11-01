//! M3-22 의 실 Redis 시험(14-m3-plan.md M3-22 완료 기준 ① ~ ⑤): 실제 드라이버(`pw-db-driver-redis`)를 관문 뒤에 붙여 `db.kv.*`
//! 를 RPC 세션 너머에서 부른다. `PW_TEST_REDIS_URL` 이 있을 때만 목록에 있다(`pw_db_driver::live`).
//!
//! 「서버에 닿지 않았다」 는 서버에 물어 판정한다 — `INFO commandstats` 의 호출 수(다른 시험이 그 명령을 서버에 보내지 않는 것만 —
//! CONTRIBUTING.md 「명령」 의 실 DB 시험 단락), 그 밖은 키가 그대로인지(`EXISTS` · `PTTL`), 연결은 드라이버의 `connect` 수.
//!
//! 시험마다 DB 번호를 따로 쓴다(nextest 는 시험마다 프로세스를 띄워 함께 돌린다). M3-09 의 드라이버 시험이 0 ~ 10 을 쓰므로 여기는
//! 11(키 100만 개 — 이미 심겨 있으면 다시 심지 않는다) · 12 ~ 15 다. 13 은 여러 시험이 함께 쓴다 — 모두 DB 를 지우지 않고 자기 접두사의
//! 키만 쓴다.

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
use pw_db_driver_redis::{DEFAULT_RESPONSE_TIMEOUT, RedisDriver};
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

/// 전용 메서드가 실 서버에서: 값 쪽 읽기, TTL 바꾸기(`PEXPIRE` 의 확인 뒤) · 지우기, 덮지 않는 이름 바꾸기(`RENAMENX` 의 확인 뒤)의
/// `targetExists`, 덮는 이름 바꾸기는 확인 뒤에만.
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
    let ttl = json!({ "sessionId": s, "key": "m:a", "ttlMs": 600000 });
    let set = call_confirmed(&mut w, "db.kv.setTtl", ttl).await;
    assert_eq!(set, true);
    assert!(int(&run(&mut admin, "PTTL m:a").await) > 0);
    let persisted = w
        .call("db.kv.persist", json!({ "sessionId": s, "key": "m:a" }))
        .await
        .unwrap();
    assert_eq!(persisted, true);
    assert_eq!(int(&run(&mut admin, "PTTL m:a").await), -1);

    let rename = json!({ "sessionId": s, "from": "m:a", "to": "m:b" });
    let outcome = call_confirmed(&mut w, "db.kv.rename", rename).await;
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

/// 확인(3001)을 받고 그 해시로 다시 부른다. 확인 전에는 서버에 닿지 않는다(부르는 쪽이 본다).
async fn call_confirmed(w: &mut Conn, method: &str, mut params: Value) -> Value {
    let err = w.call(method, params.clone()).await.unwrap_err();
    assert_eq!(err.code, code::CONFIRM_REQUIRED, "{method}: {err:?}");
    let data = err.data.unwrap();
    params["confirmed"] =
        json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });
    w.call(method, params).await.unwrap()
}

/// 관문을 거치지 않는 준비용 명령(argv 그대로 — `EVAL` 의 스크립트처럼 공백이 든 인자).
async fn run_argv(conn: &mut Box<dyn KvConnection>, argv: &[&str]) -> KvReply {
    conn.command(argv.iter().map(|a| (*a).to_owned()).collect())
        .await
        .unwrap()
}

/// 키 `key` 가 `n` 개의 원소를 갖게 서버에서 채운다(이미 그 크기면 그대로). `kind` 는 `set` · `hash` · `list` · `zset` · `stream`.
async fn fill(conn: &mut Box<dyn KvConnection>, kind: &str, key: &str, n: i64) {
    let (length, script) = match kind {
        "set" => (
            "SCARD",
            "for i=1,tonumber(ARGV[1]),1000 do local t={} for j=i,math.min(i+999,tonumber(ARGV[1])) do t[#t+1]=j end redis.call('SADD',KEYS[1],unpack(t)) end",
        ),
        "hash" => (
            "HLEN",
            "for i=1,tonumber(ARGV[1]),500 do local t={} for j=i,math.min(i+499,tonumber(ARGV[1])) do t[#t+1]=j t[#t+1]='v' end redis.call('HSET',KEYS[1],unpack(t)) end",
        ),
        "list" => (
            "LLEN",
            "for i=1,tonumber(ARGV[1]),1000 do local t={} for j=i,math.min(i+999,tonumber(ARGV[1])) do t[#t+1]=j end redis.call('RPUSH',KEYS[1],unpack(t)) end",
        ),
        "zset" => (
            "ZCARD",
            "for i=1,tonumber(ARGV[1]),500 do local t={} for j=i,math.min(i+499,tonumber(ARGV[1])) do t[#t+1]=j t[#t+1]=j end redis.call('ZADD',KEYS[1],unpack(t)) end",
        ),
        "stream" => (
            "XLEN",
            "for i=1,tonumber(ARGV[1]) do redis.call('XADD',KEYS[1],i..'-1','f','v') end",
        ),
        _ => panic!("{kind}"),
    };
    if int(&run_argv(conn, &[length, key]).await) == n {
        return;
    }
    run_argv(conn, &["DEL", key]).await;
    run_argv(conn, &["EVAL", script, "1", key, &n.to_string()]).await;
    assert_eq!(int(&run_argv(conn, &[length, key]).await), n);
}

/// M3-22 리뷰 H1: `db.kv.setTtl` 은 `PEXPIRE` 와 같은 확인(3001)을 받는다 — `ttlMs: 1` 은 확인 없이 키를 지우지 않는다(서버에
/// 닿지 않는다). 확인을 받은 뒤에만 그 TTL 이 걸린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_set_ttl_needs_the_confirmation_of_pexpire() {
    let e = env(13);
    let mut admin = e.admin().await;
    // ② 와 같은 DB 라 지우지 않고 자기 키만 쓴다.
    run(&mut admin, "SET h1:victim 1").await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let pexpire = w
        .call("db.kv.command", command(&s, "PEXPIRE h1:victim 1"))
        .await
        .unwrap_err();
    assert_eq!(pexpire.code, code::CONFIRM_REQUIRED, "{pexpire:?}");
    let err = w
        .call(
            "db.kv.setTtl",
            json!({ "sessionId": s, "key": "h1:victim", "ttlMs": 1 }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, code::CONFIRM_REQUIRED, "{err:?}");
    let data = err.data.unwrap();
    assert_eq!(data["grade"], pexpire.data.unwrap()["grade"]);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        int(&run(&mut admin, "EXISTS h1:victim").await),
        1,
        "setTtl 1 ms deleted the key without a confirmation"
    );
    assert_eq!(int(&run(&mut admin, "PTTL h1:victim").await), -1);
    let set = w
        .call(
            "db.kv.setTtl",
            json!({ "sessionId": s, "key": "h1:victim", "ttlMs": 1,
                    "confirmed": { "grade": data["grade"], "statementsHash": data["statementsHash"] } }),
        )
        .await
        .unwrap();
    assert_eq!(set, true);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(int(&run(&mut admin, "EXISTS h1:victim").await), 0);
}

/// M3-22 리뷰 H2 · M3: 중첩 응답은 원소 · 바이트를 중첩 전체로 세어 끊는다(`XREAD` 는 맨 위 원소가 stream 하나뿐이다). `COUNT` 로
/// 묶지 않은 `XREAD` 는 stream 길이를 먼저 물어 서버에 보내지 않는다(리뷰어 재현: 항목 20만 개 stream 이 20 MB 로 갔다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_a_nested_reply_is_cut_at_the_reply_limits() {
    const ENTRIES: i64 = 200_000;
    let e = env(13);
    let mut admin = e.admin().await;
    fill(&mut admin, "stream", "h2:st", ENTRIES).await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;

    let xread = calls_of(&mut admin, "xread").await;
    let err = w
        .call("db.kv.command", command(&s, "XREAD STREAMS h2:st 0"))
        .await
        .unwrap_err();
    assert_eq!(err.data.unwrap()["error"]["code"], "KEY_TOO_LARGE");
    // 재리뷰 A: `COUNT 0` 은 Redis 에서 무제한이고, 키 자리의 `COUNT` 낱말은 선택 인자가 아니다 — 둘 다 묻는다.
    for text in [
        "XREAD COUNT 0 STREAMS h2:st 0",
        "XREAD STREAMS h2:st COUNT 0 0",
        // 재리뷰 2 H: 서버는 음수 `COUNT` 를 0, 곧 무제한으로 읽는다.
        "XREAD COUNT -1 STREAMS h2:st 0",
        // 재리뷰 3 J: 겹친 `COUNT` 는 서버가 마지막 것을 쓴다(묶이지 않은 `COUNT 1000000`).
        "XRANGE h2:st - + COUNT 5 COUNT 1000000",
        "XREVRANGE h2:st + - COUNT 5 COUNT 1000000",
    ] {
        let err = w
            .call("db.kv.command", command(&s, text))
            .await
            .unwrap_err();
        assert_eq!(
            err.data.unwrap()["error"]["code"],
            "KEY_TOO_LARGE",
            "{text}"
        );
    }
    assert_eq!(
        calls_of(&mut admin, "xread").await,
        xread,
        "the unbounded XREAD reached the server"
    );
    // 재리뷰 F: id `$` 는 그 뒤의 새 항목만이라 빈 답이다 — 묻지 않고 간다.
    let reply = w
        .call("db.kv.command", command(&s, "XREAD STREAMS h2:st $"))
        .await
        .unwrap();
    assert_eq!(reply, json!("nil"));
    assert_eq!(calls_of(&mut admin, "xread").await, xread + 1);
    // 재리뷰 E: `XREADGROUP` 도 같다(`>` 는 쌓인 만큼 준다). 서버에 가지 않으므로 PEL 도 비어 있다.
    run_argv(&mut admin, &["XGROUP", "DESTROY", "h2:st", "g"]).await;
    run_argv(&mut admin, &["XGROUP", "CREATE", "h2:st", "g", "0"]).await;
    let xreadgroup = calls_of(&mut admin, "xreadgroup").await;
    for text in [
        "XREADGROUP GROUP g c STREAMS h2:st >",
        "XREADGROUP GROUP g c COUNT 0 STREAMS h2:st >",
        "XREADGROUP GROUP g c COUNT -1 STREAMS h2:st >",
    ] {
        let err = w
            .call("db.kv.command", command(&s, text))
            .await
            .unwrap_err();
        assert_eq!(
            err.data.unwrap()["error"]["code"],
            "KEY_TOO_LARGE",
            "{text}"
        );
    }
    assert_eq!(calls_of(&mut admin, "xreadgroup").await, xreadgroup);
    let KvReply::Array(pending) = run(&mut admin, "XPENDING h2:st g").await else {
        panic!("XPENDING is a list")
    };
    assert_eq!(pending.first().map(int), Some(0), "entries went to the PEL");
    run_argv(&mut admin, &["XGROUP", "DESTROY", "h2:st", "g"]).await;

    // 묶인 `XREAD`(COUNT 9000 — 맨 위 원소 하나, 중첩 원소 45,003개)는 서버에 가고 응답 상한에서 끊긴다.
    let started = Instant::now();
    let reply = w
        .call(
            "db.kv.command",
            command(&s, "XREAD COUNT 9000 STREAMS h2:st 0"),
        )
        .await
        .unwrap();
    let elapsed = started.elapsed();
    let json = serde_json::to_vec(&reply).unwrap().len();
    let cut = &reply["truncated"];
    assert_eq!(
        cut["elements"],
        1 + 2 + 9_000 * 5,
        "{}",
        &reply.to_string()[..200]
    );
    assert!(json < 2 * 1024 * 1024, "{json} bytes went to the window");
    println!("bounded XREAD of a {ENTRIES}-entry stream: {elapsed:?}, {json} JSON bytes");
}

/// M3-22 리뷰 M3: 응답을 부풀리는 읽기(음수 count 의 `SRANDMEMBER`)와 키를 통째로 읽는 `GETRANGE 0 -1` · `DUMP` 는 크기를
/// 먼저 물어 서버에 보내지 않는다(`KEY_TOO_LARGE`). 상한 안의 것은 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_amplifying_and_whole_reads_are_refused_before_the_server() {
    let e = env(13);
    let mut admin = e.admin().await;
    run(&mut admin, "DEL m3:tiny").await;
    run(&mut admin, "SADD m3:tiny x").await;
    let big = "y".repeat(8 * 1024 * 1024);
    run_argv(&mut admin, &["SET", "m3:big", &big]).await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let (srandmember, dump, mget) = (
        calls_of(&mut admin, "srandmember").await,
        calls_of(&mut admin, "dump").await,
        calls_of(&mut admin, "mget").await,
    );
    for text in [
        "SRANDMEMBER m3:tiny -2000000",
        "GETRANGE m3:big 0 -1",
        "GETRANGE m3:big 0 5000000",
        "DUMP m3:big",
        // 재리뷰 B: 다른 타입 키(`m3:tiny` 는 set — `STRLEN` 이 WRONGTYPE)를 섞어도 묻기가 열리지 않는다. `MGET` 은 그 키에 nil
        // 로 답하므로 서버 쪽은 실패하지 않는다.
        "MGET m3:tiny m3:big",
    ] {
        let err = w
            .call("db.kv.command", command(&s, text))
            .await
            .unwrap_err();
        assert_eq!(
            err.data.as_ref().unwrap()["error"]["code"],
            "KEY_TOO_LARGE",
            "{text}: {err:?}"
        );
    }
    assert_eq!(calls_of(&mut admin, "srandmember").await, srandmember);
    assert_eq!(calls_of(&mut admin, "dump").await, dump);
    assert_eq!(
        calls_of(&mut admin, "mget").await,
        mget,
        "the mixed MGET reached the server"
    );
    // 상한 안이면 돈다.
    let reply = w
        .call("db.kv.command", command(&s, "SRANDMEMBER m3:tiny -3"))
        .await
        .unwrap();
    assert_eq!(reply["array"].as_array().unwrap().len(), 3, "{reply}");
    let reply = w
        .call("db.kv.command", command(&s, "GETRANGE m3:big 0 9"))
        .await
        .unwrap();
    assert_eq!(reply["bulk"]["text"], "yyyyyyyyyy", "{reply}");
    run(&mut admin, "DEL m3:big").await;
}

/// M3-22 리뷰 M1: 요청 취소는 그 연결을 버린다 — 버린 응답 뒤에 다음 요청이 서지 않고, 새 연결은 그 DB 번호로 다시 붙는다.
/// 리뷰어 재현(큰 응답을 취소한 뒤 같은 세션의 `PING` 3,565 ms)이 빠른 응답이 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_a_cancelled_request_does_not_hold_up_the_next() {
    let e = env(13);
    let mut admin = e.admin().await;
    fill(&mut admin, "set", "m1:big", 1_500_000).await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let connections = info_field(&mut admin, "stats", "total_connections_received").await;
    // 서버 전체의 호출 수로 「서버가 마쳤다」 를 본다 — 다른 시험이 쓰지 않는 명령이다.
    let sdiff = calls_of(&mut admin, "sdiff").await;
    // 여러 키의 `SDIFF`(빈 키를 빼면 그 set 전체 — 크기 사전 확인 목록 밖, 응답 상한이 끊는다): 응답 150만 원소. 서버가 명령을 마친 뒤(응답을
    // 보내고 Host 가 받아 읽는 중) 취소한다 — 그 뒤의 기다림은 서버가 아니라 버린 응답 몫이다.
    w.session.handle_value(json!({
        "jsonrpc": "2.0", "id": 900, "method": "db.kv.command",
        "params": command(&s, "SDIFF m1:big m1:none"),
    }));
    let deadline = Instant::now() + Duration::from_secs(10);
    while calls_of(&mut admin, "sdiff").await == sdiff {
        assert!(Instant::now() < deadline, "SDIFF never ran");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    w.session.handle_value(json!({
        "jsonrpc": "2.0", "method": "request.cancel", "params": { "id": 900 },
    }));
    let started = Instant::now();
    let pong = w.call("db.kv.command", command(&s, "PING")).await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(pong["status"], "PONG");
    println!("PING after a cancelled 1.5M-element reply: {elapsed:?}");
    assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
    assert!(
        info_field(&mut admin, "stats", "total_connections_received").await > connections,
        "the cancelled request's connection was kept"
    );
    // 새 연결도 같은 DB 번호다(그 키는 DB 13 에만 있다).
    let exists = w
        .call("db.kv.command", command(&s, "EXISTS m1:big"))
        .await
        .unwrap();
    assert_eq!(exists["int"]["int"], 1, "{exists}");
}

/// M3-22 리뷰 M2: 취소는 그 실행에만 닿는다. ① 요청을 보내고 곧바로 취소한 뒤의 다음 요청이 `cancelled` 가 아니다(리뷰어 재현:
/// 200번 중 8번) ② credit 을 기다리는(서버에서 돌지 않는) 스캔을 `db.query.cancel` 로 끌 때, 같은 세션에서 도는 무관한 명령이
/// 멈추지 않는다. 각각 100번 되풀이해 한 번도 멈추지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_a_late_cancel_does_not_stop_the_next_request() {
    let e = env(13);
    let mut admin = e.admin().await;
    // 다른 시험이 세지 않는 명령(`SINTER` — 같은 키 둘이면 그 set 전체, 사전 확인 없이 바로 간다)으로 몇 ms 걸리는 응답을 만든다.
    fill(&mut admin, "set", "m2:mid", 9_000).await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let stopped = |r: &Result<Value, RpcError>| {
        r.as_ref().is_err_and(|e| {
            e.code == rpc_code::CANCELLED
                || e.data
                    .as_ref()
                    .is_some_and(|d| d["error"]["kind"] == "cancelled")
        })
    };
    let mut cancelled = 0;
    for round in 0..100 {
        let id = 10_000 + round;
        w.session.handle_value(json!({
            "jsonrpc": "2.0", "id": id, "method": "db.kv.command",
            "params": command(&s, "SINTER m2:mid m2:mid"),
        }));
        tokio::time::sleep(Duration::from_millis(2)).await;
        w.session.handle_value(json!({
            "jsonrpc": "2.0", "method": "request.cancel", "params": { "id": id },
        }));
        let pong = w.call("db.kv.command", command(&s, "PING")).await;
        if stopped(&pong) {
            cancelled += 1;
        } else {
            assert_eq!(pong.unwrap()["status"], "PONG", "round {round}");
        }
    }
    assert_eq!(
        cancelled, 0,
        "a late cancel stopped {cancelled} of 100 PINGs"
    );

    for round in 0..100 {
        let stream = w
            .start("db.kv.scan", json!({ "sessionId": s, "count": 1 }))
            .await
            .unwrap();
        w.credit(stream, 2);
        let StreamMsg::Chunk(accepted) = w.next(stream).await else {
            panic!("accepted")
        };
        let execution = accepted["executionId"].as_str().unwrap().to_owned();
        let _ = w.next(stream).await;
        // 다음 묶음을 읽고 credit 을 기다릴 틈.
        tokio::time::sleep(Duration::from_millis(5)).await;
        let id = 20_000 + round;
        w.session.handle_value(json!({
            "jsonrpc": "2.0", "id": id, "method": "db.kv.command",
            "params": command(&s, "SINTER m2:mid m2:mid"),
        }));
        tokio::time::sleep(Duration::from_micros(500)).await;
        w.call("db.query.cancel", json!({ "executionId": execution }))
            .await
            .unwrap();
        let reply = w.response(id).await;
        if stopped(&reply) {
            cancelled += 1;
        } else {
            assert_eq!(
                reply.unwrap()["array"].as_array().unwrap().len(),
                9_000,
                "round {round}"
            );
        }
        w.cancel_stream(stream);
    }
    assert_eq!(
        cancelled, 0,
        "a scan's cancel stopped {cancelled} of 100 commands"
    );
}

/// M3-22 리뷰 L1: 값 쪽 읽기의 예산은 창으로 갈 JSON 크기다 — 작은 필드 30만 개 hash 를 큰 `range.count` 로 읽어도 한 쪽이
/// 예산(4 MiB) 근처에서 끝나고 다음 커서가 있다(리뷰어 재현: 30만 필드 전부, JSON 14.6 MB).
///
/// 이 시험은 쪽의 크기를 재지 빠르기를 재지 않는다. 4 MiB 쪽 읽기는 한가할 때 0.3 s 남짓이지만 빈 서버(CI)에서는 같은 서버에 키
/// 100만 개를 심는 시험 둘이 함께 돌아, 부하에서 3 s 가까이 걸린 일이 있다 — `call` 의 5 초 무응답 감시 대신 제품의 Redis 응답
/// 상한([`DEFAULT_RESPONSE_TIMEOUT`])까지 기다린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_a_value_page_is_bounded_by_its_json_size() {
    let e = env(13);
    let mut admin = e.admin().await;
    fill(&mut admin, "hash", "l1:h", 300_000).await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let page = w
        .call_within(
            "db.kv.get",
            json!({ "sessionId": s, "key": "l1:h", "range": { "count": 4_194_304 } }),
            DEFAULT_RESPONSE_TIMEOUT,
        )
        .await
        .unwrap();
    let json = serde_json::to_vec(&page).unwrap().len();
    let fields = page["value"]["hash"].as_array().unwrap().len();
    println!("one page of a 300k-field hash: {fields} fields, {json} JSON bytes");
    assert!(json <= 4 * 1024 * 1024 + 1024 * 1024, "{json}");
    assert!(fields < 300_000);
    assert!(page["nextCursor"].is_string(), "{}", page["nextCursor"]);
}

/// `INFO <section>` 의 한 값(수).
async fn info_field(conn: &mut Box<dyn KvConnection>, section: &str, field: &str) -> u64 {
    let KvReply::Bulk(DbValue::Text(info)) = run(conn, &format!("INFO {section}")).await else {
        panic!("INFO is a bulk string")
    };
    let prefix = format!("{field}:");
    info.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|n| n.trim().parse().unwrap())
        .unwrap_or_else(|| panic!("no {field}"))
}

/// 14 M3-22 판정(1): 서버를 오래 붙잡는 명령(`LCS` · `COUNT n ANY` 없는 `GEOSEARCH`)은 확인(3001)을 받고,
/// 확인 전에는 서버에 닿지 않는다. 등급은 조회라 read-only 연결에서도 확인 뒤 돈다. 판정은 다른 시험이 세지 않는 명령의 수다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_commands_that_hold_the_server_need_a_confirmation() {
    let e = env(13);
    let mut admin = e.admin().await;
    run(&mut admin, "MSET p1:a ohmytext p1:b mynewtext").await;
    run(&mut admin, "GEOADD p1:g 13.361389 38.115556 palermo").await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "ro").await;
    let (lcs, geosearch) = (
        calls_of(&mut admin, "lcs").await,
        calls_of(&mut admin, "geosearch").await,
    );
    for text in [
        "LCS p1:a p1:b",
        "GEOSEARCH p1:g FROMLONLAT 13 38 BYRADIUS 200 km",
        // 재리뷰 C: `ANY` 없는 `COUNT` 는 서버 일을 줄이지 않는다 — 큰 `COUNT` 로 비켜 가지 않는다.
        "GEOSEARCH p1:g FROMLONLAT 13 38 BYRADIUS 200 km COUNT 9223372036854775807",
        // 재리뷰 2 C′: `ANY` 는 n 개를 찾으면 멈출 뿐 — 큰 n 은 면제하지 않는다.
        "GEOSEARCH p1:g FROMLONLAT 13 38 BYRADIUS 200 km COUNT 9223372036854775807 ANY",
    ] {
        let err = w
            .call("db.kv.command", command(&s, text))
            .await
            .unwrap_err();
        assert_eq!(err.code, code::CONFIRM_REQUIRED, "{text}: {err:?}");
        assert_eq!(err.data.unwrap()["grade"], "read", "{text}");
    }
    assert_eq!(
        calls_of(&mut admin, "lcs").await,
        lcs,
        "LCS reached the server"
    );
    assert_eq!(calls_of(&mut admin, "geosearch").await, geosearch);

    // 확인을 받으면 read-only 연결에서도 돈다.
    let reply = call_confirmed(&mut w, "db.kv.command", command(&s, "LCS p1:a p1:b")).await;
    assert_eq!(reply["bulk"]["text"], "mytext", "{reply}");
    assert_eq!(calls_of(&mut admin, "lcs").await, lcs + 1);
    // `COUNT n ANY` 로 묶으면 확인 없이 돈다.
    let reply = w
        .call(
            "db.kv.command",
            command(
                &s,
                "GEOSEARCH p1:g FROMLONLAT 13 38 BYRADIUS 200 km COUNT 5 ANY",
            ),
        )
        .await
        .unwrap();
    assert_eq!(reply["array"][0]["bulk"]["text"], "palermo", "{reply}");
    run(&mut admin, "DEL p1:a p1:b p1:g").await;
}

/// 14 M3-22 판정(2): 키 하나의 `SUNION` · `SINTER` · `SDIFF` · `ZUNION` · `ZINTER` · `ZDIFF` 는 그 키 전체라 `SMEMBERS` 처럼
/// 길이(`SCARD` · `ZCARD`)를 먼저 묻고, 상한을 넘으면 서버에 보내지 않는다. 여러 키는 그대로 가서 응답 상한이 끊는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_a_one_key_set_operation_is_probed_like_smembers() {
    let e = env(13);
    let mut admin = e.admin().await;
    fill(&mut admin, "set", "p2:set", 15_000).await;
    fill(&mut admin, "zset", "p2:zset", 15_000).await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "r").await;
    let (zunion, zdiff) = (
        calls_of(&mut admin, "zunion").await,
        calls_of(&mut admin, "zdiff").await,
    );
    for text in [
        "SUNION p2:set",
        "SINTER p2:set",
        "SDIFF p2:set",
        "ZUNION 1 p2:zset",
        "ZINTER 1 p2:zset",
        "ZDIFF 1 p2:zset WITHSCORES",
        // 재리뷰 2 I: 입력은 set 도 된다(점수 1) — ZCARD 만 물으면 WRONGTYPE(0)으로 비켜 간다.
        "ZUNION 1 p2:set",
        "ZDIFF 1 p2:set",
        // 재리뷰 3 J: 겹친 `LIMIT` 는 서버가 마지막 것을 쓴다(`LIMIT 0 -1` = 전부).
        "ZRANGEBYSCORE p2:zset -inf +inf LIMIT 0 5 LIMIT 0 -1",
        "ZRANGE p2:zset -inf +inf BYSCORE LIMIT 0 5 LIMIT 0 -1",
    ] {
        let err = w
            .call("db.kv.command", command(&s, text))
            .await
            .unwrap_err();
        assert_eq!(
            err.data.as_ref().unwrap()["error"]["code"],
            "KEY_TOO_LARGE",
            "{text}: {err:?}"
        );
    }
    assert_eq!(
        calls_of(&mut admin, "zunion").await,
        zunion,
        "ZUNION reached the server"
    );
    assert_eq!(calls_of(&mut admin, "zdiff").await, zdiff);
    // 여러 키는 묻지 않고 가서 응답 상한에서 끊긴다.
    let reply = w
        .call("db.kv.command", command(&s, "ZUNION 2 p2:zset p2:none"))
        .await
        .unwrap();
    assert_eq!(reply["truncated"]["elements"], 15_000, "{reply}");
    assert_eq!(calls_of(&mut admin, "zunion").await, zunion + 1);
}

/// 재리뷰 D: `SORT_RO` 의 비용(`BY` · 정렬)은 목록 길이에 비례한다 — `LIMIT` 과 상관없이 길이를 먼저 묻고, `SMEMBERS` 와 같은
/// 상한(원소 1만)을 넘으면 서버에 보내지 않는다. 타입마다 길이 명령이 다르다(list · set · zset).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_sort_ro_is_probed_by_the_list_length() {
    let e = env(13);
    let mut admin = e.admin().await;
    fill(&mut admin, "list", "p3:list", 15_000).await;
    fill(&mut admin, "set", "p3:set", 15_000).await;
    run(&mut admin, "DEL p3:small").await;
    run(&mut admin, "RPUSH p3:small 3 1 2").await;
    let mut w = e.f.window();
    let s = e.open(&mut w, "ro").await;
    let sort_ro = calls_of(&mut admin, "sort_ro").await;
    for text in [
        "SORT_RO p3:list",
        "SORT_RO p3:list LIMIT 0 10",
        "SORT_RO p3:set BY nosort LIMIT 0 10",
    ] {
        let err = w
            .call("db.kv.command", command(&s, text))
            .await
            .unwrap_err();
        assert_eq!(
            err.data.as_ref().unwrap()["error"]["code"],
            "KEY_TOO_LARGE",
            "{text}: {err:?}"
        );
    }
    assert_eq!(
        calls_of(&mut admin, "sort_ro").await,
        sort_ro,
        "SORT_RO reached the server"
    );
    // 짧은 목록은 확인 없이(read-only 에서도) 돈다.
    let reply = w
        .call("db.kv.command", command(&s, "SORT_RO p3:small"))
        .await
        .unwrap();
    assert_eq!(reply["array"].as_array().unwrap().len(), 3, "{reply}");
    assert_eq!(calls_of(&mut admin, "sort_ro").await, sort_ro + 1);
}
