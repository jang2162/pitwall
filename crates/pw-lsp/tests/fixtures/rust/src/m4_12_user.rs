// M4-12 의 rename live 시험 — `relabel` 의 import 하나 · 호출 둘.
use crate::m4_12_target::relabel;

pub fn twice(x: u32) -> u32 {
    relabel(relabel(x))
}
