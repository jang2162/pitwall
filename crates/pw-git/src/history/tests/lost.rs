//! reset hard 의 잃을 변경 목록이 git 이 실제로 덮어쓰는 것을 빠뜨리지 않는다: 추적하다 ignore 로 뺀 파일, 대상에서 파일이 되는
//! 폴더 · 폴더가 되는 파일, 중첩 저장소, macOS 의 대소문자, `submodule.recurse` 의 서브모듈 편집. 확인한 뒤 그사이 바뀌면 지문이
//! 거절한다.

use pw_rpc::code as rpc_code;

use super::super::ResetMode;
use super::super::methods::{GitResetParams, reset, reset_preview};
use super::reset::{confirmed_hard, preview_params, read};
use super::window;
use crate::testing::repo::TestRepo;
use crate::testing::write::WriteFixture;
use crate::write::Askpass;

/// 추적하던 파일을 `git rm --cached` 와 `.gitignore` 로 뺀 커밋 전으로 되돌리면 git 은 ignore 된 로컬 파일을 묻지 않고 덮어쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_tracked_then_ignored_is_overwritten() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("secret.txt", "v1\n");
    let tracked = r.commit_all("track the secret");
    r.git(&["rm", "-q", "--cached", "secret.txt"]);
    r.write(".gitignore", "secret.txt\n");
    r.commit_all("ignore the secret");
    r.write("secret.txt", "MY LOCAL SECRET\n");
    assert_eq!(
        r.git(&["status", "--porcelain"]),
        "",
        "status shows nothing"
    );

    let (preview, done) = confirmed_hard(&f, &tracked).await;
    assert!(preview.lost.changes.is_empty());
    assert_eq!(preview.lost.overwritten, ["secret.txt"]);
    done.unwrap();
    assert_eq!(
        read(&f, "secret.txt").as_deref(),
        Some("v1\n"),
        "overwritten"
    );
}

/// 대상에서 파일인 경로가 지금은 untracked 파일이 든 폴더면 git 은 폴더째 지운다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_that_becomes_a_file_is_lost_with_its_untracked_files() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("a", "a file\n");
    let target = r.commit_all("a is a file");
    r.git(&["rm", "-q", "a"]);
    r.commit_all("drop a");
    r.write("a/b.txt", "notes\n");

    let (preview, done) = confirmed_hard(&f, &target).await;
    assert_eq!(preview.lost.overwritten, ["a/"]);
    done.unwrap();
    assert_eq!(read(&f, "a").as_deref(), Some("a file\n"));
    assert_eq!(read(&f, "a/b.txt"), None, "deleted");
}

/// 대상에서 폴더인 경로가 지금은 untracked 파일이면 git 은 그 파일을 지운다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_where_the_target_has_a_folder_is_lost() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("x/y.txt", "y\n");
    let target = r.commit_all("x is a folder");
    r.git(&["rm", "-q", "-r", "x"]);
    r.commit_all("drop x");
    r.write("x", "notes\n");

    let (preview, done) = confirmed_hard(&f, &target).await;
    assert_eq!(preview.lost.overwritten, ["x"]);
    done.unwrap();
    assert_eq!(read(&f, "x/y.txt").as_deref(), Some("y\n"));
}

/// 중첩 저장소(untracked 폴더 하나로 보인다)의 파일도 대상 트리의 경로면 git 이 덮어쓴다. 대상에서 파일인 경로의 중첩 저장소는
/// 폴더째다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_in_a_nested_repository_are_overwritten() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("d/f", "tracked\n");
    r.write("n", "a file\n");
    let target = r.commit_all("d/f and n");
    r.git(&["rm", "-q", "-r", "d", "n"]);
    r.commit_all("drop d and n");
    for nested in ["d", "n"] {
        let dir = r.root().join(nested);
        std::fs::create_dir_all(&dir).unwrap();
        r.git_in(&dir, &["init", "-q"]);
    }
    r.write("d/f", "NESTED WORK\n");
    r.write("d/other", "untouched\n");
    r.write("n/work", "nested\n");

    let (preview, done) = confirmed_hard(&f, &target).await;
    assert_eq!(preview.lost.overwritten, ["d/f", "n/"]);
    done.unwrap();
    assert_eq!(read(&f, "d/f").as_deref(), Some("tracked\n"), "overwritten");
    assert_eq!(read(&f, "d/other").as_deref(), Some("untouched\n"));
    assert_eq!(read(&f, "n").as_deref(), Some("a file\n"));
}

/// 대소문자를 무시하는 파일 시스템(macOS 기본)에서 대상의 `readme.md` 는 untracked `README.md` 를 덮어쓴다 — 디스크의 이름으로
/// 싣는다. 대소문자만 바꾼 이름 바꾸기(추적 중인 `README.md` → 대상의 `readme.md`)는 잃는 것이 없다.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_case_only_different_untracked_file_is_overwritten() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("notes.md", "theirs\n");
    let target = r.commit_all("lower case");
    r.git(&["rm", "-q", "notes.md"]);
    r.commit_all("drop it");
    r.write("NOTES.md", "my notes\n");

    let (preview, done) = confirmed_hard(&f, &target).await;
    assert_eq!(preview.lost.overwritten, ["NOTES.md"]);
    done.unwrap();
    assert_eq!(read(&f, "notes.md").as_deref(), Some("theirs\n"));

    // README.md(추적, 바꾸지 않음) → 대상에는 readme.md 만.
    let r = &f.repo;
    r.git(&["mv", "README.md", "tmp"]);
    r.git(&["mv", "tmp", "readme.md"]);
    let lower = r.commit_all("rename to lower case");
    r.git(&["mv", "readme.md", "tmp"]);
    r.git(&["mv", "tmp", "README.md"]);
    r.commit_all("rename back");
    let preview = reset_preview(f.write.clone(), preview_params(&f, &lower))
        .await
        .unwrap();
    assert!(preview.lost.is_empty(), "{:?}", preview.lost);
}

/// 사람의 설정이 `submodule.recurse=true` 여도 reset 은 서브모듈 작업 트리를 건드리지 않는다 — 목록이 서브모듈 안을 세지 않는 것과
/// 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submodule_edits_survive_even_with_submodule_recurse() {
    let library = TestRepo::with_commit();
    let repo = TestRepo::with_commit();
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        library.root().to_str().unwrap(),
        "sub",
    ]);
    let target = repo.commit_all("add the submodule");
    repo.commit_file("x.txt", "x\n", "x");
    repo.git(&["config", "submodule.recurse", "true"]);
    let f = WriteFixture::with_repo(repo, true, Askpass::default());
    let _window = window(&f);
    f.repo.write("sub/README.md", "SUB LOCAL EDIT\n");

    let (preview, done) = confirmed_hard(&f, &target).await;
    assert!(preview.lost.is_empty(), "{:?}", preview.lost);
    done.unwrap();
    assert_eq!(read(&f, "x.txt"), None);
    assert_eq!(
        read(&f, "sub/README.md").as_deref(),
        Some("SUB LOCAL EDIT\n"),
        "the submodule work tree is untouched"
    );
}

fn hard(f: &WriteFixture, commit: &str, fingerprint: &str) -> GitResetParams {
    GitResetParams {
        project_id: f.project.clone(),
        worktree: None,
        commit: commit.to_owned(),
        mode: ResetMode::Hard,
        expected: None,
        expected_ref: None,
        fingerprint: Some(fingerprint.to_owned()),
    }
}

/// 확인한 뒤 잃을 파일을 더 고치거나 덮어써질 파일이 새로 생기면 아무것도 하지 않고 conflict 다 — `data` 에 새 목록과 지문.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changes_made_after_the_confirmation_are_not_reset_away() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    let first = r.head();
    r.write("a.txt", "a\n");
    let second = r.commit_all("a");
    r.write("README.md", "edited\n");
    let preview = reset_preview(f.write.clone(), preview_params(&f, &first))
        .await
        .unwrap();

    // 같은 목록, 다른 내용.
    r.write("README.md", "edited much more\n");
    let e = reset(f.write.clone(), hard(&f, &first, &preview.fingerprint))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    let data = e.data.unwrap();
    assert_eq!(data["lost"]["changes"][0]["path"], "README.md");
    let fresh = data["fingerprint"].as_str().unwrap().to_owned();
    assert_ne!(fresh, preview.fingerprint);
    assert_eq!(r.head(), second);
    assert_eq!(read(&f, "README.md").as_deref(), Some("edited much more\n"));

    // 덮어써질 untracked 파일이 새로 생겼다.
    r.git(&["rm", "-q", "a.txt"]);
    r.commit_all("drop a");
    let preview = reset_preview(f.write.clone(), preview_params(&f, &second))
        .await
        .unwrap();
    r.write("a.txt", "new work\n");
    let e = reset(f.write.clone(), hard(&f, &second, &preview.fingerprint))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    assert_eq!(e.data.unwrap()["lost"]["overwritten"][0], "a.txt");
    assert_eq!(read(&f, "a.txt").as_deref(), Some("new work\n"));
}
