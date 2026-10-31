//! M3-11 의 완료 기준 중 관문 쪽(14-m3-plan.md M3-11 행): 터널을 여는 연결이 관문 · 신뢰 게이트 · 확인을 지나야만 ssh 를 띄우고(⑦,
//! 터널 목적지 · 인증서 파일의 확인), 세션이 터널 리스를 쥐고 놓으며, 터널이 끊기면 그 세션이 「끊김」 이벤트를 받는다(⑥).
//! 터널 자체(argv · 포트 · 준비 · 함께 쓰기 · 그룹 끝내기)는 `pw_db_driver::tunnel` 의 시험이다.
//!
//! - 가짜 ssh(unix): 런타임 환경의 PATH 에 둔 `ssh` 스크립트가 받은 인자 · 환경을 적고 실패로 끝난다 — 「ssh 가 떴는가」 를 센다.
//! - 가짜 터널([`FakeTunnels`]): 리스를 버렸는지 · 끊김을 흉내 낸다(모든 OS).
//! - 실 sshd(`live_ssh_tunnel_*`, `cfg(pw_live_ssh)`): ⑤ 모르는 호스트 키의 askpass 가 그 프로젝트 창으로 가고 [아니오] 면 실패,
//!   ⑥ `kill -9` 가 그 세션을 「끊김」 으로.

mod common;

#[cfg(unix)]
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Conn, Fixture};
use pw_core::bus::{Filter, Received, Subscription};
use pw_db::DbSessions;
use pw_db::session::{DbTunnelLease, DbTunnels};
use pw_db::trust::code as trust_code;
#[cfg(unix)]
use pw_db::tunnel::{SshTunnels, TunnelEnv};
use pw_db_driver::fake::{FakeCall, FakeDriver};
use pw_db_driver::{BoxFuture, Connected, DbDriver, DbResult};
use pw_db_types::{Capabilities, ConnectionSpec, ServerInfo, SqlDialect, Target};
use pw_rpc::RpcError;
#[cfg(unix)]
use pw_rpc::{ConnectionScope, code as rpc_code};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// 연결 설정을 기록하는 가짜 드라이버(터널이 `connect_to` 를 채웠는지 본다).
#[derive(Clone)]
struct SpyDriver {
    inner: FakeDriver,
    specs: Arc<Mutex<Vec<ConnectionSpec>>>,
}

impl DbDriver for SpyDriver {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn dialect(&self) -> Option<SqlDialect> {
        self.inner.dialect()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        self.specs.lock().unwrap().push(spec.clone());
        self.inner.connect(spec)
    }

    fn test(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<ServerInfo>> {
        self.specs.lock().unwrap().push(spec.clone());
        self.inner.test(spec)
    }
}

/// 런타임 환경(고정).
#[cfg(unix)]
struct FixedEnv(Vec<(OsString, OsString)>);

#[cfg(unix)]
impl TunnelEnv for FixedEnv {
    fn env(&self, _project_id: &str, _root: &Path) -> Vec<(OsString, OsString)> {
        self.0.clone()
    }
}

/// 리스를 놓을 때 부르는 것(시험이 그때의 상태를 적는다 — 예: 드라이버가 닫은 연결 수).
type Probe = Arc<dyn Fn() -> usize + Send + Sync>;

/// 가짜 터널: 리스를 버렸는지(`dropped`) · 살아 있는 리스 수(`live`)와 끊김(`lost`)을 시험이 본다.
#[derive(Default)]
struct FakeTunnels {
    opened: AtomicUsize,
    dropped: Arc<AtomicBool>,
    live: Arc<AtomicUsize>,
    lost: Mutex<Option<CancellationToken>>,
    /// 놓을 때마다 `probe` 의 값을 `released` 에 적는다.
    probe: Mutex<Option<Probe>>,
    released: Arc<Mutex<Vec<usize>>>,
}

struct DropFlag {
    dropped: Arc<AtomicBool>,
    live: Arc<AtomicUsize>,
    probe: Option<Probe>,
    released: Arc<Mutex<Vec<usize>>>,
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        if let Some(probe) = &self.probe {
            self.released.lock().unwrap().push(probe());
        }
        self.live.fetch_sub(1, Ordering::SeqCst);
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl DbTunnels for FakeTunnels {
    fn open<'a>(
        &'a self,
        _project_id: &'a str,
        _root: &'a Path,
        _host_ref: &'a str,
        spec: &'a mut ConnectionSpec,
    ) -> BoxFuture<'a, Result<DbTunnelLease, RpcError>> {
        Box::pin(async move {
            self.opened.fetch_add(1, Ordering::SeqCst);
            if let Target::Network { connect_to, .. } = &mut spec.target {
                *connect_to = Some(pw_db_types::SocketTarget {
                    host: "127.0.0.1".into(),
                    port: 40999,
                });
            }
            let lost = CancellationToken::new();
            *self.lost.lock().unwrap() = Some(lost.clone());
            self.dropped.store(false, Ordering::SeqCst);
            self.live.fetch_add(1, Ordering::SeqCst);
            Ok(DbTunnelLease::new(
                DropFlag {
                    dropped: Arc::clone(&self.dropped),
                    live: Arc::clone(&self.live),
                    probe: self.probe.lock().unwrap().clone(),
                    released: Arc::clone(&self.released),
                },
                lost,
            ))
        })
    }
}

struct Env {
    f: Fixture,
    sessions: Arc<DbSessions>,
    specs: Arc<Mutex<Vec<ConnectionSpec>>>,
    /// 드라이버(같은 상태를 나눈 복제 — 호출을 센다).
    pg: FakeDriver,
}

fn env(trusted: bool, tunnels: Arc<dyn DbTunnels>) -> Env {
    let specs = Arc::new(Mutex::new(Vec::new()));
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let driver = SpyDriver {
        inner: pg.clone(),
        specs: Arc::clone(&specs),
    };
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(driver)).unwrap();
    let mut slot = None;
    let f = Fixture::build(trusted, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        sessions.attach_tunnels(tunnels);
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    Env {
        f,
        sessions: slot.unwrap(),
        specs,
        pg,
    }
}

/// 터널을 쓰는 연결 설정(DB 는 터널 너머의 `127.0.0.1:<db_port>`).
fn tunnelled(host_ref: &str, db_port: u16) -> String {
    format!(
        "schema = 1\nname = \"via bastion\"\ndriver = \"postgres\"\nhost = \"127.0.0.1\"\nport = {db_port}\nsslmode = \"disable\"\n\n[ssh_tunnel]\nhost_ref = \"{host_ref}\"\n"
    )
}

fn open_params(f: &Fixture, connection: &str) -> Value {
    json!({ "projectId": f.id, "connectionId": connection })
}

/// 확인 요청을 받아 그대로 확인한다(창이 사람에게 보여 주고 [확인] 을 누른 흐름). 요청이 아니면 패닉.
async fn approve(w: &mut Conn, f: &Fixture, connection: &str, e: &RpcError) {
    assert_eq!(e.code, trust_code::APPROVAL_REQUIRED, "{e:?}");
    let request = e.data.as_ref().unwrap()["request"].clone();
    let approval = match request["kind"].as_str().unwrap() {
        "tunnel" => {
            json!({ "kind": "tunnel", "hostRef": request["hostRef"], "target": request["target"] })
        }
        "certFiles" => {
            json!({ "kind": "certFiles", "files": request["files"], "target": request["target"] })
        }
        other => panic!("unexpected approval {other}"),
    };
    w.call(
        "db.connection.approve",
        json!({ "projectId": f.id, "connectionId": connection, "approval": approval }),
    )
    .await
    .unwrap();
}

fn events(f: &Fixture) -> Subscription {
    f.bus.subscribe(Filter::all())
}

/// `db.sessionChanged` 를 기다린다(`want` 가 참인 것).
async fn session_event(events: &mut Subscription, want: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no matching db.sessionChanged"
        );
        while let Some(received) = events.try_recv() {
            if let Received::Event(event) = received
                && event.topic == "db.sessionChanged"
                && want(&event.payload)
            {
                return event.payload.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// --- 세션이 리스를 쥐고 놓는다 · 끊김(가짜 터널, 모든 OS) -----------------------------------------------------------------

#[tokio::test]
async fn a_session_holds_its_tunnel_until_it_closes_and_the_driver_goes_through_it() {
    let tunnels = Arc::new(FakeTunnels::default());
    let e = env(true, tunnels.clone());
    e.f.write_db("t.toml", &tunnelled("bastion", 5432));
    let mut w = e.f.window();
    let err = w
        .call("db.session.open", open_params(&e.f, "t"))
        .await
        .unwrap_err();
    approve(&mut w, &e.f, "t", &err).await;
    let opened = w
        .call("db.session.open", open_params(&e.f, "t"))
        .await
        .unwrap();
    let session = opened["session"]["sessionId"].as_str().unwrap().to_owned();
    assert_eq!(tunnels.opened.load(Ordering::SeqCst), 1);
    let spec = e.specs.lock().unwrap().last().cloned().unwrap();
    match spec.target {
        Target::Network {
            host,
            port,
            connect_to,
        } => {
            // TLS 이름 검증은 설정의 DB 호스트로(결정 M3-D10), 붙는 곳은 터널의 로컬 끝.
            assert_eq!((host.as_str(), port), ("127.0.0.1", 5432));
            assert_eq!(connect_to.unwrap().port, 40999);
        }
        other => panic!("{other:?}"),
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !tunnels.dropped.load(Ordering::SeqCst),
        "the session must hold its tunnel"
    );
    w.call("db.session.close", json!({ "sessionId": session }))
        .await
        .unwrap();
    assert!(
        tunnels.dropped.load(Ordering::SeqCst),
        "closing the session releases the tunnel"
    );

    // 연결 테스트는 닫은 뒤에 놓는다.
    w.call("db.connection.test", open_params(&e.f, "t"))
        .await
        .unwrap();
    assert_eq!(tunnels.opened.load(Ordering::SeqCst), 2);
    assert!(tunnels.dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_lost_tunnel_disconnects_its_session_with_an_event() {
    let tunnels = Arc::new(FakeTunnels::default());
    let e = env(true, tunnels.clone());
    e.f.write_db("t.toml", &tunnelled("bastion", 5432));
    let mut w = e.f.window();
    let err = w
        .call("db.session.open", open_params(&e.f, "t"))
        .await
        .unwrap_err();
    approve(&mut w, &e.f, "t", &err).await;
    let opened = w
        .call("db.session.open", open_params(&e.f, "t"))
        .await
        .unwrap();
    let session = opened["session"]["sessionId"].as_str().unwrap().to_owned();
    let mut events = events(&e.f);
    tunnels.lost.lock().unwrap().as_ref().unwrap().cancel();
    let event = session_event(&mut events, |p| {
        p["session"]["sessionId"] == session.as_str()
    })
    .await;
    assert_eq!(event["session"]["state"]["kind"], "disconnected", "{event}");
    assert_eq!(event["notice"]["kind"], "connectionLost", "{event}");
    // 연결과 함께 터널도 놓는다 — 다시 붙는 것은 사람이 누를 때(새 세션).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !tunnels.dropped.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the lease was not released"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        tunnels.opened.load(Ordering::SeqCst),
        1,
        "no reconnect by itself"
    );
    assert_eq!(e.sessions.session_count(), 1, "the tab keeps its session");
}

/// 조건이 참이 될 때까지 기다린다(2 s).
async fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 스키마 탐색(M3-10)의 메타데이터용 연결도 터널 리스를 쥔다: 조회 둘이 같은 터널로 가고 그 사이 리스가 0 이 되지 않으며, 연결이
/// 닫히면(취소 · 쉼) 연결을 **닫은 뒤에** 리스를 놓는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schema_metadata_connections_hold_their_tunnel_until_they_close() {
    let tunnels = Arc::new(FakeTunnels::default());
    let e = env(true, tunnels.clone());
    let pg = e.pg.clone();
    *tunnels.probe.lock().unwrap() = Some(Arc::new(move || pg.count(&FakeCall::Close)));
    e.f.write_db("t.toml", &tunnelled("bastion", 5432));
    let mut w = e.f.window();
    let objects =
        json!({ "projectId": e.f.id, "connectionId": "t", "kind": "table", "limit": 500 });
    let columns = json!({ "projectId": e.f.id, "connectionId": "t",
                          "object": { "schema": "public", "name": "t0" } });
    let err = w
        .call("db.schema.objects", objects.clone())
        .await
        .unwrap_err();
    approve(&mut w, &e.f, "t", &err).await;

    // 조회 둘(두 번째는 캐시가 아니라 드라이버로 간다)이 한 연결 · 한 터널을 쓰고, 그 사이 리스는 살아 있다.
    w.call("db.schema.objects", objects.clone()).await.unwrap();
    assert_eq!(tunnels.opened.load(Ordering::SeqCst), 1);
    assert_eq!(tunnels.live.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        tunnels.live.load(Ordering::SeqCst),
        1,
        "the metadata connection must hold its tunnel between queries"
    );
    w.call("db.schema.columns", columns.clone()).await.unwrap();
    assert_eq!(e.pg.count(&FakeCall::Metadata("columns")), 1);
    assert_eq!(e.pg.count(&FakeCall::Connect), 1);
    assert_eq!(tunnels.opened.load(Ordering::SeqCst), 1, "one tunnel");
    assert_eq!(tunnels.live.load(Ordering::SeqCst), 1);

    // 질의 도중 취소 → 연결을 닫고 그다음 리스를 놓는다.
    e.pg.hold_metadata(true);
    let mut filtered = objects.clone();
    filtered["filter"] = json!("t");
    w.session.handle_value(json!({
        "jsonrpc": "2.0", "id": 900, "method": "db.schema.objects", "params": filtered,
    }));
    wait_until("the query did not reach the driver", || {
        e.pg.count(&FakeCall::Metadata("objects")) == 2
    })
    .await;
    w.session.handle_value(json!({
        "jsonrpc": "2.0", "method": "request.cancel", "params": { "id": 900 },
    }));
    wait_until("the cancelled connection kept its tunnel", || {
        tunnels.live.load(Ordering::SeqCst) == 0
    })
    .await;
    e.pg.hold_metadata(false);
    assert_eq!(
        *tunnels.released.lock().unwrap(),
        vec![1],
        "the tunnel is released after its connection is closed"
    );

    // 다음 조회는 새 연결 · 새 리스. 쉬어서 닫히면 역시 닫은 뒤에 놓는다.
    e.sessions
        .schema_state()
        .set_idle_close(Duration::from_millis(200));
    w.call("db.schema.ddl", columns).await.unwrap();
    assert_eq!(tunnels.opened.load(Ordering::SeqCst), 2);
    assert_eq!(tunnels.live.load(Ordering::SeqCst), 1);
    wait_until("the idle connection kept its tunnel", || {
        tunnels.live.load(Ordering::SeqCst) == 0
    })
    .await;
    assert_eq!(*tunnels.released.lock().unwrap(), vec![1, 2]);
}

// --- 관문 · 확인(가짜 ssh — unix) -------------------------------------------------------------------------------------

#[cfg(unix)]
mod fake_ssh {
    use std::fs;
    use std::path::PathBuf;

    use pw_db_driver::tunnel::testing::{TempDir, shell_quote, write_script};

    use super::*;

    /// PATH 에 둔 가짜 `ssh`: 인자 · 환경을 적고 실패로 끝난다.
    struct FakeSsh {
        dir: TempDir,
    }

    impl FakeSsh {
        fn new() -> Self {
            let dir = TempDir::new("pw-db-fake-ssh", None);
            fs::create_dir_all(dir.path().join("bin")).unwrap();
            let log = dir.path().join("calls");
            let envs = dir.path().join("env");
            write_script(
                &dir.path().join("bin").join("ssh"),
                &format!(
                    "#!/bin/sh\necho \"$*\" >> {}\nenv > {}\necho 'pw-fake-ssh: Permission denied (publickey).' >&2\nexit 255\n",
                    shell_quote(&log),
                    shell_quote(&envs)
                ),
            );
            Self { dir }
        }

        fn path(&self) -> OsString {
            let mut path = OsString::from(self.dir.path().join("bin"));
            path.push(":/usr/bin:/bin");
            path
        }

        fn tunnels(&self) -> Arc<SshTunnels> {
            Arc::new(SshTunnels::new(Arc::new(FixedEnv(vec![
                ("PATH".into(), self.path()),
                ("SSH_AUTH_SOCK".into(), "/pw-test/agent.sock".into()),
            ]))))
        }

        fn calls(&self) -> Vec<String> {
            fs::read_to_string(self.dir.path().join("calls"))
                .map(|t| t.lines().map(str::to_owned).collect())
                .unwrap_or_default()
        }

        fn env(&self) -> String {
            fs::read_to_string(self.dir.path().join("env")).unwrap()
        }

        fn file(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }
    }

    fn db_failed(e: &RpcError) -> bool {
        e.code == pw_db::gate::code::DB_ERROR
    }

    /// 터널 목적지는 처음 쓸 때 묻고, 확인한 뒤에 ssh 가 뜨며(고정 argv, 그 프로젝트의 런타임 환경), 목적지가 바뀌면 다시 묻는다.
    #[tokio::test]
    async fn the_tunnel_destination_is_asked_before_the_first_ssh_and_again_when_it_changes() {
        let ssh = FakeSsh::new();
        let e = env(true, ssh.tunnels());
        e.f.write_db("t.toml", &tunnelled("bastion", 5432));
        let mut w = e.f.window();
        let err = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        let request = &err.data.as_ref().unwrap()["request"];
        assert_eq!(request["kind"], "tunnel");
        assert_eq!(request["hostRef"], "bastion");
        assert_eq!(request["previous"], Value::Null);
        assert!(ssh.calls().is_empty(), "no ssh before the approval");
        // 목록도 확인이 필요하다고 보인다.
        let list = w
            .call("db.connection.list", json!({ "projectId": e.f.id }))
            .await
            .unwrap();
        assert!(list.to_string().contains("approvalRequired"), "{list}");

        approve(&mut w, &e.f, "t", &err).await;
        let err = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        assert!(db_failed(&err), "{err:?}");
        assert!(
            err.message.contains("Permission denied (publickey)"),
            "{}",
            err.message
        );
        let calls = ssh.calls();
        assert_eq!(calls.len(), 1);
        let args: Vec<&str> = calls[0].split(' ').collect();
        assert_eq!(
            &args[..8],
            [
                "-N",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ControlPath=none",
                "-L"
            ]
        );
        assert!(
            args[8].starts_with("127.0.0.1:") && args[8].ends_with(":127.0.0.1:5432"),
            "{}",
            args[8]
        );
        assert_eq!(&args[9..], ["--", "bastion"]);
        let child_env = ssh.env();
        assert!(
            child_env.contains("SSH_AUTH_SOCK=/pw-test/agent.sock"),
            "{child_env}"
        );

        e.f.edit_db(
            "t.toml",
            "host_ref = \"bastion\"",
            "host_ref = \"bastion2\"",
        );
        let err = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        let request = &err.data.as_ref().unwrap()["request"];
        assert_eq!(request["kind"], "tunnel", "{err:?}");
        assert_eq!(request["hostRef"], "bastion2");
        assert_eq!(request["previous"]["hostRef"], "bastion");
        assert_eq!(ssh.calls().len(), 1, "no ssh to the changed destination");
        // 대상(DB 호스트)이 바뀌어도 다시 묻는다.
        approve(&mut w, &e.f, "t", &err).await;
        e.f.edit_db("t.toml", "port = 5432", "port = 6543");
        let err = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        assert_eq!(err.code, trust_code::APPROVAL_REQUIRED, "{err:?}");
        assert_eq!(ssh.calls().len(), 1);
    }

    /// 인증서 파일도 처음 쓸 때 묻고 (파일, 대상)에 묶는다.
    #[tokio::test]
    async fn certificate_files_are_asked_before_first_use_and_bound_to_the_target() {
        let ssh = FakeSsh::new();
        let e = env(true, ssh.tunnels());
        e.f.write_db(
            "c.toml",
            "schema = 1\nname = \"tls\"\ndriver = \"postgres\"\nhost = \"db.example\"\nsslmode = \"verify-full\"\nsslrootcert = \"certs/ca.pem\"\n",
        );
        let mut w = e.f.window();
        let err = w
            .call("db.session.open", open_params(&e.f, "c"))
            .await
            .unwrap_err();
        let request = &err.data.as_ref().unwrap()["request"];
        assert_eq!(request["kind"], "certFiles", "{err:?}");
        let expected = e.f.root.join("certs/ca.pem");
        assert_eq!(
            request["files"]["sslrootcert"],
            expected.to_string_lossy().as_ref()
        );
        assert!(
            e.specs.lock().unwrap().is_empty(),
            "the driver must not run"
        );
        approve(&mut w, &e.f, "c", &err).await;
        w.call("db.session.open", open_params(&e.f, "c"))
            .await
            .unwrap();
        e.f.edit_db("c.toml", "host = \"db.example\"", "host = \"evil.example\"");
        let err = w
            .call("db.session.open", open_params(&e.f, "c"))
            .await
            .unwrap_err();
        let request = &err.data.as_ref().unwrap()["request"];
        assert_eq!(request["kind"], "certFiles");
        assert_eq!(request["previous"]["target"]["host"], "db.example");
        assert_eq!(e.specs.lock().unwrap().len(), 1);
    }

    /// 「연결 추가」 의 초안(연결 id 없음)은 창에서 사람이 친 값이라 터널을 묻지 않는다. 저장한 연결을 고치는 초안은 그 기록대로 묻는다.
    #[tokio::test]
    async fn a_new_draft_tunnel_is_not_asked_but_a_saved_connections_draft_is() {
        let ssh = FakeSsh::new();
        let e = env(true, ssh.tunnels());
        let mut w = e.f.window();
        let draft = json!({
            "name": "new", "driver": "postgres", "host": "127.0.0.1", "port": 5432,
            "sslmode": "disable", "sshTunnel": { "hostRef": "bastion" },
        });
        let err = w
            .call(
                "db.connection.test",
                json!({ "projectId": e.f.id, "draft": draft }),
            )
            .await
            .unwrap_err();
        assert!(
            db_failed(&err),
            "the draft runs ssh without asking: {err:?}"
        );
        assert_eq!(ssh.calls().len(), 1);
        e.f.write_db("t.toml", &tunnelled("bastion", 5432));
        let err = w
            .call(
                "db.connection.test",
                json!({ "projectId": e.f.id, "connectionId": "t", "draft": draft }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, trust_code::APPROVAL_REQUIRED, "{err:?}");
        assert_eq!(ssh.calls().len(), 1);
    }

    /// ⑦ 미신뢰 프로젝트 · 다른 창의 터널 요청에서 ssh 가 뜨지 않는다.
    #[tokio::test]
    async fn untrusted_projects_and_other_windows_start_no_ssh() {
        let ssh = FakeSsh::new();
        let e = env(false, ssh.tunnels());
        e.f.write_db("t.toml", &tunnelled("bastion", 5432));
        let draft = json!({
            "name": "new", "driver": "postgres", "host": "127.0.0.1",
            "sslmode": "disable", "sshTunnel": { "hostRef": "bastion" },
        });
        let mut w = e.f.window();
        for (method, params) in [
            ("db.session.open", open_params(&e.f, "t")),
            ("db.connection.test", open_params(&e.f, "t")),
            (
                "db.connection.test",
                json!({ "projectId": e.f.id, "draft": draft }),
            ),
        ] {
            let err = w.call(method, params).await.unwrap_err();
            assert_eq!(err.code, rpc_code::PERMISSION_DENIED, "{method}: {err:?}");
            assert_eq!(err.data.as_ref().unwrap()["reason"], "untrusted");
        }
        e.f.projects.set_trust(&e.f.id, true).unwrap();
        for scope in [
            ConnectionScope::Project(e.f.other.clone()),
            ConnectionScope::Host,
        ] {
            let mut other = e.f.connect(scope.clone());
            for (method, params) in [
                ("db.session.open", open_params(&e.f, "t")),
                (
                    "db.connection.test",
                    json!({ "projectId": e.f.id, "draft": draft }),
                ),
            ] {
                let err = other.call(method, params).await.unwrap_err();
                assert_eq!(
                    err.code,
                    rpc_code::PERMISSION_DENIED,
                    "{scope:?} {method}: {err:?}"
                );
            }
        }
        let file = ssh.file("f.txt");
        fs::write(&file, "").unwrap();
        let mut in_file =
            e.f.connect(pw_rpc::Caller::file(pw_rpc::Role::Owner, file.clone()).scope);
        let err = in_file
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        assert_eq!(err.code, rpc_code::PERMISSION_DENIED);
        assert!(ssh.calls().is_empty(), "ssh ran: {:?}", ssh.calls());
    }

    /// ssh 가 프로젝트의 PATH 에 없으면 그렇게 알린다.
    #[tokio::test]
    async fn no_ssh_on_the_projects_path_is_reported() {
        let e = env(
            true,
            Arc::new(SshTunnels::new(Arc::new(FixedEnv(vec![(
                "PATH".into(),
                "/nonexistent-pw".into(),
            )])))),
        );
        e.f.write_db("t.toml", &tunnelled("bastion", 5432));
        let mut w = e.f.window();
        let err = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        approve(&mut w, &e.f, "t", &err).await;
        let err = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not on the project's PATH"), "{err:?}");
    }
}

// --- 실 sshd --------------------------------------------------------------------------------------------------------

/// askpass 도우미 프로세스의 자리: ⑤ 의 시험이 이 시험 실행 파일을 `pitwall askpass` 대신 띄운다(`PW_TEST_ASKPASS_OUT` 이 있을
/// 때만 일한다). 진짜 도우미(`pw_git::write::askpass::client_main`)와 같은 `request` 로 앱의 소켓에 묻고 답을 그 파일에 쓴다.
#[cfg(unix)]
#[test]
#[ignore = "helper process for the tunnel askpass test; it does nothing on its own"]
fn askpass_helper_process() {
    use pw_git::write::AskpassHint;
    use pw_git::write::askpass::{SOCKET_ENV, TOKEN_ENV, request};

    let Some(out) = std::env::var_os("PW_TEST_ASKPASS_OUT") else {
        return;
    };
    let prompt = std::env::var("PW_TEST_ASKPASS_PROMPT").unwrap_or_default();
    let (Some(socket), Ok(token)) = (std::env::var_os(SOCKET_ENV), std::env::var(TOKEN_ENV)) else {
        return;
    };
    let hint = match std::env::var("SSH_ASKPASS_PROMPT").as_deref() {
        Ok("confirm") => AskpassHint::Confirm,
        Ok("none") => AskpassHint::Notice,
        _ => AskpassHint::Secret,
    };
    if let Ok(value) = request(Path::new(&socket), &token, &prompt, hint) {
        std::fs::write(out, value).unwrap();
    }
}

#[cfg(pw_live_ssh)]
mod live {
    use std::time::Duration;

    use pw_db::tunnel::TunnelAskpass;
    use pw_db_driver::tunnel::testing::{self, SshFixture, shell_quote, write_script};
    use pw_db_driver::tunnel::{AskpassGuard, Tunnels, alive};
    use pw_git::write::{Askpass, GitPromptAnswer, GitPromptEvent, GitPromptRequest, Prompts};

    use super::*;

    /// 데스크톱(`apps/desktop/src-tauri/src/db.rs`)과 같은 붙임: `pw_git` 의 askpass 세션을 ssh 에 준다.
    struct GitAskpass {
        askpass: Askpass,
        prompts: Prompts,
    }

    impl TunnelAskpass for GitAskpass {
        fn session(&self, project_id: &str) -> Option<AskpassGuard> {
            let session = self.askpass.session(
                project_id,
                &self.prompts,
                pw_git::write::prompt::ASKPASS_TIMEOUT,
            )?;
            let mut env = Vec::new();
            session.apply(&mut env);
            let session = Arc::new(session);
            let asking = Arc::clone(&session);
            Some(AskpassGuard::new(env, move || asking.asking(), session))
        }
    }

    async fn ready(f: &SshFixture) {
        let (host, port) = (f.host.clone(), f.port);
        tokio::task::spawn_blocking(move || {
            testing::wait_sshd(&host, port, Duration::from_secs(120))
        })
        .await
        .unwrap()
        .unwrap();
    }

    fn ssh_tunnels(f: &SshFixture) -> Arc<SshTunnels> {
        Arc::new(SshTunnels::with_tunnels(
            Arc::new(FixedEnv(f.env())),
            Tunnels::new(),
        ))
    }

    /// ⑥ ssh 를 `kill -9` 하면 그 터널을 쓰는 세션이 「끊김」 이벤트를 받는다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_ssh_tunnel_kill_9_disconnects_the_session() {
        let sshd = SshFixture::from_env();
        ready(&sshd).await;
        let tunnels = ssh_tunnels(&sshd);
        let e = env(true, tunnels.clone());
        e.f.write_db("t.toml", &tunnelled(testing::HOST, testing::ECHO_PORT));
        let mut w = e.f.window();
        let err = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        approve(&mut w, &e.f, "t", &err).await;
        let first = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap();
        let second = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap();
        let pids = tunnels.tunnels().pids();
        assert_eq!(pids.len(), 1, "two sessions share one tunnel: {pids:?}");
        let mut events = events(&e.f);
        kill9(pids[0]);
        for opened in [&first, &second] {
            let id = opened["session"]["sessionId"].as_str().unwrap().to_owned();
            let event =
                session_event(&mut events, |p| p["session"]["sessionId"] == id.as_str()).await;
            assert_eq!(event["session"]["state"]["kind"], "disconnected", "{event}");
            assert_eq!(event["notice"]["kind"], "connectionLost", "{event}");
        }
        assert!(
            !alive(pids[0]) || {
                tokio::time::sleep(Duration::from_millis(500)).await;
                !alive(pids[0])
            }
        );
        assert!(tunnels.tunnels().pids().is_empty());
    }

    /// 이 시험이 띄운 ssh 하나에만 보낸다.
    fn kill9(pid: u32) {
        let status = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// ⑤ 모르는 호스트 키에서 askpass 질문이 그 프로젝트 창으로 가고(다른 프로젝트 창은 받지 않는다) [아니오] 면 실패한다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_ssh_tunnel_unknown_host_key_asks_that_projects_window_and_no_fails() {
        let sshd = SshFixture::from_env();
        ready(&sshd).await;
        // 앱의 askpass 소켓. 도우미 실행 파일 자리에는 이 시험 실행 파일을 askpass_helper_process 로 부르는 스크립트를 둔다.
        let askpass = Askpass::default();
        let app = sshd.dir.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        let exe = app.join("fake-pitwall");
        let me = std::env::current_exe().unwrap();
        write_script(
            &exe,
            &format!(
                "#!/bin/sh\nout={}/answer.$$\nPW_TEST_ASKPASS_OUT=\"$out\" PW_TEST_ASKPASS_PROMPT=\"$2\" {} --exact askpass_helper_process --ignored --nocapture >/dev/null 2>&1\n[ -f \"$out\" ] || exit 1\ncat \"$out\"\necho\nrm -f \"$out\"\n",
                shell_quote(&app),
                shell_quote(&me)
            ),
        );
        let socket =
            std::path::PathBuf::from(format!("/tmp/pw-ap-{}", std::process::id())).join("x.sock");
        let server = tokio::spawn(askpass.clone().serve(socket.clone(), exe));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while askpass.socket().is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "askpass socket did not open"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let prompts = Prompts::default();
        let tunnels = ssh_tunnels(&sshd);
        tunnels.attach_askpass(Arc::new(GitAskpass {
            askpass: askpass.clone(),
            prompts: prompts.clone(),
        }));
        let e = env(true, tunnels.clone());
        let mut window = prompts.listen(&e.f.id);
        let mut other_window = prompts.listen(&e.f.other);
        e.f.write_db(
            "t.toml",
            &tunnelled(testing::HOST_UNKNOWN, testing::ECHO_PORT),
        );
        let mut w = e.f.window();
        let err = w
            .call("db.session.open", open_params(&e.f, "t"))
            .await
            .unwrap_err();
        approve(&mut w, &e.f, "t", &err).await;
        let params = open_params(&e.f, "t");
        let opening = tokio::spawn(async move { w.call("db.session.open", params).await });
        let event = tokio::time::timeout(Duration::from_secs(30), window.recv())
            .await
            .expect("the project window gets the question")
            .unwrap();
        let GitPromptEvent::Open { prompt } = event else {
            panic!("{event:?}")
        };
        let GitPromptRequest::Askpass { text, .. } = &prompt.request else {
            panic!("{:?}", prompt.request)
        };
        assert!(
            text.contains("authenticity") || text.contains("fingerprint"),
            "a host key question: {text}"
        );
        assert!(
            other_window.try_recv().is_none(),
            "another project's window got the question"
        );
        prompts
            .answer(&e.f.id, prompt.prompt_id, GitPromptAnswer::Cancel)
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(30), opening)
            .await
            .unwrap()
            .unwrap();
        let err = result.unwrap_err();
        assert_eq!(err.code, pw_db::gate::code::DB_ERROR, "{err:?}");
        assert!(
            err.message.contains("Host key verification failed"),
            "{}",
            err.message
        );
        assert!(tunnels.tunnels().pids().is_empty());
        assert!(
            e.specs.lock().unwrap().is_empty(),
            "the driver must not connect"
        );
        server.abort();
        let _ = std::fs::remove_dir_all(socket.parent().unwrap());
    }
}
