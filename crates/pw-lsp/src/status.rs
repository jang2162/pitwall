//! 상태바 `lsp` 항목 · 설치 배너 · 상태 팝업의 Host 쪽(15-m4-plan.md M4-17).
//!
//! - `lsp.status.document` — 문서 하나(경로 · 언어)를 맡는 인스턴스. 언어(Linguist 이름 · 확장 언어 id)를 서버로 짝짓는 규칙은
//!   문서 열기와 같은 [`LspManager::language_servers`] 하나다 — 웹뷰가 따로 짝짓지 않는다. 인스턴스는 그 서버의 것 중 작업 공간 루트가
//!   그 경로를 담는 것(여럿이면 가장 깊은 것). 서버를 띄우지 않는다. 권한은 `lsp.status.get` 과 같다(그 프로젝트 창만).
//! - `lsp.server.setLocal` — 그 프로젝트의 **개인** 파일 `project.local.toml` 에 `[lsp.<id>] enabled` · `install_prompt` 한 키만 쓴다
//!   (`pw_lsp_servers::LspServers::write_local` — 커밋되는 `.pitwall/project.toml` 에는 쓰지 않는다). 그 프로젝트 창 · 신뢰한
//!   프로젝트만(`lsp_grant` — 다른 창 · 웰컴 · 설정 · 파일 창 · 미신뢰는 거절). `enabled` 를 쓰면 그 서버의 이 워크트리 인스턴스를
//!   `lsp.server.restart` 길로 다시 지나게 한다 — 끄면 `disabled` 로 서버가 끝나고, 켜면 다시 뜬다(허가를 다시 받는다).

use std::path::{Path, PathBuf};

use pw_core::registry::exec::{Launch, exec_grant};
use pw_lsp_servers::LspLocalSetting;
use pw_rpc::{Ctx, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::grant::{lsp_grant, real_path};
use crate::manager::LspManager;
use crate::methods::{LspInstanceStatus, LspServerControlParams};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentStatusParams {
    pub project_id: String,
    /// 탭의 워크트리 꼭대기(`ActiveWorktree.top`). 메인 체크아웃이면 없다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 문서 파일의 절대 경로.
    pub path: String,
    /// 문서 언어 — `lang.detect` 의 Linguist 이름 또는 확장의 언어 id(`lsp.document.open` 과 같다).
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentStatus {
    /// 확장의 언어 id(`rust`).
    pub language: String,
    /// 이 언어를 맡는 서버 id(정의 순서 — 결정 M4-D12). 비면 그 언어의 언어 서버 정의가 없다.
    pub servers: Vec<String>,
    /// 이 문서를 맡는 인스턴스(`servers` 순서). 아직 문서를 열지 않은 서버는 없다.
    pub instances: Vec<LspInstanceStatus>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspServerSetLocalParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 서버 id. 등록되지 않은 id 는 invalid params.
    pub server: String,
    pub key: LspLocalSetting,
    pub value: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspServerSetLocalResult {
    /// 쓴 개인 파일(절대 경로) — 알림에 「어디에 썼나」 를 보인다.
    pub file: String,
}

rpc_method! {
    /// 문서 하나를 맡는 서버와 그 인스턴스 상태(상태바 `lsp` 항목 · 설치 배너). 서버를 띄우지 않는다. 그 프로젝트 창의 연결만.
    pub LspStatusDocument: "lsp.status.document" {
        params: LspDocumentStatusParams,
        result: LspDocumentStatus,
        tier: Read,
        roles: [Owner],
    }

    /// 그 프로젝트의 개인 파일(`project.local.toml`)에 `[lsp.<id>] enabled` · `install_prompt` 를 쓴다(「이 프로젝트에서 끄기」 ·
    /// 「이 프로젝트에서 묻지 않기」). 커밋되는 `.pitwall/project.toml` 에는 쓰지 않는다. 그 프로젝트 창 · 신뢰한 프로젝트만.
    /// `enabled` 면 그 서버의 인스턴스를 다시 지나게 한다(끄면 끝난다).
    pub LspServerSetLocal: "lsp.server.setLocal" {
        params: LspServerSetLocalParams,
        result: LspServerSetLocalResult,
        tier: Write,
        roles: [Owner],
    }
}

/// `path` 를 담는 루트 중 가장 깊은 것을 맡는 인스턴스로 고른다.
fn serving<'a>(
    instances: &'a [LspInstanceStatus],
    server: &str,
    path: &Path,
) -> Option<&'a LspInstanceStatus> {
    instances
        .iter()
        .filter(|i| i.server == server && path.starts_with(Path::new(&i.root)))
        .max_by_key(|i| i.root.len())
}

pub fn document_status(
    manager: &LspManager,
    ctx: &Ctx,
    params: &LspDocumentStatusParams,
) -> Result<LspDocumentStatus, RpcError> {
    exec_grant(ctx.scope(), &params.project_id, &Launch::Process)?;
    let path = PathBuf::from(&params.path);
    let path = real_path(&path).unwrap_or(path);
    let (language, defs) = manager.language_servers(&params.language);
    let status = manager.status(&params.project_id);
    let servers: Vec<String> = defs.into_iter().map(|(_, id, _)| id).collect();
    let instances = servers
        .iter()
        .filter_map(|s| serving(&status.instances, s, &path).cloned())
        .collect();
    Ok(LspDocumentStatus {
        language,
        servers,
        instances,
    })
}

pub async fn set_local(
    manager: &LspManager,
    ctx: &Ctx,
    params: LspServerSetLocalParams,
) -> Result<LspServerSetLocalResult, RpcError> {
    let projects = manager.projects()?;
    let root = {
        let grant = lsp_grant(
            ctx.scope(),
            ctx.connection_closed(),
            &projects,
            &params.project_id,
            params.worktree.as_deref(),
        )?;
        grant.root().to_path_buf()
    };
    let servers = manager.servers();
    let p = params.clone();
    let file = tokio::task::spawn_blocking(move || {
        servers.write_local(
            &p.project_id,
            p.worktree.as_deref(),
            &p.server,
            p.key,
            p.value,
        )
    })
    .await
    .map_err(|e| RpcError::internal(e.to_string()))??;
    if params.key == LspLocalSetting::Enabled {
        let roots: Vec<String> = manager
            .status(&params.project_id)
            .instances
            .into_iter()
            .filter(|i| i.server == params.server && Path::new(&i.root).starts_with(&root))
            .map(|i| i.root)
            .collect();
        for root in roots {
            let control = LspServerControlParams {
                project_id: params.project_id.clone(),
                worktree: params.worktree.clone(),
                server: params.server.clone(),
                root,
            };
            if let Err(e) = manager.restart(ctx, control).await {
                tracing::warn!(target: "pw_lsp", %e, server = %params.server, "cannot apply the personal enabled setting to a running instance");
            }
        }
    }
    Ok(LspServerSetLocalResult {
        file: file.display().to_string(),
    })
}

/// 핸들러를 붙인다(`crate::register` 가 부른다).
pub fn register(router: &mut Router, manager: &LspManager) {
    let m = manager.clone();
    router.unary(
        LspStatusDocument,
        move |ctx, params: LspDocumentStatusParams| {
            let m = m.clone();
            async move { document_status(&m, &ctx, &params) }
        },
    );
    let m = manager.clone();
    router.unary(
        LspServerSetLocal,
        move |ctx, params: LspServerSetLocalParams| {
            let m = m.clone();
            async move { set_local(&m, &ctx, params).await }
        },
    );
}
