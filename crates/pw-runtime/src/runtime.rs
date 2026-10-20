//! 앱 전체가 함께 쓰는 런타임 환경: (프로젝트, 디렉터리)별 캐시, 「환경 다시 읽기」, `runtime.changed`(03 §5-1 「캐시 · 무효화」).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use pw_core::registry::path::{normalize, worktree_link};
use pw_core::registry::{Project, Registry as Projects, RegistryError};
use pw_core::{EventBus, Scope};
use pw_rpc::{RpcError, code};
use serde::Serialize;

use crate::cache::Cache;
use crate::env::RuntimeEnv;
use crate::login_shell::LoginShell;
use crate::methods::{
    RuntimeGetParams, RuntimeInfo, RuntimeSource, RuntimeStatus, RuntimeToolInfo,
};
use crate::provider::{Layer, RuntimeProvider};

/// 런타임 환경이 바뀌었다. 새 터미널 · 새로 띄우는 프로세스부터 새 환경을 쓴다.
pub const TOPIC_CHANGED: &str = "runtime.changed";

/// `runtime.changed` 의 내용.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeChanged {
    pub reason: ChangeReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeReason {
    /// 사용자가 「환경 다시 읽기」(`runtime.reload`)를 했다.
    Reload,
    /// 프로젝트의 신뢰가 바뀌었다([`Runtime::trust_changed`]). 프로젝트 코드를 돌리는 제공자가 붙거나 빠진다.
    Trust,
    /// 시간을 넘겼던 로그인 셸을 백그라운드로 다시 돌려 환경을 얻었다(`login_shell` 머리 주석). 물러난 환경을 받았던 프로젝트에 나간다.
    LoginShell,
}

/// 한 디렉터리의 환경과 그 설명.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub env: RuntimeEnv,
    pub info: RuntimeInfo,
    /// PATH 의 바탕이 로그인 셸의 것인가(아니면 앱 프로세스의 것).
    pub login_shell: bool,
}

/// Host 를 만든 뒤에 붙이는 것(Host 의 메서드는 Host 를 만드는 중에 붙으므로).
#[derive(Clone)]
pub struct RuntimeHost {
    pub projects: Projects,
    pub bus: EventBus,
}

type TrustFn = dyn Fn(&str) -> bool + Send + Sync;

pub struct Runtime {
    shell: Arc<LoginShell>,
    /// 로그인 셸 위에 덮는 제공자. 앞의 것부터 감지해 처음 맞는 하나를 쓴다.
    providers: Vec<Arc<dyn RuntimeProvider>>,
    /// 프로젝트를 신뢰하는가(F1-7). 신뢰 전에는 프로젝트 코드를 돌리는 제공자를 고르지 않는다.
    trusted: Box<TrustFn>,
    /// 키는 (프로젝트, 디렉터리, 계산할 때 판정한 신뢰). 꺼낼 때마다 지금 신뢰로 키를 만들므로 신뢰가 바뀌면 — 신뢰를 가진
    /// 프로젝트가 바뀐 경우(메인 저장소 프로젝트를 등록 · 삭제)나 그 이벤트를 놓친 경우도 — 옛 신뢰로 만든 환경을 주지 않는다.
    cache: Cache<(String, PathBuf, bool), Resolved>,
    /// 로그인 셸 없이 물러난 환경을 받은 프로젝트. 셸이 백그라운드 재시도로 살아나면 이들에 `runtime.changed` 를 낸다.
    fell_back: Arc<Mutex<BTreeSet<String>>>,
    host: OnceLock<RuntimeHost>,
}

impl Runtime {
    /// 로그인 셸만 쓰는 런타임. 신뢰한 프로젝트는 아직 없다.
    pub fn new(shell: LoginShell) -> Self {
        Self {
            shell: Arc::new(shell),
            providers: Vec::new(),
            trusted: Box::new(|_| false),
            cache: Cache::default(),
            fell_back: Arc::default(),
            host: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn with_provider(mut self, provider: Arc<dyn RuntimeProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    #[must_use]
    pub fn with_trust(mut self, trusted: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        self.trusted = Box::new(trusted);
        self
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, host: RuntimeHost) {
        let bus = host.bus.clone();
        if self.host.set(host).is_err() {
            return;
        }
        let fell_back = Arc::clone(&self.fell_back);
        self.shell.on_recovered(move || {
            let projects =
                std::mem::take(&mut *fell_back.lock().unwrap_or_else(PoisonError::into_inner));
            tracing::info!(target: "pw_runtime", ?projects, "login shell environment recovered");
            announce(&bus, projects, ChangeReason::LoginShell);
        });
    }

    /// `project` 의 `dir`(절대 경로)에서 띄울 프로세스의 환경. 처음이면 지금 만든다(로그인 셸을 기다릴 수 있다). 물러난
    /// 결과(`RuntimeStatus::Fallback`)는 남기지 않아 다음 호출이 다시 시도한다(로그인 셸이 한 번 시간을 넘긴 뒤로는 기다리지
    /// 않고 곧바로 물러난다).
    pub fn env(&self, project: &str, dir: &Path) -> Arc<Resolved> {
        let trusted = (self.trusted)(project);
        let resolved = self.cache.get_or_compute(
            &(project.to_owned(), dir.to_path_buf(), trusted),
            || self.resolve(dir, trusted),
            |r| r.info.status == RuntimeStatus::Ok,
        );
        if !resolved.login_shell {
            self.fell_back
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(project.to_owned());
        }
        resolved
    }

    /// 로그인 셸 환경 가져오기를 백그라운드로 시작한다. 이미 있으면 아무것도 하지 않는다.
    pub fn prefetch(&self) {
        if self.shell.is_ready() {
            return;
        }
        let shell = Arc::clone(&self.shell);
        let spawned = std::thread::Builder::new()
            .name("login-shell-env".to_owned())
            .spawn(move || {
                shell.get();
            });
        if let Err(e) = spawned {
            tracing::warn!(target: "pw_runtime", %e, "cannot prefetch the login shell environment");
        }
    }

    /// 「환경 다시 읽기」. 로그인 셸 환경과 모든 디렉터리의 캐시를 잊고, 캐시가 있던 프로젝트와 `project` 마다
    /// `runtime.changed` 를 한 번씩 낸다(로그인 셸은 모든 프로젝트의 바탕이다). 시간 초과의 냉각도 푼다. 다시 읽기는 다음
    /// 요청에서 한다(RPC 핸들러는 곧바로 백그라운드로 시작한다).
    pub fn reload(&self, project: &str) {
        self.reload_with(Some(project));
    }

    /// 부른 프로젝트가 없는 「환경 다시 읽기」: 캐시가 있던 프로젝트에만 `runtime.changed` 가 나간다. 무엇이 바뀌었는지 모를 때
    /// 쓴다(레지스트리 이벤트를 놓쳤을 때 등).
    pub fn reload_all(&self) {
        self.reload_with(None);
    }

    fn reload_with(&self, project: Option<&str>) {
        self.shell.invalidate();
        let mut projects: Vec<String> = self.cache.clear().into_iter().map(|(p, ..)| p).collect();
        projects.extend(project.map(str::to_owned));
        projects.sort();
        projects.dedup();
        tracing::info!(target: "pw_runtime", ?projects, "runtime environment reload");
        self.announce(projects, ChangeReason::Reload);
    }

    /// 신뢰가 바뀐 프로젝트들(신뢰를 가진 프로젝트와 그 신뢰를 따르는 워크트리 프로젝트)의 디렉터리 캐시만 잊고 각각
    /// `runtime.changed`(`trust`)를 낸다. 로그인 셸 환경과 다른 프로젝트의 캐시는 그대로다.
    pub fn trust_changed(&self, projects: &[String]) {
        self.cache.forget(|(p, ..)| projects.contains(p));
        tracing::info!(target: "pw_runtime", ?projects, "project trust changed");
        self.announce(projects.to_vec(), ChangeReason::Trust);
    }

    fn announce(&self, projects: Vec<String>, reason: ChangeReason) {
        if let Some(host) = self.host.get() {
            announce(&host.bus, projects, reason);
        }
    }

    /// 디렉터리의 제공자 없이 로그인 셸의 환경. 셸을 읽지 못했으면 앱 프로세스의 환경과 그 이유. 다른 앱이 로그인 셸 PATH 로
    /// 무엇을 찾을지 볼 때 쓴다(Orca 의 「Open in」, `crates/pw-open-with` 의 `cli.status`). 처음이면 셸을 기다린다.
    pub fn login_shell_env(&self) -> (Layer, Option<String>) {
        match &*self.shell.get() {
            Ok(layer) => (layer.clone(), None),
            Err(e) => (
                Layer {
                    env: RuntimeEnv::process(),
                    source: RuntimeSource::Process,
                },
                Some(e.reason.clone()),
            ),
        }
    }

    fn resolve(&self, dir: &Path, trusted: bool) -> Resolved {
        let (base, failure) = self.login_shell_env();
        let mut failures: Vec<String> = failure.into_iter().collect();
        let login_shell = failures.is_empty();
        let picked = self
            .providers
            .iter()
            .find(|p| (trusted || !p.runs_project_code()) && p.applies_to(dir, &base.env));
        let (provider, layer) = match picked {
            None => (LoginShell::ID, base),
            Some(p) => match p.resolve(dir, &base.env) {
                Ok(layer) => (p.id(), layer),
                Err(reason) => {
                    tracing::warn!(target: "pw_runtime", provider = p.id(), %reason, dir = %dir.display(), "runtime provider failed");
                    failures.push(reason);
                    (p.id(), base)
                }
            },
        };
        let status = if failures.is_empty() {
            RuntimeStatus::Ok
        } else {
            RuntimeStatus::Fallback {
                reason: failures.join("; "),
            }
        };
        let tools = layer
            .env
            .tools
            .iter()
            .map(|t| RuntimeToolInfo {
                name: t.name.clone(),
                version: t.version.clone(),
                source_file: t
                    .source_file
                    .as_ref()
                    .map(|f| f.to_string_lossy().into_owned()),
            })
            .collect();
        Resolved {
            env: layer.env,
            info: RuntimeInfo {
                provider: provider.to_owned(),
                source: layer.source,
                status,
                tools,
            },
            login_shell,
        }
    }

    fn projects(&self) -> Result<&Projects, RpcError> {
        self.host
            .get()
            .map(|h| &h.projects)
            .ok_or_else(|| RpcError::internal("runtime is not attached to a host"))
    }

    /// `runtime.get`. `dir` 이 없으면 프로젝트의 주 루트. `dir` 은 레지스트리와 같은 규칙으로 정규화하고(캐시 키가 `/a/../a` 와
    /// `/a` 로 갈리지 않게), 프로젝트 루트나 그 루트를 담은 연결 워크트리 안이어야 한다 — 제공자가 호출자가 고른 아무 폴더에서
    /// 돌지 않게.
    pub fn get(&self, params: &RuntimeGetParams) -> Result<RuntimeInfo, RpcError> {
        let project = self
            .projects()?
            .get(&params.project_id)
            .map_err(registry_error)?;
        let dir = match &params.dir {
            Some(dir) => project_dir(&project, Path::new(dir))?,
            None => normalize(project.root()).unwrap_or_else(|_| project.root().to_path_buf()),
        };
        Ok(self.env(&project.id, &dir).info.clone())
    }

    /// `runtime.reload`. 없는 프로젝트는 invalid params 오류.
    pub fn reload_project(&self, project_id: &str) -> Result<(), RpcError> {
        let project = self.projects()?.get(project_id).map_err(registry_error)?;
        self.reload(&project.id);
        Ok(())
    }
}

fn announce(bus: &EventBus, projects: impl IntoIterator<Item = String>, reason: ChangeReason) {
    let changed = RuntimeChanged { reason };
    for p in projects {
        if let Err(e) = bus.publish(Scope::Project(p), TOPIC_CHANGED, &changed) {
            tracing::warn!(target: "pw_runtime", %e, "cannot publish runtime.changed");
        }
    }
}

/// `dir` 을 정규화한 것. 프로젝트의 루트 안이거나, 연결 워크트리 안이면서 메인 체크아웃 쪽 대응 경로가 루트 안이어야 한다.
fn project_dir(project: &Project, dir: &Path) -> Result<PathBuf, RpcError> {
    let dir = normalize(dir).map_err(|_| {
        RpcError::invalid_params(format!("dir `{}` must be an absolute path", dir.display()))
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
            "dir `{}` is not inside project `{}`",
            dir.display(),
            project.id
        )))
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
