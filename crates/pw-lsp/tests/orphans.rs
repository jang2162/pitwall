//! Host 가 강제로 끝난 뒤 남은 서버 그룹 거두기(M4-01 리뷰 L1 — `pw_lsp::ledger`).
//!
//! 「Host」 는 이 시험 바이너리를 도우미 모드(`orphan_helper_host` — 환경 변수가 있을 때만 일한다)로 다시 띄운 프로세스다. 그것이
//! 기록부를 켠 채 가짜 서버(같은 그룹에 `sleep` 자식을 둔다)를 띄우면 `kill -9` 로 끝내고, 남은 것을 [`Ledger::reap_orphans`] 가
//! 끄는지 본다.

#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use pw_lsp::fake::{Action, Scenario};
use pw_lsp::ledger::Ledger;
use pw_lsp::{InitializeOptions, LanguageServer, ServerOptions};
use serde_json::Value;

const HELPER_DIR: &str = "PW_LSP_ORPHAN_HELPER_DIR";
const HELPER_IGNORE_EXIT: &str = "PW_LSP_ORPHAN_HELPER_IGNORE_EXIT";

/// 도우미: 기록부를 켜고 가짜 서버를 띄운 뒤 `READY <서버 pid>` 를 찍고 끝없이 기다린다(밖에서 `kill -9` 한다). 환경 변수가 없으면
/// 아무것도 하지 않는다 — `--run-ignored all` 로 돌아도 곧바로 끝난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "helper process for the orphan tests; does nothing on its own"]
async fn orphan_helper_host() {
    let Some(dir) = std::env::var_os(HELPER_DIR) else {
        return;
    };
    let dir = Path::new(&dir);
    let p = common::project_at(&dir.join("host"));
    let scenario = Scenario {
        start: vec![Action::SpawnChild { seconds: 300 }],
        record: Some(dir.join("record.jsonl")),
        ignore_exit: std::env::var_os(HELPER_IGNORE_EXIT).is_some(),
        ..Scenario::default()
    };
    let path = scenario.write(dir, "scenario").unwrap();
    let server = LanguageServer::spawn(
        &p.grant,
        common::fake_spec(&p.root, &path),
        ServerOptions {
            instance: "orphan".into(),
            ledger: Some(Ledger::new(dir.join("ledger"))),
            ..ServerOptions::default()
        },
    )
    .await
    .unwrap();
    server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    println!("READY {}", server.pid().unwrap());
    std::future::pending::<()>().await;
}

/// 도우미를 띄워 서버 pid 와 자식 pid 를 받고 `kill -9` 한다.
fn run_and_kill_host(dir: &Path, ignore_exit: bool) -> (u32, u32) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["orphan_helper_host", "--exact", "--ignored", "--nocapture"])
        .env(HELPER_DIR, dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if ignore_exit {
        command.env(HELPER_IGNORE_EXIT, "1");
    }
    let mut host = command.spawn().unwrap();
    let stdout = BufReader::new(host.stdout.take().unwrap());
    let server = stdout
        .lines()
        .map_while(Result::ok)
        .find_map(|l| {
            l.strip_prefix("READY ")
                .and_then(|p| p.trim().parse::<u32>().ok())
        })
        .expect("the helper host started a server");
    let child = common::received(&dir.join("record.jsonl"))
        .iter()
        .find_map(|m| m.get("childPid").and_then(Value::as_u64))
        .and_then(|p| u32::try_from(p).ok())
        .expect("the fake server spawned its child");
    host.kill().unwrap();
    host.wait().unwrap();
    (server, child)
}

fn records(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("ledger")).map_or(0, |d| d.flatten().count())
}

async fn gone(pids: &[u32]) -> bool {
    common::eventually(Duration::from_secs(3), || {
        pids.iter().all(|&p| !common::alive(p))
    })
    .await
}

/// 서버는 stdin 의 끝에서 스스로 끝나지만 같은 그룹의 자식은 고아로 남는다 — 다음 시작의 거두기가 그 그룹을 끈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grandchildren_left_by_a_killed_host_are_reaped_on_the_next_start() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (server, child) = run_and_kill_host(dir, false);
    assert_eq!(records(dir), 1);
    assert!(gone(&[server]).await, "the server should end on stdin EOF");
    assert!(
        common::alive(child),
        "the child is left behind (what the ledger is for)"
    );
    // 리더가 없는 그룹: 남은 손자를 끈다.
    assert_eq!(Ledger::new(dir.join("ledger")).reap_orphans(), 1);
    assert!(gone(&[child]).await, "the orphaned child is still alive");
    assert_eq!(records(dir), 0);
    // 다시 돌려도 끌 것이 없다.
    assert_eq!(Ledger::new(dir.join("ledger")).reap_orphans(), 0);
}

/// 서버가 stdin 의 끝을 무시하면(macOS — Linux 는 `PR_SET_PDEATHSIG` 가 SIGTERM 을 보낸다) 서버와 자식이 함께 남는다 — 그 그룹을
/// 통째로 끈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_outlives_a_killed_host_is_reaped_with_its_group() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (server, child) = run_and_kill_host(dir, true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    if cfg!(target_os = "linux") {
        assert!(
            gone(&[server]).await,
            "PR_SET_PDEATHSIG should have ended the server"
        );
    } else {
        assert!(common::alive(server), "the server ignores stdin EOF");
    }
    assert!(common::alive(child));
    assert_eq!(Ledger::new(dir.join("ledger")).reap_orphans(), 1);
    assert!(gone(&[server, child]).await);
    assert_eq!(records(dir), 0);
}

/// 정상으로 끝낸 서버는 기록을 남기지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_server_leaves_no_record() {
    let p = common::project();
    let ledger = Ledger::new(p.base.join("ledger"));
    let path = Scenario::default().write(&p.base, "plain").unwrap();
    let server = LanguageServer::spawn(
        &p.grant,
        common::fake_spec(&p.root, &path),
        ServerOptions {
            ledger: Some(std::sync::Arc::clone(&ledger)),
            ..ServerOptions::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_dir(ledger.dir()).unwrap().count(), 1);
    server.shutdown().await;
    assert!(
        common::eventually(Duration::from_secs(2), || {
            std::fs::read_dir(ledger.dir()).unwrap().count() == 0
        })
        .await
    );
}
