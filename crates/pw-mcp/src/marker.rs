//! 결과 표지(`docs/design/05-mcp.md` 「툴 레지스트리」 의 「결과 표지」): `tools/call` 의 결과가 성공인지, 툴이 불렸다 실패했는지,
//! 부르지 않았는지(거절)와 그 까닭을 기계가 읽는 꼴로 싣는다. 글(`content`)만으로는 거절과 툴 오류가 둘 다 `isError` 라
//! CLI(`pitwall tool`)가 종료 코드 1 과 3 을 가를 수 없다(16-m5-plan 결정 M5-D12 ⓑ).
//!
//! - `tools/call` 결과: `_meta` 에 [`OUTCOME`] 와(까닭이 있으면) [`REASON`].
//! - 노출 안 된 툴 호출(JSON-RPC invalid params): 오류의 `data` 객체에 같은 두 키.
//!
//! 결과 값은 감사 로그의 `outcome` 가운데 셋이다(`ok` · `error` · `denied` — 넷째 `started` 는 실행 전에 쓴 감사 행에만 있다). 까닭 코드는 [`Reason`] 이고, 거절의 까닭이
//! 대부분이지만 [`Reason::Cancelled`] · [`Reason::TooLarge`] 는 결과 `error` 의 까닭이다. 다른 행이 코드를 더하면 05 의 표에
//! 한 줄을 더한다.
//!
//! **툴의 Host RPC 가 `PERMISSION_DENIED` 로 답하면 결과는 `denied` 다**(메인 판정 2026-11-14 — 16-m5-plan M5-06): 까닭은 그
//! `data.reason` 이 [`Reason::of_permission_denied`] 의 표에 있으면 그 코드, 아니면 `permission_denied`. 다른 오류(비밀 · 확인이
//! 필요하다는 DB 의 3101 · 3102 등)는 결과 `error` 다.

use serde_json::{Map, Value};

use crate::audit::Outcome;

/// 결과 값(`ok` · `error` · `denied`)의 `_meta` 키.
pub const OUTCOME: &str = "pitwall/outcome";
/// 까닭 코드의 `_meta` 키. 까닭이 없으면 키가 없다.
pub const REASON: &str = "pitwall/reason";

/// 까닭 코드. 표기는 snake_case 글자열이다([`Reason::as_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// (거절) 세션 또는 대상 프로젝트의 노출 설정이 그 툴을 껐다. 모르는 툴 이름도 이것이다.
    NotExposed,
    /// (거절) 사람의 승인이 있어야 하는데 지금 받을 길이 없다 — 승인 경로가 없는 세션, 묻을 창이 없음(결정 M5-D29), 묻는 도중
    /// 창이 닫힘.
    NeedsApproval,
    /// (거절) 사람이 승인 창에서 거절했다(16-m5-plan M5-12).
    ApprovalDenied,
    /// (거절) 승인 창이 시간(`[mcp.policy] approval_timeout_secs`, 기본 300초) 안에 답을 받지 못했다(M5-12).
    ApprovalTimeout,
    /// (거절) 승인 뒤 툴이 다시 만든 계획의 해시가 승인한 것과 다르다(파일 내용 · 서버가 채운 `worktree` · 대상이 바뀜) — 돌리지
    /// 않았고 다시 묻지 않았다(결정 M5-D30). 다시 부르면 새 계획으로 묻는다.
    PlanChanged,
    /// (거절) 앱이 떠 있어야 하는 툴을 헤드리스 세션에서 불렀다(결정 M5-D3).
    NeedsApp,
    /// (거절) 헤드리스 세션에서 `write` 이상 등급의 툴을 불렀다 — 헤드리스에는 승인 경로가 없다(결정 M5-D32).
    HeadlessWrite,
    /// (결과 `error`) 호출이 취소됐다 — MCP 의 취소 알림, 세션 끊김, 또는 Host 가 그 RPC 를 거뒀다.
    Cancelled,
    /// (결과 `error`) 응답이 [`crate::server::MAX_RESPONSE_BYTES`] 를 넘었다. 범위를 좁혀 다시 부른다.
    TooLarge,
    /// (거절) 툴의 Host RPC 가 `PERMISSION_DENIED` 로 답했고 그 `data.reason` 이 아래 코드 가운데 어느 것도 아니다(없음 포함).
    PermissionDenied,
    /// (거절) 사람이 이번 앱 실행에서 Pitwall 의 DB 탐색기로 그 연결(같은 대상)을 연 적이 없다(16-m5-plan M5-06 —
    /// `data.reason = "notOpenedByUser"`).
    NotOpenedByUser,
    /// (거절) 프로젝트를 신뢰하지 않았다(`data.reason = "untrusted"`).
    Untrusted,
    /// (거절) 프로덕션 표지가 있는 DB 대상이다(M5-06 — `data.reason = "productionBlocked"`).
    ProductionBlocked,
    /// (거절) Host 전체의 호출 빈도를 넘었다(16-m5-plan M5-09 — `project_open`, `data.reason = "rateLimited"`). 잠시 뒤 다시 부른다.
    RateLimited,
    /// (거절) 선기록하는 호출(`write` 이상 · 승인을 지난 호출)인데 실행 전의 감사 행을 쓰지 못했다 — 돌리지 않았다(16-m5-plan
    /// 결정 M5-D11).
    AuditUnavailable,
}

impl Reason {
    pub const ALL: &'static [Reason] = &[
        Reason::NotExposed,
        Reason::NeedsApproval,
        Reason::ApprovalDenied,
        Reason::ApprovalTimeout,
        Reason::PlanChanged,
        Reason::NeedsApp,
        Reason::HeadlessWrite,
        Reason::Cancelled,
        Reason::TooLarge,
        Reason::PermissionDenied,
        Reason::NotOpenedByUser,
        Reason::Untrusted,
        Reason::ProductionBlocked,
        Reason::RateLimited,
        Reason::AuditUnavailable,
    ];

    /// Host RPC 의 `PERMISSION_DENIED` 오류가 싣는 `data.reason`(camelCase) ↔ 까닭 코드. 거절 까닭을 더하는 행은 [`Reason`] 과 이
    /// 표에 한 줄씩 더한다 — 여기 없는 `data.reason` 은 [`Reason::PermissionDenied`] 다.
    const FROM_RPC: &'static [(&'static str, Reason)] = &[
        ("notOpenedByUser", Reason::NotOpenedByUser),
        ("untrusted", Reason::Untrusted),
        ("productionBlocked", Reason::ProductionBlocked),
        ("rateLimited", Reason::RateLimited),
    ];

    /// `PERMISSION_DENIED` 오류의 `data.reason` 을 까닭 코드로. 모르거나 없으면 [`Reason::PermissionDenied`].
    pub fn of_permission_denied(rpc_reason: Option<&str>) -> Self {
        Self::FROM_RPC
            .iter()
            .find(|(r, _)| Some(*r) == rpc_reason)
            .map_or(Reason::PermissionDenied, |(_, reason)| *reason)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::NotExposed => "not_exposed",
            Reason::NeedsApproval => "needs_approval",
            Reason::ApprovalDenied => "approval_denied",
            Reason::ApprovalTimeout => "approval_timeout",
            Reason::PlanChanged => "plan_changed",
            Reason::NeedsApp => "needs_app",
            Reason::HeadlessWrite => "headless_write",
            Reason::Cancelled => "cancelled",
            Reason::TooLarge => "too_large",
            Reason::PermissionDenied => "permission_denied",
            Reason::NotOpenedByUser => "not_opened_by_user",
            Reason::Untrusted => "untrusted",
            Reason::ProductionBlocked => "production_blocked",
            Reason::RateLimited => "rate_limited",
            Reason::AuditUnavailable => "audit_unavailable",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|r| r.as_str() == s)
    }
}

/// 결과 값의 표기. 감사 로그의 `outcome` 열과 같다. 표지는 `started` 를 싣지 않는다 — 읽을 때도 셋만 받는다.
pub fn outcome_str(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Ok => "ok",
        Outcome::Error => "error",
        Outcome::Denied => "denied",
        Outcome::Started => "started",
    }
}

fn parse_outcome(s: &str) -> Option<Outcome> {
    [Outcome::Ok, Outcome::Error, Outcome::Denied]
        .into_iter()
        .find(|o| outcome_str(*o) == s)
}

/// 호출 하나의 표지.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub outcome: Outcome,
    pub reason: Option<Reason>,
}

impl Mark {
    pub fn ok() -> Self {
        Self {
            outcome: Outcome::Ok,
            reason: None,
        }
    }

    pub fn error(reason: Option<Reason>) -> Self {
        Self {
            outcome: Outcome::Error,
            reason,
        }
    }

    pub fn denied(reason: Reason) -> Self {
        Self {
            outcome: Outcome::Denied,
            reason: Some(reason),
        }
    }

    /// `_meta`(또는 오류의 `data`)에 넣을 두 키.
    pub fn to_map(self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert(OUTCOME.into(), Value::from(outcome_str(self.outcome)));
        if let Some(reason) = self.reason {
            map.insert(REASON.into(), Value::from(reason.as_str()));
        }
        map
    }

    /// `_meta` 나 오류 `data` 에서 읽는다. 결과 키가 없거나 모르는 값이면 `None`. 모르는 까닭 코드는 까닭 없음으로 읽는다 —
    /// 서버가 새 코드를 더해도 결과 값으로 가르는 쪽(CLI 의 종료 코드)은 그대로 돈다.
    pub fn read(map: &Map<String, Value>) -> Option<Self> {
        let outcome = parse_outcome(map.get(OUTCOME)?.as_str()?)?;
        let reason = map
            .get(REASON)
            .and_then(Value::as_str)
            .and_then(Reason::parse);
        Some(Self { outcome, reason })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_round_trip_through_the_meta_keys() {
        for mark in [
            Mark::ok(),
            Mark::error(None),
            Mark::error(Some(Reason::Cancelled)),
            Mark::denied(Reason::NotExposed),
        ] {
            assert_eq!(Mark::read(&mark.to_map()), Some(mark));
        }
        let map = Mark::denied(Reason::NeedsApproval).to_map();
        assert_eq!(map[OUTCOME], "denied");
        assert_eq!(map[REASON], "needs_approval");
        assert!(!Mark::ok().to_map().contains_key(REASON));
    }

    #[test]
    fn reason_codes_are_unique_snake_case() {
        let mut seen = std::collections::BTreeSet::new();
        for reason in Reason::ALL {
            let code = reason.as_str();
            assert!(
                code.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{code}"
            );
            assert!(seen.insert(code), "{code} twice");
            assert_eq!(Reason::parse(code), Some(*reason));
        }
    }

    #[test]
    fn permission_denied_reasons_map_to_reason_codes() {
        assert_eq!(
            Reason::of_permission_denied(Some("notOpenedByUser")),
            Reason::NotOpenedByUser
        );
        assert_eq!(
            Reason::of_permission_denied(Some("untrusted")),
            Reason::Untrusted
        );
        assert_eq!(
            Reason::of_permission_denied(Some("productionBlocked")),
            Reason::ProductionBlocked
        );
        assert_eq!(
            Reason::of_permission_denied(Some("rateLimited")),
            Reason::RateLimited
        );
        assert_eq!(
            Reason::of_permission_denied(Some("notProjectWindow")),
            Reason::PermissionDenied
        );
        assert_eq!(Reason::of_permission_denied(None), Reason::PermissionDenied);
        for (rpc, reason) in Reason::FROM_RPC {
            assert!(Reason::ALL.contains(reason), "{rpc}");
        }
    }

    #[test]
    fn an_unknown_reason_still_reads_the_outcome() {
        let mut map = Mark::denied(Reason::NotExposed).to_map();
        map.insert(REASON.into(), Value::from("from_the_future"));
        assert_eq!(
            Mark::read(&map),
            Some(Mark {
                outcome: Outcome::Denied,
                reason: None
            })
        );
        assert_eq!(Mark::read(&Map::new()), None);
    }
}
