//! Relay 의 Host(18-m7-plan M7-04): Desktop 과 같은 조립에서 묶음만 빠지고, 등록은 `relay.toml` 의 `roots` 안만 받는다.

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use pw_core::Host;
use pw_core::config::AppPaths;
use pw_relay::config::RelayConfig;
use pw_rpc::{Caller, Role};
use serde_json::{Value, json};

fn assemble(dir: &Path, roots: Vec<std::path::PathBuf>) -> pw_host::Assembled {
    pw_relay::host::assemble(
        pw_relay::paths::relay_paths(&AppPaths::under(&dir.join("app"))),
        &RelayConfig {
            roots,
            ..RelayConfig::default()
        },
        tokio::runtime::Handle::current(),
    )
}

async fn call(
    conn: &mut pw_core::Connection,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value, Value> {
    conn.sink
        .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(20), conn.source.recv())
            .await
            .expect("no reply")
            .expect("closed");
        if frame["id"] == json!(id) {
            return match frame.get("error") {
                Some(error) => Err(error.clone()),
                None => Ok(frame["result"].clone()),
            };
        }
    }
}

/// `project.create`(owner 의 RPC)는 `roots` 안만 등록한다 — 밖, `roots` 안의 심링크로 밖을 가리키는 경로는 `INVALID_PARAMS` 로
/// 거절된다(18-m7-plan M7-04 시험 ① — 판정을 걷으면 실패).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_create_registers_only_inside_the_roots() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let allowed = base.join("work");
    let outside = base.join("secret");
    std::fs::create_dir_all(allowed.join("app")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, allowed.join("link")).unwrap();
    let assembled = assemble(&base, vec![allowed.clone()]);
    let mut conn = assembled
        .host
        .connect(Caller::new(Role::Owner))
        .await
        .unwrap();

    let created = call(
        &mut conn,
        1,
        "project.create",
        json!({"root": allowed.join("app")}),
    )
    .await;
    assert!(created.is_ok(), "{created:?}");
    for (id, refused) in [(2, outside.clone()), (3, allowed.join("link"))] {
        let error = call(&mut conn, id, "project.create", json!({"root": refused}))
            .await
            .expect_err(&refused.display().to_string());
        assert_eq!(
            error["code"],
            json!(pw_rpc::code::INVALID_PARAMS),
            "{error}"
        );
        assert!(
            error["data"].as_str().unwrap().contains("relay.toml"),
            "{error}"
        );
    }
    let list = call(&mut conn, 4, "project.list", Value::Null)
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
}

/// Relay 의 메서드 목록은 Desktop 의 목록(`pw-host` 의 `methods.snapshot` — 이름 · 종류 · 등급 · role)에서 Desktop 전용 갈래만 뺀
/// 것이다(`bundle.*` · `openWith.*` · `cli.*` — `pw_host::RELAY_EXCLUDED_PREFIXES`). 그래서 `relay.toml` 을 쓰는 RPC 가 없다(18-m7-plan M7-04 시험 ② · 메인 판정 M7-P5): Relay 전용 메서드는 없고, Desktop 의
/// 설정 쓰기(`mcp.settings.set` 등)는 `settings.toml` · 프로젝트 파일만 쓴다. 메서드가 늘면 스냅숏을 다시 써야 해 리뷰에 걸린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_methods_are_the_desktop_methods_without_desktop_only_ones() {
    let dir = tempfile::tempdir().unwrap();
    let assembled = assemble(dir.path(), Vec::new());
    let relay: Vec<String> = assembled
        .host
        .router()
        .methods()
        .into_iter()
        .map(|m| format!("{} {:?} {:?} {:?}", m.name, m.kind, m.tier, m.roles))
        .collect();
    let snapshot = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../pw-host/tests/methods.snapshot"),
    )
    .unwrap();
    let desktop: Vec<String> = snapshot
        .lines()
        .filter(|line| {
            !pw_host::RELAY_EXCLUDED_PREFIXES
                .iter()
                .any(|p| line.starts_with(p))
        })
        .map(str::to_owned)
        .collect();
    // 빼는 갈래는 이 셋이고, 셋 다 Desktop 목록에 있다(이름이 바뀌어 아무것도 빼지 않게 되면 실패한다).
    assert_eq!(
        pw_host::RELAY_EXCLUDED_PREFIXES,
        ["bundle.", "cli.", "openWith."]
    );
    for prefix in pw_host::RELAY_EXCLUDED_PREFIXES {
        assert!(snapshot.lines().any(|l| l.starts_with(prefix)), "{prefix}");
    }
    assert_eq!(relay, desktop);
    assert!(
        !relay.iter().any(|m| m.to_lowercase().contains("relay")),
        "{relay:?}"
    );

    let mut conn = assembled
        .host
        .connect(Caller::new(Role::Owner))
        .await
        .unwrap();
    let error = call(&mut conn, 1, "bundle.list", Value::Null)
        .await
        .unwrap_err();
    assert_eq!(
        error["code"],
        json!(pw_rpc::code::METHOD_NOT_FOUND),
        "{error}"
    );
}
