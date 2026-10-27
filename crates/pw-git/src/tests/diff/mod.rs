//! M2-04: `git.diff` · `git.show` · `git.containedIn` · `git.blob`. 결과를 같은 저장소에서 돌린 진짜 git 의 출력과 견준다 — 파일
//! 목록은 `git diff --raw -z --find-renames`, hunk 는 `git diff --find-renames -U3` 의 patch 를 줄 단위로.

mod blob;
mod compare;
mod flags;
mod limits;
mod release;
mod show;
#[cfg(unix)]
mod untrusted;
mod view_fixture;
#[cfg(unix)]
mod worktree;

use super::*;
use crate::diff::methods::{
    DiffFile, DiffHunk, DiffSide, FileStatus, GitBlob, GitBlobParams, GitDiff, GitDiffParams,
    GitRevParams, LineKind, RefKind,
};
use crate::testing::TestRepo;

/// 빈 트리(루트 커밋의 부모 자리).
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// 시험 저장소 하나와 그것을 등록한 Host.
struct Repo {
    r: TestRepo,
    f: Fixture,
    id: String,
}

impl Repo {
    fn new(r: TestRepo) -> Self {
        isolate_global_config(r.base());
        let f = Fixture::new("", None);
        let id = f.project(r.root());
        Self { r, f, id }
    }

    fn params(&self, from: Option<DiffSide>, to: DiffSide) -> GitDiffParams {
        GitDiffParams {
            project_id: self.id.clone(),
            worktree: None,
            from,
            to,
            paths: None,
            ignore_whitespace: false,
        }
    }

    fn diff(&self, from: Option<DiffSide>, to: DiffSide) -> GitDiff {
        self.f.git.diff(&self.params(from, to)).unwrap()
    }

    fn rev(&self, rev: &str) -> GitRevParams {
        GitRevParams {
            project_id: self.id.clone(),
            worktree: None,
            rev: rev.to_owned(),
        }
    }

    fn blob_params(&self, rev: Option<&str>, path: &str) -> GitBlobParams {
        GitBlobParams {
            project_id: self.id.clone(),
            rev: rev.map(str::to_owned),
            path: path.to_owned(),
            ..GitBlobParams::default()
        }
    }

    fn blob(&self, rev: Option<&str>, path: &str) -> GitBlob {
        self.f.git.blob(&self.blob_params(rev, path)).unwrap()
    }
}

/// gix 가 사람의 전역 · 시스템 git 설정(`~/.gitconfig` 의 `diff.algorithm` · `filter.lfs.*`, `~/.config/git/attributes`)을 읽지
/// 않게 HOME · XDG · `GIT_CONFIG_GLOBAL` 을 `<base>/home` 으로 돌리고 시스템 설정을 끈다. 견줄 git 은 이미 빈 전역 설정으로 돈다
/// (`testing::repo::git_command`). 전역 설정 파일(`<base>/home/.gitconfig`)을 돌려준다 — 시험이 거기 써서 사람의 설정을 흉내 낸다.
fn isolate_global_config(base: &Path) -> std::path::PathBuf {
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let config = home.join(".gitconfig");
    if !config.exists() {
        fs::write(&config, "").unwrap();
    }
    // SAFETY: nextest 는 시험마다 프로세스가 따로이고, 시험은 저장소를 열기 전(다른 스레드를 띄우기 전)에 이것을 부른다.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("GIT_CONFIG_GLOBAL", &config);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    config
}

fn commit(rev: &str) -> DiffSide {
    DiffSide::Commit {
        rev: rev.to_owned(),
    }
}

/// 견줄 꼴: 파일 하나의 상태 · 경로 · 모드 · 해시 · 바이너리 · hunk(patch 의 글 그대로).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Patch {
    status: FileStatus,
    path: String,
    old_path: Option<String>,
    old_mode: Option<String>,
    new_mode: Option<String>,
    old_id: Option<String>,
    new_id: Option<String>,
    binary: bool,
    hunks: Vec<String>,
}

/// 진짜 git 의 diff. `args` 는 `git diff` 뒤에 붙는 것(리비전 · `--cached` · `-w`).
fn git_patches(r: &TestRepo, args: &[&str]) -> Vec<Patch> {
    git_patches_in(r, r.root(), args)
}

/// `dir`(클론 등 다른 저장소)에서 [`git_patches`].
fn git_patches_in(r: &TestRepo, dir: &Path, args: &[&str]) -> Vec<Patch> {
    let base = [
        "-c",
        "core.quotePath=false",
        "diff",
        "--find-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-abbrev",
    ];
    let raw = r.git_in(dir, &[&base[..], &["--raw", "-z"], args].concat());
    let mut files = Vec::new();
    let mut fields = raw.split('\0').filter(|s| !s.is_empty());
    while let Some(meta) = fields.next() {
        let meta: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let (old_mode, new_mode, old_id, new_id, status) =
            (meta[0], meta[1], meta[2], meta[3], meta[4]);
        let first = fields.next().unwrap().to_owned();
        let (status, path, old_path) = match &status[..1] {
            "A" => (FileStatus::Added, first, None),
            "D" => (FileStatus::Deleted, first, None),
            "M" => (FileStatus::Modified, first, None),
            "T" => (FileStatus::TypeChanged, first, None),
            "R" => (
                FileStatus::Renamed,
                fields.next().unwrap().to_owned(),
                Some(first),
            ),
            other => panic!("unexpected raw status {other}"),
        };
        let mode = |m: &str| (m != "000000").then(|| m.to_owned());
        let id = |i: &str| (!i.chars().all(|c| c == '0')).then(|| i.to_owned());
        files.push(Patch {
            status,
            path,
            old_path,
            old_mode: mode(old_mode),
            new_mode: mode(new_mode),
            old_id: id(old_id),
            new_id: id(new_id),
            binary: false,
            hunks: Vec::new(),
        });
    }
    let patch = r.git_in(dir, &[&base[..], &["-U3"], args].concat());
    let mut current: Option<String> = None;
    let mut in_header = false;
    for line in patch.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let b = rest.trim_end_matches('\n').rsplit_once(" b/").unwrap().1;
            current = Some(b.to_owned());
            in_header = true;
            continue;
        }
        let file = files
            .iter_mut()
            .find(|f| Some(&f.path) == current.as_ref())
            .unwrap_or_else(|| panic!("patch for {current:?} is not in the raw list"));
        if line.starts_with("@@") {
            in_header = false;
            file.hunks.push(line.to_owned());
        } else if in_header {
            if line.starts_with("Binary files ") {
                file.binary = true;
            }
        } else {
            file.hunks.last_mut().unwrap().push_str(line);
        }
    }
    files
}

/// 우리 결과를 [`Patch`] 꼴로. 서브모듈 포인터는 git 이 보이는 `Subproject commit` hunk 로 옮긴다.
fn our_patches(diff: &GitDiff) -> Vec<Patch> {
    diff.files.iter().map(patch_of).collect()
}

fn patch_of(f: &DiffFile) -> Patch {
    let mut hunks: Vec<String> = f.hunks.iter().map(hunk_text).collect();
    if let Some(sub) = &f.submodule {
        let (old, new) = (sub.old.as_ref(), sub.new.as_ref());
        let mut h = format!(
            "@@ -{} +{} @@\n",
            if old.is_some() { "1" } else { "0,0" },
            if new.is_some() { "1" } else { "0,0" }
        );
        if let Some(old) = old {
            h.push_str(&format!("-Subproject commit {old}\n"));
        }
        if let Some(new) = new {
            h.push_str(&format!("+Subproject commit {new}\n"));
        }
        hunks.push(h);
    }
    Patch {
        status: f.status,
        path: f.path.clone(),
        old_path: f.old_path.clone(),
        old_mode: f.old_mode.clone(),
        new_mode: f.new_mode.clone(),
        old_id: f.old_id.clone(),
        new_id: f.new_id.clone(),
        binary: f.binary,
        hunks,
    }
}

fn hunk_text(h: &DiffHunk) -> String {
    let range = |start: u32, lines: u32| {
        if lines == 1 {
            start.to_string()
        } else {
            format!("{start},{lines}")
        }
    };
    let mut s = format!(
        "@@ -{} +{} @@",
        range(h.old_start, h.old_lines),
        range(h.new_start, h.new_lines)
    );
    if !h.header.is_empty() {
        s.push(' ');
        s.push_str(&h.header);
    }
    s.push('\n');
    for l in &h.lines {
        s.push(match l.kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
        });
        s.push_str(&l.text);
        s.push('\n');
        if l.no_newline {
            s.push_str("\\ No newline at end of file\n");
        }
    }
    s
}

/// 우리 diff 와 git 의 것이 같다.
#[track_caller]
fn assert_same_as_git(repo: &Repo, from: Option<DiffSide>, to: DiffSide, git_args: &[&str]) {
    let ours = our_patches(&repo.diff(from, to));
    let theirs = git_patches(&repo.r, git_args);
    assert!(
        !theirs.is_empty(),
        "the fixture changes nothing: {git_args:?}"
    );
    assert_patches_eq(&ours, &theirs, git_args);
}

/// 갈린 파일만 보인다(전체를 찍으면 무작위 픽스처에서 읽을 수 없다).
#[track_caller]
fn assert_patches_eq(ours: &[Patch], theirs: &[Patch], git_args: &[&str]) {
    let ours_paths: Vec<_> = ours.iter().map(|p| &p.path).collect();
    let theirs_paths: Vec<_> = theirs.iter().map(|p| &p.path).collect();
    assert_eq!(
        ours_paths, theirs_paths,
        "file lists differ: git diff {git_args:?}"
    );
    let differ: Vec<String> = ours
        .iter()
        .zip(theirs)
        .filter(|(a, b)| a != b)
        .map(|(a, b)| format!("--- ours\n{a:#?}\n--- git\n{b:#?}"))
        .collect();
    assert!(
        differ.is_empty(),
        "{} of {} files differ from git diff {git_args:?}:\n{}",
        differ.len(),
        ours.len(),
        differ
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
