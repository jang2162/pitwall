//! Desktop 쪽 비밀 답의 시험(18-m7-plan M7-12 Rust 시험 ② ④ ⑤ 와 ① 신뢰, 리뷰 F1 · F2). 원격 Relay 자리에는 Relay 와 같은
//! 「Desktop 에 묻는」 저장소(`pw_db::desktop_secret`)와, 처리하는 동안 Relay 가 비밀을 청하는 시험 메서드 셋(`db.session.open` —
//! 비밀이 필요한 요청, `db.connection.marks` — 창이 저절로 보내는 읽기, `db.secret.clear` — 사람이 비밀을 지우는 요청, 이름만
//! 빌린다)을 단 Host 를 둔다. 각 메서드는 `ask` 연결의 비밀을 `op`(read · remove)로 `times` 번 청하고 결과를 돌려준다.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{Connection, HostRouter, LocalHost, Target};
use pw_db::desktop_secret::{DesktopSecrets, SecretRequestCall};
use pw_db::secret::{
    DbSecretKey, MemorySecretStore, SecretAsker, SecretCall, SecretStore, account, machine_account,
    machine_index_account, with_asker,
};
use pw_rpc::{Caller, ConnectionScope, Ctx, Role, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;

pub(crate) const A: &str = "SHA256:machine-a";
const B: &str = "SHA256:machine-b";
pub(crate) const P: &str = "0123abcd";
pub(crate) const ITEM: &str = "{\"v\":1,\"value\":\"from-a\"}";
const LABEL: &str = "project-1";

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeParams {
    project_id: String,
    connection_id: String,
    /// Relay 가 Desktop 에 청할 연결.
    ask: String,
    /// `read` · `remove`.
    #[serde(default)]
    op: Option<String>,
    /// 청 횟수(기본 1).
    #[serde(default)]
    times: Option<u32>,
}

rpc_method! {
    /// 시험용(비밀이 필요한 요청 자리).
    ProbeOpen: "db.session.open" {
        params: ProbeParams,
        result: Vec<Option<String>>,
        tier: Read,
    }

    /// 시험용(창이 저절로 보내는 읽기 자리).
    ProbeMarks: "db.connection.marks" {
        params: ProbeParams,
        result: Vec<Option<String>>,
        tier: Read,
    }

    /// 시험용(사람이 비밀을 지우는 요청 자리).
    ProbeClear: "db.secret.clear" {
        params: ProbeParams,
        result: Vec<Option<String>>,
        tier: Read,
    }
}

type ProbeFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Vec<Option<String>>, RpcError>> + Send>,
>;

/// 처리하는 동안 그 창의 이 요청을 대고(`SecretAsker::window`) 비밀을 청한다.
fn probe_handler(
    secrets: DesktopSecrets,
) -> impl Fn(Ctx, ProbeParams) -> ProbeFuture + Send + Sync {
    move |_ctx, p: ProbeParams| {
        let s = secrets.clone();
        Box::pin(async move {
            let asker = SecretAsker::window();
            let account = account(&p.project_id, &p.ask, DbSecretKey::Password);
            let remove = p.op.as_deref() == Some("remove");
            let times = p.times.unwrap_or(1);
            let out = tokio::task::spawn_blocking(move || {
                with_asker(asker, || {
                    (0..times)
                        .map(|_| {
                            if remove {
                                s.delete(&account).ok().map(|()| "removed".to_owned())
                            } else {
                                s.get(&account)
                                    .ok()
                                    .flatten()
                                    .map(|v| v.expose().to_owned())
                            }
                        })
                        .collect()
                })
            })
            .await
            .unwrap();
            Ok(out)
        })
    }
}

/// 원격 Relay 자리: Relay 의 비밀 저장소와 시험 메서드 셋. Host id 는 `id`.
pub(crate) fn relay_host(id: &str, secrets: &DesktopSecrets) -> LocalHost {
    let store = Arc::new(secrets.clone());
    let s = secrets.clone();
    LocalHost::builder(id)
        .methods(move |router: &mut Router| {
            store.register(router);
            router
                .unary(ProbeOpen, probe_handler(s.clone()))
                .unary(ProbeMarks, probe_handler(s.clone()))
                .unary(ProbeClear, probe_handler(s));
        })
        .build()
}

struct Setup {
    secrets: DesktopSecrets,
    relay: Arc<dyn Host>,
    gated: Arc<dyn Host>,
    flights: Arc<Flights>,
    store: Arc<MemorySecretStore>,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

pub(crate) async fn listening(secrets: &DesktopSecrets) {
    for _ in 0..400 {
        if secrets.desktops() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(secrets.desktops(), 1, "the desktop listens");
}

/// Desktop 이 `machine` 으로 증명된 링크의 Relay 에 붙은 꼴.
async fn setup(machine: &str, store: Arc<MemorySecretStore>) -> Setup {
    let secrets = DesktopSecrets::with_wait(Duration::from_secs(3));
    let relay: Arc<dyn Host> = Arc::new(relay_host("relay", &secrets));
    let desk = Arc::new(SecretDesk::new(store.clone()));
    let (gated, flights) = SecretDesk::gate(Arc::clone(&relay));
    let serving =
        tokio::spawn(desk.serve(machine.to_owned(), Arc::clone(&relay), Arc::clone(&flights)));
    listening(&secrets).await;
    Setup {
        secrets,
        relay,
        gated,
        flights,
        store,
        serving,
    }
}

pub(crate) fn store_with(entries: &[(String, &str)]) -> Arc<MemorySecretStore> {
    let store = Arc::new(MemorySecretStore::new());
    for (account, value) in entries {
        store.put_raw(account, value);
    }
    store
}

fn a_dev() -> Arc<MemorySecretStore> {
    store_with(&[(machine_account(A, P, "dev", DbSecretKey::Password), ITEM)])
}

pub(crate) fn window() -> Caller {
    Caller {
        role: Role::Owner,
        scope: ConnectionScope::Project(P.into()),
        window: Some(LABEL.into()),
    }
}

pub(crate) async fn call(conn: &mut Connection, method: &str, params: Value) -> Value {
    conn.sink
        .send(json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }));
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(10), conn.source.recv())
        .await
        .expect("an answer")
    {
        if frame.get("id") == Some(&json!(7)) {
            return frame;
        }
    }
    panic!("closed");
}

/// 창 연결 하나로 `method` 를 부른다. `extra` 는 `op` · `times`.
pub(crate) async fn probe_with(
    host: &Arc<dyn Host>,
    caller: Caller,
    method: &str,
    connection: &str,
    ask: &str,
    extra: Value,
) -> Value {
    let mut conn = host.connect(caller).await.unwrap();
    let mut params = json!({ "projectId": P, "connectionId": connection, "ask": ask });
    if let Value::Object(extra) = extra {
        params.as_object_mut().unwrap().extend(extra);
    }
    let frame = call(&mut conn, method, params).await;
    frame["result"].clone()
}

async fn probe(host: &Arc<dyn Host>, caller: Caller, connection: &str, ask: &str) -> Value {
    probe_with(
        host,
        caller,
        "db.session.open",
        connection,
        ask,
        Value::Null,
    )
    .await
}

/// ② 리뷰 F1: Desktop 은 청이 댄 **그 요청 하나**가 지금 기다리는 중이고(①), 비밀이 필요한 메서드이고(②), 그 연결의 것이고(③),
/// 청의 종류가 그 메서드에 맞고(④), 아직 쓰지 않았을 때(한 번)만 답한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_named_request_that_needs_the_secret_gets_an_answer() {
    let s = setup(A, a_dev()).await;
    assert_eq!(probe(&s.gated, window(), "dev", "dev").await, json!([ITEM]));
    assert!(s.flights.is_empty());
    // ③ 같은 창의 요청이라도 다른 연결의 것이면 거절.
    assert_eq!(
        probe(&s.gated, window(), "other", "dev").await,
        json!([null])
    );
    // 한 요청에 한 번 — 같은 요청이 두 번 청하면 둘째는 거절.
    let twice = probe_with(
        &s.gated,
        window(),
        "db.session.open",
        "dev",
        "dev",
        json!({ "times": 2 }),
    );
    assert_eq!(twice.await, json!([ITEM, null]));
    // ① 기다리지 않는 요청을 대면 거절.
    let secrets = s.secrets.clone();
    let lone = tokio::task::spawn_blocking(move || {
        let named = SecretAsker::Window(Some(SecretCall {
            window: LABEL.into(),
            request: "7".into(),
        }));
        with_asker(named, || {
            secrets.get(&account(P, "dev", DbSecretKey::Password))
        })
    })
    .await
    .unwrap();
    assert_eq!(lone, Ok(None));
}

/// 리뷰 F1 재현: 창이 저절로 보내는 읽기(`db.connection.marks`)를 Relay 가 붙잡고 그 연결의 비밀을 읽거나 지우려 해도 거절된다(②).
/// 비밀이 필요한 읽기 요청(`db.session.open`)으로는 지우지 못하고(④), 사람이 비밀을 지우는 요청(`db.secret.clear`)만 지운다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_automatic_read_neither_reads_nor_removes_a_secret() {
    let s = setup(A, a_dev()).await;
    let stored = machine_account(A, P, "dev", DbSecretKey::Password);
    let marks = |op: &str| {
        probe_with(
            &s.gated,
            window(),
            "db.connection.marks",
            "dev",
            "dev",
            json!({ "op": op }),
        )
    };
    assert_eq!(marks("read").await, json!([null]));
    assert_eq!(marks("remove").await, json!([null]));
    let open_remove = probe_with(
        &s.gated,
        window(),
        "db.session.open",
        "dev",
        "dev",
        json!({ "op": "remove" }),
    );
    assert_eq!(open_remove.await, json!([null]));
    let clear_read = probe_with(
        &s.gated,
        window(),
        "db.secret.clear",
        "dev",
        "dev",
        json!({ "op": "read" }),
    );
    assert_eq!(clear_read.await, json!([null]));
    assert_eq!(s.store.raw(&stored).as_deref(), Some(ITEM));
    let clear = probe_with(
        &s.gated,
        window(),
        "db.secret.clear",
        "dev",
        "dev",
        json!({ "op": "remove" }),
    );
    assert_eq!(clear.await, json!(["removed"]));
    assert_eq!(s.store.raw(&stored), None);
}

/// ④ 에이전트 범위 · Host 범위 · 창 label 이 없는 연결의 요청은 세지 않는다 — 그 흐름의 청에는 답하지 않는다. 감싸지 않은 Host 로
/// 보낸 요청도 같다(지켜보는 것은 감싼 Host 다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_and_host_connections_are_not_watched() {
    let s = setup(A, a_dev()).await;
    let callers = [
        Caller {
            role: Role::Owner,
            scope: ConnectionScope::Agent,
            window: None,
        },
        Caller::new(Role::Owner),
        Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(P.into()),
            window: None,
        },
    ];
    for caller in callers {
        let scope = caller.scope.clone();
        // 에이전트 · Host 범위에도 창 label 을 붙여 Relay 가 그 요청을 대게 한다(label 만으로는 열리지 않음을 본다).
        let caller = Caller {
            window: Some(LABEL.into()),
            ..caller
        };
        let caller = if scope == ConnectionScope::Project(P.into()) {
            Caller {
                window: None,
                ..caller
            }
        } else {
            caller
        };
        assert_eq!(
            probe(&s.gated, caller, "dev", "dev").await,
            json!([null]),
            "{scope:?}"
        );
    }
    assert_eq!(probe(&s.relay, window(), "dev", "dev").await, json!([null]));
}

/// ⑤ 열쇠의 머신 id 는 그 링크가 증명한 것이다: 다른 머신(B)으로 증명된 링크는 같은 프로젝트 · 연결 id 를 대도 A 의 비밀 · 로컬
/// 비밀을 받지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_machine_link_gets_none_of_this_machines_secrets() {
    let entries = [
        (machine_account(A, P, "dev", DbSecretKey::Password), ITEM),
        (account(P, "dev", DbSecretKey::Password), "local-item"),
    ];
    let b = setup(B, store_with(&entries)).await;
    assert_eq!(probe(&b.gated, window(), "dev", "dev").await, json!([null]));
    let a = setup(A, store_with(&entries)).await;
    assert_eq!(probe(&a.gated, window(), "dev", "dev").await, json!([ITEM]));
}

fn request(id: u64, op: SecretRequestOp) -> SecretRequest {
    SecretRequest {
        call: SecretRequestCall {
            window: format!("SHA256:dev/{LABEL}"),
            request: id.to_string(),
        },
        id: 1,
        project_id: P.into(),
        connection_id: "dev".into(),
        key: DbSecretKey::Password,
        op,
    }
}

fn sent(flights: &Flights, conn: u64, id: u64, method: &str, connection: &str) {
    flights.sent(
        conn,
        LABEL,
        P,
        &json!({ "jsonrpc": "2.0", "id": id, "method": method,
                 "params": { "projectId": P, "connectionId": connection } }),
    );
}

/// 두기는 그 창의 `db.secret.set` 이 기다릴 때만 하고 계정 목록에 적는다. 답이 오거나 연결이 닫히면 기다림이 걷힌다. 표에 없는
/// 메서드 · 다른 프로젝트를 실은 요청 · 알림은 세지 않는다. 요청을 보낸 지 [`FLIGHT_LIMIT`] 가 지나면 열지 않는다. 계정 마디에 `/`
/// · 빈 값이 오면 거절한다.
#[test]
fn storing_needs_a_waiting_secret_set_and_is_indexed() {
    let store = Arc::new(MemorySecretStore::new());
    let desk = SecretDesk::new(store.clone());
    let flights = Flights::default();
    let item = Secret::new(ITEM);
    let store_op = |id| request(id, SecretRequestOp::Store { item: item.clone() });

    sent(&flights, 1, 1, "db.session.open", "dev");
    assert_eq!(
        desk.answer(A, &flights, &store_op(1)),
        SecretAnswer::Declined
    );
    sent(&flights, 1, 2, "db.secret.set", "dev");
    // 다른 요청 id · 같은 id 의 다른 창을 대면 거절, 그 요청을 대면 둔다.
    assert_eq!(
        desk.answer(A, &flights, &store_op(1)),
        SecretAnswer::Declined
    );
    let mut elsewhere = store_op(2);
    elsewhere.call.window = "SHA256:dev/project-2".into();
    assert_eq!(desk.answer(A, &flights, &elsewhere), SecretAnswer::Declined);
    assert_eq!(desk.answer(A, &flights, &store_op(2)), SecretAnswer::Done);
    let stored = machine_account(A, P, "dev", DbSecretKey::Password);
    assert_eq!(store.raw(&stored).as_deref(), Some(ITEM));
    assert_eq!(
        store.raw(&machine_index_account(A)),
        Some(json!([stored]).to_string())
    );

    flights.received(1, &json!({ "jsonrpc": "2.0", "id": 1, "result": null }));
    flights.received(1, &json!({ "jsonrpc": "2.0", "id": 2, "result": null }));
    assert!(flights.is_empty());
    sent(&flights, 1, 3, "db.session.open", "dev");
    flights.closed(1);
    assert!(flights.is_empty());

    // 표에 없는 메서드 · 다른 프로젝트를 실은 요청 · id 없는 알림은 세지 않는다.
    sent(&flights, 2, 1, "db.connection.marks", "dev");
    sent(&flights, 2, 2, "db.connection.get", "dev");
    flights.sent(
        2,
        LABEL,
        P,
        &json!({ "jsonrpc": "2.0", "id": 3, "method": "db.secret.set",
                 "params": { "projectId": "other", "connectionId": "dev" } }),
    );
    flights.sent(
        2,
        LABEL,
        P,
        &json!({ "jsonrpc": "2.0", "method": "db.secret.set",
                 "params": { "projectId": P, "connectionId": "dev" } }),
    );
    assert!(flights.is_empty());

    // 오래된 요청은 열지 않는다(Relay 가 답을 붙잡아 기다림을 늘린 꼴).
    sent(&flights, 3, 1, "db.secret.clear", "dev");
    if let Some(old) = Instant::now().checked_sub(FLIGHT_LIMIT + Duration::from_secs(1)) {
        for f in lock(&flights.entries).values_mut() {
            f.sent = old;
        }
        assert_eq!(
            desk.answer(A, &flights, &request(1, SecretRequestOp::Remove)),
            SecretAnswer::Declined
        );
    }
    flights.closed(3);
    sent(&flights, 3, 1, "db.secret.clear", "dev");
    let mut bad = request(1, SecretRequestOp::Remove);
    bad.connection_id = "dev/../x".into();
    assert_eq!(desk.answer(A, &flights, &bad), SecretAnswer::Declined);
    assert_eq!(
        desk.answer(A, &flights, &request(1, SecretRequestOp::Remove)),
        SecretAnswer::Done
    );
    assert_eq!(store.raw(&stored), None);
    assert_eq!(store.raw(&machine_index_account(A)), None);
}

/// 「이 머신을 새로 믿기」: 옛 머신 id 의 비밀과 계정 목록을 지우고, 다른 머신 · 로컬 비밀은 둔다.
#[test]
fn forgetting_a_machine_removes_only_its_secrets() {
    let store = Arc::new(MemorySecretStore::new());
    let desk = SecretDesk::new(store.clone());
    let flights = Flights::default();
    let item = Secret::new(ITEM);
    for machine in [A, B] {
        for connection in ["dev", "prod"] {
            sent(&flights, 1, 1, "db.secret.set", connection);
            let mut r = request(1, SecretRequestOp::Store { item: item.clone() });
            r.connection_id = connection.into();
            assert_eq!(desk.answer(machine, &flights, &r), SecretAnswer::Done);
            flights.closed(1);
        }
    }
    let local = account(P, "dev", DbSecretKey::Password);
    store.put_raw(&local, "local");
    desk.forget_machine(A);
    let left = store.accounts();
    assert!(left.iter().all(|a| !a.starts_with(A)), "{left:?}");
    assert!(left.contains(&local));
    assert!(left.contains(&machine_account(B, P, "prod", DbSecretKey::Password)));
    assert!(left.contains(&machine_index_account(B)));
}

/// 원격 Host 자리 — 라우터가 그 프로젝트를 `Target::Member` 로만 고르게 다른 머신의 Host 로 보인다.
struct Remote(LocalHost);

impl Host for Remote {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn connect(&self, caller: Caller) -> BoxFuture<'_, Result<Connection, HostError>> {
        self.0.connect(caller)
    }

    fn is_remote(&self) -> bool {
        true
    }
}

fn project(host: &LocalHost, root: &Path) -> String {
    host.registry()
        .unwrap()
        .create(NewProject {
            root: root.to_path_buf(),
            ..NewProject::default()
        })
        .unwrap()
        .id
}

/// ① 신뢰는 머신마다: 같은 체크아웃 경로를 두 머신이 등록해도, 원격 연결로 준 신뢰 · 확인 다이얼로그 차례는 그 머신의
/// 레지스트리에만 남는다. 로컬은 그대로 「아직 묻지 않음」 이고 로컬 다이얼로그 차례도 그대로다. 반대도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_trust_does_not_touch_local_trust() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let checkout = base.join("work/app");
    std::fs::create_dir_all(&checkout).unwrap();
    let local = Arc::new(LocalHost::with_config(
        "local",
        AppPaths::under(&base.join("desktop")),
    ));
    let remote = LocalHost::with_config(A, AppPaths::under(&base.join("relay")));
    let local_id = project(&local, &checkout);
    let remote_id = project(&remote, &checkout);
    let remote_registry = remote.registry().unwrap().clone();
    let router = HostRouter::new();
    router.add_host(local.clone()).unwrap();
    router.add_host(Arc::new(Remote(remote))).unwrap();
    router.bind_project(local_id.clone(), "local").unwrap();
    router.bind_project(remote_id.clone(), A).unwrap();

    let mut on_remote = router
        .connect(
            &Target::Member {
                host: A.into(),
                project: remote_id.clone(),
            },
            Caller::new(Role::Owner).in_window("project-r"),
        )
        .await
        .unwrap();
    let claimed = call(
        &mut on_remote,
        "project.claimTrustPrompt",
        json!({ "id": remote_id }),
    )
    .await;
    assert_eq!(claimed["result"], json!(true), "{claimed}");
    let set = call(
        &mut on_remote,
        "project.setTrust",
        json!({ "id": remote_id, "trusted": true }),
    )
    .await;
    assert_eq!(set["result"]["state"], json!("trusted"), "{set}");

    let local_registry = local.registry().unwrap();
    assert_eq!(
        local_registry.trust(&local_id).unwrap().state,
        pw_core::registry::TrustState::Unknown
    );
    assert!(remote_registry.trust(&remote_id).unwrap().trusted());
    let mut on_local = router
        .connect(
            &Target::ProjectId(local_id.clone()),
            Caller::new(Role::Owner).in_window("project-l"),
        )
        .await
        .unwrap();
    let claimed = call(
        &mut on_local,
        "project.claimTrustPrompt",
        json!({ "id": local_id }),
    )
    .await;
    assert_eq!(
        claimed["result"],
        json!(true),
        "the local prompt is still to come: {claimed}"
    );
    let untrust = call(
        &mut on_local,
        "project.setTrust",
        json!({ "id": local_id, "trusted": false }),
    )
    .await;
    assert_eq!(untrust["result"]["state"], json!("untrusted"), "{untrust}");
    assert!(remote_registry.trust(&remote_id).unwrap().trusted());
}

/// 델타 리뷰 G1 ①(털린 Relay 흉내): 기다리는 요청과 같은 창 · 요청 id · 연결을 대도 **다른 프로젝트** 의 청이면 거절 — 그 창이
/// 프로젝트 X 의 연결을 여는 동안 Relay 가 Y 의 같은 연결 id 비밀을 읽지 못한다.
#[test]
fn a_request_for_another_project_is_declined_even_with_the_waiting_ids() {
    const Y: &str = "0123ffff";
    let store = store_with(&[
        (machine_account(A, P, "dev", DbSecretKey::Password), ITEM),
        (
            machine_account(A, Y, "dev", DbSecretKey::Password),
            "item-of-y",
        ),
    ]);
    let desk = SecretDesk::new(store);
    let flights = Flights::default();
    sent(&flights, 1, 1, "db.session.open", "dev");
    let mut other = request(1, SecretRequestOp::Read);
    other.project_id = Y.into();
    assert_eq!(desk.answer(A, &flights, &other), SecretAnswer::Declined);
    assert_eq!(
        desk.answer(A, &flights, &request(1, SecretRequestOp::Read)),
        SecretAnswer::Value {
            item: Secret::new(ITEM)
        }
    );
}

/// 델타 리뷰 G1 ②(털린 Relay 흉내): 창 label 이 없는 `Project` 연결이 보낸 비밀이 필요한 요청은 세지 않는다 — Relay 가 빈 label
/// 로 그 요청 id 를 대도 거절. 같은 요청을 label 있는 창이 보내면 답한다(대조). 요청은 답하지 않는 Host 로 보내 기다림을 둔다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unlabeled_project_connection_opens_nothing_even_for_a_matching_request() {
    let desk = SecretDesk::new(a_dev());
    let silent: Arc<dyn Host> = Arc::new(LocalHost::new("silent"));
    let (gated, flights) = SecretDesk::gate(silent);
    let open = json!({ "jsonrpc": "2.0", "id": 1, "method": "db.session.open",
                       "params": { "projectId": P, "connectionId": "dev" } });
    let unlabeled = gated
        .connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(P.into()),
            window: None,
        })
        .await
        .unwrap();
    unlabeled.sink.send(open.clone());
    let mut forged = request(1, SecretRequestOp::Read);
    forged.call.window = String::new();
    assert_eq!(desk.answer(A, &flights, &forged), SecretAnswer::Declined);
    forged.call.window = "SHA256:dev/".into();
    assert_eq!(desk.answer(A, &flights, &forged), SecretAnswer::Declined);
    assert!(flights.is_empty());

    let labeled = gated.connect(window()).await.unwrap();
    labeled.sink.send(open);
    assert_eq!(
        desk.answer(A, &flights, &request(1, SecretRequestOp::Read)),
        SecretAnswer::Value {
            item: Secret::new(ITEM)
        }
    );
}

/// 표를 못 박는다(델타 리뷰 R1): 메서드를 더하거나 빼거나 종류를 바꾸면 실패한다 — 그 행이 이 시험을 고치며 리뷰에 보이게.
/// 판정 규칙은 표 옆 주석(그 창이 스스로 보여 주는 그 연결 하나를 위해 연결을 여는 요청만).
#[test]
fn the_secret_flows_are_exactly_these() {
    use SecretUse::{Read, Remove, Store};
    assert_eq!(
        SECRET_FLOWS,
        [
            ("db.session.open", Read),
            ("db.connection.test", Read),
            ("db.query.page", Read),
            ("db.schema.databases", Read),
            ("db.schema.schemas", Read),
            ("db.schema.objects", Read),
            ("db.schema.columns", Read),
            ("db.schema.indexes", Read),
            ("db.schema.foreignKeys", Read),
            ("db.schema.ddl", Read),
            ("db.schema.refresh", Read),
            ("db.secret.set", Store),
            ("db.secret.clear", Remove),
            ("db.connection.delete", Remove),
        ]
    );
}
