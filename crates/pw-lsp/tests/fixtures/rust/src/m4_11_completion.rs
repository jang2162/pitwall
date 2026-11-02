//! 자동완성 · 시그니처 도움말(15-m4-plan.md M4-11 live). 크레이트의 `lib.rs` 에 없으므로 시험이 임시 복사본의 `lib.rs` 에
//! `pub mod m4_11_completion;` 을 더한다(../../README.md 「규칙」).

use crate::shapes::{Rect, scale};

/// 계량기.
pub struct Meter {
    pub reading: u32,
    pub limit: u32,
}

impl Meter {
    /// 남은 양. `self.` 바로 뒤의 완성 후보에 `reading` · `limit` 이 든다.
    pub fn headroom(&self) -> u32 {
        self./*@m4-11.self-members*/limit - self.reading
    }
}

/// 시그니처 도움말 자리: 둘째 인자(`height`) 앞.
pub fn sized(r: &Rect) -> u32 {
    scale(r.width, /*@m4-11.signature-second*/r.height)
}
