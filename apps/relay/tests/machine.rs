//! `pitwall-relay machine`(18-m7-plan M7-07b 리뷰 R-1) — 실제 바이너리. 이 머신의 id 는 페어링이 내는 id(Relay 기기 키 지문)와 같고,
//! 키가 없으면 만들지 않고 3 으로 끝난다(Relay 를 쓴 적 없는 머신에 키를 남기지 않는다).

#![cfg(unix)]

use std::process::Command;

use pw_link::DeviceKey;

const BIN: &str = env!("CARGO_BIN_EXE_pitwall-relay");

fn relay(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("PITWALL_DATA_DIR", home.join("data"))
        .env("PITWALL_STATE_DIR", home.join("state"))
        .env("PITWALL_CONFIG_DIR", home.join("config"))
        .env("PITWALL_CACHE_DIR", home.join("cache"))
        .output()
        .unwrap()
}

#[test]
fn the_machine_id_is_the_pairing_id_and_no_key_is_made_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();

    let none = relay(home, &["machine"]);
    assert_eq!(none.status.code(), Some(3), "{none:?}");
    assert!(none.stdout.is_empty());
    assert!(
        !home.join("data/relay/device.key").exists(),
        "asking for the machine id made a key"
    );

    let key = DeviceKey::from_seed(&[9; 32]).public().to_string();
    let paired = relay(home, &["pair", "--from-ssh", &key, "mac"]);
    assert!(paired.status.success(), "{paired:?}");
    let paired: serde_json::Value = serde_json::from_slice(&paired.stdout).unwrap();

    let machine = relay(home, &["machine"]);
    assert!(machine.status.success(), "{machine:?}");
    assert_eq!(
        String::from_utf8(machine.stdout).unwrap().trim(),
        paired["machine"].as_str().unwrap()
    );
}
