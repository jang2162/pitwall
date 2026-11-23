//! Desktop 쪽 비밀 답의 시험(18-m7-plan M7-12 Rust 시험 ② ④ ⑤ 와 ① 신뢰). 원격 Relay 자리에는 Relay 와 같은 「Desktop 에 묻는」
//! 저장소(`pw_db::desktop_secret`)와 그것을 부르는 시험 메서드 `db.probe` 를 단 Host 를 둔다 — `db.probe` 는 창의 DB 요청처럼
//! `projectId` · `connectionId` 를 싣고, 처리하는 동안 Relay 가 `ask` 연결의 비밀을 Desktop 에 청한다.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{Connection, HostRouter, LocalHost, Target};
use pw_db::desktop_secret::DesktopSecrets;
use pw_db::secret::{
    DbSecretKey, MemorySecretStore, SecretAsker, SecretStore, account, machine_account,
    machine_index_account, with_asker,
};
use pw_rpc::{Caller, ConnectionScope, Role, Router, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;

const A: &str = "SHA256:machine-a";
const B: &str = "SHA256:machine-b";
const P: &str = "0123abcd";
const ITEM: &str = "{\"v\":1,\"value\":\"from-a\"}";

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProbeParams {
    project_id: String,
    connection_id: String,
    /// Relay 가 Desktop 에 청할 연결.
    ask: String,
}

rpc_method! {
    /// 시험용: 처리하는 동안 Relay 가 `ask` 연결의 비밀을 창의 흐름으로 청한다. 받은 항목을 돌려준다.
    ProbeMethod: "db.probe" {
        params: ProbeParams,
        result: Option<String>,
        tier: Read,
    }
}

/// 원격 Relay 자리: Relay 의 비밀 저장소와 `db.probe`.
fn relay(secrets: &DesktopSecrets) -> Arc<dyn Host> {
    let store = Arc::new(secrets.clone());
    let probe = secrets.clone();
    let host = LocalHost::builder("relay")
        .methods(move |router: &mut Router| {
            store.register(router);
            router.unary(ProbeMethod, move |_ctx, p: ProbeParams| {
                let s = probe.clone();
                async move {
                    let account = account(&p.project_id, &p.ask, DbSecretKey::Password);
                    let got = tokio::task::spawn_blocking(move || {
                        with_asker(SecretAsker::Window, || s.get(&account))
                    })
                    .await
                    .unwrap();
                    Ok(got.ok().flatten().map(|s| s.expose().to_owned()))
                }
            });
        })
        .build();
    Arc::new(host)
}

struct Setup {
    secrets: DesktopSecrets,
    relay: Arc<dyn Host>,
    gated: Arc<dyn Host>,
    flights: Arc<Flights>,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// Desktop 이 `machine` 으로 증명된 링크의 Relay 에 붙은 꼴.
async fn setup(machine: &str, store: Arc<MemorySecretStore>) -> Setup {
    let secrets = DesktopSecrets::with_wait(Duration::from_secs(3));
    let relay = relay(&secrets);
    let desk = Arc::new(SecretDesk::new(store.clone()));
    let (gated, flights) = SecretDesk::gate(Arc::clone(&relay));
    let serving =
        tokio::spawn(desk.serve(machine.to_owned(), Arc::clone(&relay), Arc::clone(&flights)));
    for _ in 0..400 {
        if secrets.desktops() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(secrets.desktops(), 1, "the desktop listens");
    Setup {
        secrets,
        relay,
        gated,
        flights,
        serving,
    }
}

fn store_with(entries: &[(String, &str)]) -> Arc<MemorySecretStore> {
    let store = Arc::new(MemorySecretStore::new());
    for (account, value) in entries {
        store.put_raw(account, value);
    }
    store
}

fn window() -> Caller {
    Caller {
        role: Role::Owner,
        scope: ConnectionScope::Project(P.into()),
        window: Some("project-1".into()),
    }
}

async fn call(conn: &mut Connection, method: &str, params: Value) -> Value {
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

async fn probe(host: &Arc<dyn Host>, caller: Caller, connection: &str, ask: &str) -> Value {
    let mut conn = host.connect(caller).await.unwrap();
    let frame = call(
        &mut conn,
        "db.probe",
        json!({ "projectId": P, "connectionId": connection, "ask": ask }),
    )
    .await;
    frame["result"].clone()
}

/// ② 그 프로젝트 창이 그 연결로 요청을 기다리는 동안만 답한다. 답이 오면 기다림이 걷힌다 — 그 뒤의 청은 거절이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_window_waiting_on_that_connection_gets_an_answer() {
    let s = setup(
        A,
        store_with(&[(machine_account(A, P, "dev", DbSecretKey::Password), ITEM)]),
    )
    .await;
    assert_eq!(probe(&s.gated, window(), "dev", "dev").await, json!(ITEM));
    assert!(s.flights.is_empty());
    // 같은 창이 다른 연결로 기다리는 동안의 청 · 아무도 기다리지 않을 때의 청은 거절이다.
    assert_eq!(probe(&s.gated, window(), "other", "dev").await, Value::Null);
    let secrets = s.secrets.clone();
    let lone = tokio::task::spawn_blocking(move || {
        with_asker(SecretAsker::Window, || {
            secrets.get(&account(P, "dev", DbSecretKey::Password))
        })
    })
    .await
    .unwrap();
    assert_eq!(lone, Ok(None));
}

/// ④ 에이전트 범위 · Host 범위 · 창 label 이 없는 연결의 요청은 세지 않는다 — 그 흐름의 청에는 답하지 않는다. 감싸지 않은 Host 로
/// 보낸 요청도 같다(지켜보는 것은 감싼 Host 다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_and_host_connections_are_not_watched() {
    let s = setup(
        A,
        store_with(&[(machine_account(A, P, "dev", DbSecretKey::Password), ITEM)]),
    )
    .await;
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
        assert_eq!(
            probe(&s.gated, caller, "dev", "dev").await,
            Value::Null,
            "{scope:?}"
        );
    }
    assert_eq!(probe(&s.relay, window(), "dev", "dev").await, Value::Null);
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
    assert_eq!(probe(&b.gated, window(), "dev", "dev").await, Value::Null);
    let a = setup(A, store_with(&entries)).await;
    assert_eq!(probe(&a.gated, window(), "dev", "dev").await, json!(ITEM));
}

fn request(op: SecretRequestOp) -> SecretRequest {
    SecretRequest {
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
        P,
        &json!({ "jsonrpc": "2.0", "id": id, "method": method,
                 "params": { "projectId": P, "connectionId": connection } }),
    );
}

/// 두기는 그 창의 `db.secret.set` 이 기다릴 때만 하고, 계정 목록에 적는다. 답이 오거나 연결이 닫히면 기다림이 걷힌다. 다른
/// 프로젝트를 실은 요청 · 알림은 세지 않는다.
#[test]
fn storing_needs_a_waiting_secret_set_and_is_indexed() {
    let store = Arc::new(MemorySecretStore::new());
    let desk = SecretDesk::new(store.clone());
    let flights = Flights::default();
    let item = Secret::new(ITEM);
    let store_op = || request(SecretRequestOp::Store { item: item.clone() });

    sent(&flights, 1, 1, "db.session.open", "dev");
    assert_eq!(
        desk.answer(A, &flights, &store_op()),
        SecretAnswer::Declined
    );
    sent(&flights, 1, 2, "db.secret.set", "dev");
    assert_eq!(desk.answer(A, &flights, &store_op()), SecretAnswer::Done);
    let stored = machine_account(A, P, "dev", DbSecretKey::Password);
    assert_eq!(store.raw(&stored).as_deref(), Some(ITEM));
    assert_eq!(
        store.raw(&machine_index_account(A)),
        Some(json!([stored]).to_string())
    );

    flights.received(1, &json!({ "jsonrpc": "2.0", "id": 2, "result": null }));
    assert_eq!(
        desk.answer(A, &flights, &store_op()),
        SecretAnswer::Declined
    );
    flights.closed(1);
    assert!(flights.is_empty());
    assert_eq!(
        desk.answer(A, &flights, &request(SecretRequestOp::Read)),
        SecretAnswer::Declined
    );

    // 다른 프로젝트를 실은 요청(Relay 가 거절한다) · id 없는 알림은 세지 않는다.
    flights.sent(
        2,
        P,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "db.secret.set",
                 "params": { "projectId": "other", "connectionId": "dev" } }),
    );
    flights.sent(
        2,
        P,
        &json!({ "jsonrpc": "2.0", "method": "db.secret.set",
                 "params": { "projectId": P, "connectionId": "dev" } }),
    );
    assert!(flights.is_empty());

    sent(&flights, 3, 1, "db.secret.clear", "dev");
    assert_eq!(
        desk.answer(A, &flights, &request(SecretRequestOp::Remove)),
        SecretAnswer::Done
    );
    assert_eq!(store.raw(&stored), None);
    assert_eq!(store.raw(&machine_index_account(A)), None);
}

/// 「이 머신을 새로 믿기」: 옛 머신 id 의 비밀과 계정 목록을 지우고, 다른 머신 · 로컬 비밀은 두다.
#[test]
fn forgetting_a_machine_removes_only_its_secrets() {
    let store = Arc::new(MemorySecretStore::new());
    let desk = SecretDesk::new(store.clone());
    let flights = Flights::default();
    let item = Secret::new(ITEM);
    for machine in [A, B] {
        for connection in ["dev", "prod"] {
            sent(&flights, 1, 1, "db.secret.set", connection);
            let mut r = request(SecretRequestOp::Store { item: item.clone() });
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
