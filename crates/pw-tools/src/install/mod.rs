//! 도구 설치기(15-m4-plan.md M4-04 · 결정 M4-D17 · D18, 07-extensibility.md §4.4).
//!
//! - [`grant`]: 설치 허가 [`ToolGrant`](설정 창 · 그 프로젝트 창만).
//! - [`consent`]: 라이선스 · 출처 확인 기록과 라이선스 덮어쓰기 표.
//! - [`extract`]: 받은 파일 풀기(폴더 밖 · 링크 · 같은 이름 · 상한).
//! - [`layout`]: `$DATA/tools` 배치 · 설치 기록 · 실행 링크 · 설치본 찾기([`InstalledTools`] — M4-02 의 `ToolLocator`).
//! - [`runner`]: npm · pip 을 격리해 부르는 자리.
//! - [`methods`]: `tools.list` · `install` · `update` · `rollback` · `uninstall` · `log` · `updates` RPC.
//!
//! 설치 한 번의 흐름([`ToolsInstaller::install`]): 정의를 이 기기 대상으로 풀고 → 확인 기록을 보고(없거나 라이선스 · 출처가 바뀌었으면
//! [`code::CONFIRM_REQUIRED`]) → npm · python 이 필요하면 런타임 환경에서 찾고(없으면 [`code::RUNTIME_MISSING`]) → 같은 패키지의
//! 설치가 이미 돌면 그 결과를 기다리고, 아니면 `Task` 를 띄운다. 작업은 잠금 파일을 잡고(다른 프로세스의 앱 포함 하나만) 새 판의
//! 폴더를 만들어 받고 · 확인하고 · 풀고, 다 되면 설치 기록과 실행 링크만 바꾼다. 실패 · 취소면 그 판의 폴더를 지운다.

pub mod consent;
pub mod extract;
pub mod grant;
pub mod layout;
pub mod methods;
pub mod runner;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pw_core::task::{TaskCtx, TaskProgress, TaskSpawner, TaskStatus};
use pw_rpc::{RpcError, code as rpc_code};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::watch;

pub use consent::{ConsentEntry, ConsentStore, LicenseNotes};
pub use grant::{ToolGrant, tool_grant};
pub use layout::{InstallRecord, InstalledTools, InstalledVersion, Layout};
pub use runner::{ProcessToolchains, RuntimeToolchains, Toolchains};

use crate::fetch::{FetchError, Fetcher};
use crate::methods::ToolChecksum;
use crate::package::{PurlKind, ResolvedTool, ToolBin, ToolSource};
use crate::pinned::{Artifact, normalize_pypi};
use crate::registry::ToolsRegistry;
use crate::target::ToolTarget;
use extract::Unpacker;
use layout::{FetchedFile, INSTALLING_MARK, UsedRuntime, dir_size};

/// 설치기의 애플리케이션 오류 코드(`data` 는 객체 — 15-m4-plan.md §2 「오류 코드」: `pw-tools` 4001~).
pub mod code {
    /// 라이선스 · 출처 확인이 필요하다(`data` 는 [`super::methods::ToolConfirmRequired`]). 창은 그 값을 보이고 확인을 받은 뒤
    /// `confirmed: { version, licenses, sourceDomains }` 로 다시 부른다.
    pub const CONFIRM_REQUIRED: i32 = 4001;
    /// 설치에 필요한 런타임(node · python)이 런타임 환경에 없다. `data`: `runtime`(`node` · `python`) · `hint`.
    pub const RUNTIME_MISSING: i32 = 4002;
    /// 설치가 실패했다(받기 · 해시 · 풀기 · npm · pip). `data`: `stage` · `name`. 그 판의 폴더는 지웠다.
    pub const INSTALL_FAILED: i32 = 4003;
    /// 이 기기에서 설치할 수 없는 패키지. `data`: `reason`.
    pub const UNSUPPORTED: i32 = 4004;
    /// 그 패키지의 설치 · 되돌리기 · 제거가 이미 돈다(다른 창 · 다른 앱 프로세스 포함).
    pub const BUSY: i32 = 4005;
    /// 설치되지 않았다(업데이트 · 되돌리기 · 제거 · 로그), 또는 되돌릴 이전 판이 없다. `data`: `name`.
    pub const NOT_INSTALLED: i32 = 4006;
    /// 설치 작업이 취소됐다. 그 판의 폴더는 지웠고 지금 판 · 링크는 그대로다.
    pub const CANCELLED: i32 = 4007;
}

/// 설치 하나의 크기 상한(결정 M4-D17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// 받는 바이트(설치기가 직접 받는 파일의 합) — 1 GiB.
    pub download: u64,
    /// 풀린 바이트 — 2 GiB.
    pub unpacked: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            download: 1 << 30,
            unpacked: 2 << 30,
        }
    }
}

/// 제거 전에 그 패키지를 쓰는 서버를 멈추는 곳(서버 관리자 M4-05 가 붙인다).
pub trait ToolUsers: Send + Sync {
    /// `package` 의 실행 파일을 쓰는 프로세스를 모두 끝낸다(끝날 때까지 기다린다).
    fn stop(&self, package: &str) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

struct NoUsers;

impl ToolUsers for NoUsers {
    fn stop(&self, _package: &str) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

/// 설치 기록이 있는 패키지 하나와 그 기록(읽지 못했으면 까닭).
pub type InstalledEntry = (String, Result<InstallRecord, String>);

/// 창 label 이 설정 창인가.
pub type SettingsWindow = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Host 를 만든 뒤 붙이는 것.
#[derive(Clone)]
pub struct InstallerHost {
    /// `$DATA/tools`.
    pub tools_dir: PathBuf,
    /// `$STATE`(확인 기록 `tools/consent.json`, 로그 `logs/tools/<패키지>.log`).
    pub state_dir: PathBuf,
    pub tasks: TaskSpawner,
    pub toolchains: Arc<dyn Toolchains>,
    pub settings_window: SettingsWindow,
}

/// 설치 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum InstallOutcome {
    /// 새로 받아 설치했다.
    Installed,
    /// 그 판이 이미 설치돼 있었다(다른 앱 프로세스가 먼저 끝낸 경우 포함) — 받지 않았다.
    AlreadyInstalled,
    /// 남겨 둔 이전 판이 그 판이라 링크만 돌렸다 — 받지 않았다.
    Switched,
}

/// 끝난 설치.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub version: String,
    pub previous: Option<String>,
    pub outcome: InstallOutcome,
    pub checksum: ToolChecksum,
}

/// 도는 설치 하나. 같은 패키지를 부른 쪽이 모두 이 결과를 받는다.
struct Job {
    task_id: u64,
    version: String,
    outcome: watch::Receiver<Option<Result<Installed, RpcError>>>,
}

/// 설치에 필요한 런타임(작업을 띄우기 전에 찾는다).
#[derive(Debug, Clone)]
enum Toolchain {
    None,
    Npm {
        found: runner::Found,
        base: pw_runtime::RuntimeEnv,
    },
    Python {
        found: runner::Found,
        base: pw_runtime::RuntimeEnv,
    },
}

/// 설치 계획(확인을 지난 뒤 작업이 받는 것).
#[derive(Clone)]
struct Plan {
    resolved: ResolvedTool,
    pinned: Option<Vec<Artifact>>,
    licenses: Vec<String>,
    registry_version: String,
    toolchain: Toolchain,
}

pub struct ToolsInstaller {
    registry: Arc<ToolsRegistry>,
    notes: LicenseNotes,
    limits: Limits,
    host: OnceLock<InstallerHost>,
    fetcher: OnceLock<Result<Fetcher, String>>,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    /// 되돌리기 · 제거가 도는 패키지(같은 프로세스).
    busy: Mutex<BTreeSet<String>>,
    users: RwLock<Arc<dyn ToolUsers>>,
    #[cfg(test)]
    test: Option<TestNetwork>,
}

#[cfg(test)]
#[derive(Clone)]
struct TestNetwork {
    fetcher: Fetcher,
    /// 정의의 `https://<앞>` 을 가짜 서버로 바꾼다.
    rewrite: (String, String),
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn failed(stage: &'static str, name: &str, message: impl Into<String>) -> RpcError {
    RpcError::new(code::INSTALL_FAILED, message).with_data(json!({ "stage": stage, "name": name }))
}

fn not_installed(name: &str, message: &str) -> RpcError {
    RpcError::new(code::NOT_INSTALLED, message.to_owned()).with_data(json!({ "name": name }))
}

fn busy(name: &str) -> RpcError {
    RpcError::new(code::BUSY, "another install of this tool is running")
        .with_data(json!({ "name": name }))
}

/// 패키지가 받는 곳의 도메인(확인 다이얼로그 · 기록). 이름순.
pub fn source_domains(resolved: &ResolvedTool) -> Vec<String> {
    let mut out = BTreeSet::new();
    match &resolved.source {
        ToolSource::Files { files } => {
            for f in files {
                if let Ok(u) = url::Url::parse(&f.url)
                    && let Some(h) = u.host_str()
                {
                    out.insert(h.to_ascii_lowercase());
                }
            }
        }
        ToolSource::Npm { .. } => {
            out.insert("registry.npmjs.org".to_owned());
        }
        ToolSource::Pypi { .. } => {
            out.insert("pypi.org".to_owned());
            out.insert("files.pythonhosted.org".to_owned());
        }
    }
    out.into_iter().collect()
}

impl ToolsInstaller {
    pub fn new(registry: Arc<ToolsRegistry>) -> Self {
        ToolsInstaller::with_limits(registry, Limits::default())
    }

    pub fn with_limits(registry: Arc<ToolsRegistry>, limits: Limits) -> Self {
        ToolsInstaller {
            registry,
            notes: LicenseNotes::builtin(),
            limits,
            host: OnceLock::new(),
            fetcher: OnceLock::new(),
            jobs: Mutex::new(HashMap::new()),
            busy: Mutex::new(BTreeSet::new()),
            users: RwLock::new(Arc::new(NoUsers)),
            #[cfg(test)]
            test: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_network(mut self, fetcher: Fetcher, rewrite: (String, String)) -> Self {
        self.test = Some(TestNetwork { fetcher, rewrite });
        self
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, host: InstallerHost) {
        let _ = self.host.set(host);
    }

    /// 제거 전에 그 패키지를 쓰는 서버를 멈추는 곳(M4-05).
    pub fn set_users(&self, users: Arc<dyn ToolUsers>) {
        *self.users.write().unwrap_or_else(PoisonError::into_inner) = users;
    }

    /// 설치본 찾기(`LspServers::set_tools` 에 넘긴다). 붙기 전이면 `None`.
    pub fn locator(&self) -> Option<InstalledTools> {
        self.host
            .get()
            .map(|h| InstalledTools::new(Layout::new(h.tools_dir.clone())))
    }

    pub fn registry(&self) -> &Arc<ToolsRegistry> {
        &self.registry
    }

    pub fn notes(&self) -> &LicenseNotes {
        &self.notes
    }

    fn host(&self) -> Result<&InstallerHost, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("the tool installer is not attached to a host"))
    }

    fn layout(&self) -> Result<Layout, RpcError> {
        Ok(Layout::new(self.host()?.tools_dir.clone()))
    }

    fn consent(&self) -> Result<ConsentStore, RpcError> {
        Ok(ConsentStore::new(&self.host()?.state_dir))
    }

    fn log_path(&self, name: &str) -> Result<PathBuf, RpcError> {
        Ok(self
            .host()?
            .state_dir
            .join("logs")
            .join("tools")
            .join(format!("{name}.log")))
    }

    fn fetcher(&self) -> Result<Fetcher, String> {
        #[cfg(test)]
        if let Some(t) = &self.test {
            return Ok(t.fetcher.clone());
        }
        self.fetcher
            .get_or_init(|| Fetcher::for_install().map_err(|e| e.to_string()))
            .clone()
    }

    fn fetch_url(&self, url: &str) -> String {
        #[cfg(test)]
        if let Some(t) = &self.test
            && let Some(rest) = url.strip_prefix(&t.rewrite.0)
        {
            return format!("{}{rest}", t.rewrite.1);
        }
        url.to_owned()
    }

    /// 이 기기에서 그 패키지가 받을 것 · 실행할 것과 그 확인 값.
    fn resolve(&self, name: &str) -> Result<(ResolvedTool, Vec<String>, String), RpcError> {
        // `bin` 은 실행 링크 폴더(`$DATA/tools/bin`)와 겹친다.
        if !crate::package::valid_file_name(name) || name.len() > 256 || name == "bin" {
            return Err(RpcError::invalid_params("not a package name"));
        }
        let index = self.registry.index();
        let package = index.get(name).ok_or_else(|| {
            RpcError::new(rpc_code::NOT_FOUND, "not found").with_data(json!(name))
        })?;
        let resolved = package
            .resolve(ToolTarget::current(), self.registry.overrides().get(name))
            .map_err(|e| {
                RpcError::new(code::UNSUPPORTED, e.to_string())
                    .with_data(json!({ "reason": e.reason(), "name": name }))
            })?;
        if let Some((bin, _)) = resolved
            .bin
            .iter()
            .find(|(_, b)| matches!(b, ToolBin::Python(_)))
        {
            return Err(RpcError::new(
                code::UNSUPPORTED,
                format!("bin `{bin}` runs a python script — the installer does not make launchers for it yet"),
            )
            .with_data(json!({ "reason": "bin", "name": name })));
        }
        let licenses = self.notes.effective(name, &package.licenses);
        Ok((resolved, licenses, index.version.clone()))
    }

    /// 확인 다이얼로그에 보일 값. 확인 기록이 덮으면 `None`.
    pub fn confirm_values(
        &self,
        resolved: &ResolvedTool,
        licenses: &[String],
        pinned: Option<&[&Artifact]>,
    ) -> methods::ToolConfirmRequired {
        methods::ToolConfirmRequired {
            name: resolved.name.clone(),
            version: resolved.version.clone(),
            licenses: licenses.to_vec(),
            source_domains: source_domains(resolved),
            size: pinned.map(|rows| rows.iter().map(|a| a.size).sum()),
            checksum: if pinned.is_some() {
                ToolChecksum::Pinned
            } else {
                ToolChecksum::None
            },
            note: self.notes.get(&resolved.name).map(|n| n.note.clone()),
            rejected: None,
        }
    }

    /// 설치(또는 업데이트). 확인 · 런타임 · 같은 패키지 설치 합류를 거쳐 작업을 기다린다.
    pub async fn install(
        self: &Arc<Self>,
        grant: &ToolGrant,
        name: &str,
        version: Option<&str>,
        confirmed: Option<&methods::ToolConfirmed>,
        update_only: bool,
    ) -> Result<methods::ToolInstallResult, RpcError> {
        let host = self.host()?.clone();
        self.registry.ensure().await;
        let (resolved, licenses, registry_version) = self.resolve(name)?;
        if let Some(v) = version
            && v != resolved.version
        {
            return Err(RpcError::invalid_params(format!(
                "only the registry version {} can be installed",
                resolved.version
            )));
        }
        let layout = Layout::new(host.tools_dir.clone());
        let record = layout
            .read_record(name)
            .map_err(|e| failed("record", name, e))?;
        if update_only && record.is_none() {
            return Err(not_installed(name, "the tool is not installed"));
        }
        // 이미 그 판이면(그리고 쓸 수 있으면) 받지 않는다 — 확인도 묻지 않는다.
        if let Some(r) = &record
            && r.current == resolved.version
            && entry_problem(&layout, name, r).is_none()
        {
            return Ok(methods::ToolInstallResult {
                name: name.to_owned(),
                version: r.current.clone(),
                previous: r.previous.clone(),
                outcome: InstallOutcome::AlreadyInstalled,
                checksum: r.current_entry().map_or(ToolChecksum::None, |e| e.checksum),
                task_id: None,
                joined: false,
            });
        }

        let pinned_owned: Option<Vec<Artifact>> = self
            .registry
            .pinned()
            .pinned_for(&resolved)
            .map(|rows| rows.into_iter().cloned().collect());
        let pinned_refs: Option<Vec<&Artifact>> = pinned_owned.as_ref().map(|v| v.iter().collect());
        let mut ask = self.confirm_values(&resolved, &licenses, pinned_refs.as_deref());
        let store = ConsentStore::new(&host.state_dir);
        let consented = store
            .get(name)
            .is_some_and(|c| consent::covers(&c, &ask.licenses, &ask.source_domains));
        if !consented {
            match confirmed {
                Some(c)
                    if c.version == ask.version
                        && consent::covers(
                            &ConsentEntry {
                                licenses: c.licenses.clone(),
                                source_domains: c.source_domains.clone(),
                                version: c.version.clone(),
                                confirmed_at_ms: 0,
                            },
                            &ask.licenses,
                            &ask.source_domains,
                        ) =>
                {
                    store
                        .record(
                            name,
                            ConsentEntry {
                                licenses: ask.licenses.clone(),
                                source_domains: ask.source_domains.clone(),
                                version: ask.version.clone(),
                                confirmed_at_ms: now_ms(),
                            },
                        )
                        .map_err(|e| failed("consent", name, e))?;
                }
                other => {
                    ask.rejected = other.map(|_| "changed".to_owned());
                    return Err(RpcError::new(
                        code::CONFIRM_REQUIRED,
                        "confirm the license and source before installing",
                    )
                    .with_data(serde_json::to_value(&ask).unwrap_or_default()));
                }
            }
        }

        // 합류: 같은 패키지의 설치가 돌고 있으면 그 결과를 받는다.
        let existing = lock(&self.jobs).get(name).cloned();
        if let Some(job) = existing {
            return wait_job(&job, true).await;
        }

        let toolchain = self.find_toolchain(&host, grant, &resolved).await?;
        let plan = Plan {
            resolved,
            pinned: pinned_owned,
            licenses,
            registry_version,
            toolchain,
        };
        let (job, joined) = {
            let mut jobs = lock(&self.jobs);
            match jobs.get(name).cloned() {
                Some(job) => (job, true),
                None => {
                    let job = self.start_job(&host, plan);
                    jobs.insert(name.to_owned(), Arc::clone(&job));
                    (job, false)
                }
            }
        };
        wait_job(&job, joined).await
    }

    async fn find_toolchain(
        &self,
        host: &InstallerHost,
        grant: &ToolGrant,
        resolved: &ResolvedTool,
    ) -> Result<Toolchain, RpcError> {
        let kind = resolved.kind;
        if !matches!(kind, PurlKind::Npm | PurlKind::Pypi) {
            return Ok(Toolchain::None);
        }
        let toolchains = Arc::clone(&host.toolchains);
        let project = grant.project().map(str::to_owned);
        let (env, base) = tokio::task::spawn_blocking(move || {
            let env = toolchains.lookup(project.as_deref());
            (env, toolchains.login_shell())
        })
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?;
        let env = env.map_err(RpcError::internal)?;
        let missing = |runtime: &str| {
            RpcError::new(
                code::RUNTIME_MISSING,
                format!("{runtime} is needed to install this tool — install it with mise"),
            )
            .with_data(json!({
                "runtime": runtime,
                "hint": format!("mise use -g {runtime}"),
                "name": resolved.name,
            }))
        };
        Ok(if kind == PurlKind::Npm {
            let found = runner::find_npm(&env).map_err(|m| missing(m.0))?;
            Toolchain::Npm { found, base }
        } else {
            let found = runner::find_python(&env).map_err(|m| missing(m.0))?;
            Toolchain::Python { found, base }
        })
    }

    fn start_job(self: &Arc<Self>, host: &InstallerHost, plan: Plan) -> Arc<Job> {
        let name = plan.resolved.name.clone();
        let version = plan.resolved.version.clone();
        let (tx, rx) = watch::channel(None);
        let slot: Arc<Mutex<Option<Result<Installed, RpcError>>>> = Arc::default();
        let this = Arc::clone(self);
        let body_slot = Arc::clone(&slot);
        let title = format!("Install {name} {version}");
        let mut handle = host.tasks.spawn(title, move |ctx| async move {
            let result = this.run_install(&ctx, plan).await;
            let message = result.as_ref().err().map(|e| e.message.clone());
            *lock(&body_slot) = Some(result);
            match message {
                None => Ok(()),
                Some(m) => Err(m),
            }
        });
        let job = Arc::new(Job {
            task_id: handle.id(),
            version,
            outcome: rx,
        });
        let this = Arc::clone(self);
        let job_name = name.clone();
        tokio::spawn(async move {
            let status = handle.wait().await;
            let outcome = lock(&slot).take().unwrap_or_else(|| match status {
                TaskStatus::Cancelled => Err(RpcError::new(
                    code::CANCELLED,
                    "the install was cancelled — nothing changed",
                )
                .with_data(json!({ "name": job_name }))),
                TaskStatus::Failed { error } => Err(failed("task", &job_name, error)),
                _ => Err(RpcError::internal(
                    "the install task ended without a result",
                )),
            });
            if let Err(e) = &outcome {
                this.log(&job_name, &format!("failed: {} ({})", e.message, e.code));
            }
            lock(&this.jobs).remove(&job_name);
            let _ = tx.send(Some(outcome));
        });
        job
    }

    /// 설치 로그에 한 줄. 1 MiB 를 넘으면 `.log.1` 로 돌린다.
    fn log(&self, name: &str, line: &str) {
        let Ok(path) = self.log_path(name) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
            let _ = fs::rename(&path, path.with_extension("log.1"));
        }
        use std::io::Write as _;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "[{}] {line}", now_ms());
        }
    }

    /// 작업 본문.
    async fn run_install(&self, ctx: &TaskCtx, plan: Plan) -> Result<Installed, RpcError> {
        let host = self.host()?.clone();
        let layout = Layout::new(host.tools_dir.clone());
        let name = plan.resolved.name.clone();
        let version = plan.resolved.version.clone();
        fs::create_dir_all(layout.root()).map_err(|e| failed("prepare", &name, e.to_string()))?;
        self.log(
            &name,
            &format!(
                "install {name} {version} (registry {})",
                plan.registry_version
            ),
        );

        // 다른 앱 프로세스의 설치와 하나만(잠금 파일). 기다리는 동안도 취소된다.
        let _file_lock = acquire_file_lock(&layout.lock_file(&name), Some(ctx)).await?;

        let record = layout
            .read_record(&name)
            .map_err(|e| failed("record", &name, e))?;
        if let Some(r) = &record {
            // 다른 프로세스가 먼저 그 판을 끝냈다.
            if r.current == version && entry_problem(&layout, &name, r).is_none() {
                self.log(&name, "already installed by another process");
                return Ok(Installed {
                    name,
                    version,
                    previous: r.previous.clone(),
                    outcome: InstallOutcome::AlreadyInstalled,
                    checksum: r.current_entry().map_or(ToolChecksum::None, |e| e.checksum),
                });
            }
            // 남겨 둔 이전 판이 그 판이다 — 링크만 돌린다.
            if r.previous.as_deref() == Some(&version)
                && let Some(entry) = r.versions.get(&version)
                && !layout
                    .version_dir(&name, &version)
                    .join(INSTALLING_MARK)
                    .exists()
                && layout.version_dir(&name, &version).is_dir()
            {
                let mut next = r.clone();
                next.previous = Some(r.current.clone());
                next.current = version.clone();
                layout
                    .write_record(&name, &next)
                    .map_err(|e| failed("record", &name, e))?;
                layout
                    .link_bins(&name, entry)
                    .map_err(|e| failed("link", &name, e))?;
                self.log(&name, &format!("switched to the kept version {version}"));
                return Ok(Installed {
                    name,
                    version,
                    previous: next.previous,
                    outcome: InstallOutcome::Switched,
                    checksum: entry.checksum,
                });
            }
        }

        let dir = layout.version_dir(&name, &version);
        let mut guard = DirGuard::new(dir.clone(), &layout, &name)?;
        fs::create_dir_all(&dir).map_err(|e| failed("prepare", &name, e.to_string()))?;
        fs::write(dir.join(INSTALLING_MARK), b"")
            .map_err(|e| failed("prepare", &name, e.to_string()))?;

        let (files, checksum, used) = match plan.resolved.source.clone() {
            ToolSource::Files { files } => {
                let (fetched, checksum) = self.install_files(ctx, &plan, &files, &dir).await?;
                (fetched, checksum, Used::None)
            }
            ToolSource::Npm { package, extra } => {
                let (checksum, node) = self.install_npm(ctx, &plan, &package, &extra, &dir).await?;
                (pinned_files(&plan), checksum, Used::Node(node))
            }
            ToolSource::Pypi { package, extra } => {
                let (checksum, python) = self
                    .install_pypi(ctx, &plan, &package, &extra, &dir)
                    .await?;
                (pinned_files(&plan), checksum, Used::Python(python))
            }
        };

        // 실행 파일.
        let mut bins = BTreeMap::new();
        for (bin, at) in &plan.resolved.bin {
            let rel = match at {
                ToolBin::Path(p) => {
                    extract::make_executable(&dir, p)
                        .map_err(|e| failed("bin", &name, e.to_string()))?;
                    p.clone()
                }
                ToolBin::Npm(n) => npm_bin(n),
                ToolBin::Pypi(n) => venv_bin(n),
                ToolBin::Python(_) => {
                    return Err(failed("bin", &name, "python launchers are not supported"));
                }
            };
            if !dir.join(&rel).is_file() {
                return Err(failed(
                    "bin",
                    &name,
                    format!("{bin} ({rel}) is not in the install folder"),
                ));
            }
            bins.insert(bin.clone(), rel);
        }

        let _ = fs::remove_file(dir.join(INSTALLING_MARK));
        let entry = InstalledVersion {
            version: version.clone(),
            kind: plan.resolved.kind,
            registry_version: plan.registry_version.clone(),
            source_urls: source_urls(&plan.resolved),
            files,
            checksum,
            licenses: plan.licenses.clone(),
            size: dir_size(&dir),
            installed_at_ms: now_ms(),
            bins,
            python: match &used {
                Used::Python(p) => Some(p.clone()),
                _ => None,
            },
            node: match &used {
                Used::Node(n) => Some(n.clone()),
                _ => None,
            },
        };

        // 기록 → 링크 → 남길 것 밖의 판 지우기. 기록을 쓴 뒤로는 되돌리지 않는다.
        let mut next = match record {
            Some(mut r) => {
                if r.current != version {
                    r.previous = Some(r.current.clone());
                }
                r.current = version.clone();
                r.versions.insert(version.clone(), entry.clone());
                r
            }
            None => InstallRecord::new(&name, entry.clone()),
        };
        let keep: BTreeSet<String> = [Some(next.current.clone()), next.previous.clone()]
            .into_iter()
            .flatten()
            .collect();
        next.versions.retain(|v, _| keep.contains(v));
        layout
            .write_record(&name, &next)
            .map_err(|e| failed("record", &name, e))?;
        guard.commit();
        layout
            .link_bins(&name, &entry)
            .map_err(|e| failed("link", &name, e))?;
        remove_other_versions(&layout, &name, &keep);
        self.log(
            &name,
            &format!("installed {name} {version} (checksum {checksum:?})"),
        );
        ctx.report(TaskProgress {
            done: 1,
            total: Some(1),
            message: Some("installed".into()),
        });
        Ok(Installed {
            name,
            version,
            previous: next.previous,
            outcome: InstallOutcome::Installed,
            checksum,
        })
    }

    /// github · generic: 파일을 받아 고정 해시와 대조하고 푼다.
    async fn install_files(
        &self,
        ctx: &TaskCtx,
        plan: &Plan,
        files: &[crate::package::ToolDownload],
        dir: &Path,
    ) -> Result<(Vec<FetchedFile>, ToolChecksum), RpcError> {
        let name = &plan.resolved.name;
        let fetcher = self.fetcher().map_err(|e| failed("download", name, e))?;
        let downloads = dir.parent().unwrap_or(dir).join(format!(
            ".download-{}-{}",
            layout::version_dir_name(&plan.resolved.version),
            std::process::id()
        ));
        let _downloads_guard = RemoveOnDrop(downloads.clone());
        fs::create_dir_all(&downloads).map_err(|e| failed("download", name, e.to_string()))?;
        let total: Option<u64> = plan
            .pinned
            .as_ref()
            .map(|rows| rows.iter().map(|a| a.size).sum());
        let mut done: u64 = 0;
        let mut remaining = self.limits.download;
        let mut fetched = Vec::new();
        let mut unpacker = Unpacker::new(dir.to_path_buf(), self.limits.unpacked);
        for f in files {
            let path = downloads.join(&f.name);
            let mut out =
                fs::File::create(&path).map_err(|e| failed("download", name, e.to_string()))?;
            self.log(name, &format!("download {}", f.url));
            let got = fetcher
                .download(&self.fetch_url(&f.url), &mut out, remaining, |n| {
                    done += n;
                    ctx.report(TaskProgress {
                        done,
                        total,
                        message: Some(f.name.clone()),
                    });
                })
                .await
                .map_err(|e| match e {
                    FetchError::TooLarge { .. } => failed(
                        "download",
                        name,
                        format!("the download is larger than {} bytes", self.limits.download),
                    ),
                    e => failed("download", name, e.to_string()),
                })?;
            drop(out);
            remaining = remaining.saturating_sub(got.size);
            // ⓐ 고정 해시(기본 번들). ⓑ registry 의 체크섬은 Mason 패키지 정의에 칸이 없어(M4-03 — 체크섬은 릴리스 산출물 단위뿐)
            // 볼 것이 없다. 둘 다 없으면 「체크섬 없음」 으로 기록한다.
            if let Some(rows) = &plan.pinned {
                let row = rows.iter().find(|a| a.name == f.name && a.url == f.url);
                match row {
                    Some(a) if a.sha256 == got.sha256 => {}
                    Some(a) => {
                        return Err(failed(
                            "checksum",
                            name,
                            format!(
                                "{} has SHA-256 {} but {} is pinned",
                                f.name, got.sha256, a.sha256
                            ),
                        ));
                    }
                    None => {
                        return Err(failed(
                            "checksum",
                            name,
                            format!("{} has no pinned hash", f.name),
                        ));
                    }
                }
            }
            unpacker
                .unpack(&path, &f.name, f.format, f.dest.as_deref())
                .map_err(|e| failed("unpack", name, e.to_string()))?;
            let _ = fs::remove_file(&path);
            fetched.push(FetchedFile {
                name: f.name.clone(),
                url: f.url.clone(),
                sha256: got.sha256,
                size: got.size,
            });
        }
        let checksum = if plan.pinned.is_some() {
            ToolChecksum::Pinned
        } else {
            ToolChecksum::None
        };
        Ok((fetched, checksum))
    }

    /// npm: 설치 폴더에 `npm install --prefix …` 하고 `package-lock.json` 의 `integrity` 를 고정 값과 대조한다.
    async fn install_npm(
        &self,
        ctx: &TaskCtx,
        plan: &Plan,
        package: &crate::package::ToolPackageSpec,
        extra: &[crate::package::ToolPackageSpec],
        dir: &Path,
    ) -> Result<(ToolChecksum, UsedRuntime), RpcError> {
        let name = &plan.resolved.name;
        let Toolchain::Npm { found, base } = &plan.toolchain else {
            return Err(RpcError::internal("npm package without npm"));
        };
        // 실제 경로로 준다 — `--prefix` 와 cwd 가 링크(macOS 의 `/var` → `/private/var`)로 갈리면 npm 이 lock 의 키를 상대 경로로 적는다.
        let dir = &fs::canonicalize(dir).map_err(|e| failed("prepare", name, e.to_string()))?;
        // 사용자 · 전역 설정 자리에 빈 파일을 준다(npm 은 같은 파일을 두 자리에 받지 않는다).
        let user_rc = dir.join(".pitwall-npmrc-user");
        let global_rc = dir.join(".pitwall-npmrc-global");
        for rc in [&user_rc, &global_rc] {
            fs::write(rc, b"").map_err(|e| failed("prepare", name, e.to_string()))?;
        }
        let mut args: Vec<OsString> = ["install", "--prefix"].iter().map(OsString::from).collect();
        args.push(dir.as_os_str().to_owned());
        for a in [
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--no-update-notifier",
            "--registry",
            runner::NPM_REGISTRY,
        ] {
            args.push(a.into());
        }
        args.push("--userconfig".into());
        args.push(user_rc.as_os_str().to_owned());
        args.push("--globalconfig".into());
        args.push(global_rc.as_os_str().to_owned());
        args.push("--".into());
        for spec in std::iter::once(package).chain(extra) {
            args.push(npm_spec(spec).into());
        }
        let env = runner::install_env(base, &found.dirs);
        ctx.report(TaskProgress {
            done: 0,
            total: None,
            message: Some("npm install".into()),
        });
        self.log(
            name,
            &format!("run {} {}", found.program.display(), show_args(&args)),
        );
        let ran = runner::run(&found.program, &args, dir, &env)
            .await
            .map_err(|e| failed("npm", name, e))?;
        self.log(name, &ran.output);
        let _ = fs::remove_file(&user_rc);
        let _ = fs::remove_file(&global_rc);
        if !ran.success {
            return Err(failed(
                "npm",
                name,
                format!("npm install failed (exit {:?})", ran.code),
            ));
        }
        let checksum = match &plan.pinned {
            None => ToolChecksum::None,
            Some(rows) => {
                check_npm_lock(dir, rows).map_err(|e| failed("checksum", name, e))?;
                ToolChecksum::Pinned
            }
        };
        Ok((
            checksum,
            UsedRuntime {
                path: found.runtime.clone(),
                version: None,
            },
        ))
    }

    /// pypi: 설치 폴더에 venv 를 만들고 `pip install --only-binary=:all: …`(고정 해시가 있으면 `--require-hashes`).
    async fn install_pypi(
        &self,
        ctx: &TaskCtx,
        plan: &Plan,
        package: &crate::package::ToolPackageSpec,
        extra: &[crate::package::ToolPackageSpec],
        dir: &Path,
    ) -> Result<(ToolChecksum, UsedRuntime), RpcError> {
        let name = &plan.resolved.name;
        let Toolchain::Python { found, base } = &plan.toolchain else {
            return Err(RpcError::internal("pypi package without python"));
        };
        let dir = &fs::canonicalize(dir).map_err(|e| failed("prepare", name, e.to_string()))?;
        let env = runner::install_env(base, &found.dirs);
        let venv = dir.join("venv");
        ctx.report(TaskProgress {
            done: 0,
            total: None,
            message: Some("python -m venv".into()),
        });
        let venv_args: Vec<OsString> = vec![
            "-I".into(),
            "-m".into(),
            "venv".into(),
            venv.as_os_str().to_owned(),
        ];
        self.log(
            name,
            &format!("run {} {}", found.program.display(), show_args(&venv_args)),
        );
        let ran = runner::run(&found.program, &venv_args, dir, &env)
            .await
            .map_err(|e| failed("venv", name, e))?;
        self.log(name, &ran.output);
        if !ran.success {
            return Err(failed(
                "venv",
                name,
                format!("python -m venv failed (exit {:?})", ran.code),
            ));
        }
        let venv_python = dir.join(venv_python());
        let mut args: Vec<OsString> = [
            "-I",
            "-m",
            "pip",
            "install",
            "--isolated",
            "--no-input",
            "--disable-pip-version-check",
            "--only-binary=:all:",
            "--index-url",
            runner::PYPI_INDEX,
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let checksum = match &plan.pinned {
            Some(rows) => {
                let file = dir.join(".pitwall-requirements.txt");
                fs::write(&file, requirements(rows))
                    .map_err(|e| failed("prepare", name, e.to_string()))?;
                args.push("--require-hashes".into());
                args.push("-r".into());
                args.push(file.as_os_str().to_owned());
                args.push("--".into());
                ToolChecksum::Pinned
            }
            None => {
                args.push("--".into());
                for spec in std::iter::once(package).chain(extra) {
                    args.push(pip_spec(spec).into());
                }
                ToolChecksum::None
            }
        };
        ctx.report(TaskProgress {
            done: 0,
            total: None,
            message: Some("pip install".into()),
        });
        self.log(
            name,
            &format!("run {} {}", venv_python.display(), show_args(&args)),
        );
        let ran = runner::run(&venv_python, &args, dir, &env)
            .await
            .map_err(|e| failed("pip", name, e))?;
        self.log(name, &ran.output);
        let _ = fs::remove_file(dir.join(".pitwall-requirements.txt"));
        if !ran.success {
            return Err(failed(
                "pip",
                name,
                format!("pip install failed (exit {:?})", ran.code),
            ));
        }
        // venv 를 만든 python 의 실제 경로(mise shim 이 아니라 그 뒤의 인터프리터).
        let real = fs::canonicalize(&venv_python).unwrap_or_else(|_| found.runtime.clone());
        let version = python_version(&venv.join("pyvenv.cfg"));
        Ok((
            checksum,
            UsedRuntime {
                path: real,
                version,
            },
        ))
    }

    /// 되돌리기: 이전 판으로 기록 · 링크를 돌린다.
    pub async fn rollback(
        &self,
        _grant: &ToolGrant,
        name: &str,
    ) -> Result<InstallRecord, RpcError> {
        let layout = self.layout()?;
        let _busy = self.mark_busy(name)?;
        let _file_lock = acquire_file_lock(&layout.lock_file(name), None).await?;
        let record = layout
            .read_record(name)
            .map_err(|e| failed("record", name, e))?
            .ok_or_else(|| not_installed(name, "the tool is not installed"))?;
        let Some(prev) = record.previous.clone() else {
            return Err(not_installed(
                name,
                "there is no previous version to roll back to",
            ));
        };
        let entry = record
            .versions
            .get(&prev)
            .cloned()
            .ok_or_else(|| not_installed(name, "the previous version is missing"))?;
        if !layout.version_dir(name, &prev).is_dir() {
            return Err(not_installed(
                name,
                "the previous version's folder is missing",
            ));
        }
        let mut next = record.clone();
        next.previous = Some(record.current.clone());
        next.current = prev.clone();
        layout
            .write_record(name, &next)
            .map_err(|e| failed("record", name, e))?;
        layout
            .link_bins(name, &entry)
            .map_err(|e| failed("link", name, e))?;
        self.log(name, &format!("rolled back to {prev}"));
        Ok(next)
    }

    /// 제거: 쓰는 서버를 멈춘 뒤 기록 · 링크 · 폴더를 지운다. 확인 기록은 남긴다(다시 설치할 때 묻지 않는다).
    pub async fn uninstall(&self, _grant: &ToolGrant, name: &str) -> Result<(), RpcError> {
        let layout = self.layout()?;
        let _busy = self.mark_busy(name)?;
        if layout
            .read_record(name)
            .map_err(|e| failed("record", name, e))?
            .is_none()
        {
            return Err(not_installed(name, "the tool is not installed"));
        }
        let users = Arc::clone(&self.users.read().unwrap_or_else(PoisonError::into_inner));
        users.stop(name).await;
        let _file_lock = acquire_file_lock(&layout.lock_file(name), None).await?;
        let _ = fs::remove_file(layout.record_path(name));
        layout.unlink_bins(name);
        let dir = layout.package_dir(name);
        tokio::task::spawn_blocking(move || fs::remove_dir_all(dir))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
            .map_err(|e| failed("remove", name, e.to_string()))?;
        self.log(name, "uninstalled");
        Ok(())
    }

    /// 같은 프로세스에서 설치 작업이 돌거나 다른 되돌리기 · 제거가 돌면 [`code::BUSY`].
    fn mark_busy(&self, name: &str) -> Result<BusyGuard<'_>, RpcError> {
        if !crate::package::valid_file_name(name) {
            return Err(RpcError::invalid_params("not a package name"));
        }
        if lock(&self.jobs).contains_key(name) {
            return Err(busy(name));
        }
        if !lock(&self.busy).insert(name.to_owned()) {
            return Err(busy(name));
        }
        Ok(BusyGuard {
            owner: self,
            name: name.to_owned(),
        })
    }

    /// 설치 기록(없으면 `None`).
    pub fn record(&self, name: &str) -> Result<Option<InstallRecord>, RpcError> {
        self.layout()?
            .read_record(name)
            .map_err(|e| failed("record", name, e))
    }

    /// 설치 기록이 있는 패키지와 그 기록(읽지 못했으면 까닭).
    pub fn installed(&self) -> Result<Vec<InstalledEntry>, RpcError> {
        let layout = self.layout()?;
        Ok(layout
            .packages()
            .into_iter()
            .map(|n| {
                let r = layout
                    .read_record(&n)
                    .and_then(|r| r.ok_or_else(|| "no install record".to_owned()));
                (n, r)
            })
            .collect())
    }

    pub fn problem_of(&self, name: &str, record: &InstallRecord) -> Option<String> {
        let layout = self.layout().ok()?;
        entry_problem(&layout, name, record)
    }

    /// 도는 설치(이름 → 판, 작업 id).
    pub fn running(&self) -> BTreeMap<String, (String, u64)> {
        lock(&self.jobs)
            .iter()
            .map(|(n, j)| (n.clone(), (j.version.clone(), j.task_id)))
            .collect()
    }

    pub fn read_log(&self, name: &str) -> Result<Option<String>, RpcError> {
        if !crate::package::valid_file_name(name) {
            return Err(RpcError::invalid_params("not a package name"));
        }
        let path = self.log_path(name)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(RpcError::internal(e.to_string())),
        }
    }

    pub fn consent_store(&self) -> Result<ConsentStore, RpcError> {
        self.consent()
    }
}

struct BusyGuard<'a> {
    owner: &'a ToolsInstaller,
    name: String,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        lock(&self.owner.busy).remove(&self.name);
    }
}

enum Used {
    None,
    Node(UsedRuntime),
    Python(UsedRuntime),
}

async fn wait_job(job: &Job, joined: bool) -> Result<methods::ToolInstallResult, RpcError> {
    let mut rx = job.outcome.clone();
    let outcome = rx
        .wait_for(Option::is_some)
        .await
        .map_err(|_| RpcError::internal("the install task went away"))?
        .clone()
        .unwrap_or_else(|| Err(RpcError::internal("no install result")));
    let installed = outcome?;
    Ok(methods::ToolInstallResult {
        name: installed.name,
        version: installed.version,
        previous: installed.previous,
        outcome: installed.outcome,
        checksum: installed.checksum,
        task_id: Some(job.task_id),
        joined,
    })
}

/// 지금 판을 쓸 수 없는 까닭(없으면 `None`).
fn entry_problem(layout: &Layout, name: &str, record: &InstallRecord) -> Option<String> {
    let Some(entry) = record.current_entry() else {
        return Some("the install record has no current version".into());
    };
    let dir = layout.version_dir(name, &entry.version);
    if !dir.is_dir() || dir.join(INSTALLING_MARK).exists() {
        return Some("the install folder is missing or unfinished".into());
    }
    if let Some(p) = &entry.python
        && !p.path.exists()
    {
        return Some(format!(
            "the python that made its environment is gone ({})",
            p.path.display()
        ));
    }
    for (bin, rel) in &entry.bins {
        if !dir.join(rel).is_file() {
            return Some(format!("{bin} is missing from the install folder"));
        }
    }
    None
}

/// 남길 판 밖의 폴더(이전 설치의 판 · 남은 조각)를 지운다.
fn remove_other_versions(layout: &Layout, name: &str, keep: &BTreeSet<String>) {
    let keep_dirs: BTreeSet<String> = keep.iter().map(|v| layout::version_dir_name(v)).collect();
    let Ok(entries) = fs::read_dir(layout.package_dir(name)) else {
        return;
    };
    for e in entries.filter_map(Result::ok) {
        let file_name = e.file_name().to_string_lossy().into_owned();
        if file_name == layout::RECORD_FILE || keep_dirs.contains(&file_name) {
            continue;
        }
        if e.path().is_dir() && !file_name.starts_with(".download-") {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// 잠금 파일을 잡는다. 다른 프로세스가 잡고 있으면 기다린다(작업이면 취소로 멈춘다, 아니면 [`code::BUSY`]).
async fn acquire_file_lock(path: &Path, ctx: Option<&TaskCtx>) -> Result<fs::File, RpcError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| RpcError::internal(e.to_string()))?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|e| RpcError::internal(format!("cannot open the install lock: {e}")))?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) => {
                let Some(ctx) = ctx else {
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    return Err(busy(&name));
                };
                ctx.report(TaskProgress {
                    done: 0,
                    total: None,
                    message: Some("waiting for another install".into()),
                });
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(fs::TryLockError::Error(e)) => {
                return Err(RpcError::internal(format!("cannot lock: {e}")));
            }
        }
    }
}

/// 새 판의 폴더: 끝내지 못하면(실패 · 취소로 future 가 버려짐) 지운다.
struct DirGuard {
    dir: PathBuf,
    committed: bool,
}

impl DirGuard {
    /// `dir` 이 이미 있으면(같은 판을 다시 설치 · 남은 조각) 먼저 지운다.
    fn new(dir: PathBuf, _layout: &Layout, name: &str) -> Result<Self, RpcError> {
        if fs::symlink_metadata(&dir).is_ok() {
            fs::remove_dir_all(&dir).map_err(|e| failed("prepare", name, e.to_string()))?;
        }
        Ok(DirGuard {
            dir,
            committed: false,
        })
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for DirGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn source_urls(resolved: &ResolvedTool) -> Vec<String> {
    match &resolved.source {
        ToolSource::Files { files } => files.iter().map(|f| f.url.clone()).collect(),
        ToolSource::Npm { .. } => vec![runner::NPM_REGISTRY.to_owned()],
        ToolSource::Pypi { .. } => vec![runner::PYPI_INDEX.to_owned()],
    }
}

fn pinned_files(plan: &Plan) -> Vec<FetchedFile> {
    plan.pinned
        .iter()
        .flatten()
        .map(|a| FetchedFile {
            name: a.name.clone(),
            url: a.url.clone(),
            sha256: a.sha256.clone(),
            size: a.size,
        })
        .collect()
}

fn npm_spec(spec: &crate::package::ToolPackageSpec) -> String {
    match &spec.version {
        Some(v) => format!("{}@{v}", spec.name),
        None => spec.name.clone(),
    }
}

fn pip_spec(spec: &crate::package::ToolPackageSpec) -> String {
    match &spec.version {
        Some(v) => format!("{}=={v}", spec.name),
        None => spec.name.clone(),
    }
}

/// `--require-hashes` 의 해시 파일: 배포마다 `이름==판 --hash=sha256:…`(그 대상과 `any` 의 휠 해시 모두).
fn requirements(rows: &[Artifact]) -> String {
    let mut by_dist: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for a in rows {
        let (Some(dist), Some(ver)) = (&a.dist, &a.dist_version) else {
            continue;
        };
        by_dist
            .entry((normalize_pypi(dist), ver.clone()))
            .or_default()
            .insert(a.sha256.clone());
    }
    let mut out = String::new();
    for ((dist, ver), hashes) in by_dist {
        out.push_str(&format!("{dist}=={ver}"));
        for h in hashes {
            out.push_str(&format!(" --hash=sha256:{h}"));
        }
        out.push('\n');
    }
    out
}

/// `package-lock.json` 의 `packages["node_modules/<이름>"]` 의 판 · `integrity` 가 고정 값과 같은가.
fn check_npm_lock(dir: &Path, rows: &[Artifact]) -> Result<(), String> {
    let path = dir.join("package-lock.json");
    let meta = fs::metadata(&path).map_err(|e| format!("package-lock.json: {e}"))?;
    if meta.len() > 32 * 1024 * 1024 {
        return Err("package-lock.json is too large".into());
    }
    let lock: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("package-lock.json: {e}"))?;
    for a in rows {
        let (Some(dist), Some(ver)) = (&a.dist, &a.dist_version) else {
            continue;
        };
        let entry = lock
            .get("packages")
            .and_then(|p| p.get(format!("node_modules/{dist}")))
            .ok_or_else(|| {
                let keys: Vec<&str> = lock
                    .get("packages")
                    .and_then(serde_json::Value::as_object)
                    .map(|m| m.keys().take(8).map(String::as_str).collect())
                    .unwrap_or_default();
                format!("{dist} is not in package-lock.json (it has {keys:?})")
            })?;
        let got_version = entry.get("version").and_then(serde_json::Value::as_str);
        let got_integrity = entry.get("integrity").and_then(serde_json::Value::as_str);
        if got_version != Some(ver.as_str()) {
            return Err(format!(
                "{dist} is {got_version:?} in package-lock.json but {ver} is pinned"
            ));
        }
        if got_integrity != a.integrity.as_deref() {
            return Err(format!(
                "{dist} {ver} has integrity {got_integrity:?} but another is pinned"
            ));
        }
    }
    Ok(())
}

fn npm_bin(name: &str) -> String {
    if cfg!(windows) {
        format!("node_modules/.bin/{name}.cmd")
    } else {
        format!("node_modules/.bin/{name}")
    }
}

fn venv_bin(name: &str) -> String {
    if cfg!(windows) {
        format!("venv/Scripts/{name}.exe")
    } else {
        format!("venv/bin/{name}")
    }
}

fn venv_python() -> &'static str {
    if cfg!(windows) {
        "venv/Scripts/python.exe"
    } else {
        "venv/bin/python"
    }
}

/// `pyvenv.cfg` 의 `version`(3.11 까지) · `version_info`(3.12 부터).
fn python_version(cfg: &Path) -> Option<String> {
    let text = fs::read_to_string(cfg).ok()?;
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        let k = k.trim();
        (k == "version" || k == "version_info").then(|| v.trim().to_owned())
    })
}

/// 로그에 싣는 인자(경로는 그대로 — 환경 값은 싣지 않는다).
fn show_args(args: &[OsString]) -> String {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests;
