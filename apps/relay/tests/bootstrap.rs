//! 설치 · SSH 자동 페어링 · 해제(18-m7-plan M7-06 시험 ⑧ ⑨ ⑩ · ④ 의 해시 도구 없는 원격) — 실제 `pitwall-relay` 를 가짜
//! `ssh`(`pw_link::fake_ssh` — 원격 명령을 이 기기의 `sh` 로 돌린다)로 「원격」 에 설치하고, 그 `proxy` 의 stdio 로 링크를 연다.
//! **실제 SSH 는 쓰지 않는다.** 원격 `$HOME` 은 임시 폴더(공백 · 따옴표 · `$` 가 든 이름)이고 원격 환경은 `HOME` · `PATH` 뿐이다.
//!
//! 올리는 「바이너리」 는 이 시험이 빌드한 `pitwall-relay` 를 부르는 작은 스크립트다 — debug 빌드의 Relay 는 바이너리 상한(메인
//! 판정 M7-P13 · `MAX_BINARY_BYTES`)을 넘어 설치기가 올리지 않는다. 설치 순서(해시 · rename)는 그대로 돈다.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_link::fake_ssh::{FakeRemote, write_executable};
use pw_link::install::{Installer, Progress, RelayDist};
use pw_link::ssh::{Ssh, SshTarget};
use pw_link::{
    DesktopIdentity, DeviceKey, DialError, Expect, LinkLimits, RejectReason, Transport, Versions,
};
use pw_release::relay::{
    MANIFEST_FILE, MAX_BINARY_BYTES, PLATFORMS, assemble, asset_name, describe,
};
use pw_rpc::Role;

const BIN: &str = env!("CARGO_BIN_EXE_pitwall-relay");
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn channel() -> &'static str {
    option_env!("PITWALL_RELEASE_CHANNEL").unwrap_or("dev")
}

/// 이 기기의 플랫폼(가짜 원격의 `uname -sm` 그대로).
fn local_platform() -> &'static pw_release::relay::Platform {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    PLATFORMS
        .iter()
        .find(|p| p.runs_on.contains(&(os, arch)))
        .expect("a relay platform")
}

/// 개발 빌드 폴더: 이 시험의 Relay 를 부르는 스크립트와 서명 없는 매니페스트.
fn dev_dist(dir: &Path) -> PathBuf {
    let dist = dir.join("relay-dist-dev");
    std::fs::create_dir_all(&dist).unwrap();
    let version = semver::Version::parse(VERSION).unwrap();
    let name = asset_name(&version, local_platform());
    let path = dist.join(&name);
    write_executable(&path, &format!("#!/bin/sh\nexec '{BIN}' \"$@\"\n"));
    let entry = describe(&path, &name, MAX_BINARY_BYTES).unwrap();
    let manifest = assemble(&version, &[entry], false).unwrap();
    std::fs::write(dist.join(MANIFEST_FILE), manifest.to_bytes()).unwrap();
    // SAFETY: nextest 는 시험마다 프로세스를 따로 띄운다.
    unsafe { std::env::set_var(pw_release::relay::DEV_DIST_ENV, &dist) };
    dist
}

struct Offline;

impl pw_link::install::Fetch for Offline {
    fn get<'a>(
        &'a self,
        _url: &'a str,
        _max: u64,
    ) -> pw_core::BoxFuture<'a, std::io::Result<Vec<u8>>> {
        Box::pin(async { Err(std::io::Error::other("offline")) })
    }
}

fn installer(dir: &Path, remote: &FakeRemote) -> Installer {
    let ssh = Ssh::with_program(remote.ssh.clone(), SshTarget::alias("mini"), Vec::new())
        .unwrap()
        .env(remote.env(&[]));
    Installer::new(
        Arc::new(ssh),
        Versions::new(VERSION, channel()),
        RelayDist::new(dir.join("cache/relay-dist"), Arc::new(Offline)),
    )
    .unwrap()
}

/// 원격의 relay.toml — 시험 뒤 데몬이 오래 남지 않게 유휴 시한을 짧게.
fn short_idle(remote: &FakeRemote) {
    let config = remote.home.join(".config/pitwall/relay");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("relay.toml"),
        "schema = 1\n[idle]\nexit_after = \"10s\"\n",
    )
    .unwrap();
}

/// 원격에서 뜬 데몬을 끝낸다(`status` 의 pid 로).
struct Daemon {
    home: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let out = std::process::Command::new(BIN)
            .arg("status")
            .env_clear()
            .env("HOME", &self.home)
            .output();
        if let Ok(out) = out
            && let Ok(status) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
            && let Some(pid) = status["pid"].as_i64()
        {
            // SAFETY: 이 시험의 원격 홈으로 뜬 데몬의 pid 다.
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }
}

fn identity(key: &Arc<DeviceKey>) -> DesktopIdentity {
    DesktopIdentity {
        key: Arc::clone(key),
        versions: Versions::new(VERSION, channel()),
    }
}

fn limits() -> LinkLimits {
    LinkLimits {
        ping_every: Duration::from_millis(500),
        dead_after: Duration::from_secs(10),
        ..LinkLimits::default()
    }
}

async fn dial(
    installer: &Installer,
    key: &Arc<DeviceKey>,
) -> Result<pw_link::DesktopLink, DialError> {
    let stream = installer.transport().open().await.unwrap();
    pw_link::dial(stream, &identity(key), &Expect::FirstContact, limits()).await
}

fn table(remote: &FakeRemote) -> Vec<pw_link::Pairing> {
    pw_link::pairing::load(&remote.relay_dir().join("pairings.json")).unwrap()
}

fn no_progress(_: Progress) {}

/// ⑧ SSH 자동 페어링 뒤 페어링 표에는 그 Desktop 키만 owner 로 있고, 그 키의 링크는 owner · 다른 키의 링크는 거절(role 없음)이다.
/// ⑨ 이미 설치된 머신에 두 번째 Desktop 이 붙으면 설치는 「이미 있음」 이고 그 키도 표에 든다. ⑩ 해제하면 그 키의 열린 링크가
/// 끊기고 다시 붙지 못한다 — 다른 Desktop 은 그대로다.
#[tokio::test(flavor = "multi_thread")]
async fn ssh_pairing_admits_only_the_paired_desktops_and_unpairing_cuts_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "a b'c$d", true);
    short_idle(&remote);
    dev_dist(dir.path());
    let _daemon = Daemon {
        home: remote.home.clone(),
    };
    let installer = installer(dir.path(), &remote);

    let first = Arc::new(DeviceKey::generate().unwrap());
    let done = installer
        .bootstrap(&first.public(), "mac", &no_progress)
        .await
        .unwrap();
    assert!(done.uploaded && done.paired_now);
    assert_eq!(
        table(&remote)
            .iter()
            .map(|p| (p.key, p.role))
            .collect::<Vec<_>>(),
        vec![(first.public(), Role::Owner)],
        "only this desktop's key is paired"
    );
    let link = dial(&installer, &first).await.unwrap();
    assert_eq!(link.role(), Role::Owner);
    assert_eq!(
        *link.machine(),
        done.machine,
        "the machine id pairing printed is the relay's key"
    );

    let stranger = Arc::new(DeviceKey::generate().unwrap());
    assert!(matches!(
        dial(&installer, &stranger).await.err(),
        Some(DialError::Rejected(RejectReason::Unpaired))
    ));

    // ⑨ 두 번째 Desktop.
    let second = Arc::new(DeviceKey::generate().unwrap());
    let seen = std::sync::Mutex::new(Vec::new());
    let again = installer
        .bootstrap(&second.public(), "mac2", &|p| seen.lock().unwrap().push(p))
        .await
        .unwrap();
    assert!(!again.uploaded && again.paired_now);
    assert!(seen.lock().unwrap().contains(&Progress::AlreadyInstalled));
    assert_eq!(again.machine, done.machine);
    assert_eq!(table(&remote).len(), 2);
    let second_link = dial(&installer, &second).await.unwrap();

    // ⑩ 해제.
    assert!(installer.unpair(&first.id()).await.unwrap());
    tokio::time::timeout(Duration::from_secs(10), link.closed())
        .await
        .expect("the unpaired desktop's link is cut");
    assert!(matches!(
        dial(&installer, &first).await.err(),
        Some(DialError::Rejected(RejectReason::Unpaired))
    ));
    assert_eq!(second_link.role(), Role::Owner);
    assert!(
        dial(&installer, &second).await.is_ok(),
        "the other desktop still connects"
    );
    second_link.close();
}

/// 원격에 `sha256sum` · `shasum` 이 없으면 올린 바이너리의 `--verify` 로 해시를 보고 설치한다. 다시 붙으면 같은 길로 「이미 있음」.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_without_hash_tools_verifies_with_the_relay_itself() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", false);
    assert!(!remote.bin.join("sha256sum").exists() && !remote.bin.join("shasum").exists());
    short_idle(&remote);
    dev_dist(dir.path());
    let installer = installer(dir.path(), &remote);
    let probe = installer.probe().await.unwrap();
    assert_eq!(probe.installed, pw_link::install::Installed::Absent);
    assert!(installer.install(&probe, &no_progress).await.unwrap());
    let probe = installer.probe().await.unwrap();
    assert_eq!(probe.installed, pw_link::install::Installed::Unknown);
    assert!(
        !installer.install(&probe, &no_progress).await.unwrap(),
        "already installed"
    );
}

/// `--verify` 는 해시가 다르면 1 이다.
#[test]
fn verify_answers_by_the_hash() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f");
    std::fs::write(&file, b"abc").unwrap();
    let run = |sha: &str| {
        std::process::Command::new(BIN)
            .args(["--verify", file.to_str().unwrap(), sha])
            .status()
            .unwrap()
            .code()
    };
    assert_eq!(
        run("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        Some(0)
    );
    assert_eq!(
        run("ca7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        Some(1)
    );
}
