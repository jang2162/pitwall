//! 정의 · 구현 · rename 대상. 표지는 그 이름의 첫 글자 바로 앞에 있다.

/// 넓이를 가진 도형.
pub trait /*@def.shape*/Shape {
    /// 넓이.
    fn /*@def.shape-area*/area(&self) -> u32;
}

/// 사각형.
pub struct /*@def.rect*/Rect {
    pub width: u32,
    pub height: u32,
}

impl Shape for /*@impl.rect-shape*/Rect {
    fn area(&self) -> u32 {
        self.width * self.height
    }
}

/// 두 변으로 넓이를 낸다. rename 대상(이 정의와 calls.rs 의 호출 둘).
pub fn /*@rename.scale*/scale(width: u32, height: u32) -> u32 {
    width * height
}
