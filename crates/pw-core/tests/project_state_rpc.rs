//! `project.state.get` · `project.state.set`(결정 M1-D19)을 LocalHost 연결의 JSON 메시지로 왕복한다.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::state::{ProjectState, STATE_LIMIT};
use pw_core::registry::{NewProject, Project};
use pw_core::{Connection, Host, LocalHost};
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

    async fn state(&mut self, id: &str) -> ProjectState {
        self.ok("project.state.get", json!({"projectId": id})).await
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    paths: AppPaths,
    host: LocalHost,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let paths = AppPaths::under(&base.join("app"));
        let host = LocalHost::with_config("local", paths.clone());
        Self {
            _tmp: tmp,
            base,
            paths,
            host,
        }
    }

    fn project(&self, name: &str) -> Project {
        let root = self.base.join("work").join(name);
        std::fs::create_dir_all(&root).unwrap();
        self.host
            .registry()
            .unwrap()
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap()
    }

    fn file(&self, id: &str) -> PathBuf {
        self.paths.data.join("projects").join(id).join("state.json")
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_round_trips_per_project() {
    let f = Fixture::new();
    let a = f.project("a").id;
    let b = f.project("b").id;
    let mut c = Client::connect(&f.host, Role::Owner).await;

    assert_eq!(
        c.state(&a).await,
        ProjectState {
            state: None,
            problem: None
        }
    );
    let first = json!({"version": 1, "layout": {"x": 1}, "worktrees": {"/w1": {"tabs": []}}});
    let _: () = c
        .ok("project.state.set", json!({"projectId": a, "state": first}))
        .await;
    let _: () = c
        .ok(
            "project.state.set",
            json!({"projectId": b, "state": {"version": 1, "layout": {"x": 2}}}),
        )
        .await;
    assert_eq!(c.state(&a).await.state, Some(first.clone()));
    assert_eq!(c.state(&b).await.state.unwrap()["layout"]["x"], json!(2));
    assert!(f.file(&a).is_file());
    // 임시 파일은 rename 으로 사라진다.
    assert_eq!(names(f.file(&a).parent().unwrap()), vec!["state.json"]);

    // 통째로 바꾼다.
    let second = json!({"version": 1});
    let _: () = c
        .ok(
            "project.state.set",
            json!({"projectId": a, "state": second}),
        )
        .await;
    assert_eq!(c.state(&a).await.state, Some(second));
}

/// 깨진 저장본은 없는 것처럼 `null` 과 이유를 돌려주고, 다음 쓰기가 덮는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broken_state_file_reads_as_none_with_the_reason() {
    let f = Fixture::new();
    let id = f.project("a").id;
    let mut c = Client::connect(&f.host, Role::Owner).await;
    let file = f.file(&id);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    for (contents, reason) in [
        ("{not json", "not JSON"),
        ("[1, 2]", "not a JSON object"),
        ("\"text\"", "not a JSON object"),
    ] {
        std::fs::write(&file, contents).unwrap();
        let got = c.state(&id).await;
        assert_eq!(got.state, None, "{contents}");
        assert!(
            got.problem.as_deref().is_some_and(|p| p.contains(reason)),
            "{contents}: {got:?}"
        );
    }
    std::fs::write(&file, vec![b' '; STATE_LIMIT + 1]).unwrap();
    let got = c.state(&id).await;
    assert_eq!(got.state, None);
    assert!(got.problem.unwrap().contains("limit"));

    let _: () = c
        .ok(
            "project.state.set",
            json!({"projectId": id, "state": {"ok": true}}),
        )
        .await;
    assert_eq!(c.state(&id).await.state, Some(json!({"ok": true})));
}

/// 객체가 아니거나 상한을 넘는 쓰기는 거절하고 저장본을 건드리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_writes_are_refused_and_keep_the_saved_state() {
    let f = Fixture::new();
    let id = f.project("a").id;
    let mut c = Client::connect(&f.host, Role::Owner).await;
    let _: () = c
        .ok(
            "project.state.set",
            json!({"projectId": id, "state": {"keep": 1}}),
        )
        .await;
    let invalid = i64::from(code::INVALID_PARAMS);
    let big = "x".repeat(STATE_LIMIT);
    for state in [json!([1]), json!(null), json!({"big": big})] {
        assert_eq!(
            c.error_code(
                "project.state.set",
                json!({"projectId": id, "state": state})
            )
            .await,
            invalid
        );
    }
    assert_eq!(c.state(&id).await.state, Some(json!({"keep": 1})));
    assert_eq!(names(f.file(&id).parent().unwrap()), vec!["state.json"]);
}

/// 프로젝트 id 로 경로를 만들므로 경로 조각이 될 수 없는 id 와 레지스트리에 없는 id 는 읽지도 쓰지도 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ids_cannot_escape_the_projects_folder_and_must_be_registered() {
    let f = Fixture::new();
    f.project("a");
    let mut c = Client::connect(&f.host, Role::Owner).await;
    let invalid = i64::from(code::INVALID_PARAMS);
    let outside = f.paths.data.join("escaped");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("state.json"), "{\"secret\": 1}").unwrap();
    for id in [
        "../escaped",
        "..",
        "",
        "a/b",
        "a\\b",
        "deadbeefdeadbeef",
        &"a".repeat(65),
    ] {
        assert_eq!(
            c.error_code("project.state.get", json!({"projectId": id}))
                .await,
            invalid,
            "get {id:?}"
        );
        assert_eq!(
            c.error_code(
                "project.state.set",
                json!({"projectId": id, "state": {"x": 1}})
            )
            .await,
            invalid,
            "set {id:?}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(outside.join("state.json")).unwrap(),
        "{\"secret\": 1}"
    );
    assert!(!f.paths.data.join("projects").exists());
}

/// 둘 다 owner 만이고 MCP 툴이 아니다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_owner_reads_and_writes_and_mcp_does_not_see_it() {
    let f = Fixture::new();
    let id = f.project("a").id;
    let denied = i64::from(code::PERMISSION_DENIED);
    for role in [Role::Readonly, Role::Mobile] {
        let mut c = Client::connect(&f.host, role).await;
        assert_eq!(
            c.error_code("project.state.get", json!({"projectId": id}))
                .await,
            denied,
            "{role:?}"
        );
        assert_eq!(
            c.error_code("project.state.set", json!({"projectId": id, "state": {}}))
                .await,
            denied,
            "{role:?}"
        );
    }
    for name in ["project.state.get", "project.state.set"] {
        let info = pw_rpc::methods()
            .into_iter()
            .find(|m| m.meta.name == name)
            .unwrap_or_else(|| panic!("{name} is declared"));
        assert!(!info.meta.mcp, "{name}");
    }
}

/// 다른 프로젝트 창(또는 웰컴 창)의 연결은 이 프로젝트의 상태를 쓰지 못한다(M1-25 브리프).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "연결이 자기 프로젝트를 아직 모른다 — M1-24 의 Host 연결 범위가 들어오면 state::connection_may_write 를 채우고 켠다"]
async fn another_projects_connection_cannot_write() {
    let f = Fixture::new();
    let a = f.project("a").id;
    let b = f.project("b").id;
    // M1-24 뒤: 프로젝트 B 의 범위로 연 연결이어야 한다(지금은 범위를 실을 길이 없다).
    let mut c = Client::connect(&f.host, Role::Owner).await;
    let _ = b;
    assert_eq!(
        c.error_code(
            "project.state.set",
            json!({"projectId": a, "state": {"x": 1}})
        )
        .await,
        i64::from(code::PERMISSION_DENIED)
    );
}
