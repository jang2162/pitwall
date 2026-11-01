//! 언어 서버 정의 · 설정 · 실행 파일 해석(`docs/design/15-m4-plan.md` M4-02).
//!
//! - 정의는 내장 언어 확장의 `[contributes.language_servers.<id>]` 다(파서 · 검사는 `pw_ext::manifest`).
//! - [`settings`]: `[lsp.<id>]` 설정(전역 · 프로젝트)을 정의 위에 합친다(결정 M4-D22).
//! - [`resolve`]: 실행 파일 해석 순서(결정 M4-D2 · D7)와 설치본을 찾는 [`ToolLocator`](설치기 M4-04 가 구현한다).
//! - [`rustup`]: rustup 프록시 판별(결정 M4-D7).
//! - [`methods`]: `lsp.servers.list` RPC.
//!
//! 서버를 띄우는 것은 서버 관리자(`pw-lsp`, M4-05)다 — 그쪽이 [`LspServers::setup`] 으로 합친 설정 · 해석 결과를 가져가고, 서버를
//! 돌릴 런타임(`requires`, 결정 M4-D6)을 찾고, 설정이 바뀌면 [`changed_servers`] 로 「다시 시작 필요」 를 낸다. 이 크레이트는 서버
//! 프로세스를 띄우지 않는다(rustup 확인의 `rustup --version` · `rustup which` 만 — 신뢰한 프로젝트에서).
//!
//! 신뢰: 미신뢰 프로젝트에서는 공유 레이어(`.pitwall/project.toml`)를 읽지 않고(`ConfigSource::without_shared` — 클론한 저장소가
//! 실행 파일 · 인자 · 환경을 고르지 못하게), 프로젝트 로컬 실행 파일 · `project_local_options` · `rustup which` 를 건너뛴다.

pub mod methods;
pub mod resolve;
pub mod rustup;
pub mod settings;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use pw_core::config::{AppPaths, ConfigSnapshot, ConfigStore};
use pw_core::registry::path::{WorktreeRootError, worktree_root};
use pw_core::registry::{Project, Registry as Projects, RegistryError};
use pw_ext::{LanguageServerDecl, LspServerRuntime, Registry};
use pw_rpc::{RpcError, code};
use pw_runtime::{Runtime, RuntimeEnv};
use serde_json::{Map, Value as Json};

pub use resolve::{
    Located, LspResolution, LspResolveStep, LspSkipped, NoTools, ResolveContext, ToolLocator,
};
pub use settings::{ConfiguredPath, LspSettingsOrigin, ServerConfig, ServerSettings};

use crate::methods::{LspRuntimeRequirement, LspServerInfo, LspServerList, LspServersListParams};

/// Host 를 만든 뒤에 붙이는 것(Host 의 메서드는 Host 를 만드는 중에 붙으므로).
#[derive(Clone)]
pub struct HostParts {
    pub paths: AppPaths,
    /// 전역 설정(`$CONFIG/settings.toml` · `settings.local.toml`).
    pub global: ConfigStore,
    pub projects: Projects,
}

pub struct LspServers {
    extensions: Arc<Registry>,
    runtime: Arc<Runtime>,
    tools: RwLock<Arc<dyn ToolLocator>>,
    host: OnceLock<HostParts>,
}

/// 한 프로젝트에서 서버 하나를 띄우는 데 필요한 것. 서버 관리자(M4-05)가 받는다.
///
/// 서버 환경(`env`) · 합친 설정은 비밀을 담을 수 있어 `Debug` 는 손으로 쓴다 — 값 대신 이름 · 키만 찍는다
/// (`docs/design/03-architecture.md` §5-1). 로그에는 이것을 `{:?}` 로 써도 된다.
#[derive(Clone, PartialEq)]
pub struct ServerSetup {
    /// 기여한 확장의 id.
    pub extension: String,
    pub definition: LanguageServerDecl,
    pub config: ServerConfig,
    /// `initialize` 의 `initializationOptions`: 정의의 값에, 신뢰한 프로젝트면 `project_local_options` 중 프로젝트에 있는 것의
    /// 절대 경로를 넣은 것. 객체다.
    pub initialization_options: Json,
    pub resolution: LspResolution,
    /// 서버 프로세스에 넘길 환경 전체: 그 디렉터리의 런타임 환경 위에 정의 · 설정의 `env` 를 덮은 것. 서버 관리자는 `requires`
    /// 로 찾은 런타임을 [`ServerSetup::with_runtime`] 으로 더한 뒤 이것을 그대로 준다(앱 프로세스의 환경을 따로 물려주지 않는다).
    pub env: RuntimeEnv,
}

impl std::fmt::Debug for ServerSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerSetup")
            .field("extension", &self.extension)
            .field("server", &self.definition.id)
            .field("command", &self.definition.command)
            .field("config", &self.config)
            .field(
                "initialization_options_keys",
                &settings::top_keys(&self.initialization_options),
            )
            .field("resolution", &self.resolution)
            .field("env", &self.env)
            .finish()
    }
}

/// 서버를 돌릴 런타임으로 찾은 설치본(결정 M4-D6 — 찾는 것은 서버 관리자 M4-05).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundRuntime {
    pub runtime: LspServerRuntime,
    /// 설치 폴더(java 면 `JAVA_HOME` 이 되는 곳).
    pub home: PathBuf,
    /// 실행 파일이 있는 폴더(`<home>/bin` 등). 서버 환경의 PATH 맨 앞에 들어간다.
    pub bin: PathBuf,
}

impl ServerSetup {
    /// `requires` 로 찾은 런타임을 서버 환경에 넣는다: PATH 맨 앞에 그 `bin`, java 면 `JAVA_HOME` 도 그 `home` 으로(결정 M4-D6 ·
    /// 메인 판정 2026-11-02). 그래서 PATH 에서 고른 런처(`jdtls` 등)도 프로젝트의 JDK 가 아니라 서버용 JDK 로 뜬다. 사람이
    /// `[lsp.<id>] env` 에 `JAVA_HOME` 을 적었으면 그것을 지킨다.
    pub fn with_runtime(&mut self, found: &FoundRuntime) {
        let mut dirs = vec![found.bin.clone()];
        dirs.extend(std::env::split_paths(&self.env.path).filter(|d| *d != found.bin));
        if let Ok(path) = std::env::join_paths(dirs) {
            self.env.path = path;
        }
        if found.runtime == LspServerRuntime::Java && !self.config.env.contains_key("JAVA_HOME") {
            self.env
                .vars
                .insert("JAVA_HOME".into(), found.home.clone().into_os_string());
        }
    }
}

/// 프로젝트 하나의 문맥.
struct ProjectView {
    project: Project,
    /// 프로젝트 루트(활성 워크트리면 그 안의 루트)의 실제 경로(심볼릭 링크를 푼 것). 해석 · 올라가기는 이 경로로 한다.
    root: PathBuf,
    trusted: bool,
    /// 프로젝트 설정을 읽지 못한 까닭. 그동안은 전역 설정만 쓴다.
    problem: Option<String>,
    project_layer: BTreeMap<String, ServerSettings>,
    global_layer: BTreeMap<String, ServerSettings>,
    warnings: Vec<String>,
}

impl LspServers {
    /// 설치본이 없는 상태로 만든다([`NoTools`]). 설치기가 붙으면 [`Self::set_tools`].
    pub fn new(extensions: Arc<Registry>, runtime: Arc<Runtime>) -> Self {
        Self {
            extensions,
            runtime,
            tools: RwLock::new(Arc::new(NoTools)),
            host: OnceLock::new(),
        }
    }

    /// 설치본을 찾는 것을 바꾼다(설치기 M4-04).
    pub fn set_tools(&self, tools: Arc<dyn ToolLocator>) {
        *self
            .tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = tools;
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.host.set(parts);
    }

    fn host(&self) -> Result<&HostParts, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("language servers are not attached to a host"))
    }

    fn tools(&self) -> Arc<dyn ToolLocator> {
        Arc::clone(
            &self
                .tools
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// 등록된 언어 서버 정의(확장 id, 정의). 등록 순서다(결정 M4-D12 의 순서).
    pub fn definitions(&self) -> impl Iterator<Item = (&str, &LanguageServerDecl)> {
        self.extensions
            .language_servers()
            .map(|(e, s)| (e.manifest.id.as_str(), s))
    }

    fn view(&self, project_id: &str, worktree: Option<&str>) -> Result<ProjectView, RpcError> {
        let host = self.host()?;
        let project = host.projects.get(project_id).map_err(registry_error)?;
        let root = active_root(&project, worktree)?;
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        let trusted = host.projects.is_trusted(&project.id);
        let mut warnings = Vec::new();
        let global = host.global.snapshot();
        let global_layer = settings::parse_layer(
            global.get("settings", settings::SETTINGS_KEY),
            "settings",
            &mut warnings,
        );
        let (project_layer, problem) = match project_snapshot(host, &project, &root, trusted) {
            Ok(snapshot) => (
                settings::parse_layer(
                    snapshot.get("project", settings::SETTINGS_KEY),
                    "project",
                    &mut warnings,
                ),
                None,
            ),
            Err(problem) => (BTreeMap::new(), Some(problem)),
        };
        for id in global_layer.keys().chain(project_layer.keys()) {
            if self.extensions.language_server(id).is_none() {
                warnings.push(format!(
                    "`[lsp.{id}]`: no language server `{id}` is registered; ignored"
                ));
            }
        }
        warnings.dedup();
        for w in &warnings {
            tracing::warn!(target: "pw_lsp_servers", project = %project.id, "{w}");
        }
        Ok(ProjectView {
            project,
            root,
            trusted,
            problem,
            project_layer,
            global_layer,
            warnings,
        })
    }

    fn config_in(view: &ProjectView, def: &LanguageServerDecl) -> ServerConfig {
        settings::server_config(
            def,
            view.global_layer.get(&def.id),
            view.project_layer.get(&def.id),
        )
    }

    fn setup_in(
        &self,
        view: &ProjectView,
        extension: &str,
        def: &LanguageServerDecl,
        dir: &Path,
    ) -> ServerSetup {
        let config = Self::config_in(view, def);
        let resolved = self.runtime.env(&view.project.id, dir);
        let tools = self.tools();
        let resolution = resolve::resolve(
            def,
            &config,
            &ResolveContext {
                trusted: view.trusted,
                project_root: &view.root,
                dir,
                env: &resolved.env,
                tools: tools.as_ref(),
            },
        );
        let mut env = resolved.env.clone();
        for (name, value) in &config.env {
            if name == "PATH" {
                env.path = value.into();
            } else {
                env.vars.insert(name.into(), value.into());
            }
        }
        ServerSetup {
            extension: extension.to_owned(),
            initialization_options: initialization_options(def, view, dir),
            definition: def.clone(),
            config,
            resolution,
            env,
        }
    }

    /// 서버 하나를 `dir`(작업 공간 루트 — 프로젝트 루트 안의 있는 폴더, 절대 경로)에서 띄우는 데 필요한 것. 해석은 그 디렉터리의
    /// 런타임 환경을 쓴다(처음이면 로그인 셸을 기다릴 수 있다 — 블로킹 스레드에서 부른다). 없는 서버 · 프로젝트, `..` · `.` 마디가
    /// 있거나 실제 경로(심볼릭 링크를 푼 것)가 프로젝트 루트 밖인 `dir` 은 invalid params 다 — 신뢰는 그 프로젝트를 허락한 것이지
    /// 그 위 폴더를 허락한 것이 아니다.
    pub fn setup(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        server_id: &str,
        dir: &Path,
    ) -> Result<ServerSetup, RpcError> {
        let view = self.view(project_id, worktree)?;
        let (extension, def) = self
            .extensions
            .language_server(server_id)
            .ok_or_else(|| RpcError::invalid_params(format!("no language server `{server_id}`")))?;
        let dir = dir_inside(&view.root, dir).map_err(|reason| {
            RpcError::invalid_params(format!(
                "`{}` {reason} (project root `{}`)",
                dir.display(),
                view.root.display()
            ))
        })?;
        Ok(self.setup_in(&view, &extension.manifest.id, def, &dir))
    }

    /// 서버마다의 합친 설정. 설정이 바뀌었을 때 [`changed_servers`] 로 견줄 값이다.
    pub fn configs(
        &self,
        project_id: &str,
        worktree: Option<&str>,
    ) -> Result<BTreeMap<String, ServerConfig>, RpcError> {
        let view = self.view(project_id, worktree)?;
        Ok(self
            .definitions()
            .map(|(_, def)| (def.id.clone(), Self::config_in(&view, def)))
            .collect())
    }

    /// `lsp.servers.list`. 해석은 프로젝트 루트(활성 워크트리면 그 안의 루트)에서 한다.
    pub fn list(&self, params: &LspServersListParams) -> Result<LspServerList, RpcError> {
        let view = self.view(&params.project_id, params.worktree.as_deref())?;
        let servers = self
            .definitions()
            .map(|(extension, def)| {
                let setup = self.setup_in(&view, extension, def, &view.root);
                LspServerInfo::new(setup)
            })
            .collect();
        Ok(LspServerList {
            servers,
            trusted: view.trusted,
            problem: view.problem,
            warnings: view.warnings,
        })
    }
}

impl LspServerInfo {
    fn new(setup: ServerSetup) -> Self {
        let ServerSetup {
            extension,
            definition: def,
            config,
            initialization_options,
            resolution,
            env: _,
        } = setup;
        Self {
            id: def.id,
            extension,
            languages: def.languages.into_iter().map(|l| l.name).collect(),
            mason: def.mason,
            command: def.command,
            args: config.args,
            env: config.env,
            root_markers: def.root_markers,
            requires: def
                .requires
                .into_iter()
                .map(|r| LspRuntimeRequirement {
                    runtime: r.runtime,
                    version: r.version.to_string(),
                })
                .collect(),
            disabled_features: config.disabled_features,
            enabled: config.enabled,
            settings: config.settings,
            initialization_options,
            resolution,
        }
    }
}

/// 설정이 바뀐 서버 id(전과 후 어느 쪽에만 있는 서버 포함). 서버 관리자(M4-05)가 `config.changed` 를 받으면 [`LspServers::configs`]
/// 를 다시 읽어 견주고, 여기 나온 서버의 인스턴스에 「다시 시작 필요」 를 낸다.
pub fn changed_servers(
    before: &BTreeMap<String, ServerConfig>,
    after: &BTreeMap<String, ServerConfig>,
) -> Vec<String> {
    let mut ids: Vec<&String> = before.keys().chain(after.keys()).collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .filter(|id| before.get(*id) != after.get(*id))
        .cloned()
        .collect()
}

/// 프로젝트 설정. 디스크에 쓰지 않고 읽는다 — 목록만 봤다고 커밋된 파일이 마이그레이션되면 안 된다. 미신뢰면 공유 레이어를 뺀다.
fn project_snapshot(
    host: &HostParts,
    project: &Project,
    root: &Path,
    trusted: bool,
) -> Result<ConfigSnapshot, String> {
    let source = project
        .config_source(&host.paths, root)
        .map_err(|e| e.to_string())?
        .without_writes();
    let source = if trusted {
        source
    } else {
        source.without_shared()
    };
    source.load().map_err(|e| e.to_string())
}

/// 정의의 `initialization_options` 에, 신뢰한 프로젝트면 `project_local_options` 중 있는 것의 절대 경로를 넣는다. 찾는 곳은
/// 프로젝트 로컬 실행 파일과 같다 — `dir` 에서 프로젝트 루트까지 올라가며 처음 있는 것.
fn initialization_options(def: &LanguageServerDecl, view: &ProjectView, dir: &Path) -> Json {
    let mut options = def
        .initialization_options
        .clone()
        .unwrap_or_else(|| Json::Object(Map::new()));
    if !view.trusted {
        return options;
    }
    let dirs = resolve::dirs_up_to_root(&view.root, dir);
    for (key, rel) in &def.project_local_options {
        let found = dirs
            .iter()
            .map(|d| d.join(rel.as_str()))
            .find(|p| p.exists());
        if let Some(found) = found {
            settings::set_path(&mut options, key, Json::String(found.display().to_string()));
        }
    }
    options
}

/// `dir` 이 `root`(실제 경로) 안의 있는 폴더면 그 실제 경로. `..` · `.` 마디는 어휘로 거절하고(`Path::starts_with` 는 그것을
/// 풀지 않는다), 심볼릭 링크는 실제 경로로 견준다.
fn dir_inside(root: &Path, dir: &Path) -> Result<PathBuf, &'static str> {
    if !dir.is_absolute() {
        return Err("is not an absolute path");
    }
    // `Path::components` 는 가운데의 `.` 을 지워 버리므로 바이트로 마디를 나눈다.
    let separator = |b: &u8| *b == b'/' || (cfg!(windows) && *b == b'\\');
    if dir
        .as_os_str()
        .as_encoded_bytes()
        .split(separator)
        .any(|segment| segment == b"." || segment == b"..")
    {
        return Err("has a `..` or `.` segment");
    }
    let real = std::fs::canonicalize(dir).map_err(|_| "does not exist")?;
    if !real.is_dir() {
        return Err("is not a folder");
    }
    if !real.starts_with(root) {
        return Err("is not inside the project root");
    }
    Ok(real)
}

/// 활성 워크트리(`ActiveWorktree::top`) 안의 프로젝트 루트. 없으면 주 루트. 다른 프로젝트 RPC(`fs.*` · `openWith.*` · `db.*`)와
/// 같은 계산이다.
fn active_root(project: &Project, worktree: Option<&str>) -> Result<PathBuf, RpcError> {
    let Some(top) = worktree else {
        return Ok(project.root().to_path_buf());
    };
    worktree_root(project.root(), Path::new(top)).map_err(|e| match e {
        WorktreeRootError::Relative => {
            RpcError::invalid_params(format!("worktree `{top}` must be an absolute path"))
        }
        WorktreeRootError::NotOurs => RpcError::invalid_params(format!(
            "`{top}` is not a worktree of project `{}`",
            project.id
        )),
    })
}

fn registry_error(e: RegistryError) -> RpcError {
    match e {
        RegistryError::Db(_) => RpcError::internal(e.to_string()),
        RegistryError::Unavailable(_) => {
            RpcError::new(code::STORAGE_UNAVAILABLE, "storage unavailable")
                .with_data(Json::String(e.to_string()))
        }
        _ => RpcError::invalid_params(e.to_string()),
    }
}

#[cfg(all(test, unix))]
mod tests;
