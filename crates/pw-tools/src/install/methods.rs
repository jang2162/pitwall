//! 설치 RPC(15-m4-plan.md M4-04 · 결정 M4-D18). MCP 에 내지 않는다(`mcp` 없음 — 설치는 사람이 고른 작업으로만, 결정 M4-D17).
//!
//! - 읽기(tier Read): `tools.list` · `tools.updates` · `tools.log`.
//! - 바꾸기(tier Exec — owner 만. npm · pip 을 띄우고 실행 파일을 만든다): `tools.install` · `tools.update` · `tools.rollback` ·
//!   `tools.uninstall`. 연결은 설정 창 또는 프로젝트 창만([`super::grant`]).

use std::sync::Arc;

use pw_rpc::{Ctx, JsNumber, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::{InstallOutcome, ToolsInstaller, consent, source_domains, tool_grant};
use crate::methods::{
    ToolChecksum, ToolsRegistrySetModeMethod, ToolsRegistrySetModeParams, status_of,
};
use crate::package::PurlKind;
use crate::target::ToolTarget;

/// 설치 로그를 돌려주는 바이트 상한(끝에서부터).
const MAX_LOG_BYTES: usize = 256 * 1024;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsInstallParams {
    pub name: String,
    /// 주면 registry 의 판과 같아야 한다(다른 판은 설치하지 않는다).
    #[serde(default)]
    pub version: Option<String>,
    /// [`super::code::CONFIRM_REQUIRED`] 를 받은 뒤 사람이 확인한 값.
    #[serde(default)]
    pub confirmed: Option<ToolConfirmed>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsUpdateParams {
    pub name: String,
    #[serde(default)]
    pub confirmed: Option<ToolConfirmed>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsNameParams {
    pub name: String,
}

/// 사람이 확인한 값 — [`ToolConfirmRequired`] 의 셋을 그대로 돌려준다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfirmed {
    pub version: String,
    pub licenses: Vec<String>,
    pub source_domains: Vec<String>,
}

/// [`super::code::CONFIRM_REQUIRED`](4001)의 `data`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfirmRequired {
    pub name: String,
    pub version: String,
    /// Mason `licenses` 위에 Pitwall 의 덮어쓰기 표(`license-notes.toml`)를 먼저 본 것.
    pub licenses: Vec<String>,
    /// 받는 곳의 도메인(이름순). 라이선스 · 이것이 바뀐 판은 다시 묻는다.
    pub source_domains: Vec<String>,
    /// 받을 바이트(고정 표에 있을 때만).
    #[specta(type = Option<JsNumber>)]
    pub size: Option<u64>,
    /// 받을 것에 고정 해시가 있나(`none` 이면 「체크섬 없음」 을 함께 보인다 — 결정 M4-D17).
    pub checksum: ToolChecksum,
    /// 덮어쓰기 표의 설명(있으면).
    pub note: Option<String>,
    /// 앞서 실은 `confirmed` 를 받지 않았으면 `changed`(다이얼로그를 띄운 뒤 판 · 라이선스 · 출처가 바뀌었다). 처음이면 `null`.
    pub rejected: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolInstallResult {
    pub name: String,
    /// 지금 판.
    pub version: String,
    /// 되돌리기로 갈 판.
    pub previous: Option<String>,
    pub outcome: InstallOutcome,
    pub checksum: ToolChecksum,
    /// 설치 작업(`task.*`). 받지 않았으면 `null`.
    #[specta(type = Option<JsNumber>)]
    pub task_id: Option<u64>,
    /// 다른 창이 먼저 시작한 같은 패키지의 설치에 합류했다.
    pub joined: bool,
}

/// 설치본 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolInstalledInfo {
    pub name: String,
    pub version: String,
    pub previous: Option<String>,
    pub kind: PurlKind,
    pub licenses: Vec<String>,
    pub checksum: ToolChecksum,
    /// 설치 폴더의 바이트.
    #[specta(type = JsNumber)]
    pub size: u64,
    #[specta(type = JsNumber)]
    pub installed_at_ms: u64,
    pub source_urls: Vec<String>,
    /// 실행 파일 이름들.
    pub bins: Vec<String>,
    /// 쓸 수 없는 까닭(다시 설치 필요 — venv 를 만든 python 이 사라짐 등). 쓸 수 있으면 `null`.
    pub problem: Option<String>,
}

/// 설치 기록을 읽지 못한 패키지.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolBroken {
    pub name: String,
    pub problem: String,
}

/// 도는 설치.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolInstalling {
    pub name: String,
    pub version: String,
    #[specta(type = JsNumber)]
    pub task_id: u64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsList {
    pub items: Vec<ToolInstalledInfo>,
    pub unreadable: Vec<ToolBroken>,
    pub installing: Vec<ToolInstalling>,
}

/// 업데이트가 있는 설치본.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolUpdateInfo {
    pub name: String,
    pub installed: String,
    /// registry(지금 쓰는 판)가 가리키는 판.
    pub available: String,
    /// 라이선스나 출처 도메인이 바뀌었거나, 고정 해시로 확인한 도구가 「체크섬 없음」 판이 되어 업데이트 전에 다시 묻는다.
    pub confirm_required: bool,
    /// 받을 판에 고정 해시가 있나(`none` 이면 「체크섬 없음」 — 알림 · 버튼이 경고한다).
    pub checksum: ToolChecksum,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsUpdates {
    pub items: Vec<ToolUpdateInfo>,
    /// 견준 registry 판.
    pub registry_version: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolLog {
    /// 설치 로그(끝에서부터 상한까지). 로그가 없으면 빈 글.
    pub text: String,
    pub truncated: bool,
}

rpc_method! {
    /// 설치본 목록(판 · 이전 판 · 크기 · 라이선스 · 체크섬 유무 · 쓸 수 없는 까닭)과 도는 설치.
    pub ToolsListMethod: "tools.list" {
        params: (),
        result: ToolsList,
        tier: Read,
    }

    /// 설치본 가운데 registry 의 판이 다른 것(업데이트 알림은 M4-17).
    pub ToolsUpdatesMethod: "tools.updates" {
        params: (),
        result: ToolsUpdates,
        tier: Read,
    }

    /// 패키지의 설치 로그.
    pub ToolsLogMethod: "tools.log" {
        params: ToolsNameParams,
        result: ToolLog,
        tier: Read,
    }

    /// registry 의 판을 설치한다(작업이 끝날 때까지 기다린다 — 진행 · 취소는 `task.*`). 확인 기록이 없거나 라이선스 · 출처가 바뀌었으면
    /// 4001, npm · python 이 없으면 4002, 설치 실패 4003, 지원하지 않음 4004, 취소 4007. 같은 패키지의 설치가 돌면 그 결과를 받는다.
    pub ToolsInstallMethod: "tools.install" {
        params: ToolsInstallParams,
        result: ToolInstallResult,
        tier: Exec,
    }

    /// 설치본을 registry 의 판으로 올린다(새 판은 다른 폴더에 다 만든 뒤 링크만 바꾸고 이전 판 하나를 남긴다). 설치돼 있지 않으면 4006.
    pub ToolsUpdateMethod: "tools.update" {
        params: ToolsUpdateParams,
        result: ToolInstallResult,
        tier: Exec,
    }

    /// 남겨 둔 이전 판으로 링크를 돌린다(지금 판이 이전 판이 된다). 이전 판이 없으면 4006.
    pub ToolsRollbackMethod: "tools.rollback" {
        params: ToolsNameParams,
        result: ToolInstalledInfo,
        tier: Exec,
    }

    /// 설치본을 지운다(쓰는 서버가 있으면 멈춘 뒤). 확인 기록은 남긴다.
    pub ToolsUninstallMethod: "tools.uninstall" {
        params: ToolsNameParams,
        result: (),
        tier: Exec,
    }
}

// `4001` 의 data 는 메서드 꼴에 들지 않아 생성물로 따로 낸다 — 확인 다이얼로그(M4-17)가 읽는다.
pw_rpc::rpc_type!(ToolConfirmRequired);

fn grant_of(installer: &ToolsInstaller, ctx: &Ctx) -> Result<super::ToolGrant, RpcError> {
    let host = installer.host()?;
    let settings = Arc::clone(&host.settings_window);
    tool_grant(ctx.scope(), ctx.window(), &move |label| settings(label))
}

pub fn register(router: &mut Router, installer: Arc<ToolsInstaller>) {
    let i = || Arc::clone(&installer);
    let (list, updates, log, install, update, rollback, uninstall, set_mode) =
        (i(), i(), i(), i(), i(), i(), i(), i());
    router
        .unary(
            ToolsRegistrySetModeMethod,
            move |ctx, params: ToolsRegistrySetModeParams| {
                let r = Arc::clone(&set_mode);
                let grant = grant_of(&r, &ctx);
                async move {
                    grant?;
                    let registry = r.registry();
                    let w = Arc::clone(registry);
                    tokio::task::spawn_blocking(move || w.set_mode(params.mode))
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?
                        .map_err(RpcError::internal)?;
                    registry.ensure().await;
                    Ok(status_of(registry))
                }
            },
        )
        .unary(ToolsListMethod, move |_ctx, ()| {
            let r = Arc::clone(&list);
            async move { list_of(&r) }
        })
        .unary(ToolsUpdatesMethod, move |_ctx, ()| {
            let r = Arc::clone(&updates);
            async move {
                r.registry().ensure().await;
                updates_of(&r)
            }
        })
        .unary(ToolsLogMethod, move |_ctx, params: ToolsNameParams| {
            let r = Arc::clone(&log);
            async move {
                let text = r.read_log(&params.name)?.unwrap_or_default();
                Ok(tail(text))
            }
        })
        .unary(
            ToolsInstallMethod,
            move |ctx, params: ToolsInstallParams| {
                let r = Arc::clone(&install);
                let grant = grant_of(&r, &ctx);
                async move {
                    let grant = grant?;
                    r.install(
                        &grant,
                        &params.name,
                        params.version.as_deref(),
                        params.confirmed.as_ref(),
                        false,
                    )
                    .await
                }
            },
        )
        .unary(ToolsUpdateMethod, move |ctx, params: ToolsUpdateParams| {
            let r = Arc::clone(&update);
            let grant = grant_of(&r, &ctx);
            async move {
                let grant = grant?;
                r.install(&grant, &params.name, None, params.confirmed.as_ref(), true)
                    .await
            }
        })
        .unary(ToolsRollbackMethod, move |ctx, params: ToolsNameParams| {
            let r = Arc::clone(&rollback);
            let grant = grant_of(&r, &ctx);
            async move {
                let grant = grant?;
                let record = r.rollback(&grant, &params.name).await?;
                info_of(&r, &params.name, &record).ok_or_else(|| {
                    RpcError::internal("the rolled back record has no current version")
                })
            }
        })
        .unary(ToolsUninstallMethod, move |ctx, params: ToolsNameParams| {
            let r = Arc::clone(&uninstall);
            let grant = grant_of(&r, &ctx);
            async move {
                let grant = grant?;
                r.uninstall(&grant, &params.name).await
            }
        });
}

fn info_of(
    r: &ToolsInstaller,
    name: &str,
    record: &super::InstallRecord,
) -> Option<ToolInstalledInfo> {
    let entry = record.current_entry()?;
    Some(ToolInstalledInfo {
        name: name.to_owned(),
        version: record.current.clone(),
        previous: record.previous.clone(),
        kind: entry.kind,
        licenses: entry.licenses.clone(),
        checksum: entry.checksum,
        size: entry.size,
        installed_at_ms: entry.installed_at_ms,
        source_urls: entry.source_urls.clone(),
        bins: entry.bins.keys().cloned().collect(),
        problem: r.problem_of(name, record),
    })
}

pub fn list_of(r: &ToolsInstaller) -> Result<ToolsList, RpcError> {
    let mut items = Vec::new();
    let mut unreadable = Vec::new();
    for (name, record) in r.installed()? {
        match record {
            Ok(rec) => match info_of(r, &name, &rec) {
                Some(info) => items.push(info),
                None => unreadable.push(ToolBroken {
                    name,
                    problem: "the install record has no current version".into(),
                }),
            },
            Err(problem) => unreadable.push(ToolBroken { name, problem }),
        }
    }
    let installing = r
        .running()
        .into_iter()
        .map(|(name, (version, task_id))| ToolInstalling {
            name,
            version,
            task_id,
        })
        .collect();
    Ok(ToolsList {
        items,
        unreadable,
        installing,
    })
}

pub fn updates_of(r: &ToolsInstaller) -> Result<ToolsUpdates, RpcError> {
    let index = r.registry().index();
    let target = ToolTarget::current();
    let store = r.consent_store()?;
    let mut items = Vec::new();
    for (name, record) in r.installed()? {
        let Ok(record) = record else { continue };
        let Some(package) = index.get(&name) else {
            continue;
        };
        let Ok(resolved) = package.resolve(target, r.registry().overrides().get(&name)) else {
            continue;
        };
        if resolved.version == record.current {
            continue;
        }
        let licenses = r.notes().effective(&name, &package.licenses);
        let domains = source_domains(&resolved);
        let checksum = if r.registry().pinned().pinned_for(&resolved).is_some() {
            ToolChecksum::Pinned
        } else {
            ToolChecksum::None
        };
        let confirm_required = !store
            .get(&name)
            .is_some_and(|c| consent::covers(&c, &licenses, &domains, checksum));
        items.push(ToolUpdateInfo {
            name,
            installed: record.current,
            available: resolved.version,
            confirm_required,
            checksum,
        });
    }
    Ok(ToolsUpdates {
        items,
        registry_version: index.version.clone(),
    })
}

fn tail(text: String) -> ToolLog {
    if text.len() <= MAX_LOG_BYTES {
        return ToolLog {
            text,
            truncated: false,
        };
    }
    let mut start = text.len() - MAX_LOG_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    ToolLog {
        text: text[start..].to_owned(),
        truncated: true,
    }
}
