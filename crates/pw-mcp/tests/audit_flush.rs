//! `pw_mcp::serve` 는 돌아오기 전에 그 세션의 감사 기록을 다 쓴다(`flush_audit`). 세션 하나로 끝나는 프로세스(헤드리스
//! `pitwall tool`)는 `serve` 가 돌아오면 곧 끝나므로, 기다리지 않으면 쓰기 스레드가 기록하던 중에 죽어 행을 잃는다.
//!
//! 쓰기를 늦추는 자리는 프로덕션 코드가 아니라 감사 DB 파일이다: 시험이 그 파일에 배타 잠금을 쥐면 쓰기 스레드는 SQLite 의 바쁨
//! 대기(`audit.rs` 의 `BUSY_TIMEOUT` 2초)에서 멈춘다. 그동안 `serve` 가 돌아오지 않아야 하고, 잠금을 풀면 돌아온 그 자리에서 행이
//! 있어야 한다. 시험 전용 분기 · 주입 인자는 없다.
//!
//! session.rs 와 따로 둔 것은 그 파일을 다른 행이 고치는 중이라서다(16-m5-plan M5-08 후속).

use std::sync::Arc;
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::{Catalog, HostKind, Services, SessionRequest};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use tokio_util::sync::CancellationToken;

/// 잠금을 쥐고 `serve` 가 돌아오지 않는지 보는 시간. 쓰기 스레드의 바쁨 대기(2초)보다 넉넉히 짧아야 한다 — 그 대기가 먼저 끝나면
/// 기록이 실패로 끝나 `serve` 가 돌아온다(그러면 이 시험은 잘못 실패한다).
const HELD: Duration = Duration::from_millis(700);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_returns_only_after_the_session_audit_rows_are_written() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(tmp.path());
    let host = Arc::new(LocalHost::builder("local").config(paths.clone()).build());
    let router = Arc::new(HostRouter::new());
    router.add_host(host.clone()).unwrap();
    let audit = Arc::new(AuditLog::under_state(&paths.state));
    let services = Arc::new(Services {
        router,
        host_id: "local".into(),
        paths: paths.clone(),
        global: host.config().unwrap().global.clone(),
        bus: host.bus().clone(),
        audit: Arc::clone(&audit),
        catalog: Arc::new(Catalog::linked().routed_by(host.router())),
        approval: Approval::Unavailable,
        transport: "test",
        host_kind: HostKind::Headless,
        sessions: Default::default(),
    });

    // 감사 DB 를 시험이 먼저 만들고 배타 잠금을 쥔다 — 쓰기 스레드는 파일을 열고 버전을 읽는 자리부터 기다린다.
    let file = audit.file().to_path_buf();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let lock = rusqlite::Connection::open(&file).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();

    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let mut server = tokio::spawn(pw_mcp::serve(
        services,
        SessionRequest::default(),
        tokio::io::split(server_io),
        CancellationToken::new(),
    ));
    let client = ().serve(client_io).await.unwrap();
    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("project_list"))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true), "{result:?}");
    // 응답을 받은 호출의 기록은 이미 큐에 있다. 클라이언트가 끊으면 세션이 끝난다.
    client.cancel().await.unwrap();

    // 기록이 잠금에 막힌 동안 `serve` 는 돌아오지 않는다.
    assert!(
        tokio::time::timeout(HELD, &mut server).await.is_err(),
        "serve returned while its audit row was still waiting to be written"
    );
    lock.execute_batch("ROLLBACK").unwrap();
    drop(lock);

    // 잠금을 풀면 기록이 끝나고 `serve` 가 돌아온다. 돌아온 그 자리에서 행이 있다 — 기다리지 않는다.
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("serve did not return after the lock was released")
        .unwrap()
        .unwrap();
    let conn =
        rusqlite::Connection::open_with_flags(&file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let tools: Vec<String> = conn
        .prepare("SELECT tool FROM calls ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(tools, ["project_list"]);
}
