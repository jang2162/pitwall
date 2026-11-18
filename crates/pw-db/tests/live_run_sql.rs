//! 실 DB 로 실행 구성의 `sql` 종류를 본다(17-m6-plan.md M6-09 — `db-live` 잡에 PG · MySQL 의 `per_file` 하나씩, SQLite 는 서버가
//! 없어 `tests/run_sql.rs` 의 보통 시험이다): 둘째 파일이 실패하면 첫 파일은 커밋된 채 남고 둘째 파일의 앞 문장은 롤백된다. PG 는
//! `none` 이 트랜잭션 안에서 못 도는 문장(`CREATE INDEX CONCURRENTLY` — 결정 M5-D35 의 이월)을 돌린다는 것도 본다. 사람이 창에서
//! 돌리는 길(`run.sql.start` — 확인을 받아 다시 부른다)이다. `PW_TEST_PG_URL` · `PW_TEST_MYSQL_URL` 이 있을 때만 목록에 생긴다
//! (`mise run test-db -- pg mysql mariadb`).
#![cfg(any(pw_live_pg, pw_live_mysql))]

mod common;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use common::{Conn, Fixture};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::live::{LiveDb, endpoint};
use pw_run::runs::Runs;
use serde_json::json;

/// 시험 URL 의 서버를 연결 `dev` 로 둔 Host(실행 관리자 · `run.sql.start` 포함)와 그 프로젝트 창.
async fn host(live: LiveDb, driver: Arc<dyn pw_db_driver::DbDriver>) -> (Fixture, Conn, Runs) {
    let url = live.url();
    let end = endpoint(live, &url).unwrap();
    let rest = url.split_once("://").unwrap().1;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let user = authority
        .rsplit_once('@')
        .map(|(info, _)| info.split(':').next().unwrap().to_owned())
        .unwrap();
    let database = path.split(['?', '#']).next().unwrap_or_default().to_owned();
    let id = driver.id().to_owned();
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(driver).unwrap();
    let runs = Runs::new(Arc::new(pw_runtime::Runtime::new(
        pw_runtime::LoginShell::new(None, Duration::from_secs(1)),
    )));
    let for_router = runs.clone();
    let f = Fixture::build(true, |router, connections| {
        let sessions = Arc::new(DbSessions::new(Arc::clone(connections), drivers));
        pw_db::register(router, Arc::clone(&sessions));
        pw_db::run_sql::register(router, sessions, for_router);
    });
    f.write_db(
        "dev.toml",
        &format!(
            "schema = 1\nname = \"dev\"\ndriver = \"{id}\"\nhost = \"{}\"\nport = {}\nuser = \"{user}\"\ndatabase = \"{database}\"\nsslmode = \"disable\"\npassword = {{ secret_ref = \"keychain\" }}\n",
            end.host, end.port
        ),
    );
    let mut w = f.window();
    if let Some(password) = &end.password {
        w.set_password(&f.id, "dev", password).await.unwrap();
    }
    (f, w, runs)
}

/// 파일들과 `sql` 구성을 쓴다.
fn config(f: &Fixture, id: &str, files: &[(&str, String)], transaction: &str) {
    for (path, text) in files {
        let at = f.root.join(path);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(at, text).unwrap();
    }
    let list: Vec<String> = files.iter().map(|(p, _)| format!("\"{p}\"")).collect();
    let at = f.root.join(format!(".pitwall/run/{id}.toml"));
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    fs::write(
        at,
        format!(
            "schema = 1\nname = \"{id}\"\ntype = \"sql\"\nconnection = \"dev\"\nfiles = [{}]\ntransaction = \"{transaction}\"\n",
            list.join(", ")
        ),
    )
    .unwrap();
}

/// 사람이 돌린다(확인을 물으면 받은 해시로 한 번 확인한다). 끝날 때까지 기다려 콘솔 글.
async fn run(f: &Fixture, w: &mut Conn, runs: &Runs, id: &str) -> String {
    let params = json!({ "projectId": f.id, "configId": id });
    let info = match w.call("run.sql.start", params.clone()).await {
        Ok(info) => info,
        Err(asked) if asked.code == code::CONFIRM_REQUIRED => {
            let request = asked.data.unwrap()["request"].clone();
            let mut params = params;
            params["confirmed"] =
                json!({ "grade": request["grade"], "statementsHash": request["statementsHash"] });
            w.call("run.sql.start", params).await.unwrap()
        }
        Err(e) => panic!("{id}: {e:?}"),
    };
    let run = runs.find(&f.id, info["runId"].as_str().unwrap()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), run.exited())
        .await
        .expect("the run ends");
    run.with_log(|log| String::from_utf8(log.read(0, usize::MAX).bytes).unwrap())
}

/// 표의 행 수(조회 구성을 돌려 콘솔의 `read <n> rows` 를 읽는다).
async fn count(f: &Fixture, w: &mut Conn, runs: &Runs, table: &str) -> u64 {
    config(
        f,
        "count",
        &[("db/count.sql", format!("SELECT id FROM {table};\n"))],
        "all",
    );
    let console = run(f, w, runs, "count").await;
    let line = console
        .lines()
        .find(|l| l.starts_with("#1 "))
        .unwrap_or_else(|| panic!("{console}"));
    let words: Vec<&str> = line.split(' ').collect();
    assert_eq!(words[2], "read", "{console}");
    words[3].parse().unwrap()
}

/// `per_file`: 첫 파일(표 만들기 · 행 둘)은 커밋되고, 둘째 파일은 행 하나를 넣은 뒤 키 중복으로 실패해 그 행도 남지 않는다.
async fn per_file(live: LiveDb, driver: Arc<dyn pw_db_driver::DbDriver>, table: &str) {
    let (f, mut w, runs) = host(live, driver).await;
    config(
        &f,
        "drop",
        &[("db/drop.sql", format!("DROP TABLE IF EXISTS {table};\n"))],
        "all",
    );
    run(&f, &mut w, &runs, "drop").await;
    config(
        &f,
        "seed",
        &[
            (
                "db/schema.sql",
                format!(
                    "CREATE TABLE {table} (id INT PRIMARY KEY, note VARCHAR(20));\nINSERT INTO {table} (id, note) VALUES (1, 'one'), (2, 'two');\n"
                ),
            ),
            (
                "db/fixtures.sql",
                format!(
                    "INSERT INTO {table} (id, note) VALUES (3, 'three');\nINSERT INTO {table} (id, note) VALUES (1, 'dup');\n"
                ),
            ),
        ],
        "per_file",
    );
    let console = run(&f, &mut w, &runs, "seed").await;
    assert!(console.contains("committed: db/schema.sql"), "{console}");
    assert!(console.contains("#4 db/fixtures.sql:2 error"), "{console}");
    assert!(
        console.contains("rolled back: db/fixtures.sql"),
        "{console}"
    );
    assert_eq!(
        count(&f, &mut w, &runs, table).await,
        2,
        "the first file stays, the second does not"
    );
    run(&f, &mut w, &runs, "drop").await;
}

#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_sql_run_per_file_keeps_the_files_before_a_failure() {
    per_file(
        LiveDb::Pg,
        Arc::new(pw_db_driver_pg::PgDriver::new()),
        "m6_09_per_file_probe",
    )
    .await;
}

/// PG `none`: 트랜잭션 안에서 못 도는 `CREATE INDEX CONCURRENTLY` 가 돈다(`all` 이면 SQLSTATE 25001 로 실패한다).
#[cfg(pw_live_pg)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pg_sql_run_none_runs_a_statement_that_needs_no_transaction() {
    let table = "m6_09_none_probe";
    let (f, mut w, runs) = host(LiveDb::Pg, Arc::new(pw_db_driver_pg::PgDriver::new())).await;
    let files = [(
        "db/index.sql",
        format!(
            "DROP TABLE IF EXISTS {table};\nCREATE TABLE {table} (id INT PRIMARY KEY, note TEXT);\nCREATE INDEX CONCURRENTLY m6_09_none_idx ON {table} (note);\n"
        ),
    )];
    config(&f, "index", &files, "all");
    let in_one = run(&f, &mut w, &runs, "index").await;
    assert!(in_one.contains("error [25001]"), "{in_one}");
    config(&f, "index", &files, "none");
    let alone = run(&f, &mut w, &runs, "index").await;
    assert!(alone.contains("no transaction"), "{alone}");
    assert!(alone.ends_with("done: 3 statements\r\n"), "{alone}");
    config(
        &f,
        "drop",
        &[("db/drop.sql", format!("DROP TABLE IF EXISTS {table};\n"))],
        "all",
    );
    run(&f, &mut w, &runs, "drop").await;
}

#[cfg(pw_live_mysql)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_mysql_sql_run_per_file_keeps_the_files_before_a_failure() {
    per_file(
        LiveDb::Mysql,
        Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
        "m6_09_per_file_probe",
    )
    .await;
}

/// MySQL · MariaDB `all`: 첫 파일의 `CREATE TABLE`(암묵 커밋) 뒤 둘째 파일이 실패하면, 콘솔은 「아무것도 남지 않는다」 가 아니라
/// 암묵 커밋 문장까지 남았다고 쓴다 — 표는 남고, 그 뒤 넣은 행은 롤백된다.
#[cfg(pw_live_mysql)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_mysql_sql_run_all_after_an_implicit_commit_says_what_is_kept() {
    let table = "m6_09_implicit_probe";
    let (f, mut w, runs) = host(
        LiveDb::Mysql,
        Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
    )
    .await;
    config(
        &f,
        "drop",
        &[("db/drop.sql", format!("DROP TABLE IF EXISTS {table};\n"))],
        "all",
    );
    run(&f, &mut w, &runs, "drop").await;
    config(
        &f,
        "seed",
        &[
            (
                "db/schema.sql",
                format!(
                    "CREATE TABLE {table} (id INT PRIMARY KEY);\nINSERT INTO {table} (id) VALUES (1);\n"
                ),
            ),
            (
                "db/fixtures.sql",
                format!(
                    "INSERT INTO {table} (id) VALUES (2);\nINSERT INTO {table} (id) VALUES (1);\n"
                ),
            ),
        ],
        "all",
    );
    let console = run(&f, &mut w, &runs, "seed").await;
    assert!(
        console.contains("rolled back after an implicit commit: db/schema.sql, db/fixtures.sql — the statement at db/schema.sql:1"),
        "{console}"
    );
    assert!(!console.contains("nothing of"), "{console}");
    assert_eq!(
        count(&f, &mut w, &runs, table).await,
        0,
        "the table stays, the rows after it do not"
    );
    run(&f, &mut w, &runs, "drop").await;
}
