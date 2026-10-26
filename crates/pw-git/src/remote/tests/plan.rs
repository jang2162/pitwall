//! push 계획: 대상이 터미널의 `git push` 와 같다 — 설정마다 진짜 `git push --dry-run --porcelain` 이 보낼 것과 대조한다
//! (`push.default` 다섯 · push refspec 의 정확한 이름 · 패턴 · `HEAD` · `:` · `^`). 응답의 원격 주소에 사용자 정보가 없다. 미신뢰
//! 프로젝트의 계획은 gix 로만 셈하고 저장소 설정의 프로그램을 돌리지 않는다.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::super::methods::plan_result;
use super::super::target::{Mapping, plan as plan_in};
use super::super::*;
use crate::testing::TestRepo;
use crate::testing::write::WriteFixture;
use crate::write::Askpass;

/// `origin`(bare)에 `main` · `other` 가 있고, `feat` 를 `origin/main` 에서 만들어(upstream = origin/main) 커밋 하나를 더한 저장소.
/// HEAD 는 `feat` 다.
fn feature_repo() -> TestRepo {
    let repo = TestRepo::with_commit();
    repo.add_bare_remote("origin");
    repo.git(&["branch", "other"]);
    repo.git(&["push", "--quiet", "origin", "other"]);
    repo.git(&["branch", "feat", "origin/main"]);
    repo.checkout("feat");
    repo.commit_file("feat.txt", "feat\n", "feat");
    repo
}

fn planned(repo: &TestRepo) -> Planned {
    let opened = crate::repo::discover(repo.root(), true).unwrap().unwrap();
    plan_in(&opened, None, None, None).unwrap()
}

/// 진짜 git 이 보낼 것(`<src>:<dst>`, `HEAD` 는 지금 브랜치로 바꾼다). 거절하면 `Err(stderr)`.
fn dry_run(repo: &TestRepo) -> Result<BTreeSet<(String, String)>, String> {
    let out = repo.try_git_in(repo.root(), &["push", "--dry-run", "--porcelain"]);
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let head = repo.git(&["symbolic-ref", "HEAD"]).trim().to_owned();
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let flag = fields.next()?;
            let spec = fields.next()?;
            (flag.len() == 1).then_some(())?;
            let (src, dst) = spec.split_once(':')?;
            let src = if src == "HEAD" {
                head.clone()
            } else {
                src.to_owned()
            };
            Some((src, dst.to_owned()))
        })
        .collect())
}

fn sends(mappings: &[Mapping]) -> BTreeSet<(String, String)> {
    mappings
        .iter()
        .map(|m| (m.src.clone(), m.dst.clone()))
        .collect()
}

use GitPushRefusalReason as R;

/// 한 설정: 이름, 그 설정을 만드는 git 명령들, 기대하는 대상(원격 ref 또는 거절 까닭), 설정이 강제 push 인가.
struct Case {
    name: &'static str,
    setup: &'static [&'static [&'static str]],
    expect: Result<&'static str, R>,
    forced: bool,
}

const fn case(
    name: &'static str,
    setup: &'static [&'static [&'static str]],
    expect: Result<&'static str, R>,
) -> Case {
    Case {
        name,
        setup,
        expect,
        forced: false,
    }
}

const CASES: &[Case] = &[
    // push.default 다섯.
    case("simple (default)", &[], Err(R::UpstreamNameDiffers)),
    case(
        "simple, no upstream",
        &[&["branch", "--unset-upstream"]],
        Err(R::NoUpstream),
    ),
    case(
        "simple, autoSetupRemote",
        &[
            &["branch", "--unset-upstream"],
            &["config", "push.autoSetupRemote", "true"],
        ],
        Ok("refs/heads/feat"),
    ),
    case(
        "current",
        &[&["config", "push.default", "current"]],
        Ok("refs/heads/feat"),
    ),
    case(
        "upstream",
        &[&["config", "push.default", "upstream"]],
        Ok("refs/heads/main"),
    ),
    case(
        "tracking",
        &[&["config", "push.default", "tracking"]],
        Ok("refs/heads/main"),
    ),
    case(
        "nothing",
        &[&["config", "push.default", "nothing"]],
        Err(R::Nothing),
    ),
    case(
        "matching",
        &[&["config", "push.default", "matching"]],
        Err(R::Matching),
    ),
    case(
        "unknown push.default",
        &[&["config", "push.default", "bogus"]],
        Err(R::InvalidConfig),
    ),
    // push 원격이 upstream 원격이 아니다.
    case(
        "simple to another push remote",
        &[
            &["remote", "add", "fork", "../fork.git"],
            &["config", "remote.pushDefault", "fork"],
        ],
        Ok("refs/heads/feat"),
    ),
    case(
        "upstream to another push remote",
        &[
            &["remote", "add", "fork", "../fork.git"],
            &["config", "remote.pushDefault", "fork"],
            &["config", "push.default", "upstream"],
        ],
        Err(R::NotUpstreamRemote),
    ),
    // push refspec.
    case(
        "exact",
        &[&["config", "remote.origin.push", "refs/heads/feat"]],
        Ok("refs/heads/feat"),
    ),
    case(
        "exact, short destination",
        &[&["config", "remote.origin.push", "feat:review/feat"]],
        Ok("refs/heads/review/feat"),
    ),
    case(
        "HEAD",
        &[&["config", "remote.origin.push", "HEAD"]],
        Ok("refs/heads/feat"),
    ),
    case(
        "HEAD to a ref",
        &[&["config", "remote.origin.push", "HEAD:refs/heads/hx"]],
        Ok("refs/heads/hx"),
    ),
    case(
        "glob",
        &[&[
            "config",
            "remote.origin.push",
            "refs/heads/*:refs/heads/u/*",
        ]],
        Ok("refs/heads/u/feat"),
    ),
    case(
        "the first glob that matches",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/f*:refs/heads/a/*",
            ],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/*:refs/heads/b/*",
            ],
        ],
        Ok("refs/heads/a/eat"),
    ),
    case(
        "an exact name before an earlier glob",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/*:refs/heads/x/*",
            ],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/feat:refs/heads/main",
            ],
        ],
        Ok("refs/heads/main"),
    ),
    case(
        "two exact names",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/feat:refs/heads/a",
            ],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/feat:refs/heads/b",
            ],
        ],
        Ok("refs/heads/a"),
    ),
    case(
        "matching (`:`), not on the remote",
        &[&["config", "remote.origin.push", ":"]],
        Err(R::NotPushedByConfig),
    ),
    case(
        "matching (`:`), on the remote",
        &[
            &["push", "--quiet", "origin", "feat"],
            &["commit", "--quiet", "--allow-empty", "-m", "more"],
            &["config", "remote.origin.push", ":"],
        ],
        Ok("refs/heads/feat"),
    ),
    case(
        "negative",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/*:refs/heads/*",
            ],
            &["config", "--add", "remote.origin.push", "^refs/heads/feat"],
        ],
        Err(R::NotPushedByConfig),
    ),
    case(
        "another branch only",
        &[&[
            "config",
            "remote.origin.push",
            "refs/heads/other:refs/heads/other",
        ]],
        Err(R::NotPushedByConfig),
    ),
    case(
        "a delete",
        &[&["config", "remote.origin.push", ":refs/heads/other"]],
        Err(R::NotPushedByConfig),
    ),
    case(
        "a source that matches nothing",
        &[&["config", "remote.origin.push", "nosuch:refs/heads/x"]],
        Err(R::InvalidConfig),
    ),
    Case {
        name: "forced",
        setup: &[&[
            "config",
            "remote.origin.push",
            "+refs/heads/feat:refs/heads/main",
        ]],
        expect: Ok("refs/heads/main"),
        forced: true,
    },
];

#[test]
fn the_plan_is_where_git_push_goes() {
    for case in CASES {
        let repo = feature_repo();
        let fork = repo.base().join("fork.git");
        repo.git_in(
            repo.base(),
            &["init", "--bare", "--quiet", fork.to_str().unwrap()],
        );
        for args in case.setup {
            repo.git(args);
        }
        let planned = planned(&repo);
        let git = dry_run(&repo);
        let name = case.name;
        match (&case.expect, &git) {
            (Err(reason), Err(stderr)) => {
                assert_eq!(
                    planned.target.as_ref().map_err(|r| r.reason),
                    Err(*reason),
                    "{name}: git refused: {stderr}"
                );
                assert_eq!(
                    planned.terminal.as_ref().map_err(|r| r.reason).err(),
                    Some(*reason),
                    "{name}"
                );
                assert_eq!(planned.terminal_difference(), None, "{name}: both refuse");
            }
            (Err(reason), Ok(git)) => {
                // git 은 다른 ref 를 보내지만 이 브랜치는 보내지 않는다(또는 matching 이라 하지 않는다).
                assert_eq!(
                    planned.target.as_ref().map_err(|r| r.reason),
                    Err(*reason),
                    "{name}"
                );
                assert!(
                    git.iter().all(|(src, _)| src != "refs/heads/feat"),
                    "{name}: git pushes feat: {git:?}"
                );
                assert_eq!(sends(planned.terminal.as_ref().unwrap()), *git, "{name}");
                assert!(planned.terminal_difference().is_some(), "{name}");
            }
            (Ok(dst), Ok(git)) => {
                assert_eq!(
                    planned.target.as_ref().map(|(d, _)| d.as_str()),
                    Ok(*dst),
                    "{name}"
                );
                assert!(
                    git.contains(&("refs/heads/feat".to_owned(), (*dst).to_owned())),
                    "{name}: git does not push feat to {dst}: {git:?}"
                );
                assert_eq!(sends(planned.terminal.as_ref().unwrap()), *git, "{name}");
                let only_this = git.len() == 1 && !case.forced;
                assert_eq!(
                    planned.terminal_difference().is_none(),
                    only_this,
                    "{name}: {:?}",
                    planned.terminal_difference()
                );
            }
            (Ok(dst), Err(stderr)) => panic!("{name}: expected {dst}, git refused: {stderr}"),
        }
    }
}

#[test]
fn the_plan_reports_the_forced_refspec_and_the_lease_ref() {
    let repo = feature_repo();
    repo.git(&[
        "config",
        "remote.origin.push",
        "+refs/heads/feat:refs/heads/main",
    ]);
    let planned = planned(&repo);
    let terminal = planned.terminal_difference().unwrap();
    assert_eq!(terminal.configured, ["+refs/heads/feat:refs/heads/main"]);
    assert_eq!(terminal.sends, ["+refs/heads/feat:refs/heads/main"]);
    assert_eq!(terminal.refusal, None);
    // force 의 기대값은 대상(main)의 원격 추적 ref 에서 읽는다 — 이 브랜치의 이름이 아니다.
    assert_eq!(
        planned.lease,
        Some(GitPushLease {
            tracking_ref: "refs/remotes/origin/main".into(),
            commit: Some(repo.rev_parse("refs/remotes/origin/main")),
        })
    );
}

#[test]
fn a_refusal_names_the_targets_to_choose_from() {
    let repo = feature_repo();
    let planned = planned(&repo);
    let refusal = planned.target.clone().unwrap_err();
    assert_eq!(refusal.reason, R::UpstreamNameDiffers);
    assert_eq!(refusal.candidates, ["refs/heads/main", "refs/heads/feat"]);
    // 사람이 대상을 고르면 그것이다.
    let opened = crate::repo::discover(repo.root(), true).unwrap().unwrap();
    for (input, full) in [
        ("feat", "refs/heads/feat"),
        ("refs/heads/main", "refs/heads/main"),
    ] {
        let chosen = plan_in(&opened, None, None, Some(input)).unwrap();
        assert_eq!(
            chosen.target,
            Ok((full.to_owned(), GitPushTargetSource::Input)),
            "{input}"
        );
    }
    for bad in ["*", "a:b", "refs/heads/a..b", "-x", "+x:y"] {
        assert!(plan_in(&opened, None, None, Some(bad)).is_err(), "{bad}");
    }
}

#[test]
fn the_push_remote_is_the_only_remote_when_there_is_no_origin() {
    let repo = feature_repo();
    repo.git(&["branch", "--unset-upstream"]);
    repo.git(&["remote", "rename", "origin", "up"]);
    let planned = planned(&repo);
    assert_eq!(planned.remote, "up");
    assert_eq!(planned.target.unwrap_err().reason, R::NoUpstream);
    assert!(
        dry_run(&repo)
            .unwrap_err()
            .contains("git push --set-upstream up feat")
    );
}

#[test]
fn a_mirror_remote_is_refused_even_with_a_chosen_target() {
    let repo = feature_repo();
    repo.git(&["config", "remote.origin.mirror", "true"]);
    let opened = crate::repo::discover(repo.root(), true).unwrap().unwrap();
    for input in [None, Some("refs/heads/feat")] {
        let planned = plan_in(&opened, None, None, input).unwrap();
        assert_eq!(planned.target.unwrap_err().reason, R::Mirror, "{input:?}");
    }
    // git 도 명시한 refspec 을 받지 않는다.
    let out = repo.try_git_in(
        repo.root(),
        &[
            "push",
            "--dry-run",
            "origin",
            "refs/heads/feat:refs/heads/feat",
        ],
    );
    assert!(!out.status.success());
}

#[test]
fn a_remote_url_is_shown_without_its_user_info() {
    for (url, shown) in [
        (
            "http://alice:SECRET@127.0.0.1:9/r.git",
            "http://127.0.0.1:9/r.git",
        ),
        ("https://tok@host/a@b.git", "https://host/a@b.git"),
        ("ssh://git@host:22/r.git", "ssh://host:22/r.git"),
        ("git@host:r.git", "git@host:r.git"),
        ("origin", "origin"),
        ("file:///tmp/r.git", "file:///tmp/r.git"),
    ] {
        assert_eq!(redact(url), shown, "{url}");
    }
    let repo = feature_repo();
    let url = "http://alice:SECRET@127.0.0.1:9/r.git";
    repo.git(&["config", "branch.feat.pushRemote", url]);
    repo.git(&["config", "push.default", "upstream"]);
    let planned = planned(&repo);
    assert_eq!(planned.remote, url, "git gets the configured remote as is");
    let json = serde_json::to_string(&plan_result(String::new(), &planned)).unwrap();
    assert!(!json.contains("SECRET"), "{json}");
    assert!(json.contains("http://127.0.0.1:9/r.git"), "{json}");
    assert_eq!(
        planned.target.unwrap_err().reason,
        R::NotUpstreamRemote,
        "the URL is not the upstream remote"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_to_a_url_with_a_token_keeps_the_token_out_of_the_error() {
    let f = WriteFixture::new(true);
    f.repo.add_bare_remote("origin");
    let url = "http://alice:SECRET@127.0.0.1:9/r.git";
    f.repo.git(&["config", "branch.main.pushRemote", url]);
    let e = push(
        &f.write,
        f.project.clone(),
        None,
        None,
        None,
        None,
        false,
        None,
    )
    .await
    .unwrap_err();
    let rpc = RpcError::from(e);
    assert_eq!(rpc.code, crate::write::code::GIT_FAILED, "{rpc:?}");
    let json = serde_json::to_string(&rpc).unwrap();
    assert!(!json.contains("SECRET"), "{json}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_plan_reads_with_gix_and_runs_no_planted_program() {
    let repo = feature_repo();
    let marks = repo.base().join("marks");
    let bin = repo.base().join("planted");
    fs::create_dir_all(&marks).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let script = |name: &str| {
        let path = bin.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\nexit 1\n",
                marks.join(name).display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    };
    for (key, value) in [
        ("core.fsmonitor", script("fsmonitor")),
        ("credential.helper", script("credential")),
        ("remote.origin.receivepack", script("receivepack")),
        ("core.sshCommand", script("ssh")),
    ] {
        repo.git(&["config", key, &value]);
    }
    let trusted = planned(&repo);
    let f = WriteFixture::with_repo(repo, false, Askpass::default());
    assert!(!f.projects.is_trusted(&f.project));
    let (work_tree, untrusted) = plan(f.write.git(), f.project.clone(), None, None, None, None)
        .await
        .unwrap();
    assert_eq!(work_tree, f.repo.root().display().to_string());
    assert_eq!(untrusted, trusted);
    assert_eq!(fs::read_dir(&marks).unwrap().count(), 0, "nothing ran");
}
