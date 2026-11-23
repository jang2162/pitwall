//! 묶음 판정이 다른 머신에 묻는 길(18-m7-plan M7-08 — M7-10 이 남긴 「원격 `BundleMachine`」). [`BundleMachine`] 의 메서드마다
//! 같은 이름의 RPC(`machine.*`)가 있고 그 Host 의 [`LocalMachine`] 이 답한다. Desktop 은 원격 Host(`pw_link::RemoteHost`)를
//! [`HostMachine`] 으로 감싸 그 RPC 로 묻는다 — Relay 의 Host 에도 이 메서드가 있어야 한다(`bundle.*` 와 달리 Relay 조립에서
//! 빼지 않는다).
//!
//! - 모두 owner 만이고 **Host 범위 연결만** 받는다(프로젝트 창 · 파일 창 · MCP 세션은 거절) — Desktop 의 묶음 판정이 링크로 여는
//!   연결이 그 범위다. MCP 툴이 아니다.
//! - `machine.register` · `machine.unregister` 는 그 머신 레지스트리에 프로젝트를 만들고 지운다(`project.create` · `project.delete`
//!   와 같은 일 — 폴더는 건드리지 않는다). `unregister` 는 거절된 더하기의 등록을 되돌릴 때만 부른다(M7-10 리뷰).

use std::sync::Arc;

use pw_rpc::{Caller, ConnectionScope, Ctx, Role, Router, RpcError, code, rpc_method};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::add::{AddMemberError, BundleMachine, CheckoutFacts, LocalMachine, MachineFuture};
use super::history::HistoryBottoms;
use crate::registry::Registry;
use crate::{Host, HostError};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachinePathParams {
    /// 그 머신의 절대 경로.
    pub path: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachineProjectParams {
    pub project_id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachineContainedParams {
    pub path: String,
    /// 물어볼 커밋(hex).
    pub commits: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachineReachesFilesParams {
    pub path: String,
    #[specta(type = pw_rpc::JsNumber)]
    pub limit: u64,
}

/// 한 경로의 사실(`CheckoutFacts` 의 와이어 꼴).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachineCheckoutFacts {
    /// 정규화한 경로.
    pub path: String,
    /// 공용 git 폴더. git 저장소가 아니면 `null`.
    pub repository: Option<String>,
    pub history: HistoryBottoms,
    /// 이 경로를 루트로 가진 그 머신의 프로젝트.
    pub project: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachineProjectRoot {
    /// 없는 프로젝트면 `null`.
    pub root: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachineCommits {
    pub commits: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachineReaches {
    pub reached: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MachineRegistered {
    pub project_id: String,
}

rpc_method! {
    /// 묶음 판정: 그 머신의 한 경로의 사실(정규화한 경로 · 공용 git 폴더 · 역사 바닥 · 그 경로를 루트로 가진 프로젝트).
    pub MachineInspect: "machine.inspect" {
        params: MachinePathParams,
        result: MachineCheckoutFacts,
        tier: Read,
        roles: [Owner],
    }

    /// 묶음 판정: 그 머신 프로젝트의 주 루트.
    pub MachineProjectRootMethod: "machine.projectRoot" {
        params: MachineProjectParams,
        result: MachineProjectRoot,
        tier: Read,
        roles: [Owner],
    }

    /// 묶음 판정: `commits` 가운데 `path` 의 저장소가 가진 것.
    pub MachineContained: "machine.contained" {
        params: MachineContainedParams,
        result: MachineCommits,
        tier: Read,
        roles: [Owner],
    }

    /// 묶음 판정: `path` 아래 파일이 `limit` 개 이상인가(`.git` 은 세지 않는다).
    pub MachineReachesFilesMethod: "machine.reachesFiles" {
        params: MachineReachesFilesParams,
        result: MachineReaches,
        tier: Read,
        roles: [Owner],
    }

    /// 묶음 판정: 그 머신의 레지스트리에 `path` 를 루트로 한 프로젝트를 만든다.
    pub MachineRegister: "machine.register" {
        params: MachinePathParams,
        result: MachineRegistered,
        tier: Write,
        roles: [Owner],
    }

    /// 묶음 판정: `machine.register` 로 만든 프로젝트를 지운다(거절된 더하기의 되돌리기 — 폴더는 그대로).
    pub MachineUnregister: "machine.unregister" {
        params: MachineProjectParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

/// Host 범위 연결만.
fn host_scope(ctx: &Ctx) -> Result<(), RpcError> {
    match ctx.scope() {
        ConnectionScope::Host => Ok(()),
        _ => Err(RpcError::new(
            code::PERMISSION_DENIED,
            "machine.* takes only a host connection",
        )),
    }
}

fn machine_error(e: AddMemberError) -> RpcError {
    match e {
        AddMemberError::Registry(e) => crate::registry::methods::rpc_error(e),
        other => RpcError::invalid_params(other.to_string()),
    }
}

/// `machine.*` 핸들러를 붙인다.
pub fn register(router: &mut Router, registry: &Registry) {
    let local = Arc::new(LocalMachine::new(registry.clone()));
    let m = Arc::clone(&local);
    router.unary(MachineInspect, move |ctx, p: MachinePathParams| {
        let m = Arc::clone(&m);
        async move {
            host_scope(&ctx)?;
            let facts = m.inspect(&p.path).await.map_err(machine_error)?;
            Ok(MachineCheckoutFacts {
                path: facts.path,
                repository: facts.repository,
                history: facts.history,
                project: facts.project,
            })
        }
    });
    let m = Arc::clone(&local);
    router.unary(
        MachineProjectRootMethod,
        move |ctx, p: MachineProjectParams| {
            let m = Arc::clone(&m);
            async move {
                host_scope(&ctx)?;
                let root = m.project_root(&p.project_id).await.map_err(machine_error)?;
                Ok(MachineProjectRoot { root })
            }
        },
    );
    let m = Arc::clone(&local);
    router.unary(MachineContained, move |ctx, p: MachineContainedParams| {
        let m = Arc::clone(&m);
        async move {
            host_scope(&ctx)?;
            let commits = m
                .contained(&p.path, &p.commits)
                .await
                .map_err(machine_error)?;
            Ok(MachineCommits { commits })
        }
    });
    let m = Arc::clone(&local);
    router.unary(
        MachineReachesFilesMethod,
        move |ctx, p: MachineReachesFilesParams| {
            let m = Arc::clone(&m);
            async move {
                host_scope(&ctx)?;
                let reached = m
                    .reaches_files(&p.path, p.limit)
                    .await
                    .map_err(machine_error)?;
                Ok(MachineReaches { reached })
            }
        },
    );
    let m = Arc::clone(&local);
    router.unary(MachineRegister, move |ctx, p: MachinePathParams| {
        let m = Arc::clone(&m);
        async move {
            host_scope(&ctx)?;
            let project_id = m.register(&p.path).await.map_err(machine_error)?;
            Ok(MachineRegistered { project_id })
        }
    });
    let m = local;
    router.unary(MachineUnregister, move |ctx, p: MachineProjectParams| {
        let m = Arc::clone(&m);
        async move {
            host_scope(&ctx)?;
            m.unregister(&p.project_id).await.map_err(machine_error)
        }
    });
}

/// 다른 머신의 Host 로 묻는 [`BundleMachine`]. 호출마다 owner · Host 범위 연결을 하나 열고 요청 하나를 보낸 뒤 닫는다.
pub struct HostMachine {
    machine: String,
    host: Arc<dyn Host>,
}

impl HostMachine {
    /// `machine` 은 오류 문장에 쓰는 머신 id.
    pub fn new(machine: impl Into<String>, host: Arc<dyn Host>) -> Self {
        Self {
            machine: machine.into(),
            host,
        }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, AddMemberError> {
        let mut connection =
            self.host
                .connect(Caller::new(Role::Owner))
                .await
                .map_err(|e| match e {
                    HostError::Disconnected(_) | HostError::UnknownHost(_) => {
                        AddMemberError::MachineUnavailable(self.machine.clone())
                    }
                    other => AddMemberError::Machine(format!("{other:?}")),
                })?;
        connection.sink.send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }));
        while let Some(frame) = connection.source.recv().await {
            if frame.get("id") != Some(&json!(1)) {
                continue;
            }
            if let Some(error) = frame.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the remote machine refused");
                return Err(AddMemberError::Machine(format!(
                    "{}: {message}",
                    self.machine
                )));
            }
            let result = frame.get("result").cloned().unwrap_or(Value::Null);
            return serde_json::from_value(result).map_err(|e| {
                AddMemberError::Machine(format!("{}: bad answer to {method}: {e}", self.machine))
            });
        }
        Err(AddMemberError::MachineUnavailable(self.machine.clone()))
    }
}

impl BundleMachine for HostMachine {
    fn inspect<'a>(&'a self, path: &'a str) -> MachineFuture<'a, CheckoutFacts> {
        Box::pin(async move {
            let facts: MachineCheckoutFacts = self
                .call("machine.inspect", json!({ "path": path }))
                .await?;
            Ok(CheckoutFacts {
                path: facts.path,
                repository: facts.repository,
                history: facts.history,
                project: facts.project,
            })
        })
    }

    fn project_root<'a>(&'a self, project_id: &'a str) -> MachineFuture<'a, Option<String>> {
        Box::pin(async move {
            let root: MachineProjectRoot = self
                .call("machine.projectRoot", json!({ "projectId": project_id }))
                .await?;
            Ok(root.root)
        })
    }

    fn contained<'a>(
        &'a self,
        path: &'a str,
        commits: &'a [String],
    ) -> MachineFuture<'a, Vec<String>> {
        Box::pin(async move {
            let found: MachineCommits = self
                .call(
                    "machine.contained",
                    json!({ "path": path, "commits": commits }),
                )
                .await?;
            Ok(found.commits)
        })
    }

    fn reaches_files<'a>(&'a self, path: &'a str, limit: u64) -> MachineFuture<'a, bool> {
        Box::pin(async move {
            let reached: MachineReaches = self
                .call(
                    "machine.reachesFiles",
                    json!({ "path": path, "limit": limit }),
                )
                .await?;
            Ok(reached.reached)
        })
    }

    fn register<'a>(&'a self, path: &'a str) -> MachineFuture<'a, String> {
        Box::pin(async move {
            let made: MachineRegistered = self
                .call("machine.register", json!({ "path": path }))
                .await?;
            Ok(made.project_id)
        })
    }

    fn unregister<'a>(&'a self, project_id: &'a str) -> MachineFuture<'a, ()> {
        Box::pin(async move {
            let _: Value = self
                .call("machine.unregister", json!({ "projectId": project_id }))
                .await?;
            Ok(())
        })
    }
}
