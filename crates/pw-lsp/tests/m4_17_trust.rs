//! 신뢰를 얻으면 미신뢰로 연 문서의 서버를 띄운다(15-m4-plan.md M4-17 — Host 규칙, 웹뷰는 문서를 다시 열지 않는다). 신뢰를 다시
//! 보는 그때의 신뢰로만 띄우고, 문서가 없거나 연 창이 닫힌 인스턴스는 띄우지 않는다.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::time::Duration;

use pw_lsp::fake::Scenario;
use pw_lsp::methods::LspInstanceState;
use serde_json::json;

use common::env::{Env, EnvOptions};
use common::{eventually, received};

fn incremental() -> Scenario {
    Scenario {
        capabilities: json!({ "textDocumentSync": 2, "hoverProvider": true }),
        ..Scenario::default()
    }
}

fn did_open(record: &Path) -> usize {
    received(record)
        .into_iter()
        .filter(|m| m["method"] == "textDocument/didOpen")
        .count()
}

fn untrusted() -> Env {
    Env::new(EnvOptions {
        trusted: false,
        ..EnvOptions::default()
    })
}

// ---- 신뢰를 얻으면 띄운다 -------------------------------------------------------------------------------------------------------

/// 미신뢰로 연 문서 → 신뢰 → 웹뷰가 다시 열지 않아도 서버가 한 번 뜨고, 그 문서를 받고, 문서의 능력 칸이 찬다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusting_the_project_starts_the_servers_of_its_open_documents() {
    let env = untrusted();
    let record = env.fake("rust-analyzer", incremental());
    let file = env.file("src/main.rs", "fn main() {}\n");
    let mut win = env.window();
    let opened = win.open(&env, &file, "Rust", "fn main() {}\n").await;
    assert_eq!(opened["instances"][0]["state"]["kind"], "untrusted");
    let doc = opened["docId"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(env.manager.spawn_count(), 0);

    env.projects.set_trust(&env.id, true).unwrap();
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert_eq!(env.manager.spawn_count(), 1);
    assert!(eventually(Duration::from_secs(2), || did_open(&record) == 1).await);
    let caps = win
        .call("lsp.document.capabilities", json!({ "docId": doc }))
        .await
        .unwrap();
    assert_eq!(
        caps["features"]["hover"]["server"], "rust-analyzer",
        "{caps}"
    );
}

/// 신뢰 → 곧바로 해제: 신뢰를 다시 보는 그때의 신뢰로만 띄운다 — 띄우지 않는다. 관리자의 신뢰 따르기를 끄고 손으로 불러 순서를
/// 정한다(따르기가 두 이벤트 사이에 끼어드는 경합을 없앤다). 같은 자리에서 신뢰를 주고 부르면 뜬다(대조).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trust_revoked_before_the_check_starts_nothing() {
    let env = Env::new(EnvOptions {
        trusted: false,
        follow: false,
        ..EnvOptions::default()
    });
    env.fake("rust-analyzer", incremental());
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    env.projects.set_trust(&env.id, true).unwrap();
    env.projects.set_trust(&env.id, false).unwrap();
    env.manager.start_trusted().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(env.manager.spawn_count(), 0);
    assert_eq!(
        env.state_of("rust-analyzer"),
        Some(LspInstanceState::Untrusted)
    );

    env.projects.set_trust(&env.id, true).unwrap();
    env.manager.start_trusted().await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    assert_eq!(env.manager.spawn_count(), 1);
}

/// 문서를 모두 닫은 인스턴스 · 문서를 연 창이 닫힌 인스턴스는 신뢰를 얻어도 띄우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn instances_without_open_documents_or_windows_are_not_started() {
    let env = Env::new(EnvOptions {
        trusted: false,
        follow: false,
        ..EnvOptions::default()
    });
    env.fake("rust-analyzer", incremental());
    env.fake("typescript-language-server", incremental());
    let mut win = env.window();
    let opened = win
        .open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    win.call("lsp.document.close", json!({ "docId": opened["docId"] }))
        .await
        .unwrap();
    {
        let mut gone = env.window();
        gone.open(&env, &env.file("web/a.ts", ""), "TypeScript", "")
            .await;
        // 연결이 닫힌다(창 닫힘).
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    env.projects.set_trust(&env.id, true).unwrap();
    env.manager.start_trusted().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(env.manager.spawn_count(), 0, "{:?}", env.states());
}
