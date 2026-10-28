//! askpass 다이얼로그에 넣은 값으로 원격에 인증한다(docs/design/13-m2-plan.md M2-20 완료 기준): 진짜 `pitwall askpass` 를 거친
//! `git.push`(`pw_git::remote::push`)가 암호를 묻는 로컬 원격 픽스처로 보낸다. 창이 맞는 값을 답하면 원격 ref 가 움직이고(값이
//! 인증에 쓰였다), 틀린 값이면 원격이 거절해 그대로다. 두 경우 모두 넣은 값이 앱 로그(TRACE 까지) · 버스 이벤트(Task 진행 · 알림) ·
//! 결과 · 오류 · 앱 폴더 · 그때 떠 있던 모든 프로세스의 인자와 환경(`ps`, `ps -E`) · git 의 자식 환경 어디에도 없다.
//!
//! 원격 픽스처: git 의 `ext::` 전송으로 띄우는 `sh <authremote> <서비스> <bare 저장소>` 가 `$GIT_ASKPASS` 로 암호를 묻고, 그 답이 암호
//! 파일의 내용과 같을 때만 그 bare 저장소의 `git receive-pack` 을 잇는다. 사람의 git 설정 · 셸 · 네트워크를 쓰지 않는다 — 모두 임시
//! 폴더다. 앱(GUI)은 띄우지 않는다. `pw_diag::init` 은 프로세스에 한 번이라 이 파일은 시험 하나다(같은 파일의 다른 시험과
//! 프로세스를 나누지 않는 `cargo test` 에서도 돈다).

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pw_core::bus::{Filter, Received, Subscription};
use pw_git::testing::TestRepo;
use pw_git::testing::write::{Window, WriteFixture};
use pw_git::write::{Askpass, GitPromptAnswer, GitPromptRequest};

const SECRET: &str = "pw-push-secret-3e9a71";
const WRONG: &str = "pw-wrong-guess-c05b28";
const QUESTION: &str = "Password for 'ext://fixture': ";

/// `dir` 아래 모든 파일에서 `needle` 이 든 것.
fn files_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            found.extend(files_containing(&path, needle));
        } else if kind.is_file()
            && fs::read(&path).is_ok_and(|bytes| {
                bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes())
            })
        {
            found.push(path);
        }
    }
    found
}

/// 원격 픽스처의 스크립트를 `bin/authremote` 에 만든다. 암호는 `password` 파일에서 읽는다(인자 · 환경으로 넘기지 않는다). 물은 뒤
/// 모든 프로세스의 인자를 `<out>.ps`, 환경을 `<out>.psenv`(macOS `ps -E`), 자기 환경(git 의 자식 환경)을 `<out>.env`, 인증 결과를
/// `<out>.auth` 에 더한다.
fn auth_remote(bin: &Path, password: &Path, out: &Path) -> PathBuf {
    let path = bin.join("authremote");
    let out = out.display();
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nservice=\"$1\"\nrepo=\"$2\"\nanswer=$(\"$GIT_ASKPASS\" \"{QUESTION}\")\nps -axww -o args= >> '{out}.ps'\nps -axwwE -o command= >> '{out}.psenv' 2>/dev/null || true\nenv >> '{out}.env'\nif [ \"$answer\" != \"$(cat '{password}')\" ]; then\n  echo denied >> '{out}.auth'\n  echo 'fatal: Authentication failed for ext://fixture' >&2\n  exit 128\nfi\necho granted >> '{out}.auth'\nexec git \"${{service#git-}}\" \"$repo\"\n",
            password = password.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// 지금까지 버스에 나간 이벤트의 토픽. 어느 내용에도 `needles` 가 없어야 한다.
fn drain(events: &mut Subscription, needles: &[&str], topics: &mut Vec<String>) {
    while let Some(received) = events.try_recv() {
        if let Received::Event(event) = received {
            let payload = event.payload.to_string();
            for needle in needles {
                assert!(!payload.contains(needle), "{}: {payload}", event.topic);
            }
            topics.push(event.topic.clone());
        }
    }
}

/// `task.finished` 가 몇 번 나갈 때까지 버스를 비운다.
async fn until_finished(events: &mut Subscription, needles: &[&str], count: usize) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut topics = Vec::new();
    while topics.iter().filter(|t| *t == "task.finished").count() < count {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no task.finished: {topics:?}"
        );
        drain(events, needles, &mut topics);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    topics
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_askpass_answer_authenticates_a_push_and_is_kept_nowhere() {
    let askpass = Askpass::default();
    let f = WriteFixture::with_repo(TestRepo::with_commit(), true, askpass.clone());
    let logs = f.app.join("state").join("logs");
    pw_diag::init(&pw_diag::LogConfig {
        level: tracing::level_filters::LevelFilter::TRACE,
        ..pw_diag::LogConfig::new(&logs)
    })
    .unwrap();
    let _server = tokio::spawn(askpass.clone().serve(
        f.app.join("state").join("run").join("askpass.sock"),
        PathBuf::from(env!("CARGO_BIN_EXE_pitwall")),
    ));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while askpass.socket().is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the askpass socket did not open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 원격: 처음 커밋은 인증 없이(file://) 보내 두고, 그 뒤로는 암호를 묻는 `ext::` 로 바꾼다.
    let remote = f.repo.add_bare_remote("origin");
    let password = f.repo.base().join("password");
    fs::write(&password, SECRET).unwrap();
    let out = f.repo.base().join("remote-out");
    let script = auth_remote(&f.bin, &password, &out);
    f.repo.git(&["config", "protocol.ext.allow", "always"]);
    f.repo.git(&[
        "remote",
        "set-url",
        "origin",
        &format!("ext::sh {} %S {}", script.display(), remote.dir.display()),
    ]);
    let before = f.repo.remote_ref(&remote, "refs/heads/main");
    let tip = f.repo.commit_file("pushed.txt", "pushed\n", "pushed");

    let mut events = f.bus.subscribe(Filter::all());
    let needles = [SECRET, WRONG];
    let push = || {
        pw_git::remote::push(
            &f.write,
            f.caller(),
            f.project.clone(),
            None,
            None,
            None,
            None,
            false,
            None,
            None,
        )
    };

    // ① 틀린 값: 원격이 거절하고 그대로다. 오류에는 원격의 원문이 있고 넣은 값은 없다.
    let wrong = Window::open(&f.write, &f.project, |request| match request {
        GitPromptRequest::Askpass { .. } => Some(GitPromptAnswer::Askpass {
            value: WRONG.into(),
        }),
        GitPromptRequest::Unsaved { .. } => Some(GitPromptAnswer::Proceed),
    });
    let refused = push().await.unwrap_err();
    let refused = format!("{refused:?}");
    assert!(refused.contains("Authentication failed"), "{refused}");
    assert!(!refused.contains(WRONG), "{refused}");
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), before);
    assert_eq!(wrong.seen().len(), 1, "{:?}", wrong.seen());
    drop(wrong);
    until_finished(&mut events, &needles, 1).await;

    // ② 맞는 값: 그 값으로 인증해 원격이 움직인다.
    let right = Window::open(&f.write, &f.project, |request| match request {
        GitPromptRequest::Askpass { .. } => Some(GitPromptAnswer::Askpass {
            value: SECRET.into(),
        }),
        GitPromptRequest::Unsaved { .. } => Some(GitPromptAnswer::Proceed),
    });
    let pushed = push().await.unwrap();
    assert_eq!(
        right.seen(),
        [GitPromptRequest::Askpass {
            text: QUESTION.into(),
            hint: pw_git::write::AskpassHint::Secret,
        }],
        "the question went to the project window"
    );
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(tip));
    assert_eq!(
        fs::read_to_string(out.with_extension("auth")).unwrap(),
        "denied\ngranted\n"
    );
    let result = format!("{pushed:?}");
    assert!(
        !result.contains(SECRET),
        "the push result holds the answer: {result}"
    );
    until_finished(&mut events, &needles, 1).await;

    // 물을 때 떠 있던 프로세스의 인자에도, 앱 폴더(로그 포함)에도 없다.
    let ps = fs::read_to_string(out.with_extension("ps")).unwrap();
    assert!(ps.contains("authremote"), "ps saw the fixture: {ps}");
    for needle in needles {
        assert!(!ps.contains(needle), "a process argument holds {needle}");
    }
    // 환경: 물은 순간 git 이 띄운 픽스처의 환경(git 의 자식 환경)과, 같은 사용자의 모든 프로세스의 환경(`ps -E`, macOS).
    let env = fs::read_to_string(out.with_extension("env")).unwrap();
    assert!(
        env.contains("GIT_ASKPASS="),
        "the fixture saw git's environment"
    );
    let psenv = fs::read_to_string(out.with_extension("psenv")).unwrap_or_default();
    for needle in needles {
        assert!(
            !env.contains(needle),
            "the git child environment holds {needle}"
        );
        assert!(
            !psenv.contains(needle),
            "a process environment holds {needle}"
        );
    }
    let log = fs::read_dir(&logs)
        .unwrap()
        .flatten()
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect::<String>();
    assert!(
        log.contains("askpass answered"),
        "the log file is written: {log}"
    );
    for needle in needles {
        assert_eq!(files_containing(&f.app, needle), Vec::<PathBuf>::new());
    }
}
