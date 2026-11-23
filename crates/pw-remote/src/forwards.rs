//! 포트 포워딩의 로컬 쪽(18-m7-plan M7-22 — F7-11 · F9-6). 이 컴퓨터의 루프백 포트에서 듣고, 받은 TCP 연결 하나마다 그 호스트의
//! 지금 링크로 채널 하나를 연다(`pw_link::forward` — Relay 가 그 머신에서 `remote_host:remote_port` 로 잇는다).
//!
//! - **두 갈래**: 원격 호스트 설정의 `[[remote_hosts.port_forwards]]` 는 그 호스트가 붙을 때 연다(붙어 있는 동안 — 끊기 · 쓰는 창이
//!   모두 닫힘에서 닫는다). 실행 탭 포트의 「로컬로 포워딩」 은 `remote.forward.open` 스트림이 열린 동안이다 — 창이 그 실행이 끝나면
//!   스트림을 닫는다. 실행 탭의 포워딩은 원격 쪽이 늘 `localhost` 다: 원격 머신을 경유지로 쓰는 것(다른 `remote_host`)은 사람의
//!   설정일 때만이다(메인 판정 M7-P15).
//! - **로컬은 루프백에만**(결정 M7-D12): 설정의 `local_host` 가 루프백이 아니면 그 포워딩은 열지 않고 목록에 까닭(`notLoopback`)을
//!   보이며 연결 로그에 경고를 남긴다. 듣기 전에 [`pw_link::forward::local_addr`] 와 [`pw_link::forward::bind`] 가 두 번 본다.
//! - 로컬 포트가 이미 쓰이면 설정의 포워딩은 `portInUse` 로 목록에 남고, 실행 탭의 포워딩은 그 번호를 실은 오류로 끝난다 — 창이 다른
//!   포트를 고르게 한다.
//! - 링크가 이어 붙는 동안(M7-09)은 채널이 그대로라 연결도 산다. 링크가 끊겨 다시 붙는 동안 들어온 연결은 곧 닫는다(듣기는 남는다).
//! - **한 호스트에 동시에 잇는 연결은 [`MAX_CONNECTIONS`] 까지** — 루프백 포트는 이 컴퓨터의 어느 프로세스든 열 수 있으므로, 연결을
//!   몰아 열어 링크의 채널 상한(창 · 셸 · LSP 의 채널과 같이 쓴다)을 채우지 못하게. 넘는 연결은 곧 닫는다.
//! - MCP 툴이 없다(메인 판정 M7-P15). RPC 는 owner · Host 범위 연결만이다(`methods.rs`).

use std::collections::HashMap;
use std::sync::Arc;

use pw_core::{BoxFuture, Scope};
use pw_link::forward::{self, DEFAULT_REMOTE_HOST, ForwardError, ForwardSetting, ForwardTo};
use tokio::net::TcpStream;
use tokio::sync::{Semaphore, watch};

use crate::{
    RemoteError, RemoteForward, RemoteForwardFailure, RemoteForwardFailureKind,
    RemoteForwardOrigin, RemoteForwardState, RemoteForwardsChanged, RemoteHosts, RemoteLogLevel,
    lock, read,
};

/// 포워딩 목록이 바뀔 때 내는 토픽(payload [`RemoteForwardsChanged`]). 호스트 이름 · 포트를 실으므로 owner 만.
pub const TOPIC_FORWARDS: &str = "remote.forwards";
pw_rpc::rpc_event!(TOPIC_FORWARDS, roles: [Owner]);

/// 한 호스트에 동시에 잇는 포워딩 연결의 상한(링크 채널 상한 256 의 1/4 — 출발값).
pub const MAX_CONNECTIONS: usize = 64;

/// 붙은 머신의 링크로 로컬 TCP 연결 하나를 원격 대상에 잇는 것(`pw_link::RemoteHost::forward`). 연결이 끝날 때까지 돈다.
pub trait Forwarder: Send + Sync {
    fn carry(&self, socket: TcpStream, to: ForwardTo) -> BoxFuture<'static, Result<(), String>>;
}

/// 실행 탭 포워딩을 열지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardOpenError {
    Remote(RemoteError),
    /// 이 컴퓨터에서 그 로컬 포트를 이미 누가 쓴다.
    PortInUse(u16),
    Bind(String),
}

impl std::fmt::Display for ForwardOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remote(e) => e.fmt(f),
            Self::PortInUse(port) => ForwardError::PortInUse(*port).fmt(f),
            Self::Bind(why) => write!(f, "cannot listen: {why}"),
        }
    }
}

/// 열린(또는 열지 못한 설정의) 포워딩 하나.
struct Entry {
    info: RemoteForward,
    /// 듣기 태스크(받은 연결의 태스크도 그 안에서 돈다 — 버리면 모두 끝난다).
    task: Option<tokio::task::AbortHandle>,
    /// 실행 탭 포워딩의 스트림이 보는 것. 닫으면 `None`.
    watch: Option<watch::Sender<Option<RemoteForward>>>,
    /// 설정의 포워딩이면 그 설정(다시 붙을 때 같은 것을 두 번 열지 않게).
    setting: Option<ForwardSetting>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(watch) = &self.watch {
            watch.send_replace(None);
        }
    }
}

/// 포워딩 표(호스트마다).
#[derive(Default)]
pub(crate) struct Forwards {
    next_id: u64,
    entries: Vec<Entry>,
    /// 호스트마다 동시 연결 상한.
    slots: HashMap<String, Arc<Semaphore>>,
}

/// 실행 탭 포워딩을 쥐는 것. 버리면 닫는다(`remote.forward.open` 스트림이 닫힘).
pub struct ForwardUse {
    hosts: RemoteHosts,
    pub id: u64,
    /// 지금 포워딩(닫히면 `None`).
    pub watch: watch::Receiver<Option<RemoteForward>>,
}

impl Drop for ForwardUse {
    fn drop(&mut self) {
        self.hosts.close_forward(self.id);
    }
}

impl RemoteHosts {
    /// 포워딩 목록(`remote.forwards.list`). `name` 이 없으면 모든 호스트.
    pub fn forwards(&self, name: Option<&str>) -> Vec<RemoteForward> {
        lock(&self.inner.forwards)
            .entries
            .iter()
            .filter(|e| name.is_none_or(|n| e.info.name == n))
            .map(|e| e.info.clone())
            .collect()
    }

    /// 실행 탭 포트의 「로컬로 포워딩」(`remote.forward.open`): 그 호스트의 `localhost:remote_port` 를 이 컴퓨터의 루프백
    /// `local_port`(없으면 같은 번호, `0` 이면 OS 가 고름)로. 돌려준 것을 쥐는 동안 연다.
    pub async fn open_forward(
        &self,
        name: &str,
        remote_port: u16,
        local_port: Option<u16>,
    ) -> Result<ForwardUse, ForwardOpenError> {
        self.entry(name).map_err(ForwardOpenError::Remote)?;
        let to = ForwardTo::new(DEFAULT_REMOTE_HOST, remote_port);
        to.check()
            .map_err(|e| ForwardOpenError::Remote(RemoteError::Conflict(e)))?;
        let addr = forward::local_addr(None, local_port.unwrap_or(remote_port))
            .map_err(|e| ForwardOpenError::Bind(e.to_string()))?;
        let listener = forward::bind(addr).await.map_err(|e| match e {
            ForwardError::PortInUse(port) => ForwardOpenError::PortInUse(port),
            other => ForwardOpenError::Bind(other.to_string()),
        })?;
        let local = listener
            .local_addr()
            .map_err(|e| ForwardOpenError::Bind(e.to_string()))?;
        let (tx, rx) = watch::channel(None);
        let task = self.spawn_accept(name, listener, to.clone());
        let id = {
            let mut table = lock(&self.inner.forwards);
            table.next_id += 1;
            let id = table.next_id;
            let info = RemoteForward {
                id,
                name: name.to_owned(),
                origin: RemoteForwardOrigin::Run,
                local_host: local.ip().to_string(),
                local_port: local.port(),
                remote_host: to.host.clone(),
                remote_port,
                state: RemoteForwardState::Listening,
            };
            tx.send_replace(Some(info.clone()));
            table.entries.push(Entry {
                info,
                task: Some(task),
                watch: Some(tx),
                setting: None,
            });
            id
        };
        self.publish_forwards(name);
        self.note(
            name,
            None,
            RemoteLogLevel::Info,
            &format!("forwarding {local} to localhost:{remote_port} on the remote host"),
        );
        Ok(ForwardUse {
            hosts: self.clone(),
            id,
            watch: rx,
        })
    }

    /// 포워딩 하나를 끈다(`remote.forward.close` — 상태바 · 원격 호스트 시트). 실행 탭의 것이면 그 스트림이 끝난다. 설정의 것은
    /// 다시 붙을 때 다시 연다.
    pub fn close_forward(&self, id: u64) -> bool {
        let removed = {
            let mut table = lock(&self.inner.forwards);
            table
                .entries
                .iter()
                .position(|e| e.info.id == id)
                .map(|at| table.entries.remove(at))
        };
        match removed {
            Some(entry) => {
                let name = entry.info.name.clone();
                drop(entry);
                self.publish_forwards(&name);
                true
            }
            None => false,
        }
    }

    /// 그 호스트의 포워딩을 모두 닫는다(사람이 끊음 · 쓰는 창이 모두 닫힘). 슬롯 잠금을 쥔 채 부를 수 있다(로그를 남기지 않는다).
    pub(crate) fn close_host_forwards(&self, name: &str) {
        let removed: Vec<Entry> = {
            let mut table = lock(&self.inner.forwards);
            let (gone, kept) = std::mem::take(&mut table.entries)
                .into_iter()
                .partition(|e| e.info.name == name);
            table.entries = kept;
            table.slots.remove(name);
            gone
        };
        if !removed.is_empty() {
            drop(removed);
            self.publish_forwards(name);
        }
    }

    /// 붙은 뒤: 그 호스트의 설정 포워딩을 연다(이미 연 것은 그대로).
    pub(crate) async fn open_configured_forwards(&self, name: &str) {
        let Some(backend) = self.backend() else {
            return;
        };
        let snapshot = read(&self.inner.host)
            .and_then(|h| h.settings)
            .map(|s| s.snapshot());
        let settings = snapshot
            .as_ref()
            .and_then(|s| s.get("settings", crate::pw_link_key()));
        let wanted = backend.port_forwards(settings, name);
        let mut changed = false;
        for setting in wanted {
            let open = lock(&self.inner.forwards)
                .entries
                .iter()
                .any(|e| e.info.name == name && e.setting.as_ref() == Some(&setting));
            if open {
                continue;
            }
            let to = ForwardTo::new(
                setting
                    .remote_host
                    .clone()
                    .unwrap_or_else(|| DEFAULT_REMOTE_HOST.to_owned()),
                setting.remote_port,
            );
            let local_host = setting
                .local_host
                .clone()
                .unwrap_or_else(|| "127.0.0.1".to_owned());
            let bound = match forward::local_addr(setting.local_host.as_deref(), setting.local_port)
            {
                Ok(addr) => forward::bind(addr).await,
                Err(e) => Err(e),
            };
            let (state, task, local_port) = match bound {
                Ok(listener) => {
                    let port = listener
                        .local_addr()
                        .map_or(setting.local_port, |a| a.port());
                    let task = self.spawn_accept(name, listener, to.clone());
                    (RemoteForwardState::Listening, Some(task), port)
                }
                Err(e) => {
                    let kind = match &e {
                        ForwardError::NotLoopback(_) => RemoteForwardFailureKind::NotLoopback,
                        ForwardError::PortInUse(_) => RemoteForwardFailureKind::PortInUse,
                        ForwardError::Bind(_) => RemoteForwardFailureKind::Bind,
                    };
                    tracing::warn!(target: "pw_remote", host = name, error = %e, "a port forward was not opened");
                    self.note(
                        name,
                        None,
                        RemoteLogLevel::Error,
                        &format!(
                            "port forward {} → {}:{} was not opened: {e}",
                            setting.local_port, to.host, to.port
                        ),
                    );
                    (
                        RemoteForwardState::Failed {
                            error: RemoteForwardFailure {
                                kind,
                                message: e.to_string(),
                            },
                        },
                        None,
                        setting.local_port,
                    )
                }
            };
            let mut table = lock(&self.inner.forwards);
            table.next_id += 1;
            let id = table.next_id;
            table.entries.push(Entry {
                info: RemoteForward {
                    id,
                    name: name.to_owned(),
                    origin: RemoteForwardOrigin::Settings,
                    local_host,
                    local_port,
                    remote_host: to.host,
                    remote_port: to.port,
                    state,
                },
                task,
                watch: None,
                setting: Some(setting),
            });
            changed = true;
        }
        if changed {
            self.publish_forwards(name);
        }
    }

    /// 듣기 태스크. 받은 연결마다 그 호스트의 지금 링크로 잇는다.
    fn spawn_accept(
        &self,
        name: &str,
        listener: tokio::net::TcpListener,
        to: ForwardTo,
    ) -> tokio::task::AbortHandle {
        let slots = Arc::clone(
            lock(&self.inner.forwards)
                .slots
                .entry(name.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(MAX_CONNECTIONS))),
        );
        let (hosts, name) = (self.clone(), name.to_owned());
        tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { continue };
                        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
                            tracing::info!(target: "pw_remote", host = %name, "too many forwarded connections; closed one");
                            continue;
                        };
                        let Some(forwarder) = hosts.forwarder(&name) else {
                            // 링크가 없다(다시 붙는 중) — 연결을 닫는다.
                            continue;
                        };
                        let to = to.clone();
                        connections.spawn(async move {
                            let _permit = permit;
                            if let Err(why) = forwarder.carry(socket, to).await {
                                tracing::debug!(target: "pw_remote", %why, "a forwarded connection was refused");
                            }
                        });
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        })
        .abort_handle()
    }

    /// 그 호스트의 지금 링크의 포워더.
    fn forwarder(&self, name: &str) -> Option<Arc<dyn Forwarder>> {
        lock(&self.inner.slots)
            .get(name)
            .and_then(|s| s.forwarder.clone())
    }

    fn publish_forwards(&self, name: &str) {
        let forwards = self.forwards(Some(name));
        if let Some(host) = read(&self.inner.host) {
            let _ = host.bus.publish(
                Scope::Host,
                TOPIC_FORWARDS,
                &RemoteForwardsChanged {
                    name: name.to_owned(),
                    forwards,
                },
            );
        }
    }
}
