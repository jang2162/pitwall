//! Git(`docs/design/13-m2-plan.md` M2-01 — 결정 M2-D1 · M2-D3 · M2-D8).
//!
//! 읽기는 gitoxide(`gix`)로 Host 안에서 하고, ref · index · 작업 트리를 바꾸는 쓰기는 사람의 설정(훅 · 서명 · credential helper)을
//! git 자신이 처리하도록 git CLI 로 한다.
//!
//! - [`repo`]: 저장소 해석 — 프로젝트(활성 워크트리)의 저장소 하나, HEAD, 진행 중 작업.
//! - [`cli`]: git CLI 실행기 — 실행 파일 · 자식 환경 · 기계 형식 출력 · 프로세스 그룹째 취소.
//! - [`methods`]: `git.repo` RPC.
//! - [`status`]: 작업 트리 상태 · ref — `git.status` · `git.refs`(M2-02).
//! - [`watch`]: 저장소 감시 — `git.watch`, 이벤트 `git.statusChanged` · `git.refsChanged`(M2-02).
//! - [`write`]: 쓰기 실행기 · 진행 중 작업 · askpass(M2-06).
//! - [`conflict`]: 충돌 데이터 · 해결 — `git.conflicts` · `git.conflict.get` · `git.conflict.resolve`(M2-11).
//! - `testing`(시험 · `test-support` 기능): 시험 저장소 도우미.
//!
//! 이 크레이트의 모듈 배치 · 메서드 등록([`methods::register`])의 주인은 M2-01 이다. 이후 행은 모듈을 나눠(`status` · `log` ·
//! `diff` · `write`) 더하고 등록 줄을 하나씩 더한다(13-m2-plan.md 「공유 부품의 주인」).

pub mod cli;
pub mod conflict;
pub mod diff;
pub mod methods;
pub mod repo;
pub mod status;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod watch;
pub mod write;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use pw_core::bus::{EventBus, Scope};
use pw_core::config::ConfigStore;
use pw_core::notification::{Level, Notification};
use pw_core::registry::path::{WorktreeRootError, worktree_root};
use pw_core::registry::{Registry as Projects, RegistryError};
use pw_rpc::{RpcError, code};
use pw_runtime::Runtime;
use pw_runtime::program::find_program;
use tokio_util::sync::CancellationToken;

use crate::cli::{
    Access, GitCommand, GitVersion, MIN_VERSION, SETTINGS_KEY, VERSION_TIMEOUT, child_env,
};
use crate::methods::{GitRepo, GitRepoParams};

/// 쓰기를 끈 알림의 Fluent 메시지 id(`extensions/i18n-default/i18n/*/project.ftl`).
pub const TOO_OLD_MESSAGE: &str = "git-too-old";

/// Host 를 만든 뒤에 붙이는 것(Host 의 메서드는 Host 를 만드는 중에 붙으므로).
#[derive(Clone)]
pub struct HostParts {
    /// 전역 설정(`[git] path`).
    pub global: ConfigStore,
    pub projects: Projects,
    /// 알림(`notification.show`)을 내는 버스.
    pub bus: EventBus,
}

/// git 기능의 대상: 프로젝트와 그 저장소의 작업 트리.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub project_id: String,
    /// 활성 워크트리 안의 프로젝트 루트(없으면 주 루트). git 자식의 런타임 환경을 이 디렉터리로 만든다.
    pub project_root: PathBuf,
    /// 저장소의 작업 트리 꼭대기. git 자식의 작업 디렉터리다.
    pub work_tree: PathBuf,
}

/// 연 저장소. 열 때의 신뢰로 열린다([`repo::open_options`] — 미신뢰면 저장소 설정에서 프로그램을 고르지 않는다). 들고 있는 쪽은
/// 신뢰가 바뀌면(`pw_core::registry::ChangeKind::Trust`) 다시 연다 — [`Git::is_current`] 가 그 판정이다.
pub struct Opened {
    pub target: Target,
    pub repo: gix::Repository,
    /// 열 때 프로젝트를 신뢰했는가.
    pub trusted: bool,
}

/// git CLI 를 쓸 수 없는 이유.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// 미신뢰 프로젝트의 읽기다 — 신뢰가 필요하다(git 은 읽는 명령에서도 저장소 설정의 프로그램을 돌린다, [`cli`] 머리 주석).
    /// 미신뢰 프로젝트의 읽기는 gix 로만 한다.
    Untrusted { project_id: String },
    /// 실행 파일을 찾지 못했다. `login_shell_path` 는 찾아본 PATH 가 로그인 셸의 것인가(아니면 앱 프로세스의 것).
    NotFound {
        program: String,
        login_shell_path: bool,
    },
    /// 버전이 [`MIN_VERSION`] 미만이거나 읽지 못했다 — 쓰기를 끈다. `version` 은 읽은 버전, 못 읽었으면 그 이유.
    TooOld { program: PathBuf, version: String },
}

/// 실행 파일 하나의 버전 판정. 파일이 바뀌면(git 을 올리면) 다시 읽는다.
#[derive(Debug, Clone)]
struct VersionEntry {
    stamp: Option<(SystemTime, u64)>,
    version: Result<GitVersion, String>,
}

pub struct Git {
    runtime: Arc<Runtime>,
    host: OnceLock<HostParts>,
    versions: Mutex<HashMap<PathBuf, VersionEntry>>,
    /// 쓰기를 끈 알림을 이미 낸 실행 파일.
    warned: Mutex<HashSet<PathBuf>>,
}

impl Git {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            host: OnceLock::new(),
            versions: Mutex::default(),
            warned: Mutex::default(),
        }
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.host.set(parts);
    }

    fn host(&self) -> Result<&HostParts, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("git is not attached to a host config"))
    }

    /// `git.repo`.
    pub fn repo(&self, params: &GitRepoParams) -> Result<GitRepo, RpcError> {
        let repo = match self.open(&params.project_id, params.worktree.as_deref())? {
            Some(opened) => Some(repo::info(&opened.repo).map_err(|e| {
                RpcError::internal(format!(
                    "cannot read the repository at {}: {e}",
                    opened.target.work_tree.display()
                ))
            })?),
            None => None,
        };
        Ok(GitRepo { repo })
    }

    /// 프로젝트(활성 워크트리 `worktree` 가 있으면 그 안의 프로젝트 루트)의 저장소를 연다. 저장소가 없으면 `None`. `worktree` 는
    /// 창이 받은 `ActiveWorktree::top` 이다 — 그 워크트리의 메인 체크아웃 안에 프로젝트 루트가 있어야 한다(M1-05 의 위조 판정을
    /// 거친다, `pw_core::registry::path::worktree_root`).
    pub fn open(
        &self,
        project_id: &str,
        worktree: Option<&str>,
    ) -> Result<Option<Opened>, RpcError> {
        let host = self.host()?;
        let project = host.projects.get(project_id).map_err(registry_error)?;
        let project_root = match worktree {
            None => project.root().to_path_buf(),
            Some(top) => match worktree_root(project.root(), Path::new(top)) {
                Ok(root) => root,
                Err(WorktreeRootError::Relative) => {
                    return Err(RpcError::invalid_params(format!(
                        "worktree `{top}` must be an absolute path"
                    )));
                }
                Err(WorktreeRootError::NotOurs) => {
                    return Err(RpcError::invalid_params(format!(
                        "`{top}` is not a worktree of project `{project_id}`"
                    )));
                }
            },
        };
        let trusted = host.projects.is_trusted(project_id);
        let repo = match repo::discover(&project_root, trusted) {
            Ok(Ok(repo)) => repo,
            Ok(Err(reason)) => {
                tracing::debug!(target: "pw_git", project = project_id, root = %project_root.display(), ?reason, "no repository");
                return Ok(None);
            }
            Err(e) => {
                return Err(RpcError::internal(format!(
                    "cannot open the repository of {}: {e}",
                    project_root.display()
                )));
            }
        };
        let work_tree = repo
            .workdir()
            .map(Path::to_path_buf)
            .ok_or_else(|| RpcError::internal("the repository has no work tree"))?;
        Ok(Some(Opened {
            target: Target {
                project_id: project_id.to_owned(),
                project_root,
                work_tree,
            },
            repo,
            trusted,
        }))
    }

    /// `opened` 를 연 뒤로 그 프로젝트의 신뢰가 그대로인가. 아니면 그 핸들은 옛 신뢰의 설정을 들고 있다 — 다시 연다.
    pub fn is_current(&self, opened: &Opened) -> bool {
        self.trusted(&opened.target.project_id) == opened.trusted
    }

    /// `target` 의 작업 트리에서 돌릴 git 명령. 런타임 환경을 기다릴 수 있다(로그인 셸). 읽기는 신뢰한 프로젝트에서만 만든다
    /// ([`CliError::Untrusted`]). 쓰기면 버전을 보고, [`MIN_VERSION`] 미만이면 거절한다 — 그 실행 파일에 대해 알림을 한 번
    /// 낸다(Host 범위).
    pub async fn command(&self, target: &Target, access: Access) -> Result<GitCommand, CliError> {
        if access == Access::Read && !self.trusted(&target.project_id) {
            return Err(CliError::Untrusted {
                project_id: target.project_id.clone(),
            });
        }
        let runtime = Arc::clone(&self.runtime);
        let (id, root) = (target.project_id.clone(), target.project_root.clone());
        let resolved = tokio::task::spawn_blocking(move || runtime.env(&id, &root))
            .await
            .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
        let name = self.configured_program();
        let program = find_program(&name, &resolved.env.path).ok_or(CliError::NotFound {
            program: name,
            login_shell_path: resolved.login_shell,
        })?;
        let env = child_env(&resolved.env, access, None);
        let command = GitCommand::new(program.clone(), target.work_tree.clone(), env, access);
        if access == Access::Write {
            let version = self.version(&command).await;
            if !matches!(version, Ok(v) if v >= MIN_VERSION) {
                let version = version.map_or_else(|e| e, |v| v.to_string());
                self.warn_too_old(&program, &version);
                return Err(CliError::TooOld { program, version });
            }
        }
        Ok(command)
    }

    /// 프로젝트를 신뢰했는가(M1-05, 워크트리 프로젝트는 메인 저장소 프로젝트의 신뢰). Host 에 붙기 전 · 읽기 실패는 미신뢰다.
    fn trusted(&self, project_id: &str) -> bool {
        self.host
            .get()
            .is_some_and(|host| host.projects.is_trusted(project_id))
    }

    /// 전역 설정 `[git] path`. 없거나 문자열이 아니면(경고) `git`.
    fn configured_program(&self) -> String {
        let Some(host) = self.host.get() else {
            return "git".to_owned();
        };
        let snapshot = host.global.snapshot();
        match snapshot.get("settings", SETTINGS_KEY) {
            None => "git".to_owned(),
            Some(serde_json::Value::String(path)) if !path.is_empty() => path.clone(),
            Some(other) => {
                tracing::warn!(target: "pw_git", value = %other, "`{SETTINGS_KEY}` must be a non-empty string; using `git`");
                "git".to_owned()
            }
        }
    }

    /// `command` 의 실행 파일 버전. 파일이 그대로면 앞서 읽은 것을 쓴다.
    async fn version(&self, command: &GitCommand) -> Result<GitVersion, String> {
        let program = command.program().to_path_buf();
        let stamp = std::fs::metadata(&program)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        let cached = self.lock_versions().get(&program).cloned();
        if let Some(entry) = cached
            && stamp.is_some()
            && entry.stamp == stamp
        {
            return entry.version;
        }
        let cancel = CancellationToken::new();
        let timer = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(VERSION_TIMEOUT).await;
                cancel.cancel();
            })
        };
        let version = match command.clone().arg("--version").output(&cancel).await {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                GitVersion::parse(&text)
                    .ok_or_else(|| format!("unrecognized `git --version` output: {}", text.trim()))
            }
            Ok(out) => Err(format!(
                "`git --version` failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(e) => Err(format!("`git --version`: {e}")),
        };
        timer.abort();
        self.lock_versions().insert(
            program,
            VersionEntry {
                stamp,
                version: version.clone(),
            },
        );
        version
    }

    fn lock_versions(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, VersionEntry>> {
        self.versions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn warn_too_old(&self, program: &Path, version: &str) {
        tracing::warn!(target: "pw_git", program = %program.display(), version, required = %MIN_VERSION, "git is too old; writes are turned off");
        let first = self
            .warned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(program.to_path_buf());
        if !first {
            return;
        }
        if let Some(host) = self.host.get() {
            Notification::new(Level::Warning, TOO_OLD_MESSAGE)
                .arg("version", version)
                .arg(
                    "required",
                    format!("{}.{}", MIN_VERSION.major, MIN_VERSION.minor),
                )
                .body(program.display().to_string())
                .show(&host.bus, Scope::Host);
        }
    }
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

#[cfg(test)]
mod tests;
