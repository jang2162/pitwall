//! 전송 추상(18-m7-plan M7-03 ①): 바이트 스트림 하나를 연다. 1차는 `Ssh`(M7-06 — `pitwall-relay proxy` 의 stdio), 시험은
//! [`memory`], 3차에 `Quic`(M7-24). 링크는 그 위에 틀 · 핸드셰이크 · 다중화를 얹는다 — 전송은 암호화 · 인증을 하지 않는다고 본다.

use std::io;

use pw_core::BoxFuture;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc};

pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
pub type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

/// 연 바이트 스트림 하나. 읽는 쪽과 쓰는 쪽을 따로 태스크로 옮길 수 있게 나눠 둔다.
pub struct LinkStream {
    pub read: BoxRead,
    pub write: BoxWrite,
    /// Noise 세션 위의 스트림이면 그 세션이 증명한 상대(`crate::noise` — `dial` · `serve` 가 안쪽 `hello` 의 키와 견준다). 이
    /// 크레이트의 Noise 만 채운다.
    pub(crate) bound: Option<crate::noise::Bound>,
}

impl LinkStream {
    pub fn new(
        read: impl AsyncRead + Send + Unpin + 'static,
        write: impl AsyncWrite + Send + Unpin + 'static,
    ) -> Self {
        Self {
            read: Box::new(read),
            write: Box::new(write),
            bound: None,
        }
    }

    /// Noise 세션 위의 스트림인가(직접 연결 · 코드 페어링 — M7-23).
    pub fn is_secured(&self) -> bool {
        self.bound.is_some()
    }

    /// 읽고 쓰는 스트림 하나(유닉스 소켓 · duplex)를 나눈다.
    pub fn split<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (read, write) = tokio::io::split(stream);
        Self::new(read, write)
    }
}

/// Desktop 이 Relay 로 링크 하나를 열 때 쓰는 길.
pub trait Transport: Send + Sync + 'static {
    /// 사람이 읽는 이름(진단 · 로그). 비밀을 싣지 않는다.
    fn describe(&self) -> String;

    /// 바이트 스트림 하나를 연다. 다시 붙을 때(M7-09)는 다시 부른다.
    fn open(&self) -> BoxFuture<'_, io::Result<LinkStream>>;
}

/// 같은 프로세스 안의 가짜 전송(시험 — 18-m7-plan 「1」 머리의 「in-memory 링크」). `open` 마다 `tokio::io::duplex` 하나를 만들어
/// 한쪽을 [`MemoryAcceptor`] 로 건넨다. `buffer` 는 한 방향에 쌓이는 바이트의 상한이다 — 상대가 읽지 않으면 쓰기가 멈춘다.
pub fn memory(buffer: usize) -> (MemoryTransport, MemoryAcceptor) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        MemoryTransport { buffer, tx },
        MemoryAcceptor { rx: Mutex::new(rx) },
    )
}

pub struct MemoryTransport {
    buffer: usize,
    tx: mpsc::UnboundedSender<LinkStream>,
}

impl Transport for MemoryTransport {
    fn describe(&self) -> String {
        "memory".to_owned()
    }

    fn open(&self) -> BoxFuture<'_, io::Result<LinkStream>> {
        let (ours, theirs) = tokio::io::duplex(self.buffer);
        let sent = self.tx.send(LinkStream::split(theirs));
        Box::pin(async move {
            sent.map_err(|_| io::Error::new(io::ErrorKind::ConnectionRefused, "no acceptor"))?;
            Ok(LinkStream::split(ours))
        })
    }
}

/// [`MemoryTransport::open`] 이 연 스트림의 받는 쪽(Relay 데몬의 소켓 자리).
pub struct MemoryAcceptor {
    rx: Mutex<mpsc::UnboundedReceiver<LinkStream>>,
}

impl MemoryAcceptor {
    /// 다음에 열린 스트림. 전송을 버리면 `None`.
    pub async fn accept(&self) -> Option<LinkStream> {
        self.rx.lock().await.recv().await
    }
}
