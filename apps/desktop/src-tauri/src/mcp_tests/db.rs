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

/// `db_query`(16-m5-plan M5-07)를 진짜 세션으로: 그룹 `db` · 등급 `read`, 앱의 실 SQLite 로 `sql` · `file` 왕복(문장마다 결과
/// 하나), 쓰기가 섞인 글은 `denied`(`permission_denied` — CLI 종료 3). 감사 행의 상세는 문장 수 · 등급 · 해시(`file` 이면 경로와
/// 내용 해시)이고 SQL 원문의 문자열 리터럴은 감사 행 어디에도 없다 — 리터럴을 글 맨 앞(인자 요약의 80자 안)에 둬도 `sql` 은 원문 키라
/// 글자 수만 남는다(리뷰 F1, `pw_mcp::audit::CONTENT_WORDS`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_query_runs_sql_and_files_and_audits_hashes_not_sql() {
    const { assert!(pw_db::script::RESPONSE_BUDGET < pw_mcp::server::MAX_RESPONSE_BYTES) };
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
    let tool = host.tool("db_query");
    assert_eq!((tool.group, tool.tier), ("db", pw_rpc::Tier::Read));
    host.window_call(
        &alpha,
        "db.schema.objects",
        serde_json::json!({ "projectId": alpha, "connectionId": "lite", "kind": "table", "limit": 10 }),
    )
    .await
    .unwrap();
    let session = host.session(SessionRequest::default()).await;
    const LITERAL: &str = "literal-in-the-sql-5be1";
    let sql = format!("SELECT 1 AS pad; SELECT '{LITERAL}' AS v");
    let ran = session
        .call(
            "db_query",
            serde_json::json!({ "projectId": alpha, "connectionId": "lite", "sql": sql }),
        )
        .await;
    assert_ne!(ran.is_error, Some(true), "{ran:?}");
    let out = structured(&ran);
    assert_eq!(out["results"].as_array().unwrap().len(), 2);
    assert_eq!(out["results"][1]["rows"][0][0]["text"], LITERAL);

    std::fs::create_dir_all(root.join("queries")).unwrap();
    std::fs::write(
        root.join("queries/report.sql"),
        "SELECT 1 AS a;\nSELECT 2 AS b;\n",
    )
    .unwrap();
    let from_file = structured(
        &session
            .call(
                "db_query",
                serde_json::json!({ "projectId": alpha, "connectionId": "lite", "file": "queries/report.sql" }),
            )
            .await,
    );
    assert_eq!(from_file["results"].as_array().unwrap().len(), 2);
    assert_eq!(from_file["file"], "queries/report.sql");

    let refused = session
        .call(
            "db_query",
            serde_json::json!({ "projectId": alpha, "connectionId": "lite",
                                "sql": format!("SELECT 1; DELETE FROM t WHERE v = '{LITERAL}'") }),
        )
        .await;
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(
        refused
            .meta
            .as_ref()
            .and_then(|m| pw_mcp::marker::Mark::read(m)),
        Some(pw_mcp::marker::Mark::denied(
            pw_mcp::marker::Reason::PermissionDenied
        )),
    );
    assert!(text(&refused).contains("db_execute"), "{}", text(&refused));

    let rows = host.audit_rows(3);
    let rows: Vec<_> = rows.iter().filter(|r| r.tool == "db_query").collect();
    assert_eq!(
        rows.iter().map(|r| r.outcome.as_str()).collect::<Vec<_>>(),
        ["ok", "ok", "denied"]
    );
    let detail = |i: usize| rows[i].detail.clone().unwrap_or_default();
    assert!(
        detail(0).starts_with("statements=2 grade=read sql=sha256:"),
        "{}",
        detail(0)
    );
    assert!(
        detail(1).contains(" file=queries/report.sql content=sha256:"),
        "{}",
        detail(1)
    );
    assert!(detail(2).contains("statements=2 grade="), "{}", detail(2));
    for row in &rows {
        let all = format!("{} {}", row.args, row.detail.clone().unwrap_or_default());
        assert!(!all.contains(LITERAL), "{all}");
        assert!(
            !row.args.contains("SELECT"),
            "the SQL text is not in the argument summary: {}",
            row.args
        );
    }
    session.close().await;
}

/// Redis 조회 툴(16-m5-plan M5-19)을 진짜 세션으로: 둘 다 그룹 `db` · 등급 `read` 이고 입력에 명령 칸이 없다. 사람이 그 프로젝트
/// 창에서 연 적 없는 Redis 연결은 서버에 붙지 않고 `denied` · `not_opened_by_user` 다(앱의 실 Redis 드라이버 — 서버 없이 관문에서
/// 끝난다). 실 Redis 왕복은 `pw-db` 의 `kv_live.rs`(`live_redis_kv_tool_*`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redis_tools_are_db_reads_without_a_command_input() {
    let host = ToolHost::app();
    let alpha = host.project("alpha");
    host.host
        .registry()
        .unwrap()
        .set_trust(&alpha, true)
        .unwrap();
    let root = host.base.join("work").join("alpha");
    std::fs::create_dir_all(root.join(".pitwall/db")).unwrap();
    std::fs::write(
        root.join(".pitwall/db/cache.toml"),
        "schema = 1\nname = \"cache\"\ndriver = \"redis\"\nhost = \"127.0.0.1\"\nport = 1\nsslmode = \"disable\"\n",
    )
    .unwrap();
    for name in ["db_kv_scan", "db_kv_get"] {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier),
            ("db", pw_rpc::Tier::Read),
            "{name}"
        );
    }
    let session = host.session(SessionRequest::default()).await;
    let listed = session.list().await;
    for name in ["db_kv_scan", "db_kv_get"] {
        let tool = listed.iter().find(|t| t.name == name).unwrap();
        let properties = tool
            .input_schema
            .get("properties")
            .unwrap()
            .as_object()
            .unwrap();
        for command in ["argv", "command", "commands"] {
            assert!(!properties.contains_key(command), "{name}: {command}");
        }
    }
    for (name, extra) in [
        ("db_kv_scan", serde_json::json!({ "match": "user:*" })),
        ("db_kv_get", serde_json::json!({ "key": "user:1" })),
    ] {
        let mut args = serde_json::json!({ "projectId": alpha, "connectionId": "cache" });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let refused = session.call(name, args).await;
        assert_eq!(refused.is_error, Some(true), "{name}");
        assert_eq!(
            refused
                .meta
                .as_ref()
                .and_then(|m| pw_mcp::marker::Mark::read(m)),
            Some(pw_mcp::marker::Mark::denied(
                pw_mcp::marker::Reason::NotOpenedByUser
            )),
            "{name}: {refused:?}"
        );
    }
    let rows = host.audit_rows(2);
    let outcomes: Vec<_> = rows
        .iter()
        .map(|r| (r.tool.as_str(), r.outcome.as_str()))
        .collect();
    assert_eq!(
        outcomes,
        [("db_kv_scan", "denied"), ("db_kv_get", "denied")]
    );
    session.close().await;
}
