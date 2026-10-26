//! 임시 폴더의 시험 저장소: 만들기 · 파일 쓰기 · 커밋 · 브랜치 · 연결 워크트리.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

/// 첫 커밋의 시각(초). 커밋마다 1분씩 늘린다 — 해시와 시각 순서가 실행마다 같다.
const EPOCH: u64 = 1_700_000_000;

/// 사람의 git 설정 · 저장소와 떨어진 git 명령. 시스템 · 전역 설정을 읽지 않고(`GIT_CONFIG_NOSYSTEM`, 빈 `GIT_CONFIG_GLOBAL`),
/// 위로 찾기를 `ceiling` 에서 멈춘다(임시 폴더가 어느 저장소 안이어도 그 저장소를 건드리지 않는다). 실행기가 자식 환경에서 빼는
/// 변수(`crate::cli::is_dropped` — `GIT_DIR` 처럼 시험이 git 훅 안에서 돌 때 남아 있는 것, 부른 쪽이 `-c` 로 넣은 설정)는 지운다.
pub fn git_command(dir: &Path, ceiling: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir);
    for (name, _) in std::env::vars_os() {
        if crate::cli::is_dropped(&name) {
            c.env_remove(name);
        }
    }
    let empty = ceiling.join(".gitconfig-empty");
    if !empty.exists() {
        std::fs::write(&empty, "").expect("write the empty git config");
    }
    c.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &empty)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", ":")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_AUTHOR_NAME", "Pitwall Test")
        .env("GIT_AUTHOR_EMAIL", "test@pitwall.invalid")
        .env("GIT_COMMITTER_NAME", "Pitwall Test")
        .env("GIT_COMMITTER_EMAIL", "test@pitwall.invalid")
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            "advice.detachedHead=false",
        ]);
    c
}

/// 임시 폴더 안의 저장소. 버리면 폴더째 지운다.
pub struct TestRepo {
    dir: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    clock: AtomicU64,
}

impl Default for TestRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl TestRepo {
    /// `git init`(기본 브랜치 `main`) 한 빈 저장소. 커밋이 없으니 HEAD 는 태어나지 않은 `main` 이다.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create a temp dir");
        // 레지스트리 · 저장소 해석이 돌려주는 경로(실제 경로)와 비교할 수 있게 한다(macOS 의 `/var` → `/private/var`).
        let base = pw_core::registry::path::normalize(&std::fs::canonicalize(dir.path()).unwrap())
            .unwrap();
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let repo = Self {
            dir,
            base,
            root,
            clock: AtomicU64::new(EPOCH),
        };
        repo.git(&["init", "--quiet"]);
        repo
    }

    /// 첫 커밋(`README.md`)이 있는 저장소.
    pub fn with_commit() -> Self {
        let repo = Self::new();
        repo.commit_file("README.md", "hello\n", "initial");
        repo
    }

    /// 저장소 바깥의 임시 폴더(연결 워크트리 · 저장소 밖 폴더를 여기에 만든다). 시험이 끝나면 함께 지워진다.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// 저장소의 작업 트리 꼭대기(실제 경로).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 임시 폴더 그 자체(지울 때 쓰는 핸들).
    pub fn temp_dir(&self) -> &tempfile::TempDir {
        &self.dir
    }

    /// `dir` 에서 도는 격리된 git 명령([`git_command`]). 커밋 시각은 커밋마다 늘어난다.
    pub fn command_in(&self, dir: &Path) -> Command {
        let mut c = git_command(dir, &self.base);
        let at = format!("@{} +0000", self.clock.fetch_add(60, Ordering::SeqCst));
        c.env("GIT_AUTHOR_DATE", &at).env("GIT_COMMITTER_DATE", &at);
        c
    }

    /// `dir` 에서 git 을 돌린다. 실패해도 패닉하지 않는다(충돌을 만드는 명령 등).
    pub fn try_git_in(&self, dir: &Path, args: &[&str]) -> Output {
        self.command_in(dir)
            .args(args)
            .output()
            .expect("run git (is it installed?)")
    }

    /// `dir` 에서 git 을 돌려 stdout 을 돌려준다. 실패하면 stderr 와 함께 패닉한다.
    pub fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.try_git_in(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} in {} failed ({}): {}",
            dir.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf-8 git output")
    }

    /// 저장소 꼭대기에서 [`Self::git_in`].
    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.root, args)
    }

    /// 작업 트리의 `rel` 에 쓴다(폴더를 만든다).
    pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// 바뀐 것을 모두 담아 커밋하고 HEAD 의 해시를 돌려준다.
    pub fn commit_all(&self, message: &str) -> String {
        self.commit_all_in(&self.root, message)
    }

    /// `dir`(연결 워크트리 등)에서 [`Self::commit_all`].
    pub fn commit_all_in(&self, dir: &Path, message: &str) -> String {
        self.git_in(dir, &["add", "--all"]);
        self.git_in(dir, &["commit", "--quiet", "--allow-empty", "-m", message]);
        self.rev_parse_in(dir, "HEAD")
    }

    /// 파일 하나를 쓰고 커밋한다.
    pub fn commit_file(&self, rel: &str, contents: &str, message: &str) -> String {
        self.write(rel, contents);
        self.commit_all(message)
    }

    pub fn rev_parse(&self, rev: &str) -> String {
        self.rev_parse_in(&self.root, rev)
    }

    pub fn rev_parse_in(&self, dir: &Path, rev: &str) -> String {
        self.git_in(dir, &["rev-parse", "--verify", rev])
            .trim()
            .to_owned()
    }

    pub fn head(&self) -> String {
        self.rev_parse("HEAD")
    }

    /// 지금 HEAD 에서 브랜치를 만든다(옮기지 않는다).
    pub fn branch(&self, name: &str) {
        self.git(&["branch", name]);
    }

    pub fn checkout(&self, rev: &str) {
        self.git(&["checkout", "--quiet", rev]);
    }

    /// HEAD 를 지금 커밋에서 분리한다.
    pub fn detach(&self) {
        self.git(&["checkout", "--quiet", "--detach"]);
    }

    /// [`Self::base`] 아래 `name` 에 새 브랜치 `branch` 의 연결 워크트리를 만든다(`git worktree add -b`). 그 꼭대기를 돌려준다.
    pub fn add_worktree(&self, name: &str, branch: &str) -> PathBuf {
        let path = self.base.join(name);
        self.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch,
            path.to_str().expect("utf-8 temp path"),
        ]);
        path
    }
}
