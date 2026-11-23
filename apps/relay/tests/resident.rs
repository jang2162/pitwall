//! 상주(18-m7-plan M7-24 ⑤ · 시험 ③) — 실제 바이너리. 단위 파일은 사람이 `resident on` 을 고를 때만 생긴다: 데몬이 떠서 끝나도
//! 생기지 않고, `off` 는 서비스 관리자에서 내리고 파일을 지운다. 서비스 관리자(`launchctl` · `systemctl`)는 인자를 적는 가짜다 — 이
//! 기기의 launchd 를 건드리지 않는다.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_pitwall-relay");

fn fake_managers(dir: &Path) -> PathBuf {
    let bin = dir.join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["launchctl", "systemctl"] {
        let path = bin.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"{name} $*\" >> \"{}\"\n",
                dir.join("calls.log").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }
    bin
}

fn relay(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", fake_managers(home))
        .env("PITWALL_DATA_DIR", home.join("data"))
        .env("PITWALL_STATE_DIR", home.join("state"))
        .env("PITWALL_CONFIG_DIR", home.join("config"))
        .env("PITWALL_CACHE_DIR", home.join("cache"));
    command
}

fn unit_files(home: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in ["Library/LaunchAgents", ".config/systemd/user"] {
        if let Ok(entries) = std::fs::read_dir(home.join(dir)) {
            found.extend(entries.flatten().map(|e| e.path()));
        }
    }
    found
}

fn calls(home: &Path) -> String {
    std::fs::read_to_string(home.join("calls.log")).unwrap_or_default()
}

fn status(home: &Path) -> serde_json::Value {
    let out = relay(home, &["resident", "status"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

/// 데몬이 떠서 유휴로 끝나도 단위 파일이 생기지 않는다(기본은 상주가 아니다). 서비스 관리자도 부르지 않는다.
#[test]
fn a_daemon_run_leaves_no_unit_behind() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let config = home.join("config/relay");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("relay.toml"),
        "schema = 1\n[idle]\nexit_after = 1\n[listen]\ninterfaces = []\n",
    )
    .unwrap();
    let mut child = relay(home, &["daemon"]).spawn().unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status:?}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the idle daemon did not exit"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(unit_files(home).is_empty(), "{:?}", unit_files(home));
    assert_eq!(calls(home), "");
    assert_eq!(status(home)["resident"], false);
}

/// `resident on` 이 단위를 쓰고 올리며, `off` 가 내리고 지운다. 그 뒤 상태는 꺼짐이다.
#[test]
fn resident_on_and_off_write_and_remove_the_unit() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let on = relay(home, &["resident", "on"]).output().unwrap();
    assert!(on.status.success(), "{on:?}");
    let answer: serde_json::Value =
        serde_json::from_slice(on.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(answer["resident"], true);
    let files = unit_files(home);
    assert_eq!(files.len(), 1, "{files:?}");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("--resident"), "{text}");
    let up = calls(home);
    assert!(
        up.contains("launchctl bootstrap") || up.contains("systemctl --user enable --now"),
        "{up}"
    );
    assert_eq!(status(home)["resident"], true);

    let off = relay(home, &["resident", "off"]).output().unwrap();
    assert!(off.status.success(), "{off:?}");
    assert!(unit_files(home).is_empty());
    let down = calls(home);
    assert!(
        down.contains("launchctl bootout") || down.contains("systemctl --user disable --now"),
        "{down}"
    );
    assert_eq!(status(home)["resident"], false);
}
