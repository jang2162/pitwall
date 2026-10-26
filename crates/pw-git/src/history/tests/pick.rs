//! cherry-pick · revert: 커밋 2개 중 두 번째가 충돌하면 첫 번째는 커밋돼 있고 continue 로 끝난다, revert 가 역 커밋을 만든다.

use std::fs;

use super::super::methods::{GitCommitsParams, Pick, pick};
use super::super::{GitRefMove, HistoryOutcome};
use super::{parents, subject, window};
use crate::methods::RepoOperation;
use crate::testing::write::WriteFixture;
use crate::write::OperationAction;

fn params(f: &WriteFixture, commits: &[&str]) -> GitCommitsParams {
    GitCommitsParams {
        project_id: f.project.clone(),
        worktree: None,
        commits: commits.iter().map(|c| (*c).to_owned()).collect(),
    }
}

/// `other` 에 c1(`g.txt` 를 더한다) · c2(`f.txt` 를 고친다), `main` 에 `f.txt` 를 다르게 고친 커밋. `main` 에 있다. (c1, c2).
fn two_commits_the_second_conflicting(f: &WriteFixture) -> (String, String) {
    let r = &f.repo;
    r.commit_file("f.txt", "base\n", "base");
    r.git(&["checkout", "-q", "-b", "other"]);
    let c1 = r.commit_file("g.txt", "g\n", "c1");
    let c2 = r.commit_file("f.txt", "other\n", "c2");
    r.checkout("main");
    r.commit_file("f.txt", "main\n", "main");
    (c1, c2)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_the_second_of_two_picks_conflicts_the_first_is_committed_and_continue_finishes() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (c1, c2) = two_commits_the_second_conflicting(&f);
    let before = f.repo.head();
    let stopped = pick(f.write.clone(), params(&f, &[&c1, &c2]), Pick::CherryPick)
        .await
        .unwrap();
    assert_eq!(stopped.outcome, HistoryOutcome::Stopped);
    let operation = stopped.operation.unwrap();
    assert_eq!(operation.kind, RepoOperation::CherryPick);
    assert_eq!(operation.heads, std::slice::from_ref(&c2));
    assert_eq!(operation.remaining, Some(0), "c2 is the last");
    assert_eq!(subject(&f, "HEAD"), "c1", "the first pick is committed");
    assert_eq!(parents(&f, "HEAD"), [before]);
    assert_eq!(
        fs::read_to_string(f.repo.root().join("g.txt")).unwrap(),
        "g\n"
    );

    f.repo.write("f.txt", "resolved\n");
    f.repo.git(&["add", "f.txt"]);
    let done = f
        .write
        .act(&f.project, None, OperationAction::Continue)
        .await
        .unwrap();
    assert_eq!(done.operation, None);
    assert_eq!(subject(&f, "HEAD"), "c2");
    assert_eq!(subject(&f, "HEAD~1"), "c1");
    assert_eq!(
        fs::read_to_string(f.repo.root().join("f.txt")).unwrap(),
        "resolved\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn picks_without_conflicts_commit_in_the_given_order() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.git(&["checkout", "-q", "-b", "other"]);
    let a = r.commit_file("a.txt", "a\n", "a");
    let b = r.commit_file("b.txt", "b\n", "b");
    r.checkout("main");
    let before = r.head();
    let done = pick(f.write.clone(), params(&f, &[&b, &a]), Pick::CherryPick)
        .await
        .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::Done);
    assert_eq!(done.operation, None);
    assert_eq!(subject(&f, "HEAD"), "a");
    assert_eq!(subject(&f, "HEAD~1"), "b");
    let head = r.head();
    assert_eq!(
        done.moved,
        Some(GitRefMove {
            ref_name: "refs/heads/main".into(),
            previous: before,
            current: Some(head),
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revert_makes_the_inverse_commit() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    f.repo.commit_file("f.txt", "one\n", "one");
    let two = f.repo.commit_file("f.txt", "two\n", "two");
    let done = pick(f.write.clone(), params(&f, &[&two]), Pick::Revert)
        .await
        .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::Done);
    assert_eq!(parents(&f, "HEAD"), std::slice::from_ref(&two));
    assert_eq!(
        fs::read_to_string(f.repo.root().join("f.txt")).unwrap(),
        "one\n"
    );
    assert_eq!(
        f.repo.git(&["rev-parse", "HEAD^{tree}"]),
        f.repo.git(&["rev-parse", "HEAD~2^{tree}"]),
        "the tree is the one before the reverted commit"
    );
    assert!(subject(&f, "HEAD").starts_with("Revert \"two\""));
    assert_eq!(done.moved.unwrap().previous, two);
    assert!(
        f.repo.git(&["status", "--porcelain"]).is_empty(),
        "committed, nothing left in the work tree"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicting_revert_stops_as_a_revert() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    f.repo.commit_file("f.txt", "one\n", "one");
    let two = f.repo.commit_file("f.txt", "two\n", "two");
    f.repo.commit_file("f.txt", "three\n", "three");
    let stopped = pick(f.write.clone(), params(&f, &[&two]), Pick::Revert)
        .await
        .unwrap();
    assert_eq!(stopped.outcome, HistoryOutcome::Stopped);
    let operation = stopped.operation.unwrap();
    assert_eq!(operation.kind, RepoOperation::Revert);
    assert_eq!(operation.heads, [two]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_commits_is_invalid() {
    let f = WriteFixture::new(true);
    let e = pick(f.write.clone(), params(&f, &[]), Pick::CherryPick)
        .await
        .unwrap_err();
    assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS);
}
