//! M3-02 의 완료 기준(14-m3-plan.md M3-02 행): 연결 설정 파일 왕복 · 비밀의 대상 묶기 · `env` 참조 확인 · 신뢰 게이트 · 루트 밖
//! SQLite · 쓰기의 연결 범위 · `host_ref` 거절, 그리고 `config.changed` · 활성 워크트리 · 초안 풀기. 키체인은 가짜 저장소, 런타임
//! 환경은 가짜다. 비밀 값이 로그 파일 · 응답에 없는 것(⑦)은 `secret_log.rs` 가 본다(전역 로그 구독자를 따로 설치한다).

mod common;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use common::{Fixture, REMOTE_PG, fake_worktree, target_of};
use pw_core::bus::{Filter, Received, Scope};
use pw_core::registry::NewProject;
use pw_db::config::DbConnectionConfig;
use pw_db::secret::{DbSecretKey, account};
use pw_db::trust::code;
use pw_db_types::{Secret, Target};
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};
use serde_json::{Value, json};

fn upsert_params(f: &Fixture, id: &str, config: Value) -> Value {
    json!({ "projectId": f.id, "connectionId": id, "config": config })
}

fn remote_config() -> Value {
    json!({
        "name": "prod pg",
        "driver": "postgres",
        "host": "db.internal",
        "port": 5432,
        "database": "billing",
        "user": "app",
        "password": { "kind": "keychain" },
        "production": true,
        "sslmode": "verify-full",
        "sshTunnel": { "hostRef": "bastion" },
    })
}

fn reason(e: &RpcError) -> Option<&str> {
    e.data.as_ref()?.get("reason")?.as_str()
}

fn data(e: &RpcError) -> &Value {
    e.data.as_ref().expect("error data")
}

fn file_path(spec: &pw_db_types::ConnectionSpec) -> PathBuf {
    match &spec.target {
        Target::File { path } => path.clone(),
        other => panic!("not a file target: {other:?}"),
    }
}

/// ① 설정 파일 왕복: upsert → 파일 → list. 파일에는 `secret_ref` 만 있고, 비밀을 넣은 뒤에도 파일 · 목록에 값이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_round_trip_keeps_only_the_secret_ref() {
    let f = Fixture::new(true);
    let mut w = f.window();
    let entry = w
        .call(
            "db.connection.upsert",
            upsert_params(&f, "prod", remote_config()),
        )
        .await
        .unwrap();
    assert_eq!(entry["config"]["password"], json!({ "kind": "keychain" }));
    // 터널 목적지는 처음 쓸 때 묻는다(M3-11) — 확인한 뒤에 비밀번호를 묻는다.
    assert_eq!(entry["state"]["kind"], "approvalRequired", "{entry}");
    assert_eq!(entry["state"]["request"]["kind"], "tunnel", "{entry}");
    w.approve_tunnel(&f.id, "prod").await.unwrap();
    let entry = w
        .call(
            "db.connection.get",
            json!({ "projectId": f.id, "connectionId": "prod" }),
        )
        .await
        .unwrap();
    assert_eq!(entry["state"]["kind"], "secretRequired", "{entry}");
    assert_eq!(entry["state"]["reason"], "missing", "{entry}");
    assert_eq!(entry["state"]["target"], target_of(&entry["config"]));

    let text = fs::read_to_string(f.db_file("prod.toml")).unwrap();
    assert!(text.starts_with("schema = 1\n"), "{text}");
    assert!(
        text.contains(r#"password = { secret_ref = "keychain" }"#),
        "{text}"
    );
    assert!(
        text.contains("[ssh_tunnel]\nhost_ref = \"bastion\""),
        "{text}"
    );

    w.set_password(&f.id, "prod", "s3cret-hunter2")
        .await
        .unwrap();
    let text = fs::read_to_string(f.db_file("prod.toml")).unwrap();
    assert!(!text.contains("s3cret-hunter2"), "{text}");

    let list = w
        .call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    assert!(!list.to_string().contains("s3cret-hunter2"), "{list}");
    let connections = list["connections"].as_array().unwrap();
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0]["id"], "prod");
    assert_eq!(connections[0]["config"], entry["config"]);
    assert_eq!(connections[0]["layers"], json!(["shared"]));
    assert_eq!(connections[0]["state"], json!({ "kind": "ready" }));
    assert_eq!(list["trusted"], true);
    assert_eq!(list["stateProblem"], Value::Null);

    // 손으로 단 주석(파일 머리 · 값 뒤)은 다시 써도 남는다.
    fs::write(
        f.db_file("prod.toml"),
        format!("# owned by the platform team\n{text}")
            .replace("port = 5432", "port = 5432 # pgbouncer"),
    )
    .unwrap();
    let mut local = remote_config();
    local["user"] = json!("me");
    w.call(
        "db.connection.upsert",
        upsert_params(&f, "prod", remote_config()),
    )
    .await
    .unwrap();
    let text = fs::read_to_string(f.db_file("prod.toml")).unwrap();
    assert!(text.contains("# owned by the platform team"), "{text}");
    assert!(text.contains("port = 5432 # pgbouncer"), "{text}");

    // 개인 파일은 공유 파일 위에 덮인다(설정 계층 D9).
    let mut params = upsert_params(&f, "prod", local);
    params["location"] = json!("local");
    let entry = w.call("db.connection.upsert", params).await.unwrap();
    assert_eq!(entry["config"]["user"], "me");
    assert_eq!(entry["layers"], json!(["shared", "local"]));
    assert!(f.db_file("prod.local.toml").exists());
}

/// ② `db.secret.set` 뒤 설정 파일의 `host` 를, 다른 경우에는 `sslmode` 를 `disable` 로 바꾸면 그 비밀을 쓰지 않고 「비밀 필요」
/// 이다. 답에는 이전 → 지금 대상이 실린다(값은 없다). 목록은 키체인을 읽지 않고 같은 답을 낸다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_is_not_sent_to_a_changed_target() {
    for (from, to) in [
        ("host = \"db.internal\"", "host = \"evil.example\""),
        ("sslmode = \"verify-full\"", "sslmode = \"disable\""),
        ("host_ref = \"bastion\"", "host_ref = \"other-bastion\""),
        ("user = \"app\"", "user = \"postgres\""),
        ("port = 5432", "port = 6543"),
    ] {
        let f = Fixture::new(true);
        f.write_db("prod.toml", REMOTE_PG);
        let mut w = f.window();
        w.approve_tunnel(&f.id, "prod").await.unwrap();
        w.set_password(&f.id, "prod", "s3cret").await.unwrap();
        let before = w
            .call(
                "db.connection.get",
                json!({ "projectId": f.id, "connectionId": "prod" }),
            )
            .await
            .unwrap();
        let gate = f.connections.trust_gate(&f.id, None).unwrap();
        let resolved = f.connections.resolve(&gate, "prod").unwrap();
        assert_eq!(resolved.spec.password.as_ref().unwrap().expose(), "s3cret");

        f.edit_db("prod.toml", from, to);
        // 터널 목적지 확인도 (값, 대상)에 묶여 먼저 다시 묻는다(M3-11) — 사람이 확인한 뒤에 비밀을 본다.
        let e = f.connections.resolve(&gate, "prod").unwrap_err();
        assert_eq!(e.code, code::APPROVAL_REQUIRED, "{from} → {to}: {e:?}");
        assert_eq!(data(&e)["request"]["kind"], "tunnel");
        w.approve_tunnel(&f.id, "prod").await.unwrap();
        let e = f.connections.resolve(&gate, "prod").unwrap_err();
        assert_eq!(e.code, code::SECRET_REQUIRED, "{from} → {to}: {e:?}");
        assert_eq!(reason(&e), Some("targetChanged"), "{e:?}");
        assert_eq!(data(&e)["previous"], target_of(&before["config"]), "{to}");
        assert_ne!(data(&e)["target"], data(&e)["previous"], "{to}");
        assert!(!format!("{e:?}").contains("s3cret"));
        let reads = f.store.reads().len();
        let list = w
            .call("db.connection.list", json!({ "projectId": f.id }))
            .await
            .unwrap();
        let state = &list["connections"][0]["state"];
        assert_eq!(state["kind"], "secretRequired", "{from} → {to}");
        assert_eq!(state["reason"], "targetChanged", "{from} → {to}");
        assert_eq!(state["previous"], data(&e)["previous"]);
        assert_eq!(state["target"], data(&e)["target"]);
        assert_eq!(f.store.reads().len(), reads, "the list read the keychain");

        // 다시 물어 넣으면 새 대상으로 쓴다.
        w.set_password(&f.id, "prod", "s3cret-2").await.unwrap();
        let resolved = f.connections.resolve(&gate, "prod").unwrap();
        assert_eq!(
            resolved.spec.password.as_ref().unwrap().expose(),
            "s3cret-2"
        );
    }
}

/// 창이 보여 준 대상과 저장 순간의 설정이 다르면(그 사이 `git pull`) `db.secret.set` 은 conflict 이고 키체인에 쓰지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_is_not_bound_to_a_target_nobody_saw() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    let mut w = f.window();
    let entry = w
        .call(
            "db.connection.get",
            json!({ "projectId": f.id, "connectionId": "prod" }),
        )
        .await
        .unwrap();
    let shown = target_of(&entry["config"]);
    f.edit_db(
        "prod.toml",
        "host = \"db.internal\"",
        "host = \"evil.example\"",
    );
    let e = w
        .call(
            "db.secret.set",
            json!({ "projectId": f.id, "connectionId": "prod", "key": "password",
                    "target": shown, "value": "s3cret" }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT, "{e:?}");
    assert_eq!(data(&e)["target"]["host"], "evil.example");
    assert_eq!(f.store.accounts(), Vec::<String>::new());
}

/// 키체인에 항목이 없으면 「비밀 필요」(`missing`), 지운 뒤에도 같다. 읽을 수 없는 항목도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_secret_is_asked_for() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    f.window().approve_tunnel(&f.id, "prod").await.unwrap();
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    let e = f.connections.resolve(&gate, "prod").unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (code::SECRET_REQUIRED, Some("missing"))
    );
    assert_eq!(data(&e)["previous"], Value::Null);

    let mut w = f.window();
    w.set_password(&f.id, "prod", "x").await.unwrap();
    assert_eq!(
        f.store.accounts(),
        vec![account(&f.id, "prod", DbSecretKey::Password)]
    );
    assert!(f.connections.resolve(&gate, "prod").is_ok());
    w.call(
        "db.secret.clear",
        json!({ "projectId": f.id, "connectionId": "prod", "key": "password" }),
    )
    .await
    .unwrap();
    let e = f.connections.resolve(&gate, "prod").unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (code::SECRET_REQUIRED, Some("missing"))
    );
    let list = w
        .call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    assert_eq!(list["connections"][0]["state"]["reason"], "missing");

    // 읽을 수 없는 항목(옛 형식)은 없는 것과 같다 — 다시 묻는다.
    f.store.put_raw(
        &account(&f.id, "prod", DbSecretKey::Password),
        "plain-password",
    );
    let e = f.connections.resolve(&gate, "prod").unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (code::SECRET_REQUIRED, Some("missing"))
    );
}

/// 미신뢰 프로젝트의 비밀 쓰기는 `untrusted` 다 — 미신뢰 설정의 대상으로 키체인 항목을 만들지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_cannot_store_secrets() {
    let f = Fixture::new(false);
    f.write_db("prod.toml", REMOTE_PG);
    let mut w = f.window();
    let e = w.set_password(&f.id, "prod", "x").await.unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (rpc_code::PERMISSION_DENIED, Some("untrusted"))
    );
    assert_eq!(f.store.accounts(), Vec::<String>::new());
}

const ENV_PG: &str = r#"schema = 1
name = "ci pg"
driver = "postgres"
host = "db.internal"
user = "app"
password = { env = "PGPASSWORD" }
"#;

/// 확인 요청을 그대로 확인으로 돌려준다(창이 보여 주고 [확인] 을 누른 흐름).
async fn approve(
    w: &mut common::Conn,
    f: &Fixture,
    id: &str,
    request: &Value,
) -> Result<Value, RpcError> {
    let mut approval = request.clone();
    approval.as_object_mut().unwrap().remove("previous");
    w.call(
        "db.connection.approve",
        json!({ "projectId": f.id, "connectionId": id, "approval": approval }),
    )
    .await
}

/// ③ `env` 참조: 처음 쓸 때도 묻고(`previous: null`), 확인한 (변수, 대상)을 기억한다. 변수 이름이나 호스트가 바뀌면 다시 「확인
/// 필요」 이고 그동안 변수 값을 읽지 않는다. 확인(`db.connection.approve`) 뒤에는 새 변수로 연다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_env_reference_is_asked_before_reading_the_variable() {
    for (from, to, variable) in [
        (
            "password = { env = \"PGPASSWORD\" }",
            "password = { env = \"AWS_SECRET_ACCESS_KEY\" }",
            "AWS_SECRET_ACCESS_KEY",
        ),
        (
            "host = \"db.internal\"",
            "host = \"evil.example\"",
            "PGPASSWORD",
        ),
    ] {
        let f = Fixture::new(true);
        f.env.set("PGPASSWORD", "pg-pass");
        f.env.set("AWS_SECRET_ACCESS_KEY", "aws-key");
        f.write_db("ci.toml", ENV_PG);
        let gate = f.connections.trust_gate(&f.id, None).unwrap();
        let mut w = f.window();
        // 처음 — 목록도 풀기도 묻고, 변수를 읽지 않는다.
        let list = w
            .call("db.connection.list", json!({ "projectId": f.id }))
            .await
            .unwrap();
        let state = &list["connections"][0]["state"];
        assert_eq!(state["kind"], "approvalRequired", "{list}");
        assert_eq!(state["request"]["previous"], Value::Null);
        let e = f.connections.resolve(&gate, "ci").unwrap_err();
        assert_eq!(e.code, code::APPROVAL_REQUIRED);
        assert_eq!(data(&e)["request"]["variable"], "PGPASSWORD");
        assert_eq!(f.env.reads(), Vec::<String>::new());
        approve(&mut w, &f, "ci", &data(&e)["request"])
            .await
            .unwrap();

        let resolved = f.connections.resolve(&gate, "ci").unwrap();
        assert_eq!(resolved.spec.password.as_ref().unwrap().expose(), "pg-pass");
        assert_eq!(f.env.reads(), vec!["PGPASSWORD"]);

        f.edit_db("ci.toml", from, to);
        let e = f.connections.resolve(&gate, "ci").unwrap_err();
        assert_eq!(e.code, code::APPROVAL_REQUIRED, "{to}: {e:?}");
        let request = data(&e)["request"].clone();
        assert_eq!(request["kind"], "env");
        assert_eq!(request["variable"], variable);
        assert_eq!(request["previous"]["variable"], "PGPASSWORD");
        assert_eq!(
            f.env.reads(),
            vec!["PGPASSWORD"],
            "{to}: the variable was read before asking"
        );
        let list = w
            .call("db.connection.list", json!({ "projectId": f.id }))
            .await
            .unwrap();
        assert_eq!(list["connections"][0]["state"]["kind"], "approvalRequired");
        assert_eq!(f.env.reads(), vec!["PGPASSWORD"]);

        // 창이 보여 준 것과 다른 대상으로 확인하면 conflict 이고 기록하지 않는다.
        if variable == "PGPASSWORD" {
            let mut stale = request.clone();
            stale["target"]["host"] = json!("db.internal");
            let e = approve(&mut w, &f, "ci", &stale).await.unwrap_err();
            assert_eq!(e.code, rpc_code::CONFLICT);
        }
        approve(&mut w, &f, "ci", &request).await.unwrap();
        let resolved = f.connections.resolve(&gate, "ci").unwrap();
        let expected = if variable == "PGPASSWORD" {
            "pg-pass"
        } else {
            "aws-key"
        };
        assert_eq!(resolved.spec.password.as_ref().unwrap().expose(), expected);
    }
}

/// H1: 새로 커밋된 연결이 민감한 변수를 남의 호스트로 보내게 해도, 처음이라고 묻지 않고 보내지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_env_connection_asks_before_its_first_use() {
    let f = Fixture::new(true);
    f.env.set("AWS_SECRET_ACCESS_KEY", "aws-value");
    f.write_db(
        "evil.toml",
        "schema = 1\nname = \"x\"\ndriver = \"postgres\"\nhost = \"evil.example\"\nsslmode = \"disable\"\npassword = { env = \"AWS_SECRET_ACCESS_KEY\" }\n",
    );
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    let e = f.connections.resolve(&gate, "evil").unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED);
    let request = &data(&e)["request"];
    assert_eq!(request["variable"], "AWS_SECRET_ACCESS_KEY");
    assert_eq!(request["target"]["host"], "evil.example");
    assert_eq!(request["target"]["sslmode"], "disable");
    assert_eq!(request["previous"], Value::Null);
    assert_eq!(f.env.reads(), Vec::<String>::new());
    assert!(!format!("{e:?}").contains("aws-value"));
}

/// H1 과 같은 뿌리: 확인 기록(db.json)이 깨지면 「확인 없음」 으로 읽어 다시 묻고, 덮어쓰지 않으며, 목록에 문제로 알린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_approvals_file_asks_again_and_is_not_overwritten() {
    let f = Fixture::new(true);
    f.env.set("PGPASSWORD", "pg-pass");
    f.write_db("ci.toml", ENV_PG);
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    let mut w = f.window();
    let e = f.connections.resolve(&gate, "ci").unwrap_err();
    approve(&mut w, &f, "ci", &data(&e)["request"])
        .await
        .unwrap();
    assert!(f.connections.resolve(&gate, "ci").is_ok());

    let state = f.app.data.join("projects").join(&f.id).join("db.json");
    fs::write(&state, "{ broken").unwrap();
    f.edit_db(
        "ci.toml",
        "host = \"db.internal\"",
        "host = \"evil2.example\"",
    );
    let reads = f.env.reads().len();
    let e = f.connections.resolve(&gate, "ci").unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED, "{e:?}");
    assert_eq!(data(&e)["request"]["previous"], Value::Null);
    assert_eq!(
        f.env.reads().len(),
        reads,
        "read the variable with a broken record"
    );
    let list = w
        .call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    assert!(
        list["stateProblem"].as_str().unwrap().contains("db.json"),
        "{list}"
    );
    assert_eq!(list["connections"][0]["state"]["kind"], "approvalRequired");
    let e = approve(&mut w, &f, "ci", &data(&e)["request"])
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::STORAGE_UNAVAILABLE, "{e:?}");
    assert_eq!(fs::read_to_string(&state).unwrap(), "{ broken");

    // 크기 상한을 넘어 잘린 기록도 같다.
    fs::write(
        &state,
        format!("{{\"env\":{{}},\"pad\":\"{}\"}}", "x".repeat(1024 * 1024)),
    )
    .unwrap();
    let e = f.connections.resolve(&gate, "ci").unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED);
}

/// H1 과 같은 뿌리: 연결을 지웠다가 같은 id 로 다시 만들면(동료 커밋) 처음과 같다 — 다시 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recreated_connection_asks_again() {
    let f = Fixture::new(true);
    f.env.set("PGPASSWORD", "pg-pass");
    f.write_db("ci.toml", ENV_PG);
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    let mut w = f.window();
    let e = f.connections.resolve(&gate, "ci").unwrap_err();
    approve(&mut w, &f, "ci", &data(&e)["request"])
        .await
        .unwrap();
    assert!(f.connections.resolve(&gate, "ci").is_ok());
    w.call(
        "db.connection.delete",
        json!({ "projectId": f.id, "connectionId": "ci" }),
    )
    .await
    .unwrap();
    f.write_db("ci.toml", &ENV_PG.replace("db.internal", "evil.example"));
    let e = f.connections.resolve(&gate, "ci").unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED);
    assert_eq!(data(&e)["request"]["previous"], Value::Null);
}

/// H1 과 같은 뿌리: 워크트리 프로젝트(메인 저장소 프로젝트의 신뢰를 따른다)에서 처음 쓸 때도 묻는다 — 메인에서 확인했어도.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worktree_project_asks_on_its_first_use() {
    let f = Fixture::new(true);
    f.env.set("PGPASSWORD", "pg-pass");
    fs::create_dir_all(f.root.join(".git")).unwrap();
    let wt = fake_worktree(&f.root, &f.base.join("p1-wt"));
    let wt_project = f
        .projects
        .create(NewProject {
            root: wt.clone(),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    f.write_db("ci.toml", ENV_PG);
    fs::create_dir_all(wt.join(".pitwall/db")).unwrap();
    fs::write(
        wt.join(".pitwall/db/ci.toml"),
        ENV_PG.replace("db.internal", "evil.example"),
    )
    .unwrap();
    let main_gate = f.connections.trust_gate(&f.id, None).unwrap();
    let mut w = f.window();
    let e = f.connections.resolve(&main_gate, "ci").unwrap_err();
    approve(&mut w, &f, "ci", &data(&e)["request"])
        .await
        .unwrap();
    assert!(f.connections.resolve(&main_gate, "ci").is_ok());

    let gate = f.connections.trust_gate(&wt_project, None).unwrap();
    let reads = f.env.reads().len();
    let e = f.connections.resolve(&gate, "ci").unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED, "{e:?}");
    assert_eq!(data(&e)["request"]["target"]["host"], "evil.example");
    assert_eq!(data(&e)["request"]["previous"], Value::Null);
    assert_eq!(f.env.reads().len(), reads);
}

/// ④ 미신뢰 프로젝트: `db_trust_gate` 가 `untrusted` 로 거절하고, `db.connection.list` 는 되며 키체인을 읽지 않는다. 확인 기록도
/// 미신뢰면 거절한다. 신뢰하면 지난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_lists_but_never_opens() {
    let f = Fixture::new(false);
    f.write_db("prod.toml", REMOTE_PG);
    f.write_db("ci.toml", ENV_PG);
    let e = pw_db::trust::db_trust_gate(&f.projects, &f.id, None).unwrap_err();
    assert_eq!(e.code, rpc_code::PERMISSION_DENIED);
    assert_eq!(reason(&e), Some("untrusted"));

    let mut w = f.window();
    let list = w
        .call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    assert_eq!(list["trusted"], false);
    let states: Vec<&Value> = list["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["state"])
        .collect();
    assert_eq!(states, [&json!({ "kind": "untrusted" }); 2]);
    assert_eq!(
        f.store.reads(),
        Vec::<String>::new(),
        "listing read the keychain"
    );
    assert_eq!(f.env.reads(), Vec::<String>::new());
    let e = w
        .call(
            "db.connection.approve",
            json!({ "projectId": f.id, "connectionId": "x", "approval": { "kind": "file", "realPath": "/x" } }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (rpc_code::PERMISSION_DENIED, Some("untrusted"))
    );

    f.projects.set_trust(&f.id, true).unwrap();
    assert!(f.connections.trust_gate(&f.id, None).is_ok());
    // 모르는 프로젝트는 not found.
    let e = f
        .connections
        .trust_gate("0123456789abcdef", None)
        .unwrap_err();
    assert_eq!(e.code, rpc_code::NOT_FOUND);
}

/// M2: 게이트를 지난 뒤 신뢰를 거두면, 들고 있던 `TrustedProject` 로도 풀리지 않는다(저장한 연결 · 초안 둘 다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_trust_stops_a_held_gate_pass() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    let mut w = f.window();
    w.approve_tunnel(&f.id, "prod").await.unwrap();
    w.set_password(&f.id, "prod", "s3cret").await.unwrap();
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    assert!(f.connections.resolve(&gate, "prod").is_ok());
    f.projects.set_trust(&f.id, false).unwrap();
    let reads = f.store.reads().len();
    let e = f.connections.resolve(&gate, "prod").unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (rpc_code::PERMISSION_DENIED, Some("untrusted"))
    );
    let draft: DbConnectionConfig = serde_json::from_value(remote_config()).unwrap();
    let e = f
        .connections
        .resolve_draft(&gate, Some("prod"), &draft, None)
        .unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (rpc_code::PERMISSION_DENIED, Some("untrusted"))
    );
    assert_eq!(
        f.store.reads().len(),
        reads,
        "read the keychain after trust was revoked"
    );
}

/// ⑤ 루트 밖 SQLite 경로(루트 안 심링크로 가리킨 것 포함)는 확인 전 거절, 확인하면 연다 — 그 실제 경로가 허용 루트에 든다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sqlite_file_outside_the_root_needs_approval() {
    let f = Fixture::new(true);
    let outside = f.base.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("app.db"), "").unwrap();
    fs::write(f.root.join("inside.db"), "").unwrap();
    std::os::unix::fs::symlink(outside.join("app.db"), f.root.join("link.db")).unwrap();
    let real = outside.join("app.db").display().to_string();
    f.write_db(
        "abs.toml",
        &format!("schema = 1\nname = \"abs\"\ndriver = \"sqlite\"\npath = \"{real}\"\n"),
    );
    f.write_db(
        "link.toml",
        "schema = 1\nname = \"link\"\ndriver = \"sqlite\"\npath = \"link.db\"\n",
    );
    f.write_db(
        "inside.toml",
        "schema = 1\nname = \"inside\"\ndriver = \"sqlite\"\npath = \"inside.db\"\n",
    );
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    let inside = f.connections.resolve(&gate, "inside").unwrap();
    assert_eq!(inside.spec.allowed_roots, vec![f.root.clone()]);
    for id in ["abs", "link"] {
        let e = f.connections.resolve(&gate, id).unwrap_err();
        assert_eq!(e.code, code::APPROVAL_REQUIRED, "{id}");
        assert_eq!(
            data(&e)["request"],
            json!({ "kind": "file", "realPath": real }),
            "{id}"
        );
    }

    let mut w = f.window();
    let e = w
        .call(
            "db.connection.approve",
            json!({ "projectId": f.id, "connectionId": "link",
                    "approval": { "kind": "file", "realPath": f.root.join("inside.db").display().to_string() } }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        e.code,
        rpc_code::CONFLICT,
        "approving a path the config does not name"
    );
    w.call(
        "db.connection.approve",
        json!({ "projectId": f.id, "connectionId": "link", "approval": { "kind": "file", "realPath": real } }),
    )
    .await
    .unwrap();
    // 기억은 (프로젝트, 실제 경로)다 — 같은 파일을 가리키는 다른 연결도 연다.
    for id in ["abs", "link"] {
        let resolved = f.connections.resolve(&gate, id).unwrap();
        assert!(
            resolved
                .spec
                .allowed_roots
                .contains(&outside.join("app.db")),
            "{id}"
        );
    }
    let state = f.app.data.join("projects").join(&f.id).join("db.json");
    assert!(fs::read_to_string(state).unwrap().contains(&real));
}

/// M3: 드라이버에는 판정한 실제 경로가 간다 — 판정 뒤 루트 안 링크를 루트 밖 파일로 바꿔 끼워도, 받은 spec 은 확인받지 않은 파일을
/// 열지 않는다. 다시 풀면 그때는 묻는다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_driver_opens_the_path_that_was_judged() {
    let f = Fixture::new(true);
    let outside = f.base.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("app.db"), "").unwrap();
    fs::write(f.root.join("real-inside.db"), "").unwrap();
    let link = f.root.join("link.db");
    std::os::unix::fs::symlink(f.root.join("real-inside.db"), &link).unwrap();
    f.write_db(
        "link.toml",
        "schema = 1\nname = \"link\"\ndriver = \"sqlite\"\npath = \"link.db\"\n",
    );
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    let resolved = f.connections.resolve(&gate, "link").unwrap();
    assert_eq!(file_path(&resolved.spec), f.root.join("real-inside.db"));

    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(outside.join("app.db"), &link).unwrap();
    assert_eq!(
        fs::canonicalize(file_path(&resolved.spec)).unwrap(),
        f.root.join("real-inside.db"),
        "the judged spec now opens another file"
    );
    let e = f.connections.resolve(&gate, "link").unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED);
}

/// M4: 초안(연결 테스트)도 저장한 연결과 같은 판정을 지난다 — 루트 밖 SQLite · `env` 참조는 묻고, 입력한 비밀은 그 요청에만 쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_draft_passes_the_same_checks() {
    let f = Fixture::new(true);
    let outside = f.base.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("app.db"), "").unwrap();
    f.env.set("PGPASSWORD", "pg-pass");
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    let draft = |v: Value| -> DbConnectionConfig { serde_json::from_value(v).unwrap() };

    let sqlite = draft(json!({ "name": "x", "driver": "sqlite", "path": outside.join("app.db") }));
    let e = f
        .connections
        .resolve_draft(&gate, None, &sqlite, None)
        .unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED);
    assert_eq!(data(&e)["request"]["kind"], "file");

    let env = draft(
        json!({ "name": "x", "driver": "postgres", "host": "db.internal",
                            "password": { "kind": "env", "variable": "PGPASSWORD" } }),
    );
    let e = f
        .connections
        .resolve_draft(&gate, None, &env, None)
        .unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED);
    assert_eq!(f.env.reads(), Vec::<String>::new());

    let typed = draft(remote_config());
    let e = f
        .connections
        .resolve_draft(&gate, None, &typed, None)
        .unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (code::SECRET_REQUIRED, Some("missing"))
    );
    let resolved = f
        .connections
        .resolve_draft(&gate, None, &typed, Some(Secret::new("typed")))
        .unwrap();
    assert_eq!(resolved.spec.password.as_ref().unwrap().expose(), "typed");
    assert_eq!(
        f.store.accounts(),
        Vec::<String>::new(),
        "a draft secret was stored"
    );

    let mut bad = remote_config();
    bad["sshTunnel"]["hostRef"] = json!("-oProxyCommand=x");
    let e = f
        .connections
        .resolve_draft(&gate, None, &draft(bad), Some(Secret::new("typed")))
        .unwrap_err();
    assert_eq!(e.code, rpc_code::INVALID_PARAMS);
}

/// M5: 활성 워크트리를 주면 공유 설정은 그 워크트리의 `.pitwall/db/` 를 읽고 쓰고, 개인 파일은 메인 체크아웃에 하나다. 상대
/// SQLite 경로는 워크트리 안에서 푼다. 확인 기억은 프로젝트 하나에 하나지만 대상이 다르면 다시 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_active_worktree_owns_the_shared_config() {
    let f = Fixture::new(true);
    f.env.set("PGPASSWORD", "pg-pass");
    fs::create_dir_all(f.root.join(".git")).unwrap();
    let wt = fake_worktree(&f.root, &f.base.join("p1-wt"));
    let top = wt.display().to_string();
    f.write_db("prod.toml", REMOTE_PG);
    f.write_db("ci.toml", ENV_PG);
    let wt_db = wt.join(".pitwall/db");
    fs::create_dir_all(&wt_db).unwrap();
    fs::write(
        wt_db.join("prod.toml"),
        REMOTE_PG.replace("\"billing\"", "\"billing_wt\""),
    )
    .unwrap();
    fs::write(wt_db.join("ci.toml"), ENV_PG).unwrap();
    fs::write(wt.join("app.db"), "").unwrap();
    fs::write(
        wt_db.join("lite.toml"),
        "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n",
    )
    .unwrap();
    let mut w = f.window();

    let list = w
        .call(
            "db.connection.list",
            json!({ "projectId": f.id, "worktree": top }),
        )
        .await
        .unwrap();
    let ids: Vec<&str> = list["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["ci", "lite", "prod"]);
    assert_eq!(list["connections"][2]["config"]["database"], "billing_wt");
    let main = w
        .call(
            "db.connection.get",
            json!({ "projectId": f.id, "connectionId": "prod" }),
        )
        .await
        .unwrap();
    assert_eq!(main["config"]["database"], "billing");

    let mut params = upsert_params(&f, "new", remote_config());
    params["worktree"] = json!(top);
    w.call("db.connection.upsert", params.clone())
        .await
        .unwrap();
    assert!(wt_db.join("new.toml").exists());
    assert!(
        !f.db_file("new.toml").exists(),
        "wrote the main checkout's shared file"
    );
    params["location"] = json!("local");
    w.call("db.connection.upsert", params).await.unwrap();
    assert!(
        f.db_file("new.local.toml").exists(),
        "the personal file lives in the main checkout"
    );
    assert!(!wt_db.join("new.local.toml").exists());

    let gate = f.connections.trust_gate(&f.id, Some(&top)).unwrap();
    let lite = f.connections.resolve(&gate, "lite").unwrap();
    assert_eq!(file_path(&lite.spec), wt.join("app.db"));

    // 메인에서 확인한 env 는 같은 대상이면 워크트리에서도 쓴다. 워크트리의 대상이 다르면 다시 묻는다.
    let main_gate = f.connections.trust_gate(&f.id, None).unwrap();
    let e = f.connections.resolve(&main_gate, "ci").unwrap_err();
    approve(&mut w, &f, "ci", &data(&e)["request"])
        .await
        .unwrap();
    assert!(f.connections.resolve(&gate, "ci").is_ok());
    fs::write(
        wt_db.join("ci.toml"),
        ENV_PG.replace("db.internal", "evil.example"),
    )
    .unwrap();
    let e = f.connections.resolve(&gate, "ci").unwrap_err();
    assert_eq!(e.code, code::APPROVAL_REQUIRED);
    assert_eq!(
        data(&e)["request"]["previous"]["target"]["host"],
        "db.internal"
    );

    // 그 프로젝트의 것이 아닌 워크트리는 거절한다.
    let e = f
        .connections
        .trust_gate(&f.id, Some(&f.base.join("elsewhere").display().to_string()))
        .unwrap_err();
    assert_eq!(e.code, rpc_code::INVALID_PARAMS);
}

/// ⑥ 다른 프로젝트 창 · 웰컴(Host) · 파일 창 연결의 설정 · 비밀 쓰기는 `PERMISSION_DENIED` 이고 아무것도 바뀌지 않는다. 목록은 막지
/// 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_windows_cannot_write_connections_or_secrets() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    let before = fs::read_to_string(f.db_file("prod.toml")).unwrap();
    let file = pw_rpc::Caller::file(pw_rpc::Role::Owner, f.root.join("a.txt"));
    let scopes = [
        ("other project", ConnectionScope::Project(f.other.clone())),
        ("welcome", ConnectionScope::Host),
        ("file window", file.scope.clone()),
    ];
    let target = target_of(&remote_config());
    let writes = [
        (
            "db.connection.upsert",
            upsert_params(&f, "new", remote_config()),
        ),
        (
            "db.connection.upsert",
            upsert_params(&f, "prod", remote_config()),
        ),
        (
            "db.connection.delete",
            json!({ "projectId": f.id, "connectionId": "prod" }),
        ),
        (
            "db.secret.set",
            json!({ "projectId": f.id, "connectionId": "prod", "key": "password",
                    "target": target, "value": "x" }),
        ),
        (
            "db.secret.clear",
            json!({ "projectId": f.id, "connectionId": "prod", "key": "password" }),
        ),
        (
            "db.connection.approve",
            json!({ "projectId": f.id, "connectionId": "prod",
                    "approval": { "kind": "file", "realPath": "/etc/passwd" } }),
        ),
    ];
    f.store
        .put_raw(&account(&f.id, "prod", DbSecretKey::Password), "kept");
    for (who, scope) in scopes {
        let mut conn = f.connect(scope);
        for (method, params) in &writes {
            let e = conn.call(method, params.clone()).await.unwrap_err();
            assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{who} {method}: {e:?}");
            assert_eq!(reason(&e), None, "{who} {method}: scope, not trust");
        }
        // 읽기는 막지 않는다 — 파일 창은 디스패처가 막는다(파일 범위 연결에는 열지 않는다, 결정 M3-D18).
        let list = conn
            .call("db.connection.list", json!({ "projectId": f.id }))
            .await;
        if who == "file window" {
            assert_eq!(list.unwrap_err().code, rpc_code::PERMISSION_DENIED);
        } else {
            assert_eq!(
                list.unwrap()["connections"].as_array().unwrap().len(),
                1,
                "{who}"
            );
        }
    }
    assert_eq!(fs::read_to_string(f.db_file("prod.toml")).unwrap(), before);
    assert!(!f.db_file("new.toml").exists());
    assert_eq!(
        f.store
            .raw(&account(&f.id, "prod", DbSecretKey::Password))
            .as_deref(),
        Some("kept")
    );
    assert!(
        !f.app
            .data
            .join("projects")
            .join(&f.id)
            .join("db.json")
            .exists()
    );
}

/// ⑧ `host_ref` 가 `-` 로 시작하거나 공백 · 제어 문자를 담으면 저장을 거절하고 파일을 만들지 않는다. 손으로 쓴 파일은 「설정 오류」
/// 이고 열지 않는다. ssh 옵션을 적을 자리도 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dangerous_host_ref_is_refused() {
    let f = Fixture::new(true);
    let mut w = f.window();
    for host_ref in [
        "-oProxyCommand=touch /tmp/x",
        "-J",
        "bastion host",
        "bastion\u{7}",
        "bastion\n",
        "bastion\t",
        "db:2222",
        "ssh://-oProxyCommand=x",
        "ssh://user@",
        "ssh://host:notaport",
    ] {
        let mut config = remote_config();
        config["sshTunnel"]["hostRef"] = json!(host_ref);
        let e = w
            .call("db.connection.upsert", upsert_params(&f, "t", config))
            .await
            .unwrap_err();
        assert_eq!(e.code, rpc_code::INVALID_PARAMS, "{host_ref:?}");
        assert!(!f.db_file("t.toml").exists(), "{host_ref:?}");
    }
    for host_ref in [
        "bastion",
        "ssh://jump.example",
        "ssh://me@jump.example:2222",
        "ssh://[::1]:22",
    ] {
        let mut config = remote_config();
        config["sshTunnel"]["hostRef"] = json!(host_ref);
        w.call("db.connection.upsert", upsert_params(&f, "t", config))
            .await
            .unwrap_or_else(|e| panic!("{host_ref:?}: {e:?}"));
    }
    // DB 호스트도 ssh argv(`-L`)로 간다.
    let mut config = remote_config();
    config["host"] = json!("-oProxyCommand=x");
    let e = w
        .call("db.connection.upsert", upsert_params(&f, "h", config))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::INVALID_PARAMS);

    f.write_db(
        "hand.toml",
        "schema = 1\nname = \"x\"\ndriver = \"postgres\"\nhost = \"db\"\n[ssh_tunnel]\nhost_ref = \"-oProxyCommand=touch /tmp/x\"\n",
    );
    f.write_db(
        "opts.toml",
        "schema = 1\nname = \"x\"\ndriver = \"postgres\"\nhost = \"db\"\n[ssh_tunnel]\nhost_ref = \"bastion\"\noptions = \"-oProxyCommand=touch /tmp/x\"\n",
    );
    let list = w
        .call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    for c in list["connections"].as_array().unwrap() {
        if c["id"] == "hand" || c["id"] == "opts" {
            assert_eq!(c["state"]["kind"], "invalid", "{c}");
            assert_eq!(c["config"], Value::Null);
        }
    }
    let gate = f.connections.trust_gate(&f.id, None).unwrap();
    for id in ["hand", "opts"] {
        let e = f.connections.resolve(&gate, id).unwrap_err();
        assert_eq!(e.code, rpc_code::INVALID_PARAMS, "{id}");
    }
}

/// L1: 대소문자만 다른 연결 id 는 같은 파일일 수 있다 — `Prod` 가 있으면 `prod` 를 새로 쓰지 않고, `prod` 로 `Prod` 를 지우지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ids_that_differ_only_in_case_are_refused() {
    let f = Fixture::new(true);
    f.write_db("Prod.toml", REMOTE_PG);
    let before = fs::read_to_string(f.db_file("Prod.toml")).unwrap();
    let mut w = f.window();
    let mut config = remote_config();
    config["host"] = json!("other.example");
    let e = w
        .call("db.connection.upsert", upsert_params(&f, "prod", config))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT, "{e:?}");
    let e = w
        .call(
            "db.connection.delete",
            json!({ "projectId": f.id, "connectionId": "prod" }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::NOT_FOUND);
    assert_eq!(fs::read_to_string(f.db_file("Prod.toml")).unwrap(), before);
}

/// 연결 설정 파일이 바뀌면(`git pull` 흉내 — 파일 감시의 `fs.changed`) 바뀐 연결만 `config.changed` 로 낸다. 관계없는 파일은 내지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_connection_file_is_announced() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    f.write_db("ci.toml", ENV_PG);
    let mut events = f
        .bus
        .subscribe(Filter::all().topics([pw_db::config::TOPIC_CHANGED]));
    tokio::spawn(pw_db::config::follow_files(
        pw_db::config::subscribe_files(&f.bus),
        f.connections.clone(),
    ));
    let mut w = f.window();
    w.call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    let fs_changed = |path: &std::path::Path| {
        f.bus
            .publish(
                Scope::Project(f.id.clone()),
                pw_db::config::FS_CHANGED,
                &json!({ "root": f.root, "worktree": null, "paths": [path], "kinds": ["modified"], "rescan": false }),
            )
            .unwrap();
    };
    // 관계없는 파일.
    fs::write(f.root.join("README.md"), "x").unwrap();
    fs_changed(&f.root.join("README.md"));
    f.edit_db(
        "prod.toml",
        "database = \"billing\"",
        "database = \"billing2\"",
    );
    fs_changed(&f.db_file("prod.toml"));
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("no config.changed")
        .unwrap();
    let Received::Event(event) = event else {
        panic!("lagged");
    };
    assert_eq!(event.scope, Scope::Project(f.id.clone()));
    assert_eq!(event.payload["changed"], json!(["db/prod"]));
    // 같은 내용으로 다시 알려도(파일 시각만 바뀜) 내지 않는다. 지우면 낸다.
    fs_changed(&f.db_file("prod.toml"));
    fs::remove_file(f.db_file("ci.toml")).unwrap();
    fs_changed(&f.db_file("ci.toml"));
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("no config.changed")
        .unwrap();
    let Received::Event(event) = event else {
        panic!("lagged");
    };
    assert_eq!(event.payload["changed"], json!(["db/ci"]));
}

/// 파일 감시보다 목록 요청이 먼저 바뀐 값을 읽어도 `config.changed` 가 나간다 — 열린 연결(M3-05)이 그 변경을 놓치지 않는다.
/// 감시를 따라가는 작업은 띄우지 않는다(이벤트는 목록 요청만 낼 수 있다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_seen_first_by_the_list_is_announced() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    let mut events = f
        .bus
        .subscribe(Filter::all().topics([pw_db::config::TOPIC_CHANGED]));
    let mut w = f.window();
    w.call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    f.edit_db(
        "prod.toml",
        "database = \"billing\"",
        "database = \"billing2\"",
    );
    w.call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("no config.changed")
        .unwrap();
    let Received::Event(event) = event else {
        panic!("lagged");
    };
    assert_eq!(event.payload["changed"], json!(["db/prod"]));
    w.call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), events.recv())
            .await
            .is_err(),
        "an unchanged list announced again"
    );
}

/// 지우면 공유 · 개인 파일과 그 연결의 비밀 · `env` 확인 · 비밀 대상 사본이 사라진다. 없는 연결은 not found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_files_secrets_and_approvals() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    f.write_db("prod.local.toml", "schema = 1\nuser = \"me\"\n");
    let mut w = f.window();
    w.set_password(&f.id, "prod", "x").await.unwrap();
    w.call(
        "db.connection.delete",
        json!({ "projectId": f.id, "connectionId": "prod" }),
    )
    .await
    .unwrap();
    assert!(!f.db_file("prod.toml").exists());
    assert!(!f.db_file("prod.local.toml").exists());
    assert_eq!(f.store.accounts(), Vec::<String>::new());
    let state = f.app.data.join("projects").join(&f.id).join("db.json");
    assert!(!fs::read_to_string(state).unwrap().contains("\"prod\""));
    let e = w
        .call(
            "db.connection.get",
            json!({ "projectId": f.id, "connectionId": "prod" }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::NOT_FOUND);
    let e = w
        .call(
            "db.connection.delete",
            json!({ "projectId": f.id, "connectionId": "prod" }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::NOT_FOUND);
    // 파일 이름이 될 수 없는 id 는 거절한다.
    for id in ["../x", "a.local", "", "a/b"] {
        let e = w
            .call(
                "db.connection.get",
                json!({ "projectId": f.id, "connectionId": id }),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, rpc_code::INVALID_PARAMS, "{id:?}");
    }
}

/// 깨진 설정 파일은 목록의 `problem` 이다(설정 계층은 파일 하나라도 깨지면 전체가 실패).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_file_is_a_list_problem() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", "name = ");
    let mut w = f.window();
    let list = w
        .call("db.connection.list", json!({ "projectId": f.id }))
        .await
        .unwrap();
    assert_eq!(list["connections"], json!([]));
    assert!(
        list["problem"].as_str().unwrap().contains("prod.toml"),
        "{list}"
    );
}
