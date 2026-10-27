//! 브랜치 생성(옮기지 않음 · 체크아웃) · 이름 바꾸기 · 삭제(병합 안 된 브랜치는 `force` 없이 거절, 이전 해시로 되살리기 — 어느
//! 워크트리가 쓰는 브랜치면 `-D` 도 안 되니 git 의 원문) · 태그 · `-` 로 시작하는 이름과 `git check-ref-format --branch` 가 받지
//! 않는 이름 거절.

use super::super::*;
use super::{proceed, stderr_of};
use crate::testing::write::WriteFixture;

/// 브랜치가 있는가(`refs/heads/<name>`).
fn exists(f: &WriteFixture, name: &str) -> bool {
    f.repo
        .try_git_in(
            f.repo.root(),
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .status
        .success()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_makes_a_branch_and_checks_it_out_only_when_asked() {
    let f = WriteFixture::new(true);
    let first = f.repo.head();
    let second = f.repo.commit_file("a.txt", "a\n", "second");

    let (branch, head) = create(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "plain".into(),
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(branch.ref_name, "refs/heads/plain");
    assert_eq!(branch.commit.as_deref(), Some(second.as_str()));
    assert_eq!(head, None);
    assert_eq!(
        f.repo.git(&["symbolic-ref", "--short", "HEAD"]).trim(),
        "main"
    );

    // 로그 우클릭의 「여기서 브랜치」: 시작 커밋을 주고 그리로 간다.
    let _window = proceed(&f);
    let (branch, head) = create(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "from-first".into(),
        Some(first.clone()),
        true,
    )
    .await
    .unwrap();
    assert_eq!(branch.commit.as_deref(), Some(first.as_str()));
    assert_eq!(
        head,
        Some(RepoHead::Branch {
            name: "from-first".into(),
            commit: first
        })
    );
    assert!(!f.repo.root().join("a.txt").exists());

    // 이미 있는 이름은 git 의 원문으로 실패한다.
    let e = create(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "plain".into(),
        None,
        false,
    )
    .await
    .unwrap_err();
    assert!(stderr_of(&e).contains("plain"), "{e:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_moves_the_branch_and_refuses_an_existing_name() {
    let f = WriteFixture::new(true);
    f.repo.branch("old");
    f.repo.branch("taken");
    let branch = rename(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "old".into(),
        "new".into(),
    )
    .await
    .unwrap();
    assert_eq!(branch.ref_name, "refs/heads/new");
    assert_eq!(branch.commit.as_deref(), Some(f.repo.head().as_str()));
    assert!(!exists(&f, "old") && exists(&f, "new"));

    let e = rename(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "new".into(),
        "taken".into(),
    )
    .await
    .unwrap_err();
    assert!(stderr_of(&e).contains("taken"), "{e:?}");
    assert!(exists(&f, "new"), "git -m does not overwrite");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unmerged_branch_is_not_deleted_without_force() {
    let f = WriteFixture::new(true);
    f.repo.git(&["checkout", "--quiet", "-b", "wip"]);
    let tip = f.repo.commit_file("wip.txt", "wip\n", "wip");
    f.repo.checkout("main");

    let refused = delete(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "wip".into(),
        false,
    )
    .await
    .unwrap();
    let Deleted::NotMerged { stderr, commit } = refused else {
        panic!("expected a not-merged refusal, got {refused:?}");
    };
    assert!(stderr.contains("wip"), "the git text: {stderr}");
    assert_eq!(commit.as_deref(), Some(tip.as_str()));
    assert!(exists(&f, "wip"), "the branch is still there");
    let e = Deleted::not_merged(stderr, commit);
    assert_eq!(e.code, code::NOT_MERGED);
    assert_eq!(e.data.as_ref().unwrap()["commit"], tip);

    // 창이 확인한 뒤 `force` 로.
    let deleted = delete(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "wip".into(),
        true,
    )
    .await
    .unwrap();
    let Deleted::Deleted { work_tree, moved } = deleted else {
        panic!("expected a deletion, got {deleted:?}");
    };
    assert_eq!(work_tree, f.repo.root().display().to_string());
    assert_eq!(
        moved,
        GitRefMove {
            ref_name: "refs/heads/wip".into(),
            previous: Some(tip.clone()),
            current: None,
        }
    );
    assert!(!exists(&f, "wip"));

    // [되돌리기]: 이전 해시에서 같은 이름으로 다시 만든다.
    let (branch, _) = create(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "wip".into(),
        moved.previous,
        false,
    )
    .await
    .unwrap();
    assert_eq!(branch.commit.as_deref(), Some(tip.as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_merged_branch_is_deleted_and_other_refusals_stay_git_failures() {
    let f = WriteFixture::new(true);
    f.repo.branch("done");
    let deleted = delete(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "done".into(),
        false,
    )
    .await
    .unwrap();
    assert!(matches!(deleted, Deleted::Deleted { .. }), "{deleted:?}");

    // 지금 브랜치는 git 이 지우지 않는다 — 병합 여부와 상관없는 거절이라 「병합 안 됨」 이 아니라 git 의 원문이다.
    let e = delete(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "main".into(),
        true,
    )
    .await
    .unwrap_err();
    assert!(stderr_of(&e).contains("main"), "{e:?}");
    // 없는 브랜치도.
    let e = delete(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "missing".into(),
        false,
    )
    .await
    .unwrap_err();
    assert!(stderr_of(&e).contains("missing"), "{e:?}");
}

/// 지우기가 git 의 원문(`2004`)으로 끝났다 — 「병합 안 됨」(`2101`)이 아니다.
async fn refused_by_git(f: &WriteFixture, name: &str) -> String {
    let e = delete(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        name.into(),
        false,
    )
    .await
    .map(|d| format!("{d:?}"))
    .unwrap_err();
    let text = stderr_of(&e);
    assert_eq!(RpcError::from(e).code, crate::write::code::GIT_FAILED);
    assert!(exists(f, name), "{name} is still there");
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unmerged_branch_a_worktree_uses_is_not_offered_a_force_delete() {
    let f = WriteFixture::new(true);
    f.repo.add_bare_remote("origin");
    // 다른 워크트리가 체크아웃한 병합 안 된 브랜치.
    let side = f.repo.add_worktree("side", "side");
    std::fs::write(side.join("side.txt"), "side\n").unwrap();
    f.repo.commit_all_in(&side, "side");
    let opened = crate::repo::discover(f.repo.root(), true).unwrap().unwrap();
    assert_eq!(merged(&opened, "side"), Some(false));
    let text = refused_by_git(&f, "side").await;
    assert!(text.contains("worktree"), "{text}");

    // 지금 브랜치가 upstream 보다 앞서 있다(upstream 기준 병합 안 됨).
    f.repo.commit_file("ahead.txt", "ahead\n", "ahead");
    let text = refused_by_git(&f, "main").await;
    assert!(text.contains("main"), "{text}");

    // 다른 워크트리가 rebase 하는 중인 브랜치(HEAD 는 분리돼 있다).
    let rebasing = f.repo.add_worktree("rebasing", "rebasing");
    f.repo.commit_file("c.txt", "main\n", "main side of c");
    std::fs::write(rebasing.join("c.txt"), "rebasing\n").unwrap();
    f.repo.commit_all_in(&rebasing, "rebasing side of c");
    let out = f.repo.try_git_in(&rebasing, &["rebase", "main"]);
    assert!(!out.status.success(), "the rebase stops on the conflict");
    let text = refused_by_git(&f, "rebasing").await;
    assert!(text.contains("rebasing"), "{text}");
}

#[test]
fn branch_names_follow_git_check_ref_format() {
    let repo = crate::testing::TestRepo::with_commit();
    for name in [
        "feat",
        "a/b",
        "+x",
        "@",
        "x@",
        "a@b",
        "ü",
        "a-b",
        "a.b",
        "*",
        "f*",
        "a:b",
        "a^b",
        "a~b",
        "a?b",
        "a[b",
        "a\\b",
        "a b",
        "a\tb",
        "a..b",
        ".a",
        "a.",
        "a/.b",
        "a.lock",
        "a/b.lock/c",
        "a/",
        "/a",
        "a//b",
        "a@{b",
        "HEAD",
        "refs/heads/x",
    ] {
        let git = repo
            .try_git_in(repo.root(), &["check-ref-format", "--branch", name])
            .status
            .success();
        assert_eq!(branch_name("branch", name).is_ok(), git, "{name:?}");
    }
    // 이전 브랜치 표기는 git 이 펼치지만 여기서는 이름이 아니다.
    assert!(branch_name("branch", "@{-1}").is_err());
}

#[test]
fn merged_follows_git_branch_d() {
    let repo = crate::testing::TestRepo::with_commit();
    // HEAD 에 든 브랜치, HEAD 에 없는 브랜치.
    repo.branch("in-head");
    repo.git(&["checkout", "--quiet", "-b", "ahead"]);
    repo.commit_file("x.txt", "x\n", "x");
    repo.checkout("main");
    // upstream 이 있으면 HEAD 가 아니라 upstream 이 기준이다: `ahead` 를 upstream 으로 둔 `tracks` 는 HEAD 에 없어도 병합됨,
    // 고아 브랜치 `side` 를 upstream 으로 둔 `in-head` 의 쌍둥이 `not-in-upstream` 은 HEAD 에 있어도 병합 안 됨.
    repo.git(&["branch", "tracks", "ahead"]);
    repo.git(&["branch", "--set-upstream-to=ahead", "tracks"]);
    repo.git(&["checkout", "--quiet", "--orphan", "side"]);
    repo.commit_file("side.txt", "side\n", "side");
    repo.checkout("main");
    repo.git(&["branch", "not-in-upstream", "main"]);
    repo.git(&["branch", "--set-upstream-to=side", "not-in-upstream"]);
    let opened = crate::repo::discover(repo.root(), true).unwrap().unwrap();
    assert_eq!(merged(&opened, "in-head"), Some(true));
    assert_eq!(merged(&opened, "ahead"), Some(false));
    assert_eq!(merged(&opened, "tracks"), Some(true));
    assert_eq!(merged(&opened, "not-in-upstream"), Some(false));
    assert_eq!(merged(&opened, "missing"), None);
    // git 과 같은 판정인지 git 자신으로 본다(`-d` 가 되는가).
    for (name, want) in [
        ("in-head", true),
        ("ahead", false),
        ("tracks", true),
        ("not-in-upstream", false),
    ] {
        let out = repo.try_git_in(repo.root(), &["branch", "-d", name]);
        assert_eq!(out.status.success(), want, "git branch -d {name}: {out:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tags_are_lightweight_or_annotated_with_the_message() {
    let f = WriteFixture::new(true);
    let first = f.repo.head();
    let second = f.repo.commit_file("a.txt", "a\n", "second");

    let light = tag(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "v1".into(),
        Some(first.clone()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(light.ref_name, "refs/tags/v1");
    assert_eq!(light.commit.as_deref(), Some(first.as_str()));
    assert_eq!(
        f.repo.git(&["cat-file", "-t", "refs/tags/v1"]).trim(),
        "commit"
    );

    let message = "-starts with a dash\n\n두 번째 줄\n";
    let annotated = tag(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "v2".into(),
        None,
        Some(message.into()),
    )
    .await
    .unwrap();
    assert_eq!(annotated.commit.as_deref(), Some(second.as_str()));
    assert_eq!(
        f.repo.git(&["cat-file", "-t", "refs/tags/v2"]).trim(),
        "tag"
    );
    assert_eq!(
        f.repo.git(&["tag", "--list", "--format=%(contents)", "v2"]),
        "-starts with a dash\n\n두 번째 줄\n\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn names_that_look_like_options_are_refused_before_git_runs() {
    let f = WriteFixture::new(true);
    let before = f.repo.git(&["for-each-ref"]);
    for e in [
        create(
            &f.write,
            f.caller(),
            f.project.clone(),
            None,
            "-D".into(),
            None,
            true,
        )
        .await
        .unwrap_err(),
        create(
            &f.write,
            f.caller(),
            f.project.clone(),
            None,
            "x".into(),
            Some("--orphan".into()),
            false,
        )
        .await
        .unwrap_err(),
        rename(
            &f.write,
            f.caller(),
            f.project.clone(),
            None,
            "main".into(),
            "-f".into(),
        )
        .await
        .unwrap_err(),
        delete(
            &f.write,
            f.caller(),
            f.project.clone(),
            None,
            "--all".into(),
            true,
        )
        .await
        .map(drop)
        .unwrap_err(),
        tag(
            &f.write,
            f.caller(),
            f.project.clone(),
            None,
            "-d".into(),
            None,
            None,
        )
        .await
        .unwrap_err(),
        checkout(
            &f.write,
            f.caller(),
            f.project.clone(),
            None,
            GitCheckoutTarget::Remote {
                remote: "origin".into(),
                branch: "x".into(),
                local: Some("-f".into()),
            },
        )
        .await
        .unwrap_err(),
    ] {
        assert_eq!(RpcError::from(e).code, pw_rpc::code::INVALID_PARAMS);
    }
    assert_eq!(f.repo.git(&["for-each-ref"]), before);
}
