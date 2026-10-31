//! M3-02 의 완료 기준(14-m3-plan.md M3-02 행): 연결 설정 파일 왕복 · 비밀의 대상 묶기 · `env` 참조 확인 · 신뢰 게이트 · 루트 밖
//! SQLite · 쓰기의 연결 범위 · `host_ref` 거절, 그리고 `config.changed`. 키체인은 가짜 저장소, 런타임 환경은 가짜다.
//! 비밀 값이 로그 파일 · 응답에 없는 것(⑦)은 `secret_log.rs` 가 본다(전역 로그 구독자를 따로 설치한다).

mod common;

use std::fs;
use std::time::Duration;

use common::{Fixture, REMOTE_PG};
use pw_core::bus::{Filter, Received, Scope};
use pw_db::secret::DbSecretKey;
use pw_db::secret::account;
use pw_db::trust::code;
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
    assert_eq!(entry["state"]["kind"], "secretRequired", "{entry}");

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

    w.call(
        "db.secret.set",
        json!({ "projectId": f.id, "connectionId": "prod", "key": "password", "value": "s3cret-hunter2" }),
    )
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

    // 손으로 단 주석과 모르는 순서는 다시 써도 남는다.
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

/// ② `db.secret.set` 뒤 설정 파일의 `host` 를 바꾸면, 다른 경우에는 `sslmode` 를 `disable` 로 바꾸면 그 비밀을 쓰지 않고
/// 「비밀 필요」 다.
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
        w.call(
            "db.secret.set",
            json!({ "projectId": f.id, "connectionId": "prod", "key": "password", "value": "s3cret" }),
        )
        .await
        .unwrap();
        let gate = f.connections.trust_gate(&f.id).unwrap();
        let resolved = f.connections.resolve(&gate, "prod").unwrap();
        assert_eq!(resolved.spec.password.as_ref().unwrap().expose(), "s3cret");

        f.edit_db("prod.toml", from, to);
        let e = f.connections.resolve(&gate, "prod").unwrap_err();
        assert_eq!(e.code, code::SECRET_REQUIRED, "{from} → {to}: {e:?}");
        assert_eq!(reason(&e), Some("targetChanged"), "{e:?}");
        assert!(!format!("{e:?}").contains("s3cret"));
        let list = w
            .call("db.connection.list", json!({ "projectId": f.id }))
            .await
            .unwrap();
        assert_eq!(
            list["connections"][0]["state"],
            json!({ "kind": "secretRequired", "reason": "targetChanged" }),
            "{from} → {to}"
        );

        // 다시 물어 넣으면 새 대상으로 쓴다.
        w.call(
            "db.secret.set",
            json!({ "projectId": f.id, "connectionId": "prod", "key": "password", "value": "s3cret-2" }),
        )
        .await
        .unwrap();
        let resolved = f.connections.resolve(&gate, "prod").unwrap();
        assert_eq!(
            resolved.spec.password.as_ref().unwrap().expose(),
            "s3cret-2"
        );
    }
}

/// 키체인에 항목이 없으면 「비밀 필요」(`missing`), 지운 뒤에도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_secret_is_asked_for() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    let gate = f.connections.trust_gate(&f.id).unwrap();
    let e = f.connections.resolve(&gate, "prod").unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        (code::SECRET_REQUIRED, Some("missing"))
    );

    let mut w = f.window();
    let set = json!({ "projectId": f.id, "connectionId": "prod", "key": "password", "value": "x" });
    w.call("db.secret.set", set).await.unwrap();
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

const ENV_PG: &str = r#"schema = 1
name = "ci pg"
driver = "postgres"
host = "db.internal"
user = "app"
password = { env = "PGPASSWORD" }
"#;

/// ③ `env` 참조: 처음 쓸 때 (변수, 대상)을 기억하고, 변수 이름이나 호스트가 바뀌면 「확인 필요」 이고 변수 값을 읽지 않는다.
/// 확인(`db.connection.approve`) 뒤에는 새 변수로 연다.
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
        let gate = f.connections.trust_gate(&f.id).unwrap();
        let mut w = f.window();
        // 목록은 변수를 읽지 않는다.
        let list = w
            .call("db.connection.list", json!({ "projectId": f.id }))
            .await
            .unwrap();
        assert_eq!(list["connections"][0]["state"], json!({ "kind": "ready" }));
        assert_eq!(f.env.reads(), Vec::<String>::new());

        let resolved = f.connections.resolve(&gate, "ci").unwrap();
        assert_eq!(resolved.spec.password.as_ref().unwrap().expose(), "pg-pass");
        assert_eq!(f.env.reads(), vec!["PGPASSWORD"]);

        f.edit_db("ci.toml", from, to);
        let e = f.connections.resolve(&gate, "ci").unwrap_err();
        assert_eq!(e.code, code::APPROVAL_REQUIRED, "{to}: {e:?}");
        let request = e.data.as_ref().unwrap()["request"].clone();
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
        let mut stale = request.clone();
        stale["target"]["host"] = json!("db.internal");
        stale.as_object_mut().unwrap().remove("previous");
        if variable == "PGPASSWORD" {
            let e = w
                .call(
                    "db.connection.approve",
                    json!({ "projectId": f.id, "connectionId": "ci", "approval": stale }),
                )
                .await
                .unwrap_err();
            assert_eq!(e.code, rpc_code::CONFLICT);
        }
        let mut approval = request;
        approval.as_object_mut().unwrap().remove("previous");
        w.call(
            "db.connection.approve",
            json!({ "projectId": f.id, "connectionId": "ci", "approval": approval }),
        )
        .await
        .unwrap();
        let resolved = f.connections.resolve(&gate, "ci").unwrap();
        let expected = if variable == "PGPASSWORD" {
            "pg-pass"
        } else {
            "aws-key"
        };
        assert_eq!(resolved.spec.password.as_ref().unwrap().expose(), expected);
    }
}

/// ④ 미신뢰 프로젝트: `db_trust_gate` 가 `untrusted` 로 거절하고, `db.connection.list` 는 되며 키체인을 읽지 않는다. 확인 기록도
/// 미신뢰면 거절한다. 신뢰하면 지난다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_lists_but_never_opens() {
    let f = Fixture::new(false);
    f.write_db("prod.toml", REMOTE_PG);
    f.write_db("ci.toml", ENV_PG);
    let e = pw_db::trust::db_trust_gate(&f.projects, &f.id).unwrap_err();
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
    assert!(f.connections.trust_gate(&f.id).is_ok());
    // 모르는 프로젝트는 not found.
    let e = f.connections.trust_gate("0123456789abcdef").unwrap_err();
    assert_eq!(e.code, rpc_code::NOT_FOUND);
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
    let gate = f.connections.trust_gate(&f.id).unwrap();
    let inside = f.connections.resolve(&gate, "inside").unwrap();
    assert_eq!(inside.spec.allowed_roots, vec![f.root.clone()]);
    for id in ["abs", "link"] {
        let e = f.connections.resolve(&gate, id).unwrap_err();
        assert_eq!(e.code, code::APPROVAL_REQUIRED, "{id}");
        assert_eq!(
            e.data.as_ref().unwrap()["request"],
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
            json!({ "projectId": f.id, "connectionId": "prod", "key": "password", "value": "x" }),
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
    let gate = f.connections.trust_gate(&f.id).unwrap();
    for id in ["hand", "opts"] {
        let e = f.connections.resolve(&gate, id).unwrap_err();
        assert_eq!(e.code, rpc_code::INVALID_PARAMS, "{id}");
    }
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
                &json!({ "root": f.root, "paths": [path], "kinds": ["modified"], "rescan": false }),
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

/// 지우면 공유 · 개인 파일과 그 연결의 비밀 · `env` 확인이 사라진다. 없는 연결은 not found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_files_secrets_and_approvals() {
    let f = Fixture::new(true);
    f.write_db("prod.toml", REMOTE_PG);
    f.write_db("prod.local.toml", "schema = 1\nuser = \"me\"\n");
    let mut w = f.window();
    w.call(
        "db.secret.set",
        json!({ "projectId": f.id, "connectionId": "prod", "key": "password", "value": "x" }),
    )
    .await
    .unwrap();
    w.call(
        "db.connection.delete",
        json!({ "projectId": f.id, "connectionId": "prod" }),
    )
    .await
    .unwrap();
    assert!(!f.db_file("prod.toml").exists());
    assert!(!f.db_file("prod.local.toml").exists());
    assert_eq!(f.store.accounts(), Vec::<String>::new());
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
