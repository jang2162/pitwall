//! 런타임 환경 RPC. UI 전용이라 MCP 툴이 아니다.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Runtime;

/// 환경을 실제로 어디서 얻었는가.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RuntimeSource {
    /// 로그인 셸. `shell` 은 셸 실행 파일의 경로.
    LoginShell { shell: String },
    /// 앱 프로세스의 환경(로그인 셸을 읽지 못했다).
    Process,
    /// 디렉터리의 설정 파일로 환경을 만드는 제공자(mise 등). `files` 는 그 파일들의 경로.
    ConfigFiles { files: Vec<String> },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RuntimeStatus {
    Ok,
    /// 제공자가 실패해 그 아래 환경(로그인 셸, 또는 앱 프로세스)으로 물러났다. 남기지 않으므로 다음 요청이 다시 시도한다.
    Fallback {
        reason: String,
    },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeToolInfo {
    pub name: String,
    pub version: String,
    /// 버전을 정한 파일.
    pub source_file: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    /// 고른 제공자(`"shell"`. mise 제공자가 들어오면 `"mise"`).
    pub provider: String,
    pub source: RuntimeSource,
    pub status: RuntimeStatus,
    /// 제공자가 고정한 도구와 버전. 로그인 셸은 비어 있다.
    pub tools: Vec<RuntimeToolInfo>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeGetParams {
    pub project_id: String,
    /// 절대 경로. 없으면 프로젝트의 주 루트.
    #[serde(default)]
    pub dir: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeReloadParams {
    /// 다시 읽기를 부른 프로젝트. 이 프로젝트에는 언제나 `runtime.changed` 가 나간다.
    pub project_id: String,
}

rpc_method! {
    /// 그 디렉터리에서 띄울 프로세스의 환경: 제공자 · 출처 · 상태(03 §5-1). 처음이면 로그인 셸을 기다린다(최대 5초 남짓).
    /// 없는 프로젝트 · 상대 경로는 invalid params 오류.
    pub RuntimeGet: "runtime.get" {
        params: RuntimeGetParams,
        result: RuntimeInfo,
        tier: Read,
        roles: [Owner],
    }

    /// 「환경 다시 읽기」: 로그인 셸 환경과 디렉터리별 캐시를 잊고 `runtime.changed` 를 낸다. 셸은 다음 요청에서 다시 돈다
    /// (백그라운드로 미리 시작한다). 이미 띄운 프로세스(열린 터미널)의 환경은 바뀌지 않는다.
    pub RuntimeReload: "runtime.reload" {
        params: RuntimeReloadParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 레지스트리 읽기와 로그인 셸 기다리기는 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, runtime: Arc<Runtime>) {
    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
    }
    let get = Arc::clone(&runtime);
    router
        .unary(RuntimeGet, move |_ctx, params: RuntimeGetParams| {
            let s = Arc::clone(&get);
            blocking(move || s.get(&params))
        })
        .unary(RuntimeReload, move |_ctx, params: RuntimeReloadParams| {
            let s = Arc::clone(&runtime);
            blocking(move || {
                s.reload_project(&params.project_id)?;
                s.prefetch();
                Ok(())
            })
        });
}
