//! 질문 표: 듣는 창이 없으면 기다리지 않는다, 첫 답만 받는다, 종류에 맞는 답만 받는다, 끝나면 듣는 창 전부에 닫힘, 늦게 연 창도
//! 열린 질문을 받는다, 시간이 지나면 끝난다, 비밀은 `Debug` 에 나오지 않는다.

use std::time::Duration;

use super::super::prompt::*;

const LONG: Duration = Duration::from_secs(600);

fn askpass() -> GitPromptRequest {
    GitPromptRequest::Askpass {
        text: "Password for 'https://host': ".into(),
    }
}

fn open_id(event: Option<GitPromptEvent>) -> u64 {
    match event {
        Some(GitPromptEvent::Open { prompt }) => prompt.prompt_id,
        other => panic!("expected an open prompt, got {other:?}"),
    }
}

#[tokio::test]
async fn without_a_listening_window_ask_fails_at_once() {
    let prompts = Prompts::default();
    assert_eq!(
        prompts.ask("p", askpass(), LONG).await,
        Err(PromptError::NoWindow)
    );
    let other = prompts.listen("other-project");
    assert_eq!(
        prompts.ask("p", askpass(), LONG).await,
        Err(PromptError::NoWindow),
        "다른 프로젝트의 창은 듣는 창이 아니다"
    );
    drop(other);
    let window = prompts.listen("p");
    drop(window);
    assert_eq!(prompts.listeners("p"), 0, "버린 듣기는 빠진다");
    assert_eq!(
        prompts.ask("p", askpass(), LONG).await,
        Err(PromptError::NoWindow)
    );
    assert_eq!(prompts.pending(), 0);
}

#[tokio::test]
async fn the_first_answer_wins_and_every_window_hears_the_close() {
    let prompts = Prompts::default();
    let mut a = prompts.listen("p");
    let mut b = prompts.listen("p");
    let asking = tokio::spawn({
        let prompts = prompts.clone();
        async move { prompts.ask("p", askpass(), LONG).await }
    });
    let id = open_id(a.recv().await);
    assert_eq!(open_id(b.recv().await), id);

    assert_eq!(
        prompts.answer("other-project", id, GitPromptAnswer::Cancel),
        Err(AnswerError::NotFound),
        "다른 프로젝트의 답은 받지 않는다"
    );
    assert_eq!(
        prompts.answer("p", id, GitPromptAnswer::Proceed),
        Err(AnswerError::Mismatch),
        "askpass 에 proceed 는 맞지 않는다"
    );
    let secret = GitPromptAnswer::Askpass {
        value: "s3cret".into(),
    };
    prompts.answer("p", id, secret.clone()).unwrap();
    assert_eq!(
        prompts.answer("p", id, GitPromptAnswer::Cancel),
        Err(AnswerError::NotFound),
        "두 번째 답은 받지 않는다"
    );
    assert_eq!(asking.await.unwrap(), Ok(secret));
    for window in [&mut a, &mut b] {
        assert_eq!(
            window.recv().await,
            Some(GitPromptEvent::Closed { prompt_id: id })
        );
    }
    assert_eq!(prompts.pending(), 0);
}

#[tokio::test]
async fn a_window_that_starts_listening_late_gets_the_open_prompt() {
    let prompts = Prompts::default();
    let mut first = prompts.listen("p");
    let asking = tokio::spawn({
        let prompts = prompts.clone();
        async move {
            prompts
                .ask(
                    "p",
                    GitPromptRequest::Unsaved {
                        work_tree: "/w".into(),
                        paths: None,
                    },
                    LONG,
                )
                .await
        }
    });
    let id = open_id(first.recv().await);
    let mut late = prompts.listen("p");
    assert_eq!(open_id(late.try_recv()), id, "열린 질문이 먼저 온다");
    prompts.answer("p", id, GitPromptAnswer::Proceed).unwrap();
    assert_eq!(asking.await.unwrap(), Ok(GitPromptAnswer::Proceed));
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_prompt_times_out_and_closes() {
    let prompts = Prompts::default();
    let mut window = prompts.listen("p");
    let result = prompts.ask("p", askpass(), Duration::from_secs(120)).await;
    assert_eq!(result, Err(PromptError::TimedOut));
    let id = open_id(window.try_recv());
    assert_eq!(
        window.try_recv(),
        Some(GitPromptEvent::Closed { prompt_id: id })
    );
    assert_eq!(
        prompts.answer("p", id, GitPromptAnswer::Cancel),
        Err(AnswerError::NotFound)
    );
}

#[tokio::test]
async fn dropping_the_asker_closes_the_prompt() {
    let prompts = Prompts::default();
    let mut window = prompts.listen("p");
    let asking = tokio::spawn({
        let prompts = prompts.clone();
        async move { prompts.ask("p", askpass(), LONG).await }
    });
    let id = open_id(window.recv().await);
    asking.abort();
    assert_eq!(
        window.recv().await,
        Some(GitPromptEvent::Closed { prompt_id: id })
    );
    assert_eq!(prompts.pending(), 0);
}

#[tokio::test]
async fn when_the_last_window_stops_listening_the_open_prompt_ends_as_no_window() {
    let prompts = Prompts::default();
    let mut a = prompts.listen("p");
    let b = prompts.listen("p");
    let asking = tokio::spawn({
        let prompts = prompts.clone();
        async move { prompts.ask("p", askpass(), LONG).await }
    });
    open_id(a.recv().await);
    drop(b);
    tokio::task::yield_now().await;
    assert!(!asking.is_finished(), "a window is still listening");
    drop(a);
    let ended = tokio::time::timeout(Duration::from_secs(10), asking)
        .await
        .expect("the prompt did not wait for the 10-minute timeout")
        .unwrap();
    assert_eq!(ended, Err(PromptError::NoWindow));
    assert_eq!(prompts.pending(), 0);
}

#[test]
fn the_secret_is_hidden_from_debug_output() {
    let answer = GitPromptAnswer::Askpass {
        value: "hunter2".into(),
    };
    let text = format!("{answer:?} {:?}", Some(&answer));
    assert!(!text.contains("hunter2"), "{text}");
    assert!(text.contains("<hidden>"), "{text}");
}

#[test]
fn prompts_and_answers_have_the_documented_wire_format() {
    let event = GitPromptEvent::Open {
        prompt: GitPrompt {
            prompt_id: 7,
            request: GitPromptRequest::Unsaved {
                work_tree: "/w".into(),
                paths: Some(vec!["src/a.rs".into()]),
            },
        },
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        serde_json::json!({
            "kind": "open",
            "prompt": { "promptId": 7, "request": { "kind": "unsaved", "workTree": "/w", "paths": ["src/a.rs"] } },
        })
    );
    assert_eq!(
        serde_json::to_value(GitPromptEvent::Closed { prompt_id: 7 }).unwrap(),
        serde_json::json!({ "kind": "closed", "promptId": 7 })
    );
    let answer: GitPromptAnswer =
        serde_json::from_value(serde_json::json!({ "kind": "askpass", "value": "v" })).unwrap();
    assert_eq!(answer, GitPromptAnswer::Askpass { value: "v".into() });
    let proceed: GitPromptAnswer =
        serde_json::from_value(serde_json::json!({ "kind": "proceed" })).unwrap();
    assert_eq!(proceed, GitPromptAnswer::Proceed);
}
