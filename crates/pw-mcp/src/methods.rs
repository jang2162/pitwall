//! 사람의 화면이 부르는 `mcp.*` RPC(16-m5-plan.md M5-10): 세션 목록 · 툴 목록과 노출 판정 · 감사 로그.
//!
//! - **에이전트에게 열지 않는다** — 선언에 `mcp` 를 달지 않는다(에이전트가 자기 감사 로그 · 노출 설정을 읽는 툴은 05 에 없다).
//!   role 은 owner 만, 파일 범위 연결(단일 파일 창)도 부르지 못한다(`file_scope` 를 달지 않는다).
//! - 문장(오류 글)은 사람이 보는 화면 쪽이지만 RPC 오류 글은 지금 저장소의 다른 RPC 처럼 영어다. 화면 글자는 웹뷰의 i18n 이다.
//! - 이벤트: [`crate::sessions`] 의 `mcp.sessionsChanged` · `mcp.activity`. payload 는 수 · id 뿐이라 화면은 받으면 이 RPC 를
//!   다시 부른다.
//! - 다른 행이 더하는 메서드: `mcp.audit.clear`(M5-11), `mcp.approvals` · `mcp.approval.answer`(M5-12), `mcp.settings.set`(M5-15).
//!   더할 때 [`register`] 에 붙이고, 필요한 것은 [`HostParts`] 에서 꺼낸다.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pw_core::config::{AppPaths, ConfigStore};
use pw_core::registry::Registry as Projects;
use pw_rpc::{Router, RpcError, Tier, rpc_method};
use serde::{Deserialize, Serialize};

use crate::audit::{self, AuditLog, AuditRow, Outcome};
use crate::catalog::Catalog;
use crate::exposure::{self, Exposure, ExposureRule};
use crate::sessions::{McpSessionInfo, Sessions};

/// `mcp.audit.list` 의 `limit` 상한.
pub const MAX_AUDIT_LIMIT: u32 = 500;
/// `mcp.tools.list` 의 「최근 호출 수」 가 세는 기간.
pub const RECENT: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpToolsListParams {
    /// 그 프로젝트의 설정(전역 위에 덮은 것 — 공유 설정은 좁히기만)으로 판정한다. 없으면 전역 설정만.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// 카탈로그의 툴 하나와 지금 노출 판정.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    /// 툴 설명(선언의 문서 주석 — 에이전트가 읽는 영어 글이다).
    pub description: String,
    pub tier: Tier,
    /// 선언의 그룹(`pw_mcp::catalog::GROUPS`).
    pub group: String,
    /// 입력 JSON Schema(세션 프로젝트가 없을 때의 꼴 — `projectId` 가 필수).
    #[specta(type = pw_rpc::JsonValue)]
    pub input_schema: serde_json::Value,
    /// 지금 켜졌는가.
    pub enabled: bool,
    /// 켜거나 끈 규칙(`pw_mcp::exposure::Exposure::verdict` 그대로).
    pub rule: ExposureRule,
    /// 그 규칙의 키: 툴 이름 · glob · 그룹 · 프리셋 이름. `shared` 는 `null`.
    pub key: Option<String>,
    /// 최근([`RECENT`] — 30일) 감사 행 수. `projectId` 가 있으면 그 프로젝트의 행만.
    #[specta(type = pw_rpc::JsNumber)]
    pub recent_calls: u64,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpAuditListParams {
    /// 그 프로젝트의 행과 프로젝트 없는 행(「에이전트 활동」 패널이 보이는 것). 없으면 전부.
    #[serde(default)]
    pub project_id: Option<String>,
    /// 앞 쪽의 `nextCursor`. 없으면 가장 최신부터.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 1 ~ [`MAX_AUDIT_LIMIT`](500). 없으면 `pw_rpc::DEFAULT_LIST_LIMIT`(100).
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub outcome: Option<Outcome>,
    /// 툴 이름(정확히 같은 것).
    #[serde(default)]
    pub tool: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpAuditPage {
    /// 최신부터.
    pub entries: Vec<AuditRow>,
    /// 다음(더 오래된) 쪽의 커서. 마지막 쪽이면 `null`.
    pub next_cursor: Option<String>,
}

rpc_method! {
    /// 이 앱에 열린 MCP 세션(시작 순서): id · 클라이언트 · 세션 프로젝트 · 전송 · 시작 시각 · 호출 수. 열고 닫히면
    /// `mcp.sessionsChanged`(Host 범위)가 나간다. 앱 없이 뜬 `pitwall mcp` 의 헤드리스 세션은 보이지 않는다.
    pub McpSessions: "mcp.sessions" {
        params: (),
        result: Vec<McpSessionInfo>,
        tier: Read,
        roles: [Owner],
    }

    /// MCP 툴 카탈로그 전부와 지금 노출 판정(켜졌는지 · 그 규칙), 최근 호출 수. 이 앱의 Host 가 처리하는 툴만이다.
    pub McpToolsList: "mcp.tools.list" {
        params: McpToolsListParams,
        result: Vec<McpToolInfo>,
        tier: Read,
        roles: [Owner],
    }

    /// 감사 로그를 최신부터 쪽 단위로 읽는다. 다른 프로세스(헤드리스 `pitwall mcp` · `pitwall tool`)가 쓴 행도 있다 —
    /// 그 행은 `mcp.activity` 가 나가지 않으므로 화면이 열릴 때 · 창이 앞으로 올 때 다시 읽는다.
    pub McpAuditList: "mcp.audit.list" {
        params: McpAuditListParams,
        result: McpAuditPage,
        tier: Read,
        roles: [Owner],
    }
}

/// Host 를 만든 뒤 붙이는 것.
pub struct HostParts {
    pub paths: AppPaths,
    /// 전역 설정(`[mcp]`).
    pub global: ConfigStore,
    pub projects: Projects,
    /// 이 Host 가 처리하는 툴(`Catalog::linked().routed_by(host.router())` — 세션의 카탈로그와 같은 규칙).
    pub catalog: Arc<Catalog>,
    /// 감사 로그 파일(`$STATE/mcp/audit.db`). 세션이 쓰는 것과 다른 인스턴스여도 같은 파일이면 된다.
    pub audit: Arc<AuditLog>,
}

/// `mcp.*` RPC 의 상태. 세션 등록부는 소켓 세션들([`crate::Services::sessions`])과 같은 것을 넣는다.
pub struct Mcp {
    sessions: Sessions,
    parts: OnceLock<HostParts>,
}

impl Mcp {
    pub fn new(sessions: Sessions) -> Self {
        Self {
            sessions,
            parts: OnceLock::new(),
        }
    }

    pub fn sessions(&self) -> &Sessions {
        &self.sessions
    }

    /// Host 를 만든 뒤 한 번. 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.parts.set(parts);
    }

    fn parts(&self) -> Result<&HostParts, RpcError> {
        self.parts
            .get()
            .ok_or_else(|| RpcError::internal("the mcp status is not attached to the host yet"))
    }

    /// `mcp.tools.list`. 디스크 IO(설정 · 감사 로그)를 한다.
    pub fn tools(&self, params: &McpToolsListParams) -> Result<Vec<McpToolInfo>, RpcError> {
        let parts = self.parts()?;
        let exposure: Exposure = match &params.project_id {
            Some(id) => {
                let project = parts
                    .projects
                    .get(id)
                    .map_err(pw_core::registry::methods::rpc_error)?;
                exposure::of_project(&parts.paths, &parts.global, &project)
            }
            None => exposure::layered(
                parts.global.snapshot().get("settings", exposure::KEY),
                None,
                None,
            ),
        };
        let since = now_millis().saturating_sub(i64::try_from(RECENT.as_millis()).unwrap_or(0));
        let counts = parts
            .audit
            .tool_counts(since, params.project_id.as_deref())
            .unwrap_or_else(|e| {
                // 감사 파일이 깨져도 목록은 보인다 — 수만 0 이다.
                tracing::warn!(%e, "cannot count recent mcp calls");
                Default::default()
            });
        Ok(parts
            .catalog
            .tools()
            .iter()
            .map(|tool| {
                let verdict = exposure.verdict(tool);
                McpToolInfo {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    tier: tool.tier,
                    group: tool.group.to_owned(),
                    input_schema: serde_json::Value::Object(tool.input_schema_for(false)),
                    enabled: verdict.enabled,
                    rule: verdict.rule,
                    key: verdict.key,
                    recent_calls: counts.get(&tool.name).copied().unwrap_or(0),
                }
            })
            .collect())
    }

    /// `mcp.audit.list`. 디스크 IO 를 한다.
    pub fn audit(&self, params: &McpAuditListParams) -> Result<McpAuditPage, RpcError> {
        let parts = self.parts()?;
        let limit = params.limit.unwrap_or(pw_rpc::DEFAULT_LIST_LIMIT);
        if limit == 0 || limit > MAX_AUDIT_LIMIT {
            return Err(RpcError::invalid_params(format!(
                "limit must be between 1 and {MAX_AUDIT_LIMIT}, got {limit}"
            )));
        }
        let before = params
            .cursor
            .as_deref()
            .map(|c| {
                c.parse::<i64>()
                    .map_err(|_| RpcError::invalid_params(format!("bad cursor `{c}`")))
            })
            .transpose()?;
        let (entries, more) = parts
            .audit
            .list(&audit::Query {
                project_id: params.project_id.clone(),
                before,
                limit: limit as usize,
                outcome: params.outcome,
                tool: params.tool.clone(),
            })
            .map_err(RpcError::internal)?;
        let next_cursor = more
            .then(|| entries.last().map(|r| r.id.to_string()))
            .flatten();
        Ok(McpAuditPage {
            entries,
            next_cursor,
        })
    }
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, mcp: Arc<Mcp>) {
    let sessions = mcp.sessions.clone();
    router.unary(McpSessions, move |_ctx, ()| {
        let sessions = sessions.clone();
        async move { Ok::<_, RpcError>(sessions.list()) }
    });
    let tools = Arc::clone(&mcp);
    router.unary(McpToolsList, move |_ctx, params: McpToolsListParams| {
        let mcp = Arc::clone(&tools);
        async move { blocking(move || mcp.tools(&params)).await }
    });
    router.unary(McpAuditList, move |_ctx, params: McpAuditListParams| {
        let mcp = Arc::clone(&mcp);
        async move { blocking(move || mcp.audit(&params)).await }
    });
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
