//! `project_register` 툴 메서드(`docs/design/16-m5-plan.md` M5-16 ①, `docs/design/05-mcp.md` 「프로젝트」). `write`, 그룹
//! `project`, 프로젝트가 없는 쓰기다(입력에 `projectId` 가 없다 — 승인은 가장 최근에 앞에 있던 창에 묻는다, 결정 M5-D29). 코어
//! 툴이라 헤드리스 Host 에도 붙지만 헤드리스 세션은 승인 경로가 없어 `write` 를 거절한다(`headless_write`).
//!
//! # 두 단계(`super::plan` 머리 주석)
//!
//! 1. 승인 표 없이 오면 **아무것도 쓰지 않고** 계획만 만든다: 등록될 루트(정규화한 실제 경로 — 심볼릭 링크 · `..` 를 푼 것) ·
//!    이름(없으면 그 폴더 이름) · 설정 저장 방식. 그 판정에서 걸리는 것은 승인을 묻기 전에 거절한다(아래).
//! 2. 승인 표가 계획의 해시와 맞으면 **계획 때 푼 루트 · 이름 · 방식으로** 등록한다 — 입력을 다시 풀지 않는다. 쓰기 직전에
//!    그 루트가 아직 같은 실제 경로로 풀리는지만 본다(파일을 고치는 툴이 쓰기 직전에 내용을 비교하는 것과 같다).
//!
//! # 승인을 묻기 전에 거절하는 루트
//!
//! - 상대 경로, 없는 폴더, 폴더가 아닌 것, UTF-8 이 아닌 경로(`project.create` 와 같다).
//! - **넓은 경로**: 파일 시스템 루트, 홈 폴더와 그 위(`/Users` 등). 루트 후보 추천([`super::candidates`])이 빼는 것과 같은 규칙이다
//!   — 사람의 폴더 전체를 프로젝트로 만들면 파일 트리 · 검색 · 감시가 홈 전체에 걸린다.
//! - **이미 등록된 프로젝트에 드는 경로**: 그 루트 자체, 그 안쪽, 그리고 등록된 프로젝트의 git 워크트리(와 그 안쪽) — 경로 해석
//!   ([`Registry::resolve`])이 이미 그 프로젝트로 여는 곳이다(`project_open` 을 쓰면 된다). 등록된 프로젝트의 워크트리를 따로
//!   등록하면 그 프로젝트는 메인 저장소 프로젝트의 신뢰를 따른다([`super::trust`]) — 에이전트가 등록한 프로젝트가 신뢰된 채
//!   생기지 않게 막는다. 등록된 프로젝트들을 **품는** 위 폴더는 받는다(그 프로젝트들은 계속 자기 루트로 풀리고, 새 프로젝트의
//!   신뢰는 따로다).
//!
//! # 신뢰
//!
//! 신뢰를 주지 않는다(03-architecture.md 「신뢰」 — 신뢰는 사람만 바꾼다). 새 프로젝트는 「아직 묻지 않음」(미신뢰)이고, MCP 세션은
//! 신뢰를 바꾸는 RPC 를 부를 수 없다(`project.setTrust` · `project.claimTrustPrompt` 가 에이전트 범위를 거절한다 — 결정 M5-D18).

use std::path::{Path, PathBuf};

use pw_rpc::{ConnectionScope, Router, RpcError, Tier, code, rpc_method};
use serde::{Deserialize, Serialize};

use super::info::ProjectInfo;
use super::methods::rpc_error;
use super::path::normalize;
use super::plan::{PlanChange, Planned, ToolPlan};
use super::{ConfigMode, NewProject, Registry, RegistryError, existing_dir, utf8};

/// 이 툴 메서드의 RPC 이름.
const METHOD: &str = "project.tool.register";

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRegisterToolParams {
    /// Absolute path of an existing folder. Symbolic links and `..` are resolved: the real path is what gets
    /// registered.
    pub root: String,
    /// Display name. Without it, the folder name.
    #[serde(default)]
    pub name: Option<String>,
    /// Where the project settings live. Without it, `repository`.
    #[serde(default)]
    pub config_mode: Option<ConfigMode>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRegisterToolResult {
    /// The new project and what was detected in its root (the same summary as `project_info`).
    pub info: ProjectInfo,
    /// Whether the project is trusted. Registering never trusts a project — only a person trusts it in Pitwall, and
    /// until then project code (runtime providers, language servers) does not run for it.
    pub trusted: bool,
}

rpc_method! {
    /// Register a folder as a Pitwall project and return what was detected in it (build files, languages, the settings
    /// folder). Needs approval in Pitwall: the request shows the real path that will be registered (symbolic links and
    /// `..` resolved), the name and where the settings live. Refused before asking: a relative path, a folder that does
    /// not exist, the file system root, the home folder or a folder above it, and a folder that is already a project
    /// root or lies inside a registered project, including a git worktree of one (use `project_open` for those). The
    /// new project is not trusted: only a person can trust it in Pitwall.
    pub ProjectToolRegister: "project.tool.register" {
        params: Planned<ProjectRegisterToolParams>,
        result: ProjectRegisterToolResult,
        tier: Write,
        roles: [Owner],
        mcp: { group: "project", name: "project_register" },
    }
}

/// 계획 때 푼 등록 — 승인 뒤에는 이것으로 등록한다(입력을 다시 풀지 않는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRegistration {
    /// 정규화한 실제 경로.
    pub root: PathBuf,
    pub name: String,
    pub config_mode: ConfigMode,
}

impl PlannedRegistration {
    /// 승인 요청이 보일 계획. 해시는 입력과 푼 루트 · 이름 · 방식을 덮는다 — 승인 뒤 링크가 다른 폴더를 가리키게 되면 다시 만든
    /// 계획의 해시가 달라 돌지 않는다.
    pub fn plan(&self, params: &ProjectRegisterToolParams) -> ToolPlan {
        let root = self.root.display().to_string();
        let settings = match self.config_mode {
            ConfigMode::Repository => format!(
                "repository ({}, committed with the code)",
                self.root.join(crate::config::PROJECT_DIR).display()
            ),
            ConfigMode::Personal => "personal (outside the repository, not committed)".to_owned(),
        };
        ToolPlan::builder(METHOD, Tier::Write, params)
            .summary(format!("Register {root} as the project `{}`", self.name))
            .change(PlanChange::Text {
                label: "project".to_owned(),
                text: format!(
                    "root: {root}\nname: {}\nsettings: {settings}\ntrusted: no",
                    self.name
                ),
            })
            .target("root", &root)
            .target("name", &self.name)
            .target("configMode", &self.config_mode)
            .build()
    }
}

impl Registry {
    /// `project_register` 의 계획 단계: 디스크 · 레지스트리에 쓰지 않고 등록할 것을 푼다. 승인을 묻기 전에 거절할 루트(모듈
    /// 문서)는 오류다.
    pub fn plan_registration(
        &self,
        params: &ProjectRegisterToolParams,
    ) -> Result<PlannedRegistration, RegistryError> {
        // 쓸 수 없는 저장소면 사람에게 묻기 전에 알린다.
        self.ensure_writable()?;
        let root = existing_dir(Path::new(&params.root))?;
        let root_text = utf8(&root)?.to_owned();
        let home = self.inner.home.as_deref().and_then(|h| normalize(h).ok());
        if root.parent().is_none() || home.as_deref().is_some_and(|h| h.starts_with(&root)) {
            return Err(RegistryError::InvalidPath {
                path: root,
                reason: "is too broad to be a project (the file system root, the home folder or a folder above it); register the project's own folder".into(),
            });
        }
        if let Some(found) = self.resolve(&root)? {
            if found.worktree.is_none() && Path::new(&found.root) == root {
                return Err(RegistryError::RootTaken {
                    root,
                    project_id: found.project.id,
                });
            }
            let reason = match &found.worktree {
                Some(w) => format!(
                    "is in a git worktree ({}) of the registered project `{}` ({}); open it with `project_open` instead",
                    w.top, found.project.id, found.project.name
                ),
                None => format!(
                    "is inside the registered project `{}` ({}, root {}); open it with `project_open` instead",
                    found.project.id, found.project.name, found.root
                ),
            };
            return Err(RegistryError::InvalidPath { path: root, reason });
        }
        let name = match params.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            Some(_) => return Err(RegistryError::InvalidName),
            None => root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(root_text),
        };
        Ok(PlannedRegistration {
            root,
            name,
            config_mode: params.config_mode.unwrap_or_default(),
        })
    }

    /// 승인을 지난 등록. 계획 때 푼 값으로만 한다 — 루트가 지금도 같은 실제 경로로 풀리지 않으면 등록하지 않는다
    /// (`PERMISSION_DENIED`, `data.reason = "planChanged"` — 결과 표지 `plan_changed`).
    pub fn register_planned(
        &self,
        planned: &PlannedRegistration,
    ) -> Result<super::Project, RpcError> {
        let now = existing_dir(&planned.root).map_err(rpc_error)?;
        if now != planned.root {
            return Err(RpcError::new(
                code::PERMISSION_DENIED,
                format!(
                    "{} now resolves to {}, not the folder that was approved; not registering it",
                    planned.root.display(),
                    now.display()
                ),
            )
            .with_data(serde_json::json!({ "reason": "planChanged" })));
        }
        self.create(NewProject {
            root: planned.root.clone(),
            name: Some(planned.name.clone()),
            config_mode: planned.config_mode,
            favorite: false,
            tags: Vec::new(),
        })
        .map_err(rpc_error)
    }
}

/// `project_register` 의 본체. 블로킹(레지스트리 · 디스크).
pub fn register_project(
    scope: &ConnectionScope,
    registry: &Registry,
    input: &Planned<ProjectRegisterToolParams>,
) -> Result<ProjectRegisterToolResult, RpcError> {
    if !matches!(scope, ConnectionScope::Agent) {
        return Err(RpcError::new(
            code::PERMISSION_DENIED,
            "only an mcp session may call agent tools",
        )
        .with_data(serde_json::json!({ "reason": "notAgentSession" })));
    }
    let planned = registry
        .plan_registration(&input.params)
        .map_err(rpc_error)?;
    // 이 줄 위에는 부작용이 없다 — 승인 표가 없거나 맞지 않으면 계획을 실은 「승인 필요」 로 돌아간다.
    input.approve(planned.plan(&input.params))?;
    let project = registry.register_planned(&planned)?;
    let info = registry.info(&project.id).map_err(rpc_error)?;
    let trusted = registry.is_trusted(&project.id);
    Ok(ProjectRegisterToolResult { info, trusted })
}

/// 핸들러를 붙인다. 레지스트리 · 디스크를 읽으므로 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, registry: Registry) {
    router.unary(
        ProjectToolRegister,
        move |ctx, input: Planned<ProjectRegisterToolParams>| {
            let (registry, scope) = (registry.clone(), ctx.scope().clone());
            async move {
                tokio::task::spawn_blocking(move || register_project(&scope, &registry, &input))
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
            }
        },
    );
}

#[cfg(test)]
mod tests;
