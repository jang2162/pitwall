//! 푸시 다이얼로그(M2-20)가 보내는 값 그대로의 push: 계획한 브랜치(`branch`)를 싣고 보내면, 계획 뒤에 터미널에서 다른 브랜치로
//! 체크아웃해도 계획한 브랜치가 계획한 대상 · lease 로 간다 — 지금 HEAD 의 브랜치가 아니다(M2-20 리뷰 F1: `branch` 없이 보내면
//! 같은 upstream 의 다른 브랜치가 원격 main 을 덮었다).

use super::super::*;
use super::with_remote;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_planned_branch_goes_even_after_a_checkout_to_another_branch() {
    let (f, remote) = with_remote();
    let c0 = f.repo.head();
    let c1 = f.repo.commit_file("c1.txt", "c1\n", "c1 on main");
    f.repo
        .git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    f.repo.git(&["config", "push.default", "upstream"]);
    // B 는 C0 에서 갈라지고 같은 upstream(origin/main)을 따른다.
    f.repo.git(&["branch", "B", &c0]);
    f.repo.git(&["branch", "-u", "origin/main", "B"]);
    // main 에 보낼 커밋이 하나 더 있다 — 다이얼로그는 main 으로 계획했다(origin : refs/heads/main, lease C1).
    let c2 = f.repo.commit_file("c2.txt", "c2\n", "c2 on main");
    // 그 뒤 터미널에서 B 로 가서 커밋.
    f.repo.git(&["checkout", "--quiet", "B"]);
    let b = f.repo.commit_file("b.txt", "b\n", "b only");

    let pushed = push(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        Some("main".into()),
        None,
        None,
        false,
        Some(c1.clone()),
        Some(GitPushExpectedTarget {
            remote: "origin".into(),
            remote_ref: "refs/heads/main".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(pushed.plan.branch, "main");
    assert_eq!(pushed.plan.remote_ref, "refs/heads/main");
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(c2));
    assert_ne!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(b));
    assert_eq!(
        f.repo.rev_parse("HEAD"),
        f.repo.rev_parse("refs/heads/B"),
        "HEAD stayed on B"
    );
}
