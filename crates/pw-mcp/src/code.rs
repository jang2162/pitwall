//! `pw-mcp` 의 애플리케이션 오류 코드 — **6001~**(`docs/design/16-m5-plan.md` 「공유 부품의 주인」 의 오류 코드 줄. 3xxx 는
//! `pw-db`, 2xxx 는 git, 4001~ 은 `pw-tools`, 5001~ 은 `pw-lsp`). MCP 쪽 RPC(`mcp.*`)와 MCP 계층이 내는 RPC 오류가 쓴다.
//!
//! 코드를 더하는 행은 상수와 그 `data` 꼴을 여기에 적고 [`ALL`] 에 더한다. 툴 메서드가 기능 크레이트의 오류를 그대로 내는 것은
//! 그 크레이트의 범위다 — 여기에 다시 만들지 않는다.

use std::ops::RangeInclusive;

/// 이 크레이트가 쓰는 범위.
pub const RANGE: RangeInclusive<i32> = 6001..=6999;

/// 위 코드 전부(상수 이름, 값). 아직 없다 — 첫 코드를 더하는 행이 채운다.
pub const ALL: [(&str, i32); 0] = [];

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
