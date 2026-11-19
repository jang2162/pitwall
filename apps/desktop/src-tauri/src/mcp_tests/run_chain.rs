//! 사슬(`before` · 복합 구성 — 17-m6-plan M6-21)의 왕복: 사람의 창(`run.start` · `run.stop`)과 에이전트의 `run_start` 를 앱과 같은
//! Host 에서 진짜 PTY · 실 SQLite 로 돌린다. 「띄우지 않았다」 는 `run.list` 에 그 구성의 실행이 없는 것으로 본다.

use std::path::Path;
use std::time::Duration;

use pw_core::bus::{Filter, Received, Scope};
use pw_mcp::SessionRequest;
use pw_mcp::approval::McpApprovalAnswer;
use serde_json::{Value, json};

use super::ToolHost;
use super::run_exec::{
    approvals, enable, hub_answers, last_approval, listen, ok, open_in_window, sleeper,
    sql_project, start, trusted, window_answers, write_config,
};

// --- 도움 ----------------------------------------------------------------------------------------------------------

fn shell(root: &Path, id: &str, command: &str, before: &[&str]) {
    let before = before
        .iter()
        .map(|b| format!("\"{b}\""))
        .collect::<Vec<_>>()
        .join(", ");
    write_config(
        root,
        id,
        &format!(
            "name = \"{id}\"\ntype = \"shell\"\ncommand = \"{command}\"\nbefore = [{before}]\n"
        ),
    );
}

fn compound(root: &Path, id: &str, configs: &[&str], mode: &str) {
    let configs = configs
        .iter()
        .map(|b| format!("\"{b}\""))
        .collect::<Vec<_>>()
        .join(", ");
    write_config(
        root,
        id,
        &format!(
            "name = \"{id}\"\ntype = \"compound\"\nconfigs = [{configs}]\nmode = \"{mode}\"\n"
        ),
    );
}

/// 그 프로젝트의 실행(`run.list`).
async fn runs(host: &ToolHost, project: &str) -> Vec<Value> {
    host.window_call(project, "run.list", json!({ "projectId": project }))
        .await
        .unwrap()["runs"]
        .as_array()
        .unwrap()
        .clone()
}

/// 그 구성의 실행들.
async fn runs_of(host: &ToolHost, project: &str, config: &str) -> Vec<Value> {
    runs(host, project)
        .await
        .into_iter()
        .filter(|r| r["config"] == config)
        .collect()
}

/// 사슬 이벤트를 듣는다(띄우기 전에 붙인다).
fn chain_events(host: &ToolHost, project: &str) -> pw_core::bus::Subscription {
    host.host.bus().subscribe(
        Filter::all()
            .scope(Scope::Project(project.to_owned()))
            .topics([pw_run::runs::chain::TOPIC_CHAIN]),
    )
}

/// 끝난(`running` 이 아닌) 사슬 이벤트를 기다린다.
async fn chain_end(events: &mut pw_core::bus::Subscription) -> Value {
    loop {
        let received = tokio::time::timeout(Duration::from_secs(30), events.recv())
            .await
            .expect("the chain did not end")
            .expect("bus closed");
        if let Received::Event(event) = received
            && event.payload["state"] != "running"
        {
            return event.payload.clone();
        }
    }
}

/// 사람이 창에서 사슬을 띄운다: 첫 실행 확인(7001)이면 그 `hash` 를, `sql` 단계의 위험 확인(3001)이면 그 확인을 실어 다시 부른다
/// (사람이 모두 [실행] 한 것처럼). 띄운 첫 실행과, 받은 확인들(차례대로 `7001` · `3001:<구성>`).
async fn window_chain(host: &ToolHost, project: &str, config: &str) -> (Value, Vec<String>) {
    let mut params = json!({ "projectId": project, "configId": config });
    let mut asked = Vec::new();
    for _ in 0..8 {
        match host.window_call(project, "run.start", params.clone()).await {
            Ok(info) => return (info, asked),
            Err(e) if e["code"] == 7001 => {
                asked.push("7001".to_owned());
                params["confirmed"] = e["data"]["hash"].clone();
            }
            Err(e) if e["code"] == 3001 => {
                let key = e["data"]["config"]
                    .as_str()
                    .expect("3001 names its step")
                    .to_owned();
                asked.push(format!("3001:{key}"));
                let request = &e["data"]["request"];
                params["sqlConfirmed"][&key] = json!({
                    "statementsHash": request["statementsHash"],
                    "grade": request["grade"],
                    "name": e["data"]["connection"]["name"],
                });
            }
            Err(e) => panic!("{e}"),
        }
    }
    panic!("asked too many times: {asked:?}");
}

async fn wait_until<F: std::future::Future<Output = bool>>(
    what: &str,
    mut check: impl FnMut() -> F,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !check().await {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn stop_all(project: &str) {
    pw_run::runs::project_closed(project);
}

// --- 시험 -----------------------------------------------------------------------------------------------------------

/// ① 앞 구성이 실패(끝 코드 3)하면 다음을 띄우지 않는다 — `before` 와 복합 구성의 `sequence` 둘 다. 성공하면(끝 코드 0) 앞 구성이
/// 끝난 뒤에 다음이 뜬다. `parallel` 은 함께 띄운다. 사슬 이벤트가 실패한 단계를 알린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_step_stops_the_chain_and_the_rest_never_starts() {
    let host = ToolHost::app();
    let (alpha, root) = trusted(&host, "alpha");
    shell(&root, "fail", "exit 3", &[]);
    shell(&root, "ok", "true", &[]);
    sleeper(&root, "server", "one");
    shell(&root, "after-fail", "sleep 30", &["fail"]);
    shell(&root, "after-ok", "sleep 30", &["ok"]);
    compound(&root, "seq", &["fail", "server"], "sequence");
    compound(&root, "par", &["ok", "server"], "parallel");

    let mut events = chain_events(&host, &alpha);
    let (first, asked) = window_chain(&host, &alpha, "after-fail").await;
    assert_eq!(first["config"], "fail");
    assert_eq!(asked, ["7001"]);
    let end = chain_end(&mut events).await;
    assert_eq!(end["state"], "failed", "{end}");
    assert_eq!(end["step"], "fail", "{end}");
    assert!(
        end["message"]
            .as_str()
            .unwrap()
            .contains("exited with code 3"),
        "{end}"
    );
    assert!(
        runs_of(&host, &alpha, "after-fail").await.is_empty(),
        "the main config never started"
    );

    window_chain(&host, &alpha, "seq").await;
    let end = chain_end(&mut events).await;
    assert_eq!(
        (end["state"].as_str(), end["step"].as_str()),
        (Some("failed"), Some("fail"))
    );
    assert!(
        runs_of(&host, &alpha, "server").await.is_empty(),
        "a sequence stops at the failure"
    );

    window_chain(&host, &alpha, "after-ok").await;
    let end = chain_end(&mut events).await;
    assert_eq!(end["state"], "done", "{end}");
    let ok_run = &runs_of(&host, &alpha, "ok").await[0];
    let main = &runs_of(&host, &alpha, "after-ok").await[0];
    assert_eq!(ok_run["exit"]["code"], 0);
    assert!(
        main["startedAt"].as_i64().unwrap() >= ok_run["endedAt"].as_i64().unwrap(),
        "the main config starts after `ok` ended: {ok_run} {main}"
    );
    assert_eq!(main["state"], "running");

    window_chain(&host, &alpha, "par").await;
    let end = chain_end(&mut events).await;
    assert_eq!(end["state"], "done");
    assert_eq!(
        end["runs"].as_array().unwrap().len(),
        2,
        "both start together: {end}"
    );
    assert_eq!(runs_of(&host, &alpha, "server").await.len(), 1);
    stop_all(&alpha);
}

/// ② 순환(자기 자신 · 둘 사이 · 복합 구성을 거쳐)은 읽을 때 「쓸 수 없음」 이고 창 · 에이전트 어느 길로도 돌지 않는다(묻지 않는다).
/// 에이전트의 `run_config_upsert` 는 순환 · 없는 구성을 묻기 전에 거절한다(M6-13 에서 넘긴 ⑦) — 사람이 본 적 없는 구성은 가리켜도 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loop_is_unusable_and_an_agent_cannot_write_one() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    shell(&root, "selfish", "true", &["selfish"]);
    shell(&root, "a", "true", &["b"]);
    shell(&root, "b", "true", &["c"]);
    compound(&root, "c", &["a"], "sequence");
    shell(&root, "plain", "true", &[]);

    let list = host
        .window_call(&alpha, "run.config.list", json!({ "projectId": alpha }))
        .await
        .unwrap();
    let unusable: Vec<(String, String)> = list["unusable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["id"].as_str().unwrap().to_owned(),
                u["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    for id in ["selfish", "a", "b", "c"] {
        let reason = &unusable
            .iter()
            .find(|(u, _)| u == id)
            .unwrap_or_else(|| panic!("{id}: {unusable:?}"))
            .1;
        assert!(reason.contains("loops back"), "{id}: {reason}");
    }
    let e = host
        .window_call(
            &alpha,
            "run.start",
            json!({ "projectId": alpha, "configId": "a" }),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("loops back"), "{e}");

    let session = host.session(SessionRequest::default()).await;
    let refused = start(&session, &alpha, "selfish").await;
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(host.services.sessions.approvals().pending(), 0, "not asked");

    let upsert = |id: &str, before: Vec<&str>| json!({ "projectId": alpha, "id": id, "config": { "name": id, "type": "shell", "command": "true", "before": before } });
    for (id, before, why) in [
        ("plain", vec!["plain"], "loops back"),
        ("plain", vec!["missing"], "does not exist"),
        ("d", vec!["a"], "cannot be used"),
    ] {
        let r = session.call("run_config_upsert", upsert(id, before)).await;
        assert_eq!(r.is_error, Some(true), "{id}");
        assert!(super::text(&r).contains(why), "{id}: {}", super::text(&r));
        assert_eq!(
            host.services.sessions.approvals().pending(),
            0,
            "refused before asking"
        );
    }
    // 둘째 구성이 첫째를 가리키는 순환: plain → e 를 쓰고(쓸 수 있다), e → plain 을 쓰면 순환이다.
    let mut window = listen(&host, &alpha);
    let (written, _) = tokio::join!(
        session.call("run_config_upsert", upsert("e", vec!["plain"])),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    ok(&written);
    let r = session
        .call("run_config_upsert", upsert("plain", vec!["e"]))
        .await;
    assert!(
        super::text(&r).contains("loops back"),
        "{}",
        super::text(&r)
    );
    session.close().await;
}

/// ③ `before` 의 `sql` 구성이 위험 확인을 청하면 본 구성도 기다린다 — 창은 첫 실행 확인(모든 단계) 뒤 `3001`(그 단계 열쇠)을 받고,
/// 그동안 아무것도 돌지 않는다. 확인을 실으면 `sql` 단계 → 본 구성 차례로 돈다. 같은 확인을 다시 실으면 다시 묻는다(한 번만 쓰인다).
/// 에이전트의 `run_start` 는 쓰기 `sql` 단계가 들면 계획이 `dangerous` · 매번이고, [거절] 하면 아무것도 돌지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_before_sql_step_that_needs_a_risk_confirmation_holds_the_main_config() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = sql_project(&host, false, "all");
    std::fs::write(root.join("two.sql"), "INSERT INTO t VALUES ('seeded');\n").unwrap();
    sleeper(&root, "server", "one");
    shell(&root, "app", "sleep 30", &["seed"]);
    open_in_window(&host, &alpha).await;

    // 확인을 하나씩: 7001 → 3001 사이에는 아무것도 돌지 않는다.
    let first = host
        .window_call(
            &alpha,
            "run.start",
            json!({ "projectId": alpha, "configId": "app" }),
        )
        .await
        .unwrap_err();
    assert_eq!(first["code"], 7001);
    let steps: Vec<&str> = first["data"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert_eq!(steps, ["seed", "app"], "the confirmation shows every step");
    assert_eq!(first["data"]["order"], "seed, then app");
    let second = host
        .window_call(
            &alpha,
            "run.start",
            json!({ "projectId": alpha, "configId": "app", "confirmed": first["data"]["hash"] }),
        )
        .await
        .unwrap_err();
    assert_eq!(second["code"], 3001, "{second}");
    assert_eq!(second["data"]["config"], "seed");
    assert!(
        runs(&host, &alpha).await.is_empty(),
        "nothing ran while asking"
    );

    let mut events = chain_events(&host, &alpha);
    let (info, asked) = window_chain(&host, &alpha, "app").await;
    // 사슬이 뜨지 않았으니 첫 실행 확인은 기록되지 않았다 — 다시 묻는다.
    assert_eq!(asked, ["7001", "3001:seed"]);
    assert_eq!(info["config"], "seed");
    assert_eq!(chain_end(&mut events).await["state"], "done");
    let count: i64 = rusqlite::Connection::open(root.join("app.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM t WHERE v = 'seeded'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(runs_of(&host, &alpha, "app").await.len(), 1);
    stop_all(&alpha);

    // 쓴 확인을 다시 실으면 다시 묻는다.
    let again = host
        .window_call(
            &alpha,
            "run.start",
            json!({ "projectId": alpha, "configId": "app", "sqlConfirmed": { "seed": {
                "statementsHash": second["data"]["request"]["statementsHash"],
                "grade": second["data"]["request"]["grade"],
            } } }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        again["code"], 3001,
        "a used confirmation does not run again: {again}"
    );

    // 에이전트: 쓰기 `sql` 단계가 든 사슬은 dangerous · 매번 — [거절] 이면 아무것도 돌지 않는다.
    let before = runs(&host, &alpha).await.len();
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let (denied, request) = tokio::join!(
        start(&session, &alpha, "app"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(request.plan.tier, pw_rpc::Tier::Dangerous);
    assert!(!request.plan.rules.policy_auto);
    assert_eq!(
        request.answers,
        [McpApprovalAnswer::Deny, McpApprovalAnswer::Once]
    );
    assert!(
        request
            .plan
            .changes
            .iter()
            .any(|c| matches!(c, pw_core::registry::plan::PlanChange::Sql { .. })),
        "the sql step's statements are in the plan"
    );
    assert_eq!(runs(&host, &alpha).await.len(), before, "nothing ran");
    let (ran, _) = tokio::join!(
        start(&session, &alpha, "app"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    let ran = ok(&ran);
    let chain: Vec<&str> = ran["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["config"].as_str().unwrap())
        .collect();
    assert_eq!(chain, ["seed", "app"]);
    assert_eq!(ran["run"]["config"], "app");
    stop_all(&alpha);
    session.close().await;
}

/// ④ 복합 구성 하나가 본 적 없는 구성을 품으면 `run_start` 가 묻는다 — 사람이 복합 구성을 창에서 돌린 뒤(모든 단계와 복합 구성이
/// 기록된다) 에이전트는 묻지 않고(`policy`), **둘째** 구성의 내용만 바뀌면 다시 묻는다(첫 구성만 보면 실패한다). 사람이 본 구성
/// 사이에 본 적 없는 구성을 끼워도 묻는다. [허용] 하면 모든 단계 · 복합 구성이 기록된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compound_with_one_unseen_config_asks() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "api", "one");
    sleeper(&root, "web", "one");
    sleeper(&root, "extra", "one");
    compound(&root, "all", &["api", "web"], "parallel");

    window_chain(&host, &alpha, "all").await;
    stop_all(&alpha);
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    ok(&start(&session, &alpha, "all").await);
    assert_eq!(last_approval(&host, 1).await.as_deref(), Some("policy"));
    stop_all(&alpha);

    sleeper(&root, "web", "two");
    let (asked, request) = tokio::join!(
        start(&session, &alpha, "all"),
        window_answers(&host, &mut window, &alpha, "once")
    );
    ok(&asked);
    assert!(!request.plan.rules.policy_auto, "the second config changed");
    let seen: Vec<&str> = request.plan.seen.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(seen, ["api", "web", "all"]);
    stop_all(&alpha);
    ok(&start(&session, &alpha, "all").await);
    assert_eq!(
        last_approval(&host, 3).await.as_deref(),
        Some("policy"),
        "the allow recorded every step"
    );
    stop_all(&alpha);

    compound(&root, "all", &["api", "extra", "web"], "parallel");
    let (asked, request) = tokio::join!(
        start(&session, &alpha, "all"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(asked.is_error, Some(true));
    assert!(
        !request.plan.rules.policy_auto,
        "an unseen config between seen ones"
    );
    session.close().await;
}

/// ⑤ 앞 단계가 뒤 단계의 `.env`(프로세스) 또는 SQL 파일(`sql`)을 바꾸면 그 단계 앞에서 `planChanged` 로 멈춘다 — 다시 읽어 돌리지
/// 않는다. 에이전트 · 창 둘 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_step_changed_by_an_earlier_step_stops_with_plan_changed() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = sql_project(&host, false, "all");
    open_in_window(&host, &alpha).await;
    std::fs::write(root.join(".env"), "MODE=one\n").unwrap();
    write_config(
        &root,
        "server",
        "name = \"server\"\ntype = \"shell\"\ncommand = \"sleep 30\"\nenv_file = [\".env\"]\nbefore = [\"touch-env\"]\n",
    );
    shell(&root, "touch-env", "echo MODE=two > .env", &[]);
    shell(
        &root,
        "touch-sql",
        "echo \\\"INSERT INTO t VALUES ('sneaked');\\\" > two.sql",
        &[],
    );
    shell(&root, "migrate", "sleep 30", &["touch-sql", "seed"]);

    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let (changed, _) = tokio::join!(
        start(&session, &alpha, "server"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    assert_eq!(changed.is_error, Some(true));
    assert!(
        super::text(&changed).contains("planChanged"),
        "{}",
        super::text(&changed)
    );
    assert!(runs_of(&host, &alpha, "server").await.is_empty());
    assert_eq!(
        runs_of(&host, &alpha, "touch-env").await[0]["exit"]["code"],
        0
    );

    let (changed, _) = tokio::join!(
        start(&session, &alpha, "migrate"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    assert!(
        super::text(&changed).contains("planChanged"),
        "{}",
        super::text(&changed)
    );
    assert!(
        runs_of(&host, &alpha, "seed").await.is_empty(),
        "the sql step did not run"
    );
    let sneaked: i64 = rusqlite::Connection::open(root.join("app.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM t WHERE v = 'sneaked'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(sneaked, 0);
    session.close().await;

    // 창: 같은 사슬이 확인 때의 `.env` 로만 돈다 — 바뀌면 멈추고 사슬 이벤트가 그 단계를 알린다.
    std::fs::write(root.join(".env"), "MODE=one\n").unwrap();
    let mut events = chain_events(&host, &alpha);
    window_chain(&host, &alpha, "server").await;
    let end = chain_end(&mut events).await;
    assert_eq!(
        (end["state"].as_str(), end["step"].as_str()),
        (Some("failed"), Some("server")),
        "{end}"
    );
    assert!(runs_of(&host, &alpha, "server").await.is_empty());
}

/// ⑥ 사람이 본 구성의 `before` 에 본 적 없는 구성을 더하면 `run_start` 가 묻는다(`before` 가 해시 · 계획에 든다). 사람이 본 사슬을
/// 이룬 `before` 대상의 내용을 바꿔치기해도 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adding_an_unseen_before_to_a_seen_config_asks() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    sleeper(&root, "server", "one");
    shell(&root, "prep", "true", &[]);
    window_chain(&host, &alpha, "server").await;
    stop_all(&alpha);
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    ok(&start(&session, &alpha, "server").await);
    assert_eq!(last_approval(&host, 1).await.as_deref(), Some("policy"));
    stop_all(&alpha);

    shell(&root, "server", "sleep 30 # one", &["prep"]);
    let (asked, request) = tokio::join!(
        start(&session, &alpha, "server"),
        window_answers(&host, &mut window, &alpha, "once")
    );
    ok(&asked);
    assert!(!request.plan.rules.policy_auto);
    let labels: Vec<String> = request
        .plan
        .changes
        .iter()
        .filter_map(|c| match c {
            pw_core::registry::plan::PlanChange::Text { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect();
    assert!(labels.contains(&"Step 1: prep".to_owned()), "{labels:?}");
    assert!(labels.contains(&"Step 2: server".to_owned()), "{labels:?}");
    stop_all(&alpha);
    ok(&start(&session, &alpha, "server").await);
    assert_eq!(last_approval(&host, 3).await.as_deref(), Some("policy"));
    stop_all(&alpha);

    // `before` 대상을 바꿔치기.
    shell(&root, "prep", "echo swapped", &[]);
    let (asked, request) = tokio::join!(
        start(&session, &alpha, "server"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(asked.is_error, Some(true));
    assert!(
        !request.plan.rules.policy_auto,
        "the swapped before step is unseen"
    );
    session.close().await;
}

/// 중지: 사슬이 띄우는 중이면 그 사슬의 실행 하나를 멈춰도 사슬 전체가 멈추고 남은 단계를 띄우지 않는다. 다 띄운 뒤에는 실행마다
/// 따로다(복합 구성의 한 서버만 멈춘다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_a_step_stops_a_chain_that_is_still_starting() {
    let host = ToolHost::app();
    let (alpha, root) = trusted(&host, "alpha");
    shell(&root, "slow", "sleep 30", &[]);
    shell(&root, "server", "sleep 30", &["slow"]);
    sleeper(&root, "api", "one");
    sleeper(&root, "web", "one");
    compound(&root, "all", &["api", "web"], "parallel");

    let mut events = chain_events(&host, &alpha);
    let (slow, _) = window_chain(&host, &alpha, "server").await;
    host.window_call(&alpha, "run.stop", json!({ "runId": slow["runId"] }))
        .await
        .unwrap();
    let end = chain_end(&mut events).await;
    assert_eq!(end["state"], "stopped", "{end}");
    assert!(runs_of(&host, &alpha, "server").await.is_empty());

    window_chain(&host, &alpha, "all").await;
    assert_eq!(chain_end(&mut events).await["state"], "done");
    let api = runs_of(&host, &alpha, "api").await[0]["runId"].clone();
    host.window_call(&alpha, "run.stop", json!({ "runId": api }))
        .await
        .unwrap();
    wait_until("api ended", || async {
        runs_of(&host, &alpha, "api").await[0]["state"] == "exited"
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        runs_of(&host, &alpha, "web").await[0]["state"],
        "running",
        "web keeps running"
    );
    stop_all(&alpha);
}

/// 감사에 명령줄 원문 · env 값이 없다 — 구성 열쇠 · 내용 해시 · 단계 수 · `runId` 만.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_chain_audit_has_no_command_text() {
    let host = ToolHost::app();
    enable(&host, "");
    let (alpha, root) = trusted(&host, "alpha");
    shell(&root, "prep", "echo secretmarker-prep", &[]);
    write_config(
        &root,
        "server",
        "name = \"server\"\ntype = \"shell\"\ncommand = \"sleep 30 # secretmarker-main\"\nenv = { MODE = \"secretmarker-env\" }\nbefore = [\"prep\"]\n",
    );
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let (ran, _) = tokio::join!(
        start(&session, &alpha, "server"),
        hub_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    let ran = ok(&ran);
    approvals(&host, 1).await;
    let row = host
        .audit_rows(1)
        .into_iter()
        .find(|r| r.tool == "run_start")
        .unwrap();
    let all = format!("{} {}", row.args, row.detail.clone().unwrap_or_default());
    assert!(!all.contains("secretmarker"), "{all}");
    assert!(
        all.contains("config=server") && all.contains("steps=2"),
        "{all}"
    );
    assert!(
        ran["audit"].as_str().unwrap().contains("runIds="),
        "{ran:?}"
    );
    stop_all(&alpha);
    session.close().await;
}
