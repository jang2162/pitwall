//! push: 새 브랜치 push 가 upstream 을 정한다, 앞선 원격으로의 일반 push 는 git 의 원문으로 거절된다, force 는
//! `--force-with-lease=<ref>:<마지막으로 본 원격 해시>` + `--force-if-includes` 뿐이고 원격이 그 사이 움직였으면 거절되어 원격 ref
//! 가 그대로다, 다른 꼴로 force 할 길이 없다(입력 · 설정 · 이름), git 이 거절할 대상이면 보내지 않는다(`2102`), 설정의 push
//! refspec 이 정한 곳으로 간다. 대상 계산 자체는 `plan.rs` 가 진짜 `git push --dry-run` 과 대조한다.

use super::super::methods::{GitPushParams, plan_result};
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
    push_to(f, branch, None, set_upstream, lease).await
}

async fn push_to(
    f: &WriteFixture,
    branch: Option<&str>,
    remote_ref: Option<&str>,
    set_upstream: bool,
    lease: Option<String>,
) -> Result<Pushed, WriteError> {
    push(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        branch.map(str::to_owned),
        None,
        remote_ref.map(str::to_owned),
        set_upstream,
        lease,
        None,
    )
    .await
}

/// `2102` 의 거절 내용.
fn refusal(e: WriteError) -> GitPushRefusal {
    let e = RpcError::from(e);
    assert_eq!(e.code, code::PUSH_REFUSED, "{e:?}");
    serde_json::from_value(e.data.unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pushing_a_new_branch_sets_its_upstream() {
    let (f, remote) = with_remote();
    f.repo.git(&["checkout", "--quiet", "-b", "feat"]);
    let tip = f.repo.commit_file("feat.txt", "feat\n", "feat");
    assert_eq!(f.repo.upstream_of("feat"), None);

    // upstream 이 없으면 터미널의 `git push` 처럼 보내지 않는다 — 창이 대상(같은 이름)을 골라 명시한다.
    let refused = refusal(push_branch(&f, None, true, None).await.unwrap_err());
    assert_eq!(refused.reason, GitPushRefusalReason::NoUpstream);
    assert_eq!(refused.candidates, ["refs/heads/feat"]);
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/feat"), None);

    let pushed = push_to(&f, None, Some(&refused.candidates[0]), true, None)
        .await
        .unwrap();
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
    assert!(
        wire(serde_json::json!({"expectedTarget": {"remote": "origin", "remoteRef": "refs/heads/main"}}))
            .is_ok()
    );
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
    // 설정의 강제 push refspec 은 대상(main)만 정한다 — 보내는 refspec 은 `+` 없는 `refs/heads/main:refs/heads/main` 이라 앞선
    // 원격에는 git 이 거절한다. 터미널이라면 강제했을 것을 계획이 알린다.
    f.repo
        .git(&["config", "remote.origin.push", "+refs/heads/*:refs/heads/*"]);
    let e = push_branch(&f, None, false, None).await.unwrap_err();
    assert_eq!(RpcError::from(e).code, write_code::GIT_FAILED);
    let (_, planned) = plan(f.write.git(), f.project.clone(), None, None, None, None)
        .await
        .unwrap();
    assert!(
        planned
            .terminal_difference()
            .unwrap()
            .sends
            .contains(&"+refs/heads/main:refs/heads/main".to_owned())
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_branch_made_from_origin_main_is_not_pushed_to_main() {
    let (f, remote) = with_remote();
    let main = f.repo.rev_parse("main");
    // `git.branch.create { start: "origin/main", checkout }` 는 upstream 을 origin/main 으로 정한다(branch.autoSetupMerge).
    crate::branch::create(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "feat".into(),
        Some("origin/main".into()),
        true,
    )
    .await
    .unwrap();
    assert_eq!(f.repo.upstream_of("feat").as_deref(), Some("origin/main"));
    let tip = f.repo.commit_file("feat.txt", "feat\n", "feat");

    // 터미널의 `git push`(push.default = simple)는 upstream 이름이 달라 거절한다 — 이 push 도 아무것도 보내지 않는다.
    let terminal = f.repo.try_git_in(f.repo.root(), &["push", "--dry-run"]);
    assert!(!terminal.status.success());
    let refused = refusal(push_branch(&f, None, false, None).await.unwrap_err());
    assert_eq!(refused.reason, GitPushRefusalReason::UpstreamNameDiffers);
    assert_eq!(refused.candidates, ["refs/heads/main", "refs/heads/feat"]);
    assert_eq!(
        f.repo.remote_ref(&remote, "refs/heads/main"),
        Some(main.clone())
    );
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/feat"), None);

    // 계획이 같은 거절을 보내기 전에 보인다.
    let (_, planned) = plan(f.write.git(), f.project.clone(), None, None, None, None)
        .await
        .unwrap();
    assert_eq!(planned.target, Err(refused));
    assert_eq!(
        planned.terminal_difference(),
        None,
        "the terminal refuses too"
    );

    // 사람이 같은 이름을 고르면 그리로 간다 — main 은 그대로다.
    let pushed = push_to(&f, None, Some("feat"), false, None).await.unwrap();
    assert_eq!(pushed.plan.refspec(), "refs/heads/feat:refs/heads/feat");
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/feat"), Some(tip));
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(main));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_configured_push_refspec_decides_the_target() {
    let (f, remote) = with_remote();
    let tip = f.repo.commit_file("a.txt", "a\n", "a");
    f.repo.git(&[
        "config",
        "remote.origin.push",
        "refs/heads/main:refs/heads/elsewhere",
    ]);
    let before = f.repo.remote_ref(&remote, "refs/heads/main");
    let pushed = push_branch(&f, None, false, None).await.unwrap();
    assert_eq!(
        pushed.plan.refspec(),
        "refs/heads/main:refs/heads/elsewhere"
    );
    assert_eq!(pushed.terminal, None, "the terminal pushes the same");
    assert_eq!(
        f.repo.remote_ref(&remote, "refs/heads/elsewhere"),
        Some(tip)
    );
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_names_an_existing_local_branch() {
    let (f, remote) = with_remote();
    let before = f.repo.remote_ref(&remote, "refs/heads/main");
    // glob · refspec 문자가 든 이름은 git 을 띄우기 전에 거절한다 — `*` 가 모든 브랜치를 보내는 refspec 이 되지 않게.
    for branch in ["*", "ma*", "main:other", "^main", "no-such-branch"] {
        let e = push_branch(&f, Some(branch), false, None)
            .await
            .unwrap_err();
        assert_eq!(
            RpcError::from(e).code,
            pw_rpc::code::INVALID_PARAMS,
            "{branch}"
        );
    }
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), before);
}

/// 원격 `main` 에서 만든 `feat`(upstream = origin/main)에 커밋 하나. 그 끝.
fn feature_from_main(f: &WriteFixture) -> String {
    f.repo.git(&[
        "checkout",
        "--quiet",
        "-b",
        "feat",
        "--track",
        "origin/main",
    ]);
    f.repo.commit_file("feat.txt", "feat\n", "feat")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_git_would_send_elsewhere_or_refuse_sends_nothing() {
    let (f, remote) = with_remote();
    f.repo.git(&["tag", "v1"]);
    f.repo.git(&["push", "--quiet", "origin", "v1"]);
    feature_from_main(&f);
    let main = f.repo.remote_ref(&remote, "refs/heads/main");
    let nothing_sent = |what: &str| {
        for name in ["refs/heads/v1", "refs/heads/x", "refs/heads/feat"] {
            assert_eq!(f.repo.remote_ref(&remote, name), None, "{what}: {name}");
        }
        assert_eq!(
            f.repo.remote_ref(&remote, "refs/heads/main"),
            main,
            "{what}"
        );
    };

    // 짧은 대상이 원격의 태그 이름이다 — git 은 `refs/tags/v1` 로 보내는데, 원격을 보지 않고 채우면 `refs/heads/v1` 을 새로
    // 만든다.
    f.repo.git(&["config", "remote.origin.push", "feat:v1"]);
    let refused = refusal(push_branch(&f, None, false, None).await.unwrap_err());
    assert_eq!(refused.reason, GitPushRefusalReason::UnqualifiedDestination);
    nothing_sent("a short destination");

    // 한 원격 ref 로 보내는 정확한 refspec 둘 — git 은 push 전체를 거절한다.
    f.repo.git(&[
        "config",
        "remote.origin.push",
        "refs/heads/feat:refs/heads/x",
    ]);
    f.repo.git(&[
        "config",
        "--add",
        "remote.origin.push",
        "refs/heads/main:refs/heads/x",
    ]);
    let refused = refusal(push_branch(&f, None, false, None).await.unwrap_err());
    assert_eq!(refused.reason, GitPushRefusalReason::InvalidConfig);
    nothing_sent("two sources to one destination");
    f.repo.git(&["config", "--unset-all", "remote.origin.push"]);

    // 이름 붙은 원격이 없고 push 원격이 주소다 — git 은 그 주소를 하나뿐인 원격으로 보아 upstream 을 요구한다.
    let url = remote.url.clone();
    f.repo.git(&["remote", "remove", "origin"]);
    f.repo.git(&["config", "branch.feat.pushRemote", &url]);
    let refused = refusal(push_branch(&f, None, false, None).await.unwrap_err());
    assert_eq!(refused.reason, GitPushRefusalReason::NoUpstream);
    nothing_sent("a URL push remote and no named remote");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_whose_target_changed_after_the_plan_sends_nothing() {
    let (f, remote) = with_remote();
    let main = f.repo.remote_ref(&remote, "refs/heads/main");
    let tip = feature_from_main(&f);
    f.repo.git(&["config", "push.default", "current"]);
    let (_, planned) = plan(f.write.git(), f.project.clone(), None, None, None, None)
        .await
        .unwrap();
    let shown = plan_result(String::new(), &planned);
    let expected = GitPushExpectedTarget {
        remote: shown.remote,
        remote_ref: shown.remote_ref.unwrap(),
    };
    assert_eq!(expected.remote_ref, "refs/heads/feat");

    // 다이얼로그가 계획을 보인 뒤 터미널에서 설정이 바뀌었다 — 다시 세면 대상이 main 이다.
    f.repo.git(&["config", "push.default", "upstream"]);
    let push_expecting = |expected: GitPushExpectedTarget| {
        push(
            &f.write,
            f.caller(),
            f.project.clone(),
            None,
            None,
            None,
            None,
            false,
            None,
            Some(expected),
        )
    };
    let e = RpcError::from(push_expecting(expected.clone()).await.unwrap_err());
    assert_eq!(e.code, pw_rpc::code::CONFLICT, "{e:?}");
    assert_eq!(
        e.data,
        Some(serde_json::json!({"remote": "origin", "remoteRef": "refs/heads/main"}))
    );
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), main);
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/feat"), None);

    // 같은 대상이면 보낸다.
    f.repo.git(&["config", "push.default", "current"]);
    let pushed = push_expecting(expected).await.unwrap();
    assert_eq!(pushed.plan.remote_ref, "refs/heads/feat");
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/feat"), Some(tip));
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), main);
}
