//! 파일 단위: `git.stage` · `git.unstage` 가 untracked · 지운 파일 · 충돌 파일을 다루고 경로를 글자 그대로 보며, 커밋이 없는
//! 저장소에서도 언스테이징된다. `git.discard` 는 확인이 있어야 하고 untracked 를 지우지 않고 돌려준다.

use pw_rpc::code as rpc_code;

use super::{bytes, proceed};
use crate::stage::methods::{GitDiscardParams, GitPathsParams};
use crate::testing::TestRepo;
use crate::testing::write::WriteFixture;
use crate::write::{Askpass, WriteError};

fn paths(f: &WriteFixture, paths: &[&str]) -> GitPathsParams {
    GitPathsParams {
        project_id: f.project.clone(),
        worktree: None,
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
    }
}

/// `git status --porcelain` 의 줄(경로순).
fn status(f: &WriteFixture) -> String {
    f.repo
        .git(&["status", "--porcelain=v1", "--untracked-files=all"])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untracked_deleted_and_modified_files_stage_and_unstage() {
    let f = WriteFixture::new(true);
    f.repo.commit_file("gone.txt", "bye\n", "gone");
    f.repo.write("README.md", "changed\n");
    f.repo.write("dir/new.txt", "new\n");
    std::fs::remove_file(f.repo.root().join("gone.txt")).unwrap();
    let before = status(&f);
    assert_eq!(before, " M README.md\n D gone.txt\n?? dir/new.txt\n");

    f.write
        .stage(paths(&f, &["README.md", "gone.txt", "dir"]))
        .await
        .unwrap();
    assert_eq!(status(&f), "M  README.md\nA  dir/new.txt\nD  gone.txt\n");

    let done = f
        .write
        .unstage(paths(&f, &["README.md", "gone.txt", "dir/new.txt"]))
        .await
        .unwrap();
    assert_eq!(status(&f), before);
    assert_eq!(done.work_tree, f.repo.root().display().to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicted_file_is_marked_resolved_and_unstaging_puts_head_back() {
    let f = WriteFixture::new(true);
    let d = f.repo.start_conflicting_merge();
    // 충돌 중인 파일은 줄 단위로 스테이징하지 않는다(파일 단위로 해결한다).
    let any_hunk = crate::diff::methods::DiffHunk {
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 1,
        header: String::new(),
        lines: Vec::new(),
    };
    let refused = f
        .write
        .stage_lines(crate::stage::methods::GitLinesParams {
            project_id: f.project.clone(),
            worktree: None,
            path: d.path.clone(),
            old_path: None,
            hunks: vec![crate::stage::methods::HunkSelection {
                hunk: any_hunk,
                lines: None,
            }],
            ignore_whitespace: false,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, WriteError::Rpc(e) if e.code == rpc_code::INVALID_PARAMS),
        "{refused:?}"
    );
    f.repo.write(&d.path, "resolved\n");
    f.write.stage(paths(&f, &[&d.path])).await.unwrap();
    assert_eq!(
        f.repo
            .git(&["ls-files", "-s", "--", &d.path])
            .split('\t')
            .next()
            .unwrap()
            .split(' ')
            .nth(2),
        Some("0"),
        "one stage-0 entry after staging"
    );
    assert_eq!(bytes(&f, &["show", &format!(":{}", d.path)]), b"resolved\n");

    f.write.unstage(paths(&f, &[&d.path])).await.unwrap();
    assert_eq!(bytes(&f, &["show", &format!(":{}", d.path)]), b"ours\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paths_are_literal_and_unstaging_works_before_the_first_commit() {
    let f = WriteFixture::with_repo(TestRepo::new(), true, Askpass::default());
    f.repo.write("x*.txt", "star\n");
    f.repo.write("xy.txt", "plain\n");
    f.repo.write("line\nbreak.txt", "newline in the name\n");
    f.write
        .stage(paths(&f, &["x*.txt", "line\nbreak.txt"]))
        .await
        .unwrap();
    assert_eq!(
        f.repo.git(&["ls-files", "-z", "--cached"]),
        "line\nbreak.txt\0x*.txt\0",
        "`x*.txt` is a name, not a glob"
    );

    // HEAD 가 아직 없다 — 언스테이징은 index 에서 뺀다.
    f.write.unstage(paths(&f, &["x*.txt"])).await.unwrap();
    assert_eq!(
        f.repo.git(&["ls-files", "-z", "--cached"]),
        "line\nbreak.txt\0"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discarding_needs_confirmation_restores_tracked_files_and_leaves_untracked_ones() {
    let f = WriteFixture::new(true);
    f.repo.commit_file("gone.txt", "bye\n", "gone");
    f.repo.write("README.md", "changed\n");
    std::fs::remove_file(f.repo.root().join("gone.txt")).unwrap();
    f.repo.write("loose.txt", "untracked\n");
    let request = |confirmed| GitDiscardParams {
        project_id: f.project.clone(),
        worktree: None,
        paths: vec!["loose.txt".into(), "README.md".into(), "gone.txt".into()],
        confirmed,
    };

    let refused = f.write.discard(request(false)).await.unwrap_err();
    assert!(
        matches!(&refused, WriteError::Rpc(e) if e.code == rpc_code::INVALID_PARAMS),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("README.md")).unwrap(),
        "changed\n"
    );

    let window = proceed(&f);
    let done = f.write.discard(request(true)).await.unwrap();
    assert_eq!(done.untracked, ["loose.txt"]);
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("README.md")).unwrap(),
        "hello\n"
    );
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("gone.txt")).unwrap(),
        "bye\n"
    );
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("loose.txt")).unwrap(),
        "untracked\n",
        "an untracked file is not deleted"
    );
    // 미저장 확인에는 요청한 경로가 그대로 갔다.
    assert_eq!(
        window.seen(),
        [crate::write::GitPromptRequest::Unsaved {
            work_tree: f.repo.root().display().to_string(),
            paths: Some(vec![
                "loose.txt".into(),
                "README.md".into(),
                "gone.txt".into()
            ]),
        }]
    );

    // 모두 untracked 면 git 을 돌리지 않고 그대로 돌려준다.
    let only = f
        .write
        .discard(GitDiscardParams {
            project_id: f.project.clone(),
            worktree: None,
            paths: vec!["loose.txt".into()],
            confirmed: true,
        })
        .await
        .unwrap();
    assert_eq!(only.untracked, ["loose.txt"]);
    assert!(f.repo.root().join("loose.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_paths_are_refused_before_git_runs() {
    let f = WriteFixture::new(true);
    for bad in ["", "/abs", "../up", "a//b", "a/./b", "a\\b", "dir/"] {
        let refused = f.write.stage(paths(&f, &[bad])).await.unwrap_err();
        assert!(
            matches!(&refused, WriteError::Rpc(e) if e.code == rpc_code::INVALID_PARAMS),
            "{bad:?}: {refused:?}"
        );
    }
    let refused = f.write.stage(paths(&f, &[])).await.unwrap_err();
    assert!(matches!(&refused, WriteError::Rpc(e) if e.code == rpc_code::INVALID_PARAMS));
}
