//! Relay 서명 매니페스트(src/relay.rs, 18-m7-plan M7-05). 시험 입력은 `tests/fixtures/relay/`:
//!
//! - `updater.pub` — **시험 전용** minisign 공개키(Tauri 서명 도구의 꼴). 비밀키는 저장소에 없다(만들고 버렸다). 운영 키가 아니다.
//! - `relay-manifest.json` — 판 1.2.3 의 매니페스트. 해시는 `bin/` 의 가짜 바이너리(글 한 줄)의 것이다.
//! - `relay-manifest.json.sig` — 그 키로 `tauri signer sign --app-version 1.2.3` 한 서명. `other-key.sig` 는 다른 키,
//!   `other-version.sig` 는 `--app-version 1.2.4`, `no-version.sig` 는 `--app-version` 없이 서명했다.
//!
//! 다시 만드는 법(매니페스트를 바꾸면 서명도 다시): `pnpm --filter @pitwall/desktop exec tauri signer generate --ci -p <암호> -w
//! <임시>/a.key` 로 키를 만들고 `a.key.pub` 를 `updater.pub` 로, `tauri signer sign -k "$(cat a.key)" -p <암호> --app-version
//! 1.2.3 relay-manifest.json` 으로 서명한 뒤 비밀키를 지운다.

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use pw_release::relay::{
    self, DEV_DIST_DIR, Entry, MANIFEST_FILE, MAX_BINARY_BYTES, PLATFORMS, Source,
};
use semver::Version;

const VERSION: &str = "1.2.3";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/relay")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap()
}

fn manifest_bytes() -> Vec<u8> {
    std::fs::read(fixture(MANIFEST_FILE)).unwrap()
}

fn verify_with(manifest: &[u8], signature: &str) -> Result<relay::Manifest, pw_release::Error> {
    relay::verify(manifest, signature, &read("updater.pub"), VERSION)
}

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[test]
fn a_signed_manifest_verifies_and_names_a_binary_per_os_and_arch() {
    let manifest = verify_with(&manifest_bytes(), &read("relay-manifest.json.sig")).unwrap();
    assert_eq!(manifest.version, VERSION);
    let linux = manifest.asset("linux", "x86_64").unwrap();
    assert_eq!(linux.name, "pitwall-relay-1.2.3-linux-x86_64");
    // macOS 는 아키텍처별 바이너리다(결정 M7-D4, 2026-11-23 바뀜 — 유니버설을 내지 않는다).
    assert_eq!(
        manifest.asset("macos", "aarch64").unwrap().name,
        "pitwall-relay-1.2.3-macos-aarch64"
    );
    assert_eq!(
        manifest.asset("macos", "x86_64").unwrap().name,
        "pitwall-relay-1.2.3-macos-x86_64"
    );
    assert_eq!(manifest.assets.len(), 4);
    assert!(manifest.asset("windows", "x86_64").is_none());
}

/// 서명 파일의 서명 줄 · trusted comment · 전역 서명의 어느 한 바이트를 바꿔도 거절이다(완료 기준 「서명을 한 바이트 바꾸면 거절」).
/// untrusted comment(첫 줄)는 서명이 덮지 않는 자리라 보지 않는다.
#[test]
fn changing_any_signed_byte_of_the_signature_is_refused() {
    let text =
        String::from_utf8(B64.decode(read("relay-manifest.json.sig").trim()).unwrap()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    let manifest = manifest_bytes();
    let rebuild = |lines: &[String]| B64.encode(format!("{}\n", lines.join("\n")));
    let original: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
    assert!(verify_with(&manifest, &rebuild(&original)).is_ok());
    let mut tried = 0;
    // 서명 줄(1) · 전역 서명 줄(3)은 base64 를 풀어 바이트마다 한 비트를 뒤집는다.
    for line in [1usize, 3] {
        let raw = B64.decode(lines[line]).unwrap();
        for i in 0..raw.len() {
            let mut changed = raw.clone();
            changed[i] ^= 0x01;
            let mut edited = original.clone();
            edited[line] = B64.encode(&changed);
            assert!(
                verify_with(&manifest, &rebuild(&edited)).is_err(),
                "line {line} byte {i} flipped but the manifest still verified"
            );
            tried += 1;
        }
    }
    // trusted comment 는 글자마다 바꾼다(판 칸도 여기 든다).
    let comment = lines[2].as_bytes();
    for i in "trusted comment: ".len()..comment.len() {
        let mut changed = comment.to_vec();
        changed[i] = if changed[i] == b'x' { b'y' } else { b'x' };
        let mut edited = original.clone();
        edited[2] = String::from_utf8(changed).unwrap();
        assert!(
            verify_with(&manifest, &rebuild(&edited)).is_err(),
            "trusted comment byte {i} changed but the manifest still verified"
        );
        tried += 1;
    }
    assert!(tried > 100, "{tried}");
}

#[test]
fn changing_any_byte_of_the_manifest_is_refused() {
    let manifest = manifest_bytes();
    let signature = read("relay-manifest.json.sig");
    for i in 0..manifest.len() {
        let mut changed = manifest.clone();
        changed[i] ^= 0x01;
        assert!(
            verify_with(&changed, &signature).is_err(),
            "manifest byte {i} flipped but it still verified"
        );
    }
}

#[test]
fn a_signature_by_another_key_is_refused() {
    let e = verify_with(&manifest_bytes(), &read("other-key.sig")).unwrap_err();
    assert!(e.to_string().contains("does not verify"), "{e}");
}

/// 서명에 실린 판이 이 Desktop 의 판이어야 한다 — 다른 판으로 서명했거나 판 없이 서명했으면 거절, 서명은 맞아도 다른 판의 Desktop
/// 이 읽으면 거절.
#[test]
fn the_signed_version_must_be_this_desktop_version() {
    let e = verify_with(&manifest_bytes(), &read("other-version.sig")).unwrap_err();
    assert!(e.to_string().contains("signed for 1.2.4"), "{e}");
    let e = verify_with(&manifest_bytes(), &read("no-version.sig")).unwrap_err();
    assert!(e.to_string().contains("carries no version"), "{e}");
    let e = relay::verify(
        &manifest_bytes(),
        &read("relay-manifest.json.sig"),
        &read("updater.pub"),
        "1.2.4",
    )
    .unwrap_err();
    assert!(e.to_string().contains("signed for 1.2.3"), "{e}");
}

fn verified() -> relay::Manifest {
    verify_with(&manifest_bytes(), &read("relay-manifest.json.sig")).unwrap()
}

#[test]
fn a_binary_must_carry_the_manifest_hash_of_its_os_and_arch() {
    let manifest = verified();
    let dir = tempfile::tempdir().unwrap();
    let linux = fixture("bin/pitwall-relay-1.2.3-linux-x86_64");
    assert_eq!(
        manifest
            .check_binary("linux", "x86_64", &linux)
            .unwrap()
            .name,
        "pitwall-relay-1.2.3-linux-x86_64"
    );
    // 같은 크기 · 다른 바이트 — 매니페스트에 없는 해시.
    let mut bytes = std::fs::read(&linux).unwrap();
    bytes[0] ^= 0x20;
    let tampered = dir.path().join("tampered");
    std::fs::write(&tampered, &bytes).unwrap();
    let e = manifest
        .check_binary("linux", "x86_64", &tampered)
        .unwrap_err();
    assert!(e.to_string().contains("sha256"), "{e}");
    // 다른 아키텍처의 (매니페스트에 있는) 바이너리.
    let arm = fixture("bin/pitwall-relay-1.2.3-linux-aarch64");
    assert!(manifest.check_binary("linux", "x86_64", &arm).is_err());
    // 크기가 다르면 해시를 보기 전에 거절.
    let longer = dir.path().join("longer");
    std::fs::write(
        &longer,
        [std::fs::read(&linux).unwrap(), b"x".to_vec()].concat(),
    )
    .unwrap();
    let e = manifest
        .check_binary("linux", "x86_64", &longer)
        .unwrap_err();
    assert!(e.to_string().contains("bytes"), "{e}");
    // 매니페스트에 없는 플랫폼.
    assert!(manifest.check_binary("windows", "x86_64", &linux).is_err());
}

/// 서명은 맞는데 꼴이 틀린 매니페스트는 없다(서명하는 쪽이 [`relay::assemble`] 만 쓴다). 꼴 검사는 그래도 읽는 쪽에 있다 — 개발 빌드
/// 매니페스트(서명 없음)가 같은 검사를 지난다.
#[test]
fn a_dev_manifest_is_read_only_from_the_dev_source_and_checked_for_its_version() {
    let dir = tempfile::tempdir().unwrap();
    let Source::Dev(dist) = relay::source_for(true, Some(dir.path().as_os_str()), Path::new("/ws"))
    else {
        panic!("debug with the variable is a dev build");
    };
    assert_eq!(dist.dir(), dir.path());
    assert!(
        relay::read_dev_manifest(&dist, VERSION).is_err(),
        "no manifest yet"
    );
    std::fs::write(dir.path().join(MANIFEST_FILE), manifest_bytes()).unwrap();
    assert_eq!(
        relay::read_dev_manifest(&dist, VERSION).unwrap(),
        verified()
    );
    let e = relay::read_dev_manifest(&dist, "1.2.4").unwrap_err();
    assert!(e.to_string().contains("not 1.2.4"), "{e}");
    let mut twice = verified();
    twice.assets.push(twice.assets[0].clone());
    std::fs::write(dir.path().join(MANIFEST_FILE), twice.to_bytes()).unwrap();
    let e = relay::read_dev_manifest(&dist, VERSION).unwrap_err();
    assert!(e.to_string().contains("twice"), "{e}");
}

/// **release 빌드는 환경 변수가 있어도 개발 빌드 갈래로 가지 않는다**(완료 기준 ② — `instance.rs` 의 `TEST_KEY_ENV` 와 같은 꼴).
/// 판정은 `source_for` 에 있고 `source` 는 `cfg!(debug_assertions)` 를 넘긴다 — 이 시험 바이너리(debug)에서는 개발 갈래다.
#[test]
fn a_release_build_never_takes_the_dev_build_branch() {
    let ws = Path::new("/ws");
    let dir = Path::new("/elsewhere/relay-dist");
    assert_eq!(
        relay::source_for(false, Some(dir.as_os_str()), ws),
        Source::Signed
    );
    assert_eq!(relay::source_for(false, None, ws), Source::Signed);
    let Source::Dev(dist) = relay::source_for(true, Some(dir.as_os_str()), ws) else {
        panic!("debug follows the variable");
    };
    assert_eq!(dist.dir(), dir);
    let Source::Dev(dist) = relay::source_for(true, Some("".as_ref()), ws) else {
        panic!("an empty variable is unset");
    };
    assert_eq!(dist.dir(), ws.join(DEV_DIST_DIR));
    assert_eq!(
        matches!(relay::source(), Source::Dev(_)),
        cfg!(debug_assertions)
    );
}

fn entry(name: &str) -> Entry {
    Entry {
        name: name.to_owned(),
        sha256: "ab".repeat(32),
        size: 7,
    }
}

#[test]
fn a_release_manifest_needs_every_platform_once() {
    let version = Version::parse(VERSION).unwrap();
    let all: Vec<Entry> = PLATFORMS
        .iter()
        .map(|p| entry(&relay::asset_name(&version, p)))
        .collect();
    let manifest = relay::assemble(&version, &all, true).unwrap();
    assert_eq!(manifest.assets.len(), 4, "linux and macOS, two arches each");
    let e = relay::assemble(&version, &all[..2], true).unwrap_err();
    assert!(
        e.to_string()
            .contains("no entry for pitwall-relay-1.2.3-macos-aarch64"),
        "{e}"
    );
    // 개발 빌드는 만든 것만.
    let partial = relay::assemble(&version, &all[..1], false).unwrap();
    assert_eq!(partial.assets.len(), 1);
    let mut doubled = all.clone();
    doubled.push(all[0].clone());
    assert!(relay::assemble(&version, &doubled, false).is_err());
    let e = relay::assemble(
        &version,
        &[entry("pitwall-relay-1.2.2-linux-x86_64")],
        false,
    )
    .unwrap_err();
    assert!(e.to_string().contains("not a relay asset of 1.2.3"), "{e}");
    let mut bad = entry(&relay::asset_name(&version, &PLATFORMS[0]));
    bad.sha256 = "AB".repeat(32);
    assert!(relay::assemble(&version, &[bad], false).is_err());
    // 만든 매니페스트는 읽는 쪽 검사를 지난다.
    let parsed: relay::Manifest = serde_json::from_slice(&manifest.to_bytes()).unwrap();
    assert_eq!(parsed, manifest);
}

/// 크기 상한(메인 판정 M7-P13). 넘으면 항목을 만들지 않는다 — 빌드가 실패한다.
#[test]
fn a_binary_over_the_size_limit_fails_the_build() {
    let file = fixture("bin/pitwall-relay-1.2.3-linux-x86_64");
    let size = std::fs::metadata(&file).unwrap().len();
    assert!(relay::describe(&file, "x", size).is_ok());
    let e = relay::describe(&file, "x", size - 1).unwrap_err();
    assert!(e.to_string().contains("over the relay binary limit"), "{e}");
    assert_eq!(MAX_BINARY_BYTES, 56 * 1024 * 1024, "18-m7-plan M7-P13");
}

/// 앱의 updater 공개키(운영 키의 공개 쪽)가 Relay 매니페스트를 검증할 수 있는 꼴이고, 시험 키와 다르다.
#[test]
fn the_app_updater_key_is_a_minisign_public_key_and_not_the_test_key() {
    let conf: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../apps/desktop/src-tauri/tauri.conf.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let key = conf["plugins"]["updater"]["pubkey"].as_str().unwrap();
    assert_ne!(key.trim(), read("updater.pub").trim());
    let e = relay::verify(
        &manifest_bytes(),
        &read("relay-manifest.json.sig"),
        key,
        VERSION,
    )
    .unwrap_err();
    // 키는 읽혔고(꼴이 맞다) 서명만 맞지 않는다.
    assert!(e.to_string().contains("does not verify"), "{e}");
}

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pw-release"))
        .args(args)
        .output()
        .unwrap()
}

/// 워크플로가 부르는 길: 바이너리마다 `relay-describe`, 모아서 `relay-manifest`. `--check` 는 올릴 바이트가 항목과 다르면 실패한다.
#[test]
fn the_cli_describes_binaries_and_assembles_the_manifest() {
    let version = Version::parse(pw_release::SOURCE_VERSION).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (bins, entries) = (dir.path().join("bins"), dir.path().join("entries"));
    std::fs::create_dir_all(&bins).unwrap();
    std::fs::create_dir_all(&entries).unwrap();
    for platform in &PLATFORMS {
        let name = relay::asset_name(&version, platform);
        std::fs::write(bins.join(&name), platform.asset).unwrap();
        let out = cli(&[
            "relay-describe",
            "--file",
            bins.join(&name).to_str().unwrap(),
            "--platform",
            platform.asset,
            "--out",
            entries
                .join(format!("{}.json", platform.asset))
                .to_str()
                .unwrap(),
        ]);
        assert!(out.status.success(), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("relay binary size:"));
    }
    let manifest = dir.path().join(MANIFEST_FILE);
    let assemble = |check: Option<&Path>| {
        let mut args = vec![
            "relay-manifest",
            "--entries",
            entries.to_str().unwrap(),
            "--out",
            manifest.to_str().unwrap(),
            "--complete",
            "true",
        ];
        if let Some(check) = check {
            args.extend(["--check", check.to_str().unwrap()]);
        }
        cli(&args)
    };
    let out = assemble(Some(&bins));
    assert!(out.status.success(), "{out:?}");
    let written: relay::Manifest =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(written.version, version.to_string());
    assert_eq!(written.assets.len(), 4);
    // 항목을 만든 뒤 바뀐 바이너리.
    std::fs::write(
        bins.join(relay::asset_name(&version, &PLATFORMS[0])),
        "linux-x86_65",
    )
    .unwrap();
    let out = assemble(Some(&bins));
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("does not match its entry"),
        "{out:?}"
    );
    // 플랫폼 하나가 빠진 릴리스 매니페스트는 만들지 않는다.
    std::fs::remove_file(entries.join("macos-x86_64.json")).unwrap();
    let out = assemble(None);
    assert!(!out.status.success());
    // 모르는 플랫폼.
    let out = cli(&[
        "relay-describe",
        "--file",
        "x",
        "--platform",
        "windows-x86_64",
        "--out",
        "y",
    ]);
    assert!(!out.status.success());
}
