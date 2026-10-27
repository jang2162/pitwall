//! 작업 트리 diff 의 서브모듈 안 변경(`-dirty`, 13-m2-plan 넘김 표 — M2-16): 체크아웃된 서브모듈 안에 커밋하지 않은 변경이 있으면
//! git 처럼 `Subproject commit <해시>-dirty` 다(`SubmodulePointer::dirty`). 포인터가 그대로여도 목록에 든다. 서브모듈은 바깥
//! 프로젝트의 신뢰로 연다 — 미신뢰면 서브모듈 저장소의 설정이 고른 필터도 돌지 않는다.

use std::os::unix::fs::PermissionsExt;

use super::*;

/// 커밋 하나를 담은 서브모듈 `sub` 를 바깥 저장소에 담는다. 서브모듈 폴더를 돌려준다.
fn with_submodule(r: &TestRepo) -> std::path::PathBuf {
    let sub = r.root().join("sub");
    fs::create_dir_all(&sub).unwrap();
    r.git_in(&sub, &["init", "-q"]);
    fs::write(sub.join("s.txt"), "one\n").unwrap();
    r.commit_all_in(&sub, "s1");
    r.commit_all("add sub");
    sub
}

fn dirty_of(diff: &GitDiff) -> Option<bool> {
    diff.files
        .iter()
        .find(|f| f.path == "sub")
        .and_then(|f| f.submodule.as_ref())
        .map(|s| s.dirty)
}

#[test]
fn a_dirty_submodule_matches_git() {
    let r = TestRepo::with_commit();
    let sub = with_submodule(&r);
    let repo = Repo::new(r);
    // 깨끗하면 목록에 없다.
    assert_eq!(repo.diff(None, DiffSide::WorkTree).files, []);
    assert_eq!(repo.r.git(&["diff"]), "");

    // 고친 파일(포인터는 그대로).
    fs::write(sub.join("s.txt"), "changed\n").unwrap();
    assert_same_as_git(&repo, None, DiffSide::WorkTree, &[]);
    assert_eq!(dirty_of(&repo.diff(None, DiffSide::WorkTree)), Some(true));
    repo.r.git_in(&sub, &["checkout", "-q", "--", "s.txt"]);

    // untracked 파일만 — `git diff` 는 달지 않는다(`git status` 는 ` M` 으로 보인다).
    fs::write(sub.join("new.txt"), "new\n").unwrap();
    assert_eq!(repo.r.git(&["diff"]), "");
    assert_eq!(repo.diff(None, DiffSide::WorkTree).files, []);

    // 스테이징만.
    repo.r.git_in(&sub, &["add", "new.txt"]);
    assert_same_as_git(&repo, None, DiffSide::WorkTree, &[]);
    assert_eq!(dirty_of(&repo.diff(None, DiffSide::WorkTree)), Some(true));

    // 커밋해 포인터가 옮겨졌고 안에 또 고친 것이 있다. 옮겨진 포인터의 새 쪽 해시는 git 의 `--raw` 가 0 으로 적는다(작업 트리
    // 쪽) — hunk 만 견준다(`compare::a_submodule_pointer_matches_git` 와 같다).
    repo.r.commit_all_in(&sub, "s2");
    let same_hunks = |repo: &Repo| {
        let ours = our_patches(&repo.diff(None, DiffSide::WorkTree));
        let theirs = git_patches(&repo.r, &[]);
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].hunks, theirs[0].hunks);
    };
    assert_eq!(dirty_of(&repo.diff(None, DiffSide::WorkTree)), Some(false));
    same_hunks(&repo);
    fs::write(sub.join("s.txt"), "again\n").unwrap();
    same_hunks(&repo);
    assert_eq!(dirty_of(&repo.diff(None, DiffSide::WorkTree)), Some(true));

    // 무시된 파일은 세지 않는다(git 과 같다).
    repo.r.commit_all_in(&sub, "s3");
    repo.r.commit_all("bump");
    fs::write(sub.join(".gitignore"), "*.log\n").unwrap();
    repo.r.commit_all_in(&sub, "ignore");
    repo.r.commit_all("bump again");
    fs::write(sub.join("x.log"), "log\n").unwrap();
    assert_eq!(repo.r.git(&["diff"]), "");
    assert_eq!(repo.diff(None, DiffSide::WorkTree).files, []);

    // 커밋끼리 · index 쪽은 서브모듈 안을 보지 않는다.
    fs::write(sub.join("s.txt"), "dirty again\n").unwrap();
    assert!(
        repo.diff(None, DiffSide::Index)
            .files
            .iter()
            .chain(&repo.diff(None, commit("HEAD")).files)
            .all(|f| f.submodule.as_ref().is_none_or(|s| !s.dirty))
    );
}

/// 서브모듈 저장소의 설정이 고른 clean 필터는 미신뢰 프로젝트에서 돌지 않는다(서브모듈을 바깥 프로젝트의 신뢰로 연다). 신뢰하면
/// 서브모듈 안을 견주며 git 처럼 돈다 — 「돌지 않는다」 가 신뢰로 갈린다.
#[test]
fn an_untrusted_project_runs_no_filter_inside_its_submodule() {
    let r = TestRepo::with_commit();
    let sub = with_submodule(&r);
    let marks = r.base().join("marks");
    fs::create_dir_all(&marks).unwrap();
    let clean = r.base().join("clean.sh");
    fs::write(
        &clean,
        format!(
            "#!/bin/sh\necho clean >> '{}'\ncat\n",
            marks.join("clean").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&clean, fs::Permissions::from_mode(0o755)).unwrap();
    r.git_in(
        &sub,
        &["config", "filter.mark.clean", &clean.display().to_string()],
    );
    fs::write(sub.join(".git/info/attributes"), "* filter=mark\n").unwrap();
    // 크기는 같고 내용 · 시각이 다르다 — 서브모듈 안 견주기가 그 파일을 (필터를 거쳐) 읽어야 안다.
    fs::write(sub.join("s.txt"), "two\n").unwrap();
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(sub.join("s.txt"))
        .unwrap()
        .set_modified(later)
        .unwrap();
    for e in fs::read_dir(&marks).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    let repo = Repo::new(r);
    assert!(!repo.f.projects.is_trusted(&repo.id));

    let diff = repo.diff(None, DiffSide::WorkTree);
    assert_eq!(dirty_of(&diff), Some(true), "{diff:#?}");
    assert!(fs::read_dir(&marks).unwrap().next().is_none());

    repo.f.trust(&repo.id, true);
    let diff = repo.diff(None, DiffSide::WorkTree);
    assert_eq!(dirty_of(&diff), Some(true), "{diff:#?}");
    assert!(marks.join("clean").exists());
}
