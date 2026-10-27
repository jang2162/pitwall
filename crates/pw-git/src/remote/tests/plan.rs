//! push 계획: 대상이 터미널의 `git push` 와 같다 — 설정마다 진짜 `git push --dry-run --porcelain` 이 보낼 것과 대조한다
//! (`push.default` 다섯 · 원격 고르기 · push refspec 의 정확한 이름 · 짧은 대상 · 패턴 · `HEAD` · `:` · `^`). git 과 다른 것은
//! 우리가 거절하는 쪽뿐이다(13-m2-plan.md M2-09). 응답의 원격 주소에 사용자 정보가 없다. 미신뢰 프로젝트의 계획은 gix 로만
//! 셈하고 저장소 설정의 프로그램을 돌리지 않는다.

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
    planned_to(repo, None)
}

fn planned_to(repo: &TestRepo, remote: Option<&str>) -> Planned {
    let opened = crate::repo::discover(repo.root(), true).unwrap().unwrap();
    plan_in(&opened, None, remote, None).unwrap()
}

/// 진짜 `git push --dry-run --porcelain [<원격>]`.
struct DryRun {
    /// git 이 보낼 것(`<src>:<dst>`, `HEAD` 는 지금 브랜치로 바꾼다). 원격이 받지 않을 것(`!`)도 든다 — git 이 고른 대상이다.
    sends: BTreeSet<(String, String)>,
    /// 보낼 것을 정하지도 못하고 실패했으면 그 stderr.
    refused: Option<String>,
}

fn dry_run(repo: &TestRepo, remote: Option<&str>) -> DryRun {
    let mut args = vec!["push", "--dry-run", "--porcelain"];
    args.extend(remote);
    let out = repo.try_git_in(repo.root(), &args);
    // 분리된 HEAD · 깨진 설정이면 git 이 아무것도 보내지 않으므로 비어도 된다.
    let head = repo.try_git_in(repo.root(), &["symbolic-ref", "HEAD"]);
    let head = String::from_utf8_lossy(&head.stdout).trim().to_owned();
    let sends: BTreeSet<_> = String::from_utf8_lossy(&out.stdout)
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
        .collect();
    let refused = (!out.status.success() && sends.is_empty())
        .then(|| String::from_utf8_lossy(&out.stderr).into_owned());
    DryRun { sends, refused }
}

/// 계획의 터미널 몫을 git 의 꼴로: 짧은 대상(설정 원문)은 git 이 같은 로컬 ref 에서 그 이름으로 끝나는 곳으로 보냈으면 그것으로
/// 본다(git 은 원격의 ref 로 정한다).
fn sends(mappings: &[Mapping], git: &BTreeSet<(String, String)>) -> BTreeSet<(String, String)> {
    mappings
        .iter()
        .map(|m| {
            let dst = if m.dst.starts_with("refs/") {
                m.dst.clone()
            } else {
                git.iter()
                    .find(|(src, dst)| *src == m.src && dst.ends_with(&format!("/{}", m.dst)))
                    .map_or_else(|| m.dst.clone(), |(_, dst)| dst.clone())
            };
            (m.src.clone(), dst)
        })
        .collect()
}

use GitPushRefusalReason as R;

/// 기대하는 대상.
#[derive(Debug)]
enum Expect {
    /// 이 원격 ref 로 보낸다 — git 도 이 브랜치를 그리로 보낸다.
    To(&'static str),
    /// 이 까닭으로 거절한다 — git 도 거절하거나 이 브랜치는 보내지 않는다.
    Refused(R),
    /// 이 까닭으로 거절하는데 git 은 이 브랜치를 보낸다 — 안전한 쪽으로 다른 것(13-m2-plan.md M2-09).
    Stricter(R),
}

use Expect::{Refused, Stricter, To};

/// 한 설정: 이름, 그 설정을 만드는 git 명령들(`{main}` 은 main 의 해시), 입력 원격, 기대, 설정이 강제 push 인가.
struct Case {
    name: &'static str,
    setup: &'static [&'static [&'static str]],
    remote: Option<&'static str>,
    expect: Expect,
    forced: bool,
}

const fn case(
    name: &'static str,
    setup: &'static [&'static [&'static str]],
    expect: Expect,
) -> Case {
    Case {
        name,
        setup,
        remote: None,
        expect,
        forced: false,
    }
}

const fn case_to(
    name: &'static str,
    setup: &'static [&'static [&'static str]],
    remote: &'static str,
    expect: Expect,
) -> Case {
    Case {
        name,
        setup,
        remote: Some(remote),
        expect,
        forced: false,
    }
}

/// `fork` 원격(`../fork.git`, 시험이 bare 로 만든다).
const ADD_FORK: &[&str] = &["remote", "add", "fork", "../fork.git"];
const UNSET_UPSTREAM: &[&str] = &["branch", "--unset-upstream"];

/// `push.default` 다섯과 upstream 의 모양.
const PUSH_DEFAULT: &[Case] = &[
    case("simple (default)", &[], Refused(R::UpstreamNameDiffers)),
    case(
        "simple, no upstream",
        &[UNSET_UPSTREAM],
        Refused(R::NoUpstream),
    ),
    case(
        "simple, autoSetupRemote",
        &[UNSET_UPSTREAM, &["config", "push.autoSetupRemote", "true"]],
        To("refs/heads/feat"),
    ),
    case(
        "current",
        &[&["config", "push.default", "current"]],
        To("refs/heads/feat"),
    ),
    case(
        "upstream",
        &[&["config", "push.default", "upstream"]],
        To("refs/heads/main"),
    ),
    case(
        "tracking",
        &[&["config", "push.default", "tracking"]],
        To("refs/heads/main"),
    ),
    case(
        "nothing",
        &[&["config", "push.default", "nothing"]],
        Refused(R::Nothing),
    ),
    case(
        "matching",
        &[&["config", "push.default", "matching"]],
        Refused(R::Matching),
    ),
    case(
        "unknown push.default",
        &[&["config", "push.default", "bogus"]],
        Refused(R::InvalidConfig),
    ),
    // upstream 의 모양.
    case(
        "no merge",
        &[&["config", "--unset", "branch.feat.merge"]],
        Refused(R::NoUpstream),
    ),
    case(
        "no branch remote",
        &[&["config", "--unset", "branch.feat.remote"]],
        Refused(R::NoUpstream),
    ),
    case(
        "two merges",
        &[&["config", "--add", "branch.feat.merge", "refs/heads/other"]],
        Refused(R::MultipleUpstreams),
    ),
    case(
        "short merge, another name",
        &[&["config", "branch.feat.merge", "main"]],
        Refused(R::UpstreamNameDiffers),
    ),
    case(
        // git 은 `merge` 원문을 브랜치의 전체 이름과 견주므로 짧은 같은 이름도 다르다.
        "short merge, the same name",
        &[&["config", "branch.feat.merge", "feat"]],
        Refused(R::UpstreamNameDiffers),
    ),
    case(
        "short merge, upstream mode",
        &[
            &["config", "branch.feat.merge", "main"],
            &["config", "push.default", "upstream"],
        ],
        Stricter(R::UnqualifiedDestination),
    ),
    case(
        "local upstream (`.`), simple",
        &[&["config", "branch.feat.remote", "."]],
        Refused(R::UpstreamNameDiffers),
    ),
    case(
        "local upstream (`.`), upstream mode",
        &[
            &["config", "branch.feat.remote", "."],
            &["config", "push.default", "upstream"],
        ],
        To("refs/heads/main"),
    ),
    case(
        "autoSetupRemote, upstream mode",
        &[
            UNSET_UPSTREAM,
            &["config", "push.autoSetupRemote", "true"],
            &["config", "push.default", "upstream"],
        ],
        To("refs/heads/feat"),
    ),
    case(
        "autoSetupRemote, current",
        &[
            UNSET_UPSTREAM,
            &["config", "push.autoSetupRemote", "true"],
            &["config", "push.default", "current"],
        ],
        To("refs/heads/feat"),
    ),
    case(
        "autoSetupRemote, upstream mode to another push remote",
        &[
            UNSET_UPSTREAM,
            ADD_FORK,
            &["config", "push.autoSetupRemote", "true"],
            &["config", "push.default", "upstream"],
            &["config", "remote.pushDefault", "fork"],
        ],
        Refused(R::NotUpstreamRemote),
    ),
];

/// 원격 고르기 — push 원격 · upstream 원격 · 원격 개수(주소도 센다).
const REMOTES: &[Case] = &[
    case(
        "simple to another push remote",
        &[ADD_FORK, &["config", "remote.pushDefault", "fork"]],
        To("refs/heads/feat"),
    ),
    case(
        "upstream to another push remote",
        &[
            ADD_FORK,
            &["config", "remote.pushDefault", "fork"],
            &["config", "push.default", "upstream"],
        ],
        Refused(R::NotUpstreamRemote),
    ),
    case(
        "upstream to another branch push remote",
        &[
            ADD_FORK,
            &["config", "branch.feat.pushRemote", "fork"],
            &["config", "push.default", "upstream"],
        ],
        Refused(R::NotUpstreamRemote),
    ),
    case(
        "current to another push remote",
        &[
            ADD_FORK,
            &["config", "remote.pushDefault", "fork"],
            &["config", "push.default", "current"],
        ],
        To("refs/heads/feat"),
    ),
    case(
        "branch pushRemote is the upstream remote",
        &[ADD_FORK, &["config", "branch.feat.pushRemote", "origin"]],
        Refused(R::UpstreamNameDiffers),
    ),
    case(
        "pushDefault is the upstream remote",
        &[ADD_FORK, &["config", "remote.pushDefault", "origin"]],
        Refused(R::UpstreamNameDiffers),
    ),
    case(
        "upstream on another remote, the same name",
        &[
            ADD_FORK,
            &["push", "--quiet", "fork", "feat"],
            &["branch", "--set-upstream-to", "fork/feat"],
        ],
        To("refs/heads/feat"),
    ),
    case(
        "upstream on another remote, pushDefault origin",
        &[
            ADD_FORK,
            &["push", "--quiet", "fork", "main"],
            &["branch", "--set-upstream-to", "fork/main"],
            &["config", "remote.pushDefault", "origin"],
        ],
        To("refs/heads/feat"),
    ),
    case(
        "the only remote is not origin",
        &[&["remote", "remove", "origin"], ADD_FORK],
        Refused(R::NoUpstream),
    ),
    case(
        "a remote key without a URL counts",
        &[UNSET_UPSTREAM, &["config", "remote.ghost.prune", "true"]],
        Refused(R::NoUpstream),
    ),
    case(
        "the only remote has a pushurl only",
        &[
            &["remote", "remove", "origin"],
            &["config", "remote.pub.pushurl", "../origin.git"],
        ],
        Refused(R::NoUpstream),
    ),
    // push 원격이 주소다 — git 은 그것도 원격으로 센다.
    case_to(
        "a URL input beside origin",
        &[UNSET_UPSTREAM],
        "../origin.git",
        To("refs/heads/feat"),
    ),
    case_to(
        "a URL input and no named remote",
        &[&["remote", "remove", "origin"]],
        "../origin.git",
        Refused(R::NoUpstream),
    ),
    case(
        "a URL pushRemote and no named remote",
        &[
            &["remote", "remove", "origin"],
            &["config", "branch.feat.pushRemote", "../fork.git"],
        ],
        Refused(R::NoUpstream),
    ),
];

/// push refspec(대상이 전체 이름).
const REFSPECS: &[Case] = &[
    case(
        "exact",
        &[&["config", "remote.origin.push", "refs/heads/feat"]],
        To("refs/heads/feat"),
    ),
    case(
        "exact, short source",
        &[&[
            "config",
            "remote.origin.push",
            "feat:refs/heads/review/feat",
        ]],
        To("refs/heads/review/feat"),
    ),
    case(
        "HEAD",
        &[&["config", "remote.origin.push", "HEAD"]],
        To("refs/heads/feat"),
    ),
    case(
        "HEAD to a ref",
        &[&["config", "remote.origin.push", "HEAD:refs/heads/hx"]],
        To("refs/heads/hx"),
    ),
    case(
        "glob",
        &[&[
            "config",
            "remote.origin.push",
            "refs/heads/*:refs/heads/u/*",
        ]],
        To("refs/heads/u/feat"),
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
        To("refs/heads/a/eat"),
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
        To("refs/heads/main"),
    ),
    Case {
        name: "a forced glob and an exact name",
        setup: &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "+refs/heads/*:refs/heads/*",
            ],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/feat:refs/heads/x",
            ],
        ],
        remote: None,
        expect: To("refs/heads/x"),
        forced: true,
    },
    Case {
        name: "forced",
        setup: &[&[
            "config",
            "remote.origin.push",
            "+refs/heads/feat:refs/heads/main",
        ]],
        remote: None,
        expect: To("refs/heads/main"),
        forced: true,
    },
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
        To("refs/heads/a"),
    ),
    case(
        "two sources to one destination",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/feat:refs/heads/x",
            ],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/main:refs/heads/x",
            ],
        ],
        Refused(R::InvalidConfig),
    ),
    case(
        "the same exact name twice",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/feat:refs/heads/x",
            ],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/feat:refs/heads/x",
            ],
        ],
        Refused(R::InvalidConfig),
    ),
    case(
        "two globs to one destination",
        &[
            &["tag", "feat", "main"],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/*:refs/heads/u/*",
            ],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/tags/*:refs/heads/u/*",
            ],
        ],
        To("refs/heads/u/feat"),
    ),
    case(
        "a glob destination taken by an exact name",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/*:refs/heads/*",
            ],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/other:refs/heads/feat",
            ],
        ],
        Refused(R::NotPushedByConfig),
    ),
    case(
        "matching (`:`), not on the remote",
        &[&["config", "remote.origin.push", ":"]],
        Refused(R::NotPushedByConfig),
    ),
    case(
        "matching (`:`), on the remote",
        &[
            &["push", "--quiet", "origin", "feat"],
            &["commit", "--quiet", "--allow-empty", "-m", "more"],
            &["config", "remote.origin.push", ":"],
        ],
        To("refs/heads/feat"),
    ),
    case(
        "matching (`:`) and a glob",
        &[
            &["config", "--add", "remote.origin.push", ":"],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/f*:refs/heads/q/*",
            ],
        ],
        To("refs/heads/q/eat"),
    ),
    Case {
        name: "forced matching (`+:`) and a glob",
        setup: &[
            &["config", "--add", "remote.origin.push", "+:"],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/f*:refs/heads/q/*",
            ],
        ],
        remote: None,
        expect: To("refs/heads/q/eat"),
        forced: true,
    },
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
        Refused(R::NotPushedByConfig),
    ),
    case(
        // `^` 는 전체 이름에만 맞는다 — 짧은 이름은 아무것도 빼지 않는다.
        "a short negative",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/*:refs/heads/*",
            ],
            &["config", "--add", "remote.origin.push", "^feat"],
        ],
        To("refs/heads/feat"),
    ),
    case(
        "a negative glob and matching (`:`)",
        &[
            &["push", "--quiet", "origin", "feat"],
            &["config", "--add", "remote.origin.push", "^refs/heads/f*"],
            &["config", "--add", "remote.origin.push", ":"],
        ],
        Refused(R::NotPushedByConfig),
    ),
    case(
        "an exact name and a negative glob",
        &[
            &["config", "--add", "remote.origin.push", "refs/heads/feat"],
            &["config", "--add", "remote.origin.push", "^refs/heads/*"],
        ],
        Refused(R::NotPushedByConfig),
    ),
    case(
        // git 은 정확한 refspec 에는 `^` 를 쓰지 않는다 — 우리는 빼고 거절한다.
        "an exact negative and the same exact name",
        &[
            &["config", "--add", "remote.origin.push", "^refs/heads/feat"],
            &[
                "config",
                "--add",
                "remote.origin.push",
                "refs/heads/feat:refs/heads/x",
            ],
        ],
        Stricter(R::NotPushedByConfig),
    ),
    case(
        "another branch only",
        &[&[
            "config",
            "remote.origin.push",
            "refs/heads/other:refs/heads/other",
        ]],
        Refused(R::NotPushedByConfig),
    ),
    case(
        "a delete",
        &[&["config", "remote.origin.push", ":refs/heads/other"]],
        Refused(R::NotPushedByConfig),
    ),
    case(
        "a delete and HEAD",
        &[
            &["config", "--add", "remote.origin.push", ":refs/heads/other"],
            &["config", "--add", "remote.origin.push", "HEAD"],
        ],
        To("refs/heads/feat"),
    ),
    case(
        "a source that matches nothing",
        &[&["config", "remote.origin.push", "nosuch:refs/heads/x"]],
        Refused(R::InvalidConfig),
    ),
    case(
        "an ambiguous source",
        &[
            &["tag", "feat", "main"],
            &["config", "remote.origin.push", "feat:refs/heads/x"],
        ],
        Refused(R::InvalidConfig),
    ),
    case(
        "an invalid glob",
        &[&[
            "config",
            "remote.origin.push",
            "refs/heads/*/*:refs/heads/*",
        ]],
        Refused(R::InvalidConfig),
    ),
    case(
        // git 은 해시를 그대로 보낸다 — 우리는 로컬 ref 만 받는다.
        "a hash source",
        &[
            &[
                "config",
                "--add",
                "remote.origin.push",
                "{main}:refs/heads/h",
            ],
            &["config", "--add", "remote.origin.push", "HEAD"],
        ],
        Stricter(R::InvalidConfig),
    ),
];

/// 짧은 대상: git 은 원격이 광고하는 ref 로 정한다 — 원격을 보지 않는 계획은 거절한다.
const SHORT_DESTINATIONS: &[Case] = &[
    case(
        "a short destination that is a remote tag",
        &[
            &["tag", "v1", "main"],
            &["push", "--quiet", "origin", "v1"],
            &["config", "remote.origin.push", "feat:v1"],
        ],
        Stricter(R::UnqualifiedDestination),
    ),
    case(
        "a short destination that is a remote branch",
        &[&["config", "remote.origin.push", "feat:other"]],
        Stricter(R::UnqualifiedDestination),
    ),
    case(
        "a short destination not on the remote",
        &[&["config", "remote.origin.push", "feat:review/feat"]],
        Stricter(R::UnqualifiedDestination),
    ),
    case(
        "HEAD to a short name",
        &[&["config", "remote.origin.push", "HEAD:main"]],
        Stricter(R::UnqualifiedDestination),
    ),
    case(
        "a short destination of another branch",
        &[
            &["config", "--add", "remote.origin.push", "HEAD"],
            &["config", "--add", "remote.origin.push", "main:other"],
        ],
        Stricter(R::UnqualifiedDestination),
    ),
    case(
        "a short delete",
        &[&["config", "remote.origin.push", ":other"]],
        Refused(R::UnqualifiedDestination),
    ),
];

#[test]
fn the_plan_follows_push_default_like_git() {
    check(PUSH_DEFAULT);
}

#[test]
fn the_plan_picks_the_push_remote_like_git() {
    check(REMOTES);
}

#[test]
fn the_plan_follows_push_refspecs_like_git() {
    check(REFSPECS);
}

#[test]
fn the_plan_refuses_short_destinations_git_resolves_on_the_remote() {
    check(SHORT_DESTINATIONS);
}

/// 설정마다 계획과 진짜 `git push --dry-run` 을 견준다.
fn check(cases: &[Case]) {
    for case in cases {
        let repo = feature_repo();
        let fork = repo.base().join("fork.git");
        repo.git_in(
            repo.base(),
            &["init", "--bare", "--quiet", fork.to_str().unwrap()],
        );
        let main = repo.rev_parse("main");
        for args in case.setup {
            let args: Vec<String> = args.iter().map(|a| a.replace("{main}", &main)).collect();
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            repo.git(&args);
        }
        let planned = planned_to(&repo, case.remote);
        let git = dry_run(&repo, case.remote);
        let name = case.name;
        let target = planned.target.as_ref().map(|(d, _)| d.as_str());
        let feat = |git: &BTreeSet<(String, String)>| {
            git.iter()
                .filter(|(src, _)| src == "refs/heads/feat")
                .map(|(_, dst)| dst.clone())
                .collect::<Vec<_>>()
        };
        match (&case.expect, &git.refused) {
            (To(dst), None) => {
                assert_eq!(target, Ok(*dst), "{name}");
                assert!(
                    feat(&git.sends).contains(&(*dst).to_owned()),
                    "{name}: git does not push feat to {dst}: {:?}",
                    git.sends
                );
                let terminal = planned.terminal.as_ref().unwrap();
                assert_eq!(sends(terminal, &git.sends), git.sends, "{name}");
                let only_this = git.sends.len() == 1 && !case.forced;
                assert_eq!(
                    planned.terminal_difference().is_none(),
                    only_this,
                    "{name}: {:?}",
                    planned.terminal_difference()
                );
            }
            (Refused(reason), Some(stderr)) => {
                assert_eq!(
                    planned.target.as_ref().map_err(|r| r.reason),
                    Err(*reason),
                    "{name}: git refused: {stderr}"
                );
                match &planned.terminal {
                    Err(terminal) => {
                        assert_eq!(terminal.reason, *reason, "{name}");
                        assert_eq!(planned.terminal_difference(), None, "{name}: both refuse");
                    }
                    // 남는 것이 없어 git 이 아무것도 보내지 않고 실패한다(「doing nothing」).
                    Ok(sends) => assert_eq!(sends, &[], "{name}: {stderr}"),
                }
            }
            (Refused(reason), None) => {
                // git 은 다른 ref 를 보내지만 이 브랜치는 보내지 않는다(또는 matching 이라 하지 않는다).
                assert_eq!(
                    planned.target.as_ref().map_err(|r| r.reason),
                    Err(*reason),
                    "{name}"
                );
                assert!(
                    feat(&git.sends).is_empty(),
                    "{name}: git pushes feat: {:?}",
                    git.sends
                );
                let terminal = planned.terminal.as_ref().unwrap();
                assert_eq!(sends(terminal, &git.sends), git.sends, "{name}");
                assert!(planned.terminal_difference().is_some(), "{name}");
            }
            (Stricter(reason), None) => {
                assert_eq!(
                    planned.target.as_ref().map_err(|r| r.reason),
                    Err(*reason),
                    "{name}"
                );
                assert!(
                    !feat(&git.sends).is_empty(),
                    "{name}: git does not push feat: {:?}",
                    git.sends
                );
                // 짧은 대상이면 터미널 몫은 git 과 같다(그 대상은 설정 원문 그대로) — 창은 git 이 보낼 것을 경고로 보인다.
                if *reason == R::UnqualifiedDestination {
                    let terminal = planned.terminal.as_ref().unwrap();
                    assert_eq!(sends(terminal, &git.sends), git.sends, "{name}");
                    assert!(planned.terminal_difference().is_some(), "{name}");
                }
            }
            (expect, refused) => panic!("{name}: expected {expect:?}, git: {refused:?}"),
        }
    }
}

#[test]
fn a_short_destination_names_the_full_ref_git_would_guess() {
    // 원격의 태그 v1 에 git 은 `refs/tags/v1` 로 보내고(이미 있어 거절된다), 원격을 보지 않는 셈은 `refs/heads/v1` 을 만들었을
    // 것이다 — 거절하고 고를 것을 보인다.
    let repo = feature_repo();
    repo.git(&["tag", "v1", "main"]);
    repo.git(&["push", "--quiet", "origin", "v1"]);
    repo.git(&["config", "remote.origin.push", "feat:v1"]);
    let git = dry_run(&repo, None);
    assert!(
        git.sends
            .contains(&("refs/heads/feat".to_owned(), "refs/tags/v1".to_owned())),
        "{:?}",
        git.sends
    );
    let planned = planned(&repo);
    let refusal = planned.target.clone().unwrap_err();
    assert_eq!(refusal.reason, R::UnqualifiedDestination);
    assert_eq!(
        refusal.candidates,
        ["refs/heads/v1", "refs/heads/main", "refs/heads/feat"]
    );
    assert!(refusal.message.contains("`v1`"), "{}", refusal.message);
    let terminal = planned.terminal_difference().unwrap();
    assert_eq!(terminal.sends, ["refs/heads/feat:v1"]);
    assert_eq!(terminal.refusal, None);
}

#[test]
fn a_detached_head_is_not_planned() {
    let repo = feature_repo();
    repo.git(&["checkout", "--quiet", "--detach"]);
    let opened = crate::repo::discover(repo.root(), true).unwrap().unwrap();
    assert!(plan_in(&opened, None, None, None).is_err());
    assert!(dry_run(&repo, None).refused.is_some(), "git refuses too");
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
        dry_run(&repo, None)
            .refused
            .unwrap()
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
        f.caller(),
        f.project.clone(),
        None,
        None,
        None,
        None,
        false,
        None,
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
