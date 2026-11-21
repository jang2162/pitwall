//! 묶음 RPC(`bundle.*`, 18-m7-plan M7-10). 묶음은 Desktop 의 것이라 모두 owner 만이고 **MCP 툴로 내놓지 않는다** — 에이전트는
//! M7-18 · 19 의 세션 툴(`checkout` 입력 · `checkout_list`)로 묶음을 읽는다. 읽기는 `read`, 바꾸기는 `write` 다. 멤버를 빼는 것은
//! 묶음의 기록만 바꾸고 그 머신의 등록 · 파일은 그대로라 `dangerous` 가 아니다.

use pw_rpc::{Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};

use super::add::{AddMemberError, AddMemberOutcome, AddTo, add_member};
use super::{CheckoutRef, ProjectBundle};
use crate::registry::methods::rpc_error;
use crate::registry::{Registry, RegistryError};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct BundleIdParams {
    pub id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct AddBundleMemberParams {
    /// 더할 묶음(암묵 묶음이면 로컬 프로젝트 id). `null` 이면 원격만인 새 묶음을 만든다(원격 호스트 시트).
    #[serde(default)]
    pub bundle_id: Option<String>,
    /// 머신 id(이 머신이면 `local`).
    pub machine: String,
    /// 그 머신의 절대 경로.
    pub path: String,
    /// 파일이 많은 원격 경로의 경고(`largeDirectory`)를 사람이 넘겼다.
    #[serde(default)]
    pub allow_large: Option<bool>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoveBundleMemberParams {
    pub bundle_id: String,
    pub machine: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SetLastSendTargetParams {
    pub bundle_id: String,
    /// 원본 멤버의 머신 id.
    pub source: String,
    /// `null` 이면 지운다.
    #[serde(default)]
    pub target: Option<CheckoutRef>,
}

rpc_method! {
    /// 묶음 전부(이름순): 행 있는 묶음과, 어느 묶음에도 들지 않은 로컬 프로젝트의 암묵 묶음(id = 프로젝트 id).
    pub BundleList: "bundle.list" {
        params: (),
        result: Vec<ProjectBundle>,
        tier: Read,
        roles: [Owner],
    }

    /// 묶음 하나. 없으면 invalid params 오류.
    pub BundleGet: "bundle.get" {
        params: BundleIdParams,
        result: ProjectBundle,
        tier: Read,
        roles: [Owner],
    }

    /// 「체크아웃 추가」: 머신과 그 머신의 경로를 묶음의 멤버로 더한다. 판정에서 멈추면 `rejected` · `largeDirectory` 를
    /// 돌려준다(오류가 아니다). 그 머신에 닿지 못하면 not found 오류(`data.reason = "machineUnavailable"`).
    pub BundleAddMember: "bundle.addMember" {
        params: AddBundleMemberParams,
        result: AddMemberOutcome,
        tier: Write,
        roles: [Owner],
    }

    /// 멤버를 묶음에서만 뺀다. 그 머신의 프로젝트 등록과 파일은 그대로 둔다.
    pub BundleRemoveMember: "bundle.removeMember" {
        params: RemoveBundleMemberParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// 원본 멤버마다 마지막 보내기 대상을 적는다(보내기 화면이 다음에 먼저 고른다).
    pub BundleSetLastSendTarget: "bundle.setLastSendTarget" {
        params: SetLastSendTargetParams,
        result: ProjectBundle,
        tier: Write,
        roles: [Owner],
    }
}

fn add_error(e: AddMemberError) -> RpcError {
    match e {
        AddMemberError::Registry(e) => rpc_error(e),
        AddMemberError::MachineUnavailable(machine) => RpcError::new(
            code::NOT_FOUND,
            format!("machine `{machine}` is not connected"),
        )
        .with_data(serde_json::json!({ "reason": "machineUnavailable", "machine": machine })),
        AddMemberError::Machine(e) => RpcError::internal(e),
    }
}

/// `bundle.*` 핸들러를 붙인다(`registry::methods::register` 가 부른다).
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
    router.unary(BundleList, move |_ctx, ()| {
        blocking(r.clone(), Registry::bundles)
    });
    let r = registry.clone();
    router.unary(BundleGet, move |_ctx, params| {
        blocking(r.clone(), move |r| r.bundle(&params.id))
    });
    let r = registry.clone();
    router.unary(BundleAddMember, move |_ctx, params| {
        let r = r.clone();
        async move {
            let to = match params.bundle_id.as_deref() {
                Some(id) => AddTo::Bundle(id),
                None => AddTo::NewRemote,
            };
            add_member(
                &r,
                to,
                &params.machine,
                &params.path,
                params.allow_large.unwrap_or(false),
            )
            .await
            .map_err(add_error)
        }
    });
    let r = registry.clone();
    router.unary(BundleRemoveMember, move |_ctx, params| {
        blocking(r.clone(), move |r| {
            r.remove_bundle_member(&params.bundle_id, &params.machine)
        })
    });
    let r = registry.clone();
    router.unary(BundleSetLastSendTarget, move |_ctx, params| {
        blocking(r.clone(), move |r| {
            r.set_last_send_target(&params.bundle_id, &params.source, params.target.as_ref())
        })
    });
}
