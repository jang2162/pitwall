//! 파일 감시 전달을 가짜 서버로 끝까지(15-m4-plan.md M4-05b 완료 기준 ① ~ ④ · 브리프의 상한 · 미신뢰 · 루트 밖 · 해제). 서버는
//! `initialized` 를 받으면 `client/registerCapability` 로 글롭을 등록한다. 증분은 버스에 `fs.changed` 를 직접 내고(종류를 정해),
//! 다시 알리기는 실제 파일 목록 인덱스 · OS 감시(`EnvOptions::files`)로 본다.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use pw_core::bus::Scope;
use pw_fs::watch::{FsChangeKind, FsChanged, TOPIC_CHANGED};
use pw_lsp::fake::{Action, Reply, Scenario};
use pw_lsp::manager::ManagerOptions;
use pw_lsp::methods::LspInstanceState;
use pw_lsp::uri::file_uri;
use pw_lsp::watched::WatchLimits;
use serde_json::{Value, json};

use common::env::{Env, EnvOptions};
use common::{eventually, received};

const METHOD: &str = "workspace/didChangeWatchedFiles";

/// `initialized` 에 `watchers` 를 등록하는 가짜 서버(등록 id `w1`, 서버 요청 id `fake-1`).
fn watching(watchers: Value) -> Scenario {
    Scenario {
        capabilities: json!({ "textDocumentSync": 2, "hoverProvider": true }),
        ..Scenario::default()
    }
    .on(
        "initialized",
        vec![Action::Request {
            method: "client/registerCapability".into(),
            params: json!({ "registrations": [{
                "id": "w1", "method": METHOD, "registerOptions": { "watchers": watchers },
            }] }),
        }],
    )
}

fn unregister() -> Action {
    Action::Request {
        method: "client/unregisterCapability".into(),
        params: json!({ "unregisterations": [{ "id": "w1", "method": METHOD }] }),
    }
}

/// 서버 요청 `id` 에 클라이언트가 답했다(기록 파일에 그 응답이 있다).
async fn answered(record: &Path, id: &str) {
    let ok = eventually(Duration::from_secs(10), || {
        received(record)
            .iter()
            .any(|m| m["id"] == id && m.get("method").is_none())
    })
    .await;
    assert!(ok, "the client never answered {id}");
}

/// 서버가 받은 감시 알림마다 (루트 기준 상대 경로, 종류) 목록.
fn notifications(env: &Env, record: &Path) -> Vec<Vec<(String, u64)>> {
    let root = file_uri(&env.root).unwrap();
    received(record)
        .into_iter()
        .filter(|m| m["method"] == METHOD)
        .map(|m| {
            m["params"]["changes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    let uri = c["uri"].as_str().unwrap();
                    let rel = uri
                        .strip_prefix(&format!("{root}/"))
                        .unwrap_or(uri)
                        .to_owned();
                    (rel, c["type"].as_u64().unwrap())
                })
                .collect()
        })
        .collect()
}

fn changes(env: &Env, record: &Path) -> Vec<(String, u64)> {
    notifications(env, record).into_iter().flatten().collect()
}

fn fs_changed(root: &Path, paths: &[(&Path, FsChangeKind)]) -> FsChanged {
    FsChanged {
        root: root.display().to_string(),
        worktree: None,
        paths: paths.iter().map(|(p, _)| p.display().to_string()).collect(),
        kinds: paths.iter().map(|(_, k)| *k).collect(),
        rescan: false,
    }
}

fn publish(env: &Env, project: &str, changed: &FsChanged) {
    env.bus
        .publish(Scope::Project(project.to_owned()), TOPIC_CHANGED, changed)
        .unwrap();
}

fn rescan(env: &Env) {
    publish(
        env,
        &env.id,
        &FsChanged {
            root: env.root.display().to_string(),
            worktree: None,
            paths: Vec::new(),
            kinds: Vec::new(),
            rescan: true,
        },
    );
}

/// 문서 하나를 열어 서버를 띄우고 등록이 끝날 때까지 기다린다. (창, 문서 id)
async fn ready(env: &Env, record: &Path, doc: &str) -> (common::env::Conn, String) {
    let file = env.file(doc, "fn open() {}\n");
    let mut win = env.window();
    let opened = win.open(env, &file, "Rust", "fn open() {}\n").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    answered(record, "fake-1").await;
    (win, opened["docId"].as_str().unwrap().to_owned())
}

/// 알림이 `count` 개 이상 올 때까지 기다린 뒤 조금 더 기다린다(뒤늦은 알림이 없는지 보려고).
async fn settle(env: &Env, record: &Path, count: usize) {
    eventually(Duration::from_secs(5), || {
        notifications(env, record).len() >= count
    })
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
}

fn options(watch: WatchLimits, files: bool, follow: bool) -> EnvOptions {
    let base = EnvOptions::default();
    EnvOptions {
        files,
        follow,
        manager: ManagerOptions {
            watch,
            ..base.manager
        },
        ..base
    }
}

// ---- ① 등록한 글롭 · 열린 문서 · 루트 ---------------------------------------------------------------------------------------

/// ① 등록 글롭에 맞는 `fs.changed` 만 보내고, 열린 문서의 경로 · 루트 밖 경로 · 다른 루트 · 다른 프로젝트의 바뀜은 뺀다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_matching_changes_inside_the_root_reach_the_server_without_open_documents() {
    let env = Env::new(EnvOptions::default());
    let sub = env.root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let record = env.fake(
        "rust-analyzer",
        watching(json!([
            { "globPattern": "**/*.rs" },
            { "globPattern": { "baseUri": file_uri(&sub).unwrap(), "pattern": "*.toml" } },
        ])),
    );
    let _win = ready(&env, &record, "src/open.rs").await;
    let root = &env.root;
    let outside = env.base.join("p2/c.rs");
    // 다른 루트 · 다른 프로젝트의 이벤트는 닿지 않는다. 루트 안의 다른 감시 루트(메인 체크아웃 안의 워크트리)의 바뀜도 — 경로가
    // 이 루트 안이어도 그 감시의 것이다(두 감시가 같은 바뀜을 두 번 보내지 않게).
    let nested = env.root.join(".worktrees/x");
    publish(
        &env,
        &env.id,
        &fs_changed(&nested, &[(&nested.join("y.rs"), FsChangeKind::Created)]),
    );
    publish(
        &env,
        &env.id,
        &fs_changed(&env.base.join("p2"), &[(&outside, FsChangeKind::Created)]),
    );
    publish(
        &env,
        &env.other,
        &fs_changed(root, &[(&root.join("src/z.rs"), FsChangeKind::Created)]),
    );
    publish(
        &env,
        &env.id,
        &fs_changed(
            root,
            &[
                (&root.join("src/a.rs"), FsChangeKind::Created),
                (&root.join("src/b.txt"), FsChangeKind::Created),
                (&root.join("sub/x.toml"), FsChangeKind::Modified),
                (&root.join("x.toml"), FsChangeKind::Modified),
                (&root.join("src/open.rs"), FsChangeKind::Modified),
                (&outside, FsChangeKind::Created),
                (&root.join("src/../../p2/d.rs"), FsChangeKind::Created),
            ],
        ),
    );
    settle(&env, &record, 1).await;
    assert_eq!(
        changes(&env, &record),
        [("src/a.rs".to_owned(), 1), ("sub/x.toml".to_owned(), 2)]
    );
}

// ---- ② 종류 마스크 -------------------------------------------------------------------------------------------------------------

/// ② 글롭의 종류 마스크(`Create` 1 · `Change` 2 · `Delete` 4) 밖의 바뀜은 보내지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changes_outside_the_kind_mask_stay_out() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        watching(json!([
            { "globPattern": "**/*.rs", "kind": 1 },
            { "globPattern": "**/*.toml", "kind": 6 },
        ])),
    );
    let _win = ready(&env, &record, "src/open.rs").await;
    let root = &env.root;
    publish(
        &env,
        &env.id,
        &fs_changed(
            root,
            &[
                (&root.join("a.rs"), FsChangeKind::Created),
                (&root.join("b.rs"), FsChangeKind::Modified),
                (&root.join("c.rs"), FsChangeKind::Removed),
                (&root.join("d.toml"), FsChangeKind::Created),
                (&root.join("e.toml"), FsChangeKind::Modified),
                (&root.join("f.toml"), FsChangeKind::Removed),
            ],
        ),
    );
    settle(&env, &record, 1).await;
    assert_eq!(
        changes(&env, &record),
        [
            ("a.rs".to_owned(), 1),
            ("e.toml".to_owned(), 2),
            ("f.toml".to_owned(), 3)
        ]
    );
}

// ---- ③ 해제 -------------------------------------------------------------------------------------------------------------------

/// ③ 서버가 등록을 지우면 그 뒤의 바뀜은 보내지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_sent_after_the_server_unregisters() {
    let env = Env::new(EnvOptions::default());
    let mut hover = Reply::result(json!(null));
    hover.then = vec![unregister()];
    let record = env.fake(
        "rust-analyzer",
        watching(json!([{ "globPattern": "**/*.rs" }])).reply("textDocument/hover", hover),
    );
    let (mut win, doc) = ready(&env, &record, "src/open.rs").await;
    let root = &env.root;
    publish(
        &env,
        &env.id,
        &fs_changed(root, &[(&root.join("a.rs"), FsChangeKind::Created)]),
    );
    settle(&env, &record, 1).await;
    assert_eq!(changes(&env, &record), [("a.rs".to_owned(), 1)]);

    win.call(
        "test.lsp.feature",
        json!({ "docId": doc, "feature": "hover", "method": "textDocument/hover", "offset": 0 }),
    )
    .await
    .unwrap();
    answered(&record, "fake-2").await;
    publish(
        &env,
        &env.id,
        &fs_changed(root, &[(&root.join("b.rs"), FsChangeKind::Created)]),
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(changes(&env, &record), [("a.rs".to_owned(), 1)]);
}

/// ③ 큰 다시 알리기를 보내는 중에 등록이 지워지면 남은 묶음을 버린다 — 서버는 첫 알림을 받자마자 등록을 지운다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unregistration_stops_a_rescan_midway() {
    let env = Env::new(options(
        WatchLimits {
            batch_events: 100,
            batch_interval: Duration::from_millis(300),
            ..WatchLimits::default()
        },
        true,
        true,
    ));
    for n in 0..250 {
        env.file(&format!("gen/f{n}.rs"), "");
    }
    let _lease = env.watch_project().await;
    let record = env.fake(
        "rust-analyzer",
        watching(json!([{ "globPattern": "**/*.rs" }])).on(METHOD, vec![unregister()]),
    );
    let _win = ready(&env, &record, "src/open.rs").await;
    rescan(&env);
    settle(&env, &record, 1).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let got = notifications(&env, &record);
    assert_eq!(
        got.len(),
        1,
        "{:?}",
        got.iter().map(Vec::len).collect::<Vec<_>>()
    );
    assert_eq!(got[0].len(), 100);
}

// ---- ④ 다시 알리기 ------------------------------------------------------------------------------------------------------------

/// ④ `rescan` 뒤 파일 목록 인덱스가 다시 만든 목록으로 그 폴더의 파일을 다시 알린다(`Changed`) — 글롭에 맞는 것만, 열린 문서 ·
/// ignore 된 폴더는 빼고.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rescan_announces_the_folder_again() {
    let env = Env::new(options(WatchLimits::default(), true, true));
    std::fs::create_dir_all(env.root.join(".git")).unwrap();
    env.file(".gitignore", "ignored/\n");
    env.file("src/a.rs", "");
    env.file("src/deep/b.rs", "");
    env.file("notes.txt", "");
    env.file("ignored/x.rs", "");
    let files = env.files.clone().unwrap();
    let _lease = env.watch_project().await;
    assert_eq!(files.observer_count(), 0);
    let record = env.fake(
        "rust-analyzer",
        watching(json!([{ "globPattern": "**/*.rs" }])),
    );
    let _win = ready(&env, &record, "src/open.rs").await;
    // 떠 있는 서버마다 관찰자 하나.
    assert_eq!(files.observer_count(), 1);
    // 감시가 걸린 뒤 만든 파일(src/open.rs)의 OS 바뀜이 따로 올 수 있다 — 그 뒤부터 센다.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = notifications(&env, &record).len();
    rescan(&env);
    settle(&env, &record, before + 1).await;
    let mut announced: Vec<(String, u64)> = notifications(&env, &record)[before..]
        .iter()
        .flatten()
        .cloned()
        .collect();
    announced.sort();
    announced.dedup();
    assert_eq!(
        announced,
        [("src/a.rs".to_owned(), 2), ("src/deep/b.rs".to_owned(), 2)]
    );
}

/// 브리프의 상한: 큰 다시 알리기는 알림 하나에 `batch_events` 까지, 알림 사이 `batch_interval` 이상을 두고 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn big_rescans_go_out_in_paced_batches() {
    let env = Env::new(options(
        WatchLimits {
            batch_events: 100,
            batch_interval: Duration::from_millis(300),
            ..WatchLimits::default()
        },
        true,
        true,
    ));
    for n in 0..250 {
        env.file(&format!("gen/f{n}.rs"), "");
    }
    let _lease = env.watch_project().await;
    let record = env.fake(
        "rust-analyzer",
        watching(json!([{ "globPattern": "gen/*.rs" }])),
    );
    let _win = ready(&env, &record, "src/open.rs").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = notifications(&env, &record).len();
    rescan(&env);
    let mut arrivals = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while arrivals.len() < 3 && Instant::now() < deadline {
        let now = notifications(&env, &record).len() - before;
        while arrivals.len() < now {
            arrivals.push(Instant::now());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    let sizes: Vec<usize> = notifications(&env, &record)[before..]
        .iter()
        .map(Vec::len)
        .collect();
    assert_eq!(sizes, [100, 100, 50]);
    for pair in arrivals.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(gap >= Duration::from_millis(250), "batches {gap:?} apart");
    }
}

/// 브리프의 상한: 보내기 전에 쌓아 둘 바뀜 수를 넘친 것은 버린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_waiting_queue_has_a_limit() {
    let env = Env::new(options(
        WatchLimits {
            max_pending: 3,
            ..WatchLimits::default()
        },
        false,
        true,
    ));
    let record = env.fake(
        "rust-analyzer",
        watching(json!([{ "globPattern": "*.rs" }])),
    );
    let _win = ready(&env, &record, "src/open.rs").await;
    let root = &env.root;
    let paths: Vec<_> = (0..5).map(|n| root.join(format!("{n}.rs"))).collect();
    let events: Vec<_> = paths
        .iter()
        .map(|p| (p.as_path(), FsChangeKind::Created))
        .collect();
    publish(&env, &env.id, &fs_changed(root, &events));
    settle(&env, &record, 1).await;
    assert_eq!(changes(&env, &record).len(), 3);
}

// ---- 신뢰 · 수명 ---------------------------------------------------------------------------------------------------------------

/// 미신뢰 프로젝트에는 보내지 않는다 — 신뢰 해제 알림을 따르지 않아 서버가 살아 있어도(관리자의 `follow` 없음). 다시 신뢰하면 그
/// 뒤의 바뀜은 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_gets_no_file_changes() {
    let env = Env::new(options(WatchLimits::default(), false, false));
    let record = env.fake(
        "rust-analyzer",
        watching(json!([{ "globPattern": "**/*.rs" }])),
    );
    let _win = ready(&env, &record, "src/open.rs").await;
    let root = &env.root;
    let send = |name: &str| {
        env.manager.fs_changed(
            &env.id,
            &fs_changed(root, &[(&root.join(name), FsChangeKind::Created)]),
        );
    };
    send("a.rs");
    settle(&env, &record, 1).await;
    env.projects.set_trust(&env.id, false).unwrap();
    send("b.rs");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(changes(&env, &record), [("a.rs".to_owned(), 1)]);
    env.projects.set_trust(&env.id, true).unwrap();
    send("c.rs");
    settle(&env, &record, 2).await;
    assert_eq!(
        changes(&env, &record),
        [("a.rs".to_owned(), 1), ("c.rs".to_owned(), 1)]
    );
}

/// 서버가 끝나면 그 관찰(파일 목록 인덱스)을 뗀다. 다시 띄우면 다시 붙는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_observation_ends_with_the_server() {
    let env = Env::new(options(WatchLimits::default(), true, true));
    let files = env.files.clone().unwrap();
    let _lease = env.watch_project().await;
    let record = env.fake(
        "rust-analyzer",
        watching(json!([{ "globPattern": "**/*.rs" }])),
    );
    let (mut win, _) = ready(&env, &record, "src/open.rs").await;
    assert_eq!(files.observer_count(), 1);
    let root = env.manager.status(&env.id).instances[0].root.clone();
    let control = json!({ "projectId": env.id, "server": "rust-analyzer", "root": root });
    win.call("lsp.server.stop", control.clone()).await.unwrap();
    assert!(
        eventually(Duration::from_secs(2), || files.observer_count() == 0).await,
        "the observation outlived the server"
    );
    win.call("lsp.server.restart", control).await.unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert!(eventually(Duration::from_secs(2), || files.observer_count() == 1).await);
}
