//! 실 Redis 시험(14-m3-plan.md M3-09 완료 기준 ① ~ ⑦). `PW_TEST_REDIS_URL` 이 있을 때만 목록에 있다(`pw_db_driver::live`).
//!
//! 시험마다 DB 번호를 따로 쓴다 — nextest 는 시험마다 프로세스를 띄워 함께 돌리므로 같은 DB 를 지우면 서로를 깬다. 쓰는 번호:
//! 0 · 2(⑤), 1(① 키 100만 개 — 이미 심겨 있으면 다시 심지 않는다), 3 ~ 9(나머지). Redis 의 기본 DB 수는 16 이다.
//!
//! TLS 시험(⑦)은 `PW_TEST_REDIS_TLS_URL`(`rediss://localhost:6380/0`)과 `PW_TEST_TLS_CA`(시험 CA 의 PEM 경로)를 읽는다 —
//! `scripts/db-tls.mjs` 의 redis 항목이 띄운다(CI 는 `db-live` 잡 「tls test servers」 단계, 로컬은 `mise run test-db`). 없으면
//! 건너뛰지 않고 실패한다.

use std::time::{Duration, Instant};

use pw_db_driver::live::{LiveDb, endpoint};
use pw_db_driver::{DbConnection, DbDriver, KvConnection};
use pw_db_sql::kv_command::{
    BlockReason, KvCommandClass, classify_argv, classify_command, parse_command_argv,
};
use pw_db_types::{
    ConnectionSpec, DbErrorKind, DbValue, DriverKind, KvKey, KvRange, KvRenameOutcome, KvReply,
    KvScanRequest, KvType, KvValue, Secret, SslMode, Target, TlsSettings, TlsStatus,
};
use redis::AsyncCommands;

use crate::{PAGE_BUDGET, RedisDriver};

/// `PW_TEST_REDIS_URL` 의 서버, DB `db`, 평문.
fn spec(db: u32) -> ConnectionSpec {
    let url = LiveDb::Redis.url();
    let ep = endpoint(LiveDb::Redis, &url).unwrap();
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
    spec
}

async fn open(spec: ConnectionSpec) -> Box<dyn KvConnection> {
    match RedisDriver::new().connect(spec).await.unwrap().connection {
        DbConnection::Kv(conn) => conn,
        DbConnection::Sql(_) => unreachable!("redis is a kv driver"),
    }
}

/// 시험 준비용 redis-rs 연결(파이프라인으로 많이 심는다). 드라이버를 거치지 않는다. redis-rs `Client` 의 기본 응답 타임아웃
/// (500 ms)은 함께 도는 시험의 부하에서 10 MiB 파이프라인을 끊으므로 늘린다.
async fn raw(db: u32) -> redis::aio::MultiplexedConnection {
    let ep = endpoint(LiveDb::Redis, &LiveDb::Redis.url()).unwrap();
    let auth = ep.password.map(|p| format!(":{p}@")).unwrap_or_default();
    let host = if ep.host.contains(':') {
        format!("[{}]", ep.host)
    } else {
        ep.host
    };
    redis::Client::open(format!("redis://{auth}{host}:{}/{db}", ep.port))
        .unwrap()
        .get_multiplexed_async_connection_with_config(
            &redis::AsyncConnectionConfig::new()
                .set_connection_timeout(Some(Duration::from_secs(10)))
                .set_response_timeout(Some(Duration::from_secs(120))),
        )
        .await
        .unwrap()
}

fn argv(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

/// ① 키 100만 개에서 스캔 첫 묶음(1,000키 + 타입 · TTL · 크기)이 200 ms 안.
#[tokio::test(flavor = "multi_thread")]
async fn live_redis_the_first_scan_batch_of_a_million_keys_is_fast() {
    const KEYS: u64 = 1_000_000;
    let mut seed = raw(1).await;
    let size: u64 = redis::cmd("DBSIZE").query_async(&mut seed).await.unwrap();
    if size != KEYS {
        let _: () = redis::cmd("FLUSHDB").query_async(&mut seed).await.unwrap();
        for chunk in 0..(KEYS / 10_000) {
            let mut pipe = redis::pipe();
            for i in chunk * 10_000..(chunk + 1) * 10_000 {
                if i % 10 == 0 {
                    pipe.cmd("SET")
                        .arg(format!("k:{i}"))
                        .arg(i)
                        .arg("EX")
                        .arg(86_400)
                        .ignore();
                } else {
                    pipe.cmd("SET").arg(format!("k:{i}")).arg(i).ignore();
                }
            }
            let _: () = pipe.query_async(&mut seed).await.unwrap();
        }
    }
    let size: u64 = redis::cmd("DBSIZE").query_async(&mut seed).await.unwrap();
    assert_eq!(size, KEYS);

    let mut conn = open(spec(1)).await;
    let mut best = Duration::MAX;
    let mut page = None;
    // 다섯 번 재고 가장 빠른 것으로 판정한다(첫 호출의 소켓 · 할당 준비가 섞이지 않게). 다섯 번 모두 같은 일을 한다.
    for _ in 0..5 {
        let started = Instant::now();
        let got = conn
            .scan(KvScanRequest {
                pattern: None,
                cursor: None,
                count: 1_000,
            })
            .await
            .unwrap();
        best = best.min(started.elapsed());
        page = Some(got);
    }
    let page = page.unwrap();
    println!(
        "first scan batch: {} keys, best of 5 {best:?}",
        page.keys.len()
    );
    assert!(page.keys.len() >= 1_000, "{}", page.keys.len());
    assert!(page.next_cursor.is_some());
    assert!(
        page.keys
            .iter()
            .all(|k| k.kind == KvType::String && k.size.is_some())
    );
    assert!(page.keys.iter().any(|k| k.ttl_ms.is_some()));
    assert!(page.keys.iter().any(|k| k.ttl_ms.is_none()));
    assert!(best < Duration::from_millis(200), "{best:?}");

    // 리뷰 5: 드라이버가 묶음 크기를 묶는다 — `count` 를 아무리 크게 줘도 한 묶음은 `MAX_SCAN_COUNT` 근처다(묶지 않으면 키 공간
    // 전체 + 키당 세 명령 파이프라인).
    let huge = conn
        .scan(KvScanRequest {
            pattern: None,
            cursor: None,
            count: u32::MAX,
        })
        .await
        .unwrap();
    assert!(
        huge.keys.len() <= 2 * crate::MAX_SCAN_COUNT as usize,
        "{}",
        huge.keys.len()
    );
}

/// ② 분류 표(완료 기준) + 서버가 아는 명령의 플래그와의 대조 — 허용 목록의 명령에 `write` · `blocking` 플래그가 없고,
/// 서버가 `blocking` 이라 부르는 명령은 모두 거절이다(`XREAD` · `XREADGROUP` 은 `BLOCK` 인자가 있을 때만 — 인자 규칙).
#[tokio::test]
async fn live_redis_the_classifier_agrees_with_the_server_s_command_flags() {
    for text in [
        "KEYS *",
        "FLUSHALL",
        "CONFIG SET maxmemory 1",
        "EVAL \"return 1\" 0",
        "SELECT 2",
        "PITWALL.NOSUCH x",
    ] {
        let class = classify_argv(&parse_command_argv(text).unwrap());
        assert!(matches!(class, KvCommandClass::Blocked(_)), "{text}");
    }
    for text in ["DEL a", "RENAME a b"] {
        let class = classify_argv(&parse_command_argv(text).unwrap());
        assert!(matches!(class, KvCommandClass::Confirm(_)), "{text}");
    }

    let mut conn = raw(3).await;
    let docs: redis::Value = redis::cmd("COMMAND").query_async(&mut conn).await.unwrap();
    let redis::Value::Array(commands) = docs else {
        panic!("COMMAND is a list")
    };
    let mut checked = 0;
    for command in commands {
        let redis::Value::Array(fields) = command else {
            continue;
        };
        let name = match &fields[0] {
            redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_ascii_uppercase(),
            redis::Value::SimpleString(s) => s.to_ascii_uppercase(),
            other => panic!("command name {other:?}"),
        };
        let redis::Value::Array(flags) = &fields[2] else {
            panic!("flags of {name}")
        };
        let flags: Vec<String> = flags
            .iter()
            .filter_map(|f| match f {
                redis::Value::SimpleString(s) => Some(s.clone()),
                redis::Value::BulkString(b) => Some(String::from_utf8_lossy(b).into_owned()),
                _ => None,
            })
            .collect();
        let class = classify_command(&name);
        if class == KvCommandClass::Allowed {
            assert!(!flags.iter().any(|f| f == "write"), "{name} {flags:?}");
        }
        if flags.iter().any(|f| f == "blocking") && !matches!(name.as_str(), "XREAD" | "XREADGROUP")
        {
            assert_eq!(
                class,
                KvCommandClass::Blocked(BlockReason::Blocking),
                "{name} {flags:?}"
            );
        }
        checked += 1;
    }
    assert!(checked > 150, "the server listed {checked} commands");
}

/// ③ 응답 타임아웃 1 s: 서버에서 끝없이 막히는 `BLPOP` 이 1 s 뒤 타임아웃이고, 같은 연결의 다음 `PING` 이 곧바로 된다 —
/// 연결을 다시 만들지 않으면 `PING` 이 서버에서 `BLPOP` 뒤에 줄을 서 끝나지 않는다. (드라이버를 직접 부르는 시험이라 분류기가
/// `BLPOP` 을 거절하는 것과 부딪히지 않는다 — 분류는 관문 M3-22 가 한다.)
#[tokio::test]
async fn live_redis_a_response_timeout_drops_the_request_and_the_next_command_runs() {
    let mut spec = spec(4);
    spec.response_timeout_ms = Some(1_000);
    let mut conn = open(spec).await;
    let started = Instant::now();
    let error = conn
        .command(argv("BLPOP pitwall:live:never 0"))
        .await
        .unwrap_err();
    let waited = started.elapsed();
    assert_eq!(error.kind, DbErrorKind::Timeout, "{error}");
    assert!(
        waited >= Duration::from_millis(950) && waited < Duration::from_millis(2_500),
        "{waited:?}"
    );
    let started = Instant::now();
    let pong = conn.command(argv("PING")).await.unwrap();
    assert_eq!(pong, KvReply::Status("PONG".into()));
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
}

/// 취소: 다른 태스크의 인터럽트가 진행 중인 요청을 버리고(취소 오류), 같은 연결의 다음 명령이 된다. 도는 요청이 없을 때의
/// 인터럽트는 아무것도 하지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn live_redis_an_interrupt_cancels_the_request_and_the_connection_recovers() {
    let mut conn = open(spec(4)).await;
    let interrupt = conn.interrupter();
    interrupt.interrupt().await.unwrap();
    assert_eq!(
        conn.command(argv("PING")).await.unwrap(),
        KvReply::Status("PONG".into())
    );

    let later = interrupt.clone();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        later.interrupt().await.unwrap();
    });
    let started = Instant::now();
    let error = conn
        .command(argv("BLPOP pitwall:live:never-too 0"))
        .await
        .unwrap_err();
    canceller.await.unwrap();
    assert_eq!(error.kind, DbErrorKind::Cancelled, "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let started = Instant::now();
    assert_eq!(
        conn.command(argv("PING")).await.unwrap(),
        KvReply::Status("PONG".into())
    );
    assert!(started.elapsed() < Duration::from_millis(900));
}

/// ④ 4 MiB 를 넘는 hash 가 쪽으로 나뉜다: 필드 40개 × 256 KiB = 10 MiB 가 세 쪽 이상이고, 쪽마다 예산의 1.25 배 안이며,
/// 쪽을 다 모으면 필드 40개가 한 번씩 나온다.
#[tokio::test]
async fn live_redis_a_hash_over_the_budget_is_read_in_pages() {
    const FIELDS: usize = 40;
    const FIELD_SIZE: usize = 256 * 1024;
    let mut seed = raw(5).await;
    let _: () = redis::cmd("FLUSHDB").query_async(&mut seed).await.unwrap();
    let mut pipe = redis::pipe();
    for i in 0..FIELDS {
        let value = vec![b'a' + (i % 26) as u8; FIELD_SIZE];
        pipe.hset("big", format!("f{i:02}"), value).ignore();
    }
    let _: () = pipe.query_async(&mut seed).await.unwrap();

    let mut conn = open(spec(5)).await;
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = conn
            .read(
                KvKey::Text("big".into()),
                KvRange {
                    cursor: cursor.clone(),
                    count: 1_000,
                },
            )
            .await
            .unwrap();
        pages += 1;
        assert_eq!(page.total, Some(FIELDS as u64));
        let KvValue::Hash(fields) = page.value else {
            panic!("hash")
        };
        let bytes: usize = fields
            .iter()
            .map(|f| match (&f.field, &f.value) {
                (DbValue::Text(k), DbValue::Text(v)) => k.len() + v.len(),
                other => panic!("{other:?}"),
            })
            .sum();
        println!("page {pages}: {} fields, {bytes} bytes", fields.len());
        // 예산은 「닿으면 멈춘다」 다 — 넘는 것은 마지막 응답 하나(`HSCAN` 의 `COUNT` 는 근사)뿐이다(`browse` 머리 주석).
        assert!(
            bytes <= PAGE_BUDGET + PAGE_BUDGET / 4,
            "page {pages}: {bytes}"
        );
        for field in fields {
            let DbValue::Text(name) = field.field else {
                unreachable!()
            };
            seen.insert(name);
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(pages < 50, "the pages do not end");
    }
    assert!(pages >= 3, "{pages} pages for 10 MiB");
    assert_eq!(seen.len(), FIELDS);
}

/// 타입마다 값 읽기: string 창(UTF-8 글자를 자르지 않는다) · list · set · zset · stream, 없는 키는 `Missing`.
#[tokio::test]
async fn live_redis_each_type_reads_in_pages() {
    let mut seed = raw(6).await;
    let _: () = redis::cmd("FLUSHDB").query_async(&mut seed).await.unwrap();
    let _: () = seed.set("s", "가나다라마").await.unwrap(); // 글자마다 3바이트
    let _: () = seed
        .rpush("l", (0..25).collect::<Vec<i32>>())
        .await
        .unwrap();
    let _: () = seed
        .sadd("set", (0..25).collect::<Vec<i32>>())
        .await
        .unwrap();
    let members: Vec<(f64, String)> = (0..25).map(|i| (f64::from(i), format!("m{i}"))).collect();
    let _: () = seed.zadd_multiple("z", &members).await.unwrap();
    for i in 0..25 {
        let _: String = redis::cmd("XADD")
            .arg("x")
            .arg("*")
            .arg("n")
            .arg(i)
            .query_async(&mut seed)
            .await
            .unwrap();
    }

    let mut conn = open(spec(6)).await;
    // string: 창 4바이트 — 첫 쪽은 둘째 글자 앞에서 끊는다.
    let page = conn
        .read(
            KvKey::Text("s".into()),
            KvRange {
                cursor: None,
                count: 4,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.value, KvValue::String(DbValue::Text("가".into())));
    assert_eq!(page.total, Some(15));
    assert_eq!(page.next_cursor.as_deref(), Some("3"));
    let rest = conn
        .read(
            KvKey::Text("s".into()),
            KvRange {
                cursor: page.next_cursor,
                count: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rest.value,
        KvValue::String(DbValue::Text("나다라마".into()))
    );
    assert_eq!(rest.next_cursor, None);

    // 컬렉션: 쪽 10개씩, 다 모으면 25개.
    for (key, expect) in [
        ("l", "list"),
        ("set", "set"),
        ("z", "zset"),
        ("x", "stream"),
    ] {
        let mut cursor = None;
        let mut items = 0;
        let mut pages = 0;
        loop {
            let page = conn
                .read(
                    KvKey::Text(key.into()),
                    KvRange {
                        cursor: cursor.clone(),
                        count: 10,
                    },
                )
                .await
                .unwrap();
            pages += 1;
            assert_eq!(page.total, Some(25), "{expect}");
            items += match page.value {
                KvValue::List(v) | KvValue::Set(v) => v.len(),
                KvValue::Zset(v) => v.len(),
                KvValue::Stream(v) => v.len(),
                other => panic!("{expect}: {other:?}"),
            };
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
            assert!(pages < 20, "{expect} pages do not end");
        }
        // set 은 SSCAN 이라 작은 집합(listpack)이면 한 번에 다 온다 — 개수만 본다.
        assert_eq!(items, 25, "{expect}");
        if expect != "set" {
            assert!(pages >= 3, "{expect}: {pages} pages");
        }
    }
    // 리뷰 6: 창이 보낸 커서가 끝 값이어도 넘치지 않고 요청 오류다(debug 빌드에서 panic 하지 않는다).
    for key in ["s", "l", "z"] {
        let error = conn
            .read(
                KvKey::Text(key.into()),
                KvRange {
                    cursor: Some(u64::MAX.to_string()),
                    count: 10,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, DbErrorKind::Invalid, "{key}: {error}");
    }
    let missing = conn
        .read(
            KvKey::Text("nope".into()),
            KvRange {
                cursor: None,
                count: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(missing.value, KvValue::Missing);
}

/// ⑤ DB 0 연결과 DB 2 연결이 서로의 키를 보지 않는다.
#[tokio::test]
async fn live_redis_db_0_and_db_2_do_not_see_each_other() {
    let stamp = std::process::id();
    let in0 = format!("pitwall:live:db0:{stamp}");
    let in2 = format!("pitwall:live:db2:{stamp}");
    let mut zero = open(spec(0)).await;
    let mut two = open(spec(2)).await;
    zero.command(vec!["SET".into(), in0.clone(), "0".into()])
        .await
        .unwrap();
    two.command(vec!["SET".into(), in2.clone(), "2".into()])
        .await
        .unwrap();
    assert_eq!(
        zero.command(vec!["GET".into(), in2.clone()]).await.unwrap(),
        KvReply::Nil
    );
    assert_eq!(
        two.command(vec!["GET".into(), in0.clone()]).await.unwrap(),
        KvReply::Nil
    );
    let found = |page: pw_db_types::KvScanPage| {
        page.keys.into_iter().map(|k| k.key).collect::<Vec<KvKey>>()
    };
    let scan = |pattern: String| KvScanRequest {
        pattern: Some(pattern),
        cursor: None,
        count: 1_000,
    };
    let mut zero_keys = Vec::new();
    let mut request = scan(format!("pitwall:live:db*:{stamp}"));
    loop {
        let page = zero.scan(request.clone()).await.unwrap();
        let next = page.next_cursor.clone();
        zero_keys.extend(found(page));
        match next {
            Some(cursor) => request.cursor = Some(cursor),
            None => break,
        }
    }
    assert_eq!(zero_keys, vec![KvKey::Text(in0.clone())]);
    assert_eq!(two.key_info(KvKey::Text(in0.clone())).await.unwrap(), None);
    assert!(
        two.key_info(KvKey::Text(in2.clone()))
            .await
            .unwrap()
            .is_some()
    );
    zero.delete(vec![KvKey::Text(in0)]).await.unwrap();
    two.delete(vec![KvKey::Text(in2)]).await.unwrap();
}

/// ⑥ `RENAMENX` 의 대상이 있으면 덮지 않고 「덮어쓰기 확인」 을 돌려준다. 덮어쓰기를 받으면 바꾼다. read-only 연결은 바꾸지 않는다.
#[tokio::test]
async fn live_redis_rename_does_not_overwrite_without_confirmation() {
    let mut conn = open(spec(7)).await;
    conn.command(argv("FLUSHDB")).await.unwrap();
    conn.command(argv("SET from 1")).await.unwrap();
    conn.command(argv("SET to 2")).await.unwrap();
    let outcome = conn
        .rename(KvKey::Text("from".into()), KvKey::Text("to".into()), false)
        .await
        .unwrap();
    assert_eq!(outcome, KvRenameOutcome::TargetExists);
    assert_eq!(
        conn.command(argv("GET to")).await.unwrap(),
        KvReply::Bulk(DbValue::Text("2".into()))
    );
    assert_eq!(
        conn.command(argv("GET from")).await.unwrap(),
        KvReply::Bulk(DbValue::Text("1".into()))
    );

    let mut read_only = spec(7);
    read_only.read_only = true;
    let mut viewer = open(read_only).await;
    let refused = viewer
        .rename(KvKey::Text("from".into()), KvKey::Text("to".into()), true)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, DbErrorKind::ReadOnly);

    let outcome = conn
        .rename(KvKey::Text("from".into()), KvKey::Text("to".into()), true)
        .await
        .unwrap();
    assert_eq!(outcome, KvRenameOutcome::Renamed);
    assert_eq!(
        conn.command(argv("GET to")).await.unwrap(),
        KvReply::Bulk(DbValue::Text("1".into()))
    );
    let outcome = conn
        .rename(KvKey::Text("free".into()), KvKey::Text("x".into()), false)
        .await;
    assert_eq!(
        outcome.unwrap_err().kind,
        DbErrorKind::Server,
        "no such key"
    );
}

/// TTL 읽기 · 바꾸기 · 지우기와 키 삭제. 바이트 키(UTF-8 아님)도 그대로 다룬다. 서버의 오류 응답은 명령의 응답이다.
#[tokio::test]
async fn live_redis_ttls_deletes_and_error_replies() {
    let mut conn = open(spec(8)).await;
    conn.command(argv("FLUSHDB")).await.unwrap();
    conn.command(argv("SET plain v")).await.unwrap();
    let key = KvKey::Text("plain".into());
    assert_eq!(
        conn.key_info(key.clone()).await.unwrap().unwrap().ttl_ms,
        None
    );
    assert!(conn.set_ttl(key.clone(), 60_000).await.unwrap());
    let ttl = conn
        .key_info(key.clone())
        .await
        .unwrap()
        .unwrap()
        .ttl_ms
        .unwrap();
    assert!(ttl > 50_000 && ttl <= 60_000, "{ttl}");
    assert!(conn.persist(key.clone()).await.unwrap());
    assert!(!conn.persist(key.clone()).await.unwrap(), "no TTL left");
    assert!(!conn.set_ttl(KvKey::Text("absent".into()), 1).await.unwrap());

    let mut seed = raw(8).await;
    let binary = vec![0xff_u8, 0x00, b'k'];
    let _: () = seed.set(&binary, "b").await.unwrap();
    let page = conn
        .scan(KvScanRequest {
            pattern: None,
            cursor: None,
            count: 100,
        })
        .await
        .unwrap();
    let binary_key = KvKey::from_bytes(&binary);
    assert!(page.keys.iter().any(|k| k.key == binary_key));
    assert_eq!(
        conn.delete(vec![binary_key, key, KvKey::Text("absent".into())])
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        conn.command(argv("DBSIZE")).await.unwrap(),
        KvReply::Int(DbValue::int(0))
    );

    let wrong = conn.command(argv("NOSUCHCOMMAND")).await.unwrap();
    let KvReply::Error(text) = wrong else {
        panic!("{wrong:?}")
    };
    assert!(text.starts_with("ERR"), "{text}");
    // 오류 응답 뒤에도 연결은 그대로다.
    assert_eq!(
        conn.command(argv("PING")).await.unwrap(),
        KvReply::Status("PONG".into())
    );
}

/// ⑦ TLS: `verify-full` 은 OS 신뢰 저장소에 없는 시험 CA 의 서버를 거절하고, 시험 CA 를 주입하면 붙어 「검증함」 이다. 이름이
/// 틀리면 CA 가 있어도 거절이다(터널처럼 `connect_to` 로 붙고 TLS 이름은 설정의 호스트). `require` 는 검증 없이 붙는다.
#[tokio::test]
async fn live_redis_tls_verify_full_trusts_only_the_given_ca() {
    let url = std::env::var("PW_TEST_REDIS_TLS_URL")
        .expect("PW_TEST_REDIS_TLS_URL is not set (CI: the 'tls test servers' step)");
    let ca = std::env::var("PW_TEST_TLS_CA").expect("PW_TEST_TLS_CA is not set");
    let ep = endpoint(LiveDb::Redis, &url).unwrap();
    let tls_spec = |mode: SslMode, root: bool, host: &str| {
        let mut spec = ConnectionSpec::new(
            DriverKind::Redis.id(),
            Target::Network {
                host: host.to_owned(),
                port: ep.port,
                connect_to: Some(pw_db_types::SocketTarget {
                    host: ep.host.clone(),
                    port: ep.port,
                }),
            },
        );
        spec.tls = TlsSettings::mode(mode);
        if root {
            spec.tls.root_cert = Some(ca.clone().into());
        }
        spec.password = ep.password.clone().map(Secret::new);
        spec.connect_timeout_ms = 5_000;
        spec
    };
    let driver = RedisDriver::new();

    // 서버는 시험 직전에 뜰 수 있다(`mise run test-db` 가 띄운다). 준비 확인(`wait_ready`)은 평문 PING 이라 TLS 만 받는 포트에는 못
    // 쓴다 — 검증 없는 TLS 연결이 될 때까지 기다린다.
    let started = Instant::now();
    while let Err(error) = driver
        .connect(tls_spec(SslMode::Require, false, "localhost"))
        .await
    {
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "the TLS server is not ready: {error}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let refused = driver
        .connect(tls_spec(SslMode::VerifyFull, false, "localhost"))
        .await
        .err()
        .expect("an unknown CA is refused");
    assert_eq!(refused.kind, DbErrorKind::Tls, "{refused}");

    let wrong_name = driver
        .connect(tls_spec(SslMode::VerifyFull, true, "cache.invalid"))
        .await
        .err()
        .expect("a name outside the certificate is refused");
    assert_eq!(wrong_name.kind, DbErrorKind::Tls, "{wrong_name}");

    let connected = driver
        .connect(tls_spec(SslMode::VerifyFull, true, "localhost"))
        .await
        .unwrap();
    assert_eq!(connected.server.tls, TlsStatus::Verified);
    let DbConnection::Kv(mut conn) = connected.connection else {
        unreachable!()
    };
    assert_eq!(
        conn.command(argv("PING")).await.unwrap(),
        KvReply::Status("PONG".into())
    );

    let unverified = driver
        .connect(tls_spec(SslMode::Require, false, "localhost"))
        .await
        .unwrap();
    assert_eq!(unverified.server.tls, TlsStatus::Unverified);

    // 평문 서버(`PW_TEST_REDIS_URL`)에 TLS 를 요구하면 핸드셰이크에서 끊긴다(평문으로 떨어지지 않는다).
    let mut plain = spec(9);
    plain.tls = TlsSettings::mode(SslMode::Require);
    plain.connect_timeout_ms = 5_000;
    let error = driver
        .connect(plain)
        .await
        .err()
        .expect("no plaintext fallback");
    assert!(
        matches!(error.kind, DbErrorKind::Tls | DbErrorKind::Timeout),
        "{error}"
    );
}

/// 연결 테스트(`DbDriver::test`)가 서버 판본과 TLS 상태를 돌려준다.
#[tokio::test]
async fn live_redis_the_connection_test_reports_the_server_version() {
    let server = RedisDriver::new().test(spec(9)).await.unwrap();
    let version = server.version.unwrap();
    assert!(
        version.chars().next().unwrap().is_ascii_digit(),
        "{version}"
    );
    assert_eq!(server.tls, TlsStatus::Plaintext);
}

/// 리뷰 2: 응답 타임아웃 · 취소 뒤 다시 연 연결이 DB 번호와 사용자(AUTH)를 다시 적용한다 — DB 2 · ACL 사용자로 연 연결이 다시
/// 열린 뒤에도 키를 DB 2 에 쓰고(DB 0 에 없다) `ACL WHOAMI` 가 그 사용자다.
#[tokio::test(flavor = "multi_thread")]
async fn live_redis_a_rebuilt_connection_keeps_its_database_and_user() {
    let stamp = std::process::id();
    let user = format!("pitwall-live-{stamp}");
    let password = format!("pw-{stamp}");
    let mut admin = raw(0).await;
    let _: () = redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&user)
        .arg("reset")
        .arg("on")
        .arg(format!(">{password}"))
        .arg("~*")
        .arg("&*")
        .arg("+@all")
        .query_async(&mut admin)
        .await
        .unwrap();

    let mut spec = spec(2);
    spec.user = Some(user.clone());
    spec.password = Some(Secret::new(password));
    spec.response_timeout_ms = Some(1_000);
    let mut conn = open(spec).await;
    let whoami = |reply: KvReply| match reply {
        KvReply::Bulk(DbValue::Text(name)) | KvReply::Status(name) => name,
        other => panic!("{other:?}"),
    };

    let after_timeout = format!("pitwall:live:rebuilt:timeout:{stamp}");
    let error = conn
        .command(argv("BLPOP pitwall:live:never-again 0"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, DbErrorKind::Timeout);
    conn.command(vec!["SET".into(), after_timeout.clone(), "1".into()])
        .await
        .unwrap();
    assert_eq!(
        whoami(conn.command(argv("ACL WHOAMI")).await.unwrap()),
        user
    );

    let after_cancel = format!("pitwall:live:rebuilt:cancel:{stamp}");
    let interrupt = conn.interrupter();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        interrupt.interrupt().await.unwrap();
    });
    let error = conn
        .command(argv("BLPOP pitwall:live:never-again 0"))
        .await
        .unwrap_err();
    canceller.await.unwrap();
    assert_eq!(error.kind, DbErrorKind::Cancelled);
    conn.command(vec!["SET".into(), after_cancel.clone(), "1".into()])
        .await
        .unwrap();
    assert_eq!(
        whoami(conn.command(argv("ACL WHOAMI")).await.unwrap()),
        user
    );

    let mut two = raw(2).await;
    for key in [&after_timeout, &after_cancel] {
        let in_two: bool = redis::cmd("EXISTS")
            .arg(key)
            .query_async(&mut two)
            .await
            .unwrap();
        let in_zero: bool = redis::cmd("EXISTS")
            .arg(key)
            .query_async(&mut admin)
            .await
            .unwrap();
        assert!(in_two && !in_zero, "{key}: db2 {in_two}, db0 {in_zero}");
        let _: () = redis::cmd("DEL")
            .arg(key)
            .query_async(&mut two)
            .await
            .unwrap();
    }
    let _: () = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&user)
        .query_async(&mut admin)
        .await
        .unwrap();
}

/// 서버가 그 명령을 몇 번 돌렸는가(`INFO commandstats` 의 `cmdstat_<name>:calls=`).
async fn calls_of(conn: &mut redis::aio::MultiplexedConnection, name: &str) -> u64 {
    let info: String = redis::cmd("INFO")
        .arg("commandstats")
        .query_async(conn)
        .await
        .unwrap();
    let prefix = format!("cmdstat_{name}:calls=");
    info.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|rest| rest.split(',').next())
        .map_or(0, |n| n.parse().unwrap())
}

/// 리뷰 3 · 메인 판정: 키 하나를 통째로 읽는 명령은 크기를 먼저 물어 상한을 넘으면 서버에 보내지 않고(`KEY_TOO_LARGE`),
/// 묻지 않는 응답은 상한에서 끊어 `Truncated` 로 싣는다. 상한 안의 읽기는 그대로 돈다.
#[tokio::test]
async fn live_redis_whole_key_reads_are_bounded_in_the_console() {
    let mut seed = raw(10).await;
    let _: () = redis::cmd("FLUSHDB").query_async(&mut seed).await.unwrap();
    let mut pipe = redis::pipe();
    for i in 0..6_000 {
        pipe.hset("big", format!("f{i}"), i).ignore();
    }
    for i in 0..10 {
        pipe.hset("small", format!("f{i}"), i).ignore();
    }
    for i in 0..15_000 {
        pipe.sadd("members", i).ignore();
    }
    pipe.set("huge", vec![b'x'; PAGE_BUDGET + 1]).ignore();
    for i in 0..20_000 {
        pipe.rpush("list", i).ignore();
    }
    let _: () = pipe.query_async(&mut seed).await.unwrap();

    let mut conn = open(spec(10)).await;
    let before = calls_of(&mut seed, "hgetall").await;
    for text in [
        "HGETALL big",
        "SMEMBERS members",
        "GET huge",
        "LRANGE list 0 -1",
    ] {
        let error = conn.command(argv(text)).await.unwrap_err();
        assert_eq!(
            error.code.as_deref(),
            Some("KEY_TOO_LARGE"),
            "{text}: {error}"
        );
    }
    assert_eq!(
        calls_of(&mut seed, "hgetall").await,
        before,
        "the refused HGETALL reached the server"
    );
    assert_eq!(calls_of(&mut seed, "smembers").await, 0);

    // 상한 안이면 그대로 돈다(서버에 닿는다).
    let KvReply::Array(small) = conn.command(argv("HGETALL small")).await.unwrap() else {
        panic!("array")
    };
    assert_eq!(small.len(), 20);
    assert_eq!(calls_of(&mut seed, "hgetall").await, before + 1);
    let KvReply::Array(window) = conn.command(argv("LRANGE list 0 99")).await.unwrap() else {
        panic!("array")
    };
    assert_eq!(window.len(), 100);

    // 묻지 않는 명령(여러 키에 걸친 `SUNION`)은 응답 상한에서 끊는다.
    let KvReply::Truncated(cut) = conn.command(argv("SUNION members")).await.unwrap() else {
        panic!("truncated")
    };
    assert_eq!(cut.elements, 15_000);
    let KvReply::Array(shown) = *cut.reply else {
        panic!("array")
    };
    assert_eq!(shown.len(), 10_000);
}
