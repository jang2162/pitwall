//! 바이너리 시험: `pitwall-relay mcp`(18-m7-plan M7-20 ①). 원격 머신의 에이전트가 이 명령으로 그 머신의 Relay 에 붙는다 — 데몬이
//! 없으면 띄우고(헤드리스는 없다) MCP 소켓에 붙어 stdio 를 잇는다. 사람이 고른 `pitwall` 링크(M7-P12)로 불러도 같다. 실제 SSH 는
//! 쓰지 않는다 — 임시 `PITWALL_*_DIR` 로 이 기기에서 띄운다.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_pitwall-relay");

fn relay(program: &Path, root: &Path, args: &[&str]) -> Command {
    let config = root.join("config/relay/relay.toml");
    if !config.exists() {
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "schema = 1\n[idle]\nexit_after = \"10s\"\n").unwrap();
    }
    let mut command = Command::new(program);
    command.args(args);
    for (var, leaf) in [
        ("PITWALL_CONFIG_DIR", "config"),
        ("PITWALL_DATA_DIR", "data"),
        ("PITWALL_STATE_DIR", "state"),
        ("PITWALL_CACHE_DIR", "cache"),
    ] {
        command.env(var, root.join(leaf));
    }
    command
}

fn alive(pid: i32) -> bool {
    // SAFETY: 신호 0 은 존재만 묻는다.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// 시험이 띄운 데몬을 끝낸다(SIGTERM).
struct Daemon(i32);

impl Drop for Daemon {
    fn drop(&mut self) {
        // SAFETY: 이 시험이 띄운 데몬의 pid 다.
        unsafe { libc::kill(self.0, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(self.0) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn daemon_of(root: &Path) -> Daemon {
    let out = relay(Path::new(BIN), root, &["status"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    Daemon(i32::try_from(status["pid"].as_i64().unwrap()).unwrap())
}

/// MCP 세션 하나: initialize → tools/list. 돌려주는 것은 툴 목록.
fn session(program: &Path, root: &Path, args: &[&str]) -> Vec<Value> {
    let mut child = relay(program, root, args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut send = |frame: Value| {
        let mut line = serde_json::to_vec(&frame).unwrap();
        line.push(b'\n');
        stdin.write_all(&line).unwrap();
        stdin.flush().unwrap();
    };
    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "remote-agent", "version": "1"}}}),
    );
    let init: Value =
        serde_json::from_str(&lines.next().expect("no initialize answer").unwrap()).unwrap();
    assert!(init["result"]["serverInfo"].is_object(), "{init}");
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}));
    let listed = loop {
        let frame: Value =
            serde_json::from_str(&lines.next().expect("no tools/list answer").unwrap()).unwrap();
        if frame["id"] == json!(2) {
            break frame;
        }
    };
    drop(send);
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status:?}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "`mcp` did not end after stdin closed"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    listed["result"]["tools"].as_array().unwrap().clone()
}

/// 데몬이 없으면 `mcp` 가 띄우고 붙는다. 세션은 앱 세션이다(헤드리스 안내가 없고 코어 밖 툴이 있다) · `checkout` 을 내지 않는다.
/// `pitwall` 이름(사람이 고른 링크)으로 부른 `pitwall mcp` 도 같은 데몬에 붙는다. Desktop 의 `$STATE/mcp` 에 쓰지 않는다.
#[test]
fn mcp_starts_the_daemon_and_serves_the_relay_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let tools = session(Path::new(BIN), &root, &["mcp"]);
    let _daemon = daemon_of(&root);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"run_config_upsert"), "{names:?}");
    assert!(names.contains(&"project_list"), "{names:?}");
    for tool in &tools {
        assert!(
            tool["inputSchema"]["properties"].get("checkout").is_none(),
            "{}",
            tool["name"]
        );
    }

    let link = root.join("bin/pitwall");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(BIN, &link).unwrap();
    let again = session(&link, &root, &["mcp"]);
    assert_eq!(again.len(), tools.len());
    assert!(
        !root.join("state/mcp").exists(),
        "the relay wrote under the desktop's state"
    );
}

/// `mcp` 의 모르는 인자는 쓰는 법과 2.
#[test]
fn mcp_refuses_unknown_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let out = relay(Path::new(BIN), dir.path(), &["mcp", "--headless"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: pitwall-relay mcp"));
    assert!(out.stdout.is_empty());
}
