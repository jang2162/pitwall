//! `git.show` · `git.containedIn` 을 같은 저장소의 git 출력과 견준다.

use std::collections::BTreeSet;

use super::*;

#[test]
fn show_reports_the_commit_like_git() {
    let r = TestRepo::with_commit();
    r.write("a.txt", "a\n");
    r.git(&["add", "a.txt"]);
    r.git(&[
        "commit",
        "-q",
        "--author=Ann Lee <ann@example.com>",
        "--date=@1700000123 +0930",
        "-m",
        "Subject line",
        "-m",
        "Body line one.\nBody line two.",
    ]);
    r.branch("topic");
    r.git(&["tag", "-a", "v1", "-m", "tag message"]);
    r.git(&["tag", "light"]);
    r.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    r.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    let repo = Repo::new(r);
    let got = repo.f.git.show(&repo.rev("HEAD")).unwrap();

    let format = |f: &str| {
        repo.r
            .git(&["show", "-s", &format!("--format={f}"), "HEAD"])
            .trim_end()
            .to_owned()
    };
    assert_eq!(got.commit, format("%H"));
    assert_eq!(got.tree, format("%T"));
    assert_eq!(got.parents, [format("%P")]);
    assert_eq!(
        (
            got.author.name.as_str(),
            got.author.email.as_str(),
            got.author.time,
            got.author.offset
        ),
        (
            "Ann Lee",
            "ann@example.com",
            1_700_000_123,
            9 * 3600 + 30 * 60
        )
    );
    assert_eq!(got.committer.name, format("%cn"));
    assert_eq!(got.committer.time.to_string(), format("%ct"));
    let raw = repo.r.git(&["cat-file", "commit", "HEAD"]);
    assert_eq!(got.message, raw.split_once("\n\n").unwrap().1);
    assert!(
        got.message
            .starts_with("Subject line\n\nBody line one.\nBody line two.")
    );
    assert!(got.head);
    let labels: Vec<(RefKind, &str, &str)> = got
        .refs
        .iter()
        .map(|l| (l.kind, l.name.as_str(), l.full_name.as_str()))
        .collect();
    assert_eq!(
        labels,
        [
            (RefKind::LocalBranch, "main", "refs/heads/main"),
            (RefKind::LocalBranch, "topic", "refs/heads/topic"),
            (
                RefKind::RemoteBranch,
                "origin/main",
                "refs/remotes/origin/main"
            ),
            (RefKind::Tag, "light", "refs/tags/light"),
            (RefKind::Tag, "v1", "refs/tags/v1"),
        ]
    );
    // git 이 그 커밋을 가리킨다고 보는 ref(심볼릭 `origin/HEAD` 빼고)와 같다.
    let points_at: BTreeSet<String> = repo
        .r
        .git(&[
            "for-each-ref",
            "--points-at",
            "HEAD",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])
        .lines()
        .chain(
            repo.r
                .git(&[
                    "for-each-ref",
                    "--format=%(refname)",
                    "--points-at",
                    "v1^{}",
                    "refs/tags",
                ])
                .lines(),
        )
        .filter(|r| *r != "refs/remotes/origin/HEAD")
        .map(str::to_owned)
        .collect();
    let ours: BTreeSet<String> = got.refs.iter().map(|l| l.full_name.clone()).collect();
    assert_eq!(ours, points_at);

    // 앞 커밋은 ref 가 없고 HEAD 도 아니다.
    let parent = repo.f.git.show(&repo.rev("HEAD~1")).unwrap();
    assert!(parent.refs.is_empty() && !parent.head);
}

#[test]
fn show_lists_both_parents_of_a_merge_in_order() {
    let r = TestRepo::with_commit();
    r.git(&["checkout", "-q", "-b", "side"]);
    r.commit_file("s.txt", "s\n", "side");
    r.checkout("main");
    r.commit_file("m.txt", "m\n", "main");
    r.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
    let repo = Repo::new(r);
    let got = repo.f.git.show(&repo.rev("HEAD")).unwrap();
    let expected: Vec<String> = repo
        .r
        .git(&["rev-list", "--parents", "-n1", "HEAD"])
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect();
    assert_eq!(got.parents, expected);
    assert_eq!(got.parents[0], repo.r.rev_parse("HEAD^1"));
}

#[test]
fn contained_in_lists_the_branches_and_tags_git_lists() {
    // main: A - B - C, side: B - D, old: A, origin/main: B, 태그 t-c: C, 주석 태그 t-a: A.
    let r = TestRepo::with_commit();
    let a = r.head();
    r.branch("old");
    r.git(&["tag", "-a", "t-a", "-m", "at a"]);
    let b = r.commit_file("b.txt", "b\n", "B");
    r.git(&["update-ref", "refs/remotes/origin/main", &b]);
    r.git(&["checkout", "-q", "-b", "side"]);
    r.commit_file("d.txt", "d\n", "D");
    r.checkout("main");
    r.commit_file("c.txt", "c\n", "C");
    r.git(&["tag", "t-c"]);
    let repo = Repo::new(r);
    let git_contains = |rev: &str| -> BTreeSet<String> {
        let branches = repo
            .r
            .git(&["branch", "-a", "--contains", rev, "--format=%(refname)"]);
        let tags = repo
            .r
            .git(&["tag", "--contains", rev, "--format=%(refname)"]);
        branches
            .lines()
            .chain(tags.lines())
            .map(str::to_owned)
            .collect()
    };
    for rev in [a.as_str(), b.as_str(), "HEAD", "side"] {
        let got = repo.f.git.contained_in(&repo.rev(rev)).unwrap();
        let names: BTreeSet<String> = got.refs.iter().map(|l| l.full_name.clone()).collect();
        assert_eq!(names, git_contains(rev), "contains {rev}");
    }
    let got = repo.f.git.contained_in(&repo.rev(&b)).unwrap();
    let names: Vec<&str> = got.refs.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["main", "side", "origin/main", "t-c"]);
}

#[test]
fn an_unknown_revision_is_invalid_params() {
    let repo = Repo::new(TestRepo::with_commit());
    for err in [
        repo.f.git.show(&repo.rev("nope")).unwrap_err(),
        repo.f.git.contained_in(&repo.rev("nope")).unwrap_err(),
    ] {
        assert_eq!(err.code, pw_rpc::code::INVALID_PARAMS);
    }
}
