//! M5-17 의 완료 기준(16-m5-plan.md M5-17 행): 툴 메서드 `db_connection_upsert`(`db.tool.connectionUpsert`) ·
//! `db_connection_test`(`db.tool.connectionTest`)와 「승인받은 연결」 입구를 가짜 드라이버로 실제 RPC 세션 너머에서 본다. 승인 경로는
//! 세션(`pw_mcp`)의 일이라 여기서는 그 규약 — 첫 호출은 부작용 없이 계획을 실은 「승인 필요」(6001), 승인 표(`pitwall/approval` 의
//! 해시)를 실은 둘째 호출이 돈다 — 을 시험이 직접 한다. MCP 세션 하나는 Host 연결 하나다(여기서는 [`Conn`] 하나). MCP 왕복 ·
//! `auto_approve` · 툴 목록은 데스크톱 크레이트의 `mcp_tests/db.rs`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Conn, Fixture, fake_worktree};
use pw_core::bus::{Filter, Received, Scope, Subscription};
use pw_core::registry::plan::{
    APPROVAL_KEY, APPROVAL_REQUIRED, ApprovalRequired, PlanChange, Planned, Remember,
};
use pw_db::DbSessions;
use pw_db::connection_tool::{DbToolConnectionTestParams, DbToolConnectionUpsertParams};
use pw_db_driver::fake::{FakeCall, FakeDriver};
use pw_db_types::SqlDialect;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

struct Env {
    f: Fixture,
    sessions: Arc<DbSessions>,
    pg: FakeDriver,
}

const DEV: &str =
    "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"localhost\"\ndatabase = \"app\"\n";
const PROD: &str = "schema = 1\nname = \"prod\"\ndriver = \"postgres\"\nhost = \"db.prod.example\"\ndatabase = \"app\"\nuser = \"app\"\nproduction = true\n";
/// [`PROD`] 와 같은 대상을 표지 없이 가리키는 다른 연결.
const SNEAKY: &str = "schema = 1\nname = \"sneaky\"\ndriver = \"postgres\"\nhost = \"DB.prod.example\"\nport = 5432\ndatabase = \"app\"\nuser = \"app\"\n";

fn env() -> Env {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(pg.clone())).unwrap();
    let mut slot = None;
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        slot = Some(sessions);
    });
    f.write_db("dev.toml", DEV);
    Env {
        f,
        sessions: slot.unwrap(),
        pg,
    }
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

#[track_caller]
fn refused(result: &Result<Value, RpcError>, why: &str) -> RpcError {
    let e = result.clone().expect_err("refused");
    assert_eq!(
        (e.code, reason(&e)),
        (rpc_code::PERMISSION_DENIED, Some(why)),
        "{e:?}"
    );
    e
}

/// 「승인 필요」(6001)의 계획. 다른 오류 · 결과면 실패.
#[track_caller]
fn asked(result: &Result<Value, RpcError>) -> ApprovalRequired {
    let e = result.clone().expect_err("approval required");
    assert_eq!(e.code, APPROVAL_REQUIRED, "{e:?}");
    serde_json::from_value(e.data.unwrap()).unwrap()
}

/// 계획의 글 갈래 하나(「이 연결 열기」 의 대상).
fn text_of(required: &ApprovalRequired) -> String {
    match required.plan.changes.as_slice() {
        [PlanChange::Text { text, .. }] => text.clone(),
        other => panic!("{other:?}"),
    }
}

impl Env {
    /// 앱을 다시 켠 흉내(같은 파일 · 키체인, 빈 메모리).
    fn restart(&mut self) {
        let pg = self.pg.clone();
        let mut slot = None;
        self.f.restart(|router, connections| {
            let mut drivers = pw_db::DriverRegistry::new();
            drivers.register(Arc::new(pg)).unwrap();
            let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
            pw_db::register(router, Arc::clone(&sessions));
            slot = Some(sessions);
        });
        self.sessions = slot.unwrap();
    }

    fn agent(&self) -> Conn {
        self.f.connect(ConnectionScope::Agent)
    }

    /// 연결 워크트리 `p1-wt` 를 만들고 그 꼭대기를 돌려준다. `files` 는 그 워크트리의 `.pitwall/db/` 에 쓴다.
    fn worktree(&self, files: &[(&str, &str)]) -> String {
        let wt = fake_worktree(&self.f.root, &self.f.base.join("p1-wt"));
        std::fs::create_dir_all(wt.join(".pitwall/db")).unwrap();
        for (name, text) in files {
            std::fs::write(wt.join(".pitwall/db").join(name), text).unwrap();
        }
        wt.to_string_lossy().into_owned()
    }

    /// 사람이 그 프로젝트 창의 탐색기에서 연결을 펼친다.
    async fn human_opens(&self, connection: &str, worktree: Option<&str>) {
        let mut w = self.f.window();
        w.call(
            "db.schema.objects",
            json!({ "projectId": self.f.id, "worktree": worktree, "connectionId": connection,
                    "kind": "table", "limit": 500 }),
        )
        .await
        .unwrap();
    }

    /// `[mcp.policy] production` 을 사람의 설정(`.pitwall/project.local.toml`)에.
    fn production_policy(&self, value: &str) {
        let dir = self.f.root.join(".pitwall");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("project.local.toml"),
            format!("schema = 1\n[mcp.policy]\nproduction = \"{value}\"\n"),
        )
        .unwrap();
    }

    async fn call(
        &self,
        agent: &mut Conn,
        method: &str,
        mut params: Value,
        hash: Option<&str>,
    ) -> Result<Value, RpcError> {
        let object = params.as_object_mut().unwrap();
        object.insert("projectId".into(), json!(self.f.id));
        if let Some(hash) = hash {
            object.insert(APPROVAL_KEY.into(), json!({ "hash": hash }));
        }
        agent.call(method, params).await
    }

    async fn test(
        &self,
        agent: &mut Conn,
        connection: &str,
        worktree: Option<&str>,
        hash: Option<&str>,
    ) -> Result<Value, RpcError> {
        self.call(
            agent,
            "db.tool.connectionTest",
            json!({ "connectionId": connection, "worktree": worktree }),
            hash,
        )
        .await
    }

    /// 묻고(첫 호출) 승인해(둘째 호출) 연결 테스트를 돌린다.
    async fn test_approved(
        &self,
        agent: &mut Conn,
        connection: &str,
        worktree: Option<&str>,
    ) -> (ApprovalRequired, Result<Value, RpcError>) {
        let required = asked(&self.test(agent, connection, worktree, None).await);
        let ran = self
            .test(agent, connection, worktree, Some(&required.plan.hash))
            .await;
        (required, ran)
    }

    async fn schema(
        &self,
        agent: &mut Conn,
        connection: &str,
        worktree: Option<&str>,
    ) -> Result<Value, RpcError> {
        self.call(
            agent,
            "db.tool.schema",
            json!({ "connectionId": connection, "worktree": worktree }),
            None,
        )
        .await
    }

    async fn upsert(
        &self,
        agent: &mut Conn,
        params: Value,
        hash: Option<&str>,
    ) -> Result<Value, RpcError> {
        self.call(agent, "db.tool.connectionUpsert", params, hash)
            .await
    }

    fn connects(&self) -> usize {
        self.pg.count(&FakeCall::Connect)
    }

    fn secret_requests(&self) -> Subscription {
        self.f.bus.subscribe(
            Filter::all()
                .scope(Scope::Project(self.f.id.clone()))
                .topics(["db.secretRequested"]),
        )
    }
}

async fn next_event(events: &mut Subscription) -> Value {
    let received = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("no event")
        .unwrap();
    let Received::Event(event) = received else {
        panic!("lagged")
    };
    event.payload.clone()
}

/// JSON Schema 안의 모든 속성 이름(중첩 · 갈래 · 정의 포함).
fn property_names(schema: &Value, out: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::Object(props)) = map.get("properties") {
                out.extend(props.keys().cloned());
            }
            for value in map.values() {
                property_names(value, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| property_names(v, out)),
        _ => {}
    }
}

fn schema_of<T: schemars::JsonSchema>() -> Value {
    schemars::SchemaGenerator::default()
        .into_root_schema_for::<T>()
        .to_value()
}

// --- 입력 스키마 ------------------------------------------------------------------------------------------------

/// `db_connection_upsert` 의 입력에는 비밀 값이 들어갈 칸이 없다 — 비밀번호 칸은 출처(`keychain` · `env` 의 변수 이름)뿐이다.
/// 비밀 값을 실어 보내면(알 수 없는 키 · 글자열 비밀번호) 아무것도 쓰지 않고 invalid params 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_upsert_input_has_no_place_for_a_secret() {
    let schema = schema_of::<Planned<DbToolConnectionUpsertParams>>();
    let mut names = Vec::new();
    property_names(&schema, &mut names);
    for word in ["secret", "value", "token"] {
        assert!(
            !names.iter().any(|n| n.to_ascii_lowercase().contains(word)),
            "{word} in {names:?}"
        );
    }
    let password = schema.to_string();
    assert!(
        password.contains("keychain") && password.contains("variable"),
        "{schema}"
    );
    // `password` 의 갈래는 둘이고, 갈래의 속성은 `kind` 와 `variable` 뿐이다.
    let mut password_props = Vec::new();
    let defs = schema["$defs"]["DbToolPasswordRef"].clone();
    property_names(&defs, &mut password_props);
    password_props.sort();
    password_props.dedup();
    assert_eq!(password_props, ["kind", "variable"], "{defs}");

    let e = env();
    let mut agent = e.agent();
    let config = json!({ "name": "new", "driver": "postgres", "host": "localhost" });
    for bad in [
        json!({ "connectionId": "new", "config": config, "secret": "hunter2" }),
        json!({ "connectionId": "new", "config": { "name": "new", "driver": "postgres", "host": "localhost",
                "password": "hunter2" } }),
        json!({ "connectionId": "new", "config": { "name": "new", "driver": "postgres", "host": "localhost",
                "password": { "kind": "env", "variable": "PGPASS", "value": "hunter2" } } }),
    ] {
        let err = e.upsert(&mut agent, bad.clone(), None).await.unwrap_err();
        assert_eq!(err.code, rpc_code::INVALID_PARAMS, "{bad}: {err:?}");
        assert!(!err.message.contains("hunter2"), "{err:?}");
    }
    assert!(!e.f.db_file("new.toml").exists());
}

/// `db_connection_test` 의 입력은 `{ projectId, worktree?, connectionId }` 뿐이다 — 밑의 RPC 가 받는 `draft` · `secret` 이 없다.
/// 실어 보내면 거절되고 드라이버에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_connection_test_input_has_no_draft_or_secret() {
    let schema = schema_of::<Planned<DbToolConnectionTestParams>>();
    let mut names = Vec::new();
    property_names(&schema, &mut names);
    names.sort();
    assert_eq!(names, ["connectionId", "projectId", "worktree"], "{schema}");

    let e = env();
    let mut agent = e.agent();
    for extra in [
        json!({ "draft": { "name": "x", "driver": "postgres", "host": "evil.example" } }),
        json!({ "secret": "hunter2" }),
    ] {
        let mut params = json!({ "connectionId": "dev" });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let err = e
            .call(&mut agent, "db.tool.connectionTest", params, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, rpc_code::INVALID_PARAMS, "{extra}: {err:?}");
    }
    assert_eq!(e.connects(), 0);
}

// --- db_connection_upsert -------------------------------------------------------------------------------------

/// 첫 호출은 파일을 쓰지 않고 그 파일의 diff 를 계획으로 낸다. 승인 표를 실은 둘째 호출이 계획의 `after` 를 그대로 쓴다. 있는
/// 파일이면 앞의 내용(사람이 단 주석 포함)이 `before` 이고 주석이 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upsert_writes_nothing_before_approval_and_the_planned_file_after() {
    let e = env();
    let mut agent = e.agent();
    let params = json!({ "connectionId": "fresh", "config": {
        "name": "fresh", "driver": "postgres", "host": "db.example", "database": "app",
        "password": { "kind": "env", "variable": "PGPASSWORD" } } });
    let required = asked(&e.upsert(&mut agent, params.clone(), None).await);
    assert!(!e.f.db_file("fresh.toml").exists(), "nothing written yet");
    let [
        PlanChange::File {
            path,
            before,
            after,
        },
    ] = required.plan.changes.as_slice()
    else {
        panic!("{:?}", required.plan.changes)
    };
    assert_eq!(std::path::Path::new(path), e.f.db_file("fresh.toml"));
    assert_eq!(before, &None);
    let after = after.clone().unwrap();
    assert!(after.contains("env = \"PGPASSWORD\""), "{after}");
    assert!(
        required.plan.rules.auto_approve,
        "write follows the usual rules"
    );
    let saved = e
        .upsert(&mut agent, params, Some(&required.plan.hash))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(e.f.db_file("fresh.toml")).unwrap(),
        after
    );
    assert_eq!(saved["connectionId"], "fresh");
    // `env` 참조는 사람이 처음 쓸 때 확인한다 — 저장이 그 확인을 대신하지 않는다.
    assert_eq!(saved["state"], "approvalRequired", "{saved}");

    // 있는 파일: 앞의 내용이 `before`, 사람의 주석이 남는다.
    let commented = format!("# team db\n{DEV}");
    e.f.write_db("dev.toml", &commented);
    let change = json!({ "connectionId": "dev", "config": {
        "name": "dev", "driver": "postgres", "host": "localhost", "database": "other" } });
    let required = asked(&e.upsert(&mut agent, change.clone(), None).await);
    let [PlanChange::File { before, after, .. }] = required.plan.changes.as_slice() else {
        panic!()
    };
    assert_eq!(before.as_deref(), Some(commented.as_str()));
    assert_eq!(
        std::fs::read_to_string(e.f.db_file("dev.toml")).unwrap(),
        commented
    );
    e.upsert(&mut agent, change, Some(&required.plan.hash))
        .await
        .unwrap();
    let written = std::fs::read_to_string(e.f.db_file("dev.toml")).unwrap();
    assert_eq!(Some(written.as_str()), after.as_deref());
    assert!(
        written.starts_with("# team db\n") && written.contains("\"other\""),
        "{written}"
    );
}

/// 승인 뒤 파일이 바뀌었으면(동료의 커밋 · 다른 쓰기) 승인받은 diff 로 덮지 않는다 — 다시 만든 계획의 해시가 달라 「승인 필요」
/// (`mismatch`)이고 파일은 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upsert_does_not_overwrite_a_file_changed_after_approval() {
    let e = env();
    let mut agent = e.agent();
    let change = json!({ "connectionId": "dev", "config": {
        "name": "dev", "driver": "postgres", "host": "localhost", "database": "other" } });
    let required = asked(&e.upsert(&mut agent, change.clone(), None).await);
    let theirs = DEV.replace("\"app\"", "\"theirs\"");
    e.f.write_db("dev.toml", &theirs);
    let again = asked(
        &e.upsert(&mut agent, change, Some(&required.plan.hash))
            .await,
    );
    assert!(again.mismatch);
    assert_eq!(
        std::fs::read_to_string(e.f.db_file("dev.toml")).unwrap(),
        theirs
    );
}

/// 비밀이 필요한 연결(키체인 비밀번호 — 아직 없다): 저장하면 그 프로젝트 창에 비밀 입력 요청 이벤트가 가고 에이전트에게는
/// 사람이 넣어야 한다는 안내가 간다. `db_connection_test` 는 승인을 묻기 전에 같은 이벤트를 내고 안내 오류(3101,
/// `secretRequired`)로 답한다 — 드라이버에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_needs_its_password_asks_the_window() {
    let e = env();
    let mut events = e.secret_requests();
    let mut agent = e.agent();
    let params = json!({ "connectionId": "kc", "config": {
        "name": "Keychain DB", "driver": "postgres", "host": "localhost", "database": "app",
        "password": { "kind": "keychain" } } });
    let required = asked(&e.upsert(&mut agent, params.clone(), None).await);
    let saved = e
        .upsert(&mut agent, params, Some(&required.plan.hash))
        .await
        .unwrap();
    assert_eq!(saved["state"], "secretRequired", "{saved}");
    assert!(
        saved["notice"]
            .as_str()
            .unwrap()
            .contains("only the person can enter"),
        "{saved}"
    );
    let event = next_event(&mut events).await;
    assert_eq!(
        event,
        json!({ "connectionId": "kc", "name": "Keychain DB", "worktree": null })
    );

    e.human_opens("dev", None).await;
    let before = e.connects();
    let err = e.test(&mut agent, "kc", None, None).await.unwrap_err();
    assert_eq!(
        (err.code, reason(&err)),
        (pw_db::trust::code::SECRET_REQUIRED, Some("secretRequired")),
        "{err:?}"
    );
    assert!(
        err.message.contains("only the person can enter in Pitwall"),
        "{err:?}"
    );
    assert_eq!(next_event(&mut events).await["connectionId"], "kc");
    assert_eq!(e.connects(), before, "the driver was not reached");
}

// --- db_connection_test -------------------------------------------------------------------------------------------

/// 사람이 연 연결은 `write` 의 보통 규칙(`auto_approve` · 「이 세션 동안 허용」 = 그 툴)으로 묻고, 승인 뒤 연결을 열어 서버 판본을
/// 돌려준다. 「승인받은 연결」 기록을 더하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_opened_connection_is_tested_under_the_usual_write_rules() {
    let e = env();
    e.human_opens("dev", None).await;
    let mut agent = e.agent();
    let before = e.connects();
    let (required, ran) = e.test_approved(&mut agent, "dev", None).await;
    assert!(required.plan.rules.auto_approve);
    assert_eq!(required.plan.rules.remember, Remember::Tool);
    assert!(
        required.plan.summary.starts_with("Test the DB connection"),
        "{}",
        required.plan.summary
    );
    let ran = ran.unwrap();
    assert_eq!(ran["approvedForSession"], false, "{ran}");
    assert!(ran.get("tls").is_some(), "{ran}");
    assert_eq!(e.connects(), before + 1);
    assert_eq!(e.sessions.approved_connections(), 0);
}

/// 연 적 없는 연결: 첫 호출은 드라이버에 닿지 않고 「이 연결 열기」 계획을 낸다 — `auto_approve` 를 적용하지 않고, 세션 허용은
/// (연결 id, 푼 대상)의 열쇠이며, 요약이 「이 에이전트 세션 동안」 을 말하고 대상(호스트 · DB)을 보인다. 승인 뒤 연결을 열고
/// 그 세션의 `db_schema` 가 된다. 다른 세션에는 닿지 않고, 「사람이 연 연결」 기록에도 섞이지 않으며(M5-06 리뷰 E), 세션이
/// 끝나면 기록이 지워진다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approving_an_open_lets_that_session_use_the_connection() {
    let e = env();
    let mut agent = e.agent();
    let mut other = e.agent();
    let required = asked(&e.test(&mut agent, "dev", None, None).await);
    assert_eq!(e.connects(), 0, "nothing opened before the approval");
    assert!(
        !required.plan.rules.auto_approve,
        "auto_approve does not apply"
    );
    let Remember::Key { key } = &required.plan.rules.remember else {
        panic!("{:?}", required.plan.rules)
    };
    assert!(key.starts_with("dev@sha256:"), "{key}");
    assert!(
        required
            .plan
            .summary
            .starts_with("Open this DB connection for this agent session"),
        "{}",
        required.plan.summary
    );
    let target = text_of(&required);
    assert!(
        target.contains("host: localhost") && target.contains("database: app"),
        "{target}"
    );
    refused(&e.schema(&mut agent, "dev", None).await, "notOpenedByUser");

    let ran = e
        .test(&mut agent, "dev", None, Some(&required.plan.hash))
        .await
        .unwrap();
    assert_eq!(ran["approvedForSession"], true, "{ran}");
    assert_eq!(e.connects(), 1);
    e.schema(&mut agent, "dev", None).await.unwrap();
    // 같은 세션의 다음 테스트는 사람이 연 연결과 같은 보통 규칙이다.
    let again = asked(&e.test(&mut agent, "dev", None, None).await);
    assert!(again.plan.rules.auto_approve);

    // 다른 세션 · 사람의 기록에는 닿지 않는다.
    refused(&e.schema(&mut other, "dev", None).await, "notOpenedByUser");
    assert!(
        !e.sessions.opened_by_user(&e.f.id, "dev"),
        "not recorded as opened by the user"
    );
    let list = e
        .call(&mut other, "db.tool.connectionList", json!({}), None)
        .await
        .unwrap();
    assert_eq!(
        list["connections"][0]["unusableReason"], "notOpenedByUser",
        "{list}"
    );

    // 세션이 끝나면 기록이 지워진다.
    assert_eq!(e.sessions.approved_connections(), 1);
    agent.session.close();
    for _ in 0..500 {
        if e.sessions.approved_connections() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(e.sessions.approved_connections(), 0);
}

/// 승인은 그 연결 id 와 **푼 대상**에 묶인다: 워크트리의 같은 id 가 다른 대상이면 닿지 않고, 승인 뒤 연결의 대상이 바뀌면(파일을
/// 고치든 `db_connection_upsert` 로 바꾸든) 열지 않는다 — 다시 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_approval_is_bound_to_the_connection_id_and_its_target() {
    let e = env();
    let elsewhere = DEV.replace("localhost", "other.example");
    let wt = e.worktree(&[("dev.toml", &elsewhere)]);
    let mut agent = e.agent();
    let (_, ran) = e.test_approved(&mut agent, "dev", None).await;
    ran.unwrap();
    e.schema(&mut agent, "dev", None).await.unwrap();
    refused(
        &e.schema(&mut agent, "dev", Some(&wt)).await,
        "notOpenedByUser",
    );
    let in_wt = asked(&e.test(&mut agent, "dev", Some(&wt), None).await);
    assert!(
        text_of(&in_wt).contains("host: other.example"),
        "{}",
        text_of(&in_wt)
    );

    // 에이전트가 대상을 바꾼다(승인받은 id 를 다른 대상에 다시 쓰기).
    let change = json!({ "connectionId": "dev", "config": {
        "name": "dev", "driver": "postgres", "host": "moved.example", "database": "app" } });
    let required = asked(&e.upsert(&mut agent, change.clone(), None).await);
    e.upsert(&mut agent, change, Some(&required.plan.hash))
        .await
        .unwrap();
    let before = e.connects();
    refused(&e.schema(&mut agent, "dev", None).await, "notOpenedByUser");
    let again = asked(&e.test(&mut agent, "dev", None, None).await);
    assert!(!again.plan.rules.auto_approve);
    assert!(text_of(&again).contains("host: moved.example"));
    assert_eq!(e.connects(), before);
}

/// 「이 연결 열기」 승인 뒤 · 연결 직전에 설정 파일의 대상을 바꿔 끼우면 연결을 열지 않는다 — 연결 테스트의 열기 자리도 에이전트의
/// 증거를 견준다(`check_agent_use`, M5-06 리뷰 D). 승인 뒤 · 연결 전에 프로덕션 표지를 더해도 열지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_target_swapped_after_the_approval_is_not_opened_by_the_test() {
    for (swap, why) in [
        (
            ("host = \"localhost\"", "host = \"evil.example\""),
            "notOpenedByUser",
        ),
        (
            (
                "database = \"app\"\n",
                "database = \"app\"\nproduction = true\n",
            ),
            "productionBlocked",
        ),
    ] {
        let e = env();
        let mut agent = e.agent();
        let required = asked(&e.test(&mut agent, "dev", None, None).await);
        let file = e.f.db_file("dev.toml");
        e.sessions.test_hooks.set_before_resolve(Box::new(move || {
            let text = std::fs::read_to_string(&file).unwrap();
            std::fs::write(&file, text.replace(swap.0, swap.1)).unwrap();
            Box::pin(async {})
        }));
        let ran = e
            .test(&mut agent, "dev", None, Some(&required.plan.hash))
            .await;
        refused(&ran, why);
        assert_eq!(e.connects(), 0, "{why}: the swapped target was not reached");
    }
}

/// `db_schema` · `db_query` 는 연 적 없는 연결에 승인을 청하지 않는다 — 묻지 않고 거절하고(6001 이 아니다), 문장이
/// `db_connection_test` 로 승인을 청하는 길을 알린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_schema_and_db_query_do_not_ask_to_open() {
    let e = env();
    let mut agent = e.agent();
    let err = refused(&e.schema(&mut agent, "dev", None).await, "notOpenedByUser");
    assert!(err.message.contains("db_connection_test"), "{err:?}");
    let query = e
        .call(
            &mut agent,
            "db.tool.query",
            json!({ "connectionId": "dev", "sql": "SELECT 1" }),
            None,
        )
        .await;
    refused(&query, "notOpenedByUser");
    assert_eq!(e.connects(), 0);
}

// --- 프로덕션 -------------------------------------------------------------------------------------------------

/// 프로덕션 연결은 승인을 묻지 않고 거절한다 — `block`(기본)이면 `productionBlocked`, `ask` 여도 사람이 연 적이 없으면
/// `notOpenedByUser`(승인으로도 열리지 않는다 — 결정 M5-D21). `ask` 이고 사람이 연 연결이면 호출마다 묻는다(세션 허용 없음).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_production_connection_is_refused_without_asking() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    let mut agent = e.agent();
    refused(
        &e.test(&mut agent, "prod", None, None).await,
        "productionBlocked",
    );
    e.production_policy("ask");
    let err = refused(
        &e.test(&mut agent, "prod", None, None).await,
        "notOpenedByUser",
    );
    assert!(!err.message.contains("db_connection_test"), "{err:?}");
    assert_eq!(e.connects(), 0);
    e.human_opens("prod", None).await;
    let required = asked(&e.test(&mut agent, "prod", None, None).await);
    assert!(!required.plan.rules.auto_approve);
    assert_eq!(required.plan.rules.remember, Remember::Never);
}

/// 메인 창에서 프로덕션 연결을 연 뒤 워크트리의 같은 id 에서 `production` 을 지워도, `db_connection_test` 는 승인을 묻지 않고
/// 거절한다(표지는 대상에 묶인다 — 결정 M5-D37).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_production_in_a_worktree_does_not_open_it_by_approval() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    e.human_opens("prod", None).await;
    let unmarked = PROD.replace("production = true\n", "");
    let wt = e.worktree(&[("prod.toml", &unmarked)]);
    let mut agent = e.agent();
    refused(
        &e.test(&mut agent, "prod", Some(&wt), None).await,
        "productionBlocked",
    );
    // 에이전트는 `db_connection_upsert` 로 메인의 표지를 지우지 못한다(묻지 않고 거절 — 표지는 조이기만 한다).
    let change = json!({ "connectionId": "prod", "config": {
        "name": "prod", "driver": "postgres", "host": "db.prod.example", "database": "app", "user": "app" } });
    refused(&e.upsert(&mut agent, change, None).await, "lowersMarks");
    assert_eq!(
        std::fs::read_to_string(e.f.db_file("prod.toml")).unwrap(),
        PROD
    );
    refused(
        &e.test(&mut agent, "prod", None, None).await,
        "productionBlocked",
    );
    assert_eq!(e.connects(), 1, "only the person's open");
}

/// 프로덕션으로 연 대상을 표지 없는 새 연결 id 로 가리켜도 `db_schema` 가 거절되고, `db_connection_test` 의 승인으로도 열리지
/// 않는다(묻지 않고 거절한다) — 에이전트가 `db_connection_upsert` 로 그 새 연결을 만들어도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_unmarked_id_on_a_production_target_is_not_opened_by_approval() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    e.human_opens("prod", None).await;
    let wt = e.worktree(&[("sneaky.toml", SNEAKY)]);
    let mut agent = e.agent();
    refused(
        &e.schema(&mut agent, "sneaky", Some(&wt)).await,
        "productionBlocked",
    );
    refused(
        &e.test(&mut agent, "sneaky", Some(&wt), None).await,
        "productionBlocked",
    );
    // 표지 없는 새 연결을 그 대상에 만드는 쓰기는 묻지 않고 거절한다(표지를 달면 보통대로 묻는다).
    let fresh = json!({ "connectionId": "fresh", "config": {
        "name": "fresh", "driver": "postgres", "host": "db.prod.example", "database": "app", "user": "app" } });
    refused(&e.upsert(&mut agent, fresh, None).await, "lowersMarks");
    assert!(!e.f.db_file("fresh.toml").exists());
    assert_eq!(e.connects(), 1, "only the person's open");
}

/// 사람이 연 적이 없어도 메인 체크아웃의 설정이 프로덕션으로 적은 대상을 워크트리의 표지 없는 새 연결로 가리키면
/// `db_connection_test` 가 승인을 묻지 않고 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_main_checkout_marks_a_target_nobody_opened_for_the_test() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    let wt = e.worktree(&[("sneaky.toml", SNEAKY)]);
    let mut agent = e.agent();
    refused(
        &e.test(&mut agent, "sneaky", Some(&wt), None).await,
        "productionBlocked",
    );
    assert_eq!(e.connects(), 0);
}

/// 승인받은 연결에 뒤에 프로덕션 표지가 더해지면 그 세션의 `db_schema` 도 거절한다 — 승인받은 연결은 프로덕션 대상을 지나지
/// 못한다(`ask` 여도 사람이 연 연결만).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approved_connection_marked_production_later_is_refused() {
    let e = env();
    let mut agent = e.agent();
    let (_, ran) = e.test_approved(&mut agent, "dev", None).await;
    ran.unwrap();
    e.schema(&mut agent, "dev", None).await.unwrap();
    // 같은 대상을 프로덕션으로 적은 다른 연결이 읽힌다.
    e.f.write_db(
        "dev-prod.toml",
        &DEV.replace("name = \"dev\"", "name = \"dev-prod\"")
            .replace(
                "database = \"app\"\n",
                "database = \"app\"\nproduction = true\n",
            ),
    );
    refused(
        &e.schema(&mut agent, "dev", None).await,
        "productionBlocked",
    );
    e.production_policy("ask");
    refused(&e.schema(&mut agent, "dev", None).await, "notOpenedByUser");
}

// --- 에이전트의 연결 쓰기는 표지를 조이기만 한다(메인 판정 2026-11-17 — 리뷰 B1) ----------------------------------------

/// 리뷰 B1 의 재현: 메인의 `prod.toml` 이 프로덕션(또는 read-only)일 때 에이전트가 같은 대상 · 표지 없는 설정을 개인 파일
/// (`location: "local"`)이나 공유 파일에 쓰려 하면 묻지 않고 거절하고(6001 이 아니다) 파일이 그대로다. 앱을 다시 켜도(기억이 빈
/// 새 실행) 그 대상은 여전히 표지가 있다 — 「이 연결 열기」 승인으로 열리지 않고, 목록이 표지를 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_cannot_lower_marks_even_across_app_runs() {
    for (mark, location) in [
        ("production", "local"),
        ("production", "shared"),
        ("read_only", "local"),
        ("read_only", "shared"),
    ] {
        let mut e = env();
        let marked = format!(
            "schema = 1\nname = \"prod\"\ndriver = \"postgres\"\nhost = \"db.prod.example\"\ndatabase = \"app\"\nuser = \"app\"\n{mark} = true\n"
        );
        e.f.write_db("prod.toml", &marked);
        let mut agent = e.agent();
        let field = if mark == "production" {
            "production"
        } else {
            "readOnly"
        };
        let lowered = json!({ "connectionId": "prod", "location": location, "config": {
            "name": "prod", "driver": "postgres", "host": "db.prod.example", "database": "app", "user": "app",
            field: false } });
        let err = refused(&e.upsert(&mut agent, lowered, None).await, "lowersMarks");
        assert!(
            err.message.contains("only the person can lower a mark"),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read_to_string(e.f.db_file("prod.toml")).unwrap(),
            marked
        );
        assert!(
            !e.f.db_file("prod.local.toml").exists(),
            "{mark} {location}"
        );

        e.restart();
        let mut agent = e.agent();
        let list = e
            .call(&mut agent, "db.tool.connectionList", json!({}), None)
            .await
            .unwrap();
        let prod = list["connections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "prod")
            .unwrap();
        assert_eq!(prod[field], true, "{mark} {location}: {list}");
        if mark == "production" {
            refused(
                &e.test(&mut agent, "prod", None, None).await,
                "productionBlocked",
            );
        }
        assert_eq!(e.connects(), 0);
    }
}

/// 레이어 · 체크아웃으로 비껴가지 못한다: 공유 파일은 표지가 없고 개인 파일(`.local.toml`)만 `production = true` 여도, 메인
/// 체크아웃만 표지가 있고 워크트리의 같은 id 는 표지가 없어도, 표지를 지우는 쓰기는 거절한다. 표지를 조이는 쓰기(read-only 를
/// 더함)는 보통대로 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marks_in_any_layer_or_checkout_hold() {
    let e = env();
    let mut agent = e.agent();
    // 조이는 쓰기는 묻는다(표지를 기억하기 전에 — 아래에서 dev 의 대상이 프로덕션으로 기억된다).
    let tighter = json!({ "connectionId": "dev", "config": {
        "name": "dev", "driver": "postgres", "host": "localhost", "database": "app", "readOnly": true } });
    asked(&e.upsert(&mut agent, tighter, None).await);
    // 개인 파일에만 표지.
    e.f.write_db("dev.local.toml", "schema = 1\nproduction = true\n");
    let plain = json!({ "connectionId": "dev", "config": {
        "name": "dev", "driver": "postgres", "host": "localhost", "database": "app" } });
    refused(&e.upsert(&mut agent, plain, None).await, "lowersMarks");
    std::fs::remove_file(e.f.db_file("dev.local.toml")).unwrap();

    // 공유 파일은 표지가 있고 사람의 개인 파일이 그것을 덮어 병합 값은 표지가 없다 — 그래도 공유 레이어의 표지가 바닥이다.
    e.f.write_db(
        "prod2.toml",
        &PROD.replace("db.prod.example", "db2.prod.example"),
    );
    e.f.write_db("prod2.local.toml", "schema = 1\nproduction = false\n");
    let list = e
        .call(&mut agent, "db.tool.connectionList", json!({}), None)
        .await
        .unwrap();
    assert!(list.to_string().contains("\"id\":\"prod2\""), "{list}");
    let lowered = json!({ "connectionId": "prod2", "location": "local", "config": {
        "name": "prod", "driver": "postgres", "host": "db2.prod.example", "database": "app", "user": "app" } });
    refused(&e.upsert(&mut agent, lowered, None).await, "lowersMarks");

    // 메인 체크아웃만 표지, 워크트리의 같은 id 는 표지 없음.
    e.f.write_db("prod.toml", PROD);
    let unmarked = PROD.replace("production = true\n", "");
    let wt = e.worktree(&[("prod.toml", &unmarked)]);
    let in_wt = json!({ "connectionId": "prod", "worktree": wt, "config": {
        "name": "prod", "driver": "postgres", "host": "db.prod.example", "database": "app", "user": "app",
        "readOnly": true } });
    refused(&e.upsert(&mut agent, in_wt, None).await, "lowersMarks");
    // 워크트리의 같은 id 가 다른 대상(표지 없음)이어도 메인 체크아웃의 그 연결이 프로덕션이면 표지 없는 쓰기는 거절한다.
    let elsewhere = unmarked.replace("db.prod.example", "staging.example");
    std::fs::write(
        std::path::Path::new(&wt).join(".pitwall/db/prod.toml"),
        &elsewhere,
    )
    .unwrap();
    let in_wt_elsewhere = json!({ "connectionId": "prod", "worktree": wt, "config": {
        "name": "prod", "driver": "postgres", "host": "staging.example", "database": "app", "user": "app" } });
    refused(
        &e.upsert(&mut agent, in_wt_elsewhere, None).await,
        "lowersMarks",
    );
}

/// 표지가 붙은 연결의 대상(호스트 · DB · 사용자 · 포트 등)을 바꾸는 쓰기는 표지를 그대로 달아도 거절한다. 표지 없는 연결은 대상을
/// 바꿀 수 있다(보통대로 묻는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_cannot_move_a_marked_connection() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    let mut agent = e.agent();
    for (field, value) in [
        ("host", json!("elsewhere.example")),
        ("database", json!("other")),
        ("user", json!("admin")),
        ("port", json!(6543)),
    ] {
        let mut config = json!({ "name": "prod", "driver": "postgres", "host": "db.prod.example",
            "database": "app", "user": "app", "production": true });
        config[field] = value;
        let moved = json!({ "connectionId": "prod", "config": config });
        let err = refused(
            &e.upsert(&mut agent, moved, None).await,
            "movesMarkedTarget",
        );
        assert!(
            err.message.contains("changes its target"),
            "{field}: {err:?}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(e.f.db_file("prod.toml")).unwrap(),
        PROD
    );
    let same = json!({ "connectionId": "prod", "config": { "name": "prod (renamed)", "driver": "postgres",
        "host": "db.prod.example", "database": "app", "user": "app", "production": true } });
    asked(&e.upsert(&mut agent, same, None).await);
    let moved_dev = json!({ "connectionId": "dev", "config": {
        "name": "dev", "driver": "postgres", "host": "other.example", "database": "app" } });
    asked(&e.upsert(&mut agent, moved_dev, None).await);
}

/// 이번 실행에서 표지가 기억된 대상을 표지 없이 가리키는 쓰기 — 새 연결이든 표지 없는 연결의 대상을 그리로 옮기든 — 는 거절한다.
/// 열쇠는 표지 기억의 것이다: MySQL · MariaDB 는 같은 서버 · 사용자의 다른 DB 도 그 대상이다. 표지를 달면 보통대로 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_onto_a_remembered_target_must_carry_its_marks() {
    let e = env();
    e.f.write_db(
        "myprod.toml",
        "schema = 1\nname = \"myprod\"\ndriver = \"mysql\"\nhost = \"my.example\"\ndatabase = \"sales\"\nuser = \"app\"\nproduction = true\n",
    );
    e.f.write_db("prod.toml", PROD);
    let mut agent = e.agent();
    // 목록이 설정을 읽어 표지를 기억한다. 그 뒤 메인의 파일이 사라져도 이번 실행의 기억은 남는다.
    e.call(&mut agent, "db.tool.connectionList", json!({}), None)
        .await
        .unwrap();
    std::fs::remove_file(e.f.db_file("prod.toml")).unwrap();
    let onto_prod = |id: &str, production: bool| {
        json!({ "connectionId": id, "config": { "name": id, "driver": "postgres", "host": "DB.prod.example",
            "database": "app", "user": "app", "production": production } })
    };
    refused(
        &e.upsert(&mut agent, onto_prod("fresh", false), None).await,
        "lowersMarks",
    );
    // 표지 없는 기존 연결을 그 대상으로 옮기기.
    refused(
        &e.upsert(&mut agent, onto_prod("dev", false), None).await,
        "lowersMarks",
    );
    asked(&e.upsert(&mut agent, onto_prod("fresh", true), None).await);
    // MySQL: 같은 서버 · 사용자의 다른 DB.
    let other_db = json!({ "connectionId": "mydev", "config": { "name": "mydev", "driver": "mysql",
        "host": "my.example", "database": "scratch", "user": "app" } });
    refused(&e.upsert(&mut agent, other_db, None).await, "lowersMarks");
    let other_user = json!({ "connectionId": "mydev", "config": { "name": "mydev", "driver": "mysql",
        "host": "my.example", "database": "scratch", "user": "reader" } });
    asked(&e.upsert(&mut agent, other_user, None).await);
    assert!(!e.f.db_file("fresh.toml").exists());
}
