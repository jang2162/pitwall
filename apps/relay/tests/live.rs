//! `relay-live`(18-m7-plan M7-05 ③): 실제 `ssh` 로 Linux 컨테이너(sshd)에 Relay 를 올리고 `proxy` 로 붙어 링크 · RPC 를 돈다.
//! `scripts/relay-live.mjs` 가 컨테이너를 띄우고 이 시험만 돌린 뒤 실행 개수를 하한(scripts/relay-live.toml)과 견준다 — CI 의
//! `relay-live` 잡. **이 기기(개발자 맥)의 sshd 는 쓰지 않는다.**
//!
//! 환경(스크립트가 넘긴다):
//! - `PW_TEST_RELAY_SSH_URL` — `ssh://<사용자>@<호스트>:<포트>`. 없으면 이 파일의 시험이 목록에 없다(build.rs).
//! - `PW_TEST_RELAY_SSH_DIR` — 클라이언트 키(`client`)와 호스트 키를 박은 `known_hosts`. 호스트 키 확인을 끄지 않는다.
//! - `PW_TEST_RELAY_BIN` — 원격에 올릴 Relay 바이너리(원격 OS · 아키텍처의 것 — CI 는 x86_64 musl, `scripts/relay-build.mjs`).
//! - `PW_TEST_RELAY_CONTAINER` — 컨테이너 이름(끊김 시나리오가 `docker pause` 한다).
//!
//! 시나리오 대부분은 원격 `~/t-<이름>/` 에 따로 `PITWALL_*_DIR` 을 두고 손 절차로 올린다(임시 이름 → 원격 해시 → 0700 · rename,
//! 페어링 표 파일을 SSH 로). **설치기 시나리오**(`live_relay_the_installer_installs_pairs_and_reconnects` — M7-06)는 실제 설치기
//! (`pw_link::install`)로 컨테이너 사용자의 기본 `$DATA/relay/` 에 올리고 `pair --from-ssh` 로 페어링한다.
//!
//! 시나리오: 설치 · RPC 왕복, 다시 붙기(같은 데몬 — SSH 세션이 끝나도 데몬이 남는다), 끊김(컨테이너를 얼려 링크가 조용해짐 → 링크가
//! 끊겼다고 보고, 풀면 다시 붙어 같은 데몬), 페어링 안 된 기기 거절, 커밋 보내기(이 기기의 Relay 데몬 → 원격 체크아웃, M7-16 RPC).
//! 판 바꾸기는 M7-07 이 판 둘을 만드는 시나리오로 더한다.

#![cfg(all(unix, pw_live_relay))]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::{Connection, Host};
use pw_link::{
    DesktopIdentity, DeviceKey, Expect, LinkLimits, LinkStream, Pairing, RemoteHost, Transport,
};
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn env(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("{name} is not set (scripts/relay-live.mjs sets it)"))
}

/// SSH 대상. 키 · known_hosts 는 스크립트가 만든 것만 쓴다(`-F /dev/null` — 사람의 `~/.ssh/config` 를 읽지 않는다).
#[derive(Clone)]
struct Remote {
    user_host: String,
    port: String,
    dir: PathBuf,
}

impl Remote {
    fn from_env() -> Self {
        let url = env("PW_TEST_RELAY_SSH_URL");
        let rest = url.strip_prefix("ssh://").expect("ssh:// url");
        let (user_host, port) = rest.rsplit_once(':').expect("ssh://user@host:port");
        Self {
            user_host: user_host.to_owned(),
            port: port.to_owned(),
            dir: PathBuf::from(env("PW_TEST_RELAY_SSH_DIR")),
        }
    }

    /// 원격 명령 하나(`sh -c` 로 푼다 — 고정 글만 넘긴다).
    fn command(&self, remote: &str) -> Command {
        let mut command = Command::new("ssh");
        command.args(self.args()).arg(remote);
        command
    }

    fn args(&self) -> Vec<String> {
        let mut args = self.options();
        args.push(self.user_host.clone());
        args
    }

    /// 대상 없이 옵션만(`GIT_SSH_COMMAND` 에도 쓴다).
    fn options(&self) -> Vec<String> {
        let known = self.dir.join("known_hosts");
        vec![
            "-F".into(),
            "/dev/null".into(),
            "-T".into(),
            "-p".into(),
            self.port.clone(),
            "-i".into(),
            self.dir.join("client").display().to_string(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-o".into(),
            format!("UserKnownHostsFile={}", known.display()),
            "-o".into(),
            "LogLevel=ERROR".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
        ]
    }

    /// 원격 명령을 돌리고 stdout. 실패하면 패닉.
    fn sh(&self, remote: &str) -> String {
        let out = self.command(remote).output().unwrap();
        assert!(
            out.status.success(),
            "ssh {remote}: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// stdin 을 원격 명령에 흘린다.
    fn sh_with_input(&self, remote: &str, input: &Path) {
        let out = self
            .command(remote)
            .stdin(std::fs::File::open(input).unwrap())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "ssh {remote}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// 원격의 시험 자리 하나: `~/t-<이름>/`(바이너리 · `PITWALL_*_DIR` · 작업 폴더).
struct Site {
    remote: Remote,
    base: String,
}

impl Site {
    fn new(name: &str) -> Self {
        let remote = Remote::from_env();
        let home = remote.sh("printf %s \"$HOME\"");
        let base = format!("{home}/t-{name}");
        // 앞 실행이 남긴 데몬 · 파일을 치운다(같은 컨테이너로 다시 돌릴 때).
        remote.sh(&format!(
            "pkill -f '{base}/pitwall-relay' 2>/dev/null; rm -rf '{base}' && mkdir -p '{base}/work'"
        ));
        Self { remote, base }
    }

    fn bin(&self) -> String {
        format!("{}/pitwall-relay", self.base)
    }

    /// `PITWALL_*_DIR` 을 붙인 원격 명령.
    fn relay(&self, args: &str) -> String {
        format!(
            "env PITWALL_CONFIG_DIR='{b}/config' PITWALL_DATA_DIR='{b}/data' PITWALL_STATE_DIR='{b}/state' PITWALL_CACHE_DIR='{b}/cache' '{bin}' {args}",
            b = self.base,
            bin = self.bin()
        )
    }

    /// 바이너리를 올린다: 임시 이름 → 원격 해시 확인 → 0700 → rename(M7-06 의 설치 순서와 같다).
    fn install(&self) {
        let local = PathBuf::from(env("PW_TEST_RELAY_BIN"));
        let expected = sha256_local(&local);
        let tmp = format!("{}/.pitwall-relay.tmp", self.base);
        self.remote.sh_with_input(&format!("cat > '{tmp}'"), &local);
        let remote_sum = self.remote.sh(&format!("sha256sum '{tmp}'"));
        assert_eq!(
            remote_sum.split_whitespace().next(),
            Some(expected.as_str()),
            "the uploaded binary hash differs"
        );
        self.remote.sh(&format!(
            "chmod 700 '{tmp}' && mv -f '{tmp}' '{}'",
            self.bin()
        ));
        let version = self.remote.sh(&format!("'{}' --version", self.bin()));
        assert!(version.contains(VERSION), "{version}");
    }

    /// relay.toml(`roots` 와 짧은 유휴 시한)과 페어링 표(이 Desktop 키를 owner 로)를 쓴다.
    fn configure(&self, owners: &[&DeviceKey]) {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("pairings.json");
        pw_link::pairing::save(
            &table,
            &owners
                .iter()
                .map(|key| Pairing {
                    key: key.public(),
                    role: Role::Owner,
                    name: "relay-live".into(),
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let toml = dir.path().join("relay.toml");
        std::fs::write(
            &toml,
            format!(
                "schema = 1\nroots = [\"{}/work\"]\n[idle]\nexit_after = \"60s\"\n",
                self.base
            ),
        )
        .unwrap();
        self.remote.sh(&format!(
            "mkdir -p '{b}/config/relay' '{b}/data/relay' && chmod 700 '{b}/data/relay'",
            b = self.base
        ));
        self.remote.sh_with_input(
            &format!("cat > '{}/config/relay/relay.toml'", self.base),
            &toml,
        );
        self.remote.sh_with_input(
            &format!(
                "umask 077 && cat > '{}/data/relay/pairings.json'",
                self.base
            ),
            &table,
        );
    }

    /// 원격 데몬의 `status`(데몬이 없으면 None).
    fn status(&self) -> Option<Value> {
        let out = self.remote.command(&self.relay("status")).output().unwrap();
        out.status
            .success()
            .then(|| serde_json::from_slice(&out.stdout).expect("status json"))
    }

    fn transport(&self) -> Arc<dyn Transport> {
        Arc::new(SshTransport {
            remote: self.remote.clone(),
            command: self.relay("proxy"),
        })
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = self
            .remote
            .command(&format!("pkill -f '{}/pitwall-relay'", self.base))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn sha256_local(path: &Path) -> String {
    let out = Command::new("sha256sum").arg(path).output();
    let out = match out {
        Ok(out) if out.status.success() => out,
        _ => Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
            .unwrap(),
    };
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

/// `ssh <대상> <relay> proxy` 의 stdio 가 링크 바이트 스트림이다(M7-06 의 전송과 같은 길). ssh 프로세스는 스트림을 버리면 죽는다.
struct SshTransport {
    remote: Remote,
    command: String,
}

struct ChildOut {
    out: tokio::process::ChildStdout,
    _child: tokio::process::Child,
}

impl tokio::io::AsyncRead for ChildOut {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.out).poll_read(cx, buf)
    }
}

impl Transport for SshTransport {
    fn describe(&self) -> String {
        "ssh (relay-live)".into()
    }

    fn open(&self) -> pw_core::BoxFuture<'_, std::io::Result<LinkStream>> {
        Box::pin(async move {
            let mut child = tokio::process::Command::new("ssh")
                .args(self.remote.args())
                .arg(&self.command)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()?;
            let stdin = child.stdin.take().unwrap();
            let out = child.stdout.take().unwrap();
            Ok(LinkStream::new(ChildOut { out, _child: child }, stdin))
        })
    }
}

fn identity(key: Arc<DeviceKey>) -> Arc<DesktopIdentity> {
    Arc::new(DesktopIdentity {
        key,
        versions: pw_link::Versions::new(
            VERSION,
            option_env!("PITWALL_RELEASE_CHANNEL").unwrap_or("dev"),
        ),
    })
}

fn quick_limits() -> LinkLimits {
    LinkLimits {
        ping_every: Duration::from_secs(1),
        dead_after: Duration::from_secs(5),
        ..LinkLimits::default()
    }
}

/// JSON-RPC 한 연결.
struct Rpc {
    conn: Connection,
    next: i64,
}

impl Rpc {
    fn new(conn: Connection) -> Self {
        Self { conn, next: 0 }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.conn
            .sink
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(120), self.conn.source.recv())
                .await
                .unwrap_or_else(|_| panic!("{method}: no reply"))
                .unwrap_or_else(|| panic!("{method}: closed"));
            if frame["id"] == json!(id) {
                assert!(frame.get("error").is_none(), "{method}: {frame}");
                return frame["result"].clone();
            }
        }
    }

    /// 스트림을 열고 끝까지 받은 항목들.
    async fn stream(&mut self, method: &str, params: Value) -> Vec<Value> {
        let opened = self.call(method, params).await;
        let stream_id = opened["streamId"].clone();
        self.conn.sink.send(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream_id, "credit": 1000}}));
        let mut items = Vec::new();
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(120), self.conn.source.recv())
                .await
                .expect("stream stalled")
                .expect("closed");
            match frame["method"].as_str() {
                Some("stream.chunk") if frame["params"]["streamId"] == stream_id => {
                    items.push(frame["params"]["data"].clone());
                }
                Some("stream.end") if frame["params"]["streamId"] == stream_id => {
                    assert_eq!(frame["params"]["status"], "completed", "{frame}");
                    return items;
                }
                _ => {}
            }
        }
    }
}

/// 프로젝트를 등록하고 그 창의 연결을 연다.
async fn project_window(host: &RemoteHost, root: &str) -> (String, Rpc) {
    let mut welcome = Rpc::new(host.connect(Caller::new(Role::Owner)).await.unwrap());
    let created = welcome.call("project.create", json!({"root": root})).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let window = host
        .connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(id.clone()),
            window: Some("relay-live".into()),
        })
        .await
        .unwrap();
    (id, Rpc::new(window))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// 준비 확인(실행 개수 하한에서 뺀다): sshd 가 키로 받고 컨테이너의 git 설치가 끝났다. 컨테이너는 시작할 때 패키지를 받으므로
/// 180 s 까지 기다린다.
#[test]
fn live_relay_ready() {
    let remote = Remote::from_env();
    let started = Instant::now();
    loop {
        let ok = remote
            .command("command -v git && command -v sha256sum")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(180),
            "the sshd container did not come up"
        );
        std::thread::sleep(Duration::from_secs(2));
    }
    assert!(Path::new(&env("PW_TEST_RELAY_BIN")).is_file());
}

/// 설치 → `proxy` 로 붙기 → 프로젝트 등록 · 파일 검색 · git 상태(실제 ssh · 실제 Relay 바이너리 · Linux).
#[test]
fn live_relay_installs_and_answers_rpc_over_ssh() {
    let site = Site::new("install");
    site.install();
    let key = Arc::new(DeviceKey::generate().unwrap());
    site.configure(&[&key]);
    let repo = format!("{}/work/repo", site.base);
    site.remote.sh(&format!(
        "git init -q -b main '{repo}' && printf 'hello\\n' > '{repo}/hello.rs' && git -C '{repo}' -c user.name=t -c user.email=t@t add -A && git -C '{repo}' -c user.name=t -c user.email=t@t commit -qm one"
    ));
    runtime().block_on(async {
        let host = RemoteHost::new(
            "live",
            site.transport(),
            identity(key),
            Expect::FirstContact,
            LinkLimits::default(),
        );
        let machine = host.attach().await.unwrap();
        assert_eq!(host.link().unwrap().role(), Role::Owner);
        let (id, mut window) = project_window(&host, &repo).await;
        let files = window
            .call("search.files", json!({"projectId": id, "query": "hello"}))
            .await;
        assert!(files.to_string().contains("hello.rs"), "{files}");
        let status = window.call("git.status", json!({"projectId": id})).await;
        assert!(status.is_object(), "{status}");
        drop(window);
        host.detach();
        // 머신 id 는 Relay 기기 키의 지문이다 — 다시 붙어도 같다(다음 시험이 본다).
        assert!(!machine.to_string().is_empty());
    });
    assert_eq!(site.status().unwrap()["version"], VERSION);
}

/// 두 번째로 붙으면 같은 데몬이다. 앞 SSH 세션이 끝나도 데몬은 남는다(`proxy` 의 `setsid` — M7-04 가 넘긴 확인).
#[test]
fn live_relay_reattaches_to_the_same_daemon_after_the_ssh_session_ends() {
    let site = Site::new("reattach");
    site.install();
    let key = Arc::new(DeviceKey::generate().unwrap());
    site.configure(&[&key]);
    let rt = runtime();
    let first = rt.block_on(async {
        let host = RemoteHost::new(
            "live",
            site.transport(),
            identity(Arc::clone(&key)),
            Expect::FirstContact,
            LinkLimits::default(),
        );
        let machine = host.attach().await.unwrap();
        host.detach();
        machine
    });
    // ssh 프로세스는 끝났다. 데몬은 남아 있다.
    std::thread::sleep(Duration::from_secs(1));
    let before = site.status().expect("the daemon outlives the ssh session");
    let second = rt.block_on(async {
        let host = RemoteHost::new(
            "live",
            site.transport(),
            identity(Arc::clone(&key)),
            Expect::Machine(first.clone()),
            LinkLimits::default(),
        );
        let machine = host.attach().await.unwrap();
        let status = site.status().unwrap();
        assert_eq!(status["links"], 1, "{status}");
        host.detach();
        machine
    });
    assert_eq!(first, second);
    assert_eq!(site.status().unwrap()["pid"], before["pid"]);
}

/// 끊김: 컨테이너를 얼리면(`docker pause` — 네트워크가 조용해진 것과 같다) 링크가 시한 안에 끊겼다고 본다. 풀면 다시 붙고 데몬은 같다.
#[test]
fn live_relay_notices_a_silent_network_and_reattaches_after() {
    let container = env("PW_TEST_RELAY_CONTAINER");
    let site = Site::new("drop");
    site.install();
    let key = Arc::new(DeviceKey::generate().unwrap());
    site.configure(&[&key]);
    let before = runtime().block_on(async {
        let host = RemoteHost::new(
            "live",
            site.transport(),
            identity(Arc::clone(&key)),
            Expect::FirstContact,
            quick_limits(),
        );
        host.attach().await.unwrap();
        let pid = site.status().unwrap()["pid"].clone();
        let link = host.link().unwrap();
        let docker = |verb: &str| {
            let status = Command::new("docker")
                .args([verb, &container])
                .status()
                .unwrap();
            assert!(status.success(), "docker {verb}");
        };
        docker("pause");
        let started = Instant::now();
        let reason = tokio::time::timeout(Duration::from_secs(30), link.closed()).await;
        let waited = started.elapsed();
        docker("unpause");
        let reason = reason.expect("the link did not notice the silence within 30 s");
        assert!(
            waited <= quick_limits().dead_after + Duration::from_secs(5),
            "noticed after {waited:?} ({reason:?})"
        );
        // 풀린 뒤 다시 붙는다.
        let started = Instant::now();
        loop {
            match host.attach().await {
                Ok(_) => break,
                Err(e) => {
                    assert!(
                        started.elapsed() < Duration::from_secs(60),
                        "no reattach: {e:?}"
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
        host.detach();
        pid
    });
    assert_eq!(site.status().unwrap()["pid"], before);
}

/// 페어링 표에 없는 기기의 링크는 거절이다(실제 바이너리 · 실제 ssh 위에서도).
#[test]
fn live_relay_refuses_an_unpaired_desktop() {
    let site = Site::new("unpaired");
    site.install();
    let paired = DeviceKey::generate().unwrap();
    site.configure(&[&paired]);
    let stranger = Arc::new(DeviceKey::generate().unwrap());
    runtime().block_on(async {
        let host = RemoteHost::new(
            "live",
            site.transport(),
            identity(stranger),
            Expect::FirstContact,
            LinkLimits::default(),
        );
        let e = host.attach().await.unwrap_err();
        assert!(format!("{e:?}").contains("Unpaired"), "{e:?}");
    });
}

/// M7-06 설치기: 시스템 ssh 로 감지 → 올리기(원격 해시 확인 · rename) → SSH 자동 페어링 → `proxy` 링크. 다시 붙으면 「이미 있음」 ·
/// 이미 페어링됨이고 같은 머신 id 다. 호스트 키는 스크립트가 만든 known_hosts 로 확인한다(끄지 않는다).
#[test]
fn live_relay_the_installer_installs_pairs_and_reconnects() {
    use pw_link::install::{Installer, Progress, RelayDist};
    use pw_link::ssh::{Ssh, SshTarget};

    struct Offline;
    impl pw_link::install::Fetch for Offline {
        fn get<'a>(
            &'a self,
            _url: &'a str,
            _max: u64,
        ) -> pw_core::BoxFuture<'a, std::io::Result<Vec<u8>>> {
            Box::pin(async { Err(std::io::Error::other("offline")) })
        }
    }

    let remote = Remote::from_env();
    // 앞 실행이 남긴 설치 · 데몬을 치운다. relay.toml 은 짧은 유휴 시한만.
    remote.sh(
        "pkill -f pitwall-relay-dev 2>/dev/null; rm -rf \"$HOME/.local/share/pitwall\" && mkdir -p \"$HOME/.config/pitwall/relay\" && \
         printf 'schema = 1\\n[idle]\\nexit_after = \"30s\"\\n' > \"$HOME/.config/pitwall/relay/relay.toml\"",
    );
    // 개발 빌드 폴더(서명 없는 매니페스트 — debug 설치기의 출처).
    let dist = tempfile::tempdir().unwrap();
    let version = semver::Version::parse(VERSION).unwrap();
    let platform = pw_release::relay::PLATFORMS
        .iter()
        .find(|p| p.asset == "linux-x86_64")
        .unwrap();
    let name = pw_release::relay::asset_name(&version, platform);
    std::fs::copy(env("PW_TEST_RELAY_BIN"), dist.path().join(&name)).unwrap();
    let entry = pw_release::relay::describe(
        &dist.path().join(&name),
        &name,
        pw_release::relay::MAX_BINARY_BYTES,
    )
    .unwrap();
    let manifest = pw_release::relay::assemble(&version, &[entry], false).unwrap();
    std::fs::write(
        dist.path().join(pw_release::relay::MANIFEST_FILE),
        manifest.to_bytes(),
    )
    .unwrap();
    // SAFETY: 이 시험만 도는 프로세스다(nextest).
    unsafe { std::env::set_var(pw_release::relay::DEV_DIST_ENV, dist.path()) };

    let (user, host) = remote.user_host.split_once('@').expect("user@host");
    let mut options = remote.options();
    options.retain(|o| o != "-T");
    let ssh = Ssh::new(
        SshTarget {
            host: host.to_owned(),
            user: Some(user.to_owned()),
            port: None,
        },
        options,
    )
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let installer = Installer::new(
        Arc::new(ssh),
        pw_link::Versions::new(
            VERSION,
            option_env!("PITWALL_RELEASE_CHANNEL").unwrap_or("dev"),
        ),
        RelayDist::new(cache.path().join("relay-dist"), Arc::new(Offline)),
    )
    .unwrap();
    let key = Arc::new(DeviceKey::generate().unwrap());
    runtime().block_on(async {
        let first = installer
            .bootstrap(&key.public(), "relay-live", &|_| {})
            .await
            .unwrap();
        assert!(first.uploaded && first.paired_now, "{first:?}");
        let link = pw_link::dial(
            installer.transport().open().await.unwrap(),
            &identity(Arc::clone(&key)),
            &Expect::Machine(first.machine.clone()),
            quick_limits(),
        )
        .await
        .unwrap();
        assert_eq!(link.role(), Role::Owner);
        link.close();

        let seen = std::sync::Mutex::new(Vec::new());
        let again = installer
            .bootstrap(&key.public(), "relay-live", &|p| {
                seen.lock().unwrap().push(p)
            })
            .await
            .unwrap();
        assert!(!again.uploaded && !again.paired_now, "{again:?}");
        assert_eq!(again.machine, first.machine);
        assert!(seen.lock().unwrap().contains(&Progress::AlreadyInstalled));
        let link = pw_link::dial(
            installer.transport().open().await.unwrap(),
            &identity(Arc::clone(&key)),
            &Expect::Machine(first.machine.clone()),
            quick_limits(),
        )
        .await
        .unwrap();
        assert_eq!(link.role(), Role::Owner);
        link.close();
    });
    let listing = remote.sh("ls -la \"$HOME/.local/share/pitwall/relay\"");
    assert!(listing.contains("rwx------"), "{listing}");
    assert!(!listing.contains(".part."), "{listing}");
    let _ = remote
        .command("pkill -f pitwall-relay-dev")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// 이 기기에서 Relay 데몬 하나를 띄운다(로컬 Host 자리 — Desktop 의 로컬 Host 와 같은 조립). 유닉스 소켓으로 붙는다.
struct LocalDaemon {
    root: tempfile::TempDir,
    child: std::process::Child,
}

impl LocalDaemon {
    fn start(key: &DeviceKey, work: &Path) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = |leaf: &str| root.path().join(leaf);
        std::fs::create_dir_all(dir("config/relay")).unwrap();
        std::fs::write(
            dir("config/relay/relay.toml"),
            format!(
                "schema = 1\nroots = [{:?}]\n[idle]\nexit_after = \"60s\"\n",
                work.display().to_string()
            ),
        )
        .unwrap();
        pw_link::pairing::save(
            &dir("data/relay").join(pw_link::pairing::PAIRINGS_FILE),
            &[Pairing {
                key: key.public(),
                role: Role::Owner,
                name: "relay-live-local".into(),
            }],
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pitwall-relay"));
        command
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        for (var, leaf) in [
            ("PITWALL_CONFIG_DIR", "config"),
            ("PITWALL_DATA_DIR", "data"),
            ("PITWALL_STATE_DIR", "state"),
            ("PITWALL_CACHE_DIR", "cache"),
        ] {
            command.env(var, dir(leaf));
        }
        let child = command.spawn().unwrap();
        let socket = dir("state/relay").join(format!("{VERSION}.sock"));
        let started = Instant::now();
        while !socket.exists() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the local daemon did not come up"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        Self { root, child }
    }

    fn transport(&self) -> Arc<dyn Transport> {
        Arc::new(UnixTransport(
            self.root
                .path()
                .join("state/relay")
                .join(format!("{VERSION}.sock")),
        ))
    }
}

impl Drop for LocalDaemon {
    fn drop(&mut self) {
        // SAFETY: 이 시험이 띄운 데몬이다.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let _ = self.child.wait();
    }
}

struct UnixTransport(PathBuf);

impl Transport for UnixTransport {
    fn describe(&self) -> String {
        "unix".into()
    }

    fn open(&self) -> pw_core::BoxFuture<'_, std::io::Result<LinkStream>> {
        Box::pin(async move {
            let stream = tokio::net::UnixStream::connect(&self.0).await?;
            Ok(LinkStream::split(stream))
        })
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// 커밋 보내기(M7-16 RPC — M7-17 의 Desktop 이 잇는 순서): 이 기기의 체크아웃에서 만든 커밋을 원격 체크아웃으로. 원격의 브랜치가
/// 그 커밋이 되고, 원격 체크아웃의 origin 은 그대로다.
#[test]
fn live_relay_sends_a_commit_to_the_remote_checkout() {
    let site = Site::new("send");
    site.install();
    let key = Arc::new(DeviceKey::generate().unwrap());
    site.configure(&[&key]);
    // 원격: 커밋 하나가 있는 main, origin 은 닿지 않는 자리.
    let remote_repo = format!("{}/work/repo", site.base);
    site.remote.sh(&format!(
        "git init -q -b main '{r}' && git -C '{r}' -c user.name=t -c user.email=t@t commit -q --allow-empty -m base && git -C '{r}' remote add origin /nonexistent/origin.git",
        r = remote_repo
    ));
    let base = site
        .remote
        .sh(&format!("git -C '{remote_repo}' rev-parse HEAD"))
        .trim()
        .to_owned();
    // 로컬: 원격 저장소를 ssh 로 clone 해 같은 base 위에 커밋 둘(보내기의 `theirs` 가 그 base 다).
    let work = tempfile::tempdir().unwrap();
    let work_root = work.path().canonicalize().unwrap();
    let local_repo = work_root.join("repo");
    let ssh_command = format!("ssh {}", site.remote.options().join(" "));
    let out = Command::new("git")
        .args(["clone", "-q"])
        .arg(format!("{}:{remote_repo}", site.remote.user_host))
        .arg(&local_repo)
        .env("GIT_SSH_COMMAND", &ssh_command)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "clone: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(git(&local_repo, &["rev-parse", "HEAD"]), base);
    git(&local_repo, &["switch", "-q", "-c", "feat/x"]);
    std::fs::write(local_repo.join("a.txt"), "a\n").unwrap();
    git(&local_repo, &["add", "-A"]);
    git(&local_repo, &["commit", "-qm", "a"]);
    std::fs::write(local_repo.join("b.txt"), "b\n").unwrap();
    git(&local_repo, &["add", "-A"]);
    git(&local_repo, &["commit", "-qm", "b"]);
    let commit = git(&local_repo, &["rev-parse", "HEAD"]);

    let local = LocalDaemon::start(&key, &work_root);
    runtime().block_on(async {
        let here = RemoteHost::new(
            "local",
            local.transport(),
            identity(Arc::clone(&key)),
            Expect::FirstContact,
            LinkLimits::default(),
        );
        here.attach().await.unwrap();
        let there = RemoteHost::new(
            "live",
            site.transport(),
            identity(Arc::clone(&key)),
            Expect::FirstContact,
            LinkLimits::default(),
        );
        there.attach().await.unwrap();
        let (src_id, mut src) = project_window(&here, local_repo.to_str().unwrap()).await;
        let (dst_id, mut dst) = project_window(&there, &remote_repo).await;
        let theirs = dst
            .call("git.send.theirs", json!({"projectId": dst_id, "branch": "feat/x"}))
            .await;
        assert_eq!(theirs["theirs"], json!([base]), "{theirs}");
        let items = src
            .stream(
                "git.send.prepare",
                json!({"projectId": src_id, "branch": "feat/x", "commit": commit, "theirs": theirs["theirs"]}),
            )
            .await;
        assert_eq!(items[0]["kind"], "info");
        let size = items[0]["size"].as_u64().unwrap();
        let begin = dst
            .call(
                "git.send.receive.begin",
                json!({"projectId": dst_id, "sourceMachine": "laptop", "branch": "feat/x", "commit": commit, "size": size}),
            )
            .await;
        let upload = begin["uploadId"].clone();
        for item in &items[1..] {
            dst.call(
                "git.send.receive.chunk",
                json!({"uploadId": upload, "offset": item["offset"], "data": item["data"]}),
            )
            .await;
        }
        let finished = dst
            .call("git.send.receive.finish", json!({"uploadId": upload}))
            .await;
        assert_eq!(finished["commit"], commit);
        let applied = dst
            .call(
                "git.send.apply",
                json!({"projectId": dst_id, "sourceMachine": "laptop", "branch": "feat/x", "commit": commit, "expected": null}),
            )
            .await;
        assert_eq!(applied["outcome"]["kind"], "applied", "{applied}");
        drop((src, dst));
        here.detach();
        there.detach();
    });
    assert_eq!(
        site.remote
            .sh(&format!("git -C '{remote_repo}' rev-parse feat/x"))
            .trim(),
        commit
    );
    assert_eq!(
        site.remote
            .sh(&format!("git -C '{remote_repo}' remote get-url origin"))
            .trim(),
        "/nonexistent/origin.git"
    );
}
