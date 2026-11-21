//! 18-m7-plan M7-01 완료 기준 ②: pw-host 의 의존 그래프(`cargo tree -p pw-host` 와 같은 범위 — 일반 · 빌드 의존, 그리고 pw-host
//! 자신의 dev 의존)에 tauri 가 없다. Relay 는 창 없이 같은 Host 를 띄운다 — tauri 를 들이는 의존이 생기면 실패한다.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value;

#[test]
fn pw_host_does_not_depend_on_tauri() {
    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--manifest-path",
        ])
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
