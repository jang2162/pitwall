//! 실행 관리자 시험(17-m6-plan M6-02 완료 기준 ① ~ ⑦). 실제 PTY 로 띄우는 시험은 가짜 로그인 셸(터미널 시험과 같다 — 사람의 셸
//! 시작 파일은 돌지 않는다)의 환경을 쓰고, 버퍼 · 범위 · 신뢰 시험은 가짜 실행기로 프로세스 없이 본다.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Filter, Received, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::agent::agent_grant;
use pw_core::registry::exec::{Launch, agent_run_grant, exec_grant};
use pw_core::registry::plan::{ApprovalTicket, ToolPlan, approve};
use pw_core::registry::{NewProject, Registry as Projects};
use pw_rpc::RpcError;
use pw_rpc::protocol::{
    STREAM_CHUNK, STREAM_CREDIT, STREAM_END, StreamChunk, StreamEnd, StreamOpened, StreamOutcome,
};
use pw_rpc::{
    Caller, ConnectionScope, EventHub, Id, Outbound, Response, Role, Router, Session, Tier,
};
use pw_runtime::LoginShell;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use super::methods::RunAttachEvent;
use super::*;

const KIB: usize = 1024;

// ---- 가짜 실행기 --------------------------------------------------------------------------------------------------------

/// 띄운 것을 적고, 출력 · 끝을 시험이 직접 낸다. 중지는 곧바로 끝낸다.
#[derive(Default)]
struct Fake {
    calls: AtomicUsize,
    /// 띄우는 자리에서 곧바로 끝낸다(띄우기가 돌려주기 전에 끝나는 프로그램).
    exit_at_launch: std::sync::atomic::AtomicBool,
    launched: Mutex<Vec<(ProcessSpec, Arc<FakeRun>)>>,
}

struct FakeRun {
    output: RunOutput,
    stops: AtomicUsize,
    inputs: Mutex<Vec<Vec<u8>>>,
}

impl ProcessLauncher for Fake {
    fn launch(&self, spec: &ProcessSpec, output: RunOutput) -> Result<Arc<dyn RunKind>, RpcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let run = Arc::new(FakeRun {
            output,
            stops: AtomicUsize::new(0),
            inputs: Mutex::default(),
        });
        lock(&self.launched).push((spec.clone(), Arc::clone(&run)));
        if self.exit_at_launch.load(Ordering::SeqCst) {
            run.output.write(b"bye\n");
            run.output.exited(RunExit {
                code: Some(0),
                signal: None,
            });
        }
        Ok(run)
    }
}

impl RunKind for FakeRun {
    fn kind(&self) -> &'static str {
        process::KIND
    }

    fn input(&self, data: Vec<u8>) -> Result<(), String> {
        lock(&self.inputs).push(data);
        Ok(())
    }

    fn stop(&self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.output.exited(RunExit {
            code: None,
            signal: Some("fake".into()),
        });
    }
}

impl Fake {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn last(&self) -> (ProcessSpec, Arc<FakeRun>) {
        lock(&self.launched)
            .last()
            .cloned()
            .expect("nothing launched")
    }
}

// ---- 픽스처 --------------------------------------------------------------------------------------------------------------

struct Fixture {
    tmp: tempfile::TempDir,
    runs: Runs,
    projects: Projects,
    bus: EventBus,
    fake: Option<Arc<Fake>>,
    /// 신뢰한 프로젝트.
    project: String,
    root: PathBuf,
    /// 가짜 로그인 셸의 PATH 맨 앞 폴더.
    bin: PathBuf,
}

impl Fixture {
    /// 실제 PTY 로 띄운다.
    fn real() -> Self {
        Self::build(None, RunLimits::default())
    }

    fn fake(limits: RunLimits) -> Self {
        Self::build(Some(Arc::new(Fake::default())), limits)
    }

    fn build(fake: Option<Arc<Fake>>, limits: RunLimits) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let projects = Projects::new(paths.clone(), None, bus.clone());
        let home = tmp.path().join("home");
        let bin = normalize(&tmp.path().join("bin")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let shell = tmp.path().join("fake-login-shell");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nHOME='{}'; export HOME\nunset LANG LC_ALL LC_CTYPE\nPATH='{}':/usr/bin:/bin; export PATH\nSHELL=/bin/sh; export SHELL\n[ -f '{hook}' ] && . '{hook}'\neval \"$4\"\n",
                home.display(),
                bin.display(),
                hook = tmp.path().join("login-hook.sh").display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = Arc::new(Runtime::new(
            LoginShell::new(Some(shell), Duration::from_secs(5))
                .with_cooldown(pw_runtime::login_shell::TIMEOUT_COOLDOWN),
        ));
        let launcher: Arc<dyn ProcessLauncher> = match &fake {
            Some(fake) => Arc::clone(fake) as Arc<dyn ProcessLauncher>,
            None => Arc::new(PtyLauncher),
        };
        let runs = Runs::with(runtime, launcher, limits);
        runs.attach(HostParts {
            paths,
            projects: projects.clone(),
            bus: bus.clone(),
        });
        let mut f = Self {
            tmp,
            runs,
            projects,
            bus,
            fake,
            project: String::new(),
            root: PathBuf::new(),
            bin,
        };
        let (project, root) = f.project("project", true);
        f.project = project;
        f.root = root;
        f
    }

    /// 프로젝트 하나를 만든다.
    fn project(&self, name: &str, trusted: bool) -> (String, PathBuf) {
        let root = self.tmp.path().join(name);
        fs::create_dir_all(&root).unwrap();
        let id = self
            .projects
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        if trusted {
            self.projects.set_trust(&id, true).unwrap();
        }
        (id, normalize(&root).unwrap())
    }

    /// `root/.pitwall/run/<id>.toml` 을 쓴다(`body` 는 `schema` 뒤의 내용).
    fn config(&self, root: &Path, id: &str, body: &str) {
        let dir = root.join(".pitwall/run");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{id}.toml")),
            format!("schema = 1\n{body}\n"),
        )
        .unwrap();
    }

    fn shell_config(&self, id: &str, command: &str) {
        self.config(
            &self.root.clone(),
            id,
            &format!(
                "name = \"{id}\"\ntype = \"shell\"\ncommand = {}",
                toml_str(command)
            ),
        );
    }

    fn fake_launcher(&self) -> &Fake {
        self.fake.as_deref().expect("a fake launcher")
    }

    fn client(&self) -> Client {
        self.client_in(ConnectionScope::Project(self.project.clone()))
    }

    fn client_in(&self, scope: ConnectionScope) -> Client {
        self.client_as(Caller {
            role: Role::Owner,
            scope,
            window: Some("project-window".into()),
        })
    }

    fn client_as(&self, caller: Caller) -> Client {
        let mut router = Router::new();
        methods::register(&mut router, self.runs.clone());
        let (session, rx) = Session::new(Arc::new(router), EventHub::new(), caller);
        Client {
            session,
            rx,
            next_id: 1,
            responses: Vec::new(),
            events: Vec::new(),
            ended: Vec::new(),
        }
    }

    /// 이 프로젝트 창의 허가로 구성 `id` 를 띄운다(가짜 실행기 시험).
    fn start(&self, id: &str) -> RunInfo {
        let grant = exec_grant(
            &ConnectionScope::Project(self.project.clone()),
            &self.project,
            &Launch::Process,
        )
        .unwrap();
        self.runs
            .start(
                &grant,
                None,
                &RunStartParams {
                    project_id: self.project.clone(),
                    config_id: id.into(),
                    ..RunStartParams::default()
                },
            )
            .unwrap()
    }

    /// 승인을 지난 에이전트 허가(실행 내용 `spec` 하나).
    fn agent_grant(&self, project: &str, spec: RunSpec) -> AgentRunGrant {
        let agent = agent_grant(&ConnectionScope::Agent, &self.projects, project).unwrap();
        let plan = ToolPlan::builder(
            "run.tool.start",
            Tier::Exec,
            &json!({"configId": spec.config}),
        )
        .summary("run")
        .runs([spec])
        .build();
        let ticket = ApprovalTicket {
            hash: plan.hash.clone(),
        };
        agent_run_grant(&agent, &approve(Some(&ticket), plan).unwrap()).unwrap()
    }
}

fn toml_str(text: &str) -> String {
    toml_edit::Value::from(text).to_string()
}

// ---- RPC 클라이언트 ------------------------------------------------------------------------------------------------------

struct Client {
    session: Session,
    rx: UnboundedReceiver<Outbound>,
    next_id: i64,
    responses: Vec<Response>,
    events: Vec<(u64, RunAttachEvent)>,
    ended: Vec<StreamEnd>,
}

/// 붙은 스트림에서 받은 것.
struct Attached {
    stream: u64,
    from: u64,
    dropped: u64,
    info: RunInfo,
    text: String,
    /// 받은 출력의 끝 오프셋.
    end: u64,
}

impl Client {
    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

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

    async fn call(&mut self, method: &str, params: Value) -> Response {
        let id = self.request(method, params);
        loop {
            if let Some(i) = self.responses.iter().position(|r| r.id == Id::Number(id)) {
                return self.responses.remove(i);
            }
            self.receive(Duration::from_secs(20)).await;
        }
    }

    async fn ok<T: serde::de::DeserializeOwned>(&mut self, method: &str, params: Value) -> T {
        let response = self.call(method, params).await;
        response
            .result_as()
            .unwrap_or_else(|e| panic!("{method}: {e:?}"))
    }

    /// 오류. 스트림 메서드는 스트림을 연 뒤 `stream.end` 의 실패로 온다.
    async fn error(&mut self, method: &str, params: Value) -> pw_rpc::RpcError {
        let opened = match self.call(method, params).await.outcome {
            Err(e) => return e,
            Ok(v) => v,
        };
        let Some(stream) = opened.get("streamId").and_then(Value::as_u64) else {
            panic!("{method} succeeded: {opened}");
        };
        loop {
            if let Some(end) = self.ended.iter().find(|e| e.stream_id == stream) {
                match &end.outcome {
                    StreamOutcome::Failed { error } => return error.clone(),
                    other => panic!("{method} stream ended with {other:?}"),
                }
            }
            if let Some(event) = self.events.iter().find(|(s, _)| *s == stream) {
                panic!("{method} streamed {:?}", event.1);
            }
            self.receive(Duration::from_secs(10)).await;
        }
    }

    async fn next_event(&mut self, stream: u64) -> RunAttachEvent {
        loop {
            if let Some(i) = self.events.iter().position(|(s, _)| *s == stream) {
                return self.events.remove(i).1;
            }
            if let Some(end) = self.ended.iter().find(|e| e.stream_id == stream) {
                panic!("stream {stream} ended: {end:?}");
            }
            self.receive(Duration::from_secs(30)).await;
        }
    }

    /// `run.attach` 를 열고 첫 항목(`attached`)을 받는다.
    async fn attach(&mut self, run_id: &str, since: Option<u64>) -> Attached {
        let opened: StreamOpened = self
            .ok("run.attach", json!({"runId": run_id, "since": since}))
            .await;
        self.session.handle_value(json!({
            "jsonrpc": "2.0", "method": STREAM_CREDIT,
            "params": {"streamId": opened.stream_id, "credit": 1_000_000}
        }));
        match self.next_event(opened.stream_id).await {
            RunAttachEvent::Attached { run, from, dropped } => Attached {
                stream: opened.stream_id,
                from,
                dropped,
                info: *run,
                text: String::new(),
                end: from,
            },
            other => panic!("expected attached, got {other:?}"),
        }
    }

    /// 출력에 `wanted` 가 나올 때까지 읽는다. 출력의 오프셋은 이어져야 한다(빈틈이 없다).
    async fn read_until(&mut self, a: &mut Attached, wanted: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !a.text.contains(wanted) {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(event) = tokio::time::timeout(left, self.next_event(a.stream)).await else {
                panic!("`{wanted}` did not appear in {:?}", a.text);
            };
            match event {
                RunAttachEvent::Output { offset, data } => {
                    assert_eq!(offset, a.end, "a gap in the output after {:?}", a.text);
                    let bytes = STANDARD.decode(data).unwrap();
                    a.end = offset + bytes.len() as u64;
                    a.text.push_str(&String::from_utf8_lossy(&bytes));
                }
                other => panic!("expected output, got {other:?} after {:?}", a.text),
            }
        }
    }

    /// `exited` 까지 읽어 끝난 꼴을 준다.
    async fn exit(&mut self, a: &mut Attached) -> RunExit {
        loop {
            match self.next_event(a.stream).await {
                RunAttachEvent::Output { offset, data } => {
                    let bytes = STANDARD.decode(data).unwrap();
                    a.end = offset + bytes.len() as u64;
                    a.text.push_str(&String::from_utf8_lossy(&bytes));
                }
                RunAttachEvent::Exited { exit } => return exit,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn alive(pid: u32) -> bool {
    let pid = libc::pid_t::try_from(pid).unwrap();
    // SAFETY: 신호 0 은 존재만 본다.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// 출력의 `BG=<pid>` 하나.
fn background_pid(text: &str) -> u32 {
    let rest = text.split("BG=").nth(1).expect("no BG= in the output");
    rest.chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

fn kill(pid: u32) {
    // SAFETY: 이 시험이 띄운 프로세스다.
    unsafe { libc::kill(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL) };
}

/// `pid` 가 `within` 안에 사라지는가.
fn gone_within(pid: u32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    !alive(pid)
}

fn info(response: Response) -> RunInfo {
    response.result_as().unwrap()
}

// ---- ① 손자 프로세스까지 끝낸다 ------------------------------------------------------------------------------------------

/// 완료 기준 ①: 자식이 띄운 백그라운드 `sleep`(손자)까지 중지 뒤 남지 않는다 — 세션 전체에 신호를 보낸다. 손자는 SIGHUP 을
/// 무시한다 — 리더(자식 pid)만 죽이면 커널이 보내는 hangup 으로는 끝나지 않아 손자가 남고 실패한다.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_run_ends_its_grandchildren() {
    let f = Fixture::real();
    f.shell_config("dev", "(trap '' HUP; exec sleep 300) & echo BG=$!; wait");
    let mut c = f.client();
    let run = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "dev"}),
        )
        .await,
    );
    assert_eq!(run.state, RunState::Running);
    assert_eq!(run.kind, "process");
    assert_eq!(
        run.launched_by,
        LaunchedBy::Window {
            window: Some("project-window".into())
        }
    );
    let mut a = c.attach(&run.run_id, None).await;
    c.read_until(&mut a, "BG=").await;
    c.read_until(&mut a, "\n").await;
    let grandchild = background_pid(&a.text);
    let leader = run.pid.unwrap();
    assert!(alive(grandchild) && alive(leader));

    let stopped = info(c.call("run.stop", json!({"runId": run.run_id})).await);
    assert_ne!(stopped.state, RunState::Running);
    let exit = tokio::time::timeout(Duration::from_secs(10), c.exit(&mut a))
        .await
        .expect("the run did not end");
    assert_eq!(exit.code, None, "{exit:?}");
    let left_grandchild = !gone_within(grandchild, Duration::from_millis(500));
    let left_leader = !gone_within(leader, Duration::from_millis(500));
    if left_grandchild {
        kill(grandchild);
    }
    if left_leader {
        kill(leader);
    }
    assert!(!left_grandchild, "the background sleep outlived the stop");
    assert!(!left_leader, "the shell outlived the stop");
    let listed: methods::RunList = c.ok("run.list", json!({"projectId": f.project})).await;
    assert_eq!(listed.runs.len(), 1);
    assert_eq!(listed.runs[0].state, RunState::Exited);
    assert_eq!(listed.runs[0].exit, Some(exit));
}

/// 리더가 스스로 끝나도 세션에 두고 간 프로세스는 남지 않는다(실행이 끝났는데 자식이 포트를 쥐고 남지 않게). 그 정리는 중지와
/// 같은 SIGTERM 이고 `exited` 는 정리가 끝난 **뒤**에 나간다 — 손자가 SIGTERM 을 받고 남긴 표지가 `exited` 를 받은 때 이미 있다
/// (정리를 걷으면 `Pty` 의 Drop 이 SIGHUP → SIGKILL 로 대신 죽여 표지가 없다).
#[tokio::test(flavor = "multi_thread")]
async fn a_finished_run_leaves_no_background_process_behind() {
    let f = Fixture::real();
    let (mark, ready) = (f.tmp.path().join("got-term"), f.tmp.path().join("ready"));
    // 리더는 손자가 신호 처리를 건 뒤에 끝난다 — 먼저 끝나면 커널이 보내는 SIGHUP 에 손자가 그 전에 죽는다.
    f.shell_config(
        "bg",
        &format!(
            "(trap '' HUP; trap 'touch {mark}; exit' TERM; touch {ready}; while :; do sleep 0.1; done) & echo BG=$!; while [ ! -f {ready} ]; do sleep 0.01; done",
            mark = mark.display(),
            ready = ready.display(),
        ),
    );
    let mut c = f.client();
    let run = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "bg"}),
        )
        .await,
    );
    let mut a = c.attach(&run.run_id, None).await;
    let exit = tokio::time::timeout(Duration::from_secs(10), c.exit(&mut a))
        .await
        .expect("the run did not end");
    assert_eq!(exit.code, Some(0), "{exit:?} {:?}", a.text);
    let terminated_first = mark.exists();
    let grandchild = background_pid(&a.text);
    let left = !gone_within(grandchild, Duration::from_millis(500));
    if left {
        kill(grandchild);
    }
    assert!(!left, "the background loop outlived the run");
    assert!(
        terminated_first,
        "`exited` went out before the session got SIGTERM"
    );
}

// ---- ② SIGTERM 을 무시하면 5초 뒤 SIGKILL ---------------------------------------------------------------------------------

/// 완료 기준 ②: SIGTERM 을 무시하는 자식은 [`RunLimits::stop_grace`](5초) 뒤 SIGKILL 로 끝난다.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_ignoring_sigterm_is_killed_after_five_seconds() {
    let f = Fixture::real();
    assert_eq!(f.runs.limits().stop_grace, Duration::from_secs(5));
    f.shell_config(
        "stubborn",
        "trap '' TERM; echo READY; while :; do sleep 1; done",
    );
    let mut c = f.client();
    let run = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "stubborn"}),
        )
        .await,
    );
    let mut a = c.attach(&run.run_id, None).await;
    c.read_until(&mut a, "READY").await;
    let leader = run.pid.unwrap();
    let asked = Instant::now();
    c.ok::<RunInfo>("run.stop", json!({"runId": run.run_id}))
        .await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(alive(leader), "SIGTERM should have been ignored");
    let exit = tokio::time::timeout(Duration::from_secs(10), c.exit(&mut a)).await;
    let elapsed = asked.elapsed();
    if alive(leader) {
        kill(leader);
    }
    let exit = exit.expect("the run was not killed");
    assert!(elapsed >= Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(exit.code, None);
    assert!(
        exit.signal.as_deref().is_some_and(|s| s.contains("Kill")),
        "{exit:?}"
    );
}

// ---- ③ 링 버퍼 상한 -----------------------------------------------------------------------------------------------------

/// 완료 기준 ③(실행 중): 8 MiB 링에 9 MiB 를 쓰면 앞 1 MiB 가 사라지고 `since` 오프셋이 그만큼 간다 — `run.attach { since: 0 }`
/// 은 1 MiB 부터 보내고 버려진 1 MiB 를 알린다.
#[tokio::test(flavor = "multi_thread")]
async fn nine_mib_into_a_running_run_drops_the_first_mib() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("big", "true");
    let run = f.start("big");
    let (_, fake) = f.fake_launcher().last();
    let chunk = vec![b'x'; 64 * KIB];
    for _ in 0..(9 * 1024 * KIB / chunk.len()) {
        fake.output.write(&chunk);
    }
    let mut c = f.client();
    let a = c.attach(&run.run_id, Some(0)).await;
    assert_eq!(a.from, (1024 * KIB) as u64);
    assert_eq!(a.dropped, (1024 * KIB) as u64);
    assert_eq!(a.info.log_start, (1024 * KIB) as u64);
    assert_eq!(a.info.log_end, (9 * 1024 * KIB) as u64);
    assert_eq!(f.runs.log_bytes(), 8 * 1024 * KIB);
}

fn small_limits() -> RunLimits {
    RunLimits {
        running: 64 * KIB,
        finished: 8 * KIB,
        finished_per_project: 20,
        host_total: 20 * KIB,
        ..RunLimits::default()
    }
}

/// 가짜 실행 하나를 띄워 `bytes` 를 쓰고 끝낸다.
fn finished_run(f: &Fixture, bytes: usize) -> RunInfo {
    let run = f.start("job");
    let (_, fake) = f.fake_launcher().last();
    fake.output.write(&vec![b'y'; bytes]);
    fake.output.exited(RunExit {
        code: Some(0),
        signal: None,
    });
    run
}

/// 완료 기준 ③(끝난 실행 · Host 총량): 끝난 실행은 [`RunLimits::finished`] 로 줄고, Host 총량을 넘으면 가장 오래 끝난 실행이
/// 목록에서 빠진다(총량 판정을 걷으면 셋 다 남아 실패).
#[tokio::test(flavor = "multi_thread")]
async fn finished_runs_shrink_and_the_oldest_goes_over_the_host_total() {
    let f = Fixture::fake(small_limits());
    f.shell_config("job", "true");
    let first = finished_run(&f, 10 * KIB);
    let listed = f.runs.list(&f.project);
    assert_eq!(listed[0].log_end, (10 * KIB) as u64);
    assert_eq!(
        listed[0].log_start,
        (2 * KIB) as u64,
        "shrunk to the last 8 KiB"
    );
    assert_eq!(f.runs.log_bytes(), 8 * KIB);
    let second = finished_run(&f, 10 * KIB);
    // 8 + 8 = 16 KiB ≤ 20 KiB — 둘 다 남는다.
    assert_eq!(f.runs.list(&f.project).len(), 2);
    let third = finished_run(&f, 10 * KIB);
    let ids: Vec<String> = f
        .runs
        .list(&f.project)
        .into_iter()
        .map(|r| r.run_id)
        .collect();
    assert_eq!(
        ids,
        [second.run_id, third.run_id],
        "the oldest finished run goes first"
    );
    assert_eq!(f.runs.log_bytes(), 16 * KIB);
    let mut c = f.client();
    let e = c.error("run.attach", json!({"runId": first.run_id})).await;
    assert_eq!(e.code, code::NOT_FOUND);
}

/// 실행 중인 실행의 출력이 Host 총량을 넘겨도 실행 중인 것은 버리지 않고 끝난 것부터 뺀다.
#[tokio::test(flavor = "multi_thread")]
async fn running_output_over_the_host_total_drops_finished_runs_first() {
    let f = Fixture::fake(small_limits());
    f.shell_config("job", "true");
    let old = finished_run(&f, 8 * KIB);
    let running = f.start("job");
    let (_, fake) = f.fake_launcher().last();
    fake.output.write(&vec![b'z'; 16 * KIB]);
    let ids: Vec<String> = f
        .runs
        .list(&f.project)
        .into_iter()
        .map(|r| r.run_id)
        .collect();
    assert_eq!(ids, [running.run_id]);
    assert!(!ids.contains(&old.run_id));
}

/// 프로젝트마다 끝난 실행은 최근 [`RunLimits::finished_per_project`] 개만 남는다. 다른 프로젝트의 것은 세지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn each_project_keeps_its_latest_finished_runs() {
    let f = Fixture::fake(RunLimits {
        finished_per_project: 2,
        ..small_limits()
    });
    f.shell_config("job", "true");
    let first = finished_run(&f, 10);
    let second = finished_run(&f, 10);
    let third = finished_run(&f, 10);
    let ids: Vec<String> = f
        .runs
        .list(&f.project)
        .into_iter()
        .map(|r| r.run_id)
        .collect();
    assert_eq!(ids, [second.run_id, third.run_id]);
    assert!(!ids.contains(&first.run_id));
}

// ---- ④ 끊었다 다시 붙기 ---------------------------------------------------------------------------------------------------

/// 완료 기준 ④: 연결을 끊었다 다른 연결로 다시 붙어 `run.attach { since }` 로 잃은 출력이 없다 — 실행은 연결의 것이 아니다.
/// 끊긴 동안 쓴 줄도 받고, 두 연결이 받은 글자를 이으면 줄 번호가 빈틈 · 겹침 없이 이어진다.
#[tokio::test(flavor = "multi_thread")]
async fn reattaching_with_since_loses_no_output() {
    let f = Fixture::real();
    f.shell_config(
        "ticker",
        "i=0; while :; do echo line$i; i=$((i+1)); sleep 0.02; done",
    );
    let mut first = f.client();
    let run = info(
        first
            .call(
                "run.start",
                json!({"projectId": f.project, "configId": "ticker"}),
            )
            .await,
    );
    let mut a = first.attach(&run.run_id, None).await;
    first.read_until(&mut a, "line5\r\n").await;
    let (seen, since) = (a.text.clone(), a.end);
    first.session.close();
    drop(first);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut second = f.client();
    let mut b = second.attach(&run.run_id, Some(since)).await;
    assert_eq!((b.from, b.dropped), (since, 0));
    assert_eq!(b.info.state, RunState::Running);
    second.read_until(&mut b, "line40\r\n").await;
    let text = format!("{seen}{}", b.text);
    let numbers: Vec<u32> = text
        .split("line")
        .skip(1)
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap()
        })
        .collect();
    let expected: Vec<u32> = (0..numbers.len() as u32).collect();
    assert_eq!(numbers, expected, "{text:?}");
    second
        .ok::<RunInfo>("run.stop", json!({"runId": run.run_id}))
        .await;
    tokio::time::timeout(Duration::from_secs(10), second.exit(&mut b))
        .await
        .unwrap();
}

// ---- ⑤ 범위 ------------------------------------------------------------------------------------------------------------

/// 완료 기준 ⑤: 다른 프로젝트 창 · Host · 에이전트 범위 연결의 `run.start` 와, Host · 에이전트 범위의 `run.input` 은
/// `PERMISSION_DENIED` 이고 아무것도 띄우지 않는다. 다른 프로젝트 창은 그 `runId` 를 없는 실행으로 본다(attach · input · stop ·
/// list) — 없는 id 와 같은 오류다. 프로젝트 판정을 걷으면 다른 창이 그 실행에 붙어 실패한다.
#[tokio::test(flavor = "multi_thread")]
async fn other_windows_and_connections_cannot_see_or_drive_a_run() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("dev", "true");
    let (other, _) = f.project("other", true);
    let mut own = f.client();
    let run = info(
        own.call(
            "run.start",
            json!({"projectId": f.project, "configId": "dev"}),
        )
        .await,
    );
    assert_eq!(f.fake_launcher().calls(), 1);

    let start = json!({"projectId": f.project, "configId": "dev"});
    for scope in [
        ConnectionScope::Project(other.clone()),
        ConnectionScope::Host,
        ConnectionScope::Agent,
    ] {
        let mut c = f.client_in(scope.clone());
        let e = c.error("run.start", start.clone()).await;
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?} {e:?}");
        let e = c
            .error("run.input", json!({"runId": run.run_id, "data": "x"}))
            .await;
        let expected = if matches!(scope, ConnectionScope::Project(_)) {
            code::NOT_FOUND
        } else {
            code::PERMISSION_DENIED
        };
        assert_eq!(e.code, expected, "{scope:?} {e:?}");
        let e = c.error("run.attach", json!({"runId": run.run_id})).await;
        let missing = c.error("run.attach", json!({"runId": "no-such-run"})).await;
        assert_eq!(e.code, code::NOT_FOUND, "{scope:?} {e:?}");
        assert_eq!(
            e.message.replace(&run.run_id, "ID"),
            missing.message.replace("no-such-run", "ID")
        );
        let e = c.error("run.list", json!({"projectId": f.project})).await;
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?} {e:?}");
    }
    let mut c = f.client_in(ConnectionScope::Project(other.clone()));
    for method in ["run.stop", "run.restart"] {
        let e = c.error(method, json!({"runId": run.run_id})).await;
        assert_eq!(e.code, code::NOT_FOUND, "{method} {e:?}");
    }
    let e = c
        .error(
            "run.resize",
            json!({"runId": run.run_id, "cols": 10, "rows": 10}),
        )
        .await;
    assert_eq!(e.code, code::NOT_FOUND);
    let listed: methods::RunList = c.ok("run.list", json!({"projectId": other})).await;
    assert!(listed.runs.is_empty());
    assert_eq!(f.fake_launcher().calls(), 1, "nothing else was launched");
    assert_eq!(f.fake_launcher().last().1.stops.load(Ordering::SeqCst), 0);
    assert!(lock(&f.fake_launcher().last().1.inputs).is_empty());

    // 파일 창은 디스패처가 먼저 거절한다 — `runId` 와 상관없는 같은 오류다.
    let file = Caller::file(Role::Owner, f.root.join("a.txt"));
    let mut c = f.client_as(file);
    let e = c.error("run.attach", json!({"runId": run.run_id})).await;
    let missing = c.error("run.attach", json!({"runId": "no-such-run"})).await;
    assert_eq!(e.code, code::PERMISSION_DENIED);
    assert_eq!((e.code, e.message), (missing.code, missing.message));

    // 그 프로젝트 창은 입력 · 크기 · 중지를 한다.
    own.ok::<()>("run.input", json!({"runId": run.run_id, "data": "hi"}))
        .await;
    assert_eq!(*lock(&f.fake_launcher().last().1.inputs), [b"hi".to_vec()]);
    own.ok::<()>(
        "run.resize",
        json!({"runId": run.run_id, "cols": 80, "rows": 20}),
    )
    .await;
    own.ok::<RunInfo>("run.stop", json!({"runId": run.run_id}))
        .await;
    assert_eq!(f.fake_launcher().last().1.stops.load(Ordering::SeqCst), 1);
}

// ---- ⑥ 미신뢰 프로젝트 ----------------------------------------------------------------------------------------------------

/// 완료 기준 ⑥: 미신뢰 프로젝트의 `run.start` 는 프로세스를 띄우지 않고 `untrusted` 다(가짜 실행기에 불린 수 0). 에이전트 허가로도
/// 띄우지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_project_runs_nothing() {
    let f = Fixture::fake(RunLimits::default());
    let (untrusted, root) = f.project("cloned", false);
    f.config(
        &root,
        "dev",
        "name = \"dev\"\ntype = \"npm\"\nscript = \"dev\"",
    );
    let mut c = f.client_in(ConnectionScope::Project(untrusted.clone()));
    let e = c
        .error(
            "run.start",
            json!({"projectId": untrusted, "configId": "dev"}),
        )
        .await;
    assert_eq!(e.code, code::PERMISSION_DENIED);
    assert_eq!(e.data.as_ref().unwrap()["reason"], "untrusted", "{e:?}");

    let spec = RunSpec {
        config: "dev".into(),
        argv: vec!["npm".into(), "run".into(), "dev".into()],
        cwd: root.clone(),
        env: BTreeMap::new(),
        profile: None,
    };
    let grant = f.agent_grant(&untrusted, spec);
    let e = f
        .runs
        .start_agent(
            &grant,
            0,
            AgentLaunch {
                session: "s1".into(),
                client: None,
            },
            None,
        )
        .unwrap_err();
    assert_eq!(e.data.as_ref().unwrap()["reason"], "untrusted", "{e:?}");
    assert_eq!(f.fake_launcher().calls(), 0);
    assert!(f.runs.list(&untrusted).is_empty());
}

// ---- ⑦ 런타임 환경의 PATH --------------------------------------------------------------------------------------------------

/// 완료 기준 ⑦: 런타임 환경의 PATH 가 프로세스에 들어간다 — 맨 이름 프로그램(`cargo`)을 그 PATH 로 찾고, 프로세스의 `$PATH` 가 그
/// 값이다. 구성의 `env` 와 `env_file`(M6-03 의 `RunEnv`)도 얹힌다.
#[tokio::test(flavor = "multi_thread")]
async fn the_runtime_path_reaches_the_process() {
    let f = Fixture::real();
    let probe = f.bin.join("cargo");
    fs::write(
        &probe,
        "#!/bin/sh\necho \"PROBE PATH=$PATH GREETING=$GREETING FROM_FILE=$FROM_FILE ARGS=$*\"\n",
    )
    .unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(f.root.join(".env.local"), "FROM_FILE=yes\n").unwrap();
    f.config(
        &f.root,
        "probe",
        "name = \"probe\"\ntype = \"cargo\"\ncommand = \"run\"\nargs = [\"a b\"]\nenv = { GREETING = \"hello\" }\nenv_file = [\".env.local\"]",
    );
    let mut c = f.client();
    let run = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "probe"}),
        )
        .await,
    );
    let mut a = c.attach(&run.run_id, None).await;
    let exit = tokio::time::timeout(Duration::from_secs(10), c.exit(&mut a))
        .await
        .unwrap();
    assert_eq!(exit.code, Some(0), "{:?}", a.text);
    let expected = format!(
        "PROBE PATH={}:/usr/bin:/bin GREETING=hello FROM_FILE=yes ARGS=run a b",
        f.bin.display()
    );
    assert!(a.text.contains(&expected), "{:?}", a.text);
    assert_eq!(
        run.cwd.as_deref(),
        Some(f.root.display().to_string().as_str())
    );
}

// ---- 에이전트 갈래 ------------------------------------------------------------------------------------------------------

fn agent() -> AgentLaunch {
    AgentLaunch {
        session: "session-1".into(),
        client: Some("claude-code".into()),
    }
}

/// 에이전트 갈래는 허가의 실행 내용 그대로 띄운다(argv · 작업 폴더 · env 의 값) — 구성을 읽지 않는다. 띄운 자리는 에이전트다.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_branch_launches_the_granted_spec() {
    let f = Fixture::fake(RunLimits::default());
    let sub = f.root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let spec = RunSpec {
        config: "dev".into(),
        argv: vec!["npm".into(), "run".into(), "dev".into()],
        cwd: sub.clone(),
        env: BTreeMap::from([("TOKEN".to_owned(), "secret".to_owned())]),
        profile: Some("fast".into()),
    };
    let grant = f.agent_grant(&f.project, spec);
    let info = f.runs.start_agent(&grant, 0, agent(), None).unwrap();
    assert_eq!(
        info.launched_by,
        LaunchedBy::Agent {
            session: "session-1".into(),
            client: Some("claude-code".into())
        }
    );
    assert_eq!(info.config, "dev");
    assert_eq!(info.profile.as_deref(), Some("fast"));
    let (launched, _) = f.fake_launcher().last();
    assert_eq!(launched.argv, ["npm", "run", "dev"]);
    assert_eq!(launched.cwd, sub);
    assert!(
        launched
            .env
            .iter()
            .any(|(k, v)| k == "TOKEN" && v == "secret")
    );
    assert!(launched.env.iter().any(|(k, _)| k == "PATH"));
    assert!(matches!(launched.ending, Ending::Terminate { .. }));
    // `Debug` 는 env 의 값을 보이지 않는다.
    assert!(!format!("{launched:?}").contains("secret"));
}

/// M6-10 이 넘긴 경합(⑦): 허가의 작업 폴더는 판정 때 정규화한 경로다. 그 뒤 경로의 한 구성 요소가 심링크로 바뀌면(여기서는
/// 프로젝트 밖을 가리킨다) 띄우기 직전의 재검사가 거절하고 아무것도 띄우지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn a_cwd_swapped_for_a_symlink_after_approval_is_refused() {
    let f = Fixture::fake(RunLimits::default());
    let sub = f.root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let outside = f.tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let spec = RunSpec {
        config: "dev".into(),
        argv: vec!["make".into()],
        cwd: sub.clone(),
        env: BTreeMap::new(),
        profile: None,
    };
    let grant = f.agent_grant(&f.project, spec);
    fs::rename(&sub, f.root.join("sub-moved")).unwrap();
    std::os::unix::fs::symlink(&outside, &sub).unwrap();
    let e = f.runs.start_agent(&grant, 0, agent(), None).unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
    assert_eq!(e.data.as_ref().unwrap()["reason"], "cwd_changed", "{e:?}");
    assert_eq!(f.fake_launcher().calls(), 0);
}

// ---- 이벤트 · 다시 실행 · 프로젝트 닫기 --------------------------------------------------------------------------------

/// `run.started` 다음에 `run.exited` 가 프로젝트 범위로 나간다 — 곧바로 끝나는 실행도 차례가 바뀌지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn started_and_exited_events_go_out_in_order() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("job", "true");
    let mut events = f.bus.subscribe(
        Filter::all()
            .scope(Scope::Project(f.project.clone()))
            .topics([TOPIC_STARTED, TOPIC_EXITED]),
    );
    let run = finished_run(&f, 3);
    let mut got = Vec::new();
    while let Some(Received::Event(e)) = events.try_recv() {
        let info: RunInfo = serde_json::from_value(e.payload.clone()).unwrap();
        got.push((e.topic.clone(), info.run_id, info.state));
    }
    assert_eq!(
        got,
        [
            (
                TOPIC_STARTED.to_owned(),
                run.run_id.clone(),
                RunState::Running
            ),
            (TOPIC_EXITED.to_owned(), run.run_id, RunState::Exited),
        ]
    );
}

/// 「다시 실행」 은 앞 실행의 프로세스 트리가 끝난 것을 본 뒤 띄운다 — 손자까지 사라진 뒤 새 실행이 앞 실행을 가리킨다.
#[tokio::test(flavor = "multi_thread")]
async fn restart_waits_for_the_old_tree_then_starts_again() {
    let f = Fixture::real();
    // 손자는 SIGTERM 을 받고 1초 더 산다 — 다시 실행이 기다리지 않으면 새 실행이 뜬 때 아직 있다.
    f.shell_config(
        "dev",
        "(trap '' HUP; trap 'sleep 1; exit' TERM; while :; do sleep 0.1; done) & echo BG=$!; wait",
    );
    let mut c = f.client();
    let old = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "dev"}),
        )
        .await,
    );
    let mut a = c.attach(&old.run_id, None).await;
    c.read_until(&mut a, "BG=").await;
    c.read_until(&mut a, "\n").await;
    let grandchild = background_pid(&a.text);
    let new = info(c.call("run.restart", json!({"runId": old.run_id})).await);
    let left = alive(grandchild);
    if left {
        kill(grandchild);
    }
    assert!(!left, "the old tree was still there when the run restarted");
    let listed: methods::RunList = c.ok("run.list", json!({"projectId": f.project})).await;
    let ended = listed
        .runs
        .iter()
        .find(|r| r.run_id == old.run_id)
        .and_then(|r| r.ended_at)
        .unwrap();
    assert!(new.started_at >= ended, "{} < {ended}", new.started_at);
    assert_eq!(new.restart_of.as_deref(), Some(old.run_id.as_str()));
    assert_ne!(new.run_id, old.run_id);
    assert_eq!(new.state, RunState::Running);
    let mut b = c.attach(&new.run_id, None).await;
    c.read_until(&mut b, "BG=").await;
    c.read_until(&mut b, "\n").await;
    let second = background_pid(&b.text);
    c.ok::<RunInfo>("run.stop", json!({"runId": new.run_id}))
        .await;
    tokio::time::timeout(Duration::from_secs(10), c.exit(&mut b))
        .await
        .unwrap();
    assert!(gone_within(second, Duration::from_millis(500)));
}

/// 프로젝트 창이 닫히면(창이 확인한 뒤) 그 프로젝트의 실행만 모두 중지한다.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_project_stops_only_its_runs() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("job", "true");
    let (other, other_root) = f.project("other", true);
    f.config(
        &other_root,
        "job",
        "name = \"job\"\ntype = \"shell\"\ncommand = \"true\"",
    );
    let mine = [f.start("job"), f.start("job")];
    let grant = exec_grant(
        &ConnectionScope::Project(other.clone()),
        &other,
        &Launch::Process,
    )
    .unwrap();
    let theirs = f
        .runs
        .start(
            &grant,
            None,
            &RunStartParams {
                project_id: other.clone(),
                config_id: "job".into(),
                ..RunStartParams::default()
            },
        )
        .unwrap();
    f.runs.stop_project(&f.project);
    for run in &mine {
        assert_eq!(
            f.runs.find(&f.project, &run.run_id).unwrap().state(),
            RunState::Exited
        );
    }
    assert_eq!(
        f.runs.find(&other, &theirs.run_id).unwrap().state(),
        RunState::Running
    );
}

/// compose 는 SIGINT 를 먼저(M6-01), 나머지는 SIGTERM 이다.
#[test]
fn compose_is_interrupted_first() {
    let f = Fixture::fake(RunLimits::default());
    let ending = |argv: &[&str]| {
        f.runs
            .ending_for(&argv.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    };
    let limits = RunLimits::default();
    let compose = Ending::InterruptThenTerminate {
        interrupt: limits.interrupt_grace,
        grace: limits.stop_grace,
    };
    assert_eq!(ending(&["docker", "compose", "up"]), compose);
    assert_eq!(
        ending(&["/usr/local/bin/docker", "compose", "-f", "x.yml", "up"]),
        compose
    );
    assert_eq!(ending(&["docker-compose", "up"]), compose);
    assert_eq!(
        ending(&["docker", "run", "x"]),
        Ending::Terminate {
            grace: limits.stop_grace
        }
    );
}

/// 쓸 수 없는 구성 · 없는 구성 · 없는 프로필은 띄우지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn missing_or_unusable_configs_launch_nothing() {
    let f = Fixture::fake(RunLimits::default());
    f.config(&f.root, "broken", "name = \"broken\"\ntype = \"teleport\"");
    f.shell_config("dev", "true");
    let mut c = f.client();
    let e = c
        .error(
            "run.start",
            json!({"projectId": f.project, "configId": "nope"}),
        )
        .await;
    assert_eq!(e.code, code::NOT_FOUND);
    let e = c
        .error(
            "run.start",
            json!({"projectId": f.project, "configId": "broken"}),
        )
        .await;
    assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    let e = c
        .error(
            "run.start",
            json!({"projectId": f.project, "configId": "dev", "profile": "nope"}),
        )
        .await;
    assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    assert_eq!(f.fake_launcher().calls(), 0);
}

/// 창 이벤트의 자리([`project_closed`])는 이 프로세스의 관리자에서 그 프로젝트의 실행을 중지한다.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_project_window_stops_its_runs() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("job", "true");
    let run = f.start("job");
    project_closed("some-other-project");
    assert_eq!(
        f.runs.find(&f.project, &run.run_id).unwrap().state(),
        RunState::Running
    );
    project_closed(&f.project);
    assert_eq!(
        f.runs.find(&f.project, &run.run_id).unwrap().state(),
        RunState::Exited
    );
}

/// 띄우기가 돌려주기 전에 끝난 실행도 `run.started` 가 먼저, `run.exited` 가 한 번 나간다.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_ending_while_it_starts_still_announces_started_first() {
    let f = Fixture::fake(RunLimits::default());
    f.fake_launcher()
        .exit_at_launch
        .store(true, Ordering::SeqCst);
    f.shell_config("job", "true");
    let mut events = f.bus.subscribe(
        Filter::all()
            .scope(Scope::Project(f.project.clone()))
            .topics([TOPIC_STARTED, TOPIC_EXITED]),
    );
    let run = f.start("job");
    assert_eq!(run.state, RunState::Exited);
    let mut topics = Vec::new();
    while let Some(Received::Event(e)) = events.try_recv() {
        topics.push(e.topic.clone());
    }
    assert_eq!(topics, [TOPIC_STARTED, TOPIC_EXITED]);
    let mut c = f.client();
    let mut a = c.attach(&run.run_id, None).await;
    assert_eq!(c.exit(&mut a).await.code, Some(0));
    assert_eq!(a.text, "bye\n");
}

/// 앱 종료 흉내(시험 [`exiting_app_ends_runs`] 가 이 시험 바이너리를 자식으로 띄울 때만 돈다): 실행 하나(SIGHUP · SIGTERM 을 무시하는
/// 손자를 둔다)를 띄워 pid 를 찍고, `PW_RUNS_EXIT_PROBE=shutdown` 이면 [`crate::shutdown`] 을 부른 뒤 Tauri 처럼
/// `std::process::exit` 로 끝난다(Drop 이 돌지 않는다).
#[test]
fn run_exit_probe() {
    let Ok(mode) = std::env::var("PW_RUNS_EXIT_PROBE") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let pids = runtime.block_on(async {
        let runs = Runs::new(Arc::new(Runtime::new(LoginShell::new(
            None,
            Duration::from_secs(1),
        ))));
        let spec = ProcessSpec {
            argv: [
                "/bin/sh",
                "-c",
                "(trap '' HUP TERM; exec sleep 100) & echo BG=$!; echo DONE; wait",
            ]
            .map(str::to_owned)
            .to_vec(),
            cwd: fs::canonicalize(std::env::temp_dir()).unwrap(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cols: 80,
            rows: 24,
            ending: Ending::Terminate {
                grace: Duration::from_secs(5),
            },
        };
        let info = runs
            .start_kind(
                NewRun {
                    project_id: "p".into(),
                    config: "probe".into(),
                    name: "probe".into(),
                    launched_by: LaunchedBy::Window { window: None },
                    worktree: None,
                    cwd: Some(spec.cwd.clone()),
                    profile: None,
                    relaunch: Relaunch::None,
                    size: (80, 24),
                    restart_of: None,
                },
                |output| PtyLauncher.launch(&spec, output),
            )
            .unwrap();
        let run = runs.find("p", &info.run_id).unwrap();
        let text = loop {
            let text =
                run.with_log(|log| String::from_utf8_lossy(&log.read(0, 4096).bytes).into_owned());
            if text.contains("DONE") {
                break text;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        // 앱이 끝날 때 중지 중(SIGTERM 의 5초 안)이던 실행도 덮는다.
        runs.stop("p", &info.run_id).unwrap();
        [background_pid(&text), info.pid.unwrap()]
    });
    println!("PIDS={},{}", pids[0], pids[1]);
    if mode == "shutdown" {
        crate::shutdown();
    }
    std::process::exit(0);
}

/// 완료 기준 ①(앱 종료): 앱이 끝날 때(Drop 없이 `std::process::exit`) [`crate::shutdown`] 이 실행의 세션도 끝낸다 — 중지 중이던
/// 실행과 SIGHUP · SIGTERM 을 무시하는 손자까지 1초 안에 사라진다.
#[test]
fn exiting_app_ends_runs() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["runs::tests::run_exit_probe", "--exact", "--nocapture"])
        .env("PW_RUNS_EXIT_PROBE", "shutdown")
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
    let left = loop {
        let left: Vec<u32> = pids.iter().copied().filter(|&p| alive(p)).collect();
        if left.is_empty() || exited.elapsed() > Duration::from_secs(1) {
            break left;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    for &pid in &left {
        kill(pid);
    }
    assert!(left.is_empty(), "{left:?} of {pids:?} outlived the app");
}

// ---- 리뷰 B1: 작업 폴더의 재검사는 띄우는 자리에서 ------------------------------------------------------------------------

/// 로그인 셸이 도는 동안(런타임 환경을 기다리는 동안) 셸 스크립트 `hook` 을 돌린다.
fn login_hook(f: &Fixture, hook: &str) {
    fs::write(f.tmp.path().join("login-hook.sh"), hook).unwrap();
}

/// 에이전트 허가 하나(작업 폴더 `cwd` 에서 `marker` 를 만드는 명령).
fn touching_spec(cwd: &Path, marker: &Path) -> RunSpec {
    RunSpec {
        config: "dev".into(),
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("touch '{}'", marker.display()),
        ],
        cwd: cwd.to_path_buf(),
        env: BTreeMap::new(),
        profile: None,
    }
}

fn assert_cwd_changed(result: Result<RunInfo, RpcError>) {
    let e = result.expect_err("the run started");
    assert_eq!(e.code, code::PERMISSION_DENIED, "{e:?}");
    assert_eq!(e.data.as_ref().unwrap()["reason"], "cwd_changed", "{e:?}");
}

/// 아무것도 돌지 않았다 — 표지가 없고 목록이 비었다.
async fn assert_nothing_ran(f: &Fixture, marker: &Path) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists(), "the program ran");
    assert!(f.runs.list(&f.project).is_empty());
}

/// 리뷰 B1 ①: 허가 뒤, 런타임 환경을 기다리는 동안 작업 폴더가 밖을 가리키는 링크로 바뀌면 띄우지 않는다(재검사가 런타임 환경 앞에만
/// 있으면 통과해 밖에서 돈다).
#[tokio::test(flavor = "multi_thread")]
async fn a_cwd_swapped_while_the_environment_loads_is_refused() {
    let f = Fixture::real();
    let (sub, outside, marker) = (
        f.root.join("sub"),
        normalize(&f.tmp.path().join("outside")).unwrap(),
        f.tmp.path().join("ran"),
    );
    fs::create_dir_all(&sub).unwrap();
    fs::create_dir_all(&outside).unwrap();
    login_hook(
        &f,
        &format!(
            "mv '{sub}' '{sub}-moved'; ln -s '{}' '{sub}'\n",
            outside.display(),
            sub = sub.display()
        ),
    );
    let grant = f.agent_grant(&f.project, touching_spec(&sub, &marker));
    assert_cwd_changed(f.runs.start_agent(&grant, 0, agent(), None));
    assert_nothing_ran(&f, &marker).await;
}

/// 리뷰 B1 ②: 런타임 환경을 기다리는 동안 작업 폴더가 지워지면 띄우지 않는다 — portable-pty 처럼 `$HOME` 으로 떨어지지 않는다(창의
/// 길).
#[tokio::test(flavor = "multi_thread")]
async fn a_cwd_deleted_while_the_environment_loads_is_refused() {
    let f = Fixture::real();
    let (sub, marker) = (f.root.join("sub"), f.tmp.path().join("ran"));
    fs::create_dir_all(&sub).unwrap();
    f.config(
        &f.root,
        "dev",
        &format!(
            "name = \"dev\"\ntype = \"shell\"\ncwd = \"sub\"\ncommand = {}",
            toml_str(&format!("touch '{}'", marker.display()))
        ),
    );
    login_hook(&f, &format!("rm -rf '{}'\n", sub.display()));
    let mut c = f.client();
    let e = c
        .error(
            "run.start",
            json!({"projectId": f.project, "configId": "dev"}),
        )
        .await;
    assert_eq!(e.code, code::PERMISSION_DENIED, "{e:?}");
    assert_eq!(e.data.as_ref().unwrap()["reason"], "cwd_changed", "{e:?}");
    assert_nothing_ran(&f, &marker).await;
}

/// 리뷰 B1 · 메인 판정: 띄우기 바로 앞의 재검사와 자식의 `chdir` 사이에 링크로 바뀌어도 — 자식은 프로그램을 돌리기 전에 멈춰 있고
/// 그 실제 작업 폴더가 쥔 폴더와 달라 끝난다.
#[tokio::test(flavor = "multi_thread")]
async fn a_cwd_swapped_between_the_check_and_the_spawn_is_refused() {
    let f = Fixture::real();
    let (sub, outside, marker) = (
        f.root.join("sub"),
        normalize(&f.tmp.path().join("outside")).unwrap(),
        f.tmp.path().join("ran"),
    );
    fs::create_dir_all(&sub).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let (from, to) = (sub.clone(), outside.clone());
    process::hooks::on_spawn(&sub, move || {
        fs::rename(&from, from.with_extension("moved")).unwrap();
        std::os::unix::fs::symlink(&to, &from).unwrap();
    });
    let grant = f.agent_grant(&f.project, touching_spec(&sub, &marker));
    assert_cwd_changed(f.runs.start_agent(&grant, 0, agent(), None));
    assert_nothing_ran(&f, &marker).await;
}

/// 같은 틈에 폴더가 지워지면 portable-pty 는 `$HOME` 에서 띄운다 — 띄운 뒤의 확인이 그것을 잡아 끝낸다.
#[tokio::test(flavor = "multi_thread")]
async fn a_cwd_deleted_between_the_check_and_the_spawn_does_not_fall_back_to_home() {
    let f = Fixture::real();
    let (sub, marker) = (f.root.join("sub"), f.tmp.path().join("ran"));
    fs::create_dir_all(&sub).unwrap();
    let gone = sub.clone();
    process::hooks::on_spawn(&sub, move || fs::remove_dir_all(&gone).unwrap());
    let grant = f.agent_grant(&f.project, touching_spec(&sub, &marker));
    assert_cwd_changed(f.runs.start_agent(&grant, 0, agent(), None));
    assert_nothing_ran(&f, &marker).await;
}

/// 정상 길: 감싼 셸은 같은 pid 로 프로그램을 `exec` 한다 — 실행의 pid 가 프로그램의 pid 이고 작업 폴더가 그 폴더다.
#[tokio::test(flavor = "multi_thread")]
async fn the_held_shell_becomes_the_program() {
    let f = Fixture::real();
    let sub = f.root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let spec = RunSpec {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo PID=$$ CWD=$(pwd -P)".into(),
        ],
        ..touching_spec(&sub, Path::new("/nonexistent"))
    };
    let grant = f.agent_grant(&f.project, spec);
    let run = f.runs.start_agent(&grant, 0, agent(), None).unwrap();
    let mut c = f.client();
    let mut a = c.attach(&run.run_id, None).await;
    assert_eq!(c.exit(&mut a).await.code, Some(0), "{:?}", a.text);
    let expected = format!("PID={} CWD={}", run.pid.unwrap(), sub.display());
    assert!(a.text.contains(&expected), "{:?} vs {expected}", a.text);
}

// ---- 리뷰 F1 · F2 · R2 ---------------------------------------------------------------------------------------------------

/// 리뷰 F1: 띄우는 동안 온 중지(창 닫기 · `run.stop`)도 종류에 닿는다 — 실행이 영영 `stopping` 에 머물지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_that_arrives_while_starting_still_stops_the_run() {
    let f = Fixture::fake(RunLimits::default());
    let (runs, project) = (f.runs.clone(), f.project.clone());
    let stopped = Arc::new(Mutex::new(None::<Arc<FakeRun>>));
    let kept = Arc::clone(&stopped);
    let info = f
        .runs
        .start_kind(
            NewRun {
                project_id: f.project.clone(),
                config: "slow".into(),
                name: "slow".into(),
                launched_by: LaunchedBy::Window { window: None },
                worktree: None,
                cwd: None,
                profile: None,
                relaunch: Relaunch::None,
                size: (80, 24),
                restart_of: None,
            },
            move |output| {
                // 종류가 서기 전에 창이 닫혔다.
                runs.stop_project(&project);
                let run = Arc::new(FakeRun {
                    output,
                    stops: AtomicUsize::new(0),
                    inputs: Mutex::default(),
                });
                *lock(&kept) = Some(Arc::clone(&run));
                Ok(run as Arc<dyn RunKind>)
            },
        )
        .unwrap();
    let run = lock(&stopped).clone().unwrap();
    assert_eq!(run.stops.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.runs.find(&f.project, &info.run_id).unwrap().state(),
        RunState::Exited
    );
}

/// 리뷰 F2: 빈 줄만 쓰는 실행도 색인까지 합쳐 실행 하나의 상한 · Host 계량기 안이다.
#[tokio::test(flavor = "multi_thread")]
async fn blank_lines_stay_within_the_running_cap() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("blank", "true");
    let run = f.start("blank");
    let (_, fake) = f.fake_launcher().last();
    let newlines = vec![b'\n'; 1024 * KIB];
    for _ in 0..16 {
        fake.output.write(&newlines);
    }
    assert!(
        f.runs.log_bytes() <= 8 * 1024 * KIB,
        "{}",
        f.runs.log_bytes()
    );
    let memory = f
        .runs
        .find(&f.project, &run.run_id)
        .unwrap()
        .with_log(RunLog::memory);
    assert_eq!(memory, f.runs.log_bytes());
}

/// 리뷰 R2: 실행이 띄운 자식이 `setsid` 로 세션을 떠나도 중지가 닿는다 — 끝내기를 시작할 때 세션의 자손을 찾고, 리더가 죽어 그
/// 자식이 init 으로 넘어간 뒤에도(SIGTERM 을 무시해 5초를 버틴다) SIGKILL 이 간다. 세션만 보면 남아 실패한다.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_left_the_session_is_stopped_too() {
    let f = Fixture::real();
    f.shell_config(
        "escape",
        "perl -MPOSIX -e '$|=1; POSIX::setsid() or die; $SIG{TERM}=q(IGNORE); print qq(ESC=$$\\n); sleep 300' & wait",
    );
    let mut c = f.client();
    let run = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "escape"}),
        )
        .await,
    );
    let mut a = c.attach(&run.run_id, None).await;
    c.read_until(&mut a, "ESC=").await;
    c.read_until(&mut a, "\n").await;
    let escaped: u32 = a
        .text
        .split("ESC=")
        .nth(1)
        .unwrap()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap();
    // SAFETY: getsid(2) 는 메모리를 건드리지 않는다.
    let own_session = unsafe { libc::getsid(libc::pid_t::try_from(escaped).unwrap()) };
    assert_eq!(
        own_session,
        libc::pid_t::try_from(escaped).unwrap(),
        "perl left the session"
    );
    c.ok::<RunInfo>("run.stop", json!({"runId": run.run_id}))
        .await;
    tokio::time::timeout(Duration::from_secs(15), c.exit(&mut a))
        .await
        .unwrap();
    let left = !gone_within(escaped, Duration::from_millis(500));
    if left {
        kill(escaped);
    }
    assert!(!left, "the child that left the session outlived the stop");
}

/// 감싼 셸을 풀어 주는 줄은 출력에 비치지 않고, 띄운 실행마다 프로그램이 돈다(멈췄다 SIGCONT 로 잇던 앞 방식은 macOS 에서 수백 번에
/// 한 번 깨지 않고 남았다 — 여기서는 50번이 모두 끝나야 한다).
#[tokio::test(flavor = "multi_thread")]
async fn every_held_run_is_released_without_a_stray_line() {
    let f = Fixture::real();
    // 10개씩 — 끝난 실행은 프로젝트마다 20개만 남으므로 묶음마다 확인한다.
    for batch in 0..5 {
        let mut runs = Vec::new();
        for i in batch * 10..batch * 10 + 10 {
            let spec = RunSpec {
                argv: vec!["/bin/echo".into(), format!("hi{i}")],
                ..touching_spec(&f.root, Path::new("/nonexistent"))
            };
            let grant = f.agent_grant(&f.project, spec);
            let info = f.runs.start_agent(&grant, 0, agent(), None).unwrap();
            runs.push((i, f.runs.find(&f.project, &info.run_id).unwrap()));
        }
        for (i, run) in runs {
            let exit = tokio::time::timeout(Duration::from_secs(10), run.exited())
                .await
                .unwrap_or_else(|_| panic!("run {i} was never released"));
            assert_eq!(exit.code, Some(0));
            let text = run.with_log(|log| log.read(0, 4096).bytes);
            assert_eq!(String::from_utf8_lossy(&text), format!("hi{i}\r\n"));
        }
    }
}

/// 읽지 못한 `env_file`(없는 파일)은 띄우지 않고 invalid params — 오류 글에 값이 없다(M6-03 의 `EnvError`).
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_env_file_launches_nothing() {
    let f = Fixture::fake(RunLimits::default());
    f.config(
        &f.root,
        "dev",
        "name = \"dev\"\ntype = \"shell\"\ncommand = \"true\"\nenv_file = [\".env.missing\"]",
    );
    let mut c = f.client();
    let e = c
        .error(
            "run.start",
            json!({"projectId": f.project, "configId": "dev"}),
        )
        .await;
    assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    assert!(format!("{:?}", e.data).contains(".env.missing"), "{e:?}");
    assert_eq!(f.fake_launcher().calls(), 0);
}
