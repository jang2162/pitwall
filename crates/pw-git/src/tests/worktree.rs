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

/// reftable 저장소의 `HEAD` 파일은 자리표시(`ref: refs/heads/.invalid`)다 — 그것을 브랜치로 보이지 않고 「알 수 없는 형식」 으로
/// 답한다(상태바의 「지원하지 않는 저장소」 와 같은 값). 웰컴 창은 비운다. `--ref-format` 을 모르는 git 이면 건너뛴다.
#[test]
fn a_reftable_repository_is_unsupported_not_a_branch_named_invalid() {
    use crate::status::GitUnsupported;
    use crate::worktree::{HeadBranch, read_head};

    let r = TestRepo::new();
    let top = r.base().join("rt");
    let made = r.try_git_in(
        r.base(),
        &[
            "init",
            "--quiet",
            "--ref-format=reftable",
            "-b",
            "main",
            top.to_str().unwrap(),
        ],
    );
    if !made.status.success() {
        eprintln!("skipped: this git cannot make a reftable repository");
        return;
    }
    fs::write(top.join("a.txt"), "a\n").unwrap();
    r.git_in(&top, &["add", "a.txt"]);
    r.git_in(&top, &["commit", "--quiet", "-m", "a"]);
    assert_eq!(
        fs::read_to_string(top.join(".git/HEAD")).unwrap().trim(),
        "ref: refs/heads/.invalid",
        "git's reftable placeholder"
    );
    assert_eq!(
        read_head(&top),
        HeadBranch::Unsupported(GitUnsupported::Reftable)
    );
    assert_eq!(read_branch(&top), None);

    let f = Fixture::new("", None);
    let id = f.project(&top);
    let got = f.git.project_branches().unwrap().projects;
    let row = got.iter().find(|p| p.project_id == id).unwrap();
    assert_eq!(row.branch, None);
    assert_eq!(row.unsupported, Some(GitUnsupported::Reftable));
}

/// `HEAD` 나 `.git` 이 FIFO 여도(압축으로 받은 저장소) 읽기가 막히지 않고 「없음」 이다 — 웰컴 창의 `git.projectBranches` 가
/// 멈추지 않는다. 다른 스레드에서 돌려 시간 제한을 건다.
#[cfg(unix)]
#[test]
fn a_fifo_head_or_git_file_does_not_block() {
    use std::ffi::CString;
    use std::sync::mpsc;
    use std::time::Duration;

    fn mkfifo(path: &Path) {
        let c = CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: 이 시험이 만든 임시 폴더 안의 경로로 mkfifo(3) 를 부른다.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo");
    }
    fn within<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{what}: blocked (잡힘)"))
    }

    let dir = tempfile::tempdir().unwrap();
    let head_fifo = dir.path().join("head-fifo");
    fs::create_dir_all(head_fifo.join(".git")).unwrap();
    mkfifo(&head_fifo.join(".git/HEAD"));
    let git_file_fifo = dir.path().join("git-file-fifo");
    fs::create_dir_all(&git_file_fifo).unwrap();
    mkfifo(&git_file_fifo.join(".git"));
    let linked = dir.path().join("linked");
    fs::create_dir_all(dir.path().join("admin")).unwrap();
    fs::create_dir_all(&linked).unwrap();
    fs::write(linked.join(".git"), "gitdir: ../admin\n").unwrap();
    mkfifo(&dir.path().join("admin/HEAD"));

    for root in [head_fifo.clone(), git_file_fifo.clone(), linked.clone()] {
        let shown = root.display().to_string();
        assert_eq!(within(&shown, move || read_branch(&root)), None);
    }

    // 쓰는 쪽이 열려 있고 이름이 든 FIFO 도 읽지 않는다 — 일반 파일만 HEAD 로 본다.
    let fed = dir.path().join("fed");
    fs::create_dir_all(fed.join(".git")).unwrap();
    mkfifo(&fed.join(".git/HEAD"));
    // O_RDWR 은 읽는 쪽이 없어도 막히지 않는다. 이 핸들이 열려 있는 동안 파이프에 이름이 남아 있다.
    let mut writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fed.join(".git/HEAD"))
        .unwrap();
    // 읽기 상한(4096 바이트)을 채운다 — 그러면 막히지 않는 읽기가 `EAGAIN` 없이 끝나 검사가 없을 때 이름이 그대로 나온다.
    let mut fed_head = b"ref: refs/heads/from-fifo\n".to_vec();
    fed_head.resize(4096, b'\n');
    std::io::Write::write_all(&mut writer, &fed_head).unwrap();
    let probe = fed.clone();
    assert_eq!(within("a fed FIFO", move || read_branch(&probe)), None);
    drop(writer);

    let f = Arc::new(Fixture::new("", None));
    for root in [&head_fifo, &git_file_fifo, &linked] {
        f.project(root);
    }
    let g = Arc::clone(&f);
    let rows = within("git.projectBranches", move || {
        g.git.project_branches().unwrap().projects
    });
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|p| p.branch.is_none()));
}

/// 브랜치 팝업의 merge 는 ref 전체 이름을 보낸다(apps/desktop/src/git/branch-ops.ts). 같은 이름의 브랜치와 태그가 다른 커밋을
/// 가리키면 짧은 이름은 태그로 풀린다 — git 은 「ambiguous」 경고와 함께 태그를 merge 한다. 전체 이름이면 브랜치다(모호하니
/// Host 가 짧게 바꾸지 않는다, `history::unambiguous_short`). 체크아웃
/// (`kind: "branch"`) · 삭제는 짧은 이름을 받아 Host 가 `refs/heads/` 로만 다루므로 태그와 겹치지 않는다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_branch_and_a_tag_with_the_same_name_resolve_to_the_branch_by_full_name() {
    use crate::branch::{Deleted, GitCheckoutTarget, checkout, delete};
    use crate::history::FastForward;
    use crate::history::methods::{GitMergeParams, merge};
    use crate::testing::write::{Window, WriteFixture};
    use crate::write::GitPromptAnswer;

    let f = WriteFixture::new(true);
    let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
    let main = f.repo.head();
    f.repo.git(&["tag", "v1"]);
    f.repo.git(&["checkout", "-q", "-b", "v1"]);
    let branch_tip = f.repo.commit_file("v1.txt", "branch\n", "on the branch");
    f.repo.checkout("main");
    // 재현: 짧은 이름은 태그(= main)로 풀린다.
    assert_eq!(f.repo.rev_parse("v1"), main);
    let params = |rev: &str| GitMergeParams {
        project_id: f.project.clone(),
        worktree: None,
        rev: rev.to_owned(),
        fast_forward: FastForward::Allow,
    };
    let short = merge(f.write.clone(), f.caller(), params("v1"))
        .await
        .unwrap();
    assert_eq!(
        short.outcome,
        crate::history::HistoryOutcome::UpToDate,
        "the short name merged the tag"
    );
    assert_eq!(f.repo.head(), main);

    let full = merge(f.write.clone(), f.caller(), params("refs/heads/v1"))
        .await
        .unwrap();
    assert_eq!(full.outcome, crate::history::HistoryOutcome::FastForward);
    assert_eq!(f.repo.head(), branch_tip);

    // 체크아웃 · 삭제의 짧은 이름은 브랜치다.
    f.repo.git(&["reset", "-q", "--hard", &main]);
    checkout(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        GitCheckoutTarget::Branch { name: "v1".into() },
    )
    .await
    .unwrap();
    assert_eq!(
        f.repo.git(&["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/v1"
    );
    assert_eq!(f.repo.head(), branch_tip);
    f.repo.checkout("main");
    let deleted = delete(
        &f.write,
        f.caller(),
        f.project.clone(),
        None,
        "v1".into(),
        true,
    )
    .await
    .unwrap();
    assert!(matches!(deleted, Deleted::Deleted { .. }), "{deleted:?}");
    assert_eq!(f.repo.rev_parse("refs/tags/v1"), main, "the tag stays");
    assert!(
        f.repo
            .try_git_in(
                f.repo.root(),
                &["rev-parse", "--verify", "--quiet", "refs/heads/v1"]
            )
            .stdout
            .is_empty()
    );
}

/// 팝업은 ref 전체 이름을 보내지만, 보통 경우(짧은 이름이 그 ref 하나로만 풀린다) Host 가 잠금 안에서 짧은 이름으로 git 을 부른다 —
/// merge 메시지가 터미널의 `git merge <짧은 이름>` 과 같다. 로컬 브랜치 · 원격 브랜치 · 태그 각각을 같은 저장소에서 RPC 로 한 번,
/// 되돌린 뒤 터미널로 한 번 합쳐 커밋 제목을 견준다. rebase 도 같은 규칙이라 HEAD 의 reflog 가 터미널과 같다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unambiguous_full_ref_merges_and_rebases_with_the_same_messages_as_the_terminal() {
    use crate::history::FastForward;
    use crate::history::methods::{GitMergeParams, GitRebaseParams, merge, rebase};
    use crate::testing::write::{Window, WriteFixture};
    use crate::write::GitPromptAnswer;

    let f = WriteFixture::new(true);
    let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
    let remote = f.repo.add_bare_remote("origin");
    let other = f.repo.clone_remote(&remote, "other");
    f.repo.push_from(&other, "topic", "topic.txt", "topic\n");
    f.repo.git(&["fetch", "--quiet", "origin"]);
    f.repo.git(&["checkout", "-q", "-b", "feature"]);
    f.repo.commit_file("feature.txt", "feature\n", "feature");
    f.repo.git(&["tag", "release"]);
    f.repo.checkout("main");
    f.repo.commit_file("main.txt", "main\n", "main");
    let start = f.repo.head();

    let subject = || f.repo.git(&["log", "-1", "--format=%s"]).trim().to_owned();
    for (full, short, expected) in [
        ("refs/heads/feature", "feature", "Merge branch 'feature'"),
        (
            "refs/remotes/origin/topic",
            "origin/topic",
            "Merge remote-tracking branch 'origin/topic'",
        ),
        ("refs/tags/release", "release", "Merge tag 'release'"),
    ] {
        f.repo.git(&["reset", "-q", "--hard", &start]);
        let done = merge(
            f.write.clone(),
            f.caller(),
            GitMergeParams {
                project_id: f.project.clone(),
                worktree: None,
                rev: full.to_owned(),
                fast_forward: FastForward::Never,
            },
        )
        .await
        .unwrap();
        assert_eq!(done.outcome, crate::history::HistoryOutcome::Done, "{full}");
        let ours = subject();
        f.repo.git(&["reset", "-q", "--hard", &start]);
        f.repo
            .git(&["merge", "--quiet", "--no-edit", "--no-ff", short]);
        assert_eq!(ours, subject(), "{full}: the terminal's message");
        assert_eq!(ours, expected);
    }

    // rebase: HEAD 의 reflog 제목이 터미널의 `git rebase feature` 와 같다.
    let reflog = |n: &str| f.repo.git(&["reflog", "-n", n, "--format=%gs", "HEAD"]);
    f.repo.git(&["reset", "-q", "--hard", &start]);
    rebase(
        f.write.clone(),
        f.caller(),
        GitRebaseParams {
            project_id: f.project.clone(),
            worktree: None,
            upstream: "refs/heads/feature".into(),
            branch: None,
        },
    )
    .await
    .unwrap();
    let ours = reflog("3");
    f.repo.git(&["reset", "-q", "--hard", &start]);
    f.repo.git(&["rebase", "--quiet", "feature"]);
    assert_eq!(ours, reflog("3"));
    assert!(ours.contains("checkout feature"), "{ours}");
}

/// 짧은 이름 판정: 그 ref 하나로만 풀릴 때만 짧은 이름이다. 같은 이름의 태그 · 다른 종류의 브랜치 · 원격의 HEAD · 의사 ref 꼴 ·
/// 해시 길이의 16진수 · 전체 이름이 아닌 것 · 없는 ref 는 `None`(전체 이름 그대로).
#[test]
fn a_short_name_is_used_only_when_it_resolves_to_that_ref_alone() {
    use crate::history::unambiguous_short;

    let r = TestRepo::with_commit();
    let head = r.head();
    r.git(&["branch", "plain"]);
    r.git(&["branch", "both"]);
    r.git(&["tag", "both"]);
    r.git(&["tag", "only-tag"]);
    r.git(&["update-ref", "refs/remotes/origin/main", &head]);
    r.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    r.git(&["branch", "origin"]);
    r.git(&["update-ref", "refs/remotes/up/feat", &head]);
    r.git(&["branch", "up/feat"]);
    let hex = "a".repeat(40);
    r.git(&["branch", &hex]);
    r.git(&["branch", "ORIG"]);
    let repo = gix::open(r.root()).unwrap();
    let short = |full: &str| unambiguous_short(&repo, full);
    assert_eq!(short("refs/heads/plain").as_deref(), Some("plain"));
    assert_eq!(short("refs/tags/only-tag").as_deref(), Some("only-tag"));
    assert_eq!(
        short("refs/remotes/origin/main").as_deref(),
        Some("origin/main")
    );
    // 같은 이름의 태그와 브랜치 — 어느 쪽도 짧게 하지 않는다.
    assert_eq!(short("refs/heads/both"), None);
    assert_eq!(short("refs/tags/both"), None);
    // 브랜치 `origin` 과 `refs/remotes/origin/HEAD`.
    assert_eq!(short("refs/heads/origin"), None);
    // 로컬 `up/feat` 와 원격 `up/feat`.
    assert_eq!(short("refs/heads/up/feat"), None);
    assert_eq!(short("refs/remotes/up/feat"), None);
    assert_eq!(short(&format!("refs/heads/{hex}")), None);
    assert_eq!(short("refs/heads/ORIG"), None);
    assert_eq!(short("refs/heads/missing"), None);
    assert_eq!(short("plain"), None);
    assert_eq!(short("refs/notes/commits"), None);
}
