//! 연결 하나 = DB 번호 하나(결정 M3-D15). `MultiplexedConnection` 과 그 구동 태스크를 함께 쥐고, 응답 타임아웃 · 취소에서
//! 진행 중 요청을 버리며 그 연결을 다시 만든다.
//!
//! - **구동 태스크**: `new_with_config` 가 돌려준 future 를 `spawn` 하고, 연결을 버릴 때 그 태스크를 끝낸다(abort). 소켓이
//!   닫히므로 서버는 그 연결에서 막혀 있던 명령(`BLPOP` 등)을 거둔다. 이미 도는 명령(서버를 붙잡는 명령)은 멈추지 않는다 —
//!   그래서 무거운 명령은 분류가 막는다(`pw_db_sql::kv_command`).
//! - **응답 타임아웃 · 취소**: 요청 하나마다 [`Connection::response_timeout`] 과 인터럽트를 함께 기다린다. 어느 쪽이 먼저
//!   오면 요청을 버리고 연결을 버린다 — 같은 연결에 남기면 다음 요청이 서버에서 그 명령 뒤에 줄을 선다. 다음 호출이 새로
//!   연다(같은 설정, 연결 타임아웃 안에서).
//! - **끊김**: 서버가 연결을 닫으면(재시작 · `CLIENT KILL`) 저절로 다시 붙지 않는다 — 다음 호출마다 `Disconnected` 이고, 다시
//!   붙는 것은 사람이 누를 때다(결정 M3-D10 의 대가, 14 §3).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pw_db_driver::{BoxFuture, DbResult, Interrupt, TlsPlan};
use pw_db_types::{DbError, DbErrorKind, TlsSettings, TlsStatus};
use redis::aio::MultiplexedConnection;
use redis::{AsyncConnectionConfig, RedisConnectionInfo, RedisResult};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::{reply, tls};

/// 다시 열 때 쓰는 설정(연 연결의 설정과 같다).
pub(crate) struct Endpoint {
    /// 실제로 붙을 주소(터널이 있으면 그 로컬 포트).
    pub host: String,
    pub port: u16,
    pub plan: TlsPlan,
    pub tls: TlsSettings,
    pub info: RedisConnectionInfo,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
}

/// 연 연결. 버리면 구동 태스크를 끝낸다.
struct Live {
    conn: MultiplexedConnection,
    driver: JoinHandle<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

/// 인터럽트가 기다리는 요청을 깨운다. 도는 요청이 없으면 아무것도 하지 않는다(`Interrupt` 계약).
#[derive(Default)]
struct Shared {
    notify: Notify,
    running: AtomicBool,
}

struct RedisInterrupt(Arc<Shared>);

impl Interrupt for RedisInterrupt {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>> {
        if self.0.running.load(Ordering::SeqCst) {
            self.0.notify.notify_waiters();
        }
        Box::pin(std::future::ready(Ok(())))
    }
}

pub(crate) struct Connection {
    endpoint: Arc<Endpoint>,
    live: Option<Live>,
    /// 서버 쪽에서 끊겼다. 다시 붙지 않는다.
    broken: Option<DbError>,
    shared: Arc<Shared>,
}

impl Connection {
    /// 연다. TLS 상태(연결 테스트 · 서버 정보)를 함께 돌려준다.
    pub(crate) async fn open(endpoint: Endpoint) -> DbResult<(Self, TlsStatus)> {
        let endpoint = Arc::new(endpoint);
        let (live, used_tls) = connect(&endpoint).await?;
        let status = endpoint.plan.status(used_tls);
        Ok((
            Self {
                endpoint,
                live: Some(live),
                broken: None,
                shared: Arc::default(),
            },
            status,
        ))
    }

    pub(crate) fn interrupter(&self) -> Arc<dyn Interrupt> {
        Arc::new(RedisInterrupt(self.shared.clone()))
    }

    /// 지금 연결(없으면 다시 연다)로 요청 하나를 보낸다. 응답 타임아웃 · 인터럽트면 요청과 연결을 버린다.
    pub(crate) async fn run<T, F, Fut>(&mut self, request: F) -> DbResult<RedisResult<T>>
    where
        F: FnOnce(MultiplexedConnection) -> Fut,
        Fut: Future<Output = RedisResult<T>>,
    {
        if let Some(error) = &self.broken {
            return Err(error.clone());
        }
        let conn = match &self.live {
            Some(live) => live.conn.clone(),
            None => {
                let (live, _) = connect(&self.endpoint).await?;
                let conn = live.conn.clone();
                self.live = Some(live);
                conn
            }
        };
        let notified = self.shared.notify.notified();
        let mut notified = std::pin::pin!(notified);
        notified.as_mut().enable();
        self.shared.running.store(true, Ordering::SeqCst);
        let outcome = tokio::select! {
            result = tokio::time::timeout(self.endpoint.response_timeout, request(conn)) => {
                result.map_err(|_| DbError::timeout("Redis response"))
            }
            () = &mut notified => Err(DbError::cancelled()),
        };
        self.shared.running.store(false, Ordering::SeqCst);
        match outcome {
            Ok(Err(error)) if !reply::is_server_reply(&error) => {
                let error = reply::db_error(error);
                if reply::breaks_connection(&error) {
                    self.live = None;
                    self.broken = Some(error.clone());
                }
                Err(error)
            }
            Ok(result) => Ok(result),
            Err(error) => {
                // 요청을 버렸다 — 같은 연결의 다음 요청이 서버에서 그 명령 뒤에 서지 않게 연결도 버린다.
                self.live = None;
                Err(error)
            }
        }
    }

    /// 요청 하나. 서버의 오류 응답도 오류다(`Server`).
    pub(crate) async fn query<T, F, Fut>(&mut self, request: F) -> DbResult<T>
    where
        F: FnOnce(MultiplexedConnection) -> Fut,
        Fut: Future<Output = RedisResult<T>>,
    {
        self.run(request).await?.map_err(reply::db_error)
    }

    pub(crate) fn close(self) {
        drop(self.live);
    }
}

/// TCP → (TLS) → 핸드셰이크(`AUTH` · `SELECT <db>`). 연결 타임아웃 하나 안에서.
async fn connect(endpoint: &Endpoint) -> DbResult<(Live, bool)> {
    pw_db_driver::budget::within(endpoint.connect_timeout, "connect", async {
        let tcp = connect_tcp(&endpoint.host, endpoint.port).await?;
        let (stream, used_tls) = tls::wrap(tcp, &endpoint.plan, &endpoint.tls).await?;
        // 응답 타임아웃은 이 크레이트가 요청마다 건다(연결을 버리려고) — redis-rs 의 것은 끈다.
        let config = AsyncConnectionConfig::new().set_response_timeout(None);
        let (conn, driver) = MultiplexedConnection::new_with_config(&endpoint.info, stream, config)
            .await
            .map_err(reply::db_error)?;
        let driver = tokio::spawn(driver);
        Ok((Live { conn, driver }, used_tls))
    })
    .await
}

async fn connect_tcp(host: &str, port: u16) -> DbResult<TcpStream> {
    let unreachable = |e: std::io::Error| {
        DbError::new(
            DbErrorKind::Connect,
            format!("cannot connect to {host}:{port}: {e}"),
        )
    };
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(unreachable)?
        .collect();
    let mut last = None;
    for address in addresses {
        match TcpStream::connect(address).await {
            Ok(tcp) => {
                let _ = tcp.set_nodelay(true);
                return Ok(tcp);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(unreachable(last.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no address")
    })))
}
