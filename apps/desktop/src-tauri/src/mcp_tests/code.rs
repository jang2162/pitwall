//! 코드 인텔리전스 툴(16-m5-plan M5-08)의 왕복: 앱과 같은 Host 에 진짜 MCP 세션을 열어([`super::ToolHost`]) 이름 · 그룹 · 등급 ·
//! `readOnlyHint` · 스키마 · 감사 행과, 창이 없을 때의 결과(서버 · 색인을 만들지 않는다)를 본다. 툴의 동작 자체(진단의 줄 · 쪽 ·
//! 심각도 · 미신뢰, 심볼의 이름 · 종류 · 쪽)는 `pw-lsp` 의 `tests/tool.rs` 와 `pw-index` 의 `tests::tool` 이 본다. 앱 전용 툴이라
//! 헤드리스 목록에 없다는 것은 [`super::mcp_tools_are_exactly_these`] 가 본다.

use pw_mcp::SessionRequest;
use serde_json::{Value, json};

use super::{ToolHost, structured};

/// 두 툴과 프로젝트 말고 더 줄 입력.
const CODE_TOOLS: [(&str, &str); 2] = [
    ("diagnostics", "{}"),
    ("symbol_search", r#"{"query": "main"}"#),
];

fn args(extra: &str, project: &str) -> Value {
    let mut args: Value = serde_json::from_str(extra).unwrap();
    args["projectId"] = json!(project);
    args
}

/// 툴마다 왕복: 그룹 `code` · 등급 `read` · `readOnlyHint` · 스키마의 `worktree` 가 선택 입력, 부르면 결과와 감사 행이 `ok`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_tools_round_trip_through_a_real_session() {
    const { assert!(pw_lsp::tools::RESPONSE_BUDGET < pw_mcp::server::MAX_RESPONSE_BYTES) };
    const { assert!(pw_index::tools::RESPONSE_BUDGET < pw_mcp::server::MAX_RESPONSE_BYTES) };
    let host = ToolHost::app();
    let id = host.project("app");
    let session = host.session(SessionRequest::default()).await;
    let listed = session.list().await;
    for (n, (name, extra)) in CODE_TOOLS.iter().enumerate() {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier),
            ("code", pw_rpc::Tier::Read),
            "{name}"
        );
        let mcp = listed
            .iter()
            .find(|t| t.name == *name)
            .unwrap_or_else(|| panic!("`{name}` is not listed"));
        let read_only = mcp.annotations.as_ref().and_then(|a| a.read_only_hint);
        assert_eq!(read_only, Some(true), "{name}");
        let schema = &mcp.input_schema;
        assert!(
            schema["properties"].get("worktree").is_some(),
            "{name}: {schema:?}"
        );
        let required = schema.get("required").and_then(Value::as_array);
        assert!(
            !required.is_some_and(|r| r.iter().any(|k| k == "worktree")),
            "{name}: `worktree` must be optional"
        );

        let result = session.call(name, args(extra, &id)).await;
        assert_ne!(result.is_error, Some(true), "{name}: {result:?}");
        let rows = host.audit_rows(n + 1);
        assert_eq!(
            (rows[n].tool.as_str(), rows[n].outcome.as_str()),
            (*name, "ok")
        );
        assert_eq!(rows[n].project_id.as_deref(), Some(id.as_str()), "{name}");
    }
    session.close().await;
}

/// 그 프로젝트 창이 없으면: `diagnostics` 는 빈 서버 요약과 빈 목록(서버를 띄우지 않는다), `symbol_search` 는 `unindexed` 와
/// 「Pitwall 에서 열어라」(색인을 만들지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_window_nothing_is_started_and_the_agent_is_told() {
    let host = ToolHost::app();
    let id = host.project("closed");
    std::fs::create_dir_all(host.base.join("work/closed/src")).unwrap();
    std::fs::write(host.base.join("work/closed/src/main.rs"), "fn main() {}\n").unwrap();
    let session = host.session(SessionRequest::default()).await;
    let diagnostics = session.call("diagnostics", json!({"projectId": id})).await;
    let got = structured(&diagnostics);
    assert_eq!(got["servers"], json!([]), "{got:?}");
    assert_eq!(got["files"], json!([]), "{got:?}");
    let symbols = session
        .call("symbol_search", json!({"projectId": id, "query": "main"}))
        .await;
    let got = structured(&symbols);
    assert_eq!(got["symbols"], json!([]), "{got:?}");
    assert_eq!(
        got["unindexed"],
        json!([host.base.join("work/closed").display().to_string()]),
        "{got:?}"
    );
    assert!(
        got["hint"]
            .as_str()
            .is_some_and(|h| h.contains("open the project in Pitwall")),
        "{got:?}"
    );
    session.close().await;
}
