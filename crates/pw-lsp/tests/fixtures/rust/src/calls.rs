//! 호출 자리(사용처 · 정의로 이동 · hover · 시그니처 도움말).

use crate::shapes::{scale, Rect, Shape};

/// 정사각형의 넓이.
pub fn square(side: u32) -> u32 {
    /*@call.scale-1*/scale(side, side)
}

/// 사각형 둘의 넓이 합.
pub fn total(a: &Rect, b: &Rect) -> u32 {
    a./*@call.area-1*/area() + /*@call.scale-2*/scale(b.width, b.height)
}

/// 시그니처 도움말 자리: 여는 괄호 바로 뒤.
pub fn half(side: u32) -> u32 {
    scale(/*@signature.scale*/side, 1) / 2
}

/// 자동완성 자리: 점 바로 뒤(`area` · `width` · `height` 가 후보 — `area` 는 `Shape` 를 들여와서).
pub fn complete(r: &Rect) -> u32 {
    r./*@complete.rect-members*/width
}
