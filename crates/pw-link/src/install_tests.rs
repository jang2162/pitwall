//! 설치기 시험(18-m7-plan M7-06 시험 ① ② ③ ④ ⑥ · 호스트 키 ②) — 가짜 `ssh`(`crate::fake_ssh`)가 원격 명령을 이 기기의 `sh` 로
//! 돌린다. 원격에 올리는 「Relay」 는 셸 스크립트다(설치 · 해시 · rename 만 본다 — 실제 바이너리 · 페어링 · 링크는
//! `apps/relay/tests/bootstrap.rs`).

use std::path::Path;
use std::sync::Mutex;

use pw_release::relay::{
    MANIFEST_FILE, MAX_BINARY_BYTES, PLATFORMS, assemble, asset_name, describe,
};
use semver::Version;

use super::*;
use crate::fake_ssh::{FakeRemote, write_executable};
use crate::ssh::{AskpassSession, SshAskpass, SshTarget};
use crate::transport::Transport;

const VERSION: &str = "1.2.3";
const CHANNEL: &str = "dev";

const FAKE_RELAY: &str = r#"#!/bin/sh
case "$1" in
  --verify) exit 1 ;;
  pair) echo '{"machine":"SHA256:fakemachine","added":true}' ;;
  unpair) echo '{"removed":true}' ;;
  proxy) exec cat ;;
  *) exit 2 ;;
esac
"#;

/// 받기가 늘 실패한다(오프라인). 부른 주소를 적는다.
#[derive(Default)]
struct Offline(Mutex<Vec<String>>);

impl Fetch for Offline {
    fn get<'a>(&'a self, url: &'a str, _max: u64) -> BoxFuture<'a, io::Result<Vec<u8>>> {
        self.0.lock().unwrap().push(url.to_owned());
        Box::pin(async { Err(io::Error::other("offline")) })
    }
}

/// 개발 빌드 폴더: linux-x86_64 의 가짜 Relay 와 서명 없는 매니페스트.
fn dev_dist(dir: &Path) -> std::path::PathBuf {
    let dist = dir.join("relay-dist-dev");
    std::fs::create_dir_all(&dist).unwrap();
    let version = Version::parse(VERSION).unwrap();
    let platform = PLATFORMS
        .iter()
        .find(|p| p.asset == "linux-x86_64")
        .unwrap();
    let name = asset_name(&version, platform);
    let path = dist.join(&name);
    write_executable(&path, FAKE_RELAY);
    let entry = describe(&path, &name, MAX_BINARY_BYTES).unwrap();
    let manifest = assemble(&version, &[entry], false).unwrap();
    std::fs::write(dist.join(MANIFEST_FILE), manifest.to_bytes()).unwrap();
    dist
}

fn use_dev_dist(dist: &Path) {
    // SAFETY: nextest 는 시험마다 프로세스를 따로 띄운다 — 이 변수를 읽는 다른 스레드가 없다.
    unsafe { std::env::set_var(relay::DEV_DIST_ENV, dist) };
}

fn installer(remote: &FakeRemote, extra: &[(&str, &str)], dist: RelayDist) -> Installer {
    installer_with(remote, extra, dist, None)
}

fn installer_with(
    remote: &FakeRemote,
    extra: &[(&str, &str)],
    dist: RelayDist,
    askpass: Option<Arc<dyn SshAskpass>>,
) -> Installer {
    let mut ssh = Ssh::with_program(remote.ssh.clone(), SshTarget::alias("mini"), Vec::new())
        .unwrap()
        .env(remote.env(extra));
    if let Some(askpass) = askpass {
        ssh = ssh.askpass(askpass);
    }
    Installer::new(Arc::new(ssh), Versions::new(VERSION, CHANNEL), dist).unwrap()
}

fn offline_dist(dir: &Path) -> RelayDist {
    RelayDist::new(dir.join("cache/relay-dist"), Arc::new(Offline::default()))
}

fn installed_name() -> String {
    format!("pitwall-relay-{CHANNEL}-{VERSION}")
}

fn no_progress(_: Progress) {}

/// 설치된 것 · 남은 임시 파일.
fn remote_files(remote: &FakeRemote) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(remote.relay_dir())
        .map(|dir| {
            dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// ④ 원격 `$HOME` 에 공백 · 홑따옴표 · 겹따옴표 · `$` 가 있어도 바이너리는 그 홈의 `$DATA/relay/` 한 경로에 0700 으로 든다. 다시
/// 붙으면 「이미 있음」 이고 올리지 않는다. ① 모든 ssh 호출의 argv 에 호스트 키 확인을 끄는 옵션이 없고 호스트는 `--` 뒤다.
#[tokio::test]
async fn the_relay_lands_in_the_remote_data_folder_even_under_an_odd_home() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "a b'c\"d$HOME e", true);
    remote.uname("Linux x86_64");
    use_dev_dist(&dev_dist(dir.path()));
    let installer = installer(&remote, &[], offline_dist(dir.path()));

    let seen = Mutex::new(Vec::new());
    let progress = |p: Progress| seen.lock().unwrap().push(p);
    let key = crate::DeviceKey::generate().unwrap().public();
    let done = installer.bootstrap(&key, "mac", &progress).await.unwrap();
    assert!(done.uploaded);
    assert_eq!(done.machine.as_str(), "SHA256:fakemachine");
    let installed = remote.relay_dir().join(installed_name());
    let mode = std::fs::metadata(&installed).unwrap().permissions();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
        0o700
    );
    assert_eq!(
        remote_files(&remote),
        vec![installed_name()],
        "one path, no leftovers"
    );
    // 홈 밖(따옴표 · `$` 가 풀려 다른 곳)에 생긴 것이 없다.
    let stray: Vec<_> = std::fs::read_dir(dir.path().join("remote"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(stray.len(), 1, "{stray:?}");

    // 다시 붙기 — 이미 있음.
    seen.lock().unwrap().clear();
    let again = installer.bootstrap(&key, "mac", &progress).await.unwrap();
    assert!(!again.uploaded);
    assert!(seen.lock().unwrap().contains(&Progress::AlreadyInstalled));

    let calls = remote.calls();
    assert!(calls.len() >= 5, "{calls:?}");
    for argv in calls {
        let dash = argv
            .iter()
            .position(|a| a == "--")
            .expect("`--` before the host");
        let options = argv[..dash].join(" ").to_ascii_lowercase();
        for bypass in [
            "stricthostkeychecking",
            "userknownhostsfile",
            "/dev/null",
            "batchmode",
        ] {
            assert!(!options.contains(bypass), "{argv:?}");
        }
        assert_eq!(argv[dash + 1], "mini");
        assert_eq!(
            argv.len(),
            dash + 3,
            "one remote command after the host: {argv:?}"
        );
        assert!(argv[dash + 2].starts_with("sh -c '"), "{argv:?}");
    }
}

/// ② 올린 바이트가 원격에서 매니페스트 해시와 다르면 rename 하지 않고 임시 파일도 지운다.
#[tokio::test]
async fn a_remote_hash_mismatch_installs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    remote.uname("Linux x86_64");
    use_dev_dist(&dev_dist(dir.path()));
    let installer = installer(
        &remote,
        &[("FAKE_SSH_CORRUPT", "1")],
        offline_dist(dir.path()),
    );
    let probe = installer.probe().await.unwrap();
    assert_eq!(probe.installed, Installed::Absent);
    let err = installer.install(&probe, &no_progress).await.unwrap_err();
    assert!(matches!(err, InstallError::RemoteHashMismatch), "{err}");
    assert!(
        remote_files(&remote).is_empty(),
        "{:?}",
        remote_files(&remote)
    );
}

/// ⑥ 지원 밖 플랫폼은 감지에서 끝나고 아무것도 올리지 않는다(ssh 호출은 감지 하나뿐).
#[tokio::test]
async fn an_unsupported_platform_uploads_nothing() {
    for (uname, says) in [
        ("Linux i686", "64-bit"),
        ("Linux armv7l", "64-bit"),
        ("MINGW64_NT-10.0-19045 x86_64", "Windows"),
        ("FreeBSD amd64", "macOS and Linux only"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let remote = FakeRemote::new(dir.path(), "home", true);
        remote.uname(uname);
        use_dev_dist(&dev_dist(dir.path()));
        let installer = installer(&remote, &[], offline_dist(dir.path()));
        let key = crate::DeviceKey::generate().unwrap().public();
        let err = installer
            .bootstrap(&key, "mac", &no_progress)
            .await
            .unwrap_err();
        let InstallError::Unsupported(why) = err else {
            panic!("{uname}: {err}");
        };
        assert!(why.contains(says), "{uname}: {why}");
        assert_eq!(remote.calls().len(), 1, "{uname}");
        assert!(remote_files(&remote).is_empty(), "{uname}");
    }
}

/// ③ 캐시의 매니페스트 서명이 앱의 키로 검증되지 않으면(시험 키로 서명 · 한 바이트 바뀜) 받기를 다시 해 보고, 그것도 안 되면 아무것도
/// 올리지 않는다. 캐시의 바이너리가 매니페스트 해시와 같아도 마찬가지다.
#[tokio::test]
async fn a_cached_binary_under_a_bad_manifest_signature_is_not_uploaded() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pw-release/tests/fixtures/relay");
    let good = std::fs::read(fixtures.join(MANIFEST_FILE)).unwrap();
    let mut changed = good.clone();
    let at = changed.iter().position(|b| *b == b'3').unwrap();
    changed[at] = b'4';
    for manifest in [good, changed] {
        let dir = tempfile::tempdir().unwrap();
        let remote = FakeRemote::new(dir.path(), "home", true);
        remote.uname("Linux x86_64");
        let cache = dir.path().join("cache/relay-dist");
        let version_dir = cache.join(VERSION);
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(version_dir.join(MANIFEST_FILE), &manifest).unwrap();
        std::fs::copy(
            fixtures.join("relay-manifest.json.sig"),
            version_dir.join("relay-manifest.json.sig"),
        )
        .unwrap();
        let name = format!("pitwall-relay-{VERSION}-linux-x86_64");
        std::fs::copy(fixtures.join("bin").join(&name), version_dir.join(&name)).unwrap();
        let fetch = Arc::new(Offline::default());
        let mut dist = RelayDist::new(cache, fetch.clone());
        dist.signed_only = true;
        let installer = installer(&remote, &[], dist);
        let key = crate::DeviceKey::generate().unwrap().public();
        let err = installer
            .bootstrap(&key, "mac", &no_progress)
            .await
            .unwrap_err();
        assert!(matches!(err, InstallError::Dist(_)), "{err}");
        assert_eq!(
            *fetch.0.lock().unwrap(),
            vec![release_url(VERSION, MANIFEST_FILE)],
            "it tried the release once for a fresh manifest"
        );
        assert_eq!(remote.calls().len(), 1, "only the probe reached the remote");
        assert!(remote_files(&remote).is_empty());
    }
}

/// 질문을 기록하고 파일의 답을 주는 askpass.
struct ScriptAskpass {
    script: std::path::PathBuf,
}

impl SshAskpass for ScriptAskpass {
    fn session(&self) -> Option<AskpassSession> {
        Some(AskpassSession::new(
            vec![("SSH_ASKPASS".into(), self.script.clone().into())],
            (),
        ))
    }
}

fn askpass(dir: &Path, answer_yes: bool) -> (Arc<dyn SshAskpass>, std::path::PathBuf) {
    std::fs::create_dir_all(dir).unwrap();
    let asked = dir.join("asked");
    let script = dir.join("askpass");
    let answer = if answer_yes {
        "echo yes; exit 0"
    } else {
        "exit 1"
    };
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf '%s\\n%s\\n' \"$SSH_ASKPASS_PROMPT\" \"$1\" >> '{}'\n{answer}\n",
            asked.display()
        ),
    );
    (Arc::new(ScriptAskpass { script }), asked)
}

/// 호스트 키(메인 판정 M7-P17): 처음 보는 호스트는 ssh 가 askpass 의 `confirm` 으로 지문 원문을 묻는다 — 아니오면 아무것도 돌지
/// 않고, 예면 known_hosts 에 남아 다음에는 묻지 않는다. 바뀐 호스트 키는 묻지 않고 ssh 의 거절 원문을 그대로 돌려준다.
#[tokio::test]
async fn host_keys_are_asked_through_askpass_and_a_changed_key_is_refused_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    remote.uname("Linux x86_64");
    use_dev_dist(&dev_dist(dir.path()));

    let (no, asked) = askpass(&dir.path().join(""), false);
    let refused = installer_with(
        &remote,
        &[("FAKE_SSH_HOSTKEY", "unknown")],
        offline_dist(dir.path()),
        Some(no),
    );
    let err = refused.probe().await.unwrap_err();
    assert!(
        matches!(err, InstallError::Ssh(SshError::HostKeyNotAccepted { .. })),
        "{err}"
    );
    let question = std::fs::read_to_string(&asked).unwrap();
    assert!(question.starts_with("confirm\n"), "{question}");
    assert!(question.contains("SHA256:fakefingerprint"), "{question}");
    assert!(
        question.contains("The authenticity of host 'mini"),
        "{question}"
    );
    assert!(remote_files(&remote).is_empty());

    let (yes, _) = askpass(&dir.path().join(""), true);
    let accepted = installer_with(
        &remote,
        &[("FAKE_SSH_HOSTKEY", "unknown")],
        offline_dist(dir.path()),
        Some(yes),
    );
    accepted.probe().await.unwrap();
    std::fs::remove_file(&asked).unwrap();
    accepted.probe().await.unwrap();
    assert!(!asked.exists(), "a known host is not asked again");

    // askpass 가 없으면 묻지 못하고 실패한다 — 묻지 않고 받는 길로 가지 않는다.
    std::fs::remove_file(&remote.known).unwrap();
    let silent = installer(
        &remote,
        &[("FAKE_SSH_HOSTKEY", "unknown")],
        offline_dist(dir.path()),
    );
    assert!(matches!(
        silent.probe().await.unwrap_err(),
        InstallError::Ssh(SshError::HostKeyNotAccepted { .. })
    ));

    let (yes, asked_changed) = askpass(&dir.path().join("x"), true);
    let changed = installer_with(
        &remote,
        &[("FAKE_SSH_HOSTKEY", "changed")],
        offline_dist(dir.path()),
        Some(yes),
    );
    let InstallError::Ssh(SshError::HostKeyChanged { stderr }) = changed.probe().await.unwrap_err()
    else {
        panic!("a changed host key is its own error");
    };
    assert!(
        stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED"),
        "{stderr}"
    );
    assert!(stderr.contains("SHA256:changedfingerprint"), "{stderr}");
    assert!(!asked_changed.exists(), "a changed key is never asked");
}

/// 링크 전송은 그 판의 `proxy` 의 stdio 다 — 쓴 바이트가 그대로 돌아온다(가짜 Relay 의 `proxy` 는 `cat`).
#[tokio::test]
async fn the_transport_is_the_stdio_of_the_installed_proxy() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    remote.uname("Linux x86_64");
    use_dev_dist(&dev_dist(dir.path()));
    let installer = installer(&remote, &[], offline_dist(dir.path()));
    let probe = installer.probe().await.unwrap();
    installer.install(&probe, &no_progress).await.unwrap();
    let mut stream = installer.transport().open().await.unwrap();
    stream.write.write_all(b"hello").await.unwrap();
    // `ChildStdin::shutdown` 은 파이프를 닫지 않는다 — 버려야 원격 `cat` 이 끝난다.
    drop(stream.write);
    let mut echoed = Vec::new();
    stream.read.read_to_end(&mut echoed).await.unwrap();
    assert_eq!(echoed, b"hello");
    assert!(
        installer
            .unpair(&crate::DeviceId::from_stored("SHA256:x"))
            .await
            .unwrap()
    );
}
