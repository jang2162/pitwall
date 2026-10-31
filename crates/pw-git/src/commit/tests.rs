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

    let result = commit(&f.write, f.caller(), params(&f, "add bad.rs")).await;
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

    let result = commit(&f.write, f.caller(), params(&f, message))
        .await
        .unwrap();
    assert_eq!(result.commit, f.repo.head());
    assert_ne!(result.commit, before);
    assert_eq!(
        result.moved.ref_name,
        f.repo.git(&["symbolic-ref", "HEAD"]).trim()
    );
    assert_eq!(result.moved.previous.as_deref(), Some(before.as_str()));
    assert_eq!(
        result.moved.current.as_deref(),
        Some(result.commit.as_str())
    );
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

/// ③ amend 뒤 HEAD 의 트리는 index 의 트리이고 부모는 그대로다. `moved.previous` 는 amend 전 HEAD 다([되돌리기] 의 해시).
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
        f.caller(),
        GitCommitParams {
            amend: true,
            ..params(&f, "second try\n")
        },
    )
    .await
    .unwrap();
    assert_eq!(result.moved.previous.as_deref(), Some(amended.as_str()));
    assert_eq!(
        result.moved.current.as_deref(),
        Some(result.commit.as_str())
    );
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
        f.caller(),
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
        f.caller(),
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
        commit(&f.write, f.caller(), params(&f, "hook env\n"))
            .await
            .unwrap();
        let seen = fs::read_to_string(&marker).expect("the hook ran");
        assert_eq!(
            seen,
            if trusted { "1" } else { "unset" },
            "trusted: {trusted}"
        );
    }
}

/// 커밋의 훅은 askpass 도우미를 받지 않는다 — 저장소의 훅이 진짜와 같은 암호 다이얼로그를 띄우지 못한다(`askpass: false`).
/// 소켓을 연 앱에서도 그렇다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hook_gets_no_askpass_helper_even_when_the_app_serves_one() {
    let askpass = crate::write::Askpass::default();
    let f = WriteFixture::with_repo(TestRepo::with_commit(), true, askpass.clone());
    let socket = f.app.join("run").join("x.sock");
    let server = tokio::spawn(askpass.clone().serve(socket, "/unused/pitwall".into()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while askpass.helper().is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the askpass socket did not open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let seen = f.repo.base().join("hook-env");
    hook(
        &f.repo,
        "pre-commit",
        &format!(
            "printf '%s|%s|%s|%s' \"$GIT_ASKPASS\" \"$SSH_ASKPASS\" \"$PITWALL_ASKPASS_SOCKET\" \"$PITWALL_ASKPASS_TOKEN\" > '{}'",
            seen.display()
        ),
    );
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    commit(&f.write, f.caller(), params(&f, "no helper\n"))
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&seen).unwrap(), "|||");
    server.abort();
}

/// 훅이 stderr 로 출력하고 git 이 이유를 stdout 으로만 낼 때(`nothing to commit`) Task 의 실패 알림 body 에 둘 다 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_failure_notification_has_the_hook_output_and_gits_stdout_reason() {
    let f = WriteFixture::new(true);
    hook(&f.repo, "pre-commit", "echo 'hook ran' >&2");
    let before = f.repo.head();
    let mut sub = f.bus.subscribe(Filter::all());

    let result = commit(&f.write, f.caller(), params(&f, "nothing\n")).await;
    assert!(
        matches!(result, Err(WriteError::Failed { .. })),
        "{result:?}"
    );
    assert_eq!(f.repo.head(), before);

    let events = until_finished(&mut sub).await;
    let finished = events
        .iter()
        .find(|(t, _)| t == TOPIC_FINISHED)
        .map(|(_, p)| p)
        .unwrap();
    let error = finished["error"].as_str().unwrap();
    assert!(error.contains("hook ran"), "{finished}");
    assert!(error.contains("nothing to commit"), "{finished}");
}

/// git 이 거절하는 것(빈 메시지)은 덮지 않고 원문으로 실패한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_message_is_refused_by_git() {
    let f = WriteFixture::new(true);
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    let before = f.repo.head();
    match commit(&f.write, f.caller(), params(&f, "")).await {
        Err(WriteError::Failed { stderr, .. }) => {
            assert!(stderr.contains("empty commit message"), "{stderr:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(f.repo.head(), before);
}

/// 아직 커밋이 없는 브랜치의 첫 커밋은 `moved.previous` 가 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_commit_has_no_previous() {
    let f = WriteFixture::with_repo(TestRepo::new(), true, crate::write::Askpass::default());
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    let result = commit(&f.write, f.caller(), params(&f, "root\n"))
        .await
        .unwrap();
    assert_eq!(result.moved.previous, None);
    assert_eq!(
        result.moved.current.as_deref(),
        Some(result.commit.as_str())
    );
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
            window: None,
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
    assert!(result["moved"]["previous"].is_string(), "{reply}");
    assert_eq!(result["moved"]["current"], result["commit"], "{reply}");
    assert!(
        result["moved"]["refName"]
            .as_str()
            .unwrap()
            .starts_with("refs/heads/"),
        "{reply}"
    );
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

/// 커밋은 그 프로젝트 창의 연결만 한다(쓰기 실행기의 범위 판정, M2-06): 다른 프로젝트 창 · Host 범위(웰컴 · 설정 창)는 owner
/// 라도 거절되고 HEAD 가 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_project_s_own_window_may_commit() {
    let f = WriteFixture::new(true);
    f.repo.write("a.txt", "a\n");
    f.repo.git(&["add", "a.txt"]);
    let before = f.repo.head();
    let other_root = f.repo.base().join("other");
    std::fs::create_dir_all(&other_root).unwrap();
    let other = f
        .projects
        .create(pw_core::registry::NewProject {
            root: other_root,
            ..Default::default()
        })
        .unwrap()
        .id;
    let write = Arc::clone(&f.write);
    let host = LocalHost::builder("local")
        .methods(move |router| register(router, write))
        .build();
    let params = json!({"projectId": f.project, "message": "from elsewhere\n"});
    for (who, scope) in [
        ("other project window", ConnectionScope::Project(other)),
        ("host", ConnectionScope::Host),
    ] {
        let mut connection = host
            .connect(Caller {
                role: Role::Owner,
                scope,
                window: None,
            })
            .await
            .unwrap();
        let reply = call(&mut connection, 1, params.clone()).await;
        assert_eq!(
            reply["error"]["code"],
            pw_rpc::code::PERMISSION_DENIED,
            "{who}: {reply}"
        );
        assert_eq!(f.repo.head(), before, "{who}");
    }

    let mut own = host
        .connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(f.project.clone()),
            window: None,
        })
        .await
        .unwrap();
    let reply = call(&mut own, 1, params).await;
    assert_eq!(reply["result"]["commit"], f.repo.head(), "{reply}");
    assert_ne!(f.repo.head(), before);
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

/// gix(이 프로세스)가 사람의 전역 설정 대신 `<base>/home/.gitconfig` 를 읽게 한다. 그 파일을 돌려준다.
fn isolate_global_config(base: &std::path::Path) -> std::path::PathBuf {
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let config = home.join(".gitconfig");
    fs::write(&config, "").unwrap();
    // SAFETY: nextest 는 시험마다 프로세스가 따로이고, 이 시험은 저장소를 열기 전에 부른다.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("GIT_CONFIG_GLOBAL", &config);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    config
}

/// `git.commit.template`(M2-16): 미신뢰 프로젝트는 저장소 설정이 정한 템플릿을 읽지 않고 사람의 전역 설정의 것(`~` 는 홈)을 읽는다.
/// 신뢰하면 저장소 설정의 것(상대 경로는 작업 트리 꼭대기 기준)이 앞선다. 너무 크거나 없는 파일 · 설정 없음은 `null`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_commit_template_comes_from_trusted_config_only() {
    let f = WriteFixture::new(false);
    let global = isolate_global_config(f.repo.base());
    let p = GitCommitTemplateParams {
        project_id: f.project.clone(),
        worktree: None,
    };
    let read = || template(f.write.git(), &p).unwrap().template;
    assert_eq!(read(), None);

    let home = global.parent().unwrap();
    fs::write(home.join("tpl.txt"), "global subject\n\n# global\n").unwrap();
    fs::write(&global, "[commit]\n\ttemplate = ~/tpl.txt\n").unwrap();
    f.repo.write("local.txt", "local subject\n");
    f.repo.git(&["config", "commit.template", "local.txt"]);
    // 저장소 설정이 저장소 밖 비밀 파일을 가리켜도 미신뢰면 읽지 않는다.
    assert_eq!(read().as_deref(), Some("global subject\n\n# global\n"));

    f.projects.set_trust(&f.project, true).unwrap();
    assert_eq!(read().as_deref(), Some("local subject\n"));

    f.repo
        .write("local.txt", &"x".repeat(TEMPLATE_MAX as usize + 1));
    assert_eq!(read(), None);
    f.repo
        .write("local.txt", &"y".repeat(TEMPLATE_MAX as usize));
    assert_eq!(read().map(|t| t.len()), Some(TEMPLATE_MAX as usize));
    fs::remove_file(f.repo.root().join("local.txt")).unwrap();
    assert_eq!(read(), None);
}

/// `git.commit.template` 의 읽는 파일(M2-16 리뷰 F1): 미신뢰 프로젝트는 전역 설정의 상대 경로를 풀지 않고, 절대 경로라도 작업 트리
/// 안을 지나면(직접 · 밖의 링크가 안을 가리킴) 읽지 않는다. 신뢰 · 미신뢰 모두 FIFO 에서 막히지 않고 일반 파일이 아니면 · 너무 크면
/// 읽지 않는다. 신뢰한 프로젝트의 상대 경로는 git 처럼 작업 트리 꼭대기 기준이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_commit_template_reads_only_regular_files_outside_an_untrusted_work_tree() {
    use std::os::unix::fs::symlink;
    let f = WriteFixture::new(false);
    let global = isolate_global_config(f.repo.base());
    let p = GitCommitTemplateParams {
        project_id: f.project.clone(),
        worktree: None,
    };
    let read = || template(f.write.git(), &p).unwrap().template;
    let set = |value: &str| {
        fs::write(&global, format!("[commit]\n\ttemplate = {value}\n")).unwrap();
    };
    let secret = f.repo.base().join("outside-secret.txt");
    fs::write(&secret, "SECRET KEY\n").unwrap();

    // 전역 설정의 상대 경로 + 저장소 안의 링크 → 루트 밖 파일. 미신뢰면 풀지 않는다.
    symlink(&secret, f.repo.root().join(".gitmessage")).unwrap();
    set(".gitmessage");
    assert_eq!(read(), None);
    // 절대 경로라도 작업 트리 안이면(그 링크) 읽지 않는다.
    set(&f.repo.root().join(".gitmessage").display().to_string());
    assert_eq!(read(), None);
    // 밖의 링크가 작업 트리 안 파일을 가리키면 읽지 않는다.
    f.repo.write("inside.txt", "inside\n");
    let hop = f.repo.base().join("hop.txt");
    symlink(f.repo.root().join("inside.txt"), &hop).unwrap();
    set(&hop.display().to_string());
    assert_eq!(read(), None);
    // 밖의 일반 파일(절대 경로)은 읽는다.
    let plain = f.repo.base().join("plain.txt");
    fs::write(&plain, "Subject\n# why\n").unwrap();
    set(&plain.display().to_string());
    assert_eq!(read().as_deref(), Some("Subject\n# why\n"));

    // FIFO: 막히지 않고 읽지 않는다(신뢰 · 미신뢰).
    let fifo = f.repo.base().join("fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    set(&fifo.display().to_string());
    for trusted in [false, true] {
        f.projects.set_trust(&f.project, trusted).unwrap();
        let git = Arc::clone(f.write.git());
        let p = p.clone();
        let got = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::task::spawn_blocking(move || template(&git, &p).unwrap().template),
        )
        .await
        .expect("reading a FIFO template must not block")
        .unwrap();
        assert_eq!(got, None);
    }
    // 폴더 · 너무 큰 파일도 읽지 않는다(신뢰).
    set(&f.repo.base().display().to_string());
    assert_eq!(read(), None);
    let big = f.repo.base().join("big.txt");
    fs::write(&big, "x".repeat(TEMPLATE_MAX as usize + 1)).unwrap();
    set(&big.display().to_string());
    assert_eq!(read(), None);
    // 신뢰하면 상대 경로는 작업 트리 꼭대기 기준이다(git 과 같다).
    set("inside.txt");
    assert_eq!(read().as_deref(), Some("inside\n"));
}

/// `core.commentChar`(M2-16 리뷰 F5): 창이 템플릿 주석을 걷을 글자. 없거나 `auto` 면 `#`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_comment_char_comes_from_core_comment_char() {
    let f = WriteFixture::new(true);
    isolate_global_config(f.repo.base());
    let p = GitCommitTemplateParams {
        project_id: f.project.clone(),
        worktree: None,
    };
    let char = || template(f.write.git(), &p).unwrap().comment_char;
    assert_eq!(char(), "#");
    f.repo.git(&["config", "core.commentChar", ";"]);
    assert_eq!(char(), ";");
    f.repo.git(&["config", "core.commentChar", "auto"]);
    assert_eq!(char(), "#");
}

/// 커밋 툴 윈도우의 ⌘⌥Z 를 스테이징된 이름 바뀜에 했을 때 창이 보내는 두 호출(M2-16 리뷰 F2): 스테이징을 풀고 옛 경로 · 새 경로를 둘 다
/// 버리면 옛 파일이 HEAD 의 바이트로 돌아오고 새 파일은 untracked 로 남는다(지우지 않는다). 옛 경로를 빼고 버리면(고치기 전의 창)
/// 옛 파일이 지워진 채 남는다 — 그 차이를 함께 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rolling_back_a_staged_rename_restores_the_old_file() {
    use crate::stage::methods::{GitDiscardParams, GitPathsParams};
    for with_old in [true, false] {
        let f = WriteFixture::new(true);
        // 버리기는 미저장 문서를 창에 묻는다 — 창이 「계속」 으로 답한다.
        let _window = crate::testing::write::Window::open(&f.write, &f.project, |_| {
            Some(crate::write::prompt::GitPromptAnswer::Proceed)
        });
        let body = "line one\nline two\r\nno newline at end";
        f.repo.write("old.txt", body);
        f.repo.commit_all("old");
        f.repo.git(&["mv", "old.txt", "new.txt"]);
        assert_eq!(
            f.repo.git(&["status", "--porcelain=v1"]),
            "R  old.txt -> new.txt\n"
        );
        let both = vec!["new.txt".to_owned(), "old.txt".to_owned()];
        f.write
            .unstage(
                f.caller(),
                GitPathsParams {
                    project_id: f.project.clone(),
                    worktree: None,
                    paths: both.clone(),
                },
            )
            .await
            .unwrap();
        let result = f
            .write
            .discard(
                f.caller(),
                GitDiscardParams {
                    project_id: f.project.clone(),
                    worktree: None,
                    paths: if with_old {
                        both
                    } else {
                        vec!["new.txt".to_owned()]
                    },
                    confirmed: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(result.untracked, ["new.txt"]);
        let status = f.repo.git(&["status", "--porcelain=v1"]);
        if with_old {
            assert_eq!(status, "?? new.txt\n");
            assert_eq!(
                fs::read(f.repo.root().join("old.txt")).unwrap(),
                body.as_bytes()
            );
            assert_eq!(f.repo.git(&["show", "HEAD:old.txt"]), body);
        } else {
            assert_eq!(status, " D old.txt\n?? new.txt\n");
            assert!(!f.repo.root().join("old.txt").exists());
        }
    }
}

/// 커밋 툴 윈도우의 창 시험 픽스처(13-m2-plan M2-16 완료 기준 「시나리오 2」): 작업 트리 diff 의 hunk 하나(묶음 체크)와 다른 hunk 의
/// 줄 둘(줄 체크)을 차례로 `git.stageLines` 로 스테이징하고 커밋한 결과를 JSON 으로 남긴다(`apps/desktop/src/git/commit/fixtures/
/// stage-commit.json`). 단계마다 그때의 `git.diff`(작업 트리) · index 쪽 `git.blob` · 작업 트리 파일 · 그 단계에서 보낸 요청이 있다 —
/// 창 시험(`apps/desktop/src/git/commit/commit-window.test.tsx`)이 이것을 가짜 Host 의 답으로 쓰고, 체크를 눌러 창이 보낸 요청이 이
/// 요청과 같은지 본다. 이 요청을 진짜 git 으로 돌린 뒤의 `git show --stat` 이 `stat` 이다. 다른지는 이 시험이 본다 — 다시 만들 때는
/// `PW_UPDATE_COMMIT_FIXTURE=1`.
mod window_fixture {
    use std::path::PathBuf;

    use super::*;
    use crate::diff::methods::{DiffSide, GitBlobParams, GitDiffParams, LineKind};
    use crate::stage::methods::{GitLinesParams, HunkSelection};

    const FIXTURE: &str = "../../apps/desktop/src/git/commit/fixtures/stage-commit.json";
    const PATH: &str = "src/app.rs";
    pub const MESSAGE: &str = "Change app\n\nStage one hunk and two lines of another.\n";

    fn text(variant: u8) -> String {
        let mut s = String::from("fn a() {\n");
        s.push_str(if variant == 0 {
            "    a1\n"
        } else {
            "    a1 changed\n"
        });
        s.push_str("}\n");
        for i in 0..30 {
            s.push_str(&format!("const F{i}: u32 = {i};\n"));
        }
        s.push_str("fn b() {\n");
        if variant == 0 {
            s.push_str("    b1 old\n");
        } else {
            s.push_str("    b1 new\n    b extra\n");
        }
        s.push_str("}\n");
        s
    }

    /// 고를 것: 첫 변경 묶음 전체, 또는 글자가 같은 바뀐 줄 하나.
    enum Pick {
        FirstGroup,
        Line(&'static str),
    }

    fn select(diff: &crate::diff::methods::GitDiff, pick: &Pick) -> HunkSelection {
        let hunks = &diff.files[0].hunks;
        match pick {
            Pick::FirstGroup => {
                let hunk = &hunks[0];
                let first = hunk
                    .lines
                    .iter()
                    .position(|l| l.kind != LineKind::Context)
                    .unwrap();
                let lines: Vec<u32> = (first..hunk.lines.len())
                    .take_while(|&i| hunk.lines[i].kind != LineKind::Context)
                    .map(|i| i as u32)
                    .collect();
                HunkSelection {
                    hunk: hunk.clone(),
                    lines: Some(lines),
                }
            }
            Pick::Line(text) => {
                let (hunk, index) = hunks
                    .iter()
                    .find_map(|h| {
                        h.lines
                            .iter()
                            .position(|l| l.kind != LineKind::Context && l.text == *text)
                            .map(|i| (h, i))
                    })
                    .unwrap();
                HunkSelection {
                    hunk: hunk.clone(),
                    lines: Some(vec![index as u32]),
                }
            }
        }
    }

    async fn fixture_value() -> serde_json::Value {
        let f = WriteFixture::new(true);
        f.repo.write(PATH, &text(0));
        f.repo.commit_all("base");
        f.repo.write(PATH, &text(1));
        let git = f.write.git();
        let diff_params = GitDiffParams {
            project_id: f.project.clone(),
            worktree: None,
            from: None,
            to: DiffSide::WorkTree,
            paths: Some(vec![PATH.to_owned()]),
            ignore_whitespace: false,
        };
        let state = || {
            let mut diff = git.diff(&diff_params).unwrap();
            diff.work_tree = "/repo".to_owned();
            let index = git
                .blob(&GitBlobParams {
                    project_id: f.project.clone(),
                    path: PATH.to_owned(),
                    ..GitBlobParams::default()
                })
                .unwrap();
            let work_tree = fs::read_to_string(f.repo.root().join(PATH)).unwrap();
            (diff, index, work_tree)
        };
        let mut steps = Vec::new();
        for pick in [
            Pick::FirstGroup,
            Pick::Line("    b1 new"),
            Pick::Line("    b1 old"),
        ] {
            let (diff, index, work_tree) = state();
            let selection = select(&diff, &pick);
            let request = GitLinesParams {
                project_id: f.project.clone(),
                worktree: None,
                path: PATH.to_owned(),
                old_path: None,
                hunks: vec![selection],
                ignore_whitespace: false,
            };
            f.write
                .stage_lines(f.caller(), request.clone())
                .await
                .unwrap();
            steps.push(json!({
                "diff": diff,
                "index": index,
                "workTree": work_tree,
                "request": { "path": request.path, "hunks": request.hunks },
            }));
        }
        let (diff, index, work_tree) = state();
        commit(&f.write, f.caller(), params(&f, MESSAGE))
            .await
            .unwrap();
        json!({
            "path": PATH,
            "message": MESSAGE,
            "steps": steps,
            "staged": { "diff": diff, "index": index, "workTree": work_tree },
            "stat": f.repo.git(&["show", "--stat", "--format=", "HEAD"]),
            "patch": f.repo.git(&["show", "--format=", "-U3", "--no-color", "HEAD"]),
            "left": f.repo.git(&["diff", "--no-color"]),
            "committedMessage": message_of(&f.repo, "HEAD"),
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_window_fixture_matches_pw_git_and_git() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
        let got = fixture_value().await;
        // 줄 단위로 고른 커밋: 첫 hunk 전체와 둘째 hunk 의 두 줄만 들었고 `b extra` 는 작업 트리에 남았다.
        assert!(got["patch"].as_str().unwrap().contains("+    a1 changed\n"));
        assert!(got["patch"].as_str().unwrap().contains("+    b1 new\n"));
        assert!(!got["patch"].as_str().unwrap().contains("b extra"));
        assert!(got["left"].as_str().unwrap().contains("+    b extra\n"));
        if std::env::var_os("PW_UPDATE_COMMIT_FIXTURE").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = serde_json::to_string_pretty(&got).unwrap();
            std::fs::write(&path, format!("{text}\n")).unwrap();
            return;
        }
        let want: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        assert!(
            got == want,
            "{} 이 지금 pw-git · git 의 출력과 다르다 — PW_UPDATE_COMMIT_FIXTURE=1 로 이 시험을 돌려 다시 만든다",
            path.display()
        );
    }
}

/// 커밋 툴 윈도우의 위험 작업 확인(M2-20 의 부품으로 바꾼 M2-16 의 확인)이 보이는 목록이 진짜 git 이 하는 일과 같은지 — 넷을 진짜
/// pw-git · git 으로 돌린 결과를 JSON 으로 남긴다(`apps/desktop/src/git/commit/fixtures/discard.json`). 창 시험(`commit-window.test.tsx`)이
/// 이것을 가짜 Host 의 답으로 쓰고, 확인 목록이 여기의 결과와 같은지 · 확인한 뒤 보낸 요청이 여기서 돌린 요청과 같은지 본다:
///
/// - `rollback`: 상태가 여러 가지인 파일(` M` · `MM` · ` D` · `A ` · `AD` · ` A` · 옛 경로에 추적 안 된 파일이 있는 `R `)을 모두 골라
///   ⌘⌥Z 한 요청(`git.unstage` → `git.discard`). `lost` 는 디스크의 바이트가 바뀌었거나 남지 않은 경로, `kept` 는 그대로 남은 경로다.
///   `rollbackChanges` 는 변경 그룹만 고른 것(`MM` · `AM` · ` A` — 스테이징을 풀지 않는다), `rollbackSubmodule` 은 가리키는 커밋을
///   스테이징한 서브모듈과 보통 파일. `lostIndex` 는 스테이징한 내용이 index 에도 디스크에도 남지 않은 경로, `untouched` 는 그대로인
///   서브모듈이다.
/// - `amendMixed`: 여러 상태가 섞인 저장소에서 amend → [되돌리기](soft) 앞뒤가 같은지.
/// - `hunk`: 작업 트리 diff 의 첫 변경 묶음을 `git.discardLines` 로 버린 요청과 그 뒤의 작업 트리.
/// - `stash`: stash 둘 중 오래된 것을 버린 요청과 그 뒤의 목록.
/// - `amend`: amend 결과의 `moved` 로 [되돌리기](`git.reset` soft)한 요청과 그 뒤의 HEAD · index.
///
/// 커밋 · stash 해시와 stash 의 시각은 돌 때마다 바뀌므로 정한 값으로 바꿔 둔다. 다시 만들 때는 `PW_UPDATE_COMMIT_FIXTURE=1`.
mod discard_fixture {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use super::*;
    use crate::diff::methods::{DiffSide, GitBlobParams, GitDiffParams, LineKind};
    use crate::history::ResetMode;
    use crate::history::methods::{GitResetParams, reset};
    use crate::stage::methods::{
        GitDiscardLinesParams, GitDiscardParams, GitPathsParams, GitStashDropParams,
        GitStashListParams, GitStashPushParams, HunkSelection,
    };
    use crate::status::{GitChange, GitStatusEntry, GitStatusParams, Status};
    use crate::testing::write::Window;
    use crate::write::GitPromptAnswer;

    const FIXTURE: &str = "../../apps/desktop/src/git/commit/fixtures/discard.json";

    fn proceed(f: &WriteFixture) -> Window {
        Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed))
    }

    async fn entries(f: &WriteFixture) -> Vec<GitStatusEntry> {
        Arc::new(Status::new(Arc::clone(f.write.git())))
            .status(GitStatusParams {
                project_id: f.project.clone(),
                worktree: None,
                paths: None,
            })
            .await
            .unwrap()
            .entries
    }

    /// 디스크의 모습: 파일이면 바이트, 폴더(서브모듈)면 그 안의 HEAD 와 상태.
    fn disk(f: &WriteFixture, path: &str) -> Option<Vec<u8>> {
        let full = f.repo.root().join(path);
        if full.is_dir() {
            let head = f.repo.git_in(&full, &["rev-parse", "HEAD"]);
            let status = f.repo.git_in(&full, &["status", "--porcelain"]);
            return Some(format!("dir {head}{status}").into_bytes());
        }
        fs::read(full).ok()
    }

    /// index 에 있는 그 경로의 객체(없으면 `None`).
    fn staged_object(f: &WriteFixture, path: &str) -> Option<String> {
        let out = f.repo.try_git_in(
            f.repo.root(),
            &["rev-parse", "--verify", "-q", &format!(":{path}")],
        );
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// 작업 트리 파일의 blob 해시(파일이 아니면 `None`).
    fn disk_object(f: &WriteFixture, path: &str) -> Option<String> {
        if !f.repo.root().join(path).is_file() {
            return None;
        }
        Some(f.repo.git(&["hash-object", "--", path]).trim().to_owned())
    }

    fn remove(f: &WriteFixture, path: &str) {
        fs::remove_file(f.repo.root().join(path)).unwrap();
    }

    /// 창의 고르기와 같다(commit-model.ts `groupEntries` · `rollbackPlan`): 스테이징됨 그룹(경로순, `staged_too` 일 때만) · 변경
    /// 그룹(경로순)을 모두 고르면 스테이징을 풀 경로는 스테이징됨 그룹의 경로와 그 옛 경로(그 차례 그대로, 겹치면 하나), 버릴 경로는
    /// 둘 다의 경로순이다.
    fn requests(entries: &[GitStatusEntry], staged_too: bool) -> (Vec<String>, Vec<String>) {
        let untracked = |e: &GitStatusEntry| e.unstaged == Some(GitChange::Untracked);
        let mut unstage: Vec<String> = Vec::new();
        let add = |list: &mut Vec<String>, path: &str| {
            if !list.iter().any(|p| p == path) {
                list.push(path.to_owned());
            }
        };
        if staged_too {
            for e in entries
                .iter()
                .filter(|e| e.staged.is_some() && !untracked(e))
            {
                add(&mut unstage, &e.path);
                if let Some(orig) = &e.orig_path {
                    add(&mut unstage, orig);
                }
            }
        }
        let mut discard: BTreeSet<String> = unstage.iter().cloned().collect();
        for e in entries
            .iter()
            .filter(|e| e.unstaged.is_some() && !untracked(e))
        {
            discard.insert(e.path.clone());
        }
        (unstage, discard.into_iter().collect())
    }

    /// 지금 상태에서 고른 것을 되돌리고(`git.unstage` → `git.discard`) 견준다: 디스크가 그대로인 파일(`kept`) · 아닌 경로(`lost`) ·
    /// 그대로인 서브모듈(`untouched`), 그리고 스테이징한 내용이 index 에도 디스크에도 남지 않은 경로(`lostIndex` — intent-to-add 의 빈
    /// 항목과 서브모듈은 빼고).
    async fn roll_back(f: &WriteFixture, staged_too: bool) -> serde_json::Value {
        let before = entries(f).await;
        let (unstage, discard) = requests(&before, staged_too);
        let touched: BTreeSet<String> = unstage.iter().chain(&discard).cloned().collect();
        let bytes: BTreeMap<String, Option<Vec<u8>>> =
            touched.iter().map(|p| (p.clone(), disk(f, p))).collect();
        let submodules: BTreeSet<String> = touched
            .iter()
            .filter(|p| f.repo.root().join(p).is_dir())
            .cloned()
            .collect();
        let intent = |path: &str| {
            before.iter().any(|e| {
                e.path == path && e.staged.is_none() && e.unstaged == Some(GitChange::Added)
            })
        };
        let objects: BTreeMap<String, Option<String>> = touched
            .iter()
            .filter(|p| !submodules.contains(*p) && !intent(p))
            .map(|p| (p.clone(), staged_object(f, p)))
            .collect();
        if !unstage.is_empty() {
            f.write
                .unstage(
                    f.caller(),
                    GitPathsParams {
                        project_id: f.project.clone(),
                        worktree: None,
                        paths: unstage.clone(),
                    },
                )
                .await
                .unwrap();
        }
        let result = f
            .write
            .discard(
                f.caller(),
                GitDiscardParams {
                    project_id: f.project.clone(),
                    worktree: None,
                    paths: discard.clone(),
                    confirmed: true,
                },
            )
            .await
            .unwrap();
        let (mut lost, mut kept, mut untouched) = (Vec::new(), Vec::new(), Vec::new());
        for (path, was) in &bytes {
            let now = disk(f, path);
            if was.is_some() && *was == now {
                if submodules.contains(path) {
                    untouched.push(path.clone());
                } else {
                    kept.push(path.clone());
                }
            } else {
                lost.push(path.clone());
            }
        }
        let lost_index: Vec<String> = objects
            .iter()
            .filter(|(path, was)| {
                was.is_some() && **was != staged_object(f, path) && **was != disk_object(f, path)
            })
            .map(|(path, _)| path.clone())
            .collect();
        json!({
            "entries": before,
            "unstage": unstage,
            "discard": discard,
            "untracked": result.untracked,
            "lost": lost,
            "lostIndex": lost_index,
            "kept": kept,
            "untouched": untouched,
            "after": f.repo.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
        })
    }

    /// 스테이징됨 · 변경 그룹을 모두 고른다.
    async fn rollback() -> serde_json::Value {
        let f = WriteFixture::new(true);
        let _window = proceed(&f);
        for (path, body) in [
            ("mod.txt", "m0\n"),
            ("staged.txt", "s0\n"),
            ("gone.txt", "g0\n"),
            ("old.txt", "renamed body\n"),
        ] {
            f.repo.write(path, body);
        }
        f.repo.commit_all("base");
        f.repo.write("mod.txt", "m1\n");
        f.repo.write("staged.txt", "s1\n");
        f.repo.git(&["add", "staged.txt"]);
        f.repo.write("staged.txt", "s2\n");
        remove(&f, "gone.txt");
        f.repo.write("added.txt", "new file\n");
        f.repo.write("added-gone.txt", "staged then deleted\n");
        f.repo.git(&["add", "added.txt", "added-gone.txt"]);
        remove(&f, "added-gone.txt");
        f.repo.write("am.txt", "staged first\n");
        f.repo.git(&["add", "am.txt"]);
        f.repo.write("am.txt", "then edited\n");
        f.repo.write("ita.txt", "intent to add\n");
        f.repo.git(&["add", "-N", "ita.txt"]);
        f.repo.git(&["mv", "old.txt", "new.txt"]);
        f.repo
            .write("old.txt", "someone else's file at the old path\n");
        f.repo.write("loose.txt", "untracked, not chosen\n");
        roll_back(&f, true).await
    }

    /// 서브모듈(가리키는 커밋을 바꿔 스테이징한 것)과 보통 파일을 함께 고른다 — `git restore` 는 서브모듈을 건드리지 않는다.
    async fn rollback_submodule() -> serde_json::Value {
        let f = WriteFixture::new(true);
        let _window = proceed(&f);
        let sub = crate::testing::TestRepo::with_commit();
        f.repo.write("plain.txt", "p0\n");
        f.repo.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub.root().to_str().unwrap(),
            "sm",
        ]);
        f.repo.commit_all("base with a submodule");
        f.repo.write("plain.txt", "p1\n");
        f.repo.write("sm/inner.txt", "inner change\n");
        f.repo
            .commit_all_in(&f.repo.root().join("sm"), "move the submodule");
        f.repo.git(&["add", "sm"]);
        let mut value = roll_back(&f, true).await;
        // 서브모듈 안의 커밋 해시는 돌 때마다 다르다 — 그 해시를 담은 `.gitmodules` 밖의 것은 없다.
        value["after"] = json!(f.repo.git(&["status", "--porcelain=v1"]));
        value
    }

    /// 변경 그룹만 고른다(스테이징은 그대로 — 작업 트리를 index 로).
    async fn rollback_changes() -> serde_json::Value {
        let f = WriteFixture::new(true);
        let _window = proceed(&f);
        f.repo.write("both.txt", "b0\n");
        f.repo.commit_all("base");
        f.repo.write("both.txt", "b1\n");
        f.repo.git(&["add", "both.txt"]);
        f.repo.write("both.txt", "b2\n");
        f.repo.write("added.txt", "staged\n");
        f.repo.git(&["add", "added.txt"]);
        f.repo.write("added.txt", "staged, then edited\n");
        f.repo.write("ita.txt", "intent to add\n");
        f.repo.git(&["add", "-N", "ita.txt"]);
        roll_back(&f, false).await
    }

    fn app_text(variant: u8) -> String {
        let mut s = String::from("fn a() {\n");
        s.push_str(if variant == 0 {
            "    a1\n"
        } else {
            "    a1 changed\n    a2 added\n"
        });
        s.push_str("}\n");
        for i in 0..30 {
            s.push_str(&format!("const F{i}: u32 = {i};\n"));
        }
        s.push_str(if variant == 0 {
            "fn b() {}\n"
        } else {
            "fn b() { changed }\n"
        });
        s
    }

    async fn hunk() -> serde_json::Value {
        const PATH: &str = "src/app.rs";
        let f = WriteFixture::new(true);
        let _window = proceed(&f);
        f.repo.write(PATH, &app_text(0));
        f.repo.commit_all("base");
        f.repo.write(PATH, &app_text(1));
        let git = f.write.git();
        let mut diff = git
            .diff(&GitDiffParams {
                project_id: f.project.clone(),
                worktree: None,
                from: None,
                to: DiffSide::WorkTree,
                paths: Some(vec![PATH.to_owned()]),
                ignore_whitespace: false,
            })
            .unwrap();
        diff.work_tree = "/repo".to_owned();
        let index = git
            .blob(&GitBlobParams {
                project_id: f.project.clone(),
                path: PATH.to_owned(),
                ..GitBlobParams::default()
            })
            .unwrap();
        let work_tree = fs::read_to_string(f.repo.root().join(PATH)).unwrap();
        // 첫 변경 묶음(창의 ↶ 와 같다 — hunk-staging.ts `stageGroups`): 첫 hunk 의 문맥 줄 없이 이어진 바뀐 줄.
        let first = &diff.files[0].hunks[0];
        let start = first
            .lines
            .iter()
            .position(|l| l.kind != LineKind::Context)
            .unwrap();
        let lines: Vec<u32> = (start..first.lines.len())
            .take_while(|&i| first.lines[i].kind != LineKind::Context)
            .map(|i| i as u32)
            .collect();
        let hunks = vec![HunkSelection {
            hunk: first.clone(),
            lines: Some(lines),
        }];
        f.write
            .discard_lines(
                f.caller(),
                GitDiscardLinesParams {
                    project_id: f.project.clone(),
                    worktree: None,
                    path: PATH.to_owned(),
                    hunks: hunks.clone(),
                    ignore_whitespace: false,
                    confirmed: true,
                },
            )
            .await
            .unwrap();
        json!({
            "path": PATH,
            "diff": diff,
            "index": index,
            "workTree": work_tree,
            "request": { "path": PATH, "hunks": hunks },
            "after": fs::read_to_string(f.repo.root().join(PATH)).unwrap(),
            "left": f.repo.git(&["diff", "--no-color"]),
        })
    }

    async fn stash(names: &mut Vec<(String, String)>) -> serde_json::Value {
        let f = WriteFixture::new(true);
        let _window = proceed(&f);
        f.repo.write("a.txt", "a0\n");
        f.repo.commit_all("base");
        for (i, message) in ["older", "newer"].into_iter().enumerate() {
            f.repo.write("a.txt", &format!("a{}\n", i + 1));
            f.write
                .stash_push(
                    f.caller(),
                    GitStashPushParams {
                        project_id: f.project.clone(),
                        worktree: None,
                        message: Some(message.to_owned()),
                        include_untracked: false,
                    },
                )
                .await
                .unwrap();
        }
        // 만든 시각은 돌 때마다 다르다 — 정한 값으로 둔다.
        let list = || async {
            let mut stashes = f
                .write
                .stash_list(GitStashListParams {
                    project_id: f.project.clone(),
                    worktree: None,
                })
                .await
                .unwrap()
                .stashes;
            for s in &mut stashes {
                s.time = 1_700_000_000;
            }
            stashes
        };
        let before = list().await;
        let older = before
            .iter()
            .find(|s| s.message.ends_with("older"))
            .unwrap();
        for (i, s) in before.iter().enumerate() {
            names.push((s.commit.clone(), format!("{}", i + 1).repeat(40)));
        }
        let request = GitStashDropParams {
            project_id: f.project.clone(),
            worktree: None,
            index: older.index,
            commit: older.commit.clone(),
            confirmed: true,
        };
        f.write
            .stash_drop(f.caller(), request.clone())
            .await
            .unwrap();
        json!({
            "stashes": before,
            "request": { "index": request.index, "commit": request.commit },
            "after": list().await,
        })
    }

    async fn amend(names: &mut Vec<(String, String)>) -> serde_json::Value {
        let f = WriteFixture::new(true);
        f.repo.write("a.txt", "first\n");
        let previous = f.repo.commit_all("add a");
        f.repo.write("a.txt", "fixed\n");
        f.repo.git(&["add", "a.txt"]);
        let index_before = f.repo.git(&["ls-files", "-s"]);
        let result = commit(
            &f.write,
            f.caller(),
            GitCommitParams {
                amend: true,
                ..params(&f, "add a\n")
            },
        )
        .await
        .unwrap();
        assert_eq!(result.moved.previous.as_deref(), Some(previous.trim()));
        names.push((previous.trim().to_owned(), "a".repeat(40)));
        names.push((result.commit.clone(), "b".repeat(40)));
        let request = GitResetParams {
            project_id: f.project.clone(),
            worktree: None,
            commit: result.moved.previous.clone().unwrap(),
            mode: ResetMode::Soft,
            expected: result.moved.current.clone(),
            expected_ref: Some(result.moved.ref_name.clone()),
            fingerprint: None,
        };
        reset(f.write.clone(), f.caller(), request.clone())
            .await
            .unwrap();
        json!({
            "moved": result.moved,
            "request": {
                "commit": request.commit,
                "mode": request.mode,
                "expected": request.expected,
                "expectedRef": request.expected_ref,
                "fingerprint": request.fingerprint,
            },
            "headBefore": previous.trim(),
            "head": f.repo.head(),
            "indexBefore": index_before,
            "index": f.repo.git(&["ls-files", "-s"]),
            "staged": f.repo.git(&["diff", "--cached", "--name-status"]),
        })
    }

    /// 스테이징한 수정 · 삭제 · 이름 바뀜 · 새 파일, intent-to-add, 작업 트리 편집, untracked 를 섞은 저장소에서 amend 뒤 [되돌리기]
    /// (soft) 하면 HEAD · index · 상태 · 작업 파일이 amend 전과 같다.
    async fn amend_mixed() -> serde_json::Value {
        let f = WriteFixture::new(true);
        for (path, body) in [("m.txt", "m0\n"), ("d.txt", "d0\n"), ("r.txt", "r0\n")] {
            f.repo.write(path, body);
        }
        let previous = f.repo.commit_all("base");
        f.repo.write("m.txt", "m1\n");
        f.repo.git(&["add", "m.txt"]);
        f.repo.write("m.txt", "m2 not staged\n");
        f.repo.git(&["rm", "-q", "d.txt"]);
        f.repo.git(&["mv", "r.txt", "renamed.txt"]);
        f.repo.write("n.txt", "new\n");
        f.repo.git(&["add", "n.txt"]);
        f.repo.write("ita.txt", "intent\n");
        f.repo.git(&["add", "-N", "ita.txt"]);
        f.repo.write("loose.txt", "untracked\n");
        let look = || {
            let mut files = BTreeMap::new();
            for path in [
                "m.txt",
                "d.txt",
                "r.txt",
                "renamed.txt",
                "n.txt",
                "ita.txt",
                "loose.txt",
            ] {
                files.insert(path, disk(&f, path).map(|b| String::from_utf8(b).unwrap()));
            }
            json!({
                "index": f.repo.git(&["ls-files", "-s"]),
                "status": f.repo.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
                "files": files,
            })
        };
        let before = look();
        let result = commit(
            &f.write,
            f.caller(),
            GitCommitParams {
                amend: true,
                ..params(&f, "base, amended\n")
            },
        )
        .await
        .unwrap();
        reset(
            f.write.clone(),
            f.caller(),
            GitResetParams {
                project_id: f.project.clone(),
                worktree: None,
                commit: result.moved.previous.clone().unwrap(),
                mode: ResetMode::Soft,
                expected: result.moved.current.clone(),
                expected_ref: Some(result.moved.ref_name.clone()),
                fingerprint: None,
            },
        )
        .await
        .unwrap();
        json!({
            "headIsPrevious": f.repo.head().trim() == previous.trim(),
            "before": before,
            "after": look(),
        })
    }

    async fn fixture_value() -> serde_json::Value {
        let mut names = Vec::new();
        let value = json!({
            "rollback": rollback().await,
            "rollbackChanges": rollback_changes().await,
            "rollbackSubmodule": rollback_submodule().await,
            "amendMixed": amend_mixed().await,
            "hunk": hunk().await,
            "stash": stash(&mut names).await,
            "amend": amend(&mut names).await,
        });
        let mut text = serde_json::to_string(&value).unwrap();
        for (hash, name) in &names {
            text = text.replace(hash.as_str(), name);
        }
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discard_window_fixture_matches_pw_git_and_git() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
        let got = fixture_value().await;
        let rollback = &got["rollback"];
        // 새 파일 · intent-to-add · 이름 바뀜의 새 경로는 디스크에 남고, 나머지(옛 경로의 남의 파일 포함)는 바뀐다.
        assert_eq!(
            rollback["kept"],
            json!(["added.txt", "am.txt", "ita.txt", "new.txt"]),
            "{rollback}"
        );
        assert_eq!(
            rollback["lost"],
            json!([
                "added-gone.txt",
                "gone.txt",
                "mod.txt",
                "old.txt",
                "staged.txt"
            ]),
            "{rollback}"
        );
        // 스테이징한 내용이 index 에도 디스크에도 남지 않는 것: `MM` · `AD` 와, 디스크에는 남는 `AM` 의 스테이징 쪽.
        assert_eq!(
            rollback["lostIndex"],
            json!(["added-gone.txt", "am.txt", "staged.txt"]),
            "{rollback}"
        );
        // 서브모듈은 가리키는 커밋을 스테이징한 것을 풀어도 그 안은 그대로다(` M sm` 이 남는다).
        let sub = &got["rollbackSubmodule"];
        assert_eq!(sub["untouched"], json!(["sm"]), "{sub}");
        assert_eq!(sub["lost"], json!(["plain.txt"]), "{sub}");
        assert_eq!(sub["after"], json!(" M sm\n"), "{sub}");
        // 섞인 저장소에서도 amend 를 되돌리면 HEAD · index · 상태 · 작업 파일이 amend 전과 같다.
        let mixed = &got["amendMixed"];
        assert_eq!(mixed["headIsPrevious"], json!(true));
        assert_eq!(mixed["before"], mixed["after"]);
        // 고르지 않은 untracked 는 그대로다.
        assert!(rollback["after"].as_str().unwrap().contains("?? loose.txt"));
        // 변경 그룹만이면 작업 트리를 index 로 — 스테이징한 것은 남고 intent-to-add 만 그대로다.
        let changes = &got["rollbackChanges"];
        assert_eq!(changes["unstage"], json!([]), "{changes}");
        assert_eq!(changes["kept"], json!(["ita.txt"]), "{changes}");
        assert_eq!(
            changes["lost"],
            json!(["added.txt", "both.txt"]),
            "{changes}"
        );
        assert_eq!(
            changes["after"],
            json!("A  added.txt\nM  both.txt\n A ita.txt\n"),
            "{changes}"
        );
        // 첫 묶음만 버렸다.
        let left = got["hunk"]["left"].as_str().unwrap();
        assert!(
            !left.contains("a1 changed") && !left.contains("a2 added"),
            "{left}"
        );
        assert!(left.contains("+fn b() { changed }"), "{left}");
        // 오래된 stash 만 사라졌다.
        let after = got["stash"]["after"].as_array().unwrap();
        assert_eq!(after.len(), 1);
        assert!(after[0]["message"].as_str().unwrap().ends_with("newer"));
        // amend 를 되돌리면 HEAD 는 이전 커밋이고 index 는 amend 직전 그대로(고친 내용이 스테이징돼 있다).
        let amend = &got["amend"];
        assert_eq!(amend["head"], amend["headBefore"]);
        assert_eq!(amend["index"], amend["indexBefore"]);
        assert_eq!(amend["staged"], json!("M\ta.txt\n"));
        if std::env::var_os("PW_UPDATE_COMMIT_FIXTURE").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = serde_json::to_string_pretty(&got).unwrap();
            std::fs::write(&path, format!("{text}\n")).unwrap();
            return;
        }
        let want: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        assert!(
            got == want,
            "{} 이 지금 pw-git · git 의 출력과 다르다 — PW_UPDATE_COMMIT_FIXTURE=1 로 이 시험을 돌려 다시 만든다",
            path.display()
        );
    }
}
