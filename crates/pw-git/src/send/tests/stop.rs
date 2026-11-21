//! 멈춤(② ③ ⑨): 어느 갈래든 대상 브랜치 ref · HEAD · 작업 트리가 처음과 같고 받은 커밋은 전용 ref 에 남는다.

use super::super::apply::{GitSendOutcome, GitSendStop};
use super::*;

fn reason(outcome: &GitSendOutcome) -> GitSendStop {
    match outcome {
        GitSendOutcome::Stopped { reason, .. } => *reason,
        other => panic!("expected a stop, got {other:?}"),
    }
}

fn message(outcome: &GitSendOutcome) -> String {
    match outcome {
        GitSendOutcome::Stopped { message, .. } => message.clone().unwrap_or_default(),
        other => panic!("expected a stop, got {other:?}"),
    }
}

/// ② 갈라짐: 대상 브랜치에 원본에 없는 커밋이 있으면 멈춘다.
#[tokio::test]
async fn a_diverged_branch_stops_and_changes_nothing() {
    let pair = Pair::new(true);
    pair.dst
        .repo
        .commit_file("theirs.txt", "only on the target\n", "theirs");
    let commit = pair
        .src
        .repo
        .commit_file("ours.txt", "only on the source\n", "ours");
    let before = snapshot(&pair.dst.repo);

    let outcome = pair.send("main", &commit).await;

    assert_eq!(reason(&outcome), GitSendStop::Diverged);
    assert_eq!(snapshot(&pair.dst.repo), before);
    assert_eq!(incoming(&pair.dst.repo, "main"), Some(commit));
}

/// ② 갈라짐(체크아웃되지 않은 브랜치): `update-ref` 는 fast-forward 를 보지 않으므로 엔진이 먼저 멈춰야 한다.
#[tokio::test]
async fn a_diverged_branch_that_is_not_checked_out_stops_too() {
    let pair = Pair::new(true);
    let dst = &pair.dst.repo;
    dst.git(&["switch", "--quiet", "-c", "feat"]);
    dst.commit_file("theirs.txt", "only on the target\n", "theirs");
    dst.git(&["switch", "--quiet", "main"]);
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair.src.repo.commit_file("ours.txt", "ours\n", "ours");
    let before = snapshot(dst);

    let outcome = pair.send("feat", &commit).await;

    assert_eq!(reason(&outcome), GitSendStop::Diverged);
    assert_eq!(snapshot(dst), before);
}

/// ② 막는 변경(체크아웃된 브랜치): 커밋 안 한 변경이 fast-forward 를 막으면 git 이 거절하고 그 원문이 결과에 있다.
#[tokio::test]
async fn local_changes_blocking_the_fast_forward_stop_with_git_text() {
    let pair = Pair::new(true);
    pair.dst.repo.write("README.md", "edited on the target\n");
    let commit = pair
        .src
        .repo
        .commit_file("README.md", "edited on the source\n", "readme");
    let before = snapshot(&pair.dst.repo);

    let outcome = pair.send("main", &commit).await;

    assert_eq!(reason(&outcome), GitSendStop::Blocked);
    assert!(
        message(&outcome).contains("README.md"),
        "{}",
        message(&outcome)
    );
    assert_eq!(snapshot(&pair.dst.repo), before);
}

/// ⑨ 막는 변경(전환): `switch` 가 실패하면 움직인 브랜치를 `old` 로 되돌린다.
#[tokio::test]
async fn a_failed_switch_puts_the_branch_back() {
    let pair = Pair::new(true);
    let dst = &pair.dst.repo;
    dst.branch("feat");
    let old = dst.head();
    dst.write("README.md", "edited on the target\n");
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair
        .src
        .repo
        .commit_file("README.md", "edited on the source\n", "readme");
    let before = snapshot(dst);

    let outcome = pair.send("feat", &commit).await;

    assert_eq!(reason(&outcome), GitSendStop::Blocked);
    assert!(
        message(&outcome).contains("README.md"),
        "{}",
        message(&outcome)
    );
    assert_eq!(ref_of(dst, "refs/heads/feat"), Some(old));
    assert_eq!(snapshot(dst), before);
}

/// ⑨ 만들었던 브랜치는 `switch` 가 실패하면 지운다.
#[tokio::test]
async fn a_failed_switch_removes_a_branch_it_created() {
    let pair = Pair::new(true);
    let dst = &pair.dst.repo;
    dst.write("README.md", "edited on the target\n");
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair
        .src
        .repo
        .commit_file("README.md", "edited on the source\n", "readme");
    let before = snapshot(dst);

    let outcome = pair.send("feat", &commit).await;

    assert_eq!(reason(&outcome), GitSendStop::Blocked);
    assert_eq!(ref_of(dst, "refs/heads/feat"), None);
    assert_eq!(snapshot(dst), before);
}

/// ② 그 브랜치를 이 머신의 다른 워크트리가 쓰면 멈춘다.
#[tokio::test]
async fn a_branch_used_by_another_worktree_stops() {
    let pair = Pair::new(true);
    let dst = &pair.dst.repo;
    let other = dst.add_worktree("other", "feat");
    let old = dst.rev_parse("feat");
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair.src.repo.commit_file("feat.txt", "feat\n", "feat");
    let before = snapshot(dst);
    let other_before = dst.git_in(&other, &["status", "--porcelain=v1"]);

    let outcome = pair.send("feat", &commit).await;

    assert_eq!(reason(&outcome), GitSendStop::InOtherWorktree);
    assert_eq!(ref_of(dst, "refs/heads/feat"), Some(old));
    assert_eq!(snapshot(dst), before);
    assert_eq!(
        dst.git_in(&other, &["status", "--porcelain=v1"]),
        other_before
    );
}

/// ③ `expected` 와 다른 브랜치는 움직이지 않는다(CAS) — 브랜치가 그사이 움직였거나, 없다고 했는데 있다.
#[tokio::test]
async fn a_branch_that_is_not_expected_is_not_moved() {
    let pair = Pair::new(true);
    let dst = &pair.dst.repo;
    dst.branch("feat");
    let old = dst.head();
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair.src.repo.commit_file("feat.txt", "feat\n", "feat");
    let before = snapshot(dst);

    // 브랜치 끝이 아닌 값 · 「없음」 둘 다.
    let outcome = pair
        .send_expecting("feat", &commit, Some(commit.clone()))
        .await;
    assert_eq!(reason(&outcome), GitSendStop::BranchMoved);
    let outcome = pair.apply("feat", &commit, None).await.unwrap();
    assert_eq!(reason(&outcome), GitSendStop::BranchMoved);
    assert_eq!(ref_of(dst, "refs/heads/feat"), Some(old));
    assert_eq!(snapshot(dst), before);
}

/// 전용 ref 가 알린 커밋이 아니면(받지 않았거나 다른 보내기가 덮었다) 맞추지 않는다.
#[tokio::test]
async fn an_incoming_ref_at_another_commit_is_not_applied() {
    let pair = Pair::new(true);
    let first = pair.src.repo.commit_file("a.txt", "a\n", "a");
    let second = pair.src.repo.commit_file("b.txt", "b\n", "b");
    let prepared = pair.prepare("main", &first).await;
    pair.receive_file(&prepared.file, "main", &first)
        .await
        .unwrap();
    let before = snapshot(&pair.dst.repo);
    let tip = pair.theirs("main").await.branch_tip;

    let outcome = pair.apply("main", &second, tip).await.unwrap();

    assert_eq!(reason(&outcome), GitSendStop::IncomingMoved);
    assert_eq!(snapshot(&pair.dst.repo), before);
}
