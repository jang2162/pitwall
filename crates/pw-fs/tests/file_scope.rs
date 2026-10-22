//! 단일 파일 창의 연결(파일 범위 호출자, `pw_rpc::Caller::file`)로 `fs.*` 를 부른다(12-m1-plan M1-24): 그 파일 하나만 읽고 쓰고
//! 감시한다. 같은 폴더의 다른 파일 · 상위 폴더 · `..` · 다른 파일을 가리키는 링크 · 프로젝트를 대는 요청은 모두 거절한다.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_fs::Fs;
use pw_fs::watch::Watch;
use pw_rpc::{Caller, EventHub, Outbound, Role, Router, RpcError, Session, code};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

struct Fixture {
    _tmp: tempfile::TempDir,
    /// 임시 폴더의 실제 경로.
    base: PathBuf,
    registry: Registry,
    fs: Arc<Fs>,
    hub: EventHub,
    watch: Watch,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let hub = EventHub::new();
        let bus = EventBus::with_rpc_hub(DEFAULT_CAPACITY, hub.clone());
        let registry = Registry::new(AppPaths::under(&base.join("app")), None, bus.clone());
        let fs = Arc::new(Fs::new());
        fs.attach(registry.clone());
        let watch = Watch::new(Arc::clone(&fs));
        watch.attach(bus);
        Self {
            _tmp: tmp,
            base,
            registry,
            fs,
            hub,
            watch,
        }
    }

    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.base.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    /// `file` 에만 닿는 연결.
    fn connect(&self, file: &Path) -> Conn {
        let mut router = Router::new();
        pw_fs::methods::register(&mut router, Arc::clone(&self.fs));
        pw_fs::watch::register(&mut router, self.watch.clone());
        let (session, rx) = Session::new(
            Arc::new(router),
            self.hub.clone(),
            Caller::file(Role::Owner, file.to_path_buf()),
        );
        Conn {
            session,
            rx,
            next_id: 0,
        }
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

    /// 다음 `method` 알림. 지연을 재지 않는다 — 시험을 함께 돌리는 부하 아래에서는 OS 감시를 거는 데만 10초 넘게 걸리기도 한다.
    async fn notification(&mut self, method: &str) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(30), self.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("no {method}"))
                .expect("closed");
            if let Some(note) = msg.into_notification()
                && note.method == method
            {
                return note.params;
            }
        }
    }
}

fn read(path: &Path) -> Value {
    json!({"projectId": "", "path": path.display().to_string()})
}

/// `fs.readBytes` 의 첫 16바이트.
fn page(path: &Path) -> Value {
    json!({"projectId": "", "path": path.display().to_string(), "offset": 0, "length": 16})
}

fn denied_code(result: Result<Value, RpcError>) -> i32 {
    result.expect_err("expected the request to be refused").code
}

/// 범위 밖이라 거절했다(`scope::Denied` · `Escaped`) — 판정을 지난 뒤 디스크에서 실패한 것(「폴더가 아님」 등)과 가른다.
fn outside(result: Result<Value, RpcError>) -> bool {
    let error = result.expect_err("expected the request to be refused");
    error.code == code::INVALID_PARAMS
        && error
            .data
            .as_ref()
            .and_then(Value::as_str)
            .is_some_and(|detail| detail.contains("outside"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_file_itself_is_read() {
    let f = Fixture::new();
    let file = f.file("x/a.txt", "inside");
    let sibling = f.file("x/b.txt", "sibling");
    let secret = f.file("secret.txt", "secret");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&sibling, f.base.join("x/to-b")).unwrap();
        std::os::unix::fs::symlink(&file, f.base.join("x/to-a")).unwrap();
        std::os::unix::fs::symlink("..", f.base.join("x/up")).unwrap();
    }
    let mut conn = f.connect(&file);

    let got = conn.call("fs.read", read(&file)).await.unwrap();
    assert_eq!(got["content"]["text"], "inside");
    assert_eq!(got["path"], file.display().to_string());
    let stat = conn.call("fs.stat", read(&file)).await.unwrap();
    assert_eq!(stat["kind"], "file");
    // hex 뷰어의 페이지도 그 파일은 읽는다.
    let bytes = conn.call("fs.readBytes", page(&file)).await.unwrap();
    assert_eq!(bytes["data"], "aW5zaWRl");
    assert_eq!(bytes["path"], file.display().to_string());

    let refused = vec![
        sibling.clone(),
        secret.clone(),
        f.base.join("x"),
        f.base.clone(),
        f.base.join("x/a.txt/../b.txt"),
        f.base.join("x/./../secret.txt"),
        // 그 파일 「아래」 경로는 루트 안이 아니다(폴더 루트와 다르다).
        f.base.join("x/a.txt/child"),
        // 이름이 그 파일로 시작할 뿐인 형제.
        f.base.join("x/a.txt.bak"),
        #[cfg(unix)]
        f.base.join("x/to-b"),
        #[cfg(unix)]
        f.base.join("x/up/secret.txt"),
    ];
    for path in &refused {
        for method in ["fs.read", "fs.stat"] {
            assert!(
                outside(conn.call(method, read(path)).await),
                "{method} {}",
                path.display()
            );
        }
        assert!(
            outside(conn.call("fs.readBytes", page(path)).await),
            "fs.readBytes {}",
            path.display()
        );
    }
    // 그 파일을 가리키는 링크로 온 요청은 실제 대상이 그 파일이라 받는다(결정 M1-D8 의 규칙 그대로).
    #[cfg(unix)]
    {
        let via_link = conn
            .call("fs.read", read(&f.base.join("x/to-a")))
            .await
            .unwrap();
        assert_eq!(via_link["path"], file.display().to_string());
    }
    // 상대 경로.
    assert_eq!(
        denied_code(
            conn.call("fs.read", json!({"projectId": "", "path": "b.txt"}))
                .await
        ),
        code::INVALID_PARAMS
    );
}

/// 이 Host 에 그 폴더를 담은 프로젝트가 있어도 그 프로젝트 id 를 대어 범위를 넓히지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn naming_a_project_does_not_widen_the_scope() {
    let f = Fixture::new();
    let file = f.file("proj/a.txt", "a");
    let sibling = f.file("proj/b.txt", "b");
    let project = f
        .registry
        .create(NewProject {
            root: f.base.join("proj"),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    let mut conn = f.connect(&file);
    let params = json!({"projectId": project, "path": sibling.display().to_string()});
    assert!(outside(conn.call("fs.read", params.clone()).await));
    let bytes = json!({"projectId": project, "path": sibling.display().to_string(),
                       "offset": 0, "length": 16});
    assert!(outside(conn.call("fs.readBytes", bytes).await));
    let watch = json!({"projectId": project});
    assert_eq!(
        denied_code(conn.call("fs.list", read(&f.base.join("proj"))).await),
        code::PERMISSION_DENIED
    );
    // 감시는 프로젝트를 보지 않고 그 파일을 본다.
    let opened = conn.call("fs.watch", watch).await.unwrap();
    conn.session
        .handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit",
        "params": {"streamId": opened["streamId"], "credit": 1}}));
    let chunk = conn.notification("stream.chunk").await;
    assert_eq!(
        chunk["data"],
        json!({"roots": [{"path": file.display().to_string(), "mode": "file"}]})
    );
    // 그 프로젝트의 이벤트도 듣지 못한다.
    assert_eq!(
        denied_code(
            conn.call(
                "events.subscribe",
                json!({"projectId": project, "topics": ["fs.changed"]})
            )
            .await
        ),
        code::PERMISSION_DENIED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_file_itself_is_written_and_no_other_name_appears() {
    let f = Fixture::new();
    let file = f.file("x/a.txt", "old");
    f.file("x/b.txt", "sibling");
    let mut conn = f.connect(&file);
    let version = conn.call("fs.read", read(&file)).await.unwrap()["version"].clone();
    let written = conn
        .call(
            "fs.write",
            json!({"projectId": "", "path": file.display().to_string(),
                   "content": {"kind": "text", "text": "new"}, "expectedVersion": version}),
        )
        .await
        .unwrap();
    assert_eq!(written["path"], file.display().to_string());
    assert_eq!(fs::read_to_string(&file).unwrap(), "new");

    for (path, expected) in [
        (f.base.join("x/c.txt"), Value::Null),
        (f.base.join("x/b.txt"), Value::Null),
        (f.base.join("x/a.txt/../c.txt"), Value::Null),
        (f.base.join("x/a.txt/c.txt"), Value::Null),
        (f.base.join("y.txt"), Value::Null),
    ] {
        let result = conn
            .call(
                "fs.write",
                json!({"projectId": "", "path": path.display().to_string(),
                       "content": {"kind": "text", "text": "nope"}, "expectedVersion": expected}),
            )
            .await;
        assert!(outside(result), "{}", path.display());
    }
    // 원자 쓰기의 임시 파일도 남지 않았다 — 폴더에는 원래 두 파일뿐이다.
    let mut names: Vec<String> = fs::read_dir(f.base.join("x"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt"]);
    assert!(!f.base.join("y.txt").exists());
    // 다른 이름을 만드는 조작은 부를 수도 없다.
    for method in ["fs.create", "fs.rename", "fs.copy", "fs.trash"] {
        assert_eq!(
            denied_code(conn.call(method, json!({})).await),
            code::PERMISSION_DENIED,
            "{method}"
        );
    }
}

/// 그 파일 자리에 다른 곳을 가리키는 링크를 두면(파일을 지우고 같은 이름의 링크) 그 연결은 아무것도 읽지 못한다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_replaced_by_an_outside_link_is_not_followed() {
    let f = Fixture::new();
    let file = f.file("x/a.txt", "inside");
    let secret = f.file("secret.txt", "secret");
    let mut conn = f.connect(&file);
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&secret, &file).unwrap();
    assert!(outside(conn.call("fs.read", read(&file)).await));
    let written = conn
        .call(
            "fs.write",
            json!({"projectId": "", "path": file.display().to_string(),
                   "content": {"kind": "text", "text": "x"}, "expectedVersion": null}),
        )
        .await;
    assert!(outside(written));
    assert_eq!(fs::read_to_string(&secret).unwrap(), "secret");
}

/// 감시: 그 파일이 바뀌면 그 연결의 범위(`file:<경로>`)로 `fs.changed` 가 온다. 같은 폴더의 다른 파일은 오지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_watch_reports_the_file_and_nothing_else() {
    let f = Fixture::new();
    let file = f.file("x/a.txt", "old");
    let sibling = f.file("x/b.txt", "b");
    let mut conn = f.connect(&file);
    let scope = format!("file:{}", file.display());
    conn.call(
        "events.subscribe",
        json!({"projectId": scope, "topics": ["fs.changed"]}),
    )
    .await
    .unwrap();
    let opened = conn
        .call("fs.watch", json!({"projectId": ""}))
        .await
        .unwrap();
    conn.session
        .handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit",
        "params": {"streamId": opened["streamId"], "credit": 1}}));
    conn.notification("stream.chunk").await;

    // 다른 파일의 변경은 내지 않는다 — 그 뒤 그 파일의 변경 하나만 온다.
    fs::write(&sibling, "changed").unwrap();
    // 디바운스(75 ms)의 몇 배를 기다려도 아무 이벤트가 오지 않는다.
    let early = tokio::time::timeout(
        Duration::from_millis(500),
        conn.notification("events.event"),
    )
    .await;
    assert!(
        early.is_err(),
        "the sibling's change was reported: {early:?}"
    );
    // 원자 저장(같은 폴더의 임시 파일 → rename)도 그 파일의 변경이다.
    let temp = f.base.join("x/.a.txt.tmp");
    fs::write(&temp, "new").unwrap();
    fs::rename(&temp, &file).unwrap();
    let event = conn.notification("events.event").await;
    assert_eq!(event["projectId"], scope);
    assert_eq!(
        event["payload"]["paths"],
        json!([file.display().to_string()])
    );
    assert_eq!(event["payload"]["kinds"], json!(["modified"]));

    fs::remove_file(&file).unwrap();
    let event = conn.notification("events.event").await;
    assert_eq!(event["payload"]["kinds"], json!(["removed"]));
}
