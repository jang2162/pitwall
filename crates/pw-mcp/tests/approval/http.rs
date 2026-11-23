//! Streamable HTTP(18-m7-plan M7-25): 진짜 hyper 끝점 · 진짜 세션(`pw_mcp::serve`) · 진짜 Host · 감사 파일에, **손으로 짠 HTTP/1.1
//! 바이트**로 붙는다 — 같은 기기의 다른 프로세스(토큰 없음 · 틀린 토큰), 브라우저 페이지(`Origin` · `Host` · 단순 요청 ·
//! 프리플라이트), 에이전트(토큰 있음)의 자리. 머리를 그대로 보내려고 HTTP 클라이언트 크레이트를 쓰지 않는다.

use std::sync::OnceLock;
use std::time::Instant;

use pw_mcp::http::{self, Http, HttpParts, Limits, TokenFile};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::*;

// ---- 로그 모으기: 이 시험 바이너리의 모든 tracing 기록(모든 수준)을 한 글에 모은다 — 토큰이 들어가지 않는지 본다 ----

static LOG: OnceLock<Arc<std::sync::Mutex<String>>> = OnceLock::new();

fn log() -> Arc<std::sync::Mutex<String>> {
    Arc::clone(LOG.get_or_init(|| {
        let buffer = Arc::new(std::sync::Mutex::new(String::new()));
        let _ = tracing::subscriber::set_global_default(Capture(Arc::clone(&buffer)));
        buffer
    }))
}

struct Capture(Arc<std::sync::Mutex<String>>);

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut out = self.0.lock().unwrap();
        span.record(&mut Fields(&mut out));
        out.push('\n');
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut out = self.0.lock().unwrap();
        values.record(&mut Fields(&mut out));
        out.push('\n');
    }
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut out = self.0.lock().unwrap();
        out.push_str(event.metadata().target());
        out.push(' ');
        event.record(&mut Fields(&mut out));
        out.push('\n');
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

// ---- 끝점 ----

/// 이 기기에서 비어 있는 포트(`127.0.0.1` 과, 있으면 `::1` 둘 다).
fn free_port() -> u16 {
    loop {
        let v4 = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = v4.local_addr().unwrap().port();
        let v6_ok = std::net::TcpListener::bind(("::1", port)).is_ok()
            || std::net::TcpListener::bind(("::1", 0)).is_err();
        if v6_ok {
            return port;
        }
    }
}

fn http_global(enabled: bool, port: u16) -> String {
    format!("[mcp.http]\nenabled = {enabled}\nport = {port}\n")
}

struct Endpoint {
    http: Http,
    port: u16,
    stop: CancellationToken,
    connections: pw_mcp::Connections,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Fixture {
    fn http_services(&self) -> Arc<Services> {
        let router = Arc::new(HostRouter::new());
        router.add_host(self.host.clone()).unwrap();
        Arc::new(Services {
            router,
            host_id: "local".into(),
            paths: self.paths.clone(),
            global: self.host.config().unwrap().global.clone(),
            bus: self.host.bus().clone(),
            audit: Arc::new(AuditLog::under_state(&self.paths.state)),
            catalog: Arc::new(fake_catalog().routed_by(self.host.router())),
            approval: Approval::for_host(HostKind::App, &self.sessions),
            transport: http::TRANSPORT,
            host_kind: HostKind::App,
            sessions: self.sessions.clone(),
            machines: None,
        })
    }

    /// 끝점을 띄운다(설정의 `[mcp.http]` 를 읽는다). `port` 는 설정에 쓴 포트(기본 포트로도 쓴다).
    async fn endpoint(&self, port: u16, limits: Limits) -> Endpoint {
        let stop = CancellationToken::new();
        let connections = pw_mcp::Connections::default();
        let http = Http::start(
            HttpParts {
                services: self.http_services(),
                tokens: TokenFile::under_state(&self.paths.state),
                default_port: port,
                connections: connections.clone(),
                limits,
                settings_window: true,
            },
            stop.clone(),
        );
        http.apply().await;
        Endpoint {
            http,
            port,
            stop,
            connections,
        }
    }

    /// 전역 설정의 `[mcp.http]` 를 바꾸고 다시 읽는다.
    fn set_http(&self, enabled: bool, port: u16) {
        std::fs::write(
            self.paths.config.join("settings.toml"),
            format!("schema = 1\n{}{TOOLS}", http_global(enabled, port)),
        )
        .unwrap();
        self.host.config().unwrap().global.reload().unwrap();
    }
}

impl Endpoint {
    fn token(&self) -> String {
        self.http.token().unwrap().unwrap().reveal().to_owned()
    }

    fn client(&self) -> Client {
        Client {
            port: self.port,
            token: self.token(),
            session: None,
        }
    }
}

// ---- 손으로 짠 HTTP/1.1 ----

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }

    fn all(&self) -> String {
        format!("{:?}\n{}", self.headers, self.body)
    }
}

fn request(method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("{method} {path} HTTP/1.1\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || method == "POST" {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// 한 연결에 보내고 응답 하나(`Content-Length` 만큼)를 읽은 뒤 **이쪽이 먼저** 닫는다 — 서버 쪽 포트에 `TIME_WAIT` 를 남기지 않는다
/// (그러면 다시 띄운 끝점이 곧바로 같은 포트에 바인드된다). 응답 전에 닫히면 `None`.
async fn exchange(port: u16, bytes: &[u8]) -> Option<Reply> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    stream.write_all(bytes).await.ok()?;
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        if let Some(reply) = complete(&raw) {
            return Some(reply);
        }
        let n = within(stream.read(&mut buf)).await.ok()?;
        if n == 0 {
            return complete(&raw).or_else(|| parse(&raw));
        }
        raw.extend_from_slice(&buf[..n]);
    }
}

/// 머리와 `Content-Length` 만큼의 본문이 다 왔으면 그 응답.
fn complete(raw: &[u8]) -> Option<Reply> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let reply = parse(raw)?;
    let length: usize = reply.header("content-length")?.parse().ok()?;
    (raw.len() >= end + length).then_some(reply)
}

fn parse(raw: &[u8]) -> Option<Reply> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Some(Reply {
        status,
        headers,
        body: body.to_owned(),
    })
}

struct Client {
    port: u16,
    token: String,
    session: Option<String>,
}

impl Client {
    fn headers(&self) -> Vec<(String, String)> {
        let mut h = vec![
            ("Host".into(), format!("127.0.0.1:{}", self.port)),
            ("Authorization".into(), format!("Bearer {}", self.token)),
            ("Content-Type".into(), "application/json".into()),
            (
                "Accept".into(),
                "application/json, text/event-stream".into(),
            ),
        ];
        if let Some(s) = &self.session {
            h.push(("Mcp-Session-Id".into(), s.clone()));
        }
        h
    }

    async fn send(&self, method: &str, headers: Vec<(String, String)>, body: &[u8]) -> Reply {
        exchange(self.port, &request(method, http::PATH, &headers, body))
            .await
            .expect("no reply")
    }

    async fn post_with(&self, headers: Vec<(String, String)>, body: &Value) -> Reply {
        self.send("POST", headers, body.to_string().as_bytes())
            .await
    }

    async fn post(&self, body: &Value) -> Reply {
        self.post_with(self.headers(), body).await
    }

    /// `initialize` · `notifications/initialized` 를 마치고 세션 id 를 둔다.
    async fn initialize(&mut self) -> Reply {
        let reply = self
            .post(&json!({
                "jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "http-agent", "version": "1"},
                },
            }))
            .await;
        assert_eq!(reply.status, 200, "{}", reply.all());
        self.session = Some(
            reply
                .header("mcp-session-id")
                .expect("no session id")
                .to_owned(),
        );
        let initialized = self
            .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        assert_eq!(initialized.status, 202, "{}", initialized.all());
        reply
    }

    async fn call(&self, id: u64, tool: &str, args: Value) -> Reply {
        self.post(&json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": tool, "arguments": args},
        }))
        .await
    }

    fn with_token(&self, token: &str) -> Self {
        Self {
            port: self.port,
            token: token.to_owned(),
            session: self.session.clone(),
        }
    }
}

fn http_mark(reply: &Reply) -> Mark {
    let result = &reply.json()["result"];
    Mark::read(result["_meta"].as_object().expect("no _meta")).expect("no mark")
}

fn no_cors(reply: &Reply) {
    assert!(
        !reply
            .headers
            .iter()
            .any(|(k, _)| k.to_ascii_lowercase().starts_with("access-control-")),
        "CORS 머리를 내지 않는다: {:?}",
        reply.headers
    );
}

// ---- 시험 ----

/// ④ 기본 꺼짐: 설정을 켜지 않으면 듣는 소켓이 없다(`127.0.0.1` · `::1` 둘 다). 켜면 듣고, 끄면 다시 닫는다. 토큰도 켤 때 만든다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_listens_until_it_is_enabled() {
    let port = free_port();
    let f = Fixture::new();
    let e = f.endpoint(port, Limits::default()).await;
    let refused = |port| async move {
        let v4 = TcpStream::connect(("127.0.0.1", port)).await.is_err();
        let v6 = TcpStream::connect(("::1", port)).await.is_err();
        v4 && v6
    };
    assert!(refused(port).await, "기본은 듣지 않는다");
    let status = e.http.status();
    assert!(!status.enabled && !status.listening);
    assert!(
        !TokenFile::under_state(&f.paths.state).path().exists(),
        "켜기 전에는 토큰도 만들지 않는다"
    );

    f.set_http(true, port);
    e.http.apply().await;
    assert!(e.http.status().listening);
    assert_eq!(
        e.http.status().url.as_deref(),
        Some(format!("http://127.0.0.1:{port}/mcp").as_str())
    );
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_ok());

    f.set_http(false, port);
    e.http.apply().await;
    assert!(!e.http.status().listening);
    assert!(refused(port).await, "끄면 닫는다");
}

/// 설정을 바꾸면(`config.changed`) 부르지 않아도 따라간다 — 설정 창이 키를 쓰는 길.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn it_follows_the_settings_on_its_own() {
    let port = free_port();
    let f = Fixture::new();
    let e = f.endpoint(port, Limits::default()).await;
    f.set_http(true, port);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !e.http.status().listening {
        assert!(Instant::now() < deadline, "did not start listening");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// ① 토큰 없음 · 틀린 토큰 · 다른 꼴(Basic · 질의 글)은 401 이고 툴이 돌지 않는다(계획도 만들지 않는다). 맞는 토큰이면 같은 호출이
/// 돈다(대조).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_token_or_a_wrong_one_is_401_and_nothing_runs() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    let mut agent = e.client();
    agent.initialize().await;
    let echo = json!({"projectId": f.alpha, "tag": "x"});
    let write = json!({"projectId": f.alpha, "tag": "w"});

    let mut without = agent.headers();
    without.retain(|(k, _)| k != "Authorization");
    let token = e.token();
    let mut near = token.clone().into_bytes();
    near[0] = if near[0] == b'a' { b'b' } else { b'a' };
    let near = String::from_utf8(near).unwrap();
    let call = |id: u64, tool: &str, args: &Value| json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool, "arguments": args}});
    let variants: Vec<(&str, Vec<(String, String)>)> = vec![
        ("none", without.clone()),
        ("wrong", agent.with_token(&near).headers()),
        ("short", agent.with_token(&token[..32]).headers()),
        ("basic", {
            let mut h = without.clone();
            h.push(("Authorization".into(), format!("Basic {token}")));
            h
        }),
        ("twice", {
            let mut h = agent.headers();
            h.push(("Authorization".into(), "Bearer nope".into()));
            h
        }),
    ];
    for (i, (name, headers)) in variants.into_iter().enumerate() {
        for tool in ["fake_echo", "fake_write"] {
            let args = if tool == "fake_echo" { &echo } else { &write };
            let reply = agent
                .post_with(headers.clone(), &call(i as u64 + 10, tool, args))
                .await;
            assert_eq!(reply.status, 401, "{name} {tool}: {}", reply.all());
            assert_eq!(reply.header("www-authenticate"), Some("Bearer"));
            assert!(!reply.all().contains(&token), "{name}");
        }
    }
    // 질의 글의 토큰은 받지 않는다.
    let reply = exchange(
        port,
        &request(
            "POST",
            &format!("/mcp?token={token}"),
            &without,
            call(30, "fake_echo", &echo).to_string().as_bytes(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(reply.status, 401);
    // 토큰 없이는 세션도 열리지 않는다.
    let mut stranger = agent.with_token("");
    stranger.session = None;
    let mut no_auth = stranger.headers();
    no_auth.retain(|(k, _)| k != "Authorization");
    let opened = stranger
        .post_with(
            no_auth,
            &json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "x", "version": "1"}}}),
        )
        .await;
    assert_eq!(opened.status, 401);
    assert_eq!(e.http.sessions(), 1);
    assert_eq!((f.runs(), f.plans()), (0, 0), "툴도 계획도 돌지 않았다");
    assert!(f.rows(0).is_empty(), "감사 행도 없다 — 세션에 닿지 않았다");

    // 대조: 맞는 토큰.
    let ok = agent.call(40, "fake_echo", echo.clone()).await;
    assert_eq!(ok.status, 200, "{}", ok.all());
    assert_eq!(http_mark(&ok), Mark::ok());
}

/// ② 브라우저: 다른 `Origin`(그리고 `null` · 다른 포트 · `https`) · 다른 `Host`(DNS rebinding) · `Sec-Fetch-Site: cross-site` 는 맞는
/// 토큰이어도 403 이고 툴이 돌지 않는다. CORS 머리는 어느 응답에도 없고 프리플라이트는 405. 폼 · `text/plain` 단순 요청은 415.
/// 같은 끝점의 `Origin` 은 받는다(대조).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_origins_hosts_and_simple_requests_are_refused() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    let mut agent = e.client();
    agent.initialize().await;
    let call = json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {"name": "fake_echo", "arguments": {"projectId": f.alpha}}});
    let with = |extra: &[(&str, String)], replace_host: Option<String>| {
        let mut h = agent.headers();
        if let Some(host) = replace_host {
            h.retain(|(k, _)| k != "Host");
            h.push(("Host".into(), host));
        }
        for (k, v) in extra {
            h.push(((*k).to_owned(), v.clone()));
        }
        h
    };
    let forbidden: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "evil origin",
            with(&[("Origin", "http://evil.example".into())], None),
        ),
        ("null origin", with(&[("Origin", "null".into())], None)),
        (
            "other port",
            with(&[("Origin", "http://127.0.0.1:3000".into())], None),
        ),
        (
            "https",
            with(&[("Origin", format!("https://127.0.0.1:{port}"))], None),
        ),
        ("rebinding", with(&[], Some(format!("evil.example:{port}")))),
        (
            "rebinding with origin",
            with(
                &[("Origin", format!("http://evil.example:{port}"))],
                Some(format!("evil.example:{port}")),
            ),
        ),
        ("no port", with(&[], Some("localhost".into()))),
        (
            "cross-site",
            with(&[("Sec-Fetch-Site", "cross-site".into())], None),
        ),
    ];
    for (name, headers) in forbidden {
        let reply = agent.post_with(headers, &call).await;
        assert_eq!(reply.status, 403, "{name}: {}", reply.all());
        no_cors(&reply);
    }
    // 프리플라이트: 허락하지 않는다.
    let preflight = agent
        .send(
            "OPTIONS",
            with(
                &[
                    ("Origin", format!("http://localhost:{port}")),
                    ("Access-Control-Request-Method", "POST".into()),
                    (
                        "Access-Control-Request-Headers",
                        "authorization, content-type".into(),
                    ),
                ],
                None,
            ),
            b"",
        )
        .await;
    assert_eq!(preflight.status, 405);
    no_cors(&preflight);
    // 단순 요청(폼 · text/plain)으로는 툴에 닿지 않는다.
    for kind in [
        "application/x-www-form-urlencoded",
        "text/plain",
        "multipart/form-data; boundary=x",
    ] {
        let mut h = agent.headers();
        h.retain(|(k, _)| k != "Content-Type");
        h.push(("Content-Type".into(), kind.into()));
        let reply = agent.post_with(h, &call).await;
        assert_eq!(reply.status, 415, "{kind}");
    }
    assert_eq!(f.rows(0).len(), 0, "툴에 닿지 않았다");
    // 대조: 이 끝점의 Origin · localhost Host 는 받는다.
    let same = agent
        .post_with(
            with(
                &[("Origin", format!("http://localhost:{port}"))],
                Some(format!("localhost:{port}")),
            ),
            &call,
        )
        .await;
    assert_eq!(same.status, 200, "{}", same.all());
    no_cors(&same);
}

/// ⑥ 앱을 다시 띄워도 토큰이 같다(클라이언트 설정을 고치지 않는다). 새로 만들면 옛 토큰은 401, 옛 세션은 새 토큰으로도 404 — 끊긴다.
/// 새 토큰으로 새 세션은 열린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_token_survives_a_restart_and_regenerating_cuts_the_old_one_and_its_sessions() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let first = f.endpoint(port, Limits::default()).await;
    let token = first.token();
    let mut agent = first.client();
    agent.initialize().await;
    drop(first);
    // 같은 자리에 다시 띄운다(앱 재시작).
    let deadline = Instant::now() + Duration::from_secs(10);
    let second = loop {
        let e = f.endpoint(port, Limits::default()).await;
        if e.http.status().listening {
            break e;
        }
        assert!(Instant::now() < deadline, "the port was not released");
        drop(e);
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(second.token(), token, "다시 떠도 같은 토큰");
    let mut again = second.client();
    again.initialize().await;
    assert_eq!(
        again
            .call(1, "fake_echo", json!({"projectId": f.alpha}))
            .await
            .status,
        200
    );

    let fresh = second.http.regenerate().await.unwrap();
    assert_ne!(fresh.reveal(), token);
    assert_eq!(second.token(), fresh.reveal());
    let old = again
        .call(2, "fake_echo", json!({"projectId": f.alpha}))
        .await;
    assert_eq!(old.status, 401, "옛 토큰은 거절");
    let hijack = again
        .with_token(fresh.reveal())
        .call(3, "fake_echo", json!({"projectId": f.alpha}))
        .await;
    assert_eq!(hijack.status, 404, "옛 세션은 끊겼다: {}", hijack.all());
    assert_eq!(second.http.sessions(), 0);
    let mut renewed = again.with_token(fresh.reveal());
    renewed.session = None;
    renewed.initialize().await;
    assert_eq!(
        renewed
            .call(4, "fake_echo", json!({"projectId": f.alpha}))
            .await
            .status,
        200
    );
    // 파일도 새 토큰이다(다음 시작이 읽는다).
    assert_eq!(
        TokenFile::under_state(&f.paths.state)
            .read()
            .unwrap()
            .unwrap(),
        fresh
    );
}

/// ⑦ 틀린 토큰이 잇따르면 늦춘다(처음 세 번은 늦추지 않는다). 늦추는 동안에도 맞는 토큰의 요청은 늦지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_wrong_tokens_are_slowed() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    let mut agent = e.client();
    agent.initialize().await;
    let wrong = agent.with_token(&"0".repeat(64));
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    for _ in 0..3 {
        let started = Instant::now();
        assert_eq!(wrong.post(&ping).await.status, 401);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "처음 몇 번은 늦추지 않는다"
        );
    }
    let started = Instant::now();
    assert_eq!(wrong.post(&ping).await.status, 401);
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "넷째부터 늦춘다"
    );
    let started = Instant::now();
    assert_eq!(wrong.post(&ping).await.status, 401);
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "잇따르면 더 늦춘다"
    );
    // 늦추는 중인 실패와 나란히 보낸 맞는 요청은 늦지 않는다.
    let slow = tokio::spawn({
        let wrong = wrong.with_token(&"0".repeat(64));
        let ping = ping.clone();
        async move { wrong.post(&ping).await.status }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    assert_eq!(agent.post(&ping).await.status, 200);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(slow.await.unwrap(), 401);
}

/// ⑤ 토큰이 로그(모든 수준) · 감사 파일 · 에이전트가 받는 글(응답 · 오류)에 남지 않는다 — 켜기 · 틀린 토큰 · 맞는 호출 · 쓰기 거절 ·
/// 새로 만들기를 다 지난 뒤 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_token_is_not_in_logs_the_audit_or_replies() {
    let log = log();
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    let token = e.token();
    let mut agent = e.client();
    let mut replies = vec![agent.initialize().await];
    replies.push(
        agent
            .call(
                1,
                "fake_echo",
                json!({"projectId": f.alpha, "tag": token[..20]}),
            )
            .await,
    );
    let mut window = f.window(&f.alpha);
    let pending = tokio::spawn({
        let agent = agent.with_token(&token);
        let alpha = f.alpha.clone();
        async move {
            agent
                .call(2, "fake_write", json!({"projectId": alpha, "tag": "w"}))
                .await
        }
    });
    let asked = window.opened().await;
    window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    replies.push(within(pending).await.unwrap());
    replies.push(
        agent
            .with_token(&token[..63])
            .call(3, "fake_echo", json!({"projectId": f.alpha}))
            .await,
    );
    replies.push(agent.call(4, "no_such_tool", json!({})).await);
    let old = token.clone();
    let fresh = e.http.regenerate().await.unwrap();
    replies.push(
        agent
            .call(5, "fake_echo", json!({"projectId": f.alpha}))
            .await,
    );
    f.rows(3);
    for reply in &replies {
        assert!(!reply.all().contains(&old), "{}", reply.all());
    }
    let audit = std::fs::read(f.paths.state.join("mcp/audit.db")).unwrap();
    for t in [old.as_str(), fresh.reveal()] {
        assert!(
            !audit.windows(t.len()).any(|w| w == t.as_bytes()),
            "감사 파일에 토큰이 없다"
        );
        let logged = log.lock().unwrap();
        assert!(!logged.is_empty(), "로그를 모으고 있다");
        assert!(!logged.contains(t), "로그에 토큰이 없다");
    }
}

/// HTTP 세션의 판정은 소켓 · stdio 와 같은 `Session` 이다: 쓰기 툴은 그 프로젝트 창에 묻고(거절이면 돌지 않음), 허용하면 한 번 돈다.
/// 감사 행의 전송은 `http`, 세션 목록의 전송도 `http`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_http_session_is_judged_like_any_session_and_audited_as_http() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    let mut agent = e.client();
    agent.initialize().await;
    let mut window = f.window(&f.alpha);
    let call = |id| {
        let agent = agent.with_token(&agent.token);
        let alpha = f.alpha.clone();
        tokio::spawn(async move {
            agent
                .call(id, "fake_write", json!({"projectId": alpha, "tag": "h"}))
                .await
        })
    };
    let denied = call(1);
    let request = window.opened().await;
    assert_eq!(request.tool, "fake_write");
    window.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    window.closed(request.id).await;
    let denied = within(denied).await.unwrap();
    assert_eq!(http_mark(&denied), Mark::denied(Reason::ApprovalDenied));
    assert_eq!(f.runs(), 0);

    let allowed = call(2);
    let request = window.opened().await;
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    assert_eq!(http_mark(&within(allowed).await.unwrap()), Mark::ok());
    assert_eq!(f.runs(), 1);

    let transports: Vec<Option<String>> = {
        let conn = rusqlite::Connection::open(f.paths.state.join("mcp/audit.db")).unwrap();
        f.rows(2);
        let mut stmt = conn
            .prepare("SELECT transport FROM calls ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert!(!transports.is_empty());
    assert!(
        transports.iter().all(|t| t.as_deref() == Some("http")),
        "{transports:?}"
    );
    let listed = f.sessions.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].transport, "http");
    assert_eq!(e.connections.active(), 1, "인증을 지난 세션만 센다");
}

/// 세션: 세션 id 는 토큰 없이 쓸 수 없고(401), 모르는 id 는 404, `DELETE` 로 닫힌 세션도 404. 세션 없는 요청(2026-07-28 꼴)은
/// 400 이고 묶음(배열)도 400. 세션 수 상한을 넘는 `initialize` 는 503. SSE 스트림은 세션마다 하나(둘째는 409).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_need_the_token_and_their_limits_hold() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let limits = Limits {
        max_sessions: 2,
        ..Limits::default()
    };
    let e = f.endpoint(port, limits).await;
    let mut agent = e.client();
    agent.initialize().await;
    let ping = json!({"jsonrpc": "2.0", "id": 9, "method": "ping"});
    // 다른 프로세스가 세션 id 를 알아도 토큰 없이는 쓸 수 없다.
    let mut thief = agent.headers();
    thief.retain(|(k, _)| k != "Authorization");
    assert_eq!(agent.post_with(thief, &ping).await.status, 401);
    let mut unknown = agent.with_token(&agent.token);
    unknown.session = Some("0".repeat(64));
    assert_eq!(unknown.post(&ping).await.status, 404);
    // 세션 없는 요청 · 묶음.
    let mut sessionless = agent.with_token(&agent.token);
    sessionless.session = None;
    let reply = sessionless.post(&ping).await;
    assert_eq!(reply.status, 400);
    assert!(reply.body.contains("stdio"), "{}", reply.body);
    assert_eq!(agent.post(&json!([ping.clone()])).await.status, 400);
    // 스트림은 하나.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut get = agent.headers();
    get.retain(|(k, _)| k != "Content-Type");
    stream
        .write_all(&request("GET", http::PATH, &get, b""))
        .await
        .unwrap();
    let mut first = [0u8; 256];
    let n = within(stream.read(&mut first)).await.unwrap();
    let head = String::from_utf8_lossy(&first[..n]);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.to_ascii_lowercase().contains("text/event-stream"));
    let second = agent.send("GET", get.clone(), b"").await;
    assert_eq!(second.status, 409);
    drop(stream);
    // 세션 수 상한.
    let mut two = agent.with_token(&agent.token);
    two.session = None;
    two.initialize().await;
    let mut three = agent.with_token(&agent.token);
    three.session = None;
    let full = three
        .post(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "x", "version": "1"}}}))
        .await;
    assert_eq!(full.status, 503);
    // 닫기.
    let deleted = agent.send("DELETE", agent.headers(), b"").await;
    assert_eq!(deleted.status, 200);
    assert_eq!(agent.post(&ping).await.status, 404);
    three.initialize().await;
}

/// 앞단의 상한: 본문(413) · 머리(431) · 느린 머리(시한에 닫힘) · 느린 본문(408) · 동시 연결 수(넘는 연결은 받자마자 닫힘).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_front_bounds_bodies_headers_slow_requests_and_connections() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let limits = Limits {
        max_body: 1024,
        header_timeout: Duration::from_millis(300),
        body_timeout: Duration::from_millis(300),
        ..Limits::default()
    };
    let e = f.endpoint(port, limits).await;
    let mut agent = e.client();
    agent.initialize().await;
    // 본문.
    let big =
        json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {"pad": "x".repeat(2048)}});
    assert_eq!(agent.post(&big).await.status, 413);
    // 길이를 숨긴(chunked) 큰 본문도.
    let mut chunked = String::from("POST /mcp HTTP/1.1\r\n");
    for (k, v) in agent.headers() {
        chunked.push_str(&format!("{k}: {v}\r\n"));
    }
    let payload = big.to_string();
    chunked.push_str(&format!(
        "Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{payload}\r\n0\r\n\r\n",
        payload.len()
    ));
    assert_eq!(
        exchange(port, chunked.as_bytes()).await.unwrap().status,
        413
    );
    // 머리.
    let mut fat = agent.headers();
    fat.push(("X-Pad".into(), "y".repeat(http::MAX_HEADER + 1)));
    let reply = exchange(port, &request("POST", http::PATH, &fat, b"{}")).await;
    assert!(reply.as_ref().is_none_or(|r| r.status == 431), "{reply:?}");
    // 느린 머리: 시한 뒤 닫힌다.
    let mut slow = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    slow.write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n")
        .await
        .unwrap();
    let started = Instant::now();
    let mut buf = Vec::new();
    within(slow.read_to_end(&mut buf)).await.ok();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "머리 시한에 닫혔다"
    );
    // 느린 본문: 408.
    let mut slow = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut head = String::from("POST /mcp HTTP/1.1\r\n");
    for (k, v) in agent.headers() {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("Content-Length: 100\r\nConnection: close\r\n\r\n{\"jsonrpc\"");
    slow.write_all(head.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    within(slow.read_to_end(&mut raw)).await.unwrap();
    assert_eq!(parse(&raw).unwrap().status, 408);
    // 동시 연결 수.
    let mut held = Vec::new();
    for _ in 0..http::MAX_CONNECTIONS {
        held.push(TcpStream::connect(("127.0.0.1", port)).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ping = json!({"jsonrpc": "2.0", "id": 2, "method": "ping"});
    let over = exchange(
        port,
        &request(
            "POST",
            http::PATH,
            &agent.headers(),
            ping.to_string().as_bytes(),
        ),
    )
    .await;
    assert!(over.is_none(), "상한을 넘는 연결은 닫힌다: {over:?}");
    drop(held);
    // 머리 시한이 놀고 있던 연결을 닫아 자리가 돌아온다.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(agent.post(&ping).await.status, 200);
}

/// 서버가 먼저 보내는 것(`tools/list_changed`)은 `GET` 의 SSE 스트림으로 온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_list_changes_arrive_on_the_stream() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    let mut agent = e.client();
    agent.initialize().await;
    let listed = agent
        .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .await;
    assert_eq!(listed.status, 200);
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut get = agent.headers();
    get.retain(|(k, _)| k != "Content-Type");
    stream
        .write_all(&request("GET", http::PATH, &get, b""))
        .await
        .unwrap();
    let mut seen = Vec::new();
    let mut buf = [0u8; 1024];
    let n = within(stream.read(&mut buf)).await.unwrap();
    seen.extend_from_slice(&buf[..n]);
    // 그룹 하나를 끈다 — 보이는 툴이 바뀐다.
    std::fs::write(
        f.paths.config.join("settings.toml"),
        format!(
            "schema = 1\n{}[mcp.groups]\nproject = false\n{TOOLS}",
            http_global(true, port)
        ),
    )
    .unwrap();
    f.host.config().unwrap().global.reload().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&seen).contains("notifications/tools/list_changed") {
        assert!(
            Instant::now() < deadline,
            "{}",
            String::from_utf8_lossy(&seen)
        );
        let n = within(stream.read(&mut buf)).await.unwrap();
        assert!(n > 0, "stream closed");
        seen.extend_from_slice(&buf[..n]);
    }
    assert!(
        e.http.status().listening,
        "설정을 다시 읽어도 같은 값이면 닫지 않는다"
    );
}

/// 토큰 RPC 의 문: 설정 · 웰컴 창(Host 범위)만 읽고 새로 만든다. 프로젝트 · 파일 창, 에이전트 범위, owner 아닌 role 은 거절.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_settings_window_reads_or_replaces_the_token() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    f.sessions.attach_http(e.http.clone());
    let token = e.token();
    let connect = |role: Role, scope: ConnectionScope| {
        let host = Arc::clone(&f.host);
        async move {
            let caller = Caller {
                role,
                scope,
                window: Some("w".into()),
            };
            Rpc(pw_core::Host::connect(&*host, caller).await.unwrap(), 0)
        }
    };
    let mut settings = connect(Role::Owner, ConnectionScope::Host).await;
    let read = settings.call("mcp.http.token", json!(null)).await;
    assert_eq!(read["result"]["token"], json!(token), "{read}");
    let view = settings.call("mcp.settings.get", json!({})).await;
    assert_eq!(view["result"]["http"]["listening"], json!(true), "{view}");
    assert!(
        !view.to_string().contains(&token),
        "설정 보기에는 토큰이 없다"
    );

    let refused = [
        (Role::Owner, ConnectionScope::Project(f.alpha.clone())),
        (Role::Owner, ConnectionScope::Agent),
        (Role::Readonly, ConnectionScope::Host),
        (Role::Mobile, ConnectionScope::Host),
    ];
    for (role, scope) in refused {
        let label = format!("{role:?} {scope:?}");
        let mut rpc = connect(role, scope).await;
        for method in ["mcp.http.token", "mcp.http.regenerateToken"] {
            let reply = rpc.call(method, json!(null)).await;
            assert_eq!(
                reply["error"]["code"],
                json!(pw_rpc::code::PERMISSION_DENIED),
                "{label} {method}: {reply}"
            );
            assert!(!reply.to_string().contains(&token));
        }
    }
    assert_eq!(e.token(), token, "거절된 다시 만들기는 바꾸지 않았다");
    let replaced = settings.call("mcp.http.regenerateToken", json!(null)).await;
    let fresh = replaced["result"]["token"].as_str().unwrap().to_owned();
    assert_ne!(fresh, token);
    assert_eq!(e.token(), fresh);
}

/// 설정 창이 켜기 · 포트를 쓰면(`mcp.settings.set`) 곧바로 맞춰 그 결과를 돌려준다. 잘못된 포트는 쓰지 않는다. 이 컴퓨터의 다른
/// 소켓이 쥔 포트면 듣지 않고 `portInUse`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_settings_window_turns_it_on_and_sees_a_busy_port() {
    let port = free_port();
    let f = Fixture::new();
    let e = f.endpoint(port, Limits::default()).await;
    f.sessions.attach_http(e.http.clone());
    let mut settings = Rpc::connect(&f, ConnectionScope::Host).await;
    let set = |key: &str, value: Value| json!({"key": key, "value": value});
    let view = settings
        .call("mcp.settings.set", set("httpEnabled", json!(true)))
        .await;
    assert_eq!(view["result"]["http"]["listening"], json!(true), "{view}");
    let bad = settings
        .call("mcp.settings.set", set("httpPort", json!(0)))
        .await;
    assert_eq!(bad["error"]["data"]["reason"], json!("badPort"), "{bad}");
    let holder = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
    let busy = holder.local_addr().unwrap().port();
    let view = settings
        .call("mcp.settings.set", set("httpPort", json!(busy)))
        .await;
    assert_eq!(view["result"]["http"]["listening"], json!(false), "{view}");
    assert_eq!(view["result"]["http"]["problem"], json!("portInUse"));
    assert!(
        !std::fs::read_to_string(f.paths.config.join("settings.toml"))
            .unwrap()
            .contains("port = 0")
    );
}

/// 켰는데 포트가 쓰이는 중이면 듣지 않고(`portInUse`), 그 포트가 풀리면 설정을 다시 건드리지 않아도 듣기 시작한다(앱 재시작 직후의
/// `TIME_WAIT` 자리).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_port_is_retried_until_it_frees() {
    let holder = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = holder.local_addr().unwrap().port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    // 감시 태스크의 첫 맞추기도 끝나게 둔다 — 그 뒤로는 재시도만이 다시 바인드한다(설정은 건드리지 않는다).
    tokio::time::sleep(Duration::from_millis(300)).await;
    e.http.apply().await;
    let status = e.http.status();
    assert!(!status.listening);
    assert_eq!(status.problem, Some(http::McpHttpProblem::PortInUse));
    drop(holder);
    let deadline = Instant::now() + http::RETRY_BIND * 4;
    while !e.http.status().listening {
        assert!(Instant::now() < deadline, "did not retry");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(e.http.status().problem, None);
}

/// 유휴 판정에 드는 것은 인증을 지나 `initialize` 를 마친 세션뿐이다 — 토큰 없는 요청 · 인증 전 연결 · 실패한 `initialize` 는 세지
/// 않는다(그것으로 Relay 데몬을 깨워 두지 못한다). 세션이 닫히면 빠진다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_initialized_sessions_are_counted() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    let _idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut agent = e.client();
    let stranger = agent.with_token("");
    let mut no_auth = stranger.headers();
    no_auth.retain(|(k, _)| k != "Authorization");
    let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "x", "version": "1"}}});
    assert_eq!(stranger.post_with(no_auth, &init).await.status, 401);
    let broken = agent
        .post(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"nope": 1}}))
        .await;
    assert_ne!(
        broken.header("mcp-session-id").map(str::len),
        Some(64),
        "{}",
        broken.all()
    );
    assert_eq!(e.connections.active(), 0);
    agent.initialize().await;
    assert_eq!(e.connections.active(), 1);
    assert_eq!(agent.send("DELETE", agent.headers(), b"").await.status, 200);
    assert_eq!(e.connections.active(), 0);
}

/// 답을 기다리던 HTTP 요청의 연결이 끊기면 그 호출을 거둔다(`notifications/cancelled` — stdio 의 취소와 같은 길): 승인 요청이 창에서
/// 내려가고 툴은 돌지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_http_request_withdraws_its_call() {
    let port = free_port();
    let f = Fixture::with_global(&http_global(true, port));
    let e = f.endpoint(port, Limits::default()).await;
    let mut agent = e.client();
    agent.initialize().await;
    let mut window = f.window(&f.alpha);
    let body = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "fake_write", "arguments": {"projectId": f.alpha, "tag": "gone"}}});
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream
        .write_all(&request(
            "POST",
            http::PATH,
            &agent.headers(),
            body.to_string().as_bytes(),
        ))
        .await
        .unwrap();
    let asked = window.opened().await;
    drop(stream);
    window.closed(asked.id).await;
    assert_eq!(f.runs(), 0);
    assert!(
        window.answer(asked.id, McpApprovalAnswer::Once).is_err(),
        "이미 거둔 요청"
    );
}
