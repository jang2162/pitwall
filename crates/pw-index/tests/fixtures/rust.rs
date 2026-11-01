//! 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

pub const LIMIT: usize = 8;
static GREETING: &str = "안녕";

pub struct Point {
    pub x: f64,
    pub y: f64,
}

pub enum Shape {
    Circle(f64),
    Square { side: f64 },
}

pub type Pair = (Point, Point);

pub trait Area {
    type Unit;
    fn area(&self) -> f64;
}

impl Area for Shape {
    type Unit = f64;
    fn area(&self) -> f64 {
        match self {
            Shape::Circle(r) => r * r * 3.14,
            Shape::Square { side } => side * side,
        }
    }
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

macro_rules! square {
    ($x:expr) => {
        $x * $x
    };
}

pub mod nested {
    pub fn helper() -> usize {
        super::LIMIT
    }
}

fn main() {
    let p = Point::new(1.0, 2.0);
    println!("{} {}", square!(p.x), GREETING);
}
