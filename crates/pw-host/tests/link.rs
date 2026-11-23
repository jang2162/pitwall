//! 조립한 Host 를 링크 너머에 둔 시험(18-m7-plan M7-03 ② · M7-02 ②). Relay 자리는 `pw_link::serve` 가 조립한 Host 를 메모리
//! 전송으로 내놓고, Desktop 자리는 `pw_link::RemoteHost` 다.
//!
//! - 링크의 role 은 링크가 증명한 기기의 role 이다 — `readonly` 기기의 링크가 `owner` · `Project` 범위를 주장해도 `fs.write` ·
//!   `mcp.approval.answer` 는 role 로 거절된다(자르기를 걷으면 실패).
//! - 페어링 표에 없는 기기의 링크는 서지 않는다 — owner · `Project` 범위를 얻지 못하고 승인 답에 닿지 못한다(증명 검사를 걷으면
//!   실패).
//! - 채널을 닫으면 Host 의 그 연결이 닫힌다 — 그 연결로 연 터미널의 셸이 끝난다.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pw_core::registry::NewProject;
use pw_core::{Connection, Host, HostError};
use pw_db::secret::MemorySecretStore;
use pw_link::{
    DesktopIdentity, DeviceKey, DialError, Expect, LinkLimits, Pairing, PairingTable, RejectReason,
    RelayIdentity, RelayParts, RemoteHost, Versions,
};
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::json;

/// 연결 수를 세는 Relay 쪽 Host.
struct Counted {
    inner: pw_core::LocalHost,
    connects: AtomicUsize,
}

impl Host for Counted {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn connect(&self, caller: Caller) -> pw_core::BoxFuture<'_, Result<Connection, HostError>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.inner.connect(caller)
    }
}

struct Linked {
    host: Arc<Counted>,
    remote: RemoteHost,
    project: String,
    root: std::path::PathBuf,
}

/// 조립한 Host(프로젝트 하나)를 Relay 로 두고, `paired` role 로 페어링한(또는 하지 않은) Desktop 을 붙인다.
async fn linked(base: &std::path::Path, paired: Option<Role>) -> (Linked, Result<(), DialError>) {
    linked_as(base, paired, false).await
}

/// `relay` 면 Relay 의 조립(`assemble_relay` — Host 수명 터미널)을 링크 너머에 둔다.
async fn linked_as(
    base: &std::path::Path,
    paired: Option<Role>,
    relay: bool,
) -> (Linked, Result<(), DialError>) {
    let assemble = if relay {
        common::assemble_relay
    } else {
        common::assemble
    };
    let assembled = assemble(
        &base.join("relay"),
        Arc::new(MemorySecretStore::new()),
        tokio::runtime::Handle::current(),
    );
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let project = assembled
        .host
        .registry()
        .unwrap()
        .create(NewProject {
            root: root.clone(),
            ..Default::default()
        })
        .unwrap()
        .id;
    let host = Arc::new(Counted {
        inner: assembled.host,
        connects: AtomicUsize::new(0),
    });
    let versions = Versions::new("0.0.0", "dev");
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let desktop_key = Arc::new(DeviceKey::generate().unwrap());
    let pairings = PairingTable::new(
        paired
            .map(|role| Pairing {
                key: desktop_key.public(),
                role,
                name: "mac".into(),
            })
            .into_iter()
            .collect(),
    );
    let (transport, acceptor) = pw_link::transport::memory(256 * 1024);
    let parts = RelayParts {
        identity: Arc::new(RelayIdentity {
            key: Arc::clone(&relay_key),
            versions: versions.clone(),
        }),
        pairings,
        host: Arc::clone(&host) as Arc<dyn Host>,
        limits: LinkLimits::default(),
        resumes: pw_link::Resumes::default(),
    };
    tokio::spawn(async move {
        while let Some(stream) = acceptor.accept().await {
            let parts = parts.clone();
            tokio::spawn(async move {
                let _ = pw_link::serve(stream, &parts).await;
            });
        }
    });
    let remote = RemoteHost::new(
        "mini",
        Arc::new(transport),
        Arc::new(DesktopIdentity {
            key: desktop_key,
            versions,
        }),
        Expect::Machine(relay_key.id()),
        LinkLimits::default(),
    );
    let attached = remote.attach().await.map(|_| ());
    (
        Linked {
            host,
            remote,
            project,
            root,
        },
        attached,
    )
}

/// 프로젝트 창의 주장(owner · 그 프로젝트 범위).
fn project_window(project: &str) -> Caller {
    Caller {
        role: Role::Owner,
        scope: ConnectionScope::Project(project.to_owned()),
        window: Some(format!("project-{project}")),
    }
}

fn base() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
    (tmp, base)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_readonly_link_claiming_owner_cannot_write_or_answer_approvals() {
    let (_tmp, base) = base();
    let (linked, attached) = linked(&base, Some(Role::Readonly)).await;
    attached.unwrap();
    let mut window = linked
        .remote
        .connect(project_window(&linked.project))
        .await
        .unwrap();

    let file = linked.root.join("a.txt");
    let denied = common::call(
        &mut window,
        1,
        "fs.write",
        json!({
            "projectId": linked.project,
            "path": file.display().to_string(),
            "content": {"kind": "text", "text": "hello"},
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        denied["code"],
        json!(pw_rpc::code::PERMISSION_DENIED),
        "{denied}"
    );
    assert_eq!(denied["data"]["role"], "readonly", "{denied}");
    assert!(!file.exists());

    let denied = common::call(
        &mut window,
        2,
        "mcp.approval.answer",
        json!({"id": 1, "answer": "once"}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        denied["code"],
        json!(pw_rpc::code::PERMISSION_DENIED),
        "{denied}"
    );
    assert_eq!(denied["data"]["role"], "readonly", "{denied}");

    // 읽기는 된다.
    common::call(&mut window, 3, "host.info", serde_json::Value::Null)
        .await
        .unwrap();
}

/// 같은 주장이 증명된 owner 링크에서는 지난다 — 위 시험의 거절이 role 자르기 때문임을 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proven_owner_link_writes_with_its_project_scope() {
    let (_tmp, base) = base();
    let (linked, attached) = linked(&base, Some(Role::Owner)).await;
    attached.unwrap();
    let mut window = linked
        .remote
        .connect(project_window(&linked.project))
        .await
        .unwrap();
    let file = linked.root.join("a.txt");
    common::call(
        &mut window,
        1,
        "fs.write",
        json!({
            "projectId": linked.project,
            "path": file.display().to_string(),
            "content": {"kind": "text", "text": "hello"},
        }),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello");
}

/// 데몬 소켓에 붙었지만 기기 키를 증명하지 못한(페어링 표에 없는) 링크는 서지 않는다 — owner · `Project` 범위를 얻지 못하고
/// 승인 답(`mcp.approval.answer`)에 닿지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unproven_link_gets_no_owner_scope_and_cannot_answer_approvals() {
    let (_tmp, base) = base();
    let (linked, attached) = linked(&base, None).await;
    assert_eq!(
        attached.unwrap_err(),
        DialError::Rejected(RejectReason::Unpaired)
    );
    let result = linked.remote.connect(project_window(&linked.project)).await;
    assert!(
        matches!(result, Err(HostError::Disconnected(_))),
        "{:?}",
        result.err()
    );
    assert_eq!(
        linked.host.connects.load(Ordering::SeqCst),
        0,
        "a connection reached the host"
    );
}

/// 채널을 닫으면 그 연결로 연 터미널의 셸이 끝난다(Host 의 연결 수명 — 18-m7-plan M7-03 시험 ②). Desktop 의 조립이다 — Relay 의
/// 조립은 아래 시험(Host 수명).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_channel_ends_the_terminal_shell_on_the_host() {
    let (_tmp, base) = base();
    let (linked, attached) = linked(&base, Some(Role::Owner)).await;
    attached.unwrap();
    let mut window = linked
        .remote
        .connect(project_window(&linked.project))
        .await
        .unwrap();
    let opened = common::call(
        &mut window,
        1,
        "terminal.create",
        json!({"projectId": linked.project, "cols": 80, "rows": 24}),
    )
    .await
    .unwrap();
    let stream = opened["streamId"].clone();
    window.sink.send(
        json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream, "credit": 1000}}),
    );
    let pid = loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(20), window.source.recv())
            .await
            .expect("no terminal output")
            .expect("connection closed");
        if frame["method"] == "stream.chunk" && frame["params"]["data"]["kind"] == "started" {
            break frame["params"]["data"]["pid"]
                .as_u64()
                .expect("the shell pid") as u32;
        }
    };
    assert!(alive(pid), "the shell is not running");

    drop(window);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while alive(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "the shell outlived the closed channel"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// 스트림의 첫 항목 `kind` 가 나올 때까지 받아 그 항목을 준다.
async fn stream_item(window: &mut Connection, kind: &str) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(20), window.source.recv())
            .await
            .unwrap_or_else(|_| panic!("no {kind} item"))
            .expect("connection closed");
        if frame["method"] == "stream.chunk" && frame["params"]["data"]["kind"] == kind {
            return frame["params"]["data"].clone();
        }
    }
}

/// Relay 의 조립에서는 채널(연결)이 닫혀도 터미널의 셸이 살고, 새 채널이 `terminal.attach` 로 다시 붙어 입력 · 출력이 이어진다
/// (18-m7-plan M7-09 ② ③ — Relay 조립의 터미널을 연결 수명으로 되돌리면 실패).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_terminal_outlives_the_channel_and_a_new_channel_attaches() {
    let (_tmp, base) = base();
    let (linked, attached) = linked_as(&base, Some(Role::Owner), true).await;
    attached.unwrap();
    let mut window = linked
        .remote
        .connect(project_window(&linked.project))
        .await
        .unwrap();
    let opened = common::call(
        &mut window,
        1,
        "terminal.create",
        json!({"projectId": linked.project, "cols": 80, "rows": 24}),
    )
    .await
    .unwrap();
    window.sink.send(
        json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened["streamId"], "credit": 1000}}),
    );
    let started = stream_item(&mut window, "started").await;
    let pid = started["pid"].as_u64().unwrap() as u32;
    let terminal = started["terminalId"].clone();
    drop(window);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(alive(pid), "the relay's shell died with its channel");

    let mut window = linked
        .remote
        .connect(project_window(&linked.project))
        .await
        .unwrap();
    let listed = common::call(
        &mut window,
        1,
        "terminal.list",
        json!({"projectId": linked.project}),
    )
    .await
    .unwrap();
    assert_eq!(listed["terminals"][0]["terminalId"], terminal);
    assert_eq!(listed["terminals"][0]["attached"], false);
    let opened = common::call(
        &mut window,
        2,
        "terminal.attach",
        json!({"terminalId": terminal, "since": 0}),
    )
    .await
    .unwrap();
    window.sink.send(
        json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened["streamId"], "credit": 1000}}),
    );
    stream_item(&mut window, "attached").await;
    common::call(
        &mut window,
        3,
        "terminal.input",
        json!({"terminalId": terminal, "data": "echo AGAIN-$((6*7))\n"}),
    )
    .await
    .unwrap();
    let mut text = String::new();
    while !text.contains("AGAIN-42") {
        let output = stream_item(&mut window, "output").await;
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(output["data"].as_str().unwrap())
            .unwrap();
        text.push_str(&String::from_utf8_lossy(&bytes));
    }
    common::call(
        &mut window,
        4,
        "terminal.close",
        json!({"terminalId": terminal}),
    )
    .await
    .unwrap();
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
