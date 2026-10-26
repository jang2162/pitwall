//! `git.repo`(M2-01 완료 기준 ①): 보통 저장소 · 연결 워크트리 · 분리된 HEAD · 태어나지 않은 브랜치 · 진행 중 작업 · 저장소 밖.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use super::*;
use crate::methods::{RepoHead, RepoInfo, RepoOperation};
use crate::testing::TestRepo;

fn s(path: &Path) -> String {
    path.display().to_string()
}

fn info(f: &Fixture, project: &str, worktree: Option<&Path>) -> RepoInfo {
    f.repo(project, worktree)
        .unwrap()
        .repo
        .expect("a repository")
}

/// `.git` 아래 모든 파일의 (크기, 수정 시각). 읽기가 아무것도 쓰지 않았는지 본다.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                out.insert(entry.path(), (meta.len(), meta.modified().unwrap()));
            }
        }
    }
    out
}

#[test]
fn a_plain_repository_is_on_its_branch() {
    let r = TestRepo::with_commit();
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    let before = snapshot(&r.root().join(".git"));
    assert_eq!(
        info(&f, &id, None),
        RepoInfo {
            work_tree: s(r.root()),
            git_dir: s(&r.root().join(".git")),
            common_dir: s(&r.root().join(".git")),
            linked_worktree: false,
            head: RepoHead::Branch {
                name: "main".into(),
                commit: r.head(),
            },
            operation: None,
        }
    );
    // gix 로 읽었다 — 잠금 파일 · index 갱신이 없다.
    assert_eq!(snapshot(&r.root().join(".git")), before);
}

/// 프로젝트 루트가 저장소의 하위 폴더(모노레포의 한 패키지)여도 저장소 전체다(결정 M2-D3).
#[test]
fn a_project_inside_a_repository_uses_the_whole_repository() {
    let r = TestRepo::with_commit();
    let f = Fixture::new("", None);
    let id = f.project(&r.root().join("packages/app"));
    let got = info(&f, &id, None);
    assert_eq!(got.work_tree, s(r.root()));
    assert_eq!(
        got.head,
        RepoHead::Branch {
            name: "main".into(),
            commit: r.head()
        }
    );
}

#[test]
fn a_linked_worktree_reads_its_own_head_and_shares_the_common_dir() {
    let r = TestRepo::with_commit();
    r.commit_file("packages/app/a.txt", "a\n", "app");
    let wt = r.add_worktree("wt", "feat/x");
    let wt_commit = r.commit_all_in(&wt, "on the worktree");
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    let expected = RepoInfo {
        work_tree: s(&wt),
        git_dir: s(&r.root().join(".git/worktrees/wt")),
        common_dir: s(&r.root().join(".git")),
        linked_worktree: true,
        head: RepoHead::Branch {
            name: "feat/x".into(),
            commit: wt_commit,
        },
        operation: None,
    };
    // 창이 넘긴 활성 워크트리.
    assert_eq!(info(&f, &id, Some(&wt)), expected);
    // 메인 체크아웃은 그대로 main 이다.
    assert_eq!(
        info(&f, &id, None).head,
        RepoHead::Branch {
            name: "main".into(),
            commit: r.head()
        }
    );
    // 모노레포의 하위 폴더 프로젝트도 워크트리 안의 같은 폴더에서 그 워크트리의 저장소다.
    let sub = f.project(&r.root().join("packages/app"));
    assert_eq!(info(&f, &sub, Some(&wt)), expected);
    // 워크트리 폴더 자체를 프로젝트로 등록해도 같다(`.git` 파일의 위조 판정을 지난다).
    let wt_project = f.project(&wt);
    assert_eq!(info(&f, &wt_project, None), expected);
}

#[test]
fn a_detached_head_reads_the_commit() {
    let r = TestRepo::with_commit();
    let first = r.head();
    r.commit_file("b.txt", "b\n", "second");
    r.checkout(&first);
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    assert_eq!(
        info(&f, &id, None).head,
        RepoHead::Detached { commit: first }
    );
    // 주석 태그를 체크아웃해도 그 커밋이다.
    r.checkout("main");
    r.git(&["tag", "-a", "-m", "v1", "v1", "HEAD"]);
    r.checkout("v1");
    assert_eq!(
        info(&f, &id, None).head,
        RepoHead::Detached { commit: r.head() }
    );
}

#[test]
fn an_unborn_branch_has_a_name_but_no_commit() {
    let r = TestRepo::new();
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    assert_eq!(
        info(&f, &id, None).head,
        RepoHead::Unborn {
            name: "main".into()
        }
    );
    // 커밋이 있는 저장소의 고아 브랜치도 태어나지 않았다.
    r.commit_file("a.txt", "a\n", "first");
    r.git(&["checkout", "--quiet", "--orphan", "fresh"]);
    assert_eq!(
        info(&f, &id, None).head,
        RepoHead::Unborn {
            name: "fresh".into()
        }
    );
}

#[test]
fn a_merge_in_progress_is_read() {
    let r = TestRepo::with_commit();
    r.start_conflicting_merge();
    assert!(r.root().join(".git/MERGE_HEAD").is_file());
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    let got = info(&f, &id, None);
    assert_eq!(got.operation, Some(RepoOperation::Merge));
    assert_eq!(
        got.head,
        RepoHead::Branch {
            name: "main".into(),
            commit: r.head()
        }
    );
}

#[test]
fn other_operations_in_progress_are_read() {
    let f = Fixture::new("", None);
    type Start = fn(&TestRepo);
    let cases: [(Start, RepoOperation); 4] = [
        (
            |r| {
                r.start_conflicting_cherry_pick();
            },
            RepoOperation::CherryPick,
        ),
        (
            |r| {
                r.start_conflicting_rebase();
            },
            RepoOperation::Rebase,
        ),
        (
            |r| {
                r.start_conflicting_revert();
            },
            RepoOperation::Revert,
        ),
        (
            |r| {
                r.commit_file("b.txt", "b\n", "b");
                r.start_bisect();
            },
            RepoOperation::Bisect,
        ),
    ];
    for (start, expected) in cases {
        let r = TestRepo::with_commit();
        start(&r);
        let id = f.project(r.root());
        assert_eq!(info(&f, &id, None).operation, Some(expected));
    }
}

#[test]
fn a_merge_in_progress_in_a_linked_worktree_is_its_own() {
    let r = TestRepo::with_commit();
    let d = r.diverge();
    let wt = r.add_worktree("wt", "side");
    let out = r.try_git_in(&wt, &["merge", &d.theirs]);
    assert!(!out.status.success());
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    assert_eq!(
        info(&f, &id, Some(&wt)).operation,
        Some(RepoOperation::Merge)
    );
    assert_eq!(info(&f, &id, None).operation, None);
}

#[test]
fn outside_a_repository_there_is_none() {
    let r = TestRepo::with_commit();
    let f = Fixture::new("", None);
    let outside = r.base().join("outside/project");
    let id = f.project(&outside);
    assert_eq!(f.repo(&id, None).unwrap(), GitRepo { repo: None });
}

#[test]
fn a_bare_repository_has_no_work_tree_so_none() {
    let r = TestRepo::with_commit();
    let bare = r.base().join("bare.git");
    r.git(&["clone", "--quiet", "--bare", ".", bare.to_str().unwrap()]);
    let f = Fixture::new("", None);
    let id = f.project(&bare);
    assert_eq!(f.repo(&id, None).unwrap(), GitRepo { repo: None });
    // 그 bare 저장소의 연결 워크트리는 저장소다(메인 체크아웃은 없다).
    let wt = r.base().join("bare-wt");
    r.git_in(
        &bare,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "w",
            wt.to_str().unwrap(),
        ],
    );
    let wt_project = f.project(&wt);
    let got = info(&f, &wt_project, None);
    assert_eq!(got.work_tree, s(&wt));
    assert_eq!(got.common_dir, s(&bare));
    assert!(got.linked_worktree);
}

/// 관리 폴더 · `commondir` · 되돌림 포인터를 제 폴더 안에 둔 위조 `.git` 파일(M1-05)은 저장소가 아니다 — 남의 저장소의 ref ·
/// 객체를 그 폴더의 git 기능으로 보이지 않는다.
#[test]
fn a_forged_worktree_git_file_is_not_a_repository() {
    let r = TestRepo::with_commit();
    let evil = r.base().join("downloads/evil");
    fs::create_dir_all(evil.join(".admin")).unwrap();
    fs::write(evil.join(".git"), "gitdir: .admin\n").unwrap();
    fs::write(evil.join(".admin/gitdir"), "../.git\n").unwrap();
    fs::write(
        evil.join(".admin/commondir"),
        format!("{}\n", r.root().join(".git").display()),
    )
    .unwrap();
    fs::write(evil.join(".admin/HEAD"), "ref: refs/heads/main\n").unwrap();
    // gix 는 이것을 연결 워크트리로 연다 — 판정이 없으면 이 시험이 실패한다.
    assert!(gix::open(&evil).is_ok());
    let f = Fixture::new("", None);
    let id = f.project(&evil);
    assert_eq!(f.repo(&id, None).unwrap(), GitRepo { repo: None });
    // 활성 워크트리로 넘겨도 거절한다(그 프로젝트의 워크트리가 아니다).
    let main = f.project(r.root());
    let err = f.repo(&main, Some(&evil)).unwrap_err();
    assert_eq!(err.code, pw_rpc::code::INVALID_PARAMS, "{err:?}");
}

#[test]
fn a_bad_worktree_argument_or_project_is_invalid_params() {
    let r = TestRepo::with_commit();
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    for worktree in [Path::new("relative/wt"), r.base()] {
        let err = f.repo(&id, Some(worktree)).unwrap_err();
        assert_eq!(
            err.code,
            pw_rpc::code::INVALID_PARAMS,
            "{worktree:?}: {err:?}"
        );
    }
    let err = f.repo("no-such-project", None).unwrap_err();
    assert_eq!(err.code, pw_rpc::code::INVALID_PARAMS, "{err:?}");
}

/// `.git` 파일이 다른 저장소의 `.git`(관리 폴더가 아니라 본체)을 직접 가리켜도 저장소가 아니다 — 서브모듈이면 그 관리 폴더의
/// `core.worktree` 가 이 폴더를 되가리켜야 한다(M1-05 와 같은 되돌림 판정, `pw_core::config::gitdir::git_file`).
#[test]
fn a_git_file_pointing_straight_at_another_repository_is_not_a_repository() {
    let victim = TestRepo::with_commit();
    let r = TestRepo::with_commit();
    let evil = r.base().join("downloads/evil");
    fs::create_dir_all(&evil).unwrap();
    fs::write(
        evil.join(".git"),
        format!("gitdir: {}\n", victim.root().join(".git").display()),
    )
    .unwrap();
    // gix 는 이것을 victim 의 저장소로 연다 — 판정이 없으면 이 시험이 실패한다.
    assert!(gix::open(&evil).is_ok());
    let f = Fixture::new("", None);
    let id = f.project(&evil);
    assert_eq!(f.repo(&id, None).unwrap(), GitRepo { repo: None });
}

/// git 이 만든 진짜 서브모듈 폴더는 그 서브모듈의 저장소다(관리 폴더 `.git/modules/<이름>` 의 `core.worktree` 가 되가리킨다).
#[test]
fn a_submodule_is_its_own_repository() {
    let sub = TestRepo::with_commit();
    let r = TestRepo::with_commit();
    r.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        sub.root().to_str().unwrap(),
        "m",
    ]);
    let module = r.root().join("m");
    assert!(module.join(".git").is_file());
    let f = Fixture::new("", None);
    let id = f.project(&module);
    let got = info(&f, &id, None);
    assert_eq!(got.work_tree, s(&module));
    assert_eq!(got.git_dir, s(&r.root().join(".git/modules/m")));
    assert_eq!(
        got.head,
        RepoHead::Branch {
            name: "main".into(),
            commit: sub.head()
        }
    );
}

/// `core.worktree` 가 `.git` 이 있는 폴더가 아닌 곳을 가리키면 「저장소 없음」 이다 — git CLI 는 작업 트리에서 다시 찾아 다른
/// 저장소(또는 없음)를 보므로, 읽기(gix)와 쓰기(CLI)가 다른 저장소를 다루게 된다.
#[test]
fn a_work_tree_moved_elsewhere_by_core_worktree_is_none() {
    let r = TestRepo::with_commit();
    let f = Fixture::new("", None);
    let id = f.project(r.root());
    assert!(f.repo(&id, None).unwrap().repo.is_some());
    let other = r.base().join("other");
    fs::create_dir_all(&other).unwrap();
    r.git(&["config", "core.worktree", other.to_str().unwrap()]);
    // 미신뢰(gix 가 저장소 설정의 `core.worktree` 를 쓰지 않는다) · 신뢰(쓴다) 모두.
    for trusted in [false, true] {
        f.trust(&id, trusted);
        assert_eq!(
            f.repo(&id, None).unwrap(),
            GitRepo { repo: None },
            "trusted: {trusted}"
        );
    }
    // 같은 폴더를 가리키는 값(상대 경로 포함)은 그대로 저장소다.
    r.git(&["config", "core.worktree", ".."]);
    for trusted in [false, true] {
        f.trust(&id, trusted);
        assert_eq!(info(&f, &id, None).work_tree, s(r.root()));
    }
}

/// `git init --separate-git-dir` 저장소(저장소 폴더에 `commondir` 도 `core.worktree` 도 없다)는 그 작업 트리의 저장소다 — 신뢰와
/// 상관없이.
#[test]
fn a_separate_git_dir_repository_is_a_repository() {
    let r = TestRepo::new();
    let store = r.base().join("store/proj.git");
    let proj = r.base().join("proj");
    fs::create_dir_all(r.base().join("store")).unwrap();
    r.git_in(
        r.base(),
        &[
            "init",
            "-q",
            &format!("--separate-git-dir={}", store.display()),
            proj.to_str().unwrap(),
        ],
    );
    r.git_in(&proj, &["commit", "-q", "--allow-empty", "-m", "x"]);
    assert!(proj.join(".git").is_file());
    let f = Fixture::new("", None);
    let id = f.project(&proj);
    for trusted in [false, true] {
        f.trust(&id, trusted);
        let got = info(&f, &id, None);
        assert_eq!(got.work_tree, s(&proj), "trusted: {trusted}");
        assert_eq!(got.git_dir, s(&store));
        assert_eq!(got.common_dir, s(&store));
        assert!(!got.linked_worktree);
        assert_eq!(
            got.head,
            RepoHead::Branch {
                name: "main".into(),
                commit: r.rev_parse_in(&proj, "HEAD"),
            }
        );
    }
}
