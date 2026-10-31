//! 실행 예산: 연결 타임아웃 · 취소 정리 · 비활동 타임아웃(14-m3-plan.md M3-05 의 기본값, DBX `DbOperationBudget` 의 방식).
//!
//! 비활동 타임아웃은 전체 실행 시간이 아니라 「다음 행 묶음이 올 때까지」 의 상한이다 — 행이 올 때마다 다시 감는다
//! ([`Inactivity`]). 콘솔은 기본으로 꺼져 있고(긴 분석 쿼리), 쪽 단위 실행(`db.query.page`)은 30 s 다.

use std::future::Future;
use std::time::Duration;

use pw_db_types::DbError;
use tokio::time::Instant;

use crate::driver::DbResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// 연결을 여는 시간 상한.
    pub connect: Duration,
    /// 취소를 보낸 뒤 실행이 실제로 끝나기를 기다리는 시간. 넘으면 기다리지 않고 타임아웃으로 끝낸다.
    pub cancel_cleanup: Duration,
    /// 다음 행 묶음을 기다리는 시간 상한. `None` 이면 없다.
    pub inactivity: Option<Duration>,
}

impl Budget {
    /// SQL 콘솔: 비활동 타임아웃 없음.
    pub const CONSOLE: Self = Self {
        connect: Duration::from_secs(10),
        cancel_cleanup: Duration::from_secs(5),
        inactivity: None,
    };

    /// 쪽 단위 실행(`db.query.page`, 결정 M3-D19).
    pub const PAGE: Self = Self {
        inactivity: Some(Duration::from_secs(30)),
        ..Self::CONSOLE
    };
}

impl Default for Budget {
    fn default() -> Self {
        Self::CONSOLE
    }
}

/// `limit` 안에 끝나지 않으면 `what` 의 타임아웃 오류. 넘으면 `future` 를 버린다.
pub async fn within<T>(
    limit: Duration,
    what: &str,
    future: impl Future<Output = DbResult<T>>,
) -> DbResult<T> {
    tokio::time::timeout(limit, future)
        .await
        .unwrap_or_else(|_| Err(DbError::timeout(what)))
}

/// 「행이 올 때마다 다시 감는」 비활동 타임아웃.
#[derive(Debug)]
pub struct Inactivity {
    limit: Option<Duration>,
    deadline: Option<Instant>,
}

impl Inactivity {
    pub fn new(limit: Option<Duration>) -> Self {
        Self {
            limit,
            deadline: limit.map(|l| Instant::now() + l),
        }
    }

    /// 지금부터 다시 감는다.
    pub fn rearm(&mut self) {
        self.deadline = self.limit.map(|l| Instant::now() + l);
    }

    /// `future` 를 기다린다. 마감을 넘으면 타임아웃 오류로 끝내고, 성공하면 다시 감는다.
    pub async fn guard<T>(&mut self, future: impl Future<Output = DbResult<T>>) -> DbResult<T> {
        let result = match self.deadline {
            None => future.await,
            Some(deadline) => tokio::time::timeout_at(deadline, future)
                .await
                .unwrap_or_else(|_| Err(DbError::timeout("waiting for rows"))),
        };
        if result.is_ok() {
            self.rearm();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use pw_db_types::DbErrorKind;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn inactivity_rearms_on_every_batch() {
        let mut inactivity = Inactivity::new(Some(Duration::from_secs(30)));
        // 묶음마다 20 s — 합치면 상한을 넘지만 묶음 사이 간격은 늘 안이다.
        for _ in 0..3 {
            let batch = inactivity
                .guard(async {
                    tokio::time::sleep(Duration::from_secs(20)).await;
                    Ok(())
                })
                .await;
            assert!(batch.is_ok());
        }
        let stalled = inactivity
            .guard(async {
                tokio::time::sleep(Duration::from_secs(31)).await;
                Ok(())
            })
            .await;
        assert_eq!(stalled.unwrap_err().kind, DbErrorKind::Timeout);
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_limit_nothing_times_out() {
        let mut inactivity = Inactivity::new(Budget::CONSOLE.inactivity);
        let slow = inactivity
            .guard(async {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                Ok(7)
            })
            .await;
        assert_eq!(slow.unwrap(), 7);
        let late = within(Duration::from_secs(1), "connect", async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(())
        })
        .await;
        assert_eq!(late.unwrap_err().message, "connect timed out");
    }
}
