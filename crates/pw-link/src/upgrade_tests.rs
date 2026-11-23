//! 판 바꾸기 · 정리 시험(18-m7-plan M7-07 시험 ② ③ · [옛 판 끝내기]) — 가짜 `ssh`(`crate::fake_ssh`)가 원격 명령을 이 기기의 `sh`
//! 로 돌린다. 원격의 「Relay 바이너리」 는 `status` 에만 답하는 셸 스크립트다: 표지 파일이 있으면 그 JSON 한 줄(떠 있는 데몬), 없으면
//! 3(데몬 없음). 「데몬」 의 pid 는 이 시험이 띄운 `sleep` 이다. 실제 Relay 의 판 바꾸기는 `apps/relay/tests/upgrade.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::fake_ssh::{FakeRemote, write_executable};
use crate::handshake::Versions;
use crate::install::{Fetch, RelayDist};
use crate::ssh::{Ssh, SshTarget};

const CURRENT: &str = "3.0.0";

struct Offline;

impl Fetch for Offline {
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
        Versions::new(CURRENT, "dev"),
        RelayDist::new(dir.join("cache"), Arc::new(Offline)),
    )
    .unwrap()
}

/// 원격의 가짜 Relay 하나(`pitwall-relay-dev-<판>`). 표지 파일(`<시험 폴더>/marker-<판>`)에 status JSON 을 쓰면 떠 있는 데몬이다.
struct Stub {
    marker: PathBuf,
    version: String,
}

impl Stub {
    fn new(dir: &Path, remote: &FakeRemote, version: &str) -> Self {
        let marker = dir.join(format!("marker-{version}"));
        std::fs::create_dir_all(remote.relay_dir()).unwrap();
        write_executable(
            &remote
                .relay_dir()
                .join(format!("pitwall-relay-dev-{version}")),
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  status) if [ -f '{m}' ]; then cat '{m}'; exit 0; else exit 3; fi ;;\n  *) exit 2 ;;\nesac\n",
                m = marker.display()
            ),
        );
        Self {
            marker,
            version: version.to_owned(),
        }
    }

    fn run(&self, pid: u32, runs: usize, terminals: usize) {
        std::fs::write(
            &self.marker,
            format!(
                "{{\"version\":\"{}\",\"releaseChannel\":\"dev\",\"name\":null,\"pid\":{pid},\"uptimeMs\":5,\"links\":0,\"ownerLinks\":0,\"runs\":{runs},\"terminals\":{terminals}}}\n",
                self.version
            ),
        )
        .unwrap();
    }

    fn stop(&self) {
        let _ = std::fs::remove_file(&self.marker);
    }
}

/// 「데몬」 프로세스(`sleep`). 끝나면 거둔다 — 좀비가 `kill -0` 에 살아 있게 보이지 않게.
struct Sleeper {
    pid: u32,
}

impl Sleeper {
    fn start() -> Self {
        let mut child = std::process::Command::new("sleep")
            .arg("300")
            .spawn()
            .unwrap();
        let pid = child.id();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Self { pid }
    }

    fn alive(&self) -> bool {
        signal("-0", self.pid)
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        signal("-KILL", self.pid);
    }
}

fn signal(signal: &str, pid: u32) -> bool {
    std::process::Command::new("kill")
        .args([signal, &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn files(remote: &FakeRemote) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(remote.relay_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Relay 가 같은 폴더에 두는 것(M7-04 리뷰 권고 9) — 정리가 닿으면 안 된다.
const RELAY_STATE: [&str; 4] = ["device.key", "pairings.json", "pairings.lock", "state.db"];

fn relay_state(remote: &FakeRemote) {
    for name in RELAY_STATE {
        std::fs::write(remote.relay_dir().join(name), name).unwrap();
    }
}

/// ③ 세 판이 있으면 가장 오래된 하나만 지운다. 셸이 끝난 올리기 임시 파일은 지우고 올리는 중인 것은 남긴다. 같은 폴더의
/// `state.db` · `device.key` · `pairings.json` · `pairings.lock` 은 그대로다.
#[tokio::test]
async fn three_versions_lose_only_the_oldest() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    for version in [CURRENT, "2.0.0", "1.0.0"] {
        Stub::new(dir.path(), &remote, version);
    }
    relay_state(&remote);
    let dead_upload = format!("pitwall-relay-dev-{CURRENT}.part.1");
    let live_upload = format!("pitwall-relay-dev-{CURRENT}.part.{}", std::process::id());
    for upload in [&dead_upload, &live_upload] {
        std::fs::write(remote.relay_dir().join(upload), b"half").unwrap();
    }
    let installer = installer(dir.path(), &remote);

    let survey = installer.survey().await.unwrap();
    assert_eq!(survey.binaries.len(), 5, "{survey:?}");
    assert!(survey.old_daemons().is_empty());
    let cleaned = installer.clean(&survey).await.unwrap();
    assert_eq!(
        cleaned.removed,
        ["pitwall-relay-dev-1.0.0".to_owned(), dead_upload.clone()]
    );
    assert!(
        cleaned.uploading.is_empty(),
        "a live upload is not even asked for"
    );
    let mut left = vec![
        "pitwall-relay-dev-2.0.0".to_owned(),
        format!("pitwall-relay-dev-{CURRENT}"),
        live_upload,
    ];
    left.extend(RELAY_STATE.map(String::from));
    left.sort();
    assert_eq!(files(&remote), left);

    // 두 판만 남았으면 다시 돌려도 ssh 를 부르지 않는다.
    let calls = remote.calls().len();
    let again = installer.survey().await.unwrap();
    assert_eq!(installer.clean(&again).await.unwrap(), Cleaned::default());
    assert_eq!(remote.calls().len(), calls + 1, "only the survey ran");
}

/// ② 떠 있는 데몬의 바이너리는 정리에서 남는다 — 가장 오래된 판이어도. 그 데몬이 끝난 뒤의 정리에서 지운다.
#[tokio::test]
async fn a_running_daemons_binary_stays() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    Stub::new(dir.path(), &remote, "2.0.0");
    let old = Stub::new(dir.path(), &remote, "1.0.0");
    let daemon = Sleeper::start();
    old.run(daemon.pid, 1, 0);
    let installer = installer(dir.path(), &remote);

    let survey = installer.survey().await.unwrap();
    assert_eq!(survey.to_remove(), ["pitwall-relay-dev-1.0.0"]);
    let cleaned = installer.clean(&survey).await.unwrap();
    assert_eq!(cleaned.running, ["pitwall-relay-dev-1.0.0"]);
    assert!(cleaned.removed.is_empty(), "{cleaned:?}");
    assert!(
        remote.relay_dir().join("pitwall-relay-dev-1.0.0").exists(),
        "the running daemon's binary was removed"
    );

    // 데몬이 「조사 뒤 · 정리 전」 에 뜬 경우도 원격 셸이 지우기 직전에 본다(조사는 멈춤이었다).
    old.stop();
    let stale = installer.survey().await.unwrap();
    old.run(daemon.pid, 1, 0);
    let cleaned = installer.clean(&stale).await.unwrap();
    assert_eq!(cleaned.running, ["pitwall-relay-dev-1.0.0"]);
    assert!(remote.relay_dir().join("pitwall-relay-dev-1.0.0").exists());

    old.stop();
    let cleaned = installer
        .clean(&installer.survey().await.unwrap())
        .await
        .unwrap();
    assert_eq!(cleaned.removed, ["pitwall-relay-dev-1.0.0"]);
}

/// `status` 가 데몬 여부를 답하지 못하는 바이너리(종료 3 · 126 이 아님)는 지우지 않고 알린다.
#[tokio::test]
async fn a_binary_that_cannot_tell_stays() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    Stub::new(dir.path(), &remote, "2.1.0");
    // 1 그 밖의 오류, 4 데몬이 답하지 않음(증명 전 연결 상한 · 바쁨 · 시간 초과), 126 실행 못 함 — 모두 「없음」 이 확실하지 않다.
    for (version, code) in [("1.0.0", 1), ("1.1.0", 4), ("1.2.0", 126)] {
        write_executable(
            &remote
                .relay_dir()
                .join(format!("pitwall-relay-dev-{version}")),
            &format!("#!/bin/sh\nexit {code}\n"),
        );
    }
    let installer = installer(dir.path(), &remote);
    let survey = installer.survey().await.unwrap();
    assert_eq!(survey.unknown().len(), 3, "{survey:?}");
    assert!(survey.old_daemons().is_empty());
    let mut cleaned = installer.clean(&survey).await.unwrap();
    cleaned.unknown.sort();
    assert_eq!(
        cleaned.unknown,
        [
            ("pitwall-relay-dev-1.0.0".to_owned(), Some(1)),
            ("pitwall-relay-dev-1.1.0".to_owned(), Some(4)),
            ("pitwall-relay-dev-1.2.0".to_owned(), Some(126)),
        ]
    );
    assert!(cleaned.removed.is_empty(), "{cleaned:?}");
    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        assert!(
            remote
                .relay_dir()
                .join(format!("pitwall-relay-dev-{version}"))
                .exists()
        );
    }
}

/// 답하지 않는 옛 판 데몬은 「없음」 이 아니다 — 끝내기는 `NoAnswer` 로 아무것도 보내지 않는다(F1).
#[tokio::test]
async fn a_daemon_that_does_not_answer_is_not_gone() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    let old = Stub::new(dir.path(), &remote, "2.0.0");
    let daemon = Sleeper::start();
    old.run(daemon.pid, 1, 0);
    let installer = installer(dir.path(), &remote);
    let seen = installer.survey().await.unwrap().old_daemons()[0].clone();
    // 이제 그 데몬이 답하지 않는다(4).
    write_executable(
        &remote.relay_dir().join("pitwall-relay-dev-2.0.0"),
        "#!/bin/sh\nexit 4\n",
    );
    assert_eq!(
        installer.stop_old(&seen).await.unwrap(),
        StopOutcome::NoAnswer
    );
    assert!(daemon.alive());
    // 조사와 끝내기 사이에 답이 끊긴 경우도 원격 셸이 「gone」 으로 읽지 않는다.
    let pid = daemon.pid.to_string();
    let output = installer
        .run("stop", STOP, &["pitwall-relay-dev-2.0.0", &pid], None)
        .await
        .unwrap();
    assert_eq!(
        marked(&output.stdout, "pitwall-stop ").as_deref(),
        Some("unknown")
    );
    assert!(daemon.alive());
}

/// 원격이 답하지 않으면(옛 판 `status` 가 멈춤) 조사는 시간 상한에서 끝난다.
#[tokio::test]
async fn a_hanging_survey_ends_at_its_time_limit() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    write_executable(
        &remote.relay_dir().join("pitwall-relay-dev-2.0.0"),
        "#!/bin/sh\nexec sleep 30\n",
    );
    TEST_LIMIT_MS.store(1500, std::sync::atomic::Ordering::Relaxed);
    let installer = installer(dir.path(), &remote);
    let started = std::time::Instant::now();
    let err = installer.survey().await.unwrap_err();
    assert!(
        matches!(
            err,
            InstallError::Remote {
                step: "survey",
                code: None,
                ..
            }
        ),
        "{err}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

/// 사람이 둔 `pitwall-relay-*` 파일 가운데 판 꼴이 아닌 것은 조사가 실행하지 않는다(R3).
#[tokio::test]
async fn the_survey_runs_only_relay_version_files() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    let ran = dir.path().join("ran");
    write_executable(
        &remote.relay_dir().join("pitwall-relay-backup.sh"),
        &format!("#!/bin/sh\necho x > '{}'\nexit 3\n", ran.display()),
    );
    let installer = installer(dir.path(), &remote);
    let survey = installer.survey().await.unwrap();
    assert!(
        !ran.exists(),
        "the survey ran a file that is not a relay version"
    );
    assert_eq!(survey.binaries.len(), 1, "{survey:?}");
}

/// 정리 스크립트는 `pitwall-relay-*` 이름만 지운다 — Desktop 이 다른 이름을 넘겨도(`state.db` · `/` 가 든 이름) 거절한다.
#[tokio::test]
async fn the_clean_script_refuses_anything_but_relay_binaries() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    relay_state(&remote);
    std::fs::write(remote.relay_dir().join("pitwall-relay-x"), b"").unwrap();
    let installer = installer(dir.path(), &remote);
    let names = [
        "state.db",
        "device.key",
        "pairings.json",
        "pitwall-relay-x/../state.db",
        "pitwall-relay-dev-3.0.0",
    ];
    let output = installer
        .run("clean-up", CLEAN, &names, None)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    for name in &names[..4] {
        assert!(
            text.contains(&format!("pitwall-clean refused {name}")),
            "{text}"
        );
    }
    assert!(text.contains("pitwall-clean current pitwall-relay-dev-3.0.0"));
    for name in RELAY_STATE {
        assert!(remote.relay_dir().join(name).exists(), "{name} was removed");
    }
    assert!(remote.relay_dir().join("pitwall-relay-dev-3.0.0").exists());
}

/// [옛 판 끝내기]: 조사가 옛 판 데몬과 그 실행 · 터미널 수를 낸다. 사람이 본 수와 지금 수가 다르면 보내지 않고 지금 것을 돌려준다.
/// 같으면 SIGTERM 으로 끝낸다. 이 판에는 보내지 않는다.
#[tokio::test]
async fn an_old_daemon_is_stopped_only_as_the_person_saw_it() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    let current = Stub::new(dir.path(), &remote, CURRENT);
    let old = Stub::new(dir.path(), &remote, "2.0.0");
    let daemon = Sleeper::start();
    old.run(daemon.pid, 1, 2);
    let installer = installer(dir.path(), &remote);

    let survey = installer.survey().await.unwrap();
    let found = survey.old_daemons();
    assert_eq!(found.len(), 1, "{survey:?}");
    let seen = found[0].clone();
    assert_eq!(seen.file, "pitwall-relay-dev-2.0.0");
    assert_eq!((seen.status.runs, seen.status.terminals), (1, 2));
    assert_eq!(seen.left(), 3);

    // 사람이 본 뒤 다른 Desktop 이 옛 판에 붙었다(링크 — R2).
    std::fs::write(
        &old.marker,
        std::fs::read_to_string(&old.marker)
            .unwrap()
            .replace("\"links\":0", "\"links\":1"),
    )
    .unwrap();
    match installer.stop_old(&seen).await.unwrap() {
        StopOutcome::Changed(now) => assert_eq!(now.status.links, 1),
        other => panic!("{other:?}"),
    }
    assert!(daemon.alive(), "cut a desktop the person did not see");
    let seen = installer.survey().await.unwrap().old_daemons()[0].clone();

    // 사람이 본 뒤 터미널이 하나 더 열렸다.
    old.run(daemon.pid, 1, 3);
    match installer.stop_old(&seen).await.unwrap() {
        StopOutcome::Changed(now) => assert_eq!(now.left(), 4),
        other => panic!("{other:?}"),
    }
    assert!(daemon.alive(), "stopped a daemon the person did not see");

    let seen = installer.survey().await.unwrap().old_daemons()[0].clone();
    assert_eq!(
        installer.stop_old(&seen).await.unwrap(),
        StopOutcome::Stopped
    );
    assert!(!daemon.alive());
    old.stop();
    assert_eq!(installer.stop_old(&seen).await.unwrap(), StopOutcome::Gone);

    // 이 판의 데몬은 옛 판이 아니다 — 조사에 나오지 않고, 끝내기 스크립트도 거절한다.
    let mine = Sleeper::start();
    current.run(mine.pid, 0, 0);
    assert!(installer.survey().await.unwrap().old_daemons().is_empty());
    let pid = mine.pid.to_string();
    let output = installer
        .run(
            "stop",
            STOP,
            &[&format!("pitwall-relay-dev-{CURRENT}"), &pid],
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        marked(&output.stdout, "pitwall-stop ").as_deref(),
        Some("current")
    );
    assert!(mine.alive());
}

/// 끝내기 스크립트는 그 바이너리의 `status` 가 지금 내는 pid 에만 신호를 보낸다 — 넘겨받은 pid 가 다르면(pid 재사용 · 다른
/// 데몬) 보내지 않는다.
#[tokio::test]
async fn the_stop_script_signals_only_the_pid_the_daemon_reports() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    let old = Stub::new(dir.path(), &remote, "2.0.0");
    let daemon = Sleeper::start();
    let bystander = Sleeper::start();
    old.run(daemon.pid, 0, 0);
    let installer = installer(dir.path(), &remote);
    let pid = bystander.pid.to_string();
    let output = installer
        .run("stop", STOP, &["pitwall-relay-dev-2.0.0", &pid], None)
        .await
        .unwrap();
    assert_eq!(
        marked(&output.stdout, "pitwall-stop ").as_deref(),
        Some("changed")
    );
    assert!(
        bystander.alive(),
        "signalled a pid the daemon did not report"
    );
    assert!(daemon.alive());
}

/// 끝내기 스크립트도 `/` 가 든 이름 · 올리기 임시 파일을 거절한다 — Desktop 의 이름 거르기를 지나 오지 않은 경우(R1).
#[tokio::test]
async fn the_stop_script_refuses_paths_and_uploads() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    let daemon = Sleeper::start();
    let pid = daemon.pid.to_string();
    // `.part` 도 이름 그대로면 `status` 에 pid 를 내는 실행 파일이다 — 거절하지 않으면 신호가 간다.
    let fake = format!("#!/bin/sh\necho '{{\"pid\":{pid},\"runs\":0,\"terminals\":0}}'\n");
    write_executable(
        &remote.relay_dir().join("pitwall-relay-dev-2.0.0.part.1"),
        &fake,
    );
    std::fs::create_dir_all(remote.relay_dir().join("pitwall-relay-x")).unwrap();
    write_executable(&remote.relay_dir().join("pitwall-relay-y"), &fake);
    let installer = installer(dir.path(), &remote);
    for name in [
        "pitwall-relay-dev-2.0.0.part.1",
        "pitwall-relay-x/../pitwall-relay-y",
    ] {
        let refused = installer.run("stop", STOP, &[name, &pid], None).await;
        assert!(
            matches!(refused, Err(InstallError::Remote { code: Some(64), .. })),
            "{name}: {refused:?}"
        );
        assert!(daemon.alive(), "{name}");
    }
}

/// `stop_old` 는 Relay 바이너리 이름이 아니면 ssh 를 부르지 않는다(R1).
#[tokio::test]
async fn stop_old_refuses_a_name_that_is_not_a_relay_binary() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    let installer = installer(dir.path(), &remote);
    let seen = OldDaemon {
        file: "pitwall-relay-dev-1.0.0/../x".into(),
        status: serde_json::from_str(r#"{"pid":7}"#).unwrap(),
    };
    assert!(matches!(
        installer.stop_old(&seen).await,
        Err(InstallError::Protocol(_))
    ));
    assert!(remote.calls().is_empty(), "ssh ran");
}

/// 정리 스크립트는 그 셸이 살아 있는 올리기 임시 파일을 남긴다 — 조사 뒤 · 정리 전에 올리기가 시작된 경우(R1).
#[tokio::test]
async fn the_clean_script_keeps_an_upload_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    Stub::new(dir.path(), &remote, CURRENT);
    let live = format!("pitwall-relay-dev-{CURRENT}.part.{}", std::process::id());
    std::fs::write(remote.relay_dir().join(&live), b"half").unwrap();
    let installer = installer(dir.path(), &remote);
    let output = installer
        .run("clean-up", CLEAN, &[&live], None)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("pitwall-clean uploading {live}"))
    );
    assert!(remote.relay_dir().join(&live).exists());
}
