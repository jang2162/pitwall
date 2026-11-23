//! 열린 MCP 세션의 등록부(16-m5-plan.md M5-10 ①)와 상태 이벤트.
//!
//! - 세션은 [`crate::server::serve`] 가 초기화를 마친 뒤 등록하고, 세션이 끝나면(가드 [`Registered`] 가 버려지면) 빠진다.
//!   열고 닫을 때마다 [`TOPIC_SESSIONS_CHANGED`](Host 범위)가 한 번씩 나간다. 호출 수 · 클라이언트가 바뀌는 것은 이벤트를
//!   내지 않는다 — 화면은 [`TOPIC_ACTIVITY`] 를 받으면 `mcp.sessions` 를 다시 읽는다.
//! - 감사 행이 실제로 쓰이면 [`TOPIC_ACTIVITY`] 가 나간다(그 프로젝트 범위, 프로젝트 없는 호출은 Host 범위). 쓰기가 실패하면
//!   내지 않는다. 다른 프로세스(앱 없이 뜬 `pitwall mcp` · `pitwall tool`)가 쓴 행은 이 등록부를 지나지 않아 이벤트가 없다 —
//!   화면이 열릴 때 · 창이 앞으로 올 때 다시 읽는다.
//! - **이벤트 payload 는 수 · id 만 싣는다.** 이벤트는 role 을 가리지 않고 구독자에게 가는데(`pw_rpc::events` 머리 주석)
//!   세션 목록 · 감사 행은 owner 만 읽는다(`mcp.sessions` · `mcp.audit.list`). 화면은 이벤트를 받으면 RPC 로 다시 읽는다.
//! - 등록부는 승인 허브([`crate::approval::Approvals`] — `mcp.approvals` 를 듣는 창들)도 함께 든다. 앱의 `mcp.*` RPC 와 소켓
//!   세션이 같은 등록부를 쓰므로 창과 세션이 같은 허브를 본다(16-m5-plan M5-12).
//! - 앱이 끝날 때 [`Sessions::shutdown`] 이 대기 중인 승인을 거두고 살아 있는 세션의 감사 큐를 비운다(시한까지).
//! - 등록부는 프로세스 하나의 것이다. 앱은 소켓 세션들과 `mcp.*` RPC 가 같은 [`Sessions`] 를 쓰고, 헤드리스는 자기 것을 둔다
//!   (보는 쪽이 없다).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pw_core::{EventBus, Scope};
use serde::{Deserialize, Serialize};

use crate::approval::Approvals;

/// 세션이 열리거나 닫혔다. payload [`McpSessionsChanged`], Host 범위.
pub const TOPIC_SESSIONS_CHANGED: &str = "mcp.sessionsChanged";
pw_rpc::rpc_event!(TOPIC_SESSIONS_CHANGED, roles: [Owner, Mobile, Readonly]);
/// 감사 로그에 새 행이 쓰였다. payload [`McpActivity`], 그 프로젝트 범위(프로젝트 없는 호출은 Host 범위).
pub const TOPIC_ACTIVITY: &str = "mcp.activity";
pw_rpc::rpc_event!(TOPIC_ACTIVITY, roles: [Owner, Mobile, Readonly]);

/// 열린 세션 하나(`mcp.sessions` 의 원소). M5-11 이후 행이 칸을 더할 자리다 — 칸은 기본값을 가진 뒤에 더한다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpSessionInfo {
    /// 감사 행의 `session` 과 같은 id.
    pub id: String,
    /// MCP 클라이언트(`이름 버전`). 2025-11-25 까지의 클라이언트는 initialize 로, 그 뒤 버전은 첫 툴 호출로 안다. 모르면 `null`.
    pub client: Option<String>,
    /// 세션 프로젝트(`pitwall mcp --project` · cwd 로 푼 것). 없으면 `null`.
    pub project_id: Option<String>,
    /// 연결 종류(`socket` · `stdio`).
    pub transport: String,
    /// 시작 시각(Unix 밀리초).
    #[specta(type = pw_rpc::JsNumber)]
    pub started_at: i64,
    /// 이 세션이 받은 `tools/call` 수 — 감사 행 하나가 호출 하나다(허용 · 실패 · 거절 모두).
    #[specta(type = pw_rpc::JsNumber)]
    pub calls: u64,
}

/// [`TOPIC_SESSIONS_CHANGED`] 의 payload.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpSessionsChanged {
    /// 지금 열린 세션 수.
    pub total: u32,
}

/// [`TOPIC_ACTIVITY`] 의 payload.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpActivity {
    /// 쓰인 감사 행의 id(`mcp.audit.list` 의 `id`).
    #[specta(type = pw_rpc::JsNumber)]
    pub id: i64,
    pub project_id: Option<String>,
}

// 이벤트 payload 는 메서드가 참조하지 않으므로 TS 에 따로 등록한다(CONTRIBUTING.md 「RPC 타입 생성」).
pw_rpc::rpc_type!(McpSessionsChanged, McpActivity);

/// 등록 전 호출을 세어 두는 세션 수의 상한([`Sessions::called`]).
const MAX_EARLY: usize = 64;

/// 열린 세션의 등록부. 복제해도 같은 표다.
#[derive(Clone, Default)]
pub struct Sessions(Arc<Shared>);

#[derive(Default)]
struct Shared {
    live: Mutex<BTreeMap<String, McpSessionInfo>>,
    /// 등록 전에 받은 호출(수 · 클라이언트). 세션은 초기화를 마친 뒤에 등록하는데, 그 사이 rmcp 의 서비스 루프가 먼저 받은
    /// `tools/call` 이 끝날 수 있다(16-m5-plan M5-10 → M5-11 R7). 등록하면 옮기고, 등록 없이 끝나면 버린다.
    early: Mutex<BTreeMap<String, (u64, Option<String>)>>,
    /// 이벤트를 낼 버스. 붙기 전에는 이벤트를 내지 않는다(헤드리스 · 시험).
    bus: OnceLock<EventBus>,
    /// 승인 허브(창의 `mcp.approvals` 와 세션의 승인 요청).
    approvals: Approvals,
    /// 세션 id → 그 세션의 감사 큐를 `deadline` 까지 비우는 함수([`Sessions::shutdown`]).
    flushers: Mutex<BTreeMap<String, Flusher>>,
    /// Streamable HTTP(18-m7-plan M7-25) — 켜고 끄는 것 · 토큰. 앱 · Relay 가 붙인다(`mcp.http.*` · `mcp.settings.get` 이 읽는다).
    http: OnceLock<crate::http::Http>,
}

/// 감사 큐를 비우는 함수. 시한 안에 비웠으면 `true`.
pub type Flusher = Arc<dyn Fn(Instant) -> bool + Send + Sync>;

impl Sessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// 이벤트를 낼 버스를 붙인다(앱의 Host 버스). 두 번째부터는 무시한다.
    pub fn attach(&self, bus: EventBus) {
        let _ = self.0.bus.set(bus);
    }

    /// Streamable HTTP 를 붙인다(앱 · Relay 가 한 번). 두 번째부터는 무시한다.
    pub fn attach_http(&self, http: crate::http::Http) {
        let _ = self.0.http.set(http);
    }

    /// 붙은 Streamable HTTP. 헤드리스 · 시험에는 없다.
    pub fn http(&self) -> Option<&crate::http::Http> {
        self.0.http.get()
    }

    /// 승인 허브.
    pub fn approvals(&self) -> &Approvals {
        &self.0.approvals
    }

    /// 세션의 감사 큐를 비우는 함수를 둔다. 돌려준 가드가 버려지면 뺀다(세션이 끝남).
    pub fn add_flusher(&self, id: &str, flush: Flusher) -> FlusherGuard {
        self.flushers().insert(id.to_owned(), flush);
        FlusherGuard {
            sessions: self.clone(),
            id: id.to_owned(),
        }
    }

    /// 앱이 끝난다(블로킹): 대기 중인 승인을 모두 거두고, 승인을 지나던 호출이 감사 기록을 큐에 넣기를 기다린 뒤, 살아 있는 세션의
    /// 감사 큐를 비운다 — 모두 `timeout` 안에서. 그 시간만큼 앱 종료가 늦어질 수 있다. 시한 안에 다 했으면 `true`.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut done = self.0.approvals.shutdown_and_wait(deadline);
        let flushers: Vec<Flusher> = self.flushers().values().cloned().collect();
        for flush in flushers {
            done &= flush(deadline);
        }
        done
    }

    fn flushers(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Flusher>> {
        self.0
            .flushers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// 열린 세션(시작 순서).
    pub fn list(&self) -> Vec<McpSessionInfo> {
        let mut out: Vec<McpSessionInfo> = self.live().values().cloned().collect();
        out.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
        out
    }

    /// 열린 세션 수.
    pub fn total(&self) -> usize {
        self.live().len()
    }

    /// 세션을 등록한다. 돌려준 가드가 버려지면 빠진다. 같은 id 를 다시 등록하면 앞의 것을 바꾼다.
    pub fn register(
        &self,
        id: &str,
        client: Option<String>,
        project_id: Option<String>,
        transport: &str,
    ) -> Registered {
        let total = {
            // 앞서 받은 호출을 옮기는 것과 올리는 것을 `live` 잠금 안에서 한다 — 그 사이의 `called` 가 빠지지 않는다.
            let mut live = self.live();
            let (calls, early_client) = self.early().remove(id).unwrap_or_default();
            let info = McpSessionInfo {
                id: id.to_owned(),
                client: client.or(early_client),
                project_id,
                transport: transport.to_owned(),
                started_at: now_millis(),
                calls,
            };
            live.insert(id.to_owned(), info);
            live.len()
        };
        self.changed(total);
        Registered {
            sessions: self.clone(),
            id: id.to_owned(),
        }
    }

    /// 세션 `id` 가 툴 호출 하나를 받았다. 클라이언트를 알면 그것으로 고친다. 이벤트는 내지 않는다. 아직 등록 전이면 세어 두었다가
    /// 등록할 때 옮긴다.
    pub fn called(&self, id: &str, client: Option<&str>) {
        // 잠금 순서는 `live` → `early`([`Self::register`] 와 같다). `live` 를 쥔 채 `early` 에 세어야 그 사이의 등록이 이 수를
        // 놓치지 않는다.
        let mut live = self.live();
        if let Some(info) = live.get_mut(id) {
            info.calls += 1;
            if let Some(client) = client {
                client.clone_into(info.client.get_or_insert_with(String::new));
            }
            return;
        }
        let mut early = self.early();
        // 끝난 세션의 늦은 호출(끊긴 뒤 5초까지 도는 핸들러 — 05 「호출 취소」)도 여기로 와 등록 없이 남는다. 쌓이지 않게 상한을
        // 둔다 — 등록을 기다리는 세션이 이만큼 동시에 있을 일은 없다.
        if early.len() >= MAX_EARLY && !early.contains_key(id) {
            early.clear();
        }
        let (calls, known) = early.entry(id.to_owned()).or_default();
        *calls += 1;
        if let Some(client) = client {
            *known = Some(client.to_owned());
        }
    }

    /// 감사 행 `row` 가 쓰였다 — [`TOPIC_ACTIVITY`] 를 낸다.
    pub fn recorded(&self, row: i64, project_id: Option<&str>) {
        let Some(bus) = self.0.bus.get() else {
            return;
        };
        let scope = project_id.map_or(Scope::Host, |p| Scope::Project(p.to_owned()));
        let payload = McpActivity {
            id: row,
            project_id: project_id.map(str::to_owned),
        };
        if let Err(e) = bus.publish(scope, TOPIC_ACTIVITY, &payload) {
            tracing::warn!(%e, "cannot publish mcp.activity");
        }
    }

    fn unregister(&self, id: &str) {
        let total = {
            let mut live = self.live();
            self.early().remove(id);
            if live.remove(id).is_none() {
                return;
            }
            live.len()
        };
        self.changed(total);
    }

    fn changed(&self, total: usize) {
        let Some(bus) = self.0.bus.get() else {
            return;
        };
        let payload = McpSessionsChanged {
            total: u32::try_from(total).unwrap_or(u32::MAX),
        };
        if let Err(e) = bus.publish(Scope::Host, TOPIC_SESSIONS_CHANGED, &payload) {
            tracing::warn!(%e, "cannot publish mcp.sessionsChanged");
        }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, McpSessionInfo>> {
        self.0.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn early(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, (u64, Option<String>)>> {
        self.0.early.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// [`Sessions::add_flusher`] 의 가드.
pub struct FlusherGuard {
    sessions: Sessions,
    id: String,
}

impl Drop for FlusherGuard {
    fn drop(&mut self) {
        self.sessions.flushers().remove(&self.id);
    }
}

/// [`Sessions::register`] 의 가드. 버려지면 그 세션이 빠지고 [`TOPIC_SESSIONS_CHANGED`] 가 나간다.
pub struct Registered {
    sessions: Sessions,
    id: String,
}

impl Drop for Registered {
    fn drop(&mut self) {
        self.sessions.unregister(&self.id);
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use pw_core::Filter;

    use super::*;

    #[test]
    fn registering_and_dropping_publishes_once_each() {
        let bus = EventBus::new(16);
        let mut events = bus.subscribe(
            Filter::all()
                .scope(Scope::Host)
                .topics([TOPIC_SESSIONS_CHANGED]),
        );
        let sessions = Sessions::new();
        sessions.attach(bus);
        let a = sessions.register("a", None, Some("p1".into()), "socket");
        let b = sessions.register("b", Some("claude 1".into()), None, "socket");
        sessions.called("a", Some("codex 2"));
        sessions.called("a", None);
        let list = sessions.list();
        assert_eq!(list.len(), 2);
        let first = list.iter().find(|s| s.id == "a").unwrap();
        assert_eq!(first.calls, 2);
        assert_eq!(first.client.as_deref(), Some("codex 2"));
        drop(a);
        drop(b);
        let totals: Vec<u32> = std::iter::from_fn(|| events.try_recv())
            .map(|r| match r {
                pw_core::Received::Event(e) => {
                    serde_json::from_value::<McpSessionsChanged>(e.payload.clone())
                        .unwrap()
                        .total
                }
                pw_core::Received::Lagged(n) => panic!("lagged {n}"),
            })
            .collect();
        assert_eq!(totals, [1, 2, 1, 0]);
        assert_eq!(sessions.total(), 0);
    }

    /// 등록 전에 끝난 호출도 센다(R7) — 세션은 초기화를 마친 뒤 등록하는데 그 사이 서비스 루프가 받은 호출이 먼저 끝날 수 있다.
    #[test]
    fn calls_before_registration_are_counted_when_the_session_registers() {
        let sessions = Sessions::new();
        sessions.called("a", Some("codex 2"));
        sessions.called("a", None);
        sessions.called("other", None);
        let a = sessions.register("a", None, None, "socket");
        let list = sessions.list();
        assert_eq!(list.len(), 1, "등록하지 않은 세션은 목록에 없다");
        assert_eq!(list[0].calls, 2);
        assert_eq!(list[0].client.as_deref(), Some("codex 2"));
        sessions.called("a", None);
        assert_eq!(sessions.list()[0].calls, 3);
        // initialize 로 안 이름이 있으면 그것이다.
        drop(a);
        sessions.called("b", Some("early 1"));
        let _b = sessions.register("b", Some("rmcp 1".into()), None, "socket");
        assert_eq!(sessions.list()[0].client.as_deref(), Some("rmcp 1"));
        assert_eq!(sessions.list()[0].calls, 1);
    }
}
