//! live 시험 픽스처의 크레이트 뿌리. 표지 주석(`/*@이름*/`)과 그 뜻은 ../../README.md 「앵커 표」.
//! 표준 라이브러리 소스(rust-src) 없이도 시험이 되도록 앵커는 이 크레이트의 타입 · 함수와 원시 타입에만 건다.

pub mod calls;
pub mod errors;
pub mod messy;
pub mod shapes;
pub mod unused;
