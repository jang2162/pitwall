//! 시험 전용 가짜 디버그 어댑터(17-m6-plan.md M6-15). `pw-dap-fake <시나리오.json> [--port N --host H | --socket 경로]` — 형식은
//! `pw_dap::fake` 머리 주석. `pw-dap-fake --sleep N` 은 N 초 자고 끝난다(`hold` 가 띄우는 자식). 배포물에 들지 않는다.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use pw_dap::fake::{Action, Reply, Scenario};
use pw_dap::protocol::{self, Incoming};
use pw_wire::FrameReader;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

struct Fake {
    scenario: Scenario,
    tcp: bool,
}

impl Fake {
    fn record(&self, line: &Value) {
        let Some(path) = &self.scenario.record else {
            return;
        };
        // 한 줄을 한 번의 `write` 로 — 같은 기록 파일을 쓰는 가짜 어댑터 여럿(자식 세션)의 줄이 섞이지 않게(O_APPEND).
        let mut text = line.to_string();
        text.push('\n');
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(text.as_bytes());
        }
    }

    fn initialized_after(&self) -> &str {
        self.scenario
            .initialized_after
            .as_deref()
            .unwrap_or("initialize")
    }
}

/// 연결 하나.
struct Conn {
    fake: Arc<Fake>,
    index: usize,
    out: mpsc::Sender<Vec<u8>>,
    seq: AtomicI64,
    initialized: AtomicBool,
    received: watch::Sender<HashSet<String>>,
    /// 이 연결의 소켓(unix — `hold` 가 자식에게 물려준다).
    fd: Option<i32>,
}

impl Conn {
    /// 이 연결의 소켓을 물려받은 자식을 새 프로세스 그룹에 띄운다 — 어댑터(이 프로세스)가 끝나도 연결이 열린 채 남는다.
    fn hold(&self, seconds: u64) {
        #[cfg(unix)]
        {
            let Some(fd) = self.fd else {
                eprintln!("pw-dap-fake: hold needs a socket connection");
                return;
            };
            // SAFETY: 이 프로세스가 연 소켓의 FD_CLOEXEC 를 걷는다 — 다음 exec 에 물려준다.
            unsafe {
                libc::fcntl(fd, libc::F_SETFD, 0);
            }
            let Ok(me) = std::env::current_exe() else {
                return;
            };
            use std::os::unix::process::CommandExt as _;
            match std::process::Command::new(me)
                .args(["--sleep", &seconds.to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .process_group(0)
                .spawn()
            {
                Ok(child) => self.fake.record(&json!({ "holder": child.id() })),
                Err(e) => eprintln!("pw-dap-fake: cannot spawn the holder: {e}"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (seconds, self.fd);
            eprintln!("pw-dap-fake: hold is unix only");
        }
    }

    async fn send(&self, body: Vec<u8>) {
        let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        bytes.extend_from_slice(&body);
        let _ = self.out.send(bytes).await;
    }

    fn next_seq(&self) -> i64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }

    async fn initialized(&self) {
        self.initialized.store(true, Ordering::SeqCst);
        let seq = self.next_seq();
        self.send(protocol::event(seq, "initialized", &Value::Null))
            .await;
    }

    async fn act(&self, actions: &[Action], trigger: &Value) {
        for action in actions {
            match action {
                Action::Event { event, body } => {
                    let seq = self.next_seq();
                    self.send(protocol::event(seq, event, body)).await;
                }
                Action::Request {
                    command,
                    arguments,
                    unless_arg,
                } => {
                    if unless_arg
                        .as_deref()
                        .is_some_and(|key| trigger.get(key).is_some())
                    {
                        continue;
                    }
                    let seq = self.next_seq();
                    self.send(protocol::request(seq, command, arguments)).await;
                }
                Action::Stderr { text } => eprintln!("{text}"),
                Action::Raw { text } => {
                    let _ = self.out.send(text.as_bytes().to_vec()).await;
                }
                Action::Body { text } => self.send(text.as_bytes().to_vec()).await,
                Action::Sleep { ms } => tokio::time::sleep(Duration::from_millis(*ms)).await,
                Action::Hold { seconds } => self.hold(*seconds),
                Action::Exit { code } => {
                    // 보낼 것을 먼저 내보낼 틈을 준다.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    std::process::exit(*code);
                }
            }
        }
    }

    fn default_body(&self, command: &str, arguments: &Value) -> Value {
        match command {
            "initialize" => self.fake.scenario.capabilities.clone(),
            "threads" => json!({ "threads": [{ "id": 1, "name": "main" }] }),
            "setFunctionBreakpoints" => {
                let n = arguments
                    .get("breakpoints")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                json!({ "breakpoints": vec![json!({ "verified": true }); n] })
            }
            "setBreakpoints" => {
                let lines: Vec<Value> = arguments
                    .get("breakpoints")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|b| json!({ "verified": true, "line": b.get("line").cloned().unwrap_or(Value::Null) }))
                    .collect();
                json!({ "breakpoints": lines })
            }
            _ => Value::Null,
        }
    }

    async fn handle(self: Arc<Self>, request_seq: i64, command: String, arguments: Value) {
        let scenario = &self.fake.scenario;
        if let Some(actions) = scenario.on.get(&command) {
            self.act(actions, &arguments).await;
        }
        let after = self.fake.initialized_after();
        if command == after && command != "initialize" {
            self.initialized().await;
        }
        let reply = scenario.replies.get(&command).cloned().unwrap_or_default();
        if !reply.after.is_empty() {
            let mut received = self.received.subscribe();
            let _ = received
                .wait_for(|got| reply.after.iter().all(|c| got.contains(c)))
                .await;
        }
        if reply.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(reply.delay_ms)).await;
        }
        if !reply.no_reply {
            let result = match &reply {
                Reply {
                    error: Some(message),
                    ..
                } => Err(message.clone()),
                Reply {
                    body: Some(body), ..
                } => Ok(body.clone()),
                _ => Ok(self.default_body(&command, &arguments)),
            };
            let seq = self.next_seq();
            self.send(protocol::response(seq, request_seq, &command, &result))
                .await;
        }
        if command == "initialize" && after == "initialize" {
            self.initialized().await;
        }
        self.act(&reply.then, &arguments).await;
        if command == "disconnect" && (!self.fake.tcp || self.index == 0) {
            tokio::time::sleep(Duration::from_millis(50)).await;
            std::process::exit(0);
        }
    }
}

async fn serve<R, W>(fake: Arc<Fake>, index: usize, fd: Option<i32>, reader: R, mut writer: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, mut out_rx) = mpsc::channel::<Vec<u8>>(1024);
    tokio::spawn(async move {
        while let Some(bytes) = out_rx.recv().await {
            if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
    });
    let conn = Arc::new(Conn {
        fake: Arc::clone(&fake),
        index,
        out,
        seq: AtomicI64::new(1),
        initialized: AtomicBool::new(false),
        received: watch::channel(HashSet::new()).0,
        fd,
    });
    conn.act(&fake.scenario.start, &Value::Null).await;
    let mut frames = FrameReader::new(reader);
    while let Ok(Some(body)) = frames.read_frame().await {
        let message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        fake.record(&json!({ "conn": index, "msg": message }));
        if let Ok(Incoming::Request {
            seq,
            command,
            arguments,
        }) = protocol::parse(&body)
        {
            if fake.scenario.require_initialized.contains(&command)
                && !conn.initialized.load(Ordering::SeqCst)
            {
                fake.record(&json!({
                    "conn": index,
                    "violation": format!("`{command}` came before the `initialized` event"),
                }));
                eprintln!("pw-dap-fake: `{command}` came before the `initialized` event");
                std::process::exit(97);
            }
            conn.received.send_modify(|got| {
                got.insert(command.clone());
            });
            tokio::spawn(Arc::clone(&conn).handle(seq, command, arguments));
        }
    }
    if fake.scenario.ignore_eof {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--listen-detached") {
        let (Some(scenario), Some(socket)) = (args.get(2), args.get(3)) else {
            return ExitCode::from(2);
        };
        let Ok(scenario) = Scenario::read(std::path::Path::new(scenario)) else {
            return ExitCode::from(2);
        };
        let fake = Arc::new(Fake {
            scenario,
            tcp: true,
        });
        return listen_socket(fake, socket).await;
    }
    if args.get(1).map(String::as_str) == Some("--sleep") {
        let seconds = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
        std::thread::sleep(Duration::from_secs(seconds));
        return ExitCode::SUCCESS;
    }
    let Some(path) = args.get(1).map(PathBuf::from) else {
        eprintln!("usage: pw-dap-fake <scenario.json> [--port N --host H | --socket PATH]");
        return ExitCode::from(2);
    };
    let scenario = match Scenario::read(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("pw-dap-fake: cannot read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let (port, host, socket) = (flag("--port"), flag("--host"), flag("--socket"));
    let fake = Arc::new(Fake {
        scenario,
        tcp: port.is_some() || socket.is_some(),
    });
    fake.record(&json!({ "argv": &args[1..], "pid": std::process::id() }));
    if let Some(socket) = socket {
        if fake.scenario.listen_elsewhere {
            return listen_elsewhere(&fake, &path, &socket).await;
        }
        return listen_socket(fake, &socket).await;
    }
    match (port, host) {
        (Some(port), Some(host)) => {
            let listener = match tokio::net::TcpListener::bind(format!("{host}:{port}")).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("pw-dap-fake: cannot listen on {host}:{port}: {e}");
                    return ExitCode::from(3);
                }
            };
            let mut index = 0;
            while let Ok((stream, _)) = listener.accept().await {
                let fd = raw_fd(&stream);
                let (read, write) = stream.into_split();
                tokio::spawn(serve(Arc::clone(&fake), index, fd, read, write));
                index += 1;
            }
            ExitCode::SUCCESS
        }
        (None, None) => {
            serve(fake, 0, None, tokio::io::stdin(), tokio::io::stdout()).await;
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("pw-dap-fake: --port and --host go together");
            ExitCode::from(2)
        }
    }
}

#[cfg(unix)]
fn raw_fd<T: std::os::fd::AsRawFd>(stream: &T) -> Option<i32> {
    Some(stream.as_raw_fd())
}

#[cfg(not(unix))]
fn raw_fd<T>(_stream: &T) -> Option<i32> {
    None
}

/// 소켓 경로(unix 소켓 · Windows named pipe)에서 연결을 받는다.
#[cfg(unix)]
async fn listen_socket(fake: Arc<Fake>, path: &str) -> ExitCode {
    let listener = match tokio::net::UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("pw-dap-fake: cannot listen on {path}: {e}");
            return ExitCode::from(3);
        }
    };
    let mut index = 0;
    while let Ok((stream, _)) = listener.accept().await {
        let fd = raw_fd(&stream);
        let (read, write) = stream.into_split();
        tokio::spawn(serve(Arc::clone(&fake), index, fd, read, write));
        index += 1;
    }
    ExitCode::SUCCESS
}

#[cfg(windows)]
async fn listen_socket(fake: Arc<Fake>, path: &str) -> ExitCode {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut first = true;
    let mut index = 0;
    loop {
        let server = match ServerOptions::new().first_pipe_instance(first).create(path) {
            Ok(server) => server,
            Err(e) => {
                eprintln!("pw-dap-fake: cannot listen on {path}: {e}");
                return ExitCode::from(3);
            }
        };
        first = false;
        if server.connect().await.is_err() {
            return ExitCode::SUCCESS;
        }
        let (read, write) = tokio::io::split(server);
        tokio::spawn(serve(Arc::clone(&fake), index, None, read, write));
        index += 1;
    }
}

#[cfg(not(any(unix, windows)))]
async fn listen_socket(_fake: Arc<Fake>, path: &str) -> ExitCode {
    eprintln!("pw-dap-fake: cannot listen on {path} here");
    ExitCode::from(3)
}

/// 소켓을 스스로 받지 않고 새 프로세스 그룹의 자식이 받게 한 뒤, 이 프로세스는 자기만 한다(시나리오의 `listenElsewhere`).
async fn listen_elsewhere(fake: &Fake, scenario: &std::path::Path, socket: &str) -> ExitCode {
    let Ok(me) = std::env::current_exe() else {
        return ExitCode::from(2);
    };
    let mut command = std::process::Command::new(me);
    command
        .arg("--listen-detached")
        .arg(scenario)
        .arg(socket)
        .stdin(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    match command.spawn() {
        Ok(child) => fake.record(&json!({ "listener": child.id() })),
        Err(e) => {
            eprintln!("pw-dap-fake: cannot spawn the listener: {e}");
            return ExitCode::from(3);
        }
    }
    tokio::time::sleep(Duration::from_secs(60)).await;
    ExitCode::SUCCESS
}
