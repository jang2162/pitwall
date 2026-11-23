//! 실제 ssh 길의 시험 — 가짜 `ssh`(`pw_link::fake_ssh` — **실제 SSH 는 쓰지 않는다**, OpenSSH 의 호스트 키 · 암호 묻기를 따른다)가
//! 원격 명령을 이 기기의 `sh` 로 돌린다. 원격의 「Relay」 는 셸 스크립트라 `proxy` 는 받은 바이트를 되돌릴 뿐이다 — 핸드셰이크까지
//! 가서 실패한다(실제 Relay 와의 핸드셰이크 · 다시 붙기는 `apps/relay/tests/bootstrap.rs` · CI 의 `relay-live`).

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use pw_link::fake_ssh::{FakeRemote, write_executable};
use pw_link::install::Fetch;
use pw_link::{DeviceKey, DeviceKeyStore};
use pw_release::relay::{
    MANIFEST_FILE, MAX_BINARY_BYTES, PLATFORMS, assemble, asset_name, describe,
};
use semver::Version;
use serde_json::json;

use super::*;
use crate::{RemoteHosts, RemoteLogEntry, RemoteStatus};

const VERSION: &str = "1.2.3";

const FAKE_RELAY: &str = r#"#!/bin/sh
case "$1" in
  --verify) exit 1 ;;
  pair) echo '{"machine":"SHA256:fakemachine","added":true}' ;;
  unpair) echo '{"removed":true}' ;;
  proxy) exec cat ;;
  *) exit 2 ;;
esac
"#;

struct Fixed;

impl DeviceKeyStore for Fixed {
    fn load_or_create(&self) -> Result<DeviceKey, pw_link::device::KeyStoreError> {
        Ok(DeviceKey::from_seed(&[7; 32]))
    }
}

struct Offline;

impl Fetch for Offline {
    fn get<'a>(&'a self, _url: &'a str, _max: u64) -> BoxFuture<'a, io::Result<Vec<u8>>> {
        Box::pin(async { Err(io::Error::other("offline")) })
    }
}

/// askpass 대신: 정해 둔 답을 찍는 스크립트(창이 그 답을 넣은 것과 같다).
struct Answers(std::path::PathBuf);

impl SshAskpass for Answers {
    fn session(&self) -> Option<AskpassSession> {
        Some(AskpassSession::new(
            vec![("SSH_ASKPASS".into(), self.0.clone().into())],
            (),
        ))
    }
}

/// 「원격」 에 올릴 Relay 스크립트를 `script` 로.
fn dev_dist_with(dir: &Path, script: &str) {
    let dist = dir.join("relay-dist-dev");
    std::fs::create_dir_all(&dist).unwrap();
    let version = Version::parse(VERSION).unwrap();
    let platform = PLATFORMS
        .iter()
        .find(|p| p.asset == "linux-x86_64")
        .unwrap();
    let name = asset_name(&version, platform);
    let path = dist.join(&name);
    write_executable(&path, script);
    let entry = describe(&path, &name, MAX_BINARY_BYTES).unwrap();
    let manifest = assemble(&version, &[entry], false).unwrap();
    std::fs::write(dist.join(MANIFEST_FILE), manifest.to_bytes()).unwrap();
    // SAFETY: nextest 는 시험마다 프로세스를 따로 띄운다 — 이 변수를 읽는 다른 스레드가 없다.
    unsafe { std::env::set_var(pw_release::relay::DEV_DIST_ENV, &dist) };
}

struct Setup {
    dir: tempfile::TempDir,
    remote: FakeRemote,
}

fn setup() -> Setup {
    setup_with(FAKE_RELAY)
}

fn setup_with(script: &str) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    remote.uname("Linux x86_64");
    dev_dist_with(dir.path(), script);
    Setup { dir, remote }
}

impl Setup {
    fn backend(&self, extra: &[(&str, &str)], askpass: Option<Arc<dyn SshAskpass>>) -> SshBackend {
        SshBackend::new(SshParts {
            keys: Arc::new(Fixed),
            dist: self.dir.path().join("cache/relay-dist"),
            fetch: Arc::new(Offline),
            askpass,
            device_name: "mac".into(),
            home: Some(self.dir.path().join("local-home")),
        })
        .with_program(
            self.remote.ssh.clone(),
            self.remote.env(extra),
            Versions::new(VERSION, "dev"),
            Duration::from_millis(10),
        )
    }

    fn answers(&self, answer: &str) -> Arc<dyn SshAskpass> {
        let path = self.dir.path().join("answer.sh");
        write_executable(&path, &format!("#!/bin/sh\nprintf '%s\\n' '{answer}'\n"));
        Arc::new(Answers(path))
    }
}

fn mini() -> HostEntry {
    HostEntry {
        name: "mini".into(),
        host: "mini".into(),
        user: None,
        port: None,
        source: RemoteHostSource::SshConfig,
        ssh_args: Vec::new(),
        direct: None,
        ssh: true,
    }
}

async fn connect(
    backend: &SshBackend,
    expect: Option<&str>,
) -> (Result<Link, ConnectError>, Vec<Progress>) {
    let seen = Mutex::new(Vec::new());
    let progress = |p: Progress| seen.lock().unwrap().push(p);
    let entry = mini();
    let result = backend.connect(&entry, expect, &progress).await;
    (result, seen.into_inner().unwrap())
}

fn stages(seen: &[Progress]) -> Vec<RemoteStage> {
    let mut out: Vec<RemoteStage> = Vec::new();
    for p in seen {
        if let Progress::Stage(s) = p
            && out.last() != Some(s)
        {
            out.push(*s);
        }
    }
    out
}

// 진단 칸: SSH → 설치 → 데몬 → 핸드셰이크 단계가 차례로 오고, 핸드셰이크를 하지 않는 「Relay」 는 그 단계의 실패다.
#[tokio::test]
async fn the_stages_come_in_order_and_a_relay_without_a_handshake_fails_there() {
    let s = setup();
    let backend = s.backend(&[], None);
    let (result, seen) = connect(&backend, None).await;
    let e = result.err().expect("the echoing relay must not link");
    assert_eq!(
        stages(&seen),
        [
            RemoteStage::Ssh,
            RemoteStage::Install,
            RemoteStage::Daemon,
            RemoteStage::Handshake
        ]
    );
    // 되돌린 자기 hello 를 받아 다른 키로 보인다 — 어느 까닭이든 핸드셰이크 단계의 실패다.
    assert_eq!(e.stage, RemoteStage::Handshake, "{e:?}");
    let notes: Vec<_> = seen
        .iter()
        .filter_map(|p| match p {
            Progress::Note(n) => Some(n.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        notes.iter().any(|n| n.contains("linux x86_64")),
        "{notes:?}"
    );
    assert!(notes.iter().any(|n| n.contains("uploading")), "{notes:?}");
    // 같은 호스트에 다시 붙으면 설치는 「이미 있음」 이다.
    let (_, again) = connect(&backend, None).await;
    let notes: Vec<_> = again
        .iter()
        .filter_map(|p| match p {
            Progress::Note(n) => Some(n.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        notes.iter().any(|n| n.contains("already installed")),
        "{notes:?}"
    );
}

// 머신 id(M7-P6 · 델타 리뷰 F-a): 페어링 답(원격 stdout)이 기억한 id 와 달라도 그것만으로 「머신이 바뀜」 이 되지 않는다 — 원격이
// 고른 값이 「새로 믿을」 id 가 되지 않게. 핸드셰이크가 기억한 id 를 기대하고 서명으로 가른다. 되돌린 자기 hello 는 서명이 없어
// 거절이다(서명이 맞는 다른 키의 `MachineMismatch` 는 `pw-link` 의 시험 — `found` 는 증명된 지문).
#[tokio::test]
async fn a_pairing_answer_unlike_the_remembered_id_is_left_to_the_handshake() {
    let s = setup();
    let backend = s.backend(&[], None);
    let (result, seen) = connect(&backend, Some("SHA256:remembered")).await;
    let e = result.err().unwrap();
    assert_eq!(e.machine_changed, None, "{e:?}");
    assert_eq!(
        (e.stage, e.kind),
        (RemoteStage::Handshake, RemoteFailureKind::Rejected)
    );
    assert!(stages(&seen).contains(&RemoteStage::Handshake), "{seen:?}");
    assert!(
        seen.iter()
            .any(|p| matches!(p, Progress::Note(n) if n.contains("the handshake decides"))),
        "{seen:?}"
    );
}

// 호스트 키가 바뀌었으면 ssh 가 거절한 원문을 진단에 그대로 — 덮는 길이 없다. 처음 보는 키는 askpass 로 묻고 `yes` 여야 받는다.
#[tokio::test]
async fn a_changed_host_key_is_shown_verbatim_and_an_unknown_one_is_asked() {
    let s = setup();
    let (result, _) = connect(&s.backend(&[("FAKE_SSH_HOSTKEY", "changed")], None), None).await;
    let e = result.err().unwrap();
    assert_eq!(
        (e.stage, e.kind),
        (RemoteStage::Ssh, RemoteFailureKind::HostKeyChanged)
    );
    assert!(
        e.detail
            .unwrap()
            .contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
    );

    let unknown = [("FAKE_SSH_HOSTKEY", "unknown")];
    let (result, _) = connect(&s.backend(&unknown, Some(s.answers(""))), None).await;
    assert_eq!(
        result.err().unwrap().kind,
        RemoteFailureKind::HostKeyNotAccepted
    );
    let (result, seen) = connect(&s.backend(&unknown, Some(s.answers("yes"))), None).await;
    // 키를 받고 설치까지 갔다(핸드셰이크에서 멈추는 것은 이 가짜 Relay 의 탓).
    assert!(
        stages(&seen).contains(&RemoteStage::Handshake),
        "{:?}",
        result.err()
    );
}

// ③ 연결 로그에 askpass 답이 없다 — 실제 ssh 길에서 진단 글(stderr)에 답이 섞여 와도. 창이 넣은 답은 `remote.prompt.answer` 가
// 가릴 것으로 남긴다(여기서는 그 자리를 바로 부른다 — RPC 길은 `crate::tests`).
#[tokio::test]
async fn an_askpass_answer_in_the_ssh_stderr_stays_out_of_the_connection_log() {
    let s = setup();
    let answer = "wrong-secret-77";
    let backend = s.backend(
        &[
            ("FAKE_SSH_PASSWORD", "right-secret"),
            ("FAKE_SSH_ECHO_ANSWER", "1"),
        ],
        Some(s.answers(answer)),
    );
    let remote = RemoteHosts::default();
    let bus = pw_core::EventBus::new(16);
    remote.attach_host(None, bus, None);
    let settings = json!([{ "host": "mini" }]);
    remote.attach_backend(Arc::new(WithSettings(backend, settings)));
    remote.remember_secret(answer);
    let _use = remote.hold("mini").unwrap();
    let mut rx = remote.watch("mini");
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if matches!(
                *rx.borrow_and_update(),
                RemoteStatus::Disconnected { error: Some(_), .. }
            ) {
                return;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let log: Vec<RemoteLogEntry> = remote.log("mini");
    let text = log
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Permission denied"), "{text}");
    assert!(
        text.contains("answer was [hidden]"),
        "the fake ssh did not echo: {text}"
    );
    assert!(!text.contains(answer), "{text}");
}

/// 설정 값을 넣어 목록을 만드는 백엔드(RemoteHosts 에 전역 설정이 없는 시험).
struct WithSettings(SshBackend, Value);

impl Backend for WithSettings {
    fn hosts(&self, _settings: Option<&Value>) -> (Vec<HostEntry>, Vec<String>) {
        self.0.hosts(Some(&self.1))
    }
    fn connect<'a>(
        &'a self,
        entry: &'a HostEntry,
        expect: Option<&'a str>,
        progress: &'a (dyn Fn(Progress) + Send + Sync),
    ) -> BoxFuture<'a, Result<Link, ConnectError>> {
        self.0.connect(entry, expect, progress)
    }
    fn check<'a>(&'a self, entry: &'a HostEntry) -> BoxFuture<'a, Option<bool>> {
        self.0.check(entry)
    }
}

// 목록: 전역 `[[remote_hosts]]`(별명이 이름)과 `~/.ssh/config` 의 이름(와일드카드 · 부정 · 옵션처럼 읽히는 이름은 뺀다), 같은
// 이름은 설정의 것. 가벼운 확인은 마스터가 있을 때만 참이고 원격 명령을 돌리지 않는다.
#[tokio::test]
async fn the_list_joins_the_settings_and_the_ssh_config_and_the_check_runs_no_command() {
    let s = setup();
    let ssh_dir = s.dir.path().join("local-home/.ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    std::fs::write(
        ssh_dir.join("config"),
        "Host mini build *.lan !bad\n  HostName 10.0.0.2\nHost other\n",
    )
    .unwrap();
    let backend = s.backend(&[("FAKE_SSH_MASTER", "running")], None);
    let (entries, warnings) = backend.hosts(Some(&json!([
        { "nickname": "build", "host": "build.tail.ts.net", "username": "dev", "port": 2222,
          "ssh_args": ["-o", "ServerAliveInterval=30"] },
        { "host": "x", "ssh_args": ["-oStrictHostKeyChecking=no"] }
    ])));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let names: BTreeSet<_> = entries
        .iter()
        .map(|e| (e.name.as_str(), format!("{:?}", e.source)))
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            ("build", "Settings".to_owned()),
            ("mini", "SshConfig".to_owned()),
            ("other", "SshConfig".to_owned()),
        ])
    );
    let build = entries.iter().find(|e| e.name == "build").unwrap();
    assert_eq!(
        (build.user.as_deref(), build.port),
        (Some("dev"), Some(2222))
    );
    assert_eq!(backend.check(&mini()).await, Some(true));
    assert_eq!(s.backend(&[], None).check(&mini()).await, None);
    for call in s.remote.calls() {
        assert_eq!(call, ["-O", "check", "--", "mini"]);
    }
}

// R-g: 방금 페어링했는데 `Unpaired` 면 한 번 더 붙는다. 처음부터 있던 페어링이면 다시 하지 않는다(다른 거절도).
#[tokio::test]
async fn an_unpaired_refusal_right_after_pairing_is_tried_once_more() {
    for (paired_now, first, expect_calls, ok) in [
        (true, DialError::Rejected(RejectReason::Unpaired), 2, true),
        (false, DialError::Rejected(RejectReason::Unpaired), 1, false),
        (true, DialError::Rejected(RejectReason::Version), 1, false),
    ] {
        let calls = AtomicUsize::new(0);
        let result = attach_retrying(
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let first = first.clone();
                async move { if n == 0 { Err(first) } else { Ok(()) } }
            },
            paired_now,
            Duration::from_millis(1),
            &|_| {},
            &|| {},
        )
        .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            expect_calls,
            "{paired_now} {first:?}"
        );
        assert_eq!(result.is_ok(), ok);
    }
}

// 리뷰 B1 · 델타 F-a: 「머신이 바뀜」 은 Desktop 이 기억한 id 와 **서명이 맞는** 다른 키뿐이다. 처음 붙는 호스트에서 원격이 남의
// id 를 대거나, 키를 증명하지 못하면 그냥 거절이다 — 되돌린 자기 hello 는 원격이 말한 `SHA256:fakemachine` 도 아니고 서명도 없다.
#[tokio::test]
async fn a_relay_that_does_not_prove_the_id_it_reported_is_refused() {
    let s = setup();
    let backend = s.backend(&[], None);
    let (result, _) = connect(&backend, None).await;
    let e = result.err().unwrap();
    assert_eq!(e.machine_changed, None, "{e:?}");
    assert_eq!(
        (e.stage, e.kind),
        (RemoteStage::Handshake, RemoteFailureKind::Rejected)
    );

    // 기억이 있어도 서명 없는 키(되돌린 자기 hello)는 「머신이 바뀜」 이 아니다(델타 리뷰 F-a).
    let (result, _) = connect(&backend, Some("SHA256:fakemachine")).await;
    let e = result.err().unwrap();
    assert_eq!(e.machine_changed, None, "{e:?}");
    assert_eq!(e.kind, RemoteFailureKind::Rejected);
}

#[test]
fn only_a_remembered_id_makes_a_machine_change() {
    let mismatch = DialError::MachineMismatch {
        expected: pw_link::DeviceId::from_stored("SHA256:claimed-by-the-remote"),
        found: pw_link::DeviceId::from_stored("SHA256:its-key"),
    };
    assert_eq!(machine_change(None, &mismatch), None);
    assert_eq!(
        machine_change(Some("SHA256:remembered"), &mismatch),
        Some(("SHA256:remembered".into(), "SHA256:its-key".into()))
    );
    assert_eq!(machine_change(Some("x"), &DialError::Timeout), None);
}

// 리뷰 F4: 암호 질문에 틀린 답(또는 취소)이면 ssh 는 「Permission denied」 로 끝난다 — 저절로 다시 하지 않는 로그인 실패다.
#[tokio::test]
async fn a_wrong_or_cancelled_password_is_a_sign_in_failure() {
    let s = setup();
    for answer in ["wrong-secret", ""] {
        let backend = s.backend(
            &[("FAKE_SSH_PASSWORD", "right-secret")],
            Some(s.answers(answer)),
        );
        let (result, _) = connect(&backend, None).await;
        let e = result.err().unwrap();
        assert_eq!(
            (e.stage, e.kind),
            (RemoteStage::Ssh, RemoteFailureKind::Auth),
            "{e:?}"
        );
        assert!(!e.kind.retryable());
    }
}

#[cfg(test)]
mod direct;
