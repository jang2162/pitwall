//! 열린 MCP 세션 수 — 로컬 소켓(`crate::socket`)과 Streamable HTTP(`crate::http`)가 함께 센다. Relay 의 유휴 판정이 본다
//! (18-m7-plan M7-20 · M7-25: HTTP 는 인증을 지나 세션이 된 것만 센다).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 열려 있는 세션 수. 시험과 로그가 본다.
#[derive(Clone, Default)]
pub struct Connections(Arc<AtomicUsize>);

impl Connections {
    pub fn active(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

pub(crate) struct Counted(Connections);

impl Counted {
    pub(crate) fn new(c: &Connections) -> Self {
        c.0.fetch_add(1, Ordering::SeqCst);
        Self(c.clone())
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.0.fetch_sub(1, Ordering::SeqCst);
    }
}
