//! M3-12 의 실 DB 완료 기준(14-m3-plan.md M3-12 행): 실제 드라이버로 `db.edit.*` 를 관문 너머에서 — 창이 밟는 RPC 순서 그대로
//! (`db.session.open` → `db.edit.preview` → `db.edit.apply` → `db.query.execute` 로 다시 읽기).
//!
//! 시나리오 일곱을 DB 마다 같은 이름으로 돈다: SQLite 는 늘(`sqlite_edit_*`), PG 는 `PW_TEST_PG_URL` 이 있을 때(`live_pg_edit_*`),
//! MySQL · MariaDB 는 `PW_TEST_MYSQL_URL` 이 있을 때(`live_mysql_edit_*` — `scripts/db-live.mjs` 가 MariaDB URL 로 한 번 더 돈다).
//! 「다른 세션」 은 같은 연결로 연 두 번째 세션(서버 연결이 따로다)이다. 테이블 이름은 시험마다(nextest 는 시험마다 프로세스 하나)
//! 다르다 — 한 서버에서 함께 돈다.

mod common;

use std::sync::Arc;

use common::{Conn, Fixture};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_types::SqlDialect;
use pw_rpc::RpcError;
use pw_rpc::protocol::StreamOutcome;
use serde_json::{Value, json};

/// 열린 Host 하나: 창 연결과 세션 둘(A 가 편집하고 B 가 다른 세션으로 본다), 시험 테이블 `t`(`id` PK, `name`, `note`).
struct Db {
    _f: Fixture,
    w: Conn,
    a: String,
    b: String,
    dialect: SqlDialect,
    /// 콘솔 문장의 테이블 이름(세션의 기본 DB · 스키마).
    name: String,
    /// 편집의 테이블(한정한 `ObjectRef`).
    table: Value,
}

/// 실 DB 연결 설정 하나로 Host 를 연다. `config` 는 `name`/`driver` 뒤의 줄들.
async fn host(
    driver: Arc<dyn pw_db_driver::DbDriver>,
    dialect: SqlDialect,
    config: &str,
    password: Option<&str>,
    table: impl FnOnce(&str) -> Value,
    prepare: impl FnOnce(&Fixture),
) -> Db {
    let mut drivers = pw_db::DriverRegistry::new();
    let id = driver.id().to_owned();
    drivers.register(driver).unwrap();
    let f = Fixture::build(true, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    prepare(&f);
    f.write_db(
        "dev.toml",
        &format!("schema = 1\nname = \"dev\"\ndriver = \"{id}\"\n{config}"),
    );
    let mut w = f.window();
    if let Some(password) = password {
        w.set_password(&f.id, "dev", password).await.unwrap();
    }
    let open = async |w: &mut Conn| {
        let opened = w
            .call(
                "db.session.open",
                json!({ "projectId": f.id, "connectionId": "dev" }),
            )
            .await
            .unwrap();
        opened["session"]["sessionId"].as_str().unwrap().to_owned()
    };
    let a = open(&mut w).await;
    let b = open(&mut w).await;
    let name = format!("pw_edit_{}", std::process::id());
    let mut db = Db {
        table: table(&name),
        _f: f,
        w,
        a,
        b,
        dialect,
        name,
    };
    let (text, bytes) = match dialect {
        SqlDialect::Postgres => ("text", "bytea"),
        SqlDialect::Mysql => ("TEXT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin", "LONGBLOB"),
        SqlDialect::Sqlite => ("TEXT", "BLOB"),
    };
    let b = db.b.clone();
    db.exec(&b, &format!("DROP TABLE IF EXISTS {}", db.name))
        .await;
    db.exec(
        &b,
        &format!(
            "CREATE TABLE {} (id INTEGER NOT NULL PRIMARY KEY, name {text}, note {text}, bin {bytes})",
            db.name
        ),
    )
    .await;
    db.exec(
        &b,
        &format!(
            "INSERT INTO {} (id, name, note) VALUES (1, 'one', 'n1'), (2, 'two', 'n2')",
            db.name
        ),
    )
    .await;
    db
}

impl Db {
    /// 문장을 돌린다. 확인을 물으면 받은 해시로 한 번 확인한다(창이 사람의 확인을 받은 흐름).
    async fn exec(&mut self, session: &str, sql: &str) -> Vec<Value> {
        let params = json!({ "sessionId": session, "sql": sql });
        match self.stream(params.clone()).await {
            Ok(items) => items,
            Err(asked) if asked.code == code::CONFIRM_REQUIRED => {
                let data = asked.data.unwrap();
                let mut params = params;
                params["confirmed"] =
                    json!({ "grade": data["grade"], "statementsHash": data["statementsHash"] });
                self.stream(params)
                    .await
                    .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
            }
            Err(e) => panic!("{sql}: {e:?}"),
        }
    }

    async fn stream(&mut self, params: Value) -> Result<Vec<Value>, RpcError> {
        let stream = self.w.start("db.query.execute", params).await?;
        let (items, outcome) = self.w.collect(stream).await;
        match outcome {
            StreamOutcome::Completed => {
                if let Some(error) = items.iter().find(|i| i["kind"] == "error") {
                    panic!("statement failed: {error}");
                }
                Ok(items)
            }
            StreamOutcome::Failed { error } => Err(error),
            StreamOutcome::Cancelled => panic!("cancelled"),
        }
    }

    /// `SELECT … ORDER BY id` 의 행(셀은 `DbValue` 의 와이어 JSON).
    async fn rows(&mut self, session: &str, columns: &str) -> Vec<Vec<Value>> {
        let sql = format!("SELECT {columns} FROM {} ORDER BY id", self.name);
        self.exec(session, &sql)
            .await
            .iter()
            .filter(|i| i["kind"] == "rows")
            .flat_map(|i| i["rows"].as_array().unwrap().clone())
            .map(|r| r.as_array().unwrap().clone())
            .collect()
    }

    /// (id, name) 쌍.
    async fn names(&mut self, session: &str) -> Vec<(i64, Option<String>)> {
        self.rows(session, "id, name")
            .await
            .into_iter()
            .map(|r| (int(&r[0]), r[1]["text"].as_str().map(str::to_owned)))
            .collect()
    }

    async fn preview(&mut self, session: &str, changes: &Value) -> Result<Value, RpcError> {
        self.w
            .call(
                "db.edit.preview",
                json!({ "sessionId": session, "table": self.table, "changes": changes }),
            )
            .await
    }

    async fn apply(
        &mut self,
        session: &str,
        changes: &Value,
        hash: &Value,
    ) -> Result<Value, RpcError> {
        self.w
            .call(
                "db.edit.apply",
                json!({ "sessionId": session, "table": self.table, "changes": changes, "previewHash": hash }),
            )
            .await
    }

    async fn submit(&mut self, session: &str, changes: &Value) -> Result<Value, RpcError> {
        let p = self.preview(session, changes).await?;
        self.apply(session, changes, &p["previewHash"]).await
    }

    async fn manual(&mut self, session: &str) {
        self.w
            .call(
                "db.session.setMode",
                json!({ "sessionId": session, "mode": "manual" }),
            )
            .await
            .unwrap();
    }

    async fn commit(&mut self, session: &str) {
        self.w
            .call("db.session.commit", json!({ "sessionId": session }))
            .await
            .unwrap();
    }

    async fn cleanup(mut self) {
        let (b, name) = (self.b.clone(), self.name.clone());
        self.exec(&b, &format!("DROP TABLE {name}")).await;
    }
}

fn int(v: &Value) -> i64 {
    v["int"]
        .as_i64()
        .or_else(|| v["int"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or_else(|| panic!("not an int: {v}"))
}

fn text(s: &str) -> Value {
    json!({ "text": s })
}

fn cell(column: &str, value: Value) -> Value {
    json!({ "column": column, "value": value })
}

fn key(id: i64) -> Value {
    json!([cell("id", json!({ "int": id }))])
}

fn named(rows: &[(i64, &str)]) -> Vec<(i64, Option<String>)> {
    rows.iter()
        .map(|(i, n)| (*i, Some((*n).to_owned())))
        .collect()
}

// ── 시나리오 ──────────────────────────────────────────────────────────────────────────────────────────

/// 시나리오 5 의 RPC 순서 그대로: 셀 2개 수정 + 행 1개 추가 + 1개 삭제 → 미리보기 → 적용 → 다시 읽으면 기대값.
async fn edit_applies_two_cells_an_insert_and_a_delete(mut db: Db) {
    let a = db.a.clone();
    let changes = json!({
        "updates": [{ "key": key(1), "values": [cell("name", text("uno")), cell("note", Value::from("null"))] }],
        "inserts": [{ "values": [cell("id", json!({ "int": 3 })), cell("name", text("three"))] }],
        "deletes": [{ "key": key(2) }],
    });
    let p = db.preview(&a, &changes).await.unwrap();
    assert_eq!(p["statements"].as_array().unwrap().len(), 3);
    assert_eq!(p["grade"], "write");
    let applied = db.apply(&a, &changes, &p["previewHash"]).await.unwrap();
    assert_eq!(applied["rowsAffected"], 3, "{applied}");
    let rows = db.rows(&a, "id, name, note").await;
    let got: Vec<(i64, Value, Value)> = rows
        .into_iter()
        .map(|r| (int(&r[0]), r[1].clone(), r[2].clone()))
        .collect();
    assert_eq!(
        got,
        [
            (1, text("uno"), json!("null")),
            (3, text("three"), json!("null")),
        ]
    );
    db.cleanup().await;
}

/// 다른 세션이 그사이 그 행을 지웠다: 영향 행 0 — 편집 전부가 롤백되고 그 변경이 오류에 있다.
async fn edit_rolls_back_when_another_session_deleted_the_row(mut db: Db) {
    let (a, b) = (db.a.clone(), db.b.clone());
    let changes = json!({
        "updates": [{ "key": key(1), "values": [cell("name", text("uno"))] }],
        "inserts": [{ "values": [cell("id", json!({ "int": 3 })), cell("name", text("three"))] }],
        "deletes": [{ "key": key(2) }],
    });
    let p = db.preview(&a, &changes).await.unwrap();
    let name = db.name.clone();
    db.exec(&b, &format!("DELETE FROM {name} WHERE id = 1"))
        .await;
    let err = db.apply(&a, &changes, &p["previewHash"]).await.unwrap_err();
    let data = err.data.unwrap();
    assert_eq!(data["reason"], "rowCount", "{data}");
    assert_eq!(
        data["failure"]["change"],
        json!({ "kind": "update", "index": 0 })
    );
    assert_eq!(data["failure"]["rowsAffected"], 0);
    assert_eq!(data["failure"]["undone"], "edit");
    // 삭제(앞 문장)도 되돌려졌고 추가는 돌지 않았다.
    assert_eq!(db.names(&b).await, named(&[(2, "two")]));
    db.cleanup().await;
}

/// 수동 커밋 탭에서 앞서 한 `INSERT` 가 있을 때 편집이 DB 오류(중복 키)로 실패하면 편집의 세이브포인트까지만 되돌린다 — 그
/// `INSERT` 는 남고, PG 의 실패한 트랜잭션도 되살아나 커밋된다.
async fn edit_failing_in_a_manual_tab_keeps_the_earlier_insert(mut db: Db) {
    let (a, b) = (db.a.clone(), db.b.clone());
    db.manual(&a).await;
    let name = db.name.clone();
    db.exec(
        &a,
        &format!("INSERT INTO {name} (id, name) VALUES (10, 'ten')"),
    )
    .await;
    let changes = json!({
        "updates": [{ "key": key(2), "values": [cell("name", text("deux"))] }],
        "inserts": [{ "values": [cell("id", json!({ "int": 1 })), cell("name", text("dup"))] }],
    });
    let err = db.submit(&a, &changes).await.unwrap_err();
    assert_eq!(err.code, code::DB_ERROR, "{err:?}");
    let data = err.data.unwrap();
    assert_eq!(
        data["failure"]["change"],
        json!({ "kind": "insert", "index": 0 })
    );
    assert_eq!(data["failure"]["undone"], "savepoint");
    assert_eq!(data["session"]["state"]["kind"], "inTransaction", "{data}");
    // 열린 트랜잭션 안: 다른 세션에는 아직 안 보인다.
    assert_eq!(db.names(&b).await, named(&[(1, "one"), (2, "two")]));
    db.commit(&a).await;
    assert_eq!(
        db.names(&b).await,
        named(&[(1, "one"), (2, "two"), (10, "ten")])
    );
    db.cleanup().await;
}

/// 수동 커밋 탭: 적용 뒤 다른 세션에는 안 보이고 커밋 뒤 보인다.
async fn edit_in_a_manual_tab_shows_after_commit(mut db: Db) {
    let (a, b) = (db.a.clone(), db.b.clone());
    db.manual(&a).await;
    let changes = json!({ "updates": [{ "key": key(1), "values": [cell("name", text("uno"))] }] });
    let p = db.preview(&a, &changes).await.unwrap();
    assert_eq!(p["applies"], "openTransaction");
    let applied = db.apply(&a, &changes, &p["previewHash"]).await.unwrap();
    assert_eq!(applied["session"]["state"]["kind"], "inTransaction");
    assert_eq!(db.names(&b).await, named(&[(1, "one"), (2, "two")]));
    assert_eq!(db.names(&a).await, named(&[(1, "uno"), (2, "two")]));
    db.commit(&a).await;
    assert_eq!(db.names(&b).await, named(&[(1, "uno"), (2, "two")]));
    db.cleanup().await;
}

/// 값에 `'; DROP TABLE t; --` 를 넣어도 그 값이 그대로 들어간다(수정 · 추가 · 키).
async fn edit_stores_a_hostile_value_as_it_is(mut db: Db) {
    let a = db.a.clone();
    let hostile = "'; DROP TABLE t; --";
    let changes = json!({
        "updates": [{ "key": key(1), "values": [cell("name", text(hostile))] }],
        "inserts": [{ "values": [cell("id", json!({ "int": 3 })), cell("note", text(hostile))] }],
    });
    db.submit(&a, &changes).await.unwrap();
    let rows = db.rows(&a, "id, name, note").await;
    assert_eq!(rows[0][1], text(hostile));
    assert_eq!(rows[2][2], text(hostile));
    // 키 값으로 와도 값이다(그런 행이 없으니 영향 행 0 — 문장이 깨지지 않는다).
    let by_key = json!({ "deletes": [{ "key": [cell("id", text(hostile))] }] });
    let failed = db.submit(&a, &by_key).await.unwrap_err();
    assert!(
        failed
            .data
            .as_ref()
            .is_some_and(|d| d["reason"] == "rowCount" || d["error"].is_object()),
        "{failed:?}"
    );
    assert_eq!(db.rows(&a, "id").await.len(), 3);
    db.cleanup().await;
}

/// 미리보기 글의 리터럴이 바인드 값과 같다: 같은 값을 바인드로(편집 적용) 한 행, 미리보기 글을 그대로 돌려 한 행 넣고 다시 읽은 두
/// 값이 같다 — 따옴표 · 백슬래시 · 제어 문자 · 유니코드 · 빈 글 · NULL · 바이트(NUL · 따옴표 · 백슬래시 바이트 포함).
async fn edit_preview_literals_equal_the_bound_values(mut db: Db) {
    let (a, b) = (db.a.clone(), db.b.clone());
    let mut texts = vec![
        "it's",
        r"a\b",
        r"\x41\\",
        "line1\nline2\r\n\ttab",
        "",
        "한글 😀 \u{200b}",
        "'; DROP TABLE t; --",
        "\u{7}\u{1b}\u{7f}",
        "$1 ? :name",
    ];
    if db.dialect != SqlDialect::Postgres {
        texts.push("a\0b");
    }
    let mut values: Vec<(Value, Value)> = texts
        .iter()
        .map(|t| (text(t), Value::from("null")))
        .collect();
    values.push((Value::from("null"), Value::from("null")));
    for bytes in ["0x", "0x00275cff", "0x41"] {
        let size = (bytes.len() - 2) / 2;
        values.push((
            Value::from("null"),
            json!({ "bytes": { "hex": bytes, "size": size, "truncated": false } }),
        ));
    }
    for (i, (name, bin)) in values.iter().enumerate() {
        let (bound, literal) = (100 + 2 * i as i64, 101 + 2 * i as i64);
        let insert = |id: i64| json!({ "inserts": [{ "values": [cell("id", json!({ "int": id })), cell("name", name.clone()), cell("bin", bin.clone())] }] });
        db.submit(&a, &insert(bound)).await.unwrap();
        let p = db.preview(&a, &insert(literal)).await.unwrap();
        let sql = p["statements"][0]["sql"].as_str().unwrap().to_owned();
        db.exec(&b, &sql).await;
        let rows = db.rows(&b, "id, name, bin").await;
        let find = |id: i64| rows.iter().find(|r| int(&r[0]) == id).unwrap().clone();
        let (by_bind, by_literal) = (find(bound), find(literal));
        assert_eq!(by_bind[1..], by_literal[1..], "{sql}");
        assert_eq!(by_bind[1], *name, "the bound value changed: {sql}");
        assert_eq!(by_bind[2], *bin, "the bound bytes changed: {sql}");
    }
    db.cleanup().await;
}

/// 값이 그대로인 수정도 그 행을 찾는다(MySQL 은 영향 행 0 을 세므로 잠그는 읽기로 본다).
async fn edit_to_the_same_value_counts_its_row(mut db: Db) {
    let a = db.a.clone();
    let changes = json!({ "updates": [{ "key": key(1), "values": [cell("name", text("one"))] }] });
    let applied = db.submit(&a, &changes).await.unwrap();
    assert_eq!(applied["rowsAffected"], 1);
    assert_eq!(db.names(&a).await, named(&[(1, "one"), (2, "two")]));
    db.cleanup().await;
}

/// 열 이름은 방언의 규칙으로 메타데이터의 정본 이름에 맞춘 뒤 판정한다: MySQL · SQLite 는 `ID` 가 키 열 `id` 라 키 열 수정으로
/// 거절되고(글자 비교면 PK 가 바뀐다), `NAME` 은 `name` 으로 적용된다. PG 는 글자 그대로라 `ID` 는 없는 열이고, 63바이트를 넘는
/// 이름(서버가 잘라 63자 PK 열에 맞춘다)은 받지 않는다. 어느 쪽이든 행은 그대로다.
async fn edit_matches_column_names_by_the_dialect_rules(mut db: Db) {
    let a = db.a.clone();
    let change = |column: &str, value: Value| json!({ "updates": [{ "key": key(1), "values": [cell(column, value)] }] });
    let err = db
        .submit(&a, &change("ID", json!({ "int": 77 })))
        .await
        .unwrap_err();
    let data = err.data.unwrap();
    let want = match db.dialect {
        SqlDialect::Postgres => "unknownColumn",
        SqlDialect::Mysql | SqlDialect::Sqlite => "keyColumnChanged",
    };
    assert_eq!(data["reason"], want, "{data}");
    assert_eq!(db.names(&a).await, named(&[(1, "one"), (2, "two")]));
    let renamed = db.submit(&a, &change("NAME", text("uno"))).await;
    if db.dialect == SqlDialect::Postgres {
        assert_eq!(
            renamed.unwrap_err().data.unwrap()["reason"],
            "unknownColumn"
        );
    } else {
        renamed.unwrap();
        assert_eq!(db.names(&a).await, named(&[(1, "uno"), (2, "two")]));
    }
    if db.dialect == SqlDialect::Postgres {
        let long = "a".repeat(63);
        let table = format!("{}_long", db.name);
        db.exec(
            &a,
            &format!("CREATE TABLE {table} (\"{long}\" INTEGER NOT NULL PRIMARY KEY, v INTEGER)"),
        )
        .await;
        db.exec(&a, &format!("INSERT INTO {table} VALUES (1, 5)"))
            .await;
        let mut t = db.table.clone();
        t["name"] = json!(table);
        let changes = json!({ "updates": [{ "key": [cell(&long, json!({ "int": 1 }))],
                                            "values": [cell(&format!("{long}X"), json!({ "int": 77 }))] }] });
        let err =
            db.w.call(
                "db.edit.preview",
                json!({ "sessionId": a, "table": t, "changes": changes }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.data.unwrap()["reason"], "nameTooLong");
        let rows = db
            .exec(&a, &format!("SELECT \"{long}\" FROM {table}"))
            .await;
        let row = rows.iter().find(|i| i["kind"] == "rows").unwrap()["rows"][0][0].clone();
        assert_eq!(int(&row), 1);
        db.exec(&a, &format!("DROP TABLE {table}")).await;
    }
    db.cleanup().await;
}

/// 시나리오마다 시험 함수 하나(이름은 그 DB 의 접두어로 시작한다 — 실 DB 시험의 필터가 이름 조각의 처음을 본다).
macro_rules! scenarios {
    ($open:ident; $($name:ident => $scenario:ident),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn $name() {
                $scenario($open().await).await
            }
        )*
    };
}

mod sqlite {
    use super::*;

    async fn open() -> Db {
        host(
            Arc::new(pw_db_driver_sqlite::SqliteDriver::new()),
            SqlDialect::Sqlite,
            "path = \"app.db\"\n",
            None,
            |name| json!({ "name": name }),
            |f| std::fs::write(f.root.join("app.db"), "").unwrap(),
        )
        .await
    }

    scenarios!(open;
        sqlite_edit_applies_two_cells_an_insert_and_a_delete => edit_applies_two_cells_an_insert_and_a_delete,
        sqlite_edit_rolls_back_when_another_session_deleted_the_row => edit_rolls_back_when_another_session_deleted_the_row,
        sqlite_edit_failing_in_a_manual_tab_keeps_the_earlier_insert => edit_failing_in_a_manual_tab_keeps_the_earlier_insert,
        sqlite_edit_in_a_manual_tab_shows_after_commit => edit_in_a_manual_tab_shows_after_commit,
        sqlite_edit_stores_a_hostile_value_as_it_is => edit_stores_a_hostile_value_as_it_is,
        sqlite_edit_preview_literals_equal_the_bound_values => edit_preview_literals_equal_the_bound_values,
        sqlite_edit_to_the_same_value_counts_its_row => edit_to_the_same_value_counts_its_row,
        sqlite_edit_matches_column_names_by_the_dialect_rules => edit_matches_column_names_by_the_dialect_rules,
    );
}

/// URL(`scheme://user:password@host:port/db`)의 사용자 · DB.
#[cfg(any(pw_live_pg, pw_live_mysql))]
fn user_and_database(url: &str) -> (String, String) {
    let rest = url.split_once("://").unwrap().1;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let user = authority
        .rsplit_once('@')
        .map(|(info, _)| info.split(':').next().unwrap().to_owned())
        .unwrap();
    (
        user,
        path.split(['?', '#']).next().unwrap_or_default().to_owned(),
    )
}

#[cfg(pw_live_pg)]
mod pg {
    use pw_db_driver::live::{LiveDb, endpoint};

    use super::*;

    async fn open() -> Db {
        let url = LiveDb::Pg.url();
        let end = endpoint(LiveDb::Pg, &url).unwrap();
        let (user, database) = user_and_database(&url);
        let db = database.clone();
        host(
            Arc::new(pw_db_driver_pg::PgDriver::new()),
            SqlDialect::Postgres,
            &format!(
                "host = \"{}\"\nport = {}\nuser = \"{user}\"\ndatabase = \"{database}\"\nsslmode = \"disable\"\npassword = {{ secret_ref = \"keychain\" }}\n",
                end.host, end.port
            ),
            end.password.as_deref(),
            move |name| json!({ "database": db, "schema": "public", "name": name }),
            |_| {},
        )
        .await
    }

    scenarios!(open;
        live_pg_edit_applies_two_cells_an_insert_and_a_delete => edit_applies_two_cells_an_insert_and_a_delete,
        live_pg_edit_rolls_back_when_another_session_deleted_the_row => edit_rolls_back_when_another_session_deleted_the_row,
        live_pg_edit_failing_in_a_manual_tab_keeps_the_earlier_insert => edit_failing_in_a_manual_tab_keeps_the_earlier_insert,
        live_pg_edit_in_a_manual_tab_shows_after_commit => edit_in_a_manual_tab_shows_after_commit,
        live_pg_edit_stores_a_hostile_value_as_it_is => edit_stores_a_hostile_value_as_it_is,
        live_pg_edit_preview_literals_equal_the_bound_values => edit_preview_literals_equal_the_bound_values,
        live_pg_edit_to_the_same_value_counts_its_row => edit_to_the_same_value_counts_its_row,
        live_pg_edit_matches_column_names_by_the_dialect_rules => edit_matches_column_names_by_the_dialect_rules,
    );
}

#[cfg(pw_live_mysql)]
mod mysql {
    use pw_db_driver::live::{LiveDb, endpoint};

    use super::*;

    async fn open() -> Db {
        let url = LiveDb::Mysql.url();
        let end = endpoint(LiveDb::Mysql, &url).unwrap();
        let (user, database) = user_and_database(&url);
        let db = database.clone();
        host(
            Arc::new(pw_db_driver_mysql::MysqlDriver::new()),
            SqlDialect::Mysql,
            &format!(
                "host = \"{}\"\nport = {}\nuser = \"{user}\"\ndatabase = \"{database}\"\nsslmode = \"disable\"\npassword = {{ secret_ref = \"keychain\" }}\n",
                end.host, end.port
            ),
            end.password.as_deref(),
            move |name| json!({ "database": db, "name": name }),
            |_| {},
        )
        .await
    }

    scenarios!(open;
        live_mysql_edit_applies_two_cells_an_insert_and_a_delete => edit_applies_two_cells_an_insert_and_a_delete,
        live_mysql_edit_rolls_back_when_another_session_deleted_the_row => edit_rolls_back_when_another_session_deleted_the_row,
        live_mysql_edit_failing_in_a_manual_tab_keeps_the_earlier_insert => edit_failing_in_a_manual_tab_keeps_the_earlier_insert,
        live_mysql_edit_in_a_manual_tab_shows_after_commit => edit_in_a_manual_tab_shows_after_commit,
        live_mysql_edit_stores_a_hostile_value_as_it_is => edit_stores_a_hostile_value_as_it_is,
        live_mysql_edit_preview_literals_equal_the_bound_values => edit_preview_literals_equal_the_bound_values,
        live_mysql_edit_to_the_same_value_counts_its_row => edit_to_the_same_value_counts_its_row,
        live_mysql_edit_matches_column_names_by_the_dialect_rules => edit_matches_column_names_by_the_dialect_rules,
        live_mysql_edit_refuses_an_update_whose_key_matches_several_rows => edit_refuses_an_update_whose_key_matches_several_rows,
    );

    /// MySQL · MariaDB 의 영향 행은 바뀐 행이다: 숫자꼴 VARCHAR 키에 정수 키 값을 보내면 `'1'` · `'01'` · `'1.0'` 세 행에 맞고 하나만 값이
    /// 달라 영향 행이 1 이다. 수정 앞의 잠그는 읽기가 2행 이상을 보고 거절해야 하고(전부 롤백) 세 행은 그대로다.
    async fn edit_refuses_an_update_whose_key_matches_several_rows(mut db: Db) {
        let a = db.a.clone();
        let table = format!("{}_codes", db.name);
        db.exec(
            &a,
            &format!("CREATE TABLE {table} (code VARCHAR(10) NOT NULL PRIMARY KEY, v INT)"),
        )
        .await;
        db.exec(
            &a,
            &format!("INSERT INTO {table} VALUES ('1', 5), ('01', 9), ('1.0', 5)"),
        )
        .await;
        let mut t = db.table.clone();
        t["name"] = json!(table);
        let changes = json!({ "updates": [{ "key": [cell("code", json!({ "int": 1 }))],
                                            "values": [cell("v", json!({ "int": 5 }))] }] });
        let p =
            db.w.call(
                "db.edit.preview",
                json!({ "sessionId": a, "table": t, "changes": changes }),
            )
            .await
            .unwrap();
        let err = db
            .w
            .call(
                "db.edit.apply",
                json!({ "sessionId": a, "table": t, "changes": changes, "previewHash": p["previewHash"] }),
            )
            .await
            .unwrap_err();
        let data = err.data.unwrap();
        assert_eq!(data["reason"], "rowCount", "{data}");
        assert_eq!(data["failure"]["rowsAffected"], 2);
        let rows = db
            .exec(&a, &format!("SELECT code, v FROM {table} ORDER BY code"))
            .await;
        let got: Vec<(String, i64)> = rows
            .iter()
            .filter(|i| i["kind"] == "rows")
            .flat_map(|i| i["rows"].as_array().unwrap().clone())
            .map(|r| (r[0]["text"].as_str().unwrap().to_owned(), int(&r[1])))
            .collect();
        assert_eq!(
            got,
            [
                ("01".to_owned(), 9),
                ("1".to_owned(), 5),
                ("1.0".to_owned(), 5)
            ]
        );
        db.exec(&a, &format!("DROP TABLE {table}")).await;
        db.cleanup().await;
    }
}
