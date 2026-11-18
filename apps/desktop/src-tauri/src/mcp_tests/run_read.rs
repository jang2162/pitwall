//! 실행 툴 ① 조회(17-m6-plan M6-12)의 목록 · 왕복: `run_config_list` · `run_status` · `run_logs` 를 앱과 같은 Host 에 진짜 MCP
//! 세션으로 부른다. 로그 고르기 · 상한 · ANSI · 범위의 판정 자체는 `pw-run` 의 `runs::tool::tests` 가 본다. 헤드리스 목록에 없다는
//! 것은 `mcp_tools_are_exactly_these`.

use pw_mcp::SessionRequest;
use serde_json::json;

use super::{ToolHost, structured, text};

/// 완료 기준 ①: 세 툴은 그룹 `run` 의 `read` 툴이다. ⑤: 앱 전용 — 헤드리스 Host 의 목록에 없다.
#[test]
fn the_run_read_tools_are_read_tools_of_the_run_group() {
    let host = ToolHost::app();
    let headless = crate::mcp::headless_host(&host.paths);
    let headless = crate::mcp::services(
        &headless,
        &host.paths,
        "test",
        pw_mcp::HostKind::Headless,
        Default::default(),
    );
    for name in ["run_config_list", "run_status", "run_logs"] {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier),
            ("run", pw_rpc::Tier::Read),
            "{name}"
        );
        assert!(!tool.planned, "{name} asks for no approval");
        assert!(headless.catalog.get(name).is_none(), "{name} is app-only");
    }
}

/// 진짜 세션으로 세 툴을 부른다 — 구성 목록(가린 env), 빈 실행 목록, 없는 실행의 로그는 not found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tools_answer_through_a_real_session() {
    let host = ToolHost::app();
    let alpha = host.project("alpha");
    let run = host.base.join("work/alpha/.pitwall/run");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join("dev.toml"),
        "schema = 1\nname = \"Dev\"\ntype = \"shell\"\ncommand = \"echo hi\"\n[env]\nAPI_TOKEN = \"tok-9f2e\"\n",
    )
    .unwrap();
    let session = host.session(SessionRequest::default()).await;

    let list = session
        .call("run_config_list", json!({ "projectId": alpha }))
        .await;
    assert_ne!(list.is_error, Some(true), "{list:?}");
    let list = structured(&list);
    assert_eq!(list["configs"][0]["id"], "dev", "{list:?}");
    assert_eq!(
        list["configs"][0]["config"]["env"]["API_TOKEN"],
        "[redacted]"
    );

    let status = session
        .call("run_status", json!({ "projectId": alpha }))
        .await;
    assert_ne!(status.is_error, Some(true), "{status:?}");
    assert_eq!(structured(&status)["runs"], json!([]));

    let logs = session
        .call("run_logs", json!({ "projectId": alpha, "runId": "none-1" }))
        .await;
    assert_eq!(logs.is_error, Some(true), "{logs:?}");
    assert!(text(&logs).contains("none-1"), "{}", text(&logs));
    session.close().await;
}
