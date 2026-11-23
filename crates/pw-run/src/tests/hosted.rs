//! Host 수명 터미널(18-m7-plan M7-09 ② — 완료 기준 ③ ~ ⑦ 와 신뢰 해제). 실제 PTY 와 `/bin/sh`.

use super::*;

/// 붙을 때까지 기다린다(`attached` 항목). 스트림 번호와 그 본문.
async fn attach(c: &mut Client, terminal: TerminalId, since: Option<u64>) -> (u64, Value) {
    let mut params = json!({ "terminalId": terminal });
    if let Some(since) = since {
        params["since"] = json!(since);
    }
    let response = c.call("terminal.attach", params).await;
    let opened: StreamOpened = response.result_as().unwrap();
    c.credit(opened.stream_id, 1_000_000);
    loop {
        if let Some(i) = c.others.iter().position(|(s, _)| *s == opened.stream_id) {
            let attached = c.others.remove(i).1;
            assert_eq!(attached["kind"], "attached", "{attached}");
            return (opened.stream_id, attached);
        }
        if let Some(end) = c.ended.iter().find(|e| e.stream_id == opened.stream_id) {
            panic!("attach ended: {end:?}");
        }
        c.receive(Duration::from_secs(10)).await;
    }
}

/// 스트림이 오류로 끝난 꼴(붙기 · 띄우기가 거절됨).
async fn refused(c: &mut Client, method: &str, params: Value) -> (i32, Value) {
    let response = c.call(method, params).await;
    match response.outcome {
        Err(e) => (e.code, e.data.unwrap_or(Value::Null)),
        Ok(value) => {
            let opened: StreamOpened =
                serde_json::from_value(value).expect("neither an error nor a stream");
            let (_, end) = c.end(opened.stream_id).await;
            match end.outcome {
                StreamOutcome::Failed { error } => (error.code, error.data.unwrap_or(Value::Null)),
                other => panic!("{method} was not refused: {other:?}"),
            }
        }
    }
}

async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 다른 프로젝트 하나를 더 만든다(신뢰한다).
fn other_project(f: &Fixture) -> String {
    let id = untrusted_project(f, "other");
    f.projects.set_trust(&id, true).unwrap();
    id
}

/// 신뢰하지 않은 프로젝트 하나를 더 만든다.
fn untrusted_project(f: &Fixture, name: &str) -> String {
    let root = f.tmp.path().join(name);
    fs::create_dir_all(&root).unwrap();
    f.projects
        .create(NewProject {
            root,
            ..NewProject::default()
        })
        .unwrap()
        .id
}

/// 완료 기준 ③: 원격 Host 의 터미널은 연결이 닫혀도 셸이 살고 다른 연결이 다시 붙어 입력 · 출력이 이어진다. 로컬 Host 의 터미널은
/// 연결이 닫히면 끝난다 — 로컬에도 떼기를 적용하면(`Terminals::new` 가 Host 수명이면) 뒤 절반이 실패한다.
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_terminal_outlives_its_connection_and_a_local_one_does_not() {
    let f = Fixture::hosted(DETACHED_LIMIT);
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    c.input(t.terminal, "echo BEFORE-$((40+2))\n").await;
    c.read_until(t.stream, "BEFORE-42").await;
    c.session.close();
    drop(c);
    until("the terminal was not detached", || {
        f.terminals.detached_count() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(alive(t.pid), "the remote shell died with its connection");
    assert_eq!(f.terminals.count(), 1);

    let mut d = f.client();
    let (stream, attached) = attach(&mut d, t.terminal, None).await;
    assert_eq!(attached["dropped"], 0);
    assert_eq!(attached["terminal"]["attached"], true);
    d.input(t.terminal, "echo AFTER-$((40+3))\n").await;
    let text = d.read_until(stream, "AFTER-43").await;
    assert!(
        text.contains("BEFORE-42"),
        "the output before the detach was lost: {text:?}"
    );
    assert_eq!(f.terminals.detached_count(), 0);

    // 로컬 Host: 같은 일에 셸이 끝난다.
    let local = Fixture::new("schema = 1\n");
    let mut c = local.client();
    let t = c.open(local.create_params(80, 24)).await;
    c.session.close();
    drop(c);
    assert_gone_within_a_second(&[t.pid], Instant::now()).await;
    until("the local terminal stayed", || local.terminals.count() == 0).await;
    assert_eq!(local.terminals.detached_count(), 0);
    // 로컬 Host 는 떼지 않고 목록도 비었다.
    let mut c = local.client();
    let t = c.open(local.create_params(80, 24)).await;
    let (code_, _) = refused(&mut c, "terminal.detach", json!({"terminalId": t.terminal})).await;
    assert_eq!(code_, code::INVALID_PARAMS);
    let listed = c
        .call("terminal.list", json!({"projectId": local.project}))
        .await;
    assert_eq!(listed.outcome.unwrap()["terminals"], json!([]));
}

/// 완료 기준 ④: `terminal.attach { since }` 는 링에서 버려진 앞을 잃은 바이트(`dropped`)로 알리고, 남은 범위의 `since` 는 잃은 것
/// 없이 그 오프셋부터 준다.
#[tokio::test(flavor = "multi_thread")]
async fn attach_since_counts_the_bytes_the_ring_dropped() {
    let f = Fixture::hosted(DETACHED_LIMIT);
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    let detach = c
        .call("terminal.detach", json!({"terminalId": t.terminal}))
        .await;
    assert_eq!(detach.outcome.unwrap(), json!({"ended": []}));
    let (events, _) = c.end(t.stream).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, TerminalEvent::Exited { .. })),
        "the detach ended the shell"
    );
    // 떼어진 동안 링(512 KiB)보다 많이 낸다.
    let total = TERMINAL_LOG + 300 * 1024;
    c.input(
        t.terminal,
        &format!("head -c {total} /dev/zero | tr '\\0' a; echo; echo DONE-$((1+1))\n"),
    )
    .await;
    let hosted = f
        .terminals
        .hosting
        .find(&ConnectionScope::Project(f.project.clone()), t.terminal)
        .unwrap();
    until("the output did not arrive", || {
        hosted.first_from(u64::MAX) > total as u64 + 8
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let end = hosted.first_from(u64::MAX);
    let start = hosted.first_from(0);
    assert!(start > 0, "the ring kept everything");
    assert_eq!(end - start, TERMINAL_LOG as u64);

    // 버려진 앞부터: 잃은 바이트가 버려진 만큼이다.
    let mut d = f.client();
    let (stream, attached) = attach(&mut d, t.terminal, Some(1000)).await;
    assert_eq!(attached["from"], start);
    assert_eq!(attached["dropped"], start - 1000);
    let text = d.read_until(stream, "DONE-2").await;
    assert!(text.len() as u64 >= end - start - 64);
    d.session.close();

    // 남은 범위 안이면 잃은 것이 없고 그 오프셋부터다.
    let since = end - 100;
    let mut e = f.client();
    let (stream, attached) = attach(&mut e, t.terminal, Some(since)).await;
    assert_eq!(attached["from"], since);
    assert_eq!(attached["dropped"], 0);
    let first = e.next_event(stream).await;
    let TerminalEvent::Output { offset, .. } = first else {
        panic!("{first:?}")
    };
    assert_eq!(offset, Some(since));
}

/// 완료 기준 ⑤: 다른 프로젝트 창이 대는 `terminalId` 는 없는 터미널이다(실행과 같은 판정) — 붙기 · 입력 · 크기 · 떼기 모두. 목록도
/// 그 프로젝트의 창만.
#[tokio::test(flavor = "multi_thread")]
async fn another_projects_window_sees_no_such_terminal() {
    let f = Fixture::hosted(DETACHED_LIMIT);
    let other = other_project(&f);
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    let mut o = f.client_in(ConnectionScope::Project(other.clone()));
    let missing = 9999;
    for terminal in [t.terminal, missing] {
        let (code_, _) = refused(&mut o, "terminal.attach", json!({"terminalId": terminal})).await;
        assert_eq!(code_, code::INVALID_PARAMS, "attach {terminal}");
        let input = o
            .call(
                "terminal.input",
                json!({"terminalId": terminal, "data": "x"}),
            )
            .await;
        assert_eq!(input.outcome.unwrap_err().code, code::INVALID_PARAMS);
        let resize = o
            .call(
                "terminal.resize",
                json!({"terminalId": terminal, "cols": 10, "rows": 10}),
            )
            .await;
        assert_eq!(resize.outcome.unwrap_err().code, code::INVALID_PARAMS);
        let (code_, _) = refused(&mut o, "terminal.detach", json!({"terminalId": terminal})).await;
        assert_eq!(code_, code::INVALID_PARAMS);
        // 닫기는 없는 터미널처럼 아무것도 하지 않는다.
        o.call("terminal.close", json!({"terminalId": terminal}))
            .await
            .outcome
            .unwrap();
    }
    assert!(alive(t.pid));
    let listed = o.call("terminal.list", json!({"projectId": other})).await;
    assert_eq!(listed.outcome.unwrap()["terminals"], json!([]));
    let (code_, _) = refused(&mut o, "terminal.list", json!({"projectId": f.project})).await;
    assert_eq!(code_, code::PERMISSION_DENIED);
    // 그 프로젝트 창에서는 보인다.
    let listed = c
        .call("terminal.list", json!({"projectId": f.project}))
        .await;
    let listed = listed.outcome.unwrap();
    assert_eq!(listed["terminals"][0]["terminalId"], t.terminal);
    assert_eq!(listed["terminals"][0]["attached"], true);
}

/// 완료 기준 ⑥: 떼어진 터미널은 머신당 상한까지 — 연결이 닫혀 넘으면 가장 오래 떼어진 것부터 끝내고, `terminal.detach` 는 넘을 때
/// 끝낼 번호를 conflict 로 알린 뒤 `endOldest` 에만 끝낸다.
#[tokio::test(flavor = "multi_thread")]
async fn detached_terminals_are_capped_per_machine() {
    let f = Fixture::hosted(2);
    let mut opened = Vec::new();
    for i in 0..3_usize {
        let mut c = f.client();
        let t = c.open(f.create_params(80, 24)).await;
        c.session.close();
        drop(c);
        let kept = (i + 1).min(2);
        until("the terminal was not detached", || {
            f.terminals.detached_count() == kept && f.terminals.count() == kept
        })
        .await;
        opened.push(t);
    }
    // 셋째가 떼어지며 첫째(가장 오래)가 끝났다.
    assert_gone_within_a_second(&[opened[0].pid], Instant::now()).await;
    assert!(alive(opened[1].pid) && alive(opened[2].pid));
    assert_eq!(f.terminals.detached_count(), 2);

    // 붙어 있는 넷째를 떼면 상한을 넘는다 — 묻는다.
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    let (code_, data) = refused(&mut c, "terminal.detach", json!({"terminalId": t.terminal})).await;
    assert_eq!(code_, code::CONFLICT);
    assert_eq!(data["wouldEnd"], json!([opened[1].terminal]));
    assert!(alive(opened[1].pid), "a refused detach ended a terminal");
    let detached = c
        .call(
            "terminal.detach",
            json!({"terminalId": t.terminal, "endOldest": true}),
        )
        .await;
    assert_eq!(
        detached.outcome.unwrap(),
        json!({"ended": [opened[1].terminal]})
    );
    assert_gone_within_a_second(&[opened[1].pid], Instant::now()).await;
    assert!(alive(t.pid) && alive(opened[2].pid));
    assert_eq!(f.terminals.detached_count(), 2);
}

/// 완료 기준 ⑦: 떼어진 터미널의 붙기 · 목록 · 입력은 그 프로젝트의 창(`Project` 범위) · owner 만 — `Agent` · `Host` 범위와 `readonly`
/// · `mobile` role 은 `PERMISSION_DENIED`.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_projects_owner_window_may_attach() {
    let f = Fixture::hosted(DETACHED_LIMIT);
    let mut c = f.client();
    let t = c.open(f.create_params(80, 24)).await;
    c.session.close();
    drop(c);
    until("the terminal was not detached", || {
        f.terminals.detached_count() == 1
    })
    .await;
    let attach_params = json!({"terminalId": t.terminal, "since": 0});
    let input_params = json!({"terminalId": t.terminal, "data": "touch PWNED\n"});
    let list_params = json!({"projectId": f.project});
    let project = ConnectionScope::Project(f.project.clone());
    for (role, scope) in [
        (Role::Owner, ConnectionScope::Agent),
        (Role::Owner, ConnectionScope::Host),
        (Role::Readonly, project.clone()),
        (Role::Mobile, project.clone()),
    ] {
        let mut x = f.client_as(role, scope.clone());
        let (code_, _) = refused(&mut x, "terminal.attach", attach_params.clone()).await;
        assert_eq!(
            code_,
            code::PERMISSION_DENIED,
            "attach as {role:?} {scope:?}"
        );
        let (code_, _) = refused(&mut x, "terminal.list", list_params.clone()).await;
        assert_eq!(code_, code::PERMISSION_DENIED, "list as {role:?} {scope:?}");
        let (code_, _) = refused(&mut x, "terminal.input", input_params.clone()).await;
        assert_eq!(
            code_,
            code::PERMISSION_DENIED,
            "input as {role:?} {scope:?}"
        );
        // 닫기도 거절한다(없는 터미널처럼 조용히 넘기지 않는다 — 리뷰 권고 3).
        let (code_, _) = refused(&mut x, "terminal.close", json!({"terminalId": t.terminal})).await;
        assert_eq!(
            code_,
            code::PERMISSION_DENIED,
            "close as {role:?} {scope:?}"
        );
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!f.root.join("PWNED").exists());
    assert_eq!(f.terminals.detached_count(), 1);
}

/// 신뢰를 거둔 프로젝트의 원격 터미널은 붙은 것 · 떼어진 것 모두 끝난다(리뷰 2.2 — 붙어 있던 것이 나중에 떼어져 미신뢰 프로젝트의
/// 셸로 남지 않게). 붙은 스트림은 남은 출력과 `exited` 를 받는다. 다른 프로젝트의 것은 그대로다.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_trust_ends_the_projects_terminals_attached_or_not() {
    let f = Fixture::hosted(DETACHED_LIMIT);
    let other = other_project(&f);
    let follow = tokio::spawn(
        Arc::clone(&f.terminals)
            .follow_trust(Terminals::subscribe_trust(&f.bus), f.projects.clone()),
    );
    let mut c = f.client();
    let detached = c.open(f.create_params(80, 24)).await;
    c.session.close();
    drop(c);
    let mut c = f.client();
    let attached = c.open(f.create_params(80, 24)).await;
    let mut o = f.client_in(ConnectionScope::Project(other.clone()));
    let elsewhere = o
        .open(json!({"projectId": other, "cols": 80, "rows": 24}))
        .await;
    o.session.close();
    drop(o);
    until("the terminals were not detached", || {
        f.terminals.detached_count() == 2
    })
    .await;
    f.projects.set_trust(&f.project, false).unwrap();
    assert_gone_within_a_second(&[detached.pid, attached.pid], Instant::now()).await;
    let (_, end) = c.exit(attached.stream).await;
    assert_eq!(end.outcome, StreamOutcome::Completed);
    assert!(alive(elsewhere.pid), "another project's terminal ended");
    assert_eq!(f.terminals.detached_count(), 1);
    assert_eq!(f.terminals.count(), 1);
    follow.abort();
}

/// 미신뢰 프로젝트의 원격 터미널은 연결 수명이다(메인 판정 2026-11-23): 띄우기 · 입력 · 크기는 로컬과 같이 받고, 떼기 · 다시
/// 붙기는 `PERMISSION_DENIED`(`untrusted`), 연결이 닫히면 셸이 끝난다.
#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_projects_remote_terminal_lives_with_its_connection() {
    let f = Fixture::hosted(DETACHED_LIMIT);
    let project = untrusted_project(&f, "cloned");
    let scope = ConnectionScope::Project(project.clone());
    let params = json!({"projectId": project, "cols": 80, "rows": 24});
    let mut c = f.client_in(scope.clone());
    let t = c.open(params.clone()).await;
    c.input(t.terminal, "echo HI-$((2+3))\n").await;
    c.read_until(t.stream, "HI-5").await;
    c.call(
        "terminal.resize",
        json!({"terminalId": t.terminal, "cols": 100, "rows": 30}),
    )
    .await
    .outcome
    .unwrap();
    let (code_, data) = refused(&mut c, "terminal.detach", json!({"terminalId": t.terminal})).await;
    assert_eq!(
        (code_, data["reason"].clone()),
        (code::PERMISSION_DENIED, json!("untrusted"))
    );
    let mut d = f.client_in(scope.clone());
    let (code_, data) = refused(&mut d, "terminal.attach", json!({"terminalId": t.terminal})).await;
    assert_eq!(
        (code_, data["reason"].clone()),
        (code::PERMISSION_DENIED, json!("untrusted"))
    );
    assert!(alive(t.pid));
    c.session.close();
    drop(c);
    assert_gone_within_a_second(&[t.pid], Instant::now()).await;
    until("the untrusted terminal stayed", || f.terminals.count() == 0).await;
    assert_eq!(f.terminals.detached_count(), 0);
}
