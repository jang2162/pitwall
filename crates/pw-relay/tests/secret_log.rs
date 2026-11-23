//! 18-m7-plan M7-12 시험 ③: Relay 가 붙은 Desktop 에게서 받은 비밀이 로그 · `$STATE`(그리고 Relay 의 다른 뿌리 `$DATA` ·
//! `$CONFIG` · `$CACHE`) 어느 파일에도 없다. 실제 Relay 조립(`pw_relay::host::assemble` — 내장 DB 드라이버)에 데몬과 같은 로그
//! 구독자(`pw_diag::logging`)를 가장 낮은 수준으로 전역에 설치하고, 비밀이 지나는 길(창이 넣기 → Desktop 에 두기 청, 세션 열기 ·
//! 연결 시험 → Desktop 에 읽기 청 → 드라이버 연결 실패)을 돈 뒤 Relay 의 모든 파일과 응답을 grep 한다. 전역 구독자라 이 파일에는
//! 시험이 하나다.

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use pw_core::Host;
use pw_core::config::AppPaths;
use pw_diag::logging::{LogConfig, subscriber};
use pw_relay::config::RelayConfig;
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};
use tracing::level_filters::LevelFilter;

const PLANTED: &str = "PLANTED-relay-9d2b71";

async fn call(conn: &mut pw_core::Connection, id: i64, method: &str, params: Value) -> Value {
    conn.sink
        .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), conn.source.recv())
            .await
            .expect("no reply")
            .expect("closed");
        if frame["id"] == json!(id) {
            return frame;
        }
    }
}

fn files_containing(dir: &Path, needle: &[u8], found: &mut Vec<String>, count: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_containing(&path, needle, found, count);
        } else if let Ok(bytes) = std::fs::read(&path) {
            *count += 1;
            if bytes.windows(needle.len()).any(|w| w == needle) {
                found.push(path.display().to_string());
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_from_the_desktop_never_reach_relay_logs_or_files() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let paths = pw_relay::paths::relay_paths(&AppPaths::under(&base.join("app")));
    let logs = pw_relay::paths::logs_dir(&paths);
    let mut config = LogConfig::new(&logs);
    config.level = LevelFilter::TRACE;
    tracing::subscriber::set_global_default(subscriber(&config).unwrap()).unwrap();
    tracing::info!(target: "pw_db", "relay-secret-log-test marker");

    let work = base.join("work");
    let checkout = work.join("app");
    std::fs::create_dir_all(checkout.join(".pitwall/db")).unwrap();
    // 닿지 않는 포트 — 드라이버는 비밀을 받아 붙으려다 실패한다.
    std::fs::write(
        checkout.join(".pitwall/db/dev.toml"),
        "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"127.0.0.1\"\nport = 1\nuser = \"app\"\nconnect_timeout = 2\npassword = { secret_ref = \"keychain\" }\n",
    )
    .unwrap();
    let assembled = pw_relay::host::assemble(
        paths.clone(),
        &RelayConfig {
            roots: vec![work.clone()],
            ..RelayConfig::default()
        },
        tokio::runtime::Handle::current(),
    );
    let registry = assembled.host.registry().unwrap().clone();
    let id = registry
        .create(pw_core::registry::NewProject {
            root: checkout.clone(),
            ..Default::default()
        })
        .unwrap()
        .id;
    registry.set_trust(&id, true).unwrap();

    // 붙은 Desktop(가짜): 읽기에는 비밀을 담은 항목을, 두기 · 지우기에는 `done` 을 답한다.
    let mut desktop = assembled
        .host
        .connect(Caller::new(Role::Owner))
        .await
        .unwrap();
    let opened = call(&mut desktop, 1, "secret.requests", Value::Null).await;
    let stream = opened["result"]["streamId"].as_u64().expect("a stream");
    desktop
        .sink
        .send(json!({"jsonrpc": "2.0", "method": "stream.credit",
                             "params": {"streamId": stream, "credit": 64}}));

    let mut window = assembled
        .host
        .connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(id.clone()),
            window: Some("SHA256:desktop/project-1".into()),
        })
        .await
        .unwrap();
    let entry = call(
        &mut window,
        1,
        "db.connection.get",
        json!({"projectId": id, "connectionId": "dev"}),
    )
    .await;
    let config: pw_db::config::DbConnectionConfig =
        serde_json::from_value(entry["result"]["config"].clone())
            .unwrap_or_else(|e| panic!("{e}: {entry}"));
    let target = pw_db::secret::DbSecretTarget::of(&config);
    let item = json!({"v": 1, "target": target, "value": PLANTED}).to_string();

    let answering = tokio::spawn(async move {
        let mut next = 100;
        let mut answered = 0;
        while let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_secs(60), desktop.source.recv()).await
        {
            if frame["method"] != "stream.chunk" {
                continue;
            }
            let request = &frame["params"]["data"];
            let answer = if request["op"]["kind"] == "read" {
                json!({"kind": "value", "item": item})
            } else {
                json!({"kind": "done"})
            };
            next += 1;
            desktop.sink.send(
                json!({"jsonrpc": "2.0", "id": next, "method": "secret.answer",
                                     "params": {"id": request["id"], "answer": answer}}),
            );
            answered += 1;
        }
        answered
    });

    let mut responses = Vec::new();
    let set = call(
        &mut window,
        2,
        "db.secret.set",
        json!({"projectId": id, "connectionId": "dev", "key": "password",
               "target": target, "value": PLANTED}),
    )
    .await;
    assert!(set.get("error").is_none(), "{set}");
    responses.push(set);
    for (n, (method, params)) in [
        (
            "db.session.open",
            json!({"projectId": id, "connectionId": "dev"}),
        ),
        (
            "db.connection.test",
            json!({"projectId": id, "connectionId": "dev"}),
        ),
        (
            "db.secret.clear",
            json!({"projectId": id, "connectionId": "dev", "key": "password"}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        responses.push(call(&mut window, 10 + n as i64, method, params).await);
    }
    // 세션 열기 · 연결 시험은 비밀을 받아 드라이버까지 갔다(닿지 않는 포트라 실패한다 — 「비밀 필요」 가 아니다).
    for r in &responses[1..3] {
        assert_ne!(
            r["error"]["code"],
            json!(pw_db::trust::code::SECRET_REQUIRED),
            "{r}"
        );
    }
    drop(window);
    drop(assembled);
    tokio::time::sleep(Duration::from_millis(300)).await;
    answering.abort();

    let all = format!("{responses:?}");
    assert!(
        !all.contains(PLANTED),
        "a response carries the secret: {all}"
    );
    let mut found = Vec::new();
    let mut count = 0;
    for root in [&paths.state, &paths.data, &paths.config, &paths.cache] {
        files_containing(root, PLANTED.as_bytes(), &mut found, &mut count);
    }
    assert!(found.is_empty(), "the secret is in {found:?}");
    // 로그가 실제로 그 폴더에 쓰였는지 본다(읽는 법이 맞는지).
    let mut marked = Vec::new();
    let mut logged = 0;
    files_containing(
        &logs,
        b"relay-secret-log-test marker",
        &mut marked,
        &mut logged,
    );
    assert!(
        !marked.is_empty(),
        "no log written under {}",
        logs.display()
    );
    let mut asked = Vec::new();
    files_containing(&logs, b"db secret saved", &mut asked, &mut logged);
    assert!(!asked.is_empty(), "the secret flow did not run");
    assert!(count > 1, "{count} files scanned");
}
