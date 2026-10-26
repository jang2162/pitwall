//! 재리뷰가 진짜 `git reset` 과 대조한 사례들을 고정 시험으로([`super::differ::check`] — hard · keep 둘 다 대조한다). 사례마다
//! git 이 실제로 무엇을 잃는지도 적어, 사례가 아무것도 잃지 않는 꼴로 무너지면 알게 한다.

use std::os::unix::fs::{PermissionsExt, symlink};

use super::differ::{Verdict, check};
use crate::testing::repo::TestRepo;

/// 대조해 놓친 것이 없음을 확인한 판정.
fn probe(r: &TestRepo, target: &str) -> Verdict {
    let v = check(r, target);
    v.assert_complete();
    v
}

fn listed(v: &super::differ::Side, entry: &str) -> bool {
    v.preview.changes.iter().any(|c| c.path == entry)
        || v.preview.overwritten.iter().any(|o| o == entry)
}

/// 대상 커밋과, 그 뒤에 `paths` 를 지운 커밋(HEAD).
fn target_then_drop(r: &TestRepo, files: &[(&str, &str)], drop: &[&str]) -> String {
    for (path, text) in files {
        r.write(path, text);
    }
    let target = r.commit_all("target");
    let mut args = vec!["rm", "-q", "-r", "--"];
    args.extend_from_slice(drop);
    r.git(&args);
    r.commit_all("drop");
    target
}

#[test]
fn a_tracked_then_ignored_file() {
    let r = TestRepo::with_commit();
    r.write(".env", "v1\n");
    let target = r.commit_all("track .env");
    r.git(&["rm", "-q", "--cached", ".env"]);
    r.write(".gitignore", ".env\n");
    r.commit_all("ignore .env");
    r.write(".env", "MY SECRET\n");
    let v = probe(&r, &target);
    assert_eq!(v.hard.lost, [".env"]);
    assert_eq!(v.keep.lost, [".env"], "keep overwrites an ignored file too");
    assert!(listed(&v.keep, ".env"));
}

#[test]
fn a_case_only_different_untracked_file() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("notes.md", "theirs\n")], &["notes.md"]);
    r.write("NOTES.md", "mine\n");
    let v = probe(&r, &target);
    if cfg!(target_os = "macos") {
        assert_eq!(v.hard.lost, ["NOTES.md"]);
    }
}

#[test]
fn a_case_only_different_untracked_folder() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("Docs/x.md", "theirs\n")], &["Docs"]);
    r.write("docs/x.md", "mine\n");
    let v = probe(&r, &target);
    if cfg!(target_os = "macos") {
        assert_eq!(v.hard.lost, ["docs/x.md"]);
    }
}

#[test]
fn an_untracked_file_where_the_target_has_a_folder() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("a/b.txt", "b\n")], &["a"]);
    r.write("a", "mine\n");
    let v = probe(&r, &target);
    assert_eq!(v.hard.lost, ["a"]);
    assert!(listed(&v.hard, "a"));
}

#[test]
fn a_deep_untracked_file_where_the_target_has_a_folder() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("p/q/r.txt", "r\n")], &["p"]);
    r.write("p/q", "mine\n");
    let v = probe(&r, &target);
    assert_eq!(v.hard.lost, ["p/q"]);
}

#[test]
fn a_file_in_a_nested_repository() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("d/f", "tracked\n")], &["d"]);
    std::fs::create_dir_all(r.root().join("d")).unwrap();
    r.git_in(&r.root().join("d"), &["init", "-q"]);
    r.write("d/f", "NESTED WORK\n");
    let v = probe(&r, &target);
    assert_eq!(v.hard.lost, ["d/f"]);
}

#[test]
fn a_nested_repository_where_the_target_has_a_file() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("n", "a file\n")], &["n"]);
    std::fs::create_dir_all(r.root().join("n")).unwrap();
    r.git_in(&r.root().join("n"), &["init", "-q"]);
    r.write("n/work", "nested\n");
    let v = probe(&r, &target);
    assert!(v.hard.lost.contains(&"n/work".to_owned()));
    assert!(listed(&v.hard, "n/"));
}

#[test]
fn submodule_edits_with_submodule_recurse() {
    let library = TestRepo::with_commit();
    let r = TestRepo::with_commit();
    r.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        library.root().to_str().unwrap(),
        "sub",
    ]);
    let target = r.commit_all("add the submodule");
    r.commit_file("x.txt", "x\n", "x");
    r.git(&["config", "submodule.recurse", "true"]);
    r.write("sub/README.md", "SUB EDIT\n");
    let v = probe(&r, &target);
    assert!(v.hard.lost.is_empty(), "{:?}", v.hard.lost);
}

#[test]
fn a_decomposed_unicode_name() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("caf\u{e9}.txt", "theirs\n")], &["caf\u{e9}.txt"]);
    r.write("cafe\u{301}.txt", "mine\n");
    let v = probe(&r, &target);
    if cfg!(target_os = "macos") {
        assert_eq!(v.hard.lost.len(), 1, "{:?}", v.hard.lost);
    }
}

#[test]
fn an_untracked_link_where_the_target_has_a_file() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("l", "file\n")], &["l"]);
    symlink("somewhere", r.root().join("l")).unwrap();
    let v = probe(&r, &target);
    assert_eq!(v.hard.lost, ["l"]);
}

#[test]
fn an_untracked_link_to_a_folder_where_the_target_has_a_folder() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("l/x", "x\n")], &["l"]);
    r.write("real/x", "mine\n");
    symlink("real", r.root().join("l")).unwrap();
    let v = probe(&r, &target);
    assert_eq!(v.hard.lost, ["l"]);
    assert_eq!(
        std::fs::read_to_string(r.root().join("real/x")).unwrap(),
        "mine\n"
    );
}

#[test]
fn a_tracked_link_replaced_by_a_file() {
    let r = TestRepo::with_commit();
    symlink("README.md", r.root().join("l")).unwrap();
    r.commit_all("link");
    std::fs::remove_file(r.root().join("l")).unwrap();
    r.write("l", "now a file\n");
    let v = probe(&r, "HEAD");
    assert_eq!(v.hard.lost, ["l"]);
}

#[test]
fn only_the_execute_bit_changed() {
    let r = TestRepo::with_commit();
    let path = r.root().join("README.md");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let v = probe(&r, "HEAD");
    assert!(listed(&v.hard, "README.md"));
}

#[test]
fn an_ignored_file_at_a_target_path() {
    let r = TestRepo::with_commit();
    r.write(".gitignore", "build/\n");
    r.commit_all("ignore build");
    r.write("build/out.txt", "theirs\n");
    r.git(&["add", "-f", "build/out.txt"]);
    let target = r.commit_all("force-add");
    r.git(&["rm", "-q", "--cached", "build/out.txt"]);
    r.commit_all("drop");
    r.write("build/out.txt", "mine\n");
    let v = probe(&r, &target);
    assert_eq!(v.hard.lost, ["build/out.txt"]);
    assert_eq!(v.keep.lost, ["build/out.txt"]);
}

#[test]
fn an_ignored_folder_where_the_target_has_a_file() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("build", "a file\n")], &["build"]);
    r.write(".gitignore", "build/\n");
    r.commit_all("ignore build");
    r.write("build/a/out.txt", "mine\n");
    let v = probe(&r, &target);
    assert_eq!(v.hard.lost, ["build/a/out.txt"]);
    assert!(listed(&v.hard, "build/"));
    assert_eq!(v.keep.lost, ["build/a/out.txt"], "keep deletes it too");
}

#[test]
fn a_file_renamed_on_disk_and_edited() {
    let r = TestRepo::with_commit();
    std::fs::rename(r.root().join("README.md"), r.root().join("moved.md")).unwrap();
    r.write("moved.md", "edited\n");
    probe(&r, "HEAD");
}

#[test]
fn a_file_renamed_with_git_mv_and_edited() {
    let r = TestRepo::with_commit();
    r.git(&["mv", "README.md", "moved.md"]);
    r.write("moved.md", "edited\n");
    let v = probe(&r, "HEAD");
    assert_eq!(v.hard.lost, ["moved.md"]);
}

#[test]
fn an_intent_to_add_file() {
    let r = TestRepo::with_commit();
    r.write("new.txt", "new work\n");
    r.git(&["add", "-N", "new.txt"]);
    let v = probe(&r, "HEAD");
    assert_eq!(v.hard.lost, ["new.txt"]);
}

#[test]
fn a_submodule_where_the_target_has_a_folder() {
    let r = TestRepo::with_commit();
    r.write("sub/README.md", "a plain folder\n");
    let target = r.commit_all("plain folder");
    r.git(&["rm", "-q", "-r", "sub"]);
    let library = TestRepo::with_commit();
    r.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        library.root().to_str().unwrap(),
        "sub",
    ]);
    r.commit_all("submodule");
    r.write("sub/README.md", "SUB EDIT\n");
    let v = probe(&r, &target);
    assert!(
        v.hard.lost.contains(&"sub/README.md".to_owned()),
        "{:?}",
        v.hard.lost
    );
}

#[test]
fn an_untracked_folder_where_a_tracked_file_was() {
    let r = TestRepo::with_commit();
    r.commit_file("a", "tracked\n", "a");
    std::fs::remove_file(r.root().join("a")).unwrap();
    r.write("a/notes.txt", "mine\n");
    let v = probe(&r, "HEAD");
    assert_eq!(v.hard.lost, ["a/notes.txt"]);
    assert!(listed(&v.hard, "a/"));
}

#[test]
fn an_untracked_file_where_a_tracked_folder_was() {
    let r = TestRepo::with_commit();
    r.commit_file("x/y.txt", "tracked\n", "x");
    std::fs::remove_dir_all(r.root().join("x")).unwrap();
    r.write("x", "mine\n");
    let v = probe(&r, "HEAD");
    assert_eq!(v.hard.lost, ["x"]);
    assert!(listed(&v.hard, "x"));
}

/// sparse checkout 의 콘 밖 파일을 다시 만들어 고쳤다. `git status` 를 먼저 돌렸는지(index 를 고친다)와 상관없이.
fn sparse(status_first: bool) {
    let r = TestRepo::with_commit();
    r.write("in/a.txt", "in\n");
    r.write("out/x.txt", "out\n");
    r.commit_all("two folders");
    r.git(&["sparse-checkout", "set", "--cone", "in"]);
    assert!(!r.root().join("out").exists());
    r.write("out/x.txt", "MINE\n");
    if status_first {
        r.git(&["status", "--porcelain"]);
    }
    let v = probe(&r, "HEAD");
    assert!(listed(&v.hard, "out/x.txt"), "{:?}", v.hard.preview);
}

#[test]
fn a_file_outside_the_sparse_cone() {
    sparse(false);
}

#[test]
fn a_file_outside_the_sparse_cone_after_a_status() {
    sparse(true);
}

/// index 플래그(`--assume-unchanged` · `--skip-worktree`)를 단 파일을 고쳤다.
fn flagged(flag: &str) -> Verdict {
    let r = TestRepo::with_commit();
    r.git(&["update-index", flag, "README.md"]);
    r.write("README.md", "MINE\n");
    assert_eq!(
        r.git(&["status", "--porcelain"]),
        "",
        "status does not see it"
    );
    let v = probe(&r, "HEAD");
    assert!(listed(&v.hard, "README.md"), "{:?}", v.hard.preview);
    v
}

#[test]
fn an_assume_unchanged_edit() {
    let v = flagged("--assume-unchanged");
    assert_eq!(v.hard.lost, ["README.md"]);
}

#[test]
fn a_skip_worktree_edit() {
    flagged("--skip-worktree");
}

/// [되돌리기] 사례: `first` 로 keep. HEAD 는 `second`.
fn undo_case(r: &TestRepo) -> (String, String) {
    let first = r.head();
    let second = r.commit_file("a.txt", "a\n", "a");
    (first, second)
}

#[test]
fn keep_refuses_over_an_untracked_file() {
    let r = TestRepo::with_commit();
    r.write("t.txt", "theirs\n");
    let target = r.commit_all("t");
    r.git(&["rm", "-q", "t.txt"]);
    r.commit_all("drop t");
    r.write("t.txt", "mine\n");
    let v = probe(&r, &target);
    assert!(!v.keep.succeeded, "git refuses");
    assert!(v.keep.lost.is_empty());
}

#[test]
fn keep_refuses_over_a_case_only_different_file() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("t.txt", "theirs\n")], &["t.txt"]);
    r.write("T.txt", "mine\n");
    let v = probe(&r, &target);
    assert!(v.keep.lost.is_empty(), "{:?}", v.keep.lost);
}

#[test]
fn keep_refuses_over_a_nested_repository() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("d/f", "tracked\n")], &["d"]);
    std::fs::create_dir_all(r.root().join("d")).unwrap();
    r.git_in(&r.root().join("d"), &["init", "-q"]);
    r.write("d/f", "NESTED WORK\n");
    let v = probe(&r, &target);
    assert!(v.keep.lost.is_empty(), "{:?}", v.keep.lost);
}

#[test]
fn keep_leaves_an_unrelated_edit() {
    let r = TestRepo::with_commit();
    let (first, _) = undo_case(&r);
    r.write("README.md", "edited\n");
    let v = probe(&r, &first);
    assert!(v.keep.succeeded);
    assert!(v.keep.lost.is_empty());
    assert!(v.keep.preview.is_empty(), "{:?}", v.keep.preview);
}

#[test]
fn keep_drops_staged_content_that_differs_from_the_disk() {
    let r = TestRepo::with_commit();
    let (first, _) = undo_case(&r);
    r.write("README.md", "STAGED\n");
    r.git(&["add", "README.md"]);
    r.write("README.md", "DISK\n");
    let v = probe(&r, &first);
    assert!(v.keep.succeeded);
    assert_eq!(v.keep.lost, ["README.md"]);
}

#[test]
fn keep_drops_a_staged_new_file_that_differs_from_the_disk() {
    let r = TestRepo::with_commit();
    let (first, _) = undo_case(&r);
    r.write("n.txt", "STAGED\n");
    r.git(&["add", "n.txt"]);
    r.write("n.txt", "DISK\n");
    let v = probe(&r, &first);
    assert_eq!(v.keep.lost, ["n.txt"]);
}

#[test]
fn keep_deletes_an_ignored_folder_where_the_target_has_a_file() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("a", "f\n")], &["a"]);
    r.write(".gitignore", "a/\n");
    r.commit_all("ignore a/");
    r.write("a/x", "MINE\n");
    let v = probe(&r, &target);
    assert_eq!(v.keep.lost, ["a/x"]);
    assert!(listed(&v.keep, "a/"));
}

#[test]
fn keep_replaces_an_ignored_file_where_the_target_has_a_folder() {
    let r = TestRepo::with_commit();
    let target = target_then_drop(&r, &[("a/b", "b\n")], &["a"]);
    r.write(".gitignore", "a\n");
    r.commit_all("ignore a");
    r.write("a", "MINE\n");
    let v = probe(&r, &target);
    assert_eq!(v.keep.lost, ["a"]);
}
