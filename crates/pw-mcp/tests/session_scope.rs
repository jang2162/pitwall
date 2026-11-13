//! MCP 세션의 연결 범위(결정 M5-D27 — 16-m5-plan.md M5-02 ④). 세션은 Host 범위도 창 범위도 아닌 제 범위
//! (`ConnectionScope::Agent`)로 Host 에 붙는다 — 시험용 `read` 툴의 핸들러가 본 `Ctx::scope` 로 확인한다. 지금 있는 두 툴
//! (`project_list` · `project_info`)은 범위를 보지 않으므로 새 범위에서도 그대로 된다.
//!
//! `tests/session.rs` 와 따로 둔 까닭: 그 파일은 M5-03 이 크게 고친다(병합 충돌을 피하려는 메인 판정). 시험 도우미는 여기에 필요한
//! 만큼만 둔다.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::{Catalog, HostKind, Services, SessionRequest};
use pw_rpc::{ConnectionScope, RpcError, rpc_method};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

rpc_method! {
    /// 시험용 읽기 툴. 핸들러가 본 연결 범위를 [`SEEN`] 에 남긴다.
    pub TestScope: "mcptest.scope" {
        params: (),
        result: (),
        tier: Read,
        mcp: true,
    }
}

/// [`TestScope`] 의 핸들러가 본 범위.
static SEEN: Mutex<Vec<ConnectionScope>> = Mutex::new(Vec::new());

struct Fixture {
    _tmp: tempfile::TempDir,
    paths: AppPaths,
    host: Arc<LocalHost>,
    project: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        let host = Arc::new(
            LocalHost::builder("local")
                .config(paths.clone())
                .methods(|router| {
                    router.unary(TestScope, |ctx, ()| {
                        SEEN.lock().unwrap().push(ctx.scope().clone());
                        async { Ok::<(), RpcError>(()) }
                    });
                })
                .build(),
        );
        let root = base.join("work/alpha");
        std::fs::create_dir_all(&root).unwrap();
        let project = host
            .registry()
            .unwrap()
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap()
            .id;
        Self {
            _tmp: tmp,
            paths,
            host,
            project,
        }
    }

    fn services(&self) -> Arc<Services> {
        let router = Arc::new(HostRouter::new());
        router.add_host(self.host.clone()).unwrap();
        let config = self.host.config().unwrap();
        Arc::new(Services {
            router,
            host_id: "local".into(),
            paths: self.paths.clone(),
            global: config.global.clone(),
            bus: self.host.bus().clone(),
            audit: Arc::new(AuditLog::under_state(&self.paths.state)),
            catalog: Arc::new(Catalog::linked()),
            approval: Approval::Unavailable,
            transport: "test",
            // 앱의 Host 에 붙은 세션(시험이 LocalHost 를 앱처럼 띄운다) — 범위는 Host 종류와 무관하다.
            host_kind: HostKind::App,
        })
    }

    async fn connect(&self) -> (RunningService<RoleClient, ()>, CancellationToken) {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let (r, w) = tokio::io::split(server_io);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            self.services(),
            SessionRequest {
                project: None,
                cwd: None::<PathBuf>,
            },
            (r, w),
            cancel.clone(),
        ));
        let running = ().serve(client_io).await.unwrap();
        (running, cancel)
    }
}

async fn call(client: &RunningService<RoleClient, ()>, name: &str, args: Value) -> CallToolResult {
    let mut params = CallToolRequestParams::new(name.to_owned());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    client.peer().call_tool(params).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_mcp_session_connects_with_its_own_scope() {
    let f = Fixture::new();
    let (client, cancel) = f.connect().await;

    let seen = call(&client, "mcptest_scope", json!({})).await;
    assert_eq!(seen.is_error, Some(false), "{seen:?}");
    assert_eq!(*SEEN.lock().unwrap(), [ConnectionScope::Agent]);

    // 범위를 보지 않는 지금의 두 툴은 새 범위에서도 된다.
    let list = call(&client, "project_list", json!({})).await;
    assert_eq!(list.is_error, Some(false), "{list:?}");
    let info = call(&client, "project_info", json!({"projectId": f.project})).await;
    assert_eq!(info.is_error, Some(false), "{info:?}");
    assert_eq!(
        info.structured_content.unwrap()["project"]["id"],
        json!(f.project)
    );

    cancel.cancel();
    let _ = client.cancel().await;
}
