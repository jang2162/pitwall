//! MCP 세션이 다른 머신을 겨누는 데 쓰는 Desktop 쪽 부품(18-m7-plan M7-18 — `pw_mcp::checkout::Machines`).
//!
//! - **묶음**은 이 Desktop 의 레지스트리가 안다(`Registry::bundle` · `bundle_of`). 세션의 `projectId` 는 로컬 프로젝트 id 나 묶음 id
//!   다 — 다른 머신의 프로젝트 id 는 여기서 찾지 못한다(`None`).
//! - **머신 이름**은 이 Desktop 의 원격 호스트 항목 이름(별명)이다 — 처음 붙을 때 기억한 머신 id 로 잇는다(`remote_hosts` 표, M7-P6).
//!   Relay 가 댄 이름이 아니다(M7-P19).
//! - **링크**: 이미 붙은 머신이면 그 링크를 세션이 사는 동안 쥔다(`RemoteHosts::hold`). 붙지 않은 머신은 **사람에게 묻지 않고 붙을
//!   수 있을 때만** 연다(메인 판정 — 에이전트가 사람 화면에 SSH 질문을 띄우지 않는다): ssh 의 공유 연결(ControlMaster)이 살아 있어
//!   (`remote.hosts.check` 와 같은 가벼운 확인) 암호 · 호스트 키 질문이 나오지 않는 경우다. 머신 id 를 기억하지 않은 호스트(처음 보는
//!   키), 사람이 끊어 둔 호스트, 다른 머신이 답한 호스트(`machineChanged`), 누가 붙는 중인 호스트(그 붙기가 질문 중일 수 있다)는
//!   열지 않고 「Pitwall 에서 먼저 연결하라」 로 끝낸다.
//! - 연결은 그 머신 Host 에 `Agent` 범위로 연다 — 원격 Host 의 `agent_grant` · `agent_run_grant` 가 그대로 돈다(링크는 증명된 owner
//!   기기의 `Agent` 주장만 받는다 — `pw_link::claim`).

use std::sync::Arc;
use std::time::Duration;

use pw_core::registry::bundle::{LOCAL_MACHINE, ProjectBundle, is_bundle_row_id};
use pw_core::registry::{Registry, RegistryError};
use pw_core::{BoxFuture, HostRouter, Target};
use pw_mcp::checkout::{LinkState, MachineError, MachineLink, Machines};
use pw_remote::{RemoteHosts, RemoteStatus};
use pw_rpc::{Caller, Role};

/// 조용히 붙을 수 있는 링크가 붙기를 기다리는 상한.
pub const CONNECT_WAIT: Duration = Duration::from_secs(30);

pub struct DesktopMachines {
    registry: Registry,
    remote: Option<RemoteHosts>,
    router: Arc<HostRouter>,
    wait: Duration,
}

impl DesktopMachines {
    /// `router` 는 앱의 라우터다 — 붙은 원격 Host 가 그 머신 id 로 더해지는 곳(M7-08).
    pub fn new(registry: Registry, remote: Option<RemoteHosts>, router: Arc<HostRouter>) -> Self {
        Self {
            registry,
            remote,
            router,
            wait: CONNECT_WAIT,
        }
    }

    #[cfg(test)]
    fn with_wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    /// 그 머신 id 를 기억한 원격 호스트 항목 이름.
    fn host_of(&self, machine: &str) -> Option<String> {
        self.registry
            .remote_hosts()
            .ok()?
            .into_iter()
            .find(|r| r.machine.as_deref() == Some(machine))
            .map(|r| r.host)
    }

    async fn link(&self, machine: &str) -> Result<pw_remote::Use, MachineError> {
        let Some(remote) = &self.remote else {
            return Err(MachineError::Unreachable(
                "remote machines are not available in this Pitwall".into(),
            ));
        };
        let Some(name) = self.host_of(machine) else {
            return Err(MachineError::NeedsPerson(format!(
                "Pitwall has not connected to machine `{machine}` yet; connect to it in Pitwall first"
            )));
        };
        let first = |why: &str| {
            MachineError::NeedsPerson(format!(
                "connect to `{name}` in Pitwall first ({why}); the agent does not open a link that may ask a person"
            ))
        };
        match remote.status(&name) {
            RemoteStatus::Connected { machine: m } | RemoteStatus::Resuming { machine: m }
                if m == machine =>
            {
                return remote
                    .hold(&name)
                    .map_err(|e| MachineError::Unreachable(e.to_string()));
            }
            RemoteStatus::Connected { .. } | RemoteStatus::Resuming { .. } => {
                return Err(first("it answers as another machine"));
            }
            RemoteStatus::MachineChanged { .. } => {
                return Err(first("it answered as a different machine"));
            }
            RemoteStatus::Disconnected { by_user: true, .. } => {
                return Err(first("it was disconnected in Pitwall"));
            }
            RemoteStatus::Connecting { .. } | RemoteStatus::Reconnecting { .. } => {
                return Err(first("it is connecting now"));
            }
            RemoteStatus::Disconnected { by_user: false, .. } => {}
        }
        // 링크가 없다 — ssh 의 공유 연결이 살아 있을 때만 연다(그때는 암호 · 호스트 키 질문이 없다).
        let quiet = remote
            .check(&name)
            .await
            .map_err(|e| MachineError::Unreachable(e.to_string()))?
            .master
            == Some(true);
        if !quiet {
            return Err(first(
                "opening the link may ask for a password or a host key",
            ));
        }
        let held = remote
            .hold(&name)
            .map_err(|e| MachineError::Unreachable(e.to_string()))?;
        // `hold` 가 붙기를 시작하며 상태를 「붙는 중」 으로 둔다 — 그 뒤의 끊김 · 다른 머신은 그 붙기의 실패다.
        let mut status = remote.watch(&name);
        let connected = tokio::time::timeout(self.wait, async {
            loop {
                match &*status.borrow_and_update() {
                    RemoteStatus::Connected { machine: m } => return m == machine,
                    RemoteStatus::MachineChanged { .. }
                    | RemoteStatus::Disconnected { .. }
                    | RemoteStatus::Reconnecting { .. } => return false,
                    RemoteStatus::Connecting { .. } | RemoteStatus::Resuming { .. } => {}
                }
                if status.changed().await.is_err() {
                    return false;
                }
            }
        })
        .await;
        match connected {
            Ok(true) => Ok(held),
            Ok(false) => Err(MachineError::Unreachable(format!(
                "the link to `{name}` did not come up"
            ))),
            Err(_) => Err(MachineError::Unreachable(format!(
                "the link to `{name}` did not come up within {} seconds",
                self.wait.as_secs()
            ))),
        }
    }
}

fn found(r: Result<ProjectBundle, RegistryError>) -> Result<Option<ProjectBundle>, String> {
    match r {
        Ok(bundle) => Ok(Some(bundle)),
        Err(RegistryError::NotFound(_)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

impl Machines for DesktopMachines {
    fn bundle(&self, project_id: &str) -> Result<Option<ProjectBundle>, String> {
        if is_bundle_row_id(project_id) {
            return found(self.registry.bundle(project_id));
        }
        self.registry
            .bundle_of(LOCAL_MACHINE, project_id)
            .map_err(|e| e.to_string())
    }

    fn bundles(&self) -> Result<Vec<ProjectBundle>, String> {
        self.registry.bundles().map_err(|e| e.to_string())
    }

    fn machine_id(&self, name: &str) -> Option<String> {
        let records = self.registry.remote_hosts().ok()?;
        records
            .iter()
            .find(|r| r.host == name)
            .and_then(|r| r.machine.clone())
            .or_else(|| {
                records
                    .iter()
                    .find(|r| r.machine.as_deref() == Some(name))
                    .and_then(|r| r.machine.clone())
            })
    }

    fn machine_name(&self, machine: &str) -> Option<String> {
        self.host_of(machine)
    }

    /// 링크를 열지 않는다: 지금 상태를 읽고, 붙어 있지 않으면 ssh 공유 연결이 살아 있는지만 본다(`remote.hosts.check` 와 같은
    /// 가벼운 확인 — 18-m7-plan M7-19 ①).
    fn link_state<'a>(&'a self, machine: &'a str) -> BoxFuture<'a, LinkState> {
        Box::pin(async move {
            let (Some(remote), Some(name)) = (&self.remote, self.host_of(machine)) else {
                return LinkState::Disconnected;
            };
            match remote.status(&name) {
                RemoteStatus::Connected { machine: m } | RemoteStatus::Resuming { machine: m }
                    if m == machine =>
                {
                    LinkState::Connected
                }
                RemoteStatus::Connecting { .. } | RemoteStatus::Reconnecting { .. } => {
                    LinkState::Connecting
                }
                RemoteStatus::Disconnected { by_user: false, .. } => {
                    match remote.check(&name).await {
                        Ok(check) if check.master == Some(true) => LinkState::Available,
                        Ok(_) => LinkState::Disconnected,
                        Err(_) => LinkState::Unknown,
                    }
                }
                _ => LinkState::Disconnected,
            }
        })
    }

    fn connect<'a>(&'a self, machine: &'a str) -> BoxFuture<'a, Result<MachineLink, MachineError>> {
        Box::pin(async move {
            let held = self.link(machine).await?;
            let connection = self
                .router
                .connect(
                    &Target::HostId(machine.to_owned()),
                    Caller::agent(Role::Owner),
                )
                .await
                .map_err(|e| MachineError::Unreachable(e.to_string()))?;
            Ok(MachineLink {
                connection,
                hold: Box::new(held),
            })
        })
    }
}

#[cfg(test)]
#[path = "machines_tests.rs"]
mod tests;
