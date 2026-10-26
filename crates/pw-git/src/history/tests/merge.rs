//! merge: fast-forward 일 때 · 아닐 때(merge 커밋) · 충돌일 때 각각 HEAD 와 `git.operation`, 이미 들어 있음, `--ff-only` 의 거절,
//! `--no-ff`.

use super::super::methods::{GitMergeParams, merge};
use super::super::{FastForward, GitRefMove, HistoryOutcome};
use super::{parents, window};
use crate::methods::RepoOperation;
use crate::testing::write::WriteFixture;

fn params(f: &WriteFixture, rev: &str, fast_forward: FastForward) -> GitMergeParams {
    GitMergeParams {
        project_id: f.project.clone(),
        worktree: None,
        rev: rev.to_owned(),
        fast_forward,
    }
}

/// `feature` 에 커밋 하나(`f.txt`)를 더하고 `main` 으로 돌아온다. (main 끝, feature 끝).
fn feature_ahead(f: &WriteFixture) -> (String, String) {
    let main = f.repo.head();
    f.repo.git(&["checkout", "-q", "-b", "feature"]);
    let feature = f.repo.commit_file("f.txt", "feature\n", "feature");
    f.repo.checkout("main");
    (main, feature)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fast_forward_moves_the_branch_without_a_new_commit() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (main, feature) = feature_ahead(&f);
    let done = merge(f.write.clone(), params(&f, "feature", FastForward::Allow))
        .await
        .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::FastForward);
    assert_eq!(done.head.as_deref(), Some(feature.as_str()));
    assert_eq!(f.repo.head(), feature);
    assert_eq!(
        done.moved,
        Some(GitRefMove {
            ref_name: "refs/heads/main".into(),
            previous: main,
            current: Some(feature),
        })
    );
    assert_eq!(done.operation, None);
    assert_eq!(f.write.operation(&f.project, None).await, Ok(None));
    assert_eq!(done.work_tree, f.repo.root().display().to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_diverged_merge_makes_a_merge_commit_with_both_parents() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (_, feature) = feature_ahead(&f);
    let main = f.repo.commit_file("m.txt", "main\n", "main");
    let done = merge(f.write.clone(), params(&f, "feature", FastForward::Allow))
        .await
        .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::Done);
    let head = f.repo.head();
    assert_eq!(done.head.as_deref(), Some(head.as_str()));
    assert_eq!(parents(&f, "HEAD"), [main.clone(), feature]);
    assert_eq!(done.moved.unwrap().previous, main);
    assert_eq!(f.write.operation(&f.project, None).await, Ok(None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_ff_makes_a_merge_commit_even_when_it_could_fast_forward() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (main, feature) = feature_ahead(&f);
    let done = merge(f.write.clone(), params(&f, "feature", FastForward::Never))
        .await
        .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::Done);
    assert_eq!(parents(&f, "HEAD"), [main, feature]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflict_stops_with_the_merge_in_progress_and_head_unmoved() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let d = f.repo.diverge();
    let before = f.repo.head();
    let stopped = merge(f.write.clone(), params(&f, &d.theirs, FastForward::Allow))
        .await
        .unwrap();
    assert_eq!(stopped.outcome, HistoryOutcome::Stopped);
    assert_eq!(stopped.head.as_deref(), Some(before.as_str()));
    assert_eq!(stopped.moved, None, "the undo of a stopped merge is abort");
    let operation = stopped.operation.unwrap();
    assert_eq!(operation.kind, RepoOperation::Merge);
    assert_eq!(operation.heads, [d.theirs_commit]);
    assert_eq!(
        f.write.operation(&f.project, None).await.unwrap(),
        Some(operation)
    );
    assert_eq!(f.repo.head(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merging_what_is_already_in_is_up_to_date() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let first = f.repo.head();
    f.repo.commit_file("a.txt", "a\n", "a");
    let head = f.repo.head();
    let done = merge(f.write.clone(), params(&f, &first, FastForward::Allow))
        .await
        .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::UpToDate);
    assert_eq!(done.moved, None);
    assert_eq!(f.repo.head(), head);
}

/// git 이 스스로 거절하는 것은 원문 그대로 실패다(결정 M2-D6 ④) — 진행 중 작업이 생기지 않았으니 멈춤이 아니다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ff_only_on_a_diverged_branch_fails_with_gits_message() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    feature_ahead(&f);
    let main = f.repo.commit_file("m.txt", "main\n", "main");
    let result = merge(f.write.clone(), params(&f, "feature", FastForward::Only)).await;
    let e = result.unwrap_err();
    assert_eq!(e.code, crate::write::code::GIT_FAILED, "{e:?}");
    let stderr = e.data.as_ref().unwrap()["stderr"].as_str().unwrap();
    assert!(!stderr.trim().is_empty());
    assert_eq!(f.repo.head(), main);
}

/// 시작 전부터 진행 중 작업이 있으면 git 이 거절한다 — 그 작업을 이 쓰기가 멈춘 것으로 보이지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_merge_during_another_operation_fails_instead_of_reporting_it_as_stopped() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let d = f.repo.start_conflicting_merge();
    let result = merge(f.write.clone(), params(&f, &d.theirs, FastForward::Allow)).await;
    assert_eq!(
        result.unwrap_err().code,
        crate::write::code::GIT_FAILED,
        "git refuses: the merge in progress is not this write's"
    );
}
