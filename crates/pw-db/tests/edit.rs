//! M3-12 의 Host 쪽(14-m3-plan.md M3-12 행): 가짜 드라이버로 `db.edit.*` 를 실제 RPC 세션 너머에서 본다 — 관문(다른 프로젝트 창
//! · 웰컴 · 파일 창 · 미신뢰 · read-only 는 드라이버에 닿지 않는다), 미리보기 해시(다른 변경 · 끼어든 실행 · 두 번째 적용은
//! `previewChanged`), 자동 커밋 탭의 한 트랜잭션(영향 행이 1 이 아니면 전부 롤백), 수동 커밋 탭의 세이브포인트(앞선 변경은 남는다),
//! MySQL 의 「값이 그대로인 수정」. 실 DB 의 같은 흐름은 `edit_live.rs` · `edit_sqlite.rs`.

mod common;

use std::sync::Arc;

use common::{Conn, Fixture};
use pw_db::DbSessions;
use pw_db::gate::code;
use pw_db_driver::fake::{FakeCall, FakeDriver, FakeResponse, FakeTable};
use pw_db_types::{ColumnInfo, DbError, DbErrorKind, IndexInfo, SqlDialect};
use pw_rpc::protocol::StreamOutcome;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

struct Env {
    f: Fixture,
    db: FakeDriver,
}

fn env_with(trusted: bool, id: &str, dialect: SqlDialect) -> Env {
    let db = FakeDriver::named(id, dialect);
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(db.clone())).unwrap();
    let f = Fixture::build(trusted, |router, connections| {
        pw_db::register(
            router,
            Arc::new(DbSessions::new(Arc::clone(connections), drivers)),
        );
    });
    for (file, extra) in [
        ("dev.toml", ""),
        ("ro.toml", "read_only = true\n"),
        ("prod.toml", "production = true\n"),
    ] {
        let name = file.trim_end_matches(".toml");
        f.write_db(
            file,
            &format!(
                "schema = 1\nname = \"{name}\"\ndriver = \"{id}\"\nhost = \"localhost\"\n{extra}"
            ),
        );
    }
    db.set_table("t", keyed());
    db.set_table(
        "loose",
        FakeTable {
            columns: vec![column("v", true)],
            ..FakeTable::default()
        },
    );
    for (needle, affected) in [("DELETE", 1), ("UPDATE", 1), ("INSERT", 1)] {
        db.respond(needle, FakeResponse::write(affected));
    }
    Env { f, db }
}

fn env(trusted: bool) -> Env {
    env_with(trusted, "postgres", SqlDialect::Postgres)
}

fn column(name: &str, nullable: bool) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        db_type: "int8".into(),
        nullable,
        default: None,
        comment: None,
    }
}

/// `id` PK + `name` · `note`.
fn keyed() -> FakeTable {
    FakeTable {
        columns: vec![
            column("id", false),
            column("name", true),
            column("note", true),
        ],
        indexes: vec![IndexInfo {
            name: "t_pkey".into(),
            columns: vec!["id".into()],
            primary: true,
            unique: true,
            partial: false,
        }],
        ..FakeTable::default()
    }
}

fn table() -> Value {
    json!({ "database": "app", "schema": "public", "name": "t" })
}

/// 셀 둘 수정 + 추가 하나 + 삭제 하나(완료 기준의 시나리오).
fn changes() -> Value {
    json!({
        "updates": [{ "key": [{ "column": "id", "value": { "int": 1 } }],
                      "values": [{ "column": "name", "value": { "text": "a'; DROP TABLE t; --" } },
                                 { "column": "note", "value": "null" }] }],
        "inserts": [{ "values": [{ "column": "id", "value": { "int": 3 } },
                                 { "column": "name", "value": { "text": "c" } }] }],
        "deletes": [{ "key": [{ "column": "id", "value": { "int": 2 } }] }],
    })
}

const DELETE: &str = r#"DELETE FROM "app"."public"."t" WHERE "id" = $1"#;
const UPDATE: &str = r#"UPDATE "app"."public"."t" SET "name" = $1, "note" = $2 WHERE "id" = $3"#;
const INSERT: &str = r#"INSERT INTO "app"."public"."t" ("id", "name") VALUES ($1, $2)"#;

impl Env {
    async fn open(&self, w: &mut Conn, connection: &str) -> String {
        let opened = w
            .call(
                "db.session.open",
                json!({ "projectId": self.f.id, "connectionId": connection }),
            )
            .await
            .unwrap();
        opened["session"]["sessionId"].as_str().unwrap().to_owned()
    }

    /// 세션 연결(0번)에 닿은 문장 · 트랜잭션 호출.
    fn session_calls(&self) -> Vec<FakeCall> {
        self.db
            .calls_of(0)
            .into_iter()
            .filter(|c| !matches!(c, FakeCall::Connect | FakeCall::ReadTransactionState))
            .collect()
    }

    /// 연결 · 닫기를 뺀 드라이버 호출(모든 연결 — 메타데이터 포함).
    fn reached(&self) -> Vec<FakeCall> {
        self.db
            .calls()
            .into_iter()
            .filter(|c| !matches!(c, FakeCall::Connect | FakeCall::Close))
            .collect()
    }
}

async fn preview(w: &mut Conn, session: &str, changes: Value) -> Result<Value, RpcError> {
    w.call(
        "db.edit.preview",
        json!({ "sessionId": session, "table": table(), "changes": changes }),
    )
    .await
}

async fn apply(
    w: &mut Conn,
    session: &str,
    changes: Value,
    hash: &Value,
) -> Result<Value, RpcError> {
    w.call(
        "db.edit.apply",
        json!({ "sessionId": session, "table": table(), "changes": changes, "previewHash": hash }),
    )
    .await
}

/// 미리보기 → 적용.
async fn submit(w: &mut Conn, session: &str, changes: Value) -> Result<Value, RpcError> {
    let p = preview(w, session, changes.clone()).await?;
    apply(w, session, changes, &p["previewHash"]).await
}

async fn run(w: &mut Conn, session: &str, sql: &str) -> Vec<Value> {
    let stream = w
        .start(
            "db.query.execute",
            json!({ "sessionId": session, "sql": sql }),
        )
        .await
        .unwrap();
    let (items, outcome) = w.collect(stream).await;
    assert!(
        matches!(outcome, StreamOutcome::Completed),
        "{sql}: {outcome:?}"
    );
    items
}

fn reason(e: &RpcError) -> Value {
    e.data.as_ref().map_or(Value::Null, |d| d["reason"].clone())
}

/// 미리보기: 삭제 → 수정 → 추가 순, 값은 리터럴(따옴표를 두 번), 등급은 쓰기, 적용 방식은 자동 커밋 탭의 자기 트랜잭션. 실행하지
/// 않는다(세션 연결에 문장이 닿지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_preview_shows_each_statement_with_literals_and_runs_nothing() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let p = preview(&mut w, &s, changes()).await.unwrap();
    let texts: Vec<&str> = p["statements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["sql"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        [
            r#"DELETE FROM "app"."public"."t" WHERE "id" = 2"#,
            r#"UPDATE "app"."public"."t" SET "name" = 'a''; DROP TABLE t; --', "note" = NULL WHERE "id" = 1"#,
            r#"INSERT INTO "app"."public"."t" ("id", "name") VALUES (3, 'c')"#,
        ]
    );
    assert_eq!(
        p["statements"][1]["change"],
        json!({ "kind": "update", "index": 0 })
    );
    assert_eq!(p["grade"], "write");
    assert_eq!(p["key"], json!(["id"]));
    assert_eq!(p["applies"], "ownTransaction");
    assert_eq!(p["previewHash"].as_str().unwrap().len(), 64);
    assert_eq!(e.session_calls(), vec![], "the preview ran something");
}

/// 적용(자동 커밋 탭): 한 트랜잭션 안에서 바인드 값의 문장 셋 → 커밋. 값은 글에 이어 붙지 않는다(`DROP` 이 글에 없다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applying_runs_the_bound_statements_in_one_transaction() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let applied = submit(&mut w, &s, changes()).await.unwrap();
    assert_eq!(applied["statements"], 3);
    assert_eq!(applied["rowsAffected"], 3);
    assert_eq!(applied["session"]["state"]["kind"], "idle");
    assert_eq!(
        e.session_calls(),
        vec![
            FakeCall::Begin { read_only: false },
            FakeCall::Execute(DELETE.into()),
            FakeCall::Execute(UPDATE.into()),
            FakeCall::Execute(INSERT.into()),
            FakeCall::Commit,
        ]
    );
    assert_eq!(e.db.committed(), vec![DELETE, UPDATE, INSERT]);
}

/// 다른 세션이 그사이 그 행을 지웠다(영향 행 0): 전부 롤백하고 그 변경이 오류에 있다. 커밋하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_gone_meanwhile_rolls_the_whole_edit_back() {
    let e = env(true);
    e.db.respond("UPDATE", FakeResponse::write(0));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let err = submit(&mut w, &s, changes()).await.unwrap_err();
    assert_eq!(err.code, rpc_code::CONFLICT, "{err:?}");
    let data = err.data.unwrap();
    assert_eq!(data["reason"], "rowCount");
    assert_eq!(
        data["failure"],
        json!({ "statement": 1, "change": { "kind": "update", "index": 0 }, "rowsAffected": 0, "undone": "edit" })
    );
    assert_eq!(data["session"]["state"]["kind"], "idle");
    assert_eq!(
        e.session_calls(),
        vec![
            FakeCall::Begin { read_only: false },
            FakeCall::Execute(DELETE.into()),
            FakeCall::Execute(UPDATE.into()),
            FakeCall::Rollback,
        ]
    );
    assert_eq!(e.db.committed(), Vec::<String>::new());
}

/// DB 오류(제약 위반 등)도 전부 롤백하고 `3003` 에 그 오류 · 변경을 싣는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_db_error_rolls_the_whole_edit_back() {
    let e = env(true);
    e.db.respond(
        "INSERT",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "duplicate key")),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let err = submit(&mut w, &s, changes()).await.unwrap_err();
    assert_eq!(err.code, code::DB_ERROR, "{err:?}");
    let data = err.data.unwrap();
    assert_eq!(data["error"]["message"], "duplicate key");
    assert_eq!(
        data["failure"]["change"],
        json!({ "kind": "insert", "index": 0 })
    );
    assert_eq!(data["failure"]["undone"], "edit");
    assert_eq!(e.session_calls().last(), Some(&FakeCall::Rollback));
    assert_eq!(e.db.committed(), Vec::<String>::new());
}

/// 미리보기와 다른 변경 · 끼어든 실행 · 같은 해시의 두 번째 적용은 `previewChanged` 이고 드라이버에 문장이 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_runs_only_the_previewed_statements_once() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;

    // 값 하나를 바꾼 변경.
    let p = preview(&mut w, &s, changes()).await.unwrap();
    let mut other = changes();
    other["updates"][0]["values"][0]["value"] = json!({ "text": "b" });
    let err = apply(&mut w, &s, other, &p["previewHash"])
        .await
        .unwrap_err();
    assert_eq!(
        (err.code, reason(&err)),
        (rpc_code::CONFLICT, json!("previewChanged"))
    );
    // 다른 세션의 해시.
    let s2 = e.open(&mut w, "dev").await;
    let err = apply(&mut w, &s2, changes(), &p["previewHash"])
        .await
        .unwrap_err();
    assert_eq!(reason(&err), json!("previewChanged"));
    assert!(e.session_calls().is_empty());

    // 미리보기 뒤 콘솔 실행이 끼었다.
    run(&mut w, &s, "SELECT 1").await;
    let before = e.session_calls().len();
    let err = apply(&mut w, &s, changes(), &p["previewHash"])
        .await
        .unwrap_err();
    assert_eq!(reason(&err), json!("previewChanged"));
    assert_eq!(e.session_calls().len(), before);

    // 한 번 적용한 해시는 다시 쓰이지 않는다.
    let p = preview(&mut w, &s, changes()).await.unwrap();
    apply(&mut w, &s, changes(), &p["previewHash"])
        .await
        .unwrap();
    let after = e.session_calls().len();
    let err = apply(&mut w, &s, changes(), &p["previewHash"])
        .await
        .unwrap_err();
    assert_eq!(reason(&err), json!("previewChanged"));
    assert_eq!(e.session_calls().len(), after);
}

/// 수동 커밋 탭: 앞서 한 `INSERT` 가 있는 트랜잭션 안에서 편집이 실패하면 편집의 세이브포인트까지만 되돌린다 — 앞의 `INSERT` 는
/// 남고 커밋하면 그것만 커밋된다. 성공한 편집은 커밋 전까지 트랜잭션 안에 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_manual_tab_undoes_only_the_edit_savepoint() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    w.call(
        "db.session.setMode",
        json!({ "sessionId": s, "mode": "manual" }),
    )
    .await
    .unwrap();
    let earlier = "INSERT INTO other VALUES (1)";
    run(&mut w, &s, earlier).await;

    e.db.respond("UPDATE", FakeResponse::write(0));
    let p = preview(&mut w, &s, changes()).await.unwrap();
    assert_eq!(p["applies"], "openTransaction");
    let err = apply(&mut w, &s, changes(), &p["previewHash"])
        .await
        .unwrap_err();
    let data = err.data.unwrap();
    assert_eq!(data["failure"]["undone"], "savepoint");
    assert_eq!(data["session"]["state"]["kind"], "inTransaction");
    let calls = e.session_calls();
    let savepoint = calls
        .iter()
        .find_map(|c| match c {
            FakeCall::Savepoint(name) => Some(name.clone()),
            _ => None,
        })
        .unwrap();
    assert!(calls.contains(&FakeCall::RollbackToSavepoint(savepoint.clone())));
    assert!(!calls.contains(&FakeCall::Rollback), "{calls:?}");

    // 성공한 편집은 열린 트랜잭션 안에 있다 — 커밋은 사람이 한다.
    e.db.respond("UPDATE", FakeResponse::write(1));
    let applied = submit(&mut w, &s, changes()).await.unwrap();
    assert_eq!(applied["session"]["state"]["kind"], "inTransaction");
    assert_eq!(e.db.committed(), Vec::<String>::new());
    w.call("db.session.commit", json!({ "sessionId": s }))
        .await
        .unwrap();
    assert_eq!(e.db.committed(), vec![earlier, DELETE, UPDATE, INSERT]);
}

/// 프로덕션 연결: 편집은 쓰기라 확인이 필요하지만 미리보기의 [적용] 이 그 확인이다 — `confirmed` 없이 해시로 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_production_connection_takes_the_preview_as_the_confirmation() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "prod").await;
    submit(&mut w, &s, changes()).await.unwrap();
    assert_eq!(e.db.committed(), vec![DELETE, UPDATE, INSERT]);
}

/// read-only 연결은 미리보기 · 적용 모두 거절하고 드라이버(메타데이터 포함)에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_connection_refuses_edits_before_the_driver() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "ro").await;
    let err = preview(&mut w, &s, changes()).await.unwrap_err();
    assert_eq!(
        (err.code, reason(&err)),
        (rpc_code::PERMISSION_DENIED, json!("readOnlyConnection"))
    );
    let err = apply(&mut w, &s, changes(), &json!("0".repeat(64)))
        .await
        .unwrap_err();
    assert_eq!(reason(&err), json!("readOnlyConnection"));
    assert_eq!(e.reached(), vec![]);
}

/// 다른 프로젝트 창 · 웰컴(Host 범위) · 파일 창의 `db.edit.*` 는 `PERMISSION_DENIED` 이고 드라이버가 불리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_windows_cannot_edit_through_a_session() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let hash = preview(&mut w, &s, changes()).await.unwrap()["previewHash"].clone();
    let before = e.reached();
    let file = e.f.root.join("a.txt");
    std::fs::write(&file, "").unwrap();
    for scope in [
        ConnectionScope::Project(e.f.other.clone()),
        ConnectionScope::Host,
        pw_rpc::Caller::file(pw_rpc::Role::Owner, file.clone()).scope,
    ] {
        let mut other = e.f.connect(scope.clone());
        for (method, params) in [
            (
                "db.edit.rowKey",
                json!({ "sessionId": s, "table": table() }),
            ),
            (
                "db.edit.preview",
                json!({ "sessionId": s, "table": table(), "changes": changes() }),
            ),
            (
                "db.edit.apply",
                json!({ "sessionId": s, "table": table(), "changes": changes(), "previewHash": hash }),
            ),
        ] {
            let err = other.call(method, params).await.unwrap_err();
            assert_eq!(
                err.code,
                rpc_code::PERMISSION_DENIED,
                "{scope:?} {method}: {err:?}"
            );
            if !matches!(scope, ConnectionScope::File(_)) {
                assert_eq!(
                    reason(&err),
                    json!("notProjectWindow"),
                    "{scope:?} {method}"
                );
            }
        }
    }
    assert_eq!(e.reached(), before, "another window reached the driver");
}

/// 신뢰를 거두면 열린 세션의 `db.edit.*` 도 드라이버에 닿기 전에 `untrusted` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_cannot_edit() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    e.f.projects.set_trust(&e.f.id, false).unwrap();
    for (method, params) in [
        (
            "db.edit.rowKey",
            json!({ "sessionId": s, "table": table() }),
        ),
        (
            "db.edit.preview",
            json!({ "sessionId": s, "table": table(), "changes": changes() }),
        ),
        (
            "db.edit.apply",
            json!({ "sessionId": s, "table": table(), "changes": changes(), "previewHash": "x" }),
        ),
    ] {
        let err = w.call(method, params).await.unwrap_err();
        assert_eq!(reason(&err), json!("untrusted"), "{method}: {err:?}");
    }
    assert_eq!(e.reached(), vec![]);
}

/// 행 키: 키 있는 테이블은 그 열, 키 없는 테이블은 `null` 이고 미리보기는 `noRowKey` 다. 키 열을 고치는 수정 · 한정하지 않은
/// 테이블도 거절(문장이 닿지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tables_without_a_key_and_key_edits_are_refused() {
    let e = env(true);
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let key = w
        .call(
            "db.edit.rowKey",
            json!({ "sessionId": s, "table": table() }),
        )
        .await
        .unwrap();
    assert_eq!(
        key,
        json!({ "key": { "index": "t_pkey", "columns": ["id"], "primary": true } })
    );
    let loose = json!({ "database": "app", "schema": "public", "name": "loose" });
    let key = w
        .call("db.edit.rowKey", json!({ "sessionId": s, "table": loose }))
        .await
        .unwrap();
    assert_eq!(key, json!({ "key": null }));
    let err = w
        .call(
            "db.edit.preview",
            json!({ "sessionId": s, "table": loose, "changes": changes() }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (err.code, reason(&err)),
        (rpc_code::INVALID_PARAMS, json!("noRowKey"))
    );

    let edits_key = json!({ "updates": [{ "key": [{ "column": "id", "value": { "int": 1 } }],
                                         "values": [{ "column": "id", "value": { "int": 9 } }] }] });
    let err = preview(&mut w, &s, edits_key).await.unwrap_err();
    assert_eq!(reason(&err), json!("keyColumnChanged"));
    assert_eq!(
        err.data.unwrap()["change"],
        json!({ "kind": "update", "index": 0 })
    );

    let err = w
        .call(
            "db.edit.preview",
            json!({ "sessionId": s, "table": { "name": "t" }, "changes": changes() }),
        )
        .await
        .unwrap_err();
    assert_eq!(reason(&err), json!("unqualifiedTable"));
    assert_eq!(e.session_calls(), vec![]);
}

/// MySQL: 값이 그대로인 수정은 영향 행이 0 이다 — 같은 키의 잠그는 읽기로 행이 있으면 성공, 없으면(지워졌으면) 전부 롤백.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_updates_that_change_nothing_still_find_their_row() {
    let e = env_with(true, "mysql", SqlDialect::Mysql);
    e.db.respond("UPDATE", FakeResponse::write(0));
    e.db.respond("SELECT 1 FROM", FakeResponse::rows(1));
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    let table = json!({ "database": "app", "name": "t" });
    let update = json!({ "updates": [{ "key": [{ "column": "id", "value": { "int": 1 } }],
                                       "values": [{ "column": "name", "value": { "text": "same" } }] }] });
    let call = |method: &str, hash: Option<&Value>| {
        let mut params = json!({ "sessionId": s, "table": table, "changes": update });
        if let Some(hash) = hash {
            params["previewHash"] = hash.clone();
        }
        (method.to_owned(), params)
    };
    let (m, p) = call("db.edit.preview", None);
    let hash = w.call(&m, p).await.unwrap()["previewHash"].clone();
    let (m, p) = call("db.edit.apply", Some(&hash));
    let applied = w.call(&m, p).await.unwrap();
    assert_eq!(applied["rowsAffected"], 1);
    let probe = "SELECT 1 FROM `app`.`t` WHERE `id` = ? FOR UPDATE";
    assert!(e.session_calls().contains(&FakeCall::Execute(probe.into())));
    assert_eq!(e.session_calls().last(), Some(&FakeCall::Commit));

    // 행이 없다.
    e.db.respond("SELECT 1 FROM", FakeResponse::rows(0));
    let (m, p) = call("db.edit.preview", None);
    let hash = w.call(&m, p).await.unwrap()["previewHash"].clone();
    let (m, p) = call("db.edit.apply", Some(&hash));
    let err = w.call(&m, p).await.unwrap_err();
    assert_eq!(reason(&err), json!("rowCount"));
    assert_eq!(e.session_calls().last(), Some(&FakeCall::Rollback));
}

/// 실패한 트랜잭션(PG)에는 세이브포인트를 걸지 않는다 — 미리보기 · 적용 모두 `transactionFailed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_transaction_takes_no_edit() {
    let e = env(true);
    e.db.respond(
        "BOOM",
        FakeResponse::error(DbError::new(DbErrorKind::Server, "boom")),
    );
    let mut w = e.f.window();
    let s = e.open(&mut w, "dev").await;
    w.call(
        "db.session.setMode",
        json!({ "sessionId": s, "mode": "manual" }),
    )
    .await
    .unwrap();
    run(&mut w, &s, "SELECT 1").await;
    let stream = w
        .start(
            "db.query.execute",
            json!({ "sessionId": s, "sql": "SELECT BOOM" }),
        )
        .await
        .unwrap();
    w.collect(stream).await;
    // 가짜는 오류 뒤 트랜잭션을 실패로 둔다(PG).
    let err = preview(&mut w, &s, changes()).await.unwrap_err();
    assert_eq!(reason(&err), json!("transactionFailed"), "{err:?}");
}
