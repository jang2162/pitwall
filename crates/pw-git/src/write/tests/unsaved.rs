//! 미저장 문서 보호(⑤ 의 Host 쪽): 작업 트리를 바꾸는 쓰기는 바뀔 경로를 창에 보내고 답을 받은 뒤에만 git 을 돌린다. 창이 취소하면
//! git 은 돌지 않는다. 듣는 창이 없으면 그대로 한다. 바뀔 경로를 모르면 작업 트리 전체(`paths: null`)다. 창이 답하지 않으면 시간이
//! 지나 취소된다. 창 쪽(미저장 문서와 겹치면 M1-13 의 확인)은 `apps/desktop/src/git/git-prompts.test.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::super::*;
use crate::testing::TestRepo;
use crate::testing::write::{Window, WriteFixture};

/// `other` 브랜치가 `README.md` 를 바꾼 저장소. 체크아웃하면 그 파일이 바뀐다.
fn fixture(timeouts: Option<(Duration, Duration)>) -> WriteFixture {
    let repo = TestRepo::with_commit();
    repo.git(&["checkout", "-q", "-b", "other"]);
    repo.commit_file("README.md", "changed on other\n", "other");
    repo.checkout("main");
    WriteFixture::with_options(repo, true, Askpass::default(), timeouts)
}

/// `other` 를 체크아웃하는 쓰기(바뀌는 경로는 `README.md`). 본문이 돌았는지를 함께 돌려준다.
async fn checkout(f: &WriteFixture, changes: Changes) -> (Result<(), WriteError>, bool) {
    let ran = Arc::new(AtomicBool::new(false));
    let mut request = f.request("test git checkout");
    request.changes = changes;
    let result = f
        .write
        .run(request, {
            let ran = Arc::clone(&ran);
            move |ctx| async move {
                ran.store(true, Ordering::SeqCst);
                ctx.run(ctx.git().args(["checkout", "-q", "other"]))
                    .await
                    .map(drop)
            }
        })
        .await;
    (result, ran.load(Ordering::SeqCst))
}

fn branch(f: &WriteFixture) -> String {
    f.repo
        .git(&["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned()
}

fn readme() -> Changes {
    Changes::Paths(vec!["README.md".into()])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkout_does_not_run_until_the_window_answers_and_cancel_keeps_the_tree() {
    let f = fixture(None);
    let window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Cancel));
    let (result, ran) = checkout(&f, readme()).await;
    assert_eq!(result, Err(WriteError::Cancelled));
    assert!(!ran, "git must not run when the window cancels");
    assert_eq!(branch(&f), "main");
    assert_eq!(
        window.seen(),
        [GitPromptRequest::Unsaved {
            work_tree: f.repo.root().display().to_string(),
            paths: Some(vec!["README.md".into()]),
        }]
    );
    drop(window);

    let window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
    let (result, ran) = checkout(&f, readme()).await;
    assert_eq!(result, Ok(()));
    assert!(ran);
    assert_eq!(branch(&f), "other");
    assert_eq!(window.seen().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_window_the_write_goes_ahead() {
    let f = fixture(None);
    let (result, ran) = checkout(&f, readme()).await;
    assert_eq!(result, Ok(()));
    assert!(ran);
    assert_eq!(branch(&f), "other");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_paths_ask_about_the_whole_work_tree_and_no_changes_ask_nothing() {
    let f = fixture(None);
    let window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
    checkout(&f, Changes::WorkTree).await.0.unwrap();
    f.repo.checkout("main");
    checkout(&f, Changes::None).await.0.unwrap();
    f.repo.checkout("main");
    checkout(&f, Changes::Paths(Vec::new())).await.0.unwrap();
    assert_eq!(
        window.seen(),
        [GitPromptRequest::Unsaved {
            work_tree: f.repo.root().display().to_string(),
            paths: None,
        }],
        "only the work-tree-wide write asked"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unanswered_confirmation_cancels_the_write() {
    let f = fixture(Some((Duration::from_secs(60), Duration::from_millis(300))));
    let window = Window::open(&f.write, &f.project, |_| None);
    let (result, ran) = checkout(&f, readme()).await;
    assert_eq!(result, Err(WriteError::Cancelled));
    assert!(!ran);
    assert_eq!(branch(&f), "main");
    assert_eq!(window.seen().len(), 1);
    assert_eq!(f.write.prompts().pending(), 0, "the prompt was closed");
}
