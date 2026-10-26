//! 작업 트리 diff 는 저장소 밖을 읽지 않는다(M1-01 의 규칙 A). 압축으로 받은 저장소는 index 와 작업 트리를 둘 다 만들 수 있다 —
//! index 에 `dir/id_rsa` 를 두고 작업 트리의 `dir` 를 `~/.ssh` 를 가리키는 링크로 두면, 디스크를 따라 읽는 diff 는 그 내용을
//! hunk 로 보인다.

use std::os::unix::fs::symlink;

use super::*;

const SECRET: &str = "SECRET OUTSIDE\n";

/// 저장소 밖 폴더(`file.txt` · `new.txt` 에 [`SECRET`])와 그 폴더를 가리키게 바꿀 `dir`.
fn outside(r: &TestRepo) -> std::path::PathBuf {
    let outside = r.base().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("file.txt"), SECRET).unwrap();
    fs::write(outside.join("new.txt"), SECRET).unwrap();
    outside
}

fn leaked(diff: &GitDiff) -> bool {
    diff.files
        .iter()
        .flat_map(|f| f.hunks.iter().flat_map(|h| &h.lines))
        .any(|l| l.text.contains("SECRET"))
}

/// 지운 파일의 새 쪽(없는 쪽)은 디스크를 읽지 않는다 — 그 자리가 저장소 밖 폴더를 가리키는 링크여도. git 처럼 index 쪽 줄만 지운
/// 줄로 보인다.
#[test]
fn a_deleted_file_behind_a_folder_link_is_not_read_from_outside() {
    let r = TestRepo::with_commit();
    r.write("dir/file.txt", "inside\n");
    r.commit_all("dir");
    let outside = outside(&r);
    fs::remove_dir_all(r.root().join("dir")).unwrap();
    symlink(&outside, r.root().join("dir")).unwrap();
    let repo = Repo::new(r);

    let got = repo.diff(None, DiffSide::WorkTree);
    assert!(!leaked(&got), "{got:#?}");
    assert_same_as_git(&repo, None, DiffSide::WorkTree, &[]);
}

/// `git add -N` 한 파일(index 에는 빈 자리만 있다)의 폴더를 저장소 밖 링크로 바꾸어도 그 파일을 읽지 않는다 — git 처럼 지운
/// 것으로 보인다.
#[test]
fn an_intent_to_add_file_behind_a_folder_link_is_not_read_from_outside() {
    let r = TestRepo::with_commit();
    r.write("dir/new.txt", "inside\n");
    r.git(&["add", "--intent-to-add", "dir/new.txt"]);
    r.write("README.md", "hello\nchanged\n");
    let outside = outside(&r);
    fs::remove_dir_all(r.root().join("dir")).unwrap();
    symlink(&outside, r.root().join("dir")).unwrap();
    let repo = Repo::new(r);

    let got = repo.diff(None, DiffSide::WorkTree);
    assert!(!leaked(&got), "{got:#?}");
    assert_same_as_git(&repo, None, DiffSide::WorkTree, &[]);
}

/// 작업 트리 쪽 읽기 판정(규칙 A): 든 폴더가 밖을 가리키는 링크면 거절, 링크 파일 자체는 대상이 밖이어도 받는다(대상 글자를 읽는다),
/// 루트 안 파일은 받는다.
#[test]
fn the_work_tree_check_follows_folders_but_not_the_link_itself() {
    use crate::diff::changes::WorkTree;
    use gix::index::entry::Mode;
    let r = TestRepo::with_commit();
    let outside = outside(&r);
    symlink(&outside, r.root().join("dir")).unwrap();
    symlink("/etc/hosts", r.root().join("link")).unwrap();
    let work = WorkTree::new(r.root());
    let check = |rel: &str, mode| work.check(rel.into(), mode);
    assert!(check("dir/file.txt", Mode::FILE).is_err());
    assert!(check("dir/file.txt", Mode::SYMLINK).is_err());
    assert!(check("dir", Mode::DIR).is_err());
    assert!(check("link", Mode::SYMLINK).is_ok());
    assert!(check("link", Mode::FILE).is_err());
    assert!(check("README.md", Mode::FILE).is_ok());
}

/// 작업 트리의 서브모듈 자리는 git 과 같다: 체크아웃되지 않은 빈 폴더는 바뀌지 않았고, 지우면 `D`, 저장소 밖 저장소를 가리키는
/// 링크로 바꾸면 `T`(160000 → 120000)이고 그 저장소를 열지 않는다(그 HEAD 가 응답에 없다).
#[test]
fn submodule_places_in_the_work_tree_match_git() {
    let r = TestRepo::with_commit();
    let sub = r.root().join("sub");
    fs::create_dir_all(&sub).unwrap();
    r.git_in(&sub, &["init", "-q"]);
    fs::write(sub.join("s.txt"), "one\n").unwrap();
    r.commit_all_in(&sub, "s1");
    r.commit_all("add sub");
    let pointer = r.rev_parse(":sub");
    let repo = Repo::new(r);

    // 체크아웃되지 않은 서브모듈(`git clone` 뒤 `submodule update` 전).
    let moved = repo.r.base().join("outside-sub");
    fs::rename(&sub, &moved).unwrap();
    fs::create_dir(&sub).unwrap();
    assert_eq!(repo.r.git(&["diff", "--raw"]), "");
    assert_eq!(repo.diff(None, DiffSide::WorkTree).files, []);

    fs::remove_dir(&sub).unwrap();
    assert_same_as_git(&repo, None, DiffSide::WorkTree, &[]);

    // git 은 이 자리의 patch 를 만들지 못한다(「symbolic link 이면 안 된다」) — 목록(`--raw`)만 견준다.
    fs::write(moved.join("s.txt"), "two\n").unwrap();
    let elsewhere = repo.r.commit_all_in(&moved, "s2");
    symlink(&moved, &sub).unwrap();
    let zeros = "0".repeat(pointer.len());
    assert_eq!(
        repo.r.git(&["diff", "--raw", "--no-abbrev"]),
        format!(":160000 120000 {pointer} {zeros} T\tsub\n")
    );
    let got = repo.diff(None, DiffSide::WorkTree);
    let f = &got.files[..];
    assert!(
        f.len() == 1
            && f[0].status == FileStatus::TypeChanged
            && (f[0].old_mode.as_deref(), f[0].new_mode.as_deref())
                == (Some("160000"), Some("120000"))
            && (f[0].old_id.as_deref(), f[0].new_id.as_deref()) == (Some(pointer.as_str()), None),
        "{got:#?}"
    );
    let json = serde_json::to_string(&got).unwrap();
    assert!(!json.contains(&elsewhere), "{json}");
}

/// 루트 안을 가리키는 심링크 파일은 링크 자체(대상 글자)를 견준다 — 판정이 링크를 따라가 거절하지 않는다.
#[test]
fn a_symlink_file_is_diffed_as_its_target_text() {
    let r = TestRepo::with_commit();
    symlink("README.md", r.root().join("link")).unwrap();
    r.commit_all("link");
    fs::remove_file(r.root().join("link")).unwrap();
    symlink("/etc/hosts", r.root().join("link")).unwrap();
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, DiffSide::WorkTree, &[]);
    let got = repo.diff(None, DiffSide::WorkTree);
    assert!(got.files.iter().all(|f| f.error.is_none()), "{got:#?}");
}
