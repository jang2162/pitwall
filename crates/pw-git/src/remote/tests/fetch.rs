//! fetch: 원격 전부 · 하나, prune 은 고를 때만, 작업 트리를 바꾸지 않아 창이 없어도 된다.

use super::super::*;
use super::with_remote;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_updates_remote_branches_and_prunes_only_when_asked() {
    let (f, remote) = with_remote();
    let other = f.repo.clone_remote(&remote, "other");
    f.repo
        .git_in(&other, &["checkout", "--quiet", "-b", "gone"]);
    let gone = f.repo.push_from(&other, "gone", "g.txt", "g\n");
    f.repo.git_in(&other, &["checkout", "--quiet", "main"]);
    let theirs = f.repo.push_from(&other, "main", "m.txt", "m\n");
    let head = f.repo.head();

    // 듣는 창이 없다 — fetch 는 작업 트리를 바꾸지 않으므로 묻지 않는다.
    let work_tree = fetch(&f.write, f.caller(), f.project.clone(), None, None, false)
        .await
        .unwrap();
    assert_eq!(work_tree, f.repo.root().display().to_string());
    assert_eq!(f.repo.rev_parse("refs/remotes/origin/main"), theirs);
    assert_eq!(f.repo.rev_parse("refs/remotes/origin/gone"), gone);
    assert_eq!(f.repo.head(), head, "fetch does not move HEAD");

    f.repo
        .git_in(&other, &["push", "--quiet", "origin", "--delete", "gone"]);
    fetch(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        Some("origin".into()),
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        f.repo.rev_parse("refs/remotes/origin/gone"),
        gone,
        "kept without prune"
    );
    fetch(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        Some("origin".into()),
        true,
    )
    .await
    .unwrap();
    assert!(
        !f.repo
            .try_git_in(
                f.repo.root(),
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "refs/remotes/origin/gone"
                ]
            )
            .status
            .success(),
        "pruned"
    );

    // 없는 원격은 git 의 원문이다.
    let e = fetch(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        Some("nowhere".into()),
        false,
    )
    .await
    .unwrap_err();
    assert!(matches!(e, WriteError::Failed { .. }), "{e:?}");
}
