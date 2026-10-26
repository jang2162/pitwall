//! stash(완료 기준 ④): 만들기 → pop 왕복 뒤 작업 트리 · index 가 같다. 목록 · 항목 대조 · 버리기의 확인 · 충돌로 끝나는 적용.

use pw_rpc::code as rpc_code;

use super::{index, proceed};
use crate::stage::methods::{
    GitStashApplyParams, GitStashDropParams, GitStashListParams, GitStashPushParams, StashEntry,
};
use crate::testing::write::WriteFixture;
use crate::write::{GitPromptRequest, WriteError};

fn push(f: &WriteFixture, message: Option<&str>, include_untracked: bool) -> GitStashPushParams {
    GitStashPushParams {
        project_id: f.project.clone(),
        worktree: None,
        message: message.map(str::to_owned),
        include_untracked,
    }
}

fn pick(f: &WriteFixture, entry: &StashEntry, reinstate_index: bool) -> GitStashApplyParams {
    GitStashApplyParams {
        project_id: f.project.clone(),
        worktree: None,
        index: entry.index,
        commit: entry.commit.clone(),
        reinstate_index,
    }
}

async fn list(f: &WriteFixture) -> Vec<StashEntry> {
    f.write
        .stash_list(GitStashListParams {
            project_id: f.project.clone(),
            worktree: None,
        })
        .await
        .unwrap()
        .stashes
}

/// 작업 트리 · index 의 모습: 추적 파일과 untracked 파일의 바이트, index 항목.
fn snapshot(f: &WriteFixture) -> (String, Vec<(String, Vec<u8>)>) {
    let mut files = Vec::new();
    for name in ["README.md", "staged.txt", "both.txt", "loose.txt"] {
        let path = f.repo.root().join(name);
        files.push((name.to_owned(), std::fs::read(&path).unwrap_or_default()));
    }
    (index(f), files)
}

fn code(e: WriteError) -> i32 {
    match e {
        WriteError::Rpc(e) => e.code,
        other => panic!("not an rpc error: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stash_push_and_pop_round_trip_keeps_the_work_tree_and_the_index() {
    let f = WriteFixture::new(true);
    f.repo.commit_file("both.txt", "base\r\n", "both");
    f.repo.write("staged.txt", "staged only\n");
    f.repo.git(&["add", "staged.txt"]);
    f.repo.write("both.txt", "in index\r\n");
    f.repo.git(&["add", "both.txt"]);
    f.repo.write("both.txt", "in work tree\r\nno eol");
    f.repo.write("README.md", "unstaged\n");
    f.repo.write("loose.txt", "untracked\n");
    let before = snapshot(&f);
    assert!(list(&f).await.is_empty());

    let window = proceed(&f);
    let made = f
        .write
        .stash_push(push(&f, Some("wip: round trip"), true))
        .await
        .unwrap()
        .stash
        .expect("a new stash");
    assert_eq!(made.index, 0);
    assert_eq!(made.message, "On main: wip: round trip");
    assert_eq!(made.commit, f.repo.rev_parse("stash@{0}"));
    assert_eq!(list(&f).await, std::slice::from_ref(&made));
    assert_eq!(
        f.repo.git(&["status", "--porcelain"]),
        "",
        "the work tree is clean"
    );
    // 만들기는 작업 트리 전체를 바꾸므로 경로 없이 물었다.
    assert!(matches!(
        &window.seen()[0],
        GitPromptRequest::Unsaved { paths: None, .. }
    ));

    let popped = f.write.stash_pop(pick(&f, &made, true)).await.unwrap();
    assert!(!popped.conflicted);
    assert_eq!(snapshot(&f), before);
    assert!(list(&f).await.is_empty());
    // 적용은 stash 가 바꾸는 경로만 물었다(untracked 포함).
    assert_eq!(
        window.seen()[1],
        GitPromptRequest::Unsaved {
            work_tree: f.repo.root().display().to_string(),
            paths: Some(vec![
                "README.md".into(),
                "both.txt".into(),
                "loose.txt".into(),
                "staged.txt".into(),
            ]),
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_to_stash_is_not_an_error_and_untracked_files_stay_unless_asked() {
    let f = WriteFixture::new(true);
    let _window = proceed(&f);
    let none = f.write.stash_push(push(&f, None, false)).await.unwrap();
    assert_eq!(none.stash, None);

    f.repo.write("README.md", "changed\n");
    f.repo.write("loose.txt", "untracked\n");
    let made = f
        .write
        .stash_push(push(&f, Some(""), false))
        .await
        .unwrap()
        .stash
        .unwrap();
    assert!(
        made.message.starts_with("WIP on main: "),
        "{}",
        made.message
    );
    assert!(
        f.repo.root().join("loose.txt").exists(),
        "untracked stays without includeUntracked"
    );
    // 항목이 있어도 담을 변경이 없으면 새 항목이 없다(맨 위 항목을 새 것으로 돌려주지 않는다).
    let none = f.write.stash_push(push(&f, None, false)).await.unwrap();
    assert_eq!(none.stash, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_keeps_the_entry_and_a_moved_list_or_missing_confirmation_does_nothing() {
    let f = WriteFixture::new(true);
    let _window = proceed(&f);
    f.repo.write("README.md", "first\n");
    let first = f
        .write
        .stash_push(push(&f, Some("first"), false))
        .await
        .unwrap()
        .stash
        .unwrap();
    f.repo.write("README.md", "second\n");
    let second = f
        .write
        .stash_push(push(&f, Some("second"), false))
        .await
        .unwrap()
        .stash
        .unwrap();
    let entries = list(&f).await;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].commit, second.commit);
    assert_eq!(entries[1].commit, first.commit);
    assert_eq!(entries[1].index, 1);

    // `first` 를 index 0 으로 고르면 지금 목록과 맞지 않는다.
    let stale = f
        .write
        .stash_apply(GitStashApplyParams {
            index: 0,
            ..pick(&f, &entries[1], false)
        })
        .await
        .unwrap_err();
    assert_eq!(code(stale), rpc_code::CONFLICT);
    assert_eq!(f.repo.git(&["status", "--porcelain"]), "");

    let applied = f
        .write
        .stash_apply(pick(&f, &entries[1], false))
        .await
        .unwrap();
    assert!(!applied.conflicted);
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("README.md")).unwrap(),
        "first\n"
    );
    assert_eq!(list(&f).await.len(), 2, "apply keeps the entry");

    let drop = |confirmed, entry: &StashEntry| GitStashDropParams {
        project_id: f.project.clone(),
        worktree: None,
        index: entry.index,
        commit: entry.commit.clone(),
        confirmed,
    };
    let refused = f
        .write
        .stash_drop(drop(false, &entries[1]))
        .await
        .unwrap_err();
    assert_eq!(code(refused), rpc_code::INVALID_PARAMS);
    let stale = f
        .write
        .stash_drop(GitStashDropParams {
            index: 0,
            ..drop(true, &entries[1])
        })
        .await
        .unwrap_err();
    assert_eq!(code(stale), rpc_code::CONFLICT);
    assert_eq!(list(&f).await.len(), 2);

    f.write.stash_drop(drop(true, &entries[1])).await.unwrap();
    let left = list(&f).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].commit, second.commit);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicting_pop_reports_conflicts_and_keeps_the_entry() {
    let f = WriteFixture::new(true);
    let _window = proceed(&f);
    f.repo.write("README.md", "stashed\n");
    let made = f
        .write
        .stash_push(push(&f, None, false))
        .await
        .unwrap()
        .stash
        .unwrap();
    f.repo
        .commit_file("README.md", "committed\n", "conflicting");

    let popped = f.write.stash_pop(pick(&f, &made, false)).await.unwrap();
    assert!(popped.conflicted);
    assert_eq!(
        list(&f).await,
        [made],
        "git keeps the entry after a conflict"
    );
    assert!(
        f.repo.git(&["ls-files", "-u"]).contains("README.md"),
        "README.md is in conflict"
    );
}
