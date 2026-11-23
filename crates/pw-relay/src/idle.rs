//! 유휴 끝내기(18-m7-plan M7-04 ④ — 메인 판정 M7-P13: 상주하지 않는 쪽이 가볍다, SSH 로 다시 띄우면 된다). 링크가 없고 실행 ·
//! 터미널(떼어진 것 포함)도 없는 채 `[idle] exit_after` 가 지나면 데몬이 끝난다. 링크가 없는 채 `stop_lsp_after` 가 지나면 언어
//! 서버를 끈다. 하나라도 생기면 시계는 처음부터다.

use std::time::Duration;

use tokio::time::Instant;

use crate::config::IdleConfig;

/// 지금 남은 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Busy {
    pub links: usize,
    pub runs: usize,
    pub terminals: usize,
    /// 열린 MCP 연결(원격 에이전트 — 18-m7-plan M7-20). 에이전트가 붙어 있는 동안 데몬이 끝나 세션이 끊기지 않게.
    pub agents: usize,
}

impl Busy {
    /// 끝나지 못하게 하는 것이 있는가.
    pub fn holds(&self) -> bool {
        self.links > 0 || self.runs > 0 || self.terminals > 0 || self.agents > 0
    }
}

/// 유휴 판정이 낸 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    StopLanguageServers,
    Exit,
}

/// 유휴 시계. [`Self::tick`] 을 주기적으로 부른다.
#[derive(Debug)]
pub struct IdleClock {
    config: IdleConfig,
    /// 마지막으로 무엇이 남아 있던(끝나지 못하게 하던) 때.
    held_at: Instant,
    /// 마지막으로 링크가 있던 때.
    linked_at: Instant,
    lsp_stopped: bool,
}

impl IdleClock {
    pub fn new(config: IdleConfig, now: Instant) -> Self {
        Self {
            config,
            held_at: now,
            linked_at: now,
            lsp_stopped: false,
        }
    }

    pub fn tick(&mut self, now: Instant, busy: Busy) -> Option<Action> {
        if busy.links > 0 {
            self.linked_at = now;
            self.lsp_stopped = false;
        }
        if busy.holds() {
            self.held_at = now;
            return None;
        }
        if now.duration_since(self.held_at) >= self.config.exit_after {
            return Some(Action::Exit);
        }
        if !self.lsp_stopped && now.duration_since(self.linked_at) >= self.config.stop_lsp_after {
            self.lsp_stopped = true;
            return Some(Action::StopLanguageServers);
        }
        None
    }

    /// 다음 판정까지 기다릴 시간(시한이 짧으면 그만큼 자주).
    pub fn period(&self) -> Duration {
        self.config
            .exit_after
            .min(self.config.stop_lsp_after)
            .div_f64(10.0)
            .clamp(Duration::from_millis(50), Duration::from_secs(10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn config() -> IdleConfig {
        IdleConfig {
            exit_after: 30 * MIN,
            stop_lsp_after: 10 * MIN,
        }
    }

    #[test]
    fn nothing_left_exits_after_exit_after() {
        let start = Instant::now();
        let mut clock = IdleClock::new(config(), start);
        assert_eq!(clock.tick(start + 9 * MIN, Busy::default()), None);
        assert_eq!(
            clock.tick(start + 10 * MIN, Busy::default()),
            Some(Action::StopLanguageServers)
        );
        assert_eq!(clock.tick(start + 20 * MIN, Busy::default()), None);
        assert_eq!(
            clock.tick(start + 30 * MIN, Busy::default()),
            Some(Action::Exit)
        );
    }

    #[test]
    fn a_link_restarts_both_clocks() {
        let start = Instant::now();
        let mut clock = IdleClock::new(config(), start);
        let linked = Busy {
            links: 1,
            ..Busy::default()
        };
        assert_eq!(clock.tick(start + 29 * MIN, linked), None);
        assert_eq!(clock.tick(start + 38 * MIN, Busy::default()), None);
        assert_eq!(
            clock.tick(start + 39 * MIN, Busy::default()),
            Some(Action::StopLanguageServers)
        );
        assert_eq!(clock.tick(start + 58 * MIN, Busy::default()), None);
        assert_eq!(
            clock.tick(start + 59 * MIN, Busy::default()),
            Some(Action::Exit)
        );
    }
}
