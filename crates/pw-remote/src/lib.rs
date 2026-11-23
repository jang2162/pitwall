//! 원격 호스트 · 연결 관리 · 진단(18-m7-plan M7-08 — S8 · F9-11). Desktop 만의 것이다 — `pw_host::assemble` 이 Desktop 갈래에서만
//! `remote.*` 를 등록하고 관리자를 [`RemoteHosts`] 로 돌려준다. Relay 는 이 RPC 가 없다.
//!
//! - **목록**: 전역 `[[remote_hosts]]` + `~/.ssh/config` 의 `Host` 이름(와일드카드 없음) — [`Backend::hosts`]. 프로젝트 설정의 그
//!   표는 읽지 않는다(메인 판정 M7-P16).
//! - **상태 기계**([`RemoteStatus`]): 끊김 → 붙는 중(SSH · 설치 · 데몬 · 핸드셰이크) → 붙음 → 끊기면 다시 붙는 중(지수 백오프
//!   1 · 2 · 4 … 최대 30초 — 출발값 [`Backoff::default`]). **사람이 끊으면 다시 붙지 않는다**. 처음 붙기가 실패하면 저절로 다시
//!   해 보지 않는다(호스트 키 · 암호처럼 사람이 답할 것이 많다 — 사람이 [다시 시도]).
//! - **연결은 쓰는 머신만**(08 「창과 활성 체크아웃」 · D56): 링크는 그 호스트를 「쓰는」 것([`RemoteHosts::hold`] — RPC 로는
//!   `remote.connect` 스트림이 열린 동안)이 하나라도 있을 때만 연다. 모두 놓으면 링크를 닫는다.
//! - **머신 id**(메인 판정 M7-P6): 처음 붙을 때 본 Relay 기기 키 지문을 `state.db` 의 `remote_hosts` 에 기억하고 다음부터 그
//!   id 만 받는다. 다르면 [`RemoteStatus::MachineChanged`] 로 멈춘다 — 사람이 「이 머신을 새로 믿기」 를 고르면 묶음 멤버의 머신
//!   id 를 옮기고(`Registry::rename_bundle_machine`) 새 id 를 기억한다.
//! - **진단**: 단계 · 실패 · ssh stderr 의 끝부분(4 KiB, 제어 문자 제거)을 호스트마다 「연결 로그」 로 둔다. askpass 답은 남지
//!   않는다 — 답은 askpass 도우미로만 가고, 그래도 진단 글(ssh · 원격 셸이 찍은 글)에 섞여 오면 가린다.
//! - 붙으면 원격 Host(머신 id 가 Host id)를 `HostRouter` 에 더하고 묶음 판정의 원격 머신(`RemoteMachines`)으로 내놓는다.
//!
//! 실제 SSH · 설치 · 링크는 [`Backend`] 다 — 기능 `ssh` 의 [`ssh::SshBackend`](데스크톱 앱이 켠다).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use pw_core::registry::Registry;
use pw_core::registry::bundle::add::{BundleMachine, RemoteMachines};
use pw_core::registry::bundle::machine::HostMachine;
use pw_core::registry::remote_hosts::LinkMark;
use pw_core::{BoxFuture, EventBus, Host, HostError, HostRouter, Scope};
use pw_git::write::Prompts;
use serde_json::Value;
use tokio::sync::watch;
use zeroize::Zeroizing;

pub mod methods;
pub mod secrets;
#[cfg(feature = "ssh")]
pub mod ssh;
mod types;

pub use types::*;

/// 상태가 바뀔 때 내는 토픽(payload [`RemoteStatusChanged`]). 호스트 이름 · 머신 id 를 실으므로 owner 만.
pub const TOPIC_STATUS: &str = "remote.status";
pw_rpc::rpc_event!(TOPIC_STATUS, roles: [Owner]);

/// 그 호스트의 원격 Relay 조사(`remote.relays`)가 바뀔 때 내는 토픽(payload [`RemoteRelaysChanged`]). 호스트 이름만 싣지만 원격
/// 호스트 관리는 owner 의 것이라 owner 만.
pub const TOPIC_RELAYS: &str = "remote.relays";
pw_rpc::rpc_event!(TOPIC_RELAYS, roles: [Owner]);

/// 원격 askpass 질문을 듣는 열쇠(`pw_git::write::Prompts` — 원격 호스트의 질문은 모두 여기로).
pub const PROMPTS_KEY: &str = "remote";

/// 호스트마다 남기는 연결 로그 줄 수.
pub const LOG_LINES: usize = 200;
/// 로그 한 줄의 최대 바이트(ssh stderr 끝부분 4 KiB + 문장).
pub const LOG_LINE_BYTES: usize = 6 * 1024;

/// 다시 붙기의 기다림: `base * 2^(시도-1)`, `max` 에서 멈춘다.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub base: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    /// `attempt` 번째(1부터) 시도 전의 기다림.
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32
            .checked_shl(attempt.saturating_sub(1).min(16))
            .unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.max)
    }
}

/// 호스트 항목 하나(목록의 줄 — 붙을 때 다시 읽는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub source: RemoteHostSource,
    /// 사람의 ssh 옵션(전역 설정의 것만 — 거르기를 지난 것).
    pub ssh_args: Vec<String>,
}

/// 붙는 동안 [`Backend::connect`] 가 알리는 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Stage(RemoteStage),
    /// 연결 로그에 남길 한 줄(영어).
    Note(String),
}

/// 붙지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectError {
    pub stage: RemoteStage,
    pub kind: RemoteFailureKind,
    pub message: String,
    /// ssh stderr 끝부분 등 진단 원문(로그에만).
    pub detail: Option<String>,
    /// 기억한 머신 id 와 다른 id 가 답했다(기대, 본 것).
    pub machine_changed: Option<(String, String)>,
}

impl ConnectError {
    pub fn new(stage: RemoteStage, kind: RemoteFailureKind, message: impl Into<String>) -> Self {
        Self {
            stage,
            kind,
            message: message.into(),
            detail: None,
            machine_changed: None,
        }
    }
}

/// 선 링크.
pub struct Link {
    /// 원격 Host. id 는 머신 id 다.
    pub host: Arc<dyn Host>,
    /// 증명된 머신 id.
    pub machine: String,
    /// 링크가 닫히면 끝난다(까닭).
    pub closed: BoxFuture<'static, String>,
    /// 링크를 닫는다.
    pub close: Box<dyn FnOnce() + Send>,
}

/// 실제 SSH · 설치 · 링크([`ssh::SshBackend`]). 시험은 가짜를 넣는다.
pub trait Backend: Send + Sync {
    /// 전역 설정의 `remote_hosts` 값으로 호스트 목록과 경고.
    fn hosts(&self, settings: Option<&Value>) -> (Vec<HostEntry>, Vec<String>);
    /// 붙는다. `expect` 는 기억한 머신 id.
    fn connect<'a>(
        &'a self,
        entry: &'a HostEntry,
        expect: Option<&'a str>,
        progress: &'a (dyn Fn(Progress) + Send + Sync),
    ) -> BoxFuture<'a, Result<Link, ConnectError>>;
    /// 링크 없이 가벼운 확인(`ssh -O check`). 확인할 수 없으면 `None`.
    fn check<'a>(&'a self, entry: &'a HostEntry) -> BoxFuture<'a, Option<bool>>;
    /// 붙은 뒤 한 번(18-m7-plan M7-07b ①): 원격 Relay 조사 → 사람이 둔 `pitwall` 링크가 옛 판을 가리키면 이 판으로 옮김 → 정리(최근
    /// 두 판만 — 떠 있는 데몬의 바이너리는 남긴다). 판 바꾸기가 없는 백엔드는 아무것도 없다고 답한다.
    fn tidy<'a>(&'a self, _entry: &'a HostEntry) -> BoxFuture<'a, Result<Tidied, String>> {
        Box::pin(async { Ok(Tidied::default()) })
    }
    /// [옛 판 끝내기]: 사람이 본 그 데몬(`seen`)이 지금도 그대로일 때만 끝낸다(`pw_link::upgrade::Installer::stop_old`).
    fn stop_old<'a>(
        &'a self,
        _entry: &'a HostEntry,
        _seen: &'a RemoteOldRelay,
    ) -> BoxFuture<'a, Result<RemoteStopOutcome, String>> {
        Box::pin(async { Err("this backend cannot stop old relays".to_owned()) })
    }
    /// `pitwall` 명령 링크를 이 판 바이너리로 두거나(`on`) 걷는다. 그 뒤 링크가 있는가.
    fn link_command<'a>(
        &'a self,
        _entry: &'a HostEntry,
        _on: bool,
    ) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async { Err("this backend cannot link the pitwall command".to_owned()) })
    }
}

/// [`Backend::tidy`] 의 결과(머신 id 는 [`RemoteHosts`] 가 붙인다).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tidied {
    pub old: Vec<RemoteOldRelay>,
    pub silent: Vec<RemoteSilentRelay>,
    pub command: RemoteCommandLink,
    pub removed: Vec<String>,
    /// 연결 로그에 남길 줄(영어 — 옮긴 링크 · 남긴 바이너리 등).
    pub notes: Vec<String>,
}

/// `remote.*` 의 실패.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    /// 그런 이름의 호스트가 없다.
    UnknownHost(String),
    /// 원격 호스트를 쓸 수 없다(부품이 붙지 않음).
    Unavailable,
    /// 지금 상태에서 할 수 없다(사람이 본 것과 다르다 등).
    Conflict(String),
    /// 원격에서 하던 일이 실패했다(ssh · 원격 스크립트).
    Remote(String),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::UnknownHost(name) => write!(f, "no remote host named `{name}`"),
            RemoteError::Unavailable => f.write_str("remote hosts are not available"),
            RemoteError::Conflict(why) => f.write_str(why),
            RemoteError::Remote(why) => write!(f, "the remote host failed: {why}"),
        }
    }
}

/// Host 를 만든 뒤 붙이는 것(`pw_host::assemble`).
#[derive(Clone)]
struct HostParts {
    registry: Option<Registry>,
    bus: EventBus,
    settings: Option<pw_core::config::ConfigStore>,
}

struct Slot {
    status: watch::Sender<RemoteStatus>,
    uses: usize,
    by_user: bool,
    /// 붙기 한 바퀴마다 는다 — 버린 바퀴의 태스크가 상태를 덮지 않게.
    generation: u64,
    driver: Option<tokio::task::JoinHandle<()>>,
    /// 선 링크를 닫는 것과 그 머신 id.
    link: Option<(String, Box<dyn FnOnce() + Send>)>,
    log: VecDeque<RemoteLogEntry>,
    /// 지금 링크에서 본 원격 Relay(붙은 뒤 조사 — 링크가 닫히면 비운다).
    relays: Option<RemoteRelays>,
    /// 조사 · 끝내기 · 링크 바꾸기를 한 번에 하나씩(원격 셸이 서로의 결과를 덮지 않게).
    upgrade: Arc<tokio::sync::Mutex<()>>,
}

impl Slot {
    fn new() -> Self {
        Self {
            status: watch::Sender::new(RemoteStatus::idle()),
            uses: 0,
            by_user: false,
            generation: 0,
            driver: None,
            link: None,
            log: VecDeque::new(),
            relays: None,
            upgrade: Arc::default(),
        }
    }
}

struct Inner {
    slots: Mutex<HashMap<String, Slot>>,
    backend: RwLock<Option<Arc<dyn Backend>>>,
    router: RwLock<Option<Arc<HostRouter>>>,
    host: RwLock<Option<HostParts>>,
    prompts: Prompts,
    backoff: Backoff,
    /// 진단 글에서 가릴 askpass 답. 붙는 바퀴가 하나도 없으면 비운다.
    secrets: Mutex<Vec<Zeroizing<String>>>,
    /// 지금 붙기를 하는 바퀴 수.
    attempting: Mutex<usize>,
    /// 원격 Relay 의 비밀 청에 답하는 것(앱이 키체인으로 붙인다 — 18-m7-plan M7-12). 없으면 원격 Host 를 감싸지 않고 청에 답하지
    /// 않는다.
    desk: RwLock<Option<Arc<secrets::SecretDesk>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T: Clone>(m: &RwLock<Option<T>>) -> Option<T> {
    m.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// 진단에 보일 글: 줄바꿈만 남기고 제어 · 양방향 글자를 뺀다(`pw_link::ssh::clean_text` 와 같은 규칙), 길이를 자른다.
pub fn clean(raw: &str) -> String {
    let mut text: String = raw
        .chars()
        .filter_map(|c| match c {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            c if c.is_control() => None,
            '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => None,
            c => Some(c),
        })
        .collect();
    if text.len() > LOG_LINE_BYTES {
        let mut cut = text.len() - LOG_LINE_BYTES;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text = format!("…{}", &text[cut..]);
    }
    text
}

/// 원격 호스트 관리자. 복제하면 같은 것을 가리킨다.
#[derive(Clone)]
pub struct RemoteHosts {
    inner: Arc<Inner>,
}

impl Default for RemoteHosts {
    fn default() -> Self {
        Self::new(Backoff::default())
    }
}

/// 그 호스트를 쓰는 것. 버리면 놓는다 — 마지막 것을 놓으면 링크를 닫는다.
pub struct Use {
    hosts: RemoteHosts,
    name: String,
}

impl Drop for Use {
    fn drop(&mut self) {
        self.hosts.release(&self.name);
    }
}

impl RemoteHosts {
    pub fn new(backoff: Backoff) -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: Mutex::default(),
                backend: RwLock::default(),
                router: RwLock::default(),
                host: RwLock::default(),
                prompts: Prompts::default(),
                backoff,
                secrets: Mutex::default(),
                attempting: Mutex::new(0),
                desk: RwLock::default(),
            }),
        }
    }

    /// Host 를 만든 뒤 붙인다(`pw_host::assemble`): 레지스트리(머신 id 기억 · 묶음의 원격 머신), 버스(`remote.status`), 전역 설정
    /// (`[[remote_hosts]]`).
    pub fn attach_host(
        &self,
        registry: Option<Registry>,
        bus: EventBus,
        settings: Option<pw_core::config::ConfigStore>,
    ) {
        if let Some(registry) = &registry {
            registry.set_remote_machines(Arc::new(Machines(self.clone())));
        }
        *self
            .inner
            .host
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(HostParts {
            registry,
            bus,
            settings,
        });
    }

    /// 실제 SSH · 설치 · 링크(데스크톱 앱이 setup 에서 붙인다). 붙이기 전에는 목록이 비고 `remote.connect` 는 쓸 수 없다.
    pub fn attach_backend(&self, backend: Arc<dyn Backend>) {
        *self
            .inner
            .backend
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(backend);
    }

    /// 붙은 원격 Host 를 더할 라우터(앱의 `HostRouter`).
    pub fn attach_router(&self, router: Arc<HostRouter>) {
        *self
            .inner
            .router
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(router);
    }

    /// 원격 Relay 의 비밀 청에 답할 저장소(앱은 키체인 — 18-m7-plan M7-12). 붙인 뒤에 선 링크부터 원격 Host 를 감싸고
    /// (`secrets::SecretDesk::gate`) 그 Relay 의 청에 답한다.
    pub fn attach_secrets(&self, store: Arc<dyn pw_db::secret::SecretStore>) {
        *self
            .inner
            .desk
            .write()
            .unwrap_or_else(PoisonError::into_inner) =
            Some(Arc::new(secrets::SecretDesk::new(store)));
    }

    /// 원격 askpass 질문(열쇠 [`PROMPTS_KEY`]). ssh 의 askpass 세션이 이것으로 묻는다.
    pub fn prompts(&self) -> &Prompts {
        &self.inner.prompts
    }

    fn backend(&self) -> Option<Arc<dyn Backend>> {
        read(&self.inner.backend)
    }

    fn registry(&self) -> Option<Registry> {
        read(&self.inner.host).and_then(|h| h.registry)
    }

    fn entries(&self) -> (Vec<HostEntry>, Vec<String>) {
        let Some(backend) = self.backend() else {
            return (
                Vec::new(),
                vec!["remote hosts are not available in this process".into()],
            );
        };
        let snapshot = read(&self.inner.host)
            .and_then(|h| h.settings)
            .map(|s| s.snapshot());
        let settings = snapshot
            .as_ref()
            .and_then(|s| s.get("settings", pw_link_key()));
        backend.hosts(settings)
    }

    fn entry(&self, name: &str) -> Result<HostEntry, RemoteError> {
        if self.backend().is_none() {
            return Err(RemoteError::Unavailable);
        }
        self.entries()
            .0
            .into_iter()
            .find(|e| e.name == name)
            .ok_or_else(|| RemoteError::UnknownHost(name.to_owned()))
    }

    fn last_link(&self, name: &str) -> (Option<String>, Option<RemoteLastLink>) {
        let record = self
            .registry()
            .and_then(|r| r.remote_host(name).ok().flatten());
        match record {
            Some(r) => (
                r.machine,
                r.last.map(|(mark, at)| RemoteLastLink {
                    state: match mark {
                        LinkMark::Connected => RemoteLastLinkState::Connected,
                        LinkMark::Disconnected => RemoteLastLinkState::Disconnected,
                        LinkMark::Failed => RemoteLastLinkState::Failed,
                    },
                    at,
                }),
            ),
            None => (None, None),
        }
    }

    /// 지금 상태(아직 쓴 적 없으면 끊김).
    pub fn status(&self, name: &str) -> RemoteStatus {
        lock(&self.inner.slots)
            .get(name)
            .map_or_else(RemoteStatus::idle, |s| s.status.borrow().clone())
    }

    /// 상태를 따라간다.
    pub fn watch(&self, name: &str) -> watch::Receiver<RemoteStatus> {
        lock(&self.inner.slots)
            .entry(name.to_owned())
            .or_insert_with(Slot::new)
            .status
            .subscribe()
    }

    /// 목록(`remote.hosts.list`).
    pub fn list(&self) -> RemoteHostList {
        let (entries, warnings) = self.entries();
        let hosts = entries
            .into_iter()
            .map(|e| {
                let (machine, last_link) = self.last_link(&e.name);
                RemoteHostInfo {
                    status: self.status(&e.name),
                    name: e.name,
                    host: e.host,
                    user: e.user,
                    port: e.port,
                    source: e.source,
                    machine,
                    last_link,
                }
            })
            .collect();
        RemoteHostList { hosts, warnings }
    }

    /// 가벼운 상태 확인(`remote.hosts.check`) — 링크를 열지 않는다.
    pub async fn check(&self, name: &str) -> Result<RemoteHostCheck, RemoteError> {
        let entry = self.entry(name)?;
        let backend = self.backend().ok_or(RemoteError::Unavailable)?;
        let master = backend.check(&entry).await;
        Ok(RemoteHostCheck {
            master,
            last_link: self.last_link(name).1,
        })
    }

    /// 연결 로그(`remote.log`).
    pub fn log(&self, name: &str) -> Vec<RemoteLogEntry> {
        lock(&self.inner.slots)
            .get(name)
            .map(|s| s.log.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 그 호스트를 쓰기 시작한다. 링크가 없으면 붙는다. 사람이 끊어 둔 호스트도 다시 붙는다(새로 쓰겠다는 것이 사람의 뜻이다).
    pub fn hold(&self, name: &str) -> Result<Use, RemoteError> {
        self.entry(name)?;
        let mut slots = lock(&self.inner.slots);
        let slot = slots.entry(name.to_owned()).or_insert_with(Slot::new);
        slot.uses += 1;
        let idle = slot.driver.as_ref().is_none_or(|d| d.is_finished()) && slot.link.is_none();
        let stuck = matches!(*slot.status.borrow(), RemoteStatus::MachineChanged { .. });
        if (idle || slot.by_user) && !stuck {
            slot.by_user = false;
            self.start(name, slot);
        }
        drop(slots);
        Ok(Use {
            hosts: self.clone(),
            name: name.to_owned(),
        })
    }

    fn release(&self, name: &str) {
        let mut slots = lock(&self.inner.slots);
        let Some(slot) = slots.get_mut(name) else {
            return;
        };
        slot.uses = slot.uses.saturating_sub(1);
        if slot.uses > 0 {
            return;
        }
        let had_link = self.stop(name, slot);
        let keep = matches!(
            &*slot.status.borrow(),
            RemoteStatus::MachineChanged { .. } | RemoteStatus::Disconnected { .. }
        );
        if !keep {
            self.set_status(name, slot, RemoteStatus::idle());
        }
        drop(slots);
        if had_link {
            self.note(
                name,
                None,
                RemoteLogLevel::Info,
                "no window uses this host; closed the link",
            );
            self.mark(name, LinkMark::Disconnected);
        }
    }

    /// 사람이 끊는다(`remote.disconnect`). 쓰는 것이 남아 있어도 다시 붙지 않는다 — 새 `remote.connect` · [`Self::retry`] 까지.
    pub fn disconnect(&self, name: &str) -> Result<(), RemoteError> {
        self.entry(name)?;
        let mut slots = lock(&self.inner.slots);
        let slot = slots.entry(name.to_owned()).or_insert_with(Slot::new);
        slot.by_user = true;
        let had_link = self.stop(name, slot);
        self.set_status(
            name,
            slot,
            RemoteStatus::Disconnected {
                by_user: true,
                error: None,
            },
        );
        drop(slots);
        self.note(name, None, RemoteLogLevel::Info, "disconnected by you");
        if had_link {
            self.mark(name, LinkMark::Disconnected);
        }
        Ok(())
    }

    /// 다시 시도(`remote.retry`): 쓰는 것이 있으면 기다림 없이 지금 붙는다. 사람이 끊어 둔 것도 푼다.
    pub fn retry(&self, name: &str) -> Result<(), RemoteError> {
        self.entry(name)?;
        let mut slots = lock(&self.inner.slots);
        let slot = slots.entry(name.to_owned()).or_insert_with(Slot::new);
        if matches!(*slot.status.borrow(), RemoteStatus::MachineChanged { .. }) {
            return Err(RemoteError::Conflict(
                "this host answered with another machine; trust the new machine first".into(),
            ));
        }
        slot.by_user = false;
        if slot.uses == 0 {
            return Err(RemoteError::Conflict(
                "no window uses this host; connect it first".into(),
            ));
        }
        if slot.link.is_some() {
            return Ok(());
        }
        self.stop(name, slot);
        self.start(name, slot);
        Ok(())
    }

    /// 「이 머신을 새로 믿기」(`remote.trustMachine` — 메인 판정 M7-P6). 지금 상태가 `machineChanged` 이고 그 `found` 가
    /// `machine` 일 때만: 묶음 멤버의 머신 id 를 옮기고 새 id 를 기억한 뒤, 쓰는 것이 있으면 다시 붙는다(ssh 호스트 키 확인을
    /// 다시 지난다).
    pub fn trust_machine(&self, name: &str, machine: &str) -> Result<(), RemoteError> {
        self.entry(name)?;
        let expected = match self.status(name) {
            RemoteStatus::MachineChanged { expected, found } if found == machine => expected,
            _ => {
                return Err(RemoteError::Conflict(
                    "this host is not waiting for a new machine id (or another id came since)"
                        .into(),
                ));
            }
        };
        let registry = self.registry().ok_or(RemoteError::Unavailable)?;
        // 검사(기억한 id 가 `expected` 인가 — 리뷰 B1, `machine` · `expected` 를 다른 호스트 이름이 기억하는가 — F1)와 쓰기(묶음
        // 멤버 옮기기 · 새 id 기억)는 레지스트리의 한 트랜잭션이다(F-b). `machine` 은 핸드셰이크가 서명으로 증명한 키의 지문이다(F-a).
        registry
            .trust_remote_machine(name, &expected, machine)
            .map_err(|e| RemoteError::Conflict(e.to_string()))?;
        // 옛 id 의 키체인 비밀은 지운다(M7-P6 — 새 키의 머신이 옛 머신의 비밀을 물려받지 않는다).
        if let Some(desk) = read(&self.inner.desk) {
            desk.forget_machine(&expected);
        }
        self.note(
            name,
            None,
            RemoteLogLevel::Info,
            &format!("trusted the new machine {machine} (was {expected})"),
        );
        let mut slots = lock(&self.inner.slots);
        let slot = slots.entry(name.to_owned()).or_insert_with(Slot::new);
        self.set_status(name, slot, RemoteStatus::idle());
        if slot.uses > 0 {
            self.start(name, slot);
        }
        Ok(())
    }

    /// askpass 답을 진단 글에서 가릴 것으로 둔다(`remote.prompt.answer`). 호스트 키 확인의 `yes` · `no` 는 비밀이 아니다.
    pub fn remember_secret(&self, value: &str) {
        if value.is_empty() || value == "yes" || value == "no" {
            return;
        }
        lock(&self.inner.secrets).push(Zeroizing::new(value.to_owned()));
    }

    fn redact(&self, text: &str) -> String {
        let secrets = lock(&self.inner.secrets);
        let mut out = text.to_owned();
        for secret in secrets.iter() {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "[hidden]");
            }
        }
        out
    }

    fn note(&self, name: &str, stage: Option<RemoteStage>, level: RemoteLogLevel, text: &str) {
        let text = clean(&self.redact(text));
        let mut slots = lock(&self.inner.slots);
        let slot = slots.entry(name.to_owned()).or_insert_with(Slot::new);
        if slot.log.len() == LOG_LINES {
            slot.log.pop_front();
        }
        slot.log.push_back(RemoteLogEntry {
            at: now_ms(),
            stage,
            level,
            text,
        });
    }

    fn mark(&self, name: &str, mark: LinkMark) {
        if let Some(registry) = self.registry()
            && let Err(e) = registry.record_remote_link(name, mark)
        {
            tracing::warn!(target: "pw_remote", host = name, %e, "cannot record the remote link state");
        }
    }

    fn set_status(&self, name: &str, slot: &mut Slot, status: RemoteStatus) {
        if *slot.status.borrow() == status {
            return;
        }
        slot.status.send_replace(status.clone());
        if let Some(host) = read(&self.inner.host) {
            let _ = host.bus.publish(
                Scope::Host,
                TOPIC_STATUS,
                &RemoteStatusChanged {
                    name: name.to_owned(),
                    status,
                },
            );
        }
    }

    /// 돌던 바퀴를 멈추고 링크를 닫는다. 링크가 있었으면 참.
    fn stop(&self, name: &str, slot: &mut Slot) -> bool {
        slot.generation += 1;
        if let Some(driver) = slot.driver.take() {
            driver.abort();
        }
        self.set_relays(name, slot, None);
        match slot.link.take() {
            Some((machine, close)) => {
                close();
                if let Some(router) = read(&self.inner.router) {
                    router.remove_host(&machine);
                }
                true
            }
            None => false,
        }
    }

    fn start(&self, name: &str, slot: &mut Slot) {
        slot.generation += 1;
        if let Some(driver) = slot.driver.take() {
            driver.abort();
        }
        let generation = slot.generation;
        self.set_status(
            name,
            slot,
            RemoteStatus::Connecting {
                stage: RemoteStage::Ssh,
            },
        );
        let this = self.clone();
        let name = name.to_owned();
        slot.driver = Some(tokio::spawn(async move {
            this.drive(name, generation).await;
        }));
    }

    /// 이 바퀴가 아직 살아 있으면 상태를 바꾼다.
    fn update(
        &self,
        name: &str,
        generation: u64,
        f: impl FnOnce(&mut Slot) -> Option<RemoteStatus>,
    ) -> bool {
        let mut slots = lock(&self.inner.slots);
        let Some(slot) = slots.get_mut(name) else {
            return false;
        };
        if slot.generation != generation {
            return false;
        }
        if let Some(status) = f(slot) {
            self.set_status(name, slot, status);
        }
        true
    }

    async fn drive(self, name: String, generation: u64) {
        let mut attempt: u32 = 0;
        loop {
            let outcome = self.attempt(&name, generation).await;
            let Some(outcome) = outcome else {
                return;
            };
            match outcome {
                Ok(closed) => {
                    // 붙었다가 닫혔다 — 다시 붙기로.
                    let alive = self.update(&name, generation, |slot| {
                        if let Some((machine, _)) = slot.link.take()
                            && let Some(router) = read(&self.inner.router)
                        {
                            router.remove_host(&machine);
                        }
                        self.set_relays(&name, slot, None);
                        None
                    });
                    if !alive {
                        return;
                    }
                    self.note(
                        &name,
                        None,
                        RemoteLogLevel::Error,
                        &format!("the link closed: {closed}"),
                    );
                    self.mark(&name, LinkMark::Disconnected);
                    attempt = 1;
                    let error = RemoteFailure {
                        stage: RemoteStage::Handshake,
                        kind: RemoteFailureKind::Link,
                        message: format!("the link closed: {closed}"),
                    };
                    if !self.wait(&name, generation, attempt, error).await {
                        return;
                    }
                }
                Err(e) => {
                    self.mark(&name, LinkMark::Failed);
                    let failure = RemoteFailure {
                        stage: e.stage,
                        kind: e.kind,
                        message: clean(&self.redact(&e.message)),
                    };
                    if let Some((expected, found)) = e.machine_changed {
                        self.update(&name, generation, |_| {
                            Some(RemoteStatus::MachineChanged { expected, found })
                        });
                        return;
                    }
                    if attempt == 0 || !e.kind.retryable() {
                        self.update(&name, generation, |slot| {
                            slot.driver = None;
                            Some(RemoteStatus::Disconnected {
                                by_user: false,
                                error: Some(failure),
                            })
                        });
                        return;
                    }
                    attempt += 1;
                    if !self.wait(&name, generation, attempt, failure).await {
                        return;
                    }
                }
            }
        }
    }

    /// 다시 붙기 전의 기다림. 바퀴가 버려졌으면 거짓.
    async fn wait(&self, name: &str, generation: u64, attempt: u32, error: RemoteFailure) -> bool {
        let delay = self.inner.backoff.delay(attempt);
        let alive = self.update(name, generation, |_| {
            Some(RemoteStatus::Reconnecting {
                attempt,
                retry_in_ms: delay.as_millis() as u64,
                error,
            })
        });
        if !alive {
            return false;
        }
        self.note(
            name,
            None,
            RemoteLogLevel::Info,
            &format!(
                "reconnecting in {} s (attempt {attempt})",
                delay.as_secs_f32()
            ),
        );
        tokio::time::sleep(delay).await;
        true
    }

    /// 붙기 한 번. 붙었으면 링크가 닫힐 때까지 기다려 그 까닭을, 못 붙었으면 오류. 바퀴가 버려졌으면 `None`.
    async fn attempt(&self, name: &str, generation: u64) -> Option<Result<String, ConnectError>> {
        let entry = match self.entry(name) {
            Ok(entry) => entry,
            Err(e) => {
                return Some(Err(ConnectError::new(
                    RemoteStage::Ssh,
                    RemoteFailureKind::Unavailable,
                    e.to_string(),
                )));
            }
        };
        let backend = self.backend()?;
        let expect = self
            .registry()
            .and_then(|r| r.remote_host(name).ok().flatten())
            .and_then(|r| r.machine);
        self.note(
            name,
            Some(RemoteStage::Ssh),
            RemoteLogLevel::Info,
            &format!("connecting to {}", describe(&entry)),
        );
        let _attempting = Attempting::new(self);
        let progress = |p: Progress| match p {
            Progress::Stage(stage) => {
                self.update(name, generation, |_| {
                    Some(RemoteStatus::Connecting { stage })
                });
                self.note(name, Some(stage), RemoteLogLevel::Info, stage_line(stage));
            }
            Progress::Note(text) => {
                let stage = match self.status(name) {
                    RemoteStatus::Connecting { stage } => Some(stage),
                    _ => None,
                };
                self.note(name, stage, RemoteLogLevel::Info, &text);
            }
        };
        let result = backend.connect(&entry, expect.as_deref(), &progress).await;
        let result = match result {
            Ok(link) => self.connected(name, generation, expect.is_none(), link),
            Err(e) => {
                let mut text = format!("{:?} failed: {}", e.stage, e.message);
                if let Some(detail) = &e.detail {
                    text.push('\n');
                    text.push_str(detail);
                }
                self.note(name, Some(e.stage), RemoteLogLevel::Error, &text);
                Err(e)
            }
        };
        drop(_attempting);
        match result {
            Ok(Some((closed, machine))) => {
                // 붙은 뒤 한 번 — 조사 · 링크 옮기기 · 정리(M7-07b ①). 링크가 먼저 닫히면 조사를 버린다(ssh 도 끝난다).
                let mut closed = closed;
                let tidy = self.tidy(name, generation, &backend, &entry, &machine);
                tokio::select! {
                    reason = &mut closed => Some(Ok(reason)),
                    () = tidy => Some(Ok(closed.await)),
                }
            }
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }

    /// 선 링크를 라우터에 더하고 상태를 바꾼다. 바퀴가 버려졌으면 링크를 닫고 `Ok(None)`.
    fn connected(
        &self,
        name: &str,
        generation: u64,
        first: bool,
        link: Link,
    ) -> Result<Option<(BoxFuture<'static, String>, String)>, ConnectError> {
        let Link {
            host,
            machine,
            closed,
            close,
        } = link;
        // 처음 본 id 는 검사(다른 호스트 이름이 이미 그 id 를 기억하는가)와 같은 트랜잭션으로 기억한다(리뷰 F-b). 다른 이름이
        // 가진 머신이면 이 링크를 쓰지 않는다 — 한 머신의 묶음 멤버가 두 이름으로 갈라지지 않게.
        if first
            && let Some(registry) = self.registry()
            && let Err(e) = registry.remember_first_remote_machine(name, &machine)
        {
            close();
            let e = ConnectError::new(
                RemoteStage::Handshake,
                RemoteFailureKind::AlreadyConnected,
                e.to_string(),
            );
            self.note(
                name,
                Some(RemoteStage::Handshake),
                RemoteLogLevel::Error,
                &e.message,
            );
            return Err(e);
        }
        // 비밀 청에 답할 것이 있으면 원격 Host 를 감싸 그 머신 창 연결의 DB 요청을 지켜보고, 링크가 선 뒤 그 Relay 의 청에 답한다
        // (18-m7-plan M7-12 — 열쇠의 머신 id 는 핸드셰이크가 증명한 `machine`).
        let desk = read(&self.inner.desk);
        let (host, answering) = match &desk {
            Some(_) => {
                let (gated, flights) = secrets::SecretDesk::gate(Arc::clone(&host));
                (gated, Some((host, flights)))
            }
            None => (host, None),
        };
        if let Some(router) = read(&self.inner.router)
            && let Err(HostError::DuplicateHost(_)) = router.add_host(host)
        {
            close();
            let e = ConnectError::new(
                RemoteStage::Handshake,
                RemoteFailureKind::AlreadyConnected,
                format!("machine {machine} is already connected under another host name"),
            );
            self.note(
                name,
                Some(RemoteStage::Handshake),
                RemoteLogLevel::Error,
                &e.message,
            );
            return Err(e);
        }
        let mut close = Some(close);
        let alive = self.update(name, generation, |slot| {
            slot.link = Some((machine.clone(), close.take().expect("한 번")));
            Some(RemoteStatus::Connected {
                machine: machine.clone(),
            })
        });
        if !alive {
            if let Some(close) = close {
                close();
            }
            if let Some(router) = read(&self.inner.router) {
                router.remove_host(&machine);
            }
            return Ok(None);
        }
        if let (Some(desk), Some((inner, flights))) = (desk, answering) {
            tokio::spawn(desk.serve(machine.clone(), inner, flights));
        }
        self.mark(name, LinkMark::Connected);
        self.note(
            name,
            Some(RemoteStage::Handshake),
            RemoteLogLevel::Info,
            &format!("connected to machine {machine}"),
        );
        Ok(Some((closed, machine)))
    }

    /// 조사 결과를 바꾸고 알린다(`remote.relays`).
    fn set_relays(&self, name: &str, slot: &mut Slot, relays: Option<RemoteRelays>) {
        if slot.relays == relays {
            return;
        }
        slot.relays = relays;
        if let Some(host) = read(&self.inner.host) {
            let _ = host.bus.publish(
                Scope::Host,
                TOPIC_RELAYS,
                &RemoteRelaysChanged {
                    name: name.to_owned(),
                },
            );
        }
    }

    /// 지금 링크에서 본 원격 Relay(`remote.relays`). 링크가 없거나 아직 보지 않았으면 `None`.
    pub fn relays(&self, name: &str) -> Option<RemoteRelays> {
        lock(&self.inner.slots)
            .get(name)
            .and_then(|s| s.relays.clone())
    }

    /// 그 호스트가 지금 `machine` 에 붙어 있으면 그 바퀴와 원격 일의 자물쇠. 사람이 본 머신과 다르면 거절한다 — 그사이 다른 머신에
    /// 붙었거나 끊겼으면 그 머신의 Relay 를 끝내거나 그 머신에 쓰지 않는다.
    fn linked_to(
        &self,
        name: &str,
        machine: &str,
    ) -> Result<(u64, Arc<tokio::sync::Mutex<()>>), RemoteError> {
        let slots = lock(&self.inner.slots);
        let slot = slots.get(name);
        match slot {
            Some(slot)
                if slot.link.as_ref().is_some_and(|(m, _)| m == machine)
                    && slot.relays.as_ref().is_some_and(|r| r.machine == machine) =>
            {
                Ok((slot.generation, Arc::clone(&slot.upgrade)))
            }
            _ => Err(RemoteError::Conflict(format!(
                "`{name}` is not connected to machine {machine} now"
            ))),
        }
    }

    /// 붙은 뒤 한 번: 조사 · 링크 옮기기 · 정리(M7-07b ①).
    async fn tidy(
        &self,
        name: &str,
        generation: u64,
        backend: &Arc<dyn Backend>,
        entry: &HostEntry,
        machine: &str,
    ) {
        let gate = {
            let slots = lock(&self.inner.slots);
            match slots.get(name) {
                Some(slot) if slot.generation == generation => Arc::clone(&slot.upgrade),
                _ => return,
            }
        };
        let _one = gate.lock().await;
        self.refresh(name, generation, backend, entry, machine)
            .await;
    }

    /// 조사 · 정리를 돌려 `remote.relays` 를 바꾼다(자물쇠를 쥔 쪽이 부른다). 그사이 바퀴가 바뀌었으면 결과를 버린다.
    async fn refresh(
        &self,
        name: &str,
        generation: u64,
        backend: &Arc<dyn Backend>,
        entry: &HostEntry,
        machine: &str,
    ) {
        match backend.tidy(entry).await {
            Ok(tidied) => {
                for line in &tidied.notes {
                    self.note(name, None, RemoteLogLevel::Info, line);
                }
                if !tidied.removed.is_empty() {
                    self.note(
                        name,
                        None,
                        RemoteLogLevel::Info,
                        &format!("removed old relay versions: {}", tidied.removed.join(", ")),
                    );
                }
                for old in &tidied.old {
                    self.note(
                        name,
                        None,
                        RemoteLogLevel::Info,
                        &format!(
                            "the old relay {} (pid {}) still has {} runs, {} terminals and {} links",
                            old.file, old.pid, old.runs, old.terminals, old.links
                        ),
                    );
                }
                for silent in &tidied.silent {
                    self.note(
                        name,
                        None,
                        RemoteLogLevel::Info,
                        &format!("the old relay {} does not answer; kept it", silent.file),
                    );
                }
                let relays = RemoteRelays {
                    machine: machine.to_owned(),
                    old: tidied.old,
                    silent: tidied.silent,
                    command: tidied.command,
                    removed: tidied.removed,
                };
                self.update(name, generation, |slot| {
                    if slot.link.as_ref().is_some_and(|(m, _)| m == machine) {
                        self.set_relays(name, slot, Some(relays));
                    }
                    None
                });
            }
            Err(e) => self.note(
                name,
                None,
                RemoteLogLevel::Error,
                &format!("could not look at the old relay versions: {e}"),
            ),
        }
    }

    /// [옛 판 끝내기](`remote.stopOld` — M7-07b ③). 그 호스트가 지금 `machine`(사람이 본 머신)에 붙어 있고 `seen` 이 보인 옛 판
    /// 하나일 때만 백엔드에 넘긴다 — 백엔드는 원격을 다시 보고, 사람이 본 pid · 실행 · 터미널 · 링크 수와 다르면 끝내지 않는다. 끝난
    /// 뒤 다시 조사한다(지울 수 있게 된 바이너리를 지우고 새 수를 보인다).
    pub async fn stop_old(
        &self,
        name: &str,
        machine: &str,
        seen: &RemoteOldRelay,
    ) -> Result<RemoteStopOutcome, RemoteError> {
        let entry = self.entry(name)?;
        let backend = self.backend().ok_or(RemoteError::Unavailable)?;
        let (generation, gate) = self.linked_to(name, machine)?;
        let _one = gate.lock().await;
        let shown = self
            .relays(name)
            .is_some_and(|r| r.machine == machine && r.old.iter().any(|o| o.file == seen.file));
        if self.linked_to(name, machine)?.0 != generation || !shown {
            return Err(RemoteError::Conflict(
                "that old relay is not among what this host shows now".into(),
            ));
        }
        self.note(
            name,
            None,
            RemoteLogLevel::Info,
            &format!(
                "stopping the old relay {} as you saw it (pid {}, {} runs, {} terminals, {} links)",
                seen.file, seen.pid, seen.runs, seen.terminals, seen.links
            ),
        );
        let outcome = match backend.stop_old(&entry, seen).await {
            Ok(outcome) => outcome,
            Err(e) => {
                self.note(
                    name,
                    None,
                    RemoteLogLevel::Error,
                    &format!("could not stop the old relay {}: {e}", seen.file),
                );
                return Err(RemoteError::Remote(e));
            }
        };
        let said = match &outcome {
            RemoteStopOutcome::Stopped => "stopped it".to_owned(),
            RemoteStopOutcome::Stopping => "sent the stop; it is still ending its runs".to_owned(),
            RemoteStopOutcome::Gone => "it was already gone".to_owned(),
            RemoteStopOutcome::NoAnswer => "it does not answer; did not stop it".to_owned(),
            RemoteStopOutcome::Changed { now } => format!(
                "it changed since you saw it (now pid {}, {} runs, {} terminals, {} links); did not stop it",
                now.pid, now.runs, now.terminals, now.links
            ),
        };
        self.note(
            name,
            None,
            RemoteLogLevel::Info,
            &format!("old relay {}: {said}", seen.file),
        );
        self.refresh(name, generation, &backend, &entry, machine)
            .await;
        Ok(outcome)
    }

    /// `pitwall` 명령 링크 켜기 · 끄기(`remote.linkCommand` — 메인 판정 M7-P12, 사람이 고를 때만). 그 호스트가 지금 `machine` 에
    /// 붙어 있을 때만. 끝난 뒤 다시 조사한다.
    pub async fn link_command(
        &self,
        name: &str,
        machine: &str,
        on: bool,
    ) -> Result<(), RemoteError> {
        let entry = self.entry(name)?;
        let backend = self.backend().ok_or(RemoteError::Unavailable)?;
        let (generation, gate) = self.linked_to(name, machine)?;
        let _one = gate.lock().await;
        if self.linked_to(name, machine)?.0 != generation {
            return Err(RemoteError::Conflict(format!(
                "`{name}` reconnected; look again"
            )));
        }
        let result = backend.link_command(&entry, on).await;
        match &result {
            Ok(linked) => self.note(
                name,
                None,
                RemoteLogLevel::Info,
                if *linked {
                    "linked ~/.local/bin/pitwall to this relay version"
                } else {
                    "removed the ~/.local/bin/pitwall link"
                },
            ),
            Err(e) => self.note(
                name,
                None,
                RemoteLogLevel::Error,
                &format!("could not change the ~/.local/bin/pitwall link: {e}"),
            ),
        }
        self.refresh(name, generation, &backend, &entry, machine)
            .await;
        result.map(|_| ()).map_err(RemoteError::Remote)
    }
}

fn describe(entry: &HostEntry) -> String {
    let mut text = format!("{} (ssh ", entry.name);
    if let Some(user) = &entry.user {
        text.push_str(user);
        text.push('@');
    }
    text.push_str(&entry.host);
    if let Some(port) = entry.port {
        text.push_str(&format!(":{port}"));
    }
    text.push(')');
    text
}

fn stage_line(stage: RemoteStage) -> &'static str {
    match stage {
        RemoteStage::Ssh => "ssh: connecting and probing the remote platform",
        RemoteStage::Install => "install: checking the relay of this version",
        RemoteStage::Daemon => "daemon: starting the relay proxy",
        RemoteStage::Handshake => "handshake: proving this device and the machine id",
    }
}

/// 붙기 한 바퀴. 떨어질 때(끝남 · 바퀴를 버림 — 끊기 · 놓기 · 다시 시도) 세기를 내리고, 붙는 바퀴가 없으면 가릴 askpass 답을
/// 메모리에서 지운다(리뷰 F2 — 버린 바퀴가 세기를 남기면 답이 앱이 끝날 때까지 남는다).
struct Attempting(Arc<Inner>);

impl Attempting {
    fn new(hosts: &RemoteHosts) -> Self {
        *lock(&hosts.inner.attempting) += 1;
        Self(Arc::clone(&hosts.inner))
    }
}

impl Drop for Attempting {
    fn drop(&mut self) {
        let mut attempting = lock(&self.0.attempting);
        *attempting = attempting.saturating_sub(1);
        if *attempting == 0 {
            lock(&self.0.secrets).clear();
        }
    }
}

/// 설정의 키(`[[remote_hosts]]`).
fn pw_link_key() -> &'static str {
    "remote_hosts"
}

/// 묶음 판정의 원격 머신: 붙어 있는 원격 Host 를 그 RPC(`machine.*`)로 묻는다.
struct Machines(RemoteHosts);

impl RemoteMachines for Machines {
    fn machine(&self, id: &str) -> Option<Arc<dyn BundleMachine>> {
        let router = read(&self.0.inner.router)?;
        let host = router.host(id).ok()?;
        host.is_remote()
            .then(|| Arc::new(HostMachine::new(id, host)) as Arc<dyn BundleMachine>)
    }
}

#[cfg(test)]
mod tests;
