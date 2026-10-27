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
        .stash_push(f.caller(), push(&f, Some("wip: round trip"), true))
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

    let popped = f
        .write
        .stash_pop(f.caller(), pick(&f, &made, true))
        .await
        .unwrap();
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
    let none = f
        .write
        .stash_push(f.caller(), push(&f, None, false))
        .await
        .unwrap();
    assert_eq!(none.stash, None);

    f.repo.write("README.md", "changed\n");
    f.repo.write("loose.txt", "untracked\n");
    let made = f
        .write
        .stash_push(f.caller(), push(&f, Some(""), false))
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
    let none = f
        .write
        .stash_push(f.caller(), push(&f, None, false))
        .await
        .unwrap();
    assert_eq!(none.stash, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_keeps_the_entry_and_a_moved_list_or_missing_confirmation_does_nothing() {
    let f = WriteFixture::new(true);
    let _window = proceed(&f);
    f.repo.write("README.md", "first\n");
    let first = f
        .write
        .stash_push(f.caller(), push(&f, Some("first"), false))
        .await
        .unwrap()
        .stash
        .unwrap();
    f.repo.write("README.md", "second\n");
    let second = f
        .write
        .stash_push(f.caller(), push(&f, Some("second"), false))
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
        .stash_apply(
            f.caller(),
            GitStashApplyParams {
                index: 0,
                ..pick(&f, &entries[1], false)
            },
        )
        .await
        .unwrap_err();
    assert_eq!(code(stale), rpc_code::CONFLICT);
    assert_eq!(f.repo.git(&["status", "--porcelain"]), "");

    let applied = f
        .write
        .stash_apply(f.caller(), pick(&f, &entries[1], false))
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
        .stash_drop(f.caller(), drop(false, &entries[1]))
        .await
        .unwrap_err();
    assert_eq!(code(refused), rpc_code::INVALID_PARAMS);
    let stale = f
        .write
        .stash_drop(
            f.caller(),
            GitStashDropParams {
                index: 0,
                ..drop(true, &entries[1])
            },
        )
        .await
        .unwrap_err();
    assert_eq!(code(stale), rpc_code::CONFLICT);
    assert_eq!(list(&f).await.len(), 2);

    f.write
        .stash_drop(f.caller(), drop(true, &entries[1]))
        .await
        .unwrap();
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
        .stash_push(f.caller(), push(&f, None, false))
        .await
        .unwrap()
        .stash
        .unwrap();
    f.repo
        .commit_file("README.md", "committed\n", "conflicting");

    let popped = f
        .write
        .stash_pop(f.caller(), pick(&f, &made, false))
        .await
        .unwrap();
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

/// `bin/git` 래퍼: `stash <verb>` 를 넘기기 직전에, `bin/intrude-<verb>` 가 있으면 그 안의 커밋을 `stash store -m intruder` 로
/// 끼운다(한 번). 확인(잠금 안의 목록 읽기)과 실행 사이에 다른 프로세스가 stash 를 만든 것과 같다.
fn intruding_git(f: &WriteFixture) {
    let real = crate::testing::write::which_git();
    let script = format!(
        "#!/bin/sh\nfor verb in apply drop; do\n  t='{bin}/intrude-'$verb\n  case \" $* \" in *\" stash $verb \"*)\n    if [ -f \"$t\" ]; then sha=$(cat \"$t\"); rm -f \"$t\"; '{real}' stash store -m intruder \"$sha\" || exit 99; fi;;\n  esac\ndone\nexec '{real}' \"$@\"\n",
        bin = f.bin.display(),
        real = real.display(),
    );
    let path = f.bin.join("git");
    std::fs::write(&path, script).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
}

/// 작업 트리 · refs 를 건드리지 않고 만든 stash 꼴 커밋(`git stash create`). 그 커밋 메시지는 `WIP on main: …` 이다 — 되돌려
/// 넣은 항목의 메시지가 된다.
fn intruder(f: &WriteFixture) -> String {
    f.repo.write("README.md", "intruder\n");
    let sha = f.repo.git(&["stash", "create"]).trim().to_owned();
    f.repo.git(&["checkout", "--", "README.md"]);
    sha
}

fn intrude(f: &WriteFixture, verb: &str, sha: &str) {
    std::fs::write(f.bin.join(format!("intrude-{verb}")), sha).unwrap();
}

/// 항목의 메시지. 래퍼가 끼운 항목은 `intruder`, 되돌려 넣은 그 항목은 커밋 메시지(`WIP on main: …`)라 `put back` 으로 적는다.
fn messages(entries: &[StashEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|e| {
            if e.message.starts_with("WIP on main: ") {
                "put back"
            } else {
                e.message.as_str()
            }
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stash_put_in_before_drop_is_put_back_and_nothing_of_ours_is_dropped() {
    let f = WriteFixture::new(true);
    intruding_git(&f);
    f.repo.write("README.md", "mine\n");
    f.repo.git(&["stash", "push", "--quiet", "-m", "mine"]);
    let sha = intruder(&f);
    let mine = list(&f).await.remove(0);
    let drop = |entry: &StashEntry| GitStashDropParams {
        project_id: f.project.clone(),
        worktree: None,
        index: entry.index,
        commit: entry.commit.clone(),
        confirmed: true,
    };

    intrude(&f, "drop", &sha);
    let raced = f
        .write
        .stash_drop(f.caller(), drop(&mine))
        .await
        .unwrap_err();
    assert_eq!(code(raced), rpc_code::CONFLICT);
    let left = list(&f).await;
    assert_eq!(messages(&left), ["put back", "On main: mine"], "{left:?}");
    assert_eq!(left[0].commit, sha, "the other stash is back");
    assert_eq!(left[1].commit, mine.commit);

    // 다시 읽은 목록으로 고르면 된다.
    f.write
        .stash_drop(f.caller(), drop(&left[1]))
        .await
        .unwrap();
    assert_eq!(messages(&list(&f).await), ["put back"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pop_applies_the_chosen_commit_and_drops_only_it_when_the_list_moves() {
    let f = WriteFixture::new(true);
    let _window = proceed(&f);
    intruding_git(&f);
    f.repo.write("README.md", "mine\n");
    f.repo.git(&["stash", "push", "--quiet", "-m", "mine"]);
    let sha = intruder(&f);
    let mine = list(&f).await.remove(0);

    // 확인과 적용 사이에 끼면: 적용은 커밋으로 하므로 고른 것을 적용하고, 버리기는 밀린 번호에서 그것만 지운다.
    intrude(&f, "apply", &sha);
    let popped = f
        .write
        .stash_pop(f.caller(), pick(&f, &mine, false))
        .await
        .unwrap();
    assert!(!popped.conflicted);
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("README.md")).unwrap(),
        "mine\n"
    );
    let left = list(&f).await;
    assert_eq!(messages(&left), ["intruder"]);
    assert_eq!(left[0].commit, sha);

    // 적용 뒤 버리기 직전에 끼면: 적용은 됐고, 남의 것을 지운 버리기는 되돌려 넣고 conflict — 우리 항목은 남는다.
    f.repo.git(&["checkout", "--", "README.md"]);
    f.repo.write("README.md", "mine again\n");
    f.repo
        .git(&["stash", "push", "--quiet", "-m", "mine again"]);
    let again = list(&f).await.remove(0);
    intrude(&f, "drop", &sha);
    let raced = f
        .write
        .stash_pop(f.caller(), pick(&f, &again, false))
        .await
        .unwrap_err();
    let WriteError::Rpc(e) = &raced else {
        panic!("{raced:?}")
    };
    assert_eq!(e.code, rpc_code::CONFLICT);
    assert!(e.message.contains("applied"), "{}", e.message);
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("README.md")).unwrap(),
        "mine again\n"
    );
    let left = list(&f).await;
    assert_eq!(
        messages(&left),
        ["put back", "On main: mine again", "intruder"],
        "{left:?}"
    );
    assert_eq!(left[1].commit, again.commit);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applying_during_a_merge_conflict_fails_with_git_s_words() {
    let f = WriteFixture::new(true);
    let _window = proceed(&f);
    f.repo.write("README.md", "stashed\n");
    f.repo.git(&["stash", "push", "--quiet"]);
    let made = list(&f).await.remove(0);
    f.repo.start_conflicting_merge();
    let unmerged = f.repo.git(&["ls-files", "-u"]);
    assert!(!unmerged.is_empty());

    for pop in [false, true] {
        let result = if pop {
            f.write.stash_pop(f.caller(), pick(&f, &made, false)).await
        } else {
            f.write
                .stash_apply(f.caller(), pick(&f, &made, false))
                .await
        };
        let Err(WriteError::Failed { stderr, .. }) = &result else {
            panic!("pop {pop}: {result:?}");
        };
        assert!(!stderr.trim().is_empty(), "git's own words");
        assert_eq!(list(&f).await, std::slice::from_ref(&made), "pop {pop}");
        assert_eq!(f.repo.git(&["ls-files", "-u"]), unmerged);
        assert_eq!(
            std::fs::read_to_string(f.repo.root().join("README.md")).unwrap(),
            "hello\n"
        );
    }
}
