//! M5-07 의 완료 기준(16-m5-plan.md M5-07 행): 툴 메서드 `db_query`(`db.tool.query`)와 실행 파이프라인(`pw_db::script`)을 가짜
//! 드라이버 · 실 SQLite 로 실제 RPC 세션 너머에서 본다. MCP 세션은 그 연결 범위(`ConnectionScope::Agent`)로 붙인 RPC 연결이다 —
//! MCP 왕복과 감사 행은 데스크톱 크레이트의 `mcp_tests/db.rs`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Conn, Fixture, fake_worktree};
use pw_db::DbSessions;
use pw_db::script::{MAX_FILE_BYTES, RESPONSE_BUDGET, TOOL_CELL_BYTES};
use pw_db_driver::fake::kv::FakeKvDriver;
use pw_db_driver::fake::{FakeCall, FakeDriver, FakeResponse};
use pw_db_driver::{BoxFuture, Connected, DbDriver, DbResult};
use pw_db_driver_sqlite::SqliteDriver;
use pw_db_types::{Capabilities, ConnectionSpec, DbError, DbErrorKind, SqlDialect};
use pw_rpc::protocol::StreamOutcome;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
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

/// 연결을 열기 직전에 시험의 동작을 끼우는 가짜(분류 뒤 · 실행 전 — 파일을 바꿔 쓴다).
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
    hook: Hook,
}

const DEV: &str =
    "schema = 1\nname = \"dev\"\ndriver = \"postgres\"\nhost = \"localhost\"\ndatabase = \"app\"\n";
const LITE: &str = "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n";
const MY_DEV: &str = "schema = 1\nname = \"my dev\"\ndriver = \"mysql\"\nhost = \"db.example\"\ndatabase = \"app_dev\"\nuser = \"app\"\n";
/// [`MY_DEV`] 와 같은 서버 · 사용자의 다른 DB — 프로덕션.
const MY_PROD: &str = "schema = 1\nname = \"my prod\"\ndriver = \"mysql\"\nhost = \"db.example\"\ndatabase = \"app\"\nuser = \"app\"\nproduction = true\n";

fn env() -> Env {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let my = FakeDriver::named("mysql", SqlDialect::Mysql);
    let hook: Hook = Arc::default();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers
        .register(Arc::new(Hooked {
            inner: pg.clone(),
            before_connect: Arc::clone(&hook),
        }))
        .unwrap();
    drivers.register(Arc::new(my.clone())).unwrap();
    drivers.register(Arc::new(SqliteDriver::new())).unwrap();
    drivers
        .register(Arc::new(AsRedis(FakeKvDriver::new())))
        .unwrap();
    let f = Fixture::build(true, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    f.write_db("dev.toml", DEV);
    Env { f, pg, my, hook }
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

#[track_caller]
fn invalid(result: &Result<Value, RpcError>, needle: &str) {
    let e = result.clone().expect_err("invalid");
    assert_eq!(e.code, rpc_code::INVALID_PARAMS, "{e:?}");
    let text = format!("{} {:?}", e.message, e.data);
    assert!(text.contains(needle), "{needle:?} not in {text}");
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

    async fn query(
        &self,
        agent: &mut Conn,
        connection: &str,
        extra: Value,
    ) -> Result<Value, RpcError> {
        let mut params = json!({ "projectId": self.f.id, "connectionId": connection });
        if let Value::Object(extra) = extra {
            params.as_object_mut().unwrap().extend(extra);
        }
        agent.call("db.tool.query", params).await
    }

    /// 프로젝트 루트에 파일을 쓴다.
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
}

/// 조회 두 문장 → 결과 둘이고, 새 연결의 읽기 전용 트랜잭션 하나에서 돈 뒤 롤백하고 닫는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_queries_give_two_results_in_one_read_only_transaction() {
    let e = env();
    e.pg.respond("FROM users", FakeResponse::rows(3));
    e.pg.respond("FROM orders", FakeResponse::rows(2));
    e.human_opens("dev").await;
    let tool = e.connects() as u32;
    let mut agent = e.agent();
    let out = e
        .query(
            &mut agent,
            "dev",
            json!({ "sql": "SELECT * FROM users;\nSELECT * FROM orders" }),
        )
        .await
        .unwrap();
    assert_eq!(out["statements"], 2);
    assert_eq!(out["error"], Value::Null);
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        (
            results[0]["index"].clone(),
            results[0]["line"].clone(),
            results[0]["rows"].as_array().unwrap().len()
        ),
        (json!(0), json!(1), 3)
    );
    assert_eq!(
        (
            results[1]["index"].clone(),
            results[1]["line"].clone(),
            results[1]["rows"].as_array().unwrap().len()
        ),
        (json!(1), json!(2), 2)
    );
    assert_eq!(results[0]["columns"][0]["name"], "id");
    assert_eq!(results[0]["truncated"], false);
    let calls = e.pg.calls_of(tool);
    let begins: Vec<&FakeCall> = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Begin { .. }))
        .collect();
    assert_eq!(begins, [&FakeCall::Begin { read_only: true }], "{calls:?}");
    let executes = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Execute(_)))
        .count();
    assert_eq!(executes, 2);
    assert!(!calls.contains(&FakeCall::Commit));
    assert!(calls.contains(&FakeCall::Rollback));
    assert_eq!(calls.last(), Some(&FakeCall::Close));
    assert_eq!(e.pg.committed(), Vec::<String>::new());
}

/// 조회가 아닌 문장이 하나라도 있으면 드라이버에 아무것도 가지 않고 거절한다 — 연결도 열지 않는다. 까닭에 문장 번호와 등급,
/// 「쓰기는 db_execute」.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_among_queries_refuses_everything_before_the_driver() {
    let e = env();
    e.human_opens("dev").await;
    let before = (e.connects(), e.executed().len());
    let mut agent = e.agent();
    let cases = [
        (
            "SELECT 1;\nUPDATE users SET name = 'x' WHERE id = 1",
            1,
            "write",
        ),
        ("SELECT 1; DELETE FROM users", 1, "unboundedWrite"),
        ("DROP TABLE users", 0, "destructive"),
        // 읽기 전용 트랜잭션을 푸는 첫 문장(14-m3-plan.md 결정 M3-D6)도 조회가 아니다.
        ("SET TRANSACTION READ WRITE; SELECT 1", 0, "write"),
        ("BEGIN; SELECT 1", 0, "write"),
        ("SELECT 1; COMMIT", 1, "write"),
        // 부작용 함수를 부르는 조회(분류기의 표 — `nextval` · `pg_sleep` …)도 조회가 아니다.
        ("SELECT nextval('ids')", 0, "write"),
        ("SELECT pg_sleep(600)", 0, "write"),
    ];
    for (sql, index, grade) in cases {
        let result = e.query(&mut agent, "dev", json!({ "sql": sql })).await;
        let error = refused(&result, "notAQuery");
        let data = error.data.unwrap();
        assert_eq!(data["statements"][0]["index"], index, "{sql}");
        assert_eq!(data["statements"][0]["grade"], grade, "{sql}");
        assert!(error.message.contains("db_execute"), "{}", error.message);
        assert!(
            error.message.contains(&format!("statement {index}")),
            "{}",
            error.message
        );
        assert!(
            data["audit"].as_str().unwrap().starts_with("statements="),
            "{data}"
        );
    }
    assert_eq!((e.connects(), e.executed().len()), before);
}

/// 파싱 안 되는 글과 512 KiB 넘는 문장(`tooLarge`)은 조회가 아니다(결정 M5-D22 — 승인으로도 돌리지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unparsed_and_too_large_statements_are_refused() {
    let e = env();
    e.human_opens("dev").await;
    let before = e.connects();
    let mut agent = e.agent();
    let huge = format!(
        "SELECT * FROM users WHERE name IN ({})",
        vec!["'aaaaaaaa'"; 60_000].join(",")
    );
    assert!(huge.len() > pw_db_sql::risk::MAX_STATEMENT_BYTES);
    for (sql, why) in [
        ("SELEC * FROMM users WHERE", "unparsed"),
        (huge.as_str(), "tooLarge"),
    ] {
        let result = e.query(&mut agent, "dev", json!({ "sql": sql })).await;
        let error = refused(&result, "notAQuery");
        let class = &error.data.unwrap()["statements"][0];
        assert_eq!(class["grade"], "destructive", "{why}");
        assert!(
            class["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == why),
            "{why}: {class}"
        );
    }
    assert_eq!(e.connects(), before);
}

/// SQLite `ATTACH` 는 다른 파일을 붙여 읽는 길이다 — 분류가 쓰기라 `db_query` 가 거절한다(표지의 열쇠는 파일이다, 결정 M5-D37).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_attach_is_not_a_query() {
    let e = env();
    std::fs::write(e.f.root.join("app.db"), "").unwrap();
    std::fs::write(e.f.base.join("prod.db"), "").unwrap();
    e.f.write_db("lite.toml", LITE);
    e.human_opens("lite").await;
    let mut agent = e.agent();
    let prod = e.f.base.join("prod.db");
    for sql in [
        format!(
            "ATTACH DATABASE '{}' AS prod; SELECT * FROM prod.sqlite_master",
            prod.display()
        ),
        format!("attach '{}' as prod", prod.display()),
        format!("/* x */ ATTACH\n'{}' AS \"p\"", prod.display()),
        "DETACH DATABASE main".to_owned(),
        "PRAGMA query_only = 0; SELECT 1".to_owned(),
    ] {
        let result = e.query(&mut agent, "lite", json!({ "sql": sql })).await;
        refused(&result, "notAQuery");
    }
    // 실제 SQLite 로 조회는 돈다.
    let out = e
        .query(
            &mut agent,
            "lite",
            json!({ "sql": "SELECT 1 AS one; SELECT 'a' || 'b' AS ab" }),
        )
        .await
        .unwrap();
    assert_eq!(out["results"][1]["rows"][0][0], json!({ "text": "ab" }));
}

/// 실 SQLite 로: 조회 문장들이 차례로 돌고, 셋째 문장이 실패하면 거기서 멈추고 앞의 결과 둘과 오류(문장 번호 · 줄 · 서버 오류)를
/// 준다. (분류를 지난 부작용 함수가 DB 수준 읽기 전용에 막히는 것은 실 PG · MySQL 의 live 시험 몫이다 — 보고서 「확인하지 못한 것」.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_runs_inside_a_read_only_transaction() {
    let e = env();
    let db = e.f.root.join("app.db");
    std::fs::write(&db, "").unwrap();
    e.f.write_db("lite.toml", LITE);
    let mut w = e.f.window();
    let opened = w
        .call(
            "db.session.open",
            json!({ "projectId": e.f.id, "connectionId": "lite" }),
        )
        .await
        .unwrap();
    let s = opened["session"]["sessionId"].as_str().unwrap().to_owned();
    run_confirmed(
        &mut w,
        &s,
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)",
    )
    .await;
    let mut agent = e.agent();
    let out = e
        .query(
            &mut agent,
            "lite",
            json!({ "sql": "SELECT count(*) FROM t; SELECT * FROM t; SELECT nosuchcolumn FROM t" }),
        )
        .await
        .unwrap();
    assert_eq!(out["results"].as_array().unwrap().len(), 2, "{out}");
    // 둘째 문장 다음의 실패: 그때까지의 결과와 오류(문장 번호 · 줄).
    assert_eq!(out["error"]["index"], 2);
    assert_eq!(out["error"]["line"], 1);
    assert_eq!(out["error"]["error"]["kind"], "server");
}

/// `file` 은 프로젝트 루트 안의 UTF-8 일반 파일, 5 MiB 까지. 루트 밖 · 루트 밖을 가리키는 심링크 · 5 MiB + 1 바이트 · UTF-8 아님 ·
/// FIFO · 폴더 · 없는 파일은 거절이고 드라이버에 닿지 않는다. `sql` 과 `file` 은 하나만.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_input_reads_only_regular_utf8_files_inside_the_root() {
    let e = env();
    e.human_opens("dev").await;
    let before = e.connects();
    let mut agent = e.agent();
    std::fs::write(e.f.base.join("outside.sql"), "SELECT 1").unwrap();
    e.file("q/ok.sql", "SELECT 1");
    #[cfg(unix)]
    std::os::unix::fs::symlink(e.f.base.join("outside.sql"), e.f.root.join("q/link.sql")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(e.f.base.as_path(), e.f.root.join("up")).unwrap();
    let mut big = b"SELECT 1;".to_vec();
    big.resize(MAX_FILE_BYTES as usize + 1, b' ');
    e.file("q/big.sql", &big);
    e.file("q/latin1.sql", b"SELECT '\xe9t\xe9'");
    #[cfg(unix)]
    {
        let fifo = e.f.root.join("q/fifo.sql");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let outside = e.f.base.join("outside.sql").display().to_string();
    let mut cases: Vec<(String, &str)> = vec![
        ("../outside.sql".into(), "outside the project roots"),
        (outside, "outside the project roots"),
        ("q/big.sql".into(), "larger than"),
        ("q/latin1.sql".into(), "not UTF-8"),
        ("q".into(), "is a folder"),
    ];
    #[cfg(unix)]
    {
        cases.push(("q/link.sql".into(), "symlink that points outside"));
        cases.push(("up/outside.sql".into(), "symlink that points outside"));
        cases.push(("q/fifo.sql".into(), "not a regular file"));
    }
    for (file, needle) in &cases {
        let result = e.query(&mut agent, "dev", json!({ "file": file })).await;
        invalid(&result, needle);
    }
    let missing = e
        .query(&mut agent, "dev", json!({ "file": "q/none.sql" }))
        .await;
    assert_eq!(missing.unwrap_err().code, rpc_code::NOT_FOUND);
    invalid(
        &e.query(
            &mut agent,
            "dev",
            json!({ "file": "q/ok.sql", "sql": "SELECT 1" }),
        )
        .await,
        "not both",
    );
    invalid(
        &e.query(&mut agent, "dev", json!({})).await,
        "give `sql` or `file`",
    );
    assert_eq!(e.connects(), before, "nothing reached the driver");

    // 루트 기준 상대 경로 · 루트 안 절대 경로 · BOM 은 된다.
    e.file("q/bom.sql", b"\xef\xbb\xbfSELECT 2");
    let abs = e.f.root.join("q/ok.sql").display().to_string();
    for file in ["q/ok.sql", abs.as_str(), "q/bom.sql"] {
        let out = e
            .query(&mut agent, "dev", json!({ "file": file }))
            .await
            .unwrap();
        assert_eq!(out["error"], Value::Null, "{file}");
        assert!(out["file"].as_str().unwrap().starts_with("q"), "{out}");
    }
    assert_eq!(
        e.executed()
            .iter()
            .rev()
            .take(3)
            .cloned()
            .collect::<Vec<_>>(),
        ["SELECT 2", "SELECT 1", "SELECT 1"]
    );
}

/// 워크트리를 준 호출은 그 워크트리 안의 프로젝트 루트에서 파일을 읽는다(같은 상대 경로라도 메인 체크아웃의 것이 아니다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worktree_call_reads_the_worktrees_file() {
    let e = env();
    let wt = fake_worktree(&e.f.root, &e.f.base.join("p1-wt"));
    std::fs::create_dir_all(wt.join(".pitwall/db")).unwrap();
    std::fs::write(wt.join(".pitwall/db/dev.toml"), DEV).unwrap();
    e.file("q/a.sql", "SELECT 'main'");
    std::fs::create_dir_all(wt.join("q")).unwrap();
    std::fs::write(wt.join("q/a.sql"), "SELECT 'worktree'").unwrap();
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let wt = wt.display().to_string();
    e.query(
        &mut agent,
        "dev",
        json!({ "file": "q/a.sql", "worktree": wt }),
    )
    .await
    .unwrap();
    e.query(&mut agent, "dev", json!({ "file": "q/a.sql" }))
        .await
        .unwrap();
    let executed = e.executed();
    assert_eq!(
        executed[executed.len() - 2..],
        ["SELECT 'worktree'", "SELECT 'main'"]
    );
    // 워크트리의 파일이 메인 체크아웃 쪽 경로로 빠져나가지 못한다.
    let main_abs = e.f.root.join("q/a.sql").display().to_string();
    invalid(
        &e.query(
            &mut agent,
            "dev",
            json!({ "file": main_abs, "worktree": wt }),
        )
        .await,
        "outside the project roots",
    );
}

/// 분류한 내용과 실행한 내용이 같다: 읽은 뒤(분류 뒤 · 연결을 열기 직전) 파일을 쓰기로 바꿔도 읽은 조회가 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_file_runs_as_read_and_classified() {
    let e = env();
    e.file("q/a.sql", "SELECT * FROM users");
    e.human_opens("dev").await;
    let path = e.f.root.join("q/a.sql");
    *e.hook.lock().unwrap() = Some(Box::new(move || {
        std::fs::write(&path, "DELETE FROM users").unwrap();
    }));
    let mut agent = e.agent();
    let out = e
        .query(&mut agent, "dev", json!({ "file": "q/a.sql" }))
        .await
        .unwrap();
    assert_eq!(out["error"], Value::Null);
    assert_eq!(
        std::fs::read_to_string(e.f.root.join("q/a.sql")).unwrap(),
        "DELETE FROM users",
        "the hook ran"
    );
    assert_eq!(e.executed().last().unwrap(), "SELECT * FROM users");
    assert!(!e.executed().iter().any(|s| s.contains("DELETE")));
}

/// 행 제한은 문장마다(기본 100, `maxRows`), 넘으면 그 문장의 `truncated`. 범위 밖 `maxRows` 는 invalid params.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_are_limited_per_statement() {
    let e = env();
    e.pg.respond("FROM big", FakeResponse::rows(250));
    e.pg.respond("FROM small", FakeResponse::rows(5));
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let sql = "SELECT * FROM big; SELECT * FROM small; SELECT * FROM big";
    let out = e
        .query(&mut agent, "dev", json!({ "sql": sql }))
        .await
        .unwrap();
    let counts: Vec<(usize, bool)> = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["rows"].as_array().unwrap().len(),
                r["truncated"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(counts, [(100, true), (5, false), (100, true)]);
    let out = e
        .query(&mut agent, "dev", json!({ "sql": sql, "maxRows": 7 }))
        .await
        .unwrap();
    assert_eq!(out["results"][0]["rows"].as_array().unwrap().len(), 7);
    assert_eq!(out["results"][1]["truncated"], false);
    for bad in [0, 10_001] {
        invalid(
            &e.query(
                &mut agent,
                "dev",
                json!({ "sql": "SELECT 1", "maxRows": bad }),
            )
            .await,
            "maxRows",
        );
    }
}

/// 셀은 4 KiB 에서 잘리고 표지(`truncated` 값 — 원래 크기)가 남는다. 응답 전체가 예산을 넘으면 뒤쪽 행을 덜고 `truncated`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cells_and_the_whole_response_are_capped() {
    let e = env();
    std::fs::write(e.f.root.join("app.db"), "").unwrap();
    e.f.write_db("lite.toml", LITE);
    e.human_opens("lite").await;
    let mut agent = e.agent();
    let out = e
        .query(
            &mut agent,
            "lite",
            json!({ "sql": "SELECT printf('%.10000c', 'z') AS big, 'short' AS s" }),
        )
        .await
        .unwrap();
    let cell = &out["results"][0]["rows"][0][0]["truncated"];
    assert_eq!(cell["size"], 10_000, "{cell}");
    let prefix = cell["prefix"].as_str().unwrap();
    assert!(
        prefix.len() <= TOOL_CELL_BYTES && prefix.len() > TOOL_CELL_BYTES / 2,
        "{}",
        prefix.len()
    );
    assert_eq!(out["results"][0]["rows"][0][1], json!({ "text": "short" }));

    let sql = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 1000) \
               SELECT x, printf('%.3000c', 'y') FROM c; SELECT 'after' AS a";
    let out = e
        .query(&mut agent, "lite", json!({ "sql": sql, "maxRows": 1000 }))
        .await
        .unwrap();
    let size = out.to_string().len();
    assert!(size <= RESPONSE_BUDGET, "{size}");
    let first = &out["results"][0];
    assert_eq!(first["truncated"], true);
    let kept = first["rows"].as_array().unwrap().len();
    assert!(kept > 10 && kept < 1000, "{kept}");
    // 예산을 다 쓴 뒤의 문장은 행을 잃고 `truncated` 다(열은 남는다).
    assert_eq!(out["results"][1]["rows"], json!([]));
    assert_eq!(out["results"][1]["truncated"], true);
    assert_eq!(out["results"][1]["columns"][0]["name"], "a");
}

/// 둘째 문장이 실패하면 거기서 멈추고 첫 결과와 오류(문장 번호 · 줄 · 범위)를 돌려준다. 셋째 문장은 돌지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_statement_stops_the_run_with_its_index() {
    let e = env();
    e.pg.respond("FROM users", FakeResponse::rows(2));
    e.pg.respond(
        "FROM nowhere",
        FakeResponse::error(
            DbError::new(DbErrorKind::Server, "relation \"nowhere\" does not exist")
                .with_code("42P01"),
        ),
    );
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let sql = "SELECT * FROM users;\n\nSELECT * FROM nowhere;\nSELECT * FROM users";
    let out = e
        .query(&mut agent, "dev", json!({ "sql": sql }))
        .await
        .unwrap();
    assert_eq!(out["statements"], 3);
    assert_eq!(out["results"].as_array().unwrap().len(), 1);
    assert_eq!(out["error"]["index"], 1);
    assert_eq!(out["error"]["line"], 3);
    assert_eq!(out["error"]["error"]["code"], "42P01");
    let from = out["error"]["range"]["from"].as_u64().unwrap() as usize;
    assert!(
        sql.encode_utf16()
            .skip(from)
            .map(|u| u as u8 as char)
            .collect::<String>()
            .starts_with("SELECT * FROM nowhere")
    );
    assert_eq!(
        e.executed()
            .iter()
            .filter(|s| s.contains("FROM users"))
            .count(),
        1,
        "the third statement did not run"
    );
}

/// 클라이언트가 호출을 취소하면(`request.cancel` — MCP 취소가 Host 에 닿는 길) 버려진 핸들러가 드라이버 호출을 버리고(인터럽트)
/// 롤백한 뒤 그 연결을 닫는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_call_interrupts_the_driver_and_closes_the_connection() {
    let e = env();
    e.pg.respond("pg_sleep_table", FakeResponse::rows(1).hanging());
    e.human_opens("dev").await;
    let tool = e.connects() as u32;
    let agent = e.agent();
    agent.session.handle_value(json!({
        "jsonrpc": "2.0", "id": 900, "method": "db.tool.query",
        "params": { "projectId": e.f.id, "connectionId": "dev", "sql": "SELECT * FROM pg_sleep_table" },
    }));
    wait_until("the query to hang", || {
        e.pg.calls_of(tool)
            .iter()
            .any(|c| matches!(c, FakeCall::Fetch { .. }))
    })
    .await;
    agent.session.handle_value(json!({
        "jsonrpc": "2.0", "method": "request.cancel", "params": { "id": 900 },
    }));
    wait_until("the connection to close", || {
        e.pg.calls_of(tool).contains(&FakeCall::Close)
    })
    .await;
    let calls = e.pg.calls_of(tool);
    assert!(calls.contains(&FakeCall::Interrupt), "{calls:?}");
    assert!(calls.contains(&FakeCall::Rollback), "{calls:?}");
}

/// 관문은 M5-06 의 입구다: 사람이 연 적 없는 연결은 `notOpenedByUser`, Redis 연결은 `unsupported`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gate_is_the_agent_entry() {
    let e = env();
    let mut agent = e.agent();
    refused(
        &e.query(&mut agent, "dev", json!({ "sql": "SELECT 1" }))
            .await,
        "notOpenedByUser",
    );
    assert_eq!(e.connects(), 0);
    e.f.write_db(
        "cache.toml",
        "schema = 1\nname = \"cache\"\ndriver = \"redis\"\nhost = \"localhost\"\ndatabase = \"3\"\n",
    );
    let mut w = e.f.window();
    w.call(
        "db.session.open",
        json!({ "projectId": e.f.id, "connectionId": "cache" }),
    )
    .await
    .unwrap();
    let error = e
        .query(&mut agent, "cache", json!({ "sql": "SELECT 1" }))
        .await
        .unwrap_err();
    assert_eq!(error.code, pw_db::gate::code::DB_ERROR);
    assert_eq!(error.data.unwrap()["error"]["kind"], "unsupported");
}

/// MySQL · MariaDB 는 표지의 열쇠가 서버 + 사용자다(결정 M5-D37): 같은 서버 · 사용자의 다른 DB 가 프로덕션이면 dev 연결의
/// `db_query` 도 거절한다 — `SELECT … FROM app.users` 처럼 한 연결로 다른 DB 를 읽을 수 있어서다. 그 설정이 없으면 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_production_database_on_the_same_mysql_server_blocks_the_dev_connection() {
    let e = env();
    e.f.write_db("my_dev.toml", MY_DEV);
    e.human_opens("my_dev").await;
    let mut agent = e.agent();
    let sql = json!({ "sql": "SELECT * FROM app.users" });
    let out = e.query(&mut agent, "my_dev", sql.clone()).await.unwrap();
    assert_eq!(out["error"], Value::Null);
    let executed_before =
        e.my.count(&FakeCall::Execute("SELECT * FROM app.users".into()));
    assert_eq!(executed_before, 1);

    e.f.write_db("my_prod.toml", MY_PROD);
    refused(
        &e.query(&mut agent, "my_dev", sql).await,
        "productionBlocked",
    );
    assert_eq!(
        e.my.count(&FakeCall::Execute("SELECT * FROM app.users".into())),
        executed_before
    );
}

/// 감사 상세(결과의 `audit`): 문장 수 · 가장 높은 등급 · 문장 해시, `file` 이면 경로와 내용 해시. SQL 원문 · 값은 없다. 같은 글이면
/// 같은 해시, 다르면 다른 해시다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_audit_detail_has_counts_and_hashes_but_no_sql() {
    let e = env();
    e.human_opens("dev").await;
    let mut agent = e.agent();
    let sql = "SELECT * FROM users WHERE name = 'literal-in-the-sql-91c2'; SELECT 2";
    let audit = |out: &Value| out["audit"].as_str().unwrap().to_owned();
    let first = audit(
        &e.query(&mut agent, "dev", json!({ "sql": sql }))
            .await
            .unwrap(),
    );
    assert!(
        first.starts_with("statements=2 grade=read sql=sha256:"),
        "{first}"
    );
    assert!(
        !first.contains("literal-in-the-sql") && !first.contains("users"),
        "{first}"
    );
    let again = audit(
        &e.query(&mut agent, "dev", json!({ "sql": sql }))
            .await
            .unwrap(),
    );
    assert_eq!(first, again);
    let other = audit(
        &e.query(&mut agent, "dev", json!({ "sql": "SELECT 3; SELECT 2" }))
            .await
            .unwrap(),
    );
    assert_ne!(first, other);

    e.file("q/r.sql", sql);
    let from_file = audit(
        &e.query(&mut agent, "dev", json!({ "file": "q/r.sql" }))
            .await
            .unwrap(),
    );
    let content = sha256_hex(sql.as_bytes());
    assert!(
        from_file.ends_with(&format!(" file=q/r.sql content=sha256:{content}")),
        "{from_file}"
    );
    assert!(
        from_file.starts_with(&first),
        "same statements, same statement hash"
    );
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 창의 실행 스트림으로 돌리고, 확인을 물으면 확인해 다시 돌린다(`tests/schema.rs` 와 같다).
async fn run_confirmed(w: &mut Conn, session: &str, sql: &str) {
    let params = json!({ "sessionId": session, "sql": sql });
    match run(w, params.clone()).await {
        Ok(()) => {}
        Err(asked) if asked.code == pw_db::gate::code::CONFIRM_REQUIRED => {
            let data = asked.data.unwrap();
            let mut params = params;
            params["confirmed"] =
                json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });
            run(w, params).await.unwrap();
        }
        Err(e) => panic!("{sql}: {e:?}"),
    }
}

async fn run(w: &mut Conn, params: Value) -> Result<(), RpcError> {
    let stream = w.start("db.query.execute", params).await?;
    match w.collect(stream).await.1 {
        StreamOutcome::Completed => Ok(()),
        StreamOutcome::Failed { error } => Err(error),
        StreamOutcome::Cancelled => panic!("cancelled"),
    }
}

async fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "waited for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
