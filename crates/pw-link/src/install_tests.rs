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

/// 부를 때마다 `answers` 의 다음 줄을 답하는 askpass(`FAIL` 이면 1 로 끝남 — 사람이 [취소]). 질문마다 `[hint=<SSH_ASKPASS_PROMPT>]`
/// 줄과 질문 글을 `asked` 에 적는다.
fn askpass(dir: &Path, answers: &[&str]) -> (Arc<dyn SshAskpass>, std::path::PathBuf) {
    std::fs::create_dir_all(dir).unwrap();
    let asked = dir.join("asked");
    let script = dir.join("askpass");
    let _ = std::fs::remove_file(dir.join("count"));
    let _ = std::fs::remove_file(&asked);
    std::fs::write(dir.join("answers"), answers.join("\n") + "\n").unwrap();
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nd='{}'\nn=$(cat \"$d/count\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$d/count\"\n\
             printf '[hint=%s]\\n%s\\n' \"$SSH_ASKPASS_PROMPT\" \"$1\" >> \"$d/asked\"\n\
             line=$(sed -n \"${{n}}p\" \"$d/answers\")\n\
             if [ \"$line\" = FAIL ]; then exit 1; fi\nprintf '%s\\n' \"$line\"\n",
            dir.display()
        ),
    );
    (Arc::new(ScriptAskpass { script }), asked)
}

/// 호스트 키(메인 판정 M7-P17): 처음 보는 호스트는 ssh(OpenSSH `confirm()`)가 askpass 를 `SSH_ASKPASS_PROMPT` 힌트 **없이** 지문이
/// 든 문장 원문으로 부른다. 답이 `yes`(또는 지문)여야 받고, 빈 답 · `no` · 취소는 거절이며 아무것도 돌지 않는다. 다른 답이면 다시
/// 묻는다. 받으면 known_hosts 에 남아 다음에는 묻지 않는다. 바뀐 호스트 키는 묻지 않고 ssh 의 거절 원문을 그대로 돌려준다.
#[tokio::test]
async fn host_keys_are_asked_through_askpass_and_a_changed_key_is_refused_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    remote.uname("Linux x86_64");
    use_dev_dist(&dev_dist(dir.path()));
    let ask_dir = dir.path().join("ask");
    let probe_with = |answers: &[&str]| {
        let (askpass, asked) = askpass(&ask_dir, answers);
        let asker = installer_with(
            &remote,
            &[("FAKE_SSH_HOSTKEY", "unknown")],
            offline_dist(dir.path()),
            Some(askpass),
        );
        (asker, asked)
    };

    // 취소 · 빈 답 · `no` 는 거절이다 — 빈 답을 「예」 로 돌리는 다이얼로그는 호스트를 받지 못한다.
    for answers in [&["FAIL"][..], &[""], &["no"]] {
        let (asker, asked) = probe_with(answers);
        let err = asker.probe().await.unwrap_err();
        assert!(
            matches!(err, InstallError::Ssh(SshError::HostKeyNotAccepted { .. })),
            "{answers:?}: {err}"
        );
        let question = std::fs::read_to_string(&asked).unwrap();
        assert!(
            question.starts_with("[hint=]\n"),
            "no askpass hint: {question}"
        );
        assert!(
            question.contains("The authenticity of host 'mini"),
            "{question}"
        );
        assert!(question.contains("SHA256:fakefingerprint"), "{question}");
        assert!(!remote.known.exists(), "{answers:?}");
        assert!(remote_files(&remote).is_empty());
    }

    // 다른 답이면 다시 묻고, `yes` 를 돌려야 받는다.
    let (asker, asked) = probe_with(&["ok", "yes"]);
    asker.probe().await.unwrap();
    let question = std::fs::read_to_string(&asked).unwrap();
    assert_eq!(question.matches("[hint=]").count(), 2, "{question}");
    assert!(
        question.contains("Please type 'yes', 'no' or the fingerprint"),
        "{question}"
    );
    std::fs::remove_file(&asked).unwrap();
    asker.probe().await.unwrap();
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

    let (yes, asked_changed) = askpass(&dir.path().join("x"), &["yes"]);
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

/// ⑫ 매니페스트를 쓴 뒤 바뀐 바이너리(같은 크기 · 다른 바이트)는 올리지 않는다 — 올릴 바이트 자체를 매니페스트 해시와 견준다.
#[tokio::test]
async fn a_binary_that_differs_from_its_manifest_is_not_uploaded() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    remote.uname("Linux x86_64");
    let dist = dev_dist(dir.path());
    use_dev_dist(&dist);
    let binary = dist.join(format!("pitwall-relay-{VERSION}-linux-x86_64"));
    let mut bytes = std::fs::read(&binary).unwrap();
    let last = bytes.len() - 2;
    bytes[last] ^= 0x20;
    std::fs::write(&binary, bytes).unwrap();
    let installer = installer(&remote, &[], offline_dist(dir.path()));
    let probe = installer.probe().await.unwrap();
    let err = installer.install(&probe, &no_progress).await.unwrap_err();
    assert!(matches!(err, InstallError::Dist(_)), "{err}");
    assert_eq!(
        remote.calls().len(),
        1,
        "nothing but the probe reached the remote"
    );
    assert!(remote_files(&remote).is_empty());
}

/// 이 판 이름의 바이너리가 이미 있어도 해시가 매니페스트와 다르면(깨짐 · 반쯤 올라감) 다시 올린다(M7-06 리뷰 F2).
#[tokio::test]
async fn an_installed_binary_with_another_hash_is_uploaded_again() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    remote.uname("Linux x86_64");
    let dist = dev_dist(dir.path());
    use_dev_dist(&dist);
    std::fs::create_dir_all(remote.relay_dir()).unwrap();
    let installed = remote.relay_dir().join(installed_name());
    std::fs::write(&installed, b"half an upload").unwrap();
    let installer = installer(&remote, &[], offline_dist(dir.path()));
    let probe = installer.probe().await.unwrap();
    assert!(matches!(probe.installed, Installed::Sha256(_)));
    assert!(
        installer.install(&probe, &no_progress).await.unwrap(),
        "uploaded again"
    );
    assert_eq!(
        std::fs::read(&installed).unwrap(),
        std::fs::read(dist.join(format!("pitwall-relay-{VERSION}-linux-x86_64"))).unwrap()
    );
}

/// 원격 자리는 Relay 의 `AppPaths` 와 같은 순서로 푼다: `PITWALL_DATA_DIR` → `$XDG_DATA_HOME/pitwall` → `$HOME/.local/share/pitwall`
/// (M7-06 리뷰 R-b).
#[tokio::test]
async fn the_remote_data_folder_follows_pitwall_data_dir_then_xdg() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    remote.uname("Linux x86_64");
    use_dev_dist(&dev_dist(dir.path()));
    let xdg = dir.path().join("xdg");
    let own = dir.path().join("own");
    let xdg_s = xdg.to_str().unwrap();
    let own_s = own.to_str().unwrap();
    let probe = |extra: &[(&str, &str)]| {
        let installer = installer(&remote, extra, offline_dist(dir.path()));
        async move {
            let probe = installer.probe().await.unwrap();
            installer.install(&probe, &no_progress).await.unwrap();
        }
    };
    probe(&[("FAKE_REMOTE_XDG_DATA_HOME", xdg_s)]).await;
    assert!(xdg.join("pitwall/relay").join(installed_name()).exists());
    probe(&[
        ("FAKE_REMOTE_XDG_DATA_HOME", xdg_s),
        ("FAKE_REMOTE_PITWALL_DATA_DIR", own_s),
    ])
    .await;
    assert!(own.join("relay").join(installed_name()).exists());
    assert!(
        remote_files(&remote).is_empty(),
        "the home default is not used"
    );
}

/// 판 · 채널에 `/` 가 있으면 원격 파일 이름이 폴더를 넘으므로 받지 않는다(M7-06 리뷰 R-d).
#[test]
fn a_version_or_channel_with_a_slash_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    for (version, channel) in [("1.2.3/../x", "dev"), ("1.2.3", "a/b")] {
        let ssh =
            Ssh::with_program(remote.ssh.clone(), SshTarget::alias("mini"), Vec::new()).unwrap();
        assert!(
            Installer::new(
                Arc::new(ssh),
                Versions::new(version, channel),
                offline_dist(dir.path())
            )
            .is_err(),
            "{version} {channel}"
        );
    }
}

/// 가벼운 상태 확인(M7-08 ③): `ssh -O check` 는 원격 명령을 돌리지 않고, 마스터가 떠 있을 때만 참이다.
#[tokio::test]
async fn the_master_check_runs_no_remote_command() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    let ssh = |extra: &[(&str, &str)]| {
        Ssh::with_program(remote.ssh.clone(), SshTarget::alias("mini"), Vec::new())
            .unwrap()
            .env(remote.env(extra))
    };
    assert!(
        ssh(&[("FAKE_SSH_MASTER", "running")])
            .check_master()
            .await
            .unwrap()
    );
    assert!(!ssh(&[]).check_master().await.unwrap());
    let calls = remote.calls();
    assert_eq!(calls.len(), 2);
    for call in calls {
        assert_eq!(call, ["-O", "check", "--", "mini"]);
    }
}

/// M7-07 이 넘긴 `Ssh::run` 의 일반 시간 상한: 넘으면 ssh 를 끝내고 `TimedOut`. 창에 질문이 떠 사람의 답을 기다리는 동안은
/// 세지 않는다(askpass 가 답하는 데 상한보다 오래 걸려도 끝까지 간다).
#[tokio::test]
async fn ssh_runs_have_a_time_limit_that_skips_the_time_spent_asking() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    let ssh = Ssh::with_program(remote.ssh.clone(), SshTarget::alias("mini"), Vec::new())
        .unwrap()
        .env(remote.env(&[]))
        .timeout(std::time::Duration::from_millis(500));
    let started = std::time::Instant::now();
    let err = ssh.run("sleep 30", &[], None).await.unwrap_err();
    assert!(matches!(err, SshError::TimedOut { .. }), "{err:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));

    // 호스트 키 질문에 2초 걸려 답한다 — 그동안 「묻는 중」 이라 0.5초 상한을 세지 않는다.
    let answer = dir.path().join("slow-answer.sh");
    write_executable(&answer, "#!/bin/sh\nsleep 2\necho yes\n");
    struct Slow(std::path::PathBuf, std::time::Instant);
    impl SshAskpass for Slow {
        fn session(&self) -> Option<AskpassSession> {
            let since = self.1;
            Some(
                AskpassSession::new(vec![("SSH_ASKPASS".into(), self.0.clone().into())], ())
                    .with_asking(Arc::new(move || {
                        since.elapsed() < std::time::Duration::from_secs(3)
                    })),
            )
        }
    }
    let ssh = Ssh::with_program(remote.ssh.clone(), SshTarget::alias("mini"), Vec::new())
        .unwrap()
        .env(remote.env(&[("FAKE_SSH_HOSTKEY", "unknown")]))
        .askpass(Arc::new(Slow(answer, std::time::Instant::now())))
        .timeout(std::time::Duration::from_millis(500));
    let done = ssh.run("true", &[], None).await.unwrap();
    assert_eq!(done.code, Some(0));
}
