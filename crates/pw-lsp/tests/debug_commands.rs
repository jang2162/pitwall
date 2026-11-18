//! 디버그 세션이 언어 서버가 여는 어댑터(java-debug)를 시작할 때 쓰는 명령 길 — `LspManager` 의 `pw_lsp_servers::LspCommands`
//! 구현(17-m6-plan.md M6-19). (프로젝트, 작업 공간 루트, 서버)가 정확히 맞고 떠 있는 인스턴스에만 보낸다 — 같은 서버가 다른
//! 프로젝트에 떠 있어도 그쪽으로 가지 않는다(엉뚱한 jdtls 가 연 포트에 붙지 않게). 서버는 가짜(rust-analyzer 자리)다 — 구현은 서버 id
//! 와 상관없다.

#![cfg(unix)]

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use pw_lsp::fake::Reply;
use pw_lsp_servers::LspCommands;
use pw_rpc::ConnectionScope;
use serde_json::json;

use common::env::{Env, EnvOptions, fake_program};

const SERVER: &str = "rust-analyzer";
const COMMAND: &str = "vscode.java.startDebugSession";

/// 그 프로젝트의 개인 설정으로 가짜 서버를 꽂는다 — `workspace/executeCommand` 에 `answer` 로 답한다.
fn fake_for(env: &Env, root: &Path, name: &str, answer: u16) {
    let scenario = pw_lsp::fake::Scenario {
        capabilities: json!({ "textDocumentSync": 2, "executeCommandProvider": { "commands": [COMMAND] } }),
        ..pw_lsp::fake::Scenario::default()
    }
    .reply("workspace/executeCommand", Reply::result(json!(answer)));
    let dir = env.base.join("fake");
    fs::create_dir_all(&dir).unwrap();
    let path = scenario.write(&dir, name).unwrap();
    fs::create_dir_all(root.join(".pitwall")).unwrap();
    fs::write(
        root.join(".pitwall/project.local.toml"),
        format!(
            "schema = 1\n[lsp.{SERVER}]\npath = \"{}\"\nargs = [\"{}\"]\n",
            fake_program(),
            path.display()
        ),
    )
    .unwrap();
}

async fn ask(env: &Env, project: &str, root: &Path, server: &str) -> Option<Result<u64, String>> {
    let commands: &dyn LspCommands = &env.manager;
    commands
        .execute_command(project, root, server, COMMAND, Vec::new())
        .await
        .map(|r| r.map(|v| v.as_u64().unwrap()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_debug_command_reaches_only_the_server_of_that_project_and_root() {
    let env = Env::new(EnvOptions::default());
    env.projects.set_trust(&env.other, true).unwrap();
    let other_root = env.base.join("p2");
    fake_for(&env, &env.root, "p1", 1111);
    fake_for(&env, &other_root, "p2", 2222);

    // 아무것도 떠 있지 않다 — 띄우지 않고 None.
    assert_eq!(ask(&env, &env.id, &env.root, SERVER).await, None);
    assert_eq!(env.manager.spawn_count(), 0);

    // 두 프로젝트에 같은 서버를 띄운다.
    let mut one = env.window();
    one.open(&env, &env.file("src/main.rs", ""), "Rust", "")
        .await;
    let mut two = env.connect(ConnectionScope::Project(env.other.clone()));
    let main2 = other_root.join("src/main.rs");
    fs::create_dir_all(main2.parent().unwrap()).unwrap();
    fs::write(&main2, "").unwrap();
    two.call(
        "lsp.document.open",
        json!({ "projectId": env.other, "path": main2, "language": "Rust", "version": 0, "text": "" }),
    )
    .await
    .unwrap();
    for (project, root) in [(&env.id, &env.root), (&env.other, &other_root)] {
        let deadline = Instant::now() + Duration::from_secs(10);
        while ask(&env, project, root, SERVER).await.is_none() {
            assert!(
                Instant::now() < deadline,
                "{project} never answered: {:?}",
                env.states()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    // 그 프로젝트 · 루트의 서버만 답한다.
    assert_eq!(ask(&env, &env.id, &env.root, SERVER).await, Some(Ok(1111)));
    assert_eq!(
        ask(&env, &env.other, &other_root, SERVER).await,
        Some(Ok(2222))
    );
    // 프로젝트와 루트가 어긋나면(다른 프로젝트의 루트 · 같은 프로젝트의 다른 체크아웃) 어느 쪽으로도 가지 않는다.
    assert_eq!(ask(&env, &env.id, &other_root, SERVER).await, None);
    assert_eq!(ask(&env, &env.other, &env.root, SERVER).await, None);
    assert_eq!(
        ask(&env, &env.id, &env.root.join("src"), SERVER).await,
        None
    );
    // 떠 있지 않은 서버 · 등록되지 않은 프로젝트.
    assert_eq!(ask(&env, &env.id, &env.root, "basedpyright").await, None);
    assert_eq!(ask(&env, "no-such-project", &env.root, SERVER).await, None);
}
