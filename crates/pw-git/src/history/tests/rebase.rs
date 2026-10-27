//! 비대화형 rebase: 끝나면 그 브랜치의 이전 끝이 `moved`, 다른 브랜치를 이름으로 rebase, 충돌이면 진행 중 rebase 와 중단,
//! 이미 위에 있으면 up to date.

use super::super::methods::{GitRebaseParams, rebase};
use super::super::{GitRefMove, HistoryOutcome};
use super::{parents, window};
use crate::methods::RepoOperation;
use crate::testing::write::WriteFixture;
use crate::write::OperationAction;

fn params(f: &WriteFixture, upstream: &str, branch: Option<&str>) -> GitRebaseParams {
    GitRebaseParams {
        project_id: f.project.clone(),
        worktree: None,
        upstream: upstream.to_owned(),
        branch: branch.map(str::to_owned),
    }
}

/// `other` 에 `h.txt` 를 더한 커밋, `main` 에 `f.txt` 를 더한 커밋. `main` 에 있다. (main 끝, other 끝).
fn diverged(f: &WriteFixture) -> (String, String) {
    let r = &f.repo;
    r.git(&["checkout", "-q", "-b", "other"]);
    let other = r.commit_file("h.txt", "h\n", "o1");
    r.checkout("main");
    let main = r.commit_file("f.txt", "f\n", "m");
    (main, other)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebase_rewrites_the_branch_onto_upstream_and_reports_its_old_tip() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (main, other) = diverged(&f);
    f.repo.checkout("other");
    let done = rebase(f.write.clone(), f.caller(), params(&f, "main", None))
        .await
        .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::Done);
    let head = f.repo.head();
    assert_eq!(parents(&f, "HEAD"), [main]);
    assert_eq!(
        done.moved,
        Some(GitRefMove {
            ref_name: "refs/heads/other".into(),
            previous: Some(other),
            current: Some(head),
        })
    );
    assert_eq!(done.operation, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_named_branch_is_checked_out_and_rebased() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (main, other) = diverged(&f);
    let done = rebase(
        f.write.clone(),
        f.caller(),
        params(&f, "main", Some("other")),
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::Done);
    assert_eq!(
        f.repo.git(&["symbolic-ref", "--short", "HEAD"]).trim(),
        "other"
    );
    assert_eq!(parents(&f, "other"), [main]);
    let moved = done.moved.unwrap();
    assert_eq!(moved.ref_name, "refs/heads/other");
    assert_eq!(moved.previous, Some(other));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflict_stops_with_the_rebase_in_progress_and_abort_restores_the_branch() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let d = f.repo.diverge();
    let tip = f.repo.head();
    let stopped = rebase(f.write.clone(), f.caller(), params(&f, &d.theirs, None))
        .await
        .unwrap();
    assert_eq!(stopped.outcome, HistoryOutcome::Stopped);
    assert_eq!(stopped.moved, None);
    let operation = stopped.operation.unwrap();
    assert_eq!(operation.kind, RepoOperation::Rebase);
    assert_eq!(operation.branch.as_deref(), Some("main"));
    assert_eq!(operation.onto.as_deref(), Some(d.theirs_commit.as_str()));

    let aborted = f
        .write
        .act(f.caller(), &f.project, None, OperationAction::Abort)
        .await
        .unwrap();
    assert_eq!(aborted.operation, None);
    assert_eq!(aborted.moved, None, "abort is the undo");
    assert_eq!(f.repo.head(), tip);
    assert_eq!(
        f.repo.git(&["symbolic-ref", "--short", "HEAD"]).trim(),
        "main"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebasing_onto_an_ancestor_is_up_to_date() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let first = f.repo.head();
    let tip = f.repo.commit_file("a.txt", "a\n", "a");
    let done = rebase(f.write.clone(), f.caller(), params(&f, &first, None))
        .await
        .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::UpToDate);
    assert_eq!(done.moved, None);
    assert_eq!(f.repo.head(), tip);
}
