//! `fs.*` 쓰기의 연결 범위(`pw_fs::Fs::write_roots`): 그 프로젝트의 창(`ConnectionScope::Project(그 id)`)만 그 프로젝트의 파일을 쓴다.
//! Host 연결(웰컴 · 설정 창, 창의 UI 로그) · 다른 프로젝트 창 · 다른 파일의 파일 창은 쓰기 RPC 마다 거절되고 파일 바이트가 그대로다.
//! git 쓰기를 그 프로젝트의 창으로 묶은 판정(13-m2-plan M2-06)이 `.git/config` 를 `fs.write` 로 덮는 길로 새지 않는다.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_fs::Fs;
use pw_rpc::{Caller, ConnectionScope, EventHub, Outbound, Role, Router, RpcError, Session, code};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

const GIT_CONFIG: &str = "[core]\n\trepositoryformatversion = 0\n\tbare = false\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    router: Arc<Router>,
    /// 쓰기 대상 프로젝트와 그 루트.
    id: String,
    root: PathBuf,
    /// 같은 Host 의 다른 프로젝트.
    other: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let registry = Registry::new(
            AppPaths::under(&base.join("app")),
            None,
            EventBus::new(DEFAULT_CAPACITY),
        );
        let fs = Arc::new(Fs::new());
        fs.attach(registry.clone());
        let mut router = Router::new();
        pw_fs::methods::register(&mut router, fs);
        let project = |name: &str| {
            let root = base.join(name);
            fs::create_dir_all(&root).unwrap();
            let id = registry
                .create(NewProject {
                    root: root.clone(),
                    ..NewProject::default()
                })
                .unwrap()
                .id;
            (id, root)
        };
        let (id, root) = project("p1");
        let (other, _) = project("p2");
        write(&root.join(".git/config"), GIT_CONFIG);
        write(&root.join("a.txt"), "a");
        write(&root.join("other.txt"), "other");
        Self {
            _tmp: tmp,
            base,
            router: Arc::new(router),
            id,
            root,
            other,
        }
    }

    fn connect(&self, scope: ConnectionScope) -> Conn {
        let (session, rx) = Session::new(
            Arc::clone(&self.router),
            EventHub::new(),
            Caller {
                role: Role::Owner,
                scope,
                window: None,
            },
        );
        Conn {
            session,
            rx,
            next_id: 0,
        }
    }

    fn path(&self, rel: &str) -> String {
        self.root
            .join(rel.split('/').collect::<PathBuf>())
            .display()
            .to_string()
    }

    /// 프로젝트 루트 아래 모든 파일의 바이트(폴더는 빈 값). 거절한 쓰기가 아무것도 바꾸지 않았는지 이것으로 본다.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    out.insert(path.clone(), Vec::new());
                    walk(&path, out);
                } else {
                    out.insert(path.clone(), fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &mut out);
        out
    }

    /// 프로젝트의 파일을 쓰는 RPC 전부 — 이름을 바꾸고 지우는 것까지(`fs.trash` 는 휴지통과 영구 삭제 둘 다).
    fn writes(&self, version: &Value) -> Vec<(&'static str, Value)> {
        let (id, a) = (&self.id, self.path("a.txt"));
        vec![
            (
                "fs.write",
                json!({"projectId": id, "path": a, "content": {"kind": "text", "text": "pwned"},
                       "expectedVersion": version}),
            ),
            (
                "fs.write",
                json!({"projectId": id, "path": self.path("new.txt"),
                       "content": {"kind": "text", "text": "pwned"}, "expectedVersion": null}),
            ),
            (
                "fs.create",
                json!({"projectId": id, "path": self.path("new.txt"), "kind": "file"}),
            ),
            (
                "fs.create",
                json!({"projectId": id, "path": self.path("new-dir"), "kind": "dir"}),
            ),
            (
                "fs.rename",
                json!({"projectId": id, "from": a, "to": self.path("moved.txt")}),
            ),
            (
                "fs.copy",
                json!({"projectId": id, "from": a, "to": self.path("copy.txt")}),
            ),
            (
                "fs.trash",
                json!({"projectId": id, "paths": [a], "permanent": false}),
            ),
            (
                "fs.trash",
                json!({"projectId": id, "paths": [a], "permanent": true}),
            ),
        ]
    }
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<Outbound>,
    next_id: i64,
}

impl Conn {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.next_id += 1;
        let id = self.next_id;
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), self.rx.recv())
                .await
                .expect("no response")
                .expect("closed");
            if let Some(response) = msg.into_response()
                && response.id == pw_rpc::Id::Number(id)
            {
                return response.outcome;
            }
        }
    }

    async fn version(&mut self, project: &str, path: &str) -> Value {
        self.call("fs.read", json!({"projectId": project, "path": path}))
            .await
            .unwrap()["version"]
            .clone()
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Host 연결 · 다른 프로젝트 창은 쓰기 RPC 마다 `PERMISSION_DENIED` 이고 프로젝트의 바이트가 그대로다. 요청의 `projectId` 를 그
/// 프로젝트로 대도 연결의 범위가 정한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_and_other_project_connections_write_nothing() {
    let f = Fixture::new();
    let mut own = f.connect(ConnectionScope::Project(f.id.clone()));
    let version = own.version(&f.id, &f.path("a.txt")).await;
    let before = f.snapshot();
    for (name, scope) in [
        ("host", ConnectionScope::Host),
        ("other project", ConnectionScope::Project(f.other.clone())),
    ] {
        let mut conn = f.connect(scope);
        for (method, params) in f.writes(&version) {
            let e = conn.call(method, params.clone()).await.expect_err(method);
            assert_eq!(e.code, code::PERMISSION_DENIED, "{name} {method} {params}");
        }
    }
    assert_eq!(f.snapshot(), before);
}

/// 다른 파일의 파일 창(단일 파일 창, M1-24)은 그 파일 밖을 쓰지 못한다: 이름을 만드는 조작은 디스패처가 `PERMISSION_DENIED`,
/// `fs.write` 는 그 파일 하나가 루트라 루트 밖(invalid params, M1-24 의 규칙 그대로)이다. 요청의 `projectId` 를 대도 넓어지지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_window_of_another_file_writes_nothing() {
    let f = Fixture::new();
    let mut own = f.connect(ConnectionScope::Project(f.id.clone()));
    let version = own.version(&f.id, &f.path("a.txt")).await;
    let before = f.snapshot();
    let mut conn = f.connect(Caller::file(Role::Owner, f.root.join("other.txt")).scope);
    for (method, params) in f.writes(&version) {
        let e = conn.call(method, params.clone()).await.expect_err(method);
        let expected = if method == "fs.write" {
            code::INVALID_PARAMS
        } else {
            code::PERMISSION_DENIED
        };
        assert_eq!(e.code, expected, "{method} {params} {e:?}");
    }
    assert_eq!(f.snapshot(), before);
}

/// 그 프로젝트의 창은 쓰기 RPC 를 모두 부른다. 그 파일의 파일 창은 그 파일을 쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_project_window_and_the_file_window_write_their_own() {
    let f = Fixture::new();
    let mut own = f.connect(ConnectionScope::Project(f.id.clone()));
    let version = own.version(&f.id, &f.path("a.txt")).await;
    let calls = [
        (
            "fs.write",
            json!({"projectId": f.id, "path": f.path("a.txt"),
                   "content": {"kind": "text", "text": "mine"}, "expectedVersion": version}),
        ),
        (
            "fs.create",
            json!({"projectId": f.id, "path": f.path("new.txt"), "kind": "file"}),
        ),
        (
            "fs.copy",
            json!({"projectId": f.id, "from": f.path("a.txt"), "to": f.path("copy.txt")}),
        ),
        (
            "fs.rename",
            json!({"projectId": f.id, "from": f.path("copy.txt"), "to": f.path("moved.txt")}),
        ),
        (
            "fs.trash",
            json!({"projectId": f.id, "paths": [f.path("moved.txt")], "permanent": true}),
        ),
    ];
    for (method, params) in calls {
        own.call(method, params.clone())
            .await
            .unwrap_or_else(|e| panic!("{method} {params}: {e:?}"));
    }
    assert_eq!(fs::read_to_string(f.root.join("a.txt")).unwrap(), "mine");
    assert_eq!(fs::read_to_string(f.root.join("new.txt")).unwrap(), "");
    assert!(!f.root.join("copy.txt").exists());
    assert!(!f.root.join("moved.txt").exists());

    let file = f.root.join("other.txt");
    let mut window = f.connect(Caller::file(Role::Owner, file.clone()).scope);
    let path = file.display().to_string();
    let version = window.version("", &path).await;
    window
        .call(
            "fs.write",
            json!({"projectId": "", "path": path, "content": {"kind": "text", "text": "edited"},
                   "expectedVersion": version}),
        )
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "edited");
}

/// 재리뷰어(M2-07)의 재현: 웰컴 창의 Host 연결 · 다른 프로젝트 창이 그 프로젝트의 `.git/config` 를 읽어 `version` 을 받고 그것으로
/// `fs.write` 를 부르면 덮어썼다 — `core.fsmonitor` 는 다음 git 명령에서 그 명령을 실행한다. 이제 둘 다 거절되고 바이트가 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_or_other_project_connection_cannot_overwrite_git_config() {
    let f = Fixture::new();
    let config = f.path(".git/config");
    let hook = f.base.join("pwn.sh").display().to_string();
    for scope in [
        ConnectionScope::Host,
        ConnectionScope::Project(f.other.clone()),
    ] {
        let mut conn = f.connect(scope.clone());
        let version = conn.version(&f.id, &config).await;
        let e = conn
            .call(
                "fs.write",
                json!({"projectId": f.id, "path": config,
                       "content": {"kind": "text", "text": format!("[core]\n\tfsmonitor = {hook}\n")},
                       "expectedVersion": version}),
            )
            .await
            .expect_err("the write must be refused");
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
        assert_eq!(
            fs::read_to_string(f.root.join(".git/config")).unwrap(),
            GIT_CONFIG,
            "{scope:?}"
        );
    }
}
