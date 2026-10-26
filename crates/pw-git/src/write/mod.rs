//! 쓰기 실행기(13-m2-plan.md M2-06 — 결정 M2-D6 · M2-D8). ref · index · 작업 트리를 바꾸는 RPC(M2-07 ~ M2-11)는 모두 이것으로
//! 돌고, 그 행들은 [`GitWrite::run`] 을 부르는 메서드만 더한다.
//!
//! 쓰기 하나([`GitWrite::run`])의 차례:
//! 1. 저장소를 연다(`crate::Git::open` — 없으면 [`WriteError::NoRepository`]). 쓰기 명령을 만든다(`crate::Git::command` 의 쓰기 —
//!    실행 파일 · 버전 · 런타임 환경. 못 쓰면 [`WriteError::WritesOff`]). 미신뢰 프로젝트도 쓰기는 된다(사람이 누른 작업, D43).
//! 2. 프로젝트 범위 `Task` 를 띄운다(제목은 `task-title-git-*` Fluent id). 그 안에서 저장소 잠금([`lock`])을 기다려 받는다 — 같은
//!    저장소(연결 워크트리 포함)의 쓰기는 한 줄로 돌고, 읽기는 막지 않는다.
//! 3. 잠금을 받은 뒤, 작업 트리를 바꾸는 쓰기([`Changes`])면 미저장 문서 확인: 그 프로젝트를 듣는 창에 바뀔 경로를 보내고 답을
//!    기다린다([`prompt`]). 잠금 안에서 묻는다 — 앞 쓰기를 기다리는 사이 생긴 미저장 편집도 확인에 든다. 묻기 시작할 때 듣는 창이
//!    없으면 [`WriteError::NoWindow`], 취소 · 시간 초과면 [`WriteError::Cancelled`] 로 아무것도 하지 않는다. 묻는 도중 창이 닫혔으면
//!    진행한다(창 닫기가 M1-13 의 확인으로 그 창의 미저장 문서를 이미 처리했다). 그 뒤 `index.lock` 이 있으면 다른 git 이 작업
//!    중이라 [`WriteError::IndexLocked`] 로 끝낸다(그 파일은 지우지 않는다). 그 뒤 본문(`job`)이 [`WriteCtx`] 로 git 을 돌린다.
//! 4. 진행은 명령마다 `git <하위 명령>` 이다(git 의 사람용 출력은 해석하지 않는다). 실패면 git 의 stderr 원문이 Task 의 오류가 되어
//!    창의 실패 알림 body 로 나간다 — RPC 의 오류([`code::GIT_FAILED`])를 받은 창은 다시 알리지 않는다. 취소(`task.cancel`)는
//!    git 의 프로세스 그룹에 SIGTERM 을 보내 git 이 잠금 파일을 지우고 끝난 뒤에 저장소 잠금을 놓는다. RPC 를 부른 쪽이 사라져도
//!    쓰기는 끝까지 간다 — 멈추는 길은 Task 취소다.
//! 5. 인증: 인증을 물을 수 있는 쓰기([`WriteRequest::askpass`] — 원격에 닿는 fetch · pull · push 등)만, 소켓을 연 앱이면 쓰기마다
//!    askpass 토큰을 만들어 git 의 환경에 넣는다([`askpass`]). 그 밖의 쓰기(계속 · 중단 · 커밋 등)의 훅은 도우미를 받지 못한다 —
//!    저장소의 훅이 진짜와 같은 암호 다이얼로그를 띄우지 못하게. 쓰기가 끝나면 토큰을 버린다.
//!
//! 진행 중 작업(`git.operation` 과 계속 · 중단 · 건너뛰기)은 [`operation`], RPC 는 [`methods`].

pub mod askpass;
mod lock;
pub mod methods;
pub mod operation;
pub mod prompt;
#[cfg(test)]
mod tests;

use std::fmt;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use pw_core::bus::Scope;
use pw_core::task::{TaskCtx, TaskProgress, TaskSpawner};
use pw_rpc::RpcError;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub use askpass::Askpass;
pub use methods::register;
pub use operation::{OperationAction, OperationInfo, OperationStep};
pub use prompt::{
    AnswerError, AskpassHint, GitPrompt, GitPromptAnswer, GitPromptEvent, GitPromptRequest,
    Listener, PromptError, Prompts, askpass_text,
};

use crate::cli::{Access, GitCommand, Output, RunError};
use crate::methods::RepoOperation;
use crate::{CliError, Git, Opened, Target};

/// 쓰기 RPC 의 애플리케이션 오류 코드(`pw_rpc::code` 의 범위 밖). `data` 는 객체다.
pub mod code {
    /// 저장소가 없다(「저장소 없음」 — git 기능 전체가 빈 상태).
    pub const NO_REPOSITORY: i32 = 2001;
    /// git 쓰기가 꺼져 있다 — 실행 파일이 없거나 버전이 낮다(`data.reason` 원문). 버전이 낮으면 Host 가 알림을 한 번 냈다.
    pub const WRITES_OFF: i32 = 2002;
    /// `index.lock` 이 있다 — 다른 git 이 이 저장소에서 작업 중이다(`data.path`). 파일은 그대로 두었다.
    pub const INDEX_LOCKED: i32 = 2003;
    /// git 이 실패로 끝났다(`data.stderr` · `data.stdout` 원문, `data.exitCode`). 창은 Task 의 실패 알림으로 이미 보였다.
    pub const GIT_FAILED: i32 = 2004;
    /// 진행 중 작업이 없거나 지금 작업에서 할 수 없는 동작이다.
    pub const NO_OPERATION: i32 = 2005;
    /// 작업 트리를 바꾸는 쓰기인데 미저장 문서를 확인할 그 프로젝트의 창이 듣고 있지 않다 — 쓰기를 하지 않았다.
    pub const NO_WINDOW: i32 = 2006;
}

/// 쓰기가 작업 트리를 어떻게 바꾸는가 — 미저장 문서 확인의 범위.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changes {
    /// 작업 트리를 바꾸지 않는다(ref · index 만 — 커밋 · fetch · push · 스테이징 등). 묻지 않는다.
    None,
    /// 이 경로들이 바뀐다. 작업 트리 기준 상대 경로이고 구분자는 `/` 다([`repo_path`]). 비었으면 묻지 않는다.
    Paths(Vec<String>),
    /// 바뀔 경로를 미리 모른다 — 작업 트리 전체로 본다.
    WorkTree,
}

/// git RPC 의 경로 꼴: 작업 트리 기준 상대 경로, 구분자 `/`(git porcelain 과 같다). `path` 가 절대 경로면 `work_tree` 안이어야
/// 하고, 상대 경로면 그대로 쓴다. 작업 트리 밖(`..` · 다른 루트)이거나 비었으면 `None`. UTF-8 이 아닌 이름은 바꿀 수 없는 바이트를
/// U+FFFD 로 바꾼다(표시 · 겹침 판정용).
pub fn repo_path(work_tree: &Path, path: &Path) -> Option<String> {
    let relative = if path.is_absolute() {
        path.strip_prefix(work_tree).ok()?
    } else {
        path
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// 쓰기가 끝난 이유(성공이 아닌 것).
#[derive(Debug, Clone, PartialEq)]
pub enum WriteError {
    NoRepository,
    WritesOff(CliError),
    IndexLocked {
        path: PathBuf,
    },
    /// git 이 0 이 아닌 코드로 끝났다. `command` 는 하위 명령(`rebase` 등).
    Failed {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
        stdout: String,
    },
    /// 미저장 확인에서 취소했거나, Task 를 취소했다.
    Cancelled,
    /// 미저장 확인을 묻기 시작할 때 그 프로젝트를 듣는 창이 없었다.
    NoWindow,
    NoOperation,
    Unsupported {
        kind: RepoOperation,
        action: OperationAction,
    },
    /// git 을 띄우지 못했다.
    Spawn(String),
    /// 입력 오류 등(저장소 열기가 낸 RPC 오류 그대로).
    Rpc(RpcError),
    Internal(String),
}

impl WriteError {
    /// 끝난 git 의 출력으로 [`WriteError::Failed`] 를 만든다.
    pub fn failed(command: &str, output: &Output) -> Self {
        Self::Failed {
            command: command.to_owned(),
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        }
    }

    /// Task 의 실패 문구(창의 실패 알림 body). git 이 실패했으면 stderr 원문(비었으면 stdout)이다.
    pub fn task_message(&self) -> String {
        match self {
            Self::Failed {
                command,
                exit_code,
                stderr,
                stdout,
            } => [stderr.trim_end(), stdout.trim_end()]
                .into_iter()
                .find(|text| !text.is_empty())
                .map_or_else(
                    || match exit_code {
                        Some(code) => format!("git {command} exited with code {code}"),
                        None => format!("git {command} was killed by a signal"),
                    },
                    str::to_owned,
                ),
            other => other.to_string(),
        }
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRepository => f.write_str("the project is not in a git repository"),
            Self::WritesOff(CliError::NotFound { program, .. }) => {
                write!(
                    f,
                    "cannot find git (`{program}`); git writes are turned off"
                )
            }
            Self::WritesOff(CliError::TooOld { program, version }) => write!(
                f,
                "git {version} at {} is too old; git writes are turned off",
                program.display()
            ),
            Self::WritesOff(CliError::Untrusted { project_id }) => {
                write!(f, "project `{project_id}` is not trusted")
            }
            Self::IndexLocked { path } => write!(
                f,
                "{} exists: another git process seems to be running in this repository",
                path.display()
            ),
            Self::Failed { .. } => f.write_str(&self.task_message()),
            Self::Cancelled => f.write_str("cancelled"),
            Self::NoWindow => f.write_str(
                "no window of this project is listening to confirm unsaved documents; the write did not run",
            ),
            Self::NoOperation => {
                f.write_str("no merge, rebase, cherry-pick, revert or am is in progress")
            }
            Self::Unsupported { kind, action } => {
                write!(f, "cannot {action:?} a {kind:?} in progress")
            }
            Self::Spawn(e) => write!(f, "cannot run git: {e}"),
            Self::Rpc(e) => fmt::Display::fmt(e, f),
            Self::Internal(e) => f.write_str(e),
        }
    }
}

impl From<WriteError> for RpcError {
    fn from(e: WriteError) -> Self {
        let message = e.to_string();
        match e {
            WriteError::NoRepository => RpcError::new(code::NO_REPOSITORY, message),
            WriteError::WritesOff(_) => RpcError::new(code::WRITES_OFF, "git writes are turned off")
                .with_data(serde_json::json!({ "reason": message })),
            WriteError::IndexLocked { path } => RpcError::new(code::INDEX_LOCKED, message)
                .with_data(serde_json::json!({ "path": path.display().to_string() })),
            WriteError::Failed {
                command,
                exit_code,
                stderr,
                stdout,
            } => RpcError::new(code::GIT_FAILED, format!("git {command} failed")).with_data(
                serde_json::json!({ "command": command, "exitCode": exit_code, "stderr": stderr, "stdout": stdout }),
            ),
            WriteError::Cancelled => RpcError::cancelled(),
            WriteError::NoWindow => RpcError::new(code::NO_WINDOW, message),
            WriteError::NoOperation | WriteError::Unsupported { .. } => {
                RpcError::new(code::NO_OPERATION, message)
            }
            WriteError::Rpc(e) => e,
            WriteError::Spawn(_) | WriteError::Internal(_) => RpcError::internal(message),
        }
    }
}

/// 쓰기 하나.
#[derive(Debug, Clone)]
pub struct WriteRequest {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기(`git.repo` 와 같다).
    pub worktree: Option<String>,
    /// Task 제목 — `task-title-git-*` Fluent id(`extensions/i18n-default/i18n/*/project.ftl`).
    pub title: String,
    pub changes: Changes,
    /// git · ssh 가 인증을 물을 수 있는 쓰기인가(원격에 닿는 fetch · pull · push 등). 참일 때만 askpass 도우미를 git 의 환경에
    /// 넣는다 — 거짓이면 훅 · 필터도 도우미를 받지 못한다(모듈 머리 주석 5).
    pub askpass: bool,
}

/// 쓰기의 저장소: 대상과 git 디렉터리.
#[derive(Debug, Clone)]
struct RepoDirs {
    target: Target,
    git_dir: PathBuf,
    /// 정규화한 공유 git 디렉터리(잠금의 열쇠).
    common_dir: PathBuf,
}

impl RepoDirs {
    fn of(opened: &Opened) -> Self {
        let common = opened.repo.common_dir();
        Self {
            target: opened.target.clone(),
            git_dir: opened.repo.git_dir().to_path_buf(),
            common_dir: pw_core::registry::path::normalize(common)
                .unwrap_or_else(|_| common.to_path_buf()),
        }
    }
}

/// 쓰기 실행기 · 창 질의 · askpass. 데스크톱이 하나 만들어 Host 의 Task 를 붙인다([`GitWrite::attach`]).
pub struct GitWrite {
    git: Arc<Git>,
    askpass: Askpass,
    /// 파일 서비스 — 충돌 해결의 내용 쓰기가 `fs.write` 와 같은 경로별 쓰기 잠금을 쥔다([`pw_fs::Fs::write_lock`]).
    fs: Arc<pw_fs::Fs>,
    prompts: Prompts,
    locks: lock::RepoLocks,
    tasks: OnceLock<TaskSpawner>,
    askpass_timeout: Duration,
    confirm_timeout: Duration,
    /// 시험: 충돌 해결의 내용 쓰기가 잠금 안 version 확인을 지나 파일을 쓰기 직전에 부른다.
    #[cfg(test)]
    pub(crate) before_conflict_write: std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl GitWrite {
    /// `fs` 는 `fs.*` RPC 와 같은 것을 넘긴다.
    pub fn new(git: Arc<Git>, askpass: Askpass, fs: Arc<pw_fs::Fs>) -> Self {
        Self {
            git,
            askpass,
            fs,
            prompts: Prompts::default(),
            locks: lock::RepoLocks::default(),
            tasks: OnceLock::new(),
            askpass_timeout: prompt::ASKPASS_TIMEOUT,
            confirm_timeout: prompt::CONFIRM_TIMEOUT,
            #[cfg(test)]
            before_conflict_write: std::sync::Mutex::new(None),
        }
    }

    /// 질문을 기다리는 시간을 바꾼다(시험).
    #[must_use]
    pub fn with_timeouts(mut self, askpass: Duration, confirm: Duration) -> Self {
        self.askpass_timeout = askpass;
        self.confirm_timeout = confirm;
        self
    }

    /// Host 의 작업(쓰기가 프로젝트 범위 Task 로 돈다). 두 번째부터는 무시한다.
    pub fn attach(&self, tasks: TaskSpawner) {
        let _ = self.tasks.set(tasks);
    }

    pub fn git(&self) -> &Arc<Git> {
        &self.git
    }

    pub fn fs(&self) -> &Arc<pw_fs::Fs> {
        &self.fs
    }

    pub fn askpass(&self) -> &Askpass {
        &self.askpass
    }

    /// 창 질의(`git.prompts` 가 듣고 `git.prompt.answer` 가 답한다).
    pub fn prompts(&self) -> &Prompts {
        &self.prompts
    }

    /// 쓰기 하나를 돌린다(모듈 머리 주석의 차례). `job` 은 저장소 잠금을 쥔 채 Task 안에서 돈다.
    pub async fn run<T, F, Fut>(&self, request: WriteRequest, job: F) -> Result<T, WriteError>
    where
        T: Send + 'static,
        F: FnOnce(WriteCtx) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, WriteError>> + Send + 'static,
    {
        let tasks =
            self.tasks.get().cloned().ok_or_else(|| {
                WriteError::Internal("git writes are not attached to a host".into())
            })?;
        let repo = self
            .open(&request.project_id, request.worktree.as_deref())
            .await?
            .ok_or(WriteError::NoRepository)?;
        let base = self
            .git
            .command(&repo.target, Access::Write)
            .await
            .map_err(WriteError::WritesOff)?;
        let askpass = if request.askpass {
            self.askpass
                .session(&repo.target.project_id, &self.prompts, self.askpass_timeout)
        } else {
            None
        };
        let base = match &askpass {
            Some(session) => {
                let mut env = base.env().to_vec();
                session.apply(&mut env);
                base.with_env(env)
            }
            None => base,
        };
        let lock = self.locks.get(&repo.common_dir);
        let git = Arc::clone(&self.git);
        let (prompts, confirm_timeout, changes) =
            (self.prompts.clone(), self.confirm_timeout, request.changes);
        let worktree = request.worktree;
        let spawner = tasks.scoped(
            Scope::Project(repo.target.project_id.clone()),
            CancellationToken::new(),
        );
        let (tx, rx) = oneshot::channel();
        spawner.spawn(request.title, move |task| {
            let cancel = task.cancellation().clone();
            // 본문은 따로 띄운다 — Task 를 취소하면 Task 의 future 는 곧바로 버려지지만, 본문은 취소 신호를 받아 git 이 끝나기를
            // 기다린 뒤에 잠금을 놓는다(git 이 지운 `index.lock` 을 다음 쓰기가 보지 않게).
            let work = tokio::spawn(async move {
                let result = async {
                    let _held = tokio::select! {
                        biased;
                        () = cancel.cancelled() => return Err(WriteError::Cancelled),
                        held = lock.lock_owned() => held,
                    };
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => return Err(WriteError::Cancelled),
                        confirmed = confirm_unsaved(&prompts, &repo, changes, confirm_timeout) => confirmed?,
                    }
                    let index_lock = repo.git_dir.join("index.lock");
                    if std::fs::symlink_metadata(&index_lock).is_ok() {
                        return Err(WriteError::IndexLocked { path: index_lock });
                    }
                    let ctx = WriteCtx {
                        git,
                        worktree,
                        repo,
                        base,
                        cancel,
                        task,
                        steps: AtomicU64::new(0),
                    };
                    job(ctx).await
                }
                .await;
                drop(askpass);
                let message = result.as_ref().err().map(WriteError::task_message);
                let _ = tx.send(result);
                message
            });
            async move {
                match work.await {
                    Ok(None) => Ok(()),
                    Ok(Some(message)) => Err(message),
                    Err(e) => Err(format!("the git write stopped: {e}")),
                }
            }
        });
        rx.await.unwrap_or_else(|_| {
            Err(WriteError::Internal(
                "the git write ended without a result".into(),
            ))
        })
    }

    /// 프로젝트의 저장소(없으면 `None`). 디스크를 읽으므로 블로킹 스레드에서 연다.
    async fn open(
        &self,
        project_id: &str,
        worktree: Option<&str>,
    ) -> Result<Option<RepoDirs>, WriteError> {
        let git = Arc::clone(&self.git);
        let (project_id, worktree) = (project_id.to_owned(), worktree.map(str::to_owned));
        tokio::task::spawn_blocking(move || {
            git.open(&project_id, worktree.as_deref())
                .map(|opened| opened.as_ref().map(RepoDirs::of))
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))?
        .map_err(WriteError::Rpc)
    }

    /// `git.operation`: 진행 중 작업. 저장소가 없거나 진행 중 작업이 없으면 `None`.
    pub async fn operation(
        &self,
        project_id: &str,
        worktree: Option<&str>,
    ) -> Result<Option<OperationInfo>, WriteError> {
        let git = Arc::clone(&self.git);
        let (project_id, worktree) = (project_id.to_owned(), worktree.map(str::to_owned));
        tokio::task::spawn_blocking(move || {
            git.open(&project_id, worktree.as_deref())
                .map(|opened| opened.and_then(|o| operation::read(&o.repo)))
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))?
        .map_err(WriteError::Rpc)
    }

    /// `git.operation.continue` · `abort` · `skip`. 끝난 뒤의 진행 중 작업을 돌려준다. git 이 다음 단계에서 다시 멈췄으면(충돌)
    /// 실패가 아니라 `stopped` 다 — 대상이나 단계가 바뀐 채 같은 작업이 남아 있으면 그렇게 본다.
    pub async fn act(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        action: OperationAction,
    ) -> Result<methods::GitOperationResult, WriteError> {
        let before = self
            .operation(project_id, worktree)
            .await?
            .ok_or(WriteError::NoOperation)?;
        if operation::args(before.kind, action).is_none() {
            return Err(WriteError::Unsupported {
                kind: before.kind,
                action,
            });
        }
        let request = WriteRequest {
            project_id: project_id.to_owned(),
            worktree: worktree.map(str::to_owned),
            title: action_title(action).to_owned(),
            changes: operation::changes(before.kind, action),
            askpass: false,
        };
        self.run(request, move |ctx| async move {
            // 잠금을 받은 뒤 다시 읽는다 — 기다리는 사이 터미널에서 끝냈을 수 있다.
            let before = ctx
                .read(|o| operation::read(&o.repo))
                .await?
                .ok_or(WriteError::NoOperation)?;
            let args = operation::args(before.kind, action).ok_or(WriteError::Unsupported {
                kind: before.kind,
                action,
            })?;
            let output = ctx.output(ctx.git().args(args)).await?;
            let after = ctx.read(|o| operation::read(&o.repo)).await?;
            if output.status.success() {
                return Ok(methods::GitOperationResult {
                    operation: after,
                    stopped: false,
                });
            }
            let moved = after.as_ref().is_some_and(|after| {
                after.kind == before.kind
                    && (after.heads != before.heads || after.step != before.step)
            });
            if action != OperationAction::Abort && moved {
                return Ok(methods::GitOperationResult {
                    operation: after,
                    stopped: true,
                });
            }
            Err(WriteError::failed(args[0], &output))
        })
        .await
    }
}

/// 미저장 문서 확인(모듈 머리 주석 3). 저장소 잠금을 쥔 채 부른다.
async fn confirm_unsaved(
    prompts: &Prompts,
    repo: &RepoDirs,
    changes: Changes,
    timeout: Duration,
) -> Result<(), WriteError> {
    let paths = match changes {
        Changes::None => return Ok(()),
        Changes::Paths(paths) if paths.is_empty() => return Ok(()),
        Changes::Paths(paths) => Some(paths),
        Changes::WorkTree => None,
    };
    let request = GitPromptRequest::Unsaved {
        work_tree: display(&repo.target.work_tree),
        paths,
    };
    match prompts.ask(&repo.target.project_id, request, timeout).await {
        Ok(GitPromptAnswer::Proceed) | Err(PromptError::WindowLeft) => Ok(()),
        Err(PromptError::NoWindow) => Err(WriteError::NoWindow),
        Ok(_) | Err(PromptError::TimedOut) => Err(WriteError::Cancelled),
    }
}

fn action_title(action: OperationAction) -> &'static str {
    match action {
        OperationAction::Continue => "task-title-git-continue",
        OperationAction::Abort => "task-title-git-abort",
        OperationAction::Skip => "task-title-git-skip",
    }
}

/// 쓰기 본문이 받는 것: 대상 저장소, askpass 가 든 쓰기 명령, 취소, 진행 보고.
pub struct WriteCtx {
    git: Arc<Git>,
    worktree: Option<String>,
    repo: RepoDirs,
    base: GitCommand,
    cancel: CancellationToken,
    task: TaskCtx,
    steps: AtomicU64,
}

impl WriteCtx {
    pub fn target(&self) -> &Target {
        &self.repo.target
    }

    /// 작업 트리 꼭대기(git 의 작업 디렉터리).
    pub fn work_tree(&self) -> &Path {
        &self.repo.target.work_tree
    }

    /// 이 작업 트리의 git 디렉터리.
    pub fn git_dir(&self) -> &Path {
        &self.repo.git_dir
    }

    /// 공유 git 디렉터리(정규화).
    pub fn common_dir(&self) -> &Path {
        &self.repo.common_dir
    }

    /// Task 취소. [`WriteCtx::output`] 이 이것으로 git 을 끝낸다.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancel
    }

    /// 새 git 명령(쓰기 환경 · askpass 포함, 인자 없음).
    pub fn git(&self) -> GitCommand {
        self.base.clone()
    }

    /// 저장소를 다시 열어 `read` 로 읽는다(잠금 안에서 지금 상태를 볼 때). gix 의 저장소는 스레드를 넘지 못하므로 블로킹 스레드에서
    /// 열고 읽은 값만 돌려준다.
    pub async fn read<R, F>(&self, read: F) -> Result<R, WriteError>
    where
        R: Send + 'static,
        F: FnOnce(&Opened) -> R + Send + 'static,
    {
        let git = Arc::clone(&self.git);
        let (project, worktree) = (self.repo.target.project_id.clone(), self.worktree.clone());
        tokio::task::spawn_blocking(move || {
            git.open(&project, worktree.as_deref())
                .map(|opened| opened.as_ref().map(read))
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))?
        .map_err(WriteError::Rpc)?
        .ok_or(WriteError::NoRepository)
    }

    /// 띄워 끝을 기다린다. 종료 코드와 상관없이 출력을 돌려준다 — 충돌로 멈추는 명령(merge · cherry-pick 등)은 부른 쪽이 판정한다.
    /// Task 가 취소되면 git 을 끝내고 [`WriteError::Cancelled`].
    pub async fn output(&self, command: GitCommand) -> Result<Output, WriteError> {
        if self.cancel.is_cancelled() {
            return Err(WriteError::Cancelled);
        }
        let done = self.steps.fetch_add(1, Ordering::Relaxed);
        self.task.report(TaskProgress {
            done,
            total: None,
            message: Some(format!("git {}", subcommand(&command))),
        });
        command.output(&self.cancel).await.map_err(|e| match e {
            RunError::Cancelled => WriteError::Cancelled,
            RunError::Spawn(e) => WriteError::Spawn(e),
        })
    }

    /// [`WriteCtx::output`] 과 같되 0 이 아닌 종료는 [`WriteError::Failed`] 다.
    pub async fn run(&self, command: GitCommand) -> Result<Output, WriteError> {
        let name = subcommand(&command);
        let output = self.output(command).await?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(WriteError::failed(&name, &output))
        }
    }
}

/// 진행에 보이는 하위 명령: `-` 로 시작하지 않는 첫 인자(`-c` 의 값은 건너뛴다). 그 밖의 인자(경로 · URL)는 보이지 않는다 — URL 에
/// 자격 증명이 들어 있을 수 있다.
fn subcommand(command: &GitCommand) -> String {
    let mut args = command.arguments().iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if text == "-c" || text == "-C" {
            args.next();
            continue;
        }
        if !text.starts_with('-') {
            return text.into_owned();
        }
    }
    String::new()
}

/// 레지스트리 · 파일 RPC 와 같은 정규화를 거친 경로 문자열.
fn display(path: &Path) -> String {
    pw_core::registry::path::normalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}
