//! 조립의 배선(18-m7-plan M7-01 리뷰 권고 1): `assemble` 은 Desktop 과 Relay 가 함께 쓰는 단 한 곳이라, 그 안에서 기능끼리
//! 잇는 줄이 빠지거나 바뀌면 Host 를 거친 호출로 드러나야 한다.
//!
//! - 런타임 제공자의 신뢰 관문: 미신뢰 프로젝트는 프로젝트 코드를 돌리는 제공자(mise)를 고르지 않고 로그인 셸(`shell`)이다. 신뢰하면
//!   mise 로 바뀐다 — 관문이 늘 「신뢰」 면(fail-open) 앞쪽이, 신뢰 게이트에 레지스트리를 붙이지 않으면(늘 미신뢰) 뒤쪽이 실패한다.
//! - `db.driver.list` 가 부품의 드라이버 넷을 돌려준다.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::registry::NewProject;
use pw_core::{HostRouter, Target};
use pw_db::secret::MemorySecretStore;
use pw_rpc::{Caller, Role};
use serde_json::json;

/// `runtime.get` 이 고른 제공자.
async fn provider(window: &mut pw_core::Connection, id: i64, get: &serde_json::Value) -> String {
    let info = common::call(window, id, "runtime.get", get.clone())
        .await
        .unwrap();
    info["provider"].as_str().unwrap().to_owned()
}

/// 조립한 Host 에 프로젝트 하나(`repo/` — `mise.toml` 이 있다)를 등록하고 그 프로젝트 창의 연결을 연다.
async fn window(
    base: &std::path::Path,
) -> (
    Arc<pw_core::LocalHost>,
    String,
    std::path::PathBuf,
    pw_core::Connection,
) {
    let assembled = common::assemble(
        &base.join("app"),
        Arc::new(MemorySecretStore::new()),
        tokio::runtime::Handle::current(),
    );
    let host = Arc::new(assembled.host);
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    // 도구가 없는 mise 설정 — mise 가 이 디렉터리에 적용되는지만 가른다(설치하지 않는다).
    std::fs::write(root.join("mise.toml"), "[tools]\n").unwrap();
    let id = host
        .registry()
        .unwrap()
        .create(NewProject {
            root: root.clone(),
            ..Default::default()
        })
        .unwrap()
        .id;
    let router = HostRouter::new();
    router
        .add_host(Arc::clone(&host) as Arc<dyn pw_core::Host>)
        .unwrap();
    router.bind_project(id.clone(), "local").unwrap();
    let connection = router
        .connect(&Target::ProjectId(id.clone()), Caller::new(Role::Owner))
        .await
        .unwrap();
    (host, id, root, connection)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_gets_the_login_shell_and_trusting_it_picks_mise() {
    let tmp = tempfile::tempdir().unwrap();
    let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
    let (host, id, root, mut window) = window(&base).await;
    let get = json!({"projectId": id, "dir": root.display().to_string()});
    assert_eq!(
        provider(&mut window, 1, &get).await,
        "shell",
        "미신뢰 프로젝트에서 mise 를 골랐다"
    );

    host.registry().unwrap().set_trust(&id, true).unwrap();
    // 신뢰 변경은 버스의 `project.changed` 를 따라가 그 프로젝트의 캐시를 버린다(뒤에서 돈다).
    let deadline = Instant::now() + Duration::from_secs(20);
    for id in 2.. {
        let now = provider(&mut window, id, &get).await;
        if now == "mise" {
            break;
        }
        assert!(Instant::now() < deadline, "신뢰한 뒤에도 제공자가 {now}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_driver_list_returns_the_drivers_in_the_parts() {
    let tmp = tempfile::tempdir().unwrap();
    let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
    let (_host, _id, _root, mut window) = window(&base).await;
    let list = common::call(&mut window, 1, "db.driver.list", serde_json::Value::Null)
        .await
        .unwrap();
    let kinds: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["mysql", "postgres", "redis", "sqlite"]);
}

/// Host 의 남은 일(`Assembled::activity`)이 열린 터미널을 센다 — Relay 데몬의 유휴 판정(18-m7-plan M7-04 ④)이 이것을 본다. 창의
/// 연결이 닫혀 셸이 끝나면 0 으로 돌아온다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_activity_counts_a_terminal_while_it_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
    let assembled = common::assemble(
        &base.join("app"),
        Arc::new(MemorySecretStore::new()),
        tokio::runtime::Handle::current(),
    );
    let activity = assembled.activity.clone();
    let host = Arc::new(assembled.host);
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let id = host
        .registry()
        .unwrap()
        .create(NewProject {
            root,
            ..Default::default()
        })
        .unwrap()
        .id;
    let router = HostRouter::new();
    router
        .add_host(Arc::clone(&host) as Arc<dyn pw_core::Host>)
        .unwrap();
    router.bind_project(id.clone(), "local").unwrap();
    let mut window = router
        .connect(&Target::ProjectId(id.clone()), Caller::new(Role::Owner))
        .await
        .unwrap();
    assert_eq!((activity.terminals(), activity.runs()), (0, 0));
    common::call(
        &mut window,
        1,
        "terminal.create",
        json!({"projectId": id, "cols": 80, "rows": 24}),
    )
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while activity.terminals() != 1 {
        assert!(Instant::now() < deadline, "the terminal is not counted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(window);
    while activity.terminals() != 0 {
        assert!(
            Instant::now() < deadline,
            "the closed terminal is still counted"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
