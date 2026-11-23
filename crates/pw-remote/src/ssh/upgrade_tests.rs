//! 판 바꾸기 배선 시험(18-m7-plan M7-07b Rust 시험 ① ②) — 붙기(핸드셰이크)만 가짜 링크로 대신하고, 붙은 뒤의 조사 · 링크 옮기기 ·
//! 정리 · [옛 판 끝내기] 는 **실제 `SshBackend`** 가 가짜 `ssh`(`pw_link::fake_ssh` — 실제 SSH 는 쓰지 않는다)로 원격 셸 스크립트를
//! 돌린다. 원격의 「Relay 바이너리」 는 `status` · `link-command` 에만 답하는 셸 스크립트다(`pw_link` 의 `upgrade_tests` 와 같은 꼴).
//! RPC 는 실제 `methods::register` 를 `LocalHost` 에 붙여 연결 범위(Host · Project)를 그대로 지난다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use pw_core::{Host, LocalHost};
use pw_link::fake_ssh::{FakeRemote, write_executable};
use pw_link::install::Fetch;
use pw_link::{DeviceKey, DeviceKeyStore};
use pw_rpc::ConnectionScope;
use serde_json::json;

use super::*;
use crate::{RemoteHosts, RemoteRelays, methods};

const VERSION: &str = "1.2.3";
const MACHINE: &str = "SHA256:mini";

struct Fixed;

impl DeviceKeyStore for Fixed {
    fn load_or_create(&self) -> Result<DeviceKey, pw_link::device::KeyStoreError> {
        Ok(DeviceKey::from_seed(&[7; 32]))
    }
}

struct Offline;

impl Fetch for Offline {
    fn get<'a>(&'a self, _url: &'a str, _max: u64) -> BoxFuture<'a, std::io::Result<Vec<u8>>> {
        Box::pin(async { Err(std::io::Error::other("offline")) })
    }
}

/// 붙기는 가짜 링크(증명된 머신 [`MACHINE`]), 붙은 뒤의 원격 일은 실제 `SshBackend`.
struct Linked {
    ssh: SshBackend,
    connects: AtomicUsize,
}

impl Backend for Linked {
    fn hosts(&self, _settings: Option<&Value>) -> (Vec<HostEntry>, Vec<String>) {
        (vec![mini()], Vec::new())
    }

    fn connect<'a>(
        &'a self,
        _entry: &'a HostEntry,
        _expect: Option<&'a str>,
        _progress: &'a (dyn Fn(Progress) + Send + Sync),
    ) -> BoxFuture<'a, Result<Link, ConnectError>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let tx = Mutex::new(Some(tx));
        Box::pin(async move {
            Ok(Link {
                host: Arc::new(LocalHost::new(MACHINE)),
                machine: MACHINE.into(),
                closed: Box::pin(async move { rx.await.unwrap_or_else(|_| "closed".into()) }),
                close: Box::new(move || {
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send("closed by us".into());
                    }
                }),
            })
        })
    }

    fn check<'a>(&'a self, _entry: &'a HostEntry) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async { None })
    }

    fn tidy<'a>(&'a self, entry: &'a HostEntry) -> BoxFuture<'a, Result<Tidied, String>> {
        self.ssh.tidy(entry)
    }

    fn stop_old<'a>(
        &'a self,
        entry: &'a HostEntry,
        seen: &'a RemoteOldRelay,
    ) -> BoxFuture<'a, Result<RemoteStopOutcome, String>> {
        self.ssh.stop_old(entry, seen)
    }

    fn link_command<'a>(
        &'a self,
        entry: &'a HostEntry,
        on: bool,
    ) -> BoxFuture<'a, Result<bool, String>> {
        self.ssh.link_command(entry, on)
    }
}

fn mini() -> HostEntry {
    HostEntry {
        name: "mini".into(),
        host: "mini".into(),
        user: None,
        port: None,
        source: RemoteHostSource::SshConfig,
        ssh_args: Vec::new(),
    }
}

/// 원격의 가짜 Relay 하나(`pitwall-relay-dev-<판>`). 표지 파일에 status JSON 을 쓰면 떠 있는 데몬이다. `link-command` 로 불린 인자는
/// `<표지>.link` 에 남는다.
struct Stub {
    marker: PathBuf,
    version: String,
}

impl Stub {
    fn new(dir: &Path, remote: &FakeRemote, version: &str) -> Self {
        let marker = dir.join(format!("marker-{version}"));
        std::fs::create_dir_all(remote.relay_dir()).unwrap();
        write_executable(
            &remote
                .relay_dir()
                .join(format!("pitwall-relay-dev-{version}")),
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  status) if [ -f '{m}' ]; then cat '{m}'; exit 0; else exit 3; fi ;;\n  link-command) echo \"$*\" >> '{m}.link'; if [ \"$2\" = --remove ]; then echo '{{\"path\":\"p\",\"linked\":false}}'; else echo '{{\"path\":\"p\",\"linked\":true}}'; fi ;;\n  *) exit 2 ;;\nesac\n",
                m = marker.display()
            ),
        );
        Self {
            marker,
            version: version.to_owned(),
        }
    }

    fn file(&self) -> String {
        format!("pitwall-relay-dev-{}", self.version)
    }

    fn run(&self, pid: u32, runs: usize, terminals: usize, links: usize) {
        std::fs::write(
            &self.marker,
            format!(
                "{{\"version\":\"{}\",\"releaseChannel\":\"dev\",\"name\":null,\"pid\":{pid},\"uptimeMs\":5,\"links\":{links},\"ownerLinks\":{links},\"runs\":{runs},\"terminals\":{terminals}}}\n",
                self.version
            ),
        )
        .unwrap();
    }

    fn link_calls(&self) -> Vec<String> {
        std::fs::read_to_string(format!("{}.link", self.marker.display()))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

/// 「데몬」 프로세스(`sleep`). 끝나면 거둔다.
struct Sleeper {
    pid: u32,
}

impl Sleeper {
    fn start() -> Self {
        let mut child = std::process::Command::new("sleep")
            .arg("300")
            .spawn()
            .unwrap();
        let pid = child.id();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Self { pid }
    }

    fn alive(&self) -> bool {
        signal("-0", self.pid)
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        signal("-KILL", self.pid);
    }
}

fn signal(signal: &str, pid: u32) -> bool {
    std::process::Command::new("kill")
        .args([signal, &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

struct Setup {
    dir: tempfile::TempDir,
    remote: FakeRemote,
    hosts: RemoteHosts,
    backend: Arc<Linked>,
    rpc: LocalHost,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let remote = FakeRemote::new(dir.path(), "home", true);
    let ssh = SshBackend::new(SshParts {
        keys: Arc::new(Fixed),
        dist: dir.path().join("cache/relay-dist"),
        fetch: Arc::new(Offline),
        askpass: None,
        device_name: "mac".into(),
        home: None,
    })
    .with_program(
        remote.ssh.clone(),
        remote.env(&[]),
        Versions::new(VERSION, "dev"),
        Duration::from_millis(10),
    );
    let backend = Arc::new(Linked {
        ssh,
        connects: AtomicUsize::new(0),
    });
    let hosts = RemoteHosts::default();
    hosts.attach_host(None, pw_core::EventBus::new(64), None);
    hosts.attach_backend(backend.clone());
    let registered = hosts.clone();
    let rpc = LocalHost::builder("local")
        .methods(move |r| methods::register(r, &registered))
        .build();
    Setup {
        dir,
        remote,
        hosts,
        backend,
        rpc,
    }
}

impl Setup {
    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.remote.relay_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// 조사가 끝날 때까지(`remote.relays` 가 차거나 바뀔 때까지).
    async fn relays(&self, unlike: Option<&RemoteRelays>) -> RemoteRelays {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if let Some(now) = self.hosts.relays("mini")
                    && Some(&now) != unlike
                {
                    return now;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the survey never came")
    }

    async fn call(&self, scope: ConnectionScope, method: &str, params: Value) -> Value {
        let mut conn = self
            .rpc
            .connect(pw_rpc::Caller {
                role: pw_rpc::Role::Owner,
                scope,
                window: None,
            })
            .await
            .unwrap();
        conn.sink
            .send(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
        loop {
            let frame = conn.source.recv().await.unwrap();
            if frame.get("id") == Some(&json!(1)) {
                return frame;
            }
        }
    }
}

/// ① 붙은 뒤 정리가 **한 번** 돈다: 조사 → 사람이 둔 `pitwall` 링크가 옛 판이면 이 판으로 옮김 → 최근 두 판만 남김. 떠 있는 데몬의
/// 바이너리는 가장 오래된 판이어도 남고, 화면에 보일 옛 판 데몬(실행 · 터미널 · 붙은 링크 수)이 `remote.relays` 에 온다. 같은 링크를
/// 더 쓴다고 정리가 다시 돌지 않는다.
#[tokio::test]
async fn connecting_cleans_up_once_and_keeps_a_running_daemons_binary() {
    let s = setup();
    let current = Stub::new(s.dir.path(), &s.remote, VERSION);
    Stub::new(s.dir.path(), &s.remote, "1.1.0");
    Stub::new(s.dir.path(), &s.remote, "1.0.5");
    let oldest = Stub::new(s.dir.path(), &s.remote, "1.0.0");
    let daemon = Sleeper::start();
    oldest.run(daemon.pid, 1, 2, 1);
    let bin = s.remote.home.join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(
        s.remote.relay_dir().join(oldest.file()),
        bin.join("pitwall"),
    )
    .unwrap();

    let _use = s.hosts.hold("mini").unwrap();
    let relays = s.relays(None).await;
    assert_eq!(relays.machine, MACHINE);
    assert_eq!(
        relays.old,
        [RemoteOldRelay {
            file: oldest.file(),
            version: Some("1.0.0".into()),
            channel: Some("dev".into()),
            pid: daemon.pid,
            runs: 1,
            terminals: 2,
            links: 1,
        }]
    );
    assert_eq!(relays.removed, ["pitwall-relay-dev-1.0.5"]);
    assert_eq!(relays.command, RemoteCommandLink::Linked);
    assert_eq!(
        s.files(),
        [
            "pitwall-relay-dev-1.0.0",
            "pitwall-relay-dev-1.1.0",
            "pitwall-relay-dev-1.2.3"
        ],
        "the running daemon's binary must stay"
    );
    assert!(daemon.alive(), "the clean-up must not stop a daemon");
    assert_eq!(
        current.link_calls(),
        ["link-command"],
        "the person's pitwall link moves to this version before the old one is cleaned"
    );
    let log = s
        .hosts
        .log("mini")
        .into_iter()
        .map(|l| l.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        log.contains("kept pitwall-relay-dev-1.0.0: its daemon is running"),
        "{log}"
    );
    assert!(
        log.contains("removed old relay versions: pitwall-relay-dev-1.0.5"),
        "{log}"
    );

    // 한 번: 조사 · 링크 옮기기 · 정리 — ssh 셋. 더 쓰거나 기다려도 다시 돌지 않는다.
    let calls = s.remote.calls().len();
    assert_eq!(calls, 3, "{:?}", s.remote.calls());
    let _second = s.hosts.hold("mini").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.remote.calls().len(), calls);
    assert_eq!(s.backend.connects.load(Ordering::SeqCst), 1);
}

/// ② 사람이 본 뒤 그 옛 판 데몬에 터미널이 늘면 `remote.stopOld` 는 끝내지 않고 지금 수를 돌려준다. 지금 수를 본 사람이 다시 고르면
/// 끝낸다. 엉뚱한 머신 · 프로젝트 창 · 보이지 않은 바이너리 · 끊긴 뒤에는 원격에 아무것도 보내지 않는다.
#[tokio::test]
async fn stop_old_does_not_stop_a_daemon_that_changed_since_it_was_seen() {
    let s = setup();
    Stub::new(s.dir.path(), &s.remote, VERSION);
    let old = Stub::new(s.dir.path(), &s.remote, "1.0.0");
    let daemon = Sleeper::start();
    old.run(daemon.pid, 0, 1, 0);
    let _use = s.hosts.hold("mini").unwrap();
    let relays = s.relays(None).await;
    let seen = relays.old[0].clone();
    assert_eq!(seen.terminals, 1);
    let calls = s.remote.calls().len();

    // 엉뚱한 자리에서 온 끝내기 — 원격에 닿지 않는다.
    let other_machine = s
        .call(
            ConnectionScope::Host,
            "remote.stopOld",
            json!({"name": "mini", "machine": "SHA256:other", "seen": seen}),
        )
        .await;
    assert!(other_machine.get("error").is_some(), "{other_machine}");
    let from_project = s
        .call(
            ConnectionScope::Project("p".into()),
            "remote.stopOld",
            json!({"name": "mini", "machine": MACHINE, "seen": seen}),
        )
        .await;
    assert_eq!(
        from_project["error"]["code"],
        json!(pw_rpc::code::PERMISSION_DENIED),
        "{from_project}"
    );
    let mut current = seen.clone();
    current.file = format!("pitwall-relay-dev-{VERSION}");
    let unseen = s
        .call(
            ConnectionScope::Host,
            "remote.stopOld",
            json!({"name": "mini", "machine": MACHINE, "seen": current}),
        )
        .await;
    assert!(unseen.get("error").is_some(), "{unseen}");
    assert_eq!(
        s.remote.calls().len(),
        calls,
        "a refused stop reached the remote"
    );
    assert!(daemon.alive());

    // 본 뒤 터미널이 하나 늘었다.
    old.run(daemon.pid, 0, 2, 0);
    let changed = s
        .call(
            ConnectionScope::Host,
            "remote.stopOld",
            json!({"name": "mini", "machine": MACHINE, "seen": seen}),
        )
        .await;
    assert_eq!(changed["result"]["outcome"], "changed", "{changed}");
    assert_eq!(changed["result"]["now"]["terminals"], 2, "{changed}");
    assert!(
        daemon.alive(),
        "stopped a daemon the person did not see as it is"
    );
    let relays = s.relays(Some(&relays)).await;
    assert_eq!(relays.old[0].terminals, 2, "the screen shows the new count");

    // 지금 수를 본 사람이 다시 고르면 끝낸다.
    let stopped = s
        .call(
            ConnectionScope::Host,
            "remote.stopOld",
            json!({"name": "mini", "machine": MACHINE, "seen": relays.old[0]}),
        )
        .await;
    assert_eq!(stopped["result"]["outcome"], "stopped", "{stopped}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while daemon.alive() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the old daemon did not end");

    // 끊긴 뒤에는 그 머신의 것을 끝내지 않는다.
    s.hosts.disconnect("mini").unwrap();
    assert_eq!(s.hosts.relays("mini"), None);
    let calls = s.remote.calls().len();
    let after = s
        .call(
            ConnectionScope::Host,
            "remote.stopOld",
            json!({"name": "mini", "machine": MACHINE, "seen": relays.old[0]}),
        )
        .await;
    assert!(after.get("error").is_some(), "{after}");
    assert_eq!(s.remote.calls().len(), calls);
}

/// `pitwall` 명령 링크(메인 판정 M7-P12)는 사람이 고를 때만 — 이 판의 바이너리의 `link-command` 로. 엉뚱한 머신이면 부르지 않는다.
#[tokio::test]
async fn the_command_link_follows_the_person_on_the_seen_machine_only() {
    let s = setup();
    let current = Stub::new(s.dir.path(), &s.remote, VERSION);
    let _use = s.hosts.hold("mini").unwrap();
    let relays = s.relays(None).await;
    assert_eq!(relays.command, RemoteCommandLink::Absent);
    assert!(
        current.link_calls().is_empty(),
        "connecting must not link the command on its own"
    );

    let refused = s
        .call(
            ConnectionScope::Host,
            "remote.linkCommand",
            json!({"name": "mini", "machine": "SHA256:other", "on": true}),
        )
        .await;
    assert!(refused.get("error").is_some(), "{refused}");
    assert!(current.link_calls().is_empty());

    for on in [true, false] {
        let done = s
            .call(
                ConnectionScope::Host,
                "remote.linkCommand",
                json!({"name": "mini", "machine": MACHINE, "on": on}),
            )
            .await;
        assert!(done.get("error").is_none(), "{done}");
    }
    assert_eq!(
        current.link_calls(),
        ["link-command", "link-command --remove"]
    );
}
