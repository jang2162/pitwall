//! `project.console.*`(14-m3-plan.md M3-15 — SQL 콘솔 파일, `$DATA/projects/<id>/consoles/<콘솔 id>.sql`)를 LocalHost 연결의 JSON
//! 메시지로 왕복한다. 쓰기 · 지우기는 그 프로젝트로 연 연결(프로젝트 창)만, 읽기 · 목록은 owner 만이다.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::consoles::{CONSOLE_LIMIT, ConsoleFile, ConsoleText};
use pw_core::registry::{NewProject, Project};
use pw_core::{Connection, Host, LocalHost};
use pw_rpc::{Caller, ConnectionScope, Role, code};
use serde_json::{Value, json};

struct Client {
    conn: Connection,
    next_id: i64,
}

impl Client {
    async fn connect(host: &LocalHost, role: Role) -> Self {
        Self::with(host, Caller::new(role)).await
    }

    /// 프로젝트 창처럼 그 프로젝트로 연 연결(Host Router 가 채우는 범위, `pw_core::HostRouter::connect`).
    async fn of_project(host: &LocalHost, role: Role, id: &str) -> Self {
        Self::with(
            host,
            Caller {
                role,
                scope: ConnectionScope::Project(id.to_owned()),
                window: None,
            },
        )
        .await
    }

    async fn with(host: &LocalHost, caller: Caller) -> Self {
        Self {
            conn: host.connect(caller).await.unwrap(),
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

    async fn read(&mut self, id: &str, console: &str) -> Option<String> {
        let got: ConsoleText = self
            .ok(
                "project.console.read",
                json!({"projectId": id, "consoleId": console}),
            )
            .await;
        got.text
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

    fn dir(&self, id: &str) -> PathBuf {
        self.paths.data.join("projects").join(id).join("consoles")
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

/// 쓰고 읽기 · 목록 · 바꾸기 · 지우기. 글은 바이트 그대로다(CRLF · 한글), 임시 파일은 남지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consoles_round_trip_in_the_project_window() {
    let f = Fixture::new();
    let a = f.project("a").id;
    let mut c = Client::connect(&f.host, Role::Owner).await;
    let mut w = Client::of_project(&f.host, Role::Owner, &a).await;

    let empty: Vec<ConsoleFile> = c.ok("project.console.list", json!({"projectId": a})).await;
    assert!(empty.is_empty());
    assert_eq!(c.read(&a, "c1").await, None);

    let text = "SELECT 1;\r\nSELECT '한글';\r\n";
    let _: () = w
        .ok(
            "project.console.write",
            json!({"projectId": a, "consoleId": "c1", "text": text}),
        )
        .await;
    let _: () = w
        .ok(
            "project.console.write",
            json!({"projectId": a, "consoleId": "c0", "text": ""}),
        )
        .await;
    assert_eq!(c.read(&a, "c1").await.as_deref(), Some(text));
    let listed: Vec<ConsoleFile> = c.ok("project.console.list", json!({"projectId": a})).await;
    let ids: Vec<&str> = listed.iter().map(|f| f.console_id.as_str()).collect();
    assert_eq!(ids, ["c0", "c1"]);
    assert_eq!(listed[1].size, text.len() as u64);
    assert!(listed[1].modified_ms.is_some());
    assert_eq!(names(&f.dir(&a)), ["c0.sql", "c1.sql"]);

    let _: () = w
        .ok(
            "project.console.write",
            json!({"projectId": a, "consoleId": "c1", "text": "SELECT 2"}),
        )
        .await;
    assert_eq!(c.read(&a, "c1").await.as_deref(), Some("SELECT 2"));
    for _ in 0..2 {
        let _: () = w
            .ok(
                "project.console.delete",
                json!({"projectId": a, "consoleId": "c1"}),
            )
            .await;
    }
    assert_eq!(c.read(&a, "c1").await, None);
    // 이름 꼴이 아닌 파일은 목록에 없다.
    std::fs::write(f.dir(&a).join("Notes.sql"), "x").unwrap();
    std::fs::write(f.dir(&a).join("c2.txt"), "x").unwrap();
    let listed: Vec<ConsoleFile> = c.ok("project.console.list", json!({"projectId": a})).await;
    assert_eq!(listed.len(), 1);
}

/// 쓰기 · 지우기는 그 프로젝트 창만, 읽기는 owner 만. 이름 · 크기 · 없는 프로젝트는 invalid params 이고 파일을 만들지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_project_window_writes_its_consoles() {
    let f = Fixture::new();
    let a = f.project("a").id;
    let b = f.project("b").id;
    let write = json!({"projectId": a, "consoleId": "c1", "text": "DROP TABLE t"});
    let mut host = Client::connect(&f.host, Role::Owner).await;
    let mut other = Client::of_project(&f.host, Role::Owner, &b).await;
    for client in [&mut host, &mut other] {
        assert_eq!(
            client
                .error_code("project.console.write", write.clone())
                .await,
            i64::from(code::PERMISSION_DENIED)
        );
        assert_eq!(
            client
                .error_code(
                    "project.console.delete",
                    json!({"projectId": a, "consoleId": "c1"})
                )
                .await,
            i64::from(code::PERMISSION_DENIED)
        );
    }
    let mut mobile = Client::connect(&f.host, Role::Mobile).await;
    assert_ne!(
        mobile
            .call(
                "project.console.read",
                json!({"projectId": a, "consoleId": "c1"})
            )
            .await
            .get("error"),
        None
    );

    let mut w = Client::of_project(&f.host, Role::Owner, &a).await;
    for console in ["", "../x", "A1", "a.b", &"a".repeat(65)] {
        assert_eq!(
            w.error_code(
                "project.console.write",
                json!({"projectId": a, "consoleId": console, "text": ""})
            )
            .await,
            i64::from(code::INVALID_PARAMS),
            "{console:?}"
        );
    }
    assert_eq!(
        w.error_code(
            "project.console.write",
            json!({"projectId": a, "consoleId": "big", "text": " ".repeat(CONSOLE_LIMIT + 1)})
        )
        .await,
        i64::from(code::INVALID_PARAMS)
    );
    assert_eq!(
        host.error_code(
            "project.console.read",
            json!({"projectId": "nope", "consoleId": "c1"})
        )
        .await,
        i64::from(code::INVALID_PARAMS)
    );
    assert!(!f.dir(&a).exists());
}
