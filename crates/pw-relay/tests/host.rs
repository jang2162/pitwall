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
/// 것이다(`bundle.*` · `openWith.*` · `cli.*` — `pw_host::RELAY_EXCLUDED_PREFIXES`). 그래서 `relay.toml` 을 쓰는 RPC 가 없다(18-m7-plan M7-04 시험 ② · 메인 판정 M7-P5): Relay 전용 메서드는 `secret.*`(붙은 Desktop 에 비밀을 청하는 역요청 — M7-12)와 기기 관리 `relay.*`(owner 기기만 — M7-23)뿐이고, Desktop 의
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
    // 빼는 갈래는 이 넷이고(`remote.` — M7-08), 넷 다 Desktop 목록에 있다(이름이 바뀌어 아무것도 빼지 않게 되면 실패한다).
    assert_eq!(
        pw_host::RELAY_EXCLUDED_PREFIXES,
        ["bundle.", "cli.", "openWith.", "remote."]
    );
    for prefix in pw_host::RELAY_EXCLUDED_PREFIXES {
        assert!(snapshot.lines().any(|l| l.starts_with(prefix)), "{prefix}");
    }
    // Relay 에만 있는 것은 「Desktop 에 묻는」 비밀 저장소의 역요청 둘(`secret.*` — 18-m7-plan M7-12)이다. 비밀을 붙은 Desktop 에
    // 청하고 답을 받을 뿐 `relay.toml` 을 쓰지 않는다.
    // 기기 관리(`relay.*` — 18-m7-plan M7-23)는 Relay 의 페어링 표를 보고 바꾼다 — owner 기기만, `relay.toml` 을 쓰지 않는다.
    let (relay_only, relay): (Vec<String>, Vec<String>) = relay.into_iter().partition(|m| {
        pw_db::desktop_secret::METHODS
            .iter()
            .any(|s| m.starts_with(&format!("{s} ")))
            || pw_host::DESKTOP_EXCLUDED_PREFIXES
                .iter()
                .any(|p| m.starts_with(p))
    });
    assert_eq!(pw_host::DESKTOP_EXCLUDED_PREFIXES, ["relay."]);
    assert_eq!(
        relay_only.len(),
        pw_db::desktop_secret::METHODS.len() + 3,
        "{relay_only:?}"
    );
    for m in relay_only.iter().filter(|m| m.starts_with("relay.")) {
        assert!(m.ends_with("{Owner}"), "{m}");
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

/// Relay 에 프로젝트 하나를 등록하고 그 프로젝트 창(붙은 Desktop 의 창 — 링크의 label 은 `<기기 id>/<label>`)의 연결을 연다.
async fn project_window(
    assembled: &pw_host::Assembled,
    root: &Path,
) -> (String, pw_core::Connection) {
    std::fs::create_dir_all(root).unwrap();
    let mut owner = assembled
        .host
        .connect(Caller::new(Role::Owner))
        .await
        .unwrap();
    let created = call(&mut owner, 1, "project.create", json!({ "root": root }))
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    let window = assembled
        .host
        .connect(Caller {
            role: Role::Owner,
            scope: pw_rpc::ConnectionScope::Project(id.clone()),
            window: Some("SHA256:mac/project-1".into()),
        })
        .await
        .unwrap();
    (id, window)
}

/// 18-m7-plan M7-21 ①(재리뷰 F-a · M7-04 ⑦): 원격 체크아웃의 브레이크포인트는 그 머신 Relay 의 것이다 —
/// `$DATA/relay/projects/<id>/breakpoints.json`. 같은 머신 Desktop 의 `$DATA/projects/…` 에는 쓰지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_breakpoints_are_stored_under_the_relay_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let work = base.join("work");
    let assembled = assemble(&base, vec![work.clone()]);
    let (id, mut window) = project_window(&assembled, &work.join("app")).await;
    let saved = call(
        &mut window,
        2,
        "debug.breakpoints.set",
        json!({ "projectId": id, "path": "src/app.py", "breakpoints": [{ "line": 3 }] }),
    )
    .await;
    assert!(saved.is_ok(), "{saved:?}");
    let desktop = AppPaths::under(&base.join("app"));
    let relay = pw_relay::paths::relay_paths(&desktop);
    let file = |paths: &AppPaths| {
        paths
            .data
            .join("projects")
            .join(&id)
            .join("breakpoints.json")
    };
    assert!(file(&relay).is_file(), "{}", file(&relay).display());
    assert!(!file(&desktop).exists(), "{}", file(&desktop).display());
    let listed = call(
        &mut window,
        3,
        "debug.breakpoints.list",
        json!({ "projectId": id }),
    )
    .await
    .unwrap();
    assert_eq!(listed["files"][0]["path"], "src/app.py", "{listed}");
}

/// 18-m7-plan M7-21 ③(결정 M7-D7 — 그 머신의 설치기가 그 머신에서 받는다): 붙은 Desktop 의 프로젝트 창이 부른 `tools.install` 은
/// 그 머신(Relay)의 설치기가 처음 받기 확인(`4001`)을 낸다. 확인 기록 · 로그는 Relay 의 `$STATE/relay/` 아래다 — 같은 머신 Desktop
/// 의 확인 기록(`$STATE/tools/consent.json`)으로는 묻지 않고 넘어가지 않는다(동의는 그 머신에 설치하는 것에 대한 것이다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_tool_install_asks_with_the_relays_own_consent_and_logs() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let work = base.join("work");
    let assembled = assemble(&base, vec![work.clone()]);
    let (_, mut window) = project_window(&assembled, &work.join("app")).await;
    let ask = call(
        &mut window,
        2,
        "tools.install",
        json!({ "name": "debugpy" }),
    )
    .await
    .expect_err("a first install asks");
    assert_eq!(ask["code"], json!(4001), "{ask}");
    let data = &ask["data"];
    assert_eq!(data["name"], "debugpy", "{ask}");

    let desktop = AppPaths::under(&base.join("app"));
    let relay = pw_relay::paths::relay_paths(&desktop);
    // 같은 값으로 Desktop 쪽에 확인을 남겨 두어도 Relay 는 다시 묻는다.
    let consent = json!({
        "schemaVersion": 1,
        "packages": { "debugpy": {
            "licenses": data["licenses"], "sourceDomains": data["sourceDomains"],
            "version": data["version"], "confirmedAtMs": 1, "checksum": data["checksum"],
        } },
    });
    let desktop_consent = desktop.state.join("tools").join("consent.json");
    std::fs::create_dir_all(desktop_consent.parent().unwrap()).unwrap();
    std::fs::write(&desktop_consent, consent.to_string()).unwrap();
    let again = call(
        &mut window,
        3,
        "tools.install",
        json!({ "name": "debugpy" }),
    )
    .await
    .expect_err("the desktop's consent is not the relay's");
    assert_eq!(again["code"], json!(4001), "{again}");
    assert!(!relay.state.join("tools").join("consent.json").exists());

    // 설치 로그도 Relay 의 것을 읽는다.
    for (paths, text) in [(&desktop, "desktop log"), (&relay, "relay log")] {
        let log = paths.state.join("logs").join("tools").join("debugpy.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, text).unwrap();
    }
    let log = call(&mut window, 4, "tools.log", json!({ "name": "debugpy" }))
        .await
        .unwrap();
    assert_eq!(log["text"], "relay log", "{log}");
}
