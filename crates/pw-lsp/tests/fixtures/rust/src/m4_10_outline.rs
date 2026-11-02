//! M4-10 아웃라인: 구조 툴 윈도우의 live 시험(`live_lsp_rust_analyzer_document_symbols`)이 이 파일의 문서 심볼에서 아래 이름을
//! 찾는다 — 구조체 · 필드 · 열거 · 변형 · impl 안 메서드 · 함수 · 모듈 안 함수.

pub struct OutlineBox {
    pub outline_width: u32,
}

pub enum OutlineColor {
    OutlineRed,
    OutlineBlue,
}

impl OutlineBox {
    pub fn outline_area(&self) -> u32 {
        self.outline_width * 2
    }
}

pub fn outline_entry() -> u32 {
    OutlineBox { outline_width: 3 }.outline_area()
}

pub mod outline_inner {
    pub fn outline_nested() -> u32 {
        1
    }
}
