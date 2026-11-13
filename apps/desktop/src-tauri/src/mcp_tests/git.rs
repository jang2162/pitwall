//! git 조회 툴(16-m5-plan M5-05)의 왕복: 앱과 같은 Host 에 진짜 MCP 세션을 열어([`super::ToolHost`]) 이름 · 그룹 · 등급 ·
//! `readOnlyHint` · 스키마 · 감사 행, 워크트리, 응답 상한을 본다. 툴의 동작 자체(쪽 · 필터 · 범위 · 미신뢰)는 `pw-git` 의
//! `tools::tests` 가 본다.

use pw_core::registry::NewProject;
use pw_git::testing::TestRepo;
use pw_mcp::SessionRequest;
use serde_json::{Value, json};

use super::{ToolHost, structured};

/// 다섯 툴과 프로젝트 말고 더 줄 입력.
const GIT_TOOLS: [(&str, &str); 5] = [
    (
        "git_blame",
        r#"{"path": "README.md", "startLine": 1, "endLine": 1}"#,
    ),
    ("git_branch_list", "{}"),
    ("git_diff", "{}"),
    ("git_log", "{}"),
    ("git_status", "{}"),
];

/// `root` 를 프로젝트로 등록한다(`ToolHost::project` 는 빈 폴더를 만든다 — 여기는 시험 저장소다).
fn register(host: &ToolHost, root: &std::path::Path) -> String {
    host.host
        .registry()
        .unwrap()
        .create(NewProject {
            root: root.to_path_buf(),
            ..NewProject::default()
        })
        .unwrap()
        .id
}

fn args(extra: &str, project: &str) -> Value {
    let mut args: Value = serde_json::from_str(extra).unwrap();
    args["projectId"] = json!(project);
    args
}

/// 툴마다 왕복: 그룹 `git` · 등급 `read` · `readOnlyHint` · 스키마의 `worktree` 가 선택 입력, 부르면 결과와 감사 행이 `ok`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_tools_round_trip_through_a_real_session() {
    let host = ToolHost::app();
    let r = TestRepo::with_commit();
    let id = register(&host, r.root());
    let session = host.session(SessionRequest::default()).await;
    let listed = session.list().await;
    for (n, (name, extra)) in GIT_TOOLS.iter().enumerate() {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier),
            ("git", pw_rpc::Tier::Read),
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

/// `worktree` 를 준 `git_status` 는 메인 체크아웃이 아니라 그 워크트리의 상태다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_status_with_a_worktree_reads_that_worktree() {
    let host = ToolHost::app();
    let r = TestRepo::with_commit();
    let wt = r.add_worktree("wt", "feat");
    std::fs::write(wt.join("only-in-wt.txt"), "x\n").unwrap();
    r.write("README.md", "main changed\n");
    let id = register(&host, r.root());
    let session = host.session(SessionRequest::default()).await;
    let paths = |result: &rmcp::model::CallToolResult| -> Vec<String> {
        structured(result)["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_owned())
            .collect()
    };
    let main = session.call("git_status", json!({"projectId": id})).await;
    assert_eq!(paths(&main), ["README.md"]);
    let other = session
        .call(
            "git_status",
            json!({"projectId": id, "worktree": wt.display().to_string()}),
        )
        .await;
    assert_eq!(paths(&other), ["only-in-wt.txt"]);
    assert_eq!(
        structured(&other)["workTree"],
        json!(wt.display().to_string())
    );
    session.close().await;
}

/// 큰 diff 는 세션의 응답 상한(`too_large` 오류)에 닿지 않고 툴이 잘라 `truncated` 로 온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_diff_comes_back_truncated_under_the_response_cap() {
    const { assert!(pw_git::tools::RESPONSE_BUDGET < pw_mcp::server::MAX_RESPONSE_BYTES) };
    let host = ToolHost::app();
    let r = TestRepo::with_commit();
    let body = |tag: &str| -> String {
        (0..400)
            .map(|i| format!("{tag} line {i} with enough words to weigh something\n"))
            .collect()
    };
    for i in 0..40 {
        r.write(&format!("f{i:02}.txt"), &body("old"));
    }
    r.commit_all("files");
    for i in 0..40 {
        r.write(&format!("f{i:02}.txt"), &body("new"));
    }
    let id = register(&host, r.root());
    let session = host.session(SessionRequest::default()).await;
    let result = session
        .call("git_diff", json!({"projectId": id, "detail": true}))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let got = structured(&result);
    assert_eq!(got["truncated"], json!(true));
    assert_eq!(got["files"].as_array().unwrap().len(), 40);
    let rows = host.audit_rows(1);
    assert_eq!(rows[0].outcome, "ok", "{:?}", rows[0]);
    session.close().await;
}
