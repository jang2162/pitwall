//! [되돌리기](결정 M2-D6 ③): `moved` 로 `git.reset { previous, keep, expected: current, expectedRef: refName }` 를 부르면 원래대로다.
//! 같은 커밋의 다른 브랜치로 옮겨 갔으면 그 브랜치를 되돌리지 않고, 그사이 편집은 keep 이 지킨다. 충돌로 멈췄다가 continue 로
//! 끝난 rebase · merge 도 결과의 `moved` 로 되돌린다.

use pw_rpc::code as rpc_code;

use super::super::methods::{
    GitMergeParams, GitRebaseParams, GitResetParams, merge, rebase, reset,
};
use super::super::{FastForward, GitRefMove, HistoryOutcome, ResetMode};
use super::reset::read;
use super::window;
use crate::testing::write::WriteFixture;
use crate::write::OperationAction;

fn undo(f: &WriteFixture, moved: &GitRefMove) -> GitResetParams {
    GitResetParams {
        project_id: f.project.clone(),
        worktree: None,
        commit: moved.previous.clone().unwrap(),
        mode: ResetMode::Keep,
        expected: moved.current.clone(),
        expected_ref: Some(moved.ref_name.clone()),
        fingerprint: None,
    }
}

/// `main` 에 `other`(g.txt 를 더한다)를 merge 커밋으로 들인다. (merge 전 main, 결과의 moved).
async fn merged(f: &WriteFixture) -> (String, GitRefMove) {
    let r = &f.repo;
    r.git(&["checkout", "-q", "-b", "other"]);
    r.commit_file("g.txt", "g\n", "g");
    r.checkout("main");
    let before = r.commit_file("m.txt", "m\n", "m");
    let done = merge(
        f.write.clone(),
        GitMergeParams {
            project_id: f.project.clone(),
            worktree: None,
            rev: "other".into(),
            fast_forward: FastForward::Never,
        },
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, HistoryOutcome::Done);
    (before, done.moved.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undoing_a_merge_with_keep_brings_the_branch_back() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (before, moved) = merged(&f).await;
    let done = reset(f.write.clone(), undo(&f, &moved)).await.unwrap();
    assert_eq!(f.repo.head(), before);
    assert_eq!(read(&f, "g.txt"), None);
    assert_eq!(done.moved.unwrap().current, Some(before));
}

/// 같은 커밋에 만든 다른 브랜치로 옮겨 갔으면 HEAD 커밋은 같아도 그 브랜치를 되돌리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_branch_on_the_same_commit_is_not_undone() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (_, moved) = merged(&f).await;
    let merge_commit = f.repo.head();
    f.repo.git(&["switch", "-q", "-c", "another"]);
    let e = reset(f.write.clone(), undo(&f, &moved)).await.unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    assert_eq!(e.data.unwrap()["refName"], "refs/heads/another");
    assert_eq!(f.repo.rev_parse("another"), merge_commit);
    assert_eq!(f.repo.rev_parse("main"), merge_commit);
}

/// merge 뒤 들어온 파일을 고쳤으면 keep 이 거절한다(git 의 원문) — 편집은 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keep_refuses_to_undo_over_an_edit_made_since() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let (_, moved) = merged(&f).await;
    let merge_commit = f.repo.head();
    f.repo.write("g.txt", "edited since\n");
    let e = reset(f.write.clone(), undo(&f, &moved)).await.unwrap_err();
    assert_eq!(e.code, crate::write::code::GIT_FAILED);
    assert_eq!(f.repo.head(), merge_commit);
    assert_eq!(read(&f, "g.txt").as_deref(), Some("edited since\n"));
}

/// 충돌로 멈춘 rebase 를 continue 로 끝내면 결과의 `moved` 가 rebase 전 끝(`rebase-merge/orig-head`)부터다 — 그것으로 되돌린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebase_finished_by_continue_can_be_undone() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let d = f.repo.diverge();
    let tip = f.repo.head();
    let stopped = rebase(
        f.write.clone(),
        GitRebaseParams {
            project_id: f.project.clone(),
            worktree: None,
            upstream: d.theirs.clone(),
            branch: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(stopped.outcome, HistoryOutcome::Stopped);
    f.repo.write(&d.path, "resolved\n");
    f.repo.git(&["add", &d.path]);
    let done = f
        .write
        .act(&f.project, None, OperationAction::Continue)
        .await
        .unwrap();
    assert_eq!(done.operation, None);
    let moved = done.moved.unwrap();
    assert_eq!(
        moved,
        GitRefMove {
            ref_name: "refs/heads/main".into(),
            previous: Some(tip.clone()),
            current: Some(f.repo.head()),
        }
    );
    reset(f.write.clone(), undo(&f, &moved)).await.unwrap();
    assert_eq!(f.repo.head(), tip);
    assert_eq!(read(&f, &d.path).as_deref(), Some("ours\n"));
}

/// 충돌로 멈춘 merge 를 continue 로 끝내면 `moved` 가 merge 전 HEAD 부터다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_merge_finished_by_continue_records_the_move() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let d = f.repo.start_conflicting_merge();
    let tip = f.repo.head();
    f.repo.write(&d.path, "resolved\n");
    f.repo.git(&["add", &d.path]);
    let done = f
        .write
        .act(&f.project, None, OperationAction::Continue)
        .await
        .unwrap();
    assert_eq!(done.operation, None);
    assert_eq!(
        done.moved,
        Some(GitRefMove {
            ref_name: "refs/heads/main".into(),
            previous: Some(tip),
            current: Some(f.repo.head()),
        })
    );
}
