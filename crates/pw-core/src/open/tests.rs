use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pw_rpc::{Caller, ConnectionScope, Role, code};
use serde_json::{Value, json};

use super::*;
use crate::config::AppPaths;
use crate::registry::NewProject;
use crate::{Host, LocalHost};

/// 받은 요청을 쌓기만 하는 가짜 여는 쪽(데스크톱 대신).
#[derive(Default)]
struct FakeOpener {
    queue: Mutex<Vec<OpenRequest>>,
}

impl Opener for FakeOpener {
    fn open(&self, request: &OpenRequest) -> Result<OpenedIn, String> {
        self.queue.lock().unwrap().push(request.clone());
        Ok(OpenedIn::ExistingWindow)
    }
}

impl FakeOpener {
    fn taken(&self) -> Vec<OpenRequest> {
        std::mem::take(&mut *self.queue.lock().unwrap())
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    host: LocalHost,
    opener: Arc<FakeOpener>,
    id: String,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = normalize(tmp.path()).unwrap();
    let root = base.join("work").join("alpha");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(base.join("secret.txt"), "outside\n").unwrap();
    let requests = OpenRequests::default();
    let opener = Arc::new(FakeOpener::default());
    requests.attach(opener.clone());
    let host = LocalHost::builder("local")
        .config(AppPaths::under(&base.join("app")))
        .open_requests(requests)
        .build();
    let id = host
        .registry()
        .unwrap()
        .create(NewProject {
            root: root.clone(),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    Fixture {
        _tmp: tmp,
        base,
        host,
        opener,
        id,
        root,
    }
}

/// 연결 하나(세션 하나)로 `project.tool.open` 을 차례로 부른다.
struct Conn(crate::Connection);

impl Conn {
    async fn open(host: &LocalHost, caller: Caller) -> Self {
        Self(host.connect(caller).await.unwrap())
    }

    async fn call(&mut self, params: Value) -> Result<Value, Value> {
        self.0.sink.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "project.tool.open", "params": params,
        }));
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(10), self.0.source.recv())
                .await
                .expect("no response")
                .expect("closed");
            if frame.get("id") == Some(&Value::from(1)) {
                return match frame.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(frame["result"].clone()),
                };
            }
        }
    }
}

fn agent() -> Caller {
    Caller::agent(Role::Owner)
}

#[tokio::test]
async fn a_request_inside_the_root_lands_in_the_openers_queue() {
    let f = fixture();
    let mut conn = Conn::open(&f.host, agent()).await;
    let result = conn
        .call(json!({ "projectId": f.id, "path": "src/main.rs", "line": 10, "column": 2 }))
        .await
        .unwrap();
    let file = f.root.join("src").join("main.rs");
    assert_eq!(result["openedIn"], "existingWindow");
    assert_eq!(result["path"], file.display().to_string());
    assert_eq!(
        f.opener.taken(),
        [OpenRequest {
            project_id: f.id.clone(),
            worktree: None,
            file: Some(file.clone()),
            line: Some(10),
            column: Some(2),
        }]
    );
    // 절대 경로도 루트 안이면 같다. 경로가 없으면 프로젝트만 연다.
    conn.call(json!({ "projectId": f.id, "path": file.display().to_string() }))
        .await
        .unwrap();
    conn.call(json!({ "projectId": f.id })).await.unwrap();
    let taken = f.opener.taken();
    assert_eq!(taken[0].file.as_deref(), Some(file.as_path()));
    assert_eq!(taken[1].file, None);
}

#[tokio::test]
async fn paths_outside_the_project_root_are_refused() {
    let f = fixture();
    let secret = f.base.join("secret.txt");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, f.root.join("link.txt")).unwrap();
    let mut conn = Conn::open(&f.host, agent()).await;
    let mut paths = vec![
        "../../secret.txt".to_owned(),
        secret.display().to_string(),
        "/etc/hosts".to_owned(),
    ];
    if cfg!(unix) {
        // 루트 안 이름이지만 루트 밖을 가리키는 링크.
        paths.push("link.txt".to_owned());
    }
    for path in paths {
        let e = conn
            .call(json!({ "projectId": f.id, "path": path }))
            .await
            .unwrap_err();
        assert_eq!(e["code"], code::INVALID_PARAMS, "{path}: {e}");
        assert!(
            e["data"].as_str().unwrap().contains("outside"),
            "{path}: {e}"
        );
    }
    assert!(
        f.opener.taken().is_empty(),
        "거절한 요청은 큐에 들지 않는다"
    );
}

#[tokio::test]
async fn the_sixth_call_within_ten_seconds_is_rate_limited() {
    let f = fixture();
    let mut conn = Conn::open(&f.host, agent()).await;
    for _ in 0..RATE_LIMIT {
        conn.call(json!({ "projectId": f.id })).await.unwrap();
    }
    let e = conn.call(json!({ "projectId": f.id })).await.unwrap_err();
    assert_eq!(e["code"], code::PERMISSION_DENIED, "{e}");
    assert_eq!(e["data"]["reason"], "rateLimited", "{e}");
    assert!(e["data"]["retryAfterMs"].as_u64().unwrap() <= 10_000, "{e}");
    assert_eq!(f.opener.taken().len(), RATE_LIMIT, "여섯째는 열지 않았다");
}

/// 한도는 Host 전체에 한 벌이다 — 호출마다 새 세션으로 와도(`pitwall tool` 의 꼴) 여섯째는 거절이다(메인 판정 2026-11-14).
#[tokio::test]
async fn the_limit_is_shared_by_every_session_of_the_host() {
    let f = fixture();
    for _ in 0..RATE_LIMIT {
        let mut conn = Conn::open(&f.host, agent()).await;
        conn.call(json!({ "projectId": f.id })).await.unwrap();
    }
    let mut sixth = Conn::open(&f.host, agent()).await;
    let e = sixth.call(json!({ "projectId": f.id })).await.unwrap_err();
    assert_eq!(e["data"]["reason"], "rateLimited", "{e}");
    assert_eq!(
        f.opener.taken().len(),
        RATE_LIMIT,
        "새 세션의 여섯째도 열지 않았다"
    );
}

#[test]
fn the_rate_limit_frees_up_after_the_window() {
    let mut limiter = RateLimiter::default();
    let start = Instant::now();
    for i in 0..RATE_LIMIT {
        limiter.take(start + Duration::from_secs(i as u64)).unwrap();
    }
    let wait = limiter.take(start + Duration::from_secs(9)).unwrap_err();
    assert_eq!(
        wait,
        Duration::from_secs(1),
        "가장 오래된 호출이 창을 벗어날 때까지"
    );
    assert!(limiter.take(start + RATE_WINDOW).is_ok());
}

#[tokio::test]
async fn bad_inputs_and_other_connections_are_refused() {
    let f = fixture();
    let mut conn = Conn::open(&f.host, agent()).await;
    for params in [
        json!({ "projectId": f.id, "line": 3 }),
        json!({ "projectId": f.id, "path": "src/main.rs", "column": 3 }),
        json!({ "projectId": f.id, "path": "src/main.rs", "line": 0 }),
        json!({ "projectId": f.id, "worktree": "relative/top" }),
        json!({ "projectId": "nope" }),
    ] {
        let e = conn.call(params.clone()).await.unwrap_err();
        assert_eq!(e["code"], code::INVALID_PARAMS, "{params}: {e}");
    }
    let mut window = Conn::open(
        &f.host,
        Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(f.id.clone()),
            window: Some("main".into()),
        },
    )
    .await;
    let e = window.call(json!({ "projectId": f.id })).await.unwrap_err();
    assert_eq!(e["data"]["reason"], "notAgentSession", "{e}");
    assert!(f.opener.taken().is_empty());
}

#[tokio::test]
async fn a_host_without_an_attached_opener_refuses_and_a_headless_host_has_no_method() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("work");
    std::fs::create_dir_all(&root).unwrap();
    let host = LocalHost::builder("local")
        .config(AppPaths::under(&tmp.path().join("app")))
        .open_requests(OpenRequests::default())
        .build();
    let id = host
        .registry()
        .unwrap()
        .create(NewProject {
            root,
            ..NewProject::default()
        })
        .unwrap()
        .id;
    let mut conn = Conn::open(&host, agent()).await;
    // 열지 못한 호출은 한도에 세지 않는다 — 여섯째도 「준비되지 않았다」 다.
    for _ in 0..=RATE_LIMIT {
        let e = conn.call(json!({ "projectId": id })).await.unwrap_err();
        assert_eq!(e["code"], code::INTERNAL_ERROR, "{e}");
    }

    let headless = LocalHost::builder("local")
        .config(AppPaths::under(&tmp.path().join("app")))
        .build();
    let methods: Vec<_> = headless.router().methods().iter().map(|m| m.name).collect();
    assert!(!methods.contains(&"project.tool.open"), "{methods:?}");
    assert!(methods.contains(&"config.tool.get"), "{methods:?}");
}
