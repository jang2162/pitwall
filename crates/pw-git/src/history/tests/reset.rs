//! reset: soft · mixed · hard 가 무엇을 바꾸는가, hard 결과의 「이전 해시」 로 되돌리면(keep) HEAD 가 돌아온다, 잃을 변경 목록이
//! git 이 실제로 잃는 것과 같다, `expected` 가 그사이 움직인 HEAD 를 덮지 않는다, hard 는 지문이 있어야 한다, 브랜치와 같은 이름의
//! 파일.

use std::fs;

use pw_rpc::code as rpc_code;

use super::super::methods::{
    GitResetParams, GitResetPreview, GitResetPreviewParams, reset, reset_preview,
};
use super::super::{GitRefMove, ResetMode};
use super::window;
use crate::status::GitChange;
use crate::testing::write::WriteFixture;
use crate::write::GitPromptRequest;

fn params(
    f: &WriteFixture,
    commit: &str,
    mode: ResetMode,
    expected: Option<&str>,
) -> GitResetParams {
    GitResetParams {
        project_id: f.project.clone(),
        worktree: None,
        commit: commit.to_owned(),
        mode,
        expected: expected.map(str::to_owned),
        expected_ref: None,
        fingerprint: None,
    }
}

/// 확인 다이얼로그가 하는 것: 미리보기를 읽고, 그 HEAD 와 지문으로 hard reset 한다.
pub(super) async fn confirmed_hard(
    f: &WriteFixture,
    commit: &str,
) -> (
    GitResetPreview,
    Result<super::super::GitResetResult, pw_rpc::RpcError>,
) {
    let preview = reset_preview(f.write.clone(), preview_params(f, commit))
        .await
        .unwrap();
    let mut p = params(f, commit, ResetMode::Hard, preview.head.as_deref());
    p.fingerprint.clone_from(&preview.fingerprint);
    let done = reset(f.write.clone(), f.caller(), p).await;
    (preview, done)
}

pub(super) fn preview_params(f: &WriteFixture, commit: &str) -> GitResetPreviewParams {
    GitResetPreviewParams {
        project_id: f.project.clone(),
        worktree: None,
        commit: commit.to_owned(),
        mode: ResetMode::Hard,
    }
}

pub(super) fn read(f: &WriteFixture, rel: &str) -> Option<String> {
    fs::read_to_string(f.repo.root().join(rel)).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_moves_only_the_ref_and_asks_nothing() {
    let f = WriteFixture::new(true);
    let window = window(&f);
    let first = f.repo.head();
    let second = f.repo.commit_file("a.txt", "a\n", "a");
    let done = reset(
        f.write.clone(),
        f.caller(),
        params(&f, &first, ResetMode::Soft, None),
    )
    .await
    .unwrap();
    assert_eq!(f.repo.head(), first);
    assert_eq!(f.repo.git(&["diff", "--cached", "--name-only"]), "a.txt\n");
    assert_eq!(read(&f, "a.txt").as_deref(), Some("a\n"));
    assert_eq!(
        done.moved,
        Some(GitRefMove {
            ref_name: "refs/heads/main".into(),
            previous: Some(second),
            current: Some(first),
        })
    );
    assert_eq!(done.lost, None);
    assert!(
        window.seen().is_empty(),
        "soft does not change the work tree"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_resets_the_index_and_keeps_the_work_tree() {
    let f = WriteFixture::new(true);
    let window = window(&f);
    let first = f.repo.head();
    f.repo.commit_file("a.txt", "a\n", "a");
    reset(
        f.write.clone(),
        f.caller(),
        params(&f, &first, ResetMode::Mixed, None),
    )
    .await
    .unwrap();
    assert_eq!(f.repo.head(), first);
    assert_eq!(f.repo.git(&["diff", "--cached", "--name-only"]), "");
    assert_eq!(f.repo.git(&["status", "--porcelain"]), "?? a.txt\n");
    assert!(
        window.seen().is_empty(),
        "mixed does not change the work tree"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undoing_a_hard_reset_with_its_previous_hash_brings_head_back() {
    let f = WriteFixture::new(true);
    let window = window(&f);
    let first = f.repo.head();
    let second = f.repo.commit_file("a.txt", "a\n", "a");
    f.repo.write("README.md", "edited\n");

    let (_, done) = confirmed_hard(&f, &first).await;
    let done = done.unwrap();
    assert_eq!(f.repo.head(), first);
    assert_eq!(read(&f, "a.txt"), None);
    assert_eq!(read(&f, "README.md").as_deref(), Some("hello\n"));
    let lost = done.lost.unwrap();
    assert_eq!(
        lost.changes
            .iter()
            .map(|e| e.path.as_str())
            .collect::<Vec<_>>(),
        ["README.md"]
    );
    assert!(matches!(
        window.seen().as_slice(),
        [GitPromptRequest::Unsaved { paths: None, .. }]
    ));
    let moved = done.moved.unwrap();
    assert_eq!(moved.previous.as_deref(), Some(second.as_str()));

    // [되돌리기]: 이전 해시로 keep, HEAD 가 아직 이 reset 이 둔 곳 · 그 ref 일 때만.
    let mut undo = params(
        &f,
        moved.previous.as_deref().unwrap(),
        ResetMode::Keep,
        moved.current.as_deref(),
    );
    undo.expected_ref = Some(moved.ref_name.clone());
    let undone = reset(f.write.clone(), f.caller(), undo).await.unwrap();
    assert_eq!(f.repo.head(), second);
    assert_eq!(read(&f, "a.txt").as_deref(), Some("a\n"));
    assert_eq!(undone.moved.unwrap().previous, Some(first));
    assert!(undone.lost.unwrap().is_empty(), "keep loses nothing");
}

/// 지문 없는 hard 는 확인을 건너뛴 것이다 — 아무것도 하지 않고 invalid params.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hard_reset_without_the_fingerprint_is_refused() {
    let f = WriteFixture::new(true);
    let window = window(&f);
    let first = f.repo.head();
    let second = f.repo.commit_file("a.txt", "a\n", "a");
    f.repo.write("README.md", "edited\n");
    let e = reset(
        f.write.clone(),
        f.caller(),
        params(&f, &first, ResetMode::Hard, Some(&second)),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, rpc_code::INVALID_PARAMS);
    assert_eq!(f.repo.head(), second);
    assert_eq!(read(&f, "README.md").as_deref(), Some("edited\n"));
    assert!(window.seen().is_empty(), "refused before asking anything");
}

/// 사전 조회의 목록이 git 이 실제로 하는 것과 같다: 추적 파일의 변경(스테이징 · 작업 트리 · `git rm --cached`)은 사라지고, 대상
/// 커밋에 있는 untracked 파일은 덮어써지고, 나머지 untracked 는 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_preview_lists_what_a_hard_reset_actually_loses() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let r = &f.repo;
    r.write("keep.txt", "keep\n");
    r.write("old.txt", "old\n");
    r.write("cached.txt", "cached\n");
    let c1 = r.commit_all("c1");
    r.git(&["rm", "-q", "old.txt"]);
    r.write("new.txt", "new\n");
    r.commit_all("c2");
    r.write("keep.txt", "staged\n");
    r.git(&["add", "keep.txt"]);
    r.write("new.txt", "edited\n");
    r.git(&["rm", "-q", "--cached", "cached.txt"]);
    r.write("old.txt", "untracked, in c1\n");
    r.write("scratch.txt", "untracked, not in c1\n");

    let preview = reset_preview(f.write.clone(), preview_params(&f, "HEAD~1"))
        .await
        .unwrap();
    assert_eq!(preview.target, c1);
    assert_eq!(preview.head, Some(r.head()));
    assert_eq!(preview.work_tree, r.root().display().to_string());
    let changes: Vec<_> = preview
        .lost
        .changes
        .iter()
        .map(|e| (e.path.as_str(), e.staged, e.unstaged))
        .collect();
    assert_eq!(
        changes,
        [
            (
                "cached.txt",
                Some(GitChange::Deleted),
                Some(GitChange::Untracked)
            ),
            ("keep.txt", Some(GitChange::Modified), None),
            ("new.txt", None, Some(GitChange::Modified)),
        ]
    );
    assert_eq!(preview.lost.overwritten, ["old.txt"]);

    let mut confirmed = params(&f, &c1, ResetMode::Hard, preview.head.as_deref());
    confirmed.fingerprint.clone_from(&preview.fingerprint);
    let done = reset(f.write.clone(), f.caller(), confirmed).await.unwrap();
    assert_eq!(done.lost, Some(preview.lost));
    assert_eq!(read(&f, "old.txt").as_deref(), Some("old\n"), "overwritten");
    assert_eq!(read(&f, "keep.txt").as_deref(), Some("keep\n"));
    assert_eq!(read(&f, "cached.txt").as_deref(), Some("cached\n"));
    assert_eq!(read(&f, "new.txt"), None);
    assert_eq!(
        read(&f, "scratch.txt").as_deref(),
        Some("untracked, not in c1\n"),
        "other untracked files stay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_head_that_moved_since_the_confirmation_is_not_reset() {
    let f = WriteFixture::new(true);
    let window = window(&f);
    let first = f.repo.head();
    let seen = f.repo.commit_file("a.txt", "a\n", "a");
    let preview = reset_preview(f.write.clone(), preview_params(&f, &first))
        .await
        .unwrap();
    let moved_on = f.repo.commit_file("b.txt", "b\n", "b");
    let mut confirmed = params(&f, &first, ResetMode::Hard, Some(&seen));
    confirmed.fingerprint = preview.fingerprint;
    let e = reset(f.write.clone(), f.caller(), confirmed)
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::CONFLICT);
    assert_eq!(e.data.unwrap()["head"], moved_on.as_str());
    assert_eq!(f.repo.head(), moved_on);
    assert_eq!(read(&f, "b.txt").as_deref(), Some("b\n"));
    assert_eq!(
        window.seen().len(),
        1,
        "checked under the lock, after the confirmation"
    );
}

/// 리비전 뒤의 `--` 가 없으면 git 은 같은 이름의 파일이 있을 때 모호하다며 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_branch_named_like_a_file_is_still_a_revision() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let first = f.repo.head();
    f.repo.branch("other");
    f.repo
        .commit_file("other", "a file named like the branch\n", "file");
    reset(
        f.write.clone(),
        f.caller(),
        params(&f, "other", ResetMode::Soft, None),
    )
    .await
    .unwrap();
    assert_eq!(f.repo.head(), first);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_revision_fails_with_gits_message_and_the_preview_is_invalid_params() {
    let f = WriteFixture::new(true);
    let _window = window(&f);
    let head = f.repo.head();
    let e = reset(
        f.write.clone(),
        f.caller(),
        params(&f, "no-such-branch", ResetMode::Soft, None),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, crate::write::code::GIT_FAILED);
    assert_eq!(f.repo.head(), head);
    // mixed 는 git 을 띄우기 전에 잃을 변경을 읽는다.
    let e = reset(
        f.write.clone(),
        f.caller(),
        params(&f, "no-such-branch", ResetMode::Mixed, None),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, rpc_code::INVALID_PARAMS);
    assert_eq!(f.repo.head(), head);
    let e = reset_preview(f.write.clone(), preview_params(&f, "no-such-branch"))
        .await
        .unwrap_err();
    assert_eq!(e.code, rpc_code::INVALID_PARAMS);
}
