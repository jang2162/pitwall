//! 진짜 `pitwall askpass` 를 거친 git 인증(docs/design/13-m2-plan.md M2-06 완료 기준 ④): askpass 를 부르는 시험 원격 도우미
//! (`git-remote-testauth`, `$GIT_ASKPASS` 를 그대로 부른다)로 git 이 암호를 물으면, 질문이 그 프로젝트를 듣는 창으로 가고, 창이 답한
//! 값이 이 실행 파일(`pitwall askpass`)의 stdout 으로 도우미에게 나오며, 앱의 로그 파일(TRACE 까지) · Task 진행 · 알림 · 앱 폴더
//! 어디에도 그 값이 없다.
//!
//! Host 쪽(쓰기 실행기 · askpass 소켓 · 창 흉내)은 이 시험 프로세스 안에서 돈다(`pw_git::testing::write`). 앱(GUI)은 띄우지 않는다.
//! 사람의 git 설정 · 셸 · `$DATA` 를 읽거나 쓰지 않는다 — 모두 임시 폴더다.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pw_core::bus::{Filter, Received};
use pw_git::testing::TestRepo;
use pw_git::testing::write::{Window, WriteFixture, credential_remote};
use pw_git::write::{
    Askpass, AskpassHint, GitPromptAnswer, GitPromptRequest, WriteError, WriteRequest,
};

const SECRET: &str = "pw-e2e-secret-81d4c2";
const QUESTION: &str = "Password for 'https://someone@example.invalid': ";

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_window_answer_reaches_git_through_pitwall_askpass_and_not_the_log() {
    let askpass = Askpass::default();
    let f = WriteFixture::with_repo(TestRepo::with_commit(), true, askpass.clone());
    // 앱과 같은 로그 구성(`$STATE/logs/core.<날짜>.log`)을 가장 자세한 수준으로.
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
    let window = Window::open(&f.write, &f.project, |request| match request {
        GitPromptRequest::Askpass { .. } => Some(GitPromptAnswer::Askpass {
            value: SECRET.into(),
        }),
        GitPromptRequest::Unsaved { .. } => Some(GitPromptAnswer::Proceed),
    });
    let mut events = f.bus.subscribe(Filter::all());
    let out = f.repo.base().join("remote-out");
    credential_remote(&f.bin, &out, QUESTION);

    let result = f
        .write
        .run(
            WriteRequest {
                askpass: true,
                ..f.request("test git fetch")
            },
            |ctx| async move {
                ctx.run(ctx.git().args(["fetch", "testauth::example"]))
                    .await
                    .map(drop)
            },
        )
        .await;
    // 시험 원격 도우미는 답을 받은 뒤 실패로 끝난다 — 그 원문이 Task 의 실패(알림 body)가 된다.
    assert!(
        matches!(result, Err(WriteError::Failed { .. })),
        "{result:?}"
    );

    assert_eq!(
        window.seen(),
        [GitPromptRequest::Askpass {
            text: QUESTION.into(),
            hint: AskpassHint::Secret,
        }],
        "the question went to the project window"
    );
    assert_eq!(
        fs::read_to_string(out.with_extension("code"))
            .unwrap()
            .trim(),
        "0",
        "pitwall askpass succeeded"
    );
    assert_eq!(
        fs::read_to_string(&out).unwrap(),
        format!("{SECRET}\n"),
        "the answer is on the helper's stdout"
    );

    // Task 는 표에서 먼저 빠지고 종료 알림이 뒤에 나간다(pw-core `task.rs`) — 알림 자체를 기다린다.
    let mut topics = Vec::new();
    while !topics.iter().any(|t| t == "task.finished") {
        assert!(
            tokio::time::Instant::now() < deadline + Duration::from_secs(20),
            "no task.finished: {topics:?}"
        );
        while let Some(received) = events.try_recv() {
            if let Received::Event(event) = received {
                assert!(
                    !event.payload.to_string().contains(SECRET),
                    "{}: {}",
                    event.topic,
                    event.payload
                );
                topics.push(event.topic.clone());
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(topics.iter().any(|t| t == "task.finished"), "{topics:?}");

    let log = fs::read_dir(&logs)
        .unwrap()
        .flatten()
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect::<String>();
    assert!(
        log.contains("askpass answered"),
        "the log file is written: {log}"
    );
    assert_eq!(files_containing(&f.app, SECRET), Vec::<PathBuf>::new());
}
