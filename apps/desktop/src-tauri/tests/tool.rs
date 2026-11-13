//! 실행 파일로 확인하는 `pitwall tool`(16-m5-plan M5-04). GUI 를 띄우지 않는다.
//!
//! - 앱이 꺼져 있으면(헤드리스) `pitwall tool list` · `pitwall tool project_list` 가 stdout 에 결과만 쓰고 0 으로 끝난다.
//! - 이 시험 프로세스가 앱 자리에서 소켓을 연 시험 Host 에 붙어 같은 결과를 내고, 감사 행의 클라이언트가 `pitwall-cli <버전>` 이다.
//! - 오래 도는 가짜 툴을 부른 CLI 에 SIGINT 를 보내면 서버의 핸들러가 버려지고 CLI 가 130 으로 끝난다.
//! - 호출 뒤 `$STATE/logs/` 에 `cli.<날짜>.log` 가 있다.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pw_core::EventBus;
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use serde_json::Value;

/// 앱과 같은 단일 인스턴스 키(main.rs 의 `single_instance_key`)를 계산한다 — 소켓 이름이 그 키에서 나온다.
#[allow(dead_code)] // 키 계산에 쓰는 것만 부른다.
#[path = "../src/instance.rs"]
mod instance;

/// 시험 바이너리의 키를 이 실행의 것으로 가른다(`PITWALL_TEST_INSTANCE`, debug 만 듣는다).
fn test_instance() -> String {
    format!("tool{}", std::process::id())
}

/// 실제 경로. Windows 의 canonicalize 는 verbatim 접두(`\\?\`)를 붙인다 — 레지스트리가 저장하는 꼴로 맞춘다.
fn real(path: &Path) -> PathBuf {
    pw_core::registry::path::simplify(std::fs::canonicalize(path).unwrap())
}

fn command(dir: &Path, args: &[&str], cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pitwall"));
    command
        .arg("tool")
        .args(args)
        .current_dir(cwd)
        .env("PITWALL_CONFIG_DIR", dir.join("config"))
        .env("PITWALL_DATA_DIR", dir.join("data"))
        .env("PITWALL_STATE_DIR", dir.join("state"))
        .env("PITWALL_CACHE_DIR", dir.join("cache"))
        .env("PITWALL_TEST_INSTANCE", test_instance())
        .env("PITWALL_DETACHED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// 끝날 때까지 기다려 (종료 코드, stdout, stderr).
fn finish(mut child: Child) -> (Option<i32>, String, String) {
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        stdout.read_to_string(&mut s).unwrap();
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        stderr.read_to_string(&mut s).unwrap();
        s
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("`pitwall tool` 이 끝나지 않았다");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    (status.code(), out.join().unwrap(), err.join().unwrap())
}

fn tool(dir: &Path, args: &[&str], cwd: &Path) -> (Option<i32>, String, String) {
    finish(command(dir, args, cwd).spawn().unwrap())
}

/// 프로젝트 둘(`alpha` · `beta`)을 등록한 임시 `PITWALL_*_DIR`.
fn workspace() -> (tempfile::TempDir, PathBuf, AppPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = real(tmp.path());
    let paths = AppPaths::under(&dir);
    let registry = Registry::new(paths.clone(), None, EventBus::new(16));
    for name in ["alpha", "beta"] {
        let root = dir.join("work").join(name);
        std::fs::create_dir_all(&root).unwrap();
        registry
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap();
    }
    drop(registry);
    (tmp, dir, paths)
}

fn project_names(stdout: &str) -> Vec<String> {
    let projects: Value = serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout 이 JSON 문서 하나가 아니다 ({e}): {stdout:?}"));
    projects
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_owned())
        .collect()
}

fn has_cli_log(paths: &AppPaths) -> bool {
    paths
        .state
        .join("logs")
        .read_dir()
        .is_ok_and(|mut entries| {
            entries.any(|e| {
                let name = e.unwrap().file_name();
                let name = name.to_string_lossy();
                name.starts_with("cli.") && name.ends_with(".log")
            })
        })
}

#[test]
fn headless_tool_writes_only_results_to_stdout() {
    let (_tmp, dir, paths) = workspace();
    let db = paths.data.join("state.db");
    let before = std::fs::read(&db).unwrap();

    let (code, stdout, stderr) = tool(&dir, &["list"], &dir);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    let names: Vec<_> = stdout
        .lines()
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(names, ["project_info", "project_list"], "{stdout}");
    for line in stdout.lines() {
        assert_eq!(line.split_whitespace().nth(1), Some("read"), "{line}");
    }

    let (code, stdout, stderr) = tool(&dir, &["project_list"], &dir);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(project_names(&stdout), ["alpha", "beta"]);
    assert!(!stderr.contains("panicked"), "{stderr}");

    // 사용법 오류는 stdout 에 아무것도 쓰지 않는다.
    let (code, stdout, stderr) = tool(&dir, &[], &dir);
    assert_eq!((code, stdout.as_str()), (Some(2), ""), "{stderr}");
    assert!(stderr.contains("usage: pitwall tool"), "{stderr}");

    assert_eq!(
        std::fs::read(&db).unwrap(),
        before,
        "헤드리스가 레지스트리 파일을 고쳤다"
    );
    assert!(
        has_cli_log(&paths),
        "로그는 $STATE/logs/cli.<날짜>.log 로 간다"
    );
}

#[cfg(unix)]
mod app {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use pw_core::{HostRouter, LocalHost};
    use pw_mcp::audit::AuditLog;
    use pw_mcp::policy::Approval;
    use pw_mcp::{Catalog, HostKind, Services, socket};
    use pw_rpc::{RpcError, rpc_method};
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    use super::*;

    rpc_method! {
        /// 시험용: 취소될 때까지 돈다.
        pub CliSlow: "clitest.slow" {
            params: (),
            result: (),
            tier: Read,
            mcp: { group: "project", name: "clitest_slow" },
        }
    }

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// main.rs 의 `single_instance_key` 와 같은 계산.
    fn instance_key(paths: &AppPaths) -> String {
        let conf: Value = serde_json::from_str(
            &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
                .unwrap(),
        )
        .unwrap();
        let base = conf["identifier"].as_str().unwrap();
        let data =
            pw_core::registry::path::normalize(&paths.data).unwrap_or_else(|_| paths.data.clone());
        if cfg!(debug_assertions) {
            instance::with_test_suffix(
                instance::identifier(base, Some((Path::new(env!("CARGO_MANIFEST_DIR")), &data))),
                Some(&test_instance()),
                true,
            )
        } else {
            // 임시 `$DATA` 는 기본이 아니다.
            instance::release_identifier(base, Some(&data))
        }
    }

    /// 앱 자리의 시험 Host: 가짜 툴 `clitest_slow` 를 단 Host 의 세션을 그 키의 소켓에서 받는다.
    struct TestApp {
        runtime: tokio::runtime::Runtime,
        cancel: CancellationToken,
        started: Arc<Notify>,
        dropped: Arc<AtomicBool>,
    }

    impl TestApp {
        fn start(paths: &AppPaths) -> Self {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let started = Arc::new(Notify::new());
            let dropped = Arc::new(AtomicBool::new(false));
            let (s, d) = (started.clone(), dropped.clone());
            let host = Arc::new(
                LocalHost::builder("local")
                    .config(paths.clone())
                    .methods(move |router| {
                        router.unary(CliSlow, move |_ctx, ()| {
                            let (started, dropped) = (s.clone(), d.clone());
                            async move {
                                let flag = DropFlag(dropped);
                                started.notify_one();
                                tokio::time::sleep(Duration::from_secs(60)).await;
                                std::mem::forget(flag);
                                Ok::<(), RpcError>(())
                            }
                        });
                    })
                    .build(),
            );
            let router = Arc::new(HostRouter::new());
            router.add_host(host.clone()).unwrap();
            let services = Arc::new(Services {
                router,
                host_id: "local".into(),
                paths: paths.clone(),
                global: host.config().unwrap().global.clone(),
                bus: host.bus().clone(),
                audit: Arc::new(AuditLog::under_state(&paths.state)),
                catalog: Arc::new(Catalog::linked().routed_by(host.router())),
                approval: Approval::Unavailable,
                transport: "socket",
                host_kind: HostKind::App,
            });
            let uid = socket::current_uid();
            let path = socket::endpoint(
                &instance_key(paths),
                &paths.state,
                &std::env::temp_dir(),
                uid,
            )
            .unwrap();
            let cancel = CancellationToken::new();
            let listener = runtime
                .block_on(async { socket::bind(&path, uid) })
                .unwrap();
            runtime.spawn(socket::accept_loop(
                listener,
                services,
                socket::Connections::default(),
                cancel.clone(),
            ));
            Self {
                runtime,
                cancel,
                started,
                dropped,
            }
        }
    }

    impl Drop for TestApp {
        fn drop(&mut self) {
            self.cancel.cancel();
        }
    }

    /// 감사 로그의 (툴, 결과, 클라이언트, 상세) — 그 툴의 행이 생길 때까지 기다린다.
    fn audit_row(paths: &AppPaths, tool: &str) -> (String, String, Option<String>, Option<String>) {
        let file = paths.state.join("mcp").join("audit.db");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(conn) = rusqlite::Connection::open_with_flags(
                &file,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            ) && let Ok(row) = conn.query_row(
                "SELECT tool, outcome, client, detail FROM calls WHERE tool = ?1 ORDER BY id DESC",
                [tool],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ) {
                return row;
            }
            assert!(Instant::now() < deadline, "{tool} 의 감사 행이 없다");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_cli_reaches_a_running_app_through_its_socket() {
        let (_tmp, dir, paths) = workspace();
        let app = TestApp::start(&paths);

        // 앱의 목록이다 — 시험 Host 의 가짜 툴이 있다(헤드리스에는 없다).
        let (code, stdout, stderr) = tool(&dir, &["list"], &dir);
        assert_eq!(code, Some(0), "stderr: {stderr}");
        assert!(
            stdout.contains("clitest_slow"),
            "앱에 붙지 않았다: {stdout}"
        );

        let (code, stdout, stderr) = tool(&dir, &["project_list"], &dir);
        assert_eq!(code, Some(0), "stderr: {stderr}");
        assert_eq!(project_names(&stdout), ["alpha", "beta"]);
        let (tool_name, outcome, client, _) = audit_row(&paths, "project_list");
        assert_eq!(
            (tool_name.as_str(), outcome.as_str()),
            ("project_list", "ok")
        );
        assert_eq!(
            client.as_deref(),
            Some(concat!("pitwall-cli ", env!("CARGO_PKG_VERSION")))
        );

        // Ctrl+C: 서버의 핸들러가 버려지고 CLI 는 130.
        let child = command(&dir, &["clitest_slow"], &dir).spawn().unwrap();
        let pid = child.id();
        app.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), app.started.notified())
                .await
                .expect("가짜 핸들러가 시작하지 않았다")
        });
        // SAFETY: 이 시험이 띄운 자식 프로세스에 신호를 보낸다.
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) }, 0);
        let (code, stdout, stderr) = finish(child);
        assert_eq!((code, stdout.as_str()), (Some(130), ""), "stderr: {stderr}");
        // 연결이 끊긴 뒤 5초의 drain 이 아니라 취소 알림으로 거둔다 — 그보다 빨리 버려진다.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !app.dropped.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "서버의 핸들러가 버려지지 않았다");
            std::thread::sleep(Duration::from_millis(20));
        }
        let (_, outcome, client, detail) = audit_row(&paths, "clitest_slow");
        assert_eq!(
            (outcome.as_str(), detail.as_deref()),
            ("error", Some("cancelled"))
        );
        assert!(client.is_some_and(|c| c.starts_with("pitwall-cli ")));
        assert!(has_cli_log(&paths));
    }
}
