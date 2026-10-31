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

/// 충돌의 종류(UU · AA · UD · DU)가 git 과 같고, 충돌한 경로는 스테이징 쪽이 비어 있다. 대소문자를 가리는 작업 트리
/// (`core.ignorecase=false` — Linux 의 기본)와 가리지 않는 작업 트리(macOS · Windows 의 기본) 둘 다 — 앞쪽에서는 「우리가
/// 지움」 파일이 untracked 로 섞이면 안 된다(`crate::status::compute::status`).
#[test]
fn conflicts_match_git() {
    for ignorecase in ["false", "true"] {
        let r = TestRepo::with_commit();
        r.git(&["config", "core.ignorecase", ignorecase]);
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
            ],
            "core.ignorecase={ignorecase}"
        );
    }
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
            entries: Vec::new(),
            unsupported: None,
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

/// 깨진 ref 하나는 그것만 `broken` 에 적히고 나머지는 git 과 같다(git 은 `warning: ignoring broken ref` 를 내고 건너뛴다):
/// 내용이 해시가 아닌 loose ref, 없는 객체를 가리키는 태그, upstream 이 없는 객체를 가리켜 셀 수 없는 브랜치.
#[test]
fn a_broken_ref_does_not_hide_the_others() {
    let r = TestRepo::with_commit();
    r.branch("good");
    r.git(&["tag", "fine"]);
    let fx = Fx::new();
    let id = fx.project(r.root());
    let clean = fx.refs(&id, None);
    assert!(clean.broken.is_empty());

    let missing = "0123456789abcdef0123456789abcdef01234567";
    fs::write(r.root().join(".git/refs/heads/garbage"), "not a hash\n").unwrap();
    fs::write(
        r.root().join(".git/refs/tags/dangling"),
        format!("{missing}\n"),
    )
    .unwrap();
    fs::write(
        r.root().join(".git/refs/heads/lost"),
        format!("{missing}\n"),
    )
    .unwrap();
    r.git(&["branch", "--quiet", "counts", "good"]);
    r.git(&["config", "branch.counts.remote", "."]);
    r.git(&["config", "branch.counts.merge", "refs/heads/lost"]);

    let refs = fx.refs(&id, None);
    let mut broken: Vec<Option<String>> = refs.broken.iter().map(|b| b.ref_name.clone()).collect();
    broken.sort();
    assert_eq!(
        broken,
        [
            Some("refs/heads/counts".to_owned()),
            Some("refs/heads/garbage".to_owned()),
            Some("refs/tags/dangling".to_owned()),
        ],
        "{:?}",
        refs.broken
    );
    assert!(refs.broken.iter().all(|b| !b.error.is_empty()));
    let names: Vec<&str> = refs.branches.iter().map(|b| b.name.as_str()).collect();
    // 없는 커밋을 가리키는 브랜치 자체는 git 처럼 보인다(for-each-ref 도 낸다) — 그것을 upstream 으로 둔 브랜치만 셀 수 없다.
    assert_eq!(names, ["good", "lost", "main"]);
    assert_eq!(
        refs.tags
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["fine"]
    );
}

/// `parents` 를 부모로, 커미터 시각 `at`(초)으로 커밋을 만든다(트리는 HEAD 의 것). 그 해시.
fn commit_at(r: &TestRepo, parents: &[&str], at: u64, message: &str) -> String {
    let tree = r.rev_parse("HEAD^{tree}");
    let mut c = r.command_in(r.root());
    let at = format!("@{at} +0000");
    c.env("GIT_AUTHOR_DATE", &at)
        .env("GIT_COMMITTER_DATE", &at)
        .args(["commit-tree", &tree, "-m", message]);
    for parent in parents {
        c.args(["-p", parent]);
    }
    let out = c.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// 무작위 이력(시드 고정): 커밋마다 앞선 커밋 1~3개를 부모로, 시각은 뒤죽박죽(같은 시각 포함)이다. 여러 쌍의 ahead/behind 가
/// `git rev-list --left-right --count` 와 같다 — commit-graph 없이, 그리고 절반만 덮는 commit-graph 와 함께.
#[test]
fn ahead_and_behind_match_rev_list_on_shuffled_dates() {
    let r = TestRepo::with_commit();
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let t = 1_700_200_000;
    let mut commits: Vec<String> = Vec::new();
    let commit = |commits: &mut Vec<String>, next: &mut dyn FnMut(u64) -> u64| {
        let mut parents: Vec<String> = Vec::new();
        if !commits.is_empty() {
            for _ in 0..=next(3) {
                // 가까운 커밋을 자주 고른다 — 갈래가 길어진다.
                let back = (next(6) + 1).min(commits.len() as u64) as usize;
                let parent = commits[commits.len() - back].clone();
                if !parents.contains(&parent) {
                    parents.push(parent);
                }
            }
        }
        let parents: Vec<&str> = parents.iter().map(String::as_str).collect();
        let at = t + next(5) * 1_000;
        let id = commit_at(&r, &parents, at, &format!("c{}", commits.len()));
        commits.push(id);
    };
    for _ in 0..30 {
        commit(&mut commits, &mut next);
    }
    r.git(&["branch", "--quiet", "graphed", commits.last().unwrap()]);
    r.git(&["commit-graph", "write", "--reachable"]);
    for _ in 0..30 {
        commit(&mut commits, &mut next);
    }
    let check = |label: &str| {
        let fx = Fx::new();
        let id = fx.project(r.root());
        let opened = fx.git.open(&id, None).unwrap().unwrap();
        let mut mismatches = Vec::new();
        for i in (0..commits.len()).step_by(4) {
            for j in (1..commits.len()).step_by(7) {
                let (a, b) = (&commits[i], &commits[j]);
                let range = format!("{a}...{b}");
                let text = String::from_utf8(git_read(
                    &r,
                    r.root(),
                    &["rev-list", "--left-right", "--count", &range],
                ))
                .unwrap();
                let (ahead, behind) = text.trim().split_once('\t').unwrap();
                let want: (u32, u32) = (ahead.parse().unwrap(), behind.parse().unwrap());
                let got = count(&opened.repo, a, b);
                if got != want {
                    mismatches.push(format!("c{i}...c{j}: got {got:?}, git {want:?}"));
                }
            }
        }
        assert!(mismatches.is_empty(), "{label}: {mismatches:#?}");
    };
    check("with a partial commit-graph");
    fs::remove_file(r.root().join(".git/objects/info/commit-graph")).unwrap();
    check("without a commit-graph");
}

fn count(repo: &gix::Repository, a: &str, b: &str) -> (u32, u32) {
    let id = |s: &str| gix::ObjectId::from_hex(s.as_bytes()).unwrap();
    let cache = repo.commit_graph_if_enabled().unwrap();
    let mut counter = crate::status::refs::Counter::new(repo, cache.as_ref());
    let t = counter.ahead_behind(id(a), id(b)).unwrap();
    (t.ahead, t.behind)
}

/// ahead/behind 는 커밋 시각과 상관없이 센다 — 조상이 자손보다 늦은 시각(시계가 틀린 기계, 고친 커밋)이어도 rev-list 와 같다.
/// commit-graph 가 없을 때(세대를 직접 센다)와 있을 때(그 세대 번호를 쓴다) 둘 다.
#[test]
fn ahead_and_behind_do_not_trust_commit_dates() {
    let r = TestRepo::with_commit();
    let t = 1_700_100_000;
    // y ← x ← s ← theirs, 그리고 ours = 병합(x, s, z), z ← y. x · z · y 는 미래 시각이라 s 보다 먼저 걷힌다 — 시각 순으로
    // 칠하면 y 가 theirs 쪽이라는 것이 s → x → y 로 닿기 전에 걷기가 끝난다.
    let y = commit_at(&r, &[], t + 7_000, "y");
    let x = commit_at(&r, &[&y], t + 9_000, "x");
    let z = commit_at(&r, &[&y], t + 8_500, "z");
    let s = commit_at(&r, &[&x], t + 2_000, "s");
    let theirs = commit_at(&r, &[&s], t + 2_500, "theirs");
    let ours = commit_at(&r, &[&x, &s, &z], t + 3_000, "ours");
    // 병합 커밋이 부모보다 이른 모양: 양쪽에 미래 시각 커밋이 줄지어 있고 병합이 과거다.
    let base = commit_at(&r, &[], t, "base");
    let mut left = base.clone();
    for i in 0..4 {
        left = commit_at(&r, &[&left], t + 10_000 + i, "left");
    }
    let mut right = base.clone();
    for i in 0..5 {
        right = commit_at(&r, &[&right], t + 20_000 + i, "right");
    }
    let merged = commit_at(&r, &[&left, &right], t + 1, "merge in the past");
    let mut after = merged.clone();
    for i in 0..3 {
        after = commit_at(&r, &[&after], t + 30_000 + i, "after");
    }
    let mut other = right.clone();
    for i in 0..2 {
        other = commit_at(&r, &[&other], t + 2 + i, "other");
    }
    for (name, tip) in [
        ("ours", &ours),
        ("theirs", &theirs),
        ("after", &after),
        ("other", &other),
    ] {
        r.git(&["branch", "--quiet", name, tip]);
    }
    for (branch, upstream) in [("ours", "theirs"), ("after", "other"), ("other", "after")] {
        r.git(&["config", &format!("branch.{branch}.remote"), "."]);
        r.git(&[
            "config",
            &format!("branch.{branch}.merge"),
            &format!("refs/heads/{upstream}"),
        ]);
    }
    let fx = Fx::new();
    let id = fx.project(r.root());
    let track = |branch: &str| {
        fx.refs(&id, None)
            .branches
            .into_iter()
            .find(|b| b.name == branch)
            .and_then(|b| b.upstream?.track)
            .map(|t| (t.ahead, t.behind))
    };
    // 손으로 센 값: ours 에만 ours · z, theirs 에만 theirs. after 에만 after 3 · merged · left 4, other 에만 other 2.
    assert_eq!(track("ours"), Some((2, 1)));
    assert_eq!(track("after"), Some((8, 2)));
    assert_eq!(track("other"), Some((2, 8)));
    assert_same_refs(&fx, &r, &id);
    r.git(&["commit-graph", "write", "--reachable"]);
    assert!(r.root().join(".git/objects/info/commit-graph").exists());
    assert_same_refs(&fx, &r, &id);
    assert_eq!(track("after"), Some((8, 2)));
    // commit-graph 뒤의 커밋(세대가 없는 것)이 섞여도 같다.
    let newer = commit_at(&r, &[&ours], t - 5_000, "older than everything");
    r.git(&["branch", "--quiet", "--force", "ours", &newer]);
    assert_eq!(track("ours"), Some((3, 1)));
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

/// reftable 저장소(`git init --ref-format=reftable`)는 gix 가 ref 를 읽지 못한다 — 내부 오류 대신 「지원 안 함」 이다. 그 형식을 모르는
/// git(2.45 전)이면 건너뛴다.
#[test]
fn a_reftable_repository_is_unsupported() {
    let r = TestRepo::new();
    let dir = r.base().join("reftable");
    let made = r.try_git_in(
        r.base(),
        &[
            "init",
            "--quiet",
            "--ref-format=reftable",
            dir.to_str().unwrap(),
        ],
    );
    if !made.status.success() {
        eprintln!("skipped: this git cannot make a reftable repository");
        return;
    }
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    r.git_in(&dir, &["add", "a.txt"]);
    r.git_in(&dir, &["commit", "--quiet", "-m", "x"]);
    let fx = Fx::new();
    let id = fx.project(&dir);
    let status = fx.try_status(&id, None, None).unwrap();
    assert_eq!(status.work_tree.as_deref(), Some(dir.to_str().unwrap()));
    assert_eq!(
        status.unsupported,
        Some(crate::status::GitUnsupported::Reftable)
    );
    assert!(status.entries.is_empty());
    let refs = fx.refs(&id, None);
    assert_eq!(
        refs.unsupported,
        Some(crate::status::GitUnsupported::Reftable)
    );
    assert!(refs.branches.is_empty());
}

/// sparse index(`git sparse-checkout set --cone --sparse-index`)도 읽는다 — index 에 폴더 하나로 접힌 항목이 있어도 git 과 같다.
/// 접힌 폴더 안의 스테이징한 변경(`reset --soft` 로 HEAD 만 옮긴 것)과 펼쳐진 쪽의 스테이징 · 수정 · untracked 를 함께 본다.
#[test]
fn a_sparse_index_matches_git() {
    let r = TestRepo::new();
    r.write("kept/a.txt", "a\n");
    r.write("folded/b.txt", "b\n");
    r.write("folded/deep/c.txt", "c\n");
    r.commit_all("tree");
    r.write("folded/b.txt", "b2\n");
    r.commit_all("change inside the folded directory");
    r.git(&["sparse-checkout", "set", "--cone", "--sparse-index", "kept"]);
    let index = fs::read(r.root().join(".git/index")).unwrap();
    assert!(
        index.windows(4).any(|w| w == b"sdir"),
        "the index is not sparse"
    );
    let fx = Fx::new();
    let id = fx.project(r.root());
    assert_same_status(&fx, &r, &id, None);
    assert!(fx.status(&id, None).entries.is_empty());

    r.write("kept/new.txt", "new\n");
    r.git(&["add", "kept/new.txt"]);
    r.write("kept/a.txt", "a2\n");
    r.write("kept/untracked.txt", "u\n");
    assert_same_status(&fx, &r, &id, None);
    assert_same_status(&fx, &r, &id, Some(&["kept/a.txt"]));
    // HEAD 만 앞 커밋으로 — 접힌 폴더 안의 `folded/b.txt` 가 스테이징한 수정이 된다.
    r.git(&["reset", "--quiet", "--soft", "HEAD~"]);
    let index = fs::read(r.root().join(".git/index")).unwrap();
    assert!(index.windows(4).any(|w| w == b"sdir"));
    assert_same_status(&fx, &r, &id, None);
    let got = fx.status(&id, None);
    assert!(
        got.entries
            .iter()
            .any(|e| e.path == "folded/b.txt" && e.staged == Some(GitChange::Modified))
    );
    fx.projects.set_trust(&id, true).unwrap();
    assert_same_status(&fx, &r, &id, None);
}

/// 이름 바뀜 찾기는 저장소 설정을 따른다(git 과 같이 `status.renames`, 없으면 `diff.renames`): 끄면 스테이징한 이름 바뀜이 지움 ·
/// 더함 두 줄이다. 설정만 바꾸고 HEAD · index 가 그대로여도 앞 결과(캐시)를 쓰지 않는다.
#[test]
fn rename_detection_follows_the_repository_config() {
    let r = TestRepo::new();
    r.write("old.txt", "the same content on both sides\n");
    r.commit_all("one");
    r.git(&["mv", "old.txt", "new.txt"]);
    let fx = Fx::new();
    let id = fx.project(r.root());
    let renamed = |fx: &Fx| {
        fx.status(&id, None)
            .entries
            .iter()
            .any(|e| e.staged == Some(GitChange::Renamed))
    };
    assert_same_status(&fx, &r, &id, None);
    assert!(renamed(&fx));
    for (key, value, want) in [
        ("status.renames", "false", false),
        ("status.renames", "true", true),
        ("diff.renames", "false", true),
    ] {
        r.git(&["config", key, value]);
        assert_same_status(&fx, &r, &id, None);
        assert_eq!(renamed(&fx), want, "{key}={value}");
    }
    r.git(&["config", "--unset", "status.renames"]);
    assert_same_status(&fx, &r, &id, None);
    assert!(!renamed(&fx), "diff.renames=false without status.renames");
    // 경로 한정도 같은 설정을 따른다.
    assert_same_status(&fx, &r, &id, Some(&["new.txt"]));
    fx.projects.set_trust(&id, true).unwrap();
    assert_same_status(&fx, &r, &id, None);
}

/// `.git` 폴더 안의 `commondir` 로 다른 저장소의 git 디렉터리를 빌려 오는 폴더(받은 압축 등)는 저장소가 아니다 — git 은 그쪽을 공유
/// git 디렉터리로 읽지만(`rev-parse --git-common-dir`), 연결 워크트리의 git 디렉터리는 언제나 공유 git 디렉터리의
/// `worktrees/<이름>` 바로 아래다. 받으면 그쪽 ref 를 읽고 그 폴더를 감시하게 된다. 진짜 연결 워크트리는 그대로 받는다.
#[test]
fn a_dot_git_folder_borrowing_another_common_dir_is_no_repository() {
    let victim = TestRepo::with_commit();
    victim.branch("secret-branch");
    let r = TestRepo::new();
    let evil = r.root();
    fs::write(
        evil.join(".git/commondir"),
        format!("{}\n", victim.root().join(".git").display()),
    )
    .unwrap();
    let common = git_read(&r, evil, &["rev-parse", "--git-common-dir"]);
    assert_eq!(
        Path::new(String::from_utf8(common).unwrap().trim()),
        victim.root().join(".git"),
        "git reads the borrowed common dir"
    );
    let fx = Fx::new();
    let id = fx.project(evil);
    assert!(fx.git.open(&id, None).unwrap().is_none());
    let refs = fx.refs(&id, None);
    assert_eq!(refs.work_tree, None);
    assert!(refs.branches.is_empty());
    assert_eq!(fx.status(&id, None).work_tree, None);
    // 상대 경로 `commondir` 도 같다(두 임시 폴더는 같은 폴더 안에 있다).
    let sibling = victim.base().file_name().unwrap().to_str().unwrap();
    fs::write(
        evil.join(".git/commondir"),
        format!("../../../{sibling}/repo/.git\n"),
    )
    .unwrap();
    assert!(fx.git.open(&id, None).unwrap().is_none());

    let wt = victim.add_worktree("wt", "feature");
    let wt_id = fx.project(&wt);
    let opened = fx
        .git
        .open(&wt_id, None)
        .unwrap()
        .expect("a linked worktree");
    assert_ne!(opened.repo.git_dir(), opened.repo.common_dir());
}

/// 센 세대 번호는 다음 `git.refs` 에 넘긴다(`Generations`) — 객체 바꿔치기(`git replace --graft`)로 부모가 바뀌면 넘긴 것이 틀리므로,
/// 바꿔치기가 있는 저장소는 넘기지 않고 매번 센다. 바꾼 뒤에도 rev-list 와 같다.
#[test]
fn replaced_parents_do_not_reuse_kept_generations() {
    let r = TestRepo::with_commit();
    let t = 1_700_300_000;
    let a = commit_at(&r, &[], t, "a");
    let b = commit_at(&r, &[&a], t + 1, "b");
    let c = commit_at(&r, &[&b], t + 2, "c");
    let x = commit_at(&r, &[], t + 3, "x");
    let y = commit_at(&r, &[&x], t + 4, "y");
    r.git(&["branch", "--quiet", "side", &c]);
    r.git(&["branch", "--quiet", "up", &y]);
    r.git(&["config", "branch.side.remote", "."]);
    r.git(&["config", "branch.side.merge", "refs/heads/up"]);
    let fx = Fx::new();
    let id = fx.project(r.root());
    assert_same_refs(&fx, &r, &id);
    // a 의 부모를 y 로: 넘긴 세대(a 1, y 2)로는 자손이 조상보다 작아진다.
    r.git(&["replace", "--graft", &a, &y]);
    assert_same_refs(&fx, &r, &id);
    let track = fx
        .refs(&id, None)
        .branches
        .into_iter()
        .find(|b| b.name == "side")
        .and_then(|b| b.upstream?.track)
        .map(|t| (t.ahead, t.behind));
    assert_eq!(track, Some((3, 0)));
}

/// `core.safecrlf=true` 는 git 에서 쓰기(`git add`)의 검사다 — `git status` 는 그것으로 실패하지 않는다. `autocrlf=true` 에 LF 작업
/// 트리 파일이 있어도 상태는 git 과 같다(`crate::diff::changes::without_safecrlf`).
#[test]
fn safecrlf_does_not_fail_the_status_like_git() {
    let r = TestRepo::with_commit();
    r.write("lf.txt", "a\nb\n");
    r.commit_all("lf");
    r.write("lf.txt", "a\nB\n");
    r.git(&["config", "core.autocrlf", "true"]);
    r.git(&["config", "core.safecrlf", "true"]);
    let fx = Fx::new();
    let id = fx.project(r.root());
    assert_same_status(&fx, &r, &id, None);
    let got = fx.status(&id, None);
    let lf = got.entries.iter().find(|e| e.path == "lf.txt").unwrap();
    assert_eq!(lf.unstaged, Some(GitChange::Modified));
}
