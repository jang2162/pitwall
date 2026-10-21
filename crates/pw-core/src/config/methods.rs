//! 설정 RPC. 지금은 읽기만 둔다 — 쓰기는 설정 화면이 키 스키마와 함께 정한다.

use pw_rpc::rpc_method;
use serde::{Deserialize, Serialize};

use super::paths::AppPaths;
use super::source::ConfigSnapshot;
use super::store::{ConfigFault, ConfigStore};

/// 해석된 앱 디렉터리(`docs/design/03-architecture.md` 「5-0. 파일 위치 — XDG 규칙」). UTF-8 이 아닌 경로는 손실 변환된다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPathsInfo {
    pub config: String,
    pub data: String,
    pub state: String,
    pub cache: String,
}

impl From<&AppPaths> for ConfigPathsInfo {
    fn from(p: &AppPaths) -> Self {
        let s = |p: &std::path::Path| p.display().to_string();
        Self {
            config: s(&p.config),
            data: s(&p.data),
            state: s(&p.state),
            cache: s(&p.cache),
        }
    }
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConfigGetParams {
    /// 없으면 전역 설정, 있으면 그 프로젝트의 설정.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// 지금 적용 중인 설정과, 마지막 다시 읽기의 실패(있으면). 실패해도 `snapshot` 은 직전에 성공한 값이다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub snapshot: ConfigSnapshot,
    pub error: Option<ConfigFault>,
}

impl From<&ConfigStore> for ConfigView {
    fn from(store: &ConfigStore) -> Self {
        Self {
            snapshot: (*store.snapshot()).clone(),
            error: store.fault(),
        }
    }
}

rpc_method! {
    /// 해석된 `$CONFIG`/`$DATA`/`$STATE`/`$CACHE`. 설정 화면의 「설정 폴더 열기」 · 「경로 복사」 가 쓴다.
    pub ConfigPathsMethod: "config.paths" {
        params: (),
        result: ConfigPathsInfo,
        tier: Read,
    }

    /// 병합된 설정 스냅숏. `projectId` 가 없으면 전역(`$CONFIG`), 있으면 그 프로젝트(설정을 붙여 연 프로젝트만).
    pub ConfigGet: "config.get" {
        params: ConfigGetParams,
        result: ConfigView,
        tier: Read,
        file_scope: true,
    }
}
