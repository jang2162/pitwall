//! Host 수명 터미널(18-m7-plan M7-09 ② — 메인 판정 M7-P8). 원격 Host(Relay)의 터미널은 실행처럼 Host 의 것이다.
//!
//! - `terminalId` 는 Host 안에서 유일하다. 붙기 · 입력 · 크기 · 닫기 · 떼기는 **그 프로젝트의 창**(`Project` 범위 — 실행과 같은 판정,
//!   `scope_may_write`)만 한다. 다른 범위(`Host` · `Agent` · `File`)는 `PERMISSION_DENIED`, 다른 프로젝트 창이 대는 번호는 없는 터미널
//!   이다(있다는 것을 알리지 않는다). role 은 메서드 선언(`roles: [Owner]`)이 자른다 — `readonly` · `mobile` 링크는 디스패처가 거절한다.
//! - 셸의 출력은 붙은 스트림과 상관없이 늘 링 버퍼([`TERMINAL_LOG`] — 지금 Host 버퍼와 같은 512 KiB)로 읽는다. 오프셋은 셸이 시작된
//!   뒤 낸 바이트 수다. 링이 넘치면 앞을 버리고(소비자가 느리거나 떼어진 동안), `terminal.attach { since }` 는 그 차이를 잃은 바이트
//!   (`dropped`)로 알린다. 로컬 터미널과 달리 소비자가 느려도 셸을 멈추지 않는다 — 떼어진 동안에도 dev 서버가 돌아야 한다.
//! - 붙은 스트림(`terminal.create` · `terminal.attach`)이 하나도 없으면 **떼어진** 터미널이다. 연결이 닫히거나 스트림이 취소돼도 셸은
//!   산다. 끝내기는 `terminal.close`(사람이 탭을 닫음), 셸이 스스로 끝남, 떼어진 터미널 상한(머신당 [`DETACHED_LIMIT`] — 넘으면 가장
//!   오래 떼어진 것부터), 신뢰 해제(그 프로젝트의 터미널 모두 — 붙은 것도, [`crate::Terminals::follow_trust`]), Host 의 끝
//!   ([`crate::shutdown`]).
//! - **미신뢰 프로젝트의 터미널은 연결 수명이다**(리뷰 2.2 · 메인 판정 2026-11-23): 셸은 사람의 로그인 셸이라 띄우기 · 입력 · 크기 ·
//!   닫기는 로컬과 같게 받지만, 떼기 · 다시 붙기는 `PERMISSION_DENIED`(`data.reason = "untrusted"`)이고 붙은 스트림이 모두 끝나면
//!   (연결이 닫힘) 셸을 끝낸다 — 미신뢰 프로젝트의 셸이 Host 에 남아 다른 창이 그것으로 명령을 보내지 못하게.
//! - 셸이 끝난 터미널은 붙은 스트림이 `exited` 를 보내면 목록에서 빠진다. 떼어진 동안 끝났으면 다음 붙기가 남은 출력과 `exited` 를
//!   받을 때까지(또는 상한으로 밀릴 때까지) 남는다.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pw_rpc::{ConnectionScope, RpcError, SessionId, StreamSink, code};
use serde_json::json;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::methods::{TerminalId, TerminalInfo};
use crate::pty::{CHUNK, Pty};
use crate::{DRAIN_AFTER_EXIT, Live, lock};

/// 떼어진 터미널의 상한(머신 하나 — 출발값). 넘으면 가장 오래 떼어진 것부터 끝낸다.
pub const DETACHED_LIMIT: usize = 16;

/// 터미널 하나의 출력 링 버퍼(지금 Host 버퍼 [`crate::pty::OUTPUT_BUFFER`] 와 같다).
pub const TERMINAL_LOG: usize = crate::pty::OUTPUT_BUFFER;

/// 터미널의 수명.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// 그것을 연 `terminal.create` 스트림과 같다(로컬 Host — 창이 닫히면 끝난다).
    Connection,
    /// Host 의 것(원격 Host — 연결이 닫혀도 떼어진 터미널로 남는다). `max_detached` 는 떼어진 터미널의 상한.
    Host { max_detached: usize },
}

impl Lifetime {
    /// 원격 Host 의 기본값.
    pub const fn host() -> Self {
        Self::Host {
            max_detached: DETACHED_LIMIT,
        }
    }
}

/// 앞을 버리는 바이트 링.
struct Ring {
    buf: VecDeque<u8>,
    /// `buf[0]` 의 오프셋(버린 바이트 수).
    start: u64,
    cap: usize,
}

impl Ring {
    fn append(&mut self, bytes: &[u8]) {
        self.buf.extend(bytes);
        let excess = self.buf.len().saturating_sub(self.cap);
        if excess > 0 {
            self.buf.drain(..excess);
            self.start += excess as u64;
        }
    }

    fn end(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    /// `since` 부터 최대 `max` 바이트와 그 첫 오프셋. `since` 가 버려진 앞이면 남은 처음부터.
    fn read(&self, since: u64, max: usize) -> (u64, Vec<u8>) {
        let from = since.clamp(self.start, self.end());
        let skip = usize::try_from(from - self.start).unwrap_or(usize::MAX);
        (
            from,
            self.buf.iter().skip(skip).take(max).copied().collect(),
        )
    }
}

/// 붙은 스트림 하나.
struct Stream {
    key: u64,
    session: SessionId,
    cancel: CancellationToken,
}

#[derive(Default)]
struct Attachments {
    streams: Vec<Stream>,
    /// 떼어진 순서(작을수록 오래). 붙은 스트림이 있으면 `None`.
    detached_at: Option<u64>,
}

/// Host 수명 터미널 하나.
pub(crate) struct Hosted {
    pub(crate) id: TerminalId,
    pub(crate) project: String,
    worktree: Option<String>,
    pid: Option<u32>,
    shell: Vec<String>,
    cwd: String,
    pty: Arc<Pty>,
    /// 버려지면 셸을 끝내고 열린 터미널 목록에서 뺀다.
    _live: Live,
    log: Mutex<Ring>,
    /// 출력이 붙거나 셸이 끝날 때마다 깬다.
    changed: watch::Sender<()>,
    /// 셸이 끝났고 남은 출력을 다 읽었으면 `Some(code)`.
    exit: watch::Sender<Option<Option<i32>>>,
    attachments: Mutex<Attachments>,
}

impl Hosted {
    pub(crate) fn new(
        live: Live,
        project: String,
        worktree: Option<String>,
        shell: Vec<String>,
        cwd: String,
    ) -> Self {
        let pty = Arc::clone(&live.pty);
        Self {
            id: live.id,
            project,
            worktree,
            pid: pty.pid(),
            shell,
            cwd,
            pty,
            _live: live,
            log: Mutex::new(Ring {
                buf: VecDeque::new(),
                start: 0,
                cap: TERMINAL_LOG,
            }),
            changed: watch::Sender::new(()),
            exit: watch::Sender::new(None),
            attachments: Mutex::default(),
        }
    }

    pub(crate) fn info(&self) -> TerminalInfo {
        let attached = !lock(&self.attachments).streams.is_empty();
        let exit = *self.exit.borrow();
        TerminalInfo {
            terminal_id: self.id,
            worktree: self.worktree.clone(),
            cwd: self.cwd.clone(),
            shell: self.shell.clone(),
            pid: self.pid,
            attached,
            exited: exit.is_some(),
            code: exit.flatten(),
        }
    }

    pub(crate) fn pty(&self) -> &Arc<Pty> {
        &self.pty
    }

    /// `since` 가 버려진 앞이면 실제로 보낼 첫 오프셋.
    pub(crate) fn first_from(&self, since: u64) -> u64 {
        lock(&self.log).read(since, 0).0
    }
}

/// 셸의 출력을 EOF 까지 링으로 읽는다(붙은 스트림과 상관없이 — 셸이 쓰기에서 멈추지 않게). 셸이 끝났는데 EOF 가 오지 않으면
/// [`DRAIN_AFTER_EXIT`] 동안 출력이 없을 때 그만둔다(로컬 터미널의 `pump` 와 같다).
pub(crate) async fn pump(hosted: Arc<Hosted>) {
    let pty = Arc::clone(&hosted.pty);
    let mut exited = false;
    loop {
        let next = if exited {
            tokio::time::timeout(DRAIN_AFTER_EXIT, pty.peek(CHUNK))
                .await
                .unwrap_or(None)
        } else {
            tokio::select! {
                next = pty.peek(CHUNK) => next,
                _ = pty.exited() => {
                    exited = true;
                    continue;
                }
            }
        };
        let Some(bytes) = next else { break };
        lock(&hosted.log).append(&bytes);
        pty.consume(bytes.len());
        hosted.changed.send_replace(());
    }
    let code = pty.exited().await;
    tracing::info!(target: "pw_run", terminal = hosted.id, ?code, "terminal exited");
    hosted.exit.send_replace(Some(code));
    hosted.changed.send_replace(());
}

/// 스트림이 셸의 출력을 `since` 부터 보낸다(쌓인 것 → 새로 나오는 것 → 끝). `cancel` 되면(`terminal.detach`) 끝을 보내지 않고
/// 그만둔다. `exited` 를 보냈으면 참.
pub(crate) async fn follow<E: serde::Serialize + Send + 'static>(
    hosted: &Hosted,
    since: u64,
    sink: &StreamSink<E>,
    cancel: &CancellationToken,
    output: impl Fn(u64, String) -> E,
    exited: impl Fn(Option<i32>) -> E,
) -> Result<bool, RpcError> {
    let mut changed = hosted.changed.subscribe();
    let mut next = since;
    loop {
        // 읽기 전에 본 것으로 표시한다 — 읽은 뒤에 붙은 출력은 `changed` 가 깨운다.
        changed.borrow_and_update();
        let exit = *hosted.exit.borrow();
        let (from, bytes) = lock(&hosted.log).read(next, CHUNK);
        if !bytes.is_empty() {
            next = from + bytes.len() as u64;
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(false),
                sent = sink.send(output(from, STANDARD.encode(&bytes))) => sent?,
            }
            continue;
        }
        // 끝은 출력을 다 읽은 뒤에 선다 — 끝을 본 다음 빈 조각이면 남은 출력이 없다.
        if let Some(code) = exit {
            sink.send(exited(code)).await?;
            return Ok(true);
        }
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(false),
            changed = changed.changed() => if changed.is_err() { return Ok(false) },
        }
    }
}

/// 한 Host 의 Host 수명 터미널들.
/// 프로젝트를 신뢰하는가(레지스트리 — Host 를 만든 뒤 붙는다).
pub(crate) type TrustCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

pub(crate) struct Hosting {
    terminals: Mutex<HashMap<TerminalId, Arc<Hosted>>>,
    /// 붙기 전에는 어느 프로젝트도 신뢰하지 않는다(fail-closed).
    trust: std::sync::OnceLock<TrustCheck>,
    /// 떼어진 순서.
    clock: AtomicU64,
    next_key: AtomicU64,
    max_detached: usize,
}

/// 붙은 스트림 하나. 버려지면(스트림이 끝남 · 연결이 닫힘) 떼고, 마지막이었으면 터미널은 떼어진 것이 된다.
pub(crate) struct Attachment {
    hosting: Arc<Hosting>,
    hosted: Arc<Hosted>,
    key: u64,
    cancel: CancellationToken,
}

impl Attachment {
    pub(crate) fn cancel(&self) -> &CancellationToken {
        &self.cancel
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.hosting.unattach(&self.hosted, self.key);
    }
}

fn not_running(id: TerminalId) -> RpcError {
    RpcError::invalid_params(format!("terminal {id} is not running"))
}

/// 그 프로젝트의 창(`Project` 범위)인가 — 아니면 `PERMISSION_DENIED`(실행과 같은 판정).
pub(crate) fn window_project(scope: &ConnectionScope) -> Result<&str, RpcError> {
    match scope {
        ConnectionScope::Project(id) => Ok(id),
        _ => Err(RpcError::new(
            code::PERMISSION_DENIED,
            "only a project window may use its terminals",
        )),
    }
}

impl Hosting {
    pub(crate) fn new(max_detached: usize) -> Self {
        Self {
            terminals: Mutex::default(),
            trust: std::sync::OnceLock::new(),
            clock: AtomicU64::new(1),
            next_key: AtomicU64::new(1),
            max_detached,
        }
    }

    /// 두 번째부터는 무시한다.
    pub(crate) fn set_trust(&self, check: TrustCheck) {
        let _ = self.trust.set(check);
    }

    pub(crate) fn trusted(&self, project: &str) -> bool {
        self.trust.get().is_some_and(|check| check(project))
    }

    /// 미신뢰 프로젝트면 떼기 · 붙기를 거절한다.
    pub(crate) fn require_trust(&self, project: &str) -> Result<(), RpcError> {
        if self.trusted(project) {
            Ok(())
        } else {
            Err(RpcError::new(
                code::PERMISSION_DENIED,
                "trust the project before keeping or reattaching its terminals",
            )
            .with_data(json!({ "reason": "untrusted", "projectId": project })))
        }
    }

    pub(crate) fn insert(&self, hosted: Arc<Hosted>) {
        lock(&self.terminals).insert(hosted.id, hosted);
    }

    /// 그 프로젝트 창의 터미널. 다른 프로젝트의 것은 없는 터미널이다.
    pub(crate) fn find(
        &self,
        scope: &ConnectionScope,
        id: TerminalId,
    ) -> Result<Arc<Hosted>, RpcError> {
        let project = window_project(scope)?;
        lock(&self.terminals)
            .get(&id)
            .filter(|t| t.project == project)
            .cloned()
            .ok_or_else(|| not_running(id))
    }

    /// 스트림 하나를 붙인다. 떼어진 터미널이면 더는 떼어진 것이 아니다.
    pub(crate) fn attach(self: &Arc<Self>, hosted: &Arc<Hosted>, session: SessionId) -> Attachment {
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        {
            let mut a = lock(&hosted.attachments);
            a.streams.push(Stream {
                key,
                session,
                cancel: cancel.clone(),
            });
            a.detached_at = None;
        }
        Attachment {
            hosting: Arc::clone(self),
            hosted: Arc::clone(hosted),
            key,
            cancel,
        }
    }

    fn unattach(&self, hosted: &Arc<Hosted>, key: u64) {
        let became_detached = {
            let mut a = lock(&hosted.attachments);
            let before = a.streams.len();
            a.streams.retain(|s| s.key != key);
            if before != a.streams.len() && a.streams.is_empty() {
                a.detached_at = Some(self.clock.fetch_add(1, Ordering::Relaxed));
                true
            } else {
                false
            }
        };
        if became_detached && lock(&self.terminals).contains_key(&hosted.id) {
            // 미신뢰 프로젝트의 터미널은 떼어 두지 않는다(연결 수명).
            if !self.trusted(&hosted.project) {
                tracing::info!(target: "pw_run", terminal = hosted.id, "ending a terminal of an untrusted project with its connection");
                self.remove(hosted.id);
                return;
            }
            tracing::info!(target: "pw_run", terminal = hosted.id, "terminal detached");
            self.keep_within_limit();
        }
    }

    /// 떼어진 터미널들, 오래 떼어진 것부터.
    fn detached(&self) -> Vec<(u64, Arc<Hosted>)> {
        let mut out: Vec<_> = lock(&self.terminals)
            .values()
            .filter_map(|t| {
                let a = lock(&t.attachments);
                a.streams
                    .is_empty()
                    .then(|| (a.detached_at.unwrap_or(0), Arc::clone(t)))
            })
            .collect();
        out.sort_by_key(|(at, _)| *at);
        out
    }

    /// 떼어진 터미널이 상한을 넘으면 가장 오래 떼어진 것부터 끝낸다. 끝낸 번호.
    fn keep_within_limit(&self) -> Vec<TerminalId> {
        let detached = self.detached();
        let excess = detached.len().saturating_sub(self.max_detached);
        let ended: Vec<TerminalId> = detached.iter().take(excess).map(|(_, t)| t.id).collect();
        for id in &ended {
            tracing::info!(target: "pw_run", terminal = id, limit = self.max_detached, "ending the oldest detached terminal");
            self.remove(*id);
        }
        ended
    }

    /// 목록에서 빼고 셸을 끝낸다.
    pub(crate) fn remove(&self, id: TerminalId) {
        let removed = lock(&self.terminals).remove(&id);
        if let Some(t) = removed {
            t.pty.terminate();
            for s in &lock(&t.attachments).streams {
                s.cancel.cancel();
            }
            t.changed.send_replace(());
        }
    }

    /// 붙은 스트림이 `exited` 를 보냈다 — 목록에서 뺀다(셸은 이미 끝났다).
    pub(crate) fn finished(&self, id: TerminalId) {
        lock(&self.terminals).remove(&id);
    }

    /// `terminal.close`: 셸을 끝낸다. 붙은 스트림은 남은 출력과 `exited` 를 받는다. 붙은 스트림이 없으면 곧바로 뺀다.
    pub(crate) fn close(&self, hosted: &Hosted) {
        tracing::info!(target: "pw_run", terminal = hosted.id, "closing the terminal");
        hosted.pty.terminate();
        if lock(&hosted.attachments).streams.is_empty() {
            self.remove(hosted.id);
        }
    }

    /// `terminal.detach`: 이 세션의 스트림을 떼고(끝을 보내지 않고 끝난다) 셸은 둔다. 그래서 떼어진 터미널이 상한을 넘으면
    /// `end_oldest` 가 아닐 때 아무것도 하지 않고 `CONFLICT` 와 끝낼 번호(`data.wouldEnd`)를 준다 — 창이 사람에게 묻는다. 끝낸 번호.
    pub(crate) fn detach(
        &self,
        hosted: &Arc<Hosted>,
        session: SessionId,
        end_oldest: bool,
    ) -> Result<Vec<TerminalId>, RpcError> {
        let becomes_detached = lock(&hosted.attachments)
            .streams
            .iter()
            .all(|s| s.session == session);
        if becomes_detached && !end_oldest {
            let detached = self.detached();
            let already = detached.iter().any(|(_, t)| t.id == hosted.id);
            let after = detached.len() + usize::from(!already);
            if after > self.max_detached {
                // 상한은 머신 하나의 것이라 다른 프로젝트의 터미널도 끝날 수 있다. 번호는 이 프로젝트 것만 주고, 다른 프로젝트 것은
                // 수만 준다(M7-09 리뷰 권고 4 — 다른 프로젝트의 번호는 없는 터미널과 같은 판정, 창은 「다른 프로젝트의 터미널 N개」로 묻는다).
                let ending = detached.iter().take(after - self.max_detached);
                let (mine, others): (Vec<_>, Vec<_>) =
                    ending.partition(|(_, t)| t.project == hosted.project);
                let would_end: Vec<TerminalId> = mine.iter().map(|(_, t)| t.id).collect();
                return Err(RpcError::new(
                    code::CONFLICT,
                    format!(
                        "this machine keeps at most {} detached terminals",
                        self.max_detached
                    ),
                )
                .with_data(json!({
                    "wouldEnd": would_end,
                    "otherProjects": others.len(),
                    "limit": self.max_detached,
                })));
            }
        }
        let became_detached = {
            let mut a = lock(&hosted.attachments);
            let before = a.streams.len();
            a.streams.retain(|s| {
                let mine = s.session == session;
                if mine {
                    s.cancel.cancel();
                }
                !mine
            });
            if a.streams.is_empty() && (before > 0 || a.detached_at.is_none()) {
                a.detached_at = Some(self.clock.fetch_add(1, Ordering::Relaxed));
                true
            } else {
                false
            }
        };
        if became_detached {
            tracing::info!(target: "pw_run", terminal = hosted.id, "terminal detached");
            return Ok(self.keep_within_limit());
        }
        Ok(Vec::new())
    }

    /// 그 프로젝트의 터미널(붙음 · 떼어짐 · 떼어진 채 끝남), 번호순.
    pub(crate) fn list(&self, project: &str) -> Vec<TerminalInfo> {
        let mut out: Vec<TerminalInfo> = lock(&self.terminals)
            .values()
            .filter(|t| t.project == project)
            .map(|t| t.info())
            .collect();
        out.sort_by_key(|t| t.terminal_id);
        out
    }

    /// 그 프로젝트들의 터미널을 모두 끝낸다(신뢰를 거둠) — 떼어진 것은 곧바로 빼고, 붙은 것은 셸을 끝내 스트림이 남은 출력과
    /// `exited` 를 받게 한다. 끝낸 번호.
    pub(crate) fn end_all_of(&self, projects: &[String]) -> Vec<TerminalId> {
        let ended: Vec<Arc<Hosted>> = lock(&self.terminals)
            .values()
            .filter(|t| projects.contains(&t.project))
            .cloned()
            .collect();
        for t in &ended {
            tracing::info!(target: "pw_run", terminal = t.id, "ending a terminal of a project that is no longer trusted");
            self.close(t);
        }
        ended.iter().map(|t| t.id).collect()
    }

    /// 터미널이 있는 프로젝트들.
    pub(crate) fn projects(&self) -> Vec<String> {
        let mut out: Vec<String> = lock(&self.terminals)
            .values()
            .map(|t| t.project.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub(crate) fn detached_count(&self) -> usize {
        self.detached().len()
    }
}

#[cfg(test)]
mod tests {
    use super::Ring;
    use std::collections::VecDeque;

    #[test]
    fn the_ring_drops_the_front_and_keeps_offsets() {
        let mut ring = Ring {
            buf: VecDeque::new(),
            start: 0,
            cap: 4,
        };
        ring.append(b"abc");
        ring.append(b"def");
        assert_eq!(ring.start, 2);
        assert_eq!(ring.end(), 6);
        assert_eq!(ring.read(0, 10), (2, b"cdef".to_vec()));
        assert_eq!(ring.read(4, 1), (4, b"e".to_vec()));
        assert_eq!(ring.read(9, 10), (6, Vec::new()));
        ring.append(b"0123456789");
        assert_eq!(ring.read(0, 10), (12, b"6789".to_vec()));
    }
}
