//! 미신뢰 프로젝트에서는 저장소의 설정이 코드를 돌리지 않는다(결정 M2-D8, 03-architecture.md 「보안」). 압축으로 받은 저장소처럼
//! `.git/config` 에 프로그램을 심고(`core.fsmonitor` · `filter.<x>.clean` · `diff.external` · `diff.<x>.textconv`, include 한 파일의
//! 필터), 읽기 경로마다 그 프로그램이 남기는 표식 파일이 생기지 않는지 본다. 신뢰하면 사람의 설정으로 다시 돈다 — 표식이 생긴다
//! (위의 「안 생긴다」 가 신뢰로 갈리는지).
//!
//! 읽기 경로: `git.repo`(gix), gix 로 연 저장소가 프로그램을 고르는 설정(이후 행의 status · diff 가 쓰는 필터 드라이버 · textconv),
//! CLI 읽기(`Access::Read`).

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::cli::{Cli, tokio_rt, which_git};
use super::*;
use crate::cli::Access;
use crate::testing::TestRepo;

/// 표식을 남기는 저장소: 심은 프로그램은 부를 때마다 `marks/<이름>` 에 한 줄을 더한다.
struct Planted {
    r: TestRepo,
    marks: PathBuf,
}

impl Planted {
    fn new() -> Self {
        let r = TestRepo::with_commit();
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
        // fsmonitor 훅이 실패하면 git 은 전부 훑는다 — 결과는 같고 표식만 남는다.
        let fsmonitor = script("fsmonitor", "exit 1");
        let clean = script("clean", "cat");
        let external = script("external", "exit 0");
        let textconv = script("textconv", "cat \"$1\"");
        let included = script("included", "cat");
        for (key, value) in [
            ("core.fsmonitor", fsmonitor.as_str()),
            ("filter.mark.clean", clean.as_str()),
            ("diff.external", external.as_str()),
            ("diff.mark.textconv", textconv.as_str()),
            ("include.path", "planted.cfg"),
        ] {
            r.git(&["config", key, value]);
        }
        fs::write(
            r.root().join(".git/planted.cfg"),
            format!("[filter \"inc\"]\n\tclean = {included}\n"),
        )
        .unwrap();
        fs::write(
            r.root().join(".git/info/attributes"),
            "* filter=mark diff=mark\n",
        )
        .unwrap();
        Self { r, marks }
    }

    /// 지금까지 남은 표식 이름.
    fn marks(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.marks)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// `README.md` 의 내용과 stat 을 바꾼다 — status 가 clean 필터를 돌리고 diff 가 외부 diff 를 부를 일이 생긴다.
    fn dirty(&self) {
        self.r.write("README.md", "changed\n");
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(self.r.root().join("README.md"))
            .unwrap()
            .set_modified(later)
            .unwrap();
    }
}

/// gix 가 프로그램을 고를 때 쓰는 값(이후 행의 status · diff 가 고르는 필터 드라이버 · textconv). 없으면 `None`.
fn programs(repo: &gix::Repository) -> Vec<Option<String>> {
    let config = repo.config_snapshot();
    [
        "filter.mark.clean",
        "filter.inc.clean",
        "diff.mark.textconv",
    ]
    .iter()
    .map(|key| {
        config
            .trusted_program(*key)
            .unwrap()
            .map(|p| p.to_string_lossy().into_owned())
    })
    .collect()
}

fn setup() -> (Planted, Cli, Target) {
    let p = Planted::new();
    let git = which_git();
    let c = Cli::with_path("", &git.parent().unwrap().display().to_string());
    let id = c.f.project(p.r.root());
    let target = Target {
        project_id: id,
        project_root: p.r.root().to_path_buf(),
        work_tree: p.r.root().to_path_buf(),
    };
    p.dirty();
    (p, c, target)
}

#[test]
fn an_untrusted_repository_runs_none_of_its_programs_on_reads() {
    let (p, c, target) = setup();
    let id = &target.project_id;

    // `git.repo`(gix).
    let got = c.f.repo(id, None).unwrap().repo.expect("a repository");
    assert_eq!(got.work_tree, p.r.root().display().to_string());

    // gix 로 연 저장소는 저장소 안의 설정(include 한 파일 포함)에서 프로그램을 고르지 않는다.
    let opened = c.f.git.open(id, None).unwrap().expect("a repository");
    assert_eq!(programs(&opened.repo), [None, None, None]);
    // gix 가 저장소를 완전히 믿어도(사람의 전역 설정 `safe.directory = *` 가 그렇게 만든다) 같다 — 판정이 소유자 · 신뢰 수준이 아니라
    // 설정의 자리로 갈린다.
    let full = gix::open_opts(
        p.r.root(),
        crate::repo::open_options(false).with(gix::sec::Trust::Full),
    )
    .unwrap();
    assert_eq!(full.git_dir_trust(), gix::sec::Trust::Full);
    assert_eq!(programs(&full), [None, None, None]);

    // CLI 읽기(status · diff)는 띄우지 않는다 — 명령을 만들기 전에 거절한다.
    let refused = tokio_rt().block_on(c.f.git.command(&target, Access::Read));
    assert_eq!(
        refused.unwrap_err(),
        CliError::Untrusted {
            project_id: id.clone()
        }
    );

    assert_eq!(p.marks(), Vec::<String>::new());
}

/// 신뢰하면 사람의 설정대로 돈다: gix 는 저장소의 필터 드라이버 · textconv 를 고르고, CLI 읽기가 fsmonitor · clean 필터 · 외부
/// diff 를 부른다.
#[test]
fn trusting_the_project_lets_its_repository_programs_run_again() {
    let (p, c, target) = setup();
    let id = &target.project_id;
    c.f.trust(id, true);

    let opened = c.f.git.open(id, None).unwrap().expect("a repository");
    let chosen = programs(&opened.repo);
    assert!(chosen.iter().all(Option::is_some), "{chosen:?}");

    let rt = tokio_rt();
    let run = |args: &[&str]| {
        rt.block_on(async {
            let command = c.f.git.command(&target, Access::Read).await.unwrap();
            let out = command
                .args(args)
                .output(&CancellationToken::new())
                .await
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        });
    };
    run(&["status", "--porcelain=v2", "-z"]);
    run(&["diff", "--name-only"]);
    run(&["diff"]);
    assert_eq!(p.marks(), ["clean", "external", "fsmonitor"]);
}

/// `--separate-git-dir` 로 만든 저장소 하나(`<base>/<name>` 작업 트리, `<base>/store/<name>.git` 저장소 폴더).
fn separate_git_dir(r: &TestRepo, name: &str) -> (PathBuf, PathBuf) {
    let store = r.base().join(format!("store/{name}.git"));
    let proj = r.base().join(name);
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
    (proj, store)
}

fn target_of(id: &str, root: &Path) -> Target {
    Target {
        project_id: id.to_owned(),
        project_root: root.to_path_buf(),
        work_tree: root.to_path_buf(),
    }
}

/// `--separate-git-dir` 저장소도 신뢰로 갈린다: 미신뢰면 CLI 읽기를 거절하고, 신뢰하면 만든다.
#[test]
fn a_separate_git_dir_repository_reads_with_the_cli_only_when_trusted() {
    let r = TestRepo::new();
    let (proj, _store) = separate_git_dir(&r, "proj");
    let git = which_git();
    let c = Cli::with_path("", &git.parent().unwrap().display().to_string());
    let id = c.f.project(&proj);
    let opened = c.f.git.open(&id, None).unwrap().expect("a repository");
    assert_eq!(opened.target.work_tree, proj);
    let rt = tokio_rt();
    let refused = rt.block_on(c.f.git.command(&opened.target, Access::Read));
    assert_eq!(
        refused.unwrap_err(),
        CliError::Untrusted {
            project_id: id.clone()
        }
    );
    c.f.trust(&id, true);
    let opened = c.f.git.open(&id, None).unwrap().expect("a repository");
    rt.block_on(async {
        let out =
            c.f.git
                .command(&opened.target, Access::Read)
                .await
                .unwrap()
                .args(["rev-parse", "--show-toplevel"])
                .output(&CancellationToken::new())
                .await
                .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            proj.display().to_string()
        );
    });
}

/// 남의 `--separate-git-dir` 저장소 폴더를 가리키는 `.git` 파일은 git 이 되돌림을 남기지 않아 가를 수 없다 — 그 폴더의 저장소로
/// 받지만 신뢰는 그 폴더(프로젝트 경로)의 것이다. 저장소 주인 프로젝트를 신뢰해도 이 폴더는 미신뢰로 남아, CLI 로 읽지 않고 gix 도
/// 그 저장소 설정의 프로그램을 고르지 않는다.
#[test]
fn a_git_file_borrowing_a_separate_git_dir_does_not_borrow_its_trust() {
    let r = TestRepo::new();
    let (owner, store) = separate_git_dir(&r, "owner");
    r.git_in(&owner, &["config", "filter.mark.clean", "planted-clean"]);
    let evil = r.base().join("downloads/evil");
    fs::create_dir_all(&evil).unwrap();
    fs::write(evil.join(".git"), format!("gitdir: {}\n", store.display())).unwrap();
    let git = which_git();
    let c = Cli::with_path("", &git.parent().unwrap().display().to_string());
    let owner_id = c.f.project(&owner);
    c.f.trust(&owner_id, true);
    let id = c.f.project(&evil);
    assert!(!c.f.projects.is_trusted(&id));

    let opened = c.f.git.open(&id, None).unwrap().expect("a repository");
    assert_eq!(opened.target.work_tree, evil);
    let chosen = opened
        .repo
        .config_snapshot()
        .trusted_program("filter.mark.clean")
        .unwrap()
        .map(|p| p.to_string_lossy().into_owned());
    assert_eq!(chosen, None);
    let refused = tokio_rt().block_on(c.f.git.command(&target_of(&id, &evil), Access::Read));
    assert_eq!(
        refused.unwrap_err(),
        CliError::Untrusted {
            project_id: id.clone()
        }
    );
    // 주인 프로젝트에서는 같은 설정이 프로그램을 고른다(위의 `None` 이 신뢰로 갈린다).
    let owned =
        c.f.git
            .open(&owner_id, None)
            .unwrap()
            .expect("a repository");
    assert!(
        owned
            .repo
            .config_snapshot()
            .trusted_program("filter.mark.clean")
            .unwrap()
            .is_some()
    );
}

/// 받아 둔 핸들은 열 때의 신뢰를 들고 있다 — 신뢰를 바꾸면 [`Git::is_current`] 가 다시 열 때라고 알린다(신뢰 · 해제 둘 다).
#[test]
fn an_opened_repository_is_stale_once_the_trust_changes() {
    let (_p, c, target) = setup();
    let id = &target.project_id;
    let opened = c.f.git.open(id, None).unwrap().expect("a repository");
    assert!(!opened.trusted);
    assert!(c.f.git.is_current(&opened));
    c.f.trust(id, true);
    assert!(!c.f.git.is_current(&opened));
    // 옛 핸들은 여전히 미신뢰의 설정이다 — 다시 열어야 사람의 설정이 먹는다.
    assert_eq!(programs(&opened.repo), [None, None, None]);
    let reopened = c.f.git.open(id, None).unwrap().expect("a repository");
    assert!(c.f.git.is_current(&reopened));
    assert!(programs(&reopened.repo).iter().all(Option::is_some));
    c.f.trust(id, false);
    assert!(!c.f.git.is_current(&reopened));
}
