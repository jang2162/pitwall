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
