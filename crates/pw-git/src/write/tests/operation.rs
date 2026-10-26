//! 진행 중 작업: merge 충돌 픽스처에서 `git.operation` 이 merge 이고 abort 뒤 HEAD · 작업 트리가 머지 전과 같다(③). rebase 의 브랜치
//! · 대상 · 단계와 건너뛰기 뒤 다음 충돌에서 다시 멈춤, 순차 cherry-pick 의 남은 단계, 충돌을 풀고 계속, 못 하는 동작.

use std::fs;

use super::super::*;
use crate::testing::write::WriteFixture;

/// 작업 트리 상태(`git status --porcelain` 과 추적 파일 내용).
fn snapshot(f: &WriteFixture) -> (String, String, Vec<(String, String)>) {
    let head = f.repo.head();
    let status = f
        .repo
        .git(&["status", "--porcelain=v1", "--untracked-files=all"]);
    let mut files: Vec<(String, String)> = f
        .repo
        .git(&["ls-files"])
        .lines()
        .map(|p| {
            (
                p.to_owned(),
                fs::read_to_string(f.repo.root().join(p)).unwrap(),
            )
        })
        .collect();
    files.sort();
    (head, status, files)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_merge_conflict_is_a_merge_and_abort_restores_head_and_work_tree() {
    let f = WriteFixture::new(true);
    let d = f.repo.diverge();
    let before = snapshot(&f);
    let out = f.repo.try_git_in(f.repo.root(), &["merge", &d.theirs]);
    assert!(!out.status.success(), "the fixture merge conflicts");

    let info = f
        .write
        .operation(&f.project, None)
        .await
        .unwrap()
        .expect("a merge is in progress");
    assert_eq!(info.kind, RepoOperation::Merge);
    assert_eq!(info.heads, std::slice::from_ref(&d.theirs_commit));
    assert_eq!(
        info.actions,
        [OperationAction::Continue, OperationAction::Abort]
    );
    assert_eq!((info.branch, info.step, info.remaining), (None, None, None));

    let done = f
        .write
        .act(&f.project, None, OperationAction::Abort)
        .await
        .unwrap();
    assert_eq!(done.operation, None);
    assert!(!done.stopped);
    assert_eq!(
        snapshot(&f),
        before,
        "HEAD and the work tree are as before the merge"
    );
    assert_eq!(f.write.operation(&f.project, None).await, Ok(None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continue_fails_with_gits_message_until_the_conflict_is_resolved_then_commits() {
    let f = WriteFixture::new(true);
    let d = f.repo.start_conflicting_merge();
    let result = f
        .write
        .act(&f.project, None, OperationAction::Continue)
        .await;
    let Err(WriteError::Failed { stderr, stdout, .. }) = &result else {
        panic!("expected Failed, got {result:?}");
    };
    assert!(!(stderr.trim().is_empty() && stdout.trim().is_empty()));
    assert_eq!(
        f.write
            .operation(&f.project, None)
            .await
            .unwrap()
            .map(|i| i.kind),
        Some(RepoOperation::Merge),
        "still merging"
    );

    f.repo.write(&d.path, "resolved\n");
    f.repo.git(&["add", &d.path]);
    let done = f
        .write
        .act(&f.project, None, OperationAction::Continue)
        .await
        .unwrap();
    assert_eq!(done.operation, None);
    let parents = f.repo.git(&["rev-list", "--parents", "-n1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "a merge commit");
}

/// `other` 의 세 커밋(o1 · o2 가 `f` 를, o3 가 `h` 를 바꾼다)을 `f` 를 바꾼 `main` 위로 rebase 해 o1 에서 멈춘다.
fn start_rebase(f: &WriteFixture) -> (String, String, [String; 3]) {
    let r = &f.repo;
    r.commit_file("f", "base\n", "base");
    r.git(&["checkout", "-q", "-b", "other"]);
    let o1 = r.commit_file("f", "o1\n", "o1");
    let o2 = r.commit_file("f", "o2\n", "o2");
    let o3 = r.commit_file("h", "o3\n", "o3");
    r.checkout("main");
    let main = r.commit_file("f", "main\n", "m");
    r.checkout("other");
    let out = r.try_git_in(r.root(), &["rebase", "main"]);
    assert!(!out.status.success(), "the fixture rebase stops on o1");
    (main, o3.clone(), [o1, o2, o3])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebase_reports_branch_onto_and_step_and_skip_stops_again_on_the_next_conflict() {
    let f = WriteFixture::new(true);
    let (main, tip, [o1, o2, _]) = start_rebase(&f);
    let info = f.write.operation(&f.project, None).await.unwrap().unwrap();
    assert_eq!(info.kind, RepoOperation::Rebase);
    assert_eq!(info.branch.as_deref(), Some("other"));
    assert_eq!(info.onto.as_deref(), Some(main.as_str()));
    assert_eq!(
        info.step,
        Some(OperationStep {
            current: 1,
            total: 3
        })
    );
    assert_eq!(info.remaining, Some(2));
    assert_eq!(info.heads, [o1]);
    assert_eq!(
        info.actions,
        [
            OperationAction::Continue,
            OperationAction::Abort,
            OperationAction::Skip
        ]
    );

    let skipped = f
        .write
        .act(&f.project, None, OperationAction::Skip)
        .await
        .unwrap();
    assert!(skipped.stopped, "o2 conflicts too — stopped, not failed");
    let next = skipped.operation.unwrap();
    assert_eq!(
        next.step,
        Some(OperationStep {
            current: 2,
            total: 3
        })
    );
    assert_eq!(next.heads, [o2]);

    let aborted = f
        .write
        .act(&f.project, None, OperationAction::Abort)
        .await
        .unwrap();
    assert_eq!(aborted.operation, None);
    assert_eq!(
        f.repo.head(),
        tip,
        "back on the branch tip before the rebase"
    );
    assert_eq!(
        f.repo.git(&["symbolic-ref", "--short", "HEAD"]).trim(),
        "other"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cherry_pick_sequence_counts_the_steps_left() {
    let f = WriteFixture::new(true);
    let (_, _, [o1, o2, o3]) = start_rebase(&f);
    f.repo.git(&["rebase", "--abort"]);
    f.repo.checkout("main");
    let out = f
        .repo
        .try_git_in(f.repo.root(), &["cherry-pick", &o1, &o2, &o3]);
    assert!(!out.status.success());
    let info = f.write.operation(&f.project, None).await.unwrap().unwrap();
    assert_eq!(info.kind, RepoOperation::CherryPick);
    assert_eq!(info.heads, [o1]);
    assert_eq!(info.remaining, Some(2), "o2 and o3 are left after o1");
    assert_eq!(info.step, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actions_the_operation_does_not_have_are_refused() {
    let f = WriteFixture::new(true);
    assert_eq!(
        f.write
            .act(&f.project, None, OperationAction::Continue)
            .await,
        Err(WriteError::NoOperation)
    );
    f.repo.start_conflicting_merge();
    let skip = f.write.act(&f.project, None, OperationAction::Skip).await;
    assert_eq!(
        skip,
        Err(WriteError::Unsupported {
            kind: RepoOperation::Merge,
            action: OperationAction::Skip
        })
    );
    assert_eq!(
        pw_rpc::RpcError::from(skip.unwrap_err()).code,
        code::NO_OPERATION
    );
    f.repo.git(&["merge", "--abort"]);
    f.repo.start_bisect();
    let info = f.write.operation(&f.project, None).await.unwrap().unwrap();
    assert_eq!(info.kind, RepoOperation::Bisect);
    assert!(info.actions.is_empty());
    assert!(matches!(
        f.write.act(&f.project, None, OperationAction::Abort).await,
        Err(WriteError::Unsupported { .. })
    ));
}

/// 상태 파일은 일반 파일 · 1 MiB 이하만 읽는다 — 받은 저장소가 그것을 링크(저장소 밖 파일 · `/dev/zero`)나 거대한 파일로 두어도
/// 따라가거나 멈추지 않고 그 값만 모른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_files_that_are_links_or_huge_are_not_read() {
    let f = WriteFixture::new(true);
    start_rebase(&f);
    let state = f.repo.root().join(".git/rebase-merge");
    let elsewhere = f.repo.base().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("head-name"), "refs/heads/elsewhere\n").unwrap();
    fs::write(elsewhere.join("msgnum"), "1\n").unwrap();
    for name in ["head-name", "msgnum"] {
        fs::remove_file(state.join(name)).unwrap();
        std::os::unix::fs::symlink(elsewhere.join(name), state.join(name)).unwrap();
    }
    fs::remove_file(state.join("end")).unwrap();
    std::os::unix::fs::symlink("/dev/zero", state.join("end")).unwrap();
    let huge = "0".repeat(usize::try_from(operation::MAX_STATE_FILE).unwrap() + 1);
    fs::write(state.join("onto"), huge).unwrap();
    let info = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        f.write.operation(&f.project, None),
    )
    .await
    .expect("reading a /dev/zero link must not hang")
    .unwrap()
    .unwrap();
    assert_eq!(info.kind, RepoOperation::Rebase);
    assert_eq!((info.branch, info.onto, info.step), (None, None, None));
}
