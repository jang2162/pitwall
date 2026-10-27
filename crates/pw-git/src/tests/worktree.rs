//! 워크트리 목록(`git.worktrees`)과 체크아웃의 브랜치(`read_branch` · `git.projectBranches`, M2-13). 목록은 `git worktree list
//! --porcelain` 과 같은 작업 트리 · HEAD 를 보이고, 각 항목의 `switchTo` 는 Host 가 받아 주는 활성 워크트리다. 미신뢰 프로젝트에서
//! 저장소에 심은 프로그램을 돌리지 않는다.

use crate::methods::RepoHead;
use crate::testing::TestRepo;
use crate::worktree::{GitWorktree, GitWorktreeSwitch, read_branch};

use super::*;

fn list(f: &Fixture, id: &str, worktree: Option<&Path>) -> Vec<GitWorktree> {
    f.git
        .worktrees(&GitRepoParams {
            project_id: id.to_owned(),
            worktree: worktree.map(|w| w.display().to_string()),
        })
        .unwrap()
        .worktrees
}

fn normalized(p: &Path) -> String {
    pw_core::registry::path::normalize(p)
        .unwrap()
        .display()
        .to_string()
}

/// `git worktree list --porcelain` 의 (경로, HEAD 커밋, 브랜치 짧은 이름 · 분리면 없음, 잠김).
fn porcelain(r: &TestRepo) -> Vec<(String, String, Option<String>, bool)> {
    let out = r.git(&["worktree", "list", "--porcelain"]);
    out.split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .map(|block| {
            let mut path = String::new();
            let mut head = String::new();
            let mut branch = None;
            let mut locked = false;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = normalized(Path::new(p));
                } else if let Some(h) = line.strip_prefix("HEAD ") {
                    head = h.to_owned();
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(b.to_owned());
                } else if line == "locked" || line.starts_with("locked ") {
                    locked = true;
                }
            }
            (path, head, branch, locked)
        })
        .collect()
}

fn ours(worktrees: &[GitWorktree]) -> Vec<(String, String, Option<String>, bool)> {
    worktrees
        .iter()
        .map(|w| {
            let (commit, branch) = match w.head.clone().expect("a readable HEAD") {
                RepoHead::Branch { name, commit } => (commit, Some(name)),
                RepoHead::Detached { commit } => (commit, None),
                RepoHead::Unborn { name } => (String::new(), Some(name)),
            };
            (w.path.clone(), commit, branch, w.locked)
        })
        .collect()
}

/// 메인 체크아웃 · 연결 워크트리 둘(하나는 분리된 HEAD, 하나는 잠김)이 `git worktree list` 와 같은 차례 · 경로 · HEAD 로 나오고,
/// 어느 창에서 보든 같은 목록에 `current` 만 그 창의 것이다. `switchTo` 는 메인이면 `null`, 연결 워크트리면 그 꼭대기 — Host 가
/// 그 값을 활성 워크트리로 받는다.
#[test]
fn lists_the_worktrees_like_git_and_says_how_to_switch() {
    let r = TestRepo::with_commit();
    let a = r.add_worktree("wt-a", "feature-a");
    let b = r.add_worktree("wt-b", "feature-b");
    r.git_in(&b, &["checkout", "--quiet", "--detach"]);
    r.git(&["worktree", "lock", b.to_str().unwrap()]);
    let f = Fixture::new("", None);
    let id = f.project(r.root());

    let from_main = list(&f, &id, None);
    assert_eq!(ours(&from_main), porcelain(&r));
    assert_eq!(
        from_main.iter().map(|w| w.main).collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(
        from_main.iter().map(|w| w.current).collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(
        from_main
            .iter()
            .map(|w| w.switch_to.clone())
            .collect::<Vec<_>>(),
        [
            Some(GitWorktreeSwitch { worktree: None }),
            Some(GitWorktreeSwitch {
                worktree: Some(normalized(&a))
            }),
            Some(GitWorktreeSwitch {
                worktree: Some(normalized(&b))
            }),
        ]
    );
    // 각 switchTo 로 저장소를 열면 그 작업 트리다.
    for w in &from_main {
        let switch = w.switch_to.clone().unwrap();
        let repo = f
            .repo(&id, switch.worktree.as_deref().map(Path::new))
            .unwrap()
            .repo
            .unwrap();
        assert_eq!(repo.work_tree, w.path);
    }

    let from_a = list(&f, &id, Some(&a));
    assert_eq!(ours(&from_a), porcelain(&r));
    assert_eq!(
        from_a.iter().map(|w| w.current).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert_eq!(
        from_a
            .iter()
            .map(|w| w.switch_to.clone())
            .collect::<Vec<_>>(),
        from_main
            .iter()
            .map(|w| w.switch_to.clone())
            .collect::<Vec<_>>()
    );
    assert!(from_a.iter().all(|w| !w.missing));
}

/// 폴더를 지운 연결 워크트리는 목록에 남되(`git worktree list` 의 prunable) 바꿀 수 없다. 저장소가 없으면 빈 목록.
#[test]
fn a_removed_worktree_folder_is_missing_and_no_repository_lists_nothing() {
    let r = TestRepo::with_commit();
    let gone = r.add_worktree("wt-gone", "gone");
    fs::remove_dir_all(&gone).unwrap();
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    let listed = list(&f, &id, None);
    assert_eq!(listed.len(), 2);
    assert!(listed[1].missing);
    assert_eq!(listed[1].switch_to, None);

    let plain = tempfile::tempdir().unwrap();
    let none = f.project(&plain.path().join("plain"));
    let empty = f
        .git
        .worktrees(&GitRepoParams {
            project_id: none,
            worktree: None,
        })
        .unwrap();
    assert_eq!(empty.work_tree, None);
    assert!(empty.worktrees.is_empty());
}

/// 프로젝트 루트가 저장소의 하위 폴더여도(모노레포의 일부) 목록은 저장소의 작업 트리이고, 연결 워크트리로 바꾸는 값은 그
/// 워크트리의 꼭대기다(Host 가 그 안의 프로젝트 루트로 옮긴다).
#[test]
fn a_subfolder_project_switches_by_the_worktree_top() {
    let r = TestRepo::with_commit();
    r.commit_file("app/main.rs", "fn main() {}\n", "app");
    let wt = r.add_worktree("wt", "feature");
    let f = Fixture::new("", None);
    let id = f.project(&r.root().join("app"));
    let listed = list(&f, &id, None);
    assert_eq!(listed[0].path, normalized(r.root()));
    assert_eq!(
        listed[1].switch_to,
        Some(GitWorktreeSwitch {
            worktree: Some(normalized(&wt))
        })
    );
    let repo = f.repo(&id, Some(&wt)).unwrap().repo.unwrap();
    assert_eq!(repo.work_tree, normalized(&wt));
}

#[test]
fn reads_the_branch_of_a_checkout_and_a_linked_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
    fs::create_dir_all(main.join("sub/deeper")).unwrap();
    fs::write(main.join(".git/HEAD"), "ref: refs/heads/feat/a b\n").unwrap();
    assert_eq!(
        read_branch(&main.join("sub/deeper")).as_deref(),
        Some("feat/a b")
    );

    let wt = dir.path().join("wt");
    fs::create_dir_all(&wt).unwrap();
    fs::write(wt.join(".git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
    fs::write(
        main.join(".git/worktrees/wt/HEAD"),
        "ref: refs/heads/$(x);y\n",
    )
    .unwrap();
    assert_eq!(read_branch(&wt).as_deref(), Some("$(x);y"));
}

#[test]
fn a_detached_head_or_no_repository_has_no_branch() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_branch(dir.path()), None);
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    fs::write(
        dir.path().join(".git/HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .unwrap();
    assert_eq!(read_branch(dir.path()), None);
}

/// 웰컴 창의 브랜치: 프로젝트마다 주 루트의 브랜치, 분리된 HEAD · 저장소 밖이면 `null`. 진짜 git 저장소로 본다.
#[test]
fn project_branches_name_each_projects_branch_and_leave_a_detached_head_empty() {
    let on_branch = TestRepo::with_commit();
    on_branch.git(&["checkout", "--quiet", "-b", "feat/welcome"]);
    let detached = TestRepo::with_commit();
    detached.detach();
    let wt_repo = TestRepo::with_commit();
    let wt = wt_repo.add_worktree("wt", "linked");
    let plain = tempfile::tempdir().unwrap();
    let f = Fixture::new("", None);
    let ids = [
        f.project(on_branch.root()),
        f.project(detached.root()),
        f.project(&wt),
        f.project(&plain.path().join("plain")),
    ];
    let got = f.git.project_branches().unwrap().projects;
    let branch_of = |id: &str| {
        got.iter()
            .find(|p| p.project_id == id)
            .expect("every project is listed")
            .branch
            .clone()
    };
    assert_eq!(got.len(), ids.len());
    assert_eq!(branch_of(&ids[0]).as_deref(), Some("feat/welcome"));
    assert_eq!(branch_of(&ids[1]), None);
    assert_eq!(branch_of(&ids[2]).as_deref(), Some("linked"));
    assert_eq!(branch_of(&ids[3]), None);
}

/// 미신뢰 프로젝트: 저장소에 프로그램을 심어도(`core.fsmonitor` · clean 필터 · textconv) 워크트리 목록 · 웰컴 창 브랜치를 읽는 것이
/// 그 프로그램을 돌리지 않는다. 신뢰해도 목록 · 브랜치 읽기는 프로그램을 부를 일이 없다 — 같은 결과다.
#[cfg(unix)]
#[test]
fn untrusted_worktree_and_branch_reads_run_no_planted_program() {
    use std::os::unix::fs::PermissionsExt;

    let r = TestRepo::with_commit();
    let wt = r.add_worktree("wt", "feature");
    let marks = r.base().join("marks");
    let bin = r.base().join("planted");
    fs::create_dir_all(&marks).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let script = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n",
                marks.join(name).display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    };
    for (key, value) in [
        ("core.fsmonitor", script("fsmonitor", "exit 1")),
        ("filter.mark.clean", script("clean", "cat")),
        ("diff.mark.textconv", script("textconv", "cat \"$1\"")),
    ] {
        r.git(&["config", key, &value]);
    }
    fs::write(
        r.root().join(".git/info/attributes"),
        "* filter=mark diff=mark\n",
    )
    .unwrap();
    // 작업 트리를 더럽혀 둔다 — 무엇이든 status 를 셈하면 clean 필터가 돈다.
    r.write("README.md", "dirty\n");
    let marked = || fs::read_dir(&marks).unwrap().count();
    assert_eq!(marked(), 0);

    let f = Fixture::new("", None);
    let id = f.project(r.root());
    assert!(!f.projects.is_trusted(&id));
    let listed = list(&f, &id, None);
    assert_eq!(listed.len(), 2);
    assert_eq!(list(&f, &id, Some(&wt)).len(), 2);
    let branches = f.git.project_branches().unwrap().projects;
    assert_eq!(branches[0].branch.as_deref(), Some("main"));
    assert_eq!(marked(), 0, "a planted program ran on an untrusted read");
}
