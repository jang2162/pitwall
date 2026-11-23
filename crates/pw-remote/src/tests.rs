//! 상태 기계 시험(18-m7-plan M7-08 Rust 시험 ① ② ③) — 가짜 [`Backend`] 로 SSH 없이 돈다. 실제 ssh · 설치 길은 `ssh` 모듈의
//! 시험(가짜 `ssh`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use pw_core::LocalHost;
use pw_core::config::AppPaths;
use tokio::sync::oneshot;

use super::*;

const MINI: &str = "SHA256:mini";

struct FakeLink {
    closed_by_us: AtomicBool,
    drop_tx: Mutex<Option<oneshot::Sender<String>>>,
    /// 링크가 이어 붙는 중인가(M7-09 ① — 진짜 링크의 `LinkState::Reconnecting`).
    resuming: tokio::sync::watch::Sender<bool>,
}

impl FakeLink {
    /// 링크가 저절로 끊긴다(Wi-Fi 등).
    fn drop_link(&self) {
        if let Some(tx) = lock(&self.drop_tx).take() {
            let _ = tx.send("peer closed".into());
        }
    }
}

enum Outcome {
    Link(&'static str),
    Fail(ConnectError),
    /// 붙기가 끝나지 않는다(암호 질문에 아무도 답하지 않는 중 등).
    Hang,
}

/// 증명된 머신 id 로 링크의 원격 Host 를 만드는 것.
type MakeHost = Box<dyn Fn(&str) -> Arc<dyn Host> + Send>;

#[derive(Default)]
struct Fake {
    script: Mutex<VecDeque<Outcome>>,
    connects: AtomicUsize,
    links: Mutex<Vec<Arc<FakeLink>>>,
    expects: Mutex<Vec<Option<String>>>,
    /// 링크의 원격 Host 를 만드는 것(없으면 빈 `LocalHost`). 받는 것은 증명된 머신 id.
    hosts: Mutex<Option<MakeHost>>,
    /// 붙은 뒤 조사 · 정리를 부른 수.
    tidies: AtomicUsize,
    /// 설정의 포트 포워딩(18-m7-plan M7-22).
    port_forwards: Mutex<Vec<pw_link::forward::ForwardSetting>>,
    /// 포워더가 이은 연결 수.
    carried: Arc<AtomicUsize>,
}

/// 가짜 링크의 포워더: 「원격 머신」 이 이 컴퓨터라고 보고 `127.0.0.1:<원격 포트>` 로 곧바로 잇는다.
struct DirectForwarder(Arc<AtomicUsize>);

impl Forwarder for DirectForwarder {
    fn carry(
        &self,
        mut socket: tokio::net::TcpStream,
        to: pw_link::forward::ForwardTo,
    ) -> BoxFuture<'static, Result<(), String>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let mut remote = tokio::net::TcpStream::connect(("127.0.0.1", to.port))
                .await
                .map_err(|e| e.to_string())?;
            tokio::io::copy_bidirectional(&mut socket, &mut remote)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }
}

impl Fake {
    fn push(&self, outcome: Outcome) {
        lock(&self.script).push_back(outcome);
    }

    fn last_link(&self) -> Arc<FakeLink> {
        lock(&self.links).last().cloned().expect("a link")
    }
}

impl Backend for Fake {
    fn port_forwards(
        &self,
        _settings: Option<&Value>,
        name: &str,
    ) -> Vec<pw_link::forward::ForwardSetting> {
        if name == "mini" {
            lock(&self.port_forwards).clone()
        } else {
            Vec::new()
        }
    }

    fn forwarder(&self, _machine: &str) -> Option<Arc<dyn Forwarder>> {
        Some(Arc::new(DirectForwarder(Arc::clone(&self.carried))))
    }

    fn hosts(&self, _settings: Option<&Value>) -> (Vec<HostEntry>, Vec<String>) {
        (
            vec![HostEntry {
                name: "mini".into(),
                host: "mini.local".into(),
                user: None,
                port: None,
                source: RemoteHostSource::SshConfig,
                ssh_args: Vec::new(),
            }],
            Vec::new(),
        )
    }

    fn connect<'a>(
        &'a self,
        _entry: &'a HostEntry,
        expect: Option<&'a str>,
        progress: &'a (dyn Fn(Progress) + Send + Sync),
    ) -> BoxFuture<'a, Result<Link, ConnectError>> {
        Box::pin(async move {
            self.connects.fetch_add(1, Ordering::SeqCst);
            lock(&self.expects).push(expect.map(str::to_owned));
            for stage in [
                RemoteStage::Ssh,
                RemoteStage::Install,
                RemoteStage::Daemon,
                RemoteStage::Handshake,
            ] {
                progress(Progress::Stage(stage));
            }
            let outcome = lock(&self.script)
                .pop_front()
                .unwrap_or(Outcome::Link(MINI));
            match outcome {
                Outcome::Hang => std::future::pending().await,
                Outcome::Fail(e) => Err(e),
                Outcome::Link(machine) => {
                    let (tx, rx) = oneshot::channel();
                    let link = Arc::new(FakeLink {
                        closed_by_us: AtomicBool::new(false),
                        drop_tx: Mutex::new(Some(tx)),
                        resuming: tokio::sync::watch::Sender::new(false),
                    });
                    lock(&self.links).push(Arc::clone(&link));
                    let closer = Arc::clone(&link);
                    Ok(Link {
                        host: match lock(&self.hosts).as_ref() {
                            Some(make) => make(machine),
                            None => Arc::new(LocalHost::new(machine)),
                        },
                        machine: machine.into(),
                        closed: Box::pin(
                            async move { rx.await.unwrap_or_else(|_| "closed".into()) },
                        ),
                        resuming: Some(link.resuming.subscribe()),
                        close: Box::new(move || {
                            closer.closed_by_us.store(true, Ordering::SeqCst);
                            closer.drop_link();
                        }),
                    })
                }
            }
        })
    }

    fn check<'a>(&'a self, _entry: &'a HostEntry) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async { Some(false) })
    }

    fn tidy<'a>(
        &'a self,
        _entry: &'a HostEntry,
        _machine: &'a str,
    ) -> BoxFuture<'a, Result<Tidied, UpgradeError>> {
        let n = self.tidies.fetch_add(1, Ordering::SeqCst) as u32;
        Box::pin(async move {
            Ok(Tidied {
                old: vec![RemoteOldRelay {
                    file: "pitwall-relay-dev-0.9.0".into(),
                    version: Some("0.9.0".into()),
                    channel: Some("dev".into()),
                    pid: 40 + n,
                    runs: 1,
                    terminals: 0,
                    links: 0,
                }],
                ..Tidied::default()
            })
        })
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    remote: RemoteHosts,
    fake: Arc<Fake>,
    router: Arc<HostRouter>,
    registry: Registry,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let bus = EventBus::new(64);
    let registry = Registry::new(AppPaths::under(dir.path()), None, bus.clone());
    let remote = RemoteHosts::default();
    let fake = Arc::new(Fake::default());
    let router = Arc::new(HostRouter::new());
    remote.attach_host(Some(registry.clone()), bus, None);
    remote.attach_backend(fake.clone());
    remote.attach_router(Arc::clone(&router));
    Setup {
        _dir: dir,
        remote,
        fake,
        router,
        registry,
    }
}

/// 상태가 `pred` 에 맞을 때까지 기다린다(멈춘 시계에서도 — 기다리는 동안 시계가 앞으로 간다).
async fn until(remote: &RemoteHosts, pred: impl Fn(&RemoteStatus) -> bool) -> RemoteStatus {
    let mut rx = remote.watch("mini");
    tokio::time::timeout(Duration::from_secs(600), async {
        loop {
            let now = rx.borrow_and_update().clone();
            if pred(&now) {
                return now;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("status never matched; now {:?}", remote.status("mini")))
}

fn connected(s: &RemoteStatus) -> bool {
    matches!(s, RemoteStatus::Connected { .. })
}

// 18-m7-plan M7-14: 링크가 이어 붙는 동안(M7-09 ①)은 상태가 `resuming` 이고 그 머신은 라우터에 남는다 — 창이 그 머신 탭을 읽기
// 전용으로 보이고 연결을 닫지 않는다. 이어 붙으면 `connected` 로 돌아간다. 새로 붙지 않는다(다이얼 1번).
#[tokio::test(start_paused = true)]
async fn a_resuming_link_reports_resuming_and_keeps_the_machine_routed() {
    let s = setup();
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    s.fake.last_link().resuming.send_replace(true);
    let st = until(&s.remote, |st| matches!(st, RemoteStatus::Resuming { .. })).await;
    assert_eq!(
        st,
        RemoteStatus::Resuming {
            machine: MINI.into()
        }
    );
    assert!(
        s.router.host(MINI).is_ok(),
        "the resuming machine left the router"
    );
    s.fake.last_link().resuming.send_replace(false);
    until(&s.remote, connected).await;
    assert!(s.router.host(MINI).is_ok());
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 1);
}

fn fail(kind: RemoteFailureKind, detail: Option<&str>) -> ConnectError {
    ConnectError {
        detail: detail.map(str::to_owned),
        ..ConnectError::new(RemoteStage::Ssh, kind, "ssh could not connect")
    }
}

// ① 사람이 끊은 뒤에는 다시 붙지 않는다 — 쓰는 창이 남아 있어도, 시간이 지나도. 사람이 끊지 않은 끊김은 다시 붙는다.
#[tokio::test(start_paused = true)]
async fn a_host_the_human_disconnected_is_not_reconnected() {
    let s = setup();
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 1);

    // 끊김(사람이 아님): 백오프 뒤 다시 붙는다.
    s.fake.last_link().drop_link();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::Reconnecting { attempt: 1, .. })
    })
    .await;
    until(&s.remote, connected).await;
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 2);

    s.remote.disconnect("mini").unwrap();
    assert!(s.fake.last_link().closed_by_us.load(Ordering::SeqCst));
    assert_eq!(
        s.remote.status("mini"),
        RemoteStatus::Disconnected {
            by_user: true,
            error: None
        }
    );
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(
        s.fake.connects.load(Ordering::SeqCst),
        2,
        "reconnected after the human disconnected"
    );
    assert!(s.router.host(MINI).is_err());
    // 새로 쓰겠다고 하면 다시 붙는다.
    let _again = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 3);
}

// ② 쓰는 것(체크아웃 탭 · 창)이 모두 놓이면 링크를 닫는다. 하나라도 남으면 열어 둔다.
#[tokio::test(start_paused = true)]
async fn the_link_closes_when_every_use_is_released() {
    let s = setup();
    let first = s.remote.hold("mini").unwrap();
    let second = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    assert_eq!(
        s.fake.connects.load(Ordering::SeqCst),
        1,
        "one link for two uses"
    );
    assert!(
        s.router.host(MINI).is_ok(),
        "the remote host is routed by its machine id"
    );

    drop(first);
    assert!(!s.fake.last_link().closed_by_us.load(Ordering::SeqCst));
    assert!(connected(&s.remote.status("mini")));

    drop(second);
    assert!(
        s.fake.last_link().closed_by_us.load(Ordering::SeqCst),
        "the link stayed open with no use"
    );
    assert_eq!(s.remote.status("mini"), RemoteStatus::idle());
    assert!(s.router.host(MINI).is_err());
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 1);
    let record = s.registry.remote_host("mini").unwrap().unwrap();
    assert_eq!(record.last.map(|(m, _)| m), Some(LinkMark::Disconnected));
}

// ③ 연결 로그에 askpass 답이 없다 — 진단 글(ssh stderr)에 섞여 와도 가린다.
#[tokio::test(start_paused = true)]
async fn the_connection_log_has_no_askpass_answer() {
    let s = setup();
    s.remote.remember_secret("hunter2-secret");
    s.remote.remember_secret("yes");
    s.fake.push(Outcome::Fail(fail(
        RemoteFailureKind::Ssh,
        Some("debug1: answer was hunter2-secret\ndev@mini: Permission denied (password)."),
    )));
    let _use = s.remote.hold("mini").unwrap();
    let status = until(&s.remote, |st| {
        matches!(st, RemoteStatus::Disconnected { error: Some(_), .. })
    })
    .await;
    let log = s.remote.log("mini");
    let text: String = log
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(text.contains("[hidden]"), "{text}");
    assert!(text.contains("Permission denied"), "{text}");
    assert!(!format!("{status:?}").contains("hunter2"));
    // 붙기가 끝나면 가릴 것도 지운다(메모리에 오래 두지 않는다).
    assert!(lock(&s.remote.inner.secrets).is_empty());
}

// 단계가 차례로 로그 · 상태에 남고, 처음 붙은 머신 id 를 기억해 다음에는 그 id 만 기대한다. 다른 id 가 오면 멈추고, 사람이 본
// 그 id 로만 「새로 믿기」 가 된다(묶음 멤버의 머신 id 도 옮긴다).
#[tokio::test(start_paused = true)]
async fn the_machine_id_is_remembered_and_a_changed_one_waits_for_the_human() {
    let s = setup();
    let first = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    let stages: Vec<_> = s
        .remote
        .log("mini")
        .iter()
        .filter_map(|l| l.stage)
        .collect();
    for stage in [
        RemoteStage::Ssh,
        RemoteStage::Install,
        RemoteStage::Daemon,
        RemoteStage::Handshake,
    ] {
        assert!(stages.contains(&stage), "{stages:?}");
    }
    let list = s.remote.list();
    assert_eq!(list.hosts[0].machine.as_deref(), Some(MINI));
    assert_eq!(
        list.hosts[0].last_link.as_ref().map(|l| l.state),
        Some(RemoteLastLinkState::Connected)
    );
    drop(first);

    s.fake.push(Outcome::Fail(ConnectError {
        machine_changed: Some((MINI.into(), "SHA256:other".into())),
        ..ConnectError::new(
            RemoteStage::Handshake,
            RemoteFailureKind::Rejected,
            "machine id changed",
        )
    }));
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::MachineChanged { .. })
    })
    .await;
    assert_eq!(
        lock(&s.fake.expects).last().cloned().flatten().as_deref(),
        Some(MINI)
    );
    tokio::time::sleep(Duration::from_secs(120)).await;
    // 다른 창이 그 호스트를 써도 바뀐 머신에 다시 붙지 않는다(ssh · 설치 · 페어링을 되풀이하지 않는다 — 리뷰 R2).
    let _another = s.remote.hold("mini").unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        s.fake.connects.load(Ordering::SeqCst),
        2,
        "retried a changed machine"
    );
    assert!(s.remote.retry("mini").is_err());
    assert!(s.remote.trust_machine("mini", "SHA256:third").is_err());
    assert_eq!(
        s.registry
            .remote_host("mini")
            .unwrap()
            .unwrap()
            .machine
            .as_deref(),
        Some(MINI)
    );

    s.fake.push(Outcome::Link("SHA256:other"));
    s.remote.trust_machine("mini", "SHA256:other").unwrap();
    until(&s.remote, connected).await;
    assert_eq!(
        s.registry
            .remote_host("mini")
            .unwrap()
            .unwrap()
            .machine
            .as_deref(),
        Some("SHA256:other")
    );
}

// 처음 붙기가 실패하면 저절로 다시 하지 않는다(사람이 [다시 시도]). 붙었다 끊긴 뒤의 실패는 백오프(1 · 2 · 4 …)로 다시 하고,
// 사람이 손대야 하는 실패(호스트 키)에서는 멈춘다.
#[tokio::test(start_paused = true)]
async fn the_first_failure_waits_for_retry_and_a_dropped_link_backs_off() {
    let s = setup();
    s.fake
        .push(Outcome::Fail(fail(RemoteFailureKind::Ssh, None)));
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::Disconnected { error: Some(_), .. })
    })
    .await;
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 1);

    s.remote.retry("mini").unwrap();
    until(&s.remote, connected).await;
    s.fake
        .push(Outcome::Fail(fail(RemoteFailureKind::Link, None)));
    s.fake
        .push(Outcome::Fail(fail(RemoteFailureKind::Ssh, None)));
    s.fake
        .push(Outcome::Fail(fail(RemoteFailureKind::HostKeyChanged, None)));
    s.fake.last_link().drop_link();
    let mut delays = Vec::new();
    let mut rx = s.remote.watch("mini");
    loop {
        rx.changed().await.unwrap();
        let now = rx.borrow_and_update().clone();
        match now {
            RemoteStatus::Reconnecting { retry_in_ms, .. } => delays.push(retry_in_ms),
            RemoteStatus::Disconnected { error: Some(e), .. } => {
                assert_eq!(e.kind, RemoteFailureKind::HostKeyChanged);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(delays, [1000, 2000, 4000]);
    let backoff = Backoff::default();
    assert_eq!(backoff.delay(5), Duration::from_secs(16));
    assert_eq!(backoff.delay(6), Duration::from_secs(30));
    assert_eq!(backoff.delay(40), Duration::from_secs(30));
}

// 이름 없는 호스트 · 부품이 없는 프로세스.
#[tokio::test]
async fn unknown_hosts_and_a_process_without_the_backend_are_refused() {
    let s = setup();
    assert!(matches!(
        s.remote.hold("nope"),
        Err(RemoteError::UnknownHost(_))
    ));
    let bare = RemoteHosts::default();
    assert!(matches!(bare.hold("mini"), Err(RemoteError::Unavailable)));
    assert!(bare.list().hosts.is_empty());
    assert_eq!(bare.list().warnings.len(), 1);
}

async fn call(
    host: &LocalHost,
    scope: pw_rpc::ConnectionScope,
    method: &str,
    params: Value,
) -> Value {
    let mut conn = host
        .connect(pw_rpc::Caller {
            role: pw_rpc::Role::Owner,
            scope,
            window: None,
        })
        .await
        .unwrap();
    conn.sink
        .send(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
    loop {
        let frame = conn.source.recv().await.unwrap();
        if frame.get("id") == Some(&serde_json::json!(1)) {
            // 스트림 메서드는 먼저 열리고 핸들러의 거절은 `stream.end` 로 온다.
            let Some(stream) = frame["result"].get("streamId").cloned() else {
                return frame;
            };
            loop {
                let end = conn.source.recv().await.unwrap();
                if end["method"] == "stream.end" && end["params"]["streamId"] == stream {
                    return serde_json::json!({ "error": end["params"]["error"] });
                }
            }
        }
    }
}

// RPC 길: 창이 `remote.prompt.answer` 로 넣은 askpass 답은 질문한 쪽에 닿고, 그 뒤 진단 글에 섞여 와도 로그에 남지 않는다.
// `remote.*` 는 Host 범위 연결만 받는다(프로젝트 창 · MCP 세션은 거절).
#[tokio::test]
async fn an_answer_given_through_the_rpc_is_hidden_from_the_log_and_only_host_connections_manage_hosts()
 {
    use pw_git::write::{AskpassHint, GitPromptAnswer, GitPromptRequest};
    use pw_rpc::ConnectionScope;

    let s = setup();
    let registered = s.remote.clone();
    let host = LocalHost::builder("local")
        .methods(move |r| methods::register(r, &registered))
        .build();
    let _window = s.remote.prompts().listen(PROMPTS_KEY);
    let prompts = s.remote.prompts().clone();
    let asked = tokio::spawn(async move {
        prompts
            .ask(
                PROMPTS_KEY,
                GitPromptRequest::Askpass {
                    text: "dev@mini's password: ".into(),
                    hint: AskpassHint::Secret,
                },
                Duration::from_secs(10),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let answer = call(
        &host,
        ConnectionScope::Host,
        "remote.prompt.answer",
        serde_json::json!({"promptId": 1, "answer": {"kind": "askpass", "value": "s3cret-xyz"}}),
    )
    .await;
    assert!(answer.get("error").is_none(), "{answer}");
    assert_eq!(
        asked.await.unwrap().unwrap(),
        GitPromptAnswer::Askpass {
            value: "s3cret-xyz".into()
        }
    );

    s.fake.push(Outcome::Fail(fail(
        RemoteFailureKind::Ssh,
        Some("ProxyCommand said: s3cret-xyz"),
    )));
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::Disconnected { error: Some(_), .. })
    })
    .await;
    let log = call(
        &host,
        ConnectionScope::Host,
        "remote.log",
        serde_json::json!({"name": "mini"}),
    )
    .await;
    let text = log["result"].to_string();
    assert!(text.contains("ProxyCommand said: [hidden]"), "{text}");
    assert!(!text.contains("s3cret"), "{text}");

    for scope in [ConnectionScope::Project("p".into()), ConnectionScope::Agent] {
        let refused = call(&host, scope, "remote.hosts.list", Value::Null).await;
        assert_eq!(
            refused["error"]["code"],
            serde_json::json!(pw_rpc::code::PERMISSION_DENIED),
            "{refused}"
        );
    }
    let listed = call(
        &host,
        ConnectionScope::Host,
        "remote.hosts.list",
        Value::Null,
    )
    .await;
    assert_eq!(listed["result"]["hosts"][0]["name"], "mini", "{listed}");
}

fn changed(expected: &str, found: &str) -> Outcome {
    Outcome::Fail(ConnectError {
        machine_changed: Some((expected.into(), found.into())),
        ..ConnectError::new(
            RemoteStage::Handshake,
            RemoteFailureKind::Rejected,
            "machine id changed",
        )
    })
}

// 리뷰 B1: 이 호스트에 Desktop 이 기억한 id 가 없으면 「새로 믿기」 는 아무것도 옮기지 않는다 — `expected` 가 원격이 고른 값일 수
// 있다(남의 묶음 멤버를 원격으로 옮기는 길).
#[tokio::test(start_paused = true)]
async fn trusting_a_new_machine_needs_the_id_this_desktop_remembered() {
    let s = setup();
    s.fake.push(changed("SHA256:victim", "SHA256:attacker"));
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::MachineChanged { .. })
    })
    .await;
    let refused = s.remote.trust_machine("mini", "SHA256:attacker");
    assert!(
        matches!(refused, Err(RemoteError::Conflict(_))),
        "{refused:?}"
    );
    assert_eq!(
        s.registry
            .remote_host("mini")
            .unwrap()
            .and_then(|r| r.machine),
        None
    );
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 1);
}

// 리뷰 F1: 새 id 가 이미 다른 호스트 이름으로 아는 머신이거나, 옛 id 를 다른 이름이 아직 기억하면 두 머신의 멤버가 섞인다 — 거절.
#[tokio::test(start_paused = true)]
async fn trusting_a_machine_known_under_another_host_name_is_refused() {
    let s = setup();
    s.registry.remember_remote_machine("mini", MINI).unwrap();
    s.registry.remember_remote_machine("z", "SHA256:z").unwrap();
    s.fake.push(changed(MINI, "SHA256:z"));
    let first = s.remote.hold("mini").unwrap();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::MachineChanged { .. })
    })
    .await;
    let refused = s.remote.trust_machine("mini", "SHA256:z");
    assert!(
        matches!(&refused, Err(RemoteError::Conflict(why)) if why.contains("`z`")),
        "{refused:?}"
    );
    drop(first);

    // 옛 id 를 다른 이름(`mini-old`)이 아직 기억한다.
    let s = setup();
    s.registry.remember_remote_machine("mini", MINI).unwrap();
    s.registry
        .remember_remote_machine("mini-old", MINI)
        .unwrap();
    s.fake.push(changed(MINI, "SHA256:fresh"));
    let _use = s.remote.hold("mini").unwrap();
    until(
        &s.remote,
        |st| matches!(st, RemoteStatus::MachineChanged { found, .. } if found == "SHA256:fresh"),
    )
    .await;
    let refused = s.remote.trust_machine("mini", "SHA256:fresh");
    assert!(
        matches!(&refused, Err(RemoteError::Conflict(why)) if why.contains("`mini-old`")),
        "{refused:?}"
    );
    assert_eq!(
        s.registry
            .remote_host("mini")
            .unwrap()
            .unwrap()
            .machine
            .as_deref(),
        Some(MINI)
    );
}

// 리뷰 F2: 붙는 중(암호를 묻는 중)에 사람이 끊거나 창이 놓으면 가릴 askpass 답도 메모리에서 지운다.
#[tokio::test(start_paused = true)]
async fn askpass_answers_are_forgotten_when_a_connecting_host_is_dropped() {
    for by_user in [true, false] {
        let s = setup();
        s.fake.push(Outcome::Hang);
        let use_ = s.remote.hold("mini").unwrap();
        until(&s.remote, |st| {
            matches!(st, RemoteStatus::Connecting { .. })
        })
        .await;
        tokio::task::yield_now().await;
        s.remote.remember_secret("hunter2-secret");
        assert!(!lock(&s.remote.inner.secrets).is_empty());
        if by_user {
            s.remote.disconnect("mini").unwrap();
        } else {
            drop(use_);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            lock(&s.remote.inner.secrets).is_empty(),
            "the answer outlived the dropped attempt (by_user {by_user})"
        );
        assert_eq!(*lock(&s.remote.inner.attempting), 0);
    }
}

// 리뷰 F4: 끊긴 뒤 다시 붙기에서 ssh 가 로그인하지 못하면(암호 질문 취소 · 틀린 답) 저절로 다시 묻지 않고 멈춘다.
#[tokio::test(start_paused = true)]
async fn a_failed_sign_in_while_reconnecting_is_not_asked_again() {
    let s = setup();
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    s.fake.push(Outcome::Fail(fail(
        RemoteFailureKind::Auth,
        Some("dev@mini: Permission denied (password)."),
    )));
    s.fake.last_link().drop_link();
    let status = until(&s.remote, |st| {
        matches!(st, RemoteStatus::Disconnected { error: Some(_), .. })
    })
    .await;
    assert!(
        matches!(&status, RemoteStatus::Disconnected { error: Some(e), .. } if e.kind == RemoteFailureKind::Auth),
        "{status:?}"
    );
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(
        s.fake.connects.load(Ordering::SeqCst),
        2,
        "asked the password again"
    );
}

// 리뷰 F3: `remote.*` 의 모든 메서드는 Host 범위 연결만 받는다(프로젝트 창 · MCP 세션은 거절) — 메서드마다 따로 있는 검사를
// 표로 본다.
#[tokio::test]
async fn every_remote_method_takes_only_a_host_connection() {
    use pw_rpc::ConnectionScope;

    let s = setup();
    let registered = s.remote.clone();
    let host = LocalHost::builder("local")
        .methods(move |r| methods::register(r, &registered))
        .build();
    let name = serde_json::json!({ "name": "mini" });
    let table: Vec<(&str, Value)> = vec![
        ("remote.hosts.list", Value::Null),
        ("remote.hosts.check", name.clone()),
        ("remote.log", name.clone()),
        ("remote.relays", name.clone()),
        ("remote.connect", name.clone()),
        ("remote.prompts", Value::Null),
        ("remote.disconnect", name.clone()),
        ("remote.retry", name.clone()),
        (
            "remote.trustMachine",
            serde_json::json!({ "name": "mini", "machine": "SHA256:x" }),
        ),
        (
            "remote.stopOld",
            serde_json::json!({ "name": "mini", "machine": "SHA256:x", "seen": {
                "file": "pitwall-relay-dev-1.0.0", "version": null, "channel": null,
                "pid": 1, "runs": 0, "terminals": 0, "links": 0 } }),
        ),
        (
            "remote.linkCommand",
            serde_json::json!({ "name": "mini", "machine": "SHA256:x", "on": true }),
        ),
        (
            "remote.prompt.answer",
            serde_json::json!({ "promptId": 1, "answer": { "kind": "cancel" } }),
        ),
    ];
    let declared: std::collections::BTreeSet<&str> = host
        .router()
        .methods()
        .into_iter()
        .map(|m| m.name)
        .filter(|n| n.starts_with("remote."))
        .collect();
    assert_eq!(
        declared,
        table.iter().map(|(m, _)| *m).collect(),
        "the table must cover every remote.* method"
    );
    for (method, params) in &table {
        for scope in [ConnectionScope::Project("p".into()), ConnectionScope::Agent] {
            // 거절되지 않은 스트림(`remote.connect`)은 끝나지 않는다 — 시간 상한으로 실패로 만든다.
            let refused = tokio::time::timeout(
                Duration::from_secs(5),
                call(&host, scope.clone(), method, params.clone()),
            )
            .await
            .unwrap_or_else(|_| panic!("{method} {scope:?} was not refused (it is still running)"));
            assert_eq!(
                refused["error"]["code"],
                serde_json::json!(pw_rpc::code::PERMISSION_DENIED),
                "{method} {scope:?}: {refused}"
            );
        }
    }
    assert_eq!(s.fake.connects.load(Ordering::SeqCst), 0);
}

// 델타 리뷰 F-b: 처음 붙은 호스트의 id 기억도 「다른 이름이 그 id 를 아는가」 와 한 트랜잭션이다 — 다른 이름(`z`)이 이미 아는
// 머신이면 그 링크를 쓰지 않고(닫고 라우터에 두지 않는다) `mini` 에 그 id 를 기억하지 않는다.
#[tokio::test(start_paused = true)]
async fn a_first_connection_to_a_machine_known_under_another_name_is_not_used() {
    let s = setup();
    s.registry.remember_remote_machine("z", MINI).unwrap();
    let _use = s.remote.hold("mini").unwrap();
    let status = until(&s.remote, |st| {
        matches!(st, RemoteStatus::Disconnected { error: Some(_), .. })
    })
    .await;
    assert!(
        matches!(&status, RemoteStatus::Disconnected { error: Some(e), .. } if e.kind == RemoteFailureKind::AlreadyConnected),
        "{status:?}"
    );
    assert!(s.fake.last_link().closed_by_us.load(Ordering::SeqCst));
    assert!(s.router.host(MINI).is_err());
    assert_eq!(
        s.registry
            .remote_host("mini")
            .unwrap()
            .and_then(|r| r.machine),
        None
    );
}

/// 리뷰 F2 ①: 비밀 청에 답하는 열쇠는 그 링크가 증명한 머신 id 다(호스트 별명 `mini` 가 아니다). 붙은 원격 Host 는 라우터에 감싼
/// 채로 들어가, 그 창 연결의 비밀이 필요한 요청이 청을 연다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secret_answers_use_the_machine_id_the_link_proved() {
    use crate::secrets::tests::{ITEM, P, listening, probe_with, relay_host, store_with, window};
    use pw_db::desktop_secret::DesktopSecrets;
    use pw_db::secret::{DbSecretKey, machine_account};

    let s = setup();
    let secrets = DesktopSecrets::with_wait(Duration::from_secs(3));
    let made = secrets.clone();
    *lock(&s.fake.hosts) = Some(Box::new(move |machine: &str| -> Arc<dyn Host> {
        Arc::new(relay_host(machine, &made))
    }));
    let entries = [
        (machine_account(MINI, P, "dev", DbSecretKey::Password), ITEM),
        (
            machine_account("mini", P, "dev", DbSecretKey::Password),
            "by-alias",
        ),
    ];
    s.remote.attach_secrets(store_with(&entries));
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    listening(&secrets).await;
    let gated = s.router.host(MINI).unwrap();
    let got = probe_with(
        &gated,
        window(),
        "db.session.open",
        "dev",
        "dev",
        Value::Null,
    )
    .await;
    assert_eq!(got, serde_json::json!([ITEM]));
}

/// 리뷰 F2 ②: 「이 머신을 새로 믿기」 는 옛 머신 id 의 키체인 비밀만 지운다 — 새 id · 다른 머신의 것은 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusting_a_new_machine_forgets_only_the_old_ids_secrets() {
    use crate::secrets::tests::{ITEM, P, store_with};
    use pw_db::secret::{DbSecretKey, machine_account, machine_index_account};

    const NEW: &str = "SHA256:other";
    const ELSE: &str = "SHA256:elsewhere";
    let s = setup();
    let mut entries = Vec::new();
    for machine in [MINI, NEW, ELSE] {
        let account = machine_account(machine, P, "dev", DbSecretKey::Password);
        entries.push((
            machine_index_account(machine),
            serde_json::json!([account]).to_string(),
        ));
        entries.push((account, ITEM.to_owned()));
    }
    let borrowed: Vec<(String, &str)> = entries
        .iter()
        .map(|(a, v)| (a.clone(), v.as_str()))
        .collect();
    let store = store_with(&borrowed);
    s.remote.attach_secrets(store.clone());
    let first = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    drop(first);
    s.fake.push(Outcome::Fail(ConnectError {
        machine_changed: Some((MINI.into(), NEW.into())),
        ..ConnectError::new(
            RemoteStage::Handshake,
            RemoteFailureKind::Rejected,
            "machine id changed",
        )
    }));
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::MachineChanged { .. })
    })
    .await;
    s.fake.push(Outcome::Link(NEW));
    let remote = s.remote.clone();
    tokio::task::spawn_blocking(move || remote.trust_machine("mini", NEW))
        .await
        .unwrap()
        .unwrap();
    let left = store.accounts();
    assert!(left.iter().all(|a| !a.starts_with(MINI)), "{left:?}");
    for machine in [NEW, ELSE] {
        assert!(
            left.contains(&machine_account(machine, P, "dev", DbSecretKey::Password)),
            "{left:?}"
        );
        assert!(left.contains(&machine_index_account(machine)), "{left:?}");
    }
}

// M7-07b ①: 붙은 뒤 조사 · 정리가 링크마다 한 번 돈다. 그 결과(`remote.relays`)는 그 링크의 머신 id 를 달고, 링크가 닫히면 비고
// (`remote.relays` 이벤트), 다시 붙으면 다시 돈다.
#[tokio::test(start_paused = true)]
async fn each_link_tidies_once_and_its_relays_go_with_it() {
    let s = setup();
    let bus = read(&s.remote.inner.host).unwrap().bus;
    let mut events = bus.subscribe(pw_core::bus::Filter::all().topics([TOPIC_RELAYS]));
    let _use = s.remote.hold("mini").unwrap();
    until(&s.remote, connected).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while s.remote.relays("mini").is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("no survey after connecting");
    let relays = s.remote.relays("mini").unwrap();
    assert_eq!(relays.machine, MINI);
    assert_eq!(relays.old[0].pid, 40);
    let _again = s.remote.hold("mini").unwrap();
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(s.fake.tidies.load(Ordering::SeqCst), 1, "one tidy per link");
    assert!(
        matches!(events.try_recv(), Some(pw_core::bus::Received::Event(_))),
        "no remote.relays event"
    );

    s.fake.last_link().drop_link();
    until(&s.remote, |st| {
        matches!(st, RemoteStatus::Reconnecting { .. })
    })
    .await;
    assert_eq!(
        s.remote.relays("mini"),
        None,
        "the old link's survey stayed"
    );
    until(&s.remote, connected).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while s.remote.relays("mini").is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("no survey after reconnecting");
    assert_eq!(s.fake.tidies.load(Ordering::SeqCst), 2);
}

mod forwards;
