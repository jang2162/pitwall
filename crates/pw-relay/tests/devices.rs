//! 기기 관리(`relay.*`)와 즉시 해제 · 코드 페어링(18-m7-plan M7-23 ② ③ ④ — 공격자 입장). Relay 자리는 Relay 조립
//! (`pw_relay::host::assemble`)에 데몬과 같은 몸통(`RelayDevices` · `FileCodeBook`)을 붙여 메모리 전송으로 내놓는다.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{BoxFuture, Connection, Host};
use pw_link::{
    DesktopIdentity, DeviceKey, DialError, Expect, LinkLimits, LinkState, LinkStream, Pairing,
    PairingTable, RejectReason, RelayIdentity, RelayParts, RemoteHost, SecuredTransport, Transport,
    Versions,
};
use pw_relay::codes::{FileCodeBook, system_clock};
use pw_relay::config::RelayConfig;
use pw_relay::devices::RelayDevices;
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);

async fn call(conn: &mut Connection, id: i64, method: &str, params: Value) -> Result<Value, Value> {
    conn.sink
        .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
        let frame = tokio::time::timeout(WAIT, conn.source.recv())
            .await
            .unwrap_or_else(|_| panic!("{method}: no reply"))
            .expect("closed");
        if frame["id"] == json!(id) {
            return match frame.get("error") {
                Some(error) => Err(error.clone()),
                None => Ok(frame["result"].clone()),
            };
        }
    }
}

/// 창이 아닌 기기 관리 연결(Desktop 의 중계가 여는 것과 같은 꼴 — Host 범위 owner).
fn admin() -> Caller {
    Caller::new(Role::Owner).in_window("devices")
}

/// 가운데에서 끊을 수 있는 전송(멈춘 링크 — 이어 붙기 토큰을 쥔 채 — 를 만든다).
struct Cuttable {
    inner: Arc<pw_link::transport::MemoryTransport>,
    down: Arc<AtomicBool>,
    pipes: Mutex<Vec<tokio::task::AbortHandle>>,
}

impl Cuttable {
    fn cut(&self) {
        self.down.store(true, Ordering::SeqCst);
        for pipe in self.pipes.lock().unwrap().drain(..) {
            pipe.abort();
        }
    }

    fn restore(&self) {
        self.down.store(false, Ordering::SeqCst);
    }
}

impl Transport for Cuttable {
    fn describe(&self) -> String {
        "cuttable".into()
    }

    fn open(&self) -> BoxFuture<'_, std::io::Result<LinkStream>> {
        Box::pin(async move {
            if self.down.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("down"));
            }
            let far = self.inner.open().await?;
            let (near, mid) = tokio::io::duplex(256 * 1024);
            let (mut mid_r, mut mid_w) = tokio::io::split(mid);
            let (mut far_r, mut far_w) = (far.read, far.write);
            let up = tokio::spawn(async move {
                let _ = tokio::io::copy(&mut mid_r, &mut far_w).await;
            });
            let downward = tokio::spawn(async move {
                let _ = tokio::io::copy(&mut far_r, &mut mid_w).await;
            });
            self.pipes
                .lock()
                .unwrap()
                .extend([up.abort_handle(), downward.abort_handle()]);
            Ok(LinkStream::split(near))
        })
    }
}

/// 연결이 닫힐 때까지 읽는다(시한 안에 닫히지 않으면 실패).
async fn ended(conn: &mut Connection) {
    loop {
        match tokio::time::timeout(WAIT, conn.source.recv())
            .await
            .expect("still open")
        {
            None => return,
            Some(_) => continue,
        }
    }
}

struct Relay {
    data: PathBuf,
    pairings: PairingTable,
    parts: RelayParts,
    transport: Arc<pw_link::transport::MemoryTransport>,
    noise: Arc<pw_link::transport::MemoryTransport>,
    project: String,
    relay_key: Arc<DeviceKey>,
}

fn versions() -> Versions {
    pw_relay::versions()
}

/// Relay 하나: 페어링 표에 `paired` 를 넣고, 평문(데몬 소켓 자리)과 Noise(직접 연결 자리) 두 받기를 연다.
fn relay(base: &Path, paired: &[(&Arc<DeviceKey>, Role, &str)]) -> Relay {
    let paths = pw_relay::paths::relay_paths(&AppPaths::under(&base.join("app")));
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let assembled = pw_relay::host::assemble(
        paths.clone(),
        &RelayConfig {
            roots: vec![base.to_owned()],
            ..RelayConfig::default()
        },
        tokio::runtime::Handle::current(),
    );
    let registry = assembled.host.registry().unwrap().clone();
    let project = registry
        .create(NewProject {
            root: repo,
            ..Default::default()
        })
        .unwrap()
        .id;
    // 신뢰한 프로젝트의 터미널은 Host 수명이다(M7-09 ②) — 해제가 붙기를 끊는지 본다.
    registry.set_trust(&project, true).unwrap();
    let devices: Vec<Pairing> = paired
        .iter()
        .map(|(key, role, name)| Pairing {
            key: key.public(),
            role: *role,
            name: (*name).to_owned(),
        })
        .collect();
    let data = paths.data.clone();
    // 데몬은 기기 키를 만들 때 이 폴더를 0700 으로 둔다(`RelayKeyFile`) — 조립이 먼저 만든 폴더를 그와 같게.
    std::fs::set_permissions(&data, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    pw_link::pairing::save(&pw_relay::pairings::file(&data), &devices).unwrap();
    let pairings = PairingTable::new(devices);
    assembled
        .devices
        .as_ref()
        .expect("the relay assembly manages devices")
        .attach(Arc::new(RelayDevices::new(
            data.clone(),
            pairings.clone(),
            system_clock(),
        )));
    let relay_key = Arc::new(DeviceKey::generate().unwrap());
    let parts = RelayParts::new(
        Arc::new(RelayIdentity {
            key: Arc::clone(&relay_key),
            versions: versions(),
        }),
        pairings.clone(),
        Arc::new(assembled.host) as Arc<dyn Host>,
        LinkLimits::default(),
    );
    let (transport, acceptor) = pw_link::transport::memory(256 * 1024);
    {
        let parts = parts.clone();
        tokio::spawn(async move {
            while let Some(stream) = acceptor.accept().await {
                let parts = parts.clone();
                tokio::spawn(async move {
                    let _ = pw_link::serve(stream, &parts).await;
                });
            }
        });
    }
    let (noise, noise_acceptor) = pw_link::transport::memory(256 * 1024);
    {
        let parts = parts.clone();
        let book = Arc::new(FileCodeBook::new(
            data.clone(),
            Some(pairings.clone()),
            system_clock(),
        ));
        tokio::spawn(async move {
            while let Some(stream) = noise_acceptor.accept().await {
                let (parts, book) = (parts.clone(), Arc::clone(&book));
                tokio::spawn(async move {
                    let _ = pw_link::serve_noise(stream, &parts, &*book, |_| {}).await;
                });
            }
        });
    }
    Relay {
        data,
        pairings,
        parts,
        transport: Arc::new(transport),
        noise: Arc::new(noise),
        project,
        relay_key,
    }
}

struct Forward(Arc<pw_link::transport::MemoryTransport>);

impl Transport for Forward {
    fn describe(&self) -> String {
        "memory".into()
    }

    fn open(&self) -> BoxFuture<'_, std::io::Result<LinkStream>> {
        self.0.open()
    }
}

fn desktop(relay: &Relay, key: &Arc<DeviceKey>, transport: Arc<dyn Transport>) -> RemoteHost {
    RemoteHost::new(
        "mini",
        transport,
        Arc::new(DesktopIdentity {
            key: Arc::clone(key),
            versions: versions(),
        }),
        Expect::Machine(relay.relay_key.id()),
        LinkLimits::default(),
    )
}

fn base() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
    (tmp, base)
}

fn key() -> Arc<DeviceKey> {
    Arc::new(DeviceKey::generate().unwrap())
}

/// 낮은 role 기기(readonly · mobile)는 기기 목록을 보거나 다른 기기를 해제하거나 코드를 띄우지 못한다(owner 를 주장해도 — 링크가
/// role 을 자른다). 프로젝트 범위 연결(창 · 에이전트 자리)의 owner 기기도 받지 않는다. owner 기기의 Host 범위 연결만 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_an_owner_device_lists_unpairs_or_issues_codes() {
    let (_tmp, base) = base();
    let (owner, phone, viewer) = (key(), key(), key());
    let relay = relay(
        &base,
        &[
            (&owner, Role::Owner, "mac"),
            (&phone, Role::Mobile, "phone"),
            (&viewer, Role::Readonly, "tv"),
        ],
    );
    for low in [&phone, &viewer] {
        let remote = desktop(&relay, low, Arc::new(Forward(Arc::clone(&relay.transport))));
        remote.attach().await.unwrap();
        let mut conn = remote.connect(admin()).await.unwrap();
        for (i, (method, params)) in [
            ("relay.devices", Value::Null),
            ("relay.unpair", json!({"device": owner.id()})),
            ("relay.pairCode", json!({"role": "owner"})),
        ]
        .into_iter()
        .enumerate()
        {
            let denied = call(&mut conn, i as i64, method, params).await.unwrap_err();
            assert_eq!(
                denied["code"],
                json!(pw_rpc::code::PERMISSION_DENIED),
                "{method}"
            );
        }
    }
    assert_eq!(
        relay.pairings.snapshot().len(),
        3,
        "a low role device changed the table"
    );
    assert!(!relay.data.join(pw_relay::codes::CODE_FILE).exists());

    let remote = desktop(
        &relay,
        &owner,
        Arc::new(Forward(Arc::clone(&relay.transport))),
    );
    remote.attach().await.unwrap();
    let mut window = remote
        .connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(relay.project.clone()),
            window: Some("project-1".into()),
        })
        .await
        .unwrap();
    // 리뷰 F2: owner 기기라도 Host 범위가 아닌 연결(프로젝트 창 · 묶음 창 자리)은 셋 모두 거절된다 — 코드도 해제도 없다.
    for (i, (method, params)) in [
        ("relay.devices", Value::Null),
        ("relay.unpair", json!({"device": viewer.id()})),
        ("relay.pairCode", json!({"role": "owner"})),
    ]
    .into_iter()
    .enumerate()
    {
        let denied = call(&mut window, i as i64, method, params)
            .await
            .unwrap_err();
        assert_eq!(
            denied["code"],
            json!(pw_rpc::code::PERMISSION_DENIED),
            "{method}"
        );
    }
    assert_eq!(
        relay.pairings.snapshot().len(),
        3,
        "a project connection unpaired a device"
    );
    assert!(
        !relay.data.join(pw_relay::codes::CODE_FILE).exists(),
        "a project connection issued a code"
    );

    let mut conn = remote.connect(admin()).await.unwrap();
    let list = call(&mut conn, 1, "relay.devices", Value::Null)
        .await
        .unwrap();
    let devices = list["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 3);
    let me = devices.iter().find(|d| d["you"] == json!(true)).unwrap();
    assert_eq!(me["device"], json!(owner.id()));
    assert_eq!(me["role"], json!("owner"));
    let tv = devices.iter().find(|d| d["name"] == json!("tv")).unwrap();
    assert_eq!(tv["role"], json!("readonly"));
    assert_eq!(tv["you"], json!(false));
}

/// 해제는 즉시다(파일 다시 읽기를 기다리지 않는다 — 이 시험은 따라가기를 띄우지 않는다): 그 기기의 열린 링크 · 구독 · 터미널 붙기가
/// 끊기고, 이어 붙기 토큰을 쥔 멈춘 링크도 닫혀 이어 붙지 못하고, 새로 붙지도 못한다. 다른 기기의 링크는 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unpairing_cuts_links_subscriptions_terminals_and_resumes_at_once() {
    let (_tmp, base) = base();
    let (owner, laptop, tablet) = (key(), key(), key());
    let relay = relay(
        &base,
        &[
            (&owner, Role::Owner, "mac"),
            (&laptop, Role::Owner, "laptop"),
            (&tablet, Role::Owner, "tablet"),
        ],
    );
    let admin_remote = desktop(
        &relay,
        &owner,
        Arc::new(Forward(Arc::clone(&relay.transport))),
    );
    admin_remote.attach().await.unwrap();
    let mut admin_conn = admin_remote.connect(admin()).await.unwrap();

    // laptop: 구독 하나 · 터미널 하나에 붙어 있다.
    let laptop_remote = desktop(
        &relay,
        &laptop,
        Arc::new(Forward(Arc::clone(&relay.transport))),
    );
    laptop_remote.attach().await.unwrap();
    let laptop_state = laptop_remote.state();
    let project_window = Caller {
        role: Role::Owner,
        scope: ConnectionScope::Project(relay.project.clone()),
        window: Some("project-1".into()),
    };
    let mut subscribed = laptop_remote.connect(project_window.clone()).await.unwrap();
    call(
        &mut subscribed,
        1,
        "events.subscribe",
        json!({"topics": ["task.progress"]}),
    )
    .await
    .unwrap();
    let mut terminal = laptop_remote.connect(project_window).await.unwrap();
    let opened = call(
        &mut terminal,
        1,
        "terminal.create",
        json!({"projectId": relay.project, "cols": 80, "rows": 24}),
    )
    .await
    .unwrap();
    terminal
        .sink
        .send(json!({"jsonrpc": "2.0", "method": "stream.credit",
        "params": {"streamId": opened["streamId"], "credit": 1000}}));

    // tablet: 전송을 잃고 멈췄다 — Relay 는 이어 붙기 토큰과 함께 그 링크를 쥐고 있다.
    let cuttable = Arc::new(Cuttable {
        inner: Arc::clone(&relay.transport),
        down: Arc::default(),
        pipes: Mutex::default(),
    });
    let tablet_remote = desktop(&relay, &tablet, Arc::clone(&cuttable) as Arc<dyn Transport>);
    tablet_remote.attach().await.unwrap();
    let tablet_state = tablet_remote.state();
    let held_before = relay.parts.resumes.len();
    cuttable.cut();
    let mut watch = tablet_state.clone();
    tokio::time::timeout(WAIT, watch.wait_for(|s| *s == LinkState::Reconnecting))
        .await
        .expect("the tablet link did not suspend")
        .unwrap();
    assert_eq!(
        relay.parts.resumes.len(),
        held_before,
        "the suspended link was not held"
    );

    let started = std::time::Instant::now();
    for (i, device) in [&laptop, &tablet].iter().enumerate() {
        let removed = call(
            &mut admin_conn,
            10 + i as i64,
            "relay.unpair",
            json!({"device": device.id()}),
        )
        .await
        .unwrap();
        assert_eq!(removed["removed"], json!(true));
    }
    // 열린 링크 · 구독 · 터미널 붙기가 끊긴다.
    let mut watch = laptop_state.clone();
    tokio::time::timeout(
        WAIT,
        watch.wait_for(|s| matches!(s, LinkState::Disconnected { .. })),
    )
    .await
    .expect("the unpaired laptop kept its link")
    .unwrap();
    assert!(
        started.elapsed() < pw_relay::pairings::POLL_EVERY,
        "unpairing waited for the file to be read again ({:?})",
        started.elapsed()
    );
    ended(&mut subscribed).await;
    ended(&mut terminal).await;
    // 멈춘 링크(토큰을 쥔)도 닫혔다 — 전송이 돌아와도 이어 붙지 못하고 새로 붙지도 못한다.
    tokio::time::timeout(WAIT, async {
        while relay.parts.resumes.len() > 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the unpaired device's suspended link still holds its resume token");
    cuttable.restore();
    let mut watch = tablet_state.clone();
    tokio::time::timeout(
        WAIT,
        watch.wait_for(|s| matches!(s, LinkState::Disconnected { .. })),
    )
    .await
    .expect("the unpaired tablet resumed")
    .unwrap();
    for device in [&laptop, &tablet] {
        let again = desktop(
            &relay,
            device,
            Arc::new(Forward(Arc::clone(&relay.transport))),
        );
        assert_eq!(
            again.attach().await.err(),
            Some(DialError::Rejected(RejectReason::Unpaired))
        );
    }
    // 해제한 owner 의 링크는 그대로다.
    let list = call(&mut admin_conn, 20, "relay.devices", Value::Null)
        .await
        .unwrap();
    assert_eq!(list["devices"].as_array().unwrap().len(), 1);
    assert_eq!(
        pw_link::pairing::load(&pw_relay::pairings::file(&relay.data))
            .unwrap()
            .len(),
        1
    );
}

/// owner 기기가 띄운 코드로 새 기기가 Noise 위에서 페어링한다 — 코드에 고른 role 로, 한 번만. 페어링 시각이 기기 목록에 보이고,
/// 그 기기는 곧바로(다시 읽기를 기다리지 않고) Noise 링크를 연다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owner_issued_code_pairs_a_new_device_over_noise_once() {
    let (_tmp, base) = base();
    let owner = key();
    let relay = relay(&base, &[(&owner, Role::Owner, "mac")]);
    let remote = desktop(
        &relay,
        &owner,
        Arc::new(Forward(Arc::clone(&relay.transport))),
    );
    remote.attach().await.unwrap();
    let mut conn = remote.connect(admin()).await.unwrap();
    let issued = call(
        &mut conn,
        1,
        "relay.pairCode",
        json!({"role": "readonly", "name": "tv"}),
    )
    .await
    .unwrap();
    let code = issued["code"].as_str().unwrap().to_owned();
    assert_eq!(code.len(), 6);

    let tv = key();
    let me = DesktopIdentity {
        key: Arc::clone(&tv),
        versions: versions(),
    };
    let secured = pw_link::noise::initiate(relay.noise.open().await.unwrap(), &tv)
        .await
        .unwrap();
    let paired = pw_link::pair_with_code(secured, &me, &code, "ignored", WAIT)
        .await
        .unwrap();
    assert_eq!(paired.machine, relay.relay_key.id());
    assert_eq!(paired.role, Role::Readonly);

    let tv_remote = RemoteHost::new(
        "mini",
        Arc::new(SecuredTransport::new(
            Forward(Arc::clone(&relay.noise)),
            Arc::clone(&tv),
        )),
        Arc::new(DesktopIdentity {
            key: Arc::clone(&tv),
            versions: versions(),
        }),
        Expect::Machine(paired.machine.clone()),
        LinkLimits::default(),
    );
    tv_remote.attach().await.unwrap();
    assert_eq!(tv_remote.link().unwrap().role(), Role::Readonly);

    let list = call(&mut conn, 2, "relay.devices", Value::Null)
        .await
        .unwrap();
    let entry = list["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["device"] == json!(tv.id()))
        .unwrap()
        .clone();
    assert_eq!(entry["name"], json!("tv"));
    assert!(entry["pairedAtMs"].as_u64().is_some(), "{entry}");

    // 한 번 쓴 코드.
    let other = key();
    let secured = pw_link::noise::initiate(relay.noise.open().await.unwrap(), &other)
        .await
        .unwrap();
    let again = pw_link::pair_with_code(
        secured,
        &DesktopIdentity {
            key: Arc::clone(&other),
            versions: versions(),
        },
        &code,
        "x",
        WAIT,
    )
    .await;
    assert_eq!(
        again.err(),
        Some(pw_link::PairError::Rejected(RejectReason::Code))
    );
}
