//! 헤드리스로 시작한 `pitwall mcp` 가 앱으로 넘겨 붙기(16-m5-plan M5-20, 결정 M5-D3, 05-mcp.md 「로컬 소켓과 헤드리스」).
//!
//! 헤드리스 세션(`pw_mcp::serve`)은 stdio 에 바로 붙지 않고 프로세스 안 duplex 에 붙는다. 그 사이에 이 층이 서서 클라이언트의
//! 줄을 고르고 옮긴다. 줄은 고치지 않는다(합성하는 `tools/list_changed` 한 줄만 새로 쓴다).
//!
//! - **앱 찾기는 요청 하나에 한 번**: 넘기기 전에 `tools/list` · `tools/call` 요청이 올 때마다 소켓을 한 번 본다([`run`] 의
//!   `dial`). 주기적으로 보지 않는다 — 요청이 없으면 일도 없다. 넘긴 뒤에는 보지 않는다.
//! - **넘겨 붙기**: 앱이 받으면 클라이언트가 헤드리스에 보냈던 `initialize` 와 `notifications/initialized` 를 그대로 앱에 다시
//!   보내 앱 쪽 세션을 연다(같은 핸드셰이크 · 같은 클라이언트 이름). 앱의 `initialize` 응답은 클라이언트에 보내지 않는다 — 이미
//!   헤드리스의 응답을 받았다. 2026-07-28 클라이언트는 `initialize` 가 없고 요청마다 `_meta` 를 싣는다 — 다시 보낼 것이 없다.
//!   그 클라이언트가 헤드리스에 열어 둔 `subscriptions/listen` 은 같은 줄로 앱에 다시 열어 그 구독(같은 요청 id)을 앱으로 옮긴다
//!   (앱이 보내는 두 번째 수락 알림은 삼킨다). 그 뒤 `tools/list_changed` 를 클라이언트에 보낸다 — 2025-11-25 까지는 그냥,
//!   2026-07-28 은 툴 목록 변경을 받겠다고 한 구독마다 그 구독 id 를 실어서. 넘기게 한 요청부터 앱이 받는다.
//! - **넘기기 전에 시작된 호출은 헤드리스에서 끝난다**: 요청 id 마다 받은 쪽을 적어 두고, 응답 · 취소 알림을 그쪽으로 돌린다.
//!   헤드리스에 남은 요청이 없어지면 헤드리스의 입력을 닫는다 — 헤드리스 세션은 끝나며 감사 기록을 다 쓴다(`pw_mcp::serve`).
//!   넘긴 뒤 헤드리스가 내는 툴 목록 알림은 버린다(목록의 주인은 앱이다).
//! - **넘긴 뒤는 프록시와 같다**: 앱이 소켓을 닫으면 앱이 받은 요청마다 `-32001` 오류를 쓰고 [`End::AppClosed`]
//!   (`pw_mcp::proxy` 와 같은 꼴). 클라이언트가 stdin 을 닫으면 양쪽 입력을 닫고 남은 응답을 [`DRAIN`] 까지 옮긴다.
//! - 서버가 클라이언트에 보내는 요청의 응답은 그 요청을 낸 쪽으로 돌린다. 두 서버가 같은 id 를 쓰면 가를 수 없지만 Pitwall 의
//!   세션은 클라이언트에 요청을 보내지 않는다.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use pw_mcp::proxy::{APP_CLOSED, DRAIN};
use pw_mcp::server::ServeError;
use pw_mcp::{Services, SessionRequest};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 앱이 다시 보낸 `initialize` 에 답하기를 기다리는 시간. 넘으면 넘기지 않고 헤드리스로 남는다(다음 요청이 다시 본다).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// 앱을 찾을 때마다 로그에 남는 문장. 「요청 하나에 한 번」 을 진짜 바이너리 시험이 이 줄의 수로 본다.
pub const CHECK_LOG: &str = "mcp handover: looking for the Pitwall app";

const INITIALIZE: &str = "initialize";
const INITIALIZED: &str = "notifications/initialized";
const LISTEN: &str = "subscriptions/listen";
const ACKNOWLEDGED: &str = "notifications/subscriptions/acknowledged";
const LIST_CHANGED: &str = "notifications/tools/list_changed";
const CANCELLED: &str = "notifications/cancelled";
const SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";
/// 앱을 찾게 하는 요청. 그 밖의 요청(`ping` · `initialize` 등)은 앱을 찾지 않는다.
const TRIGGERS: [&str; 2] = ["tools/list", "tools/call"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // `pw_mcp::proxy::ProxyEnd` 와 같은 이름 — 넘긴 뒤는 프록시와 같은 꼴로 끝난다.
pub enum End {
    /// 클라이언트가 stdin 을 닫았다. 정상 종료.
    ClientClosed,
    /// 넘겨 붙은 뒤 앱이 소켓을 닫았다. 앱이 받은 `pending` 개의 요청에 오류 응답을 써 주었다(프록시와 같다).
    AppClosed { pending: usize },
    /// 넘기기 전에 헤드리스 세션이 먼저 끝났다(초기화 실패 등). 종료 코드는 그 세션의 결과가 정한다.
    HeadlessClosed,
}

/// [`serve_headless`] 의 결과: 이 층이 끝난 꼴과 헤드리스 세션의 결과.
#[derive(Debug)]
pub struct Outcome {
    pub end: io::Result<End>,
    pub session: Result<(), ServeError>,
}

/// 앱 없이 시작한 `pitwall mcp` 의 본체: 헤드리스 세션을 프로세스 안 duplex 에 띄우고 [`run`] 을 그 앞에 세운다. 헤드리스 세션이
/// 끝날 때까지(감사 기록을 다 쓸 때까지) 기다린 뒤 돌아온다. `dial` 은 앱 소켓에 붙어 본다(`super::connect_app`).
pub async fn serve_headless<I, O, D, F, A>(
    services: Arc<Services>,
    request: SessionRequest,
    stdin: I,
    mut stdout: O,
    dial: D,
) -> Outcome
where
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Unpin,
    D: FnMut() -> F,
    F: Future<Output = Option<A>>,
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let cancel = CancellationToken::new();
    let mut session = tokio::spawn(pw_mcp::serve(
        services,
        request,
        tokio::io::split(theirs),
        cancel.clone(),
    ));
    let end = run(stdin, &mut stdout, ours, dial).await;
    // `run` 이 돌아오면 헤드리스의 입력은 닫혔다 — 세션이 끝나며 감사 기록을 다 쓴다. 끊긴 뒤에도 도는 핸들러(rmcp 의 drain)가
    // 오래 붙잡으면 취소하고 마저 기다린다.
    let joined = match tokio::time::timeout(DRAIN * 2, &mut session).await {
        Ok(joined) => joined,
        Err(_) => {
            tracing::warn!("the headless mcp session did not end; cancelling it");
            cancel.cancel();
            session.await
        }
    };
    let session = joined.unwrap_or_else(|e| Err(ServeError::Host(format!("session task: {e}"))));
    Outcome { end, session }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Headless,
    App,
}

/// JSON-RPC 한 줄의 꼴.
enum Message {
    Request {
        key: String,
        method: String,
        value: Value,
    },
    Notification {
        method: String,
        value: Value,
    },
    Response {
        key: String,
    },
    /// JSON 이 아니거나 배치 등 — 지금 쪽으로 그대로 옮긴다.
    Other,
}

fn classify(line: &[u8]) -> Message {
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return Message::Other;
    };
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let id = value
        .get("id")
        .filter(|id| !id.is_null())
        .map(Value::to_string);
    match (method, id) {
        (Some(method), Some(key)) => Message::Request { key, method, value },
        (Some(method), None) => Message::Notification { method, value },
        (None, Some(key)) => Message::Response { key },
        (None, None) => Message::Other,
    }
}

/// 알림 `_meta` 의 구독 id(2026-07-28).
fn subscription_key(value: &Value) -> Option<String> {
    value
        .pointer("/params/_meta")
        .and_then(|meta| meta.get(SUBSCRIPTION_ID))
        .map(Value::to_string)
}

type Lines = mpsc::UnboundedReceiver<Vec<u8>>;
type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// 줄을 읽어 채널로 보낸다. 끝(EOF · 읽기 오류)이면 채널이 닫힌다.
fn lines<R: AsyncRead + Send + Unpin + 'static>(read: R) -> Lines {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut read = BufReader::new(read);
        loop {
            let mut line = Vec::new();
            match read.read_until(b'\n', &mut line).await {
                Ok(0) => return,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    tracing::debug!(%e, "mcp handover: read failed");
                    return;
                }
            }
        }
    });
    rx
}

async fn write_line(to: &mut (impl AsyncWrite + Unpin + ?Sized), line: &[u8]) -> io::Result<()> {
    to.write_all(line).await?;
    if line.last() != Some(&b'\n') {
        to.write_all(b"\n").await?;
    }
    to.flush().await
}

/// 헤드리스에 열린 `subscriptions/listen`.
struct Listen {
    key: String,
    line: Vec<u8>,
    /// 툴 목록 변경을 받겠다고 했다(`params.notifications.toolsListChanged`).
    tools: bool,
}

/// 줄을 어느 쪽으로 보낼지의 상태.
struct Router<'a, O> {
    stdout: &'a mut O,
    /// 헤드리스의 입력. 넘긴 뒤 헤드리스에 남은 요청이 없으면 닫는다(`None`).
    headless: Option<Writer>,
    /// 넘긴 뒤의 앱 입력.
    app: Option<Writer>,
    /// 클라이언트가 처음 보낸 `initialize`(줄, id 의 키).
    initialize: Option<(Vec<u8>, String)>,
    initialized: Option<Vec<u8>>,
    /// 응답을 기다리는 클라이언트 요청: id 의 키 → 받은 쪽과 id.
    pending: HashMap<String, (Side, Value)>,
    /// 서버가 클라이언트에 보낸 요청: id 의 키 → 낸 쪽.
    server_requests: HashMap<String, Side>,
    listens: Vec<Listen>,
    /// 앱으로 옮긴 구독 id. 헤드리스가 그 구독에 내는 것(최종 결과 · 알림)은 버린다.
    moved: HashSet<String>,
    /// 앱이 아직 수락 알림을 보내지 않은 옮긴 구독. 그 알림은 클라이언트가 이미 받았으므로 삼킨다.
    unacknowledged: HashSet<String>,
}

impl<O: AsyncWrite + Unpin> Router<'_, O> {
    fn handed(&self) -> bool {
        self.app.is_some()
    }

    fn current(&self) -> Side {
        if self.handed() {
            Side::App
        } else {
            Side::Headless
        }
    }

    async fn send(&mut self, side: Side, line: &[u8]) {
        let (to, name) = match side {
            Side::Headless => (self.headless.as_mut(), "headless"),
            Side::App => (self.app.as_mut(), "app"),
        };
        let Some(to) = to else {
            tracing::debug!(
                side = name,
                "mcp handover: that side is closed; dropping a line"
            );
            return;
        };
        // 쓰기가 실패해도 읽기 쪽이 끝을 보고 판정한다(앱이면 AppClosed).
        if let Err(e) = write_line(to, line).await {
            tracing::debug!(%e, side = name, "mcp handover: write failed");
        }
    }

    async fn write_client(&mut self, line: &[u8]) -> io::Result<()> {
        write_line(self.stdout, line).await
    }

    /// 클라이언트의 줄 하나. 넘겨 붙었으면 앱의 줄을 읽는 채널을 돌려준다.
    async fn on_client<D, F, A>(&mut self, line: Vec<u8>, dial: &mut D) -> io::Result<Option<Lines>>
    where
        D: FnMut() -> F,
        F: Future<Output = Option<A>>,
        A: AsyncRead + AsyncWrite + Send + 'static,
    {
        let mut opened = None;
        match classify(&line) {
            Message::Request { key, method, value } => {
                if method == INITIALIZE && self.initialize.is_none() {
                    self.initialize = Some((line.clone(), key.clone()));
                }
                if !self.handed() && TRIGGERS.contains(&method.as_str()) {
                    opened = self.hand_over(dial).await?;
                }
                let side = self.current();
                if side == Side::Headless && method == LISTEN {
                    self.listens.push(Listen {
                        key: key.clone(),
                        line: line.clone(),
                        tools: value.pointer("/params/notifications/toolsListChanged")
                            == Some(&Value::Bool(true)),
                    });
                }
                let id = value.get("id").cloned().unwrap_or(Value::Null);
                self.pending.insert(key, (side, id));
                self.send(side, &line).await;
            }
            Message::Notification { method, value } => {
                if method == INITIALIZED && self.initialized.is_none() {
                    self.initialized = Some(line.clone());
                }
                let side = if method == CANCELLED {
                    // 취소된 요청에는 응답이 오지 않는다 — 기다림에서 뺀다(헤드리스를 닫는 판정이 그것을 기다리지 않게).
                    value
                        .pointer("/params/requestId")
                        .map(Value::to_string)
                        .and_then(|key| self.pending.remove(&key))
                        .map_or(self.current(), |(side, _)| side)
                } else {
                    self.current()
                };
                self.send(side, &line).await;
            }
            Message::Response { key } => {
                let side = self.server_requests.remove(&key).unwrap_or(self.current());
                self.send(side, &line).await;
            }
            Message::Other => {
                let side = self.current();
                self.send(side, &line).await;
            }
        }
        self.close_idle_headless().await;
        Ok(opened)
    }

    /// 앱을 한 번 찾아 보고, 있으면 넘겨 붙는다. 넘겨 붙었으면 앱의 줄을 읽는 채널.
    async fn hand_over<D, F, A>(&mut self, dial: &mut D) -> io::Result<Option<Lines>>
    where
        D: FnMut() -> F,
        F: Future<Output = Option<A>>,
        A: AsyncRead + AsyncWrite + Send + 'static,
    {
        tracing::info!("{CHECK_LOG}");
        let Some(stream) = dial().await else {
            return Ok(None);
        };
        let (read, write) = tokio::io::split(stream);
        let mut read = BufReader::new(read);
        let mut write: Writer = Box::new(write);
        if let Some((line, key)) = &self.initialize {
            let opened = async {
                write_line(&mut write, line).await?;
                let answer = loop {
                    let mut answer = Vec::new();
                    if read.read_until(b'\n', &mut answer).await? == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "the app closed before answering initialize",
                        ));
                    }
                    match classify(&answer) {
                        Message::Response { key: got } if &got == key => break answer,
                        _ => tracing::debug!(
                            "mcp handover: skipping a line before the initialize answer"
                        ),
                    }
                };
                let answer: Value = serde_json::from_slice(&answer).map_err(io::Error::other)?;
                if let Some(error) = answer.get("error") {
                    return Err(io::Error::other(format!(
                        "the app refused initialize: {error}"
                    )));
                }
                if let Some(initialized) = &self.initialized {
                    write_line(&mut write, initialized).await?;
                }
                Ok(())
            };
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, opened).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(%e, "not handing over to the Pitwall app; staying headless");
                    return Ok(None);
                }
                Err(_) => {
                    tracing::warn!("the Pitwall app did not answer initialize; staying headless");
                    return Ok(None);
                }
            }
        }
        // 2026-07-28: 헤드리스에 열린 구독을 같은 요청 id 로 앱에 다시 연다.
        for listen in &self.listens {
            if let Err(e) = write_line(&mut write, &listen.line).await {
                tracing::warn!(%e, "not handing over to the Pitwall app; staying headless");
                return Ok(None);
            }
        }
        let listens = std::mem::take(&mut self.listens);
        for listen in &listens {
            if let Some(entry) = self.pending.get_mut(&listen.key) {
                entry.0 = Side::App;
            }
            self.moved.insert(listen.key.clone());
            self.unacknowledged.insert(listen.key.clone());
        }
        tracing::info!("handed the mcp session over to the Pitwall app");
        self.app = Some(write);
        // 앱의 줄을 읽기 전에 쓴다 — 넘기게 한 요청의 응답보다 먼저 간다.
        if self.initialize.is_some() {
            self.write_client(&list_changed(None)).await?;
        }
        for listen in listens.iter().filter(|l| l.tools) {
            let id: Value = serde_json::from_str(&listen.key).expect("id 의 키는 JSON 이다");
            self.write_client(&list_changed(Some(id))).await?;
        }
        Ok(Some(lines(read)))
    }

    /// 넘긴 뒤 헤드리스에 남은 요청이 없으면 헤드리스의 입력을 닫는다 — 세션이 끝나며 감사 기록을 다 쓴다.
    async fn close_idle_headless(&mut self) {
        if !self.handed() || self.headless.is_none() {
            return;
        }
        let waiting = self
            .pending
            .values()
            .any(|(side, _)| *side == Side::Headless)
            || self
                .server_requests
                .values()
                .any(|side| *side == Side::Headless);
        if waiting {
            return;
        }
        if let Some(mut headless) = self.headless.take() {
            let _ = headless.shutdown().await;
            tracing::debug!("closed the headless mcp session after the handover");
        }
    }

    async fn on_headless(&mut self, line: Vec<u8>) -> io::Result<()> {
        match classify(&line) {
            Message::Response { key } => {
                if self.moved.contains(&key) {
                    return Ok(());
                }
                self.pending.remove(&key);
                self.listens.retain(|l| l.key != key);
            }
            Message::Notification { method, value } => {
                let moved = subscription_key(&value).is_some_and(|k| self.moved.contains(&k));
                if moved || (self.handed() && (method == LIST_CHANGED || method == ACKNOWLEDGED)) {
                    return Ok(());
                }
            }
            Message::Request { key, .. } => {
                self.server_requests.insert(key, Side::Headless);
            }
            Message::Other => {}
        }
        self.write_client(&line).await?;
        self.close_idle_headless().await;
        Ok(())
    }

    async fn on_app(&mut self, line: Vec<u8>) -> io::Result<()> {
        match classify(&line) {
            Message::Response { key } => {
                self.pending.remove(&key);
            }
            Message::Notification { method, value } => {
                if method == ACKNOWLEDGED
                    && subscription_key(&value).is_some_and(|k| self.unacknowledged.remove(&k))
                {
                    return Ok(());
                }
            }
            Message::Request { key, .. } => {
                self.server_requests.insert(key, Side::App);
            }
            Message::Other => {}
        }
        self.write_client(&line).await
    }

    /// 앱이 닫혔다: 앱이 받은 요청마다 오류 응답을 쓴다(`pw_mcp::proxy` 와 같은 꼴). 쓴 수.
    async fn answer_app_closed(&mut self) -> io::Result<usize> {
        let waiting: Vec<Value> = self
            .pending
            .iter()
            .filter(|(_, (side, _))| *side == Side::App)
            .map(|(_, (_, id))| id.clone())
            .collect();
        self.pending.retain(|_, (side, _)| *side != Side::App);
        for id in &waiting {
            let line = serde_json::to_vec(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": APP_CLOSED, "message": "the Pitwall app closed the connection"},
            }))
            .expect("JSON 값은 직렬화된다");
            self.write_client(&line).await?;
        }
        self.app = None;
        Ok(waiting.len())
    }

    async fn close_inputs(&mut self) {
        for mut to in [self.headless.take(), self.app.take()]
            .into_iter()
            .flatten()
        {
            let _ = to.shutdown().await;
        }
    }
}

/// `notifications/tools/list_changed` 한 줄. 2026-07-28 구독이면 그 구독 id 를 `_meta` 에 싣는다.
fn list_changed(subscription: Option<Value>) -> Vec<u8> {
    let message = match subscription {
        Some(id) => json!({
            "jsonrpc": "2.0",
            "method": LIST_CHANGED,
            "params": {"_meta": {SUBSCRIPTION_ID: id}},
        }),
        None => json!({"jsonrpc": "2.0", "method": LIST_CHANGED}),
    };
    serde_json::to_vec(&message).expect("JSON 값은 직렬화된다")
}

async fn next(lines: &mut Option<Lines>) -> Option<Vec<u8>> {
    match lines {
        Some(lines) => lines.recv().await,
        None => std::future::pending().await,
    }
}

/// 클라이언트(`stdin` · `stdout`)와 헤드리스 세션(`headless` — 그 세션이 붙은 duplex 의 이쪽 끝) 사이에 서서, 넘기기 전에는
/// 헤드리스로, 넘긴 뒤에는 `dial` 이 준 앱 연결로 옮긴다. 돌아오기 전에 헤드리스의 입력을 닫는다.
pub async fn run<I, O, H, D, F, A>(
    stdin: I,
    stdout: &mut O,
    headless: H,
    mut dial: D,
) -> io::Result<End>
where
    I: AsyncRead + Send + Unpin + 'static,
    O: AsyncWrite + Unpin,
    H: AsyncRead + AsyncWrite + Send + 'static,
    D: FnMut() -> F,
    F: Future<Output = Option<A>>,
    A: AsyncRead + AsyncWrite + Send + 'static,
{
    let (from_headless, to_headless) = tokio::io::split(headless);
    let mut client = Some(lines(stdin));
    let mut headless = Some(lines(from_headless));
    let mut app: Option<Lines> = None;
    let mut router = Router {
        stdout,
        headless: Some(Box::new(to_headless)),
        app: None,
        initialize: None,
        initialized: None,
        pending: HashMap::new(),
        server_requests: HashMap::new(),
        listens: Vec::new(),
        moved: HashSet::new(),
        unacknowledged: HashSet::new(),
    };
    // 끝내는 중이면 그 꼴과 남은 응답을 옮길 시한.
    let mut ending: Option<End> = None;
    let drain = tokio::time::sleep(Duration::MAX);
    tokio::pin!(drain);
    let result = loop {
        if let (Some(end), None, None) = (ending, &headless, &app) {
            break Ok(end);
        }
        tokio::select! {
            line = next(&mut client), if client.is_some() => match line {
                Some(line) => match router.on_client(line, &mut dial).await {
                    Ok(Some(opened)) => app = Some(opened),
                    Ok(None) => {}
                    Err(e) => break Err(e),
                },
                None => {
                    client = None;
                    router.close_inputs().await;
                    ending = Some(End::ClientClosed);
                    drain.as_mut().reset(tokio::time::Instant::now() + DRAIN);
                }
            },
            line = next(&mut headless), if headless.is_some() => match line {
                Some(line) => {
                    if let Err(e) = router.on_headless(line).await {
                        break Err(e);
                    }
                }
                None => {
                    headless = None;
                    router.headless = None;
                    if ending.is_none() && !router.handed() {
                        break Ok(End::HeadlessClosed);
                    }
                }
            },
            line = next(&mut app), if app.is_some() => match line {
                Some(line) => {
                    if let Err(e) = router.on_app(line).await {
                        break Err(e);
                    }
                }
                None => {
                    app = None;
                    if ending.is_none() {
                        // 넘긴 뒤 앱이 먼저 닫혔다. 헤드리스에 남은 호출의 응답은 마저 옮긴다.
                        let pending = match router.answer_app_closed().await {
                            Ok(pending) => pending,
                            Err(e) => break Err(e),
                        };
                        client = None;
                        router.close_inputs().await;
                        ending = Some(End::AppClosed { pending });
                        drain.as_mut().reset(tokio::time::Instant::now() + DRAIN);
                    }
                }
            },
            () = &mut drain, if ending.is_some() => break Ok(ending.expect("조건에서 봤다")),
        }
    };
    router.close_inputs().await;
    let _ = router.stdout.flush().await;
    result
}

#[cfg(test)]
#[path = "handover_tests.rs"]
mod tests;
