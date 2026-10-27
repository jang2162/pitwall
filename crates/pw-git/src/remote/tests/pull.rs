//! pull: fast-forward merge, pull --rebase 충돌이면 진행 중 작업이 rebase 이고 continue · abort 가 된다, merge 충돌, upstream 없는
//! pull 은 git 의 원문.

use std::fs;

use super::super::*;
use super::{proceed, with_remote};
use crate::methods::RepoOperation;
use crate::testing::write::WriteFixture;
use crate::write::OperationAction;

/// 원격 `main` 과 이 저장소가 `same.txt` 를 서로 다르게 고친다. 이 저장소는 원격의 것을 아직 모른다.
fn conflicting(f: &WriteFixture, remote: &crate::testing::remote::Remote) -> (String, String) {
    let other = f.repo.clone_remote(remote, "other");
    let theirs = f.repo.push_from(&other, "main", "same.txt", "theirs\n");
    let ours = f.repo.commit_file("same.txt", "ours\n", "ours");
    (theirs, ours)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pull_fast_forwards_to_the_upstream() {
    let (f, remote) = with_remote();
    let other = f.repo.clone_remote(&remote, "other");
    let theirs = f.repo.push_from(&other, "main", "new.txt", "new\n");
    let window = proceed(&f);
    let pulled = pull(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        GitPullMode::Merge,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        (pulled.operation, pulled.stopped),
        (None, false),
        "nothing in progress"
    );
    assert_eq!(f.repo.head(), theirs);
    assert_eq!(
        window.seen().len(),
        1,
        "a pull asks about the whole work tree first"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicting_pull_rebase_stops_in_a_rebase_that_continues() {
    let (f, remote) = with_remote();
    let (theirs, _) = conflicting(&f, &remote);
    let _window = proceed(&f);
    let pulled = pull(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        GitPullMode::Rebase,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(pulled.stopped);
    let info = pulled.operation.expect("a rebase is in progress");
    assert_eq!(info.kind, RepoOperation::Rebase);
    assert_eq!(info.onto.as_deref(), Some(theirs.as_str()));
    assert_eq!(
        f.write
            .operation(&f.project, None)
            .await
            .unwrap()
            .map(|o| o.kind),
        Some(RepoOperation::Rebase)
    );

    // 충돌을 풀고 계속한다(M2-06 의 `git.operation.continue`).
    fs::write(f.repo.root().join("same.txt"), "resolved\n").unwrap();
    f.repo.git(&["add", "same.txt"]);
    let done = f
        .write
        .act(f.caller(), &f.project, None, OperationAction::Continue)
        .await
        .unwrap();
    assert_eq!((done.operation, done.stopped), (None, false));
    assert_eq!(f.repo.rev_parse("HEAD~1"), theirs);
    assert_eq!(
        fs::read_to_string(f.repo.root().join("same.txt")).unwrap(),
        "resolved\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicting_pull_rebase_aborts_back_to_where_it_was() {
    let (f, remote) = with_remote();
    let (_, ours) = conflicting(&f, &remote);
    let _window = proceed(&f);
    let pulled = pull(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        GitPullMode::Rebase,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        pulled.operation.map(|o| o.kind),
        Some(RepoOperation::Rebase)
    );
    let done = f
        .write
        .act(f.caller(), &f.project, None, OperationAction::Abort)
        .await
        .unwrap();
    assert_eq!(done.operation, None);
    assert_eq!(f.repo.head(), ours);
    assert_eq!(
        f.repo.git(&["symbolic-ref", "--short", "HEAD"]).trim(),
        "main"
    );
    assert_eq!(
        fs::read_to_string(f.repo.root().join("same.txt")).unwrap(),
        "ours\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicting_pull_merge_stops_in_a_merge() {
    let (f, remote) = with_remote();
    let (theirs, _) = conflicting(&f, &remote);
    let _window = proceed(&f);
    let pulled = pull(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        GitPullMode::Merge,
        Some("origin".into()),
        Some("main".into()),
    )
    .await
    .unwrap();
    assert!(pulled.stopped);
    let info = pulled.operation.expect("a merge is in progress");
    assert_eq!(info.kind, RepoOperation::Merge);
    assert_eq!(info.heads, [theirs]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pull_without_an_upstream_shows_the_git_text() {
    let (f, _remote) = with_remote();
    f.repo.git(&["checkout", "--quiet", "-b", "lonely"]);
    let _window = proceed(&f);
    let e = pull(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        GitPullMode::Merge,
        None,
        None,
    )
    .await
    .unwrap_err();
    let WriteError::Failed { stderr, .. } = &e else {
        panic!("expected a git failure, got {e:?}");
    };
    assert!(!stderr.is_empty());
    assert_eq!(f.write.operation(&f.project, None).await.unwrap(), None);
    // 원격 없이 브랜치만 주면 git 을 띄우기 전에 거절한다.
    let e = pull(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        GitPullMode::Merge,
        None,
        Some("main".into()),
    )
    .await
    .unwrap_err();
    assert_eq!(RpcError::from(e).code, pw_rpc::code::INVALID_PARAMS);
}
