//! 역요청 `runInTerminal` 이 디버그 대상을 띄우는 곳(17-m6-plan.md M6-15 ③).
//!
//! 띄우는 것은 실행 관리자(M6-02 — `pw-run` 의 실행 · PTY 콘솔)의 몫이다. 이 크레이트는 판정([`crate::DebugGrant`])을 지난
//! [`TerminalRun`] 과 그 허가([`ExecGrant`])를 [`Terminals`] 에 넘기기만 한다 — Host 에서 잇는 것은 디버그 세션 행(M6-17)이다.
//! 디버그 대상의 출력은 그 실행의 콘솔에 나온다(메인 판정 M6-P15 — 디버그 세션은 실행 하나).

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use pw_core::registry::exec::ExecGrant;

/// 판정을 지난 `runInTerminal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRun {
    pub title: Option<String>,
    /// 루트 안의 실제 경로.
    pub cwd: PathBuf,
    /// 첫째가 명령. 셸 없이 띄운다.
    pub args: Vec<String>,
    /// 런타임 환경 위에 더할(글) · 지울(`None`) 변수.
    pub env: BTreeMap<String, Option<String>>,
}

/// 띄운 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalStarted {
    /// 디버그 대상의 pid(어댑터가 붙을 때 쓴다).
    pub process_id: Option<u32>,
    /// 셸을 거쳤으면 그 셸의 pid.
    pub shell_process_id: Option<u32>,
}

pub type TerminalFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TerminalStarted, String>> + Send + 'a>>;

/// 실행 관리자. 실패의 글은 어댑터에 그대로 돌려준다(영어).
pub trait Terminals: Send + Sync {
    fn run_in_terminal<'a>(&'a self, grant: &'a ExecGrant, run: TerminalRun) -> TerminalFuture<'a>;
}

/// `runInTerminal` 을 모두 거절하는 것(실행 관리자가 없는 자리 — 시험 · 붙기만 하는 세션).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoTerminals;

impl Terminals for NoTerminals {
    fn run_in_terminal<'a>(
        &'a self,
        _grant: &'a ExecGrant,
        _run: TerminalRun,
    ) -> TerminalFuture<'a> {
        Box::pin(async { Err("this debug session cannot run programs in a terminal".to_owned()) })
    }
}
