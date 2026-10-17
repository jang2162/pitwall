//! 권한 등급별 정책(`docs/design/05-mcp.md` §3). 노출([`crate::exposure`])과 별개다 — 보이는 툴도 여기서 거절될 수 있다.
//!
//! | 등급 | 설계의 기본 정책 | 지금 |
//! |---|---|---|
//! | `read` | 자동 허용 | 허용 |
//! | `write` · `exec` | UI 승인 | 승인 경로가 없어 **거절** |
//! | `dangerous` | 매번 승인, 자동 승인 불가 | 승인 경로가 없어 **거절** |
//!
//! 승인 UI · OS 알림(§3 「승인 요청 라우팅」), `auto_approve` · `[mcp.policy]` 는 아직 읽지 않는다. 그것들이 들어오면
//! [`Approval`] 에 경로가 생기고, 이 함수가 `Ask` 를 돌려줄 자리에서 승인을 기다린다.

use pw_rpc::Tier;

/// 승인을 받을 길이 있는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// 승인 UI 가 없다(지금은 언제나. 앱이 떠 있어도 승인 화면이 아직 없다).
    Unavailable,
    /// 승인을 물을 수 있다.
    Available,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// 사람에게 물어야 한다.
    Ask,
    /// 거절. 이유는 에이전트에게 그대로 보이고 감사 로그에 남는다.
    Deny(String),
}

pub fn decide(tier: Tier, approval: Approval) -> Decision {
    match (tier, approval) {
        (Tier::Read, _) => Decision::Allow,
        (Tier::Write | Tier::Exec | Tier::Dangerous, Approval::Available) => Decision::Ask,
        (Tier::Write | Tier::Exec | Tier::Dangerous, Approval::Unavailable) => {
            Decision::Deny(format!(
                "`{}` tools need approval in Pitwall, and there is no approval route yet",
                tier_name(tier)
            ))
        }
    }
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Read => "read",
        Tier::Write => "write",
        Tier::Exec => "exec",
        Tier::Dangerous => "dangerous",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_is_allowed_and_everything_else_is_denied_without_an_approval_route() {
        assert_eq!(decide(Tier::Read, Approval::Unavailable), Decision::Allow);
        for tier in [Tier::Write, Tier::Exec, Tier::Dangerous] {
            match decide(tier, Approval::Unavailable) {
                Decision::Deny(reason) => assert!(reason.contains(tier_name(tier)), "{reason}"),
                other => panic!("{tier:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn with_an_approval_route_non_read_tiers_ask() {
        assert_eq!(decide(Tier::Read, Approval::Available), Decision::Allow);
        for tier in [Tier::Write, Tier::Exec, Tier::Dangerous] {
            assert_eq!(decide(tier, Approval::Available), Decision::Ask, "{tier:?}");
        }
    }
}
