//! 실제 PTY 와 셸(`/bin/sh`)로 RPC 세션 전체를 본다. 환경은 가짜 로그인 셸(로캘 변수를 지우고 — launchd 로 띄운 GUI 앱처럼 —
//! `PATH` · `SHELL=/bin/sh` · 임시 `HOME` · 앱을 띄운 터미널의 것처럼 남은 `TERM_PROGRAM_VERSION` 을 정하고 받은 스크립트를 돌리는
//! sh 스크립트)에서 온다 — 사람의 셸 시작 파일은 돌지 않는다.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry as Projects};
use pw_rpc::protocol::{
    STREAM_CHUNK, STREAM_CREDIT, STREAM_END, StreamChunk, StreamEnd, StreamOpened, StreamOutcome,
};
use pw_rpc::{Caller, EventHub, Id, Outbound, Response, Role, Router, Session, code};
use pw_runtime::LoginShell;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use super::*;
use crate::pty::{CREDIT_WINDOW, OUTPUT_BUFFER, OUTPUT_LIMIT};

const LOGIN_SHELL: &str = "unset LANG LC_ALL LC_CTYPE\nPATH=/usr/bin:/bin; export PATH\nSHELL=/bin/sh; export SHELL\nFROM_LOGIN_SHELL=yes; export FROM_LOGIN_SHELL\nTERM_PROGRAM_VERSION=9.9; export TERM_PROGRAM_VERSION\neval \"$4\"";
const HANGING: &str = "sleep 30";

struct Fixture {
    tmp: tempfile::TempDir,
    terminals: Arc<Terminals>,
    project: String,
    root: PathBuf,
}

impl Fixture {
    fn new(settings: &str) -> Self {
        Self::with_login_shell(
            settings,
            LOGIN_SHELL,
            Duration::from_secs(5),
            pw_runtime::login_shell::TIMEOUT_COOLDOWN,
        )
    }

    /// `settings` 는 전역 `settings.toml` 내용, `login_shell` 은 가짜 로그인 셸의 본문.
    fn with_login_shell(
        settings: &str,
        login_shell: &str,
        timeout: Duration,
        cooldown: Duration,
    ) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(paths.config.join("settings.toml"), settings).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        assert_eq!(global.fault(), None);
        let projects = Projects::new(paths, None, bus);
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let shell = tmp.path().join("fake-login-shell");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nHOME='{}'; export HOME\n{login_shell}\n",
                home.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = Runtime::new(LoginShell::new(Some(shell), timeout).with_cooldown(cooldown));
        let terminals = Arc::new(Terminals::new(Arc::new(runtime)));
        terminals.attach(HostParts {
            global,
            projects: projects.clone(),
        });
        let root = tmp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        let project = projects
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap();
        let root = normalize(&root).unwrap();
        Self {
            tmp,
            terminals,
            project: project.id,
            root,
        }
    }

    fn client(&self) -> Client {
        let mut router = Router::new();
        methods::register(&mut router, Arc::clone(&self.terminals));
        let (session, rx) =
            Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
        Client {
            session,
            rx,
            next_id: 1,
            responses: Vec::new(),
            events: Vec::new(),
            ended: Vec::new(),
        }
    }

    fn create_params(&self, cols: u16, rows: u16) -> Value {
        json!({ "projectId": self.project, "cols": cols, "rows": rows })
    }
}

struct Opened {
    stream: u64,
    terminal: TerminalId,
    pid: u32,
    shell: Vec<String>,
    cwd: String,
}

/// 세션의 클라이언트 쪽. 응답과 스트림 항목이 섞여 오므로 받는 대로 나눠 둔다.
struct Client {
    session: Session,
    rx: UnboundedReceiver<Outbound>,
    next_id: i64,
    responses: Vec<Response>,
    events: Vec<(u64, TerminalEvent)>,
    ended: Vec<StreamEnd>,
}

impl Client {
    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn credit(&self, stream: u64, credit: u32) {
        self.session.handle_value(json!({
            "jsonrpc": "2.0", "method": STREAM_CREDIT, "params": {"streamId": stream, "credit": credit}
        }));
    }

    /// 메시지 하나를 받아 나눠 둔다. `wait` 안에 오지 않으면 실패.
    async fn receive(&mut self, wait: Duration) {
        let message = tokio::time::timeout(wait, self.rx.recv())
            .await
            .expect("timed out waiting for a message")
            .expect("session closed");
        match message {
            Outbound::Response(r) => self.responses.push(r),
            Outbound::Notification(n) if n.method == STREAM_CHUNK => {
                let chunk: StreamChunk = n.params_as().unwrap();
                let event = serde_json::from_value(chunk.data).unwrap();
                self.events.push((chunk.stream_id, event));
            }
            Outbound::Notification(n) if n.method == STREAM_END => {
                self.ended.push(n.params_as().unwrap());
            }
            Outbound::Notification(n) => panic!("unexpected notification {}", n.method),
        }
    }

    async fn response(&mut self, id: i64) -> Response {
        loop {
            if let Some(i) = self.responses.iter().position(|r| r.id == Id::Number(id)) {
                return self.responses.remove(i);
            }
            self.receive(Duration::from_secs(10)).await;
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Response {
        let id = self.request(method, params);
        self.response(id).await
    }

    /// 다음 스트림 항목(그 스트림의 것만).
    async fn next_event(&mut self, stream: u64) -> TerminalEvent {
        loop {
            if let Some(i) = self.events.iter().position(|(s, _)| *s == stream) {
                return self.events.remove(i).1;
            }
            // 기다리는 시간은 부른 쪽이 정한다(read_until 은 받은 글자와 함께 실패한다).
            self.receive(Duration::from_secs(30)).await;
        }
    }

    async fn open(&mut self, params: Value) -> Opened {
        let response = self.call("terminal.create", params).await;
        let opened: StreamOpened = response.result_as().unwrap();
        self.credit(opened.stream_id, 1_000_000);
        match self.next_event(opened.stream_id).await {
            TerminalEvent::Started {
                terminal_id,
                pid,
                shell,
                cwd,
            } => Opened {
                stream: opened.stream_id,
                terminal: terminal_id,
                pid: pid.unwrap(),
                shell,
                cwd,
            },
            other => panic!("expected started, got {other:?}"),
        }
    }

    /// 출력에 `wanted` 가 나올 때까지 읽어 그때까지의 글자를 준다.
    async fn read_until(&mut self, stream: u64, wanted: &str) -> String {
        let mut text = String::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !text.contains(wanted) {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(event) = tokio::time::timeout(left, self.next_event(stream)).await else {
                panic!("`{wanted}` did not appear in {text:?}");
            };
            match event {
                TerminalEvent::Output { data } => {
                    text.push_str(&String::from_utf8_lossy(&STANDARD.decode(data).unwrap()));
                }
                other => panic!("expected output, got {other:?} after {text:?}"),
            }
        }
        text
    }

    async fn input(&mut self, terminal: TerminalId, data: &str) {
        let response = self
            .call(
                "terminal.input",
                json!({"terminalId": terminal, "data": data}),
            )
            .await;
        response.outcome.unwrap();
    }

    /// 스트림이 끝날 때까지 받은 항목과 그 종료.
    async fn end(&mut self, stream: u64) -> (Vec<TerminalEvent>, StreamEnd) {
        while !self.ended.iter().any(|e| e.stream_id == stream) {
            self.receive(Duration::from_secs(10)).await;
        }
        let events = self
            .events
            .extract_if(.., |(s, _)| *s == stream)
            .map(|(_, e)| e)
            .collect();
        let end = self
            .ended
            .iter()
            .find(|e| e.stream_id == stream)
            .unwrap()
            .clone();
        (events, end)
    }

    /// `exited` 까지 출력을 버리고 그 code 와 스트림 종료를 준다.
    async fn exit(&mut self, stream: u64) -> (Option<i32>, StreamEnd) {
        let code = loop {
            match self.next_event(stream).await {
                TerminalEvent::Output { .. } => {}
                TerminalEvent::Exited { code } => break code,
                other => panic!("unexpected {other:?}"),
            }
        };
        while !self.ended.iter().any(|e| e.stream_id == stream) {
            self.receive(Duration::from_secs(10)).await;
        }
        let end = self
            .ended
            .iter()
            .find(|e| e.stream_id == stream)
            .unwrap()
            .clone();
        (code, end)
    }
}

fn alive(pid: u32) -> bool {
    let pid = libc::pid_t::try_from(pid).unwrap();
    // SAFETY: 신호 0 은 존재만 본다.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// 출력의 `BG=<pid>` 들.
fn background_pids(text: &str) -> Vec<u32> {
    text.split("BG=")
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect()
}

/// 기본 셸은 런타임 환경의 `$SHELL -l` 이고, 런타임 환경 위에 터미널 변수가 덮이며, cwd 는 프로젝트 루트다.
#[tokio::test(flavor = "multi_thread")]
async fn the_shell_runs_in_the_runtime_environment_with_xterm_256color() {
    let f = Fixture::new("schema = 1\n");
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    assert_eq!(t.shell, ["/bin/sh", "-l"]);
    assert_eq!(PathBuf::from(&t.cwd), f.root);
    c.input(
        t.terminal,
        "echo \"T=$TERM C=$COLORTERM P=$TERM_PROGRAM V=${TERM_PROGRAM_VERSION-none} F=$FROM_LOGIN_SHELL\"; echo \"D=$(pwd -P)\"; echo \"L=[$LANG]\"\n",
    )
    .await;
    // 가짜 로그인 셸은 로캘 변수를 정하지 않는다 — 시스템 로캘의 UTF-8 판이 들어간다.
    let lang = format!("L=[{}]", shell::default_lang().unwrap());
    assert!(lang.ends_with(".UTF-8]"), "{lang}");
    let text = c.read_until(t.stream, &lang).await;
    assert!(
        text.contains("T=xterm-256color C=truecolor P=Pitwall V=none F=yes"),
        "{text:?}"
    );
    assert!(
        text.contains(&format!("D={}", f.root.display())),
        "{text:?}"
    );
}

/// 터미널은 그것을 연 세션의 것이다. 다른 세션(다른 창 · 연결)이 같은 `terminalId` 로 보낸 입력 · 크기는 없는 터미널로
/// 거절되고, 닫기는 아무것도 하지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn another_session_cannot_use_the_terminal() {
    let f = Fixture::new("schema = 1\n");
    let mut owner = f.client();
    let mut other = f.client();
    let t = owner.open(f.create_params(80, 24)).await;
    for (method, params) in [
        (
            "terminal.input",
            json!({"terminalId": t.terminal, "data": "echo INJECTED_$((40+2))\n"}),
        ),
        (
            "terminal.resize",
            json!({"terminalId": t.terminal, "cols": 100, "rows": 40}),
        ),
    ] {
        let response = other.call(method, params).await;
        assert_eq!(
            response.outcome.unwrap_err().code,
            code::INVALID_PARAMS,
            "{method}"
        );
    }
    other
        .call("terminal.close", json!({"terminalId": t.terminal}))
        .await
        .outcome
        .unwrap();
    // 셸은 살아 있고, 크기는 그대로이며, 주입한 명령은 돌지 않았다(돌았으면 이 출력보다 먼저 나온다).
    owner
        .input(t.terminal, "stty size; echo OWNER_$((1+1))\n")
        .await;
    let text = owner.read_until(t.stream, "OWNER_2").await;
    assert!(text.contains("24 80"), "{text:?}");
    assert!(!text.contains("INJECTED_42"), "{text:?}");
    assert!(alive(t.pid));
}

/// 설정 `[terminal] shell` 이 셸을 덮고, `cwd` 를 주면 그 폴더에서 돈다.
#[tokio::test(flavor = "multi_thread")]
async fn the_shell_setting_and_cwd_are_used() {
    let f = Fixture::new(
        "schema = 1\n[terminal]\nshell = [\"/bin/sh\", \"-c\", \"echo ARGS=$0; pwd -P\"]\n",
    );
    let sub = f.root.join("sub dir");
    fs::create_dir_all(&sub).unwrap();
    let mut c = f.client();
    let mut params = f.create_params(80, 24);
    params["cwd"] = json!(sub.join("..").join("sub dir"));
    let t = c.open(params).await;
    assert_eq!(t.shell, ["/bin/sh", "-c", "echo ARGS=$0; pwd -P"]);
    assert_eq!(PathBuf::from(&t.cwd), sub);
    c.read_until(t.stream, &sub.display().to_string()).await;
    let (code, end) = c.exit(t.stream).await;
    assert_eq!(code, Some(0));
    assert_eq!(end.outcome, StreamOutcome::Completed);
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_are_invalid_params() {
    let f = Fixture::new("schema = 1\n");
    let mut c = f.client();
    let outside = f.tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    for params in [
        json!({"projectId": f.project, "cwd": outside, "cols": 80, "rows": 24}),
        json!({"projectId": f.project, "cwd": "relative", "cols": 80, "rows": 24}),
        json!({"projectId": f.project, "cwd": f.root.join("missing"), "cols": 80, "rows": 24}),
        json!({"projectId": f.project, "cols": 0, "rows": 24}),
        json!({"projectId": "nope", "cols": 80, "rows": 24}),
    ] {
        // 스트림 메서드는 먼저 열리고 핸들러의 오류로 끝난다.
        let response = c.call("terminal.create", params.clone()).await;
        let stream = response.result_as::<StreamOpened>().unwrap().stream_id;
        let (events, end) = c.end(stream).await;
        assert_eq!(events, [], "{params}");
        match end.outcome {
            StreamOutcome::Failed { error } => {
                assert_eq!(error.code, code::INVALID_PARAMS, "{params}")
            }
            other => panic!("{params}: {other:?}"),
        }
    }
    let response = c
        .call("terminal.input", json!({"terminalId": 99, "data": "x"}))
        .await;
    assert_eq!(response.outcome.unwrap_err().code, code::INVALID_PARAMS);
    // 이미 끝난 터미널을 닫는 것은 오류가 아니다.
    c.call("terminal.close", json!({"terminalId": 99}))
        .await
        .outcome
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn resize_changes_the_size_the_shell_sees() {
    let f = Fixture::new("schema = 1\n");
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    c.input(t.terminal, "stty size\n").await;
    c.read_until(t.stream, "24 80").await;
    c.call(
        "terminal.resize",
        json!({"terminalId": t.terminal, "cols": 100, "rows": 40}),
    )
    .await
    .outcome
    .unwrap();
    c.input(t.terminal, "stty size\n").await;
    c.read_until(t.stream, "40 100").await;
    let response = c
        .call(
            "terminal.resize",
            json!({"terminalId": t.terminal, "cols": 0, "rows": 40}),
        )
        .await;
    assert_eq!(response.outcome.unwrap_err().code, code::INVALID_PARAMS);
}

#[tokio::test(flavor = "multi_thread")]
async fn exited_is_the_last_item_with_the_exit_code() {
    let f = Fixture::new("schema = 1\n");
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    c.input(t.terminal, "exit 3\n").await;
    let (code, end) = c.exit(t.stream).await;
    assert_eq!(code, Some(3));
    assert_eq!(end.outcome, StreamOutcome::Completed);
    // 끝난 터미널은 목록에서 빠진다.
    assert!(f.terminals.pty(t.terminal).is_none());
    assert!(!alive(t.pid));
}

/// 셸과 그 자식 둘(백그라운드 작업 — 하나는 SIGHUP 을 무시한다)을 띄우고, 끝낸 뒤 셋의 pid 가 1초 안에 사라지는지 본다.
async fn start_shell_with_children(c: &mut Client, f: &Fixture) -> (Opened, Vec<u32>) {
    let t = c.open(f.create_params(80, 24)).await;
    c.input(
        t.terminal,
        // `$!` 뒤에 공백을 둔다 — bash(macOS 의 sh)의 대화형 히스토리 확장이 `!"` · `!;` 를 이벤트로 읽는다.
        "sleep 100 & echo BG=$! ; (trap '' HUP; exec sleep 100) & echo BG=$! \n",
    )
    .await;
    let mut text = String::new();
    while background_pids(&text).len() < 2 {
        text.push_str(&c.read_until(t.stream, "BG=").await);
    }
    let mut pids = background_pids(&text);
    pids.push(t.pid);
    for &pid in &pids {
        assert!(alive(pid), "{pid} is not running");
    }
    (t, pids)
}

async fn assert_gone_within_a_second(pids: &[u32], since: Instant) {
    while pids.iter().any(|&p| alive(p)) {
        assert!(
            since.elapsed() < Duration::from_secs(1),
            "still running after a second: {:?}",
            pids.iter().filter(|&&p| alive(p)).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    eprintln!("{pids:?} gone after {:?}", since.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_connection_ends_the_shell_and_its_children_within_a_second() {
    let f = Fixture::new("schema = 1\n");
    let mut c = f.client();
    let (t, pids) = start_shell_with_children(&mut c, &f).await;
    let closed = Instant::now();
    c.session.close();
    assert_gone_within_a_second(&pids, closed).await;
    // 세션이 버린 스트림의 터미널도 목록에서 빠진다.
    let deadline = Instant::now() + Duration::from_secs(1);
    while f.terminals.pty(t.terminal).is_some() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_close_ends_the_tree_and_the_stream_reports_a_signal_exit() {
    let f = Fixture::new("schema = 1\n");
    let mut c = f.client();
    let (t, pids) = start_shell_with_children(&mut c, &f).await;
    let closed = Instant::now();
    c.call("terminal.close", json!({"terminalId": t.terminal}))
        .await
        .outcome
        .unwrap();
    let (code, end) = c.exit(t.stream).await;
    assert_eq!(code, None);
    assert_eq!(end.outcome, StreamOutcome::Completed);
    assert_gone_within_a_second(&pids, closed).await;
}

/// 소비자가 출력을 읽지 않아 tty 가 꽉 찬 채로 닫아도 셸의 프로세스가 1초 안에 끝난다. 끝낸 뒤 PTY 를 더 읽지 않으면 tty 를
/// 닫는 프로세스가 출력이 비워지기를 기다리며 끝나는 중(E · U)에 멈춘다.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_terminal_with_unread_output_ends_its_processes() {
    let f = Fixture::new(
        "schema = 1\n[terminal]\nshell = [\"/bin/sh\", \"-c\", \"yes 0123456789 & echo BG=$! ; exec yes\"]\n",
    );
    let mut c = f.client();
    let response = c.call("terminal.create", f.create_params(80, 24)).await;
    let stream = response.result_as::<StreamOpened>().unwrap().stream_id;
    c.credit(stream, 2);
    let TerminalEvent::Started {
        terminal_id, pid, ..
    } = c.next_event(stream).await
    else {
        panic!("expected started");
    };
    let TerminalEvent::Output { data } = c.next_event(stream).await else {
        panic!("expected output");
    };
    let text = String::from_utf8_lossy(&STANDARD.decode(data).unwrap()).into_owned();
    let mut pids = background_pids(&text);
    assert_eq!(pids.len(), 1, "{text:?}");
    pids.push(pid.unwrap());
    let pty = f.terminals.pty(terminal_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while pty.peak_buffered() < OUTPUT_BUFFER {
        assert!(Instant::now() < deadline, "the buffer did not fill");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // 커널의 PTY 버퍼도 찰 때까지.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let closed = Instant::now();
    c.call("terminal.close", json!({"terminalId": terminal_id}))
        .await
        .outcome
        .unwrap();
    assert_gone_within_a_second(&pids, closed).await;
}

/// 10 MB 를 한 청크씩 credit 을 주며 천천히 읽는다. 바이트가 그대로 오고, Host 버퍼는 상한을 넘지 않되 그 근처까지 찬다(백프레셔가
/// 실제로 걸렸다).
#[tokio::test(flavor = "multi_thread")]
async fn ten_megabytes_reach_a_slow_reader_intact_within_the_buffer_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("payload");
    let mut seed: u32 = 0x2545_f491;
    let payload: Vec<u8> = (0..10 * 1024 * 1024)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed.to_le_bytes()[0]
        })
        .collect();
    fs::write(&file, &payload).unwrap();
    // raw: 출력 후처리(`\n` → `\r\n`)를 끈다. 바이트를 그대로 비교한다.
    let f = Fixture::new(&format!(
        "schema = 1\n[terminal]\nshell = [\"/bin/sh\", \"-c\", \"stty raw -echo && exec cat '{}'\"]\n",
        file.display()
    ));
    let mut c = f.client();
    let response = c.call("terminal.create", f.create_params(80, 24)).await;
    let stream = response.result_as::<StreamOpened>().unwrap().stream_id;
    c.credit(stream, 1);
    let TerminalEvent::Started { terminal_id, .. } = c.next_event(stream).await else {
        panic!("expected started");
    };
    let pty = f.terminals.pty(terminal_id).unwrap();
    let mut received = Vec::with_capacity(payload.len());
    let mut chunks = 0;
    let code = loop {
        c.credit(stream, 1);
        match c.next_event(stream).await {
            TerminalEvent::Output { data } => {
                let bytes = STANDARD.decode(data).unwrap();
                assert!(bytes.len() <= CHUNK);
                received.extend_from_slice(&bytes);
                chunks += 1;
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
            TerminalEvent::Exited { code } => break code,
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(code, Some(0));
    assert_eq!(received.len(), payload.len(), "after {chunks} chunks");
    assert!(received == payload, "the bytes differ");
    let peak = pty.peak_buffered();
    eprintln!("{chunks} chunks, peak host buffer {peak} bytes");
    assert!(peak <= OUTPUT_BUFFER, "peak {peak}");
    assert!(
        peak >= OUTPUT_BUFFER / 2,
        "peak {peak}: the reader never waited for the consumer"
    );
}

/// 로그인 셸이 시간을 넘기면 그 첫 터미널만 기다린다. 냉각(30초) 동안도, 냉각이 끝난 뒤(재시도는 백그라운드)도 터미널은
/// 기다리지 않고 앱 프로세스 환경을 받는다.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_first_terminal_waits_for_a_hanging_login_shell() {
    let timeout = Duration::from_millis(600);
    let cooldown = Duration::from_millis(300);
    let f = Fixture::with_login_shell(
        "schema = 1\n[terminal]\nshell = [\"/bin/sh\"]\n",
        HANGING,
        timeout,
        cooldown,
    );
    let mut c = f.client();
    let started = Instant::now();
    c.open(f.create_params(80, 24)).await;
    assert!(started.elapsed() >= timeout, "{:?}", started.elapsed());
    let quick = Duration::from_millis(400);
    // 냉각 중, 냉각이 끝난 직후(재시도가 시작된다), 재시도도 시간을 넘긴 뒤 다시 냉각이 끝났을 때.
    for wait in [
        Duration::ZERO,
        cooldown + Duration::from_millis(50),
        timeout + cooldown + Duration::from_millis(300),
    ] {
        tokio::time::sleep(wait).await;
        let started = Instant::now();
        c.open(f.create_params(80, 24)).await;
        assert!(
            started.elapsed() < quick,
            "after sleeping {wait:?} the terminal waited {:?}",
            started.elapsed()
        );
    }
}

/// 소비자가 읽지 않으면 터미널 하나가 쌓는 출력 — Host 버퍼와 credit 을 받아 내보낸 청크의 합 — 이 기본 window(32)에서
/// 1 MiB 를 넘지 않는다(credit 은 바이트가 아니라 청크 수다).
#[tokio::test(flavor = "multi_thread")]
async fn unread_output_stays_within_a_megabyte_with_the_default_window() {
    let f = Fixture::new(
        "schema = 1\n[terminal]\nshell = [\"/bin/sh\", \"-c\", \"exec yes 0123456789\"]\n",
    );
    let mut c = f.client();
    let response = c.call("terminal.create", f.create_params(80, 24)).await;
    let stream = response.result_as::<StreamOpened>().unwrap().stream_id;
    c.credit(stream, 1);
    let TerminalEvent::Started { terminal_id, .. } = c.next_event(stream).await else {
        panic!("expected started");
    };
    let pty = f.terminals.pty(terminal_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while pty.peak_buffered() < OUTPUT_BUFFER {
        assert!(
            Instant::now() < deadline,
            "the buffer did not fill: {}",
            pty.peak_buffered()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // 클라이언트가 window 만큼 credit 을 주고 읽지 않는다(TS 클라이언트가 처음 주는 양).
    c.credit(stream, u32::try_from(CREDIT_WINDOW).unwrap());
    let mut sent = 0;
    for _ in 0..CREDIT_WINDOW {
        match c.next_event(stream).await {
            TerminalEvent::Output { data } => sent += STANDARD.decode(data).unwrap().len(),
            other => panic!("unexpected {other:?}"),
        }
    }
    // credit 이 없으니 더 나가지 않고, 버퍼는 다시 상한까지 찬다.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(c.rx.try_recv().is_err(), "a chunk went out without credit");
    let peak = pty.peak_buffered();
    eprintln!("sent {sent} bytes in {CREDIT_WINDOW} chunks, peak host buffer {peak} bytes");
    assert!(peak <= OUTPUT_BUFFER, "peak {peak}");
    assert!(
        sent + peak <= OUTPUT_LIMIT,
        "{sent} sent + {peak} buffered > {OUTPUT_LIMIT}"
    );
}

/// 앱 종료 흉내(시험 [`exiting_app_ends_terminal_processes_that_ignore_sighup`] 가 이 시험 바이너리를 자식으로 띄울 때만 돈다):
/// 터미널에서 자식 셋(보통 · SIGHUP 무시 · nohup)을 띄워 pid 를 찍고, `PW_RUN_EXIT_PROBE=shutdown` 이면 [`crate::shutdown`] 을
/// 부른 뒤 Tauri 처럼 `std::process::exit` 로 끝난다(Drop 이 돌지 않는다).
#[test]
fn exit_probe() {
    let Ok(mode) = std::env::var("PW_RUN_EXIT_PROBE") else {
        return;
    };
    let pty = Pty::spawn(&Spec {
        argv: [
            "/bin/sh",
            "-c",
            "sleep 100 & echo BG=$! ; (trap '' HUP; exec sleep 100) & echo BG=$! ; nohup sleep 100 >/dev/null 2>&1 & echo BG=$! ; echo DONE; wait",
        ]
        .map(OsString::from)
        .to_vec(),
        cwd: std::env::temp_dir(),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        cols: 80,
        rows: 24,
    })
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut text = String::new();
    while !text.contains("DONE") {
        let bytes = runtime
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(10), pty.peek(CHUNK)).await
            })
            .unwrap()
            .unwrap();
        pty.consume(bytes.len());
        text.push_str(&String::from_utf8_lossy(&bytes));
    }
    let mut pids = background_pids(&text);
    pids.push(pty.pid().unwrap());
    println!(
        "PIDS={}",
        pids.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    if mode == "shutdown" {
        crate::shutdown();
    }
    std::process::exit(0);
}

/// 앱이 끝날 때(Drop 없이 `std::process::exit`) [`crate::shutdown`] 을 부르면 셸과 그 자식 — SIGHUP 을 무시하는 것 · nohup 포함 —
/// 이 1초 안에 사라진다. 부르지 않으면 커널의 hangup 만 가서 둘이 남는다(리뷰의 실측).
#[test]
fn exiting_app_ends_terminal_processes_that_ignore_sighup() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["tests::exit_probe", "--exact", "--nocapture"])
        .env("PW_RUN_EXIT_PROBE", "shutdown")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let exited = Instant::now();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pids: Vec<u32> = stdout
        .lines()
        .find_map(|l| l.strip_prefix("PIDS="))
        .unwrap_or_else(|| {
            panic!(
                "no pids in {stdout:?} {:?}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .split(',')
        .map(|p| p.parse().unwrap())
        .collect();
    assert_eq!(pids.len(), 4, "{pids:?}");
    let left = loop {
        let left: Vec<u32> = pids.iter().copied().filter(|&p| alive(p)).collect();
        if left.is_empty() || exited.elapsed() > Duration::from_secs(1) {
            break left;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    for &pid in &left {
        // SAFETY: 이 시험이 띄운 sleep 들이다.
        unsafe { libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL) };
    }
    assert_eq!(
        left,
        Vec::<u32>::new(),
        "still running a second after the app exited"
    );
}

/// 응답을 기다리지 않고 연달아 보낸 입력이 보낸 순서대로 셸에 들어간다(입력은 핸들러의 동기 구간에서 큐에 들어간다).
#[tokio::test(flavor = "multi_thread")]
async fn inputs_reach_the_shell_in_the_order_they_were_sent() {
    // raw: 받은 바이트를 cat 이 그대로 돌려준다.
    let f = Fixture::new(
        "schema = 1\n[terminal]\nshell = [\"/bin/sh\", \"-c\", \"stty raw -echo && echo READY && exec cat\"]\n",
    );
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    // raw 가 걸리기 전에 보내면 줄 버퍼(MAX_CANON)가 넘친다.
    c.read_until(t.stream, "READY\n").await;
    let sent: String = (0..300).map(|i| format!("{i:04},")).collect();
    for i in 0..300 {
        c.request(
            "terminal.input",
            json!({"terminalId": t.terminal, "data": format!("{i:04},")}),
        );
    }
    let text = c.read_until(t.stream, "0299,").await;
    assert_eq!(text, sent);
}
