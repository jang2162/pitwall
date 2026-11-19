//! 실행 툴 ③(17-m6-plan M6-14)의 왕복: `run_start` · `run_stop` · `run_restart` 를 앱과 같은 Host 에 진짜 MCP 세션으로 부르고,
//! 그 프로젝트 창(`mcp.approvals` 허브를 듣는 연결 · `mcp.approval.answer` RPC)이 승인에 답한다. 프로세스 구성은 진짜 PTY 로,
//! `sql` 구성은 앱의 실 SQLite 로 돈다. 「묻지 않았다」 는 감사 행의 승인 방식(`policy` · `session`)과 대기 요청 0 으로 본다.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pw_core::registry::plan::PlanChange;
use pw_mcp::SessionRequest;
use pw_mcp::approval::{
    Connection, Listener, McpApprovalAnswer, McpApprovalEvent, McpApprovalRequest,
};
use pw_rpc::ConnectionScope;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

use super::{ToolHost, ToolSession, structured, text};

// --- 도움 ----------------------------------------------------------------------------------------------------------

/// 실행 툴을 켠다 — `run_start` · `run_restart` 는 선언 `dangerous` 라 기본 프리셋(`standard`)에서 보이지 않는다(`db_execute` 와
/// 같다 — 노출은 선언 등급). `extra` 는 그 뒤에 붙는 설정.
fn enable(host: &ToolHost, extra: &str) {
    host.global_settings(&format!(
        "{extra}\n[mcp.tools]\nrun_start = true\nrun_restart = true\nrun_stop = true\n"
    ));
}

/// 신뢰한 프로젝트 하나와 그 루트.
fn trusted(host: &ToolHost, name: &str) -> (String, PathBuf) {
    let id = host.project(name);
    host.host.registry().unwrap().set_trust(&id, true).unwrap();
    (id, host.base.join("work").join(name))
}

fn write_config(root: &Path, id: &str, body: &str) {
    std::fs::create_dir_all(root.join(".pitwall/run")).unwrap();
    std::fs::write(root.join(format!(".pitwall/run/{id}.toml")), body).unwrap();
}

/// 오래 도는 셸 구성(`sleep`) — 끄는 시험이 쓴다.
fn sleeper(root: &Path, id: &str, marker: &str) {
    write_config(
        root,
        id,
        &format!("name = \"{id}\"\ntype = \"shell\"\ncommand = \"sleep 30 # {marker}\"\n"),
    );
}

fn listen(host: &ToolHost, project: &str) -> Listener {
    host.services
        .sessions
        .approvals()
        .listen(Some(project), None, Connection::named("window"))
}

async fn next_request(listener: &mut Listener) -> McpApprovalRequest {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), listener.recv())
            .await
            .expect("no approval request")
            .unwrap();
        if let McpApprovalEvent::Open { request } = event {
            return *request;
        }
    }
}

/// 허브로 답한다(「사람이 본 구성」 은 기록되지 않는 길 — 기록은 `mcp.approval.answer` RPC 에 있다).
async fn hub_answers(
    host: &ToolHost,
    listener: &mut Listener,
    project: &str,
    answer: McpApprovalAnswer,
) -> McpApprovalRequest {
    let request = next_request(listener).await;
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
    request
}

/// 그 프로젝트 창이 `mcp.approval.answer` RPC 로 답한다(앱의 길 — 허용이면 「사람이 본 구성」 을 기록한다).
async fn window_answers(
    host: &ToolHost,
    listener: &mut Listener,
    project: &str,
    answer: &str,
) -> McpApprovalRequest {
    let request = next_request(listener).await;
    host.window_call(
        project,
        "mcp.approval.answer",
        json!({ "id": request.id, "answer": answer }),
    )
    .await
    .unwrap();
    request
}

/// 사람이 창에서 띄운다(`run.start` — 「사람이 본 구성」 을 기록한다).
async fn window_starts(host: &ToolHost, project: &str, config: &str) -> String {
    let info = host
        .window_call(
            project,
            "run.start",
            json!({ "projectId": project, "configId": config }),
        )
        .await
        .unwrap();
    info["runId"].as_str().unwrap().to_owned()
}

fn ok(result: &CallToolResult) -> serde_json::Map<String, Value> {
    assert_ne!(result.is_error, Some(true), "{}", text(result));
    structured(result)
}

fn mark(result: &CallToolResult) -> Option<pw_mcp::marker::Mark> {
    result
        .meta
        .as_ref()
        .and_then(|m| pw_mcp::marker::Mark::read(m))
}

/// 감사 행의 (툴, 결과, 승인 방식) — `count` 개가 되고 모두 끝 기록을 받을 때까지.
async fn approvals(host: &ToolHost, count: usize) -> Vec<(String, String, Option<String>)> {
    let read = || -> Vec<(String, String, Option<String>)> {
        let Ok(conn) = rusqlite::Connection::open_with_flags(
            host.services.audit.file(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare("SELECT tool, outcome, approval FROM calls ORDER BY id")
        else {
            return Vec::new();
        };
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let rows = read();
        if rows.len() >= count && rows.iter().all(|(_, outcome, _)| outcome != "started") {
            return rows;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "audit rows not finished: {rows:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 마지막 감사 행의 승인 방식.
async fn last_approval(host: &ToolHost, count: usize) -> Option<String> {
    approvals(host, count).await.pop().and_then(|(_, _, a)| a)
}

async fn start(session: &ToolSession, project: &str, config: &str) -> CallToolResult {
    session
        .call(
            "run_start",
            json!({ "projectId": project, "configId": config }),
        )
        .await
}

fn stop_all(project: &str) {
    pw_run::runs::project_closed(project);
}

// --- 시험 -----------------------------------------------------------------------------------------------------------

/// 셋 다 그룹 `run` 의 프로젝트 툴이고 계획을 만든다. `run_start` · `run_restart` 는 `dangerous`(선언 — `sql` 쓰기에 닿는다),
/// `run_stop` 은 `exec`. 앱 전용이라 헤드리스 목록에 없다(`mcp_tools_are_exactly_these`).
#[test]
fn the_run_tools_are_planned_project_tools_of_the_run_group() {
    let host = ToolHost::app();
    for (name, method, tier) in [
        ("run_start", "run.tool.start", pw_rpc::Tier::Dangerous),
        ("run_restart", "run.tool.restart", pw_rpc::Tier::Dangerous),
        ("run_stop", "run.tool.stop", pw_rpc::Tier::Exec),
    ] {
        let tool = host.tool(name);
        assert_eq!((tool.group, tool.tier, tool.method), ("run", tier, method));
        assert!(tool.planned, "{name}");
        assert!(tool.project_scoped(), "{name}");
    }
    assert_eq!(
        host.tool("run_start").lowered,
        [pw_rpc::Tier::Read, pw_rpc::Tier::Exec]
    );
}

/// ① 본 적 없는 구성의 `run_start` 는 묻고, 사람이 화면에서 한 번 실행한 구성은 묻지 않는다(`policy`). ② 파일을 밖에서 고치면
/// 다시 묻는다. 계획에는 (구성 열쇠, 내용 해시)가 실리고 그 해시는 창의 실행이 기록한 것과 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unseen_config_asks_and_one_the_person_ran_does_not() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "dev", "one");
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);

    let (refused, request) = tokio::join!(
        start(&session, &alpha, "dev"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(request.tool, "run_start");
    assert_eq!(
        request.plan.tier,
        pw_rpc::Tier::Exec,
        "a process plan is exec"
    );
    assert!(!request.plan.rules.policy_auto);
    assert_eq!(request.plan.seen.len(), 1);
    assert_eq!(request.plan.seen[0].key, "dev");

    window_starts(&host, &alpha, "dev").await;
    let registry = host.host.registry().unwrap();
    let recorded = registry
        .run_seen(&alpha, "dev")
        .unwrap()
        .expect("the window run is recorded");
    assert_eq!(
        recorded.hash.as_str(),
        request.plan.seen[0].hash,
        "the same content hash"
    );

    let ran = start(&session, &alpha, "dev").await;
    let out = ok(&ran);
    assert_eq!(out["run"]["launchedBy"], "agent");
    assert_eq!(last_approval(&host, 2).await.as_deref(), Some("policy"));
    assert_eq!(host.services.sessions.approvals().pending(), 0);

    // 밖에서 한 글자 고친다.
    sleeper(&root, "dev", "two");
    let (asked, request) = tokio::join!(
        start(&session, &alpha, "dev"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(asked.is_error, Some(true));
    assert_ne!(request.plan.seen[0].hash, recorded.hash.as_str());
    stop_all(&alpha);
    session.close().await;
}

/// 사람이 `mcp.approval.answer` 로 [이번만 허용] 하면 그 내용이 「사람이 본 구성」 으로 기록된다(M6-11 ⑥ — 기록 자리 셋째) — 다음
/// 같은 구성은 묻지 않는다. [거절] 은 기록하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_persons_allow_records_the_content_and_a_deny_does_not() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "dev", "one");
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let registry = host.host.registry().unwrap();

    let (denied, _) = tokio::join!(
        start(&session, &alpha, "dev"),
        window_answers(&host, &mut window, &alpha, "deny")
    );
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(
        registry.run_seen(&alpha, "dev").unwrap(),
        None,
        "a deny records nothing"
    );

    let (allowed, request) = tokio::join!(
        start(&session, &alpha, "dev"),
        window_answers(&host, &mut window, &alpha, "once")
    );
    ok(&allowed);
    let seen = registry.run_seen(&alpha, "dev").unwrap().expect("recorded");
    assert_eq!(seen.hash.as_str(), request.plan.seen[0].hash);

    ok(&start(&session, &alpha, "dev").await);
    assert_eq!(last_approval(&host, 3).await.as_deref(), Some("policy"));
    stop_all(&alpha);
    session.close().await;
}

/// ⑥ 묻는 동안 구성이 바뀌면 [허용] 해도 돌지 않는다(`plan_changed`) — 실행이 생기지 않는다. ⑦ 미신뢰 프로젝트는 묻지 않고
/// 거절한다(대기 요청 0 · 실행 0).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_config_changed_while_asking_does_not_run_and_untrusted_projects_are_refused() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "dev", "one");
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let changed = async {
        let request = next_request(&mut window).await;
        sleeper(&root, "dev", "swapped");
        host.services
            .sessions
            .approvals()
            .answer(
                &pw_mcp::approval::Answerer {
                    scope: &ConnectionScope::Project(alpha.clone()),
                    connection: Connection::named("window"),
                },
                request.id,
                McpApprovalAnswer::Once,
            )
            .unwrap();
    };
    let (result, ()) = tokio::join!(start(&session, &alpha, "dev"), changed);
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        mark(&result),
        Some(pw_mcp::marker::Mark::denied(
            pw_mcp::marker::Reason::PlanChanged
        )),
        "{}",
        text(&result)
    );
    let runs = host
        .window_call(&alpha, "run.list", json!({ "projectId": alpha }))
        .await
        .unwrap();
    assert_eq!(runs["runs"], json!([]), "nothing ran");

    let beta = host.project("beta");
    let beta_root = host.base.join("work/beta");
    sleeper(&beta_root, "dev", "x");
    let refused = start(&session, &beta, "dev").await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        text(&refused).contains("untrusted") || text(&refused).contains("trust"),
        "{}",
        text(&refused)
    );
    assert_eq!(host.services.sessions.approvals().pending(), 0);
    session.close().await;
}

/// ⑧ 구성 A 에 「이 세션 동안 허용」 을 누른 뒤 A 는 묻지 않고, A 의 파일을 밖에서 고치거나 다른 구성 B 를 부르면 다시 묻는다
/// (열쇠 `<구성 열쇠>@<내용 해시>`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_allowance_reaches_only_that_config_content() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "a", "one");
    sleeper(&root, "b", "one");
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);

    let (first, request) = tokio::join!(
        start(&session, &alpha, "a"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Session)
    );
    ok(&first);
    assert!(request.answers.contains(&McpApprovalAnswer::Session));
    ok(&start(&session, &alpha, "a").await);
    assert_eq!(last_approval(&host, 2).await.as_deref(), Some("session"));

    let (other, _) = tokio::join!(
        start(&session, &alpha, "b"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(other.is_error, Some(true), "another config asks");

    sleeper(&root, "a", "two");
    let (changed, _) = tokio::join!(
        start(&session, &alpha, "a"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(changed.is_error, Some(true), "a changed config asks");
    stop_all(&alpha);
    session.close().await;
}

/// ⑧ 의 CLI: 「창이 열려 있는 동안 CLI 허용」 도 그 구성의 그 내용에만 닿는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cli_allowance_reaches_only_that_config_content() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "a", "one");
    sleeper(&root, "b", "one");
    let cli = host.cli_session().await;
    let mut window = listen(&host, &alpha);
    let (first, request) = tokio::join!(
        start(&cli, &alpha, "a"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Window)
    );
    ok(&first);
    assert!(request.cli);
    ok(&start(&cli, &alpha, "a").await);
    assert_eq!(last_approval(&host, 2).await.as_deref(), Some("window"));
    let (other, _) = tokio::join!(
        start(&cli, &alpha, "b"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(other.is_error, Some(true));
    sleeper(&root, "a", "two");
    let (changed, _) = tokio::join!(
        start(&cli, &alpha, "a"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(changed.is_error, Some(true));
    stop_all(&alpha);
    cli.close().await;
}

/// ⑨ 다른 프로젝트의 `runId` 로 부른 `run_stop` · `run_restart` 는 「없는 실행」 이고 묻지 않는다 — 그 실행은 그대로 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_projects_run_is_not_found() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    let (beta, _) = trusted(&host, "beta");
    sleeper(&root, "dev", "one");
    let run = window_starts(&host, &alpha, "dev").await;
    let session = host.session(SessionRequest::default()).await;
    for tool in ["run_stop", "run_restart"] {
        let result = session
            .call(tool, json!({ "projectId": beta, "runId": run }))
            .await;
        assert_eq!(result.is_error, Some(true), "{tool}");
        assert!(
            text(&result).contains(&format!("run `{run}` does not exist")),
            "{tool}: {}",
            text(&result)
        );
    }
    assert_eq!(host.services.sessions.approvals().pending(), 0);
    let runs = host
        .window_call(&alpha, "run.list", json!({ "projectId": alpha }))
        .await
        .unwrap();
    assert_eq!(runs["runs"][0]["state"], "running");
    stop_all(&alpha);
    session.close().await;
}

/// ⑭ 사람이 화면에서 띄운 실행(본 구성)의 `run_stop` · `run_restart` 는 묻는다 — `auto_approve = ["exec"]` 여도, 정책 자동도 없이.
/// 사람이 띄운 실행을 `run_restart` 로 다시 띄운 새 실행도 사람의 실행이라 그 `run_stop` 도 묻는다(`restartOf` — M6-02 리뷰 R6).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_the_person_started_is_always_asked_about() {
    let host = ToolHost::app();
    enable(&host, "[mcp]\nauto_approve = [\"write\", \"exec\"]");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "dev", "one");
    let human = window_starts(&host, &alpha, "dev").await;
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);

    let (stopped, request) = tokio::join!(
        session.call("run_stop", json!({ "projectId": alpha, "runId": human })),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(stopped.is_error, Some(true));
    assert!(!request.plan.rules.policy_auto && !request.plan.rules.auto_approve);
    assert_eq!(
        request.plan.rules.remember,
        pw_core::registry::plan::Remember::Key {
            key: format!("human:{human}")
        }
    );

    let (restarted, request) = tokio::join!(
        session.call("run_restart", json!({ "projectId": alpha, "runId": human })),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    let restarted = ok(&restarted);
    assert!(
        !request.plan.rules.policy_auto,
        "a seen config of a person's run is still asked"
    );
    let new = restarted["run"]["runId"].as_str().unwrap().to_owned();
    assert_eq!(restarted["run"]["restartOf"], json!(human));

    let (stopped, _) = tokio::join!(
        session.call("run_stop", json!({ "projectId": alpha, "runId": new })),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Session)
    );
    ok(&stopped);
    stop_all(&alpha);
    session.close().await;
}

/// ⑮ 에이전트 실행에 준 세션 허용 뒤 같은 구성을 사람이 띄우면 그 실행의 `run_restart` 는 묻는다 — 에이전트 실행의 다시 띄우기는
/// 허용으로 묻지 않는다. 에이전트가 띄운 실행의 `run_stop` 은 신뢰한 프로젝트면 묻지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_allowance_for_agent_runs_does_not_reach_a_persons_run() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "dev", "one");
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);

    let (first, _) = tokio::join!(
        start(&session, &alpha, "dev"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    let agent_run = ok(&first)["run"]["runId"].as_str().unwrap().to_owned();
    let (restarted, _) = tokio::join!(
        session.call(
            "run_restart",
            json!({ "projectId": alpha, "runId": agent_run })
        ),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Session)
    );
    let agent_run = ok(&restarted)["run"]["runId"].as_str().unwrap().to_owned();
    let again = session
        .call(
            "run_restart",
            json!({ "projectId": alpha, "runId": agent_run }),
        )
        .await;
    let agent_run = ok(&again)["run"]["runId"].as_str().unwrap().to_owned();
    assert_eq!(last_approval(&host, 3).await.as_deref(), Some("session"));

    // 사람이 같은 구성을 띄운다(이 실행은 사람의 것).
    let human = window_starts(&host, &alpha, "dev").await;
    let (asked, request) = tokio::join!(
        session.call("run_restart", json!({ "projectId": alpha, "runId": human })),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(asked.is_error, Some(true));
    assert_eq!(request.tool, "run_restart");

    let stopped = session
        .call(
            "run_stop",
            json!({ "projectId": alpha, "runId": agent_run }),
        )
        .await;
    ok(&stopped);
    assert_eq!(last_approval(&host, 5).await.as_deref(), Some("policy"));
    stop_all(&alpha);
    session.close().await;
}

/// ⑯ `.env.local` 에서 온 키의 값이 승인 계획에 출처와 함께 보이고, `API_TOKEN` 과 `DATABASE_URL` 의 비밀번호 자리는 가려진다.
/// `GIT_CONFIG_KEY_<n>` 은 가리지 않는다. 감사 행 어디에도 값 · 명령 원문이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_plan_shows_env_values_with_their_file_and_masks_secrets() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    std::fs::write(
        root.join(".env.local"),
        "PORT=3000\nAPI_TOKEN=tok-secret-4f1c\nDATABASE_URL=postgres://app:pw-secret-77aa@db.local/app\nGIT_CONFIG_KEY_0=core.fsmonitor\n",
    )
    .unwrap();
    write_config(
        &root,
        "dev",
        "name = \"dev\"\ntype = \"shell\"\ncommand = \"sleep 30 # cmd-literal-31c9\"\nenv_file = [\".env.local\"]\n[env]\nNODE_ENV = \"env-value-0d2e\"\n",
    );
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let (_, request) = tokio::join!(
        start(&session, &alpha, "dev"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    let texts: Vec<(String, String)> = request
        .plan
        .changes
        .iter()
        .filter_map(|c| match c {
            PlanChange::Text { label, text } => Some((label.clone(), text.clone())),
            _ => None,
        })
        .collect();
    let env = &texts.iter().find(|(l, _)| l == "Environment").unwrap().1;
    assert!(env.contains("PORT=3000    [from .env.local]"), "{env}");
    assert!(
        env.contains("NODE_ENV=env-value-0d2e    [from the configuration]"),
        "{env}"
    );
    assert!(env.contains("GIT_CONFIG_KEY_0=core.fsmonitor"), "{env}");
    assert!(env.contains("API_TOKEN=•••• (sha256 "), "{env}");
    assert!(env.contains("postgres://app:•••• (sha256 "), "{env}");
    let all = serde_json::to_string(&request.plan).unwrap();
    for secret in ["tok-secret-4f1c", "pw-secret-77aa"] {
        assert!(!all.contains(secret), "`{secret}` is in the plan");
    }
    let run = &texts.iter().find(|(l, _)| l == "Run").unwrap().1;
    assert!(
        run.contains("cmd-literal-31c9"),
        "the command is shown as written: {run}"
    );
    assert!(
        texts
            .iter()
            .any(|(l, t)| l == "Environment files" && t.contains(".env.local  sha256 "))
    );
    for row in host.audit_rows(1) {
        for raw in [
            "tok-secret-4f1c",
            "pw-secret-77aa",
            "cmd-literal-31c9",
            "env-value-0d2e",
            "3000",
        ] {
            let all = format!("{} {:?}", row.args, row.detail);
            assert!(!all.contains(raw), "`{raw}` in the audit row: {all}");
        }
    }
    session.close().await;
}

/// 승인된 계획의 env 값(`.env.local` 에서 읽은 것)이 띄운 프로세스에 닿는다 — 프로세스가 받은 값을 파일로 남겨 본다. 승인과 띄우기
/// 사이에 파일이 바뀌는 틈 자체는 `pw-mcp` 의 `the_launch_uses_the_env_read_when_planning`(M6-10 ⑧)이 갈고리로 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_run_gets_the_approved_env_values() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    std::fs::write(root.join(".env.local"), "PROBE=approved-value\n").unwrap();
    write_config(
        &root,
        "probe",
        "name = \"probe\"\ntype = \"shell\"\ncommand = \"printf %s \\\"$PROBE\\\" > probe.out\"\nenv_file = [\".env.local\"]\n",
    );
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let swap = async {
        let request = next_request(&mut window).await;
        host.services
            .sessions
            .approvals()
            .answer(
                &pw_mcp::approval::Answerer {
                    scope: &ConnectionScope::Project(alpha.clone()),
                    connection: Connection::named("window"),
                },
                request.id,
                McpApprovalAnswer::Once,
            )
            .unwrap();
    };
    let (ran, ()) = tokio::join!(start(&session, &alpha, "probe"), swap);
    ok(&ran);
    // 실행은 띄워졌다 — 지금 파일을 바꿔도 이미 띄운 값이다.
    std::fs::write(root.join(".env.local"), "PROBE=swapped-value\n").unwrap();
    let out = root.join("probe.out");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !out.exists() || std::fs::read_to_string(&out).unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "the run never wrote");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "approved-value");
    session.close().await;
}

// --- sql 구성 ------------------------------------------------------------------------------------------------------

/// SQLite 파일 · 연결 `lite`(`production` 이면 표지) · `sql` 구성 `seed`(파일 `seed.sql`).
fn sql_project(host: &ToolHost, production: bool, transaction: &str) -> (String, PathBuf) {
    let (alpha, root) = trusted(host, "alpha");
    {
        let setup = rusqlite::Connection::open(root.join("app.db")).unwrap();
        setup
            .execute_batch("CREATE TABLE t (v text); INSERT INTO t VALUES ('a');")
            .unwrap();
    }
    std::fs::create_dir_all(root.join(".pitwall/db")).unwrap();
    std::fs::write(
        root.join(".pitwall/db/lite.toml"),
        format!(
            "schema = 1\nname = \"lite\"\ndriver = \"sqlite\"\npath = \"app.db\"\n{}",
            if production {
                "production = true\n"
            } else {
                ""
            }
        ),
    )
    .unwrap();
    write_config(
        &root,
        "seed",
        &format!(
            "name = \"seed\"\ntype = \"sql\"\nconnection = \"lite\"\nfiles = [\"one.sql\", \"two.sql\"]\ntransaction = \"{transaction}\"\n"
        ),
    );
    std::fs::write(root.join("one.sql"), "SELECT count(*) FROM t;\n").unwrap();
    std::fs::write(root.join("two.sql"), "SELECT v FROM t;\n").unwrap();
    (alpha, root)
}

async fn open_in_window(host: &ToolHost, project: &str) {
    host.window_call(
        project,
        "db.schema.objects",
        json!({ "projectId": project, "connectionId": "lite", "kind": "table", "limit": 10 }),
    )
    .await
    .unwrap();
}

/// ③ 조회만인 `sql` 구성은 묻지 않고(`policy`) 읽기 전용 트랜잭션으로 돈다(⑬ — 끝이 `readOnly`), 결과에 요약과 `runId` 가 함께
/// 있다. 파일에 `UPDATE` 를 넣으면 매번 묻고(계획 `dangerous`) [이 세션 동안 허용] 이 없다 — [이번만 허용] 뒤에도 다시 묻는다.
/// ⑩ `per_file` 계획은 「실패해도 앞 파일은 남는다」 표지를 싣는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_query_only_sql_config_runs_without_asking_and_a_write_asks_every_time() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = sql_project(&host, false, "per_file");
    open_in_window(&host, &alpha).await;
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);

    let ran = ok(&start(&session, &alpha, "seed").await);
    assert_eq!(ran["run"]["kind"], "sql");
    assert_eq!(ran["sql"]["statements"], 2);
    assert!(
        ran["sql"]["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["end"] == "readOnly"),
        "{ran:?}"
    );
    assert_eq!(last_approval(&host, 1).await.as_deref(), Some("policy"));

    std::fs::write(root.join("two.sql"), "UPDATE t SET v = 'b';\n").unwrap();
    for n in 0..2 {
        let (wrote, request) = tokio::join!(
            start(&session, &alpha, "seed"),
            hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
        );
        let wrote = ok(&wrote);
        assert_eq!(request.plan.tier, pw_rpc::Tier::Dangerous, "#{n}");
        assert_eq!(
            request.answers,
            [McpApprovalAnswer::Deny, McpApprovalAnswer::Once],
            "#{n}"
        );
        match &request.plan.changes[..] {
            [
                PlanChange::Sql {
                    keeps_on_failure,
                    files,
                    ..
                },
            ] => {
                assert!(
                    *keeps_on_failure,
                    "per_file keeps the files before a failure"
                );
                assert_eq!(files.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        assert!(
            wrote["audit"].as_str().unwrap().contains("file=two.sql"),
            "{wrote:?}"
        );
    }
    let v: String = rusqlite::Connection::open(root.join("app.db"))
        .unwrap()
        .query_row("SELECT v FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, "b");
    session.close().await;
}

/// ⑤ 사람이 연 적 없는 연결은 「Pitwall 에서 먼저 열어라」 거절(`notOpenedByUser` — `needs_app` 도 묻기도 아니다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unopened_connection_is_refused_without_asking() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, _) = sql_project(&host, false, "all");
    let session = host.session(SessionRequest::default()).await;
    let unopened = start(&session, &alpha, "seed").await;
    assert_eq!(
        mark(&unopened),
        Some(pw_mcp::marker::Mark::denied(
            pw_mcp::marker::Reason::NotOpenedByUser
        )),
        "{}",
        text(&unopened)
    );
    assert!(
        text(&unopened).contains("notOpenedByUser"),
        "{}",
        text(&unopened)
    );
    assert_eq!(host.services.sessions.approvals().pending(), 0);
    session.close().await;
}

/// ④ 프로덕션 연결의 `sql` 구성은 막는다(기본 `block`) — 묻지 않는다. ⑫ `production = "ask"` 면 조회만이어도 매번 묻는다(정책 자동
/// 없음 · 세션 허용 없음).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_production_connection_is_blocked_or_asked_every_time() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, _) = sql_project(&host, true, "all");
    open_in_window(&host, &alpha).await;
    let session = host.session(SessionRequest::default()).await;
    let blocked = start(&session, &alpha, "seed").await;
    assert_eq!(
        mark(&blocked),
        Some(pw_mcp::marker::Mark::denied(
            pw_mcp::marker::Reason::ProductionBlocked
        )),
        "{}",
        text(&blocked)
    );
    assert_eq!(host.services.sessions.approvals().pending(), 0);
    session.close().await;

    enable(&host, "[mcp.policy]\nproduction = \"ask\"");
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    for _ in 0..2 {
        let (asked, request) = tokio::join!(
            start(&session, &alpha, "seed"),
            hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
        );
        ok(&asked);
        assert_eq!(request.plan.tier, pw_rpc::Tier::Read);
        assert!(!request.plan.rules.policy_auto);
        assert_eq!(
            request.answers,
            [McpApprovalAnswer::Deny, McpApprovalAnswer::Once]
        );
    }
    session.close().await;
}

/// ⑪ MCP 취소가 커밋 뒤에 닿으면 감사 상세에 `CANCELLED_AFTER_THE_POINT` 가 붙고 결과는 `ok`, 커밋은 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_after_the_commit_point_is_recorded_as_committed() {
    use rmcp::model::{CallToolRequest, CallToolRequestParams, ClientRequest};
    use rmcp::service::PeerRequestOptions;
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = sql_project(&host, false, "all");
    std::fs::write(root.join("two.sql"), "INSERT INTO t VALUES ('after');\n").unwrap();
    open_in_window(&host, &alpha).await;
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    // `sql` 실행은 호출 밖의 태스크에서 돈다 — 커밋을 정한 자리에서 멈춰 두고, 취소가 Host 에 닿은 뒤 놓아 준다.
    let (reached, at_hook) = tokio::sync::oneshot::channel();
    let (go, released) = tokio::sync::oneshot::channel::<()>();
    host.db.test_hooks.set_after_commit_point(Box::new(move || {
        Box::pin(async move {
            let _ = reached.send(());
            let _ = released.await;
        })
    }));
    let params = CallToolRequestParams::new("run_start").with_arguments(
        json!({ "projectId": alpha, "configId": "seed" })
            .as_object()
            .unwrap()
            .clone(),
    );
    let handle = session
        .client
        .peer()
        .clone()
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once).await;
    tokio::time::timeout(Duration::from_secs(10), at_hook)
        .await
        .expect("the run never reached the commit")
        .unwrap();
    handle.cancel(Some("test".into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    go.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let row = loop {
        let row = host
            .audit_rows(1)
            .into_iter()
            .find(|r| r.tool == "run_start" && r.outcome != "started");
        if let Some(row) = row {
            break row;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let detail = row.detail.clone().unwrap_or_default();
    assert_eq!(row.outcome, "ok", "{row:?}");
    assert!(detail.contains("transaction=committed"), "{detail}");
    assert!(
        detail.ends_with(&format!(" · {}", pw_mcp::server::CANCELLED_AFTER_THE_POINT)),
        "{detail}"
    );
    let count: i64 = rusqlite::Connection::open(root.join("app.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2, "the commit stayed");
    session.close().await;
}

// --- CLI ----------------------------------------------------------------------------------------------------------

/// CLI: `pitwall tool run_start --config-id dev` 는 사람이 본 구성이면 0, 창이 거절하면 3.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cli_exits_0_for_a_seen_config_and_3_when_denied() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "dev", "one");
    sleeper(&root, "other", "one");
    window_starts(&host, &alpha, "dev").await;
    let (code, out, err) = host
        .cli(&["run_start", "--project-id", &alpha, "--config-id", "dev"])
        .await;
    assert_eq!(code, crate::tool_cli::exit::OK, "{err}");
    let document: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(document["run"]["config"], "dev");

    let mut window = listen(&host, &alpha);
    let args = [
        "run_start",
        "--project-id",
        alpha.as_str(),
        "--config-id",
        "other",
    ];
    let ((code, out, err), _) = tokio::join!(
        host.cli(&args),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(
        (code, out.as_str()),
        (crate::tool_cli::exit::DENIED, ""),
        "{err}"
    );
    stop_all(&alpha);
}
