//! M4-08 내비게이션 live 픽스처. 표지 주석(`/*@이름*/`)과 그 뜻은 ../../README.md 「앵커 표」. 크레이트에 들도록 시험이 임시
//! 복사본의 `lib.rs` 끝에 `pub mod m4_08_nav;` 를 더한다.

/// 정의로 이동 · 사용처의 대상.
pub fn /*@nav.def*/greet(name: &str) -> usize {
    name.len()
}

/// 호출 둘 — 사용처(선언 포함)는 이 파일 안에 셋.
pub fn twice() -> usize {
    /*@nav.call-1*/greet("a") + /*@nav.call-2*/greet("b")
}

/// 표준 라이브러리 항목의 정의 — rust-src 가 있어야 풀린다(루트 밖 읽기 전용 문서).
pub fn numbers() -> Vec<u32> {
    /*@nav.vec*/Vec::new()
}
