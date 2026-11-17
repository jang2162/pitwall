//! M5-19 의 실 Redis 시험(16-m5-plan.md M5-19 완료 기준의 live 줄): 실제 드라이버(`pw-db-driver-redis`)를 관문 뒤에 붙여 에이전트의
//! Redis 조회 툴 `db_kv_scan`(`db.tool.kvScan`) · `db_kv_get`(`db.tool.kvGet`)을 MCP 세션 범위의 RPC 연결로 부른다.
//! `PW_TEST_REDIS_URL` 이 있을 때만 목록에 있다(`pw_db_driver::live`, `mise run test-db -- redis`).
//!
//! DB 번호: 0 ~ 15 가 모두 다른 시험의 것이라(`kv_live.rs` 머리 주석) 여러 시험이 함께 쓰는 13 을 쓰고, 이 파일의 키는 모두 접두사
//! `m519:` 로 시작한다(DB 를 지우지 않는다). 보낸 명령은 서버의 `MONITOR` 로 모은다 — 서버 전체의 명령이 오므로 이 시험의 접두사가 든
//! 명령을 보낸 클라이언트 주소(툴이 호출마다 연 연결)의 줄만 본다.

#![cfg(pw_live_redis)]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Conn, Fixture};
use pw_db::DbSessions;
use pw_db_driver::live::{LiveDb, endpoint};
use pw_db_driver::{DbConnection, DbDriver, KvConnection};
use pw_db_driver_redis::RedisDriver;
use pw_db_sql::kv_command::{KvCommandClass, classify_argv};
use pw_db_types::{ConnectionSpec, DriverKind, KvReply, Secret, SslMode, Target, TlsSettings};
use pw_rpc::ConnectionScope;
use serde_json::{Value, json};

const DB: u32 = 13;

struct Env {
    f: Fixture,
    agent: Conn,
}

/// 실 Redis 드라이버를 둔 Host, DB 13 을 가리키는 연결 `r`, 그 연결을 한 번 연 프로젝트 창, MCP 세션 범위의 연결.
async fn env() -> Env {
    let ep = endpoint(LiveDb::Redis, &LiveDb::Redis.url()).unwrap();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(RedisDriver::new())).unwrap();
    let f = Fixture::build(true, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    let password = if ep.password.is_some() {
        "password = { secret_ref = \"keychain\" }\n"
    } else {
        ""
    };
    f.write_db(
        "r.toml",
        &format!(
            "schema = 1\nname = \"live redis\"\ndriver = \"redis\"\nhost = \"{}\"\nport = {}\ndatabase = \"{DB}\"\nsslmode = \"disable\"\n{password}",
            ep.host, ep.port
        ),
    );
    let mut w = f.window();
    if let Some(password) = &ep.password {
        w.set_password(&f.id, "r", password).await.unwrap();
    }
    let mut agent = f.connect(ConnectionScope::Agent);
    // 사람이 열기 전에는 거절된다.
    let refused = agent
        .call(
            "db.tool.kvScan",
            json!({ "projectId": f.id, "connectionId": "r" }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.data.unwrap()["reason"], "notOpenedByUser");
    w.call(
        "db.session.open",
        json!({ "projectId": f.id, "connectionId": "r" }),
    )
    .await
    .unwrap();
    Env { f, agent }
}

impl Env {
    async fn call(&mut self, method: &str, extra: Value) -> Value {
        let mut params = json!({ "projectId": self.f.id, "connectionId": "r" });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        self.agent.call(method, params).await.unwrap()
    }

    /// `match` 로 끝까지 쪽을 넘겨 모은 키 이름과 쪽 수.
    async fn scan_all(&mut self, pattern: &str, limit: u32) -> (Vec<String>, usize) {
        let mut keys = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut extra = json!({ "match": pattern, "limit": limit });
            if let Some(c) = &cursor {
                extra["cursor"] = json!(c);
            }
            let page = self.call("db.tool.kvScan", extra).await;
            assert!(page.to_string().len() <= pw_db::script::RESPONSE_BUDGET);
            pages += 1;
            let mut on_page: Vec<String> = Vec::new();
            for k in page["keys"].as_array().unwrap() {
                on_page.push(k["key"].as_str().unwrap().to_owned());
            }
            let n = on_page.len();
            on_page.sort();
            on_page.dedup();
            assert_eq!(on_page.len(), n, "a key twice within one page");
            keys.extend(on_page);
            match page["nextCursor"].as_str() {
                Some(c) => cursor = Some(c.to_owned()),
                None => return (keys, pages),
            }
            assert!(pages < 100_000, "the scan does not end");
        }
    }

    /// 한 키의 값을 끝까지 쪽으로 읽는다(쪽마다 JSON 이 응답 예산 안).
    async fn read_all(&mut self, key: Value, limit: Option<u32>) -> Vec<Value> {
        let mut pages = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut extra = json!({ "key": key });
            if let Some(limit) = limit {
                extra["limit"] = json!(limit);
            }
            if let Some(c) = &cursor {
                extra["cursor"] = json!(c);
            }
            let page = self.call("db.tool.kvGet", extra).await;
            assert!(page.to_string().len() <= pw_db::script::RESPONSE_BUDGET);
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            pages.push(page);
            if cursor.is_none() {
                return pages;
            }
            assert!(pages.len() < 10_000, "the read does not end");
        }
    }
}

/// 시험 준비 · 판정용 연결(관문 · 분류를 거치지 않는다).
async fn admin() -> Box<dyn KvConnection> {
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
    spec.database = Some(DB.to_string());
    spec.password = ep.password.map(Secret::new);
    match RedisDriver::new().connect(spec).await.unwrap().connection {
        DbConnection::Kv(conn) => conn,
        DbConnection::Sql(_) => unreachable!("redis is a kv driver"),
    }
}

async fn run(conn: &mut Box<dyn KvConnection>, argv: &[&str]) -> KvReply {
    conn.command(argv.iter().map(|a| (*a).to_owned()).collect())
        .await
        .unwrap()
}

/// 키 1,200개를 쪽으로 넘겨 빠짐없이 모은다(쪽 안의 중복 없음). `match` 가 다른 접두사의 키를 거른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_kv_tool_pages_through_1200_keys_with_match() {
    let mut admin = admin().await;
    let mut set = vec!["MSET".to_owned()];
    for i in 0..1_200 {
        set.push(format!("m519:page:{i:04}"));
        set.push("v".into());
    }
    for i in 0..50 {
        set.push(format!("m519:other:{i}"));
        set.push("v".into());
    }
    admin.command(set).await.unwrap();
    let mut e = env().await;
    let (mut keys, pages) = e.scan_all("m519:page:*", 100).await;
    println!("1,200 keys in {pages} pages of limit 100");
    assert!(pages >= 8, "{pages} pages");
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 1_200);
    assert!(keys.iter().all(|k| k.starts_with("m519:page:")));
    let (other, _) = e.scan_all("m519:other:*", 500).await;
    assert_eq!(other.len(), 50);
    // 키마다 타입 · 크기가 있다.
    let page = e
        .call(
            "db.tool.kvScan",
            json!({ "match": "m519:other:1", "limit": 10 }),
        )
        .await;
    let mut cursor = page;
    let info = loop {
        if let Some(k) = cursor["keys"].as_array().unwrap().first() {
            break k.clone();
        }
        let c = cursor["nextCursor"].as_str().unwrap().to_owned();
        cursor = e
            .call(
                "db.tool.kvScan",
                json!({ "match": "m519:other:1", "limit": 10, "cursor": c }),
            )
            .await;
    };
    assert_eq!(info["kind"], "string");
    assert!(info["size"].as_u64().is_some(), "{info}");
}

/// 타입별 값 읽기 — string · hash · list · set · zset · stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_kv_tool_reads_each_type() {
    let mut admin = admin().await;
    for key in [
        "m519:t:hash",
        "m519:t:list",
        "m519:t:set",
        "m519:t:zset",
        "m519:t:stream",
    ] {
        run(&mut admin, &["DEL", key]).await;
    }
    run(&mut admin, &["SET", "m519:t:string", "hello"]).await;
    run(&mut admin, &["HSET", "m519:t:hash", "f1", "a", "f2", "b"]).await;
    run(&mut admin, &["RPUSH", "m519:t:list", "x", "y", "z"]).await;
    run(&mut admin, &["SADD", "m519:t:set", "s1", "s2"]).await;
    run(&mut admin, &["ZADD", "m519:t:zset", "1", "one", "2", "two"]).await;
    run(
        &mut admin,
        &["XADD", "m519:t:stream", "1-1", "field", "value"],
    )
    .await;
    let mut e = env().await;
    let get = |page: &Value, kind: &str| page["value"][kind].clone();

    let s = e
        .call("db.tool.kvGet", json!({ "key": "m519:t:string" }))
        .await;
    assert_eq!(
        (&s["kind"], &get(&s, "string")["text"]),
        (&json!("string"), &json!("hello"))
    );

    let h = e
        .call("db.tool.kvGet", json!({ "key": "m519:t:hash" }))
        .await;
    let mut fields: Vec<(String, String)> = get(&h, "hash")
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["field"]["text"].as_str().unwrap().to_owned(),
                f["value"]["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        [("f1".into(), "a".into()), ("f2".into(), "b".into())]
    );

    let l = e
        .call("db.tool.kvGet", json!({ "key": "m519:t:list" }))
        .await;
    assert_eq!(
        get(&l, "list"),
        json!([{ "text": "x" }, { "text": "y" }, { "text": "z" }])
    );
    assert_eq!(l["total"], 3);

    let set = e
        .call("db.tool.kvGet", json!({ "key": "m519:t:set" }))
        .await;
    assert_eq!(
        (&set["kind"], set["total"].as_u64()),
        (&json!("set"), Some(2))
    );

    let z = e
        .call("db.tool.kvGet", json!({ "key": "m519:t:zset" }))
        .await;
    assert_eq!(get(&z, "zset")[1]["member"]["text"], "two");
    assert_eq!(get(&z, "zset")[1]["score"], "2");

    let x = e
        .call("db.tool.kvGet", json!({ "key": "m519:t:stream" }))
        .await;
    assert_eq!(get(&x, "stream")[0]["id"], "1-1");
    assert_eq!(get(&x, "stream")[0]["fields"][0]["value"]["text"], "value");
}

/// 큰 값은 응답 상한 안의 쪽으로 나뉜다 — 300 KiB string 은 바이트 창으로, 필드 2,000개(필드마다 200 바이트) hash 는 쪽으로. 모아 보면
/// 빠진 것이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_kv_tool_cuts_a_large_value_into_pages() {
    let mut admin = admin().await;
    let text: String = (0..300 * 1024)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    run(&mut admin, &["SET", "m519:big:string", &text]).await;
    run(&mut admin, &["DEL", "m519:big:hash"]).await;
    let mut hset = vec!["HSET".to_owned(), "m519:big:hash".to_owned()];
    for i in 0..2_000 {
        hset.push(format!("field{i:04}"));
        hset.push("w".repeat(200));
    }
    admin.command(hset).await.unwrap();
    let mut e = env().await;

    let pages = e.read_all(json!("m519:big:string"), None).await;
    assert!(pages.len() >= 5, "{} pages", pages.len());
    let joined: String = pages
        .iter()
        .map(|p| p["value"]["string"]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(joined, text);
    assert_eq!(pages[0]["total"], 300 * 1024);

    let pages = e.read_all(json!("m519:big:hash"), Some(500)).await;
    assert!(pages.len() >= 4, "{} pages", pages.len());
    let mut fields: Vec<String> = pages
        .iter()
        .flat_map(|p| p["value"]["hash"].as_array().unwrap().clone())
        .map(|f| {
            assert_eq!(f["value"]["text"].as_str().unwrap().len(), 200);
            f["field"]["text"].as_str().unwrap().to_owned()
        })
        .collect();
    fields.sort();
    fields.dedup();
    assert_eq!(fields.len(), 2_000);
}

/// 없는 키는 `missing` 이다(타입 · TTL 도 없다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_kv_tool_a_missing_key_is_missing() {
    let mut admin = admin().await;
    run(&mut admin, &["DEL", "m519:missing"]).await;
    let mut e = env().await;
    let page = e
        .call("db.tool.kvGet", json!({ "key": "m519:missing" }))
        .await;
    assert_eq!(page["value"], "missing");
    assert!(page["kind"].is_null() && page["ttlMs"].is_null(), "{page}");
}

// --- 보낸 명령(MONITOR) -------------------------------------------------------------------------------------------------

/// `MONITOR` 한 줄: `+<시각> [<db> <주소>] "<인자>" …`. 인자는 서버의 repr(`\"` · `\\` · `\n` · `\r` · `\t` · `\a` · `\b` · `\xhh`).
fn monitor_line(line: &str) -> Option<(String, Vec<Vec<u8>>)> {
    let line = line.strip_prefix('+')?;
    let open = line.find('[')?;
    let close = line[open..].find(']')? + open;
    let client = line[open + 1..close].to_owned();
    let mut args = Vec::new();
    let bytes = line[close + 1..].as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        i += 1;
        let mut arg = Vec::new();
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 1;
                match bytes[i] {
                    b'n' => arg.push(b'\n'),
                    b'r' => arg.push(b'\r'),
                    b't' => arg.push(b'\t'),
                    b'a' => arg.push(0x07),
                    b'b' => arg.push(0x08),
                    b'x' => {
                        let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                        arg.push(u8::from_str_radix(hex, 16).ok()?);
                        i += 2;
                    }
                    other => arg.push(other),
                }
            } else {
                arg.push(bytes[i]);
            }
            i += 1;
        }
        args.push(arg);
        i += 1;
    }
    Some((client, args))
}

/// 서버의 `MONITOR` 를 따로 연결로 열어 받은 줄을 모은다. `stop` 이 든 줄을 받으면 끝난다.
fn monitor(stop: &'static str) -> (std::thread::JoinHandle<()>, Arc<Mutex<Vec<String>>>) {
    let ep = endpoint(LiveDb::Redis, &LiveDb::Redis.url()).unwrap();
    let mut stream = TcpStream::connect((ep.host.as_str(), ep.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if let Some(password) = &ep.password {
        let p = password.as_bytes();
        write!(stream, "*2\r\n$4\r\nAUTH\r\n${}\r\n", p.len()).unwrap();
        stream.write_all(p).unwrap();
        stream.write_all(b"\r\n").unwrap();
        reader.read_line(&mut line).unwrap();
        assert!(line.starts_with("+OK"), "{line}");
        line.clear();
    }
    stream.write_all(b"*1\r\n$7\r\nMONITOR\r\n").unwrap();
    reader.read_line(&mut line).unwrap();
    assert!(line.starts_with("+OK"), "{line}");
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let handle = std::thread::spawn(move || {
        let _stream = stream;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let done = line.contains(stop);
            sink.lock().unwrap().push(line.trim_end().to_owned());
            if done {
                return;
            }
        }
    });
    (handle, lines)
}

/// 연결을 열 때의 설정 명령(창의 연결과 같은 드라이버 열기 길 — 메인 판정 2026-11-17, 16-m5-plan.md M5-19 ③).
fn connection_setup(args: &[Vec<u8>]) -> bool {
    let word = |i: usize| {
        args.get(i)
            .map(|a| String::from_utf8_lossy(a).to_ascii_uppercase())
    };
    match word(0).as_deref() {
        Some("SELECT" | "AUTH" | "HELLO") => true,
        Some("CLIENT") => word(1).as_deref() == Some("SETINFO"),
        Some("INFO") => word(1).as_deref() == Some("SERVER") && args.len() == 2,
        _ => false,
    }
}

/// ③ 툴이 서버에 보낸 명령은 연결 설정 명령 밖은 모두 분류기가 확인 없는 조회(`Allowed`)로 매기는 것이다 — 서버의 `MONITOR` 로
/// 모아 판정한다. 키 이름 · `match` 패턴에 줄바꿈 · RESP 조각 · 따옴표를 넣어도 그 글은 인자 하나로만 가고(명령이 되지 않는다), 서버의
/// 키는 그대로이며 그 키를 이름 그대로 읽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_redis_kv_tool_sends_only_reads_with_names_as_arguments() {
    const STOP: &str = "m519:monitor:stop";
    let injected = [
        "m519:inj:a\r\nFLUSHDB\r\n",
        "m519:inj:*1\r\n$8\r\nFLUSHALL\r\n",
        "m519:inj:\" ; DEL m519:inj:canary \"",
        "m519:inj:x SET m519:inj:canary hacked",
    ];
    let mut admin = admin().await;
    run(&mut admin, &["SET", "m519:inj:canary", "safe"]).await;
    for (i, key) in injected.iter().enumerate() {
        run(&mut admin, &["SET", key, &format!("value{i}")]).await;
    }
    run(&mut admin, &["DEL", "m519:inj:list"]).await;
    run(&mut admin, &["RPUSH", "m519:inj:list", "a", "b"]).await;
    let mut e = env().await;
    let (handle, lines) = monitor(STOP);

    let patterns = [
        "m519:inj:*".to_owned(),
        "m519:inj:*\r\nFLUSHDB\r\n*".to_owned(),
        "m519:inj:\" FLUSHALL \"*".to_owned(),
    ];
    let mut found = Vec::new();
    for pattern in &patterns {
        let mut cursor: Option<String> = None;
        loop {
            let mut extra = json!({ "match": pattern, "limit": 500 });
            if let Some(c) = &cursor {
                extra["cursor"] = json!(c);
            }
            let page = e.call("db.tool.kvScan", extra).await;
            for k in page["keys"].as_array().unwrap() {
                found.push(k["key"].as_str().unwrap().to_owned());
            }
            match page["nextCursor"].as_str() {
                Some(c) => cursor = Some(c.to_owned()),
                None => break,
            }
        }
    }
    for key in injected {
        assert!(found.iter().any(|k| k == key), "{key:?} not scanned");
    }
    for (i, key) in injected.iter().enumerate() {
        let page = e.call("db.tool.kvGet", json!({ "key": key })).await;
        assert_eq!(
            page["value"]["string"]["text"],
            format!("value{i}"),
            "{key:?}"
        );
    }
    let list = e
        .call("db.tool.kvGet", json!({ "key": "m519:inj:list" }))
        .await;
    assert_eq!(list["total"], 2);
    // MONITOR 를 끝낸다(관문을 거치지 않는 판정용 연결).
    run(&mut admin, &["ECHO", STOP]).await;
    handle.join().unwrap();

    let canary = run(&mut admin, &["GET", "m519:inj:canary"]).await;
    assert!(
        matches!(canary, KvReply::Bulk(ref v) if serde_json::to_value(v).unwrap()["text"] == "safe"),
        "{canary:?}"
    );
    for key in injected {
        let exists = run(&mut admin, &["EXISTS", key]).await;
        assert!(matches!(exists, KvReply::Int(_)), "{exists:?}");
    }

    let lines = lines.lock().unwrap().clone();
    let parsed: Vec<(String, Vec<Vec<u8>>)> =
        lines.iter().filter_map(|l| monitor_line(l)).collect();
    // 툴이 연 연결: 이 시험의 접두사가 든 명령을 보낸 주소 가운데 판정용 연결(ECHO · 준비)이 아닌 것.
    let admin_clients: Vec<&String> = parsed
        .iter()
        .filter(|(_, args)| {
            args.first()
                .is_some_and(|a| a.eq_ignore_ascii_case(b"ECHO"))
        })
        .map(|(c, _)| c)
        .collect();
    let tool_clients: std::collections::HashSet<&String> = parsed
        .iter()
        .filter(|(c, args)| {
            !admin_clients.contains(&c)
                && args.iter().any(|a| a.windows(9).any(|w| w == b"m519:inj:"))
        })
        .map(|(c, _)| c)
        .collect();
    assert!(tool_clients.len() >= patterns.len(), "{tool_clients:?}");
    let sent: Vec<&Vec<Vec<u8>>> = parsed
        .iter()
        .filter(|(c, _)| tool_clients.contains(c))
        .map(|(_, args)| args)
        .collect();
    let mut names = std::collections::BTreeSet::new();
    for args in &sent {
        if connection_setup(args) {
            continue;
        }
        names.insert(String::from_utf8_lossy(&args[0]).to_ascii_uppercase());
        assert_eq!(
            classify_argv(args),
            KvCommandClass::Allowed,
            "{:?}",
            args.iter()
                .map(|a| String::from_utf8_lossy(a))
                .collect::<Vec<_>>()
        );
    }
    for needed in ["SCAN", "TYPE", "PTTL", "MEMORY", "GETRANGE", "LRANGE"] {
        assert!(names.contains(needed), "{needed} not seen in {names:?}");
    }
    // 이름 · 패턴은 인자 하나로 갔다 — 줄바꿈 · RESP 조각이 든 키가 TYPE 의 인자 그대로 있고, 패턴이 MATCH 의 인자 그대로 있다.
    for key in injected {
        assert!(
            sent.iter().any(|a| a.len() == 2
                && a[0].eq_ignore_ascii_case(b"TYPE")
                && a[1] == key.as_bytes()),
            "{key:?}"
        );
    }
    for pattern in &patterns {
        assert!(
            sent.iter().any(|a| a[0].eq_ignore_ascii_case(b"SCAN")
                && a.windows(2)
                    .any(|w| w[0].eq_ignore_ascii_case(b"MATCH") && w[1] == pattern.as_bytes())),
            "{pattern:?}"
        );
    }
}
