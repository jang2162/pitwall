//! 프로젝트 · 설정 조회 툴(16-m5-plan M5-09)의 왕복: `settings_get` · `open_with_list` · `project_open` 을 앱과 같은 Host 에 진짜 MCP
//! 세션으로 부른다. `project_open` 은 Tauri 의 mock 앱에 창을 여는 쪽(`agent_open::WindowOpener`)을 붙여, 요청이 그 프로젝트 창의
//! 보류 큐 · 새 창이 되는지와 창을 앞으로 가져오지 않는지(결정 M5-D24)를 본다. 판정 자체(루트 밖 · 빈도 · 관문)는 `pw-core` 의
//! `open::tests` · `config::tool::tests` 가 본다.

use std::sync::Arc;

use pw_core::open::OpenRequests;
use pw_mcp::marker::{Mark, Reason};
use pw_mcp::{HostKind, SessionRequest};
use serde_json::json;
use tauri::Manager as _;
use tauri::test::MockRuntime;

use super::{ToolHost, ToolSession, structured};
use crate::mcp;
use crate::windows::{self, ProjectOpen};

/// 앱과 같은 Host 에 창을 여는 쪽을 붙인 것. 창은 Tauri 의 mock 앱에 열린다.
struct WindowHost {
    tools: ToolHost,
    app: tauri::App<MockRuntime>,
}

impl WindowHost {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = pw_core::config::AppPaths::under(&base.join("app"));
        let requests = OpenRequests::default();
        let host = Arc::new(crate::local_host_with(
            paths.clone(),
            Arc::new(pw_diag::Diagnostics::new(base.join("logs"), "0.0.0")),
            Arc::new(pw_ext::Registry::default()),
            Arc::new(pw_i18n::I18n::new(pw_i18n::Catalog::default(), Vec::new())),
            Arc::new(pw_fs::Fs::new()),
            pw_git::write::Askpass::default(),
            requests.clone(),
            Default::default(),
        ));
        let services = mcp::services(&host, &paths, "test", HostKind::App, Default::default());
        let app = tauri::test::mock_app();
        app.manage(crate::launch::Launch::new(host.registry().cloned()));
        app.manage(windows::PendingOpens::default());
        requests.attach(Arc::new(crate::agent_open::WindowOpener::new(
            app.handle().clone(),
        )));
        Self {
            tools: ToolHost {
                _tmp: tmp,
                base,
                paths,
                host,
                services,
            },
            app,
        }
    }

    /// 그 프로젝트 창이 아직 꺼내지 않은 열기 요청(꺼내면 비운다).
    fn take_queue(&self, project: &str) -> Vec<ProjectOpen> {
        self.app
            .state::<windows::PendingOpens>()
            .take(&windows::project_label(project))
    }

    fn has_window(&self, project: &str) -> bool {
        self.app
            .get_webview_window(&windows::project_label(project))
            .is_some()
    }
}

fn mark(result: &rmcp::model::CallToolResult) -> Option<Mark> {
    result.meta.as_ref().and_then(|m| Mark::read(m))
}

/// 툴 이름 · 그룹 · 등급(헤드리스 목록에 `settings_get` 만 있다는 것은 `mcp_tools_are_exactly_these`).
#[test]
fn the_three_tools_are_read_tools_of_their_groups() {
    let host = ToolHost::app();
    for (name, group) in [
        ("settings_get", "project"),
        ("project_open", "project"),
        ("open_with_list", "open_with"),
    ] {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier),
            (group, pw_rpc::Tier::Read),
            "{name}"
        );
    }
}

/// `settings_get` 은 구버전 `project.toml` 을 읽어도 그 파일을 바이트 그대로 두고 `.backup/` 을 만들지 않는다 — 앱 세션 · 헤드리스
/// 세션 둘 다(코어 툴). 값마다 레이어가 실린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_get_reads_an_old_file_without_writing_in_the_app_and_headless() {
    let host = ToolHost::app();
    let alpha = host.project("alpha");
    let pitwall = host.base.join("work/alpha/.pitwall");
    std::fs::create_dir_all(&pitwall).unwrap();
    let old = "# no schema key\n[mcp]\npreset = \"read-only\"\n";
    std::fs::write(pitwall.join("project.toml"), old).unwrap();
    host.global_settings("[ui]\nlanguage = \"ko\"\n");

    let headless = mcp::headless_host(&host.paths);
    let headless_services = mcp::services(
        &headless,
        &host.paths,
        "test",
        HostKind::Headless,
        Default::default(),
    );
    for services in [Arc::clone(&host.services), headless_services] {
        let session = ToolSession::open(services, SessionRequest::default()).await;
        let result = session
            .call(
                "settings_get",
                json!({ "projectId": alpha, "key": "mcp.preset" }),
            )
            .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let entries = structured(&result)["entries"].clone();
        assert_eq!(
            entries,
            json!([{ "key": "mcp.preset", "value": "read-only", "layer": "shared", "redacted": false }])
        );
        let global = structured(&session.call("settings_get", json!({ "key": "ui" })).await);
        assert_eq!(global["entries"][0]["layer"], "global", "{global:?}");
        session.close().await;
    }
    assert_eq!(
        std::fs::read_to_string(pitwall.join("project.toml")).unwrap(),
        old
    );
    assert!(!pitwall.join(".backup").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_with_list_answers_an_mcp_session() {
    let host = ToolHost::app();
    let alpha = host.project("alpha");
    let session = host.session(SessionRequest::default()).await;
    let result = session
        .call("open_with_list", json!({ "projectId": alpha }))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let listed = structured(&result);
    assert!(listed["tools"].is_array(), "{listed:?}");
    assert_eq!(listed["default"]["kind"], "unset");
    let rows = host.audit_rows(1);
    assert_eq!(
        (rows[0].tool.as_str(), rows[0].outcome.as_str()),
        ("open_with_list", "ok")
    );
    session.close().await;
}

/// 요청 → 그 프로젝트 창이 그 파일 · 줄을 연다. 창이 없던 프로젝트는 창이 포커스 없이 만들어지고, 열린 창은 앞으로 가져오지 않고
/// 큐에만 넣는다(결정 M5-D24). 루트 밖 경로는 열지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_open_opens_the_file_without_bringing_the_window_forward() {
    let w = WindowHost::new();
    let id = w.tools.project("m5-09-open-target");
    let root = w.tools.base.join("work/m5-09-open-target");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let label = windows::project_label(&id);
    let file = |line| ProjectOpen {
        file: Some(root.join("src/main.rs").display().to_string()),
        line: Some(line),
        ..ProjectOpen::default()
    };
    assert!(!w.has_window(&id));
    let session = w.tools.session(SessionRequest::default()).await;

    let first = session
        .call(
            "project_open",
            json!({ "projectId": id, "path": "src/main.rs", "line": 10 }),
        )
        .await;
    assert_ne!(first.is_error, Some(true), "{first:?}");
    assert_eq!(structured(&first)["openedIn"], "newWindow");
    assert!(w.has_window(&id), "창이 없던 프로젝트는 창이 만들어진다");
    assert_eq!(
        windows::tests::built_focus(&label),
        [false],
        "포커스 없이 만든다"
    );
    // 창 제목은 레지스트리의 프로젝트 이름이다 — `launch::open_project_as` 가 넘기는 값을 본다(이름은 루트 폴더 이름).
    assert_eq!(
        windows::tests::built_titles(&label),
        ["m5-09-open-target"],
        "창 제목은 프로젝트 이름"
    );
    assert_eq!(w.take_queue(&id), [file(10)]);

    let second = session
        .call(
            "project_open",
            json!({ "projectId": id, "path": "src/main.rs", "line": 20 }),
        )
        .await;
    assert_eq!(structured(&second)["openedIn"], "existingWindow");
    assert_eq!(
        windows::tests::built_focus(&label),
        [false],
        "새로 만들지 않았다"
    );
    assert!(
        !windows::tests::was_focused(&label),
        "앞으로 가져오지 않는다"
    );
    assert_eq!(w.take_queue(&id), [file(20)]);

    let outside = session
        .call(
            "project_open",
            json!({ "projectId": id, "path": "../../app" }),
        )
        .await;
    assert_eq!(outside.is_error, Some(true), "{outside:?}");
    assert_eq!(mark(&outside), Some(Mark::error(None)));
    assert!(w.take_queue(&id).is_empty());
    session.close().await;
}

/// 미신뢰 프로젝트도 창은 만들어지지만 신뢰는 그대로다 — 그 창의 연결로 언어 서버를 띄우는 관문(`lsp_grant`)이 거절하고, 런타임
/// 제공자 관문(`trust::TrustGate` — 앱이 `Runtime::with_trust` 에 넘기는 판정)이 미신뢰로 본다. 창의 웹뷰는 mock 이라 돌지 않으므로
/// 창이 부를 관문을 그 창의 범위로 직접 부른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_projects_new_window_does_not_start_providers_or_servers() {
    let w = WindowHost::new();
    let id = w.tools.project("m5-09-untrusted");
    let root = w.tools.base.join("work/m5-09-untrusted");
    std::fs::write(root.join("mise.toml"), "[tools]\nnode = \"22\"\n").unwrap();
    let registry = w.tools.host.registry().unwrap().clone();
    assert!(!registry.is_trusted(&id));
    let session = w.tools.session(SessionRequest::default()).await;
    let opened = session
        .call(
            "project_open",
            json!({ "projectId": id, "path": "mise.toml" }),
        )
        .await;
    assert_eq!(structured(&opened)["openedIn"], "newWindow", "{opened:?}");
    assert!(w.has_window(&id));
    session.close().await;

    assert!(
        !registry.is_trusted(&id),
        "창을 만들어도 신뢰는 바뀌지 않는다"
    );
    let gate = crate::trust::TrustGate::default();
    gate.attach(registry.clone());
    assert!(!(gate.check())(&id), "런타임 제공자(mise)를 고르지 않는다");
    let Err(refused) = pw_lsp::lsp_grant(
        &pw_rpc::ConnectionScope::Project(id.clone()),
        tokio_util::sync::CancellationToken::new(),
        &registry,
        &id,
        None,
    ) else {
        panic!("미신뢰 프로젝트의 창은 언어 서버를 띄우지 못한다");
    };
    assert_eq!(refused.data.unwrap()["reason"], "untrusted");
}

/// 세션에 프로젝트가 있으면(`pitwall tool` 의 cwd · `--project`) `project_open` 도 다른 프로젝트 툴처럼 `projectId` 가 필수가 아니고,
/// 빠지면 서버가 세션 프로젝트로 채운다(05-mcp.md 「툴 레지스트리」 의 「프로젝트 지정」).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_open_takes_the_session_project_when_project_id_is_left_out() {
    let w = WindowHost::new();
    let id = w.tools.project("m5-09-session");
    let root = w.tools.base.join("work/m5-09-session");
    let session = w
        .tools
        .session(SessionRequest {
            project: Some(root.clone()),
            cwd: None,
        })
        .await;
    let listed = session.list().await;
    let schema = &listed
        .iter()
        .find(|t| t.name == "project_open")
        .unwrap()
        .input_schema;
    let required = schema.get("required").and_then(|r| r.as_array());
    assert!(
        required.is_none_or(|r| !r.iter().any(|k| k == "projectId")),
        "{schema:?}"
    );
    let opened = session.call("project_open", json!({})).await;
    assert_ne!(opened.is_error, Some(true), "{opened:?}");
    assert_eq!(structured(&opened)["openedIn"], "newWindow");
    assert!(w.has_window(&id));
    session.close().await;
}

/// 앱 전체로 10초에 5번 — 호출마다 새 MCP 세션이어도(`pitwall tool` 의 꼴) 여섯째는 거절이고 결과 표지가 `denied` ·
/// `rate_limited`(CLI 종료 3), 감사 행은 `denied` 다(메인 판정 2026-11-14 — Host 전체 한도).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sixth_project_open_within_ten_seconds_is_rate_limited() {
    let w = WindowHost::new();
    let id = w.tools.project("m5-09-rate");
    for _ in 0..pw_core::open::RATE_LIMIT {
        let session = w.tools.session(SessionRequest::default()).await;
        let ok = session
            .call("project_open", json!({ "projectId": id }))
            .await;
        assert_ne!(ok.is_error, Some(true), "{ok:?}");
        session.close().await;
    }
    let session = w.tools.session(SessionRequest::default()).await;
    let sixth = session
        .call("project_open", json!({ "projectId": id }))
        .await;
    assert_eq!(sixth.is_error, Some(true));
    assert_eq!(
        mark(&sixth),
        Some(Mark::denied(Reason::RateLimited)),
        "{sixth:?}"
    );
    // 세션마다 감사 기록 스레드가 따로라 행의 순서는 세션 순서와 다를 수 있다 — 결과별 수로 본다.
    let rows = w.tools.audit_rows(pw_core::open::RATE_LIMIT + 1);
    let denied = rows.iter().filter(|r| r.outcome == "denied").count();
    assert_eq!(
        (rows.len(), denied),
        (pw_core::open::RATE_LIMIT + 1, 1),
        "{rows:?}"
    );
    session.close().await;
}

impl ToolSession {
    /// [`ToolHost::session`] 과 같고 세션의 `Services` 를 고른다(헤드리스).
    async fn open(services: Arc<pw_mcp::Services>, request: SessionRequest) -> Self {
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        let cancel = tokio_util::sync::CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            services,
            request,
            tokio::io::split(server_io),
            cancel.clone(),
        ));
        let client = rmcp::ServiceExt::serve((), client_io)
            .await
            .expect("MCP 세션이 열리지 않았다");
        ToolSession { client, cancel }
    }
}
