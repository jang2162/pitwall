//! 그룹 노출(`[mcp.groups]`)을 진짜 MCP 세션으로 본다: 선언이 실은 그룹이 `tools/list` 를 가른다(결정 M5-D23).
//!
//! 툴은 이 파일의 가짜 선언이다 — 실제 툴은 툴 행이 더한다. `db` 그룹에 이름 접두사가 없는 툴(`sql_explain`)을 둬서 「꺼 둔 그룹에
//! 뒤에 더해진 툴도 꺼져 있다」 를 이름이 아니라 그룹으로 본다.

use std::path::PathBuf;
use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::{Catalog, Services, SessionRequest};
use pw_rpc::{RpcError, rpc_method};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use tokio_util::sync::CancellationToken;

rpc_method! {
    /// 시험용 db 그룹 툴.
    pub DbProbe: "mcptest.dbProbe" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "db", name: "db_probe" },
    }

    /// 시험용 db 그룹 툴 — 이름에 `db_` 접두사가 없다.
    pub SqlExplain: "mcptest.sqlExplain" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "db", name: "sql_explain" },
    }

    /// 시험용 code 그룹 툴.
    pub Lint: "mcptest.lint" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "code", name: "lint" },
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    paths: AppPaths,
    host: Arc<LocalHost>,
}

impl Fixture {
    fn new(global: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("settings.toml"),
            format!("schema = 1\n{global}"),
        )
        .unwrap();
        let host = Arc::new(
            LocalHost::builder("local")
                .config(paths.clone())
                .methods(|router| {
                    router.unary(DbProbe, |_ctx, ()| async { Ok::<(), RpcError>(()) });
                    router.unary(SqlExplain, |_ctx, ()| async { Ok::<(), RpcError>(()) });
                    router.unary(Lint, |_ctx, ()| async { Ok::<(), RpcError>(()) });
                })
                .build(),
        );
        let root = base.join("work/alpha");
        std::fs::create_dir_all(&root).unwrap();
        host.registry()
            .unwrap()
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap();
        Self {
            _tmp: tmp,
            base,
            paths,
            host,
        }
    }

    async fn connect(
        &self,
        request: SessionRequest,
    ) -> (RunningService<RoleClient, ()>, CancellationToken) {
        let router = Arc::new(HostRouter::new());
        router.add_host(self.host.clone()).unwrap();
        let config = self.host.config().unwrap();
        let services = Arc::new(Services {
            router,
            host_id: "local".into(),
            paths: self.paths.clone(),
            global: config.global.clone(),
            bus: self.host.bus().clone(),
            audit: Arc::new(AuditLog::under_state(&self.paths.state)),
            catalog: Arc::new(Catalog::linked()),
            approval: Approval::Unavailable,
            transport: "test",
            host_kind: pw_mcp::HostKind::App,
            sessions: Default::default(),
        });
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            services,
            request,
            tokio::io::split(server_io),
            cancel.clone(),
        ));
        (().serve(client_io).await.unwrap(), cancel)
    }
}

async fn tool_names(client: &RunningService<RoleClient, ()>) -> Vec<String> {
    client
        .peer()
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

#[test]
fn the_declarations_carry_their_groups_into_the_catalog() {
    let catalog = Catalog::linked();
    let groups: Vec<_> = ["db_probe", "sql_explain", "lint", "project_list"]
        .iter()
        .map(|name| catalog.get(name).unwrap().group)
        .collect();
    assert_eq!(groups, ["db", "db", "code", "project"]);
}

/// `[mcp.groups] db = false` 면 db 그룹의 툴이 모두 빠지고 부를 수도 없다 — 이름 접두사와 상관없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_turned_off_drops_all_of_its_tools() {
    let f = Fixture::new("[mcp.groups]\ndb = false\n");
    let (client, cancel) = f.connect(SessionRequest::default()).await;
    assert_eq!(
        tool_names(&client).await,
        [
            "lint",
            "project_info",
            "project_list",
            "project_open",
            "project_register",
            "settings_get"
        ]
    );
    let hidden = client
        .peer()
        .call_tool(CallToolRequestParams::new("sql_explain"))
        .await;
    assert!(hidden.is_err(), "꺼진 그룹의 툴이 불렸다: {hidden:?}");
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 정확한 툴 키와 glob 은 그룹을 이긴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exact_key_and_a_glob_beat_the_group() {
    let f = Fixture::new(
        "[mcp.tools]\nsql_explain = true\n\"lin*\" = false\n[mcp.groups]\ndb = false\ncode = true\n",
    );
    let (client, cancel) = f.connect(SessionRequest::default()).await;
    assert_eq!(
        tool_names(&client).await,
        [
            "project_info",
            "project_list",
            "project_open",
            "project_register",
            "settings_get",
            "sql_explain"
        ]
    );
    client.cancel().await.unwrap();
    cancel.cancel();
}

/// 커밋되는 공유 설정(`.pitwall/project.toml`)의 `groups` 는 전역이 끈 그룹을 켜지 못한다. 끄는 것은 먹는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_project_settings_cannot_turn_a_group_on() {
    let f = Fixture::new("[mcp.groups]\ndb = false\n");
    let pitwall = f.base.join("work/alpha/.pitwall");
    std::fs::create_dir_all(&pitwall).unwrap();
    std::fs::write(
        pitwall.join("project.toml"),
        "schema = 1\n[mcp.groups]\ndb = true\ncode = false\n",
    )
    .unwrap();
    let (client, cancel) = f
        .connect(SessionRequest {
            project: Some(PathBuf::from("alpha")),
            cwd: Some(f.base.join("work")),
        })
        .await;
    assert_eq!(
        tool_names(&client).await,
        [
            "project_info",
            "project_list",
            "project_open",
            "project_register",
            "settings_get"
        ]
    );
    client.cancel().await.unwrap();
    cancel.cancel();
}
