//! 설정 창 「Git」: 찾은 버전이 `git --version` 과 같다, 2.40 미만 · 읽지 못함 · 찾지 못함이 쓰기를 끈 이유로 나온다(쓰기 판정과
//! 같다), 띄우는 것은 고정 argv `git --version` 하나이고 작업 디렉터리는 어느 프로젝트 안도 아니다.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry};
use pw_runtime::{LoginShell, Runtime};

use super::*;
use crate::cli::GitVersion;
use crate::testing::FakeGit;
use crate::testing::write::{fake_login_shell, which_git};
use crate::{CliError, HostParts, Target};

struct Fixture {
    git: Git,
    projects: Registry,
    bin: PathBuf,
    dir: tempfile::TempDir,
}

impl Fixture {
    /// `settings` 는 전역 `settings.toml`. 로그인 셸의 PATH 는 `<bin>:<진짜 git 의 폴더>:/usr/bin:/bin`.
    fn new(settings: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let shell = fake_login_shell(dir.path(), &bin);
        let paths = AppPaths::under(&dir.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(paths.config.join("settings.toml"), settings).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let projects = Registry::new(paths, None, bus.clone());
        let runtime = Runtime::new(LoginShell::new(Some(shell), Duration::from_secs(10)));
        let git = Git::new(Arc::new(runtime));
        git.attach(HostParts {
            global,
            projects: projects.clone(),
            bus,
        });
        Self {
            git,
            projects,
            bin,
            dir,
        }
    }

    fn info(&self) -> GitExecutable {
        tokio_rt().block_on(self.git.executable())
    }

    /// `root` 를 프로젝트로 등록한 쓰기 대상.
    fn target(&self, root: &Path) -> Target {
        fs::create_dir_all(root).unwrap();
        let id = self
            .projects
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        Target {
            project_id: id,
            project_root: root.to_path_buf(),
            work_tree: root.to_path_buf(),
        }
    }
}

fn tokio_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn the_version_is_the_one_git_version_prints() {
    let f = Fixture::new("");
    let real = which_git();
    let out = std::process::Command::new(&real)
        .arg("--version")
        .output()
        .unwrap();
    let expected = GitVersion::parse(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert!(
        expected >= MIN_VERSION,
        "this machine's git ({expected}) is older than {MIN_VERSION} — the test needs a git that can write"
    );

    let info = f.info();
    assert_eq!(info.version, Some(expected.to_string()));
    assert_eq!(info.program, Some(real.display().to_string()));
    assert_eq!(info.configured, None);
    assert_eq!(info.minimum, "2.40");
    assert_eq!(info.writes_off, None);
    assert!(
        matches!(info.path_source, RuntimeSource::LoginShell { .. }),
        "{:?}",
        info.path_source
    );
    assert_eq!(info.shell_failure, None);
}

/// 2.40 미만이면 꺼짐과 그 버전 — 쓰기 실행기도 같은 판정으로 거절한다(같은 캐시).
#[test]
fn an_old_git_shows_why_writes_are_off_as_the_writes_do() {
    let f = Fixture::new("");
    let fake = FakeGit::new(&f.bin, "git", "2.39.5");
    let info = f.info();
    assert_eq!(info.program, Some(fake.path().display().to_string()));
    assert_eq!(info.version.as_deref(), Some("2.39.5"));
    assert_eq!(
        info.writes_off,
        Some(GitWritesOff::TooOld {
            version: "2.39.5".into()
        })
    );
    let target = f.target(&f.dir.path().join("project"));
    let refused = tokio_rt()
        .block_on(f.git.command(&target, Access::Write))
        .unwrap_err();
    assert_eq!(
        refused,
        CliError::TooOld {
            program: fake.path().to_path_buf(),
            version: "2.39.5".into()
        }
    );
}

#[test]
fn a_version_that_cannot_be_read_turns_writes_off() {
    let f = Fixture::new("");
    script(&f.bin.join("git"), "echo 'hub version 2.14'");
    let info = f.info();
    assert_eq!(info.version, None);
    assert!(
        matches!(&info.writes_off, Some(GitWritesOff::Unreadable { reason }) if reason.contains("hub version")),
        "{:?}",
        info.writes_off
    );
}

#[test]
fn a_configured_path_that_does_not_exist_is_not_found() {
    let f = Fixture::new("[git]\npath = \"/nonexistent/pitwall-git\"\n");
    let info = f.info();
    assert_eq!(info.configured.as_deref(), Some("/nonexistent/pitwall-git"));
    assert_eq!(info.program, None);
    assert_eq!(info.version, None);
    assert_eq!(
        info.writes_off,
        Some(GitWritesOff::NotFound {
            program: "/nonexistent/pitwall-git".into()
        })
    );
}

#[test]
fn a_configured_path_is_used() {
    let other = tempfile::tempdir().unwrap();
    let fake = FakeGit::new(other.path(), "my-git", "2.45.1");
    let f = Fixture::new(&format!("[git]\npath = \"{}\"\n", fake.path().display()));
    let info = f.info();
    assert_eq!(info.configured, Some(fake.path().display().to_string()));
    assert_eq!(info.program, Some(fake.path().display().to_string()));
    assert_eq!(info.version.as_deref(), Some("2.45.1"));
    assert_eq!(info.writes_off, None);
}

/// Host 가 이 RPC 로 프로젝트 안 프로세스를 띄울 수 없다: argv 는 `--version` 하나, 작업 디렉터리는 어느 프로젝트의 루트 아래도
/// 아니다(프로젝트가 등록돼 있어도).
#[test]
fn only_git_version_runs_and_never_inside_a_project() {
    let f = Fixture::new("");
    let project = f.target(&f.dir.path().join("project"));
    let record = f.dir.path().join("record");
    script(
        &f.bin.join("git"),
        &format!(
            "printf '%s\\n' \"$@\" >> '{r}.argv'\npwd -P >> '{r}.cwd'\necho 'git version 2.50.0'",
            r = record.display()
        ),
    );
    let info = f.info();
    assert_eq!(info.version.as_deref(), Some("2.50.0"));
    assert_eq!(
        fs::read_to_string(record.with_extension("argv")).unwrap(),
        "--version\n"
    );
    let cwd = PathBuf::from(
        fs::read_to_string(record.with_extension("cwd"))
            .unwrap()
            .trim(),
    );
    assert_eq!(cwd, neutral_dir());
    let root = fs::canonicalize(&project.project_root).unwrap();
    assert!(
        !cwd.starts_with(&root),
        "{} is inside {}",
        cwd.display(),
        root.display()
    );
}
