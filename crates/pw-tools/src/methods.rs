//! `tools.registry.*` RPC(15-m4-plan.md M4-03 — tier Read, MCP 에 내지 않는다). 설정 창 「도구」(M4-17)가 읽는다.

use std::collections::BTreeMap;
use std::sync::Arc;

use pw_rpc::{JsNumber, Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};

use crate::index::RegistryOrigin;
use crate::package::{Package, ResolvedTool, ToolDeprecation, ToolUnsupported};
use crate::registry::ToolsRegistry;
use crate::source::RegistryMode;
use crate::target::ToolTarget;

/// 검색 결과의 기본 · 최대 수.
pub const DEFAULT_SEARCH_LIMIT: u32 = 100;
pub const MAX_SEARCH_LIMIT: u32 = 1000;
/// 질의 글의 상한(바이트).
const MAX_QUERY_BYTES: usize = 256;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct UnsupportedCount {
    /// [`ToolUnsupported`] 의 `reason`.
    pub reason: String,
    pub count: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsRegistryStatus {
    /// 지금 쓰는 registry 판(릴리스 태그).
    pub version: String,
    /// 이 Pitwall 판이 고정한 판(`registry.toml`).
    pub pinned_version: String,
    /// 설정 `[tools] registry`.
    pub mode: RegistryMode,
    /// 스냅숏이면 기본 번들 패키지만 있다.
    pub origin: RegistryOrigin,
    /// 받은 때(유닉스 밀리초). 스냅숏이면 `null`.
    #[specta(type = Option<JsNumber>)]
    pub fetched_at_ms: Option<u64>,
    /// 이 기기의 대상. Pitwall 이 지원하지 않는 기기면 `null`(모든 패키지가 지원하지 않음).
    pub target: Option<ToolTarget>,
    pub supported: u32,
    pub unsupported: u32,
    /// 까닭별 수(까닭 이름순).
    pub unsupported_by_reason: Vec<UnsupportedCount>,
    /// 패키지로 읽지 못해 뺀 항목 수.
    pub unreadable: u32,
    /// 지금 뒤에서 가져오는 중.
    pub fetching: bool,
    /// 마지막 가져오기의 실패(성공하면 지운다).
    pub last_error: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsRegistrySearchParams {
    /// 이름 · 설명에서 찾는다(대소문자 무시). 비면 모두.
    pub query: String,
    /// `LSP` · `Formatter` 등(대소문자 무시, 같은 것).
    #[serde(default)]
    pub category: Option<String>,
    /// `Python` 등(대소문자 무시, 같은 것).
    #[serde(default)]
    pub language: Option<String>,
    /// 기본 100, 최대 1000.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolPackageSummary {
    pub name: String,
    pub description: String,
    /// registry 가 가리키는 판.
    pub version: String,
    pub licenses: Vec<String>,
    pub languages: Vec<String>,
    pub categories: Vec<String>,
    /// 기본 번들(Pitwall 이 해시를 고정한다).
    pub bundled: bool,
    pub deprecated: bool,
    /// 이 기기에서 설치할 수 없는 까닭. 설치할 수 있으면 `null`.
    pub unsupported: Option<ToolUnsupported>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsRegistrySearch {
    /// 이름이 같은 것 → 이름이 질의로 시작하는 것 → 이름에 든 것 → 설명에 든 것, 같으면 이름순.
    pub items: Vec<ToolPackageSummary>,
    /// `limit` 에서 잘랐다.
    pub truncated: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolsRegistryGetParams {
    pub name: String,
}

/// 고정 해시 하나(기본 번들).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct PinnedFile {
    pub name: String,
    pub url: String,
    pub sha256: String,
    #[specta(type = JsNumber)]
    pub size: u64,
    pub target: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolPackageDetail {
    pub summary: ToolPackageSummary,
    pub homepage: String,
    /// `source.id`(purl).
    pub source_id: String,
    pub deprecation: Option<ToolDeprecation>,
    /// 이 기기에서 받을 것 · 실행할 것. 지원하지 않으면 `null`(까닭은 `summary.unsupported`).
    pub resolved: Option<ResolvedTool>,
    /// 받을 파일의 고정 해시(기본 번들이고 판이 고정 판과 같을 때). 없으면 빈 목록.
    pub pinned: Vec<PinnedFile>,
    /// 이 정의를 읽은 registry 판.
    pub registry_version: String,
}

rpc_method! {
    /// registry 의 판 · 받은 때 · 이 기기에서 지원하는 패키지 수와 지원하지 않는 까닭별 수. 처음 부르면 캐시를 읽고 가져오기를
    /// 뒤에서 시작한다(`fetching`).
    pub ToolsRegistryStatusMethod: "tools.registry.status" {
        params: (),
        result: ToolsRegistryStatus,
        tier: Read,
    }

    /// 패키지 찾기(이름 · 설명, 분류 · 언어 거르기).
    pub ToolsRegistrySearchMethod: "tools.registry.search" {
        params: ToolsRegistrySearchParams,
        result: ToolsRegistrySearch,
        tier: Read,
    }

    /// 패키지 하나의 정의와 이 기기에서 받을 것. 없으면 not found.
    pub ToolsRegistryGetMethod: "tools.registry.get" {
        params: ToolsRegistryGetParams,
        result: ToolPackageDetail,
        tier: Read,
    }
}

pub fn register(router: &mut Router, registry: Arc<ToolsRegistry>) {
    let (status, search) = (Arc::clone(&registry), Arc::clone(&registry));
    router
        .unary(ToolsRegistryStatusMethod, move |_ctx, ()| {
            let r = Arc::clone(&status);
            async move {
                r.ensure().await;
                Ok(status_of(&r))
            }
        })
        .unary(
            ToolsRegistrySearchMethod,
            move |_ctx, params: ToolsRegistrySearchParams| {
                let r = Arc::clone(&search);
                async move {
                    r.ensure().await;
                    search_in(&r, &params)
                }
            },
        )
        .unary(
            ToolsRegistryGetMethod,
            move |_ctx, params: ToolsRegistryGetParams| {
                let r = Arc::clone(&registry);
                async move {
                    r.ensure().await;
                    get_in(&r, &params.name)
                }
            },
        );
}

pub fn status_of(r: &ToolsRegistry) -> ToolsRegistryStatus {
    let index = r.index();
    let target = ToolTarget::current();
    let stats = index.stats(target, r.overrides());
    ToolsRegistryStatus {
        version: index.version.clone(),
        pinned_version: r.pinned().registry.version.clone(),
        mode: r.mode(),
        origin: index.origin,
        fetched_at_ms: index.fetched_at_ms,
        target,
        supported: stats.supported as u32,
        unsupported: stats.unsupported as u32,
        unsupported_by_reason: stats
            .by_reason
            .iter()
            .map(|(reason, count)| UnsupportedCount {
                reason: (*reason).to_owned(),
                count: *count as u32,
            })
            .collect(),
        unreadable: index.unreadable as u32,
        fetching: r.fetching(),
        last_error: r.last_error(),
    }
}

fn summary(r: &ToolsRegistry, p: &Package, target: Option<ToolTarget>) -> ToolPackageSummary {
    ToolPackageSummary {
        name: p.name.clone(),
        description: p.description.clone(),
        version: p.version().to_owned(),
        licenses: p.licenses.clone(),
        languages: p.languages.clone(),
        categories: p.categories.clone(),
        bundled: r.pinned().is_bundled(&p.name),
        deprecated: p.deprecation.is_some(),
        unsupported: p.resolve(target, r.overrides().get(&p.name)).err(),
    }
}

pub fn search_in(
    r: &ToolsRegistry,
    params: &ToolsRegistrySearchParams,
) -> Result<ToolsRegistrySearch, RpcError> {
    if params.query.len() > MAX_QUERY_BYTES {
        return Err(RpcError::invalid_params(format!(
            "query is longer than {MAX_QUERY_BYTES} bytes"
        )));
    }
    let limit = params.limit.unwrap_or(DEFAULT_SEARCH_LIMIT);
    if limit == 0 || limit > MAX_SEARCH_LIMIT {
        return Err(RpcError::invalid_params(format!(
            "limit must be 1..={MAX_SEARCH_LIMIT}"
        )));
    }
    let query = params.query.trim().to_lowercase();
    let eq = |list: &[String], want: &Option<String>| match want {
        None => true,
        Some(w) => list.iter().any(|x| x.eq_ignore_ascii_case(w)),
    };
    let index = r.index();
    let mut ranked: BTreeMap<(u8, String), &Package> = BTreeMap::new();
    for p in index.packages() {
        if !eq(&p.categories, &params.category) || !eq(&p.languages, &params.language) {
            continue;
        }
        let name = p.name.to_lowercase();
        let rank = if query.is_empty() || name == query {
            0
        } else if name.starts_with(&query) {
            1
        } else if name.contains(&query) {
            2
        } else if p.description.to_lowercase().contains(&query) {
            3
        } else {
            continue;
        };
        ranked.insert((rank, p.name.clone()), p);
    }
    let target = ToolTarget::current();
    let truncated = ranked.len() > limit as usize;
    let items = ranked
        .values()
        .take(limit as usize)
        .map(|p| summary(r, p, target))
        .collect();
    Ok(ToolsRegistrySearch { items, truncated })
}

pub fn get_in(r: &ToolsRegistry, name: &str) -> Result<ToolPackageDetail, RpcError> {
    let index = r.index();
    let p = index.get(name).ok_or_else(|| {
        RpcError::new(code::NOT_FOUND, "not found").with_data(serde_json::Value::String(
            name.chars().take(MAX_QUERY_BYTES).collect(),
        ))
    })?;
    let target = ToolTarget::current();
    let resolved = p.resolve(target, r.overrides().get(&p.name)).ok();
    let pinned = match (&resolved, target) {
        (Some(res), Some(t)) => r
            .pinned()
            .artifacts_for(&p.name, &res.version, t)
            .into_iter()
            .map(|a| PinnedFile {
                name: a.name.clone(),
                url: a.url.clone(),
                sha256: a.sha256.clone(),
                size: a.size,
                target: a.target.clone(),
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(ToolPackageDetail {
        summary: summary(r, p, target),
        homepage: p.homepage.clone(),
        source_id: p.source_id.clone(),
        deprecation: p.deprecation.clone(),
        resolved,
        pinned,
        registry_version: index.version.clone(),
    })
}
