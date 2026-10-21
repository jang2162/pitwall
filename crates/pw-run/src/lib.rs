//! 실행(`docs/design/02-tech-stack.md` 「레포 구조」 의 `pw-run`). 지금은 내장 터미널의 PTY 세션만 있다(M1-08). 실행 구성 ·
//! 태스크 감지는 M6 이 같은 PTY 위에 더한다.
//!
//! - [`shell`]: 띄울 셸의 argv 와 환경(결정 M1-D14).
//! - [`pty`]: PTY 하나 — 상한 있는 출력 버퍼, 순서 지키는 입력, 크기, 끝내기.
//! - [`methods`]: `terminal.*` RPC.
//!
//! 터미널은 그것을 연 `terminal.create` 스트림과 수명이 같다. 스트림이 어떻게 끝나든(셸 종료, `terminal.close`, `stream.cancel`,
//! 연결이 닫혀 세션이 스트림을 버림) 셸의 세션에 든 프로세스를 모두 끝낸다. 앱이 끝날 때는 Drop 이 돌지 않으므로(Tauri 는
//! `std::process::exit` 로 끝난다) 앱이 [`shutdown`] 을 부른다.
//!
//! `terminalId` 는 그 터미널을 연 RPC 세션(연결) 안에서만 유효하다 — 다른 창 · 다른 연결이 같은 번호로 부르면 없는 터미널이다
//! (03-architecture.md 「RPC 설계」).

pub mod methods;
pub mod pty;
pub mod shell;
#[cfg(unix)]
mod tree;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pw_core::config::ConfigStore;
use pw_core::registry::path::{WorktreeRootError, normalize, worktree_link, worktree_root};
use pw_core::registry::{Project, Registry as Projects, RegistryError};
use pw_rpc::{RpcError, SessionId, StreamSink, code};
use pw_runtime::Runtime;

use crate::methods::{TerminalCreateParams, TerminalEvent, TerminalId};
use crate::pty::{CHUNK, Pty, Spec};

/// 이 프로세스가 띄운 모든 터미널의 셸과 그 세션의 프로세스를 끝낸다(SIGHUP · SIGHUP 을 무시하면 300 ms 뒤 SIGKILL). 끝날 때까지
/// 블로킹한다. 앱이 끝나기 직전(Tauri `RunEvent::Exit`)에 부른다 — 그 뒤 `std::process::exit` 라 Drop 이 돌지 않는다.
pub fn shutdown() {
    pty::shutdown_all();
}

/// 셸이 끝난 뒤 남은 출력을 기다리는 시간. 셸이 띄운 백그라운드 프로세스가 PTY 를 쥐고 있으면 EOF 가 오지 않는다.
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(200);

/// Host 를 만든 뒤에 붙이는 것(Host 의 메서드는 Host 를 만드는 중에 붙으므로).
#[derive(Clone)]
pub struct HostParts {
    /// 전역 설정(`[terminal] shell`).
    pub global: ConfigStore,
    pub projects: Projects,
}

/// 열린 터미널과 그것을 연 RPC 세션.
type Sessions = Mutex<HashMap<TerminalId, (SessionId, Arc<Pty>)>>;

/// 한 Host 의 터미널들.
pub struct Terminals {
    runtime: Arc<Runtime>,
    host: OnceLock<HostParts>,
    sessions: Arc<Sessions>,
    next_id: AtomicU32,
}

impl Terminals {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            host: OnceLock::new(),
            sessions: Arc::default(),
            next_id: AtomicU32::new(1),
        }
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.host.set(parts);
    }

    fn host(&self) -> Result<&HostParts, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("terminals are not attached to a host"))
    }

    /// `owner` 세션이 연 터미널. 다른 세션의 것은 없는 것으로 본다.
    fn owned(&self, owner: SessionId, id: TerminalId) -> Option<Arc<Pty>> {
        lock(&self.sessions)
            .get(&id)
            .filter(|(session, _)| *session == owner)
            .map(|(_, pty)| Arc::clone(pty))
    }

    fn session(&self, owner: SessionId, id: TerminalId) -> Result<Arc<Pty>, RpcError> {
        self.owned(owner, id)
            .ok_or_else(|| RpcError::invalid_params(format!("terminal {id} is not running")))
    }

    pub fn input(&self, owner: SessionId, id: TerminalId, data: Vec<u8>) -> Result<(), RpcError> {
        if self.session(owner, id)?.write(data) {
            Ok(())
        } else {
            Err(RpcError::invalid_params(format!(
                "terminal {id} is not running"
            )))
        }
    }

    pub fn resize(
        &self,
        owner: SessionId,
        id: TerminalId,
        cols: u16,
        rows: u16,
    ) -> Result<(), RpcError> {
        check_size(cols, rows)?;
        self.session(owner, id)?
            .resize(cols, rows)
            .map_err(RpcError::internal)
    }

    /// 없는 터미널(다른 세션의 것 포함)이면 아무것도 하지 않는다.
    pub fn close(&self, owner: SessionId, id: TerminalId) {
        if let Some(pty) = self.owned(owner, id) {
            tracing::info!(target: "pw_run", terminal = id, "closing the terminal");
            pty.terminate();
        }
    }

    /// 열린 터미널의 PTY(시험용).
    #[doc(hidden)]
    pub fn pty(&self, id: TerminalId) -> Option<Arc<Pty>> {
        lock(&self.sessions)
            .get(&id)
            .map(|(_, pty)| Arc::clone(pty))
    }

    /// `terminal.create` 스트림 본체.
    pub async fn run(
        self: Arc<Self>,
        owner: SessionId,
        params: TerminalCreateParams,
        sink: StreamSink<TerminalEvent>,
    ) -> Result<(), RpcError> {
        check_size(params.cols, params.rows)?;
        // 레지스트리 읽기 · 로그인 셸 기다리기 · 띄우기는 블로킹이다. 스트림이 그사이 버려지면 결과(`Live`)도 버려져 띄운 셸이 끝난다.
        let this = Arc::clone(&self);
        let (live, started) = tokio::task::spawn_blocking(move || this.start(owner, &params))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))??;
        sink.send(started).await?;
        pump(&live.pty, &sink).await?;
        let code = live.pty.exited().await;
        tracing::info!(target: "pw_run", terminal = live.id, ?code, "terminal exited");
        sink.send(TerminalEvent::Exited { code }).await
    }

    fn start(
        &self,
        owner: SessionId,
        params: &TerminalCreateParams,
    ) -> Result<(Live, TerminalEvent), RpcError> {
        let host = self.host()?;
        let project = host
            .projects
            .get(&params.project_id)
            .map_err(registry_error)?;
        // 워크트리는 `cwd` 를 줬어도 검사한다 — 위조한 워크트리 인자를 조용히 무시하지 않는다.
        let in_worktree = match &params.worktree {
            Some(top) => Some(
                worktree_root(project.root(), Path::new(top))
                    .map_err(|e| worktree_error(e, top, &project.id))?,
            ),
            None => None,
        };
        let cwd = match (&params.cwd, in_worktree) {
            (Some(dir), _) => project_dir(&project, Path::new(dir))?,
            (None, Some(root)) => root,
            (None, None) => {
                normalize(project.root()).unwrap_or_else(|_| project.root().to_path_buf())
            }
        };
        if !cwd.is_dir() {
            return Err(RpcError::invalid_params(format!(
                "cwd `{}` is not a directory",
                cwd.display()
            )));
        }
        let runtime = self.runtime.env(&project.id, &cwd);
        let argv = shell::argv(
            host.global.snapshot().get("settings", shell::SETTINGS_KEY),
            &runtime.env,
        );
        let spec = Spec {
            argv: argv.iter().map(OsString::from).collect(),
            cwd: cwd.clone(),
            env: shell::child_env(&runtime.env),
            cols: params.cols,
            rows: params.rows,
        };
        let pty = Arc::new(Pty::spawn(&spec).map_err(|reason| {
            tracing::warn!(target: "pw_run", ?argv, cwd = %cwd.display(), %reason, "cannot start the terminal");
            RpcError::internal(reason)
        })?);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.sessions).insert(id, (owner, Arc::clone(&pty)));
        tracing::info!(
            target: "pw_run",
            terminal = id, pid = ?pty.pid(), ?argv, cwd = %cwd.display(),
            env_from_login_shell = runtime.login_shell,
            "terminal started"
        );
        let started = TerminalEvent::Started {
            terminal_id: id,
            pid: pty.pid(),
            shell: argv,
            cwd: cwd.display().to_string(),
        };
        let live = Live {
            id,
            pty,
            sessions: Arc::clone(&self.sessions),
        };
        Ok((live, started))
    }
}

/// 출력을 EOF 까지 보낸다. 셸이 끝났는데 EOF 가 오지 않으면(백그라운드 프로세스가 PTY 를 쥐고 있다) 출력이
/// [`DRAIN_AFTER_EXIT`] 동안 없을 때 그만둔다.
async fn pump(pty: &Pty, sink: &StreamSink<TerminalEvent>) -> Result<(), RpcError> {
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
        let Some(bytes) = next else {
            return Ok(());
        };
        // 보낸 뒤에 버퍼에서 뺀다 — credit 을 기다리는 청크도 버퍼 상한에 든다.
        sink.send(TerminalEvent::Output {
            data: STANDARD.encode(&bytes),
        })
        .await?;
        pty.consume(bytes.len());
    }
}

/// 살아 있는 터미널. 버려지면(스트림이 끝나거나 버려짐) 목록에서 빼고 프로세스 트리를 끝낸다.
struct Live {
    id: TerminalId,
    pty: Arc<Pty>,
    sessions: Arc<Sessions>,
}

impl Drop for Live {
    fn drop(&mut self) {
        lock(&self.sessions).remove(&self.id);
        self.pty.terminate();
    }
}

fn check_size(cols: u16, rows: u16) -> Result<(), RpcError> {
    if cols == 0 || rows == 0 {
        return Err(RpcError::invalid_params(format!(
            "terminal size must be positive, got {cols}x{rows}"
        )));
    }
    Ok(())
}

/// `dir` 을 정규화한 것. 프로젝트의 루트 안이거나, 연결 워크트리 안이면서 메인 체크아웃 쪽 대응 경로가 루트 안이어야 한다
/// (`pw_runtime::Runtime::get` 의 `dir` 과 같은 규칙).
fn project_dir(project: &Project, dir: &Path) -> Result<PathBuf, RpcError> {
    let dir = normalize(dir).map_err(|_| {
        RpcError::invalid_params(format!("cwd `{}` must be an absolute path", dir.display()))
    })?;
    let roots: Vec<PathBuf> = project
        .roots
        .iter()
        .filter_map(|r| normalize(Path::new(r)).ok())
        .collect();
    let inside = |path: &Path| roots.iter().any(|root| path.starts_with(root));
    let via_worktree = || {
        worktree_link(&dir)
            .and_then(|link| link.map_to_main(&dir))
            .is_some_and(|main| inside(&main))
    };
    if inside(&dir) || via_worktree() {
        Ok(dir)
    } else {
        Err(RpcError::invalid_params(format!(
            "cwd `{}` is not inside project `{}`",
            dir.display(),
            project.id
        )))
    }
}

fn worktree_error(e: WorktreeRootError, top: &str, project: &str) -> RpcError {
    RpcError::invalid_params(match e {
        WorktreeRootError::Relative => format!("worktree `{top}` must be an absolute path"),
        WorktreeRootError::NotOurs => format!("`{top}` is not a worktree of project `{project}`"),
    })
}

fn registry_error(e: RegistryError) -> RpcError {
    match e {
        RegistryError::Db(_) => RpcError::internal(e.to_string()),
        RegistryError::Unavailable(_) => {
            RpcError::new(code::STORAGE_UNAVAILABLE, "storage unavailable")
                .with_data(serde_json::Value::String(e.to_string()))
        }
        _ => RpcError::invalid_params(e.to_string()),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(all(test, unix))]
mod tests;
