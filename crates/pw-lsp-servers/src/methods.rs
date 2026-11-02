//! `lsp.servers.list` — 언어 서버 정의 · 해석 결과 · 합친 설정(`docs/design/15-m4-plan.md` M4-02). UI 전용이라 MCP 툴이 아니다
//! (결정 M4-D21). 서버의 상태(시작 중 · 준비 · 죽음)는 서버 관리자(M4-05)가 이 결과에 더한다.

use std::collections::BTreeMap;
use std::sync::Arc;

use pw_ext::{LspFeature, LspServerRuntime};
use pw_rpc::{JsonValue, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::LspServers;
use crate::resolve::LspResolution;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspServersListParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 그 워크트리의 설정을 읽고 그 안에서 해석한다.
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspRuntimeRequirement {
    pub runtime: LspServerRuntime,
    /// semver 범위(`>=21`).
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspServerInfo {
    /// 서버 id(`[contributes.language_servers.<id>]` · `[lsp.<id>]`).
    pub id: String,
    /// 기여한 확장의 id.
    pub extension: String,
    /// 이 서버가 맡는 언어 id. 같은 언어의 서버가 여럿이면 목록 순서가 기능을 맡는 순서다(결정 M4-D12).
    pub languages: Vec<String>,
    /// Mason registry 패키지 이름. 설치를 제안할 때 쓴다.
    pub mason: Option<String>,
    /// 정의의 실행 파일 이름.
    pub command: String,
    /// 합친 인자(설정의 `args` 가 정의의 것을 바꾼다).
    pub args: Vec<String>,
    /// 서버에 더할 환경 변수(정의 위에 설정을 덮은 것).
    pub env: BTreeMap<String, String>,
    pub root_markers: Vec<String>,
    /// 서버를 돌리는 런타임(결정 M4-D6).
    pub requires: Vec<LspRuntimeRequirement>,
    /// 이 서버에 맡기지 않는 기능(정의 + 설정).
    pub disabled_features: Vec<LspFeature>,
    pub enabled: bool,
    /// `workspace/configuration` 의 답의 바탕(정의 → 전역 → 프로젝트를 깊게 합친 것).
    #[specta(type = JsonValue)]
    pub settings: Value,
    /// `initialize` 의 `initializationOptions`.
    #[specta(type = JsonValue)]
    pub initialization_options: Value,
    pub resolution: LspResolution,
    /// 서버가 없을 때 설치 안내를 띄우는가(`[lsp.<id>] install_prompt` — 기본 `true`, M4-17).
    pub install_prompt: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspServerList {
    /// 등록 순서(내장 확장 폴더 이름순, 한 확장 안에서는 정의 순서).
    pub servers: Vec<LspServerInfo>,
    /// 프로젝트를 신뢰하는가. 아니면 공유 설정 · 프로젝트 로컬 실행 파일을 보지 않았다(서버도 띄우지 않는다 — 결정 M4-D8).
    pub trusted: bool,
    /// 프로젝트 설정을 읽지 못한 까닭. 그러면 전역 설정만 합쳤다.
    pub problem: Option<String>,
    /// 설정의 형식 오류 · 모르는 키 · 모르는 서버(그 값은 버렸다).
    pub warnings: Vec<String>,
}

rpc_method! {
    /// 언어 서버 정의와 그 프로젝트에서의 해석 결과 · 합친 설정. 없는 프로젝트 · 그 프로젝트의 것이 아닌 워크트리는 invalid params
    /// 오류. 신뢰한 프로젝트에서 rustup 프록시를 확인하느라 `rustup which` 를 돌릴 수 있다(결정 M4-D7).
    pub LspServersList: "lsp.servers.list" {
        params: LspServersListParams,
        result: LspServerList,
        tier: Read,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 해석(로그인 셸 환경을 기다릴 수 있다 · `rustup which`)은 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, servers: Arc<LspServers>) {
    router.unary(LspServersList, move |_ctx, params: LspServersListParams| {
        let servers = Arc::clone(&servers);
        async move {
            tokio::task::spawn_blocking(move || servers.list(&params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
}
