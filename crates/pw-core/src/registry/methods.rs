//! 프로젝트 레지스트리 RPC(`project.*`).
//!
//! 권한 등급과 role:
//! - 목록 · 최근 · 조회 · 저장소 상태는 `read`, 모든 role. 레지스트리 안의 것만 돌려준다.
//! - 해석 · 후보는 `read` 지만 **owner 만**: 받은 임의의 경로를 따라 디스크를 본다(있는지, `.git` · 빌드 파일이 있는지).
//!   readonly · mobile 기기가 이것으로 Host 의 파일 시스템을 더듬을 수 있으면 「조회만」의 범위를 넘는다.
//! - 생성 · 수정 · 삭제 · 「열었음」 기록은 `write`, owner 만(등급의 기본값). 지우는 것은 레지스트리 행뿐이고 폴더 · 설정 파일은
//!   건드리지 않아 다시 등록하면 되돌아오므로 `dangerous` 가 아니다. 「열었음」도 최근 목록 순서만 바꾸지만 레지스트리 파일을
//!   쓰는 조작이라 `write` 로 둔다.

use std::path::PathBuf;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::{
    ConfigMode, NewProject, Project, ProjectPatch, Registry, RegistryError, Resolution,
    RootCandidate, StorageStatus,
};

/// `project.recent` 의 기본 개수.
pub const DEFAULT_RECENT: u32 = 20;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RecentParams {
    /// 없으면 20.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdParams {
    pub id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectParams {
    /// 절대 경로. 있는 폴더여야 한다.
    pub root: String,
    /// 없으면 루트 폴더 이름.
    #[serde(default)]
    pub name: Option<String>,
    /// 없으면 `repository`.
    #[serde(default)]
    pub config_mode: Option<ConfigMode>,
    #[serde(default)]
    pub favorite: Option<bool>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectParams {
    pub id: String,
    pub patch: ProjectPatch,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct PathParams {
    /// 절대 경로. 파일이든 폴더든, 없는 경로든 된다.
    pub path: String,
}

rpc_method! {
    /// 등록된 프로젝트 전부, 이름순.
    pub ProjectList: "project.list" {
        params: (),
        result: Vec<Project>,
        tier: Read,
    }

    /// 최근에 연 프로젝트, 최근 것부터. 연 적 없는 프로젝트는 빠진다.
    pub ProjectRecent: "project.recent" {
        params: RecentParams,
        result: Vec<Project>,
        tier: Read,
    }

    /// 프로젝트 하나. 없으면 invalid params 오류.
    pub ProjectGet: "project.get" {
        params: ProjectIdParams,
        result: Project,
        tier: Read,
    }

    /// 레지스트리 파일(`$DATA/state.db`)의 상태. `readOnly` · `memory` 면 쓰기 메서드가 오류로 거절된다 — 웰컴 화면은 정상이 아니면
    /// `reason` 을 배너로 보여 준다.
    pub ProjectStorage: "project.storage" {
        params: (),
        result: StorageStatus,
        tier: Read,
    }

    /// 프로젝트를 등록한다. 루트는 정규화해(심링크 · `..` 를 풀어) 저장하고, 이미 다른 프로젝트의 루트면 오류다.
    pub ProjectCreate: "project.create" {
        params: CreateProjectParams,
        result: Project,
        tier: Write,
    }

    /// 이름 · 즐겨찾기 · 태그 · 설정 저장 방식을 바꾼다. 루트와 slug 는 바꾸지 않는다.
    pub ProjectUpdate: "project.update" {
        params: UpdateProjectParams,
        result: Project,
        tier: Write,
    }

    /// 레지스트리에서 지운다. 프로젝트 폴더와 설정 파일은 그대로 둔다.
    pub ProjectDelete: "project.delete" {
        params: ProjectIdParams,
        result: (),
        tier: Write,
    }

    /// 프로젝트를 열었다고 기록한다(최근 목록 순서).
    pub ProjectMarkOpened: "project.markOpened" {
        params: ProjectIdParams,
        result: Project,
        tier: Write,
    }

    /// 경로를 포함하는 프로젝트(가장 깊은 루트). 등록되지 않은 git 워크트리 안이면 메인 저장소의 프로젝트와 그 워크트리.
    /// 없으면 `null` — 그때는 `project.candidates` 로 루트 후보를 보여 준다.
    pub ProjectResolve: "project.resolve" {
        params: PathParams,
        result: Option<Resolution>,
        tier: Read,
        roles: [Owner],
    }

    /// 경로의 루트 후보: 워크트리의 메인 저장소, 가장 가까운 `.git`, 빌드 파일이 있는 상위 폴더(가까운 것부터).
    /// 홈 디렉터리와 그 위, 파일 시스템 루트는 넣지 않는다.
    pub ProjectCandidates: "project.candidates" {
        params: PathParams,
        result: Vec<RootCandidate>,
        tier: Read,
        roles: [Owner],
    }
}

fn rpc_error(e: RegistryError) -> RpcError {
    match e {
        // 입력이 아니라 Host 쪽 상태(파일을 쓸 수 없음 · DB 오류)라 invalid params 가 아니다.
        RegistryError::Db(_) | RegistryError::Unavailable(_) => RpcError::internal(e.to_string()),
        _ => RpcError::invalid_params(e.to_string()),
    }
}

/// `project.*` 핸들러를 붙인다. 레지스트리는 디스크를 동기로 읽고 쓰므로 블로킹 스레드에서 돌린다.
pub fn register(router: &mut Router, registry: &Registry) {
    async fn blocking<T: Send + 'static>(
        registry: Registry,
        f: impl FnOnce(&Registry) -> Result<T, RegistryError> + Send + 'static,
    ) -> Result<T, RpcError> {
        tokio::task::spawn_blocking(move || f(&registry))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
            .map_err(rpc_error)
    }

    let r = registry.clone();
    router.unary(ProjectList, move |_ctx, ()| {
        blocking(r.clone(), Registry::list)
    });
    let r = registry.clone();
    router.unary(ProjectRecent, move |_ctx, params| {
        let limit = params.limit.unwrap_or(DEFAULT_RECENT) as usize;
        blocking(r.clone(), move |r| r.recent(limit))
    });
    let r = registry.clone();
    router.unary(ProjectGet, move |_ctx, params| {
        blocking(r.clone(), move |r| r.get(&params.id))
    });
    let r = registry.clone();
    router.unary(ProjectStorage, move |_ctx, ()| {
        blocking(r.clone(), |r| Ok(r.status()))
    });
    let r = registry.clone();
    router.unary(ProjectCreate, move |_ctx, params| {
        blocking(r.clone(), move |r| {
            r.create(NewProject {
                root: PathBuf::from(params.root),
                name: params.name,
                config_mode: params.config_mode.unwrap_or_default(),
                favorite: params.favorite.unwrap_or(false),
                tags: params.tags.unwrap_or_default(),
            })
        })
    });
    let r = registry.clone();
    router.unary(ProjectUpdate, move |_ctx, params| {
        blocking(r.clone(), move |r| r.update(&params.id, params.patch))
    });
    let r = registry.clone();
    router.unary(ProjectDelete, move |_ctx, params| {
        blocking(r.clone(), move |r| r.delete(&params.id))
    });
    let r = registry.clone();
    router.unary(ProjectMarkOpened, move |_ctx, params| {
        blocking(r.clone(), move |r| r.mark_opened(&params.id))
    });
    let r = registry.clone();
    router.unary(ProjectResolve, move |_ctx, params| {
        blocking(r.clone(), move |r| r.resolve(&PathBuf::from(params.path)))
    });
    let r = registry.clone();
    router.unary(ProjectCandidates, move |_ctx, params| {
        blocking(r.clone(), move |r| {
            r.candidates(&PathBuf::from(params.path))
        })
    });
}
