//! `run.config.*` RPC. UI(실행 선택기 · 구성 편집 — M6-07)가 부른다. MCP 툴(M6-12 · 13)은 같은 함수([`super::read`] ·
//! [`super::write`])를 부르는 툴 메서드를 따로 둔다(쓰기 툴은 [`super::tool`] — [`register`] 가 함께 붙인다).
//!
//! - 입력은 struct 이고 `worktree` 는 선택 입력이다(08-remote-mobile.md 「프로젝트와 체크아웃 (D57)」 — M7 이 `checkout` 을 깨지
//!   않고 더한다).
//! - 쓰기(`save` · `delete`)는 그 프로젝트 창의 연결만이다(`scope_may_write` — 다른 프로젝트 창 · Host · 파일 범위 · 에이전트
//!   범위 연결은 permission denied). 목록은 어느 연결이든 읽는다(`db.connection.list` 와 같다).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use pw_core::bus::Scope;
use pw_core::config::AppPaths;
use pw_core::registry::exec::{Launch, exec_grant};
use pw_core::registry::state::{PROJECTS_DIR, scope_may_write, valid_id};
use pw_core::registry::{Registry, RegistryError};
use pw_core::task::{TaskProgress, TaskSpawner, TaskStatus};
use pw_rpc::{ConnectionScope, Router, RpcError, code, rpc_method};
use pw_runtime::{Cancel, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::RunConfig;
use super::read::{RunConfigLayer, RunConfigPlace, UnusableRunConfig, read};
use super::write::{RunConfigLocation, check_id, file_of, plan};
use crate::detect::exec::{AUTOMATIC, CacheOnly, EnvSource, ExecLimits, PRESSED, TrustedExec};
use crate::detect::refresh::{RunTasksRefresh, RunTasksRefreshParams, RunTasksRefreshed};
use crate::detect::{
    self, DetectExec, DetectProblem, NoExec, Seen, TaskList, cache, gradle, maven, mise,
};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigListParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 공유 구성(`.pitwall/run/*.toml`)을 그 워크트리에서 읽고, `cwd` 는
    /// 그 워크트리 안의 프로젝트 루트 기준이다. 없으면 주 루트.
    #[serde(default)]
    pub worktree: Option<String>,
}

/// 쓸 수 있는 구성 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigEntry {
    /// 구성 id(파일 이름).
    pub id: String,
    pub config: RunConfig,
    /// 값을 준 파일(공유 · 개인, 아래부터) — 구성 편집의 「저장 위치」 가 처음 고를 것.
    pub layers: Vec<RunConfigLayer>,
}

/// 파일 없이 감지된 태스크(가상 구성 — 감지기는 `crate::detect`). 「구성으로 저장」 은 `config` 를 `run.config.save` 로 쓴다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DetectedRunTask {
    /// `detected:<감지기>:<이름>` — 「사람이 본 구성」 기록(M6-11)의 열쇠.
    pub key: String,
    /// 감지기 id(`npm` · `make` · `just` · `compose`, M6-05 가 `mise` · `gradle` · `maven`).
    pub detector: String,
    /// 감지 출처 파일(절대 경로).
    pub source: String,
    pub config: RunConfig,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigList {
    /// 쓸 수 있는 구성(id 순).
    pub configs: Vec<RunConfigEntry>,
    /// 쓸 수 없는 구성과 까닭(id 순) — 모르는 `type` · 형식 오류 · 루트 밖 `cwd` 등. 버리지 않고 보인다.
    pub unusable: Vec<UnusableRunConfig>,
    /// 감지된 태스크(감지기 순서, 감지기 안에서는 이름순). 미신뢰 프로젝트에서도 파일만 읽는 감지기의 것은 보인다.
    pub detected: Vec<DetectedRunTask>,
    /// 감지기마다의 실패 · 감지에서 뺀 태스크의 까닭. 실패한 감지기의 태스크는 `detected` 에 없다.
    #[serde(default)]
    pub detect_problems: Vec<DetectProblem>,
    /// 사람이 「태스크 읽기」 를 눌러야 채워지는 목록(gradle · maven)의 상태 — 창이 그 단추를 보인다(`run.tasks.refresh`).
    #[serde(default)]
    pub task_lists: Vec<TaskList>,
    /// 구성 폴더 자체를 읽지 못한 까닭. 그 폴더의 구성은 목록에 없다.
    pub problem: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigSaveParams {
    pub project_id: String,
    /// [`RunConfigListParams::worktree`] 와 같다. 공유 파일은 그 워크트리의 것에 쓴다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 구성 id(파일 이름): 영문자 · 숫자 · `-` · `_`, 1~64 글자.
    pub id: String,
    pub config: RunConfig,
    /// 없으면 공유 파일.
    #[serde(default)]
    pub location: RunConfigLocation,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigSaved {
    /// 쓴 파일.
    pub path: String,
    /// 내용이 바뀌었는가(같은 내용이면 쓰지 않는다).
    pub changed: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigDeleteParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub id: String,
    /// 지울 파일. 없으면 공유 · 개인 둘 다(구성이 목록에서 사라진다). 개인만 지우면 공유 파일의 구성이 다시 보인다.
    #[serde(default)]
    pub location: Option<RunConfigLocation>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigDeleted {
    /// 지운 파일.
    pub paths: Vec<String>,
}

rpc_method! {
    /// 프로젝트의 실행 구성 목록: 쓸 수 있는 구성 · 쓸 수 없는 구성(까닭) · 감지된 태스크. 디스크에 쓰지 않는다(구버전 파일도
    /// 메모리에서만 올린다). 없는 프로젝트는 not found, 그 프로젝트의 것이 아닌 `worktree` 는 invalid params.
    pub RunConfigListMethod: "run.config.list" {
        params: RunConfigListParams,
        result: RunConfigList,
        tier: Read,
        roles: [Owner],
    }

    /// 구성 파일(`location` 의 것 하나)을 만들거나 고친다 — 주석과 모르는 키는 그대로 두고, 개인 파일에 쓰면 공유 파일에만 있는
    /// 키는 그대로 비친다. 그 프로젝트 창의 연결만(아니면 permission denied). 구성이 검증(`cwd` 가 루트 안 등)을 지나지 못하거나
    /// id 가 틀렸으면 invalid params, 대소문자만 다른 구성이 있거나 · 파일이 TOML 이 아니거나 · 앱보다 새 `schema` 거나 · 저장소 안의
    /// 링크면 conflict.
    pub RunConfigSave: "run.config.save" {
        params: RunConfigSaveParams,
        result: RunConfigSaved,
        tier: Write,
        roles: [Owner],
    }

    /// 구성 파일을 지운다(`location` 이 없으면 공유 · 개인 둘 다). 그 프로젝트 창의 연결만. 그 id 의 구성(쓸 수 없는 것 포함)이
    /// 없으면 not found.
    pub RunConfigDelete: "run.config.delete" {
        params: RunConfigDeleteParams,
        result: RunConfigDeleted,
        tier: Write,
        roles: [Owner],
    }
}

/// Host 를 만든 뒤에 붙이는 것.
#[derive(Clone)]
pub struct HostParts {
    pub paths: AppPaths,
    pub projects: Registry,
}

/// 프로세스를 띄우는 감지(M6-05)에 Host 를 만든 뒤 붙이는 것 — 런타임 환경과 Host 작업.
#[derive(Clone)]
pub struct DetectHost {
    /// 그 프로젝트 · 작업 폴더의 런타임 환경.
    pub env: EnvSource,
    /// 「태스크 읽기」 를 Host 작업(상태바 진행 · 취소)으로 띄운다.
    pub tasks: TaskSpawner,
}

impl DetectHost {
    pub fn new(runtime: Arc<Runtime>, tasks: TaskSpawner) -> Self {
        Self {
            env: Arc::new(move |project, dir| runtime.env(project, dir).env.clone()),
            tasks,
        }
    }
}

/// 실행 구성 RPC 의 서비스.
#[derive(Default)]
pub struct RunConfigs {
    host: OnceLock<HostParts>,
    /// 목록이 감지한 결과의 지문 — 파일 감시가 감지만 다시 해 달라졌을 때 알린다([`detect::follow_files`]).
    seen: Arc<Seen>,
    /// 프로세스를 띄우는 감지의 자리. 붙기 전에는 그 감지기들이 아무것도 띄우지 않는다([`NoExec`]).
    detect: OnceLock<DetectHost>,
}

/// 그 프로젝트의 신뢰 판정을 지나는 감지 실행기. 캐시 폴더는 `$DATA/projects/<id>/`.
fn trusted_exec(
    parts: &HostParts,
    detect: &DetectHost,
    project_id: &str,
    limits: ExecLimits,
) -> TrustedExec {
    let projects = parts.projects.clone();
    let exec = TrustedExec::new(
        project_id,
        Arc::new(move |id| projects.is_trusted(id)),
        Arc::clone(&detect.env),
        limits,
    );
    match cache_dir(&parts.paths, project_id) {
        Some(dir) => exec.with_cache_dir(dir),
        None => exec,
    }
}

fn cache_dir(paths: &AppPaths, project_id: &str) -> Option<PathBuf> {
    valid_id(project_id).then(|| paths.data.join(PROJECTS_DIR).join(project_id))
}

impl RunConfigs {
    pub fn new() -> Self {
        Self::default()
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.host.set(parts);
    }

    /// 감지 기억 — [`detect::follow_files`] 에 넘긴다.
    pub fn seen(&self) -> Arc<Seen> {
        Arc::clone(&self.seen)
    }

    /// 프로세스를 띄우는 감지(mise · gradle · maven)를 붙인다 — [`Self::attach`] 뒤에 부른다. 두 번째부터는 무시한다.
    pub fn attach_detect(&self, detect: DetectHost) {
        if self.detect.set(detect.clone()).is_err() {
            return;
        }
        let Some(parts) = self.host.get().cloned() else {
            tracing::warn!(target: "pw_run", "run configs: detection attached before the host; file watch re-detection runs no process");
            return;
        };
        self.seen.set_exec(Arc::new(move |project: &str| {
            Box::new(trusted_exec(&parts, &detect, project, AUTOMATIC)) as Box<dyn DetectExec>
        }));
    }

    /// 목록 · 감시가 쓰는 감지 실행기(자동 감지의 상한).
    fn exec(&self, project_id: &str, limits: ExecLimits) -> Box<dyn DetectExec> {
        match (self.host.get(), self.detect.get()) {
            (Some(parts), Some(detect)) => {
                Box::new(trusted_exec(parts, detect, project_id, limits))
            }
            _ => Box::new(NoExec),
        }
    }

    pub(crate) fn host(&self) -> Result<&HostParts, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("run configs are not attached to a host"))
    }

    /// 그 프로젝트(활성 워크트리)의 구성 자리.
    pub fn place(
        &self,
        project_id: &str,
        worktree: Option<&str>,
    ) -> Result<RunConfigPlace, RpcError> {
        let host = self.host()?;
        let project = host.projects.get(project_id).map_err(registry_error)?;
        RunConfigPlace::new(&host.paths, &project, worktree)
    }

    /// `run.config.list`. 프로세스를 띄우는 감지(mise)는 그 프로젝트 창의 연결(`scope`)일 때만 돈다 — 다른 연결에는 캐시 · 정적 결과만
    /// 준다([`CacheOnly`]). 파일 감시가 다시 감지할 자리도 그 창의 목록만 기억한다.
    pub fn list(
        &self,
        scope: &ConnectionScope,
        params: &RunConfigListParams,
    ) -> Result<RunConfigList, RpcError> {
        let place = self.place(&params.project_id, params.worktree.as_deref())?;
        let set = read(&place);
        let own_window = scope_may_write(scope, &params.project_id);
        let exec: Box<dyn DetectExec> = if own_window {
            self.exec(&params.project_id, AUTOMATIC)
        } else {
            let dir = self
                .host
                .get()
                .and_then(|parts| cache_dir(&parts.paths, &params.project_id));
            Box::new(CacheOnly::new(dir))
        };
        let detection = detect::detect(&place.root, exec.as_ref());
        if own_window {
            self.seen.remember(
                &params.project_id,
                params.worktree.as_deref(),
                &place.root,
                &detection,
            );
        }
        Ok(RunConfigList {
            configs: set
                .configs
                .into_iter()
                .map(|c| RunConfigEntry {
                    id: c.id,
                    config: c.config,
                    layers: c.layers,
                })
                .collect(),
            unusable: set.unusable,
            detected: listed(detection.tasks),
            detect_problems: detection.problems,
            task_lists: detection.lists,
            problem: set.problem,
        })
    }

    /// `run.tasks.refresh`(머리 주석은 [`crate::detect::refresh`]).
    pub async fn refresh(
        self: &Arc<Self>,
        scope: &ConnectionScope,
        params: RunTasksRefreshParams,
    ) -> Result<RunTasksRefreshed, RpcError> {
        exec_grant(scope, &params.project_id, &Launch::Process)?;
        let detector: &'static str = match params.detector.as_str() {
            gradle::ID => gradle::ID,
            maven::ID => maven::ID,
            mise::ID => mise::ID,
            other => {
                return Err(RpcError::new(
                    code::INVALID_PARAMS,
                    format!(
                        "`{other}` is not a detector whose tasks can be read (gradle, maven, mise)"
                    ),
                ));
            }
        };
        let this = Arc::clone(self);
        let (project_id, worktree) = (params.project_id.clone(), params.worktree.clone());
        let (place, parts) = blocking(move || {
            let place = this.place(&project_id, worktree.as_deref())?;
            Ok((place, this.host()?.clone()))
        })
        .await?;
        let id = params.project_id.clone();
        if !parts.projects.is_trusted(&id) {
            return Err(crate::runs::untrusted(&id));
        }
        let detect = self
            .detect
            .get()
            .cloned()
            .ok_or_else(|| RpcError::internal("task detection is not attached to a host"))?;
        let cancel = Cancel::default();
        let exec = trusted_exec(&parts, &detect, &id, PRESSED).with_cancel(cancel.clone());
        let root = place.root.clone();
        let tasks = detect.tasks.scoped(
            Scope::Project(id.clone()),
            tokio_util::sync::CancellationToken::new(),
        );
        let title = match detector {
            gradle::ID => "Reading Gradle tasks",
            maven::ID => "Reading Maven goals",
            _ => "Reading mise tasks",
        };
        let mut handle = tasks.spawn(title, move |ctx| async move {
            ctx.report(TaskProgress {
                done: 0,
                total: None,
                message: Some(title.to_owned()),
            });
            // 작업이 취소돼 이 future 가 버려지면 도는 프로세스를 세션째 끝낸다.
            let _stop = CancelOnDrop(cancel);
            tokio::task::spawn_blocking(move || read_tasks(detector, &root, &exec))
                .await
                .map_err(|e| e.to_string())?
        });
        // 요청이 버려지면(창이 닫힘 · 취소) 작업도 멈춘다.
        let _cancel_with_request = handle.cancel_on_drop();
        let status = handle.wait().await;
        let error = match status {
            TaskStatus::Completed => None,
            TaskStatus::Failed { error } => Some(error),
            TaskStatus::Cancelled => {
                return Err(RpcError::new(
                    code::CANCELLED,
                    "reading the tasks was cancelled",
                ));
            }
            TaskStatus::Running => Some("the task ended without a result".to_owned()),
        };
        let this = Arc::clone(self);
        let project_id = params.project_id.clone();
        let root = place.root.clone();
        let detection = blocking(move || {
            let exec = this.exec(&project_id, AUTOMATIC);
            let all: &[&dyn detect::TaskDetector] = detect::DETECTORS;
            let only: Vec<&dyn detect::TaskDetector> =
                all.iter().copied().filter(|d| d.id() == detector).collect();
            Ok(detect::detect_with(&only, &root, exec.as_ref()))
        })
        .await?;
        Ok(RunTasksRefreshed {
            detected: listed(detection.tasks),
            detect_problems: detection.problems,
            task_lists: detection.lists,
            error,
        })
    }

    /// `run.config.save`.
    pub fn save(
        &self,
        scope: &ConnectionScope,
        params: &RunConfigSaveParams,
    ) -> Result<RunConfigSaved, RpcError> {
        require_project_window(scope, &params.project_id)?;
        check_id(&params.id)?;
        let place = self.place(&params.project_id, params.worktree.as_deref())?;
        if let Some(twin) = read(&place).case_twin(&params.id) {
            return Err(RpcError::new(
                code::CONFLICT,
                format!(
                    "run config `{twin}` differs from `{}` only in case; edit `{twin}` instead",
                    params.id
                ),
            ));
        }
        let edit = plan(&place, &params.id, params.location, Some(&params.config))?;
        super::write::apply(&place, &edit)?;
        Ok(RunConfigSaved {
            path: edit.path.display().to_string(),
            changed: edit.changes(),
        })
    }

    /// `run.config.delete`.
    pub fn delete(
        &self,
        scope: &ConnectionScope,
        params: &RunConfigDeleteParams,
    ) -> Result<RunConfigDeleted, RpcError> {
        require_project_window(scope, &params.project_id)?;
        check_id(&params.id)?;
        let place = self.place(&params.project_id, params.worktree.as_deref())?;
        // 정확히 그 id 의 구성만 — 대소문자를 가리지 않는 파일 시스템에서 `dev` 로 `Dev.toml` 을 지우지 않는다.
        if !read(&place).contains(&params.id) {
            return Err(not_found(&params.id));
        }
        let locations = match params.location {
            Some(location) => vec![location],
            None => vec![RunConfigLocation::Shared, RunConfigLocation::Local],
        };
        let mut paths = Vec::new();
        for location in locations {
            if !exists(&file_of(&place, &params.id, location)) {
                continue;
            }
            let edit = plan(&place, &params.id, location, None)?;
            super::write::apply(&place, &edit)?;
            paths.push(edit.path.display().to_string());
        }
        if paths.is_empty() {
            return Err(not_found(&params.id));
        }
        Ok(RunConfigDeleted { paths })
    }
}

/// 감지된 태스크 → 목록의 항목.
fn listed(tasks: Vec<detect::DetectedTask>) -> Vec<DetectedRunTask> {
    tasks
        .into_iter()
        .map(|t| DetectedRunTask {
            key: t.key(),
            detector: t.detector.to_owned(),
            source: t.source.display().to_string(),
            config: t.config,
        })
        .collect()
}

/// 버려질 때 취소를 보낸다.
struct CancelOnDrop(Cancel);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// 「태스크 읽기」 의 몸통(블로킹): gradle · maven 은 읽어서 캐시에 쓰고, mise 는 아무것도 하지 않는다(감지가 늘 새로 읽는다).
fn read_tasks(detector: &str, root: &Path, exec: &TrustedExec) -> Result<(), String> {
    let (names, kind) = match detector {
        gradle::ID => (gradle::read(root, exec)?, cache::BuildFiles::Gradle),
        maven::ID => (maven::read(root, exec)?, cache::BuildFiles::Maven),
        _ => return Ok(()),
    };
    // 캐시에는 목록에 둘 이름만 쓴다(꼴 · 길이 · 개수 — 수만 개를 내는 빌드도 캐시 파일이 상한을 넘지 않는다).
    let names = gradle::plain_names(detector, names, &mut Vec::new());
    let dir = exec
        .cache_dir()
        .ok_or_else(|| "this project has no folder for the task list".to_owned())?;
    let key = cache::key(root, kind)?;
    cache::store(dir, detector, &key, &names)
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn not_found(id: &str) -> RpcError {
    RpcError::new(code::NOT_FOUND, format!("run config `{id}` does not exist"))
}

/// 그 프로젝트 창의 연결이 아니면 permission denied(파일 쓰기 · DB 연결 쓰기와 같은 판정).
fn require_project_window(scope: &ConnectionScope, project_id: &str) -> Result<(), RpcError> {
    if scope_may_write(scope, project_id) {
        Ok(())
    } else {
        Err(RpcError::new(
            code::PERMISSION_DENIED,
            "only the project's own window may change its run configs",
        )
        .with_data(json!({ "projectId": project_id })))
    }
}

fn registry_error(e: RegistryError) -> RpcError {
    match e {
        RegistryError::NotFound(id) => {
            RpcError::new(code::NOT_FOUND, format!("project `{id}` is not registered"))
        }
        RegistryError::Unavailable(_) => {
            RpcError::new(code::STORAGE_UNAVAILABLE, "storage unavailable")
                .with_data(serde_json::Value::String(e.to_string()))
        }
        other => RpcError::internal(other.to_string()),
    }
}

/// 블로킹 스레드에서 돌린다(설정 파일 · 레지스트리 읽기).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, configs: Arc<RunConfigs>) {
    let (list, save, delete, refresh) = (
        Arc::clone(&configs),
        Arc::clone(&configs),
        Arc::clone(&configs),
        Arc::clone(&configs),
    );
    router
        .unary(
            RunConfigListMethod,
            move |ctx, params: RunConfigListParams| {
                let (c, scope) = (Arc::clone(&list), ctx.scope().clone());
                blocking(move || c.list(&scope, &params))
            },
        )
        .unary(RunConfigSave, move |ctx, params: RunConfigSaveParams| {
            let (c, scope) = (Arc::clone(&save), ctx.scope().clone());
            blocking(move || c.save(&scope, &params))
        })
        .unary(
            RunConfigDelete,
            move |ctx, params: RunConfigDeleteParams| {
                let (c, scope) = (Arc::clone(&delete), ctx.scope().clone());
                blocking(move || c.delete(&scope, &params))
            },
        )
        .unary(
            RunTasksRefresh,
            move |ctx, params: RunTasksRefreshParams| {
                let (c, scope) = (Arc::clone(&refresh), ctx.scope().clone());
                async move { c.refresh(&scope, params).await }
            },
        );
    // MCP 툴 메서드(M6-13 — `run_config_upsert` · `run_config_delete`).
    super::tool::register(router, configs);
}
