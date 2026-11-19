//! 추적 판정 시험. 저장소는 실제 `git` 으로 만든다(인덱스 꼴 · 워크트리 · 서브모듈 · bare 를 손으로 흉내 내지 않는다).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;

/// [`git`] 과 같되 실패해도 멈추지 않고 성공 여부를 돌려준다(일부러 실패시키는 checkout).
#[cfg(unix)]
pub(crate) fn git_may_fail(dir: &Path, args: &[&str]) -> bool {
    command(dir)
        .args(args)
        .output()
        .expect("run git")
        .status
        .success()
}

/// 사람의 git 설정과 떨어진 git 명령(`pw_git::testing::git_command` 와 같은 뜻 — pw-git 이 이 크레이트에 의존해 그것을 쓸 수
/// 없다). 시험이 git 훅 안에서 돌 때 남는 `GIT_DIR` · `GIT_INDEX_FILE` 같은 변수는 지운다.
pub(crate) fn git(dir: &Path, args: &[&str]) {
    let out = command(dir).args(args).output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn command(dir: &Path) -> Command {
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
    c.env("GIT_CONFIG_NOSYSTEM", "1")
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
            "-c",
            "uploadpack.allowFilter=true",
        ]);
    c
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

/// git 이 아직 본 적 없는 내용으로 쓴다 — 판정이 인덱스만으로 갈리게(내용 판정은 「객체 DB 에 없음」).
pub(crate) fn mine(path: &Path) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    put(
        path,
        &format!(
            "schema = 1\n# personal {n} {:?}\n",
            std::time::SystemTime::now()
        ),
    );
}

fn unknown(t: Tracking) -> bool {
    matches!(t, Tracking::Unknown(_))
}

#[test]
fn outside_a_repository_and_in_a_fresh_one_nothing_is_tracked() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    mine(&plain.join(LOCAL));
    assert_eq!(judge(&plain), Tracking::Untracked);

    // 갓 `git init` — 인덱스도 커밋도 없다.
    let fresh = tmp.path().join("fresh");
    mine(&fresh.join(LOCAL));
    git(&fresh, &["init", "-q"]);
    assert!(!fresh.join(".git/index").exists());
    assert_eq!(judge(&fresh), Tracking::Untracked);

    // 다른 파일만 추적하면 개인 파일은 사람의 것이다. 강제로 더하면(커밋 전이어도 인덱스에 있으면) 추적이다.
    let root = repo(&tmp.path().join("repo"));
    mine(&root.join(LOCAL));
    assert_eq!(judge(&root), Tracking::Untracked);
    git(&root, &["add", "-f", LOCAL]);
    assert_eq!(judge(&root), Tracking::Tracked);
    git(&root, &["commit", "-q", "-m", "commit the local file"]);
    mine(&root.join(LOCAL));
    assert_eq!(judge(&root), Tracking::Tracked, "the index decides");
    // 아직 없는 하위 폴더 파일(쓰기 전 판정)도 된다.
    assert_eq!(
        tracking(&root, &root.join(".pitwall/db/pg.local.toml")),
        Tracking::Untracked
    );
    // 추적을 풀고 고치면 사람의 것이다.
    git(&root, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&root), Tracking::Untracked);
}

/// 오탐의 방향: git 이 한 번이라도 저장한 내용과 같은 파일은 인덱스에서 빠져도 커밋된 내용으로 본다(실수로 `add` 했다가
/// `rm --cached` 한 직후) — 좁히는 쪽이고, 고치면 풀린다.
#[test]
fn content_git_stored_reads_as_committed_until_it_is_edited() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"));
    mine(&root.join(LOCAL));
    git(&root, &["add", "-f", LOCAL]);
    git(&root, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&root), Tracking::Fetched);
    mine(&root.join(LOCAL));
    assert_eq!(judge(&root), Tracking::Untracked);
    // 커밋된 `project.toml` 과 바이트가 같은 개인 파일도 커밋된 내용으로 본다.
    put(&root.join(LOCAL), "schema = 1\n");
    assert_eq!(judge(&root), Tracking::Fetched);
}

/// 프로젝트 루트가 저장소의 (대문자가 든) 하위 폴더여도 저장소 기준 경로로 본다. 대소문자만 다른 항목도 추적으로 본다(대소문자를
/// 가리지 않는 파일 시스템에서 같은 파일이다 — 틀려도 받지 않는 쪽이다).
#[test]
fn a_project_in_a_subfolder_and_case_differences_are_judged_by_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"));
    let sub = root.join("Apps/Web");
    mine(&sub.join(LOCAL));
    assert_eq!(judge(&sub), Tracking::Untracked);
    git(
        &root,
        &["add", "-f", "Apps/Web/.pitwall/project.local.toml"],
    );
    mine(&sub.join(LOCAL));
    assert_eq!(judge(&sub), Tracking::Tracked);

    mine(&root.join(".pitwall/DB/PG.local.toml"));
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
    mine(&root.join("conf/project.local.toml"));
    put(&root.join("conf/.gitignore"), "*.local.toml\n");
    git(&root, &["add", ".pitwall", "conf/.gitignore"]);
    // `conf/project.local.toml` 은 무시되어 추적하지 않지만, 그것에 닿는 `.pitwall` 이 커밋된 링크다.
    assert_eq!(judge(&root), Tracking::Tracked);

    let other = tmp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    mine(&other.join("conf/project.local.toml"));
    git(&other, &["add", "conf/project.local.toml"]);
    mine(&other.join("conf/project.local.toml"));
    std::os::unix::fs::symlink("conf", other.join(".pitwall")).unwrap();
    // `.pitwall` 링크는 추적하지 않지만 닿는 파일이 추적 파일이다.
    assert_eq!(judge(&other), Tracking::Tracked);
}

/// 연결 워크트리(`.git` 파일 → `<main>/.git/worktrees/<이름>`)는 자기 인덱스로 판정한다.
#[test]
fn a_linked_worktree_is_judged_by_its_own_index() {
    let tmp = tempfile::tempdir().unwrap();
    let main = repo(&tmp.path().join("main"));
    mine(&main.join(LOCAL));
    git(&main, &["add", "-f", LOCAL]);
    git(&main, &["commit", "-q", "-m", "local"]);
    let wt = tmp.path().join("wt");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    assert!(wt.join(".git").is_file());
    mine(&wt.join(LOCAL));
    assert_eq!(judge(&wt), Tracking::Tracked);
    // 메인에서만 추적을 풀면 워크트리의 인덱스에는 아직 있다.
    git(&main, &["rm", "-q", "--cached", LOCAL]);
    mine(&main.join(LOCAL));
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
    mine(&upstream.join(LOCAL));
    git(&upstream, &["add", "-f", LOCAL]);
    git(&upstream, &["commit", "-q", "-m", "local"]);

    let outer = repo(&tmp.path().join("outer"));
    git(
        &outer,
        &["submodule", "add", "-q", upstream.to_str().unwrap(), "sub"],
    );
    let sub = outer.join("sub");
    assert!(sub.join(".git").is_file());
    mine(&sub.join(LOCAL));
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
    mine(&wt.join(LOCAL));
    assert_eq!(judge(&wt), Tracking::Tracked);
    git(&wt, &["rm", "-q", "--cached", LOCAL]);
    assert_eq!(judge(&wt), Tracking::Untracked);
}

/// split index(`core.splitIndex` — 공유 인덱스 `sharedindex.<해시>` 와 나눠 적는다)도 푼다.
#[test]
fn a_split_index_is_read_whole() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"));
    mine(&root.join(LOCAL));
    git(&root, &["config", "core.splitIndex", "true"]);
    git(&root, &["update-index", "--split-index"]);
    git(&root, &["add", "-f", LOCAL]);
    let shared = fs::read_dir(root.join(".git"))
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().starts_with("sharedindex."));
    assert!(shared, "git did not split the index");
    mine(&root.join(LOCAL));
    assert_eq!(judge(&root), Tracking::Tracked);
    // 공유 인덱스에만 있는 항목(나눈 뒤에 바뀌지 않은 것)도 본다.
    git(&root, &["update-index", "--split-index"]);
    mine(&root.join(".pitwall/project.toml"));
    assert_eq!(
        tracking(&root, &root.join(".pitwall/project.toml")),
        Tracking::Tracked
    );
}

/// sparse index: 콘 밖 폴더는 인덱스에 폴더 항목 하나(`.pitwall/`)로 남는다 — 그 아래 파일도 추적이다.
#[test]
fn a_sparse_folder_entry_covers_the_files_under_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"));
    mine(&root.join(LOCAL));
    put(&root.join("src/a.txt"), "a\n");
    git(&root, &["add", "-f", LOCAL, "src/a.txt"]);
    git(&root, &["commit", "-q", "-m", "files"]);
    git(
        &root,
        &["sparse-checkout", "init", "--cone", "--sparse-index"],
    );
    git(&root, &["sparse-checkout", "set", "src"]);
    assert!(!root.join(LOCAL).exists(), "git kept the folder");
    // 파일이 없으니 내용 판정은 없다 — 인덱스의 폴더 항목만으로 추적이다.
    assert_eq!(judge(&root), Tracking::Tracked);
}

/// checkout 이 중간에 실패하면 커밋된 개인 파일은 디스크에 있고 인덱스에는 없다(리뷰 F1). 실제 git 으로 실패시킨다 — 이름 순으로
/// `.pitwall/` 뒤에 오는 파일에 필수 smudge 필터가 실패한다(git-lfs 의 받기 실패와 같은 꼴).
#[cfg(unix)]
#[test]
fn a_failed_clone_pull_or_checkout_does_not_leave_a_personal_file() {
    let tmp = tempfile::tempdir().unwrap();
    const FAILING: [&str; 4] = [
        "-c",
        "filter.boom.smudge=false",
        "-c",
        "filter.boom.required=true",
    ];
    let with_failing = |args: &[&str]| -> Vec<String> {
        FAILING
            .iter()
            .chain(args)
            .map(|s| (*s).to_owned())
            .collect()
    };
    let run = |dir: &Path, args: &[&str]| {
        let args = with_failing(args);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        git_may_fail(dir, &args)
    };
    let widening = "schema = 1\n[mcp]\nauto_approve = [\"read\", \"write\", \"exec\"]\n";
    let upstream = repo(&tmp.path().join("upstream"));
    put(&upstream.join(".gitattributes"), "zz* filter=boom\n");
    git(&upstream, &["add", ".gitattributes"]);
    git(&upstream, &["commit", "-q", "-m", "attributes"]);
    let base = tmp.path().join("victim");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            base.to_str().unwrap(),
        ],
    );
    put(&upstream.join(LOCAL), widening);
    put(&upstream.join("zzz"), "z\n");
    git(&upstream, &["add", "-f", LOCAL, "zzz"]);
    git(&upstream, &["commit", "-q", "-m", "evil"]);
    git(&upstream, &["branch", "evil"]);

    // 실패한 클론: 인덱스가 없고 HEAD 는 커밋을 가리킨다 — 「모름」.
    let cloned = tmp.path().join("cloned");
    assert!(!run(
        tmp.path(),
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            cloned.to_str().unwrap()
        ]
    ));
    assert_eq!(fs::read_to_string(cloned.join(LOCAL)).unwrap(), widening);
    let t = judge(&cloned);
    assert!(!t.is_personal(), "{t:?}");

    // 실패한 pull: 인덱스 · HEAD 는 옛 것이고 파일 내용은 받아 온 객체다 — 받아 온 내용.
    let pulled = tmp.path().join("pulled");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            base.to_str().unwrap(),
            pulled.to_str().unwrap(),
        ],
    );
    git(
        &pulled,
        &["remote", "set-url", "origin", upstream.to_str().unwrap()],
    );
    assert!(!run(
        &pulled,
        &["pull", "-q", "--no-rebase", "origin", "main"]
    ));
    assert_eq!(fs::read_to_string(pulled.join(LOCAL)).unwrap(), widening);
    assert_eq!(judge(&pulled), Tracking::Fetched);
    // Pitwall 도 그 파일에 쓰지 않는다 — 키 하나를 쓰면 남은 넓히는 값까지 사람의 설정이 된다.
    assert!(
        crate::config::ConfigSource::project(&pulled)
            .prepare_local_write(&pulled.join(LOCAL))
            .is_err()
    );

    // 받아 둔 다른 브랜치로 바꾸다 실패.
    let switched = tmp.path().join("switched");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            base.to_str().unwrap(),
            switched.to_str().unwrap(),
        ],
    );
    git(
        &switched,
        &["remote", "set-url", "origin", upstream.to_str().unwrap()],
    );
    git(&switched, &["fetch", "-q", "origin"]);
    assert!(!run(
        &switched,
        &["checkout", "-q", "-b", "evil", "origin/evil"]
    ));
    assert_eq!(fs::read_to_string(switched.join(LOCAL)).unwrap(), widening);
    assert_eq!(judge(&switched), Tracking::Fetched);
}

/// 판정을 못 하면 「모름」 — 부른 쪽은 사람의 레이어로 받지 않는다.
#[test]
fn an_unreadable_repository_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();

    // 깨진 인덱스.
    let broken = repo(&tmp.path().join("broken"));
    mine(&broken.join(LOCAL));
    fs::write(broken.join(".git/index"), b"DIRC garbage").unwrap();
    assert!(unknown(judge(&broken)), "{:?}", judge(&broken));

    // 아무 데도 가리키지 않는 `.git` 파일.
    let dangling = tmp.path().join("dangling");
    mine(&dangling.join(LOCAL));
    fs::write(dangling.join(".git"), "gitdir: /nowhere/at/all\n").unwrap();
    assert!(unknown(judge(&dangling)));

    // SHA-256 저장소(이 앱의 gitoxide 는 SHA-1 만 읽는다).
    let sha256 = repo(&tmp.path().join("sha256"));
    mine(&sha256.join(LOCAL));
    let config = sha256.join(".git/config");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        format!("{text}[extensions]\n\tobjectformat = sha256\n"),
    )
    .unwrap();
    assert!(unknown(judge(&sha256)));

    // `core.worktree` 가 다른 폴더를 가리키는 `.git` 폴더(인덱스의 경로가 다른 작업 트리 기준이다).
    let moved = repo(&tmp.path().join("moved"));
    mine(&moved.join(LOCAL));
    git(&moved, &["config", "core.worktree", "/somewhere/else"]);
    assert!(unknown(judge(&moved)));

    // 인덱스가 없는데 HEAD 가 커밋을 가리킨다(실패한 클론의 꼴).
    let no_index = repo(&tmp.path().join("no-index"));
    mine(&no_index.join(LOCAL));
    fs::remove_file(no_index.join(".git/index")).unwrap();
    assert!(unknown(judge(&no_index)), "{:?}", judge(&no_index));
    // 브랜치가 `packed-refs` 에만 있어도 HEAD 는 커밋을 가리킨다.
    let packed = repo(&tmp.path().join("packed"));
    mine(&packed.join(LOCAL));
    git(&packed, &["pack-refs", "--all"]);
    assert!(!packed.join(".git/refs/heads/main").exists());
    fs::remove_file(packed.join(".git/index")).unwrap();
    assert!(unknown(judge(&packed)), "{:?}", judge(&packed));
    // 갓 init(태어나지 않은 브랜치)은 인덱스가 없어도 「모름」 이 아니다 — 위 `outside_…` 시험.

    // 대체 객체 저장소(alternates)가 없는 곳을 가리킨다.
    let alternates = repo(&tmp.path().join("alternates"));
    mine(&alternates.join(LOCAL));
    put(
        &alternates.join(".git/objects/info/alternates"),
        "/nowhere/objects\n",
    );
    assert!(unknown(judge(&alternates)));
}

/// 객체가 다 있다고 할 수 없는 저장소(shallow · partial clone)에서 인덱스에 없는 개인 파일은 「모름」. 인덱스에 있으면 추적이다.
#[test]
fn shallow_and_partial_clones_cannot_clear_a_file_by_its_content() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = repo(&tmp.path().join("upstream"));
    put(&upstream.join("a.txt"), "a\n");
    git(&upstream, &["add", "a.txt"]);
    git(&upstream, &["commit", "-q", "-m", "second"]);
    let url = format!("file://{}", upstream.display());
    for (name, flag) in [("shallow", "--depth=1"), ("partial", "--filter=blob:none")] {
        let clone = tmp.path().join(name);
        git(
            tmp.path(),
            &["clone", "-q", flag, &url, clone.to_str().unwrap()],
        );
        mine(&clone.join(LOCAL));
        assert!(unknown(judge(&clone)), "{name}: {:?}", judge(&clone));
        git(&clone, &["add", "-f", LOCAL]);
        assert_eq!(judge(&clone), Tracking::Tracked, "{name}");
        // 아직 없는 파일(쓰기 전 판정)은 내용 판정이 없다.
        assert_eq!(
            tracking(&clone, &clone.join(".pitwall/db/x.local.toml")),
            Tracking::Untracked,
            "{name}"
        );
    }
}

/// 읽을 수 없는 인덱스 · 객체 폴더는 「모름」 이다(인덱스 없음 · 객체 없음으로 읽으면 넓어진다).
#[cfg(unix)]
#[test]
fn unreadable_index_or_objects_are_unknown() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    // 커밋이 없는 저장소 — 인덱스를 「없음」 으로 읽으면 HEAD 판정도 막지 못한다.
    let root = tmp.path().join("repo");
    put(&root.join("a.txt"), "a\n");
    git(&root, &["init", "-q"]);
    git(&root, &["add", "a.txt"]);
    mine(&root.join(LOCAL));
    let lock = |path: &Path, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    let index = root.join(".git/index");
    lock(&index, 0o000);
    if fs::read(&index).is_ok() {
        // root 로 돌면 권한이 막지 않는다.
        lock(&index, 0o644);
        return;
    }
    let t = judge(&root);
    lock(&index, 0o644);
    assert!(unknown(t.clone()), "{t:?}");

    let objects = root.join(".git/objects");
    lock(&objects, 0o000);
    let t = judge(&root);
    lock(&objects, 0o755);
    assert!(unknown(t.clone()), "{t:?}");
    assert_eq!(judge(&root), Tracking::Untracked);
}

/// 캐시는 인덱스 내용의 체크섬으로 맞춘다 — 크기 · 수정 시각 · inode 가 같은 제자리 덮어쓰기(같은 초 안 두 번 쓰기)도 놓치지 않는다.
/// 판정이 낡은 「추적 안 함」 에 머물면 커밋된 파일이 사람의 설정으로 넓어진다. 판정할 파일은 디스크에서 지워 인덱스만으로 갈리게 한다.
#[test]
fn the_cache_follows_the_index_content_not_its_size_time_or_inode() {
    let tmp = tempfile::tempdir().unwrap();
    let root = repo(&tmp.path().join("repo"));
    put(&root.join(LOCAL), "schema = 1\n# a\n");
    // 길이가 같은 다른 이름을 추적한 인덱스와, 개인 파일을 추적한 인덱스.
    put(
        &root.join(".pitwall/project.locax.toml"),
        "schema = 1\n# a\n",
    );
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
    fs::remove_file(root.join(LOCAL)).unwrap();

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
    put(&root.join(LOCAL), "schema = 1\n# a\n");
    git(&root, &["add", "-f", LOCAL]);
    fs::remove_file(root.join(LOCAL)).unwrap();
    assert_eq!(judge(&root), Tracking::Tracked);
}
