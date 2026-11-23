//! 에이전트 갈래(18-m7-plan M7-19 — 메인 판정 2026-11-23 Q1): MCP 세션(`Agent` 범위) 연결의 받기 · 맞추기는 대상 Host 가 다시
//! 만든 계획(`git.send.plan`)의 해시를 승인 표가 지날 때만 돈다. 창의 `Project` 범위 길은 그대로다(`wire` 시험). 두 Host 의
//! Router 를 RPC 로 부른다 — 범위는 실제 판정(`scope_may_write` · `agent_grant`)이 가른다.

use pw_core::registry::plan::{APPROVAL_KEY, APPROVAL_REQUIRED};
use pw_rpc::ConnectionScope;
use serde_json::{Value, json};

use super::wire::Conn;
use super::*;

/// 에이전트 보내기의 앞부분: 대상 theirs → 원본 compare → 대상 plan → 원본 prepare(묶음 항목들).
struct Planned {
    hash: String,
    tip: Value,
    items: Vec<Value>,
}

async fn plan(
    pair: &Pair,
    agent: &mut Conn,
    src: &mut Conn,
    branch: &str,
    commit: &str,
) -> Planned {
    let theirs = agent
        .call(
            "git.send.theirs",
            json!({"projectId": pair.dst.project, "branch": branch}),
        )
        .await
        .unwrap();
    let compared = src
        .call(
            "git.send.compare",
            json!({"projectId": pair.src.project, "branch": branch, "theirs": theirs["theirs"], "expected": theirs["branchTip"]}),
        )
        .await
        .unwrap();
    assert_eq!(compared["commit"], commit, "{compared}");
    assert_eq!(compared["fastForward"], true, "{compared}");
    let planned = agent
        .call(
            "git.send.plan",
            json!({"projectId": pair.dst.project, "sourceMachine": MACHINE, "branch": branch, "commit": commit}),
        )
        .await
        .unwrap();
    assert_eq!(planned["branchTip"], theirs["branchTip"]);
    let items = src
        .stream(
            "git.send.prepare",
            json!({"projectId": pair.src.project, "branch": branch, "commit": commit, "theirs": theirs["theirs"]}),
        )
        .await;
    Planned {
        hash: planned["hash"].as_str().unwrap().to_owned(),
        tip: theirs["branchTip"].clone(),
        items,
    }
}

fn ticket(hash: &str) -> Value {
    json!({ "hash": hash })
}

/// 받기 한 번(begin → chunk … → finish). `ticket` 은 begin 의 승인 표.
async fn receive(
    pair: &Pair,
    conn: &mut Conn,
    planned: &Planned,
    branch: &str,
    commit: &str,
    ticket: Option<Value>,
) -> Result<Value, Value> {
    let size = planned.items[0]["size"].as_u64().unwrap();
    let mut params = json!({"projectId": pair.dst.project, "sourceMachine": MACHINE, "branch": branch, "commit": commit, "size": size});
    if let Some(ticket) = ticket {
        params[APPROVAL_KEY] = ticket;
    }
    let begun = conn.call("git.send.receive.begin", params).await?;
    let upload = begun["uploadId"].clone();
    for item in &planned.items[1..] {
        conn.call(
            "git.send.receive.chunk",
            json!({"uploadId": upload, "offset": item["offset"], "data": item["data"]}),
        )
        .await?;
    }
    conn.call("git.send.receive.finish", json!({"uploadId": upload}))
        .await
}

async fn apply(
    pair: &Pair,
    conn: &mut Conn,
    branch: &str,
    commit: &str,
    expected: &Value,
    ticket: Option<Value>,
) -> Result<Value, Value> {
    let mut params = json!({"projectId": pair.dst.project, "sourceMachine": MACHINE, "branch": branch, "commit": commit, "expected": expected});
    if let Some(ticket) = ticket {
        params[APPROVAL_KEY] = ticket;
    }
    conn.call("git.send.apply", params).await
}

fn approval_required(error: &Value, mismatch: bool) {
    assert_eq!(error["code"], APPROVAL_REQUIRED, "{error}");
    assert_eq!(error["data"]["mismatch"], mismatch, "{error}");
}

fn feature(pair: &Pair) -> String {
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat/x"]);
    pair.src.repo.commit_file("x.txt", "x\n", "x")
}

#[tokio::test]
async fn an_approved_agent_send_lands_on_the_target() {
    let pair = Pair::new(true);
    let commit = feature(&pair);
    let mut agent = Conn::open_as(&pair.dst, ConnectionScope::Agent);
    let mut src = Conn::open_as(&pair.src, ConnectionScope::Agent);
    let planned = plan(&pair, &mut agent, &mut src, "feat/x", &commit).await;
    let finished = receive(
        &pair,
        &mut agent,
        &planned,
        "feat/x",
        &commit,
        Some(ticket(&planned.hash)),
    )
    .await
    .unwrap();
    assert_eq!(finished["commit"], commit);
    let applied = apply(
        &pair,
        &mut agent,
        "feat/x",
        &commit,
        &planned.tip,
        Some(ticket(&planned.hash)),
    )
    .await
    .unwrap();
    assert_eq!(applied["outcome"]["kind"], "applied", "{applied}");
    assert_eq!(pair.dst.repo.head(), commit);
}

#[tokio::test]
async fn an_agent_receive_or_apply_without_approval_is_refused() {
    let pair = Pair::new(true);
    let commit = feature(&pair);
    let before = snapshot(&pair.dst.repo);
    let mut agent = Conn::open_as(&pair.dst, ConnectionScope::Agent);
    let mut src = Conn::open_as(&pair.src, ConnectionScope::Agent);
    let planned = plan(&pair, &mut agent, &mut src, "feat/x", &commit).await;
    let refused = receive(&pair, &mut agent, &planned, "feat/x", &commit, None)
        .await
        .unwrap_err();
    approval_required(&refused, false);
    assert_eq!(incoming(&pair.dst.repo, "feat/x"), None, "승인 없이 받았다");

    // 창의 연결로 받아 둔 뒤에도(전용 ref 가 있어도) 에이전트의 승인 없는 맞추기는 거절이다.
    let mut window = Conn::open(&pair.dst);
    receive(&pair, &mut window, &planned, "feat/x", &commit, None)
        .await
        .unwrap();
    let refused = apply(&pair, &mut agent, "feat/x", &commit, &planned.tip, None)
        .await
        .unwrap_err();
    approval_required(&refused, false);
    assert_eq!(snapshot(&pair.dst.repo), before, "승인 없이 맞췄다");
}

#[tokio::test]
async fn an_approval_does_not_carry_to_another_branch_commit_or_a_moved_target() {
    let pair = Pair::new(true);
    let commit = feature(&pair);
    let mut agent = Conn::open_as(&pair.dst, ConnectionScope::Agent);
    let mut src = Conn::open_as(&pair.src, ConnectionScope::Agent);
    let planned = plan(&pair, &mut agent, &mut src, "feat/x", &commit).await;
    let approved = Some(ticket(&planned.hash));
    receive(
        &pair,
        &mut agent,
        &planned,
        "feat/x",
        &commit,
        approved.clone(),
    )
    .await
    .unwrap();
    let before = snapshot(&pair.dst.repo);

    // 같은 표로 다른 브랜치에 받기 · 맞추기.
    let other = receive(
        &pair,
        &mut agent,
        &planned,
        "feat/y",
        &commit,
        approved.clone(),
    )
    .await
    .unwrap_err();
    approval_required(&other, true);
    let other = apply(
        &pair,
        &mut agent,
        "main",
        &commit,
        &planned.tip,
        approved.clone(),
    )
    .await
    .unwrap_err();
    approval_required(&other, true);
    // 같은 표로 다른 원본 머신(전용 ref 의 이름이 바뀐다 — 대상에 남는 것이다, m7-19 리뷰 F1).
    let size = planned.items[0]["size"].as_u64().unwrap();
    let other = agent
        .call(
            "git.send.receive.begin",
            json!({"projectId": pair.dst.project, "sourceMachine": "elsewhere", "branch": "feat/x",
                   "commit": commit, "size": size, APPROVAL_KEY: ticket(&planned.hash)}),
        )
        .await
        .unwrap_err();
    approval_required(&other, true);
    // 같은 표로 다른 커밋(원본에 그 뒤 더한 커밋).
    let later = pair.src.repo.commit_file("y.txt", "y\n", "y");
    let other = apply(
        &pair,
        &mut agent,
        "feat/x",
        &later,
        &planned.tip,
        approved.clone(),
    )
    .await
    .unwrap_err();
    approval_required(&other, true);
    assert_eq!(snapshot(&pair.dst.repo), before);

    // 승인 뒤 대상에 그 브랜치가 생겼다(계획 때는 없었다) — 해시가 덮는 브랜치 끝이 바뀌었다.
    pair.dst.repo.git(&["branch", "feat/x", "main"]);
    let moved = apply(&pair, &mut agent, "feat/x", &commit, &planned.tip, approved)
        .await
        .unwrap_err();
    approval_required(&moved, true);
}

#[tokio::test]
async fn another_scope_with_a_ticket_still_cannot_write() {
    let pair = Pair::new(true);
    let commit = feature(&pair);
    let mut agent = Conn::open_as(&pair.dst, ConnectionScope::Agent);
    let mut src = Conn::open_as(&pair.src, ConnectionScope::Agent);
    let planned = plan(&pair, &mut agent, &mut src, "feat/x", &commit).await;
    for scope in [
        ConnectionScope::Host,
        ConnectionScope::Project("someone-else".into()),
    ] {
        let mut conn = Conn::open_as(&pair.dst, scope.clone());
        let refused = receive(
            &pair,
            &mut conn,
            &planned,
            "feat/x",
            &commit,
            Some(ticket(&planned.hash)),
        )
        .await
        .unwrap_err();
        assert_eq!(
            refused["code"],
            pw_rpc::code::PERMISSION_DENIED,
            "{scope:?}"
        );
        let refused = apply(
            &pair,
            &mut conn,
            "feat/x",
            &commit,
            &planned.tip,
            Some(ticket(&planned.hash)),
        )
        .await
        .unwrap_err();
        assert_eq!(
            refused["code"],
            pw_rpc::code::PERMISSION_DENIED,
            "{scope:?}"
        );
    }
    assert_eq!(incoming(&pair.dst.repo, "feat/x"), None);
}

#[tokio::test]
async fn compare_reads_the_tip_the_count_and_a_divergence() {
    let pair = Pair::new(true);
    let base = pair.dst.repo.head();
    pair.src.repo.commit_file("a.txt", "a\n", "a");
    let tip = pair.src.repo.commit_file("b.txt", "b\n", "b");
    let mut src = Conn::open_as(&pair.src, ConnectionScope::Agent);
    let ahead = src
        .call(
            "git.send.compare",
            json!({"projectId": pair.src.project, "branch": "main", "theirs": [base], "expected": base}),
        )
        .await
        .unwrap();
    assert_eq!(
        ahead,
        json!({"commit": tip, "commits": 2, "fastForward": true, "uncommitted": false})
    );
    // 대상에만 있는 커밋(원본이 모르는 끝) — 갈라짐.
    let theirs_only = pair.dst.repo.commit_file("t.txt", "t\n", "t");
    let diverged = src
        .call(
            "git.send.compare",
            json!({"projectId": pair.src.project, "branch": "main", "theirs": [theirs_only], "expected": theirs_only}),
        )
        .await
        .unwrap();
    assert_eq!(diverged["fastForward"], false);
    assert_eq!(diverged["commits"], 3, "원본에 없는 theirs 는 빼지 못한다");
    let missing = src
        .call(
            "git.send.compare",
            json!({"projectId": pair.src.project, "branch": "nope"}),
        )
        .await
        .unwrap();
    assert_eq!(missing["commit"], Value::Null);
}

/// 관문(`agent_grant`)은 다른 무엇보다 먼저 레지스트리에 없는 프로젝트를 그 까닭으로 거절한다 — 입력 검사 · 저장소 읽기 · 계획보다
/// 앞이다(m7-19 리뷰 F1: 뒤의 `git.open` 도 같은 오류로 거절하므로 순서로 가른다 — 틀린 원본 머신 이름을 함께 준다).
#[tokio::test]
async fn an_unregistered_project_is_refused_by_the_agent_gate_first() {
    let pair = Pair::new(true);
    let commit = feature(&pair);
    let mut agent = Conn::open_as(&pair.dst, ConnectionScope::Agent);
    for (method, extra) in [
        ("git.send.receive.begin", json!({"size": 10})),
        ("git.send.apply", json!({"expected": null})),
    ] {
        let mut params = json!({"projectId": "p-not-registered", "sourceMachine": ".bad",
                                "branch": "feat/x", "commit": commit, APPROVAL_KEY: {"hash": "sha256:x"}});
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        let refused = agent.call(method, params).await.unwrap_err();
        assert_eq!(
            refused["code"],
            pw_rpc::code::INVALID_PARAMS,
            "{method}: {refused}"
        );
        let message = refused["data"].as_str().unwrap_or_default();
        assert!(
            message.contains("p-not-registered") && !message.contains("source machine"),
            "{method}: 관문보다 다른 판정이 먼저 돌았다: {refused}"
        );
    }
}
