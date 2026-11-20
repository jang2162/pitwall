//! 디버그 툴(17-m6-plan M6-20)의 왕복: 앱과 같은 Host 에 진짜 MCP 세션을 열고 목록 · 등급 · 프리셋 · 승인 · 감사를 본다. 툴의 판정
//! 자체(식 거절 · 그 줄의 바뀜 · 루트 밖 · `debug_state` 의 상한 · 다른 프로젝트의 세션)는 `pw-debug` 의 `tests/tool.rs` 가 가짜
//! 어댑터로 본다. 헤드리스 목록에 없는 것(앱 전용)은 `mcp_tools_are_exactly_these` 가 본다.

use std::time::Duration;

use pw_core::registry::plan::PlanChange;
use pw_mcp::SessionRequest;
use pw_mcp::approval::{Connection, Listener, McpApprovalAnswer, McpApprovalRequest};
use pw_rpc::{ConnectionScope, Tier};
use serde_json::{Value, json};

use super::{ToolHost, structured, text};

const TOOLS: [(&str, &str, Tier, bool); 4] = [
    (
        "breakpoint_list",
        "debug.tool.breakpointList",
        Tier::Read,
        false,
    ),
    (
        "breakpoint_set",
        "debug.tool.breakpointSet",
        Tier::Write,
        true,
    ),
    (
        "breakpoint_set_expr",
        "debug.tool.breakpointSetExpr",
        Tier::Exec,
        true,
    ),
    ("debug_state", "debug.tool.state", Tier::Read, false),
];

fn listen(host: &ToolHost, project: &str) -> Listener {
    host.services
        .sessions
        .approvals()
        .listen(Some(project), None, Connection::named("window"))
}

/// 그 프로젝트 창이 승인 요청 하나를 받아 `answer` 로 답한다(`run` 의 것과 같다).
async fn window_answers(
    host: &ToolHost,
    listener: &mut Listener,
    project: &str,
    answer: McpApprovalAnswer,
) -> McpApprovalRequest {
    let request = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), listener.recv())
            .await
            .expect("no approval request")
            .unwrap();
        if let pw_mcp::approval::McpApprovalEvent::Open { request } = event {
            break request;
        }
    };
    host.services
        .sessions
        .approvals()
        .answer(
            &pw_mcp::approval::Answerer {
                scope: &ConnectionScope::Project(project.to_owned()),
                connection: Connection::named("window"),
            },
            request.id,
            answer,
        )
        .unwrap();
    *request
}

/// 신뢰한 프로젝트 `alpha` 와 그 안의 `src/main.rs`.
fn alpha(host: &ToolHost) -> String {
    let alpha = host.project("alpha");
    let src = host.base.join("work/alpha/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("main.rs"), "fn main() {}\n").unwrap();
    host.host
        .registry()
        .unwrap()
        .set_trust(&alpha, true)
        .unwrap();
    alpha
}

/// 지금까지 온 이벤트 가운데 새 승인 요청(`Open`)이 없다 — 앞 요청의 거둠(`Close`)은 세지 않는다.
fn no_request(listener: &mut Listener) -> bool {
    std::iter::from_fn(|| listener.try_recv())
        .all(|e| !matches!(e, pw_mcp::approval::McpApprovalEvent::Open { .. }))
}

fn listed_names(listed: &[rmcp::model::Tool]) -> Vec<String> {
    listed.iter().map(|t| t.name.to_string()).collect()
}

/// ① 넷 다 그룹 `debug` 의 프로젝트 툴이고 등급은 05 의 표 그대로다. 두 쓰기 툴은 계획을 만든다(입력 스키마에 승인 표의 자리는
/// 없다). ② `breakpoint_set` 의 입력 스키마에는 식(조건 · 로그 메시지 · hit count)의 칸이 없다 — `breakpoint_set_expr` 에는 있다.
#[test]
fn the_debug_tools_are_project_tools_in_the_debug_group() {
    let host = ToolHost::app();
    for (name, method, tier, planned) in TOOLS {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier, tool.method),
            ("debug", tier, method)
        );
        assert_eq!(tool.planned, planned, "{name}");
        assert!(tool.project_scoped(), "{name}");
        let schema = Value::Object(tool.input_schema_for(false)).to_string();
        assert!(!schema.contains("pitwall/approval"), "{name}: {schema}");
    }
    let keys = |name: &str| -> Vec<String> {
        host.tool(name).input_schema_for(false)["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    let plain = keys("breakpoint_set");
    for expression in ["condition", "logMessage", "hitCondition"] {
        assert!(
            !plain.contains(&expression.to_owned()),
            "breakpoint_set takes `{expression}`: {plain:?}"
        );
        assert!(
            keys("breakpoint_set_expr").contains(&expression.to_owned()),
            "{expression}"
        );
    }
}

/// 어댑터가 없을 때 설치 확인으로 잇는 것은 사람의 🐞 · ⌃D 뿐이다(실기 F-1) — 에이전트에게는 디버그 세션을 여는 툴(`debug.start`)도
/// 도구를 설치하는 툴(`tools.*`)도 없다. 그런 툴이 생기면 사람 몰래 설치를 띄우는 길인지 먼저 따진다.
#[test]
fn an_agent_can_neither_start_a_debug_session_nor_install_an_adapter() {
    let host = ToolHost::app();
    let reaching: Vec<(&str, &str)> = host
        .services
        .catalog
        .tools()
        .iter()
        .filter(|t| t.method == "debug.start" || t.method.starts_with("tools."))
        .map(|t| (t.name.as_str(), t.method))
        .collect();
    assert!(reaching.is_empty(), "{reaching:?}");
}

/// ① 프리셋 `standard`(설정 없음)에서는 `breakpoint_set` 이 보이고 `breakpoint_set_expr`(exec)는 없다 — `full` 에서만 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_expression_tool_is_listed_only_with_the_full_preset() {
    let host = ToolHost::app();
    let session = host.session(SessionRequest::default()).await;
    let standard = listed_names(&session.list().await);
    for name in ["breakpoint_list", "breakpoint_set", "debug_state"] {
        assert!(standard.contains(&name.to_owned()), "{name}: {standard:?}");
    }
    assert!(!standard.contains(&"breakpoint_set_expr".to_owned()));
    session.close().await;

    host.global_settings("[mcp]\npreset = \"full\"\n");
    let session = host.session(SessionRequest::default()).await;
    let full = listed_names(&session.list().await);
    assert!(full.contains(&"breakpoint_set_expr".to_owned()), "{full:?}");
    session.close().await;
}

/// ③ `auto_approve = ["write"]` 이면 `breakpoint_set` 은 묻지 않고 돌지만 `breakpoint_set_expr` 은 그 프로젝트 창에 묻는다 —
/// 요청은 계획 등급 `exec` 이고 식을 가리지 않고 그대로 보인다. 거절하면 아무것도 저장되지 않는다. `exec` 까지 자동 승인이면 묻지
/// 않는다(대조). 감사 행에는 식 원문이 없고 글자 수만 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_expression_tool_asks_even_when_writes_are_approved_automatically() {
    let host = ToolHost::app();
    host.global_settings("[mcp]\npreset = \"full\"\nauto_approve = [\"write\"]\n");
    let alpha = alpha(&host);
    let mut window = listen(&host, &alpha);
    let session = host.session(SessionRequest::default()).await;

    let plain = session
        .call(
            "breakpoint_set",
            json!({ "projectId": alpha, "path": "src/main.rs", "line": 1 }),
        )
        .await;
    assert_ne!(plain.is_error, Some(true), "{}", text(&plain));
    assert!(no_request(&mut window), "auto_approve 인데 물었다");
    assert_eq!(structured(&plain)["breakpoint"]["byAgent"], true);

    let condition = "api_secret_value_7c1d.len() > 3";
    let expr =
        json!({ "projectId": alpha, "path": "src/main.rs", "line": 1, "condition": condition });
    let (denied, request) = tokio::join!(
        session.call("breakpoint_set_expr", expr.clone()),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(request.tool, "breakpoint_set_expr");
    assert_eq!(request.plan.tier, Tier::Exec);
    assert!(
        request.plan.changes.iter().any(|c| matches!(c,
            PlanChange::Text { label, text } if label == "Condition" && text == condition)),
        "{:?}",
        request.plan.changes
    );
    let listed = session
        .call("breakpoint_list", json!({ "projectId": alpha }))
        .await;
    assert_eq!(
        structured(&listed)["files"][0]["breakpoints"][0]["condition"],
        Value::Null,
        "a denied expression is not stored"
    );
    session.close().await;

    host.global_settings("[mcp]\npreset = \"full\"\nauto_approve = [\"write\", \"exec\"]\n");
    let session = host.session(SessionRequest::default()).await;
    let set = session.call("breakpoint_set_expr", expr).await;
    assert_ne!(set.is_error, Some(true), "{}", text(&set));
    assert_eq!(structured(&set)["breakpoint"]["condition"], condition);
    assert!(no_request(&mut window), "auto_approve 인데 물었다");
    session.close().await;

    let rows = host.audit_rows(4);
    let expr_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.tool == "breakpoint_set_expr")
        .collect();
    assert_eq!(expr_rows.len(), 2, "{rows:?}");
    for row in &rows {
        assert!(!row.args.contains("api_secret_value"), "{row:?}");
        assert!(
            !row.detail
                .as_deref()
                .unwrap_or("")
                .contains("api_secret_value"),
            "{row:?}"
        );
    }
    let chars = format!("[{} chars]", condition.chars().count());
    assert!(
        expr_rows.iter().all(|r| r.args.contains(&chars)),
        "{expr_rows:?}"
    );
}

/// ④ 루트 밖 경로는 진짜 세션에서도 툴 오류다(승인을 묻지 않는다). ⑥ 디버그 세션이 없으면 `debug_state` 는 오류가 아니라
/// `noSession` 이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outside_paths_fail_and_no_session_is_not_an_error() {
    let host = ToolHost::app();
    let alpha = alpha(&host);
    std::fs::write(host.base.join("outside.rs"), "x\n").unwrap();
    let mut window = listen(&host, &alpha);
    let session = host.session(SessionRequest::default()).await;
    let outside = host.base.join("outside.rs").display().to_string();
    for path in ["../../outside.rs", outside.as_str()] {
        let result = session
            .call(
                "breakpoint_set",
                json!({ "projectId": alpha, "path": path, "line": 1 }),
            )
            .await;
        assert_eq!(result.is_error, Some(true), "{path}");
        assert!(text(&result).contains("outside"), "{}", text(&result));
    }
    assert!(
        no_request(&mut window),
        "asked for a path outside the project"
    );

    let state = session
        .call("debug_state", json!({ "projectId": alpha }))
        .await;
    assert_ne!(state.is_error, Some(true), "{}", text(&state));
    assert_eq!(structured(&state)["state"], "noSession");
    session.close().await;
}
