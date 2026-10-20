//! 외부 도구로 열기(`docs/design/04-features.md` F8-1 · F8-2 · F8-4).
//!
//! - [`tools`]: 확장의 외부 도구 위에 전역 설정을 덮은 이 OS 의 목록, 기본 도구 고르기(프로젝트 설정은 고르기만 한다).
//! - [`context`]: 템플릿 변수의 값(활성 워크트리 기준 프로젝트 루트, 파일 · 줄 · 열, 브랜치).
//! - [`launch`]: 실행 파일 찾기(런타임 환경의 PATH, `crates/pw-runtime`)와 셸 없는 떼어 띄우기(자식은 그 환경 전체를 받는다).
//! - [`methods`]: `openWith.list` · `openWith.run` RPC.
//!
//! 명령 형식(argv 배열, 첫 요소에 변수 금지, `command` · `file_command`)은 `pw_ext::template`.

pub mod context;
pub mod launch;
pub mod methods;
pub mod tools;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use pw_core::config::gitdir::linked_worktree;
use pw_core::config::{AppPaths, ConfigStore};
use pw_core::registry::path::{PathRejected, normalize};
use pw_core::registry::{Project, Registry as Projects, RegistryError};
use pw_ext::Registry;
use pw_ext::template::{SubstituteError, substitute};
use pw_rpc::{RpcError, code};
use pw_runtime::Runtime;

use crate::context::OpenContext;
use crate::launch::{
    EARLY_EXIT, Spawned, find_program, is_bundle, is_launch_services_open, spawn_detached,
};
use crate::methods::{
    DefaultToolInfo, ExternalToolInfo, LaunchProblem, OpenWithList, OpenWithListParams,
    OpenWithOutcome, OpenWithRunParams,
};
use crate::tools::{DefaultTool, SETTINGS_KEY, Tool};

/// Host 를 만든 뒤에 붙이는 것(Host 의 메서드는 Host 를 만드는 중에 붙으므로).
#[derive(Clone)]
pub struct HostParts {
    pub paths: AppPaths,
    /// 전역 설정(`$CONFIG/settings.toml` · `settings.local.toml`).
    pub global: ConfigStore,
    pub projects: Projects,
}

pub struct OpenWith {
    extensions: Arc<Registry>,
    host: OnceLock<HostParts>,
    /// 도구를 찾을 PATH 와 도구에 줄 환경(프로젝트 루트 기준).
    runtime: Arc<Runtime>,
    early_exit: Duration,
}

impl OpenWith {
    pub fn new(extensions: Arc<Registry>, runtime: Arc<Runtime>) -> Self {
        Self {
            extensions,
            host: OnceLock::new(),
            runtime,
            early_exit: EARLY_EXIT,
        }
    }

    #[must_use]
    pub fn with_early_exit(mut self, early_exit: Duration) -> Self {
        self.early_exit = early_exit;
        self
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.host.set(parts);
    }

    /// 로그인 셸 환경 가져오기를 백그라운드로 시작한다(이미 있으면 아무것도 하지 않는다).
    pub fn prefetch_env(&self) {
        self.runtime.prefetch();
    }

    fn host(&self) -> Result<&HostParts, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("open with is not attached to a host config"))
    }

    fn tools(&self, host: &HostParts) -> Vec<Tool> {
        let snapshot = host.global.snapshot();
        let mut warnings = Vec::new();
        let list = tools::tools(
            &self.extensions,
            snapshot.get("settings", SETTINGS_KEY),
            tools::current_platform(),
            &mut warnings,
        );
        for w in warnings {
            tracing::warn!(target: "pw_open_with", "settings: {w}");
        }
        list
    }

    /// 프로젝트 설정의 `open_with.default`. 디스크에 쓰지 않고 읽는다 — 열기만 했다고 커밋된 `.pitwall/project.toml` 이
    /// 마이그레이션되면 안 된다. 읽지 못하면 경고하고 없는 것으로 본다(전역 기본값을 쓴다).
    fn project_default(
        host: &HostParts,
        project: &Project,
        root: &Path,
    ) -> Option<serde_json::Value> {
        let source = match project.config_source(&host.paths, root) {
            Ok(source) => source.without_writes(),
            Err(e) => {
                tracing::warn!(target: "pw_open_with", %e, project = %project.id, "cannot read the project config");
                return None;
            }
        };
        let snapshot = match source.load() {
            Ok(snapshot) => snapshot,
            Err(e) => {
                tracing::warn!(target: "pw_open_with", %e, project = %project.id, "cannot read the project config; using the global default tool");
                return None;
            }
        };
        if snapshot.get("project", "open_with.tools").is_some() {
            tracing::warn!(
                target: "pw_open_with",
                project = %project.id,
                "`[open_with.tools]` in a project config is ignored: project settings can only pick the default among registered tools"
            );
        }
        snapshot.get("project", "open_with.default").cloned()
    }

    fn default_tool(
        &self,
        host: &HostParts,
        project: Option<(&Project, &Path)>,
        tools: &[Tool],
    ) -> DefaultTool {
        let project_default = project.and_then(|(p, root)| Self::project_default(host, p, root));
        let global = host.global.snapshot();
        let mut warnings = Vec::new();
        let picked = tools::default_tool(
            project_default.as_ref(),
            global.get("settings", "open_with.default"),
            tools,
            &mut warnings,
        );
        for w in warnings {
            tracing::warn!(target: "pw_open_with", "{w}");
        }
        picked
    }

    /// 프로젝트와, 활성 워크트리가 있으면 그 워크트리 안의 프로젝트 루트(`ActiveWorktree::root` 와 같은 계산). `worktree` 는
    /// 창이 받은 `ActiveWorktree::top` 이다 — 그 워크트리의 메인 체크아웃 안에 프로젝트 루트가 있어야 한다(git 을 실행하지 않고
    /// `.git` 파일 · `commondir` 로 본다, `pw_core::config::gitdir`).
    fn project_and_root(
        host: &HostParts,
        id: &str,
        worktree: Option<&str>,
    ) -> Result<(Project, PathBuf), RpcError> {
        let project = host.projects.get(id).map_err(registry_error)?;
        let Some(top) = worktree else {
            let root = project.root().to_path_buf();
            return Ok((project, root));
        };
        // 상대 경로면 Pitwall 의 cwd 기준으로 풀려 `{project_root}` 가 상대 경로가 된다(04 F8 「절대 경로」).
        if !Path::new(top).is_absolute() {
            return Err(RpcError::invalid_params(format!(
                "worktree `{top}` must be an absolute path"
            )));
        }
        let root = linked_worktree(Path::new(top)).and_then(|link| {
            let rel = project.root().strip_prefix(&link.main_top).ok()?;
            Some(link.worktree_top.join(rel))
        });
        match root {
            Some(root) => Ok((project, root)),
            None => Err(RpcError::invalid_params(format!(
                "`{top}` is not a worktree of project `{id}`"
            ))),
        }
    }

    pub fn list(&self, params: &OpenWithListParams) -> Result<OpenWithList, RpcError> {
        let host = self.host()?;
        let tools = self.tools(host);
        let project = match &params.project_id {
            Some(id) => Some(Self::project_and_root(
                host,
                id,
                params.worktree.as_deref(),
            )?),
            None => None,
        };
        let default = self.default_tool(
            host,
            project.as_ref().map(|(p, r)| (p, r.as_path())),
            &tools,
        );
        Ok(OpenWithList {
            tools: tools
                .into_iter()
                .map(|t| ExternalToolInfo {
                    opens_files: t.file_command.is_some(),
                    id: t.id,
                    label: t.label,
                    extension: t.extension,
                    from_settings: t.from_settings,
                    problem: t.problem,
                })
                .collect(),
            default: match default {
                DefaultTool::Tool(id) => DefaultToolInfo::Tool { id },
                DefaultTool::Unset => DefaultToolInfo::Unset,
                DefaultTool::Unknown(id) => DefaultToolInfo::Unknown { id },
            },
        })
    }

    pub fn run(&self, params: &OpenWithRunParams) -> Result<OpenWithOutcome, RpcError> {
        let host = self.host()?;
        // `{file}` · `{project_root}` 는 레지스트리 · 파일 RPC(`pw-fs`)와 같은 정규화를 지난다 — `.` · `..` · 심링크가 풀린 실제
        // 경로가 도구에 간다(없는 파일은 가장 깊은 있는 조상까지).
        let file = match &params.file {
            Some(f) => Some(normalize(Path::new(f)).map_err(|PathRejected::Relative| {
                RpcError::invalid_params(format!("file `{f}` must be an absolute path"))
            })?),
            None => None,
        };
        let (project, project_root) =
            Self::project_and_root(host, &params.project_id, params.worktree.as_deref())?;
        let project_root = normalize(&project_root).unwrap_or(project_root);
        let tools = self.tools(host);
        let id = match &params.tool {
            Some(id) => id.clone(),
            None => match self.default_tool(host, Some((&project, &project_root)), &tools) {
                DefaultTool::Tool(id) => id,
                DefaultTool::Unset => return Ok(OpenWithOutcome::NoDefault { configured: None }),
                DefaultTool::Unknown(id) => {
                    return Ok(OpenWithOutcome::NoDefault {
                        configured: Some(id),
                    });
                }
            },
        };
        let failed = |label: &str, argv: Vec<String>, problem: LaunchProblem| {
            tracing::warn!(target: "pw_open_with", tool = %id, ?argv, ?problem, "cannot open with the external tool");
            Ok(OpenWithOutcome::Failed {
                tool: id.clone(),
                label: label.to_owned(),
                argv,
                problem,
            })
        };
        let Some(tool) = tools.iter().find(|t| t.id == id) else {
            return failed(&id, Vec::new(), LaunchProblem::UnknownTool);
        };
        if let Some(message) = &tool.problem {
            return failed(
                &tool.label,
                Vec::new(),
                LaunchProblem::InvalidTool {
                    message: message.clone(),
                },
            );
        }
        let ctx = OpenContext {
            project_root,
            file,
            line: params.line,
            column: params.column,
        };
        let template = match (&ctx.file, &tool.file_command) {
            (Some(_), Some(file_command)) => file_command,
            _ => &tool.command,
        };
        let argv = match substitute(template, &ctx.vars()) {
            Ok(argv) => argv,
            Err(SubstituteError::Missing(variable)) => {
                return failed(
                    &tool.label,
                    Vec::new(),
                    LaunchProblem::MissingVariable { variable },
                );
            }
            Err(SubstituteError::Invalid(message)) => {
                return failed(
                    &tool.label,
                    Vec::new(),
                    LaunchProblem::InvalidTool { message },
                );
            }
        };
        if !ctx.project_root.is_dir() {
            return failed(
                &tool.label,
                argv,
                LaunchProblem::ProjectRootMissing {
                    path: ctx.project_root.display().to_string(),
                },
            );
        }
        let runtime = self.runtime.env(&project.id, &ctx.project_root);
        let login_shell_path = runtime.login_shell;
        let program_name = &argv[0];
        let Some(program) = find_program(program_name, &runtime.env.path) else {
            return failed(
                &tool.label,
                argv.clone(),
                LaunchProblem::NotFound {
                    program: program_name.clone(),
                    login_shell_path,
                },
            );
        };
        // `open` 은 번들 폴더를 열지 않고 실행한다 — 저장소 이름이 `Tool.app` 이면 클론해 열기만 했는데 그 안의 실행 파일이 돈다.
        // 사용자가 Finder 명령을 옛 형태(`open {project_root}`)로 덮어써도 여기서 막는다(launch.rs `is_bundle`).
        if is_launch_services_open(&program) {
            let used = |var: &str| template.iter().any(|e| e.contains(var));
            let bundle = [
                (used("{project_root}"), Some(&ctx.project_root)),
                (used("{file}"), ctx.file.as_ref()),
            ]
            .into_iter()
            .find_map(|(used, path)| path.filter(|p| used && is_bundle(p)));
            if let Some(path) = bundle {
                return failed(
                    &tool.label,
                    argv.clone(),
                    LaunchProblem::BundlePath {
                        path: path.display().to_string(),
                    },
                );
            }
        }
        let mut launched_argv = vec![program.display().to_string()];
        launched_argv.extend_from_slice(&argv[1..]);
        match spawn_detached(
            &program,
            &argv[1..],
            &ctx.project_root,
            &runtime.env,
            self.early_exit,
        ) {
            Ok(Spawned::Running { pid }) => {
                tracing::info!(
                    target: "pw_open_with",
                    tool = %id, pid, argv = ?launched_argv, cwd = %ctx.project_root.display(),
                    path_from_login_shell = login_shell_path,
                    "opened with the external tool"
                );
                Ok(OpenWithOutcome::Launched {
                    tool: id.clone(),
                    label: tool.label.clone(),
                    argv: launched_argv,
                    pid,
                })
            }
            Ok(Spawned::Exited { code, .. }) => {
                failed(&tool.label, launched_argv, LaunchProblem::Exited { code })
            }
            Err(e) => failed(
                &tool.label,
                launched_argv,
                LaunchProblem::SpawnFailed {
                    program: program.display().to_string(),
                    message: e.to_string(),
                },
            ),
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

#[cfg(all(test, unix))]
mod tests;
