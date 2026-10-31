//! 연결 편집 다이얼로그(14-m3-plan.md M3-14)가 기대는 Host 쪽: 연결 타임아웃 칸(파일 · RPC · 드라이버 입력, 범위는 Host 가 판정),
//! [연결 테스트] 의 초안이 아무 파일도 쓰지 않는 것(설정 · 확인 기록 · 키체인), `db.ssh.hosts` 의 범위(그 프로젝트 창 + 신뢰한
//! 프로젝트), 연결 타임아웃이 연결을 여는 바깥 상한인 것. `db.driver.list` 는 `driver_list.rs`.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use common::Fixture;
use pw_db::DbSessions;
use pw_db::config::{DbConnectionConfig, DbProjectParams};
use pw_db_driver::fake::FakeDriver;
use pw_db_types::SqlDialect;
use pw_rpc::{ConnectionScope, code as rpc_code};
use serde_json::{Value, json};

fn upsert(f: &Fixture, id: &str, config: Value) -> Value {
    json!({ "projectId": f.id, "connectionId": id, "config": config })
}

fn local_pg(extra: Value) -> Value {
    let mut config = json!({ "name": "dev", "driver": "postgres", "host": "localhost" });
    config
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    config
}

/// 연결 타임아웃은 파일의 `connect_timeout`(초) · RPC 의 `connectTimeout` 이고, 드라이버 입력의 `connect_timeout_ms` 가 된다.
/// 적지 않으면 10 s. 저장한 연결 · 초안 모두.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_connect_timeout_round_trips_into_the_driver_spec() {
    let f = Fixture::new(true);
    let mut w = f.window();
    let entry = w
        .call(
            "db.connection.upsert",
            upsert(&f, "slow", local_pg(json!({ "connectTimeout": 30 }))),
        )
        .await
        .unwrap();
    assert_eq!(entry["config"]["connectTimeout"], 30, "{entry}");
    let text = fs::read_to_string(f.db_file("slow.toml")).unwrap();
    assert!(text.contains("connect_timeout = 30\n"), "{text}");

    w.call(
        "db.connection.upsert",
        upsert(&f, "plain", local_pg(json!({}))),
    )
    .await
    .unwrap();
    let text = fs::read_to_string(f.db_file("plain.toml")).unwrap();
    assert!(!text.contains("connect_timeout"), "{text}");

    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    let spec = |id: &str| f.connections.resolve(&gate, id).unwrap().spec;
    assert_eq!(spec("slow").connect_timeout_ms, 30_000);
    assert_eq!(spec("plain").connect_timeout_ms, 10_000);

    let draft: DbConnectionConfig =
        serde_json::from_value(local_pg(json!({ "connectTimeout": 45 }))).unwrap();
    let resolved = f
        .connections
        .resolve_draft(&gate, None, &draft, None)
        .unwrap();
    assert_eq!(resolved.spec.connect_timeout_ms, 45_000);

    // 사람이 파일에서 고친 값도 같은 길로 읽힌다.
    f.edit_db(
        "plain.toml",
        "host = \"localhost\"\n",
        "host = \"localhost\"\nconnect_timeout = 600\n",
    );
    assert_eq!(spec("plain").connect_timeout_ms, 600_000);
}

/// 범위(1 ~ 600 s) 밖은 Host 가 거절한다 — 저장은 invalid params, 파일에 적힌 것은 그 연결의 「설정 오류」.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connect_timeout_out_of_range_is_refused_by_the_host() {
    let f = Fixture::new(true);
    let mut w = f.window();
    for secs in [0, 601, 86_400] {
        let e = w
            .call(
                "db.connection.upsert",
                upsert(&f, "bad", local_pg(json!({ "connectTimeout": secs }))),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, rpc_code::INVALID_PARAMS, "{secs}: {e:?}");
        let detail = e.data.as_ref().and_then(Value::as_str).unwrap_or_default();
        assert!(detail.contains("connect_timeout"), "{e:?}");
    }
    assert!(!f.db_file("bad.toml").exists());
    for secs in [1, 600] {
        w.call(
            "db.connection.upsert",
            upsert(&f, "ok", local_pg(json!({ "connectTimeout": secs }))),
        )
        .await
        .unwrap();
    }
    f.write_db(
        "hand.toml",
        "schema = 1\nname = \"hand\"\ndriver = \"postgres\"\nhost = \"localhost\"\nconnect_timeout = 0\n",
    );
    let list = w
        .call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    let hand = list["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "hand")
        .unwrap();
    assert_eq!(hand["state"]["kind"], "invalid", "{hand}");
}

/// 폴더 아래 파일마다 (크기, 바뀐 시각, 내용).
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, Vec<u8>)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.map(Result::unwrap) {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path).unwrap_or_default();
                out.insert(path, (meta.len(), meta.modified().unwrap(), bytes));
            }
        }
    }
    out
}

/// [연결 테스트] 는 저장 전 초안으로 돈다: 새 연결의 초안이든 저장한 연결을 고치는 초안이든, 연결 설정 · 확인 기록(`$DATA`) ·
/// 키체인 어디에도 쓰지 않는다(파일의 크기 · mtime · 내용이 그대로, 새 파일 없음). 입력한 비밀번호는 그 요청에만 쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_draft_connection_test_writes_nothing() {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(pg.clone())).unwrap();
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, sessions);
    });
    let mut w = f.window();
    w.call(
        "db.connection.upsert",
        upsert(
            &f,
            "dev",
            local_pg(json!({ "password": { "kind": "keychain" } })),
        ),
    )
    .await
    .unwrap();
    // 확인 기록 파일도 하나 만들어 둔다(그것도 바뀌지 않아야 한다).
    fs::write(f.root.join("app.db"), b"").unwrap();
    let before_root = snapshot(&f.root);
    let before_app = snapshot(&f.base.join("app"));
    assert!(!before_root.is_empty());

    let typed = "typed-only-for-this-test-9f3c";
    let new_draft =
        local_pg(json!({ "name": "new", "password": { "kind": "keychain" }, "connectTimeout": 5 }));
    let info = w
        .call(
            "db.connection.test",
            json!({ "projectId": f.id, "draft": new_draft, "secret": typed }),
        )
        .await
        .unwrap();
    assert!(info.get("tls").is_some(), "{info}");
    let editing = local_pg(
        json!({ "password": { "kind": "keychain" }, "database": "other", "readOnly": true }),
    );
    w.call(
        "db.connection.test",
        json!({ "projectId": f.id, "connectionId": "dev", "draft": editing, "secret": typed }),
    )
    .await
    .unwrap();

    assert_eq!(snapshot(&f.root), before_root, "the project files changed");
    assert_eq!(
        snapshot(&f.base.join("app")),
        before_app,
        "$DATA/$STATE changed"
    );
    assert_eq!(
        f.store.accounts(),
        Vec::<String>::new(),
        "a draft secret was stored"
    );
    let saved = fs::read_to_string(f.db_file("dev.toml")).unwrap();
    assert!(
        !saved.contains("other") && !saved.contains(typed),
        "{saved}"
    );
}

fn params(f: &Fixture) -> DbProjectParams {
    DbProjectParams {
        project_id: f.id.clone(),
        worktree: None,
    }
}

/// `db.ssh.hosts`: 그 프로젝트 창 + 신뢰한 프로젝트만(메인 판정 — 호스트 이름은 사람의 개인 정보다). 거절은 파일을 읽기 전이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_hosts_are_only_for_a_trusted_project_window() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".ssh")).unwrap();
    fs::write(
        home.path().join(".ssh/config"),
        "Host bastion\n  HostName 10.0.0.1\nHost *\n",
    )
    .unwrap();

    let untrusted = Fixture::build(false, |router, c| {
        pw_db::ssh_hosts::register(router, Arc::clone(c));
    });
    let e = untrusted
        .window()
        .call("db.ssh.hosts", json!({ "projectId": untrusted.id }))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{e:?}");
    assert_eq!(e.data.as_ref().unwrap()["reason"], "untrusted");
    let e = untrusted
        .connections
        .ssh_hosts(
            &ConnectionScope::Project(untrusted.id.clone()),
            &params(&untrusted),
            Some(home.path()),
        )
        .unwrap_err();
    assert_eq!(e.code, rpc_code::PERMISSION_DENIED);

    let f = Fixture::build(true, |router, c| {
        pw_db::ssh_hosts::register(router, Arc::clone(c));
    });
    for scope in [
        ConnectionScope::Host,
        ConnectionScope::Project(f.other.clone()),
    ] {
        let e = f
            .connections
            .ssh_hosts(&scope, &params(&f), Some(home.path()))
            .unwrap_err();
        assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{scope:?}");
        let e = f
            .connect(scope.clone())
            .call("db.ssh.hosts", json!({ "projectId": f.id }))
            .await
            .unwrap_err();
        assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{scope:?}");
    }
    let hosts = f
        .connections
        .ssh_hosts(
            &ConnectionScope::Project(f.id.clone()),
            &params(&f),
            Some(home.path()),
        )
        .unwrap();
    assert_eq!(hosts.hosts, ["bastion"]);
    // 홈을 모르면 빈 목록이다(오류가 아니다).
    let none = f
        .connections
        .ssh_hosts(&ConnectionScope::Project(f.id.clone()), &params(&f), None)
        .unwrap();
    assert!(none.hosts.is_empty());
}

/// 연결이 느린 드라이버(연결 시간 상한을 스스로 지키지 않는다). 연결은 가짜 드라이버에 맡긴다.
struct SlowConnect {
    inner: FakeDriver,
    delay: std::time::Duration,
}

impl pw_db_driver::DbDriver for SlowConnect {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn dialect(&self) -> Option<SqlDialect> {
        self.inner.dialect()
    }

    fn capabilities(&self) -> pw_db_types::Capabilities {
        self.inner.capabilities()
    }

    fn connect(
        &self,
        spec: pw_db_types::ConnectionSpec,
    ) -> pw_db_driver::BoxFuture<'_, pw_db_driver::DbResult<pw_db_driver::Connected>> {
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            self.inner.connect(spec).await
        })
    }
}

/// 연결 타임아웃이 연결을 여는 바깥 상한이다 — 드라이버가 지키지 않아도 그 시간에 끊는다(전에는 10 s 고정이라 그보다 긴 값이 먹지
/// 않았다). 기본(10 s)은 그대로라 1.5 s 걸리는 연결은 붙는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_connect_timeout_bounds_opening_the_connection() {
    let slow = SlowConnect {
        inner: FakeDriver::named("postgres", SqlDialect::Postgres),
        delay: std::time::Duration::from_millis(1500),
    };
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(slow)).unwrap();
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, sessions);
    });
    let mut w = f.window();
    let test = |timeout: Value| json!({ "projectId": f.id, "draft": local_pg(json!({ "connectTimeout": timeout })) });
    let started = std::time::Instant::now();
    let e = w
        .call("db.connection.test", test(json!(1)))
        .await
        .unwrap_err();
    let took = started.elapsed();
    assert_eq!(e.code, pw_db::gate::code::DB_ERROR, "{e:?}");
    assert_eq!(
        e.data.as_ref().unwrap()["error"]["kind"],
        "timeout",
        "{e:?}"
    );
    assert!(took < std::time::Duration::from_millis(1400), "{took:?}");

    let info = w
        .call("db.connection.test", test(Value::Null))
        .await
        .unwrap();
    assert_eq!(info["version"], "fake 1.0", "{info}");
}
