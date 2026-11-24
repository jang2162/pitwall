//! Streamable HTTP(`http://127.0.0.1:<port>/mcp` — 05-mcp.md 「전송 방식」, 18-m7-plan M7-25 · 결정 M7-D11).
//!
//! - **판정은 소켓 · stdio 와 같은 `Session`**: HTTP 세션 하나가 프로세스 안 duplex 하나에 [`crate::server::serve`] 를 그대로 잇는다
//!   (`bridge`). 노출 · 승인 · 감사 · 세션 등록부 · 머신 허용 · Relay 의 「붙은 owner」 관문이 모두 그 안이다 — 이 모듈은 판정을 갖지
//!   않고 HTTP 를 JSON-RPC 줄로 옮기기만 한다. [`crate::Services::transport`] 는 `http`(감사 행 · 세션 목록).
//! - **기본 꺼짐**: 전역 설정의 `[mcp.http] enabled = true` 일 때만 듣는다(프로젝트 설정에서는 읽지 않는다). `port` 가 없으면
//!   [`HttpParts::default_port`](Desktop [`DESKTOP_PORT`] · Relay [`RELAY_PORT`]). 설정이 바뀌면(`config.changed`) 다시 읽어 열고
//!   닫는다. 값이 틀리면 듣지 않는다(fail-closed).
//! - **루프백만**: `127.0.0.1` 과 `::1`(그 기기에 IPv6 가 있으면)에만, 고를 길이 없다. `SO_REUSEADDR` 를 끄고 바인드해 다른 소켓이
//!   쥔 포트를 덮지 않는다(M7-22 리뷰 F1 — macOS 는 `SO_REUSEADDR` 면 `0.0.0.0:P` 위에 `127.0.0.1:P` 가 바인드된다). 둘 중 하나라도
//!   다른 소켓이 쥐고 있으면 아무것도 듣지 않는다 — `localhost` 가 `::1` 로 풀리는 클라이언트가 다른 프로세스에 토큰을 보내지 않게.
//! - **토큰**: [`token`]. 모든 요청이 `Authorization: Bearer <token>` 을 싣는다(질의 글 · 쿠키로는 받지 않는다). 비교는 상수 시간, 틀린
//!   토큰이 잇따르면 응답을 늦춘다(`front::Failures`). 토큰을 새로 만들면 열린 HTTP 세션을 모두 닫는다.
//! - **브라우저**: `Host` 는 `127.0.0.1:<port>` · `localhost:<port>` · `[::1]:<port>` 만(DNS rebinding), `Origin` 이 있으면
//!   `http://` 의 같은 셋만, `Sec-Fetch-Site` 가 있으면 `none` · `same-origin` 만 — 아니면 403. CORS 머리는 내지 않는다(프리플라이트
//!   `OPTIONS` 는 405). POST 는 `Content-Type: application/json` 만(415) — 폼 · `text/plain` 단순 요청으로 툴에 닿지 않는다.
//!   `Origin` 없는 요청은 브라우저 밖의 클라이언트(에이전트)의 꼴이라 받는다 — 브라우저가 `Origin` 없이 보내는 요청(주소창 이동 ·
//!   `<img>` 등)은 `Authorization` 을 실을 수 없어 401 이다.
//! - **받는 판**: 세션이 있는 Streamable HTTP(2025-03-26 ~ 2025-11-25 — `initialize` 가 `Mcp-Session-Id` 를 준다). 2026-07-28 의
//!   세션 없는 HTTP 는 받지 않는다(400 — 그 클라이언트는 stdio `pitwall mcp` 를 쓴다). JSON-RPC 묶음(배열)은 받지 않는다. 요청의
//!   응답은 `application/json` 하나, 서버가 먼저 보내는 것(`tools/list_changed`)은 `GET` 의 SSE 스트림으로 간다.
//! - **상한**: 세션 [`MAX_SESSIONS`], 연결 [`MAX_CONNECTIONS`], 본문 [`MAX_BODY`], 머리 [`MAX_HEADER`], 머리 읽기
//!   [`HEADER_TIMEOUT`](keep-alive 로 놀고 있는 연결도 이 시간에 닫힌다) · 본문 읽기 [`BODY_TIMEOUT`], 세션마다 기다리는 요청
//!   [`MAX_PENDING`] · SSE 스트림 하나, 아무것도 없이 [`SESSION_IDLE`] 지난 세션은 닫는다.
//! - **세션 id** 는 OS 난수 32바이트의 16진 글이고 로그에 쓰지 않는다. 쓰려면 지금 토큰도 함께 있어야 한다 — 토큰을 가진 쪽은 그
//!   사용자와 같은 경계다(로컬 소켓의 「그 사용자만」 과 같다. 여러 사용자 머신에서는 토큰만이 막는다 — 결정 M7-D11 의 대가).

mod bridge;
mod front;
pub mod token;

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_core::config::TOPIC_CHANGED;
use pw_core::{Filter, Scope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::{TcpListener, TcpSocket};
use tokio_util::sync::CancellationToken;

use crate::connections::Connections;
use crate::server::Services;
pub use token::{Token, TokenFile};

/// 엔드포인트의 경로.
pub const PATH: &str = "/mcp";
/// 전역 `[mcp]` 안의 표 이름(`[mcp.http]`)과 그 키.
pub const TABLE: &str = "http";
pub const ENABLED_KEY: &str = "enabled";
pub const PORT_KEY: &str = "port";
/// `port` 가 없을 때 — Desktop.
pub const DESKTOP_PORT: u16 = 7425;
/// `port` 가 없을 때 — Relay(같은 머신에 Desktop 과 Relay 가 함께 있어도 겹치지 않게).
pub const RELAY_PORT: u16 = 7426;
/// 연결 전송 이름(감사 행 · 세션 목록).
pub const TRANSPORT: &str = "http";

/// 열린 HTTP 세션의 상한. 넘는 `initialize` 는 503.
pub const MAX_SESSIONS: usize = 16;
/// 동시에 열린 TCP 연결의 상한. 넘는 연결은 받자마자 닫는다.
pub const MAX_CONNECTIONS: usize = 64;
/// 요청 본문의 상한(413).
pub const MAX_BODY: usize = 4 * 1024 * 1024;
/// 요청 머리(줄 + 머리 전부)의 상한(431).
pub const MAX_HEADER: usize = 16 * 1024;
/// 요청 머리를 다 받는 시한 — 느린 요청 · 놀고 있는 keep-alive 연결이 자리를 쥐지 못하게.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// 요청 본문을 다 받는 시한(408).
pub const BODY_TIMEOUT: Duration = Duration::from_secs(30);
/// 켰는데 듣지 못했을 때 다시 해 보는 간격.
pub const RETRY_BIND: Duration = Duration::from_secs(2);
/// 틀린 토큰으로 늦춤 중인 연결(인증 전 자리)의 상한. 늦추는 동안 그 연결은 연결 자리([`MAX_CONNECTIONS`])를 놓는다 — 늦춤이
/// 맞는 토큰의 새 연결을 막지 않는다(리뷰 F1). 넘는 실패는 늦추지 않고 곧바로 닫는다.
pub const MAX_DELAYED: usize = 8;
/// 맞는 토큰의 요청이 한 번도 지나지 않은 연결이 사는 시간(머리 시한보다 짧다).
pub const UNAUTHENTICATED_LIFETIME: Duration = Duration::from_secs(5);
/// 세션마다 답을 기다리는 요청의 상한(429).
pub const MAX_PENDING: usize = 32;
/// 기다리는 요청도 SSE 스트림도 없이 이만큼 지난 세션은 닫는다.
pub const SESSION_IDLE: Duration = Duration::from_secs(30 * 60);

/// 상한 · 시한. 앱 · Relay 는 [`Limits::default`](위 상수들)를 쓴다 — 시험만 줄인다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_sessions: usize,
    pub max_body: usize,
    pub header_timeout: Duration,
    pub body_timeout: Duration,
    pub session_idle: Duration,
    /// 틀린 토큰으로 늦춤 중인 연결의 상한 — 넘으면 늦추지 않고 곧바로 401 로 닫는다.
    pub max_delayed: usize,
    /// 맞는 토큰의 요청이 한 번도 지나지 않은 연결이 사는 시간.
    pub unauthenticated_lifetime: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_sessions: MAX_SESSIONS,
            max_body: MAX_BODY,
            header_timeout: HEADER_TIMEOUT,
            body_timeout: BODY_TIMEOUT,
            session_idle: SESSION_IDLE,
            max_delayed: MAX_DELAYED,
            unauthenticated_lifetime: UNAUTHENTICATED_LIFETIME,
        }
    }
}

/// 전역 `[mcp.http]` 의 값.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpSettings {
    pub enabled: bool,
    /// 들을 포트. 값이 틀렸으면 `None`(듣지 않는다).
    pub port: Option<u16>,
}

/// 전역 `[mcp]` 표에서 `[mcp.http]` 를 읽는다. **전역 설정만 받는다**(프로젝트 설정에는 이 표가 효과가 없다). `enabled` 가 불리언이
/// 아니면 꺼짐, `port` 가 1 ~ 65535 정수가 아니면 듣지 않는다 — 둘 다 경고 문장을 함께 돌려준다.
pub fn read_settings(
    global_mcp: Option<&Value>,
    default_port: u16,
) -> (HttpSettings, Option<String>) {
    let table = global_mcp.and_then(|mcp| mcp.get(TABLE));
    let mut warning = None;
    let enabled = match table.and_then(|t| t.get(ENABLED_KEY)) {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(v) => {
            warning = Some(format!(
                "mcp.http.enabled must be true or false, found {v}; the http endpoint stays off"
            ));
            false
        }
    };
    let port = match table.and_then(|t| t.get(PORT_KEY)) {
        None => Some(default_port),
        Some(v) => match v.as_u64().and_then(|n| u16::try_from(n).ok()) {
            Some(p) if p != 0 => Some(p),
            _ => {
                warning = Some(format!(
                    "mcp.http.port must be a port number from 1 to 65535, found {v}; the http endpoint does not listen"
                ));
                None
            }
        },
    };
    (HttpSettings { enabled, port }, warning)
}

/// 설정 창에 보이는 지금 상태(`mcp.settings.get` 의 `http`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct McpHttpStatus {
    /// `[mcp.http] enabled`.
    pub enabled: bool,
    /// 설정의 포트(없으면 기본). 값이 틀렸으면 `null`.
    pub port: Option<u16>,
    /// 클라이언트 설정에 넣는 주소(`http://127.0.0.1:<port>/mcp`). 포트가 틀렸으면 `null`.
    pub url: Option<String>,
    /// 지금 듣고 있는가.
    pub listening: bool,
    /// 켰는데 듣지 못한 까닭.
    pub problem: Option<McpHttpProblem>,
    /// 토큰 파일(`$STATE/mcp/http-token`).
    pub token_file: String,
}

/// 켰는데 듣지 못한 까닭(창이 번역한다).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum McpHttpProblem {
    /// 이 컴퓨터의 다른 소켓이 그 포트를 쥐고 있다(`127.0.0.1` 이나 `::1`, 와일드카드 포함).
    PortInUse,
    /// 그 밖의 바인드 실패(권한 등).
    BindFailed,
    /// `[mcp.http] port` 값이 틀렸다.
    InvalidPort,
    /// 토큰 파일을 읽거나 쓰지 못했다.
    TokenUnavailable,
}

/// [`Http::start`] 의 부품.
pub struct HttpParts {
    /// HTTP 세션의 부품 — `transport` 는 [`TRANSPORT`] 로 만든 것(소켓의 것과 같은 등록부 · 승인 허브 · 감사).
    pub services: Arc<Services>,
    pub tokens: TokenFile,
    pub default_port: u16,
    /// 인증을 지나 `initialize` 까지 마친 HTTP 세션을 함께 센다(Relay 의 유휴 판정 — 소켓의 것과 같은 것을 넣는다).
    pub connections: Connections,
    pub limits: Limits,
    /// 사람의 설정 창이 켜기 · 포트(`mcp.settings.set` 의 `httpEnabled` · `httpPort`)와 「토큰 새로 만들기」 를 할 수 있는가.
    /// Desktop 은 참, Relay 는 거짓 — Relay 의 HTTP 는 그 머신 `settings.toml` 을 손으로만 켜고 토큰도 그 머신에서만 바꾼다
    /// (18-m7-plan M7-25 메인 판정). 링크로 붙은 owner Desktop 도 바꾸지 못한다.
    pub settings_window: bool,
}

/// Streamable HTTP 의 켜고 끄기 · 토큰. 복제해도 같은 것이다.
#[derive(Clone)]
pub struct Http {
    inner: Arc<Inner>,
}

struct Inner {
    parts: HttpParts,
    state: Mutex<State>,
    /// 설정을 다시 읽어 맞추는 일을 한 번에 하나씩.
    apply: tokio::sync::Mutex<()>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct State {
    settings: Option<HttpSettings>,
    running: Option<Running>,
    problem: Option<McpHttpProblem>,
}

struct Running {
    port: u16,
    server: Arc<front::Server>,
    stop: CancellationToken,
    /// 듣는 태스크들 — 닫을 때 끝나기를 기다려 `apply` 가 돌아온 뒤에는 그 포트가 받지 않는다.
    listening: Vec<tokio::task::JoinHandle<()>>,
}

impl Http {
    /// 설정을 읽어 켜져 있으면 듣고, `config.changed` 마다 다시 맞춘다. `cancel` 이 취소되면 닫는다. tokio 런타임 안에서 부른다.
    pub fn start(parts: HttpParts, cancel: CancellationToken) -> Self {
        let mut events = parts
            .services
            .bus
            .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_CHANGED]));
        let http = Self {
            inner: Arc::new(Inner {
                parts,
                state: Mutex::new(State::default()),
                apply: tokio::sync::Mutex::new(()),
                cancel,
            }),
        };
        let watcher = http.clone();
        tokio::spawn(async move {
            watcher.apply().await;
            loop {
                // 켰는데 바인드하지 못했으면 다시 해 본다 — 앱을 다시 띄운 직후 그 포트에 남은 `TIME_WAIT`(이 끝점이 먼저 닫은
                // 연결)가 `SO_REUSEADDR` 없는 바인드를 잠시 막는다. 그 사이 상태는 `portInUse` 다.
                let retry = watcher.needs_retry();
                tokio::select! {
                    () = watcher.inner.cancel.cancelled() => break,
                    received = events.recv() => if received.is_none() { break },
                    () = tokio::time::sleep(RETRY_BIND), if retry => {}
                }
                watcher.apply().await;
            }
            watcher.stop_running().await;
        });
        http
    }

    /// 전역 설정을 지금 다시 읽어 맞춘다(열기 · 닫기 · 포트 바꾸기). 같은 값이고 듣고 있으면 아무것도 하지 않는다. 켰는데 듣지
    /// 못했으면 다시 해 본다.
    pub async fn apply(&self) {
        let _one = self.inner.apply.lock().await;
        if self.inner.cancel.is_cancelled() {
            return;
        }
        let global = self.inner.parts.services.global.snapshot();
        let (settings, warning) = read_settings(
            global.get("settings", crate::exposure::KEY),
            self.inner.parts.default_port,
        );
        if let Some(w) = &warning {
            tracing::warn!("{w}");
        }
        {
            let state = self.state();
            if state.settings == Some(settings) && (state.running.is_some() || !settings.enabled) {
                return;
            }
        }
        self.stop_running().await;
        let mut problem = None;
        let mut running = None;
        if settings.enabled {
            match settings.port {
                None => problem = Some(McpHttpProblem::InvalidPort),
                Some(port) => match self.open(port).await {
                    Ok(r) => running = Some(r),
                    Err(p) => problem = Some(p),
                },
            }
        }
        let mut state = self.state();
        state.settings = Some(settings);
        state.running = running;
        state.problem = problem;
    }

    async fn open(&self, port: u16) -> Result<Running, McpHttpProblem> {
        let tokens = self.inner.parts.tokens.clone();
        let token = tokio::task::spawn_blocking(move || tokens.load_or_create())
            .await
            .map_err(|_| McpHttpProblem::TokenUnavailable)?
            .map_err(|e| {
                tracing::warn!(%e, "cannot read or create the mcp http token; the http endpoint does not listen");
                McpHttpProblem::TokenUnavailable
            })?;
        let listeners = listen(port).await?;
        let stop = self.inner.cancel.child_token();
        let server = front::Server::new(
            Arc::clone(&self.inner.parts.services),
            token,
            port,
            self.inner.parts.connections.clone(),
            self.inner.parts.limits,
        );
        let listening = listeners
            .into_iter()
            .map(|listener| {
                tokio::spawn(front::accept_loop(
                    Arc::clone(&server),
                    listener,
                    stop.clone(),
                ))
            })
            .collect();
        tokio::spawn(front::reap_idle(Arc::clone(&server), stop.clone()));
        tracing::info!(port, "mcp http endpoint listening on loopback");
        Ok(Running {
            port,
            server,
            stop,
            listening,
        })
    }

    fn needs_retry(&self) -> bool {
        matches!(
            self.state().problem,
            Some(
                McpHttpProblem::PortInUse
                    | McpHttpProblem::BindFailed
                    | McpHttpProblem::TokenUnavailable
            )
        )
    }

    /// 듣기를 닫는다 — 듣는 태스크가 리스너를 놓을 때까지 기다린다(돌아온 뒤에는 그 포트에 연결이 붙지 않는다).
    async fn stop_running(&self) {
        let Some(running) = self.state().running.take() else {
            return;
        };
        running.stop.cancel();
        running.server.close_all();
        for task in running.listening {
            let _ = task.await;
        }
        tracing::info!(port = running.port, "mcp http endpoint closed");
    }

    /// 사람의 설정 창이 켜기 · 포트 · 토큰 새로 만들기를 할 수 있는가([`HttpParts::settings_window`]).
    pub fn settings_window(&self) -> bool {
        self.inner.parts.settings_window
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// 설정 창의 상태.
    pub fn status(&self) -> McpHttpStatus {
        let state = self.state();
        let settings = state.settings.unwrap_or(HttpSettings {
            enabled: false,
            port: Some(self.inner.parts.default_port),
        });
        McpHttpStatus {
            enabled: settings.enabled,
            port: settings.port,
            url: settings.port.map(url),
            listening: state.running.is_some(),
            problem: state.problem,
            token_file: self.inner.parts.tokens.path().display().to_string(),
        }
    }

    /// 지금 토큰 — 듣고 있으면 그 토큰, 아니면 파일의 것(한 번도 켜지 않았으면 `None`). 디스크 IO 를 할 수 있다.
    pub fn token(&self) -> io::Result<Option<Token>> {
        if let Some(running) = &self.state().running {
            return Ok(Some(running.server.token()));
        }
        self.inner.parts.tokens.read()
    }

    /// 「토큰 새로 만들기」: 파일을 바꾸고, 듣고 있으면 옛 토큰을 거절하고 열린 HTTP 세션을 모두 닫는다. 맞추기(`apply`)와 같은
    /// 잠금 아래서 한다 — 여는 도중 옛 토큰을 읽은 서버가 새 파일 뒤에 남지 않게.
    pub async fn regenerate(&self) -> io::Result<Token> {
        let _one = self.inner.apply.lock().await;
        let tokens = self.inner.parts.tokens.clone();
        let token = tokio::task::spawn_blocking(move || tokens.regenerate())
            .await
            .map_err(io::Error::other)??;
        if let Some(running) = &self.state().running {
            running.server.replace_token(token.clone());
        }
        tracing::info!("the mcp http token was replaced; open http sessions were closed");
        Ok(token)
    }

    /// 열린 HTTP 세션 수.
    pub fn sessions(&self) -> usize {
        self.state()
            .running
            .as_ref()
            .map_or(0, |r| r.server.sessions())
    }
}

/// 클라이언트 설정에 넣는 주소.
pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{PATH}")
}

/// `127.0.0.1:port` 와 `[::1]:port` 에 듣는다(`SO_REUSEADDR` 없이). `::1` 은 그 기기에 IPv6 가 없을 때만 건너뛴다. `port` 가 0
/// 이면(시험) `127.0.0.1` 이 고른 포트로 `::1` 도 듣는다.
async fn listen(port: u16) -> Result<Vec<TcpListener>, McpHttpProblem> {
    let v4 = bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))?;
    let port = v4
        .local_addr()
        .map_err(|_| McpHttpProblem::BindFailed)?
        .port();
    match bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)) {
        Ok(v6) => Ok(vec![v4, v6]),
        Err(McpHttpProblem::BindFailed) if !ipv6_loopback() => Ok(vec![v4]),
        Err(p) => Err(p),
    }
}

fn ipv6_loopback() -> bool {
    std::net::UdpSocket::bind((Ipv6Addr::LOCALHOST, 0)).is_ok()
}

fn bind(addr: SocketAddr) -> Result<TcpListener, McpHttpProblem> {
    let failed = |e: io::Error| {
        if e.kind() == io::ErrorKind::AddrInUse {
            tracing::warn!(
                port = addr.port(),
                "the mcp http port is in use on this computer"
            );
            McpHttpProblem::PortInUse
        } else {
            tracing::warn!(%e, %addr, "cannot listen for mcp http");
            McpHttpProblem::BindFailed
        }
    };
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .map_err(failed)?;
    socket.set_reuseaddr(false).map_err(failed)?;
    #[cfg(windows)]
    exclusive(&socket, addr).map_err(failed)?;
    socket.bind(addr).map_err(failed)?;
    socket.listen(128).map_err(failed)
}

/// Windows: 다른 소켓이 이 포트를 쥐었으면 `AddrInUse`, 아니면 `SO_EXCLUSIVEADDRUSE` 를 건다(남이 뒤에 더 좁은 주소로 들어오지 못하게).
///
/// `SO_EXCLUSIVEADDRUSE` 만으로는 앞서 쥔 소켓을 보지 못한다 — 다른 프로그램이 `0.0.0.0:P` 를 기본 옵션으로 쥐었어도 우리
/// `127.0.0.1:P` 바인드는 성공하고, 루프백 연결은 더 좁은 우리 쪽으로 온다(그 서버를 가림, M7-22 리뷰 F1 의 Windows 꼴 — CI run
/// 45316714445). 그래서 먼저 같은 가족의 와일드카드(`0.0.0.0:P` · `[::]:P`)에 배타 바인드를 해 본다: 그 포트를 어느 주소로든
/// 쥔 소켓이 있으면 실패한다(루프백이 아닌 주소로 쥔 것도 — 더 막는 쪽으로 틀린다). IPv4 를 들을 때는 `[::]:P` 도 떠본다 —
/// 두 가족 소켓이 IPv4 와일드카드 떠보기를 지나쳤다(CI run 45333303107). 그래서 IPv6 만 듣는 `[::]:P` 가 있어도 IPv4 를
/// 거절한다(더 막는 쪽). 떠본 소켓은 곧바로 닫는다. 포트 0(골라 받기)은 떠볼 포트가 없다.
#[cfg(windows)]
fn exclusive(socket: &TcpSocket, addr: SocketAddr) -> io::Result<()> {
    if addr.port() != 0 {
        let port = addr.port();
        if addr.is_ipv4() {
            probe(
                TcpSocket::new_v4()?,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
            )?;
            // 두 가족을 함께 듣는 `[::]:P` 소켓(`IPV6_V6ONLY` 끔 — Node 등의 기본)은 IPv4 루프백 연결도 받지만, IPv4 와일드카드
            // 떠보기에는 보이지 않는다(CI run 45333303107). 그래서 IPv6 와일드카드도 떠본다. IPv6 가 없는 기기에서는 소켓 ·
            // 바인드가 다른 오류로 실패한다 — 그때는 가릴 IPv6 소켓도 없으니 넘어간다.
            if let Ok(v6) = TcpSocket::new_v6() {
                match probe(
                    v6,
                    SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), port),
                ) {
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
                        ) =>
                    {
                        return Err(e);
                    }
                    _ => {}
                }
            }
        } else {
            probe(
                TcpSocket::new_v6()?,
                SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), port),
            )?;
        }
    }
    set_exclusive(socket)
}

/// 배타 바인드로 `any` 를 떠보고 곧 닫는다. 그 포트를 쥔 소켓이 있으면 `AddrInUse`(배타로 쥐었으면 `PermissionDenied`).
#[cfg(windows)]
fn probe(socket: TcpSocket, any: SocketAddr) -> io::Result<()> {
    set_exclusive(&socket)?;
    socket.bind(any)
}

#[cfg(windows)]
fn set_exclusive(socket: &TcpSocket) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        SO_EXCLUSIVEADDRUSE, SOCKET, SOL_SOCKET, setsockopt,
    };
    let on: i32 = 1;
    // SAFETY: 살아 있는 소켓 핸들과 길이가 맞는 4바이트 값을 넘긴다.
    let rc = unsafe {
        setsockopt(
            socket.as_raw_socket() as SOCKET,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&raw const on).cast(),
            std::mem::size_of::<i32>() as i32,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_are_off_by_default_and_fail_closed() {
        let off = HttpSettings {
            enabled: false,
            port: Some(DESKTOP_PORT),
        };
        assert_eq!(read_settings(None, DESKTOP_PORT), (off, None));
        assert_eq!(read_settings(Some(&json!({})), DESKTOP_PORT), (off, None));
        assert_eq!(
            read_settings(
                Some(&json!({"http": {"enabled": true, "port": 9000}})),
                DESKTOP_PORT
            ),
            (
                HttpSettings {
                    enabled: true,
                    port: Some(9000)
                },
                None
            )
        );
        let (s, w) = read_settings(Some(&json!({"http": {"enabled": "yes"}})), RELAY_PORT);
        assert!(!s.enabled && w.is_some());
        for bad in [json!(0), json!(70000), json!("80"), json!(-1)] {
            let (s, w) = read_settings(
                Some(&json!({"http": {"enabled": true, "port": bad}})),
                DESKTOP_PORT,
            );
            assert_eq!(s.port, None, "{bad}");
            assert!(w.is_some());
        }
    }

    /// 듣는 주소는 루프백뿐이다.
    #[tokio::test]
    async fn it_listens_on_loopback_only() {
        let listeners = listen(0).await.unwrap();
        assert!(!listeners.is_empty());
        for l in &listeners {
            assert!(l.local_addr().unwrap().ip().is_loopback());
        }
    }

    /// 이 컴퓨터의 다른 소켓이 와일드카드 · 같은 루프백으로 쥔 포트를 덮지 않는다(M7-22 리뷰 F1). `SO_REUSEADDR` 를 켜면 macOS
    /// 에서 이 시험이 실패한다. 한쪽(`::1`)만 쥐어도 `127.0.0.1` 을 듣지 않는다.
    #[tokio::test]
    async fn it_does_not_cover_a_port_another_socket_holds() {
        let mut held = vec!["0.0.0.0", "127.0.0.1"];
        if ipv6_loopback() {
            held.extend(["::", "::1"]);
        }
        for addr in held {
            let holder = std::net::TcpListener::bind((addr, 0)).unwrap();
            let port = holder.local_addr().unwrap().port();
            assert_eq!(
                listen(port).await.map(|_| ()).unwrap_err(),
                McpHttpProblem::PortInUse,
                "{addr} held"
            );
            // 아무것도 남기지 않았다 — 쥔 쪽을 놓으면 그 포트로 들을 수 있다.
            drop(holder);
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                assert!(listen(port).await.is_ok(), "{addr} released");
            }
        }
    }
}
