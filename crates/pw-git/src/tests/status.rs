//! `git.status` · `git.refs`(M2-02)가 git CLI 의 읽기와 같은지 본다: `git status --porcelain=v2 -z --untracked-files=all
//! --ignore-submodules=dirty` · `git for-each-ref` · `git rev-list --left-right --count`. 비교용 git 은 시험이 격리 환경으로 띄운다
//! (`testing::repo::git_command`, 읽기는 `GIT_OPTIONAL_LOCKS=0`) — Pitwall 의 코드는 git 을 띄우지 않는다.

use std::collections::BTreeMap;

use super::*;
use crate::methods::RepoHead;
use crate::status::{
    GitChange, GitConflict, GitRefs, GitStatus, GitStatusEntry, GitStatusParams, Status,
};
use crate::testing::TestRepo;

/// `Fixture` 위에 `git.status` 서비스를 얹는다.
pub(super) struct Fx {
    pub(super) _tmp: tempfile::TempDir,
    pub(super) git: Arc<Git>,
    pub(super) projects: Projects,
    pub(super) status: Arc<Status>,
}

impl Fx {
    pub(super) fn new() -> Self {
        let f = Fixture::new("", None);
        let Fixture {
            _tmp,
            git,
            projects,
            ..
        } = f;
        let git = Arc::new(git);
        let status = Arc::new(Status::new(Arc::clone(&git)));
        Self {
            _tmp,
            git,
            projects,
            status,
        }
    }

    pub(super) fn bus(&self) -> EventBus {
        self.git.host().unwrap().bus.clone()
    }

    pub(super) fn project(&self, root: &Path) -> String {
        fs::create_dir_all(root).unwrap();
        self.projects
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
            .id
    }

    pub(super) fn status(&self, id: &str, paths: Option<&[&str]>) -> GitStatus {
        self.try_status(id, None, paths).unwrap()
    }

    pub(super) fn try_status(
        &self,
        id: &str,
        worktree: Option<&Path>,
        paths: Option<&[&str]>,
    ) -> Result<GitStatus, RpcError> {
        rt().block_on(self.status.status(GitStatusParams {
            project_id: id.to_owned(),
            worktree: worktree.map(|w| w.display().to_string()),
            paths: paths.map(|p| p.iter().map(|s| (*s).to_owned()).collect()),
        }))
    }

    pub(super) fn refs(&self, id: &str, worktree: Option<&Path>) -> GitRefs {
        self.status
            .refs(&GitRepoParams {
                project_id: id.to_owned(),
                worktree: worktree.map(|w| w.display().to_string()),
            })
            .unwrap()
    }
}

pub(super) fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// 비교용 git 읽기. 사람의 설정을 읽지 않고(`git_command`), index 를 다시 쓰지 않는다.
pub(super) fn git_read(r: &TestRepo, dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = r
        .command_in(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn change(c: u8) -> Option<GitChange> {
    match c {
        b'.' => None,
        b'A' => Some(GitChange::Added),
        b'M' => Some(GitChange::Modified),
        b'D' => Some(GitChange::Deleted),
        b'R' => Some(GitChange::Renamed),
        b'C' => Some(GitChange::Copied),
        b'T' => Some(GitChange::TypeChanged),
        other => panic!("unexpected status letter {}", other as char),
    }
}

fn conflict(xy: &[u8]) -> GitConflict {
    match xy {
        b"DD" => GitConflict::BothDeleted,
        b"AU" => GitConflict::AddedByUs,
        b"UD" => GitConflict::DeletedByThem,
        b"UA" => GitConflict::AddedByThem,
        b"DU" => GitConflict::DeletedByUs,
        b"AA" => GitConflict::BothAdded,
        b"UU" => GitConflict::BothModified,
        other => panic!("unexpected conflict {}", String::from_utf8_lossy(other)),
    }
}

/// `git status --porcelain=v2 -z --untracked-files=all --ignore-submodules=dirty` 을 `git.status` 의 꼴로 읽는다. `paths` 는
/// pathspec 으로 넘긴다.
pub(super) fn porcelain(r: &TestRepo, dir: &Path, paths: &[&str]) -> Vec<GitStatusEntry> {
    let mut args = vec![
        "status",
        "--porcelain=v2",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=dirty",
    ];
    let specs: Vec<String> = paths.iter().map(|p| format!(":(top,literal){p}")).collect();
    if !specs.is_empty() {
        args.push("--");
        args.extend(specs.iter().map(String::as_str));
    }
    let out = git_read(r, dir, &args);
    let mut records = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut entries = BTreeMap::new();
    let text = |b: &[u8]| String::from_utf8(b.to_vec()).unwrap();
    while let Some(record) = records.next() {
        let fields: Vec<&[u8]> = record.splitn(11, |b| *b == b' ').collect();
        let entry = match fields[0] {
            b"1" => {
                let path = text(record.splitn(9, |b| *b == b' ').nth(8).unwrap());
                GitStatusEntry {
                    path,
                    orig_path: None,
                    staged: change(fields[1][0]),
                    unstaged: change(fields[1][1]),
                    conflict: None,
                }
            }
            b"2" => {
                let path = text(record.splitn(10, |b| *b == b' ').nth(9).unwrap());
                let orig = text(records.next().unwrap());
                GitStatusEntry {
                    path,
                    orig_path: Some(orig),
                    staged: change(fields[1][0]),
                    unstaged: change(fields[1][1]),
                    conflict: None,
                }
            }
            b"u" => {
                let path = text(record.splitn(11, |b| *b == b' ').nth(10).unwrap());
                GitStatusEntry {
                    path,
                    orig_path: None,
                    staged: None,
                    unstaged: None,
                    conflict: Some(conflict(fields[1])),
                }
            }
            b"?" => GitStatusEntry {
                path: text(&record[2..]),
                orig_path: None,
                staged: None,
                unstaged: Some(GitChange::Untracked),
                conflict: None,
            },
            other => panic!("unexpected record {}", String::from_utf8_lossy(other)),
        };
        // index 에서만 지운 파일은 git 이 두 줄(`1 D.` · `?`)로 낸다 — `git.status` 는 경로 하나에 합친다.
        let merged = entries
            .entry(entry.path.clone())
            .or_insert_with(|| GitStatusEntry {
                path: entry.path.clone(),
                orig_path: None,
                staged: None,
                unstaged: None,
                conflict: None,
            });
        merged.orig_path = merged.orig_path.take().or(entry.orig_path);
        merged.staged = merged.staged.or(entry.staged);
        merged.unstaged = merged.unstaged.or(entry.unstaged);
        merged.conflict = merged.conflict.or(entry.conflict);
    }
    entries.into_values().collect()
}

/// `git.status` 가 porcelain 과 같다. 경로 한정이면 전체 porcelain 에서 그 경로(이름 바뀜이면 원래 경로도)의 항목만 남긴 것과
/// 같다 — git 은 pathspec 밖의 짝을 보지 않아 스테이징한 이름 바뀜을 「더함」 으로 내지만 `git.status` 는 짝을 찾는다.
pub(super) fn assert_same_status(fx: &Fx, r: &TestRepo, id: &str, paths: Option<&[&str]>) {
    let got = fx.status(id, paths);
    assert_eq!(got.work_tree.as_deref(), Some(r.root().to_str().unwrap()));
    let mut want = porcelain(r, r.root(), &[]);
    if let Some(paths) = paths {
        let within = |p: &str| paths.iter().any(|l| crate::status::paths::is_within(p, l));
        want.retain(|e| within(&e.path) || e.orig_path.as_deref().is_some_and(within));
    }
    assert_eq!(got.entries, want);
}

/// 한 작업 트리에 여러 종류의 변경: 수정 · 스테이징 뒤 다시 수정 · index 에서만 지움 · 작업 트리에서 지움 · 이름 바꿈 · 새 파일 ·
/// untracked(폴더 안 파일마다) · intent-to-add · 중첩 저장소 · ignore 된 파일 · 바이트순 정렬.
#[test]
fn the_status_matches_git_porcelain_v2() {
    let r = TestRepo::new();
    for (path, body) in [
        ("a.txt", "a\n"),
        ("b.txt", "b\n"),
        ("c.txt", "c\n"),
        ("d.txt", "d\n"),
        ("dir/e.txt", "e\nshared body so the rename is found\n"),
        ("Z.txt", "z\n"),
        (".gitignore", "*.log\n"),
    ] {
        r.write(path, body);
    }
    r.commit_all("initial");
    r.write("a.txt", "a changed\n");
    r.write("b.txt", "b staged\n");
    r.git(&["add", "b.txt"]);
    r.write("b.txt", "b staged then changed\n");
    r.git(&["rm", "--quiet", "--cached", "c.txt"]);
    fs::remove_file(r.root().join("d.txt")).unwrap();
    r.git(&["mv", "dir/e.txt", "dir/f.txt"]);
    r.write("new.txt", "new\n");
    r.git(&["add", "new.txt"]);
    r.write("untracked/x/y.txt", "y\n");
    r.write("untracked/z.txt", "z\n");
    r.write("intent.txt", "intent\n");
    r.git(&["add", "--intent-to-add", "intent.txt"]);
    r.write("ignored.log", "ignored\n");
    r.write("nested/inner.txt", "inner\n");
    r.git_in(&r.root().join("nested"), &["init", "--quiet"]);
    r.write("한글.txt", "utf-8\n");

    let fx = Fx::new();
    let id = fx.project(r.root());
    assert_same_status(&fx, &r, &id, None);
    let got = fx.status(&id, None);
    let by_path = |p: &str| got.entries.iter().find(|e| e.path == p).unwrap().clone();
    assert_eq!(by_path("dir/f.txt").orig_path.as_deref(), Some("dir/e.txt"));
    assert_eq!(by_path("dir/f.txt").staged, Some(GitChange::Renamed));
    assert_eq!(by_path("intent.txt").unstaged, Some(GitChange::Added));
    assert_eq!(by_path("nested/").unstaged, Some(GitChange::Untracked));
    assert!(got.entries.iter().all(|e| e.path != "ignored.log"));
}

/// 파일 ↔ 심링크, 실행 비트(unix).
#[cfg(unix)]
#[test]
fn type_changes_and_the_executable_bit_match_git() {
    use std::os::unix::fs::PermissionsExt;
    let r = TestRepo::new();
    r.write("file.txt", "f\n");
    r.write("script.sh", "#!/bin/sh\n");
    r.write("staged.txt", "s\n");
    r.write("staged-exec.sh", "#!/bin/sh\n");
    r.commit_all("initial");
    // 실행 비트만 바꿔 담은 것은 `M.`(종류 바뀜이 아니다).
    fs::set_permissions(
        r.root().join("staged-exec.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    r.git(&["add", "staged-exec.sh"]);
    fs::remove_file(r.root().join("file.txt")).unwrap();
    std::os::unix::fs::symlink("script.sh", r.root().join("file.txt")).unwrap();
    fs::set_permissions(
        r.root().join("script.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::remove_file(r.root().join("staged.txt")).unwrap();
    std::os::unix::fs::symlink("script.sh", r.root().join("staged.txt")).unwrap();
    r.git(&["add", "staged.txt"]);
    let fx = Fx::new();
    let id = fx.project(r.root());
    assert_same_status(&fx, &r, &id, None);
    let got = fx.status(&id, None);
    assert_eq!(got.entries.len(), 4, "{got:?}");
}

/// 충돌의 종류(UU · AA · UD · DU)가 git 과 같고, 충돌한 경로는 스테이징 쪽이 비어 있다.
#[test]
fn conflicts_match_git() {
    let r = TestRepo::with_commit();
    r.write("both-modified.txt", "base\n");
    r.write("deleted-by-them.txt", "base\n");
    r.write("deleted-by-us.txt", "base\n");
    r.commit_all("base");
    r.git(&["checkout", "--quiet", "-b", "theirs"]);
    r.write("both-modified.txt", "theirs\n");
    r.write("both-added.txt", "theirs\n");
    r.write("deleted-by-us.txt", "theirs\n");
    fs::remove_file(r.root().join("deleted-by-them.txt")).unwrap();
    r.commit_all("theirs");
    r.checkout("main");
    r.write("both-modified.txt", "ours\n");
    r.write("both-added.txt", "ours\n");
    r.write("deleted-by-them.txt", "ours\n");
    fs::remove_file(r.root().join("deleted-by-us.txt")).unwrap();
    r.write("clean-change.txt", "staged during the merge\n");
    r.commit_all("ours");
    let out = r.try_git_in(r.root(), &["merge", "theirs"]);
    assert!(!out.status.success());
    let fx = Fx::new();
    let id = fx.project(r.root());
    assert_same_status(&fx, &r, &id, None);
    let got = fx.status(&id, None);
    let conflicts: Vec<_> = got
        .entries
        .iter()
        .filter_map(|e| e.conflict.map(|c| (e.path.as_str(), c, e.staged)))
        .collect();
    assert_eq!(
        conflicts,
        [
            ("both-added.txt", GitConflict::BothAdded, None),
            ("both-modified.txt", GitConflict::BothModified, None),
            ("deleted-by-them.txt", GitConflict::DeletedByThem, None),
            ("deleted-by-us.txt", GitConflict::DeletedByUs, None),
        ]
    );
}

/// 경로 한정: 그 경로(폴더면 그 아래)의 항목만, 스테이징한 이름 바뀜은 새 경로만 불러도 원래 경로와 함께 나온다.
#[test]
fn a_path_limited_status_reports_only_those_paths() {
    let r = TestRepo::new();
    r.write("a.txt", "a\n");
    r.write("dir/e.txt", "e\n");
    r.write("dir/sub/g.txt", "g\n");
    r.write("dirx/h.txt", "h\n");
    r.write("old.txt", "renamed content that is long enough\n");
    r.commit_all("initial");
    r.write("a.txt", "a2\n");
    r.write("dir/e.txt", "e2\n");
    r.write("dir/sub/g.txt", "g2\n");
    r.write("dirx/h.txt", "h2\n");
    r.write("dir/new.txt", "new\n");
    r.git(&["mv", "old.txt", "renamed.txt"]);
    let fx = Fx::new();
    let id = fx.project(r.root());
    for paths in [
        &["dir"][..],
        &["dir/"],
        &["a.txt"],
        &["dir/sub", "a.txt"],
        &["renamed.txt"],
        &["old.txt"],
        &["missing.txt"],
    ] {
        assert_same_status(&fx, &r, &id, Some(paths));
    }
    let got = fx.status(&id, Some(&["dir"]));
    let paths: Vec<&str> = got.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["dir/e.txt", "dir/new.txt", "dir/sub/g.txt"]);
    let got = fx.status(&id, Some(&["renamed.txt"]));
    assert_eq!(got.entries.len(), 1);
    assert_eq!(got.entries[0].orig_path.as_deref(), Some("old.txt"));
    assert_eq!(got.entries[0].staged, Some(GitChange::Renamed));
    // 원래 경로로 불러도 그 이름 바뀜이 나온다(받는 쪽은 옛 경로 자리의 항목을 이것으로 바꾼다).
    assert_eq!(fx.status(&id, Some(&["old.txt"])), got);
    // 빈 경로는 전체다.
    assert_eq!(fx.status(&id, Some(&[""])), fx.status(&id, None));
}

/// 경로는 글자 그대로다 — `*` 가 든 이름은 패턴이 아니다(unix: Windows 는 그 이름을 만들 수 없다).
#[cfg(unix)]
#[test]
fn path_limits_are_literal() {
    let r = TestRepo::new();
    r.write("star*.txt", "s\n");
    r.write("starX.txt", "x\n");
    r.commit_all("initial");
    r.write("starX.txt", "x2\n");
    let fx = Fx::new();
    let id = fx.project(r.root());
    assert_eq!(fx.status(&id, Some(&["star*.txt"])).entries, []);
    r.write("star*.txt", "s2\n");
    let got = fx.status(&id, Some(&["star*.txt"]));
    let paths: Vec<&str> = got.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["star*.txt"]);
}

#[test]
fn paths_outside_the_work_tree_are_invalid_params() {
    let r = TestRepo::with_commit();
    let fx = Fx::new();
    let id = fx.project(r.root());
    for bad in ["/etc", "../x", "a/../../x"] {
        let e = fx.try_status(&id, None, Some(&[bad])).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{bad}: {e:?}");
    }
}

/// 저장소 밖이면 `workTree: null` · 빈 목록, 태어나지 않은 브랜치(커밋 없음)도 읽는다.
#[test]
fn outside_a_repository_and_on_an_unborn_branch() {
    let r = TestRepo::new();
    let outside = r.base().join("outside");
    let fx = Fx::new();
    let out_id = fx.project(&outside);
    assert_eq!(
        fx.status(&out_id, None),
        GitStatus {
            work_tree: None,
            entries: Vec::new()
        }
    );
    let refs = fx.refs(&out_id, None);
    assert_eq!(refs.work_tree, None);
    assert!(refs.branches.is_empty() && refs.head.is_none());

    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.git(&["add", "a.txt"]);
    let id = fx.project(r.root());
    assert_same_status(&fx, &r, &id, None);
    let refs = fx.refs(&id, None);
    assert_eq!(
        refs.head,
        Some(RepoHead::Unborn {
            name: "main".into()
        })
    );
    assert!(refs.branches.is_empty());
}

/// 연결 워크트리 창은 그 워크트리의 index · HEAD 로 읽는다.
#[test]
fn a_linked_worktree_has_its_own_status() {
    let r = TestRepo::with_commit();
    let wt = r.add_worktree("wt", "feature");
    fs::write(wt.join("README.md"), "changed in the worktree\n").unwrap();
    fs::write(wt.join("only-here.txt"), "x\n").unwrap();
    r.write("main-only.txt", "y\n");
    let fx = Fx::new();
    let id = fx.project(r.root());
    let got = fx.try_status(&id, Some(&wt), None).unwrap();
    assert_eq!(got.work_tree.as_deref(), Some(wt.to_str().unwrap()));
    assert_eq!(got.entries, porcelain(&r, &wt, &[]));
    assert_eq!(got.entries.len(), 2);
    let refs = fx.refs(&id, Some(&wt));
    assert_eq!(
        refs.branches
            .iter()
            .find(|b| b.head)
            .map(|b| b.name.as_str()),
        Some("feature")
    );
}

/// `git for-each-ref` 한 줄: (ref 이름, 객체, upstream, 벗긴 객체).
type RefLine = (String, String, String, String);

fn for_each_ref(r: &TestRepo) -> Vec<RefLine> {
    let out = git_read(
        r,
        r.root(),
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)%00%(*objectname)%00%(symref)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    );
    String::from_utf8(out)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\0').collect();
            // 심볼릭 ref(`origin/HEAD`)는 `git.refs` 에 없다.
            (f[4].is_empty()).then(|| (f[0].into(), f[1].into(), f[2].into(), f[3].into()))
        })
        .collect()
}

fn ref_lines(refs: &GitRefs) -> Vec<RefLine> {
    let mut lines: Vec<RefLine> = refs
        .branches
        .iter()
        .map(|b| {
            (
                b.ref_name.clone(),
                b.commit.clone(),
                b.upstream
                    .as_ref()
                    .map(|u| u.ref_name.clone())
                    .unwrap_or_default(),
                String::new(),
            )
        })
        .chain(refs.remote_branches.iter().map(|b| {
            (
                b.ref_name.clone(),
                b.commit.clone(),
                String::new(),
                String::new(),
            )
        }))
        .chain(refs.tags.iter().map(|t| {
            (
                t.ref_name.clone(),
                t.target.clone(),
                String::new(),
                t.peeled.clone().unwrap_or_default(),
            )
        }))
        .collect();
    lines.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    lines
}

pub(super) fn assert_same_refs(fx: &Fx, r: &TestRepo, id: &str) {
    let refs = fx.refs(id, None);
    assert_eq!(ref_lines(&refs), for_each_ref(r));
    for branch in &refs.branches {
        let Some(upstream) = &branch.upstream else {
            continue;
        };
        let range = format!("{}...{}", branch.ref_name, upstream.ref_name);
        let out = r.try_git_in(r.root(), &["rev-list", "--left-right", "--count", &range]);
        match &upstream.track {
            Some(track) => {
                assert!(out.status.success(), "{range}");
                let text = String::from_utf8(out.stdout).unwrap();
                let (ahead, behind) = text.trim().split_once('\t').unwrap();
                assert_eq!(
                    (track.ahead, track.behind),
                    (ahead.parse().unwrap(), behind.parse().unwrap()),
                    "{range}"
                );
            }
            // 없는 upstream 은 git 도 셀 수 없다.
            None => assert!(!out.status.success(), "{range} should be gone"),
        }
    }
}

/// 로컬 · 원격 · 태그(가벼운 · 주석) · upstream(원격 · `.` · 사라짐) · ahead/behind 가 git 과 같다. 원격 이름에 `/` 가 들어도
/// 원격을 바르게 고른다. 미신뢰 프로젝트에서도 upstream 을 읽는다(이름일 뿐 프로그램을 고르지 않는다) — 신뢰해도 같다.
#[test]
fn refs_match_for_each_ref_and_rev_list() {
    let r = TestRepo::with_commit();
    let base = r.head();
    // 원격 쪽 이력: base 위에 두 커밋.
    r.git(&["checkout", "--quiet", "-b", "upstream-side"]);
    r.commit_file("remote.txt", "1\n", "remote 1");
    let remote_tip = r.commit_file("remote.txt", "2\n", "remote 2");
    r.checkout("main");
    r.git(&["branch", "--quiet", "-D", "upstream-side"]);
    // 로컬은 base 위에 한 커밋 → ahead 1, behind 2.
    r.commit_file("local.txt", "1\n", "local 1");
    for (key, value) in [
        ("remote.origin.url", "../remote.git"),
        ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
        ("branch.main.remote", "origin"),
        ("branch.main.merge", "refs/heads/main"),
        ("remote.my.url", "../my.git"),
        ("remote.my/fork.url", "../fork.git"),
        (
            "remote.my/fork.fetch",
            "+refs/heads/*:refs/remotes/my/fork/*",
        ),
    ] {
        r.git(&["config", key, value]);
    }
    r.git(&["update-ref", "refs/remotes/origin/main", &remote_tip]);
    r.git(&["update-ref", "refs/remotes/my/fork/feature", &base]);
    r.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    // upstream 이 로컬 브랜치(`.`)인 것과 사라진 것.
    r.git(&["branch", "--quiet", "local-up", &base]);
    r.git(&["config", "branch.local-up.remote", "."]);
    r.git(&["config", "branch.local-up.merge", "refs/heads/main"]);
    r.git(&["branch", "--quiet", "gone", &base]);
    r.git(&["config", "branch.gone.remote", "origin"]);
    r.git(&["config", "branch.gone.merge", "refs/heads/deleted"]);
    r.git(&["tag", "light", &base]);
    r.git(&["tag", "-a", "-m", "annotated", "annotated"]);

    let fx = Fx::new();
    let id = fx.project(r.root());
    assert!(!fx.projects.is_trusted(&id));
    assert_same_refs(&fx, &r, &id);
    let refs = fx.refs(&id, None);
    let main = refs.branches.iter().find(|b| b.name == "main").unwrap();
    assert!(main.head);
    let upstream = main.upstream.as_ref().unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!(upstream.track.map(|t| (t.ahead, t.behind)), Some((1, 2)));
    let gone = refs.branches.iter().find(|b| b.name == "gone").unwrap();
    assert_eq!(gone.upstream.as_ref().unwrap().track, None);
    let local_up = refs.branches.iter().find(|b| b.name == "local-up").unwrap();
    assert_eq!(local_up.upstream.as_ref().unwrap().name, "main");
    let fork = refs
        .remote_branches
        .iter()
        .find(|b| b.name == "my/fork/feature")
        .unwrap();
    // 원격 `my` 도 접두로 맞지만 더 긴 `my/fork` 를 고른다.
    assert_eq!(fork.remote, "my/fork");
    assert!(refs.remote_branches.iter().all(|b| b.name != "origin/HEAD"));
    let annotated = refs.tags.iter().find(|t| t.name == "annotated").unwrap();
    assert_eq!(annotated.peeled.as_deref(), Some(r.head().as_str()));
    assert!(
        refs.tags
            .iter()
            .any(|t| t.name == "light" && t.peeled.is_none())
    );
    assert_eq!(
        refs.head,
        Some(RepoHead::Branch {
            name: "main".into(),
            commit: r.head()
        })
    );

    fx.projects.set_trust(&id, true).unwrap();
    assert_same_refs(&fx, &r, &id);
}

/// 큰 저장소(index 항목이 문턱 이상)의 전체 status 는 작업으로 돈다 — 작업이 시작되고 끝나며(`task.progress` 없이도
/// `task.finished` 가 한 번), 결과는 작업 없이 돈 것과 같다. 경로 한정은 작업이 아니다.
#[test]
fn a_big_full_status_runs_as_a_task() {
    use pw_core::task::{TOPIC_FINISHED, TaskFinishedEvent};
    let r = TestRepo::new();
    for i in 0..crate::status::TASK_THRESHOLD {
        let path = r.root().join(format!("d{}/f{i}.txt", i % 100));
        if i % 100 == i {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        fs::write(path, "x").unwrap();
    }
    r.commit_all("many");
    r.write("d0/f0.txt", "changed");
    let fx = Fx::new();
    let id = fx.project(r.root());
    let without = fx.status(&id, None);
    let bus = fx.bus();
    fx.status.attach(pw_core::TaskSpawner::new(
        bus.clone(),
        Scope::Host,
        CancellationToken::new(),
    ));
    let mut finished = bus.subscribe(pw_core::Filter::all().topics([TOPIC_FINISHED]));
    let with = fx.status(&id, None);
    assert_eq!(with, without);
    assert_eq!(with.entries.len(), 1);
    let event = match finished.try_recv() {
        Some(pw_core::Received::Event(e)) => {
            serde_json::from_value::<TaskFinishedEvent>(e.payload.clone()).unwrap()
        }
        other => panic!("no task.finished: {other:?}"),
    };
    assert_eq!(event.title, crate::status::TASK_TITLE);
    assert_eq!(event.status, pw_core::TaskStatus::Completed);
    let _ = fx.status(&id, Some(&["d0"]));
    assert!(
        finished.try_recv().is_none(),
        "a path-limited status is not a task"
    );
}

/// 작업을 취소하면(`task.cancel`) status 가 cancelled 오류로 끝난다. 계산이 끝나기 전에 취소하려고 작업이 뜨자마자 취소한다.
#[test]
fn cancelling_the_status_task_ends_the_call_as_cancelled() {
    let r = TestRepo::new();
    for i in 0..crate::status::TASK_THRESHOLD {
        let path = r.root().join(format!("d{}/f{i}.txt", i % 100));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "x").unwrap();
    }
    r.commit_all("many");
    let fx = Fx::new();
    let id = fx.project(r.root());
    let bus = fx.bus();
    let spawner = pw_core::TaskSpawner::new(bus.clone(), Scope::Host, CancellationToken::new());
    fx.status.attach(spawner.clone());
    let rt = rt();
    let status = Arc::clone(&fx.status);
    let project_id = id.clone();
    let call = rt.spawn(async move {
        status
            .status(GitStatusParams {
                project_id,
                worktree: None,
                paths: None,
            })
            .await
    });
    // 작업 표에 뜨면 곧바로 취소한다.
    let task = loop {
        if let Some(task) = spawner.running(Some(&id)).first().cloned() {
            break task;
        }
        assert!(
            !call.is_finished(),
            "the status ended before it showed up as a task"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(spawner.cancel(task.task_id));
    let result = rt.block_on(call).unwrap();
    assert_eq!(result.unwrap_err().code, code::CANCELLED);
}
