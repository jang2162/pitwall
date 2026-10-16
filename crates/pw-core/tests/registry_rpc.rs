//! `project.*` RPC 를 LocalHost 연결의 JSON 메시지로 왕복한다. 규칙 자체의 시험은 `src/registry/tests.rs`.

use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::{
    ChangeKind, ConfigMode, Project, ProjectChanged, Resolution, RootCandidate, StorageStatus,
    TOPIC_CHANGED,
};
use pw_core::{Connection, Filter, Host, LocalHost, Received, Scope};
use pw_rpc::{Caller, Role, code};
use serde_json::{Value, json};

struct Client {
    conn: Connection,
    next_id: i64,
}

impl Client {
    async fn connect(host: &LocalHost, role: Role) -> Self {
        Self {
            conn: host.connect(Caller::new(role)).await.unwrap(),
            next_id: 0,
        }
    }

    /// 요청을 보내고 그 응답 프레임을 받는다.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.conn
            .sink
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let frame = tokio::time::timeout(Duration::from_secs(5), self.conn.source.recv())
            .await
            .expect("timed out waiting for a frame")
            .expect("connection closed");
        assert_eq!(frame["id"], json!(id), "{frame}");
        frame
    }

    async fn ok<T: serde::de::DeserializeOwned>(&mut self, method: &str, params: Value) -> T {
        let frame = self.call(method, params).await;
        assert!(frame.get("error").is_none(), "{frame}");
        serde_json::from_value(frame["result"].clone()).unwrap()
    }

    async fn error_code(&mut self, method: &str, params: Value) -> i64 {
        let frame = self.call(method, params).await;
        frame["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("expected an error: {frame}"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_methods_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let paths = AppPaths::under(&base.join("app"));
    let root = base.join("work/proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let host = LocalHost::with_config("local", paths.clone());
    let mut events = host
        .bus()
        .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_CHANGED]));
    let mut c = Client::connect(&host, Role::Owner).await;

    let status: StorageStatus = c.ok("project.storage", Value::Null).await;
    assert!(matches!(status, StorageStatus::Ok { .. }), "{status:?}");

    let created: Project = c
        .ok(
            "project.create",
            json!({"root": root.join("src/.."), "tags": ["x"], "configMode": "personal"}),
        )
        .await;
    assert_eq!(created.name, "proj");
    assert_eq!(created.roots, vec![root.display().to_string()]);
    assert_eq!(created.config_mode, ConfigMode::Personal);
    let id = created.id.clone();

    let got: Project = c.ok("project.get", json!({"id": id})).await;
    assert_eq!(got, created);
    let list: Vec<Project> = c.ok("project.list", Value::Null).await;
    assert_eq!(list, vec![created.clone()]);

    let updated: Project = c
        .ok(
            "project.update",
            json!({"id": id, "patch": {"name": "Renamed", "favorite": true}}),
        )
        .await;
    assert_eq!(updated.name, "Renamed");
    assert!(updated.favorite);
    assert_eq!(updated.tags, vec!["x"], "patch 에 없는 필드는 그대로");

    let recent: Vec<Project> = c.ok("project.recent", json!({})).await;
    assert!(recent.is_empty());
    let opened: Project = c.ok("project.markOpened", json!({"id": id})).await;
    assert!(opened.opened_at.is_some());
    let recent: Vec<Project> = c.ok("project.recent", json!({"limit": 5})).await;
    assert_eq!(recent, vec![opened]);

    let hit: Option<Resolution> = c
        .ok("project.resolve", json!({"path": root.join("src/new.rs")}))
        .await;
    assert_eq!(hit.unwrap().project.id, id);
    let miss: Option<Resolution> = c
        .ok("project.resolve", json!({"path": base.join("elsewhere")}))
        .await;
    assert_eq!(miss, None);

    std::fs::write(root.join("Cargo.toml"), "").unwrap();
    let candidates: Vec<RootCandidate> = c
        .ok("project.candidates", json!({"path": root.join("src")}))
        .await;
    assert_eq!(candidates[0].path, root.display().to_string());
    assert_eq!(candidates[0].build_files, ["Cargo.toml"]);

    // 잘못된 입력은 invalid params, 같은 루트 두 번 등록도.
    for (method, params) in [
        ("project.get", json!({"id": "nope"})),
        ("project.resolve", json!({"path": "relative/path"})),
        ("project.create", json!({"root": root})),
        ("project.create", json!({"root": base.join("missing")})),
    ] {
        assert_eq!(
            c.error_code(method, params.clone()).await,
            i64::from(code::INVALID_PARAMS),
            "{method} {params}"
        );
    }

    let () = c.ok("project.delete", json!({"id": id})).await;
    let list: Vec<Project> = c.ok("project.list", Value::Null).await;
    assert!(list.is_empty());

    let mut kinds = Vec::new();
    while let Some(Received::Event(event)) = events.try_recv() {
        let change: ProjectChanged = serde_json::from_value(event.payload.clone()).unwrap();
        kinds.push(change.kind);
    }
    assert_eq!(
        kinds,
        [
            ChangeKind::Created,
            ChangeKind::Updated,
            ChangeKind::Opened,
            ChangeKind::Deleted
        ]
    );
}

/// 쓰기는 owner 만. 해석 · 후보는 임의 경로의 디스크를 보므로 read 등급이어도 owner 만.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roles_limit_writes_and_disk_probing() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(&tmp.path().join("app"));
    let host = LocalHost::with_config("local", paths);
    let denied = i64::from(code::PERMISSION_DENIED);
    for role in [Role::Readonly, Role::Mobile] {
        let mut c = Client::connect(&host, role).await;
        let list: Vec<Project> = c.ok("project.list", Value::Null).await;
        assert!(list.is_empty());
        let path = json!({"path": tmp.path()});
        for (method, params) in [
            ("project.create", json!({"root": tmp.path()})),
            ("project.update", json!({"id": "x", "patch": {}})),
            ("project.delete", json!({"id": "x"})),
            ("project.markOpened", json!({"id": "x"})),
            ("project.resolve", path.clone()),
            ("project.candidates", path),
        ] {
            assert_eq!(
                c.error_code(method, params).await,
                denied,
                "{role:?} {method}"
            );
        }
    }
}

#[tokio::test]
async fn host_without_app_dirs_has_no_registry() {
    let host = LocalHost::new("local");
    assert!(host.registry().is_none());
    let mut c = Client::connect(&host, Role::Owner).await;
    assert_eq!(
        c.error_code("project.list", Value::Null).await,
        i64::from(code::METHOD_NOT_FOUND)
    );
}

/// 쓸 수 없는 레지스트리(여기서는 이 앱보다 새 스키마)에 쓰면 일반 내부 오류가 아니라 전용 코드로 거절된다. 읽기는 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writes_to_an_unwritable_registry_get_the_storage_code() {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let paths = AppPaths::under(&base.join("app"));
    let root = base.join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(base.join("other")).unwrap();
    {
        let seed =
            pw_core::registry::Registry::new(paths.clone(), None, pw_core::EventBus::new(16));
        seed.create(pw_core::registry::NewProject {
            root: root.clone(),
            ..Default::default()
        })
        .unwrap();
    }
    rusqlite::Connection::open(paths.data.join("state.db"))
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();

    let host = LocalHost::with_config("local", paths);
    let mut c = Client::connect(&host, Role::Owner).await;
    let status: StorageStatus = c.ok("project.storage", Value::Null).await;
    assert!(
        matches!(status, StorageStatus::ReadOnly { .. }),
        "{status:?}"
    );
    let list: Vec<Project> = c.ok("project.list", Value::Null).await;
    assert_eq!(list.len(), 1);
    let frame = c
        .call("project.create", json!({"root": base.join("other")}))
        .await;
    assert_eq!(
        frame["error"]["code"],
        json!(code::STORAGE_UNAVAILABLE),
        "{frame}"
    );
    assert!(
        frame["error"]["data"]
            .as_str()
            .is_some_and(|d| d.contains("newer")),
        "{frame}"
    );
}
