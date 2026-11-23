//! HTTP 앞단(hyper http1 서버): 연결 상한 · 머리 · 본문 상한과 시한, 그리고 요청마다 **`Host` → `Origin` · `Sec-Fetch-Site` →
//! 토큰 → 경로 → 메서드** 순서로 본다. 판정(노출 · 승인)은 하지 않는다 — 통과한 메시지는 [`super::bridge`] 가 세션으로 옮긴다.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use hyper::body::{Body, Frame, Incoming};
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use super::bridge::{Asked, Opened, StreamGuard, Table};
use super::token::Token;
use crate::connections::Connections;
use crate::server::Services;

/// 세션 id 머리.
const SESSION_HEADER: &str = "mcp-session-id";
/// SSE 스트림의 keep-alive 주석 간격.
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// 이만큼 틀릴 때까지는 늦추지 않는다(사람이 토큰을 잘못 붙여 넣은 몇 번).
const FREE_FAILURES: u32 = 3;
/// 늦추기의 시작과 상한.
const FIRST_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(4);
/// 마지막 실패에서 이만큼 지나면 센 것을 잊는다.
const FAILURE_WINDOW: Duration = Duration::from_secs(60);

pub(super) struct Server {
    services: Arc<Services>,
    token: RwLock<Token>,
    port: u16,
    sessions: Arc<Table>,
    connections: Connections,
    failures: Failures,
    slots: Arc<Semaphore>,
    limits: super::Limits,
    /// 세션들의 부모. 토큰을 바꾸거나 닫으면 세션이 끝난다.
    cancel: CancellationToken,
}

impl Server {
    pub(super) fn new(
        services: Arc<Services>,
        token: Token,
        port: u16,
        connections: Connections,
        limits: super::Limits,
    ) -> Arc<Self> {
        Arc::new(Self {
            services,
            token: RwLock::new(token),
            port,
            sessions: Arc::new(Table::default()),
            connections,
            failures: Failures::default(),
            slots: Arc::new(Semaphore::new(super::MAX_CONNECTIONS)),
            limits,
            cancel: CancellationToken::new(),
        })
    }

    pub(super) fn token(&self) -> Token {
        self.token
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn replace_token(&self, token: Token) {
        *self.token.write().unwrap_or_else(PoisonError::into_inner) = token;
        self.sessions.close_all();
    }

    pub(super) fn close_all(&self) {
        self.cancel.cancel();
        self.sessions.close_all();
    }

    pub(super) fn sessions(&self) -> usize {
        self.sessions.len()
    }
}

/// 연결을 받아 hyper 로 돌린다. 연결 상한을 넘으면 받자마자 닫는다. 루프백이 아닌 상대(있을 수 없다 — 루프백에만 바인드한다)도
/// 닫는다.
pub(super) async fn accept_loop(
    server: Arc<Server>,
    listener: TcpListener,
    stop: CancellationToken,
) {
    loop {
        let (stream, peer) = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!(%e, "mcp http accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
        };
        if !peer.ip().is_loopback() {
            continue;
        }
        let Ok(slot) = Arc::clone(&server.slots).try_acquire_owned() else {
            tracing::debug!("too many mcp http connections; one was closed");
            continue;
        };
        let server = Arc::clone(&server);
        let stop = stop.clone();
        let header_timeout = server.limits.header_timeout;
        tokio::spawn(async move {
            let _slot = slot;
            let service = hyper::service::service_fn(move |request| {
                let server = Arc::clone(&server);
                async move { Ok::<_, Infallible>(handle(&server, request).await) }
            });
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder
                .timer(hyper_util::rt::TokioTimer::new())
                .header_read_timeout(header_timeout)
                .max_buf_size(super::MAX_HEADER.max(8192))
                .max_headers(64);
            let connection =
                builder.serve_connection(hyper_util::rt::TokioIo::new(stream), service);
            tokio::pin!(connection);
            tokio::select! {
                _ = connection.as_mut() => {}
                () = stop.cancelled() => {
                    connection.as_mut().graceful_shutdown();
                    let _ = tokio::time::timeout(Duration::from_secs(1), connection).await;
                }
            }
        });
    }
}

/// 놀고 있는 세션을 닫는다.
pub(super) async fn reap_idle(server: Arc<Server>, stop: CancellationToken) {
    let idle = server.limits.session_idle;
    let every = (idle / 10).max(Duration::from_millis(100));
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(every) => {}
        }
        server.sessions.close_idle(idle);
    }
}

async fn handle(server: &Arc<Server>, request: Request<Incoming>) -> Response<Out> {
    if !host_allowed(request.headers(), server.port) {
        return plain(
            StatusCode::FORBIDDEN,
            "the Host header must name this loopback endpoint",
        );
    }
    if !origin_allowed(request.headers(), server.port) {
        return plain(
            StatusCode::FORBIDDEN,
            "requests from another origin are refused",
        );
    }
    if !authorized(request.headers(), &server.token()) {
        server.failures.punish().await;
        let mut response = plain(StatusCode::UNAUTHORIZED, "a valid bearer token is required");
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return response;
    }
    if request.uri().path() != super::PATH {
        return plain(StatusCode::NOT_FOUND, "not found");
    }
    match *request.method() {
        Method::POST => post(server, request).await,
        Method::GET => get(server, request.headers()),
        Method::DELETE => delete(server, request.headers()),
        _ => {
            let mut response = plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, POST, DELETE"));
            response
        }
    }
}

/// `Host` 는 이 끝점을 가리키는 셋만(DNS rebinding — 다른 이름으로 풀린 페이지가 이 포트에 닿아도 그 이름이 실린다). 없으면 거절.
pub(super) fn host_allowed(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    loopback_authorities(port).iter().any(|a| *a == host)
}

/// `Origin` 이 있으면 `http://` 의 같은 셋만. `Sec-Fetch-Site` 가 있으면 `none`(사람이 주소창에 친 것) · `same-origin` 만.
pub(super) fn origin_allowed(headers: &HeaderMap, port: u16) -> bool {
    if let Some(site) = headers.get("sec-fetch-site") {
        let site = site.to_str().unwrap_or("").to_ascii_lowercase();
        if site != "none" && site != "same-origin" {
            return false;
        }
    }
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let Some(origin) = origins.next() else {
        return true;
    };
    if origins.next().is_some() {
        return false;
    }
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let origin = origin.to_ascii_lowercase();
    loopback_authorities(port)
        .iter()
        .any(|a| origin == format!("http://{a}"))
}

fn loopback_authorities(port: u16) -> [String; 3] {
    [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ]
}

fn authorized(headers: &HeaderMap, token: &Token) -> bool {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return false;
    };
    let value = value.as_bytes();
    let scheme = b"bearer ";
    if value.len() <= scheme.len() || !value[..scheme.len()].eq_ignore_ascii_case(scheme) {
        return false;
    }
    token.matches(&value[scheme.len()..])
}

async fn post(server: &Arc<Server>, request: Request<Incoming>) -> Response<Out> {
    let json_body = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    if !json_body {
        return plain(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the body must be application/json",
        );
    }
    let session = request
        .headers()
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = match read_body(request.into_body(), server.limits).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return rpc_error(StatusCode::BAD_REQUEST, -32700, "the body is not json");
    };
    if message.is_array() {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "JSON-RPC batches are not supported; send one message per request",
        );
    }
    if !message.is_object() {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "the body must be a JSON-RPC message",
        );
    }
    let method = message.get("method").and_then(Value::as_str);
    let is_request = method.is_some() && message.get("id").is_some();
    let Some(session_id) = session else {
        if method == Some("initialize") && is_request {
            return initialize(server, message).await;
        }
        return rpc_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "missing Mcp-Session-Id: start a session with initialize (MCP 2025-11-25 or earlier); \
             sessionless MCP over http (2026-07-28) is not served here, use stdio (`pitwall mcp`) instead",
        );
    };
    let Some(session) = server.sessions.get(&session_id) else {
        return rpc_error(
            StatusCode::NOT_FOUND,
            -32001,
            "no such mcp session; initialize again",
        );
    };
    if method == Some("initialize") {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "this session is already initialized",
        );
    }
    if !is_request {
        // 알림 · 클라이언트의 응답.
        return if session.send(&message).await {
            empty(StatusCode::ACCEPTED)
        } else {
            rpc_error(StatusCode::NOT_FOUND, -32001, "the mcp session has ended")
        };
    }
    match session.ask(message).await {
        Asked::Answer(answer) => json_response(StatusCode::OK, &answer),
        Asked::Duplicate => rpc_error(
            StatusCode::CONFLICT,
            -32600,
            "a request with this id is already waiting for its answer",
        ),
        Asked::Busy => rpc_error(
            StatusCode::TOO_MANY_REQUESTS,
            -32600,
            "too many requests are waiting in this session",
        ),
        Asked::Closed => rpc_error(StatusCode::NOT_FOUND, -32001, "the mcp session has ended"),
    }
}

async fn initialize(server: &Arc<Server>, message: Value) -> Response<Out> {
    match server
        .sessions
        .open(
            &server.services,
            &server.connections,
            message,
            &server.cancel,
            server.limits,
        )
        .await
    {
        Opened::Ready(session, answer) => {
            let mut response = json_response(StatusCode::OK, &answer);
            if let Ok(value) = HeaderValue::from_str(session.id()) {
                response.headers_mut().insert(SESSION_HEADER, value);
            }
            response
        }
        Opened::Refused(Some(answer)) => json_response(StatusCode::OK, &answer),
        Opened::Refused(None) => rpc_error(
            StatusCode::SERVICE_UNAVAILABLE,
            -32603,
            "the mcp session did not start",
        ),
        Opened::Full => rpc_error(
            StatusCode::SERVICE_UNAVAILABLE,
            -32603,
            "too many mcp http sessions are open; close one (DELETE) and try again",
        ),
    }
}

fn get(server: &Arc<Server>, headers: &HeaderMap) -> Response<Out> {
    let wants_stream = headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.to_ascii_lowercase().contains("text/event-stream"));
    if !wants_stream {
        return plain(StatusCode::NOT_ACCEPTABLE, "GET opens a text/event-stream");
    }
    let Some(session) = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|id| server.sessions.get(id))
    else {
        return plain(StatusCode::NOT_FOUND, "no such mcp session");
    };
    let Some((rx, guard)) = session.open_stream() else {
        return plain(
            StatusCode::CONFLICT,
            "this session already has an open stream",
        );
    };
    let mut keep_alive = tokio::time::interval(KEEP_ALIVE);
    keep_alive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    keep_alive.reset();
    let mut response = Response::new(Out::Stream(Box::new(Stream {
        rx,
        keep_alive,
        _guard: guard,
    })));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn delete(server: &Arc<Server>, headers: &HeaderMap) -> Response<Out> {
    let closed = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|id| server.sessions.close(id));
    if closed {
        empty(StatusCode::OK)
    } else {
        plain(StatusCode::NOT_FOUND, "no such mcp session")
    }
}

/// 본문을 상한 · 시한 안에서 읽는다.
async fn read_body(mut body: Incoming, limits: super::Limits) -> Result<Vec<u8>, Response<Out>> {
    if body
        .size_hint()
        .lower()
        .try_into()
        .map_or(true, |n: usize| n > limits.max_body)
    {
        return Err(plain(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the body is too large",
        ));
    }
    let read = async {
        let mut out = Vec::new();
        while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await
        {
            let Ok(frame) = frame else {
                return Err(plain(StatusCode::BAD_REQUEST, "the body could not be read"));
            };
            if let Ok(data) = frame.into_data() {
                if out.len() + data.len() > limits.max_body {
                    return Err(plain(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "the body is too large",
                    ));
                }
                out.extend_from_slice(&data);
            }
        }
        Ok(out)
    };
    tokio::time::timeout(limits.body_timeout, read)
        .await
        .unwrap_or_else(|_| {
            Err(plain(
                StatusCode::REQUEST_TIMEOUT,
                "the body did not arrive in time",
            ))
        })
}

/// 틀린 토큰이 잇따르면 응답을 늦춘다. 늦추는 동안 다른 실패도 줄을 선다(문 하나) — 연결을 여럿 열어 늦춤을 나눠 갖지 못하게.
/// 맞는 토큰의 요청은 이 문을 지나지 않는다.
#[derive(Default)]
pub(super) struct Failures {
    seen: Mutex<(u32, Option<Instant>)>,
    gate: tokio::sync::Mutex<()>,
}

impl Failures {
    async fn punish(&self) {
        let count = {
            let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
            if seen.1.is_some_and(|at| at.elapsed() > FAILURE_WINDOW) {
                seen.0 = 0;
            }
            seen.0 = seen.0.saturating_add(1);
            seen.1 = Some(Instant::now());
            seen.0
        };
        let delay = delay(count);
        if !delay.is_zero() {
            let _one = self.gate.lock().await;
            tokio::time::sleep(delay).await;
        }
    }
}

/// `count` 번째 잇따른 실패의 늦춤.
pub(super) fn delay(count: u32) -> Duration {
    if count <= FREE_FAILURES {
        return Duration::ZERO;
    }
    let doublings = (count - FREE_FAILURES - 1).min(8);
    (FIRST_DELAY * 2u32.pow(doublings)).min(MAX_DELAY)
}

/// 응답 본문: 한 덩어리 또는 SSE 스트림.
pub(super) enum Out {
    Full(Option<Bytes>),
    Stream(Box<Stream>),
}

pub(super) struct Stream {
    rx: mpsc::Receiver<Bytes>,
    keep_alive: tokio::time::Interval,
    _guard: StreamGuard,
}

impl Body for Out {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        match &mut *self {
            Out::Full(data) => Poll::Ready(data.take().map(|d| Ok(Frame::data(d)))),
            Out::Stream(stream) => {
                if let Poll::Ready(next) = stream.rx.poll_recv(cx) {
                    return Poll::Ready(next.map(|d| Ok(Frame::data(d))));
                }
                if stream.keep_alive.poll_tick(cx).is_ready() {
                    return Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(
                        b": keep-alive\n\n",
                    )))));
                }
                Poll::Pending
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self, Out::Full(None))
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        match self {
            Out::Full(data) => {
                hyper::body::SizeHint::with_exact(data.as_ref().map_or(0, |d| d.len() as u64))
            }
            Out::Stream(_) => hyper::body::SizeHint::default(),
        }
    }
}

fn with_common_headers(mut response: Response<Out>) -> Response<Out> {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn plain(status: StatusCode, text: &'static str) -> Response<Out> {
    let mut response = Response::new(Out::Full(Some(Bytes::from_static(text.as_bytes()))));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    with_common_headers(response)
}

fn empty(status: StatusCode) -> Response<Out> {
    let mut response = Response::new(Out::Full(None));
    *response.status_mut() = status;
    with_common_headers(response)
}

fn json_response(status: StatusCode, value: &Value) -> Response<Out> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    let mut response = Response::new(Out::Full(Some(Bytes::from(body))));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    with_common_headers(response)
}

fn rpc_error(status: StatusCode, code: i64, message: &str) -> Response<Out> {
    json_response(
        status,
        &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": message } }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn only_loopback_hosts_with_this_port_pass() {
        for ok in [
            "127.0.0.1:7425",
            "localhost:7425",
            "LOCALHOST:7425",
            "[::1]:7425",
        ] {
            assert!(host_allowed(&headers(&[("host", ok)]), 7425), "{ok}");
        }
        for bad in [
            "evil.example:7425",
            "127.0.0.1",
            "localhost",
            "127.0.0.1:80",
            "127.0.0.1.nip.io:7425",
            "0.0.0.0:7425",
        ] {
            assert!(!host_allowed(&headers(&[("host", bad)]), 7425), "{bad}");
        }
        assert!(!host_allowed(&HeaderMap::new(), 7425));
    }

    #[test]
    fn origins_and_fetch_sites() {
        assert!(
            origin_allowed(&HeaderMap::new(), 7425),
            "Origin 없는 클라이언트(에이전트)"
        );
        assert!(origin_allowed(
            &headers(&[("origin", "http://127.0.0.1:7425")]),
            7425
        ));
        assert!(origin_allowed(
            &headers(&[("origin", "http://localhost:7425")]),
            7425
        ));
        for bad in [
            "http://evil.example",
            "null",
            "https://127.0.0.1:7425",
            "http://127.0.0.1:3000",
            "http://localhost",
        ] {
            assert!(!origin_allowed(&headers(&[("origin", bad)]), 7425), "{bad}");
        }
        assert!(!origin_allowed(
            &headers(&[
                ("origin", "http://127.0.0.1:7425"),
                ("origin", "http://evil.example")
            ]),
            7425
        ));
        assert!(!origin_allowed(
            &headers(&[("sec-fetch-site", "cross-site")]),
            7425
        ));
        assert!(!origin_allowed(
            &headers(&[("sec-fetch-site", "same-site")]),
            7425
        ));
        assert!(origin_allowed(
            &headers(&[("sec-fetch-site", "none")]),
            7425
        ));
    }

    #[test]
    fn repeated_failures_are_slowed_up_to_a_cap() {
        assert_eq!(delay(1), Duration::ZERO);
        assert_eq!(delay(FREE_FAILURES), Duration::ZERO);
        assert_eq!(delay(FREE_FAILURES + 1), FIRST_DELAY);
        assert_eq!(delay(FREE_FAILURES + 2), FIRST_DELAY * 2);
        assert_eq!(delay(100), MAX_DELAY);
    }
}
