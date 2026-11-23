//! Desktop 의 다른 머신 부품(18-m7-plan M7-18, 메인 판정): 사람에게 묻는 질문이 필요한 링크는 열지 않는다 — 가짜 ssh 백엔드의
//! 붙기는 한 번도 불리지 않고(그래서 askpass 질문이 0) 「Pitwall 에서 먼저 연결하라」 로 끝난다. 조용히 붙는 링크(ssh 공유 연결이
//! 살아 있음)는 열고 세션이 사는 동안 쥐었다가 놓는다. 실제 SSH 는 쓰지 않는다.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use pw_core::config::AppPaths;
use pw_core::{EventBus, Host, LocalHost};
use pw_remote::{
    Backend, Backoff, ConnectError, HostEntry, Link, PROMPTS_KEY, Progress, RemoteHostSource,
};
use serde_json::Value;
use tokio::sync::oneshot;

use super::*;

const MINI: &str = "SHA256:mini";

/// 가짜 ssh: `check` 는 정한 답, `connect` 는 센 뒤 링크를 낸다(진짜로는 여기서 암호 · 호스트 키를 물을 수 있다).
struct FakeSsh {
    master: Option<bool>,
    connects: AtomicUsize,
    closed: Mutex<Vec<oneshot::Sender<String>>>,
    paths: AppPaths,
}

impl Backend for FakeSsh {
    fn hosts(&self, _settings: Option<&Value>) -> (Vec<HostEntry>, Vec<String>) {
        (
            vec![HostEntry {
                name: "mini".into(),
                host: "mini.local".into(),
                user: None,
                port: None,
                source: RemoteHostSource::SshConfig,
                ssh_args: Vec::new(),
                direct: None,
                ssh: true,
            }],
            Vec::new(),
        )
    }

    fn connect<'a>(
        &'a self,
        _entry: &'a HostEntry,
        _expect: Option<&'a str>,
        _progress: &'a (dyn Fn(Progress) + Send + Sync),
    ) -> BoxFuture<'a, Result<Link, ConnectError>> {
        Box::pin(async move {
            self.connects.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = oneshot::channel();
            self.closed.lock().unwrap().push(tx);
            let host: Arc<dyn Host> =
                Arc::new(LocalHost::builder(MINI).config(self.paths.clone()).build());
            Ok(Link {
                host,
                machine: MINI.into(),
                closed: Box::pin(async move { rx.await.unwrap_or_else(|_| "closed".into()) }),
                resuming: None,
                close: Box::new(|| {}),
            })
        })
    }

    fn check<'a>(&'a self, _entry: &'a HostEntry) -> BoxFuture<'a, Option<bool>> {
        let master = self.master;
        Box::pin(async move { master })
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    registry: Registry,
    remote: RemoteHosts,
    router: Arc<HostRouter>,
    ssh: Arc<FakeSsh>,
    /// 원격 askpass 질문을 듣는 창(열려 있으면 질문이 여기 보인다).
    _prompts: pw_git::write::Listener,
}

fn fixture(master: Option<bool>, remembered: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(tmp.path());
    let bus = EventBus::new(64);
    let registry = Registry::new(paths.clone(), None, bus.clone());
    if remembered {
        registry.remember_remote_machine("mini", MINI).unwrap();
    }
    let router = Arc::new(HostRouter::new());
    let remote = RemoteHosts::new(Backoff::default());
    remote.attach_host(Some(registry.clone()), bus, None);
    remote.attach_router(Arc::clone(&router));
    let ssh = Arc::new(FakeSsh {
        master,
        connects: AtomicUsize::new(0),
        closed: Mutex::default(),
        paths: AppPaths::under(&tmp.path().join("remote")),
    });
    remote.attach_backend(ssh.clone());
    let prompts = remote.prompts().listen(PROMPTS_KEY);
    Fixture {
        _tmp: tmp,
        registry,
        remote,
        router,
        ssh,
        _prompts: prompts,
    }
}

impl Fixture {
    fn machines(&self) -> DesktopMachines {
        DesktopMachines::new(
            self.registry.clone(),
            Some(self.remote.clone()),
            Arc::clone(&self.router),
        )
        .with_wait(Duration::from_secs(10))
    }

    fn connects(&self) -> usize {
        self.ssh.connects.load(Ordering::SeqCst)
    }
}

fn needs_person(r: Result<MachineLink, MachineError>) -> String {
    match r {
        Err(MachineError::NeedsPerson(m)) => m,
        Err(other) => panic!("expected NeedsPerson, got {other:?}"),
        Ok(_) => panic!("expected NeedsPerson, got a link"),
    }
}

#[tokio::test]
async fn a_link_that_may_ask_a_person_is_not_opened() {
    for master in [Some(false), None] {
        let f = fixture(master, true);
        let message = needs_person(f.machines().connect(MINI).await);
        assert!(
            message.contains("connect to `mini` in Pitwall first"),
            "{message}"
        );
        assert_eq!(f.connects(), 0, "ssh 를 띄웠다 — 질문이 나올 수 있다");
        assert_eq!(f.remote.prompts().pending(), 0, "askpass 질문이 떴다");
        assert_eq!(f.remote.status("mini"), RemoteStatus::idle());
        assert!(
            f.router
                .connect(&Target::HostId(MINI.into()), Caller::agent(Role::Owner))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn a_machine_never_connected_or_disconnected_by_a_person_is_not_opened() {
    // 처음 보는 머신(기억한 머신 id 없음) — 그 키를 사람이 믿은 적이 없다.
    let f = fixture(Some(true), false);
    needs_person(f.machines().connect(MINI).await);
    assert_eq!(f.connects(), 0);

    // 사람이 끊어 둔 호스트는 에이전트가 다시 붙이지 않는다.
    let f = fixture(Some(true), true);
    f.remote.disconnect("mini").unwrap();
    let message = needs_person(f.machines().connect(MINI).await);
    assert!(message.contains("disconnected in Pitwall"), "{message}");
    assert_eq!(f.connects(), 0);
}

#[tokio::test]
async fn a_quiet_link_is_opened_for_the_session_and_released_with_it() {
    let f = fixture(Some(true), true);
    let link = f
        .machines()
        .connect(MINI)
        .await
        .expect("a quiet link opens");
    assert_eq!(f.connects(), 1);
    assert_eq!(
        f.remote.status("mini"),
        RemoteStatus::Connected {
            machine: MINI.into()
        }
    );
    assert_eq!(f.remote.prompts().pending(), 0);
    drop(link);
    // 쓰는 것이 없으니 링크를 닫는다(D56 「쓰는 머신만」).
    assert_eq!(f.remote.status("mini"), RemoteStatus::idle());
}

#[tokio::test]
async fn a_connected_machine_is_held_without_a_new_link() {
    let f = fixture(None, true);
    // 사람이 붙인 링크(창이 쓰는 중).
    let window = f.remote.hold("mini").unwrap();
    let mut status = f.remote.watch("mini");
    tokio::time::timeout(
        Duration::from_secs(10),
        status.wait_for(|s| matches!(s, RemoteStatus::Connected { .. })),
    )
    .await
    .unwrap()
    .unwrap();
    let link = f.machines().connect(MINI).await.expect("held");
    assert_eq!(f.connects(), 1, "새 링크를 열었다");
    drop(window);
    assert!(
        matches!(f.remote.status("mini"), RemoteStatus::Connected { .. }),
        "세션이 쥐는 동안 링크가 남는다"
    );
    drop(link);
    assert_eq!(f.remote.status("mini"), RemoteStatus::idle());
}

#[test]
fn machine_names_are_this_desktops_aliases_and_bundles_take_desktop_ids_only() {
    let f = fixture(None, true);
    let machines = f.machines();
    assert_eq!(machines.machine_id("mini").as_deref(), Some(MINI));
    assert_eq!(machines.machine_id(MINI).as_deref(), Some(MINI));
    assert_eq!(machines.machine_id("box"), None);
    assert_eq!(machines.machine_name(MINI).as_deref(), Some("mini"));
    assert_eq!(machines.machine_name("SHA256:other"), None);
    // 레지스트리에 없는 id(다른 머신의 프로젝트 id 따위)는 묶음이 없다.
    assert_eq!(machines.bundle("0123456789abcdef").unwrap(), None);
    assert_eq!(machines.bundle("b-0123456789abcdef").unwrap(), None);
}
