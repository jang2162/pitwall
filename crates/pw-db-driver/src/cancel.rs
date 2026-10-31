//! 실행 하나의 취소: 토큰 → 그 연결의 인터럽트(DBX `RunningQueries` · `register_interrupt` 의 방식).
//!
//! 실행 id 마다 [`QueryCancel`] 하나를 두고(id → 취소의 표는 파이프라인 M3-05 가 갖는다), `db.query.cancel` · 스트림 취소가
//! [`QueryCancel::cancel`] 을 부른다. 인터럽트는 몇 번 취소해도 한 번만 간다 — PG CancelRequest · MySQL `KILL QUERY` 를
//! 여러 번 보내면 그사이 시작한 다음 문장까지 멈출 수 있다.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pw_db_types::DbError;
use tokio_util::sync::CancellationToken;

use crate::budget::within;
use crate::driver::{DbResult, Interrupt};

#[derive(Clone)]
pub struct QueryCancel {
    token: CancellationToken,
    interrupt: Arc<dyn Interrupt>,
    fired: Arc<AtomicBool>,
    cleanup: Duration,
}

impl QueryCancel {
    /// `interrupt` 는 실행할 연결의 인터럽트, `cleanup` 은 취소 뒤 실행이 끝나기를 기다리는 시간(`Budget::cancel_cleanup`).
    pub fn new(interrupt: Arc<dyn Interrupt>, cleanup: Duration) -> Self {
        Self {
            token: CancellationToken::new(),
            interrupt,
            fired: Arc::new(AtomicBool::new(false)),
            cleanup,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// 취소한다. 인터럽트는 처음 한 번만 보내고, 그 응답을 `cleanup` 까지 기다린다.
    pub async fn cancel(&self) -> DbResult<()> {
        self.token.cancel();
        if self.fired.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        within(self.cleanup, "cancel", self.interrupt.interrupt()).await
    }

    /// 드라이버 호출 하나를 취소와 함께 기다린다.
    ///
    /// 취소되면 호출을 버리지 않고 `cleanup` 까지 더 기다린다 — 인터럽트를 받은 드라이버가 스스로 돌아와야 연결이 깨끗하다.
    /// 그 안에 돌아오면 그 결과(대개 취소 오류)를, 못 돌아오면 타임아웃 오류를 돌려준다. 이미 취소된 뒤에 부르면 호출을
    /// 시작하지 않고 취소 오류다.
    pub async fn run<T>(&self, call: impl Future<Output = DbResult<T>>) -> DbResult<T> {
        if self.is_cancelled() {
            return Err(DbError::cancelled());
        }
        let mut call = std::pin::pin!(call);
        tokio::select! {
            result = &mut call => result,
            () = self.token.cancelled() => within(self.cleanup, "cancel cleanup", call).await,
        }
    }
}
