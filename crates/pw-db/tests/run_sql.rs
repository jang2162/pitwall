//! M6-09 의 완료 기준(17-m6-plan.md M6-09 행): 실행 구성의 `sql` 종류를 창의 `run.sql.start` 로 가짜 드라이버에서 돌려 본다 —
//! 트랜잭션 방식 표 · 파일 내용에 묶인 위험 확인 · read-only 연결 · 미신뢰 프로젝트 · 콘솔 형식 · 중지의 인터럽트. 에이전트의 길
//! (승인 · `run_start`)은 M6-14 가 같은 부품(`plan_sql_run` · `start_sql_run`)으로 잇는다 — 여기서는 그 계획의 꼴만 본다. 실 DB 의
//! `per_file` 은 `tests/live_run_sql.rs`(PG · MySQL), SQLite 는 아래 [`per_file_on_real_sqlite`].

mod common;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use common::{Conn, Fixture};
use pw_core::registry::plan::PlanChange;
use pw_db::DbSessions;
use pw_db::gate::{code, db_grant};
use pw_db::run_sql::SqlRunTarget;
use pw_db::script::SqlTransaction;
use pw_db_driver::fake::{FakeCall, FakeDriver, FakeResponse};
use pw_db_types::{DbError, DbErrorKind, SqlDialect};
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use pw_run::runs::{RunState, Runs};
use serde_json::{Value, json};

const DEV: &str = "schema = 1\nname = \"Local PG\"\ndriver = \"postgres\"\nhost = \"localhost\"\ndatabase = \"app\"\n";
const RO: &str = "schema = 1\nname = \"replica\"\ndriver = \"postgres\"\nhost = \"replica\"\ndatabase = \"app\"\nread_only = true\n";
const PROD: &str = "schema = 1\nname = \"Prod\"\ndriver = \"postgres\"\nhost = \"prod.example\"\ndatabase = \"app\"\nproduction = true\n";
const LITE: &str = "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n";

struct Env {
    f: Fixture,
    pg: FakeDriver,
    runs: Runs,
    sessions: Arc<DbSessions>,
}

fn env_with(trusted: bool) -> Env {
    let pg = FakeDriver::named_with(
        "postgres",
        SqlDialect::Postgres,
        FakeDriver::full_capabilities(),
    );
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(pg.clone())).unwrap();
    drivers
        .register(Arc::new(pw_db_driver_sqlite::SqliteDriver::new()))
        .unwrap();
    let runs = Runs::new(Arc::new(pw_runtime::Runtime::new(
        pw_runtime::LoginShell::new(None, Duration::from_secs(1)),
    )));
    let mut held = None;
    let for_router = runs.clone();
    let f = Fixture::build(trusted, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        held = Some(Arc::clone(&sessions));
        pw_db::register(router, Arc::clone(&sessions));
        pw_db::run_sql::register(router, sessions, for_router.clone());
        pw_run::runs::methods::register(router, for_router);
    });
    f.write_db("dev.toml", DEV);
    // 쓰기는 행 없는 문장으로 답한다(기본 응답은 행을 내는 조회다).
    for verb in ["INSERT", "UPDATE", "DELETE", "CREATE"] {
        pg.respond(verb, FakeResponse::write(1));
    }
    Env {
        f,
        pg,
        runs,
        sessions: held.unwrap(),
    }
}

fn env() -> Env {
    env_with(true)
}

impl Env {
    fn write(&self, rel: &str, text: &str) {
        let path = self.f.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// `.pitwall/run/<id>.toml` 의 `sql` 구성.
    fn config(&self, id: &str, connection: &str, files: &[&str], transaction: &str) {
        let files: Vec<String> = files.iter().map(|f| format!("\"{f}\"")).collect();
        self.write(
            &format!(".pitwall/run/{id}.toml"),
            &format!(
                "schema = 1\nname = \"{id}\"\ntype = \"sql\"\nconnection = \"{connection}\"\nfiles = [{}]\ntransaction = \"{transaction}\"\n",
                files.join(", ")
            ),
        );
    }

    async fn start(
        &self,
        w: &mut Conn,
        id: &str,
        confirmed: Option<Value>,
    ) -> Result<Value, RpcError> {
        let mut params = json!({ "projectId": self.f.id, "configId": id });
        if let Some(c) = confirmed {
            params["confirmed"] = c;
        }
        w.call("run.sql.start", params).await
    }

    /// 사람이 돌린다 — 확인을 물으면 받은 해시 · 등급으로 한 번 확인한다(연결 이름이 필요하면 친다). 끝날 때까지 기다려 콘솔 글.
    async fn run_confirmed(&self, w: &mut Conn, id: &str) -> (Value, String) {
        let info = match self.start(w, id, None).await {
            Ok(info) => info,
            Err(asked) if asked.code == code::CONFIRM_REQUIRED => {
                let data = asked.data.unwrap();
                let request = &data["request"];
                let mut confirmed = json!({
                    "grade": request["grade"],
                    "statementsHash": request["statementsHash"],
                });
                if request["nameRequired"] == json!(true) {
                    confirmed["name"] = data["connection"]["name"].clone();
                }
                self.start(w, id, Some(confirmed)).await.unwrap()
            }
            Err(e) => panic!("{id}: {e:?}"),
        };
        let console = self.finished(&info).await;
        (info, console)
    }

    /// 실행이 끝나기를 기다려 콘솔 글.
    async fn finished(&self, info: &Value) -> String {
        let run = self
            .runs
            .find(&self.f.id, info["runId"].as_str().unwrap())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), run.exited())
            .await
            .expect("the run ends");
        run.with_log(|log| String::from_utf8(log.read(0, usize::MAX).bytes).unwrap())
    }
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

const SEED: &str = "INSERT INTO a VALUES (1);\n";
const FIXTURES: &str =
    "INSERT INTO b VALUES (1);\nINSERT INTO bad VALUES (2);\nINSERT INTO c VALUES (3);\n";

/// ① 트랜잭션 방식 표: 둘째 파일의 둘째 문장이 실패하면 `all` 은 첫 파일도 롤백, `per_file` 은 첫 파일이 남는다, `none` 은 실패
/// 문장 앞까지 남는다. 방식을 무시하면(늘 하나로 · 늘 자동 커밋) 칸 하나가 틀린다.
#[tokio::test]
async fn the_transaction_mode_decides_what_a_failure_keeps() {
    let cases: [(&str, &[&str], &str); 3] = [
        ("all", &[], "rolled back: db/seed.sql, db/fixtures.sql"),
        (
            "per_file",
            &["INSERT INTO a VALUES (1)"],
            "rolled back: db/fixtures.sql",
        ),
        (
            "none",
            &["INSERT INTO a VALUES (1)", "INSERT INTO b VALUES (1)"],
            "no transaction",
        ),
    ];
    for (mode, kept, said) in cases {
        let e = env();
        e.pg.respond(
            "bad",
            FakeResponse::error(DbError::new(
                DbErrorKind::Server,
                "relation \"bad\" does not exist",
            )),
        );
        e.write("db/seed.sql", SEED);
        e.write("db/fixtures.sql", FIXTURES);
        e.config("seed", "dev", &["db/seed.sql", "db/fixtures.sql"], mode);
        let mut w = e.f.window();
        let (info, console) = e.run_confirmed(&mut w, "seed").await;
        assert_eq!(info["kind"], "sql", "{info}");
        assert_eq!(e.pg.committed(), kept, "{mode}: {console}");
        assert!(console.contains(said), "{mode}: {console}");
        assert!(
            console.contains("#3 db/fixtures.sql:2 error: relation \"bad\" does not exist"),
            "{mode}: {console}"
        );
        assert!(
            !e.pg
                .calls()
                .contains(&FakeCall::Execute("INSERT INTO c VALUES (3)".into())),
            "a failure always stops ({mode})"
        );
        let run = e
            .runs
            .find(&e.f.id, info["runId"].as_str().unwrap())
            .unwrap();
        assert_eq!(run.state(), RunState::Exited);
        assert_eq!(run.info().exit.unwrap().code, Some(1));
        // 프로세스를 띄우지 않았다.
        assert_eq!(run.info().pid, None);
    }
}

/// 모두 성공하면 방식마다 커밋되고 끝 코드 0 이다(`before` 가 그것으로 다음을 띄운다 — M6-21).
#[tokio::test]
async fn a_run_that_succeeds_commits_and_exits_with_zero() {
    for mode in ["all", "per_file", "none"] {
        let e = env();
        e.write("db/seed.sql", SEED);
        e.write("db/fixtures.sql", "INSERT INTO b VALUES (1);\n");
        e.config("seed", "dev", &["db/seed.sql", "db/fixtures.sql"], mode);
        let mut w = e.f.window();
        let (info, console) = e.run_confirmed(&mut w, "seed").await;
        assert_eq!(
            e.pg.committed(),
            ["INSERT INTO a VALUES (1)", "INSERT INTO b VALUES (1)"],
            "{mode}: {console}"
        );
        let run = e
            .runs
            .find(&e.f.id, info["runId"].as_str().unwrap())
            .unwrap();
        assert_eq!(run.info().exit.unwrap().code, Some(0), "{mode}: {console}");
        assert!(console.ends_with("done: 2 statements\r\n"), "{console}");
    }
}

/// ② 구성은 그대로이고 파일 내용만 바꾸면 다시 분류한다 — 조회만이면 묻지 않고, `DELETE` 를 넣으면 위험 확인을 청한다. 확인을
/// 받은 뒤 파일이 바뀌면 그 확인으로는 돌지 않고 다시 묻는다(연결도 열지 않는다).
#[tokio::test]
async fn changing_only_the_file_asks_again() {
    let e = env();
    e.write("db/report.sql", "SELECT 1;\n");
    e.config("report", "dev", &["db/report.sql"], "all");
    let mut w = e.f.window();
    // 조회만 — 묻지 않고 읽기 전용 트랜잭션에서 돈다.
    let info = e.start(&mut w, "report", None).await.unwrap();
    let console = e.finished(&info).await;
    assert!(
        e.pg.calls().contains(&FakeCall::Begin { read_only: true }),
        "{console}"
    );
    assert!(
        console.contains("read-only, rolled back: db/report.sql"),
        "{console}"
    );

    // 구성은 그대로, 파일만 DELETE 로.
    e.write("db/report.sql", "DELETE FROM t WHERE id = 1;\n");
    let asked = e.start(&mut w, "report", None).await.expect_err("asks");
    assert_eq!(asked.code, code::CONFIRM_REQUIRED, "{asked:?}");
    let data = asked.data.unwrap();
    assert_eq!(data["request"]["grade"], "write");
    assert_eq!(data["files"][0]["path"], "db/report.sql");
    assert!(data["sql"].as_str().unwrap().contains("DELETE FROM t"));
    let confirmed = json!({
        "grade": data["request"]["grade"],
        "statementsHash": data["request"]["statementsHash"],
    });

    // 확인 뒤 파일이 바뀌었다 — 그 확인으로는 돌지 않는다.
    e.write("db/report.sql", "DELETE FROM t WHERE id = 2;\n");
    let connects = e.pg.count(&FakeCall::Connect);
    let again = e
        .start(&mut w, "report", Some(confirmed))
        .await
        .expect_err("asks again");
    assert_eq!(again.code, code::CONFIRM_REQUIRED);
    assert_eq!(again.data.unwrap()["request"]["rejected"], "hashMismatch");
    assert_eq!(e.pg.count(&FakeCall::Connect), connects, "nothing opened");
    assert!(e.pg.committed().is_empty());
}

/// 같은 확인 해시는 같은 내용에만 — 내용이 같으면 같은 해시, 한 글자라도 다르면 다른 해시다(구성 해시만 보면 실패).
#[tokio::test]
async fn the_confirmation_is_bound_to_the_file_contents() {
    let e = env();
    e.write("db/a.sql", "DELETE FROM t WHERE id = 1;\n");
    e.config("a", "dev", &["db/a.sql"], "all");
    let grant = db_grant(&ConnectionScope::Project(e.f.id.clone()), &e.f.id).unwrap();
    let target = |e: &Env| SqlRunTarget {
        project_id: e.f.id.clone(),
        worktree: None,
        config: "a".into(),
        name: "a".into(),
        connection: "dev".into(),
        files: vec!["db/a.sql".into()],
        transaction: SqlTransaction::All,
    };
    let first = e.sessions.plan_sql_run(&grant, &target(&e)).await.unwrap();
    let same = e.sessions.plan_sql_run(&grant, &target(&e)).await.unwrap();
    assert_eq!(first.confirm_hash(), same.confirm_hash());
    e.write("db/a.sql", "DELETE FROM t WHERE id = 1; \n");
    let changed = e.sessions.plan_sql_run(&grant, &target(&e)).await.unwrap();
    assert_ne!(first.confirm_hash(), changed.confirm_hash());
}

/// 프로덕션 연결의 파괴 등급은 연결 이름을 쳐야 돈다(SQL 콘솔과 같다).
#[tokio::test]
async fn production_destructive_needs_the_connection_name() {
    let e = env();
    e.f.write_db("prod.toml", PROD);
    e.write("db/wipe.sql", "DROP TABLE t;\n");
    e.config("wipe", "prod", &["db/wipe.sql"], "all");
    let mut w = e.f.window();
    let asked = e.start(&mut w, "wipe", None).await.expect_err("asks");
    let data = asked.data.unwrap();
    assert_eq!(data["request"]["nameRequired"], true);
    assert_eq!(data["connection"]["production"], true);
    let mut confirmed = json!({
        "grade": data["request"]["grade"],
        "statementsHash": data["request"]["statementsHash"],
        "name": "prod",
    });
    let wrong = e
        .start(&mut w, "wipe", Some(confirmed.clone()))
        .await
        .expect_err("wrong name");
    assert_eq!(wrong.data.unwrap()["request"]["rejected"], "nameMismatch");
    confirmed["name"] = json!("Prod");
    let info = e.start(&mut w, "wipe", Some(confirmed)).await.unwrap();
    let console = e.finished(&info).await;
    assert!(console.contains("committed: db/wipe.sql"), "{console}");
}

/// ③ read-only 연결의 쓰기 문장은 거절한다(묻지 않고, 연결하지 않는다).
#[tokio::test]
async fn a_read_only_connection_refuses_writes() {
    let e = env();
    e.f.write_db("ro.toml", RO);
    e.write("db/seed.sql", "SELECT 1;\nINSERT INTO a VALUES (1);\n");
    e.config("seed", "ro", &["db/seed.sql"], "per_file");
    let mut w = e.f.window();
    let refused = e.start(&mut w, "seed", None).await.expect_err("refused");
    assert_eq!(
        (refused.code, reason(&refused)),
        (rpc_code::PERMISSION_DENIED, Some("readOnlyConnection")),
        "{refused:?}"
    );
    assert_eq!(e.pg.count(&FakeCall::Connect), 0);
}

/// ④ 미신뢰 프로젝트는 연결하지 않는다(구성 · 파일도 읽지 않는다).
#[tokio::test]
async fn an_untrusted_project_does_not_connect() {
    let e = env_with(false);
    e.write("db/seed.sql", SEED);
    e.config("seed", "dev", &["db/seed.sql"], "all");
    let mut w = e.f.window();
    let refused = e.start(&mut w, "seed", None).await.expect_err("refused");
    assert_eq!(
        (refused.code, reason(&refused)),
        (rpc_code::PERMISSION_DENIED, Some("untrusted")),
        "{refused:?}"
    );
    assert_eq!(e.pg.count(&FakeCall::Connect), 0);
    assert!(e.runs.list(&e.f.id).is_empty());
}

/// 그 프로젝트의 창만 돌린다(다른 프로젝트 창 · Host · 에이전트 범위는 거절).
#[tokio::test]
async fn only_the_project_window_starts_an_sql_run() {
    let e = env();
    e.write("db/seed.sql", SEED);
    e.config("seed", "dev", &["db/seed.sql"], "all");
    for scope in [
        ConnectionScope::Project(e.f.other.clone()),
        ConnectionScope::Host,
        ConnectionScope::Agent,
    ] {
        let mut c = e.f.connect(scope.clone());
        let refused = e.start(&mut c, "seed", None).await.expect_err("refused");
        assert_eq!(refused.code, rpc_code::PERMISSION_DENIED, "{scope:?}");
    }
    assert_eq!(e.pg.count(&FakeCall::Connect), 0);
}

/// 파일 안의 트랜잭션 제어는 받지 않는다 — 구성의 `transaction` 이 정한다.
#[tokio::test]
async fn transaction_control_in_a_file_is_refused() {
    let e = env();
    e.write("db/seed.sql", "INSERT INTO a VALUES (1);\nCOMMIT;\n");
    e.config("seed", "dev", &["db/seed.sql"], "all");
    let mut w = e.f.window();
    let refused = e.start(&mut w, "seed", None).await.expect_err("refused");
    assert_eq!(
        reason(&refused),
        Some("transactionStatement"),
        "{refused:?}"
    );
}

/// 프로세스 구성의 길(`run.start`)은 `sql` 구성을 띄우지 않고, `run.sql.start` 는 프로세스 구성을 받지 않는다.
#[tokio::test]
async fn an_sql_config_is_not_a_process() {
    let e = env();
    e.write(
        ".pitwall/run/web.toml",
        "schema = 1\nname = \"web\"\ntype = \"shell\"\ncommand = \"echo hi\"\n",
    );
    let mut w = e.f.window();
    let refused = e.start(&mut w, "web", None).await.expect_err("refused");
    assert_eq!(refused.code, rpc_code::INVALID_PARAMS, "{refused:?}");
}

/// ⑤ 콘솔 형식: 머리, 문장마다 `#<번호> <파일>:<줄> <등급> <행 수> <소요 ms>`, 조회는 앞 5행의 표와 남은 행 수, 오류는 DB 원문과
/// 번호 · 줄, 트랜잭션의 끝, 돌지 않은 파일, 끝. 소요(ms)는 매번 달라 `<n>` 으로 바꿔 견준다.
#[tokio::test]
async fn the_console_format() {
    let e = env();
    e.pg.respond("SELECT", FakeResponse::rows(7));
    e.pg.respond(
        "missing",
        FakeResponse::error(DbError {
            code: Some("42P01".into()),
            ..DbError::new(DbErrorKind::Server, "relation \"missing\" does not exist")
        }),
    );
    e.write(
        "db/seed.sql",
        "-- seed\nINSERT INTO a VALUES (1);\nSELECT id, name FROM a;\n",
    );
    e.write(
        "db/fixtures.sql",
        "UPDATE missing SET x = 1 WHERE id = 1;\n",
    );
    e.write("db/later.sql", "INSERT INTO z VALUES (1);\n");
    e.config(
        "seed",
        "dev",
        &["db/seed.sql", "db/fixtures.sql", "db/later.sql"],
        "per_file",
    );
    let mut w = e.f.window();
    let (_, console) = e.run_confirmed(&mut w, "seed").await;
    let shown = regex_ms(&console);
    assert_eq!(
        shown,
        "sql: 3 files on `Local PG`, transaction per_file\r\n\
         #1 db/seed.sql:2 write 1 row affected <n> ms\r\n\
         #2 db/seed.sql:3 read 7 rows <n> ms\r\n\
         \x20 | id | name |\r\n\
         \x20 | 0  | r0c1 |\r\n\
         \x20 | 1  | r1c1 |\r\n\
         \x20 | 2  | r2c1 |\r\n\
         \x20 | 3  | r3c1 |\r\n\
         \x20 | 4  | r4c1 |\r\n\
         \x20 … 2 more rows\r\n\
         committed: db/seed.sql\r\n\
         #3 db/fixtures.sql:1 error [42P01]: relation \"missing\" does not exist\r\n\
         rolled back: db/fixtures.sql — nothing of this file is kept\r\n\
         not run: db/later.sql\r\n\
         failed: 3 statements\r\n"
    );
}

/// 소요 ms 를 `<n> ms` 로.
fn regex_ms(text: &str) -> String {
    let mut out = String::new();
    for line in text.split_inclusive("\r\n") {
        match line.rfind(" ms\r\n") {
            Some(end) if line.starts_with('#') => {
                let start = line[..end].rfind(' ').unwrap();
                out.push_str(&line[..start]);
                out.push_str(" <n> ms\r\n");
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// ⑥ 중지는 도는 문장에 드라이버의 인터럽트를 보내고(`QueryCancel`), 트랜잭션을 롤백하고 「stopped」 로 끝난다.
#[tokio::test]
async fn stopping_interrupts_the_running_statement() {
    let e = env();
    e.pg.respond("SELECT pg_sleep", FakeResponse::rows(1).hanging());
    e.write(
        "db/slow.sql",
        "INSERT INTO a VALUES (1);\nSELECT pg_sleep(60);\nINSERT INTO b VALUES (2);\n",
    );
    e.config("slow", "dev", &["db/slow.sql"], "all");
    let mut w = e.f.window();
    let asked = e.start(&mut w, "slow", None).await.expect_err("asks");
    let request = asked.data.unwrap()["request"].clone();
    let info = e
        .start(
            &mut w,
            "slow",
            Some(json!({ "grade": request["grade"], "statementsHash": request["statementsHash"] })),
        )
        .await
        .unwrap();
    let run_id = info["runId"].as_str().unwrap().to_owned();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !e
            .pg
            .calls()
            .contains(&FakeCall::Execute("SELECT pg_sleep(60)".into()))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the slow statement runs");
    w.call("run.stop", json!({ "runId": run_id }))
        .await
        .unwrap();
    let console = e.finished(&info).await;
    assert_eq!(e.pg.count(&FakeCall::Interrupt), 1, "{console}");
    assert!(e.pg.committed().is_empty(), "{console}");
    assert!(console.contains("rolled back: db/slow.sql"), "{console}");
    assert!(console.ends_with("stopped: 2 statements\r\n"), "{console}");
    assert!(
        !e.pg
            .calls()
            .contains(&FakeCall::Execute("INSERT INTO b VALUES (2)".into()))
    );
}

/// ⑥(승인 계획의 꼴 — M6-14 · 21 이 채운다): 파일 목록 · 문장마다 파일 · 트랜잭션 방식 · 「실패해도 남는다」.
#[tokio::test]
async fn the_approval_plan_carries_files_mode_and_what_a_failure_keeps() {
    let e = env();
    e.write("db/seed.sql", SEED);
    e.write("db/fixtures.sql", FIXTURES);
    let grant = db_grant(&ConnectionScope::Project(e.f.id.clone()), &e.f.id).unwrap();
    for (mode, keeps) in [
        (SqlTransaction::All, false),
        (SqlTransaction::PerFile, true),
        (SqlTransaction::None, true),
    ] {
        let target = SqlRunTarget {
            project_id: e.f.id.clone(),
            worktree: None,
            config: "seed".into(),
            name: "seed".into(),
            connection: "dev".into(),
            files: vec!["db/seed.sql".into(), "db/fixtures.sql".into()],
            transaction: mode,
        };
        let plan = e.sessions.plan_sql_run(&grant, &target).await.unwrap();
        let change = plan.plan_change();
        let PlanChange::Sql { .. } = &change else {
            panic!("{change:?}")
        };
        let value = serde_json::to_value(&change).unwrap();
        assert_eq!(value["files"][1]["path"], "db/fixtures.sql");
        assert_eq!(value["statements"].as_array().unwrap().len(), 4);
        assert_eq!(value["statements"][3]["file"], 1);
        assert_eq!(value["statements"][3]["line"], 3);
        assert_eq!(value["transaction"], json!(mode.name()));
        assert_eq!(value["keepsOnFailure"], keeps, "{mode:?}");
        assert!(
            plan.audit_detail()
                .contains("file=db/seed.sql content=sha256:")
        );
        assert!(
            !plan.audit_detail().contains("INSERT"),
            "no SQL text in the audit"
        );
    }
}

/// `per_file` 을 실 SQLite 로(서버가 없어 보통 시험이다 — PG · MySQL 은 `tests/live_run_sql.rs`): 둘째 파일이 실패하면 첫 파일의
/// 행은 남고 둘째 파일의 앞 문장은 남지 않는다.
#[tokio::test]
async fn per_file_on_real_sqlite() {
    let e = env();
    e.f.write_db("lite.toml", LITE);
    e.write("app.db", "");
    e.write(
        "db/schema.sql",
        "CREATE TABLE a (id INTEGER PRIMARY KEY);\nINSERT INTO a VALUES (1);\n",
    );
    e.write(
        "db/fixtures.sql",
        "INSERT INTO a VALUES (2);\nINSERT INTO nope VALUES (3);\n",
    );
    e.config(
        "seed",
        "lite",
        &["db/schema.sql", "db/fixtures.sql"],
        "per_file",
    );
    let mut w = e.f.window();
    let (_, console) = e.run_confirmed(&mut w, "seed").await;
    assert!(console.contains("committed: db/schema.sql"), "{console}");
    assert!(
        console.contains("rolled back: db/fixtures.sql"),
        "{console}"
    );
    e.write("db/count.sql", "SELECT count(*) FROM a;\n");
    e.config("count", "lite", &["db/count.sql"], "all");
    let (_, counted) = e.run_confirmed(&mut w, "count").await;
    assert!(counted.contains("| 1        |"), "{counted}");
}
