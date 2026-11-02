//! 진단 저장(15-m4-plan.md M4-06)의 가짜 서버 시험: push · pull 로 받은 진단이 목록 · 수 · 이벤트로 나가고, 지난 버전 · 끝난
//! 서버의 진단은 버리고, 서버가 끝나거나(멈춤 · 재기동 · 신뢰 해제 · 창 닫힘) 하면 그 서버의 진단만 지우고, 망가진 서버가 쏟아부어도
//! 상한에서 자른다. 정의는 내장 번들에 설정 `[lsp.<id>] path` 로 가짜 서버를 꽂는다(tests/manager.rs 와 같은 바탕).

#![cfg(unix)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::env::{Env, EnvOptions};
use common::{eventually, received};
use pw_core::Filter;
use pw_core::bus::{Received, Subscription};
use pw_lsp::diagnostics::{FILE_LIMIT, PROJECT_LIMIT, TOPIC_DIAGNOSTICS};
use pw_lsp::fake::{Action, Reply, Scenario};
use pw_lsp::methods::LspInstanceState;
use pw_lsp::uri::file_uri;
use pw_rpc::ConnectionScope;
use serde_json::{Value, json};

fn doc_id(opened: &Value) -> String {
    opened["docId"].as_str().unwrap().to_owned()
}

fn methods_of(record: &Path, method: &str) -> Vec<Value> {
    received(record)
        .into_iter()
        .filter(|m| m["method"] == method)
        .collect()
}

fn uri(path: &Path) -> String {
    file_uri(path).unwrap()
}

/// 진단 하나(줄 `line` 의 0 ~ 1 열).
fn diag(line: u32, severity: u64, message: &str) -> Value {
    json!({
        "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 1 } },
        "severity": severity,
        "message": message,
    })
}

fn publish(uri: &str, version: Option<i64>, diagnostics: Vec<Value>) -> Action {
    let mut params = json!({ "uri": uri, "diagnostics": diagnostics });
    if let Some(v) = version {
        params["version"] = json!(v);
    }
    Action::Notify {
        method: "textDocument/publishDiagnostics".into(),
        params,
    }
}

fn incremental() -> Value {
    json!({ "textDocumentSync": 2 })
}

fn pull_capable() -> Value {
    json!({
        "textDocumentSync": 2,
        "diagnosticProvider": { "interFileDependencies": false, "workspaceDiagnostics": false },
    })
}

fn subscribe(env: &Env) -> Subscription {
    env.bus.subscribe(Filter::all().topics([TOPIC_DIAGNOSTICS]))
}

/// `path` 의 다음 진단 이벤트(시한 10 s).
async fn next_for(events: &mut Subscription, path: &Path) -> Value {
    next_within(events, path, Duration::from_secs(10)).await
}

/// `path` 의 다음 진단 이벤트(시한 `limit`).
async fn next_within(events: &mut Subscription, path: &Path, limit: Duration) -> Value {
    let path = path.to_string_lossy();
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let got = tokio::time::timeout_at(deadline, events.recv())
            .await
            .unwrap_or_else(|_| panic!("no lsp.diagnostics for {path}"));
        if let Some(Received::Event(event)) = got
            && event.payload["path"] == path.as_ref()
        {
            return event.payload.clone();
        }
    }
}

fn messages(diagnostics: &Value) -> Vec<String> {
    diagnostics
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap().to_owned())
        .collect()
}

/// `lsp.diagnostics.list` 의 그 경로 진단 메시지.
async fn listed(win: &mut common::env::Conn, env: &Env, path: &Path) -> Vec<String> {
    let list = win
        .call(
            "lsp.diagnostics.list",
            json!({ "projectId": env.id, "path": path, "limit": 5000 }),
        )
        .await
        .unwrap();
    list["files"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| messages(&f["diagnostics"]))
        .collect()
}

/// 받은 `textDocument/publishDiagnostics` 알림 수를 센다(받는 쪽이 늦은 알림을 실제로 받아 버렸는지 기다린다).
fn counting_env(options: EnvOptions) -> (Env, Arc<AtomicUsize>) {
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    let env = Env::with_listener(options, move |manager| {
        manager.on_notification("textDocument/publishDiagnostics", move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
    });
    (env, seen)
}

/// push 진단이 목록 · 수 · 이벤트로 나간다: 심각도 · 위치 차례, 심각도 거르기, 쪽 나누기(완료 기준 ⑥ 의 Host 쪽 — 수가 목록과
/// 같다). 루트 밖 · `file:` 이 아닌 URI 의 진단은 받지 않는다. 다른 프로젝트 창은 읽지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn published_diagnostics_reach_the_list_counts_and_event() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "a\nb\nc\nd\n");
    let outside = env.base.join("elsewhere.rs");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![
                publish(
                    &uri(&file),
                    Some(0),
                    vec![
                        diag(2, 4, "hint"),
                        diag(1, 2, "warning"),
                        diag(3, 1, "error"),
                    ],
                ),
                publish(&uri(&outside), None, vec![diag(0, 1, "outside")]),
                publish("untitled:x", None, vec![diag(0, 1, "untitled")]),
            ],
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "a\nb\nc\nd\n").await;
    let event = next_for(&mut events, &file).await;
    assert_eq!(event["projectId"], json!(env.id));
    assert_eq!(event["worktree"], Value::Null);
    assert_eq!(
        messages(&event["diagnostics"]),
        ["error", "warning", "hint"]
    );
    assert_eq!(event["diagnostics"][0]["server"], "rust-analyzer");
    assert_eq!(event["diagnostics"][0]["origin"], "push");
    assert_eq!(event["diagnostics"][0]["version"], 0);
    assert_eq!(event["truncated"], false);

    let list = win
        .call(
            "lsp.diagnostics.list",
            json!({ "projectId": env.id, "limit": 100 }),
        )
        .await
        .unwrap();
    let files = list["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{list}");
    assert_eq!(files[0]["path"], json!(file));
    assert_eq!(list["nextCursor"], Value::Null);
    let counts = win
        .call("lsp.diagnostics.counts", json!({ "projectId": env.id }))
        .await
        .unwrap();
    assert_eq!(
        counts,
        json!({ "error": 1, "warning": 1, "information": 0, "hint": 1, "truncated": false })
    );
    let worse = win
        .call(
            "lsp.diagnostics.list",
            json!({ "projectId": env.id, "minSeverity": "warning", "limit": 100 }),
        )
        .await
        .unwrap();
    assert_eq!(
        messages(&worse["files"][0]["diagnostics"]),
        ["error", "warning"]
    );
    // 쪽 나누기: 둘 → 하나.
    let first = win
        .call(
            "lsp.diagnostics.list",
            json!({ "projectId": env.id, "limit": 2 }),
        )
        .await
        .unwrap();
    assert_eq!(
        messages(&first["files"][0]["diagnostics"]),
        ["error", "warning"]
    );
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = win
        .call(
            "lsp.diagnostics.list",
            json!({ "projectId": env.id, "limit": 2, "cursor": cursor }),
        )
        .await
        .unwrap();
    assert_eq!(messages(&second["files"][0]["diagnostics"]), ["hint"]);
    assert_eq!(second["nextCursor"], Value::Null);
    // 다른 워크트리를 고르면 이 진단은 없다.
    let other_worktree = win
        .call(
            "lsp.diagnostics.counts",
            json!({ "projectId": env.id, "worktree": "/elsewhere" }),
        )
        .await
        .unwrap();
    assert_eq!(other_worktree["error"], 0);

    let mut stranger = env.connect(ConnectionScope::Project(env.other.clone()));
    let refused = stranger
        .call(
            "lsp.diagnostics.list",
            json!({ "projectId": env.id, "limit": 10 }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, pw_rpc::code::PERMISSION_DENIED);
}

/// 완료 기준 ②: 진단의 `version` 이 서버에 보낸 문서 버전보다 앞서면 버린다(그 텍스트는 이미 지났다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_for_an_older_document_version_are_dropped() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "fn a() {}\n");
    let marker = env.file("src/marker.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), Some(0), vec![diag(0, 1, "fresh")])],
        )
        .on(
            "textDocument/didChange",
            vec![
                // 편집(버전 1) 뒤에 온 버전 0 의 진단 — 지난 텍스트 기준이다.
                publish(&uri(&file), Some(0), vec![diag(0, 1, "stale")]),
                // 같은 채널의 다음 알림 — 이것이 보이면 앞의 것도 처리됐다.
                publish(&uri(&marker), None, vec![diag(0, 1, "marker")]),
            ],
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Rust", "fn a() {}\n").await);
    assert_eq!(
        messages(&next_for(&mut events, &file).await["diagnostics"]),
        ["fresh"]
    );
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "x" }] }),
    )
    .await
    .unwrap();
    next_for(&mut events, &marker).await;
    assert_eq!(listed(&mut win, &env, &file).await, ["fresh"]);
}

/// 완료 기준 ③: 두 서버가 같은 파일에 낸 진단 중 멈춘 서버의 것만 사라진다(Python — basedpyright · ruff, 결정 M4-D12).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_one_server_clears_only_its_diagnostics() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("app.py", "x = 1\n");
    for (server, message) in [("basedpyright", "pyright"), ("ruff", "ruff")] {
        env.fake(
            server,
            Scenario {
                capabilities: incremental(),
                ..Scenario::default()
            }
            .on(
                "textDocument/didOpen",
                vec![publish(&uri(&file), Some(0), vec![diag(0, 2, message)])],
            ),
        );
    }
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Python", "x = 1\n").await;
    let both = eventually(Duration::from_secs(10), || {
        let Some(Received::Event(e)) = events.try_recv() else {
            return false;
        };
        e.payload["diagnostics"]
            .as_array()
            .is_some_and(|d| d.len() == 2)
    })
    .await;
    assert!(both, "both servers' diagnostics never arrived");
    let root = env
        .manager
        .status(&env.id)
        .instances
        .into_iter()
        .find(|i| i.server == "ruff")
        .unwrap()
        .root;
    win.call(
        "lsp.server.stop",
        json!({ "projectId": env.id, "server": "ruff", "root": root }),
    )
    .await
    .unwrap();
    let event = next_for(&mut events, &file).await;
    assert_eq!(messages(&event["diagnostics"]), ["pyright"]);
    assert_eq!(listed(&mut win, &env, &file).await, ["pyright"]);
}

/// 신뢰를 풀면 그 프로젝트의 서버가 끝나고 그 진단도 지운다(결정 M4-D8).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_trust_clears_the_projects_diagnostics() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), Some(0), vec![diag(0, 1, "error")])],
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    next_for(&mut events, &file).await;
    env.projects.set_trust(&env.id, false).unwrap();
    let event = next_for(&mut events, &file).await;
    assert_eq!(event["diagnostics"], json!([]));
    let counts = win
        .call("lsp.diagnostics.counts", json!({ "projectId": env.id }))
        .await
        .unwrap();
    assert_eq!(counts["error"], 0);
}

/// 창이 닫히면 그 창이 띄운 서버가 끝나고 그 진단도 지운다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_window_clears_its_servers_diagnostics() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), Some(0), vec![diag(0, 1, "error")])],
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    next_for(&mut events, &file).await;
    win.session.close();
    let event = next_for(&mut events, &file).await;
    assert_eq!(event["diagnostics"], json!([]));
    let mut other = env.window();
    assert!(listed(&mut other, &env, &file).await.is_empty());
}

/// 창을 닫고 곧바로 다시 열면 같은 키의 새 인스턴스가 생긴다 — 끝나는 옛 서버가 늦게 보낸 진단이 새 서버의 진단을 덮지 않는다
/// (세대가 인스턴스마다 0 부터면 옛 서버와 새 서버의 세대가 같아진다 — M4-06 리뷰 M1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopening_a_closed_window_ignores_the_ending_servers_late_diagnostics() {
    let (env, seen) = counting_env(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ignore_exit: true,
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), Some(0), vec![diag(0, 1, "fresh")])],
        )
        // 새 창이 새 서버를 띄운 뒤에 옛 서버가 진단을 내게 `shutdown` 을 늦게 답한다.
        .reply(
            "shutdown",
            Reply {
                result: Some(Value::Null),
                delay_ms: 900,
                then: vec![publish(&uri(&file), None, vec![diag(0, 1, "late")])],
                ..Reply::default()
            },
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    next_for(&mut events, &file).await;
    win.session.close();
    assert_eq!(next_for(&mut events, &file).await["diagnostics"], json!([]));
    let mut again = env.window();
    again.open(&env, &file, "Rust", "").await;
    assert_eq!(
        messages(&next_for(&mut events, &file).await["diagnostics"]),
        ["fresh"]
    );
    assert!(
        eventually(Duration::from_secs(10), || seen.load(Ordering::SeqCst) >= 3).await,
        "the ending server's late notification never arrived"
    );
    assert_eq!(listed(&mut again, &env, &file).await, ["fresh"]);
}

/// 멈춘 서버가 끝나는 동안 보낸 진단(`shutdown` 을 받고 낸 것)은 받지 않는다 — 지운 뒤에 다시 쌓이지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_servers_late_diagnostics_are_dropped() {
    let (env, seen) = counting_env(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            // `exit` 를 받으면 가짜 서버가 쓰기를 끊어 그 앞에 낸 알림을 잃을 수 있다 — 끝내기 상한(2 s) 뒤 죽게 둔다.
            ignore_exit: true,
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), Some(0), vec![diag(0, 1, "fresh")])],
        )
        .on(
            "shutdown",
            vec![publish(&uri(&file), None, vec![diag(0, 1, "late")])],
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    next_for(&mut events, &file).await;
    let root = env.manager.status(&env.id).instances[0].root.clone();
    win.call(
        "lsp.server.stop",
        json!({ "projectId": env.id, "server": "rust-analyzer", "root": root }),
    )
    .await
    .unwrap();
    assert!(
        eventually(Duration::from_secs(5), || seen.load(Ordering::SeqCst) >= 2).await,
        "the late notification never arrived"
    );
    assert!(listed(&mut win, &env, &file).await.is_empty());
}

/// 재기동 경합: 끝나는 서버가 새 서버보다 늦게 보낸 진단은 버리고 새 서버의 것만 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_keeps_only_the_new_servers_diagnostics() {
    let (env, seen) = counting_env(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            // `exit` 를 받으면 가짜 서버가 쓰기를 끊어 그 앞에 낸 알림을 잃을 수 있다 — 끝내기 상한(2 s) 뒤 죽게 둔다.
            ignore_exit: true,
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), Some(0), vec![diag(0, 1, "fresh")])],
        )
        // 끝나는 서버는 새 서버가 진단을 낸 뒤에 「late」 를 낸다.
        .on(
            "shutdown",
            vec![
                Action::Sleep { ms: 400 },
                publish(&uri(&file), None, vec![diag(0, 1, "late")]),
            ],
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    next_for(&mut events, &file).await;
    let root = env.manager.status(&env.id).instances[0].root.clone();
    win.call(
        "lsp.server.restart",
        json!({ "projectId": env.id, "server": "rust-analyzer", "root": root }),
    )
    .await
    .unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert!(
        eventually(Duration::from_secs(5), || seen.load(Ordering::SeqCst) >= 3).await,
        "fresh · fresh · late never all arrived ({})",
        seen.load(Ordering::SeqCst)
    );
    assert_eq!(listed(&mut win, &env, &file).await, ["fresh"]);
}

/// pull(3.17): 서버가 `diagnosticProvider` 를 알리면 문서 텍스트를 보낼 때마다 당긴다. 새 버전이 오면 앞 pull 을 취소하고
/// (`$/cancelRequest`) 그 답은 쌓지 않는다. 다음 pull 은 앞 답의 `resultId` 를 싣는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pulled_diagnostics_follow_the_document_and_cancel_older_pulls() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: pull_capable(),
            ..Scenario::default()
        }
        .reply(
            "textDocument/diagnostic",
            Reply::result(json!({
                "kind": "full",
                "resultId": "r1",
                "items": [diag(0, 1, "pulled")],
            }))
            .delayed(300),
        ),
    );
    let mut events = env.bus.subscribe(Filter::all().topics([TOPIC_DIAGNOSTICS]));
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Rust", "").await);
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    // 첫 pull(버전 0)이 도는 사이에 고친다.
    assert!(
        eventually(Duration::from_secs(5), || {
            !methods_of(&record, "textDocument/diagnostic").is_empty()
        })
        .await
    );
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "x" }] }),
    )
    .await
    .unwrap();
    let event = next_for(&mut events, &file).await;
    assert_eq!(messages(&event["diagnostics"]), ["pulled"]);
    assert_eq!(event["diagnostics"][0]["version"], 1, "{event}");
    // 웹뷰는 (서버, 길)마다 바꿔 끼운다 — 같은 서버의 push 와 가른다(M4-06 리뷰 H1).
    assert_eq!(event["diagnostics"][0]["origin"], "pull", "{event}");
    let pulls = methods_of(&record, "textDocument/diagnostic");
    assert_eq!(pulls.len(), 2, "{pulls:?}");
    let cancelled: Vec<Value> = methods_of(&record, "$/cancelRequest")
        .into_iter()
        .map(|m| m["params"]["id"].clone())
        .collect();
    assert_eq!(cancelled, [pulls[0]["id"].clone()]);
    // 다음 편집의 pull 은 앞 답의 resultId 를 싣는다.
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 2, "changes": [{ "from": 0, "to": 0, "insert": "y" }] }),
    )
    .await
    .unwrap();
    let event = next_for(&mut events, &file).await;
    assert_eq!(event["diagnostics"][0]["version"], 2);
    let pulls = methods_of(&record, "textDocument/diagnostic");
    assert_eq!(pulls[2]["params"]["previousResultId"], "r1");
    // 버전 0 의 답은 한 번도 나가지 않았다.
    while let Some(Received::Event(e)) = events.try_recv() {
        assert!(
            e.payload["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| d["version"] != 0),
            "{}",
            e.payload
        );
    }
}

/// 멈춘 서버의 늦은 pull 답은 쌓지 않는다(서버가 끝나는 사이에 답이 온다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_servers_late_pull_answer_is_dropped() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: pull_capable(),
            ..Scenario::default()
        }
        .reply(
            "textDocument/diagnostic",
            Reply::result(json!({ "kind": "full", "items": [diag(0, 1, "late pull")] }))
                .delayed(300),
        )
        // 끝내는 사이에 pull 답이 나가게 `shutdown` 을 늦게 답한다.
        .reply("shutdown", Reply::result(Value::Null).delayed(800)),
    );
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert!(
        eventually(Duration::from_secs(5), || {
            !methods_of(&record, "textDocument/diagnostic").is_empty()
        })
        .await
    );
    let root = env.manager.status(&env.id).instances[0].root.clone();
    win.call(
        "lsp.server.stop",
        json!({ "projectId": env.id, "server": "rust-analyzer", "root": root }),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(listed(&mut win, &env, &file).await.is_empty());
}

/// `workspace/diagnostic/refresh` 는 열린 문서를 다시 당기되 몰려 와도 한 번으로 합친다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refreshes_pull_again_once_however_many_arrive() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    let refresh = Action::Request {
        method: "workspace/diagnostic/refresh".into(),
        params: Value::Null,
    };
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: pull_capable(),
            ..Scenario::default()
        }
        .reply(
            "textDocument/diagnostic",
            Reply::result(json!({ "kind": "full", "items": [diag(0, 1, "pulled")] })),
        )
        .on("textDocument/didOpen", vec![refresh; 20]),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    next_for(&mut events, &file).await;
    // 처리기가 refresh 마다 답했다(null).
    assert!(
        eventually(Duration::from_secs(5), || {
            received(&record)
                .iter()
                .filter(|m| m.get("method").is_none() && m["result"].is_null())
                .count()
                >= 20
        })
        .await
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let pulls = methods_of(&record, "textDocument/diagnostic").len();
    // 열 때 하나 + 몰려 온 refresh 스물을 합친 것 — 모으는 창(20 ms)에 다 들거나, 당기는 동안 온 나머지가 끝난 뒤 한 번 더(합치지
    // 않으면 스물하나).
    assert!((2..=3).contains(&pulls), "{pulls} pulls");
}

/// 망가진 서버가 진단을 쏟아부어도 파일당 [`FILE_LIMIT`] · 프로젝트 [`PROJECT_LIMIT`] 에서 자르고 `truncated` 를 단다(오류 먼저 남는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flood_is_cut_at_the_file_and_project_limits() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    let mut actions = Vec::new();
    // 파일 하나에 상한의 1.5배(맨 끝에 오류 하나).
    let mut flood: Vec<Value> = (0..FILE_LIMIT * 3 / 2)
        .map(|i| diag(u32::try_from(i).unwrap(), 4, "hint"))
        .collect();
    flood.push(diag(9_999, 1, "the error"));
    actions.push(publish(&uri(&file), Some(0), flood));
    // 그 밖의 파일들이 프로젝트 상한을 채우고 넘는다.
    let files = PROJECT_LIMIT / FILE_LIMIT;
    for n in 0..files {
        let path = env.root.join(format!("src/f{n}.rs"));
        actions.push(publish(
            &uri(&path),
            None,
            (0..FILE_LIMIT)
                .map(|i| diag(u32::try_from(i).unwrap(), 2, "w"))
                .collect(),
        ));
    }
    let last = env.root.join("src/last.rs");
    actions.push(publish(&uri(&last), None, vec![diag(0, 2, "over")]));
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on("textDocument/didOpen", actions),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    // 9 MB 남짓한 알림 백여 개를 debug 빌드로 읽는다 — 기기 부하에서 10 s 를 넘는다.
    let slow = Duration::from_secs(90);
    let first = next_within(&mut events, &file, slow).await;
    assert_eq!(first["truncated"], true);
    assert_eq!(first["diagnostics"].as_array().unwrap().len(), FILE_LIMIT);
    assert_eq!(first["diagnostics"][0]["message"], "the error");
    let over = next_within(&mut events, &last, slow).await;
    assert_eq!(over["truncated"], true);
    assert_eq!(over["diagnostics"], json!([]));
    let counts = win
        .call("lsp.diagnostics.counts", json!({ "projectId": env.id }))
        .await
        .unwrap();
    let total = ["error", "warning", "information", "hint"]
        .iter()
        .map(|k| counts[k].as_u64().unwrap())
        .sum::<u64>();
    assert_eq!(total, PROJECT_LIMIT as u64, "{counts}");
    assert_eq!(counts["truncated"], true);
    let list = win
        .call(
            "lsp.diagnostics.list",
            json!({ "projectId": env.id, "limit": 10 }),
        )
        .await
        .unwrap();
    assert_eq!(list["truncated"], true);
}

/// 미신뢰 프로젝트에서는 서버가 뜨지 않는다(결정 M4-D8) — 진단 RPC 는 오류 없이 비어 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_has_empty_diagnostics_without_errors() {
    let env = Env::new(EnvOptions {
        trusted: false,
        ..EnvOptions::default()
    });
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), Some(0), vec![diag(0, 1, "error")])],
        ),
    );
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::Untrusted)
    );
    assert!(listed(&mut win, &env, &file).await.is_empty());
    let counts = win
        .call("lsp.diagnostics.counts", json!({ "projectId": env.id }))
        .await
        .unwrap();
    assert_eq!(
        counts,
        json!({ "error": 0, "warning": 0, "information": 0, "hint": 0, "truncated": false })
    );
}

/// 관리자 잠금 안에서 불리는 `on_synced` 의 받는 쪽이 태스크에서 관리자를 읽고, 잠금 밖의 `on_status` 받는 쪽은 곧바로 읽어도
/// 교착하지 않는다(진단 저장이 같은 길을 쓴다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listeners_that_read_the_manager_do_not_deadlock() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let statuses = Arc::new(AtomicUsize::new(0));
    let env = Env::with_listener(EnvOptions::default(), {
        let statuses = Arc::clone(&statuses);
        move |manager| {
            let m = manager.clone();
            manager.on_synced(move |source, uri, version| {
                let (m, source, uri, tx) = (m.clone(), source.clone(), uri.to_owned(), tx.clone());
                tokio::spawn(async move {
                    let _ = tx.send((version, m.synced_version(&source, &uri)));
                });
            });
            let m = manager.clone();
            manager.on_status(move |project| {
                let _ = m.status(project);
                statuses.fetch_add(1, Ordering::SeqCst);
            });
        }
    });
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        },
    );
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Rust", "").await);
    let (version, read) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the sync listener never ran")
        .unwrap();
    assert_eq!((version, read), (0, Some(0)));
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "x" }] }),
    )
    .await
    .unwrap();
    let (version, read) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the sync listener never ran after an edit")
        .unwrap();
    assert_eq!(version, 1);
    assert!(read.is_some_and(|v| v >= 1));
    assert!(statuses.load(Ordering::SeqCst) > 0);
}

/// 세대: 끝난 서버의 알림이 밀려 새 서버가 뜬 **뒤에** 처리돼도(이벤트 태스크가 밀림) 버린다 — 그때 인스턴스에는 새 서버가 있으므로
/// 「서버가 있는가」 만으로는 가를 수 없다. 앞의 받는 쪽이 옛 서버의 「late」 를 새 서버가 진단을 낸 뒤까지 붙잡아 경합을 만든다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_servers_notification_handled_after_a_new_server_started_is_dropped() {
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let go_rx = std::sync::Mutex::new(Some(go_rx));
    let held = Arc::new(AtomicUsize::new(0));
    let env = Env::with_listener(EnvOptions::default(), {
        let held = Arc::clone(&held);
        move |manager| {
            manager.on_notification("textDocument/publishDiagnostics", move |_, params| {
                if params["diagnostics"][0]["message"] != "late" {
                    return;
                }
                let Some(rx) = go_rx.lock().unwrap().take() else {
                    return;
                };
                held.fetch_add(1, Ordering::SeqCst);
                // 옛 서버의 이벤트 태스크를 붙잡는다 — 진단 저장은 이 뒤에 같은 알림을 받는다.
                tokio::task::block_in_place(|| {
                    let _ = rx.recv_timeout(Duration::from_secs(20));
                });
            });
        }
    });
    let file = env.file("src/lib.rs", "");
    env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: incremental(),
            ..Scenario::default()
        }
        .on(
            "textDocument/didOpen",
            vec![publish(&uri(&file), None, vec![diag(0, 1, "fresh")])],
        )
        .on(
            "textDocument/didChange",
            vec![publish(&uri(&file), None, vec![diag(0, 1, "late")])],
        ),
    );
    let mut events = subscribe(&env);
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Rust", "").await);
    next_for(&mut events, &file).await;
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "x" }] }),
    )
    .await
    .unwrap();
    assert!(
        eventually(Duration::from_secs(5), || held.load(Ordering::SeqCst) == 1).await,
        "the old server's late notification never arrived"
    );
    let root = env.manager.status(&env.id).instances[0].root.clone();
    win.call(
        "lsp.server.restart",
        json!({ "projectId": env.id, "server": "rust-analyzer", "root": root }),
    )
    .await
    .unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    // 옛 진단을 지운 빈 이벤트 뒤에 새 서버가 문서를 받고 「fresh」 를 낸다.
    loop {
        let event = next_for(&mut events, &file).await;
        if messages(&event["diagnostics"]) == ["fresh"] {
            break;
        }
    }
    // 이제 옛 서버의 「late」 를 놓는다 — 인스턴스에는 새 서버가 있고 세대만 다르다.
    go_tx.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(listed(&mut win, &env, &file).await, ["fresh"]);
}

/// 당기는 동안 온 refresh 는 잃지 않는다 — 그 당기기가 끝난 뒤 한 번 더 당긴다(마지막 refresh 뒤에 pull 이 있다). 몰려 와도 refresh
/// 수만큼 당기지는 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_during_a_pull_is_pulled_once_after_it() {
    let env = Env::new(EnvOptions::default());
    let file = env.file("src/lib.rs", "");
    let mut spaced = Vec::new();
    for _ in 0..8 {
        spaced.push(Action::Request {
            method: "workspace/diagnostic/refresh".into(),
            params: Value::Null,
        });
        spaced.push(Action::Sleep { ms: 50 });
    }
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: pull_capable(),
            ..Scenario::default()
        }
        .reply(
            "textDocument/diagnostic",
            Reply::result(json!({ "kind": "full", "items": [diag(0, 1, "pulled")] })).delayed(150),
        )
        .on("textDocument/didOpen", spaced),
    );
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    // 처리기가 refresh 여덟에 모두 답한 뒤 당기기가 가라앉을 때까지.
    let answered = |m: &Value| m.get("method").is_none() && m["result"].is_null();
    assert!(
        eventually(Duration::from_secs(10), || {
            received(&record).iter().filter(|m| answered(m)).count() >= 8
        })
        .await
    );
    tokio::time::sleep(Duration::from_millis(800)).await;
    let messages = received(&record);
    let last_refresh = messages.iter().rposition(answered).unwrap();
    let last_pull = messages
        .iter()
        .rposition(|m| m["method"] == "textDocument/diagnostic")
        .unwrap();
    assert!(
        last_pull > last_refresh,
        "no pull after the last refresh: {messages:?}"
    );
    let pulls = methods_of(&record, "textDocument/diagnostic").len();
    assert!(pulls < 1 + 8, "{pulls} pulls for eight refreshes");
}
