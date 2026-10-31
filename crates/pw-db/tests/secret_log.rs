//! ⑦(14-m3-plan.md M3-02): 비밀 값이 로그 파일 · RPC 응답 어디에도 없다. 앱과 같은 로그 구독자(`pw_diag::logging`)를 가장 낮은
//! 수준(`trace`)으로 전역에 설치하고, 비밀 · 환경 변수 값을 심어 비밀이 지나는 길(저장 · 목록 · 대상 바뀜 · 확인 필요 · 풀기 ·
//! 지우기 · 범위 거절)을 모두 돈 뒤 로그 폴더의 모든 파일과 모든 응답을 grep 한다. 전역 구독자라 이 파일에는 시험이 하나다.

mod common;

use std::cell::RefCell;
use std::fs;
use std::sync::Arc;

use common::{Fixture, REMOTE_PG};
use pw_diag::logging::{LogConfig, subscriber};
use pw_rpc::ConnectionScope;
use serde_json::json;
use tracing::level_filters::LevelFilter;

const PLANTED: &str = "PLANTED-s3cret-7f3a9c";
const PLANTED_ENV: &str = "PLANTED-env-0b81d2";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secret_values_never_reach_logs_or_responses() {
    let f = Fixture::new(true);
    let logs = f.base.join("logs");
    let mut config = LogConfig::new(&logs);
    config.level = LevelFilter::TRACE;
    tracing::subscriber::set_global_default(subscriber(&config).unwrap()).unwrap();
    // 로그가 실제로 이 폴더에 쓰이는지(읽는 법이 맞는지) 보려고 표지를 하나 남긴다.
    tracing::info!(target: "pw_db", "secret-log-test marker");

    f.write_db("prod.toml", REMOTE_PG);
    f.write_db(
        "ci.toml",
        "schema = 1\nname = \"ci\"\ndriver = \"postgres\"\nhost = \"db.internal\"\npassword = { env = \"PGPASSWORD\" }\n",
    );
    f.env.set("PGPASSWORD", PLANTED_ENV);
    let responses = RefCell::new(Vec::new());
    let mut w = f.window();
    let call = async |conn: &mut common::Conn, method: &str, params: serde_json::Value| {
        let outcome = conn.call(method, params).await;
        responses.borrow_mut().push(format!("{outcome:?}"));
        outcome
    };

    let set =
        json!({ "projectId": f.id, "connectionId": "prod", "key": "password", "value": PLANTED });
    call(&mut w, "db.secret.set", set.clone()).await.unwrap();
    // 다른 창 · 웰컴의 비밀 쓰기(거절)도 값을 싣는다.
    let mut other = f.connect(ConnectionScope::Project(f.other.clone()));
    call(&mut other, "db.secret.set", set.clone())
        .await
        .unwrap_err();
    let mut welcome = f.connect(ConnectionScope::Host);
    call(&mut welcome, "db.secret.set", set.clone())
        .await
        .unwrap_err();
    // 연결이 없는 비밀 쓰기 · 키체인을 쓰지 않는 연결의 비밀 쓰기.
    let mut missing = set.clone();
    missing["connectionId"] = json!("nope");
    call(&mut w, "db.secret.set", missing).await.unwrap_err();
    let mut env_conn = set.clone();
    env_conn["connectionId"] = json!("ci");
    call(&mut w, "db.secret.set", env_conn).await.unwrap_err();

    call(&mut w, "db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    call(
        &mut w,
        "db.connection.get",
        json!({ "projectId": f.id, "connectionId": "prod" }),
    )
    .await
    .unwrap();
    let gate = f.connections.trust_gate(&f.id).unwrap();
    let resolved = f.connections.resolve(&gate, "prod").unwrap();
    assert_eq!(resolved.spec.password.as_ref().unwrap().expose(), PLANTED);
    tracing::debug!(target: "pw_db", ?resolved, "a resolved connection in a debug log");
    let resolved = f.connections.resolve(&gate, "ci").unwrap();
    assert_eq!(
        resolved.spec.password.as_ref().unwrap().expose(),
        PLANTED_ENV
    );
    tracing::debug!(target: "pw_db", ?resolved, "a resolved env connection in a debug log");

    // 대상이 바뀐 비밀 · 확인이 필요한 env.
    f.edit_db(
        "prod.toml",
        "host = \"db.internal\"",
        "host = \"evil.example\"",
    );
    f.edit_db(
        "ci.toml",
        "host = \"db.internal\"",
        "host = \"evil.example\"",
    );
    for id in ["prod", "ci"] {
        let e = f.connections.resolve(&gate, id).unwrap_err();
        responses.borrow_mut().push(format!("{e:?}"));
    }
    call(&mut w, "db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    call(
        &mut w,
        "db.secret.clear",
        json!({ "projectId": f.id, "connectionId": "prod", "key": "password" }),
    )
    .await
    .unwrap();
    call(
        &mut w,
        "db.connection.delete",
        json!({ "projectId": f.id, "connectionId": "prod" }),
    )
    .await
    .unwrap();

    let mut text = String::new();
    for entry in fs::read_dir(&logs).unwrap() {
        text.push_str(&fs::read_to_string(entry.unwrap().path()).unwrap());
    }
    assert!(
        text.contains("secret-log-test marker"),
        "the log folder holds no log: {text}"
    );
    assert!(
        text.contains("db secret saved"),
        "the secret path was not logged at all: {text}"
    );
    for planted in [PLANTED, PLANTED_ENV] {
        assert!(
            !text.contains(planted),
            "a secret value is in the log:\n{text}"
        );
        for response in responses.borrow().iter() {
            assert!(
                !response.contains(planted),
                "a secret value is in a response: {response}"
            );
        }
    }
    // 설정 파일 · 확인 기록에도 없다.
    for dir in [f.root.join(".pitwall"), f.app.data.clone()] {
        for entry in walk(&dir) {
            let bytes = fs::read(&entry).unwrap();
            let body = String::from_utf8_lossy(&bytes);
            for planted in [PLANTED, PLANTED_ENV] {
                assert!(
                    !body.contains(planted),
                    "{} holds a secret",
                    entry.display()
                );
            }
        }
    }
    let _ = Arc::clone(&f.connections);
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}
