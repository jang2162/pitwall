//! DB 조회 툴(16-m5-plan M5-06)의 왕복: 앱과 같은 Host 에 진짜 MCP 세션을 열어([`super::ToolHost`]) 그룹 · 등급 · 결과 표지 ·
//! 감사 행을 본다. 관문의 판정 자체(사람이 연 연결 · 표지 · 신뢰)는 `pw-db` 의 `tests/agent.rs` 가 본다.

use pw_mcp::SessionRequest;

use super::{ToolHost, structured, text};

/// DB 툴(16-m5-plan M5-06)을 진짜 세션으로: 사람이 그 프로젝트 창에서 연 적 없는 연결은 `db_schema` 가 거절하고 결과 표지가
/// `denied` · `not_opened_by_user`(CLI 종료 3)다. 창이 탐색기로 한 번 연 뒤에는 앱의 실 SQLite 드라이버로 테이블 목록을 읽는다.
/// `db_connection_list` 는 그 연결을 쓸 수 있는지를 보인다. 둘 다 앱 전용이라 헤드리스 목록에는 없다(`mcp_tools_are_exactly_these`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_tools_need_a_connection_the_user_opened_in_the_window() {
    let host = ToolHost::app();
    let alpha = host.project("alpha");
    host.host
        .registry()
        .unwrap()
        .set_trust(&alpha, true)
        .unwrap();
    let root = host.base.join("work").join("alpha");
    std::fs::write(root.join("app.db"), "").unwrap();
    std::fs::create_dir_all(root.join(".pitwall/db")).unwrap();
    std::fs::write(
        root.join(".pitwall/db/lite.toml"),
        "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n",
    )
    .unwrap();
    for name in ["db_connection_list", "db_schema"] {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier),
            ("db", pw_rpc::Tier::Read),
            "{name}"
        );
    }
    let session = host.session(SessionRequest::default()).await;
    let args = serde_json::json!({ "projectId": alpha, "connectionId": "lite" });

    let listed = structured(
        &session
            .call(
                "db_connection_list",
                serde_json::json!({ "projectId": alpha }),
            )
            .await,
    );
    assert_eq!(listed["connections"][0]["id"], "lite");
    assert_eq!(listed["connections"][0]["usable"], false);
    assert_eq!(
        listed["connections"][0]["unusableReason"],
        "notOpenedByUser"
    );

    let refused = session.call("db_schema", args.clone()).await;
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(
        refused
            .meta
            .as_ref()
            .and_then(|m| pw_mcp::marker::Mark::read(m)),
        Some(pw_mcp::marker::Mark::denied(
            pw_mcp::marker::Reason::NotOpenedByUser
        )),
        "{refused:?}"
    );
    assert!(
        text(&refused).contains("notOpenedByUser"),
        "{}",
        text(&refused)
    );

    host.window_call(
        &alpha,
        "db.schema.objects",
        serde_json::json!({ "projectId": alpha, "connectionId": "lite", "kind": "table", "limit": 10 }),
    )
    .await
    .unwrap();
    let tables = session.call("db_schema", args).await;
    assert_ne!(tables.is_error, Some(true), "{tables:?}");
    assert_eq!(structured(&tables)["level"], "objects");
    let rows = host.audit_rows(3);
    let outcomes: Vec<_> = rows
        .iter()
        .map(|r| (r.tool.as_str(), r.outcome.as_str()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("db_connection_list", "ok"),
            ("db_schema", "denied"),
            ("db_schema", "ok"),
        ]
    );
    session.close().await;
}
