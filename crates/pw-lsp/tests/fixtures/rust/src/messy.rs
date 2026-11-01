//! 흐트러진 서식(포맷). rustfmt 를 거치면 바뀐다 — 저장소의 `cargo fmt` 는 이 픽스처를 보지 않는다(독립 작업 공간).
pub fn /*@format.messy*/messy(  a:u32,b :u32)->u32{
let  c=a+b ;
    if c>10{c-10}else    {c}
}
