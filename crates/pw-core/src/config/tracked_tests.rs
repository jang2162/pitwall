//! 추적 판정 시험. 저장소는 실제 `git` 으로 만든다(인덱스 꼴 · 워크트리 · 서브모듈 · bare 를 손으로 흉내 내지 않는다).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;

/// 사람의 git 설정과 떨어진 git 명령(`pw_git::testing::git_command` 와 같은 뜻 — pw-git 이 이 크레이트에 의존해 그것을 쓸 수
/// 없다). 시험이 git 훅 안에서 돌 때 남는 `GIT_DIR` · `GIT_INDEX_FILE` 같은 변수는 지운다.
pub(crate) fn git(dir: &Path, args: &[&str]) {
    let empty = std::env::temp_dir().join("pitwall-test-gitconfig-empty");
    if !empty.exists() {
        fs::write(&empty, "").unwrap();
    }
    let mut c = Command::new("git");
    c.current_dir(dir);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            c.env_remove(name);
        }
    }
    let out = c
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &empty)
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
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

pub(crate) fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// `root` 에 저장소를 만들고 `.pitwall/project.toml` 을 커밋한다(인덱스가 생긴다).
pub(crate) fn repo(root: &Path) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q"]);
    put(&root.join(".pitwall/project.toml"), "schema = 1\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "init"]);
    root.to_path_buf()
}

const LOCAL: &str = ".pitwall/project.local.toml";

fn judge(checkout: &Path) -> Tracking {
    tracking(checkout, &checkout.join(LOCAL))
}

#[test]
fn outside_a_repository_and_in_a_fresh_one_nothing_is_tracked() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    put(&plain.join(LOCAL), "schema = 1\n");
    assert_eq!(judge(&plain), Tracking::Untracked);

    // 갓 `git init` — 인덱스가 아직 없다.
    let fresh = tmp.path().join("fresh");
    put(&fresh.join(LOCAL), "schema = 1\n");
    git(&fresh, &["init", "-q"]);
    assert!(!fresh.join(".git/index").exists());
    assert_eq!(judge(&fresh), Tracking::Untracked);

    // 다른 파일만 추적하면 개인 파일은 사람의 것이다. 강제로 더하면(커밋 전이어도 인덱스에 있으면) 추적이다.
    let root = repo(&tmp.path().join("repo"));
    put(&root.join(LOCAL), "schema = 1\n");
    assert_eq!(judge(&root), Tracking::Untracked);
    git(&root, &["add", "-f", LOCAL]);
    assert_eq!(judge(&root), Tracking::Tracked);
    git(&root, &["commit", "-q", "-m", "commit the local file"]);
    assert_eq!(judge(&root), Tracking::Tracked);
    // 아직 없는 하위 폴더 파일(쓰기 전 판정)도 된다.
    assert_eq!(
        tracking(&root, &root.join(".pitwall/db/pg.local.toml")),
        Tracking::Untracked
    );
    // 추적을 풀면 사람의 것이다.
    git(&root, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&root), Tracking::Untracked);
}

/// 프로젝트 루트가 저장소의 하위 폴더여도 저장소 기준 경로로 본다. 대소문자만 다른 항목도 추적으로 본다(대소문자를 가리지 않는 파일
/// 시스템에서 같은 파일이다 — 틀려도 받지 않는 쪽이다).
#[test]
fn a_project_in_a_subfolder_and_case_differences_are_judged_by_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"));
    let sub = root.join("apps/web");
    put(&sub.join(LOCAL), "schema = 1\n");
    assert_eq!(judge(&sub), Tracking::Untracked);
    git(
        &root,
        &["add", "-f", "apps/web/.pitwall/project.local.toml"],
    );
    assert_eq!(judge(&sub), Tracking::Tracked);

    put(&root.join(".pitwall/DB/PG.local.toml"), "schema = 1\n");
    git(&root, &["add", "-f", ".pitwall/DB/PG.local.toml"]);
    assert_eq!(
        tracking(&root, &root.join(".pitwall/db/pg.local.toml")),
        Tracking::Tracked
    );
}

/// 커밋된 링크 `.pitwall`(조상이 인덱스 항목)과, 추적하지 않는 링크를 지나 닿는 추적 파일은 추적으로 본다.
#[cfg(unix)]
#[test]
fn links_inside_the_repository_do_not_hide_a_tracked_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    fs::create_dir_all(root.join("conf")).unwrap();
    std::os::unix::fs::symlink("conf", root.join(".pitwall")).unwrap();
    put(&root.join("conf/project.local.toml"), "schema = 1\n");
    put(&root.join("conf/.gitignore"), "*.local.toml\n");
    git(&root, &["add", ".pitwall", "conf/.gitignore"]);
    // `conf/project.local.toml` 은 무시되어 추적하지 않지만, 그것에 닿는 `.pitwall` 이 커밋된 링크다.
    assert_eq!(judge(&root), Tracking::Tracked);

    let other = tmp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    put(&other.join("conf/project.local.toml"), "schema = 1\n");
    git(&other, &["add", "conf/project.local.toml"]);
    std::os::unix::fs::symlink("conf", other.join(".pitwall")).unwrap();
    // `.pitwall` 링크는 추적하지 않지만 닿는 파일이 추적 파일이다.
    assert_eq!(judge(&other), Tracking::Tracked);
}

/// 연결 워크트리(`.git` 파일 → `<main>/.git/worktrees/<이름>`)는 자기 인덱스로 판정한다.
#[test]
fn a_linked_worktree_is_judged_by_its_own_index() {
    let tmp = tempfile::tempdir().unwrap();
    let main = repo(&tmp.path().join("main"));
    put(&main.join(LOCAL), "schema = 1\n");
    git(&main, &["add", "-f", LOCAL]);
    git(&main, &["commit", "-q", "-m", "local"]);
    let wt = tmp.path().join("wt");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    assert!(wt.join(".git").is_file());
    assert_eq!(judge(&wt), Tracking::Tracked);
    // 메인에서만 추적을 풀면 워크트리의 인덱스에는 아직 있다.
    git(&main, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&main), Tracking::Untracked);
    assert_eq!(judge(&wt), Tracking::Tracked);
    git(&wt, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&wt), Tracking::Untracked);
}

/// 서브모듈(`.git` 파일 → `<상위>/.git/modules/<이름>`)과 bare 저장소의 워크트리.
#[test]
fn submodules_and_worktrees_of_a_bare_repository_are_judged() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = repo(&tmp.path().join("upstream"));
    put(&upstream.join(LOCAL), "schema = 1\n");
    git(&upstream, &["add", "-f", LOCAL]);
    git(&upstream, &["commit", "-q", "-m", "local"]);

    let outer = repo(&tmp.path().join("outer"));
    git(
        &outer,
        &["submodule", "add", "-q", upstream.to_str().unwrap(), "sub"],
    );
    let sub = outer.join("sub");
    assert!(sub.join(".git").is_file());
    assert_eq!(judge(&sub), Tracking::Tracked);
    git(&sub, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&sub), Tracking::Untracked);

    let bare = tmp.path().join("bare.git");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            upstream.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let wt = tmp.path().join("bare-wt");
    git(&bare, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    assert_eq!(judge(&wt), Tracking::Tracked);
    git(&wt, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&wt), Tracking::Untracked);
}

/// split index(`core.splitIndex` — 공유 인덱스 `sharedindex.<해시>` 와 나눠 적는다)도 푼다.
#[test]
fn a_split_index_is_read_whole() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"));
    put(&root.join(LOCAL), "schema = 1\n");
    git(&root, &["config", "core.splitIndex", "true"]);
    git(&root, &["update-index", "--split-index"]);
    git(&root, &["add", "-f", LOCAL]);
    let shared = fs::read_dir(root.join(".git"))
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().starts_with("sharedindex."));
    assert!(shared, "git did not split the index");
    assert_eq!(judge(&root), Tracking::Tracked);
    // 공유 인덱스에만 있는 항목(나눈 뒤에 바뀌지 않은 것)도 본다.
    git(&root, &["update-index", "--split-index"]);
    assert_eq!(
        tracking(&root, &root.join(".pitwall/project.toml")),
        Tracking::Tracked
    );
}

/// 판정을 못 하면 「모름」 — 부른 쪽은 사람의 레이어로 받지 않는다.
#[test]
fn an_unreadable_repository_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let unknown = |checkout: &Path| matches!(judge(checkout), Tracking::Unknown(_));

    // 깨진 인덱스.
    let broken = repo(&tmp.path().join("broken"));
    put(&broken.join(LOCAL), "schema = 1\n");
    fs::write(broken.join(".git/index"), b"DIRC garbage").unwrap();
    assert!(unknown(&broken), "{:?}", judge(&broken));

    // 아무 데도 가리키지 않는 `.git` 파일.
    let dangling = tmp.path().join("dangling");
    put(&dangling.join(LOCAL), "schema = 1\n");
    fs::write(dangling.join(".git"), "gitdir: /nowhere/at/all\n").unwrap();
    assert!(unknown(&dangling));

    // SHA-256 저장소(이 앱의 gitoxide 는 SHA-1 만 읽는다).
    let sha256 = repo(&tmp.path().join("sha256"));
    put(&sha256.join(LOCAL), "schema = 1\n");
    let config = sha256.join(".git/config");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        format!("{text}[extensions]\n\tobjectformat = sha256\n"),
    )
    .unwrap();
    assert!(unknown(&sha256));

    // `core.worktree` 가 다른 폴더를 가리키는 `.git` 폴더(인덱스의 경로가 다른 작업 트리 기준이다).
    let moved = repo(&tmp.path().join("moved"));
    put(&moved.join(LOCAL), "schema = 1\n");
    git(&moved, &["config", "core.worktree", "/somewhere/else"]);
    assert!(unknown(&moved));
}

/// 캐시는 인덱스 내용의 체크섬으로 맞춘다 — 크기 · 수정 시각 · inode 가 같은 제자리 덮어쓰기(같은 초 안 두 번 쓰기)도 놓치지 않는다.
/// 판정이 낡은 「추적 안 함」 에 머물면 커밋된 파일이 사람의 설정으로 넓어진다.
#[test]
fn the_cache_follows_the_index_content_not_its_size_time_or_inode() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"));
    put(&root.join(LOCAL), "schema = 1\n");
    // 길이가 같은 다른 이름을 추적한 인덱스와, 개인 파일을 추적한 인덱스.
    put(&root.join(".pitwall/project.locax.toml"), "schema = 1\n");
    let index = root.join(".git/index");
    git(&root, &["add", "-f", ".pitwall/project.locax.toml"]);
    let untracked = fs::read(&index).unwrap();
    git(
        &root,
        &["rm", "-q", "--cached", ".pitwall/project.locax.toml"],
    );
    git(&root, &["add", "-f", LOCAL]);
    let tracked = fs::read(&index).unwrap();
    assert_eq!(untracked.len(), tracked.len());
    assert_ne!(untracked, tracked);

    let overwrite = |bytes: &[u8], at: std::time::SystemTime| {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().write(true).open(&index).unwrap();
        file.write_all(bytes).unwrap();
        file.set_modified(at).unwrap();
    };
    let at = fs::metadata(&index).unwrap().modified().unwrap();
    overwrite(&untracked, at);
    let before = fs::metadata(&index).unwrap();
    assert_eq!(judge(&root), Tracking::Untracked);
    overwrite(&tracked, at);
    let after = fs::metadata(&index).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(before.ino(), after.ino());
    }
    assert_eq!(judge(&root), Tracking::Tracked);

    // 체크섬 없는 인덱스(`index.skipHash`)는 캐시하지 않는다 — 바뀌면 다시 읽는다.
    git(&root, &["config", "index.skipHash", "true"]);
    git(&root, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&root), Tracking::Untracked);
    git(&root, &["add", "-f", LOCAL]);
    assert_eq!(judge(&root), Tracking::Tracked);
}
