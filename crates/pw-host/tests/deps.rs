//! 18-m7-plan M7-01 완료 기준 ②: pw-host 의 의존 그래프(`cargo tree -p pw-host` 와 같은 범위 — 일반 · 빌드 의존, 그리고 pw-host
//! 자신의 dev 의존)에 tauri 가 없다. Relay 는 창 없이 같은 Host 를 띄운다 — tauri 를 들이는 의존이 생기면 실패한다.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value;

/// 이 기기의 타깃(`cargo -vV` 의 `host:`).
fn host_triple() -> String {
    let output = std::process::Command::new(env!("CARGO"))
        .arg("-vV")
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    text.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .unwrap_or_else(|| panic!("no host line in {text}"))
        .to_owned()
}

#[test]
fn pw_host_does_not_depend_on_tauri() {
    // `--filter-platform` 이 없으면 `cargo metadata` 는 모든 타깃의 패키지를 읽어야 해, 이 타깃만 받은 CI 캐시에서 `--offline` 이
    // 실패한다(macOS arm64 에 없는 `curve25519-dalek-derive`). `cargo tree` 처럼 이 기기의 타깃만 본다 — CI 의 세 OS 가 저마다 본다.
    let output = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--offline"])
        .args(["--filter-platform", &host_triple(), "--manifest-path"])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: HashMap<&str, &str> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p["name"].as_str().unwrap()))
        .collect();
    let nodes: HashMap<&str, &Value> = metadata["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["id"].as_str().unwrap(), n))
        .collect();
    let root = names
        .iter()
        .find_map(|(id, name)| (*name == "pw-host").then_some(*id))
        .unwrap();
    let mut seen = BTreeSet::new();
    let mut stack = vec![(root, true)];
    while let Some((id, is_root)) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for dep in nodes[id]["deps"].as_array().unwrap() {
            // 일반 의존은 kind 가 null, 빌드 의존은 "build". dev 의존은 pw-host 자신의 것만 본다(cargo tree 와 같다).
            let follows = dep["dep_kinds"].as_array().unwrap().iter().any(|k| {
                let kind = k["kind"].as_str();
                kind.is_none() || kind == Some("build") || (is_root && kind == Some("dev"))
            });
            if follows {
                stack.push((dep["pkg"].as_str().unwrap(), false));
            }
        }
    }
    let reached: BTreeSet<&str> = seen.iter().map(|id| names[id]).collect();
    // 그래프를 실제로 따라갔는지: 조립이 잇는 기능 크레이트가 보여야 한다.
    for expected in ["pw-db", "pw-git", "pw-mcp", "tokio"] {
        assert!(reached.contains(expected), "{expected} not in {reached:?}");
    }
    let tauri: Vec<_> = reached
        .iter()
        .filter(|name| **name == "tauri" || name.starts_with("tauri-"))
        .collect();
    assert!(tauri.is_empty(), "pw-host reaches {tauri:?}");
}

/// `cargo tree -p <패키지> -i pw-link` 의 첫 줄(pw-link 와 그 패키지에서 켜진 기능).
fn pw_link_features_of(package: &str) -> String {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["tree", "--offline", "-e", "normal,build", "-i", "pw-link"])
        .args(["-f", "{p} [{f}]", "-p", package, "--manifest-path"])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let first = text.lines().next().unwrap_or_default().to_owned();
    assert!(first.starts_with("pw-link "), "{text}");
    first
}

/// 18-m7-plan M7-08(메인 판정): pw-host 가 원격 호스트 관리(`pw-remote`)를 링크해도 **Relay 바이너리에는 SSH 부트스트랩
/// (`pw-link/bootstrap` — 받기 · 매니페스트 검증 · 설치)이 들지 않는다**(M7-06 의 결정). 데스크톱 앱만 `pw-remote/ssh` 로 켠다 —
/// 그쪽에 기능이 보이는 것으로 이 시험이 기능을 실제로 읽는지도 본다.
#[test]
fn the_relay_does_not_link_the_ssh_bootstrap() {
    let relay = pw_link_features_of("pitwall-relay");
    assert!(!relay.contains("bootstrap"), "{relay}");
    let host = pw_link_features_of("pw-host");
    assert!(!host.contains("bootstrap"), "{host}");
    let desktop = pw_link_features_of("pitwall-desktop");
    assert!(desktop.contains("bootstrap"), "{desktop}");
}
