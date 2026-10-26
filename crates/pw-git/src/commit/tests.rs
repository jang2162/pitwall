//! `git.commit`(13-m2-plan.md M2-08 완료 기준 ① ~ ⑤). 진짜 git · 임시 폴더의 시험 저장소 · 진짜 git 을 PATH 에 둔 가짜 로그인
//! 셸(`crate::testing::write`)로 돈다. 훅은 시험 저장소의 `.git/hooks/` 에 sh 스크립트로 심는다.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{Filter, Received, Subscription};
use pw_core::task::{TOPIC_FINISHED, TOPIC_PROGRESS};
use pw_core::{Host, LocalHost};
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

use super::*;
use crate::testing::TestRepo;
use crate::testing::write::WriteFixture;
use crate::write::code;

/// 시험 저장소에 훅 `name` 을 심는다(본문은 sh).
fn hook(repo: &TestRepo, name: &str, body: &str) {
    let path = repo.root().join(".git/hooks").join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn params(f: &WriteFixture, message: &str) -> GitCommitParams {
    GitCommitParams {
        project_id: f.project.clone(),
        message: message.to_owned(),
        ..GitCommitParams::default()
    }
}

/// 커밋 객체의 메시지 원문.
fn message_of(repo: &TestRepo, rev: &str) -> String {
    let raw = repo.git(&["cat-file", "commit", rev]);
    raw.split_once("\n\n").unwrap().1.to_owned()
}

/// 첫 `task.finished` 까지 받은 이벤트(토픽, 내용). 쓰기 전에 연 구독으로 받는다.
async fn until_finished(sub: &mut Subscription) -> Vec<(String, Value)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    loop {
        while let Some(received) = sub.try_recv() {
            if let Received::Event(event) = received {
                events.push((event.topic.clone(), event.payload.clone()));
            }
        }
        if events.iter().any(|(topic, _)| topic == TOPIC_FINISHED) {
            return events;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no task.finished: {events:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// ① 출력한 뒤 1로 끝나는 pre-commit 훅이면 커밋되지 않고, 그 출력이 결과(git 실패의 stderr)와 Task 의 오류에 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_pre_commit_hook_stops_the_commit_and_its_output_is_in_the_result() {
    let f = WriteFixture::new(true);
    hook(
        &f.repo,
        "pre-commit",
        "echo 'lint: bad.rs:3 unused'\nexit 1",
    );
    f.repo.write("bad.rs", "fn main() {}\n");
    f.repo.git(&["add", "bad.rs"]);
    let before = f.repo.head();
    let mut sub = f.bus.subscribe(Filter::all());

    let result = commit(&f.write, params(&f, "add bad.rs")).await;
    let Err(WriteError::Failed {
        command,
        exit_code,
        stderr,
        ..
    }) = &result
    else {
        panic!("the hook should fail the commit: {result:?}");
    };
    assert_eq!(command, "commit");
    assert_eq!(*exit_code, Some(1));
    assert!(stderr.contains("lint: bad.rs:3 unused"), "{stderr:?}");
    assert_eq!(f.repo.head(), before, "nothing was committed");
    // index 는 그대로 — 사람이 고쳐 다시 누를 수 있다.
    assert_eq!(f.repo.git(&["diff", "--cached", "--name-only"]), "bad.rs\n");

    let events = until_finished(&mut sub).await;
    let finished = events
        .iter()
        .find(|(t, _)| t == TOPIC_FINISHED)
        .map(|(_, p)| p)
        .unwrap();
    assert_eq!(finished["status"], "failed", "{finished}");
    assert!(
        finished["error"]
            .as_str()
            .unwrap()
            .contains("lint: bad.rs:3 unused"),
        "{finished}"
    );
}

/// ② 통과하는 훅(pre-commit · commit-msg)의 출력도 결과에 있다. 메시지는 stdin 으로 넘어가 바이트 그대로 커밋되고(대시 · 따옴표 ·
/// `$` · 여러 줄), 진행에는 하위 명령만 보인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_passing_hook_output_is_in_the_result_and_the_message_goes_through_stdin() {
    let f = WriteFixture::new(true);
    hook(&f.repo, "pre-commit", "echo 'pre-commit ok'");
    hook(
        &f.repo,
        "commit-msg",
        "echo \"commit-msg saw: $(head -n1 \"$1\")\"",
    );
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    let before = f.repo.head();
    let message = "-n \"quoted\" $HOME `x`\n\nbody line\n";
    let mut sub = f.bus.subscribe(Filter::all());

    let result = commit(&f.write, params(&f, message)).await.unwrap();
    assert_eq!(result.commit, f.repo.head());
    assert_ne!(result.commit, before);
    assert_eq!(result.previous.as_deref(), Some(before.as_str()));
    assert_eq!(result.work_tree, f.repo.root().display().to_string());
    assert!(
        result.output.contains("pre-commit ok"),
        "{:?}",
        result.output
    );
    assert!(
        result
            .output
            .contains("commit-msg saw: -n \"quoted\" $HOME `x`"),
        "{:?}",
        result.output
    );
    assert_eq!(message_of(&f.repo, "HEAD"), message);
    assert_eq!(f.repo.git(&["rev-parse", "HEAD^"]).trim(), before);

    let events = until_finished(&mut sub).await;
    let progress: Vec<&Value> = events
        .iter()
        .filter(|(t, _)| t == TOPIC_PROGRESS)
        .map(|(_, p)| p)
        .collect();
    assert!(!progress.is_empty());
    for p in progress {
        assert_eq!(p["progress"]["message"], "git commit", "{p}");
    }
}

/// ③ amend 뒤 HEAD 의 트리는 index 의 트리이고 부모는 그대로다. `previous` 는 amend 전 HEAD 다([되돌리기] 의 해시).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn amend_replaces_head_with_the_index_tree_and_keeps_the_parent() {
    let f = WriteFixture::new(true);
    let parent = f.repo.head();
    let amended = f.repo.commit_file("a.txt", "one\n", "first try");
    f.repo.write("a.txt", "two\n");
    f.repo.write("b.txt", "new\n");
    f.repo.git(&["add", "a.txt", "b.txt"]);
    // 작업 트리에만 있는 변경은 커밋에 들지 않는다.
    f.repo.write("a.txt", "three\n");
    let index_tree = f.repo.git(&["write-tree"]).trim().to_owned();

    let result = commit(
        &f.write,
        GitCommitParams {
            amend: true,
            ..params(&f, "second try\n")
        },
    )
    .await
    .unwrap();
    assert_eq!(result.previous.as_deref(), Some(amended.as_str()));
    assert_eq!(result.commit, f.repo.head());
    assert_ne!(result.commit, amended);
    assert_eq!(f.repo.rev_parse("HEAD^{tree}"), index_tree);
    assert_eq!(
        f.repo.git(&["rev-list", "--parents", "-n1", "HEAD"]).trim(),
        format!("{} {parent}", result.commit)
    );
    assert_eq!(message_of(&f.repo, "HEAD"), "second try\n");
}

/// ④ sign-off 면 git 설정의 신원으로 `Signed-off-by:` 트레일러가 붙는다. 없으면 붙지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signoff_adds_the_trailer() {
    let f = WriteFixture::new(true);
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    commit(
        &f.write,
        GitCommitParams {
            signoff: true,
            ..params(&f, "signed\n")
        },
    )
    .await
    .unwrap();
    assert_eq!(
        message_of(&f.repo, "HEAD"),
        "signed\n\nSigned-off-by: Pitwall Test <test@pitwall.invalid>\n"
    );
    commit(
        &f.write,
        GitCommitParams {
            amend: true,
            ..params(&f, "unsigned\n")
        },
    )
    .await
    .unwrap();
    assert_eq!(message_of(&f.repo, "HEAD"), "unsigned\n");
}

/// ⑤ 신뢰한 프로젝트의 훅은 mise(제공자)가 정한 변수를 받고, 미신뢰 프로젝트의 훅은 받지 않는다 — 훅 자체는 미신뢰에서도
/// 돈다(사람이 누른 쓰기, 결정 M2-D8).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hooks_get_the_mise_env_only_in_a_trusted_project() {
    for trusted in [true, false] {
        let repo = TestRepo::with_commit();
        let marker = repo.base().join("hook-env");
        hook(
            &repo,
            "pre-commit",
            &format!(
                "printf '%s' \"${{PW_FAKE_MISE-unset}}\" > '{}'",
                marker.display()
            ),
        );
        let f = WriteFixture::with_provider(repo, trusted, Arc::new(crate::tests::FakeMise));
        assert_eq!(f.projects.is_trusted(&f.project), trusted);
        f.repo.write("a.txt", "a\n");
        f.repo.git(&["add", "a.txt"]);
        commit(&f.write, params(&f, "hook env\n")).await.unwrap();
        let seen = fs::read_to_string(&marker).expect("the hook ran");
        assert_eq!(
            seen,
            if trusted { "1" } else { "unset" },
            "trusted: {trusted}"
        );
    }
}

/// git 이 거절하는 것(빈 메시지)은 덮지 않고 원문으로 실패한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_message_is_refused_by_git() {
    let f = WriteFixture::new(true);
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    let before = f.repo.head();
    match commit(&f.write, params(&f, "")).await {
        Err(WriteError::Failed { stderr, .. }) => {
            assert!(stderr.contains("empty commit message"), "{stderr:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(f.repo.head(), before);
}

/// 아직 커밋이 없는 브랜치의 첫 커밋은 `previous` 가 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_commit_has_no_previous() {
    let f = WriteFixture::with_repo(TestRepo::new(), true, crate::write::Askpass::default());
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    let result = commit(&f.write, params(&f, "root\n")).await.unwrap();
    assert_eq!(result.previous, None);
    assert_eq!(result.commit, f.repo.head());
}

/// 와이어 형식: 창이 하는 그대로 Host 연결로 부른다. 성공은 결과 객체, 훅의 실패는 git 오류(`2004`)의 `data.stderr` 에 훅의 출력.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_rpc_the_result_and_the_hook_failure_carry_the_output() {
    let f = WriteFixture::new(true);
    let write = Arc::clone(&f.write);
    let host = LocalHost::builder("local")
        .methods(move |router| register(router, write))
        .build();
    let mut connection = host
        .connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(f.project.clone()),
        })
        .await
        .unwrap();

    hook(&f.repo, "pre-commit", "echo 'hook says hi'");
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    let reply = call(
        &mut connection,
        1,
        json!({"projectId": f.project, "message": "over rpc\n", "signoff": true}),
    )
    .await;
    let result = &reply["result"];
    assert_eq!(result["commit"], f.repo.head(), "{reply}");
    assert!(result["previous"].is_string(), "{reply}");
    assert_eq!(result["workTree"], f.repo.root().display().to_string());
    assert!(
        result["output"].as_str().unwrap().contains("hook says hi"),
        "{reply}"
    );

    hook(&f.repo, "pre-commit", "echo 'hook says no' >&2\nexit 3");
    let reply = call(
        &mut connection,
        2,
        json!({"projectId": f.project, "message": "refused\n", "amend": true}),
    )
    .await;
    let error = &reply["error"];
    assert_eq!(error["code"], code::GIT_FAILED, "{reply}");
    assert_eq!(error["data"]["exitCode"], 1, "{reply}");
    assert!(
        error["data"]["stderr"]
            .as_str()
            .unwrap()
            .contains("hook says no"),
        "{reply}"
    );
}

/// `git.commit` 을 부르고 그 응답을 기다린다.
async fn call(connection: &mut pw_core::Connection, id: i64, params: Value) -> Value {
    connection
        .sink
        .send(json!({"jsonrpc": "2.0", "id": id, "method": "git.commit", "params": params}));
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), connection.source.recv())
            .await
            .expect("no reply in time")
            .expect("connection closed");
        if frame["id"] == json!(id) {
            return frame;
        }
    }
}
