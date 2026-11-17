//! M5-18 의 완료 기준(16-m5-plan.md M5-18 행): 툴 메서드 `db_execute`(`db.tool.execute`)와 프로덕션 `ask` 의 조회를 가짜
//! 드라이버로 실제 RPC 세션 너머에서 본다. 승인 경로는 세션(`pw_mcp`)의 일이라 여기서는 그 규약 — 첫 호출은 부작용 없이 계획을 실은
//! 「승인 필요」(6001), 승인 표(`pitwall/approval` 의 해시)를 실은 둘째 호출이 돈다 — 을 시험이 직접 한다. MCP 왕복 · 승인 창 ·
//! 감사 행은 데스크톱 크레이트의 `mcp_tests/db.rs`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Conn, Fixture};
use pw_core::bus::{Filter, Received, Scope, Subscription};
use pw_core::registry::plan::{APPROVAL_KEY, APPROVAL_REQUIRED, ApprovalRequired, PlanChange};
use pw_db::DbSessions;
use pw_db_driver::fake::kv::{FakeKvCall, FakeKvDriver};
use pw_db_driver::fake::{FakeCall, FakeDriver, FakeEnding, FakeResponse};
use pw_db_driver::{BoxFuture, Connected, DbDriver, DbResult};
use pw_db_types::{Capabilities, ConnectionSpec, DbError, DbErrorKind, SqlDialect};
use pw_rpc::{ConnectionScope, RpcError, Tier, code as rpc_code};
use serde_json::{Value, json};

/// 가짜 KV 드라이버를 `driver = "redis"` 자리에 끼운다.
struct AsRedis(FakeKvDriver);

impl DbDriver for AsRedis {
    fn id(&self) -> &str {
        "redis"
    }

    fn dialect(&self) -> Option<SqlDialect> {
        None
    }

    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        self.0.connect(spec)
    }
}

/// 드라이버 연결 직전에 시험의 동작을 끼우는 가짜(승인을 지난 뒤 · 연결 전 — 설정 파일을 바꿔 쓴다).
struct Hooked {
    inner: FakeDriver,
    before_connect: Hook,
}

impl DbDriver for Hooked {
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
        if let Some(hook) = self.before_connect.lock().unwrap().take() {
            hook();
        }
        self.inner.connect(spec)
    }
}

type Hook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

struct Env {
    f: Fixture,
    pg: FakeDriver,
    my: FakeDriver,
    kv: FakeKvDriver,
    hook: Hook,
    /// 시험이 실행 길에 끼워 넣는 자리(`DbSessions::test_hooks`).
    sessions: Arc<DbSessions>,
}

const DEV: &str =
    "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"localhost\"\ndatabase = \"app\"\n";
const PROD: &str = "schema = 1\nname = \"Prod\"\ndriver = \"postgres\"\nhost = \"prod.example\"\ndatabase = \"app\"\nproduction = true\n";
const RO: &str = "schema = 1\nname = \"ro\"\ndriver = \"postgres\"\nhost = \"replica\"\ndatabase = \"app\"\nread_only = true\n";
const MY: &str = "schema = 1\nname = \"my\"\ndriver = \"mysql\"\nhost = \"db.example\"\ndatabase = \"app\"\nuser = \"app\"\n";
const LITE: &str = "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n";
const KV_PROD: &str = "schema = 1\nname = \"cache\"\ndriver = \"redis\"\nhost = \"cache.example\"\nproduction = true\n";

fn env() -> Env {
    let pg = FakeDriver::named_with(
        "postgres",
        SqlDialect::Postgres,
        FakeDriver::full_capabilities(),
    );
    let my = FakeDriver::named("mysql", SqlDialect::Mysql);
    let kv = FakeKvDriver::new();
    let hook: Hook = Arc::default();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers
        .register(Arc::new(Hooked {
            inner: pg.clone(),
            before_connect: Arc::clone(&hook),
        }))
        .unwrap();
    drivers.register(Arc::new(my.clone())).unwrap();
    drivers.register(Arc::new(AsRedis(kv.clone()))).unwrap();
    drivers
        .register(Arc::new(pw_db_driver_sqlite::SqliteDriver::new()))
        .unwrap();
    let mut held = None;
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        held = Some(Arc::clone(&sessions));
        pw_db::register(router, sessions);
    });
    f.write_db("dev.toml", DEV);
    Env {
        f,
        pg,
        my,
        kv,
        hook,
        sessions: held.unwrap(),
    }
}

/// 끼워 넣는 자리에서 「닿았다」 를 알리고 「가라」 를 기다리는 동작.
fn gate_hook() -> (
    pw_db::session::TestHook,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
    let hook: pw_db::session::TestHook = Box::new(move || {
        Box::pin(async move {
            let _ = reached_tx.send(());
            let _ = go_rx.await;
        })
    });
    (hook, reached_rx, go_tx)
}

/// 시험의 동작 하나를 그 자리에서 돌리는 끼워 넣기.
fn run_hook(f: impl FnOnce() + Send + 'static) -> pw_db::session::TestHook {
    Box::new(move || {
        f();
        Box::pin(async {})
    })
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

/// 계획의 SQL 갈래.
fn sql_of(required: &ApprovalRequired) -> (Value, Option<Value>, Vec<Value>, bool) {
    let [PlanChange::Sql { .. }] = required.plan.changes.as_slice() else {
        panic!("{:?}", required.plan.changes)
    };
    let change = serde_json::to_value(&required.plan.changes[0]).unwrap();
    assert_eq!(change["kind"], "sql");
    (
        change["connection"].clone(),
        Some(change["file"].clone()).filter(|f| !f.is_null()),
        change["statements"].as_array().unwrap().clone(),
        change["irreversible"].as_bool().unwrap(),
    )
}

impl Env {
    fn agent(&self) -> Conn {
        self.f.connect(ConnectionScope::Agent)
    }

    /// 사람이 그 프로젝트 창의 탐색기에서 연결을 펼친다.
    async fn human_opens(&self, connection: &str) {
        let mut w = self.f.window();
        w.call(
            "db.schema.objects",
            json!({ "projectId": self.f.id, "connectionId": connection, "kind": "table", "limit": 500 }),
        )
        .await
        .unwrap();
    }

    async fn call(
        &self,
        agent: &mut Conn,
        method: &str,
        connection: &str,
        extra: Value,
    ) -> Result<Value, RpcError> {
        let mut params = json!({ "projectId": self.f.id, "connectionId": connection });
        if let Value::Object(extra) = extra {
            params.as_object_mut().unwrap().extend(extra);
        }
        agent.call(method, params).await
    }

    async fn execute(
        &self,
        agent: &mut Conn,
        connection: &str,
        extra: Value,
    ) -> Result<Value, RpcError> {
        self.call(agent, "db.tool.execute", connection, extra).await
    }

    /// 세션이 하는 둘째 호출: 첫 호출의 계획 해시를 승인 표로 싣는다.
    async fn approved(
        &self,
        agent: &mut Conn,
        method: &str,
        connection: &str,
        extra: Value,
        hash: &str,
    ) -> Result<Value, RpcError> {
        let mut extra = extra;
        extra
            .as_object_mut()
            .unwrap()
            .insert(APPROVAL_KEY.into(), json!({ "hash": hash }));
        self.call(agent, method, connection, extra).await
    }

    /// 묻고(첫 호출) 승인해(둘째 호출) 돌린다.
    async fn ask_and_run(
        &self,
        agent: &mut Conn,
        method: &str,
        connection: &str,
        extra: Value,
    ) -> (ApprovalRequired, Result<Value, RpcError>) {
        let required = asked(&self.call(agent, method, connection, extra.clone()).await);
        let ran = self
            .approved(agent, method, connection, extra, &required.plan.hash)
            .await;
        (required, ran)
    }

    /// 프로젝트의 사람 설정(`.pitwall/project.local.toml`) 또는 공유 설정(`project.toml`)에 `[mcp.policy] production`.
    fn production_policy(&self, file: &str, value: &str) {
        let dir = self.f.root.join(".pitwall");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(file),
            format!("schema = 1\n[mcp.policy]\nproduction = \"{value}\"\n"),
        )
        .unwrap();
    }

    fn file(&self, rel: &str, bytes: impl AsRef<[u8]>) {
        let path = self.f.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn executed(&self) -> Vec<String> {
        self.pg
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                FakeCall::Execute(sql) => Some(sql),
                _ => None,
            })
            .collect()
    }

    fn connects(&self) -> usize {
        self.pg.count(&FakeCall::Connect)
    }

    fn schema_events(&self) -> Subscription {
        self.f.bus.subscribe(
            Filter::all()
                .scope(Scope::Project(self.f.id.clone()))
                .topics(["db.schemaChanged"]),
        )
    }
}

async fn next_schema_change(events: &mut Subscription) -> Value {
    let received = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("no db.schemaChanged")
        .unwrap();
    let Received::Event(event) = received else {
        panic!("lagged")
    };
    event.payload.clone()
}

// --- 승인 · 계획 ---------------------------------------------------------------------------------------------------

/// 승인 없이는 드라이버에 아무것도 가지 않는다 — 첫 호출은 계획(실제로 돌릴 문장 전부 · 문장별 등급 · 연결 이름 · 표지)만
/// 만든다. 계획은 매번 묻는 규칙(`auto_approve` 없음 · 기억 없음)이고 등급은 `dangerous` 다. 표 없이 다시 불러도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_reaches_the_driver_without_approval() {
    let e = env();
    e.human_opens("dev").await;
    let work = || {
        e.pg.calls()
            .into_iter()
            .filter(|c| {
                matches!(
                    c,
                    FakeCall::Connect | FakeCall::Execute(_) | FakeCall::Begin { .. }
                )
            })
            .count()
    };
    let before = work();
    let mut events = e.schema_events();
    let mut agent = e.agent();
    let sql = "SELECT 1;\nUPDATE users SET name = 'x' WHERE id = 1;\nCREATE TABLE t (id int)";
    for _ in 0..2 {
        let required = asked(&e.execute(&mut agent, "dev", json!({ "sql": sql })).await);
        assert!(!required.mismatch);
        let plan = &required.plan;
        assert_eq!(
            (plan.method.as_str(), plan.tier),
            ("db.tool.execute", Tier::Dangerous)
        );
        assert!(!plan.rules.auto_approve);
        assert_eq!(
            serde_json::to_value(&plan.rules.remember).unwrap()["kind"],
            "never"
        );
        assert!(
            plan.summary.contains("3 SQL statements"),
            "{}",
            plan.summary
        );
        let (connection, file, statements, irreversible) = sql_of(&required);
        assert_eq!(
            connection,
            json!({ "name": "dev", "production": false, "readOnly": false })
        );
        assert_eq!(file, None);
        assert!(!irreversible, "PG 의 DDL 은 트랜잭션 안이다");
        let shown: Vec<(Value, Value, Value)> = statements
            .iter()
            .map(|s| (s["index"].clone(), s["line"].clone(), s["grade"].clone()))
            .collect();
        assert_eq!(
            shown,
            [
                (json!(0), json!(1), json!("read")),
                (json!(1), json!(2), json!("write")),
                (json!(2), json!(3), json!("ddl")),
            ]
        );
        assert_eq!(
            statements[1]["text"],
            "UPDATE users SET name = 'x' WHERE id = 1"
        );
    }
    assert_eq!(work(), before, "{:?}", e.pg.calls());
    assert!(
        events.try_recv().is_none(),
        "no schema change before approval"
    );
}

/// 승인 뒤에는 그 문장들이 트랜잭션 하나에서 돌고 커밋된다. 결과는 문장마다 영향 행 수(행을 내는 문장은 행).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_approval_the_statements_run_in_one_committed_transaction() {
    let e = env();
    e.pg.respond("UPDATE users", FakeResponse::write(3));
    e.pg.respond("INSERT INTO log", FakeResponse::write(1));
    e.pg.respond("SELECT count", FakeResponse::rows(1));
    e.human_opens("dev").await;
    let tool = e.connects() as u32;
    let mut agent = e.agent();
    let sql = "UPDATE users SET a = 1 WHERE id < 4;\nINSERT INTO log VALUES (1);\nSELECT count(*) FROM users";
    let (_, ran) = e
        .ask_and_run(&mut agent, "db.tool.execute", "dev", json!({ "sql": sql }))
        .await;
    let out = ran.unwrap();
    assert_eq!(out["statements"], 3);
    assert_eq!(out["transaction"], "committed");
    assert_eq!(out["error"], Value::Null);
    assert_eq!(out["irreversible"], false);
    let results = out["results"].as_array().unwrap();
    assert_eq!(results[0]["rowsAffected"], 3);
    assert_eq!(results[1]["rowsAffected"], 1);
    assert_eq!(results[2]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(results[1]["grade"], "write");
    let calls = e.pg.calls_of(tool);
    let begins: Vec<&FakeCall> = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Begin { .. }))
        .collect();
    assert_eq!(begins, [&FakeCall::Begin { read_only: false }], "{calls:?}");
    assert!(calls.contains(&FakeCall::Commit), "{calls:?}");
    assert!(!calls.contains(&FakeCall::Rollback), "{calls:?}");
    assert_eq!(calls.last(), Some(&FakeCall::Close));
    assert_eq!(
        e.pg.committed(),
        [
            "UPDATE users SET a = 1 WHERE id < 4",
            "INSERT INTO log VALUES (1)"
        ]
    );
    // 감사 상세: 문장 수 · 가장 높은 등급 · 해시 — 원문은 없다.
    let audit = out["audit"].as_str().unwrap();
    assert!(
        audit.starts_with("statements=3 grade=write sql=sha256:"),
        "{audit}"
    );
    assert!(audit.ends_with(" transaction=committed"), "{audit}");
    assert!(!audit.contains("users"), "{audit}");
}

/// 승인 뒤 `file` 내용이 바뀌면 실행하지 않는다 — 다시 만든 계획의 해시가 다르다(`mismatch`). 계획에는 경로와 내용 해시가 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_changed_after_approval_does_not_run() {
    let e = env();
    e.file("sql/fix.sql", "UPDATE users SET a = 1 WHERE id = 1;\n");
    e.human_opens("dev").await;
    let before = e.connects();
    let mut agent = e.agent();
    let input = json!({ "file": "sql/fix.sql" });
    let required = asked(&e.execute(&mut agent, "dev", input.clone()).await);
    let (_, file, statements, _) = sql_of(&required);
    let file = file.unwrap();
    assert_eq!(file["path"], "sql/fix.sql");
    assert_eq!(file["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(statements.len(), 1);
    e.file("sql/fix.sql", "DELETE FROM users;\n");
    let again = asked(
        &e.approved(
            &mut agent,
            "db.tool.execute",
            "dev",
            input,
            &required.plan.hash,
        )
        .await,
    );
    assert!(again.mismatch);
    assert_ne!(again.plan.hash, required.plan.hash);
    assert_eq!(e.connects(), before);
    assert!(e.executed().is_empty(), "{:?}", e.executed());
}

/// 승인 뒤 연결 설정의 대상이 바뀌면 실행하지 않는다 — 사람이 새 대상도 연 연결이어도 해시가 다르다. 표지(read-only · 프로덕션)를
/// 바꿔도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_whose_target_or_marks_changed_after_approval_does_not_run() {
    let e = env();
    e.production_policy("project.local.toml", "ask");
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let input = json!({ "sql": "UPDATE users SET a = 1 WHERE id = 1" });
    let required = asked(&e.execute(&mut agent, "dev", input.clone()).await);
    // 대상을 바꾼다 — 사람이 바뀐 대상도 연다(「사람이 연 연결」 은 지난다).
    e.f.edit_db("dev.toml", "host = \"localhost\"", "host = \"other\"");
    e.human_opens("dev").await;
    let before = e.connects();
    let again = asked(
        &e.approved(
            &mut agent,
            "db.tool.execute",
            "dev",
            input.clone(),
            &required.plan.hash,
        )
        .await,
    );
    assert!(again.mismatch);
    assert_eq!(e.connects(), before);
    // 프로덕션 표지를 더한다(`ask` 라 거절되지 않고 계획이 바뀐다).
    let second = again.plan.hash.clone();
    e.f.edit_db(
        "dev.toml",
        "database = \"app\"",
        "database = \"app\"\nproduction = true",
    );
    let third = asked(
        &e.approved(&mut agent, "db.tool.execute", "dev", input, &second)
            .await,
    );
    assert!(third.mismatch);
    assert_eq!(sql_of(&third).0["production"], true);
    assert_eq!(e.connects(), before);
    assert!(e.executed().is_empty(), "{:?}", e.executed());
}

/// 승인을 지난 뒤 연결을 여는 사이에 설정 파일을 바꿔 끼워도 승인한 그 대상으로 연다 — 다시 읽은 값으로 하지 않는다(그 대상이
/// 연결 직전에 다시 견준 값과 다르면 열지 않는다 — M5-06). 여기서는 드라이버 연결 직전에 바꿔, 드라이버가 받은 대상이 승인한 것인지
/// 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_swap_after_approval_runs_against_the_approved_target_only() {
    let e = env();
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let input = json!({ "sql": "UPDATE users SET a = 1 WHERE id = 1" });
    let required = asked(&e.execute(&mut agent, "dev", input.clone()).await);
    let db = e.f.db_file("dev.toml");
    *e.hook.lock().unwrap() = Some(Box::new(move || {
        let text = std::fs::read_to_string(&db).unwrap();
        std::fs::write(&db, text.replace("localhost", "evil.example")).unwrap();
    }));
    e.approved(
        &mut agent,
        "db.tool.execute",
        "dev",
        input.clone(),
        &required.plan.hash,
    )
    .await
    .unwrap();
    let spec = e.pg.connect_specs().last().cloned().unwrap();
    let target = format!("{:?}", spec.target);
    assert!(
        target.contains("localhost") && !target.contains("evil"),
        "{target}"
    );
    // 바뀐 뒤의 다음 호출은 바뀐 대상의 계획이다(그 대상은 사람이 연 적이 없어 관문이 거절한다).
    refused(
        &e.approved(
            &mut agent,
            "db.tool.execute",
            "dev",
            input,
            &required.plan.hash,
        )
        .await,
        "notOpenedByUser",
    );
}

/// 파싱 안 되는 문장 · 512 KiB 넘는 문장이 든 호출은 승인 요청이 생기지 않고 거절된다(결정 M5-D22) — 연결도 열지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unclassified_statements_are_refused_without_asking() {
    let e = env();
    e.human_opens("dev").await;
    let before = e.connects();
    let mut agent = e.agent();
    let big = format!(
        "UPDATE users SET a = 1 WHERE id = 1;\nINSERT INTO t VALUES ('{}')",
        "x".repeat(pw_db_sql::risk::MAX_STATEMENT_BYTES)
    );
    for sql in [
        "UPDATE users SET a = 1 WHERE id = 1;\nTHIS IS NOT SQL AT ALL",
        big.as_str(),
    ] {
        let err = refused(
            &e.execute(&mut agent, "dev", json!({ "sql": sql })).await,
            "unclassifiedStatement",
        );
        assert!(err.message.contains("statement 1"), "{}", err.message);
        let data = err.data.unwrap();
        assert_eq!(data["statements"][0]["index"], 1);
        assert!(data["audit"].as_str().unwrap().starts_with("statements=2"));
    }
    assert_eq!(e.connects(), before);
}

/// 트랜잭션 제어 문장은 묻기 전에 거절한다 — 호출 하나가 트랜잭션 하나다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transaction_control_is_refused_without_asking() {
    let e = env();
    e.human_opens("dev").await;
    let mut agent = e.agent();
    for sql in [
        "BEGIN;\nUPDATE users SET a = 1 WHERE id = 1",
        "UPDATE users SET a = 1 WHERE id = 1;\nCOMMIT",
        "SAVEPOINT a;\nUPDATE users SET a = 1 WHERE id = 1",
    ] {
        refused(
            &e.execute(&mut agent, "dev", json!({ "sql": sql })).await,
            "transactionStatement",
        );
    }
    assert!(e.executed().is_empty());
}

/// 둘째 문장이 실패하면 거기서 멈추고 롤백한다 — 첫 문장의 쓰기도 남지 않는다. 결과는 첫 문장과 그 오류(문장 번호 · 줄).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_second_statement_rolls_everything_back() {
    let e = env();
    e.pg.respond("UPDATE users", FakeResponse::write(2));
    e.pg.respond(
        "INSERT INTO t",
        FakeResponse::error(
            DbError::new(
                DbErrorKind::Server,
                "duplicate key value violates unique constraint",
            )
            .with_code("23505"),
        ),
    );
    e.human_opens("dev").await;
    let tool = e.connects() as u32;
    let mut agent = e.agent();
    let sql = "UPDATE users SET a = 1 WHERE id < 3;\nINSERT INTO t VALUES (1);\nDELETE FROM x WHERE id = 1";
    let (_, ran) = e
        .ask_and_run(&mut agent, "db.tool.execute", "dev", json!({ "sql": sql }))
        .await;
    let out = ran.unwrap();
    assert_eq!(out["transaction"], "rolledBack");
    assert!(
        out["audit"]
            .as_str()
            .unwrap()
            .ends_with(" transaction=rolledBack"),
        "{out}"
    );
    assert_eq!(out["results"].as_array().unwrap().len(), 1);
    assert_eq!(out["error"]["index"], 1);
    assert_eq!(out["error"]["line"], 2);
    assert_eq!(out["error"]["error"]["code"], "23505");
    let calls = e.pg.calls_of(tool);
    assert!(calls.contains(&FakeCall::Rollback), "{calls:?}");
    assert!(!calls.contains(&FakeCall::Commit), "{calls:?}");
    assert!(!e.executed().iter().any(|s| s.starts_with("DELETE")));
    assert_eq!(e.pg.committed(), Vec::<String>::new());
}

/// 트랜잭션 안에서 못 도는 문장은 푸는 입력이 없고 DB 의 오류가 그대로 나온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_statement_that_cannot_run_in_a_transaction_returns_the_database_error() {
    let e = env();
    let message = "CREATE INDEX CONCURRENTLY cannot run inside a transaction block";
    e.pg.respond(
        "CONCURRENTLY",
        FakeResponse::error(DbError::new(DbErrorKind::Server, message).with_code("25001")),
    );
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let (_, ran) = e
        .ask_and_run(
            &mut agent,
            "db.tool.execute",
            "dev",
            json!({ "sql": "CREATE INDEX CONCURRENTLY i ON users (a)" }),
        )
        .await;
    let out = ran.unwrap();
    assert_eq!(out["transaction"], "rolledBack");
    assert_eq!(out["error"]["error"]["message"], message);
    assert_eq!(out["error"]["error"]["code"], "25001");
}

/// MySQL 의 DDL(암묵 커밋)이 있으면 계획이 「되돌릴 수 없다」 를 보이고, 그 문장이 트랜잭션을 끝내면 뒤 문장을 위해 다시 연다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_implicit_commit_is_shown_as_irreversible_and_the_rest_runs_in_a_new_transaction() {
    let e = env();
    e.f.write_db("my.toml", MY);
    e.my.respond(
        "ALTER TABLE",
        FakeResponse::write(0).ending(FakeEnding::ImplicitCommit),
    );
    e.my.respond("INSERT INTO t", FakeResponse::write(1));
    e.human_opens("my").await;
    let tool = e.my.count(&FakeCall::Connect) as u32;
    let mut agent = e.agent();
    let sql =
        "INSERT INTO t VALUES (1);\nALTER TABLE t ADD COLUMN b int;\nINSERT INTO t VALUES (2)";
    let (required, ran) = e
        .ask_and_run(&mut agent, "db.tool.execute", "my", json!({ "sql": sql }))
        .await;
    assert!(sql_of(&required).3, "irreversible");
    assert!(required.plan.summary.contains("cannot be rolled back"));
    let out = ran.unwrap();
    assert_eq!(out["irreversible"], true);
    assert_eq!(out["transaction"], "committed");
    let begins =
        e.my.calls_of(tool)
            .into_iter()
            .filter(|c| matches!(c, FakeCall::Begin { read_only: false }))
            .count();
    assert_eq!(begins, 2);
    assert_eq!(
        e.my.committed(),
        [
            "INSERT INTO t VALUES (1)",
            "ALTER TABLE t ADD COLUMN b int",
            "INSERT INTO t VALUES (2)"
        ]
    );
}

/// DDL 등급이 끝나면 그 연결의 스키마 캐시를 버리고 `db.schemaChanged` 를 낸다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ddl_drops_the_schema_cache() {
    let e = env();
    e.human_opens("dev").await;
    let mut events = e.schema_events();
    let mut agent = e.agent();
    let (_, ran) = e
        .ask_and_run(
            &mut agent,
            "db.tool.execute",
            "dev",
            json!({ "sql": "UPDATE users SET a = 1 WHERE id = 1" }),
        )
        .await;
    ran.unwrap();
    assert!(
        events.try_recv().is_none(),
        "a write without DDL keeps the cache"
    );
    let (_, ran) = e
        .ask_and_run(
            &mut agent,
            "db.tool.execute",
            "dev",
            json!({ "sql": "CREATE TABLE t2 (id int)" }),
        )
        .await;
    ran.unwrap();
    let payload = next_schema_change(&mut events).await;
    assert_eq!(payload["connectionId"], "dev");
}

/// 쓰기 결과의 행(`RETURNING` 등)도 응답 예산을 읽는 그때 센다. 예산을 다 써도 뒤 문장은 돌고 커밋되며, 싣지 못한 문장은
/// `unlisted` 로 센다 — 반만 돈 쓰기를 커밋하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returning_rows_use_the_budget_and_later_writes_still_run() {
    let e = env();
    let wide = FakeResponse::rows(10_000).with_columns(
        ["id", "a", "b", "c", "d", "e", "f", "g"]
            .into_iter()
            .map(|n| pw_db_driver::fake::column(n, "text"))
            .collect(),
    );
    e.pg.respond("RETURNING", wide);
    e.pg.respond("INSERT INTO b", FakeResponse::write(1));
    e.pg.respond("INSERT INTO c", FakeResponse::write(2));
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let sql = "UPDATE a SET x = repeat('y', 4000) WHERE id > 0 RETURNING x;\nINSERT INTO b VALUES (1);\nINSERT INTO c VALUES (2)";
    let (_, ran) = e
        .ask_and_run(
            &mut agent,
            "db.tool.execute",
            "dev",
            json!({ "sql": sql, "maxRows": 10000 }),
        )
        .await;
    let out = ran.unwrap();
    assert!(
        serde_json::to_string(&out).unwrap().len() <= pw_db::script::RESPONSE_BUDGET,
        "within the budget"
    );
    assert_eq!(out["transaction"], "committed");
    let results = out["results"].as_array().unwrap();
    assert_eq!(results[0]["truncated"], true);
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(
        out["unlisted"],
        json!({ "index": 1, "line": 2, "count": 2, "rowsAffected": 3 })
    );
    assert!(
        e.pg.committed()
            .iter()
            .any(|s| s.starts_with("INSERT INTO c"))
    );
}

// --- read-only ---------------------------------------------------------------------------------------------------

/// read-only 연결은 조회 밖을 묻지 않고 거절한다. 조회만 든 글은 승인 뒤 읽기 전용 트랜잭션에서 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_connection_refuses_anything_but_queries() {
    let e = env();
    e.f.write_db("ro.toml", RO);
    e.human_opens("ro").await;
    let mut agent = e.agent();
    let before = e.connects();
    refused(
        &e.execute(
            &mut agent,
            "ro",
            json!({ "sql": "SELECT 1;\nUPDATE users SET a = 1 WHERE id = 1" }),
        )
        .await,
        "readOnlyConnection",
    );
    assert_eq!(e.connects(), before);
    let tool = e.connects() as u32;
    let (required, ran) = e
        .ask_and_run(
            &mut agent,
            "db.tool.execute",
            "ro",
            json!({ "sql": "SELECT 1" }),
        )
        .await;
    assert_eq!(sql_of(&required).0["readOnly"], true);
    assert_eq!(ran.unwrap()["transaction"], "readOnly");
    assert!(
        e.pg.calls_of(tool)
            .contains(&FakeCall::Begin { read_only: true })
    );
}

/// 사람이 read-only 로 연 연결의 설정에서 `read_only` 를 지워도 조회 밖을 거절한다(결정 M5-D37 — 표지는 조여지기만 한다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_read_only_from_the_settings_still_refuses_writes() {
    let e = env();
    e.f.write_db("ro.toml", RO);
    e.human_opens("ro").await;
    e.f.edit_db("ro.toml", "read_only = true\n", "");
    e.human_opens("ro").await;
    let mut agent = e.agent();
    refused(
        &e.execute(
            &mut agent,
            "ro",
            json!({ "sql": "UPDATE users SET a = 1 WHERE id = 1" }),
        )
        .await,
        "readOnlyConnection",
    );
}

// --- 프로덕션 ------------------------------------------------------------------------------------------------------

/// 「프로덕션 태그 연결의 판정」 표의 여덟 칸: `block`(기본)은 넷 다 거절, `ask` 는 사람이 연 연결이면 조회 · 쓰기 모두 매번
/// 승인(계획이 기억 없음 · `auto_approve` 없음), 연 적 없는 연결은 거절.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_eight_cells_of_the_production_table() {
    let query = json!({ "sql": "SELECT * FROM users" });
    let write = json!({ "sql": "UPDATE users SET a = 1 WHERE id = 1" });
    for policy in ["block", "ask"] {
        for opened in [true, false] {
            let e = env();
            e.f.write_db("prod.toml", PROD);
            if policy == "ask" {
                e.production_policy("project.local.toml", "ask");
            }
            if opened {
                e.human_opens("prod").await;
            }
            let before = e.connects();
            let mut agent = e.agent();
            for (method, input) in [("db.tool.query", &query), ("db.tool.execute", &write)] {
                let cell = format!("{policy} · opened {opened} · {method}");
                let first = e.call(&mut agent, method, "prod", input.clone()).await;
                match (policy, opened) {
                    ("block", _) => {
                        refused(&first, "productionBlocked");
                    }
                    ("ask", false) => {
                        refused(&first, "notOpenedByUser");
                    }
                    _ => {
                        let required = asked(&first);
                        assert!(!required.plan.rules.auto_approve, "{cell}");
                        assert_eq!(
                            serde_json::to_value(&required.plan.rules.remember).unwrap()["kind"],
                            "never",
                            "{cell}"
                        );
                        assert_eq!(sql_of(&required).0["production"], true, "{cell}");
                        assert!(required.plan.summary.contains("production"), "{cell}");
                        let ran = e
                            .approved(
                                &mut agent,
                                method,
                                "prod",
                                input.clone(),
                                &required.plan.hash,
                            )
                            .await;
                        ran.unwrap_or_else(|err| panic!("{cell}: {err:?}"));
                        // 매번: 승인 표 없이 다시 부르면 다시 묻는다.
                        asked(&e.call(&mut agent, method, "prod", input.clone()).await);
                    }
                }
            }
            if policy == "block" || !opened {
                assert_eq!(e.connects(), before, "{policy} · {opened}");
            }
        }
    }
}

/// 공유 `project.toml` 의 `production = "ask"` 는 효과가 없고 경고가 남는다 — `.local.toml` 의 `ask` 는 먹는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_production_ask_has_no_effect_and_a_local_one_does() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    e.human_opens("prod").await;
    e.production_policy("project.toml", "ask");
    let mut agent = e.agent();
    let query = json!({ "sql": "SELECT 1" });
    refused(
        &e.call(&mut agent, "db.tool.query", "prod", query.clone())
            .await,
        "productionBlocked",
    );
    let project = e.f.projects.get(&e.f.id).unwrap();
    let (policy, warnings) =
        pw_core::config::mcp_policy::read_project_over(&e.f.app, None, &project);
    assert_eq!(
        policy.production,
        pw_core::config::mcp_policy::Production::Block
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("mcp.policy in the shared .pitwall/project.toml has no effect")),
        "{warnings:?}"
    );
    e.production_policy("project.local.toml", "ask");
    asked(&e.call(&mut agent, "db.tool.query", "prod", query).await);
}

/// 비프로덕션 연결의 조회는 `ask` 여도 승인을 묻지 않고 바로 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queries_on_non_production_connections_do_not_ask() {
    let e = env();
    e.production_policy("project.local.toml", "ask");
    e.pg.respond("FROM users", FakeResponse::rows(2));
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let out = e
        .call(
            &mut agent,
            "db.tool.query",
            "dev",
            json!({ "sql": "SELECT * FROM users" }),
        )
        .await
        .unwrap();
    assert_eq!(out["results"][0]["rows"].as_array().unwrap().len(), 2);
    e.call(&mut agent, "db.tool.schema", "dev", json!({}))
        .await
        .unwrap();
}

/// `ask` 면 `db_schema` 도 프로덕션 연결에 매번 묻고, 승인 전에는 메타데이터용 연결을 열지 않는다. 승인 뒤에는 읽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_schema_asks_for_a_production_connection_under_ask() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    e.production_policy("project.local.toml", "ask");
    e.human_opens("prod").await;
    let mut agent = e.agent();
    let metadata = || {
        e.pg.calls()
            .into_iter()
            .filter(|c| matches!(c, FakeCall::Connect | FakeCall::Metadata(_)))
            .count()
    };
    let before = metadata();
    let required = asked(
        &e.call(
            &mut agent,
            "db.tool.schema",
            "prod",
            json!({ "kind": "table" }),
        )
        .await,
    );
    assert_eq!(required.plan.method, "db.tool.schema");
    assert_eq!(required.plan.tier, Tier::Read);
    assert!(required.plan.changes.is_empty());
    assert!(
        required.plan.summary.contains("`Prod`"),
        "{}",
        required.plan.summary
    );
    assert_eq!(metadata(), before, "nothing before approval");
    e.approved(
        &mut agent,
        "db.tool.schema",
        "prod",
        json!({ "kind": "table" }),
        &required.plan.hash,
    )
    .await
    .unwrap();
}

/// `ask` 면 Redis 조회 툴도 프로덕션 연결에 매번 묻는다(승인 전에는 연결을 열지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redis_reads_ask_for_a_production_server_under_ask() {
    let e = env();
    e.f.write_db("cache.toml", KV_PROD);
    e.production_policy("project.local.toml", "ask");
    let mut w = e.f.window();
    w.call(
        "db.session.open",
        json!({ "projectId": e.f.id, "connectionId": "cache" }),
    )
    .await
    .unwrap();
    let mut agent = e.agent();
    let kv_connects = || {
        e.kv.calls()
            .iter()
            .filter(|c| matches!(c, FakeKvCall::Connect { .. }))
            .count()
    };
    let first = kv_connects();
    for (method, input) in [
        ("db.tool.kvScan", json!({})),
        ("db.tool.kvGet", json!({ "key": "k" })),
    ] {
        let connects = kv_connects();
        let required = asked(&e.call(&mut agent, method, "cache", input.clone()).await);
        assert_eq!(required.plan.method, method);
        assert_eq!(kv_connects(), connects, "nothing before approval");
        let _ = e
            .approved(&mut agent, method, "cache", input, &required.plan.hash)
            .await;
        assert_eq!(kv_connects(), connects + 1, "the approved call connects");
    }
    assert_eq!(kv_connects(), first + 2);
}

/// `db_connection_list` 가 정책을 보인다: `block` 이면 `productionBlocked`, `ask` 면 쓸 수 있고 `asksApproval`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_connection_list_shows_the_production_policy() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    e.human_opens("prod").await;
    let mut agent = e.agent();
    let prod = |list: &Value| {
        list["connections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "prod")
            .cloned()
            .unwrap()
    };
    let list = agent
        .call("db.tool.connectionList", json!({ "projectId": e.f.id }))
        .await
        .unwrap();
    let c = prod(&list);
    assert_eq!(
        (
            c["usable"].clone(),
            c["unusableReason"].clone(),
            c["asksApproval"].clone()
        ),
        (json!(false), json!("productionBlocked"), json!(false))
    );
    e.production_policy("project.local.toml", "ask");
    let list = agent
        .call("db.tool.connectionList", json!({ "projectId": e.f.id }))
        .await
        .unwrap();
    let c = prod(&list);
    assert_eq!(
        (
            c["usable"].clone(),
            c["unusableReason"].clone(),
            c["asksApproval"].clone()
        ),
        (json!(true), Value::Null, json!(true))
    );
    let dev = list["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "dev")
        .unwrap();
    assert_eq!(dev["asksApproval"], false);
}

/// 실 SQLite 드라이버로 `INSERT` 왕복과 롤백(SQLite 는 서버가 없어 live 대신 여기서 본다): 커밋된 쓰기는 다음 조회가 보고, 둘째
/// 문장이 실패하면 첫 문장의 `INSERT` 도 남지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_inserts_and_rolls_back_for_real() {
    let e = env();
    e.file("app.db", "");
    e.f.write_db("lite.toml", LITE);
    e.human_opens("lite").await;
    let mut agent = e.agent();
    let (_, ran) = e
        .ask_and_run(
            &mut agent,
            "db.tool.execute",
            "lite",
            json!({ "sql": "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);\nINSERT INTO t VALUES (1, 'a'), (2, 'b')" }),
        )
        .await;
    let out = ran.unwrap();
    assert_eq!(out["transaction"], "committed", "{out}");
    assert_eq!(out["results"][1]["rowsAffected"], 2, "{out}");
    let (_, ran) = e
        .ask_and_run(
            &mut agent,
            "db.tool.execute",
            "lite",
            json!({ "sql": "INSERT INTO t VALUES (3, 'c');\nINSERT INTO t VALUES (1, 'dup')" }),
        )
        .await;
    let out = ran.unwrap();
    assert_eq!(out["transaction"], "rolledBack", "{out}");
    assert_eq!(out["error"]["index"], 1, "{out}");
    let count = e
        .call(
            &mut agent,
            "db.tool.query",
            "lite",
            json!({ "sql": "SELECT count(*) FROM t" }),
        )
        .await
        .unwrap();
    assert_eq!(
        count["results"][0]["rows"][0][0],
        json!({ "int": 2 }),
        "{count}"
    );
}

// --- 리뷰 회차(M5-18 리뷰 1 ~ 5) -----------------------------------------------------------------------------------------

/// 승인 뒤 · 연결 전에 연결 설정의 대상을 바꿔 끼우면 돌지 않는다 — 연결을 여는 자리가 다시 푼 대상을 판정한 것과 견준다(그 틈은
/// 승인 해시가 덮지 못한다: 해시는 판정 때의 값이다). 에이전트는 큰 SQL(분류 · 계획에 시간이 드는 글)로 그 틈을 늘리고 그동안
/// 자기 워크트리의 연결 설정을 고칠 수 있다 — 시험은 그 틈을 끼워 넣는 자리로 결정적으로 만든다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_target_swapped_between_approval_and_connect_does_not_run() {
    let e = env();
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let big: String = (0..2_000)
        .map(|i| format!("UPDATE users SET a = {i} WHERE id = {i};\n"))
        .collect();
    let input = json!({ "sql": big });
    let required = asked(&e.execute(&mut agent, "dev", input.clone()).await);
    let connects = e.connects();
    let db = e.f.db_file("dev.toml");
    e.sessions.test_hooks.set_before_resolve(run_hook(move || {
        let text = std::fs::read_to_string(&db).unwrap();
        std::fs::write(&db, text.replace("localhost", "evil.example")).unwrap();
    }));
    refused(
        &e.approved(
            &mut agent,
            "db.tool.execute",
            "dev",
            input,
            &required.plan.hash,
        )
        .await,
        "notOpenedByUser",
    );
    assert_eq!(e.connects(), connects, "no connection at all");
    assert!(e.executed().is_empty(), "{:?}", e.executed());
    assert!(
        e.pg.connect_specs()
            .iter()
            .all(|s| !format!("{:?}", s.target).contains("evil")),
        "nothing went to the swapped-in target"
    );
}

/// 승인 뒤 · 연결 전에 표지를 더해도 돌지 않는다 — 프로덕션(기본 `block`)이면 `productionBlocked`, read-only 면
/// `readOnlyConnection`. 표지를 더하는 쪽은 곧바로 따른다(결정 M5-D37).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marks_added_between_approval_and_connect_stop_the_call() {
    for (mark, why) in [
        ("production = true", "productionBlocked"),
        ("read_only = true", "readOnlyConnection"),
    ] {
        let e = env();
        e.human_opens("dev").await;
        let mut agent = e.agent();
        let input = json!({ "sql": "UPDATE users SET a = 1 WHERE id = 1" });
        let required = asked(&e.execute(&mut agent, "dev", input.clone()).await);
        let connects = e.connects();
        let db = e.f.db_file("dev.toml");
        let line = format!("{mark}\n");
        e.sessions.test_hooks.set_before_resolve(run_hook(move || {
            let text = std::fs::read_to_string(&db).unwrap();
            std::fs::write(&db, format!("{text}{line}")).unwrap();
        }));
        refused(
            &e.approved(
                &mut agent,
                "db.tool.execute",
                "dev",
                input,
                &required.plan.hash,
            )
            .await,
            why,
        );
        assert_eq!(e.connects(), connects, "{mark}");
        assert!(e.executed().is_empty(), "{mark}: {:?}", e.executed());
    }
}

/// 분류기가 못 읽은 읽기(MySQL — 뒤의 읽기에서만 보이는 `INTO @변수`)가 든 글은 묻지 않고 거절한다 — 그 문장은 계획에 보일 등급과
/// 서버가 하는 일이 갈린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_statement_with_an_unread_reading_is_refused_without_asking() {
    let e = env();
    e.f.write_db("my.toml", MY);
    e.human_opens("my").await;
    let connects = e.my.count(&FakeCall::Connect);
    let mut agent = e.agent();
    let err = refused(
        &e.execute(
            &mut agent,
            "my",
            json!({ "sql": r"UPDATE t SET a = 'x\' INTO @v -- ' WHERE id = 1" }),
        )
        .await,
        "unclassifiedStatement",
    );
    assert_eq!(err.data.unwrap()["statements"][0]["index"], 0);
    assert_eq!(e.my.count(&FakeCall::Connect), connects);
}

/// 중첩이 분류기의 깊이 상한을 넘는 문장(`tooDeep`)은 묻지 않고 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_statement_nested_too_deep_is_refused_without_asking() {
    let e = env();
    e.human_opens("dev").await;
    let connects = e.connects();
    let mut agent = e.agent();
    let deep = format!(
        "UPDATE users SET a = 1 WHERE id = 1;\nSELECT {}1",
        "NOT ".repeat(1_000)
    );
    let err = refused(
        &e.execute(&mut agent, "dev", json!({ "sql": deep })).await,
        "unclassifiedStatement",
    );
    let data = err.data.unwrap();
    assert_eq!(data["statements"][0]["index"], 1);
    assert_eq!(data["statements"][0]["reasons"], json!(["tooDeep"]));
    assert_eq!(e.connects(), connects);
}

/// 세션의 글 읽기를 바꾸는 문장(MySQL `sql_mode` 의 `ANSI_QUOTES`)이 있으면 묻지 않고 거절한다 — 그 뒤 문장을 분류기와 서버가
/// 다르게 읽어, 계획에 보인 등급과 도는 것이 갈린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_statement_that_changes_how_sql_is_read_is_refused_without_asking() {
    let e = env();
    e.f.write_db("my.toml", MY);
    e.human_opens("my").await;
    let connects = e.my.count(&FakeCall::Connect);
    let mut agent = e.agent();
    let err = refused(
        &e.execute(
            &mut agent,
            "my",
            json!({ "sql": "SET sql_mode = 'ANSI_QUOTES';\nUPDATE t SET a = 1 WHERE id = 1" }),
        )
        .await,
        "changesSqlReading",
    );
    assert_eq!(err.data.unwrap()["statements"][0]["index"], 0);
    assert_eq!(e.my.count(&FakeCall::Connect), connects);
}

/// MySQL 의 `CALL`(프로시저 안에서 커밋할 수 있다 — `may_end`)도 계획에 「되돌릴 수 없다」 를 보이고, 그 문장이 트랜잭션을 끝내면
/// 뒤 문장을 위해 다시 연다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_that_may_commit_is_irreversible_and_the_rest_runs_in_a_new_transaction() {
    let e = env();
    e.f.write_db("my.toml", MY);
    e.my.respond(
        "CALL p",
        FakeResponse::write(0).ending(FakeEnding::ImplicitCommit),
    );
    e.my.respond("INSERT INTO t", FakeResponse::write(1));
    e.human_opens("my").await;
    let tool = e.my.count(&FakeCall::Connect) as u32;
    let mut agent = e.agent();
    let sql = "INSERT INTO t VALUES (1);\nCALL p();\nINSERT INTO t VALUES (2)";
    let (required, ran) = e
        .ask_and_run(&mut agent, "db.tool.execute", "my", json!({ "sql": sql }))
        .await;
    assert!(sql_of(&required).3, "irreversible");
    let out = ran.unwrap();
    assert_eq!(out["irreversible"], true);
    assert_eq!(out["transaction"], "committed");
    let begins =
        e.my.calls_of(tool)
            .into_iter()
            .filter(|c| matches!(c, FakeCall::Begin { read_only: false }))
            .count();
    assert_eq!(begins, 2);
}

/// 커밋이 실패하면 결과는 오류다(`committed` 가 아니다) — DB 오류 그대로와 감사 상세를 싣는다. 쓰기는 남지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_commit_is_an_error_not_a_commit() {
    let e = env();
    e.pg.fail_commit(Some(
        DbError::new(
            DbErrorKind::Server,
            "insert or update violates a deferred foreign key constraint",
        )
        .with_code("23503"),
    ));
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let (_, ran) = e
        .ask_and_run(
            &mut agent,
            "db.tool.execute",
            "dev",
            json!({ "sql": "INSERT INTO child VALUES (1, 99)" }),
        )
        .await;
    let err = ran.expect_err("a failed commit is an error");
    assert_eq!(err.code, pw_db::gate::code::DB_ERROR, "{err:?}");
    let data = err.data.unwrap();
    assert_eq!(data["error"]["code"], "23503", "{data}");
    assert!(
        data["audit"]
            .as_str()
            .unwrap()
            .starts_with("statements=1 grade=write"),
        "{data}"
    );
    assert_eq!(e.pg.committed(), Vec::<String>::new());
}

/// 끼워 넣는 자리에서 호출을 취소한 뒤 실행 태스크를 보내고, 그 호출의 응답을 돌려준다. 태스크가 연결을 닫을 때까지 기다린다.
/// 커밋 전 자리면 취소 응답(Host 가 핸들러를 버렸다는 증거)을 받은 뒤 보낸다. 커밋하기로 정한 뒤의 자리면 태스크가 그 자리에 묶인
/// 동안 응답이 나가지 않는 것을 본 뒤 보낸다 — 표지 뒤라 핸들러가 버려지지 않는다.
async fn cancel_at(
    e: &Env,
    agent: &mut Conn,
    input: Value,
    hash: &str,
    at_commit_point: bool,
    tool: u32,
) -> Result<Value, pw_rpc::RpcError> {
    let (hook, reached, go) = gate_hook();
    if at_commit_point {
        e.sessions.test_hooks.set_after_commit_point(hook);
    } else {
        e.sessions.test_hooks.set_before_commit(hook);
    }
    let mut params = json!({ "projectId": e.f.id, "connectionId": "dev" });
    params
        .as_object_mut()
        .unwrap()
        .extend(input.as_object().unwrap().clone());
    params[APPROVAL_KEY] = json!({ "hash": hash });
    agent.session.handle_value(json!({
        "jsonrpc": "2.0", "id": 900, "method": "db.tool.execute", "params": params,
    }));
    tokio::time::timeout(Duration::from_secs(10), reached)
        .await
        .expect("the run reached the hook")
        .unwrap();
    agent.session.handle_value(json!({
        "jsonrpc": "2.0", "method": "request.cancel", "params": { "id": 900 },
    }));
    let answer = if at_commit_point {
        assert!(
            tokio::time::timeout(Duration::from_millis(200), agent.response(900))
                .await
                .is_err(),
            "the host answered while the commit was still pending"
        );
        go.send(()).unwrap();
        agent.response(900).await
    } else {
        // 취소 응답은 Host 가 핸들러를 버린 뒤에 온다.
        let answer = agent.response(900).await;
        go.send(()).unwrap();
        answer
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !e.pg.calls_of(tool).contains(&FakeCall::Close) {
        assert!(tokio::time::Instant::now() < deadline, "{:?}", e.pg.calls());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    answer
}

/// 마지막 문장 뒤 · 커밋 전에 관측된 취소는 롤백한다 — 에이전트가 「취소」 를 받은 호출의 쓰기는 남지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_seen_before_the_commit_rolls_back() {
    let e = env();
    e.pg.respond("UPDATE users", FakeResponse::write(1));
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let input = json!({ "sql": "UPDATE users SET a = 1 WHERE id = 1" });
    let required = asked(&e.execute(&mut agent, "dev", input.clone()).await);
    let tool = e.connects() as u32;
    let answer = cancel_at(&e, &mut agent, input, &required.plan.hash, false, tool).await;
    assert_eq!(answer.expect_err("cancelled").code, pw_rpc::code::CANCELLED);
    let calls = e.pg.calls_of(tool);
    assert!(calls.contains(&FakeCall::Rollback), "{calls:?}");
    assert!(!calls.contains(&FakeCall::Commit), "{calls:?}");
    assert_eq!(e.pg.committed(), Vec::<String>::new());
}

/// 커밋하기로 정한 뒤의 취소는 커밋을 끊지 않는다 — 인터럽트도 보내지 않는다. 그 자리에서 RPC 의 「되돌릴 수 없는 지점」 을
/// 세우므로 Host 는 핸들러를 버리지 않고 커밋 결과(`committed`)로 답한다(16-m5-plan M5-18b — 감사 행은 MCP 세션이 그 답으로 끝낸다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_after_the_commit_point_does_not_stop_the_commit() {
    let e = env();
    e.pg.respond("UPDATE users", FakeResponse::write(1));
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let input = json!({ "sql": "UPDATE users SET a = 1 WHERE id = 1" });
    let required = asked(&e.execute(&mut agent, "dev", input.clone()).await);
    let tool = e.connects() as u32;
    let answer = cancel_at(&e, &mut agent, input, &required.plan.hash, true, tool)
        .await
        .expect("the commit is the answer, not a cancel");
    assert_eq!(answer["transaction"], "committed", "{answer}");
    assert_eq!(answer["results"][0]["rowsAffected"], 1, "{answer}");
    assert!(
        answer["audit"]
            .as_str()
            .unwrap()
            .ends_with(" transaction=committed"),
        "{answer}"
    );
    let calls = e.pg.calls_of(tool);
    assert!(calls.contains(&FakeCall::Commit), "{calls:?}");
    assert!(!calls.contains(&FakeCall::Interrupt), "{calls:?}");
    assert_eq!(e.pg.committed(), ["UPDATE users SET a = 1 WHERE id = 1"]);
}
