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

/// 판 `version` 의 데몬 소켓 자리(가짜 원격의 환경 — `HOME` 만). 임시 폴더는 이 프로세스의 것(`TMPDIR` 이 없는 원격 프로세스도
/// macOS 에서는 같은 사용자 임시 폴더, Linux 에서는 `/tmp` 를 쓴다 — `std::env::temp_dir`).
fn socket_of(home: &Path, version: &str) -> PathBuf {
    let mut env = pw_core::config::PathEnv::empty(pw_core::config::OsFamily::Unix);
    env.home = Some(home.to_path_buf());
    let paths = pw_relay::paths::relay_paths(&pw_core::config::AppPaths::resolve(&env).unwrap());
    pw_relay::socket::endpoint(
        &paths,
        CHANNEL,
        version,
        &std::env::temp_dir(),
        pw_relay::socket::current_uid(),
    )
    .unwrap()
}

/// 원격 환경 그대로(`HOME` · 판)로 부른 `status` 의 종료 코드. `limit` 안에 끝나지 않으면 죽이고 `None`.
fn status_code(home: &Path, version: &str, limit: Duration) -> Option<i32> {
    let mut child = std::process::Command::new(BIN)
        .arg("status")
        .env_clear()
        .env("HOME", home)
        .env(pw_relay::DEV_VERSION_ENV, version)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn bump(version: &str, by: u64) -> String {
    let mut version = semver::Version::parse(version).unwrap();
    version.patch += by;
    version.pre = semver::Prerelease::EMPTY;
    version.to_string()
}

/// 리뷰 F1: 떠 있는 옛 판 데몬의 증명 전 연결 상한이 차 있어(같은 uid 의 연결이 몰림) `status` 가 답을 받지 못해도 「데몬 없음」
/// 이 아니다 — `status` 는 4(모름)를 내고, 정리는 그 데몬의 바이너리를 가장 오래된 판이어도 남긴다. 상한이 풀리면 다시 답한다.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_old_daemon_keeps_its_binary() {
    assert_eq!(
        option_env!("PITWALL_RELEASE_CHANNEL").unwrap_or("dev"),
        CHANNEL
    );
    let old_version = BUILD.to_owned();
    let middle = bump(BUILD, 1);
    let new_version = bump(BUILD, 2);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let remote = FakeRemote::new(&root, "home", true);
    let config = remote.home.join(".config/pitwall/relay");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("relay.toml"),
        "schema = 1\n[idle]\nexit_after = \"30s\"\n",
    )
    .unwrap();
    let _daemons = Daemons {
        home: remote.home.clone(),
        versions: vec![old_version.clone()],
    };
    let key = Arc::new(DeviceKey::generate().unwrap());
    let no_progress = |_: pw_link::install::Progress| {};

    // 가장 오래된 판의 데몬을 띄운다(붙었다가 뗀다 — 데몬은 유휴 시한까지 산다).
    use_dev_dist(&root, &old_version);
    let old = installer(&root, &remote, &old_version);
    old.bootstrap(&key.public(), "mac", &no_progress)
        .await
        .unwrap();
    let old_host = remote_host(&old, &key, &old_version);
    old_host.attach().await.unwrap();
    old_host.detach();
    assert_eq!(
        status_code(&remote.home, &old_version, Duration::from_secs(15)),
        Some(0)
    );
    // 가운데 판(데몬 없음)과 새 판.
    write_executable(
        &remote
            .relay_dir()
            .join(format!("pitwall-relay-{CHANNEL}-{middle}")),
        "#!/bin/sh\nexit 3\n",
    );
    use_dev_dist(&root, &new_version);
    let new = installer(&root, &remote, &new_version);
    new.bootstrap(&key.public(), "mac", &no_progress)
        .await
        .unwrap();

    // 옛 판 데몬의 증명 전 자리를 채운다 — 첫 틀을 보내지 않는 연결을 계속 연다.
    let socket = socket_of(&remote.home, &old_version);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let filler = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut held = std::collections::VecDeque::new();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok(stream) = std::os::unix::net::UnixStream::connect(&socket) {
                    held.push_back(stream);
                    if held.len() > 400 {
                        held.pop_front();
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        status_code(&remote.home, &old_version, Duration::from_secs(15)),
        Some(4),
        "a busy daemon is not \"no daemon\""
    );
    let survey = new.survey().await.unwrap();
    let old_file = format!("pitwall-relay-{CHANNEL}-{old_version}");
    assert!(
        survey.to_remove().contains(&old_file),
        "the oldest version is a clean-up candidate: {survey:?}"
    );
    let cleaned = new.clean(&survey).await.unwrap();
    assert!(
        !cleaned.removed.contains(&old_file),
        "removed a running daemon's binary: {cleaned:?}"
    );
    assert!(remote.relay_dir().join(&old_file).exists());
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    filler.join().unwrap();

    // 상한이 풀리면(쥔 연결은 첫 틀 시한 5초에 닫힌다) 다시 답한다.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while status_code(&remote.home, &old_version, Duration::from_secs(15)) != Some(0) {
        assert!(
            std::time::Instant::now() < deadline,
            "the daemon never answered again"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let again = new.survey().await.unwrap();
    assert_eq!(again.old_daemons().len(), 1, "{again:?}");
}

/// 받기는 하되 답하지 않는 데몬이면 `status` 는 시간 상한(5초) 안에 4 로 끝난다 — 조사를 붙잡지 않는다(F1).
#[test]
fn status_gives_up_on_a_daemon_that_never_answers() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap().join("home");
    let socket = socket_of(&home, "7.7.7");
    let folder = socket.parent().unwrap();
    std::fs::create_dir_all(folder).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _ = std::fs::remove_file(&socket);
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(
        status_code(&home, "7.7.7", Duration::from_secs(20)),
        Some(4)
    );
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "{:?}",
        started.elapsed()
    );
    drop(listener);
    let _ = std::fs::remove_file(&socket);
    // 소켓이 없으면 3(데몬이 없는 것이 확실하다).
    assert_eq!(
        status_code(&home, "7.7.7", Duration::from_secs(20)),
        Some(3)
    );
}
