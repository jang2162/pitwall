//! `LOAD DATA LOCAL INFILE` 거절. 서버는 어떤 질의에든 「이 로컬 파일을 보내라」 를 요청할 수 있다(악의적 서버) — Pitwall 은
//! 사람 기기의 파일을 서버로 보내지 않는다.
//!
//! `mysql_async` 는 능력 `CLIENT_LOCAL_FILES` 를 늘 알리고 끄는 설정이 없다. 처리기가 없으면 서버에 아무 답도 하지 않은 채 오류를
//! 돌려주는데 그러면 서버는 다음 명령의 바이트를 파일 내용으로 읽고, 처리기가 오류를 내면 `mysql_async` 가 그 오류를 치명적으로 보고
//! 연결을 끊는다. 그래서 처리기는 파일 이름을 보지도 열지도 않고 **빈 내용**을 보낸다(서버는 0행으로 문장을 끝낸다 — 아무것도 쓰지
//! 않는다) — 연결은 그대로이고, 처리기가 불렸다는 표지([`RefuseLocalInfile::asked`])를 연결이 보고 그 문장을 거절 오류로 돌려준다.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use mysql_async::prelude::GlobalHandler;
use mysql_async::{InfileData, LocalInfileError};

/// 거절 글.
pub(crate) const REFUSED: &str =
    "LOAD DATA LOCAL INFILE is refused: Pitwall does not send local files to the server";

/// 모든 `LOCAL INFILE` 요청에 빈 내용으로 답하고 표지를 세운다. 연결 하나가 하나를 갖는다.
pub(crate) struct RefuseLocalInfile {
    asked: Arc<AtomicBool>,
}

impl RefuseLocalInfile {
    pub(crate) fn new(asked: Arc<AtomicBool>) -> Self {
        Self { asked }
    }

    /// 지난 확인 뒤로 서버가 파일을 청했는지(표지를 내린다).
    pub(crate) fn asked(flag: &AtomicBool) -> bool {
        flag.swap(false, Ordering::SeqCst)
    }
}

impl GlobalHandler for RefuseLocalInfile {
    fn handle(
        &self,
        _file_name: &[u8],
    ) -> futures_core::future::BoxFuture<'static, Result<InfileData, LocalInfileError>> {
        self.asked.store(true, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(Box::pin(Empty) as InfileData)))
    }
}

/// 빈 파일 내용.
struct Empty;

impl futures_core::Stream for Empty {
    type Item = io::Result<bytes::Bytes>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}
