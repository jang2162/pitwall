// A line comment.
use std::fmt;

/// Doc comment on a struct.
#[derive(Debug)]
pub struct Point<T> {
    x: T,
    y: i32,
}

const LIMIT: u32 = 0x1F;

fn main() {
    let label = "point\n";
    let p = Point { x: 1.5, y: -2 };
    if p.y < 0 && true {
        println!("{label}{:?}", p);
    }
}
