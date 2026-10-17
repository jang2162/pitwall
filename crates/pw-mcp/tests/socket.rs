//! 앱의 MCP 로컬 소켓: 위치 · 권한 · 남은 소켓 · 워크트리 격리 · 핸드셰이크 · 연결 정리.
#![cfg(unix)]

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::socket::{self, Connections, MAX_PATH};
use pw_mcp::{Catalog, Services, SessionRequest};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use tokio_util::sync::CancellationToken;

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    (tmp, base)
}

#[test]
fn the_endpoint_is_keyed_by_instance_and_state() {
    let uid = socket::current_uid();
    let temp = Path::new("/t");
    let a = socket::endpoint("dev.pitwall.desktop.dev-aaaa", Path::new("/s"), temp, uid).unwrap();
    assert_eq!(a.parent().unwrap(), Path::new("/s/run"));
    assert_eq!(
        a,
        socket::endpoint("dev.pitwall.desktop.dev-aaaa", Path::new("/s"), temp, uid).unwrap(),
        "같은 키 · 같은 상태 폴더면 같은 경로"
    );
    assert_ne!(
        a,
        socket::endpoint("dev.pitwall.desktop.dev-bbbb", Path::new("/s"), temp, uid).unwrap(),
        "다른 워크트리(키)"
    );
    assert_ne!(
        a.file_name(),
        socket::endpoint("dev.pitwall.desktop.dev-aaaa", Path::new("/s2"), temp, uid)
            .unwrap()
            .file_name(),
        "다른 상태 폴더"
    );
}

#[test]
fn a_long_state_dir_falls_back_to_the_per_user_temp_dir() {
    let uid = socket::current_uid();
    let deep = PathBuf::from(format!("/{}", "d".repeat(100)));
    let path = socket::endpoint("k", &deep, Path::new("/tmp-x"), uid).unwrap();
    assert_eq!(
        path.parent().unwrap(),
        Path::new(&format!("/tmp-x/pitwall-{uid}"))
    );
    assert!(path.as_os_str().len() <= MAX_PATH);
    assert!(socket::endpoint("k", &deep, &deep, uid).is_err());
}

#[tokio::test]
async fn bind_makes_a_private_dir_and_socket() {
    let (_tmp, base) = tmp();
    let uid = socket::current_uid();
    let dir = base.join("state/run");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.join("x.sock");
    let listener = socket::bind(&path, uid).unwrap();
    assert_eq!(mode(&dir), 0o700, "열린 폴더를 0700 으로 고친다");
    assert_eq!(mode(&path), 0o600);
    socket::check_endpoint(&path, uid).unwrap();
    drop(listener);
    assert!(!path.exists(), "버리면 소켓 파일을 지운다");
}

#[tokio::test]
async fn a_live_socket_is_not_stolen_and_a_stale_one_is_replaced() {
    let (_tmp, base) = tmp();
    let uid = socket::current_uid();
    let path = base.join("run/x.sock");
    let first = socket::bind(&path, uid).unwrap();
    let err = socket::bind(&path, uid).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    // 받는 쪽이 사라진 소켓 파일(앱이 죽고 남은 것). Drop 이 지우지 않게 파일을 잊는다.
    std::mem::forget(first);
    let stale = std::os::unix::net::UnixListener::bind(base.join("run/stale.sock")).unwrap();
    drop(stale);
    let replaced = socket::bind(&base.join("run/stale.sock"), uid).unwrap();
    assert_eq!(mode(replaced.path()), 0o600);
}

#[tokio::test]
async fn non_sockets_and_symlinked_dirs_are_refused() {
    let (_tmp, base) = tmp();
    let uid = socket::current_uid();
    let dir = base.join("run");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.join("x.sock");
    std::fs::write(&file, "not a socket").unwrap();
    assert!(socket::bind(&file, uid).is_err());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "not a socket",
        "지우지 않는다"
    );
    assert!(socket::check_endpoint(&file, uid).is_err());

    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
    let linked = base.join("linked");
    symlink(&elsewhere, &linked).unwrap();
    let err = socket::bind(&linked.join("x.sock"), uid).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");

    // 다른 사용자에게 열린 폴더의 소켓에는 클라이언트가 붙지 않는다.
    let open = base.join("open");
    let listener = socket::bind(&open.join("x.sock"), uid).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = socket::check_endpoint(listener.path(), uid).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
}

struct App {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    path: PathBuf,
    connections: Connections,
    cancel: CancellationToken,
}

/// 프로젝트 둘을 등록한 Host 와 그 소켓 서버.
async fn app() -> App {
    let (tmp, base) = tmp();
    let paths = AppPaths::under(&base.join("app"));
    let host = Arc::new(LocalHost::with_config("local", paths.clone()));
    for name in ["one", "two"] {
        let root = base.join(name);
        std::fs::create_dir_all(&root).unwrap();
        host.registry()
            .unwrap()
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap();
    }
    let router = Arc::new(HostRouter::new());
    router.add_host(host.clone()).unwrap();
    let services = Arc::new(Services {
        router,
        host_id: "local".into(),
        paths: paths.clone(),
        global: host.config().unwrap().global.clone(),
        bus: host.bus().clone(),
        audit: Arc::new(AuditLog::under_state(&paths.state)),
        catalog: Arc::new(Catalog::linked()),
        approval: Approval::Unavailable,
        transport: "socket",
    });
    let uid = socket::current_uid();
    let path = socket::endpoint("test-key", &paths.state, &base, uid).unwrap();
    let listener = socket::bind(&path, uid).unwrap();
    let connections = Connections::default();
    let cancel = CancellationToken::new();
    tokio::spawn(socket::accept_loop(
        listener,
        services,
        connections.clone(),
        cancel.clone(),
    ));
    App {
        _tmp: tmp,
        base,
        path,
        connections,
        cancel,
    }
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_calls_tools_over_the_socket_and_its_session_is_cleaned_up() {
    let app = app().await;
    let uid = socket::current_uid();
    let stream = socket::connect(
        &app.path,
        uid,
        SessionRequest {
            project: None,
            cwd: Some(app.base.join("two")),
        },
    )
    .await
    .unwrap();
    let client = ().serve(stream).await.unwrap();
    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("project_list"))
        .await
        .unwrap();
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    let projects: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    assert_eq!(projects.len(), 2);
    assert_eq!(app.connections.active(), 1);

    client.cancel().await.unwrap();
    wait_for(
        "클라이언트가 끊었는데 앱 쪽 세션이 남았다",
        || app.connections.active() == 0,
    )
    .await;
    app.cancel.cancel();
}

/// 다른 워크트리(다른 키)의 `pitwall mcp` 는 이 앱의 소켓 경로를 계산하지 않는다 — 붙을 곳이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_instance_key_does_not_reach_this_app() {
    let app = app().await;
    let uid = socket::current_uid();
    let state = app.path.parent().unwrap().parent().unwrap();
    let other = socket::endpoint("other-worktree-key", state, &app.base, uid).unwrap();
    assert_ne!(other, app.path);
    let err = socket::connect(&other, uid, SessionRequest::default())
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
    app.cancel.cancel();
}

/// 프로토콜이 다른 핸드셰이크는 거절되고 세션이 열리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_foreign_handshake_is_refused() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let app = app().await;
    let mut stream = tokio::net::UnixStream::connect(&app.path).await.unwrap();
    stream
        .write_all(b"{\"protocol\":\"something-else\"}\n")
        .await
        .unwrap();
    let mut line = String::new();
    BufReader::new(&mut stream)
        .read_line(&mut line)
        .await
        .unwrap();
    let welcome: socket::Welcome = serde_json::from_str(&line).unwrap();
    assert!(!welcome.ok);
    assert!(welcome.error.unwrap().contains("something-else"));
    wait_for("거절한 연결이 남았다", || {
        app.connections.active() == 0
    })
    .await;
    app.cancel.cancel();
}
