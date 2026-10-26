//! `git.blob`: 리비전 · index 의 파일 내용, 범위 읽기, `fs.read` 와 같은 판정, 큰 blob 의 동시 상한.

use std::time::Duration;

use pw_fs::methods::FsContent;

use super::*;
use crate::diff::blob::{BIG_BLOBS, BIG_BLOBS_AT_ONCE};

fn text(blob: &GitBlob) -> &str {
    match blob.content.as_ref().unwrap() {
        FsContent::Text { text } => text,
        FsContent::Base64 { .. } => panic!("expected text: {blob:?}"),
    }
}

#[test]
fn reads_a_file_at_a_revision_and_in_the_index() {
    let r = TestRepo::with_commit();
    r.commit_file("dir/a.txt", "hello\nworld\n", "a");
    r.write("dir/a.txt", "staged\n");
    r.git(&["add", "dir/a.txt"]);
    r.write("dir/a.txt", "work tree\n");
    let repo = Repo::new(r);

    let head = repo.blob(Some("HEAD"), "dir/a.txt");
    assert_eq!(text(&head), "hello\nworld\n");
    assert_eq!(head.id, repo.r.rev_parse("HEAD:dir/a.txt"));
    assert_eq!(
        (head.size, head.offset, head.eof, head.too_large),
        (12, 0, true, false)
    );
    assert_eq!(head.mode, "100644");
    let judgment = head.judgment.unwrap();
    assert!(judgment.editable && !judgment.binary);

    assert_eq!(text(&repo.blob(None, "dir/a.txt")), "staged\n");
    assert_eq!(text(&repo.blob(Some("HEAD~1"), "README.md")), "hello\n");

    // 범위.
    let mut params = repo.blob_params(Some("HEAD"), "dir/a.txt");
    params.offset = Some(6);
    params.length = Some(3);
    let range = repo.f.git.blob(&params).unwrap();
    assert_eq!((text(&range), range.offset, range.eof), ("wor", 6, false));
    params.offset = Some(100);
    params.length = None;
    let past = repo.f.git.blob(&params).unwrap();
    assert_eq!((text(&past), past.offset, past.eof), ("", 12, true));

    // 바이트로.
    let mut params = repo.blob_params(Some("HEAD"), "dir/a.txt");
    params.bytes = true;
    assert_eq!(
        repo.f.git.blob(&params).unwrap().content,
        Some(FsContent::Base64 {
            data: "aGVsbG8Kd29ybGQK".into()
        })
    );
}

#[test]
fn binary_and_bom_content_are_judged_like_fs_read() {
    let r = TestRepo::with_commit();
    r.write("bin.dat", "ab\0cd");
    r.write("bom.txt", "\u{feff}text\n");
    r.commit_all("files");
    let repo = Repo::new(r);
    let bin = repo.blob(Some("HEAD"), "bin.dat");
    assert!(bin.judgment.as_ref().unwrap().binary);
    assert_eq!(
        bin.content,
        Some(FsContent::Base64 {
            data: "YWIAY2Q=".into()
        })
    );
    let bom = repo.blob(Some("HEAD"), "bom.txt");
    assert_eq!(text(&bom), "text\n");
    assert!(bom.judgment.as_ref().unwrap().bom.is_some());
}

#[cfg(unix)]
#[test]
fn a_symlink_reads_as_its_target_path() {
    let r = TestRepo::with_commit();
    std::os::unix::fs::symlink("README.md", r.root().join("link")).unwrap();
    r.commit_all("link");
    let repo = Repo::new(r);
    let link = repo.blob(Some("HEAD"), "link");
    assert_eq!((link.mode.as_str(), text(&link)), ("120000", "README.md"));
}

#[test]
fn folders_submodules_missing_paths_and_long_ranges_are_refused() {
    let r = TestRepo::with_commit();
    r.commit_file("dir/a.txt", "a\n", "a");
    let sub = r.root().join("sub");
    fs::create_dir_all(&sub).unwrap();
    r.git_in(&sub, &["init", "-q"]);
    r.commit_all_in(&sub, "s");
    r.commit_all("sub");
    let repo = Repo::new(r);
    let err = |rev: Option<&str>, path: &str| {
        repo.f
            .git
            .blob(&repo.blob_params(rev, path))
            .unwrap_err()
            .code
    };
    assert_eq!(err(Some("HEAD"), "dir"), pw_rpc::code::INVALID_PARAMS);
    assert_eq!(err(Some("HEAD"), "sub"), pw_rpc::code::INVALID_PARAMS);
    assert_eq!(err(None, "sub"), pw_rpc::code::INVALID_PARAMS);
    assert_eq!(err(Some("HEAD"), "nope.txt"), pw_rpc::code::NOT_FOUND);
    assert_eq!(err(None, "nope.txt"), pw_rpc::code::NOT_FOUND);
    assert_eq!(
        err(Some("HEAD"), "/dir/a.txt"),
        pw_rpc::code::INVALID_PARAMS
    );
    assert_eq!(err(Some("nope"), "dir/a.txt"), pw_rpc::code::INVALID_PARAMS);
    let mut params = repo.blob_params(Some("HEAD"), "dir/a.txt");
    params.length = Some(pw_fs::limits::READ_ONLY_MAX + 1);
    assert_eq!(
        repo.f.git.blob(&params).unwrap_err().code,
        pw_rpc::code::INVALID_PARAMS
    );
}

/// 5 MiB 넘는 blob 은 Host 전체에서 [`BIG_BLOBS_AT_ONCE`] 개까지만 함께 푼다: 자리가 다 차 있으면 큰 blob 읽기는 기다리다
/// 「바쁨」 으로 끝나고, 작은 blob 은 막히지 않으며, 자리가 나면 큰 blob 도 읽힌다.
#[test]
fn large_blobs_wait_for_a_seat() {
    let r = TestRepo::with_commit();
    r.write("six.txt", &"x".repeat(6 * 1024 * 1024));
    r.commit_all("six");
    let repo = Repo::new(r);
    let seats: Vec<_> = (0..BIG_BLOBS_AT_ONCE)
        .map(|_| BIG_BLOBS.enter(Duration::ZERO).unwrap())
        .collect();
    let err = repo
        .f
        .git
        .blob(&repo.blob_params(Some("HEAD"), "six.txt"))
        .unwrap_err();
    assert!(format!("{:?}", err.data).contains("busy"), "{err:?}");
    assert_eq!(text(&repo.blob(Some("HEAD"), "README.md")), "hello\n");
    drop(seats);
    assert_eq!(repo.blob(Some("HEAD"), "six.txt").size, 6 * 1024 * 1024);
}
