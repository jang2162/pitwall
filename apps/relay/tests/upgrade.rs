//! 판 바꾸기(18-m7-plan M7-07 시험 ① · 결정 M7-D6 ①) — 실제 `pitwall-relay` 를 가짜 `ssh`(`pw_link::fake_ssh`)로 「원격」 에 판 둘로
//! 올린다. **실제 SSH 는 쓰지 않는다.** 판 둘은 이 빌드 하나로 만든다: 개발 채널 빌드는 `PITWALL_RELAY_DEV_VERSION` 을 판으로 쓰고
//! (`pw_relay::version`), 올리는 「바이너리」 는 그 변수를 붙여 이 빌드를 부르는 스크립트다(debug 빌드는 바이너리 상한을 넘어
//! 설치기가 그대로 올리지 않는다 — `bootstrap.rs` 와 같은 까닭).
//!
//! 판 A 의 Desktop 이 터미널을 연 채로, 판 B 의 Desktop 이 같은 머신에 붙으면: 판 B 를 깔고 판 B 데몬이 따로 뜬다(판 A 데몬에 붙지
//! 않는다 — 소켓 이름 · 핸드셰이크의 판 비교), 판 A 의 데몬 · 셸은 산다, 판 B 의 조사가 「옛 판에 터미널 1개」 를 읽는다, [옛 판
//! 끝내기] 가 판 A 데몬을 끝내고 그 데몬이 셸을 끝낸다, 판 B 링크는 그대로다.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::{Connection, Host};
use pw_link::fake_ssh::{FakeRemote, write_executable};
use pw_link::install::{Installer, RelayDist};
use pw_link::ssh::{Ssh, SshTarget};
use pw_link::upgrade::StopOutcome;
use pw_link::{DesktopIdentity, DeviceKey, Expect, LinkLimits, RemoteHost, Transport, Versions};
use pw_release::relay::{
    MANIFEST_FILE, MAX_BINARY_BYTES, PLATFORMS, assemble, asset_name, describe,
};
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_pitwall-relay");
const BUILD: &str = env!("CARGO_PKG_VERSION");
const CHANNEL: &str = "dev";

/// 이 빌드 판의 다음 판(판 B).
fn next_version() -> String {
    let mut version = semver::Version::parse(BUILD).unwrap();
    version.patch += 1;
    version.pre = semver::Prerelease::EMPTY;
    version.to_string()
}

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

/// `version` 의 개발 빌드 폴더: 그 판을 내세우는 이 빌드의 Relay 를 부르는 스크립트와 서명 없는 매니페스트. 설치기가 이 폴더를
/// 읽게 한다.
fn use_dev_dist(dir: &Path, version: &str) {
    let dist = dir.join(format!("relay-dist-{version}"));
    std::fs::create_dir_all(&dist).unwrap();
    let parsed = semver::Version::parse(version).unwrap();
    let name = asset_name(&parsed, local_platform());
    let path = dist.join(&name);
    write_executable(
        &path,
        &format!(
            "#!/bin/sh\n{}={version} exec '{BIN}' \"$@\"\n",
            pw_relay::DEV_VERSION_ENV
        ),
    );
    let entry = describe(&path, &name, MAX_BINARY_BYTES).unwrap();
    let manifest = assemble(&parsed, &[entry], false).unwrap();
    std::fs::write(dist.join(MANIFEST_FILE), manifest.to_bytes()).unwrap();
    // SAFETY: nextest 는 시험마다 프로세스를 따로 띄운다. 설치기를 부르는 동안에는 바꾸지 않는다.
    unsafe { std::env::set_var(pw_release::relay::DEV_DIST_ENV, &dist) };
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

fn installer(dir: &Path, remote: &FakeRemote, version: &str) -> Arc<Installer> {
    let ssh = Ssh::with_program(remote.ssh.clone(), SshTarget::alias("mini"), Vec::new())
        .unwrap()
        .env(remote.env(&[]));
    Arc::new(
        Installer::new(
            Arc::new(ssh),
            Versions::new(version, CHANNEL),
            RelayDist::new(dir.join("cache/relay-dist"), Arc::new(Offline)),
        )
        .unwrap(),
    )
}

/// 설치기의 `proxy` 전송(판마다).
struct ProxyOf(Arc<Installer>);

impl Transport for ProxyOf {
    fn describe(&self) -> String {
        "fake ssh proxy".into()
    }

    fn open(&self) -> pw_core::BoxFuture<'_, std::io::Result<pw_link::LinkStream>> {
        Box::pin(async move { self.0.transport().open().await })
    }
}

fn remote_host(installer: &Arc<Installer>, key: &Arc<DeviceKey>, version: &str) -> RemoteHost {
    RemoteHost::new(
        format!("mini-{version}"),
        Arc::new(ProxyOf(Arc::clone(installer))) as Arc<dyn Transport>,
        Arc::new(DesktopIdentity {
            key: Arc::clone(key),
            versions: Versions::new(version, CHANNEL),
        }),
        Expect::FirstContact,
        LinkLimits {
            ping_every: Duration::from_millis(500),
            dead_after: Duration::from_secs(10),
            ..LinkLimits::default()
        },
    )
}

async fn call(conn: &mut Connection, id: i64, method: &str, params: Value) -> Value {
    conn.sink
        .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(60), conn.source.recv())
            .await
            .unwrap_or_else(|_| panic!("{method}: no reply"))
            .unwrap_or_else(|| panic!("{method}: closed"));
        if frame["id"] == json!(id) {
            assert!(frame.get("error").is_none(), "{method}: {frame}");
            return frame["result"].clone();
        }
    }
}

/// 프로젝트를 등록하고 그 창에서 터미널을 연다. (창 연결, 셸 pid) — 창 연결을 버리면 셸이 끝난다.
async fn open_terminal(host: &RemoteHost, root: &Path) -> (Connection, Connection, u32) {
    let mut welcome = host.connect(Caller::new(Role::Owner)).await.unwrap();
    let created = call(
        &mut welcome,
        1,
        "project.create",
        json!({"root": root.display().to_string()}),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_owned();
    let mut window = host
        .connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(id.clone()),
            window: Some("upgrade".into()),
        })
        .await
        .unwrap();
    let opened = call(
        &mut window,
        2,
        "terminal.create",
        json!({"projectId": id, "cols": 80, "rows": 24}),
    )
    .await;
    window.sink.send(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened["streamId"], "credit": 1000}}));
    let pid = loop {
        let frame = tokio::time::timeout(Duration::from_secs(20), window.source.recv())
            .await
            .expect("no terminal output")
            .expect("connection closed");
        if frame["method"] == "stream.chunk" && frame["params"]["data"]["kind"] == "started" {
            break frame["params"]["data"]["pid"].as_u64().unwrap() as u32;
        }
    };
    (welcome, window, pid)
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

async fn gone(what: &str, pid: u32) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while alive(pid) {
        assert!(std::time::Instant::now() < deadline, "{what} still runs");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 시험이 띄운 데몬들을 끝낸다(판마다 `status` 의 pid).
struct Daemons {
    home: PathBuf,
    versions: Vec<String>,
}

impl Drop for Daemons {
    fn drop(&mut self) {
        for version in &self.versions {
            let out = std::process::Command::new(BIN)
                .arg("status")
                .env_clear()
                .env("HOME", &self.home)
                .env(pw_relay::DEV_VERSION_ENV, version)
                .output();
            if let Ok(out) = out
                && let Ok(status) = serde_json::from_slice::<Value>(&out.stdout)
                && let Some(pid) = status["pid"].as_i64()
            {
                // SAFETY: 이 시험의 원격 홈으로 뜬 데몬의 pid 다.
                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            }
        }
    }
}

/// ① 판 A 의 터미널이 남은 머신에 판 B 가 붙으면 판 B 데몬이 따로 뜨고(판 A 데몬에 붙지 않는다) 판 A 의 셸은 산다. 판 B 의 조사가
/// 판 A 에 남은 터미널을 세고, [옛 판 끝내기] 가 판 A 데몬과 그 셸을 끝낸다. 판 B 링크는 그대로다.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_version_runs_its_own_daemon_beside_the_old_one() {
    assert_eq!(
        option_env!("PITWALL_RELEASE_CHANNEL").unwrap_or("dev"),
        CHANNEL,
        "this test makes two versions out of a dev-channel build"
    );
    let old_version = BUILD.to_owned();
    let new_version = next_version();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let remote = FakeRemote::new(&root, "home", true);
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let config = remote.home.join(".config/pitwall/relay");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("relay.toml"),
        format!(
            "schema = 1\nroots = [\"{}\"]\n[idle]\nexit_after = \"30s\"\n",
            work.display()
        ),
    )
    .unwrap();
    let _daemons = Daemons {
        home: remote.home.clone(),
        versions: vec![old_version.clone(), new_version.clone()],
    };
    let key = Arc::new(DeviceKey::generate().unwrap());
    let no_progress = |_: pw_link::install::Progress| {};

    // 판 A: 설치 · 붙기 · 터미널.
    use_dev_dist(&root, &old_version);
    let old = installer(&root, &remote, &old_version);
    old.bootstrap(&key.public(), "mac", &no_progress)
        .await
        .unwrap();
    let old_host = remote_host(&old, &key, &old_version);
    let machine = old_host.attach().await.unwrap();
    let (_old_welcome, _old_window, shell) = open_terminal(&old_host, &work).await;
    assert!(alive(shell));
    let old_daemon = old.survey().await.unwrap();
    assert!(old_daemon.old_daemons().is_empty(), "{old_daemon:?}");

    // 판 B: 새 판을 깔고 새 판 데몬에 붙는다.
    use_dev_dist(&root, &new_version);
    let new = installer(&root, &remote, &new_version);
    let done = new
        .bootstrap(&key.public(), "mac", &no_progress)
        .await
        .unwrap();
    assert!(
        done.uploaded,
        "the new version was installed beside the old"
    );
    assert_eq!(done.machine, machine, "the same machine");
    let new_host = remote_host(&new, &key, &new_version);
    new_host
        .attach()
        .await
        .expect("the new version gets its own daemon, not the old one");
    let mut new_welcome = new_host.connect(Caller::new(Role::Owner)).await.unwrap();
    let info = call(&mut new_welcome, 1, "host.ping", json!({})).await;
    assert!(info.is_object() || info.is_null(), "{info}");
    assert!(alive(shell), "the old version's shell ended");

    // 판 B 의 조사: 옛 판 데몬 하나, 터미널 하나 · 링크 하나.
    let survey = new.survey().await.unwrap();
    let old_daemons = survey.old_daemons();
    assert_eq!(old_daemons.len(), 1, "{survey:?}");
    let seen = &old_daemons[0];
    assert_eq!(seen.file, format!("pitwall-relay-{CHANNEL}-{old_version}"));
    assert_eq!(seen.status.version, old_version);
    assert_eq!((seen.status.terminals, seen.status.runs), (1, 0));
    assert_eq!(seen.status.links, 1);
    assert_eq!(seen.left(), 1);
    let new_status = old.survey().await.unwrap().old_daemons();
    assert_eq!(new_status.len(), 1, "the old version sees the new daemon");
    assert_eq!(new_status[0].status.version, new_version);
    assert_ne!(new_status[0].status.pid, seen.status.pid, "two daemons");

    // 정리: 두 판뿐이라 지울 것이 없다 — 떠 있는 옛 판 바이너리도 그대로.
    assert!(survey.to_remove().is_empty());
    assert!(
        remote
            .relay_dir()
            .join(format!("pitwall-relay-{CHANNEL}-{old_version}"))
            .exists()
    );

    // [옛 판 끝내기]: 옛 판 데몬이 끝나고, 그 데몬이 셸을 끝낸다. 새 판은 그대로다.
    assert_eq!(new.stop_old(seen).await.unwrap(), StopOutcome::Stopped);
    gone("the old daemon", seen.status.pid).await;
    gone("the old version's shell", shell).await;
    call(&mut new_welcome, 2, "host.ping", json!({})).await;
    assert!(new.survey().await.unwrap().old_daemons().is_empty());
}
