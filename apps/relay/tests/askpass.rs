//! 원격 askpass 의 길 전체(18-m7-plan M7-15 ①): Relay 의 git 쓰기가 원격 도우미에서 암호를 물으면 git 은 데몬 소켓 옆의 도우미
//! 스크립트를, 스크립트는 **진짜 `pitwall-relay askpass`** 를 부르고, 그것이 데몬 소켓의 좁은 명령으로 질문을 넘긴다. 질문은
//! 링크로 붙은 Desktop 의 그 프로젝트 창 연결(`git.prompts`)에만 가고, 그 창의 답이 git 에 닿는다. 끝난 쓰기의 토큰은 거절한다.
//! 실제 SSH 는 쓰지 않는다 — Desktop 링크는 메모리 전송, 도우미는 실제 유닉스 소켓.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::open::OpenRequests;
use pw_core::{Connection, Host, LocalHost};
use pw_git::testing::TestRepo;
use pw_git::testing::write::WriteFixture;
use pw_git::write::{Askpass, WriteRequest};
use pw_link::transport::{self, MemoryAcceptor};
use pw_link::{
    DesktopIdentity, DeviceKey, Expect, LinkLimits, Pairing, PairingTable, RelayIdentity,
    RelayParts, RemoteHost, Transport,
};
use pw_relay::command::{self, Command};
use pw_relay::config::IdleConfig;
use pw_relay::daemon::{Activity, Daemon, DaemonParts, MAX_UNPROVEN};
use pw_relay::narrow::RelayNarrow;
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);
const SECRET: &str = "pw-relay-secret-41d7";
const QUESTION: &str = "Password for 'https://someone@example.invalid': ";

struct Idle;

impl Activity for Idle {
    fn runs(&self) -> usize {
        0
    }
    fn terminals(&self) -> usize {
        0
    }
    fn stop_language_servers(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = usize> + Send + '_>> {
        Box::pin(async { 0 })
    }
}

async fn accept_all(daemon: Daemon, acceptor: MemoryAcceptor) {
    while let Some(stream) = acceptor.accept().await {
        daemon.accept(stream);
    }
}

/// `bin/git-remote-testauth`: `$GIT_ASKPASS` 로 묻고 그 stdout 을 `out` 에, 그 쓰기의 토큰을 `out.token` 에 적고 실패로 끝난다.
fn remote_helper(bin: &Path, out: &Path) {
    let path = bin.join("git-remote-testauth");
    let out = out.display();
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s' \"$PITWALL_ASKPASS_TOKEN\" > '{out}.token'\n\"$GIT_ASKPASS\" \"{QUESTION}\" > '{out}.partial'\necho $? > '{out}.code'\nmv '{out}.partial' '{out}'\nexit 1\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// 링크 너머 RPC 연결.
struct Rpc(Connection, i64);

impl Rpc {
    async fn open(remote: &RemoteHost, scope: ConnectionScope) -> Self {
        let caller = Caller {
            role: Role::Owner,
            scope,
            window: Some("project-w".into()),
        };
        Self(remote.connect(caller).await.unwrap(), 0)
    }

    async fn next(&mut self, wait: Duration) -> Option<Value> {
        tokio::time::timeout(wait, self.0.source.recv())
            .await
            .ok()
            .flatten()
    }

    fn send(&mut self, method: &str, params: Value) -> i64 {
        self.1 += 1;
        self.0
            .sink
            .send(json!({"jsonrpc": "2.0", "id": self.1, "method": method, "params": params}));
        self.1
    }

    /// `git.prompts` 를 연다. 서면 `Ok`, 거절되면 오류.
    async fn listen(&mut self, project: &str) -> Result<(), Value> {
        let id = self.send("git.prompts", json!({"projectId": project}));
        let opened = loop {
            let frame = self.next(WAIT).await.expect("no answer to git.prompts");
            if frame["id"] == json!(id) {
                break frame;
            }
        };
        if !opened["error"].is_null() {
            return Err(opened["error"].clone());
        }
        let stream = opened["result"]["streamId"].clone();
        self.0.sink.send(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream, "credit": 16}}));
        // 거절은 열기 응답 뒤 `stream.end` 로도 온다.
        if let Some(frame) = self.next(Duration::from_millis(300)).await {
            if frame["method"] == "stream.end" {
                return Err(frame["params"]["error"].clone());
            }
            panic!("unexpected frame before any prompt: {frame}");
        }
        Ok(())
    }

    /// 다음 질문(`open`). `wait` 안에 없으면 `None`.
    async fn prompt(&mut self, wait: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame = self.next(left).await?;
            if frame["method"] == "stream.chunk" && frame["params"]["data"]["kind"] == "open" {
                return Some(frame["params"]["data"]["prompt"].clone());
            }
        }
    }
}

/// 원격 git 쓰기의 askpass 질문이 링크의 **그 프로젝트 창 연결에만** 간다 — 같은 링크의 다른 프로젝트 창 연결은 듣기만 하고 받지
/// 못하고, Host 범위 연결은 듣지도 못한다. 그 창의 답이 진짜 `pitwall-relay askpass` 를 지나 git 에 닿는다(시험 ①). 쓰기가 끝난
/// 뒤 그 토큰으로 온 질문은 거절된다(시험 ②). 도우미가 기다리는 동안 증명 전 자리를 쥐지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_git_password_question_reaches_only_that_project_window() {
    let askpass = Askpass::default();
    let f = WriteFixture::with_repo(TestRepo::with_commit(), true, askpass.clone());
    let other_root = tempfile::tempdir().unwrap();
    let other = f
        .projects
        .create(pw_core::registry::NewProject {
            root: other_root.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap()
        .id;

    // Relay 의 Host(이 시험은 git 쓰기 메서드만 — 조립 전체는 pw-relay 시험이 본다)와 데몬.
    let write = Arc::clone(&f.write);
    let host = LocalHost::builder("local")
        .methods(move |router| pw_git::write::register(router, write))
        .build();
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(Vec::new());
    let daemon = Daemon::new(DaemonParts {
        link: RelayParts {
            identity: Arc::new(RelayIdentity {
                key: Arc::clone(&relay_key),
                versions: pw_relay::versions(),
            }),
            pairings: pairings.clone(),
            host: Arc::new(host) as Arc<dyn Host>,
            limits: LinkLimits::default(),
        },
        activity: Arc::new(Idle),
        narrow: Arc::new(RelayNarrow::new(
            askpass.clone(),
            OpenRequests::default(),
            Some(f.projects.clone()),
        )),
        idle: IdleConfig::default(),
        name: Some("mini".into()),
        max_unproven: MAX_UNPROVEN,
    });

    // 데몬 소켓(도우미가 붙는다)과 그 옆의 도우미 스크립트.
    let sock_dir = tempfile::Builder::new()
        .prefix("pwr")
        .tempdir_in("/tmp")
        .unwrap();
    let run = sock_dir.path().join("run");
    let uid = pw_relay::socket::current_uid();
    pw_relay::socket::private_dir(&run, uid).unwrap();
    let socket = run.join("r.sock");
    let lock = pw_relay::socket::lock(&socket, uid).unwrap().unwrap();
    let listener = pw_relay::socket::bind(&socket, uid, &lock).unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    tokio::spawn({
        let (daemon, shutdown) = (daemon.clone(), shutdown.clone());
        async move { daemon.run(listener, shutdown).await }
    });
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_pitwall-relay"));
    let _through = askpass.serve_through(socket.clone(), &exe).unwrap();

    // Desktop 의 링크(메모리 전송).
    let desktop_key = Arc::new(DeviceKey::generate().unwrap());
    pairings.upsert(Pairing {
        key: desktop_key.public(),
        role: Role::Owner,
        name: "mac".into(),
    });
    let (transport, acceptor) = transport::memory(1024 * 1024);
    tokio::spawn(accept_all(daemon.clone(), acceptor));
    let remote = RemoteHost::new(
        "mini",
        Arc::new(transport) as Arc<dyn Transport>,
        Arc::new(DesktopIdentity {
            key: desktop_key,
            versions: pw_relay::versions(),
        }),
        Expect::Machine(relay_key.id()),
        LinkLimits::default(),
    );
    remote.attach().await.unwrap();

    let mut window = Rpc::open(&remote, ConnectionScope::Project(f.project.clone())).await;
    window.listen(&f.project).await.unwrap();
    let mut other_window = Rpc::open(&remote, ConnectionScope::Project(other.clone())).await;
    other_window.listen(&other).await.unwrap();
    let mut host_scope = Rpc::open(&remote, ConnectionScope::Host).await;
    let refused = host_scope.listen(&f.project).await.unwrap_err();
    assert!(
        !refused.is_null(),
        "a host connection listened to a project's prompts"
    );

    let out = f.repo.base().join("remote-out");
    remote_helper(&f.bin, &out);
    let request = WriteRequest {
        askpass: true,
        ..f.request("test git fetch")
    };
    let fetch = tokio::spawn({
        let write = Arc::clone(&f.write);
        async move {
            write
                .run(request, |ctx| async move {
                    ctx.output(ctx.git().args(["fetch", "testauth::example"]))
                        .await
                        .map(drop)
                })
                .await
        }
    });

    let prompt = window
        .prompt(WAIT)
        .await
        .expect("the project window got no question");
    assert_eq!(prompt["request"]["kind"], "askpass");
    assert_eq!(prompt["request"]["text"], QUESTION);
    assert_eq!(prompt["request"]["hint"], "secret");
    assert_eq!(
        daemon.unproven(),
        0,
        "a waiting helper holds an unproven slot"
    );
    assert!(
        other_window
            .prompt(Duration::from_millis(500))
            .await
            .is_none(),
        "another project's window got the question"
    );
    let answered = window.send(
        "git.prompt.answer",
        json!({"projectId": f.project, "promptId": prompt["promptId"], "answer": {"kind": "askpass", "value": SECRET}}),
    );
    assert!(answered > 0);
    assert_eq!(fetch.await.unwrap(), Ok(()));
    assert_eq!(
        fs::read_to_string(out.with_extension("code"))
            .unwrap()
            .trim(),
        "0"
    );
    assert_eq!(fs::read_to_string(&out).unwrap(), format!("{SECRET}\n"));

    // 끝난 쓰기의 토큰 — 거절이고 창에 아무것도 뜨지 않는다.
    let token = fs::read_to_string(out.with_extension("token")).unwrap();
    assert_eq!(token.len(), 32);
    let late = command::request(
        &socket,
        uid,
        Command::Askpass,
        Askpass::request_body(&token, QUESTION, Default::default()),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(late.contains("does not belong"), "{late}");
    assert!(window.prompt(Duration::from_millis(300)).await.is_none());
    shutdown.cancel();
}

/// git 이 띄운 것이 아니면(소켓 · 토큰 환경이 없으면) 도우미는 묻지 않고 2 로 끝난다 — `pw_git` 의 도우미와 같은 약속.
#[test]
fn the_askpass_helper_needs_the_git_environment() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pitwall-relay"))
        .args(["askpass", "Password: "])
        .env_remove("PITWALL_ASKPASS_SOCKET")
        .env_remove("PITWALL_ASKPASS_TOKEN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
