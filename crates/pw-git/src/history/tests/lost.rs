//! reset hard 의 잃을 변경 목록이 git 이 실제로 덮어쓰는 것을 빠뜨리지 않는다: 추적하다 ignore 로 뺀 파일, 대상에서 파일이 되는
//! 폴더 · 폴더가 되는 파일, 중첩 저장소, macOS 의 대소문자, `submodule.recurse` 의 서브모듈 편집(더 많은 사례와 퍼즈는
//! `probes` · `fuzz`). 확인한 뒤 그사이 바뀌면(폴더째 잃는 항목의 안도) 지문이 거절한다. [되돌리기] 의 keep 과 mixed 도 잃을
//! 것이 있으면 묻는다.

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
    let e = reset(
        f.write.clone(),
        f.caller(),
        hard(&f, &first, preview.fingerprint.as_deref().unwrap()),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    let data = e.data.unwrap();
    assert_eq!(data["lost"]["changes"][0]["path"], "README.md");
    let fresh = data["fingerprint"].as_str().unwrap().to_owned();
    assert_ne!(Some(fresh), preview.fingerprint);
    assert_eq!(r.head(), second);
    assert_eq!(read(&f, "README.md").as_deref(), Some("edited much more\n"));

    // 덮어써질 untracked 파일이 새로 생겼다.
    r.git(&["rm", "-q", "a.txt"]);
    r.commit_all("drop a");
    let preview = reset_preview(f.write.clone(), preview_params(&f, &second))
        .await
        .unwrap();
    r.write("a.txt", "new work\n");
    let e = reset(
        f.write.clone(),
        f.caller(),
        hard(&f, &second, preview.fingerprint.as_deref().unwrap()),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    assert_eq!(e.data.unwrap()["lost"]["overwritten"][0], "a.txt");
    assert_eq!(read(&f, "a.txt").as_deref(), Some("new work\n"));
}

/// [되돌리기] 의 keep: `previous` 로, HEAD 가 `current` · `refName` 일 때만.
fn undo(f: &WriteFixture, previous: &str, fingerprint: Option<String>) -> GitResetParams {
    GitResetParams {
        project_id: f.project.clone(),
        worktree: None,
        commit: previous.to_owned(),
        mode: ResetMode::Keep,
        expected: Some(f.repo.head()),
        expected_ref: Some("refs/heads/main".into()),
        fingerprint,
    }
}

/// 되돌릴 곳이 추적하던 파일을 지금은 ignore 로 뺐다 — keep 도 그 로컬 파일을 묻지 않고 덮으므로, 지문 없이 부르면 아무것도 하지 않고
/// 목록과 지문을 conflict 로 돌려준다. 그 미리보기(`mode: keep`)를 확인한 지문으로 부르면 한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undo_that_overwrites_an_ignored_file_asks_first() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("secret.txt", "v1\n");
    let previous = r.commit_all("track the secret");
    r.git(&["rm", "-q", "--cached", "secret.txt"]);
    r.write(".gitignore", "secret.txt\n");
    let current = r.commit_all("ignore the secret");
    r.write("secret.txt", "MY SECRET\n");

    let e = reset(f.write.clone(), f.caller(), undo(&f, &previous, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    let data = e.data.unwrap();
    assert_eq!(
        data["lost"]["overwritten"],
        serde_json::json!(["secret.txt"])
    );
    assert_eq!(r.head(), current);
    assert_eq!(read(&f, "secret.txt").as_deref(), Some("MY SECRET\n"));

    let mut p = preview_params(&f, &previous);
    p.mode = ResetMode::Keep;
    let preview = reset_preview(f.write.clone(), p).await.unwrap();
    assert_eq!(preview.lost.overwritten, ["secret.txt"]);
    assert_eq!(
        preview.fingerprint.as_deref(),
        data["fingerprint"].as_str(),
        "the conflict carries the preview's fingerprint"
    );
    let done = reset(
        f.write.clone(),
        f.caller(),
        undo(&f, &previous, preview.fingerprint),
    )
    .await
    .unwrap();
    assert_eq!(r.head(), previous);
    assert_eq!(done.lost.unwrap().overwritten, ["secret.txt"]);
    assert_eq!(read(&f, "secret.txt").as_deref(), Some("v1\n"), "confirmed");
}

/// keep 은 index 를 되돌린다 — 디스크와 다른 스테이징 내용은 사라지므로 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_undo_that_drops_staged_content_asks_first() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    let previous = r.head();
    r.commit_file("a.txt", "a\n", "a");
    r.write("README.md", "STAGED\n");
    r.git(&["add", "README.md"]);
    r.write("README.md", "DISK\n");

    let e = reset(f.write.clone(), f.caller(), undo(&f, &previous, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    assert_eq!(e.data.unwrap()["lost"]["changes"][0]["path"], "README.md");
    assert_eq!(r.git(&["show", ":README.md"]), "STAGED\n", "untouched");
}

/// 폴더째 잃는 항목(`a/`)을 확인한 뒤 그 안의 파일을 고치거나 새 파일을 만들면 지문이 달라져 아무것도 하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_inside_a_lost_folder_after_the_confirmation_is_not_reset_away() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("a", "a file\n");
    let target = r.commit_all("a is a file");
    r.git(&["rm", "-q", "a"]);
    r.commit_all("drop a");
    r.write("a/sub/b.txt", "notes\n");

    let preview = reset_preview(f.write.clone(), preview_params(&f, &target))
        .await
        .unwrap();
    assert_eq!(preview.lost.overwritten, ["a/"]);
    r.write("a/sub/b.txt", "notes, edited after the confirmation\n");
    let e = reset(
        f.write.clone(),
        f.caller(),
        hard(&f, &target, preview.fingerprint.as_deref().unwrap()),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);

    let preview = reset_preview(f.write.clone(), preview_params(&f, &target))
        .await
        .unwrap();
    r.write("a/sub/new.txt", "new work\n");
    let e = reset(
        f.write.clone(),
        f.caller(),
        hard(&f, &target, preview.fingerprint.as_deref().unwrap()),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    assert_eq!(
        read(&f, "a/sub/b.txt").as_deref(),
        Some("notes, edited after the confirmation\n")
    );
    assert_eq!(read(&f, "a/sub/new.txt").as_deref(), Some("new work\n"));
}

/// 폴더째 잃는 항목 아래가 상한을 넘으면 지문이 없다(덜 보고 셈하지 않는다) — 그 reset hard 는 어떤 지문으로도 하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_folder_too_large_to_fingerprint_cannot_be_confirmed() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("a", "a file\n");
    let target = r.commit_all("a is a file");
    r.git(&["rm", "-q", "a"]);
    r.commit_all("drop a");
    let dir = r.root().join("a");
    std::fs::create_dir_all(&dir).unwrap();
    for n in 0..=super::super::lost::FINGERPRINT_ENTRIES {
        std::fs::write(dir.join(n.to_string()), "").unwrap();
    }

    let preview = reset_preview(f.write.clone(), preview_params(&f, &target))
        .await
        .unwrap();
    assert_eq!(preview.lost.overwritten, ["a/"]);
    assert_eq!(preview.fingerprint, None, "too many to fingerprint");
    let e = reset(f.write.clone(), f.caller(), hard(&f, &target, "anything"))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    assert_eq!(e.data.unwrap()["fingerprint"], serde_json::Value::Null);
    assert!(dir.join("0").exists());
}

fn mixed(f: &WriteFixture, commit: &str, fingerprint: Option<String>) -> GitResetParams {
    GitResetParams {
        project_id: f.project.clone(),
        worktree: None,
        commit: commit.to_owned(),
        mode: ResetMode::Mixed,
        expected: None,
        expected_ref: None,
        fingerprint,
    }
}

/// mixed 는 작업 트리를 두고 index 만 되돌린다 — 작업 트리와도 대상과도 다른 스테이징 내용은 사라지므로, 지문 없이 부르면 아무것도
/// 하지 않고 목록과 지문을 conflict 로 돌려준다. 확인한 뒤 다시 스테이징했으면 지문이 달라 거절하고, 그 미리보기의 지문으로 하면 한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mixed_reset_that_drops_staged_content_asks_first() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    let first = r.head();
    r.commit_file("a.txt", "a\n", "a");
    r.write("README.md", "STAGED\n");
    r.git(&["add", "README.md"]);
    r.write("README.md", "DISK\n");

    let e = reset(f.write.clone(), f.caller(), mixed(&f, &first, None))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    let data = e.data.unwrap();
    assert_eq!(data["lost"]["changes"][0]["path"], "README.md");
    assert_eq!(data["lost"]["changes"][0]["staged"], "modified");
    assert_eq!(data["lost"]["overwritten"], serde_json::json!([]));
    assert_ne!(r.head(), first);
    assert_eq!(r.git(&["show", ":README.md"]), "STAGED\n", "untouched");

    let mut p = preview_params(&f, &first);
    p.mode = ResetMode::Mixed;
    let preview = reset_preview(f.write.clone(), p.clone()).await.unwrap();
    assert_eq!(preview.lost.changes.len(), 1);
    assert_eq!(preview.lost.changes[0].path, "README.md");
    assert_eq!(preview.fingerprint.as_deref(), data["fingerprint"].as_str());

    r.write("README.md", "STAGED AGAIN\n");
    r.git(&["add", "README.md"]);
    r.write("README.md", "DISK\n");
    let e = reset(
        f.write.clone(),
        f.caller(),
        mixed(&f, &first, preview.fingerprint),
    )
    .await
    .unwrap_err();
    assert_eq!(
        e.code,
        rpc_code::CONFLICT,
        "restaged after the confirmation"
    );
    assert_eq!(r.git(&["show", ":README.md"]), "STAGED AGAIN\n");

    let preview = reset_preview(f.write.clone(), p).await.unwrap();
    let done = reset(
        f.write.clone(),
        f.caller(),
        mixed(&f, &first, preview.fingerprint),
    )
    .await
    .unwrap();
    assert_eq!(r.head(), first);
    assert_eq!(done.lost.unwrap().changes[0].path, "README.md");
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), "");
    assert_eq!(
        read(&f, "README.md").as_deref(),
        Some("DISK\n"),
        "the disk stays"
    );
}

/// soft 는 아무것도 잃지 않는다 — 미리보기가 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_soft_reset_has_no_preview() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let mut p = preview_params(&f, &f.repo.head());
    p.mode = ResetMode::Soft;
    let e = reset_preview(f.write.clone(), p).await.unwrap_err();
    assert_eq!(e.code, rpc_code::INVALID_PARAMS);
}
