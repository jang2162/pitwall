//! 실제 SSH · 설치 · 링크(기능 `ssh` — 데스크톱 앱만). ssh argv 는 `pw_link::ssh` 의 것만 쓴다(18-m7-plan 「공유 부품의 주인」 —
//! SSH 실행은 M7-06). 이 모듈은 그 함수들을 차례로 부른다:
//!
//! 1. **SSH · 설치**: `pw_link::install::Installer::bootstrap` — 감지 → 설치(같은 판 · 같은 해시면 그대로) → SSH 자동 페어링. ssh 의
//!    질문(호스트 키 확인 · 암호)은 git 과 같은 askpass(D43)로 창에 간다([`RemoteAskpass`] — 열쇠 [`crate::PROMPTS_KEY`]).
//! 2. **머신 id**: 페어링이 돌려준 Relay 기기 키 지문이 기억한 id 와 다르면 붙지 않는다(메인 판정 M7-P6).
//! 3. **데몬 · 핸드셰이크**: `pitwall-relay proxy` 의 stdio 위에 `pw_link::RemoteHost::attach`(그 머신 id 만 기대한다). **방금 이
//!    기기를 페어링했는데 Relay 가 `Unpaired` 로 거절하면 한 번 더 붙어 본다**(M7-06 리뷰 R-g — 떠 있던 데몬이 페어링 파일을 다시
//!    읽기 전일 수 있다).
//!
//! 이 Desktop 의 기기 키는 OS 키체인(`pw_link::KeychainKeyStore::for_instance(<단일 인스턴스 키>)` — 앱이 넣는다), 판은
//! [`versions`](워크스페이스 판 + 빌드에 박은 릴리스 채널 `PITWALL_RELEASE_CHANNEL`).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_core::BoxFuture;
use pw_git::write::prompt::ASKPASS_TIMEOUT;
use pw_git::write::{Askpass, Prompts};
use pw_link::install::{Fetch, InstallError, Installer, RelayDist};
use pw_link::remote_hosts;
use pw_link::ssh::{AskpassSession, Ssh, SshAskpass, SshError, SshTarget, SshTransport};
use pw_link::transport::{LinkStream, Transport};
use pw_link::{
    DesktopIdentity, DeviceKey, DeviceKeyStore, DialError, Expect, LinkLimits, LinkState,
    RejectReason, RemoteHost, Versions,
};
use serde_json::Value;

use crate::{
    Backend, ConnectError, HostEntry, Link, PROMPTS_KEY, Progress, RemoteFailureKind,
    RemoteHostSource, RemoteStage,
};

/// 이 빌드의 판(워크스페이스 판 하나 — Relay 와 같다).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 릴리스 채널. 릴리스 빌드가 `PITWALL_RELEASE_CHANNEL` 로 박는다(release.yml 의 build 잡 — Relay 와 같은 값). 없으면 `dev`.
/// 원격 파일 이름 `pitwall-relay-{채널}-{판}` 과 링크 핸드셰이크가 이 값을 Relay 의 것과 맞춘다.
pub const RELEASE_CHANNEL: &str = match option_env!("PITWALL_RELEASE_CHANNEL") {
    Some(channel) => channel,
    None => "dev",
};

/// 이 Desktop 의 판.
pub fn versions() -> Versions {
    Versions::new(VERSION, RELEASE_CHANNEL)
}

/// 방금 페어링한 뒤 `Unpaired` 로 거절되면 다시 붙기 전에 기다리는 시간(데몬은 페어링 파일을 1초마다 본다 — `pw_relay::pairings`).
pub const PAIRING_SETTLE: Duration = Duration::from_millis(1500);

/// 원격 호스트의 ssh 가 묻는 것을 창으로(`pw_git::write::Askpass` — 열쇠 [`PROMPTS_KEY`]).
pub struct RemoteAskpass {
    askpass: Askpass,
    prompts: Prompts,
}

impl RemoteAskpass {
    pub fn new(askpass: Askpass, prompts: Prompts) -> Self {
        Self { askpass, prompts }
    }
}

impl SshAskpass for RemoteAskpass {
    fn session(&self) -> Option<AskpassSession> {
        let session = self
            .askpass
            .session(PROMPTS_KEY, &self.prompts, ASKPASS_TIMEOUT)?;
        let mut env = Vec::new();
        session.apply(&mut env);
        Some(AskpassSession::new(env, session))
    }
}

/// [`SshBackend`] 의 부품.
pub struct SshParts {
    /// 이 Desktop 의 기기 키(OS 키체인). 처음 붙을 때 읽는다(키체인이 사람에게 물을 수 있다).
    pub keys: Arc<dyn DeviceKeyStore>,
    /// `$CACHE/relay-dist/`(서명 매니페스트로 검증한 바이너리)와 받기.
    pub dist: PathBuf,
    pub fetch: Arc<dyn Fetch>,
    pub askpass: Option<Arc<dyn SshAskpass>>,
    /// 페어링 표에 남길 이 기기의 이름(`[A-Za-z0-9._-]`, 64자까지).
    pub device_name: String,
    /// `~/.ssh/config` 를 찾을 홈.
    pub home: Option<PathBuf>,
}

/// 시스템 `ssh` 로 붙는 [`Backend`].
pub struct SshBackend {
    parts: SshParts,
    versions: Versions,
    program: PathBuf,
    env: Option<Vec<(OsString, OsString)>>,
    limits: LinkLimits,
    settle: Duration,
    key: tokio::sync::OnceCell<Arc<DeviceKey>>,
}

impl SshBackend {
    pub fn new(parts: SshParts) -> Self {
        Self {
            parts,
            versions: versions(),
            program: PathBuf::from("ssh"),
            env: None,
            limits: LinkLimits::default(),
            settle: PAIRING_SETTLE,
            key: tokio::sync::OnceCell::new(),
        }
    }

    /// 시험: 가짜 `ssh` · 그 환경 · 판 · 기다림.
    pub fn with_program(
        mut self,
        program: PathBuf,
        env: Vec<(OsString, OsString)>,
        versions: Versions,
        settle: Duration,
    ) -> Self {
        self.program = program;
        self.env = Some(env);
        self.versions = versions;
        self.settle = settle;
        self
    }

    fn ssh(&self, entry: &HostEntry) -> Result<Ssh, SshError> {
        let target = SshTarget {
            host: entry.host.clone(),
            user: entry.user.clone(),
            port: entry.port,
        };
        let mut ssh = Ssh::with_program(self.program.clone(), target, entry.ssh_args.clone())?;
        if let Some(env) = &self.env {
            ssh = ssh.env(env.clone());
        }
        if let Some(askpass) = &self.parts.askpass {
            ssh = ssh.askpass(Arc::clone(askpass));
        }
        Ok(ssh)
    }

    async fn key(&self) -> Result<Arc<DeviceKey>, ConnectError> {
        self.key
            .get_or_try_init(|| async {
                let keys = Arc::clone(&self.parts.keys);
                tokio::task::spawn_blocking(move || keys.load_or_create())
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()))
                    .map(Arc::new)
            })
            .await
            .cloned()
            .map_err(|e| {
                ConnectError::new(
                    RemoteStage::Install,
                    RemoteFailureKind::Unavailable,
                    format!("cannot read this device's key from the keychain: {e}"),
                )
            })
    }

    async fn run(
        &self,
        entry: &HostEntry,
        expect: Option<&str>,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<Link, ConnectError> {
        let stage = Arc::new(Mutex::new(RemoteStage::Ssh));
        let at = || *stage.lock().unwrap_or_else(PoisonError::into_inner);
        progress(Progress::Stage(RemoteStage::Ssh));
        let ssh = Arc::new(
            self.ssh(entry)
                .map_err(|e| ssh_error(RemoteStage::Ssh, e))?,
        );
        let key = self.key().await?;
        let installer = Installer::new(
            Arc::clone(&ssh),
            self.versions.clone(),
            RelayDist::new(self.parts.dist.clone(), Arc::clone(&self.parts.fetch)),
        )
        .map_err(|e| install_error(RemoteStage::Install, e))?;
        let report = |p: pw_link::install::Progress| {
            use pw_link::install::Progress as P;
            match p {
                P::Probing => {}
                P::Detected { os, arch } => {
                    *stage.lock().unwrap_or_else(PoisonError::into_inner) = RemoteStage::Install;
                    progress(Progress::Stage(RemoteStage::Install));
                    progress(Progress::Note(format!("remote platform: {os} {arch}")));
                }
                P::AlreadyInstalled => progress(Progress::Note(format!(
                    "relay {} ({}) is already installed",
                    self.versions.app, self.versions.release_channel
                ))),
                P::Uploading { bytes } => {
                    progress(Progress::Note(format!(
                        "uploading the relay ({bytes} bytes)"
                    )));
                }
                P::Installed => progress(Progress::Note("installed the relay".into())),
                P::Pairing => progress(Progress::Note("pairing this device with the relay".into())),
            }
        };
        let boot = installer
            .bootstrap(&key.public(), &self.parts.device_name, &report)
            .await
            .map_err(|e| install_error(at(), e))?;
        if boot.paired_now {
            progress(Progress::Note(
                "this device was added to the relay's pairings".into(),
            ));
        }
        let machine = boot.machine.as_str().to_owned();
        if let Some(expected) = expect
            && expected != machine
        {
            let mut e = ConnectError::new(
                RemoteStage::Handshake,
                RemoteFailureKind::Rejected,
                format!("the machine id changed: expected {expected}, found {machine}"),
            );
            e.machine_changed = Some((expected.to_owned(), machine));
            return Err(e);
        }
        let transport = Arc::new(installer.transport());
        let reporting = Arc::new(Reporting {
            inner: Arc::clone(&transport),
            progress: Box::new({
                let stage = Arc::clone(&stage);
                move |s| *stage.lock().unwrap_or_else(PoisonError::into_inner) = s
            }),
        });
        let identity = Arc::new(DesktopIdentity {
            key,
            versions: self.versions.clone(),
        });
        let remote = Arc::new(RemoteHost::new(
            machine.clone(),
            reporting as Arc<dyn Transport>,
            identity,
            Expect::Machine(boot.machine.clone()),
            self.limits,
        ));
        progress(Progress::Stage(RemoteStage::Daemon));
        let attached = attach_retrying(
            || remote.attach(),
            boot.paired_now,
            self.settle,
            &|text| progress(Progress::Note(text)),
            &|| progress(Progress::Stage(RemoteStage::Handshake)),
        )
        .await;
        if let Err(e) = attached {
            return Err(dial_error(at(), e, &transport));
        }
        progress(Progress::Stage(RemoteStage::Handshake));
        let mut state = remote.state();
        let closed: BoxFuture<'static, String> = Box::pin(async move {
            loop {
                let now = state.borrow_and_update().clone();
                match now {
                    LinkState::Connected { .. }
                    | LinkState::Connecting
                    | LinkState::Reconnecting
                    | LinkState::Idle => {}
                    LinkState::Disconnected { reason } => return format!("{reason:?}"),
                    other => return format!("{other:?}"),
                }
                if state.changed().await.is_err() {
                    return "the link went away".into();
                }
            }
        });
        let closer = Arc::clone(&remote);
        Ok(Link {
            host: remote,
            machine,
            closed,
            close: Box::new(move || closer.detach()),
        })
    }
}

/// 링크 전송을 열 때 단계(데몬 → 핸드셰이크)를 알린다.
struct Reporting {
    inner: Arc<SshTransport>,
    progress: Box<dyn Fn(RemoteStage) + Send + Sync>,
}

impl Transport for Reporting {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn open(&self) -> BoxFuture<'_, std::io::Result<LinkStream>> {
        Box::pin(async move {
            (self.progress)(RemoteStage::Daemon);
            let stream = self.inner.open().await?;
            (self.progress)(RemoteStage::Handshake);
            Ok(stream)
        })
    }
}

/// 붙기. 방금 이 기기를 페어링했는데(`paired_now`) Relay 가 `Unpaired` 로 거절하면 `settle` 뒤 한 번 더 붙는다(M7-06 리뷰 R-g).
pub async fn attach_retrying<F, Fut, T>(
    mut attach: F,
    paired_now: bool,
    settle: Duration,
    note: &(dyn Fn(String) + Send + Sync),
    handshake: &(dyn Fn() + Send + Sync),
) -> Result<T, DialError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, DialError>>,
{
    handshake();
    match attach().await {
        Err(DialError::Rejected(RejectReason::Unpaired)) if paired_now => {
            note(format!(
                "the relay has not read the new pairing yet; trying again in {} s",
                settle.as_secs_f32()
            ));
            tokio::time::sleep(settle).await;
            attach().await
        }
        other => other,
    }
}

fn ssh_error(stage: RemoteStage, e: SshError) -> ConnectError {
    let (kind, detail) = match &e {
        SshError::HostKeyChanged { stderr } => {
            (RemoteFailureKind::HostKeyChanged, Some(stderr.clone()))
        }
        SshError::HostKeyNotAccepted { stderr } => {
            (RemoteFailureKind::HostKeyNotAccepted, Some(stderr.clone()))
        }
        SshError::Connect { stderr } => (RemoteFailureKind::Ssh, Some(stderr.clone())),
        SshError::Invalid(_) | SshError::Spawn(_) => (RemoteFailureKind::Ssh, None),
    };
    let message = match &e {
        SshError::HostKeyChanged { .. } => {
            "the remote host key changed; ssh refused to connect".into()
        }
        SshError::HostKeyNotAccepted { .. } => "the remote host key was not accepted".into(),
        SshError::Connect { .. } => "ssh could not connect".into(),
        other => other.to_string(),
    };
    ConnectError {
        detail,
        ..ConnectError::new(stage, kind, message)
    }
}

fn install_error(stage: RemoteStage, e: InstallError) -> ConnectError {
    match e {
        InstallError::Ssh(e) => ssh_error(stage, e),
        InstallError::Unsupported(why) => {
            ConnectError::new(RemoteStage::Install, RemoteFailureKind::Unsupported, why)
        }
        InstallError::Remote { step, code, stderr } => ConnectError {
            detail: Some(stderr),
            ..ConnectError::new(
                RemoteStage::Install,
                RemoteFailureKind::Install,
                match code {
                    Some(code) => format!("the remote {step} failed (exit {code})"),
                    None => format!("the remote {step} was killed"),
                },
            )
        },
        other => ConnectError::new(
            RemoteStage::Install,
            RemoteFailureKind::Install,
            other.to_string(),
        ),
    }
}

fn dial_error(stage: RemoteStage, e: DialError, transport: &SshTransport) -> ConnectError {
    let kind = match &e {
        DialError::VersionMismatch { .. } => RemoteFailureKind::VersionMismatch,
        DialError::Rejected(_) | DialError::BadRelaySignature => RemoteFailureKind::Rejected,
        DialError::MachineMismatch { .. } => RemoteFailureKind::Rejected,
        DialError::Io(_) | DialError::Timeout | DialError::Protocol(_) => RemoteFailureKind::Link,
    };
    let stderr = transport.last_stderr();
    let mut error = ConnectError::new(stage, kind, e.to_string());
    if let DialError::MachineMismatch { expected, found } = &e {
        error.machine_changed = Some((expected.as_str().to_owned(), found.as_str().to_owned()));
    }
    if !stderr.is_empty() {
        error.detail = Some(stderr);
    }
    error
}

impl Backend for SshBackend {
    fn hosts(&self, settings: Option<&Value>) -> (Vec<HostEntry>, Vec<String>) {
        let (configured, warnings) = remote_hosts::read(settings, None);
        let mut entries: Vec<HostEntry> = configured
            .into_iter()
            .map(|h| HostEntry {
                name: h.nickname.unwrap_or_else(|| h.target.host.clone()),
                host: h.target.host,
                user: h.target.user,
                port: h.target.port,
                source: RemoteHostSource::Settings,
                ssh_args: h.ssh_args,
            })
            .collect();
        if let Some(home) = &self.parts.home {
            let ssh_dir = home.join(".ssh");
            for name in pw_db::ssh_hosts::hosts_in(&ssh_dir.join("config"), &ssh_dir, home) {
                if SshTarget::alias(name.clone()).check().is_err()
                    || entries.iter().any(|e| e.name == name)
                {
                    continue;
                }
                entries.push(HostEntry {
                    host: name.clone(),
                    name,
                    user: None,
                    port: None,
                    source: RemoteHostSource::SshConfig,
                    ssh_args: Vec::new(),
                });
            }
        }
        (entries, warnings)
    }

    fn connect<'a>(
        &'a self,
        entry: &'a HostEntry,
        expect: Option<&'a str>,
        progress: &'a (dyn Fn(Progress) + Send + Sync),
    ) -> BoxFuture<'a, Result<Link, ConnectError>> {
        Box::pin(self.run(entry, expect, progress))
    }

    fn check<'a>(&'a self, entry: &'a HostEntry) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async move {
            let ssh = self.ssh(entry).ok()?;
            match ssh.check_master().await {
                Ok(true) => Some(true),
                // 마스터가 없거나 설정이 없다 — 링크 없이는 모른다(마지막 링크 상태를 본다).
                Ok(false) | Err(_) => None,
            }
        })
    }
}

#[cfg(all(test, unix))]
mod tests;
