//! 오류 자리(진단). 이 파일만 일부러 컴파일되지 않는다 — 진단 수는 이 파일 안으로만 단언한다.

/// 타입이 맞지 않는다(E0308).
pub fn mismatch() -> u32 {
    let n: u32 = /*@error.type-mismatch*/"text";
    n
}

/// 없는 이름(E0425).
pub fn unresolved() -> u32 {
    /*@error.unresolved*/missing_function()
}
