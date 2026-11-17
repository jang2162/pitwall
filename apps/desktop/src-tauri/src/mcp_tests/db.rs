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

/// 그 프로젝트 창이 승인 요청 하나를 받아 `answer` 로 답한다(창의 `mcp.approval.answer` 와 같은 판정 — 그 프로젝트 창의 범위).
async fn window_answers(
    host: &ToolHost,
    listener: &mut pw_mcp::approval::Listener,
    project: &str,
    answer: pw_mcp::approval::McpApprovalAnswer,
) -> pw_mcp::approval::McpApprovalRequest {
    // 앞 요청의 거둠(`Closed`)은 건너뛴다.
    let request = loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), listener.recv())
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
                scope: &pw_rpc::ConnectionScope::Project(project.to_owned()),
                connection: pw_mcp::approval::Connection::named("window"),
            },
            request.id,
            answer,
        )
        .unwrap();
    *request
}

/// `db_execute`(16-m5-plan M5-18)를 진짜 세션으로: 그룹 `db` · 등급 `dangerous` 라 프리셋 `standard` 의 `tools/list` 에 없고
/// `[mcp.tools] db_execute = true` 면 있다(입력 스키마에 승인 키가 없다). 호출마다 그 프로젝트 창에 묻는다 — `auto_approve` 에
/// `write` · `exec` 가 있어도, 답은 [거절] · [이번만 허용] 둘이고 계획은 SQL 갈래(문장 · 등급 · 연결)다. 앱의 실 SQLite 로 트랜잭션
/// 하나에 커밋 · 실패면 롤백. 감사 행은 승인 방식 `once` · 계획 해시 · 문장 수 · 등급이고 SQL 원문이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_execute_asks_every_time_and_runs_one_transaction() {
    use pw_mcp::approval::{Connection, McpApprovalAnswer};
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
    let tool = host.tool("db_execute");
    assert_eq!((tool.group, tool.tier), ("db", pw_rpc::Tier::Dangerous));

    let session = host.session(SessionRequest::default()).await;
    assert!(
        !session.list().await.iter().any(|t| t.name == "db_execute"),
        "프리셋 standard 에는 없다"
    );
    session.close().await;
    host.global_settings(
        "[mcp]\nauto_approve = [\"write\", \"exec\"]\n[mcp.tools]\ndb_execute = true\n",
    );
    let session = host.session(SessionRequest::default()).await;
    let listed = session.list().await;
    let tool = listed
        .iter()
        .find(|t| t.name == "db_execute")
        .expect("db_execute = true 면 보인다");
    let schema = serde_json::to_value(&tool.input_schema).unwrap();
    assert!(schema["properties"].get("sql").is_some(), "{schema}");
    assert!(
        schema["properties"]
            .get(pw_core::registry::plan::APPROVAL_KEY)
            .is_none(),
        "{schema}"
    );
    assert!(schema.get(pw_core::registry::plan::PLANNED_MARK).is_none());

    host.window_call(
        &alpha,
        "db.schema.objects",
        serde_json::json!({ "projectId": alpha, "connectionId": "lite", "kind": "table", "limit": 10 }),
    )
    .await
    .unwrap();
    let mut window =
        host.services
            .sessions
            .approvals()
            .listen(Some(&alpha), None, Connection::named("window"));
    const LITERAL: &str = "row-literal-77c1";
    let create = serde_json::json!({ "projectId": alpha, "connectionId": "lite",
        "sql": format!("CREATE TABLE t (v text);\nINSERT INTO t VALUES ('{LITERAL}')") });
    let (ran, request) = tokio::join!(
        session.call("db_execute", create),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    assert_ne!(ran.is_error, Some(true), "{ran:?}");
    assert_eq!(structured(&ran)["transaction"], "committed");
    assert_eq!(request.tier, pw_rpc::Tier::Dangerous);
    assert_eq!(
        request.answers,
        [McpApprovalAnswer::Deny, McpApprovalAnswer::Once]
    );
    let change = serde_json::to_value(&request.plan.changes[0]).unwrap();
    assert_eq!(change["kind"], "sql");
    assert_eq!(change["connection"]["name"], "lite");
    assert_eq!(change["statements"][0]["grade"], "ddl");
    assert_eq!(change["statements"][1]["grade"], "write");

    // 둘째 문장이 실패하면 첫 문장도 남지 않는다.
    let failing = serde_json::json!({ "projectId": alpha, "connectionId": "lite",
        "sql": "INSERT INTO t VALUES ('b');\nINSERT INTO missing VALUES (1)" });
    let (rolled, _) = tokio::join!(
        session.call("db_execute", failing),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    let rolled = structured(&rolled);
    assert_eq!(rolled["transaction"], "rolledBack", "{rolled:?}");
    assert_eq!(rolled["error"]["index"], 1);
    let count = structured(
        &session
            .call(
                "db_query",
                serde_json::json!({ "projectId": alpha, "connectionId": "lite", "sql": "SELECT count(*) FROM t" }),
            )
            .await,
    );
    assert_eq!(count["results"][0]["rows"][0][0]["int"], 1, "{count:?}");

    // 거절하면 돌지 않는다.
    let (denied, _) = tokio::join!(
        session.call(
            "db_execute",
            serde_json::json!({ "projectId": alpha, "connectionId": "lite", "sql": "DELETE FROM t WHERE v = 'x'" }),
        ),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(
        denied
            .meta
            .as_ref()
            .and_then(|m| pw_mcp::marker::Mark::read(m)),
        Some(pw_mcp::marker::Mark::denied(
            pw_mcp::marker::Reason::ApprovalDenied
        ))
    );

    let rows = host.audit_rows(4);
    let rows: Vec<_> = rows.iter().filter(|r| r.tool == "db_execute").collect();
    assert_eq!(
        rows.iter().map(|r| r.outcome.as_str()).collect::<Vec<_>>(),
        ["ok", "ok", "denied"]
    );
    let detail = rows[0].detail.clone().unwrap_or_default();
    assert!(
        detail.starts_with("statements=2 grade=ddl sql=sha256:"),
        "{detail}"
    );
    for row in &rows {
        let all = format!("{} {}", row.args, row.detail.clone().unwrap_or_default());
        assert!(!all.contains(LITERAL), "{all}");
        assert!(!row.args.contains("INSERT"), "{}", row.args);
    }
    let approvals: Vec<(Option<String>, bool)> = {
        let conn = rusqlite::Connection::open_with_flags(
            host.services.audit.file(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT approval, plan_hash IS NOT NULL FROM calls WHERE tool = 'db_execute' ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        approvals,
        [
            (Some("once".to_owned()), true),
            (Some("once".to_owned()), true),
            (None, false)
        ]
    );
    session.close().await;
}

/// `db_execute` 의 커밋이 실패하면(16-m5-plan M5-18 리뷰 4) 에이전트가 받는 결과도 감사 행도 오류다 — `committed` 로 나가지 않는다.
/// 실 SQLite: 다른 연결이 읽기 트랜잭션으로 공유 잠금을 쥐고 있으면 쓰기는 되지만 `COMMIT` 이 배타 잠금을 얻지 못해 `busy`
/// (드라이버의 잠금 기다림 5 s 뒤)로 끝난다. 그 쓰기는 남지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_db_execute_whose_commit_fails_is_an_error_in_the_result_and_the_audit_log() {
    use pw_mcp::approval::{Connection, McpApprovalAnswer};
    let host = ToolHost::app();
    let alpha = host.project("alpha");
    host.host
        .registry()
        .unwrap()
        .set_trust(&alpha, true)
        .unwrap();
    let root = host.base.join("work").join("alpha");
    let file = root.join("app.db");
    {
        let setup = rusqlite::Connection::open(&file).unwrap();
        setup
            .execute_batch("CREATE TABLE t (v text); INSERT INTO t VALUES ('a');")
            .unwrap();
    }
    std::fs::create_dir_all(root.join(".pitwall/db")).unwrap();
    std::fs::write(
        root.join(".pitwall/db/lite.toml"),
        "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n",
    )
    .unwrap();
    host.global_settings("[mcp.tools]\ndb_execute = true\n");
    host.window_call(
        &alpha,
        "db.schema.objects",
        serde_json::json!({ "projectId": alpha, "connectionId": "lite", "kind": "table", "limit": 10 }),
    )
    .await
    .unwrap();
    let session = host.session(SessionRequest::default()).await;
    let mut window =
        host.services
            .sessions
            .approvals()
            .listen(Some(&alpha), None, Connection::named("window"));
    // 공유 잠금을 쥔 읽기 트랜잭션(이 시험이 끝날 때까지).
    let reader = rusqlite::Connection::open(&file).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap();
    let (ran, _) = tokio::join!(
        session.call(
            "db_execute",
            serde_json::json!({ "projectId": alpha, "connectionId": "lite", "sql": "INSERT INTO t VALUES ('b')" }),
        ),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    assert_eq!(ran.is_error, Some(true), "{ran:?}");
    assert!(
        !text(&ran).contains("committed"),
        "a failed commit is not reported as committed: {}",
        text(&ran)
    );
    reader.execute_batch("COMMIT").unwrap();
    let count: i64 = reader
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "the write did not stay");
    // 선기록한 행의 끝 기록은 결과가 나간 뒤 큐로 닿는다 — `started` 가 아닐 때까지 기다린다.
    let row = finished_row(&host, 0);
    assert_eq!(row.outcome, "error", "{row:?}");
    assert!(
        row.detail
            .as_deref()
            .unwrap_or_default()
            .contains("statements=1 grade=write"),
        "{row:?}"
    );
    // SQLite 가 COMMIT 을 `busy` 로 거절했다 — 커밋되지 않은 것을 안다(「모름」 이 아니다).
    let detail = row.detail.as_deref().unwrap_or_default();
    assert!(detail.ends_with(" transaction=rolledBack"), "{row:?}");
    assert!(!detail.contains("outcome unknown"), "{row:?}");
    session.close().await;
}

/// `db_execute` 가 커밋하기로 정한 뒤 클라이언트가 MCP 로 취소하면(16-m5-plan M5-18b) DB 에는 커밋되고, 감사 행도 결과 `ok` ·
/// `transaction=committed` 로 그것을 보인다(상세 끝에 「취소됐지만 끝까지 돌았다」). 실 SQLite · 진짜 MCP 세션. 커밋 지점의 끼워
/// 넣기(`DbSessions::test_hooks`)가 Host 에 그 호출의 취소가 닿은 것을 본 뒤에 커밋을 보낸다 — 경쟁이 없다. 커밋 전의 취소는
/// 여전히 롤백 · `cancelled` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_db_execute_cancelled_after_its_commit_point_is_committed_in_the_audit_log() {
    use pw_mcp::approval::{Connection, McpApprovalAnswer};
    use rmcp::model::{CallToolRequest, CallToolRequestParams, ClientRequest};
    use rmcp::service::PeerRequestOptions;
    let host = ToolHost::app();
    let alpha = host.project("alpha");
    host.host
        .registry()
        .unwrap()
        .set_trust(&alpha, true)
        .unwrap();
    let root = host.base.join("work").join("alpha");
    let file = root.join("app.db");
    {
        let setup = rusqlite::Connection::open(&file).unwrap();
        setup.execute_batch("CREATE TABLE t (v text);").unwrap();
    }
    std::fs::create_dir_all(root.join(".pitwall/db")).unwrap();
    std::fs::write(
        root.join(".pitwall/db/lite.toml"),
        "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n",
    )
    .unwrap();
    host.global_settings("[mcp.tools]\ndb_execute = true\n");
    host.window_call(
        &alpha,
        "db.schema.objects",
        serde_json::json!({ "projectId": alpha, "connectionId": "lite", "kind": "table", "limit": 10 }),
    )
    .await
    .unwrap();
    let session = host.session(SessionRequest::default()).await;
    let mut window =
        host.services
            .sessions
            .approvals()
            .listen(Some(&alpha), None, Connection::named("window"));
    let count = || -> i64 {
        rusqlite::Connection::open(&file)
            .unwrap()
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap()
    };
    let send = |sql: &str| {
        let params = CallToolRequestParams::new("db_execute").with_arguments(
            serde_json::json!({ "projectId": alpha, "connectionId": "lite", "sql": sql })
                .as_object()
                .unwrap()
                .clone(),
        );
        let peer = session.client.peer().clone();
        async move {
            peer.send_cancellable_request(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                PeerRequestOptions::no_options(),
            )
            .await
            .unwrap()
        }
    };
    let wait = |rx: tokio::sync::oneshot::Receiver<()>| async move {
        tokio::time::timeout(std::time::Duration::from_secs(10), rx)
            .await
            .expect("the run never reached the hook")
            .unwrap();
    };

    // 커밋 전의 취소: 롤백, 감사 행은 취소.
    let (reached, at_hook) = tokio::sync::oneshot::channel();
    let (go, released) = tokio::sync::oneshot::channel::<()>();
    host.db.test_hooks.set_before_commit(Box::new(move || {
        Box::pin(async move {
            let _ = reached.send(());
            let _ = released.await;
        })
    }));
    let handle = send("INSERT INTO t VALUES ('before')").await;
    window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once).await;
    wait(at_hook).await;
    handle.cancel(Some("test".into())).await.unwrap();
    // 행이 끝난 뒤(취소 응답 — 핸들러가 버려졌다) 태스크를 보낸다.
    let row = finished_row(&host, 0);
    assert_eq!(
        (row.outcome.as_str(), row.detail.as_deref()),
        ("error", Some("cancelled")),
        "{row:?}"
    );
    go.send(()).unwrap();

    // 커밋하기로 정한 뒤의 취소: 커밋, 감사 행은 `ok` · 커밋 표시.
    let (reached, at_hook) = tokio::sync::oneshot::channel();
    host.db
        .test_hooks
        .set_after_commit_point_watching(Box::new(move |cancelled| {
            Box::pin(async move {
                let _ = reached.send(());
                tokio::time::timeout(std::time::Duration::from_secs(10), cancelled.cancelled())
                    .await
                    .expect("the cancel never reached the host");
                // Host 는 취소를 적용한 뒤 그 확인(`null`)을 보낸다 — 커밋의 답이 그 확인을 앞지르지 않게 조금 둔다(앞지르면 세션은
                // 「취소가 닿기 전에 끝났다」 로 본다 — `pw_mcp::client` 머리 주석).
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            })
        }));
    let handle = send("INSERT INTO t VALUES ('after')").await;
    window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once).await;
    wait(at_hook).await;
    handle.cancel(Some("test".into())).await.unwrap();
    let row = finished_row(&host, 1);
    assert_eq!(
        count(),
        1,
        "the write after the commit point stayed; the one before did not"
    );
    let detail = row.detail.clone().unwrap_or_default();
    assert_eq!(row.outcome, "ok", "{row:?}");
    assert!(
        detail.starts_with("statements=1 grade=write sql=sha256:"),
        "{detail}"
    );
    assert!(detail.contains(" transaction=committed"), "{detail}");
    assert!(
        detail.ends_with(&format!(" · {}", pw_mcp::server::CANCELLED_AFTER_THE_POINT)),
        "{detail}"
    );
    let kept: String = rusqlite::Connection::open(&file)
        .unwrap()
        .query_row("SELECT v FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, "after");
    session.close().await;
}

/// `db_execute` 의 `nth` 번째 감사 행 — 선기록한 행이 끝 기록(`started` 가 아닌 결과)을 받을 때까지 기다린다.
fn finished_row(host: &ToolHost, nth: usize) -> super::AuditRow {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let row = host
            .audit_rows(nth + 1)
            .into_iter()
            .filter(|r| r.tool == "db_execute")
            .nth(nth);
        if let Some(row) = &row
            && row.outcome != "started"
        {
            return row.clone();
        }
        assert!(std::time::Instant::now() < deadline, "{row:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
