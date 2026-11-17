//! `pw-mcp` 의 애플리케이션 오류 코드 — **6001~**(`docs/design/16-m5-plan.md` 「공유 부품의 주인」 의 오류 코드 줄. 3xxx 는
//! `pw-db`, 2xxx 는 git, 4001~ 은 `pw-tools`, 5001~ 은 `pw-lsp`). MCP 쪽 RPC(`mcp.*`)와 MCP 계층이 내는 RPC 오류가 쓴다.
//!
//! 코드를 더하는 행은 상수와 그 `data` 꼴을 여기에 적고 [`ALL`] 에 더한다. 툴 메서드가 기능 크레이트의 오류를 그대로 내는 것은
//! 그 크레이트의 범위다 — 여기에 다시 만들지 않는다.

use std::ops::RangeInclusive;

/// 이 크레이트가 쓰는 범위.
pub const RANGE: RangeInclusive<i32> = 6001..=6999;

/// 「승인이 있어야 한다」(16-m5-plan M5-12) — 계획을 만드는 툴 메서드가 승인 표 없이(또는 해시가 다른 표로) 불리면 낸다. `data`
/// 는 `pw_core::registry::plan::ApprovalRequired`(계획 · `mismatch`). 툴 크레이트가 이 크레이트에 의존하지 않아 정본은
/// `pw_core::registry::plan` 에 있다. MCP 세션 안에서만 쓰이고 에이전트에게 그대로 나가지 않는다.
pub const APPROVAL_REQUIRED: i32 = pw_core::registry::plan::APPROVAL_REQUIRED;

/// 위 코드 전부(상수 이름, 값).
pub const ALL: [(&str, i32); 1] = [("APPROVAL_REQUIRED", APPROVAL_REQUIRED)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_inside_the_range() {
        let mut seen = std::collections::BTreeSet::new();
        for (name, code) in ALL {
            assert!(RANGE.contains(&code), "{name} = {code}");
            assert!(seen.insert(code), "{name} = {code} 가 겹친다");
        }
        // 다른 크레이트의 범위와 겹치지 않는다.
        assert!(!RANGE.contains(&5001) && !RANGE.contains(&7001));
    }
}
