//! 18-m7-plan M7-01 완료 기준 ④: 조립한 Host 의 DB 연결은 부품의 비밀 저장소([`pw_host::HostParts::secrets`])만 본다. 가짜 저장소로
//! 조립하고 그 프로젝트 창의 연결로 비밀을 넣으면 가짜에 들어가고, 연결 테스트가 그 비밀을 가짜에서 읽는다(사람의 키체인에 닿지 않는다).

mod common;

use std::sync::Arc;

use pw_core::registry::NewProject;
use pw_core::{HostRouter, Target};
use pw_db::secret::{DbSecretKey, MemorySecretStore, account};
use pw_rpc::{Caller, Role};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_db_connection_reads_its_password_from_the_secret_store_in_the_parts() {
    let tmp = tempfile::tempdir().unwrap();
    let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
    let store = Arc::new(MemorySecretStore::new());
    let assembled = common::assemble(
        &base.join("app"),
        store.clone(),
        tokio::runtime::Handle::current(),
    );
    let host = Arc::new(assembled.host);
    let registry = host.registry().unwrap().clone();
    let root = base.join("repo");
    std::fs::create_dir_all(root.join(".pitwall/db")).unwrap();
    // 받는 곳이 없는 포트 — 드라이버까지 가면 거절당해 바로 끝난다.
    std::fs::write(
        root.join(".pitwall/db/local.toml"),
        "name = \"local pg\"\ndriver = \"postgres\"\nhost = \"127.0.0.1\"\nport = 1\nuser = \"app\"\n\
         password = { secret_ref = \"keychain\" }\n",
    )
    .unwrap();
    let id = registry
        .create(NewProject {
            root,
            ..Default::default()
        })
        .unwrap()
        .id;
    registry.set_trust(&id, true).unwrap();
    let router = HostRouter::new();
    router.add_host(host).unwrap();
    router.bind_project(id.clone(), "local").unwrap();
    let mut window = router
        .connect(&Target::ProjectId(id.clone()), Caller::new(Role::Owner))
        .await
        .unwrap();
    let test = json!({"projectId": id, "connectionId": "local"});

    // 비밀이 없다: 가짜 저장소에서 찾지 못해 「비밀 필요」.
    let missing = common::call(&mut window, 1, "db.connection.test", test.clone())
        .await
        .unwrap_err();
    assert_eq!(
        missing["code"],
        json!(pw_db::trust::code::SECRET_REQUIRED),
        "{missing}"
    );
    let password = account(&id, "local", DbSecretKey::Password);
    assert!(store.reads().contains(&password), "{:?}", store.reads());

    // 창이 비밀을 넣으면 가짜 저장소에 들어간다.
    let set = json!({
        "projectId": id,
        "connectionId": "local",
        "key": "password",
        "target": missing["data"]["target"],
        "value": "s3cret",
    });
    common::call(&mut window, 2, "db.secret.set", set)
        .await
        .unwrap();
    assert_eq!(store.accounts(), std::slice::from_ref(&password));

    // 다시 테스트하면 비밀을 가짜에서 읽어 드라이버까지 간다(받는 곳이 없어 드라이버 오류로 끝난다).
    let reads = store.reads().len();
    let failed = common::call(&mut window, 3, "db.connection.test", test)
        .await
        .unwrap_err();
    assert_ne!(
        failed["code"],
        json!(pw_db::trust::code::SECRET_REQUIRED),
        "{failed}"
    );
    assert!(
        store.reads()[reads..].contains(&password),
        "{:?}",
        store.reads()
    );
}
