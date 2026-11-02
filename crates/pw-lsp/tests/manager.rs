//! 서버 관리자를 가짜 서버로 끝까지(15-m4-plan.md M4-05 완료 기준 ① ~ ⑪ · ⓐ · ⓑ). RPC 는 실제 라우터 · 세션으로 부르고, 서버
//! 정의는 내장 번들(rust-analyzer · basedpyright · ruff · typescript-language-server)에 설정 `[lsp.<id>] path` 로 가짜 서버를 꽂는다.
//! 로그인 셸 · 런타임은 가짜다(사람의 환경 · mise 를 읽지 않는다).

#![cfg(unix)]

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pw_core::Filter;
use pw_core::bus::{Received, Scope};
use pw_core::task::{TOPIC_FINISHED, TOPIC_PROGRESS};
use pw_lsp::fake::{Action, Reply, Scenario};
use pw_lsp::manager::ManagerOptions;
use pw_lsp::methods::{LspInstanceState, TOPIC_STATUS};
use pw_rpc::{Caller, ConnectionScope, Role, code};
use serde_json::{Value, json};

use common::env::{Env, EnvOptions, executable};
use common::{alive, eventually, group_members, received};

fn doc_id(opened: &Value) -> String {
    opened["docId"].as_str().unwrap().to_owned()
}

fn methods_of(record: &Path, method: &str) -> Vec<Value> {
    received(record)
        .into_iter()
        .filter(|m| m["method"] == method)
        .collect()
}

/// 증분 동기화(2)를 알리고 hover 를 다루는 가짜 서버.
fn incremental() -> Scenario {
    Scenario {
        capabilities: json!({ "textDocumentSync": 2, "hoverProvider": true }),
        ..Scenario::default()
    }
}

// ---- ① 신뢰 ---------------------------------------------------------------------------------------------------------------------

/// ① 미신뢰 프로젝트의 open 은 `untrusted` 이고 서버가 뜨지 않는다 → 신뢰하면 다음 open 에 뜬다 → 신뢰를 풀면 1 s 안에 끝난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_starts_nothing_until_trusted_and_revoking_ends_it_within_1_s() {
    let env = Env::new(EnvOptions {
        trusted: false,
        ..EnvOptions::default()
    });
    let record = env.fake("rust-analyzer", incremental());
    let file = env.file("src/main.rs", "fn main() {}\n");
    let mut win = env.window();
    let opened = win.open(&env, &file, "Rust", "fn main() {}\n").await;
    let lib = env.file("src/lib.rs", "");
    win.open(&env, &lib, "Rust", "").await;
    assert_eq!(
        opened["instances"][0]["state"]["kind"], "untrusted",
        "{opened}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(env.manager.spawn_count(), 0);
    assert!(received(&record).is_empty());

    env.projects.set_trust(&env.id, true).unwrap();
    let again = win.open(&env, &file, "Rust", "fn main() {}\n").await;
    assert_ne!(again["instances"][0]["state"]["kind"], "untrusted");
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert_eq!(env.manager.spawn_count(), 1);
    // 신뢰 전에 연 문서(lib.rs)도 서버가 뜨면 열린다. main.rs 는 다시 연 것 하나만. 가짜 서버가 기록 파일에 적을 때까지 기다린다.
    eventually(Duration::from_secs(2), || {
        methods_of(&record, "textDocument/didOpen").len() >= 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let opened: Vec<Value> = methods_of(&record, "textDocument/didOpen");
    assert_eq!(opened.len(), 2, "{opened:?}");
    let pid = env.manager.pids(&env.id)[0];

    let revoked = std::time::Instant::now();
    env.projects.set_trust(&env.id, false).unwrap();
    assert!(
        eventually(Duration::from_secs(1), || !alive(pid)).await,
        "the server outlived the trust by 1 s"
    );
    eprintln!("ended {:?} after the trust was revoked", revoked.elapsed());
    env.wait_state("rust-analyzer", LspInstanceState::Untrusted)
        .await;
    assert_eq!(env.manager.spawn_count(), 1);
}

/// ① 물러남 재기동도 허가를 다시 받는다 — 신뢰 해제 알림을 따르지 않는 관리자에서, 신뢰를 거둔 뒤 서버가 죽으면 다시 띄우지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_backoff_restart_asks_for_the_grant_again() {
    let env = Env::new(EnvOptions {
        follow: false,
        ..EnvOptions::default()
    });
    env.fake(
        "rust-analyzer",
        incremental().on(
            "initialized",
            vec![Action::Sleep { ms: 400 }, Action::Exit { code: 3 }],
        ),
    );
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    win.open(&env, &file, "rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    env.projects.set_trust(&env.id, false).unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::Untrusted)
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(env.manager.spawn_count(), 1, "{:?}", env.states());
}

// ---- ② 인스턴스 --------------------------------------------------------------------------------------------------------------

/// ② 같은 루트의 문서 둘은 인스턴스 하나, root_markers 가 다른 하위 폴더 둘은 인스턴스 둘.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_instance_per_workspace_root() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", incremental());
    env.file("a/Cargo.toml", "");
    env.file("b/Cargo.toml", "");
    let x = env.file("a/src/x.rs", "");
    let y = env.file("a/src/deep/y.rs", "");
    let z = env.file("b/src/z.rs", "");
    let mut win = env.window();
    win.open(&env, &x, "Rust", "").await;
    win.open(&env, &y, "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let roots = |env: &Env| -> Vec<String> {
        env.manager
            .status(&env.id)
            .instances
            .into_iter()
            .map(|i| i.root)
            .collect()
    };
    assert_eq!(roots(&env), [env.root.join("a").display().to_string()]);
    win.open(&env, &z, "Rust", "").await;
    assert!(
        eventually(Duration::from_secs(10), || env
            .states()
            .iter()
            .filter(|(_, s)| *s == LspInstanceState::Ready)
            .count()
            == 2)
        .await
    );
    assert_eq!(
        roots(&env),
        [
            env.root.join("a").display().to_string(),
            env.root.join("b").display().to_string()
        ]
    );
    assert_eq!(env.manager.spawn_count(), 2);
}

// ---- ③ 동기화 ---------------------------------------------------------------------------------------------------------------

/// 작은 의사 난수(재현 가능).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// UTF-16 경계(대리쌍 가운데가 아닌 오프셋)들.
fn boundaries(text: &str) -> Vec<(u32, usize)> {
    let mut out = vec![(0, 0)];
    let mut u = 0;
    for (i, ch) in text.char_indices() {
        u += ch.len_utf16() as u32;
        out.push((u, i + ch.len_utf8()));
    }
    out
}

/// LSP 위치 → 바이트. 시험이 LSP 3.17 「Text Documents」 의 문장대로 따로 계산한다 — 구현의 표를 쓰지 않는다: 줄 끝은 `\r\n` ·
/// `\r` · `\n`, 줄 길이를 넘는 `character` 는 줄 끝으로(M4-05 리뷰 M3 — 앞의 판은 구현처럼 `\n` 만 줄 끝으로 봐 이 차이를 못 봤다).
fn byte_at(text: &str, line: u64, character: u64) -> usize {
    let b = text.as_bytes();
    let (mut start, mut i, mut at) = (0, 0, 0);
    let end = loop {
        let ending = match b.get(i) {
            None => break b.len(),
            Some(b'\r') if b.get(i + 1) == Some(&b'\n') => 2,
            Some(b'\r' | b'\n') => 1,
            Some(_) => 0,
        };
        if ending == 0 {
            i += 1;
        } else if at == line {
            break i;
        } else {
            at += 1;
            i += ending;
            start = i;
        }
    };
    let mut u = 0;
    for (i, ch) in text[start..end].char_indices() {
        if u >= character {
            return start + i;
        }
        u += ch.len_utf16() as u64;
    }
    end
}

/// 서버가 받은 `didOpen` · `didChange` 를 차례로 적용한 텍스트.
fn replay(record: &Path) -> (String, i64) {
    let mut text = String::new();
    let mut version = -1;
    for msg in received(record) {
        match msg["method"].as_str() {
            Some("textDocument/didOpen") => {
                text = msg["params"]["textDocument"]["text"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                version = msg["params"]["textDocument"]["version"].as_i64().unwrap();
            }
            Some("textDocument/didChange") => {
                let v = msg["params"]["textDocument"]["version"].as_i64().unwrap();
                assert!(v > version, "versions must rise: {v} after {version}");
                version = v;
                for change in msg["params"]["contentChanges"].as_array().unwrap() {
                    let insert = change["text"].as_str().unwrap();
                    match change.get("range") {
                        None => text = insert.to_owned(),
                        Some(range) => {
                            let at = |p: &Value| {
                                byte_at(
                                    &text,
                                    p["line"].as_u64().unwrap(),
                                    p["character"].as_u64().unwrap(),
                                )
                            };
                            let (from, to) = (at(&range["start"]), at(&range["end"]));
                            text.replace_range(from..to, insert);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (text, version)
}

async fn thousand_edits(sync: u8) {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            capabilities: json!({ "textDocumentSync": sync }),
            ..Scenario::default()
        },
    );
    let file = env.file("src/lib.rs", "");
    let mut text = String::from("// 한글 주석 🦀\r\nfn main() {\n    let s = \"😀👍🏽\";\r}\n");
    let mut win = env.window();
    let opened = win.open(&env, &file, "Rust", &text).await;
    let doc = doc_id(&opened);
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    // 섞인 줄바꿈(`\r` · `\r\n` · `\n`) — 웹뷰는 섞인 문서의 `\r` 을 글자로 남긴다(리뷰 M3).
    let pool = [
        "한", "🦀", "a", "\n", "글자 ", "😀x", "", "\r\n", "👍🏽", "\r", "x\r",
    ];
    let mut rng = Rng(0x5eed_1234_abcd);
    for version in 1..=1000u32 {
        let b = boundaries(&text);
        let i = rng.below(b.len());
        let j = (i + rng.below(4)).min(b.len() - 1);
        let insert = pool[rng.below(pool.len())];
        let (from, to) = (b[i], b[j]);
        text.replace_range(from.1..to.1, insert);
        win.call(
            "lsp.document.change",
            json!({ "docId": doc, "version": version, "changes": [{ "from": from.0, "to": to.0, "insert": insert }] }),
        )
        .await
        .unwrap();
    }
    assert!(
        eventually(Duration::from_secs(10), || replay(&record).1 == 1000).await,
        "the server did not get all edits"
    );
    let (got, _) = replay(&record);
    assert_eq!(got, text);
    let changes = methods_of(&record, "textDocument/didChange");
    if sync == 2 {
        assert_eq!(changes.len(), 1000);
    } else {
        // `Full` 서버에는 범위 없는 전체 텍스트만 간다.
        assert!(changes.iter().all(|m| {
            m["params"]["contentChanges"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c.get("range").is_none())
        }));
    }
}

/// ③ 증분 편집 1,000번 뒤 서버가 받은 텍스트가 웹뷰 텍스트와 같다(한글 · 이모지 — 대리쌍).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thousand_incremental_edits_reach_an_incremental_server_intact() {
    thousand_edits(2).await;
}

/// ③ `Full` 동기화 서버도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thousand_edits_reach_a_full_sync_server_intact() {
    thousand_edits(1).await;
}

/// 앞당겨 온 편집은 차례가 오면 적용되고, 빠진 편집은 conflict 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn early_edits_wait_for_their_turn() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake("rust-analyzer", incremental());
    let file = env.file("src/lib.rs", "");
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Rust", "ab").await);
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let change = |v: u32, from: u32, insert: &str| json!({ "docId": doc, "version": v, "changes": [{ "from": from, "to": from, "insert": insert }] });
    win.call("lsp.document.change", change(2, 3, "d"))
        .await
        .unwrap();
    win.call("lsp.document.change", change(1, 2, "c"))
        .await
        .unwrap();
    assert!(
        eventually(Duration::from_secs(5), || replay(&record)
            == ("abcd".to_owned(), 2))
        .await
    );
    let e = win
        .call("lsp.document.change", change(2, 0, "x"))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::CONFLICT);
    // 다른 연결은 그 문서를 모른다.
    let mut other = env.window();
    let e = other
        .call("lsp.document.change", change(3, 0, "x"))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
}

/// 같은 연결이 같은 파일을 다시 열면 앞 문서를 닫고 연다 — 서버는 같은 URI 의 `didOpen` 을 겹쳐 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopening_a_path_in_the_same_window_replaces_the_document() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake("rust-analyzer", incremental());
    let file = env.file("src/lib.rs", "");
    let mut win = env.window();
    let first = doc_id(&win.open(&env, &file, "Rust", "one").await);
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let second = doc_id(&win.open(&env, &file, "Rust", "two").await);
    assert_ne!(first, second);
    assert!(
        eventually(Duration::from_secs(2), || methods_of(
            &record,
            "textDocument/didOpen"
        )
        .len()
            == 2)
        .await
    );
    let kinds: Vec<String> = received(&record)
        .into_iter()
        .filter_map(|m| m["method"].as_str().map(str::to_owned))
        .filter(|m| m.starts_with("textDocument/did"))
        .collect();
    assert_eq!(
        kinds,
        [
            "textDocument/didOpen",
            "textDocument/didClose",
            "textDocument/didOpen"
        ]
    );
    let e = win
        .call(
            "lsp.document.change",
            json!({ "docId": first, "version": 1, "changes": [] }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
}

// ---- ④ 유휴 ---------------------------------------------------------------------------------------------------------------------

/// ④ 마지막 문서를 닫고 유휴 시한 뒤 끝난다. 그사이 다시 열면 그대로 쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_ends_after_the_idle_time_and_a_reopen_keeps_it() {
    let env = Env::new(EnvOptions {
        manager: ManagerOptions {
            idle_shutdown: Some(Duration::from_millis(400)),
            ..EnvOptions::default().manager
        },
        ..EnvOptions::default()
    });
    let record = env.fake("rust-analyzer", incremental());
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Rust", "").await);
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let pid = env.manager.pids(&env.id)[0];
    win.call("lsp.document.close", json!({ "docId": doc }))
        .await
        .unwrap();
    assert!(
        eventually(Duration::from_secs(2), || methods_of(
            &record,
            "textDocument/didClose"
        )
        .len()
            == 1)
        .await
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let doc = doc_id(&win.open(&env, &file, "Rust", "").await);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(alive(pid));
    assert_eq!(env.manager.pids(&env.id), [pid]);
    assert_eq!(env.manager.spawn_count(), 1);
    win.call("lsp.document.close", json!({ "docId": doc }))
        .await
        .unwrap();
    assert!(eventually(Duration::from_secs(3), || !alive(pid)).await);
    assert!(env.manager.status(&env.id).instances.is_empty());
    assert_eq!(
        methods_of(&record, "shutdown").len(),
        1,
        "it should end politely"
    );
}

// ---- ⑤ 죽음 ---------------------------------------------------------------------------------------------------------------------

/// ⑤ 3분 안에 5번 죽으면 `crashed` 이고 더 띄우지 않는다. `lsp.server.restart` 만 다시 띄운다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_deaths_stop_the_restarts_until_a_restart() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        incremental().on(
            "initialized",
            vec![Action::Sleep { ms: 30 }, Action::Exit { code: 1 }],
        ),
    );
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Crashed)
        .await;
    assert_eq!(env.manager.spawn_count(), 5);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(env.manager.spawn_count(), 5);
    // 문서를 다시 열어도 띄우지 않는다.
    win.open(&env, &file, "Rust", "").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(env.manager.spawn_count(), 5);
    let root = env.manager.status(&env.id).instances[0].root.clone();
    let restarted = win
        .call(
            "lsp.server.restart",
            json!({ "projectId": env.id, "server": "rust-analyzer", "root": root }),
        )
        .await
        .unwrap();
    assert_eq!(restarted["state"]["kind"], "starting");
    assert!(eventually(Duration::from_secs(5), || env.manager.spawn_count() >= 6).await);
}

/// `initialize` 에 실패하는 서버도 죽음으로 센다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_failing_initialize_counts_as_a_death() {
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        incremental().reply(
            "initialize",
            Reply {
                error: Some(json!({ "code": -32603, "message": "no" })),
                ..Reply::default()
            },
        ),
    );
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Crashed)
        .await;
    assert_eq!(env.manager.spawn_count(), 5);
}

// ---- ⑥ 창 닫힘 ------------------------------------------------------------------------------------------------------------------

/// ⑥ 창(연결)을 닫으면 2 s 안에 그 창의 서버 pid(와 그 자손)가 모두 없다. 다른 창의 서버는 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_window_ends_its_servers_within_2_s() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        Scenario {
            start: vec![Action::SpawnChild { seconds: 60 }],
            ..incremental()
        },
    );
    env.file("a/Cargo.toml", "");
    env.file("b/Cargo.toml", "");
    env.file("c/Cargo.toml", "");
    let mut win = env.window();
    win.open(&env, &env.file("a/x.rs", ""), "Rust", "").await;
    win.open(&env, &env.file("b/y.rs", ""), "Rust", "").await;
    // 같은 프로젝트의 다른 연결(다른 창)이 연 서버.
    let mut second = env.window();
    second.open(&env, &env.file("c/z.rs", ""), "Rust", "").await;
    assert!(
        eventually(Duration::from_secs(10), || env.manager.pids(&env.id).len()
            == 3)
        .await
    );
    let children: Vec<u32> = received(&record)
        .iter()
        .filter_map(|m| m["childPid"].as_u64())
        .map(|p| p as u32)
        .collect();
    assert_eq!(children.len(), 3);
    let c_root = env.root.join("c").display().to_string();
    let survivor = env
        .manager
        .status(&env.id)
        .instances
        .iter()
        .position(|i| i.root == c_root)
        .unwrap();
    let pids = env.manager.pids(&env.id);
    let kept = pids[survivor];
    let gone: Vec<u32> = pids.iter().copied().filter(|p| *p != kept).collect();
    let closed = std::time::Instant::now();
    win.session.close();
    assert!(
        eventually(Duration::from_secs(2), || gone
            .iter()
            .all(|p| !alive(*p) && group_members(*p).is_empty()))
        .await,
        "left: {:?}",
        gone.iter()
            .map(|p| (p, group_members(*p)))
            .collect::<Vec<_>>()
    );
    eprintln!(
        "the window's servers ended {:?} after it closed",
        closed.elapsed()
    );
    assert!(alive(kept));
    assert_eq!(env.manager.pids(&env.id), [kept]);
    second.session.close();
    assert!(eventually(Duration::from_secs(2), || !alive(kept)).await);
    // 자식(서버가 띄운 것)도 모두 끝났다.
    assert!(
        eventually(Duration::from_secs(1), || children
            .iter()
            .all(|c| !alive(*c)))
        .await
    );
}

// ---- ⑦ 런타임 · 설정 변경 -------------------------------------------------------------------------------------------------------

/// ⑦ `runtime.changed` 뒤 `restartSuggested` 이고 서버는 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_change_suggests_a_restart_and_keeps_the_server() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", incremental());
    let mut win = env.window();
    win.open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let pid = env.manager.pids(&env.id)[0];
    // 다른 프로젝트의 런타임 변경은 이 인스턴스에 닿지 않는다.
    env.bus
        .publish(
            Scope::Project(env.other.clone()),
            "runtime.changed",
            &json!({ "reason": "reload" }),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(env.state_of("rust-analyzer"), Some(LspInstanceState::Ready));
    env.bus
        .publish(
            Scope::Project(env.id.clone()),
            "runtime.changed",
            &json!({ "reason": "files" }),
        )
        .unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::RestartSuggested)
        .await;
    assert!(alive(pid));
    assert_eq!(env.manager.pids(&env.id), [pid]);
    assert_eq!(env.manager.spawn_count(), 1);
}

/// 설정이 바뀌면 그 서버의 인스턴스가 `restartNeeded` 다(저절로 다시 띄우지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_settings_change_marks_the_instance_restart_needed() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", incremental());
    let mut win = env.window();
    win.open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let settings = env.paths.config.join("settings.toml");
    let text = fs::read_to_string(&settings).unwrap();
    fs::write(
        &settings,
        format!("{text}[lsp.rust-analyzer.settings.rust-analyzer]\ncheckOnSave = false\n"),
    )
    .unwrap();
    env.global.reload().unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::RestartNeeded)
        .await;
    assert_eq!(env.manager.spawn_count(), 1);
}

// ---- ⑧ 연결 범위 ---------------------------------------------------------------------------------------------------------------

/// ⑧ 다른 프로젝트 창 · 웰컴(Host) · 파일 창 연결의 open 은 permission denied 이고 서버가 뜨지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_projects_own_window_opens_documents() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake("rust-analyzer", incremental());
    let file = env.file("src/main.rs", "fn main() {}\n");
    let file_scope = Caller::file(Role::Owner, file.clone()).scope;
    for scope in [
        ConnectionScope::Project(env.other.clone()),
        ConnectionScope::Host,
        file_scope,
    ] {
        let mut conn = env.connect(scope.clone());
        let e = conn
            .call(
                "lsp.document.open",
                json!({ "projectId": env.id, "path": file, "language": "Rust", "version": 0, "text": "" }),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}: {e:?}");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(env.manager.spawn_count(), 0);
    assert!(received(&record).is_empty());
    assert!(env.manager.status(&env.id).instances.is_empty());
    // 프로젝트 루트 밖 경로는 invalid params.
    let mut win = env.window();
    let outside = env.base.join("p2/x.rs");
    fs::write(&outside, "").unwrap();
    let e = win
        .call(
            "lsp.document.open",
            json!({ "projectId": env.id, "path": outside, "language": "Rust", "version": 0, "text": "" }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

// ---- ⑨ 편집 등급 ---------------------------------------------------------------------------------------------------------------

/// ⑨ 5 MiB 를 넘는 문서 · 1만 자 줄은 서버에 가지 않는다 — 열 때도, 편집으로 넘을 때도.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn documents_over_the_edit_tier_never_reach_the_server() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake("rust-analyzer", incremental());
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    let small = win.open(&env, &file, "Rust", "fn main() {}\n").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let big = "x\n".repeat(5 * 1024 * 1024 / 2 + 1);
    let opened = win.open(&env, &file, "Rust", &big).await;
    assert_eq!(opened["off"], "tooLarge");
    let long = "y".repeat(10_001);
    assert_eq!(
        win.open(&env, &file, "Rust", &long).await["off"],
        "longLine"
    );
    // 편집으로 넘으면 서버에서 닫고 그 편집은 보내지 않는다.
    let doc = doc_id(&small);
    let result = win
        .call(
            "lsp.document.change",
            json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": big }] }),
        )
        .await
        .unwrap();
    assert_eq!(result["off"], "tooLarge");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(methods_of(&record, "textDocument/didOpen").len(), 1);
    assert!(methods_of(&record, "textDocument/didChange").is_empty());
    assert_eq!(methods_of(&record, "textDocument/didClose").len(), 1);
    let e = win
        .call(
            "lsp.document.change",
            json!({ "docId": doc, "version": 2, "changes": [] }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
}

// ---- ⑩ 요청 길 ------------------------------------------------------------------------------------------------------------------

fn hover_scenario(delay: u64) -> Scenario {
    incremental().reply(
        "textDocument/hover",
        Reply::result(json!({ "contents": "docs" })).delayed(delay),
    )
}

fn feature(doc: &str, feature: &str, method: &str, offset: u32, version: Option<u32>) -> Value {
    json!({ "docId": doc, "feature": feature, "method": method, "offset": offset, "version": version })
}

/// ⑩ 요청 사이 편집이 끼면 결과에 `staleVersion`. 끼지 않으면 아니다. 위치는 UTF-16 으로 바뀌어 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_between_request_and_answer_marks_it_stale() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake("rust-analyzer", hover_scenario(300));
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Rust", "// 🦀\nlet 한 = 1;\n").await);
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    // 「한」 은 둘째 줄의 4번째 UTF-16 단위.
    let fresh = win
        .call(
            "test.lsp.feature",
            feature(&doc, "hover", "textDocument/hover", 10, Some(0)),
        )
        .await
        .unwrap();
    assert_eq!(fresh["staleVersion"], false);
    assert_eq!(fresh["server"], "rust-analyzer");
    assert_eq!(fresh["result"]["contents"], "docs");
    let asked = methods_of(&record, "textDocument/hover");
    assert_eq!(
        asked[0]["params"]["position"],
        json!({ "line": 1, "character": 4 })
    );

    let pending = win.send(
        "test.lsp.feature",
        feature(&doc, "hover", "textDocument/hover", 10, Some(0)),
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    win.call(
        "lsp.document.change",
        json!({ "docId": doc, "version": 1, "changes": [{ "from": 0, "to": 0, "insert": "\n" }] }),
    )
    .await
    .unwrap();
    let stale = win.response(pending).await.unwrap();
    assert_eq!(stale["staleVersion"], true);
    // 부른 쪽이 계산한 버전이 이미 지났어도.
    let behind = win
        .call(
            "test.lsp.feature",
            feature(&doc, "hover", "textDocument/hover", 10, Some(0)),
        )
        .await
        .unwrap();
    assert_eq!(behind["staleVersion"], true);
}

/// `ContentModified` 는 한 번만 다시 묻고, 그래도면 서버 오류(5002)다. 능력이 없는 기능은 5001.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_modified_is_asked_once_more_then_reported() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        incremental().reply(
            "textDocument/hover",
            Reply {
                error: Some(json!({ "code": -32801, "message": "content modified" })),
                ..Reply::default()
            },
        ),
    );
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Rust", "fn x() {}").await);
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let e = win
        .call(
            "test.lsp.feature",
            feature(&doc, "hover", "textDocument/hover", 0, None),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, pw_lsp::manager::code::SERVER_ERROR);
    assert_eq!(methods_of(&record, "textDocument/hover").len(), 2);
    let e = win
        .call(
            "test.lsp.feature",
            feature(&doc, "rename", "textDocument/rename", 0, None),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, pw_lsp::manager::code::NO_SERVER, "{e:?}");
}

/// 결정 M4-D12: 문서는 모든 서버에 동기화하고, 기능은 능력이 있고 끄지 않은 첫 서버가 맡는다. basedpyright 는 정의에서
/// 포맷을 끈다 — 포맷은 능력이 있어도 ruff 로 간다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn features_go_to_the_first_capable_enabled_server() {
    let env = Env::new(EnvOptions::default());
    let both =
        json!({ "textDocumentSync": 2, "hoverProvider": true, "documentFormattingProvider": true });
    let pyright = env.fake(
        "basedpyright",
        Scenario {
            capabilities: both.clone(),
            ..Scenario::default()
        }
        .reply("textDocument/hover", Reply::result(json!("pyright")))
        .reply("textDocument/formatting", Reply::result(json!("pyright"))),
    );
    let ruff = env.fake(
        "ruff",
        Scenario {
            capabilities: both,
            ..Scenario::default()
        }
        .reply("textDocument/hover", Reply::result(json!("ruff")))
        .reply("textDocument/formatting", Reply::result(json!("ruff"))),
    );
    let file = env.file("app.py", "");
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Python", "x = 1\n").await);
    env.wait_state("basedpyright", LspInstanceState::Ready)
        .await;
    env.wait_state("ruff", LspInstanceState::Ready).await;
    for record in [&pyright, &ruff] {
        assert_eq!(methods_of(record, "textDocument/didOpen").len(), 1);
    }
    let hover = win
        .call(
            "test.lsp.feature",
            feature(&doc, "hover", "textDocument/hover", 0, None),
        )
        .await
        .unwrap();
    assert_eq!(
        (hover["server"].clone(), hover["result"].clone()),
        (json!("basedpyright"), json!("pyright"))
    );
    let format = win
        .call(
            "test.lsp.feature",
            feature(&doc, "formatting", "textDocument/formatting", 0, None),
        )
        .await
        .unwrap();
    assert_eq!(
        (format["server"].clone(), format["result"].clone()),
        (json!("ruff"), json!("ruff"))
    );
}

/// 서버 런타임(`requires`)을 찾지 못하면 `runtimeMissing` 이고 띄우지 않는다 — TS 서버는 node ≥ 20 이 필요한데 가짜 셸의 PATH 에
/// node 가 없다. 찾으면(basedpyright 의 python ≥ 3.10) 뜬다(위 시험).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_server_runtime_is_reported_and_nothing_starts() {
    let env = Env::new(EnvOptions::default());
    env.fake("typescript-language-server", incremental());
    let mut win = env.window();
    win.open(&env, &env.file("src/a.ts", ""), "TypeScript", "")
        .await;
    env.wait_state(
        "typescript-language-server",
        LspInstanceState::RuntimeMissing {
            tool: "node".into(),
            needed: ">=20".into(),
        },
    )
    .await;
    assert_eq!(env.manager.spawn_count(), 0);
    // 런타임을 두면 다음 open 에 뜬다.
    executable(&env.bin.join("node"), "echo v22.11.0");
    win.open(&env, &env.file("src/b.ts", ""), "TypeScript", "")
        .await;
    env.wait_state("typescript-language-server", LspInstanceState::Ready)
        .await;
}

/// 설치되지 않은 서버는 `notInstalled`(Mason 이름과 함께)다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unresolved_server_is_not_installed() {
    let env = Env::new(EnvOptions::default());
    let mut win = env.window();
    let opened = win.open(&env, &env.file("Main.kt", ""), "Kotlin", "").await;
    assert_eq!(opened["instances"][0]["server"], "kotlin-lsp");
    env.wait_state(
        "kotlin-lsp",
        LspInstanceState::NotInstalled {
            mason: Some("kotlin-lsp".into()),
        },
    )
    .await;
    assert_eq!(env.manager.spawn_count(), 0);
}

// ---- 진행 · 설정 요청 · 상태 이벤트 ----------------------------------------------------------------------------------------------

/// `$/progress` 가 프로젝트 범위 `Task` 로 나왔다 끝나고, 상태의 `progress` 에 보인다. `workspace/configuration` 은 합친 설정에서 답한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn progress_becomes_a_task_and_configuration_is_answered() {
    let env = Env::new(EnvOptions::default());
    let progress = |kind: &str, extra: Value| {
        let mut value = extra;
        value["kind"] = json!(kind);
        Action::Notify {
            method: "$/progress".into(),
            params: json!({ "token": "idx", "value": value }),
        }
    };
    let record = env.fake(
        "rust-analyzer",
        incremental().on(
            "initialized",
            vec![
                Action::Request {
                    method: "workspace/configuration".into(),
                    params: json!({ "items": [{ "section": "rust-analyzer.check" }, {}] }),
                },
                progress("begin", json!({ "title": "Indexing", "percentage": 0 })),
                Action::Sleep { ms: 300 },
                progress("report", json!({ "percentage": 42, "message": "42/100" })),
                Action::Sleep { ms: 300 },
                progress("end", json!({})),
            ],
        ),
    );
    let settings = env.paths.config.join("settings.toml");
    let text = fs::read_to_string(&settings).unwrap();
    fs::write(
        &settings,
        format!("{text}[lsp.rust-analyzer.settings.rust-analyzer.check]\ncommand = \"clippy\"\n"),
    )
    .unwrap();
    env.global.reload().unwrap();
    let mut events =
        env.bus
            .subscribe(Filter::all().topics([TOPIC_PROGRESS, TOPIC_FINISHED, TOPIC_STATUS]));
    let mut win = env.window();
    win.open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    let mut seen = Vec::new();
    let mut statuses = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !seen
        .iter()
        .any(|(t, _): &(String, Value)| t == TOPIC_FINISHED)
    {
        let Ok(Some(Received::Event(event))) =
            tokio::time::timeout_at(deadline, events.recv()).await
        else {
            panic!("no task finished: {seen:?}");
        };
        if event.topic == TOPIC_STATUS {
            statuses.push(event.payload.clone());
        } else {
            assert_eq!(event.scope, Scope::Project(env.id.clone()));
            seen.push((event.topic.clone(), event.payload.clone()));
        }
    }
    assert!(
        seen.iter().any(|(t, p)| t == TOPIC_PROGRESS
            && p["title"] == "Indexing"
            && p["progress"]["done"] == 42
            && p["progress"]["total"] == 100),
        "{seen:?}"
    );
    let finished = seen.iter().find(|(t, _)| t == TOPIC_FINISHED).unwrap();
    assert_eq!(finished.1["status"], "completed");
    assert!(
        statuses
            .iter()
            .any(|s| s["instances"][0]["progress"]["percentage"] == 42),
        "{statuses:?}"
    );
    let answers: Vec<Value> = received(&record)
        .into_iter()
        .filter(|m| m["id"] == "fake-1")
        .collect();
    assert_eq!(
        answers[0]["result"],
        json!([{ "command": "clippy" }, { "rust-analyzer": { "check": { "command": "clippy" } } }])
    );
}

/// 사람이 작업을 취소하면 서버에 `window/workDoneProgress/cancel` 이 간다(서버가 취소를 받는다고 알린 진행만).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_task_cancels_the_servers_progress() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake(
        "rust-analyzer",
        incremental().on(
            "initialized",
            vec![Action::Notify {
                method: "$/progress".into(),
                params: json!({ "token": 7, "value": { "kind": "begin", "title": "Building", "cancellable": true } }),
            }],
        ),
    );
    let tasks = env.tasks.clone();
    let mut win = env.window();
    win.open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    assert!(
        eventually(Duration::from_secs(5), || !tasks
            .running(Some(&env.id))
            .is_empty())
        .await
    );
    let task = tasks.running(Some(&env.id))[0].clone();
    assert_eq!(task.title, "Building");
    assert!(tasks.cancel(task.task_id));
    assert!(
        eventually(Duration::from_secs(5), || !methods_of(
            &record,
            "window/workDoneProgress/cancel"
        )
        .is_empty())
        .await
    );
    assert_eq!(
        methods_of(&record, "window/workDoneProgress/cancel")[0]["params"],
        json!({ "token": 7 })
    );
}

/// `lsp.server.stop` 은 `stopped` 로 두고 문서를 다시 열어도 띄우지 않는다. 띄운 서버는 기록부(`$STATE/lsp/running`)에 있다가
/// 끝나면 빠진다(Host 시작 때의 거두기가 읽는 곳).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_holds_until_restart_and_the_ledger_follows_the_server() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", incremental());
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let pid = env.manager.pids(&env.id)[0];
    let ledger = pw_lsp::ledger::ledger_dir(&env.paths.state);
    let listed = |pid: u32| ledger.join(pid.to_string()).exists();
    assert!(listed(pid));
    let root = env.manager.status(&env.id).instances[0].root.clone();
    let control = json!({ "projectId": env.id, "server": "rust-analyzer", "root": root });
    let stopped = win.call("lsp.server.stop", control.clone()).await.unwrap();
    assert_eq!(stopped["state"]["kind"], "stopped");
    assert!(!alive(pid));
    assert!(eventually(Duration::from_secs(2), || !listed(pid)).await);
    win.open(&env, &file, "Rust", "").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(env.manager.spawn_count(), 1);
    // 다른 창은 멈추거나 다시 띄우지 못한다.
    let mut other = env.connect(ConnectionScope::Project(env.other.clone()));
    let e = other
        .call("lsp.server.restart", control.clone())
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
    win.call("lsp.server.restart", control).await.unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert_eq!(env.manager.spawn_count(), 2);
}

// ---- ⓐ 서버마다의 이벤트 채널 ------------------------------------------------------------------------------------------------

/// ⓐ(M4-01 재리뷰 N2) 이벤트 채널은 서버마다 따로다 — 한 서버(ruff)가 자기 채널을 꽉 채운 동안 다른 서버(basedpyright)가 알림을
/// 내도 끝나지 않는다. 받는 쪽(알림 처리기)을 일부러 느리게 해 ruff 가 채널을 정확히 채우게 한다(처리 중 하나 + 대기 4 — 넘지
/// 않아 ruff 도 살아 있다). 채널을 나누면 그때 알림을 낸 basedpyright 가 끝난다(되돌림 실증 R20).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_server_filling_its_events_does_not_end_another() {
    let slow = Arc::new(AtomicBool::new(true));
    let s = Arc::clone(&slow);
    let env = Env::with_listener(
        EnvOptions {
            manager: ManagerOptions {
                event_capacity: 4,
                ..EnvOptions::default().manager
            },
            ..EnvOptions::default()
        },
        move |manager| {
            // 받는 쪽을 느리게 — ruff 의 채널이 찬 채로 있다.
            manager.on_notification("noise/x", move |_, _| {
                // 일꾼 스레드를 막지 않고(`block_in_place` — 그 일꾼의 다른 태스크는 다른 스레드로 옮겨진다) 이 서버의 이벤트
                // 태스크만 붙잡는다. 시험이 실패해도 런타임이 끝나도록 5 s 로 묶는다. 그 일꾼 스레드는 블로킹 풀로 넘어가 쉬다 끝날
                // 수 있지만 서버에는 닿지 않는다 — fork 는 전용 띄우기 스레드가 한다(`a_server_outlives_the_thread_that_asked_for_it`).
                tokio::task::block_in_place(|| {
                    let until = std::time::Instant::now() + Duration::from_secs(5);
                    while s.load(Ordering::SeqCst) && std::time::Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                });
            });
        },
    );
    let noise: Vec<Action> = (0..5)
        .map(|_| Action::Notify {
            method: "noise/x".into(),
            params: json!({}),
        })
        .collect();
    env.fake(
        "ruff",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2 }),
            ..Scenario::default()
        }
        .on("initialized", noise),
    );
    let quiet: Vec<Action> = (0..15)
        .flat_map(|_| {
            [
                Action::Sleep { ms: 100 },
                Action::Notify {
                    method: "quiet/x".into(),
                    params: json!({}),
                },
            ]
        })
        .collect();
    env.fake(
        "basedpyright",
        Scenario {
            capabilities: json!({ "textDocumentSync": 2, "hoverProvider": true }),
            ..Scenario::default()
        }
        .on("initialized", quiet)
        .reply("textDocument/hover", Reply::result(json!("alive"))),
    );
    let file = env.file("app.py", "");
    let mut win = env.window();
    let doc = doc_id(&win.open(&env, &file, "Python", "x = 1\n").await);
    env.wait_state("basedpyright", LspInstanceState::Ready)
        .await;
    env.wait_state("ruff", LspInstanceState::Ready).await;
    let quiet_pid = env.manager.pids_of(&env.id, Some("basedpyright"));
    let noisy_pid = env.manager.pids_of(&env.id, Some("ruff"));
    assert_eq!((quiet_pid.len(), noisy_pid.len()), (1, 1));
    // basedpyright 가 알림을 다 낼 때까지(15 × 100 ms) ruff 의 채널은 찬 채로 있다.
    tokio::time::sleep(Duration::from_millis(1800)).await;
    slow.store(false, Ordering::SeqCst);
    let hover = win
        .call(
            "test.lsp.feature",
            feature(&doc, "hover", "textDocument/hover", 0, None),
        )
        .await
        .unwrap();
    assert_eq!(hover["server"], "basedpyright");
    assert_eq!(hover["result"], "alive");
    // 아무도 끝나지 않았다(물러남으로 다시 뜬 것이 아니다).
    assert_eq!(
        env.manager.pids_of(&env.id, Some("basedpyright")),
        quiet_pid
    );
    assert_eq!(env.manager.pids_of(&env.id, Some("ruff")), noisy_pid);
    assert!(alive(quiet_pid[0]) && alive(noisy_pid[0]));
    assert_eq!(env.manager.spawn_count(), 2);
}

// ---- ⓑ Linux: 띄운 스레드 ----------------------------------------------------------------------------------------------------

/// ⓑ(M4-01 재리뷰 N3) 관리자를 거쳐 띄운 서버는 해석을 한 블로킹 스레드가 쉬다 끝나도(여기선 0.1 s) 산다. Linux 의
/// `PR_SET_PDEATHSIG` 는 fork 한 스레드가 끝날 때 신호를 보내므로, 그 스레드에서 fork 하면 이 시험이 실패한다(macOS 에는 없는
/// 증상이라 macOS 에서는 늘 통과한다 — CI ubuntu `check` 가 본다). 부른 스레드가 어디든 fork 는 전용 스레드가 한다는 것은
/// `a_server_outlives_the_thread_that_asked_for_it`(리뷰 M4).
#[test]
fn servers_outlive_the_blocking_threads_that_prepared_them() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_keep_alive(Duration::from_millis(100))
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let env = Env::new(EnvOptions::default());
        env.fake("rust-analyzer", hover_scenario(0));
        let mut win = env.window();
        let doc = doc_id(
            &win.open(&env, &env.file("src/main.rs", ""), "Rust", "x")
                .await,
        );
        env.wait_state("rust-analyzer", LspInstanceState::Ready)
            .await;
        let pid = env.manager.pids(&env.id)[0];
        // 블로킹 스레드가 모두 끝날 때까지.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            alive(pid),
            "the server died with the thread that started it"
        );
        assert_eq!(env.state_of("rust-analyzer"), Some(LspInstanceState::Ready));
        let hover = win
            .call(
                "test.lsp.feature",
                feature(&doc, "hover", "textDocument/hover", 0, None),
            )
            .await
            .unwrap();
        assert_eq!(hover["result"]["contents"], "docs");
        assert_eq!(env.manager.spawn_count(), 1);
    });
}

// ---- 설치본(M4-04) ------------------------------------------------------------------------------------------------------------

/// 설치기의 설치본 찾기(`InstalledTools`)로 해석한 서버가 뜨고, 도구를 지우기 전(`ToolUsers::stop` → `stop_installed`)에 그
/// 서버만 끝나 `notInstalled` 가 된다. PATH 등 다른 곳에서 찾은 서버는 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn servers_found_in_the_installed_tools_stop_before_the_tool_is_removed() {
    use pw_tools::install::{InstallRecord, InstalledVersion, Layout};

    let env = Env::new(EnvOptions::default());
    // rust-analyzer: 설정에는 인자만 — 실행 파일은 설치본에서 찾는다.
    let dir = env.base.join("fake");
    fs::create_dir_all(&dir).unwrap();
    let mut scenario = incremental();
    scenario.record = Some(dir.join("installed.jsonl"));
    let scenario = scenario.write(&dir, "installed").unwrap();
    let settings = env.paths.config.join("settings.toml");
    let text = fs::read_to_string(&settings).unwrap();
    fs::write(
        &settings,
        format!(
            "{text}[lsp.rust-analyzer]\nargs = [\"{}\"]\n",
            scenario.display()
        ),
    )
    .unwrap();
    env.global.reload().unwrap();
    let layout = Layout::new(env.base.join("data/tools"));
    let version_dir = layout.version_dir("rust-analyzer", "2026-10-26");
    fs::create_dir_all(version_dir.join("bin")).unwrap();
    fs::copy(
        common::env::fake_program(),
        version_dir.join("bin/rust-analyzer"),
    )
    .unwrap();
    let entry: InstalledVersion = serde_json::from_value(json!({
        "version": "2026-10-26", "kind": "github", "registryVersion": "test", "sourceUrls": [], "files": [],
        "checksum": "pinned", "licenses": ["MIT"], "size": 1, "installedAtMs": 0,
        "bins": { "rust-analyzer": "bin/rust-analyzer" },
    }))
    .unwrap();
    layout
        .write_record("rust-analyzer", &InstallRecord::new("rust-analyzer", entry))
        .unwrap();
    env.servers
        .set_tools(Arc::new(pw_tools::InstalledTools::new(layout)));
    // ruff: 설정 경로(설치본이 아니다).
    env.fake("ruff", incremental());

    let mut win = env.window();
    win.open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    win.open(&env, &env.file("app.py", ""), "Python", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    env.wait_state("ruff", LspInstanceState::Ready).await;
    let pids = env.manager.pids(&env.id);
    assert_eq!(pids.len(), 2);
    env.manager.stop_installed("rust-analyzer").await;
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::NotInstalled {
            mason: Some("rust-analyzer".into())
        })
    );
    assert_eq!(env.state_of("ruff"), Some(LspInstanceState::Ready));
    let left = env.manager.pids(&env.id);
    assert_eq!(left.len(), 1);
    assert!(
        pids.iter()
            .filter(|p| !left.contains(p))
            .all(|p| !alive(*p))
    );
    // 다른 패키지 이름은 아무것도 멈추지 않는다.
    env.manager.stop_installed("ruff").await;
    assert_eq!(env.state_of("ruff"), Some(LspInstanceState::Ready));
}

// ---- 리뷰 H1: 기동 중(`initialize` 를 기다리는) 서버 -------------------------------------------------------------------------

/// `initialize` 에 `ms` 늦게 답하고, 그동안 다른 것에는 답하지 않는 가짜 서버(기동에 묶인 서버 — `shutdown` 에 답하지 않고 `exit`
/// 로 끝나지 않는다. 그래서 `shutdown` 으로 끝내면 상한 2 s 를 다 쓴다).
fn slow_initialize(ms: u64) -> Scenario {
    Scenario {
        ignore_exit: true,
        ..incremental()
    }
    .reply(
        "initialize",
        Reply::result(json!({ "capabilities": { "textDocumentSync": 2 } })).delayed(ms),
    )
    .reply("shutdown", Reply::silent())
}

/// 기동 중인 서버 하나를 띄우고 그 pid 를 돌려준다(관리자는 기동 중에도 서버를 쥔다 — `pids` 에 보인다).
async fn starting_server(env: &Env, win: &mut common::env::Conn) -> u32 {
    win.open(env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    assert!(
        eventually(Duration::from_secs(5), || env.manager.pids(&env.id).len()
            == 1)
        .await,
        "{:?}",
        env.states()
    );
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::Starting)
    );
    env.manager.pids(&env.id)[0]
}

/// ①(리뷰 H1) `initialize` 를 기다리는 서버도 신뢰를 풀면 1 s 안에 끝난다 — 앞의 판은 답(여기선 6 s)을 기다린 뒤에야 끝냈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_trust_ends_a_server_that_is_still_initializing_within_1_s() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", slow_initialize(6000));
    let mut win = env.window();
    let pid = starting_server(&env, &mut win).await;
    let revoked = std::time::Instant::now();
    env.projects.set_trust(&env.id, false).unwrap();
    assert!(
        eventually(Duration::from_secs(1), || !alive(pid)).await,
        "the initializing server outlived the trust by 1 s"
    );
    eprintln!("ended {:?} after the trust was revoked", revoked.elapsed());
    env.wait_state("rust-analyzer", LspInstanceState::Untrusted)
        .await;
    assert!(env.manager.pids(&env.id).is_empty());
    assert_eq!(env.manager.spawn_count(), 1);
}

/// ⑥(리뷰 H1) `initialize` 를 기다리는 서버도 창을 닫으면 2 s 안에 끝난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_window_ends_a_server_that_is_still_initializing_within_2_s() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", slow_initialize(6000));
    let mut win = env.window();
    let pid = starting_server(&env, &mut win).await;
    let closed = std::time::Instant::now();
    win.session.close();
    assert!(
        eventually(Duration::from_secs(2), || !alive(pid)).await,
        "the initializing server outlived its window by 2 s"
    );
    eprintln!("ended {:?} after the window closed", closed.elapsed());
    assert!(env.states().is_empty(), "{:?}", env.states());
}

/// 리뷰 H1: `lsp.server.stop` · `restart` 도 기동 중인 서버에 곧바로 닿는다(기다리지 않고 트리째 끝낸다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_and_restart_reach_a_server_that_is_still_initializing() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", slow_initialize(6000));
    let mut win = env.window();
    let first = starting_server(&env, &mut win).await;
    let root = env.manager.status(&env.id).instances[0].root.clone();
    let control = json!({ "projectId": env.id, "server": "rust-analyzer", "root": root });
    let started = std::time::Instant::now();
    let restarted = win
        .call("lsp.server.restart", control.clone())
        .await
        .unwrap();
    assert_eq!(restarted["state"]["kind"], "starting");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(eventually(Duration::from_secs(1), || !alive(first)).await);
    assert!(
        eventually(Duration::from_secs(5), || env
            .manager
            .pids(&env.id)
            .first()
            .is_some_and(|p| *p != first))
        .await
    );
    let second = env.manager.pids(&env.id)[0];
    let started = std::time::Instant::now();
    let stopped = win.call("lsp.server.stop", control).await.unwrap();
    assert_eq!(stopped["state"]["kind"], "stopped");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(eventually(Duration::from_secs(1), || !alive(second)).await);
    assert!(env.manager.pids(&env.id).is_empty());
    // 그 뒤 `initialize` 답이 올 자리(6 s)가 지나도 되살아나지 않는다 — 여기선 짧게 본다.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::Stopped)
    );
    assert_eq!(env.manager.spawn_count(), 2);
}

// ---- 리뷰 M1: 문서 없이 제어를 부른 연결 ----------------------------------------------------------------------------------

/// 리뷰 M1: 문서를 연 적 없는 연결(같은 프로젝트의 다른 창 연결)이 `lsp.server.restart` 를 부른 뒤 문서를 열고 닫혀도 그 연결의
/// 닫힘을 지켜본다 — 두 연결이 다 닫히면 서버가 끝난다. 앞의 판은 그 연결을 지켜보지 않아 서버가 창보다 오래 살았다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_only_restarted_is_still_watched() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", incremental());
    let mut a = env.window();
    a.open(&env, &env.file("src/a.rs", ""), "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let root = env.manager.status(&env.id).instances[0].root.clone();
    let mut b = env.window();
    b.call(
        "lsp.server.restart",
        json!({ "projectId": env.id, "server": "rust-analyzer", "root": root }),
    )
    .await
    .unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    b.open(&env, &env.file("src/b.rs", ""), "Rust", "").await;
    let pid = env.manager.pids(&env.id)[0];
    a.session.close();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(alive(pid), "b still holds the server");
    b.session.close();
    assert!(
        eventually(Duration::from_secs(2), || !alive(pid)).await,
        "the server outlived both windows: {:?}",
        env.states()
    );
    assert!(env.states().is_empty(), "{:?}", env.states());
}

// ---- 리뷰 M2: 같은 파일을 연 두 연결 ---------------------------------------------------------------------------------------

/// 리뷰 M2: 두 연결(창)이 같은 파일을 열면 서버에는 `didOpen` 한 번이고, 마지막 연결이 닫을 때만 `didClose` 다. 서버의 텍스트는
/// 마지막에 편집 · 요청한 문서의 것이고(다르면 전체 텍스트로 맞춘다), 버전은 한 줄로 오른다(`replay` 가 본다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_windows_share_one_server_document() {
    let env = Env::new(EnvOptions::default());
    let record = env.fake("rust-analyzer", hover_scenario(0));
    let file = env.file("src/lib.rs", "");
    let mut a = env.window();
    let mut b = env.window();
    let da = doc_id(&a.open(&env, &file, "Rust", "one").await);
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let db = doc_id(&b.open(&env, &file, "Rust", "one").await);
    let edit = |doc: &str, version: u32, at: u32, insert: &str| json!({ "docId": doc, "version": version, "changes": [{ "from": at, "to": at, "insert": insert }] });
    a.call("lsp.document.change", edit(&da, 1, 0, "A"))
        .await
        .unwrap();
    b.call("lsp.document.change", edit(&db, 1, 3, "B"))
        .await
        .unwrap();
    // a 가 묻는다 — 서버의 텍스트가 a 의 것("Aone")으로 돌아온 뒤 묻는다.
    a.call(
        "test.lsp.feature",
        feature(&da, "hover", "textDocument/hover", 1, None),
    )
    .await
    .unwrap();
    a.call("lsp.document.close", json!({ "docId": da }))
        .await
        .unwrap();
    b.call("lsp.document.change", edit(&db, 2, 4, "!"))
        .await
        .unwrap();
    let wanted = [
        "textDocument/didOpen",
        "textDocument/didChange",
        "textDocument/didChange",
        "textDocument/didChange",
        "textDocument/hover",
        "textDocument/didChange",
        "textDocument/didChange",
    ];
    let kinds = || -> Vec<String> {
        received(&record)
            .into_iter()
            .filter_map(|m| m["method"].as_str().map(str::to_owned))
            .filter(|m| m.starts_with("textDocument/"))
            .collect()
    };
    assert!(
        eventually(Duration::from_secs(5), || kinds().len() >= wanted.len()).await,
        "{:?}",
        kinds()
    );
    assert_eq!(kinds(), wanted);
    let changes = methods_of(&record, "textDocument/didChange");
    let full = |i: usize| changes[i]["params"]["contentChanges"][0]["text"].clone();
    assert_eq!(full(1), "oneB", "b took over with its whole text");
    assert_eq!(full(2), "Aone", "a's request first put a's text back");
    assert_eq!(full(3), "oneB", "a closed; b's text is the server's again");
    let (text, version) = replay(&record);
    assert_eq!(text, "oneB!");
    assert_eq!(version, 5);
    b.session.close();
    assert!(
        eventually(Duration::from_secs(2), || kinds()
            .last()
            .map(String::as_str)
            == Some("textDocument/didClose"))
        .await,
        "{:?}",
        kinds()
    );
    assert_eq!(methods_of(&record, "textDocument/didOpen").len(), 1);
    assert_eq!(methods_of(&record, "textDocument/didClose").len(), 1);
}

// ---- 리뷰 L2: 상태 읽기의 범위 ------------------------------------------------------------------------------------------------

/// 리뷰 L2: `lsp.status.get` 은 그 프로젝트 창의 연결만 읽는다(상태에 작업 공간 루트가 든다). 미신뢰여도 읽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_projects_window_reads_its_status() {
    let env = Env::new(EnvOptions {
        trusted: false,
        ..EnvOptions::default()
    });
    env.fake("rust-analyzer", incremental());
    let mut win = env.window();
    win.open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    let own = win
        .call("lsp.status.get", json!({ "projectId": env.id }))
        .await
        .unwrap();
    assert_eq!(own["instances"][0]["state"]["kind"], "untrusted");
    for scope in [
        ConnectionScope::Project(env.other.clone()),
        ConnectionScope::Host,
    ] {
        let e = env
            .connect(scope.clone())
            .call("lsp.status.get", json!({ "projectId": env.id }))
            .await
            .unwrap_err();
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
    }
}

// ---- 리뷰 M4: 띄운 스레드가 끝나도 ------------------------------------------------------------------------------------------

/// ⓑ(리뷰 M4) 서버를 띄우라고 한 스레드가 곧 끝나도 서버는 산다 — fork 는 프로세스 수명 동안 사는 전용 스레드가 한다. Linux 의
/// `PR_SET_PDEATHSIG` 는 fork 한 스레드가 끝날 때 신호를 보내므로, 부른 스레드에서 fork 하면(여기선 쉬다 0.1 s 뒤 끝나는 블로킹 풀
/// 스레드 — `block_in_place` 를 한 일꾼 스레드도 같은 풀로 넘어가 끝난다) 이 시험이 Linux 에서 실패한다(macOS 에서는 늘 통과).
#[test]
fn a_server_outlives_the_thread_that_asked_for_it() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_keep_alive(Duration::from_millis(100))
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let handle = tokio::runtime::Handle::current();
        let (project, fake) = tokio::task::spawn_blocking(move || {
            let project = common::project();
            let fake = handle.block_on(project.fake("short-lived", Scenario::default(), false));
            (project, fake)
        })
        .await
        .unwrap();
        let pid = fake.server.pid().unwrap();
        // 그 블로킹 스레드가 끝날 때까지.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            alive(pid),
            "the server died with the thread that asked for it"
        );
        fake.server
            .initialize(pw_lsp::InitializeOptions::new(&project.root))
            .await
            .unwrap();
        fake.server.shutdown().await;
    });
}
