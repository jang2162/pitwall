//! 바이너리 시험: `pitwall-relay proxy` 가 데몬을 띄우고 붙는다(18-m7-plan M7-04 ②). 실제 SSH 는 쓰지 않는다 — 임시
//! `PITWALL_*_DIR` 로 이 기기에서 띄운다.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_pitwall-relay");

fn relay(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
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

fn frame(value: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).unwrap();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend(body);
    out
}

/// `Content-Length` 틀 하나를 읽는다.
fn read_frame(read: &mut impl Read) -> serde_json::Value {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        assert_eq!(
            read.read(&mut byte).unwrap(),
            1,
            "the proxy closed before a frame"
        );
        header.push(byte[0]);
    }
    let header = String::from_utf8(header).unwrap();
    let length: usize = header
        .trim()
        .strip_prefix("Content-Length: ")
        .expect("Content-Length")
        .parse()
        .unwrap();
    let mut body = vec![0u8; length];
    read.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// proxy 하나를 띄워 `status` 를 묻고 답을 받는다. proxy 는 stdin 을 닫으면 끝난다.
fn status_through_proxy(root: &Path) -> (serde_json::Value, Duration) {
    let started = Instant::now();
    let mut proxy = relay(root, &["proxy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = proxy.stdin.take().unwrap();
    stdin
        .write_all(&frame(&serde_json::json!({"kind": "status"})))
        .unwrap();
    stdin.flush().unwrap();
    let mut stdout = proxy.stdout.take().unwrap();
    let answer = read_frame(&mut stdout);
    let took = started.elapsed();
    drop(stdin);
    finish(proxy);
    assert_eq!(answer["kind"], serde_json::json!("result"), "{answer}");
    (answer["body"].clone(), took)
}

fn finish(mut child: Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "the proxy did not end after stdin closed"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: 신호 0 은 존재만 묻는다.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// 시험이 띄운 데몬을 끝낸다(SIGTERM) — 끝날 때까지 기다린다.
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

/// 같은 판 proxy 둘이(동시에 떠도) 데몬 하나에 붙는다 — 둘째는 첫째가 띄운 데몬에 붙거나, 둘이 함께 띄운 데몬 중 소켓 자리를 쥔
/// 하나에 붙는다(18-m7-plan M7-04 시험 ⑤). 데몬 기동 → 첫 응답 시간을 잰다(가벼움 — M7-P13 의 출발값 < 300 ms).
#[test]
fn two_proxies_of_one_version_share_one_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let other = root.clone();
    let (first, second) = std::thread::scope(|s| {
        let a = s.spawn(|| status_through_proxy(&root));
        let b = s.spawn(|| status_through_proxy(&other));
        (a.join().unwrap(), b.join().unwrap())
    });
    let pid = first.0["pid"].as_i64().unwrap() as i32;
    let _daemon = Daemon(pid);
    assert_eq!(
        first.0["pid"], second.0["pid"],
        "two daemons for one version"
    );
    eprintln!(
        "proxy spawn -> first answer: {:?} / {:?} (two proxies at once)",
        first.1, second.1
    );

    // 세 번째 proxy 는 떠 있는 데몬에 곧바로 붙는다.
    let (third, took) = status_through_proxy(&root);
    assert_eq!(third["pid"], first.0["pid"]);
    eprintln!("proxy -> running daemon answer: {took:?}");

    // 판 하나에 소켓 하나 — `$STATE/relay/<판>.sock`(0600) 과 폴더(0700).
    use std::os::unix::fs::PermissionsExt;
    let state = root.join("state/relay");
    let socket = state.join(format!("{}.sock", env!("CARGO_PKG_VERSION")));
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // 로그는 `$STATE/relay/logs/relay.<날짜>.log`.
    let logs: Vec<_> = std::fs::read_dir(state.join("logs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        logs.iter()
            .any(|name| name.starts_with("relay.") && name.ends_with(".log")),
        "{logs:?}"
    );
}

/// 띄운 데몬이 받지 못하면(여기서는 `relay.toml` 이 틀려 뜨지 않는다) proxy 는 2초 안팎에 진단 글과 함께 끝난다.
#[test]
fn a_daemon_that_does_not_come_up_ends_the_proxy_with_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config/relay")).unwrap();
    std::fs::write(
        dir.path().join("config/relay/relay.toml"),
        "root = [\"/x\"]\n",
    )
    .unwrap();
    let started = Instant::now();
    let output = relay(dir.path(), &["proxy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let took = started.elapsed();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "the proxy wrote to stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("did not accept"), "{stderr}");
    assert!(stderr.contains("logs"), "{stderr}");
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(6),
        "{took:?}"
    );
}

/// `status` 는 떠 있는 데몬이 없으면 띄우지 않고 3 으로 끝난다.
#[test]
fn status_without_a_daemon_does_not_start_one() {
    let dir = tempfile::tempdir().unwrap();
    let output = relay(dir.path(), &["status"]).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(
        !dir.path()
            .join("state/relay")
            .join(format!("{}.sock", env!("CARGO_PKG_VERSION")))
            .exists()
    );
}
