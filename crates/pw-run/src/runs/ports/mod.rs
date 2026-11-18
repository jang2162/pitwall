//! 포트 표시(17-m6-plan M6-08, F7-4) — 실행의 프로세스 트리가 듣는 TCP 포트.
//!
//! # 방법(메인 판정 M6-P12)
//!
//! 플랫폼 API 를 [`RunLimits::port_interval`](2초)마다 읽는다 — 프로세스를 띄우지 않는다(`lsof` 는 실행마다 2초에 한 번 프로세스를
//! 띄우고 출력 형식에 기댄다).
//!
//! - macOS — libproc: `proc_pidinfo(PROC_PIDLISTFDS)` 로 fd 를 훑고 소켓 fd 마다 `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)` 의 TCP
//!   상태가 `LISTEN` 인 것(`macos.rs`).
//! - Linux — `/proc/<pid>/fd` 의 `socket:[inode]` 와 `/proc/net/tcp{,6}` 의 상태 `0A`(LISTEN) 줄의 inode(`linux.rs`).
//! - Windows — `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_LISTENER)` 의 소유 pid(`windows.rs`). 트리는 Toolhelp 스냅숏의 부모 pid.
//!
//! # 어느 프로세스인가
//!
//! **실행의 프로세스 트리만** 본다 — 실행의 리더(PTY 의 프로세스)의 세션에 든 것(유닉스), 그 자손(부모 pid 를 따라 — `setsid` 로 세션을
//! 떠난 것도), 그리고 앞선 폴링에서 찾은 것(부모가 먼저 죽어 init 으로 넘어가도). 찾은 것은 (pid, 시작 시각)으로 기억해 그 pid 가 남의
//! 프로세스로 다시 나가면 닿지 않는다(`tree.rs` 와 같은 판정). 소켓을 읽은 뒤 시작 시각을 다시 보고 그 사이 바뀐 pid 의 소켓은
//! 버린다. 닿지 않는 것은 두 폴링 사이(2초)에 태어나 부모를 떠났고 부모도 그 사이 끝난 것(두 번 fork 한 데몬)이다. 다른 사용자의
//! 프로세스는 macOS · Linux 에서 fd 를 읽지 못해 빠진다(Windows 는 표가 pid 를 주지만 트리의 pid 만 본다).
//!
//! # 폴링
//!
//! Host 의 실행 관리자 하나에 스레드 하나(`run-ports`)가 **끝나지 않은 실행(pid 가 있는)이 있는 동안만** 돈다 — 한 번 훑은 프로세스
//! 목록으로 모든 실행의 트리를 가르고 소켓은 트리에 든 pid 만 읽는다(실행 20개라도 프로세스 목록은 한 번). 실행이 모두 끝나면
//! 스레드가 끝나고([`Runs::port_pollers`] 가 0), 실행이 다시 시작하면 다시 띄운다. 툴 윈도우가 보이지 않아도 돈다(상태바가 쓴다).
//!
//! 바뀐 실행은 프로젝트 범위 이벤트 [`TOPIC_PORTS`](payload [`RunInfo`])를 낸다. 끝난 실행의 포트는 비운다.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::{Inner, Run, RunInfo, RunState, lock};

/// 실행의 포트가 바뀌었을 때 버스로 내는 이벤트(프로젝트 범위). payload 는 [`RunInfo`](`ports` 칸).
pub const TOPIC_PORTS: &str = "run.ports";

/// 실행의 프로세스 트리가 듣는 TCP 포트 하나.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunPort {
    pub port: u16,
    /// Listening on every address (`0.0.0.0` or `::`) — reachable from other machines on the network.
    pub all_addresses: bool,
    /// The local addresses it listens on, sorted (`127.0.0.1`, `::1`, `0.0.0.0`, `::`, …).
    pub addresses: Vec<String>,
}

/// 듣는 소켓 하나(플랫폼 층이 낸다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Listener {
    pub pid: u32,
    pub addr: SocketAddr,
}

/// 프로세스 하나 — 스냅숏의 칸.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Proc {
    pub pid: u32,
    pub parent: Option<u32>,
    /// 세션 id(유닉스). Windows 는 `None`.
    pub session: Option<u32>,
    /// 시작 시각(같은 pid 가 다른 프로세스로 다시 쓰였는지 가른다). 읽지 못하면 `None`.
    pub start: Option<u64>,
}

/// 한 번 훑은 프로세스 목록.
#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    procs: BTreeMap<u32, Proc>,
    children: BTreeMap<u32, Vec<u32>>,
    sessions: BTreeMap<u32, Vec<u32>>,
}

impl Snapshot {
    pub fn take() -> Self {
        Self::of(processes())
    }

    pub fn of(procs: Vec<Proc>) -> Self {
        let mut snapshot = Self::default();
        for proc in procs {
            if let Some(parent) = proc.parent
                && parent != proc.pid
            {
                snapshot.children.entry(parent).or_default().push(proc.pid);
            }
            if let Some(session) = proc.session {
                snapshot.sessions.entry(session).or_default().push(proc.pid);
            }
            snapshot.procs.insert(proc.pid, proc);
        }
        snapshot
    }

    pub fn len(&self) -> usize {
        self.procs.len()
    }

    /// `pid` 가 지금도 그 시작 시각의 프로세스인가.
    fn same(&self, pid: u32, start: Option<u64>) -> bool {
        self.procs.get(&pid).is_some_and(|p| p.start == start)
    }

    /// 실행 하나의 트리(머리 주석 「어느 프로세스인가」) — (pid, 시작 시각). `known` 을 새로 찾은 것으로 늘리고 사라진 것을 뺀다.
    pub fn tree(&self, leader: u32, known: &mut Known) -> BTreeMap<u32, Option<u64>> {
        let mut found: BTreeMap<u32, Option<u64>> = BTreeMap::new();
        let mut queue: Vec<u32> = Vec::new();
        // 리더는 처음 본 시작 시각일 때만 — 리더가 끝나 거둬진 뒤 그 번호가 남에게 나가면 닿지 않는다.
        let reused = match self.procs.get(&leader) {
            Some(proc) => *known.pids.entry(leader).or_insert(proc.start) != proc.start,
            None => false,
        };
        if !reused {
            queue.push(leader);
            // 세션 id 는 그 세션에 프로세스가 남아 있는 동안 다른 프로세스에 나가지 않는다 — 리더가 거둬져도 세션에 남은 것은 실행의
            // 것이다. 그 번호가 남에게 나갔다면 앞 세션은 이미 비었다.
            queue.extend(self.sessions.get(&leader).into_iter().flatten().copied());
        }
        queue.extend(
            known
                .pids
                .iter()
                .filter(|&(&pid, &start)| self.same(pid, start))
                .map(|(&pid, _)| pid),
        );
        while let Some(pid) = queue.pop() {
            let Some(proc) = self.procs.get(&pid) else {
                continue;
            };
            if found.insert(pid, proc.start).is_some() {
                continue;
            }
            for &child in self.children.get(&pid).into_iter().flatten() {
                // 부모보다 먼저 태어난 「자식」 은 부모 pid 가 다시 쓰인 것이다(Windows 는 부모가 끝나도 부모 pid 칸이 남는다).
                let older = match (self.procs.get(&child).and_then(|c| c.start), proc.start) {
                    (Some(child), Some(parent)) => child < parent,
                    _ => false,
                };
                if !older {
                    queue.push(child);
                }
            }
        }
        known
            .pids
            .retain(|&pid, &mut start| pid == leader || self.same(pid, start));
        for (&pid, &start) in &found {
            known.pids.entry(pid).or_insert(start);
        }
        found
    }
}

/// 실행마다 앞선 폴링에서 찾은 트리의 (pid, 시작 시각).
#[derive(Debug, Default)]
pub(crate) struct Known {
    pids: BTreeMap<u32, Option<u64>>,
}

/// 듣는 소켓을 포트로 묶는다(포트 순).
pub(crate) fn group(listeners: impl IntoIterator<Item = SocketAddr>) -> Vec<RunPort> {
    let mut by_port: BTreeMap<u16, BTreeSet<IpAddr>> = BTreeMap::new();
    for addr in listeners {
        by_port.entry(addr.port()).or_default().insert(addr.ip());
    }
    by_port
        .into_iter()
        .map(|(port, ips)| RunPort {
            port,
            all_addresses: ips.iter().any(IpAddr::is_unspecified),
            addresses: ips.iter().map(ToString::to_string).collect(),
        })
        .collect()
}

#[cfg(unix)]
fn processes() -> Vec<Proc> {
    crate::tree::processes()
        .into_iter()
        .filter_map(|(pid, parent, session, start)| {
            Some(Proc {
                pid: u32::try_from(pid).ok()?,
                parent: parent.and_then(|p| u32::try_from(p).ok()),
                session: u32::try_from(session).ok(),
                start,
            })
        })
        .collect()
}

#[cfg(windows)]
fn processes() -> Vec<Proc> {
    windows::processes()
}

#[cfg(not(any(unix, windows)))]
fn processes() -> Vec<Proc> {
    Vec::new()
}

/// `pids` 가 듣는 TCP 소켓.
fn listeners(pids: &BTreeSet<u32>) -> Vec<Listener> {
    #[cfg(target_os = "macos")]
    return macos::listeners(pids);
    #[cfg(target_os = "linux")]
    return linux::listeners(pids);
    #[cfg(windows)]
    return windows::listeners(pids);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = pids;
        Vec::new()
    }
}

// --- 폴링 -----------------------------------------------------------------------------------------------------------

/// 실행 관리자의 폴링 상태.
#[derive(Default)]
pub(super) struct Poller {
    /// 스레드가 돌고 있는가(또는 띄우는 중인가).
    active: std::sync::Mutex<bool>,
    /// 살아 있는 폴링 스레드 수(시험이 0 을 본다).
    threads: Arc<AtomicUsize>,
}

impl Poller {
    pub(super) fn threads(&self) -> usize {
        self.threads.load(Ordering::SeqCst)
    }
}

/// 폴링 스레드가 없으면 띄운다. 실행이 시작할 때 부른다.
pub(super) fn ensure_polling(inner: &Arc<Inner>) {
    let mut active = lock(&inner.poller.active);
    if *active {
        return;
    }
    let weak = Arc::downgrade(inner);
    let interval = inner.limits.port_interval;
    let threads = Arc::clone(&inner.poller.threads);
    threads.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("run-ports".into()).spawn({
        let threads = Arc::clone(&threads);
        move || {
            loop {
                std::thread::sleep(interval);
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                if !poll(&inner) {
                    break;
                }
            }
            threads.fetch_sub(1, Ordering::SeqCst);
        }
    });
    match spawned {
        Ok(_) => *active = true,
        Err(e) => {
            threads.fetch_sub(1, Ordering::SeqCst);
            tracing::warn!(target: "pw_run", %e, "cannot start the run port poller");
        }
    }
}

/// 끝나지 않은 실행과 그 리더 pid.
fn live_runs(inner: &Inner) -> Vec<(Arc<Run>, u32)> {
    lock(&inner.table)
        .runs
        .values()
        .filter(|run| run.state() != RunState::Exited)
        .filter_map(|run| Some((Arc::clone(run), run.control.get()?.pid()?)))
        .collect()
}

/// 한 번 읽는다. 끝나지 않은 실행이 없으면 폴링을 끝낸다(거짓).
pub(super) fn poll(inner: &Arc<Inner>) -> bool {
    let mut runs = live_runs(inner);
    if runs.is_empty() {
        let mut active = lock(&inner.poller.active);
        // 그 사이 시작한 실행은 `ensure_polling` 이 `active` 를 보고 돌아갔다 — 잠금 안에서 다시 본다.
        runs = live_runs(inner);
        if runs.is_empty() {
            *active = false;
            return false;
        }
    }
    let began = Instant::now();
    let snapshot = Snapshot::take();
    let trees: Vec<BTreeMap<u32, Option<u64>>> = runs
        .iter()
        .map(|(run, leader)| snapshot.tree(*leader, &mut lock(&run.known)))
        .collect();
    let all: BTreeSet<u32> = trees.iter().flat_map(|t| t.keys().copied()).collect();
    let found = listeners(&all);
    // 소켓을 읽는 사이 끝나 그 번호가 남에게 나간 pid 의 소켓은 버린다.
    let after = Snapshot::of(processes_of(&found.iter().map(|l| l.pid).collect()));
    let mut changed = Vec::new();
    for ((run, _), tree) in runs.iter().zip(&trees) {
        let ports = group(
            found
                .iter()
                .filter(|l| {
                    tree.get(&l.pid)
                        .is_some_and(|&start| after.same(l.pid, start))
                })
                .map(|l| l.addr),
        );
        if run.set_ports(ports) {
            changed.push(Arc::clone(run));
        }
    }
    tracing::trace!(target: "pw_run", runs = runs.len(), processes = snapshot.len(), tree = all.len(), took = ?began.elapsed(), "read the run ports");
    for run in changed {
        let info: RunInfo = run.info();
        if info.state != RunState::Exited {
            inner.publish(&info.project_id, TOPIC_PORTS, &info);
        }
    }
    true
}

/// 그 pid 들의 지금 시작 시각만(소켓을 읽은 뒤의 확인 — 프로세스 목록 전체를 다시 훑지 않는다).
fn processes_of(pids: &BTreeSet<u32>) -> Vec<Proc> {
    pids.iter()
        .map(|&pid| Proc {
            pid,
            parent: None,
            session: None,
            start: start_time(pid),
        })
        .collect()
}

#[cfg(unix)]
fn start_time(pid: u32) -> Option<u64> {
    crate::tree::start_time(libc::pid_t::try_from(pid).ok()?)
}

#[cfg(windows)]
fn start_time(pid: u32) -> Option<u64> {
    windows::start_time(pid)
}

#[cfg(not(any(unix, windows)))]
fn start_time(_pid: u32) -> Option<u64> {
    None
}
