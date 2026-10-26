//! push: 새 브랜치 push 가 upstream 을 정한다, 앞선 원격으로의 일반 push 는 git 의 원문으로 거절된다, force 는
//! `--force-with-lease=<ref>:<마지막으로 본 원격 해시>` + `--force-if-includes` 뿐이고 원격이 그 사이 움직였으면 거절되어 원격 ref
//! 가 그대로다, 다른 꼴로 force 할 길이 없다(입력 · 설정 · 이름), 보낼 곳의 차례.

use super::super::methods::GitPushParams;
use super::super::*;
use super::with_remote;
use crate::testing::write::WriteFixture;
use crate::write::code as write_code;

async fn push_branch(
    f: &WriteFixture,
    branch: Option<&str>,
    set_upstream: bool,
    lease: Option<String>,
) -> Result<Pushed, WriteError> {
    push(
        &f.write,
        f.project.clone(),
        None,
        branch.map(str::to_owned),
        None,
        set_upstream,
        lease,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pushing_a_new_branch_sets_its_upstream() {
    let (f, remote) = with_remote();
    f.repo.git(&["checkout", "--quiet", "-b", "feat"]);
    let tip = f.repo.commit_file("feat.txt", "feat\n", "feat");
    assert_eq!(f.repo.upstream_of("feat"), None);

    let pushed = push_branch(&f, None, true, None).await.unwrap();
    assert_eq!(pushed.work_tree, f.repo.root().display().to_string());
    assert_eq!(
        pushed.plan,
        PushPlan {
            branch: "feat".into(),
            remote: "origin".into(),
            remote_ref: "refs/heads/feat".into(),
        }
    );
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/feat"), Some(tip));
    assert_eq!(f.repo.upstream_of("feat").as_deref(), Some("origin/feat"));
}

/// 원격 `main` 을 다른 클론이 앞으로 움직이고, 이 저장소는 그것을 모른 채(`origin/main` 이 옛것) 따로 커밋한다.
fn diverged(f: &WriteFixture, remote: &crate::testing::remote::Remote) -> (String, String, String) {
    let seen = f.repo.rev_parse("refs/remotes/origin/main");
    let other = f.repo.clone_remote(remote, "other");
    let theirs = f.repo.push_from(&other, "main", "theirs.txt", "theirs\n");
    let ours = f.repo.commit_file("ours.txt", "ours\n", "ours");
    (seen, theirs, ours)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_push_to_a_remote_that_moved_shows_the_git_rejection() {
    let (f, remote) = with_remote();
    let (_, theirs, _) = diverged(&f, &remote);
    let e = push_branch(&f, None, false, None).await.unwrap_err();
    let WriteError::Failed { stderr, .. } = &e else {
        panic!("expected a git failure, got {e:?}");
    };
    assert!(stderr.contains("rejected"), "{stderr}");
    assert_eq!(RpcError::from(e).code, write_code::GIT_FAILED);
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(theirs));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_force_push_is_refused_when_the_remote_moved_since_it_was_seen() {
    let (f, remote) = with_remote();
    let (seen, theirs, ours) = diverged(&f, &remote);
    // 창이 마지막으로 본 원격 해시는 `seen` 이다 — 원격은 그 사이 `theirs` 로 움직였다.
    let e = push_branch(&f, None, false, Some(seen)).await.unwrap_err();
    let WriteError::Failed { stderr, .. } = &e else {
        panic!("expected a git failure, got {e:?}");
    };
    assert!(stderr.contains("stale info"), "{stderr}");
    assert_eq!(
        f.repo.remote_ref(&remote, "refs/heads/main"),
        Some(theirs.clone()),
        "the remote ref is unchanged"
    );

    // 지금 원격을 본 뒤라면 덮는다.
    push_branch(&f, None, false, Some(theirs)).await.unwrap();
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(ours));
}

#[test]
fn the_lease_is_the_only_force() {
    let plan = PushPlan {
        branch: "+main".into(),
        remote: "origin".into(),
        remote_ref: "refs/heads/main".into(),
    };
    let hash = "0".repeat(40);
    assert_eq!(
        push_args(&plan, false, None),
        [
            "push",
            "--end-of-options",
            "origin",
            "refs/heads/+main:refs/heads/main"
        ]
    );
    assert_eq!(
        push_args(&plan, true, Some(&hash)),
        [
            "push".to_owned(),
            "--set-upstream".into(),
            format!("--force-with-lease=refs/heads/main:{hash}"),
            "--force-if-includes".into(),
            "--end-of-options".into(),
            "origin".into(),
            "refs/heads/+main:refs/heads/main".into(),
        ]
    );
    for lease in [None, Some(hash.as_str())] {
        for arg in push_args(&plan, true, lease) {
            assert!(
                !matches!(
                    arg.as_str(),
                    "--force" | "-f" | "--mirror" | "--all" | "--delete" | "-d"
                ) && !arg.starts_with('+'),
                "{arg}"
            );
        }
    }
    // 입력에 `force` 가 없다 — 모르는 필드는 받지 않는다.
    let wire = |extra: serde_json::Value| {
        let mut params = serde_json::json!({"projectId": "p"});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value::<GitPushParams>(params)
    };
    assert!(wire(serde_json::json!({})).is_ok());
    assert!(wire(serde_json::json!({"forceWithLease": hash})).is_ok());
    for extra in [
        serde_json::json!({"force": true}),
        serde_json::json!({"forcePush": true}),
        serde_json::json!({"refspec": "+main:main"}),
    ] {
        assert!(wire(extra.clone()).is_err(), "{extra}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_other_path_forces_a_push() {
    let (f, remote) = with_remote();
    let (seen, theirs, _) = diverged(&f, &remote);
    // 설정의 강제 push refspec 과 matching 은 쓰이지 않는다 — 보낼 곳은 이 모듈이 정한 `refs/heads/main:refs/heads/main` 이다.
    f.repo
        .git(&["config", "remote.origin.push", "+refs/heads/*:refs/heads/*"]);
    f.repo.git(&["config", "push.default", "matching"]);
    assert!(push_branch(&f, None, false, None).await.is_err());
    assert_eq!(
        f.repo.remote_ref(&remote, "refs/heads/main"),
        Some(theirs.clone())
    );

    // `+` 로 시작하는 이름도 refspec 의 강제 표시가 되지 않는다(언제나 `refs/heads/` 뒤에 온다).
    assert!(push_branch(&f, Some("+main"), false, None).await.is_err());
    assert_eq!(
        f.repo.remote_ref(&remote, "refs/heads/main"),
        Some(theirs.clone())
    );

    // 기대 해시는 전체 16진 해시만 — 짧은 해시 · 빈 값(「원격에 없어야」) · 리비전 이름은 git 을 띄우기 전에 거절한다.
    for lease in [&seen[..7], "", "HEAD", "origin/main", &format!("{seen}x")] {
        let e = push_branch(&f, None, false, Some(lease.to_owned()))
            .await
            .unwrap_err();
        assert_eq!(
            RpcError::from(e).code,
            pw_rpc::code::INVALID_PARAMS,
            "{lease}"
        );
    }
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(theirs));
}

#[test]
fn the_push_target_follows_the_upstream_and_push_remote() {
    let repo = crate::testing::TestRepo::with_commit();
    repo.git(&["remote", "add", "origin", "file:///nowhere/origin.git"]);
    repo.git(&["remote", "add", "fork", "file:///nowhere/fork.git"]);
    let plan = |branch: Option<&str>, remote: Option<&str>| {
        let opened = crate::repo::discover(repo.root(), true).unwrap().unwrap();
        push_plan(&opened, branch, remote)
    };
    // upstream 이 없으면 같은 이름을 `origin` 으로.
    assert_eq!(
        plan(None, None).unwrap(),
        PushPlan {
            branch: "main".into(),
            remote: "origin".into(),
            remote_ref: "refs/heads/main".into()
        }
    );
    // upstream 이 다른 이름이면 그 원격의 그 이름으로.
    repo.git(&["config", "branch.main.remote", "origin"]);
    repo.git(&["config", "branch.main.merge", "refs/heads/trunk"]);
    assert_eq!(plan(None, None).unwrap().remote_ref, "refs/heads/trunk");
    // 다른 원격으로 보내면 같은 이름으로.
    let to_fork = plan(None, Some("fork")).unwrap();
    assert_eq!(
        (to_fork.remote.as_str(), to_fork.remote_ref.as_str()),
        ("fork", "refs/heads/main")
    );
    // `remote.pushDefault` · `branch.<b>.pushRemote` 차례.
    repo.git(&["config", "remote.pushDefault", "fork"]);
    assert_eq!(plan(None, None).unwrap().remote, "fork");
    repo.git(&["config", "branch.main.pushRemote", "origin"]);
    assert_eq!(plan(None, None).unwrap().remote, "origin");
    // 분리된 HEAD 는 브랜치를 골라야 한다.
    repo.detach();
    assert!(plan(None, None).is_err());
    assert_eq!(plan(Some("main"), None).unwrap().branch, "main");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_result_names_the_refspec_sent_and_a_configured_push_that_differs() {
    let (f, remote) = with_remote();
    let tip = f.repo.commit_file("a.txt", "a\n", "a");
    // 설정이 `main` 을 다른 곳으로 보내라고 해도 이 push 는 upstream(`refs/heads/main`)으로 간다 — 결과가 그 차이를 알린다.
    f.repo.git(&[
        "config",
        "remote.origin.push",
        "refs/heads/main:refs/heads/elsewhere",
    ]);
    let pushed = push_branch(&f, None, false, None).await.unwrap();
    assert_eq!(pushed.plan.refspec(), "refs/heads/main:refs/heads/main");
    assert_eq!(
        pushed.configured,
        Some(GitPushConfigured {
            refspecs: vec!["refs/heads/main:refs/heads/elsewhere".into()],
            destination: Some("refs/heads/elsewhere".into()),
        })
    );
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(tip));
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/elsewhere"), None);
}

#[test]
fn a_configured_push_is_reported_only_when_it_goes_elsewhere() {
    let repo = crate::testing::TestRepo::with_commit();
    repo.git(&["remote", "add", "origin", "file:///nowhere/origin.git"]);
    let plan = PushPlan {
        branch: "main".into(),
        remote: "origin".into(),
        remote_ref: "refs/heads/main".into(),
    };
    let configured = || {
        let opened = crate::repo::discover(repo.root(), true).unwrap().unwrap();
        configured_push(&opened, &plan)
    };
    assert_eq!(configured(), None, "no push refspec");
    repo.git(&["config", "remote.origin.push", "refs/heads/*:refs/heads/*"]);
    assert_eq!(configured(), None, "the same place");
    repo.git(&["config", "--replace-all", "remote.origin.push", ":"]);
    assert_eq!(configured(), None, "matching pushes the same name");
    repo.git(&[
        "config",
        "--replace-all",
        "remote.origin.push",
        "refs/heads/other:refs/heads/other",
    ]);
    assert_eq!(
        configured(),
        Some(GitPushConfigured {
            refspecs: vec!["refs/heads/other:refs/heads/other".into()],
            destination: None,
        }),
        "the configured refspec does not push this branch"
    );
    repo.git(&[
        "config",
        "--replace-all",
        "remote.origin.push",
        "refs/heads/*:refs/heads/team/*",
    ]);
    assert_eq!(
        configured().and_then(|c| c.destination).as_deref(),
        Some("refs/heads/team/main")
    );
}
