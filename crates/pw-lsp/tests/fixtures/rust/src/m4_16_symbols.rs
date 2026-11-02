//! M4-16 작업 공간 심볼 live 픽스처. 표지 주석(`/*@이름*/`)과 그 뜻은 ../../README.md 「앵커 표」. 크레이트에 들도록 시험이 임시
//! 복사본의 `lib.rs` 끝에 `pub mod m4_16_symbols;` 를 더한다. 이름은 픽스처의 다른 파일과 겹치지 않게 `M416` 으로 시작한다.

/// 구조체 — 심볼로 이동 · 클래스로 이동에 다 든다.
pub struct /*@sym.gadget*/M416Gadget {
    pub size: u32,
}

/// 트레이트 — 클래스로 이동에 든다(rust-analyzer 는 `Interface`).
pub trait /*@sym.spin*/M416Spin {
    fn spin(&self) -> u32;
}

/// 함수 — 심볼로 이동에만 든다(클래스로 이동은 뺀다).
pub fn /*@sym.build*/m416_build_gadget(size: u32) -> M416Gadget {
    M416Gadget { size }
}
