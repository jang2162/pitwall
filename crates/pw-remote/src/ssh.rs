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
//! 0. **직접 연결 먼저 — 그 주소를 알 때만**(18-m7-plan M7-24 ④ · 리뷰 F3 메인 판정): 이 호스트에 기억한 머신 id 가 있고, 직접 연결
//!    주소를 알 때(코드로 페어링한 직접 연결 전용 호스트 · 이 실행에서 직접 연결이 섰거나 SSH 링크 뒤 그 포트가 답한 호스트)만
//!    QUIC + Noise(`pw_link::SecuredTransport<QuicTransport>`, 그 머신 id 만 기대한다)를 먼저 해 본다 — SSH 로 떨어질 수 있으면
//!    [`DIRECT_FIRST_TIMEOUT`](1.2초) 안에. 모르면 QUIC 를 보내지 않고 SSH 로 바로 붙는다(재부팅 뒤 첫 연결은 SSH — 결정 M7-D2).
//!    안 되면 SSH 로 떨어진다 — SSH 는 원래 길이라 보안이 낮아지지 않는다. **처음 붙는 호스트(기억한 id 없음)는 직접 연결을 하지 않는다**(Noise ·
//!    링크가 처음 보는 키를 믿게 되는 길이 없게 — 처음 페어링은 SSH 나 코드로). 무엇으로 붙었는지는 상태의 `via` 로 사람에게 보인다.
//!    직접 연결이 다른 머신(다른 키)에 닿았으면 연결 로그에 경고를 남기고 SSH 로 간다.
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
use pw_link::quic::QuicTransport;
use pw_link::ssh::{AskpassSession, Ssh, SshAskpass, SshError, SshTarget, SshTransport};
use pw_link::transport::{LinkStream, Transport};
use pw_link::{
    DesktopIdentity, DeviceKey, DeviceKeyStore, DialError, Expect, LinkLimits, LinkState,
    RejectReason, RemoteHost, SecuredTransport, Versions,
};
use pw_link::{remote_hosts, upgrade};
use serde_json::Value;

use crate::{
    Backend, ConnectError, HostEntry, Link, PROMPTS_KEY, Progress, RemoteCommandLink,
    RemoteFailureKind, RemoteHostSource, RemoteOldRelay, RemoteSilentRelay, RemoteStage,
    RemoteStopOutcome, RemoteVia, Tidied, UpgradeError,
};

/// 코드 페어링 한 번의 시한.
const PAIR_TIMEOUT: Duration = Duration::from_secs(15);

/// 코드 페어링 실패를 사람이 읽는 말로(코드는 싣지 않는다).
fn pair_error(e: &pw_link::PairError) -> String {
    use pw_link::PairError as E;
    match e {
        E::Rejected(RejectReason::Code) => {
            "the relay refused the code (wrong, expired or used up — show a new code on the relay)".into()
        }
        E::RelayNotProven => "the relay at that address did not prove the code — it may not be the machine you meant; nothing was paired".into(),
        other => format!("pairing did not finish: {other}"),
    }
}

/// 직접 연결 전용 호스트(`ssh = false`)의 직접 연결 한 번(QUIC 연결 · Noise · 링크 핸드셰이크) 시한 — 떨어질 길이 없으니 넉넉히.
pub const DIRECT_TIMEOUT: Duration = Duration::from_secs(6);
/// SSH 로 떨어질 수 있는 호스트의 직접 연결 시한(리뷰 F3 · 메인 판정). LAN · tailnet 의 왕복은 보통 수십 ms 이고 붙기 한 번은 왕복
/// 여섯쯤(QUIC Retry · 핸드셰이크 2 + Noise 1.5 + 링크 핸드셰이크 2)이라, 1.2초면 왕복 200ms(DERP 를 거치는 tailnet)까지 덮으면서
/// 데몬이 없을 때(재부팅 · 유휴로 끝난 뒤) 사람이 기다리는 시간을 그만큼으로 묶는다.
pub const DIRECT_FIRST_TIMEOUT: Duration = Duration::from_millis(1200);
/// SSH 링크가 선 뒤 그 호스트의 직접 연결 포트에 받는 쪽이 있는지 보는 QUIC 핸드셰이크의 시한.
const DIRECT_PROBE_TIMEOUT: Duration = Duration::from_millis(800);

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
        // 창에 질문이 떠 있는 동안은 `Ssh::run` 의 시간 상한을 세지 않는다.
        let session = Arc::new(session);
        let asking = Arc::clone(&session);
        Some(AskpassSession::new(env, session).with_asking(Arc::new(move || asking.asking())))
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
    /// 증명된 머신 id 마다 지금 링크의 원격 Host(포트 포워딩이 연결을 잇는다 — 18-m7-plan M7-22). 링크가 사라지면 비어 있다.
    links: Mutex<std::collections::HashMap<String, std::sync::Weak<RemoteHost>>>,
    /// 직접 연결 주소를 아는 호스트 항목(리뷰 F3): 이 실행에서 직접 연결이 선 적이 있거나, SSH 링크가 선 뒤 그 주소의 직접 연결
    /// 포트가 답했다. 여기 있는 SSH 호스트만 직접 연결을 먼저 해 본다. 직접 연결이 실패하면 뺀다(다음은 SSH 로 바로). 앱이 다시
    /// 뜨면 비어 있다 — 그래서 하루의 첫 연결은 SSH 다(결정 M7-D2).
    direct_known: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// 원격 Host 의 링크로 포워딩 연결을 잇는다. 링크(원격 Host)가 사라졌으면 거절한다.
struct LinkForwarder(std::sync::Weak<RemoteHost>);

impl crate::Forwarder for LinkForwarder {
    fn carry(
        &self,
        socket: tokio::net::TcpStream,
        to: pw_link::forward::ForwardTo,
    ) -> BoxFuture<'static, Result<(), String>> {
        let remote = self.0.upgrade();
        Box::pin(async move {
            let remote = remote.ok_or("the link is gone")?;
            remote.forward(to, socket).await.map_err(|e| e.to_string())
        })
    }
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
            links: Mutex::default(),
            direct_known: Arc::default(),
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
        let mut direct_failed = None;
        // 직접 연결은 그 주소를 알 때만(코드로 페어링한 직접 연결 전용 호스트 · 이 실행에서 직접 연결이 선 · 답한 호스트) 먼저 해 본다.
        // 모르면 QUIC 를 한 번도 보내지 않고 SSH 로 바로 간다(리뷰 F3).
        let known = !entry.ssh || self.knows_direct(&entry.name);
        if let (Some(port), Some(expected), true) = (entry.direct, expect, known) {
            progress(Progress::Stage(RemoteStage::Direct));
            let limit = if entry.ssh {
                DIRECT_FIRST_TIMEOUT
            } else {
                DIRECT_TIMEOUT
            };
            match self.direct(entry, port, expected, limit).await {
                Ok(link) => {
                    self.learn_direct(&entry.name, true);
                    progress(Progress::Via(RemoteVia::Direct));
                    return Ok(link);
                }
                Err((why, other_machine)) => {
                    self.learn_direct(&entry.name, false);
                    let line = format!(
                        "direct connection to {}:{port} did not stand ({why}){}",
                        entry.host,
                        if entry.ssh {
                            "; connecting over ssh"
                        } else {
                            ""
                        }
                    );
                    progress(if other_machine {
                        Progress::Warn(line)
                    } else {
                        Progress::Note(line)
                    });
                    direct_failed = Some(why);
                }
            }
        }
        if !entry.ssh {
            return Err(ConnectError::new(
                RemoteStage::Direct,
                RemoteFailureKind::Link,
                match (direct_failed, expect) {
                    (Some(why), _) => format!("the direct connection did not stand: {why}"),
                    (None, None) => "this host connects directly only, and no machine is remembered for it; pair it again with a code".to_owned(),
                    (None, Some(_)) => "this host connects directly only, and direct connections are off for it".to_owned(),
                },
            ));
        }
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
        // 핸드셰이크가 기대할 id: 이 호스트에 기억한 id, 없으면(처음) 페어링 답의 id. 페어링 답은 원격 stdout 이라 증명이 아니다 —
        // 기억과 달라도 여기서 「머신이 바뀜」 으로 멈추지 않고 핸드셰이크가 서명으로 가른다(델타 리뷰 F-a): 기억한 키의 Relay 면
        // 붙고, 서명이 맞는 다른 키면 `MachineMismatch`(그 키의 지문 — 증명된 `found`), 서명이 맞지 않으면 거절이다.
        if let Some(expected) = expect
            && expected != boot.machine.as_str()
        {
            progress(Progress::Note(format!(
                "the relay reported machine {} but this host remembers {expected}; the handshake decides",
                boot.machine
            )));
        }
        let expected = expect.map_or_else(|| boot.machine.clone(), pw_link::DeviceId::from_stored);
        let machine = expected.as_str().to_owned();
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
            Expect::Machine(expected),
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
            return Err(dial_error(at(), e, &transport, expect));
        }
        progress(Progress::Stage(RemoteStage::Handshake));
        progress(Progress::Via(RemoteVia::Ssh));
        if let Some(port) = entry.direct {
            self.probe_direct(entry, port);
        }
        Ok(self.link_of(remote, machine))
    }

    fn knows_direct(&self, name: &str) -> bool {
        self.direct_known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(name)
    }

    fn learn_direct(&self, name: &str, works: bool) {
        let mut known = self
            .direct_known
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if works {
            known.insert(name.to_owned());
        } else {
            known.remove(name);
        }
    }

    /// 시험: 이 호스트의 직접 연결 주소를 안다고 둔다.
    #[cfg(test)]
    pub(crate) fn remember_direct(&self, name: &str) {
        self.learn_direct(name, true);
    }

    /// SSH 링크가 선 뒤 한 번: 그 호스트 주소의 직접 연결 포트에 받는 쪽이 있으면(QUIC 핸드셰이크만) 다음 붙기부터 직접 연결을 먼저
    /// 해 본다. 상대가 누구인지는 그 직접 연결의 Noise · 기억한 머신 키가 판정한다.
    fn probe_direct(&self, entry: &HostEntry, port: u16) {
        let (known, name, host) = (
            Arc::clone(&self.direct_known),
            entry.name.clone(),
            entry.host.clone(),
        );
        tokio::spawn(async move {
            let Ok(mut addrs) = tokio::net::lookup_host((host.as_str(), port)).await else {
                return;
            };
            let Some(addr) = addrs.next() else { return };
            if pw_link::quic::probe(addr, DIRECT_PROBE_TIMEOUT).await {
                known
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(name);
            }
        });
    }

    /// 직접 연결 한 번: 기억한 머신 `expected` 만 기대한다. 실패하면 (까닭, 다른 머신이 답했나).
    async fn direct(
        &self,
        entry: &HostEntry,
        port: u16,
        expected: &str,
        limit: Duration,
    ) -> Result<Link, (String, bool)> {
        let key = self.key().await.map_err(|e| (e.message, false))?;
        let transport = SecuredTransport::new(
            QuicTransport::new(entry.host.clone(), port).with_connect_timeout(limit),
            Arc::clone(&key),
        );
        let identity = Arc::new(DesktopIdentity {
            key,
            versions: self.versions.clone(),
        });
        let expected_id = pw_link::DeviceId::from_stored(expected);
        let remote = Arc::new(RemoteHost::new(
            expected.to_owned(),
            Arc::new(transport) as Arc<dyn Transport>,
            identity,
            Expect::Machine(expected_id),
            self.limits,
        ));
        match tokio::time::timeout(limit, remote.attach()).await {
            Ok(Ok(_)) => Ok(self.link_of(remote, expected.to_owned())),
            Ok(Err(e)) => {
                remote.detach();
                let other = matches!(e, DialError::MachineMismatch { .. });
                Err((format!("{e:?}"), other))
            }
            Err(_) => {
                remote.detach();
                Err((format!("no link in {} ms", limit.as_millis()), false))
            }
        }
    }

    /// 붙은 원격 Host 로 [`Link`] 를 만든다(SSH · 직접 연결이 같다).
    fn link_of(&self, remote: Arc<RemoteHost>, machine: String) -> Link {
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
        // 기억 · 라우터 id 는 핸드셰이크가 증명한 키의 지문이다(원격이 페어링 답으로 말한 값이 아니라 — 리뷰 B1).
        let machine = remote
            .link()
            .map_or(machine, |link| link.machine().as_str().to_owned());
        // 링크 상태의 이어 붙기(M7-09 ①)를 상태 `resuming` 으로 낸다(M7-14).
        let (resuming_tx, resuming) = tokio::sync::watch::channel(false);
        let mut watched = remote.state();
        tokio::spawn(async move {
            loop {
                let now = matches!(*watched.borrow_and_update(), LinkState::Reconnecting);
                resuming_tx.send_if_modified(|was| std::mem::replace(was, now) != now);
                if matches!(*watched.borrow(), LinkState::Disconnected { .. })
                    || watched.changed().await.is_err()
                    || resuming_tx.is_closed()
                {
                    return;
                }
            }
        });
        {
            let mut links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
            links.retain(|_, held| held.strong_count() > 0);
            links.insert(machine.clone(), Arc::downgrade(&remote));
        }
        let closer = Arc::clone(&remote);
        Link {
            host: remote,
            machine,
            closed,
            resuming: Some(resuming),
            close: Box::new(move || closer.detach()),
        }
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
        // 인증 실패(사람이 암호 질문을 취소했거나 틀린 답 · 받는 키가 없음)는 저절로 다시 해도 같은 질문만 되풀이한다(리뷰 F4).
        SshError::Connect { stderr } if auth_failed(stderr) => {
            (RemoteFailureKind::Auth, Some(stderr.clone()))
        }
        SshError::Connect { stderr } | SshError::TimedOut { stderr, .. } => {
            (RemoteFailureKind::Ssh, Some(stderr.clone()))
        }
        SshError::Invalid(_) | SshError::Spawn(_) => (RemoteFailureKind::Ssh, None),
    };
    let message = match &e {
        SshError::HostKeyChanged { .. } => {
            "the remote host key changed; ssh refused to connect".into()
        }
        SshError::HostKeyNotAccepted { .. } => "the remote host key was not accepted".into(),
        SshError::Connect { .. } if kind == RemoteFailureKind::Auth => {
            "ssh could not sign in".into()
        }
        SshError::Connect { .. } => "ssh could not connect".into(),
        SshError::TimedOut { seconds, .. } => format!("ssh did not finish in {seconds} s"),
        other => other.to_string(),
    };
    ConnectError {
        detail,
        ..ConnectError::new(stage, kind, message)
    }
}

/// ssh 의 인증 실패 문장(OpenSSH `sshconnect2.c` — 「Permission denied (publickey,password).」).
fn auth_failed(stderr: &str) -> bool {
    stderr.contains("Permission denied")
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

fn dial_error(
    stage: RemoteStage,
    e: DialError,
    transport: &SshTransport,
    expect: Option<&str>,
) -> ConnectError {
    let kind = match &e {
        DialError::VersionMismatch { .. } => RemoteFailureKind::VersionMismatch,
        DialError::Rejected(_) | DialError::BadRelaySignature => RemoteFailureKind::Rejected,
        DialError::MachineMismatch { .. } => RemoteFailureKind::Rejected,
        DialError::Io(_) | DialError::Timeout | DialError::Protocol(_) => RemoteFailureKind::Link,
    };
    let stderr = transport.last_stderr();
    let mut error = ConnectError::new(stage, kind, e.to_string());
    error.machine_changed = machine_change(expect, &e);
    if !stderr.is_empty() {
        error.detail = Some(stderr);
    }
    error
}

/// 핸드셰이크의 머신 id 불일치를 「머신이 바뀜」(사람이 새로 믿을 수 있음)으로 볼 것인가. `expected` 는 **Desktop 이 기억한 id**
/// (`expect`)로만 채운다 — 기억이 없는 호스트에서의 불일치는 원격이 자기 id 를 거짓으로 말한 것이라 그냥 거절이다(리뷰 B1:
/// 페어링 답의 id 는 원격이 고른다 — 그것을 `expected` 로 두면 「새로 믿기」 가 남의 묶음 멤버를 원격으로 옮긴다).
pub fn machine_change(expect: Option<&str>, e: &DialError) -> Option<(String, String)> {
    match (expect, e) {
        (Some(expected), DialError::MachineMismatch { found, .. }) => {
            Some((expected.to_owned(), found.as_str().to_owned()))
        }
        _ => None,
    }
}

/// 판 바꾸기 · 정리(`pw_link::upgrade` — M7-07b). 붙기와 같은 ssh(사람의 옵션 · askpass)와 같은 판이다.
impl SshBackend {
    /// 링크의 머신(`machine`)을 ssh 로 닿은 곳에서 확인하는 설치기(리뷰 R-1 — `Installer::expect_machine`).
    fn installer(&self, entry: &HostEntry, machine: &str) -> Result<Installer, UpgradeError> {
        let ssh = self
            .ssh(entry)
            .map_err(|e| UpgradeError::Failed(e.to_string()))?;
        Installer::new(
            Arc::new(ssh),
            self.versions.clone(),
            RelayDist::new(self.parts.dist.clone(), Arc::clone(&self.parts.fetch)),
        )
        .map(|i| i.expect_machine(machine))
        .map_err(upgrade_failure)
    }

    /// 조사 → (사람이 둔 `pitwall` 링크가 옛 판이면) 이 판으로 옮김 → 정리. 링크를 옮기지 못하면 그 링크가 가리키는 바이너리는 이번
    /// 정리에서 지우지 않는다(사람의 `pitwall` 이 깨지지 않게).
    async fn tidy_run(&self, entry: &HostEntry, machine: &str) -> Result<Tidied, UpgradeError> {
        let installer = self.installer(entry, machine)?;
        let survey = installer.survey().await.map_err(upgrade_failure)?;
        let mut notes = Vec::new();
        let mut keep = survey.clone();
        let command = match &survey.command {
            upgrade::CommandLink::Absent => RemoteCommandLink::Absent,
            upgrade::CommandLink::Other => RemoteCommandLink::Other,
            upgrade::CommandLink::Relay { file } => {
                if survey.command_needs_moving() {
                    match installer.link_command(true).await {
                        Ok(_) => notes.push(format!(
                            "moved ~/.local/bin/pitwall from {file} to this relay version"
                        )),
                        Err(e @ InstallError::OtherMachine { .. }) => {
                            return Err(upgrade_failure(e));
                        }
                        Err(e) => {
                            notes.push(format!(
                                "could not move ~/.local/bin/pitwall to this relay version ({e}); kept {file}"
                            ));
                            keep.binaries.retain(|b| &b.file != file);
                        }
                    }
                }
                RemoteCommandLink::Linked
            }
        };
        let cleaned = installer.clean(&keep).await.map_err(upgrade_failure)?;
        notes.extend(
            cleaned
                .running
                .iter()
                .map(|file| format!("kept {file}: its daemon is running")),
        );
        notes.extend(cleaned.unknown.iter().map(|(file, code)| match code {
            Some(code) => format!("kept {file}: its status did not answer (exit {code})"),
            None => format!("kept {file}: its status did not answer"),
        }));
        // 정리가 지운 것은 조사의 옛 판 목록에서도 뺀다(떠 있던 데몬이 조사 뒤 끝났을 때).
        let old = survey
            .old_daemons()
            .iter()
            .filter(|d| !cleaned.removed.contains(&d.file))
            .map(old_relay)
            .collect();
        let silent = survey
            .unknown()
            .into_iter()
            .filter(|b| !cleaned.removed.contains(&b.file))
            .map(|b| RemoteSilentRelay {
                file: b.file.clone(),
                version: b.build.as_ref().map(|(_, v)| v.to_string()),
            })
            .collect();
        Ok(Tidied {
            old,
            silent,
            command,
            removed: cleaned.removed,
            notes,
        })
    }

    async fn stop_run(
        &self,
        entry: &HostEntry,
        machine: &str,
        seen: &RemoteOldRelay,
    ) -> Result<RemoteStopOutcome, UpgradeError> {
        let installer = self.installer(entry, machine)?;
        let seen = upgrade::OldDaemon {
            file: seen.file.clone(),
            status: upgrade::DaemonStatus {
                version: seen.version.clone().unwrap_or_default(),
                release_channel: seen.channel.clone().unwrap_or_default(),
                name: None,
                pid: seen.pid,
                links: seen.links as usize,
                owner_links: 0,
                runs: seen.runs as usize,
                terminals: seen.terminals as usize,
            },
        };
        let outcome = installer.stop_old(&seen).await.map_err(upgrade_failure)?;
        Ok(match outcome {
            upgrade::StopOutcome::Stopped => RemoteStopOutcome::Stopped,
            upgrade::StopOutcome::Stopping => RemoteStopOutcome::Stopping,
            upgrade::StopOutcome::Gone => RemoteStopOutcome::Gone,
            upgrade::StopOutcome::NoAnswer => RemoteStopOutcome::NoAnswer,
            upgrade::StopOutcome::Changed(now) => RemoteStopOutcome::Changed {
                now: old_relay(&now),
            },
        })
    }
}

fn upgrade_failure(e: InstallError) -> UpgradeError {
    match e {
        InstallError::OtherMachine { found } => UpgradeError::OtherMachine { found },
        other => UpgradeError::Failed(other.to_string()),
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 조사의 옛 판 데몬 → 화면 · `remote.stopOld` 의 값.
fn old_relay(daemon: &upgrade::OldDaemon) -> RemoteOldRelay {
    let build = upgrade::parse_name(&daemon.file);
    RemoteOldRelay {
        file: daemon.file.clone(),
        version: build.as_ref().map(|(_, v)| v.to_string()),
        channel: build.map(|(c, _)| c),
        pid: daemon.status.pid,
        runs: count(daemon.status.runs),
        terminals: count(daemon.status.terminals),
        links: count(daemon.status.links),
    }
}

impl Backend for SshBackend {
    fn port_forwards(
        &self,
        settings: Option<&Value>,
        name: &str,
    ) -> Vec<pw_link::forward::ForwardSetting> {
        remote_hosts::read(settings, None)
            .0
            .into_iter()
            .find(|h| h.name() == name)
            .map(|h| h.port_forwards)
            .unwrap_or_default()
    }

    fn forwarder(&self, machine: &str) -> Option<Arc<dyn crate::Forwarder>> {
        let held = self
            .links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(machine)
            .cloned()?;
        Some(Arc::new(LinkForwarder(held)))
    }

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
                direct: h.direct,
                ssh: h.ssh,
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
                    direct: Some(pw_link::quic::DEFAULT_PORT),
                    ssh: true,
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

    fn tidy<'a>(
        &'a self,
        entry: &'a HostEntry,
        machine: &'a str,
    ) -> BoxFuture<'a, Result<Tidied, UpgradeError>> {
        Box::pin(self.tidy_run(entry, machine))
    }

    fn stop_old<'a>(
        &'a self,
        entry: &'a HostEntry,
        machine: &'a str,
        seen: &'a RemoteOldRelay,
    ) -> BoxFuture<'a, Result<RemoteStopOutcome, UpgradeError>> {
        Box::pin(self.stop_run(entry, machine, seen))
    }

    fn link_command<'a>(
        &'a self,
        entry: &'a HostEntry,
        machine: &'a str,
        on: bool,
    ) -> BoxFuture<'a, Result<bool, UpgradeError>> {
        Box::pin(async move {
            self.installer(entry, machine)?
                .link_command(on)
                .await
                .map_err(upgrade_failure)
        })
    }

    fn resident<'a>(
        &'a self,
        entry: &'a HostEntry,
        machine: &'a str,
        on: Option<bool>,
    ) -> BoxFuture<'a, Result<bool, UpgradeError>> {
        Box::pin(async move {
            self.installer(entry, machine)?
                .resident(on)
                .await
                .map_err(upgrade_failure)
        })
    }

    fn pair_direct<'a>(
        &'a self,
        address: &'a str,
        port: u16,
        code: &'a str,
    ) -> BoxFuture<'a, Result<(String, pw_rpc::Role), String>> {
        Box::pin(async move {
            let key = self.key().await.map_err(|e| e.message)?;
            let transport = SecuredTransport::new(
                QuicTransport::new(address.to_owned(), port),
                Arc::clone(&key),
            );
            let stream = transport
                .open()
                .await
                .map_err(|e| format!("cannot reach {address}:{port} directly: {e}"))?;
            let identity = DesktopIdentity {
                key,
                versions: self.versions.clone(),
            };
            let paired = pw_link::pair_with_code(
                stream,
                &identity,
                code,
                &self.parts.device_name,
                PAIR_TIMEOUT,
            )
            .await
            .map_err(|e| pair_error(&e))?;
            Ok((paired.machine.as_str().to_owned(), paired.role))
        })
    }

    fn discover(&self) -> BoxFuture<'_, crate::RemoteDiscovery> {
        Box::pin(crate::ssh::discover::run())
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

pub mod discover;

#[cfg(all(test, unix))]
mod tests;

#[cfg(all(test, unix))]
mod upgrade_tests;
