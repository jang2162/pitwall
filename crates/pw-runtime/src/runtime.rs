//! 앱 전체가 함께 쓰는 런타임 환경: (프로젝트, 디렉터리)별 캐시, 「환경 다시 읽기」, `runtime.changed`(03 §5-1 「캐시 · 무효화」).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use pw_core::registry::{Registry as Projects, RegistryError};
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
    cache: Cache<(String, PathBuf), Resolved>,
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
        let _ = self.host.set(host);
    }

    /// `project` 의 `dir`(절대 경로)에서 띄울 프로세스의 환경. 처음이면 지금 만든다(로그인 셸을 기다릴 수 있다). 물러난
    /// 결과(`RuntimeStatus::Fallback`)는 남기지 않아 다음 호출이 다시 시도한다.
    pub fn env(&self, project: &str, dir: &Path) -> Arc<Resolved> {
        self.cache.get_or_compute(
            &(project.to_owned(), dir.to_path_buf()),
            || self.resolve(project, dir),
            |r| r.info.status == RuntimeStatus::Ok,
        )
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
    /// `runtime.changed` 를 한 번씩 낸다(로그인 셸은 모든 프로젝트의 바탕이다). 다시 읽기는 다음 요청에서 한다.
    pub fn reload(&self, project: &str) {
        self.shell.invalidate();
        let mut projects: Vec<String> = self.cache.clear().into_iter().map(|(p, _)| p).collect();
        projects.push(project.to_owned());
        projects.sort();
        projects.dedup();
        tracing::info!(target: "pw_runtime", ?projects, "runtime environment reload");
        let Some(host) = self.host.get() else {
            return;
        };
        let changed = RuntimeChanged {
            reason: ChangeReason::Reload,
        };
        for p in projects {
            if let Err(e) = host.bus.publish(Scope::Project(p), TOPIC_CHANGED, &changed) {
                tracing::warn!(target: "pw_runtime", %e, "cannot publish runtime.changed");
            }
        }
    }

    fn resolve(&self, project: &str, dir: &Path) -> Resolved {
        let (base, mut failures) = match &*self.shell.get() {
            Ok(layer) => (layer.clone(), Vec::new()),
            Err(reason) => (
                Layer {
                    env: RuntimeEnv::process(),
                    source: RuntimeSource::Process,
                },
                vec![reason.clone()],
            ),
        };
        let login_shell = failures.is_empty();
        let trusted = (self.trusted)(project);
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

    /// `runtime.get`. `dir` 이 없으면 프로젝트의 주 루트.
    pub fn get(&self, params: &RuntimeGetParams) -> Result<RuntimeInfo, RpcError> {
        let project = self
            .projects()?
            .get(&params.project_id)
            .map_err(registry_error)?;
        let dir = match &params.dir {
            Some(dir) if !Path::new(dir).is_absolute() => {
                return Err(RpcError::invalid_params(format!(
                    "dir `{dir}` must be an absolute path"
                )));
            }
            Some(dir) => PathBuf::from(dir),
            None => project.root().to_path_buf(),
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
