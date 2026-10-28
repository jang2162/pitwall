//! 보낼 커밋: `git rev-list <base>..<branch>` 와 같은 커밋 · 차례, 원격에 없는 브랜치는 그 원격의 원격 추적 ref 전부를 뺀다,
//! `limit` 에서 자르고 알린다, 미신뢰 프로젝트에서도 git 을 띄우지 않고 저장소 설정의 프로그램을 돌리지 않는다.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::testing::FakeGit;
use crate::testing::write::WriteFixture;
use crate::write::Askpass;

fn params(
    f: &WriteFixture,
    branch: &str,
    base: Option<String>,
    limit: u32,
) -> GitPushCommitsParams {
    GitPushCommitsParams {
        project_id: f.project.clone(),
        worktree: None,
        branch: branch.into(),
        remote: "origin".into(),
        base,
        limit,
    }
}

fn hashes(commits: &GitPushCommits) -> Vec<String> {
    commits.commits.iter().map(|c| c.hash.clone()).collect()
}

/// `git rev-list` 의 줄(새것부터).
fn rev_list(f: &WriteFixture, args: &[&str]) -> Vec<String> {
    let mut all = vec!["rev-list"];
    all.extend_from_slice(args);
    f.repo
        .git(&all)
        .lines()
        .map(str::to_owned)
        .filter(|l| !l.is_empty())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_outgoing_commits_are_the_ones_git_lists_after_the_remote_tracking_commit() {
    let f = WriteFixture::new(true);
    f.repo.add_bare_remote("origin");
    for i in 0..3 {
        f.repo.commit_file(
            &format!("f{i}.txt"),
            &format!("{i}\n"),
            &format!("local {i}"),
        );
    }
    let base = f.repo.rev_parse("refs/remotes/origin/main");
    let got = f
        .write
        .git()
        .push_commits(&params(&f, "main", Some(base.clone()), 50))
        .unwrap();
    let expected = rev_list(&f, &[&format!("{base}..main")]);
    assert_eq!(expected.len(), 3);
    assert_eq!(hashes(&got), expected);
    assert!(!got.truncated);
    assert_eq!(got.commits[0].subject, "local 2");
    assert_eq!(got.commits[0].author, "Pitwall Test");
    assert_eq!(
        got.work_tree.as_deref(),
        Some(f.repo.root().display().to_string().as_str())
    );

    // 잘라서 알린다.
    let cut = f
        .write
        .git()
        .push_commits(&params(&f, "main", Some(base), 2))
        .unwrap();
    assert_eq!(hashes(&cut), expected[..2]);
    assert!(cut.truncated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_branch_the_remote_lacks_leaves_out_what_any_tracking_ref_of_that_remote_has() {
    let f = WriteFixture::new(true);
    f.repo.add_bare_remote("origin");
    f.repo.git(&["checkout", "--quiet", "-b", "feat"]);
    f.repo.commit_file("a.txt", "a\n", "feat a");
    f.repo.commit_file("b.txt", "b\n", "feat b");
    let got = f
        .write
        .git()
        .push_commits(&params(&f, "feat", None, 50))
        .unwrap();
    let expected = rev_list(&f, &["feat", "--not", "--remotes=origin"]);
    assert_eq!(expected.len(), 2);
    assert_eq!(hashes(&got), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_branches_commits_and_limits_are_invalid_params() {
    let f = WriteFixture::new(true);
    f.repo.add_bare_remote("origin");
    let git = f.write.git();
    for p in [
        params(&f, "nope", None, 10),
        params(&f, "main", Some("0".repeat(40)), 10),
        params(&f, "main", Some("xyz".into()), 10),
        params(&f, "main", None, 0),
        params(&f, "main", None, MAX_COMMITS + 1),
        params(&f, "../main", None, 10),
        params(&f, "-x", None, 10),
        GitPushCommitsParams {
            remote: "origin/../x".into(),
            ..params(&f, "main", None, 10)
        },
    ] {
        let e = git.push_commits(&p).unwrap_err();
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS, "{p:?}: {e:?}");
    }
}

/// 미신뢰: PATH 맨 앞의 가짜 `git` 이 불리지 않고(프로세스 0), 저장소 설정에 심은 프로그램도 돌지 않는다. 결과는 신뢰했을 때와
/// 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_lists_with_gix_only() {
    let repo = crate::testing::TestRepo::with_commit();
    let f = WriteFixture::with_repo(repo, false, Askpass::default());
    f.repo.add_bare_remote("origin");
    f.repo.commit_file("x.txt", "x\n", "local");
    let marks = f.repo.base().join("marks");
    fs::create_dir_all(&marks).unwrap();
    let planted = f.repo.base().join("planted.sh");
    fs::write(
        &planted,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}/fsmonitor'\nexit 1\n",
            marks.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&planted, fs::Permissions::from_mode(0o755)).unwrap();
    f.repo
        .git(&["config", "core.fsmonitor", planted.to_str().unwrap()]);
    let fake = FakeGit::new(&f.bin, "git", "2.54.0");
    assert!(!f.projects.is_trusted(&f.project));

    let base = f.repo.rev_parse("refs/remotes/origin/main");
    let got = f
        .write
        .git()
        .push_commits(&params(&f, "main", Some(base.clone()), 10))
        .unwrap();
    assert_eq!(hashes(&got), rev_list(&f, &[&format!("{base}..main")]));
    assert_eq!(fake.received_args(), None, "git did not run");
    assert_eq!(fs::read_dir(&marks).unwrap().count(), 0, "nothing ran");
}
