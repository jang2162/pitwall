//! 프로젝트 · 외부 도구 쓰기 툴(16-m5-plan M5-16)의 왕복: `project_register` · `open_with_upsert` 를 앱과 같은 Host 에 진짜 MCP
//! 세션으로 부르고, 가짜 창(`mcp.approvals` 허브를 듣는 연결)이 승인에 답한다. 둘 다 프로젝트가 없는 쓰기라 가장 최근에 앞에 있던
//! 창에 묻는다(결정 M5-D29). 계획 · 판정 자체(거절하는 루트 · 명령 형식 · TOCTOU)는 `pw-core` 의 `registry::tool::tests` 와
//! `pw-open-with` 의 `upsert::tests` 가 본다.

use std::sync::Arc;
use std::time::Duration;

use pw_core::registry::plan::PlanChange;
use pw_mcp::approval::{
    Answerer, Connection, Listener, McpApprovalAnswer, McpApprovalEvent, McpApprovalRequest,
};
use pw_mcp::marker::{Mark, Reason};
use pw_mcp::sessions::Sessions;
use pw_mcp::{HostKind, SessionRequest};
use pw_rpc::ConnectionScope;
use serde_json::{Value, json};

use super::{ToolHost, ToolSession, structured, text};
use crate::mcp;

/// 앱 세션과 그 세션이 쓰는 승인 허브(창이 듣는 것).
fn app_host() -> (ToolHost, Sessions) {
    let tools = ToolHost::app();
    let sessions = Sessions::new();
    let services = mcp::services(
        &tools.host,
        &tools.paths,
        "test",
        HostKind::App,
        sessions.clone(),
    );
    (ToolHost { services, ..tools }, sessions)
}

/// 창 식별자 `welcome` 의 웰컴 창(프로젝트 없음)이 듣는다.
fn welcome(sessions: &Sessions) -> Listener {
    sessions
        .approvals()
        .listen(None, Some("welcome"), Connection::named("welcome"))
}

/// 다음 새 요청(앞 요청의 닫힘은 건너뛴다).
async fn next_request(window: &mut Listener) -> McpApprovalRequest {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), window.recv())
            .await
            .expect("승인 요청이 오지 않았다")
        {
            Some(McpApprovalEvent::Open { request }) => return *request,
            Some(McpApprovalEvent::Closed { .. }) => {}
            None => panic!("듣기가 끝났다"),
        }
    }
}

fn answer(sessions: &Sessions, id: u64, answer: McpApprovalAnswer) -> Result<(), String> {
    sessions
        .approvals()
        .answer(
            &Answerer {
                scope: &ConnectionScope::Host,
                connection: Connection::named("welcome"),
            },
            id,
            answer,
        )
        .map_err(|e| format!("{e:?}"))
}

fn mark(result: &rmcp::model::CallToolResult) -> Mark {
    result
        .meta
        .as_ref()
        .and_then(|m| Mark::read(m))
        .expect("결과 표지가 없다")
}

/// 툴 호출을 띄워 두고(승인을 기다리는 동안 창이 답하게) 그 join handle 을 돌려준다.
fn spawn_call(
    session: &Arc<ToolSession>,
    name: &'static str,
    args: Value,
) -> tokio::task::JoinHandle<rmcp::model::CallToolResult> {
    let session = Arc::clone(session);
    tokio::spawn(async move { session.call(name, args).await })
}

/// 둘 다 프로젝트 없는 `write` 툴이다 — 입력에 `projectId` 가 없고(그래서 가장 최근 창에 묻는다) 계획을 만든다.
#[test]
fn the_two_tools_are_project_less_write_tools() {
    let (host, _) = app_host();
    for (name, group, method) in [
        ("project_register", "project", "project.tool.register"),
        ("open_with_upsert", "open_with", "openWith.tool.upsert"),
    ] {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier, tool.method),
            (group, pw_rpc::Tier::Write, method)
        );
        assert!(tool.planned, "{name}");
        assert!(!tool.project_scoped(), "{name}");
        assert!(
            !Value::Object(tool.input_schema_for(false))
                .to_string()
                .contains("pitwall/approval"),
            "{name}: 승인 표의 자리가 스키마에 보인다"
        );
    }
}

/// `project_register`: 가장 최근 창에 「이 프로젝트의 것이 아니다」 표지로 묻고, 묻는 동안(계획 단계)은 등록되지 않는다. 허용하면
/// 등록되고 미신뢰다. 에이전트는 그 뒤 신뢰를 바꿀 길이 없다 — 신뢰를 바꾸는 RPC 는 툴이 아니고, 에이전트 범위로 직접 불러도 거절된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_register_asks_the_front_window_and_registers_an_untrusted_project() {
    let (host, sessions) = app_host();
    let mut window = welcome(&sessions);
    let root = host.base.join("work/fresh");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("package.json"), "{}").unwrap();
    let session = Arc::new(host.session(SessionRequest::default()).await);
    let call = spawn_call(
        &session,
        "project_register",
        json!({ "root": root.display().to_string() }),
    );
    let request = next_request(&mut window).await;
    assert_eq!(request.tool, "project_register");
    assert_eq!(request.project_id, None);
    assert!(request.not_this_project);
    assert_eq!(
        request.answers,
        [
            McpApprovalAnswer::Deny,
            McpApprovalAnswer::Once,
            McpApprovalAnswer::Session
        ]
    );
    assert!(request.plan.summary.contains(&root.display().to_string()));
    let registry = host.host.registry().unwrap();
    assert_eq!(registry.list().unwrap(), Vec::new(), "묻는 동안 등록됐다");

    answer(&sessions, request.id, McpApprovalAnswer::Once).unwrap();
    let result = call.await.unwrap();
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let out = structured(&result);
    assert_eq!(out["trusted"], json!(false));
    assert_eq!(out["info"]["languages"], json!(["javascript"]));
    let id = out["info"]["project"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        registry.trust(&id).unwrap().state,
        pw_core::registry::TrustState::Unknown
    );

    // 신뢰를 바꾸는 RPC 는 툴이 아니다.
    for tool in host.services.catalog.tools() {
        assert!(
            !tool.method.to_lowercase().contains("trust"),
            "`{}` 이 신뢰를 바꾼다",
            tool.name
        );
    }
    // 에이전트 범위로 직접 불러도 거절된다(결정 M5-D18).
    for method in ["project.setTrust", "project.claimTrustPrompt"] {
        let error = agent_rpc(&host, method, json!({ "id": id, "trusted": true }))
            .await
            .unwrap_err();
        assert_eq!(error["data"]["reason"], "agentSession", "{method}: {error}");
    }
    assert!(!registry.is_trusted(&id));

    let rows = settled_rows(&host, |rows| {
        rows.iter()
            .any(|r| r.tool == "project_register" && r.outcome != "started")
    });
    let row = rows.iter().find(|r| r.tool == "project_register").unwrap();
    assert_eq!(
        (row.outcome.as_str(), row.project_id.as_deref()),
        ("ok", None)
    );
    Arc::into_inner(session).unwrap().close().await;
}

/// 감사 행이 `done` 을 만족할 때까지(선기록된 행의 결과는 응답 뒤에 따로 채워진다) 기다려 돌려준다. 5초 안에 안 되면 그때의 행.
fn settled_rows(
    host: &ToolHost,
    done: impl Fn(&[super::AuditRow]) -> bool,
) -> Vec<super::AuditRow> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let rows = host.audit_rows(1);
        if done(&rows) || std::time::Instant::now() >= deadline {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 에이전트 범위(MCP 세션의 연결)로 Host RPC 하나를 부른다.
async fn agent_rpc(host: &ToolHost, method: &str, params: Value) -> Result<Value, Value> {
    use pw_core::Host;
    let caller = pw_rpc::Caller {
        role: pw_rpc::Role::Owner,
        scope: ConnectionScope::Agent,
        window: None,
    };
    let mut connection = host.host.connect(caller).await.unwrap();
    connection.sink.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
    }));
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), connection.source.recv())
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

/// 창이 하나도 없으면 기다리지 않고 거절한다(결정 M5-D29) — 등록도 파일 쓰기도 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_window_the_writes_are_refused() {
    let (host, _sessions) = app_host();
    let root = host.base.join("work/fresh");
    std::fs::create_dir_all(&root).unwrap();
    let session = host.session(SessionRequest::default()).await;
    let result = session
        .call(
            "project_register",
            json!({ "root": root.display().to_string() }),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(mark(&result).reason, Some(Reason::NeedsApproval));
    assert!(
        text(&result).contains("no Pitwall window is open"),
        "{}",
        text(&result)
    );
    assert_eq!(host.host.registry().unwrap().list().unwrap(), Vec::new());

    let result = session
        .call(
            "open_with_upsert",
            json!({ "id": "zed", "label": "Zed", "command": ["zed", "{project_root}"] }),
        )
        .await;
    assert_eq!(
        mark(&result).reason,
        Some(Reason::NeedsApproval),
        "{}",
        text(&result)
    );
    assert!(!host.paths.config.join("settings.toml").exists());
    session.close().await;
}

/// `auto_approve = ["write"]` 이면 `project_register` 는 묻지 않고 돌지만 `open_with_upsert` 는 매번 묻고, 요청에 「이 세션 동안
/// 허용」 이 없다(그 답은 받지 않는다). 승인 diff 는 `settings.toml` 의 앞뒤이고 허용하면 그 내용이 쓰인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_with_upsert_always_asks_and_offers_no_session_allowance() {
    let (host, sessions) = app_host();
    host.global_settings("[mcp]\nauto_approve = [\"write\"]\n");
    let before = std::fs::read_to_string(host.paths.config.join("settings.toml")).unwrap();
    let mut window = welcome(&sessions);
    let session = Arc::new(host.session(SessionRequest::default()).await);

    // 대조: 같은 정책에서 `project_register` 는 묻지 않는다.
    let root = host.base.join("work/auto");
    std::fs::create_dir_all(&root).unwrap();
    let result = session
        .call(
            "project_register",
            json!({ "root": root.display().to_string() }),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert!(window.try_recv().is_none(), "auto_approve 인데 물었다");

    let args = json!({ "id": "zed", "label": "Zed", "command": ["/opt/zed", "{project_root}"] });
    for round in 0..2 {
        let call = spawn_call(&session, "open_with_upsert", args.clone());
        let request = next_request(&mut window).await;
        assert_eq!(request.tool, "open_with_upsert");
        assert!(request.not_this_project);
        assert_eq!(
            request.answers,
            [McpApprovalAnswer::Deny, McpApprovalAnswer::Once],
            "round {round}"
        );
        let [
            PlanChange::File {
                path,
                before: shown,
                after,
            },
        ] = request.plan.changes.as_slice()
        else {
            panic!("{:?}", request.plan.changes)
        };
        assert_eq!(
            path,
            &host
                .paths
                .config
                .join("settings.toml")
                .display()
                .to_string()
        );
        if round == 0 {
            assert_eq!(shown.as_deref(), Some(before.as_str()));
            assert!(
                answer(&sessions, request.id, McpApprovalAnswer::Session)
                    .unwrap_err()
                    .contains("NotOffered")
            );
        }
        let after = after.clone().unwrap();
        assert!(after.contains(
            "[open_with.tools.zed]\nlabel = \"Zed\"\ncommand = [\"/opt/zed\", \"{project_root}\"]"
        ));
        answer(&sessions, request.id, McpApprovalAnswer::Once).unwrap();
        let result = call.await.unwrap();
        assert_ne!(result.is_error, Some(true), "{}", text(&result));
        assert_eq!(
            std::fs::read_to_string(host.paths.config.join("settings.toml")).unwrap(),
            after
        );
    }
    let upserts = |rows: &[super::AuditRow]| {
        rows.iter()
            .filter(|r| r.tool == "open_with_upsert" && r.outcome == "ok")
            .count()
    };
    let rows = settled_rows(&host, |rows| upserts(rows) == 2);
    assert_eq!(upserts(&rows), 2, "{rows:?}");
    Arc::into_inner(session).unwrap().close().await;
}

/// 승인을 기다리는 사이 사람이 `settings.toml` 을 고치면, 허용해도 쓰지 않는다(`plan_changed`) — 고친 내용이 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_with_upsert_does_not_write_when_the_settings_changed_after_approval() {
    let (host, sessions) = app_host();
    host.global_settings("[ui]\nlanguage = \"ko\"\n");
    let file = host.paths.config.join("settings.toml");
    let mut window = welcome(&sessions);
    let session = Arc::new(host.session(SessionRequest::default()).await);
    let call = spawn_call(
        &session,
        "open_with_upsert",
        json!({ "id": "zed", "label": "Zed", "command": ["/opt/zed", "{project_root}"] }),
    );
    let request = next_request(&mut window).await;
    let edited = "schema = 1\n[ui]\nlanguage = \"en\"\n";
    std::fs::write(&file, edited).unwrap();
    answer(&sessions, request.id, McpApprovalAnswer::Once).unwrap();
    let result = call.await.unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        mark(&result).reason,
        Some(Reason::PlanChanged),
        "{}",
        text(&result)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
    Arc::into_inner(session).unwrap().close().await;
}

/// 헤드리스 세션(앱 없이 뜬 `pitwall mcp`)은 승인 경로가 없어 `project_register` 를 거절한다(`headless_write`). `open_with_upsert`
/// 는 헤드리스 목록에 없다(`mcp_tools_are_exactly_these`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_sessions_refuse_project_register() {
    let tools = ToolHost::app();
    let headless = mcp::headless_host(&tools.paths);
    let services = mcp::services(
        &headless,
        &tools.paths,
        "test",
        HostKind::Headless,
        Default::default(),
    );
    let host = ToolHost {
        host: headless,
        services,
        ..tools
    };
    let root = host.base.join("work/fresh");
    std::fs::create_dir_all(&root).unwrap();
    let session = host.session(SessionRequest::default()).await;
    let result = session
        .call(
            "project_register",
            json!({ "root": root.display().to_string() }),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(mark(&result).reason, Some(Reason::HeadlessWrite));
    assert!(
        session
            .try_call("open_with_upsert", json!({ "id": "zed" }))
            .await
            .is_err(),
        "헤드리스에 open_with_upsert 가 있다"
    );
    session.close().await;
}
