//! 원격 Relay 의 「Desktop 에 묻는」 비밀 저장소(`pw_db::desktop_secret` — 18-m7-plan M7-12 ② ④). Relay 의 Host 와 같은 조립(연결
//! 관리자 · DB 세션 · `secret.*`)을 가짜 드라이버로 실제 RPC 세션 너머에서 본다. Desktop 은 owner · Host 범위 연결 하나로
//! `secret.requests` 를 열고 `secret.answer` 로 답하는 가짜다(Desktop 쪽 판정 — 「그 창이 그 연결로 기다리는 동안만」 — 은
//! `pw_remote::secrets` 의 시험).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Conn, Fixture, StreamMsg, target_of};
use pw_db::DbSessions;
use pw_db::desktop_secret::{DesktopSecrets, SecretRequest, SecretRequestOp};
use pw_db::secret::{DbSecretKey, SecretAsker, SecretCall, SecretStore, with_asker};
use pw_db_driver::fake::FakeDriver;
use pw_db_types::SqlDialect;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

const PLANTED: &str = "PLANTED-remote-4c1e";
const DEV: &str = "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"db.internal\"\nuser = \"app\"\npassword = { secret_ref = \"keychain\" }\n";

struct Env {
    f: Fixture,
    pg: FakeDriver,
    secrets: DesktopSecrets,
}

fn env() -> Env {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(pg.clone())).unwrap();
    let secrets = DesktopSecrets::with_wait(Duration::from_secs(3));
    let store = Arc::new(secrets.clone());
    let as_store: Arc<dyn SecretStore> = store.clone();
    let f = Fixture::build_with_secrets(true, Some(as_store), |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
        store.register(router);
    });
    f.write_db("dev.toml", DEV);
    Env { f, pg, secrets }
}

/// 가짜 Desktop 이 내는 답.
#[derive(Clone, Copy)]
enum Answer {
    /// 읽기는 `item` 을, 두기 · 지우기는 `done`.
    Value,
    Declined,
}

/// 붙은 가짜 Desktop: 받은 청을 기록하고 `answer` 대로 답한다.
struct Desktop {
    seen: Arc<Mutex<Vec<SecretRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Desktop {
    async fn attach(f: &Fixture, secrets: &DesktopSecrets, item: Value, answer: Answer) -> Self {
        let mut conn = f.connect(ConnectionScope::Host);
        let stream = conn.start("secret.requests", Value::Null).await.unwrap();
        conn.credit(stream, 64);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            while let Some(StreamMsg::Chunk(data)) =
                conn.next_in(stream, Duration::from_secs(30)).await
            {
                let request: SecretRequest = serde_json::from_value(data).unwrap();
                record.lock().unwrap().push(request.clone());
                let answer = match (answer, &request.op) {
                    (Answer::Declined, _) => json!({ "kind": "declined" }),
                    (Answer::Value, SecretRequestOp::Read) => {
                        json!({ "kind": "value", "item": item.to_string() })
                    }
                    (Answer::Value, _) => json!({ "kind": "done" }),
                };
                conn.call(
                    "secret.answer",
                    json!({ "id": request.id, "answer": answer }),
                )
                .await
                .unwrap();
            }
        });
        // 스트림이 실제로 붙을 때까지(청을 받는 쪽으로 세어질 때까지) 기다린다.
        for _ in 0..400 {
            if secrets.desktops() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(secrets.desktops(), 1);
        Self { seen, task }
    }

    fn seen(&self) -> Vec<SecretRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 키체인 항목(값과 대상의 JSON) — Desktop 이 키체인에 둔 그대로.
async fn item(w: &mut Conn, project: &str, connection: &str) -> Value {
    let entry = w
        .call(
            "db.connection.get",
            json!({ "projectId": project, "connectionId": connection }),
        )
        .await
        .unwrap();
    json!({ "v": 1, "target": target_of(&entry["config"]), "value": PLANTED })
}

fn secret_reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

async fn open(w: &mut Conn, project: &str) -> Result<Value, RpcError> {
    w.call(
        "db.session.open",
        json!({ "projectId": project, "connectionId": "dev" }),
    )
    .await
}

const LABEL: &str = "SHA256:dev/project-1";

/// 그 프로젝트 창의 연결(링크로 온 것처럼 창 label 이 `<기기 id>/<label>`).
fn win(f: &Fixture) -> Conn {
    f.connect_in(ConnectionScope::Project(f.id.clone()), Some(LABEL))
}

/// 창의 흐름 + 그 비밀을 부른 요청(시험이 저장소를 바로 부를 때).
fn direct() -> SecretAsker {
    SecretAsker::Window(Some(SecretCall {
        window: "SHA256:dev/project-1".into(),
        request: "41".into(),
    }))
}

fn account(project: &str) -> String {
    pw_db::secret::account(project, "dev", DbSecretKey::Password)
}

/// 창이 연 세션은 Desktop 에 청하고, 받은 비밀로 드라이버에 붙는다. Relay 는 받은 비밀을 두지 않는다 — 다음 열기도 다시 청한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_flow_asks_the_desktop_each_time_and_keeps_nothing() {
    let e = env();
    let mut w = win(&e.f);
    let item = item(&mut w, &e.f.id, "dev").await;
    let desktop = Desktop::attach(&e.f, &e.secrets, item, Answer::Value).await;

    open(&mut w, &e.f.id).await.unwrap();
    open(&mut w, &e.f.id).await.unwrap();
    let specs = e.pg.connect_specs();
    assert_eq!(specs.len(), 2);
    for spec in &specs {
        assert_eq!(
            spec.password
                .as_ref()
                .map(|p| p.expose().to_owned())
                .as_deref(),
            Some(PLANTED)
        );
    }
    let seen = desktop.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    for request in &seen {
        assert_eq!(
            (
                request.project_id.as_str(),
                request.connection_id.as_str(),
                &request.op
            ),
            (e.f.id.as_str(), "dev", &SecretRequestOp::Read)
        );
    }
    // 청은 그것을 부른 창의 요청(창 label · 요청 id — 1 은 `db.connection.get`, 2 · 3 이 두 열기)을 댄다(리뷰 F1).
    let calls: Vec<(&str, &str)> = seen
        .iter()
        .map(|r| (r.call.window.as_str(), r.call.request.as_str()))
        .collect();
    assert_eq!(calls, [(LABEL, "2"), (LABEL, "3")]);
    // 창 label 이 없는 연결(요청을 댈 수 없는 흐름)은 묻지 않는다.
    let mut unlabeled = e.f.window();
    let refused = open(&mut unlabeled, &e.f.id).await.unwrap_err();
    assert_eq!(
        refused.code,
        pw_db::trust::code::SECRET_REQUIRED,
        "{refused:?}"
    );
    assert_eq!(desktop.seen().len(), 2);
    // 메모리 저장소(로컬 키체인 자리)에는 아무것도 없다.
    assert!(e.f.store.accounts().is_empty());
}

/// 창이 넣은 비밀(`db.secret.set`)은 Desktop 에 두기 청으로 가고 Relay 에는 남지 않는다. Desktop 이 거절하면 넣기가 실패한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_saved_secret_goes_to_the_desktop() {
    let e = env();
    let mut w = win(&e.f);
    let value = item(&mut w, &e.f.id, "dev").await;
    let desktop = Desktop::attach(&e.f, &e.secrets, value, Answer::Value).await;
    w.set_password(&e.f.id, "dev", PLANTED).await.unwrap();
    let seen = desktop.seen();
    assert_eq!(seen.len(), 1);
    match &seen[0].op {
        SecretRequestOp::Store { item } => {
            let item: Value = serde_json::from_str(item.expose()).unwrap();
            assert_eq!(item["value"], PLANTED);
        }
        other => panic!("{other:?}"),
    }
    // 청은 그 창의 `db.secret.set` 요청(1 · 2 는 `db.connection.get`)을 댄다.
    assert_eq!(
        (seen[0].call.window.as_str(), seen[0].call.request.as_str()),
        (LABEL, "3")
    );
    assert!(e.f.store.accounts().is_empty());
    // 지우기(`db.secret.clear`)도 그 창의 그 요청을 대고 Desktop 에 지우기 청을 보낸다(델타 리뷰 G2).
    w.call(
        "db.secret.clear",
        json!({ "projectId": e.f.id, "connectionId": "dev", "key": "password" }),
    )
    .await
    .unwrap();
    let seen = desktop.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[1].op, SecretRequestOp::Remove);
    assert_eq!(
        (seen[1].call.window.as_str(), seen[1].call.request.as_str()),
        (LABEL, "4")
    );
    drop(desktop);

    let e = env();
    let mut w = win(&e.f);
    let value = item(&mut w, &e.f.id, "dev").await;
    let _desktop = Desktop::attach(&e.f, &e.secrets, value, Answer::Declined).await;
    w.set_password(&e.f.id, "dev", PLANTED).await.unwrap_err();
}

/// Desktop 이 거절하면 「비밀 필요」, 붙은 Desktop 이 없으면 「쓸 수 없음」 이고 드라이버에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declined_or_absent_desktop_opens_nothing() {
    let e = env();
    let mut w = win(&e.f);
    let missing = open(&mut w, &e.f.id).await.unwrap_err();
    assert_eq!(
        missing.code,
        pw_db::trust::code::SECRET_REQUIRED,
        "{missing:?}"
    );
    assert_eq!(secret_reason(&missing), Some("unavailable"), "{missing:?}");

    let value = item(&mut w, &e.f.id, "dev").await;
    let desktop = Desktop::attach(&e.f, &e.secrets, value, Answer::Declined).await;
    let declined = open(&mut w, &e.f.id).await.unwrap_err();
    assert_eq!(
        declined.code,
        pw_db::trust::code::SECRET_REQUIRED,
        "{declined:?}"
    );
    assert_eq!(secret_reason(&declined), Some("missing"), "{declined:?}");
    assert_eq!(desktop.seen().len(), 1);
    assert!(e.pg.connect_specs().is_empty());
}

/// 에이전트의 증거로 연 연결은 Desktop 에 청하지 않는다(18-m7-plan M7-12 시험 ④ — D48 · D60). 사람이 그 연결을 연 뒤에도 에이전트의
/// `db_query` 는 「비밀 필요」 이고 청이 하나 더 가지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_flow_never_asks_the_desktop() {
    let e = env();
    let mut w = win(&e.f);
    let value = item(&mut w, &e.f.id, "dev").await;
    let desktop = Desktop::attach(&e.f, &e.secrets, value, Answer::Value).await;
    // 사람이 탐색기에서 연결을 펼친다(「사람이 연 연결」).
    w.call(
        "db.schema.objects",
        json!({ "projectId": e.f.id, "connectionId": "dev", "kind": "table", "limit": 500 }),
    )
    .await
    .unwrap();
    assert_eq!(desktop.seen().len(), 1);
    // 사람이 비밀을 넣는다(Desktop 에 두기 청 — Relay 는 대상 사본만 적는다). 에이전트의 툴이 「비밀 없음」 앞에서 멈추지 않고 연결을
    // 여는 데까지 가게.
    w.set_password(&e.f.id, "dev", PLANTED).await.unwrap();
    assert_eq!(desktop.seen().len(), 2);

    // 창 label 을 붙인 에이전트 연결 — label 이 있어도(Relay 의 `admit` 은 에이전트의 label 을 걷지만) 에이전트의 증거로는 묻지 않는다.
    let mut agent = e.f.connect_in(ConnectionScope::Agent, Some(LABEL));
    let refused = agent
        .call(
            "db.tool.query",
            json!({ "projectId": e.f.id, "connectionId": "dev", "sql": "SELECT 1" }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.code,
        pw_db::trust::code::SECRET_REQUIRED,
        "{refused:?}"
    );
    assert_eq!(desktop.seen().len(), 2, "{:?}", desktop.seen());
    // 연결 시험 길도 같다(`db.tool.connectionTest` — 리뷰 F3). 첫 호출은 「승인 필요」 와 계획, 승인 표를 실은 둘째 호출이 연결을
    // 시험하려다 「비밀 필요」 로 멈춘다(청이 가지 않는다).
    let test = |hash: Option<String>| {
        let mut params = json!({ "projectId": e.f.id, "connectionId": "dev" });
        if let Some(hash) = hash {
            params[pw_core::registry::plan::APPROVAL_KEY] = json!({ "hash": hash });
        }
        params
    };
    let asked = agent
        .call("db.tool.connectionTest", test(None))
        .await
        .unwrap_err();
    assert_eq!(
        asked.code,
        pw_core::registry::plan::APPROVAL_REQUIRED,
        "{asked:?}"
    );
    let hash = asked.data.as_ref().unwrap()["plan"]["hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = agent
        .call("db.tool.connectionTest", test(Some(hash)))
        .await
        .unwrap_err();
    assert_eq!(
        refused.code,
        pw_db::trust::code::SECRET_REQUIRED,
        "{refused:?}"
    );
    assert_eq!(desktop.seen().len(), 2, "{:?}", desktop.seen());
    assert_eq!(e.pg.connect_specs().len(), 1);

    // 저장소를 바로 불러도 같다: 에이전트 · 세운 곳 없는 흐름은 묻지 않고, 창의 흐름만 묻는다.
    let (s, account) = (e.secrets.clone(), account(&e.f.id));
    let (agent_read, unset_read, window_read) = tokio::task::spawn_blocking(move || {
        (
            with_asker(SecretAsker::Agent, || s.get(&account)),
            s.get(&account),
            with_asker(direct(), || s.get(&account)),
        )
    })
    .await
    .unwrap();
    assert_eq!(agent_read, Ok(None));
    assert_eq!(unset_read, Ok(None));
    assert!(window_read.unwrap().is_some());
    assert_eq!(desktop.seen().len(), 3);
}

/// 답은 그 청을 받은 연결의 것만 받는다. 다른 연결 · 프로젝트 창은 청을 받지도 답하지도 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_desktop_connection_that_got_a_request_answers_it() {
    let e = env();
    let mut window = win(&e.f);
    // 스트림은 열린 뒤 범위 판정으로 끝난다(`stream.end` failed).
    let refused = window.start("secret.requests", Value::Null).await.unwrap();
    window.credit(refused, 8);
    match window.next(refused).await {
        StreamMsg::End(pw_rpc::protocol::StreamOutcome::Failed { error }) => {
            assert_eq!(error.code, rpc_code::PERMISSION_DENIED, "{error:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(e.secrets.desktops(), 0);

    let mut desktop = e.f.connect(ConnectionScope::Host);
    let stream = desktop.start("secret.requests", Value::Null).await.unwrap();
    desktop.credit(stream, 8);
    let mut other = e.f.connect(ConnectionScope::Host);
    let (s, account) = (e.secrets.clone(), account(&e.f.id));
    let reading = tokio::task::spawn_blocking(move || with_asker(direct(), || s.get(&account)));
    let StreamMsg::Chunk(data) = desktop.next(stream).await else {
        panic!("no request");
    };
    let request: SecretRequest = serde_json::from_value(data).unwrap();
    let answer = json!({ "id": request.id, "answer": { "kind": "value", "item": "stolen" } });
    let e1 = other
        .call("secret.answer", answer.clone())
        .await
        .unwrap_err();
    assert_eq!(e1.code, rpc_code::NOT_FOUND, "{e1:?}");
    let e2 = window.call("secret.answer", answer).await.unwrap_err();
    assert_eq!(e2.code, rpc_code::PERMISSION_DENIED, "{e2:?}");
    desktop
        .call(
            "secret.answer",
            json!({ "id": request.id, "answer": { "kind": "missing" } }),
        )
        .await
        .unwrap();
    assert_eq!(reading.await.unwrap(), Ok(None));
}
