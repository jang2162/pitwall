//! M4-06 진단 live 픽스처: 타입 오류 하나(앵커 `m4_06.type-error` — 그 자리부터 `"three"` 일곱 글자에 오류가 온다).

pub fn total() -> u32 {
    let count: u32 = /*@m4_06.type-error*/"three";
    count
}
